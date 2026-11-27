#[test]
fn pinned_oracle_contains_guards_and_distinct_case_binders() {
    let oracle = include_str!("fixtures/jvm_tasty_guarded_matches/oracle.txt");
    for shape in [
        "BIND(",
        "CASEDEF(",
        "TERMREFdirect",
        "case 0 if",
        "case n if",
    ] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}

fn emit(methods: &str) -> dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts {
    dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class(
        &format!("package example; class GuardedMatches {{ {methods} }}"),
        "GuardedMatches.scala",
        dotty::core::SourceId::from_index(2471),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn guards_are_optional_case_children_in_source_order() {
    let pair = emit(
        "def choose(x: Int): Int = x match { case 0 if x > 0 => 10; case _ if x > 1 => 20; case _ => 30 }",
    );
    let tasty =
        dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
    let index = tasty.ast_address_index().unwrap();
    let guarded: Vec<_> = index
        .iter()
        .filter(|n| n.tag == dotty_tasty::tasty::CASEDEF_TAG)
        .map(|n| n.decode_case_def().unwrap().guard.is_some())
        .collect();
    assert_eq!(guarded, [true, true, false]);
}

#[test]
fn guards_still_require_a_final_unconditional_case() {
    for body in [
        "x match { case _ if x > 0 => 1 }",
        "x match { case _ => 0; case 1 => 1 }",
    ] {
        let source =
            format!("package example; class GuardedMatches {{ def f(x: Int): Int = {body} }}");
        assert!(
            dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class(
                &source,
                "GuardedMatches.scala",
                dotty::core::SourceId::from_index(2471)
            )
            .is_err()
        );
    }
}

#[path = "support/guarded_match_plan.rs"]
mod guarded_match_plan;
use dotty::core::{SourceId, ast::TreeKind};
use dotty::jvm::JvmCompilationUnit;
use guarded_match_plan::with_jvm;

#[test]
fn jvm_case_bindings_are_canonical_slots_and_cannot_change_case_scope() {
    for source in [
        "package example; class GuardedMatches { def f(x: Int): Int = x match { case n if n > 0 => n + 1; case n => n } }",
        "package example; class GuardedMatches { def f(x: Int): Int = x match { case n: Int if n > 0 => n + 1; case n: Int => n } }",
        "package example; class GuardedMatches { def f(x: Long, enabled: Boolean): Long = x match { case n if enabled => n; case n => n } }",
        "package example; class GuardedMatches { def f(x: Long, enabled: Boolean): Long = x match { case n: Long if enabled => n; case n: Long => n } }",
    ] {
        with_jvm(source, |unit, definitions| {
            let class = unit.classes[0].class;
            let generated = dotty::jvm::emit_class(unit, class, definitions).unwrap();
            assert!(!generated.bytes().is_empty());
            let method = &unit.classes[0].methods[0];
            assert_eq!(method.locals.len(), 2);
            assert_ne!(method.locals[0].symbol, method.locals[1].symbol);
            assert_ne!(
                method.locals[0].pattern_case().unwrap().0,
                method.locals[1].pattern_case().unwrap().0
            );
            let mut classes = unit.classes.clone();
            let a = classes[0].methods[0].locals[0];
            let b = classes[0].methods[0].locals[1];
            classes[0].methods[0].locals[0] = dotty::jvm::JvmLocalBinding::pattern(
                a.declaration,
                a.symbol,
                b.pattern_case().unwrap().0,
                a.pattern_case().unwrap().1,
            );
            let invalid = JvmCompilationUnit {
                store: unit.store,
                typed_ast: unit.typed_ast,
                classes,
            };
            assert!(matches!(
                invalid.validate(),
                Err(dotty::jvm::JvmCodegenError::InvalidLocalDeclaration { .. })
            ));
        });
    }
}

#[test]
fn pattern_binders_emit_distinct_tasty_declarations_and_nested_references() {
    for methods in [
        "def f(x: Int): Int = x match { case n if n > 0 => n + 1; case n => n }",
        "def f(x: Int): Int = x match { case n @ 0 if n == 0 => n; case n @ _ => n }",
        "def f(x: Int): Int = x match { case n if n > 0 => n match { case m if m > 3 => m; case _ => n }; case _ => x }",
        "def f(x: Int): Int = 0 match { case n => n }",
    ] {
        let pair = emit(methods);
        let tasty = dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes())
            .unwrap();
        let index = tasty.ast_address_index().unwrap();
        assert!(index.iter().any(|n| n.tag == dotty_tasty::tasty::BIND_TAG));
    }
}

#[test]
fn pinned_scala_reads_and_recompiles_rust_guarded_match_tasty() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let pair = dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_guarded_matches/GuardedMatches.scala"),
        "GuardedMatches.scala",
        SourceId::from_index(2470),
    )
    .unwrap();
    let directory =
        std::env::temp_dir().join(format!("dotty-guarded-scala-{}", std::process::id()));
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
            "/tests/fixtures/jvm_tasty_guarded_matches/run.py"
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
fn complete_guarded_fixture_is_deterministic() {
    let source = include_str!("fixtures/jvm_tasty_guarded_matches/GuardedMatches.scala");
    let first = dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class(
        source,
        "GuardedMatches.scala",
        SourceId::from_index(2471),
    )
    .unwrap();
    let second = dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class(
        source,
        "GuardedMatches.scala",
        SourceId::from_index(2472),
    )
    .unwrap();
    assert_eq!(first.classfile_bytes(), second.classfile_bytes());
    assert_eq!(first.tasty_bytes(), second.tasty_bytes());
}

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
    for source in [
        "package example; class GuardedMatches { def f(x: Int): Int = x match { case n if n > 0 => n; case n => n } }",
        "package example; class GuardedMatches { def f(x: Int): Int = x match { case n: Int if n > 0 => n; case n: Int => n } }",
        "package example; class GuardedMatches { def f(x: Long, enabled: Boolean): Long = x match { case n if enabled => n; case n => n } }",
        "package example; class GuardedMatches { def f(x: Long, enabled: Boolean): Long = x match { case n: Long if enabled => n; case n: Long => n } }",
        "package example; class GuardedMatches { def f(enabled: Boolean): Long = 1L match { case n if enabled => n; case n => n } }",
    ] {
        dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan(
            source,
            "GuardedMatches.scala",
            SourceId::from_index(2471),
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
                let error = dotty_jvm_tasty_contract::validate_jvm_tasty_class(&unit, plan.tasty_plan).err();
                assert!(matches!(
                    error,
                    Some(
                        dotty_jvm_tasty_contract::JvmTastyContractError::LocalBindingMismatch { .. }
                    )
                ), "{source}: {error:?}");
                assert_eq!(checkpoint, raw.store.checkpoint());
            },
        )
        .unwrap();
    }
}

