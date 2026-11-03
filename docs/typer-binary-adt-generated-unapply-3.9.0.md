# Binary ADT generated `unapply` signature (Scala 3.9.0)

This increment completes only the generated companion method signature for
`Add(left: Expr, right: Expr) extends Expr`. The frozen Scala 3.9.0 typed-tree
receipt in [the binary recursive ADT baseline](typer-binary-recursive-adt-baseline-3.9.0.md)
records two distinct class-owned source fields and `Add$.unapply(Add): Add`.
The exact compiler source revision is
`777528f19a58e794c9954a42f433373472ec57f8`; the pinned JDK for oracle
reproduction is Eclipse Temurin `21.0.8+9`.

Typer now accepts an ordinary two-field constructor only when both original
`ValDef` trees resolve independently to distinct canonical `Field` symbols
owned by the same source case class, each field type is a canonical monomorphic
source `Trait` `TypeRef`, and both target the same authenticated direct parent.
The method keeps the existing generated role and synthetic method-owned `x$1`
formal and completes to one plain `(Add): Add` `MethodType`. It does not replace
the source fields, infer identity from their names, or create Product2 selector
or parent edges.

The fixed `Int`/`Boolean` Product2 and `Int`/canonical-parent Product1 paths
remain separate. The source parent must be a direct, source-defined, generic-
free trait, and both reference fields must match it by symbol identity. A
foreign same-spelled trait, a mismatching second field, generic/applied types,
or malformed generated roles remain unsupported transactionally. The
before/after semantic receipt is in
[`typer-binary-adt-generated-unapply-methodtype-3.9.0.tsv`](typer-binary-adt-generated-unapply-methodtype-3.9.0.tsv).

This is signature completion only. Product2 selectors/class metadata and
whole-match typing remain owned by later work. On the frozen two-root fixture,
`selectLeft` and `eval` now advance to `ExtractorResultMemberNotFound` at their
first selector use in two fresh sessions; see
[`rust-after-typer-unapply-two-root.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/rust-after-typer-unapply-two-root.tsv).
This does not claim whole-match success. Rust signature and rollback
regressions are in `dotty-typer`'s unit tests. The pinned Scala typed-tree
snapshot and reproducible fixture runner are retained as the oracle evidence.
