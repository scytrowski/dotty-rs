# JDK module metadata and session resource costs

**GO** for explicit metadata inspection and caller-owned snapshot reuse.
No additional permanent FDs were observed. Four independent classpaths need
276 JMOD handles; low-FD deployments need a separate sharing follow-up.

## Scope and method

This #1201 report extends the frozen #1161 evidence in
[classpath-jdk-performance-results.md](classpath-jdk-performance-results.md).
Historical reports and raw records are unchanged. The production classloader
is unchanged by this benchmark PR.

There are four new raw reports:

- [Before the catalog API](jdk-module-metadata-before-raw.json): ten fresh
  processes at `064c07bae95cee601f86881f06de19eb1e111853`, with no metadata API.
- [Catalog present but unused](jdk-module-metadata-unused-raw.json): ten fresh
  processes at `d0596942748ca44bd0973b2ef31ff47585a08ed0` after #1210, built
  with exactly the original driver source.
- [Extended driver, metadata unused](jdk-module-metadata-extended-driver-raw.json):
  ten additional processes using this PR's larger executable without metadata
  calls, retained to separate benchmark instrumentation footprint.
- [Explicit metadata](jdk-module-metadata-performance-raw.json): ten fresh
  processes for each of 1, 2, and 4 independent `JdkClassPath` instances.

The old class workload remains byte-for-byte unchanged after the new mode
dispatch: five first/1,000 repeated class reads, negative lookup, three
package-evidence queries, and four-thread early/later/missing class reads.
Construction and full-workload high-water RSS in the matched class cohorts are
comparable. Metadata resource checkpoints are a separate workload and report
current RSS and high-water RSS at each stage, including immediately after
initialization **before any metadata call**.

The metadata API has no individual descriptor I/O operation:
`module_catalog()` reads/decodes/validates every descriptor and returns an owned
snapshot; `catalog.get(name)` only searches that snapshot. First catalog
reads and repeated fresh-catalog-plus-get calls therefore measure different
work from repeated snapshot gets. The first timed snapshot get follows the
sample's semantic assertions and is already warm. No implicit cache is added.

A separate preflight process checks count/order, exact canonical JMOD origins,
`java.base` version, `java.net.http` requires/exports, `java.desktop` uses and
provides, and a missing module before timed processes launch. Each sample
also verifies its first snapshots outside the catalog timer. The first read
on every instance is timed; repeated fresh reads/get operations use instance
0. Each initial snapshot is retained until the final drop checkpoints to
observe the effect of caller-owned snapshot reuse.

All cells below are **median / nearest-rank p95 across ten processes**. With
ten observations, p95 is the maximum. Repeated sequential metrics are per-
process averages (10,000 snapshot queries/enumerations or 20 fresh catalog
reads per queried name). They are not individual request percentiles. Fresh
sequential read+get timers exclude temporary snapshot destruction.

Concurrency starts four prepared workers through the existing ready/release
gates, with the timer started before release. Each worker requests ten fresh
catalogs, including a snapshot get and destruction. One scenario shares
instance 0; another assigns instances round-robin. Elapsed wall time includes
join completion and is divided by 40 calls. This **amortized wall time** is a
throughput measure, not individual request latency or a request p95.

`/proc/self/status` supplies current `VmRSS` and high-water `VmHWM` in KiB;
`/proc/self/fd` counts include their temporary enumeration handle. Retained
allocator pages and worker stacks can outlive snapshot destruction. Observed
`VmHWM` values sometimes fell after dropping the classpaths; the reported
observed peak uses the maximum checkpoint value, and raw values are retained.
RSS deltas cannot measure exact live heap bytes; logical catalog UTF-8 bytes
exclude String/Vec capacity, struct overhead, and origin paths.

This is a shared Linux host: an unrelated CPU-bound process was observed on
one of the 16 logical CPUs. Our compilation and test jobs completed before
sampling. No affinity restriction or cache flush was applied. Environment
fingerprinting and preflight warm the OS page cache, so these are fresh-
process, warm-cache measurements. Every completed sample is retained; no
outlier filtering or latency threshold was used.

## Environment and provenance

Measured 2026-10-16 on AMD Ryzen 9 7940HS w/ Radeon 780M Graphics, 16 logical CPUs,
30,582,380 KiB RAM; Debian 13/Linux `6.12.74+deb13+1-amd64`.
Eclipse Temurin `21.0.8+9-LTS` supplied 69 JMODs totaling
86,516,827 bytes, matching #1161. `rustc 1.98.1` / `cargo 1.98.1`
and the default release profile (opt-level 3, no LTO, 16 codegen units, no
debug info) were used with locked dependencies and no build overrides. The
soft/hard FD limits were **1024 / 524288**. All 16 CPUs were in the allowed
affinity mask. Every cohort records matching JMOD SHA-256 hashes and toolchain
identities; each binary's SHA-256 is recorded separately.

