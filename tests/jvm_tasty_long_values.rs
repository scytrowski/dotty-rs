use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class;

#[test]
fn semantic_oracle_retains_full_signed_long_values_and_control_flow() {
    let oracle = include_str!("fixtures/jvm_tasty_long_values/oracle.txt");
    for shape in [
        "scala.Long",
        "9000000000L",
        "-9223372036854775808L",
        "choose",
        "return",
    ] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}

fn emit(methods: &str) -> dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class LongValues {{ {methods} }}"),
        "LongValues.scala",
        SourceId::from_index(2473),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn long_literal_wire_values_preserve_all_64_bits() {
    use dotty_tasty::tasty::*;
    let pair = emit(
        "def big(): Long = 9_000_000_000L; def min(): Long = -9223372036854775808L; def max(): Long = 9223372036854775807L; def hex(): Long = 0xffffffffffffffffL; def bin(): Long = 0b101010L",
    );
    let file = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    let index = file.ast_address_index().unwrap();
    let values: Vec<_> = index
        .iter_nodes_with_tag(LONGCONST_TAG)
        .map(|node| {
            let RawTree::Leaf(term) =
                RawTree::decode(&mut Reader::new(&payload[node.offset..])).unwrap()
            else {
                panic!()
            };
            let TermValue::LongInt(value) = term.value else {
                panic!()
            };
            value
        })
        .collect();
    assert_eq!(values, [9_000_000_000, i64::MIN, i64::MAX, -1, 42]);
}

#[test]
fn long_parameters_and_results_have_exact_j_descriptors_and_slots() {
    use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
    let pair = emit(
        "def direct(x: Long): Long = x; def first(a: Int, x: Long, flag: Boolean, y: Long): Long = x; def last(a: Int, x: Long, flag: Boolean, y: Long): Long = y; def intResult(x: Long): Int = 7",
    );
    let file = ClassFile::decode(&mut Reader::new(pair.classfile_bytes())).unwrap();
    for (name, descriptor, slots, expected) in [
        ("direct", "(J)J", 3, &[0x1f, 0xad][..]),
        ("first", "(IJZJ)J", 7, &[0x20, 0xad][..]),
        ("last", "(IJZJ)J", 7, &[0x16, 5, 0xad][..]),
        ("intResult", "(J)I", 3, &[0x10, 7, 0xac][..]),
    ] {
        let method = file
            .methods
            .iter()
            .find(|m| file.constant_pool.utf8(m.name_index) == Ok(name))
            .unwrap();
        assert_eq!(
            file.constant_pool.utf8(method.descriptor_index),
            Ok(descriptor)
        );
        let [Attribute::Code(code)] = method.attributes.as_slice() else {
            panic!()
        };
        assert_eq!(code.max_locals, slots);
        assert_eq!(code.code, expected);
    }
}

#[test]
fn long_locals_if_while_and_local_returns_compose_in_the_real_pipeline() {
    for method in [
        "def f(x: Long): Long = { val saved: Long = x; saved }",
        "def f(x: Long): Long = { val saved = x; saved }",
        "def f(): Long = { val saved = 9000000000L; saved }",
        "def f(a: Long, b: Long): Long = { var saved: Long = a; saved = b; saved }",
        "def f(flag: Boolean, a: Long, b: Long): Long = if flag then a else b",
        "def f(flag: Boolean): Long = if flag then 1L else 9000000000L",
        "def f(n: Int, a: Long, b: Long): Long = { var saved: Long = a; var i: Int = n; while i > 0 do { saved = b; i = i - 1 }; saved }",
        "def f(flag: Boolean, a: Long, b: Long): Long = { if flag then return a; b }",
        "def f(flag: Boolean, a: Long, b: Long): Long = if flag then return a else return b",
        "def f(n: Int, a: Long, b: Long): Long = { var saved: Long = a; var i: Int = n; while i > 0 do { if i == 3 then return a; saved = b; i = i - 1 }; saved }",
        "def f(flag: Boolean, a: Long, b: Long): Long = { val saved: Long = if flag then a else b; saved }",
        "def f(flag: Boolean, a: Long, b: Long): Long = { var saved: Long = a; saved = if flag then b else a; saved }",
    ] {
        emit(method);
    }
}

