# Recursive `Expr` ADT typer baseline

This report freezes the Scala 3.9.0 oracle and the Rust source-typer baseline
for exactly two whole-method roots: `Evaluator.unwrap` and `Evaluator.eval`.
It records observations only; it does not claim recursive-reference-valued
pattern support.

## Frozen source and oracle

The positive fixture is
[`Expr.scala`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/Expr.scala),
with SHA-256
`073be43ad13b7518902a341d3e06b00254d990a325855c70738469c70c8e74ab`.
Its committed byte sequence is the issue's immutable source. Source hashes for
it and each bounded control are pinned in
[`source-hashes.tsv`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/source-hashes.tsv).

The reproducible
[`oracle runner`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/run)
requires Scala sources at revision
`777528f19a58e794c9954a42f433373472ec57f8`, Eclipse Temurin JDK
`21.0.8+9`, and exactly the eight artifacts in the adjacent
[`artifact manifest`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/scala-artifacts.sha256.tsv).
It verifies those pins before invoking the official compiler with
`-Vprint:typer -Xprint-types -Yprint-debug -Yprint-debug-owners`, runs each
control, and checks the source, output, diagnostic, and classification hashes.
The positive typed snapshot is committed as
[`Expr.scala39-typed-tree.txt`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/Expr.scala39-typed-tree.txt).

Scala's typed output establishes these generated declarations and identities:

- `Expr` is a source sealed trait; `Num` and `Succ` are distinct source case
  classes, each with the explicit `Expr` parent.
- `Num$.unapply(x$1: Num): Num` and `Succ$.unapply(x$1: Succ): Succ` are owned
  by their distinct generated companion module classes.
- `Num._1` has result `Int`; `Succ._1` has result `Expr`.
- `Num.value` has type `Int`; `Succ.inner` has type `Expr`.
- The original methods have explicit results `Expr` and `Int`. The recursive
  call is `Evaluator.eval(inner)` and returns `Int`.

The separate
[`Scala control matrix`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/scala-matrix.tsv)
was executed with the pinned compiler. `WrongSuccComponent.scala` is accepted
as its own well-typed `String`-component ADT. The mismatched `unwrap` and `eval`
results, unrelated reference class, wrong and missing parent, foreign
same-spelled extractor, both wrong extractor arities, escaped binder, invalid
recursive argument, and inferred-result recursive method are rejected by
Scala. The legal non-exhaustive control is accepted with a warning. Each
compiler diagnostic is frozen alongside the source.

## Rust whole-method baseline

`source_recursive_adt_baseline.rs` attempts both original methods through the
real Lexer, Parser, Namer and `SourceTyper::type_method_definition`. Each of
two new source sessions starts from a fresh semantic store and attaches the
same authentic `scala.Product` TASTy and JDK resolver used by the existing
source acceptance tests. No class or method is precompleted before either
whole-method attempt. The
[`two-row receipt`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/rust-two-root-baseline.tsv)
asserts the original `Expr.scala#tree36` and `Expr.scala#tree58` method roots,
their source spans, `Evaluator$` owner, canonical method/parameter symbols,
both-session determinism, and the error source span.

Both methods first fail at the source `Succ.inner` field declaration,
`Expr.scala#tree16` (bytes `99:112`), with
`UnsupportedProduct1FieldType { class: SymbolId(42), tree_index: 16 }`.
Neither failed method has a source-to-typed root mapping. The error contains no
nested type-relation error, so this baseline records the actual outer error
without manufacturing an inner cause. The methods' match bodies, including
the binder, return conformance, and recursive-call ownership/result, are
therefore unreachable in the whole-method attempts and are not characterized
as working or failing independently.

The test separately locates the original follow-on targets in the parsed
trees and asserts that failed-root rollback left no typed mappings for those
fragments. The return-type trees are `Expr.scala#tree21` (`Expr`, bytes
`171:175`) and `Expr.scala#tree39` (`Int`, bytes `281:284`). The `Succ` binder
trees are `tree30` (bytes `240:245`) and `tree48` (bytes `350:355`). The
`unwrap` arm expression is `tree32` (bytes `250:255`); the recursive call is
`tree52`, `eval(inner)` (bytes `360:371`), with function and argument trees
`tree50` and `tree51`. Namer's `Evaluator$` scope resolves the unshadowed
recursive name to the canonical `Evaluator.eval` symbol. Its typed result is
recorded as deferred because the root never reaches the call. The separate
[`deferred path receipt`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/rust-deferred-paths.tsv)
pins these source locations independently of the two-root numerator.

A separate fresh session characterizes source declarations without changing
the root attempts. Its
[`class evidence`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/rust-class-evidence.tsv)
records canonical `Expr`, `Num`, `Succ`, companion, generated `unapply`, source
field, and `_1` identities. `Num` completes with `unapply: (Num) => Num`,
`_1: Int`, and `Object`, `Expr`, and `Product` parents. `Succ` preserves its
canonical source field and direct source `Expr` edge, but generated unapply,
selector, and class-info completion all stop at the same
`UnsupportedProduct1FieldType` error. The separate probe does not alter the
two-session baseline.

The observations distinguish Scala-valid source from Rust support: the
positive Scala source is **Rust-valid but deferred** at the whole-method
boundary. The class-specific evidence also keeps the later completion
mechanisms separate. It does not test recursive method bodies after bypassing
the genuine first blocker.

## Reproduction and scope

From the repository root, with the pinned Scala checkout, JDK, and Maven
artifact cache available:

```sh
crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/run
JAVA_HOME=/path/to/temurin-21.0.8+9 cargo +1.98.1 test -p dotty-typer \
  --test source_recursive_adt_baseline --locked -- --nocapture
```

The baseline leaves production semantics unchanged and keeps generated
methods and control fixtures outside the two-root denominator. The next
focused source mechanisms are generated `Succ.unapply` result completion and
the `Product1._1: Expr`/parent completion contracts, tracked separately by
issues [#2035](https://github.com/scytrowski/dotty-rs/issues/2035) and
[#2036](https://github.com/scytrowski/dotty-rs/issues/2036). The binder and
explicit recursive call remain later whole-method checkpoints.
