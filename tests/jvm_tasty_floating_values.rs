#[test]
fn pinned_semantic_oracle_preserves_floating_values_and_control_flow() {
    let oracle = include_str!("fixtures/jvm_tasty_floating_values/oracle.txt");
    for shape in [
        "scala.Float",
        "scala.Double",
        "-0.0f",
        "1.4E-45f",
        "4.9E-324",
        "matchedD",
        "return",
    ] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}
