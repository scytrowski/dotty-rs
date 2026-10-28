# Expected Function1 lambda parameter inference (Scala 3.9.0)

This increment types a one-parameter unannotated lambda from a proven expected
canonical ordinary `Function1[A, B]`. It binds the lambda's actual parameter
symbol to `A`, types the body, and checks the resulting function against the
expected type. The expected type is forwarded for direct and parenthesized
lambdas, expression-only blocks, ordinary method arguments, and a selected
single overload candidate. Method-result checking is restricted to methods
whose declared result is a canonical ordinary `Function1` and whose RHS is a
lambda under those wrappers. Blocks with statements retain their existing
behavior. Explicitly annotated parameters continue through their existing
path.

The typer checks the canonical `scala.Function1` symbol and package prefix
before using its arguments. A same-arity applied type with another class
symbol is rejected, as are missing/non-function expectations and invalid
lambda arity. Failed body conformance rolls back the lambda parameter and
typed mappings.

## Pinned controls

The Scala oracle is the source checkout at revision
`777528f19a58e794c9954a42f433373472ec57f8`, compiler/library 3.9.0, JDK 21.
The positive oracle control compiled: an annotated `Int => String` value, a
method returning `Int => Int`, a method argument `accepts(x => x)`, and an
explicitly typed lambda. The negative control failed with the expected Scala
diagnostics for output mismatch, wrong arity, missing expected type, and a
non-function expected type. The compiler was run with
`dotty.tools.dotc.Main -classpath <Scala 3.9.0 jars> -release 21`.

Positive source:

```scala
object Positive {
  val value: Int => String = x => x.toString
  def result: Int => Int = x => x
  def accepts(f: Int => Int): Int = f(1)
  val argument: Int = accepts(x => x)
  val explicit: Int => Int = (x: Int) => x
}
```

Negative source:

```scala
object Negative {
  val outputMismatch: Int => Boolean = x => x
  val wrongArity: Int => Int = (x, y) => x
  val missingExpected = x => x
  val wrongExpected: Int = x => x
}
```

The compact real source roots and their before/after audit outcomes are in
`typer-expected-lambda-inference-3.9.0.tsv`.

The real `compose` and `andThen` members in the pinned Scala source both compile
in Scala 3.9.0. Before this change, the dotty-rs member audit stopped at
`UnsupportedFunctionLiteralParameter` for both. Afterward, it advances through
parameter inference and stops at `FunctionLiteralCaptureUnsupported`. Capture
support is explicitly outside this issue, so neither root is counted as fully
typed. Full-method acceptance still requires the whole owned `DefDef`, RHS,
result conformance, and source mapping.

## Cohort regression check

At sprint start the fresh base was `11ee89ddc7e9e1a672023995f593fa5c831c7213`;
the unchanged frozen manifests measured Tier A `14/25` and the former holdout
`9/25` (`25/25` attempted in each). Main advanced during implementation to
`57db74e0662a8dc2b49274f02df40daf515e569d`. Auditing that base without this
change measured Tier A `14/25` and holdout `15/25`; the PR branch on the same
base produced the same counts. The holdout uplift therefore predates this
change. The former holdout is an analysis cohort, not a blind sample. The two
Function1 edge roots are reported separately and do not alter either cohort.
