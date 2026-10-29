# Source Typer method-parameter captures (Scala 3.9.0)

`SourceTyper` supports capture of plain immutable parameters owned by the
immediately enclosing source method in ordinary `Function1` lambdas. It
preserves the canonical source parameter `SymbolId` and the typed identifier
tree for its first use in `Closure.env`. Environment entries follow first-use
order and repeated reads of one parameter produce one entry. Lambda parameters
remain owned by the synthetic lambda method and do not appear in the
environment.

The deterministic source acceptance is
[`method_parameter_capture.rs`](../crates/dotty-typer/src/typer/tests/method_parameter_capture.rs):
`addBy(delta)` yields `[delta]`; `addBoth(first, second)` yields
`[first, second]` despite the second read of `first`; and a same-name lambda
parameter in `shadow` yields an empty environment. Each method is typed as a
whole source `DefDef`, and each closure environment identifier's `TermRef`
points to the original method parameter. By-name capture is rejected and a
failed method leaves no typed root or lambda identity.

The pinned Scala oracle source and normalized typer output are
[`MethodParameters.scala`](../crates/dotty-typer/tests/fixtures/lexical-closure-contract/MethodParameters.scala)
and [`MethodParameters.scala39-typed-tree.txt`](../crates/dotty-typer/tests/fixtures/lexical-closure-contract/MethodParameters.scala39-typed-tree.txt).
They were compiled with Scala 3.9.0 at
`777528f19a58e794c9954a42f433373472ec57f8` on Eclipse Temurin JDK
`21.0.8+9`. The shared fixture runner includes this source in its snapshot
check.

By-name and repeated parameters, initialized locals, mutable variables,
`this`, instance members, nested lambdas, higher-arity functions, and all
backend/lowering capture behavior remain unsupported by this increment.
