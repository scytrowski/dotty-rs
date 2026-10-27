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
filter. `TMPDIR` is workspace-local because the host's `/tmp` was full.

The benchmark was run on Linux x86_64 with Rust 1.98.1, Temurin JDK 21.0.8+9,
and prebuilt cargo-nextest 0.9.150. The archive checksum and full invocation
context are in `environment.txt` inside the evidence archive. Memory sampling
is approximate: it polls `/proc` every 100 ms and the process-tree sum may
double-count shared pages.
