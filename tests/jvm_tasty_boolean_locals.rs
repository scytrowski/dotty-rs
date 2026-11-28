#[test]
fn pinned_oracle_preserves_boolean_local_values_mutation_and_flow() {
    let oracle = include_str!("fixtures/jvm_tasty_boolean_locals/oracle.txt");
    for shape in [
        "scala.Boolean",
        "val flag",
        "var ",
        "while (",
        "unary_!",
        "&&",
        "||",
        "return",
        "tailSingleton",
        "tailConstant",
        "siblingScopes",
    ] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}

use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class;

fn emit(methods: &str) -> dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class BooleanLocals {{ {methods} }}"),
        "BooleanLocals.scala",
        SourceId::from_index(2477),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn immutable_boolean_locals_and_value_compositions_emit_complete_pairs() {
    for method in [
        "def f(x: Boolean): Boolean = { val flag: Boolean = x; flag }",
        "def f(x: Boolean): Boolean = { val flag = x; flag }",
        "def f(): Boolean = { val flag = true; flag }",
        "def f(x: Boolean): Boolean = { val flag = if x then false else true; flag }",
        "def f(x: Boolean, y: Boolean): Boolean = { val flag = x && y; flag }",
        "def f(x: Int): Boolean = { val flag = x > 0; flag }",
        "def f(x: Boolean): Boolean = { val flag = x match { case true => false; case _ => true }; flag }",
        "def f(x: Boolean): Int = { val flag = x; flag match { case true => 1; case _ => 2 } }",
        "def f(x: Boolean): Boolean = x match { case n => { val flag = n; flag } }",
        "def f(x: Boolean): Boolean = { val flag = x; (flag: Boolean) }",
        "def f(x: Boolean): Boolean = { val flag = x; return flag }",
        "def f(x: Boolean, y: Boolean): Boolean = { val flag = y; if x then { return true; flag } else false }",
        "def f(x: Boolean, n: Long): Long = { val flag = x; if flag then n else 0L }",
        "def f(x: Boolean, n: Float): Float = { val flag = x; if flag then n else 0.0f }",
        "def f(x: Boolean, n: Double): Double = { val flag = x; if flag then n else 0.0 }",
        "def f(x: Boolean): Int = if x then { val flag = true; if flag then 1 else 2 } else { val flag = false; if flag then 3 else 4 }",
        "def f(flag: Boolean): Boolean = { val flag = false; flag }",
        "def f(x: Boolean): Int = ({ x }) match { case true => 1; case _ => 2 }",
    ] {
        emit(method);
    }
}

#[test]
fn mutable_boolean_values_loops_short_circuit_effects_and_return_tails_compose() {
    for method in [
        "def f(x: Boolean): Boolean = { var flag: Boolean = false; flag = x; flag }",
        "def f(x: Boolean): Boolean = { var flag = x; flag = !flag; flag }",
        "def f(x: Boolean, y: Boolean): Boolean = { var flag = x; if y then flag = false else flag = true; flag }",
        "def f(x: Boolean): Int = { var flag = x; var count = 0; while flag do { count = count + 1; flag = false }; count }",
        "def f(x: Boolean): Boolean = { var flag = false; val result = x && { flag = true; false }; flag || result }",
        "def f(x: Boolean): Boolean = { var flag = false; val result = x || { flag = true; false }; flag && !result }",
        "def f(x: Boolean, n: Int): Int = { var flag = x; n match { case 1 if { flag = !flag; flag } => 10; case _ => if flag then 20 else 30 } }",
        "def f(x: Boolean): Boolean = { var flag = x; flag = x match { case true => false; case _ => true }; flag }",
        "def f(x: Boolean): Int = { var flag = x; while flag do { flag = false; return 1 }; 0 }",
        "def f(x: Boolean): Unit = { var flag = x; flag = false; () }",
        "def f(x: Boolean): Unit = { var flag = x; while flag do { flag = false }; () }",
        "def f(x: Boolean): Unit = { var flag = x; if flag then flag = false else (); () }",
        "def f(x: Boolean): Int = x match { case n @ true => { while n do { return 1 }; 2 }; case _ => 3 }",
        "def f(x: Boolean): Int = x match { case n @ false => { while n do { return 1 }; 2 }; case _ => 3 }",
        "def f(x: Boolean): Boolean = x match { case n @ true => { return false; n }; case _ => true }",
        "def f(): Boolean = true match { case n => { return false; n } }",
        "def f(): Int = true match { case n => { while n do { return 1 }; 2 } }",
        "def f(x: Boolean, y: Boolean): Boolean = { val flag = y; if x then { return true; flag } else { return false; flag } }",
    ] {
        emit(method);
    }
    emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_boolean_locals/BooleanLocals.scala"),
        "BooleanLocals.scala",
        SourceId::from_index(2477),
    )
    .unwrap();
}

