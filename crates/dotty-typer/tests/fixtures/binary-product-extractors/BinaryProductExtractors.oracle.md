# Binary product extractor oracle

Pinned to Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`. The fixture exercises a direct
`unapply` result that conforms to `scala.Product` and the Option-like `get`
result path. For the latter, selectors come from the type of `get`, even when
the wrapper itself also declares `_1` and `_2`. Its normalized typed pattern
shapes are:

```text
direct:
  UnApply(
    DirectPair.unapply,
    patterns = [Bind(left, _): Int, Bind(right, _): Boolean],
    type = Any
  )

throughGet:
  UnApply(
    GetPair.unapply,
    patterns = [_ : Int, Bind(right, _): Boolean],
    type = Any
  )
```

Both patterns retain one exact `unapply` function and two ordered child
patterns. Scala's `Applications.UnapplyArgs` obtains direct product component
types from the product protocol; for `get` results it also supports the
conventional numbered selectors when their count matches the source pattern.
The implementation's current semantic subset discovers parameterless `_1`,
`_2`, and `_3` members through ordinary member lookup, uses `_1` then `_2`, and
rejects incomplete or wider selector sets instead of truncating them.

The reference implementation is
[`Applications.scala` at the pinned revision](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/typer/Applications.scala#L194-L294).
