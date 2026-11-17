# Issue #2355: `Enumeration.ValueSet.knownSize` diagnosis

## Pinned input and cohort membership

The current-behavior and cohort audit used integration base
`19df8dbeefee0b768d063afd1a9de4c7b76b087d` (tree
`3180e295cf5a3264e51ffbd14b0b7c6ed20b239b`). The focused regression is
committed separately in this PR. The Scala source checkout is revision
`777528f19a58e794c9954a42f433373472ec57f8`; the unchanged
`library/src/scala/Enumeration.scala` SHA-256 is
`7370a0d85123fd15c9b75c132a38b911ad71f2a41e851e3121b5a6253939c1e8`.

The canonical source root is
`library/src/scala/Enumeration.scala#tree=700`,
`scala.Enumeration.ValueSet.knownSize`, line 306, bytes `11800..11840`:
`override def knownSize: Int = nnIds.size`. It belongs to the frozen 25-root
main manifest and is absent from the frozen 25-root analysis manifest. The
unchanged manifest SHA-256 values are `59bb65f48e7d94423de0f0a00029b98a0b41583e601c700687311e9e17fc5244`
(main) and `afa5a2064efd0bfcd31bb0bcb4fb08030db2d180a7619ac3a0843ebbe867f7bc`
(analysis).

## Measured first blocker

The focused test reads and names the complete original Scala file, then types
the source `DefDef` through `SourceTyper::type_method_definition` with the
pinned Scala library and JDK classpath. A test-only transaction observer
captures the diagnostic before rollback in two fresh sessions. Both sessions
produce the same normalized receipt:

```text
source_revision=777528f19a58e794c9954a42f433373472ec57f8
source=library/src/scala/Enumeration.scala#tree=700
owner=scala.Enumeration.ValueSet owner_symbol=SymbolId(71)
method=knownSize method_symbol=SymbolId(114)
source_span=11800..11840 expected_result=Int actual_result=unavailable
receiver=nnIds receiver_symbol=SymbolId(108) receiver_decl_tree=628
receiver_type=immutable.BitSet
failure_tree=627 span=11256..11272 kind=Select(BitSet)
error=SymbolResolution { source: SourceId(11), tree_index: 627, error: Malformed {
  reason: "class scala/collection/immutable/BitSet failed to load because
  scala/collection/immutable/AbstractSet failed: class
  scala/collection/immutable/AbstractSet failed to load because
  scala/collection/AbstractSet failed: class scala/collection/AbstractSet failed
  to load because scala/collection/Set failed: class scala/collection/Set failed
  to load because scala/collection/SetOps failed: class scala/collection/SetOps
  failed to load because scala/ImpureFunction1 failed: class not found:
  scala/ImpureFunction1"
} }
```

The failure span is the constructor parameter type `immutable.BitSet`; the
method body span is separate. `nnIds` is the source receiver, but its member
`size` is never looked up. The method result type is explicitly `Int`, and no
actual method result is available because owner/type completion stops first.
`BitSet` has no resolved external symbol in this session. The missing alias
also has no canonical `SymbolId` or applied type arguments: classpath loading
fails while resolving its TASTy reference.

## `ImpureFunction1` verdict and owner boundary

**Confirmed as an indirect classpath dependency; refuted as a direct source
alias in `knownSize`.** The method source contains no function type; its body is
`nnIds.size`. The live resolver failure occurs while reading the external
`immutable.BitSet`/collection parent chain, before source function-alias
normalization or body member lookup.

