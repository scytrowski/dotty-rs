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

include!("support/guarded_match_plan.rs");

#[test]
fn jvm_case_bindings_are_canonical_slots_and_cannot_change_case_scope() {
    with_jvm(
        "package example; class GuardedMatches { def f(x: Int): Int = x match { case n if n > 0 => n + 1; case n => n } }",
        |unit, definitions| {
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
        },
    );
}
