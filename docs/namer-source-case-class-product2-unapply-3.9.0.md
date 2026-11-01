# Generated two-field case-class `unapply` identity (Scala 3.9.0)

The Namer publishes one canonical generated `unapply` Method for the bounded
top-level monomorphic source shape `case class Flagged(value: Int, enabled:
Boolean)`. It is owned by the linked `Flagged$` ModuleClass, has the class
visibility and synthetic flag, and is indexed by the original class plus the
`Unapply` role. Its sole synthetic `x$1` formal is owned by that Method and
derives from the original class declaration. The generated method has no
completed signature.

Eligibility reuses the existing source constructor checks: one ordinary
constructor clause, no type parameters, defaults, by-name or contextual
parameters, and plain parameter accessors. The existing one-field `Box`
behavior remains. Explicit companion collision checking is unchanged: a
direct exact `unapply(Flagged)` replaces the candidate; unresolved aliases or
overloads defer naming instead of publishing a second candidate. The pinned
Scala 3.9 oracle classifications for the source fixture are in
`crates/dotty-typer/tests/fixtures/source-case-class-product2-3.9.0/scala-matrix.tsv`.

This increment does not generate `_1`/`_2`, complete a `MethodType`, or type a
`Match`. Before/after identity summaries are in
[`namer-source-case-class-product2-unapply-3.9.0.tsv`](namer-source-case-class-product2-unapply-3.9.0.tsv).
