//! One-session planning for the audited JVM/TASTy class-state overlap.
//!
//! The coordinator is deliberately callback-scoped: both producer plans borrow
//! one frontend session, so callers inspect or consume the validated plans
//! while that session is alive. This module never emits bytes or writes files.

use std::error::Error;
use std::fmt;

use dotty_core::ast::{TreeKind, Untyped, VisibilitySyntax};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SourceTypedIndex, SymbolFlags,
    SymbolId, SymbolInfo, SymbolKind, TreeId,
};
use dotty_jvm::{
    JvmClassInitializationPlan, JvmClassPlan, JvmCompilationUnit, JvmFieldPlan,
    JvmFieldSourceVisibility, JvmMethodPlan, JvmPrimaryConstructorPlan,
};
use dotty_jvm_tasty_contract::{JvmTastyContractError, ValidatedJvmTastyClass};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty_pickler::{
    ClassFieldPicklePlan, ClassMemberPicklePlan, ClassStatePicklePlan, PickleError,
    PrimaryConstructorInputPlan, SourceMetadata, SourceTypedBindings, ValidatedClassStatePlan,
};
use dotty_typer::{SourceTyper, TyperError};

/// Errors are tagged by the frontend or producer stage that rejected the
/// source. The debug-form payload retains the stage's exact diagnostic data.
#[derive(Debug)]
pub enum PairedPlanError {
    SourceText(String),
    Lexing(String),
    Parsing(String),
    Naming(String),
    UnsupportedSourceShape(&'static str),
    Typer(TyperError),
    Jvm(dotty_jvm::JvmCodegenError),
    Tasty(PickleError),
    Contract(JvmTastyContractError),
    /// A batch member was rejected at a specific source declaration and
    /// planning phase. The batch API never returns earlier successful plans.
    BatchClass {
        class_index: usize,
        declaration_index: usize,
        phase: &'static str,
        reason: String,
    },
    /// A source unit cannot be represented by the bounded batch planner.
    BatchShape(&'static str),
    /// A validation failure applies to the complete source batch rather than
    /// to one class plan, so it cannot be attributed to a class receipt.
    BatchUnit {
        phase: &'static str,
        reason: String,
    },
}

impl fmt::Display for PairedPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot build paired JVM/TASTy plans: {self:?}")
    }
}

impl Error for PairedPlanError {}

/// Validated, borrowed plans for one class from a single canonical source
/// session. Its references are valid only inside
/// [`with_paired_jvm_tasty_class_plan`]'s callback.
pub struct PairedJvmTastyClassPlan<'session> {
    /// The complete producer input for JVM lowering.
    pub jvm_unit: &'session JvmCompilationUnit<'session>,
    /// The complete producer input for semantic TASTy pickling.
    pub tasty_plan: &'session ValidatedClassStatePlan<'session, 'session>,
    /// Cross-producer proof over the exact borrowed plans above.
    pub compatibility: ValidatedJvmTastyClass<'session, 'session, 'session, 'session>,
    /// Canonical source class symbol.
    pub class: SymbolId,
    /// Canonical source primary-constructor symbol.
    pub constructor: SymbolId,
    /// Frontend source identity for this borrowed session.
    pub source_id: SourceId,
    /// Field and method symbols in source declaration order.
    pub source_member_order: Vec<SymbolId>,
    /// Relative artifact paths intended by the validated plans.
    pub classfile_path: String,
    pub tasty_path: String,
}

/// One class's receipt within an ordered multi-class planning batch.
///
/// Its proof and TASTy plan are borrowed from the same callback-scoped batch
/// session and are in the same index as [`PairedJvmTastyBatchPlan::jvm_unit`.
pub struct PairedJvmTastyBatchClass<'batch> {
    /// Class-scoped proof validated against the complete shared JVM unit.
    pub compatibility: &'batch ValidatedJvmTastyClass<'batch, 'batch, 'batch, 'batch>,
    /// Complete class-scoped producer input for semantic TASTy pickling.
    pub tasty_plan: &'batch ValidatedClassStatePlan<'batch, 'batch>,
    /// Canonical source class symbol.
    pub class: SymbolId,
    /// Canonical source primary-constructor symbol.
    pub constructor: SymbolId,
    /// Canonical fields and methods in source declaration order.
    pub source_member_order: Vec<SymbolId>,
    /// Relative class-file path intended by the validated plans.
    pub classfile_path: String,
    /// Relative per-class TASTy path established by issue #1652.
    pub tasty_path: String,
}

