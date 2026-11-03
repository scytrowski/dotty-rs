# Recursive Expr ADT acceptance (Scala 3.9.0)

**Disposition: PASS.** The focused audit ran on [PR #2056](https://github.com/scytrowski/dotty-rs/pull/2056), tested head `9cc1a29ae13dc7f718e20987a582bae06e538206`, with [hosted run 41618559845](https://github.com/scytrowski/dotty-rs/actions/runs/41618559845). The integrated base was `cfaa8c1160cd190113b867b77d3644357d691a6f`.

The workflow verified the merged prerequisites: #2034 `d62365e3cf2e6b322cceabdbd9e697670eb9e1b5` ([PR #2043](https://github.com/scytrowski/dotty-rs/pull/2043)); #2035 `31ed0e3232069410e76dfa9ccfe2291bebeb97f9` ([PR #2047](https://github.com/scytrowski/dotty-rs/pull/2047)); #2036 `deedd4c8581b31392c38f48060774466f50110c0` ([PR #2044](https://github.com/scytrowski/dotty-rs/pull/2044)); #2037 `5b8a7f0603f89d458fbfb1213d7dd2b94f5204da` ([PR #2048](https://github.com/scytrowski/dotty-rs/pull/2048)); #2038 `e5afcb6fb70c6aeea574d0543df937a6e95c80ef` ([PR #2051](https://github.com/scytrowski/dotty-rs/pull/2051)); and #2039 `60cc3ce645b216b14fe90a31ee5ce975b9e2c2e6` ([PR #2054](https://github.com/scytrowski/dotty-rs/pull/2054)).

## Acceptance evidence

The frozen source SHA-256 is `073be43ad13b7518902a341d3e06b00254d990a325855c70738469c70c8e74ab`. Its Scala 3.9.0 oracle used source revision `777528f19a58e794c9954a42f433373472ec57f8`, Eclipse Temurin `21.0.8+9`, and the eight artifacts in `scala-artifacts.sha256.tsv` (manifest SHA-256 `0b69527d4b2d2bb9d6cebba92b481f0a0f2d9aed7b2a8a2b15dbda3233128573`). The positive typed-tree snapshot, all 15 Scala negative classifications, and diagnostic snapshots matched their frozen evidence.

| Scala artifact | SHA-256 |
| --- | --- |
| `scala3-library_3-3.9.0.jar` | `9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba` |
| `scala-library-3.9.0.jar` | `b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd` |
| `scala3-compiler_3-3.9.0.jar` | `3e2c09a5ca6cef17a55f2f66e123815f9c4ea061fce20d0e9d2371989405a61a` |
| `tasty-core_3-3.9.0.jar` | `66e12dda98bb23e864f3a31f8527c8338bf47bce9b85cc380b368c69c1ba8d0f` |
| `scala3-interfaces-3.9.0.jar` | `64dfde1d2c6309d65a01c5f3ea836137eb84338dcadeb16f85764561ef4e10db` |
| `scala-asm-9.9.0-scala-1.jar` | `75ac366e8ecb691e06a7e85041eed0f67919a646e5262fa0901225698c104375` |
| `compiler-interface-1.12.0.jar` | `92f487a505eafdd0a033e4ff4f38a81509a655c76257c64e6808dd873cc145c3` |
| `util-interface-1.11.5.jar` | `1fc950a9e1a35385e79316e23c120d4fd184f6a7af0274e00a703b0dc76376af` |

| Original root | Before prerequisites | Accepted result |
| --- | --- | --- |
| `Evaluator.unwrap` (`Expr.scala#tree36`, `147:255`) | `UnsupportedProduct1FieldType(Succ.inner)`; no typed root | Complete source-owned `DefDef (Expr): Expr` |
| `Evaluator.eval` (`Expr.scala#tree58`, `259:375`) | `UnsupportedProduct1FieldType(Succ.inner)`; no typed root | Complete source-owned `DefDef (Expr): Int` |

Both roots were typed in each of two independent sessions. [The four-row receipt](typer-recursive-adt-e2e-positive-3.9.0.tsv), SHA-256 `8ae0ac00b55962adc0f21f2fb8b19ef33eff70f0a603e52c1ec10b98ca45a7d5`, records frozen trees/spans, owners, signatures, canonical Expr/Num/Succ parents, Product selectors, generated extractors, ordered case results, recursive callee role and source-to-typed mappings. It compares normalized semantic shapes, not arena-local numeric IDs.

The [negative and rollback receipt](typer-recursive-adt-e2e-negative-rollback-3.9.0.tsv), SHA-256 `4da4d4c7991a99f490dfe48fa90b041e2a49da708838bb0dc3eda810c27efe35`, records all 15 Scala-classified Rust outcomes. The late failure was observed after the reference binder and recursive `Apply`; rollback restored store, resolver, typed arena, source/typed index, scopes and binder frames. Repeated failure remained stable, then both original methods typed in the same session.

Previously accepted source roots remained **16/16**: Box 2/2, Product2 2/2, Event 2/2, lexical closures 6/6, and inherited members 4/4. The unchanged deterministic cohorts retained **16/25 main** and **15/25 analysis** roots across two passes. Their shared per-root receipt is [typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv](typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv), SHA-256 `cecfedefae8695f105653462a6a068d3667f2dc2d924ecca757fecbd232c12c5`.

## Reproduction

The hosted run executed the pinned positive and negative Scala runners, the fresh-session Rust acceptance test, both #2039 rollback tests, focused Namer/Typer regressions, all prior source-root gates, the two ordinary cohorts, and the bounded focused audit. Local checks also passed for the complete `dotty-typer` tests, relevant Namer tests, Clippy, formatting, 173 audit-tool tests, frontend dependency boundaries, and the two deterministic cohorts.

```sh
crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/run
crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/run-negative-matrix
cargo +1.98.1 test -p dotty-typer --test source_recursive_adt_baseline --locked -- --test-threads=1
cargo +1.98.1 test -p dotty-typer --lib --locked recursive_adt -- --test-threads=1
```
