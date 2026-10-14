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
    JvmCodegenError, JvmCompilationUnit, JvmIntIntrinsic, JvmLocalBinding, JvmMethodPlan,
    emit_class,
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
    let initialization =
        prove_implicit_noarg_constructor(&parsed.ast, template, source_id, &index, &store);

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
fn canonical_int_arithmetic_lowers_to_ordered_jvm_instructions() {
    let output = TestOutputDirectory::new("int-arithmetic-intrinsics");
    let artifact = compile_source(
        "class IntegerOps { def add(a: Int, b: Int): Int = a + b; def subtract(a: Int, b: Int): Int = a - b; def multiply(a: Int, b: Int): Int = a * b; def divide(a: Int, b: Int): Int = a / b; def remainder(a: Int, b: Int): Int = a % b; def left(): Int = 6; def right(): Int = 2; def ordered(): Int = left() + right(); def nested(x: Int, y: Int): Int = (x + y) * 2; def localState(n: Int): Int = { var total = n + 1; total = total + n; total }; def branch(x: Int): Int = if true then x + 1 else x * 2; def loop(): Int = { var total = 0; while false do { total = total + 1 }; total } }\n",
        "IntegerOps.scala",
        output.path(),
    )
    .unwrap();
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

    if require_jdk_or_skip() {
        let output_path = output.path().to_str().expect("UTF-8 output path");
        let harness_path = output.path().join("IntegerOpsHarness.java");
        fs::write(
            &harness_path,
            "public final class IntegerOpsHarness { public static void main(String[] args) { IntegerOps value = new IntegerOps(); if (value.add(2, 5) != 7 || value.subtract(2, 5) != -3 || value.multiply(-7, 3) != -21) throw new AssertionError(\"basic arithmetic\"); if (value.divide(-7, 3) != -2 || value.divide(Integer.MIN_VALUE, -1) != Integer.MIN_VALUE) throw new AssertionError(\"division semantics\"); if (value.remainder(-7, 3) != -1 || value.remainder(7, -3) != 1 || value.remainder(Integer.MIN_VALUE, -1) != 0) throw new AssertionError(\"remainder semantics\"); if (value.add(Integer.MAX_VALUE, 1) != Integer.MIN_VALUE) throw new AssertionError(\"addition overflow\"); if (value.ordered() != 8 || value.nested(3, 4) != 14 || value.localState(5) != 11 || value.branch(3) != 4 || value.loop() != 0) throw new AssertionError(\"nested/local/control-flow lowering\"); try { value.divide(1, 0); throw new AssertionError(\"division by zero returned\"); } catch (ArithmeticException expected) { } try { value.remainder(1, 0); throw new AssertionError(\"remainder by zero returned\"); } catch (ArithmeticException expected) { } System.out.println(\"integer intrinsics ok\"); } }\n",
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
fn canonical_int_comparison_is_recognized_but_not_miscompiled_as_a_call() {
    assert_source_failure(
        "int-comparison-not-lowered",
        "class UnsupportedComparison { def less(a: Int, b: Int): Boolean = a < b }\n",
        |failure| {
            matches!(
                failure,
                PipelineFailure::Codegen(JvmCodegenError::UnsupportedIntIntrinsic {
                    intrinsic: JvmIntIntrinsic::IntLessThan,
                    ..
                })
            )
        },
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
            | 0x2a
            | 0x3b..=0x3e
            | 0x57
            | 0x60
            | 0x64
            | 0x68
            | 0x6c
            | 0x70
            | 0xac
            | 0xb1 => 1,
            0x10 | 0x15 | 0x36 => 2,
            0x99 | 0xa7 | 0xb6..=0xb8 => 3,
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
        .filter(|(_, opcode)| matches!(*opcode, 0x99 | 0xa7))
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
fn field_initializer_is_rejected_without_a_partial_class_file() {
    let output = TestOutputDirectory::new("field-initializer");
    let error = compile_fixture("FieldInitializer.scala", output.path()).unwrap_err();
    assert!(matches!(
        error,
        PipelineFailure::Codegen(JvmCodegenError::UnsupportedConstructor { .. })
    ));
    assert!(output.class_files().is_empty());
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
            methods: method_plans,
            initialization: JvmClassInitializationPlan::Unsupported,
        }],
    };
    assert_eq!(unit.validate(), Ok(()));
}
