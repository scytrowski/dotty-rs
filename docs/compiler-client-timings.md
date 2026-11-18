# Compiling dependent Scala sources through the public library

The standalone client compiles 16, 64 or 256 in-memory sources in a fresh
semantic session. Each group of four classes contains a leaf that adds an
integer to a local value and three callers with inferred results. The original
layout uses independent chains. A second layout shares three receiver types
across all groups. Callers precede their group's leaf in both layouts.
[Emission measurements for both layouts](compiler-client-emission-profile.md) · [Cross-file signature scheduling](compiler-client-signature-order.md)

Run the measurement from the repository root with the pinned Rust toolchain.

```sh
cargo test --release --locked --manifest-path tests/fixtures/compiler-client/Cargo.toml \
  --target-dir target/compiler-client --test multi_file measure_dependent_pipeline \
  -- --ignored --nocapture
```

Both configurations use the same detached parser and prepared JVM backend.
The client divides parsing and emission into at most one or four scoped worker
threads respectively. Name binding, declaration entry, signature completion
and body typing run on the calling thread. The interval starts before creating
the session and ends when the ordered class bytes are available. Source
generation, disk I/O, Java execution and artifact comparison are outside the
interval. Every sample compares the ordered class names and byte arrays against
the one-worker reference for its size and layout.

The harness warms up both worker counts for each size. Over nine pairs it
alternates the worker order and rotates which size runs first. It prints raw
wall-clock microseconds for every phase and the whole compilation. The regular client tests cover the chain without a timing threshold.

## Original measurements before call metadata reuse

The original 64-file measurement used Rust 1.98.1, a release build and an AMD
Ryzen 9 7940HS. Times are medians in milliseconds, with each column
calculated from the nine samples for that worker count. The total comes from the elapsed time of each complete run.

| Phase | One worker | Four workers |
| --- | ---: | ---: |
| Session bootstrap | 0.040 | 0.041 |
| Scan and parse | 1.120 | 0.563 |
| Bind names and enter declarations | 0.281 | 0.357 |
| Complete signatures, including inference and retries | 0.805 | 0.737 |
| Type bodies and retain bindings | 0.481 | 0.527 |
| Build class plans and prepare JVM emission | 19.563 | 21.306 |
| Emit classes | 1979.615 | 547.760 |
| **Complete compilation** | **2008.568** | **570.710** |

Total elapsed times ranged from 1900 to 2190 ms with one worker and from
499 to 616 ms with four. The ratio of median elapsed times was 3.52.
Class emission consumed a median of 95.6% of elapsed time across the four-worker
samples. The results give little incentive to parallelize the semantic store
for this workload.

## Cost of rebuilding JVM call metadata

The 16 chains contain 48 virtual calls through class-typed parameters. At
`fba3d0b5a9`, each call rebuilt metadata for all 64 classes twice, once to
check the receiver parameter and once to resolve the target method. The unit
had already built this metadata during emission preparation.

A separate release run timed each `JvmCompilationUnit::metadata` invocation
during the emission interval and recorded its caller. Each of three runs per
worker count recorded 48 invocations at each site. With one worker the median
sum of their durations was 1760 ms, or 99.0% of emission time. With four
workers the median sum across threads was 1784 ms, overlapping within a median
455 ms of wall time. Passing the prepared metadata slice to call resolution
reduced the count to zero in all six runs. All 64 class files matched the
original files byte for byte with either worker count.

The table uses fresh before-and-after runs of the command above on the same
machine, with timing probes removed. Each value is the median of nine samples
in milliseconds.

| Phase | One worker before | One worker after | Four workers before | Four workers after |
| --- | ---: | ---: | ---: | ---: |
| Build class plans and prepare JVM emission | 18.044 | 22.436 | 18.013 | 23.870 |
| Emit classes | 1747.545 | 13.177 | 444.980 | 4.365 |
| **Complete compilation** | **1768.891** | **37.238** | **464.962** | **30.034** |

On this workload, reusing metadata sped up complete compilation by 47.5× with
one worker and 15.5× with four. After the change, emission still benefits from
parallelism (3.0×), while the sequential plan construction and preparation
phase now takes the largest share of elapsed time. Total times after the change
ranged from 36.3 to 54.2 ms with one worker and 26.7 to 40.7 ms with four.

