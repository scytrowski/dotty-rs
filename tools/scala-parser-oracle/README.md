# Scala parser oracle

This tool exposes a small, stable JSON view of the syntax tree produced by
the Scala 3.9.0 compiler parser. It is a development oracle for `dotty-parser`,
not a production dependency of the Rust workspace.

The tool is intentionally pinned to Scala 3.9.0, JDK 25, and sbt 2.0.9. Run
it with the selected SDKMAN defaults:

```text
./run path/to/input.scala
./run --mode pattern path/to/pattern.scala
./run --mode compilation path/to/source-unit.scala
./run --mode block-erased path/to/erased-function-type.scala
./run --batch path/to/manifest.tsv
```

The batch manifest contains one tab-separated `mode` and absolute source path
per line. `compare.sh` uses this mode automatically: Scala parses the whole
corpus in one JVM, while the Rust dump tool is built and run once. This avoids
restarting sbt, Scala, Cargo, and Python for every fixture.

With no argument, `run` reads the source from standard input. The output keeps
only parser-facing information: node kind, source span, names, literal source
text, application kind, operators, and child nodes. It does not use `Tree.show`, because that output is a
compiler presentation rather than a compatibility protocol.

Compiler source offsets are UTF-16 code-unit offsets. Consumers comparing them
with Rust source spans must convert them to UTF-8 byte offsets first.

The default fixtures cover the expression forms currently represented by the Rust
parser: identifiers, numeric and string literals, `this`, parentheses, the
empty tuple, tuples, simple selections and applications, `super`, `new`,
simple type applications, applied union/intersection types, repeated suffix
chains, and brace blocks.
Numeric suffix fixtures also cover `Long`, `Float`, and `Double` literals.
Applied-type fixtures also cover Scala wildcard arguments, including unbounded
wildcards and `>: ... <: ...` bounds. The parser keeps those bounds in the
existing `TypeBoundsTree` and accepts wildcard syntax only in nested type
argument positions. Type-definition fixtures also cover path singleton types
such as `x.type` and `foo.bar.type`; their term-valued references are rendered
as `SingletonTypeTree`, while ordinary qualified type references remain in the
type namespace. They also cover literal singleton aliases for strings, characters,
numbers, booleans, and `null`, including applied and union compositions.
Named tuple type fixtures preserve element names as `NamedArg` children of the
shared `Tuple` node and exercise full type expressions inside named elements;
the function and context-function arrow lookahead remains compared separately.
Type projection fixtures cover `T#Member`, applied qualifiers such as
`F[A]#Result`, dotted qualifiers, and repeated projection suffixes.
Annotated type fixtures cover single and repeated annotations, annotation
arguments, and binding around union, intersection, and parenthesized types.
Refined-type fixtures cover `RefinedTypeTree` parents with abstract, aliased,
and upper-bounded `TypeDef` members, declaration-only `val`, `var`, and `def`
members, multiple members in source order, annotated and applied parents, and
the parentless refinement form. RHS-bearing declarations, default arguments,
class-like members, modifiers, refined type members, capture-checking
refinements, and capture-checking type syntax remain outside this parser
milestone. Core match types are compared in the type-definition fixtures,
including ordered `CaseDef` children, applied/tuple/infix patterns, full type
results, and upper-bounded match-type aliases. Generic symbolic and identifier
infix types are compared in
the type-definition fixtures, including shared precedence, right-associative
colon operators, and newlines after an operator.
Backquoted identifier fixtures cover standalone and selected names. The
corpus also covers the initial `Expr1` subset: ordinary assignment, the
narrow bare-identifier named-argument form, `using` argument lists, expression
type ascriptions, indented colon arguments, and `if`/`while` expressions,
including basic indented bodies, plus braced and indented `match` expressions
with case patterns, guards, and bodies. The current control-flow subset also
includes `throw`, bare/value `return`, and source-level `try`/`catch`/`finally`,
including their basic indented forms.
The lambda fixtures cover explicit function literals with single, empty,
multiple, typed, wildcard, and context-function parameters, plus nested,
applied, argument, block, and indented bodies. Context-function parameters are
compared through their normalized `given` marker on `ValDef`.
The polyfunction fixtures cover source polymorphic function literals and
polymorphic function types: type-parameter clauses with lower and upper
bounds, ordinary context-bound wrappers (including aliases), nested type
bodies, context/by-name bodies, and
value-parameter function bodies. Type-lambda fixtures additionally cover
applied, tuple, union/intersection, and nested polymorphic bodies; context
bounds are consumed, diagnosed, and normalized to Dotty's stripped source-tree
shape. Explicit variance in `ParamOwner::Type` is diagnosed rather than
included as a valid fixture. The
placeholder fixtures cover synthetic-function lowering for infix expressions,
selections, applications, multiple parameters, and nested placeholder scopes;
the oracle normalizes Dotty's `WildcardFunction` and generated names to the
shared Rust representation.
It intentionally does not claim coverage for the rest of Scala's expression,
type, or argument grammar.

