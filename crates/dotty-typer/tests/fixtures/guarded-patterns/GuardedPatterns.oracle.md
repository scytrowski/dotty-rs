# Match guard oracle

The normalized Scala 3.9.0 typed shapes in `GuardedPatterns.scala` preserve
the pattern, optional guard, and body on each typed case:

```text
case item if predicate(item) => item
  CaseDef(
    pattern = Bind(item, Ident(_) : Int) : TermRef(case-local item),
    guard = Apply(Ident(predicate), Ident(item) : TermRef(case-local item)) : Boolean,
    body = Ident(item) : TermRef(case-local item)
  ) : Int

case item @ _ if predicate(item) => item
  CaseDef(
    pattern = Bind(item, Ident(_) : Int) : TermRef(case-local item),
    guard = Apply(Ident(predicate), Ident(item) : TermRef(case-local item)) : Boolean,
    body = Ident(item) : TermRef(case-local item)
  ) : Int

case 1 if true => 1
  CaseDef(
    pattern = Typed(Literal(1) : Constant(1)) : Constant(1),
    guard = Literal(true) : Constant(true),
    body = Literal(1) : Constant(1)
  ) : Constant(1)

case Stable if true => 1
  CaseDef(
    pattern = Ident(Stable) : TermRef(GuardedPatterns.Stable),
    guard = Literal(true) : Constant(true),
    body = Literal(1) : Constant(1)
  ) : Constant(1)

case _ if true => 1
  CaseDef(
    pattern = Ident(_) : Int,
    guard = Literal(true) : Constant(true),
    body = Literal(1) : Constant(1)
  ) : Constant(1)
```

The guard is typed as an expression expected to conform to canonical Boolean,
after pattern bindings enter the case scope and before the body is typed. Its
own expression type is retained; the CaseDef own type remains the body type.
This is a normalized semantic projection pinned to Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`, not a checked-in
`-Vprint:typer` transcript.
