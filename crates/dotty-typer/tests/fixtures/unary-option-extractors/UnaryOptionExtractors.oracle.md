# Unary Option-like extractor oracle

`UnaryOptionExtractors.scala` compiles with Scala 3.9.0 at pinned revision
`777528f19a58e794c9954a42f433373472ec57f8`. The fixture covers wildcard,
variable, literal, typed, guarded, and recursively nested unary extractors.
`UnaryOptionExtractors.scala39-typed-tree.txt` records the compiler's
`-Vprint:typer` output.

For `SomeInt(x)`, the normalized semantic pattern is:

```text
case SomeInt(x)
  UnApply(
    function = Select(
      Ident(SomeInt) : TermRef(SomeInt),
      unapply
    ) : TermRef(SomeInt.unapply),
    implicits = [],
    patterns = [Bind(x, Ident(_) : Int) : TermRef(case.x)],
  ) : Any
```

The `UnApply` keeps the selector prototype (`Any` here). The nested pattern is
typed against `get`'s result type (`Int`), and the case-local binder therefore
has value type `Int`. The extractor result itself is not stored as the
`UnApply` type.

Scala 3.9's `Applications.isGetMatch` accepts a result with a parameterless
`isEmpty` whose widened type is Boolean and a parameterless `get` value. Its
`UnapplyArgs.argTypes` uses the `get` type for a unary pattern after checking
the product protocol. This implementation deliberately supports that narrow
Option-like subset and leaves product, Boolean, and sequence protocols for
their separate increments. See the pinned
[`Applications.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a2f433373472ec57f8/compiler/src/dotty/tools/dotc/typer/Applications.scala#L59-L107)
and [`UnapplyArgs`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a2f433373472ec57f8/compiler/src/dotty/tools/dotc/typer/Applications.scala#L194-L230).
