# Source function type projection oracle

`FunctionTypeProjection.scala` records the source shapes in #827, including
zero, one, and multiple input parameters, nested function results, a function
type used as another function's parameter, and a named contextual parameter. The
Scala 3.9.0 `-Vprint:typer` output is captured in
`FunctionTypeProjection.scala39-typed-tree.txt`.

The fixture was checked with Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`. The relevant shape rules are:

| Source shape | Parameter count | Canonical applied constructor |
| --- | ---: | --- |
| `() => A` | 0 | `scala.Function0[A]` |
| `A => B` | 1 | `scala.Function1[A, B]` |
| `(A, B) => C` | 2 | `scala.Function2[A, B, C]` |
| `A => B => C` | 1 at outer level | `scala.Function1[A, scala.Function1[B, C]]` |
| `(A => B) => C` | 1 | `scala.Function1[scala.Function1[A, B], C]` |
| `(x: A) ?=> B` | 1 | `scala.ContextFunction1[A, B]` |

The parser emits ordinary arrows as `UntypedNode::Function { params, body }`.
Contextual arrows are `UntypedNode::FunctionWithMods` with the `Given`
modifier, one erased-parameter flag per input, and an explicit result tree.
Named contextual inputs arrive as parameter definition trees; projection uses
their declared type while keeping the parameter name out of type identity.
Projection therefore counts only parameter trees and recursively projects
every input and result. Contextual identity is selected by the modifier rather
than inferred from spelling or from a mock classpath symbol.

Scala's pinned `Definitions.scala` synthesizes contextual function classes in
the `scala` package, as documented for #826. This fixture does not define the
wire/classpath representation for these classes. Erased parameters and other
function modifiers remain explicit unsupported cases in this increment.