/// Ordered read-only batch view over one real source frontend session.
pub struct PairedJvmTastyBatchPlan<'batch> {
    /// One complete JVM unit containing every source class in source order.
    pub jvm_unit: &'batch JvmCompilationUnit<'batch>,
    /// One class-scoped TASTy and cross-ABI receipt per source class, ordered
    /// identically to `jvm_unit.classes`.
    pub classes: &'batch [PairedJvmTastyBatchClass<'batch>],
    /// Source identity shared by every class in this compilation unit.
    pub source_id: SourceId,
}

struct BatchClassSource {
    tree: TreeId<Untyped>,
    class: SymbolId,
    constructor_tree: TreeId<Untyped>,
    constructor: SymbolId,
    constructor_parameters: Vec<Vec<SymbolId>>,
}

struct BatchSourceTypedBindings<'a> {
    index: &'a SourceTypedIndex,
    planned_classes: Vec<SymbolId>,
}

impl SourceTypedBindings for BatchSourceTypedBindings<'_> {
    fn typed_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<TreeId<dotty_core::Typed>> {
        self.index.get(source, tree)
    }

    fn planned_class_symbols(&self) -> &[SymbolId] {
        &self.planned_classes
    }
}

struct BatchClassDraft {
    source: BatchClassSource,
    tasty_members: Vec<ClassMemberPicklePlan>,
    jvm_plan: JvmClassPlan,
    source_member_order: Vec<SymbolId>,
}

