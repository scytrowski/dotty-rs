use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use dotty_classfile::access_flags::{ACC_PRIVATE, ACC_PUBLIC};
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::reader::Reader;
use dotty_core::ast::{AstArena, DefDef, Template, TreeKind, Typed, Untyped};
use dotty_core::types::{Constant, TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolFlags, SymbolId, SymbolKind,
    TreeId, Visibility,
};
use dotty_jvm::{
    GeneratedClass, JVM_CLASSFILE_MAJOR_VERSION, JvmClassInitializationPlan, JvmClassPlan,
    JvmCodegenError, JvmCompilationUnit, JvmFieldPlan, JvmFieldSourceVisibility, JvmLocalBinding,
    JvmMethodPlan, JvmNameKind, JvmOverrideIssue, JvmPrimaryConstructorPlan,
    JvmPublicBodyFieldSourceProof, JvmReturnType, JvmType, emit_class,
};
use dotty_lexer::{ContextualScanner, LexerDiagnostic};
use dotty_namer::{
    NamerDiagnostic, NamerError, NamingOutcome, SourceDefinition, SourceParseStatus,
    SourceSemanticIndex, name_compilation_unit_with_diagnostics,
};
use dotty_parser::{ParseDiagnostic, parse_compilation_unit};
use dotty_typer::{SourceTyper, TyperError};

const FIXTURE_DIRECTORY: &str = "tests/fixtures/jvm";
const LINKAGE_COUNTER: &str = include_str!("fixtures/jvm_tasty_interop/Counter.scala");

type SourceBatchObserver<'observer> =
    dyn for<'a> FnMut(&mut JvmCompilationUnit<'a>, &Definitions) + 'observer;
type SourceSemanticObserver<'observer> = dyn for<'a> FnMut(
        &AstArena<Untyped>,
        &SourceSemanticIndex,
        &mut JvmCompilationUnit<'a>,
        &Definitions,
    ) + 'observer;
type TyperFailureObserver<'observer> =
    dyn for<'a> FnMut(&AstArena<Untyped>, &SourceTyper<'a>, &TyperError) + 'observer;
type SourceFailureExpectation = (&'static str, &'static str, fn(&PipelineFailure) -> bool);

#[derive(Default)]
struct SourceBatchObservers<'observer> {
    batch: Option<&'observer mut SourceBatchObserver<'observer>>,
    typer_failure: Option<&'observer mut TyperFailureObserver<'observer>>,
    source: Option<&'observer mut SourceSemanticObserver<'observer>>,
}

#[derive(Debug)]
#[allow(dead_code)] // Retain typed stage failures so test panics report the exact pipeline error.
enum PipelineFailure {
    SourceText(String),
    Lexer(String),
    LexerDiagnostics(Vec<LexerDiagnostic>),
    Parser(Vec<ParseDiagnostic>),
    NamerDiagnostics(Vec<NamerDiagnostic>),
    NamerStructural(NamerError),
    Typer(TyperError),
    Codegen(JvmCodegenError),
    MissingSourceIdentity(&'static str),
    UnexpectedSourceShape(&'static str),
    Io(std::io::Error),
    OutputPathCollision(PathBuf),
}

#[derive(Debug)]
struct EmittedArtifact {
    class: GeneratedClass,
    path: PathBuf,
}

struct TestOutputDirectory {
    path: PathBuf,
}

impl TestOutputDirectory {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("dotty-jvm-source-{}-{label}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create isolated class output directory");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn class_files(&self) -> Vec<PathBuf> {
        fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(directory).expect("read temporary output directory") {
                let path = entry.expect("read output directory entry").path();
                if path.is_dir() {
                    collect(&path, files);
                } else if path
                    .extension()
                    .is_some_and(|extension| extension == "class")
                {
                    files.push(path);
                }
            }
        }

        let mut files = Vec::new();
        collect(&self.path, &mut files);
        files.sort();
        files
    }
}

impl Drop for TestOutputDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug)]
struct SourceMethod {
    symbol: SymbolId,
    parameters: Vec<SymbolId>,
    rhs: Option<TreeId<Untyped>>,
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(FIXTURE_DIRECTORY)
        .join(name)
}

fn compile_fixture(
    fixture_name: &str,
    output_directory: &Path,
) -> Result<EmittedArtifact, PipelineFailure> {
    let fixture = fixture_path(fixture_name);
    let source = fs::read_to_string(&fixture).map_err(PipelineFailure::Io)?;
    compile_source(&source, fixture_name, output_directory)
}

fn compile_fixture_classes(
    fixture_name: &str,
    output_directory: &Path,
) -> Result<Vec<EmittedArtifact>, PipelineFailure> {
    let fixture = fixture_path(fixture_name);
    let source = fs::read_to_string(&fixture).map_err(PipelineFailure::Io)?;
    compile_source_batch(
        &source,
        fixture_name,
        output_directory,
        SourceId::from_index(1253),
        SourceUnitCardinality::AtLeastTwo,
    )
}

fn compile_source(
    source_text: &str,
    source_file_name: &str,
    output_directory: &Path,
) -> Result<EmittedArtifact, PipelineFailure> {
    let mut artifacts = compile_source_batch(
        source_text,
        source_file_name,
        output_directory,
        SourceId::from_index(1),
        SourceUnitCardinality::ExactlyOne,
    )?;
    artifacts
        .pop()
        .ok_or(PipelineFailure::UnexpectedSourceShape(
            "exactly one ordinary top-level class",
        ))
}

fn compile_source_classes(
    source_text: &str,
    source_file_name: &str,
    output_directory: &Path,
) -> Result<Vec<EmittedArtifact>, PipelineFailure> {
    compile_source_batch(
        source_text,
        source_file_name,
        output_directory,
        SourceId::from_index(1221),
        SourceUnitCardinality::OneOrMore,
    )
}

fn compile_planned_constructor_source(
    source_text: &str,
    output_directory: &Path,
) -> Result<Vec<EmittedArtifact>, PipelineFailure> {
    compile_source_batch(
        source_text,
        "ConstructorSmoke.scala",
        output_directory,
        SourceId::from_index(1218),
        SourceUnitCardinality::AtLeastTwo,
    )
}

#[derive(Clone, Copy)]
enum SourceUnitCardinality {
    ExactlyOne,
    OneOrMore,
    AtLeastTwo,
}

struct PlannedSourceClass {
    class: SymbolId,
    template_tree: TreeId<Untyped>,
    constructor_tree: TreeId<Untyped>,
    initialization: JvmClassInitializationPlan,
    methods: Vec<SourceMethod>,
}

struct PendingArtifact {
    class: GeneratedClass,
    path: PathBuf,
    staged_path: PathBuf,
    backup_path: PathBuf,
}

fn compile_source_batch(
    source_text: &str,
    source_file_name: &str,
    output_directory: &Path,
    source_id: SourceId,
    cardinality: SourceUnitCardinality,
) -> Result<Vec<EmittedArtifact>, PipelineFailure> {
    compile_source_batch_observed(
        source_text,
        source_file_name,
        output_directory,
        source_id,
        cardinality,
        None,
    )
}

fn compile_source_batch_observed<'observer>(
    source_text: &str,
    source_file_name: &str,
    output_directory: &Path,
    source_id: SourceId,
    cardinality: SourceUnitCardinality,
    observe: Option<&'observer mut SourceBatchObserver<'observer>>,
) -> Result<Vec<EmittedArtifact>, PipelineFailure> {
    compile_source_batch_with_typer_failure_observer(
        source_text,
        source_file_name,
        output_directory,
        source_id,
        cardinality,
        SourceBatchObservers {
            batch: observe,
            ..SourceBatchObservers::default()
        },
    )
}

fn compile_source_batch_with_source_observer<'observer>(
    source_text: &str,
    source_file_name: &str,
    output_directory: &Path,
    source_id: SourceId,
    cardinality: SourceUnitCardinality,
    observe: &'observer mut SourceSemanticObserver<'observer>,
) -> Result<Vec<EmittedArtifact>, PipelineFailure> {
    compile_source_batch_with_typer_failure_observer(
        source_text,
        source_file_name,
        output_directory,
        source_id,
        cardinality,
        SourceBatchObservers {
            source: Some(observe),
            ..SourceBatchObservers::default()
        },
    )
}

fn compile_source_batch_with_typer_failure_observer(
    source_text: &str,
    source_file_name: &str,
    output_directory: &Path,
    source_id: SourceId,
    cardinality: SourceUnitCardinality,
    observers: SourceBatchObservers<'_>,
) -> Result<Vec<EmittedArtifact>, PipelineFailure> {
    let SourceBatchObservers {
        batch: mut observe,
        typer_failure: mut observe_typer_failure,
        source: mut observe_source,
    } = observers;
    let source = SourceText::new(source_text)
        .map_err(|error| PipelineFailure::SourceText(error.to_string()))?;
    let scanner = ContextualScanner::new(source_text)
        .map_err(|error| PipelineFailure::Lexer(format!("{error:?}")))?;
    let lexer_diagnostics = scanner.diagnostics().to_vec();
    if !lexer_diagnostics.is_empty() {
        return Err(PipelineFailure::LexerDiagnostics(lexer_diagnostics));
    }
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source, source_id, scanner, &mut store.names);
    if !parsed.diagnostics.is_empty() {
        return Err(PipelineFailure::Parser(parsed.diagnostics));
    }
    let mut packages = Packages::new();
    let index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        source_file_name,
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .map_err(PipelineFailure::NamerStructural)?
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        NamingOutcome::Named { diagnostics, .. } | NamingOutcome::Rejected { diagnostics } => {
            return Err(PipelineFailure::NamerDiagnostics(diagnostics));
        }
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        return Err(PipelineFailure::UnexpectedSourceShape("package root"));
    };
    let mut classes = Vec::new();
    for tree in &package.stats {
        let TreeKind::TypeDef(definition) = &parsed.ast.get(*tree).kind else {
            continue;
        };
        let class = index
            .symbol_at(source_id, *tree)
            .ok_or(PipelineFailure::MissingSourceIdentity("class declaration"))?;
        if store.symbols.get(class).kind != SymbolKind::Class {
            continue;
        }
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            return Err(PipelineFailure::UnexpectedSourceShape("class template"));
        };
        let initialization =
            source_initialization_plan(class, &parsed.ast, template, source_id, &index, &store)
                .map_err(PipelineFailure::Codegen)?;
        classes.push(PlannedSourceClass {
            class,
            template_tree: definition.rhs,
            constructor_tree: template.constructor,
            initialization,
            methods: collect_source_methods(
                &parsed.ast,
                &template.body,
                source_id,
                &index,
                &store,
            )?,
        });
    }
    let valid_cardinality = match cardinality {
        SourceUnitCardinality::ExactlyOne => classes.len() == 1,
        SourceUnitCardinality::OneOrMore => !classes.is_empty(),
        SourceUnitCardinality::AtLeastTwo => classes.len() >= 2,
    };
    if !valid_cardinality {
        return Err(PipelineFailure::UnexpectedSourceShape(match cardinality {
            SourceUnitCardinality::ExactlyOne => "exactly one ordinary top-level class",
            SourceUnitCardinality::OneOrMore => "at least one ordinary top-level class",
            SourceUnitCardinality::AtLeastTwo => "at least two ordinary top-level classes",
        }));
    }

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for class in &classes {
        if let Err(error) = typer.complete_symbol(class.class) {
            if let Some(observer) = observe_typer_failure.as_mut() {
                observer(&parsed.ast, &typer, &error);
            }
            return Err(PipelineFailure::Typer(error));
        }
    }
    for class in &classes {
        let constructor = index.symbol_at(source_id, class.constructor_tree).ok_or(
            PipelineFailure::MissingSourceIdentity("primary constructor"),
        )?;
        if matches!(
            class.initialization,
            JvmClassInitializationPlan::PrimaryConstructor(_)
        ) {
            typer
                .complete_symbol(constructor)
                .map_err(PipelineFailure::Typer)?;
        }
    }
    for class in &classes {
        if !matches!(
            class.initialization,
            JvmClassInitializationPlan::PrimaryConstructor(_)
        ) {
            continue;
        }
        let constructor = index.symbol_at(source_id, class.constructor_tree).ok_or(
            PipelineFailure::MissingSourceIdentity("primary constructor"),
        )?;
        let TreeKind::DefDef(definition) = &parsed.ast.get(class.constructor_tree).kind else {
            return Err(PipelineFailure::UnexpectedSourceShape(
                "primary constructor definition",
            ));
        };
        for parameter_tree in definition.value_param_clauses.iter().flatten() {
            let formal = index
                .derived_symbol_at(constructor, source_id, *parameter_tree)
                .ok_or(PipelineFailure::MissingSourceIdentity(
                    "derived constructor formal",
                ))?;
            typer
                .complete_symbol(formal)
                .map_err(PipelineFailure::Typer)?;
            if let Some(source_parameter) = index.symbol_at(source_id, *parameter_tree)
                && typer.store().symbols.get(source_parameter).kind == SymbolKind::Field
            {
                typer
                    .complete_symbol(source_parameter)
                    .map_err(PipelineFailure::Typer)?;
            }
        }
    }
    for class in &mut classes {
        let JvmClassInitializationPlan::PrimaryConstructor(constructor_plan) =
            &class.initialization
        else {
            continue;
        };
        let TreeKind::Template(template) = &parsed.ast.get(class.template_tree).kind else {
            return Err(PipelineFailure::UnexpectedSourceShape("class template"));
        };
        let [parent_application] = template.parents.as_slice() else {
            continue;
        };
        let TreeKind::Apply(source_apply) = &parsed.ast.get(*parent_application).kind else {
            continue;
        };
        if source_apply.args.is_empty() {
            continue;
        }
        let typed_parent_application = typer
            .source_typed_index()
            .get(source_id, *parent_application)
            .ok_or(PipelineFailure::UnexpectedSourceShape(
                "typed direct-super constructor application",
            ))?;
        let TreeKind::Apply(typed_apply) = &typer.typed_ast().get(typed_parent_application).kind
        else {
            return Err(PipelineFailure::UnexpectedSourceShape(
                "typed direct-super constructor application",
            ));
        };
        let arguments = source_apply
            .args
            .iter()
            .zip(&typed_apply.args)
            .map(|(source, typed)| {
                if typer.source_typed_index().get(source_id, *source) != Some(*typed) {
                    return Err(PipelineFailure::UnexpectedSourceShape(
                        "parent argument typed mapping",
                    ));
                }
                Ok(dotty_jvm::JvmConstructorArgumentBinding {
                    source: *source,
                    typed: *typed,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let constructor_plan = constructor_plan
            .clone()
            .with_direct_super_constructor(
                *parent_application,
                typed_parent_application,
                arguments,
                &parsed.ast,
                &index,
                typer.source_typed_index(),
                typer.store(),
                typer.typed_ast(),
            )
            .map_err(PipelineFailure::Codegen)?;
        class.initialization = JvmClassInitializationPlan::PrimaryConstructor(constructor_plan);
    }
    for class in &classes {
        let TreeKind::Template(template) = &parsed.ast.get(class.template_tree).kind else {
            return Err(PipelineFailure::UnexpectedSourceShape("class template"));
        };
        for tree in &template.body {
            if !matches!(parsed.ast.get(*tree).kind, TreeKind::ValDef(_)) {
                continue;
            }
            if let Some(field) = index.symbol_at(source_id, *tree)
                && typer.store().symbols.get(field).kind == SymbolKind::Field
            {
                typer
                    .complete_symbol(field)
                    .map_err(PipelineFailure::Typer)?;
            }
        }
    }
    for class in &classes {
        for method in &class.methods {
            if let Err(error) = typer.complete_symbol(method.symbol) {
                if let Some(observer) = observe_typer_failure.as_mut() {
                    observer(&parsed.ast, &typer, &error);
                }
                return Err(PipelineFailure::Typer(error));
            }
        }
    }

    let mut fields_by_class = Vec::with_capacity(classes.len());
    for class in &classes {
        let mut fields = Vec::new();
        let TreeKind::Template(template) = &parsed.ast.get(class.template_tree).kind else {
            return Err(PipelineFailure::UnexpectedSourceShape("class template"));
        };
        for tree in &template.body {
            let TreeKind::ValDef(definition) = &parsed.ast.get(*tree).kind else {
                continue;
            };
            let Some(field) = index.symbol_at(source_id, *tree) else {
                continue;
            };
            if typer.store().symbols.get(field).kind != SymbolKind::Field {
                continue;
            }
            if index.definition_of(field)
                != Some(SourceDefinition::Canonical {
                    source: source_id,
                    tree: *tree,
                })
            {
                return Err(PipelineFailure::MissingSourceIdentity(
                    "canonical class-body field definition",
                ));
            }
            if typer.store().symbols.get(field).owner != Some(class.class) {
                return Err(PipelineFailure::UnexpectedSourceShape(
                    "class-owned body field",
                ));
            }
            let initializer = match definition.rhs {
                Some(rhs) => {
                    let ty = match typer.store().symbols.get(field).info {
                        dotty_core::SymbolInfo::Complete(ty) => ty,
                        _ => return Err(PipelineFailure::UnexpectedSourceShape("field type")),
                    };
                    let context = typer
                        .field_initializer_context_for(field)
                        .map_err(PipelineFailure::Typer)?;
                    let typed_rhs = typer
                        .type_expression_expected(rhs, context, ty)
                        .map_err(PipelineFailure::Typer)?;
                    if typer.source_typed_index().get(source_id, rhs) != Some(typed_rhs) {
                        return Err(PipelineFailure::UnexpectedSourceShape(
                            "source-to-typed field initializer binding",
                        ));
                    }
                    Some(typed_rhs)
                }
                None => None,
            };
            let source_visibility = match definition.metadata.visibility {
                None => {
                    let initializer = initializer.ok_or(PipelineFailure::Codegen(
                        JvmCodegenError::MissingFieldInitializer { field },
                    ))?;
                    JvmFieldSourceVisibility::PlainPublic(
                        JvmPublicBodyFieldSourceProof::from_source(
                            class.class,
                            field,
                            source_id,
                            *tree,
                            initializer,
                            &parsed.ast,
                            &index,
                            typer.source_typed_index(),
                            typer.typed_ast(),
                            typer.store(),
                        )
                        .map_err(PipelineFailure::Codegen)?,
                    )
                }
                Some(dotty_core::ast::VisibilitySyntax::Private { qualifier: None }) => {
                    JvmFieldSourceVisibility::PlainPrivate
                }
                Some(dotty_core::ast::VisibilitySyntax::Private { qualifier: Some(_) }) => {
                    JvmFieldSourceVisibility::PrivateThis
                }
                Some(dotty_core::ast::VisibilitySyntax::Protected { .. }) => {
                    JvmFieldSourceVisibility::PlainPrivate
                }
            };
            fields.push(JvmFieldPlan {
                field,
                source_visibility,
                initializer,
            });
        }
        fields_by_class.push(fields);
    }

    let mut plans = Vec::with_capacity(classes.len());
    for (class, fields) in classes.into_iter().zip(fields_by_class) {
        let mut methods = Vec::with_capacity(class.methods.len());
        for source_method in class.methods {
            let body = match source_method.rhs {
                Some(rhs) => {
                    let context = match typer.expression_context_for(source_method.symbol) {
                        Ok(context) => context,
                        Err(error) => {
                            if let Some(observer) = observe_typer_failure.as_mut() {
                                observer(&parsed.ast, &typer, &error);
                            }
                            return Err(PipelineFailure::Typer(error));
                        }
                    };
                    match typer.type_expression(rhs, context) {
                        Ok(body) => Some(body),
                        Err(error) => {
                            if let Some(observer) = observe_typer_failure.as_mut() {
                                observer(&parsed.ast, &typer, &error);
                            }
                            return Err(PipelineFailure::Typer(error));
                        }
                    }
                }
                None => None,
            };
            let mut locals = parsed
                .ast
                .iter()
                .filter_map(|(source_tree, node)| {
                    if !matches!(node.kind, TreeKind::ValDef(_)) {
                        return None;
                    }
                    let binding = typer.typed_local_binding_at(source_id, source_tree)?;
                    (typer.store().symbols.get(binding.symbol()).owner
                        == Some(source_method.symbol))
                    .then_some(JvmLocalBinding {
                        declaration: binding.declaration(),
                        symbol: binding.symbol(),
                        mutable: typer
                            .store()
                            .symbols
                            .get(binding.symbol())
                            .flags
                            .contains(SymbolFlags::MUTABLE),
                    })
                })
                .collect::<Vec<_>>();
            locals.sort_by_key(|binding| binding.declaration.index());
            methods.push(JvmMethodPlan {
                method: source_method.symbol,
                parameters: source_method.parameters,
                locals,
                body,
            });
        }
        plans.push(JvmClassPlan {
            class: class.class,
            fields,
            methods,
            initialization: class.initialization,
        });
    }

    let mut unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: plans,
    };
    if let Some(observe) = observe_source.as_mut() {
        observe(&parsed.ast, &index, &mut unit, &definitions);
    }
    if let Some(observe) = observe.as_mut() {
        observe(&mut unit, &definitions);
    }
    let mut generated = Vec::with_capacity(unit.classes.len());
    for plan in &unit.classes {
        generated
            .push(emit_class(&unit, plan.class, &definitions).map_err(PipelineFailure::Codegen)?);
    }
    publish_class_batch(output_directory, generated)
}

fn term_type_target(unit: &JvmCompilationUnit<'_>, ty: dotty_core::TypeId) -> Option<SymbolId> {
    match unit.store.types.get(ty) {
        Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        } => Some(*symbol),
        _ => None,
    }
}

fn source_with_only_runner_method(source: &str, method_name: &str) -> String {
    let (shared_definitions, runner) = source
        .split_once("class Runner:\n")
        .expect("source fixture contains Runner");
    let method_prefix = format!("  def {method_name}(");
    let start = runner
        .find(&method_prefix)
        .unwrap_or_else(|| panic!("source fixture contains {method_name}"));
    let method = &runner[start..];
    let end = method[1..]
        .find("\n  def ")
        .map(|offset| offset + 1)
        .unwrap_or(method.len());
    format!(
        "{shared_definitions}class Runner:\n{}",
        method[..end].trim_end()
    )
}

fn audit_type_symbol(unit: &JvmCompilationUnit<'_>, ty: dotty_core::TypeId) -> Option<SymbolId> {
    match unit.store.types.get(ty) {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => Some(*symbol),
        _ => None,
    }
}

fn audit_class_parent(unit: &JvmCompilationUnit<'_>, class: SymbolId) -> SymbolId {
    let dotty_core::SymbolInfo::Complete(class_info) = unit.store.symbols.get(class).info else {
        panic!("class {class:?} has no completed semantic info");
    };
    let Type::ClassInfo(info) = unit.store.types.get(class_info) else {
        panic!("class {class:?} info is not ClassInfo");
    };
    let [parent] = info.parents.as_slice() else {
        panic!("class {class:?} does not have exactly one parent");
    };
    audit_type_symbol(unit, *parent).expect("canonical parent has a symbol target")
}

fn audit_class_named(unit: &JvmCompilationUnit<'_>, name: &str) -> SymbolId {
    unit.classes
        .iter()
        .map(|plan| plan.class)
        .find(|class| {
            unit.store
                .names
                .resolve(unit.store.symbols.get(*class).name.text())
                == name
        })
        .unwrap_or_else(|| panic!("missing planned class {name}"))
}

fn audit_method_named<'unit>(
    unit: &'unit JvmCompilationUnit<'_>,
    class: SymbolId,
    name: &str,
) -> &'unit JvmMethodPlan {
    let plan = unit
        .classes
        .iter()
        .find(|plan| plan.class == class)
        .expect("class has a JVM plan");
    plan.methods
        .iter()
        .find(|method| {
            unit.store
                .names
                .resolve(unit.store.symbols.get(method.method).name.text())
                == name
        })
        .unwrap_or_else(|| panic!("missing method {name} on {class:?}"))
}

fn typed_tree_targets(unit: &JvmCompilationUnit<'_>, root: TreeId<Typed>) -> Vec<SymbolId> {
    let mut work = vec![root];
    let mut targets = Vec::new();
    while let Some(tree_id) = work.pop() {
        let tree = unit.typed_ast.get(tree_id);
        match &tree.kind {
            TreeKind::Select(select) => work.push(select.qualifier),
            TreeKind::Super(super_tree) => work.push(super_tree.qual),
            TreeKind::Apply(application) => {
                if let Some(target) =
                    term_type_target(unit, unit.typed_ast.get(application.function).ty)
                {
                    targets.push(target);
                }
                work.push(application.function);
                work.extend(application.args.iter().copied());
            }
            TreeKind::If(if_tree) => {
                work.extend([if_tree.cond, if_tree.then_branch, if_tree.else_branch])
            }
            TreeKind::Block(block) => {
                work.extend(block.stats.iter().copied());
                work.push(block.expr);
            }
            TreeKind::Typed(typed) => work.extend([typed.expr, typed.tpt]),
            TreeKind::New(new_tree) => work.push(new_tree.tpt),
            _ => {}
        }
        if let TreeKind::Select(select) = &tree.kind
            && matches!(
                unit.typed_ast.get(select.qualifier).kind,
                TreeKind::Super(_)
            )
            && let Some(target) = term_type_target(unit, tree.ty)
        {
            targets.push(target);
        }
    }
    targets
}

#[test]
fn exact_tasty_linkage_counter_source_is_accepted_by_jvm_backend() {
    let output = TestOutputDirectory::new("tasty-linkage-counter");
    let emitted = compile_source(LINKAGE_COUNTER, "Counter.scala", output.path()).unwrap();
    let mut reader = Reader::new(emitted.class.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();

    assert_eq!(emitted.class.internal_name(), "example/Counter");
    assert_eq!(class.version.major, JVM_CLASSFILE_MAJOR_VERSION);
    assert_eq!(
        class.constant_pool.class_name(class.this_class).unwrap(),
        "example/Counter"
    );
    assert_eq!(class.fields.len(), 1);
    assert_eq!(class.methods.len(), 3);
    let field = &class.fields[0];
    assert_eq!(
        class.constant_pool.utf8(field.name_index).unwrap(),
        "current"
    );
    assert_eq!(
        class.constant_pool.utf8(field.descriptor_index).unwrap(),
        "I"
    );
    assert!(field.access_flags.contains(ACC_PRIVATE));
    let methods = class
        .methods
        .iter()
        .map(|method| {
            (
                class.constant_pool.utf8(method.name_index).unwrap(),
                class.constant_pool.utf8(method.descriptor_index).unwrap(),
                method.access_flags.0,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        methods
            .iter()
            .map(|method| (method.0, method.1))
            .collect::<Vec<_>>(),
        [("<init>", "(I)V"), ("get", "()I"), ("set", "(I)V")]
    );
    assert!(methods.iter().all(|method| method.2 & ACC_PUBLIC != 0));
    assert!(!class.attributes.iter().any(|attribute| match attribute {
        Attribute::Other(raw) => class.constant_pool.utf8(raw.name_index).unwrap() == "TASTY",
        _ => false,
    }));
}

#[test]
fn multilevel_source_audit_validates_the_three_level_hierarchy_proofs() {
    let source = r#"
class Base(seed: Int):
  private var current: Int = seed
  def value(): Int = current
  def doubled(): Int = value() + value()

class Middle(seed: Int) extends Base(seed):
  override def value(): Int = super.value() + 1

class Leaf(seed: Int) extends Middle(seed):
  override def value(): Int = super.value() + 1
  def parentValue(): Int = super.value()

class Runner:
  def evaluate(base: Base): Int = base.doubled()
  def run(seed: Int): Int = evaluate(new Leaf(seed))
  def choose(flag: Boolean, seed: Int): Middle =
    if flag then new Middle(seed) else new Leaf(seed)
  def chooseReverse(flag: Boolean, seed: Int): Middle =
    if flag then new Leaf(seed) else new Middle(seed)
  def chosenValue(flag: Boolean, seed: Int): Int = choose(flag, seed).value()
  def leafIdentity(leaf: Leaf): Leaf = leaf
"#;
    let output = TestOutputDirectory::new("multilevel-source-audit");
    let mut observed = false;
    let result = compile_source_batch_observed(
        source,
        "MultilevelAudit.scala",
        output.path(),
        SourceId::from_index(1401),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut |unit, definitions| {
            observed = true;
            let hierarchy = unit
                .class_hierarchy(definitions)
                .expect("the complete source-proven three-class chain is accepted");
            let base = audit_class_named(unit, "Base");
            let middle = audit_class_named(unit, "Middle");
            let leaf = audit_class_named(unit, "Leaf");
            let runner = audit_class_named(unit, "Runner");
            assert_eq!(audit_class_parent(unit, base), definitions.object_class);
            assert_eq!(audit_class_parent(unit, middle), base);
            assert_eq!(audit_class_parent(unit, leaf), middle);
            assert_eq!(
                hierarchy.nearest_common_non_object_ancestor(leaf, middle),
                Some(middle)
            );
            assert_eq!(
                hierarchy.nearest_common_non_object_ancestor(middle, base),
                Some(base)
            );
            assert_eq!(
                hierarchy.nearest_common_non_object_ancestor(leaf, leaf),
                Some(leaf)
            );
            assert_eq!(
                hierarchy
                    .superclasses()
                    .iter()
                    .map(|edge| (edge.class(), edge.parent()))
                    .collect::<Vec<_>>(),
                [
                    (base, definitions.object_class),
                    (middle, base),
                    (leaf, middle),
                    (runner, definitions.object_class),
                ]
            );
            let metadata = unit
                .metadata(definitions)
                .expect("three-level source classes retain exact immediate-parent metadata");
            assert_eq!(
                metadata
                    .iter()
                    .map(|class| (
                        class.identity().symbol(),
                        class.superclass().unwrap().parent(),
                    ))
                    .collect::<Vec<_>>(),
                [
                    (base, definitions.object_class),
                    (middle, base),
                    (leaf, middle),
                    (runner, definitions.object_class),
                ]
            );

            let base_plan = unit.classes.iter().find(|plan| plan.class == base).unwrap();
            let JvmClassInitializationPlan::PrimaryConstructor(base_constructor) =
                &base_plan.initialization
            else {
                panic!("Base source primary constructor should be retained");
            };
            assert_eq!(base_constructor.parameters().len(), 1);
            let base_formal = base_constructor.parameters()[0];
            assert_eq!(
                unit.store.symbols.get(base_formal).owner,
                Some(base_constructor.symbol())
            );
            assert_eq!(
                unit.store
                    .names
                    .resolve(unit.store.symbols.get(base_formal).name.text()),
                "seed"
            );

            for (class, parent) in [(middle, base), (leaf, middle)] {
                let plan = unit
                    .classes
                    .iter()
                    .find(|plan| plan.class == class)
                    .unwrap();
                let JvmClassInitializationPlan::PrimaryConstructor(constructor) =
                    &plan.initialization
                else {
                    panic!("source primary constructor should be retained");
                };
                assert_eq!(constructor.parameters().len(), 1);
                let formal = constructor.parameters()[0];
                assert_eq!(
                    unit.store.symbols.get(formal).owner,
                    Some(constructor.symbol())
                );
                assert_eq!(
                    unit.store
                        .names
                        .resolve(unit.store.symbols.get(formal).name.text()),
                    "seed"
                );
                let super_call = constructor
                    .direct_super_constructor()
                    .expect("parameterized direct parent constructor is recorded");
                assert_eq!(super_call.parent_class(), parent);
                assert_eq!(
                    unit.store
                        .symbols
                        .get(super_call.parent_constructor())
                        .owner,
                    Some(parent)
                );
            }

            let base_value = audit_method_named(unit, base, "value").method;
            let base_doubled = audit_method_named(unit, base, "doubled").method;
            let doubled = audit_method_named(unit, base, "doubled");
            let doubled_targets = typed_tree_targets(unit, doubled.body.unwrap());
            let value_targets = doubled_targets
                .into_iter()
                .filter(|target| {
                    unit.store.symbols.get(*target).owner == Some(base)
                        && unit
                            .store
                            .names
                            .resolve(unit.store.symbols.get(*target).name.text())
                            == "value"
                })
                .collect::<Vec<_>>();
            assert_eq!(value_targets, [base_value, base_value]);

            let evaluate = audit_method_named(unit, runner, "evaluate");
            assert_eq!(
                typed_tree_targets(unit, evaluate.body.unwrap()),
                [base_doubled]
            );

            let chosen_value = audit_method_named(unit, runner, "chosenValue");
            let value_owner_targets = typed_tree_targets(unit, chosen_value.body.unwrap())
                .into_iter()
                .filter(|target| {
                    unit.store
                        .names
                        .resolve(unit.store.symbols.get(*target).name.text())
                        == "value"
                })
                .collect::<Vec<_>>();
            assert_eq!(
                value_owner_targets,
                [audit_method_named(unit, middle, "value").method]
            );

            for (class, method_name, parent) in [
                (middle, "value", base),
                (leaf, "value", middle),
                (leaf, "parentValue", middle),
            ] {
                let method = audit_method_named(unit, class, method_name);
                let targets = typed_tree_targets(unit, method.body.unwrap());
                let mut super_targets = targets
                    .into_iter()
                    .filter(|target| {
                        unit.store.symbols.get(*target).owner == Some(parent)
                            && unit
                                .store
                                .names
                                .resolve(unit.store.symbols.get(*target).name.text())
                                == "value"
                    })
                    .collect::<Vec<_>>();
                super_targets.sort_by_key(|target| target.index());
                super_targets.dedup();
                assert_eq!(
                    super_targets.len(),
                    1,
                    "{method_name} uses direct super.value"
                );
            }

            for method_name in ["choose", "chooseReverse"] {
                let method = audit_method_named(unit, runner, method_name);
                let body = unit.typed_ast.get(method.body.unwrap());
                let TreeKind::If(_) = body.kind else {
                    panic!("{method_name} retains a canonical typed If root");
                };
                assert_eq!(audit_type_symbol(unit, body.ty), Some(middle));
            }

            let proof = dotty_classfile::bytecode::ClassHierarchy::from_edges([
                ("Base".to_owned(), "java/lang/Object".to_owned()),
                ("Middle".to_owned(), "Base".to_owned()),
                ("Leaf".to_owned(), "Middle".to_owned()),
            ])
            .expect("the JVM-only explicit hierarchy accepts a checked chain");
            assert!(proof.is_assignable("Leaf", "Base"));
            assert!(proof.is_assignable("Leaf", "Middle"));
            assert!(!proof.is_assignable("Base", "Leaf"));
        }),
    );

    assert!(observed, "source passed through the canonical frontend");
    assert!(!matches!(
        result,
        Err(PipelineFailure::Codegen(
            JvmCodegenError::InvalidJvmSuperclass {
                issue: dotty_jvm::JvmSuperclassIssue::InheritanceTooDeep,
                ..
            }
        ))
    ));
    if result.is_err() {
        assert!(
            output.class_files().is_empty(),
            "failed class batches publish nothing"
        );
    }
}

#[test]
fn ancestor_related_reference_ifs_lower_as_the_typed_result_class() {
    let source = r#"
class Base(seed: Int):
  private var current: Int = seed
  def value(): Int = current

class Middle(seed: Int) extends Base(seed):
  override def value(): Int = super.value() + 1

class Leaf(seed: Int) extends Middle(seed):
  override def value(): Int = super.value() + 1

class Runner:
  def choose(flag: Boolean, seed: Int): Middle =
    if flag then new Middle(seed) else new Leaf(seed)
  def chooseReverse(flag: Boolean, seed: Int): Middle =
    if flag then new Leaf(seed) else new Middle(seed)
  def accept(value: Middle): Int = value.value()
  def viaArgument(flag: Boolean, seed: Int): Int = accept(choose(flag, seed))
  def viaReceiver(flag: Boolean, seed: Int): Int = choose(flag, seed).value()
  def reverseViaReceiver(flag: Boolean, seed: Int): Int = chooseReverse(flag, seed).value()
"#;
    let output = TestOutputDirectory::new("ancestor-related-reference-ifs");
    let artifacts =
        compile_source_classes(source, "AncestorRelatedReferenceIfs.scala", output.path())
            .expect("ancestor-related source If expressions lower to their typed result class");
    assert_eq!(artifacts.len(), 4);

    let runner = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Runner")
        .expect("Runner artifact");
    let runner = decode_emitted_class(runner);
    for method_name in ["choose", "chooseReverse"] {
        let code = emitted_method_code(&runner, method_name);
        assert!(
            decode_subset_instructions(code)
                .iter()
                .all(|(_, opcode)| *opcode != 0xc0),
            "{method_name} must not insert a checkcast"
        );
        assert!(
            stack_map_frames(&runner, method_name).iter().any(|frame| {
                frame.stack == [StackMapVerificationType::Object("Middle".to_owned())]
            }),
            "{method_name} must encode its join as Object(Middle)"
        );
    }
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "viaReceiver"),
        &[
            (0xb6, "Runner", "choose", "(ZI)LMiddle;"),
            (0xb6, "Middle", "value", "()I"),
        ],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "viaArgument"),
        &[
            (0xb6, "Runner", "choose", "(ZI)LMiddle;"),
            (0xb6, "Runner", "accept", "(LMiddle;)I"),
        ],
    );

    if !require_jdk_or_skip() {
        return;
    }
    let harness = output.path().join("AncestorRelatedReferenceIfHarness.java");
    fs::write(
        &harness,
        r#"public final class AncestorRelatedReferenceIfHarness {
    public static void main(String[] args) {
        Runner runner = new Runner();
        if (!(runner.choose(true, 10) instanceof Middle)) throw new AssertionError("middle branch");
        if (!(runner.choose(false, 10) instanceof Leaf)) throw new AssertionError("leaf branch");
        if (!(runner.chooseReverse(true, 10) instanceof Leaf)) throw new AssertionError("reverse leaf branch");
        if (!(runner.chooseReverse(false, 10) instanceof Middle)) throw new AssertionError("reverse middle branch");
        if (runner.viaArgument(true, 10) != 11 || runner.viaArgument(false, 10) != 12) throw new AssertionError("reference argument");
        if (runner.viaReceiver(true, 10) != 11 || runner.viaReceiver(false, 10) != 12) throw new AssertionError("virtual receiver");
        if (runner.reverseViaReceiver(true, 10) != 12 || runner.reverseViaReceiver(false, 10) != 11) throw new AssertionError("reversed virtual receiver");
    }
}
"#,
    )
    .unwrap();
    let classpath = output.path().to_string_lossy().into_owned();
    let javac = Command::new("javac")
        .args(["-classpath", &classpath, "-d", &classpath])
        .arg(&harness)
        .output()
        .expect("start javac for ancestor-related reference If harness");
    assert!(
        javac.status.success(),
        "javac rejected the reference If harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            &classpath,
            "AncestorRelatedReferenceIfHarness",
        ])
        .output()
        .expect("start java for ancestor-related reference If harness");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected the generated reference If classes\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
}

#[test]
fn sibling_source_branch_uses_its_exact_non_object_parent_join() {
    let source = r#"
class Root
class Left extends Root
class Right extends Root
class Branch:
  def choose(flag: Boolean): Root =
    if flag then new Left() else new Right()
"#;
    let output = TestOutputDirectory::new("sibling-source-parent-join");
    let mut observed = false;
    let result = compile_source_batch_observed(
        source,
        "UnrelatedJoinAudit.scala",
        output.path(),
        SourceId::from_index(1402),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut |unit, _| {
            observed = true;
            let branch = audit_class_named(unit, "Branch");
            let choose = audit_method_named(unit, branch, "choose");
            let body = unit.typed_ast.get(choose.body.unwrap());
            let TreeKind::If(_) = body.kind else {
                panic!("sibling branch remains a canonical typed If");
            };
            let Type::Or { left, right } = unit.store.types.get(body.ty) else {
                panic!("sibling classes retain the typer's Or result");
            };
            let mut targets = [
                audit_type_symbol(unit, *left).expect("left Or arm has a class identity"),
                audit_type_symbol(unit, *right).expect("right Or arm has a class identity"),
            ];
            targets.sort_by_key(|symbol| symbol.index());
            let mut expected = [
                audit_class_named(unit, "Left"),
                audit_class_named(unit, "Right"),
            ];
            expected.sort_by_key(|symbol| symbol.index());
            assert_eq!(targets, expected);
        }),
    );
    assert!(observed, "frontend pipeline stopped early: {result:?}");
    let artifacts = result.expect("Root is the exact nearest non-Object parent");
    let branch = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Branch")
        .expect("Branch artifact");
    let branch = decode_emitted_class(branch);
    assert_eq!(
        methods_with_descriptors(&branch),
        vec![
            ("<init>".to_owned(), "()V".to_owned()),
            ("choose".to_owned(), "(Z)LRoot;".to_owned())
        ]
    );
    assert!(
        stack_map_frames(&branch, "choose")
            .iter()
            .any(|frame| { frame.stack == [StackMapVerificationType::Object("Root".to_owned())] })
    );
}

#[test]
fn sibling_reference_join_audit_uses_the_real_parameterized_source_program() {
    let source = r#"
class Base(seed: Int):
  private var current: Int = seed
  def value(): Int = current
  def doubled(): Int = value() + value()

class Left(seed: Int) extends Base(seed):
  override def value(): Int = super.value() + 1

class Right(seed: Int) extends Base(seed):
  override def value(): Int = super.value() + 2

class Runner:
  def choose(flag: Boolean, seed: Int): Base =
    if flag then new Left(seed) else new Right(seed)
  def chooseReverse(flag: Boolean, seed: Int): Base =
    if flag then new Right(seed) else new Left(seed)
  def run(flag: Boolean, seed: Int): Int =
    choose(flag, seed).doubled()
  def accept(value: Base): Int = value.doubled()
  def viaArgument(flag: Boolean, seed: Int): Int = accept(choose(flag, seed))
  def acceptChoice(flag: Boolean, seed: Int): Int =
    accept(if flag then new Left(seed) else new Right(seed))
  def chooseNested(first: Boolean, second: Boolean, seed: Int): Base =
    if first then new Left(seed)
    else if second then new Right(seed)
    else new Left(seed)
"#;
    let output = TestOutputDirectory::new("sibling-reference-join-audit");
    let mut observed = false;
    let result = compile_source_batch_observed(
        source,
        "SiblingReferenceJoinAudit.scala",
        output.path(),
        SourceId::from_index(1433),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut |unit, definitions| {
            observed = true;
            let base = audit_class_named(unit, "Base");
            let left = audit_class_named(unit, "Left");
            let right = audit_class_named(unit, "Right");
            let runner = audit_class_named(unit, "Runner");
            assert_eq!(audit_class_parent(unit, base), definitions.object_class);
            assert_eq!(audit_class_parent(unit, left), base);
            assert_eq!(audit_class_parent(unit, right), base);
            let hierarchy = unit
                .class_hierarchy(definitions)
                .expect("the planned sibling classes retain their exact Base edges");
            assert!(
                hierarchy
                    .superclasses()
                    .iter()
                    .any(|edge| edge.class() == left && edge.parent() == base)
            );
            assert!(
                hierarchy
                    .superclasses()
                    .iter()
                    .any(|edge| edge.class() == right && edge.parent() == base)
            );

            for method_name in ["choose", "chooseReverse", "chooseNested"] {
                let method = audit_method_named(unit, runner, method_name);
                let signature = match unit.store.symbols.info(method.method) {
                    dotty_core::SymbolInfo::Complete(signature) => *signature,
                    other => panic!("{method_name} has complete typed signature: {other:?}"),
                };
                let Type::Method(signature) = unit.store.types.get(signature) else {
                    panic!("{method_name} has a plain method type")
                };
                assert_eq!(audit_type_symbol(unit, signature.result), Some(base));

                let body = unit.typed_ast.get(method.body.expect("method has a body"));
                let TreeKind::If(if_tree) = &body.kind else {
                    panic!("{method_name} retains its source If at the typed root")
                };
                if method_name == "chooseNested" {
                    let nested = unit.typed_ast.get(if_tree.else_branch);
                    assert!(matches!(nested.kind, TreeKind::If(_)));
                }
                let Type::Or {
                    left: then_ty,
                    right: else_ty,
                } = unit.store.types.get(body.ty)
                else {
                    panic!("{method_name} preserves the typer's sibling Or result")
                };
                let mut arms = [
                    audit_type_symbol(unit, *then_ty).expect("left union arm has canonical class"),
                    audit_type_symbol(unit, *else_ty).expect("right union arm has canonical class"),
                ];
                arms.sort_by_key(|symbol| symbol.index());
                let mut expected = [left, right];
                expected.sort_by_key(|symbol| symbol.index());
                assert_eq!(
                    arms, expected,
                    "{method_name} retains the exact sibling symbols"
                );
                let projection = unit
                    .project_sibling_union_to_expected_type(body.ty, signature.result, definitions)
                    .expect("the declared Base result proves the audited sibling projection");
                assert_eq!(projection.source_type(), body.ty);
                assert_eq!(projection.expected_type(), signature.result);
                assert_eq!(projection.representation(), JvmType::Reference(base));
                let projected_arms = projection.source_arms();
                assert_eq!(
                    projected_arms,
                    [
                        audit_type_symbol(unit, *then_ty).unwrap(),
                        audit_type_symbol(unit, *else_ty).unwrap(),
                    ],
                    "the checked projection keeps arm identity and source order"
                );
            }

            let metadata = unit
                .metadata(definitions)
                .expect("the source declarations retain exact JVM signatures");
            let choose_symbol = audit_method_named(unit, runner, "choose").method;
            let choose_descriptor = metadata
                .iter()
                .find(|class| class.identity().symbol() == runner)
                .and_then(|class| {
                    class
                        .methods()
                        .iter()
                        .find(|method| method.symbol() == choose_symbol)
                })
                .expect("choose has projected method metadata")
                .descriptor();
            assert_eq!(choose_descriptor.as_str(), "(ZI)LBase;");
            assert_eq!(
                choose_descriptor.result(),
                JvmReturnType::Value(JvmType::Reference(base))
            );

            let base_doubled = audit_method_named(unit, base, "doubled").method;
            let choose = audit_method_named(unit, runner, "choose").method;
            let run = audit_method_named(unit, runner, "run");
            let run_targets = typed_tree_targets(unit, run.body.expect("run has body"));
            assert!(run_targets.contains(&choose));
            assert!(run_targets.contains(&base_doubled));

            let accept = audit_method_named(unit, runner, "accept");
            let accept_signature = match unit.store.symbols.info(accept.method) {
                dotty_core::SymbolInfo::Complete(signature) => *signature,
                other => panic!("accept has complete typed signature: {other:?}"),
            };
            let Type::Method(accept_signature) = unit.store.types.get(accept_signature) else {
                panic!("accept has a plain method type")
            };
            assert_eq!(
                audit_type_symbol(unit, accept_signature.params[0].ty),
                Some(base),
                "the argument context is explicitly Base-typed"
            );
            let choose_body = unit.typed_ast.get(
                audit_method_named(unit, runner, "choose")
                    .body
                    .expect("choose has body"),
            );
            let argument_context_projection = unit
                .project_sibling_union_to_expected_type(
                    choose_body.ty,
                    accept_signature.params[0].ty,
                    definitions,
                )
                .expect("the actual accept formal supplies the canonical Base expectation");
            assert_eq!(
                argument_context_projection.representation(),
                JvmType::Reference(base)
            );
            let via_argument = audit_method_named(unit, runner, "viaArgument");
            let argument_targets =
                typed_tree_targets(unit, via_argument.body.expect("viaArgument has body"));
            assert!(argument_targets.contains(&choose));
            assert!(argument_targets.contains(&accept.method));
        }),
    );
    assert!(
        observed,
        "the real source reaches the typed frontend observer: {result:?}"
    );
    let artifacts = result.expect("an explicit Base context authorizes each sibling If join");
    assert_eq!(artifacts.len(), 4);
    let runner = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Runner")
        .expect("Runner artifact");
    let runner = decode_emitted_class(runner);
    for method_name in ["choose", "chooseReverse", "chooseNested"] {
        let descriptors = methods_with_descriptors(&runner);
        let descriptor = if method_name == "chooseNested" {
            "(ZZI)LBase;"
        } else {
            "(ZI)LBase;"
        };
        assert!(
            descriptors.contains(&(method_name.to_owned(), descriptor.to_owned())),
            "{method_name} descriptor missing from {descriptors:?}"
        );
        let code = emitted_method_code(&runner, method_name);
        assert!(
            decode_subset_instructions(code)
                .iter()
                .all(|(_, opcode)| *opcode != 0xc0),
            "{method_name} lowers the proven join without a checkcast"
        );
        assert!(
            stack_map_frames(&runner, method_name)
                .iter()
                .any(|frame| frame.stack == [StackMapVerificationType::Object("Base".to_owned())]),
            "{method_name} uses the exact common Base verifier frame"
        );
    }
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "acceptChoice"),
        &[
            (0xb7, "Left", "<init>", "(I)V"),
            (0xb7, "Right", "<init>", "(I)V"),
            (0xb6, "Runner", "accept", "(LBase;)I"),
        ],
    );
}

#[test]
fn inferred_sibling_reference_join_audit_preserves_the_typed_union() {
    let source = r#"
class Base
class Left extends Base
class Right extends Base
class Branch:
  def choose(flag: Boolean) =
    if flag then new Left() else new Right()
"#;
    let output = TestOutputDirectory::new("inferred-sibling-reference-join-audit");
    let mut observed = false;
    let result = compile_source_batch_observed(
        source,
        "InferredSiblingReferenceJoinAudit.scala",
        output.path(),
        SourceId::from_index(1434),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut |unit, _| {
            observed = true;
            let branch = audit_class_named(unit, "Branch");
            let method = audit_method_named(unit, branch, "choose");
            let signature = match unit.store.symbols.info(method.method) {
                dotty_core::SymbolInfo::Complete(signature) => *signature,
                other => panic!("choose has complete typed signature: {other:?}"),
            };
            let Type::Method(signature) = unit.store.types.get(signature) else {
                panic!("choose has a plain method type")
            };
            assert!(matches!(
                unit.store.types.get(signature.result),
                Type::Or { .. }
            ));
            let body = unit.typed_ast.get(method.body.expect("choose has a body"));
            assert!(matches!(body.kind, TreeKind::If(_)));
            assert!(matches!(unit.store.types.get(body.ty), Type::Or { .. }));
        }),
    );
    assert!(
        observed,
        "the inferred source reached the typed observer: {result:?}"
    );
    assert!(matches!(
        result,
        Err(PipelineFailure::Codegen(
            JvmCodegenError::UnsupportedJvmType {
                kind: dotty_jvm::UnsupportedJvmTypeKind::Union,
                ..
            }
        ))
    ));
    assert!(output.class_files().is_empty());
}

#[test]
fn canonical_long_descriptors_and_category_two_parameter_slots_are_projected() {
    let source = fs::read_to_string(fixture_path("LongDescriptors.scala"))
        .expect("read focused Long descriptor fixture");
    let output = TestOutputDirectory::new("scala-long-descriptor-metadata");
    let mut observed = false;
    let result = compile_source_batch_with_source_observer(
        &source,
        "LongDescriptors.scala",
        output.path(),
        SourceId::from_index(1514),
        SourceUnitCardinality::ExactlyOne,
        &mut |_, _, unit, definitions| {
            observed = true;
            let class = audit_class_named(unit, "LongDescriptors");
            let metadata = unit
                .metadata(definitions)
                .expect("canonical Long signatures project to metadata");
            let class_metadata = metadata
                .iter()
                .find(|metadata| metadata.identity().symbol() == class)
                .expect("fixture class has metadata");
            let constructor = class_metadata
                .constructor()
                .expect("explicit Long constructor has metadata");
            assert_eq!(constructor.descriptor().as_str(), "(J)V");
            assert_eq!(
                constructor
                    .parameters()
                    .iter()
                    .map(|parameter| (parameter.ty(), parameter.slot(), parameter.width()))
                    .collect::<Vec<_>>(),
                vec![(JvmType::Long, 1, 2)]
            );

            let echo = class_metadata
                .methods()
                .iter()
                .find(|method| method.name() == "echo")
                .expect("echo method metadata");
            assert_eq!(echo.descriptor().as_str(), "(J)J");
            assert_eq!(
                echo.parameters()
                    .iter()
                    .map(|parameter| (parameter.ty(), parameter.slot(), parameter.width()))
                    .collect::<Vec<_>>(),
                vec![(JvmType::Long, 1, 2)]
            );

            let mixed = class_metadata
                .methods()
                .iter()
                .find(|method| method.name() == "mixed")
                .expect("mixed method metadata");
            assert_eq!(mixed.descriptor().as_str(), "(ZIJI)J");
            assert_eq!(
                mixed
                    .parameters()
                    .iter()
                    .map(|parameter| (parameter.ty(), parameter.slot(), parameter.width()))
                    .collect::<Vec<_>>(),
                vec![
                    (JvmType::Boolean, 1, 1),
                    (JvmType::Int, 2, 1),
                    (JvmType::Long, 3, 2),
                    (JvmType::Int, 5, 1),
                ]
            );
        },
    );
    assert!(
        observed,
        "source reaches JVM metadata projection: {result:?}"
    );
    let artifacts = result.expect("straight-line Long methods emit after #1516");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(output.class_files(), vec![artifacts[0].path.clone()]);
}

#[test]
fn long_literals_parameters_locals_and_returns_verify_and_execute() {
    let first_output = TestOutputDirectory::new("long-values-first");
    let second_output = TestOutputDirectory::new("long-values-second");
    let first = compile_fixture("LongValues.scala", first_output.path()).unwrap();
    let second = compile_fixture("LongValues.scala", second_output.path()).unwrap();

    assert_eq!(first.class.internal_name(), "Runner");
    assert_eq!(first.class.bytes(), second.class.bytes());
    let mut reader = Reader::new(first.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("Runner should decode completely");
    assert!(reader.is_at_end());
    class
        .validate_references()
        .expect("Long constant pool and method references are valid");

    let signatures = class
        .methods
        .iter()
        .map(|method| {
            (
                class.constant_pool.utf8(method.name_index).unwrap(),
                class.constant_pool.utf8(method.descriptor_index).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        signatures,
        [
            ("<init>", "()V"),
            ("zero", "()J"),
            ("one", "()J"),
            ("constant", "()J"),
            ("identity", "(J)J"),
            ("mixed", "(ZIJI)J"),
            ("roundTrip", "(J)J"),
            ("reassigned", "(JJ)J"),
            ("explicit", "(J)J"),
        ]
    );
    let long_index = (1..=u16::MAX)
        .find(|index| {
            matches!(
                class.constant_pool.get(ConstantPoolIndex(*index)),
                Some(ConstantPoolEntry::Long(9_000_000_000))
            )
        })
        .expect("Long literal constant pool entry");
    assert!(
        matches!(
            class.constant_pool.get(ConstantPoolIndex(long_index)),
            Some(ConstantPoolEntry::Long(9_000_000_000))
        ),
        "nontrivial Long literal is preserved as a CONSTANT_Long"
    );

    assert_eq!(emitted_method_code(&class, "zero"), [0x09, 0xad]);
    assert_eq!(emitted_method_code(&class, "one"), [0x0a, 0xad]);
    let constant = emitted_method_code(&class, "constant");
    assert_eq!(constant[0], 0x14, "non-small literal uses ldc2_w");
    assert_eq!(*constant.last().unwrap(), 0xad);
    assert_eq!(emitted_method_code(&class, "identity"), [0x1f, 0xad]);
    assert_eq!(emitted_method_code(&class, "mixed"), [0x21, 0xad]);
    assert_eq!(
        emitted_method_code(&class, "roundTrip"),
        [0x1f, 0x42, 0x21, 0xad]
    );
    assert_eq!(
        emitted_method_code(&class, "reassigned"),
        [0x1f, 0x37, 0x05, 0x21, 0x37, 0x05, 0x16, 0x05, 0xad]
    );
    assert_eq!(emitted_method_code(&class, "explicit"), [0x1f, 0xad]);
    for name in [
        "zero",
        "one",
        "constant",
        "identity",
        "mixed",
        "roundTrip",
        "reassigned",
        "explicit",
    ] {
        assert_eq!(
            code_max_stack(&class, name),
            2,
            "{name} counts Long as two stack slots"
        );
    }
    assert_eq!(code_max_locals(&class, "identity"), 3);
    assert_eq!(code_max_locals(&class, "mixed"), 6);
    assert_eq!(code_max_locals(&class, "roundTrip"), 5);
    assert_eq!(code_max_locals(&class, "reassigned"), 7);

    if require_jdk_or_skip() {
        let output_path = first_output.path().to_str().expect("UTF-8 output path");
        let javap = Command::new("javap")
            .args(["-v", "-c", "-classpath", output_path, "Runner"])
            .output()
            .expect("start javap");
        assert!(
            javap.status.success(),
            "javap rejected Long Runner\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javap.stdout),
            String::from_utf8_lossy(&javap.stderr)
        );
        let javap_output = String::from_utf8_lossy(&javap.stdout);
        for expected in [
            "descriptor: ()J",
            "descriptor: (J)J",
            "descriptor: (ZIJI)J",
            "lload_1",
            "lload_3",
            "lstore_3",
            "lstore",
            "ldc2_w",
            "lreturn",
        ] {
            assert!(
                javap_output.contains(expected),
                "javap output omitted {expected:?}:\n{javap_output}"
            );
        }

        let harness_path = first_output.path().join("LongValuesHarness.java");
        fs::write(
            &harness_path,
            r#"public final class LongValuesHarness {
  public static void main(String[] args) {
    Runner value = new Runner();
    if (value.zero() != 0L || value.one() != 1L || value.constant() != 9000000000L) throw new AssertionError("Long constants");
    if (value.identity(Long.MAX_VALUE) != Long.MAX_VALUE || value.identity(Long.MIN_VALUE) != Long.MIN_VALUE || value.identity(-7L) != -7L) throw new AssertionError("parameter and return");
    if (value.mixed(true, 4, Long.MIN_VALUE, 8) != Long.MIN_VALUE) throw new AssertionError("mixed parameter slots");
    if (value.roundTrip(-9000000001L) != -9000000001L) throw new AssertionError("immutable local");
    if (value.reassigned(11L, Long.MAX_VALUE) != Long.MAX_VALUE) throw new AssertionError("mutable local assignment");
    if (value.explicit(Long.MIN_VALUE) != Long.MIN_VALUE) throw new AssertionError("explicit return");
    System.out.println("long values ok");
  }
}
"#,
        )
        .expect("write LongValuesHarness.java");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness_path)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected LongValuesHarness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "LongValuesHarness",
            ])
            .output()
            .expect("start Java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected or failed Long Runner\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "long values ok"
        );
    }
}

#[test]
fn float_double_literals_parameters_locals_and_returns_verify_and_execute() {
    let first_output = TestOutputDirectory::new("float-double-values-first");
    let second_output = TestOutputDirectory::new("float-double-values-second");
    let first = compile_fixture("FloatDoubleValues.scala", first_output.path()).unwrap();
    let second = compile_fixture("FloatDoubleValues.scala", second_output.path()).unwrap();

    assert_eq!(first.class.internal_name(), "Runner");
    assert_eq!(first.class.bytes(), second.class.bytes());
    let mut reader = Reader::new(first.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("Runner should decode completely");
    assert!(reader.is_at_end());
    class
        .validate_references()
        .expect("Float/Double constant-pool references are valid");
    let methods = methods_with_descriptors(&class);
    assert_eq!(
        methods,
        [
            ("<init>".to_owned(), "()V".to_owned()),
            ("floatZero".to_owned(), "()F".to_owned()),
            ("floatOne".to_owned(), "()F".to_owned()),
            ("floatTwo".to_owned(), "()F".to_owned()),
            ("doubleZero".to_owned(), "()D".to_owned()),
            ("doubleOne".to_owned(), "()D".to_owned()),
            ("floatConstant".to_owned(), "()F".to_owned()),
            ("doubleConstant".to_owned(), "()D".to_owned()),
            ("identityFloat".to_owned(), "(F)F".to_owned()),
            ("identityDouble".to_owned(), "(D)D".to_owned()),
            ("roundTripFloat".to_owned(), "(F)F".to_owned()),
            ("roundTripDouble".to_owned(), "(D)D".to_owned()),
            ("reassignedFloat".to_owned(), "(FF)F".to_owned()),
            ("reassignedDouble".to_owned(), "(DD)D".to_owned()),
            ("mixed".to_owned(), "(FDJ)D".to_owned()),
            ("explicitFloat".to_owned(), "(F)F".to_owned()),
            ("explicitDouble".to_owned(), "(D)D".to_owned()),
            ("discardedFloat".to_owned(), "(F)F".to_owned()),
        ]
    );

    assert_eq!(emitted_method_code(&class, "floatZero"), [0x0b, 0xae]);
    assert_eq!(emitted_method_code(&class, "floatOne"), [0x0c, 0xae]);
    assert_eq!(emitted_method_code(&class, "floatTwo"), [0x0d, 0xae]);
    assert_eq!(emitted_method_code(&class, "doubleZero"), [0x0e, 0xaf]);
    assert_eq!(emitted_method_code(&class, "doubleOne"), [0x0f, 0xaf]);
    for (name, tag, expected_bits) in [
        ("floatConstant", 4, 0x3fc0_0000u64),
        ("doubleConstant", 6, 0x400c_0000_0000_0000u64),
    ] {
        let code = emitted_method_code(&class, name);
        let index = match (tag, code[0]) {
            (4, 0x12) => u16::from(code[1]),
            (4, 0x13) | (6, 0x14) => u16::from_be_bytes([code[1], code[2]]),
            _ => panic!("{name} must use the correct category-specific constant load: {code:?}"),
        };
        match class.constant_pool.get(ConstantPoolIndex(index)) {
            Some(ConstantPoolEntry::Float(value)) if tag == 4 => {
                assert_eq!(u64::from(value.to_bits()), expected_bits, "{name} raw bits");
            }
            Some(ConstantPoolEntry::Double(value)) if tag == 6 => {
                assert_eq!(value.to_bits(), expected_bits, "{name} raw bits");
            }
            found => panic!("{name} resolves to the wrong constant-pool entry: {found:?}"),
        }
        assert_eq!(*code.last().unwrap(), if tag == 4 { 0xae } else { 0xaf });
    }
    assert_eq!(emitted_method_code(&class, "identityFloat"), [0x23, 0xae]);
    assert_eq!(emitted_method_code(&class, "identityDouble"), [0x27, 0xaf]);
    assert_eq!(
        emitted_method_code(&class, "roundTripFloat"),
        [0x23, 0x45, 0x24, 0xae]
    );
    assert_eq!(
        emitted_method_code(&class, "roundTripDouble"),
        [0x27, 0x4a, 0x29, 0xaf]
    );
    assert_eq!(
        emitted_method_code(&class, "reassignedFloat"),
        [0x23, 0x46, 0x24, 0x46, 0x25, 0xae]
    );
    assert_eq!(
        emitted_method_code(&class, "reassignedDouble"),
        [0x27, 0x39, 5, 0x29, 0x39, 5, 0x18, 5, 0xaf]
    );
    assert_eq!(emitted_method_code(&class, "explicitFloat"), [0x23, 0xae]);
    assert_eq!(emitted_method_code(&class, "explicitDouble"), [0x27, 0xaf]);
    let discarded_float = emitted_method_code(&class, "discardedFloat");
    assert!(
        discarded_float.contains(&0x57),
        "Float discard uses category-one pop"
    );
    assert_eq!(*discarded_float.last().unwrap(), 0xae);

    for (name, max_stack, max_locals) in [
        ("identityFloat", 1, 2),
        ("identityDouble", 2, 3),
        ("roundTripFloat", 1, 3),
        ("roundTripDouble", 2, 5),
        ("reassignedFloat", 1, 4),
        ("reassignedDouble", 2, 7),
        ("mixed", 2, 10),
    ] {
        assert_eq!(code_max_stack(&class, name), max_stack, "{name} max_stack");
        assert_eq!(
            code_max_locals(&class, name),
            max_locals,
            "{name} max_locals"
        );
    }

    if !require_jdk_or_skip() {
        return;
    }
    let classpath = first_output.path().to_str().expect("UTF-8 output path");
    let javap = Command::new("javap")
        .args(["-v", "-c", "-classpath", classpath, "Runner"])
        .output()
        .expect("start javap");
    assert!(
        javap.status.success(),
        "javap rejected Float/Double Runner\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javap.stdout),
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap_output = String::from_utf8_lossy(&javap.stdout);
    for expected in [
        "descriptor: ()F",
        "descriptor: ()D",
        "descriptor: (F)F",
        "descriptor: (D)D",
        "descriptor: (FF)F",
        "descriptor: (DD)D",
        "fload",
        "dload",
        "fstore",
        "dstore",
        "freturn",
        "dreturn",
        "ldc",
        "ldc2_w",
    ] {
        assert!(
            javap_output.contains(expected),
            "javap omitted {expected:?}:\n{javap_output}"
        );
    }
    let harness = first_output.path().join("FloatDoubleValuesHarness.java");
    fs::write(
        &harness,
        r#"public final class FloatDoubleValuesHarness {
  private static void check(boolean ok, String label) {
    if (!ok) throw new AssertionError(label);
  }
  public static void main(String[] args) {
    Runner runner = new Runner();
    check(Float.floatToRawIntBits(runner.floatZero()) == 0x00000000, "float +0");
    check(Float.floatToRawIntBits(runner.floatOne()) == 0x3f800000, "float 1");
    check(Float.floatToRawIntBits(runner.floatTwo()) == 0x40000000, "float 2");
    check(Double.doubleToRawLongBits(runner.doubleZero()) == 0L, "double +0");
    check(Double.doubleToRawLongBits(runner.doubleOne()) == 0x3ff0000000000000L, "double 1");
    check(Float.floatToRawIntBits(runner.floatConstant()) == 0x3fc00000, "float CP bits");
    check(Double.doubleToRawLongBits(runner.doubleConstant()) == 0x400c000000000000L, "double CP bits");
    float negativeZeroF = Float.intBitsToFloat(0x80000000);
    double negativeZeroD = Double.longBitsToDouble(0x8000000000000000L);
    check(Float.floatToRawIntBits(runner.roundTripFloat(negativeZeroF)) == 0x80000000, "float -0 local");
    check(Float.floatToRawIntBits(runner.reassignedFloat(1.5f, negativeZeroF)) == 0x80000000, "float -0 assignment");
    check(Double.doubleToRawLongBits(runner.roundTripDouble(negativeZeroD)) == 0x8000000000000000L, "double -0 local");
    check(Double.doubleToRawLongBits(runner.reassignedDouble(3.5d, negativeZeroD)) == 0x8000000000000000L, "double -0 assignment");
    check(Float.isNaN(runner.roundTripFloat(Float.intBitsToFloat(0x7fc00042))), "float NaN");
    check(Double.isNaN(runner.roundTripDouble(Double.longBitsToDouble(0x7ff8000000000042L))), "double NaN");
    check(runner.identityFloat(-17.25f) == -17.25f && runner.identityDouble(-19.5d) == -19.5d, "parameters");
    check(runner.mixed(1.5f, 3.5d, Long.MIN_VALUE) == 3.5d, "mixed slots");
    check(runner.explicitFloat(-2.5f) == -2.5f && runner.explicitDouble(-4.25d) == -4.25d, "explicit returns");
    check(runner.discardedFloat(8.0f) == 1.5f, "discarded Float");
    System.out.println("Float/Double values ok");
  }
}
"#,
    )
    .expect("write FloatDoubleValuesHarness.java");
    let javac = Command::new("javac")
        .args(["-classpath", classpath, "-d", classpath])
        .arg(&harness)
        .output()
        .expect("start javac");
    assert!(
        javac.status.success(),
        "javac rejected FloatDoubleValuesHarness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            classpath,
            "FloatDoubleValuesHarness",
        ])
        .output()
        .expect("start JVM verifier");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected Float/Double Runner\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "Float/Double values ok"
    );
}

#[test]
fn discarded_long_values_remain_unsupported_and_arithmetic_fails_in_typer() {
    let output = TestOutputDirectory::new("LongDiscard.scala");
    let error = compile_source(
        "class LongDiscard:\n  def valueTwice(value: Long): Long =\n    value\n    value\n",
        "LongDiscard.scala",
        output.path(),
    )
    .expect_err("discarding a Long requires unsupported pop2");
    assert!(matches!(error, PipelineFailure::Codegen(_)));
    assert!(output.class_files().is_empty());

    let output = TestOutputDirectory::new("LongMixedJoin.scala");
    let error = compile_source(
        "class LongMixedJoin:\n  def choose(flag: Boolean, left: Long, right: Int): Long = if flag then left else right\n",
        "LongMixedJoin.scala",
        output.path(),
    )
    .expect_err("Long and Int branches do not gain an implicit numeric conversion");
    assert!(
        matches!(
            &error,
            PipelineFailure::Codegen(JvmCodegenError::ReferenceClassNotInJvmPlan { .. })
        ),
        "mixed Long/Int join fails with its exact backend plan error: {error:?}"
    );
    assert!(output.class_files().is_empty());

    let output = TestOutputDirectory::new("LongArithmetic.scala");
    let error = compile_source(
        "class LongArithmetic:\n  def increment(value: Long): Long = value + 1L\n",
        "LongArithmetic.scala",
        output.path(),
    )
    .expect_err("Typer currently cannot complete primitive Long arithmetic lookup");
    assert!(
        matches!(
            &error,
            PipelineFailure::Typer(TyperError::MemberLookup(error))
                if matches!(
                    **error,
                    dotty_typer::MemberLookupError::ClassInfoUnavailable { .. }
                )
        ),
        "Long arithmetic currently stops at missing scala.Long class info: {error:?}"
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn mismatched_long_fields_and_formals_fail_at_typer_without_class_artifacts() {
    let cases = [
        (
            "LongToIntField",
            "class LongToIntField:\n  private val value: Int = 1L\n",
        ),
        (
            "LongToBooleanField",
            "class LongToBooleanField:\n  private val value: Boolean = 1L\n",
        ),
        (
            "LongToReferenceField",
            "class LongBox(initial: Long)\nclass LongToReferenceField:\n  private val value: LongBox = 1L\n",
        ),
    ];
    for (label, source) in cases {
        let output = TestOutputDirectory::new(label);
        let error = compile_source_classes(source, &format!("{label}.scala"), output.path())
            .expect_err("a Long initializer cannot be retyped as another field type");
        let expected_error = if label == "LongToReferenceField" {
            matches!(
                &error,
                PipelineFailure::Typer(TyperError::ExpectedExpressionConformanceUnsupported { .. })
            )
        } else {
            matches!(
                &error,
                PipelineFailure::Typer(TyperError::ExpectedExpressionTypeMismatch { .. })
            )
        };
        assert!(
            expected_error,
            "{label} must fail at the exact contextual Typer check: {error:?}"
        );
        assert!(
            output.class_files().is_empty(),
            "{label} failure publishes no class files"
        );
    }

    let cases = [
        (
            "LongToIntFormal",
            "class LongToIntFormal:\n  def consume(value: Int): Int = value\n  def use(value: Long): Int = consume(value)\n",
        ),
        (
            "LongToBooleanFormal",
            "class LongToBooleanFormal:\n  def consume(value: Boolean): Boolean = value\n  def use(value: Long): Boolean = consume(value)\n",
        ),
        (
            "LongToReferenceFormal",
            "class LongBox(initial: Long)\nclass LongToReferenceFormal:\n  def consume(value: LongBox): LongBox = value\n  def use(value: Long): LongBox = consume(value)\n",
        ),
    ];
    for (label, source) in cases {
        let output = TestOutputDirectory::new(label);
        let error = compile_source_classes(source, &format!("{label}.scala"), output.path())
            .expect_err("a Long argument cannot satisfy another formal parameter type");
        let expected_error = if label == "LongToReferenceFormal" {
            matches!(
                &error,
                PipelineFailure::Typer(TyperError::ApplicationArgumentConformanceUnsupported {
                    argument_index: 0,
                    ..
                })
            )
        } else {
            matches!(
                &error,
                PipelineFailure::Typer(TyperError::ApplicationArgumentTypeMismatch {
                    argument_index: 0,
                    ..
                })
            )
        };
        assert!(
            expected_error,
            "{label} must fail at the exact Typer application argument check: {error:?}"
        );
        assert!(
            output.class_files().is_empty(),
            "{label} failure publishes no class files"
        );
    }
}

#[test]
fn long_comparison_and_mixed_object_join_fail_closed_at_typer() {
    let cases = [
        (
            "LongComparison",
            "class LongComparison:\n  def less(left: Long, right: Long): Boolean = left < right\n",
            "Long comparison remains unsupported by the source Typer",
        ),
        (
            "LongObjectJoin",
            "class LongBox(initial: Long)\nclass LongObjectJoin:\n  def choose(flag: Boolean, value: Long, box: LongBox): LongBox = if flag then value else box\n",
            "a Long and a reference do not acquire a common source result type",
        ),
    ];
    for (label, source, expectation) in cases {
        let output = TestOutputDirectory::new(label);
        let error = compile_source_classes(source, &format!("{label}.scala"), output.path())
            .expect_err(expectation);
        let expected_error = if label == "LongComparison" {
            matches!(
                &error,
                PipelineFailure::Typer(TyperError::MemberLookup(error))
                    if matches!(
                        **error,
                        dotty_typer::MemberLookupError::ClassInfoUnavailable { .. }
                    )
            )
        } else {
            matches!(
                &error,
                PipelineFailure::Typer(TyperError::IfBranchJoinUnsupported { .. })
            )
        };
        assert!(
            expected_error,
            "{label} must fail at its exact first Typer stage: {error:?}"
        );
        assert!(
            output.class_files().is_empty(),
            "{label} failure publishes no class files"
        );
    }
}

#[test]
fn source_typed_float_double_metadata_projects_descriptors_and_parameter_slots() {
    let source = r#"
class NumericMetadata(initialF: Float, initialD: Double):
  private var fieldF: Float = 0.0f
  private var fieldD: Double = 0.0
  def readF(): Float = fieldF
  def readD(): Double = fieldD
  def floatValue(): Float = 1.5f
  def doubleValue(): Double = 3.5
  def mixed(i: Int, f: Float, d: Double, j: Long): Double = 3.5
  def reverse(j: Long, d: Double, f: Float, flag: Boolean): Unit = ()
  def pair(f: Float, first: Double, second: Double): Float = 1.5f
"#;
    let output = TestOutputDirectory::new("float-double-metadata-only");
    let mut observed = false;
    let mut observe = |_: &AstArena<Untyped>,
                       _: &SourceSemanticIndex,
                       unit: &mut JvmCompilationUnit<'_>,
                       definitions: &Definitions| {
        observed = true;
        let metadata = unit
            .metadata(definitions)
            .expect("same-session typed Float/Double metadata projects");
        let class = metadata
            .iter()
            .find(|metadata| metadata.identity().internal_name() == "NumericMetadata")
            .expect("the source class is present in the metadata batch");
        let constructor = class
            .constructor()
            .expect("the primary constructor has metadata");
        assert_eq!(constructor.descriptor().as_str(), "(FD)V");
        assert_eq!(
            constructor
                .parameters()
                .iter()
                .map(|parameter| (parameter.slot(), parameter.width()))
                .collect::<Vec<_>>(),
            [(1, 1), (2, 2)]
        );
        for (name, descriptor) in [("fieldF", "F"), ("fieldD", "D")] {
            assert_eq!(
                class
                    .fields()
                    .iter()
                    .find(|field| field.name() == name)
                    .expect("floating field has metadata")
                    .descriptor(),
                descriptor,
                "{name}"
            );
        }
        let method = |name: &str| {
            class
                .methods()
                .iter()
                .find(|method| method.name() == name)
                .unwrap_or_else(|| panic!("source method {name} has metadata"))
        };
        for (name, descriptor) in [
            ("readF", "()F"),
            ("readD", "()D"),
            ("floatValue", "()F"),
            ("doubleValue", "()D"),
            ("mixed", "(IFDJ)D"),
            ("reverse", "(JDFZ)V"),
            ("pair", "(FDD)F"),
        ] {
            assert_eq!(method(name).descriptor().as_str(), descriptor, "{name}");
        }
        for name in ["readF", "readD"] {
            unit.lower_method_body(
                class.identity().symbol(),
                method(name).symbol(),
                definitions,
            )
            .unwrap_or_else(|error| panic!("{name} lowers its exact FP field read: {error:?}"));
        }
        assert_eq!(
            method("mixed")
                .parameters()
                .iter()
                .map(|parameter| (parameter.slot(), parameter.width()))
                .collect::<Vec<_>>(),
            [(1, 1), (2, 1), (3, 2), (5, 2)]
        );
        assert_eq!(
            method("reverse")
                .parameters()
                .iter()
                .map(|parameter| (parameter.slot(), parameter.width()))
                .collect::<Vec<_>>(),
            [(1, 2), (3, 2), (5, 1), (6, 1)]
        );
        assert_eq!(
            method("pair")
                .parameters()
                .iter()
                .map(|parameter| (parameter.slot(), parameter.width()))
                .collect::<Vec<_>>(),
            [(1, 1), (2, 2), (4, 2)]
        );
    };
    let result = compile_source_batch_with_source_observer(
        source,
        "NumericMetadata.scala",
        output.path(),
        SourceId::from_index(1535),
        SourceUnitCardinality::ExactlyOne,
        &mut observe,
    );
    assert!(observed, "real source typing reached the metadata observer");
    assert!(result.is_ok(), "FP field reads now lower: {result:?}");
    assert_eq!(output.class_files().len(), 1);
}

#[test]
fn float_double_calls_and_control_flow_verify_and_execute() {
    let first_output = TestOutputDirectory::new("float-double-calls-first");
    let second_output = TestOutputDirectory::new("float-double-calls-second");
    let first = compile_fixture_classes("FloatDoubleCalls.scala", first_output.path()).unwrap();
    let second = compile_fixture_classes("FloatDoubleCalls.scala", second_output.path()).unwrap();
    assert_eq!(
        first
            .iter()
            .map(|artifact| (
                artifact.class.internal_name(),
                artifact.class.bytes().to_vec()
            ))
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|artifact| (
                artifact.class.internal_name(),
                artifact.class.bytes().to_vec()
            ))
            .collect::<Vec<_>>()
    );

    let decode = |name: &str| {
        let artifact = first
            .iter()
            .find(|artifact| artifact.class.internal_name() == name)
            .unwrap_or_else(|| panic!("missing emitted class {name}"));
        decode_emitted_class(artifact)
    };
    let runner = decode("Runner");
    assert_eq!(
        methods_with_descriptors(&runner),
        [
            ("<init>".to_owned(), "()V".to_owned()),
            ("identityF".to_owned(), "(F)F".to_owned()),
            ("identityD".to_owned(), "(D)D".to_owned()),
            ("pickF".to_owned(), "(ZFF)F".to_owned()),
            ("pickD".to_owned(), "(ZDD)D".to_owned()),
            ("loop".to_owned(), "(IDD)D".to_owned()),
            ("forward".to_owned(), "(LNumericBox;)D".to_owned()),
            ("construct".to_owned(), "(FD)D".to_owned()),
            ("constructNested".to_owned(), "(FD)D".to_owned()),
            ("mixed".to_owned(), "(IFDJ)D".to_owned()),
        ]
    );
    let numeric_child = decode("NumericChild");
    assert_eq!(
        methods_with_descriptors(&numeric_child),
        [("<init>".to_owned(), "(FD)V".to_owned())]
    );
    let child_constructor = emitted_method_code(&numeric_child, "<init>");
    assert_opcode_sequence(child_constructor, &[0x2a, 0x23, 0x28, 0xb7, 0xb1]);
    assert_method_reference(
        &numeric_child,
        child_constructor,
        0xb7,
        "NumericBox",
        "<init>",
        "(FD)V",
    );
    assert_opcode_sequence(
        emitted_method_code(&runner, "pickF"),
        &[0x1b, 0x99, 0x24, 0xa7, 0x25, 0xae],
    );
    assert_eq!(code_max_locals(&runner, "pickD"), 6);
    assert_eq!(code_max_stack(&runner, "pickD"), 2);
    assert_eq!(code_max_locals(&runner, "loop"), 9);
    assert_eq!(code_max_stack(&runner, "loop"), 2);
    assert!(
        stack_map_frames(&runner, "pickF")
            .iter()
            .any(|frame| frame.stack == [StackMapVerificationType::Float])
    );
    assert!(
        stack_map_frames(&runner, "pickD")
            .iter()
            .any(|frame| frame.stack == [StackMapVerificationType::Double])
    );
    assert!(stack_map_frames(&runner, "loop").iter().any(|frame| {
        frame.locals
            == [
                StackMapVerificationType::Object("Runner".to_owned()),
                StackMapVerificationType::Integer,
                StackMapVerificationType::Double,
                StackMapVerificationType::Double,
                StackMapVerificationType::Double,
                StackMapVerificationType::Integer,
            ]
    }));
    assert_method_reference(
        &runner,
        emitted_method_code(&runner, "forward"),
        0xb6,
        "NumericBox",
        "getD",
        "()D",
    );
    assert_method_reference(
        &runner,
        emitted_method_code(&runner, "construct"),
        0xb7,
        "NumericBox",
        "<init>",
        "(FD)V",
    );
    assert_method_reference(
        &runner,
        emitted_method_code(&runner, "constructNested"),
        0xb7,
        "NumericBox",
        "<init>",
        "(FD)V",
    );
    assert_eq!(code_max_locals(&runner, "mixed"), 7);

    if require_jdk_or_skip() {
        let harness_path = first_output.path().join("FloatDoubleCallsHarness.java");
        fs::write(
            &harness_path,
            r#"public final class FloatDoubleCallsHarness {
  public static void main(String[] args) {
    Runner runner = new Runner();
    if (runner.identityF(-0.0f) != 0.0f || Float.floatToRawIntBits(runner.identityF(-0.0f)) != 0x80000000) throw new AssertionError("Float call");
    if (runner.identityD(-0.0d) != 0.0d || Double.doubleToRawLongBits(runner.identityD(-0.0d)) != 0x8000000000000000L) throw new AssertionError("Double call");
    if (!Float.isNaN(runner.identityF(Float.NaN)) || !Double.isNaN(runner.identityD(Double.NaN))) throw new AssertionError("NaN calls");
    if (runner.pickF(true, 1.25f, 2.5f) != 1.25f || runner.pickF(false, 1.25f, 2.5f) != 2.5f) throw new AssertionError("Float branches");
    if (runner.pickD(true, 1.25d, 2.5d) != 1.25d || runner.pickD(false, 1.25d, 2.5d) != 2.5d) throw new AssertionError("Double branches");
    if (runner.loop(0, 11.0d, 22.0d) != 11.0d || runner.loop(1, 11.0d, 22.0d) != 22.0d || runner.loop(3, 11.0d, 22.0d) != 22.0d || runner.loop(-1, 11.0d, 22.0d) != 11.0d) throw new AssertionError("Double loop");
    NumericBox box = new NumericBox(3.0f, 4.0d);
    if (runner.forward(box) != 4.0d || runner.construct(1.5f, 3.5d) != 3.5d || runner.mixed(7, 1.5f, 3.5d, 9000000000L) != 3.5d) throw new AssertionError("mixed calls");
    if (runner.constructNested(2.5f, 6.5d) != 6.5d) throw new AssertionError("nested floating calls");
    NumericChild child = new NumericChild(3.5f, 7.5d);
    if (child.getF() != 3.5f || child.getD() != 7.5d) throw new AssertionError("direct superclass forwarding");
  }
}
"#,
        )
        .expect("write FloatDoubleCallsHarness.java");
        let javac = Command::new("javac")
            .args(["-classpath", first_output.path().to_str().unwrap(), "-d"])
            .arg(first_output.path())
            .arg(&harness_path)
            .output()
            .expect("start javac");
        assert!(javac.status.success(), "javac failed: {javac:?}");
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                first_output.path().to_str().unwrap(),
                "FloatDoubleCallsHarness",
            ])
            .output()
            .expect("start java");
        assert!(java.status.success(), "java verification failed: {java:?}");
    }
}

#[test]
fn floating_point_category_two_acceptance_batch_is_complete_deterministic_and_jdk_verified() {
    let first_output = TestOutputDirectory::new("floating-point-category-two-first");
    let second_output = TestOutputDirectory::new("floating-point-category-two-second");
    let first = compile_fixture_classes("FloatingPointCategory2.scala", first_output.path())
        .expect("compile the complete Float/Double source batch");
    let second = compile_fixture_classes("FloatingPointCategory2.scala", second_output.path())
        .expect("compile the same source in an independent frontend session");
    let artifact_bytes = |artifacts: &[EmittedArtifact]| {
        artifacts
            .iter()
            .map(|artifact| {
                (
                    artifact.class.internal_name().to_owned(),
                    artifact.class.bytes().to_vec(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        artifact_bytes(&first),
        artifact_bytes(&second),
        "all three independently compiled class files and their publication order are deterministic"
    );
    assert_eq!(
        first
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["NumericBox", "NumericConstants", "Runner"]
    );

    let decode = |name: &str| {
        let artifact = first
            .iter()
            .find(|artifact| artifact.class.internal_name() == name)
            .unwrap_or_else(|| panic!("missing emitted class {name}"));
        let class = decode_emitted_class(artifact);
        assert_eq!(class.version.major, 65, "{name} class-file major version");
        assert!(class.access_flags.is_public() && class.access_flags.is_super());
        class
    };
    let numeric_box = decode("NumericBox");
    let constants = decode("NumericConstants");
    let runner = decode("Runner");

    let field_metadata = |class: &ClassFile<'_>| {
        class
            .fields
            .iter()
            .map(|field| {
                (
                    class
                        .constant_pool
                        .utf8(field.name_index)
                        .unwrap()
                        .to_owned(),
                    class
                        .constant_pool
                        .utf8(field.descriptor_index)
                        .unwrap()
                        .to_owned(),
                    field.access_flags.0,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        field_metadata(&numeric_box),
        [
            (
                "f".to_owned(),
                "F".to_owned(),
                dotty_classfile::access_flags::ACC_PRIVATE
            ),
            (
                "d".to_owned(),
                "D".to_owned(),
                dotty_classfile::access_flags::ACC_PRIVATE
            ),
        ]
    );
    let private_final =
        dotty_classfile::access_flags::ACC_PRIVATE | dotty_classfile::access_flags::ACC_FINAL;
    assert_eq!(
        field_metadata(&constants),
        [
            ("f".to_owned(), "F".to_owned(), private_final),
            ("d".to_owned(), "D".to_owned(), private_final),
        ]
    );
    assert_eq!(
        methods_with_descriptors(&numeric_box),
        [
            ("<init>".to_owned(), "(FD)V".to_owned()),
            ("getF".to_owned(), "()F".to_owned()),
            ("getD".to_owned(), "()D".to_owned()),
            ("setF".to_owned(), "(F)V".to_owned()),
            ("setD".to_owned(), "(D)V".to_owned()),
        ]
    );
    assert_eq!(
        methods_with_descriptors(&constants),
        [
            ("<init>".to_owned(), "()V".to_owned()),
            ("getF".to_owned(), "()F".to_owned()),
            ("getD".to_owned(), "()D".to_owned()),
        ]
    );
    assert_eq!(
        methods_with_descriptors(&runner),
        [
            ("<init>".to_owned(), "()V".to_owned()),
            ("floatValue".to_owned(), "()F".to_owned()),
            ("doubleValue".to_owned(), "()D".to_owned()),
            ("roundTripF".to_owned(), "(F)F".to_owned()),
            ("roundTripD".to_owned(), "(D)D".to_owned()),
            ("replaceD".to_owned(), "(DD)D".to_owned()),
            ("chooseF".to_owned(), "(ZFF)F".to_owned()),
            ("chooseD".to_owned(), "(ZDD)D".to_owned()),
            ("loop".to_owned(), "(IDD)D".to_owned()),
            ("forwardD".to_owned(), "(LNumericBox;)D".to_owned()),
            ("construct".to_owned(), "(FD)D".to_owned()),
            ("mixed".to_owned(), "(IFDJ)D".to_owned()),
        ]
    );

    let constructor = emitted_method_code(&numeric_box, "<init>");
    assert_opcode_sequence(
        constructor,
        &[0x2a, 0xb7, 0x2a, 0x23, 0xb5, 0x2a, 0x28, 0xb5, 0xb1],
    );
    assert_method_reference(
        &numeric_box,
        constructor,
        0xb7,
        "java/lang/Object",
        "<init>",
        "()V",
    );
    assert_field_reference_sequence(
        &numeric_box,
        constructor,
        &[
            (0xb5, "NumericBox", "f", "F"),
            (0xb5, "NumericBox", "d", "D"),
        ],
    );
    assert_eq!(code_max_locals(&numeric_box, "<init>"), 4);
    assert_eq!(code_max_stack(&numeric_box, "<init>"), 3);
    assert_eq!(
        emitted_method_code(&runner, "floatValue")[0],
        0x12,
        "1.5f is loaded from CONSTANT_Float"
    );
    let float_code = emitted_method_code(&runner, "floatValue");
    let float_index = ConstantPoolIndex(u16::from(float_code[1]));
    assert!(matches!(
        runner.constant_pool.get(float_index),
        Some(ConstantPoolEntry::Float(value)) if value.to_bits() == 0x3fc0_0000
    ));
    assert_eq!(float_code[2], 0xae);
    let double_code = emitted_method_code(&runner, "doubleValue");
    assert_eq!(double_code[0], 0x14, "3.5 is loaded with ldc2_w");
    let double_index = ConstantPoolIndex(u16::from_be_bytes([double_code[1], double_code[2]]));
    assert!(matches!(
        runner.constant_pool.get(double_index),
        Some(ConstantPoolEntry::Double(value)) if value.to_bits() == 0x400c_0000_0000_0000
    ));
    assert_eq!(
        runner
            .constant_pool
            .get(ConstantPoolIndex(double_index.0 + 1)),
        None
    );
    assert_eq!(double_code[3], 0xaf);

    let constant_loads = decode_subset_instructions(emitted_method_code(&constants, "<init>"))
        .into_iter()
        .filter_map(|(offset, opcode)| match opcode {
            0x12 => Some(ConstantPoolIndex(u16::from(
                emitted_method_code(&constants, "<init>")[offset + 1],
            ))),
            0x13 | 0x14 => Some(ConstantPoolIndex(u16::from_be_bytes([
                emitted_method_code(&constants, "<init>")[offset + 1],
                emitted_method_code(&constants, "<init>")[offset + 2],
            ]))),
            _ => None,
        })
        .filter_map(|index| constants.constant_pool.get(index))
        .collect::<Vec<_>>();
    assert!(constant_loads.iter().any(
        |entry| matches!(entry, ConstantPoolEntry::Float(value) if value.to_bits() == 0x3fc0_0000)
    ));
    assert!(constant_loads.iter().any(
        |entry| matches!(entry, ConstantPoolEntry::Double(value) if value.to_bits() == 0x400c_0000_0000_0000)
    ));

    assert_opcode_sequence(
        emitted_method_code(&runner, "roundTripF"),
        &[0x23, 0x45, 0x24, 0xae],
    );
    assert_opcode_sequence(
        emitted_method_code(&runner, "roundTripD"),
        &[0x27, 0x4a, 0x29, 0xaf],
    );
    assert_eq!(code_max_locals(&runner, "chooseF"), 4);
    assert_eq!(code_max_locals(&runner, "chooseD"), 6);
    assert_eq!(code_max_locals(&runner, "mixed"), 7);
    assert!(
        stack_map_frames(&runner, "chooseF")
            .iter()
            .any(|frame| frame.stack == [StackMapVerificationType::Float])
    );
    assert!(
        stack_map_frames(&runner, "chooseD")
            .iter()
            .any(|frame| frame.stack == [StackMapVerificationType::Double])
    );
    assert!(stack_map_frames(&runner, "loop").iter().any(|frame| {
        frame.locals
            == [
                StackMapVerificationType::Object("Runner".to_owned()),
                StackMapVerificationType::Integer,
                StackMapVerificationType::Double,
                StackMapVerificationType::Double,
                StackMapVerificationType::Double,
                StackMapVerificationType::Integer,
            ]
    }));
    assert_method_reference(
        &runner,
        emitted_method_code(&runner, "forwardD"),
        0xb6,
        "NumericBox",
        "getD",
        "()D",
    );
    assert_method_reference(
        &runner,
        emitted_method_code(&runner, "construct"),
        0xb7,
        "NumericBox",
        "<init>",
        "(FD)V",
    );

    if !require_jdk_or_skip() {
        return;
    }
    let classpath = first_output.path().to_str().expect("UTF-8 output path");
    for (class_name, expected_descriptors) in [
        (
            "NumericBox",
            &["descriptor: (FD)V", "descriptor: ()F", "descriptor: ()D"][..],
        ),
        (
            "NumericConstants",
            &["descriptor: ()F", "descriptor: ()D"][..],
        ),
        (
            "Runner",
            &[
                "descriptor: (ZFF)F",
                "descriptor: (ZDD)D",
                "descriptor: (IDD)D",
                "descriptor: (FD)D",
                "descriptor: (IFDJ)D",
            ][..],
        ),
    ] {
        let javap = Command::new("javap")
            .args(["-v", "-c", "-classpath", classpath, class_name])
            .output()
            .expect("start javap");
        assert!(
            javap.status.success(),
            "javap failed for {class_name}: {javap:?}"
        );
        let output = String::from_utf8_lossy(&javap.stdout);
        assert!(
            output.contains("major version: 65"),
            "{class_name}: {output}"
        );
        for descriptor in expected_descriptors {
            assert!(
                output.contains(descriptor),
                "{class_name} omitted {descriptor}: {output}"
            );
        }
    }
    let harness = first_output
        .path()
        .join("FloatingPointCategory2Harness.java");
    fs::write(
        &harness,
        r#"public final class FloatingPointCategory2Harness {
  private static void check(boolean ok, String label) {
    if (!ok) throw new AssertionError(label);
  }
  public static void main(String[] args) {
    Runner runner = new Runner();
    check(Float.floatToRawIntBits(runner.floatValue()) == 0x3fc00000, "Float literal bits");
    check(Double.doubleToRawLongBits(runner.doubleValue()) == 0x400c000000000000L, "Double literal bits");
    NumericConstants constants = new NumericConstants();
    check(constants.getF() == 1.5f && constants.getD() == 3.5d, "constant fields");

    float[] floats = { 17.25f, -19.5f, Float.MIN_VALUE, -Float.MIN_VALUE,
                       Float.POSITIVE_INFINITY, Float.NEGATIVE_INFINITY,
                       0.0f, -0.0f };
    for (float value : floats) {
      float actual = runner.roundTripF(value);
      check(Float.floatToRawIntBits(actual) == Float.floatToRawIntBits(value), "Float round trip");
    }
    double[] doubles = { 17.25d, -19.5d, Double.MIN_VALUE, -Double.MIN_VALUE,
                         Double.POSITIVE_INFINITY, Double.NEGATIVE_INFINITY,
                         0.0d, -0.0d };
    for (double value : doubles) {
      double actual = runner.roundTripD(value);
      check(Double.doubleToRawLongBits(actual) == Double.doubleToRawLongBits(value), "Double round trip");
    }
    check(Float.isNaN(runner.roundTripF(Float.NaN)), "Float NaN classification");
    check(Double.isNaN(runner.roundTripD(Double.NaN)), "Double NaN classification");
    check(Float.floatToRawIntBits(runner.roundTripF(-0.0f)) == 0x80000000, "Float negative zero");
    check(Double.doubleToRawLongBits(runner.replaceD(3.5d, -0.0d)) == 0x8000000000000000L, "Double negative zero assignment");

    check(runner.chooseF(true, 1.25f, 2.5f) == 1.25f, "Float true branch");
    check(runner.chooseF(false, 1.25f, 2.5f) == 2.5f, "Float false branch");
    check(runner.chooseD(true, 1.25d, 2.5d) == 1.25d, "Double true branch");
    check(runner.chooseD(false, 1.25d, 2.5d) == 2.5d, "Double false branch");
    check(runner.loop(0, 11.0d, 22.0d) == 11.0d, "zero-iteration loop");
    check(runner.loop(1, 11.0d, 22.0d) == 22.0d, "one-iteration loop");
    check(runner.loop(3, 11.0d, 22.0d) == 22.0d, "multi-iteration loop");
    check(runner.loop(-1, 11.0d, 22.0d) == 11.0d, "negative loop count");

    NumericBox first = new NumericBox(1.5f, 3.5d);
    NumericBox second = new NumericBox(-2.5f, -4.5d);
    check(first.getF() == 1.5f && first.getD() == 3.5d, "constructor fields");
    first.setF(-0.0f);
    first.setD(-0.0d);
    second.setF(8.5f);
    second.setD(9.5d);
    check(Float.floatToRawIntBits(first.getF()) == 0x80000000, "Float field signed zero");
    check(Double.doubleToRawLongBits(first.getD()) == 0x8000000000000000L, "Double field signed zero");
    check(second.getF() == 8.5f && second.getD() == 9.5d, "independent mutable instances");
    second.setF(Float.NaN);
    second.setD(Double.NaN);
    check(Float.isNaN(second.getF()) && Double.isNaN(second.getD()), "NaN fields");
    check(runner.forwardD(new NumericBox(2.0f, 7.25d)) == 7.25d, "forwarded getter call");
    check(runner.construct(1.5f, 3.5d) == 3.5d, "mixed constructor call");
    check(runner.mixed(7, 1.5f, 3.5d, 9000000000L) == 3.5d, "mixed category slots");
  }
}
"#,
    )
    .expect("write FloatingPointCategory2Harness.java");
    let javac = Command::new("javac")
        .args(["-classpath", classpath, "-d", classpath])
        .arg(&harness)
        .output()
        .expect("start javac");
    assert!(
        javac.status.success(),
        "javac rejected acceptance harness: {javac:?}"
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            classpath,
            "FloatingPointCategory2Harness",
        ])
        .output()
        .expect("start JVM verifier");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected the complete Float/Double batch: {java:?}"
    );
}

#[test]
fn float_double_fields_and_primary_constructor_initialization_verify_and_execute() {
    let first_output = TestOutputDirectory::new("float-double-fields-first");
    let second_output = TestOutputDirectory::new("float-double-fields-second");
    let first = compile_fixture_classes("FloatDoubleFields.scala", first_output.path()).unwrap();
    let second = compile_fixture_classes("FloatDoubleFields.scala", second_output.path()).unwrap();
    assert_eq!(
        first
            .iter()
            .map(|artifact| (
                artifact.class.internal_name(),
                artifact.class.bytes().to_vec()
            ))
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|artifact| (
                artifact.class.internal_name(),
                artifact.class.bytes().to_vec()
            ))
            .collect::<Vec<_>>()
    );

    let decode = |name: &str| {
        let artifact = first
            .iter()
            .find(|artifact| artifact.class.internal_name() == name)
            .unwrap_or_else(|| panic!("missing emitted class {name}"));
        decode_emitted_class(artifact)
    };
    let numeric_box = decode("NumericBox");
    let numeric_constants = decode("NumericConstants");
    assert_eq!(
        numeric_box
            .fields
            .iter()
            .map(|field| (
                numeric_box.constant_pool.utf8(field.name_index).unwrap(),
                numeric_box
                    .constant_pool
                    .utf8(field.descriptor_index)
                    .unwrap(),
                field.access_flags.0,
            ))
            .collect::<Vec<_>>(),
        [
            ("f", "F", dotty_classfile::access_flags::ACC_PRIVATE),
            ("d", "D", dotty_classfile::access_flags::ACC_PRIVATE),
        ]
    );
    assert_eq!(
        methods_with_descriptors(&numeric_box),
        [
            ("<init>".to_owned(), "(FD)V".to_owned()),
            ("getF".to_owned(), "()F".to_owned()),
            ("getD".to_owned(), "()D".to_owned()),
            ("setF".to_owned(), "(F)V".to_owned()),
            ("setD".to_owned(), "(D)V".to_owned()),
        ]
    );
    let constructor = emitted_method_code(&numeric_box, "<init>");
    assert_opcode_sequence(
        constructor,
        &[0x2a, 0xb7, 0x2a, 0x23, 0xb5, 0x2a, 0x28, 0xb5, 0xb1],
    );
    assert_method_reference(
        &numeric_box,
        constructor,
        0xb7,
        "java/lang/Object",
        "<init>",
        "()V",
    );
    assert_field_reference_sequence(
        &numeric_box,
        constructor,
        &[
            (0xb5, "NumericBox", "f", "F"),
            (0xb5, "NumericBox", "d", "D"),
        ],
    );
    assert_eq!(code_max_stack(&numeric_box, "<init>"), 3);
    assert_eq!(code_max_locals(&numeric_box, "<init>"), 4);
    assert_opcode_sequence(
        emitted_method_code(&numeric_box, "getF"),
        &[0x2a, 0xb4, 0xae],
    );
    assert_field_reference(
        &numeric_box,
        emitted_method_code(&numeric_box, "getF"),
        0xb4,
        "NumericBox",
        "f",
        "F",
    );
    assert_opcode_sequence(
        emitted_method_code(&numeric_box, "getD"),
        &[0x2a, 0xb4, 0xaf],
    );
    assert_opcode_sequence(
        emitted_method_code(&numeric_box, "setF"),
        &[0x2a, 0x4d, 0x23, 0x46, 0x2c, 0x25, 0xb5, 0xb1],
    );
    assert_eq!(code_max_stack(&numeric_box, "setF"), 2);
    assert_eq!(code_max_locals(&numeric_box, "setF"), 4);
    assert_opcode_sequence(
        emitted_method_code(&numeric_box, "setD"),
        &[0x2a, 0x4e, 0x27, 0x39, 0x2d, 0x18, 0xb5, 0xb1],
    );
    assert_eq!(code_max_stack(&numeric_box, "setD"), 3);
    assert_eq!(code_max_locals(&numeric_box, "setD"), 6);

    for (name, descriptor) in [("f", "F"), ("d", "D")] {
        let field = numeric_constants
            .fields
            .iter()
            .find(|field| numeric_constants.constant_pool.utf8(field.name_index) == Ok(name))
            .expect("private constant field emitted");
        assert_eq!(
            numeric_constants.constant_pool.utf8(field.descriptor_index),
            Ok(descriptor)
        );
        assert_eq!(
            field.access_flags.0,
            dotty_classfile::access_flags::ACC_PRIVATE | dotty_classfile::access_flags::ACC_FINAL
        );
    }
    let constant_constructor = emitted_method_code(&numeric_constants, "<init>");
    let load_constants = decode_subset_instructions(constant_constructor)
        .into_iter()
        .filter_map(|(offset, opcode)| match opcode {
            0x12 => Some(ConstantPoolIndex(u16::from(
                constant_constructor[offset + 1],
            ))),
            0x13 | 0x14 => Some(ConstantPoolIndex(u16::from_be_bytes([
                constant_constructor[offset + 1],
                constant_constructor[offset + 2],
            ]))),
            _ => None,
        })
        .filter_map(|index| numeric_constants.constant_pool.get(index))
        .collect::<Vec<_>>();
    assert!(load_constants.iter().any(
        |entry| matches!(entry, ConstantPoolEntry::Float(value) if value.to_bits() == 1.5f32.to_bits())
    ));
    assert!(load_constants.iter().any(
        |entry| matches!(entry, ConstantPoolEntry::Double(value) if value.to_bits() == 3.5f64.to_bits())
    ));

    if require_jdk_or_skip() {
        for (class_name, expected) in [
            ("NumericBox", "descriptor: (FD)V"),
            ("NumericConstants", "descriptor: ()F"),
        ] {
            let javap = Command::new("javap")
                .args([
                    "-v",
                    "-c",
                    "-classpath",
                    first_output.path().to_str().unwrap(),
                    class_name,
                ])
                .output()
                .expect("start javap");
            assert!(javap.status.success(), "javap failed: {javap:?}");
            let text = String::from_utf8_lossy(&javap.stdout);
            assert!(text.contains(expected), "{class_name}: {text}");
            assert!(text.contains("putfield"), "{class_name}: {text}");
            if class_name == "NumericBox" {
                assert!(text.contains("fload_1"), "{class_name}: {text}");
                assert!(text.contains("dload_2"), "{class_name}: {text}");
            }
        }
        let harness_path = first_output.path().join("FloatDoubleFieldsHarness.java");
        fs::write(
            &harness_path,
            r#"public final class FloatDoubleFieldsHarness {
  public static void main(String[] args) {
    NumericBox first = new NumericBox(1.25f, 2.5d);
    NumericBox second = new NumericBox(-3.0f, -4.0d);
    if (first.getF() != 1.25f || first.getD() != 2.5d) throw new AssertionError("constructor fields");
    first.setF(-0.0f);
    first.setD(-0.0d);
    if (Float.floatToRawIntBits(first.getF()) != Float.floatToRawIntBits(-0.0f)) throw new AssertionError("float negative zero");
    if (Double.doubleToRawLongBits(first.getD()) != Double.doubleToRawLongBits(-0.0d)) throw new AssertionError("double negative zero");
    second.setF(7.0f);
    second.setD(8.0d);
    if (Float.floatToRawIntBits(first.getF()) != Float.floatToRawIntBits(-0.0f) || Double.doubleToRawLongBits(first.getD()) != Double.doubleToRawLongBits(-0.0d) || second.getF() != 7.0f || second.getD() != 8.0d) throw new AssertionError("instance isolation");
    second.setF(Float.NaN);
    second.setD(Double.NaN);
    if (!Float.isNaN(second.getF()) || !Double.isNaN(second.getD())) throw new AssertionError("NaN fields");
    NumericConstants constants = new NumericConstants();
    if (constants.getF() != 1.5f || constants.getD() != 3.5d) throw new AssertionError("literal fields");
  }
}
"#,
        )
        .expect("write FloatDoubleFieldsHarness.java");
        let javac = Command::new("javac")
            .args(["-classpath", first_output.path().to_str().unwrap(), "-d"])
            .arg(first_output.path())
            .arg(&harness_path)
            .output()
            .expect("start javac");
        assert!(javac.status.success(), "javac failed: {javac:?}");
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                first_output.path().to_str().unwrap(),
                "FloatDoubleFieldsHarness",
            ])
            .output()
            .expect("start java");
        assert!(java.status.success(), "java verification failed: {java:?}");
    }
}

#[test]
fn long_fields_and_primary_constructor_initialization_verify_and_execute() {
    let first_output = TestOutputDirectory::new("long-fields-first");
    let second_output = TestOutputDirectory::new("long-fields-second");
    let first = compile_fixture_classes("LongFields.scala", first_output.path()).unwrap();
    let second = compile_fixture_classes("LongFields.scala", second_output.path()).unwrap();
    assert_eq!(
        first
            .iter()
            .map(|artifact| (
                artifact.class.internal_name(),
                artifact.class.bytes().to_vec()
            ))
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|artifact| (
                artifact.class.internal_name(),
                artifact.class.bytes().to_vec()
            ))
            .collect::<Vec<_>>()
    );

    let decode = |name: &str| {
        let artifact = first
            .iter()
            .find(|artifact| artifact.class.internal_name() == name)
            .unwrap_or_else(|| panic!("missing emitted class {name}"));
        let mut reader = Reader::new(artifact.class.bytes());
        let class = ClassFile::decode(&mut reader).expect("Long field class should decode");
        assert!(reader.is_at_end());
        class.validate_references().unwrap();
        class
    };
    let long_box = decode("LongBox");
    let long_constant = decode("LongConstant");

    let current = &long_box.fields[0];
    assert_eq!(
        long_box.constant_pool.utf8(current.name_index),
        Ok("current")
    );
    assert_eq!(
        long_box.constant_pool.utf8(current.descriptor_index),
        Ok("J")
    );
    assert_eq!(
        current.access_flags.0,
        dotty_classfile::access_flags::ACC_PRIVATE
    );
    assert_eq!(
        long_box
            .methods
            .iter()
            .map(|method| (
                long_box.constant_pool.utf8(method.name_index).unwrap(),
                long_box
                    .constant_pool
                    .utf8(method.descriptor_index)
                    .unwrap()
            ))
            .collect::<Vec<_>>(),
        [("<init>", "(J)V"), ("get", "()J"), ("set", "(J)V")]
    );
    let constructor = emitted_method_code(&long_box, "<init>");
    assert_opcode_sequence(constructor, &[0x2a, 0xb7, 0x2a, 0x1f, 0xb5, 0xb1]);
    assert_method_reference(
        &long_box,
        constructor,
        0xb7,
        "java/lang/Object",
        "<init>",
        "()V",
    );
    assert_field_reference(&long_box, constructor, 0xb5, "LongBox", "current", "J");
    assert_eq!(code_max_stack(&long_box, "<init>"), 3);
    assert_eq!(code_max_locals(&long_box, "<init>"), 3);
    assert_opcode_sequence(emitted_method_code(&long_box, "get"), &[0x2a, 0xb4, 0xad]);
    assert_field_reference(
        &long_box,
        emitted_method_code(&long_box, "get"),
        0xb4,
        "LongBox",
        "current",
        "J",
    );
    assert_opcode_sequence(
        emitted_method_code(&long_box, "set"),
        &[0x2a, 0x4e, 0x1f, 0x37, 0x2d, 0x16, 0xb5, 0xb1],
    );
    assert_eq!(code_max_stack(&long_box, "set"), 3);
    assert_eq!(code_max_locals(&long_box, "set"), 6);

    let number = &long_constant.fields[0];
    assert_eq!(
        long_constant.constant_pool.utf8(number.name_index),
        Ok("number")
    );
    assert_eq!(
        long_constant.constant_pool.utf8(number.descriptor_index),
        Ok("J")
    );
    assert_eq!(
        number.access_flags.0,
        dotty_classfile::access_flags::ACC_PRIVATE | dotty_classfile::access_flags::ACC_FINAL
    );
    let constant_constructor = emitted_method_code(&long_constant, "<init>");
    assert_opcode_sequence(constant_constructor, &[0x2a, 0xb7, 0x2a, 0x14, 0xb5, 0xb1]);
    assert_method_reference(
        &long_constant,
        constant_constructor,
        0xb7,
        "java/lang/Object",
        "<init>",
        "()V",
    );
    assert_field_reference(
        &long_constant,
        constant_constructor,
        0xb5,
        "LongConstant",
        "number",
        "J",
    );
    assert_eq!(code_max_stack(&long_constant, "<init>"), 3);
    assert_eq!(code_max_locals(&long_constant, "<init>"), 1);
    assert_opcode_sequence(
        emitted_method_code(&long_constant, "get"),
        &[0x2a, 0xb4, 0xad],
    );

    if require_jdk_or_skip() {
        for class_name in ["LongBox", "LongConstant"] {
            let javap = Command::new("javap")
                .args([
                    "-v",
                    "-c",
                    "-classpath",
                    first_output.path().to_str().unwrap(),
                    class_name,
                ])
                .output()
                .expect("start javap");
            assert!(javap.status.success(), "javap failed: {javap:?}");
            let text = String::from_utf8_lossy(&javap.stdout);
            assert!(text.contains("descriptor: (J)V") || class_name == "LongConstant");
            assert!(text.contains("descriptor: ()J"));
            assert!(text.contains("putfield"));
            assert!(text.contains("getfield"));
            if class_name == "LongBox" {
                assert!(text.contains("lload_1"));
                assert!(text.contains("lstore"));
            } else {
                assert!(text.contains("ldc2_w"));
                assert!(text.contains("9000000000"));
            }
        }

        let harness_path = first_output.path().join("LongFieldsHarness.java");
        fs::write(
            &harness_path,
            r#"public final class LongFieldsHarness {
  public static void main(String[] args) {
    LongBox first = new LongBox(9000000000L);
    LongBox second = new LongBox(-9000000000L);
    if (first.get() != 9000000000L || second.get() != -9000000000L) throw new AssertionError("constructor parameter");
    first.set(-7L);
    second.set(0L);
    if (first.get() != -7L || second.get() != 0L) throw new AssertionError("instance field state");
    if (new LongConstant().get() != 9000000000L) throw new AssertionError("Long literal field");
    System.out.println("long fields ok");
  }
}
"#,
        )
        .expect("write LongFieldsHarness.java");
        let javac = Command::new("javac")
            .args(["-classpath", first_output.path().to_str().unwrap(), "-d"])
            .arg(first_output.path())
            .arg(&harness_path)
            .output()
            .expect("start javac");
        assert!(javac.status.success(), "javac failed: {javac:?}");
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                first_output.path().to_str().unwrap(),
                "LongFieldsHarness",
            ])
            .output()
            .expect("start Java verifier");
        assert!(java.status.success(), "java failed: {java:?}");
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "long fields ok"
        );
    }
}

#[test]
fn long_calls_if_and_while_compose_and_verify_on_the_jvm() {
    let first_output = TestOutputDirectory::new("long-composition-first");
    let second_output = TestOutputDirectory::new("long-composition-second");
    let first = compile_fixture_classes("LongComposition.scala", first_output.path())
        .expect("Long calls and CFG expressions compile");
    let second = compile_fixture_classes("LongComposition.scala", second_output.path())
        .expect("a fresh frontend compiles the same Long composition");
    assert_eq!(first.len(), 4);
    assert_eq!(
        first
            .iter()
            .map(|artifact| (artifact.class.internal_name(), artifact.class.bytes()))
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|artifact| (artifact.class.internal_name(), artifact.class.bytes()))
            .collect::<Vec<_>>(),
        "fresh frontend sessions emit byte-identical Long classes"
    );

    let artifact = |name: &str| {
        first
            .iter()
            .find(|artifact| artifact.class.internal_name() == name)
            .unwrap_or_else(|| panic!("{name} class is emitted"))
    };
    let long_box = decode_emitted_class(artifact("LongBox"));
    let long_derived = decode_emitted_class(artifact("LongDerived"));
    let long_derived_literal = decode_emitted_class(artifact("LongDerivedLiteral"));
    let runner = decode_emitted_class(artifact("Runner"));
    assert_eq!(
        methods_with_descriptors(&long_box),
        [
            ("<init>".to_owned(), "(J)V".to_owned()),
            ("get".to_owned(), "()J".to_owned()),
            ("set".to_owned(), "(J)V".to_owned()),
            ("select".to_owned(), "(IJZJ)J".to_owned()),
        ]
    );
    assert_eq!(code_max_stack(&long_box, "select"), 2);
    assert_eq!(code_max_locals(&long_box, "select"), 7);
    assert_eq!(
        methods_with_descriptors(&long_derived),
        [("<init>".to_owned(), "(J)V".to_owned())]
    );
    assert_invocation_sequence(
        &long_derived,
        emitted_method_code(&long_derived, "<init>"),
        &[(0xb7, "LongBox", "<init>", "(J)V")],
    );
    assert_eq!(
        methods_with_descriptors(&long_derived_literal),
        [("<init>".to_owned(), "()V".to_owned())]
    );
    assert_invocation_sequence(
        &long_derived_literal,
        emitted_method_code(&long_derived_literal, "<init>"),
        &[(0xb7, "LongBox", "<init>", "(J)V")],
    );
    assert_eq!(
        methods_with_descriptors(&runner),
        [
            ("<init>".to_owned(), "()V".to_owned()),
            ("choose".to_owned(), "(ZJJ)J".to_owned()),
            ("loop".to_owned(), "(IJJ)J".to_owned()),
            ("forward".to_owned(), "(LLongBox;)J".to_owned()),
            ("mixed".to_owned(), "(LLongBox;ZJJ)J".to_owned()),
            ("roundTrip".to_owned(), "(J)J".to_owned()),
            ("literalRoundTrip".to_owned(), "()J".to_owned()),
        ]
    );

    let choose_code = emitted_method_code(&runner, "choose");
    assert_eq!(
        decode_subset_instructions(choose_code)
            .iter()
            .map(|(_, opcode)| *opcode)
            .collect::<Vec<_>>(),
        [0x1b, 0x99, 0x20, 0xa7, 0x16, 0xad]
    );
    assert_eq!(code_max_stack(&runner, "choose"), 2);
    assert_eq!(code_max_locals(&runner, "choose"), 6);
    let choose_join = decode_branch_targets(choose_code)
        .iter()
        .find(|(_, opcode, _)| *opcode == 0xa7)
        .map(|(_, _, target)| *target)
        .expect("choose contains a false branch");
    assert!(stack_map_frames(&runner, "choose").iter().any(|frame| {
        frame.pc == choose_join && frame.stack == [StackMapVerificationType::Long]
    }));

    let loop_code = emitted_method_code(&runner, "loop");
    assert_eq!(code_max_stack(&runner, "loop"), 2);
    assert_eq!(code_max_locals(&runner, "loop"), 9);
    let loop_targets = decode_branch_targets(loop_code);
    let loop_header = loop_targets
        .iter()
        .find(|(offset, _, target)| target < offset)
        .map(|(_, _, target)| *target)
        .expect("loop has a backward edge");
    let loop_frame = stack_map_frames(&runner, "loop")
        .into_iter()
        .find(|frame| frame.pc == loop_header)
        .expect("loop header has a full StackMapTable frame");
    assert_eq!(
        loop_frame.locals,
        [
            StackMapVerificationType::Object("Runner".to_owned()),
            StackMapVerificationType::Integer,
            StackMapVerificationType::Long,
            StackMapVerificationType::Long,
            StackMapVerificationType::Long,
            StackMapVerificationType::Integer,
        ],
        "parameter and mutable Long locals keep exact loop-header slots"
    );
    assert!(loop_frame.stack.is_empty());

    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "forward"),
        &[(0xb6, "LongBox", "get", "()J")],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "mixed"),
        &[(0xb6, "LongBox", "select", "(IJZJ)J")],
    );
    assert_eq!(code_max_stack(&runner, "mixed"), 7);
    assert_eq!(code_max_locals(&runner, "mixed"), 7);
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "roundTrip"),
        &[
            (0xb7, "LongBox", "<init>", "(J)V"),
            (0xb6, "LongBox", "get", "()J"),
        ],
    );
    assert_eq!(code_max_locals(&runner, "roundTrip"), 4);
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "literalRoundTrip"),
        &[
            (0xb7, "LongBox", "<init>", "(J)V"),
            (0xb6, "LongBox", "get", "()J"),
        ],
    );

    if !require_jdk_or_skip() {
        return;
    }

    for class_name in ["LongBox", "LongDerived", "LongDerivedLiteral", "Runner"] {
        let output_path = first_output.path().to_str().expect("UTF-8 output path");
        let javap = Command::new("javap")
            .args(["-v", "-c", "-classpath", output_path, class_name])
            .output()
            .expect("start javap");
        assert!(
            javap.status.success(),
            "javap rejected {class_name}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javap.stdout),
            String::from_utf8_lossy(&javap.stderr)
        );
        let text = String::from_utf8_lossy(&javap.stdout);
        if matches!(class_name, "LongBox" | "Runner") {
            assert!(text.contains("lreturn"));
        }
        if class_name == "Runner" {
            assert!(text.contains("StackMapTable"));
            assert!(text.contains("descriptor: (ZJJ)J"));
            assert!(text.contains("descriptor: (IJJ)J"));
            assert!(text.contains("descriptor: (LLongBox;ZJJ)J"));
            assert!(text.contains("invokespecial"));
            assert!(text.contains("invokevirtual"));
            assert!(text.contains("lload"));
            assert!(text.contains("lstore"));
        }
    }

    let output_path = first_output.path().to_str().expect("UTF-8 output path");
    let harness = first_output.path().join("LongCompositionHarness.java");
    fs::write(
        &harness,
        r#"public final class LongCompositionHarness {
  public static void main(String[] args) {
    Runner runner = new Runner();
    LongBox box = new LongBox(9000000000L);
    LongBox derived = new LongDerived(-9000000000L);
    LongBox derivedLiteral = new LongDerivedLiteral();
    if (runner.choose(true, 9000000000L, -9000000000L) != 9000000000L) throw new AssertionError("choose true");
    if (runner.choose(false, 9000000000L, -9000000000L) != -9000000000L) throw new AssertionError("choose false");
    if (runner.loop(0, 11L, 22L) != 11L || runner.loop(3, 11L, 22L) != 22L) throw new AssertionError("loop");
    if (runner.mixed(box, true, Long.MIN_VALUE, 5L) != Long.MIN_VALUE || runner.mixed(box, false, 7L, -9000000000L) != -9000000000L) throw new AssertionError("mixed call");
    if (runner.forward(box) != 9000000000L || runner.roundTrip(-9000000000L) != -9000000000L || runner.forward(derived) != -9000000000L || runner.forward(derivedLiteral) != 9000000000L || runner.literalRoundTrip() != 9000000000L) throw new AssertionError("calls");
    box.set(Long.MIN_VALUE);
    if (runner.forward(box) != Long.MIN_VALUE) throw new AssertionError("LongBox state");
    System.out.println("long composition ok");
  }
}
"#,
    )
    .expect("write Long composition harness");
    let javac = Command::new("javac")
        .args(["-classpath", output_path, "-d", output_path])
        .arg(&harness)
        .output()
        .expect("start javac");
    assert!(
        javac.status.success(),
        "javac rejected Long composition harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            output_path,
            "LongCompositionHarness",
        ])
        .output()
        .expect("start Java verifier");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected Long composition\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "long composition ok"
    );
}

#[test]
fn long_category_two_acceptance_batch_verifies_and_executes_on_jvm() {
    let first_output = TestOutputDirectory::new("long-category2-first");
    let second_output = TestOutputDirectory::new("long-category2-second");
    let first = compile_fixture_classes("LongCategory2.scala", first_output.path())
        .expect("the complete Scala 3.9 Long acceptance fixture compiles");
    let second = compile_fixture_classes("LongCategory2.scala", second_output.path())
        .expect("a fresh frontend session compiles the complete fixture");
    let first_batch = first
        .iter()
        .map(|artifact| {
            (
                artifact.class.internal_name(),
                artifact.class.bytes().to_vec(),
            )
        })
        .collect::<Vec<_>>();
    let second_batch = second
        .iter()
        .map(|artifact| {
            (
                artifact.class.internal_name(),
                artifact.class.bytes().to_vec(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        first_batch, second_batch,
        "independent canonical source sessions emit byte-identical batches"
    );
    assert_eq!(
        first_batch
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>(),
        ["LongBox", "LongConstants", "Runner"],
        "batch publication follows source class order"
    );
    assert_eq!(first.len(), 3);
    assert_eq!(first_output.class_files().len(), 3);

    let artifact = |name: &str| {
        first
            .iter()
            .find(|artifact| artifact.class.internal_name() == name)
            .unwrap_or_else(|| panic!("missing emitted {name} class"))
    };
    let decode = |name: &str| {
        let mut reader = Reader::new(artifact(name).class.bytes());
        let class = ClassFile::decode(&mut reader).expect("emitted class decodes");
        assert!(reader.is_at_end());
        class
            .validate_references()
            .expect("all encoded references resolve");
        assert_eq!(class.version.major, 65, "{name} uses Java 21 class files");
        assert_eq!(class.version.minor, 0);
        assert_eq!(class.constant_pool.class_name(class.this_class), Ok(name));
        let super_class = class.super_class.expect("source class extends Object");
        assert_eq!(
            class.constant_pool.class_name(super_class),
            Ok("java/lang/Object")
        );
        assert!(class.interfaces.is_empty());
        assert!(class.access_flags.is_public());
        for method in &class.methods {
            assert!(method.access_flags.is_public());
            assert!(matches!(method.attributes.as_slice(), [Attribute::Code(_)]));
        }
        class
    };
    let long_box = decode("LongBox");
    let long_constants = decode("LongConstants");
    let runner = decode("Runner");

    assert_eq!(
        methods_with_descriptors(&long_box),
        [
            ("<init>".to_owned(), "(J)V".to_owned()),
            ("get".to_owned(), "()J".to_owned()),
            ("set".to_owned(), "(J)V".to_owned()),
        ]
    );
    assert_eq!(
        methods_with_descriptors(&long_constants),
        [
            ("<init>".to_owned(), "()V".to_owned()),
            ("value".to_owned(), "()J".to_owned()),
        ]
    );
    assert_eq!(
        methods_with_descriptors(&runner),
        [
            ("<init>".to_owned(), "()V".to_owned()),
            ("zero".to_owned(), "()J".to_owned()),
            ("one".to_owned(), "()J".to_owned()),
            ("constant".to_owned(), "()J".to_owned()),
            ("roundTrip".to_owned(), "(J)J".to_owned()),
            ("reassigned".to_owned(), "(JJ)J".to_owned()),
            ("choose".to_owned(), "(ZJJ)J".to_owned()),
            ("loop".to_owned(), "(IJJ)J".to_owned()),
            ("forward".to_owned(), "(LLongBox;)J".to_owned()),
            ("construct".to_owned(), "(J)J".to_owned()),
        ]
    );

    let [current] = long_box.fields.as_slice() else {
        panic!("LongBox contains only its current field")
    };
    assert_eq!(
        long_box.constant_pool.utf8(current.name_index),
        Ok("current")
    );
    assert_eq!(
        long_box.constant_pool.utf8(current.descriptor_index),
        Ok("J")
    );
    assert_eq!(
        current.access_flags.0,
        dotty_classfile::access_flags::ACC_PRIVATE
    );
    assert_opcode_sequence(
        emitted_method_code(&long_box, "<init>"),
        &[0x2a, 0xb7, 0x2a, 0x1f, 0xb5, 0xb1],
    );
    assert_method_reference(
        &long_box,
        emitted_method_code(&long_box, "<init>"),
        0xb7,
        "java/lang/Object",
        "<init>",
        "()V",
    );
    assert_field_reference(
        &long_box,
        emitted_method_code(&long_box, "<init>"),
        0xb5,
        "LongBox",
        "current",
        "J",
    );
    assert_eq!(code_max_stack(&long_box, "<init>"), 3);
    assert_eq!(code_max_locals(&long_box, "<init>"), 3);
    assert_opcode_sequence(emitted_method_code(&long_box, "get"), &[0x2a, 0xb4, 0xad]);
    assert_field_reference(
        &long_box,
        emitted_method_code(&long_box, "get"),
        0xb4,
        "LongBox",
        "current",
        "J",
    );
    assert_eq!(code_max_stack(&long_box, "set"), 3);
    assert_eq!(code_max_locals(&long_box, "set"), 6);
    assert_field_reference(
        &long_box,
        emitted_method_code(&long_box, "set"),
        0xb5,
        "LongBox",
        "current",
        "J",
    );

    let [large] = long_constants.fields.as_slice() else {
        panic!("LongConstants contains only its large field")
    };
    assert_eq!(
        long_constants.constant_pool.utf8(large.name_index),
        Ok("large")
    );
    assert_eq!(
        long_constants.constant_pool.utf8(large.descriptor_index),
        Ok("J")
    );
    assert_eq!(
        large.access_flags.0,
        dotty_classfile::access_flags::ACC_PRIVATE | dotty_classfile::access_flags::ACC_FINAL
    );
    let constant_init = emitted_method_code(&long_constants, "<init>");
    assert_opcode_sequence(constant_init, &[0x2a, 0xb7, 0x2a, 0x14, 0xb5, 0xb1]);
    assert_method_reference(
        &long_constants,
        constant_init,
        0xb7,
        "java/lang/Object",
        "<init>",
        "()V",
    );
    assert_field_reference(
        &long_constants,
        constant_init,
        0xb5,
        "LongConstants",
        "large",
        "J",
    );
    assert_eq!(code_max_stack(&long_constants, "<init>"), 3);
    assert_eq!(code_max_locals(&long_constants, "<init>"), 1);
    assert_opcode_sequence(
        emitted_method_code(&long_constants, "value"),
        &[0x2a, 0xb4, 0xad],
    );

    let long_constant_index = (1..=u16::MAX)
        .find(|index| {
            matches!(
                long_constants.constant_pool.get(ConstantPoolIndex(*index)),
                Some(ConstantPoolEntry::Long(9_000_000_000))
            )
        })
        .expect("LongConstants stores the full signed 64-bit literal");
    assert!(
        long_constants
            .constant_pool
            .get(ConstantPoolIndex(long_constant_index + 1))
            .is_none(),
        "CONSTANT_Long reserves the following constant-pool index"
    );
    assert_eq!(emitted_method_code(&runner, "zero"), [0x09, 0xad]);
    assert_eq!(emitted_method_code(&runner, "one"), [0x0a, 0xad]);
    let constant_code = emitted_method_code(&runner, "constant");
    assert_eq!(constant_code[0], 0x14, "nontrivial Long uses ldc2_w");
    assert_eq!(*constant_code.last().unwrap(), 0xad);
    assert_eq!(code_max_stack(&runner, "constant"), 2);
    assert_eq!(code_max_locals(&runner, "roundTrip"), 5);
    assert_eq!(code_max_locals(&runner, "reassigned"), 7);
    assert_eq!(code_max_stack(&runner, "reassigned"), 2);

    let choose_code = emitted_method_code(&runner, "choose");
    assert_eq!(code_max_stack(&runner, "choose"), 2);
    assert_eq!(code_max_locals(&runner, "choose"), 6);
    let choose_join = decode_branch_targets(choose_code)
        .iter()
        .find(|(_, opcode, _)| *opcode == 0xa7)
        .map(|(_, _, target)| *target)
        .expect("choose has a conditional branch to its Long join");
    assert!(stack_map_frames(&runner, "choose").iter().any(|frame| {
        frame.pc == choose_join && frame.stack == [StackMapVerificationType::Long]
    }));

    let loop_code = emitted_method_code(&runner, "loop");
    assert_eq!(code_max_stack(&runner, "loop"), 2);
    assert_eq!(code_max_locals(&runner, "loop"), 9);
    let loop_header = decode_branch_targets(loop_code)
        .iter()
        .find(|(offset, _, target)| target < offset)
        .map(|(_, _, target)| *target)
        .expect("loop has a verified backedge");
    let loop_frame = stack_map_frames(&runner, "loop")
        .into_iter()
        .find(|frame| frame.pc == loop_header)
        .expect("loop header has a full Long local frame");
    assert_eq!(
        loop_frame.locals,
        [
            StackMapVerificationType::Object("Runner".to_owned()),
            StackMapVerificationType::Integer,
            StackMapVerificationType::Long,
            StackMapVerificationType::Long,
            StackMapVerificationType::Long,
            StackMapVerificationType::Integer,
        ]
    );
    assert!(loop_frame.stack.is_empty());

    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "forward"),
        &[(0xb6, "LongBox", "get", "()J")],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "construct"),
        &[
            (0xb7, "LongBox", "<init>", "(J)V"),
            (0xb6, "LongBox", "get", "()J"),
        ],
    );
    assert_eq!(code_max_locals(&runner, "construct"), 4);

    if !require_jdk_or_skip() {
        return;
    }
    let classpath = first_output.path().to_str().expect("UTF-8 output path");
    for class_name in ["LongBox", "LongConstants", "Runner"] {
        let javap = Command::new("javap")
            .args(["-v", "-c", "-classpath", classpath, class_name])
            .output()
            .expect("start javap");
        assert!(
            javap.status.success(),
            "javap rejected {class_name}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javap.stdout),
            String::from_utf8_lossy(&javap.stderr)
        );
        let text = String::from_utf8_lossy(&javap.stdout);
        assert!(text.contains("major version: 65"));
        if class_name != "LongConstants" {
            assert!(text.contains("lreturn"));
        }
        if class_name == "LongConstants" {
            assert!(text.contains("ldc2_w"));
            assert!(text.contains("9000000000"));
            assert!(text.contains("descriptor: ()J"));
        }
        if class_name == "Runner" {
            for descriptor in ["(ZJJ)J", "(IJJ)J", "(LLongBox;)J", "(J)J"] {
                assert!(text.contains(&format!("descriptor: {descriptor}")));
            }
            assert!(text.contains("StackMapTable"));
            assert!(text.contains("invokespecial"));
            assert!(text.contains("invokevirtual"));
            assert!(text.contains("ldc2_w"));
            assert!(text.contains("lload"));
            assert!(text.contains("lstore"));
        }
    }

    let harness = first_output.path().join("LongCategory2Harness.java");
    fs::write(
        &harness,
        r#"public final class LongCategory2Harness {
  public static void main(String[] args) {
    Runner runner = new Runner();
    if (runner.zero() != 0L || runner.one() != 1L || runner.constant() != 9000000000L) throw new AssertionError("constants");
    if (new LongConstants().value() != 9000000000L) throw new AssertionError("LongConstants field");
    for (long value : new long[] {0L, 9000000000L, -9000000000L, Long.MIN_VALUE, Long.MAX_VALUE}) {
      if (runner.roundTrip(value) != value || runner.construct(value) != value) throw new AssertionError("roundTrip/construct " + value);
    }
    if (runner.reassigned(7L, -9000000000L) != -9000000000L) throw new AssertionError("reassigned");
    if (runner.choose(true, 9000000000L, -9000000000L) != 9000000000L || runner.choose(false, 9000000000L, -9000000000L) != -9000000000L) throw new AssertionError("choose");
    if (runner.loop(0, 11L, 22L) != 11L || runner.loop(1, 11L, 22L) != 22L || runner.loop(3, 11L, 22L) != 22L || runner.loop(-2, 11L, 22L) != 11L) throw new AssertionError("loop");
    LongBox first = new LongBox(9000000000L);
    LongBox second = new LongBox(-9000000000L);
    if (runner.forward(first) != 9000000000L || runner.forward(second) != -9000000000L) throw new AssertionError("forward");
    first.set(Long.MIN_VALUE);
    if (runner.forward(first) != Long.MIN_VALUE || runner.forward(second) != -9000000000L) throw new AssertionError("independent fields");
    System.out.println("Long category-two acceptance ok");
  }
}
"#,
    )
    .expect("write acceptance harness");
    let javac = Command::new("javac")
        .args(["-classpath", classpath, "-d", classpath])
        .arg(&harness)
        .output()
        .expect("start javac");
    assert!(
        javac.status.success(),
        "javac rejected acceptance harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            classpath,
            "LongCategory2Harness",
        ])
        .output()
        .expect("start Java verifier");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected acceptance classes\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "Long category-two acceptance ok"
    );
}

#[test]
fn long_category_two_audit_emits_fields_and_control_flow() {
    let source = fs::read_to_string(fixture_path("LongCategoryTwo.scala"))
        .expect("read Scala Long category-two audit fixture");
    let output = TestOutputDirectory::new("scala-long-category-two-audit");
    let mut observed = false;
    let result = compile_source_batch_with_source_observer(
        &source,
        "LongCategoryTwo.scala",
        output.path(),
        SourceId::from_index(1513),
        SourceUnitCardinality::AtLeastTwo,
        &mut |_, _, unit, definitions| {
            observed = true;
            let metadata = unit
                .metadata(definitions)
                .expect("canonical Long fields and descriptors are projected");
            let long_box = audit_class_named(unit, "LongBox");
            let runner = audit_class_named(unit, "Runner");
            let long_box_metadata = metadata
                .iter()
                .find(|class| class.identity().symbol() == long_box)
                .unwrap();
            assert_eq!(long_box_metadata.fields()[0].name(), "current");
            assert_eq!(long_box_metadata.fields()[0].descriptor(), "J");
            let constructor = long_box_metadata.constructor().unwrap();
            assert_eq!(constructor.parameters()[0].ty(), dotty_jvm::JvmType::Long);
            assert_eq!(constructor.parameters()[0].slot(), 1);
            assert_eq!(constructor.parameters()[0].width(), 2);

            for (class, method_name, parameter_count, returns_long) in [
                (long_box, "get", 0, true),
                (long_box, "set", 1, false),
                (runner, "constant", 0, true),
                (runner, "roundTrip", 1, true),
                (runner, "choose", 3, true),
                (runner, "repeat", 3, true),
            ] {
                let method = audit_method_named(unit, class, method_name);
                let Some(body) = method.body else {
                    panic!("{method_name} has a typed body")
                };
                let signature_id = match unit.store.symbols.info(method.method) {
                    dotty_core::SymbolInfo::Complete(signature) => signature,
                    other => panic!("{method_name} has a completed signature: {other:?}"),
                };
                let Type::Method(signature) = unit.store.types.get(*signature_id) else {
                    panic!("{method_name} has a method type")
                };
                assert_eq!(signature.params.len(), parameter_count, "{method_name}");
                if method_name != "constant" {
                    assert_eq!(
                        signature
                            .params
                            .iter()
                            .filter(|param| param.ty == definitions.long)
                            .count(),
                        match method_name {
                            "set" | "roundTrip" => 1,
                            "choose" | "repeat" => 2,
                            _ => 0,
                        },
                        "{method_name} retains canonical Long parameter types"
                    );
                }
                assert_eq!(
                    signature.result == definitions.long,
                    returns_long,
                    "{method_name}"
                );
                let typed_body = unit.typed_ast.get(body);
                assert!(
                    typed_body.position.is_some(),
                    "{method_name} body keeps source position"
                );
                if method_name == "constant" {
                    assert!(matches!(
                        &typed_body.kind,
                        TreeKind::Literal(literal) if literal.value == Constant::Long(9_000_000_000)
                    ));
                }
                if method_name == "roundTrip" || method_name == "repeat" {
                    let saved = method
                        .locals
                        .iter()
                        .find(|local| {
                            unit.store
                                .names
                                .resolve(unit.store.symbols.get(local.symbol).name.text())
                                == "saved"
                        })
                        .expect("saved local is bound by its canonical local symbol");
                    assert!(unit.typed_ast.get(saved.declaration).position.is_some());
                    assert_eq!(
                        *unit.store.symbols.info(saved.symbol),
                        dotty_core::SymbolInfo::Complete(definitions.long)
                    );
                }
            }
        },
    );

    assert!(
        observed,
        "parser, namer and typer completed before code generation: {result:?}"
    );
    let artifacts = result.expect("Long control-flow expressions emit after #1518");
    assert_eq!(artifacts.len(), 2);
    assert!(
        output.class_files().len() == 2,
        "both Long category-two classes are published together"
    );
}

#[test]
fn sibling_source_hierarchy_proves_the_canonical_common_base_deterministically() {
    let source = fs::read_to_string(fixture_path("SiblingConstructors.scala"))
        .expect("read source fixture for sibling constructors and dispatch");
    let mut baseline_classes: Option<Vec<(String, Vec<u8>)>> = None;
    let mut baseline_edges = None;

    for (session, reverse_plans) in [(0, false), (1, true)] {
        let output = TestOutputDirectory::new(&format!("sibling-hierarchy-session-{session}"));
        let mut observed_edges = None;
        let artifacts = compile_source_batch_observed(
            &source,
            "SiblingConstructors.scala",
            output.path(),
            SourceId::from_index(1434 + session),
            SourceUnitCardinality::AtLeastTwo,
            Some(&mut |unit, definitions| {
                let base = audit_class_named(unit, "Base");
                let left = audit_class_named(unit, "Left");
                let right = audit_class_named(unit, "Right");
                assert_ne!(base, left);
                assert_ne!(base, right);
                assert_ne!(left, right);
                assert_eq!(audit_class_parent(unit, left), base);
                assert_eq!(audit_class_parent(unit, right), base);
                assert_eq!(audit_class_parent(unit, base), definitions.object_class);

                let primary_constructor = |class| {
                    let plan = unit
                        .classes
                        .iter()
                        .find(|plan| plan.class == class)
                        .unwrap();
                    let JvmClassInitializationPlan::PrimaryConstructor(constructor) =
                        &plan.initialization
                    else {
                        panic!("source class has a primary constructor plan")
                    };
                    constructor
                };
                let base_constructor = primary_constructor(base).symbol();
                let mut sibling_parameters = Vec::new();
                for child in [left, right] {
                    let constructor = primary_constructor(child);
                    let [seed] = constructor.parameters() else {
                        panic!("sibling constructor has exactly one source parameter")
                    };
                    sibling_parameters.push(*seed);
                    assert_eq!(
                        unit.store.symbols.get(*seed).owner,
                        Some(constructor.symbol())
                    );
                    let super_constructor = constructor
                        .direct_super_constructor()
                        .expect("sibling has an accessible direct-parent constructor");
                    assert_eq!(super_constructor.parent_class(), base);
                    assert_eq!(super_constructor.parent_constructor(), base_constructor);
                    let [argument] = super_constructor.arguments() else {
                        panic!("direct parent application forwards one argument")
                    };
                    assert_eq!(
                        term_type_target(unit, unit.typed_ast.get(argument.typed).ty),
                        Some(*seed),
                        "each sibling forwards its own canonical seed parameter"
                    );
                }
                assert_ne!(sibling_parameters[0], sibling_parameters[1]);

                let metadata = unit
                    .metadata(definitions)
                    .expect("sibling class plans have valid dispatch metadata");
                let metadata_for = |name: &str| {
                    metadata
                        .iter()
                        .find(|class| class.identity().internal_name() == name)
                        .unwrap_or_else(|| panic!("missing {name} class metadata"))
                };
                let base_metadata = metadata_for("Base");
                let base_value = base_metadata
                    .methods()
                    .iter()
                    .find(|method| method.name() == "value")
                    .expect("Base declares value");
                let base_doubled = base_metadata
                    .methods()
                    .iter()
                    .find(|method| method.name() == "doubled")
                    .expect("Base declares doubled");
                let mut sibling_values = Vec::new();
                for name in ["Left", "Right"] {
                    let child_metadata = metadata_for(name);
                    let child_value = child_metadata
                        .methods()
                        .iter()
                        .find(|method| method.name() == "value")
                        .expect("each sibling declares its override");
                    sibling_values.push(child_value.symbol());
                    assert_ne!(child_value.symbol(), base_value.symbol());
                    let effective_value = child_metadata
                        .dispatch_methods()
                        .iter()
                        .find(|method| method.name() == "value")
                        .expect("sibling override is effective for its receiver");
                    assert_eq!(effective_value.method(), child_value.symbol());
                    assert_eq!(
                        effective_value.declaring_class(),
                        child_metadata.identity().symbol()
                    );
                    assert_eq!(
                        effective_value.receiver_class(),
                        child_metadata.identity().symbol()
                    );
                    let inherited_doubled = child_metadata
                        .dispatch_methods()
                        .iter()
                        .find(|method| method.name() == "doubled")
                        .expect("Base.doubled is inherited by each sibling");
                    assert_eq!(inherited_doubled.method(), base_doubled.symbol());
                    assert_eq!(
                        inherited_doubled.declaring_class(),
                        base_metadata.identity().symbol()
                    );
                    assert_eq!(
                        inherited_doubled.receiver_class(),
                        child_metadata.identity().symbol()
                    );
                    assert_eq!(child_metadata.methods().len(), 1);
                }
                assert_ne!(sibling_values[0], sibling_values[1]);
                for child in [left, right] {
                    let body = audit_method_named(unit, child, "value")
                        .body
                        .expect("sibling override has a body");
                    let targets = typed_tree_targets(unit, body);
                    assert!(!targets.is_empty());
                    assert!(
                        targets.contains(&base_value.symbol()),
                        "super calls resolve to Base.value: {targets:?}"
                    );
                }

                let hierarchy = unit
                    .class_hierarchy(definitions)
                    .expect("the real source sibling graph validates with both constructors");
                assert_eq!(
                    hierarchy.nearest_common_non_object_ancestor(left, right),
                    Some(base)
                );
                assert_eq!(
                    hierarchy.nearest_common_non_object_ancestor(right, left),
                    Some(base)
                );
                assert_eq!(
                    hierarchy.nearest_common_non_object_ancestor(left, left),
                    Some(left)
                );
                assert_eq!(
                    hierarchy.nearest_common_non_object_ancestor(left, base),
                    Some(base)
                );

                for (child, expected_parent) in [(left, base), (right, base)] {
                    let class_plan = unit
                        .classes
                        .iter()
                        .find(|plan| plan.class == child)
                        .unwrap();
                    let JvmClassInitializationPlan::PrimaryConstructor(constructor) =
                        &class_plan.initialization
                    else {
                        panic!("source sibling has a primary constructor plan")
                    };
                    assert_eq!(constructor.parameters().len(), 1);
                    assert_eq!(
                        unit.store.symbols.get(constructor.parameters()[0]).owner,
                        Some(constructor.symbol())
                    );
                    assert_eq!(
                        constructor
                            .direct_super_constructor()
                            .expect("sibling has an accessible direct-parent constructor")
                            .parent_class(),
                        expected_parent
                    );
                }

                let internal_edges = hierarchy.internal_name_edges();
                let classfile_hierarchy = dotty_classfile::bytecode::ClassHierarchy::from_edges(
                    internal_edges.iter().map(|edge| {
                        (
                            edge.child_internal_name().to_owned(),
                            edge.parent_internal_name().to_owned(),
                        )
                    }),
                )
                .expect("canonical sibling edges project into the class-file hierarchy");
                assert!(classfile_hierarchy.is_assignable("Left", "Base"));
                assert!(classfile_hierarchy.is_assignable("Right", "Base"));

                let mut normalized_edges = internal_edges
                    .iter()
                    .map(|edge| {
                        (
                            edge.child_internal_name().to_owned(),
                            edge.parent_internal_name().to_owned(),
                        )
                    })
                    .collect::<Vec<_>>();
                normalized_edges.sort();
                observed_edges = Some(normalized_edges);

                if reverse_plans {
                    unit.classes.reverse();
                    let reordered = unit
                        .class_hierarchy(definitions)
                        .expect("permuting class plan order preserves the validated proof");
                    assert_eq!(
                        reordered.nearest_common_non_object_ancestor(left, right),
                        Some(base)
                    );
                }
            }),
        )
        .expect("hierarchy queries do not change sibling class emission");

        let mut emitted = artifacts
            .iter()
            .map(|artifact| {
                (
                    artifact.class.internal_name().to_owned(),
                    artifact.class.bytes().to_vec(),
                )
            })
            .collect::<Vec<_>>();
        emitted.sort_by(|left, right| left.0.cmp(&right.0));
        let edges = observed_edges.expect("the source frontend ran the hierarchy observer");
        if let Some(expected) = &baseline_classes {
            assert_eq!(
                &emitted, expected,
                "fresh sessions emit identical class bytes"
            );
            assert_eq!(Some(&edges), baseline_edges.as_ref());
        } else {
            baseline_classes = Some(emitted);
            baseline_edges = Some(edges);
        }
    }
}

#[test]
fn sibling_inheritance_batch_is_byte_for_byte_deterministic_across_fresh_frontends() {
    let source = fs::read_to_string(fixture_path("SiblingConstructors.scala"))
        .expect("read the real sibling inheritance and conditional fixture");
    let first_output = TestOutputDirectory::new("sibling-inheritance-determinism-first");
    let second_output = TestOutputDirectory::new("sibling-inheritance-determinism-second");
    let first = compile_source_batch(
        &source,
        "SiblingConstructors.scala",
        first_output.path(),
        SourceId::from_index(1439),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect("first independent source frontend emits the sibling batch");
    let second = compile_source_batch(
        &source,
        "SiblingConstructors.scala",
        second_output.path(),
        SourceId::from_index(1440),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect("second independent source frontend emits the sibling batch");
    let identity_and_bytes = |artifacts: &[EmittedArtifact]| {
        artifacts
            .iter()
            .map(|artifact| {
                (
                    artifact.class.internal_name().to_owned(),
                    artifact.class.bytes().to_vec(),
                )
            })
            .collect::<Vec<_>>()
    };
    let expected_names = ["Base", "Left", "Right", "Runner"];
    assert_eq!(
        first
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        expected_names
    );
    assert_eq!(
        second
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        expected_names
    );
    assert_eq!(identity_and_bytes(&first), identity_and_bytes(&second));
    for (output, artifacts) in [(&first_output, &first), (&second_output, &second)] {
        assert_eq!(
            output.class_files(),
            expected_names.map(|name| output.path().join(format!("{name}.class")))
        );
        for artifact in artifacts {
            decode_emitted_class(artifact);
        }
    }
}

#[test]
fn sibling_constructor_dispatch_batch_verifies_and_runs_on_the_jvm() {
    let output = TestOutputDirectory::new("sibling-constructors-runtime");
    let artifacts = compile_fixture_classes("SiblingConstructors.scala", output.path())
        .expect("emit the complete sibling constructor and dispatch batch");
    assert_eq!(artifacts.len(), 4);
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Base", "Left", "Right", "Runner"],
        "the complete class batch is published in source declaration order"
    );
    assert_eq!(
        output.class_files(),
        [
            output.path().join("Base.class"),
            output.path().join("Left.class"),
            output.path().join("Right.class"),
            output.path().join("Runner.class")
        ]
    );
    let decode_class = |name: &str| {
        let artifact = artifacts
            .iter()
            .find(|artifact| artifact.class.internal_name() == name)
            .unwrap_or_else(|| panic!("{name} class is emitted"));
        decode_emitted_class(artifact)
    };
    let base = decode_class("Base");
    let left = decode_class("Left");
    let right = decode_class("Right");
    let runner = decode_class("Runner");
    for (class, name) in [
        (&base, "Base"),
        (&left, "Left"),
        (&right, "Right"),
        (&runner, "Runner"),
    ] {
        assert_eq!(
            class.version.major, 65,
            "{name} uses the JDK 21 classfile version"
        );
        assert!(class.access_flags.is_public(), "{name} is public");
        assert!(
            !class.access_flags.is_interface(),
            "{name} is an ordinary class"
        );
        assert!(
            class.interfaces.is_empty(),
            "{name} has no fabricated interfaces"
        );
        assert_eq!(class.constant_pool.class_name(class.this_class), Ok(name));
    }
    assert_eq!(
        base.constant_pool
            .class_name(base.super_class.expect("Base has a superclass")),
        Ok("java/lang/Object")
    );
    assert_eq!(
        left.constant_pool
            .class_name(left.super_class.expect("Left has a superclass")),
        Ok("Base")
    );
    assert_eq!(
        right
            .constant_pool
            .class_name(right.super_class.expect("Right has a superclass")),
        Ok("Base")
    );
    assert_eq!(
        runner
            .constant_pool
            .class_name(runner.super_class.expect("Runner has a superclass")),
        Ok("java/lang/Object")
    );
    assert_eq!(
        methods_with_descriptors(&base),
        [
            ("<init>".to_owned(), "(I)V".to_owned()),
            ("value".to_owned(), "()I".to_owned()),
            ("doubled".to_owned(), "()I".to_owned())
        ]
    );
    for method in &base.methods {
        assert!(method.access_flags.is_public(), "Base methods are public");
        assert!(
            !method.access_flags.is_static(),
            "Base methods are instance methods"
        );
    }
    assert_eq!(base.fields.len(), 1, "Base owns the sole current field");
    let current = &base.fields[0];
    assert!(current.access_flags.is_private());
    assert_eq!(base.constant_pool.utf8(current.name_index), Ok("current"));
    assert_eq!(base.constant_pool.utf8(current.descriptor_index), Ok("I"));
    assert_invocation_sequence(
        &base,
        emitted_method_code(&base, "<init>"),
        &[(0xb7, "java/lang/Object", "<init>", "()V")],
    );
    assert_field_reference(
        &base,
        emitted_method_code(&base, "<init>"),
        0xb5,
        "Base",
        "current",
        "I",
    );
    assert_eq!(
        decode_subset_instructions(emitted_method_code(&base, "<init>"))
            .iter()
            .map(|(_, opcode)| *opcode)
            .collect::<Vec<_>>(),
        [0x2a, 0xb7, 0x2a, 0x1b, 0xb5, 0xb1],
        "Base alone initializes current from its own seed parameter after Object construction"
    );
    for class in [&left, &right] {
        assert_eq!(
            class.fields.len(),
            0,
            "children do not redeclare Base state"
        );
        assert_eq!(
            methods_with_descriptors(class),
            [
                ("<init>".to_owned(), "(I)V".to_owned()),
                ("value".to_owned(), "()I".to_owned())
            ],
            "each child declares only its forwarding constructor and override"
        );
        for method in &class.methods {
            assert!(method.access_flags.is_public(), "child methods are public");
            assert!(
                !method.access_flags.is_static(),
                "child methods are instance methods"
            );
        }
        assert_invocation_sequence(
            class,
            emitted_method_code(class, "<init>"),
            &[(0xb7, "Base", "<init>", "(I)V")],
        );
        assert_invocation_sequence(
            class,
            emitted_method_code(class, "value"),
            &[(0xb7, "Base", "value", "()I")],
        );
        assert_eq!(
            decode_subset_instructions(emitted_method_code(class, "<init>"))
                .iter()
                .map(|(_, opcode)| *opcode)
                .collect::<Vec<_>>(),
            [0x2a, 0x1b, 0xb7, 0xb1],
            "each sibling forwards its own constructor parameter from local slot one"
        );
    }
    assert_invocation_sequence(
        &base,
        emitted_method_code(&base, "doubled"),
        &[
            (0xb6, "Base", "value", "()I"),
            (0xb6, "Base", "value", "()I"),
        ],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "evaluate"),
        &[(0xb6, "Base", "doubled", "()I")],
    );
    for (method, child) in [("fromLeft", "Left"), ("fromRight", "Right")] {
        assert_invocation_sequence(
            &runner,
            emitted_method_code(&runner, method),
            &[
                (0xb7, child, "<init>", "(I)V"),
                (0xb6, "Runner", "evaluate", "(LBase;)I"),
            ],
        );
    }
    for method in ["choose", "reverseChoose"] {
        assert!(
            methods_with_descriptors(&runner)
                .contains(&(method.to_owned(), "(ZI)LBase;".to_owned()))
        );
        assert!(
            stack_map_frames(&runner, method).iter().any(|frame| {
                frame.stack == [StackMapVerificationType::Object("Base".to_owned())]
            })
        );
    }
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "run"),
        &[
            (0xb6, "Runner", "choose", "(ZI)LBase;"),
            (0xb6, "Base", "doubled", "()I"),
        ],
    );
    assert_eq!(
        methods_with_descriptors(&runner),
        [
            ("<init>".to_owned(), "()V".to_owned()),
            ("evaluate".to_owned(), "(LBase;)I".to_owned()),
            ("fromLeft".to_owned(), "(I)I".to_owned()),
            ("fromRight".to_owned(), "(I)I".to_owned()),
            ("choose".to_owned(), "(ZI)LBase;".to_owned()),
            ("reverseChoose".to_owned(), "(ZI)LBase;".to_owned()),
            ("run".to_owned(), "(ZI)I".to_owned()),
            ("acceptChoice".to_owned(), "(ZI)I".to_owned())
        ]
    );
    for method in ["choose", "reverseChoose"] {
        let code = emitted_method_code(&runner, method);
        let targets = decode_branch_targets(code);
        let frames = stack_map_frames(&runner, method);
        let base_join = frames
            .iter()
            .find(|frame| frame.stack == [StackMapVerificationType::Object("Base".to_owned())])
            .expect("the StackMapTable resolves the common Base through a CONSTANT_Class");
        assert!(
            base_join.pc < code.len(),
            "join frame PC is inside the Code array"
        );
        assert!(
            targets.iter().any(|(_, _, target)| *target == base_join.pc),
            "the Base frame starts at the emitted branch join"
        );
        assert!(
            decode_subset_instructions(code)
                .iter()
                .all(|(_, opcode)| *opcode != 0xc0),
            "{method} contains no inserted checkcast"
        );
        let method_info = runner
            .methods
            .iter()
            .find(|method_info| runner.constant_pool.utf8(method_info.name_index) == Ok(method))
            .unwrap();
        let [Attribute::Code(code_attribute)] = method_info.attributes.as_slice() else {
            panic!("{method} has one Code attribute")
        };
        assert!(code_attribute.max_stack >= 3);
        assert_eq!(code_attribute.max_locals, 3);
    }

    if !require_jdk_or_skip() {
        return;
    }
    let harness = output.path().join("SiblingConstructorsHarness.java");
    fs::write(
        &harness,
        r#"public final class SiblingConstructorsHarness {
    public static void main(String[] args) {
        Runner runner = new Runner();
        Base base = new Base(10);
        if (base.doubled() != 20) throw new AssertionError("base dispatch");
        if (new Left(10).doubled() != 22) throw new AssertionError("left override");
        if (new Right(10).doubled() != 24) throw new AssertionError("right override");
        if (runner.fromLeft(10) != 22) throw new AssertionError("left dispatch");
        if (runner.fromRight(10) != 24) throw new AssertionError("right dispatch");
        if (runner.fromLeft(-3) != -4) throw new AssertionError("left negative seed");
        if (runner.fromRight(-3) != -2) throw new AssertionError("right negative seed");
        Base leftChoice = runner.choose(true, 10);
        Base rightChoice = runner.choose(false, 10);
        if (leftChoice.getClass() != Left.class || rightChoice.getClass() != Right.class) throw new AssertionError("exact runtime classes");
        if (leftChoice.doubled() != 22) throw new AssertionError("left If result");
        if (rightChoice.doubled() != 24) throw new AssertionError("right If result");
        if (runner.reverseChoose(true, 10).doubled() != 24) throw new AssertionError("reversed right If result");
        if (runner.reverseChoose(false, 10).doubled() != 22) throw new AssertionError("reversed left If result");
        if (runner.run(true, 10) != 22 || runner.run(false, 10) != 24) throw new AssertionError("Base receiver");
        if (runner.acceptChoice(true, 10) != 22 || runner.acceptChoice(false, 10) != 24) throw new AssertionError("Base argument");
        if (runner.run(true, -3) != -4 || runner.run(false, -3) != -2) throw new AssertionError("negative seeds");
        if (runner.run(true, 0) != 2 || runner.run(false, 0) != 4) throw new AssertionError("zero seeds");
    }
}
"#,
    )
    .unwrap();
    let classpath = output.path().to_string_lossy().into_owned();
    let javac = Command::new("javac")
        .args(["-classpath", &classpath, "-d", &classpath])
        .arg(&harness)
        .output()
        .expect("start javac for sibling runtime assertions");
    assert!(
        javac.status.success(),
        "javac rejected sibling runtime harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let javap = Command::new("javap")
        .args(["-c", "-v", "-classpath", &classpath, "Runner"])
        .output()
        .expect("start javap for sibling-inheritance bytecode assertions");
    assert!(
        javap.status.success(),
        "javap rejected Runner.class\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javap.stdout),
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap = String::from_utf8_lossy(&javap.stdout);
    assert!(javap.contains("(ZI)LBase;"));
    assert!(javap.contains("StackMapTable"));
    assert!(javap.contains("Base.doubled:()I"));
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            &classpath,
            "SiblingConstructorsHarness",
        ])
        .output()
        .expect("start java for sibling runtime assertions");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected sibling dispatch\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
}

fn reject_sibling_batch_with_observer(
    source_index: u32,
    mut observe: impl for<'a> FnMut(&mut JvmCompilationUnit<'a>, &Definitions),
) -> PipelineFailure {
    let source = fs::read_to_string(fixture_path("SiblingConstructors.scala"))
        .expect("read source fixture for sibling rejection cases");
    let output = TestOutputDirectory::new(&format!("sibling-rejection-{source_index}"));
    let result = compile_source_batch_observed(
        &source,
        "SiblingConstructors.scala",
        output.path(),
        SourceId::from_index(source_index),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut observe),
    );
    assert!(
        output.class_files().is_empty(),
        "invalid sibling batches must not publish a partial classfile set"
    );
    result.expect_err("the deliberately invalid sibling plans must be rejected")
}

#[test]
fn sibling_constructor_plan_cannot_be_reused_by_its_sibling() {
    let mut observed_constructors = None;
    let error = reject_sibling_batch_with_observer(1440, |unit, _| {
        let left = audit_class_named(unit, "Left");
        let right = audit_class_named(unit, "Right");
        let left_index = unit
            .classes
            .iter()
            .position(|plan| plan.class == left)
            .unwrap();
        let right_index = unit
            .classes
            .iter()
            .position(|plan| plan.class == right)
            .unwrap();
        let JvmClassInitializationPlan::PrimaryConstructor(right_constructor) =
            &unit.classes[right_index].initialization
        else {
            panic!("Right has its own primary-constructor plan")
        };
        observed_constructors = Some((left, right, right_constructor.symbol()));
        unit.classes[left_index].initialization = unit.classes[right_index].initialization.clone();
    });
    let (left, right, right_constructor) =
        observed_constructors.expect("observer saw both siblings");
    assert!(matches!(
        error,
        PipelineFailure::Codegen(JvmCodegenError::InvalidPrimaryConstructor {
            constructor,
            issue: dotty_jvm::JvmConstructorIssue::SourceIdentity,
        }) if constructor == right_constructor
    ));
    assert_ne!(left, right);
}

#[test]
fn sibling_method_plan_cannot_select_its_siblings_override_symbol() {
    let mut observed_methods = None;
    let error = reject_sibling_batch_with_observer(1441, |unit, _| {
        let left = audit_class_named(unit, "Left");
        let right = audit_class_named(unit, "Right");
        let left_index = unit
            .classes
            .iter()
            .position(|plan| plan.class == left)
            .unwrap();
        let right_value = audit_method_named(unit, right, "value").method;
        let left_value_index = unit.classes[left_index]
            .methods
            .iter()
            .position(|method| {
                unit.store
                    .names
                    .resolve(unit.store.symbols.get(method.method).name.text())
                    == "value"
            })
            .unwrap();
        observed_methods = Some((left, right, right_value));
        unit.classes[left_index].methods[left_value_index].method = right_value;
    });
    let (left, right, right_value) = observed_methods.expect("observer saw both overrides");
    assert!(matches!(
        error,
        PipelineFailure::Codegen(JvmCodegenError::WrongOwner {
            symbol,
            expected,
            actual: Some(actual),
        }) if symbol == right_value && expected == left && actual == right
    ));
}

#[test]
fn sibling_batch_rejects_a_base_without_its_required_parent_constructor_proof() {
    let mut observed_classes = None;
    let error = reject_sibling_batch_with_observer(1442, |unit, _| {
        let base = audit_class_named(unit, "Base");
        let left = audit_class_named(unit, "Left");
        let base_index = unit
            .classes
            .iter()
            .position(|plan| plan.class == base)
            .unwrap();
        observed_classes = Some((base, left));
        unit.classes[base_index].initialization = JvmClassInitializationPlan::Unsupported;
    });
    let (base, left) = observed_classes.expect("observer saw the base and child classes");
    assert!(matches!(
        error,
        PipelineFailure::Codegen(JvmCodegenError::InvalidJvmSuperclass {
            class,
            parent: Some(parent),
            issue: dotty_jvm::JvmSuperclassIssue::ParentConstructorUnavailable,
        }) if class == left && parent == base
    ));
}

#[test]
fn object_graph_construction_audit_emits_after_frontend_identity_validation() {
    let source = fs::read_to_string(fixture_path("ObjectGraphConstruction.scala"))
        .expect("read the bounded object-graph source fixture");
    let output = TestOutputDirectory::new("object-graph-construction-audit");
    let mut observed = false;
    let source_id = SourceId::from_index(1492);
    let mut inspect = |ast: &AstArena<Untyped>,
                       index: &SourceSemanticIndex,
                       unit: &mut JvmCompilationUnit<'_>,
                       definitions: &Definitions| {
        observed = true;
        let counter = audit_class_named(unit, "Counter");
        let holder = audit_class_named(unit, "Holder");
        let owned_holder = audit_class_named(unit, "OwnedHolder");
        let pair = audit_class_named(unit, "Pair");
        let runner = audit_class_named(unit, "Runner");
        let primary_constructor = |class| {
            let plan = unit
                .classes
                .iter()
                .find(|plan| plan.class == class)
                .expect("source class has a JVM plan");
            let JvmClassInitializationPlan::PrimaryConstructor(constructor) = &plan.initialization
            else {
                panic!("source primary constructor proof is retained")
            };
            constructor
        };
        let counter_constructor = primary_constructor(counter).symbol();
        let holder_constructor = primary_constructor(holder).symbol();
        let owned_holder_constructor = primary_constructor(owned_holder).symbol();
        let pair_constructor = primary_constructor(pair).symbol();

        let owner_name = |symbol: SymbolId| {
            let owner = unit
                .store
                .symbols
                .get(symbol)
                .owner
                .expect("selected call has a canonical declaring owner");
            unit.store
                .names
                .resolve(unit.store.symbols.get(owner).name.text())
                .to_owned()
        };
        for (symbol, owner) in [
            (counter_constructor, "Counter"),
            (holder_constructor, "Holder"),
            (owned_holder_constructor, "OwnedHolder"),
            (pair_constructor, "Pair"),
            (audit_method_named(unit, counter, "get").method, "Counter"),
            (audit_method_named(unit, holder, "get").method, "Holder"),
            (audit_method_named(unit, holder, "value").method, "Holder"),
            (
                audit_method_named(unit, runner, "makeCounter").method,
                "Runner",
            ),
            (
                audit_method_named(unit, runner, "makeHolder").method,
                "Runner",
            ),
            (audit_method_named(unit, runner, "create").method, "Runner"),
        ] {
            assert_eq!(owner_name(symbol), owner, "selected symbol {symbol:?}");
        }

        let targets = |method: &str| {
            let body = audit_method_named(unit, runner, method)
                .body
                .unwrap_or_else(|| panic!("Runner.{method} has a typed body"));
            typed_tree_targets(unit, body)
        };
        let final_expression = |body: TreeId<Typed>| match &unit.typed_ast.get(body).kind {
            TreeKind::Block(block) => block.expr,
            _ => body,
        };
        let create_targets = targets("create");
        assert!(create_targets.contains(&holder_constructor));
        assert!(create_targets.contains(&counter_constructor));
        let factory_targets = targets("fromFactory");
        assert!(factory_targets.contains(&holder_constructor));
        assert!(factory_targets.contains(&audit_method_named(unit, runner, "makeCounter").method));
        let chain_targets = targets("read");
        assert!(chain_targets.contains(&audit_method_named(unit, runner, "create").method));
        assert!(chain_targets.contains(&audit_method_named(unit, holder, "value").method));
        assert!(chain_targets.contains(&audit_method_named(unit, counter, "get").method));
        let factory_chain_targets = targets("chainedFactory");
        assert!(
            factory_chain_targets.contains(&audit_method_named(unit, runner, "makeHolder").method)
        );
        assert!(factory_chain_targets.contains(&audit_method_named(unit, holder, "value").method));
        assert!(factory_chain_targets.contains(&audit_method_named(unit, counter, "get").method));
        let chained_factory = audit_method_named(unit, runner, "chainedFactory");
        let TreeKind::Apply(get_call) = &unit.typed_ast.get(chained_factory.body.unwrap()).kind
        else {
            panic!("chainedFactory ends in the typed Counter.get application")
        };
        assert_eq!(
            term_type_target(unit, unit.typed_ast.get(get_call.function).ty),
            Some(audit_method_named(unit, counter, "get").method)
        );
        let TreeKind::Select(get_selection) = &unit.typed_ast.get(get_call.function).kind else {
            panic!("Counter.get remains selected on the value() result")
        };
        let TreeKind::Apply(value_call) = &unit.typed_ast.get(get_selection.qualifier).kind else {
            panic!("Holder.value is applied once to the factory result")
        };
        assert_eq!(
            term_type_target(unit, unit.typed_ast.get(value_call.function).ty),
            Some(audit_method_named(unit, holder, "value").method)
        );
        let TreeKind::Select(value_selection) = &unit.typed_ast.get(value_call.function).kind
        else {
            panic!("Holder.value remains selected on the makeHolder result")
        };
        let TreeKind::Apply(factory_call) = &unit.typed_ast.get(value_selection.qualifier).kind
        else {
            panic!("Runner.makeHolder is applied once as the receiver")
        };
        assert_eq!(
            term_type_target(unit, unit.typed_ast.get(factory_call.function).ty),
            Some(audit_method_named(unit, runner, "makeHolder").method)
        );
        assert_eq!(
            unit.typed_ast.get(chained_factory.body.unwrap()).ty,
            definitions.int,
            "the chained method result retains its canonical Int type"
        );
        let direct_new_receiver_targets = targets("readOwned");
        assert!(direct_new_receiver_targets.contains(&owned_holder_constructor));
        assert!(
            direct_new_receiver_targets
                .contains(&audit_method_named(unit, owned_holder, "get").method)
        );
        for method in ["existing", "fromImmutable", "fromMutable", "fromNull"] {
            assert!(
                targets(method).contains(&holder_constructor),
                "Runner.{method} retains the canonical Holder constructor application"
            );
        }
        let from_local_targets = targets("fromLocal");
        assert!(from_local_targets.contains(&holder_constructor));
        let pair_targets = targets("createPair");
        assert!(pair_targets.contains(&pair_constructor));
        assert_eq!(
            pair_targets
                .iter()
                .filter(|target| **target == counter_constructor)
                .count(),
            2,
            "both Pair arguments retain the canonical Counter constructor"
        );

        let existing = audit_method_named(unit, runner, "existing");
        let TreeKind::Apply(existing_call) = &unit.typed_ast.get(existing.body.unwrap()).kind
        else {
            panic!("existing remains the typed Holder constructor application")
        };
        assert_eq!(existing.parameters.len(), 1);
        assert_eq!(existing_call.args.len(), 1);
        assert_eq!(
            term_type_target(unit, unit.typed_ast.get(existing_call.args[0]).ty),
            Some(existing.parameters[0]),
            "constructor argument keeps the exact source method parameter"
        );

        let from_mutable = audit_method_named(unit, runner, "fromMutable");
        let mutable_locals = from_mutable
            .locals
            .iter()
            .filter(|local| local.mutable)
            .collect::<Vec<_>>();
        let [mutable_local] = mutable_locals.as_slice() else {
            panic!("fromMutable has one planned mutable reference local")
        };
        let TreeKind::Block(block) = &unit.typed_ast.get(from_mutable.body.unwrap()).kind else {
            panic!("fromMutable preserves its source block")
        };
        let TreeKind::Apply(mutable_call) = &unit.typed_ast.get(block.expr).kind else {
            panic!("fromMutable ends with the Holder constructor application")
        };
        assert_eq!(mutable_call.args.len(), 1);
        assert_eq!(
            term_type_target(unit, unit.typed_ast.get(mutable_call.args[0]).ty),
            Some(mutable_local.symbol),
            "mutable constructor argument retains its local SymbolId"
        );

        let from_immutable = audit_method_named(unit, runner, "fromImmutable");
        let immutable_local = from_immutable
            .locals
            .iter()
            .find(|local| !local.mutable)
            .expect("fromImmutable has a planned immutable reference local");
        let TreeKind::Block(immutable_block) =
            &unit.typed_ast.get(from_immutable.body.unwrap()).kind
        else {
            panic!("fromImmutable preserves its local binding block")
        };
        let TreeKind::Apply(immutable_call) = &unit.typed_ast.get(immutable_block.expr).kind else {
            panic!("fromImmutable ends with the Holder constructor application")
        };
        assert_eq!(immutable_call.args.len(), 1);
        assert_eq!(
            term_type_target(unit, unit.typed_ast.get(immutable_call.args[0]).ty),
            Some(immutable_local.symbol),
            "immutable constructor argument retains its local SymbolId"
        );

        let from_null = audit_method_named(unit, runner, "fromNull");
        let TreeKind::Apply(null_call) = &unit.typed_ast.get(from_null.body.unwrap()).kind else {
            panic!("fromNull remains the typed Holder constructor application")
        };
        let TreeKind::Literal(null_literal) = &unit.typed_ast.get(null_call.args[0]).kind else {
            panic!("null constructor argument remains a literal")
        };
        assert_eq!(null_literal.value, Constant::Null);
        assert_eq!(
            unit.typed_ast.get(null_call.args[0]).ty,
            definitions.null_type
        );

        let create = audit_method_named(unit, runner, "create");
        let create_body = create.body.unwrap();
        let create_result = final_expression(create_body);
        let TreeKind::Apply(outer) = &unit.typed_ast.get(create_result).kind else {
            panic!("create remains the typed Holder constructor application")
        };
        assert_eq!(
            term_type_target(unit, unit.typed_ast.get(outer.function).ty),
            Some(holder_constructor)
        );
        assert_eq!(outer.args.len(), 1);
        let inner = outer.args[0];
        assert!(matches!(unit.typed_ast.get(inner).kind, TreeKind::Apply(_)));
        assert_eq!(
            audit_type_symbol(unit, unit.typed_ast.get(inner).ty),
            Some(counter)
        );
        assert_eq!(
            audit_type_symbol(unit, unit.typed_ast.get(create_body).ty),
            Some(holder)
        );

        let from_factory = audit_method_named(unit, runner, "fromFactory");
        let TreeKind::Apply(outer) = &unit.typed_ast.get(from_factory.body.unwrap()).kind else {
            panic!("fromFactory remains the typed Holder constructor application")
        };
        assert_eq!(
            term_type_target(unit, unit.typed_ast.get(outer.function).ty),
            Some(holder_constructor)
        );
        assert!(matches!(
            unit.typed_ast.get(outer.args[0]).kind,
            TreeKind::Apply(_)
        ));
        assert_eq!(
            audit_type_symbol(unit, unit.typed_ast.get(outer.args[0]).ty),
            Some(counter)
        );

        let make_holder = audit_method_named(unit, runner, "makeHolder");
        let make_holder_body = make_holder.body.unwrap();
        let make_holder_result = final_expression(make_holder_body);
        let TreeKind::Apply(outer) = &unit.typed_ast.get(make_holder_result).kind else {
            panic!("makeHolder retains its typed Holder constructor application")
        };
        assert_eq!(
            term_type_target(unit, unit.typed_ast.get(outer.function).ty),
            Some(holder_constructor)
        );
        assert_eq!(outer.args.len(), 1);
        let TreeKind::Apply(inner) = &unit.typed_ast.get(outer.args[0]).kind else {
            panic!("makeHolder retains the nested Counter constructor application")
        };
        assert_eq!(
            term_type_target(unit, unit.typed_ast.get(inner.function).ty),
            Some(counter_constructor)
        );
        assert_eq!(inner.args.len(), 1);
        assert_eq!(
            term_type_target(unit, unit.typed_ast.get(inner.args[0]).ty),
            Some(make_holder.parameters[0]),
            "nested constructor argument keeps the exact makeHolder seed symbol"
        );
        assert_eq!(
            audit_type_symbol(unit, unit.typed_ast.get(outer.args[0]).ty),
            Some(counter)
        );
        assert_eq!(
            audit_type_symbol(unit, unit.typed_ast.get(make_holder_body).ty),
            Some(holder)
        );

        let dotty_core::SourceDefinition::Canonical {
            source,
            tree: holder_tree,
        } = index
            .definition_of(holder)
            .expect("Holder retains source provenance")
        else {
            panic!("Holder has a canonical source definition")
        };
        assert_eq!(source, source_id);
        let TreeKind::TypeDef(holder_definition) = &ast.get(holder_tree).kind else {
            panic!("Holder source definition is a TypeDef")
        };
        let TreeKind::Template(holder_template) = &ast.get(holder_definition.rhs).kind else {
            panic!("Holder source definition retains its Template")
        };
        let TreeKind::DefDef(holder_constructor_tree) = &ast.get(holder_template.constructor).kind
        else {
            panic!("Holder source primary constructor is a DefDef")
        };
        let [holder_source_parameter] = holder_constructor_tree.value_param_clauses[0].as_slice()
        else {
            panic!("Holder source constructor has one parameter")
        };
        let class_owned_counter = index
            .symbol_at(source_id, *holder_source_parameter)
            .expect("source parameter has a class-owned identity");
        let constructor_owned_counter = index
            .derived_symbol_at(holder_constructor, source_id, *holder_source_parameter)
            .expect("source parameter has a constructor-owned identity");
        assert_ne!(class_owned_counter, constructor_owned_counter);
        assert_eq!(
            unit.store.symbols.get(class_owned_counter).owner,
            Some(holder)
        );
        assert_eq!(
            unit.store.symbols.get(constructor_owned_counter).owner,
            Some(holder_constructor)
        );

        let owned_plan = unit
            .classes
            .iter()
            .find(|plan| plan.class == owned_holder)
            .expect("OwnedHolder plan exists");
        let inner_field = owned_plan
            .fields
            .iter()
            .find(|field| {
                unit.store
                    .names
                    .resolve(unit.store.symbols.get(field.field).name.text())
                    == "inner"
            })
            .expect("OwnedHolder.inner has a field plan");
        let initializer = inner_field
            .initializer
            .expect("field initializer has a typed root");
        assert_eq!(
            audit_type_symbol(unit, unit.typed_ast.get(initializer).ty),
            Some(counter)
        );
        assert!(typed_tree_targets(unit, initializer).contains(&counter_constructor));
        let TreeKind::Apply(field_allocation) = &unit.typed_ast.get(initializer).kind else {
            panic!("OwnedHolder.inner keeps the Counter constructor application")
        };
        assert_eq!(field_allocation.args.len(), 1);
        let dotty_core::SourceDefinition::Canonical {
            tree: owned_tree, ..
        } = index
            .definition_of(owned_holder)
            .expect("OwnedHolder retains source provenance")
        else {
            panic!("OwnedHolder has a canonical source definition")
        };
        let TreeKind::TypeDef(owned_definition) = &ast.get(owned_tree).kind else {
            panic!("OwnedHolder source definition is a TypeDef")
        };
        let TreeKind::Template(owned_template) = &ast.get(owned_definition.rhs).kind else {
            panic!("OwnedHolder retains its source Template")
        };
        let TreeKind::DefDef(owned_constructor_tree) = &ast.get(owned_template.constructor).kind
        else {
            panic!("OwnedHolder primary constructor is a DefDef")
        };
        let [owned_seed_tree] = owned_constructor_tree.value_param_clauses[0].as_slice() else {
            panic!("OwnedHolder source constructor has one seed parameter")
        };
        let owned_seed = index
            .derived_symbol_at(owned_holder_constructor, source_id, *owned_seed_tree)
            .expect("OwnedHolder seed has its constructor-owned identity");
        assert_eq!(
            term_type_target(unit, unit.typed_ast.get(field_allocation.args[0]).ty),
            Some(owned_seed),
            "field allocation argument retains the constructor-owned seed"
        );
        let Type::Method(signature) =
            unit.store
                .types
                .get(match unit.store.symbols.info(owned_holder_constructor) {
                    dotty_core::SymbolInfo::Complete(ty) => *ty,
                    other => {
                        panic!("OwnedHolder constructor has complete signature: {other:?}")
                    }
                })
        else {
            panic!("OwnedHolder constructor signature is a method")
        };
        assert_eq!(signature.params.len(), 1);
        assert_eq!(owner_name(owned_holder_constructor), "OwnedHolder");
    };
    let result = compile_source_batch_with_source_observer(
        &source,
        "ObjectGraphConstruction.scala",
        output.path(),
        source_id,
        SourceUnitCardinality::AtLeastTwo,
        &mut inspect,
    );
    assert!(
        observed,
        "the real frontend completed before backend emission"
    );
    let artifacts =
        result.expect("the audited object graph now emits its direct-new field initializer");
    assert_eq!(artifacts.len(), 5, "the complete source batch is emitted");
    assert_eq!(
        output.class_files().len(),
        5,
        "all classfiles are published"
    );
}

#[test]
fn object_graph_acceptance_batch_is_complete_deterministic_and_jdk_verified() {
    let source = fs::read_to_string(fixture_path("ObjectGraphConstruction.scala"))
        .expect("read the source-backed object graph fixture");
    let output = TestOutputDirectory::new("object-graph-chained-receivers");
    let artifacts = compile_source_batch(
        &source,
        "ObjectGraphConstruction.scala",
        output.path(),
        SourceId::from_index(1499),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect("real object graph source emits chained reference receivers");

    let repeated_output = TestOutputDirectory::new("object-graph-chained-receivers-repeat");
    let repeated = compile_source_batch(
        &source,
        "ObjectGraphConstruction.scala",
        repeated_output.path(),
        SourceId::from_index(1500),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect("a fresh frontend session emits the same source batch");
    assert_eq!(artifacts.len(), repeated.len());
    for (first, second) in artifacts.iter().zip(&repeated) {
        assert_eq!(first.class.internal_name(), second.class.internal_name());
        assert_eq!(first.class.bytes(), second.class.bytes());
    }
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Counter", "Holder", "OwnedHolder", "Pair", "Runner"],
        "complete class batch publishes in source order"
    );
    assert_eq!(output.class_files().len(), 5);
    for artifact in &artifacts {
        let class = decode_emitted_class(artifact);
        class.validate_references().unwrap_or_else(|error| {
            panic!(
                "{} references validate: {error}",
                artifact.class.internal_name()
            )
        });
        assert_eq!(class.version.major, 65);
        let super_class = class
            .super_class
            .expect("emitted classes have a superclass");
        assert_eq!(
            class.constant_pool.class_name(super_class),
            Ok("java/lang/Object"),
            "{} has its checked Object superclass",
            artifact.class.internal_name()
        );
    }

    let runner_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Runner")
        .expect("Runner is emitted");
    let runner = decode_emitted_class(runner_artifact);
    runner
        .validate_references()
        .expect("all object-graph constant-pool references resolve");
    let counter = decode_emitted_class(
        artifacts
            .iter()
            .find(|artifact| artifact.class.internal_name() == "Counter")
            .expect("Counter is emitted"),
    );
    let holder = decode_emitted_class(
        artifacts
            .iter()
            .find(|artifact| artifact.class.internal_name() == "Holder")
            .expect("Holder is emitted"),
    );
    let owned_holder = decode_emitted_class(
        artifacts
            .iter()
            .find(|artifact| artifact.class.internal_name() == "OwnedHolder")
            .expect("OwnedHolder is emitted"),
    );
    let pair = decode_emitted_class(
        artifacts
            .iter()
            .find(|artifact| artifact.class.internal_name() == "Pair")
            .expect("Pair is emitted"),
    );
    for (class, field_name, descriptor, is_final) in [
        (&counter, "current", "I", false),
        (&holder, "inner", "LCounter;", true),
        (&owned_holder, "inner", "LCounter;", true),
        (&pair, "savedFirst", "LCounter;", true),
        (&pair, "savedSecond", "LCounter;", true),
        (&runner, "receiverCalls", "I", false),
    ] {
        let field = class
            .fields
            .iter()
            .find(|field| class.constant_pool.utf8(field.name_index) == Ok(field_name))
            .unwrap_or_else(|| panic!("missing field {field_name}"));
        assert_eq!(
            class.constant_pool.utf8(field.descriptor_index),
            Ok(descriptor),
            "field {field_name} descriptor"
        );
        assert!(
            field.access_flags.is_private(),
            "field {field_name} is private"
        );
        assert_eq!(
            field.access_flags.is_final(),
            is_final,
            "field {field_name} finality"
        );
    }
    assert!(methods_with_descriptors(&counter).contains(&("<init>".into(), "(I)V".into())));
    assert!(methods_with_descriptors(&holder).contains(&("<init>".into(), "(LCounter;)V".into())));
    assert!(methods_with_descriptors(&owned_holder).contains(&("<init>".into(), "(I)V".into())));
    assert!(
        methods_with_descriptors(&pair)
            .contains(&("<init>".into(), "(LCounter;LCounter;)V".into()))
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "read"),
        &[
            (0xb6, "Runner", "create", "(I)LHolder;"),
            (0xb6, "Holder", "value", "()LCounter;"),
            (0xb6, "Counter", "get", "()I"),
        ],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "chainedFactory"),
        &[
            (0xb6, "Runner", "makeHolder", "(I)LHolder;"),
            (0xb6, "Holder", "value", "()LCounter;"),
            (0xb6, "Counter", "get", "()I"),
        ],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "readOwned"),
        &[
            (0xb7, "OwnedHolder", "<init>", "(I)V"),
            (0xb6, "OwnedHolder", "get", "()I"),
        ],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "fromFactory"),
        &[
            (0xb6, "Runner", "makeCounter", "(I)LCounter;"),
            (0xb7, "Holder", "<init>", "(LCounter;)V"),
        ],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "fromLocal"),
        &[
            (0xb7, "Counter", "<init>", "(I)V"),
            (0xb7, "Holder", "<init>", "(LCounter;)V"),
        ],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "createPair"),
        &[
            (0xb7, "Counter", "<init>", "(I)V"),
            (0xb7, "Counter", "<init>", "(I)V"),
            (0xb7, "Pair", "<init>", "(LCounter;LCounter;)V"),
        ],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "fromNull"),
        &[(0xb7, "Holder", "<init>", "(LCounter;)V")],
    );
    assert_eq!(
        (
            code_max_stack(&runner, "createPair"),
            code_max_locals(&runner, "createPair")
        ),
        (6, 3)
    );
    assert_eq!(
        (
            code_max_stack(&runner, "read"),
            code_max_locals(&runner, "read")
        ),
        (2, 2)
    );
    assert_eq!(
        (
            code_max_stack(&runner, "readOwned"),
            code_max_locals(&runner, "readOwned")
        ),
        (3, 2)
    );
    for method in [
        "fromFactory",
        "fromLocal",
        "createPair",
        "fromNull",
        "read",
        "chainedFactory",
        "readOwned",
    ] {
        assert!(
            decode_subset_instructions(emitted_method_code(&runner, method))
                .iter()
                .all(|(_, opcode)| *opcode != 0xc0),
            "Runner.{method} does not insert checkcast coercions"
        );
    }
    assert_invocation_sequence(
        &owned_holder,
        emitted_method_code(&owned_holder, "<init>"),
        &[
            (0xb7, "java/lang/Object", "<init>", "()V"),
            (0xb7, "Counter", "<init>", "(I)V"),
        ],
    );
    assert_field_reference_sequence(
        &pair,
        emitted_method_code(&pair, "<init>"),
        &[
            (0xb5, "Pair", "savedFirst", "LCounter;"),
            (0xb5, "Pair", "savedSecond", "LCounter;"),
        ],
    );
    assert_field_reference_sequence(
        &runner,
        emitted_method_code(&runner, "makeCounter"),
        &[
            (0xb4, "Runner", "receiverCalls", "I"),
            (0xb5, "Runner", "receiverCalls", "I"),
        ],
    );
    assert_field_reference(
        &owned_holder,
        emitted_method_code(&owned_holder, "<init>"),
        0xb5,
        "OwnedHolder",
        "inner",
        "LCounter;",
    );
    assert_eq!(code_max_stack(&owned_holder, "<init>"), 4);
    assert_field_reference(
        &counter,
        emitted_method_code(&counter, "increment"),
        0xb4,
        "Counter",
        "current",
        "I",
    );
    assert_field_reference(
        &counter,
        emitted_method_code(&counter, "increment"),
        0xb5,
        "Counter",
        "current",
        "I",
    );

    if require_jdk_or_skip() {
        let classpath = output.path().to_str().expect("UTF-8 class output path");
        let harness = output.path().join("ObjectGraphConstructionHarness.java");
        fs::write(
            &harness,
            r#"public final class ObjectGraphConstructionHarness {
  public static void main(String[] args) {
    Runner runner = new Runner();
    Counter shared = new Counter(10);
    Holder alias = runner.existing(shared);
    if (alias.value() != shared) throw new AssertionError("reference alias preserved");
    if (alias.value().increment() != 11 || shared.get() != 11) throw new AssertionError("alias shares state");
    Holder independent = runner.fromLocal(8);
    if (independent.value().get() != 8) throw new AssertionError("mutable local passed to constructor");
    if (independent.value() == shared) throw new AssertionError("unrelated allocation is distinct");

    int expectedCalls = runner.receiverCallCount();
    Holder fromFactory = runner.fromFactory(7);
    if (fromFactory.value().get() != 7) throw new AssertionError("factory constructor argument");
    if (runner.receiverCallCount() != ++expectedCalls) throw new AssertionError("factory evaluated once");

    if (runner.fromNull().value() != null) throw new AssertionError("null identity preserved");

    Pair pair = runner.createPair(3, 4);
    Counter first = pair.firstCounter();
    Counter second = pair.secondCounter();
    if (first == second || pair.firstValue() != 3 || pair.secondValue() != 4) throw new AssertionError("sibling allocations");
    if (first.increment() != 4 || pair.secondValue() != 4) throw new AssertionError("sibling state is independent");

    OwnedHolder ownedA = new OwnedHolder(5);
    OwnedHolder ownedB = new OwnedHolder(5);
    if (ownedA.value() == ownedB.value()) throw new AssertionError("field initializer allocates per owner");
    if (ownedA.value().increment() != 6 || ownedB.get() != 5) throw new AssertionError("owned child state is independent");

    for (int seed : new int[] {10, 0, -3}) {
      if (runner.read(seed) != seed) throw new AssertionError("create/value/get result");
      if (runner.receiverCallCount() != ++expectedCalls) throw new AssertionError("create evaluated once");
      if (runner.chainedFactory(seed) != seed) throw new AssertionError("factory/value/get result");
      if (runner.receiverCallCount() != ++expectedCalls) throw new AssertionError("factory evaluated once");
      if (runner.readOwned(seed) != seed) throw new AssertionError("new OwnedHolder/get result");
    }
    System.out.println("object graph construction verified");
  }
}
"#,
        )
        .expect("write object graph runtime harness");
        let javac = Command::new("javac")
            .args(["--release", "21", "-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac for chained receiver harness");
        assert!(
            javac.status.success(),
            "javac rejected object graph harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "ObjectGraphConstructionHarness",
            ])
            .output()
            .expect("start java verifier for chained receiver harness");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected object graph classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );

        let javap = Command::new("javap")
            .args([
                "-c",
                "-v",
                "-classpath",
                classpath,
                "Runner",
                "Pair",
                "OwnedHolder",
            ])
            .output()
            .expect("start javap for emitted object graph classes");
        assert!(
            javap.status.success(),
            "javap rejected emitted object graph classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javap.stdout),
            String::from_utf8_lossy(&javap.stderr)
        );
        let javap = String::from_utf8(javap.stdout).expect("javap output is UTF-8");
        for reference in [
            "Pair.\"<init>\":(LCounter;LCounter;)V",
            "Counter.\"<init>\":(I)V",
            "Field receiverCalls:I",
            "Field savedFirst:LCounter;",
            "Field savedSecond:LCounter;",
            "Field inner:LCounter;",
        ] {
            assert!(
                javap.contains(reference),
                "javap output is missing {reference}"
            );
        }
    }
}

#[test]
fn object_graph_unsupported_deep_allocation_publishes_no_fixture_classes() {
    let mut source = fs::read_to_string(fixture_path("ObjectGraphConstruction.scala"))
        .expect("read the source-backed object graph fixture");
    source.push_str(
        "\nclass Leaf(seed: Int)\nclass Middle(leaf: Leaf)\nclass Outer(middle: Middle)\nclass TooDeep:\n  def make(): Outer = new Outer(new Middle(new Leaf(1)))\n",
    );
    let output = TestOutputDirectory::new("object-graph-deep-batch-failure");
    let result = compile_source_batch(
        &source,
        "ObjectGraphConstructionWithUnsupportedDepth.scala",
        output.path(),
        SourceId::from_index(1501),
        SourceUnitCardinality::AtLeastTwo,
    );
    assert!(
        matches!(result, Err(PipelineFailure::Codegen(_))),
        "unsupported third live allocation must fail during codegen, got {result:?}"
    );
    assert!(
        output.class_files().is_empty(),
        "a later unsupported allocation must not publish any fixture class"
    );
}

#[test]
fn inherited_accessors_emit_without_copying_parent_storage_and_verify_on_jvm() {
    let source = r#"
class Parent:
  val id: Int = 1
  var current: Int = 2

class Child extends Parent
class Leaf extends Child

class TokenParent(val token: Int)
class TokenChild extends TokenParent(7)
"#;
    let output = TestOutputDirectory::new("inherited-accessors-emission");
    let second_output = TestOutputDirectory::new("inherited-accessors-emission-repeat");
    let mut observed_metadata = None;
    let artifacts = compile_source_batch_with_source_observer(
        source,
        "InheritedAccessorEmission.scala",
        output.path(),
        SourceId::from_index(1611),
        SourceUnitCardinality::AtLeastTwo,
        &mut |_, _, unit, definitions| {
            observed_metadata = Some(
                unit.metadata(definitions)
                    .expect("validated hierarchy metadata retains effective accessors"),
            );
        },
    )
    .expect("safe accessor-bearing hierarchies emit as one complete batch");
    let second_artifacts = compile_source_batch(
        source,
        "InheritedAccessorEmission.scala",
        second_output.path(),
        SourceId::from_index(1611),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect("an independent frontend emits the same bounded hierarchy");
    assert_eq!(artifacts.len(), 5);
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| (artifact.class.internal_name(), artifact.class.bytes()))
            .collect::<Vec<_>>(),
        second_artifacts
            .iter()
            .map(|artifact| (artifact.class.internal_name(), artifact.class.bytes()))
            .collect::<Vec<_>>(),
        "two fresh source sessions emit byte-identical classes in source order"
    );

    let metadata = observed_metadata.expect("source observer captures unit metadata");
    let class_metadata = |name: &str| {
        metadata
            .iter()
            .find(|class| class.identity().internal_name() == name)
            .unwrap_or_else(|| panic!("missing metadata for {name}"))
    };
    for (receiver_name, accessor_owner, field_name) in [
        ("Child", "Parent", "id"),
        ("Leaf", "Parent", "id"),
        ("Child", "Parent", "current"),
        ("Leaf", "Parent", "current"),
        ("TokenChild", "TokenParent", "token"),
    ] {
        let receiver = class_metadata(receiver_name);
        let owner = class_metadata(accessor_owner);
        let field = owner
            .accessors()
            .iter()
            .find(|accessor| accessor.field_name() == field_name)
            .expect("only the declaring class owns the accessor metadata");
        let inherited = receiver
            .dispatch_accessors()
            .iter()
            .find(|accessor| {
                accessor.field() == field.source_field()
                    && accessor.role() == dotty_jvm::JvmAccessorDispatchRole::Getter
            })
            .expect("effective receiver metadata retains the inherited getter");
        assert_eq!(inherited.receiver_class(), receiver.identity().symbol());
        assert_eq!(inherited.declaring_class(), owner.identity().symbol());
        assert_eq!(inherited.name(), field_name);
        assert!(receiver.accessors().is_empty());
        assert!(receiver.fields().is_empty());
        assert!(receiver.methods().is_empty());
    }

    let parent = decode_emitted_class_named(&artifacts, "Parent");
    let child = decode_emitted_class_named(&artifacts, "Child");
    let leaf = decode_emitted_class_named(&artifacts, "Leaf");
    let token_parent = decode_emitted_class_named(&artifacts, "TokenParent");
    let token_child = decode_emitted_class_named(&artifacts, "TokenChild");
    let parent_name = |class: &ClassFile<'_>| {
        class
            .super_class
            .and_then(|index| class.constant_pool.class_name(index).ok())
            .map(str::to_owned)
    };
    assert_eq!(parent_name(&parent).as_deref(), Some("java/lang/Object"));
    assert_eq!(parent_name(&child).as_deref(), Some("Parent"));
    assert_eq!(parent_name(&leaf).as_deref(), Some("Child"));
    assert_eq!(
        parent_name(&token_parent).as_deref(),
        Some("java/lang/Object")
    );
    assert_eq!(parent_name(&token_child).as_deref(), Some("TokenParent"));

    assert_eq!(
        parent
            .fields
            .iter()
            .map(|field| parent.constant_pool.utf8(field.name_index).unwrap())
            .collect::<Vec<_>>(),
        ["id", "current"]
    );
    assert_eq!(child.fields.len(), 0);
    assert_eq!(leaf.fields.len(), 0);
    assert_eq!(token_parent.fields.len(), 1);
    assert_eq!(token_child.fields.len(), 0);
    assert_eq!(
        methods_with_descriptors(&child),
        [("<init>".to_owned(), "()V".to_owned())]
    );
    assert_eq!(
        methods_with_descriptors(&leaf),
        [("<init>".to_owned(), "()V".to_owned())]
    );
    assert_invocation_sequence(
        &child,
        emitted_method_code(&child, "<init>"),
        &[(0xb7, "Parent", "<init>", "()V")],
    );
    assert_invocation_sequence(
        &leaf,
        emitted_method_code(&leaf, "<init>"),
        &[(0xb7, "Child", "<init>", "()V")],
    );
    assert_invocation_sequence(
        &token_child,
        emitted_method_code(&token_child, "<init>"),
        &[(0xb7, "TokenParent", "<init>", "(I)V")],
    );

    if !require_jdk_or_skip() {
        return;
    }
    let harness = output.path().join("InheritedAccessorHarness.java");
    fs::write(
        &harness,
        r#"public final class InheritedAccessorHarness {
    public static void main(String[] args) {
        Leaf leaf = new Leaf();
        if (leaf.id() != 1 || leaf.current() != 2) throw new AssertionError("body accessors");
        leaf.current_$eq(9);
        if (leaf.current() != 9) throw new AssertionError("inherited setter");
        if (new TokenChild().token() != 7) throw new AssertionError("constructor accessor");
    }
}
"#,
    )
    .unwrap();
    let classpath = output.path().to_string_lossy().into_owned();
    let javac = Command::new("javac")
        .args(["-classpath", &classpath, "-d", &classpath])
        .arg(&harness)
        .output()
        .expect("start javac for inherited accessor harness");
    assert!(
        javac.status.success(),
        "javac rejected the inherited accessor harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            &classpath,
            "InheritedAccessorHarness",
        ])
        .output()
        .expect("start java for inherited accessor harness");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected inherited accessor classes\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
}

#[test]
fn inherited_getter_selections_use_the_static_receiver_owner_and_verify_on_jvm() {
    let source = r#"
class Parent:
  var current: Int = 1
  val id: Long = 42L

class Child extends Parent:
  def own(): Int = current
  def qualified(): Int = this.current
  def readId(): Long = id

class Reader:
  def sourceChild(): Child = new Child()
  def read(child: Child): Int = child.current
  def readId(child: Child): Long = child.id
  def readAsParent(parent: Parent): Int = parent.current
  def readFresh(): Int = sourceChild().current
"#;
    let output = TestOutputDirectory::new("inherited-getter-selection-lowering");
    let artifacts = compile_source_batch(
        source,
        "InheritedGetterSelections.scala",
        output.path(),
        SourceId::from_index(1612),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect("typed inherited field selections lower through validated getters");

    let child = decode_emitted_class_named(&artifacts, "Child");
    let reader = decode_emitted_class_named(&artifacts, "Reader");
    assert!(
        child.fields.is_empty(),
        "Child must not copy Parent storage"
    );
    assert!(
        methods_with_descriptors(&child)
            .iter()
            .all(|(name, _)| !["current", "id"].contains(&name.as_str())),
        "Child must not declare generated getter methods"
    );
    for (class, method, expected) in [
        (&child, "own", vec![(0xb6, "Child", "current", "()I")]),
        (&child, "qualified", vec![(0xb6, "Child", "current", "()I")]),
        (&child, "readId", vec![(0xb6, "Child", "id", "()J")]),
        (&reader, "read", vec![(0xb6, "Child", "current", "()I")]),
        (&reader, "readId", vec![(0xb6, "Child", "id", "()J")]),
        (
            &reader,
            "readAsParent",
            vec![(0xb6, "Parent", "current", "()I")],
        ),
        (
            &reader,
            "readFresh",
            vec![
                (0xb6, "Reader", "sourceChild", "()LChild;"),
                (0xb6, "Child", "current", "()I"),
            ],
        ),
    ] {
        let code = emitted_method_code(class, method);
        assert!(
            decode_subset_instructions(code)
                .iter()
                .all(|(_, opcode)| *opcode != 0xb4),
            "{method} must call the inherited getter instead of reading backing storage"
        );
        assert_invocation_sequence(class, code, &expected);
    }

    if !require_jdk_or_skip() {
        return;
    }
    let harness = output.path().join("InheritedGetterSelectionHarness.java");
    fs::write(
        &harness,
        r#"public final class InheritedGetterSelectionHarness {
    public static void main(String[] args) {
        Child child = new Child();
        Reader reader = new Reader();
        if (child.own() != 1 || child.qualified() != 1 || child.readId() != 42L)
            throw new AssertionError("inherited reads in Child");
        if (reader.read(child) != 1 || reader.readId(child) != 42L)
            throw new AssertionError("Child receiver reads");
        if (reader.readFresh() != 1)
            throw new AssertionError("single evaluation of call receiver");
        Parent parent = child;
        if (reader.readAsParent(parent) != 1)
            throw new AssertionError("Parent receiver read");
        child.current_$eq(9);
        if (reader.read(child) != 9)
            throw new AssertionError("inherited setter remains dispatched");
    }
}
"#,
    )
    .unwrap();
    let classpath = output.path().to_string_lossy().into_owned();
    let javac = Command::new("javac")
        .args(["-classpath", &classpath, "-d", &classpath])
        .arg(&harness)
        .output()
        .expect("start javac for inherited getter selection harness");
    assert!(
        javac.status.success(),
        "javac rejected inherited getter selection harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            &classpath,
            "InheritedGetterSelectionHarness",
        ])
        .output()
        .expect("start java for inherited getter selection harness");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected inherited getter selections\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
}

#[test]
fn inherited_mutable_assignments_lower_through_validated_setters() {
    let source = r#"
class Parent:
  var current: Long = 1L
  var enabled: Boolean = true

class Child extends Parent:
  def update(value: Long): Unit = current = value
  def qualifiedUpdate(value: Long): Unit = this.current = value

class Reader:
  var receiverEvaluations: Int = 0
  var rhsEvaluations: Int = 0
  var evaluationOrder: Int = 0
  var receiverOrder: Int = 0
  var rhsOrder: Int = 0
  def nextChild(): Child =
    receiverEvaluations = receiverEvaluations + 1
    evaluationOrder = evaluationOrder + 1
    receiverOrder = evaluationOrder
    new Child()
  def evaluate(value: Long): Long =
    rhsEvaluations = rhsEvaluations + 1
    evaluationOrder = evaluationOrder + 1
    rhsOrder = evaluationOrder
    value
  def updateChild(child: Child, value: Long): Unit = child.current = value
  def setFlag(child: Child): Unit = child.enabled = false
  def updateFresh(value: Long): Unit = nextChild().current = evaluate(value)
"#;
    let output = TestOutputDirectory::new("inherited-mutable-assignment-lowering");
    let artifacts = compile_source_batch(
        source,
        "InheritedMutableAssignments.scala",
        output.path(),
        SourceId::from_index(1613),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect("typed inherited mutable fields lower through their validated setters");

    let parent = decode_emitted_class_named(&artifacts, "Parent");
    let child = decode_emitted_class_named(&artifacts, "Child");
    let reader = decode_emitted_class_named(&artifacts, "Reader");
    assert_eq!(child.fields.len(), 0, "Child must not copy Parent storage");
    assert!(
        methods_with_descriptors(&child).iter().all(|(name, _)| ![
            "current",
            "enabled",
            "current_$eq",
            "enabled_$eq"
        ]
        .contains(&name.as_str())),
        "Child must not copy generated Parent accessors"
    );
    assert_eq!(parent.fields.len(), 2);

    for (class, method) in [(&child, "update"), (&child, "qualifiedUpdate")] {
        let code = emitted_method_code(class, method);
        assert_invocation_sequence(class, code, &[(0xb6, "Child", "current_$eq", "(J)V")]);
        assert!(
            decode_subset_instructions(code)
                .iter()
                .all(|(_, opcode)| ![0xb4, 0xb5].contains(opcode)),
            "{method} must dispatch through the inherited setter, not Parent storage"
        );
    }
    for (method, expected) in [
        ("updateChild", vec![(0xb6, "Child", "current_$eq", "(J)V")]),
        ("setFlag", vec![(0xb6, "Child", "enabled_$eq", "(Z)V")]),
        (
            "updateFresh",
            vec![
                (0xb6, "Reader", "nextChild", "()LChild;"),
                (0xb6, "Reader", "evaluate", "(J)J"),
                (0xb6, "Child", "current_$eq", "(J)V"),
            ],
        ),
    ] {
        let code = emitted_method_code(&reader, method);
        assert_invocation_sequence(&reader, code, &expected);
        assert!(
            decode_subset_instructions(code)
                .iter()
                .all(|(_, opcode)| ![0xb4, 0xb5].contains(opcode)),
            "{method} must not access inherited private backing storage"
        );
    }

    if !require_jdk_or_skip() {
        return;
    }
    let harness = output.path().join("InheritedMutableAssignmentHarness.java");
    fs::write(
        &harness,
        r#"public final class InheritedMutableAssignmentHarness {
    public static void main(String[] args) {
        Child child = new Child();
        child.update(5L);
        if (child.current() != 5L) throw new AssertionError("unqualified Child setter");
        child.qualifiedUpdate(7L);
        if (child.current() != 7L) throw new AssertionError("qualified Child setter");
        Reader reader = new Reader();
        reader.updateChild(child, 11L);
        if (child.current() != 11L) throw new AssertionError("Reader Child setter");
        reader.setFlag(child);
        if (child.enabled()) throw new AssertionError("Boolean setter");
        reader.updateFresh(13L);
        if (reader.receiverEvaluations() != 1 || reader.rhsEvaluations() != 1
                || reader.receiverOrder() != 1
                || reader.rhsOrder() != 2)
            throw new AssertionError("receiver must evaluate once before RHS");
    }
}
"#,
    )
    .unwrap();
    let classpath = output.path().to_string_lossy().into_owned();
    let javac = Command::new("javac")
        .args(["-classpath", &classpath, "-d", &classpath])
        .arg(&harness)
        .output()
        .expect("start javac for inherited mutable assignment harness");
    assert!(
        javac.status.success(),
        "javac rejected inherited mutable assignment harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            &classpath,
            "InheritedMutableAssignmentHarness",
        ])
        .output()
        .expect("start java for inherited mutable assignment harness");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected inherited mutable assignments\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
}

#[test]
fn full_inherited_accessor_hierarchy_verifies_end_to_end_on_jvm() {
    let source = fs::read_to_string(fixture_path("InheritedAccessorHierarchy.scala"))
        .expect("read the Scala 3.9.0 inherited accessor hierarchy fixture");
    let output = TestOutputDirectory::new("inherited-accessor-hierarchy-e2e");
    let second_output = TestOutputDirectory::new("inherited-accessor-hierarchy-e2e-repeat");
    let mut observed_metadata = None;
    let artifacts = compile_source_batch_with_source_observer(
        &source,
        "InheritedAccessorHierarchy.scala",
        output.path(),
        SourceId::from_index(1615),
        SourceUnitCardinality::AtLeastTwo,
        &mut |_, _, unit, definitions| {
            let base_class = audit_class_named(unit, "Base");
            let middle_class = audit_class_named(unit, "Middle");
            let leaf_class = audit_class_named(unit, "Leaf");
            let reader_class = audit_class_named(unit, "Reader");
            assert_eq!(
                audit_class_parent(unit, base_class),
                definitions.object_class
            );
            assert_eq!(audit_class_parent(unit, middle_class), base_class);
            assert_eq!(audit_class_parent(unit, leaf_class), middle_class);

            let metadata = unit
                .metadata(definitions)
                .expect("metadata retains effective accessor views across two edges");
            let base = metadata
                .iter()
                .find(|class| class.identity().symbol() == base_class)
                .expect("Base metadata exists");
            let id = base
                .accessors()
                .iter()
                .find(|accessor| accessor.field_name() == "id")
                .expect("Base.id is a canonical constructor accessor")
                .source_field();
            let current = base
                .accessors()
                .iter()
                .find(|accessor| accessor.field_name() == "current")
                .expect("Base.current is a canonical body accessor")
                .source_field();
            let enabled = base
                .accessors()
                .iter()
                .find(|accessor| accessor.field_name() == "enabled")
                .expect("Base.enabled is a canonical body val")
                .source_field();
            for (class, method_name, field, expected_references) in [
                (middle_class, "read", current, 1),
                (middle_class, "write", current, 1),
                (reader_class, "fromBase", current, 1),
                (reader_class, "fromMiddle", current, 1),
                (reader_class, "fromLeaf", current, 1),
                (reader_class, "change", current, 1),
                (base_class, "doubled", id, 2),
                (reader_class, "id", id, 1),
                (reader_class, "isEnabled", enabled, 1),
            ] {
                let method = audit_method_named(unit, class, method_name);
                let body = method.body.expect("source method retains its typed body");
                let mut targets = Vec::new();
                walk_typed_subtree(unit, body, &mut |tree_id| {
                    if term_type_target(unit, unit.typed_ast.get(tree_id).ty) == Some(field) {
                        targets.push(tree_id);
                    }
                });
                assert_eq!(
                    targets.len(),
                    expected_references,
                    "{method_name} retains the canonical Base Field identity"
                );
                assert!(
                    targets.iter().all(|tree_id| matches!(
                        unit.typed_ast.get(*tree_id).kind,
                        TreeKind::Ident(_) | TreeKind::Select(_)
                    )),
                    "{method_name} retains genuine typed identifier/selection nodes"
                );
            }
            let change = audit_method_named(unit, reader_class, "change");
            let change_body = change.body.expect("change retains its typed body");
            let mut assignments = Vec::new();
            walk_typed_subtree(unit, change_body, &mut |tree_id| {
                let tree = unit.typed_ast.get(tree_id);
                if let TreeKind::Assign(assignment) = tree.kind
                    && term_type_target(unit, unit.typed_ast.get(assignment.lhs).ty)
                        == Some(current)
                {
                    assignments.push((tree_id, assignment));
                }
            });
            assert_eq!(assignments.len(), 1);
            assert!(
                matches!(
                    unit.typed_ast.get(assignments[0].1.lhs).kind,
                    TreeKind::Select(_)
                ),
                "Reader.change retains a typed selected field on the Leaf receiver"
            );
            assert_eq!(
                unit.typed_ast.get(assignments[0].0).ty,
                definitions.unit,
                "source setter assignment retains its Unit result"
            );
            for (class, parent) in [(middle_class, base_class), (leaf_class, middle_class)] {
                let plan = unit
                    .classes
                    .iter()
                    .find(|plan| plan.class == class)
                    .unwrap();
                let JvmClassInitializationPlan::PrimaryConstructor(constructor) =
                    &plan.initialization
                else {
                    panic!("parameterized source constructor remains in the unit plan");
                };
                assert_eq!(constructor.parameters().len(), 1);
                let formal = constructor.parameters()[0];
                assert_eq!(
                    unit.store.symbols.get(formal).owner,
                    Some(constructor.symbol())
                );
                let super_call = constructor
                    .direct_super_constructor()
                    .expect("constructor forwards to its exact direct parent");
                assert_eq!(super_call.parent_class(), parent);
                assert_eq!(
                    unit.store
                        .symbols
                        .get(super_call.parent_constructor())
                        .owner,
                    Some(parent)
                );
            }
            observed_metadata = Some(metadata);
        },
    )
    .expect("real source accessors compose across Base-Middle-Leaf");
    let second = compile_source_batch(
        &source,
        "InheritedAccessorHierarchy.scala",
        second_output.path(),
        SourceId::from_index(1615),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect("fresh source session emits the same full class batch");
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| (artifact.class.internal_name(), artifact.class.bytes()))
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|artifact| (artifact.class.internal_name(), artifact.class.bytes()))
            .collect::<Vec<_>>(),
        "independent sessions emit byte-identical class batches"
    );
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Base", "Middle", "Leaf", "Reader"]
    );

    let metadata = observed_metadata.expect("observer captures JVM metadata");
    let class_metadata = |name: &str| {
        metadata
            .iter()
            .find(|class| class.identity().internal_name() == name)
            .unwrap_or_else(|| panic!("missing metadata for {name}"))
    };
    let base = class_metadata("Base");
    for receiver_name in ["Middle", "Leaf"] {
        let receiver = class_metadata(receiver_name);
        assert!(receiver.accessors().is_empty());
        assert!(receiver.fields().is_empty());
        for (field_name, role) in [
            ("id", dotty_jvm::JvmAccessorDispatchRole::Getter),
            ("current", dotty_jvm::JvmAccessorDispatchRole::Getter),
            ("current", dotty_jvm::JvmAccessorDispatchRole::Setter),
            ("enabled", dotty_jvm::JvmAccessorDispatchRole::Getter),
        ] {
            let origin = base
                .accessors()
                .iter()
                .find(|accessor| accessor.field_name() == field_name)
                .expect("only Base declares accessor storage")
                .source_field();
            let inherited = receiver
                .dispatch_accessors()
                .iter()
                .find(|entry| entry.field() == origin && entry.role() == role)
                .unwrap_or_else(|| panic!("{receiver_name} inherits {field_name} {role:?}"));
            assert_eq!(inherited.receiver_class(), receiver.identity().symbol());
            assert_eq!(inherited.declaring_class(), base.identity().symbol());
        }
    }

    let base = decode_emitted_class_named(&artifacts, "Base");
    let middle = decode_emitted_class_named(&artifacts, "Middle");
    let leaf = decode_emitted_class_named(&artifacts, "Leaf");
    let reader = decode_emitted_class_named(&artifacts, "Reader");
    for class in [&base, &middle, &leaf, &reader] {
        assert_eq!(
            class.version.major, 65,
            "all emitted classes target Java 21"
        );
        assert!(class.access_flags.is_public());
    }
    assert_eq!(
        base.constant_pool
            .class_name(base.super_class.unwrap())
            .unwrap(),
        "java/lang/Object"
    );
    assert_eq!(
        middle
            .constant_pool
            .class_name(middle.super_class.unwrap())
            .unwrap(),
        "Base"
    );
    assert_eq!(
        leaf.constant_pool
            .class_name(leaf.super_class.unwrap())
            .unwrap(),
        "Middle"
    );
    assert_eq!(
        base.fields.len(),
        3,
        "Base alone owns id, current, and enabled storage"
    );
    let base_fields = base
        .fields
        .iter()
        .map(|field| {
            (
                base.constant_pool.utf8(field.name_index).unwrap(),
                base.constant_pool.utf8(field.descriptor_index).unwrap(),
                field.access_flags.is_private(),
                field.access_flags.is_final(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        base_fields,
        [
            ("id", "I", true, true),
            ("current", "J", true, false),
            ("enabled", "Z", true, true),
        ]
    );
    assert_eq!(
        methods_with_descriptors(&base),
        [
            ("<init>".to_owned(), "(I)V".to_owned()),
            ("id".to_owned(), "()I".to_owned()),
            ("current".to_owned(), "()J".to_owned()),
            ("current_$eq".to_owned(), "(J)V".to_owned()),
            ("enabled".to_owned(), "()Z".to_owned()),
            ("doubled".to_owned(), "()I".to_owned()),
        ]
    );
    for (name, descriptor) in [
        ("id", "()I"),
        ("current", "()J"),
        ("current_$eq", "(J)V"),
        ("enabled", "()Z"),
    ] {
        let method = base
            .methods
            .iter()
            .find(|method| {
                base.constant_pool.utf8(method.name_index) == Ok(name)
                    && base.constant_pool.utf8(method.descriptor_index) == Ok(descriptor)
            })
            .unwrap_or_else(|| panic!("Base declares {name}{descriptor}"));
        assert!(
            method.access_flags.is_public(),
            "{name}{descriptor} is public"
        );
    }
    assert!(middle.fields.is_empty());
    assert!(leaf.fields.is_empty());
    for child in [&middle, &leaf] {
        assert!(methods_with_descriptors(child).iter().all(|(name, _)| {
            !["id", "current", "current_$eq", "enabled", "doubled"].contains(&name.as_str())
        }));
    }
    assert_invocation_sequence(
        &middle,
        emitted_method_code(&middle, "<init>"),
        &[(0xb7, "Base", "<init>", "(I)V")],
    );
    assert_invocation_sequence(
        &leaf,
        emitted_method_code(&leaf, "<init>"),
        &[(0xb7, "Middle", "<init>", "(I)V")],
    );
    let base_constructor = emitted_method_code(&base, "<init>");
    assert_invocation_sequence(
        &base,
        base_constructor,
        &[(0xb7, "java/lang/Object", "<init>", "()V")],
    );
    let base_constructor_instructions = decode_subset_instructions(base_constructor);
    assert_eq!(
        base_constructor_instructions
            .iter()
            .filter(|(_, opcode)| *opcode == 0xb5)
            .count(),
        3,
        "Base initializes its id, current, and enabled backing fields exactly once"
    );
    assert_field_reference_sequence(
        &base,
        base_constructor,
        &[
            (0xb5, "Base", "id", "I"),
            (0xb5, "Base", "current", "J"),
            (0xb5, "Base", "enabled", "Z"),
        ],
    );
    let object_init_pc = base_constructor_instructions
        .iter()
        .find(|(_, opcode)| *opcode == 0xb7)
        .map(|(pc, _)| *pc)
        .expect("Base calls Object.<init>");
    let first_field_store_pc = base_constructor_instructions
        .iter()
        .find(|(_, opcode)| *opcode == 0xb5)
        .map(|(pc, _)| *pc)
        .expect("Base stores constructor and body fields");
    assert!(
        object_init_pc < first_field_store_pc,
        "Base initializes Object before storing its own fields"
    );
    assert_field_reference_sequence(
        &base,
        emitted_method_code(&base, "current"),
        &[(0xb4, "Base", "current", "J")],
    );
    assert_field_reference_sequence(
        &base,
        emitted_method_code(&base, "current_$eq"),
        &[(0xb5, "Base", "current", "J")],
    );
    assert_opcode_sequence(emitted_method_code(&base, "current"), &[0x2a, 0xb4, 0xad]);
    assert_eq!(code_max_stack(&base, "current"), 2);
    assert_eq!(code_max_locals(&base, "current_$eq"), 3);
    assert_field_reference_sequence(
        &base,
        emitted_method_code(&base, "enabled"),
        &[(0xb4, "Base", "enabled", "Z")],
    );
    assert_invocation_sequence(
        &middle,
        emitted_method_code(&middle, "read"),
        &[(0xb6, "Middle", "current", "()J")],
    );
    assert_eq!(
        emitted_method_code(&middle, "read").last(),
        Some(&0xad),
        "the inherited Long getter returns with lreturn"
    );
    assert_invocation_sequence(
        &middle,
        emitted_method_code(&middle, "write"),
        &[(0xb6, "Middle", "current_$eq", "(J)V")],
    );
    for (method, owner) in [
        ("fromBase", "Base"),
        ("fromMiddle", "Middle"),
        ("fromLeaf", "Leaf"),
    ] {
        assert_invocation_sequence(
            &reader,
            emitted_method_code(&reader, method),
            &[(0xb6, owner, "current", "()J")],
        );
    }
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "change"),
        &[(0xb6, "Leaf", "current_$eq", "(J)V")],
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "id"),
        &[(0xb6, "Leaf", "id", "()I")],
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "isEnabled"),
        &[(0xb6, "Leaf", "enabled", "()Z")],
    );
    assert_invocation_sequence(
        &base,
        emitted_method_code(&base, "doubled"),
        &[(0xb6, "Base", "id", "()I"), (0xb6, "Base", "id", "()I")],
    );
    for (class, methods) in [
        (&middle, ["read", "write"].as_slice()),
        (
            &reader,
            [
                "fromBase",
                "fromMiddle",
                "fromLeaf",
                "change",
                "id",
                "isEnabled",
            ]
            .as_slice(),
        ),
    ] {
        for method in methods {
            assert_no_field_reference_to_owner(class, emitted_method_code(class, method), "Base");
        }
    }

    if !require_jdk_or_skip() {
        return;
    }
    let classpath = output.path().to_string_lossy().into_owned();
    let javap = Command::new("javap")
        .args([
            "-c",
            "-v",
            "-p",
            "-classpath",
            &classpath,
            "Base",
            "Middle",
            "Leaf",
            "Reader",
        ])
        .output()
        .expect("start javap for multilevel inherited accessors");
    assert!(
        javap.status.success(),
        "javap rejected multilevel inherited accessors\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javap.stdout),
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap_output = String::from_utf8_lossy(&javap.stdout);
    for expected in [
        "Base.\"<init>\":(I)V",
        "Middle.\"<init>\":(I)V",
        "Middle.current:()J",
        "Middle.current_$eq:(J)V",
        "Leaf.current:()J",
        "Leaf.current_$eq:(J)V",
        "major version: 65",
        "private final int id",
        "private long current",
        "private final boolean enabled",
    ] {
        assert!(javap_output.contains(expected), "javap omitted {expected}");
    }
    let harness = output
        .path()
        .join("MultilevelInheritedAccessorsHarness.java");
    fs::write(
        &harness,
        r#"public final class MultilevelInheritedAccessorsHarness {
    public static void main(String[] args) {
        Reader reader = new Reader();
        Base base = new Base(3);
        Leaf first = new Leaf(7);
        Leaf second = new Leaf(11);
        if (first.id() != 7 || second.id() != 11 || reader.id(first) != 7) throw new AssertionError("inherited id");
        if (!first.enabled() || !second.enabled() || first.doubled() != 14 || second.doubled() != 22) throw new AssertionError("body val and ordinary inherited method");
        if (reader.fromBase(base) != 0L || reader.fromBase(first) != 0L || reader.fromMiddle(first) != 0L || reader.fromLeaf(first) != 0L || new Middle(9).read() != 0L) throw new AssertionError("inherited getter");
        Middle middle = new Middle(9);
        middle.write(Long.MIN_VALUE);
        if (middle.read() != Long.MIN_VALUE) throw new AssertionError("intermediate setter");
        reader.change(first, Long.MAX_VALUE);
        second.current_$eq(Long.MIN_VALUE);
        if (reader.fromBase(first) != Long.MAX_VALUE || reader.fromMiddle(first) != Long.MAX_VALUE || reader.fromLeaf(first) != Long.MAX_VALUE) throw new AssertionError("typed inherited setter dispatch");
        if (reader.fromLeaf(second) != Long.MIN_VALUE || reader.fromBase(base) != 0L) throw new AssertionError("independent instances");
        System.out.println("multilevel inherited accessors verified");
    }
}
"#,
    )
    .expect("write multilevel inherited accessor harness");
    let javac = Command::new("javac")
        .args([
            "--release",
            "21",
            "-classpath",
            &classpath,
            "-d",
            &classpath,
        ])
        .arg(&harness)
        .output()
        .expect("start javac for multilevel inherited accessors");
    assert!(
        javac.status.success(),
        "javac rejected multilevel inherited accessors\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            &classpath,
            "MultilevelInheritedAccessorsHarness",
        ])
        .output()
        .expect("start java verifier for multilevel inherited accessors");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected multilevel inherited accessors\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
}

#[test]
fn inherited_accessor_hierarchy_rejects_new_child_storage_without_partial_output() {
    let output = TestOutputDirectory::new("inherited-accessor-child-storage-rejected");
    let source =
        "class Parent { val id: Int = 1 }\nclass Child extends Parent { val extra: Int = 2 }\n";
    let mut expected_edge = None;
    let result = compile_source_batch_observed(
        source,
        "InheritedAccessorChildStorage.scala",
        output.path(),
        SourceId::from_index(1612),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut |unit, _| {
            expected_edge = Some((
                audit_class_named(unit, "Child"),
                audit_class_named(unit, "Parent"),
            ));
        }),
    );
    assert!(
        matches!(
            result,
            Err(PipelineFailure::Codegen(
                JvmCodegenError::UnsupportedJvmAccessorInheritance { class, parent }
            )) if Some((class, parent)) == expected_edge
        ),
        "child-owned accessors remain outside the safe inherited subset: {result:?}"
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn effective_accessor_dispatch_projects_constructor_body_and_long_accessors_read_only() {
    let output = TestOutputDirectory::new("effective-inherited-accessor-metadata");
    let source = "class Parent(var current: Int)\nclass Child extends Parent(1)\nclass Leaf extends Child\nclass BodyParent { var amount: Int = 2 }\nclass BodyChild extends BodyParent\nclass LongParent { val id: Long = 42L }\nclass LongChild extends LongParent\nclass Unrelated(var current: Int)\n";
    let mut observed_metadata = None;
    let artifacts = compile_source_batch_with_source_observer(
        source,
        "EffectiveAccessorDispatch.scala",
        output.path(),
        SourceId::from_index(1609),
        SourceUnitCardinality::AtLeastTwo,
        &mut |_, _, unit, definitions| {
            observed_metadata = Some(
                unit.metadata(definitions)
                    .expect("read-only metadata projects the bounded accessor hierarchy"),
            );
        },
    )
    .expect("safe inherited-accessor hierarchies emit after metadata validation");
    assert_eq!(artifacts.len(), 8);

    let metadata = observed_metadata.expect("source observer captures metadata");
    let class = |name: &str| {
        metadata
            .iter()
            .find(|class| class.identity().internal_name() == name)
            .unwrap_or_else(|| panic!("{name} metadata"))
    };
    let parent = class("Parent");
    let child = class("Child");
    let leaf = class("Leaf");
    let parent_field = parent.accessors()[0].source_field();
    assert_eq!(parent.accessors()[0].getter().descriptor().as_str(), "()I");
    assert_eq!(
        parent.accessors()[0]
            .setter()
            .expect("var has a setter")
            .descriptor()
            .as_str(),
        "(I)V"
    );
    assert!(child.accessors().is_empty());
    assert!(leaf.accessors().is_empty());
    for (receiver, expected_class) in [(child, "Child"), (leaf, "Leaf")] {
        let current = receiver
            .dispatch_accessors()
            .iter()
            .filter(|entry| entry.field() == parent_field)
            .collect::<Vec<_>>();
        assert_eq!(current.len(), 2, "getter and setter remain distinct");
        assert!(current.iter().any(|entry| {
            entry.role() == dotty_jvm::JvmAccessorDispatchRole::Getter
                && entry.name() == "current"
                && entry.descriptor().as_str() == "()I"
        }));
        assert!(current.iter().any(|entry| {
            entry.role() == dotty_jvm::JvmAccessorDispatchRole::Setter
                && entry.name() == "current_$eq"
                && entry.descriptor().as_str() == "(I)V"
        }));
        assert!(current.iter().all(|entry| {
            entry.receiver_class() == receiver.identity().symbol()
                && entry.declaring_class() == parent.identity().symbol()
        }));
        assert_eq!(expected_class, receiver.identity().internal_name());
    }

    let body_parent = class("BodyParent");
    let body_child = class("BodyChild");
    let body_field = body_parent.accessors()[0].source_field();
    let body_getter = body_child
        .dispatch_accessors()
        .iter()
        .find(|entry| entry.field() == body_field)
        .expect("body-field getter is inherited in the metadata view");
    assert_eq!(body_getter.name(), "amount");
    assert_eq!(body_getter.descriptor().as_str(), "()I");
    assert_eq!(
        body_getter.role(),
        dotty_jvm::JvmAccessorDispatchRole::Getter
    );

    let long_parent = class("LongParent");
    let long_child = class("LongChild");
    let long_id = long_parent.accessors()[0].source_field();
    let long_getter = long_child
        .dispatch_accessors()
        .iter()
        .find(|entry| entry.field() == long_id)
        .expect("val getter is inherited");
    assert_eq!(long_getter.name(), "id");
    assert_eq!(long_getter.descriptor().as_str(), "()J");
    assert_eq!(
        long_getter.role(),
        dotty_jvm::JvmAccessorDispatchRole::Getter
    );
    assert_eq!(
        long_child
            .dispatch_accessors()
            .iter()
            .filter(|entry| entry.field() == long_id)
            .count(),
        1,
        "immutable val does not acquire a setter"
    );

    let unrelated = class("Unrelated");
    let unrelated_field = unrelated.accessors()[0].source_field();
    assert_ne!(unrelated_field, parent_field);
    assert!(
        child
            .dispatch_accessors()
            .iter()
            .all(|entry| entry.field() != unrelated_field)
    );
    for name in ["Child", "Leaf", "BodyChild", "LongChild"] {
        let emitted = decode_emitted_class_named(&artifacts, name);
        assert!(
            emitted.fields.is_empty(),
            "{name} must not copy parent fields"
        );
    }
}

#[test]
fn object_graph_direct_new_and_completed_call_receivers_remain_supported() {
    let output = TestOutputDirectory::new("object-graph-supported-receivers");
    let artifacts = compile_planned_constructor_source(
        "class Counter(initial: Int):\n  private var current: Int = initial\n  def get(): Int = current\nclass Holder(counter: Counter):\n  private val inner: Counter = counter\n  def value(): Counter = inner\nclass Link(next: Link):\n  def wrapSelf(): Link = new Link(this)\nclass Runner:\n  def makeCounter(seed: Int): Counter = new Counter(seed)\n  def create(counter: Counter): Holder = new Holder(counter)\n  def fromImmutable(counter: Counter): Holder =\n    val current: Counter = counter\n    new Holder(current)\n  def directNew(seed: Int): Int = new Counter(seed).get()\n  def factoryReceiver(seed: Int): Int = makeCounter(seed).get()\n  def chained(counter: Counter): Int = create(counter).value().get()\n",
        output.path(),
    )
    .expect("already supported direct-new and completed call receivers stay accepted");
    let class = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Runner")
        .expect("Runner is emitted")
        .class
        .bytes();
    let mut reader = Reader::new(class);
    let runner = ClassFile::decode(&mut reader).expect("decode source-backed Runner");
    runner
        .validate_references()
        .expect("Runner constant-pool references validate");
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "directNew"),
        &[
            (0xb7, "Counter", "<init>", "(I)V"),
            (0xb6, "Counter", "get", "()I"),
        ],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "factoryReceiver"),
        &[
            (0xb6, "Runner", "makeCounter", "(I)LCounter;"),
            (0xb6, "Counter", "get", "()I"),
        ],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "chained"),
        &[
            (0xb6, "Runner", "create", "(LCounter;)LHolder;"),
            (0xb6, "Holder", "value", "()LCounter;"),
            (0xb6, "Counter", "get", "()I"),
        ],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "fromImmutable"),
        &[(0xb7, "Holder", "<init>", "(LCounter;)V")],
    );
    let link_bytes = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Link")
        .expect("Link is emitted")
        .class
        .bytes();
    let mut reader = Reader::new(link_bytes);
    let link = ClassFile::decode(&mut reader).expect("decode source-backed Link");
    assert_invocation_sequence(
        &link,
        emitted_method_code(&link, "wrapSelf"),
        &[(0xb7, "Link", "<init>", "(LLink;)V")],
    );

    if require_jdk_or_skip() {
        let classpath = output.path().to_str().expect("UTF-8 class output path");
        let harness = output.path().join("ObjectGraphReceiverHarness.java");
        fs::write(
            &harness,
            r#"public final class ObjectGraphReceiverHarness {
  public static void main(String[] args) {
    Runner runner = new Runner();
    Counter counter = new Counter(11);
    if (runner.directNew(12) != 12) throw new AssertionError("direct new receiver");
    if (runner.factoryReceiver(13) != 13) throw new AssertionError("factory receiver");
    if (runner.chained(counter) != 11) throw new AssertionError("chained call-result receivers");
    if (runner.fromImmutable(counter).value().get() != 11) throw new AssertionError("immutable local constructor argument");
    if (new Link(null).wrapSelf() == null) throw new AssertionError("this constructor argument");
  }
}

"#,
        )
        .expect("write direct-new and chained receiver harness");
        let javac = Command::new("javac")
            .args(["--release", "21", "-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac for receiver characterization");
        assert!(
            javac.status.success(),
            "javac rejected receiver harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "ObjectGraphReceiverHarness",
            ])
            .output()
            .expect("start JVM verifier for receiver characterization");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected receiver harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
    }
}

#[test]
fn reference_field_initializer_constructs_one_verified_child_after_super() {
    let source = "class Counter(seed: Int):\n  private val value: Int = seed\n  def get(): Int = value\nclass OwnedHolder(seed: Int):\n  private val inner: Counter = new Counter(seed)\n  def get(): Int = inner.get()\n  def innerCounter(): Counter = inner\nclass Client:\n  def make(seed: Int): OwnedHolder = new OwnedHolder(seed)\n";
    let output = TestOutputDirectory::new("reference-field-initialized-by-new");
    let artifacts = compile_source_classes(
        source,
        "ReferenceFieldInitializedByNew.scala",
        output.path(),
    )
    .expect("a private planned reference field accepts one checked new expression");
    let repeated_output = TestOutputDirectory::new("reference-field-initialized-by-new-repeat");
    let repeated = compile_source_classes(
        source,
        "ReferenceFieldInitializedByNew.scala",
        repeated_output.path(),
    )
    .expect("a fresh source session compiles the field initializer deterministically");
    assert_eq!(artifacts.len(), repeated.len());
    for (first, second) in artifacts.iter().zip(&repeated) {
        assert_eq!(first.class.internal_name(), second.class.internal_name());
        assert_eq!(first.class.bytes(), second.class.bytes());
    }

    let owned_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "OwnedHolder")
        .expect("OwnedHolder is emitted");
    let owned = decode_emitted_class(owned_artifact);
    let field = owned
        .fields
        .iter()
        .find(|field| owned.constant_pool.utf8(field.name_index) == Ok("inner"))
        .expect("inner reference field is emitted");
    assert_eq!(
        owned.constant_pool.utf8(field.descriptor_index),
        Ok("LCounter;")
    );
    assert!(field.access_flags.is_private());
    assert!(field.access_flags.is_final());
    let constructor = emitted_method_code(&owned, "<init>");
    assert_opcode_sequence(
        constructor,
        &[0x2a, 0xb7, 0x2a, 0xbb, 0x59, 0x1b, 0xb7, 0xb5, 0xb1],
    );
    assert_invocation_sequence(
        &owned,
        constructor,
        &[
            (0xb7, "java/lang/Object", "<init>", "()V"),
            (0xb7, "Counter", "<init>", "(I)V"),
        ],
    );
    assert_field_reference(
        &owned,
        constructor,
        0xb5,
        "OwnedHolder",
        "inner",
        "LCounter;",
    );
    assert_eq!(code_max_stack(&owned, "<init>"), 4);

    if require_jdk_or_skip() {
        let classpath = output.path().to_str().expect("UTF-8 output path");
        let harness = output
            .path()
            .join("ReferenceFieldInitializedByNewHarness.java");
        fs::write(
            &harness,
            "public final class ReferenceFieldInitializedByNewHarness { public static void main(String[] args) { Client client = new Client(); OwnedHolder positive = client.make(10); OwnedHolder zero = client.make(0); OwnedHolder negative = client.make(-3); OwnedHolder separate = client.make(10); if (positive.get() != 10 || zero.get() != 0 || negative.get() != -3) throw new AssertionError(\"field constructor values\"); if (positive.innerCounter() == separate.innerCounter()) throw new AssertionError(\"each field initializer creates one distinct Counter\"); if (positive.innerCounter().get() != 10 || separate.innerCounter().get() != 10) throw new AssertionError(\"field references retain their own initialized values\"); } }\n",
        )
        .expect("write field initializer runtime harness");
        let javac = Command::new("javac")
            .args(["-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected the field initializer harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "ReferenceFieldInitializedByNewHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected the field initializer classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
    }
}
fn audit_symbol_name(unit: &JvmCompilationUnit<'_>, symbol: SymbolId) -> String {
    let info = unit.store.symbols.get(symbol);
    let name = unit.store.names.resolve(info.name.text());
    let name = if info.kind == SymbolKind::Package && name.is_empty() {
        "<root>"
    } else {
        name
    };
    format!("{:?}:{name}", info.kind)
}

fn audit_prefix(unit: &JvmCompilationUnit<'_>, ty: dotty_core::TypeId) -> String {
    match unit.store.types.get(ty) {
        Type::NoPrefix => "NoPrefix".to_owned(),
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => format!("TypeRef({})", audit_symbol_name(unit, *symbol)),
        Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        } => format!("TermRef({})", audit_symbol_name(unit, *symbol)),
        Type::ThisType { class } => format!("ThisType({})", audit_symbol_name(unit, *class)),
        other => format!("{other:?}"),
    }
}

fn audit_type_snapshot(unit: &JvmCompilationUnit<'_>, ty: dotty_core::TypeId) -> String {
    match unit.store.types.get(ty) {
        Type::TypeRef { prefix, target } => {
            let target = match target {
                TypeRefTarget::Symbol(symbol) => audit_symbol_name(unit, *symbol),
                TypeRefTarget::Name(name) => {
                    unit.store.names.resolve(name.as_name().text()).to_owned()
                }
            };
            format!(
                "TypeRef(prefix={},target={target})",
                audit_prefix(unit, *prefix)
            )
        }
        Type::TermRef { prefix, target } => {
            let target = match target {
                TermRefTarget::Symbol(symbol) => audit_symbol_name(unit, *symbol),
                TermRefTarget::Name(name) => {
                    unit.store.names.resolve(name.as_name().text()).to_owned()
                }
            };
            format!(
                "TermRef(prefix={},target={target})",
                audit_prefix(unit, *prefix)
            )
        }
        Type::ThisType { class } => format!("ThisType({})", audit_symbol_name(unit, *class)),
        other => format!("{other:?}"),
    }
}

fn typed_source_shape(unit: &JvmCompilationUnit<'_>, tree: TreeId<Typed>) -> String {
    let node = unit.typed_ast.get(tree);
    let shape = match &node.kind {
        TreeKind::Ident(ident) => {
            let name = unit.store.names.resolve(ident.name.text());
            let target = term_type_target(unit, node.ty)
                .map(|symbol| audit_symbol_name(unit, symbol))
                .unwrap_or_else(|| "?".to_owned());
            format!("Ident({name}:{target})")
        }
        TreeKind::This(this) => format!(
            "This({})",
            this.qual.map_or_else(
                || "_".to_owned(),
                |name| { unit.store.names.resolve(name.text()).to_owned() }
            )
        ),
        TreeKind::Select(select) => format!(
            "Select({}, {})",
            unit.store.names.resolve(select.name.text()),
            typed_source_shape(unit, select.qualifier)
        ),
        TreeKind::Apply(application) => format!(
            "Apply({}, [{}])",
            typed_source_shape(unit, application.function),
            application
                .args
                .iter()
                .map(|argument| typed_source_shape(unit, *argument))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TreeKind::New(new) => format!("New({})", typed_source_shape(unit, new.tpt)),
        TreeKind::If(conditional) => format!(
            "If({}, {}, {})",
            typed_source_shape(unit, conditional.cond),
            typed_source_shape(unit, conditional.then_branch),
            typed_source_shape(unit, conditional.else_branch)
        ),
        TreeKind::TypeTree(_) => "TypeTree".to_owned(),
        TreeKind::Literal(_) => "Literal".to_owned(),
        other => format!("{other:?}"),
    };
    format!("{shape}:{}", audit_type_snapshot(unit, node.ty))
}

#[test]
fn reference_method_signature_audit_characterizes_real_typed_source() {
    let source = "class Counter(start: Int):\n  def get(): Int = 1\n  def self(): Counter = this\n\nclass Client:\n  def pass(c: Counter): Counter = c\n  def make(n: Int) = new Counter(n)\n  def read(c: Counter): Int = c.get()\n  def choose(flag: Boolean, a: Counter, b: Counter): Counter =\n    if flag then a else b\n  def mix(flag: Boolean, count: Int, c: Counter, tail: Boolean): Counter = c\n";
    let output = TestOutputDirectory::new("reference-signature-audit");
    let mut observed = Vec::new();
    let mut inspect = |unit: &mut JvmCompilationUnit<'_>, definitions: &Definitions| {
        let metadata = unit
            .metadata(definitions)
            .expect("reference signatures in the validated unit produce metadata");
        for class in &unit.classes {
            let class_name = unit
                .store
                .names
                .resolve(unit.store.symbols.get(class.class).name.text());
            for method in &class.methods {
                let method_name = unit
                    .store
                    .names
                    .resolve(unit.store.symbols.get(method.method).name.text());
                let signature = match unit.store.symbols.info(method.method) {
                    dotty_core::SymbolInfo::Complete(signature) => *signature,
                    other => panic!(
                        "method {method_name} should have complete typed signature, got {other:?}"
                    ),
                };
                let Type::Method(signature) = unit.store.types.get(signature) else {
                    panic!("method {method_name} should have one plain typed method clause")
                };
                if method_name == "make" {
                    let Type::TypeRef { prefix, target } = unit.store.types.get(signature.result)
                    else {
                        panic!("inferred make result should retain a canonical class TypeRef")
                    };
                    assert_eq!(*prefix, definitions.no_prefix);
                    let TypeRefTarget::Symbol(result_class) = target else {
                        panic!("inferred make result should target its exact class symbol")
                    };
                    assert!(unit.classes.iter().any(|plan| plan.class == *result_class));
                    assert_eq!(
                        unit.store
                            .names
                            .resolve(unit.store.symbols.get(*result_class).name.text()),
                        "Counter"
                    );
                }
                let params = signature
                    .params
                    .iter()
                    .map(|parameter| {
                        let name = unit.store.names.resolve(parameter.name.as_name().text());
                        let target = audit_type_snapshot(unit, parameter.ty);
                        format!("{name}:{target}")
                    })
                    .collect::<Vec<_>>();
                let result = audit_type_snapshot(unit, signature.result);
                let class_metadata = metadata
                    .iter()
                    .find(|item| item.identity().symbol() == class.class)
                    .expect("class metadata follows source class identity");
                let method_metadata = class_metadata
                    .methods()
                    .iter()
                    .find(|item| item.symbol() == method.method)
                    .expect("method metadata follows canonical method identity");
                let projected_parameters = signature
                    .params
                    .iter()
                    .map(|parameter| match unit.store.types.get(parameter.ty) {
                        Type::TypeRef {
                            target: TypeRefTarget::Symbol(symbol),
                            ..
                        } if Some(*symbol) == audit_type_symbol(unit, definitions.int) => {
                            dotty_jvm::JvmType::Int
                        }
                        Type::TypeRef {
                            target: TypeRefTarget::Symbol(symbol),
                            ..
                        } if Some(*symbol) == audit_type_symbol(unit, definitions.boolean) => {
                            dotty_jvm::JvmType::Boolean
                        }
                        Type::TypeRef {
                            target: TypeRefTarget::Symbol(symbol),
                            ..
                        } => dotty_jvm::JvmType::Reference(*symbol),
                        other => panic!("unexpected audit parameter type {other:?}"),
                    })
                    .collect::<Vec<_>>();
                let projected_result = match unit.store.types.get(signature.result) {
                    Type::TypeRef {
                        target: TypeRefTarget::Symbol(symbol),
                        ..
                    } if Some(*symbol) == audit_type_symbol(unit, definitions.unit) => {
                        dotty_jvm::JvmReturnType::Void
                    }
                    Type::TypeRef {
                        target: TypeRefTarget::Symbol(symbol),
                        ..
                    } if Some(*symbol) == audit_type_symbol(unit, definitions.int) => {
                        dotty_jvm::JvmReturnType::Value(dotty_jvm::JvmType::Int)
                    }
                    Type::TypeRef {
                        target: TypeRefTarget::Symbol(symbol),
                        ..
                    } if Some(*symbol) == audit_type_symbol(unit, definitions.boolean) => {
                        dotty_jvm::JvmReturnType::Value(dotty_jvm::JvmType::Boolean)
                    }
                    Type::TypeRef {
                        target: TypeRefTarget::Symbol(symbol),
                        ..
                    } => dotty_jvm::JvmReturnType::Value(dotty_jvm::JvmType::Reference(*symbol)),
                    other => panic!("unexpected audit result type {other:?}"),
                };
                assert_eq!(
                    method_metadata
                        .parameters()
                        .iter()
                        .map(|parameter| parameter.symbol())
                        .collect::<Vec<_>>(),
                    method.parameters,
                    "metadata preserves canonical source parameter order"
                );
                assert_eq!(
                    method_metadata.descriptor().parameters(),
                    projected_parameters,
                    "metadata preserves exact primitive/reference parameter types"
                );
                assert_eq!(method_metadata.descriptor().result(), projected_result);
                if projected_parameters
                    .iter()
                    .any(|parameter| matches!(parameter, dotty_jvm::JvmType::Reference(_)))
                    || matches!(
                        method_metadata.descriptor().result(),
                        dotty_jvm::JvmReturnType::Value(dotty_jvm::JvmType::Reference(_))
                    )
                {
                    assert!(matches!(
                        method.descriptor(unit.store, class.class, definitions),
                        Err(JvmCodegenError::UnsupportedJvmType {
                            kind: dotty_jvm::UnsupportedJvmTypeKind::ReferenceClass,
                            ..
                        })
                    ));
                }
                let descriptor_status =
                    format!("accepted({})", method_metadata.descriptor().as_str());
                let parameter_slots = method_metadata
                    .parameters()
                    .iter()
                    .map(|parameter| {
                        let category = match parameter.ty().category() {
                            dotty_jvm::JvmValueCategory::IntLike => "intlike",
                            dotty_jvm::JvmValueCategory::Float => "float",
                            dotty_jvm::JvmValueCategory::Long => "long",
                            dotty_jvm::JvmValueCategory::Double => "double",
                            dotty_jvm::JvmValueCategory::Reference => "reference",
                        };
                        format!(
                            "{}:{category}@{}+{}",
                            unit.store
                                .names
                                .resolve(unit.store.symbols.get(parameter.symbol()).name.text()),
                            parameter.slot(),
                            parameter.width()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                let body = method
                    .body
                    .map(|body| typed_source_shape(unit, body))
                    .unwrap_or_else(|| "<none>".to_owned());
                let candidate_descriptor = match method_name {
                    "get" => "()I",
                    "pass" => "(LCounter;)LCounter;",
                    "make" => "(I)LCounter;",
                    "self" => "()LCounter;",
                    "read" => "(LCounter;)I",
                    "choose" => "(ZLCounter;LCounter;)LCounter;",
                    "mix" => "(ZILCounter;Z)LCounter;",
                    _ => panic!("unexpected audit method {method_name}"),
                };
                observed.push(format!(
                    "{class_name}.{method_name} | params=[{}] | result={result} | tree={body} | candidate={candidate_descriptor} | backend={descriptor_status} | slots=[{parameter_slots}]",
                    params.join(", "),
                ));
            }
        }
    };
    let result = compile_source_batch_observed(
        source,
        "ReferenceSignatures.scala",
        output.path(),
        SourceId::from_index(1269),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut inspect),
    );
    let artifacts = result.expect("reference signature audit source emits planned classes");
    assert_eq!(artifacts.len(), 2);
    assert_eq!(output.class_files().len(), 2);
    assert_eq!(
        observed,
        [
            "Counter.get | params=[] | result=TypeRef(prefix=NoPrefix,target=Class:Int) | tree=Literal:Constant(Int(1)) | candidate=()I | backend=accepted(()I) | slots=[]",
            "Counter.self | params=[] | result=TypeRef(prefix=TypeRef(Package:<root>),target=Class:Counter) | tree=This(_):ThisType(Class:Counter) | candidate=()LCounter; | backend=accepted(()LCounter;) | slots=[]",
            "Client.pass | params=[c:TypeRef(prefix=TypeRef(Package:<root>),target=Class:Counter)] | result=TypeRef(prefix=TypeRef(Package:<root>),target=Class:Counter) | tree=Ident(c:Parameter:c):TermRef(prefix=NoPrefix,target=Parameter:c) | candidate=(LCounter;)LCounter; | backend=accepted((LCounter;)LCounter;) | slots=[c:reference@1+1]",
            "Client.make | params=[n:TypeRef(prefix=NoPrefix,target=Class:Int)] | result=TypeRef(prefix=NoPrefix,target=Class:Counter) | tree=Apply(Select(<init>, New(TypeTree:TypeRef(prefix=TypeRef(Package:<root>),target=Class:Counter)):TypeRef(prefix=TypeRef(Package:<root>),target=Class:Counter)):TermRef(prefix=TypeRef(Class:Counter),target=Constructor:<init>), [Ident(n:Parameter:n):TermRef(prefix=NoPrefix,target=Parameter:n)]):TypeRef(prefix=NoPrefix,target=Class:Counter) | candidate=(I)LCounter; | backend=accepted((I)LCounter;) | slots=[n:intlike@1+1]",
            "Client.read | params=[c:TypeRef(prefix=TypeRef(Package:<root>),target=Class:Counter)] | result=TypeRef(prefix=NoPrefix,target=Class:Int) | tree=Apply(Select(get, Ident(c:Parameter:c):TermRef(prefix=NoPrefix,target=Parameter:c)):TermRef(prefix=TermRef(Parameter:c),target=Method:get), []):TypeRef(prefix=NoPrefix,target=Class:Int) | candidate=(LCounter;)I | backend=accepted((LCounter;)I) | slots=[c:reference@1+1]",
            "Client.choose | params=[flag:TypeRef(prefix=NoPrefix,target=Class:Boolean), a:TypeRef(prefix=TypeRef(Package:<root>),target=Class:Counter), b:TypeRef(prefix=TypeRef(Package:<root>),target=Class:Counter)] | result=TypeRef(prefix=TypeRef(Package:<root>),target=Class:Counter) | tree=If(Ident(flag:Parameter:flag):TermRef(prefix=NoPrefix,target=Parameter:flag), Ident(a:Parameter:a):TermRef(prefix=NoPrefix,target=Parameter:a), Ident(b:Parameter:b):TermRef(prefix=NoPrefix,target=Parameter:b)):TypeRef(prefix=TypeRef(Package:<root>),target=Class:Counter) | candidate=(ZLCounter;LCounter;)LCounter; | backend=accepted((ZLCounter;LCounter;)LCounter;) | slots=[flag:intlike@1+1,a:reference@2+1,b:reference@3+1]",
            "Client.mix | params=[flag:TypeRef(prefix=NoPrefix,target=Class:Boolean), count:TypeRef(prefix=NoPrefix,target=Class:Int), c:TypeRef(prefix=TypeRef(Package:<root>),target=Class:Counter), tail:TypeRef(prefix=NoPrefix,target=Class:Boolean)] | result=TypeRef(prefix=TypeRef(Package:<root>),target=Class:Counter) | tree=Ident(c:Parameter:c):TermRef(prefix=NoPrefix,target=Parameter:c) | candidate=(ZILCounter;Z)LCounter; | backend=accepted((ZILCounter;Z)LCounter;) | slots=[flag:intlike@1+1,count:intlike@2+1,c:reference@3+1,tail:intlike@4+1]",
        ]
    );
}

#[test]
fn reference_method_descriptors_use_validated_package_internal_names() {
    fn descriptor_in_independent_session(source_id: u32, label: &str) -> String {
        let source = "package example.api\nclass Counter:\n  def pass(c: Counter): Counter = c\n";
        let output = TestOutputDirectory::new(label);
        let mut descriptor = None;
        let mut inspect = |unit: &mut JvmCompilationUnit<'_>, definitions: &Definitions| {
            let class = unit
                .classes
                .iter()
                .find(|class| {
                    unit.store
                        .names
                        .resolve(unit.store.symbols.get(class.class).name.text())
                        == "Counter"
                })
                .expect("Counter plan");
            let metadata = unit.metadata(definitions).expect("validated metadata");
            let counter = metadata
                .iter()
                .find(|metadata| metadata.identity().symbol() == class.class)
                .expect("Counter metadata");
            assert_eq!(counter.identity().internal_name(), "example/api/Counter");
            descriptor = Some(
                counter
                    .methods()
                    .iter()
                    .find(|method| method.name() == "pass")
                    .expect("pass metadata")
                    .descriptor()
                    .as_str()
                    .to_owned(),
            );
        };
        let result = compile_source_batch_observed(
            source,
            "PackagedReferenceSignatures.scala",
            output.path(),
            SourceId::from_index(source_id),
            SourceUnitCardinality::ExactlyOne,
            Some(&mut inspect),
        );
        let artifacts = result.expect("reference-returning method emits its class");
        assert_eq!(artifacts.len(), 1);
        assert_eq!(output.class_files().len(), 1);
        descriptor.expect("projected packaged descriptor")
    }

    let first = descriptor_in_independent_session(1270, "packaged-reference-signature-a");
    let second = descriptor_in_independent_session(1275, "packaged-reference-signature-b");
    assert_eq!(first, "(Lexample/api/Counter;)Lexample/api/Counter;");
    assert_eq!(second, first);
}

#[test]
fn reference_parameters_and_unqualified_this_lower_as_exact_values() {
    let source = "class Counter:\n  def value(): Int = 1\n\nclass Client(seed: Int):\n  def parameter(c: Counter): Int =\n    { c }\n    1\n  def current(): Int =\n    this\n    2\n  def currentIf(flag: Boolean): Int =\n    if flag then this else this\n    3\n  def mixed(flag: Boolean, count: Int, c: Counter, tail: Boolean): Int =\n    c\n    this\n    count\n\nclass Factory:\n  def build(): Int =\n    new Client(0)\n    4\n";
    let output = TestOutputDirectory::new("reference-parameter-values");
    let mut inspect = |unit: &mut JvmCompilationUnit<'_>, definitions: &Definitions| {
        let client = unit
            .classes
            .iter()
            .find(|plan| {
                unit.store
                    .names
                    .resolve(unit.store.symbols.get(plan.class).name.text())
                    == "Client"
            })
            .expect("Client plan")
            .class;
        for method_name in ["parameter", "current", "currentIf", "mixed"] {
            let method = unit
                .classes
                .iter()
                .find(|plan| plan.class == client)
                .unwrap()
                .methods
                .iter()
                .find(|method| {
                    unit.store
                        .names
                        .resolve(unit.store.symbols.get(method.method).name.text())
                        == method_name
                })
                .unwrap()
                .method;
            unit.lower_method_body(client, method, definitions)
                .unwrap_or_else(|error| {
                    panic!("public method lowering failed for {method_name}: {error:?}")
                });
        }
    };
    let artifacts = compile_source_batch_observed(
        source,
        "ReferenceParameterValues.scala",
        output.path(),
        SourceId::from_index(1273),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut inspect),
    )
    .expect("reference parameter and this values lower in planned methods");
    let client_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Client")
        .expect("Client artifact");
    let client = decode_emitted_class(client_artifact);

    assert_eq!(
        emitted_method_code(&client, "parameter"),
        [0x2b, 0x57, 0x04, 0xac]
    );
    assert_eq!(
        emitted_method_code(&client, "current"),
        [0x2a, 0x57, 0x05, 0xac]
    );
    assert!(emitted_method_code(&client, "currentIf").contains(&0x2a));
    assert_eq!(
        emitted_method_code(&client, "mixed"),
        [0x2d, 0x57, 0x2a, 0x57, 0x1c, 0xac]
    );

    if !require_jdk_or_skip() {
        return;
    }
    let harness = output.path().join("ReferenceParameterValuesHarness.java");
    fs::write(
        &harness,
        r#"public final class ReferenceParameterValuesHarness {
    public static void main(String[] args) {
        Client client = new Client(0);
        Counter counter = new Counter();
        if (client.parameter(counter) != 1) throw new AssertionError("parameter");
        if (client.current() != 2) throw new AssertionError("this");
        if (client.currentIf(true) != 3 || client.currentIf(false) != 3) throw new AssertionError("this if");
        if (client.mixed(true, 42, counter, false) != 42) throw new AssertionError("mixed");
        if (new Factory().build() != 4) throw new AssertionError("allocation");
    }
}
"#,
    )
    .unwrap();
    let classpath = output.path().to_string_lossy().into_owned();
    let javac = Command::new("javac")
        .args(["-classpath", &classpath, "-d", &classpath])
        .arg(&harness)
        .output()
        .expect("start javac for reference parameter values");
    assert!(
        javac.status.success(),
        "javac rejected the reference parameter harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            &classpath,
            "ReferenceParameterValuesHarness",
        ])
        .output()
        .expect("start java for reference parameter values");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected the generated classes\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
}

#[test]
fn reference_value_extensions_keep_exact_identity_and_allow_reference_call_arguments() {
    let mismatch = "class Counter:\n  def value(): Int = 1\nclass Client:\n  def choose(flag: Boolean, counter: Counter): Int =\n    if flag then this else counter\n    1\n";
    let output = TestOutputDirectory::new("reference-value-identity-mismatch");
    let error = compile_source_classes(mismatch, "ReferenceIdentityMismatch.scala", output.path())
        .expect_err("different planned classes cannot join as a reference value");
    assert!(matches!(
        &error,
        PipelineFailure::Codegen(JvmCodegenError::ReferenceTypeMismatch { .. })
    ));
    assert!(output.class_files().is_empty());

    let reference_argument = "class Counter:\n  def value(): Int = 1\nclass Client:\n  def accept(counter: Counter): Int = 1\n  def pass(counter: Counter): Int = accept(counter)\n";
    let output = TestOutputDirectory::new("reference-call-argument");
    let artifacts = compile_source_classes(
        reference_argument,
        "ReferenceCallArgument.scala",
        output.path(),
    )
    .expect("an exact planned reference parameter may be forwarded to a virtual call");
    assert_eq!(artifacts.len(), 2);
    let client = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Client")
        .unwrap();
    let class = decode_emitted_class(client);
    class
        .validate_references()
        .expect("forwarded descriptor validates");
    let pass = emitted_method_code(&class, "pass");
    assert_opcode_sequence(pass, &[0x2a, 0x2b, 0xb6, 0xac]);
    assert_invocation_sequence(&class, pass, &[(0xb6, "Client", "accept", "(LCounter;)I")]);
}

#[test]
fn reference_method_descriptors_reject_canonical_classes_outside_the_unit() {
    let source =
        "class Counter:\n  def id(): Int = 1\nclass Client:\n  def pass(c: Counter): Counter = c\n";
    let output = TestOutputDirectory::new("unplanned-reference-signature");
    let mut projection_error = None;
    let mut expected_class = None;
    let mut inspect = |unit: &mut JvmCompilationUnit<'_>, definitions: &Definitions| {
        let counter = unit
            .classes
            .iter()
            .find(|class| {
                unit.store
                    .names
                    .resolve(unit.store.symbols.get(class.class).name.text())
                    == "Counter"
            })
            .expect("Counter plan")
            .class;
        expected_class = Some(counter);
        unit.classes.retain(|class| class.class != counter);
        projection_error = Some(unit.metadata(definitions).unwrap_err());
    };
    let result = compile_source_batch_observed(
        source,
        "UnplannedReferenceSignatures.scala",
        output.path(),
        SourceId::from_index(1271),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut inspect),
    );
    assert!(matches!(result, Err(PipelineFailure::Codegen(_))));
    let counter = match projection_error {
        Some(JvmCodegenError::ReferenceClassNotInJvmPlan { class }) => class,
        other => panic!("expected canonical but unplanned class rejection, got {other:?}"),
    };
    assert_eq!(Some(counter), expected_class);
    assert!(
        !output.class_files().iter().any(|path| {
            path.file_name().and_then(|name| name.to_str()) == Some("Counter.class")
        })
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn reference_constructor_parameters_project_exact_unit_descriptors() {
    let source =
        "class Counter:\n  def id(): Int = 1\nclass Client(c: Counter):\n  def id(): Int = 1\n";
    let output = TestOutputDirectory::new("reference-constructor-parameter");
    let mut projection = None;
    let mut inspect = |unit: &mut JvmCompilationUnit<'_>, definitions: &Definitions| {
        let metadata = unit.metadata(definitions).unwrap();
        let client = metadata
            .iter()
            .find(|metadata| {
                unit.store.names.resolve(
                    unit.store
                        .symbols
                        .get(metadata.identity().symbol())
                        .name
                        .text(),
                ) == "Client"
            })
            .unwrap();
        projection = Some(
            client
                .constructor()
                .unwrap()
                .descriptor()
                .as_str()
                .to_owned(),
        );
    };
    let result = compile_source_batch_observed(
        source,
        "ReferenceConstructorParameter.scala",
        output.path(),
        SourceId::from_index(1274),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut inspect),
    );
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(projection.as_deref(), Some("(LCounter;)V"));
    assert_eq!(output.class_files().len(), 2);
}

#[test]
fn reference_method_descriptor_slots_include_mixed_parameters_and_receiver() {
    fn source_with_parameter_count(count: usize) -> (String, String) {
        let mut parameters = Vec::with_capacity(count);
        let mut descriptor = String::new();
        for index in 0..count {
            let ty = match index % 3 {
                0 => {
                    descriptor.push('Z');
                    "Boolean"
                }
                1 => {
                    descriptor.push_str("LCounter;");
                    "Counter"
                }
                _ => {
                    descriptor.push('I');
                    "Int"
                }
            };
            parameters.push(format!("p{index}: {ty}"));
        }
        (
            format!(
                "class Counter:\n  def id(): Int = 1\nclass Client:\n  def slots({}): Unit = ()\n",
                parameters.join(", ")
            ),
            format!("({descriptor})V"),
        )
    }

    let (source, expected_descriptor) = source_with_parameter_count(254);
    let output = TestOutputDirectory::new("reference-slot-boundary");
    let mut projection = None;
    let mut inspect = |unit: &mut JvmCompilationUnit<'_>, definitions: &Definitions| {
        let metadata = unit.metadata(definitions).expect("254 instance slots fit");
        let client = metadata
            .iter()
            .find(|class| {
                unit.store.names.resolve(
                    unit.store
                        .symbols
                        .get(class.identity().symbol())
                        .name
                        .text(),
                ) == "Client"
            })
            .expect("Client metadata");
        let method = client
            .methods()
            .iter()
            .find(|method| method.name() == "slots")
            .expect("slots metadata");
        projection = Some((
            method.descriptor().as_str().to_owned(),
            method
                .parameters()
                .iter()
                .map(|parameter| parameter.slot())
                .collect::<Vec<_>>(),
        ));
    };
    let result = compile_source_batch_observed(
        &source,
        "ReferenceSlotBoundary.scala",
        output.path(),
        SourceId::from_index(1272),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut inspect),
    );
    assert!(result.is_ok());
    let (descriptor, slots) = projection.expect("unit metadata was inspected");
    assert_eq!(descriptor, expected_descriptor);
    assert_eq!(slots.len(), 254);
    assert_eq!(slots.first(), Some(&1));
    assert_eq!(slots.last(), Some(&254));
    assert_eq!(output.class_files().len(), 2);

    let (source, _) = source_with_parameter_count(255);
    let output = TestOutputDirectory::new("reference-slot-overflow");
    let mut projection_error = None;
    let mut inspect = |unit: &mut JvmCompilationUnit<'_>, definitions: &Definitions| {
        projection_error = Some(unit.metadata(definitions).unwrap_err());
    };
    let result = compile_source_batch_observed(
        &source,
        "ReferenceSlotOverflow.scala",
        output.path(),
        SourceId::from_index(1273),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut inspect),
    );
    assert!(matches!(result, Err(PipelineFailure::Codegen(_))));
    assert!(matches!(
        projection_error,
        Some(JvmCodegenError::TooManyParameterSlots { slots: 256, .. })
    ));
    assert!(output.class_files().is_empty());
}

fn publish_class_batch(
    output_directory: &Path,
    classes: Vec<GeneratedClass>,
) -> Result<Vec<EmittedArtifact>, PipelineFailure> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT_STAGING_ID: AtomicUsize = AtomicUsize::new(0);

    fs::create_dir_all(output_directory).map_err(PipelineFailure::Io)?;
    let staging_root = loop {
        let id = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
        let candidate =
            output_directory.join(format!(".dotty-jvm-batch-{}-{id}", std::process::id()));
        match fs::create_dir(&candidate) {
            Ok(()) => break candidate,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(PipelineFailure::Io(error)),
        }
    };
    let mut paths = std::collections::HashSet::new();
    let mut pending = Vec::with_capacity(classes.len());
    let stage_result = (|| {
        for (index, class) in classes.into_iter().enumerate() {
            let relative = PathBuf::from(format!("{}.class", class.internal_name()));
            if relative.as_os_str().is_empty()
                || !relative
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_)))
            {
                return Err(PipelineFailure::UnexpectedSourceShape(
                    "validated JVM output path",
                ));
            }
            let path = output_directory.join(&relative);
            if !paths.insert(path.clone()) {
                return Err(PipelineFailure::OutputPathCollision(path));
            }
            let staged_path = staging_root.join("new").join(&relative);
            let backup_path = staging_root.join("backups").join(format!("{index}.bak"));
            if let Some(parent) = staged_path.parent() {
                fs::create_dir_all(parent).map_err(PipelineFailure::Io)?;
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(PipelineFailure::Io)?;
            }
            fs::write(&staged_path, class.bytes()).map_err(PipelineFailure::Io)?;
            pending.push(PendingArtifact {
                class,
                path,
                staged_path,
                backup_path,
            });
        }
        Ok(())
    })();
    if let Err(error) = stage_result {
        let _ = fs::remove_dir_all(&staging_root);
        return Err(error);
    }

    let mut published = Vec::<(PathBuf, Option<PathBuf>)>::new();
    for artifact in &pending {
        let backup = match fs::symlink_metadata(&artifact.path) {
            Ok(metadata) if metadata.file_type().is_file() => {
                if let Some(parent) = artifact.backup_path.parent()
                    && let Err(error) = fs::create_dir_all(parent)
                {
                    rollback_published(&published);
                    let _ = fs::remove_dir_all(&staging_root);
                    return Err(PipelineFailure::Io(error));
                }
                if let Err(error) = fs::rename(&artifact.path, &artifact.backup_path) {
                    rollback_published(&published);
                    let _ = fs::remove_dir_all(&staging_root);
                    return Err(PipelineFailure::Io(error));
                }
                Some(artifact.backup_path.clone())
            }
            Ok(_) => {
                rollback_published(&published);
                let _ = fs::remove_dir_all(&staging_root);
                return Err(PipelineFailure::OutputPathCollision(artifact.path.clone()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                rollback_published(&published);
                let _ = fs::remove_dir_all(&staging_root);
                return Err(PipelineFailure::Io(error));
            }
        };
        if let Err(error) = fs::rename(&artifact.staged_path, &artifact.path) {
            if let Some(backup) = &backup {
                let _ = fs::rename(backup, &artifact.path);
            }
            rollback_published(&published);
            let _ = fs::remove_dir_all(&staging_root);
            return Err(PipelineFailure::Io(error));
        }
        published.push((artifact.path.clone(), backup));
    }
    let _ = fs::remove_dir_all(&staging_root);
    Ok(pending
        .into_iter()
        .map(|artifact| EmittedArtifact {
            class: artifact.class,
            path: artifact.path,
        })
        .collect())
}

fn rollback_published(published: &[(PathBuf, Option<PathBuf>)]) {
    for (path, backup) in published.iter().rev() {
        let _ = fs::remove_file(path);
        if let Some(backup) = backup {
            let _ = fs::rename(backup, path);
        }
    }
}
fn resolve_source_constructor_calls(source_text: &str) -> Result<Vec<String>, PipelineFailure> {
    let source_id = SourceId::from_index(91);
    let source = SourceText::new(source_text)
        .map_err(|error| PipelineFailure::SourceText(error.to_string()))?;
    let scanner = ContextualScanner::new(source_text)
        .map_err(|error| PipelineFailure::Lexer(format!("{error:?}")))?;
    if !scanner.diagnostics().is_empty() {
        return Err(PipelineFailure::LexerDiagnostics(
            scanner.diagnostics().to_vec(),
        ));
    }
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source, source_id, scanner, &mut store.names);
    if !parsed.diagnostics.is_empty() {
        return Err(PipelineFailure::Parser(parsed.diagnostics));
    }
    let mut packages = Packages::new();
    let index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "constructor-call-resolver.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .map_err(PipelineFailure::NamerStructural)?
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        NamingOutcome::Named { diagnostics, .. } | NamingOutcome::Rejected { diagnostics } => {
            return Err(PipelineFailure::NamerDiagnostics(diagnostics));
        }
    };
    let (class, template_tree) =
        find_single_source_class(&parsed.ast, parsed.root, source_id, &index, &store)?;
    let TreeKind::Template(template) = &parsed.ast.get(template_tree).kind else {
        return Err(PipelineFailure::UnexpectedSourceShape("class template"));
    };
    let source_methods =
        collect_source_methods(&parsed.ast, &template.body, source_id, &index, &store)?;
    let initialization =
        source_initialization_plan(class, &parsed.ast, template, source_id, &index, &store)
            .map_err(PipelineFailure::Codegen)?;
    let constructor = index.symbol_at(source_id, template.constructor).ok_or(
        PipelineFailure::MissingSourceIdentity("primary constructor"),
    )?;
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
        .map_err(PipelineFailure::Typer)?;
    if matches!(
        initialization,
        JvmClassInitializationPlan::PrimaryConstructor(_)
    ) {
        typer
            .complete_symbol(constructor)
            .map_err(PipelineFailure::Typer)?;
    }
    let mut fields = Vec::new();
    for tree in &template.body {
        let TreeKind::ValDef(definition) = &parsed.ast.get(*tree).kind else {
            continue;
        };
        let Some(field) = index.symbol_at(source_id, *tree) else {
            continue;
        };
        if typer.store().symbols.get(field).kind != SymbolKind::Field {
            continue;
        }
        if index.definition_of(field)
            != Some(SourceDefinition::Canonical {
                source: source_id,
                tree: *tree,
            })
        {
            return Err(PipelineFailure::MissingSourceIdentity(
                "canonical class-body field definition",
            ));
        }
        typer
            .complete_symbol(field)
            .map_err(PipelineFailure::Typer)?;
        let initializer = match definition.rhs {
            Some(rhs) => {
                let ty = match typer.store().symbols.get(field).info {
                    dotty_core::SymbolInfo::Complete(ty) => ty,
                    _ => return Err(PipelineFailure::UnexpectedSourceShape("field type")),
                };
                let context = typer
                    .field_initializer_context_for(field)
                    .map_err(PipelineFailure::Typer)?;
                let typed_rhs = typer
                    .type_expression_expected(rhs, context, ty)
                    .map_err(PipelineFailure::Typer)?;
                if typer.source_typed_index().get(source_id, rhs) != Some(typed_rhs) {
                    return Err(PipelineFailure::UnexpectedSourceShape(
                        "source-to-typed field initializer binding",
                    ));
                }
                Some(typed_rhs)
            }
            None => None,
        };
        let source_visibility = match definition.metadata.visibility {
            None => {
                let initializer = initializer.ok_or(PipelineFailure::Codegen(
                    JvmCodegenError::MissingFieldInitializer { field },
                ))?;
                JvmFieldSourceVisibility::PlainPublic(
                    JvmPublicBodyFieldSourceProof::from_source(
                        class,
                        field,
                        source_id,
                        *tree,
                        initializer,
                        &parsed.ast,
                        &index,
                        typer.source_typed_index(),
                        typer.typed_ast(),
                        typer.store(),
                    )
                    .map_err(PipelineFailure::Codegen)?,
                )
            }
            Some(dotty_core::ast::VisibilitySyntax::Private { qualifier: None }) => {
                JvmFieldSourceVisibility::PlainPrivate
            }
            Some(dotty_core::ast::VisibilitySyntax::Private { qualifier: Some(_) }) => {
                JvmFieldSourceVisibility::PrivateThis
            }
            Some(dotty_core::ast::VisibilitySyntax::Protected { .. }) => {
                JvmFieldSourceVisibility::PlainPrivate
            }
        };
        fields.push(JvmFieldPlan {
            field,
            source_visibility,
            initializer,
        });
    }
    let mut methods = Vec::with_capacity(source_methods.len());
    for source_method in source_methods {
        typer
            .complete_symbol(source_method.symbol)
            .map_err(PipelineFailure::Typer)?;
        let body = match source_method.rhs {
            Some(rhs) => {
                let context = typer
                    .expression_context_for(source_method.symbol)
                    .map_err(PipelineFailure::Typer)?;
                Some(
                    typer
                        .type_expression(rhs, context)
                        .map_err(PipelineFailure::Typer)?,
                )
            }
            None => None,
        };
        let mut locals = parsed
            .ast
            .iter()
            .filter_map(|(source_tree, node)| {
                if !matches!(node.kind, TreeKind::ValDef(_)) {
                    return None;
                }
                let binding = typer.typed_local_binding_at(source_id, source_tree)?;
                (typer.store().symbols.get(binding.symbol()).owner == Some(source_method.symbol))
                    .then_some(JvmLocalBinding {
                        declaration: binding.declaration(),
                        symbol: binding.symbol(),
                        mutable: typer
                            .store()
                            .symbols
                            .get(binding.symbol())
                            .flags
                            .contains(SymbolFlags::MUTABLE),
                    })
            })
            .collect::<Vec<_>>();
        locals.sort_by_key(|binding| binding.declaration.index());
        methods.push(JvmMethodPlan {
            method: source_method.symbol,
            parameters: source_method.parameters,
            locals,
            body,
        });
    }
    let unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: vec![JvmClassPlan {
            class,
            methods,
            fields,
            initialization,
        }],
    };
    let caller_method = unit.classes[0].methods[0].method;
    let applications = unit
        .typed_ast
        .iter()
        .filter_map(|(tree, node)| {
            let TreeKind::Apply(apply) = &node.kind else {
                return None;
            };
            let function = unit.typed_ast.try_get(apply.function)?;
            let TreeKind::Select(select) = &function.kind else {
                return None;
            };
            matches!(
                unit.typed_ast
                    .try_get(select.qualifier)
                    .map(|tree| &tree.kind),
                Some(TreeKind::New(_))
            )
            .then_some(tree)
        })
        .collect::<Vec<_>>();
    applications
        .into_iter()
        .map(|application| {
            unit.resolve_constructor_call(class, caller_method, application, &definitions)
                .map(|target| target.descriptor().as_str().to_owned())
                .map_err(PipelineFailure::Codegen)
        })
        .collect()
}

fn assert_source_failure(
    label: &str,
    source: &str,
    expected_stage: impl FnOnce(&PipelineFailure) -> bool,
) {
    let output = TestOutputDirectory::new(label);
    let error = compile_source(source, &format!("{label}.scala"), output.path())
        .expect_err("unsupported source must fail before publishing a class");
    assert!(
        expected_stage(&error),
        "{label} failed at an unexpected pipeline stage: {error:?}"
    );
    assert!(
        output.class_files().is_empty(),
        "{label} failure must not leave a partial class artifact"
    );
}

fn assert_source_batch_failure(
    label: &str,
    source: &str,
    expected_stage: impl FnOnce(&PipelineFailure) -> bool,
) {
    let output = TestOutputDirectory::new(label);
    let error = compile_planned_constructor_source(source, output.path())
        .expect_err("unsupported source batch must fail before publishing classes");
    assert!(
        expected_stage(&error),
        "{label} failed at an unexpected pipeline stage: {error:?}"
    );
    assert!(
        output.class_files().is_empty(),
        "{label} failure must not leave a partial class artifact"
    );
}

#[test]
fn resolves_real_source_constructor_calls_with_package_and_primitive_references() {
    let descriptors = resolve_source_constructor_calls(
        "package example\nclass Counter(start: Int) { private var stored: Int = 1; def make(seed: Int): Int = { val local = seed; new Counter(seed); new Counter(local); new Counter(this.stored); 0 } }",
    )
    .expect("real source constructor applications should resolve");

    assert_eq!(descriptors, ["(I)V", "(I)V", "(I)V"]);
}

fn find_single_source_class(
    ast: &dotty_core::AstArena<Untyped>,
    root: TreeId<Untyped>,
    source: SourceId,
    index: &SourceSemanticIndex,
    store: &SemanticStore,
) -> Result<(SymbolId, TreeId<Untyped>), PipelineFailure> {
    let TreeKind::PackageDef(package) = &ast.get(root).kind else {
        return Err(PipelineFailure::UnexpectedSourceShape("package root"));
    };
    let mut classes = Vec::new();
    for tree in &package.stats {
        if !matches!(ast.get(*tree).kind, TreeKind::TypeDef(_)) {
            continue;
        }
        let symbol = index
            .symbol_at(source, *tree)
            .ok_or(PipelineFailure::MissingSourceIdentity("class declaration"))?;
        if store.symbols.get(symbol).kind != SymbolKind::Class {
            continue;
        }
        let TreeKind::TypeDef(definition) = &ast.get(*tree).kind else {
            unreachable!();
        };
        if !matches!(ast.get(definition.rhs).kind, TreeKind::Template(_)) {
            return Err(PipelineFailure::UnexpectedSourceShape("class template"));
        }
        classes.push((symbol, definition.rhs));
    }
    match classes.as_slice() {
        [class] => Ok(*class),
        _ => Err(PipelineFailure::UnexpectedSourceShape(
            "exactly one ordinary top-level class",
        )),
    }
}

fn collect_source_methods(
    ast: &dotty_core::AstArena<Untyped>,
    body: &[TreeId<Untyped>],
    source: SourceId,
    index: &SourceSemanticIndex,
    store: &SemanticStore,
) -> Result<Vec<SourceMethod>, PipelineFailure> {
    let mut methods = Vec::new();
    for tree in body {
        let TreeKind::DefDef(definition) = &ast.get(*tree).kind else {
            continue;
        };
        let symbol = index
            .symbol_at(source, *tree)
            .ok_or(PipelineFailure::MissingSourceIdentity("method declaration"))?;
        if store.symbols.get(symbol).kind != SymbolKind::Method {
            continue;
        }
        let parameters =
            definition
                .value_param_clauses
                .iter()
                .flatten()
                .map(|parameter| {
                    index.symbol_at(source, *parameter).ok_or(
                        PipelineFailure::MissingSourceIdentity("method parameter declaration"),
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
        methods.push(SourceMethod {
            symbol,
            parameters,
            rhs: definition.rhs,
        });
    }
    Ok(methods)
}

fn source_initialization_plan(
    class: SymbolId,
    ast: &dotty_core::AstArena<Untyped>,
    template: &Template<Untyped>,
    source: SourceId,
    index: &SourceSemanticIndex,
    store: &SemanticStore,
) -> Result<JvmClassInitializationPlan, JvmCodegenError> {
    let has_fields = template.body.iter().any(|tree| {
        matches!(ast.get(*tree).kind, TreeKind::ValDef(_))
            && index
                .symbol_at(source, *tree)
                .is_some_and(|symbol| store.symbols.get(symbol).kind == SymbolKind::Field)
    });
    let has_public_body_fields = template.body.iter().any(|tree| {
        let TreeKind::ValDef(definition) = &ast.get(*tree).kind else {
            return false;
        };
        definition.metadata.visibility.is_none()
            && index
                .symbol_at(source, *tree)
                .is_some_and(|symbol| store.symbols.get(symbol).kind == SymbolKind::Field)
    });
    let constructor = index
        .symbol_at(source, template.constructor)
        .ok_or(JvmCodegenError::UnsupportedConstructor { class })?;
    let TreeKind::DefDef(definition) = &ast.get(template.constructor).kind else {
        return Ok(JvmClassInitializationPlan::Unsupported);
    };
    let has_parameters = definition
        .value_param_clauses
        .iter()
        .any(|clause| !clause.is_empty());
    let has_parent_arguments = template.parents.iter().any(|parent| {
        matches!(ast.get(*parent).kind, TreeKind::Apply(ref application) if !application.args.is_empty())
    });
    let no_template_initializers = template.body.iter().all(|tree| match &ast.get(*tree).kind {
        TreeKind::DefDef(_) => true,
        TreeKind::ValDef(_) => index
            .symbol_at(source, *tree)
            .is_some_and(|symbol| store.symbols.get(symbol).kind == SymbolKind::Field),
        _ => false,
    });
    let has_explicit_empty_clause = !definition.value_param_clauses.is_empty();
    if !has_parameters
        && !has_parent_arguments
        && (!has_fields || (has_public_body_fields && !has_explicit_empty_clause))
    {
        return Ok(prove_implicit_noarg_constructor(
            ast, template, source, index, store,
        ));
    }
    if !has_supported_parent_shape(ast, template)
        || template.self_val.is_some()
        || !no_template_initializers
    {
        return Ok(JvmClassInitializationPlan::Unsupported);
    }
    let parameters = definition
        .value_param_clauses
        .iter()
        .flatten()
        .map(|tree| {
            index.derived_symbol_at(constructor, source, *tree).ok_or(
                JvmCodegenError::InvalidPrimaryConstructor {
                    constructor,
                    issue: dotty_jvm::JvmConstructorIssue::ParameterIdentity,
                },
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(JvmClassInitializationPlan::PrimaryConstructor(
        JvmPrimaryConstructorPlan::from_source(class, constructor, parameters, ast, index)?,
    ))
}

fn prove_implicit_noarg_constructor(
    ast: &dotty_core::AstArena<Untyped>,
    template: &Template<Untyped>,
    source: SourceId,
    index: &SourceSemanticIndex,
    store: &SemanticStore,
) -> JvmClassInitializationPlan {
    let constructor_is_public =
        index
            .symbol_at(source, template.constructor)
            .is_some_and(|constructor| {
                store.symbols.contains(constructor)
                    && store.symbols.get(constructor).kind == SymbolKind::Constructor
                    && store.symbols.get(constructor).visibility == Visibility::Public
            });
    let constructor_matches_emitted_shape = match &ast.get(template.constructor).kind {
        TreeKind::DefDef(DefDef {
            type_params,
            value_param_clauses,
            metadata,
            ..
        }) => {
            constructor_is_public
                && metadata.visibility.is_none()
                && type_params.is_empty()
                && value_param_clauses.iter().all(Vec::is_empty)
        }
        _ => false,
    };
    let body_contains_supported_members =
        template.body.iter().all(|tree| match &ast.get(*tree).kind {
            TreeKind::DefDef(_) => true,
            TreeKind::ValDef(_) => index
                .symbol_at(source, *tree)
                .is_some_and(|symbol| store.symbols.get(symbol).kind == SymbolKind::Field),
            _ => false,
        });
    if constructor_matches_emitted_shape
        && has_supported_noarg_parent(ast, template)
        && template.self_val.is_none()
        && body_contains_supported_members
    {
        JvmClassInitializationPlan::ImplicitNoArg
    } else {
        JvmClassInitializationPlan::Unsupported
    }
}

fn has_supported_noarg_parent(
    ast: &dotty_core::AstArena<Untyped>,
    template: &Template<Untyped>,
) -> bool {
    match template.parents.as_slice() {
        [] => true,
        [parent] => match &ast.get(*parent).kind {
            TreeKind::Ident(_) | TreeKind::Select(_) => true,
            TreeKind::Apply(application) if application.args.is_empty() => {
                matches!(
                    ast.get(application.function).kind,
                    TreeKind::Ident(_) | TreeKind::Select(_)
                )
            }
            _ => false,
        },
        _ => false,
    }
}

fn has_supported_parent_shape(
    ast: &dotty_core::AstArena<Untyped>,
    template: &Template<Untyped>,
) -> bool {
    if has_supported_noarg_parent(ast, template) {
        return true;
    }
    match template.parents.as_slice() {
        [parent] => match &ast.get(*parent).kind {
            TreeKind::Apply(application)
                if application.kind == dotty_core::ast::ApplyKind::Regular =>
            {
                matches!(
                    ast.get(application.function).kind,
                    TreeKind::Select(select)
                        if !select.backquoted
                            && matches!(ast.get(select.qualifier).kind, TreeKind::New(_))
                )
            }
            _ => false,
        },
        _ => false,
    }
}

#[test]
fn scala_answer_source_flows_through_the_frontend_into_a_verifiable_class() {
    let first_output = TestOutputDirectory::new("answer-first");
    let second_output = TestOutputDirectory::new("answer-second");
    let first = compile_fixture("Answer.scala", first_output.path()).unwrap();
    let second = compile_fixture("Answer.scala", second_output.path()).unwrap();

    assert_eq!(first.class.internal_name(), "Answer");
    assert_eq!(
        first.path.file_name().and_then(|name| name.to_str()),
        Some("Answer.class")
    );
    assert_eq!(first.class.bytes(), second.class.bytes());
    assert_eq!(first_output.class_files(), vec![first.path.clone()]);
    assert_eq!(second_output.class_files(), vec![second.path.clone()]);
    verify_class_structure(first.class.bytes());

    if require_jdk_or_skip() {
        verify_with_jdk(first_output.path());
    }
}

#[test]
fn scala_delegator_calls_and_blocks_flow_through_the_frontend_into_a_verifiable_class() {
    let first_output = TestOutputDirectory::new("delegator-first");
    let second_output = TestOutputDirectory::new("delegator-second");
    let first = compile_fixture("Delegator.scala", first_output.path()).unwrap();
    let second = compile_fixture("Delegator.scala", second_output.path()).unwrap();

    assert_eq!(first.class.internal_name(), "Delegator");
    assert_eq!(first.class.bytes(), second.class.bytes());
    assert_eq!(first_output.class_files(), vec![first.path.clone()]);
    assert_eq!(second_output.class_files(), vec![second.path.clone()]);

    let mut reader = Reader::new(first.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("emitted Delegator should decode");
    assert!(
        reader.is_at_end(),
        "decoder consumes the complete class file"
    );
    class
        .validate_references()
        .expect("Delegator constant-pool references should be valid");
    assert_eq!(
        class.constant_pool.class_name(class.this_class),
        Ok("Delegator")
    );
    assert!(class.access_flags.is_public());
    assert_eq!(
        class
            .methods
            .iter()
            .map(|method| (
                class.constant_pool.utf8(method.name_index).unwrap(),
                class.constant_pool.utf8(method.descriptor_index).unwrap()
            ))
            .collect::<Vec<_>>(),
        [
            ("<init>", "()V"),
            ("identity", "(I)I"),
            ("direct", "(I)I"),
            ("explicit", "(I)I"),
            ("chain", "(I)I"),
            ("constant", "()I"),
            ("zero", "()I"),
            ("sequence", "(I)I"),
        ]
    );
    for method in &class.methods {
        assert!(method.access_flags.is_public());
        assert!(matches!(method.attributes.as_slice(), [Attribute::Code(_)]));
    }

    assert_opcode_sequence(emitted_method_code(&class, "identity"), &[0x1b, 0xac]);
    let direct = emitted_method_code(&class, "direct");
    assert_opcode_sequence(direct, &[0x2a, 0x1b, 0xb6, 0xac]);
    assert_virtual_invocations(&class, direct, &[("identity", "(I)I")]);

    let explicit = emitted_method_code(&class, "explicit");
    assert_eq!(explicit, direct);
    assert_virtual_invocations(&class, explicit, &[("identity", "(I)I")]);

    let chain = emitted_method_code(&class, "chain");
    assert_opcode_sequence(chain, &[0x2a, 0x2a, 0x1b, 0xb6, 0xb6, 0xac]);
    assert_virtual_invocations(&class, chain, &[("identity", "(I)I"), ("identity", "(I)I")]);

    let constant = emitted_method_code(&class, "constant");
    assert_opcode_sequence(constant, &[0x10, 0xac]);
    assert_eq!(constant[1], 42);
    let zero = emitted_method_code(&class, "zero");
    assert_opcode_sequence(zero, &[0x2a, 0xb6, 0xac]);
    assert_virtual_invocations(&class, zero, &[("constant", "()I")]);

    let sequence = emitted_method_code(&class, "sequence");
    assert_opcode_sequence(sequence, &[0x2a, 0x1b, 0xb6, 0x57, 0x2a, 0x10, 0xb6, 0xac]);
    let instructions = decode_subset_instructions(sequence);
    let (constant_offset, _) = instructions
        .iter()
        .find(|(_, opcode)| *opcode == 0x10)
        .expect("sequence has one bipush instruction");
    assert_eq!(sequence[constant_offset + 1], 42);
    let calls = assert_virtual_invocations(
        &class,
        sequence,
        &[("identity", "(I)I"), ("identity", "(I)I")],
    );
    let first_call_index = instructions
        .iter()
        .position(|(offset, opcode)| *opcode == 0xb6 && *offset == calls[0])
        .unwrap();
    let second_call_index = instructions
        .iter()
        .position(|(offset, opcode)| *opcode == 0xb6 && *offset == calls[1])
        .unwrap();
    assert_eq!(instructions[first_call_index + 1].1, 0x57);
    assert_eq!(instructions[second_call_index + 1].1, 0xac);

    if require_jdk_or_skip() {
        verify_delegator_with_jdk(first_output.path());
    }
}

#[test]
fn calculator_fixture_compiles_deterministically_and_executes_primitive_intrinsics() {
    let first_output = TestOutputDirectory::new("calculator-first");
    let second_output = TestOutputDirectory::new("calculator-second");
    let first = compile_fixture("Calculator.scala", first_output.path()).unwrap();
    let second = compile_fixture("Calculator.scala", second_output.path()).unwrap();

    assert_eq!(first.class.internal_name(), "Calculator");
    assert_eq!(first.class.bytes(), second.class.bytes());
    assert_eq!(first_output.class_files(), vec![first.path.clone()]);
    assert_eq!(second_output.class_files(), vec![second.path.clone()]);

    let mut reader = Reader::new(first.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("Calculator should decode completely");
    assert!(
        reader.is_at_end(),
        "decoder consumes the complete class file"
    );
    class
        .validate_references()
        .expect("Calculator constant-pool references should be valid");
    assert_eq!(class.version.major, JVM_CLASSFILE_MAJOR_VERSION);
    assert_eq!(
        class.version.major, 65,
        "Calculator targets JDK 21 class files"
    );
    assert_eq!(
        class.constant_pool.class_name(class.this_class),
        Ok("Calculator")
    );
    assert!(class.access_flags.is_public());

    let signatures = class
        .methods
        .iter()
        .map(|method| {
            method
                .descriptor(&class.constant_pool)
                .expect("method descriptor should parse");
            (
                class.constant_pool.utf8(method.name_index).unwrap(),
                class.constant_pool.utf8(method.descriptor_index).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        signatures,
        [
            ("<init>", "()V"),
            ("sumTo", "(I)I"),
            ("add", "(II)I"),
            ("subtract", "(II)I"),
            ("multiply", "(II)I"),
            ("divide", "(II)I"),
            ("remainder", "(II)I"),
            ("less", "(II)Z"),
            ("lessOrEqual", "(II)Z"),
            ("greater", "(II)Z"),
            ("greaterOrEqual", "(II)Z"),
        ]
    );

    let method_info = |name: &str| {
        class
            .methods
            .iter()
            .find(|method| class.constant_pool.utf8(method.name_index) == Ok(name))
            .unwrap_or_else(|| panic!("missing Calculator method {name}"))
    };
    for method in &class.methods {
        assert!(method.access_flags.is_public());
        assert!(
            matches!(method.attributes.as_slice(), [Attribute::Code(_)]),
            "{} has exactly one Code attribute",
            class.constant_pool.utf8(method.name_index).unwrap()
        );
    }

    let [Attribute::Code(constructor)] = method_info("<init>").attributes.as_slice() else {
        panic!("Calculator should have a public no-argument constructor")
    };
    assert_eq!(constructor.max_locals, 1);
    assert!(
        constructor.code.contains(&0xb7),
        "constructor calls Object.<init>"
    );

    let [Attribute::Code(sum_to)] = method_info("sumTo").attributes.as_slice() else {
        panic!("sumTo should have Code")
    };
    assert_eq!((sum_to.max_stack, sum_to.max_locals), (2, 4));
    let sum_to_instructions = decode_subset_instructions(sum_to.code);
    for opcode in [0x60, 0xa4, 0xa7] {
        assert!(
            sum_to_instructions
                .iter()
                .any(|(_, found)| *found == opcode),
            "sumTo is missing instruction {opcode:#04x}"
        );
    }
    let targets = decode_branch_targets(sum_to.code);
    let mut expected_frames = targets
        .iter()
        .map(|(_, _, target)| *target)
        .collect::<Vec<_>>();
    expected_frames.sort_unstable();
    expected_frames.dedup();
    assert_eq!(stack_map_frame_pcs(&class, "sumTo"), expected_frames);

    for (name, opcode) in [
        ("add", 0x60),
        ("subtract", 0x64),
        ("multiply", 0x68),
        ("divide", 0x6c),
        ("remainder", 0x70),
        ("less", 0xa1),
        ("lessOrEqual", 0xa4),
        ("greater", 0xa3),
        ("greaterOrEqual", 0xa2),
    ] {
        let [Attribute::Code(code)] = method_info(name).attributes.as_slice() else {
            panic!("{name} should have Code")
        };
        assert_eq!((code.max_stack, code.max_locals), (2, 3));
        assert!(
            decode_subset_instructions(code.code)
                .iter()
                .any(|(_, found)| *found == opcode),
            "{name} should use intrinsic opcode {opcode:#04x}"
        );
        assert_virtual_invocations(&class, code.code, &[]);
    }

    for index in 1..=u16::MAX {
        let Some(ConstantPoolEntry::Methodref {
            class_index,
            name_and_type_index,
        }) = class.constant_pool.get(ConstantPoolIndex(index))
        else {
            continue;
        };
        let owner = class
            .constant_pool
            .class_name(*class_index)
            .expect("Methodref owner");
        let (name, descriptor) = class
            .constant_pool
            .name_and_type(*name_and_type_index)
            .expect("Methodref name and descriptor");
        assert!(
            !["+", "-", "*", "/", "%", "<", "<=", ">", ">="].contains(&name),
            "primitive operator {owner}.{name}{descriptor} must not be a Methodref"
        );
    }

    if require_jdk_or_skip() {
        let output_path = first_output.path().to_str().expect("UTF-8 output path");
        let javap = Command::new("javap")
            .args(["-v", "-c", "-classpath", output_path, "Calculator"])
            .output()
            .expect("start javap");
        assert!(
            javap.status.success(),
            "javap rejected Calculator\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javap.stdout),
            String::from_utf8_lossy(&javap.stderr)
        );
        let javap_output = String::from_utf8_lossy(&javap.stdout);
        for expected in [
            "major version: 65",
            "descriptor: (I)I",
            "descriptor: (II)I",
            "descriptor: (II)Z",
            "iadd",
            "isub",
            "imul",
            "idiv",
            "irem",
            "if_icmplt",
            "if_icmple",
            "if_icmpgt",
            "if_icmpge",
            "iload_1",
            "istore_2",
            "goto",
            "StackMapTable",
        ] {
            assert!(
                javap_output.contains(expected),
                "javap output omitted {expected:?}:\n{javap_output}"
            );
        }

        let harness_path = first_output.path().join("CalculatorHarness.java");
        fs::write(
            &harness_path,
            r#"public final class CalculatorHarness {
  public static void main(String[] args) {
    Calculator value = new Calculator();
    if (value.sumTo(0) != 0 || value.sumTo(1) != 1 || value.sumTo(10) != 55 || value.sumTo(100) != 5050) throw new AssertionError("sumTo");
    if (value.multiply(6, 7) != 42) throw new AssertionError("multiply");
    if (!value.lessOrEqual(3, 3) || value.lessOrEqual(5, 2)) throw new AssertionError("lessOrEqual");
    if (value.add(Integer.MAX_VALUE, 1) != Integer.MIN_VALUE) throw new AssertionError("add overflow");
    if (value.subtract(2, 5) != -3 || value.divide(-7, 3) != -2 || value.remainder(-7, 3) != -1) throw new AssertionError("arithmetic");
    if (value.divide(Integer.MIN_VALUE, -1) != Integer.MIN_VALUE || value.remainder(Integer.MIN_VALUE, -1) != 0) throw new AssertionError("division overflow");
    try { value.divide(1, 0); throw new AssertionError("division by zero returned"); } catch (ArithmeticException expected) { }
    try { value.remainder(1, 0); throw new AssertionError("remainder by zero returned"); } catch (ArithmeticException expected) { }
    if (!value.less(-2, -1) || value.less(1, 1) || !value.less(Integer.MIN_VALUE, Integer.MAX_VALUE) || value.less(Integer.MAX_VALUE, Integer.MIN_VALUE)) throw new AssertionError("less signed boundaries");
    if (!value.lessOrEqual(5, 5) || value.lessOrEqual(6, 5) || !value.lessOrEqual(Integer.MIN_VALUE, Integer.MAX_VALUE)) throw new AssertionError("lessOrEqual signed boundaries");
    if (!value.greater(1, -1) || value.greater(2, 2) || !value.greater(Integer.MAX_VALUE, Integer.MIN_VALUE) || value.greater(Integer.MIN_VALUE, Integer.MAX_VALUE)) throw new AssertionError("greater signed boundaries");
    if (!value.greaterOrEqual(2, 2) || value.greaterOrEqual(-2, -1) || !value.greaterOrEqual(Integer.MAX_VALUE, Integer.MIN_VALUE)) throw new AssertionError("greaterOrEqual signed boundaries");
    System.out.println("calculator intrinsics ok");
  }
}
"#,
        )
        .expect("write Calculator Java harness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness_path)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected CalculatorHarness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "CalculatorHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected Calculator\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "calculator intrinsics ok"
        );
    }
}

#[test]
fn invalid_calculator_sources_fail_at_the_typed_pipeline_stage_without_artifacts() {
    for fixture in [
        "InvalidCalculatorOperand.scala",
        "InvalidCalculatorComparison.scala",
    ] {
        let output = TestOutputDirectory::new(fixture);
        let error = compile_fixture(fixture, output.path())
            .expect_err("ill-typed calculator fixture should fail");
        assert!(
            matches!(error, PipelineFailure::Typer(_)),
            "{fixture} failed at unexpected pipeline stage: {error:?}"
        );
        assert!(
            output.class_files().is_empty(),
            "{fixture} must not leave a partial class file"
        );
    }

    let output = TestOutputDirectory::new("user-defined-plus");
    let error = compile_fixture("UserDefinedPlus.scala", output.path())
        .expect_err("user-defined operator name is outside the JVM subset");
    assert!(
        matches!(
            &error,
            PipelineFailure::Codegen(JvmCodegenError::InvalidJvmName {
                kind: JvmNameKind::Method,
                text,
                ..
            }) if text == "+"
        ),
        "user-defined operator should fail as a structured codegen error: {error:?}"
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn canonical_int_arithmetic_and_comparisons_lower_to_ordered_jvm_instructions() {
    let output = TestOutputDirectory::new("int-operator-intrinsics");
    let source = r#"class IntegerOps {
          def add(a: Int, b: Int): Int = a + b
          def subtract(a: Int, b: Int): Int = a - b
          def multiply(a: Int, b: Int): Int = a * b
          def divide(a: Int, b: Int): Int = a / b
          def remainder(a: Int, b: Int): Int = a % b
          def left(): Int = 6
          def right(): Int = 2
          def ordered(): Int = left() + right()
          def nested(x: Int, y: Int): Int = (x + y) * 2
          def localState(n: Int): Int = { var total = n + 1; total = total + n; total }
          def branch(x: Int): Int = if true then x + 1 else x * 2
          def loop(): Int = { var total = 0; while false do { total = total + 1 }; total }
          def less(a: Int, b: Int): Boolean = a < b
          def lessOrEqual(a: Int, b: Int): Boolean = a <= b
          def greater(a: Int, b: Int): Boolean = a > b
          def greaterOrEqual(a: Int, b: Int): Boolean = a >= b
          def savedFlag(a: Int, b: Int): Boolean = { val flag: Boolean = a < b; flag }
          def comparisonIf(a: Int, b: Int): Int = if a < b then 1 else 0
          def nestedComparison(a: Int, b: Int): Int = if a < b then { if a <= b then 1 else 2 } else { if a >= b then 3 else 4 }
          def comparisonLoop(limit: Int): Int = { var i = 0; while i <= limit do { i = i + 1 }; i }
        }
"#;
    let artifact = compile_source(source, "IntegerOps.scala", output.path()).unwrap();
    let second_output = TestOutputDirectory::new("int-operator-intrinsics-second");
    let second_artifact = compile_source(source, "IntegerOps.scala", second_output.path()).unwrap();
    assert_eq!(artifact.class.bytes(), second_artifact.class.bytes());
    let mut reader = Reader::new(artifact.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("IntegerOps should decode");
    class
        .validate_references()
        .expect("IntegerOps constant-pool references should be valid");

    for (name, opcode) in [
        ("add", 0x60),
        ("subtract", 0x64),
        ("multiply", 0x68),
        ("divide", 0x6c),
        ("remainder", 0x70),
    ] {
        assert_opcode_sequence(
            emitted_method_code(&class, name),
            &[0x1b, 0x1c, opcode, 0xac],
        );
    }

    let ordered = emitted_method_code(&class, "ordered");
    assert_opcode_sequence(ordered, &[0x2a, 0xb6, 0x2a, 0xb6, 0x60, 0xac]);
    assert_invoked_method_names(&class, ordered, &["left", "right"]);

    let nested = emitted_method_code(&class, "nested");
    assert_opcode_sequence(nested, &[0x1b, 0x1c, 0x60, 0x05, 0x68, 0xac]);

    let local_state = emitted_method_code(&class, "localState");
    assert_opcode_sequence(
        local_state,
        &[0x1b, 0x04, 0x60, 0x3d, 0x1c, 0x1b, 0x60, 0x3d, 0x1c, 0xac],
    );

    let branch = decode_subset_instructions(emitted_method_code(&class, "branch"));
    let branch_opcodes = branch.iter().map(|(_, opcode)| *opcode).collect::<Vec<_>>();
    assert!(branch_opcodes.contains(&0x60), "then branch lowers iadd");
    assert!(branch_opcodes.contains(&0x68), "else branch lowers imul");
    assert!(branch_opcodes.contains(&0x99), "if branch remains in CFG");

    let loop_body = decode_subset_instructions(emitted_method_code(&class, "loop"));
    let loop_opcodes = loop_body
        .iter()
        .map(|(_, opcode)| *opcode)
        .collect::<Vec<_>>();
    assert!(loop_opcodes.contains(&0x60), "loop body lowers iadd");
    assert!(loop_opcodes.contains(&0x99), "loop exit remains in CFG");
    assert!(
        loop_opcodes.contains(&0xa7),
        "loop back-edge remains in CFG"
    );

    for (name, opcode) in [
        ("less", 0xa1),
        ("lessOrEqual", 0xa4),
        ("greater", 0xa3),
        ("greaterOrEqual", 0xa2),
    ] {
        let code = emitted_method_code(&class, name);
        assert_opcode_sequence(code, &[0x1b, 0x1c, opcode, 0x03, 0xa7, 0x04, 0xac]);
        assert!(
            has_stack_map_table(&class, name),
            "{name} should have generated join frames"
        );
        let branch_targets = decode_branch_targets(code)
            .into_iter()
            .map(|(_, _, target)| target)
            .collect::<Vec<_>>();
        let mut expected_frame_pcs = branch_targets.clone();
        expected_frame_pcs.sort_unstable();
        expected_frame_pcs.dedup();
        assert_eq!(stack_map_frame_pcs(&class, name), expected_frame_pcs);
    }
    assert_opcode_sequence(
        emitted_method_code(&class, "savedFlag"),
        &[0x1b, 0x1c, 0xa1, 0x03, 0xa7, 0x04, 0x3e, 0x1d, 0xac],
    );
    let if_opcodes = decode_subset_instructions(emitted_method_code(&class, "comparisonIf"));
    assert!(
        if_opcodes.iter().any(|(_, opcode)| *opcode == 0xa1),
        "if condition lowers the comparison branch"
    );
    let nested_opcodes =
        decode_subset_instructions(emitted_method_code(&class, "nestedComparison"));
    assert_eq!(
        nested_opcodes
            .iter()
            .filter(|(_, opcode)| (0xa1..=0xa4).contains(opcode))
            .count(),
        3,
        "nested If expressions retain all signed comparison branches"
    );
    let comparison_loop = decode_subset_instructions(emitted_method_code(&class, "comparisonLoop"));
    assert!(
        comparison_loop.iter().any(|(_, opcode)| *opcode == 0xa4),
        "while condition lowers the <= comparison"
    );
    assert!(
        comparison_loop.iter().any(|(_, opcode)| *opcode == 0xa7),
        "comparison loop retains its back-edge"
    );

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 output path");
        let harness_path = output.path().join("IntegerOpsHarness.java");
        fs::write(
            &harness_path,
            "public final class IntegerOpsHarness { public static void main(String[] args) { IntegerOps value = new IntegerOps(); if (value.add(2, 5) != 7 || value.subtract(2, 5) != -3 || value.multiply(-7, 3) != -21) throw new AssertionError(\"basic arithmetic\"); if (value.divide(-7, 3) != -2 || value.divide(Integer.MIN_VALUE, -1) != Integer.MIN_VALUE) throw new AssertionError(\"division semantics\"); if (value.remainder(-7, 3) != -1 || value.remainder(7, -3) != 1 || value.remainder(Integer.MIN_VALUE, -1) != 0) throw new AssertionError(\"remainder semantics\"); if (value.add(Integer.MAX_VALUE, 1) != Integer.MIN_VALUE) throw new AssertionError(\"addition overflow\"); if (value.ordered() != 8 || value.nested(3, 4) != 14 || value.localState(5) != 11 || value.branch(3) != 4 || value.loop() != 0) throw new AssertionError(\"arithmetic lowering\"); if (!value.less(-2, -1) || value.less(1, 1) || !value.less(Integer.MIN_VALUE, Integer.MAX_VALUE) || value.less(Integer.MAX_VALUE, Integer.MIN_VALUE)) throw new AssertionError(\"less signed boundaries\"); if (!value.lessOrEqual(5, 5) || value.lessOrEqual(6, 5) || !value.lessOrEqual(Integer.MIN_VALUE, Integer.MAX_VALUE)) throw new AssertionError(\"less-or-equal\"); if (!value.greater(1, -1) || value.greater(2, 2) || !value.greater(Integer.MAX_VALUE, Integer.MIN_VALUE)) throw new AssertionError(\"greater signed boundaries\"); if (!value.greaterOrEqual(2, 2) || value.greaterOrEqual(-2, -1) || !value.greaterOrEqual(Integer.MAX_VALUE, Integer.MIN_VALUE)) throw new AssertionError(\"greater-or-equal\"); if (!value.savedFlag(-1, 0)) throw new AssertionError(\"boolean local value\"); if (value.comparisonIf(1, 2) != 1 || value.comparisonIf(3, 2) != 0 || value.nestedComparison(1, 1) != 3 || value.nestedComparison(1, 2) != 1 || value.nestedComparison(3, 2) != 3 || value.comparisonLoop(3) != 4) throw new AssertionError(\"comparison control flow\"); try { value.divide(1, 0); throw new AssertionError(\"division by zero returned\"); } catch (ArithmeticException expected) { } try { value.remainder(1, 0); throw new AssertionError(\"remainder by zero returned\"); } catch (ArithmeticException expected) { } System.out.println(\"integer intrinsics ok\"); } }\n",
        )
        .expect("write IntegerOpsHarness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness_path)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected IntegerOpsHarness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "IntegerOpsHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected IntegerOps\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "integer intrinsics ok"
        );
    }
}

#[test]
fn int_equality_types_but_remains_outside_the_relational_intrinsic_subset() {
    assert_source_failure(
        "int-equality-not-lowered",
        "class UnsupportedEquality { def equal(a: Int, b: Int): Boolean = a == b }\n",
        |failure| {
            matches!(
                failure,
                PipelineFailure::Codegen(JvmCodegenError::IntIntrinsicResolution { .. })
            )
        },
    );
}

#[test]
fn canonical_int_intrinsics_compose_across_loop_and_branch_control_flow() {
    let output = TestOutputDirectory::new("compositional-int-intrinsics");
    let source = r#"class IntrinsicControlFlow {
              def sumTo(n: Int): Int = {
                var i = 1
                var sum = 0
                while i <= n do {
                  sum = sum + i
                  i = i + 1
                }
                sum
              }
              def select(a: Int, b: Int): Int = if a < b then a + b else a - b
              def operandOrder(flag: Boolean): Int = {
                var i = 1
                val value = if flag then i + { i = i + 1; i } else i
                value * 10 + i
              }
              def nestedComparison(a: Int, b: Int): Boolean =
                if a < b then a <= b else a >= b
              def localAfterBranch(flag: Boolean): Int = {
                val selected = if flag then 5 else 7
                selected
              }
            }
"#;
    let artifact = compile_source(source, "IntrinsicControlFlow.scala", output.path()).unwrap();
    let second_output = TestOutputDirectory::new("compositional-int-intrinsics-second");
    let second_artifact =
        compile_source(source, "IntrinsicControlFlow.scala", second_output.path()).unwrap();
    assert_eq!(artifact.class.bytes(), second_artifact.class.bytes());
    let mut reader = Reader::new(artifact.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("IntrinsicControlFlow should decode");

    for name in [
        "sumTo",
        "select",
        "operandOrder",
        "nestedComparison",
        "localAfterBranch",
    ] {
        let code = emitted_method_code(&class, name);
        let mut expected_frame_pcs = decode_branch_targets(code)
            .into_iter()
            .map(|(_, _, target)| target)
            .collect::<Vec<_>>();
        expected_frame_pcs.sort_unstable();
        expected_frame_pcs.dedup();
        assert_eq!(
            stack_map_frame_pcs(&class, name),
            expected_frame_pcs,
            "{name} should use the classfile CFG's frame targets"
        );
        assert!(
            !decode_subset_instructions(code)
                .iter()
                .any(|(_, opcode)| *opcode == 0xb6),
            "{name} should lower canonical operators without invokevirtual"
        );
    }
    let sum_to = decode_subset_instructions(emitted_method_code(&class, "sumTo"));
    for opcode in [0x60, 0xa4, 0xa7] {
        assert!(
            sum_to.iter().any(|(_, found)| *found == opcode),
            "sumTo should contain the expected intrinsic/loop opcode {opcode:#04x}"
        );
    }
    assert!(
        decode_subset_instructions(emitted_method_code(&class, "select"))
            .iter()
            .any(|(_, opcode)| *opcode == 0x60),
        "If branches should lower nested arithmetic intrinsics"
    );

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 output path");
        let harness_path = output.path().join("IntrinsicControlFlowHarness.java");
        fs::write(
            &harness_path,
            "public final class IntrinsicControlFlowHarness { public static void main(String[] args) { IntrinsicControlFlow value = new IntrinsicControlFlow(); if (value.sumTo(0) != 0 || value.sumTo(1) != 1 || value.sumTo(10) != 55) throw new AssertionError(\"multi-iteration sumTo\"); if (value.select(2, 5) != 7 || value.select(5, 2) != 3) throw new AssertionError(\"comparison-selected arithmetic\"); if (value.operandOrder(true) != 32 || value.operandOrder(false) != 11) throw new AssertionError(\"intrinsic operand order and single evaluation\"); if (!value.nestedComparison(1, 2) || !value.nestedComparison(2, 1) || !value.nestedComparison(2, 2)) throw new AssertionError(\"nested Boolean comparison\"); if (value.localAfterBranch(true) != 5 || value.localAfterBranch(false) != 7) throw new AssertionError(\"local after branch join\"); System.out.println(\"compositional intrinsics ok\"); } }\n",
        )
        .expect("write compositional intrinsic harness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness_path)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected IntrinsicControlFlowHarness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "IntrinsicControlFlowHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected IntrinsicControlFlow\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "compositional intrinsics ok"
        );
    }
}

#[test]
fn canonical_boolean_intrinsics_compose_with_source_control_flow_and_locals() {
    let output = TestOutputDirectory::new("compositional-boolean-intrinsics");
    let source = r#"class BooleanControlFlow {
              def whileAnd(n: Int): Int = {
                var i = 0
                var running = true
                while i < n && running do {
                  i = i + 1
                  if i >= 2 then running = false else ()
                }
                i
              }
              def select(flag: Boolean, i: Int): Int =
                if !flag || i >= 10 then 1 else 0
              def composed(a: Boolean, b: Boolean, c: Boolean): Boolean = {
                val result = (a && b) || c
                result
              }
              def mixedAnd(a: Boolean, b: Boolean, c: Boolean): Boolean = (a & b) || c
              def mixedOr(a: Boolean, b: Boolean, c: Boolean): Boolean = a && (b | c)
              def toggle(flag: Boolean): Boolean = {
                var current = flag
                current = !current
                current
              }
              def andHits(flag: Boolean): Int = {
                var hits = 0
                val result = flag && { hits = hits + 1; true }
                hits
              }
              def orHits(flag: Boolean): Int = {
                var hits = 0
                val result = flag || { hits = hits + 1; false }
                hits
              }
              def shortCircuitOrder(a: Boolean, b: Boolean): Int = {
                var order = 0
                val result = { order = order * 10 + 1; a } && { order = order * 10 + 2; b }
                order
              }
              def strictAndHits(a: Boolean, b: Boolean): Int = {
                var hits = 0
                val result = ({ hits = hits + 1; a } & { hits = hits + 1; b })
                hits
              }
              def strictOrHits(a: Boolean, b: Boolean): Int = {
                var hits = 0
                val result = ({ hits = hits + 1; a } | { hits = hits + 1; b })
                hits
              }
              def strictXorHits(a: Boolean, b: Boolean): Int = {
                var hits = 0
                val result = ({ hits = hits + 1; a } ^ { hits = hits + 1; b })
                hits
              }
            }
"#;
    let artifact = compile_source(source, "BooleanControlFlow.scala", output.path()).unwrap();
    let mut reader = Reader::new(artifact.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("BooleanControlFlow should decode");
    for name in [
        "whileAnd",
        "select",
        "composed",
        "mixedAnd",
        "mixedOr",
        "toggle",
        "andHits",
        "orHits",
        "shortCircuitOrder",
        "strictAndHits",
        "strictOrHits",
        "strictXorHits",
    ] {
        let code = emitted_method_code(&class, name);
        assert!(
            !decode_subset_instructions(code)
                .iter()
                .any(|(_, opcode)| *opcode == 0xb6),
            "{name} should lower canonical Boolean operators without invokevirtual"
        );
        let mut expected_frame_pcs = decode_branch_targets(code)
            .into_iter()
            .map(|(_, _, target)| target)
            .collect::<Vec<_>>();
        expected_frame_pcs.sort_unstable();
        expected_frame_pcs.dedup();
        if [
            "whileAnd",
            "select",
            "composed",
            "mixedAnd",
            "mixedOr",
            "andHits",
            "orHits",
            "shortCircuitOrder",
        ]
        .contains(&name)
        {
            assert_eq!(
                stack_map_frame_pcs(&class, name),
                expected_frame_pcs,
                "{name} should use the classfile CFG's frame targets"
            );
        }
    }
    assert!(
        decode_subset_instructions(emitted_method_code(&class, "whileAnd"))
            .iter()
            .any(|(_, opcode)| *opcode == 0xa7),
        "whileAnd should retain a symbolic loop back-edge"
    );

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 output path");
        let harness_path = output.path().join("BooleanControlFlowHarness.java");
        fs::write(
            &harness_path,
            r#"public final class BooleanControlFlowHarness {
  public static void main(String[] args) {
    BooleanControlFlow v = new BooleanControlFlow();
    if (v.whileAnd(0) != 0 || v.whileAnd(1) != 1 || v.whileAnd(10) != 2)
      throw new AssertionError("while && zero/multiple iterations");
    for (boolean f : new boolean[] {false, true}) {
      for (int i : new int[] {9, 10}) {
        if (v.select(f, i) != ((!f || i >= 10) ? 1 : 0))
          throw new AssertionError("if !flag || comparison");
      }
    }
    for (boolean a : new boolean[] {false, true}) {
      for (boolean b : new boolean[] {false, true}) {
        for (boolean c : new boolean[] {false, true}) {
          if (v.composed(a, b, c) != ((a && b) || c))
            throw new AssertionError("composed");
          if (v.mixedAnd(a, b, c) != ((a & b) || c))
            throw new AssertionError("mixed eager and short circuit");
          if (v.mixedOr(a, b, c) != (a && (b | c)))
            throw new AssertionError("mixed short circuit and eager");
        }
      }
    }
    if (!v.toggle(false) || v.toggle(true))
      throw new AssertionError("Boolean local assignment");
    if (v.andHits(false) != 0 || v.andHits(true) != 1
        || v.orHits(true) != 0 || v.orHits(false) != 1)
      throw new AssertionError("short-circuit RHS effects");
    if (v.shortCircuitOrder(false, true) != 1
        || v.shortCircuitOrder(true, false) != 12
        || v.shortCircuitOrder(true, true) != 12)
      throw new AssertionError("short-circuit evaluation order and single evaluation");
    if (v.strictAndHits(false, false) != 2
        || v.strictOrHits(true, true) != 2
        || v.strictXorHits(false, true) != 2)
      throw new AssertionError("strict Boolean RHS effects");
    System.out.println("Boolean control flow ok");
  }
}
"#,
        )
        .expect("write Boolean control-flow harness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness_path)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected BooleanControlFlowHarness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "BooleanControlFlowHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected BooleanControlFlow\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "Boolean control flow ok"
        );
    }
}

#[test]
fn logic_fixture_compiles_deterministically_and_executes_boolean_semantics() {
    let first_output = TestOutputDirectory::new("logic-first");
    let second_output = TestOutputDirectory::new("logic-second");
    let first = compile_fixture("Logic.scala", first_output.path()).unwrap();
    let second = compile_fixture("Logic.scala", second_output.path()).unwrap();

    assert_eq!(first.class.internal_name(), "Logic");
    assert_eq!(first.class.bytes(), second.class.bytes());
    assert_eq!(first_output.class_files(), vec![first.path.clone()]);
    assert_eq!(second_output.class_files(), vec![second.path.clone()]);

    let mut reader = Reader::new(first.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("Logic class should decode completely");
    assert!(
        reader.is_at_end(),
        "decoder consumes the complete class file"
    );
    class
        .validate_references()
        .expect("Logic constant-pool references should all be valid");
    assert_eq!(class.version.major, JVM_CLASSFILE_MAJOR_VERSION);
    assert_eq!(class.version.major, 65, "Logic targets JDK 21 class files");
    assert_eq!(
        class.constant_pool.class_name(class.this_class),
        Ok("Logic")
    );
    assert!(class.access_flags.is_public());

    let signatures = class
        .methods
        .iter()
        .map(|method| {
            method
                .descriptor(&class.constant_pool)
                .expect("method descriptor should parse");
            (
                class.constant_pool.utf8(method.name_index).unwrap(),
                class.constant_pool.utf8(method.descriptor_index).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        signatures,
        [
            ("<init>", "()V"),
            ("both", "(ZZ)Z"),
            ("either", "(ZZ)Z"),
            ("negate", "(Z)Z"),
            ("exclusive", "(ZZ)Z"),
            ("equal", "(ZZ)Z"),
            ("notEqual", "(ZZ)Z"),
            ("skipRight", "()Z"),
            ("skipOrRight", "()Z"),
            ("runOrRight", "()Z"),
            ("runAndRight", "()Z"),
            ("eagerAndRight", "()Z"),
            ("eagerOrRight", "()Z"),
            ("eagerXorRight", "()Z"),
            ("countRight", "(Z)I"),
            ("countStrictAnd", "(ZZ)I"),
            ("countStrictOr", "(ZZ)I"),
            ("countStrictXor", "(ZZ)I"),
        ]
    );
    for method in &class.methods {
        assert!(method.access_flags.is_public());
        assert!(!method.access_flags.is_static());
        assert!(matches!(method.attributes.as_slice(), [Attribute::Code(_)]));
    }
    for (name, max_stack, max_locals) in [
        ("both", 1, 3),
        ("either", 1, 3),
        ("negate", 2, 2),
        ("exclusive", 2, 3),
        ("countRight", 2, 4),
    ] {
        let method = class
            .methods
            .iter()
            .find(|method| class.constant_pool.utf8(method.name_index) == Ok(name))
            .unwrap_or_else(|| panic!("missing Logic method {name}"));
        let [Attribute::Code(code)] = method.attributes.as_slice() else {
            panic!("Logic.{name} should have one Code attribute")
        };
        assert_eq!(
            (code.max_stack, code.max_locals),
            (max_stack, max_locals),
            "{name}"
        );
    }

    for (name, descriptor) in [
        ("both", "(ZZ)Z"),
        ("either", "(ZZ)Z"),
        ("negate", "(Z)Z"),
        ("exclusive", "(ZZ)Z"),
        ("skipRight", "()Z"),
        ("countRight", "(Z)I"),
    ] {
        assert!(
            signatures.contains(&(name, descriptor)),
            "Logic is missing {name}{descriptor}"
        );
    }

    let expected_opcode = [
        ("both", 0x99),
        ("either", 0x9a),
        ("negate", 0x82),
        ("exclusive", 0x82),
        ("equal", 0x9f),
        ("notEqual", 0xa0),
        ("countStrictAnd", 0x7e),
        ("countStrictOr", 0x80),
        ("countStrictXor", 0x82),
    ];
    for (name, opcode) in expected_opcode {
        assert!(
            decode_subset_instructions(emitted_method_code(&class, name))
                .iter()
                .any(|(_, found)| *found == opcode),
            "{name} should contain expected JVM opcode {opcode:#04x}"
        );
    }
    for method in &class.methods {
        let name = class.constant_pool.utf8(method.name_index).unwrap();
        let code = emitted_method_code(&class, name);
        let mut expected_frames = decode_branch_targets(code)
            .into_iter()
            .map(|(_, _, target)| target)
            .collect::<Vec<_>>();
        expected_frames.sort_unstable();
        expected_frames.dedup();
        if !expected_frames.is_empty() {
            assert_eq!(
                stack_map_frame_pcs(&class, name),
                expected_frames,
                "{name} StackMapTable should cover each symbolic branch target"
            );
        }
    }

    for index in 1..=u16::MAX {
        let Some(ConstantPoolEntry::Methodref {
            name_and_type_index,
            ..
        }) = class.constant_pool.get(ConstantPoolIndex(index))
        else {
            continue;
        };
        let (name, _) = class
            .constant_pool
            .name_and_type(*name_and_type_index)
            .expect("Methodref name and descriptor");
        assert!(
            !["&&", "||", "!", "&", "|", "^", "==", "!="].contains(&name),
            "canonical Boolean intrinsic {name} must not be a Methodref"
        );
    }

    if require_jdk_or_skip() {
        let output_path = first_output.path().to_str().expect("UTF-8 output path");
        let javap = Command::new("javap")
            .args(["-c", "-v", "-classpath", output_path, "Logic"])
            .output()
            .expect("start javap");
        assert!(
            javap.status.success(),
            "javap rejected Logic\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javap.stdout),
            String::from_utf8_lossy(&javap.stderr)
        );
        let javap_output = String::from_utf8_lossy(&javap.stdout);
        for expected in [
            "major version: 65",
            "descriptor: (ZZ)Z",
            "descriptor: (Z)Z",
            "descriptor: ()Z",
            "descriptor: (Z)I",
            "ifeq",
            "ifne",
            "goto",
            "ixor",
            "iand",
            "ior",
            "stack=1, locals=3",
            "stack=2, locals=4",
            "StackMapTable",
        ] {
            assert!(
                javap_output.contains(expected),
                "javap output omitted {expected:?}:\n{javap_output}"
            );
        }
        let harness_path = first_output.path().join("LogicHarness.java");
        fs::write(
            &harness_path,
            r#"public final class LogicHarness {
  private static void expectArithmetic(Runnable action, String name) {
    try {
      action.run();
      throw new AssertionError(name + " did not evaluate its RHS");
    } catch (ArithmeticException expected) { }
  }

  public static void main(String[] args) {
    Logic logic = new Logic();
    if (logic.both(true, false) || !logic.both(true, true)) throw new AssertionError("&& truth table");
    if (!logic.either(false, true) || logic.either(false, false)) throw new AssertionError("|| truth table");
    if (logic.negate(true) || !logic.negate(false)) throw new AssertionError("! truth table");
    if (logic.exclusive(true, true) || !logic.exclusive(true, false)) throw new AssertionError("^ truth table");
    for (boolean a : new boolean[] {false, true}) {
      for (boolean b : new boolean[] {false, true}) {
        if (logic.equal(a, b) != (a == b)) throw new AssertionError("== truth table");
        if (logic.notEqual(a, b) != (a != b)) throw new AssertionError("!= truth table");
      }
    }
    if (logic.skipRight() || !logic.skipOrRight()) throw new AssertionError("short-circuit divide-by-zero");
    if (logic.countRight(false) != 0 || logic.countRight(true) != 1) throw new AssertionError("RHS side effect count");
    if (logic.countStrictAnd(false, true) != 2 || logic.countStrictOr(false, true) != 2
        || logic.countStrictXor(false, true) != 2) throw new AssertionError("strict operand side effects");
    expectArithmetic(() -> logic.runOrRight(), "false || rhs");
    expectArithmetic(() -> logic.runAndRight(), "true && rhs");
    expectArithmetic(() -> logic.eagerAndRight(), "false & rhs");
    expectArithmetic(() -> logic.eagerOrRight(), "true | rhs");
    expectArithmetic(() -> logic.eagerXorRight(), "true ^ rhs");
    System.out.println("Logic Boolean operators ok");
  }
}
"#,
        )
        .expect("write Logic Java harness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness_path)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected LogicHarness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args(["-Xverify:all", "-classpath", output_path, "LogicHarness"])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected Logic\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "Logic Boolean operators ok"
        );
    }
}

#[test]
fn boolean_source_errors_and_user_defined_operators_keep_typed_failures() {
    assert_source_failure(
        "boolean-and-wrong-rhs",
        "class WrongBooleanRhs { def run(flag: Boolean): Boolean = flag && 42 }\n",
        |failure| {
            matches!(
                failure,
                PipelineFailure::Typer(TyperError::OverloadApplicationNoApplicable { .. })
            )
        },
    );
    assert_source_failure(
        "boolean-not-on-int",
        "class BooleanNotOnInt { def run(): Boolean = !1 }\n",
        |failure| {
            matches!(
                failure,
                PipelineFailure::Typer(TyperError::MemberNotFound { .. })
            )
        },
    );
    let output = TestOutputDirectory::new("user-defined-boolean-and");
    let error = compile_fixture("UserDefinedAnd.scala", output.path())
        .expect_err("same-spelled user Boolean operator is outside the classfile subset");
    assert!(
        matches!(
            &error,
            PipelineFailure::Codegen(JvmCodegenError::InvalidJvmName {
                kind: JvmNameKind::Method,
                text,
                ..
            }) if text == "&&"
        ),
        "user-defined && must remain an ordinary method: {error:?}"
    );
    assert!(
        output.class_files().is_empty(),
        "unsupported user-defined operator must not leave a partial class"
    );
}

fn assert_invoked_method_names(class: &ClassFile<'_>, code: &[u8], expected: &[&str]) {
    let instructions = decode_subset_instructions(code);
    let calls = instructions
        .iter()
        .filter_map(|(offset, opcode)| (*opcode == 0xb6).then_some(*offset));
    let names = calls
        .map(|offset| {
            let index = u16::from_be_bytes([code[offset + 1], code[offset + 2]]);
            let ConstantPoolEntry::Methodref {
                name_and_type_index,
                ..
            } = class
                .constant_pool
                .get(ConstantPoolIndex(index))
                .expect("invokevirtual constant-pool entry")
            else {
                panic!("invokevirtual must reference Methodref")
            };
            class
                .constant_pool
                .name_and_type(*name_and_type_index)
                .expect("invokevirtual method name")
                .0
        })
        .collect::<Vec<_>>();
    assert_eq!(names, expected);
}

fn emitted_method_code<'a>(class: &ClassFile<'a>, name: &str) -> &'a [u8] {
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index) == Ok(name))
        .unwrap_or_else(|| panic!("missing method {name}"));
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!("{name} should have one Code attribute");
    };
    code.code
}

fn assert_emitted_getter(
    class: &ClassFile<'_>,
    method_name: &str,
    method_descriptor: &str,
    field_name: &str,
    field_descriptor: &str,
    return_opcode: u8,
    max_stack: u16,
) {
    let methods = class
        .methods
        .iter()
        .filter(|method| {
            class.constant_pool.utf8(method.name_index) == Ok(method_name)
                && class.constant_pool.utf8(method.descriptor_index) == Ok(method_descriptor)
        })
        .collect::<Vec<_>>();
    let [method] = methods.as_slice() else {
        panic!("expected exactly one {method_name}{method_descriptor} getter");
    };
    assert!(method.access_flags.is_public());
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!("getter {method_name} has one Code attribute");
    };
    assert_eq!(code.code.len(), 5);
    assert_eq!(code.code[0], 0x2a, "getter starts with aload_0");
    assert_eq!(code.code[1], 0xb4, "getter reads private backing field");
    assert_eq!(code.code[4], return_opcode);
    assert_eq!(code.max_stack, max_stack);
    assert_eq!(code.max_locals, 1);
    assert_field_reference_sequence(
        class,
        code.code,
        &[(
            0xb4,
            class.constant_pool.class_name(class.this_class).unwrap(),
            field_name,
            field_descriptor,
        )],
    );
}

fn assert_emitted_setter(
    class: &ClassFile<'_>,
    method_name: &str,
    method_descriptor: &str,
    backing_field: (&str, &str),
    load_opcode: u8,
    stack_and_locals: (u16, u16),
) {
    let methods = class
        .methods
        .iter()
        .filter(|method| {
            class.constant_pool.utf8(method.name_index) == Ok(method_name)
                && class.constant_pool.utf8(method.descriptor_index) == Ok(method_descriptor)
        })
        .collect::<Vec<_>>();
    let [method] = methods.as_slice() else {
        panic!("expected exactly one {method_name}{method_descriptor} setter");
    };
    assert!(method.access_flags.is_public());
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!("setter {method_name} has one Code attribute");
    };
    assert_eq!(code.code.len(), 6);
    assert_eq!(code.code[0], 0x2a, "setter starts with aload_0");
    assert_eq!(
        code.code[1], load_opcode,
        "setter loads its exact parameter"
    );
    assert_eq!(
        code.code[2], 0xb5,
        "setter writes the private backing field"
    );
    assert_eq!(code.code[5], 0xb1, "setter returns JVM void");
    assert_eq!(code.max_stack, stack_and_locals.0);
    assert_eq!(code.max_locals, stack_and_locals.1);
    assert_field_reference_sequence(
        class,
        code.code,
        &[(
            0xb5,
            class.constant_pool.class_name(class.this_class).unwrap(),
            backing_field.0,
            backing_field.1,
        )],
    );
}

fn code_max_locals(class: &ClassFile<'_>, name: &str) -> u16 {
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index) == Ok(name))
        .unwrap_or_else(|| panic!("missing method {name}"));
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!("{name} should have one Code attribute");
    };
    code.max_locals
}

fn code_max_stack(class: &ClassFile<'_>, name: &str) -> u16 {
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index) == Ok(name))
        .unwrap_or_else(|| panic!("missing method {name}"));
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!("{name} should have one Code attribute");
    };
    code.max_stack
}

fn assert_reference_local_join_frame(
    class: &ClassFile<'_>,
    method: &str,
    local_slot: usize,
    loop_header: bool,
) {
    let code = emitted_method_code(class, method);
    let frames = stack_map_frames(class, method);
    let targets = decode_branch_targets(code);
    if loop_header {
        let header = targets
            .iter()
            .filter(|(offset, _, target)| target < offset)
            .map(|(_, _, target)| *target)
            .find(|target| {
                targets
                    .iter()
                    .filter(|(_, _, candidate)| candidate == target)
                    .count()
                    >= 2
            })
            .unwrap_or_else(|| panic!("{method} has a loop header with a back-edge"));
        let frame = frames
            .iter()
            .find(|frame| frame.pc == header)
            .unwrap_or_else(|| panic!("{method} has no frame at loop header {header}"));
        assert_eq!(
            frame.locals.get(local_slot),
            Some(&StackMapVerificationType::Object("Base".to_owned()))
        );
        assert_eq!(
            frame.locals.get(local_slot + 1),
            Some(&StackMapVerificationType::Integer),
            "the primitive loop index keeps its neighboring slot"
        );
        let exit = targets
            .iter()
            .find_map(|(_, opcode, target)| (*opcode == 0x99).then_some(*target))
            .expect("the while condition has a false exit edge");
        assert!(
            frames.iter().any(|frame| {
                frame.pc == exit
                    && frame.locals.get(local_slot)
                        == Some(&StackMapVerificationType::Object("Base".to_owned()))
            }),
            "the zero-iteration exit keeps the initialized Base local"
        );
    } else {
        assert!(
            targets.iter().any(|(_, _, target)| {
                frames.iter().any(|frame| {
                    frame.pc == *target
                        && frame.locals.get(local_slot)
                            == Some(&StackMapVerificationType::Object("Base".to_owned()))
                })
            }),
            "{method} has a CP-resolved Object(Base) local at a branch target"
        );
    }
}

fn assert_reference_stack_join_frame(class: &ClassFile<'_>, method: &str) {
    let code = emitted_method_code(class, method);
    let frames = stack_map_frames(class, method);
    let targets = decode_branch_targets(code);
    assert!(
        targets.iter().any(|(_, _, target)| {
            frames.iter().any(|frame| {
                frame.pc == *target
                    && frame.stack == [StackMapVerificationType::Object("Base".to_owned())]
            })
        }),
        "{method} has a CP-resolved Object(Base) operand-stack join at a branch target"
    );
}

fn has_stack_map_table(class: &ClassFile<'_>, method_name: &str) -> bool {
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index) == Ok(method_name))
        .unwrap_or_else(|| panic!("missing method {method_name}"));
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!("{method_name} should have one Code attribute");
    };
    code.attributes.iter().any(|attribute| {
        matches!(attribute, Attribute::Other(raw) if class.constant_pool.utf8(raw.name_index) == Ok("StackMapTable"))
    })
}

fn stack_map_frame_pcs(class: &ClassFile<'_>, method_name: &str) -> Vec<usize> {
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index) == Ok(method_name))
        .unwrap_or_else(|| panic!("missing method {method_name}"));
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!("{method_name} should have one Code attribute");
    };
    let [Attribute::Other(stack_map)] = code
        .attributes
        .iter()
        .filter(|attribute| {
            matches!(attribute, Attribute::Other(raw) if class.constant_pool.utf8(raw.name_index) == Ok("StackMapTable"))
        })
        .collect::<Vec<_>>()[..]
    else {
        panic!("{method_name} should have one StackMapTable");
    };
    decode_full_frame_pcs(stack_map.bytes)
}

fn assert_virtual_invocations(
    class: &ClassFile<'_>,
    code: &[u8],
    expected: &[(&str, &str)],
) -> Vec<usize> {
    let instructions = decode_subset_instructions(code);
    let calls = instructions
        .iter()
        .filter_map(|(offset, opcode)| (*opcode == 0xb6).then_some(*offset))
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), expected.len());
    for (offset, (expected_name, expected_descriptor)) in calls.iter().zip(expected) {
        let index = u16::from_be_bytes([code[offset + 1], code[offset + 2]]);
        let ConstantPoolEntry::Methodref {
            class_index,
            name_and_type_index,
        } = class
            .constant_pool
            .get(ConstantPoolIndex(index))
            .expect("invokevirtual constant-pool entry")
        else {
            panic!("invokevirtual must reference Methodref")
        };
        assert_eq!(
            class.constant_pool.class_name(*class_index),
            Ok("Delegator")
        );
        assert_eq!(
            class.constant_pool.name_and_type(*name_and_type_index),
            Ok((*expected_name, *expected_descriptor))
        );
    }
    calls
}

fn assert_opcode_sequence(code: &[u8], expected: &[u8]) {
    let instructions = decode_subset_instructions(code);
    assert_eq!(
        instructions
            .iter()
            .map(|(_, opcode)| *opcode)
            .collect::<Vec<_>>(),
        expected
    );
}

fn decode_subset_instructions(code: &[u8]) -> Vec<(usize, u8)> {
    let mut instructions = Vec::new();
    let mut offset = 0;
    while offset < code.len() {
        let opcode = code[offset];
        let length = match opcode {
            0x01
            | 0x02..=0x0f
            | 0x1a..=0x1d
            | 0x1e..=0x21
            | 0x22..=0x29
            | 0x2a..=0x2d
            | 0x3b..=0x3e
            | 0x3f..=0x42
            | 0x43..=0x4a
            | 0x4b..=0x4e
            | 0x57
            | 0x59
            | 0x60
            | 0x64
            | 0x68
            | 0x6c
            | 0x70
            | 0x7e
            | 0x80
            | 0x82
            | 0xac
            | 0xad
            | 0xae
            | 0xaf
            | 0xb0
            | 0xb1 => 1,
            0x10 | 0x12 | 0x15..=0x19 | 0x36..=0x3a => 2,
            0x13 | 0x14 | 0x99..=0x9a | 0x9f..=0xa6 | 0xa7 | 0xb4..=0xb8 | 0xbb => 3,
            other => panic!("unexpected opcode in supported JVM fixture: {other:#04x}"),
        };
        assert!(
            offset + length <= code.len(),
            "truncated bytecode instruction"
        );
        instructions.push((offset, opcode));
        offset += length;
    }
    instructions
}

fn decode_branch_targets(code: &[u8]) -> Vec<(usize, u8, usize)> {
    decode_subset_instructions(code)
        .into_iter()
        .filter(|(_, opcode)| matches!(*opcode, 0x99..=0x9a | 0x9f..=0xa6 | 0xa7))
        .map(|(offset, opcode)| {
            let displacement = i16::from_be_bytes([code[offset + 1], code[offset + 2]]);
            let target = offset as isize + isize::from(displacement);
            assert!(target >= 0 && (target as usize) < code.len());
            (offset, opcode, target as usize)
        })
        .collect()
}

fn decode_full_frame_pcs(payload: &[u8]) -> Vec<usize> {
    fn read_u2(payload: &[u8], offset: &mut usize) -> u16 {
        let bytes = payload
            .get(*offset..*offset + 2)
            .expect("complete u2 in StackMapTable payload");
        *offset += 2;
        u16::from_be_bytes([bytes[0], bytes[1]])
    }

    fn skip_verification_types(payload: &[u8], offset: &mut usize, entries: u16) {
        for _ in 0..entries {
            let tag = *payload
                .get(*offset)
                .expect("verification_type_info tag in StackMapTable");
            *offset += 1;
            match tag {
                0..=6 => {}
                7 | 8 => {
                    let _ = read_u2(payload, offset);
                }
                _ => panic!("invalid verification_type_info tag {tag}"),
            }
        }
    }

    let mut offset = 0;
    let frame_count = read_u2(payload, &mut offset);
    let mut previous_pc = -1i32;
    let mut pcs = Vec::with_capacity(usize::from(frame_count));
    for _ in 0..frame_count {
        let frame_type = *payload.get(offset).expect("frame_type in StackMapTable");
        offset += 1;
        assert_eq!(frame_type, 255, "backend emits explicit full_frame entries");
        let delta = read_u2(payload, &mut offset);
        let pc = previous_pc + i32::from(delta) + 1;
        assert!(pc >= 0);
        pcs.push(usize::try_from(pc).expect("nonnegative StackMapTable PC"));
        previous_pc = pc;
        let locals = read_u2(payload, &mut offset);
        skip_verification_types(payload, &mut offset, locals);
        let stack = read_u2(payload, &mut offset);
        skip_verification_types(payload, &mut offset, stack);
    }
    assert_eq!(
        offset,
        payload.len(),
        "StackMapTable payload has no trailing bytes"
    );
    assert!(pcs.windows(2).all(|pair| pair[0] < pair[1]));
    pcs
}

#[derive(Debug, PartialEq, Eq)]
enum StackMapVerificationType {
    Top,
    Integer,
    Float,
    Double,
    Long,
    Null,
    UninitializedThis,
    Object(String),
    Uninitialized(usize),
}

#[derive(Debug, PartialEq, Eq)]
struct DecodedStackMapFrame {
    pc: usize,
    locals: Vec<StackMapVerificationType>,
    stack: Vec<StackMapVerificationType>,
}

fn stack_map_frames(class: &ClassFile<'_>, method_name: &str) -> Vec<DecodedStackMapFrame> {
    fn read_u2(payload: &[u8], offset: &mut usize) -> u16 {
        let bytes = payload
            .get(*offset..*offset + 2)
            .expect("complete u2 in StackMapTable payload");
        *offset += 2;
        u16::from_be_bytes([bytes[0], bytes[1]])
    }

    fn read_verification_type(
        class: &ClassFile<'_>,
        payload: &[u8],
        offset: &mut usize,
    ) -> StackMapVerificationType {
        let tag = *payload
            .get(*offset)
            .expect("verification_type_info tag in StackMapTable");
        *offset += 1;
        match tag {
            0 => StackMapVerificationType::Top,
            1 => StackMapVerificationType::Integer,
            2 => StackMapVerificationType::Float,
            3 => StackMapVerificationType::Double,
            4 => StackMapVerificationType::Long,
            5 => StackMapVerificationType::Null,
            6 => StackMapVerificationType::UninitializedThis,
            7 => {
                let index = read_u2(payload, offset);
                StackMapVerificationType::Object(
                    class
                        .constant_pool
                        .class_name(ConstantPoolIndex(index))
                        .expect("Object verification type names a class constant")
                        .to_owned(),
                )
            }
            8 => StackMapVerificationType::Uninitialized(usize::from(read_u2(payload, offset))),
            _ => panic!("invalid verification_type_info tag {tag}"),
        }
    }

    fn read_types(
        class: &ClassFile<'_>,
        payload: &[u8],
        offset: &mut usize,
        count: u16,
    ) -> Vec<StackMapVerificationType> {
        (0..count)
            .map(|_| read_verification_type(class, payload, offset))
            .collect()
    }

    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index) == Ok(method_name))
        .unwrap_or_else(|| panic!("missing method {method_name}"));
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!("{method_name} should have one Code attribute");
    };
    let [Attribute::Other(stack_map)] = code
        .attributes
        .iter()
        .filter(|attribute| {
            matches!(attribute, Attribute::Other(raw) if class.constant_pool.utf8(raw.name_index) == Ok("StackMapTable"))
        })
        .collect::<Vec<_>>()[..]
    else {
        panic!("{method_name} should have one StackMapTable");
    };

    let payload = stack_map.bytes;
    let mut offset = 0;
    let frame_count = read_u2(payload, &mut offset);
    let mut previous_pc = -1i32;
    let mut frames = Vec::with_capacity(usize::from(frame_count));
    for _ in 0..frame_count {
        let frame_type = *payload.get(offset).expect("frame_type in StackMapTable");
        offset += 1;
        assert_eq!(frame_type, 255, "backend emits explicit full_frame entries");
        let delta = read_u2(payload, &mut offset);
        let pc = previous_pc + i32::from(delta) + 1;
        assert!(pc >= 0);
        previous_pc = pc;
        let local_count = read_u2(payload, &mut offset);
        let locals = read_types(class, payload, &mut offset, local_count);
        let stack_count = read_u2(payload, &mut offset);
        let stack = read_types(class, payload, &mut offset, stack_count);
        frames.push(DecodedStackMapFrame {
            pc: usize::try_from(pc).expect("nonnegative StackMapTable PC"),
            locals,
            stack,
        });
    }
    assert_eq!(offset, payload.len(), "StackMapTable has no trailing bytes");
    assert!(frames.windows(2).all(|pair| pair[0].pc < pair[1].pc));
    frames
}

fn verify_delegator_with_jdk(output_directory: &Path) {
    let output_path = output_directory.to_str().expect("UTF-8 temporary path");
    let javap = Command::new("javap")
        .args(["-c", "-v", "-classpath", output_path, "Delegator"])
        .output()
        .expect("start javap");
    assert!(
        javap.status.success(),
        "javap rejected Delegator.class\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javap.stdout),
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap_output = String::from_utf8_lossy(&javap.stdout);
    for expected in [
        "invokevirtual",
        "aload_0",
        "pop",
        "ireturn",
        "sequence(int)",
    ] {
        assert!(
            javap_output.contains(expected),
            "javap output omitted {expected:?}:\n{javap_output}"
        );
    }

    let harness = output_directory.join("DelegatorHarness.java");
    fs::write(
        &harness,
        "public final class DelegatorHarness { public static void main(String[] args) { Delegator d = new Delegator(); if (d.direct(7) != 7) throw new AssertionError(\"direct\"); if (d.explicit(8) != 8) throw new AssertionError(\"explicit\"); if (d.chain(9) != 9) throw new AssertionError(\"chain\"); if (d.zero() != 42) throw new AssertionError(\"zero\"); if (d.sequence(10) != 42) throw new AssertionError(\"sequence\"); System.out.println(d.direct(7) + \":\" + d.explicit(8) + \":\" + d.chain(9) + \":\" + d.constant() + \":\" + d.zero() + \":\" + d.sequence(10)); } }\n",
    )
    .expect("write Delegator Java harness");
    let javac = Command::new("javac")
        .args(["-classpath", output_path, "-d", output_path])
        .arg(&harness)
        .output()
        .expect("start javac");
    assert!(
        javac.status.success(),
        "javac rejected DelegatorHarness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            output_path,
            "DelegatorHarness",
        ])
        .output()
        .expect("start java verifier");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected Delegator.class\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "7:8:9:42:42:42"
    );
}

#[test]
fn initialized_local_reads_work_and_while_lowers_with_a_symbolic_back_edge() {
    let local_output = TestOutputDirectory::new("initialized-local-val");
    let local_artifact = compile_source(
        "class LocalVal { def run(x: Int): Int = { val y = x; y } }\n",
        "LocalVal.scala",
        local_output.path(),
    )
    .unwrap();
    let mut reader = Reader::new(local_artifact.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("LocalVal should decode");
    let run = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index) == Ok("run"))
        .expect("run method should be emitted");
    let Attribute::Code(code) = &run.attributes[0] else {
        panic!("run method has no Code attribute")
    };
    assert_eq!(code.code, &[0x1b, 0x3d, 0x1c, 0xac]);
    assert_eq!(code.max_locals, 3);

    let while_output = TestOutputDirectory::new("supported-while-back-edge");
    let while_artifact = compile_source(
        "class Loop { def run(): Unit = { while true do () } }\n",
        "Loop.scala",
        while_output.path(),
    )
    .unwrap();
    let mut reader = Reader::new(while_artifact.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("Loop should decode");
    let run = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index) == Ok("run"))
        .expect("run method should be emitted");
    let Attribute::Code(code) = &run.attributes[0] else {
        panic!("run method has no Code attribute")
    };
    assert_opcode_sequence(code.code, &[0xa7, 0x04, 0x99, 0xa7, 0xb1]);
}

#[test]
fn source_primitive_local_initializers_emit_load_store_and_correct_max_locals() {
    let output = TestOutputDirectory::new("primitive-local-initializers");
    let artifact = compile_source(
        "class LocalInit { def immutable(x: Int): Int = { val saved = x; saved }; def mutable(x: Int): Int = { var current = x; current }; def booleanLocal(x: Boolean): Boolean = { val saved = x; saved } }\n",
        "LocalInit.scala",
        output.path(),
    )
    .unwrap();
    let mut reader = Reader::new(artifact.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("LocalInit should decode");
    for method_name in ["immutable", "mutable", "booleanLocal"] {
        let method = class
            .methods
            .iter()
            .find(|method| class.constant_pool.utf8(method.name_index) == Ok(method_name))
            .unwrap_or_else(|| panic!("{method_name} method should be emitted"));
        let Attribute::Code(code) = &method.attributes[0] else {
            panic!("{method_name} method has no Code attribute")
        };
        assert_eq!(code.code, &[0x1b, 0x3d, 0x1c, 0xac]);
        assert_eq!(code.max_locals, 3);
    }

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 temporary path");
        let harness_path = output.path().join("LocalInitHarness.java");
        fs::write(
            &harness_path,
            "public final class LocalInitHarness { public static void main(String[] args) { LocalInit local = new LocalInit(); System.out.println(local.immutable(42) + \":\" + local.mutable(43) + \":\" + local.booleanLocal(true)); } }\n",
        )
        .expect("write local invocation harness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness_path)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected LocalInitHarness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "LocalInitHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected LocalInit\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&java.stdout).trim(), "42:43:true");
    }
}

#[test]
fn local_state_fixture_compiles_deterministically_and_executes_with_verified_slots() {
    let first_output = TestOutputDirectory::new("local-state-first");
    let second_output = TestOutputDirectory::new("local-state-second");
    let first = compile_fixture("LocalState.scala", first_output.path()).unwrap();
    let second = compile_fixture("LocalState.scala", second_output.path()).unwrap();
    assert_eq!(first.class.internal_name(), "LocalState");
    assert_eq!(first.class.bytes(), second.class.bytes());
    assert_eq!(first_output.class_files(), vec![first.path.clone()]);
    assert_eq!(second_output.class_files(), vec![second.path.clone()]);

    let mut reader = Reader::new(first.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("LocalState should decode completely");
    assert!(reader.is_at_end(), "decoder consumes the entire class file");
    class
        .validate_references()
        .expect("LocalState constant-pool references should be valid");
    assert_eq!(class.version.major, JVM_CLASSFILE_MAJOR_VERSION);
    assert_eq!(
        class.version.major, 65,
        "fixture requires class-file version 65"
    );
    assert_eq!(
        class.constant_pool.class_name(class.this_class),
        Ok("LocalState")
    );

    let signatures = class
        .methods
        .iter()
        .map(|method| {
            (
                class.constant_pool.utf8(method.name_index).unwrap(),
                class.constant_pool.utf8(method.descriptor_index).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        signatures,
        [
            ("<init>", "()V"),
            ("copy", "(I)I"),
            ("overwrite", "(I)I"),
            ("runOnce", "(Z)I"),
            ("parameterShadow", "(I)I"),
            ("nestedShadow", "()I"),
        ]
    );
    for method in &class.methods {
        assert!(
            matches!(method.attributes.as_slice(), [Attribute::Code(_)]),
            "{} must have exactly one complete Code attribute",
            class.constant_pool.utf8(method.name_index).unwrap()
        );
    }
    let method_code = |name: &str| {
        let method = class
            .methods
            .iter()
            .find(|method| class.constant_pool.utf8(method.name_index) == Ok(name))
            .unwrap_or_else(|| panic!("missing method {name}"));
        let [Attribute::Code(code)] = method.attributes.as_slice() else {
            panic!("{name} must have exactly one Code attribute")
        };
        code
    };

    let copy = method_code("copy");
    assert_eq!(copy.code, [0x1b, 0x3d, 0x1c, 0xac]);
    assert_eq!((copy.max_stack, copy.max_locals), (1, 3));

    let overwrite = method_code("overwrite");
    assert_eq!(overwrite.code, [0x1b, 0x3d, 0x10, 42, 0x3d, 0x1c, 0xac]);
    assert_eq!((overwrite.max_stack, overwrite.max_locals), (1, 3));

    let run_once = method_code("runOnce");
    let run_once_opcodes = decode_subset_instructions(run_once.code)
        .into_iter()
        .map(|(_, opcode)| opcode)
        .collect::<Vec<_>>();
    assert!(
        run_once_opcodes.contains(&0x1b),
        "loads the Boolean parameter"
    );
    assert!(
        run_once_opcodes.contains(&0x1d),
        "loads local slot 3 for the condition"
    );
    assert!(
        run_once_opcodes.contains(&0x3d),
        "stores the Int local in slot 2"
    );
    assert!(
        run_once_opcodes.contains(&0x3e),
        "stores the Boolean local in slot 3"
    );
    assert_eq!((run_once.max_stack, run_once.max_locals), (1, 4));
    let branch_targets = decode_branch_targets(run_once.code);
    let header_targets = branch_targets
        .iter()
        .filter_map(|(_, opcode, target)| (*opcode == 0xa7).then_some(*target))
        .collect::<Vec<_>>();
    let exit_targets = branch_targets
        .iter()
        .filter_map(|(_, opcode, target)| (*opcode == 0x99).then_some(*target))
        .collect::<Vec<_>>();
    assert_eq!(
        header_targets.len(),
        2,
        "entry and back-edge target the loop header"
    );
    assert_eq!(header_targets[0], header_targets[1]);
    assert_eq!(
        exit_targets.len(),
        1,
        "false condition branches to the loop exit"
    );
    let [Attribute::Other(stack_map)] = run_once
        .attributes
        .iter()
        .filter(|attribute| matches!(attribute, Attribute::Other(raw) if class.constant_pool.utf8(raw.name_index) == Ok("StackMapTable")))
        .collect::<Vec<_>>()[..]
    else {
        panic!("runOnce should have one nested StackMapTable")
    };
    let frame_pcs = decode_full_frame_pcs(stack_map.bytes);
    let mut expected_frame_pcs = vec![header_targets[0], exit_targets[0]];
    expected_frame_pcs.sort_unstable();
    expected_frame_pcs.dedup();
    assert_eq!(frame_pcs, expected_frame_pcs);

    let parameter_shadow = method_code("parameterShadow");
    assert_eq!(parameter_shadow.code, [0x10, 42, 0x3d, 0x1c, 0xac]);
    assert_eq!(parameter_shadow.max_locals, 3);

    let nested_shadow = method_code("nestedShadow");
    assert_eq!(
        nested_shadow.code,
        [0x04, 0x3c, 0x10, 42, 0x3d, 0x1c, 0x57, 0x1b, 0xac]
    );
    assert_eq!(nested_shadow.max_locals, 3);

    if require_jdk_or_skip() {
        let output_path = first_output.path().to_str().expect("UTF-8 output path");
        let javap = Command::new("javap")
            .args(["-c", "-v", "-classpath", output_path, "LocalState"])
            .output()
            .expect("start javap");
        assert!(
            javap.status.success(),
            "javap rejected LocalState\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javap.stdout),
            String::from_utf8_lossy(&javap.stderr)
        );
        let javap_output = String::from_utf8_lossy(&javap.stdout);
        for expected in [
            "major version: 65",
            "iload_3",
            "istore_3",
            "ifeq",
            "goto",
            "StackMapTable",
        ] {
            assert!(
                javap_output.contains(expected),
                "javap output omitted {expected:?}:\n{javap_output}"
            );
        }

        let harness_path = first_output.path().join("LocalStateHarness.java");
        fs::write(
            &harness_path,
            "public final class LocalStateHarness { public static void main(String[] args) { LocalState value = new LocalState(); System.out.println(value.copy(17) + \":\" + value.overwrite(17) + \":\" + value.runOnce(false) + \":\" + value.runOnce(true) + \":\" + value.parameterShadow(17) + \":\" + value.nestedShadow()); } }\n",
        )
        .expect("write LocalState Java harness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness_path)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected LocalStateHarness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "LocalStateHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected LocalState\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "17:42:0:1:42:1"
        );
    }
}

#[test]
fn initialized_mutable_primitive_locals_support_assignments() {
    let output = TestOutputDirectory::new("primitive-local-assignments");
    let artifact = compile_source(
        "class LocalAssignment { def next(): Int = 42; def changed(): Int = { var current = 10; current = next(); current }; def booleanLocal(): Int = { var flag = true; flag = false; if flag then 1 else 2 }; def shadowed(): Int = { var value = 1; { var value = 10; value = 42; value } } }\n",
        "LocalAssignment.scala",
        output.path(),
    )
    .unwrap();
    let mut reader = Reader::new(artifact.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("LocalAssignment should decode");
    let changed = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index) == Ok("changed"))
        .expect("changed method should be emitted");
    let Attribute::Code(changed_code) = &changed.attributes[0] else {
        panic!("changed method has no Code attribute")
    };
    assert_eq!(
        changed_code
            .code
            .iter()
            .filter(|opcode| **opcode == 0xb6)
            .count(),
        1
    );
    assert_eq!(changed_code.max_locals, 2);

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 temporary path");
        let harness_path = output.path().join("LocalAssignmentHarness.java");
        fs::write(
            &harness_path,
            "public final class LocalAssignmentHarness { public static void main(String[] args) { LocalAssignment local = new LocalAssignment(); System.out.println(local.changed() + \":\" + local.booleanLocal() + \":\" + local.shadowed()); } }\n",
        )
        .expect("write local assignment harness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness_path)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected LocalAssignmentHarness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "LocalAssignmentHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected LocalAssignment\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&java.stdout).trim(), "42:2:42");
    }
}

#[test]
fn assignment_branches_accept_proven_terminal_rhs_flow() {
    let output = TestOutputDirectory::new("assignment-terminal-branches");
    let artifact = compile_source(
        "class AssignmentTerminal { def terminal(flag: Boolean): Int = { var current = 1; if flag then current = { return 4 } else current = 2; current }; def terminalBlock(flag: Boolean): Int = { var current = 1; if flag then { current = { return 5 } } else { current = 3 }; current } }\n",
        "AssignmentTerminal.scala",
        output.path(),
    )
    .unwrap();
    let mut reader = Reader::new(artifact.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("AssignmentTerminal should decode");
    let method_code = |name: &str| {
        let method = class
            .methods
            .iter()
            .find(|method| class.constant_pool.utf8(method.name_index) == Ok(name))
            .unwrap_or_else(|| panic!("missing method {name}"));
        let [Attribute::Code(code)] = method.attributes.as_slice() else {
            panic!("{name} should contain one Code attribute")
        };
        code.code
    };
    assert_opcode_sequence(
        method_code("terminal"),
        &[0x04, 0x3d, 0x1b, 0x99, 0x07, 0xac, 0x05, 0x3d, 0x1c, 0xac],
    );

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 temporary path");
        let harness_path = output.path().join("AssignmentTerminalHarness.java");
        fs::write(
            &harness_path,
            "public final class AssignmentTerminalHarness { public static void main(String[] args) { AssignmentTerminal value = new AssignmentTerminal(); System.out.println(value.terminal(true) + \":\" + value.terminal(false) + \":\" + value.terminalBlock(true) + \":\" + value.terminalBlock(false)); } }\n",
        )
        .expect("write terminal assignment harness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness_path)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected AssignmentTerminalHarness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "AssignmentTerminalHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected AssignmentTerminal\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&java.stdout).trim(), "4:2:5:3");
    }
}

#[test]
fn primitive_local_while_loops_have_verified_back_edges_and_zero_iteration_paths() {
    let output = TestOutputDirectory::new("primitive-local-while");
    let artifact = compile_source(
        "class WhileLocals { def runOnce(flag: Boolean): Int = { var seen = 0; var running = flag; while running do { seen = 1; running = false }; seen }; def nestedIf(flag: Boolean): Int = { var seen = 0; var running = flag; while running do { if running then seen = 1 else seen = 2; running = false }; seen }; def nestedLoop(flag: Boolean): Int = { var seen = 0; var outer = flag; while outer do { var inner = true; while inner do { seen = 1; inner = false }; outer = false }; seen }; def loopFalse(): Int = { var seen = 0; while false do { seen = 1 }; seen }; def returnFromBody(): Int = { while true do return 9; 0 }; def discardInt(): Int = { while false do 7; 42 } }\n",
        "WhileLocals.scala",
        output.path(),
    )
    .unwrap();
    let mut reader = Reader::new(artifact.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("WhileLocals should decode");
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index) == Ok("runOnce"))
        .expect("runOnce method should be emitted");
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!("runOnce should contain one Code attribute")
    };
    assert_opcode_sequence(
        code.code,
        &[
            0x03, 0x3d, 0x1b, 0x3e, 0xa7, 0x1d, 0x99, 0x04, 0x3d, 0x03, 0x3e, 0xa7, 0x1c, 0xac,
        ],
    );
    assert_eq!(
        code.code,
        &[
            0x03, 0x3d, 0x1b, 0x3e, 0xa7, 0x00, 0x03, 0x1d, 0x99, 0x00, 0x0a, 0x04, 0x3d, 0x03,
            0x3e, 0xa7, 0xff, 0xf8, 0x1c, 0xac,
        ]
    );
    assert_eq!(code.max_stack, 1);
    assert_eq!(code.max_locals, 4);
    assert!(code.attributes.iter().any(|attribute| {
        let Attribute::Other(raw) = attribute else {
            return false;
        };
        class.constant_pool.utf8(raw.name_index).ok() == Some("StackMapTable")
    }));
    let return_method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index) == Ok("returnFromBody"))
        .expect("returnFromBody method should be emitted");
    let [Attribute::Code(return_code)] = return_method.attributes.as_slice() else {
        panic!("returnFromBody should contain one Code attribute")
    };
    assert_opcode_sequence(
        return_code.code,
        &[0xa7, 0x04, 0x99, 0x10, 0xac, 0x03, 0xac],
    );
    assert_eq!(
        decode_subset_instructions(return_code.code)
            .iter()
            .filter(|(_, opcode)| *opcode == 0xa7)
            .count(),
        1,
        "a definitely returning loop body must not emit a back-edge"
    );

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 temporary path");
        let javap = Command::new("javap")
            .args(["-c", "-v", "-classpath", output_path, "WhileLocals"])
            .output()
            .expect("start javap");
        assert!(
            javap.status.success(),
            "javap rejected WhileLocals\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javap.stdout),
            String::from_utf8_lossy(&javap.stderr)
        );
        let javap_output = String::from_utf8_lossy(&javap.stdout);
        for expected in ["ifeq", "goto", "StackMapTable"] {
            assert!(
                javap_output.contains(expected),
                "javap output omitted {expected:?}:\n{javap_output}"
            );
        }
        let harness_path = output.path().join("WhileLocalsHarness.java");
        fs::write(
            &harness_path,
            "public final class WhileLocalsHarness { public static void main(String[] args) { WhileLocals value = new WhileLocals(); System.out.println(value.runOnce(false) + \":\" + value.runOnce(true) + \":\" + value.nestedIf(false) + \":\" + value.nestedIf(true) + \":\" + value.nestedLoop(false) + \":\" + value.nestedLoop(true) + \":\" + value.loopFalse() + \":\" + value.returnFromBody() + \":\" + value.discardInt()); } }\n",
        )
        .expect("write while loop harness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness_path)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected WhileLocalsHarness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "WhileLocalsHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected WhileLocals\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "0:1:0:1:0:1:0:9:42"
        );
    }
}

#[test]
fn while_loops_reject_invalid_conditions_and_verify_calls_inside_control_flow() {
    let invalid_output = TestOutputDirectory::new("invalid-while-condition");
    let invalid_condition = compile_source(
        "class InvalidWhile { def run(): Unit = { while 1 do () } }\n",
        "InvalidWhile.scala",
        invalid_output.path(),
    )
    .unwrap_err();
    assert!(matches!(invalid_condition, PipelineFailure::Typer(_)));
    assert!(invalid_output.class_files().is_empty());

    let call_output = TestOutputDirectory::new("while-call-control-flow");
    let call = compile_source(
        "class WhileCall { def next(): Boolean = false; def run(flag: Boolean): Int = { var running = flag; while running do running = next(); 0 } }\n",
        "WhileCall.scala",
        call_output.path(),
    )
    .expect("a planned same-class call is valid inside a primitive while CFG");
    assert_eq!(call.class.internal_name(), "WhileCall");

    let branch_output = TestOutputDirectory::new("if-call-control-flow");
    let branch = compile_source(
        "class IfCall { def identity(value: Int): Int = value; def run(flag: Boolean): Int = if flag then identity(1) else 0 }\n",
        "IfCall.scala",
        branch_output.path(),
    )
    .expect("a planned same-class call is valid in an if branch");
    assert_eq!(branch.class.internal_name(), "IfCall");
}

#[test]
fn inherited_val_assignment_remains_rejected_by_typer() {
    assert_source_batch_failure(
        "inherited-val-assignment",
        "class Parent { val id: Int = 1 }\nclass Child extends Parent { def update(): Unit = id = 2 }\n",
        |failure| {
            matches!(
                failure,
                PipelineFailure::Typer(TyperError::AssignmentTargetImmutable { .. })
            )
        },
    );
}

#[test]
fn unsupported_local_source_shapes_fail_at_their_pipeline_stage_without_artifacts() {
    assert_source_failure(
        "immutable-local-assignment",
        "class ImmutableAssignment { def run(): Int = { val fixed = 1; fixed = 2; fixed } }\n",
        |failure| matches!(failure, PipelineFailure::Typer(_)),
    );
    assert_source_failure(
        "immutable-reference-local-assignment",
        "class ImmutableReferenceAssignment(seed: Int):\n  def run(seed: Int): Int =\n    val current: ImmutableReferenceAssignment = new ImmutableReferenceAssignment(seed)\n    current = new ImmutableReferenceAssignment(seed)\n    42\n",
        |failure| {
            matches!(
                failure,
                PipelineFailure::Typer(TyperError::AssignmentTargetImmutable {
                    source,
                    tree_index,
                    target: _,
                }) if *source == SourceId::from_index(1) && *tree_index > 0
            )
        },
    );
    let unrelated_reference_output =
        TestOutputDirectory::new("unrelated-reference-local-assignment");
    let unrelated_reference_error = compile_source_batch(
        "class OtherReferenceAssignment(seed: Int)\nclass UnrelatedReferenceAssignment(seed: Int):\n  def run(seed: Int): Int =\n    var current: UnrelatedReferenceAssignment = new UnrelatedReferenceAssignment(seed)\n    current = new OtherReferenceAssignment(seed)\n    42\n",
        "UnrelatedReferenceAssignment.scala",
        unrelated_reference_output.path(),
        SourceId::from_index(1),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect_err("an unrelated reference type must be rejected at assignment");
    assert!(
        matches!(
            &unrelated_reference_error,
            PipelineFailure::Typer(TyperError::ExpectedExpressionTypeMismatch {
                source,
                tree_index,
                ..
            }) if *source == SourceId::from_index(1) && *tree_index > 0
        ),
        "unrelated reference assignment must fail at its expected-type check: {unrelated_reference_error:?}"
    );
    assert!(unrelated_reference_output.class_files().is_empty());
    assert_source_failure(
        "missing-local-reference",
        "class MissingLocal { def run(): Int = missing }\n",
        |failure| matches!(failure, PipelineFailure::Typer(_)),
    );
    assert_source_failure(
        "wrong-local-initializer-type",
        "class WrongLocalInitializer { def run(): Int = { val flag: Boolean = 1; 0 } }\n",
        |failure| matches!(failure, PipelineFailure::Typer(_)),
    );
    assert_source_failure(
        "wrong-local-assignment-type",
        "class WrongLocalAssignment { def run(): Int = { var value = 1; value = true; value } }\n",
        |failure| matches!(failure, PipelineFailure::Typer(_)),
    );
    assert_source_failure(
        "unsupported-unit-local",
        "class UnitLocal { def run(): Int = { val empty = (); 0 } }\n",
        |failure| matches!(failure, PipelineFailure::Codegen(_)),
    );
    assert_source_failure(
        "deferred-reference-local",
        "class ReferenceLocal { def run(): Int = { val text = \"text\"; 0 } }\n",
        |failure| matches!(failure, PipelineFailure::Typer(_)),
    );
    assert_source_failure(
        "unsupported-pattern-local",
        "class PatternLocal { def run(): Int = { val (left, right) = (1, 2); left } }\n",
        |failure| matches!(failure, PipelineFailure::Typer(_)),
    );
}

#[test]
fn nested_same_named_source_locals_keep_distinct_slots_after_parameter_shadowing() {
    let output = TestOutputDirectory::new("nested-shadowed-local-initializers");
    let artifact = compile_source(
        "class NestedLocal { def run(left: Int, value: Int): Int = { val value = left; { val value = 42; value } } }\n",
        "NestedLocal.scala",
        output.path(),
    )
    .unwrap();
    let mut reader = Reader::new(artifact.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("NestedLocal should decode");
    let run = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index) == Ok("run"))
        .expect("run method should be emitted");
    let Attribute::Code(code) = &run.attributes[0] else {
        panic!("run method has no Code attribute")
    };
    assert_eq!(code.code, &[0x1b, 0x3e, 0x10, 42, 0x36, 4, 0x15, 4, 0xac]);
    assert_eq!(code.max_locals, 5);
}

#[test]
fn source_blocks_keep_term_reference_results_and_nested_divergence() {
    let output = TestOutputDirectory::new("block-review-regressions");
    let artifact = compile_source(
        "class BlockCases {\n  def parameter(x: Int): Int = { 1; x }\n  def nested(): Int = { { return 4; 5 } }\n}\n",
        "BlockCases.scala",
        output.path(),
    )
    .unwrap();
    let mut reader = Reader::new(artifact.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("emitted BlockCases should decode");
    let method_code = |name: &str| {
        let method = class
            .methods
            .iter()
            .find(|method| class.constant_pool.utf8(method.name_index) == Ok(name))
            .unwrap_or_else(|| panic!("missing method {name}"));
        let [Attribute::Code(code)] = method.attributes.as_slice() else {
            panic!("{name} should contain one Code attribute");
        };
        code.code
    };
    assert_eq!(method_code("parameter"), [0x04, 0x57, 0x1b, 0xac]);
    assert_eq!(method_code("nested"), [0x07, 0xac]);

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 temporary path");
        let harness_path = output.path().join("BlockCasesHarness.java");
        fs::write(
            &harness_path,
            "public final class BlockCasesHarness { public static void main(String[] args) { BlockCases cases = new BlockCases(); System.out.println(cases.parameter(37) + \":\" + cases.nested()); } }\n",
        )
        .expect("write block invocation harness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness_path)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected BlockCasesHarness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "BlockCasesHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected BlockCases\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&java.stdout).trim(), "37:4");
    }
}

fn verify_class_structure(bytes: &[u8]) {
    let mut reader = Reader::new(bytes);
    let class = ClassFile::decode(&mut reader).expect("emitted class should decode");
    assert!(
        reader.is_at_end(),
        "decoder consumes the complete class file"
    );
    class
        .validate_references()
        .expect("emitted class constant-pool references are valid");
    assert_eq!(class.version.major, JVM_CLASSFILE_MAJOR_VERSION);
    assert_eq!(
        class.constant_pool.class_name(class.this_class),
        Ok("Answer")
    );
    assert!(class.access_flags.is_public());
    assert_eq!(class.fields.len(), 0);

    let methods = class
        .methods
        .iter()
        .map(|method| {
            (
                class.constant_pool.utf8(method.name_index).unwrap(),
                class.constant_pool.utf8(method.descriptor_index).unwrap(),
                method,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        methods
            .iter()
            .map(|(name, descriptor, _)| (*name, *descriptor))
            .collect::<Vec<_>>(),
        [("<init>", "()V"), ("value", "()I"), ("select", "(ZII)I")]
    );
    assert_eq!(
        methods
            .iter()
            .filter(|(name, _, _)| *name == "<init>")
            .count(),
        1
    );
    for (_, _, method) in methods {
        assert!(method.access_flags.is_public());
        assert!(matches!(method.attributes.as_slice(), [Attribute::Code(_)]));
    }
    let [Attribute::Code(select_code)] = class.methods[2].attributes.as_slice() else {
        panic!("select should contain exactly one Code attribute");
    };
    assert!(select_code.code.contains(&0x99));
    assert!(select_code.attributes.iter().any(|attribute| matches!(
        attribute,
        Attribute::Other(raw)
            if class.constant_pool.utf8(raw.name_index) == Ok("StackMapTable")
    )));
}

fn require_jdk_or_skip() -> bool {
    let unavailable = ["java", "javac", "javap"]
        .into_iter()
        .filter_map(|tool| match tool_major_version(tool) {
            Some(version) if version >= 21 => None,
            Some(version) => Some(format!("{tool} {version} (requires JDK 21+)")),
            None => Some(format!("{tool} (missing or version unavailable)")),
        })
        .collect::<Vec<_>>();
    if unavailable.is_empty() {
        return true;
    }
    let message = format!(
        "source-to-JVM gate requires java, javac, and javap from JDK 21+ on PATH; unavailable: {}",
        unavailable.join(", ")
    );
    if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        panic!("{message}");
    }
    eprintln!("skipping optional source-to-JVM JDK gate: {message}");
    false
}

fn tool_major_version(tool: &str) -> Option<u32> {
    let output = Command::new(tool).arg("-version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let version_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    version_text
        .split(|character: char| !character.is_ascii_digit())
        .find_map(|component| component.parse::<u32>().ok())
}

#[test]
fn jdk21_reference_cfg_oracle_records_frames_without_enabling_backend_support() {
    if !require_jdk_or_skip() {
        return;
    }

    let output = TestOutputDirectory::new("reference-cfg-oracle");
    let fixture = Path::new("tests/fixtures/jvm-reference-cfg/ReferenceFlow.java");
    let javac = Command::new("javac")
        .args(["--release", "21", "-g", "-d"])
        .arg(output.path())
        .arg(fixture)
        .output()
        .expect("start javac for reference CFG oracle");
    assert!(
        javac.status.success(),
        "javac rejected ReferenceFlow.java\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    assert!(output.path().join("ReferenceFlow.class").is_file());
    assert!(output.path().join("Counter.class").is_file());

    let classpath = output.path().to_str().expect("UTF-8 output path");
    let javap = Command::new("javap")
        .args(["-v", "-classpath", classpath, "ReferenceFlow", "Counter"])
        .output()
        .expect("start javap for reference CFG oracle");
    assert!(
        javap.status.success(),
        "javap rejected ReferenceFlow classes\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javap.stdout),
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap_output = String::from_utf8_lossy(&javap.stdout);
    for expected in [
        "major version: 65",
        "StackMapTable: number_of_entries",
        "locals = [ class Counter ]",
        "class Counter",
        "Counter.increment",
        "Counter.get",
        "newInBranch",
        "sameClassJoin",
        "invokevirtual",
        "invokespecial",
        "getfield",
        "putfield",
    ] {
        assert!(
            javap_output.contains(expected),
            "javap -v output omitted {expected:?}:\n{javap_output}"
        );
    }

    let harness = output.path().join("ReferenceFlowHarness.java");
    fs::write(
        &harness,
        "public final class ReferenceFlowHarness {\n  public static void main(String[] args) {\n    ReferenceFlow flow = new ReferenceFlow();\n    if (ReferenceFlow.loop(new Counter(0), 10) != 10) throw new AssertionError(\"loop\");\n    if (flow.branchOnThis(true) != 1 || flow.branchOnThis(false) != 0) throw new AssertionError(\"receiver\");\n    if (ReferenceFlow.sameClassJoin(true).get() != 3 || ReferenceFlow.sameClassJoin(false).get() != 4) throw new AssertionError(\"join\");\n    if (ReferenceFlow.newInBranch(true).get() != 5 || ReferenceFlow.newInBranch(false).get() != 6) throw new AssertionError(\"branch allocation\");\n    System.out.println(\"reference CFG oracle ok\");\n  }\n}\n",
    )
    .expect("write Java reference CFG harness");
    let javac = Command::new("javac")
        .args(["--release", "21", "-classpath", classpath, "-d", classpath])
        .arg(&harness)
        .output()
        .expect("start javac for reference CFG harness");
    assert!(
        javac.status.success(),
        "javac rejected ReferenceFlowHarness.java\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );

    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            classpath,
            "ReferenceFlowHarness",
        ])
        .output()
        .expect("start Java 21 verifier for reference CFG oracle");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected the Java 21 reference CFG oracle\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "reference CFG oracle ok"
    );
}

fn verify_with_jdk(output_directory: &Path) {
    let output_path = output_directory.to_str().expect("UTF-8 temporary path");
    let javap = Command::new("javap")
        .args(["-v", "-classpath", output_path, "Answer"])
        .output()
        .expect("start javap");
    assert!(
        javap.status.success(),
        "javap rejected Answer.class\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javap.stdout),
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap_output = String::from_utf8_lossy(&javap.stdout);
    for expected in [
        "major version: 65",
        "descriptor: ()I",
        "descriptor: (ZII)I",
        "ifeq",
    ] {
        assert!(
            javap_output.contains(expected),
            "javap output omitted {expected:?}:\n{javap_output}"
        );
    }

    let harness_path = output_directory.join("AnswerHarness.java");
    fs::write(
        &harness_path,
        "public final class AnswerHarness {\n  public static void main(String[] args) {\n    Answer answer = new Answer();\n    if (answer.value() != 42) throw new AssertionError(\"value\");\n    if (answer.select(true, 10, 20) != 10) throw new AssertionError(\"true\");\n    if (answer.select(false, 10, 20) != 20) throw new AssertionError(\"false\");\n    System.out.println(answer.value());\n    System.out.println(answer.select(true, 10, 20));\n    System.out.println(answer.select(false, 10, 20));\n  }\n}\n",
    )
    .expect("write Java invocation harness");
    let javac = Command::new("javac")
        .args(["-classpath", output_path, "-d", output_path])
        .arg(&harness_path)
        .output()
        .expect("start javac");
    assert!(
        javac.status.success(),
        "javac could not compile AnswerHarness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );

    let java = Command::new("java")
        .args(["-Xverify:all", "-classpath", output_path, "AnswerHarness"])
        .output()
        .expect("start java verifier");
    assert!(
        java.status.success(),
        "java -Xverify:all failed for Answer.class\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&java.stdout).trim(), "42\n10\n20");
}

#[test]
fn real_source_accessor_calls_dispatch_through_validated_getters_and_setters() {
    let source = r#"
class Counter(val id: Int, var count: Long) {
  val initial: Int = 10
  var current: Long = 0L
  def ownRead(): Long = this.count
  def ownReadId(): Int = this.id
  def ownUnqualifiedRead(): Long = count
  def ownWrite(next: Long): Unit = this.count = next
  def ownBodyRead(): Long = this.current
  def ownBodyUnqualifiedRead(): Long = current
  def ownBodyWrite(next: Long): Unit = this.current = next
}
class OtherCounter(var count: Long) {
  var current: Long = 31L
}
class CounterFactory(var events: Int) {
  def receiver(c: Counter): Counter = { events = events * 10 + 1; c }
  def value(next: Long): Long = { events = events * 10 + 2; next }
}
class PrivateCounter {
  private var count: Long = 0L
  def read(): Long = this.count
  def write(next: Long): Unit = this.count = next
}
class Reader {
  def readInitial(c: Counter): Int = c.initial
  def readCurrent(c: Counter): Long = c.current
  def writeCurrent(c: Counter, next: Long): Unit = c.current = next
  def localCurrent(c: Counter): Long = { val local = c; local.current }
  def readOtherCurrent(c: OtherCounter): Long = c.current
  def writeCurrentOnce(factory: CounterFactory, c: Counter, next: Long): Unit =
    factory.receiver(c).current = factory.value(next)
  def readId(c: Counter): Int = c.id
  def readCount(c: Counter): Long = c.count
  def write(c: Counter, next: Long): Unit = c.count = next
  def nestedWrite(a: Counter, b: Counter): Unit = a.count = { b.count = 2L; 3L }
  def readOther(c: OtherCounter): Long = c.count
  def localRead(c: Counter): Long = { val local = c; local.count }
  def localWrite(c: Counter, next: Long): Long = { val local = c; local.count = next; local.count }
  def writeOnce(factory: CounterFactory, c: Counter, next: Long): Unit =
    factory.receiver(c).count = factory.value(next)
}
"#;
    let first_output = TestOutputDirectory::new("accessor-call-first");
    let second_output = TestOutputDirectory::new("accessor-call-second");
    let first = compile_source_batch(
        source,
        "AccessorCalls.scala",
        first_output.path(),
        SourceId::from_index(1562),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect("real typed accessor selections and assignments lower through the call pipeline");
    let second = compile_source_batch(
        source,
        "AccessorCalls.scala",
        second_output.path(),
        SourceId::from_index(1563),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect("a fresh source session emits the same accessor call batch");
    assert_eq!(
        first
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        [
            "Counter",
            "OtherCounter",
            "CounterFactory",
            "PrivateCounter",
            "Reader"
        ]
    );
    assert_eq!(
        first
            .iter()
            .map(|artifact| {
                (
                    artifact.class.internal_name().to_owned(),
                    artifact.class.bytes().to_vec(),
                )
            })
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|artifact| {
                (
                    artifact.class.internal_name().to_owned(),
                    artifact.class.bytes().to_vec(),
                )
            })
            .collect::<Vec<_>>()
    );

    let counter = decode_emitted_class_named(&first, "Counter");
    let other_counter = decode_emitted_class_named(&first, "OtherCounter");
    let private_counter = decode_emitted_class_named(&first, "PrivateCounter");
    let reader = decode_emitted_class_named(&first, "Reader");

    for (method, expected) in [
        ("ownRead", &[(0xb6, "Counter", "count", "()J")][..]),
        ("ownReadId", &[(0xb6, "Counter", "id", "()I")][..]),
        (
            "ownUnqualifiedRead",
            &[(0xb6, "Counter", "count", "()J")][..],
        ),
        ("ownWrite", &[(0xb6, "Counter", "count_$eq", "(J)V")][..]),
        ("ownBodyRead", &[(0xb6, "Counter", "current", "()J")][..]),
        (
            "ownBodyUnqualifiedRead",
            &[(0xb6, "Counter", "current", "()J")][..],
        ),
        (
            "ownBodyWrite",
            &[(0xb6, "Counter", "current_$eq", "(J)V")][..],
        ),
    ] {
        assert_invocation_sequence(&counter, emitted_method_code(&counter, method), expected);
    }
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "readInitial"),
        &[(0xb6, "Counter", "initial", "()I")],
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "readCurrent"),
        &[(0xb6, "Counter", "current", "()J")],
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "writeCurrent"),
        &[(0xb6, "Counter", "current_$eq", "(J)V")],
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "localCurrent"),
        &[(0xb6, "Counter", "current", "()J")],
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "readOtherCurrent"),
        &[(0xb6, "OtherCounter", "current", "()J")],
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "writeCurrentOnce"),
        &[
            (0xb6, "CounterFactory", "receiver", "(LCounter;)LCounter;"),
            (0xb6, "CounterFactory", "value", "(J)J"),
            (0xb6, "Counter", "current_$eq", "(J)V"),
        ],
    );
    for method in ["ownBodyRead", "ownBodyUnqualifiedRead", "ownBodyWrite"] {
        assert_no_field_reference_to_owner(
            &counter,
            emitted_method_code(&counter, method),
            "Counter",
        );
    }
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "readId"),
        &[(0xb6, "Counter", "id", "()I")],
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "readCount"),
        &[(0xb6, "Counter", "count", "()J")],
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "write"),
        &[(0xb6, "Counter", "count_$eq", "(J)V")],
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "nestedWrite"),
        &[
            (0xb6, "Counter", "count_$eq", "(J)V"),
            (0xb6, "Counter", "count_$eq", "(J)V"),
        ],
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "readOther"),
        &[(0xb6, "OtherCounter", "count", "()J")],
    );
    assert_field_reference(
        &other_counter,
        emitted_method_code(&other_counter, "count"),
        0xb4,
        "OtherCounter",
        "count",
        "J",
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "localRead"),
        &[(0xb6, "Counter", "count", "()J")],
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "localWrite"),
        &[
            (0xb6, "Counter", "count_$eq", "(J)V"),
            (0xb6, "Counter", "count", "()J"),
        ],
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "writeOnce"),
        &[
            (0xb6, "CounterFactory", "receiver", "(LCounter;)LCounter;"),
            (0xb6, "CounterFactory", "value", "(J)J"),
            (0xb6, "Counter", "count_$eq", "(J)V"),
        ],
    );
    for method in [
        "readInitial",
        "readCurrent",
        "writeCurrent",
        "localCurrent",
        "readOtherCurrent",
        "writeCurrentOnce",
        "readId",
        "readCount",
        "write",
        "nestedWrite",
        "readOther",
        "localRead",
        "localWrite",
        "writeOnce",
    ] {
        assert_no_field_reference_to_owner(
            &reader,
            emitted_method_code(&reader, method),
            "Counter",
        );
    }
    assert_field_reference(
        &private_counter,
        emitted_method_code(&private_counter, "read"),
        0xb4,
        "PrivateCounter",
        "count",
        "J",
    );
    assert_field_reference(
        &private_counter,
        emitted_method_code(&private_counter, "write"),
        0xb5,
        "PrivateCounter",
        "count",
        "J",
    );
    assert_eq!(
        methods_with_descriptors(&counter)
            .iter()
            .filter(|(name, _)| name.as_str() == "id" || name.as_str() == "count")
            .map(|(name, descriptor)| (name.as_str(), descriptor.as_str()))
            .collect::<Vec<_>>(),
        [("id", "()I"), ("count", "()J")]
    );
    assert!(
        !methods_with_descriptors(&counter)
            .iter()
            .any(|(name, _)| name == "id_$eq"),
        "a val has no generated setter"
    );
    let counter_accessors = methods_with_descriptors(&counter)
        .into_iter()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "id" | "count" | "initial" | "initial_$eq" | "current" | "current_$eq"
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        counter_accessors,
        [
            ("id".to_owned(), "()I".to_owned()),
            ("count".to_owned(), "()J".to_owned()),
            ("initial".to_owned(), "()I".to_owned()),
            ("current".to_owned(), "()J".to_owned()),
            ("current_$eq".to_owned(), "(J)V".to_owned()),
        ],
        "constructor and body accessors keep their exact public ABI"
    );

    if require_jdk_or_skip() {
        let harness = first_output.path().join("AccessorCallsHarness.java");
        fs::write(
            &harness,
            r#"public final class AccessorCallsHarness {
  private static void check(boolean condition, String label) { if (!condition) throw new AssertionError(label); }
  public static void main(String[] args) {
    Counter first = new Counter(7, 10L);
    Counter second = new Counter(-3, 20L);
    Reader reader = new Reader();
    check(first.ownRead() == 10L && first.ownReadId() == 7 && first.ownUnqualifiedRead() == 10L, "this and unqualified getters");
    check(reader.readId(first) == 7 && reader.readCount(first) == 10L, "parameter getters");
    check(first.initial() == 10 && first.ownBodyRead() == 0L && first.ownBodyUnqualifiedRead() == 0L, "body field getters");
    check(reader.readInitial(first) == 10 && reader.readCurrent(first) == 0L && reader.localCurrent(first) == 0L, "body field call sites");
    reader.writeCurrent(first, Long.MIN_VALUE);
    check(first.current() == Long.MIN_VALUE, "body var setter");
    reader.writeCurrent(first, Long.MAX_VALUE);
    check(first.current() == Long.MAX_VALUE, "body var setter width");
    first.ownBodyWrite(-11L);
    check(first.current() == -11L, "same-class body var setter");
    check(second.current() == 0L, "body var instance isolation");
    reader.write(first, Long.MIN_VALUE + 9);
    check(first.count() == Long.MIN_VALUE + 9 && second.count() == 20L, "parameter setter and instance isolation");
    Counter nestedA = new Counter(1, 10L);
    Counter nestedB = new Counter(2, 20L);
    reader.nestedWrite(nestedA, nestedB);
    check(nestedA.count() == 3L && nestedB.count() == 2L, "nested assignments preserve outer receiver");
    check(reader.localRead(second) == 20L, "stable local getter");
    check(reader.localWrite(second, Long.MAX_VALUE - 4) == Long.MAX_VALUE - 4 && second.count() == Long.MAX_VALUE - 4, "stable local setter/getter and Long width");
    OtherCounter other = new OtherCounter(31L);
    check(reader.readOther(other) == 31L, "canonical same-named field owner");
    check(reader.readOtherCurrent(other) == 31L, "same-spelled body field keeps its canonical owner");
    CounterFactory factory = new CounterFactory(0);
    reader.writeOnce(factory, first, 88L);
    check(factory.events() == 12 && first.count() == 88L, "receiver then RHS exactly once");
    factory.events_$eq(0);
    reader.writeCurrentOnce(factory, first, 99L);
    check(factory.events() == 12 && first.current() == 99L, "body setter receiver then RHS exactly once");
    System.out.println("accessor calls ok");
  }
}
"#,
        )
        .expect("write Java accessor call harness");
        let classpath = first_output.path().to_str().expect("UTF-8 classpath");
        let javac = Command::new("javac")
            .args(["--release", "21", "-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac for source accessor call harness");
        assert!(
            javac.status.success(),
            "javac rejected AccessorCallsHarness.java\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "AccessorCallsHarness",
            ])
            .output()
            .expect("start Java 21 verifier for source accessor call harness");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected accessor call classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "accessor calls ok"
        );
    }
}

#[test]
fn class_body_accessor_acceptance_runs_body_only_and_parameterized_fixtures() {
    let source = r#"
class BodyOnly:
  val seed: Int = 10
  var enabled: Boolean = true
  var ratio: Double = 1.5

class Counter(val id: Int):
  val initial: Int = 10
  var current: Long = 0L
  private var internal: Boolean = false
  def snapshot(): Long = current
  def updateInternal(value: Boolean): Unit = internal = value
  def internalState(): Boolean = internal

class Reader:
  def read(c: Counter): Long = c.current
  def initial(c: Counter): Int = c.initial
  def write(c: Counter, next: Long): Unit = c.current = next
  def readCurrent(c: Counter): Long =
    val stable = c
    stable.current
"#;
    let first_output = TestOutputDirectory::new("body-accessor-acceptance-first");
    let second_output = TestOutputDirectory::new("body-accessor-acceptance-second");
    let first = compile_source_batch(
        source,
        "BodyAccessorAcceptance.scala",
        first_output.path(),
        SourceId::from_index(1585),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect("the complete class-body accessor acceptance batch compiles from real source");
    let second = compile_source_batch(
        source,
        "BodyAccessorAcceptance.scala",
        second_output.path(),
        SourceId::from_index(1586),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect("a fresh frontend session compiles the same acceptance batch");
    let names = first
        .iter()
        .map(|artifact| artifact.class.internal_name())
        .collect::<Vec<_>>();
    assert_eq!(names, ["BodyOnly", "Counter", "Reader"]);
    assert_eq!(
        first
            .iter()
            .map(|artifact| (
                artifact.class.internal_name().to_owned(),
                artifact.class.bytes().to_vec(),
            ))
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|artifact| (
                artifact.class.internal_name().to_owned(),
                artifact.class.bytes().to_vec(),
            ))
            .collect::<Vec<_>>(),
        "ordered complete classfile bytes are deterministic across fresh sessions"
    );

    let body_only = decode_emitted_class_named(&first, "BodyOnly");
    let counter = decode_emitted_class_named(&first, "Counter");
    let reader = decode_emitted_class_named(&first, "Reader");
    let field_shapes = |class: &ClassFile<'_>| {
        class
            .fields
            .iter()
            .map(|field| {
                (
                    class
                        .constant_pool
                        .utf8(field.name_index)
                        .unwrap()
                        .to_owned(),
                    class
                        .constant_pool
                        .utf8(field.descriptor_index)
                        .unwrap()
                        .to_owned(),
                    field.access_flags,
                )
            })
            .collect::<Vec<_>>()
    };
    let body_fields = field_shapes(&body_only);
    assert_eq!(
        body_fields
            .iter()
            .map(|(name, descriptor, _)| (name.as_str(), descriptor.as_str()))
            .collect::<Vec<_>>(),
        [("seed", "I"), ("enabled", "Z"), ("ratio", "D")]
    );
    assert!(body_fields[0].2.0 & 0x0002 != 0 && body_fields[0].2.0 & 0x0010 != 0);
    assert!(
        body_fields[1..]
            .iter()
            .all(|(_, _, flags)| flags.0 & 0x0002 != 0)
    );
    assert!(
        body_fields[1..]
            .iter()
            .all(|(_, _, flags)| flags.0 & 0x0010 == 0)
    );
    let counter_fields = field_shapes(&counter);
    assert_eq!(
        counter_fields
            .iter()
            .map(|(name, descriptor, _)| (name.as_str(), descriptor.as_str()))
            .collect::<Vec<_>>(),
        [
            ("id", "I"),
            ("initial", "I"),
            ("current", "J"),
            ("internal", "Z")
        ]
    );
    assert!(counter_fields[0].2.0 & 0x0002 != 0 && counter_fields[0].2.0 & 0x0010 != 0);
    assert!(counter_fields[1].2.0 & 0x0002 != 0 && counter_fields[1].2.0 & 0x0010 != 0);
    assert!(
        counter_fields[2..]
            .iter()
            .all(|(_, _, flags)| flags.0 & 0x0002 != 0)
    );
    assert!(
        counter_fields[2..]
            .iter()
            .all(|(_, _, flags)| flags.0 & 0x0010 == 0)
    );

    let body_methods = methods_with_descriptors(&body_only);
    assert!(
        body_methods
            .iter()
            .any(|(name, descriptor)| name == "<init>" && descriptor == "()V")
    );
    for accessor in [
        ("seed", "()I"),
        ("enabled", "()Z"),
        ("enabled_$eq", "(Z)V"),
        ("ratio", "()D"),
        ("ratio_$eq", "(D)V"),
    ] {
        assert!(
            body_methods
                .iter()
                .any(|(name, descriptor)| (name.as_str(), descriptor.as_str()) == accessor)
        );
    }
    assert!(!body_methods.iter().any(|(name, _)| name == "seed_$eq"));
    let counter_methods = methods_with_descriptors(&counter);
    for accessor in [
        ("<init>", "(I)V"),
        ("id", "()I"),
        ("initial", "()I"),
        ("current", "()J"),
        ("current_$eq", "(J)V"),
        ("snapshot", "()J"),
        ("updateInternal", "(Z)V"),
        ("internalState", "()Z"),
    ] {
        assert!(
            counter_methods
                .iter()
                .any(|(name, descriptor)| (name.as_str(), descriptor.as_str()) == accessor)
        );
    }
    for immutable in ["id_$eq", "initial_$eq", "internal", "internal_$eq"] {
        assert!(!counter_methods.iter().any(|(name, _)| name == immutable));
    }
    assert!(
        !counter_methods
            .iter()
            .any(|(name, descriptor)| { name == "<init>" && descriptor == "()V" })
    );
    for (class, public_methods) in [
        (
            &body_only,
            [
                ("<init>", "()V"),
                ("seed", "()I"),
                ("enabled", "()Z"),
                ("enabled_$eq", "(Z)V"),
                ("ratio", "()D"),
                ("ratio_$eq", "(D)V"),
            ]
            .as_slice(),
        ),
        (
            &counter,
            [
                ("<init>", "(I)V"),
                ("id", "()I"),
                ("initial", "()I"),
                ("current", "()J"),
                ("current_$eq", "(J)V"),
            ]
            .as_slice(),
        ),
    ] {
        for (expected_name, expected_descriptor) in public_methods {
            assert!(class.methods.iter().any(|method| {
                class.constant_pool.utf8(method.name_index).unwrap() == *expected_name
                    && class.constant_pool.utf8(method.descriptor_index).unwrap()
                        == *expected_descriptor
                    && method.access_flags.0 & 0x0001 != 0
            }));
        }
    }

    assert_field_reference_sequence(
        &body_only,
        emitted_method_code(&body_only, "<init>"),
        &[
            (0xb5, "BodyOnly", "seed", "I"),
            (0xb5, "BodyOnly", "enabled", "Z"),
            (0xb5, "BodyOnly", "ratio", "D"),
        ],
    );
    for (method, opcode, field, descriptor) in [
        ("seed", 0xb4, "seed", "I"),
        ("enabled", 0xb4, "enabled", "Z"),
        ("enabled_$eq", 0xb5, "enabled", "Z"),
        ("ratio", 0xb4, "ratio", "D"),
        ("ratio_$eq", 0xb5, "ratio", "D"),
    ] {
        assert_field_reference(
            &body_only,
            emitted_method_code(&body_only, method),
            opcode,
            "BodyOnly",
            field,
            descriptor,
        );
    }
    assert_invocation_sequence(
        &body_only,
        emitted_method_code(&body_only, "<init>"),
        &[(0xb7, "java/lang/Object", "<init>", "()V")],
    );
    assert_field_reference_sequence(
        &counter,
        emitted_method_code(&counter, "<init>"),
        &[
            (0xb5, "Counter", "id", "I"),
            (0xb5, "Counter", "initial", "I"),
            (0xb5, "Counter", "current", "J"),
            (0xb5, "Counter", "internal", "Z"),
        ],
    );
    for (method, opcode, field, descriptor) in [
        ("id", 0xb4, "id", "I"),
        ("initial", 0xb4, "initial", "I"),
        ("current", 0xb4, "current", "J"),
        ("current_$eq", 0xb5, "current", "J"),
    ] {
        assert_field_reference(
            &counter,
            emitted_method_code(&counter, method),
            opcode,
            "Counter",
            field,
            descriptor,
        );
    }
    assert_invocation_sequence(
        &counter,
        emitted_method_code(&counter, "<init>"),
        &[(0xb7, "java/lang/Object", "<init>", "()V")],
    );
    assert_invocation_sequence(
        &counter,
        emitted_method_code(&counter, "snapshot"),
        &[(0xb6, "Counter", "current", "()J")],
    );
    assert_field_reference(
        &counter,
        emitted_method_code(&counter, "updateInternal"),
        0xb5,
        "Counter",
        "internal",
        "Z",
    );
    assert_field_reference(
        &counter,
        emitted_method_code(&counter, "internalState"),
        0xb4,
        "Counter",
        "internal",
        "Z",
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "read"),
        &[(0xb6, "Counter", "current", "()J")],
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "initial"),
        &[(0xb6, "Counter", "initial", "()I")],
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "write"),
        &[(0xb6, "Counter", "current_$eq", "(J)V")],
    );
    assert_invocation_sequence(
        &reader,
        emitted_method_code(&reader, "readCurrent"),
        &[(0xb6, "Counter", "current", "()J")],
    );
    for method in ["read", "initial", "write", "readCurrent"] {
        assert_no_field_reference_to_owner(
            &reader,
            emitted_method_code(&reader, method),
            "Counter",
        );
    }

    if require_jdk_or_skip() {
        for class_name in ["BodyOnly", "Counter", "Reader"] {
            let javap = Command::new("javap")
                .args([
                    "-c",
                    "-v",
                    "-p",
                    "-classpath",
                    first_output.path().to_str().unwrap(),
                    class_name,
                ])
                .output()
                .expect("start javap for class-body accessor acceptance");
            assert!(
                javap.status.success(),
                "javap rejected {class_name}: {}",
                String::from_utf8_lossy(&javap.stderr)
            );
            assert!(String::from_utf8_lossy(&javap.stdout).contains("major version: 65"));
        }

        let harness = first_output
            .path()
            .join("BodyAccessorAcceptanceHarness.java");
        fs::write(
            &harness,
            r#"public final class BodyAccessorAcceptanceHarness {
  private static void check(boolean condition, String label) { if (!condition) throw new AssertionError(label); }
  public static void main(String[] args) {
    BodyOnly body = new BodyOnly();
    check(body.seed() == 10 && body.enabled() && body.ratio() == 1.5, "body-only getters");
    body.enabled_$eq(false);
    body.ratio_$eq(-0.0d);
    check(!body.enabled() && Double.doubleToRawLongBits(body.ratio()) == Double.doubleToRawLongBits(-0.0d), "Boolean and Double setters");
    Counter first = new Counter(7);
    Counter second = new Counter(-3);
    Reader reader = new Reader();
    check(first.id() == 7 && first.initial() == 10 && first.current() == 0L, "constructor and body getters");
    check(reader.read(first) == 0L && reader.initial(first) == 10 && reader.readCurrent(first) == 0L, "cross-class getters");
    reader.write(first, Long.MIN_VALUE);
    check(first.current() == Long.MIN_VALUE && reader.readCurrent(first) == Long.MIN_VALUE, "Long minimum setter");
    reader.write(first, Long.MAX_VALUE);
    check(first.current() == Long.MAX_VALUE && second.current() == 0L, "Long maximum and instance isolation");
    first.updateInternal(true);
    check(first.internalState() && !second.internalState(), "private field stays behind declared methods");
  }
}
"#,
        )
        .expect("write Java 21 class-body accessor acceptance harness");
        let classpath = first_output.path().to_str().expect("UTF-8 classpath");
        let javac = Command::new("javac")
            .args(["--release", "21", "-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac for class-body accessor acceptance harness");
        assert!(
            javac.status.success(),
            "javac rejected the acceptance harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "BodyAccessorAcceptanceHarness",
            ])
            .output()
            .expect("start Java 21 verifier for class-body accessor acceptance");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected the acceptance classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
    }
}

#[test]
fn constructor_accessor_source_batch_matches_the_jvm_abi_across_fresh_frontends() {
    let source = "class Measurement(val id: Int, var value: Double)\nclass Mixed(first: Int, val count: Long, var enabled: Boolean)\nclass LongBox(var count: Long)\n";
    let first_output = TestOutputDirectory::new("constructor-accessor-source-first");
    let second_output = TestOutputDirectory::new("constructor-accessor-source-second");
    let source_id = SourceId::from_index(1559);
    let mut checked_source_bindings = false;
    let mut observe_source = |ast: &AstArena<Untyped>,
                              index: &SourceSemanticIndex,
                              unit: &mut JvmCompilationUnit<'_>,
                              definitions: &Definitions| {
        let metadata = unit
            .metadata(definitions)
            .expect("validated source metadata");
        for (class_tree, node) in ast.iter() {
            let TreeKind::TypeDef(class_definition) = &node.kind else {
                continue;
            };
            let class = index
                .symbol_at(source_id, class_tree)
                .expect("canonical source class identity");
            let class_name = unit
                .store
                .names
                .resolve(class_definition.name.as_name().text());
            if !matches!(class_name, "Measurement" | "Mixed" | "LongBox") {
                continue;
            }
            let class_metadata = metadata
                .iter()
                .find(|entry| entry.identity().symbol() == class)
                .expect("source class appears in the JVM compilation unit");
            let TreeKind::Template(template) = &ast.get(class_definition.rhs).kind else {
                panic!("{class_name} has a source template");
            };
            let constructor = index
                .symbol_at(source_id, template.constructor)
                .expect("canonical primary constructor identity");
            let TreeKind::DefDef(constructor_definition) = &ast.get(template.constructor).kind
            else {
                panic!("{class_name} has a primary constructor definition");
            };
            let parameters = constructor_definition
                .value_param_clauses
                .iter()
                .flatten()
                .enumerate()
                .map(|(ordinal, parameter_tree)| {
                    let TreeKind::ValDef(parameter_definition) = &ast.get(*parameter_tree).kind
                    else {
                        panic!("constructor parameter is represented by a ValDef");
                    };
                    let source_field = index
                        .symbol_at(source_id, *parameter_tree)
                        .expect("canonical source constructor parameter");
                    let formal = index
                        .derived_symbol_at(constructor, source_id, *parameter_tree)
                        .expect("distinct constructor-owned formal identity");
                    assert_eq!(unit.store.symbols.get(formal).owner, Some(constructor));
                    let name = unit
                        .store
                        .names
                        .resolve(parameter_definition.name.as_name().text());
                    let accessor = class_metadata
                        .accessors()
                        .iter()
                        .find(|accessor| accessor.constructor_parameter() == Some(formal));
                    if name == "first" {
                        assert!(
                            accessor.is_none(),
                            "plain constructor formal has no accessor"
                        );
                        return (name, source_field, formal, ordinal, false);
                    }

                    let accessor = accessor.expect("val/var formal has an accessor binding");
                    let field_symbol = unit.store.symbols.get(source_field);
                    let mutable = field_symbol.flags.contains(SymbolFlags::MUTABLE);
                    assert_eq!(
                        mutable,
                        matches!(
                            (class_name, name),
                            ("Measurement", "value") | ("Mixed", "enabled") | ("LongBox", "count")
                        )
                    );
                    assert_eq!(accessor.source_field(), source_field);
                    assert_eq!(accessor.constructor_parameter(), Some(formal));
                    assert_eq!(accessor.owner(), class);
                    assert_eq!(accessor.source_ordinal(), ordinal);
                    assert_eq!(accessor.is_mutable(), mutable);
                    assert_eq!(field_symbol.kind, SymbolKind::Field);
                    assert_eq!(field_symbol.owner, Some(class));
                    assert_eq!(field_symbol.visibility, Visibility::Public);
                    assert_eq!(field_symbol.flags.contains(SymbolFlags::MUTABLE), mutable);
                    assert_eq!(
                        accessor.field_access_flags().0,
                        dotty_classfile::access_flags::ACC_PRIVATE
                            | if mutable {
                                0
                            } else {
                                dotty_classfile::access_flags::ACC_FINAL
                            }
                    );
                    assert_eq!(
                        accessor.getter().access_flags().0,
                        dotty_classfile::access_flags::ACC_PUBLIC
                    );
                    assert_eq!(accessor.setter().is_some(), mutable);
                    if let Some(setter) = accessor.setter() {
                        assert_eq!(
                            setter.access_flags().0,
                            dotty_classfile::access_flags::ACC_PUBLIC
                        );
                    }
                    (name, source_field, formal, ordinal, true)
                })
                .collect::<Vec<_>>();

            if class_name == "Measurement" {
                assert_eq!(
                    parameters.iter().map(|entry| entry.0).collect::<Vec<_>>(),
                    ["id", "value"]
                );
                assert_eq!(
                    class_metadata
                        .accessors()
                        .iter()
                        .map(|accessor| (
                            accessor.getter().name(),
                            accessor.getter().descriptor().as_str()
                        ))
                        .collect::<Vec<_>>(),
                    [("id", "()I"), ("value", "()D")]
                );
            } else if class_name == "Mixed" {
                assert_eq!(
                    parameters.iter().map(|entry| entry.0).collect::<Vec<_>>(),
                    ["first", "count", "enabled"]
                );
                assert_eq!(
                    class_metadata
                        .accessors()
                        .iter()
                        .map(|accessor| (
                            accessor.getter().name(),
                            accessor.getter().descriptor().as_str()
                        ))
                        .collect::<Vec<_>>(),
                    [("count", "()J"), ("enabled", "()Z")]
                );
            } else {
                assert_eq!(
                    parameters.iter().map(|entry| entry.0).collect::<Vec<_>>(),
                    ["count"]
                );
                assert_eq!(
                    class_metadata
                        .accessors()
                        .iter()
                        .map(|accessor| (
                            accessor.getter().name(),
                            accessor.getter().descriptor().as_str()
                        ))
                        .collect::<Vec<_>>(),
                    [("count", "()J")]
                );
            }
        }
        checked_source_bindings = true;
    };
    let first = compile_source_batch_with_source_observer(
        source,
        "ConstructorAccessors.scala",
        first_output.path(),
        source_id,
        SourceUnitCardinality::AtLeastTwo,
        &mut observe_source,
    )
    .expect("real Scala source pipeline emits constructor accessor classes");
    assert!(
        checked_source_bindings,
        "same-session canonical bindings inspected"
    );
    let second = compile_source_batch(
        source,
        "ConstructorAccessors.scala",
        second_output.path(),
        SourceId::from_index(1560),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect("independent source session emits the same constructor accessor classes");
    let ordered_bytes = |artifacts: &[EmittedArtifact]| {
        artifacts
            .iter()
            .map(|artifact| {
                (
                    artifact.class.internal_name().to_owned(),
                    artifact.class.bytes().to_vec(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        first
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Measurement", "Mixed", "LongBox"]
    );
    assert_eq!(ordered_bytes(&first), ordered_bytes(&second));
    assert_eq!(
        first_output.class_files(),
        [
            first_output.path().join("LongBox.class"),
            first_output.path().join("Measurement.class"),
            first_output.path().join("Mixed.class"),
        ]
    );
    assert_eq!(
        second_output.class_files(),
        [
            second_output.path().join("LongBox.class"),
            second_output.path().join("Measurement.class"),
            second_output.path().join("Mixed.class"),
        ]
    );

    let measurement = decode_emitted_class(&first[0]);
    let mixed = decode_emitted_class(&first[1]);
    let long_box = decode_emitted_class(&first[2]);
    let private_final =
        dotty_classfile::access_flags::ACC_PRIVATE | dotty_classfile::access_flags::ACC_FINAL;
    let fields = |class: &ClassFile<'_>| {
        class
            .fields
            .iter()
            .map(|field| {
                (
                    class
                        .constant_pool
                        .utf8(field.name_index)
                        .unwrap()
                        .to_owned(),
                    class
                        .constant_pool
                        .utf8(field.descriptor_index)
                        .unwrap()
                        .to_owned(),
                    field.access_flags.0,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        fields(&measurement),
        [
            ("id".to_owned(), "I".to_owned(), private_final),
            (
                "value".to_owned(),
                "D".to_owned(),
                dotty_classfile::access_flags::ACC_PRIVATE
            )
        ]
    );
    assert_eq!(
        fields(&mixed),
        [
            ("count".to_owned(), "J".to_owned(), private_final),
            (
                "enabled".to_owned(),
                "Z".to_owned(),
                dotty_classfile::access_flags::ACC_PRIVATE
            )
        ]
    );
    assert_eq!(
        methods_with_descriptors(&measurement),
        [
            ("<init>".into(), "(ID)V".into()),
            ("id".into(), "()I".into()),
            ("value".into(), "()D".into()),
            ("value_$eq".into(), "(D)V".into())
        ]
    );
    assert_eq!(
        methods_with_descriptors(&mixed),
        [
            ("<init>".into(), "(IJZ)V".into()),
            ("count".into(), "()J".into()),
            ("enabled".into(), "()Z".into()),
            ("enabled_$eq".into(), "(Z)V".into())
        ]
    );
    assert_eq!(
        fields(&long_box),
        [(
            "count".to_owned(),
            "J".to_owned(),
            dotty_classfile::access_flags::ACC_PRIVATE
        )]
    );
    assert_eq!(
        methods_with_descriptors(&long_box),
        [
            ("<init>".into(), "(J)V".into()),
            ("count".into(), "()J".into()),
            ("count_$eq".into(), "(J)V".into())
        ]
    );
    for class in [&measurement, &mixed, &long_box] {
        assert_eq!(
            class.constant_pool.class_name(class.super_class.unwrap()),
            Ok("java/lang/Object")
        );
        for method in &class.methods {
            assert!(method.access_flags.is_public());
        }
    }

    let measurement_constructor = emitted_method_code(&measurement, "<init>");
    assert_opcode_sequence(
        measurement_constructor,
        &[0x2a, 0xb7, 0x2a, 0x1b, 0xb5, 0x2a, 0x28, 0xb5, 0xb1],
    );
    assert_invocation_sequence(
        &measurement,
        measurement_constructor,
        &[(0xb7, "java/lang/Object", "<init>", "()V")],
    );
    assert_field_reference_sequence(
        &measurement,
        measurement_constructor,
        &[
            (0xb5, "Measurement", "id", "I"),
            (0xb5, "Measurement", "value", "D"),
        ],
    );
    assert_eq!(code_max_locals(&measurement, "<init>"), 4);
    assert_eq!(code_max_stack(&measurement, "<init>"), 3);
    let mixed_constructor = emitted_method_code(&mixed, "<init>");
    assert_opcode_sequence(
        mixed_constructor,
        &[0x2a, 0xb7, 0x2a, 0x20, 0xb5, 0x2a, 0x15, 0xb5, 0xb1],
    );
    assert_eq!(
        mixed_constructor[11], 4,
        "Boolean constructor formal remains in slot four"
    );
    assert_invocation_sequence(
        &mixed,
        mixed_constructor,
        &[(0xb7, "java/lang/Object", "<init>", "()V")],
    );
    assert_field_reference_sequence(
        &mixed,
        mixed_constructor,
        &[
            (0xb5, "Mixed", "count", "J"),
            (0xb5, "Mixed", "enabled", "Z"),
        ],
    );
    assert_eq!(code_max_locals(&mixed, "<init>"), 5);
    assert_eq!(code_max_stack(&mixed, "<init>"), 3);

    for (class, accessors) in [
        (
            &measurement,
            [
                ("id", "I", "()I", None),
                ("value", "D", "()D", Some("(D)V")),
            ]
            .as_slice(),
        ),
        (
            &mixed,
            [
                ("count", "J", "()J", None),
                ("enabled", "Z", "()Z", Some("(Z)V")),
            ]
            .as_slice(),
        ),
        (&long_box, [("count", "J", "()J", Some("(J)V"))].as_slice()),
    ] {
        for (name, field_descriptor, getter_descriptor, setter_descriptor) in accessors {
            assert!(
                methods_with_descriptors(class)
                    .contains(&((*name).to_owned(), (*getter_descriptor).to_owned()))
            );
            let getter = emitted_method_code(class, name);
            let getter_return = match *field_descriptor {
                "J" => 0xad,
                "D" => 0xaf,
                _ => 0xac,
            };
            assert_opcode_sequence(getter, &[0x2a, 0xb4, getter_return]);
            assert_field_reference(
                class,
                getter,
                0xb4,
                class.constant_pool.class_name(class.this_class).unwrap(),
                name,
                field_descriptor,
            );
            assert_eq!(code_max_locals(class, name), 1);
            assert_eq!(
                code_max_stack(class, name),
                if matches!(*field_descriptor, "J" | "D") {
                    2
                } else {
                    1
                }
            );
            if let Some(setter_descriptor) = setter_descriptor {
                let setter_name = format!("{name}_$eq");
                let setter = emitted_method_code(class, &setter_name);
                let load = match *field_descriptor {
                    "J" => 0x1f,
                    "D" => 0x27,
                    _ => 0x1b,
                };
                assert_opcode_sequence(setter, &[0x2a, load, 0xb5, 0xb1]);
                assert_field_reference(
                    class,
                    setter,
                    0xb5,
                    class.constant_pool.class_name(class.this_class).unwrap(),
                    name,
                    field_descriptor,
                );
                assert_eq!(
                    code_max_stack(class, &setter_name),
                    if matches!(*field_descriptor, "J" | "D") {
                        3
                    } else {
                        2
                    }
                );
                assert_eq!(
                    code_max_locals(class, &setter_name),
                    if matches!(*field_descriptor, "J" | "D") {
                        3
                    } else {
                        2
                    }
                );
                assert!(
                    methods_with_descriptors(class)
                        .contains(&(setter_name, (*setter_descriptor).into()))
                );
            } else {
                assert!(
                    !methods_with_descriptors(class)
                        .iter()
                        .any(|(method, _)| method == &format!("{name}_$eq"))
                );
            }
        }
    }

    if require_jdk_or_skip() {
        for (class_name, expected_descriptors) in [
            ("Measurement", &["(ID)V", "()I", "()D", "(D)V"][..]),
            ("Mixed", &["(IJZ)V", "()J", "()Z", "(Z)V"][..]),
            ("LongBox", &["(J)V", "()J", "(J)V"][..]),
        ] {
            let javap = Command::new("javap")
                .args([
                    "-c",
                    "-v",
                    "-p",
                    "-classpath",
                    first_output.path().to_str().unwrap(),
                    class_name,
                ])
                .output()
                .expect("start javap for constructor accessor acceptance class");
            assert!(
                javap.status.success(),
                "javap failed for {class_name}: {javap:?}"
            );
            let output = String::from_utf8_lossy(&javap.stdout);
            assert!(
                output.contains("major version: 65"),
                "{class_name}: {output}"
            );
            for descriptor in expected_descriptors {
                assert!(
                    output.contains(&format!("descriptor: {descriptor}")),
                    "{class_name} omitted {descriptor}: {output}"
                );
            }
            let instruction_checks: &[(&str, &[&str])] = match class_name {
                "Measurement" => &[
                    (
                        "public Measurement(int, double);",
                        &[
                            "aload_0",
                            "invokespecial",
                            "aload_0",
                            "iload_1",
                            "putfield",
                            "aload_0",
                            "dload_2",
                            "putfield",
                            "return",
                        ],
                    ),
                    ("public int id();", &["aload_0", "getfield", "ireturn"]),
                    (
                        "public double value();",
                        &["aload_0", "getfield", "dreturn"],
                    ),
                    (
                        "public void value_$eq(double);",
                        &["aload_0", "dload_1", "putfield", "return"],
                    ),
                ],
                "Mixed" => &[
                    (
                        "public Mixed(int, long, boolean);",
                        &[
                            "aload_0",
                            "invokespecial",
                            "aload_0",
                            "lload_2",
                            "putfield",
                            "aload_0",
                            "iload",
                            "putfield",
                            "return",
                        ],
                    ),
                    ("public long count();", &["aload_0", "getfield", "lreturn"]),
                    (
                        "public boolean enabled();",
                        &["aload_0", "getfield", "ireturn"],
                    ),
                    (
                        "public void enabled_$eq(boolean);",
                        &["aload_0", "iload_1", "putfield", "return"],
                    ),
                ],
                "LongBox" => &[
                    (
                        "public LongBox(long);",
                        &[
                            "aload_0",
                            "invokespecial",
                            "aload_0",
                            "lload_1",
                            "putfield",
                            "return",
                        ],
                    ),
                    ("public long count();", &["aload_0", "getfield", "lreturn"]),
                    (
                        "public void count_$eq(long);",
                        &["aload_0", "lload_1", "putfield", "return"],
                    ),
                ],
                _ => unreachable!(),
            };
            for (method_header, instructions) in instruction_checks {
                assert_javap_instruction_sequence(&output, method_header, instructions);
            }
        }
        let harness = first_output.path().join("ConstructorAccessorsHarness.java");
        fs::write(
            &harness,
            r#"public final class ConstructorAccessorsHarness {
  private static void check(boolean value, String label) { if (!value) throw new AssertionError(label); }
  public static void main(String[] args) {
    Measurement first = new Measurement(7, 1.5d);
    Measurement second = new Measurement(-4, -2.25d);
    check(first.id() == 7 && first.value() == 1.5d, "Measurement constructor/getters");
    check(second.id() == -4 && second.value() == -2.25d, "second Measurement");
    first.value_$eq(4.75d);
    check(first.value() == 4.75d && second.value() == -2.25d, "independent Double setter");
    Mixed mixed = new Mixed(123, Long.MIN_VALUE + 17, true);
    check(mixed.count() == Long.MIN_VALUE + 17 && mixed.enabled(), "Mixed constructor/getters");
    mixed.enabled_$eq(false);
    check(mixed.count() == Long.MIN_VALUE + 17 && !mixed.enabled(), "val and Boolean setter");
    LongBox longBox = new LongBox(Long.MIN_VALUE + 17);
    longBox.count_$eq(Long.MAX_VALUE - 9);
    check(longBox.count() == Long.MAX_VALUE - 9, "Long setter");
    check(first.id() == 7, "immutable val retains its constructor value");
  }
}
"#,
        )
        .expect("write Java constructor accessor harness");
        let classpath = first_output.path().to_str().unwrap();
        let javac = Command::new("javac")
            .args(["-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac for source-to-JVM accessor harness");
        assert!(
            javac.status.success(),
            "javac rejected source accessor harness\n{}\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "ConstructorAccessorsHarness",
            ])
            .output()
            .expect("start JDK 21 verifier for source accessor harness");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected source accessor classes\n{}\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
    }

    let failed_output = TestOutputDirectory::new("constructor-accessor-failed-batch");
    let failed = compile_source_batch(
        "class Measurement(val id: Int, var value: Double)\nclass Broken { def +(): Int = 3 }\n",
        "ConstructorAccessorFailure.scala",
        failed_output.path(),
        SourceId::from_index(1561),
        SourceUnitCardinality::AtLeastTwo,
    );
    assert!(matches!(failed, Err(PipelineFailure::Codegen(_))));
    assert!(
        failed_output.class_files().is_empty(),
        "a later batch failure publishes no accessor artifacts"
    );
}

#[test]
fn multi_class_source_compilation_emits_deterministic_classes_in_source_order() {
    let source = "class Alpha { def id(): Int = 1; def fromBeta(): Int = new Beta().id() }\nclass Beta { def id(): Int = 2 }\n";
    let first_output = TestOutputDirectory::new("multi-class-source-first");
    let first = compile_source_classes(source, "Multi.scala", first_output.path())
        .expect("both top-level classes should compile in one frontend session");

    assert_eq!(
        first
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Alpha", "Beta"]
    );
    assert_eq!(
        first_output.class_files(),
        [
            first_output.path().join("Alpha.class"),
            first_output.path().join("Beta.class")
        ]
    );
    for artifact in &first {
        assert!(artifact.path.is_file());
    }

    let decode_id = |artifact: &EmittedArtifact, expected: u8| {
        let mut reader = Reader::new(artifact.class.bytes());
        let class = ClassFile::decode(&mut reader).expect("decode emitted class");
        assert!(reader.is_at_end());
        class.validate_references().expect("valid class references");
        assert_eq!(class.version.major, JVM_CLASSFILE_MAJOR_VERSION);
        assert_eq!(class.fields.len(), 0);
        assert_opcode_sequence(emitted_method_code(&class, "id"), &[expected, 0xac]);
    };
    decode_id(&first[0], 0x04);
    decode_id(&first[1], 0x05);
    let mut alpha_reader = Reader::new(first[0].class.bytes());
    let alpha = ClassFile::decode(&mut alpha_reader).expect("decode Alpha");
    assert_opcode_sequence(
        emitted_method_code(&alpha, "fromBeta"),
        &[0xbb, 0x59, 0xb7, 0xb6, 0xac],
    );

    let second_output = TestOutputDirectory::new("multi-class-source-second");
    let second = compile_source_classes(source, "Multi.scala", second_output.path())
        .expect("a fresh frontend session should produce the same class batch");
    assert_eq!(
        first
            .iter()
            .map(|artifact| {
                (
                    artifact.class.internal_name().to_owned(),
                    artifact.class.bytes().to_vec(),
                )
            })
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|artifact| {
                (
                    artifact.class.internal_name().to_owned(),
                    artifact.class.bytes().to_vec(),
                )
            })
            .collect::<Vec<_>>()
    );

    let reversed_output = TestOutputDirectory::new("multi-class-source-reversed");
    let reversed = compile_source_classes(
        "class Beta { def id(): Int = 2 }\nclass Alpha { def id(): Int = 1 }\n",
        "Reversed.scala",
        reversed_output.path(),
    )
    .expect("reversed source declaration order should be preserved");
    assert_eq!(
        reversed
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Beta", "Alpha"]
    );

    if require_jdk_or_skip() {
        let harness = first_output.path().join("MultiClassHarness.java");
        fs::write(
            &harness,
            "public final class MultiClassHarness { public static void main(String[] args) { if (new Alpha().id() != 1 || new Alpha().fromBeta() != 2 || new Beta().id() != 2) throw new AssertionError(); System.out.println(\"multi-class ok\"); } }\n",
        )
        .expect("write Java multi-class caller");
        let output_path = first_output.path().to_str().expect("UTF-8 output path");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected multi-class caller\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "MultiClassHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected multi-class outputs\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "multi-class ok"
        );
    }
}

#[test]
fn single_class_source_wrapper_rejects_multi_class_units_without_output() {
    let output = TestOutputDirectory::new("single-class-wrapper-rejects-batch");
    let error = compile_source(
        "class Alpha { def id(): Int = 1 }\nclass Beta { def id(): Int = 2 }\n",
        "Multi.scala",
        output.path(),
    )
    .expect_err("the single-artifact wrapper must retain its one-class contract");
    assert!(
        matches!(
            error,
            PipelineFailure::UnexpectedSourceShape("exactly one ordinary top-level class")
        ),
        "unexpected single-class wrapper result: {error:?}"
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn multi_class_output_paths_use_validated_package_internal_names() {
    let output = TestOutputDirectory::new("multi-class-source-package");
    let artifacts = compile_source_classes(
        "package example\nclass Alpha { def id(): Int = 1 }\nclass Beta { def id(): Int = 2 }\n",
        "PackagedMulti.scala",
        output.path(),
    )
    .expect("packaged classes should preserve their internal-name paths");
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["example/Alpha", "example/Beta"]
    );
    assert_eq!(
        output.class_files(),
        [
            output.path().join("example/Alpha.class"),
            output.path().join("example/Beta.class")
        ]
    );
}

#[test]
fn multi_class_source_backend_failure_publishes_no_classes() {
    let output = TestOutputDirectory::new("multi-class-source-failure");
    let result = compile_source_classes(
        "class Alpha { def id(): Int = 1 }\nclass Beta { def id(): Int = 2 }\nclass Broken { def +(): Int = 3 }\n",
        "BrokenBatch.scala",
        output.path(),
    );
    assert!(
        matches!(result, Err(PipelineFailure::Codegen(_))),
        "an unsupported third class must fail during codegen, got {result:?}"
    );
    assert!(
        output.class_files().is_empty(),
        "a later class failure must not publish the earlier valid class"
    );
}

#[test]
fn body_field_initializer_failure_keeps_a_multi_class_batch_unpublished() {
    let output = TestOutputDirectory::new("body-field-initializer-batch-failure");
    let result = compile_source_classes(
        "class Ready { val value: Int = 10 }\nclass Broken { val first: Int = later; val later: Int = 2 }",
        "BodyFieldBatch.scala",
        output.path(),
    );
    assert!(matches!(
        result,
        Err(PipelineFailure::Codegen(
            JvmCodegenError::FieldInitializerReadsUninitializedField { .. }
        ))
    ));
    assert!(
        output.class_files().is_empty(),
        "a later body initialization error must not publish the earlier valid class"
    );
}

#[test]
fn public_body_field_rejections_keep_source_batches_unpublished() {
    let cases: [SourceFailureExpectation; 11] = [
        (
            "public-body-missing-rhs",
            "class Ready { def id(): Int = 1 }\nclass Broken { val value: Int }",
            |failure| {
                matches!(
                    failure,
                    PipelineFailure::Codegen(JvmCodegenError::MissingFieldInitializer { .. })
                )
            },
        ),
        (
            "public-body-wrong-rhs",
            "class Ready { def id(): Int = 1 }\nclass Broken { val value: Int = true }",
            |failure| {
                matches!(
                    failure,
                    PipelineFailure::Typer(TyperError::ExpectedExpressionTypeMismatch { .. })
                )
            },
        ),
        (
            "public-body-duplicate-field",
            "class Ready { def id(): Int = 1 }\nclass Broken { val value: Int = 1; var value: Int = 2 }",
            |failure| {
                matches!(
                    failure,
                    PipelineFailure::Codegen(JvmCodegenError::DuplicateJvmFieldName { .. })
                )
            },
        ),
        (
            "public-body-getter-collision",
            "class Ready { def id(): Int = 1 }\nclass Broken { val value: Int = 1; def value(): Int = 2 }",
            |failure| {
                matches!(
                    failure,
                    PipelineFailure::Codegen(
                        JvmCodegenError::DuplicateJvmAccessorMethodIdentity { .. }
                    )
                )
            },
        ),
        (
            "public-body-setter-collision",
            "class Ready { def id(): Int = 1 }\nclass Broken { var value: Int = 1; def value_=(next: Int): Unit = {} }",
            |failure| {
                matches!(
                    failure,
                    PipelineFailure::Codegen(
                        JvmCodegenError::DuplicateJvmAccessorMethodIdentity { .. }
                    )
                )
            },
        ),
        (
            "public-body-immutable-assignment",
            "class Ready { def id(): Int = 1 }\nclass Broken { val value: Int = 1; def update(): Unit = value = 2 }",
            |failure| {
                matches!(
                    failure,
                    PipelineFailure::Typer(TyperError::AssignmentTargetImmutable { .. })
                )
            },
        ),
        (
            "public-body-invalid-receiver",
            "class Ready { def id(): Int = 1 }\nclass Broken { def read(value: Int): Int = value.amount }",
            |failure| {
                matches!(
                    failure,
                    PipelineFailure::Typer(TyperError::MemberNotFound { .. })
                )
            },
        ),
        (
            "public-body-field-called-as-method",
            "class Ready { def id(): Int = 1 }\nclass Broken { val value: Int = 1; def read(): Int = value() }",
            |failure| {
                matches!(
                    failure,
                    PipelineFailure::Typer(TyperError::ApplicationCalleeNotMethod { .. })
                )
            },
        ),
        (
            "public-body-unsupported-protected-field",
            "class Ready { def id(): Int = 1 }\nclass Broken { protected val value: Int = 1 }",
            |failure| {
                matches!(
                    failure,
                    PipelineFailure::Codegen(JvmCodegenError::UnsupportedJvmVisibility { .. })
                )
            },
        ),
        (
            "public-body-unsupported-private-this-field",
            "class Ready { def id(): Int = 1 }\nclass Broken { private[this] val value: Int = 1 }",
            |failure| {
                matches!(
                    failure,
                    PipelineFailure::Codegen(
                        JvmCodegenError::UnsupportedJvmFieldSourceVisibility { .. }
                    )
                )
            },
        ),
        (
            "public-body-unsupported-lazy-field",
            "class Ready { def id(): Int = 1 }\nclass Broken { lazy val value: Int = 1 }",
            |failure| {
                matches!(
                    failure,
                    PipelineFailure::Codegen(JvmCodegenError::UnsupportedJvmFieldFlag { .. })
                )
            },
        ),
    ];

    for (label, source, expected_failure) in cases {
        let output = TestOutputDirectory::new(label);
        let failure = compile_source_classes(source, &format!("{label}.scala"), output.path())
            .expect_err("invalid public-body source batch must fail before publication");
        assert!(
            expected_failure(&failure),
            "{label} failed at an unexpected first pipeline stage: {failure:?}"
        );
        assert!(
            output.class_files().is_empty(),
            "{label} failure must not publish an earlier valid class"
        );
    }
}

#[test]
fn real_counter_objects_source_emits_two_deterministic_verified_classes() {
    let first_output = TestOutputDirectory::new("counter-objects-first");
    let second_output = TestOutputDirectory::new("counter-objects-second");
    let source = fs::read_to_string(fixture_path("CounterObjects.scala"))
        .expect("read CounterObjects fixture");
    let first = compile_source_classes(&source, "CounterObjects.scala", first_output.path())
        .expect("Counter and CounterClient should compile from one source session");
    let second = compile_source_classes(&source, "CounterObjects.scala", second_output.path())
        .expect("a fresh source session should compile both classes");
    let first_names = first
        .iter()
        .map(|artifact| artifact.class.internal_name())
        .collect::<Vec<_>>();
    let second_names = second
        .iter()
        .map(|artifact| artifact.class.internal_name())
        .collect::<Vec<_>>();
    assert_eq!(first_names, ["Counter", "CounterClient"]);
    assert_eq!(second_names, ["Counter", "CounterClient"]);
    assert_eq!(
        first
            .iter()
            .map(|artifact| artifact.class.bytes())
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|artifact| artifact.class.bytes())
            .collect::<Vec<_>>(),
        "fresh frontend sessions should produce byte-identical ordered classfiles"
    );
    assert_eq!(
        first_output.class_files(),
        [
            first_output.path().join("Counter.class"),
            first_output.path().join("CounterClient.class")
        ]
    );
    assert_eq!(
        second_output.class_files(),
        [
            second_output.path().join("Counter.class"),
            second_output.path().join("CounterClient.class")
        ]
    );

    let counter = decode_emitted_class(&first[0]);
    let client = decode_emitted_class(&first[1]);
    assert_eq!(counter.fields.len(), 1);
    let [current] = counter.fields.as_slice() else {
        panic!("Counter emits only its current field");
    };
    assert_eq!(
        counter.constant_pool.utf8(current.name_index),
        Ok("current")
    );
    assert_eq!(
        counter.constant_pool.utf8(current.descriptor_index),
        Ok("I")
    );
    assert_eq!(
        current.access_flags.0,
        dotty_classfile::access_flags::ACC_PRIVATE
    );
    assert_eq!(
        methods_with_descriptors(&counter),
        [
            ("<init>".to_owned(), "(I)V".to_owned()),
            ("increment".to_owned(), "()I".to_owned()),
            ("get".to_owned(), "()I".to_owned()),
        ]
    );
    assert_eq!(
        methods_with_descriptors(&client),
        [
            ("<init>".to_owned(), "()V".to_owned()),
            ("run".to_owned(), "(I)I".to_owned()),
            ("direct".to_owned(), "()I".to_owned()),
            ("independent".to_owned(), "()I".to_owned()),
        ]
    );
    assert!(
        counter
            .methods
            .iter()
            .all(|method| method.access_flags.is_public())
    );
    assert!(
        client
            .methods
            .iter()
            .all(|method| method.access_flags.is_public())
    );
    assert!(
        methods_with_descriptors(&counter)
            .iter()
            .chain(methods_with_descriptors(&client).iter())
            .all(|(_, descriptor)| !descriptor.contains('L') && !descriptor.contains('[')),
        "the fixture must not introduce public reference parameters or results"
    );

    let counter_constructor = emitted_method_code(&counter, "<init>");
    assert_opcode_sequence(counter_constructor, &[0x2a, 0xb7, 0x2a, 0x1b, 0xb5, 0xb1]);
    assert_method_reference(
        &counter,
        counter_constructor,
        0xb7,
        "java/lang/Object",
        "<init>",
        "()V",
    );
    assert_field_reference(
        &counter,
        counter_constructor,
        0xb5,
        "Counter",
        "current",
        "I",
    );
    assert_opcode_sequence(
        emitted_method_code(&counter, "increment"),
        &[
            0x2a, 0x4c, 0x2a, 0xb4, 0x04, 0x60, 0x3d, 0x2b, 0x1c, 0xb5, 0x2a, 0xb4, 0xac,
        ],
    );
    assert_field_reference(
        &counter,
        emitted_method_code(&counter, "increment"),
        0xb4,
        "Counter",
        "current",
        "I",
    );
    assert_opcode_sequence(emitted_method_code(&counter, "get"), &[0x2a, 0xb4, 0xac]);
    assert_field_reference(
        &counter,
        emitted_method_code(&counter, "get"),
        0xb4,
        "Counter",
        "current",
        "I",
    );

    assert_opcode_sequence(emitted_method_code(&client, "<init>"), &[0x2a, 0xb7, 0xb1]);
    assert_method_reference(
        &client,
        emitted_method_code(&client, "<init>"),
        0xb7,
        "java/lang/Object",
        "<init>",
        "()V",
    );
    let run = emitted_method_code(&client, "run");
    assert_opcode_sequence(
        run,
        &[
            0xbb, 0x59, 0x1b, 0xb7, 0x4d, 0x2c, 0xb6, 0x57, 0x2c, 0xb6, 0xac,
        ],
    );
    assert_invocation_sequence(
        &client,
        run,
        &[
            (0xb7, "Counter", "<init>", "(I)V"),
            (0xb6, "Counter", "increment", "()I"),
            (0xb6, "Counter", "get", "()I"),
        ],
    );
    let direct = emitted_method_code(&client, "direct");
    assert_opcode_sequence(direct, &[0xbb, 0x59, 0x10, 0xb7, 0xb6, 0xac]);
    assert_eq!(
        direct[5], 42,
        "direct passes the literal 42 to the constructor"
    );
    assert_invocation_sequence(
        &client,
        direct,
        &[
            (0xb7, "Counter", "<init>", "(I)V"),
            (0xb6, "Counter", "get", "()I"),
        ],
    );
    let independent = emitted_method_code(&client, "independent");
    assert_opcode_sequence(
        independent,
        &[
            0xbb, 0x59, 0x10, 0xb7, 0x4c, 0xbb, 0x59, 0x10, 0xb7, 0x4d, 0x2b, 0xb6, 0x57, 0x2b,
            0xb6, 0x2c, 0xb6, 0x60, 0xac,
        ],
    );
    assert_eq!(independent[5], 10, "left is initialized from 10");
    assert_eq!(
        independent[15], 20,
        "right is initialized independently from 20"
    );
    assert_invocation_sequence(
        &client,
        independent,
        &[
            (0xb7, "Counter", "<init>", "(I)V"),
            (0xb7, "Counter", "<init>", "(I)V"),
            (0xb6, "Counter", "increment", "()I"),
            (0xb6, "Counter", "get", "()I"),
            (0xb6, "Counter", "get", "()I"),
        ],
    );

    if require_jdk_or_skip() {
        verify_counter_objects_with_jdk(first_output.path());
    }
}

#[test]
fn real_reference_object_apis_compile_and_verify_end_to_end() {
    let first_output = TestOutputDirectory::new("reference-object-apis-first");
    let second_output = TestOutputDirectory::new("reference-object-apis-second");
    let source = fs::read_to_string(fixture_path("ReferenceObjectApis.scala"))
        .expect("read real-source reference API fixture");
    let first = compile_source_classes(&source, "ReferenceObjectApis.scala", first_output.path())
        .expect("Counter and CounterService compile through the real source pipeline");
    let second = compile_source_classes(&source, "ReferenceObjectApis.scala", second_output.path())
        .expect("a fresh source session compiles the same reference API fixture");

    let identities = |artifacts: &[EmittedArtifact]| {
        artifacts
            .iter()
            .map(|artifact| {
                (
                    artifact.class.internal_name().to_owned(),
                    artifact.class.bytes().to_vec(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(identities(&first), identities(&second));
    assert_eq!(
        first
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Counter", "CounterService"]
    );
    assert_eq!(
        first_output.class_files(),
        [
            first_output.path().join("Counter.class"),
            first_output.path().join("CounterService.class")
        ]
    );
    assert_eq!(
        second_output.class_files(),
        [
            second_output.path().join("Counter.class"),
            second_output.path().join("CounterService.class")
        ]
    );

    let counter = decode_emitted_class(&first[0]);
    let service = decode_emitted_class(&first[1]);
    assert!(counter.access_flags.is_public());
    assert!(service.access_flags.is_public());
    assert!(
        counter
            .methods
            .iter()
            .all(|method| method.access_flags.is_public())
    );
    assert!(
        service
            .methods
            .iter()
            .all(|method| method.access_flags.is_public())
    );
    assert!(
        counter
            .methods
            .iter()
            .all(|method| !method.access_flags.is_static())
    );
    assert!(
        service
            .methods
            .iter()
            .all(|method| !method.access_flags.is_static())
    );
    assert_eq!(
        methods_with_descriptors(&counter),
        [
            ("<init>".to_owned(), "(I)V".to_owned()),
            ("get".to_owned(), "()I".to_owned()),
            ("increment".to_owned(), "()I".to_owned()),
            ("self".to_owned(), "()LCounter;".to_owned()),
        ]
    );
    assert_eq!(
        methods_with_descriptors(&service),
        [
            ("<init>".to_owned(), "()V".to_owned()),
            ("create".to_owned(), "(I)LCounter;".to_owned()),
            ("passthrough".to_owned(), "(LCounter;)LCounter;".to_owned()),
            ("read".to_owned(), "(LCounter;)I".to_owned()),
            (
                "choose".to_owned(),
                "(ZLCounter;LCounter;)LCounter;".to_owned()
            ),
            ("incrementUntil".to_owned(), "(LCounter;I)I".to_owned()),
            ("roundTrip".to_owned(), "(I)I".to_owned()),
        ]
    );
    assert_opcode_sequence(emitted_method_code(&counter, "self"), &[0x2a, 0xb0]);
    assert_opcode_sequence(
        emitted_method_code(&service, "create"),
        &[0xbb, 0x59, 0x1b, 0xb7, 0xb0],
    );
    assert_method_reference(
        &service,
        emitted_method_code(&service, "create"),
        0xb7,
        "Counter",
        "<init>",
        "(I)V",
    );
    let create_code = emitted_method_code(&service, "create");
    let (new_offset, _) = decode_subset_instructions(create_code)
        .into_iter()
        .find(|(_, opcode)| *opcode == 0xbb)
        .expect("create emits a new instruction");
    let new_index = u16::from_be_bytes([create_code[new_offset + 1], create_code[new_offset + 2]]);
    let ConstantPoolEntry::Class { name_index } = service
        .constant_pool
        .get(ConstantPoolIndex(new_index))
        .expect("new instruction references a constant-pool entry")
    else {
        panic!("new must reference a CONSTANT_Class entry");
    };
    assert_eq!(service.constant_pool.utf8(*name_index), Ok("Counter"));
    assert_opcode_sequence(emitted_method_code(&service, "passthrough"), &[0x2b, 0xb0]);
    assert_invocation_sequence(
        &service,
        emitted_method_code(&service, "read"),
        &[(0xb6, "Counter", "get", "()I")],
    );
    assert_invocation_sequence(
        &service,
        emitted_method_code(&service, "incrementUntil"),
        &[
            (0xb6, "Counter", "get", "()I"),
            (0xb6, "Counter", "increment", "()I"),
            (0xb6, "Counter", "get", "()I"),
        ],
    );
    let choose = emitted_method_code(&service, "choose");
    let choose_frames = stack_map_frames(&service, "choose");
    let choose_targets = decode_branch_targets(choose)
        .into_iter()
        .map(|(_, _, target)| target)
        .collect::<Vec<_>>();
    assert!(!choose_targets.is_empty());
    assert!(
        choose_targets
            .iter()
            .all(|target| choose_frames.iter().any(|frame| frame.pc == *target))
    );
    assert!(choose_frames.iter().any(|frame| {
        frame
            .locals
            .contains(&StackMapVerificationType::Object("Counter".to_owned()))
            && frame.stack == [StackMapVerificationType::Object("Counter".to_owned())]
    }));
    let increment_frames = stack_map_frames(&service, "incrementUntil");
    let increment_code = emitted_method_code(&service, "incrementUntil");
    let increment_targets = decode_branch_targets(increment_code)
        .into_iter()
        .map(|(_, _, target)| target)
        .collect::<Vec<_>>();
    assert!(!increment_targets.is_empty());
    assert!(
        increment_targets
            .iter()
            .all(|target| increment_frames.iter().any(|frame| frame.pc == *target))
    );
    assert!(increment_frames.iter().any(|frame| {
        frame
            .locals
            .contains(&StackMapVerificationType::Object("Counter".to_owned()))
    }));
    let increment_method = service
        .methods
        .iter()
        .find(|method| service.constant_pool.utf8(method.name_index) == Ok("incrementUntil"))
        .unwrap();
    let [Attribute::Code(increment_body)] = increment_method.attributes.as_slice() else {
        panic!("incrementUntil has one Code attribute");
    };
    assert!(increment_body.max_stack >= 2);
    assert_eq!(increment_body.max_locals, 3);
    assert_invocation_sequence(
        &service,
        emitted_method_code(&service, "roundTrip"),
        &[
            (0xb6, "CounterService", "create", "(I)LCounter;"),
            (0xb6, "CounterService", "read", "(LCounter;)I"),
        ],
    );

    if require_jdk_or_skip() {
        verify_reference_object_apis_with_jdk(first_output.path());
    }
}

#[test]
fn parameterized_direct_super_constructors_preserve_source_arguments_and_verify() {
    let first_output = TestOutputDirectory::new("parameterized-super-first");
    let second_output = TestOutputDirectory::new("parameterized-super-second");
    let source = fs::read_to_string(fixture_path("ParameterizedSuper.scala"))
        .expect("read parameterized-super source fixture");
    let first = compile_source_classes(&source, "ParameterizedSuper.scala", first_output.path())
        .expect("parameterized direct-super constructor fixture compiles");
    let second = compile_source_classes(&source, "ParameterizedSuper.scala", second_output.path())
        .expect("fresh frontend session emits the same parameterized constructors");

    let identities = |artifacts: &[EmittedArtifact]| {
        artifacts
            .iter()
            .map(|artifact| {
                (
                    artifact.class.internal_name().to_owned(),
                    artifact.class.bytes().to_vec(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(identities(&first), identities(&second));
    assert_eq!(
        first
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        [
            "ParameterizedBase",
            "ParameterizedDerived",
            "LiteralBase",
            "LiteralDerived",
            "Dep",
            "ReferenceBase",
            "ReferenceDerived",
        ]
    );
    let classes = first
        .iter()
        .map(|artifact| {
            (
                artifact.class.internal_name(),
                decode_emitted_class(artifact),
            )
        })
        .collect::<Vec<_>>();
    let class = |name: &str| {
        &classes
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .expect("expected emitted class")
            .1
    };
    assert_eq!(
        methods_with_descriptors(class("ParameterizedDerived"))
            .iter()
            .find(|(name, _)| name == "<init>")
            .map(|(_, descriptor)| descriptor.as_str()),
        Some("(IZ)V")
    );
    let derived_constructor = emitted_method_code(class("ParameterizedDerived"), "<init>");
    assert_eq!(
        decode_subset_instructions(derived_constructor)
            .iter()
            .take(4)
            .map(|(_, opcode)| *opcode)
            .collect::<Vec<_>>(),
        [0x2a, 0x1b, 0x1c, 0xb7]
    );
    let decoded_derived = decode_subset_instructions(derived_constructor);
    let super_call = decoded_derived
        .iter()
        .position(|(_, opcode)| *opcode == 0xb7)
        .expect("derived constructor calls its parent");
    let field_initializer = decoded_derived
        .iter()
        .position(|(_, opcode)| *opcode == 0xb5)
        .expect("derived constructor initializes its source field");
    assert!(super_call < field_initializer);
    assert_method_reference(
        class("ParameterizedDerived"),
        derived_constructor,
        0xb7,
        "ParameterizedBase",
        "<init>",
        "(IZ)V",
    );
    let literal_constructor = emitted_method_code(class("LiteralDerived"), "<init>");
    assert_eq!(
        decode_subset_instructions(literal_constructor)
            .iter()
            .take(4)
            .map(|(_, opcode)| *opcode)
            .collect::<Vec<_>>(),
        [0x2a, 0x12, 0x04, 0xb7]
    );
    assert_method_reference(
        class("LiteralDerived"),
        literal_constructor,
        0xb7,
        "LiteralBase",
        "<init>",
        "(IZ)V",
    );
    let reference_constructor = emitted_method_code(class("ReferenceDerived"), "<init>");
    assert_eq!(
        decode_subset_instructions(reference_constructor)
            .iter()
            .take(3)
            .map(|(_, opcode)| *opcode)
            .collect::<Vec<_>>(),
        [0x2a, 0x2b, 0xb7]
    );
    assert_method_reference(
        class("ReferenceDerived"),
        reference_constructor,
        0xb7,
        "ReferenceBase",
        "<init>",
        "(LDep;)V",
    );
    if !require_jdk_or_skip() {
        return;
    }
    let classpath = first_output.path().to_str().expect("UTF-8 test path");
    let javap = Command::new("javap")
        .args(["-c", "-v", "-classpath", classpath, "ParameterizedDerived"])
        .output()
        .expect("start javap for parameterized direct-super class");
    assert!(
        javap.status.success(),
        "javap rejected ParameterizedDerived.class\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javap.stdout),
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap = String::from_utf8_lossy(&javap.stdout);
    assert!(javap.contains("invokespecial #"));
    assert!(javap.contains("ParameterizedBase.\"<init>\":(IZ)V"));
    assert!(javap.contains("descriptor: (IZ)V"));

    let harness = first_output.path().join("ParameterizedSuperHarness.java");
    fs::write(
        &harness,
        "public final class ParameterizedSuperHarness { public static void main(String[] args) { ParameterizedDerived value = new ParameterizedDerived(42, true); if (value.value() != 42 || !value.isActive() || value.originalValue() != 42) throw new AssertionError(\"primitive parameters\"); LiteralDerived literal = new LiteralDerived(); if (literal.value() != 123456 || !literal.isActive()) throw new AssertionError(\"literal arguments\"); new ReferenceDerived(new Dep()); System.out.println(\"parameterized-super=ok\"); } }\n",
    )
    .expect("write parameterized-super JVM harness");
    let javac = Command::new("javac")
        .args(["-classpath", classpath, "-d", classpath])
        .arg(&harness)
        .output()
        .expect("start javac for parameterized-super harness");
    assert!(
        javac.status.success(),
        "javac failed for parameterized-super harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            classpath,
            "ParameterizedSuperHarness",
        ])
        .output()
        .expect("start java with full verifier for parameterized-super harness");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected parameterized-super classes\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "parameterized-super=ok"
    );
}

#[test]
fn reference_holder_fixture_compiles_deterministically_from_fresh_frontends() {
    let first_output = TestOutputDirectory::new("reference-holder-first");
    let second_output = TestOutputDirectory::new("reference-holder-second");
    let first = compile_fixture_classes("ReferenceHolder.scala", first_output.path())
        .expect("real Scala reference holder fixture compiles");
    let second = compile_fixture_classes("ReferenceHolder.scala", second_output.path())
        .expect("an independent frontend session compiles the reference holder fixture");
    let identities = |artifacts: &[EmittedArtifact]| {
        artifacts
            .iter()
            .map(|artifact| {
                (
                    artifact.class.internal_name().to_owned(),
                    artifact.class.bytes().to_vec(),
                )
            })
            .collect::<Vec<_>>()
    };

    assert_eq!(identities(&first), identities(&second));
    assert_eq!(
        first
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Counter", "Holder", "HolderClient"]
    );
    for (output, artifacts) in [(&first_output, &first), (&second_output, &second)] {
        assert_eq!(
            output.class_files(),
            [
                output.path().join("Counter.class"),
                output.path().join("Holder.class"),
                output.path().join("HolderClient.class")
            ]
        );
        for artifact in artifacts {
            decode_emitted_class(artifact);
        }
    }

    let counter = decode_emitted_class(&first[0]);
    let holder = decode_emitted_class(&first[1]);
    let client = decode_emitted_class(&first[2]);
    for class in [&counter, &holder, &client] {
        assert!(class.access_flags.is_public());
        assert!(
            class.methods.iter().all(|method| {
                method.access_flags.is_public() && !method.access_flags.is_static()
            })
        );
    }
    assert_eq!(
        methods_with_descriptors(&counter),
        [
            ("<init>".to_owned(), "(I)V".to_owned()),
            ("get".to_owned(), "()I".to_owned()),
            ("increment".to_owned(), "()I".to_owned()),
        ]
    );
    assert_eq!(
        methods_with_descriptors(&holder),
        [
            ("<init>".to_owned(), "(LCounter;Z)V".to_owned()),
            ("value".to_owned(), "()LCounter;".to_owned()),
            ("initial".to_owned(), "()LCounter;".to_owned()),
            ("replace".to_owned(), "(LCounter;)V".to_owned()),
            ("read".to_owned(), "()I".to_owned()),
            ("isEnabled".to_owned(), "()Z".to_owned()),
            ("choose".to_owned(), "(ZLCounter;)LCounter;".to_owned()),
            ("incrementUntil".to_owned(), "(I)I".to_owned()),
        ]
    );
    assert_eq!(
        methods_with_descriptors(&client),
        [
            ("<init>".to_owned(), "()V".to_owned()),
            ("wrap".to_owned(), "(LCounter;)LHolder;".to_owned()),
            ("run".to_owned(), "(II)I".to_owned()),
        ]
    );
    let holder_fields = holder
        .fields
        .iter()
        .map(|field| {
            (
                holder.constant_pool.utf8(field.name_index).unwrap(),
                holder.constant_pool.utf8(field.descriptor_index).unwrap(),
                field.access_flags.is_private(),
                field.access_flags.is_final(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        holder_fields,
        [
            ("original", "LCounter;", true, true),
            ("current", "LCounter;", true, false),
            ("enabledFlag", "Z", true, true),
        ]
    );
    let counter_fields = counter
        .fields
        .iter()
        .map(|field| {
            (
                counter.constant_pool.utf8(field.name_index).unwrap(),
                counter.constant_pool.utf8(field.descriptor_index).unwrap(),
                field.access_flags.is_private(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(counter_fields, [("count", "I", true)]);

    assert_invocation_sequence(
        &holder,
        emitted_method_code(&holder, "<init>"),
        &[(0xb7, "java/lang/Object", "<init>", "()V")],
    );
    assert_opcode_sequence(
        emitted_method_code(&counter, "<init>"),
        &[0x2a, 0xb7, 0x2a, 0x1b, 0xb5, 0xb1],
    );
    assert_opcode_sequence(
        emitted_method_code(&holder, "<init>"),
        &[
            0x2a, 0xb7, 0x2a, 0x2b, 0xb5, 0x2a, 0x2b, 0xb5, 0x2a, 0x1c, 0xb5, 0xb1,
        ],
    );
    assert_opcode_sequence(emitted_method_code(&holder, "value"), &[0x2a, 0xb4, 0xb0]);
    assert_opcode_sequence(emitted_method_code(&holder, "initial"), &[0x2a, 0xb4, 0xb0]);
    assert_opcode_sequence(
        emitted_method_code(&holder, "replace"),
        &[0x2a, 0x4d, 0x2b, 0x4e, 0x2c, 0x2d, 0xb5, 0xb1],
    );
    assert_opcode_sequence(
        emitted_method_code(&holder, "isEnabled"),
        &[0x2a, 0xb4, 0xac],
    );
    let holder_constructor = emitted_method_code(&holder, "<init>");
    let initialized_fields = decode_subset_instructions(holder_constructor)
        .into_iter()
        .filter(|(_, opcode)| *opcode == 0xb5)
        .map(|(offset, _)| {
            let index = u16::from_be_bytes([
                holder_constructor[offset + 1],
                holder_constructor[offset + 2],
            ]);
            let ConstantPoolEntry::Fieldref {
                class_index,
                name_and_type_index,
            } = holder
                .constant_pool
                .get(ConstantPoolIndex(index))
                .expect("putfield references a field")
            else {
                panic!("putfield must reference Fieldref")
            };
            let owner = holder.constant_pool.class_name(*class_index).unwrap();
            let (name, descriptor) = holder
                .constant_pool
                .name_and_type(*name_and_type_index)
                .unwrap();
            (owner, name, descriptor)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        initialized_fields,
        [
            ("Holder", "original", "LCounter;"),
            ("Holder", "current", "LCounter;"),
            ("Holder", "enabledFlag", "Z"),
        ]
    );
    assert_invocation_sequence(
        &holder,
        emitted_method_code(&holder, "read"),
        &[
            (0xb6, "Holder", "value", "()LCounter;"),
            (0xb6, "Counter", "get", "()I"),
        ],
    );
    assert_invocation_sequence(
        &holder,
        emitted_method_code(&holder, "incrementUntil"),
        &[
            (0xb6, "Holder", "value", "()LCounter;"),
            (0xb6, "Counter", "get", "()I"),
            (0xb6, "Holder", "value", "()LCounter;"),
            (0xb6, "Counter", "increment", "()I"),
            (0xb6, "Holder", "value", "()LCounter;"),
            (0xb6, "Counter", "get", "()I"),
        ],
    );
    let increment_code = emitted_method_code(&holder, "incrementUntil");
    let increment_frames = stack_map_frames(&holder, "incrementUntil");
    let increment_targets = decode_branch_targets(increment_code)
        .into_iter()
        .map(|(_, _, target)| target)
        .collect::<Vec<_>>();
    assert!(!increment_targets.is_empty());
    assert!(
        increment_targets
            .iter()
            .all(|target| increment_frames.iter().any(|frame| frame.pc == *target))
    );
    assert!(increment_frames.iter().any(|frame| {
        frame
            .locals
            .contains(&StackMapVerificationType::Object("Holder".to_owned()))
    }));
    let choose_frames = stack_map_frames(&holder, "choose");
    let choose_targets = decode_branch_targets(emitted_method_code(&holder, "choose"))
        .into_iter()
        .map(|(_, _, target)| target)
        .collect::<Vec<_>>();
    assert!(!choose_targets.is_empty());
    assert!(
        choose_targets
            .iter()
            .all(|target| choose_frames.iter().any(|frame| frame.pc == *target))
    );
    assert!(choose_frames.iter().any(|frame| {
        frame
            .locals
            .contains(&StackMapVerificationType::Object("Holder".to_owned()))
            && frame.stack == [StackMapVerificationType::Object("Counter".to_owned())]
    }));

    if require_jdk_or_skip() {
        verify_reference_holder_with_jdk(first_output.path());
    }
}

#[test]
fn unsupported_counter_object_source_cases_fail_at_their_pipeline_stage() {
    let cases = [
        (
            "private-constructor-call",
            "class CounterClient:\n  def run(seed: Int): Int = new Counter(seed).get()\nclass Counter private (seed: Int):\n  private val current: Int = seed\n  def get(): Int = current\n",
        ),
        (
            "wrong-constructor-arity",
            "class Counter(seed: Int)\nclass CounterClient:\n  def run(): Int = new Counter().get()\n",
        ),
        (
            "wrong-constructor-argument",
            "class Counter(seed: Int)\nclass CounterClient:\n  def run(): Int = new Counter(true).get()\n",
        ),
        (
            "wrong-constructor-reference-class",
            "class Counter\nclass Other\nclass Box(counter: Counter)\nclass Client:\n  def wrong(other: Other): Box = new Box(other)\n",
        ),
        (
            "branching-constructor-argument",
            "class Counter(seed: Int):\n  def get(): Int = 1\nclass CounterClient:\n  def run(flag: Boolean): Int = new Counter(if flag then 1 else 2).get()\n",
        ),
        (
            "reference-branch-constructor-argument",
            "class Counter\nclass Box(counter: Counter)\nclass Client:\n  def choose(flag: Boolean, counter: Counter): Box = new Box(if flag then counter else counter)\n",
        ),
        (
            "unplanned-object-class",
            "class CounterClient:\n  def run(): Int =\n    new Object()\n    1\n",
        ),
        (
            "null-literal",
            "class CounterClient:\n  def use(): Int = null\n",
        ),
        (
            "inferred-null-signature",
            "class Runner:\n  def unsupported() = null\n",
        ),
        (
            "array-method-parameter",
            "class CounterClient:\n  def use(values: Array[Int]): Int = 1\n",
        ),
        (
            "different-reference-join",
            "class Counter(start: Int)\nclass Other(start: Int)\nclass CounterClient:\n  def choose(flag: Boolean): Int =\n    val selected = if flag then new Counter(1) else new Other(2)\n    1\n",
        ),
        (
            "branching-nested-constructor-argument",
            "class Counter(seed: Int)\nclass Box(counter: Counter)\nclass Client:\n  def nested(flag: Boolean): Box = new Box(new Counter(if flag then 1 else 2))\n",
        ),
        (
            "third-simultaneous-nested-allocation",
            "class Leaf(seed: Int)\nclass Middle(leaf: Leaf)\nclass Outer(middle: Middle)\nclass Client:\n  def tooDeep(): Outer = new Outer(new Middle(new Leaf(1)))\n",
        ),
        (
            "reference-var-reassignment",
            "class Counter(seed: Int)\nclass CounterClient:\n  def run(seed: Int): Int =\n    var counter = new Counter(seed)\n    counter = new Counter(seed + 1)\n    counter.get()\n",
        ),
        (
            "mutable-reference-field-call",
            "class Counter:\n  def get(): Int = 1\nclass Holder:\n  private var current: Counter = new Counter()\n  def read(): Int = current.get()\n",
        ),
        (
            "wrong-class-reference-argument",
            "class Counter\nclass Other\nclass Client:\n  def accept(counter: Counter): Counter = counter\n  def wrong(other: Other): Counter = accept(other)\n",
        ),
        (
            "wrong-method-result-class",
            "class Counter\nclass Other\nclass Client:\n  def wrong(): Counter = new Other()\n",
        ),
        (
            "different-reference-if-result",
            "class Counter\nclass Other\nclass Client:\n  def wrong(flag: Boolean, counter: Counter, other: Other): Counter = if flag then counter else other\n",
        ),
        (
            "reference-field-initialized-by-factory",
            "class Counter(seed: Int)\nclass Factory(seed: Int):\n  def make(): Counter = new Counter(seed)\nclass Holder(seed: Int):\n  private val counter: Counter = make()\n",
        ),
        (
            "reference-field-initialized-by-branch-argument",
            "class Counter(seed: Int)\nclass Holder(seed: Int, flag: Boolean):\n  private val counter: Counter = new Counter(if flag then seed else 0)\n",
        ),
        (
            "reference-field-initialized-by-nested-new",
            "class Counter(seed: Int)\nclass Holder(counter: Counter)\nclass Owner(seed: Int):\n  private val holder: Holder = new Holder(new Counter(seed))\n",
        ),
        (
            "generic-class",
            "class Box[A](value: A)\nclass Client:\n  def use(): Int = 1\n",
        ),
    ];
    for (label, source) in cases {
        let output = TestOutputDirectory::new(label);
        let error = compile_source_classes(source, &format!("{label}.scala"), output.path())
            .expect_err("unsupported object source must fail closed");
        let accepted_stage = match label {
            "private-constructor-call" => matches!(
                &error,
                PipelineFailure::Codegen(JvmCodegenError::InaccessibleJvmConstructor { .. })
            ),
            "wrong-constructor-arity" => matches!(
                &error,
                PipelineFailure::Typer(TyperError::ApplicationArityMismatch {
                    expected: 1,
                    actual: 0,
                    ..
                })
            ),
            "wrong-constructor-argument" => matches!(
                &error,
                PipelineFailure::Typer(TyperError::ApplicationArgumentTypeMismatch {
                    argument_index: 0,
                    ..
                })
            ),
            "wrong-constructor-reference-class" => matches!(
                &error,
                PipelineFailure::Typer(TyperError::ApplicationArgumentTypeMismatch {
                    argument_index: 0,
                    ..
                })
            ),
            "branching-constructor-argument" => matches!(
                &error,
                PipelineFailure::Codegen(JvmCodegenError::UnsupportedJvmConstructorArgument { .. })
            ),
            "reference-branch-constructor-argument" => matches!(
                &error,
                PipelineFailure::Codegen(
                    JvmCodegenError::JvmConstructorCallArgumentTypeMismatch { .. }
                )
            ),
            "unplanned-object-class" => matches!(
                &error,
                PipelineFailure::Typer(TyperError::NewClassInfoUnavailable { .. })
            ),
            "null-literal" => matches!(
                &error,
                PipelineFailure::Codegen(JvmCodegenError::UnsupportedTypedExpression {
                    node: "null literal with a non-reference expected type",
                    ..
                })
            ),
            "inferred-null-signature" => matches!(
                &error,
                PipelineFailure::Codegen(JvmCodegenError::UnsupportedJvmReferenceType {
                    kind: dotty_jvm::UnsupportedJvmReferenceTypeKind::ExternalClass,
                    ..
                })
            ),
            "array-method-parameter" => matches!(
                &error,
                PipelineFailure::Typer(TyperError::TypeNameNotFound { .. })
            ),
            "different-reference-join" => matches!(
                &error,
                PipelineFailure::Codegen(JvmCodegenError::UnsupportedJvmReferenceType {
                    kind: dotty_jvm::UnsupportedJvmReferenceTypeKind::NotCanonicalTypeRef,
                    ..
                })
            ),
            "branching-nested-constructor-argument" | "third-simultaneous-nested-allocation" => {
                matches!(
                    &error,
                    PipelineFailure::Codegen(
                        JvmCodegenError::UnsupportedJvmConstructorArgument { .. }
                    )
                )
            }
            "reference-field-initialized-by-factory" => matches!(
                &error,
                PipelineFailure::Typer(TyperError::TermNameNotFound { .. })
                    | PipelineFailure::Codegen(
                        JvmCodegenError::UnsupportedJvmReferenceFieldInitializer { .. }
                    )
            ),
            "reference-field-initialized-by-branch-argument"
            | "reference-field-initialized-by-nested-new" => matches!(
                &error,
                PipelineFailure::Codegen(
                    JvmCodegenError::UnsupportedConstructorFieldInitializer { .. }
                )
            ),
            "reference-var-reassignment" => matches!(
                &error,
                PipelineFailure::Typer(TyperError::UnstableSelectionPrefix { .. })
            ),
            "mutable-reference-field-call" => matches!(
                &error,
                PipelineFailure::Typer(TyperError::UnstableSelectionPrefix { .. })
            ),
            "wrong-class-reference-argument" => matches!(
                &error,
                PipelineFailure::Typer(TyperError::ApplicationArgumentTypeMismatch {
                    argument_index: 0,
                    ..
                })
            ),
            "wrong-method-result-class" => matches!(
                &error,
                PipelineFailure::Codegen(JvmCodegenError::MethodBodyTypeMismatch { .. })
            ),
            "different-reference-if-result" => matches!(
                &error,
                PipelineFailure::Codegen(JvmCodegenError::UnsupportedSiblingUnionProjection { .. })
            ),
            "generic-class" => matches!(
                &error,
                PipelineFailure::Codegen(JvmCodegenError::InvalidPrimaryConstructor {
                    issue: dotty_jvm::JvmConstructorIssue::SourceShape,
                    ..
                })
            ),
            _ => unreachable!("all cases have an expected pipeline stage"),
        };
        assert!(
            accepted_stage,
            "{label} was rejected at an unexpected pipeline stage: {error:?}"
        );
        assert!(
            output.class_files().is_empty(),
            "{label} failure must not publish any classfile"
        );
    }
}

#[test]
fn typed_null_reference_flows_emit_verifiable_null_values() {
    let output = TestOutputDirectory::new("typed-null-reference-flows");
    let second_output = TestOutputDirectory::new("typed-null-reference-flows-second");
    let artifacts = compile_fixture_classes("TypedNullReference.scala", output.path())
        .expect("real source-typed null contexts compile");
    let second_artifacts =
        compile_fixture_classes("TypedNullReference.scala", second_output.path())
            .expect("a fresh source session compiles the typed-null fixture");
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Box", "Runner"]
    );
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| (artifact.class.internal_name(), artifact.class.bytes()))
            .collect::<Vec<_>>(),
        second_artifacts
            .iter()
            .map(|artifact| (artifact.class.internal_name(), artifact.class.bytes()))
            .collect::<Vec<_>>(),
        "the typed-null fixture emits deterministic class bytes across fresh sessions"
    );
    let runner_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Runner")
        .expect("Runner class is emitted");
    let mut reader = Reader::new(runner_artifact.class.bytes());
    let runner = ClassFile::decode(&mut reader).expect("decode Runner class");
    runner
        .validate_references()
        .expect("null bytecode has valid constant-pool references");

    for method_name in [
        "empty",
        "emptyBlock",
        "bothNull",
        "passNullToFormal",
        "passBlockNullToFormal",
        "blockNullLocal",
        "choose",
        "chooseReverse",
        "replace",
        "clear",
        "keepNull",
    ] {
        let method = runner
            .methods
            .iter()
            .find(|method| runner.constant_pool.utf8(method.name_index) == Ok(method_name))
            .unwrap_or_else(|| panic!("missing {method_name} method"));
        assert_eq!(
            runner.constant_pool.utf8(method.descriptor_index),
            Ok(match method_name {
                "empty"
                | "emptyBlock"
                | "passNullToFormal"
                | "passBlockNullToFormal"
                | "blockNullLocal" => "()LBox;",
                "bothNull" => "(Z)LBox;",
                "choose" | "chooseReverse" | "replace" | "clear" => "(ZLBox;)LBox;",
                "keepNull" => "(Z)LBox;",
                _ => unreachable!(),
            }),
            "{method_name} keeps its declared Box descriptor"
        );
        assert_eq!(
            decode_subset_instructions(emitted_method_code(&runner, method_name))
                .iter()
                .filter(|(_, opcode)| *opcode == 0x01)
                .count(),
            if method_name == "bothNull" { 2 } else { 1 },
            "{method_name} emits one aconst_null for its one null literal"
        );
    }
    for method_name in ["choose", "chooseReverse", "replace", "clear"] {
        let frames = stack_map_frames(&runner, method_name);
        assert!(
            frames.iter().any(|frame| frame
                .locals
                .contains(&StackMapVerificationType::Object("Box".to_owned()))),
            "{method_name} frame retains the resolved Box local/class identity: {frames:?}"
        );
    }
    let keep_null_frames = stack_map_frames(&runner, "keepNull");
    assert!(
        keep_null_frames
            .iter()
            .any(|frame| frame.locals.contains(&StackMapVerificationType::Null)),
        "a local that remains null across the branch keeps verifier Null: {keep_null_frames:?}"
    );

    if require_jdk_or_skip() {
        let classpath = output.path().to_str().expect("UTF-8 test path");
        let harness = output.path().join("TypedNullReferenceHarness.java");
        fs::write(
            &harness,
            r#"public final class TypedNullReferenceHarness {
  public static void main(String[] args) {
    Runner runner = new Runner();
    Box box = new Box(7);
    if (runner.empty() != null || runner.emptyBlock() != null
        || runner.passNullToFormal() != null || runner.passBlockNullToFormal() != null
        || runner.blockNullLocal() != null)
      throw new AssertionError("null return or reference formal");
    if (runner.bothNull(true) != null || runner.bothNull(false) != null)
      throw new AssertionError("both-null If result");
    if (runner.choose(true, box) != null || runner.choose(false, box) != box)
      throw new AssertionError("null-first If result");
    if (runner.chooseReverse(true, box) != box || runner.chooseReverse(false, box) != null)
      throw new AssertionError("null-second If result");
    if (runner.replace(false, box) != null || runner.replace(true, box) != box)
      throw new AssertionError("null-initialized mutable local");
    if (runner.clear(true, box) != null || runner.clear(false, box) != box)
      throw new AssertionError("mutable local assigned null");
    if (runner.keepNull(true) != null || runner.keepNull(false) != null)
      throw new AssertionError("null local preserved across a branch");
  }
}
"#,
        )
        .expect("write typed-null verifier harness");
        let javac = Command::new("javac")
            .args(["--release", "21", "-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac for typed-null verifier harness");
        assert!(
            javac.status.success(),
            "javac rejected typed-null harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "TypedNullReferenceHarness",
            ])
            .output()
            .expect("start Java verifier for typed-null classes");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected typed-null classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
    }
}

#[test]
fn anyref_identity_uses_reference_branches_and_verifies_runtime_behavior() {
    let output = TestOutputDirectory::new("anyref-identity");
    let artifacts = compile_fixture_classes("AnyRefIdentity.scala", output.path())
        .expect("real canonical AnyRef.eq/ne calls compile");
    let box_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Box")
        .expect("Box class is emitted");
    let runner_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Runner")
        .expect("Runner class is emitted");
    let mut box_reader = Reader::new(box_artifact.class.bytes());
    let boxed = ClassFile::decode(&mut box_reader).expect("decode Box class");
    let mut runner_reader = Reader::new(runner_artifact.class.bytes());
    let runner = ClassFile::decode(&mut runner_reader).expect("decode Runner class");
    runner
        .validate_references()
        .expect("identity bytecode has valid constant-pool references");

    for (method_name, descriptor, opcode) in [
        ("same", "(LBox;LBox;)Z", 0xa5),
        ("different", "(LBox;LBox;)Z", 0xa6),
        ("missing", "(LBox;)Z", 0xa5),
        ("present", "(LBox;)Z", 0xa6),
    ] {
        let method = runner
            .methods
            .iter()
            .find(|method| runner.constant_pool.utf8(method.name_index) == Ok(method_name))
            .unwrap_or_else(|| panic!("missing {method_name} method"));
        assert_eq!(
            runner.constant_pool.utf8(method.descriptor_index),
            Ok(descriptor),
            "{method_name} retains its Boolean source descriptor"
        );
        let code = emitted_method_code(&runner, method_name);
        let instructions = decode_subset_instructions(code);
        if matches!(method_name, "same" | "different") {
            assert_eq!(
                instructions
                    .iter()
                    .take(2)
                    .map(|(_, opcode)| *opcode)
                    .collect::<Vec<_>>(),
                [0x2b, 0x2c],
                "{method_name} loads its receiver before its argument, once each"
            );
            assert_eq!(
                instructions
                    .iter()
                    .filter(|(_, opcode)| matches!(*opcode, 0x2b | 0x2c))
                    .count(),
                2,
                "{method_name} evaluates each parameter exactly once"
            );
        }
        assert_eq!(
            instructions
                .iter()
                .filter(|(_, found)| *found == opcode)
                .count(),
            1,
            "{method_name} uses the corresponding reference identity branch"
        );
        assert!(
            !instructions.iter().any(|(_, found)| *found == 0xb6),
            "{method_name} does not dispatch identity through invokevirtual"
        );
        let targets = decode_branch_targets(code);
        assert!(targets.iter().all(|(_, _, target)| {
            stack_map_frames(&runner, method_name)
                .iter()
                .any(|frame| frame.pc == *target)
        }));
    }

    let box_value = boxed
        .methods
        .iter()
        .find(|method| boxed.constant_pool.utf8(method.name_index) == Ok("value"))
        .expect("Box.value method exists");
    assert_eq!(
        boxed.constant_pool.utf8(box_value.descriptor_index),
        Ok("()I")
    );

    if require_jdk_or_skip() {
        let classpath = output.path().to_str().expect("UTF-8 test path");
        let harness = output.path().join("AnyRefIdentityHarness.java");
        fs::write(
            &harness,
            r#"public final class AnyRefIdentityHarness {
  public static void main(String[] args) {
    Runner runner = new Runner();
    Box first = new Box(10);
    Box second = new Box(10);
    if (!runner.same(first, first) || runner.same(first, second))
      throw new AssertionError("eq must use object identity");
    if (first.value() != 10 || second.value() != 10)
      throw new AssertionError("distinct boxes carry equal values");
    if (!runner.different(first, second) || runner.different(first, first))
      throw new AssertionError("ne must use object identity");
    if (runner.missing(first) || !runner.missing(null))
      throw new AssertionError("eq with null");
    if (!runner.blockMissing(null) || runner.blockMissing(first))
      throw new AssertionError("eq with a block-null operand");
    if (!runner.bothMissing(null, true) || runner.bothMissing(first, false))
      throw new AssertionError("eq with a both-null If operand");
    if (!runner.maybeMissing(null, second, true) || runner.maybeMissing(first, second, false))
      throw new AssertionError("eq with a mixed-null If argument");
    if (!runner.maybeReceiver(false, first) || runner.maybeReceiver(true, first))
      throw new AssertionError("eq with a mixed-null If receiver");
    if (!runner.present(first) || runner.present(null))
      throw new AssertionError("ne with null");
    if (!runner.choose(true, first, first) || runner.choose(false, first, first))
      throw new AssertionError("identity result is usable as an If condition");
    if (!runner.clearUntilNull(first) || runner.clearUntilNull(null) != true)
      throw new AssertionError("identity result is usable as a While condition");
  }
}
"#,
        )
        .expect("write AnyRef identity harness");
        let javac = Command::new("javac")
            .args(["--release", "21", "-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac for AnyRef identity harness");
        assert!(
            javac.status.success(),
            "javac rejected AnyRef identity harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "AnyRefIdentityHarness",
            ])
            .output()
            .expect("start Java verifier for AnyRef identity classes");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected AnyRef identity classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
    }
}

#[test]
fn null_identity_jdk21_acceptance_fixture_is_canonical_verified_and_deterministic() {
    let output = TestOutputDirectory::new("null-identity-acceptance");
    let second_output = TestOutputDirectory::new("null-identity-acceptance-second");
    let source = fs::read_to_string(fixture_path("NullIdentityAcceptance.scala"))
        .expect("read pinned null identity source fixture");
    let mut semantic_proof_ran = false;
    let mut observe = |unit: &mut JvmCompilationUnit<'_>, definitions: &Definitions| {
        let box_class = audit_class_named(unit, "Box");
        let runner_class = audit_class_named(unit, "Runner");
        assert_eq!(
            audit_class_parent(unit, box_class),
            definitions.object_class
        );
        assert_eq!(
            audit_class_parent(unit, runner_class),
            definitions.object_class
        );
        let identity = definitions
            .any_ref_identity_operator_symbols(unit.store)
            .expect("canonical eq/ne semantic identities remain registered");

        for (method, selected) in [
            ("same", identity.eq),
            ("different", identity.ne),
            ("missing", identity.eq),
            ("readOrDefault", identity.eq),
        ] {
            let body = audit_method_named(unit, runner_class, method)
                .body
                .unwrap_or_else(|| panic!("{method} has a typed source body"));
            assert!(
                typed_tree_targets(unit, body).contains(&selected),
                "{method} selects the canonical {:?} method symbol",
                selected
            );
        }

        for method in ["choose", "chooseReverse", "empty", "update", "keepNull"] {
            let plan = audit_method_named(unit, runner_class, method);
            let signature = match unit.store.symbols.info(plan.method) {
                dotty_core::SymbolInfo::Complete(ty) => match unit.store.types.get(*ty) {
                    Type::Method(signature) => signature,
                    other => panic!("{method} has method type, found {other:?}"),
                },
                other => panic!("{method} has completed method info, found {other:?}"),
            };
            assert_eq!(
                audit_type_symbol(unit, signature.result),
                Some(box_class),
                "{method} retains its declared canonical Box result"
            );
        }

        let null_literals = unit
            .typed_ast
            .iter()
            .filter_map(|(_, tree)| match &tree.kind {
                TreeKind::Literal(literal) if literal.value == Constant::Null => Some(tree.ty),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            null_literals.len() >= 6,
            "fixture types all null source paths"
        );
        assert!(null_literals.iter().all(|ty| *ty == definitions.null_type));
        semantic_proof_ran = true;
    };
    let artifacts = compile_source_batch_observed(
        &source,
        "NullIdentityAcceptance.scala",
        output.path(),
        SourceId::from_index(1475),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut observe),
    )
    .expect("real Scala null identity source compiles through the full pipeline");
    assert!(
        semantic_proof_ran,
        "semantic proof ran before class emission"
    );
    let second_artifacts =
        compile_fixture_classes("NullIdentityAcceptance.scala", second_output.path())
            .expect("an independent frontend session compiles the same fixture");
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Box", "Runner"]
    );
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| (artifact.class.internal_name(), artifact.class.bytes()))
            .collect::<Vec<_>>(),
        second_artifacts
            .iter()
            .map(|artifact| (artifact.class.internal_name(), artifact.class.bytes()))
            .collect::<Vec<_>>(),
        "fresh source sessions emit byte-identical classes in stable order"
    );

    fn decode(artifact: &EmittedArtifact) -> ClassFile<'_> {
        let mut reader = Reader::new(artifact.class.bytes());
        let class = ClassFile::decode(&mut reader).expect("decode emitted classfile");
        class
            .validate_references()
            .expect("classfile constant-pool and attribute references validate");
        assert_eq!(class.version.major, 65, "classfile targets Java 21");
        assert_eq!(
            class
                .constant_pool
                .class_name(class.this_class)
                .expect("this_class resolves"),
            artifact.class.internal_name()
        );
        assert_eq!(
            class
                .super_class
                .and_then(|index| class.constant_pool.class_name(index).ok()),
            Some("java/lang/Object")
        );
        class
    }
    let boxed = decode(
        artifacts
            .iter()
            .find(|artifact| artifact.class.internal_name() == "Box")
            .expect("Box class emitted"),
    );
    let runner = decode(
        artifacts
            .iter()
            .find(|artifact| artifact.class.internal_name() == "Runner")
            .expect("Runner class emitted"),
    );

    let field = boxed
        .fields
        .iter()
        .find(|field| boxed.constant_pool.utf8(field.name_index) == Ok("current"))
        .expect("Box.current field emitted");
    assert!(field.access_flags.is_private());
    assert_eq!(boxed.constant_pool.utf8(field.descriptor_index), Ok("I"));
    let method_descriptors = [
        ("same", "(LBox;LBox;)Z"),
        ("different", "(LBox;LBox;)Z"),
        ("missing", "(LBox;)Z"),
        ("choose", "(ZLBox;)LBox;"),
        ("chooseReverse", "(ZLBox;)LBox;"),
        ("empty", "()LBox;"),
        ("update", "(ZLBox;)LBox;"),
        ("keepNull", "(Z)LBox;"),
        ("readOrDefault", "(ZLBox;)I"),
    ];
    assert!(
        boxed.methods.iter().any(|method| {
            boxed.constant_pool.utf8(method.name_index) == Ok("<init>")
                && boxed.constant_pool.utf8(method.descriptor_index) == Ok("(I)V")
        }),
        "Box exposes its source primary constructor"
    );
    assert!(
        runner.methods.iter().any(|method| {
            runner.constant_pool.utf8(method.name_index) == Ok("<init>")
                && runner.constant_pool.utf8(method.descriptor_index) == Ok("()V")
        }),
        "Runner exposes its no-argument constructor"
    );
    for (name, descriptor) in method_descriptors {
        let method = runner
            .methods
            .iter()
            .find(|method| runner.constant_pool.utf8(method.name_index) == Ok(name))
            .unwrap_or_else(|| panic!("Runner.{name} emitted"));
        assert!(method.access_flags.is_public(), "Runner.{name} is public");
        assert_eq!(
            runner.constant_pool.utf8(method.descriptor_index),
            Ok(descriptor)
        );
        assert!(code_max_stack(&runner, name) > 0);
        assert!(code_max_locals(&runner, name) > 0);
        let code = emitted_method_code(&runner, name);
        let targets = decode_branch_targets(code);
        if !targets.is_empty() {
            let frames = stack_map_frames(&runner, name);
            assert!(
                targets
                    .iter()
                    .all(|(_, _, target)| frames.iter().any(|frame| frame.pc == *target)),
                "all branch targets in {name} have mapped StackMapTable frames"
            );
        }
    }
    for (name, opcode, parameter_loads) in [
        ("same", 0xa5, [0x2b, 0x2c]),
        ("different", 0xa6, [0x2b, 0x2c]),
    ] {
        let instructions = decode_subset_instructions(emitted_method_code(&runner, name));
        assert_eq!(
            instructions
                .iter()
                .take(2)
                .map(|(_, op)| *op)
                .collect::<Vec<_>>(),
            parameter_loads,
            "{name} evaluates receiver then argument exactly once"
        );
        assert_eq!(
            instructions.iter().filter(|(_, op)| *op == opcode).count(),
            1
        );
        assert!(
            !instructions
                .iter()
                .any(|(_, op)| matches!(*op, 0xb6 | 0xc0))
        );
    }
    for name in ["choose", "chooseReverse"] {
        let code = emitted_method_code(&runner, name);
        let targets = decode_branch_targets(code);
        let frames = stack_map_frames(&runner, name);
        assert!(
            targets.iter().any(|(_, _, target)| {
                frames.iter().any(|frame| {
                    frame.pc == *target
                        && frame.stack == [StackMapVerificationType::Object("Box".into())]
                })
            }),
            "{name} joins Null and Object(Box) on the operand stack"
        );
    }
    assert!(
        stack_map_frames(&runner, "update")
            .iter()
            .any(|frame| frame.locals.get(3)
                == Some(&StackMapVerificationType::Object("Box".into()))),
        "update carries the initialized Object(Box) local through its branch join"
    );
    assert!(
        stack_map_frames(&runner, "keepNull")
            .iter()
            .any(
                |frame| frame.locals.contains(&StackMapVerificationType::Null)
                    || frame.stack.contains(&StackMapVerificationType::Null)
            ),
        "keepNull preserves verifier Null at a real branch join"
    );
    assert_eq!(
        decode_subset_instructions(emitted_method_code(&runner, "empty"))
            .iter()
            .filter(|(_, opcode)| *opcode == 0x01)
            .count(),
        1,
        "empty returns aconst_null without an invented object"
    );
    if require_jdk_or_skip() {
        let classpath = output.path().to_str().expect("UTF-8 test path");
        let javap = Command::new("javap")
            .args(["-classpath", classpath, "-c", "-v", "Runner"])
            .output()
            .expect("start required javap JVM audit");
        assert!(javap.status.success(), "javap -c -v Runner succeeds");
        let javap = String::from_utf8_lossy(&javap.stdout);
        assert!(javap.contains("if_acmpeq") && javap.contains("if_acmpne"));
        assert!(javap.contains("aconst_null"));
        assert!(!javap.contains("java/lang/Object.eq"));
        assert!(!javap.contains("java/lang/Object.ne"));
        assert!(!javap.contains("equals"));
        assert!(!javap.contains("checkcast"));

        let harness = output.path().join("NullReferenceIdentityHarness.java");
        fs::write(
            &harness,
            r#"public final class NullReferenceIdentityHarness {
  public static void main(String[] args) {
    Runner runner = new Runner();
    Box a = new Box(10);
    Box b = new Box(10);
    Box c = new Box(20);
    if (runner.same(a, a) != true || runner.same(a, b) != false
        || runner.different(a, b) != true || runner.different(a, a) != false)
      throw new AssertionError("identity differs from value equality");
    if (runner.missing(a) || !runner.missing(null))
      throw new AssertionError("eq null behavior");
    if (runner.choose(true, a) != null || runner.choose(false, a) != a
        || runner.chooseReverse(true, a) != a || runner.chooseReverse(false, a) != null)
      throw new AssertionError("nullable If results preserve object identity");
    if (runner.empty() != null || runner.update(false, a) != null
        || runner.update(true, a) != a || runner.keepNull(true) != null
        || runner.keepNull(false) != null)
      throw new AssertionError("null returns and local branch flows");
    if (runner.readOrDefault(true, a) != -1
        || runner.readOrDefault(false, a) != 10
        || runner.readOrDefault(false, c) != 20)
      throw new AssertionError("null-aware dispatch uses the actual instance");
  }
}
"#,
        )
        .expect("write acceptance harness");
        let javac = Command::new("javac")
            .args(["--release", "21", "-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac for required JDK 21 harness");
        assert!(
            javac.status.success(),
            "javac --release 21 failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "NullReferenceIdentityHarness",
            ])
            .output()
            .expect("start java -Xverify:all for acceptance fixture");
        assert!(
            java.status.success(),
            "java -Xverify:all failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
    }
}

#[test]
fn reference_constructor_arguments_lower_in_order_and_verify_initialized_values() {
    let output = TestOutputDirectory::new("reference-constructor-arguments");
    let artifacts = compile_planned_constructor_source(
        "class Counter(seed: Int):\n  private val value: Int = seed\n  def get(): Int = value\nclass Other()\nclass Holder(amount: Int, counter: Counter, enabled: Boolean):\n  private val saved: Counter = counter\n  private val savedAmount: Int = amount\n  private val savedEnabled: Boolean = enabled\n  def reference(): Counter = saved\n  def number(): Int = savedAmount\n  def flag(): Boolean = savedEnabled\nclass Pair(counter: Counter, other: Other):\n  private val savedCounter: Counter = counter\n  private val savedOther: Other = other\n  def first(): Counter = savedCounter\n  def second(): Other = savedOther\nclass OwnerBox(owner: Client):\n  private val savedOwner: Client = owner\n  def get(): Client = savedOwner\nclass Client:\n  def wrap(counter: Counter): Holder = new Holder(3, counter, true)\n  def wrapLocal(counter: Counter): Holder =\n    val saved = counter\n    new Holder(4, saved, false)\n  def wrapMutable(counter: Counter): Holder =\n    var current: Counter = counter\n    current = counter\n    new Holder(1, current, false)\n  def wrapNull(): Holder = new Holder(1, null, false)\n  def makeCounter(seed: Int): Counter = new Counter(seed)\n  def wrapFactory(seed: Int): Holder = new Holder(1, makeCounter(seed), true)\n  def pair(counter: Counter, other: Other): Pair = new Pair(counter, other)\n  def ownerBox(): OwnerBox = new OwnerBox(this)\nclass FieldClient(initial: Counter):\n  private val saved: Counter = initial\n  def wrap(): Holder = new Holder(1, saved, true)\n",
        output.path(),
    )
    .unwrap();
    let decode_class = |name: &str| {
        let artifact = artifacts
            .iter()
            .find(|artifact| artifact.class.internal_name() == name)
            .unwrap_or_else(|| panic!("{name} class is emitted"));
        let mut reader = Reader::new(artifact.class.bytes());
        let class = ClassFile::decode(&mut reader).expect("decode emitted class");
        class
            .validate_references()
            .expect("generated class references are valid");
        class
    };
    let holder = decode_class("Holder");
    let holder_constructor = holder
        .methods
        .iter()
        .find(|method| holder.constant_pool.utf8(method.name_index) == Ok("<init>"))
        .unwrap();
    assert_eq!(
        holder
            .constant_pool
            .utf8(holder_constructor.descriptor_index),
        Ok("(ILCounter;Z)V")
    );

    let client = decode_class("Client");
    let wrap = emitted_method_code(&client, "wrap");
    assert_opcode_sequence(wrap, &[0xbb, 0x59, 0x06, 0x2b, 0x04, 0xb7, 0xb0]);
    let wrap_local = emitted_method_code(&client, "wrapLocal");
    assert_opcode_sequence(
        wrap_local,
        &[0x2b, 0x4d, 0xbb, 0x59, 0x07, 0x2c, 0x03, 0xb7, 0xb0],
    );
    let pair = emitted_method_code(&client, "pair");
    assert_opcode_sequence(pair, &[0xbb, 0x59, 0x2b, 0x2c, 0xb7, 0xb0]);
    let owner_box = emitted_method_code(&client, "ownerBox");
    assert_opcode_sequence(owner_box, &[0xbb, 0x59, 0x2a, 0xb7, 0xb0]);
    let wrap_mutable = emitted_method_code(&client, "wrapMutable");
    assert_opcode_sequence(
        wrap_mutable,
        &[
            0x2b, 0x4d, 0x2b, 0x4d, 0xbb, 0x59, 0x04, 0x2c, 0x03, 0xb7, 0xb0,
        ],
    );
    let wrap_null = emitted_method_code(&client, "wrapNull");
    assert_opcode_sequence(wrap_null, &[0xbb, 0x59, 0x04, 0x01, 0x03, 0xb7, 0xb0]);
    let wrap_factory = emitted_method_code(&client, "wrapFactory");
    assert_opcode_sequence(
        wrap_factory,
        &[0xbb, 0x59, 0x04, 0x2a, 0x1b, 0xb6, 0x04, 0xb7, 0xb0],
    );
    let field_client = decode_class("FieldClient");
    let wrap_field = emitted_method_code(&field_client, "wrap");
    assert_opcode_sequence(
        wrap_field,
        &[0xbb, 0x59, 0x04, 0x2a, 0xb4, 0x04, 0xb7, 0xb0],
    );
    assert_method_reference(
        &client,
        wrap_factory,
        0xb6,
        "Client",
        "makeCounter",
        "(I)LCounter;",
    );
    assert_field_reference(
        &field_client,
        wrap_field,
        0xb4,
        "FieldClient",
        "saved",
        "LCounter;",
    );
    let assert_constructor_ref =
        |class: &ClassFile<'_>, code: &[u8], expected_owner: &str, descriptor: &str| {
            let instructions = decode_subset_instructions(code);
            let invokes = instructions
                .iter()
                .filter_map(|(offset, opcode)| (*opcode == 0xb7).then_some(*offset))
                .collect::<Vec<_>>();
            assert_eq!(invokes.len(), 1);
            let index = u16::from_be_bytes([code[invokes[0] + 1], code[invokes[0] + 2]]);
            let ConstantPoolEntry::Methodref {
                class_index,
                name_and_type_index,
            } = class
                .constant_pool
                .get(ConstantPoolIndex(index))
                .expect("constructor invocation is a Methodref")
            else {
                panic!("invokespecial points at Methodref")
            };
            assert_eq!(
                class.constant_pool.class_name(*class_index),
                Ok(expected_owner)
            );
            assert_eq!(
                class.constant_pool.name_and_type(*name_and_type_index),
                Ok(("<init>", descriptor))
            );
        };
    assert_constructor_ref(&client, wrap, "Holder", "(ILCounter;Z)V");
    assert_constructor_ref(&client, wrap_local, "Holder", "(ILCounter;Z)V");
    assert_constructor_ref(&client, pair, "Pair", "(LCounter;LOther;)V");
    assert_constructor_ref(&client, owner_box, "OwnerBox", "(LClient;)V");
    assert_constructor_ref(&client, wrap_mutable, "Holder", "(ILCounter;Z)V");
    assert_constructor_ref(&client, wrap_null, "Holder", "(ILCounter;Z)V");
    assert_constructor_ref(&client, wrap_factory, "Holder", "(ILCounter;Z)V");
    assert_constructor_ref(&field_client, wrap_field, "Holder", "(ILCounter;Z)V");

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 output path");
        let harness = output
            .path()
            .join("ReferenceConstructorArgumentsHarness.java");
        fs::write(
            &harness,
            "public final class ReferenceConstructorArgumentsHarness { public static void main(String[] args) { Client client = new Client(); Counter counter = new Counter(17); Other other = new Other(); Holder holder = client.wrap(counter); if (holder.reference() != counter || holder.number() != 3 || !holder.flag()) throw new AssertionError(\"mixed constructor arguments\"); Holder local = client.wrapLocal(counter); if (local.reference() != counter || local.number() != 4 || local.flag()) throw new AssertionError(\"initialized local argument\"); Holder mutable = client.wrapMutable(counter); if (mutable.reference() != counter || mutable.number() != 1 || mutable.flag()) throw new AssertionError(\"initialized mutable local argument\"); Holder nullable = client.wrapNull(); if (nullable.reference() != null) throw new AssertionError(\"null reference argument identity\"); Holder factory = client.wrapFactory(21); if (factory.reference().get() != 21) throw new AssertionError(\"factory reference argument\"); FieldClient fieldClient = new FieldClient(counter); Holder field = fieldClient.wrap(); if (field.reference() != counter) throw new AssertionError(\"current instance field argument\"); Pair pair = client.pair(counter, other); if (pair.first() != counter || pair.second() != other) throw new AssertionError(\"distinct reference arguments\"); OwnerBox owner = client.ownerBox(); if (owner.get() != client) throw new AssertionError(\"initialized this argument\"); } }\n",
        )
        .unwrap();
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness)
            .output()
            .unwrap();
        assert!(
            javac.status.success(),
            "javac rejected reference constructor argument harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "ReferenceConstructorArgumentsHarness",
            ])
            .output()
            .unwrap();
        assert!(
            java.status.success(),
            "java -Xverify:all rejected reference constructor arguments\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
    }
}

#[test]
fn reference_factory_subtype_can_feed_constructor_argument() {
    let output = TestOutputDirectory::new("reference-factory-subtype-constructor-argument");
    let artifacts = compile_planned_constructor_source(
        "class Base()\nclass Child() extends Base()\nclass Holder(value: Base)\nclass Runner:\n  def makeChild(): Child = new Child()\n  def makeHolder(): Holder = new Holder(makeChild())\n",
        output.path(),
    )
    .expect("a subtype returned by a factory is assignable to the constructor parameter");
    let runner_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Runner")
        .expect("Runner class is emitted");
    let runner = decode_emitted_class(runner_artifact);
    let method = emitted_method_code(&runner, "makeHolder");
    assert_method_reference(&runner, method, 0xb6, "Runner", "makeChild", "()LChild;");
    assert_method_reference(&runner, method, 0xb7, "Holder", "<init>", "(LBase;)V");
}

#[test]
fn nested_new_constructor_arguments_lower_lifo_and_verify_initialized_values() {
    let output = TestOutputDirectory::new("nested-new-constructor-arguments");
    let source = "class Counter(seed: Int):\n  private val value: Int = seed\n  def get(): Int = value\nclass Holder(counter: Counter):\n  private val saved: Counter = counter\n  def get(): Int = saved.get()\nclass Pair(first: Counter, second: Counter):\n  private val savedFirst: Counter = first\n  private val savedSecond: Counter = second\n  def firstValue(): Int = savedFirst.get()\n  def secondValue(): Int = savedSecond.get()\n  def firstCounter(): Counter = savedFirst\n  def secondCounter(): Counter = savedSecond\nclass Node(next: Node):\n  private val saved: Node = next\n  def nextNode(): Node = saved\nclass Client:\n  def create(seed: Int): Holder = new Holder(new Counter(seed))\n  def pair(first: Int, second: Int): Pair = new Pair(new Counter(first), new Counter(second))\n  def sameClass(): Node = new Node(new Node(null))\n";
    let artifacts = compile_planned_constructor_source(source, output.path()).unwrap();
    let second_output = TestOutputDirectory::new("nested-new-constructor-arguments-repeat");
    let second_artifacts =
        compile_planned_constructor_source(source, second_output.path()).unwrap();
    assert_eq!(artifacts.len(), second_artifacts.len());
    for (first, second) in artifacts.iter().zip(&second_artifacts) {
        assert_eq!(first.class.internal_name(), second.class.internal_name());
        assert_eq!(first.class.bytes(), second.class.bytes());
    }
    let client_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Client")
        .expect("Client class is emitted");
    let client = decode_emitted_class(client_artifact);
    let create = emitted_method_code(&client, "create");
    assert_opcode_sequence(create, &[0xbb, 0x59, 0xbb, 0x59, 0x1b, 0xb7, 0xb7, 0xb0]);
    let pair = emitted_method_code(&client, "pair");
    assert_opcode_sequence(
        pair,
        &[
            0xbb, 0x59, 0xbb, 0x59, 0x1b, 0xb7, 0xbb, 0x59, 0x1c, 0xb7, 0xb7, 0xb0,
        ],
    );
    let constructor_references = |code: &[u8]| {
        decode_subset_instructions(code)
            .into_iter()
            .filter_map(|(offset, opcode)| {
                if opcode != 0xb7 {
                    return None;
                }
                let index = u16::from_be_bytes([code[offset + 1], code[offset + 2]]);
                let ConstantPoolEntry::Methodref {
                    class_index,
                    name_and_type_index,
                } = client.constant_pool.get(ConstantPoolIndex(index))?
                else {
                    panic!("constructor invocation references a Methodref")
                };
                Some((
                    client.constant_pool.class_name(*class_index).unwrap(),
                    client
                        .constant_pool
                        .name_and_type(*name_and_type_index)
                        .unwrap(),
                ))
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        constructor_references(create),
        vec![
            ("Counter", ("<init>", "(I)V")),
            ("Holder", ("<init>", "(LCounter;)V"))
        ]
    );
    assert_eq!(
        constructor_references(pair),
        vec![
            ("Counter", ("<init>", "(I)V")),
            ("Counter", ("<init>", "(I)V")),
            ("Pair", ("<init>", "(LCounter;LCounter;)V")),
        ]
    );
    let same_class = emitted_method_code(&client, "sameClass");
    assert_opcode_sequence(
        same_class,
        &[0xbb, 0x59, 0xbb, 0x59, 0x01, 0xb7, 0xb7, 0xb0],
    );
    assert_eq!(
        constructor_references(same_class),
        vec![
            ("Node", ("<init>", "(LNode;)V")),
            ("Node", ("<init>", "(LNode;)V")),
        ]
    );

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 output path");
        let harness = output
            .path()
            .join("NestedNewConstructorArgumentsHarness.java");
        fs::write(
            &harness,
            "public final class NestedNewConstructorArgumentsHarness { public static void main(String[] args) { Client client = new Client(); if (client.create(10).get() != 10) throw new AssertionError(\"nested constructor argument\"); Pair pair = client.pair(7, 8); if (pair.firstValue() != 7 || pair.secondValue() != 8 || pair.firstCounter() == pair.secondCounter()) throw new AssertionError(\"sequential nested constructor arguments\"); Node node = client.sameClass(); if (node.nextNode() == null || node.nextNode().nextNode() != null) throw new AssertionError(\"same-class nested constructor arguments\"); } }\n",
        )
        .unwrap();
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness)
            .output()
            .unwrap();
        assert!(
            javac.status.success(),
            "javac rejected nested-new constructor harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "NestedNewConstructorArgumentsHarness",
            ])
            .output()
            .unwrap();
        assert!(
            java.status.success(),
            "java -Xverify:all rejected nested-new constructor harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        let javap = Command::new("javap")
            .args(["-c", "-v", "-classpath", output_path, "Client"])
            .output()
            .unwrap();
        assert!(
            javap.status.success(),
            "javap -c -v rejected the emitted Client class\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javap.stdout),
            String::from_utf8_lossy(&javap.stderr)
        );
        let javap_output = String::from_utf8_lossy(&javap.stdout);
        assert!(javap_output.contains("Counter.\"<init>\":(I)V"));
        assert!(javap_output.contains("Holder.\"<init>\":(LCounter;)V"));
        assert!(javap_output.contains("Pair.\"<init>\":(LCounter;LCounter;)V"));
        assert!(javap_output.contains("Node.\"<init>\":(LNode;)V"));
    }
}

#[test]
fn allocated_immutable_reference_local_can_be_a_constructor_argument() {
    let output = TestOutputDirectory::new("allocated-reference-constructor-argument");
    let artifacts = compile_planned_constructor_source(
        "class Counter(seed: Int)\nclass Holder(counter: Counter)\nclass Client:\n  def local(): Holder =\n    val c = new Counter(1)\n    new Holder(c)\n",
        output.path(),
    )
    .unwrap();
    let client = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Client")
        .unwrap();
    let mut reader = Reader::new(client.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("decode generated Client class");
    let local = emitted_method_code(&class, "local");
    assert_opcode_sequence(
        local,
        &[0xbb, 0x59, 0x04, 0xb7, 0x4c, 0xbb, 0x59, 0x2b, 0xb7, 0xb0],
    );
    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 output path");
        let harness = output.path().join("AllocatedLocalHarness.java");
        fs::write(
            &harness,
            "public final class AllocatedLocalHarness { public static void main(String[] args) { if (new Client().local() == null) throw new AssertionError(\"allocated local constructor argument\"); } }\n",
        )
        .unwrap();
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness)
            .output()
            .unwrap();
        assert!(
            javac.status.success(),
            "javac rejected allocated-local constructor argument harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "AllocatedLocalHarness",
            ])
            .output()
            .unwrap();
        assert!(
            java.status.success(),
            "java -Xverify:all rejected allocated-local constructor argument\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
    }
}

#[test]
fn typed_new_between_two_planned_source_classes_verifies_on_the_jvm() {
    let output = TestOutputDirectory::new("two-class-constructor");
    let artifacts = compile_planned_constructor_source(
        "class Counter(seed: Int)\nclass Client:\n  def build(seed: Int): Int =\n    new Counter(seed + 1)\n    42\n",
        output.path(),
    )
    .unwrap();
    assert_eq!(artifacts.len(), 2);
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Counter", "Client"]
    );
    let client = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Client")
        .unwrap();
    let mut reader = Reader::new(client.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("decode planned Client class");
    class
        .validate_references()
        .expect("validate Client constant pool references");
    let code = emitted_method_code(&class, "build");
    assert_opcode_sequence(
        code,
        &[0xbb, 0x59, 0x1b, 0x04, 0x60, 0xb7, 0x57, 0x10, 0xac],
    );
    assert!(code[code.len() - 2] == 42);
    let allocation_index = u16::from_be_bytes([code[1], code[2]]);
    let ConstantPoolEntry::Class { name_index } = class
        .constant_pool
        .get(ConstantPoolIndex(allocation_index))
        .unwrap()
    else {
        panic!("new must reference the planned Counter class")
    };
    assert_eq!(class.constant_pool.utf8(*name_index), Ok("Counter"));
    let (invoke_offset, _) = decode_subset_instructions(code)
        .into_iter()
        .find(|(_, opcode)| *opcode == 0xb7)
        .expect("constructor expression contains invokespecial");
    let invoke_index = u16::from_be_bytes([code[invoke_offset + 1], code[invoke_offset + 2]]);
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = class
        .constant_pool
        .get(ConstantPoolIndex(invoke_index))
        .unwrap()
    else {
        panic!("invokespecial must reference the planned constructor")
    };
    assert_eq!(class.constant_pool.class_name(*class_index), Ok("Counter"));
    assert_eq!(
        class.constant_pool.name_and_type(*name_and_type_index),
        Ok(("<init>", "(I)V"))
    );

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 output path");
        let harness = output.path().join("ConstructorHarness.java");
        fs::write(
            &harness,
            "public class ConstructorHarness { public static void main(String[] args) { System.out.println(new Client().build(10)); } }\n",
        )
        .unwrap();
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness)
            .output()
            .unwrap();
        assert!(
            javac.status.success(),
            "javac rejected two-class smoke harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "ConstructorHarness",
            ])
            .output()
            .unwrap();
        assert!(
            java.status.success(),
            "java -Xverify:all rejected two planned classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&java.stdout).trim(), "42");
    }
}

#[test]
fn virtual_calls_use_validated_new_and_planned_local_receivers() {
    let output = TestOutputDirectory::new("planned-virtual-calls");
    let artifacts = compile_planned_constructor_source(
        "class Counter(seed: Int):\n  def add(value: Int): Int = value + 1\n  def combine(left: Int, right: Int): Int = left * 10 + right\n  def get(): Int = 42\n  def enabled(): Boolean = true\n  def touch(): Unit = ()\n  private def hidden(): Int = 9\n  def privateCall(): Int = this.hidden()\n  def privateReceiver(seed: Int): Int = new Counter(seed).hidden()\nclass Other(seed: Int):\n  def get(): Int = 7\nclass Client:\n  def directGet(seed: Int): Int = new Counter(seed).get()\n  def fromNew(seed: Int): Int = new Counter(seed + 1).add(seed)\n  def ordered(seed: Int): Int = new Counter(seed).combine(seed + 1, seed + 2)\n  def booleanResult(seed: Int): Boolean = new Counter(seed).enabled()\n  def unitCall(seed: Int): Unit = new Counter(seed).touch()\n  def fromLocal(seed: Int): Int =\n    val counter = new Counter(seed)\n    counter.add(seed)\n  def fromMutableLocal(seed: Int): Int =\n    var counter: Counter = new Counter(seed)\n    counter.add(seed)\n  def fromOther(seed: Int): Int = new Other(seed).get()\n",
        output.path(),
    )
    .unwrap();
    let decode_class = |name: &str| {
        let artifact = artifacts
            .iter()
            .find(|artifact| artifact.class.internal_name() == name)
            .unwrap_or_else(|| panic!("{name} class is emitted"));
        let mut reader = Reader::new(artifact.class.bytes());
        let class = ClassFile::decode(&mut reader).expect("decode emitted class");
        class.validate_references().expect("valid class references");
        class
    };
    let client = decode_class("Client");
    let direct_get = emitted_method_code(&client, "directGet");
    assert_opcode_sequence(direct_get, &[0xbb, 0x59, 0x1b, 0xb7, 0xb6, 0xac]);
    let from_new = emitted_method_code(&client, "fromNew");
    assert_opcode_sequence(
        from_new,
        &[0xbb, 0x59, 0x1b, 0x04, 0x60, 0xb7, 0x1b, 0xb6, 0xac],
    );
    let ordered = emitted_method_code(&client, "ordered");
    assert_opcode_sequence(
        ordered,
        &[
            0xbb, 0x59, 0x1b, 0xb7, 0x1b, 0x04, 0x60, 0x1b, 0x05, 0x60, 0xb6, 0xac,
        ],
    );
    let boolean_result = emitted_method_code(&client, "booleanResult");
    assert_opcode_sequence(boolean_result, &[0xbb, 0x59, 0x1b, 0xb7, 0xb6, 0xac]);
    let unit_call = emitted_method_code(&client, "unitCall");
    assert_opcode_sequence(unit_call, &[0xbb, 0x59, 0x1b, 0xb7, 0xb6, 0xb1]);
    let from_local = emitted_method_code(&client, "fromLocal");
    assert_opcode_sequence(
        from_local,
        &[0xbb, 0x59, 0x1b, 0xb7, 0x4d, 0x2c, 0x1b, 0xb6, 0xac],
    );
    let from_mutable_local = emitted_method_code(&client, "fromMutableLocal");
    assert_opcode_sequence(
        from_mutable_local,
        &[0xbb, 0x59, 0x1b, 0xb7, 0x4d, 0x2c, 0x1b, 0xb6, 0xac],
    );
    let from_other = emitted_method_code(&client, "fromOther");
    assert_opcode_sequence(from_other, &[0xbb, 0x59, 0x1b, 0xb7, 0xb6, 0xac]);

    let invocation_owner = |class: &ClassFile<'_>, code: &[u8]| {
        let offset = decode_subset_instructions(code)
            .into_iter()
            .find_map(|(offset, opcode)| (opcode == 0xb6).then_some(offset))
            .expect("one invokevirtual instruction");
        let index = u16::from_be_bytes([code[offset + 1], code[offset + 2]]);
        let ConstantPoolEntry::Methodref { class_index, .. } = class
            .constant_pool
            .get(ConstantPoolIndex(index))
            .expect("invokevirtual Methodref")
        else {
            panic!("invokevirtual references a Methodref")
        };
        class
            .constant_pool
            .class_name(*class_index)
            .unwrap()
            .to_owned()
    };
    assert_eq!(invocation_owner(&client, from_new), "Counter");
    assert_eq!(invocation_owner(&client, ordered), "Counter");
    assert_eq!(invocation_owner(&client, boolean_result), "Counter");
    assert_eq!(invocation_owner(&client, unit_call), "Counter");
    assert_eq!(invocation_owner(&client, direct_get), "Counter");
    assert_eq!(invocation_owner(&client, from_local), "Counter");
    assert_eq!(invocation_owner(&client, from_mutable_local), "Counter");
    // Counter.get and Other.get share a spelling; the canonical selected
    // method and receiver class must keep the two owners distinct.
    assert_eq!(invocation_owner(&client, from_other), "Other");

    let counter = decode_class("Counter");
    let [hidden] = counter
        .methods
        .iter()
        .filter(|method| counter.constant_pool.utf8(method.name_index) == Ok("hidden"))
        .collect::<Vec<_>>()[..]
    else {
        panic!("Counter.hidden is emitted")
    };
    assert!(hidden.access_flags.is_private());
    assert_opcode_sequence(
        emitted_method_code(&counter, "privateCall"),
        &[0x2a, 0xb6, 0xac],
    );
    let private_receiver = emitted_method_code(&counter, "privateReceiver");
    assert_opcode_sequence(private_receiver, &[0xbb, 0x59, 0x1b, 0xb7, 0xb6, 0xac]);
    assert_eq!(invocation_owner(&counter, private_receiver), "Counter");

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 output path");
        let harness = output.path().join("VirtualCallHarness.java");
        fs::write(
            &harness,
            "public final class VirtualCallHarness { public static void main(String[] args) { Client c = new Client(); if (c.directGet(1) != 42 || c.fromNew(3) != 4 || c.ordered(3) != 45 || !c.booleanResult(3) || c.fromLocal(3) != 4 || c.fromMutableLocal(3) != 4 || c.fromOther(3) != 7 || new Counter(1).privateCall() != 9 || new Counter(1).privateReceiver(2) != 9) throw new AssertionError(); c.unitCall(1); System.out.println(\"virtual calls ok\"); } }\n",
        )
        .expect("write Java verification harness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected virtual-call harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "VirtualCallHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected virtual-call classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "virtual calls ok"
        );
    }
}

#[test]
fn inherited_virtual_calls_keep_the_declaring_methodref_and_dispatch_dynamically() {
    let output = TestOutputDirectory::new("inherited-virtual-call-targets");
    let source = "class Base { def get(): Int = 1; def value(): Int = 10; def doubled(): Int = value() * 2 }\nclass Derived extends Base { override def value(): Int = 21; def read(): Int = this.get() }\nclass Runner { def read(base: Base): Int = base.doubled(); def viaDerived(derived: Derived): Int = derived.doubled(); def forward(derived: Derived): Int = read(derived) }\n";
    let artifacts = compile_source_classes(source, "InheritedVirtualCalls.scala", output.path())
        .expect("inherited methods and subtype arguments compile as virtual calls");

    let decode_class = |name: &str| {
        let artifact = artifacts
            .iter()
            .find(|artifact| artifact.class.internal_name() == name)
            .unwrap_or_else(|| panic!("{name} class is emitted"));
        let mut reader = Reader::new(artifact.class.bytes());
        let class = ClassFile::decode(&mut reader).expect("decode emitted class");
        class.validate_references().expect("valid class references");
        class
    };
    let assert_invokevirtual_owner =
        |class: &ClassFile<'_>, method: &str, owner: &str, name: &str, descriptor: &str| {
            let code = emitted_method_code(class, method);
            let offset = decode_subset_instructions(code)
                .into_iter()
                .find_map(|(offset, opcode)| (opcode == 0xb6).then_some(offset))
                .expect("method contains invokevirtual");
            let index = u16::from_be_bytes([code[offset + 1], code[offset + 2]]);
            let ConstantPoolEntry::Methodref {
                class_index,
                name_and_type_index,
            } = class
                .constant_pool
                .get(ConstantPoolIndex(index))
                .expect("invokevirtual Methodref")
            else {
                panic!("invokevirtual references a Methodref")
            };
            assert_eq!(class.constant_pool.class_name(*class_index), Ok(owner));
            assert_eq!(
                class
                    .constant_pool
                    .name_and_type(*name_and_type_index)
                    .unwrap()
                    .0,
                name
            );
            assert_eq!(
                class
                    .constant_pool
                    .name_and_type(*name_and_type_index)
                    .unwrap()
                    .1,
                descriptor
            );
        };

    let base = decode_class("Base");
    let derived = decode_class("Derived");
    let runner = decode_class("Runner");
    assert_invokevirtual_owner(&base, "doubled", "Base", "value", "()I");
    assert_invokevirtual_owner(&derived, "read", "Base", "get", "()I");
    assert_eq!(
        decode_subset_instructions(emitted_method_code(&derived, "read"))
            .into_iter()
            .map(|(_, opcode)| opcode)
            .collect::<Vec<_>>(),
        [0x2a, 0xb6, 0xac],
        "explicit this call loads aload_0, invokes Base.get, and returns its Int result"
    );
    assert_invokevirtual_owner(&runner, "read", "Base", "doubled", "()I");
    assert_invokevirtual_owner(&runner, "viaDerived", "Base", "doubled", "()I");
    assert_invokevirtual_owner(&runner, "forward", "Runner", "read", "(LBase;)I");
    assert!(
        derived.methods.iter().all(|method| !matches!(
            derived.constant_pool.utf8(method.name_index),
            Ok("doubled" | "get")
        )),
        "inherited Base.doubled and Base.get are not copied into Derived"
    );

    if require_jdk_or_skip() {
        let classpath = output.path().to_str().expect("UTF-8 output path");
        let harness = output.path().join("InheritedVirtualCallsHarness.java");
        fs::write(
            &harness,
            "public final class InheritedVirtualCallsHarness { public static void main(String[] args) { Runner runner = new Runner(); Base base = new Base(); Derived derived = new Derived(); if (runner.read(base) != 20 || runner.read(derived) != 42 || runner.viaDerived(derived) != 42 || runner.forward(derived) != 42 || derived.read() != 1) throw new AssertionError(); System.out.println(\"inherited virtual dispatch verified\"); } }\n",
        )
        .expect("write inherited-call harness");
        let javac = Command::new("javac")
            .args(["-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected inherited-call harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "InheritedVirtualCallsHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected inherited-call classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "inherited virtual dispatch verified"
        );
    }
}

#[test]
fn transitive_inherited_calls_keep_declaring_methodrefs_and_dispatch_dynamically() {
    let output = TestOutputDirectory::new("transitive-inherited-virtual-calls");
    let source = "class Base { def value(): Int = 10; def doubled(): Int = value() * 2; def baseOnly(): Int = 7 }\nclass Middle extends Base { override def value(): Int = 20 }\nclass Leaf extends Middle { override def value(): Int = 21; def parentValue(): Int = super.value(); def viaThis(): Int = this.baseOnly() }\nclass Runner { def throughBase(base: Base): Int = base.doubled(); def viaLeaf(leaf: Leaf): Int = leaf.baseOnly(); def viaLeafDoubled(leaf: Leaf): Int = leaf.doubled() }\n";
    let mut observed_metadata = None;
    let mut observe = |unit: &mut JvmCompilationUnit<'_>, definitions: &Definitions| {
        observed_metadata = Some(
            unit.metadata(definitions)
                .expect("validated transitive dispatch metadata"),
        );
    };
    let artifacts = compile_source_batch_observed(
        source,
        "TransitiveInheritedCalls.scala",
        output.path(),
        SourceId::from_index(1403),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut observe),
    )
    .expect("transitive inherited calls retain canonical declarations");

    let decode_class = |name: &str| {
        let artifact = artifacts
            .iter()
            .find(|artifact| artifact.class.internal_name() == name)
            .unwrap_or_else(|| panic!("{name} class is emitted"));
        decode_emitted_class(artifact)
    };
    let base = decode_class("Base");
    let middle = decode_class("Middle");
    let leaf = decode_class("Leaf");
    let runner = decode_class("Runner");
    let metadata = observed_metadata.expect("observer captures validated class metadata");
    let base_metadata = metadata
        .iter()
        .find(|class| class.identity().internal_name() == "Base")
        .expect("Base metadata");
    let leaf_metadata = metadata
        .iter()
        .find(|class| class.identity().internal_name() == "Leaf")
        .expect("Leaf metadata");
    let base_doubled = base_metadata
        .methods()
        .iter()
        .find(|method| method.name() == "doubled")
        .expect("Base declares doubled");
    let base_only = base_metadata
        .methods()
        .iter()
        .find(|method| method.name() == "baseOnly")
        .expect("Base declares baseOnly");
    let leaf_value = leaf_metadata
        .methods()
        .iter()
        .find(|method| method.name() == "value")
        .expect("Leaf declares its override");
    let inherited_doubled = leaf_metadata
        .dispatch_methods()
        .iter()
        .find(|method| method.name() == "doubled")
        .expect("Base.doubled is effective on Leaf");
    assert_eq!(inherited_doubled.method(), base_doubled.symbol());
    assert_eq!(
        inherited_doubled.declaring_class(),
        base_metadata.identity().symbol()
    );
    assert_eq!(
        inherited_doubled.receiver_class(),
        leaf_metadata.identity().symbol()
    );
    let inherited_base_only = leaf_metadata
        .dispatch_methods()
        .iter()
        .find(|method| method.name() == "baseOnly")
        .expect("Base.baseOnly is effective on Leaf");
    assert_eq!(inherited_base_only.method(), base_only.symbol());
    let effective_value = leaf_metadata
        .dispatch_methods()
        .iter()
        .find(|method| method.name() == "value")
        .expect("Leaf.value is the effective override");
    assert_eq!(effective_value.method(), leaf_value.symbol());
    assert_eq!(
        effective_value.declaring_class(),
        leaf_metadata.identity().symbol()
    );

    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "throughBase"),
        &[(0xb6, "Base", "doubled", "()I")],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "viaLeaf"),
        &[(0xb6, "Base", "baseOnly", "()I")],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "viaLeafDoubled"),
        &[(0xb6, "Base", "doubled", "()I")],
    );
    assert_invocation_sequence(
        &leaf,
        emitted_method_code(&leaf, "parentValue"),
        &[(0xb7, "Middle", "value", "()I")],
    );
    assert_invocation_sequence(
        &leaf,
        emitted_method_code(&leaf, "viaThis"),
        &[(0xb6, "Base", "baseOnly", "()I")],
    );
    for class in [&middle, &leaf] {
        assert!(
            class.methods.iter().all(|method| !matches!(
                class.constant_pool.utf8(method.name_index),
                Ok("doubled" | "baseOnly")
            )),
            "inherited Base methods are not copied into {}",
            class.constant_pool.class_name(class.this_class).unwrap()
        );
    }
    assert!(
        base.methods
            .iter()
            .any(|method| { base.constant_pool.utf8(method.name_index) == Ok("doubled") })
    );

    if require_jdk_or_skip() {
        let classpath = output.path().to_str().expect("UTF-8 output path");
        let harness = output.path().join("TransitiveInheritedCallsHarness.java");
        fs::write(
            &harness,
            "public final class TransitiveInheritedCallsHarness { public static void main(String[] args) { Runner runner = new Runner(); Base base = new Base(); Leaf leaf = new Leaf(); if (runner.throughBase(base) != 20 || runner.throughBase(leaf) != 42 || runner.viaLeaf(leaf) != 7 || runner.viaLeafDoubled(leaf) != 42 || leaf.parentValue() != 20 || leaf.viaThis() != 7) throw new AssertionError(); System.out.println(\"transitive inherited dispatch verified\"); } }\n",
        )
        .expect("write transitive-call harness");
        let javac = Command::new("javac")
            .args(["-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected transitive-call harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "TransitiveInheritedCallsHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected transitive-inheritance classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "transitive inherited dispatch verified"
        );
    }
}

#[test]
fn super_call_through_empty_intermediate_class_uses_direct_parent_methodref() {
    let output = TestOutputDirectory::new("inherited-super-methodref");
    let source = "class Base { def value(): Int = 10 }\nclass Middle extends Base {}\nclass Leaf extends Middle { override def value(): Int = 20; def parentValue(): Int = super.value() }\n";
    let artifacts = compile_source_classes(source, "InheritedSuper.scala", output.path())
        .expect("super can select a method inherited through the immediate parent");
    let leaf_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Leaf")
        .expect("Leaf class is emitted");
    let leaf = decode_emitted_class(leaf_artifact);

    assert_invocation_sequence(
        &leaf,
        emitted_method_code(&leaf, "parentValue"),
        &[(0xb7, "Middle", "value", "()I")],
    );

    if require_jdk_or_skip() {
        let classpath = output.path().to_str().expect("UTF-8 output path");
        let harness = output.path().join("InheritedSuperHarness.java");
        fs::write(
            &harness,
            "public final class InheritedSuperHarness { public static void main(String[] args) { if (new Leaf().parentValue() != 10) throw new AssertionError(); System.out.println(\"inherited super call verified\"); } }\n",
        )
        .expect("write inherited-super harness");
        let javac = Command::new("javac")
            .args(["-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected inherited-super harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "InheritedSuperHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected inherited-super classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "inherited super call verified"
        );
    }
}

#[test]
fn three_level_parameterized_constructors_and_super_calls_compose() {
    let output = TestOutputDirectory::new("three-level-constructor-super");
    let second_output = TestOutputDirectory::new("three-level-constructor-super-second");
    let artifacts = compile_fixture_classes("ThreeLevelConstructorSuper.scala", output.path())
        .expect("the source-backed three-level constructor and super-call chain compiles");
    let second = compile_fixture_classes("ThreeLevelConstructorSuper.scala", second_output.path())
        .expect("a fresh frontend session emits the same three-level chain");
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["ChainBase", "ChainMiddle", "ChainLeaf", "ChainRunner"]
    );
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| (artifact.class.internal_name(), artifact.class.bytes(),))
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|artifact| (artifact.class.internal_name(), artifact.class.bytes(),))
            .collect::<Vec<_>>(),
        "fresh frontend sessions preserve deterministic chain classfiles"
    );

    let classes = artifacts
        .iter()
        .map(|artifact| {
            (
                artifact.class.internal_name(),
                decode_emitted_class(artifact),
            )
        })
        .collect::<Vec<_>>();
    let class = |name: &str| {
        &classes
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .unwrap_or_else(|| panic!("{name} class is emitted"))
            .1
    };
    for (name, parent) in [
        ("ChainBase", "java/lang/Object"),
        ("ChainMiddle", "ChainBase"),
        ("ChainLeaf", "ChainMiddle"),
        ("ChainRunner", "java/lang/Object"),
    ] {
        let emitted = class(name);
        assert_eq!(
            emitted
                .constant_pool
                .class_name(emitted.super_class.unwrap()),
            Ok(parent),
            "{name} declares its immediate JVM parent"
        );
    }

    for (name, parent, descriptor) in [
        ("ChainBase", "java/lang/Object", "()V"),
        ("ChainMiddle", "ChainBase", "(I)V"),
        ("ChainLeaf", "ChainMiddle", "(I)V"),
    ] {
        let emitted = class(name);
        let constructor = emitted_method_code(emitted, "<init>");
        let calls = decode_subset_instructions(constructor)
            .into_iter()
            .filter_map(|(offset, opcode)| (opcode == 0xb7).then_some(offset))
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 1, "{name} calls exactly one constructor");
        assert_method_reference(emitted, constructor, 0xb7, parent, "<init>", descriptor);
        if name == "ChainBase" {
            let parent_call = calls[0];
            let field_store = decode_subset_instructions(constructor)
                .into_iter()
                .find_map(|(offset, opcode)| (opcode == 0xb5).then_some(offset))
                .expect("Base initializes its source field");
            assert!(
                parent_call < field_store,
                "{name} initializes its own fields after its parent constructor"
            );
        }
        if name != "ChainBase" {
            assert_eq!(
                decode_subset_instructions(constructor)
                    .iter()
                    .take(3)
                    .map(|(_, opcode)| *opcode)
                    .collect::<Vec<_>>(),
                [0x2a, 0x1b, 0xb7],
                "{name} forwards its own primary-constructor parameter"
            );
        }
    }

    assert_invocation_sequence(
        class("ChainMiddle"),
        emitted_method_code(class("ChainMiddle"), "value"),
        &[(0xb7, "ChainBase", "value", "()I")],
    );
    assert_invocation_sequence(
        class("ChainLeaf"),
        emitted_method_code(class("ChainLeaf"), "value"),
        &[(0xb7, "ChainMiddle", "value", "()I")],
    );
    assert_invocation_sequence(
        class("ChainLeaf"),
        emitted_method_code(class("ChainLeaf"), "parentValue"),
        &[(0xb7, "ChainMiddle", "value", "()I")],
    );
    assert_invocation_sequence(
        class("ChainBase"),
        emitted_method_code(class("ChainBase"), "doubled"),
        &[(0xb6, "ChainBase", "value", "()I")],
    );
    assert!(
        class("ChainLeaf").methods.iter().all(|method| {
            !matches!(
                class("ChainLeaf").constant_pool.utf8(method.name_index),
                Ok("doubled")
            )
        }),
        "the inherited Base body is emitted only once"
    );

    if !require_jdk_or_skip() {
        return;
    }
    let classpath = output.path().to_str().expect("UTF-8 classpath");
    let javap = Command::new("javap")
        .args(["-c", "-v", "-classpath", classpath, "ChainLeaf"])
        .output()
        .expect("start javap for the leaf class");
    assert!(
        javap.status.success(),
        "javap rejected ChainLeaf.class\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javap.stdout),
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap_output = String::from_utf8_lossy(&javap.stdout);
    assert!(javap_output.contains("ChainMiddle.\"<init>\":(I)V"));
    assert!(javap_output.contains("ChainMiddle.value:()I"));

    let harness = output.path().join("ThreeLevelConstructorSuperHarness.java");
    fs::write(
        &harness,
        "public final class ThreeLevelConstructorSuperHarness { public static void main(String[] args) { ChainLeaf positive = new ChainLeaf(10); ChainLeaf negative = new ChainLeaf(-3); if (positive.value() != 12 || positive.parentValue() != 11 || positive.doubled() != 24) throw new AssertionError(\"positive chain\"); if (negative.value() != -1 || negative.parentValue() != -2 || negative.doubled() != -2) throw new AssertionError(\"negative chain\"); ChainMiddle middle = new ChainMiddle(10); if (middle.value() != 11) throw new AssertionError(\"middle direct super\"); System.out.println(\"three-level constructor and super dispatch verified\"); } }\n",
    )
    .expect("write three-level constructor/super JVM harness");
    let javac = Command::new("javac")
        .args(["-classpath", classpath, "-d", classpath])
        .arg(&harness)
        .output()
        .expect("start javac for the chain harness");
    assert!(
        javac.status.success(),
        "javac rejected the three-level chain harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            classpath,
            "ThreeLevelConstructorSuperHarness",
        ])
        .output()
        .expect("start java with full verification for the chain harness");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected the three-level chain\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "three-level constructor and super dispatch verified"
    );
}

#[test]
fn full_multilevel_inheritance_fixture_verifies_construction_dispatch_and_joins() {
    let first_output = TestOutputDirectory::new("multilevel-inheritance-first");
    let second_output = TestOutputDirectory::new("multilevel-inheritance-second");
    let first = compile_fixture_classes("MultilevelInheritance.scala", first_output.path())
        .expect("the complete real-source multilevel fixture compiles in one session");
    let second = compile_fixture_classes("MultilevelInheritance.scala", second_output.path())
        .expect("a fresh frontend session emits the same complete source fixture");
    let identities = |artifacts: &[EmittedArtifact]| {
        artifacts
            .iter()
            .map(|artifact| {
                (
                    artifact.class.internal_name().to_owned(),
                    artifact.class.bytes().to_vec(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(identities(&first), identities(&second));
    assert_eq!(
        first
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Base", "Middle", "Leaf", "Runner"],
        "complete class batches retain source order"
    );

    let classes = first
        .iter()
        .map(|artifact| {
            (
                artifact.class.internal_name(),
                decode_emitted_class(artifact),
            )
        })
        .collect::<Vec<_>>();
    let class = |name: &str| {
        &classes
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .unwrap_or_else(|| panic!("{name} class is emitted"))
            .1
    };
    for (name, parent) in [
        ("Base", "java/lang/Object"),
        ("Middle", "Base"),
        ("Leaf", "Middle"),
        ("Runner", "java/lang/Object"),
    ] {
        let emitted = class(name);
        assert_eq!(emitted.version.major, 65);
        assert!(emitted.access_flags.is_public());
        assert_eq!(
            emitted
                .constant_pool
                .class_name(emitted.super_class.unwrap()),
            Ok(parent),
            "{name} records its exact immediate parent"
        );
    }

    assert_eq!(class("Base").fields.len(), 1);
    let current = &class("Base").fields[0];
    assert_eq!(
        class("Base").constant_pool.utf8(current.name_index),
        Ok("current")
    );
    assert_eq!(
        class("Base").constant_pool.utf8(current.descriptor_index),
        Ok("I")
    );
    assert!(current.access_flags.is_private());
    assert!(!current.access_flags.is_final());
    assert!(class("Middle").fields.is_empty());
    assert!(class("Leaf").fields.is_empty());

    for (name, expected) in [
        (
            "Base",
            vec![("<init>", "(I)V"), ("value", "()I"), ("doubled", "()I")],
        ),
        ("Middle", vec![("<init>", "(I)V"), ("value", "()I")]),
        (
            "Leaf",
            vec![("<init>", "(I)V"), ("value", "()I"), ("parentValue", "()I")],
        ),
        (
            "Runner",
            vec![
                ("<init>", "()V"),
                ("evaluate", "(LBase;)I"),
                ("run", "(I)I"),
                ("choose", "(ZI)LMiddle;"),
                ("reverseChoose", "(ZI)LMiddle;"),
                ("chosenValue", "(ZI)I"),
                ("reverseChosenValue", "(ZI)I"),
                ("produceBase", "(I)LBase;"),
            ],
        ),
    ] {
        assert_eq!(
            methods_with_descriptors(class(name)),
            expected
                .into_iter()
                .map(|(name, descriptor)| (name.to_owned(), descriptor.to_owned()))
                .collect::<Vec<_>>(),
            "{name} exposes only its planned method descriptors"
        );
        assert!(
            class(name)
                .methods
                .iter()
                .all(|method| method.access_flags.is_public())
        );
    }
    assert!(
        class("Middle").methods.iter().all(|method| {
            class("Middle").constant_pool.utf8(method.name_index) != Ok("doubled")
        })
    );
    assert!(
        class("Leaf")
            .methods
            .iter()
            .all(|method| { class("Leaf").constant_pool.utf8(method.name_index) != Ok("doubled") })
    );

    for (name, parent, descriptor) in [
        ("Base", "java/lang/Object", "()V"),
        ("Middle", "Base", "(I)V"),
        ("Leaf", "Middle", "(I)V"),
    ] {
        let code = emitted_method_code(class(name), "<init>");
        let constructor_calls = decode_subset_instructions(code)
            .iter()
            .filter(|(_, opcode)| *opcode == 0xb7)
            .count();
        assert_eq!(
            constructor_calls, 1,
            "{name} invokes exactly one constructor"
        );
        assert_method_reference(class(name), code, 0xb7, parent, "<init>", descriptor);
    }
    let base_constructor = emitted_method_code(class("Base"), "<init>");
    let base_instructions = decode_subset_instructions(base_constructor);
    let object_init = base_instructions
        .iter()
        .find(|(_, opcode)| *opcode == 0xb7)
        .expect("Base invokes Object constructor")
        .0;
    let current_stores = base_instructions
        .iter()
        .filter(|(_, opcode)| *opcode == 0xb5)
        .collect::<Vec<_>>();
    assert_eq!(current_stores.len(), 1, "Base.current is initialized once");
    assert_field_reference(
        class("Base"),
        base_constructor,
        0xb5,
        "Base",
        "current",
        "I",
    );
    assert!(object_init < current_stores[0].0);
    assert_method_reference(
        class("Middle"),
        emitted_method_code(class("Middle"), "<init>"),
        0xb7,
        "Base",
        "<init>",
        "(I)V",
    );
    assert_method_reference(
        class("Leaf"),
        emitted_method_code(class("Leaf"), "<init>"),
        0xb7,
        "Middle",
        "<init>",
        "(I)V",
    );

    assert_invocation_sequence(
        class("Base"),
        emitted_method_code(class("Base"), "doubled"),
        &[
            (0xb6, "Base", "value", "()I"),
            (0xb6, "Base", "value", "()I"),
        ],
    );
    assert_invocation_sequence(
        class("Middle"),
        emitted_method_code(class("Middle"), "value"),
        &[(0xb7, "Base", "value", "()I")],
    );
    for (method, parent) in [("value", "Middle"), ("parentValue", "Middle")] {
        assert_invocation_sequence(
            class("Leaf"),
            emitted_method_code(class("Leaf"), method),
            &[(0xb7, parent, "value", "()I")],
        );
    }
    assert_invocation_sequence(
        class("Runner"),
        emitted_method_code(class("Runner"), "evaluate"),
        &[(0xb6, "Base", "doubled", "()I")],
    );
    assert_invocation_sequence(
        class("Runner"),
        emitted_method_code(class("Runner"), "run"),
        &[
            (0xb7, "Leaf", "<init>", "(I)V"),
            (0xb6, "Runner", "evaluate", "(LBase;)I"),
        ],
    );
    for method in ["choose", "reverseChoose"] {
        let code = emitted_method_code(class("Runner"), method);
        let branch_constructor_order = if method == "choose" {
            [
                (0xb7, "Middle", "<init>", "(I)V"),
                (0xb7, "Leaf", "<init>", "(I)V"),
            ]
        } else {
            [
                (0xb7, "Leaf", "<init>", "(I)V"),
                (0xb7, "Middle", "<init>", "(I)V"),
            ]
        };
        assert_invocation_sequence(class("Runner"), code, &branch_constructor_order);
        assert!(
            decode_subset_instructions(code)
                .iter()
                .all(|(_, opcode)| *opcode != 0xc0),
            "{method} needs no checkcast"
        );
        let frames = stack_map_frames(class("Runner"), method);
        let branch_targets = decode_branch_targets(code)
            .into_iter()
            .map(|(_, _, target)| target)
            .collect::<Vec<_>>();
        assert!(!branch_targets.is_empty());
        assert!(
            branch_targets
                .iter()
                .all(|target| { frames.iter().any(|frame| frame.pc == *target) })
        );
        assert!(frames.iter().any(|frame| {
            frame.stack == [StackMapVerificationType::Object("Middle".to_owned())]
        }));
        let method_info = class("Runner")
            .methods
            .iter()
            .find(|entry| class("Runner").constant_pool.utf8(entry.name_index) == Ok(method))
            .expect("reference conditional method exists");
        let [Attribute::Code(code_info)] = method_info.attributes.as_slice() else {
            panic!("{method} has one Code attribute");
        };
        assert!(code_info.max_stack >= 3);
        assert_eq!(code_info.max_locals, 3);
    }
    assert_invocation_sequence(
        class("Runner"),
        emitted_method_code(class("Runner"), "chosenValue"),
        &[
            (0xb6, "Runner", "choose", "(ZI)LMiddle;"),
            (0xb6, "Middle", "value", "()I"),
        ],
    );
    assert_invocation_sequence(
        class("Runner"),
        emitted_method_code(class("Runner"), "reverseChosenValue"),
        &[
            (0xb6, "Runner", "reverseChoose", "(ZI)LMiddle;"),
            (0xb6, "Middle", "value", "()I"),
        ],
    );
    let produce_base = emitted_method_code(class("Runner"), "produceBase");
    assert_invocation_sequence(
        class("Runner"),
        produce_base,
        &[(0xb7, "Leaf", "<init>", "(I)V")],
    );
    assert!(
        !produce_base.contains(&0xc0),
        "produceBase emits no checkcast"
    );

    if !require_jdk_or_skip() {
        return;
    }
    let classpath = first_output.path().to_str().expect("UTF-8 classpath");
    let javap = Command::new("javap")
        .args([
            "-c",
            "-v",
            "-classpath",
            classpath,
            "Base",
            "Middle",
            "Leaf",
            "Runner",
        ])
        .output()
        .expect("start javap for the complete multilevel batch");
    assert!(
        javap.status.success(),
        "javap rejected the multilevel class batch\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javap.stdout),
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap_output = String::from_utf8_lossy(&javap.stdout);
    for expected in [
        "major version: 65",
        "Base.\"<init>\":(I)V",
        "Middle.\"<init>\":(I)V",
        "Leaf.\"<init>\":(I)V",
        "Base.value:()I",
        "Middle.value:()I",
        "StackMapTable",
    ] {
        assert!(
            javap_output.contains(expected),
            "javap output omitted {expected}"
        );
    }

    let harness = first_output
        .path()
        .join("MultilevelInheritanceHarness.java");
    fs::write(
        &harness,
        r#"public final class MultilevelInheritanceHarness {
    public static void main(String[] args) {
        Runner runner = new Runner();
        if (new Base(10).doubled() != 20) throw new AssertionError("base");
        if (new Middle(10).doubled() != 22) throw new AssertionError("middle virtual dispatch");
        if (new Leaf(10).doubled() != 24) throw new AssertionError("leaf virtual dispatch");
        if (new Leaf(10).parentValue() != 11) throw new AssertionError("direct parent super");
        if (runner.run(10) != 24 || runner.run(0) != 4 || runner.run(-3) != -2) throw new AssertionError("constructor arguments");
        if (runner.chosenValue(true, 10) != 11 || runner.chosenValue(false, 10) != 12) throw new AssertionError("forward join");
        if (runner.reverseChosenValue(true, 10) != 12 || runner.reverseChosenValue(false, 10) != 11) throw new AssertionError("reverse join");
        Base produced = runner.produceBase(10);
        if (!(produced instanceof Leaf) || produced.doubled() != 24) throw new AssertionError("Leaf as Base");
        System.out.println("multilevel inheritance verified");
    }
}
"#,
    )
    .expect("write multilevel source JVM harness");
    let javac = Command::new("javac")
        .args(["--release", "21", "-classpath", classpath, "-d", classpath])
        .arg(&harness)
        .output()
        .expect("start javac for the multilevel harness");
    assert!(
        javac.status.success(),
        "javac rejected the multilevel harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            classpath,
            "MultilevelInheritanceHarness",
        ])
        .output()
        .expect("start JDK 21 verifier for the multilevel harness");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected the multilevel classes\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "multilevel inheritance verified"
    );
}

#[test]
fn fourth_source_inheritance_level_fails_before_batch_publication() {
    let source = "class DepthBase\nclass DepthMiddle extends DepthBase\nclass DepthLeaf extends DepthMiddle\nclass DepthFourth extends DepthLeaf\n";
    let output = TestOutputDirectory::new("fourth-source-inheritance-level");
    let result = compile_source_batch(
        source,
        "FourthInheritanceLevel.scala",
        output.path(),
        SourceId::from_index(1408),
        SourceUnitCardinality::AtLeastTwo,
    );
    assert!(
        matches!(
            result,
            Err(PipelineFailure::Codegen(
                JvmCodegenError::InvalidJvmSuperclass {
                    issue: dotty_jvm::JvmSuperclassIssue::InheritanceTooDeep,
                    ..
                }
            ))
        ),
        "the source pipeline rejects a fourth user class with the typed hierarchy error: {result:?}"
    );
    assert!(
        output.class_files().is_empty(),
        "the rejected hierarchy publishes no partial class batch"
    );
}

#[test]
fn scala_single_inheritance_and_virtual_dispatch_pass_the_end_to_end_jdk_gate() {
    let first_output = TestOutputDirectory::new("inheritance-first-build");
    let second_output = TestOutputDirectory::new("inheritance-second-build");
    let first = compile_fixture_classes("Inheritance.scala", first_output.path())
        .expect("the real Scala source pipeline emits Base, Derived, and Runner together");
    let second = compile_fixture_classes("Inheritance.scala", second_output.path())
        .expect("a fresh Scala source pipeline emits the same class batch");
    let expected_order = ["Base", "Derived", "Runner"];
    assert_eq!(
        first
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        expected_order
    );
    assert_eq!(
        second
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        expected_order,
        "fresh frontend sessions preserve class output order"
    );
    for (left, right) in first.iter().zip(&second) {
        assert_eq!(left.class.internal_name(), right.class.internal_name());
        assert_eq!(
            left.class.bytes(),
            right.class.bytes(),
            "fresh frontend sessions emit byte-identical {}.class files",
            left.class.internal_name()
        );
    }

    let base = decode_emitted_class(&first[0]);
    let derived = decode_emitted_class(&first[1]);
    let runner = decode_emitted_class(&first[2]);
    for class in [&base, &derived, &runner] {
        assert_eq!(class.version.major, 65);
        assert!(class.access_flags.is_public());
    }
    assert_eq!(
        base.constant_pool.class_name(base.super_class.unwrap()),
        Ok("java/lang/Object")
    );
    assert_eq!(
        derived
            .constant_pool
            .class_name(derived.super_class.unwrap()),
        Ok("Base")
    );
    assert_eq!(
        runner.constant_pool.class_name(runner.super_class.unwrap()),
        Ok("java/lang/Object")
    );

    let method_descriptor = |class: &ClassFile<'_>, name: &str| {
        class
            .methods
            .iter()
            .find(|method| class.constant_pool.utf8(method.name_index) == Ok(name))
            .map(|method| {
                class
                    .constant_pool
                    .utf8(method.descriptor_index)
                    .unwrap()
                    .to_owned()
            })
            .unwrap_or_else(|| panic!("{name} method is emitted"))
    };
    for (class, method, descriptor) in [
        (&base, "<init>", "()V"),
        (&base, "value", "()I"),
        (&base, "doubled", "()I"),
        (&derived, "<init>", "()V"),
        (&derived, "value", "()I"),
        (&runner, "<init>", "()V"),
        (&runner, "evaluate", "(LBase;)I"),
        (&runner, "fromBase", "()I"),
        (&runner, "fromDerived", "()I"),
        (&runner, "produceBase", "()LBase;"),
        (&runner, "branch", "(LBase;Z)I"),
    ] {
        assert_eq!(method_descriptor(class, method), descriptor.to_owned());
        let found = class
            .methods
            .iter()
            .find(|candidate| class.constant_pool.utf8(candidate.name_index) == Ok(method))
            .unwrap();
        assert!(found.access_flags.is_public(), "{method} is public");
    }
    assert!(
        derived
            .methods
            .iter()
            .all(|method| !matches!(derived.constant_pool.utf8(method.name_index), Ok("doubled"))),
        "Derived inherits Base.doubled without copying its body"
    );

    let references = |class: &ClassFile<'_>, method: &str, opcode: u8| {
        let code = emitted_method_code(class, method);
        decode_subset_instructions(code)
            .into_iter()
            .filter_map(|(offset, found_opcode)| (found_opcode == opcode).then_some(offset))
            .map(|offset| {
                let index = u16::from_be_bytes([code[offset + 1], code[offset + 2]]);
                let ConstantPoolEntry::Methodref {
                    class_index,
                    name_and_type_index,
                } = class
                    .constant_pool
                    .get(ConstantPoolIndex(index))
                    .expect("invocation has a valid method reference")
                else {
                    panic!("invocation references Methodref")
                };
                let (name, descriptor) = class
                    .constant_pool
                    .name_and_type(*name_and_type_index)
                    .expect("method reference has a valid name and descriptor");
                (
                    class
                        .constant_pool
                        .class_name(*class_index)
                        .expect("method owner is a class")
                        .to_owned(),
                    name.to_owned(),
                    descriptor.to_owned(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        references(&base, "<init>", 0xb7),
        [(
            "java/lang/Object".to_owned(),
            "<init>".to_owned(),
            "()V".to_owned()
        )]
    );
    assert_eq!(
        references(&derived, "<init>", 0xb7),
        [("Base".to_owned(), "<init>".to_owned(), "()V".to_owned())]
    );
    assert_eq!(
        references(&base, "doubled", 0xb6),
        [
            ("Base".to_owned(), "value".to_owned(), "()I".to_owned()),
            ("Base".to_owned(), "value".to_owned(), "()I".to_owned()),
        ],
        "both calls keep Base.value as the symbolic owner for JVM dispatch"
    );
    assert_eq!(
        references(&runner, "evaluate", 0xb6),
        [("Base".to_owned(), "doubled".to_owned(), "()I".to_owned())]
    );
    assert_eq!(
        references(&runner, "fromBase", 0xb6),
        [(
            "Runner".to_owned(),
            "evaluate".to_owned(),
            "(LBase;)I".to_owned()
        )]
    );
    assert_eq!(
        references(&runner, "fromDerived", 0xb6),
        [(
            "Runner".to_owned(),
            "evaluate".to_owned(),
            "(LBase;)I".to_owned()
        )]
    );
    assert_eq!(
        references(&runner, "branch", 0xb6),
        [
            ("Base".to_owned(), "doubled".to_owned(), "()I".to_owned()),
            ("Base".to_owned(), "value".to_owned(), "()I".to_owned()),
        ]
    );
    let produce_base = emitted_method_code(&runner, "produceBase");
    assert_opcode_sequence(produce_base, &[0xbb, 0x59, 0xb7, 0xb0]);
    assert!(
        !produce_base.contains(&0xc0),
        "produceBase emits no checkcast"
    );
    assert!(has_stack_map_table(&runner, "branch"));
    assert_eq!(
        runner
            .methods
            .iter()
            .find(|method| runner.constant_pool.utf8(method.name_index) == Ok("branch"))
            .unwrap()
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::Code(code) => Some((code.max_stack, code.max_locals)),
                _ => None,
            }),
        Some((1, 3)),
        "branch uses the receiver, Base parameter, and Boolean parameter slots"
    );

    if require_jdk_or_skip() {
        let classpath = first_output.path().to_str().expect("UTF-8 output path");
        let javap = Command::new("javap")
            .args(["-c", "-v", "-classpath", classpath, "Runner"])
            .output()
            .expect("start javap");
        assert!(
            javap.status.success(),
            "javap rejected Runner.class\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javap.stdout),
            String::from_utf8_lossy(&javap.stderr)
        );
        let javap_output = String::from_utf8_lossy(&javap.stdout);
        assert!(javap_output.contains("StackMapTable"));
        assert!(javap_output.contains("branch"));
        assert!(javap_output.contains("descriptor: (LBase;Z)I"));
        assert!(javap_output.contains("Base.doubled:()I"));
        assert!(javap_output.contains("Base.value:()I"));
        assert!(javap_output.contains("Runner.evaluate:(LBase;)I"));

        let harness = first_output.path().join("InheritanceHarness.java");
        fs::write(
            &harness,
            "public final class InheritanceHarness { public static void main(String[] args) { Runner r = new Runner(); Base base = new Base(); Base derived = new Derived(); if (r.evaluate(base) != 20) throw new AssertionError(\"base dispatch\"); if (r.evaluate(derived) != 42) throw new AssertionError(\"dynamic override through Base\"); if (r.fromBase() != 20 || r.fromDerived() != 42) throw new AssertionError(\"constructor arguments\"); if (derived.value() != 21 || derived.doubled() != 42) throw new AssertionError(\"inherited body uses overridden value\"); if (!(r.produceBase() instanceof Derived) || r.produceBase().value() != 21) throw new AssertionError(\"Derived return as Base\"); if (r.branch(derived, true) != 42 || r.branch(derived, false) != 21) throw new AssertionError(\"Base parameter branch\"); System.out.println(\"single inheritance and virtual dispatch verified\"); } }\n",
        )
        .expect("write inheritance harness");
        let javac = Command::new("javac")
            .args(["-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected inheritance harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "InheritanceHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected inheritance classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "single inheritance and virtual dispatch verified"
        );
    }
}

#[test]
fn inherited_virtual_call_rejects_an_unplanned_declaring_method() {
    let output = TestOutputDirectory::new("unplanned-inherited-call-target");
    let source = "class Base { def value(): Int = 10; def doubled(): Int = value() * 2 }\nclass Derived extends Base {}\nclass Runner { def read(derived: Derived): Int = derived.doubled() }\n";
    let mut omit_parent_methods = |unit: &mut JvmCompilationUnit<'_>,
                                   _definitions: &Definitions| {
        let base = unit
            .classes
            .iter()
            .find(|plan| {
                let symbol = unit.store.symbols.get(plan.class);
                unit.store.names.try_resolve(symbol.name.text()) == Some("Base")
            })
            .map(|plan| plan.class)
            .expect("source batch has Base");
        unit.classes
            .iter_mut()
            .find(|plan| plan.class == base)
            .expect("Base plan")
            .methods
            .clear();
    };
    let error = compile_source_batch_observed(
        source,
        "UnplannedInheritedCall.scala",
        output.path(),
        SourceId::from_index(1331),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut omit_parent_methods),
    )
    .expect_err("the selected parent declaration must remain in its class plan");
    assert!(
        matches!(
            error,
            PipelineFailure::Codegen(
                JvmCodegenError::MethodNotInJvmPlan { .. }
                    | JvmCodegenError::MethodNotInJvmPlanForBody { .. }
            )
        ),
        "expected a typed inherited target plan error, got {error:?}"
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn derived_values_flow_to_base_parameters_and_returns() {
    let output = TestOutputDirectory::new("derived-to-base-values");
    let source = "class Base { def value(): Int = 10; def doubled(): Int = value() * 2 }\nclass Derived extends Base { override def value(): Int = 21 }\nclass Runner { def evaluate(base: Base): Int = base.doubled(); def viaBase(): Int = evaluate(new Base()); def viaDerived(): Int = evaluate(new Derived()); def viaLocal(): Int = { val derived = new Derived(); evaluate(derived) }; def viaWidenedLocal(): Int = { val base: Base = new Derived(); evaluate(base) }; def produced(): Base = new Derived() }\n";
    let artifacts = compile_source_classes(source, "DerivedToBase.scala", output.path())
        .expect("verified Derived values flow to Base method and return positions");

    let runner = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Runner")
        .expect("Runner class is emitted");
    let mut reader = Reader::new(runner.class.bytes());
    let runner = ClassFile::decode(&mut reader).expect("decode emitted Runner class");
    runner
        .validate_references()
        .expect("Runner references remain valid");
    let evaluate = runner
        .methods
        .iter()
        .find(|method| runner.constant_pool.utf8(method.name_index) == Ok("evaluate"))
        .expect("evaluate method is emitted");
    assert_eq!(
        runner.constant_pool.utf8(evaluate.descriptor_index),
        Ok("(LBase;)I")
    );
    let produced = runner
        .methods
        .iter()
        .find(|method| runner.constant_pool.utf8(method.name_index) == Ok("produced"))
        .expect("produced method is emitted");
    assert_eq!(
        runner.constant_pool.utf8(produced.descriptor_index),
        Ok("()LBase;")
    );
    for method in ["viaDerived", "viaLocal", "viaWidenedLocal"] {
        let code = emitted_method_code(&runner, method);
        let instructions = decode_subset_instructions(code);
        let opcodes = instructions
            .into_iter()
            .map(|(_, opcode)| opcode)
            .collect::<Vec<_>>();
        assert!(opcodes.contains(&0xb6), "{method} invokes evaluate");
        assert!(!opcodes.contains(&0xc0), "{method} uses no checkcast");
        let offset = decode_subset_instructions(code)
            .into_iter()
            .find_map(|(offset, opcode)| (opcode == 0xb6).then_some(offset))
            .expect("method contains invokevirtual");
        let index = u16::from_be_bytes([code[offset + 1], code[offset + 2]]);
        let ConstantPoolEntry::Methodref {
            class_index,
            name_and_type_index,
        } = runner
            .constant_pool
            .get(ConstantPoolIndex(index))
            .expect("invokevirtual Methodref")
        else {
            panic!("invokevirtual references a Methodref")
        };
        assert_eq!(runner.constant_pool.class_name(*class_index), Ok("Runner"));
        assert_eq!(
            runner
                .constant_pool
                .name_and_type(*name_and_type_index)
                .unwrap(),
            ("evaluate", "(LBase;)I")
        );
    }
    let produced_opcodes = decode_subset_instructions(emitted_method_code(&runner, "produced"))
        .into_iter()
        .map(|(_, opcode)| opcode)
        .collect::<Vec<_>>();
    assert!(
        produced_opcodes.contains(&0xb0),
        "produced returns a reference"
    );
    assert!(
        !produced_opcodes.contains(&0xc0),
        "produced uses no checkcast"
    );

    if require_jdk_or_skip() {
        let classpath = output.path().to_str().expect("UTF-8 output path");
        let harness = output.path().join("DerivedToBaseHarness.java");
        fs::write(
            &harness,
            "public final class DerivedToBaseHarness { public static void main(String[] args) { Runner runner = new Runner(); if (runner.viaBase() != 20 || runner.viaDerived() != 42 || runner.viaLocal() != 42 || runner.viaWidenedLocal() != 42 || runner.produced().value() != 21) throw new AssertionError(); System.out.println(\"Derived-to-Base verified\"); } }\n",
        )
        .expect("write Derived-to-Base harness");
        let javac = Command::new("javac")
            .args(["-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected Derived-to-Base harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "DerivedToBaseHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected Derived-to-Base classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "Derived-to-Base verified"
        );
    }
}

#[test]
fn transitive_leaf_values_flow_through_base_method_boundaries_without_casts() {
    let output = TestOutputDirectory::new("transitive-leaf-to-base-method-boundaries");
    let source = r#"class Base { def value(): Int = 10; def doubled(): Int = value() * 2 }
class Middle extends Base { override def value(): Int = 20 }
class Leaf extends Middle { override def value(): Int = 21 }
class Holder(initial: Base) { private var current: Base = initial; def replace(next: Leaf): Unit = { current = next }; def get(): Base = current }
class InitializedHolder(initial: Leaf) { private val current: Base = initial; def get(): Base = current }
class Runner { def evaluate(base: Base): Int = base.doubled(); def viaLeaf(): Int = evaluate(new Leaf()); def viaLocal(): Int = { val base: Base = new Leaf(); evaluate(base) }; def producedBase(): Base = new Leaf(); def producedMiddle(): Middle = new Leaf(); def directReturn(leaf: Leaf): Base = leaf; def passThrough(leaf: Leaf): Base = evaluateAndReturn(leaf); def evaluateAndReturn(base: Base): Base = base; def holder(leaf: Leaf): Holder = new Holder(leaf); def initializedHolder(leaf: Leaf): InitializedHolder = new InitializedHolder(leaf) }
"#;
    let artifacts = compile_source_classes(
        source,
        "TransitiveReferenceAssignability.scala",
        output.path(),
    )
    .expect("Leaf values flow to Base and Middle boundaries through the verified hierarchy");
    let second_output =
        TestOutputDirectory::new("transitive-leaf-to-base-method-boundaries-repeat");
    let second_artifacts = compile_source_classes(
        source,
        "TransitiveReferenceAssignability.scala",
        second_output.path(),
    )
    .expect("a fresh frontend session compiles the same ancestry deterministically");
    assert_eq!(artifacts.len(), second_artifacts.len());
    for (first, second) in artifacts.iter().zip(&second_artifacts) {
        assert_eq!(first.class.internal_name(), second.class.internal_name());
        assert_eq!(first.class.bytes(), second.class.bytes());
    }
    let runner_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Runner")
        .expect("Runner is emitted");
    let mut reader = Reader::new(runner_artifact.class.bytes());
    let runner = ClassFile::decode(&mut reader).expect("decode emitted Runner class");
    runner
        .validate_references()
        .expect("transitive method descriptors remain valid");

    for (name, descriptor) in [
        ("evaluate", "(LBase;)I"),
        ("producedBase", "()LBase;"),
        ("producedMiddle", "()LMiddle;"),
        ("directReturn", "(LLeaf;)LBase;"),
        ("passThrough", "(LLeaf;)LBase;"),
    ] {
        let method = runner
            .methods
            .iter()
            .find(|method| runner.constant_pool.utf8(method.name_index) == Ok(name))
            .unwrap_or_else(|| panic!("{name} method is emitted"));
        assert_eq!(
            runner.constant_pool.utf8(method.descriptor_index),
            Ok(descriptor)
        );
        let instructions = decode_subset_instructions(emitted_method_code(&runner, name));
        assert!(
            instructions.iter().all(|(_, opcode)| *opcode != 0xc0),
            "{name} emits no checkcast"
        );
    }
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "viaLeaf"),
        &[
            (0xb7, "Leaf", "<init>", "()V"),
            (0xb6, "Runner", "evaluate", "(LBase;)I"),
        ],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "viaLocal"),
        &[
            (0xb7, "Leaf", "<init>", "()V"),
            (0xb6, "Runner", "evaluate", "(LBase;)I"),
        ],
    );
    assert!(
        decode_subset_instructions(emitted_method_code(&runner, "producedBase"))
            .iter()
            .any(|(_, opcode)| *opcode == 0xb0),
        "Base-declared result returns the actual Leaf reference"
    );

    for (class_name, constructor_descriptor) in
        [("Holder", "(LBase;)V"), ("InitializedHolder", "(LLeaf;)V")]
    {
        let artifact = artifacts
            .iter()
            .find(|artifact| artifact.class.internal_name() == class_name)
            .unwrap_or_else(|| panic!("{class_name} is emitted"));
        let mut reader = Reader::new(artifact.class.bytes());
        let class = ClassFile::decode(&mut reader).expect("decode reference-holder class");
        let constructor = class
            .methods
            .iter()
            .find(|method| class.constant_pool.utf8(method.name_index) == Ok("<init>"))
            .expect("holder constructor is emitted");
        assert_eq!(
            class.constant_pool.utf8(constructor.descriptor_index),
            Ok(constructor_descriptor)
        );
        let current_field = class
            .fields
            .iter()
            .find(|field| class.constant_pool.utf8(field.name_index) == Ok("current"))
            .expect("holder stores a Base field");
        assert_eq!(
            class.constant_pool.utf8(current_field.descriptor_index),
            Ok("LBase;")
        );
        assert_field_reference(
            &class,
            emitted_method_code(&class, "<init>"),
            0xb5,
            class_name,
            "current",
            "LBase;",
        );
    }
    let holder_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Holder")
        .expect("Holder is emitted");
    let mut holder_reader = Reader::new(holder_artifact.class.bytes());
    let holder = ClassFile::decode(&mut holder_reader).expect("decode Holder class");
    assert_field_reference(
        &holder,
        emitted_method_code(&holder, "replace"),
        0xb5,
        "Holder",
        "current",
        "LBase;",
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "holder"),
        &[(0xb7, "Holder", "<init>", "(LBase;)V")],
    );

    if require_jdk_or_skip() {
        let classpath = output.path().to_str().expect("UTF-8 output path");
        let harness = output
            .path()
            .join("TransitiveReferenceAssignabilityHarness.java");
        fs::write(
            &harness,
            "public final class TransitiveReferenceAssignabilityHarness { public static void main(String[] args) { Runner runner = new Runner(); Leaf leaf = new Leaf(); if (runner.evaluate(leaf) != 42 || runner.viaLeaf() != 42 || runner.viaLocal() != 42 || runner.directReturn(leaf) != leaf || runner.passThrough(leaf) != leaf) throw new AssertionError(); if (!(runner.producedBase() instanceof Leaf) || runner.producedBase().value() != 21) throw new AssertionError(); if (!(runner.producedMiddle() instanceof Leaf)) throw new AssertionError(); Holder holder = runner.holder(leaf); holder.replace(leaf); if (holder.get() != leaf) throw new AssertionError(); InitializedHolder initialized = runner.initializedHolder(leaf); if (initialized.get() != leaf) throw new AssertionError(); System.out.println(\"transitive reference assignability verified\"); } }\n",
        )
        .expect("write transitive-assignability harness");
        let javac = Command::new("javac")
            .args(["-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected transitive-assignability harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "TransitiveReferenceAssignabilityHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected transitive-reference classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "transitive reference assignability verified"
        );
    }
}

#[test]
fn derived_values_flow_to_supported_constructor_arguments_and_reference_fields() {
    let output = TestOutputDirectory::new("derived-to-base-constructor-and-field");
    let source = "class Base { def value(): Int = 10 }\nclass Derived extends Base { override def value(): Int = 42 }\nclass Holder(initial: Base) { private var current: Base = initial; def replace(next: Derived): Unit = { current = next }; def get(): Base = current }\nclass InitializedHolder(initial: Derived) { private val current: Base = initial; def get(): Base = current }\nclass Runner { def fromParameter(next: Derived): Holder = new Holder(next); def fromLocal(): Holder = { val next = new Derived(); new Holder(next) }; def initialized(next: Derived): InitializedHolder = new InitializedHolder(next) }\n";
    let artifacts = compile_source_classes(
        source,
        "DerivedToBaseConstructorAndField.scala",
        output.path(),
    )
    .expect("verified Derived values flow to supported constructor and field positions");

    let holder = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Holder")
        .expect("Holder class is emitted");
    let mut reader = Reader::new(holder.class.bytes());
    let holder = ClassFile::decode(&mut reader).expect("decode emitted Holder class");
    let constructor = holder
        .methods
        .iter()
        .find(|method| holder.constant_pool.utf8(method.name_index) == Ok("<init>"))
        .expect("Holder constructor is emitted");
    assert_eq!(
        holder.constant_pool.utf8(constructor.descriptor_index),
        Ok("(LBase;)V")
    );
    let replace = emitted_method_code(&holder, "replace");
    assert_field_reference(&holder, replace, 0xb5, "Holder", "current", "LBase;");

    let initialized_holder = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "InitializedHolder")
        .expect("InitializedHolder class is emitted");
    let mut reader = Reader::new(initialized_holder.class.bytes());
    let initialized_holder =
        ClassFile::decode(&mut reader).expect("decode emitted InitializedHolder class");
    assert_eq!(
        initialized_holder
            .fields
            .iter()
            .find(|field| initialized_holder.constant_pool.utf8(field.name_index) == Ok("current"))
            .map(|field| initialized_holder
                .constant_pool
                .utf8(field.descriptor_index)),
        Some(Ok("LBase;"))
    );
    let constructor = emitted_method_code(&initialized_holder, "<init>");
    assert_field_reference(
        &initialized_holder,
        constructor,
        0xb5,
        "InitializedHolder",
        "current",
        "LBase;",
    );

    if require_jdk_or_skip() {
        let classpath = output.path().to_str().expect("UTF-8 output path");
        let harness = output
            .path()
            .join("DerivedToBaseConstructorAndFieldHarness.java");
        fs::write(
            &harness,
            "public final class DerivedToBaseConstructorAndFieldHarness { public static void main(String[] args) { Runner runner = new Runner(); Holder fromParameter = runner.fromParameter(new Derived()); if (fromParameter.get().value() != 42) throw new AssertionError(\"constructor parameter\"); fromParameter.replace(new Derived()); if (fromParameter.get().value() != 42) throw new AssertionError(\"reference field assignment\"); if (runner.fromLocal().get().value() != 42) throw new AssertionError(\"constructor local\"); if (runner.initialized(new Derived()).get().value() != 42) throw new AssertionError(\"reference field initializer\"); System.out.println(\"Derived-to-Base constructor and field verified\"); } }\n",
        )
        .expect("write constructor and field harness");
        let javac = Command::new("javac")
            .args(["-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected constructor and field harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "DerivedToBaseConstructorAndFieldHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected constructor and field classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "Derived-to-Base constructor and field verified"
        );
    }
}

#[test]
fn source_typing_rejects_reverse_and_unrelated_reference_arguments() {
    for (label, source) in [
        (
            "base-to-derived-argument",
            "class Base { def value(): Int = 1 }\nclass Derived extends Base {}\nclass Runner { def use(value: Derived): Int = value.value(); def invalid(base: Base): Int = use(base) }\n",
        ),
        (
            "base-to-leaf-argument-through-middle",
            "class Base { def value(): Int = 1 }\nclass Middle extends Base {}\nclass Leaf extends Middle {}\nclass Runner { def use(value: Leaf): Int = value.value(); def invalid(base: Base): Int = use(base) }\n",
        ),
        (
            "unrelated-to-base-argument",
            "class Base { def value(): Int = 1 }\nclass Other {}\nclass Runner { def use(value: Base): Int = value.value(); def invalid(other: Other): Int = use(other) }\n",
        ),
    ] {
        let output = TestOutputDirectory::new(label);
        let error = compile_source_classes(source, &format!("{label}.scala"), output.path())
            .expect_err("the source Typer must reject unproved reference conversions");
        assert!(
            matches!(
                &error,
                PipelineFailure::Typer(TyperError::ApplicationArgumentTypeMismatch { .. })
            ),
            "{label} should fail in the Typer, got {error:?}"
        );
        assert!(
            output.class_files().is_empty(),
            "{label} emits no class files"
        );
    }
}

#[test]
fn reference_parameters_and_call_results_flow_through_virtual_calls() {
    let output = TestOutputDirectory::new("reference-method-values");
    let source = "class Counter(start: Int):\n  def create(value: Int): Counter = new Counter(value)\n  def passthrough(counter: Counter): Counter = counter\n  def self(): Counter = this\n  def read(counter: Counter): Int = counter.get()\n  def forward(counter: Counter): Counter = passthrough(counter)\n  def readCreated(value: Int): Int = read(create(value))\n  def get(): Int = 42\nclass Client:\n  def accept(counter: Counter): Counter = counter\n  def read(counter: Counter): Int = counter.get()\n";
    let first = compile_source_classes(source, "ReferenceMethods.scala", output.path())
        .expect("planned reference-valued methods and calls compile as one batch");
    let second_output = TestOutputDirectory::new("reference-method-values-repeat");
    let second = compile_source_classes(source, "ReferenceMethods.scala", second_output.path())
        .expect("a fresh frontend session compiles reference-valued methods identically");
    assert_eq!(
        first
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Counter", "Client"]
    );
    assert_eq!(first.len(), second.len());
    for (left, right) in first.iter().zip(&second) {
        assert_eq!(left.class.bytes(), right.class.bytes());
    }

    let counter_artifact = first
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Counter")
        .unwrap();
    let mut reader = Reader::new(counter_artifact.class.bytes());
    let counter = ClassFile::decode(&mut reader).expect("decode Counter");
    counter
        .validate_references()
        .expect("Counter references validate");
    assert_opcode_sequence(
        emitted_method_code(&counter, "create"),
        &[0xbb, 0x59, 0x1b, 0xb7, 0xb0],
    );
    assert_opcode_sequence(emitted_method_code(&counter, "passthrough"), &[0x2b, 0xb0]);
    assert_opcode_sequence(emitted_method_code(&counter, "self"), &[0x2a, 0xb0]);
    assert_opcode_sequence(emitted_method_code(&counter, "read"), &[0x2b, 0xb6, 0xac]);
    assert_opcode_sequence(
        emitted_method_code(&counter, "forward"),
        &[0x2a, 0x2b, 0xb6, 0xb0],
    );
    assert_opcode_sequence(
        emitted_method_code(&counter, "readCreated"),
        &[0x2a, 0x2a, 0x1b, 0xb6, 0xb6, 0xac],
    );

    let client_artifact = first
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Client")
        .unwrap();
    let mut reader = Reader::new(client_artifact.class.bytes());
    let client = ClassFile::decode(&mut reader).expect("decode Client");
    client
        .validate_references()
        .expect("Client references validate");

    if require_jdk_or_skip() {
        let harness = output.path().join("ReferenceMethodsHarness.java");
        fs::write(
            &harness,
            "public final class ReferenceMethodsHarness { public static void main(String[] args) { Counter counter = new Counter(0); Client client = new Client(); if (counter.create(1).get() != 42 || counter.passthrough(counter).get() != 42 || counter.self().get() != 42 || counter.read(counter) != 42 || counter.forward(counter).get() != 42 || counter.readCreated(2) != 42) throw new AssertionError(\"Counter\"); if (client.read(counter) != 42 || client.accept(counter).get() != 42) throw new AssertionError(\"Client\"); System.out.println(\"reference methods verified\"); } }\n",
        )
        .expect("write reference-method verification harness");
        let classpath = output.path().to_str().unwrap();
        let javac = Command::new("javac")
            .args(["-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac for reference methods");
        assert!(
            javac.status.success(),
            "javac rejected reference-method harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "ReferenceMethodsHarness",
            ])
            .output()
            .expect("start JVM verifier for reference methods");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected reference methods\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "reference methods verified"
        );
    }
}

#[test]
fn reference_receiver_calls_keep_private_access_rejected_and_allow_verified_branches() {
    let private_output = TestOutputDirectory::new("private-cross-class-virtual-call");
    let private_result = compile_planned_constructor_source(
        "class Counter(seed: Int):\n  private def hidden(): Int = 9\nclass Client:\n  def run(seed: Int): Int = new Counter(seed).hidden()\n",
        private_output.path(),
    );
    assert!(
        matches!(
            private_result,
            Err(PipelineFailure::Codegen(
                JvmCodegenError::UnsupportedJvmVisibility {
                    visibility: dotty_core::Visibility::Private,
                    ..
                }
            ))
        ),
        "backend must reject private cross-class calls, got {private_result:?}"
    );
    assert!(private_output.class_files().is_empty());

    let branch_output = TestOutputDirectory::new("reference-call-branch");
    let branch_result = compile_planned_constructor_source(
        "class Counter(seed: Int):\n  def get(): Int = 42\nclass Client:\n  def run(flag: Boolean, seed: Int): Int =\n    val counter = new Counter(seed)\n    if flag then counter.get() else 0\n",
        branch_output.path(),
    );
    let branch_artifacts =
        branch_result.expect("planned reference local calls are verified inside an if branch");
    assert!(
        branch_artifacts
            .iter()
            .any(|artifact| { artifact.class.internal_name() == "Client" })
    );
}

#[test]
fn reference_parameters_and_results_compose_with_if_and_while_control_flow() {
    let output = TestOutputDirectory::new("reference-parameter-result-control-flow");
    let source = "class Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def increment(): Unit = current = current + 1\nclass CounterControl:\n  def pass(counter: Counter): Counter = counter\n  def choose(flag: Boolean, first: Counter, second: Counter): Counter =\n    if flag then first else second\n  def chooseThroughCall(flag: Boolean, first: Counter, second: Counter): Counter =\n    if flag then\n      val allocated = new Counter(9)\n      pass(first)\n    else second\n  def incrementUntil(counter: Counter, limit: Int): Int =\n    while counter.get() < limit do counter.increment()\n    counter.get()\n  def incrementWhile(enabled: Boolean, counter: Counter, limit: Int): Int =\n    while enabled && counter.get() < limit do counter.increment()\n    counter.get()\n";
    let first = compile_source_classes(
        source,
        "ReferenceParameterResultControlFlow.scala",
        output.path(),
    )
    .expect("exact reference method values compose through if and while");
    let second_output = TestOutputDirectory::new("reference-parameter-result-control-flow-repeat");
    let second = compile_source_classes(
        source,
        "ReferenceParameterResultControlFlow.scala",
        second_output.path(),
    )
    .expect("fresh frontend emits the same reference control-flow classes");
    assert_eq!(first.len(), 2);
    assert_eq!(second.len(), 2);
    for (left, right) in first.iter().zip(&second) {
        assert_eq!(left.class.bytes(), right.class.bytes());
    }

    let counter_artifact = first
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Counter")
        .unwrap();
    let counter = decode_emitted_class(counter_artifact);
    counter
        .validate_references()
        .expect("Counter references validate");

    let control_artifact = first
        .iter()
        .find(|artifact| artifact.class.internal_name() == "CounterControl")
        .unwrap();
    let control = decode_emitted_class(control_artifact);
    control
        .validate_references()
        .expect("CounterControl references validate");
    assert_eq!(
        methods_with_descriptors(&control),
        [
            ("<init>".to_owned(), "()V".to_owned()),
            ("pass".to_owned(), "(LCounter;)LCounter;".to_owned()),
            (
                "choose".to_owned(),
                "(ZLCounter;LCounter;)LCounter;".to_owned()
            ),
            (
                "chooseThroughCall".to_owned(),
                "(ZLCounter;LCounter;)LCounter;".to_owned()
            ),
            ("incrementUntil".to_owned(), "(LCounter;I)I".to_owned()),
            ("incrementWhile".to_owned(), "(ZLCounter;I)I".to_owned()),
        ]
    );

    for method in ["choose", "chooseThroughCall"] {
        let code = emitted_method_code(&control, method);
        assert_eq!(code.last(), Some(&0xb0), "{method} returns with areturn");
        assert!(
            stack_map_frames(&control, method).iter().any(|frame| {
                frame.stack == [StackMapVerificationType::Object("Counter".to_owned())]
            }),
            "{method} joins both branches with exact Object(Counter) on the stack"
        );
    }
    assert_invocation_sequence(
        &control,
        emitted_method_code(&control, "chooseThroughCall"),
        &[
            (0xb7, "Counter", "<init>", "(I)V"),
            (0xb6, "CounterControl", "pass", "(LCounter;)LCounter;"),
        ],
    );

    for (method, expected_locals) in [
        (
            "incrementUntil",
            vec![
                StackMapVerificationType::Object("CounterControl".to_owned()),
                StackMapVerificationType::Object("Counter".to_owned()),
                StackMapVerificationType::Integer,
            ],
        ),
        (
            "incrementWhile",
            vec![
                StackMapVerificationType::Object("CounterControl".to_owned()),
                StackMapVerificationType::Integer,
                StackMapVerificationType::Object("Counter".to_owned()),
                StackMapVerificationType::Integer,
            ],
        ),
    ] {
        let code = emitted_method_code(&control, method);
        let back_edges = decode_branch_targets(code)
            .into_iter()
            .filter(|(offset, _, target)| target < offset)
            .collect::<Vec<_>>();
        assert!(!back_edges.is_empty(), "{method} has a loop back-edge");
        let frames = stack_map_frames(&control, method);
        for (_, _, target) in back_edges {
            let frame = frames
                .iter()
                .find(|frame| frame.pc == target)
                .unwrap_or_else(|| panic!("{method} back-edge target has a stack-map frame"));
            assert_eq!(frame.locals, expected_locals, "{method} exact loop locals");
            assert!(
                frame.stack.is_empty(),
                "{method} loop header has an empty stack"
            );
        }
    }

    if require_jdk_or_skip() {
        let harness = output
            .path()
            .join("ReferenceParameterResultControlFlowHarness.java");
        fs::write(
            &harness,
            "public final class ReferenceParameterResultControlFlowHarness { public static void main(String[] args) { CounterControl control = new CounterControl(); Counter first = new Counter(0); Counter second = new Counter(0); if (control.choose(true, first, second) != first || control.choose(false, first, second) != second) throw new AssertionError(\"branch identity\"); if (control.chooseThroughCall(true, first, second) != first || control.chooseThroughCall(false, first, second) != second) throw new AssertionError(\"call branch identity\"); Counter count = new Counter(0); if (control.incrementUntil(count, 0) != 0 || control.incrementUntil(count, 1) != 1 || control.incrementUntil(count, 10) != 10) throw new AssertionError(\"zero/one/multi iteration\"); Counter disabled = new Counter(0); if (control.incrementWhile(false, disabled, 10) != 0) throw new AssertionError(\"short-circuit zero iteration\"); if (control.incrementWhile(true, disabled, 4) != 4) throw new AssertionError(\"short-circuit multi iteration\"); System.out.println(\"reference parameter/result CFG verified\"); } }\n",
        )
        .expect("write reference parameter/result verifier harness");
        let classpath = output.path().to_str().unwrap();
        let javac = Command::new("javac")
            .args(["-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac for reference control-flow harness");
        assert!(
            javac.status.success(),
            "javac rejected reference control-flow harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "ReferenceParameterResultControlFlowHarness",
            ])
            .output()
            .expect("start JVM verifier for reference control-flow classes");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected reference control-flow classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&java.stdout).trim(),
            "reference parameter/result CFG verified"
        );
    }
}

#[test]
fn planned_reference_operations_verify_across_if_while_and_short_circuit_cfgs() {
    let output = TestOutputDirectory::new("planned-reference-control-flow");
    let source = "class Counter(start: Int):\n  private var current: Int = start\n  private var enabled: Boolean = true\n  def incrementUntil(limit: Int): Int =\n    while current < limit do current = current + 1\n    current\n  def enabledUntil(limit: Int): Int =\n    while enabled && current < limit do current = current + 1\n    current\n  def setEnabled(flag: Boolean): Int =\n    if flag then enabled = true else enabled = false\n    if enabled then current else 0\n  def get(): Int = current\n  def increment(): Unit = current = current + 1\nclass CounterClient:\n  def run(limit: Int): Int =\n    val counter = new Counter(0)\n    while counter.get() < limit do counter.increment()\n    counter.get()\n  def choose(flag: Boolean): Int =\n    if flag then new Counter(1).get() else new Counter(2).get()\n  def chooseLocal(flag: Boolean): Int =\n    val counter = if flag then new Counter(5) else new Counter(6)\n    counter.get()\n  def ifIncrement(flag: Boolean): Int =\n    val counter = new Counter(0)\n    if flag then counter.increment() else counter.increment()\n    counter.get()\n  def runAnd(running: Boolean, limit: Int): Int =\n    val counter = new Counter(0)\n    while running && counter.get() < limit do counter.increment()\n    counter.get()\n";
    let artifacts = compile_planned_constructor_source(source, output.path())
        .expect("narrow planned reference operations should compose with verified CFGs");
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Counter", "CounterClient"]
    );

    let counter_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Counter")
        .unwrap();
    let mut reader = Reader::new(counter_artifact.class.bytes());
    let counter = ClassFile::decode(&mut reader).expect("decode Counter");
    counter
        .validate_references()
        .expect("Counter field and method references are valid");
    for method in ["incrementUntil", "enabledUntil", "setEnabled"] {
        assert!(
            has_stack_map_table(&counter, method),
            "{method} must carry generated CFG frames"
        );
    }
    let increment_until = emitted_method_code(&counter, "incrementUntil");
    assert!(
        increment_until.contains(&0xb4),
        "loop reads the current field"
    );
    assert!(
        increment_until.contains(&0xb5),
        "loop writes the current field"
    );
    let set_enabled = emitted_method_code(&counter, "setEnabled");
    assert!(
        set_enabled.contains(&0xb5),
        "if branches write the Boolean field"
    );
    assert!(set_enabled.contains(&0xb4), "if reads the Boolean field");

    let client_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "CounterClient")
        .unwrap();
    let mut reader = Reader::new(client_artifact.class.bytes());
    let client = ClassFile::decode(&mut reader).expect("decode CounterClient");
    client
        .validate_references()
        .expect("CounterClient references are valid");
    for method in ["run", "choose", "chooseLocal", "ifIncrement", "runAnd"] {
        assert!(
            has_stack_map_table(&client, method),
            "{method} must carry generated CFG frames"
        );
    }
    assert!(
        stack_map_frame_pcs(&client, "run")
            .windows(2)
            .all(|frames| frames[0] < frames[1]),
        "loop frame PCs must be strictly ordered"
    );
    assert_invocation_sequence(
        &client,
        emitted_method_code(&client, "run"),
        &[
            (0xb7, "Counter", "<init>", "(I)V"),
            (0xb6, "Counter", "get", "()I"),
            (0xb6, "Counter", "increment", "()V"),
            (0xb6, "Counter", "get", "()I"),
        ],
    );

    if require_jdk_or_skip() {
        verify_reference_control_flow_with_jdk(output.path());
    }
}

#[test]
fn real_counter_control_flow_fixture_has_exact_frames_and_reproducible_classes() {
    let first_output = TestOutputDirectory::new("counter-control-flow-first");
    let second_output = TestOutputDirectory::new("counter-control-flow-second");
    let first = compile_fixture_classes("CounterControlFlow.scala", first_output.path())
        .expect("real Scala source runs through lexer, parser, namer, typer and JVM backend");
    let second = compile_fixture_classes("CounterControlFlow.scala", second_output.path())
        .expect("a fresh frontend session compiles the same source independently");
    let names = first
        .iter()
        .map(|artifact| artifact.class.internal_name())
        .collect::<Vec<_>>();
    assert_eq!(names, ["Counter", "CounterClient"]);
    assert_eq!(
        second
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        names,
        "classes retain source order in the independent session"
    );
    assert_eq!(first_output.class_files().len(), 2);
    assert_eq!(second_output.class_files().len(), 2);
    for (left, right) in first.iter().zip(&second) {
        assert_eq!(
            left.class.bytes(),
            right.class.bytes(),
            "{} class bytes are deterministic across independent frontend sessions",
            left.class.internal_name()
        );
        assert_eq!(
            fs::read(&left.path).expect("read published class file"),
            left.class.bytes(),
            "{} is published byte-for-byte from its validated in-memory class",
            left.class.internal_name()
        );
    }

    let mut reader = Reader::new(first[0].class.bytes());
    let counter = ClassFile::decode(&mut reader).expect("fully decode Counter.class");
    counter
        .validate_references()
        .expect("Counter constant pool validates");
    assert_eq!(counter.version.major, 65);
    assert_eq!(
        counter.constant_pool.class_name(counter.this_class),
        Ok("Counter")
    );
    assert_eq!(
        counter.constant_pool.class_name(
            counter
                .super_class
                .expect("ordinary class has Object superclass")
        ),
        Ok("java/lang/Object")
    );
    assert!(counter.access_flags.is_public());
    assert!(!counter.access_flags.is_interface());
    assert_eq!(counter.fields.len(), 1);
    let field = &counter.fields[0];
    assert_eq!(counter.constant_pool.utf8(field.name_index), Ok("current"));
    assert_eq!(counter.constant_pool.utf8(field.descriptor_index), Ok("I"));
    assert!(field.access_flags.is_private());
    assert!(!field.access_flags.is_final());
    assert!(
        counter
            .methods
            .iter()
            .all(|method| method.access_flags.is_public())
    );
    assert_eq!(
        methods_with_descriptors(&counter),
        [
            ("<init>".to_owned(), "(I)V".to_owned()),
            ("get".to_owned(), "()I".to_owned()),
            ("increment".to_owned(), "()I".to_owned()),
            ("incrementUntil".to_owned(), "(I)I".to_owned()),
        ]
    );

    let increment_until = emitted_method_code(&counter, "incrementUntil");
    let counter_targets = decode_branch_targets(increment_until);
    assert!(
        counter_targets
            .iter()
            .any(|(offset, _, target)| target < offset)
    );
    assert!(
        counter_targets
            .iter()
            .any(|(offset, opcode, target)| *opcode == 0xa7 && target < offset)
    );
    assert!(
        counter_targets
            .iter()
            .any(|(_, opcode, _)| (0x9f..=0xa4).contains(opcode))
    );
    let counter_ops = decode_subset_instructions(increment_until);
    assert!(counter_ops.iter().any(|(_, opcode)| *opcode == 0x2a));
    assert!(counter_ops.iter().any(|(_, opcode)| *opcode == 0x1b));
    assert!(
        counter_ops
            .iter()
            .any(|(_, opcode)| (0x36..=0x3a).contains(opcode) || (0x3b..=0x3e).contains(opcode))
    );
    assert_field_reference(&counter, increment_until, 0xb4, "Counter", "current", "I");
    assert_field_reference(&counter, increment_until, 0xb5, "Counter", "current", "I");
    let counter_frames = stack_map_frames(&counter, "incrementUntil");
    let counter_frame_pcs = counter_frames
        .iter()
        .map(|frame| frame.pc)
        .collect::<Vec<_>>();
    assert!(
        counter_targets
            .iter()
            .all(|(_, _, target)| counter_frame_pcs.contains(target))
    );
    let (_, _, counter_loop_header) = counter_targets
        .iter()
        .copied()
        .find(|(offset, _, target)| target < offset)
        .expect("Counter.incrementUntil has a loop back-edge");
    let counter_loop_frame = counter_frames
        .iter()
        .find(|frame| frame.pc == counter_loop_header)
        .expect("loop header has a full_frame entry");
    assert_eq!(
        counter_loop_frame.locals,
        [
            StackMapVerificationType::Object("Counter".to_owned()),
            StackMapVerificationType::Integer,
        ]
    );
    assert!(counter_loop_frame.stack.is_empty());

    let mut reader = Reader::new(first[1].class.bytes());
    let client = ClassFile::decode(&mut reader).expect("fully decode CounterClient.class");
    client
        .validate_references()
        .expect("CounterClient constant pool validates");
    assert_eq!(client.version.major, 65);
    assert_eq!(
        client.constant_pool.class_name(client.this_class),
        Ok("CounterClient")
    );
    assert_eq!(
        client.constant_pool.class_name(
            client
                .super_class
                .expect("ordinary class has Object superclass")
        ),
        Ok("java/lang/Object")
    );
    assert!(client.access_flags.is_public());
    assert!(!client.access_flags.is_interface());
    assert!(client.fields.is_empty());
    assert!(
        client
            .methods
            .iter()
            .all(|method| method.access_flags.is_public())
    );
    assert_eq!(
        methods_with_descriptors(&client),
        [
            ("<init>".to_owned(), "()V".to_owned()),
            ("run".to_owned(), "(I)I".to_owned()),
            ("choose".to_owned(), "(Z)I".to_owned()),
        ]
    );

    let run = emitted_method_code(&client, "run");
    let run_instructions = decode_subset_instructions(run);
    let run_targets = decode_branch_targets(run);
    assert!(
        run_targets
            .iter()
            .any(|(offset, _, target)| target < offset)
    );
    assert!(
        run_targets
            .iter()
            .any(|(_, opcode, _)| (0x9f..=0xa4).contains(opcode))
    );
    let run_frames = stack_map_frames(&client, "run");
    let run_frame_pcs = run_frames.iter().map(|frame| frame.pc).collect::<Vec<_>>();
    assert!(
        run_targets
            .iter()
            .all(|(_, _, target)| run_frame_pcs.contains(target))
    );
    let (_, _, run_loop_header) = run_targets
        .iter()
        .copied()
        .find(|(offset, _, target)| target < offset)
        .expect("CounterClient.run has a loop back-edge");
    let run_loop_frame = run_frames
        .iter()
        .find(|frame| frame.pc == run_loop_header)
        .expect("run loop header has a full_frame entry");
    assert_eq!(
        run_loop_frame.locals,
        [
            StackMapVerificationType::Object("CounterClient".to_owned()),
            StackMapVerificationType::Integer,
            StackMapVerificationType::Object("Counter".to_owned()),
        ],
        "the loop frame retains exact receiver, limit and initialized Counter locals"
    );
    assert!(run_loop_frame.stack.is_empty());
    assert!(run_instructions.iter().any(|(_, opcode)| *opcode == 0x2c));
    assert_invocation_sequence(
        &client,
        run,
        &[
            (0xb7, "Counter", "<init>", "(I)V"),
            (0xb6, "Counter", "get", "()I"),
            (0xb6, "Counter", "increment", "()I"),
            (0xb6, "Counter", "get", "()I"),
        ],
    );
    assert_invocation_sequence(
        &client,
        emitted_method_code(&client, "<init>"),
        &[(0xb7, "java/lang/Object", "<init>", "()V")],
    );

    let choose = emitted_method_code(&client, "choose");
    let choose_instructions = decode_subset_instructions(choose);
    assert!(
        choose_instructions
            .iter()
            .any(|(_, opcode)| *opcode == 0x99)
    );
    let allocations = choose_instructions
        .iter()
        .filter_map(|(offset, opcode)| (*opcode == 0xbb).then_some(*offset))
        .collect::<Vec<_>>();
    assert_eq!(allocations.len(), 2);
    let constructor_offsets = choose_instructions
        .iter()
        .filter_map(|(offset, opcode)| (*opcode == 0xb7).then_some(*offset))
        .collect::<Vec<_>>();
    assert_eq!(constructor_offsets.len(), 2);
    for (allocation, constructor) in allocations.iter().zip(&constructor_offsets) {
        let allocation_position = choose_instructions
            .iter()
            .position(|(offset, _)| offset == allocation)
            .expect("find new instruction boundary");
        let constructor_position = choose_instructions
            .iter()
            .position(|(offset, _)| offset == constructor)
            .expect("find invokespecial instruction boundary");
        assert_eq!(
            choose_instructions[allocation_position..=constructor_position]
                .iter()
                .map(|(_, opcode)| *opcode)
                .collect::<Vec<_>>(),
            [0xbb, 0x59, 0x10, 0xb7],
            "each branch completes new/dup/Int-argument/invokespecial in one block"
        );
        let class_index = u16::from_be_bytes([choose[allocation + 1], choose[allocation + 2]]);
        let ConstantPoolEntry::Class { name_index } = client
            .constant_pool
            .get(ConstantPoolIndex(class_index))
            .expect("new must reference CONSTANT_Class")
        else {
            panic!("new must reference a class constant");
        };
        assert_eq!(client.constant_pool.utf8(*name_index), Ok("Counter"));
        assert!(constructor > allocation);
        assert!(
            choose_instructions
                .iter()
                .filter(|(offset, _)| *offset > *allocation && *offset < *constructor)
                .all(|(_, opcode)| !matches!(*opcode, 0x99..=0xa7))
        );
        let method_index = u16::from_be_bytes([choose[constructor + 1], choose[constructor + 2]]);
        let ConstantPoolEntry::Methodref {
            class_index,
            name_and_type_index,
        } = client
            .constant_pool
            .get(ConstantPoolIndex(method_index))
            .expect("invokespecial must reference CONSTANT_Methodref")
        else {
            panic!("constructor call must reference a method");
        };
        assert_eq!(client.constant_pool.class_name(*class_index), Ok("Counter"));
        assert_eq!(
            client.constant_pool.name_and_type(*name_and_type_index),
            Ok(("<init>", "(I)V"))
        );
    }
    assert_invocation_sequence(
        &client,
        choose,
        &[
            (0xb7, "Counter", "<init>", "(I)V"),
            (0xb6, "Counter", "get", "()I"),
            (0xb7, "Counter", "<init>", "(I)V"),
            (0xb6, "Counter", "get", "()I"),
        ],
    );
    let choose_targets = decode_branch_targets(choose);
    let choose_frames = stack_map_frames(&client, "choose");
    let choose_frame_pcs = choose_frames
        .iter()
        .map(|frame| frame.pc)
        .collect::<Vec<_>>();
    assert!(
        choose_targets
            .iter()
            .all(|(_, _, target)| choose_frame_pcs.contains(target))
    );
    assert!(choose_frames.iter().all(|frame| {
        frame.locals.first() == Some(&StackMapVerificationType::Object("CounterClient".into()))
    }));
    assert!(choose_frames.iter().all(|frame| {
        frame.stack.is_empty() || frame.stack == [StackMapVerificationType::Integer]
    }));

    if require_jdk_or_skip() {
        verify_counter_control_flow_with_jdk(first_output.path());
    }
}

#[test]
fn immutable_reference_locals_use_canonical_slots_and_verify_on_the_jvm() {
    let output = TestOutputDirectory::new("reference-local");
    let artifacts = compile_planned_constructor_source(
        "class Counter(seed: Int)\nclass Client:\n  def run(seed: Int): Int =\n    val counter = new Counter(seed)\n    counter\n    42\n",
        output.path(),
    )
    .unwrap();
    let client = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Client")
        .expect("Client source class is emitted");
    let mut reader = Reader::new(client.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("decode Client class");
    class
        .validate_references()
        .expect("valid Client references");
    let code = emitted_method_code(&class, "run");
    assert_opcode_sequence(
        code,
        &[0xbb, 0x59, 0x1b, 0xb7, 0x4d, 0x2c, 0x57, 0x10, 0xac],
    );
    let allocation = u16::from_be_bytes([code[1], code[2]]);
    let ConstantPoolEntry::Class { name_index } = class
        .constant_pool
        .get(ConstantPoolIndex(allocation))
        .expect("Counter class constant")
    else {
        panic!("new must reference a class entry")
    };
    assert_eq!(class.constant_pool.utf8(*name_index), Ok("Counter"));
    let invocation = u16::from_be_bytes([code[6], code[7]]);
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = class
        .constant_pool
        .get(ConstantPoolIndex(invocation))
        .expect("constructor method reference")
    else {
        panic!("invokespecial must reference a method")
    };
    assert_eq!(class.constant_pool.class_name(*class_index), Ok("Counter"));
    assert_eq!(
        class.constant_pool.name_and_type(*name_and_type_index),
        Ok(("<init>", "(I)V"))
    );

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 output path");
        let harness = output.path().join("ReferenceLocalHarness.java");
        fs::write(
            &harness,
            "public final class ReferenceLocalHarness { public static void main(String[] args) { if (new Client().run(10) != 42) throw new AssertionError(); } }\n",
        )
        .expect("write JDK verification harness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected reference-local harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "ReferenceLocalHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected reference-local classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
    }
}

#[test]
fn reference_local_accepts_a_straight_line_block_initializer() {
    let output = TestOutputDirectory::new("reference-local-block-initializer");
    let artifacts = compile_planned_constructor_source(
        "class Counter(seed: Int)\nclass Client:\n  def run(seed: Int): Int =\n    val counter = { val x = seed + 1; new Counter(x) }\n    counter\n    42\n",
        output.path(),
    )
    .unwrap();
    let client = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Client")
        .expect("Client source class is emitted");
    let mut reader = Reader::new(client.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("decode Client class");
    class
        .validate_references()
        .expect("valid Client references");
    let code = emitted_method_code(&class, "run");
    assert_opcode_sequence(
        code,
        &[
            0x1b, 0x04, 0x60, 0x3d, 0xbb, 0x59, 0x1c, 0xb7, 0x4e, 0x2d, 0x57, 0x10, 0xac,
        ],
    );
}

#[test]
fn locals_keep_distinct_planned_class_identities_in_one_method() {
    let output = TestOutputDirectory::new("distinct-reference-local-types");
    let artifacts = compile_planned_constructor_source(
        "class Counter(seed: Int)\nclass Other(seed: Int)\nclass Client:\n  def run(seed: Int): Int =\n    val counter = new Counter(seed)\n    val other = new Other(seed)\n    counter\n    other\n    42\n",
        output.path(),
    )
    .unwrap();
    let client = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Client")
        .expect("Client source class is emitted");
    let mut reader = Reader::new(client.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("decode Client class");
    class
        .validate_references()
        .expect("valid Client references");
    let code = emitted_method_code(&class, "run");
    assert_opcode_sequence(
        code,
        &[
            0xbb, 0x59, 0x1b, 0xb7, 0x4d, 0xbb, 0x59, 0x1b, 0xb7, 0x4e, 0x2c, 0x57, 0x2d, 0x57,
            0x10, 0xac,
        ],
    );
    let class_name = |offset| {
        let index = u16::from_be_bytes([code[offset + 1], code[offset + 2]]);
        let ConstantPoolEntry::Class { name_index } = class
            .constant_pool
            .get(ConstantPoolIndex(index))
            .expect("allocated planned class")
        else {
            panic!("new must reference a class entry")
        };
        class.constant_pool.utf8(*name_index).unwrap()
    };
    let allocations = decode_subset_instructions(code)
        .into_iter()
        .filter_map(|(offset, opcode)| (opcode == 0xbb).then_some(offset))
        .collect::<Vec<_>>();
    assert_eq!(allocations.len(), 2);
    assert_eq!(class_name(allocations[0]), "Counter");
    assert_eq!(class_name(allocations[1]), "Other");
}

#[test]
fn mutable_reference_local_initialization_uses_a_category_one_slot() {
    let output = TestOutputDirectory::new("reference-var");
    let artifacts = compile_planned_constructor_source(
        "class Counter(seed: Int)\nclass Client:\n  def run(seed: Int): Int =\n    var counter = new Counter(seed)\n    42\n",
        output.path(),
    )
    .expect("a planned mutable reference local can be initialized");
    assert!(
        !artifacts.is_empty(),
        "the successful source compilation returns its planned class batch"
    );
    let client = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Client")
        .expect("Client class is emitted");
    let mut reader = Reader::new(client.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("decode Client class");
    let code = emitted_method_code(&class, "run");
    assert!(
        code.contains(&0x4d),
        "the mutable Counter local is stored once in category-one slot 2: {code:?}"
    );
}

#[test]
fn mutable_reference_local_assignment_accepts_a_checked_subtype_and_dispatches() {
    let output = TestOutputDirectory::new("reference-var-assignment");
    let source = "class Base(seed: Int):\n  private var current: Int = seed\n  def value(): Int = current\nclass Left(seed: Int) extends Base(seed):\n  override def value(): Int = super.value() + 1\nclass Right(seed: Int) extends Base(seed):\n  override def value(): Int = super.value() + 2\nclass Runner:\n  def change(seed: Int): Base =\n    var current: Base = new Left(seed)\n    current = new Right(seed)\n    current\n  def same(seed: Int): Base =\n    var current: Base = new Left(seed)\n    current = new Base(seed)\n    current\n  def unchanged(seed: Int): Base =\n    var current: Base = new Left(seed)\n    current\n";
    let artifacts = compile_planned_constructor_source(source, output.path())
        .expect("a mutable planned-reference local accepts a checked subtype assignment");
    let runner = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Runner")
        .expect("Runner source class is emitted");
    let mut reader = Reader::new(runner.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("decode Runner class");
    class
        .validate_references()
        .expect("valid Runner references");
    let change = emitted_method_code(&class, "change");
    assert_eq!(
        decode_subset_instructions(change)
            .iter()
            .map(|(_, opcode)| *opcode)
            .collect::<Vec<_>>(),
        [
            0xbb, 0x59, 0x1b, 0xb7, 0x4d, 0xbb, 0x59, 0x1b, 0xb7, 0x4d, 0x2c, 0xb0
        ],
        "the reassigned reference is loaded from its canonical slot and returned"
    );
    assert_eq!(
        decode_subset_instructions(change)
            .iter()
            .map(|(_, opcode)| *opcode)
            .filter(|opcode| *opcode == 0x4d)
            .count(),
        2,
        "the initial declaration and reassignment each store one reference"
    );
    assert_invocation_sequence(
        &class,
        change,
        &[
            (0xb7, "Left", "<init>", "(I)V"),
            (0xb7, "Right", "<init>", "(I)V"),
        ],
    );
    assert_invocation_sequence(
        &class,
        emitted_method_code(&class, "unchanged"),
        &[(0xb7, "Left", "<init>", "(I)V")],
    );
    assert_invocation_sequence(
        &class,
        emitted_method_code(&class, "same"),
        &[
            (0xb7, "Left", "<init>", "(I)V"),
            (0xb7, "Base", "<init>", "(I)V"),
        ],
    );

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 output path");
        let harness = output.path().join("MutableReferenceAssignmentHarness.java");
        fs::write(
            &harness,
            "public final class MutableReferenceAssignmentHarness { public static void main(String[] args) { Runner runner = new Runner(); int[][] cases = {{10, 12, 11}, {0, 2, 1}, {-3, -1, -2}}; for (int[] test : cases) { if (runner.change(test[0]).value() != test[1] || runner.same(test[0]).value() != test[0] || runner.unchanged(test[0]).value() != test[2]) throw new AssertionError(java.util.Arrays.toString(test)); } } }\n",
        )
        .expect("write mutable-reference assignment harness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected mutable-reference assignment harness\\nstdout:\\n{}\\nstderr:\\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "MutableReferenceAssignmentHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected mutable-reference assignment classes\\nstdout:\\n{}\\nstderr:\\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
    }
}

#[test]
fn contextual_sibling_if_results_initialize_and_assign_declared_reference_locals() {
    let output = TestOutputDirectory::new("contextual-sibling-if-local-values");
    let source = "class Base(seed: Int):\n  private var stored: Int = seed\n  def value(): Int = stored\nclass Left(seed: Int) extends Base(seed):\n  override def value(): Int = super.value() + 1\nclass Right(seed: Int) extends Base(seed):\n  override def value(): Int = super.value() + 2\nclass Runner:\n  def init(flag: Boolean, seed: Int): Base =\n    var current: Base = if flag then new Left(seed) else new Right(seed)\n    current\n  def initReverse(flag: Boolean, seed: Int): Base =\n    var current: Base = if flag then new Right(seed) else new Left(seed)\n    current\n  def replace(flag: Boolean, seed: Int): Base =\n    var current: Base = new Left(seed)\n    current = if flag then new Right(seed) else new Left(seed)\n    current\n  def replaceNested(flag: Boolean, seed: Int): Base =\n    var current: Base = new Left(seed)\n    current = { if flag then new Right(seed) else new Left(seed) }\n    current\n";
    let mut observed_methods = 0;
    let artifacts = compile_source_batch_observed(
        source,
        "ContextualSiblingIf.scala",
        output.path(),
        SourceId::from_index(1458),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut |unit, definitions| {
            let runner = audit_class_named(unit, "Runner");
            let base = audit_class_named(unit, "Base");
            for method_name in ["init", "initReverse", "replace", "replaceNested"] {
                observed_methods += 1;
                let method = audit_method_named(unit, runner, method_name);
                let local = method
                    .locals
                    .iter()
                    .find(|binding| {
                        unit.store
                            .names
                            .resolve(unit.store.symbols.get(binding.symbol).name.text())
                            == "current"
                    })
                    .expect("each contextual method declares current");
                let dotty_core::SymbolInfo::Complete(declared_type) =
                    unit.store.symbols.info(local.symbol)
                else {
                    panic!("current keeps its canonical declared source type")
                };
                assert_eq!(audit_type_symbol(unit, *declared_type), Some(base));
                let body = method.body.expect("method has a typed body");
                let mut nodes = Vec::new();
                walk_typed_subtree(unit, body, &mut |tree| nodes.push(tree));
                let conditionals = nodes
                    .iter()
                    .filter(|tree| matches!(unit.typed_ast.get(**tree).kind, TreeKind::If(_)))
                    .copied()
                    .collect::<Vec<_>>();
                assert_eq!(conditionals.len(), 1, "{method_name} has one sibling If");
                let conditional = conditionals[0];
                assert!(
                    matches!(
                        unit.store.types.get(unit.typed_ast.get(conditional).ty),
                        Type::Or { .. }
                    ),
                    "{method_name} retains the original source Type::Or"
                );
                let projection = unit
                    .project_sibling_union_to_expected_type(
                        unit.typed_ast.get(conditional).ty,
                        *declared_type,
                        definitions,
                    )
                    .expect("declared Base supplies the existing checked projection context");
                assert_eq!(projection.representation(), JvmType::Reference(base));
            }
        }),
    )
    .expect("declared Base locals accept their checked sibling If values");
    assert_eq!(observed_methods, 4);
    let runner = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Runner")
        .expect("Runner class is emitted");
    let mut reader = Reader::new(runner.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("decode Runner class");
    class
        .validate_references()
        .expect("Runner's classfile references are valid");

    for method in ["init", "initReverse", "replace", "replaceNested"] {
        let code = emitted_method_code(&class, method);
        let expected_stores = usize::from(method.starts_with("replace")) + 1;
        assert_eq!(
            decode_subset_instructions(code)
                .iter()
                .filter(|(_, opcode)| matches!(opcode, 0x3a | 0x4b..=0x4e))
                .count(),
            expected_stores,
            "{method} stores the initialized local once and adds one store only for reassignment"
        );
        assert!(
            decode_subset_instructions(code)
                .iter()
                .all(|(_, opcode)| *opcode != 0xc0),
            "{method} projects the proven union to Base without checkcast"
        );
        assert!(
            stack_map_frames(&class, method).iter().any(|frame| {
                frame.stack == [StackMapVerificationType::Object("Base".to_owned())]
            }),
            "{method} merges the two branch values as Base on the operand stack"
        );
    }
    assert_invocation_sequence(
        &class,
        emitted_method_code(&class, "init"),
        &[
            (0xb7, "Left", "<init>", "(I)V"),
            (0xb7, "Right", "<init>", "(I)V"),
        ],
    );
    assert_invocation_sequence(
        &class,
        emitted_method_code(&class, "initReverse"),
        &[
            (0xb7, "Right", "<init>", "(I)V"),
            (0xb7, "Left", "<init>", "(I)V"),
        ],
    );
    assert_invocation_sequence(
        &class,
        emitted_method_code(&class, "replace"),
        &[
            (0xb7, "Left", "<init>", "(I)V"),
            (0xb7, "Right", "<init>", "(I)V"),
            (0xb7, "Left", "<init>", "(I)V"),
        ],
    );
    assert_invocation_sequence(
        &class,
        emitted_method_code(&class, "replaceNested"),
        &[
            (0xb7, "Left", "<init>", "(I)V"),
            (0xb7, "Right", "<init>", "(I)V"),
            (0xb7, "Left", "<init>", "(I)V"),
        ],
    );

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 output path");
        let harness = output.path().join("ContextualSiblingIfHarness.java");
        fs::write(
            &harness,
            "public final class ContextualSiblingIfHarness { private static void check(Base value, int expected) { if (value.value() != expected) throw new AssertionError(value.getClass() + \" -> \" + value.value()); } public static void main(String[] args) { Runner runner = new Runner(); check(runner.init(true, 10), 11); check(runner.init(false, 10), 12); check(runner.initReverse(true, 10), 12); check(runner.initReverse(false, 10), 11); check(runner.replace(true, 10), 12); check(runner.replace(false, 10), 11); check(runner.replaceNested(true, 10), 12); check(runner.replaceNested(false, 10), 11); } }\n",
        )
        .expect("write contextual sibling If harness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected contextual sibling If harness\\nstdout:\\n{}\\nstderr:\\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "ContextualSiblingIfHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected contextual sibling If classes\\nstdout:\\n{}\\nstderr:\\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
    }
}

#[test]
fn contextual_sibling_if_projection_rejects_inferred_and_incompatible_targets() {
    #[derive(Clone, Copy)]
    enum ExpectedFailure {
        UnsupportedReferenceType,
        LocalValueTypeMismatch,
    }

    let cases = [
        (
            "inferred-sibling-if-local-rejected",
            "class Base(seed: Int)\nclass Left(seed: Int) extends Base(seed)\nclass Right(seed: Int) extends Base(seed)\nclass Runner:\n  def infer(flag: Boolean, seed: Int): Int =\n    var current = if flag then new Left(seed) else new Right(seed)\n    42\n",
            ExpectedFailure::UnsupportedReferenceType,
        ),
        (
            "narrow-sibling-if-local-rejected",
            "class Base(seed: Int)\nclass Left(seed: Int) extends Base(seed)\nclass Right(seed: Int) extends Base(seed)\nclass Runner:\n  def narrow(flag: Boolean, seed: Int): Int =\n    var current: Left = if flag then new Left(seed) else new Right(seed)\n    42\n",
            ExpectedFailure::LocalValueTypeMismatch,
        ),
        (
            "unrelated-sibling-if-local-rejected",
            "class Base(seed: Int)\nclass Left(seed: Int) extends Base(seed)\nclass Other(seed: Int)\nclass Runner:\n  def unrelated(flag: Boolean, seed: Int): Int =\n    var current: Base = if flag then new Left(seed) else new Other(seed)\n    42\n",
            ExpectedFailure::LocalValueTypeMismatch,
        ),
    ];

    for (label, source, expected_failure) in cases {
        let output = TestOutputDirectory::new(label);
        let failure = compile_planned_constructor_source(source, output.path())
            .expect_err("unsupported or incompatible sibling unions must fail closed");
        let matches_expected = match expected_failure {
            ExpectedFailure::UnsupportedReferenceType => matches!(
                &failure,
                PipelineFailure::Codegen(JvmCodegenError::UnsupportedJvmReferenceType { .. })
            ),
            ExpectedFailure::LocalValueTypeMismatch => matches!(
                &failure,
                PipelineFailure::Typer(TyperError::LocalValueTypeMismatch { .. })
            ),
        };
        assert!(
            matches_expected,
            "{label} failed with an unexpected error: {failure:?}"
        );
        assert!(
            output.class_files().is_empty(),
            "{label} must not publish any class from a rejected source batch"
        );
    }
}

#[test]
fn mutable_reference_local_assignments_join_across_if_and_while_and_dispatch() {
    let output = TestOutputDirectory::new("reference-var-if-while-joins");
    let source = "class Base(seed: Int):\n  private val stored: Int = seed\n  def value(): Int = stored\nclass Left(seed: Int) extends Base(seed):\n  override def value(): Int = super.value() + 1\nclass Right(seed: Int) extends Base(seed):\n  override def value(): Int = super.value() + 2\nclass Runner:\n  def choose(flag: Boolean, seed: Int): Base =\n    var current: Base = new Left(seed)\n    if flag then current = new Right(seed)\n    current\n  def chooseReverse(flag: Boolean, seed: Int): Base =\n    var current: Base = new Right(seed)\n    if flag then current = new Left(seed)\n    current\n  def repeat(n: Int, seed: Int): Base =\n    var current: Base = new Left(seed)\n    var i = 0\n    while i < n do\n      current = new Right(seed)\n      i = i + 1\n    current\n  def nested(outer: Boolean, inner: Boolean, seed: Int): Base =\n    var current: Base = new Right(seed)\n    if outer then\n      if inner then current = new Left(seed) else current = new Right(seed)\n    else current = new Left(seed)\n    current\n";
    let source = format!(
        "{source}  def early(flag: Boolean, seed: Int): Base =\n    var current: Base = new Left(seed)\n    if flag then current = new Right(seed) else return current\n    current\n"
    );
    let artifacts = compile_planned_constructor_source(&source, output.path())
        .expect("initialized mutable reference locals join across supported control flow");
    let runner = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Runner")
        .expect("Runner class is emitted");
    let mut reader = Reader::new(runner.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("decode Runner class");
    class
        .validate_references()
        .expect("Runner's classfile references are valid");

    for (method, local_slot) in [
        ("choose", 3),
        ("chooseReverse", 3),
        ("repeat", 3),
        ("nested", 4),
    ] {
        let code = emitted_method_code(&class, method);
        assert!(
            decode_subset_instructions(code)
                .iter()
                .all(|(_, opcode)| *opcode != 0xc0),
            "{method} joins the local at its declared Base type without checkcast"
        );
        assert!(
            stack_map_frames(&class, method).iter().any(|frame| {
                frame.locals.get(local_slot)
                    == Some(&StackMapVerificationType::Object("Base".to_owned()))
            }),
            "{method} encodes canonical local slot {local_slot} as Object(Base) at a CFG join"
        );
    }
    for (method, local_slot) in [("choose", 3), ("chooseReverse", 3), ("nested", 4)] {
        let code = emitted_method_code(&class, method);
        let join_targets = decode_branch_targets(code)
            .into_iter()
            .filter_map(|(_, opcode, target)| (opcode == 0xa7).then_some(target))
            .collect::<Vec<_>>();
        let frames = stack_map_frames(&class, method);
        assert!(
            join_targets.iter().any(|target| {
                frames.iter().any(|frame| {
                    frame.pc == *target
                        && frame.locals.get(local_slot)
                            == Some(&StackMapVerificationType::Object("Base".to_owned()))
                })
            }),
            "{method} has an Object(Base) local frame at a reachable branch join"
        );
    }
    let repeat_code = emitted_method_code(&class, "repeat");
    let repeat_targets = decode_branch_targets(repeat_code);
    let goto_targets = repeat_targets
        .iter()
        .filter_map(|(_, opcode, target)| (*opcode == 0xa7).then_some(*target))
        .collect::<Vec<_>>();
    let loop_header = goto_targets
        .iter()
        .copied()
        .find(|target| {
            goto_targets
                .iter()
                .filter(|candidate| *candidate == target)
                .count()
                == 2
        })
        .expect("both loop entry and back-edge target the symbolic header");
    let repeat_frames = stack_map_frames(&class, "repeat");
    assert!(
        repeat_frames.iter().any(|frame| {
            frame.pc == loop_header
                && frame.locals.get(3) == Some(&StackMapVerificationType::Object("Base".to_owned()))
                && frame.locals.get(4) == Some(&StackMapVerificationType::Integer)
        }),
        "the loop-header frame retains the primitive index beside the joined Base local"
    );
    let loop_exit = repeat_targets
        .iter()
        .find_map(|(_, opcode, target)| (*opcode == 0x99).then_some(*target))
        .expect("the false while-condition edge targets the loop exit");
    assert!(
        repeat_frames.iter().any(|frame| frame.pc == loop_exit
            && frame.locals.get(3) == Some(&StackMapVerificationType::Object("Base".to_owned()))),
        "the zero-iteration exit frame preserves the initialized Base local"
    );
    for (method, expected_stores) in [
        ("choose", 2),
        ("chooseReverse", 2),
        ("repeat", 2),
        ("nested", 4),
        ("early", 2),
    ] {
        let stores = decode_subset_instructions(emitted_method_code(&class, method))
            .iter()
            .filter(|(_, opcode)| matches!(opcode, 0x3a | 0x4b..=0x4e))
            .count();
        assert_eq!(
            stores, expected_stores,
            "{method} emits each local store once"
        );
    }
    for (method, expected_constructors) in [
        (
            "choose",
            vec![
                (0xb7, "Left", "<init>", "(I)V"),
                (0xb7, "Right", "<init>", "(I)V"),
            ],
        ),
        (
            "chooseReverse",
            vec![
                (0xb7, "Right", "<init>", "(I)V"),
                (0xb7, "Left", "<init>", "(I)V"),
            ],
        ),
        (
            "repeat",
            vec![
                (0xb7, "Left", "<init>", "(I)V"),
                (0xb7, "Right", "<init>", "(I)V"),
            ],
        ),
        (
            "nested",
            vec![
                (0xb7, "Right", "<init>", "(I)V"),
                (0xb7, "Left", "<init>", "(I)V"),
                (0xb7, "Right", "<init>", "(I)V"),
                (0xb7, "Left", "<init>", "(I)V"),
            ],
        ),
        (
            "early",
            vec![
                (0xb7, "Left", "<init>", "(I)V"),
                (0xb7, "Right", "<init>", "(I)V"),
            ],
        ),
    ] {
        assert_invocation_sequence(
            &class,
            emitted_method_code(&class, method),
            &expected_constructors,
        );
    }

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 output path");
        let harness = output.path().join("MutableReferenceCfgHarness.java");
        fs::write(
            &harness,
            r#"public final class MutableReferenceCfgHarness {
  private static void check(Base value, String type, int expected) {
    if (!value.getClass().getSimpleName().equals(type) || value.value() != expected)
      throw new AssertionError(value.getClass() + " -> " + value.value() + ", expected " + type + "/" + expected);
  }
  public static void main(String[] args) {
    Runner runner = new Runner();
    for (int seed : new int[] {10, 0, -3}) {
      check(runner.choose(false, seed), "Left", seed + 1);
      check(runner.choose(true, seed), "Right", seed + 2);
      check(runner.chooseReverse(false, seed), "Right", seed + 2);
      check(runner.chooseReverse(true, seed), "Left", seed + 1);
      check(runner.repeat(0, seed), "Left", seed + 1);
      check(runner.repeat(1, seed), "Right", seed + 2);
      check(runner.repeat(3, seed), "Right", seed + 2);
      check(runner.nested(false, true, seed), "Left", seed + 1);
      check(runner.nested(true, false, seed), "Right", seed + 2);
      check(runner.nested(true, true, seed), "Left", seed + 1);
      check(runner.early(false, seed), "Left", seed + 1);
      check(runner.early(true, seed), "Right", seed + 2);
    }
  }
}
"#,
        )
        .expect("write mutable-reference CFG harness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected mutable-reference CFG harness\\nstdout:\\n{}\\nstderr:\\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "MutableReferenceCfgHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected mutable-reference CFG classes\\nstdout:\\n{}\\nstderr:\\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
    }
}

#[test]
fn mutable_reference_local_assignment_is_rejected_in_while_conditions() {
    let output = TestOutputDirectory::new("reference-var-while-condition-assignment-rejected");
    let result = compile_planned_constructor_source(
        "class Base(seed: Int)\nclass Leaf(seed: Int) extends Base(seed)\nclass Runner:\n  def change(flag: Boolean, seed: Int): Int =\n    var current: Base = new Leaf(seed)\n    while { current = new Leaf(seed); flag } do ()\n    42\n",
        output.path(),
    );
    assert!(
        matches!(
            result,
            Err(PipelineFailure::Codegen(
                JvmCodegenError::ReferenceLocalInUnsupportedControlFlow { .. }
            ))
        ),
        "assignments in a repeatedly evaluated while condition must be rejected"
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn mutable_reference_local_assignment_is_rejected_in_short_circuit_and_rhs() {
    let output = TestOutputDirectory::new("reference-var-short-circuit-and-assignment-rejected");
    let result = compile_planned_constructor_source(
        "class Base(seed: Int)\nclass Leaf(seed: Int) extends Base(seed)\nclass Runner:\n  def change(flag: Boolean, seed: Int): Int =\n    var current: Base = new Leaf(seed)\n    if flag && { current = new Leaf(seed); true } then 1 else 0\n",
        output.path(),
    );
    assert!(
        matches!(
            result,
            Err(PipelineFailure::Codegen(
                JvmCodegenError::ReferenceLocalInUnsupportedControlFlow { .. }
            ))
        ),
        "assignments in a conditionally evaluated && operand must be rejected"
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn mutable_reference_local_assignment_is_rejected_in_short_circuit_or_rhs() {
    let output = TestOutputDirectory::new("reference-var-short-circuit-or-assignment-rejected");
    let result = compile_planned_constructor_source(
        "class Base(seed: Int)\nclass Leaf(seed: Int) extends Base(seed)\nclass Runner:\n  def change(flag: Boolean, seed: Int): Int =\n    var current: Base = new Leaf(seed)\n    if flag || { current = new Leaf(seed); false } then 1 else 0\n",
        output.path(),
    );
    assert!(
        matches!(
            result,
            Err(PipelineFailure::Codegen(
                JvmCodegenError::ReferenceLocalInUnsupportedControlFlow { .. }
            ))
        ),
        "assignments in a conditionally evaluated || operand must be rejected"
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn initialized_mutable_reference_local_can_be_copied_into_an_immutable_local() {
    let output = TestOutputDirectory::new("reference-var-copy-accepted");
    let artifacts = compile_planned_constructor_source(
        "class Counter(seed: Int)\nclass Client:\n  def run(seed: Int): Int =\n    var counter = new Counter(seed)\n    val saved = counter\n    42\n",
        output.path(),
    )
    .expect("an initialized mutable reference local can be copied");
    let client = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Client")
        .expect("Client class is emitted");
    let mut reader = Reader::new(client.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("decode Client class");
    assert_eq!(
        decode_subset_instructions(emitted_method_code(&class, "run"))
            .iter()
            .map(|(_, opcode)| *opcode)
            .collect::<Vec<_>>(),
        [0xbb, 0x59, 0x1b, 0xb7, 0x4d, 0x2c, 0x4e, 0x10, 0xac],
        "the initialized mutable local is loaded once before storing the immutable copy"
    );
}

#[test]
fn initialized_mutable_reference_local_can_be_discarded_as_an_expression() {
    let output = TestOutputDirectory::new("reference-var-discard-accepted");
    let artifacts = compile_planned_constructor_source(
        "class Counter(seed: Int)\nclass Client:\n  def run(seed: Int): Int =\n    var counter = new Counter(seed)\n    counter\n    42\n",
        output.path(),
    )
    .expect("an initialized mutable reference local can be discarded");
    let client = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Client")
        .expect("Client class is emitted");
    let mut reader = Reader::new(client.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("decode Client class");
    assert_eq!(
        decode_subset_instructions(emitted_method_code(&class, "run"))
            .iter()
            .map(|(_, opcode)| *opcode)
            .collect::<Vec<_>>(),
        [0xbb, 0x59, 0x1b, 0xb7, 0x4d, 0x2c, 0x57, 0x10, 0xac],
        "the initialized mutable local is loaded once before the discarded value is popped"
    );
}

#[test]
fn mutable_reference_issue_candidate_calls_dispatch_after_local_cfg_joins() {
    let source = fs::read_to_string(fixture_path("MutableReferenceLocals.scala"))
        .expect("read the real mutable-reference-local source fixture");
    let output = TestOutputDirectory::new("mutable-reference-candidate-dispatch-first");
    let second_output = TestOutputDirectory::new("mutable-reference-candidate-dispatch-second");
    let mut observed_session = false;
    let mut observed_methods = 0;
    let artifacts = compile_source_batch_observed(
        &source,
        "MutableReferenceLocals.scala",
        output.path(),
        SourceId::from_index(1454),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut |unit, definitions| {
            observed_session = true;
            let base = audit_class_named(unit, "Base");
            let left = audit_class_named(unit, "Left");
            let right = audit_class_named(unit, "Right");
            let runner = audit_class_named(unit, "Runner");
            assert_eq!(audit_class_parent(unit, base), definitions.object_class);
            assert_eq!(audit_class_parent(unit, left), base);
            assert_eq!(audit_class_parent(unit, right), base);
            let value_method = audit_method_named(unit, base, "value").method;

            for name in ["choose", "repeat", "initialize", "swap"] {
                observed_methods += 1;
                let method = audit_method_named(unit, runner, name);
                let binding = method
                    .locals
                    .iter()
                    .find(|binding| {
                        binding.mutable
                            && unit
                                .store
                                .names
                                .resolve(unit.store.symbols.get(binding.symbol).name.text())
                                == "current"
                    })
                    .unwrap_or_else(|| panic!("{name} has its canonical mutable current local"));
                let symbol = unit.store.symbols.get(binding.symbol);
                assert_eq!(symbol.kind, SymbolKind::Local);
                assert_eq!(symbol.owner, Some(method.method));
                assert!(symbol.flags.contains(SymbolFlags::MUTABLE));
                let dotty_core::SymbolInfo::Complete(declared_type) = &symbol.info else {
                    panic!("current has complete source type information")
                };
                assert_eq!(audit_type_symbol(unit, *declared_type), Some(base));
                let layout = unit
                    .local_layout(runner, method.method, definitions)
                    .expect("all planned classes and locals share this typed session");
                let slot = layout
                    .local(binding.symbol)
                    .expect("layout uses the canonical local SymbolId");
                assert_eq!(slot.ty(), JvmType::Reference(base));
                assert!(slot.is_mutable());
                let body = method.body.expect("method has a typed body");
                assert!(
                    typed_tree_targets(unit, body).contains(&value_method),
                    "{name} selects Base.value through the same typed symbol graph"
                );
            }
            assert_eq!(observed_methods, 4);
        }),
    )
    .expect("mutable local method calls compile through the JVM backend");
    assert!(
        observed_session,
        "the complete frontend reached one typed session"
    );
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Base", "Left", "Right", "Runner"]
    );
    let second = compile_source_batch(
        &source,
        "MutableReferenceLocals.scala",
        second_output.path(),
        SourceId::from_index(1454),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect("a fresh frontend session independently emits the same source batch");
    assert_eq!(second.len(), artifacts.len());
    assert_eq!(output.class_files().len(), 4);
    assert_eq!(second_output.class_files().len(), 4);
    for (first, second) in artifacts.iter().zip(&second) {
        assert_eq!(first.class.internal_name(), second.class.internal_name());
        assert_eq!(
            first.class.bytes(),
            second.class.bytes(),
            "{} is byte-identical across independent frontend sessions",
            first.class.internal_name()
        );
        assert_eq!(
            fs::read(&first.path).expect("read first published class"),
            first.class.bytes()
        );
        assert_eq!(
            fs::read(&second.path).expect("read second published class"),
            second.class.bytes()
        );

        let mut reader = Reader::new(first.class.bytes());
        let class = ClassFile::decode(&mut reader).expect("decode every emitted class");
        class
            .validate_references()
            .expect("every emitted class has valid constant-pool references");
        assert_eq!(class.version.major, 65, "JVM 21 classfile version");
        assert!(class.access_flags.is_public());
        let expected_super = match first.class.internal_name() {
            "Base" | "Runner" => "java/lang/Object",
            "Left" | "Right" => "Base",
            name => panic!("unexpected class in acceptance fixture: {name}"),
        };
        assert_eq!(
            class
                .constant_pool
                .class_name(class.super_class.expect("source class superclass")),
            Ok(expected_super),
            "{} keeps its direct superclass",
            first.class.internal_name()
        );
    }

    let base_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Base")
        .expect("Base is emitted");
    let mut base_reader = Reader::new(base_artifact.class.bytes());
    let base = ClassFile::decode(&mut base_reader).expect("decode Base");
    assert_eq!(base.fields.len(), 1, "inherited fields are not copied");
    let current = &base.fields[0];
    assert_eq!(base.constant_pool.utf8(current.name_index), Ok("current"));
    assert_eq!(base.constant_pool.utf8(current.descriptor_index), Ok("I"));
    assert!(current.access_flags.is_private());
    assert!(!current.access_flags.is_final(), "Base.current is mutable");
    assert_eq!(
        base.methods
            .iter()
            .filter(|method| base.constant_pool.utf8(method.name_index) == Ok("value"))
            .count(),
        1
    );
    let base_constructor = emitted_method_code(&base, "<init>");
    assert_field_reference(&base, base_constructor, 0xb5, "Base", "current", "I");
    assert_invocation_sequence(
        &base,
        base_constructor,
        &[(0xb7, "java/lang/Object", "<init>", "()V")],
    );
    assert_field_reference(
        &base,
        emitted_method_code(&base, "value"),
        0xb4,
        "Base",
        "current",
        "I",
    );

    for class_name in ["Left", "Right"] {
        let artifact = artifacts
            .iter()
            .find(|artifact| artifact.class.internal_name() == class_name)
            .expect("subclass is emitted");
        let mut reader = Reader::new(artifact.class.bytes());
        let class = ClassFile::decode(&mut reader).expect("decode subclass");
        assert!(
            class.fields.is_empty(),
            "{class_name} does not duplicate Base fields"
        );
        assert_invocation_sequence(
            &class,
            emitted_method_code(&class, "<init>"),
            &[(0xb7, "Base", "<init>", "(I)V")],
        );
        assert_invocation_sequence(
            &class,
            emitted_method_code(&class, "value"),
            &[(0xb7, "Base", "value", "()I")],
        );
    }

    let runner_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Runner")
        .expect("Runner is emitted");
    let mut runner_reader = Reader::new(runner_artifact.class.bytes());
    let runner_class = ClassFile::decode(&mut runner_reader).expect("decode Runner");
    assert!(runner_class.fields.is_empty());
    for method in ["choose", "repeat", "initialize", "swap"] {
        let code = emitted_method_code(&runner_class, method);
        assert!(
            decode_subset_instructions(code)
                .iter()
                .any(|(_, opcode)| *opcode == 0xb6),
            "{method} invokes the canonical Base.value method virtually"
        );
        let expected = match method {
            "choose" | "repeat" | "initialize" => vec![
                (0xb7, "Left", "<init>", "(I)V"),
                (0xb7, "Right", "<init>", "(I)V"),
                (0xb6, "Base", "value", "()I"),
            ],
            "swap" => vec![
                (0xb7, "Left", "<init>", "(I)V"),
                (0xb7, "Right", "<init>", "(I)V"),
                (0xb7, "Left", "<init>", "(I)V"),
                (0xb6, "Base", "value", "()I"),
            ],
            _ => unreachable!(),
        };
        assert_invocation_sequence(&runner_class, code, &expected);
        let stores = decode_subset_instructions(code)
            .iter()
            .filter(|(_, opcode)| *opcode == 0x4e)
            .count();
        let expected_stores = if method == "initialize" { 1 } else { 2 };
        assert_eq!(
            stores, expected_stores,
            "{method} stores current in astore_3"
        );
        assert_eq!(
            decode_subset_instructions(code)
                .iter()
                .filter(|(_, opcode)| *opcode == 0x2d)
                .count(),
            1,
            "{method} reads current from aload_3 for the virtual call"
        );
        assert!(
            decode_subset_instructions(code)
                .iter()
                .all(|(_, opcode)| *opcode != 0xc0),
            "{method} does not insert a narrowing checkcast"
        );
    }
    assert_eq!(code_max_locals(&runner_class, "choose"), 4);
    assert_eq!(code_max_locals(&runner_class, "repeat"), 5);
    assert_eq!(code_max_locals(&runner_class, "initialize"), 4);
    assert_eq!(code_max_locals(&runner_class, "swap"), 4);
    for method in ["choose", "repeat", "initialize", "swap"] {
        assert_eq!(
            code_max_stack(&runner_class, method),
            3,
            "{method} max_stack"
        );
    }
    assert_reference_local_join_frame(&runner_class, "choose", 3, false);
    assert_reference_local_join_frame(&runner_class, "repeat", 3, true);
    assert_reference_stack_join_frame(&runner_class, "initialize");
    assert_reference_stack_join_frame(&runner_class, "swap");

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 output path");
        let harness = output.path().join("MutableReferenceCandidateHarness.java");
        fs::write(
            &harness,
            r#"public final class MutableReferenceCandidateHarness {
  public static void main(String[] args) {
    Runner runner = new Runner();
    for (int seed : new int[] {10, 0, -3}) {
      int left = seed + 1;
      int right = seed + 2;
      if (runner.choose(false, seed) != left || runner.choose(true, seed) != right)
        throw new AssertionError("branch assignment dispatch: " + seed);
      if (runner.repeat(0, seed) != left || runner.repeat(1, seed) != right || runner.repeat(3, seed) != right)
        throw new AssertionError("loop assignment dispatch: " + seed);
      if (runner.initialize(true, seed) != left || runner.initialize(false, seed) != right)
        throw new AssertionError("initializer join dispatch: " + seed);
      if (runner.swap(false, seed) != left || runner.swap(true, seed) != right)
        throw new AssertionError("assignment sibling join dispatch: " + seed);
    }
  }
}
"#,
        )
        .expect("write mutable-reference readback harness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness)
            .output()
            .expect("start javac");
        assert!(
            javac.status.success(),
            "javac rejected mutable-reference readback harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "MutableReferenceCandidateHarness",
            ])
            .output()
            .expect("start java verifier");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected mutable-reference readback classes\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );

        for class_name in ["Base", "Left", "Right", "Runner"] {
            let javap = Command::new("javap")
                .args(["-p", "-c", "-v", "-classpath", output_path, class_name])
                .output()
                .expect("start javap for acceptance class");
            assert!(
                javap.status.success(),
                "javap -c -v rejected {class_name}\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&javap.stdout),
                String::from_utf8_lossy(&javap.stderr)
            );
            let text = String::from_utf8_lossy(&javap.stdout);
            assert!(text.contains("major version: 65"));
            if class_name == "Runner" {
                assert!(text.contains("StackMapTable"));
                assert!(text.contains("invokevirtual"));
                assert!(text.contains("Method Base.value:()I"));
            }
            if class_name == "Base" {
                assert!(text.contains("current;"));
                assert!(text.contains("putfield"));
                assert!(text.contains("getfield"));
            }
        }
    }
}

fn walk_typed_subtree(
    unit: &JvmCompilationUnit<'_>,
    root: TreeId<Typed>,
    visit: &mut impl FnMut(TreeId<Typed>),
) {
    let mut pending = vec![root];
    while let Some(tree_id) = pending.pop() {
        visit(tree_id);
        match &unit.typed_ast.get(tree_id).kind {
            TreeKind::Apply(application) => {
                pending.push(application.function);
                pending.extend(application.args.iter().copied());
            }
            TreeKind::Assign(assignment) => pending.extend([assignment.lhs, assignment.rhs]),
            TreeKind::Block(block) => {
                pending.extend(block.stats.iter().copied());
                pending.push(block.expr);
            }
            TreeKind::If(if_tree) => {
                pending.extend([if_tree.cond, if_tree.then_branch, if_tree.else_branch]);
            }
            TreeKind::New(new_tree) => pending.push(new_tree.tpt),
            TreeKind::Select(select) => pending.push(select.qualifier),
            TreeKind::Typed(typed) => pending.extend([typed.expr, typed.tpt]),
            TreeKind::ValDef(definition) => {
                pending.push(definition.tpt);
                pending.extend(definition.rhs);
            }
            TreeKind::While(loop_tree) => pending.extend([loop_tree.cond, loop_tree.body]),
            _ => {}
        }
    }
}

#[test]
fn mutable_reference_local_audit_preserves_source_evidence_after_layout_acceptance() {
    let source = fs::read_to_string(fixture_path("MutableReferenceLocalsTyped.scala"))
        .expect("read the source-backed mutable-reference-local audit fixture");
    let output = TestOutputDirectory::new("mutable-reference-local-audit");
    let mut observed = false;
    let mut observed_methods = 0;
    let result = compile_source_batch_observed(
        &source,
        "MutableReferenceLocalsTyped.scala",
        output.path(),
        SourceId::from_index(1454),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut |unit, definitions| {
            observed = true;
            let base = audit_class_named(unit, "Base");
            let left = audit_class_named(unit, "Left");
            let right = audit_class_named(unit, "Right");
            let runner = audit_class_named(unit, "Runner");
            assert_eq!(audit_class_parent(unit, left), base);
            assert_eq!(audit_class_parent(unit, right), base);
            assert_eq!(audit_class_parent(unit, base), definitions.object_class);

            for method_name in ["choose", "repeat", "initialize"] {
                observed_methods += 1;
                let method = audit_method_named(unit, runner, method_name);
                let layout = unit
                    .local_layout(runner, method.method, definitions)
                    .expect("the canonical mutable Base local has a valid slot layout");
                let current = method
                    .locals
                    .iter()
                    .find(|binding| {
                        binding.mutable
                            && unit
                                .store
                                .names
                                .resolve(unit.store.symbols.get(binding.symbol).name.text())
                                == "current"
                    })
                    .expect("each real source var has one canonical mutable local binding");
                let symbol = unit.store.symbols.get(current.symbol);
                assert_eq!(symbol.owner, Some(method.method));
                assert!(symbol.flags.contains(SymbolFlags::MUTABLE));
                assert_eq!(symbol.kind, SymbolKind::Local);
                let slot = layout
                    .local(current.symbol)
                    .expect("the layout is keyed by the same canonical local symbol");
                assert_eq!(slot.declaration(), current.declaration);
                assert_eq!(slot.ty(), JvmType::Reference(base));
                assert!(slot.is_mutable());
                let dotty_core::SymbolInfo::Complete(declared_type) = &symbol.info else {
                    panic!("current has complete canonical source type information")
                };
                assert_eq!(
                    audit_type_symbol(unit, *declared_type),
                    Some(base),
                    "the declared local type remains Base"
                );
                let declaration = unit.typed_ast.get(current.declaration);
                let TreeKind::ValDef(definition) = &declaration.kind else {
                    panic!("the binding points at its canonical typed ValDef")
                };
                let initializer = definition.rhs.expect("current has a source initializer");
                assert_eq!(
                    audit_type_symbol(unit, unit.typed_ast.get(definition.tpt).ty),
                    Some(base)
                );
                let body = method.body.expect("each source method has a typed body");
                let mut nodes = Vec::new();
                walk_typed_subtree(unit, body, &mut |tree| nodes.push(tree));
                let assignments_to_current = nodes
                    .iter()
                    .filter_map(|tree_id| {
                        let TreeKind::Assign(assignment) = &unit.typed_ast.get(*tree_id).kind
                        else {
                            return None;
                        };
                        (term_type_target(unit, unit.typed_ast.get(assignment.lhs).ty)
                            == Some(current.symbol))
                        .then_some((*tree_id, assignment.rhs))
                    })
                    .collect::<Vec<_>>();

                match method_name {
                    "choose" => {
                        assert_eq!(
                            audit_type_symbol(unit, unit.typed_ast.get(initializer).ty),
                            Some(left)
                        );
                        let branch = nodes
                            .iter()
                            .find_map(|tree_id| {
                                let tree = unit.typed_ast.get(*tree_id);
                                matches!(tree.kind, TreeKind::If(_)).then_some(tree)
                            })
                            .expect("conditional reassignment remains a typed If");
                        let TreeKind::If(if_tree) = &branch.kind else {
                            unreachable!()
                        };
                        let mut then_nodes = Vec::new();
                        walk_typed_subtree(unit, if_tree.then_branch, &mut |tree| {
                            then_nodes.push(tree)
                        });
                        let then_assignments = then_nodes
                            .iter()
                            .filter_map(|tree_id| {
                                let TreeKind::Assign(assignment) =
                                    &unit.typed_ast.get(*tree_id).kind
                                else {
                                    return None;
                                };
                                (term_type_target(unit, unit.typed_ast.get(assignment.lhs).ty)
                                    == Some(current.symbol))
                                .then_some(assignment.rhs)
                            })
                            .collect::<Vec<_>>();
                        assert_eq!(then_assignments.len(), 1);
                        assert_eq!(
                            audit_type_symbol(unit, unit.typed_ast.get(then_assignments[0]).ty),
                            Some(right),
                            "the assignment in the If then-branch stores Right"
                        );
                        assert_eq!(assignments_to_current.len(), 1);
                    }
                    "repeat" => {
                        assert_eq!(
                            audit_type_symbol(unit, unit.typed_ast.get(initializer).ty),
                            Some(left)
                        );
                        let loop_tree = nodes
                            .iter()
                            .find_map(|tree_id| {
                                let tree = unit.typed_ast.get(*tree_id);
                                matches!(tree.kind, TreeKind::While(_)).then_some(tree)
                            })
                            .expect("loop reassignment remains a typed While");
                        let TreeKind::While(loop_body) = &loop_tree.kind else {
                            unreachable!()
                        };
                        let mut loop_body_nodes = Vec::new();
                        walk_typed_subtree(unit, loop_body.body, &mut |tree| {
                            loop_body_nodes.push(tree)
                        });
                        let loop_assignments = loop_body_nodes
                            .iter()
                            .filter_map(|tree_id| {
                                let TreeKind::Assign(assignment) =
                                    &unit.typed_ast.get(*tree_id).kind
                                else {
                                    return None;
                                };
                                (term_type_target(unit, unit.typed_ast.get(assignment.lhs).ty)
                                    == Some(current.symbol))
                                .then_some(assignment.rhs)
                            })
                            .collect::<Vec<_>>();
                        assert_eq!(loop_assignments.len(), 1);
                        assert_eq!(
                            audit_type_symbol(unit, unit.typed_ast.get(loop_assignments[0]).ty),
                            Some(right),
                            "the While body stores Right into current"
                        );
                        assert_eq!(assignments_to_current.len(), 1);
                    }
                    "initialize" => {
                        let Type::Or {
                            left: left_arm,
                            right: right_arm,
                        } = unit.store.types.get(unit.typed_ast.get(initializer).ty)
                        else {
                            panic!("the contextual initializer retains the source sibling union")
                        };
                        let mut arms = [
                            audit_type_symbol(unit, *left_arm)
                                .expect("left union arm is canonical"),
                            audit_type_symbol(unit, *right_arm)
                                .expect("right union arm is canonical"),
                        ];
                        arms.sort_by_key(|arm| arm.index());
                        let mut expected = [left, right];
                        expected.sort_by_key(|arm| arm.index());
                        assert_eq!(arms, expected);
                        assert!(assignments_to_current.is_empty());
                    }
                    _ => unreachable!(),
                }
            }
            assert_eq!(observed_methods, 3);
        }),
    );
    assert!(
        observed,
        "the complete source reached the typed audit observer: {result:?}"
    );
    let artifacts = result.expect("branch and loop assignments lower after the source audit");
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Base", "Left", "Right", "Runner"]
    );
    assert_eq!(
        output.class_files().len(),
        4,
        "the completed source batch publishes all four planned classes"
    );
}

#[test]
fn mutable_reference_layout_preserves_canonical_types_and_shadowed_symbols() {
    let source = fs::read_to_string(fixture_path("MutableReferenceLocalsTyped.scala"))
        .expect("read the literal-result adapted audit fixture");
    for method_name in ["layout", "shadowed"] {
        let method_source = source_with_only_runner_method(&source, method_name);
        let mut session_layouts = Vec::new();
        for source_index in [1456, 1457] {
            let output = TestOutputDirectory::new(&format!(
                "mutable-reference-layout-{method_name}-{source_index}"
            ));
            let mut observed_layout = None;
            let result = compile_source_batch_observed(
                &method_source,
                "MutableReferenceLocalsTyped.scala",
                output.path(),
                SourceId::from_index(source_index),
                SourceUnitCardinality::AtLeastTwo,
                Some(&mut |unit, definitions| {
                    let runner = audit_class_named(unit, "Runner");
                    let method = audit_method_named(unit, runner, method_name);
                    let base = audit_class_named(unit, "Base");
                    let layout = unit
                        .local_layout(runner, method.method, definitions)
                        .expect("canonical mutable reference locals have a planned slot layout");
                    let locals = method
                        .locals
                        .iter()
                        .map(|binding| {
                            let symbol = unit.store.symbols.get(binding.symbol);
                            assert_eq!(symbol.kind, SymbolKind::Local);
                            assert_eq!(symbol.owner, Some(method.method));
                            assert_eq!(
                                symbol.flags.contains(SymbolFlags::MUTABLE),
                                binding.mutable
                            );
                            let name = unit.store.names.resolve(symbol.name.text()).to_owned();
                            assert_eq!(name, "current");
                            let dotty_core::symbols::SymbolInfo::Complete(declared_type) =
                                unit.store.symbols.info(binding.symbol)
                            else {
                                panic!("the local keeps its completed declared type")
                            };
                            assert_eq!(audit_type_symbol(unit, *declared_type), Some(base));
                            let slot = layout
                                .local(binding.symbol)
                                .expect("layout lookup uses the canonical local symbol");
                            assert_eq!(slot.declaration(), binding.declaration);
                            assert_eq!(slot.is_mutable(), binding.mutable);
                            let JvmType::Reference(slot_class) = slot.ty() else {
                                panic!("the local slot retains its declared reference type")
                            };
                            assert_eq!(slot_class, base);
                            (name, slot.slot(), slot.is_mutable(), "Base".to_owned())
                        })
                        .collect::<Vec<_>>();
                    match method_name {
                        "layout" => {
                            assert_eq!(locals.len(), 1);
                            assert_eq!(locals[0].1, 2, "receiver and seed occupy slots 0 and 1");
                            assert_eq!(layout.max_locals(), 3);
                            assert!(locals[0].2);
                        }
                        "shadowed" => {
                            assert_eq!(locals.len(), 2);
                            assert_ne!(method.locals[0].symbol, method.locals[1].symbol);
                            assert_eq!(locals[0].1, 3);
                            assert_eq!(locals[1].1, 4);
                            assert_eq!(layout.max_locals(), 5);
                            assert_eq!(
                                locals.iter().map(|local| local.2).collect::<Vec<_>>(),
                                [true, false]
                            );
                        }
                        _ => unreachable!(),
                    }
                    observed_layout = Some((layout.max_locals(), locals));
                }),
            );
            assert!(
                result.is_ok(),
                "initializing planned mutable locals without reading or assigning them is supported: {result:?}"
            );
            assert!(!output.class_files().is_empty());
            session_layouts
                .push(observed_layout.expect("the source-backed typed unit was observed"));
        }
        assert_eq!(session_layouts[0], session_layouts[1]);
    }
}

#[test]
fn constructor_arguments_lower_strict_and_unary_boolean_intrinsics_from_source() {
    let output = TestOutputDirectory::new("boolean-constructor-arguments");
    let artifacts = compile_planned_constructor_source(
        "class Counter(seed: Boolean)\nclass Client:\n  def buildAnd(flag: Boolean): Int =\n    new Counter(flag & true)\n    42\n  def buildOr(flag: Boolean): Int =\n    new Counter(flag | true)\n    42\n  def buildXor(flag: Boolean): Int =\n    new Counter(flag ^ true)\n    42\n  def buildNot(flag: Boolean): Int =\n    new Counter(!flag)\n    42\n",
        output.path(),
    )
    .unwrap();
    let client = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Client")
        .expect("Client source class is emitted");
    let mut reader = Reader::new(client.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("decode Client class");
    class
        .validate_references()
        .expect("valid Client references");

    for (method, operator) in [("buildAnd", 0x7e), ("buildOr", 0x80), ("buildXor", 0x82)] {
        let code = emitted_method_code(&class, method);
        assert!(
            code.windows(3)
                .any(|sequence| sequence == [0x1b, 0x04, operator]),
            "{method} should lower its strict Boolean operator before construction: {code:?}"
        );
        assert!(
            code.contains(&0xb7),
            "{method} should invoke the constructor"
        );
    }
    let not_code = emitted_method_code(&class, "buildNot");
    assert!(
        not_code
            .windows(3)
            .any(|sequence| sequence == [0x1b, 0x04, 0x82]),
        "buildNot should lower canonical ! as iconst_1; ixor: {not_code:?}"
    );
    assert!(
        not_code.contains(&0xb7),
        "buildNot should invoke the constructor"
    );
}

#[test]
fn field_initializer_emits_a_complete_verified_class() {
    let output = TestOutputDirectory::new("field-initializer");
    let artifact = compile_fixture("FieldInitializer.scala", output.path()).unwrap();
    let mut reader = Reader::new(artifact.class.bytes());
    let class_file = ClassFile::decode(&mut reader).unwrap();
    class_file.validate_references().unwrap();
    assert_eq!(class_file.fields.len(), 1);
    assert_eq!(class_file.methods.len(), 2);
    assert_eq!(
        class_file
            .constant_pool
            .utf8(class_file.methods[0].name_index)
            .unwrap(),
        "<init>"
    );
    assert_eq!(
        class_file
            .constant_pool
            .utf8(class_file.methods[0].descriptor_index)
            .unwrap(),
        "()V"
    );

    if !require_jdk_or_skip() {
        return;
    }
    let harness_path = output.path().join("FieldInitializerHarness.java");
    fs::write(
        &harness_path,
        "public final class FieldInitializerHarness { public static void main(String[] args) { FieldInitializer instance = new FieldInitializer(); if (instance.value() != 1) throw new AssertionError(\"value\"); } }\n",
    )
    .unwrap();
    let javac = Command::new("javac")
        .args(["-classpath", output.path().to_str().unwrap(), "-d"])
        .arg(output.path())
        .arg(&harness_path)
        .output()
        .unwrap();
    assert!(javac.status.success(), "javac failed: {:?}", javac);
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            output.path().to_str().unwrap(),
            "FieldInitializerHarness",
        ])
        .output()
        .unwrap();
    assert!(java.status.success(), "java failed: {:?}", java);
}

#[test]
fn real_counter_source_compiles_deterministically_and_executes_stateful_methods() {
    let first_output = TestOutputDirectory::new("counter-first");
    let second_output = TestOutputDirectory::new("counter-second");
    let first = compile_fixture("Counter.scala", first_output.path()).unwrap();
    let second = compile_fixture("Counter.scala", second_output.path()).unwrap();

    assert_eq!(first.class.internal_name(), "Counter");
    assert_eq!(first.class.bytes(), second.class.bytes());
    assert_eq!(first_output.class_files(), vec![first.path.clone()]);
    assert_eq!(second_output.class_files(), vec![second.path.clone()]);

    let mut reader = Reader::new(first.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("Counter should decode completely");
    assert!(
        reader.is_at_end(),
        "decoder consumes the complete Counter file"
    );
    class
        .validate_references()
        .expect("Counter constant-pool references should be valid");
    assert_eq!(class.version.major, JVM_CLASSFILE_MAJOR_VERSION);
    assert_eq!(
        class.constant_pool.class_name(class.this_class),
        Ok("Counter")
    );
    assert_eq!(class.fields.len(), 1);
    let field = &class.fields[0];
    assert_eq!(class.constant_pool.utf8(field.name_index), Ok("current"));
    assert_eq!(class.constant_pool.utf8(field.descriptor_index), Ok("I"));
    assert_eq!(
        field.access_flags.0,
        dotty_classfile::access_flags::ACC_PRIVATE
    );
    assert_eq!(
        class
            .methods
            .iter()
            .map(|method| (
                class.constant_pool.utf8(method.name_index).unwrap(),
                class.constant_pool.utf8(method.descriptor_index).unwrap()
            ))
            .collect::<Vec<_>>(),
        [
            ("<init>", "(I)V"),
            ("get", "()I"),
            ("set", "(I)V"),
            ("increment", "()I"),
        ]
    );
    assert!(
        class
            .methods
            .iter()
            .all(|method| method.access_flags.is_public())
    );
    assert_eq!(
        class
            .methods
            .iter()
            .filter(|method| class.constant_pool.utf8(method.name_index) == Ok("<init>"))
            .count(),
        1,
        "the parameterized class must not acquire a phantom no-arg constructor"
    );

    let constructor = emitted_method_code(&class, "<init>");
    assert_opcode_sequence(constructor, &[0x2a, 0xb7, 0x2a, 0x1b, 0xb5, 0xb1]);
    assert_method_reference(
        &class,
        constructor,
        0xb7,
        "java/lang/Object",
        "<init>",
        "()V",
    );
    assert_field_reference(&class, constructor, 0xb5, "Counter", "current", "I");
    assert_opcode_sequence(emitted_method_code(&class, "get"), &[0x2a, 0xb4, 0xac]);
    assert_field_reference(
        &class,
        emitted_method_code(&class, "get"),
        0xb4,
        "Counter",
        "current",
        "I",
    );
    assert_opcode_sequence(
        emitted_method_code(&class, "set"),
        &[0x2a, 0x4d, 0x1b, 0x3e, 0x2c, 0x1d, 0xb5, 0xb1],
    );
    assert_opcode_sequence(
        emitted_method_code(&class, "increment"),
        &[
            0x2a, 0x4c, 0x2a, 0xb4, 0x04, 0x60, 0x3d, 0x2b, 0x1c, 0xb5, 0x2a, 0xb4, 0xac,
        ],
    );

    if require_jdk_or_skip() {
        verify_counter_with_jdk(first_output.path());
    }
}

#[test]
fn real_private_fields_source_emits_private_final_storage_and_boolean_getter() {
    let first_output = TestOutputDirectory::new("private-fields-first");
    let second_output = TestOutputDirectory::new("private-fields-second");
    let first = compile_fixture("PrivateFields.scala", first_output.path()).unwrap();
    let second = compile_fixture("PrivateFields.scala", second_output.path()).unwrap();

    assert_eq!(first.class.internal_name(), "PrivateFields");
    assert_eq!(first.class.bytes(), second.class.bytes());
    assert_eq!(first_output.class_files(), vec![first.path.clone()]);
    assert_eq!(second_output.class_files(), vec![second.path.clone()]);

    let mut reader = Reader::new(first.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("PrivateFields should decode completely");
    assert!(
        reader.is_at_end(),
        "decoder consumes the complete PrivateFields file"
    );
    class
        .validate_references()
        .expect("PrivateFields constant-pool references should be valid");
    assert_eq!(class.version.major, JVM_CLASSFILE_MAJOR_VERSION);
    let fields = class
        .fields
        .iter()
        .map(|field| {
            (
                class.constant_pool.utf8(field.name_index).unwrap(),
                class.constant_pool.utf8(field.descriptor_index).unwrap(),
                field.access_flags,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(fields.len(), 3);
    assert_eq!((fields[0].0, fields[0].1), ("seed", "I"));
    assert_eq!(
        fields[0].2.0,
        dotty_classfile::access_flags::ACC_PRIVATE | dotty_classfile::access_flags::ACC_FINAL
    );
    assert_eq!((fields[1].0, fields[1].1), ("enabled", "Z"));
    assert_eq!(
        fields[1].2.0,
        dotty_classfile::access_flags::ACC_PRIVATE | dotty_classfile::access_flags::ACC_FINAL
    );
    assert_eq!((fields[2].0, fields[2].1), ("disabled", "Z"));
    assert_eq!(
        fields[2].2.0,
        dotty_classfile::access_flags::ACC_PRIVATE | dotty_classfile::access_flags::ACC_FINAL
    );
    assert_eq!(
        class
            .methods
            .iter()
            .map(|method| (
                class.constant_pool.utf8(method.name_index).unwrap(),
                class.constant_pool.utf8(method.descriptor_index).unwrap()
            ))
            .collect::<Vec<_>>(),
        [
            ("<init>", "()V"),
            ("seedValue", "()I"),
            ("enabledValue", "()Z"),
            ("disabledValue", "()Z"),
        ]
    );
    assert_eq!(emitted_method_code(&class, "seedValue")[1], 0xb4);
    assert_field_reference(
        &class,
        emitted_method_code(&class, "seedValue"),
        0xb4,
        "PrivateFields",
        "seed",
        "I",
    );
    assert_field_reference(
        &class,
        emitted_method_code(&class, "enabledValue"),
        0xb4,
        "PrivateFields",
        "enabled",
        "Z",
    );
    assert_field_reference(
        &class,
        emitted_method_code(&class, "disabledValue"),
        0xb4,
        "PrivateFields",
        "disabled",
        "Z",
    );

    if require_jdk_or_skip() {
        verify_private_fields_with_jdk(first_output.path());
    }
}

#[test]
fn unsupported_real_field_and_constructor_sources_fail_without_artifacts() {
    assert_source_failure(
        "wrong-field-initializer",
        "class WrongInitializer { private val seed: Int = true }",
        |failure| matches!(failure, PipelineFailure::Typer(_)),
    );
    assert_source_failure(
        "immutable-field-assignment",
        "class ImmutableAssignment { private val seed: Int = 1; def update(): Unit = seed = 2 }",
        |failure| matches!(failure, PipelineFailure::Typer(_)),
    );
    assert_source_failure(
        "implicit-noarg-forward-field-read",
        "class ForwardRead { val first: Int = later; val later: Int = 2 }",
        |failure| {
            matches!(
                failure,
                PipelineFailure::Codegen(
                    JvmCodegenError::FieldInitializerReadsUninitializedField { .. }
                )
            )
        },
    );
    assert_source_failure(
        "implicit-noarg-this-escape",
        "class ThisEscape { val next: ThisEscape = this }",
        |failure| {
            matches!(
                failure,
                PipelineFailure::Codegen(
                    JvmCodegenError::UnsupportedJvmReferenceType { .. }
                        | JvmCodegenError::UnsupportedConstructorFieldInitializer { .. }
                )
            )
        },
    );
    let private_noarg_output = TestOutputDirectory::new("private-explicit-noarg-constructor");
    let private_noarg = compile_source(
        "class Hidden private () { val value: Int = 1 }",
        "PrivateNoArg.scala",
        private_noarg_output.path(),
    )
    .expect("an explicit private no-arg constructor keeps its declared access");
    let mut private_noarg_reader = Reader::new(private_noarg.class.bytes());
    let private_noarg_class = ClassFile::decode(&mut private_noarg_reader)
        .expect("decode class with explicit private constructor");
    let constructors = private_noarg_class
        .methods
        .iter()
        .filter(|method| private_noarg_class.constant_pool.utf8(method.name_index) == Ok("<init>"))
        .collect::<Vec<_>>();
    assert_eq!(
        constructors.len(),
        1,
        "no duplicate public no-arg constructor"
    );
    assert_eq!(
        private_noarg_class
            .constant_pool
            .utf8(constructors[0].descriptor_index),
        Ok("()V")
    );
    assert!(constructors[0].access_flags.is_private());
    for (label, source) in [
        (
            "defaulted-constructor-parameter",
            "class WithDefault(value: Int = 1) { private val current: Int = value }",
        ),
        (
            "contextual-constructor-parameter",
            "class WithUsing(using context: Int) { private val current: Int = context }",
        ),
        (
            "secondary-constructor",
            "class WithSecondary(value: Int) { private val current: Int = value; def this() = this(0) }",
        ),
    ] {
        assert_source_failure(label, source, |failure| {
            matches!(
                failure,
                PipelineFailure::Codegen(
                    JvmCodegenError::UnsupportedConstructor { .. }
                        | JvmCodegenError::InvalidPrimaryConstructor { .. }
                )
            )
        });
    }
}

#[test]
fn discarded_double_values_fail_without_publishing_artifacts() {
    assert_source_failure(
        "discarded-double-value",
        "class DoubleDiscard { def twice(value: Double): Double = { value; value } }",
        |failure| {
            matches!(
                failure,
                PipelineFailure::Codegen(JvmCodegenError::UnsupportedTypedExpression {
                    node: "discarded category-two value requires category-two stack operation",
                    ..
                })
            )
        },
    );
}

#[test]
fn unsupported_float_double_source_cases_fail_at_their_first_stage_without_batch_artifacts() {
    enum ExpectedFailure {
        MethodBodyTypeMismatch,
        UninitializedLocal,
        MemberLookup,
    }

    let cases = [
        (
            "float-to-int",
            "class Rejected { def narrow(value: Float): Int = value }",
            ExpectedFailure::MethodBodyTypeMismatch,
        ),
        (
            "float-to-double",
            "class Rejected { def widen(value: Float): Double = value }",
            ExpectedFailure::MethodBodyTypeMismatch,
        ),
        (
            "double-to-long",
            "class Rejected { def narrow(value: Double): Long = value }",
            ExpectedFailure::MethodBodyTypeMismatch,
        ),
        (
            "double-literal-to-float-return",
            "class Rejected { def wrongTarget(): Float = 3.5 }",
            ExpectedFailure::MethodBodyTypeMismatch,
        ),
        (
            "uninitialized-double-local",
            "class Rejected:\n  def missing(): Double =\n    var result: Double\n    result\n",
            ExpectedFailure::UninitializedLocal,
        ),
        (
            "float-addition",
            "class Rejected { def add(left: Float, right: Float): Float = left + right }",
            ExpectedFailure::MemberLookup,
        ),
        (
            "float-comparison",
            "class Rejected { def less(left: Float, right: Float): Boolean = left < right }",
            ExpectedFailure::MemberLookup,
        ),
        (
            "double-addition",
            "class Rejected { def add(left: Double, right: Double): Double = left + right }",
            ExpectedFailure::MemberLookup,
        ),
        (
            "double-comparison",
            "class Rejected { def less(left: Double, right: Double): Boolean = left < right }",
            ExpectedFailure::MemberLookup,
        ),
    ];

    for (label, invalid_class, expected_failure) in cases {
        let source =
            format!("class ValidBeforeFailure {{ def value(): Int = 1 }}\n{invalid_class}\n");
        assert_source_batch_failure(label, &source, |failure| match expected_failure {
            ExpectedFailure::MethodBodyTypeMismatch => matches!(
                failure,
                PipelineFailure::Codegen(JvmCodegenError::MethodBodyTypeMismatch { .. })
            ),
            ExpectedFailure::UninitializedLocal => matches!(
                failure,
                PipelineFailure::Typer(TyperError::LocalValueRightHandSideMissing { .. })
            ),
            ExpectedFailure::MemberLookup => {
                matches!(failure, PipelineFailure::Typer(TyperError::MemberLookup(_)))
            }
        });
    }
}

fn assert_method_reference(
    class: &ClassFile<'_>,
    code: &[u8],
    opcode: u8,
    owner: &str,
    name: &str,
    descriptor: &str,
) {
    let offset = decode_subset_instructions(code)
        .into_iter()
        .find_map(|(offset, found)| (found == opcode).then_some(offset))
        .expect("expected method invocation instruction");
    let index = u16::from_be_bytes([code[offset + 1], code[offset + 2]]);
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = class
        .constant_pool
        .get(ConstantPoolIndex(index))
        .expect("method invocation constant-pool entry")
    else {
        panic!("method invocation must reference Methodref");
    };
    assert_eq!(class.constant_pool.class_name(*class_index), Ok(owner));
    assert_eq!(
        class.constant_pool.name_and_type(*name_and_type_index),
        Ok((name, descriptor))
    );
}

fn decode_emitted_class(artifact: &EmittedArtifact) -> ClassFile<'_> {
    let mut reader = Reader::new(artifact.class.bytes());
    let class = ClassFile::decode(&mut reader).expect("decode complete generated classfile");
    assert!(
        reader.is_at_end(),
        "class decoder consumes the complete classfile"
    );
    class
        .validate_references()
        .expect("validate constant-pool references");
    assert_eq!(class.version.major, 65);
    assert_eq!(
        class.constant_pool.class_name(class.this_class),
        Ok(artifact.class.internal_name())
    );
    class
}

fn decode_emitted_class_named<'a>(artifacts: &'a [EmittedArtifact], name: &str) -> ClassFile<'a> {
    decode_emitted_class(
        artifacts
            .iter()
            .find(|artifact| artifact.class.internal_name() == name)
            .unwrap_or_else(|| panic!("missing emitted class {name}")),
    )
}

fn methods_with_descriptors(class: &ClassFile<'_>) -> Vec<(String, String)> {
    class
        .methods
        .iter()
        .map(|method| {
            (
                class
                    .constant_pool
                    .utf8(method.name_index)
                    .unwrap()
                    .to_owned(),
                class
                    .constant_pool
                    .utf8(method.descriptor_index)
                    .unwrap()
                    .to_owned(),
            )
        })
        .collect()
}

fn assert_invocation_sequence(
    class: &ClassFile<'_>,
    code: &[u8],
    expected: &[(u8, &str, &str, &str)],
) {
    let actual = decode_subset_instructions(code)
        .into_iter()
        .filter(|(_, opcode)| matches!(opcode, 0xb6 | 0xb7))
        .map(|(offset, opcode)| {
            let index = u16::from_be_bytes([code[offset + 1], code[offset + 2]]);
            let ConstantPoolEntry::Methodref {
                class_index,
                name_and_type_index,
            } = class
                .constant_pool
                .get(ConstantPoolIndex(index))
                .expect("invocation references a constant-pool entry")
            else {
                panic!("invokevirtual and invokespecial must reference Methodref");
            };
            let owner = class.constant_pool.class_name(*class_index).unwrap();
            let (name, descriptor) = class
                .constant_pool
                .name_and_type(*name_and_type_index)
                .unwrap();
            (opcode, owner, name, descriptor)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        actual, expected,
        "unexpected ordered method references in code"
    );
}

fn assert_field_reference(
    class: &ClassFile<'_>,
    code: &[u8],
    opcode: u8,
    owner: &str,
    name: &str,
    descriptor: &str,
) {
    let offset = decode_subset_instructions(code)
        .into_iter()
        .find_map(|(offset, found)| (found == opcode).then_some(offset))
        .expect("expected field access instruction");
    let index = u16::from_be_bytes([code[offset + 1], code[offset + 2]]);
    let ConstantPoolEntry::Fieldref {
        class_index,
        name_and_type_index,
    } = class
        .constant_pool
        .get(ConstantPoolIndex(index))
        .expect("field access constant-pool entry")
    else {
        panic!("field access must reference Fieldref");
    };
    assert_eq!(class.constant_pool.class_name(*class_index), Ok(owner));
    assert_eq!(
        class.constant_pool.name_and_type(*name_and_type_index),
        Ok((name, descriptor))
    );
}

fn assert_field_reference_sequence(
    class: &ClassFile<'_>,
    code: &[u8],
    expected: &[(u8, &str, &str, &str)],
) {
    let actual = decode_subset_instructions(code)
        .into_iter()
        .filter_map(|(offset, opcode)| {
            if !matches!(opcode, 0xb4 | 0xb5) {
                return None;
            }
            let index = u16::from_be_bytes([code[offset + 1], code[offset + 2]]);
            let ConstantPoolEntry::Fieldref {
                class_index,
                name_and_type_index,
            } = class.constant_pool.get(ConstantPoolIndex(index))?
            else {
                panic!("field access must reference Fieldref");
            };
            let (name, descriptor) = class
                .constant_pool
                .name_and_type(*name_and_type_index)
                .unwrap();
            Some((
                opcode,
                class.constant_pool.class_name(*class_index).unwrap(),
                name,
                descriptor,
            ))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        actual, expected,
        "unexpected ordered field references in code"
    );
}

fn assert_no_field_reference_to_owner(class: &ClassFile<'_>, code: &[u8], owner: &str) {
    for (offset, opcode) in decode_subset_instructions(code) {
        if !matches!(opcode, 0xb4 | 0xb5) {
            continue;
        }
        let index = u16::from_be_bytes([code[offset + 1], code[offset + 2]]);
        let ConstantPoolEntry::Fieldref { class_index, .. } = class
            .constant_pool
            .get(ConstantPoolIndex(index))
            .expect("field access references a constant-pool entry")
        else {
            panic!("field access must reference Fieldref");
        };
        assert_ne!(
            class.constant_pool.class_name(*class_index),
            Ok(owner),
            "external accessor call must not access private backing storage"
        );
    }
}

fn assert_javap_instruction_sequence(output: &str, method_header: &str, expected: &[&str]) {
    let method_start = output
        .find(method_header)
        .unwrap_or_else(|| panic!("javap output omitted method {method_header:?}:\n{output}"));
    let method = &output[method_start..];
    let method_end = [
        method[1..].find("\n  public "),
        method[1..].find("\n  private "),
        method[1..].find("\n}"),
    ]
    .into_iter()
    .flatten()
    .min()
    .map_or(method.len(), |offset| offset + 1);
    let method = &method[..method_end];
    let code = method
        .split_once("Code:")
        .map(|(_, code)| code)
        .unwrap_or_else(|| panic!("javap method {method_header:?} has no Code section"));
    let instructions = code
        .lines()
        .filter_map(|line| {
            let (_, instruction) = line.split_once(": ")?;
            instruction.split_whitespace().next()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        instructions, expected,
        "unexpected javap instructions for {method_header}:\n{method}"
    );
}

#[test]
fn javap_instruction_extraction_stops_at_the_enclosing_class_boundary() {
    let output = r#"
  public int doubled();
    descriptor: ()I
    Code:
       0: aload_0
       1: invokevirtual #7                  // Method value:()I
       4: ireturn
}
Classfile /tmp/Derived.class
  major version: 65
  flags: (0x0021) ACC_PUBLIC, ACC_SUPER
"#;
    assert_javap_instruction_sequence(
        output,
        "public int doubled();",
        &["aload_0", "invokevirtual", "ireturn"],
    );
}

fn verify_counter_with_jdk(output_directory: &Path) {
    let output_path = output_directory.to_str().expect("UTF-8 temporary path");
    let javap = Command::new("javap")
        .args(["-p", "-c", "-v", "-classpath", output_path, "Counter"])
        .output()
        .expect("start javap for Counter");
    assert!(
        javap.status.success(),
        "javap rejected Counter: {:?}",
        javap
    );
    let javap_output = String::from_utf8_lossy(&javap.stdout);
    for expected in [
        "major version: 65",
        "private int current;",
        "descriptor: (I)V",
        "descriptor: ()I",
        "descriptor: (I)V",
        "getfield",
        "putfield",
        "iadd",
    ] {
        assert!(
            javap_output.contains(expected),
            "javap output omitted {expected:?}:\n{javap_output}"
        );
    }
    assert!(javap_output.contains("public Counter(int)"));
    assert!(!javap_output.contains("public Counter();"));
    assert_javap_instruction_sequence(
        &javap_output,
        "public Counter(int);",
        &[
            "aload_0",
            "invokespecial",
            "aload_0",
            "iload_1",
            "putfield",
            "return",
        ],
    );
    assert_javap_instruction_sequence(
        &javap_output,
        "public int get();",
        &["aload_0", "getfield", "ireturn"],
    );
    assert_javap_instruction_sequence(
        &javap_output,
        "public void set(int);",
        &[
            "aload_0", "astore_2", "iload_1", "istore_3", "aload_2", "iload_3", "putfield",
            "return",
        ],
    );
    assert_javap_instruction_sequence(
        &javap_output,
        "public int increment();",
        &[
            "aload_0", "astore_1", "aload_0", "getfield", "iconst_1", "iadd", "istore_2",
            "aload_1", "iload_2", "putfield", "aload_0", "getfield", "ireturn",
        ],
    );

    let harness_path = output_directory.join("CounterHarness.java");
    fs::write(
        &harness_path,
        "public final class CounterHarness { public static void main(String[] args) { Counter a = new Counter(10); if (a.get() != 10) throw new AssertionError(\"initial\"); if (a.increment() != 11) throw new AssertionError(\"increment\"); a.set(41); if (a.get() != 41) throw new AssertionError(\"set\"); if (a.increment() != 42) throw new AssertionError(\"increment after set\"); Counter b = new Counter(3); if (b.get() != 3) throw new AssertionError(\"second instance\"); if (a.get() != 42) throw new AssertionError(\"instance state shared\"); } }\n",
    )
    .expect("write Counter Java harness");
    let javac = Command::new("javac")
        .args(["-classpath", output_path, "-d", output_path])
        .arg(&harness_path)
        .output()
        .expect("start javac for Counter");
    assert!(
        javac.status.success(),
        "javac failed for Counter: {:?}",
        javac
    );
    let java = Command::new("java")
        .args(["-Xverify:all", "-classpath", output_path, "CounterHarness"])
        .output()
        .expect("start java verifier for Counter");
    assert!(java.status.success(), "java failed for Counter: {:?}", java);
}

fn verify_reference_object_apis_with_jdk(output_directory: &Path) {
    let output_path = output_directory.to_str().expect("UTF-8 class output path");
    let counter = Command::new("javap")
        .args(["-c", "-v", "-p", "-classpath", output_path, "Counter"])
        .output()
        .expect("start javap for Counter API");
    assert!(
        counter.status.success(),
        "javap rejected Counter\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&counter.stdout),
        String::from_utf8_lossy(&counter.stderr)
    );
    let counter = String::from_utf8_lossy(&counter.stdout);
    assert!(counter.contains("descriptor: ()LCounter;"));
    assert_javap_instruction_sequence(&counter, "public Counter self();", &["aload_0", "areturn"]);
    for class_name in ["Counter", "CounterService"] {
        let javap = Command::new("javap")
            .args(["-c", "-v", "-p", "-classpath", output_path, class_name])
            .output()
            .expect("start javap for real-source reference APIs");
        assert!(
            javap.status.success(),
            "javap rejected {class_name}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javap.stdout),
            String::from_utf8_lossy(&javap.stderr)
        );
        let output = String::from_utf8_lossy(&javap.stdout);
        assert!(
            output.contains("major version: 65"),
            "{class_name}: {output}"
        );
    }
    let service_javap = Command::new("javap")
        .args([
            "-c",
            "-v",
            "-p",
            "-classpath",
            output_path,
            "CounterService",
        ])
        .output()
        .expect("start javap to inspect CounterService frames");
    assert!(
        service_javap.status.success(),
        "javap rejected CounterService\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&service_javap.stdout),
        String::from_utf8_lossy(&service_javap.stderr)
    );
    assert!(
        String::from_utf8_lossy(&service_javap.stdout).contains("StackMapTable"),
        "branching CounterService methods must have StackMapTable frames"
    );
    let service = String::from_utf8_lossy(&service_javap.stdout);
    for expected in [
        "descriptor: (I)LCounter;",
        "descriptor: (LCounter;)LCounter;",
        "descriptor: (LCounter;)I",
        "descriptor: (ZLCounter;LCounter;)LCounter;",
        "descriptor: (LCounter;I)I",
        "// Method Counter.get:()I",
        "// Method Counter.increment:()I",
        "create:(I)LCounter;",
        "read:(LCounter;)I",
        "new           #",
        "dup",
        "Counter.\"<init>\":(I)V",
    ] {
        assert!(
            service.contains(expected),
            "javap output omitted {expected:?}:\n{service}"
        );
    }
    assert_javap_instruction_sequence(
        &service,
        "public Counter create(int);",
        &["new", "dup", "iload_1", "invokespecial", "areturn"],
    );
    assert_javap_instruction_sequence(
        &service,
        "public Counter passthrough(Counter);",
        &["aload_1", "areturn"],
    );
    assert_javap_instruction_sequence(
        &service,
        "public int read(Counter);",
        &["aload_1", "invokevirtual", "ireturn"],
    );
    assert_javap_instruction_sequence(
        &service,
        "public int roundTrip(int);",
        &[
            "aload_0",
            "aload_0",
            "iload_1",
            "invokevirtual",
            "invokevirtual",
            "ireturn",
        ],
    );

    let harness = output_directory.join("ReferenceObjectApisHarness.java");
    fs::write(
        &harness,
        "public final class ReferenceObjectApisHarness {\n  public static void main(String[] args) {\n    CounterService service = new CounterService();\n    Counter created = service.create(5);\n    if (created.get() != 5) throw new AssertionError(\"create\");\n    if (service.passthrough(created) != created) throw new AssertionError(\"identity\");\n    if (service.read(created) != created.get()) throw new AssertionError(\"read\");\n    if (created.self() != created) throw new AssertionError(\"this identity\");\n    Counter other = service.create(8);\n    if (service.choose(true, created, other) != created) throw new AssertionError(\"true branch identity\");\n    if (service.choose(false, created, other) != other) throw new AssertionError(\"false branch identity\");\n    if (service.incrementUntil(service.create(0), 0) != 0) throw new AssertionError(\"zero loop\");\n    if (service.incrementUntil(service.create(0), 1) != 1) throw new AssertionError(\"one loop\");\n    if (service.incrementUntil(service.create(0), 10) != 10) throw new AssertionError(\"many loop\");\n    if (service.incrementUntil(service.create(12), 10) != 12) throw new AssertionError(\"start above limit\");\n    if (service.roundTrip(13) != 13) throw new AssertionError(\"nested reference result argument\");\n    System.out.println(\"reference object APIs verified\");\n  }\n}\n",
    )
    .expect("write Java reference API harness");
    let javac = Command::new("javac")
        .args(["-classpath", output_path, "-d", output_path])
        .arg(&harness)
        .output()
        .expect("start javac for reference API harness");
    assert!(
        javac.status.success(),
        "javac rejected reference API harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            output_path,
            "ReferenceObjectApisHarness",
        ])
        .output()
        .expect("start JVM verifier for reference APIs");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected reference APIs\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "reference object APIs verified"
    );
}

fn verify_reference_holder_with_jdk(output_directory: &Path) {
    let output_path = output_directory.to_str().expect("UTF-8 class output path");
    for class_name in ["Counter", "Holder", "HolderClient"] {
        let javap = Command::new("javap")
            .args(["-c", "-v", "-p", "-classpath", output_path, class_name])
            .output()
            .expect("start javap for reference-holder class");
        assert!(
            javap.status.success(),
            "javap rejected {class_name}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javap.stdout),
            String::from_utf8_lossy(&javap.stderr)
        );
        let output = String::from_utf8_lossy(&javap.stdout);
        assert!(
            output.contains("major version: 65"),
            "{class_name}: {output}"
        );
        assert!(output.contains("descriptor:"), "{class_name}: {output}");
        if class_name == "Holder" {
            assert!(output.contains("StackMapTable"), "{output}");
            for descriptor in [
                "(LCounter;Z)V",
                "()LCounter;",
                "(LCounter;)V",
                "()I",
                "()Z",
                "(ZLCounter;)LCounter;",
                "(I)I",
            ] {
                assert!(
                    output.contains(&format!("descriptor: {descriptor}")),
                    "{output}"
                );
            }
        }
    }

    let harness_path = output_directory.join("ReferenceHolderHarness.java");
    fs::write(
        &harness_path,
        r#"public final class ReferenceHolderHarness {
    public static void main(String[] args) {
        Counter original = new Counter(2);
        Counter replacement = new Counter(7);
        Holder holder = new Holder(original, true);
        if (holder.value() != original || holder.initial() != original)
            throw new AssertionError("initial identity");
        if (holder.isEnabled() != true || holder.read() != 2)
            throw new AssertionError("initial values");
        if (holder.choose(true, replacement) != original
                || holder.choose(false, original) != original)
            throw new AssertionError("branch join identity");

        holder.replace(replacement);
        if (holder.value() != replacement || holder.initial() != original || holder.read() != 7)
            throw new AssertionError("replacement identity");
        if (holder.incrementUntil(10) != 10 || replacement.get() != 10 || original.get() != 2)
            throw new AssertionError("replacement loop mutation or object isolation");

        HolderClient client = new HolderClient();
        if (client.wrap(original).value() != original)
            throw new AssertionError("wrap identity");
        if (client.run(0, 0) != 0 || client.run(0, 1) != 1
                || client.run(0, 10) != 10 || client.run(12, 5) != 12)
            throw new AssertionError("client loop boundary cases");
    }
}
"#,
    )
    .expect("write ReferenceHolder Java harness");
    let javac = Command::new("javac")
        .args(["-classpath", output_path, "-d", output_path])
        .arg(&harness_path)
        .output()
        .expect("start javac for reference-holder harness");
    assert!(
        javac.status.success(),
        "javac failed for reference holder: {:?}",
        javac
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            output_path,
            "ReferenceHolderHarness",
        ])
        .output()
        .expect("start java verifier for reference-holder harness");
    assert!(
        java.status.success(),
        "java failed for reference holder: {:?}",
        java
    );
}

fn verify_reference_control_flow_with_jdk(output_directory: &Path) {
    let output_path = output_directory.to_str().expect("UTF-8 output path");
    let harness = output_directory.join("ReferenceControlFlowHarness.java");
    fs::write(
        &harness,
        "public final class ReferenceControlFlowHarness { public static void main(String[] args) { Counter counter = new Counter(0); if (counter.incrementUntil(3) != 3 || counter.incrementUntil(0) != 3) throw new AssertionError(\"field loop\"); if (counter.setEnabled(false) != 0 || counter.enabledUntil(8) != 3 || counter.setEnabled(true) != 3 || counter.enabledUntil(5) != 5) throw new AssertionError(\"Boolean field flow\"); CounterClient client = new CounterClient(); if (client.run(0) != 0 || client.run(1) != 1 || client.run(5) != 5) throw new AssertionError(\"reference local loop\"); if (client.choose(true) != 1 || client.choose(false) != 2 || client.chooseLocal(true) != 5 || client.chooseLocal(false) != 6) throw new AssertionError(\"branch allocations and exact join\"); if (client.ifIncrement(true) != 1 || client.ifIncrement(false) != 1) throw new AssertionError(\"branch calls\"); if (client.runAnd(false, 5) != 0 || client.runAnd(true, 4) != 4) throw new AssertionError(\"short-circuit reference loop\"); System.out.println(\"reference control flow ok\"); } }\n",
    )
    .expect("write reference control-flow harness");
    let javac = Command::new("javac")
        .args(["-classpath", output_path, "-d", output_path])
        .arg(&harness)
        .output()
        .expect("start javac for reference control-flow harness");
    assert!(
        javac.status.success(),
        "javac rejected reference control-flow harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            output_path,
            "ReferenceControlFlowHarness",
        ])
        .output()
        .expect("start JVM verifier for reference control-flow classes");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected reference control-flow classes\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "reference control flow ok"
    );
}

fn verify_counter_control_flow_with_jdk(output_directory: &Path) {
    let output_path = output_directory.to_str().expect("UTF-8 output path");
    let javap = Command::new("javap")
        .args([
            "-c",
            "-v",
            "-p",
            "-classpath",
            output_path,
            "Counter",
            "CounterClient",
        ])
        .output()
        .expect("start javap for Counter control-flow fixture");
    assert!(
        javap.status.success(),
        "javap rejected Counter control-flow classes\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javap.stdout),
        String::from_utf8_lossy(&javap.stderr)
    );
    let output = String::from_utf8_lossy(&javap.stdout);
    for evidence in [
        "major version: 65",
        "private int current",
        "aload_0",
        "getfield",
        "putfield",
        "iload_1",
        "istore_",
        "if_icmp",
        "ifeq",
        "goto",
        "aload_2",
        "invokevirtual",
        "StackMapTable",
        "Counter.\"<init>\":(I)V",
        "Counter.get:()I",
        "Counter.increment:()I",
    ] {
        assert!(
            output.contains(evidence),
            "javap output omitted {evidence:?}:\n{output}"
        );
    }

    let harness = output_directory.join("CounterControlFlowHarness.java");
    fs::write(
        &harness,
        "public final class CounterControlFlowHarness { public static void main(String[] args) { CounterClient client = new CounterClient(); if (client.run(0) != 0) throw new AssertionError(\"run zero\"); if (client.run(1) != 1) throw new AssertionError(\"run one\"); if (client.run(10) != 10) throw new AssertionError(\"run multiple\"); if (client.choose(true) != 10) throw new AssertionError(\"choose true\"); if (client.choose(false) != 20) throw new AssertionError(\"choose false\"); if (new Counter(2).incrementUntil(5) != 5) throw new AssertionError(\"increment to limit\"); if (new Counter(5).incrementUntil(5) != 5) throw new AssertionError(\"already at limit\"); if (new Counter(6).incrementUntil(3) != 6) throw new AssertionError(\"start over limit\"); System.out.println(\"counter control flow verified\"); } }\n",
    )
    .expect("write Counter control-flow Java harness");
    let javac = Command::new("javac")
        .args(["-classpath", output_path, "-d", output_path])
        .arg(&harness)
        .output()
        .expect("start javac for Counter control-flow harness");
    assert!(
        javac.status.success(),
        "javac rejected Counter control-flow harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            output_path,
            "CounterControlFlowHarness",
        ])
        .output()
        .expect("start JVM verifier for Counter control-flow classes");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected Counter control-flow classes\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "counter control flow verified"
    );
}

fn verify_counter_objects_with_jdk(output_directory: &Path) {
    let output_path = output_directory.to_str().expect("UTF-8 output path");
    let javap = Command::new("javap")
        .args([
            "-p",
            "-c",
            "-v",
            "-classpath",
            output_path,
            "Counter",
            "CounterClient",
        ])
        .output()
        .expect("start javap for both CounterObjects classes");
    assert!(
        javap.status.success(),
        "javap rejected CounterObjects outputs\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javap.stdout),
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap_output = String::from_utf8_lossy(&javap.stdout);
    for expected in [
        "major version: 65",
        "private int current;",
        "public Counter(int);",
        "public int increment();",
        "public int get();",
        "public CounterClient();",
        "public int run(int);",
        "public int direct();",
        "public int independent();",
        "descriptor: (I)V",
        "descriptor: (I)I",
    ] {
        assert!(
            javap_output.contains(expected),
            "javap output omitted {expected:?}:\n{javap_output}"
        );
    }
    assert_javap_instruction_sequence(
        &javap_output,
        "public int run(int);",
        &[
            "new",
            "dup",
            "iload_1",
            "invokespecial",
            "astore_2",
            "aload_2",
            "invokevirtual",
            "pop",
            "aload_2",
            "invokevirtual",
            "ireturn",
        ],
    );
    assert_javap_instruction_sequence(
        &javap_output,
        "public int direct();",
        &[
            "new",
            "dup",
            "bipush",
            "invokespecial",
            "invokevirtual",
            "ireturn",
        ],
    );
    assert_javap_instruction_sequence(
        &javap_output,
        "public int independent();",
        &[
            "new",
            "dup",
            "bipush",
            "invokespecial",
            "astore_1",
            "new",
            "dup",
            "bipush",
            "invokespecial",
            "astore_2",
            "aload_1",
            "invokevirtual",
            "pop",
            "aload_1",
            "invokevirtual",
            "aload_2",
            "invokevirtual",
            "iadd",
            "ireturn",
        ],
    );

    let harness_path = output_directory.join("CounterObjectsHarness.java");
    fs::write(
        &harness_path,
        "public final class CounterObjectsHarness { public static void main(String[] args) { CounterClient client = new CounterClient(); if (client.run(10) != 11) throw new AssertionError(\"run(10)\"); if (client.run(0) != 1) throw new AssertionError(\"run(0)\"); if (client.direct() != 42) throw new AssertionError(\"direct\"); if (client.independent() != 31) throw new AssertionError(\"independent instances\"); System.out.println(\"counter objects ok\"); } }\n",
    )
    .expect("write CounterObjects Java harness");
    let javac = Command::new("javac")
        .args(["-classpath", output_path, "-d", output_path])
        .arg(&harness_path)
        .output()
        .expect("start javac for CounterObjects");
    assert!(
        javac.status.success(),
        "javac rejected CounterObjects harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            output_path,
            "CounterObjectsHarness",
        ])
        .output()
        .expect("start java verifier for CounterObjects");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected CounterObjects\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "counter objects ok"
    );
}

fn verify_private_fields_with_jdk(output_directory: &Path) {
    let output_path = output_directory.to_str().expect("UTF-8 temporary path");
    let javap = Command::new("javap")
        .args(["-p", "-c", "-v", "-classpath", output_path, "PrivateFields"])
        .output()
        .expect("start javap for PrivateFields");
    assert!(
        javap.status.success(),
        "javap rejected PrivateFields: {:?}",
        javap
    );
    let javap_output = String::from_utf8_lossy(&javap.stdout);
    for expected in [
        "major version: 65",
        "private final int seed;",
        "private final boolean enabled;",
        "private final boolean disabled;",
        "descriptor: ()V",
        "descriptor: ()I",
        "descriptor: ()Z",
        "getfield",
    ] {
        assert!(
            javap_output.contains(expected),
            "javap output omitted {expected:?}:\n{javap_output}"
        );
    }

    let harness_path = output_directory.join("PrivateFieldsHarness.java");
    fs::write(
        &harness_path,
        "public final class PrivateFieldsHarness { public static void main(String[] args) { PrivateFields value = new PrivateFields(); if (value.seedValue() != 42) throw new AssertionError(\"seed\"); if (!value.enabledValue()) throw new AssertionError(\"boolean true\"); if (value.disabledValue()) throw new AssertionError(\"boolean false\"); } }\n",
    )
    .expect("write PrivateFields Java harness");
    let javac = Command::new("javac")
        .args(["-classpath", output_path, "-d", output_path])
        .arg(&harness_path)
        .output()
        .expect("start javac for PrivateFields");
    assert!(
        javac.status.success(),
        "javac failed for PrivateFields: {:?}",
        javac
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            output_path,
            "PrivateFieldsHarness",
        ])
        .output()
        .expect("start java verifier for PrivateFields");
    assert!(
        java.status.success(),
        "java failed for PrivateFields: {:?}",
        java
    );
}

#[test]
fn public_body_accessors_initialize_and_emit_verified_getters_and_setters() {
    let source_text = "class BodyOnly:\n  val seed: Int = 10\n  var enabled: Boolean = true\n  val ratio: Double = 1.5\n  var score: Float = -2.25f\n  var rate: Double = 1.5\n\nclass Counter(val id: Int):\n  val initial: Int = 10\n  var current: Long = 0L\n  private val copied: Int = initial\n\nclass Holder(value: Counter):\n  var selected: Counter = value";
    let output = TestOutputDirectory::new("public-body-field-initialization");
    let mut observed = false;
    let mut observe = |ast: &AstArena<Untyped>,
                       index: &SourceSemanticIndex,
                       unit: &mut JvmCompilationUnit<'_>,
                       definitions: &Definitions| {
        observed = true;
        unit.validate()
            .expect("the source-proven public body field plans validate");
        let metadata = unit
            .metadata(definitions)
            .expect("the public fields project to checked private backing metadata");

        for (
            class_name,
            expected_body_fields,
            expected_source_fields,
            expected_private_fields,
            expected_constructor_accessors,
        ) in [
            (
                "BodyOnly",
                &["seed", "enabled", "ratio", "score", "rate"][..],
                &["seed", "enabled", "ratio", "score", "rate"][..],
                0,
                0,
            ),
            (
                "Counter",
                &["initial", "current"][..],
                &["initial", "current", "copied"][..],
                1,
                1,
            ),
            ("Holder", &["selected"][..], &["selected"][..], 0, 0),
        ] {
            let class = audit_class_named(unit, class_name);
            let plan = unit
                .classes
                .iter()
                .find(|plan| plan.class == class)
                .expect("source class has a plan");
            if class_name == "BodyOnly" {
                assert!(matches!(
                    plan.initialization,
                    JvmClassInitializationPlan::ImplicitNoArg
                ));
            } else {
                assert!(matches!(
                    plan.initialization,
                    JvmClassInitializationPlan::PrimaryConstructor(_)
                ));
            }
            assert_eq!(plan.fields.len(), expected_source_fields.len());
            assert_eq!(
                plan.fields
                    .iter()
                    .map(|field| unit
                        .store
                        .names
                        .resolve(unit.store.symbols.get(field.field).name.text())
                        .to_owned())
                    .collect::<Vec<_>>(),
                expected_source_fields
            );
            let body_field_plans = plan
                .fields
                .iter()
                .filter(|field| {
                    matches!(
                        field.source_visibility,
                        JvmFieldSourceVisibility::PlainPublic(_)
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                body_field_plans
                    .iter()
                    .map(|field| {
                        let field = *field;
                        let JvmFieldSourceVisibility::PlainPublic(proof) = field.source_visibility
                        else {
                            panic!("ordinary public body field carries its checked source proof");
                        };
                        assert_eq!(proof.class(), class);
                        assert_eq!(proof.field(), field.field);
                        let symbol = unit.store.symbols.get(field.field);
                        assert_eq!(symbol.kind, SymbolKind::Field);
                        assert_eq!(symbol.owner, Some(class));
                        assert_eq!(symbol.visibility, Visibility::Public);
                        let dotty_core::SymbolInfo::Complete(_field_type) = symbol.info else {
                            panic!("public body field type should be complete");
                        };
                        assert_eq!(proof.mutable(), symbol.flags.contains(SymbolFlags::MUTABLE));
                        let initializer = field.initializer.expect("source field has an RHS");
                        let source_definition = index
                            .definition_of(field.field)
                            .expect("body field has canonical source provenance");
                        let SourceDefinition::Canonical { source, tree } = source_definition else {
                            panic!("class-body field must keep its canonical source tree");
                        };
                        assert_eq!(
                            source,
                            SourceId::from_index(1479),
                            "canonical source identity belongs to this source unit"
                        );
                        let TreeKind::ValDef(definition) = &ast.get(tree).kind else {
                            panic!("field provenance points to its source ValDef");
                        };
                        assert_eq!(
                            proof.mutable(),
                            definition
                                .metadata
                                .modifiers
                                .contains(&dotty_core::ast::Modifier::Var)
                        );
                        assert!(definition.metadata.visibility.is_none());
                        let rhs = definition.rhs.expect("source field has an RHS");
                        assert_eq!(proof.source(), source);
                        assert_eq!(proof.declaration(), tree);
                        assert_eq!(
                            proof.template_position(),
                            expected_body_fields
                                .iter()
                                .position(
                                    |name| *name == unit.store.names.resolve(symbol.name.text())
                                )
                                .unwrap()
                        );
                        assert_eq!(proof.rhs(), rhs);
                        assert_eq!(proof.initializer(), initializer);
                        unit.store.names.resolve(symbol.name.text()).to_owned()
                    })
                    .collect::<Vec<_>>(),
                expected_body_fields
            );

            let class_metadata = metadata
                .iter()
                .find(|metadata| metadata.identity().symbol() == class)
                .expect("class has validated metadata");
            assert_eq!(class_metadata.fields().len(), expected_private_fields);
            assert_eq!(
                class_metadata.accessors().len(),
                expected_body_fields.len() + expected_constructor_accessors
            );
            if expected_constructor_accessors == 1 {
                assert!(class_metadata.accessors().iter().any(|accessor| {
                    matches!(
                        accessor.origin(),
                        dotty_jvm::JvmAccessorOrigin::ConstructorParameter(_)
                    )
                }));
            }
            for field_plan in &body_field_plans {
                let field_plan = *field_plan;
                let accessor = class_metadata
                    .accessors()
                    .iter()
                    .find(|accessor| accessor.source_field() == field_plan.field)
                    .expect("each source body field has an accessor projection");
                let expected_descriptor = match unit
                    .store
                    .names
                    .resolve(unit.store.symbols.get(field_plan.field).name.text())
                {
                    "seed" => "I",
                    "enabled" => "Z",
                    "ratio" => "D",
                    "score" => "F",
                    "rate" => "D",
                    "initial" => "I",
                    "current" => "J",
                    "selected" => "LCounter;",
                    _ => panic!("source field has a covered primitive type"),
                };
                assert_eq!(accessor.field_descriptor(), expected_descriptor);
                assert!(matches!(
                    accessor.origin(),
                    dotty_jvm::JvmAccessorOrigin::ClassBodyField(proof)
                        if proof.initializer() == field_plan.initializer.unwrap()
                ));
                assert!(
                    accessor
                        .field_access_flags()
                        .contains(dotty_classfile::access_flags::ACC_PRIVATE)
                );
                assert_eq!(
                    accessor
                        .field_access_flags()
                        .contains(dotty_classfile::access_flags::ACC_FINAL),
                    !unit
                        .store
                        .symbols
                        .get(field_plan.field)
                        .flags
                        .contains(SymbolFlags::MUTABLE)
                );
                assert_eq!(
                    accessor.getter().name(),
                    unit.store
                        .names
                        .resolve(unit.store.symbols.get(field_plan.field).name.text())
                );
                assert_eq!(
                    accessor.getter().descriptor().as_str(),
                    &format!("(){expected_descriptor}")
                );
                assert_eq!(
                    accessor.setter().is_some(),
                    unit.store
                        .symbols
                        .get(field_plan.field)
                        .flags
                        .contains(SymbolFlags::MUTABLE)
                );
                if let Some(setter) = accessor.setter() {
                    assert_eq!(setter.name(), &format!("{}_$eq", accessor.field_name()));
                    assert_eq!(
                        setter.descriptor().as_str(),
                        &format!("({expected_descriptor})V")
                    );
                    assert!(setter.access_flags().is_public());
                }
            }
            let initializers = unit
                .lower_constructor_field_initializers(class, definitions)
                .expect("public body fields lower through their checked typed RHS proofs");
            assert_eq!(initializers.fields().len(), expected_source_fields.len());
            assert_eq!(
                initializers
                    .fields()
                    .iter()
                    .map(|initializer| initializer.field())
                    .collect::<Vec<_>>(),
                plan.fields
                    .iter()
                    .map(|field| field.field)
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                initializers
                    .fields()
                    .iter()
                    .map(|initializer| {
                        let origin = match plan
                            .fields
                            .iter()
                            .find(|field| field.field == initializer.field())
                            .expect("initializer has its source field plan")
                            .source_visibility
                        {
                            JvmFieldSourceVisibility::PlainPublic(_) => {
                                dotty_jvm::JvmConstructorFieldInitializerOrigin::ClassBodyAccessor
                            }
                            JvmFieldSourceVisibility::PlainPrivate => {
                                dotty_jvm::JvmConstructorFieldInitializerOrigin::PrivateField
                            }
                            JvmFieldSourceVisibility::PrivateThis => {
                                panic!("private[this] fields are outside initialization scope")
                            }
                        };
                        assert_eq!(initializer.origin(), origin);
                        unit.store
                            .names
                            .resolve(unit.store.symbols.get(initializer.field()).name.text())
                    })
                    .collect::<Vec<_>>(),
                expected_source_fields
            );

            if expected_body_fields.len() > 1 {
                let class_index = unit
                    .classes
                    .iter()
                    .position(|candidate| candidate.class == class)
                    .expect("source class has a mutable plan");
                unit.classes[class_index].fields.swap(0, 1);
                assert!(matches!(
                    unit.validate(),
                    Err(JvmCodegenError::OutOfOrderPublicBodyFieldPlan {
                        class: found_class,
                        ..
                    }) if found_class == class
                ));
                unit.classes[class_index].fields.swap(0, 1);
                unit.validate()
                    .expect("restoring source order restores the validated plan");
            }

            if class_name == "Counter" {
                let class_index = unit
                    .classes
                    .iter()
                    .position(|candidate| candidate.class == class)
                    .expect("Counter has a mutable class plan");
                let private_index = unit.classes[class_index]
                    .fields
                    .iter()
                    .position(|field| {
                        matches!(
                            field.source_visibility,
                            JvmFieldSourceVisibility::PlainPrivate
                        )
                    })
                    .expect("Counter includes a private body field");
                unit.classes[class_index].fields.swap(0, private_index);
                assert!(matches!(
                    unit.validate(),
                    Err(JvmCodegenError::OutOfOrderFieldPlan {
                        class: found_class,
                        ..
                    }) if found_class == class
                ));
                unit.classes[class_index].fields.swap(0, private_index);
                unit.validate()
                    .expect("restoring mixed visibility field order restores the plan");
            }

            if class_name == "BodyOnly" {
                let current_plan = unit
                    .classes
                    .iter()
                    .find(|candidate| candidate.class == class)
                    .expect("source class has a plan after order validation");
                let first_field = current_plan.fields[0];
                let second_field = current_plan.fields[1];
                let JvmFieldSourceVisibility::PlainPublic(first_proof) =
                    first_field.source_visibility
                else {
                    panic!("the first public field carries checked source evidence");
                };
                let JvmFieldSourceVisibility::PlainPublic(second_proof) =
                    second_field.source_visibility
                else {
                    panic!("the second public field carries checked source evidence");
                };
                let mut forged_index = dotty_core::SourceTypedIndex::new();
                forged_index
                    .insert(
                        first_proof.source(),
                        first_proof.rhs(),
                        second_proof.initializer(),
                    )
                    .expect("the fresh source map can hold the fabricated binding");
                assert!(matches!(
                    JvmPublicBodyFieldSourceProof::from_source(
                        class,
                        first_field.field,
                        first_proof.source(),
                        first_proof.declaration(),
                        second_proof.initializer(),
                        ast,
                        index,
                        &forged_index,
                        unit.typed_ast,
                        unit.store,
                    ),
                    Err(JvmCodegenError::InvalidPublicBodyFieldSourceProof { field })
                        if field == first_field.field
                ));
                assert!(matches!(
                    JvmPublicBodyFieldSourceProof::from_source(
                        class,
                        first_field.field,
                        first_proof.source(),
                        first_proof.declaration(),
                        first_proof.initializer(),
                        ast,
                        index,
                        &dotty_core::SourceTypedIndex::new(),
                        unit.typed_ast,
                        unit.store,
                    ),
                    Err(JvmCodegenError::InvalidPublicBodyFieldSourceProof { field })
                        if field == first_field.field
                ));
            }
        }
    };

    let first_artifacts = compile_source_batch_with_source_observer(
        source_text,
        "BodyFields.scala",
        output.path(),
        SourceId::from_index(1479),
        SourceUnitCardinality::AtLeastTwo,
        &mut observe,
    )
    .expect("public class-body backing fields initialize in both constructor shapes");
    assert!(
        observed,
        "plan validation runs before the emission boundary"
    );
    assert_eq!(output.class_files().len(), 3);
    let second_output = TestOutputDirectory::new("public-body-field-initialization-second");
    let second_artifacts = compile_source_batch(
        source_text,
        "BodyFields.scala",
        second_output.path(),
        SourceId::from_index(1479),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect("a fresh frontend session emits the same body-field classes");
    assert_eq!(first_artifacts.len(), 3);
    assert_eq!(first_artifacts.len(), second_artifacts.len());
    assert!(
        first_artifacts
            .iter()
            .zip(&second_artifacts)
            .all(|(first, second)| {
                first.class.internal_name() == second.class.internal_name()
                    && first.class.bytes() == second.class.bytes()
            })
    );

    let output_path = output.path().to_str().expect("UTF-8 output path");
    let body_only_bytes =
        fs::read(output.path().join("BodyOnly.class")).expect("read emitted BodyOnly class file");
    let mut body_only_reader = Reader::new(&body_only_bytes);
    let body_only = ClassFile::decode(&mut body_only_reader).expect("decode BodyOnly class file");
    assert!(body_only_reader.is_at_end());
    body_only
        .validate_references()
        .expect("valid BodyOnly references");
    assert_eq!(body_only.version.major, JVM_CLASSFILE_MAJOR_VERSION);
    assert_field_reference_sequence(
        &body_only,
        emitted_method_code(&body_only, "<init>"),
        &[
            (0xb5, "BodyOnly", "seed", "I"),
            (0xb5, "BodyOnly", "enabled", "Z"),
            (0xb5, "BodyOnly", "ratio", "D"),
            (0xb5, "BodyOnly", "score", "F"),
            (0xb5, "BodyOnly", "rate", "D"),
        ],
    );
    let body_only_constructor = emitted_method_code(&body_only, "<init>");
    assert_eq!(
        decode_subset_instructions(body_only_constructor)[..2],
        [(0, 0x2a), (1, 0xb7)]
    );
    assert_method_reference(
        &body_only,
        body_only_constructor,
        0xb7,
        "java/lang/Object",
        "<init>",
        "()V",
    );
    let counter_bytes =
        fs::read(output.path().join("Counter.class")).expect("read emitted Counter class file");
    let mut counter_reader = Reader::new(&counter_bytes);
    let counter = ClassFile::decode(&mut counter_reader).expect("decode Counter class file");
    assert!(counter_reader.is_at_end());
    counter
        .validate_references()
        .expect("valid Counter references");
    assert_eq!(counter.version.major, JVM_CLASSFILE_MAJOR_VERSION);
    assert_field_reference_sequence(
        &counter,
        emitted_method_code(&counter, "<init>"),
        &[
            (0xb5, "Counter", "id", "I"),
            (0xb5, "Counter", "initial", "I"),
            (0xb5, "Counter", "current", "J"),
            (0xb4, "Counter", "initial", "I"),
            (0xb5, "Counter", "copied", "I"),
        ],
    );
    let holder_bytes =
        fs::read(output.path().join("Holder.class")).expect("read emitted Holder class file");
    let mut holder_reader = Reader::new(&holder_bytes);
    let holder = ClassFile::decode(&mut holder_reader).expect("decode Holder class file");
    assert!(holder_reader.is_at_end());
    holder
        .validate_references()
        .expect("valid Holder references");
    assert_eq!(holder.version.major, JVM_CLASSFILE_MAJOR_VERSION);
    let counter_constructor = emitted_method_code(&counter, "<init>");
    assert_eq!(
        decode_subset_instructions(counter_constructor)[..2],
        [(0, 0x2a), (1, 0xb7)]
    );
    assert_method_reference(
        &counter,
        counter_constructor,
        0xb7,
        "java/lang/Object",
        "<init>",
        "()V",
    );
    assert_eq!(
        body_only
            .methods
            .iter()
            .filter_map(|method| body_only.constant_pool.utf8(method.name_index).ok())
            .collect::<Vec<_>>(),
        [
            "<init>",
            "seed",
            "enabled",
            "enabled_$eq",
            "ratio",
            "score",
            "score_$eq",
            "rate",
            "rate_$eq"
        ]
    );
    let body_only_constructor = body_only
        .methods
        .iter()
        .find(|method| body_only.constant_pool.utf8(method.name_index) == Ok("<init>"))
        .expect("BodyOnly has one generated constructor");
    assert!(body_only_constructor.access_flags.is_public());
    assert_eq!(
        body_only
            .constant_pool
            .utf8(body_only_constructor.descriptor_index),
        Ok("()V")
    );
    let counter_method_names = counter
        .methods
        .iter()
        .filter_map(|method| counter.constant_pool.utf8(method.name_index).ok())
        .collect::<Vec<_>>();
    assert_eq!(
        counter_method_names,
        ["<init>", "id", "initial", "current", "current_$eq"]
    );
    let holder_method_names = holder
        .methods
        .iter()
        .filter_map(|method| holder.constant_pool.utf8(method.name_index).ok())
        .collect::<Vec<_>>();
    assert_eq!(holder_method_names, ["<init>", "selected", "selected_$eq"]);

    for (field_name, descriptor, is_final) in [
        ("seed", "I", true),
        ("enabled", "Z", false),
        ("ratio", "D", true),
        ("score", "F", false),
        ("rate", "D", false),
    ] {
        let fields = body_only
            .fields
            .iter()
            .filter(|field| {
                body_only.constant_pool.utf8(field.name_index) == Ok(field_name)
                    && body_only.constant_pool.utf8(field.descriptor_index) == Ok(descriptor)
            })
            .collect::<Vec<_>>();
        let [field] = fields.as_slice() else {
            panic!("expected exactly one {field_name}:{descriptor} backing field");
        };
        assert!(field.access_flags.is_private());
        assert_eq!(field.access_flags.is_final(), is_final);
    }
    let counter_constructors = counter
        .methods
        .iter()
        .filter(|method| counter.constant_pool.utf8(method.name_index) == Ok("<init>"))
        .collect::<Vec<_>>();
    assert_eq!(counter_constructors.len(), 1);
    assert!(counter_constructors[0].access_flags.is_public());
    assert_eq!(
        counter
            .constant_pool
            .utf8(counter_constructors[0].descriptor_index),
        Ok("(I)V")
    );
    for (field_name, descriptor, is_final) in [("initial", "I", true), ("current", "J", false)] {
        let fields = counter
            .fields
            .iter()
            .filter(|field| {
                counter.constant_pool.utf8(field.name_index) == Ok(field_name)
                    && counter.constant_pool.utf8(field.descriptor_index) == Ok(descriptor)
            })
            .collect::<Vec<_>>();
        let [field] = fields.as_slice() else {
            panic!("expected exactly one {field_name}:{descriptor} backing field");
        };
        assert!(field.access_flags.is_private());
        assert_eq!(field.access_flags.is_final(), is_final);
    }
    let selected_fields = holder
        .fields
        .iter()
        .filter(|field| {
            holder.constant_pool.utf8(field.name_index) == Ok("selected")
                && holder.constant_pool.utf8(field.descriptor_index) == Ok("LCounter;")
        })
        .collect::<Vec<_>>();
    let [selected_field] = selected_fields.as_slice() else {
        panic!("Holder has exactly one private Counter backing field");
    };
    assert!(selected_field.access_flags.is_private());
    assert!(!selected_field.access_flags.is_final());

    for (class, getter, descriptor, field_name, field_descriptor, return_opcode, stack) in [
        (&body_only, "seed", "()I", "seed", "I", 0xac, 1),
        (&body_only, "enabled", "()Z", "enabled", "Z", 0xac, 1),
        (&body_only, "ratio", "()D", "ratio", "D", 0xaf, 2),
        (&body_only, "score", "()F", "score", "F", 0xae, 1),
        (&counter, "initial", "()I", "initial", "I", 0xac, 1),
        (&counter, "current", "()J", "current", "J", 0xad, 2),
        (
            &holder,
            "selected",
            "()LCounter;",
            "selected",
            "LCounter;",
            0xb0,
            1,
        ),
    ] {
        assert_emitted_getter(
            class,
            getter,
            descriptor,
            field_name,
            field_descriptor,
            return_opcode,
            stack,
        );
    }

    for (class, setter, descriptor, field_name, field_descriptor, load, stack, locals) in [
        (
            &body_only,
            "enabled_$eq",
            "(Z)V",
            "enabled",
            "Z",
            0x1b,
            2,
            2,
        ),
        (&body_only, "score_$eq", "(F)V", "score", "F", 0x23, 2, 2),
        (&body_only, "rate_$eq", "(D)V", "rate", "D", 0x27, 3, 3),
        (&counter, "current_$eq", "(J)V", "current", "J", 0x1f, 3, 3),
        (
            &holder,
            "selected_$eq",
            "(LCounter;)V",
            "selected",
            "LCounter;",
            0x2b,
            2,
            2,
        ),
    ] {
        assert_emitted_setter(
            class,
            setter,
            descriptor,
            (field_name, field_descriptor),
            load,
            (stack, locals),
        );
    }

    if require_jdk_or_skip() {
        let harness_path = output.path().join("BodyFieldsHarness.java");
        fs::write(
            &harness_path,
            "public final class BodyFieldsHarness {\n  public static void main(String[] args) {\n    BodyOnly body = new BodyOnly(); BodyOnly otherBody = new BodyOnly();\n    if (body.seed() != 10 || !body.enabled()) throw new AssertionError(\"BodyOnly getters\");\n    if (Double.doubleToRawLongBits(body.ratio()) != Double.doubleToRawLongBits(1.5d)) throw new AssertionError(\"ratio bits\");\n    body.enabled_$eq(false); body.score_$eq(-0.0f); body.rate_$eq(-0.0d);\n    if (body.enabled() || Float.floatToRawIntBits(body.score()) != Float.floatToRawIntBits(-0.0f) || Double.doubleToRawLongBits(body.rate()) != Double.doubleToRawLongBits(-0.0d)) throw new AssertionError(\"body setters\");\n    if (!otherBody.enabled() || Float.floatToRawIntBits(otherBody.score()) != Float.floatToRawIntBits(-2.25f) || Double.doubleToRawLongBits(otherBody.rate()) != Double.doubleToRawLongBits(1.5d)) throw new AssertionError(\"body instance isolation\");\n    Counter first = new Counter(7); Counter second = new Counter(13);\n    if (first.id() != 7 || first.initial() != 10 || first.current() != 0L) throw new AssertionError(\"first counter getters\");\n    first.current_$eq(Long.MIN_VALUE); if (first.current() != Long.MIN_VALUE) throw new AssertionError(\"Long.MIN_VALUE setter\");\n    first.current_$eq(Long.MAX_VALUE); if (first.current() != Long.MAX_VALUE) throw new AssertionError(\"Long.MAX_VALUE setter\");\n    if (second.id() != 13 || second.initial() != 10 || second.current() != 0L) throw new AssertionError(\"counter instance isolation\");\n    Holder holder = new Holder(first); if (holder.selected() != first) throw new AssertionError(\"reference getter\");\n    holder.selected_$eq(second); if (holder.selected() != second) throw new AssertionError(\"reference setter\");\n  }\n}\n",
        )
        .expect("write body-field harness");
        let javac = Command::new("javac")
            .args(["-classpath", output_path, "-d", output_path])
            .arg(&harness_path)
            .output()
            .expect("start javac for body-field harness");
        assert!(javac.status.success(), "javac failed: {javac:?}");
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                output_path,
                "BodyFieldsHarness",
            ])
            .output()
            .expect("start java verifier for body-field harness");
        assert!(java.status.success(), "java failed: {java:?}");
    }
}

#[test]
fn mixed_visibility_plan_permutation_cannot_hide_a_forward_field_read() {
    let source_text = "class Mixed:\n  val first: Int = later\n  private val later: Int = 2";
    let output = TestOutputDirectory::new("mixed-field-order-permutation");
    let mut observed = false;
    let mut observe = |unit: &mut JvmCompilationUnit<'_>, _definitions: &Definitions| {
        let class = audit_class_named(unit, "Mixed");
        let class_plan = unit
            .classes
            .iter_mut()
            .find(|plan| plan.class == class)
            .expect("Mixed has a class plan");
        assert_eq!(class_plan.fields.len(), 2);
        class_plan.fields.swap(0, 1);
        observed = true;
    };
    let result = compile_source_batch_observed(
        source_text,
        "Mixed.scala",
        output.path(),
        SourceId::from_index(1481),
        SourceUnitCardinality::ExactlyOne,
        Some(&mut observe),
    );
    assert!(
        observed,
        "the test permutes the validated frontend field plan"
    );
    assert!(matches!(
        result,
        Err(PipelineFailure::Codegen(
            JvmCodegenError::OutOfOrderFieldPlan { .. }
        ))
    ));
    assert!(
        output.class_files().is_empty(),
        "an out-of-source-order mixed field plan must publish no class file"
    );
}

#[test]
fn private_primary_constructor_is_rejected_without_a_public_constructor_file() {
    let output = TestOutputDirectory::new("private-constructor");
    let error = compile_fixture("PrivateConstructor.scala", output.path()).unwrap_err();
    assert!(matches!(
        error,
        PipelineFailure::Codegen(JvmCodegenError::UnsupportedConstructor { .. })
    ));
    assert!(output.class_files().is_empty());
}

#[test]
fn invalid_source_is_rejected_with_parser_diagnostics_without_output() {
    let output = TestOutputDirectory::new("invalid-source");
    let error = compile_fixture("Invalid.scala", output.path()).unwrap_err();
    assert!(matches!(error, PipelineFailure::Parser(diagnostics) if !diagnostics.is_empty()));
    assert!(output.class_files().is_empty());
}

#[test]
fn package_qualified_source_parent_reaches_hierarchy_and_constructor_metadata() {
    let source_text = "package example\nclass Base()\nclass Derived() extends example.Base";
    let source_id = SourceId::from_index(1327);
    let source = SourceText::new(source_text).unwrap();
    let scanner = ContextualScanner::new(source_text).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "PackageQualifiedParent.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        other => panic!("source should be named without diagnostics: {other:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("source root should be a package definition")
    };
    let source_classes = package
        .stats
        .iter()
        .filter_map(|tree| {
            let TreeKind::TypeDef(definition) = &parsed.ast.get(*tree).kind else {
                return None;
            };
            let class = index.symbol_at(source_id, *tree)?;
            if store.symbols.get(class).kind != SymbolKind::Class {
                return None;
            }
            let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
                panic!("source class should have a template")
            };
            Some((class, template.constructor))
        })
        .collect::<Vec<_>>();
    assert_eq!(source_classes.len(), 2);

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for (class, _) in &source_classes {
        typer.complete_symbol(*class).unwrap();
    }
    let mut plans = Vec::new();
    for (class, constructor_tree) in source_classes {
        let constructor = index
            .symbol_at(source_id, constructor_tree)
            .expect("constructor should have a canonical source symbol");
        typer.complete_symbol(constructor).unwrap();
        plans.push(JvmClassPlan {
            class,
            methods: Vec::new(),
            fields: Vec::new(),
            initialization: JvmClassInitializationPlan::PrimaryConstructor(
                JvmPrimaryConstructorPlan::from_source(
                    class,
                    constructor,
                    Vec::new(),
                    &parsed.ast,
                    &index,
                )
                .unwrap(),
            ),
        });
    }
    let unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: plans,
    };
    let metadata = unit.metadata(&definitions).unwrap();
    assert_eq!(metadata.len(), 2);
    assert_eq!(
        metadata[1].superclass().unwrap().parent_internal_name(),
        "example/Base"
    );
}

#[test]
fn source_overrides_keep_declared_methods_separate_from_effective_dispatch() {
    let output = TestOutputDirectory::new("inherited-method-dispatch");
    let source = "class Base { def value(): Int = 10; def doubled(): Int = value() * 2 }\nclass Derived extends Base { override def value(): Int = 21 }\n";
    let mut observed_metadata = None;
    let mut observe = |unit: &mut JvmCompilationUnit<'_>, definitions: &Definitions| {
        observed_metadata = Some(
            unit.metadata(definitions)
                .expect("validated override metadata"),
        );
    };
    compile_source_batch_observed(
        source,
        "InheritedDispatch.scala",
        output.path(),
        SourceId::from_index(1330),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut observe),
    )
    .expect("an exact public override should emit both classes");

    let metadata = observed_metadata.expect("observer captures the class metadata");
    let base = metadata
        .iter()
        .find(|class| class.identity().internal_name() == "Base")
        .expect("Base metadata");
    let derived = metadata
        .iter()
        .find(|class| class.identity().internal_name() == "Derived")
        .expect("Derived metadata");
    let base_value = base
        .methods()
        .iter()
        .find(|method| method.name() == "value")
        .expect("Base declares value");
    let base_doubled = base
        .methods()
        .iter()
        .find(|method| method.name() == "doubled")
        .expect("Base declares doubled");
    let derived_value = derived
        .methods()
        .iter()
        .find(|method| method.name() == "value")
        .expect("Derived declares value");

    assert_ne!(base_value.symbol(), derived_value.symbol());
    assert_eq!(base_value.descriptor().as_str(), "()I");
    assert_eq!(derived_value.descriptor().as_str(), "()I");
    assert_eq!(
        derived.methods().len(),
        1,
        "inherited bodies are not copied"
    );

    let inherited_doubled = derived
        .dispatch_methods()
        .iter()
        .find(|method| method.name() == "doubled")
        .expect("Base.doubled remains visible on Derived");
    assert_eq!(
        inherited_doubled.receiver_class(),
        derived.identity().symbol()
    );
    assert_eq!(
        inherited_doubled.declaring_class(),
        base.identity().symbol()
    );
    assert_eq!(inherited_doubled.method(), base_doubled.symbol());
    let overridden_value = derived
        .dispatch_methods()
        .iter()
        .find(|method| method.name() == "value")
        .expect("Derived.value replaces the inherited dispatch identity");
    assert_eq!(
        overridden_value.receiver_class(),
        derived.identity().symbol()
    );
    assert_eq!(
        overridden_value.declaring_class(),
        derived.identity().symbol()
    );
    assert_eq!(overridden_value.method(), derived_value.symbol());
}

#[test]
fn source_override_accepts_equivalent_qualified_and_unqualified_reference_types() {
    let output = TestOutputDirectory::new("inherited-qualified-reference-override");
    let source = "package example\nclass Counter {}\nclass Base { def value(c: example.Counter): example.Counter = c }\nclass Derived extends Base { override def value(c: Counter): Counter = c }\n";

    compile_source_classes(source, "QualifiedOverride.scala", output.path())
        .expect("equivalent canonical class references form an exact override");
    assert_eq!(output.class_files().len(), 3);
}

#[test]
fn same_name_different_descriptor_remains_an_overload_not_an_override() {
    let output = TestOutputDirectory::new("inherited-overload-dispatch");
    let source = "class Base { def value(): Int = 10 }\nclass Derived extends Base { def value(x: Int): Int = x }\n";
    let mut observed_metadata = None;
    let mut observe = |unit: &mut JvmCompilationUnit<'_>, definitions: &Definitions| {
        observed_metadata = Some(unit.metadata(definitions).expect("valid overload metadata"));
    };
    compile_source_batch_observed(
        source,
        "InheritedOverload.scala",
        output.path(),
        SourceId::from_index(1333),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut observe),
    )
    .expect("a same-name method with another descriptor is an overload");

    let metadata = observed_metadata.expect("observer captures class metadata");
    let derived = metadata
        .iter()
        .find(|class| class.identity().internal_name() == "Derived")
        .expect("Derived metadata");
    let value_dispatches = derived
        .dispatch_methods()
        .iter()
        .filter(|method| method.name() == "value")
        .collect::<Vec<_>>();
    assert_eq!(value_dispatches.len(), 2);
    assert!(value_dispatches.iter().any(|method| {
        method.descriptor().as_str() == "()I"
            && method.declaring_class()
                == metadata
                    .iter()
                    .find(|class| class.identity().internal_name() == "Base")
                    .unwrap()
                    .identity()
                    .symbol()
    }));
    assert!(
        value_dispatches
            .iter()
            .any(|method| method.descriptor().as_str() == "(I)I")
    );
}

#[test]
fn source_override_contract_rejects_missing_targets_and_signature_mismatches() {
    for (label, source, expected_issue) in [
        (
            "no-parent-method",
            "class Base { def other(): Int = 1 }\nclass Derived extends Base { override def value(): Int = 2 }\n",
            JvmOverrideIssue::NoParentDeclaration,
        ),
        (
            "wrong-descriptor",
            "class Base { def value(x: Int): Int = x }\nclass Derived extends Base { override def value(): Int = 2 }\n",
            JvmOverrideIssue::SignatureMismatch,
        ),
        (
            "wrong-result-descriptor",
            "class Base { def value(): Int = 1 }\nclass Derived extends Base { override def value(): Boolean = true }\n",
            JvmOverrideIssue::SignatureMismatch,
        ),
        (
            "wrong-result-without-override",
            "class Base { def value(): Int = 1 }\nclass Derived extends Base { def value(): Boolean = true }\n",
            JvmOverrideIssue::SignatureMismatch,
        ),
        (
            "missing-override-modifier",
            "class Base { def value(): Int = 1 }\nclass Derived extends Base { def value(): Int = 2 }\n",
            JvmOverrideIssue::OverrideModifierRequired,
        ),
        (
            "narrower-override-visibility",
            "class Base { def value(): Int = 1 }\nclass Derived extends Base { private override def value(): Int = 2 }\n",
            JvmOverrideIssue::NarrowerVisibility,
        ),
        (
            "private-parent-override",
            "class Base { private def value(): Int = 1 }\nclass Derived extends Base { override def value(): Int = 2 }\n",
            JvmOverrideIssue::InaccessibleParentMethod,
        ),
    ] {
        let output = TestOutputDirectory::new(label);
        let error = compile_source_classes(source, "InvalidOverride.scala", output.path())
            .expect_err("invalid override metadata must fail before publishing classes");
        assert!(
            matches!(
                error,
                PipelineFailure::Codegen(JvmCodegenError::InvalidJvmOverride {
                    issue,
                    ..
                }) if issue == expected_issue
            ),
            "{label}: expected {expected_issue:?}, got {error:?}"
        );
        assert!(
            output.class_files().is_empty(),
            "{label}: no partial output"
        );
    }
}

#[test]
fn source_override_rejects_an_unplanned_parent_declaration() {
    let output = TestOutputDirectory::new("unplanned-parent-override");
    let source = "class Base { def value(): Int = 1 }\nclass Derived extends Base { override def value(): Int = 2 }\n";
    let mut omit_parent_methods = |unit: &mut JvmCompilationUnit<'_>,
                                   _definitions: &Definitions| {
        let base = unit
            .classes
            .iter()
            .find(|plan| {
                let symbol = unit.store.symbols.get(plan.class);
                unit.store.names.try_resolve(symbol.name.text()) == Some("Base")
            })
            .map(|plan| plan.class)
            .expect("source batch has Base");
        unit.classes
            .iter_mut()
            .find(|plan| plan.class == base)
            .expect("Base plan")
            .methods
            .clear();
    };
    let error = compile_source_batch_observed(
        source,
        "UnplannedParentOverride.scala",
        output.path(),
        SourceId::from_index(1331),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut omit_parent_methods),
    )
    .expect_err("an override must target a planned direct-parent method");
    assert!(
        matches!(
            error,
            PipelineFailure::Codegen(JvmCodegenError::InvalidJvmOverride {
                issue: JvmOverrideIssue::ParentMethodUnplanned,
                ..
            })
        ),
        "expected a typed unplanned-parent override error, got {error:?}"
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn source_override_requires_a_concrete_child_body() {
    let output = TestOutputDirectory::new("missing-override-body");
    let source = "class Base { def value(): Int = 1 }\nclass Derived extends Base { override def value(): Int = 2 }\n";
    let mut omit_child_body = |unit: &mut JvmCompilationUnit<'_>, _definitions: &Definitions| {
        let derived = unit
            .classes
            .iter()
            .find(|plan| {
                let symbol = unit.store.symbols.get(plan.class);
                unit.store.names.try_resolve(symbol.name.text()) == Some("Derived")
            })
            .map(|plan| plan.class)
            .expect("source batch has Derived");
        unit.classes
            .iter_mut()
            .find(|plan| plan.class == derived)
            .and_then(|plan| {
                plan.methods.iter_mut().find(|method| {
                    unit.store
                        .names
                        .try_resolve(unit.store.symbols.get(method.method).name.text())
                        == Some("value")
                })
            })
            .expect("Derived declares value")
            .body = None;
    };
    let pipeline_error = compile_source_batch_observed(
        source,
        "MissingOverrideBody.scala",
        output.path(),
        SourceId::from_index(1332),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut omit_child_body),
    )
    .expect_err("a concrete override cannot lose its body");
    assert!(
        matches!(
            pipeline_error,
            PipelineFailure::Codegen(JvmCodegenError::MissingTypedBody { .. })
        ),
        "a bodyless concrete override must fail with a typed body error: {pipeline_error:?}"
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn source_single_inheritance_emits_and_verifies_the_noarg_super_chain() {
    let output = TestOutputDirectory::new("single-inheritance-audit");
    let source = "class Base { def value(): Int = 10; def doubled(): Int = value() * 2 }\nclass Derived extends Base { private val own: Int = 7 }\n";
    let artifacts = compile_source_classes(source, "SingleInheritanceAudit.scala", output.path())
        .expect("one-hop source inheritance should emit the complete batch");
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Base", "Derived"]
    );
    let derived = decode_emitted_class(&artifacts[1]);
    assert_eq!(
        derived
            .constant_pool
            .class_name(derived.super_class.unwrap()),
        Ok("Base")
    );
    let constructor = emitted_method_code(&derived, "<init>");
    assert_method_reference(&derived, constructor, 0xb7, "Base", "<init>", "()V");
    assert_opcode_sequence(constructor, &[0x2a, 0xb7, 0x2a, 0x10, 0xb5, 0xb1]);
    assert_field_reference(&derived, constructor, 0xb5, "Derived", "own", "I");

    if require_jdk_or_skip() {
        let classpath = output.path().to_str().unwrap();
        let javap = Command::new("javap")
            .args(["-c", "-v", "-classpath", classpath, "Derived"])
            .output()
            .unwrap();
        assert!(
            javap.status.success(),
            "javap rejected Derived.class\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javap.stdout),
            String::from_utf8_lossy(&javap.stderr)
        );
        let javap_output = String::from_utf8_lossy(&javap.stdout);
        assert!(javap_output.contains("super_class: #"));
        assert!(javap_output.contains("// Base"));
        assert!(javap_output.contains("Base.\"<init>\":()V"));

        let harness = output.path().join("InheritanceHarness.java");
        fs::write(
            &harness,
            "public class InheritanceHarness { public static void main(String[] args) { Derived value = new Derived(); System.out.println(value.value() + \":\" + value.doubled()); } }\n",
        )
        .unwrap();
        let javac = Command::new("javac")
            .args(["-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .unwrap();
        assert!(
            javac.status.success(),
            "javac rejected inheritance harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "InheritanceHarness",
            ])
            .output()
            .unwrap();
        assert!(
            java.status.success(),
            "java -Xverify:all rejected the emitted superclass chain\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&java.stdout).trim(), "10:20");
    }
}

#[test]
fn source_subclass_rejects_overrides_of_final_parent_methods_without_output() {
    let output = TestOutputDirectory::new("final-parent-method-override");
    let source = "class Base { final def value(): Int = 1 }\nclass Derived extends Base { override def value(): Int = 2 }\n";
    let error = compile_source_classes(source, "FinalParentMethod.scala", output.path())
        .expect_err("a child cannot override a final JVM method");

    assert!(
        matches!(
            error,
            PipelineFailure::Codegen(JvmCodegenError::FinalJvmMethodOverride {
                ref name,
                ref descriptor,
                ..
            }) if name == "value" && descriptor.as_deref() == Some("()I")
        ),
        "expected a typed final-method override error, got {error:?}"
    );
    assert!(
        output.class_files().is_empty(),
        "a failed final-method override check must publish no class files"
    );
}

#[test]
fn source_method_cannot_redefine_an_inherited_generated_accessor() {
    let output = TestOutputDirectory::new("inherited-accessor-method-collision");
    let source = "class Base(val id: Int)\nclass Derived extends Base(1) { def id(): Int = 2 }\n";
    let error = compile_source_classes(source, "InheritedAccessorMethod.scala", output.path())
        .expect_err("a source method cannot redefine a generated inherited accessor");

    assert!(
        matches!(
            &error,
            PipelineFailure::Codegen(JvmCodegenError::InheritedJvmAccessorCollision {
                origin: dotty_jvm::JvmBinaryMethodOrigin::Method(_),
                parent_origin: dotty_jvm::JvmBinaryMethodOrigin::Accessor {
                    role: dotty_jvm::JvmAccessorDispatchRole::Getter,
                    ..
                },
                name,
                descriptor,
                ..
            }) if name == "id" && descriptor.as_deref() == Some("()I")
        ),
        "expected a typed inherited accessor collision, got {error:?}"
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn generated_accessors_cannot_redefine_inherited_source_method_slots() {
    for (label, source, expected_name, expected_descriptor) in [
        (
            "nonfinal-getter",
            "class Base { def current(): Int = 1 }\nclass Derived extends Base { val current: Int = 2 }\n",
            "current",
            "()I",
        ),
        (
            "setter",
            "class Base { def current_=(value: Int): Unit = () }\nclass Derived extends Base { var current: Int = 2 }\n",
            "current_$eq",
            "(I)V",
        ),
    ] {
        let output = TestOutputDirectory::new(label);
        let error = compile_source_classes(source, "InheritedMethodAccessor.scala", output.path())
            .expect_err("a generated accessor cannot occupy an inherited source method slot");
        assert!(
            matches!(
                &error,
                PipelineFailure::Codegen(JvmCodegenError::InheritedJvmAccessorCollision {
                    kind: dotty_jvm::JvmAccessorCollisionKind::GeneratedAccessorRedefinesInheritedMethod,
                    name,
                    descriptor,
                    ..
                }) if name == expected_name && descriptor.as_deref() == Some(expected_descriptor)
            ),
            "{label}: expected typed accessor collision, got {error:?}"
        );
        assert!(
            output.class_files().is_empty(),
            "{label}: no partial output"
        );
    }
}

#[test]
fn accessor_collision_validation_rejects_an_incompatible_inherited_result_descriptor() {
    let output = TestOutputDirectory::new("inherited-accessor-result-descriptor-mismatch");
    let source = "class Base { def current(): Long = 1L }\nclass Derived extends Base { val current: Int = 2 }\n";
    let error = compile_source_classes(source, "InheritedAccessorResult.scala", output.path())
        .expect_err(
            "same-name accessors with matching parameters and different results are unsafe",
        );

    assert!(
        matches!(
            &error,
            PipelineFailure::Codegen(JvmCodegenError::InheritedJvmAccessorCollision {
                kind: dotty_jvm::JvmAccessorCollisionKind::IncompatibleInheritedAccessorResult,
                name,
                descriptor: Some(descriptor),
                parent_descriptor: Some(parent_descriptor),
                ..
            }) if name == "current" && descriptor == "()I" && parent_descriptor == "()J"
        ),
        "expected a typed result-descriptor mismatch, got {error:?}"
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn noncolliding_inherited_accessor_and_different_descriptor_overload_pass_preflight() {
    let output = TestOutputDirectory::new("noncolliding-inherited-accessor");
    let source = "class Base(var current: Int)\nclass Derived extends Base(0) { def read(): Int = 1; def current(value: Long): Int = 2 }\n";
    let artifacts = compile_source_classes(source, "NoncollidingAccessor.scala", output.path())
        .expect(
            "a distinct descriptor overload and ordinary method coexist with inherited accessors",
        );
    assert_eq!(artifacts.len(), 2);
    let base = decode_emitted_class_named(&artifacts, "Base");
    let derived = decode_emitted_class_named(&artifacts, "Derived");
    assert_eq!(base.fields.len(), 1);
    assert!(derived.fields.is_empty(), "inherited storage stays in Base");
    assert_eq!(
        methods_with_descriptors(&derived),
        [
            ("<init>".to_owned(), "()V".to_owned()),
            ("read".to_owned(), "()I".to_owned()),
            ("current".to_owned(), "(J)I".to_owned()),
        ]
    );
    assert_invocation_sequence(
        &derived,
        emitted_method_code(&derived, "<init>"),
        &[(0xb7, "Base", "<init>", "(I)V")],
    );
}

#[test]
fn inherited_accessor_collision_reports_nearest_declaration_across_two_edges() {
    let output = TestOutputDirectory::new("grandparent-accessor-method-collision");
    let source = "class Base(val id: Int)\nclass Middle extends Base(1)\nclass Leaf extends Middle { def id(): Int = 2 }\n";
    let error = compile_source_classes(source, "GrandparentAccessor.scala", output.path())
        .expect_err("a grandchild cannot redefine a grandparent accessor");
    assert!(
        matches!(
            &error,
            PipelineFailure::Codegen(JvmCodegenError::InheritedJvmAccessorCollision {
                kind: dotty_jvm::JvmAccessorCollisionKind::SourceMethodRedefinesInheritedAccessor,
                parent_origin: dotty_jvm::JvmBinaryMethodOrigin::Accessor {
                    role: dotty_jvm::JvmAccessorDispatchRole::Getter,
                    ..
                },
                name,
                descriptor,
                ..
            }) if name == "id" && descriptor.as_deref() == Some("()I")
        ),
        "expected inherited grandparent collision, got {error:?}"
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn unplanned_final_parent_method_rejects_a_same_named_child_accessor() {
    let output = TestOutputDirectory::new("unplanned-final-parent-accessor-collision");
    let source =
        "class Base { final def id(): Int = 1 }\nclass Derived extends Base { val id: Int = 2 }\n";
    let mut omit_parent_methods = |unit: &mut JvmCompilationUnit<'_>,
                                   _definitions: &Definitions| {
        let parent = unit
            .classes
            .iter()
            .find(|plan| {
                let symbol = unit.store.symbols.get(plan.class);
                unit.store.names.try_resolve(symbol.name.text()) == Some("Base")
            })
            .map(|plan| plan.class)
            .expect("source batch has Base");
        unit.classes
            .iter_mut()
            .find(|plan| plan.class == parent)
            .expect("Base plan")
            .methods
            .clear();
    };
    let error = compile_source_batch_observed(
        source,
        "UnplannedFinalParentAccessor.scala",
        output.path(),
        SourceId::from_index(1610),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut omit_parent_methods),
    )
    .expect_err("an unplanned final source method must reject a same-named accessor");
    assert!(
        matches!(
            &error,
            PipelineFailure::Codegen(JvmCodegenError::InheritedJvmAccessorCollision {
                kind: dotty_jvm::JvmAccessorCollisionKind::GeneratedAccessorRedefinesInheritedFinalMethod,
                parent_origin: dotty_jvm::JvmBinaryMethodOrigin::Method(_),
                name,
                descriptor: None,
                ..
            }) if name == "id"
        ),
        "expected conservative unplanned-final collision, got {error:?}"
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn unplanned_final_parent_setter_rejects_a_same_named_child_var() {
    let output = TestOutputDirectory::new("unplanned-final-parent-setter-collision");
    let source = "class Base { final def current_=(value: Int): Unit = () }\nclass Derived extends Base { var current: Int = 2 }\n";
    let mut omit_parent_methods = |unit: &mut JvmCompilationUnit<'_>,
                                   _definitions: &Definitions| {
        let parent = unit
            .classes
            .iter()
            .find(|plan| {
                let symbol = unit.store.symbols.get(plan.class);
                unit.store.names.try_resolve(symbol.name.text()) == Some("Base")
            })
            .map(|plan| plan.class)
            .expect("source batch has Base");
        unit.classes
            .iter_mut()
            .find(|plan| plan.class == parent)
            .expect("Base plan")
            .methods
            .clear();
    };
    let error = compile_source_batch_observed(
        source,
        "UnplannedFinalParentSetter.scala",
        output.path(),
        SourceId::from_index(1611),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut omit_parent_methods),
    )
    .expect_err("an unplanned final setter must reject the generated child setter");
    assert!(
        matches!(
            &error,
            PipelineFailure::Codegen(JvmCodegenError::InheritedJvmAccessorCollision {
                kind: dotty_jvm::JvmAccessorCollisionKind::GeneratedAccessorRedefinesInheritedFinalMethod,
                parent_origin: dotty_jvm::JvmBinaryMethodOrigin::Method(_),
                name,
                descriptor: None,
                ..
            }) if name == "current_$eq"
        ),
        "expected conservative unplanned final setter collision, got {error:?}"
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn unplanned_nonfinal_parent_method_rejects_a_same_named_child_accessor() {
    let output = TestOutputDirectory::new("unplanned-nonfinal-parent-accessor-collision");
    let source =
        "class Base { def id(): Int = 1 }\nclass Derived extends Base { val id: Int = 2 }\n";
    let mut omit_parent_methods = |unit: &mut JvmCompilationUnit<'_>,
                                   _definitions: &Definitions| {
        let parent = unit
            .classes
            .iter()
            .find(|plan| {
                let symbol = unit.store.symbols.get(plan.class);
                unit.store.names.try_resolve(symbol.name.text()) == Some("Base")
            })
            .map(|plan| plan.class)
            .expect("source batch has Base");
        unit.classes
            .iter_mut()
            .find(|plan| plan.class == parent)
            .expect("Base plan")
            .methods
            .clear();
    };
    let error = compile_source_batch_observed(
        source,
        "UnplannedNonfinalParentAccessor.scala",
        output.path(),
        SourceId::from_index(1612),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut omit_parent_methods),
    )
    .expect_err("an unplanned non-final source method must reject a same-named accessor");
    assert!(
        matches!(
            &error,
            PipelineFailure::Codegen(JvmCodegenError::InheritedJvmAccessorCollision {
                kind: dotty_jvm::JvmAccessorCollisionKind::GeneratedAccessorRedefinesInheritedMethod,
                parent_origin: dotty_jvm::JvmBinaryMethodOrigin::Method(_),
                name,
                descriptor: None,
                parent_descriptor: None,
                ..
            }) if name == "id"
        ),
        "expected conservative unplanned-method collision, got {error:?}"
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn source_subclass_rejects_overrides_of_final_grandparent_methods_without_output() {
    let output = TestOutputDirectory::new("final-grandparent-method-override");
    let source = "class Base { final def value(): Int = 1 }\nclass Middle extends Base {}\nclass Leaf extends Middle { override def value(): Int = 2 }\n";
    let error = compile_source_classes(source, "FinalGrandparentMethod.scala", output.path())
        .expect_err("a descendant cannot override a final method declared by an ancestor");

    assert!(
        matches!(
            error,
            PipelineFailure::Codegen(JvmCodegenError::FinalJvmMethodOverride {
                ref name,
                ref descriptor,
                ..
            }) if name == "value" && descriptor.as_deref() == Some("()I")
        ),
        "expected a contextual final-method override error, got {error:?}"
    );
    assert!(
        output.class_files().is_empty(),
        "a failed ancestor final-method override check must publish no class files"
    );
}

#[test]
fn source_subclass_requires_override_for_a_concrete_grandparent_method() {
    let output = TestOutputDirectory::new("missing-grandparent-override-modifier");
    let source = "class Base { def value(): Int = 1 }\nclass Middle extends Base {}\nclass Leaf extends Middle { def value(): Int = 2 }\n";
    let error = compile_source_classes(source, "MissingGrandparentOverride.scala", output.path())
        .expect_err("a method overriding an inherited grandparent method needs `override`");

    assert!(
        matches!(
            error,
            PipelineFailure::Codegen(JvmCodegenError::InvalidJvmOverride {
                issue: JvmOverrideIssue::OverrideModifierRequired,
                ..
            })
        ),
        "expected a contextual missing-override error, got {error:?}"
    );
    assert!(
        output.class_files().is_empty(),
        "an invalid transitive override must publish no class files"
    );
}

#[test]
fn final_parent_method_name_collision_fails_closed_when_parent_method_is_unplanned() {
    let output = TestOutputDirectory::new("unplanned-final-parent-method-override");
    let source = "class Base { final def value(): Int = 1 }\nclass Derived extends Base { override def value(): Int = 2 }\n";
    let mut omit_parent_methods = |unit: &mut JvmCompilationUnit<'_>,
                                   _definitions: &Definitions| {
        let base = unit
            .classes
            .iter()
            .find(|plan| {
                let symbol = unit.store.symbols.get(plan.class);
                unit.store.names.try_resolve(symbol.name.text()) == Some("Base")
            })
            .map(|plan| plan.class)
            .expect("source batch has Base");
        unit.classes
            .iter_mut()
            .find(|plan| plan.class == base)
            .expect("Base plan")
            .methods
            .clear();
    };
    let error = compile_source_batch_observed(
        source,
        "FinalParentMethodUnplanned.scala",
        output.path(),
        SourceId::from_index(1329),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut omit_parent_methods),
    )
    .expect_err("unplanned final parent methods must not permit a same-name child method");

    assert!(
        matches!(
            error,
            PipelineFailure::Codegen(JvmCodegenError::FinalJvmMethodOverride {
                ref name,
                descriptor: None,
                ..
            }) if name == "value"
        ),
        "expected conservative final-method collision error, got {error:?}"
    );
    assert!(output.class_files().is_empty());
}

#[test]
fn typed_super_method_calls_lower_to_checked_invokespecial() {
    let source = r#"
class Base {
  private var current: Int = 0
  def value(): Int = current
  def increment(): Int = { current = current + 1; current }
  def add(amount: Int): Int = { current = current + amount; current }
  def difference(left: Int, right: Int): Int = left - right
  def enabled(flag: Boolean): Boolean = flag
  def identity(): Base = this
  def reset(): Unit = { current = 0 }
  def doubled(): Int = value() + value()
}
class Derived extends Base {
  override def value(): Int = super.value() + 1
  def original(): Int = super.value()
  def advance(): Int = super.increment()
  def addDirect(amount: Int): Int = super.add(amount)
  def differenceDirect(left: Int, right: Int): Int = super.difference(left, right)
  def orderedArguments(): Int = super.difference(super.increment(), super.increment())
  def enabledDirect(flag: Boolean): Boolean = super.enabled(flag)
  def originalBase(): Base = super.identity()
  def resetDirect(): Unit = super.reset()
  def conditional(flag: Boolean): Int = if flag then super.add(2) else super.value()
}
class Runner {
  def evaluate(base: Base): Int = base.doubled()
}
"#;
    let first_output = TestOutputDirectory::new("super-methods-first");
    let second_output = TestOutputDirectory::new("super-methods-second");
    let first = compile_source_classes(source, "SuperMethod.scala", first_output.path()).unwrap();
    let second = compile_source_classes(source, "SuperMethod.scala", second_output.path()).unwrap();
    assert_eq!(
        first
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Base", "Derived", "Runner"]
    );
    assert_eq!(
        first
            .iter()
            .map(|artifact| artifact.class.bytes())
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|artifact| artifact.class.bytes())
            .collect::<Vec<_>>(),
        "separate frontend sessions must emit deterministic class bytes and order"
    );

    let base = decode_emitted_class(
        first
            .iter()
            .find(|artifact| artifact.class.internal_name() == "Base")
            .unwrap(),
    );
    let derived = decode_emitted_class(
        first
            .iter()
            .find(|artifact| artifact.class.internal_name() == "Derived")
            .unwrap(),
    );
    let runner = decode_emitted_class(
        first
            .iter()
            .find(|artifact| artifact.class.internal_name() == "Runner")
            .unwrap(),
    );

    assert_eq!(
        derived
            .constant_pool
            .class_name(derived.super_class.expect("Derived has a direct parent")),
        Ok("Base")
    );
    assert_invocation_sequence(
        &derived,
        emitted_method_code(&derived, "value"),
        &[(0xb7, "Base", "value", "()I")],
    );
    assert_invocation_sequence(
        &derived,
        emitted_method_code(&derived, "original"),
        &[(0xb7, "Base", "value", "()I")],
    );
    assert_invocation_sequence(
        &derived,
        emitted_method_code(&derived, "advance"),
        &[(0xb7, "Base", "increment", "()I")],
    );
    assert_invocation_sequence(
        &derived,
        emitted_method_code(&derived, "addDirect"),
        &[(0xb7, "Base", "add", "(I)I")],
    );
    assert_invocation_sequence(
        &derived,
        emitted_method_code(&derived, "differenceDirect"),
        &[(0xb7, "Base", "difference", "(II)I")],
    );
    assert_invocation_sequence(
        &derived,
        emitted_method_code(&derived, "orderedArguments"),
        &[
            (0xb7, "Base", "increment", "()I"),
            (0xb7, "Base", "increment", "()I"),
            (0xb7, "Base", "difference", "(II)I"),
        ],
    );
    assert_invocation_sequence(
        &derived,
        emitted_method_code(&derived, "enabledDirect"),
        &[(0xb7, "Base", "enabled", "(Z)Z")],
    );
    assert_invocation_sequence(
        &derived,
        emitted_method_code(&derived, "originalBase"),
        &[(0xb7, "Base", "identity", "()LBase;")],
    );
    assert_invocation_sequence(
        &derived,
        emitted_method_code(&derived, "resetDirect"),
        &[(0xb7, "Base", "reset", "()V")],
    );
    assert_invocation_sequence(
        &derived,
        emitted_method_code(&derived, "conditional"),
        &[
            (0xb7, "Base", "add", "(I)I"),
            (0xb7, "Base", "value", "()I"),
        ],
    );
    assert!(has_stack_map_table(&derived, "conditional"));
    assert_invocation_sequence(
        &base,
        emitted_method_code(&base, "doubled"),
        &[
            (0xb6, "Base", "value", "()I"),
            (0xb6, "Base", "value", "()I"),
        ],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "evaluate"),
        &[(0xb6, "Base", "doubled", "()I")],
    );

    if require_jdk_or_skip() {
        verify_super_calls_with_jdk(first_output.path());
    }

    for (label, source) in [
        (
            "mixin-qualified-super",
            "class Base { def value(): Int = 10 }\nclass Derived extends Base { def read(): Int = super[Base].value() }\n",
        ),
        (
            "owner-qualified-super",
            "class Base { def value(): Int = 10 }\nclass Derived extends Base { def read(): Int = Base.super.value() }\n",
        ),
        (
            "root-class-super",
            "class Base { def read(): Int = super.value() }\n",
        ),
    ] {
        let output = TestOutputDirectory::new(label);
        let error = compile_source_classes(source, "SuperAudit.scala", output.path())
            .expect_err("unsupported super qualifier or owner must fail before publication");
        assert!(
            matches!(error, PipelineFailure::Typer(_)),
            "{label} must be rejected by the typer, not lowered through a fallback: {error:?}"
        );
        assert!(output.class_files().is_empty());
    }

    let output = TestOutputDirectory::new("unplanned-super-method");
    let source = "class Base { def value(): Int = 10 }\nclass Derived extends Base { def read(): Int = super.value() }\n";
    let mut omit_parent_methods = |unit: &mut JvmCompilationUnit<'_>,
                                   _definitions: &Definitions| {
        let base = unit
            .classes
            .iter()
            .find(|plan| {
                let symbol = unit.store.symbols.get(plan.class);
                unit.store.names.try_resolve(symbol.name.text()) == Some("Base")
            })
            .map(|plan| plan.class)
            .expect("source batch contains Base");
        unit.classes
            .iter_mut()
            .find(|plan| plan.class == base)
            .expect("Base plan")
            .methods
            .clear();
    };
    let error = compile_source_batch_observed(
        source,
        "UnplannedSuperMethod.scala",
        output.path(),
        SourceId::from_index(1374),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut omit_parent_methods),
    )
    .expect_err("a selected direct-parent method must be in the emission plan");
    assert!(
        matches!(
            error,
            PipelineFailure::Codegen(
                JvmCodegenError::MethodNotInJvmPlan { .. }
                    | JvmCodegenError::MethodNotInJvmPlanForBody { .. }
            )
        ),
        "an unplanned super target must fail instead of falling back to virtual dispatch: {error:?}"
    );
    assert!(output.class_files().is_empty());
}

fn verify_super_calls_with_jdk(output_directory: &Path) {
    let output_path = output_directory.to_str().expect("UTF-8 output path");
    let javap = Command::new("javap")
        .args([
            "-p",
            "-c",
            "-v",
            "-classpath",
            output_path,
            "Base",
            "Derived",
            "Runner",
        ])
        .output()
        .expect("start javap for direct-super classes");
    assert!(
        javap.status.success(),
        "javap rejected direct-super classes\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javap.stdout),
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap = String::from_utf8_lossy(&javap.stdout);
    for evidence in [
        "major version: 65",
        "descriptor: ()I",
        "descriptor: (I)I",
        "descriptor: (II)I",
        "descriptor: (Z)Z",
        "descriptor: ()LBase;",
        "StackMapTable",
    ] {
        assert!(
            javap.contains(evidence),
            "javap omitted {evidence:?}:\n{javap}"
        );
    }
    let derived_javap = Command::new("javap")
        .args(["-p", "-c", "-classpath", output_path, "Derived"])
        .output()
        .expect("start javap for Derived direct-super methods");
    assert!(
        derived_javap.status.success(),
        "javap rejected Derived direct-super methods\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&derived_javap.stdout),
        String::from_utf8_lossy(&derived_javap.stderr)
    );
    let derived_javap = String::from_utf8_lossy(&derived_javap.stdout);
    assert_javap_instruction_sequence(
        &derived_javap,
        "public int value();",
        &["aload_0", "invokespecial", "iconst_1", "iadd", "ireturn"],
    );
    assert_javap_instruction_sequence(
        &javap,
        "public int original();",
        &["aload_0", "invokespecial", "ireturn"],
    );
    assert_javap_instruction_sequence(
        &javap,
        "public int advance();",
        &["aload_0", "invokespecial", "ireturn"],
    );
    assert_javap_instruction_sequence(
        &javap,
        "public int addDirect(int);",
        &["aload_0", "iload_1", "invokespecial", "ireturn"],
    );
    assert_javap_instruction_sequence(
        &javap,
        "public int differenceDirect(int, int);",
        &["aload_0", "iload_1", "iload_2", "invokespecial", "ireturn"],
    );
    assert_javap_instruction_sequence(
        &javap,
        "public int orderedArguments();",
        &[
            "aload_0",
            "aload_0",
            "invokespecial",
            "aload_0",
            "invokespecial",
            "invokespecial",
            "ireturn",
        ],
    );
    assert_javap_instruction_sequence(&javap, "public Base identity();", &["aload_0", "areturn"]);
    assert_javap_instruction_sequence(
        &javap,
        "public void resetDirect();",
        &["aload_0", "invokespecial", "return"],
    );
    assert_javap_instruction_sequence(
        &javap,
        "public int doubled();",
        &[
            "aload_0",
            "invokevirtual",
            "aload_0",
            "invokevirtual",
            "iadd",
            "ireturn",
        ],
    );
    assert_javap_instruction_sequence(
        &javap,
        "public int evaluate(Base);",
        &["aload_1", "invokevirtual", "ireturn"],
    );

    let harness = output_directory.join("SuperCallsHarness.java");
    fs::write(
        &harness,
        r#"public final class SuperCallsHarness {
  public static void main(String[] args) {
    Derived value = new Derived();
    Runner runner = new Runner();
    if (value.value() != 1) throw new AssertionError("override with super.value");
    if (value.original() != 0) throw new AssertionError("direct parent result");
    if (value.originalBase() != value) throw new AssertionError("direct parent reference result");
    if (!value.enabledDirect(true) || value.enabledDirect(false)) throw new AssertionError("boolean result");
    value.resetDirect();
    if (value.original() != 0) throw new AssertionError("Unit result");
    if (value.advance() != 1 || value.value() != 2) throw new AssertionError("direct parent mutation");
    if (value.addDirect(5) != 6) throw new AssertionError("ordered Int argument");
    if (value.differenceDirect(9, 4) != 5) throw new AssertionError("source argument order");
    if (value.orderedArguments() != -1 || value.original() != 8) throw new AssertionError("ordered, exactly-once arguments");
    if (value.conditional(true) != 10 || value.conditional(false) != 10) throw new AssertionError("branched super calls");
    if (runner.evaluate(value) != 22) throw new AssertionError("ordinary virtual dispatch from Base");
    if (runner.evaluate(new Base()) != 0) throw new AssertionError("base virtual dispatch");
    System.out.println("direct super calls verified");
  }
}
"#,
    )
    .expect("write direct-super Java harness");
    let javac = Command::new("javac")
        .args([
            "--release",
            "21",
            "-classpath",
            output_path,
            "-d",
            output_path,
        ])
        .arg(&harness)
        .output()
        .expect("start javac for direct-super harness");
    assert!(
        javac.status.success(),
        "javac rejected direct-super harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            output_path,
            "SuperCallsHarness",
        ])
        .output()
        .expect("start JVM verifier for direct-super classes");
    assert!(
        java.status.success(),
        "java -Xverify:all rejected direct-super calls\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "direct super calls verified"
    );
}

#[test]
fn parameterized_parent_constructor_arguments_keep_their_source_mappings() {
    let source_text = "class Base(seed: Int):\n  def value(): Int = seed\n\nclass Derived(start: Int) extends Base(start):\n  def read(): Int = start\n\nclass Other extends Base(2) {}\n";
    let source_id = SourceId::from_index(1368);
    let source = SourceText::new(source_text).unwrap();
    let scanner = ContextualScanner::new(source_text).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("expected package root");
    };
    let mut packages = Packages::new();
    let index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "ParameterizedParent.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } => {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            index
        }
        NamingOutcome::Rejected { diagnostics } => panic!("naming failed: {diagnostics:?}"),
    };
    let class_named = |wanted: &str| {
        package
            .stats
            .iter()
            .find_map(|tree| {
                let TreeKind::TypeDef(definition) = &parsed.ast.get(*tree).kind else {
                    return None;
                };
                let symbol = index.symbol_at(source_id, *tree)?;
                (store.names.resolve(definition.name.as_name().text()) == wanted).then_some(symbol)
            })
            .unwrap_or_else(|| panic!("source declares {wanted}"))
    };
    let base = class_named("Base");
    let derived = class_named("Derived");
    let other = class_named("Other");
    let dotty_core::SourceDefinition::Canonical {
        source,
        tree: derived_tree,
    } = index.definition_of(derived).unwrap()
    else {
        panic!("Derived has canonical source provenance");
    };
    assert_eq!(source, source_id);
    let TreeKind::TypeDef(derived_definition) = &parsed.ast.get(derived_tree).kind else {
        panic!("Derived source identity points to a TypeDef");
    };
    let TreeKind::Template(template) = &parsed.ast.get(derived_definition.rhs).kind else {
        panic!("Derived has a template");
    };
    let [parent_application] = template.parents.as_slice() else {
        panic!("expected one source parent application");
    };
    let TreeKind::Apply(application) = &parsed.ast.get(*parent_application).kind else {
        panic!("Base(start) remains a parent constructor application");
    };
    assert_eq!(application.args.len(), 1);
    let TreeKind::Select(constructor_select) = &parsed.ast.get(application.function).kind else {
        panic!("parent application selects <init>");
    };
    assert_eq!(
        store.names.resolve(constructor_select.name.text()),
        "<init>"
    );
    let TreeKind::New(parent_new) = &parsed.ast.get(constructor_select.qualifier).kind else {
        panic!("constructor selection is qualified by New");
    };
    let TreeKind::Ident(parent_type) = &parsed.ast.get(parent_new.tpt).kind else {
        panic!("Base is represented by its source type identifier");
    };
    assert_eq!(store.names.resolve(parent_type.name.text()), "Base");
    let TreeKind::Ident(parent_argument) = &parsed.ast.get(application.args[0]).kind else {
        panic!("start remains the parent argument expression");
    };
    assert_eq!(store.names.resolve(parent_argument.name.text()), "start");

    let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
        panic!("template owns its primary constructor");
    };
    let [start_parameter] = constructor.value_param_clauses[0].as_slice() else {
        panic!("Derived primary constructor has one formal parameter");
    };
    let constructor_symbol = index.symbol_at(source_id, template.constructor).unwrap();
    let start_symbol = index
        .derived_symbol_at(constructor_symbol, source_id, *start_parameter)
        .expect("constructor formal has an owner-scoped derived identity");
    assert_eq!(
        store.symbols.get(start_symbol).owner,
        Some(constructor_symbol)
    );
    assert_eq!(store.symbols.get(start_symbol).kind, SymbolKind::Parameter);

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.complete_symbol(derived).unwrap();
    typer.complete_symbol(other).unwrap();
    let class_info = match typer.store().symbols.info(derived) {
        dotty_core::SymbolInfo::Complete(info) => *info,
        other => panic!("Derived class info is complete: {other:?}"),
    };
    let Type::ClassInfo(class_info) = typer.store().types.get(class_info) else {
        panic!("Derived has ClassInfo");
    };
    assert_eq!(class_info.parents.len(), 1);
    assert!(matches!(
        typer.store().types.get(class_info.parents[0]),
        Type::TypeRef {
            target: TypeRefTarget::Symbol(parent),
            ..
        } if *parent == base
    ));
    assert!(
        typer
            .source_typed_index()
            .get(source_id, *parent_application)
            .is_some()
    );
    assert!(
        typer
            .source_typed_index()
            .get(source_id, application.args[0])
            .is_some()
    );

    let dotty_core::SourceDefinition::Canonical {
        tree: other_tree, ..
    } = index.definition_of(other).unwrap()
    else {
        panic!("Other has canonical source provenance");
    };
    let TreeKind::TypeDef(other_definition) = &parsed.ast.get(other_tree).kind else {
        panic!("Other source identity points to a TypeDef");
    };
    let TreeKind::Template(other_template) = &parsed.ast.get(other_definition.rhs).kind else {
        panic!("Other has a template");
    };
    let [other_parent_application] = other_template.parents.as_slice() else {
        panic!("Other has one parent application");
    };
    let other_typed_application = typer
        .source_typed_index()
        .get(source_id, *other_parent_application)
        .unwrap();
    let TreeKind::Apply(other_typed_apply) = &typer.typed_ast().get(other_typed_application).kind
    else {
        panic!("Other parent call has a typed Apply");
    };
    let other_typed_argument = other_typed_apply.args[0];
    let constructor_plan = JvmPrimaryConstructorPlan::from_source(
        derived,
        constructor_symbol,
        vec![start_symbol],
        &parsed.ast,
        &index,
    )
    .unwrap();
    let error = constructor_plan
        .with_direct_super_constructor(
            *parent_application,
            other_typed_application,
            vec![dotty_jvm::JvmConstructorArgumentBinding {
                source: application.args[0],
                typed: other_typed_argument,
            }],
            &parsed.ast,
            &index,
            typer.source_typed_index(),
            typer.store(),
            typer.typed_ast(),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        JvmCodegenError::InvalidPrimaryConstructor {
            issue: dotty_jvm::JvmConstructorIssue::ParentConstructorArgumentIdentity,
            ..
        }
    ));
}

#[test]
fn parameterized_parent_constructor_is_emitted_from_the_source_proof() {
    let output = TestOutputDirectory::new("parameterized-parent-constructor-supported");
    let source = "class Base(seed: Int) {}\nclass Derived(start: Int) extends Base(start) {}\n";
    let artifacts = compile_source_classes(source, "ParameterizedParent.scala", output.path())
        .expect("the source proof selects the canonical parameterized parent constructor");
    let derived = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "Derived")
        .expect("Derived class was emitted");
    let class = decode_emitted_class(derived);
    let constructor = emitted_method_code(&class, "<init>");
    assert_method_reference(&class, constructor, 0xb7, "Base", "<init>", "(I)V");
}

#[test]
fn source_local_declarations_export_method_scoped_canonical_bindings() {
    let source_text = "class C {\n def first(x: Int): Int = { val value = x; { val value = value; value } }\n def second(x: Int): Int = { val value = x; value }\n}";
    let source_id = SourceId::from_index(1);
    let source = SourceText::new(source_text).unwrap();
    let scanner = ContextualScanner::new(source_text).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = dotty_core::Packages::new();
    let index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "LocalBindings.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        other => panic!("source should be named without diagnostics: {other:?}"),
    };
    let (class_symbol, template_tree) =
        find_single_source_class(&parsed.ast, parsed.root, source_id, &index, &store).unwrap();
    let TreeKind::Template(template) = &parsed.ast.get(template_tree).kind else {
        panic!("source class should have a template")
    };
    let source_methods =
        collect_source_methods(&parsed.ast, &template.body, source_id, &index, &store).unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.complete_symbol(class_symbol).unwrap();
    let mut method_plans = Vec::new();
    for source_method in source_methods {
        typer.complete_symbol(source_method.symbol).unwrap();
        let rhs = source_method.rhs.unwrap();
        let context = typer.expression_context_for(source_method.symbol).unwrap();
        let body = typer.type_expression(rhs, context).unwrap();
        let mut locals = parsed
            .ast
            .iter()
            .filter_map(|(source_tree, node)| {
                if !matches!(node.kind, TreeKind::ValDef(_)) {
                    return None;
                }
                let binding = typer.typed_local_binding_at(source_id, source_tree)?;
                (typer.store().symbols.get(binding.symbol()).owner == Some(source_method.symbol))
                    .then_some(JvmLocalBinding {
                        declaration: binding.declaration(),
                        symbol: binding.symbol(),
                        mutable: typer
                            .store()
                            .symbols
                            .get(binding.symbol())
                            .flags
                            .contains(SymbolFlags::MUTABLE),
                    })
            })
            .collect::<Vec<_>>();
        locals.sort_by_key(|binding| binding.declaration.index());
        assert_eq!(locals.len(), if method_plans.is_empty() { 2 } else { 1 });
        assert!(
            locals
                .windows(2)
                .all(|pair| pair[0].declaration.index() < pair[1].declaration.index())
        );
        method_plans.push(JvmMethodPlan {
            method: source_method.symbol,
            parameters: source_method.parameters,
            locals,
            body: Some(body),
        });
    }
    assert_eq!(method_plans.len(), 2);
    assert_eq!(method_plans[0].locals.len(), 2);
    assert_eq!(method_plans[1].locals.len(), 1);
    assert_ne!(
        method_plans[0].locals[0].symbol,
        method_plans[1].locals[0].symbol
    );
    assert_ne!(
        method_plans[0].locals[0].declaration,
        method_plans[1].locals[0].declaration
    );
    let unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: vec![JvmClassPlan {
            class: class_symbol,
            fields: Vec::new(),
            methods: method_plans,
            initialization: JvmClassInitializationPlan::Unsupported,
        }],
    };
    assert_eq!(unit.validate(), Ok(()));
}

#[test]
fn super_calls_fixture_combines_parameterized_construction_and_special_dispatch() {
    let first_output = TestOutputDirectory::new("super-calls-first");
    let second_output = TestOutputDirectory::new("super-calls-second");
    let first = compile_fixture_classes("SuperCalls.scala", first_output.path()).unwrap();
    let second = compile_fixture_classes("SuperCalls.scala", second_output.path()).unwrap();
    assert_eq!(
        first
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["Base", "Derived", "Runner"]
    );
    assert_eq!(
        first
            .iter()
            .map(|artifact| (artifact.class.internal_name(), artifact.class.bytes()))
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|artifact| (artifact.class.internal_name(), artifact.class.bytes()))
            .collect::<Vec<_>>()
    );
    for (artifacts, output) in [(&first, &first_output), (&second, &second_output)] {
        assert_eq!(output.class_files().len(), 3);
        for artifact in artifacts {
            assert_eq!(fs::read(&artifact.path).unwrap(), artifact.class.bytes());
            let class = decode_emitted_class(artifact);
            assert!(class.access_flags.is_public());
            assert!(
                class
                    .access_flags
                    .contains(dotty_classfile::access_flags::ACC_SUPER)
            );
            assert!(
                class
                    .methods
                    .iter()
                    .all(|method| method.access_flags.is_public()
                        && !method.access_flags.is_static())
            );
            for (name, _) in methods_with_descriptors(&class) {
                assert!(
                    !has_stack_map_table(&class, &name),
                    "the fixture is branchless"
                );
            }
        }
    }
    let base = decode_emitted_class(&first[0]);
    let derived = decode_emitted_class(&first[1]);
    let runner = decode_emitted_class(&first[2]);
    for (class, parent, descriptor) in [
        (&base, "java/lang/Object", "(I)V"),
        (&derived, "Base", "(I)V"),
        (&runner, "java/lang/Object", "()V"),
    ] {
        assert_eq!(
            class.constant_pool.class_name(class.super_class.unwrap()),
            Ok(parent)
        );
        assert_eq!(
            methods_with_descriptors(class)
                .into_iter()
                .filter(|(name, _)| name == "<init>")
                .collect::<Vec<_>>(),
            [("<init>".to_owned(), descriptor.to_owned())]
        );
    }
    assert_eq!(base.fields.len(), 1);
    let current = &base.fields[0];
    assert_eq!(base.constant_pool.utf8(current.name_index), Ok("current"));
    assert_eq!(base.constant_pool.utf8(current.descriptor_index), Ok("I"));
    assert!(current.access_flags.is_private());
    assert!(!current.access_flags.is_final());
    assert!(derived.fields.is_empty());
    assert!(runner.fields.is_empty());
    assert_opcode_sequence(
        emitted_method_code(&base, "<init>"),
        &[0x2a, 0xb7, 0x2a, 0x1b, 0xb5, 0xb1],
    );
    assert_field_reference(
        &base,
        emitted_method_code(&base, "<init>"),
        0xb5,
        "Base",
        "current",
        "I",
    );
    assert_opcode_sequence(
        emitted_method_code(&derived, "<init>"),
        &[0x2a, 0x1b, 0xb7, 0xb1],
    );
    assert_invocation_sequence(
        &derived,
        emitted_method_code(&derived, "<init>"),
        &[(0xb7, "Base", "<init>", "(I)V")],
    );
    assert_opcode_sequence(
        emitted_method_code(&derived, "value"),
        &[0x2a, 0xb7, 0x04, 0x60, 0xac],
    );
    for (method, target) in [
        ("value", "value"),
        ("original", "value"),
        ("advance", "increment"),
    ] {
        assert_invocation_sequence(
            &derived,
            emitted_method_code(&derived, method),
            &[(0xb7, "Base", target, "()I")],
        );
    }
    assert_invocation_sequence(
        &base,
        emitted_method_code(&base, "doubled"),
        &[
            (0xb6, "Base", "value", "()I"),
            (0xb6, "Base", "value", "()I"),
        ],
    );
    assert_invocation_sequence(
        &runner,
        emitted_method_code(&runner, "evaluate"),
        &[(0xb6, "Base", "doubled", "()I")],
    );
    for (method, owner) in [("fromBase", "Base"), ("fromDerived", "Derived")] {
        assert_invocation_sequence(
            &runner,
            emitted_method_code(&runner, method),
            &[
                (0xb7, owner, "<init>", "(I)V"),
                (0xb6, "Runner", "evaluate", "(LBase;)I"),
            ],
        );
    }
    for (class, method, stack, locals) in [
        (&base, "<init>", 2, 2),
        (&derived, "<init>", 2, 2),
        (&derived, "value", 2, 1),
        (&runner, "evaluate", 1, 2),
        (&runner, "fromDerived", 4, 2),
        (&runner, "afterAdvance", 3, 3),
    ] {
        let method = class
            .methods
            .iter()
            .find(|entry| class.constant_pool.utf8(entry.name_index) == Ok(method))
            .unwrap();
        let [Attribute::Code(code)] = method.attributes.as_slice() else {
            panic!("one Code attribute");
        };
        assert_eq!((code.max_stack, code.max_locals), (stack, locals));
    }
    if require_jdk_or_skip() {
        verify_parameterized_super_calls_with_jdk(first_output.path());
    }
}

fn verify_parameterized_super_calls_with_jdk(output: &Path) {
    let classpath = output.to_str().unwrap();
    // Separate disassemblies keep identically named parent/child methods distinct.
    for (class, methods) in [
        (
            "Base",
            vec![
                (
                    "public Base(int);",
                    vec![
                        "aload_0",
                        "invokespecial",
                        "aload_0",
                        "iload_1",
                        "putfield",
                        "return",
                    ],
                ),
                (
                    "public int doubled();",
                    vec![
                        "aload_0",
                        "invokevirtual",
                        "aload_0",
                        "invokevirtual",
                        "iadd",
                        "ireturn",
                    ],
                ),
            ],
        ),
        (
            "Derived",
            vec![
                (
                    "public Derived(int);",
                    vec!["aload_0", "iload_1", "invokespecial", "return"],
                ),
                (
                    "public int value();",
                    vec!["aload_0", "invokespecial", "iconst_1", "iadd", "ireturn"],
                ),
                (
                    "public int original();",
                    vec!["aload_0", "invokespecial", "ireturn"],
                ),
                (
                    "public int advance();",
                    vec!["aload_0", "invokespecial", "ireturn"],
                ),
            ],
        ),
        (
            "Runner",
            vec![(
                "public int evaluate(Base);",
                vec!["aload_1", "invokevirtual", "ireturn"],
            )],
        ),
    ] {
        let javap = Command::new("javap")
            .args(["-p", "-c", "-v", "-classpath", classpath, class])
            .output()
            .unwrap();
        assert!(
            javap.status.success(),
            "{}",
            String::from_utf8_lossy(&javap.stderr)
        );
        let text = String::from_utf8_lossy(&javap.stdout);
        assert!(text.contains("major version: 65"));
        for (header, instructions) in methods {
            assert_javap_instruction_sequence(&text, header, &instructions);
        }
        if class == "Derived" {
            for reference in [
                "Base.\"<init>\":(I)V",
                "Base.value:()I",
                "Base.increment:()I",
            ] {
                assert!(text.contains(reference), "missing {reference}: {text}");
            }
        }
    }
    let harness = output.join("ParameterizedSuperCallsHarness.java");
    fs::write(&harness, r#"public final class ParameterizedSuperCallsHarness {
  public static void main(String[] args) {
    Runner runner = new Runner();
    if (runner.fromBase(10) != 20) throw new AssertionError("Base constructor and virtual call");
    if (runner.fromDerived(10) != 22) throw new AssertionError("Derived override dispatch");
    if (runner.original(10) != 10) throw new AssertionError("special Base.value");
    if (runner.advance(10) != 11) throw new AssertionError("special Base.increment");
    if (runner.afterAdvance(10) != 12) throw new AssertionError("state after special mutation");
    if (new Derived(10).value() != 11 || new Derived(10).doubled() != 22) throw new AssertionError("inherited virtual dispatch");
    if (runner.fromDerived(0) != 2 || runner.fromDerived(-3) != -4 || runner.fromBase(-3) != -6) throw new AssertionError("signed constructor parameters");
    System.out.println("parameterized super calls verified");
  }
}
"#).unwrap();
    let javac = Command::new("javac")
        .args(["--release", "21", "-classpath", classpath, "-d", classpath])
        .arg(harness)
        .output()
        .unwrap();
    assert!(
        javac.status.success(),
        "{}",
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            classpath,
            "ParameterizedSuperCallsHarness",
        ])
        .output()
        .unwrap();
    assert!(
        java.status.success(),
        "{}",
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "parameterized super calls verified"
    );
}

#[test]
fn super_calls_reject_unsupported_source_before_publishing_the_batch() {
    for (label, source) in [
        (
            "parent-private",
            "class Base private (seed: Int) {}\nclass Derived(start: Int) extends Base(start) {}",
        ),
        (
            "parent-arity",
            "class Base(seed: Int) {}\nclass Derived(start: Int) extends Base() {}",
        ),
        (
            "parent-type",
            "class Base(seed: Int) {}\nclass Derived(start: Int) extends Base(true) {}",
        ),
        (
            "parent-this",
            "class Base(seed: Int) {}\nclass Derived(start: Int) extends Base(this.read()) { def read(): Int = 1 }",
        ),
        (
            "parent-branch",
            "class Base(seed: Int) {}\nclass Derived(start: Int) extends Base(if true then start else 0) {}",
        ),
        (
            "parent-new",
            "class Dep {}\nclass Base(dep: Dep) {}\nclass Derived(start: Int) extends Base(new Dep()) {}",
        ),
        (
            "parent-effects",
            "class Base(seed: Int) {}\nclass Derived(start: Int) extends Base({ var n: Int = start; n = n + 1; n }) {}",
        ),
        (
            "super-mixin",
            "class Base(seed: Int) { def value(): Int = 1 }\nclass Derived(start: Int) extends Base(start) { def value2(): Int = super[Base].value() }",
        ),
        (
            "super-qualified",
            "class Base(seed: Int) { def value(): Int = 1 }\nclass Derived(start: Int) extends Base(start) { def value2(): Int = Derived.super.value() }",
        ),
        (
            "super-private",
            "class Base(seed: Int) { private def value(): Int = 1 }\nclass Derived(start: Int) extends Base(start) { def value2(): Int = super.value() }",
        ),
        (
            "super-missing",
            "class Base(seed: Int) {}\nclass Derived(start: Int) extends Base(start) { def value(): Int = super.missing() }",
        ),
    ] {
        let output = TestOutputDirectory::new(&format!("super-calls-{label}"));
        let error = compile_source_classes(source, "RejectedSuperCalls.scala", output.path())
            .expect_err("unsupported source must fail closed");
        let expected = match label {
            "parent-private" => matches!(
                &error,
                PipelineFailure::Typer(TyperError::ParentConstructorInaccessible { .. })
            ),
            "parent-arity" => matches!(
                &error,
                PipelineFailure::Typer(TyperError::ApplicationArityMismatch {
                    expected: 1,
                    actual: 0,
                    ..
                })
            ),
            "parent-type" => matches!(
                &error,
                PipelineFailure::Typer(TyperError::ApplicationArgumentTypeMismatch {
                    argument_index: 0,
                    ..
                })
            ),
            "parent-this" => matches!(
                &error,
                PipelineFailure::Typer(TyperError::UninitializedParentArgumentReceiver { .. })
            ),
            "parent-branch" | "parent-new" | "parent-effects" => matches!(
                &error,
                PipelineFailure::Codegen(JvmCodegenError::InvalidPrimaryConstructor {
                    issue: dotty_jvm::JvmConstructorIssue::ParentConstructorTypedShape,
                    ..
                })
            ),
            "super-mixin" | "super-qualified" | "super-private" => matches!(
                &error,
                PipelineFailure::Typer(TyperError::UnsupportedSuper { .. })
            ),
            "super-missing" => {
                matches!(
                    &error,
                    PipelineFailure::Typer(TyperError::MemberNotFound { .. })
                )
            }
            _ => unreachable!(),
        };
        assert!(expected, "{label} failed at an unexpected stage: {error:?}");
        assert!(
            output.class_files().is_empty(),
            "{label} published a partial batch"
        );
    }
}

#[test]
fn super_calls_keep_canonical_targets_and_exact_frames_in_ordinary_branches() {
    let source = fs::read_to_string(fixture_path("SuperCalls.scala")).unwrap()
        + "\nclass Branching { def choose(flag: Boolean, value: Derived): Int = if flag then value.original() else value.advance() }\n";
    let output = TestOutputDirectory::new("super-calls-branches");
    let mut checked_calls = 0;
    let mut observe = |unit: &mut JvmCompilationUnit<'_>, definitions: &Definitions| {
        let derived = unit
            .classes
            .iter()
            .find(|plan| {
                unit.store
                    .names
                    .try_resolve(unit.store.symbols.get(plan.class).name.text())
                    == Some("Derived")
            })
            .unwrap();
        let parent = unit
            .class_hierarchy(definitions)
            .unwrap()
            .superclasses()
            .iter()
            .find(|edge| edge.class() == derived.class)
            .unwrap()
            .parent();
        // Follow each real method body so the resolver sees its actual caller.
        for caller in &derived.methods {
            let mut pending = vec![caller.body.unwrap()];
            while let Some(call) = pending.pop() {
                match &unit.typed_ast.get(call).kind {
                    TreeKind::Apply(apply) => {
                        pending.push(apply.function);
                        pending.extend(apply.args.iter().copied());
                        let TreeKind::Select(select) = &unit.typed_ast.get(apply.function).kind
                        else {
                            continue;
                        };
                        if !matches!(
                            unit.typed_ast.get(select.qualifier).kind,
                            TreeKind::Super(_)
                        ) {
                            continue;
                        }
                        let Some(Type::TermRef {
                            target: TermRefTarget::Symbol(selected),
                            ..
                        }) = unit
                            .store
                            .types
                            .try_get(unit.typed_ast.get(apply.function).ty)
                        else {
                            panic!("canonical selected method");
                        };
                        let target = unit
                            .resolve_direct_super_call(
                                derived.class,
                                caller.method,
                                call,
                                definitions,
                            )
                            .unwrap();
                        assert_eq!(target.method_symbol(), *selected);
                        assert_eq!(
                            target.invocation_kind(),
                            dotty_jvm::JvmInvocationKind::SpecialSuper
                        );
                        assert_eq!(unit.store.symbols.get(*selected).owner, Some(parent));
                        checked_calls += 1;
                    }
                    TreeKind::Select(select) => pending.push(select.qualifier),
                    TreeKind::Super(node) => pending.push(node.qual),
                    TreeKind::This(_) | TreeKind::Literal(_) | TreeKind::Ident(_) => {}
                    other => panic!("unexpected fixture expression: {other:?}"),
                }
            }
        }
    };
    let artifacts = compile_source_batch_observed(
        &source,
        "SuperCallsBranches.scala",
        output.path(),
        SourceId::from_index(1375),
        SourceUnitCardinality::AtLeastTwo,
        Some(&mut observe),
    )
    .unwrap();
    assert_eq!(checked_calls, 3);
    let branching = decode_emitted_class(
        artifacts
            .iter()
            .find(|artifact| artifact.class.internal_name() == "Branching")
            .unwrap(),
    );
    let frames = stack_map_frames(&branching, "choose");
    let targets = decode_branch_targets(emitted_method_code(&branching, "choose"));
    assert!(!targets.is_empty());
    for (_, _, target) in targets {
        assert!(frames.iter().any(|frame| frame.pc == target));
    }
    assert!(frames.iter().all(|frame| {
        frame
            .locals
            .contains(&StackMapVerificationType::Object("Derived".to_owned()))
    }));
    if require_jdk_or_skip() {
        let harness = output.path().join("SuperCallsBranchesHarness.java");
        fs::write(&harness, "public class SuperCallsBranchesHarness { public static void main(String[] args) { Branching b = new Branching(); Derived d = new Derived(10); if (b.choose(true, d) != 10 || b.choose(false, d) != 11 || b.choose(true, d) != 11) throw new AssertionError(); } }").unwrap();
        let classpath = output.path().to_str().unwrap();
        let javac = Command::new("javac")
            .args(["--release", "21", "-classpath", classpath, "-d", classpath])
            .arg(harness)
            .output()
            .unwrap();
        assert!(
            javac.status.success(),
            "{}",
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-classpath",
                classpath,
                "SuperCallsBranchesHarness",
            ])
            .output()
            .unwrap();
        assert!(
            java.status.success(),
            "{}",
            String::from_utf8_lossy(&java.stderr)
        );
    }
}