## Where JVM preparation spends time

This experiment uses the production backend at `13079f5bf3`. The client times
construction of the `JvmCompilationUnit`, including its class plans, separately
from `prepare_emission`. Both stages run on the calling thread in both worker
configurations.

On Linux 6.12.74, with Rust 1.98.1 in release mode on the Ryzen 9 7940HS, we
ran the test binary three times in succession, first without probes, then
with probes and finally using the original binary again. All runs used CPU
affinity 0–7 (one logical CPU per physical core) on a shared host. Each run
contributed nine samples per table row after warm-up. Each sample reproduced
the ordered class files for that workload. The CSV keeps the two control runs
and the instrumented run separate. [Individual measurements](compiler-client-timings/prepare-runs.csv)

| Files | Workers | Client plans (µs) | Prepare (ms) | Prepare middle 50% (ms) | Emit (ms) | End to end (ms) |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 16 | 1 | 7.0 | 1.90 | 1.88–1.98 | 1.86 | 8.00 |
| 16 | 4 | 6.5 | 1.90 | 1.85–1.94 | 4.85 | 10.82 |
| 64 | 1 | 40.5 | 37.98 | 34.23–45.85 | 22.09 | 65.84 |
| 64 | 4 | 38.5 | 34.58 | 32.57–49.69 | 11.04 | 54.04 |
| 256 | 1 | 249.0 | 770.16 | 619.28–878.85 | 424.88 | 1200.99 |
| 256 | 4 | 243.5 | 766.79 | 657.01–921.19 | 163.89 | 952.83 |

Times above are medians from the 18 control samples per configuration. The
middle 50% uses linearly interpolated 25th and 75th percentiles. Absolute
latencies varied across runs. At 64 files and four workers the individual
control medians for preparation were 34.20 and 36.02 ms. Client plan
construction took less than 0.25 ms at every size by median.

### Attribution inside the backend

The probes timed the first hierarchy build, `metadata_with_hierarchy`, and the
remaining accessor checks, identity copies and verifier hierarchy construction.
During metadata construction they also counted and timed the hierarchy builds
inside `reference::project_class`. Each nested build includes validation of
the full compilation unit.

| Files | Workers | First hierarchy (ms) | Metadata (ms) | Remaining setup (ms) | Hierarchy rebuilds inside metadata | Rebuilds (ms) | Share of preparation |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 16 | 1 | 0.036 | 1.87 | 0.014 | 96 | 1.68 | 87.7% |
| 16 | 4 | 0.038 | 1.86 | 0.014 | 96 | 1.69 | 87.9% |
| 64 | 1 | 0.130 | 43.64 | 0.038 | 384 | 40.78 | 94.7% |
| 64 | 4 | 0.136 | 36.16 | 0.040 | 384 | 34.50 | 94.3% |
| 256 | 1 | 0.535 | 1006.80 | 0.167 | 1536 | 977.03 | 97.3% |
| 256 | 4 | 0.512 | 621.19 | 0.164 | 1536 | 605.10 | 97.3% |

These are medians of the nine instrumented samples per configuration. The
rebuild interval is part of the metadata interval. Each percentage is the
median of the nine within-sample ratios to total internal preparation time.
Logging follows the last internal timestamp. Internal and control times come
from separate executions.

Each four-file chain declares six class-typed parameters. The class metadata
builder computes a method descriptor for accessor collision checks and again
for method metadata. Each computation projects both the parameter's signature
type and its symbol type. For each projection the backend rebuilds the entire
unit hierarchy to check that the reference class derives from Object.
This accounts for 24 nested builds per chain and 384 for the original 64
files. All samples had the predicted count. [Class metadata](../crates/dotty-jvm/src/identity.rs) · [Parameter projection](../crates/dotty-jvm/src/descriptor.rs) · [Reference validation](../crates/dotty-jvm/src/reference.rs)

