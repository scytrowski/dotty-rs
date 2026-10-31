use dotty::jvm_tasty_plan::with_paired_jvm_tasty_batch_plan;
use dotty_core::SourceId;

const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactory.scala");

#[test]
fn multi_argument_factory_variation_matrix_records_first_paired_result() {
    let source = SOURCE;
    let counter_start = source.find("class Counter(start").unwrap();
    let pair_start = source.find("class Pair(first").unwrap();
    let factory_start = source.find("class PairFactory:").unwrap();
    let counter = &source[counter_start..pair_start];
    let pair = &source[pair_start..factory_start];
    let factory = &source[factory_start..];
    let reordered = format!("package example\n\n{factory}\n{pair}\n{counter}");
    let renamed = source
        .replace("package example", "package renamed.example")
        .replace("class PairFactory:", "class Assembler:")
        .replace("def create(", "def assemble(")
        .replace("PairFactory", "Assembler")
        .replace("class Counter", "class Token")
        .replace("Counter", "Token")
        .replace("class Pair(", "class Envelope(")
        .replace("): Pair =", "): Envelope =")
        .replace("new Pair(", "new Envelope(");
    let valid_controls = [
        (
            "renamed package-level identities",
            renamed,
            "candidate method must have one plain formal clause",
        ),
        (
            "reordered declarations",
            reordered,
            "candidate method must have one plain formal clause",
        ),
        (
            "swapped formal order",
            source.replace(
                "create(first: Int, second: Int)",
                "create(second: Int, first: Int)",
            ),
            "candidate method must have one plain formal clause",
        ),
        (
            "swapped nested expressions",
            source.replace(
                "new Counter(first), new Counter(second)",
                "new Counter(second), new Counter(first)",
            ),
            "candidate method must have one plain formal clause",
        ),
        (
            "third nested allocation",
            source.replace(
                "new Counter(first), new Counter(second)",
                "new Counter(new Counter(first).get()), new Counter(second)",
            ),
            "candidate method must have one plain formal clause",
        ),
        (
            "branch argument",
            source.replace(
                "new Counter(first), new Counter(second)",
                "new Counter(if first < 0 then second else first), new Counter(second)",
            ),
            "candidate method must have one plain formal clause",
        ),
        (
            "default constructor",
            source.replace("second: Counter):", "second: Counter = new Counter(0)):"),
            "InvalidPrimaryConstructor",
        ),
        (
            "generic constructor",
            source
                .replace("class Pair(first: Counter", "class Pair[A](first: Counter")
                .replace("): Pair =", "): Pair[Any] ="),
            "generic primary constructors are unsupported",
        ),
        (
            "secondary constructor",
            source.replace(
                "  def leftValue()",
                "  def this(first: Counter) = this(first, new Counter(0))\n  def leftValue()",
            ),
            "method is not owned by its declaring class",
        ),
        (
            "contextual constructor",
            source
                .replace(
                    "class Pair(first: Counter, second: Counter):",
                    "class Pair(first: Counter)(using second: Counter):",
                )
                .replace(
                    "new Pair(new Counter(first), new Counter(second))",
                    "new Pair(new Counter(first))(using new Counter(second))",
                ),
            "InvalidPrimaryConstructor",
        ),
    ];
    for (ordinal, (name, variant, expected)) in valid_controls.into_iter().enumerate() {
        let result = with_paired_jvm_tasty_batch_plan(
            &variant,
            "PairFactory.scala",
            SourceId::from_index(19130 + ordinal as u32),
            |_| (),
        );
        let Err(error) = result else {
            panic!(
                "Scala-valid but unsupported control unexpectedly passed paired planning: {name}"
            );
        };
        assert!(format!("{error:?}").contains(expected), "{name}: {error:?}");
    }

    let invalid_controls = [
        (
            "wrong Pair arity",
            source.replace(
                "new Pair(new Counter(first), new Counter(second))",
                "new Pair(new Counter(first))",
            ),
            "ApplicationArityMismatch",
        ),
        (
            "wrong Counter argument type",
            source.replace("new Counter(first)", "new Counter(\"wrong\")"),
            "StringLiteralTypingDeferred",
        ),
        (
            "duplicate method formal",
            source.replace(
                "create(first: Int, second: Int)",
                "create(first: Int, first: Int)",
            ),
            "AmbiguousTermReference",
        ),
    ];
    for (ordinal, (name, variant, expected)) in invalid_controls.into_iter().enumerate() {
        let error = with_paired_jvm_tasty_batch_plan(
            &variant,
            "PairFactory.scala",
            SourceId::from_index(19140 + ordinal as u32),
            |_| (),
        )
        .expect_err("Scala-invalid source must reject before paired planning");
        assert!(format!("{error:?}").contains(expected), "{name}: {error:?}");
    }
}
