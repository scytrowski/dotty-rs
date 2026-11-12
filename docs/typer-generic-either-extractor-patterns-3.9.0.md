# Generic `Either` extractor patterns (Scala 3.9.0)

This increment extends source pattern typing for authenticated generic
case-class extractors whose canonical source parent is applied as
`Either[X, Y]`. It uses the generated unapply's source-semantic identity and
the case class's ordered type parameters. It does not infer extractor type
arguments from parameter names or from a matching `Left`/`Right` spelling.

For an applied selector `Either[A, B]`, the source parent arguments map the
class parameters to the selector arguments by ordinal. The implementation
requires two covariant class parameters and a canonical two-parameter
covariant `scala.util.Either` parent; it then instantiates the generated
polymorphic unapply and checks its bounds. Unsupported metadata, unrelated
parents, or noncanonical generated methods continue to produce typed
rejections. The global subtype relation remains unchanged.

The regression fixture types both `Left(a)` and `Right(b)` with
`Either[A, B]`, preserves the distinct source type-parameter identities in the
binders, and checks rollback plus a valid same-session retry. It also rejects
a reversed applied parent, an unrelated extractor class, and wrong pattern
arity. The original pinned compatibility-mode `Either.scala` oracle remains
the checked-in evidence: `Left.unapply[A, B](Left[A, B]): Option[A]` and
`Right.unapply[A, B](Right[A, B]): Option[B]`; its typed tree binds the first
pattern to `A` and the second to `B`. The pre-change Rust observer recorded
the first failure at `Either.scala#tree=84`, pattern tree 69, span
`[8288, 8295)`, as `ExtractorUnapplyPolymorphic` after prerequisite generic
method identity, signature completion, and `Product1` projection were
available.

The unchanged original `Either.scala#tree=84` was replayed with the pinned
Scala 3.9.0 checkout and JDK 21 classpath. The `Left(a)` pattern no longer
fails at polymorphic unapply. The next first error is
`UnconstrainedTypeParameter` at tree 72, span `[8300, 8308)`, the first
branch's `Right(a)` constructor application. The original `Right(b)` pattern
is tree 77, span `[8318, 8326)`, and remains outside this whole-method replay
because the earlier branch body fails first. This increment does not type
branch constructor applications or claim whole-method `Either.swap`
acceptance.
