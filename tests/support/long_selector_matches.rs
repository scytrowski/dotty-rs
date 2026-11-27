#[path = "guarded_match_plan.rs"]
mod guarded_match_plan;
use guarded_match_plan::with_jvm;

use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::{PairedJvmTastyArtifacts, emit_paired_jvm_tasty_class};

const SOURCE: &str =
    include_str!("../fixtures/jvm_tasty_long_selector_matches/LongSelectorMatches.scala");

fn emit(methods: &str) -> PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class LongSelectorMatches {{ {methods} }}"),
        "LongSelectorMatches.scala",
        SourceId::from_index(2482),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn implicit_and_explicit_typed_binders_keep_pattern_wrappers() {
    use dotty_tasty::tasty::*;
    for ty in ["Long"] {
        for pattern in [
            format!("_: {ty}"),
            format!("n: {ty}"),
            format!("n @ (_: {ty})"),
        ] {
            let pair = emit(&format!(
                "def f(x: {ty}): Int = x match {{ case {pattern} => 7 }}"
            ));
            let file = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
            let index = file.ast_address_index().unwrap();
            assert_eq!(index.iter().filter(|n| n.tag == TYPED_TAG).count(), 1);
            assert_eq!(
                index.iter().filter(|n| n.tag == BIND_TAG).count(),
                usize::from(pattern.starts_with('n'))
            );
            let typed = index
                .iter()
                .find(|n| n.tag == TYPED_TAG)
                .unwrap()
                .decode_typed()
                .unwrap();
            assert!(matches!(
                typed.expression,
                RawTree::NatAst { tag: IDENT_TAG, .. }
            ));
            let RawTree::NatAst { tag, value, .. } = typed.type_tree else {
                panic!()
            };
            assert_eq!(tag, IDENTTPT_TAG);
            assert_eq!(file.render_name(value).unwrap(), ty);
        }
    }
}

