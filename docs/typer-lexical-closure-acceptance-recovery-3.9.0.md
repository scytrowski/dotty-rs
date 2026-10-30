# Lexical closure acceptance recovery (Scala 3.9.0)

## Status

The frozen positive and negative gates pass, and the final 25 + 25 cohort audit
retains every previously typed root. The mandatory GitHub Actions run is
pending PR creation. No Typer semantics were changed for issue #1892.

## Baseline provenance

| Input | Value |
| --- | --- |
| Fetched `main` at baseline | `9042c15be3ad708d80f0ad5d637055f1ac6160b6` |
| Required #1899 merge in history | `22557fe4af779a915b32c697ffa6079cda9f0aca` |
| #1888 / PR #1893 merge | `16968f5735bd680ca8690b01f875f006d9f728bf` |
| #1889 / PR #1895 merge | `5a1e95c194ecc9d705b78c18f4df23b14de3a970` |
| #1890 / PR #1897 merge | `afd1ddeaa0b1260dd39d9a1bd39ec6f69e29a8b6` |
| #1891 / PR #1899 merge | `22557fe4af779a915b32c697ffa6079cda9f0aca` |
| Scala 3.9.0 source revision | `777528f19a58e794c9954a42f433373472ec57f8` |
| JDK | Eclipse Temurin `21.0.8+9` |
| `JAVA_HOME/release` SHA-256 | `f83f6dcc966043c93cd498ac8e7ee7c310852074970d62c702e4ad2710002898` |
| `JAVA_HOME/bin/java` SHA-256 | `9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155` |
| Rust | `rustc 1.98.1 (48a229cea 2026-09-01)` |
| Six-root contract SHA-256 | `937b18923c5720a33fd438d30469e58f1e0bca9111181f48bd5d5ce726f953c6` |
| Negative rollback matrix SHA-256 | `902b32b60b9ade3f0f76fdbb2e29007f58c1651c25f1394ca206dd69a15fbf27` |
| Frozen main manifest SHA-256 | `59bb65f48e7d94423de0f0a00029b98a0b41583e601c700687311e9e17fc5244` |
| Frozen analysis manifest SHA-256 | `afa5a2064efd0bfcd31bb0bcb4fb08030db2d180a7619ac3a0843ebbe867f7bc` |

The two frozen manifests each contain 25 unique roots and are disjoint. Their
contents and denominators were not changed. The independent compose root is
reported separately from six-root acceptance; `andThen#tree=111` remains in
the required main cohort.

The eight Scala compiler/runtime artifacts were verified against
`crates/dotty-typer/tests/fixtures/lexical-closure-contract/scala-artifacts.sha256.tsv`
(manifest SHA-256 `0b69527d4b2d2bb9d6cebba92b481f0a0f2d9aed7b2a8a2b15dbda3233128573`):

| Artifact | SHA-256 |
| --- | --- |
| `scala3-library_3-3.9.0.jar` | `9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba` |
| `scala-library-3.9.0.jar` | `b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd` |
| `scala3-compiler_3-3.9.0.jar` | `3e2c09a5ca6cef17a55f2f66e123815f9c4ea061fce20d0e9d2371989405a61a` |
| `tasty-core_3-3.9.0.jar` | `66e12dda98bb23e864f3a31f8527c8338bf47bce9b85cc380b368c69c1ba8d0f` |
| `scala3-interfaces-3.9.0.jar` | `64dfde1d2c6309d65a01c5f3ea836137eb84338dcadeb16f85764561ef4e10db` |
| `scala-asm-9.9.0-scala-1.jar` | `75ac366e8ecb691e06a7e85041eed0f67919a646e5262fa0901225698c104375` |
| `compiler-interface-1.12.0.jar` | `92f487a505eafdd0a033e4ff4f38a81509a655c76257c64e6808dd873cc145c3` |
| `util-interface-1.11.5.jar` | `1fc950a9e1a35385e79316e23c120d4fd184f6a7af0274e00a703b0dc76376af` |

## Fresh baseline measurements

The pinned Scala lexical closure oracle passed, including
`CombinedCaptures.scala`'s combined enclosing `this` plus `delta` root and its
negative controls. Rust's exact six-root test passed with six complete methods
in two independent fresh sessions. The full lexical closure rollback suite
passed all three tests, including the #1890 negative matrix and valid retry
checks.

Both unchanged ordinary-method cohorts were audited twice with deterministic
comparison at `9042c15be3ad708d80f0ad5d637055f1ac6160b6`:

| Cohort | Roots | Fully typed | Required root protections |
| --- | ---: | ---: | ---: |
| Frozen main | 25 | 16 | 16 |
| Frozen former analysis cohort | 25 | 15 | 15 |

The exact full-typed root identities, source tree numbers, and result shapes
from this fresh baseline are frozen in
[`required-main.tsv`](required-main.tsv) and
[`required-analysis.tsv`](required-analysis.tsv). These typed-only lists do
not alter either cohort manifest or count blocked roots as required successes.
The main list contains `Function1.andThen#tree=111` and
`Function1.compose#tree=86`; compose is separately reported and does not
increase the six-root gate denominator.

