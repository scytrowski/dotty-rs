# Binary ADT `selectLeft` typer proof (Scala 3.9.0)

Issue #2076 completes the existing `selectLeft` source root in
`source-binary-recursive-adt-3.9.0/Expr.scala`. The fixture is unchanged. The
proof runs the original source method first in two separately constructed
source, semantic-store, and authentic resolver sessions. Neither session types
`eval` before `selectLeft`.

The focused integration test checks the complete source-owned `DefDef`, its
canonical `(Expr): Expr` signature, source formal and match selector, and both
source-ordered cases. `Num(_)` and `Add(left, _)` narrow to the original
canonical case classes. The Add case resolves the companion-owned generated
`unapply(Add): Add`; its two typed pattern positions retain the distinct,
class-owned Product selectors derived from the original ordered constructor
parameter trees. The first position creates a case-local `left: Expr` binder
from `_1`, the second stays an unbound wildcard from `_2`, and the body refers
to that exact binder. Every nonempty source span inside the method maps to a
typed tree with the same span.

The paired receipt is in
[`rust-select-left-before-after.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/rust-select-left-before-after.tsv).
The `before` row records the prior completion-only evidence; the two `after`
rows are the normalized independent sessions. Earlier root-level receipts
remain in
[`rust-after-product2-two-root.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/rust-after-product2-two-root.tsv)
and are not used to claim selectLeft-first behavior.

An independent `Pair(first: Expr, second: Expr)` control verifies that equal
field types do not erase constructor position or identity. Its source-derived
selectors remain distinct and ordered as `_1` and `_2`, and only the first
pattern position binds `first`.

The failure control places a deliberately incompatible `Int` result after the
Add binder has been allocated. It checks rollback of semantic and typed arena
allocation, source type and typed indexes, existing symbol information,
derived method information, and entered scope members, then types the original
`selectLeft` successfully in the same typer. This tests the existing
transactional contract; it does not add recursive evaluation.

The reference-Product2 negative controls separately corrupt source provenance,
scope selector order, Add's `unapply` owner and companion link, its parent
graph, the canonical Product identity, and a selector owner. The mutable
identity controls restore their metadata and retry `selectLeft` in the same
`SourceTyper` and resolver after checking that source type mappings, typed
mappings, pattern bindings, scopes, symbols, and arenas match their
pre-attempt snapshots. The source-provenance control changes the immutable
`SourceSemanticIndex`, which a live typer borrows and cannot replace; it checks
stable rejection and rollback, while the two fresh authentic sessions above
prove valid-source completion.

Oracle classifications and source hashes remain in the pinned fixture set:
`negative-cases.tsv` distinguishes rejected malformed cases from valid
foreign-extractor, renamed/swapped-field, and warning-only sources. The
Scala-side oracle is pinned to Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8` and the fixture's Temurin 21.0.8+9
environment. Its runner reproduced all 17 frozen source hashes, the immutable
`Expr.scala` digest, the typed-tree snapshot, all 16 classifications, and the
diagnostic/output hashes in this environment. The Rust tests consume the
frozen source and oracle receipts; they do not modify Scala fixtures.

Reproduce the pinned oracle with `SCALA39_ROOT` set to the recorded Scala
checkout, `JAVA_HOME` set to Temurin 21.0.8+9, and `COURSIER_MAVEN_ROOT` set to
the cache containing the eight digested artifacts:

```sh
crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/run
```

This issue remains a typer-only increment. It adds no recursive `eval`
behavior, general arity support, backend lowering, TASTy output, or public
compiler-session API.
