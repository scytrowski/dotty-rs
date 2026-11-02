Warning: truncated output (original token count: 277733)
... 62354 bytes omitted ...

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
    Definitions, Packages, SemanticStore, SourceId, SourceSpan, SourceText, SourceTypedIndex, Span,
    SymbolFlags, SymbolId, SymbolInfo, SymbolKind, TextRange, TreeId, Visibility,
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
const MIXED_ORIGIN_WRAP_AND_CREATE_SOURCE: &str = include_str!(
    "fixtures/jvm_tasty_mixed_origin_constructor_arguments/wrap-and-create/PairFactory.scala"
);
const MIXED_ORIGIN_CREATE_AND_WRAP_SOURCE: &str = include_str!(
    "fixtures/jvm_tasty_mixed_origin_constructor_arguments/create-and-wrap/PairFactory.scala"
);

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
type SourceStoreMutator<'observer> =
    dyn FnMut(&AstArena<Untyped>, SourceId, &SourceSemanticIndex, &mut SemanticStore) + 'observer;
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
    source_store_mutator: Option<&'observer mut SourceStoreMutator<'observer>>,
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

fn compile_source_batch_with_store_mutator_and_source_observer<'observer>(
    source_text: &str,
    source_file_name: &str,
    output_directory: &Path,
    source_id: SourceId,
    cardinality: SourceUnitCardinality,
    mutate_store: &'observer mut SourceStoreMutator<'observer>,
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
            source_store_mutator: Some(mutate_store),
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
        source_store_mutator: mut mutate_source_store,
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
    macro_rules! finish_unit {
        ($unit:ident, $source_typed_index:expr) => {{
            if let Some(observe) = observe_source.as_mut() {
                observe(
                    &parsed.ast,
                    parsed.root,
                    &index,
                    $source_typed_index,
                    &mut $unit,
                    &definitions,
                );
            }
            if let Some(observe) = observe.as_mut() {
                observe(&mut $unit, &definitions);
            }
            let mut generated = Vec::with_capacity($unit.classes.len());
            for plan in &$unit.classes {
                generated.push(
                    emit_class(&$unit, plan.class, &definitions)
                        .map_err(PipelineFailure::Codegen)?,
                );
            }
            publish_class_batch(output_directory, generated)
        }};
    }
    if let Some(mutator) = mutate_source_store.as_mut() {
        let typed_ast = copy_ast(typer.typed_ast());
        let source_typed_index = typer.source_typed_index().clone();
        drop(typer);
        mutator(&parsed.ast, source_id, &index, &mut store);
        let mut unit = JvmCompilationUnit {
            store: &store,
            typed_ast: &typed_ast,
            classes: plans,
        };
        finish_unit!(unit, &source_typed_index)
    } else {
        let mut unit = JvmCompilationUnit {
            store: typer.store(),
            typed_ast: typer.typed_ast(),
            classes: plans,
        };
        finish_unit!(unit, typer.source_typed_index())
    }
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

fn copy_ast<P: dotty_core::AstPhase + Clone>(ast: &AstArena<P>) -> AstArena<P> {
    let mut copied = AstArena::new();
    for (_, tree) in ast.iter() {
        copied.alloc(tree.clone());
    }
    copied
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
            &harnes…212152 tokens truncated…ed): Int = if flag then value.original() else value.advance() }\n";
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

#[test]
fn cross_class_reader_call_compiles_and_runs_with_a_planned_counter() {
    if !require_jdk_or_skip() {
        return;
    }
    let reversed = CROSS_CLASS_TASTY_SOURCE
        .split("class Reader:")
        .collect::<Vec<_>>();
    assert_eq!(reversed.len(), 2);
    let counter_start = reversed[0]
        .find("class Counter(")
        .expect("Counter declaration starts in fixture");
    let package = &reversed[0][..counter_start];
    let counter = &reversed[0][counter_start..];
    let reader_first = format!("{package}class Reader:{}\n\n{counter}", reversed[1]);
    compile_and_run_cross_class_reader(CROSS_CLASS_TASTY_SOURCE, "source-order", 1672);
    compile_and_run_cross_class_reader(&reader_first, "reversed-order", 1673);
    let same_named_decoy =
        format!("{CROSS_CLASS_TASTY_SOURCE}\nclass Decoy:\n  def get(): Int = 0\n");
    compile_and_run_cross_class_reader(&same_named_decoy, "same-named-decoy", 1676);
}

fn compile_and_run_cross_class_reader(source: &str, label: &str, source_id: u32) {
    let output = TestOutputDirectory::new(label);
    let artifacts = compile_source_batch(
        source,
        "Reader.scala",
        output.path(),
        SourceId::from_index(source_id),
        SourceUnitCardinality::AtLeastTwo,
    )
    .unwrap_or_else(|failure| panic!("cross-class source batch compiles ({label}): {failure:?}"));
    assert!(artifacts.len() >= 2);
    assert!(output.path().join("example/Counter.class").is_file());
    assert!(output.path().join("example/Reader.class").is_file());

    let harness = output.path().join("CrossClassHarness.java");
    fs::write(
        &harness,
        r#"public final class CrossClassHarness {
    public static void main(String[] args) {
        example.Counter counter = new example.Counter(41);
        example.Reader reader = new example.Reader();
        if (reader.read(counter) != 41) {
            throw new AssertionError("Reader.read must return Counter.get");
        }
    }
}
"#,
    )
    .expect("write Java runtime harness");
    let javac = Command::new("javac")
        .arg("-classpath")
        .arg(output.path())
        .arg("-d")
        .arg(output.path())
        .arg(&harness)
        .output()
        .expect("run JDK javac");
    assert!(
        javac.status.success(),
        "javac rejected generated Counter/Reader classes ({label}): {}",
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .arg("-Xverify:all")
        .arg("-classpath")
        .arg(output.path())
        .arg("CrossClassHarness")
        .output()
        .expect("run JDK java verifier and harness");
    assert!(
        java.status.success(),
        "JDK rejected or failed generated cross-class call ({label}): {}",
        String::from_utf8_lossy(&java.stderr)
    );
}