#[test]
fn forged_boolean_declaration_assignment_condition_and_join_fail_read_only() {
    use dotty::core::{
        Constant, SymbolKind, TermRefTarget, Type, Typed, Untyped,
        ast::{AstArena, TreeKind},
    };
    use dotty_tasty_pickler::{
        ClassStatePicklePlan, PickleError, PrimaryConstructorInputPlan, ReferenceError,
    };
    let source = "package example; class BooleanLocals { def f(x: Boolean): Boolean = { var flag = x; val other = false; flag = other; if flag then true else false }; def g(x: Boolean): Boolean = { val flag = x; flag } }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
        source,
        "BooleanLocals.scala",
        SourceId::from_index(2477),
        |paired| {
            let raw = paired.tasty_plan.plan();
            let mark = raw.store.checkpoint();
            let before = format!("{:?}{:?}{:?}", raw.store, raw.source_ast, raw.typed_ast);
            for mode in 0..10 {
                let mut ast = AstArena::<Typed>::new();
                for (id, n) in raw.typed_ast.iter() {
                    assert_eq!(id, ast.alloc(n.clone()));
                }
                let mut source_ast = AstArena::<Untyped>::new();
                for (id, n) in raw.source_ast.iter() {
                    assert_eq!(id, source_ast.alloc(n.clone()));
                }
                let (decl, local) = ast
                    .iter()
                    .find_map(|(id, n)| {
                        if let TreeKind::ValDef(v) = n.kind.clone() {
                            Some((id, v))
                        } else {
                            None
                        }
                    })
                    .unwrap();
                let (assignment, assign) = ast
                    .iter()
                    .find_map(|(id, n)| {
                        if let TreeKind::Assign(a) = n.kind {
                            Some((id, a))
                        } else {
                            None
                        }
                    })
                    .unwrap();
                let (join, conditional) = ast
                    .iter()
                    .find_map(|(id, n)| {
                        if let TreeKind::If(i) = n.kind {
                            Some((id, i))
                        } else {
                            None
                        }
                    })
                    .unwrap();
                match mode {
                    0 => ast.get_mut(decl).ty = raw.definitions.int,
                    1 => ast.get_mut(local.tpt).ty = raw.definitions.int,
                    2 => ast.get_mut(assignment).ty = raw.definitions.boolean,
                    3 => ast.get_mut(assign.rhs).ty = raw.definitions.int,
                    4 => ast.get_mut(conditional.cond).ty = raw.definitions.int,
                    5 => ast.get_mut(join).ty = raw.definitions.nothing_type,
                    6 => {
                        let foreign = ast
                            .iter()
                            .filter_map(|(_, n)| {
                                let Type::TermRef {
                                    target: TermRefTarget::Symbol(s),
                                    ..
                                } = raw.store.types.try_get(n.ty)?
                                else {
                                    return None;
                                };
                                (matches!(n.kind, TreeKind::Ident(_))
                                    && raw.store.symbols.get(*s).kind == SymbolKind::Local)
                                    .then_some(n.ty)
                            })
                            .last()
                            .unwrap();
                        ast.get_mut(assign.lhs).ty = foreign;
                    }
                    8 => {
                        let id = source_ast.iter().find_map(|(id,n)| matches!(&n.kind, TreeKind::ValDef(v) if v.metadata.modifiers.contains(&dotty::core::ast::Modifier::Var)).then_some(id)).unwrap();
                        let TreeKind::ValDef(v) = &mut source_ast.get_mut(id).kind else { panic!() };
                        v.metadata.modifiers.clear();
                    }
                    9 => {
                        let TreeKind::Assign(a) = &mut ast.get_mut(assignment).kind else { panic!() };
                        a.rhs = a.lhs;
                    }
                    _ => {
                        let original = ast.get(conditional.then_branch).ty;
                        assert!(matches!(
                            raw.store.types.get(original),
                            Type::Constant(Constant::Boolean(true))
                        ));
                        ast.get_mut(conditional.then_branch).ty = raw.definitions.boolean;
                    }
                }
                let input = ClassStatePicklePlan {
                    store: raw.store,
                    definitions: raw.definitions,
                    typed_ast: &ast,
                    source_ast: &source_ast,
                    source_index: raw.source_index,
                    typed_bindings: raw.typed_bindings,
                    source: raw.source.clone(),
                    class: raw.class,
                    class_tree: raw.class_tree,
                    members: raw.members,
                    constructor: PrimaryConstructorInputPlan {
                        symbol: raw.constructor.symbol,
                        source_tree: raw.constructor.source_tree,
                        parameter_clauses: raw.constructor.parameter_clauses.clone(),
                    },
                };
                let error = input
                    .validate()
                    .and_then(|p| p.validate_method_bodies())
                    .err().unwrap_or_else(|| panic!("accepted forged mode {mode}"));
                match mode {
                    1 => assert!(
                        matches!(error, PickleError::UnsupportedTypeReference { .. }),
                        "{mode}: {error:?}"
                    ),
                    2 => assert!(
                        matches!(
                            error,
                            PickleError::Unsupported {
                                feature: dotty_tasty_pickler::UnsupportedFeature::BodyExpression,
                                ..
                            }
                        ),
                        "{mode}: {error:?}"
                    ),
                    8 => assert!(matches!(error, PickleError::Unsupported { feature: dotty_tasty_pickler::UnsupportedFeature::SymbolMetadata, .. }), "{mode}: {error:?}"),
                    9 => assert!(matches!(error, PickleError::BodyBinding { .. }), "{mode}: {error:?}"),
                    6 => assert!(
                        matches!(
                            error,
                            PickleError::Reference {
                                reason: ReferenceError::NotRegistered,
                                ..
                            }
                        ),
                        "{mode}: {error:?}"
                    ),
                    _ => assert!(
                        matches!(error, PickleError::BodyType { .. }),
                        "{mode}: {error:?}"
                    ),
                }
                assert_eq!(raw.store.checkpoint(), mark);
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
fn paired_boolean_local_proofs_require_exact_declarations_symbols_and_mutability() {
    use dotty::jvm::JvmCompilationUnit;
    use dotty_jvm_tasty_contract::{JvmTastyContractError, validate_jvm_tasty_class};
    let source = "package example; class BooleanLocals { def f(x: Boolean): Boolean = { var flag = x; flag = !flag; flag }; def g(x: Boolean): Boolean = { val flag = x; flag } }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
        source,
        "BooleanLocals.scala",
        SourceId::from_index(2477),
        |paired| {
            let original = &paired.jvm_unit;
            let mark = original.store.checkpoint();
            for mode in 0..4 {
                let mut classes = original.classes.clone();
                let other = classes[0].methods[1].locals[0];
                let target = &mut classes[0].methods[0];
                match mode {
                    0 => target.locals.clear(),
                    1 => target.locals[0].mutable = false,
                    2 => target.locals[0].symbol = other.symbol,
                    _ => target.locals[0].declaration = other.declaration,
                }
                let unit = JvmCompilationUnit {
                    store: original.store,
                    typed_ast: original.typed_ast,
                    classes,
                };
                let error = validate_jvm_tasty_class(&unit, paired.tasty_plan)
                    .err()
                    .unwrap();
                let accepted = match mode {
                    2 => matches!(
                        error,
                        JvmTastyContractError::JvmPlan(
                            dotty::jvm::JvmCodegenError::WrongOwner { .. }
                        )
                    ),
                    3 => matches!(
                        error,
                        JvmTastyContractError::JvmPlan(
                            dotty::jvm::JvmCodegenError::LocalDeclarationOutsideMethod { .. }
                        )
                    ),
                    1 => matches!(
                        error,
                        JvmTastyContractError::JvmPlan(
                            dotty::jvm::JvmCodegenError::LocalMutabilityMismatch { .. }
                        )
                    ),
                    _ => matches!(error, JvmTastyContractError::LocalBindingMismatch { .. }),
                };
                assert!(accepted, "mode {mode}: {error:?}");
                assert_eq!(original.store.checkpoint(), mark);
            }
        },
    )
    .unwrap();
}

#[test]
fn pinned_scala_consumes_and_recompiles_original_rust_boolean_local_tasty() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_boolean_locals/BooleanLocals.scala"),
        "BooleanLocals.scala",
        SourceId::from_index(2477),
    )
    .unwrap();
    let directory =
        std::env::temp_dir().join(format!("dotty-boolean-local-scala-{}", std::process::id()));
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
            "/tests/fixtures/jvm_tasty_boolean_locals/run.py"
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
            "mandatory boolean-local runtime gate requires JDK 21+"
        );
    }
    available
}

