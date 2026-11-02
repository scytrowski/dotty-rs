# Two-variant sealed ADT Match methods (Scala 3.9.0)

The frozen [`Event.scala`](../crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/Event.scala)
fixture now has complete source-owned typed `DefDef` receipts for exactly
`Matcher.extract` (`Event.scala#tree36`, bytes 150:262) and
`Matcher.classify` (`Event.scala#tree60`, bytes 266:400). The regression types
both roots in one authentic source session and repeats the compilation in a
fresh session, comparing normalized receipts without arena IDs.

The test checks each original method signature, parameter, result tree, Match,
ordered CaseDefs, selector, pattern roots, selected variant-owned `unapply`,
nested literal/binder/wildcard, and every source tree under the method RHS for a
`SourceTypedIndex` mapping. It also checks the direct source `Event` parent,
distinct `Created` and `Deleted` class identities, their companion-owned
extractors, their own `_1: Int` selectors, and distinct case-local binders.
`Created(0)`, `Created(id)`, and `Deleted(_)` retain source order and the result
joins to `Int`. The sealed flag does not trigger exhaustivity analysis.

[`rust-complete-methods.tsv`](../crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/rust-complete-methods.tsv)
records the two original pre-subtyping first errors and complete post-prerequisite
method receipts. Adjacent wrong result, wrong extractor arity, wrong pattern
type, unrelated extractor, foreign extractor-owner, and binder-escape controls
remain covered by `source_case_class_match.rs`; their failures do not publish a
typed root.

Reproduce the complete-tree proof with:

```sh
cargo +1.98.1 test -p dotty-typer --test source_case_class_match \
  sealed_event_extract_and_classify_are_complete_owned_methods -- --nocapture
```

The unchanged pinned Scala 3.9.0/JDK oracle is:

```sh
crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/run
```

It requires Scala revision `777528f19a58e794c9954a42f433373472ec57f8`,
Temurin JDK `21.0.8+9`, and the checked-in artifact and source manifests. The
oracle verifies the frozen compiler-generated typed-tree snapshot and its
classification matrix; this Rust test verifies source-to-typed identity and
ownership in the repository's own pipeline. No general ADT, exhaustivity,
transaction, or backend behavior is added here.
