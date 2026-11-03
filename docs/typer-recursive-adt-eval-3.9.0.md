# Recursive `Evaluator.eval` proof

This note records the bounded source-typing proof for the original
`Evaluator.eval(expr: Expr): Int` in the frozen Scala 3.9.0 fixture. The source
and pinned Scala oracle are documented in
[`typer-recursive-adt-baseline-3.9.0.md`](typer-recursive-adt-baseline-3.9.0.md).

The baseline already showed that the whole `eval` definition succeeds after
generated `unapply` and Product1 support: its `DefDef` had a typed root and the
source `eval(inner)` application mapped to a typed node. This increment adds
the assertions that were missing from that receipt. It proves the method is
owned by the source `Evaluator$` module class, has one completed `(Expr): Int`
signature, and types after `unwrap` in the same session. A second fresh session
and repeated typing of the same definition produce the same canonical receipt.

The proof first types `unwrap` and then `eval` in one fresh session. A further
session reverses the `Num`/`Succ` and `unwrap`/`eval` source declaration order;
the normalized owner/type/recursive-call receipt remains equal. The original
two-session run also confirms stable receipts, and retyping `eval` reuses its
canonical typed root.

The method retains its two ordered source cases. `Num(value)` binds an `Int`
and returns that binder. `Succ(inner)` binds an `Expr`; its source unqualified
`eval(inner)` resolves to the exact method symbol of the enclosing `DefDef`,
uses one ordinary argument pointing to that case-local binder, and has result
`Int`. The selected `Num$.unapply` and `Succ$.unapply` owners remain the
source-generated companions. The recursive call, `+ 1`, case bodies, Match,
method result, formal, patterns and relevant original source nodes retain
source-to-typed mappings and spans.

[`rust-eval-before-after.tsv`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/rust-eval-before-after.tsv)
records the earlier typed-root/call-mapping evidence and the stronger proof
receipt. This result required no production typer change.

The bounded rollback regression adds a separate explicitly typed
`evalBroken` method whose trailing `+ true` operand fails after the recursive
left operand has been visited. It repeats the failure and checks a stable
diagnostic, unchanged semantic allocation checkpoint and method info,
unchanged typed arena checkpoint, and no surviving source mappings. The
original valid `eval` and its `unwrap` sibling then type in the same session.
This checks method-transaction rollback for this recursive-call shape; the
broader negative matrix remains covered by the parent compatibility fixtures
and the later transaction audit.

Validation used the repository's Scala `Product.tasty` fixture and an
installed Temurin JDK 21.0.13. The pinned Scala oracle uses Temurin JDK
21.0.8+9; that oracle was not rerun for this test-only proof.