#[test]
fn cross_class_call_reports_primitive_receiver_to_the_typer() {
    let source = CROSS_CLASS_TASTY_SOURCE.replace("counter: Counter", "counter: Int");
    let output = TestOutputDirectory::new("cross-class-primitive-receiver");
    let error = compile_source_batch(
        &source,
        "Reader.scala",
        output.path(),
        SourceId::from_index(1674),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect_err("an Int receiver does not have Counter.get");
    assert!(
        matches!(
            error,
            PipelineFailure::Typer(TyperError::MemberNotFound { .. })
        ),
        "{error:?}"
    );
}

#[test]
fn cross_class_private_method_call_reports_its_first_backend_failure() {
    let source = CROSS_CLASS_TASTY_SOURCE.replace("def get()", "private def get()");
    let output = TestOutputDirectory::new("cross-class-private-method");
    let error = compile_source_batch(
        &source,
        "Reader.scala",
        output.path(),
        SourceId::from_index(1675),
        SourceUnitCardinality::AtLeastTwo,
    )
    .expect_err("a cross-class call cannot invoke Counter's private get");
    assert!(
        matches!(
            error,
            PipelineFailure::Codegen(JvmCodegenError::UnsupportedJvmVisibility {
                visibility: Visibility::Private,
                ..
            })
        ),
        "{error:?}"
    );
}

#[test]
fn dual_reference_factory_wrap_records_the_genuine_source_session_and_first_proof_error() {
    use dotty::jvm_tasty_cross_class::validate_cross_class_constructor_call_proofs;
    use dotty_tasty::{
        Reader as TastyReader,
        ast::{IDENTTPT_TAG, PARAM_TAG},
        term::RawTree,
    };

    const SOURCE: &str =
        include_str!("fixtures/jvm_tasty_dual_reference_forwarding/PairFactory.scala");
    let source_id = SourceId::from_index(19601);
    let output = TestOutputDirectory::new("dual-reference-forwarding");
    let mut proof_error = None;
    let mut pickler_error = None;
    let mut stale_error = None;
    let mut swapped_error = None;
    let mut stale_typed_root_error = None;
    let mut stale_source_root_error = None;
    let mut foreign_source_error = None;
    let mut observed_jvm_bytes = None;
    let mut observed = false;
    let mut observe_typer = |ast: &AstArena<Untyped>,
                             source_root: TreeId<Untyped>,
                             index: &SourceSemanticIndex,
                             typer: &mut SourceTyper<'_>,
                             definitions: &Definitions| {
        struct Bindings<'a> {
            typed: &'a SourceTypedIndex,
            classes: Vec<SymbolId>,
            methods: Vec<SymbolId>,
        }
        impl dotty_tasty_pickler::SourceTypedBindings for Bindings<'_> {
            fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
                self.typed.get(source, tree)
            }
            fn planned_class_symbols(&self) -> &[SymbolId] {
                &self.classes
            }
            fn planned_method_symbols(&self) -> &[SymbolId] {
                &self.methods
            }
        }
        let TreeKind::PackageDef(package) = &ast.get(source_root).kind else {
            panic!("unit root is package")
        };
        let mut classes = Vec::new();
        let mut methods = Vec::new();
        let mut factory_data = None;
        for &tree in &package.stats {
            let TreeKind::TypeDef(definition) = &ast.get(tree).kind else {
                continue;
            };
            let class = index.symbol_at(source_id, tree).unwrap();
            classes.push(class);
            let TreeKind::Template(template) = &ast.get(definition.rhs).kind else {
                continue;
            };
            for &member_tree in &template.body {
                if let Some(symbol) = index.symbol_at(source_id, member_tree) {
                    methods.push(symbol);
                }
                if let TreeKind::DefDef(method_def) = &ast.get(member_tree).kind
                    && typer
                        .store()
                        .names
                        .resolve(method_def.name.as_name().text())
                        == "wrap"
                {
                    factory_data = Some((class, tree, template.constructor, member_tree));
                }
            }
        }
        let (factory, class_tree, constructor_tree, method_tree) = factory_data.unwrap();
        let constructor = index.symbol_at(source_id, constructor_tree).unwrap();
        typer.complete_symbol(constructor).unwrap();
        let TreeKind::DefDef(constructor_def) = &ast.get(constructor_tree).kind else {
            panic!("default constructor is a source definition")
        };
        let constructor_params = constructor_def
            .value_param_clauses
            .iter()
            .map(|clause| {
                clause
                    .iter()
                    .map(|tree| index.symbol_at(source_id, *tree).unwrap())
                    .collect()
            })
            .collect();
        let method = index.symbol_at(source_id, method_tree).unwrap();
        let counter = classes[0];
        let pair = classes[1];
        let members = [dotty_tasty_pickler::ClassMemberPicklePlan::Method {
            symbol: method,
            source_tree: method_tree,
        }];
        let bindings = Bindings {
            typed: typer.source_typed_index(),
            classes,
            methods,
        };
        let state = dotty_tasty_pickler::ClassStatePicklePlan {
            store: typer.store(),
            definitions,
            source_ast: ast,
            typed_ast: typer.typed_ast(),
            source_index: index,
            typed_bindings: &bindings,
            source: dotty_tasty_pickler::SourceMetadata {
                id: source_id,
                name: "PairFactory.scala".into(),
            },
            class: factory,
            class_tree,
            constructor: dotty_tasty_pickler::PrimaryConstructorInputPlan {
                symbol: constructor,
                source_tree: constructor_tree,
                parameter_clauses: constructor_params,
            },
            members: &members,
        };
        pickler_error = Some(match state.validate() {
            Ok(validated) => match validated.distinct_reference_method_signature(method) {
                Ok(signature) => {
                    let TreeKind::DefDef(method_definition) = &ast.get(method_tree).kind else {
                        panic!("wrap source tree is a method")
                    };
                    let [first_tree, second_tree] =
                        method_definition.value_param_clauses[0].as_slice()
                    else {
                        panic!("wrap has exactly two original source formals")
                    };
                    let first = index.symbol_at(source_id, *first_tree).unwrap();
                    let second = index.symbol_at(source_id, *second_tree).unwrap();
                    assert_ne!(first, second);
                    assert_eq!(
                        signature
                            .parameters()
                            .iter()
                            .map(|formal| formal.symbol())
                            .collect::<Vec<_>>(),
                        [first, second]
                    );
                    assert!(signature.parameters().iter().all(|formal| {
                        formal.class() == counter
                            && typer.store().symbols.get(formal.symbol()).owner == Some(method)
                            && formal.source_tree()
                                == if formal.symbol() == first {
                                    *first_tree
                                } else {
                                    *second_tree
                                }
                    }));
                    assert_eq!(signature.parameter_class(), counter);
                    assert_eq!(signature.result_class(), pair);
                    assert_eq!(signature.method(), method);
                    let projected = validated
                        .project_distinct_reference_method_signature(method)
                        .unwrap();
                    assert_eq!(projected.parameter_addresses.len(), 2);
                    assert_ne!(
                        projected.parameter_addresses[0],
                        projected.parameter_addresses[1]
                    );
                    let parameters = projected
                        .parameter_addresses
                        .iter()
                        .map(|address| {
                            let RawTree::LengthNode(node) = RawTree::decode(&mut TastyReader::new(
                                &projected.fragment.ast.bytes()[*address as usize..],
                            ))
                            .unwrap() else {
                                panic!("each method formal projection is a PARAM declaration")
                            };
                            assert_eq!(node.tag, PARAM_TAG);
                            let parameter = node.decode_parameter().unwrap();
                            let body = parameter.decode_body().unwrap();
                            (
                                projected.fragment.names.render(parameter.name()).unwrap(),
                                body.type_tree
                                    .name_refs()
                                    .into_iter()
                                    .map(|name| projected.fragment.names.render(name).unwrap())
                                    .collect::<Vec<_>>(),
                            )
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(
                        parameters,
                        [
                            (
                                "first".to_owned(),
                                vec!["Counter".to_owned(), "example".to_owned()]
                            ),
                            (
                                "second".to_owned(),
                                vec!["Counter".to_owned(), "example".to_owned()]
                            ),
                        ]
                    );
                    let RawTree::NatAst { tag, .. } = RawTree::decode(&mut TastyReader::new(
                        &projected.fragment.ast.bytes()[projected.result_address as usize..],
                    ))
                    .unwrap() else {
                        panic!("method result projection is IDENTtpt")
                    };
                    assert_eq!(tag, IDENTTPT_TAG);
                    let result_tree = RawTree::decode(&mut TastyReader::new(
                        &projected.fragment.ast.bytes()[projected.result_address as usize..],
                    ))
                    .unwrap();
                    assert_eq!(
                        result_tree
                            .name_refs()
                            .into_iter()
                            .map(|name| projected.fragment.names.render(name).unwrap())
                            .collect::<Vec<_>>(),
                        ["Pair", "example"]
                    );
                    "Ok(two ordered method PARAM roots and Pair result)".to_owned()
                }
                Err(error) => format!("Err({error:?})"),
            },
            Err(error) => format!("Err(class-state {error:?})"),
        });
    };
    let mut observe_source = |ast: &AstArena<Untyped>,
                              source_root: TreeId<Untyped>,
                              index: &SourceSemanticIndex,
                              source_typed_index: &SourceTypedIndex,
                              unit: &mut JvmCompilationUnit<'_>,
                              definitions: &Definitions| {
        observed = true;
        let counter = audit_class_named(unit, "Counter");
        let pair = audit_class_named(unit, "Pair");
        let factory = audit_class_named(unit, "PairFactory");
        assert_eq!(
            unit.classes
                .iter()
                .map(|plan| unit
                    .store
                    .names
                    .resolve(unit.store.symbols.get(plan.class).name.text()))
                .collect::<Vec<_>>(),
            ["Counter", "Pair", "PairFactory"]
        );
        let source_method = ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if unit.store.names.resolve(definition.name.as_name().text()) == "wrap" =>
                {
                    Some((tree, definition))
                }
                _ => None,
            })
            .expect("original source declares PairFactory.wrap");
        let method = index.symbol_at(source_id, source_method.0).unwrap();
        assert_eq!(unit.store.symbols.get(method).owner, Some(factory));
        let [first_tree, second_tree] = source_method.1.value_param_clauses[0].as_slice() else {
            panic!("wrap has exactly two ordered formals");
        };
        let first = index.symbol_at(source_id, *first_tree).unwrap();
        let second = index.symbol_at(source_id, *second_tree).unwrap();
        assert_ne!(first, second);
        for (tree, symbol, name) in [
            (*first_tree, first, "first"),
            (*second_tree, second, "second"),
        ] {
            assert_eq!(unit.store.symbols.get(symbol).owner, Some(method));
            assert_eq!(
                unit.store
                    .names
                    .resolve(unit.store.symbols.get(symbol).name.text()),
                name
            );
            assert!(
                matches!(index.definition_of(symbol), Some(SourceDefinition::Canonical { source, tree: actual }) if source == source_id && actual == tree)
            );
            assert_eq!(source_typed_index.get(source_id, tree), None);
            assert!(
                matches!(unit.store.symbols.get(symbol).info, SymbolInfo::Complete(ty) if matches!(unit.store.types.get(ty), Type::TypeRef { target: TypeRefTarget::Symbol(target), .. } if *target == counter))
            );
        }
        let SymbolInfo::Complete(method_type) = unit.store.symbols.get(method).info else {
            panic!("wrap signature is completed in the source session");
        };
        let Type::Method(signature) = unit.store.types.get(method_type) else {
            panic!("wrap has an ordinary method signature");
        };
        assert_eq!(signature.params.len(), 2);
        assert!(signature.params.iter().all(|parameter| matches!(unit.store.types.get(parameter.ty), Type::TypeRef { target: TypeRefTarget::Symbol(target), .. } if *target == counter)));
        assert_eq!(audit_type_symbol(unit, signature.result), Some(pair));
        let pair_source = index.definition_of(pair).unwrap();
        let SourceDefinition::Canonical {
            tree: pair_tree, ..
        } = pair_source
        else {
            panic!("Pair is canonical source")
        };
        let TreeKind::TypeDef(pair_def) = &ast.get(pair_tree).kind else {
            panic!("Pair is a class definition")
        };
        let TreeKind::Template(pair_template) = &ast.get(pair_def.rhs).kind else {
            panic!("Pair has a template")
        };
        let pair_constructor = index
            .symbol_at(source_id, pair_template.constructor)
            .unwrap();
        let TreeKind::DefDef(pair_constructor_def) = &ast.get(pair_template.constructor).kind
        else {
            panic!("Pair has a primary constructor")
        };
        let pair_template_formals = pair_constructor_def.value_param_clauses[0]
            .iter()
            .map(|tree| index.symbol_at(source_id, *tree).unwrap())
            .collect::<Vec<_>>();
        let pair_constructor_formals = pair_constructor_def.value_param_clauses[0]
            .iter()
            .map(|tree| {
                index
                    .derived_symbol_at(pair_constructor, source_id, *tree)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(pair_constructor_formals.len(), 2);
        assert!(
            pair_constructor_formals
                .iter()
                .all(|symbol| unit.store.symbols.get(*symbol).owner == Some(pair_constructor))
        );

        let source_rhs = source_method.1.rhs.unwrap();
        let TreeKind::Apply(source_outer) = &ast.get(source_rhs).kind else {
            panic!("body is the direct outer Pair constructor application");
        };
        assert_eq!(source_outer.args.len(), 2);
        let typed_rhs = source_typed_index.get(source_id, source_rhs).unwrap();
        let TreeKind::Apply(typed_outer) = &unit.typed_ast.get(typed_rhs).kind else {
            panic!("typed body preserves the outer Apply");
        };
        assert_eq!(typed_outer.args.len(), 2);
        let source_select = source_outer.function;
        let TreeKind::Select(selection) = &ast.get(source_select).kind else {
            panic!("Pair constructor application has original Select");
        };
        let source_new = selection.qualifier;
        assert!(matches!(ast.get(source_new).kind, TreeKind::New(_)));
        let typed_select = source_typed_index.get(source_id, source_select).unwrap();
        let constructor = term_type_target(unit, unit.typed_ast.get(typed_select).ty).unwrap();
        assert_eq!(unit.store.symbols.get(constructor).owner, Some(pair));
        let typed_new = source_typed_index.get(source_id, source_new).unwrap();
        assert!(matches!(
            unit.typed_ast.get(typed_new).kind,
            TreeKind::New(_)
        ));
        let emitted = dotty_jvm::emit_class(unit, factory, definitions).unwrap();
        let emitted_bytes = emitted.bytes().to_vec();
        let mut class_reader = Reader::new(&emitted_bytes);
        let emitted_class = ClassFile::decode(&mut class_reader).unwrap();
        assert!(class_reader.is_at_end());
        let wrap_method = emitted_class
            .methods
            .iter()
            .find(|candidate| emitted_class.constant_pool.utf8(candidate.name_index) == Ok("wrap"))
            .unwrap();
        assert_eq!(
            emitted_class
                .constant_pool
                .utf8(wrap_method.descriptor_index),
            Ok("(Lexample/Counter;Lexample/Counter;)Lexample/Pair;")
        );
        let [Attribute::Code(wrap_code)] = wrap_method.attributes.as_slice() else {
            panic!("wrap has one code attribute")
        };
        assert_eq!((wrap_code.max_stack, wrap_code.max_locals), (4, 3));
        assert_eq!(
            decode_subset_instructions(wrap_code.code)
                .into_iter()
                .map(|(_, opcode)| opcode)
                .collect::<Vec<_>>(),
            [0xbb, 0x59, 0x2b, 0x2c, 0xb7, 0xb0],
            "Rust backend loads the two original formal slots directly, without allocating Counter"
        );
        assert_invocation_sequence(
            &emitted_class,
            wrap_code.code,
            &[(
                0xb7,
                "example/Pair",
                "<init>",
                "(Lexample/Counter;Lexample/Counter;)V",
            )],
        );
        observed_jvm_bytes = Some(emitted_bytes);
        eprintln!(
            "dual-reference session receipt source={source_id:?} classes=[Counter:{counter:?},Pair:{pair:?},PairFactory:{factory:?}] wrap_tree={:?} wrap={method:?} method_type={method_type:?} formals=[({first_tree:?},{first:?},type={:?},span={:?}),({second_tree:?},{second:?},type={:?},span={:?})] Pair_ctor={pair_constructor:?} Pair_template_formals={pair_template_formals:?} Pair_ctor_formals={pair_constructor_formals:?} outer_apply={source_rhs:?}/{typed_rhs:?} select={source_select:?}/{typed_select:?} new={source_new:?}/{typed_new:?}",
            source_method.0,
            signature.params[0].ty,
            ast.get(*first_tree).position,
            signature.params[1].ty,
            ast.get(*second_tree).position,
        );
        for (ordinal, (source_argument, formal)) in source_outer
            .args
            .iter()
            .copied()
            .zip([first, second])
            .enumerate()
        {
            let TreeKind::Ident(_) = &ast.get(source_argument).kind else {
                panic!("the outer arguments are original formal Ident leaves");
            };
            let typed_argument = source_typed_index.get(source_id, source_argument).unwrap();
            assert!(matches!(
                unit.typed_ast.get(typed_argument).kind,
                TreeKind::Ident(_)
            ));
            assert_eq!(
                term_type_target(unit, unit.typed_ast.get(typed_argument).ty),
                Some(formal)
            );
            eprintln!(
                "dual-reference argument[{ordinal}]={source_argument:?}/{typed_argument:?} source_span={:?} formal={formal:?} typed_type={:?}",
                ast.get(source_argument).position,
                unit.typed_ast.get(typed_argument).ty
            );
            assert_ne!(source_outer.args[0], source_outer.args[1]);
            assert_ne!(typed_outer.args[0], typed_outer.args[1]);
            assert_ne!(source_argument.index(), source_rhs.index());
            assert_eq!(ordinal, if formal == first { 0 } else { 1 });
        }
        let result =
            validate_cross_class_constructor_call_proofs(CrossClassConstructorCallProofInput {
                source_id,
                source_root,
                store: unit.store,
                definitions,
                source_ast: ast,
                typed_ast: unit.typed_ast,
                source_index: index,
                source_typed_index,
                jvm_unit: unit,
            });
        proof_error = Some(match result {
            Ok(proofs) => {
                assert_eq!(proofs.len(), 1);
                let proof = &proofs[0];
                assert_eq!(
                    proof.argument_proof().kind(),
                    dotty::jvm_tasty_cross_class::ConstructorArgumentProofKind::DirectFormalPair
                );
                assert_eq!(proof.formal_class(), Some(counter));
                assert_eq!(proof.target_class(), pair);
                assert_eq!(proof.constructor(), pair_constructor);
                assert_eq!(
                    proof.method_descriptor(),
                    "(Lexample/Counter;Lexample/Counter;)Lexample/Pair;"
                );
                assert_eq!(
                    proof.constructor_descriptor(),
                    "(Lexample/Counter;Lexample/Counter;)V"
                );
                let ordered = proof.ordered_argument_pair().unwrap();
                assert_eq!(ordered.method_formals(), [first, second]);
                assert_eq!(ordered.source_method_formals(), [*first_tree, *second_tree]);
                assert_eq!(
                    ordered.source_constructor_formals().as_slice(),
                    pair_constructor_def.value_param_clauses[0].as_slice()
                );
                assert_eq!(
                    ordered.constructor_formals().as_slice(),
                    pair_constructor_formals.as_slice()
                );
                assert!(
                    ordered.method_formals().iter().all(|formal| unit
                        .store
                        .symbols
                        .get(*formal)
                        .owner
                        == Some(method))
                );
                assert!(
                    ordered.constructor_formals().iter().all(|formal| unit
                        .store
                        .symbols
                        .get(*formal)
                        .owner
                        == Some(pair_constructor))
                );
                for (index, formal) in [first, second].into_iter().enumerate() {
                    let argument = ordered.arguments()[index];
                    let source_argument = source_outer.args[index];
                    let typed_argument = typed_outer.args[index];
                    assert_eq!(argument.source_argument(), source_argument);
                    assert_eq!(argument.typed_argument(), typed_argument);
                    assert_eq!(argument.formal().unwrap().formal(), formal);
                    assert!(argument.nested_new().is_none());
                }
                format!("Ok({} proofs)", proofs.len())
            }
            Err(error) => format!("Err({error:?})"),
        });
        let TreeKind::New(source_new_node) = &ast.get(source_new).kind else {
            panic!("outer Pair receiver remains the original New")
        };
        let source_new_tpt = source_new_node.tpt;
        let typed_new_tpt = source_typed_index.get(source_id, source_new_tpt).unwrap();
        let outer_position_pairs = [
            (source_rhs, typed_rhs, "factory body"),
            (source_select, typed_select, "outer Pair Select"),
            (source_new, typed_new, "outer Pair New"),
            (source_new_tpt, typed_new_tpt, "outer Pair type tree"),
        ];
        let first_error_for = |source_ast: &AstArena<Untyped>,
                               typed_ast: &AstArena<Typed>,
                               proof_unit: &JvmCompilationUnit<'_>| {
            match validate_cross_class_constructor_call_proofs(
                CrossClassConstructorCallProofInput {
                    source_id,
                    source_root,
                    store: proof_unit.store,
                    definitions,
                    source_ast,
                    typed_ast,
                    source_index: index,
                    source_typed_index,
                    jvm_unit: proof_unit,
                },
            ) {
                Ok(proofs) => format!("Ok({} proofs)", proofs.len()),
                Err(error) => format!("Err({error:?})"),
            }
        };
        for (source_tree, typed_tree, description) in outer_position_pairs {
            let mut foreign_source_position_ast = copy_ast(ast);
            let original_source_position = ast.get(source_tree).position.unwrap();
            foreign_source_position_ast.get_mut(source_tree).position = Some(SourceSpan::new(
                SourceId::from_index(19699),
                original_source_position.span(),
            ));
            assert_eq!(
                first_error_for(&foreign_source_position_ast, unit.typed_ast, unit),
                format!("Err(SourceIdentity(\"{description}\"))"),
                "foreign source position on {description} must reject before producing a receipt"
            );

            let mut mismatched_typed_position_ast = copy_ast(unit.typed_ast);
            mismatched_typed_position_ast.get_mut(typed_tree).position = Some(SourceSpan::new(
                source_id,
                Span::without_point(TextRange::new(0, 1).unwrap()),
            ));
            let mismatched_typed_unit = JvmCompilationUnit {
                store: unit.store,
                typed_ast: &mismatched_typed_position_ast,
                classes: unit.classes.clone(),
            };
            assert_eq!(
                first_error_for(ast, &mismatched_typed_position_ast, &mismatched_typed_unit),
                "Err(TypedBinding(\"source and typed trees do not retain the exact same source position and owner\"))",
                "mismatched typed position on {description} must reject before producing a receipt"
            );
        }
        let mut stale_source_typed_index = source_typed_index.clone();
        let second_argument = source_outer.args[1];
        let first_typed_argument = source_typed_index
            .get(source_id, source_outer.args[0])
            .unwrap();
        stale_source_typed_index
            .remove(source_id, second_argument)
            .unwrap();
        stale_source_typed_index
            .insert(source_id, second_argument, first_typed_argument)
            .unwrap();
        assert_ne!(
            first_typed_argument,
            source_typed_index.get(source_id, second_argument).unwrap()
        );
        let stale_result =
            validate_cross_class_constructor_call_proofs(CrossClassConstructorCallProofInput {
                source_id,
                source_root,
                store: unit.store,
                definitions,
                source_ast: ast,
                typed_ast: unit.typed_ast,
                source_index: index,
                source_typed_index: &stale_source_typed_index,
                jvm_unit: unit,
            });
        let stale_result = match stale_result {
            Ok(proofs) => format!("Ok({} proofs)", proofs.len()),
            Err(error) => format!("Err({error:?})"),
        };
        stale_error = Some(stale_result.clone());

        let mut swapped_source_typed_index = source_typed_index.clone();
        let first_argument = source_outer.args[0];
        let second_typed_argument = source_typed_index
            .get(source_id, source_outer.args[1])
            .unwrap();
        swapped_source_typed_index
            .remove(source_id, first_argument)
            .unwrap();
        swapped_source_typed_index
            .insert(source_id, first_argument, second_typed_argument)
            .unwrap();
        let second_argument = source_outer.args[1];
        let first_typed_argument = source_typed_index.get(source_id, first_argument).unwrap();
        swapped_source_typed_index
            .remove(source_id, second_argument)
            .unwrap();
        swapped_source_typed_index
            .insert(source_id, second_argument, first_typed_argument)
            .unwrap();
        let swapped_result =
            validate_cross_class_constructor_call_proofs(CrossClassConstructorCallProofInput {
                source_id,
                source_root,
                store: unit.store,
                definitions,
                source_ast: ast,
                typed_ast: unit.typed_ast,
                source_index: index,
                source_typed_index: &swapped_source_typed_index,
                jvm_unit: unit,
            });
        swapped_error = Some(match swapped_result {
            Ok(proofs) => format!("Ok({} proofs)", proofs.len()),
            Err(error) => format!("Err({error:?})"),
        });

        let mut extended_typed_ast: dotty_core::ast::TypedAst = AstArena::new();
        for (_, tree) in unit.typed_ast.iter() {
            extended_typed_ast.alloc(tree.clone());
        }
        let stale_typed_root =
            extended_typed_ast.alloc(unit.typed_ast.get(typed_outer.args[1]).clone());
        let mut stale_typed_root_index = source_typed_index.clone();
        stale_typed_root_index
            .remove(source_id, second_argument)
            .unwrap();
        stale_typed_root_index
            .insert(source_id, second_argument, stale_typed_root)
            .unwrap();
        let stale_typed_root_result =
            validate_cross_class_constructor_call_proofs(CrossClassConstructorCallProofInput {
                source_id,
                source_root,
                store: unit.store,
                definitions,
                source_ast: ast,
                typed_ast: unit.typed_ast,
                source_index: index,
                source_typed_index: &stale_typed_root_index,
                jvm_unit: unit,
            });
        stale_typed_root_error = Some(match stale_typed_root_result {
            Ok(proofs) => format!("Ok({} proofs)", proofs.len()),
            Err(error) => format!("Err({error:?})"),
        });

        let mut extended_source_ast: AstArena<Untyped> = AstArena::new();
        for (_, tree) in ast.iter() {
            extended_source_ast.alloc(tree.clone());
        }
        let stale_source_root = extended_source_ast.alloc(ast.get(source_root).clone());
        let stale_source_root_result =
            validate_cross_class_constructor_call_proofs(CrossClassConstructorCallProofInput {
                source_id,
                source_root: stale_source_root,
                store: unit.store,
                definitions,
                source_ast: ast,
                typed_ast: unit.typed_ast,
                source_index: index,
                source_typed_index,
                jvm_unit: unit,
            });
        stale_source_root_error = Some(match stale_source_root_result {
            Ok(proofs) => format!("Ok({} proofs)", proofs.len()),
            Err(error) => format!("Err({error:?})"),
        });

        let foreign_source_result =
            validate_cross_class_constructor_call_proofs(CrossClassConstructorCallProofInput {
                source_id: SourceId::from_index(19699),
                source_root,
                store: unit.store,
                definitions,
                source_ast: ast,
                typed_ast: unit.typed_ast,
                source_index: index,
                source_typed_index,
                jvm_unit: unit,
            });
        foreign_source_error = Some(match foreign_source_result {
            Ok(proofs) => format!("Ok({} proofs)", proofs.len()),
            Err(error) => format!("Err({error:?})"),
        });
        eprintln!(
            "dual-reference deliberately stale source-typed edge first result: {stale_result}"
        );
    };
    let artifacts = compile_source_batch_with_typer_and_source_observer(
        SOURCE,
        "PairFactory.scala",
        output.path(),
        source_id,
        SourceUnitCardinality::AtLeastTwo,
        &mut observe_typer,
        &mut observe_source,
    )
    .expect("JVM-only backend accepts the exact forwarding source");
    assert!(
        observed,
        "all source identities came from one genuine frontend session"
    );
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| artifact.class.internal_name())
            .collect::<Vec<_>>(),
        ["example/Counter", "example/Pair", "example/PairFactory"]
    );
    let factory_artifact = artifacts
        .iter()
        .find(|artifact| artifact.class.internal_name() == "example/PairFactory")
        .unwrap();
    assert_eq!(
        factory_artifact.class.bytes(),
        observed_jvm_bytes.as_deref().unwrap(),
        "the measured JVM output is exactly the normal published source artifact"
    );
    let mut paired_callback_ran = false;
    let paired = dotty::jvm_tasty_plan::with_paired_jvm_tasty_batch_plan(
        SOURCE,
        "PairFactory.scala",
        SourceId::from_index(19602),
        |_| paired_callback_ran = true,
    );
    let proof_error = proof_error.unwrap();
    let pickler_error = pickler_error.unwrap();
    let stale_error = stale_error.unwrap();
    let swapped_error = swapped_error.unwrap();
    let stale_typed_root_error = stale_typed_root_error.unwrap();
    let stale_source_root_error = stale_source_root_error.unwrap();
    let foreign_source_error = foreign_source_error.unwrap();
    let paired_error = match paired {
        Ok(_) => "Ok(callback reached)".to_owned(),
        Err(error) => format!("Err({error:?})"),
    };
    eprintln!("direct formal proof: {proof_error}");
    eprintln!("stale second source-typed edge: {stale_error}");
    eprintln!("swapped source-typed edges: {swapped_error}");
    eprintln!("stale typed root: {stale_typed_root_error}");
    eprintln!("stale source root: {stale_source_root_error}");
    eprintln!("foreign source id: {foreign_source_error}");
    eprintln!("paired planner first result: {paired_error}");
    assert_eq!(proof_error, "Ok(1 proofs)");
    assert_eq!(
        stale_error,
        "Err(TypedBinding(\"sibling outer arguments must retain distinct source and typed roots\"))"
    );
    assert_eq!(
        swapped_error,
        "Err(TypedBinding(\"typed outer Apply must preserve exact ordered source mappings\"))"
    );
    assert_eq!(
        stale_typed_root_error,
        "Err(TypedBinding(\"typed outer Apply must preserve exact ordered source mappings\"))"
    );
    assert_eq!(
        stale_source_root_error,
        "Err(SourceShape(\"package root is stale\"))"
    );
    assert_eq!(
        foreign_source_error,
        "Err(SourceIdentity(\"package root is not bound to the supplied source identity\"))"
    );
    assert_eq!(
        pickler_error,
        "Ok(two ordered method PARAM roots and Pair result)"
    );
    assert_eq!(paired_error, "Ok(callback reached)");
    assert!(
        paired_callback_ran,
        "validated direct-formal constructor bodies publish a complete batch"
    );
    eprintln!("dual-reference-forwarding proof result: {proof_error}");
    eprintln!("dual-reference-forwarding standalone pickler signature result: {pickler_error}");
    eprintln!("dual-reference-forwarding paired planner result: {paired_error}");
}

