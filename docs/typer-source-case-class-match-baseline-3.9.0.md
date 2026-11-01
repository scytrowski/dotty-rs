# Source case class extractor baseline (Scala 3.9.0)

This is the pinned pre-implementation baseline for issue #1933. It freezes one
positive source file, the Scala typer output and a bounded acceptance matrix;
it does not change Namer or Typer semantics. The only whole-method acceptance
roots are `Matcher.unwrap` and `Matcher.classify` in
[`Positive.scala`](../crates/dotty-typer/tests/fixtures/source-case-class-match-3.9.0/Positive.scala).
The source SHA-256 is
`df57333192eb934169f06a07873c2e1ba88c9159404f60e5bfe33b3d016e215c`.

## Pinned Scala evidence

The fixture runner requires Scala 3.9.0 compiler/runtime artifacts with the
checked-in SHA-256 values, the Scala source checkout at
`777528f19a58e794c9954a42f433373472ec57f8`, and Eclipse Temurin 21.0.8+9.
It checks these inputs, runs `-Vprint:typer -Xprint-types -Yprint-debug
-Yprint-debug-owners`, and compares the output with
`Positive.scala39-typed-tree.txt`. Source hashes and oracle classifications are
in `source-hashes.tsv` and `scala-matrix.tsv` beside the fixture.

The pinned typer output establishes:

- `matching.Box` is a case class extending `scala.Product` and
  `scala.Serializable`.
- Its generated companion is `matching.Box$`. The generated `unapply` is owned
  by that companion and has the monomorphic signature
  `unapply(x$1: matching.Box): matching.Box`; its body returns `x$1` directly.
- The class's product selector is `Box._1: Int`, implemented by selecting
  `Box.value`. Pattern binders in both real methods have type `Int`.
- The source-defined `object Box` companion interaction in the separate matrix
  control remains accepted and receives generated case-class members.

Pinned source implementation evidence is in
`scala3/compiler/src/dotty/tools/dotc/ast/Desugar.scala` at the revision above:
the case-class companion generation comments and code around lines 992–1047
specify `unapply(x$1: C) = x$1` for an ordinary non-repeated case class.

The bounded oracle matrix classifies `Box(value)`, `Box(0)`, wildcard,
explicit companion reuse and a non-case-class extractor as accepted. An
explicit companion `unapply(Box): Option[Int]` is retained and accepted with
a non-exhaustivity warning for
`case Box(member)`; an explicit same-parameter `unapply(Box): Int` is also
retained, and the pattern is rejected because `Int` is not an extractor result.
An overload `unapply(String)` is accepted alongside the generated
`unapply(Box)`. These controls establish that generated publication must not
replace a legal explicit member or mask an invalid explicit extractor. The distinct
same-spelled control keeps the package-level case class `matching.Box` and adds
the nested extractor `matching.OtherScope.Box`, whose
`unapply(matching.Box): Option[Int]` is selected by `OtherScope.extract`. The
typed snapshot `SameSpelledExtractor.scala39-typed-tree.txt` records the
pattern's qualifier as `OtherScope$.this.Box` and the distinct `unapply` owner;
Scala accepts it with a non-exhaustivity warning. A result mismatch, arity
zero, arity two and a constructor applied to an unrelated selector type are
rejected. The `Box(value: String)` type-pattern control is also accepted with a
non-exhaustivity warning: the type test is legal even though the case cannot
match an `Int` selector. These distinctions are preserved in the matrix.

## Current Rust baseline

`cargo test -p dotty-typer --test source_case_class_match_baseline --locked`
drives the exact fixture through the real lexer, parser, namer and
`SourceTyper::type_method_definition`. It discovers the two method trees from
the parsed AST and verifies that failed roots do not appear in
`SourceTypedIndex`. The two rows in `rust-baseline.tsv` include method tree IDs,
source IDs, whole-method byte spans, first diagnostic source tree and byte
span, and the stable error.

Namer already enters the `Box` class, generated companion term and module
class, links the class and term, and creates generated `apply`. The module-class symbol has no companion link itself. Namer does not
enter generated `unapply` or the case-class `_1` selector. Both exact method
roots therefore stop at `ExtractorUnapplyNotFound` on their `Box(...)` pattern;
product-result typing and selector projection have not been attempted. The
existing product protocol checks resolve `scala.Product` from the session's
`scala` package and can validate user-defined Product-shaped extractor results,
but `Definitions` has no canonical built-in Product identity. This baseline
does not infer a Product ancestry result for `Box` from a path that has not yet
run.

The test retries each failed root after transaction rollback and asserts the
same error with no typed root left behind. It then repeats the complete run in
a fresh `SemanticStore` and requires an identical normalized receipt.

## Reproduce

From the repository root, with the pinned Scala source checkout and JDK
installed locally:

```sh
export SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8
export JAVA_HOME=/path/to/temurin-21.0.8+9
export COURSIER_MAVEN_ROOT="$PWD/target/coursier/v1/https/repo1.maven.org/maven2"
export TMPDIR="$PWD/target/tmp"
export CARGO_TARGET_DIR="$PWD/target/cargo"
mkdir -p "$TMPDIR"
crates/dotty-typer/tests/fixtures/source-case-class-match-3.9.0/run
cargo test -p dotty-typer --test source_case_class_match_baseline --locked
```

The runner writes only temporary compiler output under `target/`; it never
rebuilds Scala or modifies the checked-in oracle unless `UPDATE_SNAPSHOT=1` is
explicitly set.
