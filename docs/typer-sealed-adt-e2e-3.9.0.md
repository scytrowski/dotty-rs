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

Committed TSV evidence includes the four-row
[`positive receipt`](typer-sealed-adt-e2e-positive-3.9.0.tsv), the Scala-classified
[`negative and rollback matrix`](typer-sealed-adt-e2e-negative-rollback-3.9.0.tsv),
the [`25+25 cohort contract`](typer-sealed-adt-e2e-cohorts-3.9.0.tsv), and the
[`50-root two-pass receipt`](typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv).
The workflow also uploads the complete run logs and intermediate outputs,
including every per-case Scala rollback diagnostic, and a `sealed-adt-e2e-report.md`
bound to the tested PR head. A report is PASS only when the whole focused audit
job succeeds.

## Recorded hosted acceptance

**Disposition: PASS.** The [Typer focused audit run 41519895645](https://github.com/scytrowski/dotty-rs/actions/runs/41519895645)
tested PR #2024 at `6c6c236582ba4b7f85241d0df3e381c7eb94a566` against the
integrated base `4d0c9995e8690f708d97bcd615fce8d224267789`. The pinned Scala
oracles, exact Rust acceptance tests, two deterministic 25-root cohorts, and
strict receipt verification passed. Its uploaded rollback directory contains
the frozen `scala-failure-matrix.tsv` and all nine case `.out` diagnostics.

The integrated base is PR #2021's merge for #1999. Its prerequisite merges are:

| Issue | PR | Merge commit |
| --- | --- | --- |
| #1992 | [#2003](https://github.com/scytrowski/dotty-rs/pull/2003) | `908dd53bbd545c8699045fd54f32ec6bb2f60415` |
| #1994 | [#2005](https://github.com/scytrowski/dotty-rs/pull/2005) | `708f30129c3769d02ea5640825f9dee602ae6b53` |
| #1996 | [#2008](https://github.com/scytrowski/dotty-rs/pull/2008) | `fe36f146594e5ae92e2e7758e022a997d4798d79` |
| #1998 | [#2010](https://github.com/scytrowski/dotty-rs/pull/2010) | `4db08165bbb811163a312bc36546cfcfccdb58f7` |
| #1999 | [#2021](https://github.com/scytrowski/dotty-rs/pull/2021) | `4d0c9995e8690f708d97bcd615fce8d224267789` |

The accepted source is `Event.scala` with SHA-256
`a3554b9a764b612af5b6b73779839af82a5e448e909464e45cbc18fb8a86cba1`.
The run used Rust `1.98.1` (`rustc 1.98.1 (48a229cea 2026-09-01)`, Cargo
`1.98.1 (797e8a9bc 2026-08-05)`), Scala 3.9.0 sources at
`777528f19a58e794c9954a42f433373472ec57f8`, and Eclipse Temurin JDK
`21.0.8+9`.

The eight Scala 3.9.0 runtime/compiler artifacts were SHA-256 verified in the
run before use:

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

The 50-row committed cohort receipt records each main and analysis root's
first-pass and second-pass status. The receipt SHA-256 is
`cecfedefae8695f105653462a6a068d3667f2dc2d924ecca757fecbd232c12c5`; the
focused workflow regenerates it from all four ordinary-cohort reports and
rejects changes to root identities, statuses, or results.

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
