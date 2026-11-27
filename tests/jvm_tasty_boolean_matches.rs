#[test]
fn pinned_oracle_preserves_boolean_patterns_and_binding_shapes() {
    let oracle = include_str!("fixtures/jvm_tasty_boolean_matches/oracle.txt");
    for shape in [
        "scala.Boolean",
        "case true",
        "case false",
        "n @ true",
        "(true | true)",
        "return",
    ] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}

#[test]
fn paired_boolean_literal_match_emits_complete_artifacts() {
    let result = dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class(
        "package example; class BooleanMatches { def f(x: Boolean): Int = x match { case true => 1; case _ => 2 } }",
        "BooleanMatches.scala",
        dotty::core::SourceId::from_index(2476),
    );
    let pair = result.unwrap();
    assert!(!pair.classfile_bytes().is_empty());
    assert!(!pair.tasty_bytes().is_empty());
}

#[path = "support/guarded_match_plan.rs"]
mod guarded_match_plan;

#[test]
fn jvm_boolean_literals_wildcards_and_primitive_result_flows() {
    for method in [
        "def f(x: Boolean): Int = x match { case true => 1; case _ => 2 }",
        "def f(x: Boolean): Boolean = x match { case false => true; case _ => false }",
        "def f(x: Boolean): Unit = x match { case true => (); case _ => () }",
        "def f(x: Boolean, a: Long, b: Long): Long = x match { case true => a; case _ => b }",
        "def f(x: Boolean, a: Float, b: Float): Float = x match { case false => a; case _ => b }",
        "def f(x: Boolean, a: Double, b: Double): Double = x match { case true => return a; case _ => return b }",
        "def f(x: Boolean): Int = x match { case false => return 1; case _ => 2 }",
        "def f(x: Boolean, y: Boolean): Int = (x && y) match { case true => 1; case _ => 2 }",
    ] {
        guarded_match_plan::with_jvm(
            &format!("package example; class BooleanMatches {{ {method} }}"),
            |unit, defs| {
                dotty::jvm::emit_class(unit, unit.classes[0].class, defs).unwrap();
            },
        );
    }
}

#[test]
fn jvm_rejects_boolean_int_pattern_confusion() {
    use dotty::core::{
        Typed,
        ast::{AstArena, TreeKind},
    };
    guarded_match_plan::with_jvm(
        "package example; class BooleanMatches { def f(x: Boolean): Int = x match { case true => 1; case _ => 2 } }",
        |unit, defs| {
            for mode in 0..3 {
                let mut ast = AstArena::<Typed>::new();
                for (id, node) in unit.typed_ast.iter() {
                    assert_eq!(id, ast.alloc(node.clone()));
                }
                let pattern = ast
                    .iter()
                    .find_map(|(_, n)| {
                        if let TreeKind::CaseDef(c) = n.kind {
                            Some(c.pattern)
                        } else {
                            None
                        }
                    })
                    .unwrap();
                let error_node = match mode {
                    0 => {
                        ast.get_mut(pattern).ty = defs.int;
                        "inconsistent Boolean match literal"
                    }
                    1 => {
                        let literal = ast
                            .iter()
                            .find_map(|(id, n)| {
                                matches!(
                                    n.kind,
                                    TreeKind::Literal(dotty::core::ast::Literal {
                                        value: dotty::core::Constant::Int(1)
                                    })
                                )
                                .then_some(id)
                            })
                            .unwrap();
                        ast.get_mut(pattern).kind = ast.get(literal).kind.clone();
                        ast.get_mut(pattern).ty = ast.get(literal).ty;
                        "non-Boolean match pattern type"
                    }
                    _ => {
                        let wildcard = ast
                            .iter()
                            .find_map(|(_, n)| {
                                if let TreeKind::CaseDef(c) = n.kind {
                                    (c.pattern != pattern).then_some(c.pattern)
                                } else {
                                    None
                                }
                            })
                            .unwrap();
                        ast.get_mut(wildcard).ty = defs.int;
                        "non-Boolean match pattern type"
                    }
                };
                let invalid = dotty::jvm::JvmCompilationUnit {
                    store: unit.store,
                    typed_ast: &ast,
                    classes: unit.classes.clone(),
                };
                assert!(
                    matches!(dotty::jvm::emit_class(&invalid, unit.classes[0].class, defs), Err(dotty::jvm::JvmCodegenError::UnsupportedTypedExpression{node,..}) if node == error_node)
                );
            }
        },
    );
}

