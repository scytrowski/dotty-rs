#[test]
fn pinned_oracle_records_method_targets_and_unit_returns() {
    let oracle = include_str!("fixtures/jvm_tasty_local_returns/oracle.txt");
    for shape in ["RETURN(", "UNITconst", "[direct]", "[matched]", "[inLoop]"] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}

use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class;

fn emit(methods: &str) -> dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class EarlyReturns {{ {methods} }}"),
        "EarlyReturns.scala",
        SourceId::from_index(2472),
    )
    .unwrap_or_else(|error| panic!("{methods}: {error:?}"))
}

#[test]
fn primitive_returns_target_the_exact_enclosing_method_declaration() {
    let pair = emit(
        "def direct(x: Int): Int = return x; def booleanValue(x: Boolean): Boolean = return x; def unit(): Unit = return",
    );
    let tasty =
        dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
    let index = tasty.ast_address_index().unwrap();
    let methods: Vec<_> = index
        .iter()
        .filter(|node| node.tag == dotty_tasty::tasty::DEFDEF_TAG)
        .collect();
    let returns: Vec<_> = index
        .iter()
        .filter(|node| node.tag == dotty_tasty::tasty::RETURN_TAG)
        .collect();
    assert_eq!(returns.len(), 3);
    for returned in returns {
        let target = returned.decode_return().unwrap().target;
        let enclosing = methods
            .iter()
            .filter(|method| method.offset < returned.offset)
            .max_by_key(|method| method.offset)
            .unwrap();
        assert_eq!(target.address as usize, enclosing.offset);
        assert_eq!(target.kind, dotty_tasty::tasty::AstRefKind::ReturnTarget);
        assert!(returned.decode_return().unwrap().expression.is_some());
    }
}

#[test]
fn returning_branches_compose_with_blocks_loops_and_match_binders() {
    for method in [
        "def f(x: Int): Int = { if x < 0 then return 0; x + 1 }",
        "def f(x: Int): Int = if x < 0 then return 0 else return x",
        "def f(x: Boolean): Boolean = if x then return false else true",
        "def f(x: Boolean): Unit = if x then return else ()",
        "def f(x: Int): Int = x match { case n if n < 0 => return n; case n => n + 1 }",
        "def f(x: Int): Int = { var n: Int = x; while n > 0 do { if n == 3 then return n; n = n - 1 }; 0 }",
        "def f(): Int = { return 1; 2 }",
    ] {
        emit(method);
    }
}

#[test]
fn malformed_return_targets_and_types_are_rejected_read_only() {
    use dotty::core::{
        Typed,
        ast::{AstArena, TreeKind},
    };
    use dotty_tasty_pickler::{ClassStatePicklePlan, PickleError, PrimaryConstructorInputPlan};
    let source = "package example; class EarlyReturns { def direct(x: Int): Int = return x }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
        source,
        "EarlyReturns.scala",
        SourceId::from_index(2472),
        |plan| {
            let raw = plan.tasty_plan.plan();
            let returned = raw
                .typed_ast
                .iter()
                .find(|(_, node)| matches!(node.kind, TreeKind::Return(_)))
                .unwrap()
                .0;
            let checkpoint = raw.store.checkpoint();
            for mode in 0..3 {
                let mut ast = AstArena::<Typed>::new();
                for (id, node) in raw.typed_ast.iter() {
                    assert_eq!(id, ast.alloc(node.clone()));
                }
                let node = ast.get_mut(returned);
                let TreeKind::Return(value) = &mut node.kind else {
                    panic!()
                };
                match mode {
                    0 => value.from = value.expr,
                    1 => node.ty = raw.definitions.int,
                    2 => value.expr = None,
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
                    .and_then(|checked| checked.validate_method_bodies())
                    .unwrap_err();
                assert!(
                    matches!(error, PickleError::Unsupported { .. }),
                    "{error:?}"
                );
                assert_eq!(checkpoint, raw.store.checkpoint());
            }
        },
    )
    .unwrap();
}

#[test]
fn all_returning_matches_and_unreachable_branch_tails_need_no_value_join() {
    for method in [
        "def f(x: Int): Int = x match { case n if n < 0 => return n; case n => return n + 1 }",
        "def f(flag: Boolean): Int = if flag then { return 7; 99 } else 3",
        "def f(x: Int): Int = x match { case 0 => { return 7; 99 }; case _ => 3 }",
    ] {
        emit(method);
    }
}

