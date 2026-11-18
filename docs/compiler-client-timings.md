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

## Local results

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
for this workload. A useful next profiling target is virtual call lowering:
the instance call resolver requests metadata for the entire unit to find the
receiver and method owner. The phase timings do not isolate the cost of that
individual operation. [Call target resolution](../crates/dotty-jvm/src/call_target.rs)
