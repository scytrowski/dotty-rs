# Wildcard Match oracle

This Scala source fixture is pinned to Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`. The typed shape for each supported
method is a `Match` with the typed selector and ordered `CaseDef` children.
Each wildcard pattern has the selector prototype; each ordinary term
`CaseDef` keeps its body's own type. The simple and nested examples have an
`Int` result after value widening.

The multiple-case example fixes the source order and demonstrates a result
join between `Int` and `Boolean`. Dotty's full `TypeComparer.lub` determines
the Scala result type. The current Rust typer intentionally uses the existing
bounded `if`-branch join instead: for unrelated supported types it retains the
`Type::Or` fallback, without general LUB or union normalization. Thus the
fixture pins input and typed-tree structure; result-type parity is deliberately
not claimed for that unrelated-type case.

Relevant Scala 3.9.0 implementation: `Typer.typedMatch` widens non-constant
selectors before typing cases and computes a match result; `typedCase` types
the pattern with the supplied selector prototype; `TypeAssigner` assigns an
ordinary term CaseDef the body type. The source links and compiler revision
are recorded in
[`typer-v0.1-compatibility.md`](../../../../../docs/typer-v0.1-compatibility.md).
