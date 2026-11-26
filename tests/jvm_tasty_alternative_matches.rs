#[test]
fn pinned_semantic_oracle_preserves_alternative_structure_and_returns() {
    let oracle = include_str!("fixtures/jvm_tasty_alternative_matches/oracle.txt");
    for shape in [
        "(-2147483648 | 0 | 2147483647)",
        "(0 | 0)",
        "((0 | 1) | 2)",
        "scala.Long",
        "return",
    ] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}
