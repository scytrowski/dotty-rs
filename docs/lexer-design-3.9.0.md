# Scala 3.9.0 Lexer and Scanner Design

Status: working design document for the Rust frontend.

This document records the architecture, compatibility decisions, incremental
implementation plan, and testing strategy for the Scala 3.9.0 source lexer.
It is intended to be updated as implementation and differential testing settle
details that are not obvious from the language specification. Parser and AST
work are future consumers of the lexer contract and are not current scope.

The compatibility target is the Scala 3.9.0 compiler at the `3.9.0` tag. When
the language specification and the compiler disagree, observable behavior of
that compiler is authoritative.

## 1. Scope

The current lexer work covers the path from UTF-8 source text to a
parser-facing token stream:

```text
SourceText
    ↓
RawLexer
    ↓ RawToken + Trivia
ContextualScanner
    ↓ Token
future Parser
```

The first milestone is deliberately smaller:

```text
source characters → lossless raw lexical stream
```

The first milestone includes whitespace, physical line breaks, comments,
identifiers, backquoted identifiers, operators, hard keywords, punctuation,
and EOF. It does not include indentation, semicolon inference, colon
classification, or parser-facing synthetic tokens.

The current implementation does not include:

- parser implementation or grammar;
- AST definitions or construction;
- name resolution or typing;

The lexer also does not own:

- operator precedence or associativity;
- numeric overflow checking;
- macro execution or constant folding;
- TASTy generation.

Implementation status on the current lexer branch:

- `RawLexer` emits a lossless stream for trivia, identifiers, operators,
  keywords, punctuation, numeric and character literals, ordinary and
  multiline strings, and basic string interpolation;
- `ContextualScanner` maps raw categories to the shared `dotty-token` kinds;
- the scanner currently infers `NEWLINE`/`NEWLINES` separators and maintains
  prefix-based indentation regions for non-colon triggers such as `then`,
  `else`, `match`, and `try`;
- parser-observed colon events now reclassify `COLONop`/`COLONfollow` as
  `COLONeol` and can request `INDENT`/`OUTDENT` insertion through the shared
  scanner event contract;
- leading-infix continuation is recognized for line-start operators, while a
  symbolic operator after a blank line starts a new logical statement;
- `CASECLASS`/`CASEOBJECT` fusion and basic line-start end-marker recognition
  are now part of scanner post-processing;
- dedented closing delimiters close implicit regions after `)`, `]`, or `}`;
- incomparable space/tab indentation prefixes produce recoverable diagnostics;
- quote markers and legacy quoted identifiers are emitted as `Quote` and
  `QuoteId`; splice syntax remains the `$` plus `{` token sequence;
- an XML start marker is emitted for `<` immediately followed by an XML name;
  XML tag operators retain Scala's greedy operator boundaries and the scanner
  returns to normal layout processing after a closed root literal;
- richer infix lookahead and full XML body handling remain intentionally
  staged.

XML, migration syntax, deprecated syntax, experimental syntax, parser, and AST
are staged after the core lexer. Their eventual addition must not require
changing the RawLexer/ContextualScanner boundary.

## 2. Compatibility sources

The sources are consulted in this order:

1. Observable behavior of Scala 3.9.0 compiler sources and tests.
2. The Scala 3 language specification and reference documentation.
3. The pinned implementation details described below.
4. Assumptions based on other languages or lexers.

Primary references:

