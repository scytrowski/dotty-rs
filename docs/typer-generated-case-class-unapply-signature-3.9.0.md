# Generated case-class `unapply` signature (Scala 3.9.0)

The pinned Scala 3.9.0 fixture at revision
`777528f19a58e794c9954a42f433373472ec57f8` records the generated method for
`case class Box(value: Int)` as a plain one-argument method:

```scala
def unapply(x$1: matching.Box): matching.Box
```

The argument is the case-class instance, and the result is the same
source-owned `Box` class. It is not `Option[Int]`. The Rust typer now completes
the canonical generated method through `complete_symbol` as a `MethodType`
with one non-erased, non-varargs parameter and `MethodKind::Plain`. Its
parameter has a distinct owner-specific derived symbol under `unapply`; it
does not reuse the constructor's `value: Int` parameter or generated `apply`
parameter. Ordinary selection of `Box.unapply` resolves to that exact method
symbol.

Completion authenticates the generated method's role, source tree, class,
companion and module-class owner before accepting the signature. This increment
supports only a non-generic case class with one initialized ordinary `Int`
constructor accessor. Unsupported constructor shapes leave the method
incomplete and can be retried without partial signature state.

## Remaining whole-method blocker

The frozen `unwrap` and `classify` method roots now pass generated-method
completion and ordinary unapply resolution. They stop at
`ExtractorResultMemberNotFound` for the product selector required by pattern
typing. The updated `rust-after-unapply.tsv` records this first remaining
error, including source spans. The historical `rust-baseline.tsv` still records
the earlier missing generated method state. This change does not add product
selectors or claim successful typing of either whole method.

The checked-in typed-tree oracle is
`crates/dotty-typer/tests/fixtures/source-case-class-match-3.9.0/Positive.scala39-typed-tree.txt`.