#[test]
fn jvm_boolean_case_bindings_and_alternatives_keep_exact_scopes() {
    for method in [
        "def f(x: Boolean): Boolean = x match { case n @ true if n => n; case n @ _ => n }",
        "def f(x: Boolean, n: Boolean): Boolean = x match { case n if n => n; case n => n }",
        "def f(): Boolean = true match { case n => n }",
        "def f(): Boolean = false match { case n => n }",
        "def f(x: Boolean, y: Boolean): Boolean = x match { case n => y match { case m if m => n; case _ => n } }",
        "def f(x: Boolean): Boolean = x match { case n => !n }",
        "def f(x: Boolean, y: Boolean): Boolean = x match { case n => n && y }",
        "def f(x: Boolean, flag: Boolean): Int = x match { case (true | true) | false if flag => 1; case _ => 2 }",
    ] {
        guarded_match_plan::with_jvm(
            &format!("package example; class BooleanMatches {{ {method} }}"),
            |unit, defs| {
                dotty::jvm::emit_class(unit, unit.classes[0].class, defs).unwrap();
                let method = &unit.classes[0].methods[0];
                if let Some(first) = method.locals.first() {
                    let mut classes = unit.classes.clone();
                    let (_, case) = first.pattern_case().unwrap();
                    // A case from another proof cannot authenticate a binding, even when spelling agrees.
                    if let Some(other) = method.locals.get(1) {
                        classes[0].methods[0].locals[0] = dotty::jvm::JvmLocalBinding::pattern(
                            first.declaration,
                            first.symbol,
                            other.pattern_case().unwrap().0,
                            case,
                        );
                        let invalid = dotty::jvm::JvmCompilationUnit {
                            store: unit.store,
                            typed_ast: unit.typed_ast,
                            classes,
                        };
                        assert!(matches!(
                            invalid.validate(),
                            Err(dotty::jvm::JvmCodegenError::InvalidLocalDeclaration { .. })
                        ));
                    }
                }
            },
        );
    }
}

use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class;

fn emit(methods: &str) -> dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class BooleanMatches {{ {methods} }}"),
        "BooleanMatches.scala",
        SourceId::from_index(2476),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn pinned_scala_consumes_and_recompiles_original_rust_boolean_tasty() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_boolean_matches/BooleanMatches.scala"),
        "BooleanMatches.scala",
        SourceId::from_index(2476),
    )
    .unwrap();
    let directory =
        std::env::temp_dir().join(format!("dotty-boolean-match-scala-{}", std::process::id()));
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
            "/tests/fixtures/jvm_tasty_boolean_matches/run.py"
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
            "mandatory boolean-match runtime gate requires JDK 21+"
        );
    }
    available
}

