# Heterogeneous Product2 source Match gate (Scala 3.9.0)

**Acceptance gate: PASS. Overall PR Definition of Done: UNMET pending independent review and the remaining repository checks.** The pinned Scala/JDK oracle, exact two-session Product2 receipts, and protected-cohort replays passed in [hosted focused audit run 41366399491](https://github.com/scytrowski/dotty-rs/actions/runs/41366399491). The run published its environment, oracle logs, root receipts, negative matrix, and two-pass reports in artifact `typer-focused-audit-31c3ae54f6068387604f010e8710a178cedac0db`.

## Frozen source and tool provenance

- Integrated semantic base: `3a854831b9c074261e31e82d325c0350f606b95a` (PR [#1985](https://github.com/scytrowski/dotty-rs/pull/1985)). The acceptance PR is test, CI, and report only.
- Scala compiler source: 3.9.0, revision `777528f19a58e794c9954a42f433373472ec57f8`.
- JDK: Eclipse Temurin `21.0.8+9`; hosted gate verifies release metadata and hashes `release` and `bin/java` into its environment artifact.
- Rust: `1.98.1` from `rust-toolchain.toml`; hosted gate records `rustc` and Cargo versions.
- Positive source: `crates/dotty-typer/tests/fixtures/source-case-class-product2-3.9.0/Positive.scala`, SHA-256 `4b144913df1d2021359c44633dc2a23f21bf6288bf2528c86cc6f53d79b8ee31`.
- All eight Scala runtime artifact digests are frozen in [`scala-artifacts.sha256.tsv`](../crates/dotty-typer/tests/fixtures/source-case-class-product2-3.9.0/scala-artifacts.sha256.tsv); the workflow downloads and verifies each artifact before running the oracle.

Hosted tool evidence: `rustc 1.98.1 (48a229cea 2026-09-01)`, `cargo 1.98.1 (797e8a9bc 2026-08-05)`, and `openjdk 21.0.8 2025-07-15 LTS`. SHA-256: JDK `release` `93b52921a5efb10ac0dcf9e3c955fa209fc83687df897d6ab034af967e2f5088`; JDK `bin/java` `90d0a6d25da103678013f193db078b90ed8aeb27b66ebe78e0149616c9c1b811`. The hosted environment records the exact test SHA, base SHA, run URL, artifact hashes, and fixture hashes.

The merged prerequisites are [#1958 / PR #1973](https://github.com/scytrowski/dotty-rs/pull/1973), [#1959 / PR #1977](https://github.com/scytrowski/dotty-rs/pull/1977), [#1962 / PR #1978](https://github.com/scytrowski/dotty-rs/pull/1978), [#1965 / PR #1980](https://github.com/scytrowski/dotty-rs/pull/1980), [#1968 / PR #1983](https://github.com/scytrowski/dotty-rs/pull/1983), and [#1970 / PR #1985](https://github.com/scytrowski/dotty-rs/pull/1985).

## Acceptance evidence

The authentic Lexer → Parser → Namer → `SourceTyper::type_method_definition` harness executes `Matcher.select` and `Matcher.classify` in two separately built sessions. Each session creates its own parsed AST, semantic store, Namer index, resolver, typed arenas, source-to-typed mappings, and generated-method index. Assertions require complete owned `DefDef`s, `Int` signatures/results, source spans and mappings, canonical source `Flagged`, its companion and generated `unapply`, authenticated `scala.Product`, class-owned `_1: Int` and `_2: Boolean`, and no third selector. Pattern checks preserve constructor-field order, literal types, binder ownership/scope, and the typed `If` in `select`.

The positive sidecar contains exactly four session/root receipts. Its normalized receipt hashes include source identities, owner paths, types, extractor/selector identity, case shapes, and mapping validity; no typed-arena allocation index participates in cross-session comparison. The source hash and integrated base SHA are repeated per row.

The negative sidecar preserves all ten pinned Scala classifications, including legal warning-only same-name and explicit-unapply cases. The Rust tests cover wrong arity, literal types, result type, ordinary non-case extraction, and rollback after partial case typing. The rollback test repairs the failed source node and successfully retries that root in the same session before typing its valid sibling. The existing focused matrix additionally covers selector mismatch, case-scope escape, explicit collisions, and deferred aliases. Scala-valid-but-Rust-deferred controls remain explicitly labeled.

The cohort sidecar records both deterministic statuses and actual results for all 25 main and 25 analysis roots, with the protected sets and hosted run provenance. The successful run retained Box **2/2**, closures **6/6**, inherited members **4/4**, and protected ordinary methods **16/25 main + 15/25 analysis**, comparing the exact manifests and root sets without editing them.

## Reproduction

Run `.github/workflows/typer-focused-audit.yml` on the acceptance PR. It executes the Product2 Scala oracle using the pinned checkout, Temurin JDK, and verified artifact cache; runs the complete focused Rust, Namer, Typer, and legacy protected-cohort gates; and rejects missing, duplicate, skipped, malformed, or stale receipts. Local Product2 test command:

```sh
cargo +1.98.1 test -p dotty-typer --test source_case_class_match \
  flagged_product2_whole_method_baseline --locked -- --nocapture
```

Hosted focused audit run: [PASS — PR #1987, HEAD `a187e8aa692514d83cce6ce20c330e79c73b1af9`](https://github.com/scytrowski/dotty-rs/actions/runs/41366399491). The complete repository Rust/JVM CI jobs and independent review are tracked on the PR and remain pending at this report revision.
