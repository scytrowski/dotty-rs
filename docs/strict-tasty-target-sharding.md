# Strict Scala/TASTy target inventory and shard proposal

[`tools/ci/strict_tasty_targets.json`](../tools/ci/strict_tasty_targets.json)
freezes the 23 `--test` targets currently listed in the
`Run strict linked Scala and TASTy oracle release gates` step of
`.github/workflows/jvm-tasty-factory-scala-consumer.yml`. Its deterministic
two-shard assignment uses longest-processing-time-first ordering, breaking ties
by target name and assigning the next target to the least-loaded shard (then
the lower shard ordinal). Validate the inventory and real test files offline
with:

```sh
python3 tools/ci/validate_strict_tasty_targets.py
```

The measurements come from successful Actions run
[41743845738](https://github.com/scytrowski/dotty-rs/actions/runs/41743845738),
sample SHA `c1e6873dc72287ebd1b0271d8f00637bc331b82a`. Cargo/libtest reported
1,389.31 seconds of test execution across all 23 binaries and a 73-second
Cargo test build. The strict step took 24m24s; the full job took 39m04s. The
two proposed shards account for 696.10s and 693.21s of test execution. Assuming
the 73-second test build repeats once per shard, the estimate is 12m49s for the
critical path and 25.59 runner-minutes in total, compared with the observed
24.4 runner-minutes for the single strict step. This suggests roughly half the
strict-step wall time for about 1.19 additional runner-minutes. Scheduling,
cache contention and setup overhead can change those results; they are
estimates, not guarantees.

The four-shard path is to apply the same deterministic assignment with four
shards generated from this inventory. On the same measured durations, the
projected critical path is 7m02s and total runner use is 28.02 minutes,
including one 73-second build per shard. A later rollout should generate each
shard's ordered `--test` arguments from this manifest so the frozen inventory
remains the single source of truth. This preparatory change does not alter
workflow execution or required checks.

## Required gates outside the 23 targets

The following steps remain separate mandatory gates and must stay required in
any later shard rollout:

- `Verify constructor alias PARAM addresses with pinned Scala 3.9`
- `Run required semantic loader secondary-constructor rejection`
- `Run strict Scala reader for dual-reference forwarding`
- `Run mandatory Java 21 verifier and publisher gate`

The workflow pins Scala sources to
`777528f19a58e794c9954a42f433373472ec57f8`, installs JDK `21.0.13+9.0.LTS`,
and runs Rust `1.98.1`. These pins and the separate gate commands are part of
the inventory's surrounding contract, not shard members.

The validator checks unique frozen membership, exact one-time coverage by the
proposed shards, and a corresponding `tests/<target>.rs` file for every member.
Negative fixtures cover an unassigned target, a duplicate shard assignment,
and a missing test file.
