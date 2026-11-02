# Sealed Event source acceptance gate

This final gate keeps the original #1992 `Event.scala` fixture and accepts only
`Matcher.extract` and `Matcher.classify` as roots. It records both roots in two
fresh source sessions and verifies complete typed `DefDef`/`Match` trees,
sealed `Event` and `Created`/`Deleted` identities, direct parents, canonical
`Product._1` and generated `unapply` identities, pattern narrowing, ordered
cases, binders, result types, source spans, and source-to-typed mappings.

The hosted `Typer focused audit` workflow runs the pinned Scala 3.9.0 positive
oracle and the #1999 negative/rollback matrix on the exact PR head SHA. Rust
receipt verification rejects missing, duplicate, substituted, or stale
session/root rows and requires the named rollback tests to run without skips.
It also revalidates the existing 14 source-method roots and both frozen 25-root
ordinary cohorts; the required sets remain 16/25 main and 15/25 analysis. The
full Scala corpus stays disabled on hosted CI.

Three TSV evidence sets accompany the run: the committed four-row
[`positive receipt`](typer-sealed-adt-e2e-positive-3.9.0.tsv), the committed
Scala-classified [`negative and rollback matrix`](typer-sealed-adt-e2e-negative-rollback-3.9.0.tsv),
and the two-pass 25+25 `source-case-class-match-cohorts.tsv` emitted into the
workflow artifact. The workflow also uploads
`sealed-adt-e2e-report.md`, which binds PASS or SPRINT GOAL UNMET to the tested
PR head, Actions run URL, pinned Scala/JDK/Rust versions, source hashes, and all
eight Scala artifact hashes. A report is PASS only when the whole focused audit
job succeeds.

## Reproduction

Run the pinned steps with the Scala 3.9.0 checkout at
`777528f19a58e794c9954a42f433373472ec57f8` and Temurin JDK `21.0.8+9`:

```sh
crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/run
crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/run-rollback-matrix
cargo +1.98.1 test -p dotty-typer --test source_case_class_match --locked -- --nocapture
cargo +1.98.1 test -p dotty-typer --lib --locked issue_1999 -- --nocapture
cargo +1.98.1 test -p dotty-core --lib --locked remove_drops_the_name_bucket_after_its_last_symbol -- --nocapture
```

The focused workflow artifact contains the run URL and exact tested SHA. Local
validation on the issue base is supporting evidence; it does not replace the
pinned hosted run.
