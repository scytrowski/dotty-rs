use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class;

#[test]
fn semantic_oracle_retains_full_signed_long_values_and_control_flow() {
    let oracle = include_str!("fixtures/jvm_tasty_long_values/oracle.txt");
    for shape in [
        "scala.Long",
        "9000000000L",
        "-9223372036854775808L",
        "choose",
        "return",
    ] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}

#[test]
fn characterize_the_missing_long_signature_producer() {
    let result = emit_paired_jvm_tasty_class(
        "package example; class LongValues { def literal(): Long = 9000000000L }",
        "LongValues.scala",
        SourceId::from_index(2473),
    );
    assert!(matches!(
        result,
        Err(dotty::jvm_tasty_artifacts::PairedArtifactError::Plan(_))
    ));
}