fn run_java(pair: &dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts, name: &str, source: &str) {
    let directory = std::env::temp_dir().join(format!(
        "dotty-boolean-locals-{}-{name}",
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
        "boolean local oracle ok"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn original_rust_boolean_locals_execute_on_jdk21() {
    if !jdk_available() {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_boolean_locals/BooleanLocals.scala"),
        "BooleanLocals.scala",
        SourceId::from_index(2477),
    )
    .unwrap();
    run_java(
        &pair,
        "JavaConsumer",
        include_str!("fixtures/jvm_tasty_boolean_locals/JavaConsumer.java"),
    );
}

#[test]
fn boolean_local_artifacts_are_deterministic_in_fresh_sessions() {
    let source = include_str!("fixtures/jvm_tasty_boolean_locals/BooleanLocals.scala");
    let a = emit_paired_jvm_tasty_class(source, "BooleanLocals.scala", SourceId::from_index(2477))
        .unwrap();
    let b = emit_paired_jvm_tasty_class(source, "BooleanLocals.scala", SourceId::from_index(2478))
        .unwrap();
    assert_eq!(a.classfile_bytes(), b.classfile_bytes());
    assert_eq!(a.tasty_bytes(), b.tasty_bytes());
}

#[test]
fn unsupported_boolean_flag_shapes_do_not_publish_artifact_pairs() {
    for methods in [
        "def f(x: Boolean): Boolean = { val flag = x; flag = false; flag }",
        "def f(x: Boolean): Boolean = { var flag = x; flag = 1; flag }",
        "def f(x: Boolean): Boolean = { if x then { val flag = true; () } else (); flag }",
        "def f(x: Boolean): Boolean = { val flag = x; if x then { val flag = false; flag } else flag }",
        "def f(x: Boolean): Boolean = { val flag: Boolean = { return x; false }; flag }",
        "def f(x: Boolean): Boolean = { var flag = false; flag = { return x; false }; flag }",
        "def f(x: Boolean): Boolean = x && { return false; true }",
        "def f(x: Boolean): Int = if { return 1; x } then 2 else 3",
        "def f(x: Boolean): Int = x match { case true if { return 1; x } => 2; case _ => 3 }",
        "def f(x: Boolean): Int = x match { case true => 1 }",
    ] {
        let result = emit_paired_jvm_tasty_class(
            &format!("package example; class BooleanLocals {{ {methods} }}"),
            "BooleanLocals.scala",
            SourceId::from_index(2477),
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
fn boolean_locals_keep_z_descriptors_integer_slots_and_return_opcodes() {
    use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
    let pair = emit(
        "def copy(x: Boolean): Boolean = { val flag = x; flag }; def mixed(n: Long, d: Double, x: Boolean): Boolean = { var flag = x; flag = !flag; flag }; def returned(x: Boolean): Boolean = { return true; x }",
    );
    let file = ClassFile::decode(&mut Reader::new(pair.classfile_bytes())).unwrap();
    for (name, descriptor, slots, bytes) in [
        ("copy", "(Z)Z", 3, &[0x1b, 0x3d, 0x1c, 0xac][..]),
        (
            "mixed",
            "(JDZ)Z",
            7,
            &[
                0x15, 5, 0x36, 6, 0x15, 6, 0x04, 0x82, 0x36, 6, 0x15, 6, 0xac,
            ][..],
        ),
        ("returned", "(Z)Z", 2, &[0x04, 0xac][..]),
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
        assert_eq!(code.max_locals, slots, "{name}");
        assert_eq!(code.code, bytes, "{name}");
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
fn boolean_flag_loop_has_exact_integer_verification_frames() {
    use dotty_classfile::{class_file::ClassFile, reader::Reader};
    let pair = emit(
        "def f(x: Boolean): Int = { var flag = x; var count = 0; while flag do { count = count + 1; flag = false }; count }",
    );
    let file = ClassFile::decode(&mut Reader::new(pair.classfile_bytes())).unwrap();
    assert_eq!(
        frames(&file, "f"),
        [
            (7, vec![7, 1, 1, 1], vec![]),
            (20, vec![7, 1, 1, 1], vec![])
        ]
    );
}

#[test]
fn legacy_boolean_local_proofs_and_expression_limits_are_checked_read_only() {
    use dotty::core::ast::TreeKind;
    use dotty_tasty_pickler::{
        ClassMemberPicklePlan, DefaultConstructorPlan, LimitKind, MethodPicklePlan, PickleError,
        PickleLimits, PicklePlan, ProjectionLimits, ValidationLimits,
    };
    let source = "package example; class BooleanLocals { def f(x: Boolean): Boolean = if x then { val flag = true; flag } else { val flag = false; flag } }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
        source,
        "BooleanLocals.scala",
        SourceId::from_index(2477),
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
            let original = raw
                .typed_bindings
                .local_value_bindings(raw.source.id, *symbol);
            assert_eq!(original.len(), 2);
            assert_ne!(original[0].scope, original[1].scope);
            let mark = raw.store.checkpoint();
            for mode in 0..7 {
                let mut locals = original.clone();
                match mode {
                    0 => locals.clear(),
                    1 => locals.push(locals[0]),
                    2 => locals[0].scope = locals[1].scope,
                    3 => locals[0].typed_declaration = locals[1].typed_declaration,
                    _ => {}
                }
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
                    local_values: &locals,
                    constructor: DefaultConstructorPlan {
                        symbol: raw.constructor.symbol,
                        source_tree: raw.constructor.source_tree,
                        parameter_clauses: raw.constructor.parameter_clauses.clone(),
                    },
                };
                let error = if mode == 4 {
                    input
                        .validate()
                        .unwrap()
                        .pickle_with_limits(PickleLimits {
                            projection: ProjectionLimits {
                                max_expression_depth: 0,
                                ..Default::default()
                            },
                            ..Default::default()
                        })
                        .unwrap_err()
                } else if mode == 5 {
                    input
                        .validate_with_limits(ValidationLimits {
                            max_expression_depth: 0,
                            ..Default::default()
                        })
                        .err()
                        .unwrap()
                } else if mode == 6 {
                    input
                        .validate_with_limits(ValidationLimits {
                            max_items: 5,
                            ..Default::default()
                        })
                        .err()
                        .unwrap()
                } else {
                    input.validate().err().unwrap()
                };
                match mode {
                    0 => assert!(
                        matches!(
                            error,
                            PickleError::Unsupported {
                                feature: dotty_tasty_pickler::UnsupportedFeature::BodyExpression,
                                ..
                            }
                        ),
                        "{mode}: {error:?}"
                    ),
                    1 => assert!(
                        matches!(error, PickleError::DuplicateDeclaration { .. }),
                        "{mode}: {error:?}"
                    ),
                    2 => assert!(
                        matches!(error, PickleError::ScopeMembers { .. }),
                        "{mode}: {error:?}"
                    ),
                    3 => assert!(
                        matches!(error, PickleError::BodyBinding { .. }),
                        "{mode}: {error:?}"
                    ),
                    4 | 5 => assert_eq!(
                        error,
                        PickleError::LimitExceeded {
                            kind: LimitKind::ExpressionDepth,
                            limit: 0
                        }
                    ),
                    _ => assert_eq!(
                        error,
                        PickleError::LimitExceeded {
                            kind: LimitKind::Items,
                            limit: 5
                        }
                    ),
                }
                assert_eq!(raw.store.checkpoint(), mark);
            }
        },
    )
    .unwrap();
}
