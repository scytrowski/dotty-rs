# Nested `peelLeft` typing proof (Scala 3.9.0)

The issue #2133 reproduction used verified current main commit
`7165bfc6f02c66706a86b5ea814ca49a757187b7` (the #2132 merge). The unchanged
`source-nested-recursive-adt-3.9.0/Expr.scala` fixture has SHA-256
`f700d4e27f11365a08291ae15babc65e673eb91a0bdeee45ea5dc2cf7e8c8607`.

On that commit, `Evaluator.peelLeft(expr: Expr): Expr` already completed as a
whole typed `DefDef` in both independent sessions. Its outer and inner `Add`
patterns have distinct source and typed IDs and retain their source spans; both
resolve through the canonical `Add` companion `unapply`. The inner `left`
binder is case-local, the method returns its exact typed reference, and the
fallback refers to the parameter. Retyping returns the same typed root. This
reproduction found no semantic defect, so the issue preserves the existing
result and adds occurrence, mapping, scope, and parity assertions only. The
captured root IDs and spans are pinned in
`crates/dotty-typer/tests/fixtures/source-nested-recursive-adt-3.9.0/rust-peelleft-complete.tsv`.

The regression also checks both positional arguments and source mappings,
wildcard and sibling-case scope isolation, and idempotent retyping. A nested
pattern root with an incompatible declared result fails at its completed
`Match`; the test checks rollback of the store, typed arena, and source map,
stable failure on retry, then successful typing of the original `peelLeft` in
that same session. Bounded controls cover wrong arity, reordered/renamed fields,
a corrupted unapply owner, and an escaped case binder.

The original #2132 source, Scala oracle, and root receipt remain unchanged.
