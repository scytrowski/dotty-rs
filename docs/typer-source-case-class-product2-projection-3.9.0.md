# Source case-class Product2 projection (Scala 3.9.0)

For a canonical, top-level, monomorphic source declaration
`case class Flagged(value: Int, enabled: Boolean)`, Typer completes the two
Namer-published selectors on the original `Flagged` class:

```scala
def _1: Int
def _2: Boolean
```

The fields are the original ordered constructor `ValDef`s and retain their
class-owned `Field` symbols. Each selector is a distinct class-owned `Method`
derived from its corresponding field tree. Both signatures are plain,
parameterless `MethodType`s. `Flagged` receives the resolver-authenticated
canonical `scala.Product` trait as one parent edge; its existing parent list is
preserved and repeated completion does not duplicate the edge.

The ordinary extractor protocol consumes the generated
`unapply(x$1: Flagged): Flagged` result through normal member lookup and type
relation. A `Flagged(_, _)` pattern reads component types in source order as
`Int, Boolean`. `_3` remains absent. The focused regression calls the existing
pattern typer directly; it does not type a whole `Match` method.

The older whole-method Rust receipt intentionally runs without a Scala
classpath or a `scala.Product` fixture. Its first failure therefore moves from
the old selector gate to `TypeNameNotFound(scala.Product)` when class
completion requests the canonical parent. That receipt remains a negative
whole-method boundary; focused projection tests supply a canonical local
Product trait and exercise only class completion and direct pattern typing.

## First blocker at the #1965 base

At `c9eafc1cc616701fa17f6be50ccbb03cc608b4b1`,
`source_case_class_product_selector` accepted only a single constructor value
parameter and looked up only `_1`. Thus the source-derived `_1` and `_2`
identities already published by Namer were not authenticated for `Flagged`,
the selectors remained incomplete, the class did not acquire Product ancestry,
and the ordinary extractor could not read its two components. The issue extends
that existing source-authentication and class-completion path to exactly one or
two ordered fields; it does not add Namer identities or generic `ProductN`
support.

## Pinned oracle evidence

The checked-in Scala typed-tree fixture was produced from Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`. It records the canonical
`scala.Product` parent and class-owned `_1: Int` / `_2: Boolean` methods at
[`Positive.scala39-typed-tree.txt`](../crates/dotty-typer/tests/fixtures/source-case-class-product2-3.9.0/Positive.scala39-typed-tree.txt).
Its source hash is
`4b144913df1d2021359c44633dc2a23f21bf6288bf2528c86cc6f53d79b8ee31`; the
typed-tree hash is
`e00a94565775f4c6b3424e65e68f0a1014cea681d092dde36777ef17d5072350`.
`source-case-class-product2-3.9.0/run` documents the full oracle check. This
environment has Temurin JDK 21.0.13, while that check requires the recorded
Temurin 21.0.8+9 distribution; the full pinned runner was therefore not
re-executed here.

The compact semantic delta is in
[`typer-source-case-class-product2-projection-3.9.0.tsv`](typer-source-case-class-product2-projection-3.9.0.tsv).
