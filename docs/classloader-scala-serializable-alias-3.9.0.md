# Scala 3.9 `scala.Serializable` package alias

The pinned library defines `Serializable` in `scala/package.tasty` as the type
alias `java.io.Serializable`. Its SHA-256 is
`8810ac8864239a34a9df4004090d89caad00e1fa71c6b2186bd7f6661c1fad90`; no
nominal `scala/Serializable` artifact is part of the fixture. The classloader
resolves the authenticated rooted selected-parent form only when the selected
owner is `scala`, the selected type is `Serializable`, and the qualifier path
is exactly `_root_/scala`. It then reuses package-object alias decoding and
requires `java/io/Serializable` to exist on the configured classpath. This
does not normalize other rooted owners or add a Scala substitute.

The loader regression completes the authentic pinned `scala/Some.tasty` with
the real JDK `java.base.jmod`. It checks the canonical `java/io/Serializable`
symbol, its `Jmod(java.base.jmod)` resource origin, same-session identity, and
stable parent names in a fresh session. A controlled first lookup hides the
target, verifies that loading fails, and retries successfully after the same
classpath evidence becomes visible. Unit regressions check the authentic
package alias RHS and reject missing target evidence, malformed package TASTy,
and wrong alias ownership. Existing object-owned and nested selected-parent
tests continue to require explicit failure.

At the issue base `bdf64e102ac7d5dc247205a7365b5728f883d1d6`, the pre-change
`scala/Some` load failed on a nominal `scala/Serializable` lookup; after the
change that parent resolves to `java/io/Serializable`. The
frozen semantic comparison against `comparison-1948.tsv` therefore gains nine
`some` rows: identity, visibility, flags, hierarchy, type parameters,
parameter clauses, member signatures, companion, and annotations. Before the
change each row was `UNRESOLVED_EXTERNAL` with the same missing
`scala/Serializable` dependency. The updated test checks each resulting value;
the hierarchy is `scala.Option&scala.Product&java.io.Serializable`. The
`member_signatures` row is `LOADER_UNSUPPORTED` because the newly completed
class later encounters the unrelated missing `scala/Predef/String` member
type, while preserving its thirteen declared member names. The existing
`option/member_signatures` delta from #1948 remains the tenth changed row and
is checked separately. The historical TSV is not regenerated or modified.

Follow-up #2180 preserves the class type argument in `Some`'s applied parent.
The current hierarchy observation is therefore
`scala.Option[scala.Some.A]&scala.Product&java.io.Serializable`; the value
above records the earlier alias-resolution increment before the parent
projection correction.

No TASTy bytes or other frozen fixture expectations change. Existing Array
Java-parent and package-alias behavior remain covered by their regression
tests.
