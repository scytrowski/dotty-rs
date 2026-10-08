# Function class identity oracle

`FunctionClassIdentities.scala` pins ordinary function types at arities zero,
one, and two, plus a contextual function with one input. Its
`-Vprint:typer` output is recorded in `FunctionClassIdentities.scala39-typed-tree.txt`. The reference is
Scala 3.9.0 at compiler revision
`777528f19a58e794c9954a42f433373472ec57f8`.

The pinned compiler's `QuotesImpl.defn.FunctionClass` delegates to
`Definitions.FunctionSymbol(arity, isContextual)`. `FunctionSymbol` selects a
cached `FunType` family whose prefixes are `Function` and `ContextFunction`;
`FunType` resolves the named class in package `scala`. Thus the fixture's
canonical identities are:

| Source type | Arity | Canonical class |
| --- | ---: | --- |
| `() => Int` | 0 | `scala.Function0` |
| `Int => String` | 1 | `scala.Function1` |
| `(Int, String) => Boolean` | 2 | `scala.Function2` |
| `String ?=> Int` | 1 | `scala.ContextFunction1` |

Arity counts input parameters only; the result is the final type argument of
ordinary `FunctionN`. Contextual identity is a separate `ContextFunctionN`
class at the same arity. Scala 3.9 synthesizes these function class identities
on demand. Above `Definitions.MaxImplementedFunctionArity` (22), source
function classes are still synthesized, while runtime erasure uses
`scala.runtime.FunctionXXL`. This source typer increment intentionally supports
only arities 0 through 22 and reports larger requests explicitly; it does not
invent `FunctionXXL` as a source type constructor.

Pinned source references:

- `library/src/scala/quoted/Quotes.scala`, `Quotes.reflect.defnModule.FunctionClass`.
- `compiler/src/scala/quoted/runtime/impl/QuotesImpl.scala`, implementation of `FunctionClass`.
- `compiler/src/dotty/tools/dotc/core/Definitions.scala`, `FunType`, `FunctionSymbol`, `MaxImplementedFunctionArity`, and `functionTypeErasure`.
