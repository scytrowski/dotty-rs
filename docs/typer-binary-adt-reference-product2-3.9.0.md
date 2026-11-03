# Source reference Product2 completion for a binary ADT (Scala 3.9.0)

This increment extends source case-class Product2 completion to the bounded
recursive shape:

```scala
sealed trait Expr
final case class Num(value: Int) extends Expr
final case class Add(left: Expr, right: Expr) extends Expr
```

For `Add`, Typer retains the original source class and both canonical
constructor fields. It completes `_1` and `_2` as separate class-owned,
synthetic, parameterless methods. Each selector is derived from its own
constructor parameter tree and returns a TypeRef to the same canonical source
`Expr` trait. The class parents are normalized to `Object, Expr,
scala.Product`, with one authenticated Product edge. The bounded source-parent
Product2 path requires both fields to have that exact direct parent symbol;
unrelated or mixed field shapes remain unsupported. The existing parentless
`Int, Boolean` Product2 path is preserved.

The focused regression drives the existing extractor-pattern typer with the
generated `Add$.unapply(Add): Add` signature. Normal result-member lookup reads
the two source selectors in order and returns `[Expr, Expr]`. It checks each
selector's owner and individual source-parameter provenance. It does not
change the generated unapply signature or add Match implementation code. The
existing two-root characterization now records that `eval` and `selectLeft`
complete after Product2 selectors become available; this records a consumer
consequence and does not claim the dedicated downstream proofs tracked by
#2076/#2078. The historical pre-Product2 receipt remains unchanged.

Focused tests defer projection for a mixed second field, a foreign
same-spelled `Expr`, and an `Int, Boolean` pair on a source-parented class.
Those ordinary classes retain `Object` and their source parent but receive no
Product edge and no partially completed selectors. Separate parentless
Product2 regressions exercise rollback when field-type validation fails.
Existing Product1 source-trait controls remain covered.

The pinned Scala 3.9.0 oracle uses revision
`777528f19a58e794c9954a42f433373472ec57f8` and Temurin 21.0.8+9. The checked-in
`source-binary-recursive-adt-3.9.0/Expr.scala39-typed-tree.txt` records the
oracle's source `Expr` / `Add` and generated unapply shapes. The issue-specific
before/after semantic summary is in
[`typer-binary-adt-reference-product2-3.9.0.tsv`](typer-binary-adt-reference-product2-3.9.0.tsv).
The current two-root observation is in
[`rust-after-product2-two-root.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/rust-after-product2-two-root.tsv);
the preserved historical measurement is
[`rust-after-typer-unapply-two-root.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/rust-after-typer-unapply-two-root.tsv).
