use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use dotty::jvm_tasty_cross_class::{
    CrossClassConstructorCallProofInput, validate_cross_class_constructor_call_proofs,
};
use dotty_classfile::access_flags::{ACC_PRIVATE, ACC_PUBLIC};
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::reader::Reader;
use dotty_core::ast::{AstArena, DefDef, Template, TreeKind, Typed, Untyped};
use dotty_core::types::{Constant, TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SourceTypedIndex, SymbolFlags,
    SymbolId, SymbolInfo, SymbolKind, TreeId, Visibility,
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
const CROSS_CLASS_TASTY_SOURCE: &str = include_str!("fixtures/jvm_tasty_cross_class/Reader.scala");
const FACTORY_CROSS_CLASS_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_cross_class_factories/CounterFactory.scala");
const IDENTITY_CROSS_CLASS_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_cross_class_factories/CounterIdentitySource.scala");
const TASTY_OBJECT_COMPOSITION_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_object_composition/CounterOwner.scala");
const TASTY_CONSTRUCTOR_INJECTION_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_constructor_injection/CounterOwner.scala");
const TASTY_NESTED_CONSTRUCTOR_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_nested_constructor_calls/NestedConstructorCalls.scala");
const FACTORY_RESULT_ARGUMENT_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_factory_result_arguments/CounterOwnerFactory.scala");
const REFERENCE_LOCAL_CONSTRUCTOR_FLOW_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_reference_local_constructor_flows/CounterOwnerFactory.scala");
const MULTI_ARGUMENT_FACTORY_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactory.scala");

type SourceBatchObserver<'observer> =
    dyn for<'a> FnMut(&mut JvmCompilationUnit<'a>, &Definitions) + 'observer;
type SourceSemanticObserver<'observer> = dyn for<'a> FnMut(
        &AstArena<Untyped>,
        TreeId<Untyped>,
        &SourceSemanticIndex,
        &SourceTypedIndex,
        &mut JvmCompilationUnit<'a>,
        &Definitions,
    ) + 'observer;
type SourceTyperObserver<'observer> = dyn for<'a> FnMut(
        &AstArena<Untyped>,
        TreeId<Untyped>,
        &SourceSemanticIndex,
        &mut SourceTyper<'a>,
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
    typer: Option<&'observer mut SourceTyperObserver<'observer>>,
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

fn compile_source_batch_with_typer_and_source_observer<'observer>(
    source_text: &str,
    source_file_name: &str,
    output_directory: &Path,
    source_id: SourceId,
    cardinality: SourceUnitCardinality,
    observe_typer: &'observer mut SourceTyperObserver<'observer>,
    observe_source: &'observer mut SourceSemanticObserver<'observer>,
) -> Result<Vec<EmittedArtifact>, PipelineFailure> {
    compile_source_batch_with_typer_failure_observer(
        source_text,
        source_file_name,
        output_directory,
        source_id,
        cardinality,
        SourceBatchObservers {
            source: Some(observe_source),
            typer: Some(observe_typer),
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
        typer: mut observe_typer,
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

    if let Some(observer) = observe_typer.as_mut() {
        observer(&parsed.ast, parsed.root, &index, &mut typer, &definitions);
    }
    let mut unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: plans,
    };
    if let Some(observe) = observe_source.as_mut() {
        observe(
            &parsed.ast,
            parsed.root,
            &index,
            typer.source_typed_index(),
            &mut unit,
            &definitions,
        );
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
        &mut |_, _, _, _, unit, definitions| {
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
                       _: TreeId<Untyped>,
                       _: &SourceSemanticIndex,
                       _: &SourceTypedIndex,
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
        &mut |_, _, _, _, unit, definitions| {
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
                       _: TreeId<Untyped>,
                       index: &SourceSemanticIndex,
                       _: &SourceTypedIndex,
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
        &mut |_, _, _, _, unit, definitions| {
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
        &mut |_, _, _, _, unit, definitions| {
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
        &mut |_, _, _, _, unit, definitions| {
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

#[test]
fn exact_tasty_object_composition_fixture_emits_reference_field_and_executes() {
    let output = TestOutputDirectory::new("tasty-object-composition");
    let artifacts = compile_source_classes(
        TASTY_OBJECT_COMPOSITION_SOURCE,
        "CounterOwner.scala",
        output.path(),
    )
    .expect("exact issue fixture compiles through the real source-to-JVM pipeline");
    assert_eq!(artifacts.len(), 2);

    let owner_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "example/CounterOwner")
        .expect("CounterOwner is emitted");
    let owner = decode_emitted_class(owner_artifact);
    assert_eq!((owner.version.major, owner.version.minor), (65, 0));
    let [inner] = owner.fields.as_slice() else {
        panic!("CounterOwner has one physical source field");
    };
    assert_eq!(owner.constant_pool.utf8(inner.name_index), Ok("inner"));
    assert_eq!(
        owner.constant_pool.utf8(inner.descriptor_index),
        Ok("Lexample/Counter;")
    );
    assert!(inner.access_flags.is_private());
    assert!(inner.access_flags.is_final());

    assert_opcode_sequence(
        emitted_method_code(&owner, "<init>"),
        &[0x2a, 0xb7, 0x2a, 0xbb, 0x59, 0x1b, 0xb7, 0xb5, 0xb1],
    );
    assert_invocation_sequence(
        &owner,
        emitted_method_code(&owner, "<init>"),
        &[
            (0xb7, "java/lang/Object", "<init>", "()V"),
            (0xb7, "example/Counter", "<init>", "(I)V"),
        ],
    );
    assert_field_reference(
        &owner,
        emitted_method_code(&owner, "<init>"),
        0xb5,
        "example/CounterOwner",
        "inner",
        "Lexample/Counter;",
    );
    assert_eq!(code_max_stack(&owner, "<init>"), 4);

    assert_opcode_sequence(
        emitted_method_code(&owner, "read"),
        &[0x2a, 0xb4, 0xb6, 0xac],
    );
    assert_field_reference(
        &owner,
        emitted_method_code(&owner, "read"),
        0xb4,
        "example/CounterOwner",
        "inner",
        "Lexample/Counter;",
    );
    assert_invocation_sequence(
        &owner,
        emitted_method_code(&owner, "read"),
        &[(0xb6, "example/Counter", "get", "()I")],
    );

    if require_jdk_or_skip() {
        let classpath = output.path().to_str().expect("UTF-8 output path");
        let harness = output.path().join("ObjectCompositionHarness.java");
        fs::write(
            &harness,
            "public final class ObjectCompositionHarness { public static void main(String[] args) { example.CounterOwner first = new example.CounterOwner(7); example.CounterOwner second = new example.CounterOwner(42); if (first.read() != 7 || second.read() != 42) throw new AssertionError(\"reference field object composition\"); } }\n",
        )
        .expect("write exact issue fixture runtime harness");
        let javac = Command::new("javac")
            .args(["--release", "21", "-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac for the object-composition fixture");
        assert!(
            javac.status.success(),
            "javac rejected object-composition harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-ea",
                "-classpath",
                classpath,
                "ObjectCompositionHarness",
            ])
            .output()
            .expect("start Java verifier for the object-composition fixture");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected object-composition harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&java.stdout),
            String::from_utf8_lossy(&java.stderr)
        );
    }
}

#[test]
fn constructor_injected_reference_alias_emits_and_preserves_object_identity() {
    let output = TestOutputDirectory::new("constructor-injected-reference-alias");
    let artifacts = compile_source_classes(
        TASTY_CONSTRUCTOR_INJECTION_SOURCE,
        "CounterOwner.scala",
        output.path(),
    )
    .expect("the pinned constructor-alias audit source compiles through one source batch");
    assert_eq!(artifacts.len(), 2);
    let owner_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "example/CounterOwner")
        .expect("CounterOwner is emitted from the same source unit");
    let owner = decode_emitted_class(owner_artifact);
    assert_eq!((owner.version.major, owner.version.minor), (65, 0));
    let [inner] = owner.fields.as_slice() else {
        panic!("CounterOwner has one physical field");
    };
    assert_eq!(owner.constant_pool.utf8(inner.name_index), Ok("inner"));
    assert_eq!(
        owner.constant_pool.utf8(inner.descriptor_index),
        Ok("Lexample/Counter;")
    );
    assert!(inner.access_flags.is_private());
    assert!(inner.access_flags.is_final());

    let constructor = emitted_method_code(&owner, "<init>");
    assert_opcode_sequence(constructor, &[0x2a, 0xb7, 0x2a, 0x2b, 0xb5, 0xb1]);
    assert_invocation_sequence(
        &owner,
        constructor,
        &[(0xb7, "java/lang/Object", "<init>", "()V")],
    );
    assert_field_reference(
        &owner,
        constructor,
        0xb5,
        "example/CounterOwner",
        "inner",
        "Lexample/Counter;",
    );
    assert_opcode_sequence(
        emitted_method_code(&owner, "read"),
        &[0x2a, 0xb4, 0xb6, 0xac],
    );
    assert_field_reference(
        &owner,
        emitted_method_code(&owner, "read"),
        0xb4,
        "example/CounterOwner",
        "inner",
        "Lexample/Counter;",
    );
    assert_invocation_sequence(
        &owner,
        emitted_method_code(&owner, "read"),
        &[(0xb6, "example/Counter", "get", "()I")],
    );

    if require_jdk_or_skip() {
        let classpath = output.path().to_str().expect("UTF-8 output path");
        let harness = output.path().join("ConstructorAliasHarness.java");
        fs::write(
            &harness,
            r#"public final class ConstructorAliasHarness {
  public static void main(String[] args) throws Exception {
    example.Counter supplied = new example.Counter(7);
    example.CounterOwner owner = new example.CounterOwner(supplied);
    if (owner.read() != 7) throw new AssertionError("initial aliased value");
    supplied.set(42);
    if (owner.read() != 42) throw new AssertionError("owner observes mutation of supplied object");
    supplied.set(-9);
    if (owner.read() != -9) throw new AssertionError("owner retains the same supplied object");
    example.Counter independent = new example.Counter(3);
    example.CounterOwner independentOwner = new example.CounterOwner(independent);
    supplied.set(10);
    if (independentOwner.read() != 3) throw new AssertionError("separate owner stays independent");
    var field = example.CounterOwner.class.getDeclaredField("inner");
    field.setAccessible(true);
    if (field.get(owner) != supplied) throw new AssertionError("field is the identical supplied object");
  }
}
"#,
        )
        .expect("write constructor-alias runtime harness");
        let javac = Command::new("javac")
            .args(["--release", "21", "-classpath", classpath, "-d", classpath])
            .arg(&harness)
            .output()
            .expect("start javac for constructor-alias harness");
        assert!(
            javac.status.success(),
            "javac rejected constructor-alias harness\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&javac.stdout),
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = Command::new("java")
            .args([
                "-Xverify:all",
                "-ea",
                "-classpath",
                classpath,
                "ConstructorAliasHarness",
            ])
            .output()
            .expect("start JDK verifier for constructor-alias harness");
        assert!(
            java.status.success(),
            "java -Xverify:all rejected constructor-alias classes\nstdout:\n{}\nstderr:\n{}",
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
ß:×fòµë(š+myÒÒFV6öFUöVÖ—GFVEö6Æ72†f7F÷'•ö'F–f7B“°¢ÆWB7&VFUö6öFRÒVÖ—GFVEöÖWF†öEö6öFR‚ff7F÷'•ö6Æ72Â&7&VFR"“°¢76W'Eö÷6öFU÷6WVVæ6R†7&VFUö6öFRÂe³†&"ÂƒS’Âƒ&Âƒ"Â†#bÂ†#rÂ†#Ò“°¢ÆWB7&VFUö6ÆÇ2ÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†7&VFUö6öFR¢æ–çFõö—FW"‚¢æf–ÇFW%öÖ‡Â†öfg6WBÂ÷6öFR—Â°¢–bÖF6†W2†÷6öFRÂ†#bÂ†#r’°¢&WGW&âæöæS°¢Ğ¢ÆWB–æFW‚ÒSc£¦g&öÕö&Uö'—FW2…¶7&VFUö6öFU¶öfg6WB²ÒÂ7&VFUö6öFU¶öfg6WB²%ÕÒ“°¢ÆWB6öç7FçEööÄVçG'“£¤ÖWF†öG&Vb°¢6Æ75ö–æFW‚À¢æÖUöæE÷G—Uö–æFW‚À¢ÒÒf7F÷'•ö6Æ72æ6öç7FçE÷ööÂævWB„6öç7FçEööÄ–æFW‚†–æFW‚’“ğ¢VÇ6R°¢æ–2‚&7&VFR6ÆÇ2W6RÖWF†öG&VbVçG&–W2"“°¢Ó°¢6öÖR‚€¢÷6öFRÀ¢f7F÷'•ö6Æ70¢æ6öç7FçE÷ööÀ¢æ6Æ75öæÖR‚¦6Æ75ö–æFW‚¢çVçw&‚’À¢f7F÷'•ö6Æ70¢æ6öç7FçE÷ööÀ¢ææÖUöæE÷G—R‚¦æÖUöæE÷G—Uö–æFW‚¢çVçw&‚’À¢’¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW€¢7&VFUö6ÆÇ2À¢fV2°¢€¢†#bÀ¢&W†×ÆRô6÷VçFW$÷væW$f7F÷'’"À¢‚&Ö¶T6÷VçFW""Â"„’”ÆW†×ÆRô6÷VçFW#²"¢’À¢€¢†#rÀ¢&W†×ÆRô6÷VçFW$÷væW""À¢‚#Æ–æ—Câ"Â"„ÆW†×ÆRô6÷VçFW#²•b"¢¢Ğ¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚$f7F÷'•&W7VÇD&wVÖVçG4†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ72f7F÷'•&W7VÇD&wVÖVçG4†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²W†×ÆRä6÷VçFW$÷væW$f7F÷'’f7F÷'’ÒæWrW†×ÆRä6÷VçFW$÷væW$f7F÷'’‚“²–b†f7F÷'’æ7&VFRƒr’ç&VB‚’Òr’F‡&÷ræWr76W'F–öäW'&÷"…Â&7&VFRƒr•Â"“²–b†f7F÷'’æ7&VFR‚Ó’’ç&VB‚’ÒÓ’’F‡&÷ræWr76W'F–öäW'&÷"…Â&7&VFR‚Ó’•Â"“²W†×ÆRä6÷VçFW"f—'7BÒf7F÷'’æÖ¶T6÷VçFW"ƒr“²W†×ÆRä6÷VçFW"6V6öæBÒf7F÷'’æÖ¶T6÷VçFW"ƒr“²–b†f—'7BÓÒ6V6öæB’F‡&÷ræWr76W'F–öäW'&÷"…Â&f7F÷'’&WW6W2×WF&ÆR7FFUÂ"“²f—'7Bç6WBƒC“²–b†f—'7BævWB‚’ÒCÇÂ6V6öæBævWB‚’Òr’F‡&÷ræWr76W'F–öäW'&÷"…Â$6÷VçFW"7FFR—6öÆF–öåÂ"“²ÒÕÆâ"À¢¢çVçw&‚“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWE÷F‚Â"ÖB"Â÷WGWE÷F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2f÷"F†R÷&–v–æÂ×6÷W&6R¥dÒ†&æW72"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVBF†R÷&–v–æÂ×6÷W&6R†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢$f7F÷'•&W7VÇD&wVÖVçG4†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W"f÷"÷&–v–æÂ×6÷W&6R¥dÒ÷WGWB"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVB÷&–v–æÂ×6÷W&6R¥dÒ÷WGWEÆç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ&VfW&Væ6UöÆö6Åö6öç7G'V7F÷%öfÆ÷uöVÖ—G5öæEöW†V7WFW5ööå÷F†Uö§fÕööæÇ•÷F‚‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&VfW&Væ6RÖÆö6ÂÖ6öç7G'V7F÷"ÖfÆ÷rÖ§fÒÖöæÇ’"“°¢ÆWB'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö&F6‚€¢$TdU$Tä4UôÄô4Åô4ôå5E%T5Dõ%ôdÄõuõ4õU$4RÀ¢$6÷VçFW$÷væW$f7F÷'’ç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒƒC“’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢¢æW‡V7B‚'F†R¥dÒÖöæÇ’&6¶VæB66WG2F†R÷&–v–æÂ&Æö6²ÖÆö6Â6÷W&6R"“°¢76W'EöW€¢'F–f7G0¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢°¢&W†×ÆRô6÷VçFW""À¢&W†×ÆRô6÷VçFW$÷væW""À¢&W†×ÆRô6÷VçFW$÷væW$f7F÷'’ ¢Ğ¢“°¢76W'EöW†÷WGWBæ6Æ75öf–ÆW2‚’æÆVâ‚’Â2“°¢ÆWBf7F÷'’ÒFV6öFUöVÖ—GFVEö6Æ75öæÖVB‚f'F–f7G2Â&W†×ÆRô6÷VçFW$÷væW$f7F÷'’"“°¢ÆWBf7F÷'•öÖWF†öBÒf7F÷'¢æÖWF†öG0¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂf7F÷'’æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’ÓÒö²‚&7&VFR"’¢æW‡V7B‚$f7F÷'’æ7&VFR—2VÖ—GFVB"“°¢76W'EöW€¢f7F÷'’æ6öç7FçE÷ööÂçWFc‚†f7F÷'•öÖWF†öBæFW67&—F÷%ö–æFW‚’À¢ö²‚"„’”ÆW†×ÆRô6÷VçFW$÷væW#²"¢“°¢ÆWB´GG&–'WFS£¤6öFR†6öFR•ÒÒf7F÷'•öÖWF†öBæGG&–'WFW2æ5÷6Æ–6R‚’VÇ6R°¢æ–2‚$f7F÷'’æ7&VFR†2öæR6öFRGG&–'WFR"“°¢Ó°¢76W'EöW†6öFRæÖ…÷7F6²Â2“°¢76W'EöW†6öFRæÖ…öÆö6Ç2Â2“°¢76W'Eö÷6öFU÷6WVVæ6R€¢6öFRæ6öFRÀ¢e³ƒ&Âƒ"Â†#bÂƒFBÂ†&"ÂƒS’Âƒ&2Â†#rÂ†#ÒÀ¢“°¢76W'EöÖWF†öE÷&VfW&Væ6R€¢ff7F÷'’À¢6öFRæ6öFRÀ¢†#bÀ¢&W†×ÆRô6÷VçFW$÷væW$f7F÷'’"À¢&Ö¶T6÷VçFW""À¢"„’”ÆW†×ÆRô6÷VçFW#²"À¢“°¢76W'EöÖWF†öE÷&VfW&Væ6R€¢ff7F÷'’À¢6öFRæ6öFRÀ¢†#rÀ¢&W†×ÆRô6÷VçFW$÷væW""À¢#Æ–æ—Câ"À¢"„ÆW†×ÆRô6÷VçFW#²•b"À¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚%&VfW&Væ6TÆö6ÄfÆ÷t†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'6¶vRW†×ÆS²V&Æ–2f–æÂ6Æ72&VfW&Væ6TÆö6ÄfÆ÷t†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²6÷VçFW$÷væW$f7F÷'’f7F÷'’ÒæWr6÷VçFW$÷væW$f7F÷'’‚“²–b†f7F÷'’æ7&VFRƒr’ç&VB‚’Òr’F‡&÷ræWr76W'F–öäW'&÷"…Â&7&VFRƒr•Â"“²–b†f7F÷'’æ7&VFR‚Ó’’ç&VB‚’ÒÓ’’F‡&÷ræWr76W'F–öäW'&÷"…Â&7&VFR‚Ó’•Â"“²–b†f7F÷'’æ7&VFRƒ2’ç&VB‚’Ò2’F‡&÷ræWr76W'F–öäW'&÷"…Â&7&VFRƒ2•Â"“²6÷VçFW"f—'7BÒf7F÷'’æÖ¶T6÷VçFW"ƒr“²6÷VçFW"6V6öæBÒf7F÷'’æÖ¶T6÷VçFW"ƒr“²–b†f—'7BÓÒ6V6öæB’F‡&÷ræWr76W'F–öäW'&÷"…Â&f7F÷'’&WW6VB&VfW&Væ6UÂ"“²f—'7Bç6WBƒC“²–b†f—'7BævWB‚’ÒCÇÂ6V6öæBævWB‚’Òr’F‡&÷ræWr76W'F–öäW'&÷"…Â&–æFWVæFVçB7FFUÂ"“²ÒÕÆâ"À¢¢çVçw&‚“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…°¢"Ò×&VÆV6R"À¢##"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢"ÖB"À¢÷WGWE÷F‚À¢Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¤D³#¦f2f÷"F†RÆö6ÂfÆ÷r†&æW72"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVBF†RÆö6ÂfÆ÷r†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢&W†×ÆRå&VfW&Væ6TÆö6ÄfÆ÷t†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¤D³#fW&–f–6F–öâf÷"'W7B¥dÒ÷WGWB"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢$¤D³#&V¦V7FVBÆö6ÂfÆ÷r÷WGWEÆç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâæW7FVEö6öç7G'V7F÷%ö–FVçF—G•÷&ööeö–væ÷&W5ö6Æ75÷6¶vUöæEöFV6Æ&F–öåö÷&FW"‚’°¢ÆWB&VæÖVBÒD5E•ôäU5DTEô4ôå5E%T5Dõ%õ4õU$4P¢ç&WÆ6R‚'6¶vRW†×ÆR"Â'6¶vR&VæÖVB"¢ç&WÆ6R‚$6÷VçFW$÷væW$f7F÷'’"Â%v–FvWD'V–ÆFW""¢ç&WÆ6R‚$6÷VçFW$÷væW""Â%v–FvWD†öÆFW""¢ç&WÆ6R‚$6÷VçFW""Â%v–FvWB"¢ç&WÆ6R‚&7&VFR"Â&'V–ÆB"“°¢ÆWB6¶vUöVæBÒ&VæÖVBæf–æB‚%ÆåÆâ"’çVçw&‚’²#°¢ÆWB6¶vRÒg&VæÖVE²âç6¶vUöVæEÓ°¢ÆWB6Æ76W2Òg&VæÖVE·6¶vUöVæBâåÓ°¢ÆWB÷væW%÷7F'BÒ6Æ76W2æf–æB‚&6Æ72v–FvWD†öÆFW"‚"’çVçw&‚“°¢ÆWBf7F÷'•÷7F'BÒ6Æ76W2æf–æB‚&6Æ72v–FvWD'V–ÆFW#¢"’çVçw&‚“°¢ÆWB6÷VçFW"Òf6Æ76W5²âæ÷væW%÷7F'EÓ°¢ÆWB÷væW"Òf6Æ76W5¶÷væW%÷7F'Bâæf7F÷'•÷7F'EÓ°¢ÆWBf7F÷'’Òf6Æ76W5¶f7F÷'•÷7F'BâåÓ°¢ÆWB&V÷&FW&VBÒf÷&ÖB‚'·6¶vW×¶÷væW'×¶6÷VçFW'×¶f7F÷'—Ò"“° ¢f÷"‡6÷W&6RÂ6÷W&6Uö–B’–â²‚g&VæÖVBÂƒc"’Â‚g&V÷&FW&VBÂƒc2•Ò°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&æW7FVBÖ6öç7G'V7F÷"×&VæÖVB×&ööb"“°¢ÆWB×WBö'6W'fVBÒfÇ6S°¢ÆWB×WBö'6W'fRÒÆ7C¢d7D&VæÅVçG—VCâÀ¢6÷W&6U÷&ö÷C¢G&VT–CÅVçG—VCâÀ¢6÷W&6Uö–æFWƒ¢e6÷W&6U6VÖçF–4–æFW‚À¢6÷W&6U÷G—VEö–æFWƒ¢e6÷W&6UG—VD–æFW‚À¢Væ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÀ¢FVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ö'6W'fVBÒG'VS°¢ÆWB&öög2Ğ¢fÆ–FFUö7&÷75ö6Æ75ö6öç7G'V7F÷%ö6ÆÅ÷&öög2„7&÷746Æ746öç7G'V7F÷$6ÆÅ&ööd–çWB°¢6÷W&6Uö–C¢6÷W&6T–C£¦g&öÕö–æFW‚‡6÷W&6Uö–B’À¢6÷W&6U÷&ö÷BÀ¢7F÷&S¢Væ—Bç7F÷&RÀ¢FVf–æ—F–öç2À¢6÷W&6Uö7C¢7BÀ¢G—VEö7C¢Væ—BçG—VEö7BÀ¢6÷W&6Uö–æFW‚À¢6÷W&6U÷G—VEö–æFW‚À¢§fÕ÷Væ—C¢Væ—BÀ¢Ò¢æW‡V7B‚'&VæÖVB÷&V÷&FW&VB6÷W&6R&VÖ–ç27G'V7GW&ÆÇ’&÷f&ÆR"“°¢76W'EöW‡&öög2æÆVâ‚’Â“°¢76W'B‡&öög5³ÒææW7FVEö&wVÖVçB‚’æ—5÷6öÖR‚’“°¢76W'EöW‡&öög5³ÒæÖWF†öEöFW67&—F÷"‚’Â"„’”Ç&VæÖVBõv–FvWD†öÆFW#²"“°¢Ó°¢6ö×–ÆU÷6÷W&6Uö&F6…÷v—F…÷6÷W&6Uöö'6W'fW"€¢6÷W&6RÀ¢%&VæÖVDæW7FVD6öç7G'V7F÷$6ÆÇ2ç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚‡6÷W&6Uö–B’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢f×WBö'6W'fRÀ¢¢æW‡V7B‚'&VæÖVB÷&V÷&FW&VB6÷W&6R&VÖ–ç2fÆ–Böâ¥dÒÖöæÇ’F‚"“°¢76W'B†ö'6W'fVB“°¢Ğ§Ğ ¢5·FW7EĞ¦fâæW7FVEö6öç7G'V7F÷%ö–FVçF—G•÷&ööe÷&V¦V7G5öö&ööÆVåöf7F÷'•öÆVb‚’°¢ÆWB6÷W&6RÒ'6¶vRW†×ÆUÆæ6Æ726÷VçFW"‡6VVC¢&ööÆVâ•Ææ6Æ726÷VçFW$÷væW"†6÷VçFW#¢6÷VçFW"•Ææ6Æ726÷VçFW$÷væW$f7F÷'“¥ÆâFVb7&VFR‡6VVC¢&ööÆVâ“¢6÷VçFW$÷væW"ÕÆâæWr6÷VçFW$÷væW"†æWr6÷VçFW"‡6VVB’•Æâ#°¢ÆWB6÷W&6Uö–BÒ6÷W&6T–C£¦g&öÕö–æFW‚ƒƒcb“°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&æW7FVBÖ6öç7G'V7F÷"Ö&ööÆVâÖÆVb"“°¢ÆWB×WBö'6W'fVBÒfÇ6S°¢ÆWB×WB&ööeöW'&÷"ÒæöæS°¢ÆWB×WBö'6W'fRÒÆ7C¢d7D&VæÅVçG—VCâÀ¢6÷W&6U÷&ö÷C¢G&VT–CÅVçG—VCâÀ¢6÷W&6Uö–æFWƒ¢e6÷W&6U6VÖçF–4–æFW‚À¢6÷W&6U÷G—VEö–æFWƒ¢e6÷W&6UG—VD–æFW‚À¢Væ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÀ¢FVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ö'6W'fVBÒG'VS°¢&ööeöW'&÷"Ò6öÖR€¢ÖF6‚fÆ–FFUö7&÷75ö6Æ75ö6öç7G'V7F÷%ö6ÆÅ÷&öög2€¢7&÷746Æ746öç7G'V7F÷$6ÆÅ&ööd–çWB°¢6÷W&6Uö–BÀ¢6÷W&6U÷&ö÷BÀ¢7F÷&S¢Væ—Bç7F÷&RÀ¢FVf–æ—F–öç2À¢6÷W&6Uö7C¢7BÀ¢G—VEö7C¢Væ—BçG—VEö7BÀ¢6÷W&6Uö–æFW‚À¢6÷W&6U÷G—VEö–æFW‚À¢§fÕ÷Væ—C¢Væ—BÀ¢ÒÀ¢’°¢W'"†W'&÷"’ÓâW'&÷"À¢ö²…ò’Óâæ–2‚&æW7FVB6öç7G'V7F÷"&öög2&WV—&RF†R6æöæ–6Â–çBG—R"’À¢ÒÀ¢“°¢Ó°¢6ö×–ÆU÷6÷W&6Uö&F6…÷v—F…÷6÷W&6Uöö'6W'fW"€¢6÷W&6RÀ¢$&ööÆVäæW7FVD6öç7G'V7F÷$6ÆÇ2ç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6Uö–BÀ¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢f×WBö'6W'fRÀ¢¢æW‡V7B‚'F†R&ööÆVâ6÷W&6R—2fÆ–Bf÷"F†R&VÂg&öçFVæBæB¥dÒÖöæÇ’F‚"“°¢76W'B†ö'6W'fVBÂ'F†R&ööb&â–âF†R&VÂg&öçFVæB6W76–öâ"“°¢76W'B†ÖF6†W2€¢&ööeöW'&÷"À¢6öÖR€¢F÷GG“£¦§fÕ÷F7G•ö7&÷75ö6Æ73£¤7&÷746Æ746öç7G'V7F÷$6ÆÅ&öödW'&÷#£¥6VÖçF–4–FVçF—G’€¢&æW7FVB6öç7G'V7F÷"æBf7F÷'’f÷&ÖÂ×W7B&÷F‚†fRF†R6æöæ–6Â–çBG—R ¢¢¢’“°§Ğ ¢5·FW7EĞ¦fâÆÆö6FVEö–Ö×WF&ÆU÷&VfW&Væ6UöÆö6Åö6åö&Uöö6öç7G'V7F÷%ö&wVÖVçB‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&ÆÆö6FVB×&VfW&Væ6RÖ6öç7G'V7F÷"Ö&wVÖVçB"“°¢ÆWB'F–f7G2Ò6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R€¢&6Æ726÷VçFW"‡6VVC¢–çB•Ææ6Æ72†öÆFW"†6÷VçFW#¢6÷VçFW"•Ææ6Æ726Æ–VçC¥ÆâFVbÆö6Â‚“¢†öÆFW"ÕÆâfÂ2ÒæWr6÷VçFW"ƒ•ÆâæWr†öÆFW"†2•Æâ"À¢÷WGWBçF‚‚’À¢¢çVçw&‚“°¢ÆWB6Æ–VçBÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$6Æ–VçB"¢çVçw&‚“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†6Æ–VçBæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFRvVæW&FVB6Æ–VçB6Æ72"“°¢ÆWBÆö6ÂÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â&Æö6Â"“°¢76W'Eö÷6öFU÷6WVVæ6R€¢Æö6ÂÀ¢e³†&"ÂƒS’ÂƒBÂ†#rÂƒF2Â†&"ÂƒS’Âƒ&"Â†#rÂ†#ÒÀ¢“°¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚$ÆÆö6FVDÆö6Ä†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ72ÆÆö6FVDÆö6Ä†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²–b†æWr6Æ–VçB‚’æÆö6Â‚’ÓÒçVÆÂ’F‡&÷ræWr76W'F–öäW'&÷"…Â&ÆÆö6FVBÆö6Â6öç7G'V7F÷"&wVÖVçEÂ"“²ÒÕÆâ"À¢¢çVçw&‚“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWE÷F‚Â"ÖB"Â÷WGWE÷F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢çVçw&‚“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVBÆÆö6FVBÖÆö6Â6öç7G'V7F÷"&wVÖVçB†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢$ÆÆö6FVDÆö6Ä†&æW72"À¢Ò¢æ÷WGWB‚¢çVçw&‚“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVBÆÆö6FVBÖÆö6Â6öç7G'V7F÷"&wVÖVçEÆç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâG—VEöæWuö&WGvVVå÷Gvõ÷ÆææVE÷6÷W&6Uö6Æ76W5÷fW&–f–W5ööå÷F†Uö§fÒ‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'GvòÖ6Æ72Ö6öç7G'V7F÷""“°¢ÆWB'F–f7G2Ò6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R€¢&6Æ726÷VçFW"‡6VVC¢–çB•Ææ6Æ726Æ–VçC¥ÆâFVb'V–ÆB‡6VVC¢–çB“¢–çBÕÆâæWr6÷VçFW"‡6VVB²•ÆâC%Æâ"À¢÷WGWBçF‚‚’À¢¢çVçw&‚“°¢76W'EöW†'F–f7G2æÆVâ‚’Â"“°¢76W'EöW€¢'F–f7G0¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢²$6÷VçFW""Â$6Æ–VçB%Ğ¢“°¢ÆWB6Æ–VçBÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$6Æ–VçB"¢çVçw&‚“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†6Æ–VçBæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFRÆææVB6Æ–VçB6Æ72"“°¢6Æ70¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚'fÆ–FFR6Æ–VçB6öç7FçBööÂ&VfW&Væ6W2"“°¢ÆWB6öFRÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â&'V–ÆB"“°¢76W'Eö÷6öFU÷6WVVæ6R€¢6öFRÀ¢e³†&"ÂƒS’Âƒ"ÂƒBÂƒcÂ†#rÂƒSrÂƒÂ†5ÒÀ¢“°¢76W'B†6öFU¶6öFRæÆVâ‚’Ò%ÒÓÒC"“°¢ÆWBÆÆö6F–öåö–æFW‚ÒSc£¦g&öÕö&Uö'—FW2…¶6öFU³ÒÂ6öFU³%ÕÒ“°¢ÆWB6öç7FçEööÄVçG'“£¤6Æ72²æÖUö–æFW‚ÒÒ6Æ70¢æ6öç7FçE÷ööÀ¢ævWB„6öç7FçEööÄ–æFW‚†ÆÆö6F–öåö–æFW‚’¢çVçw&‚¢VÇ6R°¢æ–2‚&æWr×W7B&VfW&Væ6RF†RÆææVB6÷VçFW"6Æ72"¢Ó°¢76W'EöW†6Æ72æ6öç7FçE÷ööÂçWFc‚‚¦æÖUö–æFW‚’Âö²‚$6÷VçFW""’“°¢ÆWB†–çfö¶Uööfg6WBÂò’ÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ–çFõö—FW"‚¢æf–æB‡Â…òÂ÷6öFR—Â¦÷6öFRÓÒ†#r¢æW‡V7B‚&6öç7G'V7F÷"W‡&W76–öâ6öçF–ç2–çfö¶W7V6–Â"“°¢ÆWB–çfö¶Uö–æFW‚ÒSc£¦g&öÕö&Uö'—FW2…¶6öFU¶–çfö¶Uööfg6WB²ÒÂ6öFU¶–çfö¶Uööfg6WB²%ÕÒ“°¢ÆWB6öç7FçEööÄVçG'“£¤ÖWF†öG&Vb°¢6Æ75ö–æFW‚À¢æÖUöæE÷G—Uö–æFW‚À¢ÒÒ6Æ70¢æ6öç7FçE÷ööÀ¢ævWB„6öç7FçEööÄ–æFW‚†–çfö¶Uö–æFW‚’¢çVçw&‚¢VÇ6R°¢æ–2‚&–çfö¶W7V6–Â×W7B&VfW&Væ6RF†RÆææVB6öç7G'V7F÷""¢Ó°¢76W'EöW†6Æ72æ6öç7FçE÷ööÂæ6Æ75öæÖR‚¦6Æ75ö–æFW‚’Âö²‚$6÷VçFW""’“°¢76W'EöW€¢6Æ72æ6öç7FçE÷ööÂææÖUöæE÷G—R‚¦æÖUöæE÷G—Uö–æFW‚’À¢ö²‚‚#Æ–æ—Câ"Â"„’•b"’¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚$6öç7G'V7F÷$†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–26Æ726öç7G'V7F÷$†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²7—7FVÒæ÷WBç&–çFÆâ†æWr6Æ–VçB‚’æ'V–ÆBƒ’“²ÒÕÆâ"À¢¢çVçw&‚“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWE÷F‚Â"ÖB"Â÷WGWE÷F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢çVçw&‚“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVBGvòÖ6Æ726Öö¶R†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢$6öç7G'V7F÷$†&æW72"À¢Ò¢æ÷WGWB‚¢çVçw&‚“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVBGvòÆææVB6Æ76W5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW…7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’Â#C""“°¢Ğ§Ğ ¢5·FW7EĞ¦fâf—'GVÅö6ÆÇ5÷W6U÷fÆ–FFVEöæWuöæE÷ÆææVEöÆö6Å÷&V6V—fW'2‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'ÆææVB×f—'GVÂÖ6ÆÇ2"“°¢ÆWB'F–f7G2Ò6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R€¢&6Æ726÷VçFW"‡6VVC¢–çB“¥ÆâFVbFB‡fÇVS¢–çB“¢–çBÒfÇVR²ÆâFVb6öÖ&–æR†ÆVgC¢–çBÂ&–v‡C¢–çB“¢–çBÒÆVgB¢²&–v‡EÆâFVbvWB‚“¢–çBÒC%ÆâFVbVæ&ÆVB‚“¢&ööÆVâÒG'VUÆâFVbF÷V6‚‚“¢Væ—BÒ‚•Æâ&—fFRFVb†–FFVâ‚“¢–çBÒ•ÆâFVb&—fFT6ÆÂ‚“¢–çBÒF†—2æ†–FFVâ‚•ÆâFVb&—fFU&V6V—fW"‡6VVC¢–çB“¢–çBÒæWr6÷VçFW"‡6VVB’æ†–FFVâ‚•Ææ6Æ72÷F†W"‡6VVC¢–çB“¥ÆâFVbvWB‚“¢–çBÒuÆæ6Æ726Æ–VçC¥ÆâFVbF—&V7DvWB‡6VVC¢–çB“¢–çBÒæWr6÷VçFW"‡6VVB’ævWB‚•ÆâFVbg&öÔæWr‡6VVC¢–çB“¢–çBÒæWr6÷VçFW"‡6VVB²’æFB‡6VVB•ÆâFVb÷&FW&VB‡6VVC¢–çB“¢–çBÒæWr6÷VçFW"‡6VVB’æ6öÖ&–æR‡6VVB²Â6VVB²"•ÆâFVb&ööÆVå&W7VÇB‡6VVC¢–çB“¢&ööÆVâÒæWr6÷VçFW"‡6VVB’æVæ&ÆVB‚•ÆâFVbVæ—D6ÆÂ‡6VVC¢–çB“¢Væ—BÒæWr6÷VçFW"‡6VVB’çF÷V6‚‚•ÆâFVbg&öÔÆö6Â‡6VVC¢–çB“¢–çBÕÆâfÂ6÷VçFW"ÒæWr6÷VçFW"‡6VVB•Æâ6÷VçFW"æFB‡6VVB•ÆâFVbg&öÔ×WF&ÆTÆö6Â‡6VVC¢–çB“¢–çBÕÆâf"6÷VçFW#¢6÷VçFW"ÒæWr6÷VçFW"‡6VVB•Æâ6÷VçFW"æFB‡6VVB•ÆâFVbg&öÔ÷F†W"‡6VVC¢–çB“¢–çBÒæWr÷F†W"‡6VVB’ævWB‚•Æâ"À¢÷WGWBçF‚‚’À¢¢çVçw&‚“°¢ÆWBFV6öFUö6Æ72ÒÆæÖS¢g7G'Â°¢ÆWB'F–f7BÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒæÖR¢çVçw&ö÷%öVÇ6R‡ÇÂæ–2‚'¶æÖWÒ6Æ72—2VÖ—GFVB"’“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†'F–f7Bæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFRVÖ—GFVB6Æ72"“°¢6Æ72çfÆ–FFU÷&VfW&Væ6W2‚’æW‡V7B‚'fÆ–B6Æ72&VfW&Væ6W2"“°¢6Æ70¢Ó°¢ÆWB6Æ–VçBÒFV6öFUö6Æ72‚$6Æ–VçB"“°¢ÆWBF—&V7EövWBÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ–VçBÂ&F—&V7DvWB"“°¢76W'Eö÷6öFU÷6WVVæ6R†F—&V7EövWBÂe³†&"ÂƒS’Âƒ"Â†#rÂ†#bÂ†5Ò“°¢ÆWBg&öÕöæWrÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ–VçBÂ&g&öÔæWr"“°¢76W'Eö÷6öFU÷6WVVæ6R€¢g&öÕöæWrÀ¢e³†&"ÂƒS’Âƒ"ÂƒBÂƒcÂ†#rÂƒ"Â†#bÂ†5ÒÀ¢“°¢ÆWB÷&FW&VBÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ–VçBÂ&÷&FW&VB"“°¢76W'Eö÷6öFU÷6WVVæ6R€¢÷&FW&VBÀ¢e°¢†&"ÂƒS’Âƒ"Â†#rÂƒ"ÂƒBÂƒcÂƒ"ÂƒRÂƒcÂ†#bÂ†2À¢ÒÀ¢“°¢ÆWB&ööÆVå÷&W7VÇBÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ–VçBÂ&&ööÆVå&W7VÇB"“°¢76W'Eö÷6öFU÷6WVVæ6R†&ööÆVå÷&W7VÇBÂe³†&"ÂƒS’Âƒ"Â†#rÂ†#bÂ†5Ò“°¢ÆWBVæ—Eö6ÆÂÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ–VçBÂ'Væ—D6ÆÂ"“°¢76W'Eö÷6öFU÷6WVVæ6R‡Væ—Eö6ÆÂÂe³†&"ÂƒS’Âƒ"Â†#rÂ†#bÂ†#Ò“°¢ÆWBg&öÕöÆö6ÂÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ–VçBÂ&g&öÔÆö6Â"“°¢76W'Eö÷6öFU÷6WVVæ6R€¢g&öÕöÆö6ÂÀ¢e³†&"ÂƒS’Âƒ"Â†#rÂƒFBÂƒ&2Âƒ"Â†#bÂ†5ÒÀ¢“°¢ÆWBg&öÕö×WF&ÆUöÆö6ÂÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ–VçBÂ&g&öÔ×WF&ÆTÆö6Â"“°¢76W'Eö÷6öFU÷6WVVæ6R€¢g&öÕö×WF&ÆUöÆö6ÂÀ¢e³†&"ÂƒS’Âƒ"Â†#rÂƒFBÂƒ&2Âƒ"Â†#bÂ†5ÒÀ¢“°¢ÆWBg&öÕö÷F†W"ÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ–VçBÂ&g&öÔ÷F†W""“°¢76W'Eö÷6öFU÷6WVVæ6R†g&öÕö÷F†W"Âe³†&"ÂƒS’Âƒ"Â†#rÂ†#bÂ†5Ò“° ¢ÆWB–çfö6F–öåö÷væW"ÒÆ6Æ73¢d6Æ74f–ÆSÂuóâÂ6öFS¢e·S…×Â°¢ÆWBöfg6WBÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ–çFõö—FW"‚¢æf–æEöÖ‡Â†öfg6WBÂ÷6öFR—Â†÷6öFRÓÒ†#b’çF†Vå÷6öÖR†öfg6WB’¢æW‡V7B‚&öæR–çfö¶Wf—'GVÂ–ç7G'V7F–öâ"“°¢ÆWB–æFW‚ÒSc£¦g&öÕö&Uö'—FW2…¶6öFU¶öfg6WB²ÒÂ6öFU¶öfg6WB²%ÕÒ“°¢ÆWB6öç7FçEööÄVçG'“£¤ÖWF†öG&Vb²6Æ75ö–æFW‚ÂââÒÒ6Æ70¢æ6öç7FçE÷ööÀ¢ævWB„6öç7FçEööÄ–æFW‚†–æFW‚’¢æW‡V7B‚&–çfö¶Wf—'GVÂÖWF†öG&Vb"¢VÇ6R°¢æ–2‚&–çfö¶Wf—'GVÂ&VfW&Væ6W2ÖWF†öG&Vb"¢Ó°¢6Æ70¢æ6öç7FçE÷ööÀ¢æ6Æ75öæÖR‚¦6Æ75ö–æFW‚¢çVçw&‚¢çFõö÷væVB‚¢Ó°¢76W'EöW†–çfö6F–öåö÷væW"‚f6Æ–VçBÂg&öÕöæWr’Â$6÷VçFW""“°¢76W'EöW†–çfö6F–öåö÷væW"‚f6Æ–VçBÂ÷&FW&VB’Â$6÷VçFW""“°¢76W'EöW†–çfö6F–öåö÷væW"‚f6Æ–VçBÂ&ööÆVå÷&W7VÇB’Â$6÷VçFW""“°¢76W'EöW†–çfö6F–öåö÷væW"‚f6Æ–VçBÂVæ—Eö6ÆÂ’Â$6÷VçFW""“°¢76W'EöW†–çfö6F–öåö÷væW"‚f6Æ–VçBÂF—&V7EövWB’Â$6÷VçFW""“°¢76W'EöW†–çfö6F–öåö÷væW"‚f6Æ–VçBÂg&öÕöÆö6Â’Â$6÷VçFW""“°¢76W'EöW†–çfö6F–öåö÷væW"‚f6Æ–VçBÂg&öÕö×WF&ÆUöÆö6Â’Â$6÷VçFW""“°¢òò6÷VçFW"ævWBæB÷F†W"ævWB6†&R7VÆÆ–æs²F†R6æöæ–6Â6VÆV7FV@¢òòÖWF†öBæB&V6V—fW"6Æ72×W7B¶VWF†RGvò÷væW'2F—7F–æ7Bà¢76W'EöW†–çfö6F–öåö÷væW"‚f6Æ–VçBÂg&öÕö÷F†W"’Â$÷F†W""“° ¢ÆWB6÷VçFW"ÒFV6öFUö6Æ72‚$6÷VçFW""“°¢ÆWB¶†–FFVåÒÒ6÷VçFW ¢æÖWF†öG0¢æ—FW"‚¢æf–ÇFW"‡ÆÖWF†öGÂ6÷VçFW"æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’ÓÒö²‚&†–FFVâ"’¢æ6öÆÆV7C££ÅfV3Åóãâ‚•²âåĞ¢VÇ6R°¢æ–2‚$6÷VçFW"æ†–FFVâ—2VÖ—GFVB"¢Ó°¢76W'B††–FFVâæ66W75öfÆw2æ—5÷&—fFR‚’“°¢76W'Eö÷6öFU÷6WVVæ6R€¢VÖ—GFVEöÖWF†öEö6öFR‚f6÷VçFW"Â'&—fFT6ÆÂ"’À¢e³ƒ&Â†#bÂ†5ÒÀ¢“°¢ÆWB&—fFU÷&V6V—fW"ÒVÖ—GFVEöÖWF†öEö6öFR‚f6÷VçFW"Â'&—fFU&V6V—fW""“°¢76W'Eö÷6öFU÷6WVVæ6R‡&—fFU÷&V6V—fW"Âe³†&"ÂƒS’Âƒ"Â†#rÂ†#bÂ†5Ò“°¢76W'EöW†–çfö6F–öåö÷væW"‚f6÷VçFW"Â&—fFU÷&V6V—fW"’Â$6÷VçFW""“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚%f—'GVÄ6ÆÄ†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ72f—'GVÄ6ÆÄ†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²6Æ–VçB2ÒæWr6Æ–VçB‚“²–b†2æF—&V7DvWBƒ’ÒC"ÇÂ2æg&öÔæWrƒ2’ÒBÇÂ2æ÷&FW&VBƒ2’ÒCRÇÂ2æ&ööÆVå&W7VÇBƒ2’ÇÂ2æg&öÔÆö6Âƒ2’ÒBÇÂ2æg&öÔ×WF&ÆTÆö6Âƒ2’ÒBÇÂ2æg&öÔ÷F†W"ƒ2’ÒrÇÂæWr6÷VçFW"ƒ’ç&—fFT6ÆÂ‚’Ò’ÇÂæWr6÷VçFW"ƒ’ç&—fFU&V6V—fW"ƒ"’Ò’’F‡&÷ræWr76W'F–öäW'&÷"‚“²2çVæ—D6ÆÂƒ“²7—7FVÒæ÷WBç&–çFÆâ…Â'f—'GVÂ6ÆÇ2öµÂ"“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FR¦ffW&–f–6F–öâ†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWE÷F‚Â"ÖB"Â÷WGWE÷F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVBf—'GVÂÖ6ÆÂ†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢%f—'GVÄ6ÆÄ†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W""“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVBf—'GVÂÖ6ÆÂ6Æ76W5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢'f—'GVÂ6ÆÇ2ö² ¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ–æ†W&—FVE÷f—'GVÅö6ÆÇ5ö¶VW÷F†UöFV6Æ&–æuöÖWF†öG&VeöæEöF—7F6…öG–æÖ–6ÆÇ’‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&–æ†W&—FVB×f—'GVÂÖ6ÆÂ×F&vWG2"“°¢ÆWB6÷W&6RÒ&6Æ72&6R²FVbvWB‚“¢–çBÒ²FVbfÇVR‚“¢–çBÒ²FVbF÷V&ÆVB‚“¢–çBÒfÇVR‚’¢"ÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²÷fW'&–FRFVbfÇVR‚“¢–çBÒ#²FVb&VB‚“¢–çBÒF†—2ævWB‚’ÕÆæ6Æ72'VææW"²FVb&VB†&6S¢&6R“¢–çBÒ&6RæF÷V&ÆVB‚“²FVbf–FW&—fVB†FW&—fVC¢FW&—fVB“¢–çBÒFW&—fVBæF÷V&ÆVB‚“²FVbf÷'v&B†FW&—fVC¢FW&—fVB“¢–çBÒ&VB†FW&—fVB’ÕÆâ#°¢ÆWB'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ$–æ†W&—FVEf—'GVÄ6ÆÇ2ç66Æ"Â÷WGWBçF‚‚’¢æW‡V7B‚&–æ†W&—FVBÖWF†öG2æB7V'G—R&wVÖVçG26ö×–ÆR2f—'GVÂ6ÆÇ2"“° ¢ÆWBFV6öFUö6Æ72ÒÆæÖS¢g7G'Â°¢ÆWB'F–f7BÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒæÖR¢çVçw&ö÷%öVÇ6R‡ÇÂæ–2‚'¶æÖWÒ6Æ72—2VÖ—GFVB"’“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†'F–f7Bæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFRVÖ—GFVB6Æ72"“°¢6Æ72çfÆ–FFU÷&VfW&Væ6W2‚’æW‡V7B‚'fÆ–B6Æ72&VfW&Væ6W2"“°¢6Æ70¢Ó°¢ÆWB76W'Eö–çfö¶Wf—'GVÅö÷væW"Ğ¢Æ6Æ73¢d6Æ74f–ÆSÂuóâÂÖWF†öC¢g7G"Â÷væW#¢g7G"ÂæÖS¢g7G"ÂFW67&—F÷#¢g7G'Â°¢ÆWB6öFRÒVÖ—GFVEöÖWF†öEö6öFR†6Æ72ÂÖWF†öB“°¢ÆWBöfg6WBÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ–çFõö—FW"‚¢æf–æEöÖ‡Â†öfg6WBÂ÷6öFR—Â†÷6öFRÓÒ†#b’çF†Vå÷6öÖR†öfg6WB’¢æW‡V7B‚&ÖWF†öB6öçF–ç2–çfö¶Wf—'GVÂ"“°¢ÆWB–æFW‚ÒSc£¦g&öÕö&Uö'—FW2…¶6öFU¶öfg6WB²ÒÂ6öFU¶öfg6WB²%ÕÒ“°¢ÆWB6öç7FçEööÄVçG'“£¤ÖWF†öG&Vb°¢6Æ75ö–æFW‚À¢æÖUöæE÷G—Uö–æFW‚À¢ÒÒ6Æ70¢æ6öç7FçE÷ööÀ¢ævWB„6öç7FçEööÄ–æFW‚†–æFW‚’¢æW‡V7B‚&–çfö¶Wf—'GVÂÖWF†öG&Vb"¢VÇ6R°¢æ–2‚&–çfö¶Wf—'GVÂ&VfW&Væ6W2ÖWF†öG&Vb"¢Ó°¢76W'EöW†6Æ72æ6öç7FçE÷ööÂæ6Æ75öæÖR‚¦6Æ75ö–æFW‚’Âö²†÷væW"’“°¢76W'EöW€¢6Æ70¢æ6öç7FçE÷ööÀ¢ææÖUöæE÷G—R‚¦æÖUöæE÷G—Uö–æFW‚¢çVçw&‚¢ãÀ¢æÖP¢“°¢76W'EöW€¢6Æ70¢æ6öç7FçE÷ööÀ¢ææÖUöæE÷G—R‚¦æÖUöæE÷G—Uö–æFW‚¢çVçw&‚¢ãÀ¢FW67&—F÷ ¢“°¢Ó° ¢ÆWB&6RÒFV6öFUö6Æ72‚$&6R"“°¢ÆWBFW&—fVBÒFV6öFUö6Æ72‚$FW&—fVB"“°¢ÆWB'VææW"ÒFV6öFUö6Æ72‚%'VææW""“°¢76W'Eö–çfö¶Wf—'GVÅö÷væW"‚f&6RÂ&F÷V&ÆVB"Â$&6R"Â'fÇVR"Â"‚”’"“°¢76W'Eö–çfö¶Wf—'GVÅö÷væW"‚fFW&—fVBÂ'&VB"Â$&6R"Â&vWB"Â"‚”’"“°¢76W'EöW€¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2†VÖ—GFVEöÖWF†öEö6öFR‚fFW&—fVBÂ'&VB"’¢æ–çFõö—FW"‚¢æÖ‡Â…òÂ÷6öFR—Â÷6öFR¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢³ƒ&Â†#bÂ†5ÒÀ¢&W‡Æ–6—BF†—26ÆÂÆöG2ÆöEóÂ–çfö¶W2&6RævWBÂæB&WGW&ç2—G2–çB&W7VÇB ¢“°¢76W'Eö–çfö¶Wf—'GVÅö÷væW"‚g'VææW"Â'&VB"Â$&6R"Â&F÷V&ÆVB"Â"‚”’"“°¢76W'Eö–çfö¶Wf—'GVÅö÷væW"‚g'VææW"Â'f–FW&—fVB"Â$&6R"Â&F÷V&ÆVB"Â"‚”’"“°¢76W'Eö–çfö¶Wf—'GVÅö÷væW"‚g'VææW"Â&f÷'v&B"Â%'VææW""Â'&VB"Â"„Ä&6S²”’"“°¢76W'B€¢FW&—fVBæÖWF†öG2æ—FW"‚’æÆÂ‡ÆÖWF†öGÂÖF6†W2€¢FW&—fVBæ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’À¢ö²‚&F÷V&ÆVB"Â&vWB"¢’’À¢&–æ†W&—FVB&6RæF÷V&ÆVBæB&6RævWB&Ræ÷B6÷–VB–çFòFW&—fVB ¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB6Æ77F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚$–æ†W&—FVEf—'GVÄ6ÆÇ4†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ72–æ†W&—FVEf—'GVÄ6ÆÇ4†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²'VææW"'VææW"ÒæWr'VææW"‚“²&6R&6RÒæWr&6R‚“²FW&—fVBFW&—fVBÒæWrFW&—fVB‚“²–b‡'VææW"ç&VB†&6R’Ò#ÇÂ'VææW"ç&VB†FW&—fVB’ÒC"ÇÂ'VææW"çf–FW&—fVB†FW&—fVB’ÒC"ÇÂ'VææW"æf÷'v&B†FW&—fVB’ÒC"ÇÂFW&—fVBç&VB‚’Ò’F‡&÷ræWr76W'F–öäW'&÷"‚“²7—7FVÒæ÷WBç&–çFÆâ…Â&–æ†W&—FVBf—'GVÂF—7F6‚fW&–f–VEÂ"“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FR–æ†W&—FVBÖ6ÆÂ†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â6Æ77F‚Â"ÖB"Â6Æ77F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVB–æ†W&—FVBÖ6ÆÂ†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢6Æ77F‚À¢$–æ†W&—FVEf—'GVÄ6ÆÇ4†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W""“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVB–æ†W&—FVBÖ6ÆÂ6Æ76W5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢&–æ†W&—FVBf—'GVÂF—7F6‚fW&–f–VB ¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâG&ç6—F—fUö–æ†W&—FVEö6ÆÇ5ö¶VWöFV6Æ&–æuöÖWF†öG&Vg5öæEöF—7F6…öG–æÖ–6ÆÇ’‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'G&ç6—F—fRÖ–æ†W&—FVB×f—'GVÂÖ6ÆÇ2"“°¢ÆWB6÷W&6RÒ&6Æ72&6R²FVbfÇVR‚“¢–çBÒ²FVbF÷V&ÆVB‚“¢–çBÒfÇVR‚’¢#²FVb&6TöæÇ’‚“¢–çBÒrÕÆæ6Æ72Ö–FFÆRW‡FVæG2&6R²÷fW'&–FRFVbfÇVR‚“¢–çBÒ#ÕÆæ6Æ72ÆVbW‡FVæG2Ö–FFÆR²÷fW'&–FRFVbfÇVR‚“¢–çBÒ#²FVb&VçEfÇVR‚“¢–çBÒ7WW"çfÇVR‚“²FVbf–F†—2‚“¢–çBÒF†—2æ&6TöæÇ’‚’ÕÆæ6Æ72'VææW"²FVbF‡&÷Vv„&6R†&6S¢&6R“¢–çBÒ&6RæF÷V&ÆVB‚“²FVbf–ÆVb†ÆVc¢ÆVb“¢–çBÒÆVbæ&6TöæÇ’‚“²FVbf–ÆVdF÷V&ÆVB†ÆVc¢ÆVb“¢–çBÒÆVbæF÷V&ÆVB‚’ÕÆâ#°¢ÆWB×WBö'6W'fVEöÖWFFFÒæöæS°¢ÆWB×WBö'6W'fRÒÇVæ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÂFVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ö'6W'fVEöÖWFFFÒ6öÖR€¢Væ—BæÖWFFF†FVf–æ—F–öç2¢æW‡V7B‚'fÆ–FFVBG&ç6—F—fRF—7F6‚ÖWFFF"’À¢“°¢Ó°¢ÆWB'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö&F6…öö'6W'fVB€¢6÷W&6RÀ¢%G&ç6—F—fT–æ†W&—FVD6ÆÇ2ç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒC2’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢6öÖR‚f×WBö'6W'fR’À¢¢æW‡V7B‚'G&ç6—F—fR–æ†W&—FVB6ÆÇ2&WF–â6æöæ–6ÂFV6Æ&F–öç2"“° ¢ÆWBFV6öFUö6Æ72ÒÆæÖS¢g7G'Â°¢ÆWB'F–f7BÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒæÖR¢çVçw&ö÷%öVÇ6R‡ÇÂæ–2‚'¶æÖWÒ6Æ72—2VÖ—GFVB"’“°¢FV6öFUöVÖ—GFVEö6Æ72†'F–f7B¢Ó°¢ÆWB&6RÒFV6öFUö6Æ72‚$&6R"“°¢ÆWBÖ–FFÆRÒFV6öFUö6Æ72‚$Ö–FFÆR"“°¢ÆWBÆVbÒFV6öFUö6Æ72‚$ÆVb"“°¢ÆWB'VææW"ÒFV6öFUö6Æ72‚%'VææW""“°¢ÆWBÖWFFFÒö'6W'fVEöÖWFFFæW‡V7B‚&ö'6W'fW"6GW&W2fÆ–FFVB6Æ72ÖWFFF"“°¢ÆWB&6UöÖWFFFÒÖWFFF¢æ—FW"‚¢æf–æB‡Æ6Æ77Â6Æ72æ–FVçF—G’‚’æ–çFW&æÅöæÖR‚’ÓÒ$&6R"¢æW‡V7B‚$&6RÖWFFF"“°¢ÆWBÆVeöÖWFFFÒÖWFFF¢æ—FW"‚¢æf–æB‡Æ6Æ77Â6Æ72æ–FVçF—G’‚’æ–çFW&æÅöæÖR‚’ÓÒ$ÆVb"¢æW‡V7B‚$ÆVbÖWFFF"“°¢ÆWB&6UöF÷V&ÆVBÒ&6UöÖWFFF¢æÖWF†öG2‚¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂÖWF†öBææÖR‚’ÓÒ&F÷V&ÆVB"¢æW‡V7B‚$&6RFV6Æ&W2F÷V&ÆVB"“°¢ÆWB&6UööæÇ’Ò&6UöÖWFFF¢æÖWF†öG2‚¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂÖWF†öBææÖR‚’ÓÒ&&6TöæÇ’"¢æW‡V7B‚$&6RFV6Æ&W2&6TöæÇ’"“°¢ÆWBÆVe÷fÇVRÒÆVeöÖWFFF¢æÖWF†öG2‚¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂÖWF†öBææÖR‚’ÓÒ'fÇVR"¢æW‡V7B‚$ÆVbFV6Æ&W2—G2÷fW'&–FR"“°¢ÆWB–æ†W&—FVEöF÷V&ÆVBÒÆVeöÖWFFF¢æF—7F6…öÖWF†öG2‚¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂÖWF†öBææÖR‚’ÓÒ&F÷V&ÆVB"¢æW‡V7B‚$&6RæF÷V&ÆVB—2VffV7F—fRöâÆVb"“°¢76W'EöW†–æ†W&—FVEöF÷V&ÆVBæÖWF†öB‚’Â&6UöF÷V&ÆVBç7–Ö&öÂ‚’“°¢76W'EöW€¢–æ†W&—FVEöF÷V&ÆVBæFV6Æ&–æuö6Æ72‚’À¢&6UöÖWFFFæ–FVçF—G’‚’ç7–Ö&öÂ‚¢“°¢76W'EöW€¢–æ†W&—FVEöF÷V&ÆVBç&V6V—fW%ö6Æ72‚’À¢ÆVeöÖWFFFæ–FVçF—G’‚’ç7–Ö&öÂ‚¢“°¢ÆWB–æ†W&—FVEö&6UööæÇ’ÒÆVeöÖWFFF¢æF—7F6…öÖWF†öG2‚¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂÖWF†öBææÖR‚’ÓÒ&&6TöæÇ’"¢æW‡V7B‚$&6Ræ&6TöæÇ’—2VffV7F—fRöâÆVb"“°¢76W'EöW†–æ†W&—FVEö&6UööæÇ’æÖWF†öB‚’Â&6UööæÇ’ç7–Ö&öÂ‚’“°¢ÆWBVffV7F—fU÷fÇVRÒÆVeöÖWFFF¢æF—7F6…öÖWF†öG2‚¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂÖWF†öBææÖR‚’ÓÒ'fÇVR"¢æW‡V7B‚$ÆVbçfÇVR—2F†RVffV7F—fR÷fW'&–FR"“°¢76W'EöW†VffV7F—fU÷fÇVRæÖWF†öB‚’ÂÆVe÷fÇVRç7–Ö&öÂ‚’“°¢76W'EöW€¢VffV7F—fU÷fÇVRæFV6Æ&–æuö6Æ72‚’À¢ÆVeöÖWFFFæ–FVçF—G’‚’ç7–Ö&öÂ‚¢“° ¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢g'VææW"À¢VÖ—GFVEöÖWF†öEö6öFR‚g'VææW"Â'F‡&÷Vv„&6R"’À¢e²ƒ†#bÂ$&6R"Â&F÷V&ÆVB"Â"‚”’"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢g'VææW"À¢VÖ—GFVEöÖWF†öEö6öFR‚g'VææW"Â'f–ÆVb"’À¢e²ƒ†#bÂ$&6R"Â&&6TöæÇ’"Â"‚”’"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢g'VææW"À¢VÖ—GFVEöÖWF†öEö6öFR‚g'VææW"Â'f–ÆVdF÷V&ÆVB"’À¢e²ƒ†#bÂ$&6R"Â&F÷V&ÆVB"Â"‚”’"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢fÆVbÀ¢VÖ—GFVEöÖWF†öEö6öFR‚fÆVbÂ'&VçEfÇVR"’À¢e²ƒ†#rÂ$Ö–FFÆR"Â'fÇVR"Â"‚”’"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢fÆVbÀ¢VÖ—GFVEöÖWF†öEö6öFR‚fÆVbÂ'f–F†—2"’À¢e²ƒ†#bÂ$&6R"Â&&6TöæÇ’"Â"‚”’"•ÒÀ¢“°¢f÷"6Æ72–â²fÖ–FFÆRÂfÆVeÒ°¢76W'B€¢6Æ72æÖWF†öG2æ—FW"‚’æÆÂ‡ÆÖWF†öGÂÖF6†W2€¢6Æ72æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’À¢ö²‚&F÷V&ÆVB"Â&&6TöæÇ’"¢’’À¢&–æ†W&—FVB&6RÖWF†öG2&Ræ÷B6÷–VB–çFò·Ò"À¢6Æ72æ6öç7FçE÷ööÂæ6Æ75öæÖR†6Æ72çF†—5ö6Æ72’çVçw&‚¢“°¢Ğ¢76W'B€¢&6RæÖWF†öG0¢æ—FW"‚¢æç’‡ÆÖWF†öGÂ²&6Ræ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’ÓÒö²‚&F÷V&ÆVB"’Ò¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB6Æ77F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚%G&ç6—F—fT–æ†W&—FVD6ÆÇ4†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ72G&ç6—F—fT–æ†W&—FVD6ÆÇ4†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²'VææW"'VææW"ÒæWr'VææW"‚“²&6R&6RÒæWr&6R‚“²ÆVbÆVbÒæWrÆVb‚“²–b‡'VææW"çF‡&÷Vv„&6R†&6R’Ò#ÇÂ'VææW"çF‡&÷Vv„&6R†ÆVb’ÒC"ÇÂ'VææW"çf–ÆVb†ÆVb’ÒrÇÂ'VææW"çf–ÆVdF÷V&ÆVB†ÆVb’ÒC"ÇÂÆVbç&VçEfÇVR‚’Ò#ÇÂÆVbçf–F†—2‚’Òr’F‡&÷ræWr76W'F–öäW'&÷"‚“²7—7FVÒæ÷WBç&–çFÆâ…Â'G&ç6—F—fR–æ†W&—FVBF—7F6‚fW&–f–VEÂ"“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FRG&ç6—F—fRÖ6ÆÂ†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â6Æ77F‚Â"ÖB"Â6Æ77F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVBG&ç6—F—fRÖ6ÆÂ†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢6Æ77F‚À¢%G&ç6—F—fT–æ†W&—FVD6ÆÇ4†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W""“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVBG&ç6—F—fRÖ–æ†W&—Fæ6R6Æ76W5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢'G&ç6—F—fR–æ†W&—FVBF—7F6‚fW&–f–VB ¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ7WW%ö6ÆÅ÷F‡&÷Vv…öV×G•ö–çFW&ÖVF–FUö6Æ75÷W6W5öF—&V7E÷&VçEöÖWF†öG&Vb‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&–æ†W&—FVB×7WW"ÖÖWF†öG&Vb"“°¢ÆWB6÷W&6RÒ&6Æ72&6R²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72Ö–FFÆRW‡FVæG2&6R·ÕÆæ6Æ72ÆVbW‡FVæG2Ö–FFÆR²÷fW'&–FRFVbfÇVR‚“¢–çBÒ#²FVb&VçEfÇVR‚“¢–çBÒ7WW"çfÇVR‚’ÕÆâ#°¢ÆWB'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ$–æ†W&—FVE7WW"ç66Æ"Â÷WGWBçF‚‚’¢æW‡V7B‚'7WW"6â6VÆV7BÖWF†öB–æ†W&—FVBF‡&÷Vv‚F†R–ÖÖVF–FR&VçB"“°¢ÆWBÆVeö'F–f7BÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$ÆVb"¢æW‡V7B‚$ÆVb6Æ72—2VÖ—GFVB"“°¢ÆWBÆVbÒFV6öFUöVÖ—GFVEö6Æ72†ÆVeö'F–f7B“° ¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢fÆVbÀ¢VÖ—GFVEöÖWF†öEö6öFR‚fÆVbÂ'&VçEfÇVR"’À¢e²ƒ†#rÂ$Ö–FFÆR"Â'fÇVR"Â"‚”’"•ÒÀ¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB6Æ77F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚$–æ†W&—FVE7WW$†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ72–æ†W&—FVE7WW$†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²–b†æWrÆVb‚’ç&VçEfÇVR‚’Ò’F‡&÷ræWr76W'F–öäW'&÷"‚“²7—7FVÒæ÷WBç&–çFÆâ…Â&–æ†W&—FVB7WW"6ÆÂfW&–f–VEÂ"“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FR–æ†W&—FVB×7WW"†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â6Æ77F‚Â"ÖB"Â6Æ77F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVB–æ†W&—FVB×7WW"†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢6Æ77F‚À¢$–æ†W&—FVE7WW$†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W""“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVB–æ†W&—FVB×7WW"6Æ76W5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢&–æ†W&—FVB7WW"6ÆÂfW&–f–VB ¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâF‡&VUöÆWfVÅ÷&ÖWFW&—¦VEö6öç7G'V7F÷'5öæE÷7WW%ö6ÆÇ5ö6ö×÷6R‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'F‡&VRÖÆWfVÂÖ6öç7G'V7F÷"×7WW""“°¢ÆWB6V6öæEö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'F‡&VRÖÆWfVÂÖ6öç7G'V7F÷"×7WW"×6V6öæB"“°¢ÆWB'F–f7G2Ò6ö×–ÆUöf—‡GW&Uö6Æ76W2‚%F‡&VTÆWfVÄ6öç7G'V7F÷%7WW"ç66Æ"Â÷WGWBçF‚‚’¢æW‡V7B‚'F†R6÷W&6RÖ&6¶VBF‡&VRÖÆWfVÂ6öç7G'V7F÷"æB7WW"Ö6ÆÂ6†–â6ö×–ÆW2"“°¢ÆWB6V6öæBÒ6ö×–ÆUöf—‡GW&Uö6Æ76W2‚%F‡&VTÆWfVÄ6öç7G'V7F÷%7WW"ç66Æ"Â6V6öæEö÷WGWBçF‚‚’¢æW‡V7B‚&g&W6‚g&öçFVæB6W76–öâVÖ—G2F†R6ÖRF‡&VRÖÆWfVÂ6†–â"“°¢76W'EöW€¢'F–f7G0¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢²$6†–ä&6R"Â$6†–äÖ–FFÆR"Â$6†–äÆVb"Â$6†–å'VææW"%Ğ¢“°¢76W'EöW€¢'F–f7G0¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ†'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’Â'F–f7Bæ6Æ72æ'—FW2‚’Â’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢6V6öæ@¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ†'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’Â'F–f7Bæ6Æ72æ'—FW2‚’Â’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢&g&W6‚g&öçFVæB6W76–öç2&W6W'fRFWFW&Ö–æ—7F–26†–â6Æ76f–ÆW2 ¢“° ¢ÆWB6Æ76W2Ò'F–f7G0¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ°¢€¢'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’À¢FV6öFUöVÖ—GFVEö6Æ72†'F–f7B’À¢¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢ÆWB6Æ72ÒÆæÖS¢g7G'Â°¢f6Æ76W0¢æ—FW"‚¢æf–æB‡Â†6æF–FFRÂò—Â¦6æF–FFRÓÒæÖR¢çVçw&ö÷%öVÇ6R‡ÇÂæ–2‚'¶æÖWÒ6Æ72—2VÖ—GFVB"’¢ã¢Ó°¢f÷"†æÖRÂ&VçB’–â°¢‚$6†–ä&6R"Â&¦föÆærôö&¦V7B"’À¢‚$6†–äÖ–FFÆR"Â$6†–ä&6R"’À¢‚$6†–äÆVb"Â$6†–äÖ–FFÆR"’À¢‚$6†–å'VææW""Â&¦föÆærôö&¦V7B"’À¢Ò°¢ÆWBVÖ—GFVBÒ6Æ72†æÖR“°¢76W'EöW€¢VÖ—GFV@¢æ6öç7FçE÷ööÀ¢æ6Æ75öæÖR†VÖ—GFVBç7WW%ö6Æ72çVçw&‚’’À¢ö²‡&VçB’À¢'¶æÖWÒFV6Æ&W2—G2–ÖÖVF–FR¥dÒ&VçB ¢“°¢Ğ ¢f÷"†æÖRÂ&VçBÂFW67&—F÷"’–â°¢‚$6†–ä&6R"Â&¦föÆærôö&¦V7B"Â"‚•b"’À¢‚$6†–äÖ–FFÆR"Â$6†–ä&6R"Â"„’•b"’À¢‚$6†–äÆVb"Â$6†–äÖ–FFÆR"Â"„’•b"’À¢Ò°¢ÆWBVÖ—GFVBÒ6Æ72†æÖR“°¢ÆWB6öç7G'V7F÷"ÒVÖ—GFVEöÖWF†öEö6öFR†VÖ—GFVBÂ#Æ–æ—Câ"“°¢ÆWB6ÆÇ2ÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öç7G'V7F÷"¢æ–çFõö—FW"‚¢æf–ÇFW%öÖ‡Â†öfg6WBÂ÷6öFR—Â†÷6öFRÓÒ†#r’çF†Vå÷6öÖR†öfg6WB’¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW†6ÆÇ2æÆVâ‚’ÂÂ'¶æÖWÒ6ÆÇ2W†7FÇ’öæR6öç7G'V7F÷""“°¢76W'EöÖWF†öE÷&VfW&Væ6R†VÖ—GFVBÂ6öç7G'V7F÷"Â†#rÂ&VçBÂ#Æ–æ—Câ"ÂFW67&—F÷"“°¢–bæÖRÓÒ$6†–ä&6R"°¢ÆWB&VçEö6ÆÂÒ6ÆÇ5³Ó°¢ÆWBf–VÆE÷7F÷&RÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öç7G'V7F÷"¢æ–çFõö—FW"‚¢æf–æEöÖ‡Â†öfg6WBÂ÷6öFR—Â†÷6öFRÓÒ†#R’çF†Vå÷6öÖR†öfg6WB’¢æW‡V7B‚$&6R–æ—F–Æ—¦W2—G26÷W&6Rf–VÆB"“°¢76W'B€¢&VçEö6ÆÂÂf–VÆE÷7F÷&RÀ¢'¶æÖWÒ–æ—F–Æ—¦W2—G2÷vâf–VÆG2gFW"—G2&VçB6öç7G'V7F÷" ¢“°¢Ğ¢–bæÖRÒ$6†–ä&6R"°¢76W'EöW€¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öç7G'V7F÷"¢æ—FW"‚¢çF¶Rƒ2¢æÖ‡Â…òÂ÷6öFR—Â¦÷6öFR¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢³ƒ&Âƒ"Â†#uÒÀ¢'¶æÖWÒf÷'v&G2—G2÷vâ&–Ö'’Ö6öç7G'V7F÷"&ÖWFW" ¢“°¢Ğ¢Ğ ¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢6Æ72‚$6†–äÖ–FFÆR"’À¢VÖ—GFVEöÖWF†öEö6öFR†6Æ72‚$6†–äÖ–FFÆR"’Â'fÇVR"’À¢e²ƒ†#rÂ$6†–ä&6R"Â'fÇVR"Â"‚”’"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢6Æ72‚$6†–äÆVb"’À¢VÖ—GFVEöÖWF†öEö6öFR†6Æ72‚$6†–äÆVb"’Â'fÇVR"’À¢e²ƒ†#rÂ$6†–äÖ–FFÆR"Â'fÇVR"Â"‚”’"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢6Æ72‚$6†–äÆVb"’À¢VÖ—GFVEöÖWF†öEö6öFR†6Æ72‚$6†–äÆVb"’Â'&VçEfÇVR"’À¢e²ƒ†#rÂ$6†–äÖ–FFÆR"Â'fÇVR"Â"‚”’"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢6Æ72‚$6†–ä&6R"’À¢VÖ—GFVEöÖWF†öEö6öFR†6Æ72‚$6†–ä&6R"’Â&F÷V&ÆVB"’À¢e²ƒ†#bÂ$6†–ä&6R"Â'fÇVR"Â"‚”’"•ÒÀ¢“°¢76W'B€¢6Æ72‚$6†–äÆVb"’æÖWF†öG2æ—FW"‚’æÆÂ‡ÆÖWF†öGÂ°¢ÖF6†W2€¢6Æ72‚$6†–äÆVb"’æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’À¢ö²‚&F÷V&ÆVB"¢¢Ò’À¢'F†R–æ†W&—FVB&6R&öG’—2VÖ—GFVBöæÇ’öæ6R ¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢&WGW&ã°¢Ğ¢ÆWB6Æ77F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚6Æ77F‚"“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…²"Ö2"Â"×b"Â"Ö6Æ77F‚"Â6Æ77F‚Â$6†–äÆVb%Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ff÷"F†RÆVb6Æ72"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦f&V¦V7FVB6†–äÆVbæ6Æ75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢ÆWB¦fö÷WGWBÒ7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB“°¢76W'B†¦fö÷WGWBæ6öçF–ç2‚$6†–äÖ–FFÆRåÂ#Æ–æ—CåÂ#¢„’•b"’“°¢76W'B†¦fö÷WGWBæ6öçF–ç2‚$6†–äÖ–FFÆRçfÇVS¢‚”’"’“° ¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚%F‡&VTÆWfVÄ6öç7G'V7F÷%7WW$†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ72F‡&VTÆWfVÄ6öç7G'V7F÷%7WW$†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²6†–äÆVb÷6—F—fRÒæWr6†–äÆVbƒ“²6†–äÆVbæVvF—fRÒæWr6†–äÆVb‚Ó2“²–b‡÷6—F—fRçfÇVR‚’Ò"ÇÂ÷6—F—fRç&VçEfÇVR‚’ÒÇÂ÷6—F—fRæF÷V&ÆVB‚’Ò#B’F‡&÷ræWr76W'F–öäW'&÷"…Â'÷6—F—fR6†–åÂ"“²–b†æVvF—fRçfÇVR‚’ÒÓÇÂæVvF—fRç&VçEfÇVR‚’ÒÓ"ÇÂæVvF—fRæF÷V&ÆVB‚’ÒÓ"’F‡&÷ræWr76W'F–öäW'&÷"…Â&æVvF—fR6†–åÂ"“²6†–äÖ–FFÆRÖ–FFÆRÒæWr6†–äÖ–FFÆRƒ“²–b†Ö–FFÆRçfÇVR‚’Ò’F‡&÷ræWr76W'F–öäW'&÷"…Â&Ö–FFÆRF—&V7B7WW%Â"“²7—7FVÒæ÷WBç&–çFÆâ…Â'F‡&VRÖÆWfVÂ6öç7G'V7F÷"æB7WW"F—7F6‚fW&–f–VEÂ"“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FRF‡&VRÖÆWfVÂ6öç7G'V7F÷"÷7WW"¥dÒ†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â6Æ77F‚Â"ÖB"Â6Æ77F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2f÷"F†R6†–â†&æW72"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVBF†RF‡&VRÖÆWfVÂ6†–â†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢6Æ77F‚À¢%F‡&VTÆWfVÄ6öç7G'V7F÷%7WW$†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦fv—F‚gVÆÂfW&–f–6F–öâf÷"F†R6†–â†&æW72"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVBF†RF‡&VRÖÆWfVÂ6†–åÆç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢'F‡&VRÖÆWfVÂ6öç7G'V7F÷"æB7WW"F—7F6‚fW&–f–VB ¢“°§Ğ ¢5·FW7EĞ¦fâgVÆÅö×VÇF–ÆWfVÅö–æ†W&—Fæ6Uöf—‡GW&U÷fW&–f–W5ö6öç7G'V7F–öåöF—7F6…öæEö¦ö–ç2‚’°¢ÆWBf—'7Eö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&×VÇF–ÆWfVÂÖ–æ†W&—Fæ6RÖf—'7B"“°¢ÆWB6V6öæEö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&×VÇF–ÆWfVÂÖ–æ†W&—Fæ6R×6V6öæB"“°¢ÆWBf—'7BÒ6ö×–ÆUöf—‡GW&Uö6Æ76W2‚$×VÇF–ÆWfVÄ–æ†W&—Fæ6Rç66Æ"Âf—'7Eö÷WGWBçF‚‚’¢æW‡V7B‚'F†R6ö×ÆWFR&VÂ×6÷W&6R×VÇF–ÆWfVÂf—‡GW&R6ö×–ÆW2–âöæR6W76–öâ"“°¢ÆWB6V6öæBÒ6ö×–ÆUöf—‡GW&Uö6Æ76W2‚$×VÇF–ÆWfVÄ–æ†W&—Fæ6Rç66Æ"Â6V6öæEö÷WGWBçF‚‚’¢æW‡V7B‚&g&W6‚g&öçFVæB6W76–öâVÖ—G2F†R6ÖR6ö×ÆWFR6÷W&6Rf—‡GW&R"“°¢ÆWB–FVçF—F–W2ÒÆ'F–f7G3¢e´VÖ—GFVD'F–f7E×Â°¢'F–f7G0¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ°¢€¢'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’çFõö÷væVB‚’À¢'F–f7Bæ6Æ72æ'—FW2‚’çFõ÷fV2‚’À¢¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚¢Ó°¢76W'EöW†–FVçF—F–W2‚ff—'7B’Â–FVçF—F–W2‚g6V6öæB’“°¢76W'EöW€¢f—'7@¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢²$&6R"Â$Ö–FFÆR"Â$ÆVb"Â%'VææW"%ÒÀ¢&6ö×ÆWFR6Æ72&F6†W2&WF–â6÷W&6R÷&FW" ¢“° ¢ÆWB6Æ76W2Òf—'7@¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ°¢€¢'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’À¢FV6öFUöVÖ—GFVEö6Æ72†'F–f7B’À¢¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢ÆWB6Æ72ÒÆæÖS¢g7G'Â°¢f6Æ76W0¢æ—FW"‚¢æf–æB‡Â†6æF–FFRÂò—Â¦6æF–FFRÓÒæÖR¢çVçw&ö÷%öVÇ6R‡ÇÂæ–2‚'¶æÖWÒ6Æ72—2VÖ—GFVB"’¢ã¢Ó°¢f÷"†æÖRÂ&VçB’–â°¢‚$&6R"Â&¦föÆærôö&¦V7B"’À¢‚$Ö–FFÆR"Â$&6R"’À¢‚$ÆVb"Â$Ö–FFÆR"’À¢‚%'VææW""Â&¦föÆærôö&¦V7B"’À¢Ò°¢ÆWBVÖ—GFVBÒ6Æ72†æÖR“°¢76W'EöW†VÖ—GFVBçfW'6–öâæÖ¦÷"ÂcR“°¢76W'B†VÖ—GFVBæ66W75öfÆw2æ—5÷V&Æ–2‚’“°¢76W'EöW€¢VÖ—GFV@¢æ6öç7FçE÷ööÀ¢æ6Æ75öæÖR†VÖ—GFVBç7WW%ö6Æ72çVçw&‚’’À¢ö²‡&VçB’À¢'¶æÖWÒ&V6÷&G2—G2W†7B–ÖÖVF–FR&VçB ¢“°¢Ğ ¢76W'EöW†6Æ72‚$&6R"’æf–VÆG2æÆVâ‚’Â“°¢ÆWB7W'&VçBÒf6Æ72‚$&6R"’æf–VÆG5³Ó°¢76W'EöW€¢6Æ72‚$&6R"’æ6öç7FçE÷ööÂçWFc‚†7W'&VçBææÖUö–æFW‚’À¢ö²‚&7W'&VçB"¢“°¢76W'EöW€¢6Æ72‚$&6R"’æ6öç7FçE÷ööÂçWFc‚†7W'&VçBæFW67&—F÷%ö–æFW‚’À¢ö²‚$’"¢“°¢76W'B†7W'&VçBæ66W75öfÆw2æ—5÷&—fFR‚’“°¢76W'B‚7W'&VçBæ66W75öfÆw2æ—5öf–æÂ‚’“°¢76W'B†6Æ72‚$Ö–FFÆR"’æf–VÆG2æ—5öV×G’‚’“°¢76W'B†6Æ72‚$ÆVb"’æf–VÆG2æ—5öV×G’‚’“° ¢f÷"†æÖRÂW‡V7FVB’–â°¢€¢$&6R"À¢fV2²‚#Æ–æ—Câ"Â"„’•b"’Â‚'fÇVR"Â"‚”’"’Â‚&F÷V&ÆVB"Â"‚”’"•ÒÀ¢’À¢‚$Ö–FFÆR"ÂfV2²‚#Æ–æ—Câ"Â"„’•b"’Â‚'fÇVR"Â"‚”’"•Ò’À¢€¢$ÆVb"À¢fV2²‚#Æ–æ—Câ"Â"„’•b"’Â‚'fÇVR"Â"‚”’"’Â‚'&VçEfÇVR"Â"‚”’"•ÒÀ¢’À¢€¢%'VææW""À¢fV2°¢‚#Æ–æ—Câ"Â"‚•b"’À¢‚&WfÇVFR"Â"„Ä&6S²”’"’À¢‚''Vâ"Â"„’”’"’À¢‚&6†ö÷6R"Â"…¤’”ÄÖ–FFÆS²"’À¢‚'&WfW'6T6†ö÷6R"Â"…¤’”ÄÖ–FFÆS²"’À¢‚&6†÷6VåfÇVR"Â"…¤’”’"’À¢‚'&WfW'6T6†÷6VåfÇVR"Â"…¤’”’"’À¢‚'&öGV6T&6R"Â"„’”Ä&6S²"’À¢ÒÀ¢’À¢Ò°¢76W'EöW€¢ÖWF†öG5÷v—F…öFW67&—F÷'2†6Æ72†æÖR’’À¢W‡V7FV@¢æ–çFõö—FW"‚¢æÖ‡Â†æÖRÂFW67&—F÷"—Â†æÖRçFõö÷væVB‚’ÂFW67&—F÷"çFõö÷væVB‚’’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢'¶æÖWÒW‡÷6W2öæÇ’—G2ÆææVBÖWF†öBFW67&—F÷'2 ¢“°¢76W'B€¢6Æ72†æÖR¢æÖWF†öG0¢æ—FW"‚¢æÆÂ‡ÆÖWF†öGÂÖWF†öBæ66W75öfÆw2æ—5÷V&Æ–2‚’¢“°¢Ğ¢76W'B€¢6Æ72‚$Ö–FFÆR"’æÖWF†öG2æ—FW"‚’æÆÂ‡ÆÖWF†öGÂ°¢6Æ72‚$Ö–FFÆR"’æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’Òö²‚&F÷V&ÆVB"¢Ò¢“°¢76W'B€¢6Æ72‚$ÆVb"¢æÖWF†öG0¢æ—FW"‚¢æÆÂ‡ÆÖWF†öGÂ²6Æ72‚$ÆVb"’æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’Òö²‚&F÷V&ÆVB"’Ò¢“° ¢f÷"†æÖRÂ&VçBÂFW67&—F÷"’–â°¢‚$&6R"Â&¦föÆærôö&¦V7B"Â"‚•b"’À¢‚$Ö–FFÆR"Â$&6R"Â"„’•b"’À¢‚$ÆVb"Â$Ö–FFÆR"Â"„’•b"’À¢Ò°¢ÆWB6öFRÒVÖ—GFVEöÖWF†öEö6öFR†6Æ72†æÖR’Â#Æ–æ—Câ"“°¢ÆWB6öç7G'V7F÷%ö6ÆÇ2ÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ—FW"‚¢æf–ÇFW"‡Â…òÂ÷6öFR—Â¦÷6öFRÓÒ†#r¢æ6÷VçB‚“°¢76W'EöW€¢6öç7G'V7F÷%ö6ÆÇ2ÂÀ¢'¶æÖWÒ–çfö¶W2W†7FÇ’öæR6öç7G'V7F÷" ¢“°¢76W'EöÖWF†öE÷&VfW&Væ6R†6Æ72†æÖR’Â6öFRÂ†#rÂ&VçBÂ#Æ–æ—Câ"ÂFW67&—F÷"“°¢Ğ¢ÆWB&6Uö6öç7G'V7F÷"ÒVÖ—GFVEöÖWF†öEö6öFR†6Æ72‚$&6R"’Â#Æ–æ—Câ"“°¢ÆWB&6Uö–ç7G'V7F–öç2ÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†&6Uö6öç7G'V7F÷"“°¢ÆWBö&¦V7Eö–æ—BÒ&6Uö–ç7G'V7F–öç0¢æ—FW"‚¢æf–æB‡Â…òÂ÷6öFR—Â¦÷6öFRÓÒ†#r¢æW‡V7B‚$&6R–çfö¶W2ö&¦V7B6öç7G'V7F÷""¢ã°¢ÆWB7W'&VçE÷7F÷&W2Ò&6Uö–ç7G'V7F–öç0¢æ—FW"‚¢æf–ÇFW"‡Â…òÂ÷6öFR—Â¦÷6öFRÓÒ†#R¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW†7W'&VçE÷7F÷&W2æÆVâ‚’ÂÂ$&6Ræ7W'&VçB—2–æ—F–Æ—¦VBöæ6R"“°¢76W'Eöf–VÆE÷&VfW&Væ6R€¢6Æ72‚$&6R"’À¢&6Uö6öç7G'V7F÷"À¢†#RÀ¢$&6R"À¢&7W'&VçB"À¢$’"À¢“°¢76W'B†ö&¦V7Eö–æ—BÂ7W'&VçE÷7F÷&W5³Òã“°¢76W'EöÖWF†öE÷&VfW&Væ6R€¢6Æ72‚$Ö–FFÆR"’À¢VÖ—GFVEöÖWF†öEö6öFR†6Æ72‚$Ö–FFÆR"’Â#Æ–æ—Câ"’À¢†#rÀ¢$&6R"À¢#Æ–æ—Câ"À¢"„’•b"À¢“°¢76W'EöÖWF†öE÷&VfW&Væ6R€¢6Æ72‚$ÆVb"’À¢VÖ—GFVEöÖWF†öEö6öFR†6Æ72‚$ÆVb"’Â#Æ–æ—Câ"’À¢†#rÀ¢$Ö–FFÆR"À¢#Æ–æ—Câ"À¢"„’•b"À¢“° ¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢6Æ72‚$&6R"’À¢VÖ—GFVEöÖWF†öEö6öFR†6Æ72‚$&6R"’Â&F÷V&ÆVB"’À¢e°¢ƒ†#bÂ$&6R"Â'fÇVR"Â"‚”’"’À¢ƒ†#bÂ$&6R"Â'fÇVR"Â"‚”’"’À¢ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢6Æ72‚$Ö–FFÆR"’À¢VÖ—GFVEöÖWF†öEö6öFR†6Æ72‚$Ö–FFÆR"’Â'fÇVR"’À¢e²ƒ†#rÂ$&6R"Â'fÇVR"Â"‚”’"•ÒÀ¢“°¢f÷"†ÖWF†öBÂ&VçB’–â²‚'fÇVR"Â$Ö–FFÆR"’Â‚'&VçEfÇVR"Â$Ö–FFÆR"•Ò°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢6Æ72‚$ÆVb"’À¢VÖ—GFVEöÖWF†öEö6öFR†6Æ72‚$ÆVb"’ÂÖWF†öB’À¢e²ƒ†#rÂ&VçBÂ'fÇVR"Â"‚”’"•ÒÀ¢“°¢Ğ¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢6Æ72‚%'VææW""’À¢VÖ—GFVEöÖWF†öEö6öFR†6Æ72‚%'VææW""’Â&WfÇVFR"’À¢e²ƒ†#bÂ$&6R"Â&F÷V&ÆVB"Â"‚”’"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢6Æ72‚%'VææW""’À¢VÖ—GFVEöÖWF†öEö6öFR†6Æ72‚%'VææW""’Â''Vâ"’À¢e°¢ƒ†#rÂ$ÆVb"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#bÂ%'VææW""Â&WfÇVFR"Â"„Ä&6S²”’"’À¢ÒÀ¢“°¢f÷"ÖWF†öB–â²&6†ö÷6R"Â'&WfW'6T6†ö÷6R%Ò°¢ÆWB6öFRÒVÖ—GFVEöÖWF†öEö6öFR†6Æ72‚%'VææW""’ÂÖWF†öB“°¢ÆWB'&æ6…ö6öç7G'V7F÷%ö÷&FW"Ò–bÖWF†öBÓÒ&6†ö÷6R"°¢°¢ƒ†#rÂ$Ö–FFÆR"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ$ÆVb"Â#Æ–æ—Câ"Â"„’•b"’À¢Ğ¢ÒVÇ6R°¢°¢ƒ†#rÂ$ÆVb"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ$Ö–FFÆR"Â#Æ–æ—Câ"Â"„’•b"’À¢Ğ¢Ó°¢76W'Eö–çfö6F–öå÷6WVVæ6R†6Æ72‚%'VææW""’Â6öFRÂf'&æ6…ö6öç7G'V7F÷%ö÷&FW"“°¢76W'B€¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ—FW"‚¢æÆÂ‡Â…òÂ÷6öFR—Â¦÷6öFRÒ†3’À¢'¶ÖWF†öGÒæVVG2æò6†V6¶67B ¢“°¢ÆWBg&ÖW2Ò7F6µöÖög&ÖW2†6Æ72‚%'VææW""’ÂÖWF†öB“°¢ÆWB'&æ6…÷F&vWG2ÒFV6öFUö'&æ6…÷F&vWG2†6öFR¢æ–çFõö—FW"‚¢æÖ‡Â…òÂòÂF&vWB—ÂF&vWB¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'B‚'&æ6…÷F&vWG2æ—5öV×G’‚’“°¢76W'B€¢'&æ6…÷F&vWG0¢æ—FW"‚¢æÆÂ‡ÇF&vWGÂ²g&ÖW2æ—FW"‚’æç’‡Æg&ÖWÂg&ÖRç2ÓÒ§F&vWB’Ò¢“°¢76W'B†g&ÖW2æ—FW"‚’æç’‡Æg&ÖWÂ°¢g&ÖRç7F6²ÓÒµ7F6´ÖfW&–f–6F–öåG—S£¤ö&¦V7B‚$Ö–FFÆR"çFõö÷væVB‚’•Ğ¢Ò’“°¢ÆWBÖWF†öEö–æfòÒ6Æ72‚%'VææW""¢æÖWF†öG0¢æ—FW"‚¢æf–æB‡ÆVçG'—Â6Æ72‚%'VææW""’æ6öç7FçE÷ööÂçWFc‚†VçG'’ææÖUö–æFW‚’ÓÒö²†ÖWF†öB’¢æW‡V7B‚'&VfW&Væ6R6öæF—F–öæÂÖWF†öBW†—7G2"“°¢ÆWB´GG&–'WFS£¤6öFR†6öFUö–æfò•ÒÒÖWF†öEö–æfòæGG&–'WFW2æ5÷6Æ–6R‚’VÇ6R°¢æ–2‚'¶ÖWF†öGÒ†2öæR6öFRGG&–'WFR"“°¢Ó°¢76W'B†6öFUö–æfòæÖ…÷7F6²ãÒ2“°¢76W'EöW†6öFUö–æfòæÖ…öÆö6Ç2Â2“°¢Ğ¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢6Æ72‚%'VææW""’À¢VÖ—GFVEöÖWF†öEö6öFR†6Æ72‚%'VææW""’Â&6†÷6VåfÇVR"’À¢e°¢ƒ†#bÂ%'VææW""Â&6†ö÷6R"Â"…¤’”ÄÖ–FFÆS²"’À¢ƒ†#bÂ$Ö–FFÆR"Â'fÇVR"Â"‚”’"’À¢ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢6Æ72‚%'VææW""’À¢VÖ—GFVEöÖWF†öEö6öFR†6Æ72‚%'VææW""’Â'&WfW'6T6†÷6VåfÇVR"’À¢e°¢ƒ†#bÂ%'VææW""Â'&WfW'6T6†ö÷6R"Â"…¤’”ÄÖ–FFÆS²"’À¢ƒ†#bÂ$Ö–FFÆR"Â'fÇVR"Â"‚”’"’À¢ÒÀ¢“°¢ÆWB&öGV6Uö&6RÒVÖ—GFVEöÖWF†öEö6öFR†6Æ72‚%'VææW""’Â'&öGV6T&6R"“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢6Æ72‚%'VææW""’À¢&öGV6Uö&6RÀ¢e²ƒ†#rÂ$ÆVb"Â#Æ–æ—Câ"Â"„’•b"•ÒÀ¢“°¢76W'B€¢&öGV6Uö&6Ræ6öçF–ç2‚c†3’À¢'&öGV6T&6RVÖ—G2æò6†V6¶67B ¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢&WGW&ã°¢Ğ¢ÆWB6Æ77F‚Òf—'7Eö÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚6Æ77F‚"“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Ö2"À¢"×b"À¢"Ö6Æ77F‚"À¢6Æ77F‚À¢$&6R"À¢$Ö–FFÆR"À¢$ÆVb"À¢%'VææW""À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ff÷"F†R6ö×ÆWFR×VÇF–ÆWfVÂ&F6‚"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦f&V¦V7FVBF†R×VÇF–ÆWfVÂ6Æ72&F6…Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢ÆWB¦fö÷WGWBÒ7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB“°¢f÷"W‡V7FVB–â°¢&Ö¦÷"fW'6–öã¢cR"À¢$&6RåÂ#Æ–æ—CåÂ#¢„’•b"À¢$Ö–FFÆRåÂ#Æ–æ—CåÂ#¢„’•b"À¢$ÆVbåÂ#Æ–æ—CåÂ#¢„’•b"À¢$&6RçfÇVS¢‚”’"À¢$Ö–FFÆRçfÇVS¢‚”’"À¢%7F6´ÖF&ÆR"À¢Ò°¢76W'B€¢¦fö÷WGWBæ6öçF–ç2†W‡V7FVB’À¢&¦f÷WGWBöÖ—GFVB¶W‡V7FVGÒ ¢“°¢Ğ ¢ÆWB†&æW72Òf—'7Eö÷WGW@¢çF‚‚¢æ¦ö–â‚$×VÇF–ÆWfVÄ–æ†W&—Fæ6T†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢"2'V&Æ–2f–æÂ6Æ72×VÇF–ÆWfVÄ–æ†W&—Fæ6T†&æW72°¢V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’°¢'VææW"'VææW"ÒæWr'VææW"‚“°¢–b†æWr&6Rƒ’æF÷V&ÆVB‚’Ò#’F‡&÷ræWr76W'F–öäW'&÷"‚&&6R"“°¢–b†æWrÖ–FFÆRƒ’æF÷V&ÆVB‚’Ò#"’F‡&÷ræWr76W'F–öäW'&÷"‚&Ö–FFÆRf—'GVÂF—7F6‚"“°¢–b†æWrÆVbƒ’æF÷V&ÆVB‚’Ò#B’F‡&÷ræWr76W'F–öäW'&÷"‚&ÆVbf—'GVÂF—7F6‚"“°¢–b†æWrÆVbƒ’ç&VçEfÇVR‚’Ò’F‡&÷ræWr76W'F–öäW'&÷"‚&F—&V7B&VçB7WW""“°¢–b‡'VææW"ç'Vâƒ’Ò#BÇÂ'VææW"ç'Vâƒ’ÒBÇÂ'VææW"ç'Vâ‚Ó2’ÒÓ"’F‡&÷ræWr76W'F–öäW'&÷"‚&6öç7G'V7F÷"&wVÖVçG2"“°¢–b‡'VææW"æ6†÷6VåfÇVR‡G'VRÂ’ÒÇÂ'VææW"æ6†÷6VåfÇVR†fÇ6RÂ’Ò"’F‡&÷ræWr76W'F–öäW'&÷"‚&f÷'v&B¦ö–â"“°¢–b‡'VææW"ç&WfW'6T6†÷6VåfÇVR‡G'VRÂ’Ò"ÇÂ'VææW"ç&WfW'6T6†÷6VåfÇVR†fÇ6RÂ’Ò’F‡&÷ræWr76W'F–öäW'&÷"‚'&WfW'6R¦ö–â"“°¢&6R&öGV6VBÒ'VææW"ç&öGV6T&6Rƒ“°¢–b‚‡&öGV6VB–ç7Fæ6VöbÆVb’ÇÂ&öGV6VBæF÷V&ÆVB‚’Ò#B’F‡&÷ræWr76W'F–öäW'&÷"‚$ÆVb2&6R"“°¢7—7FVÒæ÷WBç&–çFÆâ‚&×VÇF–ÆWfVÂ–æ†W&—Fæ6RfW&–f–VB"“°¢Ğ§Ğ¢"2À¢¢æW‡V7B‚'w&—FR×VÇF–ÆWfVÂ6÷W&6R¥dÒ†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ò×&VÆV6R"Â##"Â"Ö6Æ77F‚"Â6Æ77F‚Â"ÖB"Â6Æ77F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2f÷"F†R×VÇF–ÆWfVÂ†&æW72"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVBF†R×VÇF–ÆWfVÂ†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢6Æ77F‚À¢$×VÇF–ÆWfVÄ–æ†W&—Fæ6T†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¤D²#fW&–f–W"f÷"F†R×VÇF–ÆWfVÂ†&æW72"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVBF†R×VÇF–ÆWfVÂ6Æ76W5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢&×VÇF–ÆWfVÂ–æ†W&—Fæ6RfW&–f–VB ¢“°§Ğ ¢5·FW7EĞ¦fâf÷W'F…÷6÷W&6Uö–æ†W&—Fæ6UöÆWfVÅöf–Ç5ö&Vf÷&Uö&F6…÷V&Æ–6F–öâ‚’°¢ÆWB6÷W&6RÒ&6Æ72FWF„&6UÆæ6Æ72FWF„Ö–FFÆRW‡FVæG2FWF„&6UÆæ6Æ72FWF„ÆVbW‡FVæG2FWF„Ö–FFÆUÆæ6Æ72FWF„f÷W'F‚W‡FVæG2FWF„ÆVeÆâ#°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&f÷W'F‚×6÷W&6RÖ–æ†W&—Fæ6RÖÆWfVÂ"“°¢ÆWB&W7VÇBÒ6ö×–ÆU÷6÷W&6Uö&F6‚€¢6÷W&6RÀ¢$f÷W'F„–æ†W&—Fæ6TÆWfVÂç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒC‚’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢“°¢76W'B€¢ÖF6†W2€¢&W7VÇBÀ¢W'"…—VÆ–æTf–ÇW&S£¤6öFVvVâ€¢§fÔ6öFVvVäW'&÷#£¤–çfÆ–D§fÕ7WW&6Æ72°¢—77VS¢F÷GG•ö§fÓ£¤§fÕ7WW&6Æ74—77VS£¤–æ†W&—Fæ6UFöôFVWÀ¢âà¢Ğ¢’¢’À¢'F†R6÷W&6R—VÆ–æR&V¦V7G2f÷W'F‚W6W"6Æ72v—F‚F†RG—VB†–W&&6‡’W'&÷#¢·&W7VÇC£÷Ò ¢“°¢76W'B€¢÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’À¢'F†R&V¦V7FVB†–W&&6‡’V&Æ—6†W2æò'F–Â6Æ72&F6‚ ¢“°§Ğ ¢5·FW7EĞ¦fâ66Æ÷6–ævÆUö–æ†W&—Fæ6UöæE÷f—'GVÅöF—7F6…÷75÷F†UöVæE÷FõöVæEö¦FµövFR‚’°¢ÆWBf—'7Eö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&–æ†W&—Fæ6RÖf—'7BÖ'V–ÆB"“°¢ÆWB6V6öæEö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&–æ†W&—Fæ6R×6V6öæBÖ'V–ÆB"“°¢ÆWBf—'7BÒ6ö×–ÆUöf—‡GW&Uö6Æ76W2‚$–æ†W&—Fæ6Rç66Æ"Âf—'7Eö÷WGWBçF‚‚’¢æW‡V7B‚'F†R&VÂ66Æ6÷W&6R—VÆ–æRVÖ—G2&6RÂFW&—fVBÂæB'VææW"FövWF†W""“°¢ÆWB6V6öæBÒ6ö×–ÆUöf—‡GW&Uö6Æ76W2‚$–æ†W&—Fæ6Rç66Æ"Â6V6öæEö÷WGWBçF‚‚’¢æW‡V7B‚&g&W6‚66Æ6÷W&6R—VÆ–æRVÖ—G2F†R6ÖR6Æ72&F6‚"“°¢ÆWBW‡V7FVEö÷&FW"Ò²$&6R"Â$FW&—fVB"Â%'VææW"%Ó°¢76W'EöW€¢f—'7@¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢W‡V7FVEö÷&FW ¢“°¢76W'EöW€¢6V6öæ@¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢W‡V7FVEö÷&FW"À¢&g&W6‚g&öçFVæB6W76–öç2&W6W'fR6Æ72÷WGWB÷&FW" ¢“°¢f÷"†ÆVgBÂ&–v‡B’–âf—'7Bæ—FW"‚’ç¦—‚g6V6öæB’°¢76W'EöW†ÆVgBæ6Æ72æ–çFW&æÅöæÖR‚’Â&–v‡Bæ6Æ72æ–çFW&æÅöæÖR‚’“°¢76W'EöW€¢ÆVgBæ6Æ72æ'—FW2‚’À¢&–v‡Bæ6Æ72æ'—FW2‚’À¢&g&W6‚g&öçFVæB6W76–öç2VÖ—B'—FRÖ–FVçF–6Â·Òæ6Æ72f–ÆW2"À¢ÆVgBæ6Æ72æ–çFW&æÅöæÖR‚¢“°¢Ğ ¢ÆWB&6RÒFV6öFUöVÖ—GFVEö6Æ72‚ff—'7E³Ò“°¢ÆWBFW&—fVBÒFV6öFUöVÖ—GFVEö6Æ72‚ff—'7E³Ò“°¢ÆWB'VææW"ÒFV6öFUöVÖ—GFVEö6Æ72‚ff—'7E³%Ò“°¢f÷"6Æ72–â²f&6RÂfFW&—fVBÂg'VææW%Ò°¢76W'EöW†6Æ72çfW'6–öâæÖ¦÷"ÂcR“°¢76W'B†6Æ72æ66W75öfÆw2æ—5÷V&Æ–2‚’“°¢Ğ¢76W'EöW€¢&6Ræ6öç7FçE÷ööÂæ6Æ75öæÖR†&6Rç7WW%ö6Æ72çVçw&‚’’À¢ö²‚&¦föÆærôö&¦V7B"¢“°¢76W'EöW€¢FW&—fV@¢æ6öç7FçE÷ööÀ¢æ6Æ75öæÖR†FW&—fVBç7WW%ö6Æ72çVçw&‚’’À¢ö²‚$&6R"¢“°¢76W'EöW€¢'VææW"æ6öç7FçE÷ööÂæ6Æ75öæÖR‡'VææW"ç7WW%ö6Æ72çVçw&‚’’À¢ö²‚&¦föÆærôö&¦V7B"¢“° ¢ÆWBÖWF†öEöFW67&—F÷"ÒÆ6Æ73¢d6Æ74f–ÆSÂuóâÂæÖS¢g7G'Â°¢6Æ70¢æÖWF†öG0¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂ6Æ72æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’ÓÒö²†æÖR’¢æÖ‡ÆÖWF†öGÂ°¢6Æ70¢æ6öç7FçE÷ööÀ¢çWFc‚†ÖWF†öBæFW67&—F÷%ö–æFW‚¢çVçw&‚¢çFõö÷væVB‚¢Ò¢çVçw&ö÷%öVÇ6R‡ÇÂæ–2‚'¶æÖWÒÖWF†öB—2VÖ—GFVB"’¢Ó°¢f÷"†6Æ72ÂÖWF†öBÂFW67&—F÷"’–â°¢‚f&6RÂ#Æ–æ—Câ"Â"‚•b"’À¢‚f&6RÂ'fÇVR"Â"‚”’"’À¢‚f&6RÂ&F÷V&ÆVB"Â"‚”’"’À¢‚fFW&—fVBÂ#Æ–æ—Câ"Â"‚•b"’À¢‚fFW&—fVBÂ'fÇVR"Â"‚”’"’À¢‚g'VææW"Â#Æ–æ—Câ"Â"‚•b"’À¢‚g'VææW"Â&WfÇVFR"Â"„Ä&6S²”’"’À¢‚g'VææW"Â&g&öÔ&6R"Â"‚”’"’À¢‚g'VææW"Â&g&öÔFW&—fVB"Â"‚”’"’À¢‚g'VææW"Â'&öGV6T&6R"Â"‚”Ä&6S²"’À¢‚g'VææW"Â&'&æ6‚"Â"„Ä&6Sµ¢”’"’À¢Ò°¢76W'EöW†ÖWF†öEöFW67&—F÷"†6Æ72ÂÖWF†öB’ÂFW67&—F÷"çFõö÷væVB‚’“°¢ÆWBf÷VæBÒ6Æ70¢æÖWF†öG0¢æ—FW"‚¢æf–æB‡Æ6æF–FFWÂ6Æ72æ6öç7FçE÷ööÂçWFc‚†6æF–FFRææÖUö–æFW‚’ÓÒö²†ÖWF†öB’¢çVçw&‚“°¢76W'B†f÷VæBæ66W75öfÆw2æ—5÷V&Æ–2‚’Â'¶ÖWF†öGÒ—2V&Æ–2"“°¢Ğ¢76W'B€¢FW&—fV@¢æÖWF†öG0¢æ—FW"‚¢æÆÂ‡ÆÖWF†öGÂÖF6†W2†FW&—fVBæ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’Âö²‚&F÷V&ÆVB"’’’À¢$FW&—fVB–æ†W&—G2&6RæF÷V&ÆVBv—F†÷WB6÷––ær—G2&öG’ ¢“° ¢ÆWB&VfW&Væ6W2ÒÆ6Æ73¢d6Æ74f–ÆSÂuóâÂÖWF†öC¢g7G"Â÷6öFS¢S‡Â°¢ÆWB6öFRÒVÖ—GFVEöÖWF†öEö6öFR†6Æ72ÂÖWF†öB“°¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ–çFõö—FW"‚¢æf–ÇFW%öÖ‡Â†öfg6WBÂf÷VæEö÷6öFR—Â†f÷VæEö÷6öFRÓÒ÷6öFR’çF†Vå÷6öÖR†öfg6WB’¢æÖ‡Æöfg6WGÂ°¢ÆWB–æFW‚ÒSc£¦g&öÕö&Uö'—FW2…¶6öFU¶öfg6WB²ÒÂ6öFU¶öfg6WB²%ÕÒ“°¢ÆWB6öç7FçEööÄVçG'“£¤ÖWF†öG&Vb°¢6Æ75ö–æFW‚À¢æÖUöæE÷G—Uö–æFW‚À¢ÒÒ6Æ70¢æ6öç7FçE÷ööÀ¢ævWB„6öç7FçEööÄ–æFW‚†–æFW‚’¢æW‡V7B‚&–çfö6F–öâ†2fÆ–BÖWF†öB&VfW&Væ6R"¢VÇ6R°¢æ–2‚&–çfö6F–öâ&VfW&Væ6W2ÖWF†öG&Vb"¢Ó°¢ÆWB†æÖRÂFW67&—F÷"’Ò6Æ70¢æ6öç7FçE÷ööÀ¢ææÖUöæE÷G—R‚¦æÖUöæE÷G—Uö–æFW‚¢æW‡V7B‚&ÖWF†öB&VfW&Væ6R†2fÆ–BæÖRæBFW67&—F÷""“°¢€¢6Æ70¢æ6öç7FçE÷ööÀ¢æ6Æ75öæÖR‚¦6Æ75ö–æFW‚¢æW‡V7B‚&ÖWF†öB÷væW"—26Æ72"¢çFõö÷væVB‚’À¢æÖRçFõö÷væVB‚’À¢FW67&—F÷"çFõö÷væVB‚’À¢¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚¢Ó°¢76W'EöW€¢&VfW&Væ6W2‚f&6RÂ#Æ–æ—Câ"Â†#r’À¢²€¢&¦föÆærôö&¦V7B"çFõö÷væVB‚’À¢#Æ–æ—Câ"çFõö÷væVB‚’À¢"‚•b"çFõö÷væVB‚¢•Ğ¢“°¢76W'EöW€¢&VfW&Væ6W2‚fFW&—fVBÂ#Æ–æ—Câ"Â†#r’À¢²‚$&6R"çFõö÷væVB‚’Â#Æ–æ—Câ"çFõö÷væVB‚’Â"‚•b"çFõö÷væVB‚’•Ğ¢“°¢76W'EöW€¢&VfW&Væ6W2‚f&6RÂ&F÷V&ÆVB"Â†#b’À¢°¢‚$&6R"çFõö÷væVB‚’Â'fÇVR"çFõö÷væVB‚’Â"‚”’"çFõö÷væVB‚’’À¢‚$&6R"çFõö÷væVB‚’Â'fÇVR"çFõö÷væVB‚’Â"‚”’"çFõö÷væVB‚’’À¢ÒÀ¢&&÷F‚6ÆÇ2¶VW&6RçfÇVR2F†R7–Ö&öÆ–2÷væW"f÷"¥dÒF—7F6‚ ¢“°¢76W'EöW€¢&VfW&Væ6W2‚g'VææW"Â&WfÇVFR"Â†#b’À¢²‚$&6R"çFõö÷væVB‚’Â&F÷V&ÆVB"çFõö÷væVB‚’Â"‚”’"çFõö÷væVB‚’•Ğ¢“°¢76W'EöW€¢&VfW&Væ6W2‚g'VææW"Â&g&öÔ&6R"Â†#b’À¢²€¢%'VææW""çFõö÷væVB‚’À¢&WfÇVFR"çFõö÷væVB‚’À¢"„Ä&6S²”’"çFõö÷væVB‚¢•Ğ¢“°¢76W'EöW€¢&VfW&Væ6W2‚g'VææW"Â&g&öÔFW&—fVB"Â†#b’À¢²€¢%'VææW""çFõö÷væVB‚’À¢&WfÇVFR"çFõö÷væVB‚’À¢"„Ä&6S²”’"çFõö÷væVB‚¢•Ğ¢“°¢76W'EöW€¢&VfW&Væ6W2‚g'VææW"Â&'&æ6‚"Â†#b’À¢°¢‚$&6R"çFõö÷væVB‚’Â&F÷V&ÆVB"çFõö÷væVB‚’Â"‚”’"çFõö÷væVB‚’’À¢‚$&6R"çFõö÷væVB‚’Â'fÇVR"çFõö÷væVB‚’Â"‚”’"çFõö÷væVB‚’’À¢Ğ¢“°¢ÆWB&öGV6Uö&6RÒVÖ—GFVEöÖWF†öEö6öFR‚g'VææW"Â'&öGV6T&6R"“°¢76W'Eö÷6öFU÷6WVVæ6R‡&öGV6Uö&6RÂe³†&"ÂƒS’Â†#rÂ†#Ò“°¢76W'B€¢&öGV6Uö&6Ræ6öçF–ç2‚c†3’À¢'&öGV6T&6RVÖ—G2æò6†V6¶67B ¢“°¢76W'B††5÷7F6µöÖ÷F&ÆR‚g'VææW"Â&'&æ6‚"’“°¢76W'EöW€¢'VææW ¢æÖWF†öG0¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂ'VææW"æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’ÓÒö²‚&'&æ6‚"’¢çVçw&‚¢æGG&–'WFW0¢æ—FW"‚¢æf–æEöÖ‡ÆGG&–'WFWÂÖF6‚GG&–'WFR°¢GG&–'WFS£¤6öFR†6öFR’Óâ6öÖR‚†6öFRæÖ…÷7F6²Â6öFRæÖ…öÆö6Ç2’’À¢òÓâæöæRÀ¢Ò’À¢6öÖR‚ƒÂ2’’À¢&'&æ6‚W6W2F†R&V6V—fW"Â&6R&ÖWFW"ÂæB&ööÆVâ&ÖWFW"6Æ÷G2 ¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB6Æ77F‚Òf—'7Eö÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…²"Ö2"Â"×b"Â"Ö6Æ77F‚"Â6Æ77F‚Â%'VææW"%Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦f&V¦V7FVB'VææW"æ6Æ75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢ÆWB¦fö÷WGWBÒ7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB“°¢76W'B†¦fö÷WGWBæ6öçF–ç2‚%7F6´ÖF&ÆR"’“°¢76W'B†¦fö÷WGWBæ6öçF–ç2‚&'&æ6‚"’“°¢76W'B†¦fö÷WGWBæ6öçF–ç2‚&FW67&—F÷#¢„Ä&6Sµ¢”’"’“°¢76W'B†¦fö÷WGWBæ6öçF–ç2‚$&6RæF÷V&ÆVC¢‚”’"’“°¢76W'B†¦fö÷WGWBæ6öçF–ç2‚$&6RçfÇVS¢‚”’"’“°¢76W'B†¦fö÷WGWBæ6öçF–ç2‚%'VææW"æWfÇVFS¢„Ä&6S²”’"’“° ¢ÆWB†&æW72Òf—'7Eö÷WGWBçF‚‚’æ¦ö–â‚$–æ†W&—Fæ6T†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ72–æ†W&—Fæ6T†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²'VææW""ÒæWr'VææW"‚“²&6R&6RÒæWr&6R‚“²&6RFW&—fVBÒæWrFW&—fVB‚“²–b‡"æWfÇVFR†&6R’Ò#’F‡&÷ræWr76W'F–öäW'&÷"…Â&&6RF—7F6…Â"“²–b‡"æWfÇVFR†FW&—fVB’ÒC"’F‡&÷ræWr76W'F–öäW'&÷"…Â&G–æÖ–2÷fW'&–FRF‡&÷Vv‚&6UÂ"“²–b‡"æg&öÔ&6R‚’Ò#ÇÂ"æg&öÔFW&—fVB‚’ÒC"’F‡&÷ræWr76W'F–öäW'&÷"…Â&6öç7G'V7F÷"&wVÖVçG5Â"“²–b†FW&—fVBçfÇVR‚’Ò#ÇÂFW&—fVBæF÷V&ÆVB‚’ÒC"’F‡&÷ræWr76W'F–öäW'&÷"…Â&–æ†W&—FVB&öG’W6W2÷fW'&–FFVâfÇVUÂ"“²–b‚‡"ç&öGV6T&6R‚’–ç7Fæ6VöbFW&—fVB’ÇÂ"ç&öGV6T&6R‚’çfÇVR‚’Ò#’F‡&÷ræWr76W'F–öäW'&÷"…Â$FW&—fVB&WGW&â2&6UÂ"“²–b‡"æ'&æ6‚†FW&—fVBÂG'VR’ÒC"ÇÂ"æ'&æ6‚†FW&—fVBÂfÇ6R’Ò#’F‡&÷ræWr76W'F–öäW'&÷"…Â$&6R&ÖWFW"'&æ6…Â"“²7—7FVÒæ÷WBç&–çFÆâ…Â'6–ævÆR–æ†W&—Fæ6RæBf—'GVÂF—7F6‚fW&–f–VEÂ"“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FR–æ†W&—Fæ6R†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â6Æ77F‚Â"ÖB"Â6Æ77F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVB–æ†W&—Fæ6R†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢6Æ77F‚À¢$–æ†W&—Fæ6T†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W""“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVB–æ†W&—Fæ6R6Æ76W5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢'6–ævÆR–æ†W&—Fæ6RæBf—'GVÂF—7F6‚fW&–f–VB ¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ–æ†W&—FVE÷f—'GVÅö6ÆÅ÷&V¦V7G5öå÷VçÆææVEöFV6Æ&–æuöÖWF†öB‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'VçÆææVBÖ–æ†W&—FVBÖ6ÆÂ×F&vWB"“°¢ÆWB6÷W&6RÒ&6Æ72&6R²FVbfÇVR‚“¢–çBÒ²FVbF÷V&ÆVB‚“¢–çBÒfÇVR‚’¢"ÕÆæ6Æ72FW&—fVBW‡FVæG2&6R·ÕÆæ6Æ72'VææW"²FVb&VB†FW&—fVC¢FW&—fVB“¢–çBÒFW&—fVBæF÷V&ÆVB‚’ÕÆâ#°¢ÆWB×WBöÖ—E÷&VçEöÖWF†öG2ÒÇVæ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÀ¢öFVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ÆWB&6RÒVæ—@¢æ6Æ76W0¢æ—FW"‚¢æf–æB‡ÇÆçÂ°¢ÆWB7–Ö&öÂÒVæ—Bç7F÷&Rç7–Ö&öÇ2ævWB‡Æâæ6Æ72“°¢Væ—Bç7F÷&RææÖW2çG'•÷&W6öÇfR‡7–Ö&öÂææÖRçFW‡B‚’’ÓÒ6öÖR‚$&6R"¢Ò¢æÖ‡ÇÆçÂÆâæ6Æ72¢æW‡V7B‚'6÷W&6R&F6‚†2&6R"“°¢Væ—Bæ6Æ76W0¢æ—FW%ö×WB‚¢æf–æB‡ÇÆçÂÆâæ6Æ72ÓÒ&6R¢æW‡V7B‚$&6RÆâ"¢æÖWF†öG0¢æ6ÆV"‚“°¢Ó°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö&F6…öö'6W'fVB€¢6÷W&6RÀ¢%VçÆææVD–æ†W&—FVD6ÆÂç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒ33’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢6öÖR‚f×WBöÖ—E÷&VçEöÖWF†öG2’À¢¢æW‡V7EöW'"‚'F†R6VÆV7FVB&VçBFV6Æ&F–öâ×W7B&VÖ–â–â—G26Æ72Æâ"“°¢76W'B€¢ÖF6†W2€¢W'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ€¢§fÔ6öFVvVäW'&÷#£¤ÖWF†öDæ÷D–ä§fÕÆâ²ââĞ¢Â§fÔ6öFVvVäW'&÷#£¤ÖWF†öDæ÷D–ä§fÕÆäf÷$&öG’²ââĞ¢¢’À¢&W‡V7FVBG—VB–æ†W&—FVBF&vWBÆâW'&÷"Âv÷B¶W'&÷#£÷Ò ¢“°¢76W'B†÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°§Ğ ¢5·FW7EĞ¦fâFW&—fVE÷fÇVW5öfÆ÷u÷Fõö&6U÷&ÖWFW'5öæE÷&WGW&ç2‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&FW&—fVB×FòÖ&6R×fÇVW2"“°¢ÆWB6÷W&6RÒ&6Æ72&6R²FVbfÇVR‚“¢–çBÒ²FVbF÷V&ÆVB‚“¢–çBÒfÇVR‚’¢"ÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²÷fW'&–FRFVbfÇVR‚“¢–çBÒ#ÕÆæ6Æ72'VææW"²FVbWfÇVFR†&6S¢&6R“¢–çBÒ&6RæF÷V&ÆVB‚“²FVbf–&6R‚“¢–çBÒWfÇVFR†æWr&6R‚’“²FVbf–FW&—fVB‚“¢–çBÒWfÇVFR†æWrFW&—fVB‚’“²FVbf–Æö6Â‚“¢–çBÒ²fÂFW&—fVBÒæWrFW&—fVB‚“²WfÇVFR†FW&—fVB’Ó²FVbf–v–FVæVDÆö6Â‚“¢–çBÒ²fÂ&6S¢&6RÒæWrFW&—fVB‚“²WfÇVFR†&6R’Ó²FVb&öGV6VB‚“¢&6RÒæWrFW&—fVB‚’ÕÆâ#°¢ÆWB'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ$FW&—fVEFô&6Rç66Æ"Â÷WGWBçF‚‚’¢æW‡V7B‚'fW&–f–VBFW&—fVBfÇVW2fÆ÷rFò&6RÖWF†öBæB&WGW&â÷6—F–öç2"“° ¢ÆWB'VææW"Ò'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ%'VææW""¢æW‡V7B‚%'VææW"6Æ72—2VÖ—GFVB"“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr‡'VææW"æ6Æ72æ'—FW2‚’“°¢ÆWB'VææW"Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFRVÖ—GFVB'VææW"6Æ72"“°¢'VææW ¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚%'VææW"&VfW&Væ6W2&VÖ–âfÆ–B"“°¢ÆWBWfÇVFRÒ'VææW ¢æÖWF†öG0¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂ'VææW"æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’ÓÒö²‚&WfÇVFR"’¢æW‡V7B‚&WfÇVFRÖWF†öB—2VÖ—GFVB"“°¢76W'EöW€¢'VææW"æ6öç7FçE÷ööÂçWFc‚†WfÇVFRæFW67&—F÷%ö–æFW‚’À¢ö²‚"„Ä&6S²”’"¢“°¢ÆWB&öGV6VBÒ'VææW ¢æÖWF†öG0¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂ'VææW"æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’ÓÒö²‚'&öGV6VB"’¢æW‡V7B‚'&öGV6VBÖWF†öB—2VÖ—GFVB"“°¢76W'EöW€¢'VææW"æ6öç7FçE÷ööÂçWFc‚‡&öGV6VBæFW67&—F÷%ö–æFW‚’À¢ö²‚"‚”Ä&6S²"¢“°¢f÷"ÖWF†öB–â²'f–FW&—fVB"Â'f–Æö6Â"Â'f–v–FVæVDÆö6Â%Ò°¢ÆWB6öFRÒVÖ—GFVEöÖWF†öEö6öFR‚g'VææW"ÂÖWF†öB“°¢ÆWB–ç7G'V7F–öç2ÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR“°¢ÆWB÷6öFW2Ò–ç7G'V7F–öç0¢æ–çFõö—FW"‚¢æÖ‡Â…òÂ÷6öFR—Â÷6öFR¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'B†÷6öFW2æ6öçF–ç2‚c†#b’Â'¶ÖWF†öGÒ–çfö¶W2WfÇVFR"“°¢76W'B‚÷6öFW2æ6öçF–ç2‚c†3’Â'¶ÖWF†öGÒW6W2æò6†V6¶67B"“°¢ÆWBöfg6WBÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ–çFõö—FW"‚¢æf–æEöÖ‡Â†öfg6WBÂ÷6öFR—Â†÷6öFRÓÒ†#b’çF†Vå÷6öÖR†öfg6WB’¢æW‡V7B‚&ÖWF†öB6öçF–ç2–çfö¶Wf—'GVÂ"“°¢ÆWB–æFW‚ÒSc£¦g&öÕö&Uö'—FW2…¶6öFU¶öfg6WB²ÒÂ6öFU¶öfg6WB²%ÕÒ“°¢ÆWB6öç7FçEööÄVçG'“£¤ÖWF†öG&Vb°¢6Æ75ö–æFW‚À¢æÖUöæE÷G—Uö–æFW‚À¢ÒÒ'VææW ¢æ6öç7FçE÷ööÀ¢ævWB„6öç7FçEööÄ–æFW‚†–æFW‚’¢æW‡V7B‚&–çfö¶Wf—'GVÂÖWF†öG&Vb"¢VÇ6R°¢æ–2‚&–çfö¶Wf—'GVÂ&VfW&Væ6W2ÖWF†öG&Vb"¢Ó°¢76W'EöW‡'VææW"æ6öç7FçE÷ööÂæ6Æ75öæÖR‚¦6Æ75ö–æFW‚’Âö²‚%'VææW""’“°¢76W'EöW€¢'VææW ¢æ6öç7FçE÷ööÀ¢ææÖUöæE÷G—R‚¦æÖUöæE÷G—Uö–æFW‚¢çVçw&‚’À¢‚&WfÇVFR"Â"„Ä&6S²”’"¢“°¢Ğ¢ÆWB&öGV6VEö÷6öFW2ÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†VÖ—GFVEöÖWF†öEö6öFR‚g'VææW"Â'&öGV6VB"’¢æ–çFõö—FW"‚¢æÖ‡Â…òÂ÷6öFR—Â÷6öFR¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'B€¢&öGV6VEö÷6öFW2æ6öçF–ç2‚c†#’À¢'&öGV6VB&WGW&ç2&VfW&Væ6R ¢“°¢76W'B€¢&öGV6VEö÷6öFW2æ6öçF–ç2‚c†3’À¢'&öGV6VBW6W2æò6†V6¶67B ¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB6Æ77F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚$FW&—fVEFô&6T†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ72FW&—fVEFô&6T†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²'VææW"'VææW"ÒæWr'VææW"‚“²–b‡'VææW"çf–&6R‚’Ò#ÇÂ'VææW"çf–FW&—fVB‚’ÒC"ÇÂ'VææW"çf–Æö6Â‚’ÒC"ÇÂ'VææW"çf–v–FVæVDÆö6Â‚’ÒC"ÇÂ'VææW"ç&öGV6VB‚’çfÇVR‚’Ò#’F‡&÷ræWr76W'F–öäW'&÷"‚“²7—7FVÒæ÷WBç&–çFÆâ…Â$FW&—fVB×FòÔ&6RfW&–f–VEÂ"“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FRFW&—fVB×FòÔ&6R†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â6Æ77F‚Â"ÖB"Â6Æ77F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVBFW&—fVB×FòÔ&6R†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢6Æ77F‚À¢$FW&—fVEFô&6T†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W""“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVBFW&—fVB×FòÔ&6R6Æ76W5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢$FW&—fVB×FòÔ&6RfW&–f–VB ¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâG&ç6—F—fUöÆVe÷fÇVW5öfÆ÷u÷F‡&÷Vv…ö&6UöÖWF†öEö&÷VæF&–W5÷v—F†÷WEö67G2‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'G&ç6—F—fRÖÆVb×FòÖ&6RÖÖWF†öBÖ&÷VæF&–W2"“°¢ÆWB6÷W&6RÒ"2&6Æ72&6R²FVbfÇVR‚“¢–çBÒ²FVbF÷V&ÆVB‚“¢–çBÒfÇVR‚’¢"Ğ¦6Æ72Ö–FFÆRW‡FVæG2&6R²÷fW'&–FRFVbfÇVR‚“¢–çBÒ#Ğ¦6Æ72ÆVbW‡FVæG2Ö–FFÆR²÷fW'&–FRFVbfÇVR‚“¢–çBÒ#Ğ¦6Æ72†öÆFW"†–æ—F–Ã¢&6R’²&—fFRf"7W'&VçC¢&6RÒ–æ—F–Ã²FVb&WÆ6R†æW‡C¢ÆVb“¢Væ—BÒ²7W'&VçBÒæW‡BÓ²FVbvWB‚“¢&6RÒ7W'&VçBĞ¦6Æ72–æ—F–Æ—¦VD†öÆFW"†–æ—F–Ã¢ÆVb’²&—fFRfÂ7W'&VçC¢&6RÒ–æ—F–Ã²FVbvWB‚“¢&6RÒ7W'&VçBĞ¦6Æ72'VææW"²FVbWfÇVFR†&6S¢&6R“¢–çBÒ&6RæF÷V&ÆVB‚“²FVbf–ÆVb‚“¢–çBÒWfÇVFR†æWrÆVb‚’“²FVbf–Æö6Â‚“¢–çBÒ²fÂ&6S¢&6RÒæWrÆVb‚“²WfÇVFR†&6R’Ó²FVb&öGV6VD&6R‚“¢&6RÒæWrÆVb‚“²FVb&öGV6VDÖ–FFÆR‚“¢Ö–FFÆRÒæWrÆVb‚“²FVbF—&V7E&WGW&â†ÆVc¢ÆVb“¢&6RÒÆVc²FVb75F‡&÷Vv‚†ÆVc¢ÆVb“¢&6RÒWfÇVFTæE&WGW&â†ÆVb“²FVbWfÇVFTæE&WGW&â†&6S¢&6R“¢&6RÒ&6S²FVb†öÆFW"†ÆVc¢ÆVb“¢†öÆFW"ÒæWr†öÆFW"†ÆVb“²FVb–æ—F–Æ—¦VD†öÆFW"†ÆVc¢ÆVb“¢–æ—F–Æ—¦VD†öÆFW"ÒæWr–æ—F–Æ—¦VD†öÆFW"†ÆVb’Ğ¢"3°¢ÆWB'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2€¢6÷W&6RÀ¢%G&ç6—F—fU&VfW&Væ6T76–væ&–Æ—G’ç66Æ"À¢÷WGWBçF‚‚’À¢¢æW‡V7B‚$ÆVbfÇVW2fÆ÷rFò&6RæBÖ–FFÆR&÷VæF&–W2F‡&÷Vv‚F†RfW&–f–VB†–W&&6‡’"“°¢ÆWB6V6öæEö÷WGWBĞ¢FW7D÷WGWDF—&V7F÷'“£¦æWr‚'G&ç6—F—fRÖÆVb×FòÖ&6RÖÖWF†öBÖ&÷VæF&–W2×&WVB"“°¢ÆWB6V6öæEö'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2€¢6÷W&6RÀ¢%G&ç6—F—fU&VfW&Væ6T76–væ&–Æ—G’ç66Æ"À¢6V6öæEö÷WGWBçF‚‚’À¢¢æW‡V7B‚&g&W6‚g&öçFVæB6W76–öâ6ö×–ÆW2F†R6ÖRæ6W7G'’FWFW&Ö–æ—7F–6ÆÇ’"“°¢76W'EöW†'F–f7G2æÆVâ‚’Â6V6öæEö'F–f7G2æÆVâ‚’“°¢f÷"†f—'7BÂ6V6öæB’–â'F–f7G2æ—FW"‚’ç¦—‚g6V6öæEö'F–f7G2’°¢76W'EöW†f—'7Bæ6Æ72æ–çFW&æÅöæÖR‚’Â6V6öæBæ6Æ72æ–çFW&æÅöæÖR‚’“°¢76W'EöW†f—'7Bæ6Æ72æ'—FW2‚’Â6V6öæBæ6Æ72æ'—FW2‚’“°¢Ğ¢ÆWB'VææW%ö'F–f7BÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ%'VææW""¢æW‡V7B‚%'VææW"—2VÖ—GFVB"“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr‡'VææW%ö'F–f7Bæ6Æ72æ'—FW2‚’“°¢ÆWB'VææW"Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFRVÖ—GFVB'VææW"6Æ72"“°¢'VææW ¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚'G&ç6—F—fRÖWF†öBFW67&—F÷'2&VÖ–âfÆ–B"“° ¢f÷"†æÖRÂFW67&—F÷"’–â°¢‚&WfÇVFR"Â"„Ä&6S²”’"’À¢‚'&öGV6VD&6R"Â"‚”Ä&6S²"’À¢‚'&öGV6VDÖ–FFÆR"Â"‚”ÄÖ–FFÆS²"’À¢‚&F—&V7E&WGW&â"Â"„ÄÆVc²”Ä&6S²"’À¢‚'75F‡&÷Vv‚"Â"„ÄÆVc²”Ä&6S²"’À¢Ò°¢ÆWBÖWF†öBÒ'VææW ¢æÖWF†öG0¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂ'VææW"æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’ÓÒö²†æÖR’¢çVçw&ö÷%öVÇ6R‡ÇÂæ–2‚'¶æÖWÒÖWF†öB—2VÖ—GFVB"’“°¢76W'EöW€¢'VææW"æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBæFW67&—F÷%ö–æFW‚’À¢ö²†FW67&—F÷"¢“°¢ÆWB–ç7G'V7F–öç2ÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†VÖ—GFVEöÖWF†öEö6öFR‚g'VææW"ÂæÖR’“°¢76W'B€¢–ç7G'V7F–öç2æ—FW"‚’æÆÂ‡Â…òÂ÷6öFR—Â¦÷6öFRÒ†3’À¢'¶æÖWÒVÖ—G2æò6†V6¶67B ¢“°¢Ğ¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢g'VææW"À¢VÖ—GFVEöÖWF†öEö6öFR‚g'VææW"Â'f–ÆVb"’À¢e°¢ƒ†#rÂ$ÆVb"Â#Æ–æ—Câ"Â"‚•b"’À¢ƒ†#bÂ%'VææW""Â&WfÇVFR"Â"„Ä&6S²”’"’À¢ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢g'VææW"À¢VÖ—GFVEöÖWF†öEö6öFR‚g'VææW"Â'f–Æö6Â"’À¢e°¢ƒ†#rÂ$ÆVb"Â#Æ–æ—Câ"Â"‚•b"’À¢ƒ†#bÂ%'VææW""Â&WfÇVFR"Â"„Ä&6S²”’"’À¢ÒÀ¢“°¢76W'B€¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2†VÖ—GFVEöÖWF†öEö6öFR‚g'VææW"Â'&öGV6VD&6R"’¢æ—FW"‚¢æç’‡Â…òÂ÷6öFR—Â¦÷6öFRÓÒ†#’À¢$&6RÖFV6Æ&VB&W7VÇB&WGW&ç2F†R7GVÂÆVb&VfW&Væ6R ¢“° ¢f÷"†6Æ75öæÖRÂ6öç7G'V7F÷%öFW67&—F÷"’–à¢²‚$†öÆFW""Â"„Ä&6S²•b"’Â‚$–æ—F–Æ—¦VD†öÆFW""Â"„ÄÆVc²•b"•Ğ¢°¢ÆWB'F–f7BÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ6Æ75öæÖR¢çVçw&ö÷%öVÇ6R‡ÇÂæ–2‚'¶6Æ75öæÖWÒ—2VÖ—GFVB"’“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†'F–f7Bæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFR&VfW&Væ6RÖ†öÆFW"6Æ72"“°¢ÆWB6öç7G'V7F÷"Ò6Æ70¢æÖWF†öG0¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂ6Æ72æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’ÓÒö²‚#Æ–æ—Câ"’¢æW‡V7B‚&†öÆFW"6öç7G'V7F÷"—2VÖ—GFVB"“°¢76W'EöW€¢6Æ72æ6öç7FçE÷ööÂçWFc‚†6öç7G'V7F÷"æFW67&—F÷%ö–æFW‚’À¢ö²†6öç7G'V7F÷%öFW67&—F÷"¢“°¢ÆWB7W'&VçEöf–VÆBÒ6Æ70¢æf–VÆG0¢æ—FW"‚¢æf–æB‡Æf–VÆGÂ6Æ72æ6öç7FçE÷ööÂçWFc‚†f–VÆBææÖUö–æFW‚’ÓÒö²‚&7W'&VçB"’¢æW‡V7B‚&†öÆFW"7F÷&W2&6Rf–VÆB"“°¢76W'EöW€¢6Æ72æ6öç7FçE÷ööÂçWFc‚†7W'&VçEöf–VÆBæFW67&—F÷%ö–æFW‚’À¢ö²‚$Ä&6S²"¢“°¢76W'Eöf–VÆE÷&VfW&Væ6R€¢f6Æ72À¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â#Æ–æ—Câ"’À¢†#RÀ¢6Æ75öæÖRÀ¢&7W'&VçB"À¢$Ä&6S²"À¢“°¢Ğ¢ÆWB†öÆFW%ö'F–f7BÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$†öÆFW""¢æW‡V7B‚$†öÆFW"—2VÖ—GFVB"“°¢ÆWB×WB†öÆFW%÷&VFW"Ò&VFW#£¦æWr††öÆFW%ö'F–f7Bæ6Æ72æ'—FW2‚’“°¢ÆWB†öÆFW"Ò6Æ74f–ÆS£¦FV6öFR‚f×WB†öÆFW%÷&VFW"’æW‡V7B‚&FV6öFR†öÆFW"6Æ72"“°¢76W'Eöf–VÆE÷&VfW&Væ6R€¢f†öÆFW"À¢VÖ—GFVEöÖWF†öEö6öFR‚f†öÆFW"Â'&WÆ6R"’À¢†#RÀ¢$†öÆFW""À¢&7W'&VçB"À¢$Ä&6S²"À¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢g'VææW"À¢VÖ—GFVEöÖWF†öEö6öFR‚g'VææW"Â&†öÆFW""’À¢e²ƒ†#rÂ$†öÆFW""Â#Æ–æ—Câ"Â"„Ä&6S²•b"•ÒÀ¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB6Æ77F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGW@¢çF‚‚¢æ¦ö–â‚%G&ç6—F—fU&VfW&Væ6T76–væ&–Æ—G”†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ72G&ç6—F—fU&VfW&Væ6T76–væ&–Æ—G”†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²'VææW"'VææW"ÒæWr'VææW"‚“²ÆVbÆVbÒæWrÆVb‚“²–b‡'VææW"æWfÇVFR†ÆVb’ÒC"ÇÂ'VææW"çf–ÆVb‚’ÒC"ÇÂ'VææW"çf–Æö6Â‚’ÒC"ÇÂ'VææW"æF—&V7E&WGW&â†ÆVb’ÒÆVbÇÂ'VææW"ç75F‡&÷Vv‚†ÆVb’ÒÆVb’F‡&÷ræWr76W'F–öäW'&÷"‚“²–b‚‡'VææW"ç&öGV6VD&6R‚’–ç7Fæ6VöbÆVb’ÇÂ'VææW"ç&öGV6VD&6R‚’çfÇVR‚’Ò#’F‡&÷ræWr76W'F–öäW'&÷"‚“²–b‚‡'VææW"ç&öGV6VDÖ–FFÆR‚’–ç7Fæ6VöbÆVb’’F‡&÷ræWr76W'F–öäW'&÷"‚“²†öÆFW"†öÆFW"Ò'VææW"æ†öÆFW"†ÆVb“²†öÆFW"ç&WÆ6R†ÆVb“²–b††öÆFW"ævWB‚’ÒÆVb’F‡&÷ræWr76W'F–öäW'&÷"‚“²–æ—F–Æ—¦VD†öÆFW"–æ—F–Æ—¦VBÒ'VææW"æ–æ—F–Æ—¦VD†öÆFW"†ÆVb“²–b†–æ—F–Æ—¦VBævWB‚’ÒÆVb’F‡&÷ræWr76W'F–öäW'&÷"‚“²7—7FVÒæ÷WBç&–çFÆâ…Â'G&ç6—F—fR&VfW&Væ6R76–væ&–Æ—G’fW&–f–VEÂ"“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FRG&ç6—F—fRÖ76–væ&–Æ—G’†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â6Æ77F‚Â"ÖB"Â6Æ77F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVBG&ç6—F—fRÖ76–væ&–Æ—G’†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢6Æ77F‚À¢%G&ç6—F—fU&VfW&Væ6T76–væ&–Æ—G”†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W""“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVBG&ç6—F—fR×&VfW&Væ6R6Æ76W5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢'G&ç6—F—fR&VfW&Væ6R76–væ&–Æ—G’fW&–f–VB ¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâFW&—fVE÷fÇVW5öfÆ÷u÷Fõ÷7W÷'FVEö6öç7G'V7F÷%ö&wVÖVçG5öæE÷&VfW&Væ6Uöf–VÆG2‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&FW&—fVB×FòÖ&6RÖ6öç7G'V7F÷"ÖæBÖf–VÆB"“°¢ÆWB6÷W&6RÒ&6Æ72&6R²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²÷fW'&–FRFVbfÇVR‚“¢–çBÒC"ÕÆæ6Æ72†öÆFW"†–æ—F–Ã¢&6R’²&—fFRf"7W'&VçC¢&6RÒ–æ—F–Ã²FVb&WÆ6R†æW‡C¢FW&—fVB“¢Væ—BÒ²7W'&VçBÒæW‡BÓ²FVbvWB‚“¢&6RÒ7W'&VçBÕÆæ6Æ72–æ—F–Æ—¦VD†öÆFW"†–æ—F–Ã¢FW&—fVB’²&—fFRfÂ7W'&VçC¢&6RÒ–æ—F–Ã²FVbvWB‚“¢&6RÒ7W'&VçBÕÆæ6Æ72'VææW"²FVbg&öÕ&ÖWFW"†æW‡C¢FW&—fVB“¢†öÆFW"ÒæWr†öÆFW"†æW‡B“²FVbg&öÔÆö6Â‚“¢†öÆFW"Ò²fÂæW‡BÒæWrFW&—fVB‚“²æWr†öÆFW"†æW‡B’Ó²FVb–æ—F–Æ—¦VB†æW‡C¢FW&—fVB“¢–æ—F–Æ—¦VD†öÆFW"ÒæWr–æ—F–Æ—¦VD†öÆFW"†æW‡B’ÕÆâ#°¢ÆWB'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2€¢6÷W&6RÀ¢$FW&—fVEFô&6T6öç7G'V7F÷$æDf–VÆBç66Æ"À¢÷WGWBçF‚‚’À¢¢æW‡V7B‚'fW&–f–VBFW&—fVBfÇVW2fÆ÷rFò7W÷'FVB6öç7G'V7F÷"æBf–VÆB÷6—F–öç2"“° ¢ÆWB†öÆFW"Ò'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$†öÆFW""¢æW‡V7B‚$†öÆFW"6Æ72—2VÖ—GFVB"“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr††öÆFW"æ6Æ72æ'—FW2‚’“°¢ÆWB†öÆFW"Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFRVÖ—GFVB†öÆFW"6Æ72"“°¢ÆWB6öç7G'V7F÷"Ò†öÆFW ¢æÖWF†öG0¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂ†öÆFW"æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’ÓÒö²‚#Æ–æ—Câ"’¢æW‡V7B‚$†öÆFW"6öç7G'V7F÷"—2VÖ—GFVB"“°¢76W'EöW€¢†öÆFW"æ6öç7FçE÷ööÂçWFc‚†6öç7G'V7F÷"æFW67&—F÷%ö–æFW‚’À¢ö²‚"„Ä&6S²•b"¢“°¢ÆWB&WÆ6RÒVÖ—GFVEöÖWF†öEö6öFR‚f†öÆFW"Â'&WÆ6R"“°¢76W'Eöf–VÆE÷&VfW&Væ6R‚f†öÆFW"Â&WÆ6RÂ†#RÂ$†öÆFW""Â&7W'&VçB"Â$Ä&6S²"“° ¢ÆWB–æ—F–Æ—¦VEö†öÆFW"Ò'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$–æ—F–Æ—¦VD†öÆFW""¢æW‡V7B‚$–æ—F–Æ—¦VD†öÆFW"6Æ72—2VÖ—GFVB"“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†–æ—F–Æ—¦VEö†öÆFW"æ6Æ72æ'—FW2‚’“°¢ÆWB–æ—F–Æ—¦VEö†öÆFW"Ğ¢6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFRVÖ—GFVB–æ—F–Æ—¦VD†öÆFW"6Æ72"“°¢76W'EöW€¢–æ—F–Æ—¦VEö†öÆFW ¢æf–VÆG0¢æ—FW"‚¢æf–æB‡Æf–VÆGÂ–æ—F–Æ—¦VEö†öÆFW"æ6öç7FçE÷ööÂçWFc‚†f–VÆBææÖUö–æFW‚’ÓÒö²‚&7W'&VçB"’¢æÖ‡Æf–VÆGÂ–æ—F–Æ—¦VEö†öÆFW ¢æ6öç7FçE÷ööÀ¢çWFc‚†f–VÆBæFW67&—F÷%ö–æFW‚’’À¢6öÖR„ö²‚$Ä&6S²"’¢“°¢ÆWB6öç7G'V7F÷"ÒVÖ—GFVEöÖWF†öEö6öFR‚f–æ—F–Æ—¦VEö†öÆFW"Â#Æ–æ—Câ"“°¢76W'Eöf–VÆE÷&VfW&Væ6R€¢f–æ—F–Æ—¦VEö†öÆFW"À¢6öç7G'V7F÷"À¢†#RÀ¢$–æ—F–Æ—¦VD†öÆFW""À¢&7W'&VçB"À¢$Ä&6S²"À¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB6Æ77F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGW@¢çF‚‚¢æ¦ö–â‚$FW&—fVEFô&6T6öç7G'V7F÷$æDf–VÆD†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ72FW&—fVEFô&6T6öç7G'V7F÷$æDf–VÆD†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²'VææW"'VææW"ÒæWr'VææW"‚“²†öÆFW"g&öÕ&ÖWFW"Ò'VææW"æg&öÕ&ÖWFW"†æWrFW&—fVB‚’“²–b†g&öÕ&ÖWFW"ævWB‚’çfÇVR‚’ÒC"’F‡&÷ræWr76W'F–öäW'&÷"…Â&6öç7G'V7F÷"&ÖWFW%Â"“²g&öÕ&ÖWFW"ç&WÆ6R†æWrFW&—fVB‚’“²–b†g&öÕ&ÖWFW"ævWB‚’çfÇVR‚’ÒC"’F‡&÷ræWr76W'F–öäW'&÷"…Â'&VfW&Væ6Rf–VÆB76–væÖVçEÂ"“²–b‡'VææW"æg&öÔÆö6Â‚’ævWB‚’çfÇVR‚’ÒC"’F‡&÷ræWr76W'F–öäW'&÷"…Â&6öç7G'V7F÷"Æö6ÅÂ"“²–b‡'VææW"æ–æ—F–Æ—¦VB†æWrFW&—fVB‚’’ævWB‚’çfÇVR‚’ÒC"’F‡&÷ræWr76W'F–öäW'&÷"…Â'&VfW&Væ6Rf–VÆB–æ—F–Æ—¦W%Â"“²7—7FVÒæ÷WBç&–çFÆâ…Â$FW&—fVB×FòÔ&6R6öç7G'V7F÷"æBf–VÆBfW&–f–VEÂ"“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FR6öç7G'V7F÷"æBf–VÆB†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â6Æ77F‚Â"ÖB"Â6Æ77F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVB6öç7G'V7F÷"æBf–VÆB†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢6Æ77F‚À¢$FW&—fVEFô&6T6öç7G'V7F÷$æDf–VÆD†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W""“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVB6öç7G'V7F÷"æBf–VÆB6Æ76W5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢$FW&—fVB×FòÔ&6R6öç7G'V7F÷"æBf–VÆBfW&–f–VB ¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ6÷W&6U÷G—–æu÷&V¦V7G5÷&WfW'6UöæE÷Vç&VÆFVE÷&VfW&Væ6Uö&wVÖVçG2‚’°¢f÷"†Æ&VÂÂ6÷W&6R’–â°¢€¢&&6R×FòÖFW&—fVBÖ&wVÖVçB"À¢&6Æ72&6R²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R·ÕÆæ6Æ72'VææW"²FVbW6R‡fÇVS¢FW&—fVB“¢–çBÒfÇVRçfÇVR‚“²FVb–çfÆ–B†&6S¢&6R“¢–çBÒW6R†&6R’ÕÆâ"À¢’À¢€¢&&6R×FòÖÆVbÖ&wVÖVçB×F‡&÷Vv‚ÖÖ–FFÆR"À¢&6Æ72&6R²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72Ö–FFÆRW‡FVæG2&6R·ÕÆæ6Æ72ÆVbW‡FVæG2Ö–FFÆR·ÕÆæ6Æ72'VææW"²FVbW6R‡fÇVS¢ÆVb“¢–çBÒfÇVRçfÇVR‚“²FVb–çfÆ–B†&6S¢&6R“¢–çBÒW6R†&6R’ÕÆâ"À¢’À¢€¢'Vç&VÆFVB×FòÖ&6RÖ&wVÖVçB"À¢&6Æ72&6R²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72÷F†W"·ÕÆæ6Æ72'VææW"²FVbW6R‡fÇVS¢&6R“¢–çBÒfÇVRçfÇVR‚“²FVb–çfÆ–B†÷F†W#¢÷F†W"“¢–çBÒW6R†÷F†W"’ÕÆâ"À¢’À¢Ò°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr†Æ&VÂ“°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂff÷&ÖB‚'¶Æ&VÇÒç66Æ"’Â÷WGWBçF‚‚’¢æW‡V7EöW'"‚'F†R6÷W&6RG—W"×W7B&V¦V7BVç&÷fVB&VfW&Væ6R6öçfW'6–öç2"“°¢76W'B€¢ÖF6†W2€¢fW'&÷"À¢—VÆ–æTf–ÇW&S£¥G—W"…G—W$W'&÷#£¤Æ–6F–öä&wVÖVçEG—TÖ—6ÖF6‚²ââÒ¢’À¢'¶Æ&VÇÒ6†÷VÆBf–Â–âF†RG—W"Âv÷B¶W'&÷#£÷Ò ¢“°¢76W'B€¢÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’À¢'¶Æ&VÇÒVÖ—G2æò6Æ72f–ÆW2 ¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ&VfW&Væ6U÷&ÖWFW'5öæEö6ÆÅ÷&W7VÇG5öfÆ÷u÷F‡&÷Vv…÷f—'GVÅö6ÆÇ2‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&VfW&Væ6RÖÖWF†öB×fÇVW2"“°¢ÆWB6÷W&6RÒ&6Æ726÷VçFW"‡7F'C¢–çB“¥ÆâFVb7&VFR‡fÇVS¢–çB“¢6÷VçFW"ÒæWr6÷VçFW"‡fÇVR•ÆâFVb77F‡&÷Vv‚†6÷VçFW#¢6÷VçFW"“¢6÷VçFW"Ò6÷VçFW%ÆâFVb6VÆb‚“¢6÷VçFW"ÒF†—5ÆâFVb&VB†6÷VçFW#¢6÷VçFW"“¢–çBÒ6÷VçFW"ævWB‚•ÆâFVbf÷'v&B†6÷VçFW#¢6÷VçFW"“¢6÷VçFW"Ò77F‡&÷Vv‚†6÷VçFW"•ÆâFVb&VD7&VFVB‡fÇVS¢–çB“¢–çBÒ&VB†7&VFR‡fÇVR’•ÆâFVbvWB‚“¢–çBÒC%Ææ6Æ726Æ–VçC¥ÆâFVb66WB†6÷VçFW#¢6÷VçFW"“¢6÷VçFW"Ò6÷VçFW%ÆâFVb&VB†6÷VçFW#¢6÷VçFW"“¢–çBÒ6÷VçFW"ævWB‚•Æâ#°¢ÆWBf—'7BÒ6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ%&VfW&Væ6TÖWF†öG2ç66Æ"Â÷WGWBçF‚‚’¢æW‡V7B‚'ÆææVB&VfW&Væ6R×fÇVVBÖWF†öG2æB6ÆÇ26ö×–ÆR2öæR&F6‚"“°¢ÆWB6V6öæEö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&VfW&Væ6RÖÖWF†öB×fÇVW2×&WVB"“°¢ÆWB6V6öæBÒ6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ%&VfW&Væ6TÖWF†öG2ç66Æ"Â6V6öæEö÷WGWBçF‚‚’¢æW‡V7B‚&g&W6‚g&öçFVæB6W76–öâ6ö×–ÆW2&VfW&Væ6R×fÇVVBÖWF†öG2–FVçF–6ÆÇ’"“°¢76W'EöW€¢f—'7@¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢²$6÷VçFW""Â$6Æ–VçB%Ğ¢“°¢76W'EöW†f—'7BæÆVâ‚’Â6V6öæBæÆVâ‚’“°¢f÷"†ÆVgBÂ&–v‡B’–âf—'7Bæ—FW"‚’ç¦—‚g6V6öæB’°¢76W'EöW†ÆVgBæ6Æ72æ'—FW2‚’Â&–v‡Bæ6Æ72æ'—FW2‚’“°¢Ğ ¢ÆWB6÷VçFW%ö'F–f7BÒf—'7@¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$6÷VçFW""¢çVçw&‚“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†6÷VçFW%ö'F–f7Bæ6Æ72æ'—FW2‚’“°¢ÆWB6÷VçFW"Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFR6÷VçFW""“°¢6÷VçFW ¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚$6÷VçFW"&VfW&Væ6W2fÆ–FFR"“°¢76W'Eö÷6öFU÷6WVVæ6R€¢VÖ—GFVEöÖWF†öEö6öFR‚f6÷VçFW"Â&7&VFR"’À¢e³†&"ÂƒS’Âƒ"Â†#rÂ†#ÒÀ¢“°¢76W'Eö÷6öFU÷6WVVæ6R†VÖ—GFVEöÖWF†öEö6öFR‚f6÷VçFW"Â'77F‡&÷Vv‚"’Âe³ƒ&"Â†#Ò“°¢76W'Eö÷6öFU÷6WVVæ6R†VÖ—GFVEöÖWF†öEö6öFR‚f6÷VçFW"Â'6VÆb"’Âe³ƒ&Â†#Ò“°¢76W'Eö÷6öFU÷6WVVæ6R†VÖ—GFVEöÖWF†öEö6öFR‚f6÷VçFW"Â'&VB"’Âe³ƒ&"Â†#bÂ†5Ò“°¢76W'Eö÷6öFU÷6WVVæ6R€¢VÖ—GFVEöÖWF†öEö6öFR‚f6÷VçFW"Â&f÷'v&B"’À¢e³ƒ&Âƒ&"Â†#bÂ†#ÒÀ¢“°¢76W'Eö÷6öFU÷6WVVæ6R€¢VÖ—GFVEöÖWF†öEö6öFR‚f6÷VçFW"Â'&VD7&VFVB"’À¢e³ƒ&Âƒ&Âƒ"Â†#bÂ†#bÂ†5ÒÀ¢“° ¢ÆWB6Æ–VçEö'F–f7BÒf—'7@¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$6Æ–VçB"¢çVçw&‚“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†6Æ–VçEö'F–f7Bæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ–VçBÒ6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFR6Æ–VçB"“°¢6Æ–Vç@¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚$6Æ–VçB&VfW&Væ6W2fÆ–FFR"“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚%&VfW&Væ6TÖWF†öG4†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ72&VfW&Væ6TÖWF†öG4†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²6÷VçFW"6÷VçFW"ÒæWr6÷VçFW"ƒ“²6Æ–VçB6Æ–VçBÒæWr6Æ–VçB‚“²–b†6÷VçFW"æ7&VFRƒ’ævWB‚’ÒC"ÇÂ6÷VçFW"ç77F‡&÷Vv‚†6÷VçFW"’ævWB‚’ÒC"ÇÂ6÷VçFW"ç6VÆb‚’ævWB‚’ÒC"ÇÂ6÷VçFW"ç&VB†6÷VçFW"’ÒC"ÇÂ6÷VçFW"æf÷'v&B†6÷VçFW"’ævWB‚’ÒC"ÇÂ6÷VçFW"ç&VD7&VFVBƒ"’ÒC"’F‡&÷ræWr76W'F–öäW'&÷"…Â$6÷VçFW%Â"“²–b†6Æ–VçBç&VB†6÷VçFW"’ÒC"ÇÂ6Æ–VçBæ66WB†6÷VçFW"’ævWB‚’ÒC"’F‡&÷ræWr76W'F–öäW'&÷"…Â$6Æ–VçEÂ"“²7—7FVÒæ÷WBç&–çFÆâ…Â'&VfW&Væ6RÖWF†öG2fW&–f–VEÂ"“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FR&VfW&Væ6RÖÖWF†öBfW&–f–6F–öâ†&æW72"“°¢ÆWB6Æ77F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’çVçw&‚“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â6Æ77F‚Â"ÖB"Â6Æ77F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2f÷"&VfW&Væ6RÖWF†öG2"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVB&VfW&Væ6RÖÖWF†öB†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢6Æ77F‚À¢%&VfW&Væ6TÖWF†öG4†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¥dÒfW&–f–W"f÷"&VfW&Væ6RÖWF†öG2"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVB&VfW&Væ6RÖWF†öG5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢'&VfW&Væ6RÖWF†öG2fW&–f–VB ¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ&VfW&Væ6U÷&V6V—fW%ö6ÆÇ5ö¶VW÷&—fFUö66W75÷&V¦V7FVEöæEöÆÆ÷u÷fW&–f–VEö'&æ6†W2‚’°¢ÆWB&—fFUö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&—fFRÖ7&÷72Ö6Æ72×f—'GVÂÖ6ÆÂ"“°¢ÆWB&—fFU÷&W7VÇBÒ6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R€¢&6Æ726÷VçFW"‡6VVC¢–çB“¥Æâ&—fFRFVb†–FFVâ‚“¢–çBÒ•Ææ6Æ726Æ–VçC¥ÆâFVb'Vâ‡6VVC¢–çB“¢–çBÒæWr6÷VçFW"‡6VVB’æ†–FFVâ‚•Æâ"À¢&—fFUö÷WGWBçF‚‚’À¢“°¢76W'B€¢ÖF6†W2€¢&—fFU÷&W7VÇBÀ¢W'"…—VÆ–æTf–ÇW&S£¤6öFVvVâ€¢§fÔ6öFVvVäW'&÷#£¥Vç7W÷'FVD§fÕf—6–&–Æ—G’°¢f—6–&–Æ—G“¢F÷GG•ö6÷&S£¥f—6–&–Æ—G“£¥&—fFRÀ¢âà¢Ğ¢’¢’À¢&&6¶VæB×W7B&V¦V7B&—fFR7&÷72Ö6Æ726ÆÇ2Âv÷B·&—fFU÷&W7VÇC£÷Ò ¢“°¢76W'B‡&—fFUö÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“° ¢ÆWB'&æ6…ö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&VfW&Væ6RÖ6ÆÂÖ'&æ6‚"“°¢ÆWB'&æ6…÷&W7VÇBÒ6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R€¢&6Æ726÷VçFW"‡6VVC¢–çB“¥ÆâFVbvWB‚“¢–çBÒC%Ææ6Æ726Æ–VçC¥ÆâFVb'Vâ†fÆs¢&ööÆVâÂ6VVC¢–çB“¢–çBÕÆâfÂ6÷VçFW"ÒæWr6÷VçFW"‡6VVB•Æâ–bfÆrF†Vâ6÷VçFW"ævWB‚’VÇ6RÆâ"À¢'&æ6…ö÷WGWBçF‚‚’À¢“°¢ÆWB'&æ6…ö'F–f7G2Ğ¢'&æ6…÷&W7VÇBæW‡V7B‚'ÆææVB&VfW&Væ6RÆö6Â6ÆÇ2&RfW&–f–VB–ç6–FRâ–b'&æ6‚"“°¢76W'B€¢'&æ6…ö'F–f7G0¢æ—FW"‚¢æç’‡Æ'F–f7GÂ²'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$6Æ–VçB"Ò¢“°§Ğ ¢5·FW7EĞ¦fâ&VfW&Væ6U÷&ÖWFW'5öæE÷&W7VÇG5ö6ö×÷6U÷v—F…ö–eöæE÷v†–ÆUö6öçG&öÅöfÆ÷r‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&VfW&Væ6R×&ÖWFW"×&W7VÇBÖ6öçG&öÂÖfÆ÷r"“°¢ÆWB6÷W&6RÒ&6Æ726÷VçFW"‡7F'C¢–çB“¥Æâ&—fFRf"7W'&VçC¢–çBÒ7F'EÆâFVbvWB‚“¢–çBÒ7W'&VçEÆâFVb–æ7&VÖVçB‚“¢Væ—BÒ7W'&VçBÒ7W'&VçB²Ææ6Æ726÷VçFW$6öçG&öÃ¥ÆâFVb72†6÷VçFW#¢6÷VçFW"“¢6÷VçFW"Ò6÷VçFW%ÆâFVb6†ö÷6R†fÆs¢&ööÆVâÂf—'7C¢6÷VçFW"Â6V6öæC¢6÷VçFW"“¢6÷VçFW"ÕÆâ–bfÆrF†Vâf—'7BVÇ6R6V6öæEÆâFVb6†ö÷6UF‡&÷Vv„6ÆÂ†fÆs¢&ööÆVâÂf—'7C¢6÷VçFW"Â6V6öæC¢6÷VçFW"“¢6÷VçFW"ÕÆâ–bfÆrF†VåÆâfÂÆÆö6FVBÒæWr6÷VçFW"ƒ’•Æâ72†f—'7B•ÆâVÇ6R6V6öæEÆâFVb–æ7&VÖVçEVçF–Â†6÷VçFW#¢6÷VçFW"ÂÆ–Ö—C¢–çB“¢–çBÕÆâv†–ÆR6÷VçFW"ævWB‚’ÂÆ–Ö—BFò6÷VçFW"æ–æ7&VÖVçB‚•Æâ6÷VçFW"ævWB‚•ÆâFVb–æ7&VÖVçEv†–ÆR†Væ&ÆVC¢&ööÆVâÂ6÷VçFW#¢6÷VçFW"ÂÆ–Ö—C¢–çB“¢–çBÕÆâv†–ÆRVæ&ÆVBbb6÷VçFW"ævWB‚’ÂÆ–Ö—BFò6÷VçFW"æ–æ7&VÖVçB‚•Æâ6÷VçFW"ævWB‚•Æâ#°¢ÆWBf—'7BÒ6ö×–ÆU÷6÷W&6Uö6Æ76W2€¢6÷W&6RÀ¢%&VfW&Væ6U&ÖWFW%&W7VÇD6öçG&öÄfÆ÷rç66Æ"À¢÷WGWBçF‚‚’À¢¢æW‡V7B‚&W†7B&VfW&Væ6RÖWF†öBfÇVW26ö×÷6RF‡&÷Vv‚–bæBv†–ÆR"“°¢ÆWB6V6öæEö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&VfW&Væ6R×&ÖWFW"×&W7VÇBÖ6öçG&öÂÖfÆ÷r×&WVB"“°¢ÆWB6V6öæBÒ6ö×–ÆU÷6÷W&6Uö6Æ76W2€¢6÷W&6RÀ¢%&VfW&Væ6U&ÖWFW%&W7VÇD6öçG&öÄfÆ÷rç66Æ"À¢6V6öæEö÷WGWBçF‚‚’À¢¢æW‡V7B‚&g&W6‚g&öçFVæBVÖ—G2F†R6ÖR&VfW&Væ6R6öçG&öÂÖfÆ÷r6Æ76W2"“°¢76W'EöW†f—'7BæÆVâ‚’Â"“°¢76W'EöW‡6V6öæBæÆVâ‚’Â"“°¢f÷"†ÆVgBÂ&–v‡B’–âf—'7Bæ—FW"‚’ç¦—‚g6V6öæB’°¢76W'EöW†ÆVgBæ6Æ72æ'—FW2‚’Â&–v‡Bæ6Æ72æ'—FW2‚’“°¢Ğ ¢ÆWB6÷VçFW%ö'F–f7BÒf—'7@¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$6÷VçFW""¢çVçw&‚“°¢ÆWB6÷VçFW"ÒFV6öFUöVÖ—GFVEö6Æ72†6÷VçFW%ö'F–f7B“°¢6÷VçFW ¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚$6÷VçFW"&VfW&Væ6W2fÆ–FFR"“° ¢ÆWB6öçG&öÅö'F–f7BÒf—'7@¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$6÷VçFW$6öçG&öÂ"¢çVçw&‚“°¢ÆWB6öçG&öÂÒFV6öFUöVÖ—GFVEö6Æ72†6öçG&öÅö'F–f7B“°¢6öçG&öÀ¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚$6÷VçFW$6öçG&öÂ&VfW&Væ6W2fÆ–FFR"“°¢76W'EöW€¢ÖWF†öG5÷v—F…öFW67&—F÷'2‚f6öçG&öÂ’À¢°¢‚#Æ–æ—Câ"çFõö÷væVB‚’Â"‚•b"çFõö÷væVB‚’’À¢‚'72"çFõö÷væVB‚’Â"„Ä6÷VçFW#²”Ä6÷VçFW#²"çFõö÷væVB‚’’À¢€¢&6†ö÷6R"çFõö÷væVB‚’À¢"…¤Ä6÷VçFW#´Ä6÷VçFW#²”Ä6÷VçFW#²"çFõö÷væVB‚¢’À¢€¢&6†ö÷6UF‡&÷Vv„6ÆÂ"çFõö÷væVB‚’À¢"…¤Ä6÷VçFW#´Ä6÷VçFW#²”Ä6÷VçFW#²"çFõö÷væVB‚¢’À¢‚&–æ7&VÖVçEVçF–Â"çFõö÷væVB‚’Â"„Ä6÷VçFW#´’”’"çFõö÷væVB‚’’À¢‚&–æ7&VÖVçEv†–ÆR"çFõö÷væVB‚’Â"…¤Ä6÷VçFW#´’”’"çFõö÷væVB‚’’À¢Ğ¢“° ¢f÷"ÖWF†öB–â²&6†ö÷6R"Â&6†ö÷6UF‡&÷Vv„6ÆÂ%Ò°¢ÆWB6öFRÒVÖ—GFVEöÖWF†öEö6öFR‚f6öçG&öÂÂÖWF†öB“°¢76W'EöW†6öFRæÆ7B‚’Â6öÖR‚c†#’Â'¶ÖWF†öGÒ&WGW&ç2v—F‚&WGW&â"“°¢76W'B€¢7F6µöÖög&ÖW2‚f6öçG&öÂÂÖWF†öB’æ—FW"‚’æç’‡Æg&ÖWÂ°¢g&ÖRç7F6²ÓÒµ7F6´ÖfW&–f–6F–öåG—S£¤ö&¦V7B‚$6÷VçFW""çFõö÷væVB‚’•Ğ¢Ò’À¢'¶ÖWF†öGÒ¦ö–ç2&÷F‚'&æ6†W2v—F‚W†7Bö&¦V7B„6÷VçFW"’öâF†R7F6² ¢“°¢Ğ¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f6öçG&öÂÀ¢VÖ—GFVEöÖWF†öEö6öFR‚f6öçG&öÂÂ&6†ö÷6UF‡&÷Vv„6ÆÂ"’À¢e°¢ƒ†#rÂ$6÷VçFW""Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#bÂ$6÷VçFW$6öçG&öÂ"Â'72"Â"„Ä6÷VçFW#²”Ä6÷VçFW#²"’À¢ÒÀ¢“° ¢f÷"†ÖWF†öBÂW‡V7FVEöÆö6Ç2’–â°¢€¢&–æ7&VÖVçEVçF–Â"À¢fV2°¢7F6´ÖfW&–f–6F–öåG—S£¤ö&¦V7B‚$6÷VçFW$6öçG&öÂ"çFõö÷væVB‚’’À¢7F6´ÖfW&–f–6F–öåG—S£¤ö&¦V7B‚$6÷VçFW""çFõö÷væVB‚’’À¢7F6´ÖfW&–f–6F–öåG—S£¤–çFVvW"À¢ÒÀ¢’À¢€¢&–æ7&VÖVçEv†–ÆR"À¢fV2°¢7F6´ÖfW&–f–6F–öåG—S£¤ö&¦V7B‚$6÷VçFW$6öçG&öÂ"çFõö÷væVB‚’’À¢7F6´ÖfW&–f–6F–öåG—S£¤–çFVvW"À¢7F6´ÖfW&–f–6F–öåG—S£¤ö&¦V7B‚$6÷VçFW""çFõö÷væVB‚’’À¢7F6´ÖfW&–f–6F–öåG—S£¤–çFVvW"À¢ÒÀ¢’À¢Ò°¢ÆWB6öFRÒVÖ—GFVEöÖWF†öEö6öFR‚f6öçG&öÂÂÖWF†öB“°¢ÆWB&6µöVFvW2ÒFV6öFUö'&æ6…÷F&vWG2†6öFR¢æ–çFõö—FW"‚¢æf–ÇFW"‡Â†öfg6WBÂòÂF&vWB—ÂF&vWBÂöfg6WB¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'B‚&6µöVFvW2æ—5öV×G’‚’Â'¶ÖWF†öGÒ†2Æö÷&6²ÖVFvR"“°¢ÆWBg&ÖW2Ò7F6µöÖög&ÖW2‚f6öçG&öÂÂÖWF†öB“°¢f÷"…òÂòÂF&vWB’–â&6µöVFvW2°¢ÆWBg&ÖRÒg&ÖW0¢æ—FW"‚¢æf–æB‡Æg&ÖWÂg&ÖRç2ÓÒF&vWB¢çVçw&ö÷%öVÇ6R‡ÇÂæ–2‚'¶ÖWF†öGÒ&6²ÖVFvRF&vWB†27F6²ÖÖg&ÖR"’“°¢76W'EöW†g&ÖRæÆö6Ç2ÂW‡V7FVEöÆö6Ç2Â'¶ÖWF†öGÒW†7BÆö÷Æö6Ç2"“°¢76W'B€¢g&ÖRç7F6²æ—5öV×G’‚’À¢'¶ÖWF†öGÒÆö÷†VFW"†2âV×G’7F6² ¢“°¢Ğ¢Ğ ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB†&æW72Ò÷WGW@¢çF‚‚¢æ¦ö–â‚%&VfW&Væ6U&ÖWFW%&W7VÇD6öçG&öÄfÆ÷t†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ72&VfW&Væ6U&ÖWFW%&W7VÇD6öçG&öÄfÆ÷t†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²6÷VçFW$6öçG&öÂ6öçG&öÂÒæWr6÷VçFW$6öçG&öÂ‚“²6÷VçFW"f—'7BÒæWr6÷VçFW"ƒ“²6÷VçFW"6V6öæBÒæWr6÷VçFW"ƒ“²–b†6öçG&öÂæ6†ö÷6R‡G'VRÂf—'7BÂ6V6öæB’Òf—'7BÇÂ6öçG&öÂæ6†ö÷6R†fÇ6RÂf—'7BÂ6V6öæB’Ò6V6öæB’F‡&÷ræWr76W'F–öäW'&÷"…Â&'&æ6‚–FVçF—G•Â"“²–b†6öçG&öÂæ6†ö÷6UF‡&÷Vv„6ÆÂ‡G'VRÂf—'7BÂ6V6öæB’Òf—'7BÇÂ6öçG&öÂæ6†ö÷6UF‡&÷Vv„6ÆÂ†fÇ6RÂf—'7BÂ6V6öæB’Ò6V6öæB’F‡&÷ræWr76W'F–öäW'&÷"…Â&6ÆÂ'&æ6‚–FVçF—G•Â"“²6÷VçFW"6÷VçBÒæWr6÷VçFW"ƒ“²–b†6öçG&öÂæ–æ7&VÖVçEVçF–Â†6÷VçBÂ’ÒÇÂ6öçG&öÂæ–æ7&VÖVçEVçF–Â†6÷VçBÂ’ÒÇÂ6öçG&öÂæ–æ7&VÖVçEVçF–Â†6÷VçBÂ’Ò’F‡&÷ræWr76W'F–öäW'&÷"…Â'¦W&òööæRö×VÇF’—FW&F–öåÂ"“²6÷VçFW"F—6&ÆVBÒæWr6÷VçFW"ƒ“²–b†6öçG&öÂæ–æ7&VÖVçEv†–ÆR†fÇ6RÂF—6&ÆVBÂ’Ò’F‡&÷ræWr76W'F–öäW'&÷"…Â'6†÷'BÖ6—&7V—B¦W&ò—FW&F–öåÂ"“²–b†6öçG&öÂæ–æ7&VÖVçEv†–ÆR‡G'VRÂF—6&ÆVBÂB’ÒB’F‡&÷ræWr76W'F–öäW'&÷"…Â'6†÷'BÖ6—&7V—B×VÇF’—FW&F–öåÂ"“²7—7FVÒæ÷WBç&–çFÆâ…Â'&VfW&Væ6R&ÖWFW"÷&W7VÇB4drfW&–f–VEÂ"“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FR&VfW&Væ6R&ÖWFW"÷&W7VÇBfW&–f–W"†&æW72"“°¢ÆWB6Æ77F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’çVçw&‚“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â6Æ77F‚Â"ÖB"Â6Æ77F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2f÷"&VfW&Væ6R6öçG&öÂÖfÆ÷r†&æW72"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVB&VfW&Væ6R6öçG&öÂÖfÆ÷r†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢6Æ77F‚À¢%&VfW&Væ6U&ÖWFW%&W7VÇD6öçG&öÄfÆ÷t†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¥dÒfW&–f–W"f÷"&VfW&Væ6R6öçG&öÂÖfÆ÷r6Æ76W2"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVB&VfW&Væ6R6öçG&öÂÖfÆ÷r6Æ76W5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢'&VfW&Væ6R&ÖWFW"÷&W7VÇB4drfW&–f–VB ¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâÆææVE÷&VfW&Væ6Uö÷W&F–öç5÷fW&–g•ö7&÷75ö–e÷v†–ÆUöæE÷6†÷'Eö6—&7V—Eö6fw2‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'ÆææVB×&VfW&Væ6RÖ6öçG&öÂÖfÆ÷r"“°¢ÆWB6÷W&6RÒ&6Æ726÷VçFW"‡7F'C¢–çB“¥Æâ&—fFRf"7W'&VçC¢–çBÒ7F'EÆâ&—fFRf"Væ&ÆVC¢&ööÆVâÒG'VUÆâFVb–æ7&VÖVçEVçF–Â†Æ–Ö—C¢–çB“¢–çBÕÆâv†–ÆR7W'&VçBÂÆ–Ö—BFò7W'&VçBÒ7W'&VçB²Æâ7W'&VçEÆâFVbVæ&ÆVEVçF–Â†Æ–Ö—C¢–çB“¢–çBÕÆâv†–ÆRVæ&ÆVBbb7W'&VçBÂÆ–Ö—BFò7W'&VçBÒ7W'&VçB²Æâ7W'&VçEÆâFVb6WDVæ&ÆVB†fÆs¢&ööÆVâ“¢–çBÕÆâ–bfÆrF†VâVæ&ÆVBÒG'VRVÇ6RVæ&ÆVBÒfÇ6UÆâ–bVæ&ÆVBF†Vâ7W'&VçBVÇ6RÆâFVbvWB‚“¢–çBÒ7W'&VçEÆâFVb–æ7&VÖVçB‚“¢Væ—BÒ7W'&VçBÒ7W'&VçB²Ææ6Æ726÷VçFW$6Æ–VçC¥ÆâFVb'Vâ†Æ–Ö—C¢–çB“¢–çBÕÆâfÂ6÷VçFW"ÒæWr6÷VçFW"ƒ•Æâv†–ÆR6÷VçFW"ævWB‚’ÂÆ–Ö—BFò6÷VçFW"æ–æ7&VÖVçB‚•Æâ6÷VçFW"ævWB‚•ÆâFVb6†ö÷6R†fÆs¢&ööÆVâ“¢–çBÕÆâ–bfÆrF†VâæWr6÷VçFW"ƒ’ævWB‚’VÇ6RæWr6÷VçFW"ƒ"’ævWB‚•ÆâFVb6†ö÷6TÆö6Â†fÆs¢&ööÆVâ“¢–çBÕÆâfÂ6÷VçFW"Ò–bfÆrF†VâæWr6÷VçFW"ƒR’VÇ6RæWr6÷VçFW"ƒb•Æâ6÷VçFW"ævWB‚•ÆâFVb–d–æ7&VÖVçB†fÆs¢&ööÆVâ“¢–çBÕÆâfÂ6÷VçFW"ÒæWr6÷VçFW"ƒ•Æâ–bfÆrF†Vâ6÷VçFW"æ–æ7&VÖVçB‚’VÇ6R6÷VçFW"æ–æ7&VÖVçB‚•Æâ6÷VçFW"ævWB‚•ÆâFVb'VäæB‡'Vææ–æs¢&ööÆVâÂÆ–Ö—C¢–çB“¢–çBÕÆâfÂ6÷VçFW"ÒæWr6÷VçFW"ƒ•Æâv†–ÆR'Vææ–ærbb6÷VçFW"ævWB‚’ÂÆ–Ö—BFò6÷VçFW"æ–æ7&VÖVçB‚•Æâ6÷VçFW"ævWB‚•Æâ#°¢ÆWB'F–f7G2Ò6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R‡6÷W&6RÂ÷WGWBçF‚‚’¢æW‡V7B‚&æ'&÷rÆææVB&VfW&Væ6R÷W&F–öç26†÷VÆB6ö×÷6Rv—F‚fW&–f–VB4dw2"“°¢76W'EöW€¢'F–f7G0¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢²$6÷VçFW""Â$6÷VçFW$6Æ–VçB%Ğ¢“° ¢ÆWB6÷VçFW%ö'F–f7BÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$6÷VçFW""¢çVçw&‚“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†6÷VçFW%ö'F–f7Bæ6Æ72æ'—FW2‚’“°¢ÆWB6÷VçFW"Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFR6÷VçFW""“°¢6÷VçFW ¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚$6÷VçFW"f–VÆBæBÖWF†öB&VfW&Væ6W2&RfÆ–B"“°¢f÷"ÖWF†öB–â²&–æ7&VÖVçEVçF–Â"Â&Væ&ÆVEVçF–Â"Â'6WDVæ&ÆVB%Ò°¢76W'B€¢†5÷7F6µöÖ÷F&ÆR‚f6÷VçFW"ÂÖWF†öB’À¢'¶ÖWF†öGÒ×W7B6''’vVæW&FVB4drg&ÖW2 ¢“°¢Ğ¢ÆWB–æ7&VÖVçE÷VçF–ÂÒVÖ—GFVEöÖWF†öEö6öFR‚f6÷VçFW"Â&–æ7&VÖVçEVçF–Â"“°¢76W'B€¢–æ7&VÖVçE÷VçF–Âæ6öçF–ç2‚c†#B’À¢&Æö÷&VG2F†R7W'&VçBf–VÆB ¢“°¢76W'B€¢–æ7&VÖVçE÷VçF–Âæ6öçF–ç2‚c†#R’À¢&Æö÷w&—FW2F†R7W'&VçBf–VÆB ¢“°¢ÆWB6WEöVæ&ÆVBÒVÖ—GFVEöÖWF†öEö6öFR‚f6÷VçFW"Â'6WDVæ&ÆVB"“°¢76W'B€¢6WEöVæ&ÆVBæ6öçF–ç2‚c†#R’À¢&–b'&æ6†W2w&—FRF†R&ööÆVâf–VÆB ¢“°¢76W'B‡6WEöVæ&ÆVBæ6öçF–ç2‚c†#B’Â&–b&VG2F†R&ööÆVâf–VÆB"“° ¢ÆWB6Æ–VçEö'F–f7BÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$6÷VçFW$6Æ–VçB"¢çVçw&‚“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†6Æ–VçEö'F–f7Bæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ–VçBÒ6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFR6÷VçFW$6Æ–VçB"“°¢6Æ–Vç@¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚$6÷VçFW$6Æ–VçB&VfW&Væ6W2&RfÆ–B"“°¢f÷"ÖWF†öB–â²''Vâ"Â&6†ö÷6R"Â&6†ö÷6TÆö6Â"Â&–d–æ7&VÖVçB"Â''VäæB%Ò°¢76W'B€¢†5÷7F6µöÖ÷F&ÆR‚f6Æ–VçBÂÖWF†öB’À¢'¶ÖWF†öGÒ×W7B6''’vVæW&FVB4drg&ÖW2 ¢“°¢Ğ¢76W'B€¢7F6µöÖög&ÖU÷72‚f6Æ–VçBÂ''Vâ"¢çv–æF÷w2ƒ"¢æÆÂ‡Æg&ÖW7Âg&ÖW5³ÒÂg&ÖW5³Ò’À¢&Æö÷g&ÖR72×W7B&R7G&–7FÇ’÷&FW&VB ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f6Æ–VçBÀ¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ–VçBÂ''Vâ"’À¢e°¢ƒ†#rÂ$6÷VçFW""Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#bÂ$6÷VçFW""Â&vWB"Â"‚”’"’À¢ƒ†#bÂ$6÷VçFW""Â&–æ7&VÖVçB"Â"‚•b"’À¢ƒ†#bÂ$6÷VçFW""Â&vWB"Â"‚”’"’À¢ÒÀ¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢fW&–g•÷&VfW&Væ6Uö6öçG&öÅöfÆ÷u÷v—F…ö¦F²†÷WGWBçF‚‚’“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ&VÅö6÷VçFW%ö6öçG&öÅöfÆ÷uöf—‡GW&Uö†5öW†7Eög&ÖW5öæE÷&W&öGV6–&ÆUö6Æ76W2‚’°¢ÆWBf—'7Eö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&6÷VçFW"Ö6öçG&öÂÖfÆ÷rÖf—'7B"“°¢ÆWB6V6öæEö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&6÷VçFW"Ö6öçG&öÂÖfÆ÷r×6V6öæB"“°¢ÆWBf—'7BÒ6ö×–ÆUöf—‡GW&Uö6Æ76W2‚$6÷VçFW$6öçG&öÄfÆ÷rç66Æ"Âf—'7Eö÷WGWBçF‚‚’¢æW‡V7B‚'&VÂ66Æ6÷W&6R'Vç2F‡&÷Vv‚ÆW†W"Â'6W"ÂæÖW"ÂG—W"æB¥dÒ&6¶VæB"“°¢ÆWB6V6öæBÒ6ö×–ÆUöf—‡GW&Uö6Æ76W2‚$6÷VçFW$6öçG&öÄfÆ÷rç66Æ"Â6V6öæEö÷WGWBçF‚‚’¢æW‡V7B‚&g&W6‚g&öçFVæB6W76–öâ6ö×–ÆW2F†R6ÖR6÷W&6R–æFWVæFVçFÇ’"“°¢ÆWBæÖW2Òf—'7@¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW†æÖW2Â²$6÷VçFW""Â$6÷VçFW$6Æ–VçB%Ò“°¢76W'EöW€¢6V6öæ@¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢æÖW2À¢&6Æ76W2&WF–â6÷W&6R÷&FW"–âF†R–æFWVæFVçB6W76–öâ ¢“°¢76W'EöW†f—'7Eö÷WGWBæ6Æ75öf–ÆW2‚’æÆVâ‚’Â"“°¢76W'EöW‡6V6öæEö÷WGWBæ6Æ75öf–ÆW2‚’æÆVâ‚’Â"“°¢f÷"†ÆVgBÂ&–v‡B’–âf—'7Bæ—FW"‚’ç¦—‚g6V6öæB’°¢76W'EöW€¢ÆVgBæ6Æ72æ'—FW2‚’À¢&–v‡Bæ6Æ72æ'—FW2‚’À¢'·Ò6Æ72'—FW2&RFWFW&Ö–æ—7F–27&÷72–æFWVæFVçBg&öçFVæB6W76–öç2"À¢ÆVgBæ6Æ72æ–çFW&æÅöæÖR‚¢“°¢76W'EöW€¢g3£§&VB‚fÆVgBçF‚’æW‡V7B‚'&VBV&Æ—6†VB6Æ72f–ÆR"’À¢ÆVgBæ6Æ72æ'—FW2‚’À¢'·Ò—2V&Æ—6†VB'—FRÖf÷"Ö'—FRg&öÒ—G2fÆ–FFVB–âÖÖVÖ÷'’6Æ72"À¢ÆVgBæ6Æ72æ–çFW&æÅöæÖR‚¢“°¢Ğ ¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†f—'7E³Òæ6Æ72æ'—FW2‚’“°¢ÆWB6÷VçFW"Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&gVÆÇ’FV6öFR6÷VçFW"æ6Æ72"“°¢6÷VçFW ¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚$6÷VçFW"6öç7FçBööÂfÆ–FFW2"“°¢76W'EöW†6÷VçFW"çfW'6–öâæÖ¦÷"ÂcR“°¢76W'EöW€¢6÷VçFW"æ6öç7FçE÷ööÂæ6Æ75öæÖR†6÷VçFW"çF†—5ö6Æ72’À¢ö²‚$6÷VçFW""¢“°¢76W'EöW€¢6÷VçFW"æ6öç7FçE÷ööÂæ6Æ75öæÖR€¢6÷VçFW ¢ç7WW%ö6Æ70¢æW‡V7B‚&÷&F–æ'’6Æ72†2ö&¦V7B7WW&6Æ72"¢’À¢ö²‚&¦föÆærôö&¦V7B"¢“°¢76W'B†6÷VçFW"æ66W75öfÆw2æ—5÷V&Æ–2‚’“°¢76W'B‚6÷VçFW"æ66W75öfÆw2æ—5ö–çFW&f6R‚’“°¢76W'EöW†6÷VçFW"æf–VÆG2æÆVâ‚’Â“°¢ÆWBf–VÆBÒf6÷VçFW"æf–VÆG5³Ó°¢76W'EöW†6÷VçFW"æ6öç7FçE÷ööÂçWFc‚†f–VÆBææÖUö–æFW‚’Âö²‚&7W'&VçB"’“°¢76W'EöW†6÷VçFW"æ6öç7FçE÷ööÂçWFc‚†f–VÆBæFW67&—F÷%ö–æFW‚’Âö²‚$’"’“°¢76W'B†f–VÆBæ66W75öfÆw2æ—5÷&—fFR‚’“°¢76W'B‚f–VÆBæ66W75öfÆw2æ—5öf–æÂ‚’“°¢76W'B€¢6÷VçFW ¢æÖWF†öG0¢æ—FW"‚¢æÆÂ‡ÆÖWF†öGÂÖWF†öBæ66W75öfÆw2æ—5÷V&Æ–2‚’¢“°¢76W'EöW€¢ÖWF†öG5÷v—F…öFW67&—F÷'2‚f6÷VçFW"’À¢°¢‚#Æ–æ—Câ"çFõö÷væVB‚’Â"„’•b"çFõö÷væVB‚’’À¢‚&vWB"çFõö÷væVB‚’Â"‚”’"çFõö÷væVB‚’’À¢‚&–æ7&VÖVçB"çFõö÷væVB‚’Â"‚”’"çFõö÷væVB‚’’À¢‚&–æ7&VÖVçEVçF–Â"çFõö÷væVB‚’Â"„’”’"çFõö÷væVB‚’’À¢Ğ¢“° ¢ÆWB–æ7&VÖVçE÷VçF–ÂÒVÖ—GFVEöÖWF†öEö6öFR‚f6÷VçFW"Â&–æ7&VÖVçEVçF–Â"“°¢ÆWB6÷VçFW%÷F&vWG2ÒFV6öFUö'&æ6…÷F&vWG2†–æ7&VÖVçE÷VçF–Â“°¢76W'B€¢6÷VçFW%÷F&vWG0¢æ—FW"‚¢æç’‡Â†öfg6WBÂòÂF&vWB—ÂF&vWBÂöfg6WB¢“°¢76W'B€¢6÷VçFW%÷F&vWG0¢æ—FW"‚¢æç’‡Â†öfg6WBÂ÷6öFRÂF&vWB—Â¦÷6öFRÓÒ†rbbF&vWBÂöfg6WB¢“°¢76W'B€¢6÷VçFW%÷F&vWG0¢æ—FW"‚¢æç’‡Â…òÂ÷6öFRÂò—Âƒƒ–bâãÓ†B’æ6öçF–ç2†÷6öFR’¢“°¢ÆWB6÷VçFW%ö÷2ÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†–æ7&VÖVçE÷VçF–Â“°¢76W'B†6÷VçFW%ö÷2æ—FW"‚’æç’‡Â…òÂ÷6öFR—Â¦÷6öFRÓÒƒ&’“°¢76W'B†6÷VçFW%ö÷2æ—FW"‚’æç’‡Â…òÂ÷6öFR—Â¦÷6öFRÓÒƒ"’“°¢76W'B€¢6÷VçFW%ö÷0¢æ—FW"‚¢æç’‡Â…òÂ÷6öFR—Âƒƒ3bâãÓƒ6’æ6öçF–ç2†÷6öFR’ÇÂƒƒ6"âãÓƒ6R’æ6öçF–ç2†÷6öFR’¢“°¢76W'Eöf–VÆE÷&VfW&Væ6R‚f6÷VçFW"Â–æ7&VÖVçE÷VçF–ÂÂ†#BÂ$6÷VçFW""Â&7W'&VçB"Â$’"“°¢76W'Eöf–VÆE÷&VfW&Væ6R‚f6÷VçFW"Â–æ7&VÖVçE÷VçF–ÂÂ†#RÂ$6÷VçFW""Â&7W'&VçB"Â$’"“°¢ÆWB6÷VçFW%ög&ÖW2Ò7F6µöÖög&ÖW2‚f6÷VçFW"Â&–æ7&VÖVçEVçF–Â"“°¢ÆWB6÷VçFW%ög&ÖU÷72Ò6÷VçFW%ög&ÖW0¢æ—FW"‚¢æÖ‡Æg&ÖWÂg&ÖRç2¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'B€¢6÷VçFW%÷F&vWG0¢æ—FW"‚¢æÆÂ‡Â…òÂòÂF&vWB—Â6÷VçFW%ög&ÖU÷72æ6öçF–ç2‡F&vWB’¢“°¢ÆWB…òÂòÂ6÷VçFW%öÆö÷ö†VFW"’Ò6÷VçFW%÷F&vWG0¢æ—FW"‚¢æ6÷–VB‚¢æf–æB‡Â†öfg6WBÂòÂF&vWB—ÂF&vWBÂöfg6WB¢æW‡V7B‚$6÷VçFW"æ–æ7&VÖVçEVçF–Â†2Æö÷&6²ÖVFvR"“°¢ÆWB6÷VçFW%öÆö÷ög&ÖRÒ6÷VçFW%ög&ÖW0¢æ—FW"‚¢æf–æB‡Æg&ÖWÂg&ÖRç2ÓÒ6÷VçFW%öÆö÷ö†VFW"¢æW‡V7B‚&Æö÷†VFW"†2gVÆÅög&ÖRVçG'’"“°¢76W'EöW€¢6÷VçFW%öÆö÷ög&ÖRæÆö6Ç2À¢°¢7F6´ÖfW&–f–6F–öåG—S£¤ö&¦V7B‚$6÷VçFW""çFõö÷væVB‚’’À¢7F6´ÖfW&–f–6F–öåG—S£¤–çFVvW"À¢Ğ¢“°¢76W'B†6÷VçFW%öÆö÷ög&ÖRç7F6²æ—5öV×G’‚’“° ¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†f—'7E³Òæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ–VçBÒ6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&gVÆÇ’FV6öFR6÷VçFW$6Æ–VçBæ6Æ72"“°¢6Æ–Vç@¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚$6÷VçFW$6Æ–VçB6öç7FçBööÂfÆ–FFW2"“°¢76W'EöW†6Æ–VçBçfW'6–öâæÖ¦÷"ÂcR“°¢76W'EöW€¢6Æ–VçBæ6öç7FçE÷ööÂæ6Æ75öæÖR†6Æ–VçBçF†—5ö6Æ72’À¢ö²‚$6÷VçFW$6Æ–VçB"¢“°¢76W'EöW€¢6Æ–VçBæ6öç7FçE÷ööÂæ6Æ75öæÖR€¢6Æ–Vç@¢ç7WW%ö6Æ70¢æW‡V7B‚&÷&F–æ'’6Æ72†2ö&¦V7B7WW&6Æ72"¢’À¢ö²‚&¦föÆærôö&¦V7B"¢“°¢76W'B†6Æ–VçBæ66W75öfÆw2æ—5÷V&Æ–2‚’“°¢76W'B‚6Æ–VçBæ66W75öfÆw2æ—5ö–çFW&f6R‚’“°¢76W'B†6Æ–VçBæf–VÆG2æ—5öV×G’‚’“°¢76W'B€¢6Æ–Vç@¢æÖWF†öG0¢æ—FW"‚¢æÆÂ‡ÆÖWF†öGÂÖWF†öBæ66W75öfÆw2æ—5÷V&Æ–2‚’¢“°¢76W'EöW€¢ÖWF†öG5÷v—F…öFW67&—F÷'2‚f6Æ–VçB’À¢°¢‚#Æ–æ—Câ"çFõö÷væVB‚’Â"‚•b"çFõö÷væVB‚’’À¢‚''Vâ"çFõö÷væVB‚’Â"„’”’"çFõö÷væVB‚’’À¢‚&6†ö÷6R"çFõö÷væVB‚’Â"…¢”’"çFõö÷væVB‚’’À¢Ğ¢“° ¢ÆWB'VâÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ–VçBÂ''Vâ"“°¢ÆWB'Våö–ç7G'V7F–öç2ÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2‡'Vâ“°¢ÆWB'Vå÷F&vWG2ÒFV6öFUö'&æ6…÷F&vWG2‡'Vâ“°¢76W'B€¢'Vå÷F&vWG0¢æ—FW"‚¢æç’‡Â†öfg6WBÂòÂF&vWB—ÂF&vWBÂöfg6WB¢“°¢76W'B€¢'Vå÷F&vWG0¢æ—FW"‚¢æç’‡Â…òÂ÷6öFRÂò—Âƒƒ–bâãÓ†B’æ6öçF–ç2†÷6öFR’¢“°¢ÆWB'Våög&ÖW2Ò7F6µöÖög&ÖW2‚f6Æ–VçBÂ''Vâ"“°¢ÆWB'Våög&ÖU÷72Ò'Våög&ÖW2æ—FW"‚’æÖ‡Æg&ÖWÂg&ÖRç2’æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'B€¢'Vå÷F&vWG0¢æ—FW"‚¢æÆÂ‡Â…òÂòÂF&vWB—Â'Våög&ÖU÷72æ6öçF–ç2‡F&vWB’¢“°¢ÆWB…òÂòÂ'VåöÆö÷ö†VFW"’Ò'Vå÷F&vWG0¢æ—FW"‚¢æ6÷–VB‚¢æf–æB‡Â†öfg6WBÂòÂF&vWB—ÂF&vWBÂöfg6WB¢æW‡V7B‚$6÷VçFW$6Æ–VçBç'Vâ†2Æö÷&6²ÖVFvR"“°¢ÆWB'VåöÆö÷ög&ÖRÒ'Våög&ÖW0¢æ—FW"‚¢æf–æB‡Æg&ÖWÂg&ÖRç2ÓÒ'VåöÆö÷ö†VFW"¢æW‡V7B‚''VâÆö÷†VFW"†2gVÆÅög&ÖRVçG'’"“°¢76W'EöW€¢'VåöÆö÷ög&ÖRæÆö6Ç2À¢°¢7F6´ÖfW&–f–6F–öåG—S£¤ö&¦V7B‚$6÷VçFW$6Æ–VçB"çFõö÷væVB‚’’À¢7F6´ÖfW&–f–6F–öåG—S£¤–çFVvW"À¢7F6´ÖfW&–f–6F–öåG—S£¤ö&¦V7B‚$6÷VçFW""çFõö÷væVB‚’’À¢ÒÀ¢'F†RÆö÷g&ÖR&WF–ç2W†7B&V6V—fW"ÂÆ–Ö—BæB–æ—F–Æ—¦VB6÷VçFW"Æö6Ç2 ¢“°¢76W'B‡'VåöÆö÷ög&ÖRç7F6²æ—5öV×G’‚’“°¢76W'B‡'Våö–ç7G'V7F–öç2æ—FW"‚’æç’‡Â…òÂ÷6öFR—Â¦÷6öFRÓÒƒ&2’“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f6Æ–VçBÀ¢'VâÀ¢e°¢ƒ†#rÂ$6÷VçFW""Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#bÂ$6÷VçFW""Â&vWB"Â"‚”’"’À¢ƒ†#bÂ$6÷VçFW""Â&–æ7&VÖVçB"Â"‚”’"’À¢ƒ†#bÂ$6÷VçFW""Â&vWB"Â"‚”’"’À¢ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f6Æ–VçBÀ¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ–VçBÂ#Æ–æ—Câ"’À¢e²ƒ†#rÂ&¦föÆærôö&¦V7B"Â#Æ–æ—Câ"Â"‚•b"•ÒÀ¢“° ¢ÆWB6†ö÷6RÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ–VçBÂ&6†ö÷6R"“°¢ÆWB6†ö÷6Uö–ç7G'V7F–öç2ÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†6†ö÷6R“°¢76W'B€¢6†ö÷6Uö–ç7G'V7F–öç0¢æ—FW"‚¢æç’‡Â…òÂ÷6öFR—Â¦÷6öFRÓÒƒ“’¢“°¢ÆWBÆÆö6F–öç2Ò6†ö÷6Uö–ç7G'V7F–öç0¢æ—FW"‚¢æf–ÇFW%öÖ‡Â†öfg6WBÂ÷6öFR—Â‚¦÷6öFRÓÒ†&"’çF†Vå÷6öÖR‚¦öfg6WB’¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW†ÆÆö6F–öç2æÆVâ‚’Â"“°¢ÆWB6öç7G'V7F÷%ööfg6WG2Ò6†ö÷6Uö–ç7G'V7F–öç0¢æ—FW"‚¢æf–ÇFW%öÖ‡Â†öfg6WBÂ÷6öFR—Â‚¦÷6öFRÓÒ†#r’çF†Vå÷6öÖR‚¦öfg6WB’¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW†6öç7G'V7F÷%ööfg6WG2æÆVâ‚’Â"“°¢f÷"†ÆÆö6F–öâÂ6öç7G'V7F÷"’–âÆÆö6F–öç2æ—FW"‚’ç¦—‚f6öç7G'V7F÷%ööfg6WG2’°¢ÆWBÆÆö6F–öå÷÷6—F–öâÒ6†ö÷6Uö–ç7G'V7F–öç0¢æ—FW"‚¢ç÷6—F–öâ‡Â†öfg6WBÂò—Âöfg6WBÓÒÆÆö6F–öâ¢æW‡V7B‚&f–æBæWr–ç7G'V7F–öâ&÷VæF'’"“°¢ÆWB6öç7G'V7F÷%÷÷6—F–öâÒ6†ö÷6Uö–ç7G'V7F–öç0¢æ—FW"‚¢ç÷6—F–öâ‡Â†öfg6WBÂò—Âöfg6WBÓÒ6öç7G'V7F÷"¢æW‡V7B‚&f–æB–çfö¶W7V6–Â–ç7G'V7F–öâ&÷VæF'’"“°¢76W'EöW€¢6†ö÷6Uö–ç7G'V7F–öç5¶ÆÆö6F–öå÷÷6—F–öââãÖ6öç7G'V7F÷%÷÷6—F–öåĞ¢æ—FW"‚¢æÖ‡Â…òÂ÷6öFR—Â¦÷6öFR¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢³†&"ÂƒS’ÂƒÂ†#uÒÀ¢&V6‚'&æ6‚6ö×ÆWFW2æWröGWô–çBÖ&wVÖVçBö–çfö¶W7V6–Â–âöæR&Æö6² ¢“°¢ÆWB6Æ75ö–æFW‚ÒSc£¦g&öÕö&Uö'—FW2…¶6†ö÷6U¶ÆÆö6F–öâ²ÒÂ6†ö÷6U¶ÆÆö6F–öâ²%ÕÒ“°¢ÆWB6öç7FçEööÄVçG'“£¤6Æ72²æÖUö–æFW‚ÒÒ6Æ–Vç@¢æ6öç7FçE÷ööÀ¢ævWB„6öç7FçEööÄ–æFW‚†6Æ75ö–æFW‚’¢æW‡V7B‚&æWr×W7B&VfW&Væ6R4ôå5DåEô6Æ72"¢VÇ6R°¢æ–2‚&æWr×W7B&VfW&Væ6R6Æ726öç7FçB"“°¢Ó°¢76W'EöW†6Æ–VçBæ6öç7FçE÷ööÂçWFc‚‚¦æÖUö–æFW‚’Âö²‚$6÷VçFW""’“°¢76W'B†6öç7G'V7F÷"âÆÆö6F–öâ“°¢76W'B€¢6†ö÷6Uö–ç7G'V7F–öç0¢æ—FW"‚¢æf–ÇFW"‡Â†öfg6WBÂò—Â¦öfg6WBâ¦ÆÆö6F–öâbb¦öfg6WBÂ¦6öç7G'V7F÷"¢æÆÂ‡Â…òÂ÷6öFR—ÂÖF6†W2‚¦÷6öFRÂƒ“’âãÓ†r’¢“°¢ÆWBÖWF†öEö–æFW‚ÒSc£¦g&öÕö&Uö'—FW2…¶6†ö÷6U¶6öç7G'V7F÷"²ÒÂ6†ö÷6U¶6öç7G'V7F÷"²%ÕÒ“°¢ÆWB6öç7FçEööÄVçG'“£¤ÖWF†öG&Vb°¢6Æ75ö–æFW‚À¢æÖUöæE÷G—Uö–æFW‚À¢ÒÒ6Æ–Vç@¢æ6öç7FçE÷ööÀ¢ævWB„6öç7FçEööÄ–æFW‚†ÖWF†öEö–æFW‚’¢æW‡V7B‚&–çfö¶W7V6–Â×W7B&VfW&Væ6R4ôå5DåEôÖWF†öG&Vb"¢VÇ6R°¢æ–2‚&6öç7G'V7F÷"6ÆÂ×W7B&VfW&Væ6RÖWF†öB"“°¢Ó°¢76W'EöW†6Æ–VçBæ6öç7FçE÷ööÂæ6Æ75öæÖR‚¦6Æ75ö–æFW‚’Âö²‚$6÷VçFW""’“°¢76W'EöW€¢6Æ–VçBæ6öç7FçE÷ööÂææÖUöæE÷G—R‚¦æÖUöæE÷G—Uö–æFW‚’À¢ö²‚‚#Æ–æ—Câ"Â"„’•b"’¢“°¢Ğ¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f6Æ–VçBÀ¢6†ö÷6RÀ¢e°¢ƒ†#rÂ$6÷VçFW""Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#bÂ$6÷VçFW""Â&vWB"Â"‚”’"’À¢ƒ†#rÂ$6÷VçFW""Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#bÂ$6÷VçFW""Â&vWB"Â"‚”’"’À¢ÒÀ¢“°¢ÆWB6†ö÷6U÷F&vWG2ÒFV6öFUö'&æ6…÷F&vWG2†6†ö÷6R“°¢ÆWB6†ö÷6Uög&ÖW2Ò7F6µöÖög&ÖW2‚f6Æ–VçBÂ&6†ö÷6R"“°¢ÆWB6†ö÷6Uög&ÖU÷72Ò6†ö÷6Uög&ÖW0¢æ—FW"‚¢æÖ‡Æg&ÖWÂg&ÖRç2¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'B€¢6†ö÷6U÷F&vWG0¢æ—FW"‚¢æÆÂ‡Â…òÂòÂF&vWB—Â6†ö÷6Uög&ÖU÷72æ6öçF–ç2‡F&vWB’¢“°¢76W'B†6†ö÷6Uög&ÖW2æ—FW"‚’æÆÂ‡Æg&ÖWÂ°¢g&ÖRæÆö6Ç2æf—'7B‚’ÓÒ6öÖR‚e7F6´ÖfW&–f–6F–öåG—S£¤ö&¦V7B‚$6÷VçFW$6Æ–VçB"æ–çFò‚’’¢Ò’“°¢76W'B†6†ö÷6Uög&ÖW2æ—FW"‚’æÆÂ‡Æg&ÖWÂ°¢g&ÖRç7F6²æ—5öV×G’‚’ÇÂg&ÖRç7F6²ÓÒµ7F6´ÖfW&–f–6F–öåG—S£¤–çFVvW%Ğ¢Ò’“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢fW&–g•ö6÷VçFW%ö6öçG&öÅöfÆ÷u÷v—F…ö¦F²†f—'7Eö÷WGWBçF‚‚’“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ–Ö×WF&ÆU÷&VfW&Væ6UöÆö6Ç5÷W6Uö6æöæ–6Å÷6Æ÷G5öæE÷fW&–g•ööå÷F†Uö§fÒ‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&VfW&Væ6RÖÆö6Â"“°¢ÆWB'F–f7G2Ò6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R€¢&6Æ726÷VçFW"‡6VVC¢–çB•Ææ6Æ726Æ–VçC¥ÆâFVb'Vâ‡6VVC¢–çB“¢–çBÕÆâfÂ6÷VçFW"ÒæWr6÷VçFW"‡6VVB•Æâ6÷VçFW%ÆâC%Æâ"À¢÷WGWBçF‚‚’À¢¢çVçw&‚“°¢ÆWB6Æ–VçBÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$6Æ–VçB"¢æW‡V7B‚$6Æ–VçB6÷W&6R6Æ72—2VÖ—GFVB"“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†6Æ–VçBæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFR6Æ–VçB6Æ72"“°¢6Æ70¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚'fÆ–B6Æ–VçB&VfW&Væ6W2"“°¢ÆWB6öFRÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â''Vâ"“°¢76W'Eö÷6öFU÷6WVVæ6R€¢6öFRÀ¢e³†&"ÂƒS’Âƒ"Â†#rÂƒFBÂƒ&2ÂƒSrÂƒÂ†5ÒÀ¢“°¢ÆWBÆÆö6F–öâÒSc£¦g&öÕö&Uö'—FW2…¶6öFU³ÒÂ6öFU³%ÕÒ“°¢ÆWB6öç7FçEööÄVçG'“£¤6Æ72²æÖUö–æFW‚ÒÒ6Æ70¢æ6öç7FçE÷ööÀ¢ævWB„6öç7FçEööÄ–æFW‚†ÆÆö6F–öâ’¢æW‡V7B‚$6÷VçFW"6Æ726öç7FçB"¢VÇ6R°¢æ–2‚&æWr×W7B&VfW&Væ6R6Æ72VçG'’"¢Ó°¢76W'EöW†6Æ72æ6öç7FçE÷ööÂçWFc‚‚¦æÖUö–æFW‚’Âö²‚$6÷VçFW""’“°¢ÆWB–çfö6F–öâÒSc£¦g&öÕö&Uö'—FW2…¶6öFU³eÒÂ6öFU³uÕÒ“°¢ÆWB6öç7FçEööÄVçG'“£¤ÖWF†öG&Vb°¢6Æ75ö–æFW‚À¢æÖUöæE÷G—Uö–æFW‚À¢ÒÒ6Æ70¢æ6öç7FçE÷ööÀ¢ævWB„6öç7FçEööÄ–æFW‚†–çfö6F–öâ’¢æW‡V7B‚&6öç7G'V7F÷"ÖWF†öB&VfW&Væ6R"¢VÇ6R°¢æ–2‚&–çfö¶W7V6–Â×W7B&VfW&Væ6RÖWF†öB"¢Ó°¢76W'EöW†6Æ72æ6öç7FçE÷ööÂæ6Æ75öæÖR‚¦6Æ75ö–æFW‚’Âö²‚$6÷VçFW""’“°¢76W'EöW€¢6Æ72æ6öç7FçE÷ööÂææÖUöæE÷G—R‚¦æÖUöæE÷G—Uö–æFW‚’À¢ö²‚‚#Æ–æ—Câ"Â"„’•b"’¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚%&VfW&Væ6TÆö6Ä†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ72&VfW&Væ6TÆö6Ä†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²–b†æWr6Æ–VçB‚’ç'Vâƒ’ÒC"’F‡&÷ræWr76W'F–öäW'&÷"‚“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FR¤D²fW&–f–6F–öâ†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWE÷F‚Â"ÖB"Â÷WGWE÷F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVB&VfW&Væ6RÖÆö6Â†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢%&VfW&Væ6TÆö6Ä†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W""“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVB&VfW&Væ6RÖÆö6Â6Æ76W5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ&VfW&Væ6UöÆö6Åö66WG5ö÷7G&–v‡EöÆ–æUö&Æö6µö–æ—F–Æ—¦W"‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&VfW&Væ6RÖÆö6ÂÖ&Æö6²Ö–æ—F–Æ—¦W""“°¢ÆWB'F–f7G2Ò6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R€¢&6Æ726÷VçFW"‡6VVC¢–çB•Ææ6Æ726Æ–VçC¥ÆâFVb'Vâ‡6VVC¢–çB“¢–çBÕÆâfÂ6÷VçFW"Ò²fÂ‚Ò6VVB²²æWr6÷VçFW"‡‚’ÕÆâ6÷VçFW%ÆâC%Æâ"À¢÷WGWBçF‚‚’À¢¢çVçw&‚“°¢ÆWB6Æ–VçBÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$6Æ–VçB"¢æW‡V7B‚$6Æ–VçB6÷W&6R6Æ72—2VÖ—GFVB"“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†6Æ–VçBæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFR6Æ–VçB6Æ72"“°¢6Æ70¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚'fÆ–B6Æ–VçB&VfW&Væ6W2"“°¢ÆWB6öFRÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â''Vâ"“°¢76W'Eö÷6öFU÷6WVVæ6R€¢6öFRÀ¢e°¢ƒ"ÂƒBÂƒcÂƒ6BÂ†&"ÂƒS’Âƒ2Â†#rÂƒFRÂƒ&BÂƒSrÂƒÂ†2À¢ÒÀ¢“°§Ğ ¢5·FW7EĞ¦fâÆö6Ç5ö¶VWöF—7F–æ7E÷ÆææVEö6Æ75ö–FVçF—F–W5ö–åööæUöÖWF†öB‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&F—7F–æ7B×&VfW&Væ6RÖÆö6Â×G—W2"“°¢ÆWB'F–f7G2Ò6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R€¢&6Æ726÷VçFW"‡6VVC¢–çB•Ææ6Æ72÷F†W"‡6VVC¢–çB•Ææ6Æ726Æ–VçC¥ÆâFVb'Vâ‡6VVC¢–çB“¢–çBÕÆâfÂ6÷VçFW"ÒæWr6÷VçFW"‡6VVB•ÆâfÂ÷F†W"ÒæWr÷F†W"‡6VVB•Æâ6÷VçFW%Æâ÷F†W%ÆâC%Æâ"À¢÷WGWBçF‚‚’À¢¢çVçw&‚“°¢ÆWB6Æ–VçBÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$6Æ–VçB"¢æW‡V7B‚$6Æ–VçB6÷W&6R6Æ72—2VÖ—GFVB"“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†6Æ–VçBæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFR6Æ–VçB6Æ72"“°¢6Æ70¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚'fÆ–B6Æ–VçB&VfW&Væ6W2"“°¢ÆWB6öFRÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â''Vâ"“°¢76W'Eö÷6öFU÷6WVVæ6R€¢6öFRÀ¢e°¢†&"ÂƒS’Âƒ"Â†#rÂƒFBÂ†&"ÂƒS’Âƒ"Â†#rÂƒFRÂƒ&2ÂƒSrÂƒ&BÂƒSrÀ¢ƒÂ†2À¢ÒÀ¢“°¢ÆWB6Æ75öæÖRÒÆöfg6WGÂ°¢ÆWB–æFW‚ÒSc£¦g&öÕö&Uö'—FW2…¶6öFU¶öfg6WB²ÒÂ6öFU¶öfg6WB²%ÕÒ“°¢ÆWB6öç7FçEööÄVçG'“£¤6Æ72²æÖUö–æFW‚ÒÒ6Æ70¢æ6öç7FçE÷ööÀ¢ævWB„6öç7FçEööÄ–æFW‚†–æFW‚’¢æW‡V7B‚&ÆÆö6FVBÆææVB6Æ72"¢VÇ6R°¢æ–2‚&æWr×W7B&VfW&Væ6R6Æ72VçG'’"¢Ó°¢6Æ72æ6öç7FçE÷ööÂçWFc‚‚¦æÖUö–æFW‚’çVçw&‚¢Ó°¢ÆWBÆÆö6F–öç2ÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ–çFõö—FW"‚¢æf–ÇFW%öÖ‡Â†öfg6WBÂ÷6öFR—Â†÷6öFRÓÒ†&"’çF†Vå÷6öÖR†öfg6WB’¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW†ÆÆö6F–öç2æÆVâ‚’Â"“°¢76W'EöW†6Æ75öæÖR†ÆÆö6F–öç5³Ò’Â$6÷VçFW""“°¢76W'EöW†6Æ75öæÖR†ÆÆö6F–öç5³Ò’Â$÷F†W""“°§Ğ ¢5·FW7EĞ¦fâ×WF&ÆU÷&VfW&Væ6UöÆö6Åö–æ—F–Æ—¦F–öå÷W6W5öö6FVv÷'•ööæU÷6Æ÷B‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&VfW&Væ6R×f""“°¢ÆWB'F–f7G2Ò6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R€¢&6Æ726÷VçFW"‡6VVC¢–çB•Ææ6Æ726Æ–VçC¥ÆâFVb'Vâ‡6VVC¢–çB“¢–çBÕÆâf"6÷VçFW"ÒæWr6÷VçFW"‡6VVB•ÆâC%Æâ"À¢÷WGWBçF‚‚’À¢¢æW‡V7B‚&ÆææVB×WF&ÆR&VfW&Væ6RÆö6Â6â&R–æ—F–Æ—¦VB"“°¢76W'B€¢'F–f7G2æ—5öV×G’‚’À¢'F†R7V66W76gVÂ6÷W&6R6ö×–ÆF–öâ&WGW&ç2—G2ÆææVB6Æ72&F6‚ ¢“°¢ÆWB6Æ–VçBÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$6Æ–VçB"¢æW‡V7B‚$6Æ–VçB6Æ72—2VÖ—GFVB"“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†6Æ–VçBæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFR6Æ–VçB6Æ72"“°¢ÆWB6öFRÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â''Vâ"“°¢76W'B€¢6öFRæ6öçF–ç2‚cƒFB’À¢'F†R×WF&ÆR6÷VçFW"Æö6Â—27F÷&VBöæ6R–â6FVv÷'’ÖöæR6Æ÷B#¢¶6öFS£÷Ò ¢“°§Ğ ¢5·FW7EĞ¦fâ×WF&ÆU÷&VfW&Væ6UöÆö6Åö76–væÖVçEö66WG5öö6†V6¶VE÷7V'G—UöæEöF—7F6†W2‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&VfW&Væ6R×f"Ö76–væÖVçB"“°¢ÆWB6÷W&6RÒ&6Æ72&6R‡6VVC¢–çB“¥Æâ&—fFRf"7W'&VçC¢–çBÒ6VVEÆâFVbfÇVR‚“¢–çBÒ7W'&VçEÆæ6Æ72ÆVgB‡6VVC¢–çB’W‡FVæG2&6R‡6VVB“¥Æâ÷fW'&–FRFVbfÇVR‚“¢–çBÒ7WW"çfÇVR‚’²Ææ6Æ72&–v‡B‡6VVC¢–çB’W‡FVæG2&6R‡6VVB“¥Æâ÷fW'&–FRFVbfÇVR‚“¢–çBÒ7WW"çfÇVR‚’²%Ææ6Æ72'VææW#¥ÆâFVb6†ævR‡6VVC¢–çB“¢&6RÕÆâf"7W'&VçC¢&6RÒæWrÆVgB‡6VVB•Æâ7W'&VçBÒæWr&–v‡B‡6VVB•Æâ7W'&VçEÆâFVb6ÖR‡6VVC¢–çB“¢&6RÕÆâf"7W'&VçC¢&6RÒæWrÆVgB‡6VVB•Æâ7W'&VçBÒæWr&6R‡6VVB•Æâ7W'&VçEÆâFVbVæ6†ævVB‡6VVC¢–çB“¢&6RÕÆâf"7W'&VçC¢&6RÒæWrÆVgB‡6VVB•Æâ7W'&VçEÆâ#°¢ÆWB'F–f7G2Ò6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R‡6÷W&6RÂ÷WGWBçF‚‚’¢æW‡V7B‚&×WF&ÆRÆææVB×&VfW&Væ6RÆö6Â66WG26†V6¶VB7V'G—R76–væÖVçB"“°¢ÆWB'VææW"Ò'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ%'VææW""¢æW‡V7B‚%'VææW"6÷W&6R6Æ72—2VÖ—GFVB"“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr‡'VææW"æ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFR'VææW"6Æ72"“°¢6Æ70¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚'fÆ–B'VææW"&VfW&Væ6W2"“°¢ÆWB6†ævRÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â&6†ævR"“°¢76W'EöW€¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2†6†ævR¢æ—FW"‚¢æÖ‡Â…òÂ÷6öFR—Â¦÷6öFR¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢°¢†&"ÂƒS’Âƒ"Â†#rÂƒFBÂ†&"ÂƒS’Âƒ"Â†#rÂƒFBÂƒ&2Â†# ¢ÒÀ¢'F†R&V76–væVB&VfW&Væ6R—2ÆöFVBg&öÒ—G26æöæ–6Â6Æ÷BæB&WGW&æVB ¢“°¢76W'EöW€¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2†6†ævR¢æ—FW"‚¢æÖ‡Â…òÂ÷6öFR—Â¦÷6öFR¢æf–ÇFW"‡Æ÷6öFWÂ¦÷6öFRÓÒƒFB¢æ6÷VçB‚’À¢"À¢'F†R–æ—F–ÂFV6Æ&F–öâæB&V76–væÖVçBV6‚7F÷&RöæR&VfW&Væ6R ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f6Æ72À¢6†ævRÀ¢e°¢ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ%&–v‡B"Â#Æ–æ—Câ"Â"„’•b"’À¢ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f6Æ72À¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â'Væ6†ævVB"’À¢e²ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f6Æ72À¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â'6ÖR"’À¢e°¢ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ$&6R"Â#Æ–æ—Câ"Â"„’•b"’À¢ÒÀ¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚$×WF&ÆU&VfW&Væ6T76–væÖVçD†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ72×WF&ÆU&VfW&Væ6T76–væÖVçD†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²'VææW"'VææW"ÒæWr'VææW"‚“²–çEµÕµÒ66W2Ò·³Â"ÂÒÂ³Â"ÂÒÂ²Ó2ÂÓÂÓ'×Ó²f÷"†–çEµÒFW7B¢66W2’²–b‡'VææW"æ6†ævR‡FW7E³Ò’çfÇVR‚’ÒFW7E³ÒÇÂ'VææW"ç6ÖR‡FW7E³Ò’çfÇVR‚’ÒFW7E³ÒÇÂ'VææW"çVæ6†ævVB‡FW7E³Ò’çfÇVR‚’ÒFW7E³%Ò’F‡&÷ræWr76W'F–öäW'&÷"†¦fçWF–Âä'&—2çFõ7G&–ær‡FW7B’“²ÒÒÕÆâ"À¢¢æW‡V7B‚'w&—FR×WF&ÆR×&VfW&Væ6R76–væÖVçB†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWE÷F‚Â"ÖB"Â÷WGWE÷F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVB×WF&ÆR×&VfW&Væ6R76–væÖVçB†&æW75ÅÆç7FF÷WC¥ÅÆç·ÕÅÆç7FFW'#¥ÅÆç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢$×WF&ÆU&VfW&Væ6T76–væÖVçD†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W""“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVB×WF&ÆR×&VfW&Væ6R76–væÖVçB6Æ76W5ÅÆç7FF÷WC¥ÅÆç·ÕÅÆç7FFW'#¥ÅÆç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ6öçFW‡GVÅ÷6–&Æ–æuö–e÷&W7VÇG5ö–æ—F–Æ—¦UöæEö76–våöFV6Æ&VE÷&VfW&Væ6UöÆö6Ç2‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&6öçFW‡GVÂ×6–&Æ–ærÖ–bÖÆö6Â×fÇVW2"“°¢ÆWB6÷W&6RÒ&6Æ72&6R‡6VVC¢–çB“¥Æâ&—fFRf"7F÷&VC¢–çBÒ6VVEÆâFVbfÇVR‚“¢–çBÒ7F÷&VEÆæ6Æ72ÆVgB‡6VVC¢–çB’W‡FVæG2&6R‡6VVB“¥Æâ÷fW'&–FRFVbfÇVR‚“¢–çBÒ7WW"çfÇVR‚’²Ææ6Æ72&–v‡B‡6VVC¢–çB’W‡FVæG2&6R‡6VVB“¥Æâ÷fW'&–FRFVbfÇVR‚“¢–çBÒ7WW"çfÇVR‚’²%Ææ6Æ72'VææW#¥ÆâFVb–æ—B†fÆs¢&ööÆVâÂ6VVC¢–çB“¢&6RÕÆâf"7W'&VçC¢&6RÒ–bfÆrF†VâæWrÆVgB‡6VVB’VÇ6RæWr&–v‡B‡6VVB•Æâ7W'&VçEÆâFVb–æ—E&WfW'6R†fÆs¢&ööÆVâÂ6VVC¢–çB“¢&6RÕÆâf"7W'&VçC¢&6RÒ–bfÆrF†VâæWr&–v‡B‡6VVB’VÇ6RæWrÆVgB‡6VVB•Æâ7W'&VçEÆâFVb&WÆ6R†fÆs¢&ööÆVâÂ6VVC¢–çB“¢&6RÕÆâf"7W'&VçC¢&6RÒæWrÆVgB‡6VVB•Æâ7W'&VçBÒ–bfÆrF†VâæWr&–v‡B‡6VVB’VÇ6RæWrÆVgB‡6VVB•Æâ7W'&VçEÆâFVb&WÆ6TæW7FVB†fÆs¢&ööÆVâÂ6VVC¢–çB“¢&6RÕÆâf"7W'&VçC¢&6RÒæWrÆVgB‡6VVB•Æâ7W'&VçBÒ²–bfÆrF†VâæWr&–v‡B‡6VVB’VÇ6RæWrÆVgB‡6VVB’ÕÆâ7W'&VçEÆâ#°¢ÆWB×WBö'6W'fVEöÖWF†öG2Ò°¢ÆWB'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö&F6…öö'6W'fVB€¢6÷W&6RÀ¢$6öçFW‡GVÅ6–&Æ–æt–bç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒCS‚’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢6öÖR‚f×WBÇVæ—BÂFVf–æ—F–öç7Â°¢ÆWB'VææW"ÒVF—Eö6Æ75öæÖVB‡Væ—BÂ%'VææW""“°¢ÆWB&6RÒVF—Eö6Æ75öæÖVB‡Væ—BÂ$&6R"“°¢f÷"ÖWF†öEöæÖR–â²&–æ—B"Â&–æ—E&WfW'6R"Â'&WÆ6R"Â'&WÆ6TæW7FVB%Ò°¢ö'6W'fVEöÖWF†öG2³Ò°¢ÆWBÖWF†öBÒVF—EöÖWF†öEöæÖVB‡Væ—BÂ'VææW"ÂÖWF†öEöæÖR“°¢ÆWBÆö6ÂÒÖWF†ö@¢æÆö6Ç0¢æ—FW"‚¢æf–æB‡Æ&–æF–æwÂ°¢Væ—Bç7F÷&P¢ææÖW0¢ç&W6öÇfR‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB†&–æF–ærç7–Ö&öÂ’ææÖRçFW‡B‚’¢ÓÒ&7W'&VçB ¢Ò¢æW‡V7B‚&V6‚6öçFW‡GVÂÖWF†öBFV6Æ&W27W'&VçB"“°¢ÆWBF÷GG•ö6÷&S£¥7–Ö&öÄ–æfó£¤6ö×ÆWFR†FV6Æ&VE÷G—R’Ğ¢Væ—Bç7F÷&Rç7–Ö&öÇ2æ–æfò†Æö6Âç7–Ö&öÂ¢VÇ6R°¢æ–2‚&7W'&VçB¶VW2—G26æöæ–6ÂFV6Æ&VB6÷W&6RG—R"¢Ó°¢76W'EöW†VF—E÷G—U÷7–Ö&öÂ‡Væ—BÂ¦FV6Æ&VE÷G—R’Â6öÖR†&6R’“°¢ÆWB&öG’ÒÖWF†öBæ&öG’æW‡V7B‚&ÖWF†öB†2G—VB&öG’"“°¢ÆWB×WBæöFW2ÒfV3£¦æWr‚“°¢vÆµ÷G—VE÷7V'G&VR‡Væ—BÂ&öG’Âf×WBÇG&VWÂæöFW2çW6‚‡G&VR’“°¢ÆWB6öæF—F–öæÇ2ÒæöFW0¢æ—FW"‚¢æf–ÇFW"‡ÇG&VWÂÖF6†W2‡Væ—BçG—VEö7BævWB‚¢§G&VR’æ¶–æBÂG&VT¶–æC£¤–b…ò’’¢æ6÷–VB‚¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW†6öæF—F–öæÇ2æÆVâ‚’ÂÂ'¶ÖWF†öEöæÖWÒ†2öæR6–&Æ–ær–b"“°¢ÆWB6öæF—F–öæÂÒ6öæF—F–öæÇ5³Ó°¢76W'B€¢ÖF6†W2€¢Væ—Bç7F÷&RçG—W2ævWB‡Væ—BçG—VEö7BævWB†6öæF—F–öæÂ’çG’’À¢G—S£¤÷"²ââĞ¢’À¢'¶ÖWF†öEöæÖWÒ&WF–ç2F†R÷&–v–æÂ6÷W&6RG—S£¤÷" ¢“°¢ÆWB&ö¦V7F–öâÒVæ—@¢ç&ö¦V7E÷6–&Æ–æu÷Væ–öå÷FõöW‡V7FVE÷G—R€¢Væ—BçG—VEö7BævWB†6öæF—F–öæÂ’çG’À¢¦FV6Æ&VE÷G—RÀ¢FVf–æ—F–öç2À¢¢æW‡V7B‚&FV6Æ&VB&6R7WÆ–W2F†RW†—7F–ær6†V6¶VB&ö¦V7F–öâ6öçFW‡B"“°¢76W'EöW‡&ö¦V7F–öâç&W&W6VçFF–öâ‚’Â§fÕG—S£¥&VfW&Væ6R†&6R’“°¢Ğ¢Ò’À¢¢æW‡V7B‚&FV6Æ&VB&6RÆö6Ç266WBF†V—"6†V6¶VB6–&Æ–ær–bfÇVW2"“°¢76W'EöW†ö'6W'fVEöÖWF†öG2ÂB“°¢ÆWB'VææW"Ò'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ%'VææW""¢æW‡V7B‚%'VææW"6Æ72—2VÖ—GFVB"“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr‡'VææW"æ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFR'VææW"6Æ72"“°¢6Æ70¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚%'VææW"w26Æ76f–ÆR&VfW&Væ6W2&RfÆ–B"“° ¢f÷"ÖWF†öB–â²&–æ—B"Â&–æ—E&WfW'6R"Â'&WÆ6R"Â'&WÆ6TæW7FVB%Ò°¢ÆWB6öFRÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ72ÂÖWF†öB“°¢ÆWBW‡V7FVE÷7F÷&W2ÒW6—¦S£¦g&öÒ†ÖWF†öBç7F'G5÷v—F‚‚'&WÆ6R"’’²°¢76W'EöW€¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ—FW"‚¢æf–ÇFW"‡Â…òÂ÷6öFR—ÂÖF6†W2†÷6öFRÂƒ6ÂƒF"âãÓƒFR’¢æ6÷VçB‚’À¢W‡V7FVE÷7F÷&W2À¢'¶ÖWF†öGÒ7F÷&W2F†R–æ—F–Æ—¦VBÆö6Âöæ6RæBFG2öæR7F÷&RöæÇ’f÷"&V76–væÖVçB ¢“°¢76W'B€¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ—FW"‚¢æÆÂ‡Â…òÂ÷6öFR—Â¦÷6öFRÒ†3’À¢'¶ÖWF†öGÒ&ö¦V7G2F†R&÷fVâVæ–öâFò&6Rv—F†÷WB6†V6¶67B ¢“°¢76W'B€¢7F6µöÖög&ÖW2‚f6Æ72ÂÖWF†öB’æ—FW"‚’æç’‡Æg&ÖWÂ°¢g&ÖRç7F6²ÓÒµ7F6´ÖfW&–f–6F–öåG—S£¤ö&¦V7B‚$&6R"çFõö÷væVB‚’•Ğ¢Ò’À¢'¶ÖWF†öGÒÖW&vW2F†RGvò'&æ6‚fÇVW22&6RöâF†R÷W&æB7F6² ¢“°¢Ğ¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f6Æ72À¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â&–æ—B"’À¢e°¢ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ%&–v‡B"Â#Æ–æ—Câ"Â"„’•b"’À¢ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f6Æ72À¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â&–æ—E&WfW'6R"’À¢e°¢ƒ†#rÂ%&–v‡B"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"’À¢ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f6Æ72À¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â'&WÆ6R"’À¢e°¢ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ%&–v‡B"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"’À¢ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f6Æ72À¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â'&WÆ6TæW7FVB"’À¢e°¢ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ%&–v‡B"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"’À¢ÒÀ¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚$6öçFW‡GVÅ6–&Æ–æt–d†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ726öçFW‡GVÅ6–&Æ–æt–d†&æW72²&—fFR7FF–2fö–B6†V6²„&6RfÇVRÂ–çBW‡V7FVB’²–b‡fÇVRçfÇVR‚’ÒW‡V7FVB’F‡&÷ræWr76W'F–öäW'&÷"‡fÇVRævWD6Æ72‚’²Â"ÓâÂ"²fÇVRçfÇVR‚’“²ÒV&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²'VææW"'VææW"ÒæWr'VææW"‚“²6†V6²‡'VææW"æ–æ—B‡G'VRÂ’Â“²6†V6²‡'VææW"æ–æ—B†fÇ6RÂ’Â"“²6†V6²‡'VææW"æ–æ—E&WfW'6R‡G'VRÂ’Â"“²6†V6²‡'VææW"æ–æ—E&WfW'6R†fÇ6RÂ’Â“²6†V6²‡'VææW"ç&WÆ6R‡G'VRÂ’Â"“²6†V6²‡'VææW"ç&WÆ6R†fÇ6RÂ’Â“²6†V6²‡'VææW"ç&WÆ6TæW7FVB‡G'VRÂ’Â"“²6†V6²‡'VææW"ç&WÆ6TæW7FVB†fÇ6RÂ’Â“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FR6öçFW‡GVÂ6–&Æ–ær–b†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWE÷F‚Â"ÖB"Â÷WGWE÷F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVB6öçFW‡GVÂ6–&Æ–ær–b†&æW75ÅÆç7FF÷WC¥ÅÆç·ÕÅÆç7FFW'#¥ÅÆç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢$6öçFW‡GVÅ6–&Æ–æt–d†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W""“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVB6öçFW‡GVÂ6–&Æ–ær–b6Æ76W5ÅÆç7FF÷WC¥ÅÆç·ÕÅÆç7FFW'#¥ÅÆç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ6öçFW‡GVÅ÷6–&Æ–æuö–e÷&ö¦V7F–öå÷&V¦V7G5ö–æfW'&VEöæEö–æ6ö×F–&ÆU÷F&vWG2‚’°¢5¶FW&—fR„6ÆöæRÂ6÷’•Ğ¢VçVÒW‡V7FVDf–ÇW&R°¢Vç7W÷'FVE&VfW&Væ6UG—RÀ¢Æö6ÅfÇVUG—TÖ—6ÖF6‚À¢Ğ ¢ÆWB66W2Ò°¢€¢&–æfW'&VB×6–&Æ–ærÖ–bÖÆö6Â×&V¦V7FVB"À¢&6Æ72&6R‡6VVC¢–çB•Ææ6Æ72ÆVgB‡6VVC¢–çB’W‡FVæG2&6R‡6VVB•Ææ6Æ72&–v‡B‡6VVC¢–çB’W‡FVæG2&6R‡6VVB•Ææ6Æ72'VææW#¥ÆâFVb–æfW"†fÆs¢&ööÆVâÂ6VVC¢–çB“¢–çBÕÆâf"7W'&VçBÒ–bfÆrF†VâæWrÆVgB‡6VVB’VÇ6RæWr&–v‡B‡6VVB•ÆâC%Æâ"À¢W‡V7FVDf–ÇW&S£¥Vç7W÷'FVE&VfW&Væ6UG—RÀ¢’À¢€¢&æ'&÷r×6–&Æ–ærÖ–bÖÆö6Â×&V¦V7FVB"À¢&6Æ72&6R‡6VVC¢–çB•Ææ6Æ72ÆVgB‡6VVC¢–çB’W‡FVæG2&6R‡6VVB•Ææ6Æ72&–v‡B‡6VVC¢–çB’W‡FVæG2&6R‡6VVB•Ææ6Æ72'VææW#¥ÆâFVbæ'&÷r†fÆs¢&ööÆVâÂ6VVC¢–çB“¢–çBÕÆâf"7W'&VçC¢ÆVgBÒ–bfÆrF†VâæWrÆVgB‡6VVB’VÇ6RæWr&–v‡B‡6VVB•ÆâC%Æâ"À¢W‡V7FVDf–ÇW&S£¤Æö6ÅfÇVUG—TÖ—6ÖF6‚À¢’À¢€¢'Vç&VÆFVB×6–&Æ–ærÖ–bÖÆö6Â×&V¦V7FVB"À¢&6Æ72&6R‡6VVC¢–çB•Ææ6Æ72ÆVgB‡6VVC¢–çB’W‡FVæG2&6R‡6VVB•Ææ6Æ72÷F†W"‡6VVC¢–çB•Ææ6Æ72'VææW#¥ÆâFVbVç&VÆFVB†fÆs¢&ööÆVâÂ6VVC¢–çB“¢–çBÕÆâf"7W'&VçC¢&6RÒ–bfÆrF†VâæWrÆVgB‡6VVB’VÇ6RæWr÷F†W"‡6VVB•ÆâC%Æâ"À¢W‡V7FVDf–ÇW&S£¤Æö6ÅfÇVUG—TÖ—6ÖF6‚À¢’À¢Ó° ¢f÷"†Æ&VÂÂ6÷W&6RÂW‡V7FVEöf–ÇW&R’–â66W2°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr†Æ&VÂ“°¢ÆWBf–ÇW&RÒ6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R‡6÷W&6RÂ÷WGWBçF‚‚’¢æW‡V7EöW'"‚'Vç7W÷'FVB÷"–æ6ö×F–&ÆR6–&Æ–ærVæ–öç2×W7Bf–Â6Æ÷6VB"“°¢ÆWBÖF6†W5öW‡V7FVBÒÖF6‚W‡V7FVEöf–ÇW&R°¢W‡V7FVDf–ÇW&S£¥Vç7W÷'FVE&VfW&Væ6UG—RÓâÖF6†W2€¢ff–ÇW&RÀ¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¥Vç7W÷'FVD§fÕ&VfW&Væ6UG—R²ââÒ¢’À¢W‡V7FVDf–ÇW&S£¤Æö6ÅfÇVUG—TÖ—6ÖF6‚ÓâÖF6†W2€¢ff–ÇW&RÀ¢—VÆ–æTf–ÇW&S£¥G—W"…G—W$W'&÷#£¤Æö6ÅfÇVUG—TÖ—6ÖF6‚²ââÒ¢’À¢Ó°¢76W'B€¢ÖF6†W5öW‡V7FVBÀ¢'¶Æ&VÇÒf–ÆVBv—F‚âVæW‡V7FVBW'&÷#¢¶f–ÇW&S£÷Ò ¢“°¢76W'B€¢÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’À¢'¶Æ&VÇÒ×W7Bæ÷BV&Æ—6‚ç’6Æ72g&öÒ&V¦V7FVB6÷W&6R&F6‚ ¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ×WF&ÆU÷&VfW&Væ6UöÆö6Åö76–væÖVçG5ö¦ö–åö7&÷75ö–eöæE÷v†–ÆUöæEöF—7F6‚‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&VfW&Væ6R×f"Ö–b×v†–ÆRÖ¦ö–ç2"“°¢ÆWB6÷W&6RÒ&6Æ72&6R‡6VVC¢–çB“¥Æâ&—fFRfÂ7F÷&VC¢–çBÒ6VVEÆâFVbfÇVR‚“¢–çBÒ7F÷&VEÆæ6Æ72ÆVgB‡6VVC¢–çB’W‡FVæG2&6R‡6VVB“¥Æâ÷fW'&–FRFVbfÇVR‚“¢–çBÒ7WW"çfÇVR‚’²Ææ6Æ72&–v‡B‡6VVC¢–çB’W‡FVæG2&6R‡6VVB“¥Æâ÷fW'&–FRFVbfÇVR‚“¢–çBÒ7WW"çfÇVR‚’²%Ææ6Æ72'VææW#¥ÆâFVb6†ö÷6R†fÆs¢&ööÆVâÂ6VVC¢–çB“¢&6RÕÆâf"7W'&VçC¢&6RÒæWrÆVgB‡6VVB•Æâ–bfÆrF†Vâ7W'&VçBÒæWr&–v‡B‡6VVB•Æâ7W'&VçEÆâFVb6†ö÷6U&WfW'6R†fÆs¢&ööÆVâÂ6VVC¢–çB“¢&6RÕÆâf"7W'&VçC¢&6RÒæWr&–v‡B‡6VVB•Æâ–bfÆrF†Vâ7W'&VçBÒæWrÆVgB‡6VVB•Æâ7W'&VçEÆâFVb&WVB†ã¢–çBÂ6VVC¢–çB“¢&6RÕÆâf"7W'&VçC¢&6RÒæWrÆVgB‡6VVB•Æâf"’ÒÆâv†–ÆR’ÂâFõÆâ7W'&VçBÒæWr&–v‡B‡6VVB•Æâ’Ò’²Æâ7W'&VçEÆâFVbæW7FVB†÷WFW#¢&ööÆVâÂ–ææW#¢&ööÆVâÂ6VVC¢–çB“¢&6RÕÆâf"7W'&VçC¢&6RÒæWr&–v‡B‡6VVB•Æâ–b÷WFW"F†VåÆâ–b–ææW"F†Vâ7W'&VçBÒæWrÆVgB‡6VVB’VÇ6R7W'&VçBÒæWr&–v‡B‡6VVB•ÆâVÇ6R7W'&VçBÒæWrÆVgB‡6VVB•Æâ7W'&VçEÆâ#°¢ÆWB6÷W&6RÒf÷&ÖB€¢'·6÷W&6WÒFVbV&Ç’†fÆs¢&ööÆVâÂ6VVC¢–çB“¢&6RÕÆâf"7W'&VçC¢&6RÒæWrÆVgB‡6VVB•Æâ–bfÆrF†Vâ7W'&VçBÒæWr&–v‡B‡6VVB’VÇ6R&WGW&â7W'&VçEÆâ7W'&VçEÆâ ¢“°¢ÆWB'F–f7G2Ò6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R‚g6÷W&6RÂ÷WGWBçF‚‚’¢æW‡V7B‚&–æ—F–Æ—¦VB×WF&ÆR&VfW&Væ6RÆö6Ç2¦ö–â7&÷727W÷'FVB6öçG&öÂfÆ÷r"“°¢ÆWB'VææW"Ò'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ%'VææW""¢æW‡V7B‚%'VææW"6Æ72—2VÖ—GFVB"“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr‡'VææW"æ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFR'VææW"6Æ72"“°¢6Æ70¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚%'VææW"w26Æ76f–ÆR&VfW&Væ6W2&RfÆ–B"“° ¢f÷"†ÖWF†öBÂÆö6Å÷6Æ÷B’–â°¢‚&6†ö÷6R"Â2’À¢‚&6†ö÷6U&WfW'6R"Â2’À¢‚'&WVB"Â2’À¢‚&æW7FVB"ÂB’À¢Ò°¢ÆWB6öFRÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ72ÂÖWF†öB“°¢76W'B€¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ—FW"‚¢æÆÂ‡Â…òÂ÷6öFR—Â¦÷6öFRÒ†3’À¢'¶ÖWF†öGÒ¦ö–ç2F†RÆö6ÂB—G2FV6Æ&VB&6RG—Rv—F†÷WB6†V6¶67B ¢“°¢76W'B€¢7F6µöÖög&ÖW2‚f6Æ72ÂÖWF†öB’æ—FW"‚’æç’‡Æg&ÖWÂ°¢g&ÖRæÆö6Ç2ævWB†Æö6Å÷6Æ÷B¢ÓÒ6öÖR‚e7F6´ÖfW&–f–6F–öåG—S£¤ö&¦V7B‚$&6R"çFõö÷væVB‚’’¢Ò’À¢'¶ÖWF†öGÒVæ6öFW26æöæ–6ÂÆö6Â6Æ÷B¶Æö6Å÷6Æ÷GÒ2ö&¦V7B„&6R’B4dr¦ö–â ¢“°¢Ğ¢f÷"†ÖWF†öBÂÆö6Å÷6Æ÷B’–â²‚&6†ö÷6R"Â2’Â‚&6†ö÷6U&WfW'6R"Â2’Â‚&æW7FVB"ÂB•Ò°¢ÆWB6öFRÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ72ÂÖWF†öB“°¢ÆWB¦ö–å÷F&vWG2ÒFV6öFUö'&æ6…÷F&vWG2†6öFR¢æ–çFõö—FW"‚¢æf–ÇFW%öÖ‡Â…òÂ÷6öFRÂF&vWB—Â†÷6öFRÓÒ†r’çF†Vå÷6öÖR‡F&vWB’¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢ÆWBg&ÖW2Ò7F6µöÖög&ÖW2‚f6Æ72ÂÖWF†öB“°¢76W'B€¢¦ö–å÷F&vWG2æ—FW"‚’æç’‡ÇF&vWGÂ°¢g&ÖW2æ—FW"‚’æç’‡Æg&ÖWÂ°¢g&ÖRç2ÓÒ§F&vW@¢bbg&ÖRæÆö6Ç2ævWB†Æö6Å÷6Æ÷B¢ÓÒ6öÖR‚e7F6´ÖfW&–f–6F–öåG—S£¤ö&¦V7B‚$&6R"çFõö÷væVB‚’’¢Ò¢Ò’À¢'¶ÖWF†öGÒ†2âö&¦V7B„&6R’Æö6Âg&ÖRB&V6†&ÆR'&æ6‚¦ö–â ¢“°¢Ğ¢ÆWB&WVEö6öFRÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â'&WVB"“°¢ÆWB&WVE÷F&vWG2ÒFV6öFUö'&æ6…÷F&vWG2‡&WVEö6öFR“°¢ÆWBv÷Fõ÷F&vWG2Ò&WVE÷F&vWG0¢æ—FW"‚¢æf–ÇFW%öÖ‡Â…òÂ÷6öFRÂF&vWB—Â‚¦÷6öFRÓÒ†r’çF†Vå÷6öÖR‚§F&vWB’¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢ÆWBÆö÷ö†VFW"Òv÷Fõ÷F&vWG0¢æ—FW"‚¢æ6÷–VB‚¢æf–æB‡ÇF&vWGÂ°¢v÷Fõ÷F&vWG0¢æ—FW"‚¢æf–ÇFW"‡Æ6æF–FFWÂ¦6æF–FFRÓÒF&vWB¢æ6÷VçB‚¢ÓÒ ¢Ò¢æW‡V7B‚&&÷F‚Æö÷VçG'’æB&6²ÖVFvRF&vWBF†R7–Ö&öÆ–2†VFW""“°¢ÆWB&WVEög&ÖW2Ò7F6µöÖög&ÖW2‚f6Æ72Â'&WVB"“°¢76W'B€¢&WVEög&ÖW2æ—FW"‚’æç’‡Æg&ÖWÂ°¢g&ÖRç2ÓÒÆö÷ö†VFW ¢bbg&ÖRæÆö6Ç2ævWBƒ2’ÓÒ6öÖR‚e7F6´ÖfW&–f–6F–öåG—S£¤ö&¦V7B‚$&6R"çFõö÷væVB‚’’¢bbg&ÖRæÆö6Ç2ævWBƒB’ÓÒ6öÖR‚e7F6´ÖfW&–f–6F–öåG—S£¤–çFVvW"¢Ò’À¢'F†RÆö÷Ö†VFW"g&ÖR&WF–ç2F†R&–Ö—F—fR–æFW‚&W6–FRF†R¦ö–æVB&6RÆö6Â ¢“°¢ÆWBÆö÷öW†—BÒ&WVE÷F&vWG0¢æ—FW"‚¢æf–æEöÖ‡Â…òÂ÷6öFRÂF&vWB—Â‚¦÷6öFRÓÒƒ“’’çF†Vå÷6öÖR‚§F&vWB’¢æW‡V7B‚'F†RfÇ6Rv†–ÆRÖ6öæF—F–öâVFvRF&vWG2F†RÆö÷W†—B"“°¢76W'B€¢&WVEög&ÖW2æ—FW"‚’æç’‡Æg&ÖWÂg&ÖRç2ÓÒÆö÷öW†—@¢bbg&ÖRæÆö6Ç2ævWBƒ2’ÓÒ6öÖR‚e7F6´ÖfW&–f–6F–öåG—S£¤ö&¦V7B‚$&6R"çFõö÷væVB‚’’’’À¢'F†R¦W&òÖ—FW&F–öâW†—Bg&ÖR&W6W'fW2F†R–æ—F–Æ—¦VB&6RÆö6Â ¢“°¢f÷"†ÖWF†öBÂW‡V7FVE÷7F÷&W2’–â°¢‚&6†ö÷6R"Â"’À¢‚&6†ö÷6U&WfW'6R"Â"’À¢‚'&WVB"Â"’À¢‚&æW7FVB"ÂB’À¢‚&V&Ç’"Â"’À¢Ò°¢ÆWB7F÷&W2ÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72ÂÖWF†öB’¢æ—FW"‚¢æf–ÇFW"‡Â…òÂ÷6öFR—ÂÖF6†W2†÷6öFRÂƒ6ÂƒF"âãÓƒFR’¢æ6÷VçB‚“°¢76W'EöW€¢7F÷&W2ÂW‡V7FVE÷7F÷&W2À¢'¶ÖWF†öGÒVÖ—G2V6‚Æö6Â7F÷&Röæ6R ¢“°¢Ğ¢f÷"†ÖWF†öBÂW‡V7FVEö6öç7G'V7F÷'2’–â°¢€¢&6†ö÷6R"À¢fV2°¢ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ%&–v‡B"Â#Æ–æ—Câ"Â"„’•b"’À¢ÒÀ¢’À¢€¢&6†ö÷6U&WfW'6R"À¢fV2°¢ƒ†#rÂ%&–v‡B"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"’À¢ÒÀ¢’À¢€¢'&WVB"À¢fV2°¢ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ%&–v‡B"Â#Æ–æ—Câ"Â"„’•b"’À¢ÒÀ¢’À¢€¢&æW7FVB"À¢fV2°¢ƒ†#rÂ%&–v‡B"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ%&–v‡B"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"’À¢ÒÀ¢’À¢€¢&V&Ç’"À¢fV2°¢ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ%&–v‡B"Â#Æ–æ—Câ"Â"„’•b"’À¢ÒÀ¢’À¢Ò°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f6Æ72À¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72ÂÖWF†öB’À¢fW‡V7FVEö6öç7G'V7F÷'2À¢“°¢Ğ ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚$×WF&ÆU&VfW&Væ6T6ft†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢"2'V&Æ–2f–æÂ6Æ72×WF&ÆU&VfW&Væ6T6ft†&æW72°¢&—fFR7FF–2fö–B6†V6²„&6RfÇVRÂ7G&–ærG—RÂ–çBW‡V7FVB’°¢–b‚fÇVRævWD6Æ72‚’ævWE6–×ÆTæÖR‚’æWVÇ2‡G—R’ÇÂfÇVRçfÇVR‚’ÒW‡V7FVB¢F‡&÷ræWr76W'F–öäW'&÷"‡fÇVRævWD6Æ72‚’²"Óâ"²fÇVRçfÇVR‚’²"ÂW‡V7FVB"²G—R²"ò"²W‡V7FVB“°¢Ğ¢V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’°¢'VææW"'VææW"ÒæWr'VææW"‚“°¢f÷"†–çB6VVB¢æWr–çEµÒ³ÂÂÓ7Ò’°¢6†V6²‡'VææW"æ6†ö÷6R†fÇ6RÂ6VVB’Â$ÆVgB"Â6VVB²“°¢6†V6²‡'VææW"æ6†ö÷6R‡G'VRÂ6VVB’Â%&–v‡B"Â6VVB²"“°¢6†V6²‡'VææW"æ6†ö÷6U&WfW'6R†fÇ6RÂ6VVB’Â%&–v‡B"Â6VVB²"“°¢6†V6²‡'VææW"æ6†ö÷6U&WfW'6R‡G'VRÂ6VVB’Â$ÆVgB"Â6VVB²“°¢6†V6²‡'VææW"ç&WVBƒÂ6VVB’Â$ÆVgB"Â6VVB²“°¢6†V6²‡'VææW"ç&WVBƒÂ6VVB’Â%&–v‡B"Â6VVB²"“°¢6†V6²‡'VææW"ç&WVBƒ2Â6VVB’Â%&–v‡B"Â6VVB²"“°¢6†V6²‡'VææW"ææW7FVB†fÇ6RÂG'VRÂ6VVB’Â$ÆVgB"Â6VVB²“°¢6†V6²‡'VææW"ææW7FVB‡G'VRÂfÇ6RÂ6VVB’Â%&–v‡B"Â6VVB²"“°¢6†V6²‡'VææW"ææW7FVB‡G'VRÂG'VRÂ6VVB’Â$ÆVgB"Â6VVB²“°¢6†V6²‡'VææW"æV&Ç’†fÇ6RÂ6VVB’Â$ÆVgB"Â6VVB²“°¢6†V6²‡'VææW"æV&Ç’‡G'VRÂ6VVB’Â%&–v‡B"Â6VVB²"“°¢Ğ¢Ğ§Ğ¢"2À¢¢æW‡V7B‚'w&—FR×WF&ÆR×&VfW&Væ6R4dr†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWE÷F‚Â"ÖB"Â÷WGWE÷F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVB×WF&ÆR×&VfW&Væ6R4dr†&æW75ÅÆç7FF÷WC¥ÅÆç·ÕÅÆç7FFW'#¥ÅÆç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢$×WF&ÆU&VfW&Væ6T6ft†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W""“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVB×WF&ÆR×&VfW&Væ6R4dr6Æ76W5ÅÆç7FF÷WC¥ÅÆç·ÕÅÆç7FFW'#¥ÅÆç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ×WF&ÆU÷&VfW&Væ6UöÆö6Åö76–væÖVçEö—5÷&V¦V7FVEö–å÷v†–ÆUö6öæF—F–öç2‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&VfW&Væ6R×f"×v†–ÆRÖ6öæF—F–öâÖ76–væÖVçB×&V¦V7FVB"“°¢ÆWB&W7VÇBÒ6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R€¢&6Æ72&6R‡6VVC¢–çB•Ææ6Æ72ÆVb‡6VVC¢–çB’W‡FVæG2&6R‡6VVB•Ææ6Æ72'VææW#¥ÆâFVb6†ævR†fÆs¢&ööÆVâÂ6VVC¢–çB“¢–çBÕÆâf"7W'&VçC¢&6RÒæWrÆVb‡6VVB•Æâv†–ÆR²7W'&VçBÒæWrÆVb‡6VVB“²fÆrÒFò‚•ÆâC%Æâ"À¢÷WGWBçF‚‚’À¢“°¢76W'B€¢ÖF6†W2€¢&W7VÇBÀ¢W'"…—VÆ–æTf–ÇW&S£¤6öFVvVâ€¢§fÔ6öFVvVäW'&÷#£¥&VfW&Væ6TÆö6Ä–åVç7W÷'FVD6öçG&öÄfÆ÷r²ââĞ¢’¢’À¢&76–væÖVçG2–â&WVFVFÇ’WfÇVFVBv†–ÆR6öæF—F–öâ×W7B&R&V¦V7FVB ¢“°¢76W'B†÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°§Ğ ¢5·FW7EĞ¦fâ×WF&ÆU÷&VfW&Væ6UöÆö6Åö76–væÖVçEö—5÷&V¦V7FVEö–å÷6†÷'Eö6—&7V—EöæE÷&‡2‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&VfW&Væ6R×f"×6†÷'BÖ6—&7V—BÖæBÖ76–væÖVçB×&V¦V7FVB"“°¢ÆWB&W7VÇBÒ6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R€¢&6Æ72&6R‡6VVC¢–çB•Ææ6Æ72ÆVb‡6VVC¢–çB’W‡FVæG2&6R‡6VVB•Ææ6Æ72'VææW#¥ÆâFVb6†ævR†fÆs¢&ööÆVâÂ6VVC¢–çB“¢–çBÕÆâf"7W'&VçC¢&6RÒæWrÆVb‡6VVB•Æâ–bfÆrbb²7W'&VçBÒæWrÆVb‡6VVB“²G'VRÒF†VâVÇ6RÆâ"À¢÷WGWBçF‚‚’À¢“°¢76W'B€¢ÖF6†W2€¢&W7VÇBÀ¢W'"…—VÆ–æTf–ÇW&S£¤6öFVvVâ€¢§fÔ6öFVvVäW'&÷#£¥&VfW&Væ6TÆö6Ä–åVç7W÷'FVD6öçG&öÄfÆ÷r²ââĞ¢’¢’À¢&76–væÖVçG2–â6öæF—F–öæÆÇ’WfÇVFVBbb÷W&æB×W7B&R&V¦V7FVB ¢“°¢76W'B†÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°§Ğ ¢5·FW7EĞ¦fâ×WF&ÆU÷&VfW&Væ6UöÆö6Åö76–væÖVçEö—5÷&V¦V7FVEö–å÷6†÷'Eö6—&7V—Eö÷%÷&‡2‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&VfW&Væ6R×f"×6†÷'BÖ6—&7V—BÖ÷"Ö76–væÖVçB×&V¦V7FVB"“°¢ÆWB&W7VÇBÒ6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R€¢&6Æ72&6R‡6VVC¢–çB•Ææ6Æ72ÆVb‡6VVC¢–çB’W‡FVæG2&6R‡6VVB•Ææ6Æ72'VææW#¥ÆâFVb6†ævR†fÆs¢&ööÆVâÂ6VVC¢–çB“¢–çBÕÆâf"7W'&VçC¢&6RÒæWrÆVb‡6VVB•Æâ–bfÆrÇÂ²7W'&VçBÒæWrÆVb‡6VVB“²fÇ6RÒF†VâVÇ6RÆâ"À¢÷WGWBçF‚‚’À¢“°¢76W'B€¢ÖF6†W2€¢&W7VÇBÀ¢W'"…—VÆ–æTf–ÇW&S£¤6öFVvVâ€¢§fÔ6öFVvVäW'&÷#£¥&VfW&Væ6TÆö6Ä–åVç7W÷'FVD6öçG&öÄfÆ÷r²ââĞ¢’¢’À¢&76–væÖVçG2–â6öæF—F–öæÆÇ’WfÇVFVBÇÂ÷W&æB×W7B&R&V¦V7FVB ¢“°¢76W'B†÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°§Ğ ¢5·FW7EĞ¦fâ–æ—F–Æ—¦VEö×WF&ÆU÷&VfW&Væ6UöÆö6Åö6åö&Uö6÷–VEö–çFõöåö–Ö×WF&ÆUöÆö6Â‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&VfW&Væ6R×f"Ö6÷’Ö66WFVB"“°¢ÆWB'F–f7G2Ò6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R€¢&6Æ726÷VçFW"‡6VVC¢–çB•Ææ6Æ726Æ–VçC¥ÆâFVb'Vâ‡6VVC¢–çB“¢–çBÕÆâf"6÷VçFW"ÒæWr6÷VçFW"‡6VVB•ÆâfÂ6fVBÒ6÷VçFW%ÆâC%Æâ"À¢÷WGWBçF‚‚’À¢¢æW‡V7B‚&â–æ—F–Æ—¦VB×WF&ÆR&VfW&Væ6RÆö6Â6â&R6÷–VB"“°¢ÆWB6Æ–VçBÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$6Æ–VçB"¢æW‡V7B‚$6Æ–VçB6Æ72—2VÖ—GFVB"“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†6Æ–VçBæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFR6Æ–VçB6Æ72"“°¢76W'EöW€¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2†VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â''Vâ"’¢æ—FW"‚¢æÖ‡Â…òÂ÷6öFR—Â¦÷6öFR¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢³†&"ÂƒS’Âƒ"Â†#rÂƒFBÂƒ&2ÂƒFRÂƒÂ†5ÒÀ¢'F†R–æ—F–Æ—¦VB×WF&ÆRÆö6Â—2ÆöFVBöæ6R&Vf÷&R7F÷&–ærF†R–Ö×WF&ÆR6÷’ ¢“°§Ğ ¢5·FW7EĞ¦fâ–æ—F–Æ—¦VEö×WF&ÆU÷&VfW&Væ6UöÆö6Åö6åö&UöF—66&FVEö5öåöW‡&W76–öâ‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&VfW&Væ6R×f"ÖF—66&BÖ66WFVB"“°¢ÆWB'F–f7G2Ò6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R€¢&6Æ726÷VçFW"‡6VVC¢–çB•Ææ6Æ726Æ–VçC¥ÆâFVb'Vâ‡6VVC¢–çB“¢–çBÕÆâf"6÷VçFW"ÒæWr6÷VçFW"‡6VVB•Æâ6÷VçFW%ÆâC%Æâ"À¢÷WGWBçF‚‚’À¢¢æW‡V7B‚&â–æ—F–Æ—¦VB×WF&ÆR&VfW&Væ6RÆö6Â6â&RF—66&FVB"“°¢ÆWB6Æ–VçBÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$6Æ–VçB"¢æW‡V7B‚$6Æ–VçB6Æ72—2VÖ—GFVB"“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†6Æ–VçBæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFR6Æ–VçB6Æ72"“°¢76W'EöW€¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2†VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â''Vâ"’¢æ—FW"‚¢æÖ‡Â…òÂ÷6öFR—Â¦÷6öFR¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢³†&"ÂƒS’Âƒ"Â†#rÂƒFBÂƒ&2ÂƒSrÂƒÂ†5ÒÀ¢'F†R–æ—F–Æ—¦VB×WF&ÆRÆö6Â—2ÆöFVBöæ6R&Vf÷&RF†RF—66&FVBfÇVR—2÷VB ¢“°§Ğ ¢5·FW7EĞ¦fâ×WF&ÆU÷&VfW&Væ6Uö—77VUö6æF–FFUö6ÆÇ5öF—7F6…ögFW%öÆö6Åö6fuö¦ö–ç2‚’°¢ÆWB6÷W&6RÒg3£§&VE÷Fõ÷7G&–ær†f—‡GW&U÷F‚‚$×WF&ÆU&VfW&Væ6TÆö6Ç2ç66Æ"’¢æW‡V7B‚'&VBF†R&VÂ×WF&ÆR×&VfW&Væ6RÖÆö6Â6÷W&6Rf—‡GW&R"“°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&×WF&ÆR×&VfW&Væ6RÖ6æF–FFRÖF—7F6‚Öf—'7B"“°¢ÆWB6V6öæEö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&×WF&ÆR×&VfW&Væ6RÖ6æF–FFRÖF—7F6‚×6V6öæB"“°¢ÆWB×WBö'6W'fVE÷6W76–öâÒfÇ6S°¢ÆWB×WBö'6W'fVEöÖWF†öG2Ò°¢ÆWB'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö&F6…öö'6W'fVB€¢g6÷W&6RÀ¢$×WF&ÆU&VfW&Væ6TÆö6Ç2ç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒCSB’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢6öÖR‚f×WBÇVæ—BÂFVf–æ—F–öç7Â°¢ö'6W'fVE÷6W76–öâÒG'VS°¢ÆWB&6RÒVF—Eö6Æ75öæÖVB‡Væ—BÂ$&6R"“°¢ÆWBÆVgBÒVF—Eö6Æ75öæÖVB‡Væ—BÂ$ÆVgB"“°¢ÆWB&–v‡BÒVF—Eö6Æ75öæÖVB‡Væ—BÂ%&–v‡B"“°¢ÆWB'VææW"ÒVF—Eö6Æ75öæÖVB‡Væ—BÂ%'VææW""“°¢76W'EöW†VF—Eö6Æ75÷&VçB‡Væ—BÂ&6R’ÂFVf–æ—F–öç2æö&¦V7Eö6Æ72“°¢76W'EöW†VF—Eö6Æ75÷&VçB‡Væ—BÂÆVgB’Â&6R“°¢76W'EöW†VF—Eö6Æ75÷&VçB‡Væ—BÂ&–v‡B’Â&6R“°¢ÆWBfÇVUöÖWF†öBÒVF—EöÖWF†öEöæÖVB‡Væ—BÂ&6RÂ'fÇVR"’æÖWF†öC° ¢f÷"æÖR–â²&6†ö÷6R"Â'&WVB"Â&–æ—F–Æ—¦R"Â'7v%Ò°¢ö'6W'fVEöÖWF†öG2³Ò°¢ÆWBÖWF†öBÒVF—EöÖWF†öEöæÖVB‡Væ—BÂ'VææW"ÂæÖR“°¢ÆWB&–æF–ærÒÖWF†ö@¢æÆö6Ç0¢æ—FW"‚¢æf–æB‡Æ&–æF–æwÂ°¢&–æF–æræ×WF&ÆP¢bbVæ—@¢ç7F÷&P¢ææÖW0¢ç&W6öÇfR‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB†&–æF–ærç7–Ö&öÂ’ææÖRçFW‡B‚’¢ÓÒ&7W'&VçB ¢Ò¢çVçw&ö÷%öVÇ6R‡ÇÂæ–2‚'¶æÖWÒ†2—G26æöæ–6Â×WF&ÆR7W'&VçBÆö6Â"’“°¢ÆWB7–Ö&öÂÒVæ—Bç7F÷&Rç7–Ö&öÇ2ævWB†&–æF–ærç7–Ö&öÂ“°¢76W'EöW‡7–Ö&öÂæ¶–æBÂ7–Ö&öÄ¶–æC£¤Æö6Â“°¢76W'EöW‡7–Ö&öÂæ÷væW"Â6öÖR†ÖWF†öBæÖWF†öB’“°¢76W'B‡7–Ö&öÂæfÆw2æ6öçF–ç2…7–Ö&öÄfÆw3£¤ÕUD$ÄR’“°¢ÆWBF÷GG•ö6÷&S£¥7–Ö&öÄ–æfó£¤6ö×ÆWFR†FV6Æ&VE÷G—R’Òg7–Ö&öÂæ–æfòVÇ6R°¢æ–2‚&7W'&VçB†26ö×ÆWFR6÷W&6RG—R–æf÷&ÖF–öâ"¢Ó°¢76W'EöW†VF—E÷G—U÷7–Ö&öÂ‡Væ—BÂ¦FV6Æ&VE÷G—R’Â6öÖR†&6R’“°¢ÆWBÆ–÷WBÒVæ—@¢æÆö6ÅöÆ–÷WB‡'VææW"ÂÖWF†öBæÖWF†öBÂFVf–æ—F–öç2¢æW‡V7B‚&ÆÂÆææVB6Æ76W2æBÆö6Ç26†&RF†—2G—VB6W76–öâ"“°¢ÆWB6Æ÷BÒÆ–÷W@¢æÆö6Â†&–æF–ærç7–Ö&öÂ¢æW‡V7B‚&Æ–÷WBW6W2F†R6æöæ–6ÂÆö6Â7–Ö&öÄ–B"“°¢76W'EöW‡6Æ÷BçG’‚’Â§fÕG—S£¥&VfW&Væ6R†&6R’“°¢76W'B‡6Æ÷Bæ—5ö×WF&ÆR‚’“°¢ÆWB&öG’ÒÖWF†öBæ&öG’æW‡V7B‚&ÖWF†öB†2G—VB&öG’"“°¢76W'B€¢G—VE÷G&VU÷F&vWG2‡Væ—BÂ&öG’’æ6öçF–ç2‚gfÇVUöÖWF†öB’À¢'¶æÖWÒ6VÆV7G2&6RçfÇVRF‡&÷Vv‚F†R6ÖRG—VB7–Ö&öÂw&‚ ¢“°¢Ğ¢76W'EöW†ö'6W'fVEöÖWF†öG2ÂB“°¢Ò’À¢¢æW‡V7B‚&×WF&ÆRÆö6ÂÖWF†öB6ÆÇ26ö×–ÆRF‡&÷Vv‚F†R¥dÒ&6¶VæB"“°¢76W'B€¢ö'6W'fVE÷6W76–öâÀ¢'F†R6ö×ÆWFRg&öçFVæB&V6†VBöæRG—VB6W76–öâ ¢“°¢76W'EöW€¢'F–f7G0¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢²$&6R"Â$ÆVgB"Â%&–v‡B"Â%'VææW"%Ğ¢“°¢ÆWB6V6öæBÒ6ö×–ÆU÷6÷W&6Uö&F6‚€¢g6÷W&6RÀ¢$×WF&ÆU&VfW&Væ6TÆö6Ç2ç66Æ"À¢6V6öæEö÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒCSB’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢¢æW‡V7B‚&g&W6‚g&öçFVæB6W76–öâ–æFWVæFVçFÇ’VÖ—G2F†R6ÖR6÷W&6R&F6‚"“°¢76W'EöW‡6V6öæBæÆVâ‚’Â'F–f7G2æÆVâ‚’“°¢76W'EöW†÷WGWBæ6Æ75öf–ÆW2‚’æÆVâ‚’ÂB“°¢76W'EöW‡6V6öæEö÷WGWBæ6Æ75öf–ÆW2‚’æÆVâ‚’ÂB“°¢f÷"†f—'7BÂ6V6öæB’–â'F–f7G2æ—FW"‚’ç¦—‚g6V6öæB’°¢76W'EöW†f—'7Bæ6Æ72æ–çFW&æÅöæÖR‚’Â6V6öæBæ6Æ72æ–çFW&æÅöæÖR‚’“°¢76W'EöW€¢f—'7Bæ6Æ72æ'—FW2‚’À¢6V6öæBæ6Æ72æ'—FW2‚’À¢'·Ò—2'—FRÖ–FVçF–6Â7&÷72–æFWVæFVçBg&öçFVæB6W76–öç2"À¢f—'7Bæ6Æ72æ–çFW&æÅöæÖR‚¢“°¢76W'EöW€¢g3£§&VB‚ff—'7BçF‚’æW‡V7B‚'&VBf—'7BV&Æ—6†VB6Æ72"’À¢f—'7Bæ6Æ72æ'—FW2‚¢“°¢76W'EöW€¢g3£§&VB‚g6V6öæBçF‚’æW‡V7B‚'&VB6V6öæBV&Æ—6†VB6Æ72"’À¢6V6öæBæ6Æ72æ'—FW2‚¢“° ¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†f—'7Bæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFRWfW'’VÖ—GFVB6Æ72"“°¢6Æ70¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚&WfW'’VÖ—GFVB6Æ72†2fÆ–B6öç7FçB×ööÂ&VfW&Væ6W2"“°¢76W'EöW†6Æ72çfW'6–öâæÖ¦÷"ÂcRÂ$¥dÒ#6Æ76f–ÆRfW'6–öâ"“°¢76W'B†6Æ72æ66W75öfÆw2æ—5÷V&Æ–2‚’“°¢ÆWBW‡V7FVE÷7WW"ÒÖF6‚f—'7Bæ6Æ72æ–çFW&æÅöæÖR‚’°¢$&6R"Â%'VææW""Óâ&¦föÆærôö&¦V7B"À¢$ÆVgB"Â%&–v‡B"Óâ$&6R"À¢æÖRÓâæ–2‚'VæW‡V7FVB6Æ72–â66WFæ6Rf—‡GW&S¢¶æÖWÒ"’À¢Ó°¢76W'EöW€¢6Æ70¢æ6öç7FçE÷ööÀ¢æ6Æ75öæÖR†6Æ72ç7WW%ö6Æ72æW‡V7B‚'6÷W&6R6Æ727WW&6Æ72"’’À¢ö²†W‡V7FVE÷7WW"’À¢'·Ò¶VW2—G2F—&V7B7WW&6Æ72"À¢f—'7Bæ6Æ72æ–çFW&æÅöæÖR‚¢“°¢Ğ ¢ÆWB&6Uö'F–f7BÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$&6R"¢æW‡V7B‚$&6R—2VÖ—GFVB"“°¢ÆWB×WB&6U÷&VFW"Ò&VFW#£¦æWr†&6Uö'F–f7Bæ6Æ72æ'—FW2‚’“°¢ÆWB&6RÒ6Æ74f–ÆS£¦FV6öFR‚f×WB&6U÷&VFW"’æW‡V7B‚&FV6öFR&6R"“°¢76W'EöW†&6Ræf–VÆG2æÆVâ‚’ÂÂ&–æ†W&—FVBf–VÆG2&Ræ÷B6÷–VB"“°¢ÆWB7W'&VçBÒf&6Ræf–VÆG5³Ó°¢76W'EöW†&6Ræ6öç7FçE÷ööÂçWFc‚†7W'&VçBææÖUö–æFW‚’Âö²‚&7W'&VçB"’“°¢76W'EöW†&6Ræ6öç7FçE÷ööÂçWFc‚†7W'&VçBæFW67&—F÷%ö–æFW‚’Âö²‚$’"’“°¢76W'B†7W'&VçBæ66W75öfÆw2æ—5÷&—fFR‚’“°¢76W'B‚7W'&VçBæ66W75öfÆw2æ—5öf–æÂ‚’Â$&6Ræ7W'&VçB—2×WF&ÆR"“°¢76W'EöW€¢&6RæÖWF†öG0¢æ—FW"‚¢æf–ÇFW"‡ÆÖWF†öGÂ&6Ræ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’ÓÒö²‚'fÇVR"’¢æ6÷VçB‚’À¢¢“°¢ÆWB&6Uö6öç7G'V7F÷"ÒVÖ—GFVEöÖWF†öEö6öFR‚f&6RÂ#Æ–æ—Câ"“°¢76W'Eöf–VÆE÷&VfW&Væ6R‚f&6RÂ&6Uö6öç7G'V7F÷"Â†#RÂ$&6R"Â&7W'&VçB"Â$’"“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f&6RÀ¢&6Uö6öç7G'V7F÷"À¢e²ƒ†#rÂ&¦föÆærôö&¦V7B"Â#Æ–æ—Câ"Â"‚•b"•ÒÀ¢“°¢76W'Eöf–VÆE÷&VfW&Væ6R€¢f&6RÀ¢VÖ—GFVEöÖWF†öEö6öFR‚f&6RÂ'fÇVR"’À¢†#BÀ¢$&6R"À¢&7W'&VçB"À¢$’"À¢“° ¢f÷"6Æ75öæÖR–â²$ÆVgB"Â%&–v‡B%Ò°¢ÆWB'F–f7BÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ6Æ75öæÖR¢æW‡V7B‚'7V&6Æ72—2VÖ—GFVB"“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†'F–f7Bæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFR7V&6Æ72"“°¢76W'B€¢6Æ72æf–VÆG2æ—5öV×G’‚’À¢'¶6Æ75öæÖWÒFöW2æ÷BGWÆ–6FR&6Rf–VÆG2 ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f6Æ72À¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â#Æ–æ—Câ"’À¢e²ƒ†#rÂ$&6R"Â#Æ–æ—Câ"Â"„’•b"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f6Æ72À¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â'fÇVR"’À¢e²ƒ†#rÂ$&6R"Â'fÇVR"Â"‚”’"•ÒÀ¢“°¢Ğ ¢ÆWB'VææW%ö'F–f7BÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ%'VææW""¢æW‡V7B‚%'VææW"—2VÖ—GFVB"“°¢ÆWB×WB'VææW%÷&VFW"Ò&VFW#£¦æWr‡'VææW%ö'F–f7Bæ6Æ72æ'—FW2‚’“°¢ÆWB'VææW%ö6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB'VææW%÷&VFW"’æW‡V7B‚&FV6öFR'VææW""“°¢76W'B‡'VææW%ö6Æ72æf–VÆG2æ—5öV×G’‚’“°¢f÷"ÖWF†öB–â²&6†ö÷6R"Â'&WVB"Â&–æ—F–Æ—¦R"Â'7v%Ò°¢ÆWB6öFRÒVÖ—GFVEöÖWF†öEö6öFR‚g'VææW%ö6Æ72ÂÖWF†öB“°¢76W'B€¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ—FW"‚¢æç’‡Â…òÂ÷6öFR—Â¦÷6öFRÓÒ†#b’À¢'¶ÖWF†öGÒ–çfö¶W2F†R6æöæ–6Â&6RçfÇVRÖWF†öBf—'GVÆÇ’ ¢“°¢ÆWBW‡V7FVBÒÖF6‚ÖWF†öB°¢&6†ö÷6R"Â'&WVB"Â&–æ—F–Æ—¦R"ÓâfV2°¢ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ%&–v‡B"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#bÂ$&6R"Â'fÇVR"Â"‚”’"’À¢ÒÀ¢'7v"ÓâfV2°¢ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ%&–v‡B"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#rÂ$ÆVgB"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#bÂ$&6R"Â'fÇVR"Â"‚”’"’À¢ÒÀ¢òÓâVç&V6†&ÆR‚’À¢Ó°¢76W'Eö–çfö6F–öå÷6WVVæ6R‚g'VææW%ö6Æ72Â6öFRÂfW‡V7FVB“°¢ÆWB7F÷&W2ÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ—FW"‚¢æf–ÇFW"‡Â…òÂ÷6öFR—Â¦÷6öFRÓÒƒFR¢æ6÷VçB‚“°¢ÆWBW‡V7FVE÷7F÷&W2Ò–bÖWF†öBÓÒ&–æ—F–Æ—¦R"²ÒVÇ6R²"Ó°¢76W'EöW€¢7F÷&W2ÂW‡V7FVE÷7F÷&W2À¢'¶ÖWF†öGÒ7F÷&W27W'&VçB–â7F÷&Uó2 ¢“°¢76W'EöW€¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ—FW"‚¢æf–ÇFW"‡Â…òÂ÷6öFR—Â¦÷6öFRÓÒƒ&B¢æ6÷VçB‚’À¢À¢'¶ÖWF†öGÒ&VG27W'&VçBg&öÒÆöEó2f÷"F†Rf—'GVÂ6ÆÂ ¢“°¢76W'B€¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ—FW"‚¢æÆÂ‡Â…òÂ÷6öFR—Â¦÷6öFRÒ†3’À¢'¶ÖWF†öGÒFöW2æ÷B–ç6W'Bæ'&÷v–ær6†V6¶67B ¢“°¢Ğ¢76W'EöW†6öFUöÖ…öÆö6Ç2‚g'VææW%ö6Æ72Â&6†ö÷6R"’ÂB“°¢76W'EöW†6öFUöÖ…öÆö6Ç2‚g'VææW%ö6Æ72Â'&WVB"’ÂR“°¢76W'EöW†6öFUöÖ…öÆö6Ç2‚g'VææW%ö6Æ72Â&–æ—F–Æ—¦R"’ÂB“°¢76W'EöW†6öFUöÖ…öÆö6Ç2‚g'VææW%ö6Æ72Â'7v"’ÂB“°¢f÷"ÖWF†öB–â²&6†ö÷6R"Â'&WVB"Â&–æ—F–Æ—¦R"Â'7v%Ò°¢76W'EöW€¢6öFUöÖ…÷7F6²‚g'VææW%ö6Æ72ÂÖWF†öB’À¢2À¢'¶ÖWF†öGÒÖ…÷7F6² ¢“°¢Ğ¢76W'E÷&VfW&Væ6UöÆö6Åö¦ö–åög&ÖR‚g'VææW%ö6Æ72Â&6†ö÷6R"Â2ÂfÇ6R“°¢76W'E÷&VfW&Væ6UöÆö6Åö¦ö–åög&ÖR‚g'VææW%ö6Æ72Â'&WVB"Â2ÂG'VR“°¢76W'E÷&VfW&Væ6U÷7F6µö¦ö–åög&ÖR‚g'VææW%ö6Æ72Â&–æ—F–Æ—¦R"“°¢76W'E÷&VfW&Væ6U÷7F6µö¦ö–åög&ÖR‚g'VææW%ö6Æ72Â'7v"“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚$×WF&ÆU&VfW&Væ6T6æF–FFT†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢"2'V&Æ–2f–æÂ6Æ72×WF&ÆU&VfW&Væ6T6æF–FFT†&æW72°¢V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’°¢'VææW"'VææW"ÒæWr'VææW"‚“°¢f÷"†–çB6VVB¢æWr–çEµÒ³ÂÂÓ7Ò’°¢–çBÆVgBÒ6VVB²°¢–çB&–v‡BÒ6VVB²#°¢–b‡'VææW"æ6†ö÷6R†fÇ6RÂ6VVB’ÒÆVgBÇÂ'VææW"æ6†ö÷6R‡G'VRÂ6VVB’Ò&–v‡B¢F‡&÷ræWr76W'F–öäW'&÷"‚&'&æ6‚76–væÖVçBF—7F6ƒ¢"²6VVB“°¢–b‡'VææW"ç&WVBƒÂ6VVB’ÒÆVgBÇÂ'VææW"ç&WVBƒÂ6VVB’Ò&–v‡BÇÂ'VææW"ç&WVBƒ2Â6VVB’Ò&–v‡B¢F‡&÷ræWr76W'F–öäW'&÷"‚&Æö÷76–væÖVçBF—7F6ƒ¢"²6VVB“°¢–b‡'VææW"æ–æ—F–Æ—¦R‡G'VRÂ6VVB’ÒÆVgBÇÂ'VææW"æ–æ—F–Æ—¦R†fÇ6RÂ6VVB’Ò&–v‡B¢F‡&÷ræWr76W'F–öäW'&÷"‚&–æ—F–Æ—¦W"¦ö–âF—7F6ƒ¢"²6VVB“°¢–b‡'VææW"ç7v†fÇ6RÂ6VVB’ÒÆVgBÇÂ'VææW"ç7v‡G'VRÂ6VVB’Ò&–v‡B¢F‡&÷ræWr76W'F–öäW'&÷"‚&76–væÖVçB6–&Æ–ær¦ö–âF—7F6ƒ¢"²6VVB“°¢Ğ¢Ğ§Ğ¢"2À¢¢æW‡V7B‚'w&—FR×WF&ÆR×&VfW&Væ6R&VF&6²†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWE÷F‚Â"ÖB"Â÷WGWE÷F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVB×WF&ÆR×&VfW&Væ6R&VF&6²†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢$×WF&ÆU&VfW&Væ6T6æF–FFT†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W""“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVB×WF&ÆR×&VfW&Væ6R&VF&6²6Æ76W5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“° ¢f÷"6Æ75öæÖR–â²$&6R"Â$ÆVgB"Â%&–v‡B"Â%'VææW"%Ò°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…²"×"Â"Ö2"Â"×b"Â"Ö6Æ77F‚"Â÷WGWE÷F‚Â6Æ75öæÖUÒ¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ff÷"66WFæ6R6Æ72"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÖ2×b&V¦V7FVB¶6Æ75öæÖWÕÆç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢ÆWBFW‡BÒ7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB“°¢76W'B‡FW‡Bæ6öçF–ç2‚&Ö¦÷"fW'6–öã¢cR"’“°¢–b6Æ75öæÖRÓÒ%'VææW""°¢76W'B‡FW‡Bæ6öçF–ç2‚%7F6´ÖF&ÆR"’“°¢76W'B‡FW‡Bæ6öçF–ç2‚&–çfö¶Wf—'GVÂ"’“°¢76W'B‡FW‡Bæ6öçF–ç2‚$ÖWF†öB&6RçfÇVS¢‚”’"’“°¢Ğ¢–b6Æ75öæÖRÓÒ$&6R"°¢76W'B‡FW‡Bæ6öçF–ç2‚&7W'&VçC²"’“°¢76W'B‡FW‡Bæ6öçF–ç2‚'WFf–VÆB"’“°¢76W'B‡FW‡Bæ6öçF–ç2‚&vWFf–VÆB"’“°¢Ğ¢Ğ¢Ğ§Ğ ¦fâvÆµ÷G—VE÷7V'G&VR€¢Væ—C¢d§fÔ6ö×–ÆF–öåVæ—CÂuóâÀ¢&ö÷C¢G&VT–CÅG—VCâÀ¢f—6—C¢f×WB–×Âfä×WB…G&VT–CÅG—VCâ’À¢’°¢ÆWB×WBVæF–ærÒfV2·&ö÷EÓ°¢v†–ÆRÆWB6öÖR‡G&VUö–B’ÒVæF–ærç÷‚’°¢f—6—B‡G&VUö–B“°¢ÖF6‚gVæ—BçG—VEö7BævWB‡G&VUö–B’æ¶–æB°¢G&VT¶–æC£¤Ç’†Æ–6F–öâ’Óâ°¢VæF–ærçW6‚†Æ–6F–öâægVæ7F–öâ“°¢VæF–æræW‡FVæB†Æ–6F–öâæ&w2æ—FW"‚’æ6÷–VB‚’“°¢Ğ¢G&VT¶–æC£¤76–vâ†76–væÖVçB’ÓâVæF–æræW‡FVæB…¶76–væÖVçBæÆ‡2Â76–væÖVçBç&‡5Ò’À¢G&VT¶–æC£¤&Æö6²†&Æö6²’Óâ°¢VæF–æræW‡FVæB†&Æö6²ç7FG2æ—FW"‚’æ6÷–VB‚’“°¢VæF–ærçW6‚†&Æö6²æW‡"“°¢Ğ¢G&VT¶–æC£¤–b†–e÷G&VR’Óâ°¢VæF–æræW‡FVæB…¶–e÷G&VRæ6öæBÂ–e÷G&VRçF†Våö'&æ6‚Â–e÷G&VRæVÇ6Uö'&æ6…Ò“°¢Ğ¢G&VT¶–æC£¤æWr†æWu÷G&VR’ÓâVæF–ærçW6‚†æWu÷G&VRçGB’À¢G&VT¶–æC£¥6VÆV7B‡6VÆV7B’ÓâVæF–ærçW6‚‡6VÆV7BçVÆ–f–W"’À¢G&VT¶–æC£¥G—VB‡G—VB’ÓâVæF–æræW‡FVæB…·G—VBæW‡"ÂG—VBçGEÒ’À¢G&VT¶–æC£¥fÄFVb†FVf–æ—F–öâ’Óâ°¢VæF–ærçW6‚†FVf–æ—F–öâçGB“°¢VæF–æræW‡FVæB†FVf–æ—F–öâç&‡2“°¢Ğ¢G&VT¶–æC£¥v†–ÆR†Æö÷÷G&VR’ÓâVæF–æræW‡FVæB…¶Æö÷÷G&VRæ6öæBÂÆö÷÷G&VRæ&öG•Ò’À¢òÓâ·Ğ¢Ğ¢Ğ§Ğ ¢5·FW7EĞ¦fâ×WF&ÆU÷&VfW&Væ6UöÆö6ÅöVF—E÷&W6W'fW5÷6÷W&6UöWf–FVæ6UögFW%öÆ–÷WEö66WFæ6R‚’°¢ÆWB6÷W&6RÒg3£§&VE÷Fõ÷7G&–ær†f—‡GW&U÷F‚‚$×WF&ÆU&VfW&Væ6TÆö6Ç5G—VBç66Æ"’¢æW‡V7B‚'&VBF†R6÷W&6RÖ&6¶VB×WF&ÆR×&VfW&Væ6RÖÆö6ÂVF—Bf—‡GW&R"“°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&×WF&ÆR×&VfW&Væ6RÖÆö6ÂÖVF—B"“°¢ÆWB×WBö'6W'fVBÒfÇ6S°¢ÆWB×WBö'6W'fVEöÖWF†öG2Ò°¢ÆWB&W7VÇBÒ6ö×–ÆU÷6÷W&6Uö&F6…öö'6W'fVB€¢g6÷W&6RÀ¢$×WF&ÆU&VfW&Væ6TÆö6Ç5G—VBç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒCSB’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢6öÖR‚f×WBÇVæ—BÂFVf–æ—F–öç7Â°¢ö'6W'fVBÒG'VS°¢ÆWB&6RÒVF—Eö6Æ75öæÖVB‡Væ—BÂ$&6R"“°¢ÆWBÆVgBÒVF—Eö6Æ75öæÖVB‡Væ—BÂ$ÆVgB"“°¢ÆWB&–v‡BÒVF—Eö6Æ75öæÖVB‡Væ—BÂ%&–v‡B"“°¢ÆWB'VææW"ÒVF—Eö6Æ75öæÖVB‡Væ—BÂ%'VææW""“°¢76W'EöW†VF—Eö6Æ75÷&VçB‡Væ—BÂÆVgB’Â&6R“°¢76W'EöW†VF—Eö6Æ75÷&VçB‡Væ—BÂ&–v‡B’Â&6R“°¢76W'EöW†VF—Eö6Æ75÷&VçB‡Væ—BÂ&6R’ÂFVf–æ—F–öç2æö&¦V7Eö6Æ72“° ¢f÷"ÖWF†öEöæÖR–â²&6†ö÷6R"Â'&WVB"Â&–æ—F–Æ—¦R%Ò°¢ö'6W'fVEöÖWF†öG2³Ò°¢ÆWBÖWF†öBÒVF—EöÖWF†öEöæÖVB‡Væ—BÂ'VææW"ÂÖWF†öEöæÖR“°¢ÆWBÆ–÷WBÒVæ—@¢æÆö6ÅöÆ–÷WB‡'VææW"ÂÖWF†öBæÖWF†öBÂFVf–æ—F–öç2¢æW‡V7B‚'F†R6æöæ–6Â×WF&ÆR&6RÆö6Â†2fÆ–B6Æ÷BÆ–÷WB"“°¢ÆWB7W'&VçBÒÖWF†ö@¢æÆö6Ç0¢æ—FW"‚¢æf–æB‡Æ&–æF–æwÂ°¢&–æF–æræ×WF&ÆP¢bbVæ—@¢ç7F÷&P¢ææÖW0¢ç&W6öÇfR‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB†&–æF–ærç7–Ö&öÂ’ææÖRçFW‡B‚’¢ÓÒ&7W'&VçB ¢Ò¢æW‡V7B‚&V6‚&VÂ6÷W&6Rf"†2öæR6æöæ–6Â×WF&ÆRÆö6Â&–æF–ær"“°¢ÆWB7–Ö&öÂÒVæ—Bç7F÷&Rç7–Ö&öÇ2ævWB†7W'&VçBç7–Ö&öÂ“°¢76W'EöW‡7–Ö&öÂæ÷væW"Â6öÖR†ÖWF†öBæÖWF†öB’“°¢76W'B‡7–Ö&öÂæfÆw2æ6öçF–ç2…7–Ö&öÄfÆw3£¤ÕUD$ÄR’“°¢76W'EöW‡7–Ö&öÂæ¶–æBÂ7–Ö&öÄ¶–æC£¤Æö6Â“°¢ÆWB6Æ÷BÒÆ–÷W@¢æÆö6Â†7W'&VçBç7–Ö&öÂ¢æW‡V7B‚'F†RÆ–÷WB—2¶W–VB'’F†R6ÖR6æöæ–6ÂÆö6Â7–Ö&öÂ"“°¢76W'EöW‡6Æ÷BæFV6Æ&F–öâ‚’Â7W'&VçBæFV6Æ&F–öâ“°¢76W'EöW‡6Æ÷BçG’‚’Â§fÕG—S£¥&VfW&Væ6R†&6R’“°¢76W'B‡6Æ÷Bæ—5ö×WF&ÆR‚’“°¢ÆWBF÷GG•ö6÷&S£¥7–Ö&öÄ–æfó£¤6ö×ÆWFR†FV6Æ&VE÷G—R’Òg7–Ö&öÂæ–æfòVÇ6R°¢æ–2‚&7W'&VçB†26ö×ÆWFR6æöæ–6Â6÷W&6RG—R–æf÷&ÖF–öâ"¢Ó°¢76W'EöW€¢VF—E÷G—U÷7–Ö&öÂ‡Væ—BÂ¦FV6Æ&VE÷G—R’À¢6öÖR†&6R’À¢'F†RFV6Æ&VBÆö6ÂG—R&VÖ–ç2&6R ¢“°¢ÆWBFV6Æ&F–öâÒVæ—BçG—VEö7BævWB†7W'&VçBæFV6Æ&F–öâ“°¢ÆWBG&VT¶–æC£¥fÄFVb†FVf–æ—F–öâ’ÒfFV6Æ&F–öâæ¶–æBVÇ6R°¢æ–2‚'F†R&–æF–ærö–çG2B—G26æöæ–6ÂG—VBfÄFVb"¢Ó°¢ÆWB–æ—F–Æ—¦W"ÒFVf–æ—F–öâç&‡2æW‡V7B‚&7W'&VçB†26÷W&6R–æ—F–Æ—¦W""“°¢76W'EöW€¢VF—E÷G—U÷7–Ö&öÂ‡Væ—BÂVæ—BçG—VEö7BævWB†FVf–æ—F–öâçGB’çG’’À¢6öÖR†&6R¢“°¢ÆWB&öG’ÒÖWF†öBæ&öG’æW‡V7B‚&V6‚6÷W&6RÖWF†öB†2G—VB&öG’"“°¢ÆWB×WBæöFW2ÒfV3£¦æWr‚“°¢vÆµ÷G—VE÷7V'G&VR‡Væ—BÂ&öG’Âf×WBÇG&VWÂæöFW2çW6‚‡G&VR’“°¢ÆWB76–væÖVçG5÷Fõö7W'&VçBÒæöFW0¢æ—FW"‚¢æf–ÇFW%öÖ‡ÇG&VUö–GÂ°¢ÆWBG&VT¶–æC£¤76–vâ†76–væÖVçB’ÒgVæ—BçG—VEö7BævWB‚§G&VUö–B’æ¶–æ@¢VÇ6R°¢&WGW&âæöæS°¢Ó°¢‡FW&Õ÷G—U÷F&vWB‡Væ—BÂVæ—BçG—VEö7BævWB†76–væÖVçBæÆ‡2’çG’¢ÓÒ6öÖR†7W'&VçBç7–Ö&öÂ’¢çF†Vå÷6öÖR‚‚§G&VUö–BÂ76–væÖVçBç&‡2’¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“° ¢ÖF6‚ÖWF†öEöæÖR°¢&6†ö÷6R"Óâ°¢76W'EöW€¢VF—E÷G—U÷7–Ö&öÂ‡Væ—BÂVæ—BçG—VEö7BævWB†–æ—F–Æ—¦W"’çG’’À¢6öÖR†ÆVgB¢“°¢ÆWB'&æ6‚ÒæöFW0¢æ—FW"‚¢æf–æEöÖ‡ÇG&VUö–GÂ°¢ÆWBG&VRÒVæ—BçG—VEö7BævWB‚§G&VUö–B“°¢ÖF6†W2‡G&VRæ¶–æBÂG&VT¶–æC£¤–b…ò’’çF†Vå÷6öÖR‡G&VR¢Ò¢æW‡V7B‚&6öæF—F–öæÂ&V76–væÖVçB&VÖ–ç2G—VB–b"“°¢ÆWBG&VT¶–æC£¤–b†–e÷G&VR’Òf'&æ6‚æ¶–æBVÇ6R°¢Vç&V6†&ÆR‚¢Ó°¢ÆWB×WBF†VåöæöFW2ÒfV3£¦æWr‚“°¢vÆµ÷G—VE÷7V'G&VR‡Væ—BÂ–e÷G&VRçF†Våö'&æ6‚Âf×WBÇG&VWÂ°¢F†VåöæöFW2çW6‚‡G&VR¢Ò“°¢ÆWBF†Våö76–væÖVçG2ÒF†VåöæöFW0¢æ—FW"‚¢æf–ÇFW%öÖ‡ÇG&VUö–GÂ°¢ÆWBG&VT¶–æC£¤76–vâ†76–væÖVçB’Ğ¢gVæ—BçG—VEö7BævWB‚§G&VUö–B’æ¶–æ@¢VÇ6R°¢&WGW&âæöæS°¢Ó°¢‡FW&Õ÷G—U÷F&vWB‡Væ—BÂVæ—BçG—VEö7BævWB†76–væÖVçBæÆ‡2’çG’¢ÓÒ6öÖR†7W'&VçBç7–Ö&öÂ’¢çF†Vå÷6öÖR†76–væÖVçBç&‡2¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW‡F†Våö76–væÖVçG2æÆVâ‚’Â“°¢76W'EöW€¢VF—E÷G—U÷7–Ö&öÂ‡Væ—BÂVæ—BçG—VEö7BævWB‡F†Våö76–væÖVçG5³Ò’çG’’À¢6öÖR‡&–v‡B’À¢'F†R76–væÖVçB–âF†R–bF†VâÖ'&æ6‚7F÷&W2&–v‡B ¢“°¢76W'EöW†76–væÖVçG5÷Fõö7W'&VçBæÆVâ‚’Â“°¢Ğ¢'&WVB"Óâ°¢76W'EöW€¢VF—E÷G—U÷7–Ö&öÂ‡Væ—BÂVæ—BçG—VEö7BævWB†–æ—F–Æ—¦W"’çG’’À¢6öÖR†ÆVgB¢“°¢ÆWBÆö÷÷G&VRÒæöFW0¢æ—FW"‚¢æf–æEöÖ‡ÇG&VUö–GÂ°¢ÆWBG&VRÒVæ—BçG—VEö7BævWB‚§G&VUö–B“°¢ÖF6†W2‡G&VRæ¶–æBÂG&VT¶–æC£¥v†–ÆR…ò’’çF†Vå÷6öÖR‡G&VR¢Ò¢æW‡V7B‚&Æö÷&V76–væÖVçB&VÖ–ç2G—VBv†–ÆR"“°¢ÆWBG&VT¶–æC£¥v†–ÆR†Æö÷ö&öG’’ÒfÆö÷÷G&VRæ¶–æBVÇ6R°¢Vç&V6†&ÆR‚¢Ó°¢ÆWB×WBÆö÷ö&öG•öæöFW2ÒfV3£¦æWr‚“°¢vÆµ÷G—VE÷7V'G&VR‡Væ—BÂÆö÷ö&öG’æ&öG’Âf×WBÇG&VWÂ°¢Æö÷ö&öG•öæöFW2çW6‚‡G&VR¢Ò“°¢ÆWBÆö÷ö76–væÖVçG2ÒÆö÷ö&öG•öæöFW0¢æ—FW"‚¢æf–ÇFW%öÖ‡ÇG&VUö–GÂ°¢ÆWBG&VT¶–æC£¤76–vâ†76–væÖVçB’Ğ¢gVæ—BçG—VEö7BævWB‚§G&VUö–B’æ¶–æ@¢VÇ6R°¢&WGW&âæöæS°¢Ó°¢‡FW&Õ÷G—U÷F&vWB‡Væ—BÂVæ—BçG—VEö7BævWB†76–væÖVçBæÆ‡2’çG’¢ÓÒ6öÖR†7W'&VçBç7–Ö&öÂ’¢çF†Vå÷6öÖR†76–væÖVçBç&‡2¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW†Æö÷ö76–væÖVçG2æÆVâ‚’Â“°¢76W'EöW€¢VF—E÷G—U÷7–Ö&öÂ‡Væ—BÂVæ—BçG—VEö7BævWB†Æö÷ö76–væÖVçG5³Ò’çG’’À¢6öÖR‡&–v‡B’À¢'F†Rv†–ÆR&öG’7F÷&W2&–v‡B–çFò7W'&VçB ¢“°¢76W'EöW†76–væÖVçG5÷Fõö7W'&VçBæÆVâ‚’Â“°¢Ğ¢&–æ—F–Æ—¦R"Óâ°¢ÆWBG—S£¤÷"°¢ÆVgC¢ÆVgEö&ÒÀ¢&–v‡C¢&–v‡Eö&ÒÀ¢ÒÒVæ—Bç7F÷&RçG—W2ævWB‡Væ—BçG—VEö7BævWB†–æ—F–Æ—¦W"’çG’¢VÇ6R°¢æ–2‚'F†R6öçFW‡GVÂ–æ—F–Æ—¦W"&WF–ç2F†R6÷W&6R6–&Æ–ærVæ–öâ"¢Ó°¢ÆWB×WB&×2Ò°¢VF—E÷G—U÷7–Ö&öÂ‡Væ—BÂ¦ÆVgEö&Ò¢æW‡V7B‚&ÆVgBVæ–öâ&Ò—26æöæ–6Â"’À¢VF—E÷G—U÷7–Ö&öÂ‡Væ—BÂ§&–v‡Eö&Ò¢æW‡V7B‚'&–v‡BVæ–öâ&Ò—26æöæ–6Â"’À¢Ó°¢&×2ç6÷'Eö'•ö¶W’‡Æ&×Â&Òæ–æFW‚‚’“°¢ÆWB×WBW‡V7FVBÒ¶ÆVgBÂ&–v‡EÓ°¢W‡V7FVBç6÷'Eö'•ö¶W’‡Æ&×Â&Òæ–æFW‚‚’“°¢76W'EöW†&×2ÂW‡V7FVB“°¢76W'B†76–væÖVçG5÷Fõö7W'&VçBæ—5öV×G’‚’“°¢Ğ¢òÓâVç&V6†&ÆR‚’À¢Ğ¢Ğ¢76W'EöW†ö'6W'fVEöÖWF†öG2Â2“°¢Ò’À¢“°¢76W'B€¢ö'6W'fVBÀ¢'F†R6ö×ÆWFR6÷W&6R&V6†VBF†RG—VBVF—Bö'6W'fW#¢·&W7VÇC£÷Ò ¢“°¢ÆWB'F–f7G2Ò&W7VÇBæW‡V7B‚&'&æ6‚æBÆö÷76–væÖVçG2Æ÷vW"gFW"F†R6÷W&6RVF—B"“°¢76W'EöW€¢'F–f7G0¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢²$&6R"Â$ÆVgB"Â%&–v‡B"Â%'VææW"%Ğ¢“°¢76W'EöW€¢÷WGWBæ6Æ75öf–ÆW2‚’æÆVâ‚’À¢BÀ¢'F†R6ö×ÆWFVB6÷W&6R&F6‚V&Æ—6†W2ÆÂf÷W"ÆææVB6Æ76W2 ¢“°§Ğ ¢5·FW7EĞ¦fâ×WF&ÆU÷&VfW&Væ6UöÆ–÷WE÷&W6W'fW5ö6æöæ–6Å÷G—W5öæE÷6†F÷vVE÷7–Ö&öÇ2‚’°¢ÆWB6÷W&6RÒg3£§&VE÷Fõ÷7G&–ær†f—‡GW&U÷F‚‚$×WF&ÆU&VfW&Væ6TÆö6Ç5G—VBç66Æ"’¢æW‡V7B‚'&VBF†RÆ—FW&Â×&W7VÇBFFVBVF—Bf—‡GW&R"“°¢f÷"ÖWF†öEöæÖR–â²&Æ–÷WB"Â'6†F÷vVB%Ò°¢ÆWBÖWF†öE÷6÷W&6RÒ6÷W&6U÷v—F…ööæÇ•÷'VææW%öÖWF†öB‚g6÷W&6RÂÖWF†öEöæÖR“°¢ÆWB×WB6W76–öåöÆ–÷WG2ÒfV3£¦æWr‚“°¢f÷"6÷W&6Uö–æFW‚–â³CSbÂCSuÒ°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚ff÷&ÖB€¢&×WF&ÆR×&VfW&Væ6RÖÆ–÷WB×¶ÖWF†öEöæÖWÒ×·6÷W&6Uö–æFW‡Ò ¢’“°¢ÆWB×WBö'6W'fVEöÆ–÷WBÒæöæS°¢ÆWB&W7VÇBÒ6ö×–ÆU÷6÷W&6Uö&F6…öö'6W'fVB€¢fÖWF†öE÷6÷W&6RÀ¢$×WF&ÆU&VfW&Væ6TÆö6Ç5G—VBç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚‡6÷W&6Uö–æFW‚’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢6öÖR‚f×WBÇVæ—BÂFVf–æ—F–öç7Â°¢ÆWB'VææW"ÒVF—Eö6Æ75öæÖVB‡Væ—BÂ%'VææW""“°¢ÆWBÖWF†öBÒVF—EöÖWF†öEöæÖVB‡Væ—BÂ'VææW"ÂÖWF†öEöæÖR“°¢ÆWB&6RÒVF—Eö6Æ75öæÖVB‡Væ—BÂ$&6R"“°¢ÆWBÆ–÷WBÒVæ—@¢æÆö6ÅöÆ–÷WB‡'VææW"ÂÖWF†öBæÖWF†öBÂFVf–æ—F–öç2¢æW‡V7B‚&6æöæ–6Â×WF&ÆR&VfW&Væ6RÆö6Ç2†fRÆææVB6Æ÷BÆ–÷WB"“°¢ÆWBÆö6Ç2ÒÖWF†ö@¢æÆö6Ç0¢æ—FW"‚¢æÖ‡Æ&–æF–æwÂ°¢ÆWB7–Ö&öÂÒVæ—Bç7F÷&Rç7–Ö&öÇ2ævWB†&–æF–ærç7–Ö&öÂ“°¢76W'EöW‡7–Ö&öÂæ¶–æBÂ7–Ö&öÄ¶–æC£¤Æö6Â“°¢76W'EöW‡7–Ö&öÂæ÷væW"Â6öÖR†ÖWF†öBæÖWF†öB’“°¢76W'EöW€¢7–Ö&öÂæfÆw2æ6öçF–ç2…7–Ö&öÄfÆw3£¤ÕUD$ÄR’À¢&–æF–æræ×WF&ÆP¢“°¢ÆWBæÖRÒVæ—Bç7F÷&RææÖW2ç&W6öÇfR‡7–Ö&öÂææÖRçFW‡B‚’’çFõö÷væVB‚“°¢76W'EöW†æÖRÂ&7W'&VçB"“°¢ÆWBF÷GG•ö6÷&S£§7–Ö&öÇ3£¥7–Ö&öÄ–æfó£¤6ö×ÆWFR†FV6Æ&VE÷G—R’Ğ¢Væ—Bç7F÷&Rç7–Ö&öÇ2æ–æfò†&–æF–ærç7–Ö&öÂ¢VÇ6R°¢æ–2‚'F†RÆö6Â¶VW2—G26ö×ÆWFVBFV6Æ&VBG—R"¢Ó°¢76W'EöW†VF—E÷G—U÷7–Ö&öÂ‡Væ—BÂ¦FV6Æ&VE÷G—R’Â6öÖR†&6R’“°¢ÆWB6Æ÷BÒÆ–÷W@¢æÆö6Â†&–æF–ærç7–Ö&öÂ¢æW‡V7B‚&Æ–÷WBÆöö·WW6W2F†R6æöæ–6ÂÆö6Â7–Ö&öÂ"“°¢76W'EöW‡6Æ÷BæFV6Æ&F–öâ‚’Â&–æF–æræFV6Æ&F–öâ“°¢76W'EöW‡6Æ÷Bæ—5ö×WF&ÆR‚’Â&–æF–æræ×WF&ÆR“°¢ÆWB§fÕG—S£¥&VfW&Væ6R‡6Æ÷Eö6Æ72’Ò6Æ÷BçG’‚’VÇ6R°¢æ–2‚'F†RÆö6Â6Æ÷B&WF–ç2—G2FV6Æ&VB&VfW&Væ6RG—R"¢Ó°¢76W'EöW‡6Æ÷Eö6Æ72Â&6R“°¢†æÖRÂ6Æ÷Bç6Æ÷B‚’Â6Æ÷Bæ—5ö×WF&ÆR‚’Â$&6R"çFõö÷væVB‚’¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢ÖF6‚ÖWF†öEöæÖR°¢&Æ–÷WB"Óâ°¢76W'EöW†Æö6Ç2æÆVâ‚’Â“°¢76W'EöW†Æö6Ç5³ÒãÂ"Â'&V6V—fW"æB6VVBö67W’6Æ÷G2æB"“°¢76W'EöW†Æ–÷WBæÖ…öÆö6Ç2‚’Â2“°¢76W'B†Æö6Ç5³Òã"“°¢Ğ¢'6†F÷vVB"Óâ°¢76W'EöW†Æö6Ç2æÆVâ‚’Â"“°¢76W'EöæR†ÖWF†öBæÆö6Ç5³Òç7–Ö&öÂÂÖWF†öBæÆö6Ç5³Òç7–Ö&öÂ“°¢76W'EöW†Æö6Ç5³ÒãÂ2“°¢76W'EöW†Æö6Ç5³ÒãÂB“°¢76W'EöW†Æ–÷WBæÖ…öÆö6Ç2‚’ÂR“°¢76W'EöW€¢Æö6Ç2æ—FW"‚’æÖ‡ÆÆö6ÇÂÆö6Âã"’æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢·G'VRÂfÇ6UĞ¢“°¢Ğ¢òÓâVç&V6†&ÆR‚’À¢Ğ¢ö'6W'fVEöÆ–÷WBÒ6öÖR‚†Æ–÷WBæÖ…öÆö6Ç2‚’ÂÆö6Ç2’“°¢Ò’À¢“°¢76W'B€¢&W7VÇBæ—5öö²‚’À¢&–æ—F–Æ—¦–ærÆææVB×WF&ÆRÆö6Ç2v—F†÷WB&VF–ær÷"76–væ–ærF†VÒ—27W÷'FVC¢·&W7VÇC£÷Ò ¢“°¢76W'B‚÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°¢6W76–öåöÆ–÷WG0¢çW6‚†ö'6W'fVEöÆ–÷WBæW‡V7B‚'F†R6÷W&6RÖ&6¶VBG—VBVæ—Bv2ö'6W'fVB"’“°¢Ğ¢76W'EöW‡6W76–öåöÆ–÷WG5³ÒÂ6W76–öåöÆ–÷WG5³Ò“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ6öç7G'V7F÷%ö&wVÖVçG5öÆ÷vW%÷7G&–7EöæE÷Væ'•ö&ööÆVåö–çG&–ç6–75ög&öÕ÷6÷W&6R‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&&ööÆVâÖ6öç7G'V7F÷"Ö&wVÖVçG2"“°¢ÆWB'F–f7G2Ò6ö×–ÆU÷ÆææVEö6öç7G'V7F÷%÷6÷W&6R€¢&6Æ726÷VçFW"‡6VVC¢&ööÆVâ•Ææ6Æ726Æ–VçC¥ÆâFVb'V–ÆDæB†fÆs¢&ööÆVâ“¢–çBÕÆâæWr6÷VçFW"†fÆrbG'VR•ÆâC%ÆâFVb'V–ÆD÷"†fÆs¢&ööÆVâ“¢–çBÕÆâæWr6÷VçFW"†fÆrÂG'VR•ÆâC%ÆâFVb'V–ÆE†÷"†fÆs¢&ööÆVâ“¢–çBÕÆâæWr6÷VçFW"†fÆrâG'VR•ÆâC%ÆâFVb'V–ÆDæ÷B†fÆs¢&ööÆVâ“¢–çBÕÆâæWr6÷VçFW"‚fÆr•ÆâC%Æâ"À¢÷WGWBçF‚‚’À¢¢çVçw&‚“°¢ÆWB6Æ–VçBÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$6Æ–VçB"¢æW‡V7B‚$6Æ–VçB6÷W&6R6Æ72—2VÖ—GFVB"“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†6Æ–VçBæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFR6Æ–VçB6Æ72"“°¢6Æ70¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚'fÆ–B6Æ–VçB&VfW&Væ6W2"“° ¢f÷"†ÖWF†öBÂ÷W&F÷"’–â²‚&'V–ÆDæB"ÂƒvR’Â‚&'V–ÆD÷""Âƒƒ’Â‚&'V–ÆE†÷""Âƒƒ"•Ò°¢ÆWB6öFRÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ72ÂÖWF†öB“°¢76W'B€¢6öFRçv–æF÷w2ƒ2¢æç’‡Ç6WVVæ6WÂ6WVVæ6RÓÒ³ƒ"ÂƒBÂ÷W&F÷%Ò’À¢'¶ÖWF†öGÒ6†÷VÆBÆ÷vW"—G27G&–7B&ööÆVâ÷W&F÷"&Vf÷&R6öç7G'V7F–öã¢¶6öFS£÷Ò ¢“°¢76W'B€¢6öFRæ6öçF–ç2‚c†#r’À¢'¶ÖWF†öGÒ6†÷VÆB–çfö¶RF†R6öç7G'V7F÷" ¢“°¢Ğ¢ÆWBæ÷Eö6öFRÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â&'V–ÆDæ÷B"“°¢76W'B€¢æ÷Eö6öFP¢çv–æF÷w2ƒ2¢æç’‡Ç6WVVæ6WÂ6WVVæ6RÓÒ³ƒ"ÂƒBÂƒƒ%Ò’À¢&'V–ÆDæ÷B6†÷VÆBÆ÷vW"6æöæ–6Â2–6öç7Eó²—†÷#¢¶æ÷Eö6öFS£÷Ò ¢“°¢76W'B€¢æ÷Eö6öFRæ6öçF–ç2‚c†#r’À¢&'V–ÆDæ÷B6†÷VÆB–çfö¶RF†R6öç7G'V7F÷" ¢“°§Ğ ¢5·FW7EĞ¦fâf–VÆEö–æ—F–Æ—¦W%öVÖ—G5öö6ö×ÆWFU÷fW&–f–VEö6Æ72‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&f–VÆBÖ–æ—F–Æ—¦W""“°¢ÆWB'F–f7BÒ6ö×–ÆUöf—‡GW&R‚$f–VÆD–æ—F–Æ—¦W"ç66Æ"Â÷WGWBçF‚‚’’çVçw&‚“°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†'F–f7Bæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ75öf–ÆRÒ6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’çVçw&‚“°¢6Æ75öf–ÆRçfÆ–FFU÷&VfW&Væ6W2‚’çVçw&‚“°¢76W'EöW†6Æ75öf–ÆRæf–VÆG2æÆVâ‚’Â“°¢76W'EöW†6Æ75öf–ÆRæÖWF†öG2æÆVâ‚’Â"“°¢76W'EöW€¢6Æ75öf–ÆP¢æ6öç7FçE÷ööÀ¢çWFc‚†6Æ75öf–ÆRæÖWF†öG5³ÒææÖUö–æFW‚¢çVçw&‚’À¢#Æ–æ—Câ ¢“°¢76W'EöW€¢6Æ75öf–ÆP¢æ6öç7FçE÷ööÀ¢çWFc‚†6Æ75öf–ÆRæÖWF†öG5³ÒæFW67&—F÷%ö–æFW‚¢çVçw&‚’À¢"‚•b ¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢&WGW&ã°¢Ğ¢ÆWB†&æW75÷F‚Ò÷WGWBçF‚‚’æ¦ö–â‚$f–VÆD–æ—F–Æ—¦W$†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW75÷F‚À¢'V&Æ–2f–æÂ6Æ72f–VÆD–æ—F–Æ—¦W$†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²f–VÆD–æ—F–Æ—¦W"–ç7Fæ6RÒæWrf–VÆD–æ—F–Æ—¦W"‚“²–b†–ç7Fæ6RçfÇVR‚’Ò’F‡&÷ræWr76W'F–öäW'&÷"…Â'fÇVUÂ"“²ÒÕÆâ"À¢¢çVçw&‚“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWBçF‚‚’çFõ÷7G"‚’çVçw&‚’Â"ÖB%Ò¢æ&r†÷WGWBçF‚‚’¢æ&r‚f†&æW75÷F‚¢æ÷WGWB‚¢çVçw&‚“°¢76W'B†¦f2ç7FGW2ç7V66W72‚’Â&¦f2f–ÆVC¢³£÷Ò"Â¦f2“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWBçF‚‚’çFõ÷7G"‚’çVçw&‚’À¢$f–VÆD–æ—F–Æ—¦W$†&æW72"À¢Ò¢æ÷WGWB‚¢çVçw&‚“°¢76W'B†¦fç7FGW2ç7V66W72‚’Â&¦ff–ÆVC¢³£÷Ò"Â¦f“°§Ğ ¢5·FW7EĞ¦fâ&VÅö6÷VçFW%÷6÷W&6Uö6ö×–ÆW5öFWFW&Ö–æ—7F–6ÆÇ•öæEöW†V7WFW5÷7FFVgVÅöÖWF†öG2‚’°¢ÆWBf—'7Eö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&6÷VçFW"Öf—'7B"“°¢ÆWB6V6öæEö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&6÷VçFW"×6V6öæB"“°¢ÆWBf—'7BÒ6ö×–ÆUöf—‡GW&R‚$6÷VçFW"ç66Æ"Âf—'7Eö÷WGWBçF‚‚’’çVçw&‚“°¢ÆWB6V6öæBÒ6ö×–ÆUöf—‡GW&R‚$6÷VçFW"ç66Æ"Â6V6öæEö÷WGWBçF‚‚’’çVçw&‚“° ¢76W'EöW†f—'7Bæ6Æ72æ–çFW&æÅöæÖR‚’Â$6÷VçFW""“°¢76W'EöW†f—'7Bæ6Æ72æ'—FW2‚’Â6V6öæBæ6Æ72æ'—FW2‚’“°¢76W'EöW†f—'7Eö÷WGWBæ6Æ75öf–ÆW2‚’ÂfV2¶f—'7BçF‚æ6ÆöæR‚•Ò“°¢76W'EöW‡6V6öæEö÷WGWBæ6Æ75öf–ÆW2‚’ÂfV2·6V6öæBçF‚æ6ÆöæR‚•Ò“° ¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†f—'7Bæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚$6÷VçFW"6†÷VÆBFV6öFR6ö×ÆWFVÇ’"“°¢76W'B€¢&VFW"æ—5öEöVæB‚’À¢&FV6öFW"6öç7VÖW2F†R6ö×ÆWFR6÷VçFW"f–ÆR ¢“°¢6Æ70¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚$6÷VçFW"6öç7FçB×ööÂ&VfW&Væ6W26†÷VÆB&RfÆ–B"“°¢76W'EöW†6Æ72çfW'6–öâæÖ¦÷"Â¥dÕô4Ä54d”ÄUôÔ¤õ%õdU%4”ôâ“°¢76W'EöW€¢6Æ72æ6öç7FçE÷ööÂæ6Æ75öæÖR†6Æ72çF†—5ö6Æ72’À¢ö²‚$6÷VçFW""¢“°¢76W'EöW†6Æ72æf–VÆG2æÆVâ‚’Â“°¢ÆWBf–VÆBÒf6Æ72æf–VÆG5³Ó°¢76W'EöW†6Æ72æ6öç7FçE÷ööÂçWFc‚†f–VÆBææÖUö–æFW‚’Âö²‚&7W'&VçB"’“°¢76W'EöW†6Æ72æ6öç7FçE÷ööÂçWFc‚†f–VÆBæFW67&—F÷%ö–æFW‚’Âö²‚$’"’“°¢76W'EöW€¢f–VÆBæ66W75öfÆw2ãÀ¢F÷GG•ö6Æ76f–ÆS£¦66W75öfÆw3£¤45õ$•dDP¢“°¢76W'EöW€¢6Æ70¢æÖWF†öG0¢æ—FW"‚¢æÖ‡ÆÖWF†öGÂ€¢6Æ72æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’çVçw&‚’À¢6Æ72æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBæFW67&—F÷%ö–æFW‚’çVçw&‚¢’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢°¢‚#Æ–æ—Câ"Â"„’•b"’À¢‚&vWB"Â"‚”’"’À¢‚'6WB"Â"„’•b"’À¢‚&–æ7&VÖVçB"Â"‚”’"’À¢Ğ¢“°¢76W'B€¢6Æ70¢æÖWF†öG0¢æ—FW"‚¢æÆÂ‡ÆÖWF†öGÂÖWF†öBæ66W75öfÆw2æ—5÷V&Æ–2‚’¢“°¢76W'EöW€¢6Æ70¢æÖWF†öG0¢æ—FW"‚¢æf–ÇFW"‡ÆÖWF†öGÂ6Æ72æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’ÓÒö²‚#Æ–æ—Câ"’¢æ6÷VçB‚’À¢À¢'F†R&ÖWFW&—¦VB6Æ72×W7Bæ÷B7V—&R†çFöÒæòÖ&r6öç7G'V7F÷" ¢“° ¢ÆWB6öç7G'V7F÷"ÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â#Æ–æ—Câ"“°¢76W'Eö÷6öFU÷6WVVæ6R†6öç7G'V7F÷"Âe³ƒ&Â†#rÂƒ&Âƒ"Â†#RÂ†#Ò“°¢76W'EöÖWF†öE÷&VfW&Væ6R€¢f6Æ72À¢6öç7G'V7F÷"À¢†#rÀ¢&¦föÆærôö&¦V7B"À¢#Æ–æ—Câ"À¢"‚•b"À¢“°¢76W'Eöf–VÆE÷&VfW&Væ6R‚f6Æ72Â6öç7G'V7F÷"Â†#RÂ$6÷VçFW""Â&7W'&VçB"Â$’"“°¢76W'Eö÷6öFU÷6WVVæ6R†VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â&vWB"’Âe³ƒ&Â†#BÂ†5Ò“°¢76W'Eöf–VÆE÷&VfW&Væ6R€¢f6Æ72À¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â&vWB"’À¢†#BÀ¢$6÷VçFW""À¢&7W'&VçB"À¢$’"À¢“°¢76W'Eö÷6öFU÷6WVVæ6R€¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â'6WB"’À¢e³ƒ&ÂƒFBÂƒ"Âƒ6RÂƒ&2ÂƒBÂ†#RÂ†#ÒÀ¢“°¢76W'Eö÷6öFU÷6WVVæ6R€¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â&–æ7&VÖVçB"’À¢e°¢ƒ&ÂƒF2Âƒ&Â†#BÂƒBÂƒcÂƒ6BÂƒ&"Âƒ2Â†#RÂƒ&Â†#BÂ†2À¢ÒÀ¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢fW&–g•ö6÷VçFW%÷v—F…ö¦F²†f—'7Eö÷WGWBçF‚‚’“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ&VÅ÷&—fFUöf–VÆG5÷6÷W&6UöVÖ—G5÷&—fFUöf–æÅ÷7F÷&vUöæEö&ööÆVåövWGFW"‚’°¢ÆWBf—'7Eö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&—fFRÖf–VÆG2Öf—'7B"“°¢ÆWB6V6öæEö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&—fFRÖf–VÆG2×6V6öæB"“°¢ÆWBf—'7BÒ6ö×–ÆUöf—‡GW&R‚%&—fFTf–VÆG2ç66Æ"Âf—'7Eö÷WGWBçF‚‚’’çVçw&‚“°¢ÆWB6V6öæBÒ6ö×–ÆUöf—‡GW&R‚%&—fFTf–VÆG2ç66Æ"Â6V6öæEö÷WGWBçF‚‚’’çVçw&‚“° ¢76W'EöW†f—'7Bæ6Æ72æ–çFW&æÅöæÖR‚’Â%&—fFTf–VÆG2"“°¢76W'EöW†f—'7Bæ6Æ72æ'—FW2‚’Â6V6öæBæ6Æ72æ'—FW2‚’“°¢76W'EöW†f—'7Eö÷WGWBæ6Æ75öf–ÆW2‚’ÂfV2¶f—'7BçF‚æ6ÆöæR‚•Ò“°¢76W'EöW‡6V6öæEö÷WGWBæ6Æ75öf–ÆW2‚’ÂfV2·6V6öæBçF‚æ6ÆöæR‚•Ò“° ¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†f—'7Bæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚%&—fFTf–VÆG26†÷VÆBFV6öFR6ö×ÆWFVÇ’"“°¢76W'B€¢&VFW"æ—5öEöVæB‚’À¢&FV6öFW"6öç7VÖW2F†R6ö×ÆWFR&—fFTf–VÆG2f–ÆR ¢“°¢6Æ70¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚%&—fFTf–VÆG26öç7FçB×ööÂ&VfW&Væ6W26†÷VÆB&RfÆ–B"“°¢76W'EöW†6Æ72çfW'6–öâæÖ¦÷"Â¥dÕô4Ä54d”ÄUôÔ¤õ%õdU%4”ôâ“°¢ÆWBf–VÆG2Ò6Æ70¢æf–VÆG0¢æ—FW"‚¢æÖ‡Æf–VÆGÂ°¢€¢6Æ72æ6öç7FçE÷ööÂçWFc‚†f–VÆBææÖUö–æFW‚’çVçw&‚’À¢6Æ72æ6öç7FçE÷ööÂçWFc‚†f–VÆBæFW67&—F÷%ö–æFW‚’çVçw&‚’À¢f–VÆBæ66W75öfÆw2À¢¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW†f–VÆG2æÆVâ‚’Â2“°¢76W'EöW‚†f–VÆG5³ÒãÂf–VÆG5³Òã’Â‚'6VVB"Â$’"’“°¢76W'EöW€¢f–VÆG5³Òã"ãÀ¢F÷GG•ö6Æ76f–ÆS£¦66W75öfÆw3£¤45õ$•dDRÂF÷GG•ö6Æ76f–ÆS£¦66W75öfÆw3£¤45ôd”äÀ¢“°¢76W'EöW‚†f–VÆG5³ÒãÂf–VÆG5³Òã’Â‚&Væ&ÆVB"Â%¢"’“°¢76W'EöW€¢f–VÆG5³Òã"ãÀ¢F÷GG•ö6Æ76f–ÆS£¦66W75öfÆw3£¤45õ$•dDRÂF÷GG•ö6Æ76f–ÆS£¦66W75öfÆw3£¤45ôd”äÀ¢“°¢76W'EöW‚†f–VÆG5³%ÒãÂf–VÆG5³%Òã’Â‚&F—6&ÆVB"Â%¢"’“°¢76W'EöW€¢f–VÆG5³%Òã"ãÀ¢F÷GG•ö6Æ76f–ÆS£¦66W75öfÆw3£¤45õ$•dDRÂF÷GG•ö6Æ76f–ÆS£¦66W75öfÆw3£¤45ôd”äÀ¢“°¢76W'EöW€¢6Æ70¢æÖWF†öG0¢æ—FW"‚¢æÖ‡ÆÖWF†öGÂ€¢6Æ72æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’çVçw&‚’À¢6Æ72æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBæFW67&—F÷%ö–æFW‚’çVçw&‚¢’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢°¢‚#Æ–æ—Câ"Â"‚•b"’À¢‚'6VVEfÇVR"Â"‚”’"’À¢‚&Væ&ÆVEfÇVR"Â"‚•¢"’À¢‚&F—6&ÆVEfÇVR"Â"‚•¢"’À¢Ğ¢“°¢76W'EöW†VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â'6VVEfÇVR"•³ÒÂ†#B“°¢76W'Eöf–VÆE÷&VfW&Væ6R€¢f6Æ72À¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â'6VVEfÇVR"’À¢†#BÀ¢%&—fFTf–VÆG2"À¢'6VVB"À¢$’"À¢“°¢76W'Eöf–VÆE÷&VfW&Væ6R€¢f6Æ72À¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â&Væ&ÆVEfÇVR"’À¢†#BÀ¢%&—fFTf–VÆG2"À¢&Væ&ÆVB"À¢%¢"À¢“°¢76W'Eöf–VÆE÷&VfW&Væ6R€¢f6Æ72À¢VÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â&F—6&ÆVEfÇVR"’À¢†#BÀ¢%&—fFTf–VÆG2"À¢&F—6&ÆVB"À¢%¢"À¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢fW&–g•÷&—fFUöf–VÆG5÷v—F…ö¦F²†f—'7Eö÷WGWBçF‚‚’“°¢Ğ§Ğ ¢5·FW7EĞ¦fâVç7W÷'FVE÷&VÅöf–VÆEöæEö6öç7G'V7F÷%÷6÷W&6W5öf–Å÷v—F†÷WEö'F–f7G2‚’°¢76W'E÷6÷W&6Uöf–ÇW&R€¢'w&öærÖf–VÆBÖ–æ—F–Æ—¦W""À¢&6Æ72w&öæt–æ—F–Æ—¦W"²&—fFRfÂ6VVC¢–çBÒG'VRÒ"À¢Æf–ÇW&WÂÖF6†W2†f–ÇW&RÂ—VÆ–æTf–ÇW&S£¥G—W"…ò’’À¢“°¢76W'E÷6÷W&6Uöf–ÇW&R€¢&–Ö×WF&ÆRÖf–VÆBÖ76–væÖVçB"À¢&6Æ72–Ö×WF&ÆT76–væÖVçB²&—fFRfÂ6VVC¢–çBÒ²FVbWFFR‚“¢Væ—BÒ6VVBÒ"Ò"À¢Æf–ÇW&WÂÖF6†W2†f–ÇW&RÂ—VÆ–æTf–ÇW&S£¥G—W"…ò’’À¢“°¢76W'E÷6÷W&6Uöf–ÇW&R€¢&–×Æ–6—BÖæö&rÖf÷'v&BÖf–VÆB×&VB"À¢&6Æ72f÷'v&E&VB²fÂf—'7C¢–çBÒÆFW#²fÂÆFW#¢–çBÒ"Ò"À¢Æf–ÇW&WÂ°¢ÖF6†W2€¢f–ÇW&RÀ¢—VÆ–æTf–ÇW&S£¤6öFVvVâ€¢§fÔ6öFVvVäW'&÷#£¤f–VÆD–æ—F–Æ—¦W%&VG5Væ–æ—F–Æ—¦VDf–VÆB²ââĞ¢¢¢ÒÀ¢“°¢76W'E÷6÷W&6Uöf–ÇW&R€¢&–×Æ–6—BÖæö&r×F†—2ÖW66R"À¢&6Æ72F†—4W66R²fÂæW‡C¢F†—4W66RÒF†—2Ò"À¢Æf–ÇW&WÂ°¢ÖF6†W2€¢f–ÇW&RÀ¢—VÆ–æTf–ÇW&S£¤6öFVvVâ€¢§fÔ6öFVvVäW'&÷#£¥Vç7W÷'FVD§fÕ&VfW&Væ6UG—R²ââĞ¢Â§fÔ6öFVvVäW'&÷#£¥Vç7W÷'FVD6öç7G'V7F÷$f–VÆD–æ—F–Æ—¦W"²ââĞ¢¢¢ÒÀ¢“°¢ÆWB&—fFUöæö&uö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&—fFRÖW‡Æ–6—BÖæö&rÖ6öç7G'V7F÷""“°¢ÆWB&—fFUöæö&rÒ6ö×–ÆU÷6÷W&6R€¢&6Æ72†–FFVâ&—fFR‚’²fÂfÇVS¢–çBÒÒ"À¢%&—fFTæô&rç66Æ"À¢&—fFUöæö&uö÷WGWBçF‚‚’À¢¢æW‡V7B‚&âW‡Æ–6—B&—fFRæòÖ&r6öç7G'V7F÷"¶VW2—G2FV6Æ&VB66W72"“°¢ÆWB×WB&—fFUöæö&u÷&VFW"Ò&VFW#£¦æWr‡&—fFUöæö&ræ6Æ72æ'—FW2‚’“°¢ÆWB&—fFUöæö&uö6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&—fFUöæö&u÷&VFW"¢æW‡V7B‚&FV6öFR6Æ72v—F‚W‡Æ–6—B&—fFR6öç7G'V7F÷""“°¢ÆWB6öç7G'V7F÷'2Ò&—fFUöæö&uö6Æ70¢æÖWF†öG0¢æ—FW"‚¢æf–ÇFW"‡ÆÖWF†öGÂ&—fFUöæö&uö6Æ72æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’ÓÒö²‚#Æ–æ—Câ"’¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW€¢6öç7G'V7F÷'2æÆVâ‚’À¢À¢&æòGWÆ–6FRV&Æ–2æòÖ&r6öç7G'V7F÷" ¢“°¢76W'EöW€¢&—fFUöæö&uö6Æ70¢æ6öç7FçE÷ööÀ¢çWFc‚†6öç7G'V7F÷'5³ÒæFW67&—F÷%ö–æFW‚’À¢ö²‚"‚•b"¢“°¢76W'B†6öç7G'V7F÷'5³Òæ66W75öfÆw2æ—5÷&—fFR‚’“°¢f÷"†Æ&VÂÂ6÷W&6R’–â°¢€¢&FVfVÇFVBÖ6öç7G'V7F÷"×&ÖWFW""À¢&6Æ72v—F„FVfVÇB‡fÇVS¢–çBÒ’²&—fFRfÂ7W'&VçC¢–çBÒfÇVRÒ"À¢’À¢€¢&6öçFW‡GVÂÖ6öç7G'V7F÷"×&ÖWFW""À¢&6Æ72v—F…W6–ær‡W6–ær6öçFW‡C¢–çB’²&—fFRfÂ7W'&VçC¢–çBÒ6öçFW‡BÒ"À¢’À¢€¢'6V6öæF'’Ö6öç7G'V7F÷""À¢&6Æ72v—F…6V6öæF'’‡fÇVS¢–çB’²&—fFRfÂ7W'&VçC¢–çBÒfÇVS²FVbF†—2‚’ÒF†—2ƒ’Ò"À¢’À¢Ò°¢76W'E÷6÷W&6Uöf–ÇW&R†Æ&VÂÂ6÷W&6RÂÆf–ÇW&WÂ°¢ÖF6†W2€¢f–ÇW&RÀ¢—VÆ–æTf–ÇW&S£¤6öFVvVâ€¢§fÔ6öFVvVäW'&÷#£¥Vç7W÷'FVD6öç7G'V7F÷"²ââĞ¢Â§fÔ6öFVvVäW'&÷#£¤–çfÆ–E&–Ö'”6öç7G'V7F÷"²ââĞ¢¢¢Ò“°¢Ğ§Ğ ¢5·FW7EĞ¦fâF—66&FVEöF÷V&ÆU÷fÇVW5öf–Å÷v—F†÷WE÷V&Æ—6†–æuö'F–f7G2‚’°¢76W'E÷6÷W&6Uöf–ÇW&R€¢&F—66&FVBÖF÷V&ÆR×fÇVR"À¢&6Æ72F÷V&ÆTF—66&B²FVbGv–6R‡fÇVS¢F÷V&ÆR“¢F÷V&ÆRÒ²fÇVS²fÇVRÒÒ"À¢Æf–ÇW&WÂ°¢ÖF6†W2€¢f–ÇW&RÀ¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¥Vç7W÷'FVEG—VDW‡&W76–öâ°¢æöFS¢&F—66&FVB6FVv÷'’×GvòfÇVR&WV—&W26FVv÷'’×Gvò7F6²÷W&F–öâ"À¢âà¢Ò¢¢ÒÀ¢“°§Ğ ¢5·FW7EĞ¦fâVç7W÷'FVEöfÆöEöF÷V&ÆU÷6÷W&6Uö66W5öf–ÅöE÷F†V—%öf—'7E÷7FvU÷v—F†÷WEö&F6…ö'F–f7G2‚’°¢VçVÒW‡V7FVDf–ÇW&R°¢ÖWF†öD&öG•G—TÖ—6ÖF6‚À¢Væ–æ—F–Æ—¦VDÆö6ÂÀ¢ÖVÖ&W$Æöö·WÀ¢Ğ ¢ÆWB66W2Ò°¢€¢&fÆöB×FòÖ–çB"À¢&6Æ72&V¦V7FVB²FVbæ'&÷r‡fÇVS¢fÆöB“¢–çBÒfÇVRÒ"À¢W‡V7FVDf–ÇW&S£¤ÖWF†öD&öG•G—TÖ—6ÖF6‚À¢’À¢€¢&fÆöB×FòÖF÷V&ÆR"À¢&6Æ72&V¦V7FVB²FVbv–FVâ‡fÇVS¢fÆöB“¢F÷V&ÆRÒfÇVRÒ"À¢W‡V7FVDf–ÇW&S£¤ÖWF†öD&öG•G—TÖ—6ÖF6‚À¢’À¢€¢&F÷V&ÆR×FòÖÆöær"À¢&6Æ72&V¦V7FVB²FVbæ'&÷r‡fÇVS¢F÷V&ÆR“¢ÆöærÒfÇVRÒ"À¢W‡V7FVDf–ÇW&S£¤ÖWF†öD&öG•G—TÖ—6ÖF6‚À¢’À¢€¢&F÷V&ÆRÖÆ—FW&Â×FòÖfÆöB×&WGW&â"À¢&6Æ72&V¦V7FVB²FVbw&öæuF&vWB‚“¢fÆöBÒ2ãRÒ"À¢W‡V7FVDf–ÇW&S£¤ÖWF†öD&öG•G—TÖ—6ÖF6‚À¢’À¢€¢'Væ–æ—F–Æ—¦VBÖF÷V&ÆRÖÆö6Â"À¢&6Æ72&V¦V7FVC¥ÆâFVbÖ—76–ær‚“¢F÷V&ÆRÕÆâf"&W7VÇC¢F÷V&ÆUÆâ&W7VÇEÆâ"À¢W‡V7FVDf–ÇW&S£¥Væ–æ—F–Æ—¦VDÆö6ÂÀ¢’À¢€¢&fÆöBÖFF—F–öâ"À¢&6Æ72&V¦V7FVB²FVbFB†ÆVgC¢fÆöBÂ&–v‡C¢fÆöB“¢fÆöBÒÆVgB²&–v‡BÒ"À¢W‡V7FVDf–ÇW&S£¤ÖVÖ&W$Æöö·WÀ¢’À¢€¢&fÆöBÖ6ö×&—6öâ"À¢&6Æ72&V¦V7FVB²FVbÆW72†ÆVgC¢fÆöBÂ&–v‡C¢fÆöB“¢&ööÆVâÒÆVgBÂ&–v‡BÒ"À¢W‡V7FVDf–ÇW&S£¤ÖVÖ&W$Æöö·WÀ¢’À¢€¢&F÷V&ÆRÖFF—F–öâ"À¢&6Æ72&V¦V7FVB²FVbFB†ÆVgC¢F÷V&ÆRÂ&–v‡C¢F÷V&ÆR“¢F÷V&ÆRÒÆVgB²&–v‡BÒ"À¢W‡V7FVDf–ÇW&S£¤ÖVÖ&W$Æöö·WÀ¢’À¢€¢&F÷V&ÆRÖ6ö×&—6öâ"À¢&6Æ72&V¦V7FVB²FVbÆW72†ÆVgC¢F÷V&ÆRÂ&–v‡C¢F÷V&ÆR“¢&ööÆVâÒÆVgBÂ&–v‡BÒ"À¢W‡V7FVDf–ÇW&S£¤ÖVÖ&W$Æöö·WÀ¢’À¢Ó° ¢f÷"†Æ&VÂÂ–çfÆ–Eö6Æ72ÂW‡V7FVEöf–ÇW&R’–â66W2°¢ÆWB6÷W&6RĞ¢f÷&ÖB‚&6Æ72fÆ–D&Vf÷&Tf–ÇW&R·²FVbfÇVR‚“¢–çBÒ×ÕÆç¶–çfÆ–Eö6Æ77ÕÆâ"“°¢76W'E÷6÷W&6Uö&F6…öf–ÇW&R†Æ&VÂÂg6÷W&6RÂÆf–ÇW&WÂÖF6‚W‡V7FVEöf–ÇW&R°¢W‡V7FVDf–ÇW&S£¤ÖWF†öD&öG•G—TÖ—6ÖF6‚ÓâÖF6†W2€¢f–ÇW&RÀ¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¤ÖWF†öD&öG•G—TÖ—6ÖF6‚²ââÒ¢’À¢W‡V7FVDf–ÇW&S£¥Væ–æ—F–Æ—¦VDÆö6ÂÓâÖF6†W2€¢f–ÇW&RÀ¢—VÆ–æTf–ÇW&S£¥G—W"…G—W$W'&÷#£¤Æö6ÅfÇVU&–v‡D†æE6–FTÖ—76–ær²ââÒ¢’À¢W‡V7FVDf–ÇW&S£¤ÖVÖ&W$Æöö·WÓâ°¢ÖF6†W2†f–ÇW&RÂ—VÆ–æTf–ÇW&S£¥G—W"…G—W$W'&÷#£¤ÖVÖ&W$Æöö·W…ò’’¢Ğ¢Ò“°¢Ğ§Ğ ¦fâ76W'EöÖWF†öE÷&VfW&Væ6R€¢6Æ73¢d6Æ74f–ÆSÂuóâÀ¢6öFS¢e·S…ÒÀ¢÷6öFS¢S‚À¢÷væW#¢g7G"À¢æÖS¢g7G"À¢FW67&—F÷#¢g7G"À¢’°¢ÆWBöfg6WBÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ–çFõö—FW"‚¢æf–æEöÖ‡Â†öfg6WBÂf÷VæB—Â†f÷VæBÓÒ÷6öFR’çF†Vå÷6öÖR†öfg6WB’¢æW‡V7B‚&W‡V7FVBÖWF†öB–çfö6F–öâ–ç7G'V7F–öâ"“°¢ÆWB–æFW‚ÒSc£¦g&öÕö&Uö'—FW2…¶6öFU¶öfg6WB²ÒÂ6öFU¶öfg6WB²%ÕÒ“°¢ÆWB6öç7FçEööÄVçG'“£¤ÖWF†öG&Vb°¢6Æ75ö–æFW‚À¢æÖUöæE÷G—Uö–æFW‚À¢ÒÒ6Æ70¢æ6öç7FçE÷ööÀ¢ævWB„6öç7FçEööÄ–æFW‚†–æFW‚’¢æW‡V7B‚&ÖWF†öB–çfö6F–öâ6öç7FçB×ööÂVçG'’"¢VÇ6R°¢æ–2‚&ÖWF†öB–çfö6F–öâ×W7B&VfW&Væ6RÖWF†öG&Vb"“°¢Ó°¢76W'EöW†6Æ72æ6öç7FçE÷ööÂæ6Æ75öæÖR‚¦6Æ75ö–æFW‚’Âö²†÷væW"’“°¢76W'EöW€¢6Æ72æ6öç7FçE÷ööÂææÖUöæE÷G—R‚¦æÖUöæE÷G—Uö–æFW‚’À¢ö²‚†æÖRÂFW67&—F÷"’¢“°§Ğ ¦fâFV6öFUöVÖ—GFVEö6Æ72†'F–f7C¢dVÖ—GFVD'F–f7B’Óâ6Æ74f–ÆSÂuóâ°¢ÆWB×WB&VFW"Ò&VFW#£¦æWr†'F–f7Bæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&VFW"’æW‡V7B‚&FV6öFR6ö×ÆWFRvVæW&FVB6Æ76f–ÆR"“°¢76W'B€¢&VFW"æ—5öEöVæB‚’À¢&6Æ72FV6öFW"6öç7VÖW2F†R6ö×ÆWFR6Æ76f–ÆR ¢“°¢6Æ70¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚'fÆ–FFR6öç7FçB×ööÂ&VfW&Væ6W2"“°¢76W'EöW†6Æ72çfW'6–öâæÖ¦÷"ÂcR“°¢76W'EöW€¢6Æ72æ6öç7FçE÷ööÂæ6Æ75öæÖR†6Æ72çF†—5ö6Æ72’À¢ö²†'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’¢“°¢6Æ70§Ğ ¦fâFV6öFUöVÖ—GFVEö6Æ75öæÖVCÂvâ†'F–f7G3¢bv´VÖ—GFVD'F–f7EÒÂæÖS¢g7G"’Óâ6Æ74f–ÆSÂvâ°¢FV6öFUöVÖ—GFVEö6Æ72€¢'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒæÖR¢çVçw&ö÷%öVÇ6R‡ÇÂæ–2‚&Ö—76–ærVÖ—GFVB6Æ72¶æÖWÒ"’’À¢§Ğ ¦fâÖWF†öG5÷v—F…öFW67&—F÷'2†6Æ73¢d6Æ74f–ÆSÂuóâ’ÓâfV3Â…7G&–ærÂ7G&–ær“â°¢6Æ70¢æÖWF†öG0¢æ—FW"‚¢æÖ‡ÆÖWF†öGÂ°¢€¢6Æ70¢æ6öç7FçE÷ööÀ¢çWFc‚†ÖWF†öBææÖUö–æFW‚¢çVçw&‚¢çFõö÷væVB‚’À¢6Æ70¢æ6öç7FçE÷ööÀ¢çWFc‚†ÖWF†öBæFW67&—F÷%ö–æFW‚¢çVçw&‚¢çFõö÷væVB‚’À¢¢Ò¢æ6öÆÆV7B‚§Ğ ¦fâ76W'Eö–çfö6F–öå÷6WVVæ6R€¢6Æ73¢d6Æ74f–ÆSÂuóâÀ¢6öFS¢e·S…ÒÀ¢W‡V7FVC¢e²‡S‚Âg7G"Âg7G"Âg7G"•ÒÀ¢’°¢ÆWB7GVÂÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ–çFõö—FW"‚¢æf–ÇFW"‡Â…òÂ÷6öFR—ÂÖF6†W2†÷6öFRÂ†#bÂ†#r’¢æÖ‡Â†öfg6WBÂ÷6öFR—Â°¢ÆWB–æFW‚ÒSc£¦g&öÕö&Uö'—FW2…¶6öFU¶öfg6WB²ÒÂ6öFU¶öfg6WB²%ÕÒ“°¢ÆWB6öç7FçEööÄVçG'“£¤ÖWF†öG&Vb°¢6Æ75ö–æFW‚À¢æÖUöæE÷G—Uö–æFW‚À¢ÒÒ6Æ70¢æ6öç7FçE÷ööÀ¢ævWB„6öç7FçEööÄ–æFW‚†–æFW‚’¢æW‡V7B‚&–çfö6F–öâ&VfW&Væ6W26öç7FçB×ööÂVçG'’"¢VÇ6R°¢æ–2‚&–çfö¶Wf—'GVÂæB–çfö¶W7V6–Â×W7B&VfW&Væ6RÖWF†öG&Vb"“°¢Ó°¢ÆWB÷væW"Ò6Æ72æ6öç7FçE÷ööÂæ6Æ75öæÖR‚¦6Æ75ö–æFW‚’çVçw&‚“°¢ÆWB†æÖRÂFW67&—F÷"’Ò6Æ70¢æ6öç7FçE÷ööÀ¢ææÖUöæE÷G—R‚¦æÖUöæE÷G—Uö–æFW‚¢çVçw&‚“°¢†÷6öFRÂ÷væW"ÂæÖRÂFW67&—F÷"¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW€¢7GVÂÂW‡V7FVBÀ¢'VæW‡V7FVB÷&FW&VBÖWF†öB&VfW&Væ6W2–â6öFR ¢“°§Ğ ¦fâ76W'Eöf–VÆE÷&VfW&Væ6R€¢6Æ73¢d6Æ74f–ÆSÂuóâÀ¢6öFS¢e·S…ÒÀ¢÷6öFS¢S‚À¢÷væW#¢g7G"À¢æÖS¢g7G"À¢FW67&—F÷#¢g7G"À¢’°¢ÆWBöfg6WBÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ–çFõö—FW"‚¢æf–æEöÖ‡Â†öfg6WBÂf÷VæB—Â†f÷VæBÓÒ÷6öFR’çF†Vå÷6öÖR†öfg6WB’¢æW‡V7B‚&W‡V7FVBf–VÆB66W72–ç7G'V7F–öâ"“°¢ÆWB–æFW‚ÒSc£¦g&öÕö&Uö'—FW2…¶6öFU¶öfg6WB²ÒÂ6öFU¶öfg6WB²%ÕÒ“°¢ÆWB6öç7FçEööÄVçG'“£¤f–VÆG&Vb°¢6Æ75ö–æFW‚À¢æÖUöæE÷G—Uö–æFW‚À¢ÒÒ6Æ70¢æ6öç7FçE÷ööÀ¢ævWB„6öç7FçEööÄ–æFW‚†–æFW‚’¢æW‡V7B‚&f–VÆB66W726öç7FçB×ööÂVçG'’"¢VÇ6R°¢æ–2‚&f–VÆB66W72×W7B&VfW&Væ6Rf–VÆG&Vb"“°¢Ó°¢76W'EöW†6Æ72æ6öç7FçE÷ööÂæ6Æ75öæÖR‚¦6Æ75ö–æFW‚’Âö²†÷væW"’“°¢76W'EöW€¢6Æ72æ6öç7FçE÷ööÂææÖUöæE÷G—R‚¦æÖUöæE÷G—Uö–æFW‚’À¢ö²‚†æÖRÂFW67&—F÷"’¢“°§Ğ ¦fâ76W'Eöf–VÆE÷&VfW&Væ6U÷6WVVæ6R€¢6Æ73¢d6Æ74f–ÆSÂuóâÀ¢6öFS¢e·S…ÒÀ¢W‡V7FVC¢e²‡S‚Âg7G"Âg7G"Âg7G"•ÒÀ¢’°¢ÆWB7GVÂÒFV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR¢æ–çFõö—FW"‚¢æf–ÇFW%öÖ‡Â†öfg6WBÂ÷6öFR—Â°¢–bÖF6†W2†÷6öFRÂ†#BÂ†#R’°¢&WGW&âæöæS°¢Ğ¢ÆWB–æFW‚ÒSc£¦g&öÕö&Uö'—FW2…¶6öFU¶öfg6WB²ÒÂ6öFU¶öfg6WB²%ÕÒ“°¢ÆWB6öç7FçEööÄVçG'“£¤f–VÆG&Vb°¢6Æ75ö–æFW‚À¢æÖUöæE÷G—Uö–æFW‚À¢ÒÒ6Æ72æ6öç7FçE÷ööÂævWB„6öç7FçEööÄ–æFW‚†–æFW‚’“ğ¢VÇ6R°¢æ–2‚&f–VÆB66W72×W7B&VfW&Væ6Rf–VÆG&Vb"“°¢Ó°¢ÆWB†æÖRÂFW67&—F÷"’Ò6Æ70¢æ6öç7FçE÷ööÀ¢ææÖUöæE÷G—R‚¦æÖUöæE÷G—Uö–æFW‚¢çVçw&‚“°¢6öÖR‚€¢÷6öFRÀ¢6Æ72æ6öç7FçE÷ööÂæ6Æ75öæÖR‚¦6Æ75ö–æFW‚’çVçw&‚’À¢æÖRÀ¢FW67&—F÷"À¢’¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW€¢7GVÂÂW‡V7FVBÀ¢'VæW‡V7FVB÷&FW&VBf–VÆB&VfW&Væ6W2–â6öFR ¢“°§Ğ ¦fâ76W'Eöæõöf–VÆE÷&VfW&Væ6U÷Fõö÷væW"†6Æ73¢d6Æ74f–ÆSÂuóâÂ6öFS¢e·S…ÒÂ÷væW#¢g7G"’°¢f÷"†öfg6WBÂ÷6öFR’–âFV6öFU÷7V'6WEö–ç7G'V7F–öç2†6öFR’°¢–bÖF6†W2†÷6öFRÂ†#BÂ†#R’°¢6öçF–çVS°¢Ğ¢ÆWB–æFW‚ÒSc£¦g&öÕö&Uö'—FW2…¶6öFU¶öfg6WB²ÒÂ6öFU¶öfg6WB²%ÕÒ“°¢ÆWB6öç7FçEööÄVçG'“£¤f–VÆG&Vb²6Æ75ö–æFW‚ÂââÒÒ6Æ70¢æ6öç7FçE÷ööÀ¢ævWB„6öç7FçEööÄ–æFW‚†–æFW‚’¢æW‡V7B‚&f–VÆB66W72&VfW&Væ6W26öç7FçB×ööÂVçG'’"¢VÇ6R°¢æ–2‚&f–VÆB66W72×W7B&VfW&Væ6Rf–VÆG&Vb"“°¢Ó°¢76W'EöæR€¢6Æ72æ6öç7FçE÷ööÂæ6Æ75öæÖR‚¦6Æ75ö–æFW‚’À¢ö²†÷væW"’À¢&W‡FW&æÂ66W76÷"6ÆÂ×W7Bæ÷B66W72&—fFR&6¶–ær7F÷&vR ¢“°¢Ğ§Ğ ¦fâ76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R†÷WGWC¢g7G"ÂÖWF†öEö†VFW#¢g7G"ÂW‡V7FVC¢e²g7G%Ò’°¢ÆWBÖWF†öE÷7F'BÒ÷WGW@¢æf–æB†ÖWF†öEö†VFW"¢çVçw&ö÷%öVÇ6R‡ÇÂæ–2‚&¦f÷WGWBöÖ—GFVBÖWF†öB¶ÖWF†öEö†VFW#£÷Ó¥Æç¶÷WGWGÒ"’“°¢ÆWBÖWF†öBÒf÷WGWE¶ÖWF†öE÷7F'BâåÓ°¢ÆWBÖWF†öEöVæBÒ°¢ÖWF†öE³âåÒæf–æB‚%ÆâV&Æ–2"’À¢ÖWF†öE³âåÒæf–æB‚%Æâ&—fFR"’À¢ÖWF†öE³âåÒæf–æB‚%ÆçÒ"’À¢Ğ¢æ–çFõö—FW"‚¢æfÆGFVâ‚¢æÖ–â‚¢æÖö÷"†ÖWF†öBæÆVâ‚’ÂÆöfg6WGÂöfg6WB²“°¢ÆWBÖWF†öBÒfÖWF†öE²âæÖWF†öEöVæEÓ°¢ÆWB6öFRÒÖWF†ö@¢ç7Æ—Eööæ6R‚$6öFS¢"¢æÖ‡Â…òÂ6öFR—Â6öFR¢çVçw&ö÷%öVÇ6R‡ÇÂæ–2‚&¦fÖWF†öB¶ÖWF†öEö†VFW#£÷Ò†2æò6öFR6V7F–öâ"’“°¢ÆWB–ç7G'V7F–öç2Ò6öFP¢æÆ–æW2‚¢æf–ÇFW%öÖ‡ÆÆ–æWÂ°¢ÆWB…òÂ–ç7G'V7F–öâ’ÒÆ–æRç7Æ—Eööæ6R‚#¢"“ó°¢–ç7G'V7F–öâç7Æ—E÷v†—FW76R‚’ææW‡B‚¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW€¢–ç7G'V7F–öç2ÂW‡V7FVBÀ¢'VæW‡V7FVB¦f–ç7G'V7F–öç2f÷"¶ÖWF†öEö†VFW'Ó¥Æç¶ÖWF†öGÒ ¢“°§Ğ ¢5·FW7EĞ¦fâ¦fö–ç7G'V7F–öåöW‡G&7F–öå÷7F÷5öE÷F†UöVæ6Æ÷6–æuö6Æ75ö&÷VæF'’‚’°¢ÆWB÷WGWBÒ"2 ¢V&Æ–2–çBF÷V&ÆVB‚“°¢FW67&—F÷#¢‚”¢6öFS ¢¢ÆöEó ¢¢–çfö¶Wf—'GVÂ3ròòÖWF†öBfÇVS¢‚”¢C¢—&WGW&à§Ğ¤6Æ76f–ÆR÷F×ôFW&—fVBæ6Æ70¢Ö¦÷"fW'6–öã¢cP¢fÆw3¢ƒƒ#’45õT$Ä”2Â45õ5UU ¢"3°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢÷WGWBÀ¢'V&Æ–2–çBF÷V&ÆVB‚“²"À¢e²&ÆöEó"Â&–çfö¶Wf—'GVÂ"Â&—&WGW&â%ÒÀ¢“°§Ğ ¦fâfW&–g•ö6÷VçFW%÷v—F…ö¦F²†÷WGWEöF—&V7F÷'“¢eF‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWEöF—&V7F÷'’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚FV×÷&'’F‚"“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…²"×"Â"Ö2"Â"×b"Â"Ö6Æ77F‚"Â÷WGWE÷F‚Â$6÷VçFW"%Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ff÷"6÷VçFW""“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦f&V¦V7FVB6÷VçFW#¢³£÷Ò"À¢¦f ¢“°¢ÆWB¦fö÷WGWBÒ7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB“°¢f÷"W‡V7FVB–â°¢&Ö¦÷"fW'6–öã¢cR"À¢'&—fFR–çB7W'&VçC²"À¢&FW67&—F÷#¢„’•b"À¢&FW67&—F÷#¢‚”’"À¢&FW67&—F÷#¢„’•b"À¢&vWFf–VÆB"À¢'WFf–VÆB"À¢&–FB"À¢Ò°¢76W'B€¢¦fö÷WGWBæ6öçF–ç2†W‡V7FVB’À¢&¦f÷WGWBöÖ—GFVB¶W‡V7FVC£÷Ó¥Æç¶¦fö÷WGWGÒ ¢“°¢Ğ¢76W'B†¦fö÷WGWBæ6öçF–ç2‚'V&Æ–26÷VçFW"†–çB’"’“°¢76W'B‚¦fö÷WGWBæ6öçF–ç2‚'V&Æ–26÷VçFW"‚“²"’“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢f¦fö÷WGWBÀ¢'V&Æ–26÷VçFW"†–çB“²"À¢e°¢&ÆöEó"À¢&–çfö¶W7V6–Â"À¢&ÆöEó"À¢&–ÆöEó"À¢'WFf–VÆB"À¢'&WGW&â"À¢ÒÀ¢“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢f¦fö÷WGWBÀ¢'V&Æ–2–çBvWB‚“²"À¢e²&ÆöEó"Â&vWFf–VÆB"Â&—&WGW&â%ÒÀ¢“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢f¦fö÷WGWBÀ¢'V&Æ–2fö–B6WB†–çB“²"À¢e°¢&ÆöEó"Â&7F÷&Uó""Â&–ÆöEó"Â&—7F÷&Uó2"Â&ÆöEó""Â&–ÆöEó2"Â'WFf–VÆB"À¢'&WGW&â"À¢ÒÀ¢“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢f¦fö÷WGWBÀ¢'V&Æ–2–çB–æ7&VÖVçB‚“²"À¢e°¢&ÆöEó"Â&7F÷&Uó"Â&ÆöEó"Â&vWFf–VÆB"Â&–6öç7Eó"Â&–FB"Â&—7F÷&Uó""À¢&ÆöEó"Â&–ÆöEó""Â'WFf–VÆB"Â&ÆöEó"Â&vWFf–VÆB"Â&—&WGW&â"À¢ÒÀ¢“° ¢ÆWB†&æW75÷F‚Ò÷WGWEöF—&V7F÷'’æ¦ö–â‚$6÷VçFW$†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW75÷F‚À¢'V&Æ–2f–æÂ6Æ726÷VçFW$†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²6÷VçFW"ÒæWr6÷VçFW"ƒ“²–b†ævWB‚’Ò’F‡&÷ræWr76W'F–öäW'&÷"…Â&–æ—F–ÅÂ"“²–b†æ–æ7&VÖVçB‚’Ò’F‡&÷ræWr76W'F–öäW'&÷"…Â&–æ7&VÖVçEÂ"“²ç6WBƒC“²–b†ævWB‚’ÒC’F‡&÷ræWr76W'F–öäW'&÷"…Â'6WEÂ"“²–b†æ–æ7&VÖVçB‚’ÒC"’F‡&÷ræWr76W'F–öäW'&÷"…Â&–æ7&VÖVçBgFW"6WEÂ"“²6÷VçFW""ÒæWr6÷VçFW"ƒ2“²–b†"ævWB‚’Ò2’F‡&÷ræWr76W'F–öäW'&÷"…Â'6V6öæB–ç7Fæ6UÂ"“²–b†ævWB‚’ÒC"’F‡&÷ræWr76W'F–öäW'&÷"…Â&–ç7Fæ6R7FFR6†&VEÂ"“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FR6÷VçFW"¦f†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWE÷F‚Â"ÖB"Â÷WGWE÷F…Ò¢æ&r‚f†&æW75÷F‚¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2f÷"6÷VçFW""“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2f–ÆVBf÷"6÷VçFW#¢³£÷Ò"À¢¦f0¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…²"Õ‡fW&–g“¦ÆÂ"Â"Ö6Æ77F‚"Â÷WGWE÷F‚Â$6÷VçFW$†&æW72%Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W"f÷"6÷VçFW""“°¢76W'B†¦fç7FGW2ç7V66W72‚’Â&¦ff–ÆVBf÷"6÷VçFW#¢³£÷Ò"Â¦f“°§Ğ ¦fâfW&–g•÷&VfW&Væ6Uöö&¦V7Eö—5÷v—F…ö¦F²†÷WGWEöF—&V7F÷'“¢eF‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWEöF—&V7F÷'’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚6Æ72÷WGWBF‚"“°¢ÆWB6÷VçFW"Ò6öÖÖæC£¦æWr‚&¦f"¢æ&w2…²"Ö2"Â"×b"Â"×"Â"Ö6Æ77F‚"Â÷WGWE÷F‚Â$6÷VçFW"%Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ff÷"6÷VçFW"’"“°¢76W'B€¢6÷VçFW"ç7FGW2ç7V66W72‚’À¢&¦f&V¦V7FVB6÷VçFW%Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f6÷VçFW"ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f6÷VçFW"ç7FFW'"¢“°¢ÆWB6÷VçFW"Ò7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f6÷VçFW"ç7FF÷WB“°¢76W'B†6÷VçFW"æ6öçF–ç2‚&FW67&—F÷#¢‚”Ä6÷VçFW#²"’“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R‚f6÷VçFW"Â'V&Æ–26÷VçFW"6VÆb‚“²"Âe²&ÆöEó"Â&&WGW&â%Ò“°¢f÷"6Æ75öæÖR–â²$6÷VçFW""Â$6÷VçFW%6W'f–6R%Ò°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…²"Ö2"Â"×b"Â"×"Â"Ö6Æ77F‚"Â÷WGWE÷F‚Â6Æ75öæÖUÒ¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ff÷"&VÂ×6÷W&6R&VfW&Væ6R—2"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦f&V¦V7FVB¶6Æ75öæÖWÕÆç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢ÆWB÷WGWBÒ7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB“°¢76W'B€¢÷WGWBæ6öçF–ç2‚&Ö¦÷"fW'6–öã¢cR"’À¢'¶6Æ75öæÖWÓ¢¶÷WGWGÒ ¢“°¢Ğ¢ÆWB6W'f–6Uö¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Ö2"À¢"×b"À¢"×"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢$6÷VçFW%6W'f–6R"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦fFò–ç7V7B6÷VçFW%6W'f–6Rg&ÖW2"“°¢76W'B€¢6W'f–6Uö¦fç7FGW2ç7V66W72‚’À¢&¦f&V¦V7FVB6÷VçFW%6W'f–6UÆç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚g6W'f–6Uö¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚g6W'f–6Uö¦fç7FFW'"¢“°¢76W'B€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚g6W'f–6Uö¦fç7FF÷WB’æ6öçF–ç2‚%7F6´ÖF&ÆR"’À¢&'&æ6†–ær6÷VçFW%6W'f–6RÖWF†öG2×W7B†fR7F6´ÖF&ÆRg&ÖW2 ¢“°¢ÆWB6W'f–6RÒ7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚g6W'f–6Uö¦fç7FF÷WB“°¢f÷"W‡V7FVB–â°¢&FW67&—F÷#¢„’”Ä6÷VçFW#²"À¢&FW67&—F÷#¢„Ä6÷VçFW#²”Ä6÷VçFW#²"À¢&FW67&—F÷#¢„Ä6÷VçFW#²”’"À¢&FW67&—F÷#¢…¤Ä6÷VçFW#´Ä6÷VçFW#²”Ä6÷VçFW#²"À¢&FW67&—F÷#¢„Ä6÷VçFW#´’”’"À¢"òòÖWF†öB6÷VçFW"ævWC¢‚”’"À¢"òòÖWF†öB6÷VçFW"æ–æ7&VÖVçC¢‚”’"À¢&7&VFS¢„’”Ä6÷VçFW#²"À¢'&VC¢„Ä6÷VçFW#²”’"À¢&æWr2"À¢&GW"À¢$6÷VçFW"åÂ#Æ–æ—CåÂ#¢„’•b"À¢Ò°¢76W'B€¢6W'f–6Ræ6öçF–ç2†W‡V7FVB’À¢&¦f÷WGWBöÖ—GFVB¶W‡V7FVC£÷Ó¥Æç·6W'f–6WÒ ¢“°¢Ğ¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢g6W'f–6RÀ¢'V&Æ–26÷VçFW"7&VFR†–çB“²"À¢e²&æWr"Â&GW"Â&–ÆöEó"Â&–çfö¶W7V6–Â"Â&&WGW&â%ÒÀ¢“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢g6W'f–6RÀ¢'V&Æ–26÷VçFW"77F‡&÷Vv‚„6÷VçFW"“²"À¢e²&ÆöEó"Â&&WGW&â%ÒÀ¢“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢g6W'f–6RÀ¢'V&Æ–2–çB&VB„6÷VçFW"“²"À¢e²&ÆöEó"Â&–çfö¶Wf—'GVÂ"Â&—&WGW&â%ÒÀ¢“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢g6W'f–6RÀ¢'V&Æ–2–çB&÷VæEG&—†–çB“²"À¢e°¢&ÆöEó"À¢&ÆöEó"À¢&–ÆöEó"À¢&–çfö¶Wf—'GVÂ"À¢&–çfö¶Wf—'GVÂ"À¢&—&WGW&â"À¢ÒÀ¢“° ¢ÆWB†&æW72Ò÷WGWEöF—&V7F÷'’æ¦ö–â‚%&VfW&Væ6Tö&¦V7D—4†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ72&VfW&Væ6Tö&¦V7D—4†&æW72µÆâV&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’µÆâ6÷VçFW%6W'f–6R6W'f–6RÒæWr6÷VçFW%6W'f–6R‚“µÆâ6÷VçFW"7&VFVBÒ6W'f–6Ræ7&VFRƒR“µÆâ–b†7&VFVBævWB‚’ÒR’F‡&÷ræWr76W'F–öäW'&÷"…Â&7&VFUÂ"“µÆâ–b‡6W'f–6Rç77F‡&÷Vv‚†7&VFVB’Ò7&VFVB’F‡&÷ræWr76W'F–öäW'&÷"…Â&–FVçF—G•Â"“µÆâ–b‡6W'f–6Rç&VB†7&VFVB’Ò7&VFVBævWB‚’’F‡&÷ræWr76W'F–öäW'&÷"…Â'&VEÂ"“µÆâ–b†7&VFVBç6VÆb‚’Ò7&VFVB’F‡&÷ræWr76W'F–öäW'&÷"…Â'F†—2–FVçF—G•Â"“µÆâ6÷VçFW"÷F†W"Ò6W'f–6Ræ7&VFRƒ‚“µÆâ–b‡6W'f–6Ræ6†ö÷6R‡G'VRÂ7&VFVBÂ÷F†W"’Ò7&VFVB’F‡&÷ræWr76W'F–öäW'&÷"…Â'G'VR'&æ6‚–FVçF—G•Â"“µÆâ–b‡6W'f–6Ræ6†ö÷6R†fÇ6RÂ7&VFVBÂ÷F†W"’Ò÷F†W"’F‡&÷ræWr76W'F–öäW'&÷"…Â&fÇ6R'&æ6‚–FVçF—G•Â"“µÆâ–b‡6W'f–6Ræ–æ7&VÖVçEVçF–Â‡6W'f–6Ræ7&VFRƒ’Â’Ò’F‡&÷ræWr76W'F–öäW'&÷"…Â'¦W&òÆö÷Â"“µÆâ–b‡6W'f–6Ræ–æ7&VÖVçEVçF–Â‡6W'f–6Ræ7&VFRƒ’Â’Ò’F‡&÷ræWr76W'F–öäW'&÷"…Â&öæRÆö÷Â"“µÆâ–b‡6W'f–6Ræ–æ7&VÖVçEVçF–Â‡6W'f–6Ræ7&VFRƒ’Â’Ò’F‡&÷ræWr76W'F–öäW'&÷"…Â&Öç’Æö÷Â"“µÆâ–b‡6W'f–6Ræ–æ7&VÖVçEVçF–Â‡6W'f–6Ræ7&VFRƒ"’Â’Ò"’F‡&÷ræWr76W'F–öäW'&÷"…Â'7F'B&÷fRÆ–Ö—EÂ"“µÆâ–b‡6W'f–6Rç&÷VæEG&—ƒ2’Ò2’F‡&÷ræWr76W'F–öäW'&÷"…Â&æW7FVB&VfW&Væ6R&W7VÇB&wVÖVçEÂ"“µÆâ7—7FVÒæ÷WBç&–çFÆâ…Â'&VfW&Væ6Rö&¦V7B—2fW&–f–VEÂ"“µÆâÕÆçÕÆâ"À¢¢æW‡V7B‚'w&—FR¦f&VfW&Væ6R’†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWE÷F‚Â"ÖB"Â÷WGWE÷F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2f÷"&VfW&Væ6R’†&æW72"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVB&VfW&Væ6R’†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢%&VfW&Væ6Tö&¦V7D—4†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¥dÒfW&–f–W"f÷"&VfW&Væ6R—2"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVB&VfW&Væ6R—5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢'&VfW&Væ6Rö&¦V7B—2fW&–f–VB ¢“°§Ğ ¦fâfW&–g•÷&VfW&Væ6Uö†öÆFW%÷v—F…ö¦F²†÷WGWEöF—&V7F÷'“¢eF‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWEöF—&V7F÷'’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚6Æ72÷WGWBF‚"“°¢f÷"6Æ75öæÖR–â²$6÷VçFW""Â$†öÆFW""Â$†öÆFW$6Æ–VçB%Ò°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…²"Ö2"Â"×b"Â"×"Â"Ö6Æ77F‚"Â÷WGWE÷F‚Â6Æ75öæÖUÒ¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ff÷"&VfW&Væ6RÖ†öÆFW"6Æ72"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦f&V¦V7FVB¶6Æ75öæÖWÕÆç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢ÆWB÷WGWBÒ7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB“°¢76W'B€¢÷WGWBæ6öçF–ç2‚&Ö¦÷"fW'6–öã¢cR"’À¢'¶6Æ75öæÖWÓ¢¶÷WGWGÒ ¢“°¢76W'B†÷WGWBæ6öçF–ç2‚&FW67&—F÷#¢"’Â'¶6Æ75öæÖWÓ¢¶÷WGWGÒ"“°¢–b6Æ75öæÖRÓÒ$†öÆFW""°¢76W'B†÷WGWBæ6öçF–ç2‚%7F6´ÖF&ÆR"’Â'¶÷WGWGÒ"“°¢f÷"FW67&—F÷"–â°¢"„Ä6÷VçFW#µ¢•b"À¢"‚”Ä6÷VçFW#²"À¢"„Ä6÷VçFW#²•b"À¢"‚”’"À¢"‚•¢"À¢"…¤Ä6÷VçFW#²”Ä6÷VçFW#²"À¢"„’”’"À¢Ò°¢76W'B€¢÷WGWBæ6öçF–ç2‚ff÷&ÖB‚&FW67&—F÷#¢¶FW67&—F÷'Ò"’’À¢'¶÷WGWGÒ ¢“°¢Ğ¢Ğ¢Ğ ¢ÆWB†&æW75÷F‚Ò÷WGWEöF—&V7F÷'’æ¦ö–â‚%&VfW&Væ6T†öÆFW$†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW75÷F‚À¢"2'V&Æ–2f–æÂ6Æ72&VfW&Væ6T†öÆFW$†&æW72°¢V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’°¢6÷VçFW"÷&–v–æÂÒæWr6÷VçFW"ƒ"“°¢6÷VçFW"&WÆ6VÖVçBÒæWr6÷VçFW"ƒr“°¢†öÆFW"†öÆFW"ÒæWr†öÆFW"†÷&–v–æÂÂG'VR“°¢–b††öÆFW"çfÇVR‚’Ò÷&–v–æÂÇÂ†öÆFW"æ–æ—F–Â‚’Ò÷&–v–æÂ¢F‡&÷ræWr76W'F–öäW'&÷"‚&–æ—F–Â–FVçF—G’"“°¢–b††öÆFW"æ—4Væ&ÆVB‚’ÒG'VRÇÂ†öÆFW"ç&VB‚’Ò"¢F‡&÷ræWr76W'F–öäW'&÷"‚&–æ—F–ÂfÇVW2"“°¢–b††öÆFW"æ6†ö÷6R‡G'VRÂ&WÆ6VÖVçB’Ò÷&–v–æÀ¢ÇÂ†öÆFW"æ6†ö÷6R†fÇ6RÂ÷&–v–æÂ’Ò÷&–v–æÂ¢F‡&÷ræWr76W'F–öäW'&÷"‚&'&æ6‚¦ö–â–FVçF—G’"“° ¢†öÆFW"ç&WÆ6R‡&WÆ6VÖVçB“°¢–b††öÆFW"çfÇVR‚’Ò&WÆ6VÖVçBÇÂ†öÆFW"æ–æ—F–Â‚’Ò÷&–v–æÂÇÂ†öÆFW"ç&VB‚’Òr¢F‡&÷ræWr76W'F–öäW'&÷"‚'&WÆ6VÖVçB–FVçF—G’"“°¢–b††öÆFW"æ–æ7&VÖVçEVçF–Âƒ’ÒÇÂ&WÆ6VÖVçBævWB‚’ÒÇÂ÷&–v–æÂævWB‚’Ò"¢F‡&÷ræWr76W'F–öäW'&÷"‚'&WÆ6VÖVçBÆö÷×WFF–öâ÷"ö&¦V7B—6öÆF–öâ"“° ¢†öÆFW$6Æ–VçB6Æ–VçBÒæWr†öÆFW$6Æ–VçB‚“°¢–b†6Æ–VçBçw&†÷&–v–æÂ’çfÇVR‚’Ò÷&–v–æÂ¢F‡&÷ræWr76W'F–öäW'&÷"‚'w&–FVçF—G’"“°¢–b†6Æ–VçBç'VâƒÂ’ÒÇÂ6Æ–VçBç'VâƒÂ’Ò¢ÇÂ6Æ–VçBç'VâƒÂ’ÒÇÂ6Æ–VçBç'Vâƒ"ÂR’Ò"¢F‡&÷ræWr76W'F–öäW'&÷"‚&6Æ–VçBÆö÷&÷VæF'’66W2"“°¢Ğ§Ğ¢"2À¢¢æW‡V7B‚'w&—FR&VfW&Væ6T†öÆFW"¦f†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWE÷F‚Â"ÖB"Â÷WGWE÷F…Ò¢æ&r‚f†&æW75÷F‚¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2f÷"&VfW&Væ6RÖ†öÆFW"†&æW72"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2f–ÆVBf÷"&VfW&Væ6R†öÆFW#¢³£÷Ò"À¢¦f0¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢%&VfW&Væ6T†öÆFW$†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W"f÷"&VfW&Væ6RÖ†öÆFW"†&æW72"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦ff–ÆVBf÷"&VfW&Væ6R†öÆFW#¢³£÷Ò"À¢¦f¢“°§Ğ ¦fâfW&–g•÷&VfW&Væ6Uöf7F÷'•÷v—F…ö¦F²†÷WGWEöF—&V7F÷'“¢eF‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWEöF—&V7F÷'’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚6Æ72÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGWEöF—&V7F÷'’æ¦ö–â‚$f7F÷'”¦F´†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢"2 ¦–×÷'BW†×ÆRä6÷VçFW#°¦–×÷'BW†×ÆRä6÷VçFW$f7F÷'“° §V&Æ–2f–æÂ6Æ72f7F÷'”¦F´†&æW72°¢V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’°¢6÷VçFW"6÷VçFW"ÒæWr6÷VçFW$f7F÷'’‚’æ7&VFRƒr“°¢–b†6÷VçFW"ævWB‚’Òr’F‡&÷ræWr76W'F–öäW'&÷"‚&7&VFR&W7VÇB"“°¢6÷VçFW"ç6WBƒC"“°¢–b†6÷VçFW"ævWB‚’ÒC"’F‡&÷ræWr76W'F–öäW'&÷"‚&f7F÷'’&W7VÇB–FVçF—G’"“°¢Ğ§Ğ¢"2À¢¢æW‡V7B‚'w&—FR¦f#f7F÷'’66WFæ6R†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…°¢"Ò×&VÆV6R"À¢##"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢"ÖB"À¢÷WGWE÷F‚À¢Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2f÷"F†R&VfW&Væ6Rf7F÷'’66WFæ6R†&æW72"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVBF†Rf7F÷'’66WFæ6R†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢$f7F÷'”¦F´†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f#fW&–f–W"f÷"F†R&VfW&Væ6Rf7F÷'’"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVBf7F÷'’÷WGWEÆç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°§Ğ ¦fâfW&–g•÷&VfW&Væ6Uö6öçG&öÅöfÆ÷u÷v—F…ö¦F²†÷WGWEöF—&V7F÷'“¢eF‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWEöF—&V7F÷'’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB†&æW72Ò÷WGWEöF—&V7F÷'’æ¦ö–â‚%&VfW&Væ6T6öçG&öÄfÆ÷t†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ72&VfW&Væ6T6öçG&öÄfÆ÷t†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²6÷VçFW"6÷VçFW"ÒæWr6÷VçFW"ƒ“²–b†6÷VçFW"æ–æ7&VÖVçEVçF–Âƒ2’Ò2ÇÂ6÷VçFW"æ–æ7&VÖVçEVçF–Âƒ’Ò2’F‡&÷ræWr76W'F–öäW'&÷"…Â&f–VÆBÆö÷Â"“²–b†6÷VçFW"ç6WDVæ&ÆVB†fÇ6R’ÒÇÂ6÷VçFW"æVæ&ÆVEVçF–Âƒ‚’Ò2ÇÂ6÷VçFW"ç6WDVæ&ÆVB‡G'VR’Ò2ÇÂ6÷VçFW"æVæ&ÆVEVçF–ÂƒR’ÒR’F‡&÷ræWr76W'F–öäW'&÷"…Â$&ööÆVâf–VÆBfÆ÷uÂ"“²6÷VçFW$6Æ–VçB6Æ–VçBÒæWr6÷VçFW$6Æ–VçB‚“²–b†6Æ–VçBç'Vâƒ’ÒÇÂ6Æ–VçBç'Vâƒ’ÒÇÂ6Æ–VçBç'VâƒR’ÒR’F‡&÷ræWr76W'F–öäW'&÷"…Â'&VfW&Væ6RÆö6ÂÆö÷Â"“²–b†6Æ–VçBæ6†ö÷6R‡G'VR’ÒÇÂ6Æ–VçBæ6†ö÷6R†fÇ6R’Ò"ÇÂ6Æ–VçBæ6†ö÷6TÆö6Â‡G'VR’ÒRÇÂ6Æ–VçBæ6†ö÷6TÆö6Â†fÇ6R’Òb’F‡&÷ræWr76W'F–öäW'&÷"…Â&'&æ6‚ÆÆö6F–öç2æBW†7B¦ö–åÂ"“²–b†6Æ–VçBæ–d–æ7&VÖVçB‡G'VR’ÒÇÂ6Æ–VçBæ–d–æ7&VÖVçB†fÇ6R’Ò’F‡&÷ræWr76W'F–öäW'&÷"…Â&'&æ6‚6ÆÇ5Â"“²–b†6Æ–VçBç'VäæB†fÇ6RÂR’ÒÇÂ6Æ–VçBç'VäæB‡G'VRÂB’ÒB’F‡&÷ræWr76W'F–öäW'&÷"…Â'6†÷'BÖ6—&7V—B&VfW&Væ6RÆö÷Â"“²7—7FVÒæ÷WBç&–çFÆâ…Â'&VfW&Væ6R6öçG&öÂfÆ÷röµÂ"“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FR&VfW&Væ6R6öçG&öÂÖfÆ÷r†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWE÷F‚Â"ÖB"Â÷WGWE÷F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2f÷"&VfW&Væ6R6öçG&öÂÖfÆ÷r†&æW72"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVB&VfW&Væ6R6öçG&öÂÖfÆ÷r†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢%&VfW&Væ6T6öçG&öÄfÆ÷t†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¥dÒfW&–f–W"f÷"&VfW&Væ6R6öçG&öÂÖfÆ÷r6Æ76W2"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVB&VfW&Væ6R6öçG&öÂÖfÆ÷r6Æ76W5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢'&VfW&Væ6R6öçG&öÂfÆ÷rö² ¢“°§Ğ ¦fâfW&–g•ö6÷VçFW%ö6öçG&öÅöfÆ÷u÷v—F…ö¦F²†÷WGWEöF—&V7F÷'“¢eF‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWEöF—&V7F÷'’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Ö2"À¢"×b"À¢"×"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢$6÷VçFW""À¢$6÷VçFW$6Æ–VçB"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ff÷"6÷VçFW"6öçG&öÂÖfÆ÷rf—‡GW&R"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦f&V¦V7FVB6÷VçFW"6öçG&öÂÖfÆ÷r6Æ76W5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢ÆWB÷WGWBÒ7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB“°¢f÷"Wf–FVæ6R–â°¢&Ö¦÷"fW'6–öã¢cR"À¢'&—fFR–çB7W'&VçB"À¢&ÆöEó"À¢&vWFf–VÆB"À¢'WFf–VÆB"À¢&–ÆöEó"À¢&—7F÷&Uò"À¢&–eö–6×"À¢&–fW"À¢&v÷Fò"À¢&ÆöEó""À¢&–çfö¶Wf—'GVÂ"À¢%7F6´ÖF&ÆR"À¢$6÷VçFW"åÂ#Æ–æ—CåÂ#¢„’•b"À¢$6÷VçFW"ævWC¢‚”’"À¢$6÷VçFW"æ–æ7&VÖVçC¢‚”’"À¢Ò°¢76W'B€¢÷WGWBæ6öçF–ç2†Wf–FVæ6R’À¢&¦f÷WGWBöÖ—GFVB¶Wf–FVæ6S£÷Ó¥Æç¶÷WGWGÒ ¢“°¢Ğ ¢ÆWB†&æW72Ò÷WGWEöF—&V7F÷'’æ¦ö–â‚$6÷VçFW$6öçG&öÄfÆ÷t†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–2f–æÂ6Æ726÷VçFW$6öçG&öÄfÆ÷t†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²6÷VçFW$6Æ–VçB6Æ–VçBÒæWr6÷VçFW$6Æ–VçB‚“²–b†6Æ–VçBç'Vâƒ’Ò’F‡&÷ræWr76W'F–öäW'&÷"…Â''Vâ¦W&õÂ"“²–b†6Æ–VçBç'Vâƒ’Ò’F‡&÷ræWr76W'F–öäW'&÷"…Â''VâöæUÂ"“²–b†6Æ–VçBç'Vâƒ’Ò’F‡&÷ræWr76W'F–öäW'&÷"…Â''Vâ×VÇF—ÆUÂ"“²–b†6Æ–VçBæ6†ö÷6R‡G'VR’Ò’F‡&÷ræWr76W'F–öäW'&÷"…Â&6†ö÷6RG'VUÂ"“²–b†6Æ–VçBæ6†ö÷6R†fÇ6R’Ò#’F‡&÷ræWr76W'F–öäW'&÷"…Â&6†ö÷6RfÇ6UÂ"“²–b†æWr6÷VçFW"ƒ"’æ–æ7&VÖVçEVçF–ÂƒR’ÒR’F‡&÷ræWr76W'F–öäW'&÷"…Â&–æ7&VÖVçBFòÆ–Ö—EÂ"“²–b†æWr6÷VçFW"ƒR’æ–æ7&VÖVçEVçF–ÂƒR’ÒR’F‡&÷ræWr76W'F–öäW'&÷"…Â&Ç&VG’BÆ–Ö—EÂ"“²–b†æWr6÷VçFW"ƒb’æ–æ7&VÖVçEVçF–Âƒ2’Òb’F‡&÷ræWr76W'F–öäW'&÷"…Â'7F'B÷fW"Æ–Ö—EÂ"“²7—7FVÒæ÷WBç&–çFÆâ…Â&6÷VçFW"6öçG&öÂfÆ÷rfW&–f–VEÂ"“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FR6÷VçFW"6öçG&öÂÖfÆ÷r¦f†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWE÷F‚Â"ÖB"Â÷WGWE÷F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2f÷"6÷VçFW"6öçG&öÂÖfÆ÷r†&æW72"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVB6÷VçFW"6öçG&öÂÖfÆ÷r†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢$6÷VçFW$6öçG&öÄfÆ÷t†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¥dÒfW&–f–W"f÷"6÷VçFW"6öçG&öÂÖfÆ÷r6Æ76W2"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVB6÷VçFW"6öçG&öÂÖfÆ÷r6Æ76W5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢&6÷VçFW"6öçG&öÂfÆ÷rfW&–f–VB ¢“°§Ğ ¦fâfW&–g•ö6÷VçFW%öö&¦V7G5÷v—F…ö¦F²†÷WGWEöF—&V7F÷'“¢eF‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWEöF—&V7F÷'’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"×"À¢"Ö2"À¢"×b"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢$6÷VçFW""À¢$6÷VçFW$6Æ–VçB"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ff÷"&÷F‚6÷VçFW$ö&¦V7G26Æ76W2"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦f&V¦V7FVB6÷VçFW$ö&¦V7G2÷WGWG5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢ÆWB¦fö÷WGWBÒ7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB“°¢f÷"W‡V7FVB–â°¢&Ö¦÷"fW'6–öã¢cR"À¢'&—fFR–çB7W'&VçC²"À¢'V&Æ–26÷VçFW"†–çB“²"À¢'V&Æ–2–çB–æ7&VÖVçB‚“²"À¢'V&Æ–2–çBvWB‚“²"À¢'V&Æ–26÷VçFW$6Æ–VçB‚“²"À¢'V&Æ–2–çB'Vâ†–çB“²"À¢'V&Æ–2–çBF—&V7B‚“²"À¢'V&Æ–2–çB–æFWVæFVçB‚“²"À¢&FW67&—F÷#¢„’•b"À¢&FW67&—F÷#¢„’”’"À¢Ò°¢76W'B€¢¦fö÷WGWBæ6öçF–ç2†W‡V7FVB’À¢&¦f÷WGWBöÖ—GFVB¶W‡V7FVC£÷Ó¥Æç¶¦fö÷WGWGÒ ¢“°¢Ğ¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢f¦fö÷WGWBÀ¢'V&Æ–2–çB'Vâ†–çB“²"À¢e°¢&æWr"À¢&GW"À¢&–ÆöEó"À¢&–çfö¶W7V6–Â"À¢&7F÷&Uó""À¢&ÆöEó""À¢&–çfö¶Wf—'GVÂ"À¢'÷"À¢&ÆöEó""À¢&–çfö¶Wf—'GVÂ"À¢&—&WGW&â"À¢ÒÀ¢“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢f¦fö÷WGWBÀ¢'V&Æ–2–çBF—&V7B‚“²"À¢e°¢&æWr"À¢&GW"À¢&&—W6‚"À¢&–çfö¶W7V6–Â"À¢&–çfö¶Wf—'GVÂ"À¢&—&WGW&â"À¢ÒÀ¢“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢f¦fö÷WGWBÀ¢'V&Æ–2–çB–æFWVæFVçB‚“²"À¢e°¢&æWr"À¢&GW"À¢&&—W6‚"À¢&–çfö¶W7V6–Â"À¢&7F÷&Uó"À¢&æWr"À¢&GW"À¢&&—W6‚"À¢&–çfö¶W7V6–Â"À¢&7F÷&Uó""À¢&ÆöEó"À¢&–çfö¶Wf—'GVÂ"À¢'÷"À¢&ÆöEó"À¢&–çfö¶Wf—'GVÂ"À¢&ÆöEó""À¢&–çfö¶Wf—'GVÂ"À¢&–FB"À¢&—&WGW&â"À¢ÒÀ¢“° ¢ÆWB†&æW75÷F‚Ò÷WGWEöF—&V7F÷'’æ¦ö–â‚$6÷VçFW$ö&¦V7G4†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW75÷F‚À¢'V&Æ–2f–æÂ6Æ726÷VçFW$ö&¦V7G4†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²6÷VçFW$6Æ–VçB6Æ–VçBÒæWr6÷VçFW$6Æ–VçB‚“²–b†6Æ–VçBç'Vâƒ’Ò’F‡&÷ræWr76W'F–öäW'&÷"…Â''Vâƒ•Â"“²–b†6Æ–VçBç'Vâƒ’Ò’F‡&÷ræWr76W'F–öäW'&÷"…Â''Vâƒ•Â"“²–b†6Æ–VçBæF—&V7B‚’ÒC"’F‡&÷ræWr76W'F–öäW'&÷"…Â&F—&V7EÂ"“²–b†6Æ–VçBæ–æFWVæFVçB‚’Ò3’F‡&÷ræWr76W'F–öäW'&÷"…Â&–æFWVæFVçB–ç7Fæ6W5Â"“²7—7FVÒæ÷WBç&–çFÆâ…Â&6÷VçFW"ö&¦V7G2öµÂ"“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FR6÷VçFW$ö&¦V7G2¦f†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWE÷F‚Â"ÖB"Â÷WGWE÷F…Ò¢æ&r‚f†&æW75÷F‚¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2f÷"6÷VçFW$ö&¦V7G2"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVB6÷VçFW$ö&¦V7G2†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢$6÷VçFW$ö&¦V7G4†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W"f÷"6÷VçFW$ö&¦V7G2"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVB6÷VçFW$ö&¦V7G5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢&6÷VçFW"ö&¦V7G2ö² ¢“°§Ğ ¦fâfW&–g•÷&—fFUöf–VÆG5÷v—F…ö¦F²†÷WGWEöF—&V7F÷'“¢eF‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWEöF—&V7F÷'’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚FV×÷&'’F‚"“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…²"×"Â"Ö2"Â"×b"Â"Ö6Æ77F‚"Â÷WGWE÷F‚Â%&—fFTf–VÆG2%Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ff÷"&—fFTf–VÆG2"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦f&V¦V7FVB&—fFTf–VÆG3¢³£÷Ò"À¢¦f ¢“°¢ÆWB¦fö÷WGWBÒ7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB“°¢f÷"W‡V7FVB–â°¢&Ö¦÷"fW'6–öã¢cR"À¢'&—fFRf–æÂ–çB6VVC²"À¢'&—fFRf–æÂ&ööÆVâVæ&ÆVC²"À¢'&—fFRf–æÂ&ööÆVâF—6&ÆVC²"À¢&FW67&—F÷#¢‚•b"À¢&FW67&—F÷#¢‚”’"À¢&FW67&—F÷#¢‚•¢"À¢&vWFf–VÆB"À¢Ò°¢76W'B€¢¦fö÷WGWBæ6öçF–ç2†W‡V7FVB’À¢&¦f÷WGWBöÖ—GFVB¶W‡V7FVC£÷Ó¥Æç¶¦fö÷WGWGÒ ¢“°¢Ğ ¢ÆWB†&æW75÷F‚Ò÷WGWEöF—&V7F÷'’æ¦ö–â‚%&—fFTf–VÆG4†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW75÷F‚À¢'V&Æ–2f–æÂ6Æ72&—fFTf–VÆG4†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²&—fFTf–VÆG2fÇVRÒæWr&—fFTf–VÆG2‚“²–b‡fÇVRç6VVEfÇVR‚’ÒC"’F‡&÷ræWr76W'F–öäW'&÷"…Â'6VVEÂ"“²–b‚fÇVRæVæ&ÆVEfÇVR‚’’F‡&÷ræWr76W'F–öäW'&÷"…Â&&ööÆVâG'VUÂ"“²–b‡fÇVRæF—6&ÆVEfÇVR‚’’F‡&÷ræWr76W'F–öäW'&÷"…Â&&ööÆVâfÇ6UÂ"“²ÒÕÆâ"À¢¢æW‡V7B‚'w&—FR&—fFTf–VÆG2¦f†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWE÷F‚Â"ÖB"Â÷WGWE÷F…Ò¢æ&r‚f†&æW75÷F‚¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2f÷"&—fFTf–VÆG2"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2f–ÆVBf÷"&—fFTf–VÆG3¢³£÷Ò"À¢¦f0¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢%&—fFTf–VÆG4†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W"f÷"&—fFTf–VÆG2"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦ff–ÆVBf÷"&—fFTf–VÆG3¢³£÷Ò"À¢¦f¢“°§Ğ ¢5·FW7EĞ¦fâV&Æ–5ö&öG•ö66W76÷'5ö–æ—F–Æ—¦UöæEöVÖ—E÷fW&–f–VEövWGFW'5öæE÷6WGFW'2‚’°¢ÆWB6÷W&6U÷FW‡BÒ&6Æ72&öG”öæÇ“¥ÆâfÂ6VVC¢–çBÒÆâf"Væ&ÆVC¢&ööÆVâÒG'VUÆâfÂ&F–ó¢F÷V&ÆRÒãUÆâf"66÷&S¢fÆöBÒÓ"ã#VeÆâf"&FS¢F÷V&ÆRÒãUÆåÆæ6Æ726÷VçFW"‡fÂ–C¢–çB“¥ÆâfÂ–æ—F–Ã¢–çBÒÆâf"7W'&VçC¢ÆöærÒÅÆâ&—fFRfÂ6÷–VC¢–çBÒ–æ—F–ÅÆåÆæ6Æ72†öÆFW"‡fÇVS¢6÷VçFW"“¥Æâf"6VÆV7FVC¢6÷VçFW"ÒfÇVR#°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'V&Æ–2Ö&öG’Öf–VÆBÖ–æ—F–Æ—¦F–öâ"“°¢ÆWB×WBö'6W'fVBÒfÇ6S°¢ÆWB×WBö'6W'fRÒÆ7C¢d7D&VæÅVçG—VCâÀ¢ó¢G&VT–CÅVçG—VCâÀ¢–æFWƒ¢e6÷W&6U6VÖçF–4–æFW‚À¢ó¢e6÷W&6UG—VD–æFW‚À¢Væ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÀ¢FVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ö'6W'fVBÒG'VS°¢Væ—BçfÆ–FFR‚¢æW‡V7B‚'F†R6÷W&6R×&÷fVâV&Æ–2&öG’f–VÆBÆç2fÆ–FFR"“°¢ÆWBÖWFFFÒVæ—@¢æÖWFFF†FVf–æ—F–öç2¢æW‡V7B‚'F†RV&Æ–2f–VÆG2&ö¦V7BFò6†V6¶VB&—fFR&6¶–ærÖWFFF"“° ¢f÷"€¢6Æ75öæÖRÀ¢W‡V7FVEö&öG•öf–VÆG2À¢W‡V7FVE÷6÷W&6Uöf–VÆG2À¢W‡V7FVE÷&—fFUöf–VÆG2À¢W‡V7FVEö6öç7G'V7F÷%ö66W76÷'2À¢’–â°¢€¢$&öG”öæÇ’"À¢e²'6VVB"Â&Væ&ÆVB"Â'&F–ò"Â'66÷&R"Â'&FR%Õ²âåÒÀ¢e²'6VVB"Â&Væ&ÆVB"Â'&F–ò"Â'66÷&R"Â'&FR%Õ²âåÒÀ¢À¢À¢’À¢€¢$6÷VçFW""À¢e²&–æ—F–Â"Â&7W'&VçB%Õ²âåÒÀ¢e²&–æ—F–Â"Â&7W'&VçB"Â&6÷–VB%Õ²âåÒÀ¢À¢À¢’À¢‚$†öÆFW""Âe²'6VÆV7FVB%Õ²âåÒÂe²'6VÆV7FVB%Õ²âåÒÂÂ’À¢Ò°¢ÆWB6Æ72ÒVF—Eö6Æ75öæÖVB‡Væ—BÂ6Æ75öæÖR“°¢ÆWBÆâÒVæ—@¢æ6Æ76W0¢æ—FW"‚¢æf–æB‡ÇÆçÂÆâæ6Æ72ÓÒ6Æ72¢æW‡V7B‚'6÷W&6R6Æ72†2Æâ"“°¢–b6Æ75öæÖRÓÒ$&öG”öæÇ’"°¢76W'B†ÖF6†W2€¢Æâæ–æ—F–Æ—¦F–öâÀ¢§fÔ6Æ74–æ—F–Æ—¦F–öåÆã£¤–×Æ–6—Dæô&p¢’“°¢ÒVÇ6R°¢76W'B†ÖF6†W2€¢Æâæ–æ—F–Æ—¦F–öâÀ¢§fÔ6Æ74–æ—F–Æ—¦F–öåÆã£¥&–Ö'”6öç7G'V7F÷"…ò¢’“°¢Ğ¢76W'EöW‡Æâæf–VÆG2æÆVâ‚’ÂW‡V7FVE÷6÷W&6Uöf–VÆG2æÆVâ‚’“°¢76W'EöW€¢Æâæf–VÆG0¢æ—FW"‚¢æÖ‡Æf–VÆGÂVæ—@¢ç7F÷&P¢ææÖW0¢ç&W6öÇfR‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB†f–VÆBæf–VÆB’ææÖRçFW‡B‚’¢çFõö÷væVB‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢W‡V7FVE÷6÷W&6Uöf–VÆG0¢“°¢ÆWB&öG•öf–VÆE÷Æç2ÒÆà¢æf–VÆG0¢æ—FW"‚¢æf–ÇFW"‡Æf–VÆGÂ°¢ÖF6†W2€¢f–VÆBç6÷W&6U÷f—6–&–Æ—G’À¢§fÔf–VÆE6÷W&6Uf—6–&–Æ—G“£¥Æ–åV&Æ–2…ò¢¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW€¢&öG•öf–VÆE÷Æç0¢æ—FW"‚¢æÖ‡Æf–VÆGÂ°¢ÆWBf–VÆBÒ¦f–VÆC°¢ÆWB§fÔf–VÆE6÷W&6Uf—6–&–Æ—G“£¥Æ–åV&Æ–2‡&ööb’Òf–VÆBç6÷W&6U÷f—6–&–Æ—G¢VÇ6R°¢æ–2‚&÷&F–æ'’V&Æ–2&öG’f–VÆB6'&–W2—G26†V6¶VB6÷W&6R&ööb"“°¢Ó°¢76W'EöW‡&ööbæ6Æ72‚’Â6Æ72“°¢76W'EöW‡&ööbæf–VÆB‚’Âf–VÆBæf–VÆB“°¢ÆWB7–Ö&öÂÒVæ—Bç7F÷&Rç7–Ö&öÇ2ævWB†f–VÆBæf–VÆB“°¢76W'EöW‡7–Ö&öÂæ¶–æBÂ7–Ö&öÄ¶–æC£¤f–VÆB“°¢76W'EöW‡7–Ö&öÂæ÷væW"Â6öÖR†6Æ72’“°¢76W'EöW‡7–Ö&öÂçf—6–&–Æ—G’Âf—6–&–Æ—G“£¥V&Æ–2“°¢ÆWBF÷GG•ö6÷&S£¥7–Ö&öÄ–æfó£¤6ö×ÆWFR…öf–VÆE÷G—R’Ò7–Ö&öÂæ–æfòVÇ6R°¢æ–2‚'V&Æ–2&öG’f–VÆBG—R6†÷VÆB&R6ö×ÆWFR"“°¢Ó°¢76W'EöW‡&ööbæ×WF&ÆR‚’Â7–Ö&öÂæfÆw2æ6öçF–ç2…7–Ö&öÄfÆw3£¤ÕUD$ÄR’“°¢ÆWB–æ—F–Æ—¦W"Òf–VÆBæ–æ—F–Æ—¦W"æW‡V7B‚'6÷W&6Rf–VÆB†2â$…2"“°¢ÆWB6÷W&6UöFVf–æ—F–öâÒ–æFW€¢æFVf–æ—F–öåööb†f–VÆBæf–VÆB¢æW‡V7B‚&&öG’f–VÆB†26æöæ–6Â6÷W&6R&÷fVææ6R"“°¢ÆWB6÷W&6TFVf–æ—F–öã£¤6æöæ–6Â²6÷W&6RÂG&VRÒÒ6÷W&6UöFVf–æ—F–öâVÇ6R°¢æ–2‚&6Æ72Ö&öG’f–VÆB×W7B¶VW—G26æöæ–6Â6÷W&6RG&VR"“°¢Ó°¢76W'EöW€¢6÷W&6RÀ¢6÷W&6T–C£¦g&öÕö–æFW‚ƒCs’’À¢&6æöæ–6Â6÷W&6R–FVçF—G’&VÆöæw2FòF†—26÷W&6RVæ—B ¢“°¢ÆWBG&VT¶–æC£¥fÄFVb†FVf–æ—F–öâ’Òf7BævWB‡G&VR’æ¶–æBVÇ6R°¢æ–2‚&f–VÆB&÷fVææ6Rö–çG2Fò—G26÷W&6RfÄFVb"“°¢Ó°¢76W'EöW€¢&ööbæ×WF&ÆR‚’À¢FVf–æ—F–öà¢æÖWFFF¢æÖöF–f–W'0¢æ6öçF–ç2‚fF÷GG•ö6÷&S£¦7C£¤ÖöF–f–W#£¥f"¢“°¢76W'B†FVf–æ—F–öâæÖWFFFçf—6–&–Æ—G’æ—5öæöæR‚’“°¢ÆWB&‡2ÒFVf–æ—F–öâç&‡2æW‡V7B‚'6÷W&6Rf–VÆB†2â$…2"“°¢76W'EöW‡&ööbç6÷W&6R‚’Â6÷W&6R“°¢76W'EöW‡&ööbæFV6Æ&F–öâ‚’ÂG&VR“°¢76W'EöW€¢&ööbçFV×ÆFU÷÷6—F–öâ‚’À¢W‡V7FVEö&öG•öf–VÆG0¢æ—FW"‚¢ç÷6—F–öâ€¢ÆæÖWÂ¦æÖRÓÒVæ—Bç7F÷&RææÖW2ç&W6öÇfR‡7–Ö&öÂææÖRçFW‡B‚’¢¢çVçw&‚¢“°¢76W'EöW‡&ööbç&‡2‚’Â&‡2“°¢76W'EöW‡&ööbæ–æ—F–Æ—¦W"‚’Â–æ—F–Æ—¦W"“°¢Væ—Bç7F÷&RææÖW2ç&W6öÇfR‡7–Ö&öÂææÖRçFW‡B‚’’çFõö÷væVB‚¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢W‡V7FVEö&öG•öf–VÆG0¢“° ¢ÆWB6Æ75öÖWFFFÒÖWFFF¢æ—FW"‚¢æf–æB‡ÆÖWFFFÂÖWFFFæ–FVçF—G’‚’ç7–Ö&öÂ‚’ÓÒ6Æ72¢æW‡V7B‚&6Æ72†2fÆ–FFVBÖWFFF"“°¢76W'EöW†6Æ75öÖWFFFæf–VÆG2‚’æÆVâ‚’ÂW‡V7FVE÷&—fFUöf–VÆG2“°¢76W'EöW€¢6Æ75öÖWFFFæ66W76÷'2‚’æÆVâ‚’À¢W‡V7FVEö&öG•öf–VÆG2æÆVâ‚’²W‡V7FVEö6öç7G'V7F÷%ö66W76÷'0¢“°¢–bW‡V7FVEö6öç7G'V7F÷%ö66W76÷'2ÓÒ°¢76W'B†6Æ75öÖWFFFæ66W76÷'2‚’æ—FW"‚’æç’‡Æ66W76÷'Â°¢ÖF6†W2€¢66W76÷"æ÷&–v–â‚’À¢F÷GG•ö§fÓ£¤§fÔ66W76÷$÷&–v–ã£¤6öç7G'V7F÷%&ÖWFW"…ò¢¢Ò’“°¢Ğ¢f÷"f–VÆE÷Æâ–âf&öG•öf–VÆE÷Æç2°¢ÆWBf–VÆE÷ÆâÒ¦f–VÆE÷Æã°¢ÆWB66W76÷"Ò6Æ75öÖWFFF¢æ66W76÷'2‚¢æ—FW"‚¢æf–æB‡Æ66W76÷'Â66W76÷"ç6÷W&6Uöf–VÆB‚’ÓÒf–VÆE÷Æâæf–VÆB¢æW‡V7B‚&V6‚6÷W&6R&öG’f–VÆB†2â66W76÷"&ö¦V7F–öâ"“°¢ÆWBW‡V7FVEöFW67&—F÷"ÒÖF6‚Væ—@¢ç7F÷&P¢ææÖW0¢ç&W6öÇfR‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB†f–VÆE÷Æâæf–VÆB’ææÖRçFW‡B‚’¢°¢'6VVB"Óâ$’"À¢&Væ&ÆVB"Óâ%¢"À¢'&F–ò"Óâ$B"À¢'66÷&R"Óâ$b"À¢'&FR"Óâ$B"À¢&–æ—F–Â"Óâ$’"À¢&7W'&VçB"Óâ$¢"À¢'6VÆV7FVB"Óâ$Ä6÷VçFW#²"À¢òÓâæ–2‚'6÷W&6Rf–VÆB†26÷fW&VB&–Ö—F—fRG—R"’À¢Ó°¢76W'EöW†66W76÷"æf–VÆEöFW67&—F÷"‚’ÂW‡V7FVEöFW67&—F÷"“°¢76W'B†ÖF6†W2€¢66W76÷"æ÷&–v–â‚’À¢F÷GG•ö§fÓ£¤§fÔ66W76÷$÷&–v–ã£¤6Æ74&öG”f–VÆB‡&ööb¢–b&ööbæ–æ—F–Æ—¦W"‚’ÓÒf–VÆE÷Æâæ–æ—F–Æ—¦W"çVçw&‚¢’“°¢76W'B€¢66W76÷ ¢æf–VÆEö66W75öfÆw2‚¢æ6öçF–ç2†F÷GG•ö6Æ76f–ÆS£¦66W75öfÆw3£¤45õ$•dDR¢“°¢76W'EöW€¢66W76÷ ¢æf–VÆEö66W75öfÆw2‚¢æ6öçF–ç2†F÷GG•ö6Æ76f–ÆS£¦66W75öfÆw3£¤45ôd”äÂ’À¢Væ—@¢ç7F÷&P¢ç7–Ö&öÇ0¢ævWB†f–VÆE÷Æâæf–VÆB¢æfÆw0¢æ6öçF–ç2…7–Ö&öÄfÆw3£¤ÕUD$ÄR¢“°¢76W'EöW€¢66W76÷"ævWGFW"‚’ææÖR‚’À¢Væ—Bç7F÷&P¢ææÖW0¢ç&W6öÇfR‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB†f–VÆE÷Æâæf–VÆB’ææÖRçFW‡B‚’¢“°¢76W'EöW€¢66W76÷"ævWGFW"‚’æFW67&—F÷"‚’æ5÷7G"‚’À¢ff÷&ÖB‚"‚—¶W‡V7FVEöFW67&—F÷'Ò"¢“°¢76W'EöW€¢66W76÷"ç6WGFW"‚’æ—5÷6öÖR‚’À¢Væ—Bç7F÷&P¢ç7–Ö&öÇ0¢ævWB†f–VÆE÷Æâæf–VÆB¢æfÆw0¢æ6öçF–ç2…7–Ö&öÄfÆw3£¤ÕUD$ÄR¢“°¢–bÆWB6öÖR‡6WGFW"’Ò66W76÷"ç6WGFW"‚’°¢76W'EöW‡6WGFW"ææÖR‚’Âff÷&ÖB‚'·ÕòFW"Â66W76÷"æf–VÆEöæÖR‚’’“°¢76W'EöW€¢6WGFW"æFW67&—F÷"‚’æ5÷7G"‚’À¢ff÷&ÖB‚"‡¶W‡V7FVEöFW67&—F÷'Ò•b"¢“°¢76W'B‡6WGFW"æ66W75öfÆw2‚’æ—5÷V&Æ–2‚’“°¢Ğ¢Ğ¢ÆWB–æ—F–Æ—¦W'2ÒVæ—@¢æÆ÷vW%ö6öç7G'V7F÷%öf–VÆEö–æ—F–Æ—¦W'2†6Æ72ÂFVf–æ—F–öç2¢æW‡V7B‚'V&Æ–2&öG’f–VÆG2Æ÷vW"F‡&÷Vv‚F†V—"6†V6¶VBG—VB$…2&öög2"“°¢76W'EöW†–æ—F–Æ—¦W'2æf–VÆG2‚’æÆVâ‚’ÂW‡V7FVE÷6÷W&6Uöf–VÆG2æÆVâ‚’“°¢76W'EöW€¢–æ—F–Æ—¦W'0¢æf–VÆG2‚¢æ—FW"‚¢æÖ‡Æ–æ—F–Æ—¦W'Â–æ—F–Æ—¦W"æf–VÆB‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢Æâæf–VÆG0¢æ—FW"‚¢æÖ‡Æf–VÆGÂf–VÆBæf–VÆB¢æ6öÆÆV7C££ÅfV3Åóãâ‚¢“°¢76W'EöW€¢–æ—F–Æ—¦W'0¢æf–VÆG2‚¢æ—FW"‚¢æÖ‡Æ–æ—F–Æ—¦W'Â°¢ÆWB÷&–v–âÒÖF6‚Æà¢æf–VÆG0¢æ—FW"‚¢æf–æB‡Æf–VÆGÂf–VÆBæf–VÆBÓÒ–æ—F–Æ—¦W"æf–VÆB‚’¢æW‡V7B‚&–æ—F–Æ—¦W"†2—G26÷W&6Rf–VÆBÆâ"¢ç6÷W&6U÷f—6–&–Æ—G¢°¢§fÔf–VÆE6÷W&6Uf—6–&–Æ—G“£¥Æ–åV&Æ–2…ò’Óâ°¢F÷GG•ö§fÓ£¤§fÔ6öç7G'V7F÷$f–VÆD–æ—F–Æ—¦W$÷&–v–ã£¤6Æ74&öG”66W76÷ ¢Ğ¢§fÔf–VÆE6÷W&6Uf—6–&–Æ—G“£¥Æ–å&—fFRÓâ°¢F÷GG•ö§fÓ£¤§fÔ6öç7G'V7F÷$f–VÆD–æ—F–Æ—¦W$÷&–v–ã£¥&—fFTf–VÆ@¢Ğ¢§fÔf–VÆE6÷W&6Uf—6–&–Æ—G“£¥&—fFUF†—2Óâ°¢æ–2‚'&—fFU·F†—5Òf–VÆG2&R÷WG6–FR–æ—F–Æ—¦F–öâ66÷R"¢Ğ¢Ó°¢76W'EöW†–æ—F–Æ—¦W"æ÷&–v–â‚’Â÷&–v–â“°¢Væ—Bç7F÷&P¢ææÖW0¢ç&W6öÇfR‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB†–æ—F–Æ—¦W"æf–VÆB‚’’ææÖRçFW‡B‚’¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢W‡V7FVE÷6÷W&6Uöf–VÆG0¢“° ¢–bW‡V7FVEö&öG•öf–VÆG2æÆVâ‚’â°¢ÆWB6Æ75ö–æFW‚ÒVæ—@¢æ6Æ76W0¢æ—FW"‚¢ç÷6—F–öâ‡Æ6æF–FFWÂ6æF–FFRæ6Æ72ÓÒ6Æ72¢æW‡V7B‚'6÷W&6R6Æ72†2×WF&ÆRÆâ"“°¢Væ—Bæ6Æ76W5¶6Æ75ö–æFW…Òæf–VÆG2ç7vƒÂ“°¢76W'B†ÖF6†W2€¢Væ—BçfÆ–FFR‚’À¢W'"„§fÔ6öFVvVäW'&÷#£¤÷WDöd÷&FW%V&Æ–4&öG”f–VÆEÆâ°¢6Æ73¢f÷VæEö6Æ72À¢âà¢Ò’–bf÷VæEö6Æ72ÓÒ6Æ70¢’“°¢Væ—Bæ6Æ76W5¶6Æ75ö–æFW…Òæf–VÆG2ç7vƒÂ“°¢Væ—BçfÆ–FFR‚¢æW‡V7B‚'&W7F÷&–ær6÷W&6R÷&FW"&W7F÷&W2F†RfÆ–FFVBÆâ"“°¢Ğ ¢–b6Æ75öæÖRÓÒ$6÷VçFW""°¢ÆWB6Æ75ö–æFW‚ÒVæ—@¢æ6Æ76W0¢æ—FW"‚¢ç÷6—F–öâ‡Æ6æF–FFWÂ6æF–FFRæ6Æ72ÓÒ6Æ72¢æW‡V7B‚$6÷VçFW"†2×WF&ÆR6Æ72Æâ"“°¢ÆWB&—fFUö–æFW‚ÒVæ—Bæ6Æ76W5¶6Æ75ö–æFW…Ğ¢æf–VÆG0¢æ—FW"‚¢ç÷6—F–öâ‡Æf–VÆGÂ°¢ÖF6†W2€¢f–VÆBç6÷W&6U÷f—6–&–Æ—G’À¢§fÔf–VÆE6÷W&6Uf—6–&–Æ—G“£¥Æ–å&—fFP¢¢Ò¢æW‡V7B‚$6÷VçFW"–æ6ÇVFW2&—fFR&öG’f–VÆB"“°¢Væ—Bæ6Æ76W5¶6Æ75ö–æFW…Òæf–VÆG2ç7vƒÂ&—fFUö–æFW‚“°¢76W'B†ÖF6†W2€¢Væ—BçfÆ–FFR‚’À¢W'"„§fÔ6öFVvVäW'&÷#£¤÷WDöd÷&FW$f–VÆEÆâ°¢6Æ73¢f÷VæEö6Æ72À¢âà¢Ò’–bf÷VæEö6Æ72ÓÒ6Æ70¢’“°¢Væ—Bæ6Æ76W5¶6Æ75ö–æFW…Òæf–VÆG2ç7vƒÂ&—fFUö–æFW‚“°¢Væ—BçfÆ–FFR‚¢æW‡V7B‚'&W7F÷&–ærÖ—†VBf—6–&–Æ—G’f–VÆB÷&FW"&W7F÷&W2F†RÆâ"“°¢Ğ ¢–b6Æ75öæÖRÓÒ$&öG”öæÇ’"°¢ÆWB7W'&VçE÷ÆâÒVæ—@¢æ6Æ76W0¢æ—FW"‚¢æf–æB‡Æ6æF–FFWÂ6æF–FFRæ6Æ72ÓÒ6Æ72¢æW‡V7B‚'6÷W&6R6Æ72†2ÆâgFW"÷&FW"fÆ–FF–öâ"“°¢ÆWBf—'7Eöf–VÆBÒ7W'&VçE÷Æâæf–VÆG5³Ó°¢ÆWB6V6öæEöf–VÆBÒ7W'&VçE÷Æâæf–VÆG5³Ó°¢ÆWB§fÔf–VÆE6÷W&6Uf—6–&–Æ—G“£¥Æ–åV&Æ–2†f—'7E÷&ööb’Ğ¢f—'7Eöf–VÆBç6÷W&6U÷f—6–&–Æ—G¢VÇ6R°¢æ–2‚'F†Rf—'7BV&Æ–2f–VÆB6'&–W26†V6¶VB6÷W&6RWf–FVæ6R"“°¢Ó°¢ÆWB§fÔf–VÆE6÷W&6Uf—6–&–Æ—G“£¥Æ–åV&Æ–2‡6V6öæE÷&ööb’Ğ¢6V6öæEöf–VÆBç6÷W&6U÷f—6–&–Æ—G¢VÇ6R°¢æ–2‚'F†R6V6öæBV&Æ–2f–VÆB6'&–W26†V6¶VB6÷W&6RWf–FVæ6R"“°¢Ó°¢ÆWB×WBf÷&vVEö–æFW‚ÒF÷GG•ö6÷&S£¥6÷W&6UG—VD–æFWƒ£¦æWr‚“°¢f÷&vVEö–æFW€¢æ–ç6W'B€¢f—'7E÷&ööbç6÷W&6R‚’À¢f—'7E÷&ööbç&‡2‚’À¢6V6öæE÷&ööbæ–æ—F–Æ—¦W"‚’À¢¢æW‡V7B‚'F†Rg&W6‚6÷W&6RÖ6â†öÆBF†Rf'&–6FVB&–æF–ær"“°¢76W'B†ÖF6†W2€¢§fÕV&Æ–4&öG”f–VÆE6÷W&6U&ööc£¦g&öÕ÷6÷W&6R€¢6Æ72À¢f—'7Eöf–VÆBæf–VÆBÀ¢f—'7E÷&ööbç6÷W&6R‚’À¢f—'7E÷&ööbæFV6Æ&F–öâ‚’À¢6V6öæE÷&ööbæ–æ—F–Æ—¦W"‚’À¢7BÀ¢–æFW‚À¢ff÷&vVEö–æFW‚À¢Væ—BçG—VEö7BÀ¢Væ—Bç7F÷&RÀ¢’À¢W'"„§fÔ6öFVvVäW'&÷#£¤–çfÆ–EV&Æ–4&öG”f–VÆE6÷W&6U&ööb²f–VÆBÒ¢–bf–VÆBÓÒf—'7Eöf–VÆBæf–VÆ@¢’“°¢76W'B†ÖF6†W2€¢§fÕV&Æ–4&öG”f–VÆE6÷W&6U&ööc£¦g&öÕ÷6÷W&6R€¢6Æ72À¢f—'7Eöf–VÆBæf–VÆBÀ¢f—'7E÷&ööbç6÷W&6R‚’À¢f—'7E÷&ööbæFV6Æ&F–öâ‚’À¢f—'7E÷&ööbæ–æ—F–Æ—¦W"‚’À¢7BÀ¢–æFW‚À¢fF÷GG•ö6÷&S£¥6÷W&6UG—VD–æFWƒ£¦æWr‚’À¢Væ—BçG—VEö7BÀ¢Væ—Bç7F÷&RÀ¢’À¢W'"„§fÔ6öFVvVäW'&÷#£¤–çfÆ–EV&Æ–4&öG”f–VÆE6÷W&6U&ööb²f–VÆBÒ¢–bf–VÆBÓÒf—'7Eöf–VÆBæf–VÆ@¢’“°¢Ğ¢Ğ¢Ó° ¢ÆWBf—'7Eö'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö&F6…÷v—F…÷6÷W&6Uöö'6W'fW"€¢6÷W&6U÷FW‡BÀ¢$&öG”f–VÆG2ç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒCs’’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢f×WBö'6W'fRÀ¢¢æW‡V7B‚'V&Æ–26Æ72Ö&öG’&6¶–ærf–VÆG2–æ—F–Æ—¦R–â&÷F‚6öç7G'V7F÷"6†W2"“°¢76W'B€¢ö'6W'fVBÀ¢'ÆâfÆ–FF–öâ'Vç2&Vf÷&RF†RVÖ—76–öâ&÷VæF'’ ¢“°¢76W'EöW†÷WGWBæ6Æ75öf–ÆW2‚’æÆVâ‚’Â2“°¢ÆWB6V6öæEö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'V&Æ–2Ö&öG’Öf–VÆBÖ–æ—F–Æ—¦F–öâ×6V6öæB"“°¢ÆWB6V6öæEö'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö&F6‚€¢6÷W&6U÷FW‡BÀ¢$&öG”f–VÆG2ç66Æ"À¢6V6öæEö÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒCs’’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢¢æW‡V7B‚&g&W6‚g&öçFVæB6W76–öâVÖ—G2F†R6ÖR&öG’Öf–VÆB6Æ76W2"“°¢76W'EöW†f—'7Eö'F–f7G2æÆVâ‚’Â2“°¢76W'EöW†f—'7Eö'F–f7G2æÆVâ‚’Â6V6öæEö'F–f7G2æÆVâ‚’“°¢76W'B€¢f—'7Eö'F–f7G0¢æ—FW"‚¢ç¦—‚g6V6öæEö'F–f7G2¢æÆÂ‡Â†f—'7BÂ6V6öæB—Â°¢f—'7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ6V6öæBæ6Æ72æ–çFW&æÅöæÖR‚¢bbf—'7Bæ6Æ72æ'—FW2‚’ÓÒ6V6öæBæ6Æ72æ'—FW2‚¢Ò¢“° ¢ÆWB÷WGWE÷F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB&öG•ööæÇ•ö'—FW2Ğ¢g3£§&VB†÷WGWBçF‚‚’æ¦ö–â‚$&öG”öæÇ’æ6Æ72"’’æW‡V7B‚'&VBVÖ—GFVB&öG”öæÇ’6Æ72f–ÆR"“°¢ÆWB×WB&öG•ööæÇ•÷&VFW"Ò&VFW#£¦æWr‚f&öG•ööæÇ•ö'—FW2“°¢ÆWB&öG•ööæÇ’Ò6Æ74f–ÆS£¦FV6öFR‚f×WB&öG•ööæÇ•÷&VFW"’æW‡V7B‚&FV6öFR&öG”öæÇ’6Æ72f–ÆR"“°¢76W'B†&öG•ööæÇ•÷&VFW"æ—5öEöVæB‚’“°¢&öG•ööæÇ¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚'fÆ–B&öG”öæÇ’&VfW&Væ6W2"“°¢76W'EöW†&öG•ööæÇ’çfW'6–öâæÖ¦÷"Â¥dÕô4Ä54d”ÄUôÔ¤õ%õdU%4”ôâ“°¢76W'Eöf–VÆE÷&VfW&Væ6U÷6WVVæ6R€¢f&öG•ööæÇ’À¢VÖ—GFVEöÖWF†öEö6öFR‚f&öG•ööæÇ’Â#Æ–æ—Câ"’À¢e°¢ƒ†#RÂ$&öG”öæÇ’"Â'6VVB"Â$’"’À¢ƒ†#RÂ$&öG”öæÇ’"Â&Væ&ÆVB"Â%¢"’À¢ƒ†#RÂ$&öG”öæÇ’"Â'&F–ò"Â$B"’À¢ƒ†#RÂ$&öG”öæÇ’"Â'66÷&R"Â$b"’À¢ƒ†#RÂ$&öG”öæÇ’"Â'&FR"Â$B"’À¢ÒÀ¢“°¢ÆWB&öG•ööæÇ•ö6öç7G'V7F÷"ÒVÖ—GFVEöÖWF†öEö6öFR‚f&öG•ööæÇ’Â#Æ–æ—Câ"“°¢76W'EöW€¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2†&öG•ööæÇ•ö6öç7G'V7F÷"•²âã%ÒÀ¢²ƒÂƒ&’ÂƒÂ†#r•Ğ¢“°¢76W'EöÖWF†öE÷&VfW&Væ6R€¢f&öG•ööæÇ’À¢&öG•ööæÇ•ö6öç7G'V7F÷"À¢†#rÀ¢&¦föÆærôö&¦V7B"À¢#Æ–æ—Câ"À¢"‚•b"À¢“°¢ÆWB6÷VçFW%ö'—FW2Ğ¢g3£§&VB†÷WGWBçF‚‚’æ¦ö–â‚$6÷VçFW"æ6Æ72"’’æW‡V7B‚'&VBVÖ—GFVB6÷VçFW"6Æ72f–ÆR"“°¢ÆWB×WB6÷VçFW%÷&VFW"Ò&VFW#£¦æWr‚f6÷VçFW%ö'—FW2“°¢ÆWB6÷VçFW"Ò6Æ74f–ÆS£¦FV6öFR‚f×WB6÷VçFW%÷&VFW"’æW‡V7B‚&FV6öFR6÷VçFW"6Æ72f–ÆR"“°¢76W'B†6÷VçFW%÷&VFW"æ—5öEöVæB‚’“°¢6÷VçFW ¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚'fÆ–B6÷VçFW"&VfW&Væ6W2"“°¢76W'EöW†6÷VçFW"çfW'6–öâæÖ¦÷"Â¥dÕô4Ä54d”ÄUôÔ¤õ%õdU%4”ôâ“°¢76W'Eöf–VÆE÷&VfW&Væ6U÷6WVVæ6R€¢f6÷VçFW"À¢VÖ—GFVEöÖWF†öEö6öFR‚f6÷VçFW"Â#Æ–æ—Câ"’À¢e°¢ƒ†#RÂ$6÷VçFW""Â&–B"Â$’"’À¢ƒ†#RÂ$6÷VçFW""Â&–æ—F–Â"Â$’"’À¢ƒ†#RÂ$6÷VçFW""Â&7W'&VçB"Â$¢"’À¢ƒ†#BÂ$6÷VçFW""Â&–æ—F–Â"Â$’"’À¢ƒ†#RÂ$6÷VçFW""Â&6÷–VB"Â$’"’À¢ÒÀ¢“°¢ÆWB†öÆFW%ö'—FW2Ğ¢g3£§&VB†÷WGWBçF‚‚’æ¦ö–â‚$†öÆFW"æ6Æ72"’’æW‡V7B‚'&VBVÖ—GFVB†öÆFW"6Æ72f–ÆR"“°¢ÆWB×WB†öÆFW%÷&VFW"Ò&VFW#£¦æWr‚f†öÆFW%ö'—FW2“°¢ÆWB†öÆFW"Ò6Æ74f–ÆS£¦FV6öFR‚f×WB†öÆFW%÷&VFW"’æW‡V7B‚&FV6öFR†öÆFW"6Æ72f–ÆR"“°¢76W'B††öÆFW%÷&VFW"æ—5öEöVæB‚’“°¢†öÆFW ¢çfÆ–FFU÷&VfW&Væ6W2‚¢æW‡V7B‚'fÆ–B†öÆFW"&VfW&Væ6W2"“°¢76W'EöW††öÆFW"çfW'6–öâæÖ¦÷"Â¥dÕô4Ä54d”ÄUôÔ¤õ%õdU%4”ôâ“°¢ÆWB6÷VçFW%ö6öç7G'V7F÷"ÒVÖ—GFVEöÖWF†öEö6öFR‚f6÷VçFW"Â#Æ–æ—Câ"“°¢76W'EöW€¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2†6÷VçFW%ö6öç7G'V7F÷"•²âã%ÒÀ¢²ƒÂƒ&’ÂƒÂ†#r•Ğ¢“°¢76W'EöÖWF†öE÷&VfW&Væ6R€¢f6÷VçFW"À¢6÷VçFW%ö6öç7G'V7F÷"À¢†#rÀ¢&¦föÆærôö&¦V7B"À¢#Æ–æ—Câ"À¢"‚•b"À¢“°¢76W'EöW€¢&öG•ööæÇ¢æÖWF†öG0¢æ—FW"‚¢æf–ÇFW%öÖ‡ÆÖWF†öGÂ&öG•ööæÇ’æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’æö²‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢°¢#Æ–æ—Câ"À¢'6VVB"À¢&Væ&ÆVB"À¢&Væ&ÆVEòFW"À¢'&F–ò"À¢'66÷&R"À¢'66÷&UòFW"À¢'&FR"À¢'&FUòFW ¢Ğ¢“°¢ÆWB&öG•ööæÇ•ö6öç7G'V7F÷"Ò&öG•ööæÇ¢æÖWF†öG0¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂ&öG•ööæÇ’æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’ÓÒö²‚#Æ–æ—Câ"’¢æW‡V7B‚$&öG”öæÇ’†2öæRvVæW&FVB6öç7G'V7F÷""“°¢76W'B†&öG•ööæÇ•ö6öç7G'V7F÷"æ66W75öfÆw2æ—5÷V&Æ–2‚’“°¢76W'EöW€¢&öG•ööæÇ¢æ6öç7FçE÷ööÀ¢çWFc‚†&öG•ööæÇ•ö6öç7G'V7F÷"æFW67&—F÷%ö–æFW‚’À¢ö²‚"‚•b"¢“°¢ÆWB6÷VçFW%öÖWF†öEöæÖW2Ò6÷VçFW ¢æÖWF†öG0¢æ—FW"‚¢æf–ÇFW%öÖ‡ÆÖWF†öGÂ6÷VçFW"æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’æö²‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW€¢6÷VçFW%öÖWF†öEöæÖW2À¢²#Æ–æ—Câ"Â&–B"Â&–æ—F–Â"Â&7W'&VçB"Â&7W'&VçEòFW%Ğ¢“°¢ÆWB†öÆFW%öÖWF†öEöæÖW2Ò†öÆFW ¢æÖWF†öG0¢æ—FW"‚¢æf–ÇFW%öÖ‡ÆÖWF†öGÂ†öÆFW"æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’æö²‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW††öÆFW%öÖWF†öEöæÖW2Â²#Æ–æ—Câ"Â'6VÆV7FVB"Â'6VÆV7FVEòFW%Ò“° ¢f÷"†f–VÆEöæÖRÂFW67&—F÷"Â—5öf–æÂ’–â°¢‚'6VVB"Â$’"ÂG'VR’À¢‚&Væ&ÆVB"Â%¢"ÂfÇ6R’À¢‚'&F–ò"Â$B"ÂG'VR’À¢‚'66÷&R"Â$b"ÂfÇ6R’À¢‚'&FR"Â$B"ÂfÇ6R’À¢Ò°¢ÆWBf–VÆG2Ò&öG•ööæÇ¢æf–VÆG0¢æ—FW"‚¢æf–ÇFW"‡Æf–VÆGÂ°¢&öG•ööæÇ’æ6öç7FçE÷ööÂçWFc‚†f–VÆBææÖUö–æFW‚’ÓÒö²†f–VÆEöæÖR¢bb&öG•ööæÇ’æ6öç7FçE÷ööÂçWFc‚†f–VÆBæFW67&—F÷%ö–æFW‚’ÓÒö²†FW67&—F÷"¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢ÆWB¶f–VÆEÒÒf–VÆG2æ5÷6Æ–6R‚’VÇ6R°¢æ–2‚&W‡V7FVBW†7FÇ’öæR¶f–VÆEöæÖWÓ§¶FW67&—F÷'Ò&6¶–ærf–VÆB"“°¢Ó°¢76W'B†f–VÆBæ66W75öfÆw2æ—5÷&—fFR‚’“°¢76W'EöW†f–VÆBæ66W75öfÆw2æ—5öf–æÂ‚’Â—5öf–æÂ“°¢Ğ¢ÆWB6÷VçFW%ö6öç7G'V7F÷'2Ò6÷VçFW ¢æÖWF†öG0¢æ—FW"‚¢æf–ÇFW"‡ÆÖWF†öGÂ6÷VçFW"æ6öç7FçE÷ööÂçWFc‚†ÖWF†öBææÖUö–æFW‚’ÓÒö²‚#Æ–æ—Câ"’¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW†6÷VçFW%ö6öç7G'V7F÷'2æÆVâ‚’Â“°¢76W'B†6÷VçFW%ö6öç7G'V7F÷'5³Òæ66W75öfÆw2æ—5÷V&Æ–2‚’“°¢76W'EöW€¢6÷VçFW ¢æ6öç7FçE÷ööÀ¢çWFc‚†6÷VçFW%ö6öç7G'V7F÷'5³ÒæFW67&—F÷%ö–æFW‚’À¢ö²‚"„’•b"¢“°¢f÷"†f–VÆEöæÖRÂFW67&—F÷"Â—5öf–æÂ’–â²‚&–æ—F–Â"Â$’"ÂG'VR’Â‚&7W'&VçB"Â$¢"ÂfÇ6R•Ò°¢ÆWBf–VÆG2Ò6÷VçFW ¢æf–VÆG0¢æ—FW"‚¢æf–ÇFW"‡Æf–VÆGÂ°¢6÷VçFW"æ6öç7FçE÷ööÂçWFc‚†f–VÆBææÖUö–æFW‚’ÓÒö²†f–VÆEöæÖR¢bb6÷VçFW"æ6öç7FçE÷ööÂçWFc‚†f–VÆBæFW67&—F÷%ö–æFW‚’ÓÒö²†FW67&—F÷"¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢ÆWB¶f–VÆEÒÒf–VÆG2æ5÷6Æ–6R‚’VÇ6R°¢æ–2‚&W‡V7FVBW†7FÇ’öæR¶f–VÆEöæÖWÓ§¶FW67&—F÷'Ò&6¶–ærf–VÆB"“°¢Ó°¢76W'B†f–VÆBæ66W75öfÆw2æ—5÷&—fFR‚’“°¢76W'EöW†f–VÆBæ66W75öfÆw2æ—5öf–æÂ‚’Â—5öf–æÂ“°¢Ğ¢ÆWB6VÆV7FVEöf–VÆG2Ò†öÆFW ¢æf–VÆG0¢æ—FW"‚¢æf–ÇFW"‡Æf–VÆGÂ°¢†öÆFW"æ6öç7FçE÷ööÂçWFc‚†f–VÆBææÖUö–æFW‚’ÓÒö²‚'6VÆV7FVB"¢bb†öÆFW"æ6öç7FçE÷ööÂçWFc‚†f–VÆBæFW67&—F÷%ö–æFW‚’ÓÒö²‚$Ä6÷VçFW#²"¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢ÆWB·6VÆV7FVEöf–VÆEÒÒ6VÆV7FVEöf–VÆG2æ5÷6Æ–6R‚’VÇ6R°¢æ–2‚$†öÆFW"†2W†7FÇ’öæR&—fFR6÷VçFW"&6¶–ærf–VÆB"“°¢Ó°¢76W'B‡6VÆV7FVEöf–VÆBæ66W75öfÆw2æ—5÷&—fFR‚’“°¢76W'B‚6VÆV7FVEöf–VÆBæ66W75öfÆw2æ—5öf–æÂ‚’“° ¢f÷"†6Æ72ÂvWGFW"ÂFW67&—F÷"Âf–VÆEöæÖRÂf–VÆEöFW67&—F÷"Â&WGW&åö÷6öFRÂ7F6²’–â°¢‚f&öG•ööæÇ’Â'6VVB"Â"‚”’"Â'6VVB"Â$’"Â†2Â’À¢‚f&öG•ööæÇ’Â&Væ&ÆVB"Â"‚•¢"Â&Væ&ÆVB"Â%¢"Â†2Â’À¢‚f&öG•ööæÇ’Â'&F–ò"Â"‚”B"Â'&F–ò"Â$B"Â†bÂ"’À¢‚f&öG•ööæÇ’Â'66÷&R"Â"‚”b"Â'66÷&R"Â$b"Â†RÂ’À¢‚f6÷VçFW"Â&–æ—F–Â"Â"‚”’"Â&–æ—F–Â"Â$’"Â†2Â’À¢‚f6÷VçFW"Â&7W'&VçB"Â"‚”¢"Â&7W'&VçB"Â$¢"Â†BÂ"’À¢€¢f†öÆFW"À¢'6VÆV7FVB"À¢"‚”Ä6÷VçFW#²"À¢'6VÆV7FVB"À¢$Ä6÷VçFW#²"À¢†#À¢À¢’À¢Ò°¢76W'EöVÖ—GFVEövWGFW"€¢6Æ72À¢vWGFW"À¢FW67&—F÷"À¢f–VÆEöæÖRÀ¢f–VÆEöFW67&—F÷"À¢&WGW&åö÷6öFRÀ¢7F6²À¢“°¢Ğ ¢f÷"†6Æ72Â6WGFW"ÂFW67&—F÷"Âf–VÆEöæÖRÂf–VÆEöFW67&—F÷"ÂÆöBÂ7F6²ÂÆö6Ç2’–â°¢€¢f&öG•ööæÇ’À¢&Væ&ÆVEòFW"À¢"…¢•b"À¢&Væ&ÆVB"À¢%¢"À¢ƒ"À¢"À¢"À¢’À¢‚f&öG•ööæÇ’Â'66÷&UòFW"Â"„b•b"Â'66÷&R"Â$b"Âƒ#2Â"Â"’À¢‚f&öG•ööæÇ’Â'&FUòFW"Â"„B•b"Â'&FR"Â$B"Âƒ#rÂ2Â2’À¢‚f6÷VçFW"Â&7W'&VçEòFW"Â"„¢•b"Â&7W'&VçB"Â$¢"ÂƒbÂ2Â2’À¢€¢f†öÆFW"À¢'6VÆV7FVEòFW"À¢"„Ä6÷VçFW#²•b"À¢'6VÆV7FVB"À¢$Ä6÷VçFW#²"À¢ƒ&"À¢"À¢"À¢’À¢Ò°¢76W'EöVÖ—GFVE÷6WGFW"€¢6Æ72À¢6WGFW"À¢FW67&—F÷"À¢†f–VÆEöæÖRÂf–VÆEöFW67&—F÷"’À¢ÆöBÀ¢‡7F6²ÂÆö6Ç2’À¢“°¢Ğ ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB†&æW75÷F‚Ò÷WGWBçF‚‚’æ¦ö–â‚$&öG”f–VÆG4†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW75÷F‚À¢'V&Æ–2f–æÂ6Æ72&öG”f–VÆG4†&æW72µÆâV&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’µÆâ&öG”öæÇ’&öG’ÒæWr&öG”öæÇ’‚“²&öG”öæÇ’÷F†W$&öG’ÒæWr&öG”öæÇ’‚“µÆâ–b†&öG’ç6VVB‚’ÒÇÂ&öG’æVæ&ÆVB‚’’F‡&÷ræWr76W'F–öäW'&÷"…Â$&öG”öæÇ’vWGFW'5Â"“µÆâ–b„F÷V&ÆRæF÷V&ÆUFõ&tÆöæt&—G2†&öG’ç&F–ò‚’’ÒF÷V&ÆRæF÷V&ÆUFõ&tÆöæt&—G2ƒãVB’’F‡&÷ræWr76W'F–öäW'&÷"…Â'&F–ò&—G5Â"“µÆâ&öG’æVæ&ÆVEòFW†fÇ6R“²&öG’ç66÷&UòFW‚Óãb“²&öG’ç&FUòFW‚ÓãB“µÆâ–b†&öG’æVæ&ÆVB‚’ÇÂfÆöBæfÆöEFõ&t–çD&—G2†&öG’ç66÷&R‚’’ÒfÆöBæfÆöEFõ&t–çD&—G2‚Óãb’ÇÂF÷V&ÆRæF÷V&ÆUFõ&tÆöæt&—G2†&öG’ç&FR‚’’ÒF÷V&ÆRæF÷V&ÆUFõ&tÆöæt&—G2‚ÓãB’’F‡&÷ræWr76W'F–öäW'&÷"…Â&&öG’6WGFW'5Â"“µÆâ–b‚÷F†W$&öG’æVæ&ÆVB‚’ÇÂfÆöBæfÆöEFõ&t–çD&—G2†÷F†W$&öG’ç66÷&R‚’’ÒfÆöBæfÆöEFõ&t–çD&—G2‚Ó"ã#Vb’ÇÂF÷V&ÆRæF÷V&ÆUFõ&tÆöæt&—G2†÷F†W$&öG’ç&FR‚’’ÒF÷V&ÆRæF÷V&ÆUFõ&tÆöæt&—G2ƒãVB’’F‡&÷ræWr76W'F–öäW'&÷"…Â&&öG’–ç7Fæ6R—6öÆF–öåÂ"“µÆâ6÷VçFW"f—'7BÒæWr6÷VçFW"ƒr“²6÷VçFW"6V6öæBÒæWr6÷VçFW"ƒ2“µÆâ–b†f—'7Bæ–B‚’ÒrÇÂf—'7Bæ–æ—F–Â‚’ÒÇÂf—'7Bæ7W'&VçB‚’ÒÂ’F‡&÷ræWr76W'F–öäW'&÷"…Â&f—'7B6÷VçFW"vWGFW'5Â"“µÆâf—'7Bæ7W'&VçEòFW„ÆöæräÔ”åõdÅTR“²–b†f—'7Bæ7W'&VçB‚’ÒÆöæräÔ”åõdÅTR’F‡&÷ræWr76W'F–öäW'&÷"…Â$ÆöæräÔ”åõdÅTR6WGFW%Â"“µÆâf—'7Bæ7W'&VçEòFW„ÆöæräÔ…õdÅTR“²–b†f—'7Bæ7W'&VçB‚’ÒÆöæräÔ…õdÅTR’F‡&÷ræWr76W'F–öäW'&÷"…Â$ÆöæräÔ…õdÅTR6WGFW%Â"“µÆâ–b‡6V6öæBæ–B‚’Ò2ÇÂ6V6öæBæ–æ—F–Â‚’ÒÇÂ6V6öæBæ7W'&VçB‚’ÒÂ’F‡&÷ræWr76W'F–öäW'&÷"…Â&6÷VçFW"–ç7Fæ6R—6öÆF–öåÂ"“µÆâ†öÆFW"†öÆFW"ÒæWr†öÆFW"†f—'7B“²–b††öÆFW"ç6VÆV7FVB‚’Òf—'7B’F‡&÷ræWr76W'F–öäW'&÷"…Â'&VfW&Væ6RvWGFW%Â"“µÆâ†öÆFW"ç6VÆV7FVEòFW‡6V6öæB“²–b††öÆFW"ç6VÆV7FVB‚’Ò6V6öæB’F‡&÷ræWr76W'F–öäW'&÷"…Â'&VfW&Væ6R6WGFW%Â"“µÆâÕÆçÕÆâ"À¢¢æW‡V7B‚'w&—FR&öG’Öf–VÆB†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â÷WGWE÷F‚Â"ÖB"Â÷WGWE÷F…Ò¢æ&r‚f†&æW75÷F‚¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2f÷"&öG’Öf–VÆB†&æW72"“°¢76W'B†¦f2ç7FGW2ç7V66W72‚’Â&¦f2f–ÆVC¢¶¦f3£÷Ò"“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢$&öG”f–VÆG4†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ffW&–f–W"f÷"&öG’Öf–VÆB†&æW72"“°¢76W'B†¦fç7FGW2ç7V66W72‚’Â&¦ff–ÆVC¢¶¦f£÷Ò"“°¢Ğ§Ğ ¢5·FW7EĞ¦fâÖ—†VE÷f—6–&–Æ—G•÷Æå÷W&×WFF–öåö6ææ÷Eö†–FUööf÷'v&Eöf–VÆE÷&VB‚’°¢ÆWB6÷W&6U÷FW‡BÒ&6Æ72Ö—†VC¥ÆâfÂf—'7C¢–çBÒÆFW%Æâ&—fFRfÂÆFW#¢–çBÒ"#°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&Ö—†VBÖf–VÆBÖ÷&FW"×W&×WFF–öâ"“°¢ÆWB×WBö'6W'fVBÒfÇ6S°¢ÆWB×WBö'6W'fRÒÇVæ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÂöFVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ÆWB6Æ72ÒVF—Eö6Æ75öæÖVB‡Væ—BÂ$Ö—†VB"“°¢ÆWB6Æ75÷ÆâÒVæ—@¢æ6Æ76W0¢æ—FW%ö×WB‚¢æf–æB‡ÇÆçÂÆâæ6Æ72ÓÒ6Æ72¢æW‡V7B‚$Ö—†VB†26Æ72Æâ"“°¢76W'EöW†6Æ75÷Æâæf–VÆG2æÆVâ‚’Â"“°¢6Æ75÷Æâæf–VÆG2ç7vƒÂ“°¢ö'6W'fVBÒG'VS°¢Ó°¢ÆWB&W7VÇBÒ6ö×–ÆU÷6÷W&6Uö&F6…öö'6W'fVB€¢6÷W&6U÷FW‡BÀ¢$Ö—†VBç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒCƒ’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤W†7FÇ”öæRÀ¢6öÖR‚f×WBö'6W'fR’À¢“°¢76W'B€¢ö'6W'fVBÀ¢'F†RFW7BW&×WFW2F†RfÆ–FFVBg&öçFVæBf–VÆBÆâ ¢“°¢76W'B†ÖF6†W2€¢&W7VÇBÀ¢W'"…—VÆ–æTf–ÇW&S£¤6öFVvVâ€¢§fÔ6öFVvVäW'&÷#£¤÷WDöd÷&FW$f–VÆEÆâ²ââĞ¢’¢’“°¢76W'B€¢÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’À¢&â÷WBÖöb×6÷W&6RÖ÷&FW"Ö—†VBf–VÆBÆâ×W7BV&Æ—6‚æò6Æ72f–ÆR ¢“°§Ğ ¢5·FW7EĞ¦fâ&—fFU÷&–Ö'•ö6öç7G'V7F÷%ö—5÷&V¦V7FVE÷v—F†÷WEö÷V&Æ–5ö6öç7G'V7F÷%öf–ÆR‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&—fFRÖ6öç7G'V7F÷""“°¢ÆWBW'&÷"Ò6ö×–ÆUöf—‡GW&R‚%&—fFT6öç7G'V7F÷"ç66Æ"Â÷WGWBçF‚‚’’çVçw&öW'"‚“°¢76W'B†ÖF6†W2€¢W'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¥Vç7W÷'FVD6öç7G'V7F÷"²ââÒ¢’“°¢76W'B†÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°§Ğ ¢5·FW7EĞ¦fâ–çfÆ–E÷6÷W&6Uö—5÷&V¦V7FVE÷v—F…÷'6W%öF–væ÷7F–75÷v—F†÷WEö÷WGWB‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&–çfÆ–B×6÷W&6R"“°¢ÆWBW'&÷"Ò6ö×–ÆUöf—‡GW&R‚$–çfÆ–Bç66Æ"Â÷WGWBçF‚‚’’çVçw&öW'"‚“°¢76W'B†ÖF6†W2†W'&÷"Â—VÆ–æTf–ÇW&S£¥'6W"†F–væ÷7F–72’–bF–væ÷7F–72æ—5öV×G’‚’’“°¢76W'B†÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°§Ğ ¢5·FW7EĞ¦fâ6¶vU÷VÆ–f–VE÷6÷W&6U÷&VçE÷&V6†W5ö†–W&&6‡•öæEö6öç7G'V7F÷%öÖWFFF‚’°¢ÆWB6÷W&6U÷FW‡BÒ'6¶vRW†×ÆUÆæ6Æ72&6R‚•Ææ6Æ72FW&—fVB‚’W‡FVæG2W†×ÆRä&6R#°¢ÆWB6÷W&6Uö–BÒ6÷W&6T–C£¦g&öÕö–æFW‚ƒ3#r“°¢ÆWB6÷W&6RÒ6÷W&6UFW‡C£¦æWr‡6÷W&6U÷FW‡B’çVçw&‚“°¢ÆWB66ææW"Ò6öçFW‡GVÅ66ææW#£¦æWr‡6÷W&6U÷FW‡B’çVçw&‚“°¢ÆWB×WB7F÷&RÒ6VÖçF–57F÷&S£¦æWr‚“°¢ÆWBFVf–æ—F–öç2ÒFVf–æ—F–öç3£¦&ö÷G7G&‚f×WB7F÷&R“°¢ÆWB'6VBÒ'6Uö6ö×–ÆF–öå÷Væ—B‡6÷W&6RÂ6÷W&6Uö–BÂ66ææW"Âf×WB7F÷&RææÖW2“°¢76W'B‡'6VBæF–væ÷7F–72æ—5öV×G’‚’Â'³£÷Ò"Â'6VBæF–væ÷7F–72“°¢ÆWB×WB6¶vW2Ò6¶vW3£¦æWr‚“°¢ÆWB–æFW‚ÒÖF6‚æÖUö6ö×–ÆF–öå÷Væ—E÷v—F…öF–væ÷7F–72€¢g'6VBæ7BÀ¢'6VBç&ö÷BÀ¢6÷W&6Uö–BÀ¢%6¶vUVÆ–f–VE&VçBç66Æ"À¢6÷W&6U'6U7FGW3£¤6ÆVâÀ¢f×WB7F÷&RÀ¢f×WB6¶vW2À¢¢çVçw&‚¢°¢æÖ–æt÷WF6öÖS£¤æÖVB²–æFW‚ÂF–væ÷7F–72Ò–bF–væ÷7F–72æ—5öV×G’‚’Óâ–æFW‚À¢÷F†W"Óâæ–2‚'6÷W&6R6†÷VÆB&RæÖVBv—F†÷WBF–væ÷7F–73¢¶÷F†W#£÷Ò"’À¢Ó°¢ÆWBG&VT¶–æC£¥6¶vTFVb‡6¶vR’Òg'6VBæ7BævWB‡'6VBç&ö÷B’æ¶–æBVÇ6R°¢æ–2‚'6÷W&6R&ö÷B6†÷VÆB&R6¶vRFVf–æ—F–öâ"¢Ó°¢ÆWB6÷W&6Uö6Æ76W2Ò6¶vP¢ç7FG0¢æ—FW"‚¢æf–ÇFW%öÖ‡ÇG&VWÂ°¢ÆWBG&VT¶–æC£¥G—TFVb†FVf–æ—F–öâ’Òg'6VBæ7BævWB‚§G&VR’æ¶–æBVÇ6R°¢&WGW&âæöæS°¢Ó°¢ÆWB6Æ72Ò–æFW‚ç7–Ö&öÅöB‡6÷W&6Uö–BÂ§G&VR“ó°¢–b7F÷&Rç7–Ö&öÇ2ævWB†6Æ72’æ¶–æBÒ7–Ö&öÄ¶–æC£¤6Æ72°¢&WGW&âæöæS°¢Ğ¢ÆWBG&VT¶–æC£¥FV×ÆFR‡FV×ÆFR’Òg'6VBæ7BævWB†FVf–æ—F–öâç&‡2’æ¶–æBVÇ6R°¢æ–2‚'6÷W&6R6Æ726†÷VÆB†fRFV×ÆFR"¢Ó°¢6öÖR‚†6Æ72ÂFV×ÆFRæ6öç7G'V7F÷"’¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW‡6÷W&6Uö6Æ76W2æÆVâ‚’Â"“° ¢ÆWB×WBG—W"Ò6÷W&6UG—W#£¦æWr€¢g'6VBæ7BÀ¢6÷W&6Uö–BÀ¢f–æFW‚À¢f×WB7F÷&RÀ¢FVf–æ—F–öç2À¢g6¶vW2À¢“°¢f÷"†6Æ72Âò’–âg6÷W&6Uö6Æ76W2°¢G—W"æ6ö×ÆWFU÷7–Ö&öÂ‚¦6Æ72’çVçw&‚“°¢Ğ¢ÆWB×WBÆç2ÒfV3£¦æWr‚“°¢f÷"†6Æ72Â6öç7G'V7F÷%÷G&VR’–â6÷W&6Uö6Æ76W2°¢ÆWB6öç7G'V7F÷"Ò–æFW€¢ç7–Ö&öÅöB‡6÷W&6Uö–BÂ6öç7G'V7F÷%÷G&VR¢æW‡V7B‚&6öç7G'V7F÷"6†÷VÆB†fR6æöæ–6Â6÷W&6R7–Ö&öÂ"“°¢G—W"æ6ö×ÆWFU÷7–Ö&öÂ†6öç7G'V7F÷"’çVçw&‚“°¢Æç2çW6‚„§fÔ6Æ75Æâ°¢6Æ72À¢ÖWF†öG3¢fV3£¦æWr‚’À¢f–VÆG3¢fV3£¦æWr‚’À¢–æ—F–Æ—¦F–öã¢§fÔ6Æ74–æ—F–Æ—¦F–öåÆã£¥&–Ö'”6öç7G'V7F÷"€¢§fÕ&–Ö'”6öç7G'V7F÷%Æã£¦g&öÕ÷6÷W&6R€¢6Æ72À¢6öç7G'V7F÷"À¢fV3£¦æWr‚’À¢g'6VBæ7BÀ¢f–æFW‚À¢¢çVçw&‚’À¢’À¢Ò“°¢Ğ¢ÆWBVæ—BÒ§fÔ6ö×–ÆF–öåVæ—B°¢7F÷&S¢G—W"ç7F÷&R‚’À¢G—VEö7C¢G—W"çG—VEö7B‚’À¢6Æ76W3¢Æç2À¢Ó°¢ÆWBÖWFFFÒVæ—BæÖWFFF‚fFVf–æ—F–öç2’çVçw&‚“°¢76W'EöW†ÖWFFFæÆVâ‚’Â"“°¢76W'EöW€¢ÖWFFF³Òç7WW&6Æ72‚’çVçw&‚’ç&VçEö–çFW&æÅöæÖR‚’À¢&W†×ÆRô&6R ¢“°§Ğ ¢5·FW7EĞ¦fâ6÷W&6Uö÷fW'&–FW5ö¶VWöFV6Æ&VEöÖWF†öG5÷6W&FUög&öÕöVffV7F—fUöF—7F6‚‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&–æ†W&—FVBÖÖWF†öBÖF—7F6‚"“°¢ÆWB6÷W&6RÒ&6Æ72&6R²FVbfÇVR‚“¢–çBÒ²FVbF÷V&ÆVB‚“¢–çBÒfÇVR‚’¢"ÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²÷fW'&–FRFVbfÇVR‚“¢–çBÒ#ÕÆâ#°¢ÆWB×WBö'6W'fVEöÖWFFFÒæöæS°¢ÆWB×WBö'6W'fRÒÇVæ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÂFVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ö'6W'fVEöÖWFFFÒ6öÖR€¢Væ—BæÖWFFF†FVf–æ—F–öç2¢æW‡V7B‚'fÆ–FFVB÷fW'&–FRÖWFFF"’À¢“°¢Ó°¢6ö×–ÆU÷6÷W&6Uö&F6…öö'6W'fVB€¢6÷W&6RÀ¢$–æ†W&—FVDF—7F6‚ç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒ33’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢6öÖR‚f×WBö'6W'fR’À¢¢æW‡V7B‚&âW†7BV&Æ–2÷fW'&–FR6†÷VÆBVÖ—B&÷F‚6Æ76W2"“° ¢ÆWBÖWFFFÒö'6W'fVEöÖWFFFæW‡V7B‚&ö'6W'fW"6GW&W2F†R6Æ72ÖWFFF"“°¢ÆWB&6RÒÖWFFF¢æ—FW"‚¢æf–æB‡Æ6Æ77Â6Æ72æ–FVçF—G’‚’æ–çFW&æÅöæÖR‚’ÓÒ$&6R"¢æW‡V7B‚$&6RÖWFFF"“°¢ÆWBFW&—fVBÒÖWFFF¢æ—FW"‚¢æf–æB‡Æ6Æ77Â6Æ72æ–FVçF—G’‚’æ–çFW&æÅöæÖR‚’ÓÒ$FW&—fVB"¢æW‡V7B‚$FW&—fVBÖWFFF"“°¢ÆWB&6U÷fÇVRÒ&6P¢æÖWF†öG2‚¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂÖWF†öBææÖR‚’ÓÒ'fÇVR"¢æW‡V7B‚$&6RFV6Æ&W2fÇVR"“°¢ÆWB&6UöF÷V&ÆVBÒ&6P¢æÖWF†öG2‚¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂÖWF†öBææÖR‚’ÓÒ&F÷V&ÆVB"¢æW‡V7B‚$&6RFV6Æ&W2F÷V&ÆVB"“°¢ÆWBFW&—fVE÷fÇVRÒFW&—fV@¢æÖWF†öG2‚¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂÖWF†öBææÖR‚’ÓÒ'fÇVR"¢æW‡V7B‚$FW&—fVBFV6Æ&W2fÇVR"“° ¢76W'EöæR†&6U÷fÇVRç7–Ö&öÂ‚’ÂFW&—fVE÷fÇVRç7–Ö&öÂ‚’“°¢76W'EöW†&6U÷fÇVRæFW67&—F÷"‚’æ5÷7G"‚’Â"‚”’"“°¢76W'EöW†FW&—fVE÷fÇVRæFW67&—F÷"‚’æ5÷7G"‚’Â"‚”’"“°¢76W'EöW€¢FW&—fVBæÖWF†öG2‚’æÆVâ‚’À¢À¢&–æ†W&—FVB&öF–W2&Ræ÷B6÷–VB ¢“° ¢ÆWB–æ†W&—FVEöF÷V&ÆVBÒFW&—fV@¢æF—7F6…öÖWF†öG2‚¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂÖWF†öBææÖR‚’ÓÒ&F÷V&ÆVB"¢æW‡V7B‚$&6RæF÷V&ÆVB&VÖ–ç2f—6–&ÆRöâFW&—fVB"“°¢76W'EöW€¢–æ†W&—FVEöF÷V&ÆVBç&V6V—fW%ö6Æ72‚’À¢FW&—fVBæ–FVçF—G’‚’ç7–Ö&öÂ‚¢“°¢76W'EöW€¢–æ†W&—FVEöF÷V&ÆVBæFV6Æ&–æuö6Æ72‚’À¢&6Ræ–FVçF—G’‚’ç7–Ö&öÂ‚¢“°¢76W'EöW†–æ†W&—FVEöF÷V&ÆVBæÖWF†öB‚’Â&6UöF÷V&ÆVBç7–Ö&öÂ‚’“°¢ÆWB÷fW'&–FFVå÷fÇVRÒFW&—fV@¢æF—7F6…öÖWF†öG2‚¢æ—FW"‚¢æf–æB‡ÆÖWF†öGÂÖWF†öBææÖR‚’ÓÒ'fÇVR"¢æW‡V7B‚$FW&—fVBçfÇVR&WÆ6W2F†R–æ†W&—FVBF—7F6‚–FVçF—G’"“°¢76W'EöW€¢÷fW'&–FFVå÷fÇVRç&V6V—fW%ö6Æ72‚’À¢FW&—fVBæ–FVçF—G’‚’ç7–Ö&öÂ‚¢“°¢76W'EöW€¢÷fW'&–FFVå÷fÇVRæFV6Æ&–æuö6Æ72‚’À¢FW&—fVBæ–FVçF—G’‚’ç7–Ö&öÂ‚¢“°¢76W'EöW†÷fW'&–FFVå÷fÇVRæÖWF†öB‚’ÂFW&—fVE÷fÇVRç7–Ö&öÂ‚’“°§Ğ ¢5·FW7EĞ¦fâ6÷W&6Uö÷fW'&–FUö66WG5öWV—fÆVçE÷VÆ–f–VEöæE÷VçVÆ–f–VE÷&VfW&Væ6U÷G—W2‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&–æ†W&—FVB×VÆ–f–VB×&VfW&Væ6RÖ÷fW'&–FR"“°¢ÆWB6÷W&6RÒ'6¶vRW†×ÆUÆæ6Æ726÷VçFW"·ÕÆæ6Æ72&6R²FVbfÇVR†3¢W†×ÆRä6÷VçFW"“¢W†×ÆRä6÷VçFW"Ò2ÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²÷fW'&–FRFVbfÇVR†3¢6÷VçFW"“¢6÷VçFW"Ò2ÕÆâ#° ¢6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ%VÆ–f–VD÷fW'&–FRç66Æ"Â÷WGWBçF‚‚’¢æW‡V7B‚&WV—fÆVçB6æöæ–6Â6Æ72&VfW&Væ6W2f÷&ÒâW†7B÷fW'&–FR"“°¢76W'EöW†÷WGWBæ6Æ75öf–ÆW2‚’æÆVâ‚’Â2“°§Ğ ¢5·FW7EĞ¦fâ6ÖUöæÖUöF–ffW&VçEöFW67&—F÷%÷&VÖ–ç5öåö÷fW&ÆöEöæ÷Eöåö÷fW'&–FR‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&–æ†W&—FVBÖ÷fW&ÆöBÖF—7F6‚"“°¢ÆWB6÷W&6RÒ&6Æ72&6R²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²FVbfÇVR‡ƒ¢–çB“¢–çBÒ‚ÕÆâ#°¢ÆWB×WBö'6W'fVEöÖWFFFÒæöæS°¢ÆWB×WBö'6W'fRÒÇVæ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÂFVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ö'6W'fVEöÖWFFFÒ6öÖR‡Væ—BæÖWFFF†FVf–æ—F–öç2’æW‡V7B‚'fÆ–B÷fW&ÆöBÖWFFF"’“°¢Ó°¢6ö×–ÆU÷6÷W&6Uö&F6…öö'6W'fVB€¢6÷W&6RÀ¢$–æ†W&—FVD÷fW&ÆöBç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒ332’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢6öÖR‚f×WBö'6W'fR’À¢¢æW‡V7B‚&6ÖRÖæÖRÖWF†öBv—F‚æ÷F†W"FW67&—F÷"—2â÷fW&ÆöB"“° ¢ÆWBÖWFFFÒö'6W'fVEöÖWFFFæW‡V7B‚&ö'6W'fW"6GW&W26Æ72ÖWFFF"“°¢ÆWBFW&—fVBÒÖWFFF¢æ—FW"‚¢æf–æB‡Æ6Æ77Â6Æ72æ–FVçF—G’‚’æ–çFW&æÅöæÖR‚’ÓÒ$FW&—fVB"¢æW‡V7B‚$FW&—fVBÖWFFF"“°¢ÆWBfÇVUöF—7F6†W2ÒFW&—fV@¢æF—7F6…öÖWF†öG2‚¢æ—FW"‚¢æf–ÇFW"‡ÆÖWF†öGÂÖWF†öBææÖR‚’ÓÒ'fÇVR"¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW‡fÇVUöF—7F6†W2æÆVâ‚’Â"“°¢76W'B‡fÇVUöF—7F6†W2æ—FW"‚’æç’‡ÆÖWF†öGÂ°¢ÖWF†öBæFW67&—F÷"‚’æ5÷7G"‚’ÓÒ"‚”’ ¢bbÖWF†öBæFV6Æ&–æuö6Æ72‚¢ÓÒÖWFFF¢æ—FW"‚¢æf–æB‡Æ6Æ77Â6Æ72æ–FVçF—G’‚’æ–çFW&æÅöæÖR‚’ÓÒ$&6R"¢çVçw&‚¢æ–FVçF—G’‚¢ç7–Ö&öÂ‚¢Ò’“°¢76W'B€¢fÇVUöF—7F6†W0¢æ—FW"‚¢æç’‡ÆÖWF†öGÂÖWF†öBæFW67&—F÷"‚’æ5÷7G"‚’ÓÒ"„’”’"¢“°§Ğ ¢5·FW7EĞ¦fâ6÷W&6Uö÷fW'&–FUö6öçG&7E÷&V¦V7G5öÖ—76–æu÷F&vWG5öæE÷6–væGW&UöÖ—6ÖF6†W2‚’°¢f÷"†Æ&VÂÂ6÷W&6RÂW‡V7FVEö—77VR’–â°¢€¢&æò×&VçBÖÖWF†öB"À¢&6Æ72&6R²FVb÷F†W"‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²÷fW'&–FRFVbfÇVR‚“¢–çBÒ"ÕÆâ"À¢§fÔ÷fW'&–FT—77VS£¤æõ&VçDFV6Æ&F–öâÀ¢’À¢€¢'w&öærÖFW67&—F÷""À¢&6Æ72&6R²FVbfÇVR‡ƒ¢–çB“¢–çBÒ‚ÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²÷fW'&–FRFVbfÇVR‚“¢–çBÒ"ÕÆâ"À¢§fÔ÷fW'&–FT—77VS£¥6–væGW&TÖ—6ÖF6‚À¢’À¢€¢'w&öær×&W7VÇBÖFW67&—F÷""À¢&6Æ72&6R²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²÷fW'&–FRFVbfÇVR‚“¢&ööÆVâÒG'VRÕÆâ"À¢§fÔ÷fW'&–FT—77VS£¥6–væGW&TÖ—6ÖF6‚À¢’À¢€¢'w&öær×&W7VÇB×v—F†÷WBÖ÷fW'&–FR"À¢&6Æ72&6R²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²FVbfÇVR‚“¢&ööÆVâÒG'VRÕÆâ"À¢§fÔ÷fW'&–FT—77VS£¥6–væGW&TÖ—6ÖF6‚À¢’À¢€¢&Ö—76–ærÖ÷fW'&–FRÖÖöF–f–W""À¢&6Æ72&6R²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²FVbfÇVR‚“¢–çBÒ"ÕÆâ"À¢§fÔ÷fW'&–FT—77VS£¤÷fW'&–FTÖöF–f–W%&WV—&VBÀ¢’À¢€¢&æ'&÷vW"Ö÷fW'&–FR×f—6–&–Æ—G’"À¢&6Æ72&6R²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²&—fFR÷fW'&–FRFVbfÇVR‚“¢–çBÒ"ÕÆâ"À¢§fÔ÷fW'&–FT—77VS£¤æ'&÷vW%f—6–&–Æ—G’À¢’À¢€¢'&—fFR×&VçBÖ÷fW'&–FR"À¢&6Æ72&6R²&—fFRFVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²÷fW'&–FRFVbfÇVR‚“¢–çBÒ"ÕÆâ"À¢§fÔ÷fW'&–FT—77VS£¤–æ66W76–&ÆU&VçDÖWF†öBÀ¢’À¢Ò°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr†Æ&VÂ“°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ$–çfÆ–D÷fW'&–FRç66Æ"Â÷WGWBçF‚‚’¢æW‡V7EöW'"‚&–çfÆ–B÷fW'&–FRÖWFFF×W7Bf–Â&Vf÷&RV&Æ—6†–ær6Æ76W2"“°¢76W'B€¢ÖF6†W2€¢W'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¤–çfÆ–D§fÔ÷fW'&–FR°¢—77VRÀ¢âà¢Ò’–b—77VRÓÒW‡V7FVEö—77VP¢’À¢'¶Æ&VÇÓ¢W‡V7FVB¶W‡V7FVEö—77VS£÷ÒÂv÷B¶W'&÷#£÷Ò ¢“°¢76W'B€¢÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’À¢'¶Æ&VÇÓ¢æò'F–Â÷WGWB ¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ6÷W&6Uö÷fW'&–FU÷&V¦V7G5öå÷VçÆææVE÷&VçEöFV6Æ&F–öâ‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'VçÆææVB×&VçBÖ÷fW'&–FR"“°¢ÆWB6÷W&6RÒ&6Æ72&6R²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²÷fW'&–FRFVbfÇVR‚“¢–çBÒ"ÕÆâ#°¢ÆWB×WBöÖ—E÷&VçEöÖWF†öG2ÒÇVæ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÀ¢öFVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ÆWB&6RÒVæ—@¢æ6Æ76W0¢æ—FW"‚¢æf–æB‡ÇÆçÂ°¢ÆWB7–Ö&öÂÒVæ—Bç7F÷&Rç7–Ö&öÇ2ævWB‡Æâæ6Æ72“°¢Væ—Bç7F÷&RææÖW2çG'•÷&W6öÇfR‡7–Ö&öÂææÖRçFW‡B‚’’ÓÒ6öÖR‚$&6R"¢Ò¢æÖ‡ÇÆçÂÆâæ6Æ72¢æW‡V7B‚'6÷W&6R&F6‚†2&6R"“°¢Væ—Bæ6Æ76W0¢æ—FW%ö×WB‚¢æf–æB‡ÇÆçÂÆâæ6Æ72ÓÒ&6R¢æW‡V7B‚$&6RÆâ"¢æÖWF†öG0¢æ6ÆV"‚“°¢Ó°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö&F6…öö'6W'fVB€¢6÷W&6RÀ¢%VçÆææVE&VçD÷fW'&–FRç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒ33’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢6öÖR‚f×WBöÖ—E÷&VçEöÖWF†öG2’À¢¢æW‡V7EöW'"‚&â÷fW'&–FR×W7BF&vWBÆææVBF—&V7B×&VçBÖWF†öB"“°¢76W'B€¢ÖF6†W2€¢W'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¤–çfÆ–D§fÔ÷fW'&–FR°¢—77VS¢§fÔ÷fW'&–FT—77VS£¥&VçDÖWF†öEVçÆææVBÀ¢âà¢Ò¢’À¢&W‡V7FVBG—VBVçÆææVB×&VçB÷fW'&–FRW'&÷"Âv÷B¶W'&÷#£÷Ò ¢“°¢76W'B†÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°§Ğ ¢5·FW7EĞ¦fâ6÷W&6Uö÷fW'&–FU÷&WV—&W5öö6öæ7&WFUö6†–ÆEö&öG’‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&Ö—76–ærÖ÷fW'&–FRÖ&öG’"“°¢ÆWB6÷W&6RÒ&6Æ72&6R²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²÷fW'&–FRFVbfÇVR‚“¢–çBÒ"ÕÆâ#°¢ÆWB×WBöÖ—Eö6†–ÆEö&öG’ÒÇVæ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÂöFVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ÆWBFW&—fVBÒVæ—@¢æ6Æ76W0¢æ—FW"‚¢æf–æB‡ÇÆçÂ°¢ÆWB7–Ö&öÂÒVæ—Bç7F÷&Rç7–Ö&öÇ2ævWB‡Æâæ6Æ72“°¢Væ—Bç7F÷&RææÖW2çG'•÷&W6öÇfR‡7–Ö&öÂææÖRçFW‡B‚’’ÓÒ6öÖR‚$FW&—fVB"¢Ò¢æÖ‡ÇÆçÂÆâæ6Æ72¢æW‡V7B‚'6÷W&6R&F6‚†2FW&—fVB"“°¢Væ—Bæ6Æ76W0¢æ—FW%ö×WB‚¢æf–æB‡ÇÆçÂÆâæ6Æ72ÓÒFW&—fVB¢ææE÷F†Vâ‡ÇÆçÂ°¢ÆâæÖWF†öG2æ—FW%ö×WB‚’æf–æB‡ÆÖWF†öGÂ°¢Væ—Bç7F÷&P¢ææÖW0¢çG'•÷&W6öÇfR‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB†ÖWF†öBæÖWF†öB’ææÖRçFW‡B‚’¢ÓÒ6öÖR‚'fÇVR"¢Ò¢Ò¢æW‡V7B‚$FW&—fVBFV6Æ&W2fÇVR"¢æ&öG’ÒæöæS°¢Ó°¢ÆWB—VÆ–æUöW'&÷"Ò6ö×–ÆU÷6÷W&6Uö&F6…öö'6W'fVB€¢6÷W&6RÀ¢$Ö—76–æt÷fW'&–FT&öG’ç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒ33"’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢6öÖR‚f×WBöÖ—Eö6†–ÆEö&öG’’À¢¢æW‡V7EöW'"‚&6öæ7&WFR÷fW'&–FR6ææ÷BÆ÷6R—G2&öG’"“°¢76W'B€¢ÖF6†W2€¢—VÆ–æUöW'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¤Ö—76–æuG—VD&öG’²ââÒ¢’À¢&&öG–ÆW726öæ7&WFR÷fW'&–FR×W7Bf–Âv—F‚G—VB&öG’W'&÷#¢·—VÆ–æUöW'&÷#£÷Ò ¢“°¢76W'B†÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°§Ğ ¢5·FW7EĞ¦fâ6÷W&6U÷6–ævÆUö–æ†W&—Fæ6UöVÖ—G5öæE÷fW&–f–W5÷F†Uöæö&u÷7WW%ö6†–â‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'6–ævÆRÖ–æ†W&—Fæ6RÖVF—B"“°¢ÆWB6÷W&6RÒ&6Æ72&6R²FVbfÇVR‚“¢–çBÒ²FVbF÷V&ÆVB‚“¢–çBÒfÇVR‚’¢"ÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²&—fFRfÂ÷vã¢–çBÒrÕÆâ#°¢ÆWB'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ%6–ævÆT–æ†W&—Fæ6TVF—Bç66Æ"Â÷WGWBçF‚‚’¢æW‡V7B‚&öæRÖ†÷6÷W&6R–æ†W&—Fæ6R6†÷VÆBVÖ—BF†R6ö×ÆWFR&F6‚"“°¢76W'EöW€¢'F–f7G0¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢²$&6R"Â$FW&—fVB%Ğ¢“°¢ÆWBFW&—fVBÒFV6öFUöVÖ—GFVEö6Æ72‚f'F–f7G5³Ò“°¢76W'EöW€¢FW&—fV@¢æ6öç7FçE÷ööÀ¢æ6Æ75öæÖR†FW&—fVBç7WW%ö6Æ72çVçw&‚’’À¢ö²‚$&6R"¢“°¢ÆWB6öç7G'V7F÷"ÒVÖ—GFVEöÖWF†öEö6öFR‚fFW&—fVBÂ#Æ–æ—Câ"“°¢76W'EöÖWF†öE÷&VfW&Væ6R‚fFW&—fVBÂ6öç7G'V7F÷"Â†#rÂ$&6R"Â#Æ–æ—Câ"Â"‚•b"“°¢76W'Eö÷6öFU÷6WVVæ6R†6öç7G'V7F÷"Âe³ƒ&Â†#rÂƒ&ÂƒÂ†#RÂ†#Ò“°¢76W'Eöf–VÆE÷&VfW&Væ6R‚fFW&—fVBÂ6öç7G'V7F÷"Â†#RÂ$FW&—fVB"Â&÷vâ"Â$’"“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB6Æ77F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’çVçw&‚“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…²"Ö2"Â"×b"Â"Ö6Æ77F‚"Â6Æ77F‚Â$FW&—fVB%Ò¢æ÷WGWB‚¢çVçw&‚“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦f&V¦V7FVBFW&—fVBæ6Æ75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢ÆWB¦fö÷WGWBÒ7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB“°¢76W'B†¦fö÷WGWBæ6öçF–ç2‚'7WW%ö6Æ73¢2"’“°¢76W'B†¦fö÷WGWBæ6öçF–ç2‚"òò&6R"’“°¢76W'B†¦fö÷WGWBæ6öçF–ç2‚$&6RåÂ#Æ–æ—CåÂ#¢‚•b"’“° ¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚$–æ†W&—Fæ6T†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢'V&Æ–26Æ72–æ†W&—Fæ6T†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²FW&—fVBfÇVRÒæWrFW&—fVB‚“²7—7FVÒæ÷WBç&–çFÆâ‡fÇVRçfÇVR‚’²Â#¥Â"²fÇVRæF÷V&ÆVB‚’“²ÒÕÆâ"À¢¢çVçw&‚“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ö6Æ77F‚"Â6Æ77F‚Â"ÖB"Â6Æ77F…Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢çVçw&‚“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVB–æ†W&—Fæ6R†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢6Æ77F‚À¢$–æ†W&—Fæ6T†&æW72"À¢Ò¢æ÷WGWB‚¢çVçw&‚“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVBF†RVÖ—GFVB7WW&6Æ726†–åÆç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW…7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’Â#£#"“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ6÷W&6U÷7V&6Æ75÷&V¦V7G5ö÷fW'&–FW5ööeöf–æÅ÷&VçEöÖWF†öG5÷v—F†÷WEö÷WGWB‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&f–æÂ×&VçBÖÖWF†öBÖ÷fW'&–FR"“°¢ÆWB6÷W&6RÒ&6Æ72&6R²f–æÂFVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²÷fW'&–FRFVbfÇVR‚“¢–çBÒ"ÕÆâ#°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ$f–æÅ&VçDÖWF†öBç66Æ"Â÷WGWBçF‚‚’¢æW‡V7EöW'"‚&6†–ÆB6ææ÷B÷fW'&–FRf–æÂ¥dÒÖWF†öB"“° ¢76W'B€¢ÖF6†W2€¢W'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¤f–æÄ§fÔÖWF†öD÷fW'&–FR°¢&VbæÖRÀ¢&VbFW67&—F÷"À¢âà¢Ò’–bæÖRÓÒ'fÇVR"bbFW67&—F÷"æ5öFW&Vb‚’ÓÒ6öÖR‚"‚”’"¢’À¢&W‡V7FVBG—VBf–æÂÖÖWF†öB÷fW'&–FRW'&÷"Âv÷B¶W'&÷#£÷Ò ¢“°¢76W'B€¢÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’À¢&f–ÆVBf–æÂÖÖWF†öB÷fW'&–FR6†V6²×W7BV&Æ—6‚æò6Æ72f–ÆW2 ¢“°§Ğ ¢5·FW7EĞ¦fâ6÷W&6UöÖWF†öEö6ææ÷E÷&VFVf–æUöåö–æ†W&—FVEövVæW&FVEö66W76÷"‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&–æ†W&—FVBÖ66W76÷"ÖÖWF†öBÖ6öÆÆ—6–öâ"“°¢ÆWB6÷W&6RÒ&6Æ72&6R‡fÂ–C¢–çB•Ææ6Æ72FW&—fVBW‡FVæG2&6Rƒ’²FVb–B‚“¢–çBÒ"ÕÆâ#°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ$–æ†W&—FVD66W76÷$ÖWF†öBç66Æ"Â÷WGWBçF‚‚’¢æW‡V7EöW'"‚&6÷W&6RÖWF†öB6ææ÷B&VFVf–æRvVæW&FVB–æ†W&—FVB66W76÷""“° ¢76W'B€¢ÖF6†W2€¢fW'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¤–æ†W&—FVD§fÔ66W76÷$6öÆÆ—6–öâ°¢÷&–v–ã¢F÷GG•ö§fÓ£¤§fÔ&–æ'”ÖWF†öD÷&–v–ã£¤ÖWF†öB…ò’À¢&VçEö÷&–v–ã¢F÷GG•ö§fÓ£¤§fÔ&–æ'”ÖWF†öD÷&–v–ã£¤66W76÷"°¢&öÆS¢F÷GG•ö§fÓ£¤§fÔ66W76÷$F—7F6…&öÆS£¤vWGFW"À¢âà¢ÒÀ¢æÖRÀ¢FW67&—F÷"À¢âà¢Ò’–bæÖRÓÒ&–B"bbFW67&—F÷"æ5öFW&Vb‚’ÓÒ6öÖR‚"‚”’"¢’À¢&W‡V7FVBG—VB–æ†W&—FVB66W76÷"6öÆÆ—6–öâÂv÷B¶W'&÷#£÷Ò ¢“°¢76W'B†÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°§Ğ ¢5·FW7EĞ¦fâvVæW&FVEö66W76÷'5ö6ææ÷E÷&VFVf–æUö–æ†W&—FVE÷6÷W&6UöÖWF†öE÷6Æ÷G2‚’°¢f÷"†Æ&VÂÂ6÷W&6RÂW‡V7FVEöæÖRÂW‡V7FVEöFW67&—F÷"’–â°¢€¢&æöæf–æÂÖvWGFW""À¢&6Æ72&6R²FVb7W'&VçB‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²fÂ7W'&VçC¢–çBÒ"ÕÆâ"À¢&7W'&VçB"À¢"‚”’"À¢’À¢€¢'6WGFW""À¢&6Æ72&6R²FVb7W'&VçEóÒ‡fÇVS¢–çB“¢Væ—BÒ‚’ÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²f"7W'&VçC¢–çBÒ"ÕÆâ"À¢&7W'&VçEòFW"À¢"„’•b"À¢’À¢Ò°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr†Æ&VÂ“°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ$–æ†W&—FVDÖWF†öD66W76÷"ç66Æ"Â÷WGWBçF‚‚’¢æW‡V7EöW'"‚&vVæW&FVB66W76÷"6ææ÷Bö67W’â–æ†W&—FVB6÷W&6RÖWF†öB6Æ÷B"“°¢76W'B€¢ÖF6†W2€¢fW'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¤–æ†W&—FVD§fÔ66W76÷$6öÆÆ—6–öâ°¢¶–æC¢F÷GG•ö§fÓ£¤§fÔ66W76÷$6öÆÆ—6–öä¶–æC£¤vVæW&FVD66W76÷%&VFVf–æW4–æ†W&—FVDÖWF†öBÀ¢æÖRÀ¢FW67&—F÷"À¢âà¢Ò’–bæÖRÓÒW‡V7FVEöæÖRbbFW67&—F÷"æ5öFW&Vb‚’ÓÒ6öÖR†W‡V7FVEöFW67&—F÷"¢’À¢'¶Æ&VÇÓ¢W‡V7FVBG—VB66W76÷"6öÆÆ—6–öâÂv÷B¶W'&÷#£÷Ò ¢“°¢76W'B€¢÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’À¢'¶Æ&VÇÓ¢æò'F–Â÷WGWB ¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ66W76÷%ö6öÆÆ—6–öå÷fÆ–FF–öå÷&V¦V7G5öåö–æ6ö×F–&ÆUö–æ†W&—FVE÷&W7VÇEöFW67&—F÷"‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&–æ†W&—FVBÖ66W76÷"×&W7VÇBÖFW67&—F÷"ÖÖ—6ÖF6‚"“°¢ÆWB6÷W&6RÒ&6Æ72&6R²FVb7W'&VçB‚“¢ÆöærÒÂÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²fÂ7W'&VçC¢–çBÒ"ÕÆâ#°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ$–æ†W&—FVD66W76÷%&W7VÇBç66Æ"Â÷WGWBçF‚‚’¢æW‡V7EöW'"€¢'6ÖRÖæÖR66W76÷'2v—F‚ÖF6†–ær&ÖWFW'2æBF–ffW&VçB&W7VÇG2&RVç6fR"À¢“° ¢76W'B€¢ÖF6†W2€¢fW'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¤–æ†W&—FVD§fÔ66W76÷$6öÆÆ—6–öâ°¢¶–æC¢F÷GG•ö§fÓ£¤§fÔ66W76÷$6öÆÆ—6–öä¶–æC£¤–æ6ö×F–&ÆT–æ†W&—FVD66W76÷%&W7VÇBÀ¢æÖRÀ¢FW67&—F÷#¢6öÖR†FW67&—F÷"’À¢&VçEöFW67&—F÷#¢6öÖR‡&VçEöFW67&—F÷"’À¢âà¢Ò’–bæÖRÓÒ&7W'&VçB"bbFW67&—F÷"ÓÒ"‚”’"bb&VçEöFW67&—F÷"ÓÒ"‚”¢ ¢’À¢&W‡V7FVBG—VB&W7VÇBÖFW67&—F÷"Ö—6ÖF6‚Âv÷B¶W'&÷#£÷Ò ¢“°¢76W'B†÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°§Ğ ¢5·FW7EĞ¦fâæöæ6öÆÆ–F–æuö–æ†W&—FVEö66W76÷%öæEöF–ffW&VçEöFW67&—F÷%ö÷fW&ÆöE÷75÷&VfÆ–v‡B‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&æöæ6öÆÆ–F–ærÖ–æ†W&—FVBÖ66W76÷""“°¢ÆWB6÷W&6RÒ&6Æ72&6R‡f"7W'&VçC¢–çB•Ææ6Æ72FW&—fVBW‡FVæG2&6Rƒ’²FVb&VB‚“¢–çBÒ²FVb7W'&VçB‡fÇVS¢Æöær“¢–çBÒ"ÕÆâ#°¢ÆWB'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ$æöæ6öÆÆ–F–æt66W76÷"ç66Æ"Â÷WGWBçF‚‚’¢æW‡V7B€¢&F—7F–æ7BFW67&—F÷"÷fW&ÆöBæB÷&F–æ'’ÖWF†öB6öW†—7Bv—F‚–æ†W&—FVB66W76÷'2"À¢“°¢76W'EöW†'F–f7G2æÆVâ‚’Â"“°¢ÆWB&6RÒFV6öFUöVÖ—GFVEö6Æ75öæÖVB‚f'F–f7G2Â$&6R"“°¢ÆWBFW&—fVBÒFV6öFUöVÖ—GFVEö6Æ75öæÖVB‚f'F–f7G2Â$FW&—fVB"“°¢76W'EöW†&6Ræf–VÆG2æÆVâ‚’Â“°¢76W'B†FW&—fVBæf–VÆG2æ—5öV×G’‚’Â&–æ†W&—FVB7F÷&vR7F—2–â&6R"“°¢76W'EöW€¢ÖWF†öG5÷v—F…öFW67&—F÷'2‚fFW&—fVB’À¢°¢‚#Æ–æ—Câ"çFõö÷væVB‚’Â"‚•b"çFõö÷væVB‚’’À¢‚'&VB"çFõö÷væVB‚’Â"‚”’"çFõö÷væVB‚’’À¢‚&7W'&VçB"çFõö÷væVB‚’Â"„¢”’"çFõö÷væVB‚’’À¢Ğ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢fFW&—fVBÀ¢VÖ—GFVEöÖWF†öEö6öFR‚fFW&—fVBÂ#Æ–æ—Câ"’À¢e²ƒ†#rÂ$&6R"Â#Æ–æ—Câ"Â"„’•b"•ÒÀ¢“°§Ğ ¢5·FW7EĞ¦fâ–æ†W&—FVEö66W76÷%ö6öÆÆ—6–öå÷&W÷'G5öæV&W7EöFV6Æ&F–öåö7&÷75÷GvõöVFvW2‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&w&æG&VçBÖ66W76÷"ÖÖWF†öBÖ6öÆÆ—6–öâ"“°¢ÆWB6÷W&6RÒ&6Æ72&6R‡fÂ–C¢–çB•Ææ6Æ72Ö–FFÆRW‡FVæG2&6Rƒ•Ææ6Æ72ÆVbW‡FVæG2Ö–FFÆR²FVb–B‚“¢–çBÒ"ÕÆâ#°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ$w&æG&VçD66W76÷"ç66Æ"Â÷WGWBçF‚‚’¢æW‡V7EöW'"‚&w&æF6†–ÆB6ææ÷B&VFVf–æRw&æG&VçB66W76÷""“°¢76W'B€¢ÖF6†W2€¢fW'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¤–æ†W&—FVD§fÔ66W76÷$6öÆÆ—6–öâ°¢¶–æC¢F÷GG•ö§fÓ£¤§fÔ66W76÷$6öÆÆ—6–öä¶–æC£¥6÷W&6TÖWF†öE&VFVf–æW4–æ†W&—FVD66W76÷"À¢&VçEö÷&–v–ã¢F÷GG•ö§fÓ£¤§fÔ&–æ'”ÖWF†öD÷&–v–ã£¤66W76÷"°¢&öÆS¢F÷GG•ö§fÓ£¤§fÔ66W76÷$F—7F6…&öÆS£¤vWGFW"À¢âà¢ÒÀ¢æÖRÀ¢FW67&—F÷"À¢âà¢Ò’–bæÖRÓÒ&–B"bbFW67&—F÷"æ5öFW&Vb‚’ÓÒ6öÖR‚"‚”’"¢’À¢&W‡V7FVB–æ†W&—FVBw&æG&VçB6öÆÆ—6–öâÂv÷B¶W'&÷#£÷Ò ¢“°¢76W'B†÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°§Ğ ¢5·FW7EĞ¦fâVçÆææVEöf–æÅ÷&VçEöÖWF†öE÷&V¦V7G5ö÷6ÖUöæÖVEö6†–ÆEö66W76÷"‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'VçÆææVBÖf–æÂ×&VçBÖ66W76÷"Ö6öÆÆ—6–öâ"“°¢ÆWB6÷W&6RĞ¢&6Æ72&6R²f–æÂFVb–B‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²fÂ–C¢–çBÒ"ÕÆâ#°¢ÆWB×WBöÖ—E÷&VçEöÖWF†öG2ÒÇVæ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÀ¢öFVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ÆWB&VçBÒVæ—@¢æ6Æ76W0¢æ—FW"‚¢æf–æB‡ÇÆçÂ°¢ÆWB7–Ö&öÂÒVæ—Bç7F÷&Rç7–Ö&öÇ2ævWB‡Æâæ6Æ72“°¢Væ—Bç7F÷&RææÖW2çG'•÷&W6öÇfR‡7–Ö&öÂææÖRçFW‡B‚’’ÓÒ6öÖR‚$&6R"¢Ò¢æÖ‡ÇÆçÂÆâæ6Æ72¢æW‡V7B‚'6÷W&6R&F6‚†2&6R"“°¢Væ—Bæ6Æ76W0¢æ—FW%ö×WB‚¢æf–æB‡ÇÆçÂÆâæ6Æ72ÓÒ&VçB¢æW‡V7B‚$&6RÆâ"¢æÖWF†öG0¢æ6ÆV"‚“°¢Ó°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö&F6…öö'6W'fVB€¢6÷W&6RÀ¢%VçÆææVDf–æÅ&VçD66W76÷"ç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒc’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢6öÖR‚f×WBöÖ—E÷&VçEöÖWF†öG2’À¢¢æW‡V7EöW'"‚&âVçÆææVBf–æÂ6÷W&6RÖWF†öB×W7B&V¦V7B6ÖRÖæÖVB66W76÷""“°¢76W'B€¢ÖF6†W2€¢fW'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¤–æ†W&—FVD§fÔ66W76÷$6öÆÆ—6–öâ°¢¶–æC¢F÷GG•ö§fÓ£¤§fÔ66W76÷$6öÆÆ—6–öä¶–æC£¤vVæW&FVD66W76÷%&VFVf–æW4–æ†W&—FVDf–æÄÖWF†öBÀ¢&VçEö÷&–v–ã¢F÷GG•ö§fÓ£¤§fÔ&–æ'”ÖWF†öD÷&–v–ã£¤ÖWF†öB…ò’À¢æÖRÀ¢FW67&—F÷#¢æöæRÀ¢âà¢Ò’–bæÖRÓÒ&–B ¢’À¢&W‡V7FVB6öç6W'fF—fRVçÆææVBÖf–æÂ6öÆÆ—6–öâÂv÷B¶W'&÷#£÷Ò ¢“°¢76W'B†÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°§Ğ ¢5·FW7EĞ¦fâVçÆææVEöf–æÅ÷&VçE÷6WGFW%÷&V¦V7G5ö÷6ÖUöæÖVEö6†–ÆE÷f"‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'VçÆææVBÖf–æÂ×&VçB×6WGFW"Ö6öÆÆ—6–öâ"“°¢ÆWB6÷W&6RÒ&6Æ72&6R²f–æÂFVb7W'&VçEóÒ‡fÇVS¢–çB“¢Væ—BÒ‚’ÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²f"7W'&VçC¢–çBÒ"ÕÆâ#°¢ÆWB×WBöÖ—E÷&VçEöÖWF†öG2ÒÇVæ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÀ¢öFVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ÆWB&VçBÒVæ—@¢æ6Æ76W0¢æ—FW"‚¢æf–æB‡ÇÆçÂ°¢ÆWB7–Ö&öÂÒVæ—Bç7F÷&Rç7–Ö&öÇ2ævWB‡Æâæ6Æ72“°¢Væ—Bç7F÷&RææÖW2çG'•÷&W6öÇfR‡7–Ö&öÂææÖRçFW‡B‚’’ÓÒ6öÖR‚$&6R"¢Ò¢æÖ‡ÇÆçÂÆâæ6Æ72¢æW‡V7B‚'6÷W&6R&F6‚†2&6R"“°¢Væ—Bæ6Æ76W0¢æ—FW%ö×WB‚¢æf–æB‡ÇÆçÂÆâæ6Æ72ÓÒ&VçB¢æW‡V7B‚$&6RÆâ"¢æÖWF†öG0¢æ6ÆV"‚“°¢Ó°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö&F6…öö'6W'fVB€¢6÷W&6RÀ¢%VçÆææVDf–æÅ&VçE6WGFW"ç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒc’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢6öÖR‚f×WBöÖ—E÷&VçEöÖWF†öG2’À¢¢æW‡V7EöW'"‚&âVçÆææVBf–æÂ6WGFW"×W7B&V¦V7BF†RvVæW&FVB6†–ÆB6WGFW""“°¢76W'B€¢ÖF6†W2€¢fW'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¤–æ†W&—FVD§fÔ66W76÷$6öÆÆ—6–öâ°¢¶–æC¢F÷GG•ö§fÓ£¤§fÔ66W76÷$6öÆÆ—6–öä¶–æC£¤vVæW&FVD66W76÷%&VFVf–æW4–æ†W&—FVDf–æÄÖWF†öBÀ¢&VçEö÷&–v–ã¢F÷GG•ö§fÓ£¤§fÔ&–æ'”ÖWF†öD÷&–v–ã£¤ÖWF†öB…ò’À¢æÖRÀ¢FW67&—F÷#¢æöæRÀ¢âà¢Ò’–bæÖRÓÒ&7W'&VçEòFW ¢’À¢&W‡V7FVB6öç6W'fF—fRVçÆææVBf–æÂ6WGFW"6öÆÆ—6–öâÂv÷B¶W'&÷#£÷Ò ¢“°¢76W'B†÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°§Ğ ¢5·FW7EĞ¦fâVçÆææVEöæöæf–æÅ÷&VçEöÖWF†öE÷&V¦V7G5ö÷6ÖUöæÖVEö6†–ÆEö66W76÷"‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'VçÆææVBÖæöæf–æÂ×&VçBÖ66W76÷"Ö6öÆÆ—6–öâ"“°¢ÆWB6÷W&6RĞ¢&6Æ72&6R²FVb–B‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²fÂ–C¢–çBÒ"ÕÆâ#°¢ÆWB×WBöÖ—E÷&VçEöÖWF†öG2ÒÇVæ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÀ¢öFVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ÆWB&VçBÒVæ—@¢æ6Æ76W0¢æ—FW"‚¢æf–æB‡ÇÆçÂ°¢ÆWB7–Ö&öÂÒVæ—Bç7F÷&Rç7–Ö&öÇ2ævWB‡Æâæ6Æ72“°¢Væ—Bç7F÷&RææÖW2çG'•÷&W6öÇfR‡7–Ö&öÂææÖRçFW‡B‚’’ÓÒ6öÖR‚$&6R"¢Ò¢æÖ‡ÇÆçÂÆâæ6Æ72¢æW‡V7B‚'6÷W&6R&F6‚†2&6R"“°¢Væ—Bæ6Æ76W0¢æ—FW%ö×WB‚¢æf–æB‡ÇÆçÂÆâæ6Æ72ÓÒ&VçB¢æW‡V7B‚$&6RÆâ"¢æÖWF†öG0¢æ6ÆV"‚“°¢Ó°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö&F6…öö'6W'fVB€¢6÷W&6RÀ¢%VçÆææVDæöæf–æÅ&VçD66W76÷"ç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒc"’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢6öÖR‚f×WBöÖ—E÷&VçEöÖWF†öG2’À¢¢æW‡V7EöW'"‚&âVçÆææVBæöâÖf–æÂ6÷W&6RÖWF†öB×W7B&V¦V7B6ÖRÖæÖVB66W76÷""“°¢76W'B€¢ÖF6†W2€¢fW'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¤–æ†W&—FVD§fÔ66W76÷$6öÆÆ—6–öâ°¢¶–æC¢F÷GG•ö§fÓ£¤§fÔ66W76÷$6öÆÆ—6–öä¶–æC£¤vVæW&FVD66W76÷%&VFVf–æW4–æ†W&—FVDÖWF†öBÀ¢&VçEö÷&–v–ã¢F÷GG•ö§fÓ£¤§fÔ&–æ'”ÖWF†öD÷&–v–ã£¤ÖWF†öB…ò’À¢æÖRÀ¢FW67&—F÷#¢æöæRÀ¢&VçEöFW67&—F÷#¢æöæRÀ¢âà¢Ò’–bæÖRÓÒ&–B ¢’À¢&W‡V7FVB6öç6W'fF—fRVçÆææVBÖÖWF†öB6öÆÆ—6–öâÂv÷B¶W'&÷#£÷Ò ¢“°¢76W'B†÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°§Ğ ¢5·FW7EĞ¦fâ6÷W&6U÷7V&6Æ75÷&V¦V7G5ö÷fW'&–FW5ööeöf–æÅöw&æG&VçEöÖWF†öG5÷v—F†÷WEö÷WGWB‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&f–æÂÖw&æG&VçBÖÖWF†öBÖ÷fW'&–FR"“°¢ÆWB6÷W&6RÒ&6Æ72&6R²f–æÂFVbfÇVR‚“¢–çBÒÕÆæ6Æ72Ö–FFÆRW‡FVæG2&6R·ÕÆæ6Æ72ÆVbW‡FVæG2Ö–FFÆR²÷fW'&–FRFVbfÇVR‚“¢–çBÒ"ÕÆâ#°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ$f–æÄw&æG&VçDÖWF†öBç66Æ"Â÷WGWBçF‚‚’¢æW‡V7EöW'"‚&FW66VæFçB6ææ÷B÷fW'&–FRf–æÂÖWF†öBFV6Æ&VB'’âæ6W7F÷""“° ¢76W'B€¢ÖF6†W2€¢W'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¤f–æÄ§fÔÖWF†öD÷fW'&–FR°¢&VbæÖRÀ¢&VbFW67&—F÷"À¢âà¢Ò’–bæÖRÓÒ'fÇVR"bbFW67&—F÷"æ5öFW&Vb‚’ÓÒ6öÖR‚"‚”’"¢’À¢&W‡V7FVB6öçFW‡GVÂf–æÂÖÖWF†öB÷fW'&–FRW'&÷"Âv÷B¶W'&÷#£÷Ò ¢“°¢76W'B€¢÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’À¢&f–ÆVBæ6W7F÷"f–æÂÖÖWF†öB÷fW'&–FR6†V6²×W7BV&Æ—6‚æò6Æ72f–ÆW2 ¢“°§Ğ ¢5·FW7EĞ¦fâ6÷W&6U÷7V&6Æ75÷&WV—&W5ö÷fW'&–FUöf÷%öö6öæ7&WFUöw&æG&VçEöÖWF†öB‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&Ö—76–ærÖw&æG&VçBÖ÷fW'&–FRÖÖöF–f–W""“°¢ÆWB6÷W&6RÒ&6Æ72&6R²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72Ö–FFÆRW‡FVæG2&6R·ÕÆæ6Æ72ÆVbW‡FVæG2Ö–FFÆR²FVbfÇVR‚“¢–çBÒ"ÕÆâ#°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ$Ö—76–ætw&æG&VçD÷fW'&–FRç66Æ"Â÷WGWBçF‚‚’¢æW‡V7EöW'"‚&ÖWF†öB÷fW'&–F–ærâ–æ†W&—FVBw&æG&VçBÖWF†öBæVVG2÷fW'&–FV"“° ¢76W'B€¢ÖF6†W2€¢W'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¤–çfÆ–D§fÔ÷fW'&–FR°¢—77VS¢§fÔ÷fW'&–FT—77VS£¤÷fW'&–FTÖöF–f–W%&WV—&VBÀ¢âà¢Ò¢’À¢&W‡V7FVB6öçFW‡GVÂÖ—76–ærÖ÷fW'&–FRW'&÷"Âv÷B¶W'&÷#£÷Ò ¢“°¢76W'B€¢÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’À¢&â–çfÆ–BG&ç6—F—fR÷fW'&–FR×W7BV&Æ—6‚æò6Æ72f–ÆW2 ¢“°§Ğ ¢5·FW7EĞ¦fâf–æÅ÷&VçEöÖWF†öEöæÖUö6öÆÆ—6–öåöf–Ç5ö6Æ÷6VE÷v†Vå÷&VçEöÖWF†öEö—5÷VçÆææVB‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'VçÆææVBÖf–æÂ×&VçBÖÖWF†öBÖ÷fW'&–FR"“°¢ÆWB6÷W&6RÒ&6Æ72&6R²f–æÂFVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²÷fW'&–FRFVbfÇVR‚“¢–çBÒ"ÕÆâ#°¢ÆWB×WBöÖ—E÷&VçEöÖWF†öG2ÒÇVæ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÀ¢öFVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ÆWB&6RÒVæ—@¢æ6Æ76W0¢æ—FW"‚¢æf–æB‡ÇÆçÂ°¢ÆWB7–Ö&öÂÒVæ—Bç7F÷&Rç7–Ö&öÇ2ævWB‡Æâæ6Æ72“°¢Væ—Bç7F÷&RææÖW2çG'•÷&W6öÇfR‡7–Ö&öÂææÖRçFW‡B‚’’ÓÒ6öÖR‚$&6R"¢Ò¢æÖ‡ÇÆçÂÆâæ6Æ72¢æW‡V7B‚'6÷W&6R&F6‚†2&6R"“°¢Væ—Bæ6Æ76W0¢æ—FW%ö×WB‚¢æf–æB‡ÇÆçÂÆâæ6Æ72ÓÒ&6R¢æW‡V7B‚$&6RÆâ"¢æÖWF†öG0¢æ6ÆV"‚“°¢Ó°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö&F6…öö'6W'fVB€¢6÷W&6RÀ¢$f–æÅ&VçDÖWF†öEVçÆææVBç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒ3#’’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢6öÖR‚f×WBöÖ—E÷&VçEöÖWF†öG2’À¢¢æW‡V7EöW'"‚'VçÆææVBf–æÂ&VçBÖWF†öG2×W7Bæ÷BW&Ö—B6ÖRÖæÖR6†–ÆBÖWF†öB"“° ¢76W'B€¢ÖF6†W2€¢W'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¤f–æÄ§fÔÖWF†öD÷fW'&–FR°¢&VbæÖRÀ¢FW67&—F÷#¢æöæRÀ¢âà¢Ò’–bæÖRÓÒ'fÇVR ¢’À¢&W‡V7FVB6öç6W'fF—fRf–æÂÖÖWF†öB6öÆÆ—6–öâW'&÷"Âv÷B¶W'&÷#£÷Ò ¢“°¢76W'B†÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°§Ğ ¢5·FW7EĞ¦fâG—VE÷7WW%öÖWF†öEö6ÆÇ5öÆ÷vW%÷Fõö6†V6¶VEö–çfö¶W7V6–Â‚’°¢ÆWB6÷W&6RÒ"2 ¦6Æ72&6R°¢&—fFRf"7W'&VçC¢–çBÒ ¢FVbfÇVR‚“¢–çBÒ7W'&Vç@¢FVb–æ7&VÖVçB‚“¢–çBÒ²7W'&VçBÒ7W'&VçB²²7W'&VçBĞ¢FVbFB†Ö÷VçC¢–çB“¢–çBÒ²7W'&VçBÒ7W'&VçB²Ö÷VçC²7W'&VçBĞ¢FVbF–ffW&Væ6R†ÆVgC¢–çBÂ&–v‡C¢–çB“¢–çBÒÆVgBÒ&–v‡@¢FVbVæ&ÆVB†fÆs¢&ööÆVâ“¢&ööÆVâÒfÆp¢FVb–FVçF—G’‚“¢&6RÒF†—0¢FVb&W6WB‚“¢Væ—BÒ²7W'&VçBÒĞ¢FVbF÷V&ÆVB‚“¢–çBÒfÇVR‚’²fÇVR‚§Ğ¦6Æ72FW&—fVBW‡FVæG2&6R°¢÷fW'&–FRFVbfÇVR‚“¢–çBÒ7WW"çfÇVR‚’²¢FVb÷&–v–æÂ‚“¢–çBÒ7WW"çfÇVR‚¢FVbGfæ6R‚“¢–çBÒ7WW"æ–æ7&VÖVçB‚¢FVbFDF—&V7B†Ö÷VçC¢–çB“¢–çBÒ7WW"æFB†Ö÷VçB¢FVbF–ffW&Væ6TF—&V7B†ÆVgC¢–çBÂ&–v‡C¢–çB“¢–çBÒ7WW"æF–ffW&Væ6R†ÆVgBÂ&–v‡B¢FVb÷&FW&VD&wVÖVçG2‚“¢–çBÒ7WW"æF–ffW&Væ6R‡7WW"æ–æ7&VÖVçB‚’Â7WW"æ–æ7&VÖVçB‚’¢FVbVæ&ÆVDF—&V7B†fÆs¢&ööÆVâ“¢&ööÆVâÒ7WW"æVæ&ÆVB†fÆr¢FVb÷&–v–æÄ&6R‚“¢&6RÒ7WW"æ–FVçF—G’‚¢FVb&W6WDF—&V7B‚“¢Væ—BÒ7WW"ç&W6WB‚¢FVb6öæF—F–öæÂ†fÆs¢&ööÆVâ“¢–çBÒ–bfÆrF†Vâ7WW"æFBƒ"’VÇ6R7WW"çfÇVR‚§Ğ¦6Æ72'VææW"°¢FVbWfÇVFR†&6S¢&6R“¢–çBÒ&6RæF÷V&ÆVB‚§Ğ¢"3°¢ÆWBf—'7Eö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'7WW"ÖÖWF†öG2Öf—'7B"“°¢ÆWB6V6öæEö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'7WW"ÖÖWF†öG2×6V6öæB"“°¢ÆWBf—'7BÒ6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ%7WW$ÖWF†öBç66Æ"Âf—'7Eö÷WGWBçF‚‚’’çVçw&‚“°¢ÆWB6V6öæBÒ6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ%7WW$ÖWF†öBç66Æ"Â6V6öæEö÷WGWBçF‚‚’’çVçw&‚“°¢76W'EöW€¢f—'7@¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢²$&6R"Â$FW&—fVB"Â%'VææW"%Ğ¢“°¢76W'EöW€¢f—'7@¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ'—FW2‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢6V6öæ@¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ'—FW2‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢'6W&FRg&öçFVæB6W76–öç2×W7BVÖ—BFWFW&Ö–æ—7F–26Æ72'—FW2æB÷&FW" ¢“° ¢ÆWB&6RÒFV6öFUöVÖ—GFVEö6Æ72€¢f—'7@¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$&6R"¢çVçw&‚’À¢“°¢ÆWBFW&—fVBÒFV6öFUöVÖ—GFVEö6Æ72€¢f—'7@¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$FW&—fVB"¢çVçw&‚’À¢“°¢ÆWB'VææW"ÒFV6öFUöVÖ—GFVEö6Æ72€¢f—'7@¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ%'VææW""¢çVçw&‚’À¢“° ¢76W'EöW€¢FW&—fV@¢æ6öç7FçE÷ööÀ¢æ6Æ75öæÖR†FW&—fVBç7WW%ö6Æ72æW‡V7B‚$FW&—fVB†2F—&V7B&VçB"’’À¢ö²‚$&6R"¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢fFW&—fVBÀ¢VÖ—GFVEöÖWF†öEö6öFR‚fFW&—fVBÂ'fÇVR"’À¢e²ƒ†#rÂ$&6R"Â'fÇVR"Â"‚”’"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢fFW&—fVBÀ¢VÖ—GFVEöÖWF†öEö6öFR‚fFW&—fVBÂ&÷&–v–æÂ"’À¢e²ƒ†#rÂ$&6R"Â'fÇVR"Â"‚”’"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢fFW&—fVBÀ¢VÖ—GFVEöÖWF†öEö6öFR‚fFW&—fVBÂ&Gfæ6R"’À¢e²ƒ†#rÂ$&6R"Â&–æ7&VÖVçB"Â"‚”’"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢fFW&—fVBÀ¢VÖ—GFVEöÖWF†öEö6öFR‚fFW&—fVBÂ&FDF—&V7B"’À¢e²ƒ†#rÂ$&6R"Â&FB"Â"„’”’"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢fFW&—fVBÀ¢VÖ—GFVEöÖWF†öEö6öFR‚fFW&—fVBÂ&F–ffW&Væ6TF—&V7B"’À¢e²ƒ†#rÂ$&6R"Â&F–ffW&Væ6R"Â"„”’”’"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢fFW&—fVBÀ¢VÖ—GFVEöÖWF†öEö6öFR‚fFW&—fVBÂ&÷&FW&VD&wVÖVçG2"’À¢e°¢ƒ†#rÂ$&6R"Â&–æ7&VÖVçB"Â"‚”’"’À¢ƒ†#rÂ$&6R"Â&–æ7&VÖVçB"Â"‚”’"’À¢ƒ†#rÂ$&6R"Â&F–ffW&Væ6R"Â"„”’”’"’À¢ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢fFW&—fVBÀ¢VÖ—GFVEöÖWF†öEö6öFR‚fFW&—fVBÂ&Væ&ÆVDF—&V7B"’À¢e²ƒ†#rÂ$&6R"Â&Væ&ÆVB"Â"…¢•¢"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢fFW&—fVBÀ¢VÖ—GFVEöÖWF†öEö6öFR‚fFW&—fVBÂ&÷&–v–æÄ&6R"’À¢e²ƒ†#rÂ$&6R"Â&–FVçF—G’"Â"‚”Ä&6S²"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢fFW&—fVBÀ¢VÖ—GFVEöÖWF†öEö6öFR‚fFW&—fVBÂ'&W6WDF—&V7B"’À¢e²ƒ†#rÂ$&6R"Â'&W6WB"Â"‚•b"•ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢fFW&—fVBÀ¢VÖ—GFVEöÖWF†öEö6öFR‚fFW&—fVBÂ&6öæF—F–öæÂ"’À¢e°¢ƒ†#rÂ$&6R"Â&FB"Â"„’”’"’À¢ƒ†#rÂ$&6R"Â'fÇVR"Â"‚”’"’À¢ÒÀ¢“°¢76W'B††5÷7F6µöÖ÷F&ÆR‚fFW&—fVBÂ&6öæF—F–öæÂ"’“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f&6RÀ¢VÖ—GFVEöÖWF†öEö6öFR‚f&6RÂ&F÷V&ÆVB"’À¢e°¢ƒ†#bÂ$&6R"Â'fÇVR"Â"‚”’"’À¢ƒ†#bÂ$&6R"Â'fÇVR"Â"‚”’"’À¢ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢g'VææW"À¢VÖ—GFVEöÖWF†öEö6öFR‚g'VææW"Â&WfÇVFR"’À¢e²ƒ†#bÂ$&6R"Â&F÷V&ÆVB"Â"‚”’"•ÒÀ¢“° ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢fW&–g•÷7WW%ö6ÆÇ5÷v—F…ö¦F²†f—'7Eö÷WGWBçF‚‚’“°¢Ğ ¢f÷"†Æ&VÂÂ6÷W&6R’–â°¢€¢&Ö—†–â×VÆ–f–VB×7WW""À¢&6Æ72&6R²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²FVb&VB‚“¢–çBÒ7WW%´&6UÒçfÇVR‚’ÕÆâ"À¢’À¢€¢&÷væW"×VÆ–f–VB×7WW""À¢&6Æ72&6R²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²FVb&VB‚“¢–çBÒ&6Rç7WW"çfÇVR‚’ÕÆâ"À¢’À¢€¢'&ö÷BÖ6Æ72×7WW""À¢&6Æ72&6R²FVb&VB‚“¢–çBÒ7WW"çfÇVR‚’ÕÆâ"À¢’À¢Ò°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr†Æ&VÂ“°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ%7WW$VF—Bç66Æ"Â÷WGWBçF‚‚’¢æW‡V7EöW'"‚'Vç7W÷'FVB7WW"VÆ–f–W"÷"÷væW"×W7Bf–Â&Vf÷&RV&Æ–6F–öâ"“°¢76W'B€¢ÖF6†W2†W'&÷"Â—VÆ–æTf–ÇW&S£¥G—W"…ò’’À¢'¶Æ&VÇÒ×W7B&R&V¦V7FVB'’F†RG—W"Âæ÷BÆ÷vW&VBF‡&÷Vv‚fÆÆ&6³¢¶W'&÷#£÷Ò ¢“°¢76W'B†÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°¢Ğ ¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'VçÆææVB×7WW"ÖÖWF†öB"“°¢ÆWB6÷W&6RÒ&6Æ72&6R²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVBW‡FVæG2&6R²FVb&VB‚“¢–çBÒ7WW"çfÇVR‚’ÕÆâ#°¢ÆWB×WBöÖ—E÷&VçEöÖWF†öG2ÒÇVæ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÀ¢öFVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ÆWB&6RÒVæ—@¢æ6Æ76W0¢æ—FW"‚¢æf–æB‡ÇÆçÂ°¢ÆWB7–Ö&öÂÒVæ—Bç7F÷&Rç7–Ö&öÇ2ævWB‡Æâæ6Æ72“°¢Væ—Bç7F÷&RææÖW2çG'•÷&W6öÇfR‡7–Ö&öÂææÖRçFW‡B‚’’ÓÒ6öÖR‚$&6R"¢Ò¢æÖ‡ÇÆçÂÆâæ6Æ72¢æW‡V7B‚'6÷W&6R&F6‚6öçF–ç2&6R"“°¢Væ—Bæ6Æ76W0¢æ—FW%ö×WB‚¢æf–æB‡ÇÆçÂÆâæ6Æ72ÓÒ&6R¢æW‡V7B‚$&6RÆâ"¢æÖWF†öG0¢æ6ÆV"‚“°¢Ó°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö&F6…öö'6W'fVB€¢6÷W&6RÀ¢%VçÆææVE7WW$ÖWF†öBç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒ3sB’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢6öÖR‚f×WBöÖ—E÷&VçEöÖWF†öG2’À¢¢æW‡V7EöW'"‚&6VÆV7FVBF—&V7B×&VçBÖWF†öB×W7B&R–âF†RVÖ—76–öâÆâ"“°¢76W'B€¢ÖF6†W2€¢W'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ€¢§fÔ6öFVvVäW'&÷#£¤ÖWF†öDæ÷D–ä§fÕÆâ²ââĞ¢Â§fÔ6öFVvVäW'&÷#£¤ÖWF†öDæ÷D–ä§fÕÆäf÷$&öG’²ââĞ¢¢’À¢&âVçÆææVB7WW"F&vWB×W7Bf–Â–ç7FVBöbfÆÆ–ær&6²Fòf—'GVÂF—7F6ƒ¢¶W'&÷#£÷Ò ¢“°¢76W'B†÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’“°§Ğ ¦fâfW&–g•÷7WW%ö6ÆÇ5÷v—F…ö¦F²†÷WGWEöF—&V7F÷'“¢eF‚’°¢ÆWB÷WGWE÷F‚Ò÷WGWEöF—&V7F÷'’çFõ÷7G"‚’æW‡V7B‚%UDbÓ‚÷WGWBF‚"“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"×"À¢"Ö2"À¢"×b"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢$&6R"À¢$FW&—fVB"À¢%'VææW""À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ff÷"F—&V7B×7WW"6Æ76W2"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦f&V¦V7FVBF—&V7B×7WW"6Æ76W5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢ÆWB¦fÒ7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB“°¢f÷"Wf–FVæ6R–â°¢&Ö¦÷"fW'6–öã¢cR"À¢&FW67&—F÷#¢‚”’"À¢&FW67&—F÷#¢„’”’"À¢&FW67&—F÷#¢„”’”’"À¢&FW67&—F÷#¢…¢•¢"À¢&FW67&—F÷#¢‚”Ä&6S²"À¢%7F6´ÖF&ÆR"À¢Ò°¢76W'B€¢¦fæ6öçF–ç2†Wf–FVæ6R’À¢&¦föÖ—GFVB¶Wf–FVæ6S£÷Ó¥Æç¶¦fÒ ¢“°¢Ğ¢ÆWBFW&—fVEö¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…²"×"Â"Ö2"Â"Ö6Æ77F‚"Â÷WGWE÷F‚Â$FW&—fVB%Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦ff÷"FW&—fVBF—&V7B×7WW"ÖWF†öG2"“°¢76W'B€¢FW&—fVEö¦fç7FGW2ç7V66W72‚’À¢&¦f&V¦V7FVBFW&—fVBF—&V7B×7WW"ÖWF†öG5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚fFW&—fVEö¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚fFW&—fVEö¦fç7FFW'"¢“°¢ÆWBFW&—fVEö¦fÒ7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚fFW&—fVEö¦fç7FF÷WB“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢fFW&—fVEö¦fÀ¢'V&Æ–2–çBfÇVR‚“²"À¢e²&ÆöEó"Â&–çfö¶W7V6–Â"Â&–6öç7Eó"Â&–FB"Â&—&WGW&â%ÒÀ¢“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢f¦fÀ¢'V&Æ–2–çB÷&–v–æÂ‚“²"À¢e²&ÆöEó"Â&–çfö¶W7V6–Â"Â&—&WGW&â%ÒÀ¢“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢f¦fÀ¢'V&Æ–2–çBGfæ6R‚“²"À¢e²&ÆöEó"Â&–çfö¶W7V6–Â"Â&—&WGW&â%ÒÀ¢“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢f¦fÀ¢'V&Æ–2–çBFDF—&V7B†–çB“²"À¢e²&ÆöEó"Â&–ÆöEó"Â&–çfö¶W7V6–Â"Â&—&WGW&â%ÒÀ¢“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢f¦fÀ¢'V&Æ–2–çBF–ffW&Væ6TF—&V7B†–çBÂ–çB“²"À¢e²&ÆöEó"Â&–ÆöEó"Â&–ÆöEó""Â&–çfö¶W7V6–Â"Â&—&WGW&â%ÒÀ¢“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢f¦fÀ¢'V&Æ–2–çB÷&FW&VD&wVÖVçG2‚“²"À¢e°¢&ÆöEó"À¢&ÆöEó"À¢&–çfö¶W7V6–Â"À¢&ÆöEó"À¢&–çfö¶W7V6–Â"À¢&–çfö¶W7V6–Â"À¢&—&WGW&â"À¢ÒÀ¢“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R‚f¦fÂ'V&Æ–2&6R–FVçF—G’‚“²"Âe²&ÆöEó"Â&&WGW&â%Ò“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢f¦fÀ¢'V&Æ–2fö–B&W6WDF—&V7B‚“²"À¢e²&ÆöEó"Â&–çfö¶W7V6–Â"Â'&WGW&â%ÒÀ¢“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢f¦fÀ¢'V&Æ–2–çBF÷V&ÆVB‚“²"À¢e°¢&ÆöEó"À¢&–çfö¶Wf—'GVÂ"À¢&ÆöEó"À¢&–çfö¶Wf—'GVÂ"À¢&–FB"À¢&—&WGW&â"À¢ÒÀ¢“°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R€¢f¦fÀ¢'V&Æ–2–çBWfÇVFR„&6R“²"À¢e²&ÆöEó"Â&–çfö¶Wf—'GVÂ"Â&—&WGW&â%ÒÀ¢“° ¢ÆWB†&æW72Ò÷WGWEöF—&V7F÷'’æ¦ö–â‚%7WW$6ÆÇ4†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢"2'V&Æ–2f–æÂ6Æ727WW$6ÆÇ4†&æW72°¢V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’°¢FW&—fVBfÇVRÒæWrFW&—fVB‚“°¢'VææW"'VææW"ÒæWr'VææW"‚“°¢–b‡fÇVRçfÇVR‚’Ò’F‡&÷ræWr76W'F–öäW'&÷"‚&÷fW'&–FRv—F‚7WW"çfÇVR"“°¢–b‡fÇVRæ÷&–v–æÂ‚’Ò’F‡&÷ræWr76W'F–öäW'&÷"‚&F—&V7B&VçB&W7VÇB"“°¢–b‡fÇVRæ÷&–v–æÄ&6R‚’ÒfÇVR’F‡&÷ræWr76W'F–öäW'&÷"‚&F—&V7B&VçB&VfW&Væ6R&W7VÇB"“°¢–b‚fÇVRæVæ&ÆVDF—&V7B‡G'VR’ÇÂfÇVRæVæ&ÆVDF—&V7B†fÇ6R’’F‡&÷ræWr76W'F–öäW'&÷"‚&&ööÆVâ&W7VÇB"“°¢fÇVRç&W6WDF—&V7B‚“°¢–b‡fÇVRæ÷&–v–æÂ‚’Ò’F‡&÷ræWr76W'F–öäW'&÷"‚%Væ—B&W7VÇB"“°¢–b‡fÇVRæGfæ6R‚’ÒÇÂfÇVRçfÇVR‚’Ò"’F‡&÷ræWr76W'F–öäW'&÷"‚&F—&V7B&VçB×WFF–öâ"“°¢–b‡fÇVRæFDF—&V7BƒR’Òb’F‡&÷ræWr76W'F–öäW'&÷"‚&÷&FW&VB–çB&wVÖVçB"“°¢–b‡fÇVRæF–ffW&Væ6TF—&V7Bƒ’ÂB’ÒR’F‡&÷ræWr76W'F–öäW'&÷"‚'6÷W&6R&wVÖVçB÷&FW""“°¢–b‡fÇVRæ÷&FW&VD&wVÖVçG2‚’ÒÓÇÂfÇVRæ÷&–v–æÂ‚’Ò‚’F‡&÷ræWr76W'F–öäW'&÷"‚&÷&FW&VBÂW†7FÇ’Ööæ6R&wVÖVçG2"“°¢–b‡fÇVRæ6öæF—F–öæÂ‡G'VR’ÒÇÂfÇVRæ6öæF—F–öæÂ†fÇ6R’Ò’F‡&÷ræWr76W'F–öäW'&÷"‚&'&æ6†VB7WW"6ÆÇ2"“°¢–b‡'VææW"æWfÇVFR‡fÇVR’Ò#"’F‡&÷ræWr76W'F–öäW'&÷"‚&÷&F–æ'’f—'GVÂF—7F6‚g&öÒ&6R"“°¢–b‡'VææW"æWfÇVFR†æWr&6R‚’’Ò’F‡&÷ræWr76W'F–öäW'&÷"‚&&6Rf—'GVÂF—7F6‚"“°¢7—7FVÒæ÷WBç&–çFÆâ‚&F—&V7B7WW"6ÆÇ2fW&–f–VB"“°¢Ğ§Ğ¢"2À¢¢æW‡V7B‚'w&—FRF—&V7B×7WW"¦f†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…°¢"Ò×&VÆV6R"À¢##"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢"ÖB"À¢÷WGWE÷F‚À¢Ò¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚'7F'B¦f2f÷"F—&V7B×7WW"†&æW72"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVBF—&V7B×7WW"†&æW75Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢÷WGWE÷F‚À¢%7WW$6ÆÇ4†&æW72"À¢Ò¢æ÷WGWB‚¢æW‡V7B‚'7F'B¥dÒfW&–f–W"f÷"F—&V7B×7WW"6Æ76W2"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢&¦fÕ‡fW&–g“¦ÆÂ&V¦V7FVBF—&V7B×7WW"6ÆÇ5Æç7FF÷WC¥Æç·ÕÆç7FFW'#¥Æç·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢&F—&V7B7WW"6ÆÇ2fW&–f–VB ¢“°§Ğ ¢5·FW7EĞ¦fâ&ÖWFW&—¦VE÷&VçEö6öç7G'V7F÷%ö&wVÖVçG5ö¶VW÷F†V—%÷6÷W&6UöÖ–æw2‚’°¢ÆWB6÷W&6U÷FW‡BÒ&6Æ72&6R‡6VVC¢–çB“¥ÆâFVbfÇVR‚“¢–çBÒ6VVEÆåÆæ6Æ72FW&—fVB‡7F'C¢–çB’W‡FVæG2&6R‡7F'B“¥ÆâFVb&VB‚“¢–çBÒ7F'EÆåÆæ6Æ72÷F†W"W‡FVæG2&6Rƒ"’·ÕÆâ#°¢ÆWB6÷W&6Uö–BÒ6÷W&6T–C£¦g&öÕö–æFW‚ƒ3c‚“°¢ÆWB6÷W&6RÒ6÷W&6UFW‡C£¦æWr‡6÷W&6U÷FW‡B’çVçw&‚“°¢ÆWB66ææW"Ò6öçFW‡GVÅ66ææW#£¦æWr‡6÷W&6U÷FW‡B’çVçw&‚“°¢ÆWB×WB7F÷&RÒ6VÖçF–57F÷&S£¦æWr‚“°¢ÆWBFVf–æ—F–öç2ÒFVf–æ—F–öç3£¦&ö÷G7G&‚f×WB7F÷&R“°¢ÆWB'6VBÒ'6Uö6ö×–ÆF–öå÷Væ—B‡6÷W&6RÂ6÷W&6Uö–BÂ66ææW"Âf×WB7F÷&RææÖW2“°¢76W'B‡'6VBæF–væ÷7F–72æ—5öV×G’‚’Â'³£÷Ò"Â'6VBæF–væ÷7F–72“°¢ÆWBG&VT¶–æC£¥6¶vTFVb‡6¶vR’Òg'6VBæ7BævWB‡'6VBç&ö÷B’æ¶–æBVÇ6R°¢æ–2‚&W‡V7FVB6¶vR&ö÷B"“°¢Ó°¢ÆWB×WB6¶vW2Ò6¶vW3£¦æWr‚“°¢ÆWB–æFW‚ÒÖF6‚æÖUö6ö×–ÆF–öå÷Væ—E÷v—F…öF–væ÷7F–72€¢g'6VBæ7BÀ¢'6VBç&ö÷BÀ¢6÷W&6Uö–BÀ¢%&ÖWFW&—¦VE&VçBç66Æ"À¢6÷W&6U'6U7FGW3£¤6ÆVâÀ¢f×WB7F÷&RÀ¢f×WB6¶vW2À¢¢çVçw&‚¢°¢æÖ–æt÷WF6öÖS£¤æÖVB²–æFW‚ÂF–væ÷7F–72ÒÓâ°¢76W'B†F–væ÷7F–72æ—5öV×G’‚’Â'¶F–væ÷7F–73£÷Ò"“°¢–æFW€¢Ğ¢æÖ–æt÷WF6öÖS£¥&V¦V7FVB²F–væ÷7F–72ÒÓâæ–2‚&æÖ–ærf–ÆVC¢¶F–væ÷7F–73£÷Ò"’À¢Ó°¢ÆWB6Æ75öæÖVBÒÇvçFVC¢g7G'Â°¢6¶vP¢ç7FG0¢æ—FW"‚¢æf–æEöÖ‡ÇG&VWÂ°¢ÆWBG&VT¶–æC£¥G—TFVb†FVf–æ—F–öâ’Òg'6VBæ7BævWB‚§G&VR’æ¶–æBVÇ6R°¢&WGW&âæöæS°¢Ó°¢ÆWB7–Ö&öÂÒ–æFW‚ç7–Ö&öÅöB‡6÷W&6Uö–BÂ§G&VR“ó°¢‡7F÷&RææÖW2ç&W6öÇfR†FVf–æ—F–öâææÖRæ5öæÖR‚’çFW‡B‚’’ÓÒvçFVB’çF†Vå÷6öÖR‡7–Ö&öÂ¢Ò¢çVçw&ö÷%öVÇ6R‡ÇÂæ–2‚'6÷W&6RFV6Æ&W2·vçFVGÒ"’¢Ó°¢ÆWB&6RÒ6Æ75öæÖVB‚$&6R"“°¢ÆWBFW&—fVBÒ6Æ75öæÖVB‚$FW&—fVB"“°¢ÆWB÷F†W"Ò6Æ75öæÖVB‚$÷F†W""“°¢ÆWBF÷GG•ö6÷&S£¥6÷W&6TFVf–æ—F–öã£¤6æöæ–6Â°¢6÷W&6RÀ¢G&VS¢FW&—fVE÷G&VRÀ¢ÒÒ–æFW‚æFVf–æ—F–öåööb†FW&—fVB’çVçw&‚¢VÇ6R°¢æ–2‚$FW&—fVB†26æöæ–6Â6÷W&6R&÷fVææ6R"“°¢Ó°¢76W'EöW‡6÷W&6RÂ6÷W&6Uö–B“°¢ÆWBG&VT¶–æC£¥G—TFVb†FW&—fVEöFVf–æ—F–öâ’Òg'6VBæ7BævWB†FW&—fVE÷G&VR’æ¶–æBVÇ6R°¢æ–2‚$FW&—fVB6÷W&6R–FVçF—G’ö–çG2FòG—TFVb"“°¢Ó°¢ÆWBG&VT¶–æC£¥FV×ÆFR‡FV×ÆFR’Òg'6VBæ7BævWB†FW&—fVEöFVf–æ—F–öâç&‡2’æ¶–æBVÇ6R°¢æ–2‚$FW&—fVB†2FV×ÆFR"“°¢Ó°¢ÆWB·&VçEöÆ–6F–öåÒÒFV×ÆFRç&VçG2æ5÷6Æ–6R‚’VÇ6R°¢æ–2‚&W‡V7FVBöæR6÷W&6R&VçBÆ–6F–öâ"“°¢Ó°¢ÆWBG&VT¶–æC£¤Ç’†Æ–6F–öâ’Òg'6VBæ7BævWB‚§&VçEöÆ–6F–öâ’æ¶–æBVÇ6R°¢æ–2‚$&6R‡7F'B’&VÖ–ç2&VçB6öç7G'V7F÷"Æ–6F–öâ"“°¢Ó°¢76W'EöW†Æ–6F–öâæ&w2æÆVâ‚’Â“°¢ÆWBG&VT¶–æC£¥6VÆV7B†6öç7G'V7F÷%÷6VÆV7B’Òg'6VBæ7BævWB†Æ–6F–öâægVæ7F–öâ’æ¶–æBVÇ6R°¢æ–2‚'&VçBÆ–6F–öâ6VÆV7G2Æ–æ—Câ"“°¢Ó°¢76W'EöW€¢7F÷&RææÖW2ç&W6öÇfR†6öç7G'V7F÷%÷6VÆV7BææÖRçFW‡B‚’’À¢#Æ–æ—Câ ¢“°¢ÆWBG&VT¶–æC£¤æWr‡&VçEöæWr’Òg'6VBæ7BævWB†6öç7G'V7F÷%÷6VÆV7BçVÆ–f–W"’æ¶–æBVÇ6R°¢æ–2‚&6öç7G'V7F÷"6VÆV7F–öâ—2VÆ–f–VB'’æWr"“°¢Ó°¢ÆWBG&VT¶–æC£¤–FVçB‡&VçE÷G—R’Òg'6VBæ7BævWB‡&VçEöæWrçGB’æ¶–æBVÇ6R°¢æ–2‚$&6R—2&W&W6VçFVB'’—G26÷W&6RG—R–FVçF–f–W""“°¢Ó°¢76W'EöW‡7F÷&RææÖW2ç&W6öÇfR‡&VçE÷G—RææÖRçFW‡B‚’’Â$&6R"“°¢ÆWBG&VT¶–æC£¤–FVçB‡&VçEö&wVÖVçB’Òg'6VBæ7BævWB†Æ–6F–öâæ&w5³Ò’æ¶–æBVÇ6R°¢æ–2‚'7F'B&VÖ–ç2F†R&VçB&wVÖVçBW‡&W76–öâ"“°¢Ó°¢76W'EöW‡7F÷&RææÖW2ç&W6öÇfR‡&VçEö&wVÖVçBææÖRçFW‡B‚’’Â'7F'B"“° ¢ÆWBG&VT¶–æC£¤FVdFVb†6öç7G'V7F÷"’Òg'6VBæ7BævWB‡FV×ÆFRæ6öç7G'V7F÷"’æ¶–æBVÇ6R°¢æ–2‚'FV×ÆFR÷vç2—G2&–Ö'’6öç7G'V7F÷""“°¢Ó°¢ÆWB·7F'E÷&ÖWFW%ÒÒ6öç7G'V7F÷"çfÇVU÷&Õö6ÆW6W5³Òæ5÷6Æ–6R‚’VÇ6R°¢æ–2‚$FW&—fVB&–Ö'’6öç7G'V7F÷"†2öæRf÷&ÖÂ&ÖWFW""“°¢Ó°¢ÆWB6öç7G'V7F÷%÷7–Ö&öÂÒ–æFW‚ç7–Ö&öÅöB‡6÷W&6Uö–BÂFV×ÆFRæ6öç7G'V7F÷"’çVçw&‚“°¢ÆWB7F'E÷7–Ö&öÂÒ–æFW€¢æFW&—fVE÷7–Ö&öÅöB†6öç7G'V7F÷%÷7–Ö&öÂÂ6÷W&6Uö–BÂ§7F'E÷&ÖWFW"¢æW‡V7B‚&6öç7G'V7F÷"f÷&ÖÂ†2â÷væW"×66÷VBFW&—fVB–FVçF—G’"“°¢76W'EöW€¢7F÷&Rç7–Ö&öÇ2ævWB‡7F'E÷7–Ö&öÂ’æ÷væW"À¢6öÖR†6öç7G'V7F÷%÷7–Ö&öÂ¢“°¢76W'EöW‡7F÷&Rç7–Ö&öÇ2ævWB‡7F'E÷7–Ö&öÂ’æ¶–æBÂ7–Ö&öÄ¶–æC£¥&ÖWFW"“° ¢ÆWB×WBG—W"Ò6÷W&6UG—W#£¦æWr€¢g'6VBæ7BÀ¢6÷W&6Uö–BÀ¢f–æFW‚À¢f×WB7F÷&RÀ¢FVf–æ—F–öç2À¢g6¶vW2À¢“°¢G—W"æ6ö×ÆWFU÷7–Ö&öÂ†FW&—fVB’çVçw&‚“°¢G—W"æ6ö×ÆWFU÷7–Ö&öÂ†÷F†W"’çVçw&‚“°¢ÆWB6Æ75ö–æfòÒÖF6‚G—W"ç7F÷&R‚’ç7–Ö&öÇ2æ–æfò†FW&—fVB’°¢F÷GG•ö6÷&S£¥7–Ö&öÄ–æfó£¤6ö×ÆWFR†–æfò’Óâ¦–æfòÀ¢÷F†W"Óâæ–2‚$FW&—fVB6Æ72–æfò—26ö×ÆWFS¢¶÷F†W#£÷Ò"’À¢Ó°¢ÆWBG—S£¤6Æ74–æfò†6Æ75ö–æfò’ÒG—W"ç7F÷&R‚’çG—W2ævWB†6Æ75ö–æfò’VÇ6R°¢æ–2‚$FW&—fVB†26Æ74–æfò"“°¢Ó°¢76W'EöW†6Æ75ö–æfòç&VçG2æÆVâ‚’Â“°¢76W'B†ÖF6†W2€¢G—W"ç7F÷&R‚’çG—W2ævWB†6Æ75ö–æfòç&VçG5³Ò’À¢G—S£¥G—U&Vb°¢F&vWC¢G—U&VeF&vWC£¥7–Ö&öÂ‡&VçB’À¢âà¢Ò–b§&VçBÓÒ&6P¢’“°¢76W'B€¢G—W ¢ç6÷W&6U÷G—VEö–æFW‚‚¢ævWB‡6÷W&6Uö–BÂ§&VçEöÆ–6F–öâ¢æ—5÷6öÖR‚¢“°¢76W'B€¢G—W ¢ç6÷W&6U÷G—VEö–æFW‚‚¢ævWB‡6÷W&6Uö–BÂÆ–6F–öâæ&w5³Ò¢æ—5÷6öÖR‚¢“° ¢ÆWBF÷GG•ö6÷&S£¥6÷W&6TFVf–æ—F–öã£¤6æöæ–6Â°¢G&VS¢÷F†W%÷G&VRÂâà¢ÒÒ–æFW‚æFVf–æ—F–öåööb†÷F†W"’çVçw&‚¢VÇ6R°¢æ–2‚$÷F†W"†26æöæ–6Â6÷W&6R&÷fVææ6R"“°¢Ó°¢ÆWBG&VT¶–æC£¥G—TFVb†÷F†W%öFVf–æ—F–öâ’Òg'6VBæ7BævWB†÷F†W%÷G&VR’æ¶–æBVÇ6R°¢æ–2‚$÷F†W"6÷W&6R–FVçF—G’ö–çG2FòG—TFVb"“°¢Ó°¢ÆWBG&VT¶–æC£¥FV×ÆFR†÷F†W%÷FV×ÆFR’Òg'6VBæ7BævWB†÷F†W%öFVf–æ—F–öâç&‡2’æ¶–æBVÇ6R°¢æ–2‚$÷F†W"†2FV×ÆFR"“°¢Ó°¢ÆWB¶÷F†W%÷&VçEöÆ–6F–öåÒÒ÷F†W%÷FV×ÆFRç&VçG2æ5÷6Æ–6R‚’VÇ6R°¢æ–2‚$÷F†W"†2öæR&VçBÆ–6F–öâ"“°¢Ó°¢ÆWB÷F†W%÷G—VEöÆ–6F–öâÒG—W ¢ç6÷W&6U÷G—VEö–æFW‚‚¢ævWB‡6÷W&6Uö–BÂ¦÷F†W%÷&VçEöÆ–6F–öâ¢çVçw&‚“°¢ÆWBG&VT¶–æC£¤Ç’†÷F†W%÷G—VEöÇ’’ÒgG—W"çG—VEö7B‚’ævWB†÷F†W%÷G—VEöÆ–6F–öâ’æ¶–æ@¢VÇ6R°¢æ–2‚$÷F†W"&VçB6ÆÂ†2G—VBÇ’"“°¢Ó°¢ÆWB÷F†W%÷G—VEö&wVÖVçBÒ÷F†W%÷G—VEöÇ’æ&w5³Ó°¢ÆWB6öç7G'V7F÷%÷ÆâÒ§fÕ&–Ö'”6öç7G'V7F÷%Æã£¦g&öÕ÷6÷W&6R€¢FW&—fVBÀ¢6öç7G'V7F÷%÷7–Ö&öÂÀ¢fV2·7F'E÷7–Ö&öÅÒÀ¢g'6VBæ7BÀ¢f–æFW‚À¢¢çVçw&‚“°¢ÆWBW'&÷"Ò6öç7G'V7F÷%÷Æà¢çv—F…öF—&V7E÷7WW%ö6öç7G'V7F÷"€¢§&VçEöÆ–6F–öâÀ¢÷F†W%÷G—VEöÆ–6F–öâÀ¢fV2¶F÷GG•ö§fÓ£¤§fÔ6öç7G'V7F÷$&wVÖVçD&–æF–ær°¢6÷W&6S¢Æ–6F–öâæ&w5³ÒÀ¢G—VC¢÷F†W%÷G—VEö&wVÖVçBÀ¢ÕÒÀ¢g'6VBæ7BÀ¢f–æFW‚À¢G—W"ç6÷W&6U÷G—VEö–æFW‚‚’À¢G—W"ç7F÷&R‚’À¢G—W"çG—VEö7B‚’À¢¢çVçw&öW'"‚“°¢76W'B†ÖF6†W2€¢W'&÷"À¢§fÔ6öFVvVäW'&÷#£¤–çfÆ–E&–Ö'”6öç7G'V7F÷"°¢—77VS¢F÷GG•ö§fÓ£¤§fÔ6öç7G'V7F÷$—77VS£¥&VçD6öç7G'V7F÷$&wVÖVçD–FVçF—G’À¢âà¢Ğ¢’“°§Ğ ¢5·FW7EĞ¦fâ&ÖWFW&—¦VE÷&VçEö6öç7G'V7F÷%ö—5öVÖ—GFVEög&öÕ÷F†U÷6÷W&6U÷&ööb‚’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'&ÖWFW&—¦VB×&VçBÖ6öç7G'V7F÷"×7W÷'FVB"“°¢ÆWB6÷W&6RÒ&6Æ72&6R‡6VVC¢–çB’·ÕÆæ6Æ72FW&—fVB‡7F'C¢–çB’W‡FVæG2&6R‡7F'B’·ÕÆâ#°¢ÆWB'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ%&ÖWFW&—¦VE&VçBç66Æ"Â÷WGWBçF‚‚’¢æW‡V7B‚'F†R6÷W&6R&ööb6VÆV7G2F†R6æöæ–6Â&ÖWFW&—¦VB&VçB6öç7G'V7F÷""“°¢ÆWBFW&—fVBÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$FW&—fVB"¢æW‡V7B‚$FW&—fVB6Æ72v2VÖ—GFVB"“°¢ÆWB6Æ72ÒFV6öFUöVÖ—GFVEö6Æ72†FW&—fVB“°¢ÆWB6öç7G'V7F÷"ÒVÖ—GFVEöÖWF†öEö6öFR‚f6Æ72Â#Æ–æ—Câ"“°¢76W'EöÖWF†öE÷&VfW&Væ6R‚f6Æ72Â6öç7G'V7F÷"Â†#rÂ$&6R"Â#Æ–æ—Câ"Â"„’•b"“°§Ğ ¢5·FW7EĞ¦fâ6÷W&6UöÆö6ÅöFV6Æ&F–öç5öW‡÷'EöÖWF†öE÷66÷VEö6æöæ–6Åö&–æF–æw2‚’°¢ÆWB6÷W&6U÷FW‡BÒ&6Æ722µÆâFVbf—'7B‡ƒ¢–çB“¢–çBÒ²fÂfÇVRÒƒ²²fÂfÇVRÒfÇVS²fÇVRÒÕÆâFVb6V6öæB‡ƒ¢–çB“¢–çBÒ²fÂfÇVRÒƒ²fÇVRÕÆçÒ#°¢ÆWB6÷W&6Uö–BÒ6÷W&6T–C£¦g&öÕö–æFW‚ƒ“°¢ÆWB6÷W&6RÒ6÷W&6UFW‡C£¦æWr‡6÷W&6U÷FW‡B’çVçw&‚“°¢ÆWB66ææW"Ò6öçFW‡GVÅ66ææW#£¦æWr‡6÷W&6U÷FW‡B’çVçw&‚“°¢ÆWB×WB7F÷&RÒ6VÖçF–57F÷&S£¦æWr‚“°¢ÆWBFVf–æ—F–öç2ÒFVf–æ—F–öç3£¦&ö÷G7G&‚f×WB7F÷&R“°¢ÆWB'6VBÒ'6Uö6ö×–ÆF–öå÷Væ—B‡6÷W&6RÂ6÷W&6Uö–BÂ66ææW"Âf×WB7F÷&RææÖW2“°¢76W'B‡'6VBæF–væ÷7F–72æ—5öV×G’‚’Â'³£÷Ò"Â'6VBæF–væ÷7F–72“°¢ÆWB×WB6¶vW2ÒF÷GG•ö6÷&S£¥6¶vW3£¦æWr‚“°¢ÆWB–æFW‚ÒÖF6‚æÖUö6ö×–ÆF–öå÷Væ—E÷v—F…öF–væ÷7F–72€¢g'6VBæ7BÀ¢'6VBç&ö÷BÀ¢6÷W&6Uö–BÀ¢$Æö6Ä&–æF–æw2ç66Æ"À¢6÷W&6U'6U7FGW3£¤6ÆVâÀ¢f×WB7F÷&RÀ¢f×WB6¶vW2À¢¢çVçw&‚¢°¢æÖ–æt÷WF6öÖS£¤æÖVB²–æFW‚ÂF–væ÷7F–72Ò–bF–væ÷7F–72æ—5öV×G’‚’Óâ–æFW‚À¢÷F†W"Óâæ–2‚'6÷W&6R6†÷VÆB&RæÖVBv—F†÷WBF–væ÷7F–73¢¶÷F†W#£÷Ò"’À¢Ó°¢ÆWB†6Æ75÷7–Ö&öÂÂFV×ÆFU÷G&VR’Ğ¢f–æE÷6–ævÆU÷6÷W&6Uö6Æ72‚g'6VBæ7BÂ'6VBç&ö÷BÂ6÷W&6Uö–BÂf–æFW‚Âg7F÷&R’çVçw&‚“°¢ÆWBG&VT¶–æC£¥FV×ÆFR‡FV×ÆFR’Òg'6VBæ7BævWB‡FV×ÆFU÷G&VR’æ¶–æBVÇ6R°¢æ–2‚'6÷W&6R6Æ726†÷VÆB†fRFV×ÆFR"¢Ó°¢ÆWB6÷W&6UöÖWF†öG2Ğ¢6öÆÆV7E÷6÷W&6UöÖWF†öG2‚g'6VBæ7BÂgFV×ÆFRæ&öG’Â6÷W&6Uö–BÂf–æFW‚Âg7F÷&R’çVçw&‚“°¢ÆWB×WBG—W"Ò6÷W&6UG—W#£¦æWr€¢g'6VBæ7BÀ¢6÷W&6Uö–BÀ¢f–æFW‚À¢f×WB7F÷&RÀ¢FVf–æ—F–öç2À¢g6¶vW2À¢“°¢G—W"æ6ö×ÆWFU÷7–Ö&öÂ†6Æ75÷7–Ö&öÂ’çVçw&‚“°¢ÆWB×WBÖWF†öE÷Æç2ÒfV3£¦æWr‚“°¢f÷"6÷W&6UöÖWF†öB–â6÷W&6UöÖWF†öG2°¢G—W"æ6ö×ÆWFU÷7–Ö&öÂ‡6÷W&6UöÖWF†öBç7–Ö&öÂ’çVçw&‚“°¢ÆWB&‡2Ò6÷W&6UöÖWF†öBç&‡2çVçw&‚“°¢ÆWB6öçFW‡BÒG—W"æW‡&W76–öåö6öçFW‡Eöf÷"‡6÷W&6UöÖWF†öBç7–Ö&öÂ’çVçw&‚“°¢ÆWB&öG’ÒG—W"çG—UöW‡&W76–öâ‡&‡2Â6öçFW‡B’çVçw&‚“°¢ÆWB×WBÆö6Ç2Ò'6V@¢æ7@¢æ—FW"‚¢æf–ÇFW%öÖ‡Â‡6÷W&6U÷G&VRÂæöFR—Â°¢–bÖF6†W2†æöFRæ¶–æBÂG&VT¶–æC£¥fÄFVb…ò’’°¢&WGW&âæöæS°¢Ğ¢ÆWB&–æF–ærÒG—W"çG—VEöÆö6Åö&–æF–æuöB‡6÷W&6Uö–BÂ6÷W&6U÷G&VR“ó°¢‡G—W"ç7F÷&R‚’ç7–Ö&öÇ2ævWB†&–æF–ærç7–Ö&öÂ‚’’æ÷væW"ÓÒ6öÖR‡6÷W&6UöÖWF†öBç7–Ö&öÂ’¢çF†Vå÷6öÖR„§fÔÆö6Ä&–æF–ær°¢FV6Æ&F–öã¢&–æF–æræFV6Æ&F–öâ‚’À¢7–Ö&öÃ¢&–æF–ærç7–Ö&öÂ‚’À¢×WF&ÆS¢G—W ¢ç7F÷&R‚¢ç7–Ö&öÇ0¢ævWB†&–æF–ærç7–Ö&öÂ‚’¢æfÆw0¢æ6öçF–ç2…7–Ö&öÄfÆw3£¤ÕUD$ÄR’À¢Ò¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢Æö6Ç2ç6÷'Eö'•ö¶W’‡Æ&–æF–æwÂ&–æF–æræFV6Æ&F–öâæ–æFW‚‚’“°¢76W'EöW†Æö6Ç2æÆVâ‚’Â–bÖWF†öE÷Æç2æ—5öV×G’‚’²"ÒVÇ6R²Ò“°¢76W'B€¢Æö6Ç0¢çv–æF÷w2ƒ"¢æÆÂ‡Ç—'Â—%³ÒæFV6Æ&F–öâæ–æFW‚‚’Â—%³ÒæFV6Æ&F–öâæ–æFW‚‚’¢“°¢ÖWF†öE÷Æç2çW6‚„§fÔÖWF†öEÆâ°¢ÖWF†öC¢6÷W&6UöÖWF†öBç7–Ö&öÂÀ¢&ÖWFW'3¢6÷W&6UöÖWF†öBç&ÖWFW'2À¢Æö6Ç2À¢&öG“¢6öÖR†&öG’’À¢Ò“°¢Ğ¢76W'EöW†ÖWF†öE÷Æç2æÆVâ‚’Â"“°¢76W'EöW†ÖWF†öE÷Æç5³ÒæÆö6Ç2æÆVâ‚’Â"“°¢76W'EöW†ÖWF†öE÷Æç5³ÒæÆö6Ç2æÆVâ‚’Â“°¢76W'EöæR€¢ÖWF†öE÷Æç5³ÒæÆö6Ç5³Òç7–Ö&öÂÀ¢ÖWF†öE÷Æç5³ÒæÆö6Ç5³Òç7–Ö&öÀ¢“°¢76W'EöæR€¢ÖWF†öE÷Æç5³ÒæÆö6Ç5³ÒæFV6Æ&F–öâÀ¢ÖWF†öE÷Æç5³ÒæÆö6Ç5³ÒæFV6Æ&F–öà¢“°¢ÆWBVæ—BÒ§fÔ6ö×–ÆF–öåVæ—B°¢7F÷&S¢G—W"ç7F÷&R‚’À¢G—VEö7C¢G—W"çG—VEö7B‚’À¢6Æ76W3¢fV2´§fÔ6Æ75Æâ°¢6Æ73¢6Æ75÷7–Ö&öÂÀ¢f–VÆG3¢fV3£¦æWr‚’À¢ÖWF†öG3¢ÖWF†öE÷Æç2À¢–æ—F–Æ—¦F–öã¢§fÔ6Æ74–æ—F–Æ—¦F–öåÆã£¥Vç7W÷'FVBÀ¢ÕÒÀ¢Ó°¢76W'EöW‡Væ—BçfÆ–FFR‚’Âö²‚‚’’“°§Ğ ¢5·FW7EĞ¦fâ7WW%ö6ÆÇ5öf—‡GW&Uö6öÖ&–æW5÷&ÖWFW&—¦VEö6öç7G'V7F–öåöæE÷7V6–ÅöF—7F6‚‚’°¢ÆWBf—'7Eö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'7WW"Ö6ÆÇ2Öf—'7B"“°¢ÆWB6V6öæEö÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'7WW"Ö6ÆÇ2×6V6öæB"“°¢ÆWBf—'7BÒ6ö×–ÆUöf—‡GW&Uö6Æ76W2‚%7WW$6ÆÇ2ç66Æ"Âf—'7Eö÷WGWBçF‚‚’’çVçw&‚“°¢ÆWB6V6öæBÒ6ö×–ÆUöf—‡GW&Uö6Æ76W2‚%7WW$6ÆÇ2ç66Æ"Â6V6öæEö÷WGWBçF‚‚’’çVçw&‚“°¢76W'EöW€¢f—'7@¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢²$&6R"Â$FW&—fVB"Â%'VææW"%Ğ¢“°¢76W'EöW€¢f—'7@¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ†'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’Â'F–f7Bæ6Æ72æ'—FW2‚’’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢6V6öæ@¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ†'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’Â'F–f7Bæ6Æ72æ'—FW2‚’’¢æ6öÆÆV7C££ÅfV3Åóãâ‚¢“°¢f÷"†'F–f7G2Â÷WGWB’–â²‚ff—'7BÂff—'7Eö÷WGWB’Â‚g6V6öæBÂg6V6öæEö÷WGWB•Ò°¢76W'EöW†÷WGWBæ6Æ75öf–ÆW2‚’æÆVâ‚’Â2“°¢f÷"'F–f7B–â'F–f7G2°¢76W'EöW†g3£§&VB‚f'F–f7BçF‚’çVçw&‚’Â'F–f7Bæ6Æ72æ'—FW2‚’“°¢ÆWB6Æ72ÒFV6öFUöVÖ—GFVEö6Æ72†'F–f7B“°¢76W'B†6Æ72æ66W75öfÆw2æ—5÷V&Æ–2‚’“°¢76W'B€¢6Æ70¢æ66W75öfÆw0¢æ6öçF–ç2†F÷GG•ö6Æ76f–ÆS£¦66W75öfÆw3£¤45õ5UU"¢“°¢76W'B€¢6Æ70¢æÖWF†öG0¢æ—FW"‚¢æÆÂ‡ÆÖWF†öGÂÖWF†öBæ66W75öfÆw2æ—5÷V&Æ–2‚¢bbÖWF†öBæ66W75öfÆw2æ—5÷7FF–2‚’¢“°¢f÷"†æÖRÂò’–âÖWF†öG5÷v—F…öFW67&—F÷'2‚f6Æ72’°¢76W'B€¢†5÷7F6µöÖ÷F&ÆR‚f6Æ72ÂfæÖR’À¢'F†Rf—‡GW&R—2'&æ6†ÆW72 ¢“°¢Ğ¢Ğ¢Ğ¢ÆWB&6RÒFV6öFUöVÖ—GFVEö6Æ72‚ff—'7E³Ò“°¢ÆWBFW&—fVBÒFV6öFUöVÖ—GFVEö6Æ72‚ff—'7E³Ò“°¢ÆWB'VææW"ÒFV6öFUöVÖ—GFVEö6Æ72‚ff—'7E³%Ò“°¢f÷"†6Æ72Â&VçBÂFW67&—F÷"’–â°¢‚f&6RÂ&¦föÆærôö&¦V7B"Â"„’•b"’À¢‚fFW&—fVBÂ$&6R"Â"„’•b"’À¢‚g'VææW"Â&¦föÆærôö&¦V7B"Â"‚•b"’À¢Ò°¢76W'EöW€¢6Æ72æ6öç7FçE÷ööÂæ6Æ75öæÖR†6Æ72ç7WW%ö6Æ72çVçw&‚’’À¢ö²‡&VçB¢“°¢76W'EöW€¢ÖWF†öG5÷v—F…öFW67&—F÷'2†6Æ72¢æ–çFõö—FW"‚¢æf–ÇFW"‡Â†æÖRÂò—ÂæÖRÓÒ#Æ–æ—Câ"¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢²‚#Æ–æ—Câ"çFõö÷væVB‚’ÂFW67&—F÷"çFõö÷væVB‚’•Ğ¢“°¢Ğ¢76W'EöW†&6Ræf–VÆG2æÆVâ‚’Â“°¢ÆWB7W'&VçBÒf&6Ræf–VÆG5³Ó°¢76W'EöW†&6Ræ6öç7FçE÷ööÂçWFc‚†7W'&VçBææÖUö–æFW‚’Âö²‚&7W'&VçB"’“°¢76W'EöW†&6Ræ6öç7FçE÷ööÂçWFc‚†7W'&VçBæFW67&—F÷%ö–æFW‚’Âö²‚$’"’“°¢76W'B†7W'&VçBæ66W75öfÆw2æ—5÷&—fFR‚’“°¢76W'B‚7W'&VçBæ66W75öfÆw2æ—5öf–æÂ‚’“°¢76W'B†FW&—fVBæf–VÆG2æ—5öV×G’‚’“°¢76W'B‡'VææW"æf–VÆG2æ—5öV×G’‚’“°¢76W'Eö÷6öFU÷6WVVæ6R€¢VÖ—GFVEöÖWF†öEö6öFR‚f&6RÂ#Æ–æ—Câ"’À¢e³ƒ&Â†#rÂƒ&Âƒ"Â†#RÂ†#ÒÀ¢“°¢76W'Eöf–VÆE÷&VfW&Væ6R€¢f&6RÀ¢VÖ—GFVEöÖWF†öEö6öFR‚f&6RÂ#Æ–æ—Câ"’À¢†#RÀ¢$&6R"À¢&7W'&VçB"À¢$’"À¢“°¢76W'Eö÷6öFU÷6WVVæ6R€¢VÖ—GFVEöÖWF†öEö6öFR‚fFW&—fVBÂ#Æ–æ—Câ"’À¢e³ƒ&Âƒ"Â†#rÂ†#ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢fFW&—fVBÀ¢VÖ—GFVEöÖWF†öEö6öFR‚fFW&—fVBÂ#Æ–æ—Câ"’À¢e²ƒ†#rÂ$&6R"Â#Æ–æ—Câ"Â"„’•b"•ÒÀ¢“°¢76W'Eö÷6öFU÷6WVVæ6R€¢VÖ—GFVEöÖWF†öEö6öFR‚fFW&—fVBÂ'fÇVR"’À¢e³ƒ&Â†#rÂƒBÂƒcÂ†5ÒÀ¢“°¢f÷"†ÖWF†öBÂF&vWB’–â°¢‚'fÇVR"Â'fÇVR"’À¢‚&÷&–v–æÂ"Â'fÇVR"’À¢‚&Gfæ6R"Â&–æ7&VÖVçB"’À¢Ò°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢fFW&—fVBÀ¢VÖ—GFVEöÖWF†öEö6öFR‚fFW&—fVBÂÖWF†öB’À¢e²ƒ†#rÂ$&6R"ÂF&vWBÂ"‚”’"•ÒÀ¢“°¢Ğ¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢f&6RÀ¢VÖ—GFVEöÖWF†öEö6öFR‚f&6RÂ&F÷V&ÆVB"’À¢e°¢ƒ†#bÂ$&6R"Â'fÇVR"Â"‚”’"’À¢ƒ†#bÂ$&6R"Â'fÇVR"Â"‚”’"’À¢ÒÀ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢g'VææW"À¢VÖ—GFVEöÖWF†öEö6öFR‚g'VææW"Â&WfÇVFR"’À¢e²ƒ†#bÂ$&6R"Â&F÷V&ÆVB"Â"‚”’"•ÒÀ¢“°¢f÷"†ÖWF†öBÂ÷væW"’–â²‚&g&öÔ&6R"Â$&6R"’Â‚&g&öÔFW&—fVB"Â$FW&—fVB"•Ò°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢g'VææW"À¢VÖ—GFVEöÖWF†öEö6öFR‚g'VææW"ÂÖWF†öB’À¢e°¢ƒ†#rÂ÷væW"Â#Æ–æ—Câ"Â"„’•b"’À¢ƒ†#bÂ%'VææW""Â&WfÇVFR"Â"„Ä&6S²”’"’À¢ÒÀ¢“°¢Ğ¢f÷"†6Æ72ÂÖWF†öBÂ7F6²ÂÆö6Ç2’–â°¢‚f&6RÂ#Æ–æ—Câ"Â"Â"’À¢‚fFW&—fVBÂ#Æ–æ—Câ"Â"Â"’À¢‚fFW&—fVBÂ'fÇVR"Â"Â’À¢‚g'VææW"Â&WfÇVFR"ÂÂ"’À¢‚g'VææW"Â&g&öÔFW&—fVB"ÂBÂ"’À¢‚g'VææW"Â&gFW$Gfæ6R"Â2Â2’À¢Ò°¢ÆWBÖWF†öBÒ6Æ70¢æÖWF†öG0¢æ—FW"‚¢æf–æB‡ÆVçG'—Â6Æ72æ6öç7FçE÷ööÂçWFc‚†VçG'’ææÖUö–æFW‚’ÓÒö²†ÖWF†öB’¢çVçw&‚“°¢ÆWB´GG&–'WFS£¤6öFR†6öFR•ÒÒÖWF†öBæGG&–'WFW2æ5÷6Æ–6R‚’VÇ6R°¢æ–2‚&öæR6öFRGG&–'WFR"“°¢Ó°¢76W'EöW‚†6öFRæÖ…÷7F6²Â6öFRæÖ…öÆö6Ç2’Â‡7F6²ÂÆö6Ç2’“°¢Ğ¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢fW&–g•÷&ÖWFW&—¦VE÷7WW%ö6ÆÇ5÷v—F…ö¦F²†f—'7Eö÷WGWBçF‚‚’“°¢Ğ§Ğ ¦fâfW&–g•÷&ÖWFW&—¦VE÷7WW%ö6ÆÇ5÷v—F…ö¦F²†÷WGWC¢eF‚’°¢ÆWB6Æ77F‚Ò÷WGWBçFõ÷7G"‚’çVçw&‚“°¢òò6W&FRF—676VÖ&Æ–W2¶VW–FVçF–6ÆÇ’æÖVB&VçBö6†–ÆBÖWF†öG2F—7F–æ7Bà¢f÷"†6Æ72ÂÖWF†öG2’–â°¢€¢$&6R"À¢fV2°¢€¢'V&Æ–2&6R†–çB“²"À¢fV2°¢&ÆöEó"À¢&–çfö¶W7V6–Â"À¢&ÆöEó"À¢&–ÆöEó"À¢'WFf–VÆB"À¢'&WGW&â"À¢ÒÀ¢’À¢€¢'V&Æ–2–çBF÷V&ÆVB‚“²"À¢fV2°¢&ÆöEó"À¢&–çfö¶Wf—'GVÂ"À¢&ÆöEó"À¢&–çfö¶Wf—'GVÂ"À¢&–FB"À¢&—&WGW&â"À¢ÒÀ¢’À¢ÒÀ¢’À¢€¢$FW&—fVB"À¢fV2°¢€¢'V&Æ–2FW&—fVB†–çB“²"À¢fV2²&ÆöEó"Â&–ÆöEó"Â&–çfö¶W7V6–Â"Â'&WGW&â%ÒÀ¢’À¢€¢'V&Æ–2–çBfÇVR‚“²"À¢fV2²&ÆöEó"Â&–çfö¶W7V6–Â"Â&–6öç7Eó"Â&–FB"Â&—&WGW&â%ÒÀ¢’À¢€¢'V&Æ–2–çB÷&–v–æÂ‚“²"À¢fV2²&ÆöEó"Â&–çfö¶W7V6–Â"Â&—&WGW&â%ÒÀ¢’À¢€¢'V&Æ–2–çBGfæ6R‚“²"À¢fV2²&ÆöEó"Â&–çfö¶W7V6–Â"Â&—&WGW&â%ÒÀ¢’À¢ÒÀ¢’À¢€¢%'VææW""À¢fV2²€¢'V&Æ–2–çBWfÇVFR„&6R“²"À¢fV2²&ÆöEó"Â&–çfö¶Wf—'GVÂ"Â&—&WGW&â%ÒÀ¢•ÒÀ¢’À¢Ò°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…²"×"Â"Ö2"Â"×b"Â"Ö6Æ77F‚"Â6Æ77F‚Â6Æ75Ò¢æ÷WGWB‚¢çVçw&‚“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢'·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢ÆWBFW‡BÒ7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB“°¢76W'B‡FW‡Bæ6öçF–ç2‚&Ö¦÷"fW'6–öã¢cR"’“°¢f÷"††VFW"Â–ç7G'V7F–öç2’–âÖWF†öG2°¢76W'Eö¦fö–ç7G'V7F–öå÷6WVVæ6R‚gFW‡BÂ†VFW"Âf–ç7G'V7F–öç2“°¢Ğ¢–b6Æ72ÓÒ$FW&—fVB"°¢f÷"&VfW&Væ6R–â°¢$&6RåÂ#Æ–æ—CåÂ#¢„’•b"À¢$&6RçfÇVS¢‚”’"À¢$&6Ræ–æ7&VÖVçC¢‚”’"À¢Ò°¢76W'B‡FW‡Bæ6öçF–ç2‡&VfW&Væ6R’Â&Ö—76–ær·&VfW&Væ6WÓ¢·FW‡GÒ"“°¢Ğ¢Ğ¢Ğ¢ÆWB†&æW72Ò÷WGWBæ¦ö–â‚%&ÖWFW&—¦VE7WW$6ÆÇ4†&æW72æ¦f"“°¢g3£§w&—FR‚f†&æW72Â"2'V&Æ–2f–æÂ6Æ72&ÖWFW&—¦VE7WW$6ÆÇ4†&æW72°¢V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’°¢'VææW"'VææW"ÒæWr'VææW"‚“°¢–b‡'VææW"æg&öÔ&6Rƒ’Ò#’F‡&÷ræWr76W'F–öäW'&÷"‚$&6R6öç7G'V7F÷"æBf—'GVÂ6ÆÂ"“°¢–b‡'VææW"æg&öÔFW&—fVBƒ’Ò#"’F‡&÷ræWr76W'F–öäW'&÷"‚$FW&—fVB÷fW'&–FRF—7F6‚"“°¢–b‡'VææW"æ÷&–v–æÂƒ’Ò’F‡&÷ræWr76W'F–öäW'&÷"‚'7V6–Â&6RçfÇVR"“°¢–b‡'VææW"æGfæ6Rƒ’Ò’F‡&÷ræWr76W'F–öäW'&÷"‚'7V6–Â&6Ræ–æ7&VÖVçB"“°¢–b‡'VææW"ægFW$Gfæ6Rƒ’Ò"’F‡&÷ræWr76W'F–öäW'&÷"‚'7FFRgFW"7V6–Â×WFF–öâ"“°¢–b†æWrFW&—fVBƒ’çfÇVR‚’ÒÇÂæWrFW&—fVBƒ’æF÷V&ÆVB‚’Ò#"’F‡&÷ræWr76W'F–öäW'&÷"‚&–æ†W&—FVBf—'GVÂF—7F6‚"“°¢–b‡'VææW"æg&öÔFW&—fVBƒ’Ò"ÇÂ'VææW"æg&öÔFW&—fVB‚Ó2’ÒÓBÇÂ'VææW"æg&öÔ&6R‚Ó2’ÒÓb’F‡&÷ræWr76W'F–öäW'&÷"‚'6–væVB6öç7G'V7F÷"&ÖWFW'2"“°¢7—7FVÒæ÷WBç&–çFÆâ‚'&ÖWFW&—¦VB7WW"6ÆÇ2fW&–f–VB"“°¢Ğ§Ğ¢"2’çVçw&‚“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ò×&VÆV6R"Â##"Â"Ö6Æ77F‚"Â6Æ77F‚Â"ÖB"Â6Æ77F…Ò¢æ&r††&æW72¢æ÷WGWB‚¢çVçw&‚“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢'·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢6Æ77F‚À¢%&ÖWFW&—¦VE7WW$6ÆÇ4†&æW72"À¢Ò¢æ÷WGWB‚¢çVçw&‚“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢'·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢76W'EöW€¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FF÷WB’çG&–Ò‚’À¢'&ÖWFW&—¦VB7WW"6ÆÇ2fW&–f–VB ¢“°§Ğ ¢5·FW7EĞ¦fâ7WW%ö6ÆÇ5÷&V¦V7E÷Vç7W÷'FVE÷6÷W&6Uö&Vf÷&U÷V&Æ—6†–æu÷F†Uö&F6‚‚’°¢f÷"†Æ&VÂÂ6÷W&6R’–â°¢€¢'&VçB×&—fFR"À¢&6Æ72&6R&—fFR‡6VVC¢–çB’·ÕÆæ6Æ72FW&—fVB‡7F'C¢–çB’W‡FVæG2&6R‡7F'B’·Ò"À¢’À¢€¢'&VçBÖ&—G’"À¢&6Æ72&6R‡6VVC¢–çB’·ÕÆæ6Æ72FW&—fVB‡7F'C¢–çB’W‡FVæG2&6R‚’·Ò"À¢’À¢€¢'&VçB×G—R"À¢&6Æ72&6R‡6VVC¢–çB’·ÕÆæ6Æ72FW&—fVB‡7F'C¢–çB’W‡FVæG2&6R‡G'VR’·Ò"À¢’À¢€¢'&VçB×F†—2"À¢&6Æ72&6R‡6VVC¢–çB’·ÕÆæ6Æ72FW&—fVB‡7F'C¢–çB’W‡FVæG2&6R‡F†—2ç&VB‚’’²FVb&VB‚“¢–çBÒÒ"À¢’À¢€¢'&VçBÖ'&æ6‚"À¢&6Æ72&6R‡6VVC¢–çB’·ÕÆæ6Æ72FW&—fVB‡7F'C¢–çB’W‡FVæG2&6R†–bG'VRF†Vâ7F'BVÇ6R’·Ò"À¢’À¢€¢'&VçBÖæWr"À¢&6Æ72FW·ÕÆæ6Æ72&6R†FW¢FW’·ÕÆæ6Æ72FW&—fVB‡7F'C¢–çB’W‡FVæG2&6R†æWrFW‚’’·Ò"À¢’À¢€¢'&VçBÖVffV7G2"À¢&6Æ72&6R‡6VVC¢–çB’·ÕÆæ6Æ72FW&—fVB‡7F'C¢–çB’W‡FVæG2&6R‡²f"ã¢–çBÒ7F'C²âÒâ²²âÒ’·Ò"À¢’À¢€¢'7WW"ÖÖ—†–â"À¢&6Æ72&6R‡6VVC¢–çB’²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVB‡7F'C¢–çB’W‡FVæG2&6R‡7F'B’²FVbfÇVS"‚“¢–çBÒ7WW%´&6UÒçfÇVR‚’Ò"À¢’À¢€¢'7WW"×VÆ–f–VB"À¢&6Æ72&6R‡6VVC¢–çB’²FVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVB‡7F'C¢–çB’W‡FVæG2&6R‡7F'B’²FVbfÇVS"‚“¢–çBÒFW&—fVBç7WW"çfÇVR‚’Ò"À¢’À¢€¢'7WW"×&—fFR"À¢&6Æ72&6R‡6VVC¢–çB’²&—fFRFVbfÇVR‚“¢–çBÒÕÆæ6Æ72FW&—fVB‡7F'C¢–çB’W‡FVæG2&6R‡7F'B’²FVbfÇVS"‚“¢–çBÒ7WW"çfÇVR‚’Ò"À¢’À¢€¢'7WW"ÖÖ—76–ær"À¢&6Æ72&6R‡6VVC¢–çB’·ÕÆæ6Æ72FW&—fVB‡7F'C¢–çB’W‡FVæG2&6R‡7F'B’²FVbfÇVR‚“¢–çBÒ7WW"æÖ—76–ær‚’Ò"À¢’À¢Ò°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚ff÷&ÖB‚'7WW"Ö6ÆÇ2×¶Æ&VÇÒ"’“°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö6Æ76W2‡6÷W&6RÂ%&V¦V7FVE7WW$6ÆÇ2ç66Æ"Â÷WGWBçF‚‚’¢æW‡V7EöW'"‚'Vç7W÷'FVB6÷W&6R×W7Bf–Â6Æ÷6VB"“°¢ÆWBW‡V7FVBÒÖF6‚Æ&VÂ°¢'&VçB×&—fFR"ÓâÖF6†W2€¢fW'&÷"À¢—VÆ–æTf–ÇW&S£¥G—W"…G—W$W'&÷#£¥&VçD6öç7G'V7F÷$–æ66W76–&ÆR²ââÒ¢’À¢'&VçBÖ&—G’"ÓâÖF6†W2€¢fW'&÷"À¢—VÆ–æTf–ÇW&S£¥G—W"…G—W$W'&÷#£¤Æ–6F–öä&—G”Ö—6ÖF6‚°¢W‡V7FVC¢À¢7GVÃ¢À¢âà¢Ò¢’À¢'&VçB×G—R"ÓâÖF6†W2€¢fW'&÷"À¢—VÆ–æTf–ÇW&S£¥G—W"…G—W$W'&÷#£¤Æ–6F–öä&wVÖVçEG—TÖ—6ÖF6‚°¢&wVÖVçEö–æFWƒ¢À¢âà¢Ò¢’À¢'&VçB×F†—2"ÓâÖF6†W2€¢fW'&÷"À¢—VÆ–æTf–ÇW&S£¥G—W"…G—W$W'&÷#£¥Væ–æ—F–Æ—¦VE&VçD&wVÖVçE&V6V—fW"²ââÒ¢’À¢'&VçBÖ'&æ6‚"Â'&VçBÖæWr"Â'&VçBÖVffV7G2"ÓâÖF6†W2€¢fW'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¤–çfÆ–E&–Ö'”6öç7G'V7F÷"°¢—77VS¢F÷GG•ö§fÓ£¤§fÔ6öç7G'V7F÷$—77VS£¥&VçD6öç7G'V7F÷%G—VE6†RÀ¢âà¢Ò¢’À¢'7WW"ÖÖ—†–â"Â'7WW"×VÆ–f–VB"Â'7WW"×&—fFR"ÓâÖF6†W2€¢fW'&÷"À¢—VÆ–æTf–ÇW&S£¥G—W"…G—W$W'&÷#£¥Vç7W÷'FVE7WW"²ââÒ¢’À¢'7WW"ÖÖ—76–ær"Óâ°¢ÖF6†W2€¢fW'&÷"À¢—VÆ–æTf–ÇW&S£¥G—W"…G—W$W'&÷#£¤ÖVÖ&W$æ÷Df÷VæB²ââÒ¢¢Ğ¢òÓâVç&V6†&ÆR‚’À¢Ó°¢76W'B†W‡V7FVBÂ'¶Æ&VÇÒf–ÆVBBâVæW‡V7FVB7FvS¢¶W'&÷#£÷Ò"“°¢76W'B€¢÷WGWBæ6Æ75öf–ÆW2‚’æ—5öV×G’‚’À¢'¶Æ&VÇÒV&Æ—6†VB'F–Â&F6‚ ¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ7WW%ö6ÆÇ5ö¶VWö6æöæ–6Å÷F&vWG5öæEöW†7Eög&ÖW5ö–åö÷&F–æ'•ö'&æ6†W2‚’°¢ÆWB6÷W&6RÒg3£§&VE÷Fõ÷7G&–ær†f—‡GW&U÷F‚‚%7WW$6ÆÇ2ç66Æ"’’çVçw&‚¢²%Ææ6Æ72'&æ6†–ær²FVb6†ö÷6R†fÆs¢&ööÆVâÂfÇVS¢FW&—fVB“¢–çBÒ–bfÆrF†VâfÇVRæ÷&–v–æÂ‚’VÇ6RfÇVRæGfæ6R‚’ÕÆâ#°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚'7WW"Ö6ÆÇ2Ö'&æ6†W2"“°¢ÆWB×WB6†V6¶VEö6ÆÇ2Ò°¢ÆWB×WBö'6W'fRÒÇVæ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÂFVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ÆWBFW&—fVBÒVæ—@¢æ6Æ76W0¢æ—FW"‚¢æf–æB‡ÇÆçÂ°¢Væ—Bç7F÷&P¢ææÖW0¢çG'•÷&W6öÇfR‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB‡Æâæ6Æ72’ææÖRçFW‡B‚’¢ÓÒ6öÖR‚$FW&—fVB"¢Ò¢çVçw&‚“°¢ÆWB&VçBÒVæ—@¢æ6Æ75ö†–W&&6‡’†FVf–æ—F–öç2¢çVçw&‚¢ç7WW&6Æ76W2‚¢æ—FW"‚¢æf–æB‡ÆVFvWÂVFvRæ6Æ72‚’ÓÒFW&—fVBæ6Æ72¢çVçw&‚¢ç&VçB‚“°¢òòföÆÆ÷rV6‚&VÂÖWF†öB&öG’6òF†R&W6öÇfW"6VW2—G27GVÂ6ÆÆW"à¢f÷"6ÆÆW"–âfFW&—fVBæÖWF†öG2°¢ÆWB×WBVæF–ærÒfV2¶6ÆÆW"æ&öG’çVçw&‚•Ó°¢v†–ÆRÆWB6öÖR†6ÆÂ’ÒVæF–ærç÷‚’°¢ÖF6‚gVæ—BçG—VEö7BævWB†6ÆÂ’æ¶–æB°¢G&VT¶–æC£¤Ç’†Ç’’Óâ°¢VæF–ærçW6‚†Ç’ægVæ7F–öâ“°¢VæF–æræW‡FVæB†Ç’æ&w2æ—FW"‚’æ6÷–VB‚’“°¢ÆWBG&VT¶–æC£¥6VÆV7B‡6VÆV7B’ÒgVæ—BçG—VEö7BævWB†Ç’ægVæ7F–öâ’æ¶–æ@¢VÇ6R°¢6öçF–çVS°¢Ó°¢–bÖF6†W2€¢Væ—BçG—VEö7BævWB‡6VÆV7BçVÆ–f–W"’æ¶–æBÀ¢G&VT¶–æC£¥7WW"…ò¢’°¢6öçF–çVS°¢Ğ¢ÆWB6öÖR…G—S£¥FW&Õ&Vb°¢F&vWC¢FW&Õ&VeF&vWC£¥7–Ö&öÂ‡6VÆV7FVB’À¢âà¢Ò’ÒVæ—@¢ç7F÷&P¢çG—W0¢çG'•övWB‡Væ—BçG—VEö7BævWB†Ç’ægVæ7F–öâ’çG’¢VÇ6R°¢æ–2‚&6æöæ–6Â6VÆV7FVBÖWF†öB"“°¢Ó°¢ÆWBF&vWBÒVæ—@¢ç&W6öÇfUöF—&V7E÷7WW%ö6ÆÂ€¢FW&—fVBæ6Æ72À¢6ÆÆW"æÖWF†öBÀ¢6ÆÂÀ¢FVf–æ—F–öç2À¢¢çVçw&‚“°¢76W'EöW‡F&vWBæÖWF†öE÷7–Ö&öÂ‚’Â§6VÆV7FVB“°¢76W'EöW€¢F&vWBæ–çfö6F–öåö¶–æB‚’À¢F÷GG•ö§fÓ£¤§fÔ–çfö6F–öä¶–æC£¥7V6–Å7WW ¢“°¢76W'EöW‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB‚§6VÆV7FVB’æ÷væW"Â6öÖR‡&VçB’“°¢6†V6¶VEö6ÆÇ2³Ò°¢Ğ¢G&VT¶–æC£¥6VÆV7B‡6VÆV7B’ÓâVæF–ærçW6‚‡6VÆV7BçVÆ–f–W"’À¢G&VT¶–æC£¥7WW"†æöFR’ÓâVæF–ærçW6‚†æöFRçVÂ’À¢G&VT¶–æC£¥F†—2…ò’ÂG&VT¶–æC£¤Æ—FW&Â…ò’ÂG&VT¶–æC£¤–FVçB…ò’Óâ·Ğ¢÷F†W"Óâæ–2‚'VæW‡V7FVBf—‡GW&RW‡&W76–öã¢¶÷F†W#£÷Ò"’À¢Ğ¢Ğ¢Ğ¢Ó°¢ÆWB'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö&F6…öö'6W'fVB€¢g6÷W&6RÀ¢%7WW$6ÆÇ4'&æ6†W2ç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒ3sR’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢6öÖR‚f×WBö'6W'fR’À¢¢çVçw&‚“°¢76W'EöW†6†V6¶VEö6ÆÇ2Â2“°¢ÆWB'&æ6†–ærÒFV6öFUöVÖ—GFVEö6Æ72€¢'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ$'&æ6†–ær"¢çVçw&‚’À¢“°¢ÆWBg&ÖW2Ò7F6µöÖög&ÖW2‚f'&æ6†–ærÂ&6†ö÷6R"“°¢ÆWBF&vWG2ÒFV6öFUö'&æ6…÷F&vWG2†VÖ—GFVEöÖWF†öEö6öFR‚f'&æ6†–ærÂ&6†ö÷6R"’“°¢76W'B‚F&vWG2æ—5öV×G’‚’“°¢f÷"…òÂòÂF&vWB’–âF&vWG2°¢76W'B†g&ÖW2æ—FW"‚’æç’‡Æg&ÖWÂg&ÖRç2ÓÒF&vWB’“°¢Ğ¢76W'B†g&ÖW2æ—FW"‚’æÆÂ‡Æg&ÖWÂ°¢g&ÖP¢æÆö6Ç0¢æ6öçF–ç2‚e7F6´ÖfW&–f–6F–öåG—S£¤ö&¦V7B‚$FW&—fVB"çFõö÷væVB‚’’¢Ò’“°¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚%7WW$6ÆÇ4'&æ6†W4†&æW72æ¦f"“°¢g3£§w&—FR‚f†&æW72Â'V&Æ–26Æ727WW$6ÆÇ4'&æ6†W4†&æW72²V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’²'&æ6†–ær"ÒæWr'&æ6†–ær‚“²FW&—fVBBÒæWrFW&—fVBƒ“²–b†"æ6†ö÷6R‡G'VRÂB’ÒÇÂ"æ6†ö÷6R†fÇ6RÂB’ÒÇÂ"æ6†ö÷6R‡G'VRÂB’Ò’F‡&÷ræWr76W'F–öäW'&÷"‚“²ÒÒ"’çVçw&‚“°¢ÆWB6Æ77F‚Ò÷WGWBçF‚‚’çFõ÷7G"‚’çVçw&‚“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&w2…²"Ò×&VÆV6R"Â##"Â"Ö6Æ77F‚"Â6Æ77F‚Â"ÖB"Â6Æ77F…Ò¢æ&r††&æW72¢æ÷WGWB‚¢çVçw&‚“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢'·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&w2…°¢"Õ‡fW&–g“¦ÆÂ"À¢"Ö6Æ77F‚"À¢6Æ77F‚À¢%7WW$6ÆÇ4'&æ6†W4†&æW72"À¢Ò¢æ÷WGWB‚¢çVçw&‚“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢'·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°¢Ğ§Ğ ¢5·FW7EĞ¦fâ7&÷75ö6Æ75÷&VFW%ö6ÆÅö6ö×–ÆW5öæE÷'Vç5÷v—F…ö÷ÆææVEö6÷VçFW"‚’°¢–b&WV—&Uö¦Fµö÷%÷6¶—‚’°¢&WGW&ã°¢Ğ¢ÆWB&WfW'6VBÒ5$õ55ô4Ä55õD5E•õ4õU$4P¢ç7Æ—B‚&6Æ72&VFW#¢"¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW‡&WfW'6VBæÆVâ‚’Â"“°¢ÆWB6÷VçFW%÷7F'BÒ&WfW'6VE³Ğ¢æf–æB‚&6Æ726÷VçFW"‚"¢æW‡V7B‚$6÷VçFW"FV6Æ&F–öâ7F'G2–âf—‡GW&R"“°¢ÆWB6¶vRÒg&WfW'6VE³Õ²âæ6÷VçFW%÷7F'EÓ°¢ÆWB6÷VçFW"Òg&WfW'6VE³Õ¶6÷VçFW%÷7F'BâåÓ°¢ÆWB&VFW%öf—'7BÒf÷&ÖB‚'·6¶vWÖ6Æ72&VFW#§·ÕÆåÆç¶6÷VçFW'Ò"Â&WfW'6VE³Ò“°¢6ö×–ÆUöæE÷'Våö7&÷75ö6Æ75÷&VFW"„5$õ55ô4Ä55õD5E•õ4õU$4RÂ'6÷W&6RÖ÷&FW""Âcs"“°¢6ö×–ÆUöæE÷'Våö7&÷75ö6Æ75÷&VFW"‚g&VFW%öf—'7BÂ'&WfW'6VBÖ÷&FW""Âcs2“°¢ÆWB6ÖUöæÖVEöFV6÷’Ğ¢f÷&ÖB‚'´5$õ55ô4Ä55õD5E•õ4õU$4WÕÆæ6Æ72FV6÷“¥ÆâFVbvWB‚“¢–çBÒÆâ"“°¢6ö×–ÆUöæE÷'Våö7&÷75ö6Æ75÷&VFW"‚g6ÖUöæÖVEöFV6÷’Â'6ÖRÖæÖVBÖFV6÷’"Âcsb“°§Ğ ¦fâ6ö×–ÆUöæE÷'Våö7&÷75ö6Æ75÷&VFW"‡6÷W&6S¢g7G"ÂÆ&VÃ¢g7G"Â6÷W&6Uö–C¢S3"’°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr†Æ&VÂ“°¢ÆWB'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö&F6‚€¢6÷W&6RÀ¢%&VFW"ç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚‡6÷W&6Uö–B’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢¢çVçw&ö÷%öVÇ6R‡Æf–ÇW&WÂæ–2‚&7&÷72Ö6Æ726÷W&6R&F6‚6ö×–ÆW2‡¶Æ&VÇÒ“¢¶f–ÇW&S£÷Ò"’“°¢76W'B†'F–f7G2æÆVâ‚’ãÒ"“°¢76W'B†÷WGWBçF‚‚’æ¦ö–â‚&W†×ÆRô6÷VçFW"æ6Æ72"’æ—5öf–ÆR‚’“°¢76W'B†÷WGWBçF‚‚’æ¦ö–â‚&W†×ÆRõ&VFW"æ6Æ72"’æ—5öf–ÆR‚’“° ¢ÆWB†&æW72Ò÷WGWBçF‚‚’æ¦ö–â‚$7&÷746Æ74†&æW72æ¦f"“°¢g3£§w&—FR€¢f†&æW72À¢"2'V&Æ–2f–æÂ6Æ727&÷746Æ74†&æW72°¢V&Æ–27FF–2fö–BÖ–â…7G&–æuµÒ&w2’°¢W†×ÆRä6÷VçFW"6÷VçFW"ÒæWrW†×ÆRä6÷VçFW"ƒC“°¢W†×ÆRå&VFW"&VFW"ÒæWrW†×ÆRå&VFW"‚“°¢–b‡&VFW"ç&VB†6÷VçFW"’ÒC’°¢F‡&÷ræWr76W'F–öäW'&÷"‚%&VFW"ç&VB×W7B&WGW&â6÷VçFW"ævWB"“°¢Ğ¢Ğ§Ğ¢"2À¢¢æW‡V7B‚'w&—FR¦f'VçF–ÖR†&æW72"“°¢ÆWB¦f2Ò6öÖÖæC£¦æWr‚&¦f2"¢æ&r‚"Ö6Æ77F‚"¢æ&r†÷WGWBçF‚‚’¢æ&r‚"ÖB"¢æ&r†÷WGWBçF‚‚’¢æ&r‚f†&æW72¢æ÷WGWB‚¢æW‡V7B‚''Vâ¤D²¦f2"“°¢76W'B€¢¦f2ç7FGW2ç7V66W72‚’À¢&¦f2&V¦V7FVBvVæW&FVB6÷VçFW"õ&VFW"6Æ76W2‡¶Æ&VÇÒ“¢·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦f2ç7FFW'"¢“°¢ÆWB¦fÒ6öÖÖæC£¦æWr‚&¦f"¢æ&r‚"Õ‡fW&–g“¦ÆÂ"¢æ&r‚"Ö6Æ77F‚"¢æ&r†÷WGWBçF‚‚’¢æ&r‚$7&÷746Æ74†&æW72"¢æ÷WGWB‚¢æW‡V7B‚''Vâ¤D²¦ffW&–f–W"æB†&æW72"“°¢76W'B€¢¦fç7FGW2ç7V66W72‚’À¢$¤D²&V¦V7FVB÷"f–ÆVBvVæW&FVB7&÷72Ö6Æ726ÆÂ‡¶Æ&VÇÒ“¢·Ò"À¢7G&–æs£¦g&öÕ÷WFc…öÆ÷77’‚f¦fç7FFW'"¢“°§Ğ ¢5·FW7EĞ¦fâ7&÷75ö6Æ75ö6ÆÅ÷&W÷'G5÷&–Ö—F—fU÷&V6V—fW%÷Fõ÷F†U÷G—W"‚’°¢ÆWB6÷W&6RÒ5$õ55ô4Ä55õD5E•õ4õU$4Rç&WÆ6R‚&6÷VçFW#¢6÷VçFW""Â&6÷VçFW#¢–çB"“°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&7&÷72Ö6Æ72×&–Ö—F—fR×&V6V—fW""“°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö&F6‚€¢g6÷W&6RÀ¢%&VFW"ç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒcsB’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢¢æW‡V7EöW'"‚&â–çB&V6V—fW"FöW2æ÷B†fR6÷VçFW"ævWB"“°¢76W'B€¢ÖF6†W2€¢W'&÷"À¢—VÆ–æTf–ÇW&S£¥G—W"…G—W$W'&÷#£¤ÖVÖ&W$æ÷Df÷VæB²ââÒ¢’À¢'¶W'&÷#£÷Ò ¢“°§Ğ ¢5·FW7EĞ¦fâ7&÷75ö6Æ75÷&—fFUöÖWF†öEö6ÆÅ÷&W÷'G5ö—G5öf—'7Eö&6¶VæEöf–ÇW&R‚’°¢ÆWB6÷W&6RÒ5$õ55ô4Ä55õD5E•õ4õU$4Rç&WÆ6R‚&FVbvWB‚’"Â'&—fFRFVbvWB‚’"“°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&7&÷72Ö6Æ72×&—fFRÖÖWF†öB"“°¢ÆWBW'&÷"Ò6ö×–ÆU÷6÷W&6Uö&F6‚€¢g6÷W&6RÀ¢%&VFW"ç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒcsR’À¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢¢æW‡V7EöW'"‚&7&÷72Ö6Æ726ÆÂ6ææ÷B–çfö¶R6÷VçFW"w2&—fFRvWB"“°¢76W'B€¢ÖF6†W2€¢W'&÷"À¢—VÆ–æTf–ÇW&S£¤6öFVvVâ„§fÔ6öFVvVäW'&÷#£¥Vç7W÷'FVD§fÕf—6–&–Æ—G’°¢f—6–&–Æ—G“¢f—6–&–Æ—G“£¥&—fFRÀ¢âà¢Ò¢’À¢'¶W'&÷#£÷Ò ¢“°§Ğ ¢5·FW7EĞ¦fâGVÅ÷&VfW&Væ6Uöf7F÷'•÷w&÷&V6÷&G5÷F†UövVçV–æU÷6÷W&6U÷6W76–öåöæEöf—'7E÷&ööeöW'&÷"‚’°¢W6RF÷GG“£¦§fÕ÷F7G•ö7&÷75ö6Æ73£§fÆ–FFUö7&÷75ö6Æ75ö6öç7G'V7F÷%ö6ÆÅ÷&öög3° ¢6öç7B4õU$4S¢g7G"Ğ¢–æ6ÇVFU÷7G"‚&f—‡GW&W2ö§fÕ÷F7G•öGVÅ÷&VfW&Væ6Uöf÷'v&F–ærõ—$f7F÷'’ç66Æ"“°¢ÆWB6÷W&6Uö–BÒ6÷W&6T–C£¦g&öÕö–æFW‚ƒ“c“°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚&GVÂ×&VfW&Væ6RÖf÷'v&F–ær"“°¢ÆWB×WB&ööeöW'&÷"ÒæöæS°¢ÆWB×WB–6¶ÆW%öW'&÷"ÒæöæS°¢ÆWB×WB7FÆUöW'&÷"ÒæöæS°¢ÆWB×WB7vVEöW'&÷"ÒæöæS°¢ÆWB×WB7FÆU÷G—VE÷&ö÷EöW'&÷"ÒæöæS°¢ÆWB×WB7FÆU÷6÷W&6U÷&ö÷EöW'&÷"ÒæöæS°¢ÆWB×WBf÷&V–vå÷6÷W&6UöW'&÷"ÒæöæS°¢ÆWB×WBö'6W'fVEö§fÕö'—FW2ÒæöæS°¢ÆWB×WBö'6W'fVBÒfÇ6S°¢ÆWB×WBö'6W'fU÷G—W"ÒÆ7C¢d7D&VæÅVçG—VCâÀ¢6÷W&6U÷&ö÷C¢G&VT–CÅVçG—VCâÀ¢–æFWƒ¢e6÷W&6U6VÖçF–4–æFW‚À¢G—W#¢f×WB6÷W&6UG—W#ÂuóâÀ¢FVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢7G'V7B&–æF–æw3Âvâ°¢G—VC¢bv6÷W&6UG—VD–æFW‚À¢6Æ76W3¢fV3Å7–Ö&öÄ–CâÀ¢ÖWF†öG3¢fV3Å7–Ö&öÄ–CâÀ¢Ğ¢–×ÂF÷GG•÷F7G•÷–6¶ÆW#£¥6÷W&6UG—VD&–æF–æw2f÷"&–æF–æw3Âuóâ°¢fâG—VEöB‚g6VÆbÂ6÷W&6S¢6÷W&6T–BÂG&VS¢G&VT–CÅVçG—VCâ’Óâ÷F–öãÅG&VT–CÅG—VCãâ°¢6VÆbçG—VBævWB‡6÷W&6RÂG&VR¢Ğ¢fâÆææVEö6Æ75÷7–Ö&öÇ2‚g6VÆb’Óâeµ7–Ö&öÄ–EÒ°¢g6VÆbæ6Æ76W0¢Ğ¢fâÆææVEöÖWF†öE÷7–Ö&öÇ2‚g6VÆb’Óâeµ7–Ö&öÄ–EÒ°¢g6VÆbæÖWF†öG0¢Ğ¢Ğ¢ÆWBG&VT¶–æC£¥6¶vTFVb‡6¶vR’Òf7BævWB‡6÷W&6U÷&ö÷B’æ¶–æBVÇ6R°¢æ–2‚'Væ—B&ö÷B—26¶vR"¢Ó°¢ÆWB×WB6Æ76W2ÒfV3£¦æWr‚“°¢ÆWB×WBÖWF†öG2ÒfV3£¦æWr‚“°¢ÆWB×WBf7F÷'•öFFÒæöæS°¢f÷"gG&VR–âg6¶vRç7FG2°¢ÆWBG&VT¶–æC£¥G—TFVb†FVf–æ—F–öâ’Òf7BævWB‡G&VR’æ¶–æBVÇ6R°¢6öçF–çVS°¢Ó°¢ÆWB6Æ72Ò–æFW‚ç7–Ö&öÅöB‡6÷W&6Uö–BÂG&VR’çVçw&‚“°¢6Æ76W2çW6‚†6Æ72“°¢ÆWBG&VT¶–æC£¥FV×ÆFR‡FV×ÆFR’Òf7BævWB†FVf–æ—F–öâç&‡2’æ¶–æBVÇ6R°¢6öçF–çVS°¢Ó°¢f÷"fÖVÖ&W%÷G&VR–âgFV×ÆFRæ&öG’°¢–bÆWB6öÖR‡7–Ö&öÂ’Ò–æFW‚ç7–Ö&öÅöB‡6÷W&6Uö–BÂÖVÖ&W%÷G&VR’°¢ÖWF†öG2çW6‚‡7–Ö&öÂ“°¢Ğ¢–bÆWBG&VT¶–æC£¤FVdFVb†ÖWF†öEöFVb’Òf7BævWB†ÖVÖ&W%÷G&VR’æ¶–æ@¢bbG—W ¢ç7F÷&R‚¢ææÖW0¢ç&W6öÇfR†ÖWF†öEöFVbææÖRæ5öæÖR‚’çFW‡B‚’¢ÓÒ'w& ¢°¢f7F÷'•öFFÒ6öÖR‚†6Æ72ÂG&VRÂFV×ÆFRæ6öç7G'V7F÷"ÂÖVÖ&W%÷G&VR’“°¢Ğ¢Ğ¢Ğ¢ÆWB†f7F÷'’Â6Æ75÷G&VRÂ6öç7G'V7F÷%÷G&VRÂÖWF†öE÷G&VR’Òf7F÷'•öFFçVçw&‚“°¢ÆWB6öç7G'V7F÷"Ò–æFW‚ç7–Ö&öÅöB‡6÷W&6Uö–BÂ6öç7G'V7F÷%÷G&VR’çVçw&‚“°¢G—W"æ6ö×ÆWFU÷7–Ö&öÂ†6öç7G'V7F÷"’çVçw&‚“°¢ÆWBG&VT¶–æC£¤FVdFVb†6öç7G'V7F÷%öFVb’Òf7BævWB†6öç7G'V7F÷%÷G&VR’æ¶–æBVÇ6R°¢æ–2‚&FVfVÇB6öç7G'V7F÷"—26÷W&6RFVf–æ—F–öâ"¢Ó°¢ÆWB6öç7G'V7F÷%÷&×2Ò6öç7G'V7F÷%öFV`¢çfÇVU÷&Õö6ÆW6W0¢æ—FW"‚¢æÖ‡Æ6ÆW6WÂ°¢6ÆW6P¢æ—FW"‚¢æÖ‡ÇG&VWÂ–æFW‚ç7–Ö&öÅöB‡6÷W&6Uö–BÂ§G&VR’çVçw&‚’¢æ6öÆÆV7B‚¢Ò¢æ6öÆÆV7B‚“°¢ÆWBÖWF†öBÒ–æFW‚ç7–Ö&öÅöB‡6÷W&6Uö–BÂÖWF†öE÷G&VR’çVçw&‚“°¢ÆWBÖVÖ&W'2Ò¶F÷GG•÷F7G•÷–6¶ÆW#£¤6Æ74ÖVÖ&W%–6¶ÆUÆã£¤ÖWF†öB°¢7–Ö&öÃ¢ÖWF†öBÀ¢6÷W&6U÷G&VS¢ÖWF†öE÷G&VRÀ¢ÕÓ°¢ÆWB&–æF–æw2Ò&–æF–æw2°¢G—VC¢G—W"ç6÷W&6U÷G—VEö–æFW‚‚’À¢6Æ76W2À¢ÖWF†öG2À¢Ó°¢ÆWB7FFRÒF÷GG•÷F7G•÷–6¶ÆW#£¤6Æ757FFU–6¶ÆUÆâ°¢7F÷&S¢G—W"ç7F÷&R‚’À¢FVf–æ—F–öç2À¢6÷W&6Uö7C¢7BÀ¢G—VEö7C¢G—W"çG—VEö7B‚’À¢6÷W&6Uö–æFWƒ¢–æFW‚À¢G—VEö&–æF–æw3¢f&–æF–æw2À¢6÷W&6S¢F÷GG•÷F7G•÷–6¶ÆW#£¥6÷W&6TÖWFFF°¢–C¢6÷W&6Uö–BÀ¢æÖS¢%—$f7F÷'’ç66Æ"æ–çFò‚’À¢ÒÀ¢6Æ73¢f7F÷'’À¢6Æ75÷G&VRÀ¢6öç7G'V7F÷#¢F÷GG•÷F7G•÷–6¶ÆW#£¥&–Ö'”6öç7G'V7F÷$–çWEÆâ°¢7–Ö&öÃ¢6öç7G'V7F÷"À¢6÷W&6U÷G&VS¢6öç7G'V7F÷%÷G&VRÀ¢&ÖWFW%ö6ÆW6W3¢6öç7G'V7F÷%÷&×2À¢ÒÀ¢ÖVÖ&W'3¢fÖVÖ&W'2À¢Ó°¢–6¶ÆW%öW'&÷"Ò6öÖR†ÖF6‚7FFRçfÆ–FFR‚’°¢ö²‡fÆ–FFVB’ÓâÖF6‚fÆ–FFVBæF—7F–æ7E÷&VfW&Væ6UöÖWF†öE÷6–væGW&R†ÖWF†öB’°¢ö²…ò’Óâ$ö²‡6–væGW&R66WFVB’"çFõö÷væVB‚’À¢W'"†W'&÷"’Óâf÷&ÖB‚$W'"‡¶W'&÷#£÷Ò’"’À¢ÒÀ¢W'"†W'&÷"’Óâf÷&ÖB‚$W'"†6Æ72×7FFR¶W'&÷#£÷Ò’"’À¢Ò“°¢Ó°¢ÆWB×WBö'6W'fU÷6÷W&6RÒÆ7C¢d7D&VæÅVçG—VCâÀ¢6÷W&6U÷&ö÷C¢G&VT–CÅVçG—VCâÀ¢–æFWƒ¢e6÷W&6U6VÖçF–4–æFW‚À¢6÷W&6U÷G—VEö–æFWƒ¢e6÷W&6UG—VD–æFW‚À¢Væ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÀ¢FVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ö'6W'fVBÒG'VS°¢ÆWB6÷VçFW"ÒVF—Eö6Æ75öæÖVB‡Væ—BÂ$6÷VçFW""“°¢ÆWB—"ÒVF—Eö6Æ75öæÖVB‡Væ—BÂ%—""“°¢ÆWBf7F÷'’ÒVF—Eö6Æ75öæÖVB‡Væ—BÂ%—$f7F÷'’"“°¢76W'EöW€¢Væ—Bæ6Æ76W0¢æ—FW"‚¢æÖ‡ÇÆçÂVæ—@¢ç7F÷&P¢ææÖW0¢ç&W6öÇfR‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB‡Æâæ6Æ72’ææÖRçFW‡B‚’’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢²$6÷VçFW""Â%—""Â%—$f7F÷'’%Ğ¢“°¢ÆWB6÷W&6UöÖWF†öBÒ7@¢æ—FW"‚¢æf–æEöÖ‡Â‡G&VRÂæöFR—ÂÖF6‚fæöFRæ¶–æB°¢G&VT¶–æC£¤FVdFVb†FVf–æ—F–öâ¢–bVæ—Bç7F÷&RææÖW2ç&W6öÇfR†FVf–æ—F–öâææÖRæ5öæÖR‚’çFW‡B‚’’ÓÒ'w&"Óà¢°¢6öÖR‚‡G&VRÂFVf–æ—F–öâ’¢Ğ¢òÓâæöæRÀ¢Ò¢æW‡V7B‚&÷&–v–æÂ6÷W&6RFV6Æ&W2—$f7F÷'’çw&"“°¢ÆWBÖWF†öBÒ–æFW‚ç7–Ö&öÅöB‡6÷W&6Uö–BÂ6÷W&6UöÖWF†öBã’çVçw&‚“°¢76W'EöW‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB†ÖWF†öB’æ÷væW"Â6öÖR†f7F÷'’’“°¢ÆWB¶f—'7E÷G&VRÂ6V6öæE÷G&VUÒÒ6÷W&6UöÖWF†öBãçfÇVU÷&Õö6ÆW6W5³Òæ5÷6Æ–6R‚’VÇ6R°¢æ–2‚'w&†2W†7FÇ’Gvò÷&FW&VBf÷&ÖÇ2"“°¢Ó°¢ÆWBf—'7BÒ–æFW‚ç7–Ö&öÅöB‡6÷W&6Uö–BÂ¦f—'7E÷G&VR’çVçw&‚“°¢ÆWB6V6öæBÒ–æFW‚ç7–Ö&öÅöB‡6÷W&6Uö–BÂ§6V6öæE÷G&VR’çVçw&‚“°¢76W'EöæR†f—'7BÂ6V6öæB“°¢f÷"‡G&VRÂ7–Ö&öÂÂæÖR’–â°¢‚¦f—'7E÷G&VRÂf—'7BÂ&f—'7B"’À¢‚§6V6öæE÷G&VRÂ6V6öæBÂ'6V6öæB"’À¢Ò°¢76W'EöW‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB‡7–Ö&öÂ’æ÷væW"Â6öÖR†ÖWF†öB’“°¢76W'EöW€¢Væ—Bç7F÷&P¢ææÖW0¢ç&W6öÇfR‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB‡7–Ö&öÂ’ææÖRçFW‡B‚’’À¢æÖP¢“°¢76W'B€¢ÖF6†W2†–æFW‚æFVf–æ—F–öåööb‡7–Ö&öÂ’Â6öÖR…6÷W&6TFVf–æ—F–öã£¤6æöæ–6Â²6÷W&6RÂG&VS¢7GVÂÒ’–b6÷W&6RÓÒ6÷W&6Uö–Bbb7GVÂÓÒG&VR¢“°¢76W'EöW‡6÷W&6U÷G—VEö–æFW‚ævWB‡6÷W&6Uö–BÂG&VR’ÂæöæR“°¢76W'B€¢ÖF6†W2‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB‡7–Ö&öÂ’æ–æfòÂ7–Ö&öÄ–æfó£¤6ö×ÆWFR‡G’’–bÖF6†W2‡Væ—Bç7F÷&RçG—W2ævWB‡G’’ÂG—S£¥G—U&Vb²F&vWC¢G—U&VeF&vWC£¥7–Ö&öÂ‡F&vWB’ÂââÒ–b§F&vWBÓÒ6÷VçFW"’¢“°¢Ğ¢ÆWB7–Ö&öÄ–æfó£¤6ö×ÆWFR†ÖWF†öE÷G—R’ÒVæ—Bç7F÷&Rç7–Ö&öÇ2ævWB†ÖWF†öB’æ–æfòVÇ6R°¢æ–2‚'w&6–væGW&R—26ö×ÆWFVB–âF†R6÷W&6R6W76–öâ"“°¢Ó°¢ÆWBG—S£¤ÖWF†öB‡6–væGW&R’ÒVæ—Bç7F÷&RçG—W2ævWB†ÖWF†öE÷G—R’VÇ6R°¢æ–2‚'w&†2â÷&F–æ'’ÖWF†öB6–væGW&R"“°¢Ó°¢76W'EöW‡6–væGW&Rç&×2æÆVâ‚’Â"“°¢76W'B‡6–væGW&Rç&×2æ—FW"‚’æÆÂ‡Ç&ÖWFW'ÂÖF6†W2‡Væ—Bç7F÷&RçG—W2ævWB‡&ÖWFW"çG’’ÂG—S£¥G—U&Vb²F&vWC¢G—U&VeF&vWC£¥7–Ö&öÂ‡F&vWB’ÂââÒ–b§F&vWBÓÒ6÷VçFW"’’“°¢76W'EöW†VF—E÷G—U÷7–Ö&öÂ‡Væ—BÂ6–væGW&Rç&W7VÇB’Â6öÖR‡—"’“°¢ÆWB—%÷6÷W&6RÒ–æFW‚æFVf–æ—F–öåööb‡—"’çVçw&‚“°¢ÆWB6÷W&6TFVf–æ—F–öã£¤6æöæ–6Â°¢G&VS¢—%÷G&VRÂâà¢ÒÒ—%÷6÷W&6P¢VÇ6R°¢æ–2‚%—"—26æöæ–6Â6÷W&6R"¢Ó°¢ÆWBG&VT¶–æC£¥G—TFVb‡—%öFVb’Òf7BævWB‡—%÷G&VR’æ¶–æBVÇ6R°¢æ–2‚%—"—26Æ72FVf–æ—F–öâ"¢Ó°¢ÆWBG&VT¶–æC£¥FV×ÆFR‡—%÷FV×ÆFR’Òf7BævWB‡—%öFVbç&‡2’æ¶–æBVÇ6R°¢æ–2‚%—"†2FV×ÆFR"¢Ó°¢ÆWB—%ö6öç7G'V7F÷"Ò–æFW€¢ç7–Ö&öÅöB‡6÷W&6Uö–BÂ—%÷FV×ÆFRæ6öç7G'V7F÷"¢çVçw&‚“°¢ÆWBG&VT¶–æC£¤FVdFVb‡—%ö6öç7G'V7F÷%öFVb’Òf7BævWB‡—%÷FV×ÆFRæ6öç7G'V7F÷"’æ¶–æ@¢VÇ6R°¢æ–2‚%—"†2&–Ö'’6öç7G'V7F÷""¢Ó°¢ÆWB—%÷FV×ÆFUöf÷&ÖÇ2Ò—%ö6öç7G'V7F÷%öFVbçfÇVU÷&Õö6ÆW6W5³Ğ¢æ—FW"‚¢æÖ‡ÇG&VWÂ–æFW‚ç7–Ö&öÅöB‡6÷W&6Uö–BÂ§G&VR’çVçw&‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢ÆWB—%ö6öç7G'V7F÷%öf÷&ÖÇ2Ò—%ö6öç7G'V7F÷%öFVbçfÇVU÷&Õö6ÆW6W5³Ğ¢æ—FW"‚¢æÖ‡ÇG&VWÂ°¢–æFW€¢æFW&—fVE÷7–Ö&öÅöB‡—%ö6öç7G'V7F÷"Â6÷W&6Uö–BÂ§G&VR¢çVçw&‚¢Ò¢æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW‡—%ö6öç7G'V7F÷%öf÷&ÖÇ2æÆVâ‚’Â"“°¢76W'B€¢—%ö6öç7G'V7F÷%öf÷&ÖÇ0¢æ—FW"‚¢æÆÂ‡Ç7–Ö&öÇÂVæ—Bç7F÷&Rç7–Ö&öÇ2ævWB‚§7–Ö&öÂ’æ÷væW"ÓÒ6öÖR‡—%ö6öç7G'V7F÷"’¢“° ¢ÆWB6÷W&6U÷&‡2Ò6÷W&6UöÖWF†öBãç&‡2çVçw&‚“°¢ÆWBG&VT¶–æC£¤Ç’‡6÷W&6Uö÷WFW"’Òf7BævWB‡6÷W&6U÷&‡2’æ¶–æBVÇ6R°¢æ–2‚&&öG’—2F†RF—&V7B÷WFW"—"6öç7G'V7F÷"Æ–6F–öâ"“°¢Ó°¢76W'EöW‡6÷W&6Uö÷WFW"æ&w2æÆVâ‚’Â"“°¢ÆWBG—VE÷&‡2Ò6÷W&6U÷G—VEö–æFW‚ævWB‡6÷W&6Uö–BÂ6÷W&6U÷&‡2’çVçw&‚“°¢ÆWBG&VT¶–æC£¤Ç’‡G—VEö÷WFW"’ÒgVæ—BçG—VEö7BævWB‡G—VE÷&‡2’æ¶–æBVÇ6R°¢æ–2‚'G—VB&öG’&W6W'fW2F†R÷WFW"Ç’"“°¢Ó°¢76W'EöW‡G—VEö÷WFW"æ&w2æÆVâ‚’Â"“°¢ÆWB6÷W&6U÷6VÆV7BÒ6÷W&6Uö÷WFW"ægVæ7F–öã°¢ÆWBG&VT¶–æC£¥6VÆV7B‡6VÆV7F–öâ’Òf7BævWB‡6÷W&6U÷6VÆV7B’æ¶–æBVÇ6R°¢æ–2‚%—"6öç7G'V7F÷"Æ–6F–öâ†2÷&–v–æÂ6VÆV7B"“°¢Ó°¢ÆWB6÷W&6UöæWrÒ6VÆV7F–öâçVÆ–f–W#°¢76W'B†ÖF6†W2†7BævWB‡6÷W&6UöæWr’æ¶–æBÂG&VT¶–æC£¤æWr…ò’’“°¢ÆWBG—VE÷6VÆV7BÒ6÷W&6U÷G—VEö–æFW‚ævWB‡6÷W&6Uö–BÂ6÷W&6U÷6VÆV7B’çVçw&‚“°¢ÆWB6öç7G'V7F÷"ÒFW&Õ÷G—U÷F&vWB‡Væ—BÂVæ—BçG—VEö7BævWB‡G—VE÷6VÆV7B’çG’’çVçw&‚“°¢76W'EöW‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB†6öç7G'V7F÷"’æ÷væW"Â6öÖR‡—"’“°¢ÆWBG—VEöæWrÒ6÷W&6U÷G—VEö–æFW‚ævWB‡6÷W&6Uö–BÂ6÷W&6UöæWr’çVçw&‚“°¢76W'B†ÖF6†W2€¢Væ—BçG—VEö7BævWB‡G—VEöæWr’æ¶–æBÀ¢G&VT¶–æC£¤æWr…ò¢’“°¢ÆWBVÖ—GFVBÒF÷GG•ö§fÓ£¦VÖ—Eö6Æ72‡Væ—BÂf7F÷'’ÂFVf–æ—F–öç2’çVçw&‚“°¢ÆWBVÖ—GFVEö'—FW2ÒVÖ—GFVBæ'—FW2‚’çFõ÷fV2‚“°¢ÆWB×WB6Æ75÷&VFW"Ò&VFW#£¦æWr‚fVÖ—GFVEö'—FW2“°¢ÆWBVÖ—GFVEö6Æ72Ò6Æ74f–ÆS£¦FV6öFR‚f×WB6Æ75÷&VFW"’çVçw&‚“°¢76W'B†6Æ75÷&VFW"æ—5öEöVæB‚’“°¢ÆWBw&öÖWF†öBÒVÖ—GFVEö6Æ70¢æÖWF†öG0¢æ—FW"‚¢æf–æB‡Æ6æF–FFWÂVÖ—GFVEö6Æ72æ6öç7FçE÷ööÂçWFc‚†6æF–FFRææÖUö–æFW‚’ÓÒö²‚'w&"’¢çVçw&‚“°¢76W'EöW€¢VÖ—GFVEö6Æ70¢æ6öç7FçE÷ööÀ¢çWFc‚‡w&öÖWF†öBæFW67&—F÷%ö–æFW‚’À¢ö²‚"„ÆW†×ÆRô6÷VçFW#´ÆW†×ÆRô6÷VçFW#²”ÆW†×ÆRõ—#²"¢“°¢ÆWB´GG&–'WFS£¤6öFR‡w&ö6öFR•ÒÒw&öÖWF†öBæGG&–'WFW2æ5÷6Æ–6R‚’VÇ6R°¢æ–2‚'w&†2öæR6öFRGG&–'WFR"¢Ó°¢76W'EöW‚‡w&ö6öFRæÖ…÷7F6²Âw&ö6öFRæÖ…öÆö6Ç2’ÂƒBÂ2’“°¢76W'EöW€¢FV6öFU÷7V'6WEö–ç7G'V7F–öç2‡w&ö6öFRæ6öFR¢æ–çFõö—FW"‚¢æÖ‡Â…òÂ÷6öFR—Â÷6öFR¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢³†&"ÂƒS’Âƒ&"Âƒ&2Â†#rÂ†#ÒÀ¢%'W7B&6¶VæBÆöG2F†RGvò÷&–v–æÂf÷&ÖÂ6Æ÷G2F—&V7FÇ’Âv—F†÷WBÆÆö6F–ær6÷VçFW" ¢“°¢76W'Eö–çfö6F–öå÷6WVVæ6R€¢fVÖ—GFVEö6Æ72À¢w&ö6öFRæ6öFRÀ¢e²€¢†#rÀ¢&W†×ÆRõ—""À¢#Æ–æ—Câ"À¢"„ÆW†×ÆRô6÷VçFW#´ÆW†×ÆRô6÷VçFW#²•b"À¢•ÒÀ¢“°¢ö'6W'fVEö§fÕö'—FW2Ò6öÖR†VÖ—GFVEö'—FW2“°¢W&–çFÆâ€¢&GVÂ×&VfW&Væ6R6W76–öâ&V6V—B6÷W&6S×·6÷W&6Uö–C£÷Ò6Æ76W3Õ´6÷VçFW#§¶6÷VçFW#£÷ÒÅ—#§·—#£÷ÒÅ—$f7F÷'“§¶f7F÷'“£÷ÕÒw&÷G&VS×³£÷Òw&×¶ÖWF†öC£÷ÒÖWF†öE÷G—S×¶ÖWF†öE÷G—S£÷Òf÷&ÖÇ3Õ²‡¶f—'7E÷G&VS£÷ÒÇ¶f—'7C£÷ÒÇG—S×³£÷ÒÇ7ã×³£÷Ò’Â‡·6V6öæE÷G&VS£÷ÒÇ·6V6öæC£÷ÒÇG—S×³£÷ÒÇ7ã×³£÷Ò•Ò—%ö7F÷#×·—%ö6öç7G'V7F÷#£÷Ò—%÷FV×ÆFUöf÷&ÖÇ3×·—%÷FV×ÆFUöf÷&ÖÇ3£÷Ò—%ö7F÷%öf÷&ÖÇ3×·—%ö6öç7G'V7F÷%öf÷&ÖÇ3£÷Ò÷WFW%öÇ“×·6÷W&6U÷&‡3£÷Ò÷·G—VE÷&‡3£÷Ò6VÆV7C×·6÷W&6U÷6VÆV7C£÷Ò÷·G—VE÷6VÆV7C£÷ÒæWs×·6÷W&6UöæWs£÷Ò÷·G—VEöæWs£÷Ò"À¢6÷W&6UöÖWF†öBãÀ¢6–væGW&Rç&×5³ÒçG’À¢7BævWB‚¦f—'7E÷G&VR’ç÷6—F–öâÀ¢6–væGW&Rç&×5³ÒçG’À¢7BævWB‚§6V6öæE÷G&VR’ç÷6—F–öâÀ¢“°¢f÷"†÷&F–æÂÂ‡6÷W&6Uö&wVÖVçBÂf÷&ÖÂ’’–â6÷W&6Uö÷WFW ¢æ&w0¢æ—FW"‚¢æ6÷–VB‚¢ç¦—…¶f—'7BÂ6V6öæEÒ¢æVçVÖW&FR‚¢°¢ÆWBG&VT¶–æC£¤–FVçB…ò’Òf7BævWB‡6÷W&6Uö&wVÖVçB’æ¶–æBVÇ6R°¢æ–2‚'F†R÷WFW"&wVÖVçG2&R÷&–v–æÂf÷&ÖÂ–FVçBÆVfW2"“°¢Ó°¢ÆWBG—VEö&wVÖVçBÒ6÷W&6U÷G—VEö–æFW‚ævWB‡6÷W&6Uö–BÂ6÷W&6Uö&wVÖVçB’çVçw&‚“°¢76W'B†ÖF6†W2€¢Væ—BçG—VEö7BævWB‡G—VEö&wVÖVçB’æ¶–æBÀ¢G&VT¶–æC£¤–FVçB…ò¢’“°¢76W'EöW€¢FW&Õ÷G—U÷F&vWB‡Væ—BÂVæ—BçG—VEö7BævWB‡G—VEö&wVÖVçB’çG’’À¢6öÖR†f÷&ÖÂ¢“°¢W&–çFÆâ€¢&GVÂ×&VfW&Væ6R&wVÖVçE·¶÷&F–æÇÕÓ×·6÷W&6Uö&wVÖVçC£÷Ò÷·G—VEö&wVÖVçC£÷Ò6÷W&6U÷7ã×³£÷Òf÷&ÖÃ×¶f÷&ÖÃ£÷ÒG—VE÷G—S×³£÷Ò"À¢7BævWB‡6÷W&6Uö&wVÖVçB’ç÷6—F–öâÀ¢Væ—BçG—VEö7BævWB‡G—VEö&wVÖVçB’çG¢“°¢76W'EöæR‡6÷W&6Uö÷WFW"æ&w5³ÒÂ6÷W&6Uö÷WFW"æ&w5³Ò“°¢76W'EöæR‡G—VEö÷WFW"æ&w5³ÒÂG—VEö÷WFW"æ&w5³Ò“°¢76W'EöæR‡6÷W&6Uö&wVÖVçBæ–æFW‚‚’Â6÷W&6U÷&‡2æ–æFW‚‚’“°¢76W'EöW†÷&F–æÂÂ–bf÷&ÖÂÓÒf—'7B²ÒVÇ6R²Ò“°¢Ğ¢ÆWB&W7VÇBĞ¢fÆ–FFUö7&÷75ö6Æ75ö6öç7G'V7F÷%ö6ÆÅ÷&öög2„7&÷746Æ746öç7G'V7F÷$6ÆÅ&ööd–çWB°¢6÷W&6Uö–BÀ¢6÷W&6U÷&ö÷BÀ¢7F÷&S¢Væ—Bç7F÷&RÀ¢FVf–æ—F–öç2À¢6÷W&6Uö7C¢7BÀ¢G—VEö7C¢Væ—BçG—VEö7BÀ¢6÷W&6Uö–æFWƒ¢–æFW‚À¢6÷W&6U÷G—VEö–æFW‚À¢§fÕ÷Væ—C¢Væ—BÀ¢Ò“°¢&ööeöW'&÷"Ò6öÖR†ÖF6‚&W7VÇB°¢ö²‡&öög2’Óâ°¢76W'EöW‡&öög2æÆVâ‚’Â“°¢ÆWB&ööbÒg&öög5³Ó°¢76W'EöW€¢&ööbæ&wVÖVçE÷&ööb‚’æ¶–æB‚’À¢F÷GG“£¦§fÕ÷F7G•ö7&÷75ö6Æ73£¤6öç7G'V7F÷$&wVÖVçE&ööd¶–æC£¤F—&V7Df÷&ÖÅ— ¢“°¢76W'EöW‡&ööbæf÷&ÖÅö6Æ72‚’Â6öÖR†6÷VçFW"’“°¢76W'EöW‡&ööbçF&vWEö6Æ72‚’Â—"“°¢76W'EöW‡&ööbæ6öç7G'V7F÷"‚’Â—%ö6öç7G'V7F÷"“°¢76W'EöW€¢&ööbæÖWF†öEöFW67&—F÷"‚’À¢"„ÆW†×ÆRô6÷VçFW#´ÆW†×ÆRô6÷VçFW#²”ÆW†×ÆRõ—#² ¢“°¢76W'EöW€¢&ööbæ6öç7G'V7F÷%öFW67&—F÷"‚’À¢"„ÆW†×ÆRô6÷VçFW#´ÆW†×ÆRô6÷VçFW#²•b ¢“°¢ÆWB÷&FW&VBÒ&ööbæ÷&FW&VEö&wVÖVçE÷—"‚’çVçw&‚“°¢76W'EöW†÷&FW&VBæÖWF†öEöf÷&ÖÇ2‚’Â¶f—'7BÂ6V6öæEÒ“°¢76W'EöW†÷&FW&VBç6÷W&6UöÖWF†öEöf÷&ÖÇ2‚’Â²¦f—'7E÷G&VRÂ§6V6öæE÷G&VUÒ“°¢76W'EöW€¢÷&FW&VBç6÷W&6Uö6öç7G'V7F÷%öf÷&ÖÇ2‚’æ5÷6Æ–6R‚’À¢—%ö6öç7G'V7F÷%öFVbçfÇVU÷&Õö6ÆW6W5³Òæ5÷6Æ–6R‚¢“°¢76W'EöW€¢÷&FW&VBæ6öç7G'V7F÷%öf÷&ÖÇ2‚’æ5÷6Æ–6R‚’À¢—%ö6öç7G'V7F÷%öf÷&ÖÇ2æ5÷6Æ–6R‚¢“°¢76W'B€¢÷&FW&VBæÖWF†öEöf÷&ÖÇ2‚’æ—FW"‚’æÆÂ‡Æf÷&ÖÇÂVæ—@¢ç7F÷&P¢ç7–Ö&öÇ0¢ævWB‚¦f÷&ÖÂ¢æ÷væW ¢ÓÒ6öÖR†ÖWF†öB’¢“°¢76W'B€¢÷&FW&VBæ6öç7G'V7F÷%öf÷&ÖÇ2‚’æ—FW"‚’æÆÂ‡Æf÷&ÖÇÂVæ—@¢ç7F÷&P¢ç7–Ö&öÇ0¢ævWB‚¦f÷&ÖÂ¢æ÷væW ¢ÓÒ6öÖR‡—%ö6öç7G'V7F÷"’¢“°¢f÷"†–æFW‚Âf÷&ÖÂ’–â¶f—'7BÂ6V6öæEÒæ–çFõö—FW"‚’æVçVÖW&FR‚’°¢ÆWB&wVÖVçBÒ÷&FW&VBæ&wVÖVçG2‚•¶–æFW…Ó°¢ÆWB6÷W&6Uö&wVÖVçBÒ6÷W&6Uö÷WFW"æ&w5¶–æFW…Ó°¢ÆWBG—VEö&wVÖVçBÒG—VEö÷WFW"æ&w5¶–æFW…Ó°¢76W'EöW†&wVÖVçBç6÷W&6Uö&wVÖVçB‚’Â6÷W&6Uö&wVÖVçB“°¢76W'EöW†&wVÖVçBçG—VEö&wVÖVçB‚’ÂG—VEö&wVÖVçB“°¢76W'EöW†&wVÖVçBæf÷&ÖÂ‚’çVçw&‚’æf÷&ÖÂ‚’Âf÷&ÖÂ“°¢76W'B†&wVÖVçBææW7FVEöæWr‚’æ—5öæöæR‚’“°¢Ğ¢f÷&ÖB‚$ö²‡·Ò&öög2’"Â&öög2æÆVâ‚’¢Ğ¢W'"†W'&÷"’Óâf÷&ÖB‚$W'"‡¶W'&÷#£÷Ò’"’À¢Ò“°¢ÆWB×WB7FÆU÷6÷W&6U÷G—VEö–æFW‚Ò6÷W&6U÷G—VEö–æFW‚æ6ÆöæR‚“°¢ÆWB6V6öæEö&wVÖVçBÒ6÷W&6Uö÷WFW"æ&w5³Ó°¢ÆWBf—'7E÷G—VEö&wVÖVçBÒ6÷W&6U÷G—VEö–æFW€¢ævWB‡6÷W&6Uö–BÂ6÷W&6Uö÷WFW"æ&w5³Ò¢çVçw&‚“°¢7FÆU÷6÷W&6U÷G—VEö–æFW€¢ç&VÖ÷fR‡6÷W&6Uö–BÂ6V6öæEö&wVÖVçB¢çVçw&‚“°¢7FÆU÷6÷W&6U÷G—VEö–æFW€¢æ–ç6W'B‡6÷W&6Uö–BÂ6V6öæEö&wVÖVçBÂf—'7E÷G—VEö&wVÖVçB¢çVçw&‚“°¢76W'EöæR€¢f—'7E÷G—VEö&wVÖVçBÀ¢6÷W&6U÷G—VEö–æFW‚ævWB‡6÷W&6Uö–BÂ6V6öæEö&wVÖVçB’çVçw&‚¢“°¢ÆWB7FÆU÷&W7VÇBĞ¢fÆ–FFUö7&÷75ö6Æ75ö6öç7G'V7F÷%ö6ÆÅ÷&öög2„7&÷746Æ746öç7G'V7F÷$6ÆÅ&ööd–çWB°¢6÷W&6Uö–BÀ¢6÷W&6U÷&ö÷BÀ¢7F÷&S¢Væ—Bç7F÷&RÀ¢FVf–æ—F–öç2À¢6÷W&6Uö7C¢7BÀ¢G—VEö7C¢Væ—BçG—VEö7BÀ¢6÷W&6Uö–æFWƒ¢–æFW‚À¢6÷W&6U÷G—VEö–æFWƒ¢g7FÆU÷6÷W&6U÷G—VEö–æFW‚À¢§fÕ÷Væ—C¢Væ—BÀ¢Ò“°¢ÆWB7FÆU÷&W7VÇBÒÖF6‚7FÆU÷&W7VÇB°¢ö²‡&öög2’Óâf÷&ÖB‚$ö²‡·Ò&öög2’"Â&öög2æÆVâ‚’’À¢W'"†W'&÷"’Óâf÷&ÖB‚$W'"‡¶W'&÷#£÷Ò’"’À¢Ó°¢7FÆUöW'&÷"Ò6öÖR‡7FÆU÷&W7VÇBæ6ÆöæR‚’“° ¢ÆWB×WB7vVE÷6÷W&6U÷G—VEö–æFW‚Ò6÷W&6U÷G—VEö–æFW‚æ6ÆöæR‚“°¢ÆWBf—'7Eö&wVÖVçBÒ6÷W&6Uö÷WFW"æ&w5³Ó°¢ÆWB6V6öæE÷G—VEö&wVÖVçBÒ6÷W&6U÷G—VEö–æFW€¢ævWB‡6÷W&6Uö–BÂ6÷W&6Uö÷WFW"æ&w5³Ò¢çVçw&‚“°¢7vVE÷6÷W&6U÷G—VEö–æFW€¢ç&VÖ÷fR‡6÷W&6Uö–BÂf—'7Eö&wVÖVçB¢çVçw&‚“°¢7vVE÷6÷W&6U÷G—VEö–æFW€¢æ–ç6W'B‡6÷W&6Uö–BÂf—'7Eö&wVÖVçBÂ6V6öæE÷G—VEö&wVÖVçB¢çVçw&‚“°¢ÆWB6V6öæEö&wVÖVçBÒ6÷W&6Uö÷WFW"æ&w5³Ó°¢ÆWBf—'7E÷G—VEö&wVÖVçBÒ6÷W&6U÷G—VEö–æFW‚ævWB‡6÷W&6Uö–BÂf—'7Eö&wVÖVçB’çVçw&‚“°¢7vVE÷6÷W&6U÷G—VEö–æFW€¢ç&VÖ÷fR‡6÷W&6Uö–BÂ6V6öæEö&wVÖVçB¢çVçw&‚“°¢7vVE÷6÷W&6U÷G—VEö–æFW€¢æ–ç6W'B‡6÷W&6Uö–BÂ6V6öæEö&wVÖVçBÂf—'7E÷G—VEö&wVÖVçB¢çVçw&‚“°¢ÆWB7vVE÷&W7VÇBĞ¢fÆ–FFUö7&÷75ö6Æ75ö6öç7G'V7F÷%ö6ÆÅ÷&öög2„7&÷746Æ746öç7G'V7F÷$6ÆÅ&ööd–çWB°¢6÷W&6Uö–BÀ¢6÷W&6U÷&ö÷BÀ¢7F÷&S¢Væ—Bç7F÷&RÀ¢FVf–æ—F–öç2À¢6÷W&6Uö7C¢7BÀ¢G—VEö7C¢Væ—BçG—VEö7BÀ¢6÷W&6Uö–æFWƒ¢–æFW‚À¢6÷W&6U÷G—VEö–æFWƒ¢g7vVE÷6÷W&6U÷G—VEö–æFW‚À¢§fÕ÷Væ—C¢Væ—BÀ¢Ò“°¢7vVEöW'&÷"Ò6öÖR†ÖF6‚7vVE÷&W7VÇB°¢ö²‡&öög2’Óâf÷&ÖB‚$ö²‡·Ò&öög2’"Â&öög2æÆVâ‚’’À¢W'"†W'&÷"’Óâf÷&ÖB‚$W'"‡¶W'&÷#£÷Ò’"’À¢Ò“° ¢ÆWB×WBW‡FVæFVE÷G—VEö7C¢F÷GG•ö6÷&S£¦7C£¥G—VD7BÒ7D&Væ£¦æWr‚“°¢f÷"…òÂG&VR’–âVæ—BçG—VEö7Bæ—FW"‚’°¢W‡FVæFVE÷G—VEö7BæÆÆö2‡G&VRæ6ÆöæR‚’“°¢Ğ¢ÆWB7FÆU÷G—VE÷&ö÷BĞ¢W‡FVæFVE÷G—VEö7BæÆÆö2‡Væ—BçG—VEö7BævWB‡G—VEö÷WFW"æ&w5³Ò’æ6ÆöæR‚’“°¢ÆWB×WB7FÆU÷G—VE÷&ö÷Eö–æFW‚Ò6÷W&6U÷G—VEö–æFW‚æ6ÆöæR‚“°¢7FÆU÷G—VE÷&ö÷Eö–æFW€¢ç&VÖ÷fR‡6÷W&6Uö–BÂ6V6öæEö&wVÖVçB¢çVçw&‚“°¢7FÆU÷G—VE÷&ö÷Eö–æFW€¢æ–ç6W'B‡6÷W&6Uö–BÂ6V6öæEö&wVÖVçBÂ7FÆU÷G—VE÷&ö÷B¢çVçw&‚“°¢ÆWB7FÆU÷G—VE÷&ö÷E÷&W7VÇBĞ¢fÆ–FFUö7&÷75ö6Æ75ö6öç7G'V7F÷%ö6ÆÅ÷&öög2„7&÷746Æ746öç7G'V7F÷$6ÆÅ&ööd–çWB°¢6÷W&6Uö–BÀ¢6÷W&6U÷&ö÷BÀ¢7F÷&S¢Væ—Bç7F÷&RÀ¢FVf–æ—F–öç2À¢6÷W&6Uö7C¢7BÀ¢G—VEö7C¢Væ—BçG—VEö7BÀ¢6÷W&6Uö–æFWƒ¢–æFW‚À¢6÷W&6U÷G—VEö–æFWƒ¢g7FÆU÷G—VE÷&ö÷Eö–æFW‚À¢§fÕ÷Væ—C¢Væ—BÀ¢Ò“°¢7FÆU÷G—VE÷&ö÷EöW'&÷"Ò6öÖR†ÖF6‚7FÆU÷G—VE÷&ö÷E÷&W7VÇB°¢ö²‡&öög2’Óâf÷&ÖB‚$ö²‡·Ò&öög2’"Â&öög2æÆVâ‚’’À¢W'"†W'&÷"’Óâf÷&ÖB‚$W'"‡¶W'&÷#£÷Ò’"’À¢Ò“° ¢ÆWB×WBW‡FVæFVE÷6÷W&6Uö7C¢7D&VæÅVçG—VCâÒ7D&Væ£¦æWr‚“°¢f÷"…òÂG&VR’–â7Bæ—FW"‚’°¢W‡FVæFVE÷6÷W&6Uö7BæÆÆö2‡G&VRæ6ÆöæR‚’“°¢Ğ¢ÆWB7FÆU÷6÷W&6U÷&ö÷BÒW‡FVæFVE÷6÷W&6Uö7BæÆÆö2†7BævWB‡6÷W&6U÷&ö÷B’æ6ÆöæR‚’“°¢ÆWB7FÆU÷6÷W&6U÷&ö÷E÷&W7VÇBĞ¢fÆ–FFUö7&÷75ö6Æ75ö6öç7G'V7F÷%ö6ÆÅ÷&öög2„7&÷746Æ746öç7G'V7F÷$6ÆÅ&ööd–çWB°¢6÷W&6Uö–BÀ¢6÷W&6U÷&ö÷C¢7FÆU÷6÷W&6U÷&ö÷BÀ¢7F÷&S¢Væ—Bç7F÷&RÀ¢FVf–æ—F–öç2À¢6÷W&6Uö7C¢7BÀ¢G—VEö7C¢Væ—BçG—VEö7BÀ¢6÷W&6Uö–æFWƒ¢–æFW‚À¢6÷W&6U÷G—VEö–æFW‚À¢§fÕ÷Væ—C¢Væ—BÀ¢Ò“°¢7FÆU÷6÷W&6U÷&ö÷EöW'&÷"Ò6öÖR†ÖF6‚7FÆU÷6÷W&6U÷&ö÷E÷&W7VÇB°¢ö²‡&öög2’Óâf÷&ÖB‚$ö²‡·Ò&öög2’"Â&öög2æÆVâ‚’’À¢W'"†W'&÷"’Óâf÷&ÖB‚$W'"‡¶W'&÷#£÷Ò’"’À¢Ò“° ¢ÆWBf÷&V–vå÷6÷W&6U÷&W7VÇBĞ¢fÆ–FFUö7&÷75ö6Æ75ö6öç7G'V7F÷%ö6ÆÅ÷&öög2„7&÷746Æ746öç7G'V7F÷$6ÆÅ&ööd–çWB°¢6÷W&6Uö–C¢6÷W&6T–C£¦g&öÕö–æFW‚ƒ“c“’’À¢6÷W&6U÷&ö÷BÀ¢7F÷&S¢Væ—Bç7F÷&RÀ¢FVf–æ—F–öç2À¢6÷W&6Uö7C¢7BÀ¢G—VEö7C¢Væ—BçG—VEö7BÀ¢6÷W&6Uö–æFWƒ¢–æFW‚À¢6÷W&6U÷G—VEö–æFW‚À¢§fÕ÷Væ—C¢Væ—BÀ¢Ò“°¢f÷&V–vå÷6÷W&6UöW'&÷"Ò6öÖR†ÖF6‚f÷&V–vå÷6÷W&6U÷&W7VÇB°¢ö²‡&öög2’Óâf÷&ÖB‚$ö²‡·Ò&öög2’"Â&öög2æÆVâ‚’’À¢W'"†W'&÷"’Óâf÷&ÖB‚$W'"‡¶W'&÷#£÷Ò’"’À¢Ò“°¢W&–çFÆâ€¢&GVÂ×&VfW&Væ6RFVÆ–&W&FVÇ’7FÆR6÷W&6R×G—VBVFvRf—'7B&W7VÇC¢·7FÆU÷&W7VÇGÒ ¢“°¢Ó°¢ÆWB'F–f7G2Ò6ö×–ÆU÷6÷W&6Uö&F6…÷v—F…÷G—W%öæE÷6÷W&6Uöö'6W'fW"€¢4õU$4RÀ¢%—$f7F÷'’ç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6Uö–BÀ¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢f×WBö'6W'fU÷G—W"À¢f×WBö'6W'fU÷6÷W&6RÀ¢¢æW‡V7B‚$¥dÒÖöæÇ’&6¶VæB66WG2F†RW†7Bf÷'v&F–ær6÷W&6R"“°¢76W'B€¢ö'6W'fVBÀ¢&ÆÂ6÷W&6R–FVçF—F–W26ÖRg&öÒöæRvVçV–æRg&öçFVæB6W76–öâ ¢“°¢76W'EöW€¢'F–f7G0¢æ—FW"‚¢æÖ‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’¢æ6öÆÆV7C££ÅfV3Åóãâ‚’À¢²&W†×ÆRô6÷VçFW""Â&W†×ÆRõ—""Â&W†×ÆRõ—$f7F÷'’%Ğ¢“°¢ÆWBf7F÷'•ö'F–f7BÒ'F–f7G0¢æ—FW"‚¢æf–æB‡Æ'F–f7GÂ'F–f7Bæ6Æ72æ–çFW&æÅöæÖR‚’ÓÒ&W†×ÆRõ—$f7F÷'’"¢çVçw&‚“°¢76W'EöW€¢f7F÷'•ö'F–f7Bæ6Æ72æ'—FW2‚’À¢ö'6W'fVEö§fÕö'—FW2æ5öFW&Vb‚’çVçw&‚’À¢'F†RÖV7W&VB¥dÒ÷WGWB—2W†7FÇ’F†Ræ÷&ÖÂV&Æ—6†VB6÷W&6R'F–f7B ¢“°¢ÆWB×WB—&VEö6ÆÆ&6µ÷&âÒfÇ6S°¢ÆWB—&VBÒF÷GG“£¦§fÕ÷F7G•÷Æã£§v—F…÷—&VEö§fÕ÷F7G•ö&F6…÷Æâ€¢4õU$4RÀ¢%—$f7F÷'’ç66Æ"À¢6÷W&6T–C£¦g&öÕö–æFW‚ƒ“c"’À¢Å÷Â—&VEö6ÆÆ&6µ÷&âÒG'VRÀ¢“°¢ÆWB&ööeöW'&÷"Ò&ööeöW'&÷"çVçw&‚“°¢ÆWB–6¶ÆW%öW'&÷"Ò–6¶ÆW%öW'&÷"çVçw&‚“°¢ÆWB7FÆUöW'&÷"Ò7FÆUöW'&÷"çVçw&‚“°¢ÆWB7vVEöW'&÷"Ò7vVEöW'&÷"çVçw&‚“°¢ÆWB7FÆU÷G—VE÷&ö÷EöW'&÷"Ò7FÆU÷G—VE÷&ö÷EöW'&÷"çVçw&‚“°¢ÆWB7FÆU÷6÷W&6U÷&ö÷EöW'&÷"Ò7FÆU÷6÷W&6U÷&ö÷EöW'&÷"çVçw&‚“°¢ÆWBf÷&V–vå÷6÷W&6UöW'&÷"Òf÷&V–vå÷6÷W&6UöW'&÷"çVçw&‚“°¢ÆWB—&VEöW'&÷"ÒÖF6‚—&VB°¢ö²…ò’Óâ$ö²†6ÆÆ&6²&V6†VB’"çFõö÷væVB‚’À¢W'"†W'&÷"’Óâf÷&ÖB‚$W'"‡¶W'&÷#£÷Ò’"’À¢Ó°¢W&–çFÆâ‚&F—&V7Bf÷&ÖÂ&ööc¢·&ööeöW'&÷'Ò"“°¢W&–çFÆâ‚'7FÆR6V6öæB6÷W&6R×G—VBVFvS¢·7FÆUöW'&÷'Ò"“°¢W&–çFÆâ‚'7vVB6÷W&6R×G—VBVFvW3¢·7vVEöW'&÷'Ò"“°¢W&–çFÆâ‚'7FÆRG—VB&ö÷C¢·7FÆU÷G—VE÷&ö÷EöW'&÷'Ò"“°¢W&–çFÆâ‚'7FÆR6÷W&6R&ö÷C¢·7FÆU÷6÷W&6U÷&ö÷EöW'&÷'Ò"“°¢W&–çFÆâ‚&f÷&V–vâ6÷W&6R–C¢¶f÷&V–vå÷6÷W&6UöW'&÷'Ò"“°¢W&–çFÆâ‚'—&VBÆææW"f—'7B&W7VÇC¢·—&VEöW'&÷'Ò"“°¢76W'EöW‡&ööeöW'&÷"Â$ö²ƒ&öög2’"“°¢76W'EöW€¢7FÆUöW'&÷"À¢$W'"…G—VD&–æF–ær…Â'6–&Æ–ær÷WFW"&wVÖVçG2×W7B&WF–âF—7F–æ7B6÷W&6RæBG—VB&ö÷G5Â"’’ ¢“°¢76W'B‡7vVEöW'&÷"æ6öçF–ç2‚%G—VD&–æF–ær"’“°¢76W'B‡7FÆU÷G—VE÷&ö÷EöW'&÷"æ6öçF–ç2‚%G—VD&–æF–ær"’“°¢76W'B‡7FÆU÷6÷W&6U÷&ö÷EöW'&÷"æ6öçF–ç2‚%6÷W&6U6†R"’“°¢76W'B†f÷&V–vå÷6÷W&6UöW'&÷"æ6öçF–ç2‚%6÷W&6T–FVçF—G’"’“°¢76W'EöW€¢–6¶ÆW%öW'&÷"À¢$W'"…Vç7W÷'FVB²fVGW&S¢6Æ74ÖVÖ&W'2Â7–Ö&öÃ¢6öÖR…7–Ö&öÄ–Bƒc’’Ò’ ¢“°¢76W'B‡—&VEöW'&÷"æ6öçF–ç2‚'†6S¢Â$¥dÒõD5G’7&÷72Ö6öçG&7BfÆ–FF–öåÂ""’“°¢76W'B‡—&VEöW'&÷"æ6öçF–ç2‚$ÖÆf÷&ÖVEG—R"’“°¢76W'B‡—&VEöW'&÷"æ6öçF–ç2‚'&öÆS¢&ÖWFW""’“°¢76W'B€¢—&VEö6ÆÆ&6µ÷&âÀ¢'Vç7W÷'FVBD5G’6–væGW&R6ææ÷BV&Æ—6‚&F6‚ ¢“°¢W&–çFÆâ‚&GVÂ×&VfW&Væ6RÖf÷'v&F–ær&ööb&W7VÇC¢·&ööeöW'&÷'Ò"“°¢W&–çFÆâ‚&GVÂ×&VfW&Væ6RÖf÷'v&F–ær7FæFÆöæR–6¶ÆW"6–væGW&R&W7VÇC¢·–6¶ÆW%öW'&÷'Ò"“°¢W&–çFÆâ‚&GVÂ×&VfW&Væ6RÖf÷'v&F–ær—&VBÆææW"f—'7B&W7VÇC¢·—&VEöW'&÷'Ò"“°§Ğ ¦fâ76W'EöF—&V7Eöf÷&ÖÅ÷—%÷f&–çB€¢6÷W&6S¢g7G"À¢6÷W&6UöçVÖ&W#¢S3"À¢ÖWF†öEöf÷&ÖÅöæÖW3¢²g7G#²%ÒÀ¢&wVÖVçEöæÖW3¢²g7G#²%ÒÀ¢’°¢W6RF÷GG“£¦§fÕ÷F7G•ö7&÷75ö6Æ73£§°¢6öç7G'V7F÷$&wVÖVçE&ööd¶–æBÂ7&÷746Æ746öç7G'V7F÷$6ÆÅ&ööd–çWBÀ¢fÆ–FFUö7&÷75ö6Æ75ö6öç7G'V7F÷%ö6ÆÅ÷&öög2À¢Ó° ¢ÆWB6÷W&6Uö–BÒ6÷W&6T–C£¦g&öÕö–æFW‚‡6÷W&6UöçVÖ&W"“°¢ÆWB÷WGWBÒFW7D÷WGWDF—&V7F÷'“£¦æWr‚ff÷&ÖB‚&F—&V7BÖf÷&ÖÂ×—"×·6÷W&6UöçVÖ&W'Ò"’“°¢ÆWB×WBö'6W'fVBÒfÇ6S°¢ÆWB×WBö'6W'fU÷G—W"ÒÅó¢d7D&VæÅVçG—VCâÀ¢ó¢G&VT–CÅVçG—VCâÀ¢ó¢e6÷W&6U6VÖçF–4–æFW‚À¢ó¢f×WB6÷W&6UG—W#ÂuóâÀ¢ó¢dFVf–æ—F–öç7Â·Ó°¢ÆWB×WBö'6W'fU÷6÷W&6RÒÆ7C¢d7D&VæÅVçG—VCâÀ¢6÷W&6U÷&ö÷C¢G&VT–CÅVçG—VCâÀ¢6÷W&6Uö–æFWƒ¢e6÷W&6U6VÖçF–4–æFW‚À¢6÷W&6U÷G—VEö–æFWƒ¢e6÷W&6UG—VD–æFW‚À¢Væ—C¢f×WB§fÔ6ö×–ÆF–öåVæ—CÂuóâÀ¢FVf–æ—F–öç3¢dFVf–æ—F–öç7Â°¢ö'6W'fVBÒG'VS°¢ÆWB&öög2Ğ¢fÆ–FFUö7&÷75ö6Æ75ö6öç7G'V7F÷%ö6ÆÅ÷&öög2„7&÷746Æ746öç7G'V7F÷$6ÆÅ&ööd–çWB°¢6÷W&6Uö–BÀ¢6÷W&6U÷&ö÷BÀ¢7F÷&S¢Væ—Bç7F÷&RÀ¢FVf–æ—F–öç2À¢6÷W&6Uö7C¢7BÀ¢G—VEö7C¢Væ—BçG—VEö7BÀ¢6÷W&6Uö–æFW‚À¢6÷W&6U÷G—VEö–æFW‚À¢§fÕ÷Væ—C¢Væ—BÀ¢Ò¢æW‡V7B‚'F†—26÷W&6R6†R†2öæRgVÆÇ’WF†VçF–6FVBF—&V7BÖf÷&ÖÂ&ööb"“°¢76W'EöW‡&öög2æÆVâ‚’Â“°¢ÆWB&ööbÒg&öög5³Ó°¢76W'EöW€¢&ööbæ&wVÖVçE÷&ööb‚’æ¶–æB‚’À¢6öç7G'V7F÷$&wVÖVçE&ööd¶–æC£¤F—&V7Df÷&ÖÅ— ¢“°¢ÆWB÷&FW&VBÒ&ööbæ÷&FW&VEö&wVÖVçE÷—"‚’çVçw&‚“°¢76W'EöæR†÷&FW&VBæÖWF†öEöf÷&ÖÇ2‚•³ÒÂ÷&FW&VBæÖWF†öEöf÷&ÖÇ2‚•³Ò“°¢76W'EöæR€¢÷&FW&VBæ6öç7G'V7F÷%öf÷&ÖÇ2‚•³ÒÀ¢÷&FW&VBæ6öç7G'V7F÷%öf÷&ÖÇ2‚•³Ğ¢“°¢76W'B†÷&FW&VBæÖWF†öEöf÷&ÖÇ2‚’æ—FW"‚’æÆÂ‡Æf÷&ÖÇÂ°¢Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB‚¦f÷&ÖÂ’æ÷væW"ÓÒ6öÖR‡&ööbæÖWF†öB‚’¢bbVæ—Bç7F÷&Rç7–Ö&öÇ2ævWB‚¦f÷&ÖÂ’æ¶–æBÓÒ7–Ö&öÄ¶–æC£¥&ÖWFW ¢Ò’“°¢76W'B†÷&FW&VBæ6öç7G'V7F÷%öf÷&ÖÇ2‚’æ—FW"‚’æÆÂ‡Æf÷&ÖÇÂ°¢Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB‚¦f÷&ÖÂ’æ÷væW"ÓÒ6öÖR‡&ööbæ6öç7G'V7F÷"‚’¢bbVæ—Bç7F÷&Rç7–Ö&öÇ2ævWB‚¦f÷&ÖÂ’æ¶–æBÓÒ7–Ö&öÄ¶–æC£¥&ÖWFW ¢Ò’“°¢76W'B€¢÷&FW&V@¢æÖWF†öEöf÷&ÖÇ2‚¢æ—FW"‚¢æÆÂ‡Æf÷&ÖÇÂ²÷&FW&VBæ6öç7G'V7F÷%öf÷&ÖÇ2‚’æ6öçF–ç2†f÷&ÖÂ’Ò¢“°¢f÷"†–æFW‚Âf÷&ÖÅöæÖR’–âÖWF†öEöf÷&ÖÅöæÖW2æ–çFõö—FW"‚’æVçVÖW&FR‚’°¢ÆWBf÷&ÖÂÒ÷&FW&VBæÖWF†öEöf÷&ÖÇ2‚•¶–æFW…Ó°¢76W'EöW€¢Væ—Bç7F÷&P¢ææÖW0¢ç&W6öÇfR‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB†f÷&ÖÂ’ææÖRçFW‡B‚’’À¢f÷&ÖÅöæÖP¢“°¢76W'B†ÖF6†W2€¢÷&FW&VBç6÷W&6UöÖWF†öEöf÷&ÖÇ2‚’ævWB†–æFW‚’ææE÷F†Vâ‡ÇG&VWÂ7BçG'•övWB‚§G&VR’’À¢6öÖR†æöFR’–bÖF6†W2‚fæöFRæ¶–æBÂG&VT¶–æC£¥fÄFVb‡&ÖWFW"’–bVæ—Bç7F÷&RææÖW2ç&W6öÇfR‡&ÖWFW"ææÖRæ5öæÖR‚’çFW‡B‚’’ÓÒf÷&ÖÅöæÖR¢’“°¢Ğ¢f÷"†–æFW‚Â&wVÖVçEöæÖR’–â&wVÖVçEöæÖW2æ–çFõö—FW"‚’æVçVÖW&FR‚’°¢ÆWB&wVÖVçBÒ÷&FW&VBæ&wVÖVçG2‚•¶–æFW…Ó°¢ÆWB6÷W&6Uö&wVÖVçBÒ&wVÖVçBç6÷W&6Uö&wVÖVçB‚“°¢ÆWBG—VEö&wVÖVçBÒ&wVÖVçBçG—VEö&wVÖVçB‚“°¢76W'EöW€¢6÷W&6U÷G—VEö–æFW‚ævWB‡6÷W&6Uö–BÂ6÷W&6Uö&wVÖVçB’À¢6öÖR‡G—VEö&wVÖVçB¢“°¢ÆWBG&VT¶–æC£¤–FVçB‡6÷W&6Uö–FVçB’Òf7BævWB‡6÷W&6Uö&wVÖVçB’æ¶–æBVÇ6R°¢æ–2‚&F—&V7B—"6÷W&6R&wVÖVçB—2â÷&–v–æÂ–FVçB"¢Ó°¢76W'EöW€¢Væ—Bç7F÷&RææÖW2ç&W6öÇfR‡6÷W&6Uö–FVçBææÖRçFW‡B‚’’À¢&wVÖVçEöæÖP¢“°¢ÆWBf÷&ÖÂÒ&wVÖVçBæf÷&ÖÂ‚’çVçw&‚’æf÷&ÖÂ‚“°¢76W'EöW€¢FW&Õ÷G—U÷F&vWB‡Væ—BÂVæ—BçG—VEö7BævWB‡G—VEö&wVÖVçB’çG’’À¢6öÖR†f÷&ÖÂ¢“°¢76W'EöW€¢Væ—Bç7F÷&P¢ææÖW0¢ç&W6öÇfR‡Væ—Bç7F÷&Rç7–Ö&öÇ2ævWB†f÷&ÖÂ’ææÖRçFW‡B‚’’À¢&wVÖVçEöæÖP¢“°¢Ğ¢Ó°¢6ö×–ÆU÷6÷W&6Uö&F6…÷v—F…÷G—W%öæE÷6÷W&6Uöö'6W'fW"€¢6÷W&6RÀ¢%—$f7F÷'’ç66Æ"À¢÷WGWBçF‚‚’À¢6÷W&6Uö–BÀ¢6÷W&6UVæ—D6&F–æÆ—G“£¤DÆV7EGvòÀ¢f×WBö'6W'fU÷G—W"À¢f×WBö'6W'fU÷6÷W&6RÀ¢¢æW‡V7B‚$¥dÒÖöæÇ’6÷W&6R6ö×–ÆF–öâ&VÖ–ç27W÷'FVBf÷"F†—2&ööb6†R"“°¢76W'B€¢ö'6W'fVBÀ¢'F†R÷&–v–æÂöæR×6W76–öâ6÷W&6R&V6†VBF†R&ööb66â ¢“°§Ğ ¢5·FW7EĞ¦fâF—&V7Eöf÷&ÖÅ÷—%÷&ööe÷&W6W'fW5öæÖW5ö÷&FW%÷&WfW'6ÅöæEöÆ–6–ær‚’°¢6öç7B4õU$4S¢g7G"Ğ¢–æ6ÇVFU÷7G"‚&f—‡GW&W2ö§fÕ÷F7G•öGVÅ÷&VfW&Væ6Uöf÷'v&F–ærõ—$f7F÷'’ç66Æ"“°¢ÆWB6V7F–öç2Ò4õU$4Rç7Æ—B‚%ÆåÆâ"’æ6öÆÆV7C££ÅfV3Åóãâ‚“°¢76W'EöW‡6V7F–öç2æÆVâ‚’ÂB“°¢ÆWB&V÷&FW&VBÒf÷&ÖB€¢'·ÕÆåÆç·ÕÆåÆç·ÕÆåÆç·Ò"À¢6V7F–öç5³ÒÂ6V7F–öç5³%ÒÂ6V7F–öç5³5ÒÂ6V7F–öç5³Ğ¢“°¢ÆWB&VæÖVBÒ4õU$4P¢ç&WÆ6R‚'6¶vRW†×ÆR"Â'6¶vR&VæÖVB"¢ç&WÆ6R‚%—$f7F÷'’"Â$76VÖ&ÆW$f7F÷'’"¢ç&WÆ6R‚%—""Â$VçfVÆ÷R"¢ç&WÆ6R‚$6÷VçFW""Â$ÖWFW""¢ç&WÆ6R‚'w&"Â&f÷'v&B"¢ç&WÆ6R‚&f—'7B"Â'6÷W&6TÆVgB"¢ç&WÆ6R‚'6V6öæB"Â'6÷W&6U&–v‡B"“°¢ÆWB&WfW'6VEö&wVÖVçG2Ò4õU$4Rç&WÆ6R‚&æWr—"†f—'7BÂ6V6öæB’"Â&æWr—"‡6V6öæBÂf—'7B’"“°¢ÆWB&WfW'6VEöf÷&ÖÇ2Ò4õU$4Rç&WÆ6R€¢&FVbw&†f—'7C¢6÷VçFW"Â6V6öæC¢6÷VçFW"’"À¢&FVbw&‡6V6öæC¢6÷VçFW"Âf—'7C¢6÷VçFW"’"À¢“°¢ÆWB&WVFVEöf÷&ÖÂÒ4õU$4Rç&WÆ6R‚&æWr—"†f—'7BÂ6V6öæB’"Â&æWr—"†f—'7BÂf—'7B’"“° ¢76W'EöF—&V7Eöf÷&ÖÅ÷—%÷f&–çB…4õU$4RÂ“cÂ²&f—'7B"Â'6V6öæB%ÒÂ²&f—'7B"Â'6V6öæB%Ò“°¢76W'EöF—&V7Eöf÷&ÖÅ÷—%÷f&–çB€¢g&VæÖVBÀ¢“cÀ¢²'6÷W&6TÆVgB"Â'6÷W&6U&–v‡B%ÒÀ¢²'6÷W&6TÆVgB"Â'6÷W&6U&–v‡B%ÒÀ¢“°¢76W'EöF—&V7Eöf÷&ÖÅ÷—%÷f&–çB‚g&V÷&FW&VBÂ“c"Â²&f—'7B"Â'6V6öæB%ÒÂ²&f—'7B"Â'6V6öæB%Ò“°¢76W'EöF—&V7Eöf÷&ÖÅ÷—%÷f&–çB€¢g&WfW'6VEö&wVÖVçG2À¢“c2À¢²&f—'7B"Â'6V6öæB%ÒÀ¢²'6V6öæB"Â&f—'7B%ÒÀ¢“°¢76W'EöF—&V7Eöf÷&ÖÅ÷—%÷f&–çB€¢g&WfW'6VEöf÷&ÖÇ2À¢“cBÀ¢²'6V6öæB"Â&f—'7B%ÒÀ¢²&f—'7B"Â'6V6öæB%ÒÀ¢“°¢76W'EöF—&V7Eöf÷&ÖÅ÷—%÷f&–çB€¢g&WVFVEöf÷&ÖÂÀ¢“cRÀ¢²&f—'7B"Â'6V6öæB%ÒÀ¢²&f—'7B"Â&f—'7B%ÒÀ¢“°§Ğ