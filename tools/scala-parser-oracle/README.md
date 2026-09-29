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
Each Scala fixture is attached to a fresh `CompilationUnit`, so source-level
language imports and other per-unit parser features cannot leak to later
fixtures in the batch. The oracle initializes one Dotty compiler context with
the running classpath and applies Dotty's standard root imports to each fresh
unit context. This mirrors the parser phase's required compiler setup, including
the definitions used while parsing capture-checking types and while rendering
syntax diagnostics.

Fixtures under `fixtures/oracle-only/` exercise Dotty harness behavior that is
not currently part of the Rust parser's AST-equivalence corpus. They must still
produce a Scala syntax tree rather than an `OracleFailure`; `compare.sh` checks
that condition while deliberately skipping tree equivalence for those fixtures.
The capture-checking fixture guards against regressions in the initialized
definitions needed to build retaining annotations with multiple references.

With no argument, `run` reads the source from standard input. The output keeps
only parser-facing information: node kind, source span, names, literal source
text, application kind, operators, and child nodes. It does not use `Tree.show`, because that output is a
compiler presentation rather than a compatibility protocol.

Compiler source offsets are UTF-16 code-unit offsets. Consumers comparing them
with Rust source spans must convert them to UTF-8 byte offsets first.

For application argument lists, the Scala 3.9 parser's generic comma-list
helper creates a synthetic `???` placeholder spanning the trivia before `)`
after a trailing comma (zero-width when there is no trivia). The oracle omits
only that final child when the span contains whitespace and comments only: it
has no source argument and represents the accepted trailing-comma boundary,
not a missing argument in the normalized syntax tree. Explicit `???`
expressions and non-trailing missing arguments remain visible.

The default fixtures cover the expression forms currently represented by the Rust
parser: identifiers, numeric and string literals, `this`, parentheses, the
empty tuple, tuples, simple selections and applications, `super`, `new`,
`new` parent/mixin chains (including constructor applications on each parent),
and braced or indented anonymous template bodies after `new`,
string interpolations with simple and braced splices,
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
the parentless refinement form. The `refined-declaration-members.scala` fixture
compares retained abstract value, variable, and method declarations against
Scala 3.9. Focused Rust recovery tests cover annotation/modifier prefixes,
right-hand sides, and default arguments, which Scala 3.9 rejects and does not
retain in the refinement list. Capture-checking compilation fixtures also
compare caret disambiguation and pure function-type capture sets. RHS-bearing
refined declarations, default arguments, class-like members, and modifiers
remain unsupported. Refined type members and capture-checking refinements also
remain outside this parser milestone. Core match types are compared in the type-definition fixtures,
including ordered `CaseDef` children, applied/tuple/infix patterns, full type
results, and upper-bounded match-type aliases. Generic symbolic and identifier
infix types are compared in
the type-definition fixtures, including shared precedence, right-associative
colon operators, and newlines after an operator.
Backquoted identifier fixtures cover standalone and selected names. The
corpus also covers the initial `Expr1` subset: ordinary assignment, the
narrow bare-identifier named-argument form, `using` argument lists, expression
type ascriptions, indented colon arguments, and `if`/`while` expressions,
including basic indented bodies and local-definition sequences in indented
method, value-definition, lambda, control-flow, and for bodies nested inside
braces, plus braced and indented
`match` expressions with case patterns, same-line and line-broken guards, and
bodies. Braced case bodies also cover local statement sequences with `val` and
`var` definitions. The current
control-flow subset also includes `throw`, bare/value `return`, and source-level
`try`/`catch`/`finally`,
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

Fixtures under `fixtures/patterns/` use the explicit `pattern` mode. This
includes quote-pattern syntax and nested term splices. Scala mode calls
Dotty's real `Parser.pattern()` entry and Rust mode calls the
parser's standalone pattern-fragment entry; pattern fixtures are not wrapped
in synthetic `match` expressions. The normalized tree compares `Bind`,
`Alternative`, `Typed`, extractor-style source `Apply`, and named pattern
arguments as they appear before semantic extractor lowering. Type-pattern
fixtures cover `@unchecked` on bound variables, wildcards, and qualified types;
an expression fixture also checks the annotation inside a real case clause.
Sequence-pattern
fixtures cover trailing extractor arguments, nested extractors, wildcard and
backquoted variables, while preserving ordinary infix operators such as `*:`.
Quote syntax follows Dotty's parser-level tree (`Quote` containing
`SplicePattern` nodes); `QuotePattern` is a later typed-tree representation.

Fixtures under `fixtures/quotes/` use the normal expression mode and cover
quoted expression/type bodies, empty quoted blocks, nested quotes, braced
expression splices, and simple `$name` splices. The Rust and Scala renderers
compare quote/splice child trees and source spans; staging semantics are not
part of this parser oracle.

Fixtures under `fixtures/definitions/` cover the initial class-like
definition subset in block mode: class, trait, object, and enum nodes; primary
constructor clauses; simple parent applications, including parameterized enum
case parents and ordered parent lists; and braced or indented
template bodies. Type-parameter context-bound fixtures cover method, class,
case-class, given, and extension owners, explicit bounds, multiple braced
bounds, and `as` aliases. They also cover source annotations, hard and supported soft
modifiers, and qualified visibility. The normalized definition metadata
compares modifier order, visibility, visibility qualifiers, and annotation
trees; `ModuleDef.metadata` is included just like metadata on the other
definition nodes. The renderer exposes the source-level `Trait` and `Enum`
distinctions,
modern `given` aliases and structural templates, and `ExtMethods` parameter
clauses with method/export children. Parameter fixtures also compare inline
modifiers on method, constructor, given, and extension parameters, including
named `using` clauses, while preserving `inline` as an ordinary name before
`:`. Anonymous given names remain
empty rather than being replaced with synthetic names; anonymous `using` type
parameters use Dotty's deterministic `x$N` names. Given fixtures also cover
`GivenType`
condition chains, parenthesized anonymous context types, constructor parents,
and comma- or `with`-separated structural parent lists. Extension fixtures cover generic and
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
bounded forms), import/export clauses, and named `end` markers across nested
control-flow, member, and top-level boundaries. Dotty returns `EmptyTree`
for an empty compilation unit; the Scala renderer normalizes that one case to
the Rust parser's documented zero-width empty `PackageDef` root.
