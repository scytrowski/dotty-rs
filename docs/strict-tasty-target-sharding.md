# Strict Scala/TASTy target sharding

[`tools/ci/strict_tasty_targets.json`](../tools/ci/strict_tasty_targets.json)
records the full 24-target strict test inventory, including
`jvm_tasty_immutable_reference_local_pair_scala_consumer`, and assigns every
target exactly once to the active fixed shards. Membership is independently
declared in the workflow. The validator checks that declaration against the
manifest, verifies real test files and strict environment coverage, and scans
Cargo invocations so a newly required target cannot be left unassigned.

The active required check is the two-shard workflow from #2118. Both shards use
the pinned Scala 3.9.0 source at
`777528f19a58e794c9954a42f433373472ec57f8`, JDK `21.0.13+9.0.LTS`, Rust
`1.98.1`, the existing artifact hashes and pinned cache keys. Shard 1 is the
single dependency-cache save owner. Both shards retain `--locked`,
`--nocapture`, and `--test-threads=1`. The four mandatory follow-up gates run
once after shard 2. The aggregate retains the required check identity and fails
for unsuccessful, cancelled, skipped, or missing shard or follow-up results.

## Comparison baseline

The comparable serial baseline is hosted run
[41819990007](https://github.com/scytrowski/dotty-rs/actions/runs/41819990007)
on PR #2115's tested SHA `b02e1c781bc9120cbd314525322e9e899c75a865`. It
covered all 24 targets, including the immutable-reference local-pair consumer.
The strict step took 1,498 seconds and the complete job took 1,950 seconds.

PR #2118 reports three successful warm two-shard reruns on one fixed SHA, each
with the dependency/Rust and Scala target caches restored. PR-to-green times
were 18m00s, 21m06s, and 22m25s (mean 20m30s); combined shard runner time was
31m56s, 36m47s, and 37m19s (mean 35m21s). The cold first run (28m22s) and cold
dispatch run were excluded. These measurements establish that two-way sharding
improved warm latency over serial while increasing runner time by about 9%.

## Four-shard evaluation

A four-shard candidate must keep the same 24-target inventory, commands,
compiler/JDK/Rust pins, checksums, strict environment, follow-up gates,
aggregate check, and single cache-save owner. Its target groups are fixed in
the manifest using measured target durations. Validation must reject omitted,
duplicate, unknown, and unassigned targets; invalid shard selections; missing
strict environment variables; and any shard or follow-up failure, cancellation,
skip, or missing output.

Adoption requires hosted evidence that all targets and follow-up gates ran
exactly once, the four jobs actually ran concurrently, and at least three
comparable warm runs can be compared with the two-way results. Record candidate
SHA and run links, per-shard wall times, critical-path time, cache hit state and
bytes, setup/staging and transfer cost, PR-to-green, and total runner-minutes.
Keep cold/cache-miss runs separate. Retain two-way if completeness or
concurrency cannot be proven, or if the measured end-to-end latency improvement
does not justify added runner and cache overhead.

The validator and workflow retain manifest-driven target selection:

```sh
python3 tools/ci/validate_strict_tasty_targets.py
python3 tools/ci/validate_strict_tasty_targets.py --shard shard-1
python3 tools/ci/validate_strict_tasty_targets.py --shard shard-2
python3 tools/ci/validate_strict_tasty_targets.py --shard shard-3
python3 tools/ci/validate_strict_tasty_targets.py --shard shard-4
```
