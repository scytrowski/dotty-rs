use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::reader::Reader;
use dotty_core::ast::{DefDef, Template, TreeKind, Typed, Untyped};
use dotty_core::types::{TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolFlags, SymbolId, SymbolKind,
    TreeId, Visibility,
};
use dotty_jvm::{
    GeneratedClass, JVM_CLASSFILE_MAJOR_VERSION, JvmClassInitializationPlan, JvmClassPlan,
    JvmCodegenError, JvmCompilationUnit, JvmFieldPlan, JvmFieldSourceVisibility, JvmLocalBinding,
    JvmMethodPlan, JvmNameKind, JvmOverrideIssue, JvmPrimaryConstructorPlan, emit_class,
};
use dotty_lexer::{ContextualScanner, LexerDiagnostic};
use dotty_namer::{
    NamerDiagnostic, NamerError, NamingOutcome, SourceParseStatus, SourceSemanticIndex,
    name_compilation_unit_with_diagnostics,
};
use dotty_parser::{ParseDiagnostic, parse_compilation_unit};
use dotty_typer::{SourceTyper, TyperError};

const FIXTURE_DIRECTORY: &str = "tests/fixtures/jvm";

type SourceBatchObserver<'observer> =
    dyn for<'a> FnMut(&mut JvmCompilationUnit<'a>, &Definitions) + 'observer;

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

