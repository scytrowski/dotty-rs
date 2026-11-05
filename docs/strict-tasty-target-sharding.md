# Strict Scala/TASTy target sharding

[`tools/ci/strict_tasty_targets.json`](../tools/ci/strict_tasty_targets.json)
records the full 24-target strict test inventory, including
`jvm_tasty_immutable_reference_local_pair_scala_consumer`, and assigns every
target exactly once to the active fixed shards. Membership is independently
declared in the workflow. The validator checks that declaration against the
manifest, verifies real test files and strict environment coverage, and scans
Cargo invocations so a newly required target cannot be left unassigned.

The active required check uses four fixed shards. All shards use the pinned
Scala 3.9.0 source at `777528f19a58e794c9954a42f433373472ec57f8`, JDK
`21.0.13+9.0.LTS`, Rust `1.98.1`, the existing artifact hashes and pinned
cache keys. Shard 1 is the single dependency-cache save owner. All shards
retain `--locked`, `--nocapture`, and `--test-threads=1`. The four mandatory
follow-up gates run once after shard 4. The aggregate retains the required
check identity and fails for unsuccessful, cancelled, skipped, or missing
shard or follow-up results.

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
dispatch run were excluded. These measurements establish that two-way
sharding improved warm latency over serial while increasing runner time by
about 9%.

## Four-shard evaluation

The four fixed target groups use measured target durations and preserve the
same inventory, commands, compiler/JDK/Rust pins, checksums, strict
environment, follow-up gates, aggregate check, and single cache-save owner.
The validator and workflow reject omitted, duplicate, unknown, and unassigned
targets; invalid shard selections; missing strict environment variables; and
any shard or follow-up failure, cancellation, skip, or missing output.

The candidate is commit `864b6daa559dda2d7384b27fe35d841bfc3bde38` on PR
[#2121](https://github.com/scytrowski/dotty-rs/pull/2121). Its first hosted
attempt was cold and is excluded from the warm comparison. It completed all
four shards and follow-up gates successfully; the exact Scala-target and
dependency/Rust caches missed, and shard 1 populated the candidate-ref cache.
Its approximately 56 runner-minutes are useful as a cold-cache cost signal,
not as a comparison with warm two-way runs. Full Rust/JDK CI also passed on
the candidate revision.

Three reruns of that same SHA completed successfully with the exact pinned
Scala-target, dependency/Rust, and sbt-runner caches restored in all four
jobs. The sbt disk cache was absent in each run. All four jobs started
concurrently in each attempt, and the aggregate plus all follow-up gates
passed.

| Attempt | PR-to-green | Shard 1 | Shard 2 | Shard 3 | Shard 4 | Total shard runner time |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| [2](https://github.com/scytrowski/dotty-rs/actions/runs/41873333835/attempts/2) | 14m14s | 6m55s | 6m01s | 8m56s | 13m23s | 35m15s |
| [3](https://github.com/scytrowski/dotty-rs/actions/runs/41873333835/attempts/3) | 16m50s | 8m34s | 9m06s | 6m26s | 14m43s | 38m49s |
| [4](https://github.com/scytrowski/dotty-rs/actions/runs/41873333835/attempts/4) | 14m23s | 8m30s | 6m57s | 8m39s | 14m07s | 38m13s |

The warm four-way mean PR-to-green was 15m09s, 5m21s (about 26%) faster
than the two-way mean. Mean shard runner time was 37m26s, 2m05s (about 6%)
higher. The critical path is shard 4, which owns the follow-up gates; its
mean job time was 14m04s. Every warm run restored four copies of the pinned
849,106,509-byte dependency/Rust cache and 643,398,136-byte Scala target
cache, in addition to the 54,651,577-byte sbt-runner cache. These recorded
archive sizes make the added cache traffic explicit: about 6.2 GB across the
four jobs per run, before accounting for transport overhead.

## Decision

**Adopt the four-shard gate.** The three complete, comparable warm runs show a
repeatable end-to-end improvement that justifies the measured 6% increase in
runner time and additional cache traffic. The first cold attempt remains
separate evidence and is not included in those means.

The active validator supports each fixed shard selection:

```sh
python3 tools/ci/validate_strict_tasty_targets.py
python3 tools/ci/validate_strict_tasty_targets.py --shard shard-1
python3 tools/ci/validate_strict_tasty_targets.py --shard shard-2
python3 tools/ci/validate_strict_tasty_targets.py --shard shard-3
python3 tools/ci/validate_strict_tasty_targets.py --shard shard-4
```
