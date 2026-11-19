# Reusing a file schedule with four inferred methods per file

For 256 files and four workers, replaying file order saved 1.70 ms of
signature completion on independent chains (21.1%) and 5.9% of complete
compilation time. With shared targets it removed twelve retries and produced
no consistent speedup. When two methods per file required the reverse of the
order needed by the other two, the hint eventually increased retries from
384 to 640 and made signature completion 9.9% slower in a separate paired
comparison. File order alone is useful when the methods of a file benefit
from visiting the same producers early.

## Workloads and scheduling rule

Each group has four source files, each declaring one class with four distinct
methods in declaration order (`run0` through `run3`). Each method belongs to a
separate chain ending in an integer expression. A group contains four leaf
methods and twelve virtual calls in every layout.

- **Chains.** All four chains use the group's four classes in the same direction.
- **Shared.** Callers in every group use the three targets for their method
  from the middle group. Many calls benefit after just one target completes.
- **Crossed.** Two independent chains run forward through the four classes and
  two run backward. The method graph is acyclic. Every ordering of whole
  files puts some callers before their callees.

The external client requests methods in declaration order within each file.
It records a file path once, on the first successful method completion from
that file, including completions reached while following a missing dependency.
The next session resolves paths against its own source units. On success it
records a fresh file order by the same rule. The client keeps no method order
or semantic IDs across sessions. The producer, scheduler and history all live
in the standalone test client. [Fixture and checks](../tests/fixtures/compiler-client/tests/support/multi_method_order.rs)

## Measurements