fn run_java(pair: &dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts, name: &str, source: &str) {
    let directory = std::env::temp_dir().join(format!(
        "dotty-boolean-matchs-{}-{name}",
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
        "boolean match oracle ok"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn original_rust_boolean_matches_execute_on_jdk21() {
    if !jdk_available() {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_boolean_matches/BooleanMatches.scala"),
        "BooleanMatches.scala",
        SourceId::from_index(2476),
    )
    .unwrap();
    run_java(
        &pair,
        "JavaConsumer",
        include_str!("fixtures/jvm_tasty_boolean_matches/JavaConsumer.java"),
    );
}

#[test]
fn paired_boolean_pattern_values_preserve_scopes_and_composition() {
    for method in [
        "def f(x: Boolean): Boolean = x match { case n @ true if n => n; case n @ _ => n }",
        "def f(x: Boolean, n: Boolean): Boolean = x match { case n if n => n; case n => n }",
        "def f(): Boolean = true match { case n => n }",
        "def f(): Boolean = false match { case n => n }",
        "def f(x: Boolean, y: Boolean): Boolean = x match { case n => y match { case m if m => n; case _ => n } }",
        "def f(x: Boolean): Boolean = x match { case n => !n }",
        "def f(x: Boolean, y: Boolean): Boolean = x match { case n => n && y }",
        "def f(x: Boolean, flag: Boolean): Int = x match { case (true | true) | false if flag => 1; case _ => 2 }",
    ] {
        emit(method);
    }
}

use dotty::jvm::JvmCompilationUnit;
#[test]
fn tasty_rejects_missing_duplicate_or_cross_case_pattern_evidence_without_mutation() {
    use dotty_tasty_pickler::{
        ClassStatePicklePlan, LocalValueBinding, PickleError, ReferenceError, SourceTypedBindings,
    };
    struct Bindings<'a> {
        original: &'a dyn SourceTypedBindings,
        patterns: Vec<LocalValueBinding>,
    }
    impl SourceTypedBindings for Bindings<'_> {
        fn typed_at(
            &self,
            source: SourceId,
            tree: dotty::core::TreeId<dotty::core::Untyped>,
        ) -> Option<dotty::core::TreeId<dotty::core::Typed>> {
            self.original.typed_at(source, tree)
        }
        fn local_value_bindings(
            &self,
            source: SourceId,
            method: dotty::core::SymbolId,
        ) -> Vec<LocalValueBinding> {
            self.original.local_value_bindings(source, method)
        }
        fn pattern_value_bindings(
            &self,
            _: SourceId,
            _: dotty::core::SymbolId,
        ) -> Vec<LocalValueBinding> {
            self.patterns.clone()
        }
    }
    let source = "package example; class BooleanMatches { def f(x: Boolean): Boolean = x match { case n if n => n; case n => n } }";
    dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
        source,
        "BooleanMatches.scala",
        SourceId::from_index(2476),
        |plan| {
            let raw = plan.tasty_plan.plan();
            let method = plan.jvm_unit.classes[0].methods[0].method;
            let original = raw
                .typed_bindings
                .pattern_value_bindings(raw.source.id, method);
            assert_eq!(original.len(), 2);
            let checkpoint = raw.store.checkpoint();
            for mode in 0..4 {
                let mut patterns = original.clone();
                match mode {
                    0 => patterns.clear(),
                    1 => patterns.push(patterns[0]),
                    2 => patterns[0].scope = patterns[1].scope,
                    3 => patterns[0].typed_declaration = patterns[1].typed_declaration,
                    _ => unreachable!(),
                }
                let bindings = Bindings {
                    original: raw.typed_bindings,
                    patterns,
                };
                let input = ClassStatePicklePlan {
                    store: raw.store,
                    definitions: raw.definitions,
                    source_ast: raw.source_ast,
                    typed_ast: raw.typed_ast,
                    source_index: raw.source_index,
                    typed_bindings: &bindings,
                    source: raw.source.clone(),
                    class: raw.class,
                    class_tree: raw.class_tree,
                    constructor: dotty_tasty_pickler::PrimaryConstructorInputPlan {
                        symbol: raw.constructor.symbol,
                        source_tree: raw.constructor.source_tree,
                        parameter_clauses: raw.constructor.parameter_clauses.clone(),
                    },
                    members: raw.members,
                };
                let error = input
                    .validate()
                    .and_then(|validated| validated.validate_method_bodies())
                    .unwrap_err();
                match mode {
                    0 => assert!(
                        matches!(error, PickleError::Unsupported { .. }),
                        "{error:?}"
                    ),
                    1 => assert!(
                        matches!(error, PickleError::DuplicateDeclaration { .. }),
                        "{error:?}"
                    ),
                    _ => assert!(
                        matches!(
                            error,
                            PickleError::Reference {
                                reason: ReferenceError::Owner,
                                ..
                            }
                        ),
                        "{error:?}"
                    ),
                }
                assert_eq!(checkpoint, raw.store.checkpoint());
            }
            let mut classes = plan.jvm_unit.classes.clone();
            classes[0].methods[0].locals.clear();
            let unit = JvmCompilationUnit {
                store: raw.store,
                typed_ast: raw.typed_ast,
                classes,
            };
            assert!(matches!(
                dotty_jvm_tasty_contract::validate_jvm_tasty_class(&unit, plan.tasty_plan),
                Err(dotty_jvm_tasty_contract::JvmTastyContractError::LocalBindingMismatch { .. })
            ));
            assert_eq!(checkpoint, raw.store.checkpoint());
        },
    )
    .unwrap();
}

#[test]
fn boolean_wire_patterns_keep_constants_alternatives_and_singleton_types() {
    use dotty_tasty::tasty::*;
    let pair = emit(
        "def f(x: Boolean): Int = x match { case (true | false) | true => 1; case _ => 2 }; def t(): Boolean = true match { case n => n }; def b(): Boolean = false match { case n => n }",
    );
    let file = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    let index = file.ast_address_index().unwrap();
    assert_eq!(index.iter_nodes_with_tag(MATCH_TAG).count(), 3);
    assert_eq!(index.iter_nodes_with_tag(ALTERNATIVE_TAG).count(), 2);
    let outer = index.iter_nodes_with_tag(ALTERNATIVE_TAG).next().unwrap();
    let RawTree::LengthNode(outer) =
        RawTree::decode(&mut Reader::new(&payload[outer.offset..])).unwrap()
    else {
        panic!()
    };
    let outer = outer.decode_alternative().unwrap();
    let RawTree::LengthNode(inner) = &outer.alternatives[0] else {
        panic!()
    };
    let inner = inner.decode_alternative().unwrap();
    let values: Vec<_> = inner
        .alternatives
        .iter()
        .chain([&outer.alternatives[1]])
        .map(|n| {
            let RawTree::Leaf(t) = n else { panic!() };
            t.constant_value().unwrap().unwrap()
        })
        .collect();
    assert_eq!(
        values,
        [
            ConstantValue::Boolean(true),
            ConstantValue::Boolean(false),
            ConstantValue::Boolean(true)
        ]
    );
    let bindings: Vec<_> = index.iter().filter(|n| n.tag == BIND_TAG).collect();
    assert_eq!(bindings.len(), 2);
    for (binding, tag) in bindings.into_iter().zip([TRUECONST_TAG, FALSECONST_TAG]) {
        let RawTree::Leaf(ty) = binding.decode_bind().unwrap().type_tree else {
            panic!()
        };
        assert_eq!(ty.tag, tag);
    }
}
