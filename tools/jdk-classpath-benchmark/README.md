# JDK classpath baseline runner

`run` builds one release example binary (or accepts a prebuilt binary with
`--binary`), launches it in at least ten independent processes, measures
maximum resident set size from Linux `/proc/self/status` `VmHWM`, counts open
file descriptors after initialization and after the workload, and writes the
unaggregated JSON samples plus median/p95 summaries. It never flushes the OS
page cache: these are fresh processes, not cold-cache runs.

Run against an installed JDK 21:

```bash
./tools/jdk-classpath-benchmark/run --jmods "$JAVA_HOME/jmods" \
  --samples 10 --source-revision "$(git rev-parse HEAD)" \
  --purpose after-1160 --output docs/jdk-classpath-performance-raw.json
```

If `--jmods` is omitted, `JAVA_HOME/jmods` is used. The runner requires a
Linux host with `/proc/self/status`, Python 3, Cargo, Rust, and an
executable JDK 21 `java` (used to record and verify the JDK version). The
output captures machine/kernel/CPU and memory details, Rust/Cargo/JDK
versions, source revision, JMOD count and total bytes, per-process timings,
RSS, and descriptor counts. It does not preserve the machine-specific JDK path.

The workload includes initialization, first/sequential repeated lookups for
classes in `java.base` and `java.net.http`, a missing-class lookup, direct and
descendant package evidence, and synchronized four-thread lookups for an early
class, a later-module class, and a miss.

Run the JDK-free harness smoke path using checked-in tiny JMOD fixtures:

```bash
./tools/jdk-classpath-benchmark/run --smoke
python3 tools/jdk-classpath-benchmark/test_run.py
```

The smoke run proves the release example launches and checks two positive
class lookups, a negative lookup, a direct package query, and a package-prefix
query. `--samples` cannot be set below 10 for a real baseline. Peak RSS is
read in KiB from `VmHWM` after the process completes its full workload.

## Module metadata and independent sessions (#1201)

The original class workload and default driver invocation remain unchanged.
Use the separate metadata mode after the catalog API is available:

```bash
./tools/jdk-classpath-benchmark/run --metadata --jmods "$JAVA_HOME/jmods" \
  --samples 10 --source-revision "$(git rev-parse HEAD)" \
  --purpose module-metadata --output docs/jdk-module-metadata-performance-raw.json
```

The runner launches a semantic preflight process, then interleaves ten rounds
of **1, 2, and 4 independent classpath instances**, each cohort sample in its
own fresh process (30 samples total). Instances in a sample share JMOD files
on disk but own their archive indexes/handles. Nothing is pooled or cached by
the production implementation. The preflight verifies all catalog origins,
module count/order, `java.base` version, `java.net.http` dependencies/exports,
`java.desktop` uses/provides, and a missing module. Samples repeat semantic
checks outside the first-catalog timer.

Metadata raw JSON uses schema 2 with `cohorts["1"|"2"|"4"]`, each containing
its summary and raw samples. The schema 1 class report is retained. Resource
checkpoints record current `VmRSS`, high-water `VmHWM`, and `/proc/self/fd`
counts before construction, after all instances initialize, while all initial
catalogs are held, after sequential/concurrent calls, after dropping catalogs,
and after dropping instances. FD enumeration includes its own temporary FD.
RSS includes allocator retention and thread stacks; dropping an object need
not lower RSS or the high-water mark. Paired resource deltas are summarized
from individual processes, not differences of percentiles.

Each instance's first full `module_catalog()` is timed before any metadata
access on that instance. This API decodes **all modules**, even if the caller
only wants one descriptor. The subsequent first timed `get()` and 10,000
repeated gets operate on the retained snapshot (already semantically checked).
For `java.base`, `java.net.http`, `java.desktop`, and a missing module, another
20 calls each time `module_catalog().get(name)` on instance 0; temporary
snapshot destruction is outside that sequential read+get timer. Snapshot
enumeration walks every module name 10,000 times; full fresh enumeration cost
is represented by the catalog timings.

Four synchronized threads each request ten fresh catalogs, first sharing
instance 0 and then using instances round-robin. Concurrency timing includes
snapshot destruction and joining, starts before the release gate, and is
reported as elapsed time / 40 operations. This is **amortized wall time**, not
individual request latency. Median/p95 are across process-level averages;
nearest-rank p95 with ten processes is the maximum, not a per-query tail.

The runner records the soft/hard `RLIMIT_NOFILE` (Linux `ulimit -Sn/-Hn`), JMOD
SHA-256 values, binary checksum, toolchains and default release profile plus
build overrides. It rejects incomplete rows, unexpected field/count layouts,
and failed children. Neither mode flushes the OS page cache; environment
fingerprinting/preflight warms it. Run without competing builds and retain all
samples. Smoke now tests both class lookup and catalog projection from the
checked-in class files packaged into temporary synthetic JMODs using Python's
standard ZIP writer; neither smoke path invokes Java.
