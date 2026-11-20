# Scheduling successive builds by method completion order

With 256 crossed-dependency files and four workers, method history shortened
five complete builds by 2.5% against ordinary order and 4.3% against file
history, including learning the order and handling a two-file edit. Aligned
chains benefited from either history. Shared receivers showed little benefit.

An external compiler client compared three schedules on the four-method
sources from the [file-order experiment](compiler-client-multi-method-order.md).
All three used the same compiler libraries at `c4dd3db46b` and the same
missing-dependency and cycle handling. Each compilation started a fresh
semantic session. Only the order of requests for method signatures varied.

- **Ordinary** visits files in input order and methods in declaration order.
- **File** remembers each file on its first successful method completion and
  visits those files first on the next build.
- **Method** records the successful completion sequence, including methods
  reached recursively while another method was waiting. Each entry points
  to a path and an ordinal in that path's previous source declarations.
  The client compares the current file's complete text with its saved copy
  once per path. For a match it translates ordinals to current-session
  symbols through the source semantic index. A changed or missing path
  contributes no preferred roots. The client also visits every current root,
  so new and changed methods can complete normally. It replaces the history
  only after successful signature completion.

The method hint stores an owned copy of all input texts and paths plus two
machine-word indices per recorded method. Source comparison, copying,
symbol lookup, recording and replacement are all in the signature clock.
This in-memory prototype assumes unique logical paths and a fixed frontend
configuration. Each session infers current types and emits current classes.
[Client implementation and regressions](../tests/fixtures/compiler-client/tests/support/method_history.rs)

## Builds and edits

Each matched sequence comprised five consecutive builds per strategy:

1. **Cold** started with empty history.
2. **Replay** used the order learned on the first build.
3. **Stable** rebuilt the same sources again, including the file hint that
   the first replay had refreshed.
4. **Edited** reversed the declaration order in the first file and changed
   its `run0` to call a method in the last file. That last file gained an
   overload `run0(x: Int)` alongside `run0()`. Two paths changed, with one
   additional method across the whole batch.
5. **Repaired** used the history learned on the edited sources to build them
   again.

A timing process exercised 16, 64 and 256 files with one and four workers.
Chains used independent groups, Shared routed each chain through the middle
group, and Crossed gave two methods per file the opposite dependency
direction to the other two. Each file initially held four methods. Workers
handled parsing and JVM emission. Completion stayed sequential.

The experiment ran three release processes with Rust 1.98.1, pinned to CPUs
0–7 on the same shared Ryzen 9 7940HS. After one warm-up round, each process
ran twelve measured rounds. Within a fixed configuration and stage, all six
strategy permutations occurred equally often. Every strategy carried its own
history across the five builds of a round. Every timed batch checked class
names, order and bytes against an ordinary one-worker compilation of the
same source version after stopping the clock. There were 36 matched
observations per configuration, strategy and stage.
[All 9,720 latency samples](compiler-client-timings/method-history-runs.csv)

## Cost of all five builds

For each strategy and round, we summed the five full compilation times
from session setup to the final class bytes in memory. The time columns
show separate medians of these sums in milliseconds. The last columns give
median within-round percentage savings for Method relative to each
alternative, with linearly interpolated first and third quartiles. A positive
saving means Method finished sooner.

