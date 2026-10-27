# GitHub Actions CI performance baseline

Baseline recorded on 2026-10-27 for issue [#1735](https://github.com/scytrowski/dotty-rs/issues/1735). It describes existing workflow behavior and successful runs observed on 2026-10-26. These observations are diagnostic samples, not regression thresholds.

## Scope and check policy

The eight existing workflows are listed below. `CI`, `JVM TASTy factory Scala consumer`, and `Typer focused audit` start automatically for pull requests, with the latter two restricted by path filters. The other five workflows are opt-in through `workflow_dispatch`. The Typer full-corpus workflow requires an explicit `run-full-audit` input and defaults to `do-not-run`.

The active GitHub ruleset for the default branch currently contains only the `non_fast_forward` rule. It does not define required status checks. Therefore, the automatically triggered pull request workflows below are observed PR checks, but GitHub does not enforce them as required checks under this ruleset. Confirm this policy before describing any status as a required merge gate.

Workflow-level concurrency is part of the latency context. CI cancels an older run for the same workflow/ref; the Scala consumer cancels superseded pull-request runs but leaves manual runs alone; Typer focused and Cargo timings allow an existing run to finish; the manual compatibility workflows cancel an older run for the same ref. All job layouts are single-job workflows except CI, which has two independent parallel jobs. None uses a matrix or cross-job `needs` dependency. The full-corpus workflow has one `audit` job, conditionally created only when the dispatch input is `run-full-audit`.

| Workflow | Concurrency group | `cancel-in-progress` | Job shape |
| --- | --- | --- | --- |
| CI | `ci-${{ github.workflow }}-${{ github.ref }}` | `true` | `checks` and `classfile-jvm-encoder`, independent and parallel. |
| JVM TASTy factory Scala consumer | `jvm-tasty-factory-scala-consumer-${{ github.event.pull_request.number || github.ref }}` | `true` for pull requests; `false` for manual dispatch | One `consume-factory` job. |
| Typer focused audit | `typer-focused-audit-${{ github.event.pull_request.number || github.ref }}` | `false` | One `focused-audit` job. |
| Typer full corpus audit | `typer-full-corpus-audit-${{ github.ref }}` | `false` | One conditional `audit` job. |
| Lexer compatibility | `lexer-compatibility-${{ github.ref }}` | `true` | One `verify` job. |
| Scala parser oracle | `parser-oracle-${{ github.ref }}` | `true` | One `parser-oracle` job. |
| TASTy compatibility | `tasty-compatibility-${{ github.ref }}` | `true` | One `verify` job. |
| CI Cargo compile timings | `ci-cargo-timings-${{ github.ref }}` | `false` | One `cargo-timings` job. |

| Workflow | Trigger and gate status | Main cost centers and artifacts | Sampling in the existing history |
| --- | --- | --- | --- |
| [CI](../.github/workflows/ci.yml) | Push to `main`, every PR targeting `main`, or manual dispatch. Two jobs run in parallel. Automatic PR check; not ruleset-required. | `Rust checks`: workflow lint, dependency-only Rust cache, pinned Rust toolchain install, fmt, boundary guard, Clippy, rustdoc, workspace tests. `Classfile JVM encoder gate`: JDK 21 setup, Cargo cache, Java verification tests. No workflow artifact upload. | Four recent single-attempt successful PR runs are measured below; a retried successful run is detailed separately. |
| [JVM TASTy factory Scala consumer](../.github/workflows/jvm-tasty-factory-scala-consumer.yml) | PRs targeting `main` that touch its explicit source/test/oracle/doc paths, or manual dispatch. Automatic path-filtered PR check; not ruleset-required. | Pinned Scala 3.9.0 source checkout and `sbt dist/universal:stage`, pinned JDK/Rust, verified Maven artifacts, Scala/TASTy consumer tests, then Java 21 verification. Caches Coursier and Cargo outputs. No report artifact. | Five recent successful PR runs are measured below. |
| [Typer focused audit](../.github/workflows/typer-focused-audit.yml) | PRs targeting `main` that touch the listed typer/classloader/TASTy/audit paths, or manual dispatch. Automatic path-filtered PR check; not ruleset-required. | Pinned Scala/JDK/Rust setup, verified Scala dependencies, deterministic focused audit. Uploads environment, timings, reports, and intermediates for 30 days. The build-parallelism candidate and repeated resource measurements are recorded in [the #1739 audit](typer-ci-overhead-1739.md). | Five recent successful PR runs are measured below. |
| [Typer full corpus audit](../.github/workflows/typer-full-corpus-audit.yml) | Manual dispatch only; `run_full_audit` defaults to `do-not-run`; the audit job runs only for `run-full-audit`. Not a PR check. | Pinned Scala/JDK/Rust, dependencies, two sequential full audits and reconciliation; timeout is six hours. Uploads reports, sidecars, inputs, and intermediates for 30 days. No Cargo cache is configured. | No successful run of the current manual workflow was found. Historical workflow records are failed push events from an earlier configuration. No hosted full-corpus audit was launched for this baseline. |
| [Lexer compatibility](../.github/workflows/lexer-compatibility.yml) | Manual dispatch only, selecting fixtures, source corpus, or both. Not a PR check. | JDK 25 and sbt setup, stable Rust install, Scala lexer oracle and selected corpus comparison. The JDK setup caches sbt dependencies. No uploaded report artifact. | Two manual runs are present; both failed. No successful timing sample. |
| [Scala parser oracle](../.github/workflows/parser-oracle.yml) | Manual dispatch only. Not a PR check. | JDK 25, sbt setup, stable Rust install, Scala 3.9 parser differential oracle. Caches Coursier/sbt state. No uploaded report artifact. | No runs found in the repository's recorded workflow history. |
| [TASTy compatibility](../.github/workflows/tasty-compatibility.yml) | Manual dispatch only, selecting all, scala3-library, or scala3-compiler. Not a PR check. | JDK 25 and sbt setup, stable Rust install, structured TASTy round trips for the selected fixture corpus. The JDK setup caches sbt dependencies. No uploaded report artifact. | One successful run, 2026-09-17; it is a one-off observation, not a baseline distribution. |
| [CI Cargo compile timings](../.github/workflows/ci-cargo-timings.yml) | Manual diagnostic workflow. Selects full target cache, dependency-only cache, or cold mode, plus compile-only or full Rust checks. Not a PR check. | Compile-only mode runs test-target compilation with `--no-run --timings`; full mode runs the production Rust checks workload. Uploads Cargo HTML timing output and environment metadata for 30 days. | Used for the controlled cache comparison in #1736; profile comparison for #1737 is recorded below. |

The hosted full-corpus job remains opt-in and was not dispatched. The failure-only historical lexer records and the old push-event full-corpus records are not used as successful timing samples.

## Existing successful samples

Four single-attempt CI samples and five Scala-consumer and focused-audit samples below are successful PR runs from 2026-10-26 on `ubuntu-latest`. A fifth CI workflow completed successfully only after a failed JVM-gate attempt; it is reported separately and excluded from successful single-attempt CI distributions and the cross-workflow critical-path sample. The main Rust checks use the repository's Rust 1.98.1 toolchain file; the CI JVM gate explicitly invokes `+stable`, whose cache-key step records 1.99.1 in these samples. The Scala consumer and focused audit pin Rust 1.98.1, Scala source revision `777528f19a58e794c9954a42f433373472ec57f8`, and JDK 21.0.8. Times use GitHub's run creation timestamp as the start proxy and the last job completion as the end. Queue delay is shown separately from job execution.

Only successful single-attempt runs are used in the summary distributions; cancelled and failed attempts remain visible in Actions history and are not counted as successful samples. Of the archived logs available for the four single-attempt CI samples, two Rust-job logs show warm primary-key hits and two Rust-job logs are unavailable. Two Scala-consumer logs show cache misses followed by cache saves; one Typer-focused log shows the same miss/save pattern. The remaining specialty logs are unavailable. Those are incidental cache states; the controlled full-versus-dependency-only timing runs for issue #1736 are recorded below.

### Main CI jobs

| Run (short SHA) | Rust checks | JVM encoder gate | First-job queue | CI elapsed to last job |
| --- | ---: | ---: | ---: | ---: |
| [40415404197](https://github.com/scytrowski/dotty-rs/actions/runs/40415404197) (`d9f4cab`) | 9m47s | 2m13s | 3s | 9m50s |
| [40412150151](https://github.com/scytrowski/dotty-rs/actions/runs/40412150151) (`2c2c868`) | 10m05s | 3m13s | 3s | 10m08s |
| [40407343107](https://github.com/scytrowski/dotty-rs/actions/runs/40407343107) (`e1d2eee`) | 7m43s | 3m02s | 89s | 9m12s |
| [40407034274](https://github.com/scytrowski/dotty-rs/actions/runs/40407034274) (`387c9c6`) | 9m48s | 2m45s | 7s | 9m55s |

Across these four single-attempt runs, CI elapsed median was **9m53s**, with a range of **9m12s–10m08s**. Rust checks took a median **9m48s** (7m43s–10m05s); the JVM encoder job took a median **2m54s** (2m13s–3m13s). The elapsed value is the latest finishing parallel job, not the sum of both jobs.

Run [40410953298](https://github.com/scytrowski/dotty-rs/actions/runs/40410953298) (`f7e6bd1`) ultimately succeeded after a retry and is excluded from those distributions. Attempt 1 started both jobs within 3s of run creation: Rust checks completed successfully at 22:24:57Z, while the JVM encoder gate failed at 22:17:28Z. Attempt 2 was created at 22:26:23Z; its JVM gate started 3s later and completed successfully at 22:29:20Z. Thus the JVM gate's runner queue was 2s on attempt 1 and 3s on attempt 2, while the end-to-end successful run elapsed 14m20s from initial creation to final completion. This elapsed includes the failed attempt and the interval before retry; it is not a runner-queue or single-attempt duration.

| Rust checks step, across the same four single-attempt runs | Median | Range |
| --- | ---: | ---: |
| Runner setup | 1s | 1s–1s |
| Repository checkout | 2.5s | 2s–3s |
| Cargo registry/build-cache restore step | 1m51s | 1m19s–2m20s |
| Post-cache action step | 3s | 2s–3s |
| Install stable Rust toolchain | 9.5s | 9s–10s |
| Clippy | 21s | 15s–22s |
| Rustdoc | 9.5s | 7s–10s |
| `cargo test --workspace --all-targets --locked` step | 6m50s | 4m42s–7m23s |
| JVM verification command | 2m35s | 1m51s–2m43s |

These are GitHub step wall times, not CPU measurements. In [CI run 40415404197](https://github.com/scytrowski/dotty-rs/actions/runs/40415404197), the cache restore was 5,269,214,731 bytes (about 5.27 GB or 5,025 MiB). Cargo reported `Finished test profile ... in 4m 35s`; the `Run tests` step ended about 1m43s later. The remaining time includes test execution and process/log overhead. A second available CI log, [40412150151](https://github.com/scytrowski/dotty-rs/actions/runs/40412150151), also shows a primary-key hit for the same-size archive and a Cargo compile time of 5m10s. Both cache hits were followed by “not saving cache.” Across these two archived logs, Cargo compile time had a 4m52.5s median (4m35s–5m10s). The run-level log archive did not include Rust-check job logs for the other two selected runs, so their cache outcome and Cargo compile/runtime split are unknown. The four-run step medians above remain available from the Actions job timing API.

### Specialty workflows with recurring PR samples

| Workflow and successful samples | Queue median (range) | Job/run elapsed median (range) | Largest measured step |
| --- | ---: | ---: | --- |
| Scala consumer: [40415404272](https://github.com/scytrowski/dotty-rs/actions/runs/40415404272), [40412150338](https://github.com/scytrowski/dotty-rs/actions/runs/40412150338), [40410952824](https://github.com/scytrowski/dotty-rs/actions/runs/40410952824), [40407343386](https://github.com/scytrowski/dotty-rs/actions/runs/40407343386), [40407034283](https://github.com/scytrowski/dotty-rs/actions/runs/40407034283) | 3s (2s–5m31s) | 11m24s (10m11s–18m50s) | Runner setup: 3s median; repository/Scala checkouts: 3s/4s; JDK/Rust setup: 5s/8s. Scala source staging: median 5m42s (4m56s–7m36s). Strict Scala/TASTy gates: 2m49s median. Java verification: 1m32s median. Cache restore: 3s median (2s–15s); post-cache step: 12s median (8s–15s). Logs for two samples show a cache miss and a post-job cache save; three sample cache outcomes are unknown. |
| Typer focused audit: [40415404244](https://github.com/scytrowski/dotty-rs/actions/runs/40415404244), [40410953050](https://github.com/scytrowski/dotty-rs/actions/runs/40410953050), [40409415640](https://github.com/scytrowski/dotty-rs/actions/runs/40409415640), [40407034265](https://github.com/scytrowski/dotty-rs/actions/runs/40407034265), [40402974554](https://github.com/scytrowski/dotty-rs/actions/runs/40402974554) | 3s (2s–3s) | 3m50s (2m58s–4m15s) | Runner setup: 1s median; repository/Scala checkouts: 3s/4s; JDK/Rust setup: 4s/8s. Deterministic audit: median 3m15s (2m22s–3m42s). Cache restore/post-cache: 2s each median. Artifact upload: 2s median (1s–2s). One sample log shows a cache miss followed by a post-job save; four sample cache outcomes are unknown. |

The specialty workflow run elapsed medians include time between run creation and first job start. For the Scala consumer, the five run elapsed values were 11m24s, 10m49s, 13m28s, 18m50s, and 10m11s. Its largest sample had 5m31s of pre-job queueing; that is runner wait, not Scala compilation.

### Pull-request critical path across concurrently triggered workflows

The following four PR heads have single-attempt successful CI and successful Scala-consumer runs. Typer appears only when its path filter matched. The retried `f7e6bd1` CI run is excluded here; its 14m20s end-to-end elapsed is reported above. For each head, the critical path is from the earliest workflow creation timestamp to the latest completion among the automatic PR workflows that actually ran. It is not the sum of parallel workflow durations.

| PR head | CI | Scala consumer | Typer focused | Last automatic check / critical path |
| --- | ---: | ---: | ---: | --- |
| `d9f4cab` | [9m50s](https://github.com/scytrowski/dotty-rs/actions/runs/40415404197) | [11m25s](https://github.com/scytrowski/dotty-rs/actions/runs/40415404272) | [3m50s](https://github.com/scytrowski/dotty-rs/actions/runs/40415404244) | Scala consumer, **11m25s** |
| `2c2c868` | [10m08s](https://github.com/scytrowski/dotty-rs/actions/runs/40412150151) | [10m49s](https://github.com/scytrowski/dotty-rs/actions/runs/40412150338) | Not triggered | Scala consumer, **10m49s** |
| `e1d2eee` | [9m12s](https://github.com/scytrowski/dotty-rs/actions/runs/40407343107) | [18m50s](https://github.com/scytrowski/dotty-rs/actions/runs/40407343386) | Not triggered | Scala consumer, **18m50s** |
| `387c9c6` | [9m55s](https://github.com/scytrowski/dotty-rs/actions/runs/40407034274) | [10m12s](https://github.com/scytrowski/dotty-rs/actions/runs/40407034283) | [2m59s](https://github.com/scytrowski/dotty-rs/actions/runs/40407034265) | Scala consumer, **10m12s** |

Across these four observed PR updates, the all-triggered-workflow critical-path median was **11m07s**, with a maximum of **18m50s**. Run creation is only a proxy for the exact PR-update timestamp; the links allow rechecking the associated run records. These are samples, not an SLO or a regression threshold.

## Manually dispatched workflow observations

| Workflow | Available recent evidence | Limitation |
| --- | --- | --- |
| Typer full corpus | Current YAML and associated report-generation path inspected. | Current manual workflow has no successful sample. It is a six-hour, two-pass opt-in audit; no hosted run was launched for #1739. |
| Lexer compatibility | Two manual runs exist: [38140224461](https://github.com/scytrowski/dotty-rs/actions/runs/38140224461) and [38140097123](https://github.com/scytrowski/dotty-rs/actions/runs/38140097123), both failed in the selected-corpus step after 42s and 41s. | Setup through normalizer took 11–12s; these failures are not representative successful-cost samples. |
| Parser oracle | No recorded run. | No timing sample. |
| TASTy compatibility | One successful [run 35215208912](https://github.com/scytrowski/dotty-rs/actions/runs/35215208912) on 2026-09-17; job took 4m49s after start, with a 4m26s selected-corpus step. | Setup through toolchain installation took 16s; one observation only, with no median or tail estimate. |

The five remaining workflow paths and the focused Typer build/RSS experiment are analyzed in [the #1739 overhead report](typer-ci-overhead-1739.md).

## Cargo compile timing artifact

Use the manual **CI Cargo compile timings** workflow on the branch/commit to measure. Select `full` to restore the former full `target/debug` cache, `rust-cache` to use the pinned dependency-only strategy evaluated below, or `cold` to skip cache restoration. All modes run the same command:

```sh
cargo test --workspace --all-targets --locked --no-run --timings
```

The generated `target/cargo-timings/` HTML report and `environment.tsv` are uploaded as a 30-day artifact, including on a failed compile when Cargo produced a report. This diagnostic is excluded from push and pull-request triggers, so normal required-workflow cost is not increased by a redundant compile. The artifact measures compilation/linking for the test targets; it does not measure test execution. Compare full-cache, dependency-only, and cold results separately and include cache mode in any report.

## Cache measurements for issue #1736

The GitHub Actions cache inventory was sampled on 2026-10-27 before the strategy change. It contained 22 active entries totaling 11,313,260,641 bytes (11.31 GB) across the repository. The run links below provide the per-workflow timings; [the cache inventory API](https://api.github.com/repos/scytrowski/dotty-rs/actions/caches?per_page=100) exposes each entry's exact archived size, key, ref scope, and last access.

| Workflow cache | Stored entries and scope | Observed restore/save behavior | Interpretation |
| --- | --- | --- | --- |
| Main CI Rust checks | One `refs/heads/main` entry, 5,269,214,731 bytes. | Four selected PR restore steps: median 1m51s (1m19s–2m20s); post-cache step median 3s. Two archived logs confirm exact-key hits and skipped saves; three outcomes are unavailable. | The large target archive dominates measured cache transfer. The cache key was OS + `Cargo.lock` hash + fixed profile labels, without the Rust version. |
| Classfile JVM encoder gate | One `refs/heads/main` entry, 26,896,335 bytes. Key includes the resolved `+stable` Rust version (1.99.1 in the sampled runs), lockfile hash, OS, and profile labels. | Four cache restore steps: median 2s (1s–5s); post-cache median 0s (0s–1s). | Small archive, with no PR-scoped duplicates in this snapshot. Keep the separate key because this gate uses a different Rust toolchain from the main checks. |
| Typer focused audit | Four PR-scoped entries, 17,434,595–17,588,113 bytes each (70,010,391 bytes total). Key includes Rust 1.98.1, OS, lockfile hash, and test profile labels. | Five restore steps: median 2s (1s–3s); post-cache median 2s (1s–3s). One archived log confirms a miss and save; four cache outcomes are unavailable. | Each PR ref has its own small cache scope. The measured transfer is small relative to the focused audit. |
| Scala consumer Cargo/Coursier cache | Eight PR-scoped entries, 678,502,180–707,640,812 bytes each (5,509,980,401 bytes total). The key includes Rust 1.98.1, OS, lockfile hash, and pinned Scala 3.9.0. No `refs/heads/main` entry existed in the snapshot. | Five restore steps: median 3s (2s–15s); post-cache median 12s (8s–15s). Two archived logs confirm misses and saves; three cache outcomes are unavailable. | GitHub cache scopes prevent one PR's cache from being reused by another PR; each entry can be reused within its own PR ref and by reruns in that scope. The workflow's Scala staging step remains much larger at a 5m42s median. |
| sbt runner setup used by the Scala consumer | Eight PR-scoped entries of about 54.6 MB each (437,158,783 bytes total). | Saved/restored internally by `sbt/setup-sbt`; it has no separate cache step in the workflow timing table. | The setup action also creates per-PR entries. |

The main CI benchmark compared the same pinned Rust 1.98.1 and the same compile command on one commit. The full strategy restored the existing 5.27 GB cache. The alternative is pinned [Swatinem/rust-cache](https://github.com/Swatinem/rust-cache), configured to retain dependency artifacts while excluding workspace crates. Its cache key incorporates the Rust environment and toolchain; PR saves remain in GitHub's PR-specific scope, so they cannot update the trusted `main` cache under [GitHub's cache scope rules](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching#restrictions-for-accessing-a-cache).

| Strategy | Run | Cache result | Restore | Compile command | Benchmark job wall time |
| --- | --- | --- | ---: | ---: | ---: |
| Full target cache | [40424270004](https://github.com/scytrowski/dotty-rs/actions/runs/40424270004) | Hit | 2m01s | 4m08s | 6m30s |
| Full target cache | [40425101840](https://github.com/scytrowski/dotty-rs/actions/runs/40425101840) | Hit | 2m35s | 5m43s | 8m39s |
| Full target cache | [40426095182](https://github.com/scytrowski/dotty-rs/actions/runs/40426095182) | Hit | 1m42s | 5m48s | 7m52s |
| Dependency-only cache, first run | [40426988756](https://github.com/scytrowski/dotty-rs/actions/runs/40426988756) | Miss; seeded cache | 1s | 4m16s | 4m34s |
| Dependency-only cache, warm | [40427583531](https://github.com/scytrowski/dotty-rs/actions/runs/40427583531) | Hit | 2s | 5m52s | 6m13s |
| Dependency-only cache, warm | [40428266403](https://github.com/scytrowski/dotty-rs/actions/runs/40428266403) | Hit | 2s | 5m45s | 6m06s |
| Dependency-only cache, warm | [40428989889](https://github.com/scytrowski/dotty-rs/actions/runs/40428989889) | Hit | 1s | 4m15s | 4m31s |

Across the three warm compile-only runs, the full cache had a **7m52s median benchmark-job wall time** (6m30s–8m39s), a **2m01s median restore**, and a **5m43s median compile**. Dependency-only caching had a **6m06s median warm job wall time** (4m31s–6m13s), a **2s median restore**, and a **5m45s median compile**. The compile medians are effectively equal; avoiding the 5.27 GB transfer produced a measured 1m46s (about 22%) lower median compile-benchmark job time. After the experiment, the dependency-only cache entry on its benchmark branch was 12,347,030 bytes, about 427 times smaller than the full archive. The first dependency-only cache miss completed in 4m34s and created that entry. These are three-run samples on hosted runners, not an SLO, and this benchmark excludes test execution.

### Same-commit full Rust-check comparison

Both cache strategies were measured on commit `287880316cebe239e5ea7c754bcb77038e93d6a4`, with Rust 1.98.1, the same `ubuntu-latest` runner image, and the complete Rust checks workload: workflow lint, formatting, frontend-boundary guard, Clippy, rustdoc, workspace tests, and diff checking. The legacy strategy restores the full target cache through `actions/cache`; the dependency-only strategy uses the production `Swatinem/rust-cache` configuration. All six runs passed and reported cache hits.

| Cache strategy | Run | Restored cache | Rust checks job wall time |
| --- | --- | ---: | ---: |
| Full target cache | [40434782259](https://github.com/scytrowski/dotty-rs/actions/runs/40434782259) | 5,269,214,731 B; 1m38s | 10m38s |
| Full target cache | [40434792902](https://github.com/scytrowski/dotty-rs/actions/runs/40434792902) | 5,269,214,731 B; 1m32s | 10m34s |
| Full target cache | [40436303159](https://github.com/scytrowski/dotty-rs/actions/runs/40436303159) | 5,269,214,731 B; 2m28s | 11m17s |
| Dependency-only cache | [40434152387, attempt 1](https://github.com/scytrowski/dotty-rs/actions/runs/40434152387?attempt=1) | 27,026,064 B; 2s | 5m51s |
| Dependency-only cache | [40434152387, attempt 2](https://github.com/scytrowski/dotty-rs/actions/runs/40434152387?attempt=2) | 27,026,064 B; 1s | 7m16s |
| Dependency-only cache | [40434152387, attempt 3](https://github.com/scytrowski/dotty-rs/actions/runs/40434152387?attempt=3) | 27,026,064 B; 2s | 6m47s |

The full target cache had a **10m38s median** Rust-check job wall time (10m34s–11m17s) and a **1m38s median restore** (1m32s–2m28s). The dependency-only cache had a **6m47s median** (5m51s–7m16s) and a **2s median restore** (1s–2s). That is a measured **3m51s (about 36%) lower median job time** for the dependency-only strategy on this same workload and commit. Its 27.0 MB archive is about **195 times smaller** than the 5.27 GB full target archive.

One complete PR CI run on this commit finished in **6m21s** from workflow creation to the last of its two parallel jobs; the dependency-cache Rust checks finished in 5m51s and the unchanged JDK 21 gate in 1m51s. This is one end-to-end workflow observation. The repeated comparison above covers the full Rust checks job, while the parallel JVM gate remains unchanged because its measured archive is small and toolchain-specific.

The Typer cache transfers are small; the Scala consumer staged compiler path is measured for issue #1738 below. Preserve the Typer configuration and keep its sizes and observed cache outcomes visible for the follow-up issues in the Refactor milestone.

## Pinned Scala 3.9 consumer cache and queue measurements (#1738)

The baseline uses the five successful consumer runs above with Scala source SHA `777528f19a58e794c9954a42f433373472ec57f8`, JDK 21.0.8, Rust 1.98.1, and `ubuntu-latest`. Durations below are from the job's step timestamps, which separate the Scala source stage, combined Rust/linked Scala-TASTy tests, and Java verifier from setup. Job elapsed excludes runner queue; the end-to-end sample with a 5m31s queue took 18m50s.

| Baseline run | Scala source stage | Rust and Scala/TASTy gates | Java 21 verifier | Consumer job elapsed |
| --- | ---: | ---: | ---: | ---: |
| [40415404272](https://github.com/scytrowski/dotty-rs/actions/runs/40415404272) | 5m42s | 3m01s | 1m39s | 11m22s |
| [40412150338](https://github.com/scytrowski/dotty-rs/actions/runs/40412150338) | 5m26s | 2m49s | 1m31s | 10m46s |
| [40410952824](https://github.com/scytrowski/dotty-rs/actions/runs/40410952824) | 5m48s | 2m37s | 1m30s | 10m59s |
| [40407343386](https://github.com/scytrowski/dotty-rs/actions/runs/40407343386) | 7m36s | 3m08s | 1m37s | 13m19s |
| [40407034283](https://github.com/scytrowski/dotty-rs/actions/runs/40407034283) | 4m56s | 2m48s | 1m32s | 10m09s |

The baseline medians are **5m42s** for source staging, **2m49s** for the combined Rust/Scala-TASTy gate, **1m32s** for Java verification, and **10m59s** for the consumer job. Pre-stage job setup, checkout, JDK/Rust setup, cache restore, and sbt runner setup took 32–47s (35s median). The separately verified pinned Maven artifact download step took 0–1s. Existing Coursier/Rust cache restore was 2–15s (3s median), and its post-job save was 8–15s (12s median). Two archived samples show a cache miss and save; the other three cache outcomes are unavailable. The `sbt/setup-sbt@v1` runner cache was already present at about 54.6 MB, with a 4–9s setup and 8–9s post step; it does not cache the Scala source project's compiled outputs.

The experiment adds `setup-java@v4` sbt dependency caching and a separate cache of Scala project `target` directories. The target cache key includes runner OS/architecture, the exact pinned Scala source SHA, JDK 21.0.8, and a hash of the Scala build definitions (including `project/build.properties`, which pins sbt). The workflow checks out and verifies that same source SHA before restoring it. The cache is saved only after `dist/universal:stage` succeeds and the pinned Maven artifacts pass SHA256 verification. It does not replace or skip any strict consumer test or Java verifier.

| Candidate run | Cache state | Scala source stage | Rust and Scala/TASTy gates | Java 21 verifier | Consumer job elapsed |
| --- | --- | ---: | ---: | ---: | ---: |
| [40461901018, attempt 1](https://github.com/scytrowski/dotty-rs/actions/runs/40461901018/attempts/1) | Cold miss; populated caches | 8m04s | 5m06s | 2m17s | 16m58s |
| [40461901018, attempt 2](https://github.com/scytrowski/dotty-rs/actions/runs/40461901018/attempts/2) | Warm hit | 57s | 4m10s | 1m38s | 7m52s |
| [40461901018, attempt 3](https://github.com/scytrowski/dotty-rs/actions/runs/40461901018/attempts/3) | Warm hit | 58s | 4m18s | 1m38s | 8m17s |
| [40465699896](https://github.com/scytrowski/dotty-rs/actions/runs/40465699896) | Warm hit | 56s | 4m16s | 1m39s | 7m58s |

The three warm runs had a **7m58s median** job elapsed, 3m01s (about 28%) below the five-run baseline median. Warm source staging fell from 5m42s to 57s; the 643,190,038-byte source-build cache restored in 7–11s. Pre-stage setup and restores took 46–60s (46s median), including the pinned JDK/Rust installations, the 369,769,288-byte sbt dependency cache and the existing Rust/Coursier cache. The sbt dependency cache reported a hit on all warm runs; its restore is included in the 6–8s JDK setup step. The explicit pinned Maven artifact download remained 0s on warm runs. The existing Rust/Coursier cache remained 708,392,355 bytes, and the sbt runner cache remained 54,644,841 bytes. The two new caches therefore add about 1.01 GB to the PR-scoped cache footprint. The cold first run took 16m58s and is slower than the baseline median; the measured win applies to subsequent updates to the same PR. GitHub keeps these cache entries scoped to the PR ref, so a new PR starts cold unless a default-branch cache is seeded.

The warm Rust/Scala-TASTy step includes test compilation and execution. On attempt 2 Cargo reported 3.13s to build test targets; test-binary runtimes were 13.67s (`jvm_tasty_cross_class_factories`), 35.81s (`jvm_tasty_cross_class_scala_consumer`), 48.13s (`jvm_tasty_factory_scala_consumer`), 55.58s (`jvm_tasty_multiclass_scala_consumer`), 55.87s (`jvm_tasty_object_composition_scala_consumer`), 16.01s (`jvm_tasty_scala_consumer`), and 21.69s (`tasty_source_pickling`). Cargo's Java-gate targets built in 1.86s; their test times were 69.61s (`jvm_source_compilation`), 4.07s (`jvm_stateful_class_contract`), 2.89s (`jvm_tasty_artifacts`), and 12.87s (`classfile_jvm_encoder`). All reported gates passed in all three candidate attempts.

The existing PR filter covers the root crate, workspace crates, integration tests and fixtures, lockfile/toolchain, consumer workflow, and TASTy oracle inputs. Narrowing `crates/**` or `tests/**` could skip changes that affect generated TASTy or its interoperability fixtures, so the filter remains broad. Concurrency history shows a concrete stale-run queue: on PR branch `codex/issue-1744-option-equality`, run [40433634380](https://github.com/scytrowski/dotty-rs/actions/runs/40433634380) was created at 01:37:06 UTC while [40432673424](https://github.com/scytrowski/dotty-rs/actions/runs/40432673424) was still running until 01:42:34; the newer run began at 01:45:25, 8m19s after creation. The workflow now cancels older pull-request runs in that PR-specific group, while manual benchmark runs remain non-cancelling. This keeps the latest commit's full Scala/TASTy and Java checks intact and avoids spending runner time on an obsolete head.

## Rust test compilation and linking profile (#1737)

The pinned test profile uses `opt-level = 1`. I compared it with `opt-level = 0` on source revision [`1d4d4ce`](https://github.com/scytrowski/dotty-rs/commit/1d4d4cee70e66fdc7b673c16c7fb48126c03bd8f), using Rust/Cargo 1.98.1, `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_TEST_DEBUG=0`, and separate initially empty target directories. This was a local Linux x86_64 host with 16 logical CPUs and 29 GiB RAM; the Cargo registry was warm. The clean compile is therefore a cold target build, not a cold dependency download.

| Test profile | Cold all-target build (`--no-run --timings`) | Full test execution | Combined observation | Maximum single child RSS | Result |
| --- | ---: | ---: | ---: | ---: | --- |
| `opt-level = 1` (repository default) | 97.9s | 17.3s | 115.3s | 1.33 GiB | 7,058 passed; 18 ignored |
| `opt-level = 0` | 18.9s | Aborted after 66.5s | Incomplete | 1.18 GiB | Stack overflow in `loader::tests::inheritance_ancestry_has_a_bounded_depth` |

The faster unoptimized build is not a valid CI candidate: it fails an existing all-targets test with the runner's default test-thread stack. The repository's optimized test profile remains unchanged. Under the passing profile, three warm compile-only invocations were 0.067s–0.073s (0.069s median); three subsequent full test executions passed with 17.1s median (16.965s–18.414s). The cold build plus test run and the warm compile plus test samples keep compilation and execution separate rather than attributing test runtime to compilation.

The [preserved cold Cargo HTML report](ci-performance-baseline/issue-1737-opt1-cold-cargo-timing.html) is the timestamped report from the default-profile run (`2026-10-27T06:02:05Z`). It recorded 197 dirty units, a 97.9s build wall time, and maximum concurrency of 17 units (`jobs=16`, 16 logical CPUs). The slowest units were:

| Compilation unit | Unit time |
| --- | ---: |
| `dotty-classloader` library test build | 86.3s |
| `dotty-parser` library test build | 72.6s |
| `dotty-typer` library test build | 65.2s |
| `dotty-typer` `local_definition_audit` integration test | 62.1s |
| `dotty-tasty-pickler` `source_pipeline` integration test | 49.8s |
| `dotty-typer` `package_prefix_parity` integration test | 41.5s |
| `dotty` `jvm_source_compilation` integration test | 34.6s |
| `dotty` `classpath_source_typing` integration test | 33.8s |
| `dotty-jvm` library test build | 25.6s |
| `dotty` `jvm_tasty_plan` integration test | 25.0s |

Unit times overlap under parallel compilation and must not be summed as wall time. Cargo's stable HTML timing report does not separate the final linker phase from each Rust compilation unit. The same clean build emitted 148 test-profile executables: 126 integration-test targets, 14 library test harnesses, six binary test harnesses, and two example harnesses. Their output files totaled 447,926,632 bytes (about 427 MiB). Each integration target therefore incurs its own Rust codegen/link step; the current timing report attributes that work to its compilation unit. The targets cover distinct fixtures and suites, so none was consolidated without evidence that its process isolation and assertions were redundant.

I also measured the production check order on one fresh target directory to see whether Clippy and rustdoc outputs carry into test compilation. The sequence used Rust/Cargo 1.98.1, the same source revision and environment as the profile comparison, then ran Clippy, rustdoc, test compilation, and the full test suite in order:

| Step | Cargo profile | Cargo timing units | Wall time | Result |
| --- | --- | ---: | ---: | --- |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | `dev` | 0 fresh, 198 dirty | 7.7s | Passed |
| `cargo doc --workspace --no-deps --locked` (`RUSTDOCFLAGS=-D warnings`) | `dev` | 30 fresh, 32 dirty | 4.5s | Passed; 30 units were fresh after Clippy |
| `cargo test --workspace --all-targets --locked --no-run` | `test` | 0 fresh, 197 dirty | 91.2s | Passed; no test-profile units were reused from the preceding dev-profile checks |
| `cargo test --workspace --all-targets --locked` | `test` | Warm | 17.1s | 7,058 passed, 0 failed, 18 ignored |

This confirms that the test profile rebuilds its full set of units after the dev-profile Clippy and docs checks. Rustdoc did reuse 30 dev-profile units from Clippy. The test profile remains separately optimized because switching it to `opt-level=0` causes the existing bounded-inheritance test to overflow its stack. These timings are one local sequence, not a hosted CI benchmark.

Reproduce the profile comparison from a clean checkout with fresh target paths. The `cargo-timings/` subdirectory contains Cargo's HTML report; `--message-format=json` also records each compiler artifact and executable path.

```sh
export CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0

CARGO_TARGET_DIR=target/issue-1737/opt1 CARGO_PROFILE_TEST_OPT_LEVEL=1 \
  cargo test --workspace --all-targets --locked --no-run --timings --message-format=json
CARGO_TARGET_DIR=target/issue-1737/opt1 CARGO_PROFILE_TEST_OPT_LEVEL=1 \
  cargo test --workspace --all-targets --locked

CARGO_TARGET_DIR=target/issue-1737/opt0 CARGO_PROFILE_TEST_OPT_LEVEL=0 \
  cargo test --workspace --all-targets --locked --no-run --timings --message-format=json
CARGO_TARGET_DIR=target/issue-1737/opt0 CARGO_PROFILE_TEST_OPT_LEVEL=0 \
  cargo test --workspace --all-targets --locked

# In a separate fresh target directory, measure the production check sequence:
export RUSTDOCFLAGS="-D warnings"
CARGO_TARGET_DIR=target/issue-1737/sequence cargo clippy --workspace --all-targets --locked --timings -- -D warnings
CARGO_TARGET_DIR=target/issue-1737/sequence cargo doc --workspace --no-deps --locked --timings
CARGO_TARGET_DIR=target/issue-1737/sequence cargo test --workspace --all-targets --locked --no-run --timings
CARGO_TARGET_DIR=target/issue-1737/sequence cargo test --workspace --all-targets --locked
```

## Before/after procedure for performance issues

1. Select the exact workflow, job, and command changed by the issue. Keep the same test/feature/target set, lockfile, source revision except for the intended change, JDK, Scala 3.9.0 revision, Rust version, runner image, environment variables, and concurrency settings.
2. Run at least three comparable baseline and three after observations. For compile regressions, collect the timing artifact in full-cache, dependency-only, and cold modes where feasible. Do not mix cache modes or a compiler/toolchain update into one comparison.
3. Record run URL, commit SHA, event, status, runner label, tool versions, cache hit/miss and restored bytes when available, queue delay, job wall time, each relevant step duration, artifact links, and whether the run was cancelled or retried. Keep failures and cancellations visible; do not silently discard them from the run history.
4. For the PR latency question, correlate workflows by PR head SHA and measure from the earliest run creation to the last applicable automatic check completion. Use the latest finishing check, not a sum of parallel job times. Report queue delay separately from execution time.
5. Report the median and range for small samples. Only call a value a tail percentile when the sample is large enough to support that estimate. Label fewer than three comparable observations as insufficient data; label one-off manual audits as observations, not baselines.
6. Preserve all checks and pinned inputs. Use the opt-in timing workflow only for diagnosis; do not add a second full test-target compilation to each required PR run.

## Prioritized hypotheses to test in follow-up issues

1. **Large cache transfer (addressed by #1736):** the former CI Cargo cache was 5.27 GB with a 2m01s median restore. Dependency-only caching reduced the measured benchmark cache to 12.3 MB and cut median benchmark-job wall time by 1m46s. Track cache size and warm/cold behavior as toolchain and lockfile inputs change.
2. **Compilation within the full test step:** one successful CI run reports 4m35s in Cargo's test profile build; the full CI test step has a 6m50s four-run median. The local profile comparison found no safe test-profile change: `opt-level = 0` overflows an existing bounded-inheritance test's stack. The report identifies the largest compilation units and 126 separate integration-test targets; stable Cargo does not isolate linker time from each unit, and no target consolidation was justified by the evidence.
3. **Pinned Scala staging:** Scala source staging has a 5m42s median and up to 7m36s in five consumer samples. Separate sbt staging, cache restore, and Rust consumer tests before changing this flow.
4. **Queueing and gate fan-out:** a measured Scala consumer waited 5m31s before its job began. CI runs two parallel jobs and cancels older runs on the same workflow/ref; the retried CI example took 14m20s end to end, including its failed first JVM-gate attempt and retry interval, while the JVM-gate attempts had 2s and 3s of runner queue. Track queue separately from retries and compiler cost.
5. **Manual compatibility coverage:** lexer, parser, and TASTy workflows have inadequate successful-run samples for cost comparisons. Gather normal opt-in samples only when those workflows are intentionally run; do not turn them into extra PR compilation without an explicit design decision.

The subsequent check-DAG and PR critical-path assessment for [#1740](ci-critical-path-1740.md)
records the measured candidate split, runner-time trade-off, required-check
policy, and reasons the existing job layout was retained.

The final before/after synthesis, all-triggered PR timing snapshots, and next measured target are in the [#1741 performance results](ci-performance-results-1741.md).
