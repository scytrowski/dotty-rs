# Heterogeneous Product2 source Match gate (Scala 3.9.0)

**Status: UNMET — local whole-method typing is 2/2 in two fresh sessions; the pinned Scala oracle, complete cohort rerun, and hosted focused CI result remain required evidence.** The focused audit is fail-closed and publishes its environment, oracle logs, root receipts, negative matrix, and two-pass cohorts as the `typer-focused-audit-<sha>` artifact. This report must be updated with that successful run URL before the issue can be marked complete.

## Frozen source and tool provenance

- Integrated semantic base: `3a854831b9c074261e31e82d325c0350f606b95a` (PR [#1985](https://github.com/scytrowski/dotty-rs/pull/1985)). The acceptance PR is test, CI, and report only.
- Scala compiler source: 3.9.0, revision `777528f19a58e794c9954a42f433373472ec57f8`.
- JDK: Eclipse Temurin `21.0.8+9`; hosted gate verifies release metadata and hashes `release` and `bin/java` into its environment artifact.
- Rust: `1.98.1` from `rust-toolchain.toml`; hosted gate records `rustc` and Cargo versions.
- Positive source: `crates/dotty-typer/tests/fixtures/source-case-class-product2-3.9.0/Positive.scala`, SHA-256 `4b144913df1d2021359c44633dc2a23f21bf6288bf2528c86cc6f53d79b8ee31`.
- All eight Scala runtime artifact digests are frozen in [`scala-artifacts.sha256.tsv`](../crates/dotty-typer/tests/fixtures/source-case-class-product2-3.9.0/scala-artifacts.sha256.tsv); the workflow downloads and verifies each artifact before running the oracle.

The merged prerequisites are [#1958 / PR #1973](https://github.com/scytrowski/dotty-rs/pull/1973), [#1959 / PR #1977](https://github.com/scytrowski/dotty-rs/pull/1977), [#1962 / PR #1978](https://github.com/scytrowski/dotty-rs/pull/1978), [#1965 / PR #1980](https://github.com/scytrowski/dotty-rs/pull/1980), [#1968 / PR #1983](https://github.com/scytrowski/dotty-rs/pull/1983), and [#1970 / PR #1985](https://github.com/scytrowski/dotty-rs/pull/1985).

## Acceptance evidence

The authentic Lexer → Parser → Namer → `SourceTyper::type_method_definition` harness executes `Matcher.select` and `Matcher.classify` in two separately built sessions. Each session creates its own parsed AST, semantic store, Namer index, resolver, typed arenas, source-to-typed mappings, and generated-method index. Assertions require complete owned `DefDef`s, `Int` signatures/results, source spans and mappings, canonical source `Flagged`, its companion and generated `unapply`, authenticated `scala.Product`, class-owned `_1: Int` and `_2: Boolean`, and no third selector. Pattern checks preserve constructor-field order, literal types, binder ownership/scope, and the typed `If` in `select`.

The positive sidecar contains exactly four session/root receipts. Its normalized receipt hashes include source identities, owner paths, types, extractor/selector identity, case shapes, and mapping validity; no typed-arena allocation index participates in cross-session comparison. The source hash and integrated base SHA are repeated per row.

The negative sidecar preserves all ten pinned Scala classifications, including legal warning-only same-name and explicit-unapply cases. The Rust tests cover wrong arity, literal types, result type, ordinary non-case extraction, and rollback after partial case typing. The rollback test repairs the failed source node and successfully retries that root in the same session before typing its valid sibling. The existing focused matrix additionally covers selector mismatch, case-scope escape, explicit collisions, and deferred aliases. Scala-valid-but-Rust-deferred controls remain explicitly labeled.

The cohort sidecar freezes all 25 main and 25 analysis roots and the protected sets. The workflow's two deterministic passes and strict verifier must retain Box **2/2**, closures **6/6**, inherited members **4/4**, and protected ordinary methods **16/25 main + 15/25 analysis**, comparing the exact manifests and root sets without editing them.

## Reproduction

Run `.github/workflows/typer-focused-audit.yml` on the acceptance PR. It executes the Product2 Scala oracle using the pinned checkout, Temurin JDK, and verified artifact cache; runs the complete focused Rust, Namer, Typer, and legacy protected-cohort gates; and rejects missing, duplicate, skipped, malformed, or stale receipts. Local Product2 test command:

```sh
cargo +1.98.1 test -p dotty-typer --test source_case_class_match \
  flagged_product2_whole_method_baseline --locked -- --nocapture
```

Hosted focused audit run: **UNMET — pending PR CI**.
