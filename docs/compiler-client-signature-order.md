# Scheduling cross-file result inference in the external compiler client

Knowing the callee order in advance saved 0.46 ms in signature completion for
256 files with independent call chains and four workers. With the same methods
calling shared receivers it saved 0.02 ms, within the observed variation.
The public typer already identifies the missing method by symbol. The cost
depends on how often the client reaches an incomplete method.

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