The failure names the exact chain
`immutable.BitSet -> immutable.AbstractSet -> scala.collection.AbstractSet ->
scala.collection.Set -> scala.collection.SetOps -> scala.ImpureFunction1`.
The Scala 3.9 `scala-library-3.9.0.jar` contains
`scala/collection/SetOps.tasty` (SHA-256
`e44dcf8d3b38f318c7ca8e84cb598ac6d35ca925b7586447fbb7d5bf451fe92d` inside
the JAR whose SHA-256 is
`b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd`); its
TASTy name table contains `ImpureFunction1` at byte offset 268. The alias is
not present as a `.class` or `.tasty` entry in any of the eight pinned Scala
classpath JARs. Existing classloader metadata evidence independently records
the same `Set -> SetOps -> ImpureFunction1` reference path in
[`classloader-library-parent-current-profile-3.9.0.tsv`](../classloader-library-parent-current-profile-3.9.0.tsv)
(rows 36–41). Existing source alias normalization remains covered by
`applied_impure_function_aliases_normalize_to_their_function_families` in
`source_function.rs`; that layer cannot normalize an external TASTy reference
whose classpath owner is absent.

The measured owner boundary is **Classloader**: resolve the Scala 3.9 external
TASTy alias/reference while completing this collection metadata chain. No
Typer repair is supported by this evidence. The smallest follow-up should
cover external `scala.ImpureFunction1` TASTy resolution used by
`scala.collection.SetOps`; acceptance should require this unchanged complete
`ValueSet.knownSize` method to type as `Int` against the official pinned
classpath, preserve the missing-alias diagnostic when the entry is absent,
and retain the different-owner `ArrayBuilder.knownSize` control below.

## Oracle, controls, and cohort preservation

The official Scala 3.9.0 command
`dotty.tools.dotc.Main -Ystop-after:typer -Xprint:typer` compiled the original
`Enumeration.scala` successfully and printed
`override def knownSize: Int = ValueSet.this.nnIds.size`. The full oracle log
SHA-256 is `798f8faf5978c207a18db973320ffda1ddba184418a4879fa26cf1cb5557c2c0`.
The focused two-session test also types the unchanged
`library/src/scala/collection/mutable/ArrayBuilder.scala#tree=48`
`scala.collection.mutable.ArrayBuilder.knownSize` as `Int`, which controls for
the shared member name under a different owner.

Both unchanged frozen 25-root manifests ran twice with determinism verification
on the integration base:

| Cohort | Attempts | Typed | Blocked | Normalized report SHA-256 |
| --- | ---: | ---: | ---: | --- |
| Main | 25 | 22 | 3 | `9c686c6a01eb8d26a0b4e3afb9ee6e52867da4ebe558d59608477cde6e4458a9` |
| Analysis | 25 | 17 | 8 | `a1c7212c7d6e92c5a9f0451e8d1f86b688cc7e5df7b54889774f9826014c54a8` |

The exact official typer output is in
[`issue-2355-scala39-enumeration-oracle.txt`](issue-2355-scala39-enumeration-oracle.txt).
Re-run both full cohort reports with
`tools/typer-classpath-corpus-audit/run --method-kind ordinary --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv --verify-determinism OUTPUT`
and the same command using
`docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv`. Re-run the focused
observer and wrong-owner control with
`cargo test -p dotty-typer --lib scala39_enumeration_known_size_records_first_failure_deterministically --locked -- --ignored --nocapture`,
setting `SCALA39_ROOT`, `SCALA39_CLASSPATH`, `JAVA_HOME`,
`SCALA39_JDK_RELEASE=21`, and `TMPDIR` to the pinned inputs and a managed
scratch directory as the cohort runner does. The full report SHA-256 values
above bind the audited 25-root outputs without adding their trailing empty TSV
cells to this patch.

All 21 prior typed main roots and all 17 prior typed analysis roots retained
their owners and result shapes. `ValueSet.knownSize` remains blocked and earns
no typed-root credit. This evidence PR changes no production semantics or
cohort manifests.

The oracle and classpath audit used Eclipse Temurin 21.0.13 (`release` SHA-256
`f83f6dcc966043c93cd498ac8e7ee7c310852074970d62c702e4ad2710002898`, `java`
SHA-256 `9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`,
`java.base.jmod` SHA-256
`fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`).
