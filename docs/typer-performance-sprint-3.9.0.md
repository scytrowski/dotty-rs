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

## Full-corpus DoD status: local comparison complete; hosted gate intentionally pending

At the user's request, the full audit was run locally against the pinned Scala
3.9.0 artifacts with one Cargo job and one Rust test thread. Two sequential
passes each covered 1,236 files and 3,297 local methods. They took 1,739.42 s
and 1,733.53 s (58:00.750 total); the normalized reports were byte-identical
with SHA-256
`0794c286758fc84ac7f3eea618ffd4455b4c0aa867012fe0bacdeb88d09db645`. Peak RSS
was unavailable because `/usr/bin/time` is not installed. The raw report is
678 MB and remains in the local temporary audit directory; it is not checked
into the repository.

The checked-in report at `4025adf` was stale, so I reran the pinned audit
locally from a detached worktree at that exact source SHA. Two complete passes
processed 1,236 files / 3,297 local methods. The two-pass command took 3,742 s
(62:22); the independently repeated second pass took 1,884.58 s (31:24.58).
The first command exited while extracting its report because the temporary
filesystem filled, after both audit tests had completed. I recovered the first
report, reran the second pass with compressed logging, and compared the
normalized semantic output. The exact normalized comparison (excluding
interleaved progress diagnostics, canonicalizing JAR-origin paths, and omitting
the rollback-counter line added by the newer harness) has 2,312,215 lines and
SHA-256 `08e0c99ae89c97c67ecca4ccef52a310012bf2b17d8993cc228b3be5539ee603` at
both sprint start and current. The only report-schema difference is the newer
rollback telemetry; it does not affect semantic outcomes or the classpath
safety measurements.

The fresh baseline comparison confirms 157 successful target keys at both
sprint start and current (157 retained, none missing), 18,138 external
class-symbol successes at both points, 0 typed local methods, and identical
first-blocker outcomes. Dependency p95 is 0 at both points (limit 6), maximum
depth is 8 at both points, and maximum completed-class fanout is 29 at both
points (policy limit 16). Thus the fanout limit is exceeded, but the excess
predates this sprint and was not introduced by its changes. The 29 completions
are four
`scala.collection.immutable.List` requests from `ListMap.addOne`,
`ListBuffer.removeAfter`, and `TrieMap.LNode` insert/remove methods; each
touches 63 distinct classes. This current fanout exceeds the configured safety
limit, but the fresh baseline confirms that the same fanout existed at sprint
start. The previously reported 77 missing keys were an artifact of comparing
against the stale 2026-10-17 report.

The source code under `crates/dotty-typer/src`,
`crates/dotty-classloader/src`, and `crates/dotty-core/src` is unchanged between
the sprint-start and current audit revisions. The fresh local run confirms that
the previously observed 77 missing keys came from comparing against the stale
2026-10-17 report, not from this sprint. Keep the fanout policy violation
visible as a pre-existing condition; it is not evidence of a sprint regression.

The original audit runner stopped before assembling its final Markdown and
cohort sidecars because the gate failed. It now keeps collecting evidence,
assembles the report and writes sidecars, then exits nonzero to preserve the
failed gate status. On the retained raw report, comparison against the
sprint-start snapshot took 13.5 s and emitted 120 frozen-cohort rows, including
the current first blocker and resolver provenance for each row. The raw report
is 678 MB, so it remains a local audit artifact rather than a checked-in file.
The compact metrics and cohort reconciliation are checked in as
[`typer-performance-full-audit-3.9.0.tsv`](typer-performance-full-audit-3.9.0.tsv)
and [`typer-performance-sprint-cohorts-3.9.0.tsv`](typer-performance-sprint-cohorts-3.9.0.tsv).
The full target-by-target outcome movement was regenerated from the retained
exact-source baseline and current reports. It contains 5,553 normalized request
targets with no outcome changes between those two snapshots. The baseline key
set SHA-256 is `b11bb1a525efa8dd2ab5dd725143fcd7a67acf7088d538aed4124d651306b1eb`;
a consistency test checks it against the compact metrics and movement sidecars.
The full-audit runner now uses the compact frozen target-key and class-success
baseline at
[`typer-performance-sprint-start-gate-3.9.0.tsv`](typer-performance-sprint-start-gate-3.9.0.tsv);
#976 remains the source of historical cohort row identities only.

The opt-in frozen TASTy profile also ran locally. Its stale assertions were
updated to the observed result: 7 unresolved roots / 17 weighted observations,
34 recovered roots / 362 observations, and no newly unresolved roots. The
reconciliation script passed; it attributes 252 current source observations to
10 roots, including 5 roots absent from the frozen set. The absolute fanout
policy violation and hosted-run requirement remain outstanding.

| DoD item | Status | Evidence / blocker |
| --- | --- | --- |
| Paired measurements and sprint comparison | Partial | Existing bounded before/after profiles collated above; historical host metadata is incomplete for some pairs. |
| Deterministic focused CI gate and tooling tests | Implemented; CI pending | New workflow verifies pinned inputs, two-pass determinism, normalized baseline, timing/RSS artifacts and audit-tool tests. |
| Two complete full-corpus passes and deterministic output | **Local evidence complete; hosted run pending by user direction** | Exact sprint-start SHA: 1,236 files / 3,297 methods per pass. Normalized semantic reports match; raw local report artifacts remain under `/tmp`. |
| No lost successful targets | **Pass locally** | Fresh baseline and current report contain the same 157 successful target keys; 157 retained, 0 missing. |
| Current completed-class fanout <=16 | **Pre-existing policy violation** | Fanout is 29 at both sprint start and current; limit 16. The sprint did not introduce the excess. |
| Current TASTy parent profile and provenance reconciliation | **Local evidence complete** | Profile and reconciliation passed after refreshing stale frozen assertions. |
| Measured residual hotspot and bounded next step | Partial | `List` fanout 29 / 63 classes is unchanged from sprint start. A separate classloader follow-up may address the pre-existing policy violation; it is outside this sprint comparison. |

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

The current raw normalized report was generated at dotty-rs commit
`2f67951133`; the baseline was generated from exact sprint-start SHA
`4025adf3092cfe4a0e51597acdfc91fcf5359746`. The local run establishes the
sprint comparison and target-retention result. The hosted/manual workflow was
not run, per the user's instruction to keep this audit local. The issue's
hosted-run DoD item therefore remains explicitly pending; local evidence does
not claim to satisfy it. Peak RSS also remains unavailable locally because
`/usr/bin/time` is absent.
