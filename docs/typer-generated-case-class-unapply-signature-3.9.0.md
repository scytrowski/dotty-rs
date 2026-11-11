# Generated case-class `unapply` signature (Scala 3.9.0)

Pinned Scala 3.9.0 at revision
`777528f19a58e794c9954a42f433373472ec57f8` emits more than one generated
extractor result shape. For `case class Box(value: Int)`, the generated method
is a plain one-argument method:

```scala
def unapply(x$1: matching.Box): matching.Box
```

The argument is the case-class instance, and the result is the same
source-owned `Box` class. In contrast, compiling the original pinned
`library/src/scala/util/Either.scala` with Scala 3.9.0's typer printer emits
these declarations:

```scala
def unapply[A, B](x$1: Left[A, B]): Option[A]
def unapply[A, B](x$1: Right[A, B]): Option[B]
```

Both methods belong to their corresponding generated companion module classes.
The source classes declare `+A, +B`; the generated method binders have no
declared variance. `Left` selects the first class parameter and `Right` the
second. The typer output also shows the generated bodies invoking
`Option.apply[A](x$1.value)` and `Option.apply[B](x$1.value)`, respectively.
This original-source output resolves the older Option-result summary in the
stdlib acceptance baseline; that summary does not describe the separate `Box`
fixture.

The Rust typer completes these canonical generated methods on demand as a
fresh two-parameter `PolyType` around a plain one-argument `MethodType`. The
method input is `Left[A, B]` or `Right[A, B]`, while the result is
`Option[A]` or `Option[B]` in source declaration order. Its `x$1` formal has a
distinct owner-specific derived symbol under `unapply`; it does not reuse the
constructor or generated `apply` formal. Ordinary companion member selection
resolves to that canonical method symbol. This generic rule accepts exactly
two class type parameters and one ordinary accessor whose declared type is
one of those parameters; it does not project Product selectors or type match
patterns.

Oracle provenance: source fetched from the pinned `scala/scala3` revision;
compiler/JDK artifacts are the SHA-256-pinned Scala 3.9.0/JDK 21 set in
`tools/typer-classpath-corpus-audit/either-companion-oracle`. The exact typer
output is retained in the implementation's registered scratch run.

Completion authenticates the generated method's role, source tree, class,
companion and module-class owner before accepting the signature. The generic
case is bounded to one accessor with a direct class type-parameter type;
unsupported arity, bounds, field or owner shapes leave the method incomplete
and can be retried without partial signature state. The preexisting monomorphic
`Box` result behavior remains unchanged.

## Remaining whole-method blocker

The frozen `unwrap` and `classify` method roots now pass generated-method
completion and ordinary unapply resolution. They stop at
`ExtractorResultMemberNotFound` for the product selector required by pattern
typing. The updated `rust-after-unapply.tsv` records this first remaining
error, including source spans. The historical `rust-baseline.tsv` still records
the earlier missing generated method state. This change does not add product
selectors or claim successful typing of either whole method.

The checked-in typed-tree oracle is
`crates/dotty-typer/tests/fixtures/source-case-class-match-3.9.0/Positive.scala39-typed-tree.txt`.
