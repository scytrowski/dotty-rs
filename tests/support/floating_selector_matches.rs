#[path = "guarded_match_plan.rs"]
mod guarded_match_plan;
use guarded_match_plan::with_jvm;

use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::{PairedJvmTastyArtifacts, emit_paired_jvm_tasty_class};

const SOURCE: &str =
    include_str!("../fixtures/jvm_tasty_floating_selector_matches/FloatingSelectorMatches.scala");

fn emit(methods: &str) -> PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class FloatingSelectorMatches {{ {methods} }}"),
        "FloatingSelectorMatches.scala",
        SourceId::from_index(2484),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn implicit_and_explicit_typed_binders_keep_pattern_wrappers() {
    use dotty_tasty::tasty::*;
    for ty in ["Float", "Double"] {
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
fn complete_floating_selector_fixture_is_deterministic_and_preserves_pattern_nodes() {
    use dotty_tasty::tasty::*;
    let first = emit_paired_jvm_tasty_class(
        SOURCE,
        "FloatingSelectorMatches.scala",
        SourceId::from_index(2484),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_class(
        SOURCE,
        "FloatingSelectorMatches.scala",
        SourceId::from_index(2485),
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
        42
    );
    for (tag, count) in [
        (TYPED_TAG, 44),
        (BIND_TAG, 76),
        (MATCH_TAG, 111),
        (RETURN_TAG, 30),
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
        "FloatingSelectorMatches.scala",
        SourceId::from_index(2484),
    )
    .unwrap();
    let directory = std::env::temp_dir().join(format!(
        "dotty-floating-selector-matches-{label}-{}",
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
fn floating_selector_values_and_effects_execute_on_jdk21() {
    if !super::jdk_available() {
        return;
    }
    let directory = publish_fixture("java");
    let client = directory.join("JavaConsumer.java");
    std::fs::write(
        &client,
        include_str!("../fixtures/jvm_tasty_floating_selector_matches/JavaConsumer.java"),
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
        "OK 3502 checks"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn pinned_scala_consumes_original_and_recompiled_floating_selector_artifacts() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let directory = publish_fixture("scala");
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/jvm_tasty_floating_selector_matches/run.py"
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
    for ty in ["Float", "Double"] {
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
    let source = "package example; class FloatingSelectorMatches { def f(x: Double): Int = x match { case _: Double => 7 } }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
        source,
        "FloatingSelectorMatches.scala",
        SourceId::from_index(2484),
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
    for pattern in [
        "_: Float",
        "n: Float",
        "n @ (_: Float)",
        "_: Double",
        "n: Double",
        "n @ (_: Double)",
    ] {
        let ty = if pattern.contains("Float") {
            "Float"
        } else {
            "Double"
        };
        let source = format!(
            "package example; class FloatingSelectorMatches {{ def f(x: {ty}): Int = x match {{ case {pattern} => 7 }} }}"
        );
        dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(&source, "FloatingSelectorMatches.scala", SourceId::from_index(2484), |paired| {
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
fn singleton_floating_binders_keep_raw_bits_and_direct_reference_identity() {
    use dotty_tasty::tasty::*;
    for (ty, value, tag) in [
        ("Float", "-0.0f", FLOATCONST_TAG),
        ("Double", "-0.0d", DOUBLECONST_TAG),
    ] {
        let pair = emit(&format!("def f(): {ty} = {value} match {{ case n => n }}"));
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
            let RawTree::Leaf(term) = bind.type_tree else {
                panic!()
            };
            assert_eq!(term.tag, tag);
            assert_eq!(
                term.value,
                if ty == "Float" {
                    TermValue::Int(i32::MIN)
                } else {
                    TermValue::LongInt(i64::MIN)
                }
            );
            binds.insert(node.offset as u32);
        }
        assert_eq!(binds.len(), 1);
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
fn unsupported_floating_match_shapes_never_publish_a_pair() {
    for ty in ["Float", "Double"] {
        let literal = if ty == "Float" { "1f" } else { "1d" };
        for method in [
            format!("def f(x: {ty}): Int = x match {{ case {literal} => 1 }}"),
            format!("def f(): Int = {literal} match {{ case {literal} => 1 }}"),
            format!("def f(x: {ty}): Int = x match {{ case _ if true => 1 }}"),
            format!("def f(x: {ty}): Int = x match {{ case _: {ty} => 1; case _ => 2 }}"),
            format!("def f(x: {ty}): Int = x match {{ case 1L => 1; case _ => 2 }}"),
            format!("def f(x: {ty}): Int = x match {{ case _: Int => 1; case _ => 2 }}"),
            format!("def f(x: {ty}): Int = x match {{ case _: scala.{ty} => 1 }}"),
            format!(
                "def f(x: {ty}): Int = x match {{ case (_: {ty}) | {literal} => 1; case _ => 2 }}"
            ),
            format!("def f(x: {ty}): Int = x match {{ case n @ (m: {ty}) => 1 }}"),
            format!("def f(x: {ty}): {ty} = x match {{ case n if true => n; case _ => n }}"),
            format!(
                "def f(x: {ty}): Int = x match {{ case n if n == {literal} => 1; case _ => 2 }}"
            ),
        ] {
            let source = format!("package example; class FloatingSelectorMatches {{ {method} }}");
            let result = emit_paired_jvm_tasty_class(
                &source,
                "FloatingSelectorMatches.scala",
                SourceId::from_index(2484),
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
    for method in [
        "def f(): Float = -0.0f match { case n @ 0.0f => n; case n => n }",
        "def f(): Double = -0.0d match { case n @ 0.0d => n; case n => n }",
    ] {
        let result = emit_paired_jvm_tasty_class(
            &format!("package example; class FloatingSelectorMatches {{ {method} }}"),
            "FloatingSelectorMatches.scala",
            SourceId::from_index(2484),
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
fn malformed_double_patterns_preserve_all_bits_and_source_identity_read_only() {
    use dotty::core::{
        Constant, Typed, Untyped,
        ast::{AstArena, TreeKind},
    };
    use dotty_tasty_pickler::{ClassStatePicklePlan, PickleError, PrimaryConstructorInputPlan};
    let source = "package example; class FloatingSelectorMatches { def f(x: Double): Int = x match { case 0.0d | -0.0d => 7; case _ => 8 } }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
        source,
        "FloatingSelectorMatches.scala",
        SourceId::from_index(2484),
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
                                value: Constant::DoubleBits(_)
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
                                        value: Constant::DoubleBits(0)
                                    })
                                )
                                .then_some(id)
                            })
                            .unwrap();
                        source_ast.get_mut(leaf).kind =
                            TreeKind::Literal(dotty::core::ast::Literal {
                                value: Constant::DoubleBits(0x8000000000000000),
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
                        1..=3 => matches!(error, PickleError::SourceShape { .. }),

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
fn singleton_match_join_rejects_the_opposite_zero_sign() {
    use dotty::core::{
        Constant, Type, Typed,
        ast::{AstArena, TreeKind},
    };
    use dotty_tasty_pickler::{ClassStatePicklePlan, PickleError, PrimaryConstructorInputPlan};
    let source = "package example; class FloatingSelectorMatches { def f(): Double = 0.0d match { case n => n }; def other(): Double = -0.0d }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(source, "FloatingSelectorMatches.scala", SourceId::from_index(2484), |paired| {
        let raw = paired.tasty_plan.plan();
        let mut ast = AstArena::<Typed>::new();
        for (id, node) in raw.typed_ast.iter() { assert_eq!(id, ast.alloc(node.clone())); }
        let root = ast.iter().find_map(|(id,n)| matches!(n.kind, TreeKind::Match(_)).then_some(id)).unwrap();
        let wrong = ast.iter().find_map(|(_,n)| matches!(raw.store.types.try_get(n.ty), Some(Type::Constant(Constant::DoubleBits(0x8000000000000000)))).then_some(n.ty)).unwrap();
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

#[test]
fn floating_literal_and_typed_binders_have_exact_layout_and_join_frames() {
    use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
    for (ty, literal, descriptor, slots, expected, locals, stack, pcs) in [
        (
            "Float",
            "1f",
            "(FZ)F",
            6,
            vec![
                0x23, 0x38, 5, 0x17, 5, 0x0c, 0x95, 0x9a, 0, 14, 0x17, 5, 0x46, 0x1c, 0x99, 0, 7,
                0x25, 0xa7, 0, 9, 0x17, 5, 0x38, 4, 0x17, 4, 0xae,
            ],
            vec![7, 2, 1, 0, 0, 2],
            2,
            (21, 27),
        ),
        (
            "Double",
            "1d",
            "(DZ)D",
            10,
            vec![
                0x27, 0x39, 8, 0x18, 8, 0x0f, 0x97, 0x9a, 0, 16, 0x18, 8, 0x39, 4, 0x1d, 0x99, 0,
                8, 0x18, 4, 0xa7, 0, 9, 0x18, 8, 0x39, 6, 0x18, 6, 0xaf,
            ],
            vec![7, 3, 1, 0, 0, 0, 0, 3],
            3,
            (23, 29),
        ),
    ] {
        let pair = emit(&format!(
            "def f(x: {ty}, enabled: Boolean): {ty} = x match {{ case n @ {literal} if enabled => n; case n: {ty} => n }}"
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
        assert_eq!(
            (code.max_stack, code.max_locals),
            (if ty == "Float" { 2 } else { 4 }, slots)
        );
        assert_eq!(code.code, expected, "{ty}");
        assert_eq!(
            frames(&file, "f"),
            [
                (pcs.0, locals.clone(), vec![]),
                (pcs.1, locals, vec![stack])
            ]
        );
    }
}