#[test]
fn long_matches_and_proved_abrupt_long_tails_emit_without_a_placeholder_value() {
    for method in [
        "def f(x: Int, a: Long, b: Long): Long = x match { case 0 => a; case _ => b }",
        "def f(x: Int, a: Long, b: Long): Long = x match { case n if n < 0 => a; case _ => b }",
        "def f(x: Int): Long = x match { case 0 => 1L; case _ => 9000000000L }",
        "def f(x: Int, a: Long, b: Long): Long = x match { case 0 => return a; case _ => b }",
        "def f(x: Int, a: Long, b: Long): Long = x match { case 0 => return a; case _ => return b }",
        "def f(flag: Boolean, a: Long, b: Long): Long = if flag then { return a; b } else b",
        "def f(x: Int, a: Long, b: Long): Long = x match { case 0 => { return a; b }; case _ => b }",
        "def f(flag: Boolean, a: Long, b: Long): Long = if flag then { val saved: Long = b; return a; saved } else b",
    ] {
        emit(method);
    }
}

#[test]
fn the_full_long_fixture_is_deterministic_across_fresh_sessions() {
    let source = include_str!("fixtures/jvm_tasty_long_values/LongValues.scala");
    let first = emit_paired_jvm_tasty_class(source, "LongValues.scala", SourceId::from_index(2473))
        .unwrap();
    let second =
        emit_paired_jvm_tasty_class(source, "LongValues.scala", SourceId::from_index(2474))
            .unwrap();
    assert_eq!(first.classfile_bytes(), second.classfile_bytes());
    assert_eq!(first.tasty_bytes(), second.tasty_bytes());
}

#[test]
fn pinned_scala_consumes_and_recompiles_original_rust_long_tasty() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_long_values/LongValues.scala"),
        "LongValues.scala",
        SourceId::from_index(2473),
    )
    .unwrap();
    let directory =
        std::env::temp_dir().join(format!("dotty-long-value-scala-{}", std::process::id()));
    std::fs::create_dir_all(directory.join("example")).unwrap();
    std::fs::write(
        directory.join(pair.classfile_path()),
        pair.classfile_bytes(),
    )
    .unwrap();
    std::fs::write(directory.join(pair.tasty_path()), pair.tasty_bytes()).unwrap();
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/jvm_tasty_long_values/run.py"
        ))
        .arg(&directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("{}", String::from_utf8_lossy(&output.stdout));
    std::fs::remove_dir_all(directory).unwrap();
}

fn jdk_available() -> bool {
    let version = std::process::Command::new("java")
        .arg("-version")
        .output()
        .ok();
    let major = version.as_ref().and_then(|output| {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        text.split('"')
            .nth(1)?
            .split('.')
            .next()?
            .parse::<u32>()
            .ok()
    });
    let available = major.is_some_and(|major| major >= 21);
    if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        assert!(
            available,
            "mandatory long-value runtime gate requires JDK 21+"
        );
    }
    available
}

