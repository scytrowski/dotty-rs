# Original `Tuple.toArray` ordinary-path proof (#2237)

This receipt verifies the unchanged Scala 3.9.0 root
`library/src/scala/Tuple.scala#tree=23` through ordinary
`SourceTyper::type_method_definition`, with no explicit owner completion.
The Scala source revision is
`777528f19a58e794c9954a42f433373472ec57f8`; `Tuple.scala` has SHA-256
`4d33bdb23f8446d313d531defd9957bb82672bd04211bca54d4bf08309c14d8e`.
The verified integration base includes #2234 at
`36bcd961ad00ad9af82f518baa32885c0df253d6`.

## Two fresh-session result

The ignored focused test was run in two separate Cargo test processes with the
pinned Scala 3.9.0 classpath and JDK 21. The existing
`typer-focused-audit.yml` workflow runs the same test twice, compares the
normalized receipts, and records the source revision and source hash. Each
session also injects a late failure after the successful ordinary method path,
checks rollback of owner and root mapping state, and retries successfully in
that same session.

Both local receipts were byte-identical (SHA-256
`cd49620da2cc7d88db190e0d4ff923ba0f551c4dd9abc3b0ec98e146c44b77e7`):

```text
TUPLE_TO_ARRAY_ACCEPTED source=Tuple.scala#tree=23 method=.scala.Tuple.toArray owner=.scala.Tuple mappings=282..350:DefDef=>DefDef,302..315:AppliedTypeTree=>TypeTree,322..329:Ident=>Ident,322..336:Select=>Select,322..344:Select=>Select,322..350:Apply=>Apply,345..349:This=>This selected=.scala.runtime.Tuples$.toArray argument=scala.Tuple.this result=scala.Array[java.lang.Object]
```

The mapping receipt uses source spans and tree kinds; it contains no
session-local typed-arena IDs. The assertions also verify the canonical
source owner `.scala.Tuple`, original `ThisType(scala.Tuple)` argument,
selected TASTy member `.scala.runtime.Tuples$.toArray`, loaded formal as a
`TypeRef` to the same source `Tuple`, and exact result
`scala.Array[java.lang.Object]`.

The focused workflow already pins the Scala runtime JAR hashes in
`crates/dotty-typer/tests/fixtures/lexical-closure-contract/scala-artifacts.sha256.tsv`
and verifies their downloads. The Tuple source hash and revision are recorded
in its two-session artifact. The workflow now also runs the focused fail-closed
control group explicitly.

## Negative controls and provenance

These controls exercise existing test seams only; no production typer,
resolver, or classpath behavior changed.

| Control | Regression evidence |
| --- | --- |
| Wrong receiver owner | `same_named_non_scala_tuple_is_not_prepared` presents a same-spelled `other.Tuple` to the owner-preparation seam and confirms it is not selected as canonical `scala.Tuple`. |
| Unrelated callee owner | `wrong_package_tuples_callee_does_not_complete_source_tuple` runs ordinary method typing against `_root_.scala.fake.Tuples`; the authentic `scala.Tuple` remains incomplete. |
| Conflicting classpath owner | `other_package_tuple_does_not_match_pinned_classpath_tuple_formal` observes the original `Apply` failure before rollback, proves its actual is `ThisType(.other.Tuple)` from the canonical source definition, and verifies the selected external `.scala.runtime.Tuples$.toArray` and `TypeRef(.scala.Tuple)` formal both have TASTy origin. It rejects with `ApplicationArgumentTypeMismatch` and rolls back the method root. |
| Classpath/source owner collision acceptance | The pinned original-root positive test verifies that the selected external TASTy method's formal resolves to the exact canonical source `scala.Tuple`, while the actual remains its `ThisType`. The malformed-formal control below is separately labeled as injected. |
| Unavailable selected formal | `malformed_selected_formal_is_rejected_and_rolls_back_source_owner` injects an `ErrorType` into a test source method signature, then asserts `ApplicationArgumentConformanceUnsupported` at the original argument and no committed owner/root mapping. This does not claim to test binary decoding. |
| Invalid expected result | `invalid_expected_result_rejects_original_shape_and_rolls_back_owner` declares `Boolean` while the selected call returns `Int`; ordinary method typing returns `ExpectedExpressionTypeMismatch` at the RHS and rolls back owner completion and source mappings. |
| Unrelated receiver | `unrelated_qualified_this_call_does_not_complete_product_owner` types an ordinary qualified call on a product owner and verifies no Tuple-specific completion is applied. |
| Late failure and retry | The pinned original-root test injects failure after outermost successful typing, verifies owner and mapping rollback, then successfully retries in the same `SourceTyper`. |

The original-root test also verifies complete source-to-typed mappings and
span preservation across all seven method nodes. Wrong-formal and wrong-package
controls retain their existing explicit error/status assertions.

## Unchanged ordinary cohorts

The unchanged 25-root main and 25-root analysis manifests were each run with
`--verify-determinism` and their existing required-root gate. Both passes
matched. Counts were:

| Cohort | Selected | Typed | Blocked | Excluded / unattempted |
| --- | ---: | ---: | ---: | ---: |
| Main | 25 | 19 | 6 | 0 / 0 |
| Analysis | 25 | 16 | 9 | 0 / 0 |

Comparing against the checked-in #2233 protected-success receipt found all 18
prior main identities/results and all 16 prior analysis identities/results
unchanged. `Tuple.toArray` is the one additive typed main root; the manifests
and denominators were not changed. The generated reports and per-pass reports
were retained in the local managed scratch during validation.
