# Generic source case-class Product1 projection (Scala 3.9.0)

Source case-class `Product1` completion has a narrow generic rule for the
canonical `scala.util.Either` cases. The reference definitions are
[`Left` and `Right` in the pinned Scala 3.9.0 `Either.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/library/src/scala/util/Either.scala):

```scala
sealed abstract class Either[+A, +B] extends Product
final case class Left[+A, +B](value: A) extends Either[A, B]
final case class Right[+A, +B](value: B) extends Either[A, B]
```

Completion authenticates the canonical source class and its ordered two
parameters, one immutable constructor field, the exact source-derived selector
and field identities, and the canonical source `scala.util.Either` class parent
whose arguments are those class parameters in the same order. `_1` remains a
distinct class-owned method identity, its declaration result retains the
corresponding source type parameter, and member adaptation substitutes the
matching receiver argument (`A` for `Left`, `B` for `Right`). The original
applied `Either` parent is preserved alongside `Object` and canonical
`scala.Product` ancestry.

The rule does not infer identity from field spelling. Reversed parent arguments,
an unrelated or non-parameter field type, wrong class arity, multiple fields,
curried constructors, higher-kinded parameters, and noncanonical parents do not
receive this projection. Failed source completion rolls back selector and class
metadata; a valid sibling can still complete in the same session.

This increment covers direct field projection and ClassInfo only. Generated
`unapply` typing, extractor patterns, `Match`, and `Either.swap` completion
remain separate compatibility surfaces. Regression coverage lives in
`crates/dotty-typer/src/typer/tests/issue_2242_generic_product1.rs`.

Scala compatibility target: revision
`777528f19a58e794c9954a42f433373472ec57f8`.
