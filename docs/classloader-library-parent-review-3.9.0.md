# Scala 3.9.0 library parent review (#1416)

This focused follow-up uses the immutable library cohort in
[`classloader-unresolved-tasty-parent-profile-3.9.0.tsv`](classloader-unresolved-tasty-parent-profile-3.9.0.tsv).
That profile identifies all 32 requested library roots, their exact Scala
3.9.0 `scala-library` artifact and SHA-256, every TASTy parent, the original
weighted observations, and the source-audit file counts. Its frozen cohort is
359 weighted observations across 32 roots. Those weights are repeated source
audit observations, not a count of distinct defective classes.

The current outcomes are captured separately in
[`classloader-library-parent-current-profile-3.9.0.tsv`](classloader-library-parent-current-profile-3.9.0.tsv).
It contains all 136 parent rows for the 32 library roots, including the
resolved binary name, resolution stage, standalone decode result, deepest
load cause, failing dependency, and dependency chain. This keeps measured
post-change results separate from the frozen historical outcomes below.

## Pinned root inventory

The first-parent columns below are copied from the frozen #1412 profile. They
describe the original diagnostic and are kept separate from the current
post-change profile. `unsupported-semantic-reference-shape` means the decoder
recognized a semantic reference but did not produce a binary identity; it is
not evidence that choosing a visible same-package name would be safe.

| Requested root | Frozen weight | First parent shape | Frozen first-parent outcome |
| --- | ---: | --- | --- |
| `scala/Predef$` | 97 | package-qualified `LowPriorityImplicits` | unsupported semantic shape |
| `scala/Tuple2` | 2 | applied `*:` | unsupported semantic shape |
| `scala/Tuple3` | 1 | applied `*:` | unsupported semantic shape |
| `scala/annotation/retainsCap` | 115 | package-qualified `Annotation` | unsupported semantic shape |
| `scala/annotation/switch` | 21 | package-qualified `Annotation` | unsupported semantic shape |
| `scala/collection/IndexedSeq` | 1 | package-qualified `Object` | structural name resolved; full load failed deeper |
| `scala/collection/LinearSeq` | 1 | package-qualified `Object` | structural name resolved; full load failed deeper |
| `scala/collection/Map` | 3 | package-qualified `Object` | structural name resolved; full load failed deeper |
| `scala/collection/Seq` | 9 | package-qualified `Object` | structural name resolved; full load failed deeper |
| `scala/collection/Set` | 2 | package-qualified `Object` | structural name resolved; full load failed deeper |
| `scala/collection/SortedMapOps` | 1 | package-qualified `Object` | structural name resolved; full load failed deeper |
| `scala/collection/View$` | 2 | package-qualified `Object` | structural name resolved; full load failed deeper |
| `scala/collection/convert/impl/BoxedBooleanArrayStepper` | 1 | applied `IndexedStepperBase` | unsupported semantic shape |
| `scala/collection/convert/impl/CharStringStepper` | 1 | applied `IndexedStepperBase` | unsupported semantic shape |
| `scala/collection/convert/impl/ObjectArrayStepper` | 1 | applied `IndexedStepperBase` | unsupported semantic shape |
| `scala/collection/convert/impl/RangeStepper` | 1 | applied `IndexedStepperBase` | unsupported semantic shape |
| `scala/collection/immutable/BitSet` | 5 | applied `AbstractSet` | unsupported semantic shape |
| `scala/collection/immutable/IndexedSeq` | 4 | package-qualified `Object` | structural name resolved; full load failed deeper |
| `scala/collection/immutable/Iterable` | 1 | package-qualified `Object` | structural name resolved; full load failed deeper |
| `scala/collection/immutable/List` | 15 | applied `AbstractSeq` | unsupported semantic shape |
| `scala/collection/immutable/Seq` | 6 | package-qualified `Object` | structural name resolved; full load failed deeper |
| `scala/collection/immutable/WrappedString` | 11 | applied `AbstractSeq` | unsupported semantic shape |
| `scala/collection/mutable/AbstractMap` | 14 | applied `AbstractMap` | unsupported semantic shape |
| `scala/collection/mutable/ArrayBuffer` | 5 | applied `AbstractBuffer` | unsupported semantic shape |
| `scala/collection/mutable/ArraySeq$` | 2 | package-qualified `Object` | structural name resolved; full load failed deeper |
| `scala/collection/mutable/Buffer` | 1 | package-qualified `Object` | structural name resolved; full load failed deeper |
| `scala/collection/mutable/HashMap` | 1 | applied `AbstractMap` | unsupported semantic shape |
| `scala/collection/mutable/ListBuffer` | 21 | applied `AbstractBuffer` | unsupported semantic shape |
| `scala/collection/mutable/Map` | 3 | package-qualified `Object` | structural name resolved; full load failed deeper |
| `scala/collection/mutable/Queue` | 1 | applied `ArrayDeque` | unsupported semantic shape |
| `scala/collection/mutable/StringBuilder` | 8 | applied `AbstractSeq` | unsupported semantic shape |
| `scala/io/BufferedSource` | 2 | package-qualified `Source` | unsupported semantic shape |

