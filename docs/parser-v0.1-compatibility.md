# Parser v0.1 source-compatibility gate

This report measures the source parser after the parser backlog through issue
#193. It is an evidence-based gate for the implemented parser subset, not a
claim of complete Scala 3.9 grammar support.

## Reproduction and corpus

Run from the dotty-rs checkout, with the pinned Scala 3.9.0 checkout available
at `/tmp/scala3-3.9.0`:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-v0.1-final-scala3-3.9.0.json
```

The Scala checkout is revision
`777528f19a58e794c9954a42f433373472ec57f8`. The final measurement used
dotty-rs commit `1562374` (main after PR #217). It covers the same sorted
`library/src` and `compiler/src` files as the issue #185 baseline. The Scala
oracle emitted one result for all 1,236 files and reported 30 source-tree
exceptions; those are recorded oracle failures, not Rust parser panics or
hard failures.

## Result and delta from issue #185

| Measure | Baseline | Final | Change |
| --- | ---: | ---: | ---: |
| Files attempted | 1,236 | 1,236 | 0 |
| Clean parse | 320 (25.89%) | 389 (31.47%) | +69 files (+5.58 pp) |
| Recoverable diagnostics | 916 (74.11%) | 847 (68.53%) | -69 files (-5.58 pp) |
| Hard parser failures | 0 | 0 | 0 |
| Panics | 0 | 0 | 0 |
| Hangs | 0 | 0 | 0 |
| Scanner diagnostics | 17 | 17 | 0 |
| Scala oracle results / exceptions | 1,236 / 30 | 1,236 / 30 | unchanged |

Diagnostic counts are diagnostic occurrences, not affected-file counts:

| Diagnostic | Baseline | Final | Change |
| --- | ---: | ---: | ---: |
| `ExpectedExpression` | 43,093 | 37,442 | -5,651 |
| `ExpectedPattern` | 113 | 110 | -3 |
| `ExpectedToken` | 10,727 | 8,582 | -2,145 |
| `ExpectedType` | 2,856 | 2,423 | -433 |
| `UnexpectedToken` | 26,978 | 21,012 | -5,966 |
| `UnsupportedSyntax` | 14,705 | 6,114 | -8,591 (-58.4%) |
| **Total** | **98,472** | **75,683** | **-22,789 (-23.1%)** |

The two checked-in JSON reports are the detailed machine-readable record:
[`baseline-scala3-3.9.0.json`](../tools/parser-corpus-report/baseline-scala3-3.9.0.json)
and
[`parser-v0.1-final-scala3-3.9.0.json`](../tools/parser-corpus-report/parser-v0.1-final-scala3-3.9.0.json).

## Remaining syntax buckets

The counts below come from `first_failure_histogram`: they count files whose
first parser diagnostic belongs to that bucket, not every occurrence of the
construct. Up to five example paths are in the final JSON report.

| First-failure bucket | Files | Disposition |
| --- | ---: | --- |
| Scala 2 wildcard import/export spelling `_` | 44 | Intentional compatibility boundary: Scala 3 source uses `*`; legacy spelling remains unsupported unless a migration mode is added. |
| Legacy `implicit` parameter clauses | 25 | Feature/compatibility follow-up; supported `using` syntax is not affected. |
| Package objects | 18 | Follow-up before broad package/member naming; not a blocker for the current package/class subset. |
| Top-level expression diagnostic | 18 | Follow-up. The bucket includes valid named `end` markers (for example `end SetupAPI` in `dotty/cc/Setup.scala`), so its current diagnostic text is too generic to describe every source form. |
| Compound template self types | 7 | Follow-up; valid Scala syntax, currently rejected explicitly. |
| Unsupported enum-case forms | 5 | Follow-up; triage the exact variants in the listed examples before expanding support. |
| Interleaved type/term parameter clauses | 1 | Intentional AST/model deferral; the current `DefDef` keeps the leading type clause separate. |

The remaining first-diagnostic file counts are `ExpectedExpression` 243,
`ExpectedToken` 215, `UnexpectedToken` 159, `ExpectedType` 109, and
`ExpectedPattern` 3. These are recovery categories, not reliable syntax-family
counts: one unsupported construct can cascade into several such diagnostics.
They remain follow-up triage, not evidence of 729 independent missing grammar
features. No currently observed bucket is a reliability blocker; in
particular, the corpus produced zero hard failures, panics, or hangs.

## Parser v0.1 exit criteria

The parser is ready to unblock incremental namer work over its documented
source subset when all of these conditions hold:

1. On the pinned corpus, every file is attempted and worker/process failures,
   parser panics, and hangs are zero.
2. Clean parses do not regress below the issue #185 baseline (320 of 1,236),
   and recoverable diagnostics remain explicit rather than being hidden by the
   corpus harness.
3. The focused Scala 3.9 differential fixture suite passes without
   normalization changes that conceal syntax differences.
4. Every remaining `UnsupportedSyntax` first-failure bucket is classified as
   an intentional compatibility boundary, feature-gated form, or named
   follow-up; new buckets must receive the same treatment.
5. The supported/deferred grammar boundary and the current corpus result are
   documented. Full-corpus clean parsing is not a v0.1 requirement.

The measured final run passes these criteria: clean parses exceed the baseline
by 69 files, all 1,236 files complete, and there are no hard failures, panics,
or hangs. Recommendation: proceed with incremental namer work against parsed
source units in the supported subset. Do not treat this result as readiness to
parse and name arbitrary Scala 3.9 library/compiler source without diagnostics;
the 847 recoverable files and the follow-ups above remain a real compatibility
boundary.

Issue #222's deterministic sample-based root-cause investigation is documented in [parser-source-triage-3.9.0.md](parser-source-triage-3.9.0.md). It separates syntax families hidden by the generic first-diagnostic buckets and links the resulting implementation follow-ups.

## Corpus rerun after PR #247

Issue #252 reran the report at dotty-rs commit `222dd59` using the same
Scala 3.9.0 checkout revision (`777528f19a58e794c9954a42f433373472ec57f8`)
and the same sorted `library/src` + `compiler/src` manifest of 1,236 files.
The full run took 22.6 seconds on the measurement host. The new machine-readable
report is
[`parser-post-issue-252-scala3-3.9.0.json`](../tools/parser-corpus-report/parser-post-issue-252-scala3-3.9.0.json).

| Measure | v0.1 final (`1562374`) | After #247 (`222dd59`) | Change |
| --- | ---: | ---: | ---: |
| Files attempted | 1,236 | 1,236 | 0 |
| Clean parse | 389 (31.47%) | 455 (36.81%) | +66 (+5.34 pp) |
| Recoverable diagnostics | 847 (68.53%) | 781 (63.19%) | -66 (-5.34 pp) |
| Hard parser failures | 0 | 0 | 0 |
| Panics | 0 | 0 | 0 |
| Hangs | 0 | 0 | 0 |
| Scanner diagnostics | 17 | 17 | 0 |
| Scala oracle results / exceptions | 1,236 / 30 | 1,236 / 30 | unchanged |

| Diagnostic occurrences | v0.1 final | After #247 | Change |
| --- | ---: | ---: | ---: |
| `ExpectedExpression` | 37,442 | 35,103 | -2,339 |
| `ExpectedPattern` | 110 | 7 | -103 |
| `ExpectedToken` | 8,582 | 7,858 | -724 |
| `ExpectedType` | 2,423 | 2,529 | +106 |
| `UnexpectedToken` | 21,012 | 18,294 | -2,718 |
| `UnsupportedSyntax` | 6,114 | 4,189 | -1,925 |
| **Total** | **75,683** | **67,980** | **-7,703 (-10.2%)** |

The first-failure histogram also moved: `top-level expressions are not
supported in a compilation unit` disappeared (18 files in v0.1), while the
already-known Scala 2 wildcard-import `_` bucket grew from 44 to 61. Legacy
`implicit` parameter clauses grew from 25 to 31, and package objects from 18
to 19. The other named unsupported buckets were unchanged: compound template
self types (7), interleaved type parameter clauses (1), and unsupported enum
case syntax (5). These counts classify only each file's first parser
diagnostic; as the parser accepts more of a file, a later unsupported construct
can become its first reported failure. They are not per-feature regression
counts. Generic first-failure counts changed from 243 to 291 for
`ExpectedExpression`, 215 to 94 for `ExpectedToken`, 109 to 136 for
`ExpectedType`, 159 to 136 for `UnexpectedToken`, and 3 to 0 for
`ExpectedPattern`.

Overall, clean parses rose and total diagnostics fell, with no reliability
failures. This is an aggregate compatibility comparison, not proof that every
grammar change is correct: `ExpectedType` diagnostic occurrences increased by
106 and merit future targeted triage. The exact Scala/Rust fixture comparison
remains separately enforced by `tools/scala-parser-oracle/compare.sh`.

## Corpus rerun after PR #288

Issue #289 reran the same report at dotty-rs commit `5c25649` (main after
PR #288), against the unchanged Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8` and the same sorted 1,236-file
`library/src` + `compiler/src` manifest. The machine-readable report is
[`parser-post-issue-289-scala3-3.9.0.json`](../tools/parser-corpus-report/parser-post-issue-289-scala3-3.9.0.json).

