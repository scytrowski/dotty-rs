# Generated case-class `unapply` identity and typer discovery (Scala 3.9.0)

The Namer publishes a canonical generated `unapply` identity for two bounded
source shapes: a top-level, non-generic case class with one ordinary
constructor accessor, and a top-level generic case class with exactly two
unbounded type parameters and one ordinary constructor accessor whose direct
type reference names one of those parameters. The latter covers Scala 3.9.0's
`Left[+A, +B](value: A)` and `Right[+A, +B](value: B)` declarations without
matching their names. The identity is keyed by the canonical class and
generated-method role, with reverse provenance to the class's source tree.
The existing generated `apply` lookup and its derived-symbol identity remain
unchanged.

The generated method is entered in the companion module-class scope as a
synthetic Method with the class visibility, source origin, declaration scope,
and declaration context. It has no completed signature. Generic Product1
identity does not generate a selector or infer a `Product1` projection.
An explicit `unapply(Box)` replaces the generated method. Namer does not treat
a different simple type name as proof of a distinct parameter type: package,
imported, or companion-local aliases can name the same class. When the
parameter identity cannot be established from the direct source shape, naming
returns the existing unsupported companion-shape error without publishing a
generated candidate. This also defers otherwise valid overloads such as
`unapply(String)` until source type identity can be resolved safely.

## Pinned oracle controls

The controls were run against Scala 3.9.0 at revision
`777528f19a58e794c9954a42f433373472ec57f8`. The source hashes and accepted or
rejected classifications are in the neighboring
`source-case-class-match-3.9.0` fixture manifest and the matrix in
[`typer-source-case-class-match-baseline-3.9.0.md`](typer-source-case-class-match-baseline-3.9.0.md).
Scala accepts an explicit same-shape extractor and an overload beside the
generated extractor, and rejects the exact-shape extractor with an invalid
result. Scala also accepts package-level, imported, and companion-local aliases
for the case class as explicit extractor parameters. The Rust Namer explicitly
defers these unresolved identities instead of publishing a second candidate;
this increment does not complete alias types or the extractor signatures.

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
or TASTy behavior. Unknown explicit parameter types remain unsupported until
canonical source identity is available; no alias meaning or signature is
guessed. Generic synthesis is limited to exactly two unbounded class type
parameters, one ordinary constructor field, and a direct field-type reference
to either parameter. It does not admit bounds, unrelated field types, multiple
fields/clauses, defaults, by-name or repeated fields, or contextual clauses.

## Generic Product1 identity (#2238)

The pinned Scala source declares `Left[+A, +B](value: A)` and
`Right[+A, +B](value: B)` in `scala.util`; the existing Rust Namer source path
previously omitted both generated `unapply` roles because it required an empty
class type-parameter list. The exact original `Either.scala#tree=84` root's
prior ordinary-cohort result is `ExtractorUnapplyNotFound`, recorded in
[`namer-top-level-case-class-companions-roots-1825-3.9.0.tsv`](namer-top-level-case-class-companions-roots-1825-3.9.0.tsv).
The new structural rule recognizes two plain or variance-marked unbounded
class parameters and exactly one ordinary accessor whose direct type identifier
matches either parameter. It intentionally leaves generated method info as
`Missing` and creates no Product1 selector. Focused Namer regressions cover
both original declarations, renamed `Box[X, Y](value: X)`, swapped
`Swapped[X, Y](value: Y)`, and unsupported arity, bounds, unrelated fields,
multiple fields/clauses, defaults, repeated fields, and contextual fields. An
explicit companion candidate whose input identity cannot be compared still
fails atomically under the existing Namer rule; no generated candidate
survives that failure.

## Reproduce

With the pinned Scala checkout and JDK paths described by the adjacent
baseline document, run:

```sh
crates/dotty-typer/tests/fixtures/source-case-class-match-3.9.0/run
cargo test -p dotty-typer --test source_case_class_match_baseline --locked
cargo test -p dotty-core -p dotty-namer --locked
```
