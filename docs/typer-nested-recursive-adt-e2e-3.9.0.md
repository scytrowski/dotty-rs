# Nested recursive ADT E2E release gate (Scala 3.9.0)

The existing `typer-focused-audit.yml` workflow runs the final nested recursive
ADT acceptance against the immutable `Expr.scala` and pinned Scala 3.9.0/JDK
21.0.8+9 toolchain. It requires merge ancestry for all six prerequisite PRs
#2132–#2137 and records the integrated baseline, tested revision, and PR head
as separate values.

The positive denominator is exactly two original complete source roots,
`Evaluator.peelLeft` and `Evaluator.sum`, in each of two independently created
frontend sessions. The strict verifier requires four real receipts with source,
typed-tree, extractor, selector, binder, call, owner, and span mappings. The
Scala oracle runs the original source and its pinned negative matrix. Rust
controls must execute ten source-classified cases and both late failures,
including rollback, deterministic retry, same-session recovery, and fresh
session parity.

The workflow preserves the exact 18 protected source roots and runs the
unchanged 25-root main and 25-root analysis cohorts twice. The cohort sidecar
retains each root, including the exact 16 main and 15 analysis accepted sets.
The full hosted Scala corpus remains disabled.

The single uploaded artifact contains one concise run-bound Markdown report and
three nested acceptance TSV sidecars: four positive receipts, ten classified
cases plus two rollback/recovery receipts, and per-root protected cohort
receipts. Each sidecar is generated from current execution and binds to the
baseline, tested source, and PR-head revisions. The report includes the six
prerequisite merge SHAs, pinned Scala/JDK/source/artifact evidence, hosted run
URL, and hashes for the sidecars. It remains `SPRINT GOAL UNMET` until hosted
acceptance and independent review of that same PR head both succeed.