The matched before/unused builds use the original driver source SHA-256
`1c0acdb6b90395180a6ff119f45b26cc5fddce6aa9504004a69f649dcb905162`,
the same source as #1161. The metadata/extended-driver samples represent
`5a9091a08` (full revision in raw JSON); subsequent changes only clarify
checkpoint summaries and documentation, leaving the measured Rust code intact.
There are 60 successful fresh-process samples: three ten-process class
cohorts plus three ten-process metadata cohorts. The low-FD diagnostic below
is a separate expected failure, not a discarded performance sample.

## No-metadata baseline comparison

| Measurement | Before API | API unused, matched driver | Median delta |
| --- | ---: | ---: | ---: |
| Initialization (ms) | 17.754 / 18.756 | 17.543 / 20.223 | -0.212 (-1.19%) |
| Full-workload peak RSS (KiB) | 13,210 / 13,328 | 13,202 / 13,328 | -8 (-0.06%) |
| fd_count_after_init | 73 / 73 | 73 / 73 | 0 |
| fd_count_after_workload | 73 / 73 | 73 / 73 | 0 |

The frozen #1161 medians were 18.037 ms init, 13,198 KiB peak RSS, 73 FDs,
and 2.123 ms per repeated raw String read. The matched new unused-API cohort
is 17.543 ms, 13,202 KiB, 73 FDs, and
2.181 ms/String read. That is −2.7% init, +4 KiB RSS and unchanged FDs
versus #1161; raw String extraction is about +2.7%. The paired current before
cohort already measured 2.196 ms/String read, so this is not evidence of
a catalog-induced decompression regression. These are raw class reads, not
per-Typer-reference costs. The init p95 rose from 18.756 to 20.223 ms
with ten samples; this maximum-only tail does not establish a startup regression.
No new eager descriptor work or material startup-memory increase is observed.

The additional extended-driver/unused cohort was
17.801 / 18.886 ms init and
13,406 / 13,524 KiB peak RSS. Its
204 KiB median RSS difference from the matched unused driver is recorded
separately rather than attributed to production classpath construction.

| Class | First before (ms) | First unused (ms) | Repeated before (ms/op) | Repeated unused (ms/op) |
| --- | ---: | ---: | ---: | ---: |
| `java/lang/Object` | 0.168 / 0.173 | 0.169 / 0.180 | 0.159 / 0.161 | 0.159 / 0.160 |
| `java/lang/String` | 2.192 / 2.266 | 2.181 / 2.239 | 2.196 / 2.203 | 2.181 / 2.187 |
| `java/net/http/HttpClient` | 0.194 / 0.205 | 0.192 / 0.204 | 0.185 / 0.187 | 0.184 / 0.185 |
| `java/util/ArrayList` | 0.806 / 0.843 | 0.810 / 0.821 | 0.804 / 0.809 | 0.802 / 0.805 |
| `java/util/concurrent/CompletableFuture` | 1.648 / 1.683 | 1.635 / 1.652 | 1.632 / 1.639 | 1.624 / 1.629 |
| Missing class | 0.021 / 0.034 | 0.016 / 0.024 | 0.00524 / 0.00533 | 0.00508 / 0.00529 |

| Package query | Before (µs/op) | Unused API (µs/op) |
| --- | ---: | ---: |
| `contains_package:java/lang` | 0.124 / 0.341 | 0.080 / 0.165 |
| `contains_package_prefix:java/net/http` | 0.406 / 0.419 | 0.395 / 0.403 |
| `contains_package_prefix:java/util/concurrent` | 0.065 / 0.066 | 0.064 / 0.065 |

| Concurrent class reads | Before (µs/op) | Unused API (µs/op) |
| --- | ---: | ---: |
| `java.base:java/lang/Object` | 41.931 / 47.934 | 41.382 / 42.587 |
| `java.net.http:java/net/http/HttpClient` | 48.792 / 49.995 | 49.092 / 49.482 |
| `missing:dotty_issue_1157/DefinitelyMissing` | 2.493 / 3.101 | 2.161 / 3.115 |

## First metadata access and snapshot reuse

| Sessions | Total initialization (ms) | First catalog on session 0 (ms) | First catalog average per session (ms) |
| --- | ---: | ---: | ---: |
| 1 | 17.875 / 18.237 | 5.437 / 5.902 | 5.437 / 5.902 |
| 2 | 34.532 / 36.619 | 5.551 / 6.364 | 5.518 / 5.877 |
| 4 | 67.853 / 70.178 | 5.496 / 5.754 | 5.496 / 5.966 |

