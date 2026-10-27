# Cargo test profile experiment (#1766)

## Decision

**No-go: retain `[profile.test] opt-level = 1`.** The global `opt-level = 0` profile compiles much faster but fails the full workspace on the default test-thread stack. Selectively optimizing `dotty-classloader` moves the failure to `dotty-core`; optimizing both moves it to `dotty-tasty-unpickler`. None of the tested alternatives passed `cargo test --workspace --all-targets --locked`, so none is a correct workspace profile or a candidate for hosted CI timing. No Cargo or CI configuration change is recommended.

This is a local profile experiment, not evidence that selective optimization improves PR-to-green time. The normal pinned CI, JDK 21 gate, pinned Scala 3.9 consumer, and focused Typer audit passed on the PR tree with the unchanged `opt-level = 1` profile.

## Reproduction conditions

- Source tree: merged main commit `c7d2f33c3898f43fb4b622d313369be4b8b7f5c8` (PR #1772; based on `bf69a070380ff9e3a6a48dcaa1670b695c9cf159`). The Cargo source used for the benchmark was assembled from that base plus the exact classloader test hunk from the merged PR. The PR's other change is documentation-only and does not enter Cargo build inputs.
- Host: Debian x86_64, kernel `6.12.74+deb13+1-amd64`, AMD Ryzen 9 7940HS, 16 logical CPUs, 29 GiB RAM.
- Toolchain: `rustc 1.98.1 (48a229cea 2026-09-01)`, Cargo `1.98.1 (797e8a9bc 2026-08-05)`.
- Cargo registry was warm. Each profile used a fresh, separate target directory; target builds were cold. `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_TEST_DEBUG=0`, and `--locked` were held constant. No `RUST_MIN_STACK` override was set.
- Build time is the wall time for `cargo test --workspace --all-targets --locked --no-run --timings`. Test time is a complete subsequent `cargo test --workspace --all-targets --locked` invocation. Baseline test time has three repeated warm-target samples. Each clean build emitted 148 test-profile executables; executables are counted and sized from Cargo JSON artifacts. RSS is the largest child-process resident set reported by Linux `RUSAGE_CHILDREN` during the measured command group, rounded to MiB; it is a per-process maximum, not a sum.

## Results

| Profile | Clean test-target build | Full workspace test run | Executable bytes | Peak child RSS | Result |
| --- | ---: | ---: | ---: | ---: | --- |
| Repository baseline, `opt-level = 1` | 102.033 s | 15.937 s, 15.938 s, 15.939 s (median 15.938 s) | 458,856,056 | 1,739 MiB build; 403 MiB test | All three runs passed: 7,063 passed, 18 ignored |
| Candidate A, global `opt-level = 0` | 21.313 s | 64.736 s before abort | 933,718,576 | 1,482 MiB | Failed in `dotty-classloader` at `inheritance_ancestry_accepts_the_maximum_depth` with stack overflow; 936 tests completed before the failing target |
| Candidate B, global `0`, `dotty-classloader = 1` | 53.443 s | 275.061 s before abort | 877,551,856 | 1,858 MiB build; 402 MiB test | Failed in `dotty-core` at `types::rebind::tests::substitution_reuses_an_unchanged_binder_graph`; 1,427 passed and 7 ignored before the failing target |
| Candidate C, global `0`, `dotty-classloader = 1`, `dotty-core = 1` | 54.277 s | 342.344 s before abort | 847,698,808 | 1,852 MiB build/test group | Failed in `dotty-tasty-unpickler` at `a_deeply_nested_compound_type_is_an_error_not_a_stack_overflow`; 5,427 passed and 8 ignored before the failing target |

The combined clean-build plus first full-run observations were 117.971 s for the passing baseline, 86.049 s for Candidate A, 328.504 s for Candidate B, and 396.621 s for Candidate C. The faster result for Candidate A is invalid because it aborts. Candidates B and C both fail and take about 2.8× and 3.4× the passing baseline's combined time before aborting. Candidate B and C also produce 1.91× and 1.85× the baseline test-binary bytes. The executable count and test command stayed the same; no targets were dropped.

Baseline inventory was identical across all three runs: 7,063 passed, 0 failed, and 18 ignored. Cargo emitted 148 test-profile executables and 149 harness result summaries. Candidate A completed 43 harnesses (936 tests) before the classloader abort; Candidate B completed 50 harnesses (1,427 passed, 7 ignored); Candidate C completed 104 harnesses (5,427 passed, 8 ignored). Failed test runs are not counted as passing candidates.

## Profile flag verification

Candidates B and C were applied at the command line, without editing `Cargo.toml`:

```sh
cargo +1.98.1 \
  --config 'profile.test.opt-level=0' \
  --config 'profile.test.package.dotty-classloader.opt-level=1' \
  test --workspace --all-targets --locked --no-run --timings --message-format=json -vv

cargo +1.98.1 \
  --config 'profile.test.opt-level=0' \
  --config 'profile.test.package.dotty-classloader.opt-level=1' \
  --config 'profile.test.package.dotty-core.opt-level=1' \
  test --workspace --all-targets --locked --no-run --timings --message-format=json -vv
```

The verbose rustc invocations show `-C opt-level=1` for both the `dotty_classloader` library and its `--test` harness in Candidates B/C. In Candidate C, `dotty_core` and its test harness also compile at `opt-level = 1`; other workspace test targets remain at the global `opt-level = 0` (rustc's default, so no `-C opt-level` flag is emitted). This confirms the package overrides affect the intended package targets. The observed stack overflow in `dotty-core`, then in `dotty-tasty-unpickler`, shows that optimizing only one or two packages does not make the global unoptimized profile correct.

## Timing reports and commands

Cargo's clean-build timing reports are preserved alongside this note:

- [Baseline opt-level 1](ci-performance-baseline/issue-1766-opt1-cold-cargo-timing.html)
- [Candidate A: global opt-level 0](ci-performance-baseline/issue-1766-opt0-cold-cargo-timing.html)
- [Candidate B: classloader override](ci-performance-baseline/issue-1766-opt0-classloader1-cold-cargo-timing.html)
- [Candidate C: classloader and core overrides](ci-performance-baseline/issue-1766-opt0-classloader-core1-cold-cargo-timing.html)

Re-run the baseline and candidate matrix from the same source SHA with a warm registry and fresh target paths. For the repeated baseline execution samples, keep the same `CARGO_TARGET_DIR` after the initial clean build. Candidate A additionally uses `CARGO_PROFILE_TEST_OPT_LEVEL=0`; Candidate B/C use the `--config` arguments above. Save compiler artifact JSON, `-vv` stderr for the selective flag audit, timing HTML, full test stdout/stderr, wall time, and maximum child RSS for every profile. Do not compare these local measurements with hosted runner timings.

## Follow-up

#1765's stack-safety blocker prevents Candidate A. The selective experiments show additional failures in two other packages and sharply increased execution time before failure. Keep the current repository test profile until a separate, measured change makes the full all-target suite pass. No hosted full Typer corpus audit or profile change was run for this experiment.