/// Parse, name and type `source_text` once, construct both producer plans from
/// the resulting canonical identities, validate them independently, and prove
/// their class/member ABI agrees. The callback keeps the borrowed session alive
/// for the entire planning receipt lifetime.
pub fn with_paired_jvm_tasty_class_plan<R>(
    source_text: &str,
    source_name: &str,
    source_id: SourceId,
    consume: impl for<'session> FnOnce(PairedJvmTastyClassPlan<'session>) -> R,
) -> Result<R, PairedPlanError> {
    let source = SourceText::new(source_text)
        .map_err(|error| PairedPlanError::SourceText(format!("{error:?}")))?;
    let scanner = ContextualScanner::new(source_text)
        .map_err(|error| PairedPlanError::Lexing(format!("{error:?}")))?;
    if !scanner.diagnostics().is_empty() {
        return Err(PairedPlanError::Lexing(format!(
            "{:?}",
            scanner.diagnostics()
        )));
    }

    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source, source_id, scanner, &mut store.names);
    if !parsed.diagnostics.is_empty() {
        return Err(PairedPlanError::Parsing(format!(
            "{:?}",
            parsed.diagnostics
        )));
    }
    let mut packages = Packages::new();
    let index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        source_name,
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .map_err(|error| PairedPlanError::Naming(format!("{error:?}")))?
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => return Err(PairedPlanError::Naming(format!("{outcome:?}"))),
    };

    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        return Err(PairedPlanError::UnsupportedSourceShape(
            "one package root containing one ordinary class",
        ));
    };
    let [class_tree] = package.stats.as_slice() else {
        return Err(PairedPlanError::UnsupportedSourceShape(
            "exactly one top-level class; multi-class source units are not supported",
        ));
    };
    let class =
        index
            .symbol_at(source_id, *class_tree)
            .ok_or(PairedPlanError::UnsupportedSourceShape(
                "canonical class declaration identity",
            ))?;
    if store.symbols.get(class).kind != SymbolKind::Class {
        return Err(PairedPlanError::UnsupportedSourceShape(
            "one ordinary class declaration",
        ));
    }
    let TreeKind::TypeDef(class_definition) = &parsed.ast.get(*class_tree).kind else {
        return Err(PairedPlanError::UnsupportedSourceShape(
            "ordinary class type definition",
        ));
    };
    let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
        return Err(PairedPlanError::UnsupportedSourceShape("class template"));
    };
    if !template.parents.is_empty() {
        return Err(PairedPlanError::UnsupportedSourceShape(
            "classes without explicit parent applications",
        ));
    }
    let constructor_tree = template.constructor;
    let constructor = index.symbol_at(source_id, constructor_tree).ok_or(
        PairedPlanError::UnsupportedSourceShape("canonical primary constructor identity"),
    )?;
    let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(constructor_tree).kind else {
        return Err(PairedPlanError::UnsupportedSourceShape(
            "source primary constructor definition",
        ));
    };
    let constructor_parameters = constructor_definition
        .value_param_clauses
        .iter()
        .map(|clause| {
            clause
                .iter()
                .map(|tree| {
                    index
                        .derived_symbol_at(constructor, source_id, *tree)
                        .ok_or(PairedPlanError::UnsupportedSourceShape(
                            "canonical constructor-owned formal identity",
                        ))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer
        .complete_symbol(class)
        .map_err(PairedPlanError::Typer)?;
    typer
        .complete_symbol(constructor)
        .map_err(PairedPlanError::Typer)?;
    for parameter in constructor_parameters.iter().flatten().copied() {
        typer
            .complete_symbol(parameter)
            .map_err(PairedPlanError::Typer)?;
    }

    let mut tasty_members = Vec::with_capacity(template.body.len());
    let mut jvm_fields = Vec::new();
    let mut jvm_methods = Vec::new();
    let mut source_member_order = Vec::with_capacity(template.body.len());
    for &tree in &template.body {
        match &parsed.ast.get(tree).kind {
            TreeKind::ValDef(field_definition) => {
                let field = index.symbol_at(source_id, tree).ok_or(
                    PairedPlanError::UnsupportedSourceShape("canonical source field identity"),
                )?;
                if typer.store().symbols.get(field).kind != SymbolKind::Field
                    || typer.store().symbols.get(field).owner != Some(class)
                {
                    return Err(PairedPlanError::UnsupportedSourceShape(
                        "class-owned source field",
                    ));
                }
                if !typer
                    .store()
                    .symbols
                    .get(field)
                    .flags
                    .contains(SymbolFlags::MUTABLE)
                    || field_definition.metadata.visibility
                        != Some(VisibilitySyntax::Private { qualifier: None })
                {
                    return Err(PairedPlanError::UnsupportedSourceShape(
                        "unqualified private mutable class-body fields",
                    ));
                }
                typer
                    .complete_symbol(field)
                    .map_err(PairedPlanError::Typer)?;
                let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info else {
                    return Err(PairedPlanError::UnsupportedSourceShape(
                        "completed primitive field type",
                    ));
                };
                let rhs = field_definition
                    .rhs
                    .ok_or(PairedPlanError::UnsupportedSourceShape(
                        "field initializer expression",
                    ))?;
                let context = typer
                    .field_initializer_context_for(field)
                    .map_err(PairedPlanError::Typer)?;
                let typed_initializer = typer
                    .type_expression_expected(rhs, context, field_type)
                    .map_err(PairedPlanError::Typer)?;
                if typer.source_typed_index().get(source_id, rhs) != Some(typed_initializer) {
                    return Err(PairedPlanError::UnsupportedSourceShape(
                        "canonical source-to-typed field initializer binding",
                    ));
                }
                tasty_members.push(ClassMemberPicklePlan::Field(ClassFieldPicklePlan {
                    symbol: field,
                    source_tree: tree,
                    typed_initializer,
                    declared_type: field_type,
                }));
                jvm_fields.push(JvmFieldPlan {
                    field,
                    source_visibility: JvmFieldSourceVisibility::PlainPrivate,
                    initializer: Some(typed_initializer),
                });
                source_member_order.push(field);
            }
            TreeKind::DefDef(method_definition) if tree != constructor_tree => {
                let method = index.symbol_at(source_id, tree).ok_or(
                    PairedPlanError::UnsupportedSourceShape("canonical source method identity"),
                )?;
                typer
                    .complete_symbol(method)
                    .map_err(PairedPlanError::Typer)?;
                let parameters = method_definition
                    .value_param_clauses
                    .iter()
                    .flatten()
                    .map(|parameter_tree| {
                        index.symbol_at(source_id, *parameter_tree).ok_or(
                            PairedPlanError::UnsupportedSourceShape(
                                "canonical source method formal identity",
                            ),
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let rhs = method_definition
                    .rhs
                    .ok_or(PairedPlanError::UnsupportedSourceShape(
                        "concrete method body",
                    ))?;
                let context = typer
                    .expression_context_for(method)
                    .map_err(PairedPlanError::Typer)?;
                let body = typer
                    .type_expression(rhs, context)
                    .map_err(PairedPlanError::Typer)?;
                if typer.source_typed_index().get(source_id, rhs) != Some(body) {
                    return Err(PairedPlanError::UnsupportedSourceShape(
                        "canonical source-to-typed method body binding",
                    ));
                }
                tasty_members.push(ClassMemberPicklePlan::Method {
                    symbol: method,
                    source_tree: tree,
                });
                jvm_methods.push(JvmMethodPlan {
                    method,
                    parameters,
                    locals: Vec::new(),
                    body: Some(body),
                });
                source_member_order.push(method);
            }
            _ => {
                return Err(PairedPlanError::UnsupportedSourceShape(
                    "class body containing only initialized fields and concrete methods",
                ));
            }
        }
    }

    let constructor_plan = JvmPrimaryConstructorPlan::from_source(
        class,
        constructor,
        constructor_parameters.iter().flatten().copied().collect(),
        &parsed.ast,
        &index,
    )
    .map_err(PairedPlanError::Jvm)?;
    let class_plan = JvmClassPlan {
        class,
        fields: jvm_fields,
        methods: jvm_methods,
        initialization: JvmClassInitializationPlan::PrimaryConstructor(constructor_plan),
    };

    let typed_bindings =
        |source, tree: TreeId<Untyped>| typer.source_typed_index().get(source, tree);
    let tasty_input = ClassStatePicklePlan {
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &index,
        typed_bindings: &typed_bindings,
        source: SourceMetadata {
            id: source_id,
            name: source_name.to_owned(),
        },
        class,
        class_tree: *class_tree,
        constructor: PrimaryConstructorInputPlan {
            symbol: constructor,
            source_tree: constructor_tree,
            parameter_clauses: constructor_parameters,
        },
        members: &tasty_members,
    };
    let validated_tasty = tasty_input.validate().map_err(PairedPlanError::Tasty)?;
    let jvm_unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: vec![class_plan],
    };
    let compatibility =
        dotty_jvm_tasty_contract::validate_jvm_tasty_class(&jvm_unit, &validated_tasty)
            .map_err(PairedPlanError::Contract)?;
    let classfile_path = format!("{}.class", compatibility.internal_name());
    let tasty_path = compatibility.tasty_path().to_owned();
    let plan = PairedJvmTastyClassPlan {
        jvm_unit: &jvm_unit,
        tasty_plan: &validated_tasty,
        compatibility,
        class,
        constructor,
        source_id,
        source_member_order,
        classfile_path,
        tasty_path,
    };
    Ok(consume(plan))
}

/// Parse, name and type a bounded multi-class source unit exactly once, then
/// build one complete JVM unit and one validated TASTy/ABI proof per class.
///
/// The callback-scoped view preserves source declaration order and borrows
/// the same semantic store, source index, typed arena and source-to-typed
/// bindings for every class. This API only plans; it does not encode or
/// publish artifacts. The existing one-class API remains unchanged.
pub fn with_paired_jvm_tasty_batch_plan<R>(
    source_text: &str,
    source_name: &str,
    source_id: SourceId,
    consume: impl for<'batch> FnOnce(PairedJvmTastyBatchPlan<'batch>) -> R,
) -> Result<R, PairedPlanError> {
    const MAX_BATCH_CLASSES: usize = 32;

    let source = SourceText::new(source_text)
        .map_err(|error| PairedPlanError::SourceText(format!("{error:?}")))?;
    let scanner = ContextualScanner::new(source_text)
        .map_err(|error| PairedPlanError::Lexing(format!("{error:?}")))?;
    if !scanner.diagnostics().is_empty() {
        return Err(PairedPlanError::Lexing(format!(
            "{:?}",
            scanner.diagnostics()
        )));
    }

    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source, source_id, scanner, &mut store.names);
    if !parsed.diagnostics.is_empty() {
        return Err(PairedPlanError::Parsing(format!(
            "{:?}",
            parsed.diagnostics
        )));
    }
    let mut packages = Packages::new();
    let index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        source_name,
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .map_err(|error| PairedPlanError::Naming(format!("{error:?}")))?
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => return Err(PairedPlanError::Naming(format!("{outcome:?}"))),
    };

    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        return Err(PairedPlanError::BatchShape("one package root"));
    };
    if package.stats.len() < 2 {
        return Err(PairedPlanError::BatchShape(
            "at least two ordinary top-level classes",
        ));
    }
    if package.stats.len() > MAX_BATCH_CLASSES {
        return Err(PairedPlanError::BatchShape(
            "at most 32 ordinary top-level classes per planning batch",
        ));
    }

    let mut source_classes = Vec::with_capacity(package.stats.len());
    let mut package_owner = None;
    let mut seen_classes = std::collections::HashSet::with_capacity(package.stats.len());
    for (class_index, &tree) in package.stats.iter().enumerate() {
        let fail = |phase, reason: String| PairedPlanError::BatchClass {
            class_index,
            declaration_index: tree.index() as usize,
            phase,
            reason,
        };
        let TreeKind::TypeDef(class_definition) = &parsed.ast.get(tree).kind else {
            return Err(fail(
                "source collection",
                "top-level siblings must all be ordinary classes".to_owned(),
            ));
        };
        let class = index.symbol_at(source_id, tree).ok_or_else(|| {
            fail(
                "source collection",
                "missing canonical source class identity".to_owned(),
            )
        })?;
        let class_symbol = store.symbols.get(class);
        if class_symbol.kind != SymbolKind::Class {
            return Err(fail(
                "source collection",
                "top-level declaration is not an ordinary class".to_owned(),
            ));
        }
        if !seen_classes.insert(class) {
            return Err(fail(
                "source collection",
                "duplicate canonical class identity".to_owned(),
            ));
        }
        if index.definition_of(class)
            != Some(dotty_core::SourceDefinition::Canonical {
                source: source_id,
                tree,
            })
        {
            return Err(fail(
                "source collection",
                "class identity is not bound to this source declaration".to_owned(),
            ));
        }
        let owner = class_symbol.owner.ok_or_else(|| {
            fail(
                "source collection",
                "class has no canonical package owner".to_owned(),
            )
        })?;
        if store.symbols.get(owner).kind != SymbolKind::Package {
            return Err(fail(
                "source collection",
                "class owner is not a package".to_owned(),
            ));
        }
        if package_owner.is_some_and(|expected| expected != owner) {
            return Err(fail(
                "source collection",
                "classes do not have the exact same package owner".to_owned(),
            ));
        }
        package_owner = Some(owner);

        let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
            return Err(fail(
                "source collection",
                "class declaration must have an ordinary template".to_owned(),
            ));
        };
        if !template.parents.is_empty() || template.self_val.is_some() {
            return Err(fail(
                "source collection",
                "classes with explicit parents or self types are unsupported".to_owned(),
            ));
        }
        let constructor_tree = template.constructor;
        let constructor = index
            .symbol_at(source_id, constructor_tree)
            .ok_or_else(|| {
                fail(
                    "source collection",
                    "missing canonical primary-constructor identity".to_owned(),
                )
            })?;
        let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(constructor_tree).kind
        else {
            return Err(fail(
                "source collection",
                "primary constructor must be a source definition".to_owned(),
            ));
        };
        if !constructor_definition.type_params.is_empty() {
            return Err(fail(
                "source collection",
                "generic primary constructors are unsupported".to_owned(),
            ));
        }
        let constructor_parameters = constructor_definition
            .value_param_clauses
            .iter()
            .map(|clause| {
                clause
                    .iter()
                    .map(|parameter_tree| {
                        index
                            .derived_symbol_at(constructor, source_id, *parameter_tree)
                            .ok_or_else(|| {
                                fail(
                                    "source collection",
                                    "missing canonical constructor-owned formal identity"
                                        .to_owned(),
                                )
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        source_classes.push(BatchClassSource {
            tree,
            class,
            constructor_tree,
            constructor,
            constructor_parameters,
        });
    }

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for (class_index, source_class) in source_classes.iter().enumerate() {
        let declaration_index = source_class.tree.index() as usize;
        typer
            .complete_symbol(source_class.class)
            .map_err(|error| PairedPlanError::BatchClass {
                class_index,
                declaration_index,
                phase: "class typing",
                reason: format!("{error:?}"),
            })?;
        typer
            .complete_symbol(source_class.constructor)
            .map_err(|error| PairedPlanError::BatchClass {
                class_index,
                declaration_index,
                phase: "constructor typing",
                reason: format!("{error:?}"),
            })?;
        for parameter in source_class
            .constructor_parameters
            .iter()
            .flatten()
            .copied()
        {
            typer
                .complete_symbol(parameter)
                .map_err(|error| PairedPlanError::BatchClass {
                    class_index,
                    declaration_index,
                    phase: "constructor formal typing",
                    reason: format!("{error:?}"),
                })?;
        }
    }

    let mut drafts = Vec::with_capacity(source_classes.len());
    for (class_index, source_class) in source_classes.into_iter().enumerate() {
        let TreeKind::TypeDef(class_definition) = &parsed.ast.get(source_class.tree).kind else {
            return Err(PairedPlanError::BatchClass {
                class_index,
                declaration_index: source_class.tree.index() as usize,
                phase: "member planning",
                reason: "class tree changed during planning".to_owned(),
            });
        };
        let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
            return Err(PairedPlanError::BatchClass {
                class_index,
                declaration_index: source_class.tree.index() as usize,
                phase: "member planning",
                reason: "class template is missing".to_owned(),
            });
        };
        let mut tasty_members = Vec::with_capacity(template.body.len());
        let mut jvm_fields = Vec::new();
        let mut jvm_methods = Vec::new();
        let mut source_member_order = Vec::with_capacity(template.body.len());
        for (member_index, &tree) in template.body.iter().enumerate() {
            let fail = |phase, reason: String| PairedPlanError::BatchClass {
                class_index,
                declaration_index: member_index,
                phase,
                reason,
            };
            match &parsed.ast.get(tree).kind {
                TreeKind::ValDef(field_definition) => {
                    let field = index.symbol_at(source_id, tree).ok_or_else(|| {
                        fail(
                            "member planning",
                            "missing canonical source field identity".to_owned(),
                        )
                    })?;
                    if typer.store().symbols.get(field).kind != SymbolKind::Field
                        || typer.store().symbols.get(field).owner != Some(source_class.class)
                    {
                        return Err(fail(
                            "member planning",
                            "field is not owned by its declaring class".to_owned(),
                        ));
                    }
                    if !typer
                        .store()
                        .symbols
                        .get(field)
                        .flags
                        .contains(SymbolFlags::MUTABLE)
                        || field_definition.metadata.visibility
                            != Some(VisibilitySyntax::Private { qualifier: None })
                    {
                        return Err(fail(
                            "member planning",
                            "only unqualified private mutable fields are supported".to_owned(),
                        ));
                    }
                    typer
                        .complete_symbol(field)
                        .map_err(|error| PairedPlanError::BatchClass {
                            class_index,
                            declaration_index: member_index,
                            phase: "field typing",
                            reason: format!("{error:?}"),
                        })?;
                    let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info
                    else {
                        return Err(fail(
                            "field typing",
                            "field type did not complete".to_owned(),
                        ));
                    };
                    let rhs = field_definition.rhs.ok_or_else(|| {
                        fail("field typing", "field initializer is missing".to_owned())
                    })?;
                    let context = typer
                        .field_initializer_context_for(field)
                        .map_err(|error| PairedPlanError::BatchClass {
                            class_index,
                            declaration_index: member_index,
                            phase: "field typing",
                            reason: format!("{error:?}"),
                        })?;
                    let typed_initializer = typer
                        .type_expression_expected(rhs, context, field_type)
                        .map_err(|error| PairedPlanError::BatchClass {
                            class_index,
                            declaration_index: member_index,
                            phase: "field initializer typing",
                            reason: format!("{error:?}"),
                        })?;
                    if typer.source_typed_index().get(source_id, rhs) != Some(typed_initializer) {
                        return Err(fail(
                            "field initializer typing",
                            "source-to-typed initializer binding is not canonical".to_owned(),
                        ));
                    }
                    tasty_members.push(ClassMemberPicklePlan::Field(ClassFieldPicklePlan {
                        symbol: field,
                        source_tree: tree,
                        typed_initializer,
                        declared_type: field_type,
                    }));
                    jvm_fields.push(JvmFieldPlan {
                        field,
                        source_visibility: JvmFieldSourceVisibility::PlainPrivate,
                        initializer: Some(typed_initializer),
                    });
                    source_member_order.push(field);
                }
                TreeKind::DefDef(method_definition) if tree != source_class.constructor_tree => {
                    if !method_definition.type_params.is_empty() {
                        return Err(fail(
                            "member planning",
                            "generic methods are unsupported".to_owned(),
                        ));
                    }
                    let method = index.symbol_at(source_id, tree).ok_or_else(|| {
                        fail(
                            "member planning",
                            "missing canonical source method identity".to_owned(),
                        )
                    })?;
                    if typer.store().symbols.get(method).kind != SymbolKind::Method
                        || typer.store().symbols.get(method).owner != Some(source_class.class)
                    {
                        return Err(fail(
                            "member planning",
                            "method is not owned by its declaring class".to_owned(),
                        ));
                    }
                    typer
                        .complete_symbol(method)
                        .map_err(|error| PairedPlanError::BatchClass {
                            class_index,
                            declaration_index: member_index,
                            phase: "method typing",
                            reason: format!("{error:?}"),
                        })?;
                    let parameters = method_definition
                        .value_param_clauses
                        .iter()
                        .flatten()
                        .map(|parameter_tree| {
                            index.symbol_at(source_id, *parameter_tree).ok_or_else(|| {
                                fail(
                                    "method planning",
                                    "missing canonical method formal identity".to_owned(),
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let rhs = method_definition.rhs.ok_or_else(|| {
                        fail(
                            "method typing",
                            "concrete method body is missing".to_owned(),
                        )
                    })?;
                    let context = typer.expression_context_for(method).map_err(|error| {
                        PairedPlanError::BatchClass {
                            class_index,
                            declaration_index: member_index,
                            phase: "method typing",
                            reason: format!("{error:?}"),
                        }
                    })?;
                    let body = typer.type_expression(rhs, context).map_err(|error| {
                        PairedPlanError::BatchClass {
                            class_index,
                            declaration_index: member_index,
                            phase: "method body typing",
                            reason: format!("{error:?}"),
                        }
                    })?;
                    if typer.source_typed_index().get(source_id, rhs) != Some(body) {
                        return Err(fail(
                            "method body typing",
                            "source-to-typed method body binding is not canonical".to_owned(),
                        ));
                    }
                    tasty_members.push(ClassMemberPicklePlan::Method {
                        symbol: method,
                        source_tree: tree,
                    });
                    jvm_methods.push(JvmMethodPlan {
                        method,
                        parameters,
                        locals: Vec::new(),
                        body: Some(body),
                    });
                    source_member_order.push(method);
                }
                _ => {
                    return Err(fail(
                        "member planning",
                        "class bodies must contain only supported fields and concrete methods"
                            .to_owned(),
                    ));
                }
            }
        }

        let constructor_plan = JvmPrimaryConstructorPlan::from_source(
            source_class.class,
            source_class.constructor,
            source_class
                .constructor_parameters
                .iter()
                .flatten()
                .copied()
                .collect(),
            &parsed.ast,
            &index,
        )
        .map_err(|error| PairedPlanError::BatchClass {
            class_index,
            declaration_index: source_class.tree.index() as usize,
            phase: "constructor planning",
            reason: format!("{error:?}"),
        })?;
        let class = source_class.class;
        drafts.push(BatchClassDraft {
            source: source_class,
            tasty_members,
            jvm_plan: JvmClassPlan {
                class,
                fields: jvm_fields,
                methods: jvm_methods,
                initialization: JvmClassInitializationPlan::PrimaryConstructor(constructor_plan),
            },
            source_member_order,
        });
    }

    let jvm_unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: drafts.iter().map(|draft| draft.jvm_plan.clone()).collect(),
    };
    let _metadata =
        jvm_unit
            .metadata(&definitions)
            .map_err(|error| PairedPlanError::BatchUnit {
                phase: "complete JVM unit validation",
                reason: format!("{error:?}"),
            })?;
    let planned_classes = drafts
        .iter()
        .map(|draft| draft.source.class)
        .collect::<Vec<_>>();
    let typed_bindings = BatchSourceTypedBindings {
        index: typer.source_typed_index(),
        planned_classes,
    };
    let tasty_inputs = drafts
        .iter()
        .map(|draft| ClassStatePicklePlan {
            store: typer.store(),
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: typer.typed_ast(),
            source_index: &index,
            typed_bindings: &typed_bindings,
            source: SourceMetadata {
                id: source_id,
                name: source_name.to_owned(),
            },
            class: draft.source.class,
            class_tree: draft.source.tree,
            constructor: PrimaryConstructorInputPlan {
                symbol: draft.source.constructor,
                source_tree: draft.source.constructor_tree,
                parameter_clauses: draft.source.constructor_parameters.clone(),
            },
            members: &draft.tasty_members,
        })
        .collect::<Vec<_>>();
    let validated_tasty = tasty_inputs
        .iter()
        .enumerate()
        .map(|(class_index, plan)| {
            plan.validate()
                .map_err(|error| PairedPlanError::BatchClass {
                    class_index,
                    declaration_index: drafts[class_index].source.tree.index() as usize,
                    phase: "TASTy plan validation",
                    reason: format!("{error:?}"),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let compatibility = validated_tasty
        .iter()
        .enumerate()
        .map(|(class_index, plan)| {
            dotty_jvm_tasty_contract::validate_jvm_tasty_class(&jvm_unit, plan).map_err(|error| {
                match error {
                    JvmTastyContractError::JvmPlan(error) => PairedPlanError::BatchUnit {
                        phase: "complete JVM unit validation",
                        reason: format!("{error:?}"),
                    },
                    error => PairedPlanError::BatchClass {
                        class_index,
                        declaration_index: drafts[class_index].source.tree.index() as usize,
                        phase: "JVM/TASTy cross-contract validation",
                        reason: format!("{error:?}"),
                    },
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut classfile_paths = std::collections::HashSet::with_capacity(drafts.len());
    let mut tasty_paths = std::collections::HashSet::with_capacity(drafts.len());
    let classes = drafts
        .iter()
        .enumerate()
        .map(|(class_index, draft)| {
            let proof = &compatibility[class_index];
            let classfile_path = format!("{}.class", proof.internal_name());
            let tasty_path = proof.tasty_path().to_owned();
            if !classfile_paths.insert(classfile_path.clone())
                || !tasty_paths.insert(tasty_path.clone())
            {
                return Err(PairedPlanError::BatchClass {
                    class_index,
                    declaration_index: draft.source.tree.index() as usize,
                    phase: "artifact path validation",
                    reason: "duplicate class-file or TASTy output path".to_owned(),
                });
            }
            Ok(PairedJvmTastyBatchClass {
                compatibility: proof,
                tasty_plan: &validated_tasty[class_index],
                class: draft.source.class,
                constructor: draft.source.constructor,
                source_member_order: draft.source_member_order.clone(),
                classfile_path,
                tasty_path,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(consume(PairedJvmTastyBatchPlan {
        jvm_unit: &jvm_unit,
        classes: &classes,
        source_id,
    }))
}
