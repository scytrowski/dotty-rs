# Issue #1768: main-seeded Scala cache not yet proven

**Decision:** keep the current workflow unchanged and defer main-cache seeding. The exact cache key is compatible by construction with the pinned Scala source, JDK, and build-definition inputs, and GitHub documents that pull requests may restore the default branch cache. Existing run evidence proves only a pull-request-scoped miss followed by a hit on that same PR. No trusted `main` seed followed by a genuinely fresh PR hit was observed, and the cache metadata needed to verify the cache version and ref was not available through the current GitHub connection. The requested latency and storage benefit therefore remains unproven.

## Scope and current workflow

The inspected workflow is [`.github/workflows/jvm-tasty-factory-scala-consumer.yml`](../../.github/workflows/jvm-tasty-factory-scala-consumer.yml) at source revision `5b84f092d3abae3aed365ccfafa0f46b53e48a9d`. It runs on matching PR paths and on manual `workflow_dispatch`. After source staging and Maven artifact verification, a cache miss saves the Scala target directories; the strict Scala/TASTy and Java verifier gates still run afterward on every workflow run. The cache does not replace those checks. It checks out Scala source SHA `777528f19a58e794c9954a42f433373472ec57f8`, verifies JDK 21.0.8, and retains Rust 1.98.1, Maven SHA-256 checks, and all consumer tests.

The exact build-output cache key observed in the workflow logs is:

```text
Linux-X64-scala39-targets-777528f19a58e794c9954a42f433373472ec57f8-jdk21.0.8-f59d197fbb3cf17975bc998b9fe65c368511db193bfb5e87d86b844bf139432b
```

The logged `hashFiles(...)` value is `f59d197fbb3cf17975bc998b9fe65c368511db193bfb5e87d86b844bf139432b`; the workflow hashes `build.sbt`, `project/build.properties`, and the listed `.sbt`/`.scala` build-definition globs. The cache paths are `scala39-source/target` and `scala39-source/**/target`. The key also includes runner OS/architecture, Scala source SHA, and JDK version. There are no restore prefixes for the Scala build-output cache. The separate Rust/Coursier and setup-sbt caches have separate keys and are not evidence of a Scala target-cache hit.

GitHub's [cache access rules](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching#restrictions-for-accessing-a-cache) say that a PR may restore a cache from its base branch, including the default branch. A cache saved by a `pull_request` run is scoped to that PR's merge ref and is reusable only by reruns of the same PR. GitHub lists `workflow_dispatch` among the trusted triggers that can write to the default branch's cache scope. These rules support the proposed safe flow: dispatch this workflow on `main` to seed, then let a new PR restore from `main`. They do not substitute for observing that flow in this repository.

## Existing run evidence

All examined Scala consumer runs were `pull_request` events on branch `codex/issue-1738-scala-consumer`; none was a `workflow_dispatch` or a run on `main`.

| Run | Observed result | What it proves |
| --- | --- | --- |
| [40461901018, attempt 1](https://github.com/scytrowski/dotty-rs/actions/runs/40461901018/attempts/1) | Exact build-output key missed at 06:35:00 UTC and was saved with that key at 06:43:16 UTC. | A successful PR run can populate its own scoped cache. |
| [40461901018, attempt 3](https://github.com/scytrowski/dotty-rs/actions/runs/40461901018/attempts/3) | Exact key restored on a later attempt of the same PR. | Same-PR cache reuse works. |
| [40466722415](https://github.com/scytrowski/dotty-rs/actions/runs/40466722415) | Exact key hit and restored at 07:23:19 UTC; action log reports 643,190,038 bytes (about 613 MiB). The workflow recorded `SCALA_BUILD_CACHE_HIT=true`; strict Scala/TASTy and Java gates passed. | A warm PR-scoped build-output cache can be restored without bypassing the verification gates. It does not identify a main-scoped cache source. |

The existing [#1741 results](../ci-performance-results-1741.md) report a 16m58s first cold Scala consumer run, a 7m58s median over three warm PR-cache runs, and staging medians of 5m42s cold versus 57s warm. Those measurements compare a cold path with repeat runs after the same PR populated its cache. They do not measure a first-time PR receiving a cache seeded on `main`, the main seed's runner cost, or the fresh-PR restore transfer and wall-time savings.

## Evidence gap and no-go

The GitHub connection used for this investigation exposes PR-triggered workflow runs and job logs, but its read-only fetch rejected the Actions cache-inventory endpoint. Its available Actions tools do not provide workflow dispatch. Consequently, the cache entry's `ref`, `version`, compression metadata, and `size_in_bytes` could not be independently read, and a trusted `workflow_dispatch` on `main` could not be started from this environment. The public logs expose the exact key and successful restore/save result, but not the internal cache version. No fresh PR was run against a main-seeded entry.

Do not add a broader restore key, relax a source/JDK assertion, use `pull_request_target`, or add an automatic heavyweight seed job to bridge this evidence gap. Retain the existing exact-key PR cache, which preserves correctness and makes no cross-PR performance claim. The reported approximately 1.01 GB is the prior PR-scoped cache footprint from #1741, not a measured default-branch seed cost.

## Reproducible completion procedure

1. From the Actions UI, manually dispatch **JVM TASTy factory Scala consumer** with ref `main`. Preserve the workflow and verify the exact source SHA, JDK, build-definition hash, exact cache key, successful save, cache `ref=refs/heads/main`, internal cache version, and stored bytes from the run log and cache metadata.
2. Create a genuinely new PR whose base is `main` and whose changed path triggers this workflow. Before any rerun, record the new PR merge ref, exact cache key, restore source/ref, hit result, restored bytes, and restore time. Confirm every strict consumer and Java verifier remains green.
3. Compare this with a cold first-run observation under the same workflow inputs and runner class. Record staging, Maven verification, Rust/Scala tests, Java verifier, total job wall time, and runner minutes. Repeat fresh PR observations where feasible; separate one-off results from medians.
4. Check the exact cache entry's retention/last-access behavior and storage against the PR-scoped footprint. Keep the seed opt-in unless the measured fresh-PR savings outweigh seed and transfer cost.

GitHub documents that cache version is derived from the cache paths and compression tool, and that cache entries are branch-scoped; see the [dependency caching reference](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching) and [cache REST API](https://docs.github.com/en/rest/actions/cache). The version/ref and first-new-PR measurements above remain required evidence before adopting the seed.

No workflow change, hosted full Typer audit, or paid-runner change was made for this no-go.