#[test]
fn distinct_reference_signature_rejects_a_rolled_back_second_formal_class() {
    const SOURCE: &str =
        include_str!("fixtures/jvm_tasty_dual_reference_forwarding/PairFactory.scala");
    let source_id = SourceId::from_index(19603);
    let output = TestOutputDirectory::new("stale-dual-reference-formal");
    let mut stale_target = None;
    let mut signature_error = None;

    let mut observe_typer = |ast: &AstArena<Untyped>,
                             source_root: TreeId<Untyped>,
                             index: &SourceSemanticIndex,
                             typer: &mut SourceTyper<'_>,
                             _definitions: &Definitions| {
        let TreeKind::PackageDef(package) = &ast.get(source_root).kind else {
            panic!("authentic source root is a package")
        };
        let factory = package
            .stats
            .iter()
            .find_map(|&tree| {
                let TreeKind::TypeDef(definition) = &ast.get(tree).kind else {
                    return None;
                };
                let class = index.symbol_at(source_id, tree)?;
                (typer
                    .store()
                    .names
                    .resolve(typer.store().symbols.get(class).name.text())
                    == "PairFactory")
                    .then_some((tree, definition.rhs))
            })
            .unwrap();
        let TreeKind::Template(template) = &ast.get(factory.1).kind else {
            panic!("PairFactory has a class template")
        };
        let constructor = index.symbol_at(source_id, template.constructor).unwrap();
        typer
            .complete_symbol(constructor)
            .expect("factory's genuine default constructor completes");
    };

    let mut mutate_store = |ast: &AstArena<Untyped>,
                            source: SourceId,
                            index: &SourceSemanticIndex,
                            store: &mut SemanticStore| {
        let package_tree = ast
            .iter()
            .find_map(|(tree, node)| matches!(&node.kind, TreeKind::PackageDef(_)).then_some(tree))
            .unwrap();
        let TreeKind::PackageDef(package) = &ast.get(package_tree).kind else {
            panic!("authentic source has a package root")
        };
        let mut counter = None;
        let mut second_formal = None;
        let mut method = None;
        for &class_tree in &package.stats {
            let TreeKind::TypeDef(class_definition) = &ast.get(class_tree).kind else {
                continue;
            };
            let class = index.symbol_at(source, class_tree).unwrap();
            if store.names.resolve(store.symbols.get(class).name.text()) == "Counter" {
                counter = Some(class);
            }
            let TreeKind::Template(template) = &ast.get(class_definition.rhs).kind else {
                continue;
            };
            for &member_tree in &template.body {
                let TreeKind::DefDef(definition) = &ast.get(member_tree).kind else {
                    continue;
                };
                if store.names.resolve(definition.name.as_name().text()) == "wrap" {
                    method = Some(index.symbol_at(source, member_tree).unwrap());
                    let [first, second] = definition.value_param_clauses[0].as_slice() else {
                        panic!("factory keeps exactly two source formals")
                    };
                    let _ = first;
                    second_formal = Some(index.symbol_at(source, *second).unwrap());
                }
            }
        }
        let counter = counter.unwrap();
        let second_formal = second_formal.unwrap();
        let method = method.unwrap();
        let SymbolInfo::Complete(second_type) = store.symbols.get(second_formal).info else {
            panic!("typed source formal has a complete type")
        };
        let prefix = match store.types.get(second_type) {
            Type::TypeRef { prefix, .. } => *prefix,
            other => panic!("formal type is a class TypeRef, got {other:?}"),
        };
        let stale_formal_type = store.types.alloc(Type::type_ref(prefix, counter));
        let SymbolInfo::Complete(signature_type) = store.symbols.get(method).info else {
            panic!("typed source method has a complete signature")
        };
        let Type::Method(mut signature) = store.types.get(signature_type).clone() else {
            panic!("wrap has a method signature")
        };
        assert_eq!(signature.params.len(), 2);
        signature.params[1].ty = stale_formal_type;
        let stale_signature_type = store.types.alloc(Type::Method(signature));

        let checkpoint = store.checkpoint();
        let stale_record = store.symbols.get(counter).clone();
        let stale = store.symbols.alloc(stale_record);
        store.rollback_to(checkpoint);
        *store.types.get_mut(stale_formal_type) = Type::type_ref(prefix, stale);
        store.symbols.get_mut(second_formal).info = SymbolInfo::Complete(stale_formal_type);
        store.symbols.get_mut(method).info = SymbolInfo::Complete(stale_signature_type);
        assert!(!store.symbols.contains(stale));
        stale_target = Some(stale);
    };

    struct Bindings<'a> {
        typed: &'a SourceTypedIndex,
        classes: Vec<SymbolId>,
    }
    impl dotty_tasty_pickler::SourceTypedBindings for Bindings<'_> {
        fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
            self.typed.get(source, tree)
        }
        fn planned_class_symbols(&self) -> &[SymbolId] {
            &self.classes
        }
    }

    let mut observe_source = |ast: &AstArena<Untyped>,
                              source_root: TreeId<Untyped>,
                              index: &SourceSemanticIndex,
                              typed_index: &SourceTypedIndex,
                              unit: &mut JvmCompilationUnit<'_>,
                              definitions: &Definitions| {
        let TreeKind::PackageDef(package) = &ast.get(source_root).kind else {
            panic!("authentic source root is a package")
        };
        let factory = package
            .stats
            .iter()
            .find_map(|&tree| {
                let TreeKind::TypeDef(definition) = &ast.get(tree).kind else {
                    return None;
                };
                let symbol = index.symbol_at(source_id, tree)?;
                (unit
                    .store
                    .names
                    .resolve(unit.store.symbols.get(symbol).name.text())
                    == "PairFactory")
                    .then_some((symbol, tree, definition.rhs))
            })
            .unwrap();
        let factory_symbol = factory.0;
        let factory_tree = factory.1;
        let TreeKind::Template(template) = &ast.get(factory.2).kind else {
            panic!("PairFactory has a class template")
        };
        let method_tree = template
            .body
            .iter()
            .copied()
            .find(|&tree| {
                matches!(&ast.get(tree).kind, TreeKind::DefDef(definition)
                    if unit.store.names.resolve(definition.name.as_name().text()) == "wrap")
            })
            .unwrap();
        let method = index.symbol_at(source_id, method_tree).unwrap();
        let constructor = index.symbol_at(source_id, template.constructor).unwrap();
        let members = [dotty_tasty_pickler::ClassMemberPicklePlan::Method {
            symbol: method,
            source_tree: method_tree,
        }];
        let bindings = Bindings {
            typed: typed_index,
            classes: unit.classes.iter().map(|class| class.class).collect(),
        };
        let state = dotty_tasty_pickler::ClassStatePicklePlan {
            store: unit.store,
            definitions,
            source_ast: ast,
            typed_ast: unit.typed_ast,
            source_index: index,
            typed_bindings: &bindings,
            source: dotty_tasty_pickler::SourceMetadata {
                id: source_id,
                name: "PairFactory.scala".into(),
            },
            class: factory_symbol,
            class_tree: factory_tree,
            constructor: dotty_tasty_pickler::PrimaryConstructorInputPlan {
                symbol: constructor,
                source_tree: template.constructor,
                parameter_clauses: vec![],
            },
            members: &members,
        };
        let validated = state
            .validate()
            .expect("authentic class state remains valid before signature validation");
        let error = validated
            .distinct_reference_method_signature(method)
            .expect_err("a rolled-back formal target is a typed validation error");
        signature_error = Some(error);
    };

    let _compilation = compile_source_batch_with_typer_failure_observer(
        SOURCE,
        "PairFactory.scala",
        output.path(),
        source_id,
        SourceUnitCardinality::AtLeastTwo,
        SourceBatchObservers {
            source: Some(&mut observe_source),
            source_store_mutator: Some(&mut mutate_store),
            typer: Some(&mut observe_typer),
            ..SourceBatchObservers::default()
        },
    );
    let stale = stale_target.expect("mutator captured the rolled-back symbol ID");
    assert!(matches!(
        signature_error,
        Some(dotty_tasty_pickler::PickleError::InvalidSymbol { symbol }) if symbol == stale
    ));
}