The experiment repeated the [single-method protocol](compiler-client-signature-order.md#learning-the-order-from-a-previous-compilation)
using the library at `dae1dfa2b7`, Rust 1.98.1 in release mode and the shared
Ryzen 9 7940HS pinned to logical CPUs 0–7. Three processes each warmed all
configurations, then ran twelve rounds with balanced treatment order. The
baseline used ordinary input order. Learn recorded the first build. Replay
used that first build's history for the same sources. Edited used history from
Shared for Chains, and from Chains for Shared and Crossed. The signature clock
includes recording, path lookup, retries and replacement of the old hint.
Preparing independent copies of the starting history was outside the clock.
Workers parsed and emitted classes in parallel. Signature completion ran
sequentially. All samples checked the ordered class names and exact bytes
against ordinary one-worker compilation.

The four timing columns give separate medians in milliseconds. The last two
columns compare baseline and first replay within the same process, round and
configuration. Positive savings mean replay was faster. Brackets contain the
middle 50% of the 36 paired differences. Total time covers fresh session setup
through all class bytes in memory. [Raw observations for both studies](compiler-client-timings/multi-method-runs.csv)

| Files | Layout | Workers | Baseline (ms) | Learn (ms) | Replay (ms) | Edited (ms) | Paired signature saving (µs) [Q1, Q3] | Paired full saving |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 16 | Chains | 1 | 0.863 | 0.813 | 0.751 | 0.805 | +152 [-105, +392] | +4.3% |
| 16 | Chains | 4 | 0.535 | 0.547 | 0.426 | 0.513 | +110 [+98, +123] | +7.7% |
| 16 | Shared | 1 | 0.787 | 0.796 | 0.652 | 0.822 | +25 [-146, +328] | +0.3% |
| 16 | Shared | 4 | 0.446 | 0.454 | 0.425 | 0.457 | +19 [+10, +26] | +4.4% |
| 16 | Crossed | 1 | 0.875 | 0.865 | 0.851 | 0.796 | +52 [-304, +278] | +0.1% |
| 16 | Crossed | 4 | 0.481 | 0.490 | 0.490 | 0.489 | -10 [-16, +3] | +0.7% |
| 64 | Chains | 1 | 2.154 | 2.184 | 1.740 | 2.349 | +440 [+221, +636] | +2.3% |
| 64 | Chains | 4 | 2.070 | 2.107 | 1.632 | 2.081 | +441 [+429, +451] | +6.1% |
| 64 | Shared | 1 | 1.743 | 1.809 | 1.640 | 1.691 | +26 [-48, +337] | -0.9% |
| 64 | Shared | 4 | 1.634 | 1.659 | 1.625 | 1.662 | +8 [-3, +19] | 0.0% |
| 64 | Crossed | 1 | 1.881 | 1.967 | 1.876 | 1.881 | -19 [-170, +581] | -0.2% |
| 64 | Crossed | 4 | 1.836 | 1.866 | 1.866 | 1.859 | -32 [-44, -20] | -0.7% |
| 256 | Chains | 1 | 8.080 | 8.216 | 6.349 | 8.183 | +1705 [+1662, +1763] | +4.1% |
| 256 | Chains | 4 | 8.127 | 8.262 | 6.380 | 8.222 | +1699 [+1678, +1773] | +5.9% |
| 256 | Shared | 1 | 6.285 | 6.383 | 6.341 | 6.421 | -64 [-122, -24] | -0.4% |
| 256 | Shared | 4 | 6.314 | 6.392 | 6.383 | 6.403 | -103 [-160, +9] | -1.3% |
| 256 | Crossed | 1 | 7.196 | 7.300 | 7.341 | 7.340 | -166 [-243, -68] | -1.5% |
| 256 | Crossed | 4 | 7.228 | 7.324 | 7.302 | 7.313 | -110 [-156, +220] | +0.0% |

At 256 files and four workers, recording the first build on Chains cost a
paired median of 0.152 ms. Recording and replaying together saved an estimated
1.545 ms of signature work over two builds (middle 50% 1.242–1.642 ms),
comparing Learn plus Replay with twice the baseline in each matched round.
For Shared the corresponding saving was −0.226 ms (−0.415 to +0.018 ms).
For Crossed it was −0.227 ms (−0.360 to +0.264 ms). At 64 files and four
workers the first replay already saved 0.441 ms on Chains, while the Shared
saving of 0.008 ms had an interquartile range spanning zero.

## What happens on subsequent builds

A separate probe counted missing method results that trigger retries and
verified that all methods inferred the session's canonical `Int`. A focused
test also decoded the class files to check the constructor and all four
methods in declaration order.

| Files | Layout | First build | First replay | After graph edit | Replay after edit |
| ---: | --- | ---: | ---: | ---: | ---: |
| 16 | Chains | 48 | 0 | 36 | 0 |
| 16 | Shared | 12 | 0 | 12 | 0 |
| 16 | Crossed | 24 | 24 | 24 | 40 |
| 64 | Chains | 192 | 0 | 180 | 0 |
| 64 | Shared | 12 | 0 | 12 | 0 |
| 64 | Crossed | 96 | 96 | 96 | 160 |
| 256 | Chains | 768 | 0 | 756 | 0 |
| 256 | Shared | 12 | 0 | 12 | 0 |
| 256 | Crossed | 384 | 384 | 384 | 640 |

On unchanged Crossed sources, the first build, first replay and second replay
at 256 files also needed 384, 384 and 640 retries respectively. Within a
group numbered 0–3 in input order, the first build learns [3, 2, 1, 0].
Replaying it learns [3, 0, 1, 2], which reproduces itself on subsequent
builds. A separate 256-file, four-worker run compared this stable hint with
ordinary input order, alternating which ran first over 36 pairs across three
processes. The stable hint **added 1.085 ms** to signature completion (middle
50% 0.616–1.623 ms) and **5.6%** to full compilation time (2.3–10.1%).
Each comparison used the baseline from the same study on the shared host.

These generated sources support keeping file history optional in a compiler
client. Observing both recording cost and retries *after* replay matters. A
large initial retry count alone would have selected the unprofitable Crossed
case. The results cover four methods per class and this first-completion
heuristic. The [method-history experiment](compiler-client-method-history.md) compares both granularities across repeated builds and source edits.

## Reproduction

From the repository root, run the following command with
`measure_multi_method_order` three times for the main comparison. Substitute
`probe_multi_method_order` once for the retry counts and
`measure_stable_crossed_order` three times for the stable comparison.
The CSV distinguishes `main` (2,592 samples) and `stable` (72 samples).

```sh
taskset -c 0-7 cargo test --release --locked --manifest-path tests/fixtures/compiler-client/Cargo.toml \
  --target-dir target/compiler-client --test multi_file measure_multi_method_order -- --ignored --nocapture
```
