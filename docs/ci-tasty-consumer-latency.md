# Strict Scala/TASTy consumer latency evidence

This report audits the current 24-target required Scala/TASTy gate and keeps the
original 23-target observation as a separate historical cohort. It records
workflow wall time and runner cost, but does **not** claim that sharding has
proven an end-to-end PR-to-last-green speedup: the evidence does not provide
three comparable serial samples or three measured PR-level latency samples per
layout.

## Cohort and provenance ledger

Issue [#2102](https://github.com/scytrowski/dotty-rs/issues/2102) was written
when the required set had 23 targets. The later 24th target,
`jvm_tasty_immutable_reference_local_pair_scala_consumer`, came from the
separate immutable-local Scala consumer swimlane: it was added in
[PR #2115](https://github.com/scytrowski/dotty-rs/pull/2115) for
[#2075](https://github.com/scytrowski/dotty-rs/issues/2075). The current
[manifest](../tools/ci/strict_tasty_targets.json) and
[workflow](../.github/workflows/jvm-tasty-factory-scala-consumer.yml) each
record all 24 names and the fixed four-way partition. We retain the 23-target
sample below as historical evidence; it is not used as a 24-target baseline.

| Cohort / sample | Tested source | Exact suite and cache | Result and use |
| --- | --- | --- | --- |
| Historical serial: [run 41734662249](https://github.com/scytrowski/dotty-rs/actions/runs/41734662249) | `356bb699888cd89bf106f3a1f012ccaa643a7112` | 23 targets, before the immutable-local consumer was introduced; old timing/manifest context | Successful, 31m45s whole job and 24m07s strict step per [#2092](https://github.com/scytrowski/dotty-rs/issues/2092). Historical only; not compared numerically to the current suite. |
| Current serial: [run 41819990007](https://github.com/scytrowski/dotty-rs/actions/runs/41819990007) | PR head `dbf93dec536c75ae9b792f51ae290be249c4818e`; tested merge `b02e1c781bc9120cbd314525322e9e899c75a865` | 24 targets, all once; pinned setup and exact Scala build outputs restored; serial command keeps `--locked -- --nocapture --test-threads=1` | Successful; 24m58s strict step and 32m30s full job. One current-suite serial observation. |
| Two-way warm series: [run 41833599053](https://github.com/scytrowski/dotty-rs/actions/runs/41833599053), attempts 2–4 | Fixed PR head `43b89c97ae94acf0b49c0926a11fb734c90c1755`; attempt merge `e9bcb1eae10c0f92cb3e21b4f820005ca9eba3c4` | Same 24 names once across 12+12; dependency/Rust and Scala-target caches restored; attempts were warm; follow-ups ran on shard 2 | Three successful fixed-SHA attempts. The recorded 18m00s, 21m06s and 22m25s are attempt-to-green durations, not PR-open-to-green measurements. |
| Four-way warm series: [run 41873333835](https://github.com/scytrowski/dotty-rs/actions/runs/41873333835), attempts 2–4 | Fixed PR head `864b6daa559dda2d7384b27fe35d841bfc3bde38`; attempt merge `5b3e932869f3923e6dc3c7ed0f3a7c8e4d183e2f` | Same 24 names once across 6+5+6+7; exact pinned Scala, dependency/Rust and sbt-runner caches restored; sbt disk cache absent; follow-ups ran on shard 4 | Three successful fixed-SHA attempts. The recorded 14m14s, 16m50s and 14m23s are attempt-to-green durations, not PR-open-to-green measurements. |

The serial and sharded samples share the target-name inventory, required follow-up gates, Scala source revision `777528f19a58e794c9954a42f433373472ec57f8`, JDK `21.0.13+9.0.LTS`, Rust `1.98.1`, strict environment, checksum verification and single-threaded Cargo test profile. The parallel sample trees also include timing-wrapper instrumentation in two test files. The tested 2-/4-way trees contain the same strict test files; their changes between each other are workflow/report changes. The serial tested merge predates a separate `immutable_local_pair_batch_publisher_rejects_partial_and_unsafe_outputs` test in `tests/jvm_tasty_artifacts.rs`. These bounded differences are recorded rather than hidden; they mean the samples are suite-name and gate equivalent, not byte-identical source snapshots.

## What the successful runs establish

The manifest and workflow independently enumerate the suite. Successful run logs show every current target exactly once in each layout: 24 serial invocations, 12+12 in two-way, and 6+5+6+7 in four-way. The immutable-local target is present exactly once in all three. Every successful parallel run also reports each of the four mandatory follow-ups once: constructor PARAM address verification, secondary-constructor rejection, dual-reference reader, and the Java 21 verifier/publisher gate. All required shards and aggregate checks concluded success.

The [two-way rollout PR](https://github.com/scytrowski/dotty-rs/pull/2118) records validator and aggregate regression cases for success, failure, cancellation, skip and missing results. Its aggregate requires every shard and follow-up result. The four-way rollout reused the validator/aggregate contract and retained those negative cases. The safe negative-control evidence is these local validator/aggregation tests; no hosted failing-shard run was manufactured.

The serial evidence run and all selected parallel workflow runs used the pinned Scala 3.9.0 source, JDK 21.0.13 and Rust 1.98.1. The workflow verifies the Scala source SHA and pinned compiler artifact SHA-256 values before the tests. The manifest records the per-target serial sample and profile remains `--test-threads=1`; no Rust profile or runtime test was changed for this report.

## Latency and runner cost

The current serial run provides one 24-target observation: 32m30s whole-job wall, of which the strict Cargo step took 24m58s (1,498s); the remaining job time includes setup, cache/artifact work, Scala staging and mandatory follow-ups. It cannot establish a serial distribution.

The existing two-way report's three warm attempt-to-green values average 20m30s. Their combined shard runner time was 31m56s, 36m47s and 37m19s (mean 35m21s). The initial cold attempt, 28m22s, and the cold workflow-dispatch run are excluded from that warm mean. The run report records dependency/Rust and Scala target cache restores for all three warm attempts.

The four-way report's three warm attempt-to-green values average 15m09s. Per-attempt shard runner totals were 35m15s, 38m49s and 38m13s (mean 37m26s); the aggregate job adds only a few seconds of runner time. All four shard jobs started concurrently. The critical shard is shard 4, which owns the follow-up gates; its times were 13m23s, 14m43s and 14m07s (mean 14m04s). Four copies per attempt of the pinned 849,106,509-byte dependency/Rust cache and 643,398,136-byte Scala-target cache, plus the 54,651,577-byte sbt-runner cache, amount to about 6.2 GB of archive traffic before transport overhead. The sbt disk cache was absent in each warm attempt. The first cold four-way attempt is separate and is not included.

Thus the hosted data gives a useful attempt-level signal: the three four-way attempts were faster than the three two-way attempts by a mean 5m21s (about 26%), while adding about 2m05s (about 6%) of shard runner time and more concurrent cache transfer. The comparison keeps fixed-SHA warm attempts together and cold attempts separate.

That signal is **not** the issue's requested PR-to-last-green result. The table values above measure a workflow attempt from start to its aggregate result. For example, PR #2118 was opened at 12:03Z, while its selected later successful workflow run #41862325198 started at 15:53Z; PR #2121 was opened at 17:01Z, while selected later run #41888319758 started at 18:33Z. The intervals include author iteration, workflow scheduling and other PR activity, so they cannot be substituted for an attempt duration or treated as repeated comparable latency samples. The 24-target serial cohort has only one run, and no three matched PR-level serial observations are available. We therefore make no quantitative claim that two- or four-way sharding improved PR-to-last-green latency.

## Serial target profile

The six slowest target durations in the current serial sample, from the manifest's Cargo/libtest final-result lines, were:

| Rank | Target | Seconds |
| ---: | --- | ---: |
| 1 | `jvm_tasty_mixed_origin_constructor_scala_consumer` | 210.63 |
| 2 | `jvm_tasty_immutable_reference_local_pair_scala_consumer` | 172.37 |
| 3 | `jvm_tasty_dual_reference_forwarding_scala_consumer` | 140.47 |
| 4 | `jvm_tasty_multi_argument_constructor_scala_consumer` | 109.85 |
| 5 | `jvm_tasty_dual_reference_rust_consumer` | 90.07 |
| 6 | `jvm_tasty_reference_local_scala_consumer` | 87.71 |

These are the serial sample's per-target execution durations, not complete workflow critical-path times. Four-way's observed critical shard duration includes setup and shard 4's mandatory follow-ups; it should not be compared directly to a single target duration.

## Upstream optimization outcomes and retained gates

The prerequisites are not all performance wins and should be described separately:

- [#2092](https://github.com/scytrowski/dotty-rs/issues/2092) added per-target timing for an earlier 23-target sample. Its top target timings belong to that historical cohort; the current 24-target per-target profile is the manifest data from run #41819990007.
- [#2093](https://github.com/scytrowski/dotty-rs/issues/2093), [#2094](https://github.com/scytrowski/dotty-rs/issues/2094), and [#2096](https://github.com/scytrowski/dotty-rs/issues/2096) added opt-in subprocess timing. The timer records diagnostics only and does not change ordinary test behavior.
- [#2099](https://github.com/scytrowski/dotty-rs/issues/2099) closed with a measured no-go in [PR #2131](https://github.com/scytrowski/dotty-rs/pull/2131): the separate expected-failure probes had distinct diagnostic purposes, so no safe redundant invocation was removed.
- [#2100](https://github.com/scytrowski/dotty-rs/issues/2100) adopted a bounded optimization in [PR #2139](https://github.com/scytrowski/dotty-rs/pull/2139). It batches six independent positive lookups while retaining a per-member assertion and all six independent negative first-diagnostic controls. Three paired runs measured `lookup_probes()` falling from 33.742s to 21.340s on average (36.8%); the test's mean wall time fell from 227.19s to 190.88s and its subprocess count from 36 to 21. These commits landed after the serial and sharding samples above, so those timings are pre-optimization evidence and must not be mixed with later measurements.
- [#2097](https://github.com/scytrowski/dotty-rs/issues/2097) introduced two shards and [#2098](https://github.com/scytrowski/dotty-rs/issues/2098) evaluated/adopted four. Both retained all 24 targets, strict environment checks, checksum controls, follow-up gates and aggregate failure behavior.

## Decision and operating guidance

**Evidence decision: no-go on claiming the requested PR-to-last-green speedup from this dataset.** The fixed-SHA warm workflow-attempt samples favor four shards over two on critical-path duration, with a modest measured runner-time increase and substantial duplicated cache archives. That is a useful operational signal, but only one current-suite serial run exists, the warm reruns are attempt-level rather than PR-level latency observations, and the PR-level windows include unrelated iteration and scheduling. The historical 23-target sample cannot fill the missing serial cohort. This report therefore does not assert a statistically comparable serial-to-parallel speedup or treat PR creation-to-merge time as CI latency.

The repository currently retains the four-shard gate as adopted by [PR #2121](https://github.com/scytrowski/dotty-rs/pull/2121). That operational decision is recorded with its original evidence; this report qualifies its latency interpretation rather than changing the workflow or manufacturing another layout. A future re-evaluation should collect at least three current-inventory serial runs and, for each layout, record both (a) workflow attempt wall/cost from the same fixed source and cache classification and (b) the actual time from a ready PR revision to the last of its automatic checks. Capture run creation, job start/completion, aggregate completion, and every concurrent PR workflow needed for that last-green timestamp. Do not subtract setup or queue delay from a user-visible PR latency unless reporting that component separately.

Rollback of the four-shard topology is to restore the prior serial command from the recorded 24-target manifest, keep every strict environment and four follow-up gate, and retain the aggregate required-check identity. Do not remove or disable a target to reduce wall time.

For every future strict TASTy target, the PR author should:

1. Add its exact Cargo target to the manifest and exactly one fixed shard; run the validator and inspect the resulting shard assignments.
2. Record a pinned per-target duration and compare the new critical shard with the previous profile; rebalance only with measured evidence.
3. Preserve `--locked`, `--nocapture`, `--test-threads=1`, all required-mode environment variables, pinned source/JDK/Rust/JAR checks, negative controls, all follow-up gates, and aggregate failure propagation.
4. Link successful warm and cold runs separately, record exact SHA/cache state, and recalculate both workflow wall/runner cost and PR-to-last-green rather than reusing a prior sample.

No runtime tests, compiler pins, Rust test profile, or hosted full Typer corpus audit were changed or launched for this report.

For a concrete stage-level audit, the Actions job/step timing records for one
successful current 24-target run in each layout provide this decomposition.
These are single-run details on different PR revisions, kept separate from the
three-attempt fixed-SHA averages above:

| Run | Queue to first job | Setup and pinned inputs | Scala stage | Cargo build | Strict target execution | Follow-ups | Critical job / gate | Runner total incl. aggregate |
| --- | ---: | --- | ---: | ---: | ---: | --- | ---: | ---: |
| Serial [41819990007](https://github.com/scytrowski/dotty-rs/actions/runs/41819990007) | 76s | Checkout 6s; JDK 8s; Rust 9s; cache and exact-output restores 24s; sbt runner 4s; compiler artifacts verify 1s | 67s | 76s | 1,421.9s summed across 24 targets | 310s across the four gates | 1,950s whole job | 1,950s |
| Two-way [41862325198](https://github.com/scytrowski/dotty-rs/actions/runs/41862325198) | 3s | Two jobs: checkout 13s combined; JDK 7s; Rust 16s; dependency/Rust and exact-output restores 38s; sbt runner 7s | 108s across shards | 28.8s reported Cargo profile-build time | Shard command steps: 555s + 816s | 328s, once on shard 2 | Shard 2: 1,265s; aggregate: 6s | 1,921s (31m55s shard jobs + 6s aggregate) |
| Four-way [41888319758](https://github.com/scytrowski/dotty-rs/actions/runs/41888319758) | 2s | Four jobs: checkout 30s combined; JDK 16s; Rust 34s; dependency/Rust and exact-output restores 91s; sbt runner 19s | 217s across shards | 43.0s reported Cargo profile-build time | Shard command steps: 301s + 367s + 395s + 416s | 325s, once on shard 4 | Shard 4: 862s; aggregate: 6s | 2,270s (2,264s shard jobs + 6s aggregate) |

The first serial job started 76 seconds after workflow creation. The two- and
four-way examples had 3s and 2s to first job, respectively. Queue is separated
from job execution; the full-job/critical-job columns are wall time, while
runner totals sum concurrent job durations and therefore are not user-visible
latency. The two- and four-way cost rows are not substituted for the fixed-SHA
warm means because their source revisions and individual cache contents are
different. Per-target serial time sums to 1,421.9s and Cargo build to 76s,
accounting for the 1,498s strict step. Parallel shard command steps each
include their reported Cargo build phase; do not add build time to those step
wall times a second time.

## Report validation and reproduction

The inventory and partition can be reproduced from the repository root with:

```sh
python3 tools/ci/validate_strict_tasty_targets.py
python3 tools/ci/validate_strict_tasty_targets.py --shard shard-1
python3 tools/ci/validate_strict_tasty_targets.py --shard shard-2
python3 tools/ci/validate_strict_tasty_targets.py --shard shard-3
python3 tools/ci/validate_strict_tasty_targets.py --shard shard-4
```

The negative inventory and aggregate cases exercised by the existing CI test
suite can be checked with:

```sh
python3 -m unittest discover -s tools/ci-test-timing -p 'test_*strict_tasty*.py'
python3 -m unittest discover -s tools/ci-test-timing -p 'test_aggregate_strict_tasty_shards.py'
```

For this report, the manifest validator passed for all 24 targets; the focused
validator tests passed (32 tests), and aggregate success/failure/cancel/skip/
missing-result tests passed (8 tests). The representative step values above
come from the linked Actions job and step records, not rounded prose estimates.
The warm-run means and per-shard distributions link to the fixed-SHA attempt
series in the [existing sharding report](strict-tasty-target-sharding.md):
[2-way attempt 2](https://github.com/scytrowski/dotty-rs/actions/runs/41833599053/attempts/2),
[attempt 3](https://github.com/scytrowski/dotty-rs/actions/runs/41833599053/attempts/3),
[attempt 4](https://github.com/scytrowski/dotty-rs/actions/runs/41833599053/attempts/4),
and [4-way attempt 2](https://github.com/scytrowski/dotty-rs/actions/runs/41873333835/attempts/2),
[attempt 3](https://github.com/scytrowski/dotty-rs/actions/runs/41873333835/attempts/3),
[attempt 4](https://github.com/scytrowski/dotty-rs/actions/runs/41873333835/attempts/4).
