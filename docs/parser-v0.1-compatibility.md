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
parsing can reach later syntax in the same files. The current largest buckets
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
