use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::{PairedJvmTastyArtifacts, emit_paired_jvm_tasty_class};
const SOURCE: &str =
    include_str!("../fixtures/jvm_tasty_primitive_value_discard/PrimitiveValueDiscard.scala");

fn emit(methods: &str) -> PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class PrimitiveValueDiscard {{ {methods} }}"),
        "PrimitiveValueDiscard.scala",
        SourceId::from_index(2486),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn complete_discard_fixture_is_deterministic_and_preserves_control_flow() {
    use dotty_tasty::tasty::*;
    let first = emit_paired_jvm_tasty_class(
        SOURCE,
        "PrimitiveValueDiscard.scala",
        SourceId::from_index(2486),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_class(
        SOURCE,
        "PrimitiveValueDiscard.scala",
        SourceId::from_index(2487),
    )
    .unwrap();
    assert_eq!(first.classfile_bytes(), second.classfile_bytes());
    assert_eq!(first.tasty_bytes(), second.tasty_bytes());
    let file = TastyFile::parse_and_validate_scala_3_9(first.tasty_bytes()).unwrap();
    let index = file.ast_address_index().unwrap();
    assert_eq!(
        index
            .iter_nodes_with_tag(dotty_tasty::tasty::WHILE_TAG)
            .count(),
        5
    );
}

fn publish_fixture(label: &str) -> std::path::PathBuf {
    let pair = emit_paired_jvm_tasty_class(
        SOURCE,
        "PrimitiveValueDiscard.scala",
        SourceId::from_index(2486),
    )
    .unwrap();
    let directory = std::env::temp_dir().join(format!(
        "dotty-primitive-value-discard-{label}-{}",
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
fn primitive_value_discard_values_and_effects_execute_on_jdk21() {
    if !super::jdk_available() {
        return;
    }
    let directory = publish_fixture("java");
    let client = directory.join("JavaConsumer.java");
    std::fs::write(
        &client,
        include_str!("../fixtures/jvm_tasty_primitive_value_discard/JavaConsumer.java"),
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
        "OK 1928 checks"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn pinned_scala_consumes_original_and_recompiled_primitive_value_discard_artifacts() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let directory = publish_fixture("scala");
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/jvm_tasty_primitive_value_discard/run.py"
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
fn each_primitive_statement_composition_emits() {
    for line in SOURCE
        .lines()
        .map(str::trim)
        .filter(|s| s.starts_with("def "))
    {
        emit(line);
    }
}

#[test]
fn malformed_unit_wrappers_reject_read_only() {
    use dotty::core::{
        Constant, Typed,
        ast::{AstArena, TreeKind},
    };
    use dotty_tasty_pickler::{
        ClassStatePicklePlan, PickleError, PrimaryConstructorInputPlan, ReferenceError, SourceRole,
        UnsupportedFeature,
    };
    for ty in ["Int", "Boolean", "Long", "Float", "Double"] {
        let source = format!(
            "package example; class PrimitiveValueDiscard {{ def f(x: {ty}): Unit = x; def foreign(x: {ty}): {ty} = x }}"
        );
        dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(&source, "PrimitiveValueDiscard.scala", SourceId::from_index(2486), |paired| {
            let raw = paired.tasty_plan.plan();
            let root = paired.jvm_unit.classes[0].methods[0].body.unwrap();
            let TreeKind::Block(wrapper) = &raw.typed_ast.get(root).kind else { panic!() };
            let child = wrapper.stats[0];
            let unit = wrapper.expr;
            let foreign = paired.jvm_unit.classes[0].methods[1].body.unwrap();
            for mode in 0..11 {
                let mut ast = AstArena::<Typed>::new();
                for (id,node) in raw.typed_ast.iter() { assert_eq!(id,ast.alloc(node.clone())); }
                match mode {
                    0 => ast.get_mut(root).ty = raw.definitions.int,
                    1 => { let TreeKind::Block(b) = &mut ast.get_mut(root).kind else { panic!() }; b.stats.push(child); }
                    2 => { let TreeKind::Block(b) = &mut ast.get_mut(root).kind else { panic!() }; b.expr = child; }
                    3 => ast.get_mut(unit).ty = raw.definitions.boolean,
                    4 => ast.get_mut(unit).kind = TreeKind::Literal(dotty::core::ast::Literal { value: Constant::Boolean(false) }),
                    5 => ast.get_mut(unit).position = raw.typed_ast.get(child).position,
                    6 => ast.get_mut(child).ty = raw.definitions.boolean,
                    7 => { let TreeKind::Ident(i) = &mut ast.get_mut(child).kind else { panic!() }; i.backquoted = true; }
                    8 => ast.get_mut(child).position = raw.typed_ast.get(foreign).position,
                    9 => ast.get_mut(child).ty = raw.typed_ast.get(foreign).ty,
                    10 => { let TreeKind::Block(b) = &mut ast.get_mut(root).kind else { panic!() }; b.stats[0] = root; }
                    _ => unreachable!(),
                }
                let before = format!("{:?}{:?}",raw.store,ast);
                let input = ClassStatePicklePlan {
                    store:raw.store,definitions:raw.definitions,source_ast:raw.source_ast,typed_ast:&ast,
                    source_index:raw.source_index,typed_bindings:raw.typed_bindings,source:raw.source.clone(),class:raw.class,class_tree:raw.class_tree,
                    constructor:PrimaryConstructorInputPlan {symbol:raw.constructor.symbol,source_tree:raw.constructor.source_tree,parameter_clauses:raw.constructor.parameter_clauses.clone()},members:raw.members,
                };
                let error = input.validate().and_then(|p|p.validate_method_bodies()).unwrap_err();
                assert!(match mode {
                    0 | 10 => matches!(error, PickleError::SourceShape { expected: SourceRole::Body, .. }),
                    1 => matches!(error, PickleError::BodyType { tree, .. } if tree == root),
                    2 | 6 => matches!(error, PickleError::BodyType { tree, .. } if tree == child),
                    3..=5 => matches!(error, PickleError::BodyType { tree, .. } if tree == unit),
                    7 if ty == "Long" || ty == "Float" || ty == "Double" => matches!(error, PickleError::Unsupported { feature: UnsupportedFeature::BodyExpression, .. }),
                    7 => matches!(error, PickleError::SourceShape { expected: SourceRole::Body, .. }),
                    8 => matches!(error, PickleError::BodySpan { tree } if tree == child),
                    9 if ty == "Int" || ty == "Boolean" => matches!(error, PickleError::Owner { .. }),
                    9 => matches!(error, PickleError::Reference { reason: ReferenceError::NotRegistered, .. }),
                    _ => false,
                }, "{ty} mode {mode}: {error:?}");
                assert_eq!(before,format!("{:?}{:?}",raw.store,ast));
            }
        }).unwrap();
    }
}

#[test]
fn primitive_discard_retains_existing_unsupported_boundaries() {
    for method in [
        "def f(x: Double): Unit = {val u: Unit = x; ()}",
        "def f(): Unit = null",
        "def f(): Unit = \"value\"",
        "def f(x: Double): Unit = x + 1d",
        "def f(x: Long): Unit = x + 1L",
        "def source(x: Long): Long = x; def f(x: Long): Unit = source(x)",
        "def source(x: Boolean): Boolean = x; def f(x: Boolean): Unit = source(x)",
        "def f(x: Double): Double = {val y = {return x; x}; y}",
        "def f(x: Double): Int = true match {case n if {return 3; n} => 1;case _ => 2}",
        "def f(x: Double): Int = true match {case n if {({return 3; x}: Unit); n} => 1;case _ => 2}",
        "def f(x: Double): Int = {val y: Int = {({return 3; x}: Unit); 7}; y}",
    ] {
        let result = emit_paired_jvm_tasty_class(
            &format!("package example; class PrimitiveValueDiscard {{ {method} }}"),
            "PrimitiveValueDiscard.scala",
            SourceId::from_index(2486),
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
fn existing_int_call_can_be_adapted_to_unit_without_changing_its_target() {
    let pair = emit("def source(x: Int): Int = 10 / x; def f(x: Int): Unit = source(x)");
    let file =
        dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
    assert_eq!(
        file.ast_address_index()
            .unwrap()
            .iter_nodes_with_tag(dotty_tasty::tasty::BLOCK_TAG)
            .count(),
        1
    );
}

#[test]
fn expected_unit_wrappers_require_exact_source_edges_and_obey_limits() {
    use dotty::core::{SymbolId, TreeId, Typed, Untyped, ast::TreeKind};
    use dotty_tasty_pickler::{
        ClassStatePicklePlan, LimitKind, LocalValueBinding, PickleError, PickleLimits,
        PrimaryConstructorInputPlan, ProjectionLimits, SourceTypedBindings,
    };
    struct Missing<'a> {
        original: &'a dyn SourceTypedBindings,
        removed: TreeId<Untyped>,
    }
    impl SourceTypedBindings for Missing<'_> {
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
    for ty in ["Int", "Boolean", "Long", "Float", "Double"] {
        let source = format!(
            "package example; class PrimitiveValueDiscard {{def f(x: {ty}): Unit = (x: {ty})}}"
        );
        dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(&source,"PrimitiveValueDiscard.scala",SourceId::from_index(2486),|paired| {
            let raw = paired.tasty_plan.plan();
            let before = format!("{:?}{:?}{:?}",raw.store,raw.source_ast,raw.typed_ast);
            let mut edges = Vec::new();
            for (_,node) in raw.source_ast.iter() {
                match &node.kind {
                    TreeKind::DefDef(d) => if let Some(rhs) = d.rhs { edges.push(rhs); },
                    TreeKind::Typed(t) => {edges.push(t.expr);edges.push(t.tpt);},
                    _ => {},
                }
            }
            for removed in edges {
                let bindings = Missing {original:raw.typed_bindings,removed};
                let input = ClassStatePicklePlan {
                    store:raw.store,definitions:raw.definitions,source_ast:raw.source_ast,typed_ast:raw.typed_ast,
                    source_index:raw.source_index,typed_bindings:&bindings,source:raw.source.clone(),class:raw.class,class_tree:raw.class_tree,
                    constructor:PrimaryConstructorInputPlan {symbol:raw.constructor.symbol,source_tree:raw.constructor.source_tree,parameter_clauses:raw.constructor.parameter_clauses.clone()},members:raw.members,
                };
                let error = input.validate().and_then(|p|p.validate_method_bodies()).unwrap_err();
                let method = paired.jvm_unit.classes[0].methods[0].method;
                assert!(matches!(error,PickleError::BodyBinding {tree,actual:None,..} if tree == removed)
                    || matches!(error,PickleError::MissingBody {method: actual} if actual == method),"{ty}: {error:?}");
            }
            let encoding = paired.tasty_plan.pickle_with_limits(PickleLimits {projection:ProjectionLimits {max_expression_depth:0,..Default::default()},..Default::default()}).unwrap_err();
            assert_eq!(encoding,PickleError::LimitExceeded {kind:LimitKind::ExpressionDepth,limit:0});
            assert_eq!(before,format!("{:?}{:?}{:?}",raw.store,raw.source_ast,raw.typed_ast));
        }).unwrap();
    }
}

#[test]
fn single_and_batch_plans_borrow_the_adapted_unit_root_and_keep_binder_scope() {
    use dotty::core::ast::TreeKind;
    let source = "package example; class PrimitiveValueDiscard { def f(x: Double): Unit = x; def bound(x: Double): Int = x match {case n => {n; 7}} }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(source,"PrimitiveValueDiscard.scala",SourceId::from_index(2486),|paired| {
        let raw = paired.tasty_plan.plan();
        let root = paired.jvm_unit.classes[0].methods[0].body.unwrap();
        assert_eq!(raw.typed_ast.get(root).ty,raw.definitions.unit);
        assert!(matches!(&raw.typed_ast.get(root).kind,TreeKind::Block(b) if b.stats.len()==1));
        assert!(raw.source_ast.iter().any(|(id,n)| matches!(&n.kind,TreeKind::DefDef(d) if d.rhs.is_some_and(|rhs|raw.typed_bindings.typed_at(raw.source.id,rhs)==Some(root)) && raw.source_index.symbol_at(raw.source.id,id)==Some(paired.jvm_unit.classes[0].methods[0].method))));
    }).unwrap();
    let batch = dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch(
        "package example; class First { def f(x: Long): Unit = x }; class Second { def f(x: Double): Unit = x }",
        "UnitBatch.scala",
        SourceId::from_index(2486),
    );
    assert!(batch.is_ok(), "{batch:?}");
    let error = emit_paired_jvm_tasty_class(
        "package example; class PrimitiveValueDiscard { def f(x: Double): Int = x match {case n => {n; 7}; case _ => {n; 8}} }",
        "PrimitiveValueDiscard.scala",
        SourceId::from_index(2486),
    );
    assert!(matches!(
        error,
        Err(dotty::jvm_tasty_artifacts::PairedArtifactError::Plan(_))
    ));
}

#[test]
fn while_discards_keep_exact_descriptors_slot_widths_and_empty_body_edges() {
    use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
    for (ty, descriptor, load, pop, tag, locals) in [
        ("Int", "(I)I", 0x1b, 0x57, 1, 3),
        ("Boolean", "(Z)I", 0x1b, 0x57, 1, 3),
        ("Long", "(J)I", 0x1f, 0x58, 4, 4),
        ("Float", "(F)I", 0x23, 0x57, 2, 3),
        ("Double", "(D)I", 0x27, 0x58, 3, 4),
    ] {
        let pair = emit(&format!(
            "def f(x: {ty}): Int = {{var n: Int = 0; while n < 2 do {{n = n + 1; x}}; n}}"
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
        assert_eq!((code.max_stack, code.max_locals), (2, locals));
        assert!(
            code.code.windows(2).any(|w| w == [load, pop]),
            "{ty}: {:x?}",
            code.code
        );
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
        assert!(count > 0);
        fn types(r: &mut Reader<'_>) -> Vec<u8> {
            (0..r.read_u16().unwrap())
                .map(|_| {
                    let tag = r.read_u8().unwrap();
                    if tag == 7 || tag == 8 {
                        r.read_u16().unwrap();
                    }
                    tag
                })
                .collect()
        }
        for _ in 0..count {
            assert_eq!(r.read_u8().unwrap(), 255);
            r.read_u16().unwrap();
            assert_eq!(types(&mut r), vec![7, tag, 1]);
            let stack = types(&mut r);
            assert!(stack.is_empty() || stack == [1], "{ty}: {stack:?}");
        }
    }
}
