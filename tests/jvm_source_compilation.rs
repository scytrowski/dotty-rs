Warning: truncated output (original token count: 269015)
... 27484 bytes omitted ...

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
                        JvmCodegenError::Missi…250152 tokens truncated…t: unit,
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
