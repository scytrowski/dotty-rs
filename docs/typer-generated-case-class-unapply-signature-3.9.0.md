# Generated case-class `unapply` signature (Scala 3.9.0)

Pinned Scala 3.9.0 at revision
`777528f19a58e794c9954a42f433373472ec57f8` distinguishes ordinary Scala 3
case classes from the Scala 2.13 library compatibility mode. For
`case class Box(value: Int)`, the generated method is a plain one-argument
method:

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

Those declarations use Scala 2.13 library compatibility, enabled in the
original `Either.scala` by ``import scala.language.`2.13` ``. The pinned
`Desugar.scala` selects the Option-producing body when
`Feature.shouldBehaveAsScala2`; otherwise its generated body returns the input
instance. A pinned Scala 3.9.0 typer run on renamed controls confirms the
distinction without relying on `Left`/`Right` spellings:

```scala
// with import scala.language.`2.13`
def unapply[A, B](x$1: LegacyLeft[A, B]): Option[A]
def unapply[A, B](x$1: LegacyRight[A, B]): Option[B]

// ordinary Scala 3 source
def unapply[A, B](x$1: Carrier[A, B]): Carrier[A, B]
```

Both methods belong to their corresponding generated companion module classes.
The source classes declare `+A, +B`; the generated method binders have no
declared variance. `Left` selects the first class parameter and `Right` the
second. The typer output also shows the generated bodies invoking
`Option.apply[A](x$1.value)` and `Option.apply[B](x$1.value)`, respectively.
The original-source output resolves the older Option-result summary in the
stdlib acceptance baseline. It applies to the source's Scala 2.13 compatibility
mode and does not describe ordinary Scala 3 generic case classes or the
separate `Box` fixture.

The Rust typer completes these canonical generated methods on demand as a
fresh two-parameter `PolyType` around a plain one-argument `MethodType`. In a
source context carrying the active ``scala.language.`2.13` `` import, the method
input is `LegacyLeft[A, B]` or `LegacyRight[A, B]`, and the result is
`Option[A]` or `Option[B]` in source declaration order. Ordinary Scala 3
generic case classes such as `Carrier[A, B](value: B)` retain the class-shaped
result `Carrier[A, B]`. The compatibility result resolves canonical
`_root_.scala.Option`; a package-local `Option` or `scala.nested.Option` does
not select or shadow it. The `x$1` formal has a distinct owner-specific derived
symbol under `unapply`; it does not reuse the constructor or generated `apply`
formal. Ordinary companion member selection resolves to that canonical method
symbol. This bounded generic rule accepts exactly two class type parameters
and one ordinary accessor whose declared type is one of those parameters; it
does not project Product selectors or type match patterns.

Oracle provenance: source fetched from the pinned `scala/scala3` revision;
`Either.scala` SHA-256 is
`7991d19a22ce0eb3d63b41ac61e7feabdeb97ed2f95555073cb3757224be8caa`;
compiler/JDK artifacts are the SHA-256-pinned Scala 3.9.0/JDK 21 set in
`tools/typer-classpath-corpus-audit/either-companion-oracle`. The renamed
compatibility controls (`LegacyLeft`, `LegacyRight`, and `LegacyCarrier`) and
ordinary `Carrier` control were compiled with those pinned Scala artifacts and
JDK 21; their exact typer output is retained in the implementation's registered
scratch run. The pinned `Desugar.scala` compatibility branch is recorded in
the same checkout at revision `777528f19a58e794c9954a42f433373472ec57f8`.

Completion authenticates the generated method's role, source tree, class,
companion and module-class owner before accepting the signature. The generic
case is bounded to one accessor with a direct class type-parameter type;
unsupported arity, bounds, field or owner shapes leave the method incomplete
and can be retried without partial signature state. The preexisting monomorphic
`Box` result behavior remains unchanged.

## Remaining whole-method blocker

Replaying the unchanged original `Either.scala#tree=84` after this increment
completes the canonical `Left.unapply` signature and stops at
`ExtractorUnapplyPolymorphic` on pattern tree 69 (source span `[8288, 8295)`).
The second `Right` extractor has not been reached. This is the existing pattern
typing boundary; this increment does not add polymorphic extractor-pattern
support or claim that `Either.swap` types as a whole.

The frozen `unwrap` and `classify` method roots now pass generated-method
completion and ordinary unapply resolution. They stop at
`ExtractorResultMemberNotFound` for the product selector required by pattern
typing. The updated `rust-after-unapply.tsv` records this first remaining
error, including source spans. The historical `rust-baseline.tsv` still records
the earlier missing generated method state. This change does not add product
selectors or claim successful typing of either whole method.

The focused 25+25 method-cohort verifier keeps its checked-in historical
receipt unchanged. Its live comparison permits only the measured transition
for `Either.scala#tree=84` from the historical missing extractor to
`ExtractorUnapplyPolymorphic` at pattern tree 69; all other receipt fields and
roots must still match exactly. This records the issue's generated-signature
progress without extending the acceptance scope to whole-method typing.

The checked-in typed-tree oracle is
`crates/dotty-typer/tests/fixtures/source-case-class-match-3.9.0/Positive.scala39-typed-tree.txt`.
