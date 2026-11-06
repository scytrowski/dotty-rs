# Nested `Add(Num(value), right)` source-Typer proof (Scala 3.9.0)

This proof covers the nested pattern subtree from the pinned, accepted
`ForeignSameSpelledExtractor.scala` fixture (SHA-256
`d5b7367357aebf6a58d333d4dc10efc85e9da94bea0a64ce56c789250374a874`). It uses
the real Lexer → Parser → Namer → Typer path and separate source IDs in two
independent sessions. The method is a bounded probe whose body replaces
recursive `sum(right)` with `consume(right)`; this does **not** claim that the
original recursive `sum` method is accepted.

The outer selected method is source-owned `Foreign.Add.unapply(Expr):
Option[(Expr, Expr)]`, with a distinct owner and identity from the generated
`Add.unapply`. The nested method is the canonical generated `Num.unapply`.
Source-derived product checks establish `Add._1` and `Add._2` as distinct
`Expr` selectors and `Num._1` as `Int`. The typed source mapping retains both
extractor spans; the independent case-local `value: Int` and `right: Expr`
binders resolve to their corresponding body references.

The extractor result protocol uses ordinary journaled member selection.
For the foreign `Option` result, `get` adapts to `Tuple2[Expr, Expr]` and the
direct `Tuple2._1` bucket wins over inherited `Product2._1`. This preserves
the canonical Tuple2 selector SymbolId and its receiver-adapted first argument
type. A focused unit test also distinguishes ordinary selection from overload
discovery, and confirms that direct overload buckets and incomparable inherited
buckets remain ambiguous.

The classloader represents TASTy no-term-clause methods as `ByName(result)`.
At an extractor value-member boundary, the typer consumes `result` only for a
TASTy-owned method while leaving its stored symbol signature unchanged.
Explicit empty-clause methods, non-parameterless members, polymorphic protocol
members, and unsupported receiver shapes remain rejected.

Focused regressions cover the accepted foreign extractor in two fresh
sessions, wrong nested arity, an incompatible first component, escaped binder
scope, and a corrupted generated `Num.unapply` owner. Each negative test
checks failure rollback and a valid same-session retry. Existing
`peelLeft`/recursive-ADT baselines remain unchanged.

Run the focused proof with:

```sh
cargo +1.98.1 test -p dotty-typer \
  --test source_binary_recursive_adt_baseline \
  foreign_add_nested_num_pattern_preserves_owner_and_binder_provenance_in_fresh_sessions \
  --locked -- --exact
```

The pinned Scala oracle revision is
`777528f19a58e794c9954a42f433373472ec57f8`. Fixture classifications remain in
[`negative-cases.tsv`](../crates/dotty-typer/tests/fixtures/source-nested-recursive-adt-3.9.0/negative-cases.tsv)
and the original source tree spans remain in
[`source-tree-spans.tsv`](../crates/dotty-typer/tests/fixtures/source-nested-recursive-adt-3.9.0/source-tree-spans.tsv).
