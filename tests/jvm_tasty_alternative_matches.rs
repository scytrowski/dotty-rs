#[test]
fn pinned_semantic_oracle_preserves_alternative_structure_and_returns() {
    let oracle = include_str!("fixtures/jvm_tasty_alternative_matches/oracle.txt");
    for shape in [
        "(-2147483648 | 0 | 2147483647)",
        "(0 | 0)",
        "((0 | 1) | 2)",
        "scala.Long",
        "return",
    ] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}

use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class;

fn emit(methods: &str) -> dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class AlternativeMatches {{ {methods} }}"),
        "AlternativeMatches.scala",
        SourceId::from_index(2474),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn tasty_alternatives_preserve_nested_structure_and_literal_order() {
    use dotty_tasty::tasty::*;
    let pair = emit(
        "def f(x: Int): Int = x match { case (-2147483648 | 0) | 2147483647 => 10; case _ => 20 }",
    );
    let file = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    let index = file.ast_address_index().unwrap();
    let mut alternatives = index.iter_nodes_with_tag(ALTERNATIVE_TAG);
    let root = alternatives.next().unwrap();
    let RawTree::LengthNode(node) =
        RawTree::decode(&mut Reader::new(&payload[root.offset..])).unwrap()
    else {
        panic!()
    };
    let outer = node.decode_alternative().unwrap();
    assert_eq!(outer.alternatives.len(), 2);
    let RawTree::LengthNode(inner) = &outer.alternatives[0] else {
        panic!()
    };
    assert_eq!(inner.tag, ALTERNATIVE_TAG);
    let inner = inner.decode_alternative().unwrap();
    let values: Vec<_> = inner
        .alternatives
        .iter()
        .chain([&outer.alternatives[1]])
        .map(|v| {
            let RawTree::Leaf(v) = v else { panic!() };
            assert_eq!(v.tag, INTCONST_TAG);
            let TermValue::Int(value) = v.value else {
                panic!()
            };
            value
        })
        .collect();
    assert_eq!(values, [i32::MIN, 0, i32::MAX]);
    assert_eq!(alternatives.count(), 1);
}

#[test]
fn pinned_scala_consumes_and_recompiles_original_rust_alternative_tasty() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_alternative_matches/AlternativeMatches.scala"),
        "AlternativeMatches.scala",
        SourceId::from_index(2474),
    )
    .unwrap();
    let directory = std::env::temp_dir().join(format!(
        "dotty-alternative-match-scala-{}",
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
            "/tests/fixtures/jvm_tasty_alternative_matches/run.py"
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
            "mandatory alternative-match runtime gate requires JDK 21+"
        );
    }
    available
}

