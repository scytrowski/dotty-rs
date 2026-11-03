# Structural Product2 identities in Namer (Scala 3.9.0)

Issue #2066 extends the existing bounded source case-class publishers to a
top-level, monomorphic case class with one ordinary constructor clause and
exactly two canonical parameter `ValDef`s. Namer checks source-tree and class
identity, unique original parameter trees, and ordinary parameter metadata.
It does not inspect parameter names or resolve or compare their declared
types. Generic, curried, defaulted, by-name, repeated, contextual, or
noncanonical parameter shapes remain outside this identity increment.

For each eligible class, `_1` and `_2` are class-owned synthetic Methods
derived from the first and second original constructor parameter trees. The
existing `(owner, source, tree)` keys keep same-typed fields independent, and
an explicit selector prevents only its colliding generated identity. The
generated companion `unapply` remains a separate synthetic Method indexed by
the source class and `Unapply` role; its sole synthetic `x$1` formal has its
own method scope. An exact explicit companion `unapply` replaces generation;
an alias or overload whose parameter identity cannot be compared remains
deferred through the existing unsupported-shape rollback path.

The generated methods retain `SymbolInfo::Missing`. This change publishes
canonical identities only; it does not complete signatures, add Product
parents, or change Typer production behavior. The lookup-only Add receipt is
[`rust-after-namer-identities.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/rust-after-namer-identities.tsv).
The prior #2061 class and root receipts remain immutable historical evidence;
post-change root outcomes are recorded separately in
[`rust-after-namer-identities-two-root.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/rust-after-namer-identities-two-root.tsv).

Pinned Scala 3.9.0 controls use revision
`777528f19a58e794c9954a42f433373472ec57f8`, Temurin 21.0.8+9, and the eight
verified artifacts already recorded by the Product2 and recursive-ADT
fixtures. The new Namer oracle fixture at
[`structural-product2-3.9.0`](../crates/dotty-namer/tests/fixtures/structural-product2-3.9.0/)
compiles Add plus renamed, reordered, and same-typed Pair controls and checks
the generated Scala selectors. The existing runners cover Flagged, explicit
selectors and unapply replacement, unrelated same-spelled extractors, and
invalid pattern arities/types. Rust regression receipts and unchanged
controls are summarized in
[`namer-structural-product2-3.9.0.tsv`](namer-structural-product2-3.9.0.tsv).

Reproduce the pinned oracle checks with `JAVA_HOME`, `SCALA39_ROOT`, and
`COURSIER_MAVEN_ROOT` set to the verified local toolchains/cache:

```sh
crates/dotty-typer/tests/fixtures/source-case-class-product2-3.9.0/run
crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/run
crates/dotty-namer/tests/fixtures/structural-product2-3.9.0/run
```
