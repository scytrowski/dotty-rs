#[test]
fn pinned_scala_oracle_keeps_original_exhaustive_boolean_cases() {
    let oracle = include_str!("fixtures/jvm_tasty_boolean_exhaustive_matches/oracle.txt");
    for shape in [
        "scala.Boolean",
        "case true",
        "case false",
        "case (true | false)",
        "case n @ true",
        "guard_effect",
        "return",
        "scala.Long",
        "scala.Float",
        "scala.Double",
    ] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
    let ordered = oracle
        .split("def ordered(")
        .nth(1)
        .unwrap()
        .split("def reversed(")
        .next()
        .unwrap();
    assert!(!ordered.contains("case _"));
}

use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class;

fn emit(methods: &str) -> dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class BooleanExhaustiveMatches {{ {methods} }}"),
        "BooleanExhaustiveMatches.scala",
        SourceId::from_index(2478),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn pinned_scala_consumes_and_recompiles_original_rust_boolean_exhaustive_match_tasty() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!(
            "fixtures/jvm_tasty_boolean_exhaustive_matches/BooleanExhaustiveMatches.scala"
        ),
        "BooleanExhaustiveMatches.scala",
        SourceId::from_index(2477),
    )
    .unwrap();
    let directory = std::env::temp_dir().join(format!(
        "dotty-boolean-exhaustive-match-scala-{}",
        std::process::id()
    ));
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
            "/tests/fixtures/jvm_tasty_boolean_exhaustive_matches/run.py"
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
            "mandatory boolean-exhaustive-match runtime gate requires JDK 21+"
        );
    }
    available
}

