#[test]
fn pinned_oracle_records_method_targets_and_unit_returns() {
    let oracle = include_str!("fixtures/jvm_tasty_local_returns/oracle.txt");
    for shape in ["RETURN(", "UNITconst", "[direct]", "[matched]", "[inLoop]"] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}