fn copied_typed_ast(
    unit: &JvmCompilationUnit<'_>,
) -> dotty::core::ast::AstArena<dotty::core::Typed> {
    let mut ast = dotty::core::ast::AstArena::new();
    for (id, node) in unit.typed_ast.iter() {
        assert_eq!(id, ast.alloc(node.clone()));
    }
    ast
}

#[test]
fn jvm_rejects_non_boolean_guards_and_cross_case_binder_references() {
    for source in [
        "package example; class GuardedMatches { def f(x: Int): Int = x match { case n if x > 0 => n; case n => n } }",
        "package example; class GuardedMatches { def f(x: Int): Int = x match { case n: Int if x > 0 => n; case n: Int => n } }",
    ] {
        with_jvm(source, |unit, definitions| {
            let method = &unit.classes[0].methods[0];
            let case_id = method.locals[0].pattern_case().unwrap().1;
            let root = method.body.unwrap();
            let TreeKind::Match(matched) = &unit.typed_ast.get(root).kind else {
                panic!()
            };
            let selector = matched.selector;
            let mut ast = copied_typed_ast(unit);
            let TreeKind::CaseDef(case) = &mut ast.get_mut(case_id).kind else {
                panic!()
            };
            case.guard = Some(selector);
            let invalid = JvmCompilationUnit {
                store: unit.store,
                typed_ast: &ast,
                classes: unit.classes.clone(),
            };
            assert!(
                matches!(dotty::jvm::emit_class(&invalid, unit.classes[0].class, definitions), Err(dotty::jvm::JvmCodegenError::UnsupportedTypedExpression { tree, node: "non-Boolean match guard" }) if tree == selector)
            );
            let mut ast = copied_typed_ast(unit);
            let TreeKind::CaseDef(case) = ast.get(case_id).kind else {
                panic!()
            };
            let TreeKind::Block(block) = &ast.get(case.body).kind else {
                panic!()
            };
            let expr = block.expr;
            let foreign_type = ast.get(method.locals[1].declaration).ty;
            ast.get_mut(expr).ty = foreign_type;
            ast.get_mut(case.body).ty = foreign_type;
            ast.get_mut(case_id).ty = foreign_type;
            let invalid = JvmCompilationUnit {
                store: unit.store,
                typed_ast: &ast,
                classes: unit.classes.clone(),
            };
            let error =
                dotty::jvm::emit_class(&invalid, unit.classes[0].class, definitions).unwrap_err();
            assert!(
                matches!(
                    error,
                    dotty::jvm::JvmCodegenError::UninitializedLocalRead { .. }
                ),
                "{error:?}"
            );
        });
    }
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
            "mandatory guarded-match runtime gate requires JDK 21+"
        );
    }
    available
}

fn run_java(pair: &dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts, name: &str, source: &str) {
    let directory = std::env::temp_dir().join(format!(
        "dotty-guarded-matches-{}-{name}",
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
        "guarded match oracle ok"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn original_rust_guarded_matches_execute_on_jdk21() {
    if !jdk_available() {
        return;
    }
    let pair = dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_guarded_matches/GuardedMatches.scala"),
        "GuardedMatches.scala",
        SourceId::from_index(2471),
    )
    .unwrap();
    run_java(
        &pair,
        "JavaConsumer",
        include_str!("fixtures/jvm_tasty_guarded_matches/JavaConsumer.java"),
    );
}

#[test]
fn unsupported_patterns_and_case_binding_escape_never_publish_a_pair() {
    for body in [
        "x match { case n: Int => n; case _ => 0 }",
        "x match { case 0 | _ => 1; case _ => 0 }",
        "x match { case n if n > 0 => n }",
        "x match { case n if n > 0 => n; case _ => n }",
        "{ x match { case n => n }; n }",
        "x match { case n => n = 1 }",
    ] {
        let source =
            format!("package example; class GuardedMatches {{ def f(x: Int): Int = {body} }}");
        let error = dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class(
            &source,
            "GuardedMatches.scala",
            SourceId::from_index(2471),
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                dotty::jvm_tasty_artifacts::PairedArtifactError::Plan(_)
            ),
            "{body}: {error:?}"
        );
    }
}

#[path = "support/typed_primitive_patterns.rs"]
mod typed_primitive_patterns;