fn assert_direct_formal_pair_variant(
    source: &str,
    source_number: u32,
    method_formal_names: [&str; 2],
    argument_names: [&str; 2],
) {
    use dotty::jvm_tasty_cross_class::{
        ConstructorArgumentProofKind, CrossClassConstructorCallProofInput,
        validate_cross_class_constructor_call_proofs,
    };

    let source_id = SourceId::from_index(source_number);
    let output = TestOutputDirectory::new(&format!("direct-formal-pair-{source_number}"));
    let mut observed = false;
    let mut observe_typer = |_: &AstArena<Untyped>,
                             _: TreeId<Untyped>,
                             _: &SourceSemanticIndex,
                             _: &mut SourceTyper<'_>,
                             _: &Definitions| {};
    let mut observe_source = |ast: &AstArena<Untyped>,
                              source_root: TreeId<Untyped>,
                              source_index: &SourceSemanticIndex,
                              source_typed_index: &SourceTypedIndex,
                              unit: &mut JvmCompilationUnit<'_>,
                              definitions: &Definitions| {
        observed = true;
        let proofs =
            validate_cross_class_constructor_call_proofs(CrossClassConstructorCallProofInput {
                source_id,
                source_root,
                store: unit.store,
                definitions,
                source_ast: ast,
                typed_ast: unit.typed_ast,
                source_index,
                source_typed_index,
                jvm_unit: unit,
            })
            .expect("this source shape has one fully authenticated direct-formal proof");
        assert_eq!(proofs.len(), 1);
        let proof = &proofs[0];
        assert_eq!(
            proof.argument_proof().kind(),
            ConstructorArgumentProofKind::DirectFormalPair
        );
        let ordered = proof.ordered_argument_pair().unwrap();
        assert_ne!(ordered.method_formals()[0], ordered.method_formals()[1]);
        assert_ne!(
            ordered.constructor_formals()[0],
            ordered.constructor_formals()[1]
        );
        assert!(ordered.method_formals().iter().all(|formal| {
            unit.store.symbols.get(*formal).owner == Some(proof.method())
                && unit.store.symbols.get(*formal).kind == SymbolKind::Parameter
        }));
        assert!(ordered.constructor_formals().iter().all(|formal| {
            unit.store.symbols.get(*formal).owner == Some(proof.constructor())
                && unit.store.symbols.get(*formal).kind == SymbolKind::Parameter
        }));
        assert!(
            ordered
                .method_formals()
                .iter()
                .all(|formal| { !ordered.constructor_formals().contains(formal) })
        );
        for (index, formal_name) in method_formal_names.into_iter().enumerate() {
            let formal = ordered.method_formals()[index];
            assert_eq!(
                unit.store
                    .names
                    .resolve(unit.store.symbols.get(formal).name.text()),
                formal_name
            );
            assert!(matches!(
                ordered.source_method_formals().get(index).and_then(|tree| ast.try_get(*tree)),
                Some(node) if matches!(&node.kind, TreeKind::ValDef(parameter) if unit.store.names.resolve(parameter.name.as_name().text()) == formal_name)
            ));
        }
        for (index, argument_name) in argument_names.into_iter().enumerate() {
            let argument = ordered.arguments()[index];
            let source_argument = argument.source_argument();
            let typed_argument = argument.typed_argument();
            assert_eq!(
                source_typed_index.get(source_id, source_argument),
                Some(typed_argument)
            );
            let TreeKind::Ident(source_ident) = &ast.get(source_argument).kind else {
                panic!("direct pair source argument is an original Ident")
            };
            assert_eq!(
                unit.store.names.resolve(source_ident.name.text()),
                argument_name
            );
            let formal = argument.formal().unwrap().formal();
            assert_eq!(
                term_type_target(unit, unit.typed_ast.get(typed_argument).ty),
                Some(formal)
            );
            assert_eq!(
                unit.store
                    .names
                    .resolve(unit.store.symbols.get(formal).name.text()),
                argument_name
            );
        }
    };
    compile_source_batch_with_typer_and_source_observer(
        source,
        "PairFactory.scala",
        output.path(),
        source_id,
        SourceUnitCardinality::AtLeastTwo,
        &mut observe_typer,
        &mut observe_source,
    )
    .expect("JVM-only source compilation remains supported for this proof shape");
    assert!(
        observed,
        "the original one-session source reached the proof scan"
    );
}

