# Strict Scala/TASTy target sharding

[`tools/ci/strict_tasty_targets.json`](../tools/ci/strict_tasty_targets.json)
records all 24 strict test targets and assigns each exactly once across two
shards using deterministic longest-processing-time-first ordering. Target
membership is independently declared in the aggregate job of the workflow;
the validator compares that list to the manifest and scans literal Cargo
`--test` invocations so an added required invocation absent from the manifest
fails validation. It also checks the support-target allowlist and required
strict environment variables in both shard jobs. The obsolete 23-target
baseline file is removed.

```sh
python3 tools/ci/validate_strict_tasty_targets.py
python3 tools/ci/validate_strict_tasty_targets.py --shard shard-1
python3 tools/ci/validate_strict_tasty_targets.py --shard shard-2
```

The workflow runs exactly two shard jobs. Each uses Scala 3.9.0 at
`777528f19a58e794c9954a42f433373472ec57f8`, JDK
`21.0.13+9.0.LTS`, Rust `1.98.1`, the existing artifact hashes and pinned
cache keys. Shard 1 owns all cache saves; shard 2 only restores. Both retain
`--locked`, `--nocapture`, and `--test-threads=1`. The four required follow-up
gates run once after shard 2. The aggregate preserves the existing required
check name and fails for unsuccessful, cancelled, skipped, or missing shard or
follow-up results.

## Comparable serial baseline

The baseline comes from successful hosted run
[41819990007](https://github.com/scytrowski/dotty-rs/actions/runs/41819990007),
job `126540913351`, on PR #2115's tested merge SHA
`b02e1c781bc9120cbd314525322e9e899c75a865`. It ran the complete current
24-target inventory, including
`jvm_tasty_immutable_reference_local_pair_scala_consumer` (172.37 seconds).
Per-target measurements are recorded in the manifest from Cargo/libtest final
result lines. Their sum is 1,421.90 seconds; Cargo test build took 76 seconds,
the strict serial step took 1,498 seconds, and the full job took 1,950 seconds.
The hosted logs confirm Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`, JDK 21.0.13, the pinned Rust
1.98.1 toolchain, exact Maven and Scala-target cache hits, and the expected
cache-save skip on a target-cache hit. The shard assignments balance measured
test time at 712.72 and 709.18 seconds. The predicted candidate critical path
must also include repeated build/setup, cache transfer and four follow-up
gates, so the estimate alone does not establish a speedup.

The required gate remains serial until hosted runs on a fixed candidate SHA
prove exactly-once target and follow-up execution, all pins, safe failure and
cancellation behavior, and comparable warm-run benefit. Record setup,
staging,
cache transfer and hit state, job wall time, PR-to-last-green, and total
runner-minutes. Retain the serial gate if completeness or benefit is not
proven.
