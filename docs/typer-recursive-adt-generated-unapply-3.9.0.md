# Recursive ADT generated `unapply` signature (Scala 3.9.0)

This increment completes the existing source-generated case-class signature
path for the pinned `Succ` declaration:

```scala
sealed trait Expr
final case class Num(value: Int) extends Expr
final case class Succ(inner: Expr) extends Expr
```

`Succ$.unapply(x$1: Succ): Succ` is one ordinary, non-erased `Method` parameter
and result. `Succ.inner` remains a source-owned `Field` of type `Expr`; it is
not the generated method input or result. The method input is the synthetic
`x$1` owned by `Succ$.unapply`, and both method type references target the
canonical `Succ` class.

## Pinned oracle

The source and Scala 3.9.0 typed-tree oracle are frozen in the
[`recursive ADT fixture`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/Expr.scala)
and its [`typed tree`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/Expr.scala39-typed-tree.txt).
The oracle uses Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`, Temurin 21.0.8+9, and the adjacent
artifact manifest. It records `Succ$.unapply(x$1: adt.this.Succ): adt.this.Succ`,
`Succ.inner: Expr`, and the distinct source class and generated method owners.

## Rust path and controls

Ordinary `SourceTyper::complete_symbol` completes the class metadata, validates
the single constructor `Field`, resolves its completed type in the field's
source context, and accepts a reference field only when its canonical source
trait is the same monomorphic direct source parent. The generated formal and
the input/result `TypeRef`s remain tied to the generated method and the class
symbols respectively. The integer single-field case and the bounded
`Int`/`Boolean` two-field case keep their existing signatures.

The before/after [`method receipt`](../crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/rust-generated-unapply-signature.tsv)
captures the authentic pre-change result on merged #2036 revision
`deedd4c8581b31392c38f48060774466f50110c0` and the implementation result. It
shows `Num.unapply` unchanged and `Succ.unapply` moving from
`MalformedMethodClause` to `(Succ):Succ`. The ordinary recursive-ADT integration
test also records both `unwrap` and `eval` completing through the existing
source Match typer; the two-root set and denominator are unchanged, and no Match
implementation was changed. The focused unit regressions cover ordinary
companion member lookup, class-first and unapply-first completion, repeat
idempotence, declaration reordering, independent sessions, canonical identity,
an unrelated reference field, rollback, and valid sibling retry.

This remains a bounded monomorphic signature increment. It does not generalize
generated unapply to arbitrary reference fields, applied or generic types,
multiple fields of new shapes, or a new `Option` result contract. Product1
selector projection remains owned by #2036.

## Reproduction

```sh
cargo +1.98.1 test -p dotty-typer --lib generated_case_class_unapply_has_source_canonical_plain_signature_and_member_lookup --locked
cargo +1.98.1 test -p dotty-typer --lib reference_unapply_rejects_unrelated_field_transactionally_and_retries_sibling --locked
cargo +1.98.1 test -p dotty-typer --test source_recursive_adt_baseline --locked
```
