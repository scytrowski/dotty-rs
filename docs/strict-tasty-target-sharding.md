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

## Initial hosted four-shard candidate

The first candidate run is [41873333835](https://github.com/scytrowski/dotty-rs/actions/runs/41873333835)
on SHA `864b6daa559dda2d7384b27fe35d841bfc3bde38`. All four shard jobs
started at 17:01:09 UTC, confirming runner concurrency. Their recorded total
job intervals were 11m02s, 12m41s, 10m46s, and 21m31s (about 56 runner
minutes). Scala staging took 4m58s, 5m47s, 4m54s, and 7m49s respectively;
strict test steps took 5m03s, 6m02s, 5m04s, and 7m28s. Shard 4's four
follow-up gates and verifier took a further 5m24s.

The first three job checks completed successfully. Every recorded step on
shard 4, including its strict tests, all follow-up gates, and the Java 21
verifier, also completed successfully by 17:22:40 UTC. GitHub continued to
report shard 4 as in progress, however, and left the aggregate queued. The
run therefore has no complete PR-to-green measurement. Full Rust/JDK CI
completed successfully at 17:13:36 UTC.

This first run is recorded separately from warm comparisons. The run did not
finalize, so its cache hit flags and byte transfers are unavailable. GitHub's
cache API shows candidate-ref entries of 643,398,136 bytes for Scala build
targets and 849,106,509 bytes for dependencies/Rust build, but these sizes do
not establish how much each shard restored. Compare warm runs on this PR cache
scope before deciding whether the added four-way setup is justified.
