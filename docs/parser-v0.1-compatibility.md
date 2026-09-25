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
