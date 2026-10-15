# JDK classpath performance results

## Recommendation

**GO** for the file-backed JMOD integration from #1160. On the matched JDK 21
host, initialization and peak RSS both fell substantially. First, sequential
repeated, concurrent, negative, and package-evidence lookup p95 values show no
material regression. Opening all JMODs now keeps one file descriptor per
module open for the lifetime of `JdkClassPath`; the measured increment was 69
descriptors, which is acceptable for the current JDK runtime layout but should
be revisited if callers impose a low descriptor limit.

## Summary

Each cell is **median / p95 across 10 independently launched processes**.
Latency is milliseconds unless the table says microseconds. The benchmark's
p95 uses nearest-rank selection, so for 10 samples it is the maximum sample.
The workload and release benchmark executable source are identical before and
after. The before revision uses in-memory JMOD archives; the after revision
adds file-backed ZIP storage and wires it into JMOD loading. Intervening
classpath regression commits add fixtures but do not change that before/after
runtime path.

| Metric | Before #1160 | After #1160 | Change |
| --- | ---: | ---: | ---: |
| `JdkClassPath::new` (ms) | 43.534 / 151.845 | 17.535 / 19.233 | median −59.7%; p95 −87.3% |
| Peak process RSS (KiB) | 97,670 / 97,752 | 13,184 / 13,300 | median −86.5% |
| Open FDs after init | 4 / 4 | 73 / 73 | +69 |
| Open FDs after workload | 4 / 4 | 73 / 73 | +69 |

The descriptor counter enumerates `/proc/self/fd`, so each count includes the
temporary descriptor used for that enumeration. The delta of 69 matches the 69
open JMOD backing files. The archive index and package-prefix data are still
eagerly retained; only complete JMOD payloads stopped being retained in the
process heap.

## Lookup results

First and sequential repeated lookups include the existing #1157 workload. It
checks the actual class origin as well as success/miss status. Repeated lookup
is the average per operation over 1,000 calls in the same process; it does not
imply a decompressed-class cache.

| Class | First before (median / p95 ms) | First after (median / p95 ms) | Repeated before (median / p95 ms per op) | Repeated after (median / p95 ms per op) |
| --- | ---: | ---: | ---: | ---: |
| `java/lang/Object` (`java.base`) | 0.168 / 0.175 | 0.164 / 0.170 | 0.152 / 0.159 | 0.152 / 0.154 |
| `java/lang/String` (`java.base`) | 2.150 / 2.241 | 2.117 / 2.199 | 2.130 / 2.291 | 2.101 / 2.112 |
| `java/util/ArrayList` (`java.base`) | 0.787 / 0.868 | 0.770 / 0.802 | 0.783 / 1.021 | 0.773 / 0.777 |
| `java/util/concurrent/CompletableFuture` (`java.base`) | 1.591 / 2.408 | 1.580 / 1.596 | 1.585 / 2.007 | 1.565 / 1.579 |
| `java/net/http/HttpClient` (`java.net.http`) | 0.191 / 0.199 | 0.189 / 0.198 | 0.179 / 0.196 | 0.178 / 0.178 |
| Missing class | 0.036 / 0.042 | 0.032 / 0.036 | 0.00513 / 0.00554 | 0.00502 / 0.00511 |

The synchronized concurrent workload starts four workers together; each does
250 lookups of an early-module class, a later-module class, or a missing class.
The following values are mean latency per operation within a process, then
median / p95 across the 10 process samples:

| Concurrent lookup | Before (µs/op) | After (µs/op) |
| --- | ---: | ---: |
| `java/lang/Object` | 40.291 / 60.313 | 40.278 / 45.989 |
| `java/net/http/HttpClient` | 47.654 / 65.490 | 47.111 / 48.439 |
| Missing class | 2.407 / 2.885 | 2.298 / 3.200 |

Concurrent negative lookup p95 increased by 0.315 µs/op, while remaining below
0.004 ms/op. No concurrent scenario shows a material latency regression.

Package-evidence queries avoid reading or inflating class payloads. Values are
microseconds per operation, median / p95 across process samples:

| Query | Before (µs/op) | After (µs/op) |
| --- | ---: | ---: |
| Direct `java/lang` | 0.156 / 0.359 | 0.071 / 0.255 |
| Descendant `java/net/http` | 0.393 / 0.412 | 0.384 / 0.399 |
| Descendant `java/util/concurrent` | 0.064 / 0.069 | 0.062 / 0.066 |

## Method and environment

- JDK: Eclipse Temurin `21.0.8+9-LTS`; 69 JMODs, 86,516,827 bytes total.
- Machine: Linux Debian 13, kernel `6.12.74+deb13+1-amd64`, AMD Ryzen 9
  7940HS, 16 logical CPUs, 30,582,380 KiB physical memory.
- Toolchain: `rustc 1.98.1`, `cargo 1.98.1`; release profile and locked
  dependencies.
- The machine, JDK, JMOD bytes, Rust/Cargo versions, and benchmark source
  checksum matched between before and after. The SDKMAN JDK was installed
  locally under ignored `target/`; no global SDKMAN setting was changed.
