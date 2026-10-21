# Scala 3.9.0 library parent review (#1416)

This focused follow-up uses the immutable library cohort in
[`classloader-unresolved-tasty-parent-profile-3.9.0.tsv`](classloader-unresolved-tasty-parent-profile-3.9.0.tsv).
That profile identifies all 32 requested library roots, their exact Scala
3.9.0 `scala-library` artifact and SHA-256, every TASTy parent, the original
weighted observations, and the source-audit file counts. Its frozen cohort is
359 weighted observations across 32 roots. Those weights are repeated source
audit observations, not a count of distinct defective classes.

## Pinned root inventory

The first-parent columns below are copied from the frozen #1412 profile. They
describe the original diagnostic and are kept separate from the focused
post-#1413 checks. `unsupported-semantic-reference-shape` means the decoder
recognized a semantic reference but did not produce a binary identity; it is
not evidence that choosing a visible same-package name would be safe.

| Requested root | Frozen weight | First parent shape | Frozen first-parent outcome | Focused #1416 outcome |
| --- | ---: | --- | --- | --- |
| `scala/Predef$` | 97 | package-qualified `LowPriorityImplicits` | unsupported semantic shape | Parent decodes as exactly `scala/LowPriorityImplicits` from the pinned 28.9 TASTy with no bare-name or alias callback. |
| `scala/Tuple2` | 2 | applied `*:` | unsupported semantic shape | Retained as a negative/unverified boundary; applied constructor handling is outside this library increment. |
| `scala/Tuple3` | 1 | applied `*:` | unsupported semantic shape | Retained as a negative/unverified boundary; applied constructor handling is outside this library increment. |
| `scala/annotation/retainsCap` | 115 | package-qualified `Annotation` | unsupported semantic shape | Real classpath loading succeeds with exact ordered parents `Annotation`, `StaticAnnotation`. |
| `scala/annotation/switch` | 21 | package-qualified `Annotation` | unsupported semantic shape | Real classpath loading succeeds with exact ordered parents `Annotation`, `StaticAnnotation`. |
| `scala/collection/IndexedSeq` | 1 | package-qualified `Object` | structural name resolved; full load failed deeper | Retained baseline; not retested in this increment. |
| `scala/collection/LinearSeq` | 1 | package-qualified `Object` | structural name resolved; full load failed deeper | Retained baseline; not retested in this increment. |
| `scala/collection/Map` | 3 | package-qualified `Object` | structural name resolved; full load failed deeper | Retained baseline; not retested in this increment. |
| `scala/collection/Seq` | 9 | package-qualified `Object` | structural name resolved; full load failed deeper | Retained baseline; not retested in this increment. |
| `scala/collection/Set` | 2 | package-qualified `Object` | structural name resolved; full load failed deeper | Retained baseline; not retested in this increment. |
| `scala/collection/SortedMapOps` | 1 | package-qualified `Object` | structural name resolved; full load failed deeper | Retained baseline; not retested in this increment. |
| `scala/collection/View$` | 2 | package-qualified `Object` | structural name resolved; full load failed deeper | Retained baseline; not retested in this increment. |
| `scala/collection/convert/impl/BoxedBooleanArrayStepper` | 1 | applied `IndexedStepperBase` | unsupported semantic shape | Retained as an applied-constructor boundary; not retested. |
| `scala/collection/convert/impl/CharStringStepper` | 1 | applied `IndexedStepperBase` | unsupported semantic shape | Retained as an applied-constructor boundary; not retested. |
| `scala/collection/convert/impl/ObjectArrayStepper` | 1 | applied `IndexedStepperBase` | unsupported semantic shape | Retained as an applied-constructor boundary; not retested. |
| `scala/collection/convert/impl/RangeStepper` | 1 | applied `IndexedStepperBase` | unsupported semantic shape | Retained as an applied-constructor boundary; not retested. |
| `scala/collection/immutable/BitSet` | 5 | applied `AbstractSet` | unsupported semantic shape | Retained as an applied-constructor boundary; not retested. |
| `scala/collection/immutable/IndexedSeq` | 4 | package-qualified `Object` | structural name resolved; full load failed deeper | Retained baseline; not retested in this increment. |
| `scala/collection/immutable/Iterable` | 1 | package-qualified `Object` | structural name resolved; full load failed deeper | Retained baseline; not retested in this increment. |
| `scala/collection/immutable/List` | 15 | applied `AbstractSeq` | unsupported semantic shape | Retained as an applied-constructor boundary; not retested. |
| `scala/collection/immutable/Seq` | 6 | package-qualified `Object` | structural name resolved; full load failed deeper | Retained baseline; not retested in this increment. |
| `scala/collection/immutable/WrappedString` | 11 | applied `AbstractSeq` | unsupported semantic shape | Retained as an applied-constructor boundary; not retested. |
| `scala/collection/mutable/AbstractMap` | 14 | applied `AbstractMap` | unsupported semantic shape | Retained as an applied-constructor boundary; not retested. |
| `scala/collection/mutable/ArrayBuffer` | 5 | applied `AbstractBuffer` | unsupported semantic shape | Retained as an applied-constructor boundary; not retested. |
| `scala/collection/mutable/ArraySeq$` | 2 | package-qualified `Object` | structural name resolved; full load failed deeper | Retained baseline; not retested in this increment. |
| `scala/collection/mutable/Buffer` | 1 | package-qualified `Object` | structural name resolved; full load failed deeper | Retained baseline; not retested in this increment. |
| `scala/collection/mutable/HashMap` | 1 | applied `AbstractMap` | unsupported semantic shape | Retained as an applied-constructor boundary; not retested. |
| `scala/collection/mutable/ListBuffer` | 21 | applied `AbstractBuffer` | unsupported semantic shape | Retained as an applied-constructor boundary; not retested. |
| `scala/collection/mutable/Map` | 3 | package-qualified `Object` | structural name resolved; full load failed deeper | Retained baseline; not retested in this increment. |
| `scala/collection/mutable/Queue` | 1 | applied `ArrayDeque` | unsupported semantic shape | Retained as an applied-constructor boundary; not retested. |
| `scala/collection/mutable/StringBuilder` | 8 | applied `AbstractSeq` | unsupported semantic shape | Retained as an applied-constructor boundary; not retested. |
| `scala/io/BufferedSource` | 2 | package-qualified `Source` | unsupported semantic shape | Retained baseline; not retested in this increment. |

The inventory accounts for all 32 frozen roots and all 359 weighted
observations. The two annotation classes account for 136 observations; the
`Predef$` root accounts for 97. The other 30 roots remain explicitly at their
frozen outcomes here: this focused increment did not rerun a whole-source
audit or claim that an unchanged profile row is a current successful load.

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
- No full Typer source-corpus audit was run for this library-only increment.
