# Typer Stdlib Acceptance III closeout (Scala 3.9.0)

## Verdict

**Sprint goal unmet:** the frozen main Tier A cohort is **14/25** at the measured post-A/C comparison baseline and remains **14/25** on final main, below the `>=16/25` target. All 14 methods typed at that comparison baseline remain typed with the same result shapes; no accepted method regressed. The unchanged former holdout moves from **9/25 to 15/25**, and the retrospective third sample moves from **11/25 to 16/25**. The initial pre-feature freeze was not recovered, so these before/after counts do not measure all sprint work.

All three samples retain their original 25 identities, are disjoint, and were audited twice at final main with byte-identical normalized full-method reports. The full 1,236-file corpus was **NOT RUN**.

## Inputs and provenance

| Input | Revision or SHA-256 |
| --- | --- |
| Integrated dotty-rs main | `699b20d4932bb56efa68d9d72b8f2107126bff1e` (verified against `origin/main`) |
| Retrospective comparison baseline | `11ee89ddc7e9e1a672023995f593fa5c831c7213`; this revision already contains workstreams A and C (merges `b8913c652a9db3158f068e48b7db9820a7542191` and `e4d588e74fc1fe54027ce306415fd691c8f919cd`). It is a post-A/C comparison point, not the pre-feature sprint-start revision. |
| Scala source | `777528f19a58e794c9954a42f433373472ec57f8` |
| Scala compiler artifacts | Exact Scala 3.9.0 artifact set in `tests/fixtures/tasty_pickler/provenance.json`; all eight downloaded JARs matched its individual SHA-256 pins. The compiler JAR hash is `3e2c09a5ca6cef17a55f2f66e123815f9c4ea061fce20d0e9d2371989405a61a`. |
| JDK used for these local audits | Eclipse Temurin `21.0.13+9`; `release` SHA-256 `f83f6dcc966043c93cd498ac8e7ee7c310852074970d62c702e4ad2710002898`, `bin/java` SHA-256 `9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`. Historical PR oracles recorded Temurin 21.0.8+9; this closeout used the installed 21.0.13 runtime. |
| Frozen main manifest | [`typer-stdlib-minimal-method-cohort-3.9.0.tsv`](../typer-stdlib-minimal-method-cohort-3.9.0.tsv), SHA-256 `59bb65f48e7d94423de0f0a00029b98a0b41583e601c700687311e9e17fc5244` |
| Frozen former-holdout manifest | [`typer-stdlib-holdout-acceptance-ii-3.9.0.tsv`](../typer-stdlib-holdout-acceptance-ii-3.9.0.tsv), SHA-256 `afa5a2064efd0bfcd31bb0bcb4fb08030db2d180a7619ac3a0843ebbe867f7bc` |
| Static candidate inventory | 9,388 eligible ordinary methods across 649 scanned library files (480 files contain eligible rows); SHA-256 `db0e22b513e2582f8043dba1ce9f4405f7932627d39e3bb3289909cc9eae97c9` |
| Third manifest | [`third-manifest.tsv`](typer-stdlib-acceptance-iii-3.9.0/third-manifest.tsv), SHA-256 `62984ed25a9eb92bafa7e009bd566845011e7f16bdb21fbe8f60ba59d8beeda5` |
| Required original main successes | [`original-main-required-typed.tsv`](typer-stdlib-acceptance-iii-3.9.0/original-main-required-typed.tsv), 14 exact identities and expected result shapes recovered from Acceptance II reconciliation; SHA-256 `a00ba88a4c198c260c3ab811b44ae8aadc903d474d955324a69a226898409263` |

The third cohort uses seed `stdlib-acceptance-iii-static-v1`, A0 static score buckets, SHA-256 tie-breaking over `seed + NUL + path#tree`, and one-method-per-file/owner limits. It excludes both existing manifests. The selector reads only the pinned source inventory, static AST-derived features and seed; it does not consult typing results. However, the sample and its outcomes were produced after the semantic PRs had landed. The preselection timing requirement is therefore **UNMET**, and no blind generalization claim is made. The separate initial-freeze timing requirement is also **UNMET**: the available comparison revision already contains A and C, and no earlier cohort audit artifact was recovered. Workstream receipts below document those earlier changes separately.

## Whole-method outcomes

| Cohort | Post-A/C comparison baseline | Final main | Newly typed after comparison baseline | Retained | Blocked at baseline → final |
| --- | ---: | ---: | ---: | ---: | ---: |
| Main Tier A | 14/25 | 14/25 | 0 | 14 | 11 → 11 |
| Former A0 holdout / analysis cohort | 9/25 | 15/25 | 6 | 9 | 16 → 10 |
| Third A0 sample (retrospective) | 11/25 | 16/25 | 5 | 11 | 14 → 9 |

