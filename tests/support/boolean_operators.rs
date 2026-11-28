use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::{PairedJvmTastyArtifacts, emit_paired_jvm_tasty_class};
const SOURCE: &str = include_str!("../fixtures/jvm_tasty_boolean_operators/BooleanOperators.scala");

fn emit(methods: &str) -> PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class BooleanOperators {{ {methods} }}"),
        "BooleanOperators.scala",
        SourceId::from_index(2488),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn complete_operator_fixture_is_deterministic_and_preserves_control_flow() {
    use dotty_tasty::tasty::*;
    let first =
        emit_paired_jvm_tasty_class(SOURCE, "BooleanOperators.scala", SourceId::from_index(2488))
            .unwrap();
    let second =
        emit_paired_jvm_tasty_class(SOURCE, "BooleanOperators.scala", SourceId::from_index(2489))
            .unwrap();
    assert_eq!(first.classfile_bytes(), second.classfile_bytes());
    assert_eq!(first.tasty_bytes(), second.tasty_bytes());
    let file = TastyFile::parse_and_validate_scala_3_9(first.tasty_bytes()).unwrap();
    let index = file.ast_address_index().unwrap();
    assert_eq!(
        index
            .iter_nodes_with_tag(dotty_tasty::tasty::WHILE_TAG)
            .count(),
        7
    );
}

