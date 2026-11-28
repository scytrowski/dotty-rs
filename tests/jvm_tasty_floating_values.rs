#[test]
fn pinned_semantic_oracle_preserves_floating_values_and_control_flow() {
    let oracle = include_str!("fixtures/jvm_tasty_floating_values/oracle.txt");
    for shape in [
        "scala.Float",
        "scala.Double",
        "-0.0f",
        "1.4E-45f",
        "4.9E-324",
        "matchedD",
        "return",
    ] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}

use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class;

fn emit(methods: &str) -> dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class FloatingValues {{ {methods} }}"),
        "FloatingValues.scala",
        SourceId::from_index(2475),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn floating_literal_wire_payloads_preserve_exact_bits() {
    use dotty_tasty::tasty::*;
    let mut values = Vec::new();
    for (ty, literal) in [
        ("Float", "0.0f"),
        ("Float", "-0.0f"),
        ("Float", "1.4e-45f"),
        ("Float", "3.4028235e38f"),
        ("Double", "0.0"),
        ("Double", "-0.0"),
        ("Double", "4.9e-324"),
        ("Double", "1.7976931348623157e308"),
        ("Double", "-0.0D"),
    ] {
        let pair = emit(&format!("def f(): {ty} = {literal}"));
        let file = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
        let payload = file.section(StandardSection::Asts).unwrap().payload;
        let index = file.ast_address_index().unwrap();
        for n in index
            .iter_nodes()
            .filter(|n| matches!(n.tag, FLOATCONST_TAG | DOUBLECONST_TAG))
        {
            let RawTree::Leaf(term) =
                RawTree::decode(&mut Reader::new(&payload[n.offset..])).unwrap()
            else {
                panic!()
            };
            values.push(term.constant_value().unwrap().unwrap());
        }
    }
    assert_eq!(
        values,
        [
            ConstantValue::FloatBits(0),
            ConstantValue::FloatBits(0x80000000),
            ConstantValue::FloatBits(1),
            ConstantValue::FloatBits(0x7f7fffff),
            ConstantValue::DoubleBits(0),
            ConstantValue::DoubleBits(0x8000000000000000),
            ConstantValue::DoubleBits(1),
            ConstantValue::DoubleBits(0x7fefffffffffffff),
            ConstantValue::DoubleBits(0x8000000000000000)
        ]
    );
}

#[test]
fn mixed_primitive_formals_have_exact_f_d_descriptors_and_physical_slots() {
    use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
    let pair = emit(
        "def f(i: Int, a: Float, n: Long, b: Double, flag: Boolean): Float = a; def d(i: Int, a: Float, n: Long, b: Double, flag: Boolean): Double = b; def intResult(x: Double): Int = 7",
    );
    let file = ClassFile::decode(&mut Reader::new(pair.classfile_bytes())).unwrap();
    for (name, descriptor, slots, bytes) in [
        ("f", "(IFJDZ)F", 8, &[0x24, 0xae][..]),
        ("d", "(IFJDZ)D", 8, &[0x18, 5, 0xaf][..]),
        ("intResult", "(D)I", 3, &[0x10, 7, 0xac][..]),
    ] {
        let m = file
            .methods
            .iter()
            .find(|m| file.constant_pool.utf8(m.name_index) == Ok(name))
            .unwrap();
        assert_eq!(file.constant_pool.utf8(m.descriptor_index), Ok(descriptor));
        let [Attribute::Code(code)] = m.attributes.as_slice() else {
            panic!()
        };
        assert_eq!(code.max_locals, slots);
        assert_eq!(code.code, bytes);
    }
}

