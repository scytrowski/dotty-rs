# Source case-class trait parent and Product1 completion (Scala 3.9.0)

This increment extends the existing monomorphic, one-field source case-class
Product1 completion to a case class with exactly one explicit parent when that
parent is a canonical, source-owned, monomorphic trait in the same source
semantic index. The parent is authenticated from the projected symbol identity,
its canonical source definition and origin, its trait kind, and its completed
`ClassInfo`. Its single source field must be `Int`. Generic/applied parents,
class parents, non-`Int` fields and multiple-parent shapes do not enter this
Product1 path.

For the pinned `Event.scala` source, `Created` and `Deleted` retain their
original distinct case-class identities and direct `Event` parent. Completion
normalizes the parent list to `Object, Event, scala.Product`, adds no duplicate
Product edge, and completes each original source-derived `_1` as a
parameterless `Int` method. The existing companion `apply` and `unapply`
identities remain owned by each class's canonical module class and retain
`(Int): Created` / `(Created): Created` and `(Int): Deleted` / `(Deleted):
Deleted` signatures.

Both class-first and selector-first completion use the same transactional path.
The tests compare normalized source-owner receipts across two fresh sessions,
exercise the forward `Created/Deleted <: Event` relation through bounded
nominal subtype traversal, and reject the reverse relation. They also cover
declaration reordering, wrong parent kinds, generic and multi-parent deferral,
the existing parentless Product1 and Product2 controls, and a post-mutation
Product lookup failure followed by a valid sibling completion in the same
semantic store.

The frozen whole-method roots remain unsupported at their original first
`ExtractorPatternConstraintDeferred` errors. This change does not accept
extractor patterns or complete `Match` typing. Before/after receipts are in
[`typer-source-case-class-trait-parent-product1-3.9.0.tsv`](typer-source-case-class-trait-parent-product1-3.9.0.tsv).

Compatibility reference: Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`; frozen source SHA-256
`a3554b9a764b612af5b6b73779839af82a5e448e909464e45cbc18fb8a86cba1`.
The checked-in Scala oracle classifies `Event.scala` as accepted and records
the original trait and case-class shapes. The focused audit ran the exact
`source-sealed-adt-match-3.9.0/run` runner at
[Actions run 41432485669](https://github.com/scytrowski/dotty-rs/actions/runs/41432485669)
with Scala revision `777528f19a58e794c9954a42f433373472ec57f8` and Temurin
21.0.8+9; its Event oracle step passed and uploaded the typed-tree, diagnostics,
and classification receipts. The local environment only has Temurin 21.0.13,
so the exact pinned runner was verified on the focused hosted audit. The
preserved #1992 class-info
snapshot remains at
[`rust-classinfo.tsv`](../crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/rust-classinfo.tsv);
the current post-change metadata is recorded separately in
[`rust-after-product1.tsv`](../crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/rust-after-product1.tsv).