| Files | Layout | Workers | Ordinary (ms) | File (ms) | Method (ms) | Method vs ordinary [Q1, Q3] (%) | Method vs file [Q1, Q3] (%) |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 16 | Chains | 1 | 17.51 | 17.09 | 17.08 | +3.1% [+1.6, +4.8] | -0.3% [-3.7, +3.5] |
| 16 | Chains | 4 | 10.02 | 9.44 | 9.40 | +5.5% [+1.9, +8.1] | -0.7% [-4.2, +2.9] |
| 16 | Shared | 1 | 16.95 | 17.00 | 16.83 | +1.0% [-3.9, +4.0] | +0.9% [-2.6, +4.7] |
| 16 | Shared | 4 | 9.47 | 9.41 | 9.42 | +0.1% [-3.9, +4.0] | +0.7% [-2.4, +5.2] |
| 16 | Crossed | 1 | 17.47 | 17.76 | 17.13 | +0.3% [-3.0, +5.0] | +4.0% [+0.7, +6.5] |
| 16 | Crossed | 4 | 9.85 | 9.89 | 9.57 | +1.2% [-0.6, +6.4] | +3.0% [+0.1, +6.4] |
| 64 | Chains | 1 | 49.68 | 47.48 | 47.50 | +4.9% [+2.5, +6.7] | +0.2% [-2.0, +2.1] |
| 64 | Chains | 4 | 35.02 | 33.11 | 32.91 | +6.6% [+4.7, +7.9] | +0.8% [-0.2, +2.8] |
| 64 | Shared | 1 | 47.51 | 46.92 | 47.35 | +1.0% [-2.3, +3.0] | -0.6% [-2.3, +0.8] |
| 64 | Shared | 4 | 32.31 | 32.34 | 32.43 | -0.4% [-1.9, +1.0] | -0.1% [-1.1, +0.8] |
| 64 | Crossed | 1 | 47.60 | 49.44 | 47.36 | +0.4% [-1.7, +2.1] | +4.0% [+1.9, +7.0] |
| 64 | Crossed | 4 | 34.41 | 34.93 | 33.46 | +2.6% [+1.9, +3.7] | +4.3% [+3.8, +5.2] |
| 256 | Chains | 1 | 212.22 | 206.51 | 206.77 | +2.6% [+1.6, +3.5] | +0.1% [-1.2, +0.9] |
| 256 | Chains | 4 | 133.30 | 126.76 | 127.12 | +5.1% [+3.6, +6.0] | -0.2% [-1.8, +0.9] |
| 256 | Shared | 1 | 203.64 | 204.65 | 204.13 | -0.6% [-1.1, +0.2] | +0.2% [-1.0, +1.3] |
| 256 | Shared | 4 | 124.10 | 124.36 | 124.98 | -0.8% [-1.9, +1.0] | -0.3% [-1.1, +0.8] |
| 256 | Crossed | 1 | 212.75 | 214.25 | 209.07 | +1.4% [+0.9, +2.5] | +2.5% [+1.6, +3.9] |
| 256 | Crossed | 4 | 129.45 | 132.68 | 127.06 | +2.5% [+1.0, +3.4] | +4.3% [+3.0, +5.1] |

At 256 files and four workers, on Crossed the method hint saved 2.5% over
ordinary order and 4.3% over file history across all five builds. The middle
halves of both comparisons lay above zero. Signature completion alone saved
8.9% and 14.9%, respectively. On Chains either history was useful. The
measured difference between the two histories was small relative to their
variation. On Shared, ordinary order already resolved most dependencies
immediately and keeping history brought no consistent full-pipeline saving.

## When the cost is paid

The table gives independent stage medians for 256 files and four workers.
The ordinary, file and method columns appear in the same order for both time
intervals. On Crossed, method recording added 0.079 ms of signature time on
the cold build relative to ordinary order (paired median). The first replay
saved 0.819 ms. On the edit, method history still saved 0.798 ms relative to
ordinary order and 1.583 ms relative to file history.

| Layout | Build | Ordinary signatures (ms) | File signatures (ms) | Method signatures (ms) | Ordinary full (ms) | File full (ms) | Method full (ms) |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Chains | Cold | 8.051 | 8.186 | 8.182 | 26.54 | 26.86 | 27.04 |
| Chains | Replay | 8.082 | 6.353 | 6.381 | 26.63 | 24.87 | 25.00 |
| Chains | Stable | 8.096 | 6.347 | 6.365 | 26.65 | 24.87 | 24.91 |
| Chains | Edited | 8.096 | 6.359 | 6.366 | 26.56 | 24.82 | 25.03 |
| Chains | Repaired | 8.080 | 6.357 | 6.347 | 26.60 | 24.94 | 25.01 |
| Shared | Cold | 6.246 | 6.342 | 6.330 | 24.82 | 24.96 | 25.02 |
| Shared | Replay | 6.264 | 6.333 | 6.323 | 24.89 | 24.91 | 24.82 |
| Shared | Stable | 6.235 | 6.333 | 6.351 | 24.49 | 24.82 | 24.94 |
| Shared | Edited | 6.254 | 6.355 | 6.364 | 24.61 | 24.89 | 24.80 |
| Shared | Repaired | 6.257 | 6.366 | 6.355 | 24.56 | 24.79 | 25.04 |
| Crossed | Cold | 7.156 | 7.254 | 7.214 | 26.02 | 26.00 | 26.09 |
| Crossed | Replay | 7.178 | 7.253 | 6.331 | 25.81 | 26.09 | 25.17 |
| Crossed | Stable | 7.191 | 7.917 | 6.354 | 25.89 | 26.76 | 25.45 |
| Crossed | Edited | 7.152 | 7.935 | 6.344 | 26.02 | 26.64 | 25.23 |
| Crossed | Repaired | 7.162 | 7.933 | 6.360 | 25.75 | 26.63 | 25.14 |

