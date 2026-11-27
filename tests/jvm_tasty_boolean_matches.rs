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
fn baseline_boolean_match_is_rejected_after_frontend_typing() {
    let result = dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class(
        "package example; class BooleanMatches { def f(x: Boolean): Int = x match { case true => 1; case _ => 2 } }",
        "BooleanMatches.scala",
        dotty::core::SourceId::from_index(2476),
    );
    assert!(matches!(
        result,
        Err(dotty::jvm_tasty_artifacts::PairedArtifactError::Plan(
            dotty::jvm_tasty_plan::PairedPlanError::Contract(_)
        ))
    ));
}
