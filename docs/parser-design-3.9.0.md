# `dotty-parser` design for Scala 3.9.0

Status: incremental parser implementation. This document describes the
boundary and the deliberately small current grammar increment; it is not a
promise of full Scala grammar coverage.

The compatibility target is Scala 3.9.0 and TASTy format 28.9.0. Versioned
parser behavior must be compared with the pinned Scala release rather than
inferred from a newer compiler.

## Boundary and dependency direction

The source frontend is layered as:

```text
SourceText + dotty-core::TokenSource
                    |
                    v
              dotty-parser
                    |
                    v
             AstArena<Untyped>
```

`dotty-parser` depends on `dotty-core` only. It does not depend on
`dotty-lexer`, symbols, scopes, types, class loading, TASTy decoding, or
typer state. The concrete lexer is replaceable because the parser consumes
the shared `TokenSource` contract. Parser unit tests therefore use in-memory
token sources; the differential tool may combine the real lexer and parser as
a development-only integration.

## Parser state and ownership

`Parser<'src, 'names, S>` owns the cursor, source view, source ID, untyped AST
arena, parse context, diagnostics, and known parser names. It borrows the
session's `NameInterner`. `SourceId` is supplied by the compilation session;
the parser does not allocate or reinterpret source identity. `ParserFeatures`
is carried in the context as a separate policy for future dialect- or
feature-dependent grammar.

The public compilation entry point is:

```rust
parse_compilation_unit(source, source_id, tokens, names) -> ParseResult
```

`ParseResult` contains the arena, a root `TreeId<Untyped>`, and accumulated
parser diagnostics. Syntax errors are represented both by diagnostics and,
where recovery needs a tree-shaped placeholder, by `UntypedNode::Error`.

Parser tooling also exposes `parse_pattern_fragment(...)`. It parses one
standalone source pattern through the real pattern productions, requires the
fragment to end at EOF, and is not a separate pattern dialect.

## Token access and source text

Grammar code uses `Cursor` for `current`, `kind`, `at`, `advance`, lookahead,
scanner feedback, `accept`, and `expect`. `expect` records a typed diagnostic
instead of aborting the parse.

Token spelling is always recovered from `SourceText::slice(token.span)`.
The parser does not add owned strings to tokens or maintain a second lexical
cache. Names are interned only when a grammar production needs a term or type
name.

Source ranges are byte ranges in Rust. `Mark`, `span_from`, and `alloc_from`
centralize position construction. Synthetic `INDENT`/`OUTDENT` tokens are
zero-width, and EOF does not replace the end of the last real token.

`Cursor` exposes a `CursorCheckpoint` containing the explicit logical position
reported by `TokenSource` for progress guards. Recovery therefore does not
rely on token object identity or token equality and works with a source that
reuses one token allocation, including adjacent equal synthetic tokens. A
source that remains stuck is detected and terminates recovery.

## Context, soft keywords, and scanner feedback

`ParseContext` explicitly tracks Scala parser concepts from 3.9.0:

- `Location`: parentheses, arguments, colon arguments, patterns, guards,
  pattern arguments, blocks, and the default elsewhere location;
- `ParamOwner`: class, case class, method, type, higher-kinded parameter,
  given, and both extension positions;
- `ParseKind`: expression, type, or pattern.

Scoped helpers restore the previous context after nested parsing, including
recovery paths. `KnownNames` pre-interns the Scala 3.9 soft keywords `as`,
`derives`, `extension`, `infix`, `inline`, `opaque`, `open`, `transparent`,
`using`, `uses`, and `initially`. It also interns the future
feature-dependent names `into`, `erased`, `tracked`, and `update`. The lexer
still emits all of these as identifiers;
parser context gives them grammar meaning only in the appropriate production.
`ParserFeatures` currently exposes independent switches for capture checking,
erased definitions, `into`, legacy `postfix_ops`, and experimental single-case
`match case` syntax (`sub_cases`), all disabled by default.
The switches are a boundary for future grammar work, not an implementation of
those features.

The parser forwards `ColonEol`, `Indented`, `Outdented`, `ArrowIndented`, and
the template-specific `SelfArrow` event through readable helpers. A self arrow
suppresses a pending nested indentation region; layout classification remains
owned by the scanner while grammar decisions remain in the parser.

## Diagnostics and recovery

