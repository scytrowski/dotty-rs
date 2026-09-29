# Parser `ExpectedType` / `UnexpectedToken` triage (Scala 3.9.0)

This note records the follow-up to [issue #441](https://github.com/scytrowski/dotty-rs/issues/441).
The diagnostic kind is an outcome label, not a grammar diagnosis; the two
first-failure buckets contain several independent source forms.

## Measurement and method

The pinned input is Scala 3.9.0 at revision
`777528f19a58e794c9954a42f433373472ec57f8` (`library/src` and `compiler/src`,
1,236 files). A parser-only report was rerun on dotty-rs `8630a6c1` (after
PR #454; PR #455 only adds documentation). The runner completed all files with
no hard failures, panics, or hangs:

| Measure | After #436 | Current rerun |
| --- | ---: | ---: |
| Clean files | 1,074 | 1,091 |
| Recoverable-diagnostic files | 162 | 145 |
| First-failure `ExpectedType` | 29 | 30 |
| First-failure `UnexpectedToken` | 23 | 24 |
| `UnexpectedToken` diagnostic occurrences | 580 | 533 |

The +1 first-failure counts do not establish regressions: first-failure
attribution can move when an earlier error is fixed. The overall number of
clean files increased by 17 and `UnexpectedToken` occurrences fell by 47.
Counts below refer to the current first-failure files, not all diagnostic
occurrences or all instances of a syntax form.

All 54 current first-failure files in these two buckets were sent through the
Scala 3.9 parser oracle in one batch. Dotty emitted normalized trees for 49.
Five `ExpectedType`-bucket files failed in the oracle with a null `Context`
exception (`Iterator.scala`, `AnyRefMap.scala`, `ArrayBuilder.scala`,
`TreeSet.scala`, and `UnrolledBuffer.scala`); these results are inconclusive,
not evidence that Dotty rejects their syntax. Every one of the 24
`UnexpectedToken` examples produced a Dotty tree. Source offsets below are
UTF-8 byte ranges, end-exclusive, matching Rust diagnostics.

## `ExpectedType` (30 first failures)

| Subgroup | Observed reach | Evidence and interpretation | Follow-up direction |
| --- | ---: | --- | --- |
| Named `using` parameters with annotation/modifier prefixes | 9/30 | All nine exact first diagnostics are listed below. Rust reports `ExpectedType: expected a type operand`; Dotty emitted trees for all nine. | Fix the parameter-clause classifier so annotation/modifier prefixes do not make a named term parameter look like an anonymous context type. Owner: `parameters.rs::current_is_anonymous_using_type`. Preserve anonymous `using T`. |
| Indented given/template body after `with` | 5/30 | Five cases have this body-boundary shape: `compiler/src/dotty/tools/backend/jvm/SymbolUtils.scala:13` (`262..262`, `extension`), `compiler/src/dotty/tools/dotc/core/Constants.scala:234` (`9352..9355`, `def`), `compiler/src/dotty/tools/dotc/semanticdb/SyntheticsExtractor.scala:97` (`3465..3465`, `extension`), `library/src/scala/util/CommandLineParser.scala:81` (`3106..3109`, `def`), and `compiler/src/dotty/tools/dotc/semanticdb/Scala3.scala:80` (`2954..2954`, `extension`). Each originally reported `ExpectedType: expected a type operand`; pinned Dotty emitted a tree for each. Fixed in [issue #459](https://github.com/scytrowski/dotty-rs/issues/459): a line-final `with` now requests scanner indentation feedback and enters the shared template-body parser, while same-line `with Parent` remains a parent separator. The focused rerun parses the first four files without diagnostics; `Scala3.scala` remains a recoverable corpus result with eight independent invalid-escape scanner diagnostics in raw-regex strings at `716..719`, `721..724`, `783..792`, `841..858`, `841..863`, `841..865`, `841..870`, and `841..872`, so its aggregate status is not a clean parser-only signal. A minimized fixture differentially checks ordinary and extension members plus an explicit parent before the body. |
| Symbolic type names / constructors (#461 follow-up) | Initial sample 6/30; recheck: 5 files clean, `List.scala` has a later unrelated diagnostic | The original failures at symbolic type spellings are fixed for `Option.scala:642`, `List.scala:103`, `Decorators.scala:324`, `FunctionExtensions.scala:54`, `package.scala:78`, and `typeConstraints.scala:64`. Regression fixtures also cover `def ::` and `new ::[...]`. Current `List.scala` proceeds beyond the original `::` declaration and now first reports `expected a visibility qualifier` at byte span `20020..20022`, the `::` in `private[::] def \`next$access$1\``; Dotty accepts this source through capture-checking/visibility syntax that #461 does not cover. Dotty emitted trees for all six files in the original audit. | #461 is scoped to operator-spelled type names in type positions; it does not claim that the complete corpus files now parse without diagnostics. Track the later `private[::]` syntax independently. |
| Annotation target / nested annotation syntax | 3/30 | `library/src/scala/Predef.scala:443` (`19200..19201`), `library/src/scala/collection/immutable/RedBlackTree.scala:595` (`26725..26726`), and `library/src/scala/collection/mutable/AnyRefMap.scala:46` (`1864..1865`) each report `ExpectedType: expected an annotation type after @`. The minimized forms are `@(deprecated @companionMethod)(...)` and `@(`inline` @getter @setter)`. Dotty emitted trees for Predef and RedBlackTree; its AnyRefMap run failed with the null-`Context` harness exception, so that full-file result remains inconclusive. The new `nested-annotation-forms.scala` fixture compares both forms against Scala 3.9, and the Rust parser now accepts the parenthesized `SimpleType1` annotation type without changing ordinary annotation parsing. This validates the shared syntax, not the inconclusive AnyRefMap file. | No additional parser work indicated by these three rows; address the AnyRefMap oracle harness failure separately before claiming full-file parity. |
| Null/capture-related type forms | 6/30 | `compiler/src/dotty/tools/dotc/printing/Formatting.scala:64` (`2430..2433`, `X | Null`), `library/src/scala/collection/Iterator.scala:200` (`8330..8332`, capture-annotated function type), `library/src/scala/collection/mutable/ArrayBuilder.scala:425` (`11730..11739`, after a capture-annotated self type), `library/src/scala/collection/mutable/ListBuffer.scala:53` (`1770..1772`, `::[A] | Null`), `library/src/scala/collection/mutable/TreeSet.scala:122` (`4895..4902`, after a capture-annotated self type), and `library/src/scala/collection/mutable/UnrolledBuffer.scala:270` (`7989..7996`, after a capture-annotated self type) report `ExpectedType: expected a type operand` from `types.rs::parse_type_operand`. Dotty emitted trees for Formatting/ListBuffer; the other four are oracle-inconclusive because of the null-`Context` harness failure. | Separate nullable-union support from capture syntax before implementation. Reach is six first-failure files, but only two have a conclusive tree result in this run. |
| Wildcard type outside a type-argument position | 1/30 | `library/src/scala/collection/immutable/TreeSeqMap.scala:289` (`9203..9204`) reports `ExpectedType: a wildcard type is only valid as a type argument`; Dotty emitted a tree. | Compare the tuple/wildcard type production with Scala 3.9; keep the change limited to this position. Owner: `types.rs::parse_type_operand`. |

The nine named-parameter cases are the clearest implementation opportunity:
they are repeated, valid Scala syntax and point to one dispatch predicate.
The five given-body cases look related but should be minimized before assuming
one shared cause. All 30 first failures are now assigned to observed syntax
groups. The scanner diagnostics in `Scala3.scala` are a separate lexer finding,
not evidence for a broad parser/type change.

Exact paths for the nine named-parameter cases (all first diagnostics are
`ExpectedType: expected a type operand`; all have Dotty trees):

| Path and source location | Span |
| --- | ---: |
| `compiler/src/dotty/tools/backend/jvm/opt/OptimizerSettings.scala:12` | `387..388` |
| `compiler/src/dotty/tools/dotc/cc/Capability.scala:176` | `7456..7457` |
| `compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala:68` | `2700..2701` |
| `compiler/src/dotty/tools/dotc/core/tasty/TreeUnpickler.scala` | `5285..5286` |
| `compiler/src/dotty/tools/dotc/transform/Pickler.scala` | `1663..1664` |
| `compiler/src/dotty/tools/dotc/typer/Synthesizer.scala` | `725..726` |
| `compiler/src/scala/quoted/runtime/impl/QuotesImpl.scala` | `1405..1408` |
| `compiler/src/scala/quoted/runtime/impl/printers/Extractors.scala` | `3462..3465` |
| `compiler/src/scala/quoted/runtime/impl/printers/SourceCode.scala` | `3632..3635` |

## `UnexpectedToken` (24 first failures)

The current message split is:

| First diagnostic message | Files |
| --- | ---: |
| `expected a block statement separator` | 20 |
| `expected a template member separator` | 4 |

All 20 block-separator first failures have the exact Rust diagnostic
`UnexpectedToken: expected a block statement separator`, emitted by
`statements.rs::parse_statement_sequence`. Dotty returned a tree for each of
the 20. This confirms parser-layer acceptance in the pinned oracle; it does
not say that the files typecheck. Six observed contexts are specific enough to
motivate isolated reproducers:

| Context / observed reach | Representative source and span | Source landmark at the Rust diagnostic | Narrow follow-up |
| --- | --- | --- | --- |
| Multiline `if` call argument (1/20) | `BTypeLoader.scala:178`, `8716..8717` | Comma after the multiline expression argument. | Minimize the `if`-as-argument boundary. |
| Final expression after braced `match` (1/20) | `AliasingAnalyzer.scala:621`, `22633..22634` | Following expression in an enclosing block. | Check match-body termination against the enclosing block's final expression. |
| Nested lambda/control-flow body (1/20) | `BoxUnbox.scala:810`, `37944..37946` | `if` after local statements in a lambda passed to `flatMap`. | Isolate the lambda block and its nested branch sequence. |
| Colon-argument/layout boundary (1/20) | `CaptureAnnotation.scala:76`, `2912..2916` | `then` after an indented colon argument containing `case` clauses. | Keep separate from ordinary brace-block separators; minimize scanner feedback and colon-argument ownership. |
| `try` body followed by `finally` (2/20) | `Phases.scala:537`, `22831..22838`; `BestEffortTastyWriter.scala:26`, `992..999` | `finally` at the boundary after each try body. | Compare try/finally body ownership; two files are evidence for this context, not for a universal separator fix. |
| Inline conditional (1/20) | `reporting/trace.scala:55`, `1923..1925` | `inline if` in an expression body. | Keep inline control flow separate from the ordinary `if` production. |

The other 13 files are enumerated below rather than grouped under a guessed
grammar cause. In each, the span is where the common statement-sequence
separator check fires; the nearby token alone does not identify which earlier
construct made the sequence appear unterminated. Their Rust owner/diagnostic
and Dotty result are the same as stated above. Each currently contributes one
observed file; do not combine them into one implementation task until a
minimal reproducer establishes a common cause.

| Path and line | Span | Nearby source landmark | Why not classified more narrowly yet |
| --- | ---: | --- | --- |
| `compiler/src/dotty/tools/backend/jvm/BackendUtils.scala:385` | `17928..17929` | Closing brace after a conditional expression. | Failure is reported at the enclosing brace, not at a discriminating inner token. |
| `compiler/src/dotty/tools/dotc/core/Annotations.scala:293` | `11898..11899` | Closing brace after a contextual-function-typed lazy annotation body. | Could be a type/body-boundary cascade; the separator span does not isolate it. |
| `compiler/src/dotty/tools/dotc/core/ContextOps.scala:76` | `3616..3620` | `else` after an indented `if ... then` member expression. | Need a minimal conditional/member-body example to distinguish layout from expression parsing. |
| `compiler/src/dotty/tools/dotc/core/NamerOps.scala:254` | `11542..11547` | `match` following an `@unchecked` type ascription. | Pattern/match and enclosing method-body boundaries overlap at the reported sequence error. |
| `compiler/src/dotty/tools/dotc/core/Types.scala:2037` | `87427..87428` | Comma after a multiline `mapConserve:` argument in `FunctionNOf`. | Colon-argument termination and call-argument continuation are both involved. |
| `compiler/src/dotty/tools/dotc/quoted/PickledQuotes.scala:207` | `9255..9259` | `else` in an `if` inside a braced lambda. | The downstream separator follows conditional/lambda parsing; no smaller shared failure is established. |
| `compiler/src/dotty/tools/dotc/reporting/Reporter.scala:63` | `2070..2071` | Closing brace after a local recursive `loop()` body. | Error is at a brace after nested local control flow, so the enclosing boundary needs minimization. |
| `compiler/src/dotty/tools/dotc/reporting/messages.scala:1220` | `46020..46024` | `else` in a multiline conditional used to build an interpolated message. | May be conditional/interpolation interaction; not evidence for a generic block rule. |
| `compiler/src/dotty/tools/dotc/transform/Erasure.scala:504` | `22388..22389` | Comma after a lambda argument in a multiline call. | Lambda-body termination and following call argument are both candidates. |
| `compiler/src/dotty/tools/dotc/transform/FullParameterization.scala:123` | `5085..5086` | Comma after a multiline `PolyType` argument. | Nested type/lambda argument boundary; needs an isolated type-construction reproducer. |
| `compiler/src/dotty/tools/dotc/transform/UnrollDefinitions.scala:177` | `6822..6824` | `if` in a nested local method body. | The source landmark starts a conditional, but the diagnostic is a sequence error; ownership is not yet isolated. |
| `compiler/src/dotty/tools/dotc/typer/QuotesAndSplices.scala:393` | `18208..18209` | Closing brace after nested polyfunction-type construction. | The closing boundary follows nested type syntax; no specific shared statement form is confirmed. |
| `library/src/scala/io/Source.scala:209` | `7498..7502` | `else` in a legacy-style conditional followed by infix-style calls. | Legacy syntax and expression termination overlap; isolate before assigning a parser fix. |

The four template-separator first failures share the Rust diagnostic
`UnexpectedToken: expected a template member separator` from
`templates.rs::parse_template_body`. Dotty returned a tree for all four, but
their source landmarks differ; the current evidence supports four one-file
reproducers, not a universal template-separator change.

| Path and line | Span | Source landmark | Current interpretation / reach |
| --- | ---: | --- | --- |
| `compiler/src/dotty/tools/dotc/core/Denotations.scala:267` | `11404..11408` | `inline this match` in a template member. | Inline/match member boundary; 1 file. |
| `compiler/src/dotty/tools/dotc/core/TyperState.scala:311` | `12377..12420` | Nested conditional inside string interpolation in a member body. | Interpolation/expression boundary; 1 file. |
| `compiler/src/dotty/tools/dotc/inlines/Inlines.scala:610` | `28582..28585` | `def apply(t: Type) =` inside an indented anonymous `TypeMap` body. | Indented nested member boundary; 1 file. |
| `compiler/src/dotty/tools/dotc/typer/Inferencing.scala:65` | `2737..2738` | Comma after an override member in an anonymous `ForceDegree.Value` initializer. | Anonymous-template/call-argument boundary; 1 file. |

## Suggested implementation queue

1. Named `using` parameters with annotations/modifiers (9 observed first
   failures; add focused tests for `@constructorOnly` and `using val`).
2. Indented given/template body after `with` (5 observed first failures;
   cover ordinary `def` members and `extension` members in separate reproducers).
3. Symbolic type names in declarations/references/constructor positions (6
   observed first failures; split further if Dotty shows distinct productions).
4. Verify targeted/nested annotation forms (3 observed first failures; keep
   parser-valid forms separate from any oracle-inconclusive example).
5. Reduce the 20 block-separator files by grammar context before opening any
   broad fix. Six contexts have concrete leads; keep the other 13 enumerated
   files as separate reproducers until their earlier construct is isolated.
   Keep all four template-separator files separate as well.
6. Revisit null/capture and wildcard-position cases only with minimized
   sources and a conclusive pinned-Dotty result; four of the six null/capture
   cases currently have inconclusive oracle results. Track the raw-regex
   diagnostics in `Scala3.scala` separately in the lexer backlog.

No parser or lexer behavior was changed during this triage. The source parser
remains independent of `dotty-lexer`; the report was diagnostic research only.

## Follow-up after symbolic type-name support

The six symbolic-type corpus files were rechecked after the changes for
[#461](https://github.com/scytrowski/dotty-rs/issues/461). Five became clean;
`library/src/scala/collection/immutable/List.scala` advanced beyond its
`def ::` type-name failure and then reported three `ExpectedToken` diagnostics
and one `UnsupportedSyntax` diagnostic at byte span `20020..20022`, on the
symbol in this declaration:

```scala
private[::] def `next$access$1` = next
```

Pinned Dotty 3.9 accepts this as an `AccessQualifier` containing an identifier
(`id | this`); the parser's access-qualifier production had not accepted
symbolic identifier tokens. This narrow follow-up is tracked in
[#471](https://github.com/scytrowski/dotty-rs/issues/471). The full-file result
depends on both symbolic type-name support from #461 and this access-qualifier
support.
