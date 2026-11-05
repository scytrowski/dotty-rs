# Nested recursive ADT source-Typer baseline (Scala 3.9.0)

This is a fixture, oracle, and measurement baseline for the exact original
`Evaluator.peelLeft` and `Evaluator.sum` methods. It adds no production compiler
semantics. The immutable source is
[`Expr.scala`](../crates/dotty-typer/tests/fixtures/source-nested-recursive-adt-3.9.0/Expr.scala)
(SHA-256 `f700d4e27f11365a08291ae15babc65e673eb91a0bdeee45ea5dc2cf7e8c8607`).
The source owner tree IDs and byte spans are preserved in the fixture's
[`source-tree-spans.tsv`](../crates/dotty-typer/tests/fixtures/source-nested-recursive-adt-3.9.0/source-tree-spans.tsv)
and mirrored in the docs sidecar
[`typer-nested-recursive-adt-source-shapes-3.9.0.tsv`](typer-nested-recursive-adt-source-shapes-3.9.0.tsv).

## Pinned oracle

The Scala oracle was run against integrated `main` baseline
`29d0bbe0788171dcc723c88ca9f61ea680302a6a`, Scala 3.9.0 source revision
`777528f19a58e794c9954a42f433373472ec57f8`, Eclipse Temurin `21.0.8+9`, and
Rust 1.98.1. The checked-in runner verifies all eight existing Scala runtime
artifact digests and the positive/control source hashes before compilation.
The provenance record is
[`oracle-provenance.tsv`](../crates/dotty-typer/tests/fixtures/source-nested-recursive-adt-3.9.0/oracle-provenance.tsv);
normalized tree and diagnostic hashes are in
[`output-sha256.tsv`](../crates/dotty-typer/tests/fixtures/source-nested-recursive-adt-3.9.0/output-sha256.tsv).

The Scala typed tree observes the generated `Add$.unapply(Add): Add`,
`Num$.unapply(Num): Num`, direct `Expr` parent for `Add`, and distinct
class-owned `Add.left` / `Add.right` fields with corresponding `_1` / `_2`
selectors of type `Expr`. `Num._1` has type `Int`. The exact `peelLeft` root
contains `Add(Add(left, _), _)`; the source binder `left` has type `Expr`. The
`sum` root contains `Num(value)`, `Add(Num(value), right)`, and `Add(left,
right)`. Its three recursive calls target `Evaluator$.sum`, take the distinct
source binders in `right`, `left`, `right` order, and each returns `Int`.

The bounded Scala control matrix has seven cases. Scala rejects a nested
extractor arity mismatch, an incompatible nested `Int` literal, a binder used
outside its case, an invalid recursive argument, and an incompatible method
result. Scala accepts the qualified same-spelled `Foreign.Add` extractor and
the structural field rename/reorder control. Per-case normalized diagnostics
and rationale are recorded in the fixture's
[`scala-matrix.tsv`](../crates/dotty-typer/tests/fixtures/source-nested-recursive-adt-3.9.0/scala-matrix.tsv),
[`negative-cases.tsv`](../crates/dotty-typer/tests/fixtures/source-nested-recursive-adt-3.9.0/negative-cases.tsv),
and diagnostics files. The accepted unrelated-owner control's normalized typed
tree is
[`ForeignSameSpelledExtractor.scala39-typed-tree.txt`](../crates/dotty-typer/tests/fixtures/source-nested-recursive-adt-3.9.0/ForeignSameSpelledExtractor.scala39-typed-tree.txt);
it records the selected `Foreign.Add` companion. These are observed results
from the pinned compiler.

## Rust source-Typer baseline

The focused Rust test runs the real Lexer → Parser → Namer →
`SourceTyper::type_method_definition` path with the existing Scala `Product.tasty`
fixture and JDK resolver in two independently constructed sessions. Both
sessions publish complete whole `DefDef` roots for `peelLeft` (tree 40, bytes
158–266) and `sum` (tree 78, bytes 270–468); the receipt is in
[`rust-two-root-baseline.tsv`](../crates/dotty-typer/tests/fixtures/source-nested-recursive-adt-3.9.0/rust-two-root-baseline.tsv)
and summarized in
[`typer-nested-recursive-adt-first-blockers-3.9.0.tsv`](typer-nested-recursive-adt-first-blockers-3.9.0.tsv).
No first blocker was observed for either original root on this baseline.

The Rust assertions confirm source-to-typed mappings for the full methods,
nested `Add` → `Add` and `Add` → `Num` extractor trees, canonical generated
unapply owners and signatures, the direct canonical `Expr` parent, source-owned
constructor fields and ordered `_1` / `_2` selector types, pattern binder
identity, and all three source `sum` Apply sites with their canonical method,
argument binder, and `Int` result relationships. The test compares the source
IDs and spans across the immutable fixture receipt and checks normalized
outcomes across the fresh sessions.

## Downstream contract and current coverage

This baseline establishes that the following exact shapes currently type:

- nested Product2 → Product2 in `peelLeft`;
- Product2 → Product1 in `sum` through `Add(Num(value), right)`;
- three distinct recursive `sum` calls whose arguments preserve source binder
  identity and whose results have type `Int`;
- whole-root publication and source-to-typed mapping in both fresh sessions.

A future failure path introduced after nested binders or recursive calls have
been created must still roll back store, typed-tree, and source-to-typed state.
This issue's roots complete successfully, so it does not measure a failure
inside this exact nested shape. Existing binary-ADT rollback regressions remain
the observed coverage for failure atomicity. No nested-shape blocker was found;
other behavior beyond the assertions above is not inferred.

## Reproduction

From the repository root, with the pinned Scala checkout, JDK, cached artifacts,
and Rust toolchain available:

```sh
SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8 \
JAVA_HOME=/path/to/temurin-21.0.8+9 \
crates/dotty-typer/tests/fixtures/source-nested-recursive-adt-3.9.0/run

JAVA_HOME=/path/to/temurin-21.0.8+9 \
cargo +1.98.1 test -p dotty-typer \
  --test source_binary_recursive_adt_baseline \
  nested_recursive_adt_original_roots_are_attempted_in_two_fresh_sessions \
  --locked -- --exact --nocapture
```

The Scala runner writes scratch output below `target/source-nested-recursive-adt-oracle`
(or `DOTTY_NESTED_RECURSIVE_ADT_TMP_ROOT`) and checks the normalized typed
snapshot, diagnostics, classifications, source hashes, and eight artifact
hashes. No protected acceptance input or production Namer/Typer/core source is
changed by this baseline.