Parser diagnostics use the small categories `ExpectedToken`,
`UnexpectedToken`, `ExpectedExpression`, `ExpectedType`, `ExpectedPattern`,
`UnsupportedSyntax`, and `UnboundPlaceholderParameter`, with a source ID and
source span. `UnboundPlaceholderParameter` is reported when an expression
placeholder reaches the end of a compilation unit or expression block without
being captured by a complete expression.

Reusable recovery sets cover statements, arguments, type arguments, case
clauses, and for enumerators. Every recovery loop checks that the token source
advances; a broken external source cannot turn recovery into an infinite loop.
Valid but not yet implemented constructs such as `do ... while`, `quote`, and
XML syntax produce an `UnsupportedSyntax` diagnostic and a recoverable error
tree instead of a panic.

## AST root and current grammar

The public compilation-unit parser returns a real `TreeKind::PackageDef` root.
An explicitly declared package is returned directly; package-less source is
wrapped in an empty-package definition whose name is the zero-width
`<empty>` identifier. An empty valid source file therefore has an empty
`PackageDef` body and no synthetic trailing `Unit` expression. The lower-level
`Parser::compilation_unit()` helper remains an internal synthetic-block
sequence parser for expression/block and definition tests; it is not the
source-file entry point.

The supported source-unit grammar is intentionally small:

```text
CompilationUnit -> { package QualId [body] } TopStatSeq
TopStat         -> package QualId body
                 | import ImportExpr {',' ImportExpr}
                 | export ImportExpr {',' ImportExpr}
                 | supported top-level definition

Supported contextual definitions include modern `given` aliases and
structural instances, plus contextual `extension` method groups. Ordinary
expression uses of `extension` remain expressions unless the bounded lookahead
sees `[` or `(`.
```

Package bodies may be braced, scanner-provided indented regions, or the
unbraced outermost package form. Import and export clauses preserve source
order and expand comma-separated expressions into separate `Import` or
`Export` statement trees. Their selectors retain aliases, wildcard selectors,
and the narrow `given T` type-bound form. Arbitrary top-level expressions and
package objects remain unsupported; imports and exports are also retained as
statements inside supported blocks and templates.

Value definitions are preserved as statements in this root: simple identifiers use
`ValDef` (with `Modifier::Var` for `var`), while non-simple left-hand sides use
the source-level `PatDef` form. A typed `ValDef` may omit its RHS as a
declaration, as may a typed `PatDef` containing only simple identifiers. A
`PatDef` with a complex pattern still requires `=` and receives an error
placeholder when the source is malformed;
the parser uses a zero-width synthetic `TypeTree` when no explicit type is
written.

The current expression, pattern, and literal implementation is split by
responsibility: `compilation_unit.rs` owns orchestration and statement
separators, `expr/` owns the incremental `Expr`/`Expr1` through operator- and
simple-expression pipeline, `patterns.rs` owns the source-level pattern
pipeline, `type_definitions.rs` owns source-level type definitions, `types.rs`
owns the initial simple/applied/infix type subset used by annotations, bounds,
type applications, and definitions, and
`literals.rs` owns numeric and string decoding. These names describe the
current milestone; they do not claim complete Scala grammar coverage.

The currently implemented expression grammar covers:

```text
identifier, backquoted identifier
integer, long, decimal, exponent, float, and double literals
string literals
true, false, null, this
(expr), (), and (a, b, ...)
simple selections such as foo.bar and foo.`bar`
simple applications such as foo(42) and foo(1, 2)
super, qualified super, and simple mixin-qualified super
new with simple or qualified type names and constructor applications
simple type applications such as foo[A] and foo[A, B]
match types with braced type cases, including wildcard cases
brace blocks with separator-delimited expressions
repeated `.`, `[...]`, and `(...)` suffix chaining
expression type ascriptions with the supported simple/infix type parser
parenthesized `using` argument lists, including nested application clauses
indented colon arguments, including block, lambda, and case bodies
prefix operators `-`, `+`, `~`, and `!` on the same physical line
negative numeric literals using Scala's parser-level literal shape
infix operators with Scala 3.9 precedence and associativity
feature-gated legacy postfix operators (disabled by default)
ordinary assignment with bare `=`
named arguments in ordinary and `using` lists in the narrow bare-identifier form
the initial `if` and `while` expression forms
`throw`, bare/value `return`, and source-level `try`/`catch`/`finally`
braced and indented `match` expressions with `case` patterns, guards, and bodies
single-case `match` expressions in the expression-only form
for-comprehensions with generators, case generators, aliases, guards, and
`yield`/`do` bodies
simple `val`/`var` definitions with inferred or explicit types, declarations
without an RHS, and full-expression RHS values
pattern definitions with tuple, extractor, binder, and infix-pattern LHSs
method definitions with a leading type-parameter clause, ordered term
parameter clauses, supported type-expression return types, declarations, and
expression RHSs
parameter nodes are represented as `ValDef`; ordinary and named `using`
clauses, default parameter expressions, and indented method bodies are
supported. Interleaved type/term parameter clauses are explicitly deferred
because the current `DefDef` model keeps the leading type clause separate.
class, trait, object, case class, case object, and enum definitions with type
parameters, primary constructor clauses, simple `extends` parent applications
including parameterized enum-case parents, comma/`with` parent lists, `derives`
and capture-checking `uses` clauses, basic
`cap`/qualified capture references, self values (`self =>`, typed self types),
and braced or indented template bodies. The class/trait/case/enum distinctions
are preserved in definition metadata; case objects remain `ModuleDef` trees.
Singleton enum cases are represented as `ModuleDef` trees with the distinct
parser-generated `Modifier::EnumCase`; comma-separated singleton cases remain
one `PatDef` with the source identifiers in order. The synthetic primary
constructor and template body remain parser-level structure. Parameterized enum
cases are represented as `TypeDef(Template(...))` with `Modifier::EnumCase`;
their type parameters and constructor clauses reuse the `ParamOwner::CaseClass`
policy, including accessor/private-local metadata. Type-only cases such as
`case Empty[T]` have no value-parameter clauses.
Parameterized case templates populate `Template.parents` through the same
parent/constr-app parser used by classes; parent order and constructor
applications are preserved. Enum-case bodies and enum-case `derives`/`uses`
remain deferred. Direct enum cases accept source annotations and
`private`/`protected` visibility, including qualified visibility; the metadata
is attached to the existing `ModuleDef`, `PatDef`, or `TypeDef` shape without
introducing a new enum-case node. Annotations or modifiers after the case name,
and semantic visibility checks, remain deferred.
Constructor parameters preserve Dotty's parser-level role combinations:
explicit `val`/`var` parameters are accessors, while plain class and later
case-class parameters retain the `ParamAccessor`/`PrivateLocal` metadata needed
by later phases. This metadata describes constructor roles, not source-level
visibility.
simple type aliases and abstract type declarations with lower and/or upper
bounds
parenthesized type grouping and tuple types, represented by the shared
`Parens` and `Tuple` source nodes; tuple elements use the supported full type
expression subset and compose with applied types
ordinary function types (`A => B`, parenthesized and tuple parameter lists,
and `() => R`), represented by the shared `Function` node. Ordinary
parenthesized function types also support named typed parameters such as
`(x: A, y: B) => C`; each parameter is a direct `ValDef` child with a term
name, a full currently-supported `type_expr()` type, no RHS, and default
metadata. Backquoted parameter names and nested ordinary/context function
types are preserved. With `ParserFeatures::erased_definitions` enabled,
ordinary named function types also accept the Scala 3.9
`(erased x: A) => B` form. The parameter remains a plain `ValDef`; erasure is
recorded positionally in `FunctionWithMods.erased_params`, while the outer
function has no `Erased` modifier. Function arrows associate to the right and bind below
the supported union/intersection type operators, so function types compose
with applied, tuple, union, and intersection types. Context-function types
(`?=>`) also support named typed parameters such as
`(ctx: Context, req: Request) ?=> Response`; they use the same direct `ValDef`
parameter nodes as ordinary function types. The parameters retain default
metadata without `Given`; `Given` belongs to the surrounding
`FunctionWithMods`, whose `erased_params` contains one positional boolean per
parameter (`false` for ordinary parameters and `true` for erased ones).
With `ParserFeatures::erased_definitions` enabled, context-function types also
accept named erased parameters such as `(erased x: A) ?=> B` and mixed clauses
such as `(x: A, erased y: B) ?=> C`. They retain the plain `ValDef` parameter
shape; `Given` remains on the surrounding `FunctionWithMods`, while
`erased_params` records erasure positionally alongside ordinary parameters.
With `ParserFeatures::erased_definitions` enabled, ordinary function types also
support the narrow Scala 3.9 leading unnamed form `(erased A, B) => C`.
Unnamed parameters remain type trees rather than `ValDef` nodes, and
`FunctionWithMods.erased_params` records `[true, false]` in source order.
This parser deliberately recognizes only one leading unnamed `erased` marker;
non-leading and repeated unnamed erased markers remain outside this increment.
Context-function types support the same narrow leading unnamed form, such as
`(erased A, B) ?=> C`. These parameters remain type trees, `Given` belongs to
the surrounding `FunctionWithMods`, and `erased_params` records
`[true, false]`; the form composes with nested ordinary and context arrows.
Named parameters can use the full currently-supported `type_expr()` subset and
compose recursively with ordinary or context arrows. Named tuple types such as
`(name: String, age: Int)` are represented by the shared `Tuple` node with
`NamedArg` children; each child preserves its term name and a full
`type_expr()` element type. The lookahead for a following `=>` or `?=>` still
selects the named function-parameter path instead. Pure arrows remain deferred.
Supported
context-function types use
`FunctionWithMods` with exactly one `Given` modifier and one positional boolean
in `erased_params` per parameter (`false` for ordinary parameters and `true`
for erased ones); `?=>` is right-associative and composes
recursively with ordinary `=>`.
Ordinary parenthesized function types also support unnamed by-name parameters
in the leading-arrow `FunArgType` form, such as `(=> A) => B` and
`(A, => B) => C`. Each explicit `=> Type` parameter is represented by the
shared `ByNameTypeTree`, and strict/by-name parameters retain source order.
The wrapped result uses the full current `type_expr()` parser, so applied,
union/intersection, tuple, and nested ordinary/context function types are
preserved. By-name parameters are supported in parenthesized ordinary and
context-function types. The latter use `FunctionWithMods` with `Given` on the
outer function and retain an all-`false` `erased_params` vector; the
`ByNameTypeTree` itself carries no context metadata. Named by-name parameters,
erased/by-name combinations, and pure arrows remain deferred.
Polymorphic function types such as `[A] => A => A` reuse the shared
`parse_type_param_clause(ParamOwner::Type)` grammar and are represented by
`PolyFunction` with `TypeDef` children followed by a function-type body.
Bounds and the currently-supported ordinary, context, by-name, and feature-gated
erased function bodies compose through the existing `type_expr()` entry point.
The ordinary `=>` is required for this path and the type-lambda arrow `=>>` is
kept distinct from it. A leading type-parameter clause followed by `=>>`, such
as `[A] =>> List[A]`, is represented by the shared `LambdaTypeTree` with
`TypeDef` parameters and a body parsed through the full `type_expr()` entry
point. Nested higher-kinded clauses such as `[G[_]] =>> G[Int]` use a nested
`LambdaTypeTree` in the `G` parameter's `rhs`; the wildcard remains the shared
synthetic type-parameter `TypeDef`. Ordinary and context-function bodies
compose recursively. Type-lambda parameters preserve lower/upper bounds;
explicit `+`/`-` variance is rejected for `ParamOwner::Type`, matching Dotty,
and their bodies compose with applied, tuple,
union/intersection, ordinary function, context-function, and nested
polymorphic-function types. Ordinary polymorphic function types preserve
parser-level `ContextBounds` wrappers, including each bound's parameter name
and optional `as` alias. A context bound in a type-lambda parameter is
consumed, reported as unsupported syntax, and stripped from the source-level
tree only after the `=>>` arrow identifies the type-lambda path, matching the
current Dotty parser shape. Bounds are parsed before the context-bound suffix,
including repeated and braced context-bound forms. Empty or malformed
type-lambda clauses report diagnostics without consuming the
following type-definition boundary, including incomplete commas, nested
parameter clauses, and missing context-bound types.
parameterized type aliases and abstract declarations using the shared
`LambdaTypeTree` and higher-kinded type-parameter machinery
explicit function literals with empty, named, wildcard, and typed parameters
context-function literals using `?=>`, represented with `Given` parameter
metadata
```

The implemented selections and applications are only the simple-expression
subset above. Full selection/application grammar, including advanced argument
forms, named/using argument validation, and the remaining colon-argument
forms, remains future work. Likewise, the type parser currently handles
simple, recursively applied, projected, annotated, refined, parenthesized, and tuple type names needed by
these ascriptions, type applications, and type-definition bounds, plus
Scala 3.9 `InfixType` expressions with generic symbolic and identifier
operators, match types with ordered `CaseDef` children, ordinary/context
function arrows, and wildcard type arguments with optional lower and upper
bounds. Match-type patterns use `InfixType`, while results use the full
`type_expr()` entry. Match types support braced case regions, applied and
tuple case patterns, infix case patterns, wildcard cases, and bounded aliases
with an upper bound. For a bounded alias, the upper bound is stored in the
shared `MatchTypeTree.bound`; a lower bound combined with a match-type alias is
diagnosed according to Scala's parser rule. Layout-only type case regions and
the remaining full type grammar are still deferred.
Wildcard syntax is enabled by the type-argument context (`List[?]` and
`List[? >: String <: Number]`); a top-level `?` remains an invalid type and
produces a parser diagnostic. `type_expr()` parses arrows outside the
infix-type layer using the shared Scala precedence buckets; `&` still binds
tighter than `|`, and operators ending in `:` associate to the right. All
infix type operators produce source-level `InfixOp` trees with type-namespace
operator names; ordinary arrows produce `Function` trees and context arrows
produce `FunctionWithMods`.
Parenthesized
grouping and tuples preserve the shared source-level `Parens` and `Tuple`
nodes, including when nested or used as applied type arguments. Type
positions use `AppliedTypeTree`; the term-level `foo[A]` form remains
`TypeApply`. An empty `()` is not a standalone tuple type: it is accepted only
as the parameter list of a zero-argument function type. Named tuple types use
`NamedArg` children inside the existing `Tuple` node, while a following
ordinary or context function arrow continues to select named `ValDef`
parameters instead.
A path singleton type such as `x.type` or `foo.bar.type` is represented by the
existing `SingletonTypeTree`; its reference is parsed in the term namespace,
while an ordinary qualified type such as `foo.Bar` remains in the type
namespace. Singleton types compose with the supported applied, union, and
function type forms. Literal singleton types for strings, characters, numbers,
booleans, and `null` are represented by `SingletonTypeTree` around the existing
`Literal` tree. Type projections such as `T#Member`, `F[A]#Result`, and
repeated `#` suffixes use the existing `Select` node with type-namespace
names. `this.type` and `super.x.type` remain deferred.
An empty
context-function parameter list (`() ?=> R`) is invalid and produces a
focused parser diagnostic. The `derives` clause
intentionally keeps its narrower
qualified-identifier grammar, so `derives Eq[A]`, `derives Eq | Show`, and
parenthesized derives types remain rejected until that grammar is expanded.

Method definitions are statement-level `DefDef` trees. Their RHS is parsed as
a complete expression, so local `val`/`var` and `def` statements can be kept
inside a brace or indented `Block`; a typed declaration without `=` has no
RHS. The shared definition-prefix layer now preserves source annotations,
hard modifiers, the supported soft modifiers, and private/protected visibility
including qualifiers on the definition nodes. It deliberately does not yet
handle constructors, legacy `(implicit ...)` clauses, anonymous `(using T)`
clauses, context-type shorthand, or the remaining definition forms.
Unsupported parameter forms produce an explicit unsupported-syntax diagnostic
and synchronize at the closing parenthesis.

Type definitions are statement-level `TypeDef` trees. The implemented subset
covers simple aliases (`type A = B`), abstract declarations, lower and upper
bounds, backquoted type names, and one flat higher-kinded parameter clause
(`type F[A] = A`). Parameterized declarations preserve their parameters in a
shared `LambdaTypeTree`; an abstract declaration with no explicit bounds uses
the existing zero-width `TypeBoundsTree` representation. The supported
recursive applied-type and type-projection subset is represented by
`AppliedTypeTree` and type-namespace `Select` nodes; wildcard arguments use
`TypeBoundsTree` with optional `low` and `high` children. Annotated types use
the shared `Annotated` node and reuse the definition annotation grammar; the
annotation binds to the immediately preceding simple/parenthesized type before
the supported union/intersection layers. Path
singleton aliases use `SingletonTypeTree` and preserve the term-valued path
that precedes `.type`; literal singleton aliases use the same node around a
decoded `Literal`. Unions and intersections use `InfixOp` with `&` binding
tighter than `|`. Refined types use the shared `RefinedTypeTree`; the initial
subset supports abstract, aliased, and upper-bounded `TypeDef` members in
source order, including the parentless `{ type X }` form. Parentless
refinements use a zero-width `TypeTree` only as the non-optional AST parent
placeholder and do not create a semantic refinement scope. Declaration-only
`val`, `var`, and `def` members reuse the shared `ValDef` and `DefDef` nodes;
their right-hand sides and parameter defaults are diagnosed, as are
class-like members and definition modifiers. Refined type members,
capture-checking refinements, opaque, and other
full type forms remain deferred.

### Class-like definitions and templates

The initial class-like definition layer parses `class`, `trait`, and `object`
statements into the existing untyped AST. Classes and traits use
`TypeDef(Template(...))`; objects use `ModuleDef(Template(...))`. A trait is
marked with the parser-level `Modifier::Trait`, so the source distinction is
not inferred from constructor shape or body contents.

The supported subset preserves class type parameters, primary constructor
term-parameter clauses, simple `extends` parent types, constructor arguments,
and comma/`with` parent lists. Parent constructor syntax remains source-level
`Apply(Select(New(parent), <init>), args)`; resolving the parent or its
constructor is a later semantic phase. When the shared AST requires a
constructor for an otherwise empty class-like template, the parser allocates
a synthetic `<init>` `DefDef` and preserves the source parameter structure
there.

Braced and scanner-provided indented template bodies retain every member in
source order, including nested definitions and expressions. A self value is
stored in the existing `Template.self_val`; its source-level `ValDef` carries
`PrivateLocal`, matching Dotty's parser tree. The current type subset accepts
simple qualified and compound `InfixType` self types. This is deliberately
different from an expression block, whose last expression is its result. Layout
classification remains owned by the scanner; the parser only feeds back the
`ColonEol`, `Indented`, `Outdented`, and `SelfArrow` events needed to close a
template region. The parser preserves `derives` and ordered `uses` metadata in
`UntypedTemplateMetadata`; it does not perform derivation or capture checking.
Sequence capture references, `.only[...]`/`.rd` forms, auxiliary constructors,
and semantic template processing remain future work. Direct enum-case
annotations, access modifiers, and qualified visibility are preserved as
definition metadata; constructor-level annotations/modifiers after the case
name, enum-case bodies, and enum-case `derives`/`uses` remain deferred.

### Contextual definitions

The implemented `given` subset follows the Scala 3.9 source parser's node-kind
matrix:

```text
given T = rhs                  -> ValDef
given name: T = rhs            -> ValDef
given [A] => T = rhs            -> DefDef
given (using ctx: Ctx) => T = rhs -> DefDef
given T: body                  -> ModuleDef(Template(...))
given name: T: body            -> ModuleDef(Template(...))
given [A] => T: body            -> TypeDef(Template(...))
```

Anonymous givens use the canonical empty interned term name; the parser does
not synthesize a `$given_N` spelling. Unparameterized non-inline aliases carry
Dotty's parser-added `Given`, `Final`, and `Lazy` metadata where applicable.
Parameterized and `inline` aliases are represented as `DefDef` trees, and
structural bodies reuse the ordinary `Template` machinery. Given conditions
currently support type parameters and named `using` clauses; anonymous
context-type parameters and the full `GivenType` grammar remain deferred.

An extension is represented by the existing `UntypedNode::ExtensionMethods`
wrapper. Its parameter clauses remain in source order: leading type parameters,
leading `using` clauses, exactly one ordinary receiver, and trailing `using`
clauses. The wrapper's methods preserve `DefDef` and `Export` members in source
order. Extension receiver parameters are ordinary `ValDef` parameters, not
constructor accessors. Braced and scanner-provided indented bodies are
supported; a colon directly after the extension header is rejected according to
the Scala 3.9 grammar. Ordinary clauses after the receiver and non-method body
members are diagnosed. This is source structure only: no given synthesis,
extension lowering, symbol creation, or semantic resolution is performed.

### Definition prefixes and source metadata

Definitions share a parser-owned prefix step before dispatching to `val`,
`var`, `def`, `type`, `class`, `trait`, `object`, `case class`, `case object`,
or `enum`. Direct enum cases have an additional prefix boundary: annotations
and `private`/`protected` visibility are parsed before `case`, including
qualified visibility such as `private[pkg]`. Invalid enum-case modifiers are
diagnosed and do not change the existing node-kind matrix: singleton cases
remain `ModuleDef`, comma-separated singleton cases remain one `PatDef`, and
parameterized or parented cases remain `TypeDef(Template(...))` with
`Modifier::EnumCase`.
The current subset
preserves annotations, source order for hard modifiers (`abstract`, `final`,
`sealed`, `implicit`, `lazy`, and `override`), supported soft modifiers
(`inline`, `transparent`, `open`, and `infix`), and `private`/`protected`
visibility with an optional qualifier. The prefix is recognized only in a
definition position; soft words remain ordinary identifiers everywhere else.
Duplicate modifiers and malformed annotations produce diagnostics while
retaining a recoverable source tree.

The resulting `Modifiers` value is parser metadata, not a resolved symbol-flag
set. It is attached to the existing definition nodes, including
`ModuleDef.metadata`, so later phases can distinguish source syntax from
semantic resolution. Enum definitions use `TypeDef(Template(...))` with the
parser-level `Modifier::Enum`; this preserves enum identity without adding a
new shared tree kind. Annotation trees use the source-level
`Apply(Select(New(type), <init>), args)` shape. Parameter and type-parameter
annotations, constructor-level enum-case annotations/modifiers, enum-case
bodies, and feature-dependent
`opaque`/`erased`/`tracked`/`into`/`update` modifiers remain deferred.

The current source-level pattern grammar is layered as:

```text
Pattern -> Pattern1 -> Pattern2 -> InfixPattern -> SimplePattern
```

It covers identifiers and `_`, literals including negative numbers,
parentheses and tuples, syntactic selections including `this.member` and
`super.member`, extractor-shaped `Apply`/`TypeApply` trees, `@` binders,
simple typed patterns, precedence-aware infix patterns, `|` alternatives, and
named extractor arguments. Extractor-looking source patterns intentionally
remain `Apply`/`TypeApply`; semantic `UnApply` lowering belongs to later
phases. Sequence patterns, `given`, quoted and XML patterns, remaining
refined-type forms, remaining definition forms and full template semantics, legacy
given syntax, remaining control flow (`do`/`while`),
interpolation, quotes, and macros remain follow-up increments.

The initial match layer parses braced and indented `case` regions, including
patterns, optional guards, and expression bodies. Case bodies are represented
as source-level `Block` nodes, and extractor-looking source patterns remain
`Apply`/`TypeApply` until later semantic lowering. Full case-clause features
and pattern semantics remain future work.

The complete-expression boundary also recognizes the initial explicit function
literal subset:

```text
Expr -> function literal | Expr1
function literal -> FunParams `=>` Expr
                  | FunParams `?=>` Expr
```

Single parameters may be written without parentheses; parenthesized parameter
lists may be empty, typed, or contain wildcard parameters. A missing parameter
type is represented by a zero-width synthetic `TypeTree`, and wildcard
parameters receive parser-local generated names because the shared AST models
lambda parameters as `ValDef`. Context-function parameters retain
`Modifier::Given` metadata. The body is a complete expression; an indented
body or the remaining region of an enclosing block is represented using the
existing block-body convention. Polyfunctions use the shared `TypeDef` and
`TypeBoundsTree` representation for their type-parameter clause. Expression
placeholders create synthetic `ValDef` parameters and lower to ordinary
`Function` nodes when the enclosing expression is complete; Dotty's internal
`WildcardFunction` node is not exposed by the Rust AST. Erased parameters in
function literals and migration-only forms remain future work.

The initial polymorphic and placeholder-function subset covers:

```text
[A] => (x: A) => x
[A >: Lower <: Upper] => (x: A) => x
_ + 1
foo(_, 1)
_.name
```

Type-parameter bounds currently accept simple, qualified, recursively applied,
annotated, and refined type names, plus the supported generic infix-type
forms. Remaining refined-type forms, opaque, and other full type forms remain
deferred and produce a parser diagnostic in this milestone. Upper-bounded
match-type aliases are supported when their right-hand side is a match type;
lower bounds cannot be combined with that alias form.

Placeholder parameters are scoped to the complete expression that contains
them. A nested expression such as `foo(bar(_))` therefore creates the
placeholder function inside `bar(_)`, rather than wrapping the outer call.
Generated names and the Scala compiler's `WildcardFunction` kind are
normalized by the differential oracle.

An `if` without an `else` uses a zero-width synthetic
`Literal(Constant::Unit)` in the shared `If<P>::else_branch` slot. This is the
smallest representation compatible with the current AST contract for Dotty's
absent `EmptyTree`; the oracle omits that synthetic child when normalizing the
tree.

## Operator metadata

The operator-expression pipeline is layered as postfix/operator expression,
infix expression, prefix expression, and simple expression. Prefix support is
limited to `-`, `+`, `~`, and `!`; infix reduction uses Scala 3.9 precedence,
left/right associativity, and mixed-associativity diagnostics. Operators
ending in `:` are right-associative. Legacy postfix syntax is represented by
`PostfixOp` only when `ParserFeatures::postfix_ops` is enabled. Assignment,
the initial `if`/`while`, `throw`, `return`, `try`/`catch`/`finally`, match
clauses, for-comprehensions, ascriptions, and the initial using/colon argument
forms are implemented above this layer. Full types, argument validation,
remaining colon forms, and the rest of the higher-level expression grammar
remain future work.

## For-comprehensions

The parser implements the Scala 3.9 source-level for-comprehension subset:

```text
ForExpr -> Enumerators ('yield' | 'do') Expr
Enumerators -> Generator | Guard | Pattern1 '=' Expr
Generator -> ['case'] Pattern1 '<-' Expr
```

Generators deliberately use `Pattern1`, not the full alternative-pattern
production. Ordinary generators use `GenCheckMode::Check`; `case` generators
use `GenCheckMode::FilterAlways`. Better Fors is active for the pinned Scala
3.9 target, so leading aliases are accepted before the first generator, while
the parser still requires a generator eventually. Aliases are represented as
`GenAlias`, never as ordinary assignment trees, and guards remain ordinary
expression trees in their exact enumerator order.

Parenthesized, braced, and indentation-based enumerator regions are supported,
including the wrapped legacy form whose body has no explicit `do`. The parser
emits `ForYield` or `ForDo` directly and does not desugar comprehensions into
`map`, `flatMap`, or `withFilter`; that belongs to a later lowering phase.

## Scala parser oracle

`tools/scala-parser-oracle` is pinned to Scala 3.9.0, JDK 25, and sbt 2.0.9.
It invokes the compiler parser in expression mode by default, the real
`Parser.pattern()` entry in pattern mode, or the real `Parser.compilationUnit()`
entry in compilation mode. Historical expression-mode block fixtures remain
available in `block` mode. Every mode emits a deterministic JSON view containing
`kind`, `span`, `name`, `literal`, `operator`, `apply_kind`, parameter-clause
boundaries, and `children`. It does not compare
compiler `Tree.show` output. Nodes without a source span are omitted from the
normalized child list; this removes compiler-only synthetic qualifiers such as
the implicit qualifier of `this`. The empty compiler tuple is normalized to
the current Rust unit-literal shape.

`tools/parser-smoke-dump` is a separate development package that combines
`dotty-lexer` with `dotty-parser` and emits the same normalized view. It does
not alter the parser's production dependency direction. Run the focused
differential check with:

```text
tools/scala-parser-oracle/compare.sh
```

Scala source spans are UTF-16 offsets; the comparison script converts them to
Rust UTF-8 byte offsets before comparing. The ASCII fixtures are tiny on
purpose and cover the implemented simple-, operator-, initial `Expr1`, and
source-pattern subsets, including `super`, `new`, type applications, suffix
chains, brace blocks, prefix operators, negative literals, infix
precedence/associativity, assignment, named arguments, `if`/`while`,
for-comprehensions, match and case clauses, value, pattern, and method
definitions,
binders,
type aliases, abstract type bounds, typed patterns, extractor applications,
infix patterns, alternatives, and
named pattern arguments. Pattern fixtures live under
`tools/scala-parser-oracle/fixtures/patterns/`; statement-bearing definition
fixtures live under `tools/scala-parser-oracle/fixtures/definitions/` and are
run in block mode; source-unit fixtures live under
`tools/scala-parser-oracle/fixtures/compilation/` and are run in compilation
mode. `compare.sh` runs all four modes. The empty compilation unit is the one
intentional normalization boundary: Dotty exposes `EmptyTree`, while the Rust
source parser always exposes its documented empty `PackageDef` root.
The same command is available
as the manually dispatched `Scala 3.9 parser oracle` workflow in
`.github/workflows/parser-oracle.yml`. A normalized Scala/Rust mismatch fails
that workflow; it is intentionally not part of the automatic push/PR checks
while the oracle remains an opt-in, comparatively expensive parser gate.

## Extension rule

Each grammar increment should add the smallest AST construction and focused
unit tests at the lowest affected layer, extend the oracle fixtures when the
construct is supported, and finish with its own commit. Keep raw token access,
source spans, diagnostics, recovery, context, and ownership in these stable
infrastructure layers so later grammar work remains ordinary recursive descent.