#[test]
fn floating_locals_if_loops_and_local_returns_compose() {
    for method in [
        "def f(x: Float): Float = { val saved = x; saved }",
        "def f(x: Double): Double = { val saved = x; saved }",
        "def f(): Double = { val saved = -0.0; saved }",
        "def f(a: Float, b: Float): Float = { var saved: Float = a; saved = b; saved }",
        "def f(a: Double, b: Double): Double = { var saved: Double = a; saved = b; saved }",
        "def f(flag: Boolean, a: Float, b: Float): Float = if flag then a else b",
        "def f(flag: Boolean, a: Double, b: Double): Double = if flag then a else b",
        "def f(n: Int, a: Float, b: Float): Float = { var saved: Float = a; var i: Int = n; while i > 0 do { saved = b; i = i - 1 }; saved }",
        "def f(n: Int, a: Double, b: Double): Double = { var saved: Double = a; var i: Int = n; while i > 0 do { if i == 2 then return a; saved = b; i = i - 1 }; saved }",
        "def f(x: Double): Double = return x",
        "def f(flag: Boolean, a: Float, b: Float): Float = if flag then return a else return b",
        "def f(flag: Boolean, a: Double, b: Double): Double = if flag then return a else b",
        "def f(flag: Boolean, a: Double, b: Double): Double = { val saved: Double = if flag then a else b; saved }",
        "def f(flag: Boolean, a: Float, b: Float): Float = { var saved: Float = a; saved = if flag then b else a; saved }",
        "def f(x: Int, a: Float, b: Float): Float = x match { case 0 | 1 => return a; case _ => return b }",
        "def f(x: Int, a: Double, b: Double): Double = x match { case 0 | 1 => return a; case _ => return b }",
    ] {
        emit(method);
    }
}

#[test]
fn forged_floating_bits_bindings_types_and_returns_fail_read_only() {
    use dotty::core::{
        Constant, SymbolKind, TermRefTarget, Type, Typed,
        ast::{AstArena, TreeKind},
    };
    use dotty_tasty_pickler::{ClassStatePicklePlan, PickleError, PrimaryConstructorInputPlan};
    for (ty, zero, negative) in [("Float", "0.0f", "-0.0f"), ("Double", "0.0", "-0.0")] {
        for mode in 0..9 {
            let methods = match mode {
                0 | 1 | 8 => format!("def a(): {ty} = {zero}; def b(): {ty} = {negative}"),
                2 | 3 => format!("def f(x: {ty}): {ty} = return x"),
                4 => format!("def f(x: {ty}): {ty} = {{ val saved: {ty} = x; saved }}"),
                5 => format!("def a(x: {ty}): {ty} = x; def b(x: {ty}): {ty} = x"),
                6 => {
                    format!("def f(flag: Boolean, a: {ty}, b: {ty}): {ty} = if flag then a else b")
                }
                7 => format!(
                    "def a(x: {ty}): {ty} = {{ val saved: {ty} = x; saved }}; def b(x: {ty}): {ty} = {{ val saved: {ty} = x; saved }}"
                ),
                _ => unreachable!(),
            };
            let source = format!("package example; class FloatingValues {{ {methods} }}");
            dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
                &source,
                "FloatingValues.scala",
                SourceId::from_index(2475),
                |paired| {
                    let raw = paired.tasty_plan.plan();
                    let checkpoint = raw.store.checkpoint();
                    let mut ast = AstArena::<Typed>::new();
                    for (id, n) in raw.typed_ast.iter() {
                        assert_eq!(id, ast.alloc(n.clone()));
                    }
                    match mode {
                        0 | 1 | 8 => {
                            let literals: Vec<_> = ast
                                .iter()
                                .filter(|(_, n)| {
                                    matches!(
                                        n.kind,
                                        TreeKind::Literal(dotty::core::ast::Literal {
                                            value: Constant::FloatBits(_) | Constant::DoubleBits(_)
                                        })
                                    )
                                })
                                .map(|(id, n)| (id, n.kind.clone(), n.ty))
                                .collect();
                            assert_eq!(literals.len(), 2);
                            if mode == 0 {
                                ast.get_mut(literals[0].0).kind = literals[1].1.clone();
                                ast.get_mut(literals[0].0).ty = literals[1].2;
                            } else if mode == 1 {
                                ast.get_mut(literals[0].0).ty = raw.definitions.long;
                            } else {
                                ast.get_mut(literals[0].0).ty = literals[1].2;
                            }
                        }
                        2 | 3 => {
                            let id = ast
                                .iter()
                                .find(|(_, n)| matches!(n.kind, TreeKind::Return(_)))
                                .unwrap()
                                .0;
                            if mode == 2 {
                                ast.get_mut(id).ty = raw.definitions.float;
                            } else {
                                let TreeKind::Return(r) = &mut ast.get_mut(id).kind else {
                                    panic!()
                                };
                                r.from = r.expr;
                            }
                        }
                        4 => {
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
                            ast.get_mut(id).ty = raw.definitions.long;
                        }
                        5 | 7 => {
                            let kind = if mode == 5 {
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
                                    }) if raw.store.symbols.get(*s).kind == kind => {
                                        Some((id, n.ty))
                                    }
                                    _ => None,
                                })
                                .collect();
                            assert_eq!(reads.len(), 2);
                            ast.get_mut(reads[1].0).ty = reads[0].1;
                            if mode == 7 {
                                let blocks: Vec<_> = ast
                                    .iter()
                                    .filter_map(|(id, n)| match &n.kind {
                                        TreeKind::Block(b) if b.expr == reads[1].0 => Some(id),
                                        _ => None,
                                    })
                                    .collect();
                                for id in blocks {
                                    ast.get_mut(id).ty = reads[0].1;
                                }
                            }
                        }
                        6 => {
                            let id = ast
                                .iter()
                                .find(|(_, n)| matches!(n.kind, TreeKind::If(_)))
                                .unwrap()
                                .0;
                            ast.get_mut(id).ty = raw.definitions.nothing_type;
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
                            "{ty}/{mode}: {error:?}"
                        ),
                        1 | 6 | 8 => assert!(
                            matches!(error, PickleError::BodyType { .. }),
                            "{ty}/{mode}: {error:?}"
                        ),
                        2 | 3 => assert!(
                            matches!(error, PickleError::Unsupported { .. }),
                            "{ty}/{mode}: {error:?}"
                        ),
                        4 => assert!(
                            matches!(error, PickleError::UnsupportedTypeReference { .. }),
                            "{ty}/{mode}: {error:?}"
                        ),
                        5 | 7 => assert!(
                            matches!(error, PickleError::Reference { .. }),
                            "{ty}/{mode}: {error:?}"
                        ),
                        _ => unreachable!(),
                    }
                    assert_eq!(checkpoint, raw.store.checkpoint());
                },
            )
            .unwrap();
        }
    }
}

