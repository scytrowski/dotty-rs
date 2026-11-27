use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::{PairedJvmTastyArtifacts, emit_paired_jvm_tasty_class};

const SOURCE: &str =
    include_str!("../fixtures/jvm_tasty_numeric_ascriptions/NumericAscriptions.scala");

fn emit(methods: &str) -> PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class NumericAscriptions {{ {methods} }}"),
        "NumericAscriptions.scala",
        SourceId::from_index(2479),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn numeric_ascriptions_retain_each_typed_wrapper_and_primitive_type_syntax() {
    use dotty_tasty::tasty::*;
    for (ty, literal, expected) in [
        (
            "Long",
            "9223372036854775807L",
            ConstantValue::Long(i64::MAX),
        ),
        ("Float", "-0.0f", ConstantValue::FloatBits(0x80000000)),
        ("Double", "4.9e-324", ConstantValue::DoubleBits(1)),
    ] {
        let pair = emit(&format!("def f(): {ty} = (({literal}: {ty}): {ty})"));
        let file = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
        let index = file.ast_address_index().unwrap();
        let wrappers: Vec<_> = index.iter().filter(|n| n.tag == TYPED_TAG).collect();
        assert_eq!(wrappers.len(), 2);
        for wrapper in &wrappers {
            let typed = wrapper.decode_typed().unwrap();
            let RawTree::NatAst { tag, value, .. } = typed.type_tree else {
                panic!("explicit type syntax")
            };
            assert_eq!(tag, IDENTTPT_TAG);
            assert_eq!(file.render_name(value).unwrap(), ty);
        }
        let typed = wrappers[1].decode_typed().unwrap();
        let RawTree::Leaf(literal) = typed.expression else {
            panic!("literal")
        };
        assert_eq!(literal.constant_value().unwrap(), Some(expected));
    }
}

