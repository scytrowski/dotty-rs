# `dotty-parser` design for Scala 3.9.0

Status: initial parser infrastructure. This document describes the boundary
and the deliberately small first grammar increment; it is not a promise of
full Scala grammar coverage.

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
the parser does not allocate or reinterpret source identity.

The public compilation entry point is:

```rust
parse_compilation_unit(source, source_id, tokens, names) -> ParseResult
```

`ParseResult` contains the arena, a root `TreeId<Untyped>`, and accumulated
parser diagnostics. Syntax errors are represented both by diagnostics and,
where recovery needs a tree-shaped placeholder, by `UntypedNode::Error`.

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
and `using`. The lexer still emits these as identifiers; parser context gives
them grammar meaning.

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
implemented constructs such as `class`, `def`, and `if` produce an
`UnsupportedSyntax` diagnostic and a recoverable error tree instead of a
panic.

## AST root and current grammar

The current compilation-unit root is a synthetic `TreeKind::Block`. Earlier
expressions become its statements and the final expression becomes its `expr`
field. An empty unit receives a recoverable expression error. This convention
keeps one stable root while later grammar increments add definitions and
package-level forms.

The smoke grammar currently covers:

```text
identifier, backquoted identifier
integer, long, decimal, exponent, float, and double literals
string literals
true, false, null, this
(expr), (), and (a, b, ...)
simple selections such as `foo.bar`
```

The smoke grammar is plumbing coverage, not a substitute for the Scala
grammar. Operators, selections, applications, types, patterns, definitions,
control flow, templates, interpolation, XML, quotes, and macros remain
follow-up increments.

## Operator metadata

The parser exposes Scala's precedence buckets and associativity helpers before
full infix parsing exists. Assignment operators have precedence zero, letter
operators one, and the symbolic groups follow Scala 3.9's `| ^ & =/! </> : +/-
*/%` ordering. Operators ending in `:` are right-associative. Assignment
classification excludes comparison operators such as `==` and `<=`.

## Scala parser oracle

`tools/scala-parser-oracle` is pinned to Scala 3.9.0, JDK 25, and sbt 2.0.9.
It invokes the compiler parser and emits a deterministic JSON view containing
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
Rust UTF-8 byte offsets before comparing. The initial ASCII fixtures are tiny
on purpose and cover the complete smoke subset.

## Extension rule

Each grammar increment should add the smallest AST construction and focused
unit tests at the lowest affected layer, extend the oracle fixtures when the
construct is supported, and finish with its own commit. Keep raw token access,
source spans, diagnostics, recovery, context, and ownership in these stable
infrastructure layers so later grammar work remains ordinary recursive descent.