#[test]
fn floating_matches_and_proved_abrupt_tails_compose() {
    for ty in ["Float", "Double"] {
        for body in [
            "x match { case 0 | 1 if flag => a; case _ => b }",
            "x match { case n if n > 0 => a; case _ => b }",
            "x match { case 0 | 1 => return a; case _ => b }",
            "if flag then { return a; b } else b",
            "if flag then { val saved = b; return a; saved } else b",
        ] {
            emit(&format!(
                "def f(x: Int, flag: Boolean, a: {ty}, b: {ty}): {ty} = {body}"
            ));
        }
    }
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
fn floating_match_frames_and_opcodes_keep_exact_kinds_and_slot_widths() {
    use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
    let pair = emit(
        "def f(x: Int, a: Float, b: Float): Float = x match { case 0 | 1 => a; case _ => b }; def d(x: Int, a: Double, b: Double): Double = x match { case 0 | 1 => a; case _ => b }; def returned(x: Int, a: Double, b: Double): Double = x match { case 0 | 1 => return a; case _ => return b }; def mixed(i: Int, a: Float, n: Long, b: Double, flag: Boolean): Double = if flag then b else -0.0",
    );
    let file = ClassFile::decode(&mut Reader::new(pair.classfile_bytes())).unwrap();
    for (name, descriptor, slots, bytes, expected_frames) in [
        (
            "f",
            "(IFF)F",
            5,
            vec![
                0x1b, 0x36, 4, 0x15, 4, 3, 0x9f, 0, 9, 0x15, 4, 4, 0xa0, 0, 7, 0x24, 0xa7, 0, 4,
                0x25, 0xae,
            ],
            vec![
                (15, vec![7, 1, 2, 2, 1], vec![]),
                (19, vec![7, 1, 2, 2, 1], vec![]),
                (20, vec![7, 1, 2, 2, 1], vec![2]),
            ],
        ),
        (
            "d",
            "(IDD)D",
            7,
            vec![
                0x1b, 0x36, 6, 0x15, 6, 3, 0x9f, 0, 9, 0x15, 6, 4, 0xa0, 0, 7, 0x28, 0xa7, 0, 5,
                0x18, 4, 0xaf,
            ],
            vec![
                (15, vec![7, 1, 3, 3, 1], vec![]),
                (19, vec![7, 1, 3, 3, 1], vec![]),
                (21, vec![7, 1, 3, 3, 1], vec![3]),
            ],
        ),
        (
            "returned",
            "(IDD)D",
            7,
            vec![
                0x1b, 0x36, 6, 0x15, 6, 3, 0x9f, 0, 9, 0x15, 6, 4, 0xa0, 0, 5, 0x28, 0xaf, 0x18, 4,
                0xaf,
            ],
            vec![
                (15, vec![7, 1, 3, 3, 1], vec![]),
                (17, vec![7, 1, 3, 3, 1], vec![]),
            ],
        ),
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
        assert_eq!((code.max_stack, code.max_locals), (2, slots));
        assert_eq!(code.code, bytes);
        assert_eq!(frames(&file, name), expected_frames);
    }
    let mixed = frames(&file, "mixed");
    assert!(
        mixed
            .iter()
            .all(|(_, locals, _)| locals == &[7, 1, 2, 4, 3, 1])
    );
    assert!(mixed.iter().any(|(_, _, stack)| stack == &[3]));
}

#[test]
fn pinned_scala_consumes_and_recompiles_original_rust_floating_tasty() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_floating_values/FloatingValues.scala"),
        "FloatingValues.scala",
        SourceId::from_index(2475),
    )
    .unwrap();
    let directory =
        std::env::temp_dir().join(format!("dotty-floating-value-scala-{}", std::process::id()));
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
            "/tests/fixtures/jvm_tasty_floating_values/run.py"
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
            "mandatory floating-value runtime gate requires JDK 21+"
        );
    }
    available
}