#[test]
fn direct_formal_pair_rejects_disconnected_method_scope_provenance() {
    const SOURCE: &str =
        include_str!("fixtures/jvm_tasty_dual_reference_forwarding/PairFactory.scala");

    fn assert_scope_corruption(
        source_id: SourceId,
        remove_second_formal: bool,
        expected_error: &'static str,
    ) {
        let output = TestOutputDirectory::new(&format!("direct-formal-scope-{source_id:?}"));
        let mut proof_result = None;
        let mut mutate_scope = move |ast: &AstArena<Untyped>,
                                     source: SourceId,
                                     index: &SourceSemanticIndex,
                                     store: &mut SemanticStore| {
            let (method_tree, method_definition) = ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::DefDef(definition)
                        if store.names.resolve(definition.name.as_name().text()) == "wrap" =>
                    {
                        Some((tree, definition))
                    }
                    _ => None,
                })
                .expect("fixture declares wrap");
            let method = index.symbol_at(source, method_tree).unwrap();
            let method_scope = index.scope_of(method).unwrap();
            if remove_second_formal {
                let second_tree = method_definition.value_param_clauses[0][1];
                let second = index.symbol_at(source, second_tree).unwrap();
                store.scopes.get_mut(method_scope).remove(second);
            } else {
                let class_owner = store.symbols.get(method).owner.unwrap();
                store.scopes.get_mut(method_scope).owner = Some(class_owner);
            }
        };
        let mut observe_proof = |ast: &AstArena<Untyped>,
                                 source_root: TreeId<Untyped>,
                                 index: &SourceSemanticIndex,
                                 source_typed_index: &SourceTypedIndex,
                                 unit: &mut JvmCompilationUnit<'_>,
                                 definitions: &Definitions| {
            let result =
                validate_cross_class_constructor_call_proofs(CrossClassConstructorCallProofInput {
                    source_id,
                    source_root,
                    store: unit.store,
                    definitions,
                    source_ast: ast,
                    typed_ast: unit.typed_ast,
                    source_index: index,
                    source_typed_index,
                    jvm_unit: unit,
                });
            proof_result = Some(match result {
                Ok(proofs) => format!("unexpected receipt count: {}", proofs.len()),
                Err(error) => format!("{error:?}"),
            });
        };
        compile_source_batch_with_store_mutator_and_source_observer(
            SOURCE,
            "PairFactory.scala",
            output.path(),
            source_id,
            SourceUnitCardinality::AtLeastTwo,
            &mut mutate_scope,
            &mut observe_proof,
        )
        .expect("a test-only scope mutation leaves the already typed JVM output valid");
        assert_eq!(proof_result.as_deref(), Some(expected_error));
    }

    assert_scope_corruption(
        SourceId::from_index(19616),
        false,
        "SourceIdentity(\"factory method source scope has a noncanonical owner\")",
    );
    assert_scope_corruption(
        SourceId::from_index(19617),
        true,
        "SourceIdentity(\"factory method scope must contain the exact canonical formal entries\")",
    );
}

