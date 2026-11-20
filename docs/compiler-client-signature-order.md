# Scheduling cross-file result inference in the external compiler client

Knowing the callee order in advance saved 0.46 ms in signature completion for
256 files with independent call chains and four workers. With the same methods
calling shared receivers it saved 0.02 ms, within the observed variation.
The public typer already identifies the missing method by symbol. The cost
depends on how often the client reaches an incomplete method. The
[follow-up experiment](#learning-the-order-from-a-previous-compilation) measures
the cost of discovering and reusing an order in the external client.

## What changed between paired compilations

Both compilations used the backend at `cd2781716b` and the same external client.
Each started with a fresh semantic session. Parsing, name entry, class header
completion, body typing and class emission all used their original order.
Only the order of initial method signature requests varied.

- **Callers first.** The client visits Last, Second, First and Leaf in input
  order for each group. When a known source method is missing, it completes
  that method and retries the caller.
- **Callees first.** The client visits every Leaf, then every First, Second
  and Last. This order is known from the source generator. Both variants
  prepare their lists of root indices outside the clock and use the same
  completion loop and cycle detection.

Each method has one inferred result. Every non-leaf calls one method from
another file. In the shared layout all groups call the same three targets.
The worker setting controls parsing and JVM emission. Method completion runs
on a single thread in both variants. [External client harness](../tests/fixtures/compiler-client/tests/support/signature_order.rs)

## Operations and available work

The counter run recorded external calls to `complete_symbol`. Each source
contributed one class header and one successful method completion. Each
`MemberTypeUnavailable` report named a distinct target within its schedule
on these inputs, and led to exactly one repeated method request.

| Files | Layout | Total calls, callers → callees | Missing dependencies, callers → callees | Retries, callers → callees | Ready methods in rounds 1–4 |
| ---: | --- | ---: | ---: | ---: | --- |
| 16 | Chains | 44 → 32 | 12 → 0 | 12 → 0 | 4, 4, 4, 4 |
| 16 | Shared | 35 → 32 | 3 → 0 | 3 → 0 | 4, 4, 4, 4 |
| 64 | Chains | 176 → 128 | 48 → 0 | 48 → 0 | 16, 16, 16, 16 |
| 64 | Shared | 131 → 128 | 3 → 0 | 3 → 0 | 16, 16, 16, 16 |
| 256 | Chains | 704 → 512 | 192 → 0 | 192 → 0 | 64, 64, 64, 64 |
| 256 | Shared | 515 → 512 | 3 → 0 | 3 → 0 | 64, 64, 64, 64 |

A separate graph probe requested all consumers at each level before any of their
callees. This exposed the target symbol of every cross-file edge, including
edges that usually resolve immediately against a completed shared method.
Removing all ready methods together yielded the widths above. These widths
measure independent result dependencies under an ideal schedule. Completing
signatures concurrently would also require handling writes to the session's
semantic store. The experiment used the existing exclusive mutable borrow.

## Latency

Three consecutive release processes ran on a shared Ryzen 9 7940HS using
Rust 1.98.1 and CPUs 0–7 (one logical CPU per physical core). Each process
warmed all configurations, then ran twelve rounds, rotating case order and
alternating which schedule ran first within each pair. Each cell has 36 paired
observations. A timing run collected no operation counts. The signature
interval includes class headers, owner lookup table construction and all method
requests. The full interval runs from session setup to the ordered class bytes
in memory. [All 864 observations](compiler-client-timings/signature-order-runs.csv)

Times in the two arrow columns are separate medians, in milliseconds. The
signature saving gives the median of the within-pair differences in
microseconds and its interquartile range. The last column gives the median of
the within-pair percentage reductions in full compilation time. These paired
statistics can differ from a comparison of the two separate medians.

| Files | Layout | Workers | Signatures, callers → callees (ms) | Paired saving (µs) [Q1, Q3] | Full pipeline, callers → callees (ms) | Paired full saving |
| ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 16 | Chains | 1 | 0.159 → 0.123 | +33 [+30, +262] | 1.44 → 1.12 | +22.0% |
| 16 | Chains | 4 | 0.158 → 0.124 | +34 [+33, +57] | 1.09 → 0.97 | +10.0% |
| 16 | Shared | 1 | 0.125 → 0.120 | +11 [+7, +12] | 1.00 → 0.86 | +1.8% |
| 16 | Shared | 4 | 0.131 → 0.122 | +8 [+1, +10] | 0.94 → 0.94 | -0.4% |
| 64 | Chains | 1 | 0.735 → 0.730 | +115 [-208, +383] | 4.46 → 4.38 | +0.1% |
| 64 | Chains | 4 | 0.546 → 0.424 | +120 [+109, +123] | 2.56 → 2.66 | +0.5% |
| 64 | Shared | 1 | 0.748 → 0.728 | -8 [-386, +324] | 4.39 → 4.38 | -0.2% |
| 64 | Shared | 4 | 0.443 → 0.424 | +17 [+9, +26] | 2.54 → 2.45 | +7.7% |
| 256 | Chains | 1 | 2.080 → 1.620 | +456 [+415, +475] | 14.22 → 13.98 | +2.0% |
| 256 | Chains | 4 | 2.104 → 1.641 | +463 [+448, +482] | 8.95 → 8.14 | +8.4% |
| 256 | Shared | 1 | 1.642 → 1.611 | +30 [+10, +44] | 14.56 → 14.53 | +0.5% |
| 256 | Shared | 4 | 1.669 → 1.641 | +20 [-12, +36] | 8.20 → 8.14 | +1.7% |

The independent chains at 256 files saved about 22% of signature time with
either worker count. At four workers the middle half of the full-pipeline
savings was 6.9–9.9%. For shared receivers it ranged from −1.7% to +5.5%.
Inputs of 64 files showed substantial full-pipeline variation even when the
signature saving was visible. Root order also changes traversal locality, so
the time difference includes more than the failed calls themselves.

## Correctness and API decision

All methods in both layouts and at all three sizes inferred the session's
canonical `Int` result in both orders. Every timed compilation and warm-up
compared its ordered class names and exact bytes with the ordinary one-worker
client output. The two-method cycle produced `RecursiveInferredMethodResult`
in either root order and left both method signatures missing. The diagnostic
names the reentered method, so starting at `A.left` identifies `A.left` and
starting at `B.right` identifies `B.right`, even with the same source entry
order. A compiler that wants a stable cycle location must choose how to present
it. [Cycle regression](../tests/fixtures/compiler-client/tests/multi_file.rs)

On these chains the retries have a measurable cost. A distinct public
completion outcome for a blocked source method could simplify the client's
error handling. Returning that outcome at the same point in inference would
still repeat the caller's work. Before extending the API, measure a client
that discovers or reuses dependencies without knowledge of the generator and
include discovery in the signature clock. The observed 0.46 ms saving for
256 independent files provides a budget for that work on this workload. The
three shared dependencies offer much less room. Keep scheduling in the
external compiler client while evaluating this trade-off.

## Reproduction

Run these commands from the repository root. Run the timing command three
times for the reported sample count. The ordinary client test suite includes
the small counter, inferred-type and cycle regressions.

```sh
taskset -c 0-7 cargo test --release --locked --manifest-path tests/fixtures/compiler-client/Cargo.toml \
  --target-dir target/compiler-client --test multi_file measure_signature_order -- --ignored --nocapture
taskset -c 0-7 cargo test --release --locked --manifest-path tests/fixtures/compiler-client/Cargo.toml \
  --target-dir target/compiler-client --test multi_file signature_order_probe -- --ignored --nocapture
```

## Learning the order from a previous compilation

A client using the public API saved a median of **400 µs** of signature time
on a repeat compilation of 256 independent files with four workers. Learning
the order on the first compilation cost **32 µs** in paired observations. On
the shared-receiver layout, replay cost **48 µs** more than the ordinary
client, even though it avoided all three retries.

The client records the logical file path of each successfully completed
method. On the next build it resolves those paths against freshly named units
and requests signatures in the recorded order. Any missing dependency goes
through the ordinary completion loop. When all signatures succeed, their
actual completion order replaces the previous hint. Removed paths drop out
and new paths follow the current input order. The fixture has one method per
unique file path. A compiler using multiple methods per file would need to
measure what a file-level hint buys on those sources. The [four-method experiment](compiler-client-multi-method-order.md) tests aligned, shared and opposing dependency orders. [Client implementation and tests](../tests/fixtures/compiler-client/tests/support/learned_order.rs)

### Matched builds

The baseline used the ordinary schedule and kept no history. **Learn** started
with empty history. **Replay** started with a hint obtained from a successful
compilation of the same sources. **Edited** started with a hint learned on the
opposite call graph using the same paths. Each sample built a fresh semantic
session from source through ordered class bytes. The measured signature phase
includes constructing lookup tables, resolving old paths, completing all
signatures, allocating the new path strings and replacing the old hint.
Preparing an independent copy of the starting hint for each benchmark sample
was outside the clock. The experiment used in-memory history.

Rust 1.98.1 release builds ran on the shared Ryzen 9 7940HS, pinned to logical
CPUs 0–7. Three processes each warmed all configurations and then ran twelve
rounds. Each group of four rounds balanced the position of the four schedules
and which schedule preceded another within a group of samples. Parsing and
emission used one or four workers. Signature completion stayed sequential.
Counts came from a separate run. Every timed sample checked ordered class
names and bytes against a normal one-worker build. [All 1,728 latency observations](compiler-client-timings/learned-order-runs.csv)

The four signature columns are separate medians in milliseconds. The paired
saving is baseline minus replay within the same process, round and case, with
linearly interpolated quartiles. The last column is the median paired
percentage reduction in complete in-memory compilation time.

| Files | Layout | Workers | Baseline (ms) | Learn (ms) | Replay (ms) | Edited (ms) | Paired signature saving (µs) [Q1, Q3] | Paired full saving |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 16 | Chains | 1 | 0.172 | 0.166 | 0.136 | 0.156 | +31 [+25, +54] | +4.6% |
| 16 | Chains | 4 | 0.161 | 0.161 | 0.142 | 0.157 | +27 [-2, +34] | +2.6% |
| 16 | Shared | 1 | 0.136 | 0.140 | 0.131 | 0.140 | +6 [0, +13] | -4.5% |
| 16 | Shared | 4 | 0.140 | 0.151 | 0.132 | 0.142 | +5 [-3, +15] | -2.6% |
| 64 | Chains | 1 | 0.619 | 0.774 | 0.543 | 0.652 | +101 [-155, +382] | +6.5% |
| 64 | Chains | 4 | 0.599 | 0.613 | 0.475 | 0.609 | +109 [+101, +141] | +4.0% |
| 64 | Shared | 1 | 0.507 | 0.523 | 0.628 | 0.676 | -24 [-318, +192] | -0.3% |
| 64 | Shared | 4 | 0.473 | 0.477 | 0.477 | 0.488 | -4 [-43, +4] | -1.9% |
| 256 | Chains | 1 | 2.320 | 2.444 | 1.891 | 2.346 | +417 [+107, +523] | +2.2% |
| 256 | Chains | 4 | 2.265 | 2.324 | 1.851 | 2.377 | +400 [+160, +452] | +5.5% |
| 256 | Shared | 1 | 1.879 | 1.873 | 1.998 | 1.944 | -77 [-319, +61] | -0.9% |
| 256 | Shared | 4 | 1.804 | 1.913 | 1.912 | 1.862 | -48 [-312, -14] | -3.5% |

At 256 files and four workers the paired signature saving for chains was
18.8%. The corresponding full-pipeline saving was 5.5%, with the middle half
of samples ranging from −0.5% to +11.8%. For shared receivers the full-pipeline
saving ranged from −11.9% to +8.3% across the middle half. The phase-level
effect is clearer than the effect on total time.

For the same 256-file configuration, the estimated net signature saving over
two builds was 348 µs for chains and −121 µs for shared receivers. This
compares twice the baseline with Learn plus Replay within each matched group.
Their interquartile ranges were respectively −211 to +429 µs and −518 to
−22 µs. A single replay typically recovered the recording cost on chains,
with considerable variation on this host.

### Recovering after edits

The probe counted reports of a missing method that trigger a retry. All
successful schedules inferred the canonical session `Int` for every method.
The fourth column gives retries on unchanged inputs. The last two columns show
how the schedule adapts to the opposite graph in consecutive compilations.

| Files | Current layout | Empty history | Replay | After graph change | Next replay |
| ---: | --- | ---: | ---: | ---: | ---: |
| 16 | Chains | 12 | 0 | 9 | 0 |
| 16 | Shared | 3 | 0 | 3 | 0 |
| 64 | Chains | 48 | 0 | 45 | 0 |
| 64 | Shared | 3 | 0 | 3 | 0 |
| 256 | Chains | 192 | 0 | 189 | 0 |
| 256 | Shared | 3 | 0 | 3 | 0 |

A separate edit regression reversed source registration, reassigned source
IDs, removed four files, added a file and changed a caller to use the new
method. The first rebuild retried once, produced the same bytes as the normal
schedule and repaired the hint. Another test introduced a two-method cycle.
The client reported the reentered method, left both signatures missing and
retained the last successful hint.

These results support using a scheduling hint in an external compiler that
rebuilds similar dependency-heavy sources. A batch with few blocked requests
has little work to recover. The number of observed retries is a candidate
signal for deciding whether to retain a hint. A cutoff and its cost would
need measurement on representative multi-method sources. The current typer
API was sufficient throughout this experiment.

To reproduce the measurements at this revision, run the release command
above with `measure_learned_order` three times, then with `probe_learned_order`
once.