fn run_java(pair: &dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts, name: &str, source: &str) {
    let directory =
        std::env::temp_dir().join(format!("dotty-long-values-{}-{name}", std::process::id()));
    std::fs::create_dir_all(directory.join("example")).unwrap();
    std::fs::write(
        directory.join(pair.classfile_path()),
        pair.classfile_bytes(),
    )
    .unwrap();
    std::fs::write(directory.join(pair.tasty_path()), pair.tasty_bytes()).unwrap();
    let client = directory.join(format!("{name}.java"));
    std::fs::write(&client, source).unwrap();
    let compile = std::process::Command::new("javac")
        .args(["--release", "21", "-cp"])
        .arg(&directory)
        .arg("-d")
        .arg(&directory)
        .arg(&client)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = std::process::Command::new("java")
        .args(["-Xverify:all", "-ea", "-cp"])
        .arg(&directory)
        .arg(name)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "long value oracle ok"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn original_rust_long_values_execute_on_jdk21() {
    if !jdk_available() {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_long_values/LongValues.scala"),
        "LongValues.scala",
        SourceId::from_index(2472),
    )
    .unwrap();
    run_java(
        &pair,
        "JavaConsumer",
        include_str!("fixtures/jvm_tasty_long_values/JavaConsumer.java"),
    );
}

fn frames(
    file: &dotty_classfile::class_file::ClassFile<'_>,
    name: &str,
) -> Vec<(u32, Vec<u8>, Vec<u8>)> {
    use dotty_classfile::{attribute::Attribute, reader::Reader};
    fn types(r: &mut Reader<'_>) -> Vec<u8> {
        let count = r.read_u16().unwrap();
        (0..count)
            .map(|_| {
                let tag = r.read_u8().unwrap();
                assert!(tag <= 8);
                if tag == 7 || tag == 8 {
                    r.read_u16().unwrap();
                }
                tag
            })
            .collect()
    }
    let method = file
        .methods
        .iter()
        .find(|m| file.constant_pool.utf8(m.name_index) == Ok(name))
        .unwrap();
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!()
    };
    let data = code
        .attributes
        .iter()
        .find_map(|a| match a {
            Attribute::Other(raw)
                if file.constant_pool.utf8(raw.name_index) == Ok("StackMapTable") =>
            {
                Some(raw.bytes)
            }
            _ => None,
        })
        .unwrap();
    let mut r = Reader::new(data);
    let count = r.read_u16().unwrap();
    let mut previous: i32 = -1;
    (0..count)
        .map(|_| {
            assert_eq!(r.read_u8().unwrap(), 255);
            let pc = previous + i32::from(r.read_u16().unwrap()) + 1;
            previous = pc;
            (pc as u32, types(&mut r), types(&mut r))
        })
        .collect()
}

#[test]
fn long_match_join_and_loop_frames_count_values_separately_from_slots() {
    use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
    let pair = emit(
        "def choose(x: Int, a: Long, b: Long): Long = x match { case 0 => a; case _ => b }; def allReturn(x: Int, a: Long, b: Long): Long = x match { case 0 => return a; case _ => return b }; def rotate(n: Int, a: Long, b: Long): Long = { var saved: Long = a; var i: Int = n; while i > 0 do { saved = b; i = i - 1 }; saved }",
    );
    let file = ClassFile::decode(&mut Reader::new(pair.classfile_bytes())).unwrap();
    for (name, slots) in [("choose", 7), ("allReturn", 7), ("rotate", 9)] {
        let method = file
            .methods
            .iter()
            .find(|m| file.constant_pool.utf8(m.name_index) == Ok(name))
            .unwrap();
        let [Attribute::Code(code)] = method.attributes.as_slice() else {
            panic!()
        };
        assert_eq!(code.max_stack, 2);
        assert_eq!(code.max_locals, slots);
        if name == "choose" {
            assert_eq!(
                code.code,
                &[
                    0x1b, 0x36, 6, 0x15, 6, 3, 0xa0, 0, 7, 0x20, 0xa7, 0, 5, 0x16, 4, 0xad
                ]
            );
            assert_eq!(
                frames(&file, name),
                [
                    (13, vec![7, 1, 4, 4, 1], vec![]),
                    (15, vec![7, 1, 4, 4, 1], vec![4])
                ]
            );
        } else if name == "allReturn" {
            assert_eq!(
                code.code,
                &[
                    0x1b, 0x36, 6, 0x15, 6, 3, 0xa0, 0, 5, 0x20, 0xad, 0x16, 4, 0xad
                ]
            );
            assert_eq!(frames(&file, name), [(11, vec![7, 1, 4, 4, 1], vec![])]);
        } else {
            let frames = frames(&file, name);
            assert!(!frames.is_empty());
            assert!(
                frames
                    .iter()
                    .all(|(_, locals, _)| locals == &[7, 1, 4, 4, 4, 1])
            );
        }
    }
}