The one-session cohort separates repeated full decoding from retained-snapshot
queries. `java.desktop` exercises both service uses and provider declarations.

| Module query | First timed snapshot get (µs) | Repeated snapshot get (ns/op) | Repeated fresh catalog + get (ms/op) |
| --- | ---: | ---: | ---: |
| `dotty.missing.module` | 0.340 / 0.941 | 37.693 / 38.181 | 5.389 / 5.416 |
| `java.base` | 0.141 / 0.160 | 37.099 / 37.825 | 5.432 / 5.530 |
| `java.desktop` | 0.371 / 1.373 | 35.335 / 36.113 | 5.390 / 5.419 |
| `java.net.http` | 0.491 / 1.182 | 36.083 / 36.733 | 5.406 / 5.424 |

Walking all 69 snapshot module names costs
28.784 / 30.290 ns/enumeration. These tiny snapshot operations include
`black_box` and result checks; they are not a guarantee of nanosecond timing
for arbitrary downstream consumers. A missing-name get returns `None`; a
missing-name query that rebuilds a catalog still pays the full decoding cost.
The current API already permits inexpensive caller-owned snapshot reuse.

## Session FD and memory scaling

| Sessions | FDs after init (unchanged after metadata) | Extra FDs from metadata | RSS after init (KiB) | RSS with initial snapshots (KiB) | Paired snapshot-stage RSS delta (KiB) | Observed peak after workload (KiB) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 73 / 73 | 0 / 0 | 13,138 / 13,180 | 13,312 / 13,356 | 176 / 180 | 15,840 / 15,884 |
| 2 | 142 / 142 | 0 / 0 | 23,588 / 23,720 | 24,010 / 24,140 | 422 / 428 | 26,546 / 26,680 |
| 4 | 280 / 280 | 0 / 0 | 44,558 / 44,596 | 45,450 / 45,496 | 892 / 900 | 47,908 / 47,988 |

Every sample added exactly 69 backing handles per independent instance.
Metadata requests added **zero permanent FDs**; dropping all classpaths
returned the count to 4 in every sample. Retaining the initial catalogs added
176 / 422 / 892 KiB median current RSS for 1 / 2 / 4 instances. Each snapshot
contains 48,340 logical UTF-8 bytes (~47.2 KiB), separate from its container
overhead and paths. The larger 15.8 / 26.5 / 47.9 thousand KiB observed peaks
include repeated temporary catalogs, allocator retention and thread activity;
they do not represent the live snapshot size. Dropping catalogs did not lower
RSS in these runs, so exact retained-cache heap cost cannot be inferred.

| Sessions | Shared instance (amortized ms/op) | Round-robin instances (amortized ms/op) |
| --- | ---: | ---: |
| 1 | 1.420 / 1.435 | 1.422 / 1.451 |
| 2 | 1.421 / 1.437 | 1.424 / 1.459 |
| 4 | 1.429 / 1.437 | 1.420 / 1.503 |

The shared and independent scenarios have similar throughput here.
This does not measure individual-query p95, prove scaling beyond four workers,
or include semantic compiler sessions: a “session” here means an independent
classpath object, not a populated typer/symbol store.

## Decision and next sprint

**GO** for the explicit read-only catalog and the existing file-backed JMOD
implementation. Construction stays lazy with respect to metadata (the unit
regression also constructs empty JMODs successfully and sees a missing
descriptor only on explicit access). There is no extra persistent descriptor
handle and no material startup-memory regression in the matched comparison.

**Reuse a returned catalog for repeated metadata queries.** Rebuilding a full
catalog costs about 5.4 ms even for a miss, versus about 37 ns for a snapshot
get in this harness; retaining one snapshot increased observed RSS by about
176 KiB. This supports caller-owned snapshot reuse, which the API already
provides. It does not justify a mutable global cache or automatic library
caching with a new lifetime/invalidation policy.

**NO-GO for promising four independent instances under a 256-FD budget.**
Their measured 280-FD count exceeds that real process constraint. A separate
probe with soft `RLIMIT_NOFILE=256` and four instances exited with status 2:
`classpath I/O error: Too many open files (os error 24)`. Under this host's
actual soft limit of 1024, four instances leave substantial headroom; fifteen
would exceed it even with the observed minimal four-FD process baseline.
This is a deployment budget, not an arbitrary performance threshold.

**Separately scoped next-sprint follow-up: caller-owned JDK classpath sharing
for concurrent sessions with low FD budgets.** Evaluate explicit shared
ownership of one immutable `JdkClassPath` across compilation sessions targeting
the same JDK, with independent semantic stores and optional caller-owned
catalog snapshots. Success criteria: four consumers operate under a 256-FD
limit with one 69-handle archive set; preserve class precedence/error ordering,
archive origin/file identity and lifetime behavior; rerun this shared versus
independent workload and measure contention. No mutable global discovery/cache
and no semantic-store sharing. This benchmark supplies evidence for that
follow-up; it does not implement it.

