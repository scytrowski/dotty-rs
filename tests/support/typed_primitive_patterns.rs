use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::{PairedJvmTastyArtifacts, emit_paired_jvm_tasty_class};

const SOURCE: &str =
    include_str!("../fixtures/jvm_tasty_typed_primitive_patterns/TypedPrimitivePatterns.scala");

fn emit(methods: &str) -> PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class TypedPrimitivePatterns {{ {methods} }}"),
        "TypedPrimitivePatterns.scala",
        SourceId::from_index(2480),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn implicit_and_explicit_typed_binders_keep_pattern_wrappers() {
    use dotty_tasty::tasty::*;
    for ty in ["Int", "Boolean"] {
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
fn complete_typed_pattern_fixture_is_deterministic_and_preserves_pattern_nodes() {
    use dotty_tasty::tasty::*;
    let first = emit_paired_jvm_tasty_class(
        SOURCE,
        "TypedPrimitivePatterns.scala",
        SourceId::from_index(2480),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_class(
        SOURCE,
        "TypedPrimitivePatterns.scala",
        SourceId::from_index(2481),
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
        56
    );
    for (tag, count) in [
        (TYPED_TAG, 58),
        (BIND_TAG, 37),
        (MATCH_TAG, 47),
        (RETURN_TAG, 4),
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
        "TypedPrimitivePatterns.scala",
        SourceId::from_index(2480),
    )
    .unwrap();
    let directory = std::env::temp_dir().join(format!(
        "dotty-typed-primitive-patterns-{label}-{}",
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
fn typed_pattern_values_and_effects_execute_on_jdk21() {
    if !super::jdk_available() {
        return;
    }
    let directory = publish_fixture("java");
    let client = directory.join("JavaConsumer.java");
    std::fs::write(
        &client,
        include_str!("../fixtures/jvm_tasty_typed_primitive_patterns/JavaConsumer.java"),
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
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "OK 576 checks");
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn pinned_scala_consumes_original_and_recompiled_typed_pattern_artifacts() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let directory = publish_fixture("scala");
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/jvm_tasty_typed_primitive_patterns/run.py"
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
    for ty in ["Int", "Boolean"] {
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
            super::with_jvm(&source, |unit, definitions| {
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
fn unsupported_typed_pattern_shapes_never_publish_a_pair() {
    for method in [
        "def f(x: Long): Int = x match { case _: Long => 1 }",
        "def f(x: Float): Int = x match { case _: Float => 1 }",
        "def f(x: Double): Int = x match { case _: Double => 1 }",
        "def f(x: Int): Int = x match { case _: Boolean => 1; case _ => 2 }",
        "def f(x: Boolean): Int = x match { case _: Int => 1; case _ => 2 }",
        "def f(x: Int): Int = x match { case _: Long => 1; case _ => 2 }",
        "def f(x: Int): Int = x match { case _: scala.Int => 1 }",
        "def f(x: Int): Int = x match { case (_: Int) | 1 => 1; case _ => 2 }",
        "def f(x: Int): Int = x match { case n @ (m: Int) => 1 }",
        "def f(x: Int): Int = x match { case n: Int if n > 0 => n; case _ => n }",
    ] {
        let source = format!("package example; class TypedPrimitivePatterns {{ {method} }}");
        let error = emit_paired_jvm_tasty_class(
            &source,
            "TypedPrimitivePatterns.scala",
            SourceId::from_index(2480),
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
}

#[test]
fn malformed_typed_pattern_nodes_are_rejected_without_mutation() {
    use dotty::core::{
        Typed, Untyped,
        ast::{AstArena, TreeKind},
    };
    use dotty_tasty_pickler::{ClassStatePicklePlan, PickleError, PrimaryConstructorInputPlan};
    for ty in ["Int", "Boolean"] {
        for pattern in [
            format!("_: {ty}"),
            format!("n: {ty}"),
            format!("n @ (_: {ty})"),
        ] {
            let source = format!(
                "package example; class TypedPrimitivePatterns {{ def f(x: {ty}): Int = x match {{ case {pattern} => 7 }} }}"
            );
            dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(&source, "TypedPrimitivePatterns.scala", SourceId::from_index(2480), |paired| {
                let raw = paired.tasty_plan.plan();
                for mode in 0..11 {
                    let mut ast = AstArena::<Typed>::new();
                    for (id, node) in raw.typed_ast.iter() { assert_eq!(id, ast.alloc(node.clone())); }
                    let mut source_ast = AstArena::<Untyped>::new();
                    for (id, node) in raw.source_ast.iter() { assert_eq!(id, source_ast.alloc(node.clone())); }
                    let (root, wrapper) = ast.iter().find_map(|(id, node)| {
                        if let TreeKind::Typed(w) = node.kind { Some((id, w)) } else { None }
                    }).unwrap();
                    let (source_root, source_wrapper) = source_ast.iter().find_map(|(id, node)| {
                        if let TreeKind::Typed(w) = node.kind { Some((id, w)) } else { None }
                    }).unwrap();
                    let wrong = if ty == "Int" { raw.definitions.boolean } else { raw.definitions.int };
                    match mode {
                        0 => ast.get_mut(root).ty = wrong,
                        1 => ast.get_mut(wrapper.tpt).ty = wrong,
                        2 => ast.get_mut(wrapper.expr).ty = wrong,
                        3 => ast.get_mut(wrapper.tpt).kind = ast.get(wrapper.expr).kind.clone(),
                        4 => ast.get_mut(wrapper.expr).kind = TreeKind::Literal(dotty::core::ast::Literal { value: dotty::core::Constant::Int(0) }),
                        5 => { let TreeKind::Ident(ident) = &mut ast.get_mut(wrapper.expr).kind else { panic!() }; ident.backquoted = true; },
                        6 => { let TreeKind::Typed(w) = &mut ast.get_mut(root).kind else { panic!() }; w.expr = root; },
                        7 => ast.get_mut(wrapper.tpt).position = ast.get(wrapper.expr).position,
                        8 => source_ast.get_mut(source_wrapper.tpt).kind = source_ast.get(source_wrapper.expr).kind.clone(),
                        9 => { let TreeKind::Ident(ident) = &mut source_ast.get_mut(source_wrapper.expr).kind else { panic!() }; ident.backquoted = true; },
                        10 => { let TreeKind::Typed(w) = &mut source_ast.get_mut(source_root).kind else { panic!() }; w.tpt = source_wrapper.expr; },
                        _ => unreachable!(),
                    }
                    let before = format!("{:?}{:?}{:?}", raw.store, source_ast, ast);
                    let checkpoint = raw.store.checkpoint();
                    let input = ClassStatePicklePlan {
                        store: raw.store, definitions: raw.definitions, source_ast: &source_ast, typed_ast: &ast,
                        source_index: raw.source_index, typed_bindings: raw.typed_bindings, source: raw.source.clone(),
                        class: raw.class, class_tree: raw.class_tree,
                        constructor: PrimaryConstructorInputPlan { symbol: raw.constructor.symbol, source_tree: raw.constructor.source_tree, parameter_clauses: raw.constructor.parameter_clauses.clone() }, members: raw.members,
                    };
                    let error = input.validate().and_then(|p| p.validate_method_bodies()).unwrap_err();
                    let expected = match mode {
                        0..=5 | 8 => matches!(error, PickleError::BodyType { .. }),
                        6 => matches!(error, PickleError::RepeatedExpression { tree } if tree == root),
                        7 => matches!(error, PickleError::BodySpan { tree } if tree == wrapper.tpt),
                        9 => matches!(error, PickleError::Unsupported { .. }),
                        10 => matches!(error, PickleError::BodyBinding { .. }),
                        _ => false,
                    };
                    assert!(expected, "{ty} {pattern} mode {mode}: {error:?}");
                    if mode <= 6 {
                        let unit = dotty::jvm::JvmCompilationUnit { store: raw.store, typed_ast: &ast, classes: paired.jvm_unit.classes.clone() };
                        let error = dotty::jvm::emit_class(&unit, raw.class, raw.definitions).unwrap_err();
                        assert!(matches!(error, dotty::jvm::JvmCodegenError::UnsupportedTypedExpression { tree, node: "inconsistent typed primitive pattern" } if tree == root), "{ty} {pattern} mode {mode}: {error:?}");
                    }
                    assert_eq!(checkpoint, raw.store.checkpoint());
                    assert_eq!(before, format!("{:?}{:?}{:?}", raw.store, source_ast, ast));
                }
            }).unwrap();
        }
    }
}

#[test]
fn same_spelling_typed_binders_have_distinct_direct_wire_references() {
    use dotty_tasty::tasty::*;
    for (ty, guard) in [("Int", "n > 0"), ("Boolean", "n")] {
        let pair = emit(&format!(
            "def f(x: {ty}): {ty} = x match {{ case n: {ty} if {guard} => n; case n: {ty} => n }}"
        ));
        let file = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
        let payload = file.section(StandardSection::Asts).unwrap().payload;
        let index = file.ast_address_index().unwrap();
        let bindings: Vec<_> = index
            .iter_nodes_with_tag(BIND_TAG)
            .map(|n| n.offset as u32)
            .collect();
        assert_eq!(bindings.len(), 2);
        assert_ne!(bindings[0], bindings[1]);
        let mut referenced = std::collections::BTreeSet::new();
        for node in index.iter_nodes_with_tag(TERMREFDIRECT_TAG) {
            let RawTree::Leaf(term) =
                RawTree::decode(&mut Reader::new(&payload[node.offset..])).unwrap()
            else {
                panic!()
            };
            let TermValue::AstRef(address) = term.value else {
                panic!()
            };
            if bindings.contains(&address) {
                referenced.insert(address);
            }
        }
        assert_eq!(referenced.into_iter().collect::<Vec<_>>(), bindings);
    }
}

#[test]
fn legacy_typed_patterns_obey_validation_and_encoding_limits_read_only() {
    use dotty::core::ast::TreeKind;
    use dotty_tasty_pickler::{
        ClassMemberPicklePlan, DefaultConstructorPlan, LimitKind, MethodPicklePlan, PickleError,
        PickleLimits, PicklePlan, ProjectionLimits, ValidationLimits,
    };
    let source = "package example; class TypedPrimitivePatterns { def f(x: Int): Int = x match { case _: Int => 7 } }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
        source,
        "TypedPrimitivePatterns.scala",
        SourceId::from_index(2480),
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
    for pattern in ["_: Int", "n: Int", "n @ (_: Int)"] {
        let source = format!(
            "package example; class TypedPrimitivePatterns {{ def f(x: Int): Int = x match {{ case {pattern} => 7 }} }}"
        );
        dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(&source, "TypedPrimitivePatterns.scala", SourceId::from_index(2480), |paired| {
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
fn typed_guarded_binders_keep_wide_results_and_exact_join_frames() {
    use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
    for (ty, descriptor, slots, load_a, load_b, ret, tag) in [
        ("Long", "(ZJJ)J", 8, 0x20, vec![0x16, 4], 0xad, 4),
        ("Float", "(ZFF)F", 6, 0x24, vec![0x25], 0xae, 2),
        ("Double", "(ZDD)D", 8, 0x28, vec![0x18, 4], 0xaf, 3),
    ] {
        let pair = emit(&format!(
            "def f(x: Boolean,a: {ty},b: {ty}): {ty} = x match {{ case n: Boolean if n => a; case _: Boolean => b }}"
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
        assert_eq!(code.max_stack, if ty == "Float" { 1 } else { 2 });
        let temp = (slots - 1) as u8;
        let binder = temp - 1;
        let mut expected = vec![
            0x1b,
            0x36,
            temp,
            0x15,
            temp,
            0x36,
            binder,
            0x15,
            binder,
            0x99,
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
        let locals = vec![7, 1, tag, tag, 1, 1];
        assert_eq!(
            frames(&file, "f"),
            [
                (16, locals.clone(), vec![]),
                (if ty == "Float" { 17 } else { 18 }, locals, vec![tag])
            ]
        );
    }
    if super::jdk_available() {
        let pair = emit(
            "def l(x: Boolean,a: Long,b: Long): Long = x match { case n: Boolean if n => a; case _: Boolean => b }; def f(x: Boolean,a: Float,b: Float): Float = x match { case n: Boolean if n => a; case _: Boolean => b }; def d(x: Boolean,a: Double,b: Double): Double = x match { case n: Boolean if n => a; case _: Boolean => b }",
        );
        super::run_java(
            &pair,
            "TypedPatternJoins",
            r#"
import example.TypedPrimitivePatterns;
public class TypedPatternJoins {
  public static void main(String[] args) {
    var p = new TypedPrimitivePatterns();
    for (boolean x : new boolean[]{false, true}) {
      if (p.l(x, Long.MIN_VALUE, Long.MAX_VALUE) != (x ? Long.MIN_VALUE : Long.MAX_VALUE)) throw new AssertionError();
      if (Float.floatToRawIntBits(p.f(x, -0.0f, 0.0f)) != (x ? Integer.MIN_VALUE : 0)) throw new AssertionError();
      if (Double.doubleToRawLongBits(p.d(x, -0.0d, 0.0d)) != (x ? Long.MIN_VALUE : 0)) throw new AssertionError();
    }
    System.out.println("guarded match oracle ok");
  }
}
"#,
        );
    }
}
