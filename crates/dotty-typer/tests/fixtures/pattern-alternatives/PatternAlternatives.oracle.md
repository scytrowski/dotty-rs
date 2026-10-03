# Pattern alternative oracle

The normalized Scala 3.9.0 shapes in `PatternAlternatives.scala` preserve
literal and stable-value branch order:

```text
case 1 | 2
  Alternative(
    Literal(1) : Constant(1),
    Literal(2) : Constant(2)
  ) : Int

case First | Second
  Alternative(
    Ident(First) : TermRef(PatternAlternatives.First),
    Ident(Second) : TermRef(PatternAlternatives.Second)
  ) : Int
```

Every branch is checked against the same selector prototype. Pattern
alternatives cannot introduce bindings. The typer joins widened branch types
through its existing bounded relation and does not perform GADT constraint
merging. The normalized projection is pinned to Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`; the companion
`PatternAlternatives.scala39-typed-tree.txt` records the compiler's
`-Vprint:typer` output.