- [Scala 3 syntax summary](https://docs.scala-lang.org/scala3/reference/syntax.html)
- [Optional braces and significant indentation](https://docs.scala-lang.org/scala3/reference/other-new-features/indentation.html)
- [Soft modifiers](https://docs.scala-lang.org/scala3/reference/soft-modifier.html)
- [`Tokens.scala` at Scala 3.9.0](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/parsing/Tokens.scala)
- [`Scanners.scala` at Scala 3.9.0](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/parsing/Scanners.scala)
- [`Parsers.scala` at Scala 3.9.0](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/parsing/Parsers.scala)
- [`Chars.scala` at Scala 3.9.0](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/util/Chars.scala)
- [`CharArrayReader.scala` at Scala 3.9.0](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/parsing/CharArrayReader.scala)

`Scanners.scala` is a behavioral oracle, not a Rust architecture to copy
line-for-line. We preserve language behavior while using explicit Rust data
types, UTF-8 source ranges, bounded state machines, and lossless raw data.

## 3. Architectural boundaries

The lexer will live in dedicated workspace crates, separate from the existing
`dotty-tasty` implementation crate. The current planned crate layout is:

```text
crates/
├── dotty-source
├── dotty-diagnostics
├── dotty-token
└── dotty-lexer
```

The root `dotty` crate will later act as the public facade. Parser and AST
crates are intentionally not part of the current implementation scope.

The planned module layout for the current lexer work is:

```text
crates/dotty-source/src/
├── lib.rs
├── source_text.rs
├── span.rs
└── line_index.rs

crates/dotty-diagnostics/src/
├── lib.rs
└── diagnostic.rs

crates/dotty-token/src/
├── lib.rs
├── kind.rs
├── token.rs
├── value.rs
├── token_source.rs
└── scanner_event.rs

crates/dotty-lexer/src/
├── lib.rs
├── cursor.rs
├── raw_token.rs
├── trivia.rs
├── identifier.rs
├── number.rs
├── string.rs
├── interpolation.rs
└── scanner/
    ├── mod.rs
    ├── region.rs
    ├── indentation.rs
    ├── newline.rs
    ├── colon.rs
    └── infix.rs
```

The future parser must depend on the shared `source` and `token` contracts, never on
the concrete `lexer` module. In particular, the parser must not import
`RawLexer`, `RawToken`, `RawTokenKind`, trivia implementation details, or the
scanner's region stack.

The intended dependency direction, including future consumers, is:

```text
dotty-source ───────► dotty-token
      │                     │
      ├────► dotty-diagnostics
      │                     │
      └─────────────────────┴────► dotty-lexer

future dotty-parser ─────► dotty-token
future dotty-parser ─────► dotty-source
future dotty-parser ─────► dotty-diagnostics
```

`RawToken` and `RawTokenKind` are lexer-internal to `dotty-lexer`. `Token`,
`TokenKind`, `TokenValue`, and `ScannerEvent` belong to `dotty-token`; they are
the future parser-facing contract shared by the scanner and a future parser.

`dotty-lexer` depends on `dotty-source`, `dotty-token`, and
`dotty-diagnostics`. No current crate depends on a parser or AST crate.

When parser work begins, `dotty-parser` will depend on `dotty-source`,
`dotty-token`, `dotty-ast`, and `dotty-diagnostics`, but not on
`dotty-lexer`.

## 4. Source model

Source text is UTF-8 and source ranges use byte offsets:

```rust
pub struct TextRange {
    pub start: u32,
    pub end: u32,
}
```

The implementation must validate conversions and avoid overflow. Rust `char`
indices must never be used as source positions.

`SourceText` owns or borrows the original UTF-8 buffer and provides checked
slicing. Tokens primarily refer to this buffer through `TextRange`; they do not
allocate a `String` for every lexeme.

`LineIndex` provides:

- byte offset to logical line;
- byte offset to UTF-8 column;
- byte offset to UTF-16 column for oracle comparison and tooling.

The original bytes remain unchanged. LF and CRLF are recognized as logical
line breaks without rewriting source ranges. Exact handling of bare CR and FF
is a compatibility case to be verified against Scala 3.9.0.

The cursor must support checked lookahead, raw reads for multiline strings,
and normal reads with the source's escape and line semantics. Every operation
must either advance or reach a terminal state.

## 5. Lossless raw stream

The raw layer preserves both lexical tokens and trivia. The stream must be
ordered and lossless: token and trivia ranges together account for every
consumed source byte, including malformed input that can be recovered.

The conceptual representation is:

```rust
enum RawItem {
    Token(RawToken),
    Trivia(Trivia),
}
```

The exact public shape may change, but the following information is required:

```rust
struct RawToken {
    kind: RawTokenKind,
    span: TextRange,
}

struct Trivia {
    kind: TriviaKind,
    span: TextRange,
}
```

Trivia includes spaces, tabs, physical newlines, line comments, and nested
block comments. A block comment retains its complete source span and enough
line-break information for later layout processing. Leading whitespace at the
start of a physical line must remain available; it cannot be normalized away.

Raw token values should normally be represented by source ranges. If cooked
data is needed for a backquoted identifier or literal, retain the raw spelling
as well as the cooked value.

Malformed input produces diagnostics and, where recovery is possible, an
error/raw item that permits scanning to continue. The raw lexer must not panic,
hang, or silently discard source text.

## 6. RawLexer responsibilities

The RawLexer understands characters and lexical modes only. It handles:

- Scala-compatible identifiers and identifier/operator suffixes;
- backquoted identifiers;
- greedily scanned operators;
- hard keyword classification;
- punctuation and delimiter characters;
- physical whitespace and line breaks;
- line comments and nested block comments;
- numeric, character, ordinary string, and multiline string literals;
- interpolation state transitions;
- quote and splice syntax where required by Scala 3.9.0;
- EOF and recoverable lexical diagnostics.

The raw lexer does not decide:

- whether a soft keyword is being used contextually;
- whether `*`, `+`, `-`, or `|` has grammatical meaning;
- whether a newline separates statements;
- whether indentation opens or closes a region;
- whether `:` is `COLONop`, `COLONfollow`, or `COLONeol`;
- whether `end` is an end marker;
- whether `case class` or `case object` is a fused parser token.

Hard keywords are classified after scanning a complete identifier/operator
lexeme. Soft keywords remain identifiers in the raw layer, including `as`,
`derives`, `extension`, `infix`, `inline`, `opaque`, `open`, `transparent`,
and `using`. `end` is retained as a hard keyword because the contextual
scanner needs to recognize end-marker candidates.

Operators are greedy, except that `/` must stop before `//` and `/*` so that
comments are not absorbed into an operator lexeme.

## 7. ContextualScanner responsibilities

The ContextualScanner consumes the raw stream incrementally and emits the
parser-facing stream. It is responsible for:

- converting physical newlines to no token, `NEWLINE`, or `NEWLINES`;
- statement-separator inference;
- indentation region tracking;
- synthetic `INDENT` and `OUTDENT` tokens;
- delimiter and EOF interaction with regions;
- leading infix continuation;
- contextual colon classification;
- end-marker recognition;
- `CASECLASS` and `CASEOBJECT` post-processing.

The scanner may maintain pending synthetic tokens, but their insertion
positions must be deterministic and source ranges must remain meaningful.

The first scanner implementation materializes the raw item stream while
constructing `ContextualScanner`. This keeps the state machine small while the
raw/scanner contract is still being stabilized. It can later be replaced by
incremental buffering without changing parser-facing token kinds or ranges.

The scanner must preserve source-token ordering, balance regions by EOF, and
never underflow its region stack.

## 8. Parser-facing token contract

The future parser will consume a token source, not a production `Vec<Token>`:

```rust
trait TokenSource {
    fn current(&self) -> &Token;
    fn advance(&mut self);
    fn lookahead(&mut self, n: usize) -> &Token;
    fn observe(&mut self, event: ScannerEvent);
}
```

The exact Rust lifetime ergonomics may require a small token view or owned
lightweight token copy. The semantic contract is more important than this
initial signature: lookahead is incremental, scanner events are explicit, and
the parser does not know the scanner implementation.

The event protocol initially includes:

```rust
enum ScannerEvent {
    ColonEol { in_template: bool },
    Indented,
    Outdented,
    ArrowIndented,
}
```

When parser work begins, parser tests will use an in-memory token source,
allowing parser grammar tests to run without lexical analysis. Current scanner
tests construct the scanner directly, without constructing a parser.

## 9. Compatibility decisions and watch points

The following decisions are fixed for the design:

| Area | Decision |
| --- | --- |
| Source encoding | UTF-8 internally; UTF-16 conversion only for tooling/oracle compatibility |
| Positions | Checked byte ranges; no Rust character indices |
| Trivia | Preserved in the raw layer |
| Soft keywords | Identifiers until contextual scanner/parser interpretation |
| Operators | Greedy raw lexemes, with comment boundary handling |
| Numeric values | Lexical spelling first; semantic conversion later |
| Indentation | Prefix comparison, not a fixed tab width |
| Errors | Typed diagnostics with recovery where possible |
| Unknown future syntax | Preserve raw source information rather than guessing grammar |

The following require direct Scala 3.9.0 oracle tests rather than assumptions:

- the exact numeric grammar for `1.`, `.5`, separators, suffixes, and malformed
  literals;
- JVM `Char` compatibility for character literals and Unicode escapes;
- repeated `u` escapes and legacy octal escape recovery;
- triple-quoted string terminator edge cases;
- nested interpolation token boundaries;
- quote, splice, and legacy quote syntax;
- `NEWLINE` versus `NEWLINES` behavior;
- incomparable tab/space indentation;
- leading infix behavior after blank lines;
- colon classification and parser-observed indentation;
- end-marker tags and matching diagnostics;
- exact synthetic-token source positions.

## 10. Incremental implementation plan

Each increment must land with focused tests before the next increment starts.

### Increment 0 — Oracle and compatibility harness

Prepare a small Scala helper that scans a source string with the Scala 3.9.0
compiler and emits machine-readable token records. Record token kind, raw text,
UTF-16 offsets, synthetic status, and diagnostics where available.

The pinned local environment is:

```text
Scala compiler: 3.9.0
JDK: 25 (currently 25.0.4-amzn through SDKMAN)
sbt: 2.0.9 through SDKMAN
```

The harness normalizes Dotty UTF-16 offsets to Rust UTF-8 byte offsets before
comparison. The compiler tag, JDK version, sbt version, source mode, and
compiler flags must be recorded with oracle output. A newer Scala compiler is
not a valid compatibility oracle for this project.

### Increment 1 — Source infrastructure

Implement `SourceText`, `TextRange`, `LineIndex`, `Cursor`, `Diagnostic`, and
`Trivia`.

Acceptance tests cover UTF-8 boundaries, supplementary Unicode, LF, CRLF,
lookahead, raw reads, source slicing, and malformed traversal. This increment
must not depend on parser code.

### Increment 2 — Basic raw lexer

Implement whitespace, physical newlines, line comments, nested block comments,
identifiers, backquoted identifiers, operators, hard keywords, punctuation, and
EOF.

Tests must assert exact kinds, spans, raw text, trivia ordering, nested comment
behavior, comment/newline interaction, and recovery for unterminated comments
and backquoted identifiers.

### Increment 3 — Literals

Implement integer and floating-point lexical recognition, character literals,
ordinary strings, multiline strings, and centralized escape handling.

Tests cover valid forms, malformed forms, truncated input, source spelling,
and Dotty-compatible diagnostics. Numeric conversion and overflow checks remain
outside this increment.

### Increment 4 — Interpolation

Implement arbitrary interpolator identifiers, `STRINGPART`, `$identifier`,
`${expression}`, `$$`, escaped quotes, multiline interpolation, and recursive
nested interpolation.

The implementation uses explicit lexical modes and brace depth. Regular
expressions are not an acceptable implementation strategy.

### Increment 5 — Semantic newlines

Introduce parser-facing tokens and the scanner's statement-start and
statement-end sets. Implement `NEWLINE`, `NEWLINES`, and semicolon inference
before full indentation handling.

Tests compare continuation, statement separation, blank lines, delimiters,
comments, and leading infix candidates against the oracle.

### Increment 6 — Regions and indentation

Implement `IndentWidth`, prefix ordering, region stack, `INDENT`, `OUTDENT`,
delimiter interaction, case clauses, multiple pending outdents, and EOF
cleanup.

Tests include nested regions, tabs/spaces, incomparable prefixes, `match`,
`catch`, nested cases, explicit delimiters, and malformed indentation.

### Increment 7 — Leading infix and colon protocol

Implement independently testable leading-infix detection and parser/scanner
events for `ColonEol`, `Indented`, `Outdented`, and `ArrowIndented`. Classify
`COLONop`, `COLONfollow`, and `COLONeol` only with the context available at this
layer. The current increment covers colon classification, event transport,
leading-infix continuation, and blank-line separation; richer lookahead and
outdent rules remain next.

### Increment 8 — End markers and post-processing

Implement contextual `END`, `CASECLASS`, `CASEOBJECT`, grammar-driven region
closures, and exact EOF behavior. The current increment covers token fusion,
basic end-marker recognition, non-panicking EOF classification, and closure of
implicit regions after dedented closing delimiters. More precise region
ownership and grammar-driven closures remain to be refined.

### Increment 9 — Quotes, legacy syntax, and XML

The current implementation covers term/type quote markers, legacy quoted
identifiers, and their interaction with character literals. It also recognizes
the Scala `XMLSTART` entry point, preserves XML's greedy tag operators, and
tracks enough tag-closing state to end layout-sensitive processing after the
root literal. Add remaining quote/splice forms, compatibility syntax, and the
full XML body/attribute/expression state machine without entangling it with
normal Scala tokenization.

## 11. Testing strategy

### Focused unit tests

Every lexer/scanner increment adds tests for the behavior introduced. Tests
must cover valid input, empty input, optional and repeated constructs,
truncated input, malformed input, invalid Unicode, and trailing source bytes
where applicable. Assertions should include exact error kinds, spans, token
kinds, and references to the source text.

### Differential tests

Run the Rust implementation and the pinned Scala 3.9.0 oracle on the same
source snippets. Compare:

- token kind and order;
- raw lexeme or normalized literal spelling;
- source ranges after UTF-16-to-UTF-8 normalization;
- synthetic token positions;
- diagnostics where stable enough to compare.

Every discovered mismatch becomes a minimized regression fixture with a note
if compiler behavior differs from the language specification.

The current developer harness is `bash tools/scala-lexer-oracle/compare.sh`.
It compares normalized token kinds and source-token starts; synthetic layout
tokens are compared by kind and order while their exact ranges remain a
separate scanner test concern.

### Corpus tests

After the raw lexer is stable, tokenize Scala 3 sources from the compiler,
standard library, positive and negative compiler tests, and selected real-world
Scala 3 projects. Corpus failures report the exact path, token, offset, and
diagnostic involved.

### Fuzz and property tests

Use `proptest`, `arbitrary`, or `cargo-fuzz` for UTF-8 input, Unicode
identifiers, operators, quote sequences, escapes, nested comments,
interpolation, braces, indentation, and EOF at every input position.

For every valid UTF-8 input, the raw lexer and scanner must terminate, make
progress, reach EOF or a recoverable terminal state, and never panic.

## 12. Invariants

### RawLexer invariants

1. Cursor operations advance or reach EOF.
2. Every emitted item has a valid, non-overlapping source span.
3. Tokens and trivia account for all consumed source bytes.
4. Source slices occur only at UTF-8 boundaries.
5. Lookahead does not corrupt cursor state.
6. Malformed user input cannot cause a panic or infinite loop.

### ContextualScanner invariants

1. Synthetic tokens have deterministic insertion positions.
2. Indentation regions cannot underflow.
3. Regions are closed at EOF according to the selected recovery policy.
4. Lookahead does not expose partially committed scanner state.
5. Scanner events are handled idempotently where the parser can repeat them.
6. Source tokens are not silently lost.

## 13. Definition of done for the core lexer

The core lexer milestone is complete when:

- source ranges and line tracking are correct for UTF-8 and CRLF;
- raw tokens and trivia form a lossless stream;
- identifiers, hard keywords, soft keywords, backquotes, operators, comments,
  punctuation, and EOF match Scala 3.9.0 behavior;
- malformed input produces typed diagnostics without panic or hang;
- focused unit tests and raw differential tests exist;
- the oracle environment is pinned and reproducible with JDK 25 and sbt 2.0.9;
- no indentation or parser-specific behavior has leaked into `RawLexer`.

The complete lexer/scanner milestone additionally requires semantic newlines,
indentation, leading infix handling, colon protocol, end markers, and the
corresponding differential, corpus, and fuzz coverage.
