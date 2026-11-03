# Binary recursive ADT source-Typer baseline (Scala 3.9.0)

This fixture freezes the original `Evaluator.selectLeft` and `Evaluator.eval`
methods for a binary recursive case class. It is an oracle and measurement
baseline only. No production semantic code changed.

## Frozen input and pinned environment

The immutable positive source is
[`Expr.scala`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/Expr.scala)
(SHA-256 `1d788b3bd570df83ce2a053c29f4e48145a93815c7dea5880c71d46573dd0b0a`).
Only the two original source-owned methods are whole-`DefDef` numerator roots.
The pinned machine-readable environment and file-hash receipt is
[`oracle-provenance.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/oracle-provenance.tsv). The Scala typed-tree snapshot SHA-256 is
`a95a9cc3c9c71b1af87a305f810404f6b1bc5f95c8914e16d0e1ad49eaea1298`.

| Input | Frozen value |
| --- | --- |
| dotty-rs baseline `main` | `c16a1270b761e25011c221025d51379444e05569` |
| Rust | `rustc 1.98.1 (48a229cea 2026-09-01)`; Cargo 1.98.1 |
| Scala 3.9.0 source | `777528f19a58e794c9954a42f433373472ec57f8` |
| JDK | Eclipse Temurin `21.0.8+9` |
| Runtime artifacts | Eight hashes in [`scala-artifacts.sha256.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/scala-artifacts.sha256.tsv), each verified by the runner |
| Frozen source inventory | [`source-hashes.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/source-hashes.tsv), 17 Scala sources |
| Typed tree and diagnostics | [`output-sha256.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/output-sha256.tsv) |

The Scala compiler output confirms that `Expr` is sealed; `Num` and `Add` are
case classes; and `Add` extends `Expr`, `Product`, then `Serializable`. The
original `Add.left` and `Add.right` are distinct class-owned fields, in that
order, and both have type `Expr`. Scala generates distinct class-owned
`Add._1: Expr` and `Add._2: Expr` selectors. The companion owns
`Add$.unapply(Add): Add`; it is not modeled as an `Option` or tuple. Both
recursive calls in the original `eval` body resolve to `Evaluator$.eval` with
result type `Int`, and their argument trees are the separate `left` and
`right` binders.

## Scala controls

The runner compiles every checked-in control with the pinned compiler and
compares normalized diagnostics, expected status, source hashes, and output
hashes. The full classification/rationale inventory is in
[`negative-cases.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/negative-cases.tsv).
Scala rejects `Add` patterns of arity 0, 1, or 3; an `Int` nested literal or
`Int`-typed nested pattern where `Expr` is required; an incompatible method
result; a missing or wrong parent; an escaped case binder; and an invalid
recursive argument. Scala accepts a qualified unrelated `Foreign.Add`
extractor, an explicit companion `unapply`, an explicit `_1` selector, renamed
constructor fields, and swapped constructor field order. The intentionally
non-exhaustive match is accepted with a warning and is not classified as a
rejection.

## Rust source-Typer first errors

The baseline test executes Lexer → Parser → Namer →
`SourceTyper::type_method_definition` using the Scala `Product.tasty` fixture
and the actual JDK classpath resolver. It constructs two independent source
sessions and attempts both original whole methods in each one. The test checks
source ownership, parameter ownership, the canonical `Evaluator$` owner,
absence of partial whole-root publication, and identical normalized outcomes.
The two-row first-blocker view is
[`typer-binary-recursive-adt-first-blockers-3.9.0.tsv`](typer-binary-recursive-adt-first-blockers-3.9.0.tsv);
the full per-session receipt, including session-local IDs, is
[`rust-two-root-baseline.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/rust-two-root-baseline.tsv).

Both sessions stop at `ExtractorUnapplyNotFound` on the second (`Add`) pattern.
`selectLeft` reaches source tree 34, bytes 251–263; `eval` reaches source tree
53, bytes 366–382. Neither full method root is published. The measured source
index contains distinct `Add.left` and `Add.right` class members and resolves
the source parent `Expr` canonically. Current Namer does not publish
`Add.unapply`, `_1`, or `_2`. An independent class-completion probe succeeds,
but adds none of those identities. Therefore the generated-unapply signature,
product selector types, binder matching, and two recursive calls are recorded
as `UNREACHED`; they were not inferred from a later hypothetical path.

The next workstream boundaries follow directly from those measurements:

| Issue | Evidence consumer |
| --- | --- |
| #2066 | Namer must publish bounded structural case-class identities while retaining distinct source fields. |
| #2071 | Typer must complete the canonical generated `Add$.unapply(Add): Add` signature. |
| #2072 | Typer must prove ordered reference-valued Product2 selectors and class completion. |
| #2076 | The first positive whole-root consumer is the original `selectLeft`. |
| #2078 | The second consumer is the original `eval` with two separate recursive arguments. |
| #2079 | Negative, rollback, and same-session retry behavior follows the supported positive slice. |
| #2080 | Strict fixture/report/CI acceptance consumes the merged prerequisites without changing production semantics. |

The pre-existing 18 source-root regressions and the protected Scala stdlib
acceptance sets (16/25 main and 15/25 analysis) remain unchanged. This increment
does not run the full hosted Scala corpus or change those baselines.

## Reproduction

With the pinned Scala checkout at `SCALA39_ROOT`, Temurin JDK at `JAVA_HOME`,
and the verified Maven cache populated:

```sh
SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8 \
JAVA_HOME=/path/to/temurin-21.0.8+9 \
crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/run

JAVA_HOME=/path/to/temurin-21.0.8+9 \
cargo +1.98.1 test -p dotty-typer \
  --test source_binary_recursive_adt_baseline --locked -- --nocapture
```

The pinned runner verifies Scala and JDK versions, all eight artifact hashes,
all 17 Scala source hashes, the immutable positive source, the complete typed
snapshot and its canonical owner/type relationships, all 16 actual control
classifications and diagnostics, and their recorded hashes. Scratch output
stays under `target/source-binary-recursive-adt-oracle` unless
`DOTTY_BINARY_RECURSIVE_ADT_TMP_ROOT` is set.
