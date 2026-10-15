# JDK classpath performance baseline

This report is for the current, unmodified `JdkClassPath` implementation,
before archive storage changes. The reproducible runner is
[`tools/jdk-classpath-benchmark`](../tools/jdk-classpath-benchmark/README.md).
It measures JMOD loading and central-directory parsing during
`JdkClassPath::new`; class bytes are read/decompressed only when a matching
entry is looked up.

## Results

Measured 2026-10-15 on Temurin 21.0.8+9-LTS. The raw 10-process records and
machine metadata are in [`jdk-classpath-performance-raw.json`](jdk-classpath-performance-raw.json).
Each value below is median / p95. Latency is milliseconds except package
queries, which are microseconds per operation.

| Measurement | Median | p95 |
| --- | ---: | ---: |
| `JdkClassPath::new` | 39.453 ms | 41.067 ms |
| Peak process RSS | 97,468 KiB | 97,560 KiB |
| First lookup `java/lang/Object` | 0.171 ms | 0.209 ms |
| Repeated lookup `java/lang/Object` | 0.150 ms/op | 0.155 ms/op |
| First lookup `java/lang/String` | 2.141 ms | 2.198 ms |
| Repeated lookup `java/lang/String` | 2.114 ms/op | 2.123 ms/op |
| First lookup `java/util/ArrayList` | 0.776 ms | 0.820 ms |
| Repeated lookup `java/util/ArrayList` | 0.774 ms/op | 0.779 ms/op |
| First lookup `java/util/concurrent/CompletableFuture` | 1.592 ms | 2.108 ms |
| Repeated lookup `java/util/concurrent/CompletableFuture` | 1.570 ms/op | 1.579 ms/op |
| First lookup `java/net/http/HttpClient` | 0.186 ms | 0.189 ms |
| Repeated lookup `java/net/http/HttpClient` | 0.177 ms/op | 0.180 ms/op |
| First negative lookup | 0.039 ms | 0.042 ms |
| Repeated negative lookup | 0.0051 ms/op | 0.0053 ms/op |
| `contains_package(java/lang)` | 0.074 µs/op | 0.181 µs/op |
| `contains_package_prefix(java/net/http)` | 0.386 µs/op | 0.440 µs/op |
| `contains_package_prefix(java/util/concurrent)` | 0.062 µs/op | 0.063 µs/op |

The workload performed 1,000 repeated class lookups per class, 1,000 repeated
negative lookups, and 1,000 of each package query in every process. Repeated
means the same process and classpath instance; it does not imply a class-byte
cache in `JdkClassPath`.

The class cohort is `java/lang/Object`, `java/lang/String`,
`java/util/ArrayList`, `java/util/concurrent/CompletableFuture`, and
`java/net/http/HttpClient`. The first four must resolve from `java.base.jmod`
and the last from `java.net.http.jmod`. The missing name is
`dotty_issue_1157/DefinitelyMissing`. Every expected success and expected
miss is asserted by the executable.

## Method and rerun

Environment: Linux Debian 13, kernel `6.12.74+deb13+1-amd64`, AMD Ryzen 9
7940HS with Radeon 780M Graphics, 16 logical CPUs, and 30,582,380 KiB reported
physical memory. The JDK contained 69 JMOD files totaling 86,516,827 bytes.
The release binary was built with `rustc 1.98.1` and `cargo 1.98.1`. Full
compiler version details are in the raw JSON.

The runner launches one process per sample and requires at least ten samples.
Processes are independent; the OS file cache is left alone and will normally
be warm after early runs. These are fresh-process runs, not cold-cache runs.
Initialization timing covers only `JdkClassPath::new`. Per-process RSS is the
Linux `/proc/self/status` `VmHWM` high-water mark in KiB, read after the
workload. JDK class decompression remains lazy and occurs during the measured
class lookups.

The measured Temurin 21.0.8 JDK was installed with SDKMAN into the ignored
worktree `target/` directory, leaving the global SDKMAN setup unchanged. The
exact benchmark command was:

```bash
export TMPDIR="$PWD/target/tmp"
JDK21="$PWD/target/sdkman-local/candidates/java/21.0.8-tem"
./tools/jdk-classpath-benchmark/run --jmods "$JDK21/jmods" \
  --samples 10 --output docs/jdk-classpath-performance-raw.json
```

For another SDKMAN installation, set `JAVA_HOME` to its JDK 21 home and pass
`"$JAVA_HOME/jmods"` in the same command.

The runner builds this release executable before sampling:

```bash
cargo build --release --locked -p dotty-classloader --example jdk_classpath_benchmark
```

The raw JSON contains each process's
timings and RSS, machine/OS and CPU, memory, JDK distribution/version, JMOD
count and total byte size, and Rust/Cargo versions. It intentionally stores no
absolute `JAVA_HOME` path.
For tool validation without an installed JDK, run
`./tools/jdk-classpath-benchmark/run --smoke` and
`python3 tools/jdk-classpath-benchmark/test_run.py`; these fixture-backed
checks are not performance measurements.
