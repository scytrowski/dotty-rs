# Strict Scala/TASTy target sharding

[`tools/ci/strict_tasty_target_baseline.json`](../tools/ci/strict_tasty_target_baseline.json)
freezes the exact 23 targets from the serial gate before sharding.
[`tools/ci/strict_tasty_targets.json`](../tools/ci/strict_tasty_targets.json)
assigns every frozen target once using deterministic longest-processing-time-first
ordering, breaking duration ties by target name and assigning each next target
to the least-loaded shard (then the lower shard ordinal).

The validator checks the manifest against the independent baseline, verifies
test files and one-time coverage, and prints the ordered Cargo target list for
a selected shard:

```sh
python3 tools/ci/validate_strict_tasty_targets.py
python3 tools/ci/validate_strict_tasty_targets.py --shard shard-1
python3 tools/ci/validate_strict_tasty_targets.py --shard shard-2
```

The candidate workflow runs exactly two explicit jobs. Each independently
checks out Scala 3.9.0 at
`777528f19a58e794c9954a42f433373472ec57f8`, installs JDK
`21.0.13+9.0.LTS` and Rust `1.98.1`, restores immutable pinned caches, stages
the Scala compiler, and verifies the required SHA256 artifacts. Shard 1 owns
all cache saves; shard 2 only restores. Both jobs retain `--locked`,
`--nocapture`, and `--test-threads=1`, along with the original strict
environment variables and timing/failure reporting.

The four separate required gates run once after shard 2's targets:

- `Verify constructor alias PARAM addresses with pinned Scala 3.9`
- `Run required semantic loader secondary-constructor rejection`
- `Run strict Scala reader for dual-reference forwarding`
- `Run mandatory Java 21 verifier and publisher gate`

A final aggregate job keeps the existing `Pinned Scala 3.9 factory TASTy
consumer` check name. It requires explicit `success` results for both shard
jobs, both strict Cargo steps and each of the four follow-up steps. Failure,
cancellation, a skipped step or a missing result makes the aggregate fail.
The previous flat workflow inventory is removed only after the separate frozen
baseline and validator are in place.

## Comparable serial baseline

The baseline measurements are from successful Actions run
[41743845738](https://github.com/scytrowski/dotty-rs/actions/runs/41743845738),
sample SHA `c1e6873dc72287ebd1b0271d8f00637bc331b82a`. Cargo/libtest reported
1,389.31 seconds of test execution across all 23 binaries and a 73-second
Cargo test build. The strict step took 24m24s; the full job took 39m04s. The
estimated two-shard test totals are 696.10s and 693.21s. Assuming the 73-second
test build repeats once per shard, the estimate is 12m49s for the critical
path and 25.59 runner-minutes in total, including the repeated build. The
historical single strict step used 24.4 runner-minutes. These estimates exclude
the independently repeated source checkout, setup, Scala staging, cache
transfer and the four follow-up gates, so they do not establish a speedup.

The staged workflow change is a candidate until hosted successful runs prove
each target ran once, all four follow-up gates ran once, and comparable warm
runs show a benefit over the 23-target serial baseline. Report cache hits and
sizes, setup/staging/transfer time, job wall time, PR-to-last-green and total
runner-minutes from the candidate runs before merging. If completeness or
performance benefit cannot be demonstrated, retain the serial required gate.