#[test]
fn full_fixture_emits_identical_bytes_in_fresh_sessions() {
    let source = include_str!("fixtures/jvm_tasty_local_returns/EarlyReturns.scala");
    let first =
        emit_paired_jvm_tasty_class(source, "EarlyReturns.scala", SourceId::from_index(2472))
            .unwrap();
    let second =
        emit_paired_jvm_tasty_class(source, "EarlyReturns.scala", SourceId::from_index(2473))
            .unwrap();
    assert_eq!(first.classfile_bytes(), second.classfile_bytes());
    assert_eq!(first.tasty_bytes(), second.tasty_bytes());
}

#[test]
fn pinned_scala_reads_and_recompiles_rust_local_return_tasty() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_local_returns/EarlyReturns.scala"),
        "EarlyReturns.scala",
        SourceId::from_index(2470),
    )
    .unwrap();
    let directory =
        std::env::temp_dir().join(format!("dotty-local-return-scala-{}", std::process::id()));
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
            "/tests/fixtures/jvm_tasty_local_returns/run.py"
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

#[test]
fn direct_returns_emit_exact_opcodes_and_all_return_match_has_no_epilogue() {
    use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
    let pair = emit(
        "def direct(x: Int): Int = return x; def booleanValue(x: Boolean): Boolean = return x; def unit(): Unit = return; def f(x: Int): Int = x match { case n if n < 0 => return n; case n => return n + 1 }",
    );
    let file = ClassFile::decode(&mut Reader::new(pair.classfile_bytes())).unwrap();
    for (name, expected) in [
        ("direct", &[0x1b, 0xac][..]),
        ("booleanValue", &[0x1b, 0xac][..]),
        ("unit", &[0xb1][..]),
    ] {
        let method = file
            .methods
            .iter()
            .find(|m| file.constant_pool.utf8(m.name_index) == Ok(name))
            .unwrap();
        let [Attribute::Code(code)] = method.attributes.as_slice() else {
            panic!()
        };
        assert_eq!(code.code, expected);
    }
    let method = file
        .methods
        .iter()
        .find(|m| file.constant_pool.utf8(m.name_index) == Ok("f"))
        .unwrap();
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!()
    };
    assert_eq!(
        code.code,
        &[
            0x1b, 0x36, 4, 0x15, 4, 0x3d, 0x1c, 3, 0xa1, 0, 7, 3, 0xa7, 0, 4, 4, 0x99, 0, 5, 0x1c,
            0xac, 0x15, 4, 0x3e, 0x1d, 4, 0x60, 0xac
        ]
    );
    assert_eq!(code.max_locals, 5);
    assert_eq!(code.code.last(), Some(&0xac));
    assert_eq!(
        code.code.iter().filter(|&&opcode| opcode == 0xac).count(),
        2
    );
}

#[test]
fn nothing_typed_match_with_a_normal_case_is_rejected_by_both_producers() {
    use dotty::core::{
        Typed,
        ast::{AstArena, TreeKind},
    };
    use dotty_tasty_pickler::{ClassStatePicklePlan, PickleError, PrimaryConstructorInputPlan};
    let source = "package example; class EarlyReturns { def f(x: Int): Int = x match { case n if n < 0 => return n; case n => n + 1 } }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(source, "EarlyReturns.scala", SourceId::from_index(2472), |plan| {
        let raw = plan.tasty_plan.plan();
        let root = raw.typed_ast.iter().find(|(_, node)| matches!(node.kind, TreeKind::Match(_))).unwrap().0;
        let mut ast = AstArena::<Typed>::new();
        for (id, node) in raw.typed_ast.iter() { assert_eq!(id, ast.alloc(node.clone())); }
        ast.get_mut(root).ty = raw.definitions.nothing_type;
        let input = ClassStatePicklePlan { store: raw.store, definitions: raw.definitions,
            typed_ast: &ast, source_ast: raw.source_ast, source_index: raw.source_index,
            typed_bindings: raw.typed_bindings, source: raw.source.clone(), class: raw.class, class_tree: raw.class_tree,
            constructor: PrimaryConstructorInputPlan { symbol: raw.constructor.symbol, source_tree: raw.constructor.source_tree,
                parameter_clauses: raw.constructor.parameter_clauses.clone() }, members: raw.members };
        assert!(matches!(input.validate().and_then(|v| v.validate_method_bodies()), Err(PickleError::BodyType { tree, .. }) if tree == root));
        let unit = dotty::jvm::JvmCompilationUnit { store: raw.store, typed_ast: &ast, classes: plan.jvm_unit.classes.clone() };
        let error = dotty::jvm::emit_class(&unit, raw.class, raw.definitions).unwrap_err();
        assert!(matches!(error, dotty::jvm::JvmCodegenError::UnsupportedTypedExpression { node: "match case result mismatch", .. }), "{error:?}");
    }).unwrap();
}