With four times as many files, this workload performs four times as many
hierarchy builds, each traversing four times as many class plans. The measured
cost is concentrated in a proof for which preparation already has the unit
hierarchy. The measured shares apply to the independent chains above. Measuring the
attainable speedup would require running the same inputs with a backend that
reuses the hierarchy.

To reproduce the original instrumented run, check out `f71929570d`, apply the archived probes,
run the release command above, then remove the probes. On this host each test
run used `taskset -c 0-7`. The `prepare_probe` rows include the initial
warm-ups. For measured samples the probe row immediately precedes the
corresponding client row. [Measurement patch](compiler-client-timings/prepare-probes.patch)

```sh
git apply --check docs/compiler-client-timings/prepare-probes.patch
git apply docs/compiler-client-timings/prepare-probes.patch
taskset -c 0-7 cargo test --release --locked --manifest-path tests/fixtures/compiler-client/Cargo.toml \
  --target-dir target/compiler-client --test multi_file measure_dependent_pipeline \
  -- --ignored --nocapture
git apply -R docs/compiler-client-timings/prepare-probes.patch
```

## Reusing the hierarchy during reference projection

The backend now passes the unit hierarchy built by `prepare_emission` to
reference-type projection while building method, constructor and field metadata.
Projection still checks symbol identity, plan membership and the path from the
class to Object. Callers that supply only a compilation unit build a hierarchy
when projection needs it. The standalone unit metadata API goes through the
same metadata builder as emission preparation.

We rebuilt the 16-, 64- and 256-file benchmarks from `f71929570d` and the
changed backend with the same Rust 1.98.1 release toolchain. We then ran the
binaries in baseline, changed, changed, baseline order, using CPU affinity
0–7 on the same shared Ryzen 9 7940HS. Each binary invocation used the warm-up,
rotating workload sizes and alternating worker counts described above. Each
table entry is the median of 18 uninstrumented wall-clock samples in milliseconds.
Speedups are ratios of the unrounded medians. [All 216 samples](compiler-client-timings/hierarchy-reuse-runs.csv)

| Files | Workers | Prepare before (ms) | Prepare after (ms) | End to end before (ms) | End to end after (ms) | End-to-end speedup |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 16 | 1 | 1.872 | 0.127 | 4.64 | 2.84 | 1.63× |
| 16 | 4 | 1.917 | 0.130 | 4.84 | 2.30 | 2.11× |
| 64 | 1 | 31.356 | 0.455 | 47.14 | 16.70 | 2.82× |
| 64 | 4 | 27.019 | 0.473 | 44.11 | 11.73 | 3.76× |
| 256 | 1 | 518.275 | 2.312 | 827.48 | 249.66 | 3.31× |
| 256 | 4 | 483.502 | 2.329 | 628.81 | 92.57 | 6.79× |

For the original 64-file, four-worker case the middle 50% of preparation
times was 21.19–35.37 ms before and 0.439–0.507 ms after (linearly
interpolated quartiles). For 256 files with four workers, class emission
now takes a median of 80.25 ms out of 92.57 ms end to end. Both absolute
times and speedup ratios describe this source generator and host.

A separate run compared the baseline and changed class files by internal
name and exact contents. All 672 comparisons matched across the three sizes
and two worker counts. The timed harness also compared every ordered batch
against its own binary's one-worker reference outside the timed interval.

Counters in separate release binaries covered the full `prepare_emission`
call, including the initial hierarchy build. Both versions used the same
probe. All 20 preparations per size and version (two warm-ups plus 18
samples) recorded the counts below. The one remaining build performs the
initial full-unit validation. [Count probe for either version](compiler-client-timings/hierarchy-reuse-counts.patch)

| Files | Hierarchy builds before → after | Full-unit validations before → after |
| ---: | ---: | ---: |
| 16 | 97 → 1 | 97 → 1 |
| 64 | 385 → 1 | 385 → 1 |
| 256 | 1537 → 1 | 1537 → 1 |

To repeat the latency comparison, run the release command at the top of this
document on each revision with the same CPU affinity. To repeat the counts,
apply `hierarchy-reuse-counts.patch` to each revision and rerun that command.
The probe prints `prepare_counts,files,hierarchy_builds,unit_validations`.
