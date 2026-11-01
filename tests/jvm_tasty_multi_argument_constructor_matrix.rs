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
            "two-argument constructor receipt integration",
        ),
        (
            "reordered declarations",
            reordered,
            "two-argument constructor receipt integration",
        ),
        (
            "swapped formal order",
            source.replace(
                "create(first: Int, second: Int)",
                "create(second: Int, first: Int)",
            ),
            "two-argument constructor receipt integration",
        ),
        (
            "swapped nested expressions",
            source.replace(
                "new Counter(first), new Counter(second)",
                "new Counter(second), new Counter(first)",
            ),
            "two-argument constructor receipt integration",
        ),
        (
            "third nested allocation",
            source.replace(
                "new Counter(first), new Counter(second)",
                "new Counter(new Counter(first).get()), new Counter(second)",
            ),
            "nested Counter leaf must be an original factory formal",
        ),
        (
            "branch argument",
            source.replace(
                "new Counter(first), new Counter(second)",
                "new Counter(if first < 0 then second else first), new Counter(second)",
            ),
            "nested Counter leaf must be an original factory formal",
        ),
        (
            "legitimate repeated caller formal",
            source.replace(
                "new Counter(first), new Counter(second)",
                "new Counter(first), new Counter(first)",
            ),
            "two-argument constructor receipt integration",
        ),
        (
            "defaulted factory formal",
            source.replace("create(first: Int, second: Int)", "create(first: Int = 0, second: Int)"),
            "factory formals cannot have default values",
        ),
        (
            "generic factory method",
            source.replace("def create(", "def create[A]("),
            "generic methods are unsupported",
        ),
        (
            "overloaded factory method",
            source.replace(
                "    new Pair(new Counter(first), new Counter(second))",
                "    new Pair(new Counter(first), new Counter(second))\n  def create(seed: String, other: Int): Pair =\n    new Pair(new Counter(0), new Counter(1))",
            ),
            "TypeNameNotFound",
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
        let mut callback_ran = false;
        let result = with_paired_jvm_tasty_batch_plan(
            &variant,
            "PairFactory.scala",
            SourceId::from_index(19130 + ordinal as u32),
            |_| callback_ran = true,
        );
        let Err(error) = result else {
            panic!(
                "Scala-valid but unsupported control unexpectedly passed paired planning: {name}"
            );
        };
        assert!(
            !callback_ran,
            "pair rejection must precede producer callbacks: {name}"
        );
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
