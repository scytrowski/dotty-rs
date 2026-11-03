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

The original Rust baseline was observed against authoritative merged `main` revision
[`1ab217d716f47d68ba46915cfdea2fb842fcb173`](https://github.com/scytrowski/dotty-rs/commit/1ab217d716f47d68ba46915cfdea2fb842fcb173)
(merge of PR #2033, the base of this issue PR), using Rust `1.98.1`
(`rustc 1.98.1 (48a229cea 2026-09-01)`, Cargo
`1.98.1 (797e8a9bc 2026-08-05)`) and Temurin JDK `21.0.8+9`. The authentic
resolver's [`Product.tasty`](../crates/dotty-tasty/tests/fixtures/scala3-library/scala/Product.tasty)
fixture has SHA-256
`ad9302597f965b7e218f34b83de00381b64d83487aae80bcc410d67233751e3c`.
These pins identify the baseline independently of later changes to the
checkout or the blockers tracked by #2035 and #2036.

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
the root attempts. Before completion, the probe retrieves each generated
`x$1` using `(unapply, source, class-tree)` in the derived-symbol index. It
asserts that the formal is a synthetic `Parameter`, belongs to that exact
generated unapply, resolves in its method scope, carries the source origin
associated with the class tree, and is distinct from the class, source field,
and other case-class formal. Successful `Num.unapply` input and result
`TypeRef`s target the canonical `Num` symbol; `Num._1` targets canonical
`Int`; and the ordered `Num` parents target canonical `Object`, source `Expr`,
and TASTy-loaded `Product` symbols. The source `Succ.inner` type resolves to
the canonical `Expr` trait, while its completion remains deferred behind the
authentic error. These identities and relationships are frozen in its
[`class evidence`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/rust-class-evidence.tsv).

The original receipt records canonical `Expr`, `Num`, `Succ`, companion, generated
`unapply`, source field, synthetic formal, and `_1` identities. In that
original baseline, `Num` completes with `unapply: (Num) => Num`, `_1: Int`, and
`Object`, `Expr`, and `Product` parents. `Succ` preserves its canonical source
field and direct source `Expr` edge, but generated unapply, selector, and
class-info completion all stop at the same `UnsupportedProduct1FieldType`
error. The separate probe does not alter the two-session baseline.

## Product1 reference projection update (#2036)

The follow-on class/selector increment is recorded in
[`typer-recursive-adt-product1-reference-3.9.0.md`](typer-recursive-adt-product1-reference-3.9.0.md).
The original baseline receipts above remain unchanged. The new after-change
receipts show that `Succ` retains `Object`, the exact source `Expr` edge, and
one authentic classpath `scala.Product` edge; its constructor `Field(inner)`
remains distinct from the class-owned parameterless `_1(): Expr` method. On
the #2036 merged revision, the original whole-method roots reached the
separate generated `Succ.unapply` `MalformedMethodClause` blocker owned by
#2035. That intermediate state remains frozen in the
[`after-Product1 receipts`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/rust-two-root-after-product1.tsv)
and [`Product1 class evidence`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/rust-class-evidence-after-product1.tsv).

## Generated unapply update (#2035)

Issue #2035 completes the bounded `Succ$.unapply(x$1: Succ): Succ` signature
using the same canonical source class, without treating `inner: Expr` as the
method input or result. The unchanged `unwrap` and `eval` roots now type through
the existing Match implementation. Their two-root numerator is unchanged;
the after-signature receipts are [`rust-two-root-after-generated-unapply.tsv`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/rust-two-root-after-generated-unapply.tsv),
[`rust-class-evidence-after-generated-unapply.tsv`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/rust-class-evidence-after-generated-unapply.tsv),
and [`rust-generated-unapply-signature.tsv`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/rust-generated-unapply-signature.tsv).
No Match implementation change is part of #2035. See the
[`generated unapply report`](typer-recursive-adt-generated-unapply-3.9.0.md)
for the narrow signature contract and validation.

The original observations distinguish Scala-valid source from the support
available at the time of the baseline. After #2035 and #2036, the same positive
source is **Rust-valid and typed** for both recorded whole-method roots. The
receipts keep class completion, generated-method completion, and the unchanged
Match path visible as separate checkpoints; they do not claim broader corpus
coverage.

## Reproduction and scope

From the repository root, with the pinned Scala checkout, JDK, and Maven
artifact cache available:

```sh
crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/run
JAVA_HOME=/path/to/temurin-21.0.8+9 cargo +1.98.1 test -p dotty-typer \
  --test source_recursive_adt_baseline --locked -- --nocapture
```

The baseline leaves the pinned oracle and two-root denominator unchanged.
Generated `Succ.unapply` signature completion and the `Product1._1: Expr`/parent
completion are tracked separately by issues
[#2035](https://github.com/scytrowski/dotty-rs/issues/2035) and
[#2036](https://github.com/scytrowski/dotty-rs/issues/2036). The binder and
explicit recursive call are exercised by the unchanged roots after these two
focused prerequisites complete.

## Protected regression contract

The focused audit continues to name and verify the existing acceptance sets:
the checks are run by the existing
[`Typer focused audit workflow`](../.github/workflows/typer-focused-audit.yml).

- **Box/Flagged/Event: 2/2 each** — the frozen positive receipts are
  [`source-case-class-match-positive-3.9.0.tsv`](typer-source-case-class-match-positive-3.9.0.tsv)
  and [`typer-sealed-adt-e2e-positive-3.9.0.tsv`](typer-sealed-adt-e2e-positive-3.9.0.tsv).
  The workflow runs `cargo +1.98.1 test -p dotty-typer --test source_case_class_match`
  and the sealed Event baseline tests.
- **Lexical closures: 6/6** — roots and expected identities are pinned in
  [`six-root-gate.tsv`](../crates/dotty-typer/tests/fixtures/lexical-closure-contract/six-root-gate.tsv);
  the focused workflow runs the exact two-session gate
  `six_frozen_roots_type_as_complete_methods_in_two_independent_sessions` and
  its rollback group.
- **Inherited members: 4/4** — the Rust source receipt is
  [`four-root-baseline.tsv`](../crates/dotty-typer/tests/fixtures/inherited-members-3.9.0/four-root-baseline.tsv),
  and the workflow verifies the exact
  `cargo +1.98.1 test -p dotty-typer --test inherited_members_baseline` log.
- **Ordinary stdlib cohorts: 16/25 main and 15/25 analysis** — the 25+25
  identities and two-pass status are pinned in
  [`typer-sealed-adt-e2e-cohorts-3.9.0.tsv`](typer-sealed-adt-e2e-cohorts-3.9.0.tsv)
  and [`typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv`](typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv).
  The focused audit runs deterministic passes and verifies the exact cohort
  receipts. The full Scala corpus remains disabled.