#[test]
fn synthetic_unit_adaptations_and_semantic_joins_cannot_be_forged() {
    use dotty::core::{
        Typed,
        ast::{AstArena, TreeKind},
    };
    use dotty_tasty_pickler::{ClassStatePicklePlan, PickleError, PrimaryConstructorInputPlan};
    for (source, mode) in [
        (
            "package example; class EarlyReturns { def f(x: Int): Int = { if x < 0 then return 0; x } }",
            0,
        ),
        (
            "package example; class EarlyReturns { def f(x: Boolean): Int = if x then return 1 else return 2 }",
            1,
        ),
        (
            "package example; class EarlyReturns { def f(x: Int): Int = x match { case 0 => return 1; case _ => return 2 } }",
            2,
        ),
        (
            "package example; class EarlyReturns { def f(x: Int): Int = return x }",
            3,
        ),
    ] {
        dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
            source,
            "EarlyReturns.scala",
            SourceId::from_index(2472),
            |plan| {
                let raw = plan.tasty_plan.plan();
                let checkpoint = raw.store.checkpoint();
                let mut ast = AstArena::<Typed>::new();
                for (id, node) in raw.typed_ast.iter() {
                    assert_eq!(id, ast.alloc(node.clone()));
                }
                match mode {
                    0 => {
                        let (unit, position) = ast
                            .iter()
                            .find_map(|(_, node)| match &node.kind {
                                TreeKind::Block(block)
                                    if block.stats.len() == 1
                                        && ast.get(block.expr).position.is_none()
                                        && matches!(
                                            ast.get(block.expr).kind,
                                            TreeKind::Literal(_)
                                        ) =>
                                {
                                    Some((block.expr, node.position))
                                }
                                _ => None,
                            })
                            .unwrap();
                        ast.get_mut(unit).position = position;
                    }
                    1 | 2 => {
                        let root = ast
                            .iter()
                            .find(|(_, node)| {
                                if mode == 1 {
                                    matches!(node.kind, TreeKind::If(_))
                                } else {
                                    matches!(node.kind, TreeKind::Match(_))
                                }
                            })
                            .unwrap()
                            .0;
                        ast.get_mut(root).ty = raw.definitions.int;
                    }
                    _ => {
                        let root = ast
                            .iter()
                            .find(|(_, node)| matches!(node.kind, TreeKind::Return(_)))
                            .unwrap()
                            .0;
                        let TreeKind::Return(value) = ast.get(root).kind else {
                            panic!()
                        };
                        ast.get_mut(root).kind = TreeKind::Block(dotty::core::ast::Block {
                            stats: vec![root],
                            expr: value.expr.unwrap(),
                        });
                    }
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
                if mode == 3 {
                    assert!(
                        matches!(error, PickleError::Unsupported { .. }),
                        "{error:?}"
                    );
                } else {
                    assert!(matches!(error, PickleError::BodyType { .. }), "{error:?}");
                }
                assert_eq!(checkpoint, raw.store.checkpoint());
            },
        )
        .unwrap();
    }
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
            "mandatory local-return runtime gate requires JDK 21+"
        );
    }
    available
}

fn run_java(pair: &dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts, name: &str, source: &str) {
    let directory =
        std::env::temp_dir().join(format!("dotty-local-returns-{}-{name}", std::process::id()));
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
        "local return oracle ok"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn original_rust_local_returns_execute_on_jdk21() {
    if !jdk_available() {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_local_returns/EarlyReturns.scala"),
        "EarlyReturns.scala",
        SourceId::from_index(2472),
    )
    .unwrap();
    run_java(
        &pair,
        "JavaConsumer",
        include_str!("fixtures/jvm_tasty_local_returns/JavaConsumer.java"),
    );
}

#[test]
fn returns_outside_the_bounded_positions_or_signatures_are_explicitly_rejected() {
    for methods in [
        "def f(x: Int): Int = { def local(): Int = return x; local() }",
        "def f(x: Int) = return x",
        "def f(x: Int): Int = return false",
        "def f(x: Int): Int = return",
        "def f(x: Int): Int = x match { case n if return 1 => n; case _ => 0 }",
        "def f(x: Int): Int = 1 + (return x)",
    ] {
        let source = format!("package example; class EarlyReturns {{ {methods} }}");
        assert!(
            matches!(
                emit_paired_jvm_tasty_class(
                    &source,
                    "EarlyReturns.scala",
                    SourceId::from_index(2472)
                ),
                Err(dotty::jvm_tasty_artifacts::PairedArtifactError::Plan(_))
            ),
            "{methods}"
        );
    }
}
