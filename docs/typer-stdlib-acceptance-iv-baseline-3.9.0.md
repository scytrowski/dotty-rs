# Typer Stdlib Acceptance IV baseline (Scala 3.9.0)

## Result

Measured at protected `main` **89f81940fc538a7b24323b82b9415cdb6a7c5404** using the unchanged ordinary-member manifests. The complete-`DefDef` audit typed **16/25 main** and **15/25 analysis** roots; both repeated normalized reports were byte-identical. The pinned required-typed identity gates passed for all 16 main and 15 analysis roots. Manifest membership and protected-identity files were unchanged.

Both target roots remain whole-method blocked. `Option.map` stops at `IfBranchJoinUnsupported` at its `if` tree, with nested `NonClassLikeSymbol { kind: Package }`; this is the current first blocker, not a historical result. `Tuple.toArray` stops at `ApplicationArgumentConformanceUnsupported` at its call tree, with nested `UnsupportedType`. Neither root advanced to a different outcome or received whole-method credit. See the per-root TSV for source line/range, declared result, method owner, tree identity, and full diagnostic.

## Frozen inputs and source identity

- dotty-rs revision: `89f81940fc538a7b24323b82b9415cdb6a7c5404` (`origin/main`).
- Scala source revision: `777528f19a58e794c9954a42f433373472ec57f8`.
- Source signatures verified directly in that pinned checkout: `Option.scala#tree=214`, `scala.Option.map[B](f: A => B): Option[B]`, line 278, bytes `9281..9374`; `Tuple.scala#tree=23`, `scala.Tuple.toArray: Array[Object]`, line 13, bytes `282..350`.
- The ordinary audit parses and names each selected source file as a `SourceId(0)` source unit, then attempts the original complete `DefDef` through `SourceTyper::type_method_definition`. Thus `scala.Option.map` and `scala.Tuple.toArray` are source-owned roots; they are not reconstructed examples. The method owner/result and original path/tree identity are in the TSV.
- Pinned Scala 3.9.0 Maven artifact SHA-256 values (each checked against `tests/fixtures/tasty_pickler/provenance.json`):

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

  The ordinary audit constructs its classpath only from these jars plus the selected JDK's `jmods`; ambient `CLASSPATH` is ignored.
- JDK: Eclipse Temurin 21.0.13+9, feature 21; `release` SHA-256 `f83f6dcc966043c93cd498ac8e7ee7c310852074970d62c702e4ad2710002898`, `bin/java` SHA-256 `9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`, and `jmods/java.base.jmod` SHA-256 `fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`. This is the installed JDK used for the audit; the fixture-generation runtime recorded in `tests/fixtures/tasty_pickler/provenance.json` was Temurin 21.0.8+9.
- Unchanged main manifest SHA-256: `59bb65f48e7d94423de0f0a00029b98a0b41583e601c700687311e9e17fc5244`; analysis manifest SHA-256: `afa5a2064efd0bfcd31bb0bcb4fb08030db2d180a7619ac3a0843ebbe867f7bc`.
- Protected main identities: [`required-main.tsv`](typer-lexical-closure-acceptance-recovery-3.9.0/required-main.tsv), 16 rows, SHA-256 `05cdcf0f9185a32075ca8f6bfd3a37084e4127384c0940c9783ae2626dc9377f`. Protected analysis identities: [`required-analysis.tsv`](typer-lexical-closure-acceptance-recovery-3.9.0/required-analysis.tsv), 15 rows, SHA-256 `021bb2d5a1e26ea575bf3f89b92c5a5e41acc2e124e753e43715608f0fe4bce4`.

## Causal traces and ownership

The focused test `captures_acceptance_iv_relation_operands_before_transaction_rollback` replays each original complete `DefDef` with the pinned Scala source and classpath. A `#[cfg(test)]` transaction observer receives the actual `TyperError` and inspects the live type arena, typed source mappings, symbol table, and classpath-loaded method before the transaction rolls them back. This is diagnostic-only test code; it does not change production typing. The output distinguishes relation operands from pre-attempt declaration snapshots.