#[test]
fn direct_formal_pair_proof_preserves_names_order_reversal_and_aliasing() {
    const SOURCE: &str =
        include_str!("fixtures/jvm_tasty_dual_reference_forwarding/PairFactory.scala");
    let sections = SOURCE.split("\n\n").collect::<Vec<_>>();
    assert_eq!(sections.len(), 4);
    let reordered = format!(
        "{}\n\n{}\n\n{}\n\n{}",
        sections[0], sections[2], sections[3], sections[1]
    );
    let renamed = SOURCE
        .replace("package example", "package renamed")
        .replace("PairFactory", "AssemblerFactory")
        .replace("Pair", "Envelope")
        .replace("Counter", "Meter")
        .replace("wrap", "forward")
        .replace("first", "sourceLeft")
        .replace("second", "sourceRight");
    let reversed_arguments = SOURCE.replace("new Pair(first, second)", "new Pair(second, first)");
    let reversed_formals = SOURCE.replace(
        "def wrap(first: Counter, second: Counter)",
        "def wrap(second: Counter, first: Counter)",
    );
    let repeated_formal = SOURCE.replace("new Pair(first, second)", "new Pair(first, first)");

    assert_direct_formal_pair_variant(SOURCE, 19610, ["first", "second"], ["first", "second"]);
    assert_direct_formal_pair_variant(
        &renamed,
        19611,
        ["sourceLeft", "sourceRight"],
        ["sourceLeft", "sourceRight"],
    );
    assert_direct_formal_pair_variant(&reordered, 19612, ["first", "second"], ["first", "second"]);
    assert_direct_formal_pair_variant(
        &reversed_arguments,
        19613,
        ["first", "second"],
        ["second", "first"],
    );
    assert_direct_formal_pair_variant(
        &reversed_formals,
        19614,
        ["second", "first"],
        ["first", "second"],
    );
    assert_direct_formal_pair_variant(
        &repeated_formal,
        19615,
        ["first", "second"],
        ["first", "first"],
    );
}

#[test]
fn mixed_origin_pair_arguments_reach_jvm_and_report_the_proof_boundary() {
    for (variant, source, method_name, source_id) in [
        (
            "wrap-and-create",
            MIXED_ORIGIN_WRAP_AND_CREATE_SOURCE,
            "wrapAndCreate",
            SourceId::from_index(20110),
        ),
        (
            "create-and-wrap",
            MIXED_ORIGIN_CREATE_AND_WRAP_SOURCE,
            "createAndWrap",
            SourceId::from_index(20111),
        ),
    ] {
        let output = TestOutputDirectory::new("mixed-origin-jvm-only");
        let mut observed = false;
        let mut observed_proof_error = None;
        let mut observe_source = |ast: &AstArena<Untyped>,
                                  source_root: TreeId<Untyped>,
                                  index: &SourceSemanticIndex,
                                  source_typed_index: &SourceTypedIndex,
                                  unit: &mut JvmCompilationUnit<'_>,
                                  definitions: &Definitions| {
            observed = true;
            let counter = audit_class_named(unit, "Counter");
            let pair = audit_class_named(unit, "Pair");
            let factory = audit_class_named(unit, "PairFactory");
            let (method_tree, method_definition) = ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::DefDef(definition)
                        if unit.store.names.resolve(definition.name.as_name().text())
                            == method_name =>
                    {
                        Some((tree, definition))
                    }
                    _ => None,
                })
                .expect("the original source contains the requested factory method");
            let method = index.symbol_at(source_id, method_tree).unwrap();
            assert_eq!(unit.store.symbols.get(method).owner, Some(factory));
            assert!(matches!(
                index.definition_of(method),
                Some(SourceDefinition::Canonical { source, tree })
                    if source == source_id && tree == method_tree
            ));
            let [existing_tree, seed_tree] = method_definition.value_param_clauses[0].as_slice()
            else {
                panic!("factory has exactly two ordered formals")
            };
            let existing = index.symbol_at(source_id, *existing_tree).unwrap();
            let seed = index.symbol_at(source_id, *seed_tree).unwrap();
            assert_ne!(existing, seed);
            assert_eq!(unit.store.symbols.get(existing).owner, Some(method));
            assert_eq!(unit.store.symbols.get(seed).owner, Some(method));
            assert_eq!(source_typed_index.get(source_id, *existing_tree), None);
            assert_eq!(source_typed_index.get(source_id, *seed_tree), None);
            let SymbolInfo::Complete(existing_type) = unit.store.symbols.get(existing).info else {
                panic!("Counter formal type is completed")
            };
            assert!(matches!(
                unit.store.types.get(existing_type),
                Type::TypeRef { target: TypeRefTarget::Symbol(target), .. }
                    if *target == counter
            ));
            assert!(matches!(
                unit.store.symbols.get(seed).info,
                SymbolInfo::Complete(ty) if ty == definitions.int
            ));
            let SymbolInfo::Complete(method_type) = unit.store.symbols.get(method).info else {
                panic!("method signature is completed in the source session")
            };
            let Type::Method(signature) = unit.store.types.get(method_type) else {
                panic!("factory method has an ordinary signature")
            };
            assert_eq!(signature.params.len(), 2);
            assert_eq!(signature.params[0].ty, existing_type);
            assert_eq!(signature.params[1].ty, definitions.int);
            assert!(matches!(
                unit.store.types.get(signature.result),
                Type::TypeRef { target: TypeRefTarget::Symbol(target), .. }
                    if *target == pair
            ));

            let source_rhs = method_definition.rhs.unwrap();
            let TreeKind::Apply(source_outer) = &ast.get(source_rhs).kind else {
                panic!("method body is the direct outer Pair application")
            };
            assert_eq!(source_outer.args.len(), 2);
            let typed_rhs = source_typed_index.get(source_id, source_rhs).unwrap();
            let TreeKind::Apply(typed_outer) = &unit.typed_ast.get(typed_rhs).kind else {
                panic!("typed method body preserves the outer Apply")
            };
            assert_eq!(typed_outer.args.len(), 2);
            let (formal_index, nested_index) = if method_name == "wrapAndCreate" {
                (0, 1)
            } else {
                (1, 0)
            };
            let formal_tree = source_outer.args[formal_index];
            assert!(matches!(ast.get(formal_tree).kind, TreeKind::Ident(_)));
            let typed_formal_tree = source_typed_index.get(source_id, formal_tree).unwrap();
            assert!(matches!(
                unit.typed_ast.get(typed_formal_tree).kind,
                TreeKind::Ident(_)
            ));
            assert!(matches!(
                unit.store.types.get(unit.typed_ast.get(typed_formal_tree).ty),
                Type::TermRef { target: TermRefTarget::Symbol(target), .. }
                    if *target == existing
            ));

            let nested_tree = source_outer.args[nested_index];
            let TreeKind::Apply(nested_apply) = &ast.get(nested_tree).kind else {
                panic!("the other argument is one nested Counter constructor application")
            };
            assert_eq!(nested_apply.args.len(), 1);
            let typed_nested_tree = source_typed_index.get(source_id, nested_tree).unwrap();
            let TreeKind::Apply(typed_nested_apply) = &unit.typed_ast.get(typed_nested_tree).kind
            else {
                panic!("the typed nested constructor application remains an Apply")
            };
            assert_eq!(typed_nested_apply.args.len(), 1);
            let seed_argument = nested_apply.args[0];
            assert_eq!(index.symbol_at(source_id, seed_argument), None);
            let typed_seed_argument = source_typed_index.get(source_id, seed_argument).unwrap();
            assert!(matches!(
                unit.store.types.get(unit.typed_ast.get(typed_seed_argument).ty),
                Type::TermRef { target: TermRefTarget::Symbol(target), .. }
                    if *target == seed
            ));
            let TreeKind::Select(nested_selection) = &ast.get(nested_apply.function).kind else {
                panic!("nested constructor function is a source Select")
            };
            assert!(matches!(
                ast.get(nested_selection.qualifier).kind,
                TreeKind::New(_)
            ));
            let typed_nested_selection = source_typed_index
                .get(source_id, nested_apply.function)
                .unwrap();
            let nested_constructor =
                term_type_target(unit, unit.typed_ast.get(typed_nested_selection).ty)
                    .expect("typed nested Select resolves the actual Counter constructor");
            assert_eq!(
                unit.store.symbols.get(nested_constructor).owner,
                Some(counter)
            );
            assert_eq!(
                unit.store
                    .names
                    .resolve(unit.store.symbols.get(nested_constructor).name.text()),
                "<init>"
            );
            let TreeKind::Select(outer_selection) = &ast.get(source_outer.function).kind else {
                panic!("Pair constructor function is a source Select")
            };
            assert!(matches!(
                ast.get(outer_selection.qualifier).kind,
                TreeKind::New(_)
            ));
            let typed_outer_selection = source_typed_index
                .get(source_id, source_outer.function)
                .unwrap();
            let outer_constructor =
                term_type_target(unit, unit.typed_ast.get(typed_outer_selection).ty)
                    .expect("typed Pair Select resolves the actual Pair constructor");
            assert_eq!(unit.store.symbols.get(outer_constructor).owner, Some(pair));
            let typed_outer_new = source_typed_index
                .get(source_id, outer_selection.qualifier)
                .unwrap();
            assert!(matches!(
                unit.typed_ast.get(typed_outer_new).kind,
                TreeKind::New(_)
            ));
            let SourceDefinition::Canonical {
                source: pair_source,
                tree: pair_tree,
            } = index.definition_of(pair).unwrap()
            else {
                panic!("Pair has the canonical source definition")
            };
            assert_eq!(pair_source, source_id);
            let TreeKind::TypeDef(pair_definition) = &ast.get(pair_tree).kind else {
                panic!("Pair is a source class")
            };
            let TreeKind::Template(pair_template) = &ast.get(pair_definition.rhs).kind else {
                panic!("Pair class has a source template")
            };
            let pair_constructor = index
                .symbol_at(source_id, pair_template.constructor)
                .unwrap();
            let SourceDefinition::Canonical {
                source: counter_source,
                tree: counter_tree,
            } = index.definition_of(counter).unwrap()
            else {
                panic!("Counter has the canonical source definition")
            };
            assert_eq!(counter_source, source_id);
            let TreeKind::TypeDef(counter_definition) = &ast.get(counter_tree).kind else {
                panic!("Counter is a source class")
            };
            let TreeKind::Template(counter_template) = &ast.get(counter_definition.rhs).kind else {
                panic!("Counter class has a source template")
            };
            let counter_constructor = index
                .symbol_at(source_id, counter_template.constructor)
                .unwrap();
            assert_eq!(outer_constructor, pair_constructor);
            assert_eq!(nested_constructor, counter_constructor);
            assert_eq!(
                source_typed_index.get(source_id, source_outer.args[0]),
                Some(typed_outer.args[0])
            );
            assert_eq!(
                source_typed_index.get(source_id, source_outer.args[1]),
                Some(typed_outer.args[1])
            );
            eprintln!(
                "mixed-origin {variant}: source={source_id:?} classes=[Counter:{counter:?},Pair:{pair:?},PairFactory:{factory:?}] method_tree={method_tree:?} method={method:?} formals=[({existing_tree:?},{existing:?}:{existing_type:?}),({seed_tree:?},{seed:?}:Int)] constructors=[Counter:{counter_constructor:?},Pair:{pair_constructor:?}] outer={source_rhs:?}/{typed_rhs:?} pair_select={:?}/{typed_outer_selection:?} pair_new={:?}/{typed_outer_new:?} args={:?}/{:?} nested_apply={nested_tree:?}/{typed_nested_tree:?} nested_select={:?}/{typed_nested_selection:?} nested_new={:?}/{:?} seed={seed_argument:?}/{typed_seed_argument:?} spans=[{:?},{:?}]",
                source_outer.function,
                outer_selection.qualifier,
                source_outer.args,
                typed_outer.args,
                nested_apply.function,
                nested_selection.qualifier,
                source_typed_index
                    .get(source_id, nested_selection.qualifier)
                    .unwrap(),
                ast.get(source_outer.args[0]).position,
                ast.get(source_outer.args[1]).position,
            );

            let proofs =
                validate_cross_class_constructor_call_proofs(CrossClassConstructorCallProofInput {
                    source_id,
                    source_root,
                    store: unit.store,
                    definitions,
                    source_ast: ast,
                    typed_ast: unit.typed_ast,
                    source_index: index,
                    source_typed_index,
                    jvm_unit: unit,
                })
                .expect("both genuine source argument orders have one canonical mixed proof");
            assert_eq!(proofs.len(), 1);
            let proof = &proofs[0];
            assert_eq!(
                proof.argument_proof().kind(),
                dotty::jvm_tasty_cross_class::ConstructorArgumentProofKind::FormalNestedPair
            );
            assert_eq!(proof.formal_class(), Some(counter));
            assert_eq!(proof.target_class(), pair);
            assert_eq!(proof.constructor(), pair_constructor);
            assert_eq!(
                proof.method_descriptor(),
                "(Lexample/Counter;I)Lexample/Pair;"
            );
            assert_eq!(
                proof.constructor_descriptor(),
                "(Lexample/Counter;Lexample/Counter;)V"
            );
            let ordered = proof.ordered_argument_pair().unwrap();
            assert_eq!(ordered.method_formals(), [existing, seed]);
            assert_eq!(
                ordered.source_method_formals(),
                [*existing_tree, *seed_tree]
            );
            assert_eq!(
                ordered.arguments()[formal_index].source_argument(),
                formal_tree
            );
            assert_eq!(
                ordered.arguments()[formal_index].typed_argument(),
                typed_formal_tree
            );
            assert_eq!(
                ordered.arguments()[formal_index].formal().unwrap().formal(),
                existing
            );
            assert!(ordered.arguments()[formal_index].nested_new().is_none());
            let nested_proof = ordered.arguments()[nested_index]
                .nested_new()
                .expect("the opposite Pair slot retains its original nested call receipt");
            assert_eq!(nested_proof.source_outer_argument(), nested_tree);
            assert_eq!(nested_proof.typed_outer_argument(), typed_nested_tree);
            assert_eq!(nested_proof.source_apply(), nested_tree);
            assert_eq!(nested_proof.typed_apply(), typed_nested_tree);
            assert_eq!(nested_proof.source_select(), nested_apply.function);
            assert_eq!(nested_proof.source_new(), nested_selection.qualifier);
            assert_eq!(nested_proof.source_argument(), seed_argument);
            assert_eq!(nested_proof.typed_argument(), typed_seed_argument);
            assert_eq!(nested_proof.class(), counter);
            assert_eq!(nested_proof.constructor(), counter_constructor);
            assert_eq!(nested_proof.argument_formal(), seed);
            assert_eq!(
                nested_proof.result_type(),
                unit.typed_ast.get(typed_nested_tree).ty
            );
            observed_proof_error = Some("Ok(1 FormalNestedPair proof)".to_owned());

            let mut swapped_bindings = source_typed_index.clone();
            let first_source_argument = source_outer.args[0];
            let second_source_argument = source_outer.args[1];
            let first_typed_argument = typed_outer.args[0];
            let second_typed_argument = typed_outer.args[1];
            swapped_bindings
                .remove(source_id, first_source_argument)
                .unwrap();
            swapped_bindings
                .insert(source_id, first_source_argument, second_typed_argument)
                .unwrap();
            swapped_bindings
                .remove(source_id, second_source_argument)
                .unwrap();
            swapped_bindings
                .insert(source_id, second_source_argument, first_typed_argument)
                .unwrap();
            assert!(matches!(
                validate_cross_class_constructor_call_proofs(CrossClassConstructorCallProofInput {
                    source_id,
                    source_root,
                    store: unit.store,
                    definitions,
                    source_ast: ast,
                    typed_ast: unit.typed_ast,
                    source_index: index,
                    source_typed_index: &swapped_bindings,
                    jvm_unit: unit,
                }),
                Err(
                    dotty::jvm_tasty_cross_class::CrossClassConstructorCallProofError::TypedBinding(
                        "typed outer Apply must preserve exact ordered source mappings"
                    )
                )
            ));
        };
        let artifacts = compile_source_batch_with_source_observer(
            source,
            "PairFactory.scala",
            output.path(),
            source_id,
            SourceUnitCardinality::AtLeastTwo,
            &mut observe_source,
        )
        .expect("the JVM-only source pipeline accepts both mixed-origin methods");
        assert!(
            observed,
            "proof measurements use the genuine frontend session"
        );
        assert_eq!(
            artifacts
                .iter()
                .map(|artifact| artifact.class.internal_name())
                .collect::<Vec<_>>(),
            ["example/Counter", "example/Pair", "example/PairFactory"]
        );
        let factory_artifact = artifacts
            .iter()
            .find(|artifact| artifact.class.internal_name() == "example/PairFactory")
            .unwrap();
        let emitted_factory = decode_emitted_class(factory_artifact);
        let emitted_method = emitted_factory
            .methods
            .iter()
            .find(|candidate| {
                emitted_factory.constant_pool.utf8(candidate.name_index) == Ok(method_name)
            })
            .unwrap();
        assert_eq!(
            emitted_factory
                .constant_pool
                .utf8(emitted_method.descriptor_index),
            Ok("(Lexample/Counter;I)Lexample/Pair;")
        );
        let [Attribute::Code(method_code)] = emitted_method.attributes.as_slice() else {
            panic!("factory method has a Code attribute")
        };
        assert_eq!(
            method_code.max_stack,
            if method_name == "wrapAndCreate" { 6 } else { 5 }
        );
        assert_eq!(method_code.max_locals, 3);
        let expected_opcodes = if method_name == "wrapAndCreate" {
            [0xbb, 0x59, 0x2b, 0xbb, 0x59, 0x1c, 0xb7, 0xb7, 0xb0]
        } else {
            [0xbb, 0x59, 0xbb, 0x59, 0x1c, 0xb7, 0x2b, 0xb7, 0xb0]
        };
        assert_eq!(
            decode_subset_instructions(method_code.code)
                .into_iter()
                .map(|(_, opcode)| opcode)
                .collect::<Vec<_>>(),
            expected_opcodes,
            "JVM-only output evaluates the nested new at its source argument position"
        );
        assert_invocation_sequence(
            &emitted_factory,
            method_code.code,
            &[
                (0xb7, "example/Counter", "<init>", "(I)V"),
                (
                    0xb7,
                    "example/Pair",
                    "<init>",
                    "(Lexample/Counter;Lexample/Counter;)V",
                ),
            ],
        );
        assert_eq!(
            observed_proof_error.as_deref(),
            Some("Ok(1 FormalNestedPair proof)")
        );

        let mut paired_callback_ran = false;
        let paired = dotty::jvm_tasty_plan::with_paired_jvm_tasty_batch_plan(
            source,
            "PairFactory.scala",
            source_id,
            |_| paired_callback_ran = true,
        );
        assert!(
            paired_callback_ran,
            "the validated paired batch reaches its consumer"
        );
        assert!(
            paired.is_ok(),
            "unexpected downstream paired result: {paired:?}"
        );
    }
}