## Dependencies and correctness

A separate counter run observed missing method results that triggered retries.
Within each cell the counts follow Cold / Replay / Stable / Edited / Repaired.

| Files | Layout | Ordinary | File | Method |
| ---: | --- | --- | --- | --- |
| 16 | Chains | 48 / 48 / 48 / 48 / 48 | 48 / 0 / 0 / 2 / 2 | 48 / 0 / 0 / 5 / 0 |
| 16 | Shared | 12 / 12 / 12 / 14 / 14 | 12 / 0 / 0 / 2 / 2 | 12 / 0 / 0 / 2 / 0 |
| 16 | Crossed | 24 / 24 / 24 / 24 / 24 | 24 / 24 / 40 / 41 / 41 | 24 / 0 / 0 / 5 / 0 |
| 64 | Chains | 192 / 192 / 192 / 192 / 192 | 192 / 0 / 0 / 2 / 2 | 192 / 0 / 0 / 5 / 0 |
| 64 | Shared | 12 / 12 / 12 / 14 / 14 | 12 / 0 / 0 / 2 / 2 | 12 / 0 / 0 / 2 / 0 |
| 64 | Crossed | 96 / 96 / 96 / 96 / 96 | 96 / 96 / 160 / 161 / 161 | 96 / 0 / 0 / 5 / 0 |
| 256 | Chains | 768 / 768 / 768 / 768 / 768 | 768 / 0 / 0 / 2 / 2 | 768 / 0 / 0 / 5 / 0 |
| 256 | Shared | 12 / 12 / 12 / 14 / 14 | 12 / 0 / 0 / 2 / 2 | 12 / 0 / 0 / 2 / 0 |
| 256 | Crossed | 384 / 384 / 384 / 384 / 384 | 384 / 384 / 640 / 641 / 641 | 384 / 0 / 0 / 5 / 0 |

For Crossed, a method in one file could complete between two methods of
another file. Recording that interleaving eliminated retries on unchanged
sources. After the edit, unchanged paths retained their hints and the current
typer discovered the newly required overload. The next pass then had zero
retries. With Shared, eliminating a few retries did little to the full-build
time.

All methods inferred the canonical session `Int` during the count run.
An edit regression also reversed file registration and reassigned source IDs.
It checked both emitted overload descriptors and the edited caller's exact
`invokevirtual` owner, name and descriptor. All schedules produced identical
class bytes. A second regression introduced a mutual cycle. All schedules
reported `RecursiveInferredMethodResult` for the reentered current symbol
and left both method signatures missing. Both histories preserved their
previous successful snapshot. File history started the cycle at `B.right`.
Ordinary and method order both started at `A.left` after B's text changed.

These results support keeping method history as an optional policy in the
external compiler for repeat builds with conflicting dependencies within
files. For aligned chains, file history already captured most of the
available gain. The measurements cover small synthetic sources, an in-memory
snapshot, two edited paths and fresh full builds. On larger sources the cost
of retaining, comparing and copying file contents can change the trade-off.

## Reproduction

Run the timing command three times. Substitute `probe_method_history` once
to collect the retry counts. For each strategy, sum `total_ns` across the five
stages of a given process, round, graph, file count and worker count before
computing paired savings and medians.

```sh
taskset -c 0-7 cargo test --release --locked --manifest-path tests/fixtures/compiler-client/Cargo.toml \
  --target-dir target/compiler-client --test multi_file measure_method_history -- --ignored --nocapture
```
