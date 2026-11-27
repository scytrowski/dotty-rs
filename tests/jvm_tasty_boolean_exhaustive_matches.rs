#[test]
fn pinned_scala_oracle_keeps_original_exhaustive_boolean_cases() {
    let oracle = include_str!("fixtures/jvm_tasty_boolean_exhaustive_matches/oracle.txt");
    for shape in [
        "scala.Boolean",
        "case true",
        "case false",
        "case (true | false)",
        "case n @ true",
        "guard_effect",
        "return",
        "scala.Long",
        "scala.Float",
        "scala.Double",
    ] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
    assert!(!oracle.contains("case _"));
}
