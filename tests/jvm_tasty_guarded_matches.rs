#[test]
fn pinned_oracle_contains_guards_and_distinct_case_binders() {
    let oracle = include_str!("fixtures/jvm_tasty_guarded_matches/oracle.txt");
    for shape in ["BIND(", "CASEDEF(", "TERMREFdirect", "case 0 if", "case n if"] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}
