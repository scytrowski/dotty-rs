use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::{
    PairedArtifactError, PairedJvmTastyArtifacts, emit_paired_jvm_tasty_class,
};

const SOURCE: &str = include_str!("../fixtures/jvm_tasty_boolean_calls/BooleanCalls.scala");
const SOURCE_ID: SourceId = SourceId::from_index(2485);

fn fixture(id: SourceId) -> PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(SOURCE, "BooleanCalls.scala", id).unwrap()
}

fn emit(methods: &str) -> Result<PairedJvmTastyArtifacts, PairedArtifactError> {
    emit_paired_jvm_tasty_class(
        &format!("package example; class BooleanCalls {{{methods}}}"),
        "BooleanCalls.scala",
        SOURCE_ID,
    )
}

fn publish(label: &str) -> std::path::PathBuf {
    let pair = fixture(SOURCE_ID);
    let out = std::env::temp_dir().join(format!(
        "dotty-boolean-calls-{label}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(out.join("example")).unwrap();
    std::fs::write(out.join(pair.classfile_path()), pair.classfile_bytes()).unwrap();
    std::fs::write(out.join(pair.tasty_path()), pair.tasty_bytes()).unwrap();
    out
}

#[test]
fn boolean_calls_fixture_is_deterministic_in_fresh_sessions() {
    let first = fixture(SOURCE_ID);
    let second = fixture(SourceId::from_index(2486));
    assert_eq!(first.classfile_bytes(), second.classfile_bytes());
    assert_eq!(first.tasty_bytes(), second.tasty_bytes());
}

#[test]
fn boolean_calls_values_callee_effects_and_exceptions_execute_on_jdk21() {
    if !super::jdk_available() {
        return;
    }
    let directory = publish("java");
    let client = directory.join("JavaConsumer.java");
    std::fs::write(
        &client,
        include_str!("../fixtures/jvm_tasty_boolean_calls/JavaConsumer.java"),
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
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "OK 78 checks");
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn pinned_scala_consumes_original_and_recompiled_boolean_calls_artifacts() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let directory = publish("scala");
    let result = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/jvm_tasty_boolean_calls/run.py"
        ))
        .arg(&directory)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    println!("{}", String::from_utf8_lossy(&result.stdout));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn boolean_calls_keep_exact_virtual_targets_descriptors_and_stack_sizes() {
    use dotty_classfile::{
        attribute::Attribute,
        class_file::ClassFile,
        constant_pool::{ConstantPoolEntry, ConstantPoolIndex},
        reader::Reader,
    };
    let pair = fixture(SOURCE_ID);
    let class = ClassFile::decode(&mut Reader::new(pair.classfile_bytes())).unwrap();
    for (name, descriptor, prefix, target, stack, locals) in [
        ("direct", "(I)Z", &[0x2a, 0x1b][..], "positive", 2, 2),
        ("explicit", "(I)Z", &[0x2a, 0x1b][..], "positive", 2, 2),
        ("noarg", "()Z", &[0x2a][..], "ready", 1, 1),
        ("mixedCall", "(IZ)Z", &[0x2a, 0x1b, 0x1c][..], "mixed", 3, 3),
    ] {
        let method = class
            .methods
            .iter()
            .find(|m| class.constant_pool.utf8(m.name_index) == Ok(name))
            .unwrap();
        assert_eq!(
            class.constant_pool.utf8(method.descriptor_index),
            Ok(descriptor)
        );
        let [Attribute::Code(code)] = method.attributes.as_slice() else {
            panic!()
        };
        assert_eq!((code.max_stack, code.max_locals), (stack, locals));
        assert_eq!(&code.code[..prefix.len()], prefix);
        assert_eq!(code.code.len(), prefix.len() + 4);
        assert_eq!(code.code[prefix.len()], 0xb6);
        assert_eq!(code.code.last(), Some(&0xac));
        let index = ConstantPoolIndex(u16::from_be_bytes([
            code.code[prefix.len() + 1],
            code.code[prefix.len() + 2],
        ]));
        let Some(ConstantPoolEntry::Methodref {
            class_index,
            name_and_type_index,
        }) = class.constant_pool.get(index)
        else {
            panic!()
        };
        assert_eq!(
            class.constant_pool.class_name(*class_index),
            Ok("example/BooleanCalls")
        );
        let Some(ConstantPoolEntry::NameAndType {
            name_index,
            descriptor_index,
        }) = class.constant_pool.get(*name_and_type_index)
        else {
            panic!()
        };
        assert_eq!(class.constant_pool.utf8(*name_index), Ok(target));
        assert_eq!(class.constant_pool.utf8(*descriptor_index), Ok(descriptor));
    }
}

#[test]
fn boolean_calls_compose_with_existing_value_and_control_flow_families() {
    let base = "def truth(x: Int): Boolean = x > 0; def identity(b: Boolean): Boolean = b; def score(b: Boolean): Int = if b then 1 else 2;";
    for method in [
        "def f(x: Int): Boolean = {var flag: Boolean = false; flag = truth(x); flag}",
        "def f(x: Int): Boolean = truth(x): Boolean",
        "def f(b: Boolean): Boolean = b match {case n => identity(n)}",
        "def f(x: Int): Int = score((truth(x)))",
        "def f(x: Int): Int = score(x match {case n if truth(n) => true; case _ => false})",
        "def f(x: Int): Int = {val n: Int = x; score(truth(n))}",
        "def f(x: Int): Unit = truth(x)",
        "def f(x: Int): Boolean = if x > 0 then f(x - 1) else true",
    ] {
        emit(&format!("{base}{method}")).unwrap_or_else(|e| panic!("{method}: {e:?}"));
    }
    for (ty, yes, no) in [
        ("Int", "1", "2"),
        ("Boolean", "true", "false"),
        ("Long", "1L", "2L"),
        ("Float", "1.0f", "2.0f"),
        ("Double", "1.0d", "2.0d"),
    ] {
        emit(&format!(
            "{base}def f(x: Int): {ty} = if truth(x) then {yes} else {no}"
        ))
        .unwrap();
    }
}

#[test]
fn boolean_call_boundaries_reject_before_emitting_artifacts() {
    for methods in [
        "def truth(using b: Boolean): Boolean = b; def f(b: Boolean): Boolean = truth(using b)",
        "def truth(b: Boolean)(x: Int): Boolean = b; def f(b: Boolean,x: Int): Boolean = truth(b)(x)",
        "private def truth(b: Boolean): Boolean = b; def f(b: Boolean): Boolean = truth(b)",
        "def truth(b: Boolean) = b; def f(b: Boolean): Boolean = truth(b)",
        "def truth: Boolean = true; def f(): Boolean = truth",
        "def truth(x: Long): Boolean = true; def f(x: Long): Boolean = truth(x)",
        "def identity(x: Long): Long = x; def f(x: Long): Long = identity(x)",
        "def sink(b: Boolean): Unit = (); def f(b: Boolean): Unit = sink(b)",
        "def truth(b: Boolean): Boolean = b; def f(x: Int): Boolean = truth(x)",
        "def truth(b: Boolean): Boolean = b; def f(b: Boolean): Boolean = truth({return b; b})",
        "def truth(x: Int): Boolean = x > 0; def f(x: Int): Boolean = truth({return true; x})",
    ] {
        let result = emit(methods);
        assert!(
            matches!(result, Err(PairedArtifactError::Plan(_))),
            "{methods}: {result:?}"
        );
    }
}

#[test]
fn boolean_calls_require_each_source_edge_and_reject_projection_depth_read_only() {
    use dotty::core::{TreeId, Typed, Untyped, ast::TreeKind};
    use dotty_tasty_pickler::{
        ClassStatePicklePlan, LimitKind, PickleError, PickleLimits, ProjectionLimits,
        SourceTypedBindings,
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
    }
    let source = "package example; class BooleanCalls {def truth(b: Boolean): Boolean = b; def f(b: Boolean): Boolean = this.truth(b)}";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(source, "BooleanCalls.scala", SOURCE_ID, |paired| {
        let raw = paired.tasty_plan.plan();
        let before = format!("{:?}{:?}{:?}", raw.store, raw.source_ast, raw.typed_ast);
        let (root, call) = raw.source_ast.iter().find_map(|(id, node)| match &node.kind { TreeKind::Apply(call) => Some((id, call)), _ => None }).unwrap();
        let TreeKind::Select(select) = &raw.source_ast.get(call.function).kind else { panic!() };
        for removed in [root, call.function, select.qualifier, call.args[0]] {
            let bindings = Missing { original: raw.typed_bindings, removed };
            let candidate = ClassStatePicklePlan { typed_bindings: &bindings, source: raw.source.clone(), constructor: raw.constructor.clone(), ..*raw };
            let error = candidate.validate().and_then(|p| p.validate_method_bodies()).unwrap_err();
            assert!(if removed == root { matches!(error, PickleError::MissingBody { .. }) } else { matches!(error, PickleError::BodyBinding { tree, actual: None, .. } if tree == removed) }, "{error:?}");
        }
        assert_eq!(before, format!("{:?}{:?}{:?}", raw.store, raw.source_ast, raw.typed_ast));
    }).unwrap();
    let source = "package example; class BooleanCalls {def truth(b: Boolean): Boolean = b; def f(b: Boolean): Boolean = truth(truth(b))}";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
        source,
        "BooleanCalls.scala",
        SOURCE_ID,
        |paired| {
            let error = paired
                .tasty_plan
                .pickle_with_limits(PickleLimits {
                    projection: ProjectionLimits {
                        max_expression_depth: 0,
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .unwrap_err();
            assert_eq!(
                error,
                PickleError::LimitExceeded {
                    kind: LimitKind::ExpressionDepth,
                    limit: 0
                }
            );
        },
    )
    .unwrap();
}

#[test]
fn boolean_calls_keep_class_identity_and_order_in_deterministic_batches() {
    let source = "package example; class First {def truth(b: Boolean): Boolean = b; def f(b: Boolean): Boolean = truth(b)}; class Second {def truth(b: Boolean): Boolean = !b; def f(b: Boolean): Boolean = this.truth(b)}";
    let first =
        dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch(source, "Batch.scala", SOURCE_ID)
            .unwrap();
    let second = dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch(
        source,
        "Batch.scala",
        SourceId::from_index(2486),
    )
    .unwrap();
    assert_eq!(first.classes().len(), 2);
    for ((a, b), name) in first
        .classes()
        .iter()
        .zip(second.classes())
        .zip(["First", "Second"])
    {
        assert_eq!(a.classfile_path(), format!("example/{name}.class"));
        assert_eq!(a.classfile_bytes(), b.classfile_bytes());
        assert_eq!(a.tasty_bytes(), b.tasty_bytes());
    }
}
