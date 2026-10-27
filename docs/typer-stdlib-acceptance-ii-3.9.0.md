# Typer Stdlib Acceptance II closeout (Scala 3.9.0)

## Verdict

The frozen 25-method main cohort moved from **10/25 to 14/25 fully typed**. It met the issue's minimum goal (12/25), the aspirational goal (14/25), and the requirement for at least one new Tier A1 method. All ten previously accepted methods remain typed. There are no typed-method regressions.

The independent A0 holdout remained **9/25**. This provides no evidence of holdout uplift, so the result is limited to the frozen main cohort and does not establish broad standard-library coverage. The sprint target was met; the holdout signal is unchanged.

## Scope and exact inputs

- Baseline source revision: `dc71c164aab415dc8a75e698e7ab64fd85891faf` (pre-#1744, with the original 10 accepted methods).
- Final source under test: `034a5105b8ea256f604c74ac44134d55b7206d50` (includes the integrated changes through #1778, the deterministic holdout/gate changes, and the Namer-outcome reporting fix). Current GitHub `main` is `ab4d52c248242290404e70a43098c12e6de2083b` (#1779); its difference from the source checkout is docs-only and does not change the audited code.
- Scala 3.9.0 source: `777528f19a58e794c9954a42f433373472ec57f8`.
- Scala compiler/library artifacts: the pinned Scala 3.9.0 Maven set in `tests/fixtures/tasty_pickler/provenance.json`.
- JDK: Temurin `21.0.8+9`; Rust: `1.98.1` (`rustc 1.98.1 (48a229cea 2026-09-01)`).
- Main manifest SHA-256: `59bb65f48e7d94423de0f0a00029b98a0b41583e601c700687311e9e17fc5244`.
- Candidate inventory: 9,327 ordinary methods from 649 files; two deterministic inventory generations matched SHA-256 `b81a465b9a3179dfee8318137a4e5809e22a8e9182abb00421ed5e9c07fff297`. The inventory recorded one Namer-failed source file; it was not silently removed from the inventory.
- Holdout manifest SHA-256: `afa5a2064efd0bfcd31bb0bcb4fb08030db2d180a7619ac3a0843ebbe867f7bc`. The 25 methods are distinct from the main cohort, cover 25 files and owners, and are distributed evenly across five fixed A0 score buckets. Selection uses static inventory features and a SHA-256 tie-break; it does not inspect typing outcomes.

The main and holdout manifests, per-method comparison TSVs, and holdout selector are checked in beside this report. The original 25-method membership was not edited.

## Main cohort results

| Tier | Before | After | Newly typed |
| --- | ---: | ---: | ---: |
| A0 | 10/18 | 12/18 | 2 |
| A1 | 0/3 | 2/3 | 2 |
| Edge | 0/4 | 0/4 | 0 |
| **Total** | **10/25** | **14/25** | **4** |

Newly complete methods, counted only after the strict whole-method gate accepted their signature, RHS, result conformance, typed-tree ownership, and exact source-to-typed mapping:

| Method | Result type | Contribution |
| --- | --- | --- |
| `library/src/scala/Option.scala#tree=118` — `scala.Option.isEmpty` | `Boolean` | A0, #1744 / PR #1750 |
| `library/src/scala/Option.scala#tree=329` — `scala.Option.nonEmpty` | `Boolean` | A0, #1744 / PR #1750 |
| `library/src/scala/util/Either.scala#tree=19` — `scala.util.Either.left` | `scala.util.Either$.LeftProjection[scala.util.Either.A,scala.util.Either.B]` | A1, #1747 / PR #1778 |
| `library/src/scala/util/Either.scala#tree=31` — `scala.util.Either.right` | `scala.util.Either$.RightProjection[scala.util.Either.A,scala.util.Either.B]` | A1, #1747 / PR #1778 |

Reconciliation: 4 newly typed, 10 retained typed, 10 unchanged blocked, 1 blocker changed while still blocked, and 0 regressed. `Option.map` remains blocked; its first blocker moved from `MemberLookup` to `SymbolResolution`. The one blocker movement is not counted as a typing improvement. `Tuple.toArray` remains blocked at `ApplicationArgumentConformanceUnsupported`; #1745 was closed without implementing it because canonical source/classpath identity resolution fails earlier in that path.

Eight successful rows have different numeric typed `TreeId`s between separate audit sessions. These IDs are arena-local; the gate now compares the canonical source identity, typed root presence, and frozen result shape instead of incorrectly requiring the same allocation number across independent sessions. The frozen manifest is unchanged, and all ten original successful source roots remain present and typed.

## Independent holdout

The sealed A0 holdout result was **9/25 before and 9/25 after**: 9 retained typed, 16 unchanged blocked, no newly typed, no regressions. Both reports have SHA-256 `c1518a992487c544edf6df3764da9b23794892633d6f7c7ace3fdc817fa4b658`; the baseline and final holdout outputs are byte-identical. The holdout was not reselected after observing outcomes.

## Separate focused evidence

These rows are not ordinary Tier A methods and are excluded from the 25-method numerator:

| Focus | Result |
| --- | --- |
| `IterableOnce.scala#tree=2043` and `#tree=2097` local loop methods | Still 0/2 fully typed. Their local roots stop at `ApplicationCalleeNotMethod` before the loop bodies. The nested `IndexedSeq.apply` expression and canonical `Function2.apply` behavior improved in isolation (#1746 / PR #1764), but that does not type either whole local method. |
| `Predef.scala#tree=614` (`SeqCharSequence.charAt`) | Still blocked at `TermNameNotFound` for `sequenceOfChars(index)`; no whole-method credit. |
| Three-file focused smoke | 3 files, 31 local methods, 0 typed, 31 first-blocker outcomes; parser and Namer failures were both 0. |

The strict negative source fixture still fails with the expected Scala E007 mismatch. Positive main-cohort sources and all 25 holdout source files compile with the pinned Scala oracle.

## Historical compatibility gates

The audit runner now records a precise per-method `NamerError::...` outcome when Namer rejects a selected file, rather than omitting all selected methods from that file and panicking on a row-count assertion. The frozen legacy cohorts remain unchanged.

- `package-term`: 60 files, 369/369 selected local methods have an outcome, 0 typed, 0 missing. Nine methods in `compiler/src/dotty/tools/dotc/util/Signatures.scala` now report `NamerError::UnsupportedCaseClassCompanionShape::constructor parameter has a default or special modifier`, with individual source lines. The gate recorded one Namer-failed file; it is not treated as successful typing.
- `object-module`: 77 files, 576/576 selected local methods have an outcome, 0 typed, 0 missing. The three Namer-failed files contribute 30 concrete Namer outcomes: 20 `NamerError::UnsupportedCaseClassCompanionShape::constructor parameter has a default or special modifier` and 10 `NamerError::UnsupportedCaseClassCompanionShape::expected exactly one non-empty ordinary value parameter clause`. The other 546 methods have their current first typing blocker. The legacy cohort remains an independent local-method denominator, not ordinary-method acceptance.

The current PR does not run the hosted 1,236-file corpus audit. Heavy focused audits were run serially with scratch and logs under the task's `target/typer-acceptance-ii/` directory.

## Reproducibility and evidence

The baseline and final ordinary-main audits were each run with `--verify-determinism`. Both baseline and final holdout audits were also run twice and matched exactly. The final strict main audit used `--require-typed-cohort` against the frozen main manifest and passed; the ten original successes were retained. The ordinary cohort outputs and reconciliation sidecars are under [`typer-stdlib-acceptance-ii-3.9.0/`](typer-stdlib-acceptance-ii-3.9.0/).

The corrected focused audit reports are local evidence: package-term SHA-256 `271dad8115101bd36980b02f4a42012dad225cea41ecd001d8426de6bab9d22e`, object-module SHA-256 `631acd4997931103a45d788c5cdbaa070fc81221fb5dd8ec23e5ad184e64f128`. Each reconciler retained all frozen rows and reported 0 missing methods.

Validation on the final source revision: all 122 typer audit Python tests passed; `cargo fmt --all -- --check`, `python3 tools/check-frontend-boundaries.py`, workspace Clippy with `-D warnings`, and workspace rustdoc with `-D warnings` passed. The complete `cargo test --workspace --all-targets --locked` passed after setting `TMPDIR` to a task-local directory under `target/`; its first attempt hit the already-full shared `/tmp` and was rerun successfully without deleting unrelated temporary data. The required JDK 21 gate passed with `DOTTY_CLASSFILE_REQUIRE_JDK=1` and the pinned Temurin 21 toolchain. `git diff --check` passed.

Representative commands (environment paths omitted):

```sh
tools/typer-classpath-corpus-audit/run \
  --method-kind ordinary \
  --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --require-typed-cohort docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --verify-determinism target/typer-acceptance-ii/main.tsv

tools/typer-classpath-corpus-audit/run \
  --method-kind ordinary \
  --method-manifest docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv \
  --verify-determinism target/typer-acceptance-ii/holdout.tsv

tools/typer-classpath-corpus-audit/run \
  --focused-cohort package-term target/typer-acceptance-ii/package-term.tsv
```

Temporary reports, compiler logs, and audit intermediates are local evidence under `target/`; they are not committed. The checked-in TSV sidecars contain only compact per-method results and identities.

## Issue disposition and next targets

- #1744 / PR #1750 completed both Option methods.
- #1745 remains unresolved for `Tuple.toArray`; the issue was closed as not planned, with the canonical identity collision documented as the upstream blocker.
- #1746 / PR #1764 improved isolated indexed application behavior, but did not complete either local `IterableOnce` loop or `charAt`.
- #1747 / PR #1778 completed `Either.left` and `Either.right`.

Next work should target, in order:

1. **Canonical symbol/source identity resolution**: it blocks `Tuple.toArray` before application conformance and remains visible in `Option.map`'s current blocker. This is the strongest shared target, but the Tuple path still needs a focused regression proving the collision is corrected before claiming uplift.
2. **`MemberLookup` for simple standard-library bodies**: it is the most common blocker in the independent holdout (8/16 blocked methods), so a narrowly reusable fix can be measured against the sealed holdout without changing its membership.
3. **Namer case-class companion validation**: historical package and object cohorts still contain methods stopped at `UnsupportedCaseClassCompanionShape`; any support change needs a narrow positive/negative namer test before re-running those gates.

No future cohort expansion is needed to close #1748. The main target is met, the holdout is unchanged, and further compiler work should be proposed as a separate issue with a measurable acceptance gate.
