# Issue #1769 benchmark evidence

The compressed evidence archive linked from the parent report contains the raw
Cargo test-list log, Nextest JSON test list, exact inventory TSV files, run
logs, and per-run JSON measurements. The archive is a gzip-compressed tar file
so the raw outputs remain inspectable without adding the uncompressed files to
the repository.

`compare-inventories.py` compares the Cargo and Nextest lists. It uses the
compiled test-binary filename, test name, and test kind as the identity and
writes sorted inventories plus missing/additional sets. The recorded run found
7,089 identities for each tool, 18 ignored tests, and no differences.

`run-benchmark.py` runs one full workspace command and writes its combined
stdout/stderr log and a JSON record containing its exact command, exit status,
wall time, JDK, host load averages, and sampled process RSS. It accepts
`libtest|nextest`, `default|2|4`, and a repetition label. Set `BENCH_TARGET_DIR`
to an empty path for a clean-target build-and-test measurement. Its environment
matches the checked-in Rust CI test profile: `CARGO_INCREMENTAL=0`,
`CARGO_PROFILE_TEST_DEBUG=0`, `--workspace --all-targets --locked`, and no test
filter. It creates the workspace-local `TMPDIR` itself. The driver defaults to
the recorded JDK path; set `BENCH_JAVA_HOME` if JDK 21 is installed elsewhere.

To install the exact runner and reproduce inventory comparison on Linux x86_64
GNU, run from the repository root:

```sh
mkdir -p .benchmark-1769/tool .benchmark-1769/tmp .benchmark-1769/evidence
curl -fL 'https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.150/cargo-nextest-0.9.150-x86_64-unknown-linux-gnu.tar.gz' \
  -o .benchmark-1769/tool/cargo-nextest-0.9.150-x86_64-unknown-linux-gnu.tar.gz
echo 'b0959bc64232974224cf9fb22c915845398c0e8668fa7831eb5b43fb1eef781f  .benchmark-1769/tool/cargo-nextest-0.9.150-x86_64-unknown-linux-gnu.tar.gz' | sha256sum -c -
tar -xzf .benchmark-1769/tool/cargo-nextest-0.9.150-x86_64-unknown-linux-gnu.tar.gz -C .benchmark-1769/tool
export CARGO_TARGET_DIR="$PWD/.benchmark-1769/target" CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_TERM_COLOR=never
export BENCH_JAVA_HOME=/path/to/jdk-21
cargo test --workspace --all-targets --locked -- --list > .benchmark-1769/evidence/libtest-list.log 2>&1
PATH="$PWD/.benchmark-1769/tool:$PATH" cargo nextest list --workspace --all-targets --locked --message-format json > .benchmark-1769/evidence/nextest-list.json 2> .benchmark-1769/evidence/nextest-list.stderr
python3 docs/ci-performance-baseline/issue-1769-evidence/compare-inventories.py \
  .benchmark-1769/evidence/libtest-list.log \
  .benchmark-1769/evidence/nextest-list.json \
  .benchmark-1769/evidence
```

After verifying that the listing is identical, run three repetitions per
default runner, then the Nextest concurrency probes:

```sh
python3 docs/ci-performance-baseline/issue-1769-evidence/run-benchmark.py libtest default 1
python3 docs/ci-performance-baseline/issue-1769-evidence/run-benchmark.py libtest default 2
python3 docs/ci-performance-baseline/issue-1769-evidence/run-benchmark.py libtest default 3
python3 docs/ci-performance-baseline/issue-1769-evidence/run-benchmark.py nextest default 1
python3 docs/ci-performance-baseline/issue-1769-evidence/run-benchmark.py nextest default 2
python3 docs/ci-performance-baseline/issue-1769-evidence/run-benchmark.py nextest default 3
python3 docs/ci-performance-baseline/issue-1769-evidence/run-benchmark.py nextest 4 1
python3 docs/ci-performance-baseline/issue-1769-evidence/run-benchmark.py nextest 2 1
rm -rf .benchmark-1769/cold-libtest-target .benchmark-1769/cold-nextest-target
BENCH_TARGET_DIR="$PWD/.benchmark-1769/cold-libtest-target" python3 docs/ci-performance-baseline/issue-1769-evidence/run-benchmark.py libtest default clean
BENCH_TARGET_DIR="$PWD/.benchmark-1769/cold-nextest-target" python3 docs/ci-performance-baseline/issue-1769-evidence/run-benchmark.py nextest default clean
```

The benchmark was run on Linux x86_64 with Rust 1.98.1, Temurin JDK 21.0.8+9,
and prebuilt cargo-nextest 0.9.150. The archive checksum and full invocation
context are in `environment.txt` inside the evidence archive. Memory sampling
is approximate: it polls `/proc` every 100 ms and the process-tree sum may
double-count shared pages.