- Each sample is a new process. Linux's page cache was not flushed, so these
  are not cold-cache measurements. The before cohort's first initialization
  sample was 151.845 ms; its other nine ranged from 42.547 to 52.880 ms. This
  sample is retained in the raw results and accounts for the before p95. It is
  consistent with host/cache variance; the earlier #1157 baseline independently
  reported 39.453 / 41.067 ms init median / p95. No sample was discarded.
- RSS is `/proc/self/status` `VmHWM`, sampled after the complete workload.
  Concurrent latency is a per-process average over 1,000 operations, not an
  individual-operation latency distribution. These limits matter when
  interpreting p95 values from ten runs.

## Reproduction

The original #1157 baseline remains at
[`jdk-classpath-performance-baseline.md`](jdk-classpath-performance-baseline.md).
This issue reran the same workload with concurrent lookups and descriptor
counts added. Both builds used the same
[`jdk_classpath_benchmark.rs`](../crates/dotty-classloader/examples/jdk_classpath_benchmark.rs)
source (SHA-256 `93d95d3b8c251b93736944037f8072d97c3ccf81755a00dda9bd9cfe46f70de4`).

The before JMOD implementation is commit
`b5cb2d5b832ba40c6d2e5d9694c8f772f78401cc`; after is merge commit
`a88c94a733da2410b0d34d0438e464f323e76b56` (#1160). From the repository root:

```bash
export TMPDIR="$PWD/target/tmp-issue-1161"
JDK21="$PWD/target/sdkman-local/candidates/java/21.0.8-tem"

mkdir -p target/issue-1161/before-src target/issue-1161/before-target
git archive b5cb2d5b832ba40c6d2e5d9694c8f772f78401cc \
  | tar -x -C target/issue-1161/before-src
cp crates/dotty-classloader/examples/jdk_classpath_benchmark.rs \
  target/issue-1161/before-src/crates/dotty-classloader/examples/jdk_classpath_benchmark.rs
CARGO_TARGET_DIR="$PWD/target/issue-1161/before-target" \
  cargo build --release --locked \
    --manifest-path target/issue-1161/before-src/Cargo.toml \
    -p dotty-classloader --example jdk_classpath_benchmark

./tools/jdk-classpath-benchmark/run --jmods "$JDK21/jmods" --samples 10 \
  --binary "$PWD/target/issue-1161/before-target/release/examples/jdk_classpath_benchmark" \
  --source-revision b5cb2d5b832ba40c6d2e5d9694c8f772f78401cc \
  --purpose before-1160 --output docs/jdk-classpath-performance-before-1160-raw.json

./tools/jdk-classpath-benchmark/run --jmods "$JDK21/jmods" --samples 10 \
  --source-revision a88c94a733da2410b0d34d0438e464f323e76b56 \
  --purpose after-1160 --output docs/jdk-classpath-performance-after-1160-raw.json
```

The unaggregated JSON samples are checked in as
[`before raw`](jdk-classpath-performance-before-1160-raw.json) and
[`after raw`](jdk-classpath-performance-after-1160-raw.json). The report
tables reproduce the summary values emitted by the runner; each raw JSON file
contains its complete summary and all 10 unaggregated process samples.

## Regression gate and follow-up

The JDK 21 opt-in gate decoded classes from both `java.base` and
`java.net.http`, with their expected `ClassOrigin::Jmod`. The full classloader
fixture matrix continues to cover malformed/truncated archives, missing
entries, unsafe names, first-match-wins, package evidence, stable typed errors,
and concurrent/repeated reads. The #1156 frontend boundary checker remains
green, and frontend-only typing tests pass without `JAVA_HOME`.

Validation commands run for this report:

```bash
export TMPDIR="$PWD/target/tmp-issue-1161"
JDK21="$PWD/target/sdkman-local/candidates/java/21.0.8-tem"

cargo test -p dotty-classloader --all-targets --locked
DOTTY_JDK_JMODS="$JDK21/jmods" cargo test -p dotty-classloader \
  --test real_jdk_class_path --locked -- --ignored
env -u JAVA_HOME cargo test --test classpath_source_typing --locked
env -u JAVA_HOME cargo test -p dotty-typer --test typed_expressions --locked
JAVA_HOME="$JDK21" PATH="$JDK21/bin:$PATH" \
  cargo test --test jvm_source_compilation --locked -- --nocapture
python3 tools/check-frontend-boundaries.py
python3 tools/jdk-classpath-benchmark/test_run.py
./tools/jdk-classpath-benchmark/run --smoke
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test --workspace --all-targets --locked
git diff --check
```

The main demonstrated trade-off is the 69 long-lived JMOD descriptors. Lazy
central-directory opening could reduce that count and some metadata during
startup, at the cost of a first lookup that must open/index its module. A
class-to-JMOD index is not justified by these measurements: missing and
later-module lookup p95 did not regress materially. Decompressed-class caching
could target the roughly 2.1 ms repeated `java/lang/String` lookup, but is a
separate memory/semantics trade-off and is not part of this storage change.
