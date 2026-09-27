# Parser separator diagnostics triage (Scala 3.9.0)

This note follows issue [#356](https://github.com/scytrowski/dotty-rs/issues/356).
It classifies the current parser's first `UnexpectedToken` diagnostics without
assuming that the diagnostic wording identifies one grammar defect.

## Measurement and reference

The measurement was run on `dotty-rs` revision
`8b10dafe511105d2838f38f2a7535dc7562958f9` (after PR #361), against the same
sorted `library/src` and `compiler/src` sources from Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`. It attempted 1,236 files: 879
parsed without diagnostics, 357 had recoverable diagnostics, and there were no
hard failures, panics, or hangs. There were 3,248 `UnexpectedToken`
occurrences. Of the 110 files whose first parser diagnostic is
`UnexpectedToken`, direct per-file runs report:

| First diagnostic | Files |
| --- | ---: |
| `expected a block statement separator` | 60 |
| `expected a template member separator` | 48 |
| `a constructor application cannot be applied again` | 1 |
| `parser made no progress while parsing an expression suffix` | 1 |
| **Total** | **110** |

This is a current-main rerun, not a contradiction in the historical post-#348
report: #356 was opened against that older state, where the first-diagnostic
bucket contained 97 files. The overall histogram also counts diagnostics after
the first one, so it is not a count of independent root causes.

The pinned Scala parser oracle was run on these 110 source files. It emitted
normalized compilation trees for 104; six runs ended in a `Context` null-pointer
exception inside the oracle path and are inconclusive. For example, Dotty
produced trees for `MainGenericCompiler.scala`, `TreeInfo.scala`,
`WellKnownBTypes.scala`, `Stepper.scala`, `InlineReducer.scala`, and `Array.scala`.
The oracle is a parser/tree comparison, not a full typecheck of the source
corpus.

## Findings

The two separator messages cover heterogeneous syntax. The next-token spelling
is a useful index into the source, but not a grammar classification. In the
template bucket, common spellings include `def` (11), `end` (8), `with` (7),
`private` (6), and `override` (5). In the block bucket, common spellings
include `val` (12), `else` (11), `)` (8), and `if` (7). Those tokens occur in
different enclosing constructs.

Representative findings, with source offsets in UTF-8 bytes from the beginning
of the file:

| Source location | Rust result | Dotty 3.9.0 result | Interpretation |
| --- | --- | --- | --- |
| `compiler/src/dotty/tools/MainGenericCompiler.scala:31`, 847..850 (`end`) | Expected template-member separator | Tree emitted | A named end marker after an indented method body is not being recognized at this template boundary. |
| `compiler/src/dotty/tools/dotc/ast/TreeInfo.scala:115`, 3282..3285 (`def`) | Expected template-member separator | Tree emitted | The next member follows an indentation-style `match` body; this is a concrete outdent/separator-feedback candidate. |
| `library/src/scala/collection/Stepper.scala:226`, 9358..9362 (`with`) | Expected template-member separator | Tree emitted | `new BoxedDoubleStepper(...) with EfficientSplit`; this is an anonymous-template/mixin tail in an expression, not evidence by itself of a missing member separator. |
| `compiler/src/dotty/tools/backend/jvm/WellKnownBTypes.scala:215`, 11834..11837 (`val`) | Expected block-statement separator | Tree emitted | A local `val` sequence inside a brace-delimited lambda body; the enclosing higher-order call makes this a candidate for a focused block/argument reproducer. |
| `compiler/src/dotty/tools/dotc/core/Types.scala:314`, 13054..13059 (`catch`) | Expected block-statement separator | Tree emitted | A `catch case` following a nested expression; distinct from the simple local-`val` example. |
| `compiler/src/dotty/tools/dotc/inlines/InlineReducer.scala:453`, 20403..20404 (`(`) | Constructor application cannot be applied again | Tree emitted | A tuple expression follows a colon-indented `collect` body. This points toward colon-argument/outdent ownership, not an ordinary repeated constructor call. |
| `library/src/scala/Array.scala:397`, 13747..13748 (`:`) | Expression-suffix parser reports no progress | Tree emitted | A parenthesized type ascription on a `new` expression. This singleton is a concrete progress/recovery bug candidate and should be isolated separately. |

Dotty's Scala 3.9 `Parsers.scala` handles both `templateStatSeq` and
`blockStatSeq` through `statSepOrEnd`; that routine calls
`in.observeOutdented()` before it decides whether a statement separator or
sequence terminator follows. The Rust parser has separate template and block
sequence loops with scanner feedback at their own boundaries. The contrast
makes layout feedback worth testing, but does not prove that all 108 separator
diagnostics share one defect.

The wider samples include named end markers, dedented members after unbraced
`match`/conditional bodies, nested lambda and callback blocks, `else`/`catch`/
`finally` boundaries, anonymous `new ... with ...` expressions, and capture-
checking type spellings. Several therefore reflect independently unsupported
grammar or recovery cascades rather than a standalone separator bug. Do not
change the shared separator policy based on these counts alone.

## Recommended follow-ups

Prioritize small, independently testable tasks:

1. Minimize and test scanner/parser outdent feedback at template boundaries,
   covering a following member and a named `end` marker. Keep these together
   only if one minimal reproducer demonstrates the same cause.
2. Minimize the block-sequence cases separately, especially a local statement
   after a nested lambda/callback body and an `else`/`catch` boundary. The
   current 60-file sample is too heterogeneous to justify one broad fix.
3. Track anonymous class/mixin tails after `new` as a grammar-coverage task;
   do not relabel them as separator recovery.
4. Reproduce the `Array.scala` parenthesized-ascription no-progress case as a
   focused parser regression before changing its recovery path.
5. Reproduce the `InlineReducer.scala` colon-argument/tuple boundary separately;
   it may join the outdent task only after the minimal cases agree.

The issue findings propose these implementation scopes; no separator or
recovery behavior was changed as part of this triage.
