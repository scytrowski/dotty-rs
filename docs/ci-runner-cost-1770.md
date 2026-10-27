# CI runner cost and feasibility decision (#1770)

Recorded 2026-10-27. This is a no-go on changing runner labels: available evidence does not establish a faster paid runner's effect on PR completion time, and this repository cannot configure GitHub larger runners under its current personal-account ownership. No paid jobs or billing settings were changed.

## Recommendation

Keep the existing `ubuntu-latest` labels and all CI checks. The issue describes the current public runner as 2-core, but GitHub's current public-repository specification for `ubuntu-latest` is **4 vCPU, 16 GB RAM, and 14 GB SSD**, free and unlimited for public repositories. The repository is owned by a personal GitHub user, not an organization. GitHub larger runners are available only to organizations and enterprises on Team or Enterprise Cloud plans, so a paid 4- or 8-core candidate is not currently available to this repository. The linked-account tooling does not expose billing settings, spending limits, or organization runner-group configuration; those were not inferred.

There is also no like-for-like hosted measurement on a larger runner. The decision is therefore to retain the current free runner, not to conclude that additional CPU would never help. If the repository later moves to an eligible organization plan and an authorized spending limit exists, run the opt-in comparison below before changing labels.

## Current workflow and measured critical path

The active CI workflow has two independent jobs on `ubuntu-latest`: `Rust checks` and `Classfile JVM encoder gate`. The Rust job installs Rust 1.98.1 and runs workflow lint, formatting, frontend-boundary checks, Clippy, rustdoc, all workspace/all-target tests, and diff checking. The JVM job installs stable Rust and JDK 21 and runs the JVM verification suite. Both remain enabled.

| Evidence | Rust job wall | Queue / PR path | Interpretation |
| --- | ---: | ---: | --- |
| Successful main run [40517174944](https://github.com/scytrowski/dotty-rs/actions/runs/40517174944), commit `ab4d52c` | 6m07s | Job started 1m16s after workflow creation; last job finished 7m23s after creation | One recent sample; JVM job was 2m07s. Queue is end-to-end latency, not billed runner execution time. |
| Controlled same-commit cache comparison in [#1741 results](ci-performance-results-1741.md) | Three warm dependency-cache runs: median 6m47s (5m51s–7m16s); three full-cache runs: median 10m38s | Same commit `287880316cebe239e5ea7c754bcb77038e93d6a4`, Rust 1.98.1, runner label and full Rust checks workload | This supports the cache change, not a runner-size comparison. All six runs passed. |

The earlier controlled cache comparison also recorded a 2-second median dependency-cache restore, down from a 1m38s median full-cache restore. A separate local profile on 16 logical CPUs and 29 GiB RAM separated a 97.9-second clean all-target build from 17.3 seconds of test execution, but that machine is not comparable to GitHub-hosted hardware. Hosted CI's `Run tests` step combines compilation/linking and execution; the HTML Cargo timing report does not isolate the final linker phase.

## Resource observability and runner options

GitHub documents the public runner's CPU, RAM, and SSD capacity, but the inspected workflow logs and job API do not expose its CPU quota/throttling, peak RSS, or storage I/O performance. The successful Rust job's `Run tests` step is the longest visible step. Available timings do not establish whether it is CPU-, memory-, or I/O-bound. The active workflow does not publish a Cargo timing artifact; the manual Cargo timings workflow can upload one for 30 days, but it remains a single-run diagnostic unless repeated.

| Option | Availability here | Approximate runner charge | Decision |
| --- | --- | ---: | --- |
| Current public `ubuntu-latest`, 4-core / 16 GB | Available; free and unlimited for this public repository | $0 | Retain. |
| Larger Linux x64, 4-core | Not available under personal-user ownership; larger runners require eligible organization/enterprise plans | $0.012 per billed minute | No demonstrated resource increase over the current 4-core specification; do not adopt. |
| Larger Linux x64, 8-core | Not available under current ownership; no comparable benchmark | $0.022 per billed minute | Defer until access and explicit spending authorization exist, then benchmark. |

GitHub rounds each job's runner use up to a whole minute. At the recent 6m07s Rust-job duration, an unchanged-duration 8-core run would bill 7 minutes: **$0.154 per Rust job, or $15.40 per 100 such runs**. A 4-core larger runner would be **$0.084 per run, or $8.40 per 100** at that duration. These are illustrative costs, not measured candidate results; faster completion could change billed minutes. They exclude other jobs and assume one Rust-job run per counted update. The standard public runner costs $0. If every PR update is counted, multiply by the number of updates that trigger CI.

Public fork pull requests execute untrusted code on `pull_request` workflows. GitHub documents these jobs as read-only and without secrets by default; larger-runner access is separately controlled through runner groups, and GitHub warns that forks can execute dangerous code on a larger runner. No self-hosted runner is proposed. Any future larger-runner trial must preserve the existing read-only fork behavior and must not use privileged events to execute fork code.

## Benchmark feasibility and reproduction

There is no authorized, eligible paid runner class to benchmark today. Accordingly, the required three successful comparable samples per viable paid class cannot be collected, and no paid run was started. Do not interpret local CPU-affinity experiments or the existing 2-core/private-repository specifications as a substitute for a public-hosted runner result.

The repository already has a manual `CI Cargo compile timings` workflow with `full-rust-checks` and cache choices. Once an 8-core larger runner is available and spending is explicitly approved, the smallest useful follow-up is to add an opt-in runner choice to that manual workflow and run the exact same commit serially on the current standard runner and candidate, with the same Rust 1.98.1 toolchain, flags, full Rust checks, and dependency-cache treatment. Verify cache hit state. Collect at least three successful samples per runner class; report cold and warm results separately. Record job start/end, queue delay, test-step wall time, Cargo timing artifact, peak RSS and available disk/I/O metrics, plus the billed rounded minutes. Compare PR critical-path time as the maximum of the parallel CI jobs and any other triggered checks, not just the Rust compile duration. Do not change the normal CI runner label unless the candidate shows a repeatable end-to-end improvement that justifies its cost.

If a trial is later approved, the rollback is immediate: remove the opt-in benchmark label/input and retain the current `ubuntu-latest` labels. No CI change is part of this decision.

## Sources

- Repository metadata: [scytrowski/dotty-rs](https://github.com/scytrowski/dotty-rs) is public and owned by a personal user account.
- [GitHub-hosted runner specifications](https://docs.github.com/en/actions/reference/runners/github-hosted-runners) (public `ubuntu-latest` specifications and free/unlimited public-repository use).
- [Larger runner eligibility](https://docs.github.com/en/actions/concepts/runners/larger-runners) (Team/Enterprise Cloud organization or enterprise requirement).
- [Larger runner pricing](https://docs.github.com/en/enterprise-cloud@latest/billing/reference/actions-runner-pricing) (rates, per-job minute rounding, no included minutes for larger runners, and larger-runner public-repository charges).
- [GitHub Actions runner security](https://docs.github.com/en/actions/concepts/security/compromised-runners) and [larger-runner access control](https://docs.github.com/en/actions/how-tos/manage-runners/larger-runners/control-access) (fork permissions, runner groups, and untrusted fork-code risk).
- Local workflow and measurements: [CI workflow](../.github/workflows/ci.yml), [manual Cargo timing workflow](../.github/workflows/ci-cargo-timings.yml), [#1741 experiment](ci-performance-results-1741.md), and run [40517174944](https://github.com/scytrowski/dotty-rs/actions/runs/40517174944).