fn run_java(pair: &dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts, name: &str, source: &str) {
    let directory = std::env::temp_dir().join(format!(
        "dotty-boolean-exhaustive-matchs-{}-{name}",
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
        "exhaustive boolean match oracle ok"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn original_rust_boolean_exhaustive_matches_execute_on_jdk21() {
    if !jdk_available() {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!(
            "fixtures/jvm_tasty_boolean_exhaustive_matches/BooleanExhaustiveMatches.scala"
        ),
        "BooleanExhaustiveMatches.scala",
        SourceId::from_index(2477),
    )
    .unwrap();
    run_java(
        &pair,
        "JavaConsumer",
        include_str!("fixtures/jvm_tasty_boolean_exhaustive_matches/JavaConsumer.java"),
    );
}

#[test]
fn both_boolean_orders_literal_alternatives_and_binders_emit_complete_pairs() {
    for method in [
        "def f(x: Boolean): Int = x match { case true => 1; case false => 2 }",
        "def f(x: Boolean): Int = x match { case false => 2; case true => 1 }",
        "def f(x: Boolean): Int = x match { case true | false => 1 }",
        "def f(x: Boolean): Int = x match { case ((true | true) | false) => 1 }",
        "def f(x: Boolean): Boolean = x match { case n @ true => n; case n @ false => n }",
    ] {
        emit(method);
    }
}

#[test]
fn exhaustive_boolean_case_binders_keep_distinct_direct_targets() {
    use dotty_tasty::tasty::*;
    let output =
        emit("def f(x: Boolean): Boolean = x match { case n @ true => n; case n @ false => n }");
    let file = TastyFile::parse_and_validate_scala_3_9(output.tasty_bytes()).unwrap();
    let index = file.ast_address_index().unwrap();
    let binds: Vec<_> = index
        .iter_nodes_with_tag(BIND_TAG)
        .map(|n| n.offset as u32)
        .collect();
    assert_eq!(binds.len(), 2);
    assert_ne!(binds[0], binds[1]);
    let refs: Vec<_> = index
        .iter_nodes_with_tag(TERMREFDIRECT_TAG)
        .filter_map(|n| {
            let RawTree::Leaf(SimpleTerm {
                value: TermValue::AstRef(target),
                ..
            }) = RawTree::decode(&mut Reader::new(
                &file.section(StandardSection::Asts).unwrap().payload[n.offset..],
            ))
            .unwrap()
            else {
                return None;
            };
            binds.contains(&target).then_some(target)
        })
        .collect();
    assert_eq!(refs, binds);
}

#[test]
fn partial_guard_only_int_and_singleton_shortcuts_publish_no_artifacts() {
    for method in [
        "def f(x: Boolean): Int = x match { case true => 1 }",
        "def f(x: Boolean): Int = x match { case true | true => 1 }",
        "def f(x: Boolean,g: Boolean): Int = x match { case true if g => 1; case false => 2 }",
        "def f(x: Boolean): Int = x match { case true if true => 1; case false => 2 }",
        "def f(x: Boolean,g: Boolean): Int = x match { case true => 1; case false if g => 2 }",
        "def f(x: Boolean,g: Boolean): Int = x match { case true => 1; case false => 2; case true if g => 3 }",
        "def f(x: Boolean): Int = x match { case _ => 1; case true => 2 }",
        "def f(x: Int): Int = x match { case 0 => 1; case 1 => 2 }",
        "def f(): Int = true match { case true => 1 }",
        "def f(x: Boolean): Boolean = x match { case n @ (true | false) => n }",
    ] {
        assert!(
            emit_paired_jvm_tasty_class(
                &format!("package example; class BooleanExhaustiveMatches {{ {method} }}"),
                "BooleanExhaustiveMatches.scala",
                SourceId::from_index(2478),
            )
            .is_err(),
            "unexpected acceptance: {method}"
        );
    }
}

#[test]
fn forged_exhaustive_patterns_guards_joins_and_edges_fail_read_only() {
    use dotty::core::{
        Typed, Untyped,
        ast::{AstArena, TreeKind},
    };
    use dotty_tasty_pickler::{
        ClassStatePicklePlan, LimitKind, PickleError, PrimaryConstructorInputPlan,
    };
    let source = "package example; class BooleanExhaustiveMatches { def f(x: Boolean,g: Boolean): Int = x match { case true | true if g => 1; case true => 2; case false => 3 } }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
        source,
        "BooleanExhaustiveMatches.scala",
        SourceId::from_index(2478),
        |paired| {
            let raw = paired.tasty_plan.plan();
            let checkpoint = raw.store.checkpoint();
            let before = format!("{:?}{:?}{:?}", raw.store, raw.source_ast, raw.typed_ast);
            for mode in 0..9 {
                let mut ast = AstArena::<Typed>::new();
                for (id, n) in raw.typed_ast.iter() {
                    assert_eq!(id, ast.alloc(n.clone()));
                }
                let mut source_ast = AstArena::<Untyped>::new();
                for (id, n) in raw.source_ast.iter() {
                    assert_eq!(id, source_ast.alloc(n.clone()));
                }
                let root = paired.jvm_unit.classes[0].methods[0].body.unwrap();
                let TreeKind::Match(m) = ast.get(root).kind.clone() else {
                    panic!()
                };
                let TreeKind::CaseDef(first) = ast.get(m.cases[0]).kind else {
                    panic!()
                };
                let TreeKind::CaseDef(middle) = ast.get(m.cases[1]).kind else {
                    panic!()
                };
                let TreeKind::CaseDef(last) = ast.get(m.cases[2]).kind else {
                    panic!()
                };
                match mode {
                    0 => {
                        ast.get_mut(last.pattern).kind = ast.get(middle.pattern).kind.clone();
                        ast.get_mut(last.pattern).ty = ast.get(middle.pattern).ty;
                    }
                    1 => ast.get_mut(last.pattern).ty = raw.definitions.int,
                    2 => ast.get_mut(m.selector).ty = raw.definitions.int,
                    3 => ast.get_mut(root).ty = raw.definitions.nothing_type,
                    4 => {
                        let TreeKind::Match(m) = &mut ast.get_mut(root).kind else {
                            panic!()
                        };
                        m.cases.pop();
                        let id = source_ast
                            .iter()
                            .find_map(|(id, n)| matches!(n.kind, TreeKind::Match(_)).then_some(id))
                            .unwrap();
                        let TreeKind::Match(m) = &mut source_ast.get_mut(id).kind else {
                            panic!()
                        };
                        m.cases.pop();
                    }
                    5 => {
                        let TreeKind::CaseDef(c) = &mut ast.get_mut(m.cases[2]).kind else {
                            panic!()
                        };
                        c.guard = first.guard;
                    }
                    6 | 7 => {
                        let count = if mode == 6 {
                            1
                        } else {
                            dotty_tasty_pickler::ValidationLimits::default().max_items + 1
                        };
                        let TreeKind::Alternative(a) = &mut ast.get_mut(first.pattern).kind else {
                            panic!()
                        };
                        a.alternatives = vec![first.pattern; count];
                        let pattern = source_ast
                            .iter()
                            .find_map(|(id, n)| {
                                matches!(n.kind, TreeKind::Alternative(_)).then_some(id)
                            })
                            .unwrap();
                        let TreeKind::Alternative(a) = &mut source_ast.get_mut(pattern).kind else {
                            panic!()
                        };
                        a.alternatives = vec![pattern; count];
                    }
                    8 => ast.get_mut(first.guard.unwrap()).ty = raw.definitions.int,
                    _ => unreachable!(),
                }
                let input = ClassStatePicklePlan {
                    store: raw.store,
                    definitions: raw.definitions,
                    source_ast: &source_ast,
                    typed_ast: &ast,
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
                let tasty = input
                    .validate()
                    .and_then(|p| p.validate_method_bodies())
                    .unwrap_err();
                let expected = match mode {
                    0 | 4 | 5 => matches!(tasty, PickleError::Unsupported { .. }),
                    1 | 2 | 3 | 8 => matches!(tasty, PickleError::BodyType { .. }),
                    6 => matches!(tasty, PickleError::RepeatedExpression { .. }),
                    7 => matches!(
                        tasty,
                        PickleError::LimitExceeded {
                            kind: LimitKind::Items,
                            ..
                        }
                    ),
                    _ => false,
                };
                assert!(expected, "mode {mode}: {tasty:?}");
                let unit = dotty::jvm::JvmCompilationUnit {
                    store: raw.store,
                    typed_ast: &ast,
                    classes: paired.jvm_unit.classes.clone(),
                };
                let jvm = dotty::jvm::emit_class(&unit, raw.class, raw.definitions).unwrap_err();
                use dotty::jvm::JvmCodegenError;
                let expected = match mode {
                    0 | 4 | 5 => matches!(
                        jvm,
                        JvmCodegenError::UnsupportedTypedExpression {
                            node: "match requires exactly one final wildcard",
                            ..
                        }
                    ),
                    1 => matches!(
                        jvm,
                        JvmCodegenError::UnsupportedTypedExpression {
                            node: "inconsistent Boolean match literal",
                            ..
                        }
                    ),
                    2 => matches!(
                        jvm,
                        JvmCodegenError::UnsupportedTypedExpression {
                            node: "invalid Int match alternative",
                            ..
                        }
                    ),
                    3 => matches!(
                        jvm,
                        JvmCodegenError::UnsupportedTypedExpression {
                            node: "match case result mismatch",
                            ..
                        }
                    ),
                    6 => matches!(jvm, JvmCodegenError::ExpressionDepthExceeded { .. }),
                    7 => matches!(jvm, JvmCodegenError::ExpressionNodeBudgetExceeded { .. }),
                    8 => {
                        matches!(
                            jvm,
                            JvmCodegenError::UnsupportedTypedExpression {
                                node: "match guard flow/type mismatch" | "non-Boolean match guard",
                                ..
                            }
                        ) || matches!(jvm, JvmCodegenError::InvalidParameterReference { .. })
                    }
                    _ => false,
                };
                assert!(expected, "mode {mode}: {jvm:?}");
                assert_eq!(raw.store.checkpoint(), checkpoint);
                assert_eq!(
                    format!("{:?}{:?}{:?}", raw.store, raw.source_ast, raw.typed_ast),
                    before
                );
            }
        },
    )
    .unwrap();
}

#[test]
fn exhaustive_coverage_does_not_authorize_a_foreign_case_binder_read() {
    use dotty::core::{
        Typed,
        ast::{AstArena, TreeKind},
    };
    use dotty_tasty_pickler::{ClassStatePicklePlan, PickleError, PrimaryConstructorInputPlan};
    let source = "package example; class BooleanExhaustiveMatches { def f(x: Boolean): Boolean = x match { case n @ true => n; case n @ false => n } }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
        source,
        "BooleanExhaustiveMatches.scala",
        SourceId::from_index(2478),
        |paired| {
            let raw = paired.tasty_plan.plan();
            let mut ast = AstArena::<Typed>::new();
            for (id, n) in raw.typed_ast.iter() {
                assert_eq!(id, ast.alloc(n.clone()));
            }
            let root = paired.jvm_unit.classes[0].methods[0].body.unwrap();
            let TreeKind::Match(m) = ast.get(root).kind.clone() else {
                panic!()
            };
            let TreeKind::CaseDef(first) = ast.get(m.cases[0]).kind else {
                panic!()
            };
            let TreeKind::CaseDef(last) = ast.get(m.cases[1]).kind else {
                panic!()
            };
            let read = |mut id| {
                loop {
                    match &ast.get(id).kind {
                        TreeKind::Typed(t) => id = t.expr,
                        TreeKind::Block(b) if b.stats.is_empty() => id = b.expr,
                        TreeKind::Ident(_) => break id,
                        other => panic!("unexpected binder body: {other:?}"),
                    }
                }
            };
            let first_read = read(first.body);
            let foreign_ty = ast.get(first_read).ty;
            let mut id = last.body;
            loop {
                ast.get_mut(id).ty = foreign_ty;
                match &ast.get(id).kind {
                    TreeKind::Typed(t) => id = t.expr,
                    TreeKind::Block(b) if b.stats.is_empty() => id = b.expr,
                    TreeKind::Ident(_) => break,
                    other => panic!("unexpected binder body: {other:?}"),
                }
            }
            ast.get_mut(m.cases[1]).ty = foreign_ty;
            let checkpoint = raw.store.checkpoint();
            let input = ClassStatePicklePlan {
                store: raw.store,
                definitions: raw.definitions,
                source_ast: raw.source_ast,
                typed_ast: &ast,
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
            let tasty = input
                .validate()
                .and_then(|p| p.validate_method_bodies())
                .unwrap_err();
            assert!(
                matches!(
                    tasty,
                    PickleError::Reference {
                        reason: dotty_tasty_pickler::ReferenceError::NotRegistered,
                        ..
                    }
                ),
                "{tasty:?}"
            );
            let unit = dotty::jvm::JvmCompilationUnit {
                store: raw.store,
                typed_ast: &ast,
                classes: paired.jvm_unit.classes.clone(),
            };
            assert!(matches!(
                dotty::jvm::emit_class(&unit, raw.class, raw.definitions).unwrap_err(),
                dotty::jvm::JvmCodegenError::UninitializedLocalRead { .. }
            ));
            assert_eq!(raw.store.checkpoint(), checkpoint);
        },
    )
    .unwrap();
}

#[test]
fn exhaustive_normal_mixed_and_all_returning_primitive_joins_compose() {
    for ty in ["Int", "Boolean", "Long", "Float", "Double"] {
        for (left, right) in [("a", "b"), ("return a", "b"), ("return a", "return b")] {
            emit(&format!(
                "def f(x: Boolean,a: {ty},b: {ty}): {ty} = x match {{ case true => {left}; case false => {right} }}"
            ));
        }
    }
    emit("def f(x: Boolean): Unit = x match { case true => return (); case false => return () }");
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
fn exhaustive_numeric_joins_have_exact_descriptors_instructions_and_frames() {
    use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
    for (ty, descriptor, slots, load_a, load_b, ret, tag) in [
        ("Int", "(ZII)I", 5, 0x1c, vec![0x1d], 0xac, 1),
        ("Boolean", "(ZZZ)Z", 5, 0x1c, vec![0x1d], 0xac, 1),
        ("Long", "(ZJJ)J", 7, 0x20, vec![0x16, 4], 0xad, 4),
        ("Float", "(ZFF)F", 5, 0x24, vec![0x25], 0xae, 2),
        ("Double", "(ZDD)D", 7, 0x28, vec![0x18, 4], 0xaf, 3),
    ] {
        let pair = emit(&format!(
            "def f(x: Boolean,a: {ty},b: {ty}): {ty} = x match {{ case true => a; case false => b }}"
        ));
        let file = ClassFile::decode(&mut Reader::new(pair.classfile_bytes())).unwrap();
        let method = file
            .methods
            .iter()
            .find(|m| file.constant_pool.utf8(m.name_index) == Ok("f"))
            .unwrap();
        assert_eq!(
            file.constant_pool.utf8(method.descriptor_index),
            Ok(descriptor)
        );
        let [Attribute::Code(code)] = method.attributes.as_slice() else {
            panic!()
        };
        assert_eq!(code.max_locals, slots);
        let mut expected = vec![
            0x1b,
            0x36,
            (slots - 1) as u8,
            0x15,
            (slots - 1) as u8,
            0x04,
            0xa0,
            0,
            7,
            load_a,
            0xa7,
            0,
            (3 + load_b.len()) as u8,
        ];
        expected.extend(load_b);
        expected.push(ret);
        assert_eq!(code.code, expected, "{ty}");
        let locals = vec![7, 1, tag, tag, 1];
        let end = if slots == 7 { 15 } else { 14 };
        assert_eq!(
            frames(&file, "f"),
            [(13, locals.clone(), vec![]), (end, locals, vec![tag])],
            "{ty}"
        );
    }
}
