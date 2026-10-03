# Simple extractor resolution oracle

`SimpleExtractor.scala` compiles with the pinned Scala 3.9.0 compiler. Its
normalized extractor tree has the selected `unapply` member as the function,
and the selector prototype as the `UnApply` type:

```text
case SimpleExtractor(_)
  UnApply(
    function = Select(
      Ident(SimpleExtractor) : TermRef(SimpleExtractor),
      unapply
    ) : TermRef(SimpleExtractor.unapply),
    implicits = [],
    patterns = [Ident(_) : Any]
  ) : Any
```

The function selection retains the exact `SimpleExtractor.unapply` symbol.
Its one plain parameter has type `Any`, and its result type is `Option[Int]`;
this increment records that result without interpreting the Option-like
protocol or recursively typing the source argument. The normalized projection
is pinned to Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`. The companion
`SimpleExtractor.scala39-typed-tree.txt` records `-Vprint:typer` output.
