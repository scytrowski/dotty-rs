# Where prepared JVM class emission spends time

On 256 files with four workers, building the semantic class hierarchy during
method lowering consumed 88.8% of summed worker elapsed time for independent
chains and 88.9% for shared receivers. Each layout performed 768 full hierarchy
builds during emission, after the unit had already prepared its hierarchy.
The initial profile used backend revision `bba5c6e371`.
The [follow-up experiment](#sharing-the-semantic-hierarchy-across-emission-workers) compares the resulting change with a fresh baseline.

## Two call graphs with the same method bodies

Each group contains four classes, four source methods with inferred integer
results, four implicit no-arg constructors, six class-typed method parameters
and three virtual calls with arities zero, one and two. Both layouts declare the
same classes in the same order. The leaf bodies, parameter loads, forwarded
arguments and call depth are the same.

- **Chains.** Each group's three callers use that group's Second, First and Leaf.
- **Shared.** All groups call Second, First and Leaf from group `groups / 2`
  (zero-based). Every class still gets emitted. The three central methods each
  receive one call per group.

The fixture decodes the emitted call instructions from two groups and checks
the target classes and descriptors of all six calls in the shared layout.
Each timed run compares every class name and byte array, in order, with its
own size and layout's one-worker reference.

## Batch latency and summed worker time

Rust 1.98.1 release builds ran on the same shared Ryzen 9 7940HS with affinity
to logical CPUs 0–7, one per physical core. Each process warmed up all twelve
configurations, then rotated input sizes and varied layout and worker order
over nine rounds. Two control runs bracketed one run with internal probes.
The controls recorded wall time from immediately before `emit_batch` until
the caller had collected the ordered byte arrays. Each worker separately timed
its emission loop. With four workers these intervals overlap. They include scheduling delays
and exclude thread creation and joins.

Times are medians in milliseconds from 18 control samples per configuration.
The last column compares layouts within the same round, size, worker count and
process. It gives the median shared/chains wall-time ratio and its middle 50%
(linearly interpolated quartiles). [All 324 samples](compiler-client-timings/emission-runs.csv)

| Files | Workers | Chains wall | Shared wall | Chains worker sum | Shared worker sum | Paired wall ratio [Q1, Q3] |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 16 | 1 | 1.49 | 1.46 | 1.31 | 1.16 | 1.03 [0.93, 1.21] |
| 16 | 4 | 0.99 | 1.25 | 1.35 | 1.36 | 1.03 [0.79, 1.39] |
| 64 | 1 | 16.65 | 18.60 | 16.48 | 18.45 | 1.01 [0.85, 1.21] |
| 64 | 4 | 9.44 | 8.47 | 18.88 | 17.16 | 0.97 [0.78, 1.18] |
| 256 | 1 | 343.27 | 324.31 | 342.03 | 322.76 | 0.91 [0.84, 0.99] |
| 256 | 4 | 88.45 | 83.56 | 290.16 | 288.50 | 1.04 [0.85, 1.16] |

At 256 files the shared layout was typically faster with one worker in this
run. With four workers the paired ratios varied on both sides of one. The
operation counts and the dominant stage were the same for both layouts.

## Time inside each class

For each sample we summed worker milliseconds per stage across a complete
batch. The table gives the median for each stage across nine instrumented samples. Plan checking
includes class lookup. Constructor time covers initializer lowering and
instruction construction. Setup stages the class builder, fields and verifier
hierarchy. Method time includes local layout, call resolution, lowering and
staging the method. Bytecode checks cover field-operation preflight, descriptor
checks for linear code and the verifier analysis. Every sampled class had two
linear methods. Serialization covers structural reference validation and
encoding the final class into memory. Other encoder work includes constant-pool
construction, instruction assembly and building class-file structures.

| Files | Workers | Layout | Plan check | Constructors | Setup | Methods | Bytecode checks | Serialization | Other encoder |
| ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 16 | 1 | Chains | 0.022 | 0.024 | 0.006 | 0.957 | 0.063 | 0.046 | 0.149 |
| 16 | 1 | Shared | 0.018 | 0.025 | 0.006 | 0.940 | 0.061 | 0.045 | 0.146 |
| 16 | 4 | Chains | 0.034 | 0.035 | 0.007 | 1.010 | 0.075 | 0.053 | 0.173 |
| 16 | 4 | Shared | 0.027 | 0.033 | 0.006 | 0.978 | 0.064 | 0.048 | 0.160 |
| 64 | 1 | Chains | 0.127 | 0.155 | 0.041 | 15.914 | 0.494 | 0.290 | 1.060 |
| 64 | 1 | Shared | 0.111 | 0.134 | 0.032 | 13.739 | 0.382 | 0.254 | 0.867 |
| 64 | 4 | Chains | 0.108 | 0.129 | 0.029 | 15.476 | 0.336 | 0.228 | 0.804 |
| 64 | 4 | Shared | 0.114 | 0.146 | 0.035 | 21.943 | 0.385 | 0.247 | 0.917 |
| 256 | 1 | Chains | 0.689 | 0.845 | 0.209 | 260.936 | 2.377 | 1.353 | 6.889 |
| 256 | 1 | Shared | 0.756 | 0.922 | 0.201 | 277.160 | 2.512 | 1.479 | 6.129 |
| 256 | 4 | Chains | 0.487 | 0.639 | 0.146 | 256.596 | 1.767 | 1.084 | 5.316 |
| 256 | 4 | Shared | 0.505 | 0.656 | 0.148 | 262.572 | 1.846 | 1.143 | 5.224 |

## Repeated unit-wide work

Every build below took place inside method lowering. Each called
`JvmCompilationUnit::validate` once. The worker threads performed zero
full metadata builds. Two call sites account for every hierarchy build. The
projection function `project_class` checks a reference type's ancestry when its
context contains only the unit. The assignability check in `is_assignable`
checks plan membership even when both reference types match exactly. In the bytecode verifier, handling the separate verifier
hierarchy inside the encoder took 0.30–0.31 ms of summed worker time at 256
files and four workers. [Reference checks](../crates/dotty-jvm/src/reference.rs) · [Prepared emission](../crates/dotty-jvm/src/emission.rs)

Counts are exact per batch in every instrumented sample at either worker count
and with either layout. Percentages are medians of the within-sample hierarchy
build time divided by summed worker elapsed time. This interval includes full
unit validation and is part of the method column above.

| Files | Builds / full-unit validations | Reference projections triggering builds | Exact assignability checks triggering builds | Chains, 1 / 4 workers | Shared, 1 / 4 workers |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 16 | 48 / 48 | 36 | 12 | 64.0% / 62.1% | 63.1% / 61.3% |
| 64 | 192 / 192 | 144 | 48 | 77.4% / 82.6% | 79.3% / 83.8% |
| 256 | 768 / 768 | 576 | 192 | 87.7% / 88.8% | 87.0% / 88.9% |

These counts suggested that retaining the validated semantic hierarchy in
`PreparedJvmEmission` could eliminate all three builds per file on these call
graphs. The comparison below tests that prediction.

## Reproducing the initial profile

Run the standalone test below at `1de459c368`. To measure the
internal stages, apply the probe patch, repeat the command, reverse the patch
and run the control once more. Each `emit_probe` record follows a completed
batch and precedes its sample row. The first twelve records are warm-ups.
Fields in the probe array follow the CSV column order starting at `classes`.
Workers collect timings locally and the caller prints after stopping the batch
clock. [Probe patch](compiler-client-timings/emission-probes.patch)

```sh
taskset -c 0-7 cargo test --release --locked \
  --manifest-path tests/fixtures/compiler-client/Cargo.toml --target-dir target/compiler-client \
  --test multi_file measure_dependent_pipeline -- --ignored --nocapture
```

## Sharing the semantic hierarchy across emission workers

The baseline was `1de459c368`. The changed backend keeps the semantic hierarchy
built by `prepare_emission` and lends it to method lowering. The worker checks
the class of each call receiver, argument and result against the same unit.
An exact reference match still requires the class symbol to occur in the plan.
The public client API and the caller's scheduling code are identical for both
versions.

Both release binaries used Rust 1.98.1, the same generated sources and CPU
affinity (Ryzen 9 7940HS, logical CPUs 0–7). We ran four processes sequentially
in baseline, changed, changed, baseline order. Each warmed up all twelve cases.
The table gives median wall time in milliseconds over 18 samples per version
and configuration. Speedups divide the two unrounded medians. Emission includes
starting the workers and collecting their ordered results. The full pipeline
starts at session setup and ends with the class bytes in memory. [All 432 latency samples](compiler-client-timings/emission-worker-reuse-runs.csv)

| Files | Workers | Call graph | Emission before → after (ms) | Speedup | Full pipeline before → after (ms) | Speedup |
| ---: | ---: | --- | ---: | ---: | ---: | ---: |
| 16 | 1 | Chains | 1.14 → 0.53 | 2.2× | 1.98 → 1.33 | 1.5× |
| 16 | 1 | Shared | 0.81 → 0.49 | 1.7× | 1.55 → 1.22 | 1.3× |
| 16 | 4 | Chains | 0.62 → 0.30 | 2.0× | 1.35 → 1.02 | 1.3× |
| 16 | 4 | Shared | 0.63 → 0.30 | 2.1× | 1.44 → 1.01 | 1.4× |
| 64 | 1 | Chains | 9.67 → 1.58 | 6.1× | 12.57 → 4.54 | 2.8× |
| 64 | 1 | Shared | 9.84 → 1.29 | 7.7× | 12.41 → 3.70 | 3.4× |
| 64 | 4 | Chains | 3.28 → 0.67 | 4.9× | 5.75 → 2.87 | 2.0× |
| 64 | 4 | Shared | 3.37 → 0.66 | 5.1× | 5.61 → 2.63 | 2.1× |
| 256 | 1 | Chains | 140.81 → 5.57 | 25.3× | 149.65 → 14.57 | 10.3× |
| 256 | 1 | Shared | 140.98 → 5.67 | 24.8× | 151.00 → 14.30 | 10.6× |
| 256 | 4 | Chains | 38.26 → 2.17 | 17.6× | 45.84 → 9.06 | 5.1× |
| 256 | 4 | Shared | 38.68 → 1.86 | 20.8× | 45.59 → 8.28 | 5.5× |

At 256 files and four workers, the middle 50% of emission times fell from
37.71–39.32 ms to 2.08–2.22 ms for the chains, and from 38.37–39.45 ms to
1.76–2.18 ms for shared receivers. Preparation at this size stayed close to
1.2 ms in both versions. With reuse, going from one worker to four sped up
emission by 2.6× for chains and 3.0× for shared receivers. These figures cover
straight-line call bodies on a shared host.

### Counts and correctness

We applied the same two thread-local counters to separate release builds of
both versions and repeated the run. Fresh emission workers counted calls to
`unit_hierarchy` and `JvmCompilationUnit::validate`. Preparation takes place on
the caller's thread. Every warm-up and all nine measured rounds gave the counts
below for both call graphs and both worker counts. [Worker counter patch](compiler-client-timings/emission-worker-counts.patch)

| Files | Worker hierarchy builds before → after | Worker full-unit validations before → after |
| ---: | ---: | ---: |
| 16 | 48 → 0 | 48 → 0 |
| 64 | 192 → 0 | 192 → 0 |
| 256 | 768 → 0 | 768 → 0 |

A separate cross-version comparison checked the position, internal name and
contents of every emitted class in all twelve configurations. All 1,344 class
files matched byte for byte. The invalid-call regression produced the same
`JvmCallResultTypeMismatch` and `JvmCallArgumentMismatch` with the same tree,
method and type identities. A focused regression checks the hierarchy build
count across preparation and emission for nested reference calls. Separate
assignability assertions cover both the prepared proof and standalone lookup,
including the rejection of an unplanned class with the same name.

To repeat the latency comparison, run the release command above on the baseline
and changed revisions in the stated order. For the counts, apply
`emission-worker-counts.patch` to each revision and repeat once. The probe
prints `emission_counts,files,workers,hierarchy_builds,unit_validations`
immediately before each timed sample, after the initial twelve warm-up records.
