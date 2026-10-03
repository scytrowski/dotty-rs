# Boolean extractor oracle

`BooleanExtractors.scala` is pinned to Scala 3.9.0 at revision
`777528f19a58e794c9954a42f433373472ec57f8`; the compiler transcript is kept in
`BooleanExtractors.scala39-typed-tree.txt`.

The `Even()` case is a Boolean extractor pattern with no nested pattern
arguments. In the typed tree, its `UnApply` function is the exact selected
`Even.unapply` method, its own type is the selector prototype (`Int`), and both
its pattern and implicit lists are empty.

The normalized pattern projection is:

```text
case Even()
  UnApply(
    function = Select(
      Ident(Even) : TermRef(Even),
      unapply
    ) : TermRef(Even.unapply),
    implicits = [],
    patterns = []
  ) : Int
```
