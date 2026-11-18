# Where prepared JVM class emission spends time

On 256 files with four workers, building the semantic class hierarchy during
method lowering consumed 88.8% of summed worker elapsed time for independent
chains and 88.9% for shared receivers. Each layout performed 768 full hierarchy
builds during emission, after the unit had already prepared its hierarchy.
The backend at `bba5c6e371` produced all measurements below.

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

The next experiment is to retain the validated semantic hierarchy in
`PreparedJvmEmission` and borrow it during method lowering, in both call
receiver/parameter projection and exact-reference assignability. Both checks
can use the same unit's existing proof across workers. For these sources the count should fall
from 768 to zero during emission of the 256-class batch. The same test should
compare ordered class bytes and check rejection diagnostics on invalid plans. A fresh timing run would quantify the actual gain.
The measurements cover straight-line methods on these two call graphs.

## Reproduction

Run the standalone test below from the repository root. To measure the
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
