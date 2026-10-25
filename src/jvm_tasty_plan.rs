//! One-session planning for the audited JVM/TASTy class-state overlap.
//!
//! The coordinator is deliberately callback-scoped: both producer plans borrow
//! one frontend session, so callers inspect or consume the validated plans
//! while that session is alive. This module never emits bytes or writes files.

use std::error::Error;
use std::fmt;

use dotty_core::ast::{TreeKind, Untyped, VisibilitySyntax};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolFlags, SymbolId, SymbolInfo,
    SymbolKind, TreeId,
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
    PrimaryConstructorInputPlan, SourceMetadata, ValidatedClassStatePlan,
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
