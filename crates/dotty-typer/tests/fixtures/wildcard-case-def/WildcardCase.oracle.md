# Wildcard CaseDef oracle

The normalized typed case for `case _ => 1` is:

```text
CaseDef(
  pattern = Ident(_) : Int,
  guard = None,
  body = Literal(1) : ConstantType(1)
) : ConstantType(1)
```

This is the narrow semantic projection recorded from Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`, not a checked-in `-Vprint:typer`
transcript. `Typer.typedCase` calls `typedPattern` with the selector prototype;
`TypeAssigner.assignType(untpd.CaseDef, pat, body)` assigns an ordinary term
case the body's type. The source links are in
[`typer-v0.1-compatibility.md`](../../../../../docs/typer-v0.1-compatibility.md).
