# Compiling dependent Scala sources through the public library

The standalone client compiles 64 in-memory sources in a fresh semantic
session. They form 16 independent chains of four classes. Each method has an
inferred result. A leaf adds an integer to a local value. The other methods
call through a receiver declared in another file. Input order places the last
caller first in each chain, exercising completion of missing method signatures
and retries in the owning sources.

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
interval. Every sample compares all 64 class names and byte arrays against
the one-worker reference.

The harness warms up both configurations and then alternates their order over
nine pairs. It prints raw wall-clock microseconds for every phase and the whole
compilation. The regular client tests cover the chain without a timing threshold.

## Original measurements before call metadata reuse

Rust 1.98.1, release build, AMD Ryzen 9 7940HS. Times are medians in
milliseconds, with each column calculated from the nine samples for that
worker count. The total comes from the elapsed time of each complete run.

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