fn run_java(pair: &dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts, name: &str, source: &str) {
    let directory = std::env::temp_dir().join(format!(
        "dotty-alternative-matchs-{}-{name}",
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
        "alternative match oracle ok"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn original_rust_alternative_matches_execute_on_jdk21() {
    if !jdk_available() {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_alternative_matches/AlternativeMatches.scala"),
        "AlternativeMatches.scala",
        SourceId::from_index(2474),
    )
    .unwrap();
    run_java(
        &pair,
        "JavaConsumer",
        include_str!("fixtures/jvm_tasty_alternative_matches/JavaConsumer.java"),
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
fn long_alternative_joins_and_all_returning_frames_preserve_two_slot_values() {
    use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
    let pair = emit(
        "def choose(x: Int, a: Long, b: Long): Long = x match { case 0 | 1 => a; case _ => b }; def allReturn(x: Int, a: Long, b: Long): Long = x match { case 0 | 1 => return a; case _ => return b }",
    );
    let file = ClassFile::decode(&mut Reader::new(pair.classfile_bytes())).unwrap();
    for name in ["choose", "allReturn"] {
        let method = file
            .methods
            .iter()
            .find(|m| file.constant_pool.utf8(m.name_index) == Ok(name))
            .unwrap();
        assert_eq!(
            file.constant_pool.utf8(method.descriptor_index),
            Ok("(IJJ)J")
        );
        let [Attribute::Code(code)] = method.attributes.as_slice() else {
            panic!()
        };
        assert_eq!((code.max_stack, code.max_locals), (2, 7));
        let locals = vec![7, 1, 4, 4, 1];
        if name == "choose" {
            assert_eq!(
                code.code,
                [
                    0x1b, 0x36, 6, 0x15, 6, 3, 0x9f, 0, 9, 0x15, 6, 4, 0xa0, 0, 7, 0x20, 0xa7, 0,
                    5, 0x16, 4, 0xad
                ]
            );
            assert_eq!(
                frames(&file, name),
                [
                    (15, locals.clone(), vec![]),
                    (19, locals.clone(), vec![]),
                    (21, locals, vec![4])
                ]
            );
        } else {
            assert_eq!(
                code.code,
                [
                    0x1b, 0x36, 6, 0x15, 6, 3, 0x9f, 0, 9, 0x15, 6, 4, 0xa0, 0, 5, 0x20, 0xad,
                    0x16, 4, 0xad
                ]
            );
            assert_eq!(
                frames(&file, name),
                [(15, locals.clone(), vec![]), (17, locals, vec![])]
            );
        }
    }
}

#[test]
fn alternative_artifacts_are_deterministic_across_fresh_sessions() {
    let source = include_str!("fixtures/jvm_tasty_alternative_matches/AlternativeMatches.scala");
    let first = emit_paired_jvm_tasty_class(
        source,
        "AlternativeMatches.scala",
        SourceId::from_index(2474),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_class(
        source,
        "AlternativeMatches.scala",
        SourceId::from_index(2475),
    )
    .unwrap();
    assert_eq!(first.classfile_bytes(), second.classfile_bytes());
    assert_eq!(first.tasty_bytes(), second.tasty_bytes());
}

#[test]
fn unsupported_alternative_shapes_never_publish_an_artifact_pair() {
    use dotty::jvm_tasty_artifacts::PairedArtifactError;
    for method in [
        "def f(x: Int): Int = x match { case 0 | _ => 1; case _ => 2 }",
        "def f(x: Int): Int = x match { case n @ (0 | 1) => n; case _ => 2 }",
        "def f(x: Int): Int = x match { case n | m => 1; case _ => 2 }",
        "def f(x: Int): Int = x match { case (0: Int) | 1 => 1; case _ => 2 }",
        "def f(x: Int): Int = x match { case 0 | 1 => 1 }",
        "def f(x: Int, flag: Boolean): Int = x match { case 0 | 1 if flag => 1 }",
        "def f(x: Long): Int = x match { case 0L | 1L => 1; case _ => 2 }",
        "def f(x: Boolean): Int = x match { case true | false => 1 }",
    ] {
        let source = format!("package example; class AlternativeMatches {{ {method} }}");
        let error = emit_paired_jvm_tasty_class(
            &source,
            "AlternativeMatches.scala",
            SourceId::from_index(2474),
        )
        .unwrap_err();
        assert!(
            matches!(error, PairedArtifactError::Plan(_)),
            "{method}: {error:?}"
        );
    }
}

#[test]
fn forged_alternative_edges_values_and_types_are_rejected_read_only() {
    use dotty::core::{
        Constant, Typed,
        ast::{AstArena, TreeKind},
    };
    use dotty_tasty_pickler::{
        ClassStatePicklePlan, LimitKind, PickleError, PrimaryConstructorInputPlan,
    };
    let source = "package example; class AlternativeMatches { def f(x: Int): Int = x match { case 0 | 1 => 10; case _ => 20 } }";
    for mode in 0..10 {
        dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
            source,
            "AlternativeMatches.scala",
            SourceId::from_index(2474),
            |plan| {
                let raw = plan.tasty_plan.plan();
                let checkpoint = raw.store.checkpoint();
                let mut ast = AstArena::<Typed>::new();
                for (id, node) in raw.typed_ast.iter() {
                    assert_eq!(id, ast.alloc(node.clone()));
                }
                let id = ast
                    .iter()
                    .find(|(_, n)| matches!(n.kind, TreeKind::Alternative(_)))
                    .unwrap()
                    .0;
                let TreeKind::Alternative(alt) = ast.get(id).kind.clone() else {
                    panic!()
                };
                let leaf = alt.alternatives[0];
                match mode {
                    0 | 1 | 2 | 8 | 9 => {
                        let TreeKind::Alternative(alt) = &mut ast.get_mut(id).kind else {
                            panic!()
                        };
                        match mode {
                            0 => alt.alternatives.clear(),
                            1 => alt.alternatives.reverse(),
                            2 => alt.alternatives[1] = alt.alternatives[0],
                            8 => alt.alternatives[0] = id,
                            _ => alt.alternatives = vec![leaf; 1_000_001],
                        }
                    }
                    3 => {
                        ast.get_mut(leaf).kind = TreeKind::Literal(dotty::core::ast::Literal {
                            value: Constant::Boolean(false),
                        });
                        ast.get_mut(leaf).ty = raw.definitions.boolean;
                    }
                    4 => {
                        ast.get_mut(leaf).kind = ast.get(alt.alternatives[1]).kind.clone();
                        ast.get_mut(leaf).ty = ast.get(alt.alternatives[1]).ty;
                    }
                    5 => ast.get_mut(leaf).ty = raw.definitions.boolean,
                    6 => ast.get_mut(id).ty = raw.definitions.nothing_type,
                    7 => ast.get_mut(id).position = ast.get(leaf).position,
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
                    .and_then(|p| p.validate_method_bodies())
                    .unwrap_err();
                match mode {
                    0 => assert!(
                        matches!(error, PickleError::SourceShape { .. }),
                        "{mode}: {error:?}"
                    ),
                    4 => assert!(
                        matches!(
                            error,
                            PickleError::BodySourceValue {
                                expected: 0,
                                actual: 1,
                                ..
                            }
                        ),
                        "{mode}: {error:?}"
                    ),
                    1 | 2 | 8 => assert!(
                        matches!(error, PickleError::BodyBinding { .. }),
                        "{mode}: {error:?}"
                    ),
                    3 => assert!(
                        matches!(error, PickleError::BodyType { .. }),
                        "{mode}: {error:?}"
                    ),
                    5 | 6 => assert!(
                        matches!(error, PickleError::BodyType { .. }),
                        "{mode}: {error:?}"
                    ),
                    7 => assert!(
                        matches!(error, PickleError::BodySpan { .. }),
                        "{mode}: {error:?}"
                    ),
                    9 => assert!(
                        matches!(
                            error,
                            PickleError::LimitExceeded {
                                kind: LimitKind::Items,
                                limit: 1_000_000
                            }
                        ),
                        "{mode}: {error:?}"
                    ),
                    _ => unreachable!(),
                }
                assert_eq!(raw.store.checkpoint(), checkpoint);
            },
        )
        .unwrap();
    }
}

#[test]
fn alternative_patterns_compose_with_existing_primitive_flow_and_case_scopes() {
    for method in [
        "def f(x: Int, flag: Boolean): Int = x match { case 0 | 1 if flag && x >= 0 => 10; case n => n }",
        "def f(x: Int): Boolean = x match { case (0 | (1 | 2)) => return true; case _ => return false }",
        "def f(x: Int): Unit = x match { case 0 | 1 => return (); case _ => () }",
        "def f(x: Int, a: Long, b: Long): Long = x match { case 0 | 0 => { val saved: Long = a; saved }; case _ => b }",
        "def f(x: Int, a: Long, b: Long): Long = x match { case 0 | 1 => { return a; b }; case _ => b }",
        "def f(x: Int): Int = x match { case 0 | 1 => { val saved: Int = 10; saved }; case n if n < 0 => n; case _ => 20 }",
        "def f(x: Int): Int = x match { case n if n < 0 => n match { case -1 | -2 => n; case _ => 0 }; case 0 | 1 => 1; case _ => 2 }",
    ] {
        emit(method);
    }
}

#[test]
fn legacy_method_plan_alternatives_respect_explicit_validation_depth() {
    use dotty::core::ast::TreeKind;
    use dotty_tasty_pickler::{
        ClassMemberPicklePlan, DefaultConstructorPlan, LimitKind, MethodPicklePlan, PickleError,
        PicklePlan, ValidationLimits,
    };
    let source = "package example; class AlternativeMatches { def f(x: Int): Int = x match { case (0 | 1) | 2 => 10; case _ => 20 } }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
        source,
        "AlternativeMatches.scala",
        SourceId::from_index(2474),
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
            input.validate().unwrap().pickle().unwrap();
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
