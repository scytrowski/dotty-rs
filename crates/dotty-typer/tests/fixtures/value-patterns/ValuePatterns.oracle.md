# Literal and stable value pattern oracle

The normalized Scala 3.9.0 pattern shapes for `ValuePatterns.scala` are:

```text
case 1 => 1
  Typed(Literal(1) : Constant(1)) : Constant(1)

case true => 1
  Typed(Literal(true) : Constant(true)) : Constant(true)

case Stable => 1
  Ident(Stable) : TermRef(ValuePatterns.Stable)

case Nested.selected => 1
  Select(Ident(Nested), selected) : TermRef(ValuePatterns.Nested.selected)

case bound @ 1 => bound
  Bind(bound, Typed(Literal(1) : Constant(1)) : Constant(1))
    : TermRef(case-local bound, info = selector prototype)

case bound @ Stable => bound
  Bind(bound, Ident(Stable) : TermRef(ValuePatterns.Stable))
    : TermRef(case-local bound, info = selector prototype)
```

Literal patterns retain their exact constant type and are checked for
compatibility with the selector prototype. Stable identifiers and selections
are resolved through ordinary term lookup and keep the resolved symbol in the
typed reference. Explicit binders take their value type from the selector
prototype, while the nested pattern keeps its own type. This is a semantic
projection pinned to Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`, not a checked-in `-Vprint:typer`
transcript.
