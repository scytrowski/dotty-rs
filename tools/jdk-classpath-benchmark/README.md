# JDK classpath baseline runner

`run` builds one release example binary, launches it in at least ten
independent processes, measures maximum resident set size from Linux
`/proc/self/status` `VmHWM`, and writes the unaggregated JSON samples plus
median/p95 summaries. It never flushes the OS page cache: these are fresh
processes, not cold-cache runs.

Run against an installed JDK 21:

```bash
./tools/jdk-classpath-benchmark/run --jmods "$JAVA_HOME/jmods" \
  --samples 10 --output docs/jdk-classpath-performance-raw.json
```

If `--jmods` is omitted, `JAVA_HOME/jmods` is used. The runner requires a
Linux host with `/proc/self/status`, Python 3, Cargo, Rust, and an
executable JDK 21 `java` (used to record and verify the JDK version). The
output captures machine/kernel/CPU and memory details, Rust/Cargo versions,
JMOD count and total bytes, per-process timings and RSS. It does not preserve
the machine-specific JDK path.

Run the JDK-free harness smoke path using checked-in tiny JMOD fixtures:

```bash
./tools/jdk-classpath-benchmark/run --smoke
python3 tools/jdk-classpath-benchmark/test_run.py
```

The smoke run proves the release example launches and checks two positive
class lookups, a negative lookup, a direct package query, and a package-prefix
query. `--samples` cannot be set below 10 for a real baseline. Peak RSS is
read in KiB from `VmHWM` after the process completes its full workload.