The gate accepts only the complete Namer-owned `DefDef`: signature, all parameters, full RHS, result conformance, owned typed `DefDef`, and exact `SourceTypedIndex` mappings. The checked-in reconciliation TSVs preserve every `path#tree` identity, owner, result shape, complete before/after diagnostic cause, first blocker family, disposition, and session-local typed tree IDs. Numeric typed tree IDs are recorded for diagnostics only and are not compared as semantic identity.

The sidecars identify the six newly typed former-holdout methods as `scala.App.args` (`App.scala#tree=36`), `scala.collection.mutable.ArrayBuilder.knownSize` (`ArrayBuilder.scala#tree=48`), `scala.Short$.short2float`, `scala.Byte$.byte2double`, `scala.Char$.char2double`, and `scala.collection.mutable.HashMap.unimproveHash`. The five newly typed methods in the retrospective sample are `scala.collection.immutable.Set$.Set4.knownSize` (`Set.scala#tree=1450`), `scala.Short$.short2double`, `scala.Char$.char2float`, `scala.Long$.long2double`, and `scala.math.BigDecimal$.int2bigDecimal` (`BigDecimal.scala#tree=620`). No main-cohort method was newly completed in the post-A/C comparison.

The former holdout's remaining first-blocker families include `MemberLookup` (2), `TermNameNotFound` (2), and `ExpectedExpressionConformanceUnsupported` (2). In the third sample, `MemberLookup` remains the largest family (4). Full nested causes and moved blockers are in the sidecars.

## Workstream receipts (A–E)

| Workstream | Outcome | Reviewed integration evidence and result |
| --- | --- | --- |
| **A — #1807 canonical source/classpath identity** | **DONE** | [PR #1812](https://github.com/scytrowski/dotty-rs/pull/1812), tested head `5467a13b17e44d68c7f17c79cf93c9a28f246436`, merge `b8913c652a9db3158f068e48b7db9820a7542191`. Its deterministic roots remained whole-method blocked: `Option.map` reached `IfBranchJoinUnsupported`, `Either.swap` reached `ExtractorUnapplyNotFound`, and `Tuple.toArray` remained at `ApplicationArgumentConformanceUnsupported`. Canonical identity, rollback/retry and Scala source/classpath negative controls are documented in that PR. |
| **B — #1808 MemberLookup triage** | **DONE** | [PR #1839](https://github.com/scytrowski/dotty-rs/pull/1839), tested head `df2c9d5dc5e1fbaf4cdf5165e05f30cc617d830e`, merge `57db74e0662a8dc2b49274f02df40daf515e569d`. Main remains 14/25. The analysis cohort is 15/25 (six new whole methods); five of eight MemberLookup roots complete. `MapOps` higher-kinded `C` and `Numeric` Double class info remain blocked; `PropImpl` advances to an RHS/Unit mismatch and earns no method credit. |
| **C — #1809 nested case-class shapes** | **DONE** | [PR #1821](https://github.com/scytrowski/dotty-rs/pull/1821), tested head `3bf2af35d346102b246a5f27c8613f074d84c031`, merge `e4d588e74fc1fe54027ce306415fd691c8f919cd`. All 945 legacy local-method rows remain accounted for; 39 Namer-blocked rows reach Typer (9 package-term, 30 object-module), with no whole method newly typed and no change to main 14/25. Scala 3.9 positive and invalid-shape controls are recorded in the PR. |
| **D — #1810 expected Function1 lambda types** | **DONE** | [PR #1842](https://github.com/scytrowski/dotty-rs/pull/1842), tested head `227db87e3051a546ee59c4c886a63954fb38403a`, merge/current-main `699b20d4932bb56efa68d9d72b8f2107126bff1e`. `Function1.compose` and `andThen` advance from missing parameter annotations to `FunctionLiteralCaptureUnsupported`; capture remains explicitly excluded, so neither is typed. Main remains 14/25. Pinned positive and negative Scala controls and Rust tests are listed in the PR. |
| **E — integration and audit** | **DONE; target and initial-freeze criteria UNMET** | This report and three unchanged cohort reconciliations complete the available measurement and evidence work. Main remains below the goal at 14/25. The blind third-sample timing and pre-feature comparison-baseline requirements are unmet: no pre-feature manifest/seal receipt or pre-A/C audit artifact was recovered. The comparison is explicitly limited to the post-A/C baseline and final main; workstreams A and C are supported by their separate receipts. |