#[test]
fn complete_long_selector_fixture_is_deterministic_and_preserves_pattern_nodes() {
    use dotty_tasty::tasty::*;
    let first = emit_paired_jvm_tasty_class(
        SOURCE,
        "LongSelectorMatches.scala",
        SourceId::from_index(2482),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_class(
        SOURCE,
        "LongSelectorMatches.scala",
        SourceId::from_index(2483),
    )
    .unwrap();
    assert_eq!(first.classfile_bytes(), second.classfile_bytes());
    assert_eq!(first.tasty_bytes(), second.tasty_bytes());
    let file = TastyFile::parse_and_validate_scala_3_9(first.tasty_bytes()).unwrap();
    let index = file.ast_address_index().unwrap();
    fn pattern_wrappers(pattern: RawTree<'_>) -> usize {
        match pattern {
            RawTree::LengthNode(node) if node.tag == TYPED_TAG => 1,
            RawTree::LengthNode(node) if node.tag == BIND_TAG => {
                let BindBody::Pattern(child) = node.decode_bind().unwrap().body else {
                    panic!()
                };
                pattern_wrappers(child)
            }
            _ => 0,
        }
    }
    assert_eq!(
        index
            .iter()
            .filter(|n| n.tag == CASEDEF_TAG)
            .map(|n| pattern_wrappers(n.decode_case_def().unwrap().pattern))
            .sum::<usize>(),
        23
    );
    for (tag, count) in [
        (TYPED_TAG, 25),
        (BIND_TAG, 38),
        (MATCH_TAG, 55),
        (RETURN_TAG, 15),
    ] {
        assert_eq!(
            index.iter().filter(|n| n.tag == tag).count(),
            count,
            "tag {tag}"
        );
    }
}

fn publish_fixture(label: &str) -> std::path::PathBuf {
    let pair = emit_paired_jvm_tasty_class(
        SOURCE,
        "LongSelectorMatches.scala",
        SourceId::from_index(2482),
    )
    .unwrap();
    let directory = std::env::temp_dir().join(format!(
        "dotty-long-selector-matches-{label}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(directory.join("example")).unwrap();
    std::fs::write(
        directory.join(pair.classfile_path()),
        pair.classfile_bytes(),
    )
    .unwrap();
    std::fs::write(directory.join(pair.tasty_path()), pair.tasty_bytes()).unwrap();
    directory
}

#[test]
fn long_selector_values_and_effects_execute_on_jdk21() {
    if !super::jdk_available() {
        return;
    }
    let directory = publish_fixture("java");
    let client = directory.join("JavaConsumer.java");
    std::fs::write(
        &client,
        include_str!("../fixtures/jvm_tasty_long_selector_matches/JavaConsumer.java"),
    )
    .unwrap();
    let compile = std::process::Command::new("javac")
        .args(["--release", "21", "-cp"])
        .arg(&directory)
        .arg("-d")
        .arg(&directory)
        .arg(client)
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
        .arg("JavaConsumer")
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "OK 1122 checks"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn pinned_scala_consumes_original_and_recompiled_long_selector_artifacts() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let directory = publish_fixture("scala");
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/jvm_tasty_long_selector_matches/run.py"
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
fn typed_pattern_guards_and_fallbacks_keep_the_existing_ordering_contract() {
    for ty in ["Long"] {
        for cases in [
            format!("case _: {ty} => 1; case _ => 2"),
            format!("case n: {ty} => 1; case _ => 2"),
            format!("case n @ (_: {ty}) => 1; case _ => 2"),
            format!("case _: {ty} if true => 1"),
            format!("case n: {ty} if true => 1"),
        ] {
            let source = format!(
                "package example; class TypedPrimitivePatterns {{ def f(x: {ty}): Int = x match {{ {cases} }} }}"
            );
            with_jvm(&source, |unit, definitions| {
                let error =
                    dotty::jvm::emit_class(unit, unit.classes[0].class, definitions).unwrap_err();
                assert!(
                    matches!(
                        error,
                        dotty::jvm::JvmCodegenError::UnsupportedTypedExpression {
                            node: "match requires exactly one final wildcard",
                            ..
                        }
                    ),
                    "{cases}: {error:?}"
                );
            });
            let error = emit_paired_jvm_tasty_class(
                &source,
                "TypedPrimitivePatterns.scala",
                SourceId::from_index(2480),
            )
            .unwrap_err();
            assert!(
                matches!(
                    error,
                    dotty::jvm_tasty_artifacts::PairedArtifactError::Plan(
                        dotty::jvm_tasty_plan::PairedPlanError::Contract(
                            dotty_jvm_tasty_contract::JvmTastyContractError::TastyPlan(
                                dotty_tasty_pickler::PickleError::Unsupported { .. }
                            )
                        )
                    )
                ),
                "{cases}: {error:?}"
            );
        }
    }
}

#[test]
fn legacy_typed_patterns_obey_validation_and_encoding_limits_read_only() {
    use dotty::core::ast::TreeKind;
    use dotty_tasty_pickler::{
        ClassMemberPicklePlan, DefaultConstructorPlan, LimitKind, MethodPicklePlan, PickleError,
        PickleLimits, PicklePlan, ProjectionLimits, ValidationLimits,
    };
    let source = "package example; class LongSelectorMatches { def f(x: Long): Int = x match { case _: Long => 7 } }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
        source,
        "LongSelectorMatches.scala",
        SourceId::from_index(2482),
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
                parameter_clauses: vec![paired.jvm_unit.classes[0].methods[0].parameters.clone()],
                body: Some(
                    raw.typed_bindings
                        .typed_at(raw.source.id, def.rhs.unwrap())
                        .unwrap(),
                ),
            }];
            let input = PicklePlan {
                store: raw.store,
                definitions: raw.definitions,
                source_ast: raw.source_ast,
                typed_ast: raw.typed_ast,
                source_index: raw.source_index,
                typed_bindings: raw.typed_bindings,
                source: raw.source.clone(),
                source_root: raw
                    .source_ast
                    .iter()
                    .find_map(|(id, n)| matches!(n.kind, TreeKind::PackageDef(_)).then_some(id))
                    .unwrap(),
                class: raw.class,
                class_tree: raw.class_tree,
                methods: &methods,
                local_values: &[],
                constructor: DefaultConstructorPlan {
                    symbol: raw.constructor.symbol,
                    source_tree: raw.constructor.source_tree,
                    parameter_clauses: raw.constructor.parameter_clauses.clone(),
                },
            };
            let before = format!("{:?}{:?}{:?}", raw.store, raw.source_ast, raw.typed_ast);
            let checkpoint = raw.store.checkpoint();
            let valid = input.validate().unwrap();
            let a = valid.pickle().unwrap();
            let b = valid.pickle().unwrap();
            assert_eq!(a.bytes(), b.bytes());
            let validation_depth = input
                .validate_with_limits(ValidationLimits {
                    max_expression_depth: 0,
                    ..Default::default()
                })
                .err()
                .unwrap();
            assert_eq!(
                validation_depth,
                PickleError::LimitExceeded {
                    kind: LimitKind::ExpressionDepth,
                    limit: 0
                }
            );
            let encoding_depth = valid
                .pickle_with_limits(PickleLimits {
                    projection: ProjectionLimits {
                        max_expression_depth: 0,
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .unwrap_err();
            assert_eq!(
                encoding_depth,
                PickleError::LimitExceeded {
                    kind: LimitKind::ExpressionDepth,
                    limit: 0
                }
            );
            let items = input
                .validate_with_limits(ValidationLimits {
                    max_items: 5,
                    ..Default::default()
                })
                .err()
                .unwrap();
            assert_eq!(
                items,
                PickleError::LimitExceeded {
                    kind: LimitKind::Items,
                    limit: 5
                }
            );
            assert_eq!(raw.store.checkpoint(), checkpoint);
            assert_eq!(
                format!("{:?}{:?}{:?}", raw.store, raw.source_ast, raw.typed_ast),
                before
            );
        },
    )
    .unwrap();
}

#[test]
fn typed_patterns_require_the_actual_source_edges_including_implicit_binder_edges() {
    use dotty::core::{SymbolId, TreeId, Typed, Untyped, ast::TreeKind};
    use dotty_tasty_pickler::{
        ClassStatePicklePlan, LocalValueBinding, PickleError, PrimaryConstructorInputPlan,
        SourceTypedBindings,
    };
    struct MissingEdge<'a> {
        original: &'a dyn SourceTypedBindings,
        removed: TreeId<Untyped>,
    }
    impl SourceTypedBindings for MissingEdge<'_> {
        fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
            if tree == self.removed {
                None
            } else {
                self.original.typed_at(source, tree)
            }
        }
        fn local_value_bindings(
            &self,
            source: SourceId,
            method: SymbolId,
        ) -> Vec<LocalValueBinding> {
            self.original.local_value_bindings(source, method)
        }
        fn pattern_value_bindings(
            &self,
            source: SourceId,
            method: SymbolId,
        ) -> Vec<LocalValueBinding> {
            self.original.pattern_value_bindings(source, method)
        }
    }
    for pattern in ["_: Long", "n: Long", "n @ (_: Long)"] {
        let source = format!(
            "package example; class LongSelectorMatches {{ def f(x: Long): Int = x match {{ case {pattern} => 7 }} }}"
        );
        dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(&source, "LongSelectorMatches.scala", SourceId::from_index(2482), |paired| {
            let raw = paired.tasty_plan.plan();
            let (source_root, wrapper) = raw.source_ast.iter().find_map(|(id, node)| {
                if let TreeKind::Typed(w) = node.kind { Some((id, w)) } else { None }
            }).unwrap();
            let before = format!("{:?}{:?}{:?}", raw.store, raw.source_ast, raw.typed_ast);
            for removed in [source_root, wrapper.tpt, wrapper.expr] {
                let bindings = MissingEdge { original: raw.typed_bindings, removed };
                let input = ClassStatePicklePlan {
                    store: raw.store, definitions: raw.definitions, source_ast: raw.source_ast, typed_ast: raw.typed_ast,
                    source_index: raw.source_index, typed_bindings: &bindings, source: raw.source.clone(),
                    class: raw.class, class_tree: raw.class_tree,
                    constructor: PrimaryConstructorInputPlan { symbol: raw.constructor.symbol, source_tree: raw.constructor.source_tree, parameter_clauses: raw.constructor.parameter_clauses.clone() }, members: raw.members,
                };
                let error = input.validate().and_then(|p| p.validate_method_bodies()).unwrap_err();
                assert!(matches!(error, PickleError::BodyBinding { tree, actual: None, .. } if tree == removed), "{pattern} {removed:?}: {error:?}");
                assert_eq!(before, format!("{:?}{:?}{:?}", raw.store, raw.source_ast, raw.typed_ast));
            }
        }).unwrap();
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
fn long_literal_and_typed_binders_have_exact_two_slot_layout_and_join_frames() {
    use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
    let pair = emit(
        "def f(x: Long, enabled: Boolean): Long = x match { case n @ 1L if enabled => n; case n: Long => n }",
    );
    let file = ClassFile::decode(&mut Reader::new(pair.classfile_bytes())).unwrap();
    let method = file
        .methods
        .iter()
        .find(|m| file.constant_pool.utf8(m.name_index) == Ok("f"))
        .unwrap();
    assert_eq!(
        file.constant_pool.utf8(method.descriptor_index),
        Ok("(JZ)J")
    );
    let [Attribute::Code(code)] = method.attributes.as_slice() else {
        panic!()
    };
    assert_eq!((code.max_stack, code.max_locals), (4, 10));
    assert_eq!(
        code.code,
        [
            0x1f, 0x37, 8, 0x16, 8, 0x0a, 0x94, 0x9a, 0, 16, 0x16, 8, 0x37, 4, 0x1d, 0x99, 0, 8,
            0x16, 4, 0xa7, 0, 9, 0x16, 8, 0x37, 6, 0x16, 6, 0xad
        ]
    );
    let locals = vec![7, 4, 1, 0, 0, 0, 0, 4];
    assert_eq!(
        frames(&file, "f"),
        [(23, locals.clone(), vec![]), (29, locals, vec![4])]
    );
}

#[test]
fn singleton_long_binders_keep_constant_types_and_direct_reference_identity() {
    use dotty_tasty::tasty::*;
    for method in [
        "def f(): Long = 1L match { case n => n }",
        "def f(): Long = 4294967297L match { case n @ 4294967297L => n; case n => n }",
    ] {
        let pair = emit(method);
        let file = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
        let index = file.ast_address_index().unwrap();
        let payload = file.section(StandardSection::Asts).unwrap().payload;
        let mut binds = std::collections::BTreeSet::new();
        for node in index.iter_nodes_with_tag(BIND_TAG) {
            let RawTree::LengthNode(raw) =
                RawTree::decode(&mut Reader::new(&payload[node.offset..])).unwrap()
            else {
                panic!()
            };
            let bind = raw.decode_bind().unwrap();
            assert!(matches!(
                bind.type_tree,
                RawTree::Leaf(SimpleTerm {
                    tag: LONGCONST_TAG,
                    ..
                })
            ));
            binds.insert(node.offset as u32);
        }
        let referenced: std::collections::BTreeSet<_> = index
            .iter_nodes_with_tag(TERMREFDIRECT_TAG)
            .filter_map(|node| {
                let RawTree::Leaf(term) =
                    RawTree::decode(&mut Reader::new(&payload[node.offset..])).unwrap()
                else {
                    panic!()
                };
                let TermValue::AstRef(address) = term.value else {
                    panic!()
                };
                binds.contains(&address).then_some(address)
            })
            .collect();
        assert_eq!(referenced, binds);
    }
}

#[test]
fn unsupported_long_match_shapes_never_publish_a_pair() {
    for method in [
        "def f(x: Long): Int = x match { case 1L => 1 }",
        "def f(): Int = 1L match { case 1L => 1 }",
        "def f(x: Long): Int = x match { case _ if true => 1 }",
        "def f(x: Long): Int = x match { case _: Long => 1; case _ => 2 }",
        "def f(x: Long): Int = x match { case 1 => 1; case _ => 2 }",
        "def f(x: Long): Int = x match { case _: Int => 1; case _ => 2 }",
        "def f(x: Long): Int = x match { case _: scala.Long => 1 }",
        "def f(x: Long): Int = x match { case (_: Long) | 1L => 1; case _ => 2 }",
        "def f(x: Long): Int = x match { case n @ (m: Long) => 1 }",
        "def f(x: Long): Long = x match { case n if true => n; case _ => n }",
        "def f(x: Long): Int = x match { case n if n == 1L => 1; case _ => 2 }",
        "def f(x: Float): Int = x match { case _ => 1 }",
        "def f(x: Double): Int = x match { case _ => 1 }",
    ] {
        let source = format!("package example; class LongSelectorMatches {{ {method} }}");
        let result = emit_paired_jvm_tasty_class(
            &source,
            "LongSelectorMatches.scala",
            SourceId::from_index(2482),
        );
        assert!(
            matches!(
                result,
                Err(dotty::jvm_tasty_artifacts::PairedArtifactError::Plan(_))
            ),
            "{method}: {result:?}"
        );
    }
}

#[test]
fn malformed_long_patterns_preserve_full_value_and_source_identity_read_only() {
    use dotty::core::{
        Constant, Typed, Untyped,
        ast::{AstArena, TreeKind},
    };
    use dotty_tasty_pickler::{ClassStatePicklePlan, PickleError, PrimaryConstructorInputPlan};
    let source = "package example; class LongSelectorMatches { def f(x: Long): Int = x match { case 1L | 4294967297L => 7; case _ => 8 } }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
        source,
        "LongSelectorMatches.scala",
        SourceId::from_index(2482),
        |paired| {
            let raw = paired.tasty_plan.plan();
            for mode in 0..6 {
                let mut ast = AstArena::<Typed>::new();
                for (id, node) in raw.typed_ast.iter() {
                    assert_eq!(id, ast.alloc(node.clone()));
                }
                let mut source_ast = AstArena::<Untyped>::new();
                for (id, node) in raw.source_ast.iter() {
                    assert_eq!(id, source_ast.alloc(node.clone()));
                }
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
                    .map(|(id, n)| (id, n.ty, n.kind.clone()))
                    .collect();
                let alternative = ast
                    .iter()
                    .find_map(|(id, n)| matches!(n.kind, TreeKind::Alternative(_)).then_some(id))
                    .unwrap();
                match mode {
                    0 => ast.get_mut(literals[0].0).ty = literals[1].1,
                    1 => {
                        ast.get_mut(literals[0].0).ty = literals[1].1;
                        ast.get_mut(literals[0].0).kind = literals[1].2.clone();
                    }
                    2 => {
                        let leaf = source_ast
                            .iter()
                            .find_map(|(id, n)| {
                                matches!(
                                    n.kind,
                                    TreeKind::Literal(dotty::core::ast::Literal {
                                        value: Constant::Long(1)
                                    })
                                )
                                .then_some(id)
                            })
                            .unwrap();
                        source_ast.get_mut(leaf).kind =
                            TreeKind::Literal(dotty::core::ast::Literal {
                                value: Constant::Long(4_294_967_297),
                            });
                    }
                    3 => {
                        let TreeKind::Alternative(alt) = &mut ast.get_mut(alternative).kind else {
                            panic!()
                        };
                        alt.alternatives.clear();
                    }
                    4 => {
                        let selector = ast
                            .iter()
                            .find_map(|(_, n)| {
                                if let TreeKind::Match(m) = &n.kind {
                                    Some(m.selector)
                                } else {
                                    None
                                }
                            })
                            .unwrap();
                        ast.get_mut(selector).ty = raw.definitions.int;
                    }
                    5 => ast.get_mut(alternative).ty = raw.definitions.int,
                    _ => unreachable!(),
                }
                let before = format!("{:?}{:?}{:?}", raw.store, source_ast, ast);
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
                let error = input
                    .validate()
                    .and_then(|p| p.validate_method_bodies())
                    .unwrap_err();
                assert!(
                    match mode {
                        0 | 4 | 5 => matches!(error, PickleError::BodyType { .. }),
                        1 | 2 | 3 => matches!(error, PickleError::SourceShape { .. }),

                        _ => false,
                    },
                    "mode {mode}: {error:?}"
                );
                assert_eq!(before, format!("{:?}{:?}{:?}", raw.store, source_ast, ast));
            }
        },
    )
    .unwrap();
}

#[test]
fn singleton_match_join_rejects_a_different_long_with_the_same_low_bits() {
    use dotty::core::{
        Constant, Type, Typed,
        ast::{AstArena, TreeKind},
    };
    use dotty_tasty_pickler::{ClassStatePicklePlan, PickleError, PrimaryConstructorInputPlan};
    let source = "package example; class LongSelectorMatches { def f(): Long = 1L match { case n => n }; def other(): Long = 4294967297L }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(source, "LongSelectorMatches.scala", SourceId::from_index(2482), |paired| {
        let raw = paired.tasty_plan.plan();
        let mut ast = AstArena::<Typed>::new();
        for (id, node) in raw.typed_ast.iter() { assert_eq!(id, ast.alloc(node.clone())); }
        let root = ast.iter().find_map(|(id,n)| matches!(n.kind, TreeKind::Match(_)).then_some(id)).unwrap();
        let wrong = ast.iter().find_map(|(_,n)| matches!(raw.store.types.try_get(n.ty), Some(Type::Constant(Constant::Long(4_294_967_297)))).then_some(n.ty)).unwrap();
        ast.get_mut(root).ty = wrong;
        let before = format!("{:?}{:?}", raw.store, ast);
        let input = ClassStatePicklePlan {
            store: raw.store, definitions: raw.definitions, source_ast: raw.source_ast, typed_ast: &ast,
            source_index: raw.source_index, typed_bindings: raw.typed_bindings, source: raw.source.clone(),
            class: raw.class, class_tree: raw.class_tree,
            constructor: PrimaryConstructorInputPlan { symbol: raw.constructor.symbol, source_tree: raw.constructor.source_tree, parameter_clauses: raw.constructor.parameter_clauses.clone() }, members: raw.members,
        };
        assert!(matches!(input.validate().and_then(|p| p.validate_method_bodies()), Err(PickleError::BodyType { tree, ty }) if tree == root && ty == wrong));
        assert_eq!(before, format!("{:?}{:?}", raw.store, ast));
    }).unwrap();
}
