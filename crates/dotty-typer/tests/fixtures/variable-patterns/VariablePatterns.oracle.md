# Variable pattern oracle

The normalized Scala 3.9.0 typed shape for the cases in
`VariablePatterns.scala` is:

```text
case item => item
  CaseDef(
    pattern = Bind(item, Ident(_) : Int) : TermRef(item),
    guard = None,
    body = Ident(item) : TermRef(item)
  ) : Int

case item @ _ => item
  CaseDef(
    pattern = Bind(item, Ident(_) : Int) : TermRef(item),
    guard = None,
    body = Ident(item) : TermRef(item)
  ) : Int
```

Each case-local `item` has value info `Int`; the pattern and body references
target the same symbol. The implicit variable source `Ident(item)` maps to the
typed Bind root, whose wildcard child is synthetic. The explicit source Bind
and wildcard both retain their own source mappings.

In `shadow`, the typed body reference targets the case binding and shadows the
method parameter only within that case. These normalized observations are
pinned to Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`, using the `typedPattern` / `typedBind`
and ordinary case-local term lookup behavior. This is a semantic projection,
not a checked-in `-Vprint:typer` transcript.