A lazy archive index or bounded reopen pool is **not recommended as an
unconditional next change**: it would move validation/I/O errors to first use
and requires a decision about stable open-file identity after replacement or
unlink. Evaluate those alternatives only if caller-owned sharing cannot meet
a measured deployment budget. The observed ~1.42 ms amortized shared-request
cost does not demonstrate a mutex bottleneck needing additional JMOD handles.

## Reproduction

Use the pinned Temurin JDK 21.0.8+9-LTS and Rust/Cargo 1.98.1, from this
branch's repository root. Provide `JDK21` explicitly; do not change global JDK
settings. The runner rejects absent/empty JMOD paths and JDK versions other
than 21. On hosts with a full `/tmp`, provide a writable `TMPDIR` (the captured
runs used private user/mount/network/PID namespaces and a private tmpfs `/tmp`
for environment isolation, without flushing file cache).

```bash
export TMPDIR="$PWD/target/tmp-1201"
mkdir -p "$TMPDIR" target/before-src target/unused-src
JDK21=/path/to/temurin-21.0.8+9-LTS
BEFORE=064c07bae95cee601f86881f06de19eb1e111853
UNUSED=d0596942748ca44bd0973b2ef31ff47585a08ed0
MEASURED=5a9091a0853e5af45626ddf32b486ac40995208f

git archive "$BEFORE" | tar -x -C target/before-src
git archive "$UNUSED" | tar -x -C target/unused-src
CARGO_TARGET_DIR="$PWD/target/before-target" cargo build --release --locked \
  --manifest-path target/before-src/Cargo.toml -p dotty-classloader \
  --example jdk_classpath_benchmark
CARGO_TARGET_DIR="$PWD/target/unused-target" cargo build --release --locked \
  --manifest-path target/unused-src/Cargo.toml -p dotty-classloader \
  --example jdk_classpath_benchmark
cargo build --release --locked -p dotty-classloader --example jdk_classpath_benchmark

./tools/jdk-classpath-benchmark/run --jmods "$JDK21/jmods" --samples 10 \
  --binary "$PWD/target/before-target/release/examples/jdk_classpath_benchmark" \
  --source-revision "$BEFORE" --purpose before-catalog-no-metadata \
  --output docs/jdk-module-metadata-before-raw.json
./tools/jdk-classpath-benchmark/run --jmods "$JDK21/jmods" --samples 10 \
  --binary "$PWD/target/release/examples/jdk_classpath_benchmark" \
  --source-revision "$MEASURED" --purpose catalog-unused \
  --output docs/jdk-module-metadata-extended-driver-raw.json
./tools/jdk-classpath-benchmark/run --metadata --jmods "$JDK21/jmods" --samples 10 \
  --binary "$PWD/target/release/examples/jdk_classpath_benchmark" \
  --source-revision "$MEASURED" --purpose explicit-module-metadata \
  --output docs/jdk-module-metadata-performance-raw.json
./tools/jdk-classpath-benchmark/run --jmods "$JDK21/jmods" --samples 10 \
  --binary "$PWD/target/unused-target/release/examples/jdk_classpath_benchmark" \
  --source-revision "$UNUSED" --purpose catalog-unused-original-driver \
  --output docs/jdk-module-metadata-unused-raw.json

# Separate expected-failure diagnostic; this limit applies only to the subshell.
(ulimit -Sn 256; target/release/examples/jdk_classpath_benchmark \
  --metadata --jmods "$JDK21/jmods" --sessions 4)
```

These are the captured cohort order and workload commands. `MEASURED` names
the unchanged measured Rust code; when rebuilding changed benchmark code,
record its actual full commit with `git rev-parse HEAD`. New samples will
vary with scheduling/cache/clock conditions. The final runner computes the
observed peak as the maximum checkpoint HWM from the preserved raw samples.

## Validation

Validation completed for this increment:

```text
python3 tools/jdk-classpath-benchmark/test_run.py
./tools/jdk-classpath-benchmark/run --smoke
cargo test -p dotty-classloader --example jdk_classpath_benchmark --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test --workspace --all-targets --locked
git diff --check
```

Tool tests check aggregation, field/iteration counts, negative paths,
independent process launches, unchanged historical output defaults, and all
checked-in raw summaries against their individual samples. Smoke uses tiny
checked-in class/JMOD fixtures and needs no JDK. Metadata semantics and exact
JMOD source identity were additionally validated on the measured real JDK.
