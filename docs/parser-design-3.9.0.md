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
Valid but not yet implemented constructs such as `for`, `try`, and `match`
produce an `UnsupportedSyntax` diagnostic and a recoverable error tree instead
of a panic.

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
owns the initial type-name subset used by type applications and bounds, and
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
brace blocks with separator-delimited expressions
repeated `.`, `[...]`, and `(...)` suffix chaining
expression type ascriptions with the current narrow simple-type parser
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
parameter clauses, simple return types, declarations, and expression RHSs
parameter nodes are represented as `ValDef`; ordinary and named `using`
clauses, default parameter expressions, and indented method bodies are
supported. Interleaved type/term parameter clauses are explicitly deferred
because the current `DefDef` model keeps the leading type clause separate.
class, trait, object, case class, case object, and enum definitions with type
parameters, primary constructor clauses, simple `extends` parent applications,
comma/`with` parent lists, `derives` and capture-checking `uses` clauses, basic
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
Constructor parameters preserve Dotty's parser-level role combinations:
explicit `val`/`var` parameters are accessors, while plain class and later
case-class parameters retain the `ParamAccessor`/`PrivateLocal` metadata needed
by later phases. This metadata describes constructor roles, not source-level
visibility.
simple type aliases and abstract type declarations with lower and/or upper
bounds
parameterized type aliases and abstract declarations using the shared
`LambdaTypeTree` and higher-kinded type-parameter machinery
explicit function literals with empty, named, wildcard, and typed parameters
context-function literals using `?=>`, represented with `Given` parameter
metadata
```

The implemented selections and applications are only the simple-expression
subset above. Full selection/application grammar, including advanced argument
forms, named/using argument validation, and the remaining colon-argument
forms, remains future work. Likewise, the type parser currently handles only
the simple type names needed by these ascriptions, type applications, and
type-definition bounds.

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
the existing zero-width `TypeBoundsTree` representation. Applied, refined,
opaque, match, and other full type forms remain deferred.

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
simple qualified self types; compound `InfixType` forms are diagnosed and
recovered at the self arrow until the fuller type grammar lands. This is
deliberately different from an expression block, whose last expression is its result. Layout
classification remains owned by the scanner; the parser only feeds back the
`ColonEol`, `Indented`, `Outdented`, and `SelfArrow` events needed to close a
template region. The parser preserves `derives` and ordered `uses` metadata in
`UntypedTemplateMetadata`; it does not perform derivation or capture checking.
Sequence capture references, `.only[...]`/`.rd` forms, enum-case parent clauses,
enum-case prefixes/modifiers, auxiliary constructors, and semantic template
processing remain future work.

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
or `enum`.
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
parameter annotations, extended/prefixed enum cases, and feature-dependent
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
phases. Sequence patterns, `given`, quoted and XML patterns, full
`RefinedType`, remaining definition forms and full template semantics, legacy
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
`WildcardFunction` node is not exposed by the Rust AST. Erased parameters and
migration-only forms remain future work.

The initial polymorphic and placeholder-function subset covers:

```text
[A] => (x: A) => x
[A >: Lower <: Upper] => (x: A) => x
_ + 1
foo(_, 1)
_.name
```

Type-parameter bounds currently accept only simple or qualified type names.
Applied, infix, refined, and other full type forms (for example `List[Int]`
or `Foo & Bar`) remain deferred and produce a parser diagnostic in this
milestone.

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
