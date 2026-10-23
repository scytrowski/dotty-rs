# Typer audit rollback bookkeeping (Scala 3.9.0)

This note records the before/after evidence for issue #1485. The change affects
only audit observation bookkeeping in `local_definition_audit.rs`; semantic
resolver checkpoint and rollback behavior is unchanged.

## Focused corpus profile

The comparison used the pinned Scala source revision
`777528f19a58e794c9954a42f433373472ec57f8`, Scala 3.9.0 artifacts and Temurin
21.0.8. The selection was `tpd.scala` and `GenericSignatures.scala`, with 39
inventory methods. The baseline is commit `87c40c65cc` (instrumentation with
the old suffix scan); the after run is commit `3b1b0863de`.

| Metric | Before | After |
| --- | ---: | ---: |
| Profile wall time | 17,047 ms | 16,790 ms |
| Checkpoints / maximum active depth | 2,772 / 1,005 | 2,772 / 1,005 |
| Rollbacks | 1,258 | 1,258 |
| Operation rows appended / rollback coverage / rows scanned / status changes | 16,777 / 16,777 / 16,777 / 16,777 | 16,777 / 16,777 / 16,777 / 16,777 |
| Member rows appended / rollback coverage / rows scanned / status changes | 8,364 / 8,364 / 8,364 / 8,364 | 8,364 / 8,364 / 8,364 / 8,364 |
| Checkpoint bookkeeping stage | 344 µs | 342 µs |
| Rollback annotation stage | 550 µs | 571 µs |
| Exclusive `rollback_to` wrapper time | 752 µs | 770 µs |

The three serialized sections `module_package_term_observations`,
`module_operation_observations` and `module_member_observations` were byte
identical between the runs. The normalized focused reports were also byte
identical, including all 39 method identities and first-blocker rows. The full
profile wall-time difference is a single sample and is not evidence of a
speedup; the observed rollback rows were each visited once before the change,
so this corpus selection has no repeated suffix work to remove. The small
rollback-stage difference is within run noise. Peak RSS was unavailable from
the local timing wrapper.

## Synthetic scaling

The ignored test `synthetic_rollback_observation_scaling_benchmark` runs 1,000
samples per cell. Both strategies produce identical final status bitmaps. The
old strategy mutates every row in every rollback suffix; the new strategy
updates only rows not previously transitioned. Timing is the mean elapsed
nanoseconds per sample on the local test host. Row counts are deterministic;
the short timings are diagnostic and hardware-dependent.

| Events | Rollback pattern | Rollbacks/sample | Old rows scanned/sample | New rows scanned/sample | Old ns/sample | New ns/sample |
| ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 100 | Single rollback | 1 | 100 | 100 | 27 | 52 |
| 100 | Repeated rollback to parent | 100 | 5,050 | 100 | 180 | 497 |
| 100 | Alternating child rollback | 50 | 50 | 50 | 98 | 269 |
| 1,000 | Single rollback | 1 | 1,000 | 1,000 | 33 | 50 |
| 1,000 | Repeated rollback to parent | 100 | 50,500 | 1,000 | 481 | 384 |
| 1,000 | Alternating child rollback | 500 | 500 | 500 | 907 | 1,951 |
| 10,000 | Single rollback | 1 | 10,000 | 10,000 | 165 | 178 |
| 10,000 | Repeated rollback to parent | 100 | 505,000 | 10,000 | 3,374 | 440 |
| 10,000 | Alternating child rollback | 5,000 | 5,000 | 5,000 | 9,410 | 19,164 |

With repeated parent rollback, the old path scans a triangular number of rows
as the suffix grows. At 10,000 events this is 505,000 row visits; the interval
index visits the 10,000 newly affected rows once (50.5x fewer). In the
alternating case there is no overlap to avoid, and interval bookkeeping is
slower in this microbenchmark, although the measured cost remains below 20 µs
per 10,000-event sample. The fast append path has no temporary range-vector
allocation; ranges are cleared when a method attempt reaches its commit
boundary.

## Reproduction

Focused profile (use the pinned Scala checkout and JDK 21.0.8):

```sh
cat > /tmp/typer-audit-rollback-files.tsv <<'EOF'
compiler/src/dotty/tools/dotc/ast/tpd.scala
compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala
EOF
SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8 \
JAVA_HOME=/path/to/temurin-21.0.8 SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=/path/to/coursier/https/repo1.maven.org/maven2 \
  tools/typer-classpath-corpus-audit/run \
    --focused-manifest /tmp/typer-audit-rollback-files.tsv \
    --compare-legacy-profile /tmp/typer-audit-rollback.md
```

Run this once from a worktree at `87c40c65cc` and once from the implementation
branch to collect the paired measurements.

Synthetic scaling:

```sh
cargo test -p dotty-typer --test local_definition_audit --locked \
  synthetic_rollback_observation_scaling_benchmark -- --ignored --nocapture
```

The full corpus was not run; the focused issue selection does not require it.
