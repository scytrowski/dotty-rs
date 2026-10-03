# Wildcard pattern oracle

Expected normalized pattern tree for `case _` in `WildcardPatterns.scala`:

```text
Ident(_): selector prototype type
```

This records the relevant source-level contract, not a checked-in compiler
transcript. It is pinned to Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`: `Typer.typedIdent` returns the
wildcard tree with the supplied pattern prototype, and `Typer.typedMatch`
preserves a constant selector type while widening other selector types. See
the source links in `docs/typer-v0.1-compatibility.md`.