The inventory accounts for all 32 frozen roots and all 359 weighted
observations. The two annotation classes account for 136 observations; the
`Predef$` root accounts for 97. The focused current profile measured all 32
roots: 13 currently finish without a load cause, 3 fail with
`classloader.not_found`, 15 retain `tasty.unresolved_supertype`, and `Predef$`
reaches `tasty.missing_type_def` after its parent resolves to
`scala/LowPriorityImplicits`. The attached TSV records each parent's exact
outcome and the root's dependency failure.

## Proven identities and retained boundary

The annotation fixtures are Scala 3.9.0 TASTy 28.9 from the checked-in
`scala3-library` fixture. Their source definitions are in `scala.annotation`
and declare `Annotation` followed by `StaticAnnotation`. The classloader test
loads both real library TASTy classes and checks that exact parent order; it
does not accept a candidate based only on the lowercase annotation class
name. Their shared selected-type form is recovered from the complete TASTy
selection chain only when its proven owner is exactly the class's current
package. A longer path that would require nested JVM-name reconstruction stays
unresolved.

`Predef.tasty` declares `Predef` as an Object. Its parent tree carries a
semantic `LowPriorityImplicits` selection with the `scala` prefix. The pinned
TASTy profile resolves this directly to `scala/LowPriorityImplicits`; decoding
does not consult the bare-reference or package-object alias callbacks. This is
the same-package identity encoded by the declaration, not a default-import
guess. The classloader fixture corpus contains `Predef.tasty` but does not
contain its matching JVM `Predef$.class`, so this result is a parent-decoder
check rather than a claim that the module has completed as a `ClassInfo`.

The selected-parent resolver now accepts a reconstructed slash-separated
path only when the terminal node is a type selection and every node in its
qualifier chain is a package reference. A term reference or a selection in
the qualifier may name an object or a nested type, whose JVM identity needs
`$` separators; those forms remain `UnresolvedSupertype` unless their exact
binary owner is independently established. Two Scala 3.9.0 TASTy controls for
`Owner.Base` and `Other.Inner.Base` cover those one- and two-object cases; their
compiled JVM interfaces are `review/Owner$Base` and
`review/Other$Inner$Base` respectively.

Package-object alias traversal remains bounded to 64 hops and rejects cycles
and ambiguous/missing targets. The existing Scala 3.9 `Source` loading test
continues to exercise its `Iterator` package-object alias through a real
classloader. Applied collection parents, malformed owner chains, and
references whose semantic prefix cannot be recovered remain unresolved;
this change does not add default-import lookup or a general candidate guess.

## Validation

- `loads_scala39_annotation_types_with_their_declared_parents`: passes for
  `retainsCap` and `switch`, checking the exact two-parent hierarchy.
- `pins_predef_parent_to_its_exact_same_package_identity`: passes, checking
  the TASTy parent profile and decoded `Predef$` superclass without fallback
  callbacks.
- Existing `loads_scala39_source_with_its_applied_iterator_alias_parent`
  remains the positive package-object alias regression.
- `keeps_object_owned_selected_parent_unresolved` and
  `keeps_two_object_owned_selected_parent_unresolved`: pass for real Scala
  3.9.0 TASTy fixtures, covering one and two object-owner levels.
- The bounded 50-root profile was run twice with pinned Scala 3.9.0 JARs and
  the configured JDK 21. Both runs report 41 to 20 unresolved roots, 21 recovered roots /
  281 weighted observations, and zero newly unresolved roots; unresolved
  weighted observations change from 379 to 98. Both generated files are
  byte-identical (SHA-256 `08206a8a10bbb6b71a53a6515e89ceeded1d0eff43cfc1af9c9f0982059dcc5b`).
- The separate current-library TSV contains 136 rows covering all 32 roots /
  359 weighted observations (SHA-256
  `6d1470f10a217a96567cdea28f3398424db19b02aa4a5e38477ae1472a6ed742`).
- No full Typer source-corpus audit was run for this library-only increment.
