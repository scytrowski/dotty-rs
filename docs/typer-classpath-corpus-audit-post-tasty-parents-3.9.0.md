# Post-TASTy-parent Typer audit checkpoint (#1417)

**Status: incomplete; current full-corpus evidence has not been collected.**
The pinned full audit is intentionally dispatched manually on a hosted runner;
an attempted local run was stopped before completing its first source file. Do
not treat the partial local observations or historical values below as current
full-corpus results or as a completed Definition of Done.

## Inputs frozen by #1342

The authoritative Scala source revision is
`777528f19a58e794c9954a42f433373472ec57f8`, with Scala 3.9.0 compiler/library
TASTy 28.8.0/28.9.0 artifacts and Temurin JDK 21.0.8. The frozen audit contains
1,236 source files and an unchanged denominator of 3,297 local method
definitions; the historical typed-method count is 0.

The 50 frozen requested roots account for 8,949 weighted observations (18
compiler roots / 8,590; 32 library roots / 359) across 279 files. This is a
root-weight total, not a count of distinct defective artifacts.

## Historical source-attribution discrepancy

The frozen root table sums to 8,949, while the separately frozen
root-by-source-file provenance table sums to 8,948. Both contain 50 roots and
cover 279 source files. The one-observation difference is retained as an
unresolved historical discrepancy; it is not assigned to an invented source
file or root. The current audit now emits and compares source provenance in the
same sequential passes as its normalized reports. The reconciliation tool
reports the historical weights and source observations separately and fails
when any frozen root row is missing.

## Incomplete local audit attempt

On 2026-10-22, a local run used the pinned Scala checkout, verified Scala JAR
hashes, Temurin JDK 21.0.8, one Cargo job, and one Rust test thread. It was
stopped after 71m40s elapsed without completing the first of 1,236 source
files (`audited 0/1236`); the latest progress line recorded 1,264,000 module
identity resolver calls attributed to `GenericSignatures.scala`. The process
reached a sampled RSS of 5,519,344 KiB and CPU temperature readings reached
92°C. The run was paused intermittently to cool the mini PC, so this elapsed
time is not a comparable full-audit runtime. Peak RSS was unavailable because
`/usr/bin/time` is not installed. No report or sidecars were generated. This
attempt is only evidence that the local full audit is impractical under the
observed machine conditions; all current corpus results and DoD gates remain
unverified.

## Focused regression inventory

The [fixture matrix](typer-classpath-corpus-audit-post-tasty-parents-3.9.0-fixtures.tsv)
maps existing compiler/library parent, ModuleClass, rollback, retry, and
resolver-parity regressions. These focused tests do not replace current
full-corpus evidence.

## Final audit procedure

Use the manual `Typer full corpus audit` workflow with the `run-full-audit`
input on the issue branch. It runs two full 1,236-file passes sequentially,
with one Cargo job and one Rust test thread, on the pinned Scala revision,
artifacts, and JDK. It compares normalized reports, all generated cohort
sidecars, and the root-by-source provenance byte-for-byte. A separate focused
classloader profile joins every frozen root and parent against its original
failure and current class hierarchy outcome. The root matrix carries frozen
and current source/file counts and first-error origins side-by-side, including
roots with no current source observations. The root-level profile marks member
lookup as unprobed; real member completion remains covered by the focused
compiler ModuleClass regressions.

The canonical full-audit command is:

```bash
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --verify-determinism \
  docs/typer-classpath-corpus-audit-post-tasty-parents-3.9.0.md
```

The workflow uses the same command and flags with a runner-temporary output
path, then uploads the report and its generated sidecars as one artifact.

The current hard gates are completed dependency depth p95 <= 6 and maximum
completed-class fanout <= 16. The audit must also preserve all frozen cohort
rows and successful target identities, retain the exact 3,297-method
denominator, and report current first blockers and typed methods independently.
Elapsed time and peak RSS are reported only from the runner that actually
executes the audit.

## Current reconciliation and next sprint

The 50-root current matrix, cohort refresh, final blocker ranking, and exactly
one evidence-based next-sprint recommendation are pending the hosted audit.
The issue remains incomplete until these results are reviewed and the matrix
accounts for the historical 8,949 weights, source attribution, current
reachability, and any newly surfaced roots without dropped rows.
