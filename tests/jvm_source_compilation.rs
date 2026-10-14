use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::reader::Reader;
use dotty_core::ast::{DefDef, Template, TreeKind, Untyped};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolId, SymbolKind, TreeId,
    Visibility,
};
use dotty_jvm::{
    GeneratedClass, JVM_CLASSFILE_MAJOR_VERSION, JvmClassInitializationPlan, JvmClassPlan,
    JvmCodegenError, JvmCompilationUnit, JvmMethodPlan, emit_class,
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
        methods.push(JvmMethodPlan {
            method: source_method.symbol,
            parameters: source_method.parameters,
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
            0x1b | 0x2a | 0x57 | 0xac | 0xb1 => 1,
            0x10 => 2,
            0xb6..=0xb8 => 3,
            other => panic!("unexpected opcode in Delegator fixture: {other:#04x}"),
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
fn unsupported_local_val_and_while_fail_at_the_jvm_codegen_stage() {
    let local_output = TestOutputDirectory::new("unsupported-local-val");
    let local_error = compile_source(
        "class LocalVal { def run(x: Int): Int = { val y = x; y } }\n",
        "LocalVal.scala",
        local_output.path(),
    )
    .unwrap_err();
    assert!(matches!(
        local_error,
        PipelineFailure::Codegen(JvmCodegenError::UnsupportedTypedExpression {
            node: "ValDef",
            ..
        })
    ));
    assert!(local_output.class_files().is_empty());

    let while_output = TestOutputDirectory::new("unsupported-while");
    let while_error = compile_source(
        "class Loop { def run(): Unit = { while true do () } }\n",
        "Loop.scala",
        while_output.path(),
    )
    .unwrap_err();
    assert!(matches!(
        while_error,
        PipelineFailure::Codegen(JvmCodegenError::UnsupportedTypedExpression { node: "While", .. })
    ));
    assert!(while_output.class_files().is_empty());
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
