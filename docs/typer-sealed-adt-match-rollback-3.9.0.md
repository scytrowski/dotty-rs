# Sealed `Event` match rollback audit (Scala 3.9.0)

This audit checks case-local scopes and failure atomicity for the two complete
source methods established by #1998. It preserves the #1992 input
[`Event.scala`](../crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/Event.scala)
byte-for-byte (SHA-256
`a3554b9a764b612af5b6b73779839af82a5e448e909464e45cbc18fb8a86cba1`). The
only positive roots are `Matcher.extract` (`Event.scala#tree36`, bytes 150–262)
and `Matcher.classify` (`Event.scala#tree60`, bytes 266–400). Both are complete
`DefDef` roots with typed `Match` bodies; the existing #1998 receipt records
their source identities, exact method signatures, variant extractors, and
case-local binders.

The negative controls below start from this same canonical input and mutate
only one pattern, binder reference, result, or semantic structure at a time.
Their first diagnostic and post-failure state are checked by the focused
typer test. Scala classification is taken from the pinned 3.9.0 compiler and
does not imply that every warning-only control should be rejected by Rust.