fn publish_fixture(label: &str) -> std::path::PathBuf {
    let pair =
        emit_paired_jvm_tasty_class(SOURCE, "BooleanOperators.scala", SourceId::from_index(2488))
            .unwrap();
    let directory = std::env::temp_dir().join(format!(
        "dotty-boolean-operators-{label}-{}",
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
fn boolean_operators_values_and_effects_execute_on_jdk21() {
    if !super::jdk_available() {
        return;
    }
    let directory = publish_fixture("java");
    let client = directory.join("JavaConsumer.java");
    std::fs::write(
        &client,
        include_str!("../fixtures/jvm_tasty_boolean_operators/JavaConsumer.java"),
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
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "OK 448 checks");
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn pinned_scala_consumes_original_and_recompiled_boolean_operators_artifacts() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let directory = publish_fixture("scala");
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/jvm_tasty_boolean_operators/run.py"
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
fn each_boolean_operator_composition_emits() {
    for line in SOURCE
        .lines()
        .map(str::trim)
        .filter(|s| s.starts_with("def "))
    {
        emit(line);
    }
}

#[test]
fn strict_boolean_opcodes_descriptors_frames_and_signed_owners_are_exact() {
    use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
    use dotty_tasty::tasty::*;
    let pair = emit(
        "def and(a: Boolean,b: Boolean): Boolean = a & b; def or(a: Boolean,b: Boolean): Boolean = a | b; def xor(a: Boolean,b: Boolean): Boolean = a ^ b; def eq(a: Boolean,b: Boolean): Boolean = a == b; def ne(a: Boolean,b: Boolean): Boolean = a != b",
    );
    let class = ClassFile::decode(&mut Reader::new(pair.classfile_bytes())).unwrap();
    for (name, bytes) in [
        ("and", &[0x1b, 0x1c, 0x7e, 0xac][..]),
        ("or", &[0x1b, 0x1c, 0x80, 0xac][..]),
        ("xor", &[0x1b, 0x1c, 0x82, 0xac][..]),
        ("eq", &[0x1b, 0x1c, 0x9f, 0, 7, 3, 0xa7, 0, 4, 4, 0xac][..]),
        ("ne", &[0x1b, 0x1c, 0xa0, 0, 7, 3, 0xa7, 0, 4, 4, 0xac][..]),
    ] {
        let method = class
            .methods
            .iter()
            .find(|m| class.constant_pool.utf8(m.name_index) == Ok(name))
            .unwrap();
        assert_eq!(
            class.constant_pool.utf8(method.descriptor_index),
            Ok("(ZZ)Z")
        );
        let [Attribute::Code(code)] = method.attributes.as_slice() else {
            panic!()
        };
        assert_eq!(code.code, bytes, "{name}");
        assert_eq!((code.max_stack, code.max_locals), (2, 3));
        if name == "eq" || name == "ne" {
            let raw = code
                .attributes
                .iter()
                .find_map(|a| match a {
                    Attribute::Other(raw)
                        if class.constant_pool.utf8(raw.name_index) == Ok("StackMapTable") =>
                    {
                        Some(raw.bytes)
                    }
                    _ => None,
                })
                .unwrap();
            let mut reader = Reader::new(raw);
            assert_eq!(reader.read_u16().unwrap(), 2);
            for (delta, stack) in [(9, 0), (0, 1)] {
                assert_eq!(reader.read_u8().unwrap(), 255);
                assert_eq!(reader.read_u16().unwrap(), delta);
                assert_eq!(reader.read_u16().unwrap(), 3);
                assert_eq!(reader.read_u8().unwrap(), 7);
                reader.read_u16().unwrap();
                assert_eq!(reader.read_u8().unwrap(), 1);
                assert_eq!(reader.read_u8().unwrap(), 1);
                assert_eq!(reader.read_u16().unwrap(), stack);
                if stack == 1 {
                    assert_eq!(reader.read_u8().unwrap(), 1);
                }
            }
        }
    }
    let file = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
    let index = file.ast_address_index().unwrap();
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    fn unshare<'a>(mut tree: RawTree<'a>, payload: &'a [u8]) -> RawTree<'a> {
        for _ in 0..64 {
            let RawTree::Leaf(SimpleTerm {
                tag: SHAREDTYPE_TAG | SHAREDTERM_TAG,
                value: TermValue::AstRef(address),
                ..
            }) = &tree
            else {
                return tree;
            };
            tree = RawTree::decode(&mut dotty_tasty::reader::Reader::new(
                &payload[*address as usize..],
            ))
            .unwrap();
        }
        panic!("shared type chain");
    }
    let mut seen = Vec::new();
    for node in index.iter().filter(|node| node.tag == SELECTIN_TAG) {
        let selected = node.decode_select_in().unwrap();
        let Some(RenderedSignedName::Signed {
            original,
            signature,
        }) = file.names().render_signed_name(selected.name).unwrap()
        else {
            panic!()
        };
        if ["&", "|", "^", "==", "!="].contains(&original.as_str()) {
            assert_eq!(
                signature,
                RenderedNameSignature {
                    result: "scala.Boolean".into(),
                    parameters: vec![RenderedParamSig::TermParameter("scala.Boolean".into())]
                }
            );
            let RawTree::NatAst {
                tag: TYPEREF_TAG,
                value,
                child,
                ..
            } = unshare(selected.underlying_type, payload)
            else {
                panic!()
            };
            assert_eq!(file.names().render(value).unwrap(), "Boolean");
            let RawTree::Leaf(SimpleTerm {
                tag: TERMREFPKG_TAG,
                value: TermValue::NameRef(package),
                ..
            }) = unshare(*child, payload)
            else {
                panic!()
            };
            assert_eq!(file.names().render(package).unwrap(), "scala");
            seen.push(original);
        }
    }
    seen.sort();
    assert_eq!(seen, ["!=", "&", "==", "^", "|"]);
}

#[test]
fn boolean_and_int_equality_preserve_distinct_source_symbols_and_wire_signatures() {
    use dotty::core::{TermRefTarget, Type, ast::TreeKind};
    let source = "package example; class BooleanOperators {def boolEq(a: Boolean,b: Boolean): Boolean = a == b; def intEq(a: Int,b: Int): Boolean = a == b}";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(source,"BooleanOperators.scala",SourceId::from_index(2488),|paired| {
        let raw=paired.tasty_plan.plan();
        let boolean=raw.definitions.boolean_primitive_operator_ids().unwrap().equal;
        let int=raw.definitions.any_equality_operator_symbols(raw.store).unwrap().equal;
        assert_ne!(boolean,int);
        for (method,target) in paired.jvm_unit.classes[0].methods.iter().zip([boolean,int]) {
            let TreeKind::Apply(apply)=&raw.typed_ast.get(method.body.unwrap()).kind else {panic!()};
            assert!(matches!(raw.store.types.get(raw.typed_ast.get(apply.function).ty),Type::TermRef{target:TermRefTarget::Symbol(actual),..} if *actual==target));
        }
    }).unwrap();
    let pair = emit(
        "def boolEq(a: Boolean,b: Boolean): Boolean = a == b; def intEq(a: Int,b: Int): Boolean = a == b",
    );
    let file =
        dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
    let mut parameters = Vec::new();
    for node in file
        .ast_address_index()
        .unwrap()
        .iter()
        .filter(|node| node.tag == dotty_tasty::tasty::SELECTIN_TAG)
    {
        if let Some(dotty_tasty::tasty::RenderedSignedName::Signed {
            original,
            signature,
        }) = file
            .names()
            .render_signed_name(node.decode_select_in().unwrap().name)
            .unwrap()
            && original == "=="
        {
            parameters.push(signature.parameters);
        }
    }
    use dotty_tasty::tasty::RenderedParamSig::TermParameter;
    assert_eq!(
        parameters,
        vec![
            vec![TermParameter("scala.Boolean".into())],
            vec![TermParameter("scala.Int".into())]
        ]
    );
}

#[test]
fn strict_operators_compose_with_operand_values_mutable_flags_and_result_widths() {
    for op in ["&", "|", "^", "==", "!="] {
        for body in [
            format!("(a: Boolean) {op} (b: Boolean)"),
            format!("(if a then b else a) {op} (if b then a else b)"),
            format!("(a match {{case n => n}}) {op} (b match {{case n => n}})"),
            format!("{{var flag = a; flag = flag {op} b; flag}}"),
            format!("false match {{case n => a {op} n}}"),
        ] {
            emit(&format!("def f(a: Boolean,b: Boolean): Boolean = {body}"));
        }
        for (ty, descriptor) in [
            ("Int", "I"),
            ("Boolean", "Z"),
            ("Long", "J"),
            ("Float", "F"),
            ("Double", "D"),
        ] {
            for body in [
                format!("if a {op} b then x else y"),
                format!("(a {op} b) match {{case true => x;case false => y}}"),
            ] {
                let pair = emit(&format!(
                    "def f(a: Boolean,b: Boolean,x: {ty},y: {ty}): {ty} = {body}"
                ));
                let class = dotty_classfile::class_file::ClassFile::decode(
                    &mut dotty_classfile::reader::Reader::new(pair.classfile_bytes()),
                )
                .unwrap();
                let method = class
                    .methods
                    .iter()
                    .find(|m| class.constant_pool.utf8(m.name_index) == Ok("f"))
                    .unwrap();
                assert_eq!(
                    class.constant_pool.utf8(method.descriptor_index).unwrap(),
                    format!("(ZZ{descriptor}{descriptor}){descriptor}")
                );
            }
        }
        emit(&format!(
            "def f(a: Boolean,b: Boolean): Unit = {{var n: Int = 0; if a {op} b then n = 1 else n = 2}}"
        ));
        emit(&format!(
            "def f(a: Boolean,b: Boolean): Unit = (a {op} b) match {{case true => ();case false => ()}}"
        ));
    }
}

#[test]
fn unsupported_boolean_shapes_reject_before_artifacts() {
    for methods in [
        "def source(a: Boolean): Boolean = a; def f(a: Boolean): Boolean = source(a)",
        "def f(a: Boolean,b: Boolean): Boolean = a.&(b)",
        "def f(a: Boolean,b: Any): Boolean = a == b",
        "def f(a: Boolean,b: Boolean): Boolean = a & {return b; b}",
        "def f(a: Boolean,b: Boolean): Boolean = {return a; a} | b",
        "def f(a: Boolean,b: Boolean): Int = a match {case n if a ^ {return 3; b} => 1;case _ => 2}",
        "def f(a: Boolean,b: Boolean): Int = {val x = {a & {return 3; b}; 7}; x}",
        "def f(a: Boolean,b: Boolean): Boolean = a match {case n => n & b;case _ => n | b}",
        "def f(a: Boolean,b: Boolean): Unit = if a && b then () else ()",
        "def f(a: Int,b: Int): Int = a | b",
        "def f(b: Boolean): Boolean = true match {case n => n & b}",
    ] {
        let result = emit_paired_jvm_tasty_class(
            &format!("package example; class BooleanOperators {{{methods}}}"),
            "BooleanOperators.scala",
            SourceId::from_index(2488),
        );
        assert!(
            matches!(
                result,
                Err(dotty::jvm_tasty_artifacts::PairedArtifactError::Plan(_))
            ),
            "{methods}: {result:?}"
        );
    }
}

#[test]
fn strict_operator_plans_require_all_source_edges_and_reject_projection_limits_read_only() {
    use dotty::core::{
        SymbolId, TreeId, Typed, Untyped,
        ast::{TreeKind, UntypedNode},
    };
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
    for op in ["&", "|", "^", "==", "!="] {
        let source = format!(
            "package example; class BooleanOperators {{def f(a: Boolean,b: Boolean): Boolean = a {op} b}}"
        );
        dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(&source,"BooleanOperators.scala",SourceId::from_index(2488),|paired| {
            let raw=paired.tasty_plan.plan();
            let before=format!("{:?}{:?}{:?}",raw.store,raw.source_ast,raw.typed_ast);
            let (root,expr)=raw.source_ast.iter().find_map(|(id,node)|match &node.kind{TreeKind::PhaseSpecific(UntypedNode::InfixOp(i))=>Some((id,*i)),_=>None}).unwrap();
            for removed in [root,expr.left,expr.right] {
                let bindings=Missing{original:raw.typed_bindings,removed};
                let input=ClassStatePicklePlan{store:raw.store,definitions:raw.definitions,source_ast:raw.source_ast,typed_ast:raw.typed_ast,source_index:raw.source_index,typed_bindings:&bindings,source:raw.source.clone(),class:raw.class,class_tree:raw.class_tree,constructor:PrimaryConstructorInputPlan{symbol:raw.constructor.symbol,source_tree:raw.constructor.source_tree,parameter_clauses:raw.constructor.parameter_clauses.clone()},members:raw.members};
                let error=input.validate().and_then(|p|p.validate_method_bodies()).unwrap_err();
                if removed==root {
                    assert!(matches!(error,PickleError::MissingBody{method} if method==paired.jvm_unit.classes[0].methods[0].method),"{op}: {error:?}");
                } else {
                    assert!(matches!(error,PickleError::BodyBinding{tree,actual:None,..} if tree==removed),"{op}: {error:?}");
                }
            }
            assert_eq!(before,format!("{:?}{:?}{:?}",raw.store,raw.source_ast,raw.typed_ast));
        }).unwrap();
        let nested = format!(
            "package example; class BooleanOperators {{def f(a: Boolean,b: Boolean): Boolean = (a {op} b) || a}}"
        );
        dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
            &nested,
            "BooleanOperators.scala",
            SourceId::from_index(2488),
            |paired| {
                let raw = paired.tasty_plan.plan();
                let before = format!("{:?}{:?}{:?}", raw.store, raw.source_ast, raw.typed_ast);
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
                assert_eq!(
                    before,
                    format!("{:?}{:?}{:?}", raw.store, raw.source_ast, raw.typed_ast)
                );
            },
        )
        .unwrap();
    }
}

#[test]
fn batch_boolean_operators_keep_class_and_formal_identity() {
    let source = "package example; class First {def f(a: Boolean,b: Boolean): Boolean = a & b}; class Second {def f(a: Boolean,b: Boolean): Boolean = a == b}";
    let batch = dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch(
        source,
        "Batch.scala",
        SourceId::from_index(2488),
    )
    .unwrap();
    assert_eq!(batch.classes().len(), 2);
    assert_eq!(batch.classes()[0].classfile_path(), "example/First.class");
    assert_eq!(batch.classes()[1].classfile_path(), "example/Second.class");
    let second = dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch(
        source,
        "Batch.scala",
        SourceId::from_index(2489),
    )
    .unwrap();
    for (a, b) in batch.classes().iter().zip(second.classes()) {
        assert_eq!(a.classfile_bytes(), b.classfile_bytes());
        assert_eq!(a.tasty_bytes(), b.tasty_bytes());
    }
}