Top-level companion PR [#1826](https://github.com/scytrowski/dotty-rs/pull/1826) was an integration dependency, not a fifth semantic workstream here. Its tested head was `d596a4fb72edf968dc86066c0c1f9ab1102fff93`, merge `94bb1c416fb57e020c08aa16d411f3c0365cc891`; it resolved the `Option.map` / `Either.swap` module identity collision but did not complete either whole method.

## Separate local-method and corpus evidence

The local-method denominators are independent regression gates, not ordinary-member acceptance counts: package-term **369 rows**, object-module **576 rows**, and the earlier focused smoke **31 methods / 3 files**. The package-term rerun processed all 369 rows across 60 files with no missing outcomes, parser failures, or Namer failures; **0/369** local methods typed. It recorded 292 reachable selected targets, separately from row accounting. The object-module closeout rerun processed all 576 rows across 77 files, with no missing outcomes, parser failures, or Namer failures; **0/576** local methods typed. The report records `target_observed=0`, which is separate from row accounting and from the reviewed PR #1821 receipt that 30 formerly Namer-blocked rows reach Typer. Namer reachability remains distinct from whole-method typing. The 31-method smoke remains the historical result from Acceptance II. The full 1,236-file corpus audit was **NOT RUN**.

## Reproduction

From a checkout of the recorded source revision with the pinned Scala source and artifact inputs configured as in `tools/typer-classpath-corpus-audit/run`:

```sh
tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --require-typed-cohort docs/typer-stdlib-acceptance-iii-3.9.0/original-main-required-typed.tsv \
  --verify-determinism target/acceptance-iii/main.tsv

tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv \
  --verify-determinism target/acceptance-iii/holdout.tsv

SCALA39_ROOT=/path/to/scala3 \
DOTTY_ORDINARY_CANDIDATE_INVENTORY=target/acceptance-iii/candidates.tsv \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
cargo test -p dotty-typer --test local_definition_audit --locked \
  pinned_scala39_ordinary_member_candidate_inventory -- --ignored --nocapture

python3 tools/typer-classpath-corpus-audit/select_ordinary_holdout.py \
  target/acceptance-iii/candidates.tsv \
  docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  docs/typer-stdlib-acceptance-iii-3.9.0/third-manifest.tsv \
  --exclude-manifest docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv \
  --seed stdlib-acceptance-iii-static-v1

tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-acceptance-iii-3.9.0/third-manifest.tsv \
  --verify-determinism target/acceptance-iii/third.tsv

python3 tools/typer-classpath-corpus-audit/reconcile_acceptance_ii.py \
  --main-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --holdout-manifest docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv \
  --main-before target/acceptance-iii/start-main.tsv \
  --main-after target/acceptance-iii/main.tsv \
  --holdout-before target/acceptance-iii/start-holdout.tsv \
  --holdout-after target/acceptance-iii/holdout.tsv \
  --third-manifest docs/typer-stdlib-acceptance-iii-3.9.0/third-manifest.tsv \
  --third-before target/acceptance-iii/start-third.tsv \
  --third-after target/acceptance-iii/third.tsv \
  --output-dir docs/typer-stdlib-acceptance-iii-3.9.0
```

For the recorded before/after comparison, run the same three audit commands at the post-A/C comparison revision `11ee89ddc7e9e1a672023995f593fa5c831c7213` and at final main, then reconcile with `tools/typer-classpath-corpus-audit/reconcile_acceptance_ii.py` and all three `--third-*` arguments. This reproduces the documented comparison, not the unavailable initial sprint freeze. Exact local normalized raw outputs and audit intermediates were retained under the implementation task's ignored `target/issue1806/evidence/`; only the compact TSV sidecars are checked in.

Closeout tooling validation: `python3 -m unittest discover -s tools/typer-classpath-corpus-audit -p 'test_*.py'` (123 tests), `cargo fmt --all -- --check`, `python3 tools/check-frontend-boundaries.py`, and `git diff --check` passed. Workspace Clippy, rustdoc, full workspace tests, and the JDK backend gate were not rerun for this documentation/Python-only closeout; the merged workstream PR receipts above identify their own tested heads and validation results.

## Evidence-ranked next gaps

1. **Reusable member/class-info resolution in collection and numeric roots.** `MapOps` higher-kinded `C`, `Numeric` Double class information, and four third-sample `MemberLookup` roots remain blocked; preserve canonical owner/identity checks while addressing them.
2. **Whole-body joins and extractors after identity resolution.** `Option.map` is now at `IfBranchJoinUnsupported`; `Either.swap` is at `ExtractorUnapplyNotFound`; `Tuple.toArray` remains at argument conformance. These are distinct blockers and should be measured separately.
3. **Function literal capture support.** `compose` and `andThen` now reach capture analysis. Capture was explicitly excluded from #1810; it remains a follow-up boundary, not an accepted-method regression.
