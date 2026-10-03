# Typed pattern oracle

The normalized Scala 3.9.0 typed shapes for the supported source forms in
`TypedPatterns.scala` are:

```text
case _: Int => 1
  CaseDef(
    pattern = Typed(Ident(_) : Int, TypeTree(Int) : Int) : Int,
    guard = None,
    body = Literal(1) : Constant(1)
  ) : Constant(1)

case item: Int => item
  CaseDef(
    pattern = Bind(item, Typed(Ident(_) : Int, TypeTree(Int) : Int) : Int)
      : TermRef(case-local item),
    guard = None,
    body = Ident(item) : TermRef(case-local item)
  ) : TermRef(case-local item)

case item @ (_: Int) => item
  CaseDef(
    pattern = Bind(item, Typed(Ident(_) : Int, TypeTree(Int) : Int) : Int)
      : TermRef(case-local item),
    guard = None,
    body = Ident(item) : TermRef(case-local item)
  ) : TermRef(case-local item)
```

The type syntax maps to the reified `TypeTree(Int)`. The typed wildcard and
typed test each carry `Int`; both variable forms introduce one case-local
binding with info `Int`, and the body reference targets that binding. The
explicitly parenthesized source pattern does not add a typed parenthesis node.
These normalized observations are pinned to Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`; this is a semantic projection,
not a checked-in `-Vprint:typer` transcript.