| Measure | After #247 (`222dd59`) | After #288 (`5c25649`) | Change |
| --- | ---: | ---: | ---: |
| Files attempted | 1,236 | 1,236 | 0 |
| Clean parse | 455 (36.81%) | 550 (44.50%) | +95 (+7.69 pp) |
| Recoverable diagnostics | 781 (63.19%) | 686 (55.50%) | -95 (-7.69 pp) |
| Hard parser failures | 0 | 0 | 0 |
| Panics | 0 | 0 | 0 |
| Hangs | 0 | 0 | 0 |
| Scanner diagnostics | 17 | 17 | 0 |
| Scala oracle results / exceptions | 1,236 / 30 | 1,236 / 30 | unchanged |

| Diagnostic occurrences | After #247 | After #288 | Change |
| --- | ---: | ---: | ---: |
| `ExpectedExpression` | 35,103 | 19,183 | -15,920 |
| `ExpectedPattern` | 7 | 7 | 0 |
| `ExpectedToken` | 7,858 | 3,749 | -4,109 |
| `ExpectedType` | 2,529 | 1,553 | -976 |
| `UnexpectedToken` | 18,294 | 12,268 | -6,026 |
| `UnsupportedSyntax` | 4,189 | 7,404 | +3,215 |
| **Total** | **67,980** | **44,164** | **-23,816 (-35.0%)** |

Three named first-failure buckets disappeared: Scala 2 wildcard-import `_`
(61 files), legacy `implicit` parameter clauses (31), and unsupported package
objects (19). The clean-parse increase is 95 rather than 111 because first
failure buckets classify only the earliest diagnostic in each file; accepting
that construct can expose a later unsupported construct. Compound template
self types (7), interleaved type/term clauses (1), and unsupported enum-case
syntax (5) are unchanged.

The generic first-failure distribution is not uniformly lower: `ExpectedExpression`
rose from 291 to 310 files, `UnexpectedToken` from 136 to 160, and
`ExpectedPattern` from 0 to 1; `ExpectedToken` fell 94 to 90 and `ExpectedType`
fell 136 to 112. These are category counts, not per-file transition data, and
must not be read as independent syntax regressions. Likewise, the total
`UnsupportedSyntax` occurrence count increased while total diagnostic
occurrences fell; occurrence histograms are sensitive to how far parsing
progresses and are not a standalone coverage score. The stable file outcomes
show a net 95-file improvement with zero hard failures, panics, or hangs; the
new generic examples remain useful targets for future corpus triage.

## Corpus rerun after PR #308

