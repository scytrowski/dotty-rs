# GitHub Actions CI performance baseline

Baseline recorded on 2026-10-27 for issue [#1735](https://github.com/scytrowski/dotty-rs/issues/1735). It describes existing workflow behavior and successful runs observed on 2026-10-26. These observations are diagnostic samples, not regression thresholds.

## Scope and check policy

The seven existing workflows are listed below. `CI`, `JVM TASTy factory Scala consumer`, and `Typer focused audit` start automatically for pull requests, with the latter two restricted by path filters. The other four workflows are opt-in through `workflow_dispatch`. The Typer full-corpus workflow requires an explicit `run-full-audit` input and defaults to `do-not-run`.

The active GitHub ruleset for the default branch currently contains only the `non_fast_forward` rule. It does not define required status checks. Therefore, the automatically triggered pull request workflows below are observed PR checks, but GitHub does not enforce them as required checks under this ruleset. Confirm this policy before describing any status as a required merge gate.

Workflow-level concurrency is part of the latency context. CI cancels an older run for the same workflow/ref; Scala consumer, Typer focused, and Cargo timings allow an existing run to finish; the manual compatibility workflows cancel an older run for the same ref. All job layouts are single-job workflows except CI, which has two independent parallel jobs. None uses a matrix or cross-job `needs` dependency. The full-corpus workflow has one `audit` job, conditionally created only when the dispatch input is `run-full-audit`.

| Workflow | Concurrency group | `cancel-in-progress` | Job shape |
| --- | --- | --- | --- |
| CI | `ci-${{ github.workflow }}-${{ github.ref }}` | `true` | `checks` and `classfile-jvm-encoder`, independent and parallel. |
| JVM TASTy factory Scala consumer | `jvm-tasty-factory-scala-consumer-${{ github.event.pull_request.number || github.ref }}` | `false` | One `consume-factory` job. |
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
| [Typer focused audit](../.github/workflows/typer-focused-audit.yml) | PRs targeting `main` that touch the listed typer/classloader/TASTy/audit paths, or manual dispatch. Automatic path-filtered PR check; not ruleset-required. | Pinned Scala/JDK/Rust setup, verified Scala dependencies, deterministic focused audit. Uploads environment, timings, reports, and intermediates for 30 days. Runs serially with one Cargo build job and one test thread. | Five recent successful PR runs are measured below. |
| [Typer full corpus audit](../.github/workflows/typer-full-corpus-audit.yml) | Manual dispatch only; `run_full_audit` defaults to `do-not-run`; the audit job runs only for `run-full-audit`. Not a PR check. | Pinned Scala/JDK/Rust, dependencies, two sequential full audits and reconciliation; timeout is six hours. Uploads reports, sidecars, inputs, and intermediates for 30 days. No Cargo cache is configured. | No successful run of the current manual workflow was found. Historical workflow records are failed push events from an earlier configuration. No hosted full-corpus audit was launched for this baseline. |
| [Lexer compatibility](../.github/workflows/lexer-compatibility.yml) | Manual dispatch only, selecting fixtures, source corpus, or both. Not a PR check. | JDK 25 and sbt setup, stable Rust install, Scala lexer oracle and selected corpus comparison. The JDK setup caches sbt dependencies. No uploaded report artifact. | Two manual runs are present; both failed. No successful timing sample. |
| [Scala parser oracle](../.github/workflows/parser-oracle.yml) | Manual dispatch only. Not a PR check. | JDK 25, sbt setup, stable Rust install, Scala 3.9 parser differential oracle. Caches Coursier/sbt state. No uploaded report artifact. | No runs found in the repository's recorded workflow history. |
| [TASTy compatibility](../.github/workflows/tasty-compatibility.yml) | Manual dispatch only, selecting all, scala3-library, or scala3-compiler. Not a PR check. | JDK 25 and sbt setup, stable Rust install, structured TASTy round trips for the selected fixture corpus. The JDK setup caches sbt dependencies. No uploaded report artifact. | One successful run, 2026-09-17; it is a one-off observation, not a baseline distribution. |
| [CI Cargo compile timings](../.github/workflows/ci-cargo-timings.yml) | New manual diagnostic workflow, with warm-cache or cold-cache mode. Not a PR check. | Runs the test-target compile with `--no-run --timings`; uploads Cargo HTML timing output and environment metadata for 30 days. | Available for future opt-in measurements; it was not run while establishing this baseline. |

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
| Typer full corpus | Current YAML and associated report-generation path inspected. | Current manual workflow has no successful sample. Do not dispatch it on a hosted runner for this issue. |
| Lexer compatibility | Two manual runs exist: [38140224461](https://github.com/scytrowski/dotty-rs/actions/runs/38140224461) and [38140097123](https://github.com/scytrowski/dotty-rs/actions/runs/38140097123), both failed. | No successful duration; failed runs are not treated as representative cost samples. |
| Parser oracle | No recorded run. | No timing sample. |
| TASTy compatibility | One successful [run 35215208912](https://github.com/scytrowski/dotty-rs/actions/runs/35215208912) on 2026-09-17; observed job elapsed was 4m53s. | One observation only; there is no median or tail estimate. |

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

Across the three warm runs, the full cache had a **7m52s median benchmark-job wall time** (6m30s–8m39s), a **2m01s median restore**, and a **5m43s median compile**. Dependency-only caching had a **6m06s median warm job wall time** (4m31s–6m13s), a **2s median restore**, and a **5m45s median compile**. The compile medians are effectively equal; avoiding the 5.27 GB transfer produced a measured 1m46s (about 22%) lower median end-to-end benchmark-job time. After the experiment, the dependency-only cache entry on its benchmark branch was 12,347,030 bytes, about 427 times smaller than the full archive. The first dependency-only cache miss completed in 4m34s and created that entry. These are three-run samples on hosted runners, not an SLO; the comparison is for the compile job and does not include test execution.

The measured result led to replacing the main Rust-check job's full target cache with dependency-only caching. The JVM gate's archive is already small and toolchain-specific; the Typer cache transfers are small; and Scala consumer cache changes would require a separate end-to-end comparison of its much longer staged compiler path. Preserve those configurations for now and keep their sizes and observed cache outcomes visible for the follow-up issues in the Refactor milestone.

## Before/after procedure for performance issues

1. Select the exact workflow, job, and command changed by the issue. Keep the same test/feature/target set, lockfile, source revision except for the intended change, JDK, Scala 3.9.0 revision, Rust version, runner image, environment variables, and concurrency settings.
2. Run at least three comparable baseline and three after observations. For compile regressions, collect the timing artifact in full-cache, dependency-only, and cold modes where feasible. Do not mix cache modes or a compiler/toolchain update into one comparison.
3. Record run URL, commit SHA, event, status, runner label, tool versions, cache hit/miss and restored bytes when available, queue delay, job wall time, each relevant step duration, artifact links, and whether the run was cancelled or retried. Keep failures and cancellations visible; do not silently discard them from the run history.
4. For the PR latency question, correlate workflows by PR head SHA and measure from the earliest run creation to the last applicable automatic check completion. Use the latest finishing check, not a sum of parallel job times. Report queue delay separately from execution time.
5. Report the median and range for small samples. Only call a value a tail percentile when the sample is large enough to support that estimate. Label fewer than three comparable observations as insufficient data; label one-off manual audits as observations, not baselines.
6. Preserve all checks and pinned inputs. Use the opt-in timing workflow only for diagnosis; do not add a second full test-target compilation to each required PR run.

## Prioritized hypotheses to test in follow-up issues

1. **Large cache transfer (addressed by #1736):** the former CI Cargo cache was 5.27 GB with a 2m01s median restore. Dependency-only caching reduced the measured benchmark cache to 12.3 MB and cut median benchmark-job wall time by 1m46s. Track cache size and warm/cold behavior as toolchain and lockfile inputs change.
2. **Compilation within the full test step:** one successful run reports 4m35s in Cargo's test profile build; the full CI test step has a 6m50s four-run median. Use the uploaded Cargo timing report to locate expensive crates before proposing build-profile or test-graph changes.
3. **Pinned Scala staging:** Scala source staging has a 5m42s median and up to 7m36s in five consumer samples. Separate sbt staging, cache restore, and Rust consumer tests before changing this flow.
4. **Queueing and gate fan-out:** a measured Scala consumer waited 5m31s before its job began. CI runs two parallel jobs and cancels older runs on the same workflow/ref; the retried CI example took 14m20s end to end, including its failed first JVM-gate attempt and retry interval, while the JVM-gate attempts had 2s and 3s of runner queue. Track queue separately from retries and compiler cost.
5. **Manual compatibility coverage:** lexer, parser, and TASTy workflows have inadequate successful-run samples for cost comparisons. Gather normal opt-in samples only when those workflows are intentionally run; do not turn them into extra PR compilation without an explicit design decision.