Fixtures under `fixtures/patterns/` use the explicit `pattern` mode. Scala
mode calls Dotty's real `Parser.pattern()` entry and Rust mode calls the
parser's standalone pattern-fragment entry; pattern fixtures are not wrapped
in synthetic `match` expressions. The normalized tree compares `Bind`,
`Alternative`, `Typed`, extractor-style source `Apply`, and named pattern
arguments as they appear before semantic extractor lowering.

Fixtures under `fixtures/definitions/` cover the initial class-like
definition subset in block mode: class, trait, object, and enum nodes; primary
constructor clauses; simple parent applications, including parameterized enum
case parents and ordered parent lists; and braced or indented
template bodies. They also cover source annotations, hard and supported soft
modifiers, and qualified visibility. The normalized definition metadata
compares modifier order, visibility, visibility qualifiers, and annotation
trees; `ModuleDef.metadata` is included just like metadata on the other
definition nodes. The renderer exposes the source-level `Trait` and `Enum`
distinctions,
modern `given` aliases and structural templates, and `ExtMethods` parameter
clauses with method/export children. Anonymous given names remain empty rather
than being replaced with synthetic names. Extension fixtures cover generic and
`using` prefixes, receiver ordering, braced/indented bodies, rejection of a
colon directly after the header, method modifiers and annotations, multiple
methods, and exports. Constructor parent applications remain in the same
`New`/`Select`/`Apply` shape as the Scala parser; semantic given synthesis and
extension lowering are not compared.
The `type-function-erased-*.scala` and `type-context-function-erased-*.scala`
fixtures use a separate `block-erased` mode: their source imports
`scala.language.experimental.erasedDefinitions`, and the Rust side enables the
matching `ParserFeatures` policy. The normalized
`FunctionWithMods.erased_params` vector is compared positionally; context
functions also expose their `Given` modifier through the normal normalized
metadata. The corpus includes the narrow leading unnamed ordinary-function
form `(erased A, B) => C`; its parameters remain type trees and the first
`erased_params` entry is `true`. It also covers the corresponding context-
function form `(erased A, B) ?=> C`, where the normalized function exposes
the `Given` modifier and the same positional erased metadata.
Singleton enum cases expose a separate `enum_case: true` metadata field; this
is distinct from ordinary `case` metadata, and comma-separated singleton cases
remain one normalized `PatDef`.

Fixtures under `fixtures/compilation/` use `compilation` mode. Scala calls the
real `Parser.compilationUnit()` entry and Rust calls `parse_compilation_unit`.
The corpus covers empty and explicit package roots, nested packages, ordered
top-level definitions, opaque type aliases (including parameterized and
bounded forms), and import/export clauses. Dotty returns `EmptyTree`
for an empty compilation unit; the Scala renderer normalizes that one case to
the Rust parser's documented zero-width empty `PackageDef` root.
