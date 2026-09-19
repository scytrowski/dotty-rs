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
and `using`. It also interns the future feature-dependent names `into`,
`erased`, and `tracked`. The lexer still emits all of these as identifiers;
parser context gives them grammar meaning only in the appropriate production.
`ParserFeatures` currently exposes independent switches for capture checking,
erased definitions, `into`, and legacy `postfix_ops`, all disabled by default.
The switches are a boundary for future grammar work, not an implementation of
those features.

The parser forwards `ColonEol`, `Indented`, `Outdented`, and `ArrowIndented`
events through readable helpers. This keeps layout and colon reclassification
in the scanner while allowing grammar decisions to remain in the parser.

## Diagnostics and recovery

Parser diagnostics use the small categories `ExpectedToken`,
`UnexpectedToken`, `ExpectedExpression`, `ExpectedType`, `ExpectedPattern`,
and `UnsupportedSyntax`, with a source ID and source span.

Reusable recovery sets cover statements, arguments, type arguments, and case
clauses. Every recovery loop checks that the token source advances; a broken
external source cannot turn recovery into an infinite loop. Valid but not yet
implemented constructs such as `class`, `def`, `for`, `try`, and `match`
produce an `UnsupportedSyntax` diagnostic and a recoverable error tree instead
of a panic.

## AST root and current grammar

The current compilation-unit root is a synthetic `TreeKind::Block`. Earlier
expressions become its statements and the final expression becomes its `expr`
field. An empty valid unit has no statements and receives a synthetic
zero-width `Literal(Constant::Unit)` expression, not an error node. This
convention keeps one stable root while later grammar increments add
definitions and package-level forms.

The current expression, pattern, and literal implementation is split by
responsibility: `compilation_unit.rs` owns orchestration and statement
separators, `expr/` owns the incremental `Expr`/`Expr1` through operator- and
simple-expression pipeline, `patterns.rs` owns the source-level pattern
pipeline, `types.rs` owns the initial type-name subset used by type
applications, and `literals.rs` owns numeric and string decoding. These names
describe the current milestone; they do not claim complete Scala grammar
coverage.

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
prefix operators `-`, `+`, `~`, and `!` on the same physical line
negative numeric literals using Scala's parser-level literal shape
infix operators with Scala 3.9 precedence and associativity
feature-gated legacy postfix operators (disabled by default)
ordinary assignment with bare `=`
named arguments in the narrow bare-identifier form
the initial `if` and `while` expression forms
```

The implemented selections and applications are only the simple-expression
subset above. Full selection/application grammar, including advanced argument
forms such as `using` and colon arguments, remains future work. Likewise, the
type parser currently handles only the simple type names needed by these type
applications.

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
`RefinedType`, match/case grammar, definitions, remaining control flow
(`try`, `for`, `match`, `throw`, and `return`), templates, interpolation,
quotes, and macros remain follow-up increments.

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
`PostfixOp` only when `ParserFeatures::postfix_ops` is enabled. Assignment and
the initial `if`/`while` forms are implemented above this layer; ascription,
match clauses, colon arguments, and the remaining higher-level expression
grammar are not implemented yet.

## Scala parser oracle

`tools/scala-parser-oracle` is pinned to Scala 3.9.0, JDK 25, and sbt 2.0.9.
It invokes the compiler parser in expression mode by default, or the real
`Parser.pattern()` entry in pattern mode, and emits a deterministic JSON view containing
`kind`, `span`, `name`, `literal`, and `children`. It does not compare
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
precedence/associativity, assignment, named arguments, `if`/`while`, binders,
typed patterns, extractor applications, infix patterns, alternatives, and
named pattern arguments. Pattern fixtures live under
`tools/scala-parser-oracle/fixtures/patterns/`; `compare.sh` runs both modes.
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
