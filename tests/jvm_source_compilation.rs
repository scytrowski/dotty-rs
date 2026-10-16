use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::reader::Reader;
use dotty_core::ast::{DefDef, Template, TreeKind, Untyped};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolFlags, SymbolId, SymbolKind,
    TreeId, Visibility,
};
use dotty_jvm::{
    GeneratedClass, JVM_CLASSFILE_MAJOR_VERSION, JvmClassInitializationPlan, JvmClassPlan,
    JvmCodegenError, JvmCompilationUnit, JvmFieldPlan, JvmFieldSourceVisibility, JvmLocalBinding,
    JvmMethodPlan, JvmNameKind, JvmPrimaryConstructorPlan, emit_class,
};
use dotty_lexer::{ContextualScanner, LexerDiagnostic};
use dotty_namer::{
    NamerDiagnostic, NamerError, NamingOutcome, SourceParseStatus, SourceSemanticIndex,
    name_compilation_unit_with_diagnostics,
};
use dotty_parser::{ParseDiagnostic, parse_compilation_unit};
use dotty_typer::{SourceTyper, TyperError};

const FIXTURE_DIRECTORY: &str = "tests/fixtures/jvm";

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

fn compile_source(
    source_text: &str,
    source_file_name: &str,
    output_directory: &Path,
) -> Result<EmittedArtifact, PipelineFailure> {
    let source_id = SourceId::from_index(1);
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

    let (class_symbol, template_tree) =
        find_single_source_class(&parsed.ast, parsed.root, source_id, &index, &store)?;
    let template = match &parsed.ast.get(template_tree).kind {
        TreeKind::Template(template) => template,
        _ => return Err(PipelineFailure::UnexpectedSourceShape("class template")),
    };
    let source_methods =
        collect_source_methods(&parsed.ast, &template.body, source_id, &index, &store)?;
    let initialization = source_initialization_plan(
        class_symbol,
        &parsed.ast,
        template,
        source_id,
        &index,
        &store,
    )
    .map_err(PipelineFailure::Codegen)?;

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer
        .complete_symbol(class_symbol)
        .map_err(PipelineFailure::Typer)?;
    let constructor_symbol = index.symbol_at(source_id, template.constructor).ok_or(
        PipelineFailure::MissingSourceIdentity("primary constructor"),
    )?;
    let mut fields = Vec::new();
    if matches!(
        initialization,
        JvmClassInitializationPlan::PrimaryConstructor(_)
    ) {
        typer
            .complete_symbol(constructor_symbol)
            .map_err(PipelineFailure::Typer)?;
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
            class: class_symbol,
            fields,
            methods,
            initialization,
        }],
    };
    let class = emit_class(&unit, class_symbol, &definitions).map_err(PipelineFailure::Codegen)?;
    let path = output_directory.join(format!("{}.class", class.internal_name()));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(PipelineFailure::Io)?;
    }
    let staged_path = path.with_extension("class.tmp");
    fs::write(&staged_path, class.bytes()).map_err(PipelineFailure::Io)?;
    fs::rename(&staged_path, &path).map_err(PipelineFailure::Io)?;
    Ok(EmittedArtifact { class, path })
}