fn compile_source_batch_observed(
    source_text: &str,
    source_file_name: &str,
    output_directory: &Path,
    source_id: SourceId,
    cardinality: SourceUnitCardinality,
    mut observe: Option<&mut SourceBatchObserver<'_>>,
) -> Result<Vec<EmittedArtifact>, PipelineFailure> {
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
        typer
            .complete_symbol(class.class)
            .map_err(PipelineFailure::Typer)?;
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
            typer
                .complete_symbol(method.symbol)
                .map_err(PipelineFailure::Typer)?;
        }
    }

    let mut fields_by_class = Vec::with_capacity(classes.len());
    for class in &classes {
        let mut fields = Vec::new();
        if matches!(
            class.initialization,
            JvmClassInitializationPlan::PrimaryConstructor(_)
        ) {
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
                let initializer = match definition.rhs {
                    Some(rhs) => {
                        let ty = match typer.store().symbols.get(field).info {
                            dotty_core::SymbolInfo::Complete(ty) => ty,
                            _ => return Err(PipelineFailure::UnexpectedSourceShape("field type")),
                        };
                        let context = typer
                            .field_initializer_context_for(field)
                            .map_err(PipelineFailure::Typer)?;
                        Some(
                            typer
                                .type_expression_expected(rhs, context, ty)
                                .map_err(PipelineFailure::Typer)?,
                        )
                    }
                    None => None,
                };
                let source_visibility = match &definition.metadata.visibility {
                    Some(dotty_core::ast::VisibilitySyntax::Private { qualifier: Some(_) }) => {
                        JvmFieldSourceVisibility::PrivateThis
                    }
                    _ => JvmFieldSourceVisibility::PlainPrivate,
                };
                fields.push(JvmFieldPlan {
                    field,
                    source_visibility,
                    initializer,
                });
            }
        }
        fields_by_class.push(fields);
    }

    let mut plans = Vec::with_capacity(classes.len());
    for (class, fields) in classes.into_iter().zip(fields_by_class) {
        let mut methods = Vec::with_capacity(class.methods.len());
        for source_method in class.methods {
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

fn audit_type_symbol(unit: &JvmCompilationUnit<'_>, ty: dotty_core::TypeId) -> Option<SymbolId> {
    match unit.store.types.get(ty) {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => Some(*symbol),
        _ => None,
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
                Some(
                    typer
                        .type_expression_expected(rhs, context, ty)
                        .map_err(PipelineFailure::Typer)?,
                )
            }
            None => None,
        };
        let source_visibility = match &definition.metadata.visibility {
            Some(dotty_core::ast::VisibilitySyntax::Private { qualifier: Some(_) }) => {
                JvmFieldSourceVisibility::PrivateThis
            }
            _ => JvmFieldSourceVisibility::PlainPrivate,
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
    let no_template_initializers = template.body.iter().all(|tree| match &ast.get(*tree).kind {
        TreeKind::DefDef(_) => true,
        TreeKind::ValDef(_) => index
            .symbol_at(source, *tree)
            .is_some_and(|symbol| store.symbols.get(symbol).kind == SymbolKind::Field),
        _ => false,
    });
    if !has_fields && !has_parameters {
        return Ok(prove_implicit_noarg_constructor(
            ast, template, source, index, store,
        ));
    }
    if !has_supported_noarg_parent(ast, template)
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
    let body_contains_methods_only = template
        .body
        .iter()
        .all(|tree| matches!(ast.get(*tree).kind, TreeKind::DefDef(_)));
    if constructor_matches_emitted_shape
        && has_supported_noarg_parent(ast, template)
        && template.self_val.is_none()
        && body_contains_methods_only
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
fn int_equality_remains_outside_the_relational_intrinsic_subset() {
    assert_source_failure(
        "int-equality-not-lowered",
        "class UnsupportedEquality { def equal(a: Int, b: Int): Boolean = a == b }\n",
        |failure| matches!(failure, PipelineFailure::Typer(_)),
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
            0x03..=0x0f
            | 0x1a..=0x1d
            | 0x2a..=0x2d
            | 0x3b..=0x3e
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
            | 0xb0
            | 0xb1 => 1,
            0x10 | 0x15 | 0x19 | 0x36 | 0x3a => 2,
            0x99..=0x9a | 0x9f..=0xa4 | 0xa7 | 0xb4..=0xb8 | 0xbb => 3,
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
        .filter(|(_, opcode)| matches!(*opcode, 0x99..=0x9a | 0x9f..=0xa4 | 0xa7))
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
fn unsupported_local_source_shapes_fail_at_their_pipeline_stage_without_artifacts() {
    assert_source_failure(
        "immutable-local-assignment",
        "class ImmutableAssignment { def run(): Int = { val fixed = 1; fixed = 2; fixed } }\n",
        |failure| matches!(failure, PipelineFailure::Typer(_)),
    );
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
            "array-method-parameter",
            "class CounterClient:\n  def use(values: Array[Int]): Int = 1\n",
        ),
        (
            "different-reference-join",
            "class Counter(start: Int)\nclass Other(start: Int)\nclass CounterClient:\n  def choose(flag: Boolean): Int =\n    val selected = if flag then new Counter(1) else new Other(2)\n    1\n",
        ),
        (
            "nested-reference-constructor-argument",
            "class Counter(start: Int)\nclass Box(counter: Counter)\nclass Client:\n  def nested(): Box = new Box(new Counter(1))\n",
        ),
        (
            "reference-returning-call-constructor-argument",
            "class Counter\nclass Box(counter: Counter)\nclass Client:\n  def make(): Counter = new Counter()\n  def nested(): Box = new Box(make())\n",
        ),
        (
            "mutable-reference-local-constructor-argument",
            "class Counter\nclass Box(counter: Counter)\nclass Client:\n  def nested(counter: Counter): Box =\n    var current = counter\n    new Box(current)\n",
        ),
        (
            "reference-field-constructor-argument",
            "class Counter\nclass Box(counter: Counter)\nclass Client(counter: Counter):\n  private val saved: Counter = counter\n  def nested(): Box = new Box(saved)\n",
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
            "reference-field",
            "class Counter\nclass Holder:\n  private val counter: Counter = new Counter()\n",
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
                PipelineFailure::Codegen(JvmCodegenError::UnsupportedJvmConstructorArgument { .. })
                    | PipelineFailure::Codegen(
                        JvmCodegenError::JvmConstructorCallArgumentTypeMismatch { .. }
                    )
                    | PipelineFailure::Typer(TyperError::ApplicationArgumentTypeMismatch { .. })
            ),
            "unplanned-object-class" => matches!(
                &error,
                PipelineFailure::Typer(TyperError::NewClassInfoUnavailable { .. })
            ),
            "null-literal" => matches!(
                &error,
                PipelineFailure::Typer(TyperError::NullLiteralTypingDeferred { .. })
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
            "nested-reference-constructor-argument"
            | "reference-returning-call-constructor-argument"
            | "mutable-reference-local-constructor-argument"
            | "reference-field-constructor-argument" => matches!(
                &error,
                PipelineFailure::Codegen(
                    JvmCodegenError::JvmConstructorCallArgumentTypeMismatch { .. }
                ) | PipelineFailure::Typer(TyperError::ApplicationArgumentTypeMismatch { .. })
                    | PipelineFailure::Codegen(
                        JvmCodegenError::UnsupportedMutableReferenceLocal { .. }
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
                PipelineFailure::Codegen(JvmCodegenError::ReferenceTypeMismatch { .. })
            ),
            "reference-field" => matches!(
                &error,
                PipelineFailure::Typer(TyperError::ExpectedExpressionTypeMismatch { .. })
                    | PipelineFailure::Codegen(
                        JvmCodegenError::UnsupportedJvmReferenceFieldInitializer { .. }
                    )
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
fn reference_constructor_arguments_lower_in_order_and_verify_initialized_values() {
    let output = TestOutputDirectory::new("reference-constructor-arguments");
    let artifacts = compile_planned_constructor_source(
        "class Counter(seed: Int):\n  private val value: Int = seed\n  def get(): Int = value\nclass Other()\nclass Holder(amount: Int, counter: Counter, enabled: Boolean):\n  private val saved: Counter = counter\n  private val savedAmount: Int = amount\n  private val savedEnabled: Boolean = enabled\n  def reference(): Counter = saved\n  def number(): Int = savedAmount\n  def flag(): Boolean = savedEnabled\nclass Pair(counter: Counter, other: Other):\n  private val savedCounter: Counter = counter\n  private val savedOther: Other = other\n  def first(): Counter = savedCounter\n  def second(): Other = savedOther\nclass OwnerBox(owner: Client):\n  private val savedOwner: Client = owner\n  def get(): Client = savedOwner\nclass Client:\n  def wrap(counter: Counter): Holder = new Holder(3, counter, true)\n  def wrapLocal(counter: Counter): Holder =\n    val saved = counter\n    new Holder(4, saved, false)\n  def pair(counter: Counter, other: Other): Pair = new Pair(counter, other)\n  def ownerBox(): OwnerBox = new OwnerBox(this)\n",
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
    let assert_constructor_ref = |code: &[u8], expected_owner: &str, descriptor: &str| {
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
        } = client
            .constant_pool
            .get(ConstantPoolIndex(index))
            .expect("constructor invocation is a Methodref")
        else {
            panic!("invokespecial points at Methodref")
        };
        assert_eq!(
            client.constant_pool.class_name(*class_index),
            Ok(expected_owner)
        );
        assert_eq!(
            client.constant_pool.name_and_type(*name_and_type_index),
            Ok(("<init>", descriptor))
        );
    };
    assert_constructor_ref(wrap, "Holder", "(ILCounter;Z)V");
    assert_constructor_ref(wrap_local, "Holder", "(ILCounter;Z)V");
    assert_constructor_ref(pair, "Pair", "(LCounter;LOther;)V");
    assert_constructor_ref(owner_box, "OwnerBox", "(LClient;)V");

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 output path");
        let harness = output
            .path()
            .join("ReferenceConstructorArgumentsHarness.java");
        fs::write(
            &harness,
            "public final class ReferenceConstructorArgumentsHarness { public static void main(String[] args) { Client client = new Client(); Counter counter = new Counter(17); Other other = new Other(); Holder holder = client.wrap(counter); if (holder.reference() != counter || holder.number() != 3 || !holder.flag()) throw new AssertionError(\"mixed constructor arguments\"); Holder local = client.wrapLocal(counter); if (local.reference() != counter || local.number() != 4 || local.flag()) throw new AssertionError(\"initialized local argument\"); Pair pair = client.pair(counter, other); if (pair.first() != counter || pair.second() != other) throw new AssertionError(\"distinct reference arguments\"); OwnerBox owner = client.ownerBox(); if (owner.get() != client) throw new AssertionError(\"initialized this argument\"); } }\n",
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
fn virtual_calls_use_validated_new_and_immutable_local_receivers() {
    let output = TestOutputDirectory::new("planned-virtual-calls");
    let artifacts = compile_planned_constructor_source(
        "class Counter(seed: Int):\n  def add(value: Int): Int = value + 1\n  def combine(left: Int, right: Int): Int = left * 10 + right\n  def get(): Int = 42\n  def enabled(): Boolean = true\n  def touch(): Unit = ()\n  private def hidden(): Int = 9\n  def privateCall(): Int = this.hidden()\n  def privateReceiver(seed: Int): Int = new Counter(seed).hidden()\nclass Other(seed: Int):\n  def get(): Int = 7\nclass Client:\n  def directGet(seed: Int): Int = new Counter(seed).get()\n  def fromNew(seed: Int): Int = new Counter(seed + 1).add(seed)\n  def ordered(seed: Int): Int = new Counter(seed).combine(seed + 1, seed + 2)\n  def booleanResult(seed: Int): Boolean = new Counter(seed).enabled()\n  def unitCall(seed: Int): Unit = new Counter(seed).touch()\n  def fromLocal(seed: Int): Int =\n    val counter = new Counter(seed)\n    counter.add(seed)\n  def fromOther(seed: Int): Int = new Other(seed).get()\n",
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
            "public final class VirtualCallHarness { public static void main(String[] args) { Client c = new Client(); if (c.directGet(1) != 42 || c.fromNew(3) != 4 || c.ordered(3) != 45 || !c.booleanResult(3) || c.fromLocal(3) != 4 || c.fromOther(3) != 7 || new Counter(1).privateCall() != 9 || new Counter(1).privateReceiver(2) != 9) throw new AssertionError(); c.unitCall(1); System.out.println(\"virtual calls ok\"); } }\n",
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
fn source_typing_rejects_reverse_and_unrelated_reference_arguments() {
    for (label, source) in [
        (
            "base-to-derived-argument",
            "class Base { def value(): Int = 1 }\nclass Derived extends Base {}\nclass Runner { def use(value: Derived): Int = value.value(); def invalid(base: Base): Int = use(base) }\n",
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
fn mutable_reference_locals_remain_rejected_without_publishing_classfiles() {
    let output = TestOutputDirectory::new("reference-var");
    let result = compile_planned_constructor_source(
        "class Counter(seed: Int)\nclass Client:\n  def run(seed: Int): Int =\n    var counter = new Counter(seed)\n    42\n",
        output.path(),
    );
    assert!(matches!(
        result,
        Err(PipelineFailure::Codegen(
            JvmCodegenError::UnsupportedMutableReferenceLocal { .. }
        ))
    ));
    assert!(
        output.class_files().is_empty(),
        "failed reference-var compilation must not publish a partial classfile"
    );
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

fn assert_javap_instruction_sequence(output: &str, method_header: &str, expected: &[&str]) {
    let method_start = output
        .find(method_header)
        .unwrap_or_else(|| panic!("javap output omitted method {method_header:?}:\n{output}"));
    let method = &output[method_start..];
    let method_end = method[1..]
        .find("\n  public ")
        .or_else(|| method[1..].find("\n  private "))
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
fn public_primitive_field_storage_is_rejected_without_an_artifact() {
    assert_source_failure(
        "public-field",
        "class PublicField():\n  val seed: Int = 1",
        |failure| {
            matches!(
                failure,
                PipelineFailure::Codegen(JvmCodegenError::UnsupportedJvmVisibility { .. })
            )
        },
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
fn super_method_source_is_rejected_by_typer_before_jvm_emission() {
    let output = TestOutputDirectory::new("super-method-unsupported");
    let source = "class Base { def value(): Int = 10 }; class Derived extends Base { def read(): Int = super.value() }";
    let error = compile_source_classes(source, "SuperMethod.scala", output.path())
        .expect_err("super.method() must fail in the current frontend");
    assert!(
        matches!(
            error,
            PipelineFailure::Typer(TyperError::UnsupportedExpression {
                expression_kind: "expression or declaration tree",
                ..
            })
        ),
        "super.method() failed at an unexpected pipeline stage: {error:?}"
    );
    assert!(output.class_files().is_empty());
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