- `Option.map` fails at tree 213 with `IfBranchJoinUnsupported`. At the relation, the actual left operand is `TypeRef(scala.None$)` (`SymbolId(45)`, canonical source module class) and the right operand is `Applied(TypeRef(scala.Some), B)` (`Some` is canonical source class `SymbolId(43)`; `B` is method type parameter `SymbolId(100)`). The live prefix/tycon chain names package symbol `scala` (`SymbolId(39)`), which is exactly the nested `NonClassLikeSymbol { kind: Package }` cause. At the same failure callback it revalidates canonical source method `Option.map` (`SymbolId(73)`, owner `Option`/`SymbolId(42)`) and source-canonical declarations `Option` (`SymbolId(42)`), `Some` (`SymbolId(43)`), and `None` (`SymbolId(44)`). It resolves both `Some` and `None` declared parents by canonical source name to `Option` (`SymbolId(42)`); their parent `ClassInfo` remains `Missing`, so no instantiated parent IDs exist in the failed attempt. The `None` branch type is its source-origin module class (`SymbolId(45)`); the source object identity (`SymbolId(44)`) and its missing companion link are also recorded. No parent ID is inferred from the earlier snapshot.
- `Tuple.toArray` fails at tree 22, argument 0. The live actual operand is `ThisType { class: SymbolId(40) }` for canonical source trait `scala.Tuple`. The resolved callee is canonical member `scala.runtime.Tuples$.toArray` (`SymbolId(209)`, owner `SymbolId` for `Tuples$`), with `SymbolOrigin::Tasty(TastyOriginId(5))`; its loaded method signature is visible in the live store. Its formal operand is `Error(ErrorType { message: ... })`, not a nominal `Tuple` type. The captured message is `unresolved .tasty member type: best-effort name scala/Tuple failed to load: class scala/runtime/Tuples$ failed to load because scala/Tuple failed: cannot establish canonical identity for scala/Tuple: the canonical package name is occupied by an incompatible symbol`. Thus the nested `UnsupportedType` is caused by an unresolved TASTy formal after the resolver rejects the competing `scala.Tuple` identity. At the same failure callback it revalidates canonical source method `Tuple.toArray` (`SymbolId(50)`, owner trait `Tuple`/`SymbolId(40)`). The loaded method owner is TASTy-origin module class `Tuples$` (`SymbolId(205)`, the same `TastyOriginId(5)`). The owner `Tuple.scala` `ClassInfo` is `Missing`; its declared parent syntax is `Product`, but the test cannot establish a canonical external parent identity in this failed attempt. The focused test wraps the configured classpath entries and records the resource returned during this failed attempt: `scala/runtime/Tuples`, format `Tasty`, from `org/scala-lang/scala-library/3.9.0/scala-library-3.9.0.jar`. This directly establishes the selected resolver resource and its archive origin; the repository-relative artifact path is reported without the private workspace prefix.

All printed arena IDs belong to the focused replay session and are diagnostic only. The unchanged 25+25 audit runs independently retain their deterministic normalized reports and exact 16/25 main and 15/25 analysis protected identity gates. The committed TSV records semantic operand shapes, canonical source/member identities, and the captured cause without treating rolled-back numeric IDs as stable identity.

## Reproduction

From protected main `89f81940fc538a7b24323b82b9415cdb6a7c5404`, with the pinned Scala source checkout, JDK 21, and the eight verified artifact jars configured as below. `TMPDIR`, `TMP`, `TEMP`, and `DOTTY_AUDIT_TMP_ROOT` must point to a writable persistent scratch directory.

```sh
export SCALA39_ROOT=/path/to/scala3-at-777528f
export JAVA_HOME=/path/to/temurin-21
export SCALA39_JDK_RELEASE=21
export COURSIER_MAVEN_ROOT=/path/to/pinned-maven-cache
export TMPDIR=/path/to/managed-scratch
export TMP="$TMPDIR" TEMP="$TMPDIR" DOTTY_AUDIT_TMP_ROOT="$TMPDIR/audit"
mkdir -p target/acceptance-iv

# Set SCALA39_CLASSPATH to the colon-separated absolute paths of the eight
# verified Scala 3.9.0 jars listed above before running the focused replay.
cargo test -p dotty-typer --lib \
  captures_acceptance_iv_relation_operands_before_transaction_rollback \
  -- --ignored --nocapture

tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --require-typed-cohort docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-main.tsv \
  --verify-determinism target/acceptance-iv/main.tsv

tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv \
  --require-typed-cohort docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-analysis.tsv \
  --verify-determinism target/acceptance-iv/analysis.tsv
```

The two commands above are the unchanged 25+25 ordinary full-method audits. The committed TSV isolates the two target rows from the first full report. No cohort selection, production compiler code, workflow, or hosted Scala corpus was changed or run.

## Validation

- Main ordinary report: SHA-256 `60cdb029a1661ff55225c9a9ad52bc1c017cefa5354a77ecf4152a9f65d97bfb` on both passes.
- Analysis ordinary report: SHA-256 `aa9c1751d8cd72ec77a0af0801d4f1db9452747b433288ec9c44ad25c40c14eb` on both passes.
- Isolated target report: SHA-256 `c3e614bce4823e4a0fc0c0cb4ae9a06cb0c47d53166629f6a35c4ce0d595d0bd` on both passes.
- `cargo fmt --all -- --check`, `python3 tools/check-frontend-boundaries.py`, `git diff --check`, and all 193 `tools/typer-classpath-corpus-audit` Python tests passed (one existing skipped test).
- The focused pre-rollback relation replay passed for both original complete `DefDef`s. `cargo test -p dotty-typer --all-targets --locked` passed (1,016 unit tests passed, 2 ignored; all integration test binaries passed); the environment-gated replay is run explicitly with `--ignored`.
- `cargo clippy -p dotty-typer --all-targets --locked -- -D warnings` and the frontend boundary guard passed after the review fix.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`, `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked`, and `JAVA_HOME=<Temurin 21.0.13+9> cargo test --workspace --all-targets --locked` passed.