fn compile_planned_constructor_source(
    source_text: &str,
    output_directory: &Path,
) -> Result<Vec<EmittedArtifact>, PipelineFailure> {
    let source_id = SourceId::from_index(1218);
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
        "ConstructorSmoke.scala",
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
        let Some(class) = index.symbol_at(source_id, *tree) else {
            return Err(PipelineFailure::MissingSourceIdentity("class declaration"));
        };
        if store.symbols.get(class).kind != SymbolKind::Class {
            continue;
        }
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            return Err(PipelineFailure::UnexpectedSourceShape("class template"));
        };
        let initialization =
            source_initialization_plan(class, &parsed.ast, template, source_id, &index, &store)
                .map_err(PipelineFailure::Codegen)?;
        if template
            .body
            .iter()
            .any(|tree| matches!(parsed.ast.get(*tree).kind, TreeKind::ValDef(_)))
        {
            return Err(PipelineFailure::UnexpectedSourceShape(
                "constructor smoke classes without fields",
            ));
        }
        classes.push((
            class,
            definition.rhs,
            template.constructor,
            initialization,
            collect_source_methods(&parsed.ast, &template.body, source_id, &index, &store)?,
        ));
    }
    if classes.len() < 2 {
        return Err(PipelineFailure::UnexpectedSourceShape(
            "at least two planned constructor smoke classes",
        ));
    }

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for (class, _, constructor_tree, initialization, _) in &classes {
        typer
            .complete_symbol(*class)
            .map_err(PipelineFailure::Typer)?;
        let constructor = index.symbol_at(source_id, *constructor_tree).ok_or(
            PipelineFailure::MissingSourceIdentity("primary constructor"),
        )?;
        if matches!(
            initialization,
            JvmClassInitializationPlan::PrimaryConstructor(_)
        ) {
            typer
                .complete_symbol(constructor)
                .map_err(PipelineFailure::Typer)?;
        }
    }

    let mut plans = Vec::with_capacity(classes.len());
    for (class, _, _, initialization, source_methods) in classes {
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
            class,
            fields: Vec::new(),
            methods,
            initialization,
        });
    }

    let unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: plans,
    };
    let mut classes = Vec::with_capacity(unit.classes.len());
    for plan in &unit.classes {
        let class =
            emit_class(&unit, plan.class, &definitions).map_err(PipelineFailure::Codegen)?;
        classes.push(class);
    }
    let mut emitted = Vec::with_capacity(classes.len());
    for class in classes {
        let path = output_directory.join(format!("{}.class", class.internal_name()));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(PipelineFailure::Io)?;
        }
        fs::write(&path, class.bytes()).map_err(PipelineFailure::Io)?;
        emitted.push(EmittedArtifact { class, path });
    }
    Ok(emitted)
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
    if !template.parents.is_empty() || template.self_val.is_some() || !no_template_initializers {
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
        && template.parents.is_empty()
        && template.self_val.is_none()
        && body_contains_methods_only
    {
        JvmClassInitializationPlan::ImplicitNoArg
    } else {
        JvmClassInitializationPlan::Unsupported
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
    assert_source_failure(
        "short-circuit-reference-call",
        "class ShortCircuitReferenceCall { def probe(): Boolean = false; def run(flag: Boolean): Boolean = flag && probe() }\n",
        |failure| {
            matches!(
                failure,
                PipelineFailure::Codegen(JvmCodegenError::JvmCallInUnsupportedControlFlow { .. })
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
            | 0xb1 => 1,
            0x10 | 0x15 | 0x36 => 2,
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
fn while_loops_reject_invalid_conditions_and_calls_inside_control_flow() {
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
    let call_error = compile_source(
        "class WhileCall { def next(): Boolean = false; def run(flag: Boolean): Int = { var running = flag; while running do running = next(); 0 } }\n",
        "WhileCall.scala",
        call_output.path(),
    )
    .unwrap_err();
    assert!(matches!(
        call_error,
        PipelineFailure::Codegen(JvmCodegenError::JvmCallInUnsupportedControlFlow { .. })
    ));
    assert!(call_output.class_files().is_empty());

    let branch_output = TestOutputDirectory::new("if-call-control-flow");
    let branch_error = compile_source(
        "class IfCall { def identity(value: Int): Int = value; def run(flag: Boolean): Int = if flag then identity(1) else 0 }\n",
        "IfCall.scala",
        branch_output.path(),
    )
    .unwrap_err();
    assert!(matches!(
        branch_error,
        PipelineFailure::Codegen(JvmCodegenError::JvmCallInUnsupportedControlFlow { .. })
    ));
    assert!(branch_output.class_files().is_empty());
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
fn reference_receiver_calls_reject_private_access_and_symbolic_branches() {
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
    assert!(
        matches!(
            branch_result,
            Err(PipelineFailure::Codegen(
                JvmCodegenError::JvmCallInUnsupportedControlFlow { .. }
                    | JvmCodegenError::ReferenceLocalInUnsupportedControlFlow { .. }
            ))
        ),
        "reference calls inside symbolic branches must fail closed, got {branch_result:?}"
    );
    assert!(branch_output.class_files().is_empty());
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
fn reference_var_and_reference_local_cfg_fail_without_publishing_classfiles() {
    for (label, source, expected) in [
        (
            "reference-var",
            "class Counter(seed: Int)\nclass Client:\n  def run(seed: Int): Int =\n    var counter = new Counter(seed)\n    42\n",
            "mutable",
        ),
        (
            "reference-cfg",
            "class Counter(seed: Int)\nclass Client:\n  def run(flag: Boolean, seed: Int): Int =\n    val counter = new Counter(seed)\n    if flag then 1 else 2\n",
            "cfg",
        ),
    ] {
        let output = TestOutputDirectory::new(label);
        let result = compile_planned_constructor_source(source, output.path());
        match (expected, result) {
            (
                "mutable",
                Err(PipelineFailure::Codegen(JvmCodegenError::UnsupportedMutableReferenceLocal {
                    ..
                })),
            ) => {}
            (
                "cfg",
                Err(PipelineFailure::Codegen(
                    JvmCodegenError::ReferenceLocalInUnsupportedControlFlow { .. },
                )),
            ) => {}
            (expected, other) => panic!("expected {expected} rejection, got {other:?}"),
        }
        assert!(
            output.class_files().is_empty(),
            "failed {label} compilation published a partial classfile"
        );
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
    assert_source_failure(
        "field-read-in-branch",
        "class BranchingField { private val current: Int = 1; def value(flag: Boolean): Int = if flag then current else current }",
        |failure| {
            matches!(
                failure,
                PipelineFailure::Codegen(
                    JvmCodegenError::JvmFieldReadInUnsupportedControlFlow { .. }
                )
            )
        },
    );
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
