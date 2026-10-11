# Binary error model compatibility

This report closes the cross-crate regression gate in issue #981 for the
structured TASTy unpickler errors (#977/#978) and classloader errors
(#979/#980). It checks failure semantics and error reporting against the same
fixtures; it does not claim that every valid Scala or JVM artifact is
supported.

## Comparison setup

| Input | Before | After |
| --- | --- | --- |
| Source revision | `2cd57c1b9` (before #977/#979) | `e641f2da8` (issue #981 regression assertions) |
| Scala TASTy corpus | Pinned Scala 3.9.0: `777528f19a58e794c9954a42f433373472ec57f8`; 941 library and 1,148 compiler files | Same checked-in fixture inventory and manifest hashes |
| Corpus command | `cargo test -p dotty-tasty-unpickler --features corpus --release --locked -- --ignored --nocapture` | Same command |
| Focused crate commands | `cargo test -p dotty-tasty-unpickler --locked --quiet`; `cargo test -p dotty-classloader --locked --quiet` | Same commands |
| Environment | Linux x86_64; Rust/Cargo 1.98.1 | Same environment |

The baseline was unpacked from the Git archive of `2cd57c1b9` into a clean
temporary directory. The two focused crate suites passed on both revisions.
All five ignored, feature-gated corpus reports passed on both revisions. After
normalizing the temporary workspace path and excluding harness timing and
filter counts, their report lines were identical. The test-only additions
change the number of unit tests, as expected; corpus outcomes did not change.

Notable identical corpus measurements include:

| Report | Identical before and after |
| --- | --- |
| Companion pairing, library corpus | 575 of 1,942 class/trait identities paired; 575 of 944 objects; 274 nested pairs; 0 failed units; 0 order-dependent differences |
| Companion pairing, compiler corpus | 1,191 of 2,327 class/trait identities paired; 1,191 of 2,170 objects; 743 nested pairs; 0 failed units; 0 order-dependent differences |
| Definition-tail scan, both corpora | 2,089 files; 0 parse failures; 0 symbol-entry failures |

The remaining opaque-alias, symbol-annotation, and type-pass reports also had
identical normalized measurements and zero unexpected errors. Their ignored
test targets are `opaque_corpus`, `symbol_annotation_corpus`, and
`type_corpus`; the symbol-annotation target was enabled with the `corpus`
feature.

## Failure matrix

The “before” column records the outcome exposed by the existing structural
tests at `2cd57c1b9`. At that revision, stable error codes/categories and
classloader artifact provenance were not available. “After” records the same
failure family and its added machine-readable metadata. Inputs are synthetic
wire/class data or the checked-in Scala/JVM fixtures named by the tests.

| Case | Before | After | Regression evidence |
| --- | --- | --- | --- |
| Valid Scala 3.9 TASTy fixture projection and symbol entry | Succeeds with the same indexed symbols and types | Same successful projection; no error metadata applies | `fixture_corpus::every_small_fixture_enters_without_error_and_declares_at_least_one_symbol`, semantic fixture tests, and the five corpus reports above |
| Malformed local binder reference | `InvalidBinderReference`; rejected | Same variant; `unpickler.invalid_binder_reference`, `MalformedInput` | `binders::a_parameter_type_naming_no_node_is_an_invalid_binder_reference` |
| Invalid recursive shared reference | `InvalidReferenceTarget`; rejected | Same variant and address pair; `unpickler.invalid_reference_target`, `Conditional` because this payload also represents bounded cycle/depth cases | `recursive::a_rec_this_naming_no_node_is_an_invalid_reference` |
| Missing external member | `UnresolvedMember`; returned after one explicit owner-space lookup | Same variant; `unpickler.unresolved_member`, `ResolutionFailure`; no prefix retry | `in_reference::a_resolver_that_does_not_know_the_declaration_leaves_it_unresolved_without_a_prefix_retry` |
| Unsupported but readable TASTy shape/tag | `UnsupportedType`/unsupported AST variant; rejected instead of silently lowered | Same unsupported failure family, with a stable code and `UnsupportedFeature` classification | `annotated`, `recursive`, and `error::nested_decode_causes_keep_malformed_and_unsupported_categories_distinct` |
| Failed unpickler operation followed by retry | Failed transaction leaves no newly indexed trace; retry behavior is unchanged | Same rollback and retry; code/category queries do not dereference IDs invalidated by rollback | `in_reference::a_failure_after_both_children_decoded_leaves_no_trace_and_a_retry_works`, `lambda_tpt::a_malformed_lambda_fails_the_whole_enter_and_leaves_the_store_untouched`, and `error::classification_and_codes_do_not_dereference_rolled_back_ids` |
| Malformed class bytes from a directory | `InvalidClassFile`; rejected | Same variant; `classloader.invalid_class_file`, `MalformedArtifact`, directory origin and `Class` format | `directory_class_path::detailed_load_failure_retains_directory_resource_provenance` |
| Malformed class bytes from a JAR or JMOD | `InvalidClassFile`; rejected | Same variant; stable code/category and selected JAR/JMOD origin and `Class` format | `loader::detailed_failure_in_a_jar_retains_the_jar_origin` and `loader::detailed_failure_from_a_jmod_retains_the_jmod_origin` |
| Unsupported JVM class-file version | `UnsupportedClassVersion`; rejected | Same variant; `classloader.unsupported_class_version`, `UnsupportedFeatureOrVersion`, selected resource origin | `loader::detailed_unsupported_version_retains_selected_class_resource` |
| Malformed TASTy resource | `InvalidTastyFile`; rejected | Same variant; typed TASTy cause and selected TASTy origin/format | `loader::detailed_tasty_failure_retains_the_selected_tasty_resource` and `loader::loading_source_reports_a_malformed_package_alias_tasty_file` |
| Missing class / classpath I/O error | `NotFound` / `Io`; returned | Same error variants; stable classloader/classpath codes and categories; no fabricated resource origin | `loader::detailed_missing_failure_has_no_fabricated_provenance`, `loader::detailed_classpath_io_failure_has_no_resource_provenance` |
| Valid class after an unrelated failed load | A failed class is cached independently; a later class can still load | Same behavior; valid class becomes `Loaded` while the earlier class remains `Failed` | `loader::a_successful_load_after_an_independent_failure_is_unaffected` |
| Dependency fails in a different artifact | Outer `DependencyFailure` retains its nested source; failed dependency artifact was not available from the public API | Same nested cause identity; detailed result carries the failing dependency origin when known and preserves `None` for missing dependencies | `loader::detailed_dependency_failure_reports_the_dependency_artifact_origin`, `loader::detailed_dependency_failure_reports_the_missing_dependency_without_owner_origin`, and `loader::nested_dependency_alias_failure_preserves_cached_provenance_without_io` |
| Repeated cached failure / inheritance cycle | Same cached error and cycle rejection | Cached code, root cause, origin and format are stable without another classpath lookup | `loader::detailed_cached_failure_preserves_resource_provenance_without_reprobing`, `loader::circular_inheritance_retains_resource_context_in_cached_failures` |

Classfile, JAR, JMOD, and TASTy fixtures remain the same bytes used by the
existing crate tests. The JMOD provenance test wraps the checked-in JMOD
classpath and substitutes malformed class bytes while preserving the selected
resource's origin; it therefore tests loader provenance without depending on a
JDK installation or modifying the fixture archive.

## Limits and compatibility notes

- `java` and `javac` are not installed in the comparison environment. The
  checked-in JAR/JMOD fixtures and classloader tests ran; the optional
  `DOTTY_JDK21_HOME` real-JDK test was not run.
- No new unsupported semantic form surfaced during this regression pass. The
  existing taxonomy documents already identify conditional classifications
  where an error payload cannot distinguish malformed input from a valid
  construct beyond current depth/support limits.
- Binary error APIs remain `Result<_, UnpickleError>` and
  `Result<_, ClassLoadError>` (with the additive detailed classloader API).
  These errors retain TASTy addresses or classpath origin as appropriate;
  neither creates a Scala source `SourceSpan` or frontend `Diagnostic`.
- No `dotty-typer` source files were changed. Workspace validation below
  compiles and tests downstream crates as part of CI checks.
