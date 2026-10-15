# JDK classpath performance baseline

This report is for the current, unmodified `JdkClassPath` implementation,
before archive storage changes. The reproducible runner is
[`tools/jdk-classpath-benchmark`](../tools/jdk-classpath-benchmark/README.md).
It measures JMOD loading and central-directory parsing during
`JdkClassPath::new`; class bytes are read/decompressed only when a matching
entry is looked up.

## Results

No real-JDK measurements are recorded in this checkout yet. The runner must be
executed against a real JDK 21 installation and its generated JSON committed
as `docs/jdk-classpath-performance-raw.json` before this baseline is complete.
Do not fill these tables from fixture JMODs or label a fresh process as a cold
OS-cache run.

| Measurement | Median | p95 | Unit / notes |
| --- | ---: | ---: | --- |
| `JdkClassPath::new` | pending real JDK run | pending real JDK run | ms, timed inside each fresh process |
| Peak process RSS | pending real JDK run | pending real JDK run | KiB, Linux `/proc/self/status` `VmHWM` |
| First positive class lookup | pending real JDK run | pending real JDK run | ms, by exact class in raw results |
| Warm positive class lookup | pending real JDK run | pending real JDK run | ms/op, 1,000 lookups per class/process |
| First negative class lookup | pending real JDK run | pending real JDK run | ms |
| Warm negative class lookup | pending real JDK run | pending real JDK run | ms/op, 1,000 lookups per process |
| Direct package query | pending real JDK run | pending real JDK run | ms/op, 1,000 queries per process |
| Package-prefix query | pending real JDK run | pending real JDK run | ms/op, 1,000 queries per process |

The class cohort is `java/lang/Object`, `java/lang/String`,
`java/util/ArrayList`, `java/util/concurrent/CompletableFuture`, and
`java/net/http/HttpClient`. The first four must resolve from `java.base.jmod`
and the last from `java.net.http.jmod`. The missing name is
`dotty_issue_1157/DefinitelyMissing`. Every expected success and expected
miss is asserted by the executable.

## Method and rerun

Use the same JDK distribution/build, machine, power profile and release binary
for before/after comparisons. The runner launches one process per sample and
requires at least ten samples. Processes are independent; the OS file cache is
left alone and will normally be warm after early runs. The initialization
measurement covers the call to `JdkClassPath::new` only. Per-process RSS covers
the whole example process and is reported in KiB from the Linux
`/proc/self/status` `VmHWM` high-water mark read after the workload.

```bash
./tools/jdk-classpath-benchmark/run --jmods "$JAVA_HOME/jmods" \
  --samples 10 --output docs/jdk-classpath-performance-raw.json
```

The raw JSON contains each process's timings and RSS, machine/OS and CPU,
available memory, JDK version string, JMOD count and total byte size, and
Rust/Cargo versions. It intentionally stores no absolute `JAVA_HOME` path.
For tool validation without an installed JDK, run
`./tools/jdk-classpath-benchmark/run --smoke` and
`python3 tools/jdk-classpath-benchmark/test_run.py`; these fixture-backed
checks are not performance measurements.
