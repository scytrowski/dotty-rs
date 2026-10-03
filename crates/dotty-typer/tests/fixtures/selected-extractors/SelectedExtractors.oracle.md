# Selected extractor oracle

`SelectedExtractors.scala` and `PackageSelectedExtractor.scala` compile with
Scala 3.9.0 at pinned revision `777528f19a58e794c9954a42f433373472ec57f8`;
their typed trees are recorded in companion `*.scala39-typed-tree.txt` files.

The nested stable selections `extractors.SomeInt` and `extractors.Even` are
typed as ordinary stable references. Their source applications lower to
`UnApply` nodes whose function is the selected `unapply` member. `SomeInt`
passes its `get` type (`Int`) to its one nested pattern. `Even()` has no nested
patterns. In both cases `UnApply` keeps the selector prototype as its own type.

For the `SomeInt` case, the normalized projection is:

```text
case extractors.SomeInt(x)
  UnApply(
    function = Select(
      Select(
        Ident(extractors) : TermRef(extractors),
        SomeInt
      ) : TermRef(extractors.SomeInt),
      unapply
    ) : TermRef(extractors.SomeInt.unapply),
    implicits = [],
    patterns = [Bind(x, Ident(_) : Int) : TermRef(case.x)]
  ) : Any
```

For the `Even()` case, the same selected-member shape has an empty pattern
list and `Int` as the `UnApply` type:

```text
case extractors.Even()
  UnApply(
    function = Select(
      Select(
        Ident(extractors) : TermRef(extractors),
        Even
      ) : TermRef(extractors.Even),
      unapply
    ) : TermRef(extractors.Even.unapply),
    implicits = [],
    patterns = []
  ) : Int
```

The package-qualified `p.Extractor()` case has the same `UnApply` shape, with
`Ident(p)` as its package reference and the exact `p.Extractor.unapply`
selection as the function. Its nested and implicit lists are empty, and its
own type remains the selector prototype `Int`.