#[test]
fn forged_long_values_bindings_and_flow_are_rejected_without_store_mutation() {
    use dotty::core::{
        Constant, SymbolKind, TermRefTarget, Type, Typed,
        ast::{AstArena, TreeKind},
    };
    use dotty_tasty_pickler::{ClassStatePicklePlan, PickleError, PrimaryConstructorInputPlan};
    for (methods, mode) in [
        (
            "def a(): Long = 9000000000L; def b(): Long = 13294967296L",
            0,
        ),
        ("def a(): Long = 9000000000L", 1),
        ("def f(x: Long): Long = return x", 2),
        ("def f(x: Long): Long = return x", 3),
        (
            "def f(x: Int, a: Long, b: Long): Long = x match { case 0 => return a; case _ => b }",
            4,
        ),
        (
            "def f(x: Int, a: Long, b: Long): Long = x match { case 0 => return a; case _ => return b }",
            5,
        ),
        ("def f(x: Long): Long = { val saved: Long = x; saved }", 6),
        ("def a(x: Long): Long = x; def b(x: Long): Long = x", 7),
        (
            "def f(x: Int, a: Long, b: Long): Long = x match { case 0 => { val saved: Long = a; saved }; case _ => { val saved: Long = b; saved } }",
            8,
        ),
    ] {
        let source = format!("package example; class LongValues {{ {methods} }}");
        dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
            &source,
            "LongValues.scala",
            SourceId::from_index(2473),
            |plan| {
                let raw = plan.tasty_plan.plan();
                let checkpoint = raw.store.checkpoint();
                let mut ast = AstArena::<Typed>::new();
                for (id, node) in raw.typed_ast.iter() {
                    assert_eq!(id, ast.alloc(node.clone()));
                }
                match mode {
                    0 | 1 => {
                        let literals: Vec<_> = ast
                            .iter()
                            .filter(|(_, n)| {
                                matches!(
                                    n.kind,
                                    TreeKind::Literal(dotty::core::ast::Literal {
                                        value: Constant::Long(_)
                                    })
                                )
                            })
                            .map(|(id, n)| (id, n.kind.clone(), n.ty))
                            .collect();
                        if mode == 0 {
                            ast.get_mut(literals[0].0).kind = literals[1].1.clone();
                            ast.get_mut(literals[0].0).ty = literals[1].2;
                        } else {
                            ast.get_mut(literals[0].0).ty = raw.definitions.double;
                        }
                    }
                    2 | 3 => {
                        let id = ast
                            .iter()
                            .find(|(_, n)| matches!(n.kind, TreeKind::Return(_)))
                            .unwrap()
                            .0;
                        if mode == 2 {
                            ast.get_mut(id).ty = raw.definitions.long;
                        } else {
                            let TreeKind::Return(r) = &mut ast.get_mut(id).kind else {
                                panic!()
                            };
                            r.from = r.expr;
                        }
                    }
                    4 | 5 => {
                        let id = ast
                            .iter()
                            .find(|(_, n)| matches!(n.kind, TreeKind::Match(_)))
                            .unwrap()
                            .0;
                        ast.get_mut(id).ty = if mode == 4 {
                            raw.definitions.nothing_type
                        } else {
                            raw.definitions.long
                        };
                    }
                    6 => {
                        let id = ast
                            .iter()
                            .find_map(|(_, n)| {
                                if let TreeKind::ValDef(v) = &n.kind {
                                    Some(v.tpt)
                                } else {
                                    None
                                }
                            })
                            .unwrap();
                        ast.get_mut(id).ty = raw.definitions.double;
                    }
                    7 | 8 => {
                        let kind = if mode == 7 {
                            SymbolKind::Parameter
                        } else {
                            SymbolKind::Local
                        };
                        let reads: Vec<_> = ast
                            .iter()
                            .filter(|(_, n)| matches!(n.kind, TreeKind::Ident(_)))
                            .filter_map(|(id, n)| match raw.store.types.try_get(n.ty) {
                                Some(Type::TermRef {
                                    target: TermRefTarget::Symbol(s),
                                    ..
                                }) if raw.store.symbols.get(*s).kind == kind => Some((id, n.ty)),
                                _ => None,
                            })
                            .collect();
                        assert_eq!(reads.len(), 2);
                        ast.get_mut(reads[1].0).ty = reads[0].1;
                        if mode == 8 {
                            let blocks: Vec<_> = ast
                                .iter()
                                .filter_map(|(id, n)| match &n.kind {
                                    TreeKind::Block(block) if block.expr == reads[1].0 => Some(id),
                                    _ => None,
                                })
                                .collect();
                            for &block in &blocks {
                                ast.get_mut(block).ty = reads[0].1;
                            }
                            let cases: Vec<_> = ast
                                .iter()
                                .filter_map(|(id, n)| match &n.kind {
                                    TreeKind::CaseDef(case) if blocks.contains(&case.body) => {
                                        Some(id)
                                    }
                                    _ => None,
                                })
                                .collect();
                            for case in cases {
                                ast.get_mut(case).ty = reads[0].1;
                            }
                        }
                    }
                    _ => unreachable!(),
                }
                let input = ClassStatePicklePlan {
                    store: raw.store,
                    definitions: raw.definitions,
                    typed_ast: &ast,
                    source_ast: raw.source_ast,
                    source_index: raw.source_index,
                    typed_bindings: raw.typed_bindings,
                    source: raw.source.clone(),
                    class: raw.class,
                    class_tree: raw.class_tree,
                    constructor: PrimaryConstructorInputPlan {
                        symbol: raw.constructor.symbol,
                        source_tree: raw.constructor.source_tree,
                        parameter_clauses: raw.constructor.parameter_clauses.clone(),
                    },
                    members: raw.members,
                };
                let error = input
                    .validate()
                    .and_then(|v| v.validate_method_bodies())
                    .unwrap_err();
                match mode {
                    0 => assert!(
                        matches!(error, PickleError::SourceShape { .. }),
                        "{error:?}"
                    ),
                    1 | 4 | 5 => {
                        assert!(matches!(error, PickleError::BodyType { .. }), "{error:?}")
                    }
                    2 | 3 => assert!(
                        matches!(error, PickleError::Unsupported { .. }),
                        "{error:?}"
                    ),
                    6 => assert!(
                        matches!(error, PickleError::UnsupportedTypeReference { .. }),
                        "{error:?}"
                    ),
                    7 | 8 => assert!(matches!(error, PickleError::Reference { .. }), "{error:?}"),
                    _ => unreachable!(),
                }
                assert_eq!(checkpoint, raw.store.checkpoint());
            },
        )
        .unwrap();
    }
}

