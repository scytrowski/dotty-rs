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