fn run_java(pair: &dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts, name: &str, source: &str) {
    let directory = std::env::temp_dir().join(format!(
        "dotty-floating-values-{}-{name}",
        std::process::id()
    ));
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
        "floating value oracle ok"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn original_rust_floating_values_execute_on_jdk21() {
    if !jdk_available() {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_floating_values/FloatingValues.scala"),
        "FloatingValues.scala",
        SourceId::from_index(2475),
    )
    .unwrap();
    run_java(
        &pair,
        "JavaConsumer",
        include_str!("fixtures/jvm_tasty_floating_values/JavaConsumer.java"),
    );
}

#[test]
fn floating_artifacts_are_deterministic_across_fresh_sessions() {
    let source = include_str!("fixtures/jvm_tasty_floating_values/FloatingValues.scala");
    let first =
        emit_paired_jvm_tasty_class(source, "FloatingValues.scala", SourceId::from_index(2475))
            .unwrap();
    let second =
        emit_paired_jvm_tasty_class(source, "FloatingValues.scala", SourceId::from_index(2476))
            .unwrap();
    assert_eq!(first.classfile_bytes(), second.classfile_bytes());
    assert_eq!(first.tasty_bytes(), second.tasty_bytes());
}

#[test]
fn unsupported_floating_shapes_never_publish_a_pair() {
    for method in [
        "private val x: Float = 1.0f; def f(): Float = x",
        "private var x: Double = 1.0; def f(): Double = x",
        "def other(): Float = 1.0f; def f(): Float = other()",
        "def other(x: Double): Int = 1; def f(x: Double): Int = other(x)",
        "def f(x: Float): Float = x + 1.0f",
        "def f(x: Double): Boolean = x < 1.0",
        "def f(x: Float): Double = x",
        "def f(x: Float): Int = { x; 1 }",
        "def f(x: Double): Int = { x; 1 }",
        "def f(x: Double): Double = { val saved: Double = x; saved = x; saved }",
        "def f(x: Float): Float = { val saved: Float = return x; saved }",
        "def f(x: Double)(y: Double): Double = x",
        "def f[A](x: Float): Float = x",
        "def f(x: Double = 1.0): Double = x",
        "def f(x: Float)(using n: Int): Float = x",
    ] {
        let source = format!("package example; class FloatingValues {{ {method} }}");
        let error = emit_paired_jvm_tasty_class(
            &source,
            "FloatingValues.scala",
            SourceId::from_index(2475),
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                dotty::jvm_tasty_artifacts::PairedArtifactError::Plan(_)
            ),
            "{method}: {error:?}"
        );
    }
    let source = "package example; class FloatingValues(x: Double) { def f(): Int = 1 }";
    assert!(
        emit_paired_jvm_tasty_class(source, "FloatingValues.scala", SourceId::from_index(2475))
            .is_err()
    );
}

