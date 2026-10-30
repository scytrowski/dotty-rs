use dotty::jvm_tasty_plan::with_paired_jvm_tasty_batch_plan;
use dotty_core::SourceId;

const SOURCE: &str = include_str!("fixtures/jvm_tasty_dual_reference_constructors/Pair.scala");

#[test]
fn signature_only_renamed_pair_passes_ordered_proof_before_unchanged_tasty_gate() {
    // Envelope is intentionally first: the batch planner completes both classes,
    // validates the ordered two-reference JVM proof, then reaches the existing
    // PrimaryConstructorInput rejection while validating Pair's TASTy plan.
    let source = "package renamed.example\n\nclass Envelope(left: Token, right: Token)\n\nclass Token(start: Int)\n";
    let error = with_paired_jvm_tasty_batch_plan(
        source,
        "SignatureOnly.scala",
        SourceId::from_index(1875),
        |_| (),
    )
    .expect_err("the signature-only source still reaches the bounded TASTy guard");
    let observed = format!("{error:?}");
    assert!(
        observed.contains("phase: \"TASTy plan validation\""),
        "{observed}"
    );
    assert!(observed.contains("PrimaryConstructorInput"), "{observed}");
}

#[test]
fn two_reference_formals_to_different_batch_classes_fail_at_ordered_proof() {
    let source = "package example\n\nclass Pair(first: Counter, second: Other)\n\nclass Counter(start: Int)\n\nclass Other(start: Int)\n";
    let error = with_paired_jvm_tasty_batch_plan(
        source,
        "DifferentReferenceFormals.scala",
        SourceId::from_index(1875),
        |_| (),
    )
    .expect_err("distinct same-batch target classes must not share one formal proof");
    let observed = format!("{error:?}");
    assert!(
        observed.contains("phase: \"ordered constructor formal proof\""),
        "{observed}"
    );
    assert!(
        observed.contains("OrderedReferenceFormalIdentity"),
        "{observed}"
    );
}

#[test]
fn paired_batch_matrix_records_source_errors_and_unreached_later_stages() {
    struct Case {
        name: &'static str,
        source: String,
        phase: &'static str,
        reason: &'static str,
    }
    let pair_start = SOURCE.find("\nclass Pair").unwrap();
    let counter_source = SOURCE.strip_prefix("package example\n").unwrap();
    let reordered_classes = format!(
        "package example\n{}\n\n{}",
        &SOURCE[pair_start + 1..],
        counter_source.split_once("\nclass Pair").unwrap().0
    );
    let variants = [
        Case {
            name: "reordered class declaration order",
            source: reordered_classes,
            phase: "TASTy plan validation",
            reason: "PrimaryConstructorInput",
        },
        Case {
            name: "wrong second field source type",
            source: SOURCE.replace("second: Counter", "second: Int"),
            phase: "field initializer typing",
            reason: "ExpectedExpressionTypeMismatch",
        },
        Case {
            name: "duplicate constructor parameter name",
            source: SOURCE.replace("second: Counter", "first: Counter"),
            phase: "field initializer typing",
            reason: "AmbiguousTermReference",
        },
        Case {
            name: "generic constructor",
            source: SOURCE.replace("class Pair(first", "class Pair[A](first"),
            phase: "source collection",
            reason: "generic primary constructors are unsupported",
        },
        Case {
            name: "secondary constructor",
            source: SOURCE.replace(
                "  def leftValue(): Int = left.get()",
                "  def this(first: Counter, second: Counter, extra: Int) = this(first, second)\n  def leftValue(): Int = left.get()",
            ),
            phase: "member planning",
            reason: "method is not owned by its declaring class",
        },
        Case {
            name: "default constructor argument",
            source: SOURCE.replace(
                "second: Counter)",
                "second: Counter = new Counter(1))",
            ),
            phase: "constructor planning",
            reason: "InvalidPrimaryConstructor",
        },
        Case {
            name: "contextual constructor clause",
            source: SOURCE.replace(
                "first: Counter, second: Counter)",
                "first: Counter)(using second: Counter)",
            ),
            phase: "constructor planning",
            reason: "InvalidPrimaryConstructor",
        },
        Case {
            name: "constructor val accessors",
            source: SOURCE.replace(
                "class Pair(first: Counter, second: Counter)",
                "class Pair(val first: Counter, val second: Counter)",
            ),
            phase: "complete JVM unit validation",
            reason: "AccessorFieldSignature",
        },
        Case {
            name: "constructor var accessors",
            source: SOURCE.replace(
                "class Pair(first: Counter, second: Counter)",
                "class Pair(var first: Counter, var second: Counter)",
            ),
            phase: "complete JVM unit validation",
            reason: "AccessorFieldSignature",
        },
        Case {
            name: "more than two reference formals",
            source: SOURCE.replace(
                "first: Counter, second: Counter)",
                "first: Counter, second: Counter, third: Counter)",
            ),
            phase: "TASTy plan validation",
            reason: "PrimaryConstructorInput",
        },
        Case {
            name: "third distinct reference field",
            source: SOURCE
                .replace(
                    "first: Counter, second: Counter)",
                    "first: Counter, second: Counter, third: Counter)",
                )
                .replace(
                    "  private val right: Counter = second",
                    "  private val right: Counter = second\n  private val thirdRef: Counter = third",
                ),
            phase: "TASTy plan validation",
            reason: "PrimaryConstructorInput",
        },
        Case {
            name: "no alias fields",
            source: SOURCE.replace(
                "class Pair(first: Counter, second: Counter):\n  private val left: Counter = first\n  private val right: Counter = second\n  def leftValue(): Int = left.get()\n  def rightValue(): Int = right.get()",
                "class Pair(first: Counter, second: Counter)",
            ),
            phase: "TASTy plan validation",
            reason: "PrimaryConstructorInput",
        },
        Case {
            name: "reordered parameter names",
            source: SOURCE.replace(
                "class Pair(first: Counter, second: Counter)",
                "class Pair(second: Counter, first: Counter)",
            ),
            phase: "TASTy plan validation",
            reason: "PrimaryConstructorInput",
        },
        Case {
            name: "wrong field-to-parameter alias",
            source: SOURCE.replace(
                "private val right: Counter = second",
                "private val right: Counter = first",
            ),
            phase: "TASTy plan validation",
            reason: "PrimaryConstructorInput",
        },
    ];
    for case in variants {
        let error = with_paired_jvm_tasty_batch_plan(
            &case.source,
            "Pair.scala",
            SourceId::from_index(1875),
            |_| (),
        )
        .expect_err("each variation must report its real first paired-planner rejection");
        let observed = format!("{error:?}");
        assert!(observed.contains(case.phase), "{}: {observed}", case.name);
        assert!(observed.contains(case.reason), "{}: {observed}", case.name);
        println!("dual-reference variant {}: {observed}", case.name);
    }
}
