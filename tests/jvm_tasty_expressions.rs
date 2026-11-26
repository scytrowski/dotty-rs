use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::{PairedArtifactError, emit_paired_jvm_tasty_class};
use dotty::jvm_tasty_plan::PairedPlanError;
use dotty_jvm_tasty_contract::JvmTastyContractError;

#[test]
fn pending_expression_forms_reject_before_returning_a_partial_pair() {
    for method in [
        "def value(x: Int): Int = (x: Int)",
        "def value(x: Int): Int = x & 255",
        "def value(x: Int, y: Int): Boolean = x == y",
        "def value(x: Int): Int = x match { case 0 => 10; case _ => 20 }",
    ] {
        let source = format!("package example\nclass Expressions {{ {method} }}");
        let error =
            emit_paired_jvm_tasty_class(&source, "Expressions.scala", SourceId::from_index(2470))
                .expect_err("current expression gap must not yield either artifact");
        assert!(
            matches!(
                error,
                PairedArtifactError::Plan(
                    PairedPlanError::Tasty(_)
                        | PairedPlanError::Contract(JvmTastyContractError::TastyPlan(_))
                )
            ),
            "{method}: {error:?}"
        );
    }
}

#[test]
fn official_scala_oracle_preserves_ascriptions_matches_and_operator_names() {
    let oracle = include_str!("fixtures/jvm_tasty_expressions/oracle.txt");
    for shape in [
        "TYPED",
        "MATCH",
        "CASEDEF",
        "intValue",
        "mask",
        "equal",
        "different",
    ] {
        assert!(
            oracle.contains(shape),
            "missing official Scala shape {shape}"
        );
    }
}
