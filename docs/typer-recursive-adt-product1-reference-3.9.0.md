# Recursive ADT Product1 reference projection (Scala 3.9.0)

This increment completes the bounded source case class
`final case class Succ(inner: Expr) extends Expr` through the existing source
case-class selector path. It does not change generated `unapply`, Match typing,
or whole-method typing.

## Oracle and source identity

The source is the pinned
[`Expr.scala`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/Expr.scala)
fixture from the recursive ADT baseline. Its frozen Scala 3.9.0 oracle uses
revision `777528f19a58e794c9954a42f433373472ec57f8`, Temurin 21.0.8+9, and the
artifact manifest recorded alongside that fixture. The Scala typed tree records
`Succ._1: Expr`, `Succ.inner: Expr`, and the direct source parent `Expr`.

The Rust positive class characterization runs the real Lexer, Parser, Namer,
and `SourceTyper` with the authentic `scala/Product.tasty` fixture and JDK
resolver. It verifies the canonical source parent symbol and the classpath
`scala.Product` symbol. The Rust unit projection test separately invokes the
existing `unary_extractor_component_type` path on a completed recursive source
case class and verifies that its sole product component resolves to the same
source `Expr` symbol.

## Before and after

[`rust-product1-projection.tsv`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/rust-product1-projection.tsv)
records the narrow projection before and after this change. Before the change,
`Succ` and `_1` failed with `UnsupportedProduct1FieldType`; after it, the
class retains `Object`, the exact source `Expr`, and one canonical
`scala.Product` edge, while one distinct class-owned parameterless `_1` method
returns `Expr`. The original constructor `Field(inner)` remains a field.

The generated `Succ.unapply` method-clause blocker described in the initial
version of this report was resolved separately by issue #2035. The unchanged
`unwrap` and `eval` roots now complete through the existing source Match typer;
their two-root set is unchanged. This Product1 increment still changes only
class metadata and selector projection. `Num._1: Int`, the existing
Box/Product2 selectors, and Created/Deleted selectors retain their prior
receipts. See the [generated-unapply report](typer-recursive-adt-generated-unapply-3.9.0.md)
for the signature completion evidence.

## Reproduction

```sh
JAVA_HOME=/path/to/temurin-21.0.8+9 cargo +1.98.1 test -p dotty-typer \
  --lib recursive_source_product1_projects_its_canonical_trait_reference --locked
JAVA_HOME=/path/to/temurin-21.0.8+9 cargo +1.98.1 test -p dotty-typer \
  --test source_recursive_adt_baseline --locked
```

The Scala oracle and all source/control hashes are pinned in the fixture's
`run`, `source-hashes.tsv`, and `scala-artifacts.sha256.tsv` files.
