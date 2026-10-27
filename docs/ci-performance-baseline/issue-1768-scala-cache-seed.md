# Issue #1768: Scala 3.9 build cache seed evidence

**Result:** the existing opt-in `workflow_dispatch` successfully seeded the exact Scala build-output cache in `main`, and the first run on a new PR restored that entry. Both runs passed the strict Scala/TASTy and Java 21 gates. No workflow change was needed. The measurements show lower staging and total job time for this one cold-main/fresh-PR pair; repeat measurements are needed before treating the difference as a stable median.

## Scope and cache contract

The workflow is [`.github/workflows/jvm-tasty-factory-scala-consumer.yml`](../../.github/workflows/jvm-tasty-factory-scala-consumer.yml). The observed build-output cache key is:

```text
Linux-X64-scala39-targets-777528f19a58e794c9954a42f433373472ec57f8-jdk21.0.8-f59d197fbb3cf17975bc998b9fe65c368511db193bfb5e87d86b844bf139432b
```

The key includes Linux/X64, Scala source SHA `777528f19a58e794c9954a42f433373472ec57f8`, JDK `21.0.8`, and the build-definition hash `f59d197fbb3cf17975bc998b9fe65c368511db193bfb5e87d86b844bf139432b` over `build.sbt`, `project/build.properties`, and the listed `.sbt`/`.scala` build-definition globs. The exact cache paths are `scala39-source/target` and `scala39-source/**/target`; the workflow has no restore prefixes for this cache. Its cache version is `e42ca9e3bdbe787bfa1707e42ee788d81e9d677696d8db7ccfa27110f319bd21`.

The workflow remains pinned to the Scala revision above, Temurin JDK `21.0.8+9`, Rust `1.98.1`, and the existing Maven SHA-256 checks. Every run executes the strict linked Scala/TASTy consumer and mandatory Java 21 verifier/publisher gate after staging, regardless of a cache hit. The cache stores build outputs and does not bypass those checks.

GitHub's [cache access rules](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching#restrictions-for-accessing-a-cache) allow a PR to restore from its base/default branch; PR-created caches are scoped to the PR merge ref. GitHub lists `workflow_dispatch` as a trusted trigger that can write to the default branch cache scope. This repository's run below confirms that flow in practice.

## Main seed and first new-PR restore

On 2026-10-27, the existing workflow was manually dispatched on `main` without changing workflow code. Main was at `c200939821773c542566a25896b1f2e2cb385008`.

| Run | Cache and staging result | Strict gates | Elapsed job time |
| --- | --- | --- | --- |
| [Main seed, run 40500931608](https://github.com/scytrowski/dotty-rs/actions/runs/40500931608) | Exact key missed at 12:05:09 UTC. `dist/universal:stage` completed at 12:09:58; cache save completed at 12:10:42. | Linked Scala/TASTy passed at 12:14:20; Java 21 verifier/publisher passed at 12:16:04. | 11m50s (12:04:37–12:16:27 UTC). |
| [Fresh PR #1779, run 40501882230](https://github.com/scytrowski/dotty-rs/actions/runs/40501882230) | Exact key hit at 12:12:33; 643,180,765 bytes (~613 MiB) restored successfully by 12:12:39. Staging completed at 12:13:56. Save step was skipped because the key was already present. | Linked Scala/TASTy passed at 12:19:03; Java 21 verifier/publisher passed at 12:21:19. | 10m00s (12:11:59–12:21:59 UTC). |

Both runs used the same Ubuntu 26.04 image release (`ubuntu-26.04`, image version `20261012.163.1`), pinned Scala source, JDK, Rust, and cache inputs. In this one pair, staging took 4m49s on the cold main run and 1m17s on the fresh PR run; total consumer-job time was 1m50s lower on the fresh PR. This is an observed sample, not a stable expected saving or a median. Main seeding took 43 seconds from the start of the cache-save step to completion; the fresh PR restored the 613 MiB entry in about six seconds. The main run's entire time includes both mandatory downstream gates, so the staging comparison is the more direct indication of the cache effect.

The new PR had no prior entry for `refs/pull/1779/merge`. Immediately after its restore, the exact-key cache inventory still had no PR 1779 entry; the main entry's `last_accessed_at` advanced to 12:12:33 UTC. This, together with the exact-key hit and same cache version, identifies the source as `refs/heads/main`.

## Cache inventory and reproducible queries

Before the seed, the exact key had seven entries, all scoped to `refs/pull/*/merge`, with the same cache version. Their compressed sizes were 643,260,951 bytes (#1778), 643,228,508 (#1776), 643,198,834 (#1774), 643,269,478 (#1772), 643,203,252 (#1764), 643,144,827 (#1762), and 643,190,038 (#1760). The exact-key query for `ref=refs/heads/main` returned zero entries before dispatch. After dispatch, GitHub recorded the following main entry:

```text
id: 9580115792
ref: refs/heads/main
key: Linux-X64-scala39-targets-777528f19a58e794c9954a42f433373472ec57f8-jdk21.0.8-f59d197fbb3cf17975bc998b9fe65c368511db193bfb5e87d86b844bf139432b
version: e42ca9e3bdbe787bfa1707e42ee788d81e9d677696d8db7ccfa27110f319bd21
size_in_bytes: 643180765
created_at: 2026-10-27T12:10:42.430307Z
last_accessed_at after fresh PR restore: 2026-10-27T12:12:33.396433Z
```

The metadata was queried from the GitHub Actions cache REST API at `https://api.github.com/repos/scytrowski/dotty-rs/actions/caches`, filtering on the exact key and then checking `ref`, `version`, and `size_in_bytes`. For a repeat check with GitHub CLI:

```sh
KEY='Linux-X64-scala39-targets-777528f19a58e794c9954a42f433373472ec57f8-jdk21.0.8-f59d197fbb3cf17975bc998b9fe65c368511db193bfb5e87d86b844bf139432b'
gh api "repos/scytrowski/dotty-rs/actions/caches?key=$KEY&per_page=100" \
  --jq '{total_count, actions_caches: [.actions_caches[] | {id, ref, key, version, size_in_bytes, created_at, last_accessed_at}]}'
```

The action logs expose restore/save timestamps and the restored byte count. The inventory confirms all seven pre-seed entries were PR-scoped, then shows the new exact-version `main` entry. It also confirms the PR 1779 restore updated the main entry's last-access time without saving a PR-scoped duplicate.

## Assessment

The current opt-in workflow dispatch is sufficient to seed the cache safely: exact inputs were preserved, the first genuinely fresh PR restored the main entry, and all verification gates remained green. No broader restore key, `pull_request_target`, automatic heavyweight seed job, or correctness-check relaxation is needed. The main entry costs about 613 MiB of cache storage and its seed requires a cold stage; GitHub's cache retention and eviction behavior should be observed over time.

The existing [#1741 results](../ci-performance-results-1741.md) recorded 16m58s for a cold first Scala consumer run and 7m58s median across three later warm PR-cache runs. Those runs were on a different day and are useful context, not a direct controlled comparison with the main seed above. Repeat fresh-PR observations are still warranted before publishing a stable performance claim or changing the workflow's opt-in policy.
