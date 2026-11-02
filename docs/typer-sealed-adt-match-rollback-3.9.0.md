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

The negative controls start from this same canonical input and mutate one
pattern, binder reference, result, or semantic structure at a time. The
[`rust-failure-matrix.tsv`](../crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/rust-failure-matrix.tsv)
records each first Typer error, source method and Match trees, and byte spans.
It covers both Boolean literals, zero/two extractor arities, late result
conformance, cross-case binder escape, an unrelated `String` extractor, a
foreign same-spelled `Created` extractor, and `Deleted` as a non-`Event`
variant. Each malformed root is retried in the same `SourceTyper`. Before the
failure, a separate successful sibling is retained. After failure and retry,
the test types both untouched canonical `extract` and `classify` roots for the
first time in that same typer; a fresh session must report the same normalized
diagnostic tree/span and produce matching receipts for both roots.

The internal late-result test observes the transaction before rollback. Both
case-local `Int` binders (`id`, `code`), both typed `CaseDef`s, and the typed
`Match` exist when the declared `Boolean` result fails. Afterwards the test
compares all symbol infos, scope entries and buckets, types, resolver checkpoint,
typed nodes, source-to-typed mappings, local scopes, pattern bindings, and
expression scope frames with the pre-failure snapshot. The earlier successful
root remains published, while the failed method and Match have no published
mappings. Retrying produces the same diagnostic and unchanged snapshot.

Rust-only structural corruptions also exercise complete source-method rollback.
The private lifecycle test keeps a successful `prior` root, then corrupts the
`Created` companion link, the generated `Created._1` owner, or the `Event`
parent `ClassInfo` with an unrelated parent, duplicate edge, invalid `TypeId`,
or missing parent edge. For every row it attempts and retries the complete
`extract` `DefDef` in one live `SourceTyper`, compares the full semantic
projection, restores the original structure without dropping that typer, then
types both uncached `extract` and `classify` roots there. The
[`rust-structural-lifecycle.tsv`](../crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/rust-structural-lifecycle.tsv)
records each first diagnostic category and recovered roots. The separate
parent-graph matrix still checks class-first and selector-first completion
routes, and a negative test retains the foreign `unapply` owner check.

The only transaction repair is in `Scope::remove`: removing the final symbol
now deletes its empty name bucket. The failing extractor cases previously left
empty entries in pre-existing scopes, making rollback observably different from
the before snapshot. A unit test covers both last-symbol removal and preserving
overload siblings. No pattern acceptance or type relation was broadened;
generic, union, and unsupported extractor relationships remain deferred. The
adjacent tests `generic_typed_pattern_defers_runtime_test_without_partial_state`,
`unsupported_typed_pattern_relation_is_deferred`,
`unsupported_pattern_selector_relation_is_deferred`, and
`unrelated_extractor_input_stays_deferred_after_nominal_narrowing_attempt`
keep those boundaries explicit.

The pinned Scala case results and first compiler diagnostics are recorded in
[`scala-failure-matrix.tsv`](../crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/scala-failure-matrix.tsv).
All nine mutated sources are rejected by Scala 3.9.0. The unrelated extractor,
foreign same-spelled extractor, and non-`Event` variant first produce
unreachable/exhaustivity warnings, followed by hard unreachable-case errors;
the report keeps those warnings distinct from the compilation result. The
existing issue #1992 fixture also retains its accepted-with-warning controls
for duplicate variants, non-exhaustive matches, and wrong alias ownership.

## Reproduction

Run from the repository root with `SCALA39_ROOT` at the pinned checkout and
`JAVA_HOME` at Temurin `21.0.8+9`:

```sh
crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/run
crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/run-rollback-matrix
cargo +1.98.1 test -p dotty-core symbols::scope::tests::remove_drops_the_name_bucket_after_its_last_symbol --locked
cargo +1.98.1 test -p dotty-typer --test source_case_class_match sealed_event_case_local_negative_matrix_rejects_whole_methods --locked -- --nocapture
cargo +1.98.1 test -p dotty-typer --test source_case_class_match sealed_event_rejects_forged_companion_and_selector_ownership --locked
cargo +1.98.1 test -p dotty-typer --test source_case_class_match precompleted_source_parent_graph_failures_roll_back_and_retry_in_both_routes --locked
cargo +1.98.1 test -p dotty-typer --lib issue_1999 --locked
```

The run scripts verify Scala revision `777528f19a58e794c9954a42f433373472ec57f8`,
the eight existing artifact hashes, and the exact JDK release before compiling.
The canonical `Event.scala` SHA check is part of the rollback oracle runner.
Existing Box 2/2, Product2 2/2, closures 6/6, inherited members 4/4, and
protected 16+15 controls remain in their existing focused tests and workflows.