#[test]
fn mixed_origin_pair_rejects_wrong_arity_and_seed_type_before_proof() {
    let cases = [
        (
            "third-pair-argument",
            MIXED_ORIGIN_WRAP_AND_CREATE_SOURCE.replace(
                "new Pair(existing, new Counter(seed))",
                "new Pair(existing, new Counter(seed), existing)",
            ),
            true,
        ),
        (
            "wrong-seed-formal-type",
            MIXED_ORIGIN_WRAP_AND_CREATE_SOURCE.replace(
                "def wrapAndCreate(existing: Counter, seed: Int)",
                "def wrapAndCreate(existing: Counter, seed: Counter)",
            ),
            false,
        ),
    ];
    for (label, source, is_arity_case) in cases {
        let output = TestOutputDirectory::new(label);
        let error = compile_source_batch(
            &source,
            "PairFactory.scala",
            output.path(),
            SourceId::from_index(if is_arity_case { 20113 } else { 20114 }),
            SourceUnitCardinality::AtLeastTwo,
        )
        .expect_err("source arity and constructor input types remain checked before proof");
        if is_arity_case {
            assert!(matches!(
                error,
                PipelineFailure::Typer(TyperError::ApplicationArityMismatch {
                    expected: 2,
                    actual: 3,
                    ..
                })
            ));
        } else {
            assert!(matches!(
                error,
                PipelineFailure::Typer(TyperError::ApplicationArgumentTypeMismatch {
                    argument_index: 0,
                    ..
                })
            ));
        }
    }
}

