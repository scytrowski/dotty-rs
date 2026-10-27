# Issue #1769: cargo-nextest compared with libtest

## Decision

Keep the existing `cargo test --workspace --all-targets --locked` CI command.
Nextest ran the current harnesses successfully and reduced warm execution time
on this host, but the one clean-target build-and-test pair differed by only
6.6 seconds (4.5%). That is not a repeatable net win: the host load changed
substantially between runs, and installing the runner is an additional cost.
No CI workflow or test configuration was changed.

## Scope and test parity

The comparison used the CI test command from `.github/workflows/ci.yml`'s
`Rust checks` job. It sets `CARGO_INCREMENTAL=0` and
`CARGO_PROFILE_TEST_DEBUG=0`, pins Rust 1.98.1, and runs
`cargo test --workspace --all-targets --locked`. The benchmark used those same
build settings and command. `CARGO_TERM_COLOR` was set to `never` locally to
keep captured output stable; this does not affect compilation or test
selection. Runs were recorded at PR head
`9ae361c084f871121b690678427f0f0bb8fbcde4`; compared with base
`ab4d52c248242290404e70a43098c12e6de2083b`, that head adds only this report
and its evidence/reproduction files. Compiler sources, manifests, lockfile,
and CI configuration are identical. Each runner used an isolated target
directory.

Cargo metadata reports 149 targets across 16 packages: 14 libraries, 127
integration-test targets, 6 binaries, and 2 examples. One example is a
non-test build target. Both test discovery commands found 148 test harnesses
and exactly 7,089 identities (binary, test name, and kind): 7,071 runnable
tests and 18 ignored tests. The set difference was zero in both directions;
both runners passed all runnable tests and skipped the same 18 ignored tests.
Six discovered harnesses contain no test cases. No custom `harness = false`
target, benchmark target, repository Nextest configuration, or Nextest retry
setting was found.

Cargo documents `--all-targets` as `--lib --bins --tests --benches --examples`,
which excludes documentation tests. Nextest does not run doctests either, so
the compared command has no doctest coverage gap relative to the current CI
command. The separate JDK verification CI job and Scala oracle workflows were
not substituted by this benchmark. The local run did have Temurin JDK 21 on
`PATH`, so tests that opportunistically invoke Java could run.

## Runner and machine

- Rust: `rustc 1.98.1 (48a229cea 2026-09-01)`.
- Host: Linux x86_64, 16 logical CPUs, 29 GiB RAM; about 25 GiB available at
  the start of measurement.
- JDK: Temurin 21.0.8+9. `DOTTY_CLASSFILE_REQUIRE_JDK` was unset, matching the
  ordinary workspace test job rather than the separate strict JVM gate.
- Nextest: prebuilt `cargo-nextest 0.9.150`, commit
  `d225b1daf362c1ca8e5e048132618c1a81c57a28`, dated 2026-10-26. The official
  x86_64 GNU archive was downloaded from the
  [0.9.150 release](https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.150)
  and its SHA-256 verified as
  `b0959bc64232974224cf9fb22c915845398c0e8668fa7831eb5b43fb1eef781f`.
  The 12.2 MB archive expanded to a 36.5 MB binary. Download and extraction
  took 1.8 seconds in this run; network and installation setup are excluded
  from the clean-target timing below.
- Temporary files used a workspace-local directory because the system temp
  filesystem was full. Existing repository build artifacts were not reused.

The Nextest command was
`cargo nextest run --workspace --all-targets --locked --retries 0 --test-threads N`.
Zero retries matches Cargo's default. `N=2`, `N=4`, and `N=num-cpus` (16 on
this host) were exercised. Cargo's default run used libtest's default thread
count. No test sharding, filtering, or ignored-test override was used.

## Results

All full-suite runs exited successfully. Warm runs reused the same already
built target directory. The table shows wall-clock seconds measured around
the complete Cargo command, including Cargo startup and test discovery.

| Runner / concurrency | Repetitions | Times (s) | Median (s) |
| --- | ---: | --- | ---: |
| Cargo + libtest, default | 3 | 34.963, 35.270, 35.208 | 35.208 |
| Nextest, 16 (`num-cpus`) | 3 | 28.717, 28.586, 28.554 | 28.586 |
| Nextest, 4 | 1 | 37.935 | — |
| Nextest, 2 | 1 | 66.910 | — |

At the default concurrency, Nextest's warm-run median was 6.6 seconds (18.8%)
lower. Lowering Nextest concurrency increased elapsed time on this suite.

One clean-target build-and-test run per tool measured 146.573 seconds for
Cargo/libtest and 140.002 seconds for Nextest, a 6.571-second (4.5%)
difference. These runs used separate empty target directories, the same
checkout, profile settings, Rust toolchain, and already populated Cargo
registry cache. The Nextest binary had already been downloaded, so its
installation cost is not in that comparison.

During the measurements, host 1-minute load averages ranged from 4.65 to
25.12 and often rose while a run was in progress. In particular, the clean
Cargo run started at 6.79 and ended at 11.36, while the clean Nextest run
started at 11.36 and ended at 16.42. The environment was not an idle or
hosted-CI runner, so small end-to-end differences cannot be attributed to the
runner reliably.

Peak resident memory was sampled every 100 ms from `/proc` for the command's
visible process tree. The largest sampled tree sum was about 436 MiB for
Cargo/libtest and 65 MiB for Nextest; the largest individual process samples
were about 402 MiB and 35 MiB, respectively. Tree sums can double-count shared
pages, and 100 ms sampling can miss short-lived child processes; treat these
as rough observations, not comparable peak-memory measurements.

A scratch crate with one intentionally panicking test verified failure
propagation: Cargo exited 101 and Nextest exited 100, both with the test
identified as failed. No retries were enabled. All full-suite repetitions
passed; no flake or external-resource failure was observed. This local sample
does not establish hosted-runner flake rates or resource isolation behavior.

## Recommendation and reproduction

The warm execution improvement is real in this sample, but it does not
translate into a demonstrated end-to-end CI win: the clean-target advantage
was small, tool setup was excluded, host load was unstable, and only one
clean-target pair was measured. Keep the current command and revisit only
with repeated measurements on a stable runner, including the runner install
step and any required separate CI gates.

The core commands used were:

```sh
cargo test --workspace --all-targets --locked
cargo nextest run --workspace --all-targets --locked --retries 0 --test-threads num-cpus
```

The compressed raw listings, identity-diff outputs, environment record, and
all measured run logs are in
[`issue-1769-evidence.tar.gz`](issue-1769-evidence.tar.gz).
Its SHA-256 is recorded in
[`issue-1769-evidence.tar.gz.sha256`](issue-1769-evidence.tar.gz.sha256).
The inventory comparator and benchmark driver are alongside this report in
[`issue-1769-evidence/`](issue-1769-evidence/README.md).

Nextest references: [prebuilt binaries](https://www.nexte.st/docs/installation/pre-built-binaries/),
[configuration reference](https://www.nexte.st/docs/configuration/reference/),
[test listing](https://www.nexte.st/docs/listing/), and
[stability/MSRV policy](https://nexte.st/docs/stability/#minimum-supported-rust-versions).
Cargo's target-selection semantics are in the
[Cargo test command reference](https://doc.rust-lang.org/cargo/commands/cargo-test.html).