#[test]
fn numeric_ascription_fixture_keeps_original_wrappers_and_is_deterministic() {
    let first = emit_paired_jvm_tasty_class(
        SOURCE,
        "NumericAscriptions.scala",
        SourceId::from_index(2479),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_class(
        SOURCE,
        "NumericAscriptions.scala",
        SourceId::from_index(2480),
    )
    .unwrap();
    assert_eq!(first.classfile_bytes(), second.classfile_bytes());
    assert_eq!(first.tasty_bytes(), second.tasty_bytes());
    let file =
        dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(first.tasty_bytes()).unwrap();
    let index = file.ast_address_index().unwrap();
    assert_eq!(
        index
            .iter_nodes()
            .filter(|n| n.tag == dotty_tasty::tasty::TYPED_TAG)
            .count(),
        // The source has 83 explicit wrappers. Scala also synthesizes widening
        // wrappers around some local reads; Rust preserves its canonical trees.
        83
    );
}

#[test]
fn annotated_values_compose_with_guards_nested_matches_and_abrupt_branches() {
    for (ty, literal) in [("Long", "7L"), ("Float", "0.5f"), ("Double", "0.5")] {
        for body in [
            format!("if b then return (x: {ty}) else return ({literal}: {ty})"),
            format!(
                "b match {{ case true => return (x: {ty}); case false => return ({literal}: {ty}) }}"
            ),
            format!(
                "n match {{ case k @ 1 if (b: Boolean) => (x: {ty}); case _ => ({literal}: {ty}) }}"
            ),
            format!(
                "((n match {{ case 1 => ((b match {{ case true => (x: {ty}); case false => ({literal}: {ty}) }}): {ty}); case _ => ({literal}: {ty}) }}): {ty})"
            ),
        ] {
            emit(&format!(
                "def f(x: {ty}, b: Boolean, n: Int): {ty} = {body}"
            ));
        }
    }
}

#[test]
fn annotated_parameters_keep_ordered_descriptors_slots_and_exact_return_code() {
    use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
    let pair = emit(
        "def l(i: Int, a: Float, x: Long, d: Double, b: Boolean): Long = (x: Long); def f(i: Int, a: Float, x: Long, d: Double, b: Boolean): Float = (a: Float); def d(i: Int, a: Float, x: Long, d: Double, b: Boolean): Double = (d: Double)",
    );
    let file = ClassFile::decode(&mut Reader::new(pair.classfile_bytes())).unwrap();
    for (name, descriptor, stack, bytes) in [
        ("l", "(IFJDZ)J", 2, &[0x21, 0xad][..]),
        ("f", "(IFJDZ)F", 1, &[0x24, 0xae][..]),
        ("d", "(IFJDZ)D", 2, &[0x18, 5, 0xaf][..]),
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
        assert_eq!(code.max_locals, 8);
        assert_eq!(code.max_stack, stack);
        assert_eq!(code.code, bytes);
    }
}

#[test]
fn malformed_numeric_ascription_trees_and_foreign_formals_fail_read_only() {
    use dotty::core::{
        Typed, Untyped,
        ast::{AstArena, TreeKind},
    };
    use dotty_tasty_pickler::{ClassStatePicklePlan, PickleError, PrimaryConstructorInputPlan};
    for ty in ["Long", "Float", "Double"] {
        let source = format!(
            "package example; class NumericAscriptions {{ def f(x: {ty}): {ty} = (x: {ty}); def other(x: {ty}): {ty} = x }}"
        );
        dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
            &source,
            "NumericAscriptions.scala",
            SourceId::from_index(2479),
            |paired| {
                let raw = paired.tasty_plan.plan();
                let checkpoint = raw.store.checkpoint();
                for mode in 0..7 {
                    let mut ast = AstArena::<Typed>::new();
                    for (id, node) in raw.typed_ast.iter() {
                        assert_eq!(id, ast.alloc(node.clone()));
                    }
                    let source_ast: &AstArena<Untyped> = raw.source_ast;
                    let root = paired.jvm_unit.classes[0].methods[0].body.unwrap();
                    let TreeKind::Typed(wrapper) = ast.get(root).kind else {
                        panic!()
                    };
                    let wrong = if ty == "Double" {
                        raw.definitions.long
                    } else {
                        raw.definitions.double
                    };
                    match mode {
                        0 => ast.get_mut(wrapper.tpt).ty = wrong,
                        1 => ast.get_mut(wrapper.tpt).kind = ast.get(wrapper.expr).kind.clone(),
                        2 => ast.get_mut(root).ty = wrong,
                        3 => ast.get_mut(wrapper.expr).ty = wrong,
                        4 | 5 => {
                            let TreeKind::Typed(value) = &mut ast.get_mut(root).kind else {
                                panic!()
                            };
                            value.expr = if mode == 4 { value.tpt } else { root };
                        }
                        6 => {
                            let foreign = paired.jvm_unit.classes[0].methods[1].body.unwrap();
                            ast.get_mut(wrapper.expr).ty = ast.get(foreign).ty;
                        }
                        _ => unreachable!(),
                    }
                    let before = format!("{:?}{:?}{:?}", raw.store, source_ast, ast);
                    let input = ClassStatePicklePlan {
                        store: raw.store,
                        definitions: raw.definitions,
                        source_ast,
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
                    let expected = match mode {
                        0..=3 => matches!(error, PickleError::BodyType { .. }),
                        4 | 5 => matches!(error, PickleError::BodyBinding { .. }),
                        6 => matches!(
                            error,
                            PickleError::Reference {
                                reason: dotty_tasty_pickler::ReferenceError::NotRegistered,
                                ..
                            }
                        ),
                        _ => false,
                    };
                    assert!(expected, "{ty} mode {mode}: {error:?}");
                    let unit = dotty::jvm::JvmCompilationUnit {
                        store: raw.store,
                        typed_ast: &ast,
                        classes: paired.jvm_unit.classes.clone(),
                    };
                    let error =
                        dotty::jvm::emit_class(&unit, raw.class, raw.definitions).unwrap_err();
                    use dotty::jvm::JvmCodegenError;
                    let expected = match mode {
                        0..=2 => matches!(
                            error,
                            JvmCodegenError::UnsupportedTypedExpression {
                                node: "inconsistent ascription type tree",
                                ..
                            }
                        ),
                        3 | 6 => matches!(error, JvmCodegenError::InvalidParameterReference { .. }),
                        4 => matches!(
                            error,
                            JvmCodegenError::UnsupportedTypedExpression {
                                node: "TypeTree",
                                ..
                            }
                        ),
                        5 => matches!(error, JvmCodegenError::ExpressionDepthExceeded { .. }),
                        _ => false,
                    };
                    assert!(expected, "{ty} mode {mode}: {error:?}");
                    assert_eq!(raw.store.checkpoint(), checkpoint);
                    assert_eq!(format!("{:?}{:?}{:?}", raw.store, source_ast, ast), before);
                }
            },
        )
        .unwrap();
    }
}

#[test]
fn unsupported_numeric_ascriptions_never_publish_a_pair() {
    for (ty, value) in [("Long", "7L"), ("Float", "0.5f"), ("Double", "0.5")] {
        for method in [
            format!("def f(x: {ty}): {ty} = ({{ return x; {value} }}: {ty})"),
            format!("def f(x: {ty}): {ty} = {{ val a = ({{ return x; {value} }}: {ty}); a }}"),
            format!("def f(x: {ty}): {ty} = {{ (x: {ty}); {value} }}"),
            format!("def f(x: {ty}, b: Boolean): {ty} = ((b match {{ case true => x }}): {ty})"),
            format!("def identity(x: {ty}): {ty} = x; def f(x: {ty}): {ty} = (identity(x): {ty})"),
        ] {
            let source = format!("package example; class NumericAscriptions {{ {method} }}");
            let error = emit_paired_jvm_tasty_class(
                &source,
                "NumericAscriptions.scala",
                SourceId::from_index(2479),
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
    for method in [
        "def f(x: Int): Long = (x: Long)",
        "def f(x: Float): Double = (x: Double)",
        "def f(x: Long): Double = (x: Double)",
    ] {
        let error = emit_paired_jvm_tasty_class(
            &format!("package example; class NumericAscriptions {{ {method} }}"),
            "NumericAscriptions.scala",
            SourceId::from_index(2479),
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                dotty::jvm_tasty_artifacts::PairedArtifactError::Plan(
                    dotty::jvm_tasty_plan::PairedPlanError::Typer(
                        dotty::typer::TyperError::ExpectedExpressionTypeMismatch { .. }
                    )
                )
            ),
            "{error:?}"
        );
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
fn annotated_numeric_match_joins_keep_exact_instructions_and_frames() {
    use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
    for (ty, descriptor, slots, load_a, load_b, ret, tag) in [
        ("Long", "(ZJJ)J", 7, 0x20, vec![0x16, 4], 0xad, 4),
        ("Float", "(ZFF)F", 5, 0x24, vec![0x25], 0xae, 2),
        ("Double", "(ZDD)D", 7, 0x28, vec![0x18, 4], 0xaf, 3),
    ] {
        let pair = emit(&format!(
            "def f(x: Boolean,a: {ty},b: {ty}): {ty} = x match {{ case true => (a: {ty}); case false => (b: {ty}) }}"
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

#[test]
fn legacy_numeric_ascriptions_obey_validation_and_encoding_limits_read_only() {
    use dotty::core::ast::TreeKind;
    use dotty_tasty_pickler::{
        ClassMemberPicklePlan, DefaultConstructorPlan, LimitKind, MethodPicklePlan, PickleError,
        PickleLimits, PicklePlan, ProjectionLimits, ValidationLimits,
    };
    let source =
        "package example; class NumericAscriptions { def f(x: Long): Long = ((x: Long): Long) }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
        source,
        "NumericAscriptions.scala",
        SourceId::from_index(2478),
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

fn publish_fixture(label: &str) -> std::path::PathBuf {
    let pair = emit_paired_jvm_tasty_class(
        SOURCE,
        "NumericAscriptions.scala",
        SourceId::from_index(2479),
    )
    .unwrap();
    let directory = std::env::temp_dir().join(format!(
        "dotty-numeric-ascriptions-{label}-{}",
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
fn numeric_ascription_values_bits_and_effects_execute_on_jdk21() {
    if !super::jdk_available() {
        return;
    }
    let directory = publish_fixture("java");
    let client = directory.join("JavaConsumer.java");
    std::fs::write(
        &client,
        include_str!("../fixtures/jvm_tasty_numeric_ascriptions/JavaConsumer.java"),
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
        "OK 2888 checks"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn pinned_scala_consumes_original_and_recompiled_numeric_ascription_artifacts() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let directory = publish_fixture("scala");
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/jvm_tasty_numeric_ascriptions/run.py"
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
