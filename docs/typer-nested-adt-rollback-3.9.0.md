# Nested recursive ADT rollback audit (Scala 3.9.0)

This audit reuses the private transaction observer and full state projection
from #2079. It adds two nested-specific late-failure controls to
`recursive_adt_rollback.rs`; it does not change typer production semantics.

## Immutable roots and classifications

The unchanged positive fixture is
[`Expr.scala`](../crates/dotty-typer/tests/fixtures/source-nested-recursive-adt-3.9.0/Expr.scala),
SHA-256 `f700d4e27f11365a08291ae15babc65e673eb91a0bdeee45ea5dc2cf7e8c8607`.
Its original roots remain `peelLeft` at bytes `158..266` and `sum` at
`270..468`; the original recursive Apply sites in `sum` are at
`396..406`, `446..455`, and `458..468`. The Scala classification manifest and
the existing source-to-typed receipts continue to describe this fixture.

The bounded source inventory is frozen in `scala-matrix.tsv` and
`negative-cases.tsv`: the pinned runner rejects inner `WrongNestedArity`,
outer `WrongOuterPatternArity`, `WrongNestedLiteralType`, `EscapedCaseBinder`,
`InvalidRecursiveArgument`, `WrongMethodResult`, `LatePeelLeftResult`, and
`LateSumResult`; it accepts `ForeignSameSpelledExtractor` and
`RenamedReorderedFields`. Rust regressions
record extractor arity rejection, nested pattern type rejection, escaped-
binder `TermNameNotFound`, recursive argument/arity/result rejection, and
accepted positional/foreign-owner cases in
`source_binary_recursive_adt_baseline.rs`. Its Rust-only owner and selector
corruptions remain distinct from Scala classifications.

The bounded Rust control IDs pair with the fixture's source-span anchors as
follows:

| `SourceId` | Control | Scala class | Rust result |
| --- | --- | --- | --- |
| 21324 / 21325 / 21326 | `sum` root, source bytes `270..468` | rejected | `ApplicationArgumentTypeMismatch` / `ApplicationArityMismatch` / `OverloadApplicationNoApplicable` |
| 21327 / 21335 / 21347 | nested owner/companion identity corruption | Rust-only | `SymbolSourceKindMismatch`; original retry succeeds where repaired |
| 21328 / 21333 / 21346 | sibling-case binder escape | rejected / Rust-only | `TermNameNotFound` |
| 21332 / 21336 / 21344 | inner `Num` and outer `Add` arity | rejected | `ExtractorProductSelectorCountMismatch` |
| 21334 / 21341 / 21342 | reordered fields and foreign extractor owner | accepted | positional success and distinct owner/binder identities |
| 21345 | foreign nested literal mismatch | rejected | `PatternTypeMismatch` |
| 21321 / 21322 | original `sum` root, source bytes `270..468` | accepted | separately spelled `right` binders resolve to distinct symbols |
| 20770..20777 | Rust-only selector/source-provenance corruptions | not a Scala source case | swapped/aliased selectors and foreign owners reject, then original `selectLeft` recovers |
| 21361 / 21362 | late-failure controls in this audit | rejected | both `ExpectedExpressionTypeMismatch`; see normalized receipt below |

The original `peelLeft` source anchor is `158..266`; its wrong-outer-arity
control extends the root to `158..269`. The original `sum` anchor is
`270..468`. The exact late-failure method, diagnostic, and fresh-session spans
are recorded in the normalized Rust receipt.

## Late-failure controls

The focused Rust control extends the immutable source in memory, preserving
its exact byte prefix. It uses `SourceId(21361)` and a second parse/resolution
session with `SourceId(21362)`. The generated input is 870 bytes. The first
control's root span is `534..650` and its late result diagnostic is `579..650`.
The second control's root span
is `653..869` and its diagnostic is `693..869`.

For `peelLeftFails`, the observer proves both nested `Add` extractor trees and
the inner `left: Expr` binder exist before the `ExpectedExpressionTypeMismatch`
unwinds. For `sumFails`, it proves both nested `Add` patterns, both `Num`
extractor sites (including `Num(value)`), all three recursive `Evaluator.sum`
Apply trees, each case-local argument binder, and the canonical
`Evaluator.sum` method identity before the same late mismatch.
The independently spelled `right` binders remain distinct.

Before each attempt, the test captures the existing complete-state projection:
semantic stores and scope buckets, resolver checkpoint, typed arena and nodes,
source mappings, generated entries, caches, local method indexes, pattern
bindings, and active context frames. It compares this projection after each
failure and retry, checks no control mappings or binders remain, and confirms
the prior successful `prior` root survives. It retries both failures and
requires identical diagnostics. Finally, the untouched original `peelLeft`
and `sum` roots type in the same live session; normalized typed-tree receipts
match a separately parsed and resolved session.

The focused test compares its normalized source IDs, exact control/diagnostic
spans, and diagnostic classes against
[`rust-late-failure.tsv`](../crates/dotty-typer/tests/fixtures/source-nested-recursive-adt-3.9.0/rust-late-failure.tsv).

## Oracle execution status

The fixture's pinned runner was executed with Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`, Eclipse Temurin `21.0.8+9`, and
all eight artifacts verified against `scala-artifacts.sha256.tsv`. It verified
the unchanged positive source and typed-tree hashes, all ten classified
controls, and normalized compiler diagnostic hashes. The two new controls
compile as rejected: `LatePeelLeftResult` reports the final `Expr` arm where
`Boolean` is required; `LateSumResult` reports both Int-valued nested Add
arms where `Boolean` is required. `WrongOuterPatternArity` reports an E107
syntax error because outer `Add` accepts two arguments. `oracle-provenance.tsv`
records source, artifact, output, and control hashes.

Focused Rust validation:

```sh
cargo +1.98.1 test -p dotty-typer \
  nested_late_failures_restore_and_retry_before_original_roots_recover \
  --locked -- --nocapture
```
