# Singleton type oracle

`SingletonTypes.scala` was checked with Scala 3.9.0 at repository-pinned
compiler revision `777528f19a58e794c9954a2f433373472ec57f8`. The adjacent
`SingletonTypes.scala39-typed-tree.txt` records the `-Vprint:typer` output.

The fixture pins these source forms and their stable paths:

- `value.type` is printed as `this.value.type`.
- `this.type` remains `this.type`.
- `SingletonOuter.this.type` retains its qualified enclosing owner.
- `inner.nested.type` is printed as `this.inner.nested.type`.