Issue #309 reran the same report at dotty-rs commit `5ced54d` (main after
PR #308), against the unchanged Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8` and sorted 1,236-file
`library/src` + `compiler/src` corpus. The machine-readable report is
[`parser-post-issue-308-scala3-3.9.0.json`](../tools/parser-corpus-report/parser-post-issue-308-scala3-3.9.0.json).

| Measure | After #288 (`5c25649`) | After #308 (`5ced54d`) | Change |
| --- | ---: | ---: | ---: |
| Files attempted | 1,236 | 1,236 | 0 |
| Clean parse | 550 (44.50%) | 600 (48.54%) | +50 (+4.05 pp) |
| Recoverable diagnostics | 686 (55.50%) | 636 (51.46%) | -50 (-4.05 pp) |
| Hard parser failures | 0 | 0 | 0 |
| Panics | 0 | 0 | 0 |
| Hangs | 0 | 0 | 0 |
| Scanner diagnostics | 17 | 17 | 0 |
| Scala oracle results / exceptions | 1,236 / 30 | 1,236 / 30 | unchanged |

| Diagnostic occurrences | After #288 | After #308 | Change |
| --- | ---: | ---: | ---: |
| `ExpectedExpression` | 19,183 | 16,879 | -2,304 |
| `ExpectedPattern` | 7 | 7 | 0 |
| `ExpectedToken` | 3,749 | 2,511 | -1,238 |
| `ExpectedType` | 1,553 | 1,264 | -289 |
| `UnexpectedToken` | 12,268 | 10,418 | -1,850 |
| `UnsupportedSyntax` | 7,404 | 6,347 | -1,057 |
| **Total** | **44,164** | **37,426** | **-6,738 (-15.3%)** |

The largest remaining first-failure category is `ExpectedExpression` (378
files), followed by `ExpectedType` (118), `UnexpectedToken` (71), and
`ExpectedToken` (54). Named unsupported buckets are unchanged: compound
template self types (7 files), unsupported enum-case syntax (5), and
interleaved type/term parameter clauses (1). The increase in
`ExpectedExpression` first failures (310 to 378) is a category shift, not
evidence by itself of a regression: first-failure histograms do not identify
per-file transitions, while clean parses increased by 50 and
`UnexpectedToken`/`ExpectedToken` buckets fell by 89/36. Diagnostic occurrence
counts fell in every category except `ExpectedPattern`, which was flat.

Coverage now reaches 600 of 1,236 files (48.54%) with no hard failures,
panics, or hangs. The next useful investigation is to split the large generic
`ExpectedExpression` and `ExpectedType` buckets into concrete syntax families;
representative paths are evidence for triage, not diagnoses of those buckets.

## Corpus rerun after PR #321

Issue #323 reran the same measurement at `dotty-rs` commit `3cd9089` (main
after PR #321), against the unchanged Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8` and sorted 1,236-file
`library/src` + `compiler/src` corpus. The deterministic report is
[`parser-post-issue-323-scala3-3.9.0.json`](../tools/parser-corpus-report/parser-post-issue-323-scala3-3.9.0.json).

| Measure | After #308 (`5ced54d`) | After #321 (`3cd9089`) | Change |
| --- | ---: | ---: | ---: |
| Files attempted | 1,236 | 1,236 | 0 |
| Clean parse | 600 (48.54%) | 715 (57.85%) | +115 (+9.30 pp) |
| Recoverable diagnostics | 636 (51.46%) | 521 (42.15%) | -115 (-9.30 pp) |
| Hard parser failures | 0 | 0 | 0 |
| Panics / hangs | 0 / 0 | 0 / 0 | unchanged |
| Scanner diagnostics | 17 | 17 | 0 |
| Scala oracle results / exceptions | 1,236 / 30 | 1,236 / 30 | unchanged |

| Diagnostic occurrences | After #308 | After #321 | Change |
| --- | ---: | ---: | ---: |
| `ExpectedExpression` | 16,879 | 5,982 | -10,897 |
| `ExpectedPattern` | 7 | 5 | -2 |
| `ExpectedToken` | 2,511 | 1,966 | -545 |
| `ExpectedType` | 1,264 | 1,282 | +18 |
| `UnexpectedToken` | 10,418 | 6,048 | -4,370 |
| `UnsupportedSyntax` | 6,347 | 3,276 | -3,071 |
| **Total** | **37,426** | **18,559** | **-18,867 (-50.4%)** |

Clean coverage rose by 9.30 percentage points, and total diagnostic
occurrences roughly halved. The first-failure buckets shifted as follows:
`ExpectedExpression` fell from 378 to 176 files, while `ExpectedType` rose
from 118 to 146 and `UnexpectedToken` from 71 to 123; `ExpectedToken` moved
from 54 to 59. These are category-level counts, not per-file transition data,
so increases are not proof of regressions: as earlier failures are removed,
parsing can reach later syntax in the same files. At that point the largest buckets
are `ExpectedExpression` (176), `ExpectedType` (146), and `UnexpectedToken`
(123). Representative leads include backend builder files for expression
failures, `GenericSignatureVisitor.scala` / `SymbolUtils.scala` for type
failures, and `MainGenericCompiler.scala` / backend files for unexpected
tokens; these paths need reduction before attributing a syntax family.

The named unsupported first-failure buckets are compound template self types
(7 files), interleaved type/term parameter clauses (1), extension body syntax
(1), and enum-case syntax (6). Next, prioritize reducing samples from the
three largest generic buckets into concrete grammar families rather than
broadening grammar based only on these labels. Despite the remaining
diagnostics, the run had zero hard failures, panics, or hangs.

## Corpus rerun after PRs #340–#341

