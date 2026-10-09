# Literal singleton type oracle

`LiteralSingletons.scala` was compiled with the Scala 3.9.0 compiler artifacts
from the repository-pinned source revision
`777528f19a58e794c9954a42f433373472ec57f8`. Its saved `-Vprint:typer` output
records `true`, `false`, `1`, `2`, `'a'`, `1L`, `1.0f`, `1.0d`, and `"foo"` as
declared literal singleton types. The same boolean, integer, and string literals
conform to their underlying `Boolean`, `Int`, and `String` types when those
declared types are requested. Methods accepting and returning the same
boolean, integer, or string literal type also type successfully.

The source parser represents each literal singleton annotation as
`SingletonTypeTree(reference = Literal(...))`. The Rust regression checks the
exact constant payload, the source type-index entry, repeated projection cache
identity, equal and distinct literal values, stable identifier projection, and
rollback from a failing enclosing transaction. The sibling
`type-alias-literal-singletons.scala` fixture is included in the pinned Scala
3.9 parser comparison corpus.

The source projection subset is explicit: Boolean, Char, Int, Long, Float,
Double, and source String literals. Scala syntax has no Byte or Short literal
type token; an integer literal remains an Int literal type. `null` is rejected
as a type reference, while `Null` and `Unit` are ordinary class types rather
than literal singleton annotations. Class constants and UTF-16-only strings
belong to semantic/binary representations and are rejected by source literal
projection with `UnsupportedSingletonLiteralKind`.

Unequal literal conformance is rejected by the Scala compiler: a method taking
`false` cannot return its argument as `true`, and a method taking `2` cannot
return it as `1`. The pinned diagnostics retain the widened underlying types
(`Boolean` and `Int`) while showing the distinct constants. Literal equality
and underlying-type conformance are the follow-up relation cases; no source
Typer semantics change in this issue.
