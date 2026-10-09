# Literal singleton type oracle

`LiteralSingletons.scala` was compiled with the Scala 3.9.0 compiler artifacts
from the repository-pinned source revision
`777528f19a58e794c9954a42f433373472ec57f8`. Its saved `-Vprint:typer` output
records `true`, `false`, `1`, and `"foo"` as the declared literal singleton
types. The same literals conform to their underlying `Boolean`, `Int`, and
`String` types when those declared types are requested. Methods accepting and
returning the same boolean, integer, or string literal type also type
successfully.

The source parser represents each literal singleton annotation as
`SingletonTypeTree(reference = Literal(...))`. The Rust regression checks the
exact boolean, integer, and interned string constant values. The sibling
`type-alias-literal-singletons.scala` fixture is included in the pinned Scala
3.9 parser comparison corpus.

Unequal literal conformance is rejected by the Scala compiler: a method taking
`false` cannot return its argument as `true`, and a method taking `2` cannot
return it as `1`. The pinned diagnostics retain the widened underlying types
(`Boolean` and `Int`) while showing the distinct constants. Literal equality
and underlying-type conformance are the follow-up relation cases; no source
Typer semantics change in this issue.
