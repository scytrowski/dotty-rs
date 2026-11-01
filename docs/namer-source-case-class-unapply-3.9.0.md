# Generated case-class `unapply` identity and typer discovery (Scala 3.9.0)

This increment publishes a canonical generated `unapply` identity for the
bounded supported source shape: a top-level, non-generic case class with one
ordinary constructor accessor and a comparable companion signature. The
identity is keyed by the canonical class and generated-method role, with
reverse provenance to the class's source tree. The existing generated `apply`
lookup and its derived-symbol identity remain unchanged.

The generated method is entered in the companion module-class scope as a
synthetic Method with the class visibility, source origin, declaration scope,
and declaration context. It has no completed signature. Explicit
`unapply(Box)` replaces the generated method, while an `unapply` overload with
a different direct class parameter coexists with it. Source shapes whose
collision cannot be compared without type completion remain explicitly
unsupported.

## Pinned oracle controls

The controls were run against Scala 3.9.0 at revision
`777528f19a58e794c9954a42f433373472ec57f8`. The source hashes and accepted or
rejected classifications are in the neighboring
`source-case-class-match-3.9.0` fixture manifest and the matrix in
[`typer-source-case-class-match-baseline-3.9.0.md`](typer-source-case-class-match-baseline-3.9.0.md).
Scala accepts an explicit same-shape extractor and an overload beside the
generated extractor, and rejects the exact-shape extractor with an invalid
result. Those controls constrain namer collision behavior; this increment
does not type their signatures.

## Live typer discovery

The frozen #1933 before-state is retained in
`crates/dotty-typer/tests/fixtures/source-case-class-match-3.9.0/rust-baseline.tsv`.
It records `apply=present`, `unapply=absent`, and
`ExtractorUnapplyNotFound` for the `unwrap` and `classify` roots. The live
after-state is recorded separately in `rust-after-unapply.tsv`.

The same two roots now enter the actual `SourceTyper::type_method_definition`
path. Extractor lookup discovers the generated companion method `SymbolId(47)`.
Typer then stops at the next authentic blocker:
`SymbolSourceKindMismatch { tree_index: 6, symbol: SymbolId(47), kind: Method }`.
The generated Method's canonical provenance points to the Box `TypeDef`, and
the typer does not yet dispatch that synthetic method to a signature
completion rule. This error proves lookup reached the canonical unapply
identity without manufacturing a signature. No extractor result, selector,
or typed method root is produced; retries leave the typed index empty.

The two live rows are run twice in fresh sessions and must normalize
identically. The baseline test also checks the role-forward and reverse
identity, companion scope entry, and continued absence of `_1`.

## Deliberate limits

This increment does not complete the synthetic method's `MethodType` or body,
define `Product`/`_1` projection, complete `Match` typing, or change JVM, IR,
or TASTy behavior. Unsupported source shapes do not receive guessed
signatures.

## Reproduce

With the pinned Scala checkout and JDK paths described by the adjacent
baseline document, run:

```sh
crates/dotty-typer/tests/fixtures/source-case-class-match-3.9.0/run
cargo test -p dotty-typer --test source_case_class_match_baseline --locked
cargo test -p dotty-core -p dotty-namer --locked
```