#[test]
fn mixed_origin_pair_rejects_disconnected_method_scope_provenance() {
    fn assert_scope_corruption(
        source_id: SourceId,
        remove_seed: bool,
        expected_error: &'static str,
    ) {
        let source = MIXED_ORIGIN_WRAP_AND_CREATE_SOURCE;
        let output = TestOutputDirectory::new(&format!("mixed-origin-scope-{source_id:?}"));
        let mut proof_error = None;
        let mut mutate_scope = |ast: &AstArena<Untyped>,
                                source: SourceId,
                                index: &SourceSemanticIndex,
                                store: &mut SemanticStore| {
            let (method_tree, definition) = ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::DefDef(definition)
                        if store.names.resolve(definition.name.as_name().text())
                            == "wrapAndCreate" =>
                    {
                        Some((tree, definition))
                    }
                    _ => None,
                })
                .expect("frozen source declares wrapAndCreate");
            let method = index.symbol_at(source, method_tree).unwrap();
            let scope = index.scope_of(method).unwrap();
            if remove_seed {
                let seed_tree = definition.value_param_clauses[0][1];
                let seed = index.symbol_at(source, seed_tree).unwrap();
                store.scopes.get_mut(scope).remove(seed);
            } else {
                store.scopes.get_mut(scope).owner = store.symbols.get(method).owner;
            }
        };
        let mut observe_proof = |ast: &AstArena<Untyped>,
                                 source_root: TreeId<Untyped>,
                                 index: &SourceSemanticIndex,
                                 bindings: &SourceTypedIndex,
                                 unit: &mut JvmCompilationUnit<'_>,
                                 definitions: &Definitions| {
            proof_error = Some(
                match validate_cross_class_constructor_call_proofs(
                    CrossClassConstructorCallProofInput {
                        source_id,
                        source_root,
                        store: unit.store,
                        definitions,
                        source_ast: ast,
                        typed_ast: unit.typed_ast,
                        source_index: index,
                        source_typed_index: bindings,
                        jvm_unit: unit,
                    },
                ) {
                    Ok(proofs) => format!("unexpected receipt count: {}", proofs.len()),
                    Err(error) => format!("{error:?}"),
                },
            );
        };
        compile_source_batch_with_store_mutator_and_source_observer(
            source,
            "PairFactory.scala",
            output.path(),
            source_id,
            SourceUnitCardinality::AtLeastTwo,
            &mut mutate_scope,
            &mut observe_proof,
        )
        .expect("the test-only scope corruption leaves the typed JVM output valid");
        assert_eq!(proof_error.as_deref(), Some(expected_error));
    }

    assert_scope_corruption(
        SourceId::from_index(20115),
        false,
        "SourceIdentity(\"factory method source scope has a noncanonical owner\")",
    );
    assert_scope_corruption(
        SourceId::from_index(20116),
        true,
        "SourceIdentity(\"factory method scope must contain the exact canonical formal entries\")",
    );
}

#[test]
fn mixed_formal_signature_does_not_accept_two_nested_origins() {
    let source = MIXED_ORIGIN_WRAP_AND_CREATE_SOURCE.replace(
        "new Pair(existing, new Counter(seed))",
        "new Pair(new Counter(seed), new Counter(seed))",
    );
    let source_id = SourceId::from_index(20118);
    let output = TestOutputDirectory::new("mixed-signature-two-nested-origins");
    let mut proof_error = None;
    let mut observe_source = |ast: &AstArena<Untyped>,
                              source_root: TreeId<Untyped>,
                              index: &SourceSemanticIndex,
                              bindings: &SourceTypedIndex,
                              unit: &mut JvmCompilationUnit<'_>,
                              definitions: &Definitions| {
        proof_error = Some(
            match validate_cross_class_constructor_call_proofs(
                CrossClassConstructorCallProofInput {
                    source_id,
                    source_root,
                    store: unit.store,
                    definitions,
                    source_ast: ast,
                    typed_ast: unit.typed_ast,
                    source_index: index,
                    source_typed_index: bindings,
                    jvm_unit: unit,
                },
            ) {
                Ok(proofs) => format!("unexpected receipt count: {}", proofs.len()),
                Err(error) => format!("{error:?}"),
            },
        );
    };
    compile_source_batch_with_source_observer(
        &source,
        "PairFactory.scala",
        output.path(),
        source_id,
        SourceUnitCardinality::AtLeastTwo,
        &mut observe_source,
    )
    .expect("two nested allocations are valid source/JVM input but outside this proof shape");
    assert_eq!(
        proof_error.as_deref(),
        Some(
            "SourceShape(\"mixed Pair arguments must contain exactly one formal and one nested Counter call\")"
        )
    );
}

#[test]
fn mixed_origin_pair_rejects_duplicate_original_method_declarations() {
    let source = MIXED_ORIGIN_WRAP_AND_CREATE_SOURCE.replace(
        "new Pair(existing, new Counter(seed))",
        "new Pair(existing, new Counter(seed))\n  def wrapAndCreate(existing: Counter, alternate: Boolean): Pair =\n    new Pair(existing, new Counter(0))",
    );
    let source_id = SourceId::from_index(20117);
    let output = TestOutputDirectory::new("mixed-origin-duplicate-method");
    let mut proof_error = None;
    let mut observe_source = |ast: &AstArena<Untyped>,
                              source_root: TreeId<Untyped>,
                              index: &SourceSemanticIndex,
                              bindings: &SourceTypedIndex,
                              unit: &mut JvmCompilationUnit<'_>,
                              definitions: &Definitions| {
        proof_error = Some(
            match validate_cross_class_constructor_call_proofs(
                CrossClassConstructorCallProofInput {
                    source_id,
                    source_root,
                    store: unit.store,
                    definitions,
                    source_ast: ast,
                    typed_ast: unit.typed_ast,
                    source_index: index,
                    source_typed_index: bindings,
                    jvm_unit: unit,
                },
            ) {
                Ok(proofs) => format!("unexpected receipt count: {}", proofs.len()),
                Err(error) => format!("{error:?}"),
            },
        );
    };
    compile_source_batch_with_source_observer(
        &source,
        "PairFactory.scala",
        output.path(),
        source_id,
        SourceUnitCardinality::AtLeastTwo,
        &mut observe_source,
    )
    .expect("the different-signature overload remains valid source/JVM input");
    assert_eq!(
        proof_error.as_deref(),
        Some("SourceShape(\"overloaded factory methods are unsupported\")")
    );
}

#[test]
fn mixed_origin_pair_proof_preserves_renamed_and_reordered_identities() {
    let sections = MIXED_ORIGIN_WRAP_AND_CREATE_SOURCE
        .split("\n\n")
        .collect::<Vec<_>>();
    assert_eq!(sections.len(), 4);
    let reordered = format!(
        "{}\n\n{}\n\n{}\n\n{}",
        sections[0], sections[2], sections[3], sections[1]
    );
    let renamed = reordered
        .replace("package example", "package renamed")
        .replace("PairFactory", "Assembler")
        .replace("Pair", "Envelope")
        .replace("Counter", "Meter")
        .replace("wrapAndCreate", "assemble")
        .replace("existing", "kept")
        .replace("seed", "initial");
    let source_id = SourceId::from_index(20112);
    let output = TestOutputDirectory::new("renamed-reordered-mixed-origin-proof");
    let mut observed = false;
    let mut observe_source = |ast: &AstArena<Untyped>,
                              source_root: TreeId<Untyped>,
                              source_index: &SourceSemanticIndex,
                              source_typed_index: &SourceTypedIndex,
                              unit: &mut JvmCompilationUnit<'_>,
                              definitions: &Definitions| {
        observed = true;
        let proofs =
            validate_cross_class_constructor_call_proofs(CrossClassConstructorCallProofInput {
                source_id,
                source_root,
                store: unit.store,
                definitions,
                source_ast: ast,
                typed_ast: unit.typed_ast,
                source_index,
                source_typed_index,
                jvm_unit: unit,
            })
            .expect("renamed and reordered declarations retain the same mixed proof contract");
        assert_eq!(proofs.len(), 1);
        let proof = &proofs[0];
        assert_eq!(
            proof.argument_proof().kind(),
            dotty::jvm_tasty_cross_class::ConstructorArgumentProofKind::FormalNestedPair
        );
        assert_eq!(
            unit.store
                .names
                .resolve(unit.store.symbols.get(proof.target_class()).name.text()),
            "Envelope"
        );
        assert_eq!(
            unit.store
                .names
                .resolve(unit.store.symbols.get(proof.caller_class()).name.text()),
            "Assembler"
        );
        assert_eq!(
            proof.method_descriptor(),
            "(Lrenamed/Meter;I)Lrenamed/Envelope;"
        );
        let ordered = proof.ordered_argument_pair().unwrap();
        let method_tree = match source_index.definition_of(proof.method()) {
            Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
            _ => panic!("factory method retains its original canonical declaration"),
        };
        let TreeKind::DefDef(method) = &ast.get(method_tree).kind else {
            panic!("the factory method has its source definition")
        };
        assert_eq!(
            ordered.source_method_formals(),
            [
                method.value_param_clauses[0][0],
                method.value_param_clauses[0][1]
            ]
        );
        assert_eq!(
            ordered.method_formals(),
            [
                source_index
                    .symbol_at(source_id, method.value_param_clauses[0][0])
                    .unwrap(),
                source_index
                    .symbol_at(source_id, method.value_param_clauses[0][1])
                    .unwrap(),
            ]
        );
        let TreeKind::Apply(outer) = &ast.get(method.rhs.unwrap()).kind else {
            panic!("the factory body retains its outer constructor application")
        };
        let argument_receipts = ordered.arguments();
        assert_eq!(argument_receipts[0].source_argument(), outer.args[0]);
        assert_eq!(argument_receipts[1].source_argument(), outer.args[1]);
        assert!(argument_receipts[0].formal().is_some());
        assert!(argument_receipts[0].nested_new().is_none());
        assert!(argument_receipts[1].formal().is_none());
        let nested = argument_receipts[1]
            .nested_new()
            .expect("the reordered source has a nested constructor in slot two");
        assert_eq!(nested.source_outer_argument(), outer.args[1]);
        assert_eq!(
            source_typed_index.get(source_id, outer.args[1]),
            Some(nested.typed_outer_argument())
        );
        assert_eq!(nested.class(), proof.formal_class().unwrap());
        let TreeKind::Ident(leaf) = &ast.get(nested.source_argument()).kind else {
            panic!("the nested constructor leaf remains the original seed reference")
        };
        assert_eq!(unit.store.names.resolve(leaf.name.text()), "initial");
        assert_eq!(
            nested.argument_formal(),
            ordered.method_formals()[1],
            "source formal order remains independent of Pair argument slots"
        );
    };
    compile_source_batch_with_source_observer(
        &renamed,
        "PairFactory.scala",
        output.path(),
        source_id,
        SourceUnitCardinality::AtLeastTwo,
        &mut observe_source,
    )
    .expect("renamed and reordered classes remain valid in one source session");
    assert!(observed);
}
