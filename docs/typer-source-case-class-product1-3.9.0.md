# One-field source case class product projection (Scala 3.9.0)

For a monomorphic top-level source `case class Box(value: Int)` with one
ordinary, explicitly typed constructor parameter and no explicit parents, the
Namer enters a distinct class-owned `_1` method identity derived from the
original constructor parameter. Typer completes that selector as a
parameterless `Int` method and adds the canonical `scala.Product` parent when
that exact Product trait is found in the source package scope or through the
existing symbol resolver. Missing or non-canonical `scala` package evidence,
or a package with no unique Product, defers completion transactionally, so a
later resolution attempt can retry.
The generated selector can also be completed first; that route authenticates
its derived source identity and completes its owning class. The existing
extractor path retrieves `_1` through normal member lookup and accepts its
plain parameterless method result only for this authenticated generated
selector, preserving empty-clause Option-like method rejection.

This establishes the bounded product projection only. It does not claim full
`Match` method typing: the fixture's standalone source session has no Scala
library classpath, so its roots still defer when the option-like protocol is
probed without a resolvable `scala.Product`. The focused projection regression
provides the canonical Product declaration and verifies `Box.unapply`'s
completed `Box` result reaches `_1: Int`.

The compact transition receipt is
[`product-projection.tsv`](../crates/dotty-typer/tests/fixtures/source-case-class-match-3.9.0/product-projection.tsv).
The whole-method source fixture receipt after this increment is
[`rust-after-product.tsv`](../crates/dotty-typer/tests/fixtures/source-case-class-match-3.9.0/rust-after-product.tsv).