| Protection list | SHA-256 |
| --- | --- |
| `required-main.tsv` | `05cdcf0f9185a32075ca8f6bfd3a37084e4127384c0940c9783ae2626dc9377f` |
| `required-analysis.tsv` | `021bb2d5a1e26ea575bf3f89b92c5a5e41acc2e124e753e43715608f0fe4bce4` |

## Integrated acceptance evidence

The pinned Scala oracle, exact six-root Rust test, and complete #1890 rollback
suite passed on the integrated source revision. All six complete `DefDef`s
passed in two independent fresh sessions with canonical capture owners,
result types, and source-to-typed mappings. The pinned combined `this + delta`
source was compiled and its typed tree verified; all positive and negative
Scala controls passed. The Rust negative suite passed its three fixed test
groups, covering all nine frozen cases, rollback, diagnostics, same-session
retry, and fresh-session stability.

The deterministic final 25 + 25 audit produced identical report bytes on both
passes. It retained all 16 main and all 15 analysis typed roots, changed no
blocked root, and introduced no newly typed root. That is zero
accepted-method regressions and no denominator changes. The complete row
outcomes, including owner, source `path#tree`, first blocker, result shape, and
before/after tree IDs, are in [`stdlib-cohorts.tsv`](stdlib-cohorts.tsv).
Final report SHA-256 values equal their corresponding fresh-baseline values:

| Cohort | Baseline → final | Pass 1 / pass 2 report SHA-256 |
| --- | --- | --- |
| Main (25 roots) | 16/25 → 16/25 | `0d96b3b42c3967b6d975ffadee57ea9b157458742f0f8782c73c9156969e4ce4` |
| Analysis (25 roots) | 15/25 → 15/25 | `93952a0614a125eba72543a12c2fd121ddfeaaab89d0f77c2d231ae7c2acf36a` |

The six rows in [`positive-roots.tsv`](positive-roots.tsv) give each exact
root, complete result shape, capture identity list, mapping outcome, and a
SHA-256 receipt for each independent session. The nine rows in
[`negative-rollback.tsv`](negative-rollback.tsv) preserve each frozen Scala
classification, Rust diagnostic, rollback projection, retry/follow-up, and
test group. Raw compiler logs remain in the local worktree and are not
committed.

`Function1.compose#tree=86` was independently fully typed in the current main
cohort; this is attributed to merged #1891, not to the six-root closure gate.
`Function1.andThen#tree=111` remains fully typed and protected.

## CI gate

The PR workflow runs on changes to Typer, acceptance tooling, frozen
contracts, or this report. It fails if the pinned source, JDK, artifacts, six
positive roots, any negative test, exact ordinary-method cohort, or either
deterministic pass is missing, skipped, malformed, or regressed. It uploads
evidence and resource receipts from worktree-local `target/`; it does not run
the full 1,236-file corpus. The mandatory [Typer focused audit run #239](https://github.com/scytrowski/dotty-rs/actions/runs/41113297553)
passed on PR head `0fc7fa82e58bb517a2df0e2b820ac2025616d057` (base
`9042c15be3ad708d80f0ad5d637055f1ac6160b6`). It checked out Scala source
`777528f19a58e794c9954a42f433373472ec57f8`, installed Temurin `21.0.8+9`,
verified the eight JARs against the pinned manifest, passed the exact positive
and negative Rust gates, both deterministic cohorts, and the existing focused
smoke audit. Its [evidence artifact](https://github.com/scytrowski/dotty-rs/actions/runs/41113297553/artifacts/13003988511)
has SHA-256 `2f2f262789a45dfde6a7bfcb2c849eb9d4f8898df196c11658a2bfb126966357`.
The local environment and manifest checksums are recorded above; CI checks the
same frozen manifests, JDK release and binary, and artifact hashes.

Relevant merged prerequisite PRs: [#1893](https://github.com/scytrowski/dotty-rs/pull/1893),
[#1895](https://github.com/scytrowski/dotty-rs/pull/1895),
[#1897](https://github.com/scytrowski/dotty-rs/pull/1897), and
[#1899](https://github.com/scytrowski/dotty-rs/pull/1899).

## Reproduction

With the pinned Scala checkout, Temurin JDK, and eight JARs configured as in
the workflow, run:

```sh
crates/dotty-typer/tests/fixtures/lexical-closure-contract/run
cargo +1.98.1 test -p dotty-typer --lib \
  six_frozen_roots_type_as_complete_methods_in_two_independent_sessions --locked
cargo +1.98.1 test -p dotty-typer --lib lexical_closure_rollback --locked
DOTTY_AUDIT_TYPE_METHOD_DEFINITION=1 \
tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --verify-determinism target/issue-1892/baseline/main.tsv
DOTTY_AUDIT_TYPE_METHOD_DEFINITION=1 \
tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv \
  --verify-determinism target/issue-1892/baseline/analysis.tsv
```

The CI run URL and SHA recorded above identify the mandatory PR run. Reproduce
the frozen local baseline and final cohorts with these commands; scratch paths
are beneath the worktree's ignored `target/issue-1892/` directory.
