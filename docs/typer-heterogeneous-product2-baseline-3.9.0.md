# Heterogeneous Product2 source baseline (Scala 3.9.0)

This is an oracle and current first-blocker baseline for issue #1958. It does
not change production Namer or Typer behavior. The immutable source is
`crates/dotty-typer/tests/fixtures/source-case-class-product2-3.9.0/Positive.scala`
(SHA-256 `4b144913df1d2021359c44633dc2a23f21bf6288bf2528c86cc6f53d79b8ee31`).
Its only numerator roots are `Positive.scala#tree24` (`Matcher.select`) and
`Positive.scala#tree44` (`Matcher.classify`); parser/Namer source spans and
canonical session-local owners are recorded in `rust-baseline.tsv`. The
integration test analyzes both roots in each of two fresh source sessions and
asserts identical canonical source, owner, field-order, and error relationships.
The emitted receipt names source identities as `path#tree`; arena-local symbol
numbers are only compared within one session and are not used as cross-session
identity.

## Pinned Scala oracle

The runner enforces Scala 3.9.0 artifacts and source revision
`777528f19a58e794c9954a42f433373472ec57f8`, Temurin 21.0.8+9, all eight
runtime artifact hashes, and the frozen source hashes. The positive typer
snapshot SHA-256 is
`e00a94565775f4c6b3424e65e68f0a1014cea681d092dde36777ef17d5072350`.
It confirms the actual Scala-generated declarations:

- `matching.Flagged` extends `scala.Product` and `scala.Serializable`.
- In source order, class-owned `value: Int` and `enabled: Boolean` are
  distinct constructor fields; class-owned `_1: Int` and `_2: Boolean` are
  distinct product selectors.
- `matching.Flagged$` owns `unapply(x$1: matching.Flagged): matching.Flagged`.
  The result is the case-class value itself, not an invented `Option` or tuple.
- `Matcher.select` binds `value: Int` and `enabled: Boolean`; its result is
  `Int`. `Matcher.classify` matches `Flagged(0, true)` and
  `Flagged(value, _)`, with the second binder `value: Int` and result `Int`.

These shapes agree with the pinned compiler source in
`compiler/src/dotty/tools/dotc/ast/Desugar.scala`: case-class product selectors
are generated from the first constructor parameter clause, non-enum case
classes add `scala.Product`, and the companion's generated `unapply` returns
the input case-class value for nonzero arity.

Observed matrix outcomes are pinned in `scala-matrix.tsv` with the compiler
diagnostic output for each row beside it (trailing whitespace is normalized).
Scala rejects arities one and three, the swapped literal types, an `Int`
result body for an `Int` method, the explicit-selector result mismatch, and a
non-case class without an extractor. In `ExplicitSelectorResultMismatch.scala`,
the declared `_1: String` selector binds `value: String`, which does not conform
to the declared `select(...): Int` result. The rejection is result conformance;
the explicit selector declarations themselves are accepted.

`ExplicitSelectors.scala` is the accepted control. Its typed-tree snapshot
`ExplicitSelectors.scala39-typed-tree.txt` independently pins class-owned
`_1: String` and `_2: String`, and the pattern binders `value: String` and
`enabled: String`; the enclosing `select` result is `String`. This confirms
that explicitly declared selectors suppress generated selectors, consistent
with the pinned compiler's `selectorNamesInBody`. A case class whose constructor
order is `(enabled: Boolean, value: Int)` is accepted and keeps that order. An
explicit companion `unapply` with a tuple result and an unrelated same-spelled
extractor are accepted with an exhaustivity warning. Those two cases are
Scala-valid warning cases, not invalid controls.

## Rust source baseline

The Rust test runs Lexer → Parser → Namer → `SourceTyper::type_method_definition`
against the same source. Each source constructor parameter gets a distinct
canonical field symbol with its own source tree and byte span. The source
companion and `apply` exist, but the current Namer index has no generated
`Flagged.unapply`, `_1`, or `_2` identity. Both whole-method attempts stop at
the extractor with `ExtractorUnapplyNotFound`; neither method enters the
source-to-typed index. This is the observed first blocker. Product identity or
selector completion is downstream and is not inferred from the Scala oracle.
The baseline uses the real source semantic store and current bootstrap
definitions; no classpath Product identity is available in that source-session
harness before the missing extractor blocks typing.

The full `dotty-typer` Rust test suite passed with this fixture. Existing Box
Match, lexical-closure, and inherited-method regressions remain in that suite;
the focused audit workflow continues to own the accepted 16+15 stdlib-root
controls. This change does not run a full hosted corpus audit or alter that
workflow.

The two-row first-blocker receipt is
[`typer-heterogeneous-product2-first-blockers-3.9.0.tsv`](typer-heterogeneous-product2-first-blockers-3.9.0.tsv).
The Rust first-blocker baseline was measured with rustc 1.98.1 and Cargo
1.98.1 against repository main commit
`fd090895e587852f9feca5599d4ddc55211ce81b`. The toolchain and Git commit SHA
are recorded in the receipt's `rust_toolchain` and `baseline_repository_sha`
columns. The Git SHA identifies the production repository state, while the
separate `baseline_receipt_sha256` field hashes the Rust receipt contents.
The Rust receipt SHA-256 is
`1c9a73c7dbb0c82664b53a9d784a2facadb1d62d7640fcd1a19464e73984a8c6`.

## Reproduction

With the pinned Scala source checkout, JDK and artifact cache available:

```sh
SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8 \
JAVA_HOME=/path/to/temurin-21.0.8+9 \
COURSIER_MAVEN_ROOT=/path/to/verified-maven-cache \
DOTTY_PRODUCT2_TMP_ROOT="$PWD/target/source-case-class-product2-oracle" \
crates/dotty-typer/tests/fixtures/source-case-class-product2-3.9.0/run

cargo +1.98.1 test -p dotty-typer --test source_case_class_product2_baseline \
  --locked -- --nocapture
```

The oracle runner verifies the pinned source/compiler/JDK/artifact hashes,
recompiles the complete positive typed-tree snapshot, checks the negative
matrix and diagnostic outputs, and verifies the output SHA manifest. Its
scratch and `java.io.tmpdir` remain under the supplied task-local target path.
The runner creates each compiler output directory before invoking scalac, so
`DOTTY_PRODUCT2_TMP_ROOT` may name a path that does not yet exist.
