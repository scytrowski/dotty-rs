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