#[test]
fn unsupported_long_and_other_category_two_shapes_publish_no_pair() {
    for methods in [
        "private val x: Long = 1L; def f(): Long = x",
        "def other(): Long = 1L; def f(): Long = other()",
        "def other(x: Long): Int = 1; def f(x: Long): Int = other(x)",
        "def f(x: Long): Long = x + 1L",
        "def f(x: Long): Int = x match { case 0L => 1; case _ => 2 }",
        "def f(a: Long, b: Long): Long = { a; b }",
        "def f(): Long = 1",
        "def f(x: Double): Double = (x: Double)",
        "def f(x: Float): Float = (x: Float)",
        "def f(x: Long): Long = { val saved: Long = x; saved = x; saved }",
        "def f(x: Long): Long = { val saved: Long = return x; saved }",
        "def f(using x: Long): Long = x",
    ] {
        let source = format!("package example; class LongValues {{ {methods} }}");
        let result =
            emit_paired_jvm_tasty_class(&source, "LongValues.scala", SourceId::from_index(2473));
        assert!(
            matches!(
                result,
                Err(dotty::jvm_tasty_artifacts::PairedArtifactError::Plan(_))
            ),
            "{methods}: {result:?}"
        );
    }
    assert!(
        emit_paired_jvm_tasty_class(
            "package example; class LongValues(x: Long) { def f(): Int = 1 }",
            "LongValues.scala",
            SourceId::from_index(2473)
        )
        .is_err()
    );
}
