# Generated Product2 `unapply` signature (Scala 3.9.0)

The pinned Scala 3.9.0 source revision
`777528f19a58e794c9954a42f433373472ec57f8` emits this companion method for
`case class Flagged(value: Int, enabled: Boolean)`:

```scala
def unapply(x$1: matching.Flagged): matching.Flagged
```

The input is one `Flagged` instance and the result is that same canonical
source class. The original ordered constructor fields remain
`value: Int` and `enabled: Boolean`; they are checked separately and retain
their class-owned identities. The method-owned synthetic `x$1` is distinct
from both fields and from the generated `apply` formals. Completion accepts
the previous one-field `Box(value: Int)` shape and this heterogeneous
Product2 shape only. Other Product2 shapes, defaults, repeated parameters,
multiple clauses, type parameters, mutable fields, and explicit `unapply`
collisions remain unsupported or use the explicit source method.

Before this increment, the canonical `Flagged$` `unapply` identity had source
provenance back to the `Flagged` `TypeDef` and a method-owned `x$1` parameter,
but the method stayed incomplete. Its constructor tree contains separate
`ValDef` nodes for `value` and `enabled`; signature completion rejected it
at the one-field clause-shape guard. The after receipt below records the
completed plain `MethodType`. Ordinary `Flagged.unapply` member lookup selects
that same generated symbol. The existing Scala oracle fixture is
[`Positive.scala39-typed-tree.txt`](../crates/dotty-typer/tests/fixtures/source-case-class-product2-3.9.0/Positive.scala39-typed-tree.txt).

The frozen whole-method attempts still stop after unapply resolution at the
missing `isEmpty` result-protocol member, as recorded in the Product2 source
baseline. This change does not complete `_1` or `_2`, type a whole `Match`, or
add general `ProductN` inference.
