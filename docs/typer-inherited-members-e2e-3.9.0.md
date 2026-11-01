# Inherited-member source acceptance closeout (Scala 3.9.0)

This is the acceptance and CI report for [issue #1907](https://github.com/scytrowski/dotty-rs/issues/1907). It adds no production semantic behavior.

## Frozen inputs and provenance

The fresh pre-edit baseline was captured from `main` at `8191715ba7bbc22d307643fb33cea8bf9223a235` (merged [#1924](https://github.com/scytrowski/dotty-rs/pull/1924)). The integrated prerequisites are [#1904 / PR #1911](https://github.com/scytrowski/dotty-rs/pull/1911), [#1905 / PR #1912](https://github.com/scytrowski/dotty-rs/pull/1912), and [#1906 / PR #1924](https://github.com/scytrowski/dotty-rs/pull/1924).

The sole positive denominator is the four source-owned methods in `crates/dotty-typer/tests/fixtures/inherited-members-3.9.0/InheritedMembers.scala` (SHA-256 `f93da74272996204a4c29d07385d3988c811c8084e7fb3352fdb2be00eee26be`). The pinned Scala source checkout is revision `777528f19a58e794c9954a42f433373472ec57f8`; the compiler and libraries are Scala 3.9.0. Oracle provenance requires Eclipse Temurin `21.0.8+9`, Rust `1.98.1`, and Cargo `1.98.1`. The eight exact Maven artifact hashes are in `crates/dotty-typer/tests/fixtures/inherited-members-3.9.0/scala-artifacts.sha256.tsv`; the fixture runner verifies those hashes, the Scala revision, JDK release metadata, source snapshots, and diagnostic files before compiling.

## Positive result

All four fixed `Child` methods are fully typed in two separately constructed source sessions. Each session creates a fresh semantic store, Namer index, type arena, typed arena, and resolver state. The harness checks the source parent edge, original Parent declaration ownership, complete typed method and body mappings, result type, original spans, and capture environment. It compares normalized semantic receipts across sessions rather than session-local numeric IDs. `lifted` and `mapped` each contain one `Child.this` closure environment entry.

The compact receipt is [typer-inherited-members-e2e-3.9.0.tsv](typer-inherited-members-e2e-3.9.0.tsv). The Rust harness also checks the canonical source `SymbolId` and root-local source-to-typed mappings. The receipt retains the baseline source tree indices and spans; the full test recomputes and verifies them from the authentic source.

## Negative and protected results

The pinned Scala runner now covers missing, private, wrong-argument, wrong-method-result, wrong-field-result, ambiguous-import, and nested outer/inherited-name ambiguity controls. It separately compiles valid local/Child shadowing, unrelated same-spelling, and ordinary overloaded/generic parent controls. The Rust suite rejects unsupported overload/generic lookup without publishing a typed root; malformed hierarchy controls stop without lexical fallback. The scoped cases and Rust/Scala classifications are recorded in [typer-inherited-members-negative-receipt-3.9.0.tsv](typer-inherited-members-negative-receipt-3.9.0.tsv). Capture rollback and successful same-session retry remain required through the existing lexical closure rollback gate.

The six-root lexical closure acceptance remains **6/6**. The unchanged ordinary cohorts remain **16/25 main** and **15/25 analysis**; both were run as two deterministic passes before edits and again on the integrated revision. The exact protected root sets are unchanged. Per-root before/after statuses and pass equality are in [typer-inherited-members-cohort-receipt-3.9.0.tsv](typer-inherited-members-cohort-receipt-3.9.0.tsv).

## Reproduction and CI evidence

Run from the repository root, with `SCALA39_ROOT` at the pinned Scala checkout, `JAVA_HOME` at Temurin 21.0.8+9, `SCALA39_JDK_RELEASE=21`, and the Maven cache from the pinned artifact manifest:

```sh
crates/dotty-typer/tests/fixtures/inherited-members-3.9.0/run
cargo +1.98.1 test -p dotty-typer --test inherited_members_baseline --locked -- --nocapture
python3 tools/typer-classpath-corpus-audit/verify_inherited_members_acceptance.py test-log <inherited-members-rust.log>
cargo +1.98.1 test -p dotty-typer --lib --locked -- lexical_closure_contract --nocapture
cargo +1.98.1 test -p dotty-typer --lib --locked lexical_closure_rollback -- --nocapture
```

The required [Typer focused audit workflow](https://github.com/scytrowski/dotty-rs/actions/workflows/typer-focused-audit.yml) now runs the pinned inherited-member oracle and exact Rust matrix. It uploads provenance, positive and negative receipts, cohort passes, and test logs. It fails on absent, skipped, malformed, duplicate, or mismatched evidence. Workflow run URL and reviewed head SHA are recorded with the PR checks.

The first integrated run on `a0a870c334da5bafc14ab3adcd950511133f2e0c` passed [the Typer focused audit](https://github.com/scytrowski/dotty-rs/actions/runs/41257972670) and [general CI](https://github.com/scytrowski/dotty-rs/actions/runs/41257972675). The workflow artifacts record the full SHA as `pull_request_head_sha` alongside exact Scala/JDK/artifact provenance. The report-only follow-up commit adds these links; it does not change the accepted source roots or tests.

Only the bounded four roots, the two unchanged 25-root cohorts, and focused controls run in hosted CI; the full Scala corpus is not run. If a later integrated run does not preserve 4/4 and the protected sets, report `SPRINT GOAL UNMET` and route the separate defect outside this acceptance gate.
