# Typer standard-library acceptance VI baseline (Scala 3.9.0)

## Result

This evidence-only increment is pinned to integrated main
`353e7de5138976bf5ef0dcc60b044f7fcc3adfc4` (PR #2296 merge) and Scala source
revision `777528f19a58e794c9954a42f433373472ec57f8`. It re-ran the existing
ordinary `DefDef` main and analysis manifests twice each. Each invocation also
used the runner's two-fresh-session determinism check. All four pairs of
normalized reports were byte-identical.

The main cohort is currently **20/25 typed, 5 blocked**; its historical
acceptance-V receipt is 18/25 typed. The only root outcome changes are
`Tuple.toArray` (tree 23), now typed as
`scala.Array[java.lang.Object]`, and `Either.swap` (tree 84), now typed as
`scala.util.Either[scala.util.Either.B,scala.util.Either.A]`. All other
identities retain their historical status and result shape. The analysis
cohort remains **16/25 typed, 9 blocked**, with all historical statuses and
results unchanged. The frozen 16 main and 15 analysis required-typed identities
still match their recorded exact result shapes. Per-root outcomes and the
comparison with the historical receipts are in
[`acceptance-vi/roots.tsv`](typer-stdlib-acceptance-vi/roots.tsv).

Both target methods remain blocked on the current main. A focused test-only
transaction observer captures each original `DefDef` failure before rollback
in its own fresh semantic session and verifies that semantic-store, typed-tree,
index, and source-to-typed mappings are restored afterward:

- `scala.Enumeration.Value.hashCode`, source tree 459, source bytes
  `9430..9466`, has canonical source owner `.scala.Enumeration.Value`. Its
  first diagnostic is `UnstableSelectionPrefix` at tree 458, bytes
  `9461..9466`; the qualifier is the canonical `.scala.Enumeration.Value.id`
  term reference. This error carries no expected-type comparison or nested
  cause.
- `scala.sys.PropImpl.clear`, source tree 99, source bytes `1297..1334`, has
  canonical source owner `.scala.sys.PropImpl`. Its first diagnostic is
  `ExpectedExpressionTypeMismatch` at tree 98, bytes `1317..1334`: actual
  `.scala.collection.mutable.Shrinkable`, expected `Unit`, with no nested
  cause.

The complete observer receipt is in
[`acceptance-vi/target-observations.tsv`](typer-stdlib-acceptance-vi/target-observations.tsv).
Numeric arena and symbol IDs in the test output are session-local and are not
treated as stable identity. Neither target is silently reported as successful.

The pinned Scala 3.9.0 compiler successfully typed both unchanged source files
with `-Ystop-after:typer -Xprint:typer`. The typer tree shows
`hashCode(): Int = this.id.##` and `clear(): Unit`, whose body discards the map
operation's value. The existing positive ordinary-method oracle also passed;
the negative control failed with Scala diagnostic E007 (`Int` found, `String`
required). Results and output hashes are recorded in
[`acceptance-vi/scala-oracles.tsv`](typer-stdlib-acceptance-vi/scala-oracles.tsv).

## Provenance

The two input manifests remain unchanged:

| Input | SHA-256 |
| --- | --- |
| Main 25-root manifest | `59bb65f48e7d94423de0f0a00029b98a0b41583e601c700687311e9e17fc5244` |
| Analysis 25-root manifest | `afa5a2064efd0bfcd31bb0bcb4fb08030db2d180a7619ac3a0843ebbe867f7bc` |
| Historical main audit receipt | `d6f2c0772afe4b0ef8da8d6f6c19961b4c5975e3a8db00840f38cc8c6850fc30` |
| Historical analysis audit receipt | `fe61dc2d328bd9d65c6d6c020fc22dff31f60890387ff3839c115b7eaa2c93d3` |

Current report hashes are `36369b3aad1bd5155912aad4cbf679fe01f33d48cbabea931febe039c9997bbf`
(main, both runs) and `671b3b9e1037d0416b228ab634b2cde4515beecab0926ae945488eb95eb9ed01`
(analysis, both runs). The required identity gates are the unchanged files
`typer-lexical-closure-acceptance-recovery-3.9.0/required-main.tsv` (SHA-256
`05cdcf0f9185a32075ca8f6bfd3a37084e4127384c0940c9783ae2626dc9377f`) and
`required-analysis.tsv` (SHA-256
`021bb2d5a1e26ea575bf3f89b92c5a5e41acc2e124e753e43715608f0fe4bce4`). All
31 required identities matched exactly.

The original 33 unique selected Scala input files, including both target
sources, are pinned by SHA-256 in
[`acceptance-vi/source-inputs.tsv`](typer-stdlib-acceptance-vi/source-inputs.tsv).
The target sources hash to `7370a0d85123fd15c9b75c132a38b911ad71f2a41e851e3121b5a6253939c1e8`
(`Enumeration.scala`) and
`3e9946686782a9af7259a130cf8ee7467cac28a91469c0e3f59c5fc778c6157e`
(`PropImpl.scala`).

The audit used Eclipse Temurin `21.0.13+9` (feature 21). JDK `release`,
`bin/java`, and `jmods/java.base.jmod` SHA-256 values are respectively
`f83f6dcc966043c93cd498ac8e7ee7c310852074970d62c702e4ad2710002898`,
`9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`, and
`fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`.
The audit classpath uses these exact Scala 3.9.0 artifacts:

| Artifact | SHA-256 |
| --- | --- |
| `compiler-interface-1.12.0.jar` | `92f487a505eafdd0a033e4ff4f38a81509a655c76257c64e6808dd873cc145c3` |
| `scala-asm-9.9.0-scala-1.jar` | `75ac366e8ecb691e06a7e85041eed0f67919a646e5262fa0901225698c104375` |
| `scala-library-3.9.0.jar` | `b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd` |
| `scala3-compiler_3-3.9.0.jar` | `3e2c09a5ca6cef17a55f2f66e123815f9c4ea061fce20d0e9d2371989405a61a` |
| `scala3-interfaces-3.9.0.jar` | `64dfde1d2c6309d65a01c5f3ea836137eb84338dcadeb16f85764561ef4e10db` |
| `scala3-library_3-3.9.0.jar` | `9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba` |
| `tasty-core_3-3.9.0.jar` | `66e12dda98bb23e864f3a31f8527c8338bf47bce9b85cc380b368c69c1ba8d0f` |
| `util-interface-1.11.5.jar` | `1fc950a9e1a35385e79316e23c120d4fd184f6a7af0274e00a703b0dc76376af` |

These identities match `tests/fixtures/tasty_pickler/provenance.json`.

## Reproduction

Set `SCALA39_ROOT` to a checkout at the pinned source revision, `JAVA_HOME` to
Temurin 21, and `COURSIER_MAVEN_ROOT` to a cache containing the pinned Scala
3.9.0 artifacts. Put `$JAVA_HOME/bin` on `PATH`. Use a writable persistent
scratch directory for `TMPDIR`, `TMP`, `TEMP`, and `DOTTY_AUDIT_TMP_ROOT`; set
`SCALA39_JDK_RELEASE=21`. Construct `SCALA39_CLASSPATH` from the eight
artifacts listed in the Acceptance VI provenance table.

Run each audit command twice. The `--verify-determinism` flag additionally
runs two fresh audit sessions and rejects differing normalized reports.

```sh
export SCALA39_ROOT=/path/to/scala3-at-777528f
export JAVA_HOME=/path/to/temurin-21
export PATH="$JAVA_HOME/bin:$PATH"
export SCALA39_JDK_RELEASE=21
export COURSIER_MAVEN_ROOT=/path/to/pinned-maven-cache
export TMPDIR=/path/to/persistent-scratch
export TMP="$TMPDIR" TEMP="$TMPDIR"
export DOTTY_AUDIT_TMP_ROOT="$TMPDIR/audit"

# Set SCALA39_CLASSPATH to the colon-separated eight pinned jars.

tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --require-typed-cohort docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-main.tsv \
  --verify-determinism "$TMPDIR/main.tsv"

tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv \
  --require-typed-cohort docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-analysis.tsv \
  --verify-determinism "$TMPDIR/analysis.tsv"

RUST_TEST_THREADS=1 cargo test -p dotty-typer --lib \
  captures_acceptance_vi_hash_code_and_prop_clear_before_rollback \
  --locked -- --ignored --nocapture

mkdir -p "$TMPDIR/oracles"
java -Djava.io.tmpdir="$TMPDIR" -cp "$SCALA39_CLASSPATH" \
  dotty.tools.dotc.Main -classpath "$SCALA39_CLASSPATH" \
  -Yexplicit-nulls -Ystop-after:typer -Xprint:typer -d "$TMPDIR/oracles" \
  "$SCALA39_ROOT/library/src/scala/Enumeration.scala" \
  > "$TMPDIR/oracles/enumeration.log" 2>&1
java -Djava.io.tmpdir="$TMPDIR" -cp "$SCALA39_CLASSPATH" \
  dotty.tools.dotc.Main -classpath "$SCALA39_CLASSPATH" \
  -Yexplicit-nulls -Ystop-after:typer -Xprint:typer -d "$TMPDIR/oracles" \
  "$SCALA39_ROOT/library/src/scala/sys/PropImpl.scala" \
  > "$TMPDIR/oracles/propimpl.log" 2>&1
java -Djava.io.tmpdir="$TMPDIR" -cp "$SCALA39_CLASSPATH" \
  dotty.tools.dotc.Main -classpath "$SCALA39_CLASSPATH" \
  -Yexplicit-nulls -Ystop-after:typer -Xprint:typer -d "$TMPDIR/oracles" \
  tools/typer-classpath-corpus-audit/oracle-controls/OrdinaryPositive.scala
! java -Djava.io.tmpdir="$TMPDIR" -cp "$SCALA39_CLASSPATH" \
  dotty.tools.dotc.Main -classpath "$SCALA39_CLASSPATH" \
  -Yexplicit-nulls -Ystop-after:typer -Xprint:typer -d "$TMPDIR/oracles" \
  tools/typer-classpath-corpus-audit/oracle-controls/OrdinaryNegative.scala
```

The focused observer test was run twice in separate processes. Its failure
callback sees the original attempt before transactional rollback; the test
then checks that the failed transaction restores the store, typed arena,
type index, typed index, and source-to-typed index. No production typing
semantics, manifest selection, source method, or corpus harness was changed.

## Rust validation

The focused crate suite passed with `cargo test -p dotty-typer --all-targets
--locked`; the ignored observer was then run twice explicitly as shown above.
Whole-workspace Clippy, formatting, and frontend dependency-boundary checks
passed with:

```sh
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
python3 tools/check-frontend-boundaries.py
git diff --check
```
