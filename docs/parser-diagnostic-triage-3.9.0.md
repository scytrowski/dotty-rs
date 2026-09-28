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
| Prefixes on named `using` parameters | 9/30 | Six files use `using @constructorOnly name: T` (including `OptimizerSettings.scala:12`, `387..388`, and `Capability.scala:176`, `7456..7457`); three use constructor accessors such as `using val ctx: Context` (`QuotesImpl.scala`, `Extractors.scala`, `SourceCode.scala`). Rust reports `expected a type operand` at `@`/`val`. Dotty emits trees for these examples. | Fix the parameter-clause classifier so annotations and allowed parameter modifiers do not make a named term parameter look like an anonymous context type. Test both forms, plus anonymous `using T`, which must remain unchanged. The shared path is `current_is_anonymous_using_type` in `parameters.rs`. |
| Indented structural `given` body after `with` | 2/30 | `SymbolUtils.scala:13`, `262..262` fails at `extension`; `Constants.scala:234`, `9352..9355` fails at `def`. Both are members in an indented `given ... with` body and both have Dotty trees. The parser's given-parent loop treats `with` as another parent separator and attempts to parse the first body member as a type. | Separate given-template-body boundary work in `givens.rs`; distinguish a following indented body from another parent before parsing a parent type. Include a normal member and an extension member. |
| Symbolic type names / constructors | 6/30 | Examples include `::` in `Option.scala:642` (`21781..21783`), `List.scala:103` (`4038..4040`), `Decorators.scala:324` (`12521..12523`), `=:=` in `FunctionExtensions.scala:54` (`2076..2079`), and symbolic declarations in `package.scala:78` and `typeConstraints.scala:64`. Diagnostics vary between `expected a type operand`, `expected a simple type`, and a missing type name. Dotty emits trees for the sampled forms. | Treat operator-spelled type names as a type-name grammar issue, not a general missing-type issue. Compare declarations, references, and constructor syntax in Dotty before choosing one shared parser change. |
| Annotation target / nested annotation syntax | 3/30 | `Predef.scala:443`, `19200..19201`; `RedBlackTree.scala:595`, `26725..26726`; and `AnyRefMap.scala:46`, `1864..1865` report `expected an annotation type after @`. The spelling includes annotations applied to annotations or targeted annotations. The `AnyRefMap` oracle result is inconclusive due to the null-context exception above. | Verify each precise annotation form separately against Dotty; do not broaden ordinary annotation parsing based only on the shared message. |
| Remaining type/declaration cases | 10/30 | Heterogeneous remainder: nullable/union types (`Formatting.scala`, `ArrayBuilder.scala`, `ListBuffer.scala`), wildcard placement (`TreeSeqMap.scala:289`), extension/type contexts (`SyntheticsExtractor.scala`), symbolic or unusual parameter forms, and one parser diagnostic overlapped by scanner diagnostics in `semanticdb/Scala3.scala`. Some source-level oracle runs are inconclusive. | Keep these as individual reproducers until each has a confirmed Dotty parse result and owning production. No single “ExpectedType support” increment is justified. |

The nine parameter-prefix cases are the clearest implementation opportunity:
they are repeated, valid Scala syntax and point to one dispatch predicate. The
two structural-given cases are a second, independent candidate. The remaining
bucket should not be collapsed into either fix.

## `UnexpectedToken` (24 first failures)

The current message split is:

| First diagnostic message | Files |
| --- | ---: |
| `expected a block statement separator` | 20 |
| `expected a template member separator` | 4 |

Dotty produced trees for all 24 files, but the 20 block-separator locations do
not identify one shared separator defect. Representative distinct contexts:

| Source location | Rust span / message | Context indicated by source | Recommended treatment |
| --- | --- | --- | --- |
| `BTypeLoader.scala:178` | `8716..8717`, block separator | Comma after a multiline `if` expression used as an argument. | Minimize conditional-expression/argument boundary independently. |
| `AliasingAnalyzer.scala:621` | `22633..22634`, block separator | Final expression after a braced `match` inside a block. | Check match-body termination and the block's final expression as a separate case. |
| `BoxUnbox.scala:810` | `37944..37946`, block separator | `if` after local statements in a lambda passed to `flatMap`. | Isolate multi-statement lambda-body parsing; do not change generic separators first. |
| `CaptureAnnotation.scala:76` | `2912..2916`, block separator | `then` after an indented colon argument containing `case` clauses. | Track as a colon-argument/layout boundary; it is distinct from an ordinary brace block. |
| `Phases.scala:537` and `BestEffortTastyWriter.scala:26` | `22831..22838` / `992..999`, block separator | `finally` following a `try` body. | Confirm try/finally body ownership; avoid counting both as evidence for a universal block rule. |
| `trace.scala:55` | `1923..1925`, block separator | `inline if` form. | Keep inline control-flow support separate from ordinary `if` parsing. |
| `Denotations.scala:267`, `TyperState.scala:311`, `Inlines.scala:610`, `Inferencing.scala:65` | template separator | Inline/match, interpolation, or nested member/extension boundaries. | Triage each enclosing production and layout transition separately. |

Other block-separator first failures occur around body/branch boundaries or
nested expressions in `BackendUtils.scala`, `Annotations.scala`,
`ContextOps.scala`, `NamerOps.scala`, `Types.scala`, `PickledQuotes.scala`,
`Reporter.scala`, `messages.scala`, `Erasure.scala`,
`FullParameterization.scala`, `UnrollDefinitions.scala`,
`QuotesAndSplices.scala`, and `Source.scala`. These are valid Dotty inputs,
but their enclosing forms differ; a lower diagnostic count alone would not
prove that a common separator change is correct.

## Suggested implementation queue

1. Named `using` parameters with annotations/modifiers (9 observed first
   failures; add focused tests for `@constructorOnly` and `using val`).
2. Indented structural-given body after `with` (2 observed first failures;
   cover both `def` and nested `extension`).
3. Symbolic type names in declarations/references/constructor positions (6
   observed first failures; split further if Dotty shows distinct productions).
4. Verify targeted/nested annotation forms (3 observed first failures; keep
   parser-valid forms separate from any oracle-inconclusive example).
5. Reduce the 20 block-separator files by grammar context before opening any
   broad fix: conditional arguments, final expressions after `match`, lambda
   bodies, colon arguments, `try/finally`, and inline control flow are the
   current concrete leads. Keep the four template-separator files separate.
6. Revisit the remaining type cases only with minimized sources and a conclusive
   pinned-Dotty result.

No parser or lexer behavior was changed during this triage. The source parser
remains independent of `dotty-lexer`; the report was diagnostic research only.
