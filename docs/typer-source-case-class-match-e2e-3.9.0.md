# Source case-class Match acceptance (Scala 3.9.0)

This gate is test, report, and CI work for issue #1938. It does not add
compiler semantics. The integrated base is `e7f1941d072b705207f8e3606f50b9592c6ea826`
(merged #1937 / PR #1954); the test fixture `Positive.scala` is unchanged at
SHA-256 `df57333192eb934169f06a07873c2e1ba88c9159404f60e5bfe33b3d016e215c`.
The Scala oracle is pinned to revision
`777528f19a58e794c9954a42f433373472ec57f8`, Scala 3.9.0, and Temurin
21.0.8+9. The eight runtime artifact hashes are in the fixture's
`scala-artifacts.sha256.tsv`.

Before gate edits, the pinned oracle runner passed on the integrated base:
both whole-source typed-tree snapshots and the existing extractor matrix
matched, and the runner verified all eight JAR hashes and source hashes. After
the three scoped invalid controls were added, the same runner was executed
again; it confirmed the snapshots remained unchanged and classified
`WrongPatternType`, `NonCaseMissingExtractor`, and `CaseBinderEscape` as
rejected. The new committed source hashes and Scala classifications are part of
the same fixture manifest and matrix.

The exact Typer test runs both original roots in separate fresh parser, Namer,
semantic-store, resolver, and typed-arena sessions. It requires complete owned
`DefDef`s, a single `(Box): Int` signature, full typed Match and ordered case
trees, source-to-typed mappings, and `Int` result conformance. Each session
authenticates `scala.Product` from TASTy, the unique source-derived `Box._1:
Int` member, the original generated `Box$.unapply` role/owner, and its
independence from `apply`. The `classify` cases remain `0: Int` followed by the
case-local `value: Int` binder. Receipts use canonical paths and roles; they
never compare numeric session-local TreeIds or SymbolIds.

The compact positive receipt is
[`typer-source-case-class-match-positive-3.9.0.tsv`](typer-source-case-class-match-positive-3.9.0.tsv).
The negative receipt is
[`typer-source-case-class-match-negative-3.9.0.tsv`](typer-source-case-class-match-negative-3.9.0.tsv).
It distinguishes Scala-invalid controls from valid Scala forms that this Rust
subset explicitly defers. Failed cases must leave the semantic store, typed
arena, and source mappings unchanged. The rollback test also retries a valid
sibling method in the same session. Exact companion collision/alias tests and
the existing focused Typer extractor regressions run in the pinned workflow.

The 25 main and 25 analysis ordinary-method cohort contracts are recorded in
[`typer-source-case-class-match-cohorts-3.9.0.tsv`](typer-source-case-class-match-cohorts-3.9.0.tsv).
The workflow reruns both cohorts twice, verifies the exact protected sets
(16/25 main and 15/25 analysis), compares deterministic semantic results while
ignoring session-local typed TreeIds, and uploads the resulting 50-row receipt.
It does not run the full Scala corpus.

For each workflow run, `target/typer-focused-report/environment.tsv` records
the tested integrated commit, PR head and base SHAs, Scala revision, JDK and
artifact hashes, and the GitHub Actions run URL. The accompanying
`source-case-class-match-rust.log`, `source-case-class-match-oracle.log`,
`source-case-class-match-cohorts.tsv`, and existing closure/inherited/cohort
reports are the executable evidence for that SHA. The focused audit is required
on changes to core, lexer/parser/namer/typer, fixture, report, verifier, or
workflow paths.
