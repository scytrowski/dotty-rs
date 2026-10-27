# Remaining CI workflow cost audit (#1739)

Recorded 2026-10-27 from the workflow definitions, existing hosted run metadata and retained focused-audit artifacts. This is an investigation, not an SLO. Missing or failed samples are called out instead of being used as successful-cost estimates.

## Workflow inventory

| Workflow | Trigger / check role | Main work, inputs and artifacts | Available cost evidence |
| --- | --- | --- | --- |
| [Typer focused audit](../.github/workflows/typer-focused-audit.yml) | Path-filtered PR check and manual dispatch; automatic, but not a ruleset-required check. | Repository and pinned Scala 3.9.0 checkout; JDK 21.0.8 and Rust 1.98.1; audit-tool unit tests; Cargo cache; exact eight pinned Scala runtime downloads with SHA256 checks; test-target build; two sequential focused audits with deterministic report comparison; 30-day report/log/intermediate artifact. `CARGO_BUILD_JOBS` controls compilation; `RUST_TEST_THREADS=1` remains fixed. | Five recent successful PR runs, detailed below. |
| [Typer full-corpus audit](../.github/workflows/typer-full-corpus-audit.yml) | Manual dispatch only. Explicit `run-full-audit` opt-in; default is `do-not-run`. Not a PR check. | Pinned Scala 3.9.0, JDK 21.0.8 and Rust 1.98.1; eight verified runtime artifacts; two sequential normalized full-corpus audits, TASTy-parent reconciliation, and a 30-day report/log artifact. `CARGO_BUILD_JOBS=1`, `RUST_TEST_THREADS=1`, six-hour timeout; no Cargo cache. | No successful run of this workflow was found. No hosted full-corpus run was started for #1739. Full cost is unknown. |
| [Lexer compatibility](../.github/workflows/lexer-compatibility.yml) | Manual dispatch selecting fixtures, source corpus, or both. Not a PR check. | Repository checkout; JDK 25 with sbt dependency cache; sbt launcher; stable Rust; Python normalizer unit test; Scala oracle plus Rust comparison for each selected Scala file. No artifact upload. | Two October 11 runs failed in corpus verification after 41s and 42s. Setup through normalizer took 11–12s after job start; failures are not successful-cost samples. |
| [Scala parser oracle](../.github/workflows/parser-oracle.yml) | Manual dispatch only. Not a PR check. | Repository checkout; JDK 25; Coursier/Ivy/sbt cache; sbt launcher; stable Rust; one batched Scala 3.9 differential oracle run and one Rust parser build/comparison over sorted fixtures. No artifact upload. | No workflow runs found; cost unknown. |
| [TASTy compatibility](../.github/workflows/tasty-compatibility.yml) | Manual dispatch selecting all fixtures, scala3-library, or scala3-compiler. Not a PR check. | Repository checkout; JDK 25 with sbt dependency cache; sbt launcher; stable Rust; structured Rust TASTy round trip and Scala inspector semantic comparisons for each selected corpus. No artifact upload. | One successful run, [35215208912](https://github.com/scytrowski/dotty-rs/actions/runs/35215208912), took 4m49s from job start; corpus verification took 4m26s. Setup through toolchain installation took 16s. This is one sample only. |

The focused and full-corpus workflows preserve pinned Scala/JDK inputs and verify all eight Maven artifact SHA256 values. The manual lexer/parser/TASTy workflows are not converted into default PR checks. The two failed lexer runs are retained as failure evidence, not a performance baseline.

## Focused audit baseline

The five runs below were successful and used the same focused three-file manifest and sequential deterministic audit. Build duration comes from the retained Cargo build log. `Audit command` is the outer `/usr/bin/time -v` measurement for target build plus two test runs and report comparison. Per-pass durations come from the test's `FOCUSED_TIMING` sidecar. RSS is the outer GNU time maximum-resident-set-size field; it is not the per-test RSS sidecar.

| Run | Test-target build | Audit command | First pass | Second pass | Outer max RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| [40402974554](https://github.com/scytrowski/dotty-rs/actions/runs/40402974554) | 1m57s | 3m41s | 50.80s | 50.45s | 745,092 KiB |
| [40407034265](https://github.com/scytrowski/dotty-rs/actions/runs/40407034265) | 1m23s | 2m22s | 28.19s | 29.02s | 763,508 KiB |
| [40409415640](https://github.com/scytrowski/dotty-rs/actions/runs/40409415640) | 1m56s | 3m30s | 47.02s | 45.68s | 764,180 KiB |
| [40410953050](https://github.com/scytrowski/dotty-rs/actions/runs/40410953050) | 1m46s | 2m55s | 33.27s | 33.73s | 766,076 KiB |
| [40415404244](https://github.com/scytrowski/dotty-rs/actions/runs/40415404244) | 2m00s | 3m15s | 36.61s | 36.68s | 760,768 KiB |

Median test-target build was **1m56s**; median audit command was **3m15s**; median combined first/second test runtime was **73.3s**. Median outer max RSS was **763,508 KiB** (about 746 MiB). The complete workflow job median was 3m50s (2m58s–4m15s), including setup, uploads and post-job actions. Rust build-cache restore and save each took a median 2s; the cache entries were 17.4–17.6 MB. The eight pinned Maven artifacts and checksum checks took about 1s in sampled focused jobs. The uploaded compressed artifact was about 120 KB and upload took 1–2s. These cache and download costs are small next to compilation and audit execution, so no cache-key or artifact-verification change is justified.

The step currently builds the test executable and runs both deterministic passes together. The inner timings make test execution visible independently; the added `build.log` GNU-time record separates build wall time and peak RSS from audit execution in subsequent runs.

## Focused build-parallelism experiment

The workflow previously forced one Cargo build job. On this 16-CPU, 30-GB-memory development host, two fresh, non-incremental target directories were used with Rust 1.98.1 and the same `local_definition_audit --no-run` target:

| `CARGO_BUILD_JOBS` | Cold test-target build | Maximum RSS of any child process |
| ---: | ---: | ---: |
| 1 | 70.0s | 744,300 KiB |
| 2 | 38.3s | 803,320 KiB |

The two-job local build was 31.7s faster (45%) and raised the largest individual child RSS by about 8%. That RSS is not the total of concurrent processes, and this host has more CPU and memory than the hosted runner. Therefore the workflow now tries two compile jobs only for the bounded focused audit; it keeps one Rust test thread, the same focused coverage, pinned inputs, checksums, and byte-for-byte two-pass determinism gate. The audit runner defaults to one Cargo job for callers that do not set the environment variable. The build log now includes GNU time output. The hosted PR run is the final resource and correctness check for this candidate; if it fails, restore the one-job setting rather than weakening the gate.

The hosted cold-cache candidate, [PR run 40468729227](https://github.com/scytrowski/dotty-rs/actions/runs/40468729227), passed the complete focused semantic regression gate. The exact Cargo cache lookup missed and was populated after the successful run. Test-target compilation took **64.99s** with an outer maximum RSS of **824,052 KiB**; the two sequential audit passes took **36.293s** and **36.407s** and the normalized reports matched. The outer audit command took **2m18s**, versus the five-run baseline median of 3m15s. The complete job took **2m52s**, versus the baseline median of 3m50s. This is one cold-cache candidate compared with a five-run baseline (whose cache state is mostly unavailable); treat the 58s / 25% lower job time as a promising observation, not a new distribution. On the hosted runner, the two-job cold build was 18s faster than the fastest retained one-job build and 51s faster than its 1m56s median. Its maximum RSS was about 57–77 MiB above the previous sample range, with no swap and no resource failure. The 1-thread audit runtime stayed near the historical median.

No cache changes were made. The existing focused Cargo cache transfers are about 2s and the explicit Scala artifact download/checksum step about 1s. Expanding cache scope or caching verified binaries would add storage and invalidation rules for negligible measured savings.

## Remaining manual workflow recommendations

1. Keep the full-corpus workflow opt-in. Its current contract runs two serialized full audits and a reconciliation with a six-hour timeout, but there is no successful current-workflow duration to report. Do not dispatch it on hosted runners for this issue. If local profiling becomes necessary, use the exact pinned checkout and artifacts, a scratch report/intermediate directory, one Cargo job and one test thread; capture `/usr/bin/time -v`, preserve the environment and logs, and avoid writing to checked-in baseline paths.
2. Keep lexer, parser, and TASTy oracle workflows manual. Their current samples do not establish repeatable cost or safe savings. Preserve JDK/Scala inputs, checksum or semantic comparison gates, corpus choices, and failure behavior.
3. For future measurements, record selected corpus/input, cache state, toolchain versions, job queue and step timestamps. Use multiple successful runs before changing cache policy or reducing oracle coverage.
