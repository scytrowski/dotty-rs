# Original `Tuple.toArray` owner preparation (#2183)

This regression records the original Scala 3.9.0 source root
`library/src/scala/Tuple.scala#tree=23` (`scala.Tuple.toArray`) without
changing production subtype or application conformance behavior. The source
checkout is pinned to `777528f19a58e794c9954a42f433373472ec57f8`; the original
`Tuple.scala` SHA-256 is
`4d33bdb23f8446d313d531defd9957bb82672bd04211bca54d4bf08309c14d8e`.

## Executable proof

The focused regression is
`original_tuple_to_array_types_after_explicit_canonical_owner_preparation` in
`crates/dotty-typer/src/typer/tests/issue_2179_relation_observation.rs`. It
parses and names the unmodified pinned Scala source, resolves `scala.Tuple`
through that source semantic index, and uses the existing public
`SourceTyper::complete_symbol` API explicitly before retrying the original
method.

The test first preserves the unprepared result as a labelled before-state. Its
pre-rollback observer verifies the exact first blocker at tree 22, argument 0:
actual `ThisType(scala.Tuple)` and expected `ErrorType`. The observer runs
before the failed method transaction rolls back, so the regression does not
inspect stale arena IDs afterward.

The prepared retry proves that the completed `ClassInfo.class` is the same
source-index owner, its declarations scope is the source declaration scope,
and the selected runtime `scala.runtime.Tuples$.toArray` formal is a
`TypeRef` of that same canonical owner. The original source `this` remains
mapped and typed as `ThisType(scala.Tuple)`. The complete original root is an
owned typed `DefDef`, with the original method, runtime selection and source
to typed mappings preserved; its result is exactly
`scala.Array[java.lang.Object]`.

The CI workflow runs this ignored external-input test in two separate Cargo
test processes, compares the normalized acceptance receipts byte-for-byte,
and uploads the logs and source/tested revision hashes. A local pair of fresh
processes on the implementation checkout produced identical receipts with
SHA-256
`256b380805c9ce8bf05ab04c14a40e7c08e9dbf6efdadf785b2a477bbcfbe115`:

```text
TUPLE_TO_ARRAY_ACCEPTED source=Tuple.scala#tree=23 method=.scala.Tuple.toArray owner=.scala.Tuple mappings=17:0,18:1,19:2,20:4,21:3,22:5,23:6 selected=.scala.runtime.Tuples$.toArray argument=scala.Tuple.this result=scala.Array[java.lang.Object]
```

This success is explicitly conditional on caller preparation. It does not
establish automatic source-owner completion from `type_method_definition` or
change the unprepared behavior. The protected root manifests and baseline
reports remain unchanged.
