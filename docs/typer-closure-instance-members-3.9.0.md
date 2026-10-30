# Same-owner instance member capture in `Function1` lambdas

An ordinary `Function1` lambda inside a source class or trait method may refer
to a method or immutable field declared by that same class or trait. The source
typer resolves the existing declaration and records the canonical enclosing
`this` in `Closure.env`; repeated member references share one capture. An
explicit `this.member` uses the same capture already recorded for its lexical
`this` expression. Member identity remains the declaration's `SymbolId`, and
the captured receiver retains `Type::ThisType` for the source owner.

The implementation deliberately requires an exact owner match. It does not
resolve inherited members for this capture path, capture mutable fields, or
capture package/object members. Local declarations still shadow members. A
failed lambda typing attempt does not publish its closure or typed-source
mapping.

The focused `SourceTyper` regressions are in
[`enclosing_this_capture.rs`](../crates/dotty-typer/src/typer/tests/enclosing_this_capture.rs).
They cover the `Calculator.lifted`, `Calculator.explicit`,
`Calculator.withBias`, and `Adapter.lifted` shapes with real named symbols and
typed `DefDef` roots, canonical method identity, shadowing, inherited lookup
rejection, mutable assignment rejection, and rollback after invalid calls.
The existing pinned Scala 3.9.0 closure oracle fixture also contains implicit
and explicit immutable-field access in
[`Positive.scala`](../crates/dotty-typer/tests/fixtures/lexical-closure-contract/Positive.scala).

The pinned standard-library roots were audited twice with determinism
verification using the exact source revision and classpath. `Function1.andThen`
is fully typed, including its closure, expected result, and source mapping.
`Function1.compose` remains incomplete: its first blocker is
`ApplicationArgumentConformanceUnsupported` at `Function1.scala#tree=81`,
after member capture. The exact outcomes are recorded in
[`typer-closure-instance-members-roots-3.9.0.tsv`](typer-closure-instance-members-roots-3.9.0.tsv).
The checked-in `typer-expected-lambda-inference-3.9.0.tsv` records the earlier
#1810 baseline and remains a historical receipt. The frozen 25-method ordinary
cohort now reports 15/25 typed; its prior 14 required successes all remain
typed, and `andThen` accounts for the one additional complete method. The
remaining `compose` issue is a conformance gap outside this focused capture
increment.

Reproduce the exact two-root audit from the repository root with the pinned
Scala checkout, artifact cache, and JDK 21 configured:

```sh
SCALA39_ROOT=/path/to/scala3 \
JAVA_HOME=/path/to/jdk-21 SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=/path/to/coursier/maven \
tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest /path/to/function1-captures.tsv --verify-determinism \
  target/function1-captures-report.tsv
```
