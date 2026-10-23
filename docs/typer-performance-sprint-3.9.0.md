# Typer performance sprint closure evidence (Scala 3.9.0)

Issue: [#1487](https://github.com/scytrowski/dotty-rs/issues/1487)

## Frozen comparison inputs

The sprint comparison starts at `4025adf3092cfe4a0e51597acdfc91fcf5359746`
(the #1483 audit baseline before the optimization increments) and ends at
`c51a9ab2b1365c16b2e5dc7d7963cd82842dc0e9` (the current source snapshot after
#1486 and #1498). The latter contains no changes to the Typer audit code after
the measured #1486 implementation. The source oracle is Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`, artifacts are Scala 3.9.0, and the
profiles use Temurin JDK 21.0.8 with Cargo/test concurrency set to one where
recorded.

The four-file profile cohort used by #1483, #1484 and #1486 is the frozen
selection in `docs/typer-hot-path-profile-manifest-3.9.0.tsv`: `GenericSignatures.scala`,
`tpd.scala`, `Types.scala` and `compileTimeOnly.scala`, with at most two root
methods and one match probe per file. The bounded #1485 rollback profile is a
different two-file, 39-method selection and is kept separate below. Historical
#1342 and incomplete #1417 full-audit times are not used as paired measurements.

## Paired results

| Increment | Selection / samples | Before → after | Observed change | Interpretation |
| --- | --- | --- | --- | --- |
| #1484 borrowed observation sorting | 4 files, 6 root methods, 3 match probes; 2 samples per mode | Cargo wall 12,180.5 → 12,689 ms; user CPU 10.765 → 11.195 s; RSS 114,307,072 → 114,860,032 B | Wall +4.18%; RSS +552,960 B | No end-to-end speed or RSS improvement. The identified sorting copies fall by an estimated 4,385,332 requested transient bytes/sample; this is a component calculation, not total allocations. |
| #1484 reuse prefix profile | Same 4-file cohort; 2 baseline and 3 after samples | Cargo wall 12,180.5 → 12,250 ms; user CPU 10.765 → 10.80 s; RSS 114,307,072 → 115,433,472 B | Wall +0.57%; RSS +1,126,400 B | No material whole-process change. The exact request-shape evidence shows 6,530 fewer redundant profile renderings/sample. |
| #1484 defer member-shape rendering | Same 4-file cohort; 3 samples before and after | Cargo wall 12,250 → 12,048.3 ms; user CPU 10.80 → 10.73 s; RSS 115,433,472 → 115,671,040 B | Wall −1.65%; RSS +237,568 B | Small wall-time reduction within observed run variation; no established end-to-end gain. Estimated identified transient payload reduction is another 2,453,250 requested bytes/sample. |
| #1486 postprocess indexes | Same 4-file cohort; run 2 from each two-run profile | Audit wall 12,098 → 12,255 ms; Cargo wall 12,178 → 12,387 ms; RSS 113.8 → 115.2 MiB | Audit +1.30%; Cargo +1.72%; RSS +1.4 MiB | No end-to-end speedup detected. Reported postprocess time moved from 13 to 6 ms, a 7 ms directional difference at millisecond granularity. Host recorded for this profile: AMD Ryzen 9 7940HS. |
| #1485 rollback bookkeeping | Different two-file, 39-method selection; one before/after sample | Profile wall 17,047 → 16,790 ms; counts and normalized report unchanged | Wall −1.51% | One sample is insufficient to claim a speedup. In the measured corpus, each rollback row was already scanned once. The synthetic repeated-parent case at 10,000 rows reduced visits 505,000 → 10,000 and reported 3,374 → 440 ns/sample; this is a narrow synthetic result, not corpus runtime. |

The #1483 observer A/B/C experiment is attribution, not an optimization comparison:
the same measured code averaged 11,981.5 ms (semantic only), 12,030 ms (cheap
counters) and 12,180.5 ms (full observer). Two repetitions showed full observer
wall overhead of 1.66% relative to semantic only, with 10.24 MB higher sampled
peak RSS. The observer itself accounted for about 0.58% of a sample; delegated
`member_lookup` dominated at approximately 11.72 s/sample. Subphase timings
overlap resolver windows and are not additive. The source report
[`typer-auditresolver-overhead-3.9.0.md`](typer-auditresolver-overhead-3.9.0.md)
contains the full A/B/C evidence and limits.

Machine-readable values and missing-data labels are in
[`typer-performance-sprint-3.9.0.tsv`](typer-performance-sprint-3.9.0.tsv).
Detailed per-increment evidence remains in the linked #1484–#1486 profile notes.
The persisted #1483/#1484/#1485 notes do not all record a host model, so this
report does not claim that every historical pair used identical hardware. Their
selection, JDK, artifacts and serial settings are retained where available.
This limits cross-issue precision; the per-issue before/after values remain the
reported measurements, not a new benchmark reconstructed from rounded prose.

## Focused regression gate

The automated PR gate runs only the checked-in three-file smoke manifest twice
with pinned Scala 3.9.0 inputs and compares the full normalized focused output
against `typer-focused-smoke-baseline-3.9.0.txt`. The baseline covers 3 files,
31 method identities and resolver provenance rows. It reports
**FOCUSED / PARTIAL**, currently 0/31 typed methods; that is a diagnostic
regression baseline, not evidence of Scala compiler compatibility or full
corpus success. Timings and process RSS are emitted as artifacts for each
sample; they are informational and have no flaky wall-clock pass threshold.
The job uses one Cargo job and one Rust test thread and has a 30-minute hard
timeout. It never invokes the full-corpus audit.

The clean local gate reported 86 package requests, 88 member requests and 31
first-blocker rows. Its two per-run timing rows, normalized report hash and
unavailable local RSS are retained in
[`typer-focused-audit-gate-3.9.0.tsv`](typer-focused-audit-gate-3.9.0.tsv).

The clean-target local verification produced two byte-identical reports with
SHA-256 `e8ce98a75282a4783cffb816ea91a1818e2105a6f680dbae14e6e386036f28e2`,
matching the checked-in normalized baseline. With the test binary already
built, its samples were 3,167 and 2,985 ms wall, including 250/72 ms classpath
setup and 2,911/2,908 ms source processing. This local environment does not
provide `/usr/bin/time`, so peak RSS is unavailable for these samples. An
attempt using Cargo's target directory shared with another worktree produced a
larger, differing report and 157,759–174,527 ms source-processing figures;
those values are excluded because that binary's build provenance is
uncertain. Focused CI records per-sample wall/phase timing and process-tree RSS
without enforcing a single-run latency threshold.

## Full-corpus DoD status: local run completed; gates fail

At the user's request, the full audit was run locally against the pinned Scala
3.9.0 artifacts with one Cargo job and one Rust test thread. Two sequential
passes each covered 1,236 files and 3,297 local methods. They took 1,739.42 s
and 1,733.53 s (58:00.750 total); the normalized reports were byte-identical
with SHA-256
`0794c286758fc84ac7f3eea618ffd4455b4c0aa867012fe0bacdeb88d09db645`. Peak RSS
was unavailable because `/usr/bin/time` is not installed. The raw report is
678 MB and remains in the local temporary audit directory; it is not checked
into the repository.

The corpus gate correctly rejected this run. It found 22 previously successful
type-target keys missing from the current success set, and the maximum eager
completed-class fanout is 29, above the required 16. The p95 dependency depth
is 0 (limit 6), with maximum depth 8 and 63 distinct classes touched by the
worst request. External class-symbol successes rose from 358 to 18,138, but
that aggregate increase does not erase the 22 missing target keys. In
particular, `dotty.tools.io.AbstractFile` has current type-request errors.
These results are evidence of incomplete DoD, not a passing audit.

The audit runner stopped before assembling its final Markdown and cohort
sidecars because the gate failed. The comparison tool now reads report sections
incrementally instead of materializing millions of lines; on the retained raw
report, the gate comparison took 1.96 s and reported the same failures.

The opt-in frozen TASTy profile also ran locally. Its stale assertions were
updated to the observed result: 7 unresolved roots / 17 weighted observations,
34 recovered roots / 362 observations, and no newly unresolved roots. The
reconciliation script passed; it attributes 252 current source observations to
10 roots, including 5 roots absent from the frozen set. This does not resolve
the two corpus-gate failures above.

| DoD item | Status | Evidence / blocker |
| --- | --- | --- |
| Paired measurements and sprint comparison | Partial | Existing bounded before/after profiles collated above; historical host metadata is incomplete for some pairs. |
| Deterministic focused CI gate and tooling tests | Implemented; CI pending | New workflow verifies pinned inputs, two-pass determinism, normalized baseline, timing/RSS artifacts and audit-tool tests. |
| Two complete full-corpus passes and deterministic output | **Local evidence complete** | 1,236 files / 3,297 methods per pass; hashes match. Temporary raw reports are not repository artifacts. |
| No lost successful targets and completed-class fanout <=16 | **Failed** | 22 missing successful type-target keys; maximum fanout 29. |
| Current TASTy parent profile and provenance reconciliation | **Local evidence complete** | Profile and reconciliation passed after refreshing stale frozen assertions. |
| Measured residual hotspot and bounded next step | Partial | The full-corpus evidence now identifies eager hierarchy fanout (`List` reaches 29 completed classes) and missing successful type targets. Resolve those semantic causes before proposing another audit-speed optimization. |

### Reproduction

The focused CI command is:

```sh
tools/typer-classpath-corpus-audit/run \
  --focused-manifest docs/typer-focused-smoke-manifest-3.9.0.tsv \
  --verify-determinism <runner-temp>/focused-report.md
```

The full-corpus command remains confined to the manual hosted workflow:

```sh
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
  tools/typer-classpath-corpus-audit/run --verify-determinism \
  <runner-output-path>/typer-classpath-corpus-audit-performance.md
```
