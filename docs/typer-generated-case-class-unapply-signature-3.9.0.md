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
instance. Pinned Scala 3.9.0 typer runs on renamed controls confirm the
distinction without relying on `Left`/`Right` spellings. In one valid source
file, the import follows `Before`; the compilation-unit source mode applies to
both declarations:

```scala
package probe
case class Before[A, B](value: A)
import scala.language.{`2.13`, implicitConversions}
case class After[A, B](value: B)

// generated declarations
def unapply[A, B](x$1: probe.Before[A, B]): Option[A]
def unapply[A, B](x$1: probe.After[A, B]): Option[B]

// a separate ordinary Scala 3 source file
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
fresh two-parameter `PolyType` around a plain one-argument `MethodType`. The
presence of a valid ``scala.language.`2.13` `` feature import in the source
unit, including when selected alongside other named imports,
selects the compatibility result for supported generic case classes in that
unit, including declarations before the import: `Before[A, B]` returns
`Option[A]`, and `After[A, B]` returns `Option[B]`. Ordinary Scala 3 generic
case classes in a separate source unit, such as `Carrier[A, B](value: B)`,
retain the class-shaped result `Carrier[A, B]`. The compatibility result
resolves canonical `_root_.scala.Option`; a package-local `Option` or
`scala.nested.Option` does not select or shadow it. The `x$1` formal has a
distinct owner-specific derived symbol under `unapply`; it does not reuse the
constructor or generated `apply` formal. Ordinary companion member selection
resolves to that canonical method symbol. This bounded generic rule accepts
exactly two class type parameters and one ordinary accessor whose declared
type is one of those parameters; it does not project Product selectors or type
match patterns.

Oracle provenance: source fetched from the pinned `scala/scala3` revision;
`Either.scala` SHA-256 is
`7991d19a22ce0eb3d63b41ac61e7feabdeb97ed2f95555073cb3757224be8caa`;
compiler/JDK artifacts are the SHA-256-pinned Scala 3.9.0/JDK 21 set in
`tools/typer-classpath-corpus-audit/either-companion-oracle`. The compatibility
`Before`/`After` and separate ordinary `Carrier` controls were compiled with
those pinned Scala artifacts and JDK 21; their exact typer output is retained
in the implementation's registered scratch run. The pinned `Parsers.scala`
source-mode behavior and `Desugar.scala` compatibility branch are recorded in
the same checkout at revision `777528f19a58e794c9954a42f433373472ec57f8`.

Completion authenticates the generated method's role, source tree, class,
companion and module-class owner before accepting the signature. The generic
case is bounded to one accessor with a direct class type-parameter type;
unsupported arity, bounds, field or owner shapes leave the method incomplete
and can be retried without partial signature state. The preexisting monomorphic
`Box` result behavior remains unchanged.

## Follow-up generic extractor-pattern increment

Issue #2245 extends the pattern-typing boundary described above. With its
authenticated generic unapply instantiation, replaying the unchanged original
`Either.scala#tree=84` now passes the first `Left(a)` pattern and stops at the
next in-scope boundary: `UnconstrainedTypeParameter` at source tree 72, span
`[8300, 8308)`, the `Right(a)` constructor application in the first branch
body. The second `Right(b)` source pattern is at tree 77, span
`[8318, 8326)`; it is present in the unchanged original AST and in the pinned
Scala 3.9.0 typed-tree evidence, but the whole-method replay does not reach it
because branch constructor applications are outside #2245's scope. The
separate issue-2245 source fixture types both generic extractor patterns and
checks `a` against the first method parameter and `b` against the second.

This does not claim full `Either.swap` acceptance.

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