#[test]
fn legacy_numeric_method_plans_respect_validation_and_encoding_depth() {
    use dotty::core::ast::TreeKind;
    use dotty_tasty_pickler::{
        ClassMemberPicklePlan, DefaultConstructorPlan, LimitKind, MethodPicklePlan, PickleError,
        PicklePlan, ValidationLimits,
    };
    for ty in ["Float", "Double", "Long"] {
        let source = format!(
            "package example; class FloatingValues {{ def f(flag: Boolean, x: {ty}): {ty} = if flag then {{ {{ {{ x }} }} }} else x }}"
        );
        dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
            &source,
            "FloatingValues.scala",
            SourceId::from_index(2475),
            |paired| {
                let raw = paired.tasty_plan.plan();
                let [
                    ClassMemberPicklePlan::Method {
                        symbol,
                        source_tree,
                    },
                ] = raw.members
                else {
                    panic!()
                };
                let TreeKind::DefDef(def) = &raw.source_ast.get(*source_tree).kind else {
                    panic!()
                };
                let methods = [MethodPicklePlan {
                    symbol: *symbol,
                    source_tree: *source_tree,
                    parameter_clauses: def
                        .value_param_clauses
                        .iter()
                        .map(|clause| {
                            clause
                                .iter()
                                .map(|&t| raw.source_index.symbol_at(raw.source.id, t).unwrap())
                                .collect()
                        })
                        .collect(),
                    body: Some(
                        raw.typed_bindings
                            .typed_at(raw.source.id, def.rhs.unwrap())
                            .unwrap(),
                    ),
                }];
                let input = PicklePlan {
                    store: raw.store,
                    definitions: raw.definitions,
                    typed_ast: raw.typed_ast,
                    source_ast: raw.source_ast,
                    source_index: raw.source_index,
                    typed_bindings: raw.typed_bindings,
                    source: raw.source.clone(),
                    source_root: raw
                        .source_ast
                        .iter()
                        .find(|(_, n)| matches!(n.kind, TreeKind::PackageDef(_)))
                        .unwrap()
                        .0,
                    class: raw.class,
                    class_tree: raw.class_tree,
                    constructor: DefaultConstructorPlan {
                        symbol: raw.constructor.symbol,
                        source_tree: raw.constructor.source_tree,
                        parameter_clauses: raw.constructor.parameter_clauses.clone(),
                    },
                    methods: &methods,
                    local_values: &[],
                };
                let checkpoint = raw.store.checkpoint();
                let validated = input.validate().unwrap();
                validated.pickle().unwrap();
                let encoded = validated
                    .pickle_with_limits(dotty_tasty_pickler::PickleLimits {
                        projection: dotty_tasty_pickler::ProjectionLimits {
                            max_expression_depth: 0,
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .unwrap_err();
                assert!(
                    matches!(
                        encoded,
                        PickleError::LimitExceeded {
                            kind: LimitKind::ExpressionDepth,
                            limit: 0
                        }
                    ),
                    "{encoded:?}"
                );
                let error = input
                    .validate_with_limits(ValidationLimits {
                        max_expression_depth: 3,
                        ..ValidationLimits::default()
                    })
                    .err()
                    .unwrap();
                assert!(
                    matches!(
                        error,
                        PickleError::LimitExceeded {
                            kind: LimitKind::ExpressionDepth,
                            limit: 3
                        }
                    ),
                    "{error:?}"
                );
                assert_eq!(raw.store.checkpoint(), checkpoint);
            },
        )
        .unwrap();
    }
}

#[path = "support/floating_selector_matches.rs"]
mod floating_selector_matches;