Issue #342 reran the same measurement at dotty-rs commit
`87b309d2` (main after PR #340, including PR #341), using the unchanged Scala
3.9.0 revision `777528f19a58e794c9954a42f433373472ec57f8` and the same sorted 1,236-file
`library/src` + `compiler/src` corpus. The deterministic report is
[`parser-post-issue-342-scala3-3.9.0.json`](../tools/parser-corpus-report/parser-post-issue-342-scala3-3.9.0.json).

| Measure | After #321 (`3cd9089`) | After #340–#341 (`87b309d2`) | Change |
| --- | ---: | ---: | ---: |
| Files attempted | 1,236 | 1,236 | 0 |
| Clean parse | 715 (57.85%) | 758 (61.33%) | +43 (+3.48 pp) |
| Recoverable diagnostics | 521 (42.15%) | 478 (38.67%) | -43 (-3.48 pp) |
| Hard parser failures | 0 | 0 | 0 |
| Panics / hangs | 0 / 0 | 0 / 0 | unchanged |
| Scanner diagnostics | 17 | 13 | -4 |
| Scala oracle results / exceptions | 1,236 / 30 | 1,236 / 30 | unchanged |

| Diagnostic occurrences | After #321 | After #340–#341 | Change |
| --- | ---: | ---: | ---: |
| `ExpectedExpression` | 5,982 | 4,496 | -1,486 |
| `ExpectedPattern` | 5 | 5 | 0 |
| `ExpectedToken` | 1,966 | 1,740 | -226 |
| `ExpectedType` | 1,282 | 1,273 | -9 |
| `UnexpectedToken` | 6,048 | 4,697 | -1,351 |
| `UnsupportedSyntax` | 3,276 | 2,522 | -754 |
| **Total** | **18,559** | **14,733** | **-3,826 (-20.6%)** |

The latest run adds 43 clean files and reduces recoverable files by the same
amount. Diagnostic occurrences decrease in every bucket except that
`ExpectedPattern` is unchanged. The largest raw reductions are
`ExpectedExpression` (-1,486) and `UnexpectedToken` (-1,351); these are
aggregate changes across all parser work since #321 and do not attribute the
gain to one individual PR. First-failure counts now stand at 163
`ExpectedExpression`, 150 `ExpectedType`, 92 `UnexpectedToken`, and 57
`ExpectedToken` files. The `ExpectedType` first-failure bucket increased by
four, which is not by itself evidence of regressions: this report does not
track per-file transitions, and removing earlier failures can expose later
ones. Hard failures, process failures, panics, and hangs remain zero; the
Scala oracle still returns all 1,236 results with 30 existing exceptions.

## Corpus rerun after capture-checking type support

Issue #348 reran the same corpus after PR #351 at dotty-rs revision
`d2f710011606a2a72bdae001c0579de91ee356e8`, using the unchanged Scala 3.9.0
revision `777528f19a58e794c9954a42f433373472ec57f8`. The deterministic report
is [`parser-post-issue-348-scala3-3.9.0.json`](../tools/parser-corpus-report/parser-post-issue-348-scala3-3.9.0.json).

| Measure | After #340–#341 (`87b309d2`) | After #351 (`d2f71001`) | Change |
| --- | ---: | ---: | ---: |
| Files attempted | 1,236 | 1,236 | 0 |
| Clean parse | 758 (61.33%) | 797 (64.48%) | +39 (+3.16 pp) |
| Recoverable diagnostics | 478 (38.67%) | 439 (35.52%) | -39 (-3.16 pp) |
| Hard failures / panics / hangs | 0 / 0 / 0 | 0 / 0 / 0 | unchanged |
| Scanner diagnostics | 13 | 13 | 0 |
| Scala oracle results / exceptions | 1,236 / 30 | 1,236 / 30 | unchanged |

| Diagnostic occurrences | After #340–#341 | After #351 | Change |
| --- | ---: | ---: | ---: |
| `ExpectedExpression` | 4,496 | 3,742 | -754 |
| `ExpectedPattern` | 5 | 5 | 0 |
| `ExpectedToken` | 1,740 | 1,083 | -657 |
| `ExpectedType` | 1,273 | 609 | -664 |
| `UnexpectedToken` | 4,697 | 3,300 | -1,397 |
| `UnsupportedSyntax` | 2,522 | 1,729 | -793 |
| **Total** | **14,733** | **10,468** | **-4,265 (-29.0%)** |

The report partitions files by the parser's effective capture-checking policy,
not by grepping for the import spelling. All 39 newly clean files are among
the 242 files where the parser enables capture checking; that cohort improved
from 109 to 148 clean files while the 994 disabled files remained at 649.
Diagnostic occurrences fell by 4,265 in the enabled cohort (8,090 to 3,825),
with no aggregate change in the disabled cohort.

Capture-marker triage further separates 113 raw `^` characters in the enabled
cohort from 109 actual lexer operator tokens and 103 files with capture-specific
AST nodes. Four raw-only hits are comments/documentation; five of the six
token-only cases are ordinary XOR/type-operator uses. One remaining file,
`library/src/scala/collection/package.scala`, contains capture suffix syntax,
but parsing stops earlier at its unsupported symbolic `object +:` declaration;
this is not evidence of a capture-production failure. Details and paths are in
the [capture corpus audit](parser-capture-checking-corpus-audit-3.9.0.md).

First-failure counts shifted: `ExpectedType` fell from 150 to 108 files,
`ExpectedToken` from 57 to 54, while `ExpectedExpression` rose from 163 to 171
and `UnexpectedToken` from 92 to 97. In the capture-enabled cohort itself,
`ExpectedType` fell from 83 to 41, while expression/token failures rose as
parsing reached later syntax. These histograms are not per-file transition
data. The next broad target remains the 148 `ExpectedExpression` first
failures in the capture-disabled cohort; for capture-enabled files, investigate
the remaining 41 `ExpectedType` first failures. The audit document records the
syntax-level caveats and concrete next-step interpretation.

## Corpus rerun after PRs #371, #373, #375, #377, and #378

Issue #379 reran the same source corpus at `dotty-rs` revision
`e6cc85b06082850ca52855bb510500f66535c200` (main after PR #378), with the
unchanged Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`. The deterministic report is
[`parser-post-issue-379-scala3-3.9.0.json`](../tools/parser-corpus-report/parser-post-issue-379-scala3-3.9.0.json).

| Measure | After #351 (`d2f71001`) | After #378 (`e6cc85b0`) | Change |
| --- | ---: | ---: | ---: |
| Files attempted | 1,236 | 1,236 | 0 |
| Clean parse | 797 (64.48%) | 908 (73.46%) | +111 (+8.98 pp) |
| Recoverable diagnostics | 439 (35.52%) | 328 (26.54%) | -111 (-8.98 pp) |
| Hard parser failures / panics / hangs | 0 / 0 / 0 | 0 / 0 / 0 | unchanged |
| Scanner diagnostics | 13 | 13 | 0 |
| Scala oracle results / exceptions | 1,236 / 30 | 1,236 / 30 | unchanged |

| Diagnostic occurrences | After #351 | After #378 | Change |
| --- | ---: | ---: | ---: |
| `ExpectedExpression` | 3,742 | 3,091 | -651 |
| `ExpectedPattern` | 5 | 5 | 0 |
| `ExpectedToken` | 1,083 | 1,071 | -12 |
| `ExpectedType` | 609 | 428 | -181 |
| `UnexpectedToken` | 3,300 | 2,837 | -463 |
| `UnsupportedSyntax` | 1,729 | 1,644 | -85 |
| **Total** | **10,468** | **9,076** | **-1,392 (-13.3%)** |

Both effective capture-checking cohorts improved: the disabled cohort went
from 649 to 720 clean files (+71 of 994), and the enabled cohort from 148 to
188 (+40 of 242). These are aggregate results across all changes since PR
#351; they do not attribute each newly clean file to a specific PR.

The largest reductions in first-failure files were `ExpectedType` (108 to 54),
`ExpectedExpression` (171 to 121), and `UnexpectedToken` (97 to 76).
`ExpectedToken` rose from 54 to 66, while the unsupported enum-case bucket rose
from 6 to 7 and a refinement-declaration unsupported bucket appeared with one
file. A first-failure histogram records only the earliest diagnostic per
file: these increases can reflect later failures becoming visible after an
earlier one was fixed, and are not by themselves evidence of regressions.
The largest current first-failure buckets are `ExpectedExpression` (121),
`UnexpectedToken` (76), `ExpectedToken` (66), and `ExpectedType` (54), with
their representative paths in the JSON report. Hard failures, process
failures, panics, and hangs remain zero.

## Corpus rerun after PR #393

Issue #399 reran the same 1,236-file source corpus at parser revision
`d0c2f517a25d08f31e1f90bb34a1935dc038c4aa` (main after PR #393), using the
unchanged Scala 3.9.0 source revision
`777528f19a58e794c9954a42f433373472ec57f8`. The deterministic report is
[`parser-post-issue-399-scala3-3.9.0.json`](../tools/parser-corpus-report/parser-post-issue-399-scala3-3.9.0.json).

| Measure | After #378 (`e6cc85b0`) | After #393 (`d0c2f517`) | Change |
| --- | ---: | ---: | ---: |
| Files attempted | 1,236 | 1,236 | 0 |
| Clean parse | 908 (73.46%) | 987 (79.85%) | +79 (+6.39 pp) |
| Recoverable diagnostics | 328 (26.54%) | 249 (20.15%) | -79 (-6.39 pp) |
| Hard parser failures / panics / hangs | 0 / 0 / 0 | 0 / 0 / 0 | unchanged |
| Scanner diagnostics | 13 | 13 | 0 |
| Scala oracle results / exceptions | 1,236 / 30 | 1,236 / 30 | unchanged |

Both capture-checking cohorts improved: the disabled cohort gained 58 clean
files (720 to 778 of 994), and the enabled cohort gained 21 (188 to 209 of
242). Diagnostic occurrences fell from 9,076 to 6,746 (-2,330, or 25.7%):

| Diagnostic occurrences | After #378 | After #393 | Change |
| --- | ---: | ---: | ---: |
| `ExpectedExpression` | 3,091 | 2,112 | -979 |
| `ExpectedPattern` | 5 | 5 | 0 |
| `ExpectedToken` | 1,071 | 989 | -82 |
| `ExpectedType` | 428 | 376 | -52 |
| `UnexpectedToken` | 2,837 | 1,932 | -905 |
| `UnsupportedSyntax` | 1,644 | 1,332 | -312 |
| **Total** | **9,076** | **6,746** | **-2,330 (-25.7%)** |

The first-failure histogram also shifted substantially: `ExpectedExpression`
fell from 121 to 48 files and `UnexpectedToken` from 76 to 56. `ExpectedToken`
rose from 66 to 77 and `ExpectedType` from 54 to 56; one
`UnsupportedSyntax` pattern-form bucket appeared, while the other unsupported
syntax buckets were unchanged. First-failure counts identify only the earliest
diagnostic in each file, so increases can mean parsing now reaches a later
failure rather than a regression. The largest current buckets are
`ExpectedToken` (77), `ExpectedType` and `UnexpectedToken` (56 each), and
`ExpectedExpression` (48); inspect their representative paths in the JSON
report before choosing follow-up grammar work. The aggregate clean-file gains
are real for this corpus, but this measurement alone does not attribute each
gain to a specific parser change. Hard failures, process failures, panics, and
hangs remain zero.

## Corpus rerun after PR #411

Issue #413 reran the same 1,236-file source corpus at parser revision
`ff5a6949ee09c5846ad585d8f1580a3e7d376827` (main after PR #411), using the
same Scala 3.9.0 source revision
`777528f19a58e794c9954a42f433373472ec57f8`. The deterministic report is
[`parser-post-issue-411-scala3-3.9.0.json`](../tools/parser-corpus-report/parser-post-issue-411-scala3-3.9.0.json).

| Measure | After #393 (`d0c2f517`) | After #411 (`ff5a6949`) | Change |
| --- | ---: | ---: | ---: |
| Files attempted | 1,236 | 1,236 | 0 |
| Clean parse | 987 (79.85%) | 1,040 (84.14%) | +53 (+4.29 pp) |
| Recoverable diagnostics | 249 (20.15%) | 196 (15.86%) | -53 (-4.29 pp) |
| Hard parser failures / panics / hangs | 0 / 0 / 0 | 0 / 0 / 0 | unchanged |
| Scanner diagnostics | 13 | 13 | 0 |
| Scala oracle results / exceptions | 1,236 / 30 | 1,236 / 30 | unchanged |

The capture-checking-disabled cohort gained 50 clean files (778 to 828 of
994); the enabled cohort gained 3 (209 to 212 of 242). Diagnostic occurrences
fell from 6,746 to 5,384 (-1,362, or 20.2%):

| Diagnostic occurrences | After #393 | After #411 | Change |
| --- | ---: | ---: | ---: |
| `ExpectedExpression` | 2,112 | 1,777 | -335 |
| `ExpectedPattern` | 5 | 5 | 0 |
| `ExpectedToken` | 989 | 556 | -433 |
| `ExpectedType` | 376 | 269 | -107 |
| `UnexpectedToken` | 1,932 | 1,527 | -405 |
| `UnsupportedSyntax` | 1,332 | 1,250 | -82 |
| **Total** | **6,746** | **5,384** | **-1,362 (-20.2%)** |

The largest first-failure buckets are now `ExpectedToken` (85 files),
`ExpectedExpression` (37), `ExpectedType` (34), and `UnexpectedToken` (27).
Compared with the previous report, the first three expression/type/token
counts moved by -11, -22, and +8 respectively, while `UnexpectedToken` fell
by 29. These counts describe only each file's first diagnostic; newly exposed
later failures can increase a bucket even when overall parsing improves. The
largest diagnostic-occurrence buckets are `ExpectedExpression` (1,777),
`UnexpectedToken` (1,527), and `UnsupportedSyntax` (1,250). Representative
paths and capture-checking cohort details are in the JSON report.

This is an aggregate comparison from the post-#393 report through the main tip
after #411, including the intervening merged parser and typer changes; it does
not attribute the 53 newly clean files to PR #411 alone. Hard failures,
process failures, panics, and hangs remain zero.

## Corpus rerun after PR #435

Issue #436 reran the same 1,236-file corpus at parser revision
`45191bc6a6336f045d6e8fa35e799785134dec42` (main after PRs #433–#435),
using Scala 3.9.0 source revision
`777528f19a58e794c9954a42f433373472ec57f8`. The report is
[`parser-post-issue-436-scala3-3.9.0.json`](../tools/parser-corpus-report/parser-post-issue-436-scala3-3.9.0.json).

| Measure | After #411 (`ff5a6949`) | After #435 (`45191bc6`) | Change |
| --- | ---: | ---: | ---: |
| Files attempted | 1,236 | 1,236 | 0 |
| Clean parse | 1,040 (84.14%) | 1,074 (86.89%) | +34 (+2.75 pp) |
| Recoverable diagnostics | 196 (15.86%) | 162 (13.11%) | -34 (-2.75 pp) |
| Hard parser failures / panics / hangs | 0 / 0 / 0 | 0 / 0 / 0 | unchanged |
| Scanner diagnostics | 13 | 13 | 0 |
| Scala oracle results / exceptions | 1,236 / 30 | 1,236 / 30 | unchanged |

The capture-checking-disabled cohort increased from 828 to 853 clean files
(+25 of 994); the enabled cohort increased from 212 to 221 (+9 of 242).
Diagnostic occurrences fell from 5,384 to 2,488 (-2,896, or 53.8%):

| Diagnostic occurrences | After #411 | After #435 | Change |
| --- | ---: | ---: | ---: |
| `ExpectedExpression` | 1,777 | 804 | -973 |
| `ExpectedPattern` | 5 | 5 | 0 |
| `ExpectedToken` | 556 | 355 | -201 |
| `ExpectedType` | 269 | 236 | -33 |
| `UnexpectedToken` | 1,527 | 580 | -947 |
| `UnsupportedSyntax` | 1,250 | 508 | -742 |
| **Total** | **5,384** | **2,488** | **-2,896 (-53.8%)** |

The largest first-failure buckets are `ExpectedToken` (51 files),
`ExpectedExpression` (42), `ExpectedType` (29), and `UnexpectedToken` (23).
Relative to #411 these changed by -34, +5, -5, and -4. First-failure counts
record only the earliest diagnostic per file, so an increase can reflect
parsing farther before encountering a later unsupported construct. The largest
diagnostic-occurrence buckets are now `ExpectedExpression` (804),
`UnexpectedToken` (580), and `UnsupportedSyntax` (508).

This is an aggregate comparison from the post-#411 baseline through PRs
#433–#435; it does not attribute the 34 additional clean files or diagnostic
reduction to any one PR. The intervening changes include parser hardening,
assignment parsing, and constructor parameter modifiers. The three
constructor-modifier corpus examples named in #420 parse without diagnostics.
Hard failures, process failures, panics, and hangs remain zero.

## Corpus rerun after PR #477

Issue #479 reran the same 1,236-file corpus at parser revision
`46bab6e40f78f67e8ff2f39fe6864574b78adf66` (main after PR #477), using Scala
3.9.0 source revision `777528f19a58e794c9954a42f433373472ec57f8`. The report is
[`parser-post-issue-477-scala3-3.9.0.json`](../tools/parser-corpus-report/parser-post-issue-477-scala3-3.9.0.json).

| Measure | After #435 (`45191bc6`) | After #477 (`46bab6e4`) | Change |
| --- | ---: | ---: | ---: |
| Files attempted | 1,236 | 1,236 | 0 |
| Clean parse | 1,074 (86.89%) | 1,113 (90.05%) | +39 (+3.16 pp) |
| Recoverable diagnostics | 162 (13.11%) | 123 (9.95%) | -39 (-3.16 pp) |
| Hard parser failures / process failures / panics / hangs | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | unchanged |
| Scanner diagnostics | 13 | 13 | 0 |
| Scala oracle results / exceptions | 1,236 / 30 | 1,236 / 30 | unchanged |

The capture-checking-disabled cohort increased from 853 to 882 clean files
(+29 of 994); the enabled cohort increased from 221 to 231 (+10 of 242).
Diagnostic occurrences fell from 2,488 to 1,738 (-750, or 30.1%):

| Diagnostic occurrences | After #435 | After #477 | Change |
| --- | ---: | ---: | ---: |
| `ExpectedExpression` | 804 | 675 | -129 |
| `ExpectedPattern` | 5 | 5 | 0 |
| `ExpectedToken` | 355 | 232 | -123 |
| `ExpectedType` | 236 | 7 | -229 |
| `UnexpectedToken` | 580 | 413 | -167 |
| `UnsupportedSyntax` | 508 | 406 | -102 |
| **Total** | **2,488** | **1,738** | **-750 (-30.1%)** |

The largest first-failure buckets are now `ExpectedToken` (41 files),
`ExpectedExpression` (38), `UnexpectedToken` (26), and `ExpectedType` (4).
Against #435, these changed by -10, -4, +3, and -25 respectively. The small
increase in `UnexpectedToken` is a first-diagnostic-only shift and does not
outweigh the net 39 additional clean files; first-failure counts can move as
parsing proceeds farther. The largest diagnostic-occurrence buckets are
`ExpectedExpression` (675), `UnexpectedToken` (413), and `UnsupportedSyntax`
(406). Oracle exceptions remain 30, so those files' Scala behavior is still
not represented by successful oracle trees.

This is an aggregate comparison from the post-#435 baseline through the main
tip after PR #477; it does not attribute the 39 additional clean files or the
diagnostic reduction to PR #477 alone. The corpus run took about 15.7 seconds
on the measurement machine. Hard failures, process failures, panics, and hangs
remain zero.

## Corpus rerun after PR #503

Issue #506 reran the same 1,236-file corpus at parser revision
`4c6b1e2e53aa4bd988c11e7352a506758133b990` (main after PR #503), using Scala
3.9.0 source revision `777528f19a58e794c9954a42f433373472ec57f8`. The report is
[`parser-post-issue-506-scala3-3.9.0.json`](../tools/parser-corpus-report/parser-post-issue-506-scala3-3.9.0.json).

| Measure | After #477 (`46bab6e4`) | After #503 (`4c6b1e2e`) | Change |
| --- | ---: | ---: | ---: |
| Files attempted | 1,236 | 1,236 | 0 |
| Clean parse | 1,113 (90.05%) | 1,127 (91.18%) | +14 (+1.13 pp) |
| Recoverable diagnostics | 123 (9.95%) | 109 (8.82%) | -14 (-1.13 pp) |
| Hard parser failures / process failures / panics / hangs | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | unchanged |
| Scanner diagnostics | 13 | 13 | 0 |
| Scala oracle results / exceptions | 1,236 / 30 | 1,236 / 0 | 0 / -30 |

The clean-parse gain splits into +13 files in the capture-checking-disabled
cohort (882 → 895 of 994) and +1 in the enabled cohort (231 → 232 of 242).
Diagnostic occurrences fell from 1,738 to 1,471 (-267, or 15.4%):

| Diagnostic occurrences | After #477 | After #503 | Change |
| --- | ---: | ---: | ---: |
| `ExpectedExpression` | 675 | 577 | -98 |
| `ExpectedPattern` | 5 | 5 | 0 |
| `ExpectedToken` | 232 | 185 | -47 |
| `ExpectedType` | 7 | 7 | 0 |
| `UnexpectedToken` | 413 | 336 | -77 |
| `UnsupportedSyntax` | 406 | 361 | -45 |
| **Total** | **1,738** | **1,471** | **-267 (-15.4%)** |

The largest first-failure buckets are now `ExpectedExpression` (35 files),
`ExpectedToken` (28), `UnexpectedToken` (28), `ExpectedType` (4), and
`ExpectedPattern` (2). Relative to #477, the first-failure counts changed by
-3, -13, +2, 0, and 0 respectively. The two-file increase in
`UnexpectedToken` is a first-diagnostic shift; the net result is 14 more clean
files and fewer total diagnostic occurrences.

The Scala oracle failure count fell from 30 to zero after the oracle-context
initialization change in PR #483; this is a harness reliability improvement,
not a 30-file Rust parser coverage gain. The source inventory and Rust
hard-failure counts are unchanged. The corpus confirms that
`compiler/src/dotty/tools/dotc/core/NameOps.scala` now parses without
diagnostics. `compiler/src/dotty/tools/dotc/cc/SafeRefs.scala` still reports an
`ExpectedToken` and an `UnexpectedToken`: its continuation is `&& !isSafe(...)`,
and the scanner's leading-infix lookahead currently does not consider a prefix
operator at the start of the right operand. The existing comment-continuation
fixture uses an identifier operand and does not cover this remaining case.

This is an aggregate before/after measurement across the changes since PR #477;
it does not attribute all 14 clean parses or the diagnostic reduction to PR
#503 alone. Hard failures, process failures, panics, and hangs remain zero.

## Corpus rerun after PR #510

Issue #516 reran the same pinned Scala 3.9.0 corpus at parser merge revision `c08a2f4880fa687167f288fc50bbc5e94a98de6a` (PR #510). The immutable report is [`parser-post-issue-516-scala3-3.9.0.json`](../tools/parser-corpus-report/parser-post-issue-516-scala3-3.9.0.json). Scala source revision `777528f19a58e794c9954a42f433373472ec57f8` and the sorted 1,236-file inventory are unchanged.

| Measure | After #506 / PR #503 | After #510 | Change |
| --- | ---: | ---: | ---: |
| Files attempted | 1,236 | 1,236 | 0 |
| Clean parses | 1,127 (91.18%) | 1,127 (91.18%) | 0 |
| Recoverable files | 109 (8.82%) | 109 (8.82%) | 0 |
| Hard failures / process failures / panics / hangs | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | unchanged |
| Scanner diagnostics | 13 | 13 | 0 |
| Scala oracle files / failures | 1,236 / 0 | 1,236 / 0 | unchanged |

Diagnostic occurrences increased from 1,471 to 1,557 (+86, or 5.85%), even though the first-failure histogram is unchanged. The change is concentrated in `ExpectedExpression` (+51), `UnsupportedSyntax` (+25), and `UnexpectedToken` (+10); `ExpectedPattern`, `ExpectedToken`, and `ExpectedType` are unchanged. Capture-checking cohorts are also unchanged: disabled 895/994 clean and enabled 232/242 clean.

`compiler/src/dotty/tools/dotc/cc/SafeRefs.scala`, previously reported with `ExpectedToken` and `UnexpectedToken`, is now clean in the full corpus and in a focused single-file run. Since the overall clean count remains unchanged, that gain is offset by at least one newly diagnostic file. The aggregate report does not retain per-file outcomes, so this measurement alone cannot identify it. Treat the result as a targeted fix with no net corpus-coverage gain yet; investigate the diagnostic-occurrence increase and offsetting file change before attributing broader improvement.

## Corpus rerun after PR #538

Issue #541 reran the same corpus at parser revision
`c6061d1f4235dfb48514a0ac4e90c17df51f1cbc` (main after PR #538). The immutable
report is [`parser-post-issue-541-scala3-3.9.0.json`](../tools/parser-corpus-report/parser-post-issue-541-scala3-3.9.0.json).
The pinned Scala revision and sorted 1,236-file inventory are unchanged.

| Measure | After #510 / PR #516 | After #538 | Change |
| --- | ---: | ---: | ---: |
| Files attempted | 1,236 | 1,236 | 0 |
| Clean parses | 1,127 (91.18%) | 1,135 (91.83%) | +8 (+0.65 pp) |
| Recoverable files | 109 (8.82%) | 101 (8.17%) | -8 |
| Hard failures / process failures / panics / hangs | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | unchanged |
| Scanner diagnostics | 13 | 13 | 0 |
| Scala oracle files / failures | 1,236 / 0 | 1,236 / 0 | unchanged |

Diagnostic occurrences fell from 1,557 to 1,427 (-130, or 8.35%). The category
changes were `ExpectedExpression` 628 → 646 (+18), `ExpectedPattern` 5 → 5
(0), `ExpectedToken` 185 → 179 (-6), `ExpectedType` 7 → 7 (0),
`UnexpectedToken` 346 → 339 (-7), and `UnsupportedSyntax` 386 → 251 (-135).
Thus the net reduction is driven by unsupported syntax, despite an increase in
expression diagnostics. The first-failure buckets also move slightly:
`ExpectedExpression` 35 → 38, `ExpectedPattern` 2 → 2, `ExpectedToken` 28 → 24,
`ExpectedType` 4 → 5, and `UnexpectedToken` 28 → 29. Clean parses rose from
895/994 to 903/994 in the capture-checking-disabled cohort and stayed at
232/242 in the enabled cohort.

This compares a range of changes between PR #510 and PR #538, not the isolated
effect of one parser increment. It is a useful coverage gain, but not a blanket
correctness claim: the rise in `ExpectedExpression` and the new first-failure
examples should be investigated separately. The baseline has zero hard parser
failures, worker failures, panics, or hangs, and the Scala oracle completed all
files without exceptions.

Recreate the report from the pinned checkout with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-541-scala3-3.9.0.json
```

## Corpus rerun after PR #561

Issue #562 reran the same corpus at parser revision
`49da0fa117ad406e4a97c785e73e2fb9349107b4` (main after PR #561). The immutable
report is [`parser-post-issue-562-scala3-3.9.0.json`](../tools/parser-corpus-report/parser-post-issue-562-scala3-3.9.0.json).
The pinned Scala 3.9.0 source revision and sorted 1,236-file inventory are
unchanged.

| Measure | After #538 / #541 | After #561 | Change |
| --- | ---: | ---: | ---: |
| Files attempted | 1,236 | 1,236 | 0 |
| Clean parses | 1,135 (91.83%) | 1,142 (92.39%) | +7 (+0.57 pp) |
| Recoverable files | 101 (8.17%) | 94 (7.61%) | -7 |
| Hard failures / process failures / panics / hangs | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | unchanged |
| Scanner diagnostics | 13 | 13 | 0 |
| Scala oracle files / failures | 1,236 / 0 | 1,236 / 0 | unchanged |

Diagnostic occurrences fell from 1,427 to 1,402 (-25, or 1.75%). By kind,
`ExpectedExpression` changed 646 → 641 (-5), `ExpectedPattern` stayed at 5,
`ExpectedToken` changed 179 → 174 (-5), `ExpectedType` changed 7 → 1 (-6),
`UnexpectedToken` changed 339 → 332 (-7), and `UnsupportedSyntax` changed 251
→ 249 (-2). First-failure counts decreased for `ExpectedToken` (24 → 22) and
`ExpectedType` (5 → 0); the other first-failure buckets were unchanged. The
largest remaining first-failure groups are `ExpectedExpression` (38),
`UnexpectedToken` (29), and `ExpectedToken` (22).

The capture-checking-disabled cohort gained two clean files (903/994 →
905/994); the enabled cohort gained five (232/242 → 237/242). These are
aggregate changes across the parser merges since the prior measurement, not
isolated effects attributable to PR #561. The result is a modest coverage
gain with unchanged scanner diagnostics and no reliability failures; it does
not establish that every newly clean parse matches Dotty semantically.

Recreate the report from the pinned checkout with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-562-scala3-3.9.0.json
```
