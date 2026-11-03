# Semantic TASTy convergence III: integration receipt

**Verdict: PARTIAL.** The integrated positive and regression suites pass locally
on base `9341a1138326c6636af53dda0c28be13f1fab65c` with Temurin JDK 21.0.13.
The required hosted checks on this PR head and an authentic nonempty trace
receipt for execution run `4d88b28e-40a6-439c-8c52-398221815884` are not yet
available. This report does not claim `SPRINT GOAL MET` until both are attached
to one candidate. No production semantic code changed for C1.

## Frozen inputs

- Integrated main: `9341a1138326c6636af53dda0c28be13f1fab65c` (merged #2055).
- Scala source revision: `777528f19a58e794c9954a42f433373472ec57f8`.
- Source-generated TASTy: 28.9 for the five acceptance inputs below. The
  repository's compatibility policy retains previously frozen 28.8 inputs in
  the historical cohort; no fixture was regenerated or substituted.
- JDK: Temurin `21.0.13+9-LTS`; `java.base.jmod` SHA-256
  `fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`.
- The class receipts select the repository's `tasty_visibility` and semantic
  fixture directories. The method receipts select the original `Animal.tasty`,
  `Dog.tasty`, and `Predef.tasty` fixtures and resolve `java.lang.String` from
  `java.base.jmod`. Their individual SHA-256 values are in
  [acceptance.tsv](acceptance.tsv).
- `Predef.tasty` SHA-256:
  `2faef7420109c4e44074ab0385dce2c31e751fd992621fd1de312eed6c362fb2`.

## Positive acceptance

The non-ignored unit test
`loader::tests::semantic_adapter_loads_one_plain_int_constructor_between_visibility_roots`
calls the same crate-private semantic classloader operation for `Open`,
`CtorPlain`, and `PlainPrivate` in that mixed order, twice in independent fresh
stores. It checks class kind, exact package owner and visibility, canonical
`Object` parent, selected resource origin, constructor ownership and result,
same-session cache identity, and CtorPlain's one regular clause with the
canonical bootstrapped `scala.Int` formal. It compares normalized roles across
stores rather than arena IDs. The 3-class receipt is in [acceptance.tsv](acceptance.tsv).

The non-ignored test
`original_animal_and_dog_sound_methods_resolve_predef_string_target` explicitly
completes `Animal.sound@22` and `Dog.sound@36` from their original indexes. It
asserts the source-owned Type-namespace alias on the canonical Predef
ModuleClass, distinct alias and target identities, by-name method signature,
canonical JDK class identity and origin, and deterministic fresh-store receipts.
The unchanged historical four-method receipt remains separate. The 2-method
receipt is in [acceptance.tsv](acceptance.tsv).

## Rejection, rollback, and regression map

| Requirement | Existing required coverage |
| --- | --- |
| Missing Predef or `java.lang.String` dependency | `missing_predef_and_jdk_string_resources_fail_typed_without_partial_method_state`; asserts typed error address and method remains incomplete. |
| Malformed module TASTy | `resolver::tests::malformed_module_tasty_errors_without_retaining_identity_or_package_changes`; a malformed selected module resource leaves package and module identity checkpoints unchanged. |
| Missing or malformed JDK String class | `resolver::tests::predef_string_target_missing_or_corrupt_rolls_back_and_retries_in_session`; checks store, resolver and package checkpoints, then retries after restoring the real JMOD entry. |
| Foreign alias owner or a same-spelled foreign type | `resolver::tests::predef_module_scope_exposes_the_source_indexed_string_alias`; validates canonical indexed owner and rejects a foreign same-spelled type. |
| Direct class/type alias name collision | `resolver::tests::complete_module_rejects_a_direct_string_class_alias_collision`. |
| Wrong class package/owner, module root, generic constructor, `CtorWithVal`, or multiple term clauses | `loader::tests::semantic_class_loader_rejects_wrong_package_and_module_before_mutation`; exact typed shape errors and unchanged store/session/package checkpoints. |
| `CtorWithVal` field/accessor neighbor | `authentic_plain_constructor_is_completed_repeatably_and_val_neighbor_is_distinct` and the loader shape rejection above; the header field and constructor formal remain separate identities. |
| Secondary constructor | `loader::tests::semantic_class_loader_rejects_secondary_constructor_with_rollback`, run by the required pinned Scala consumer job with `DOTTY_REQUIRE_SCALA39_SECONDARY_CONSTRUCTOR_TEST=1`. This test returns early without that workflow environment and is not counted as locally executed. |
| Invalid TASTy minor version | The classloader TASTy parser uses the Scala 3.9 validation path; `tasty_symbol::tests::compiler_package_alias_rejects_unvalidated_tasty_minor_versions` exercises the unvalidated-version rejection. |
| Late constructor failure and same-store retry | `loader::tests::semantic_plain_int_constructor_rollback_preserves_prior_root_and_retries`; checks store, package, resolver/cache state and preserves the previously committed Open root. |
| Late alias target failure and retry | `enclosing_tasty_rollback_restores_predef_target_and_allows_retry` and the missing/corrupt JDK resolver test. |
| Malformed class TASTy and wrong requested binary name | `authentic_regular_constructor_is_accepted_by_semantic_and_legacy_loaders`; malformed, missing and wrong-owner requests are typed and do not publish semantic cache entries. |
| Legacy loader default and TASTy-first selection | `authentic_regular_constructor_is_accepted_by_semantic_and_legacy_loaders`; directory resource precedence is also checked by the existing `prefers_a_tasty_entry_over_a_class_entry_for_the_same_name` regression. |

The focused tests below ran successfully, including the historical
14-unit/126-row comparator and the original four-method comparator. Historical
files are byte-for-byte unchanged from the integrated base: `cohort.tsv`
SHA-256 `bfa365d830a627ce46bb5682a93cad90e26dbc9a90ead76bcaa65a48539a8a95`,
`comparison.tsv` SHA-256
`34097e61b691ff6ee9c16089336c495decc31241a10e448c1a1776849888aa4e`,
`comparison-1948.tsv` SHA-256
`a4b14bdf2ba921363358f568b8bce636e0e24453c2a3c6e57bd61478c231e6e0`, and
the historical convergence-II member receipt SHA-256
`5bc658a2c13e35c7751b5ab3a29ed6f0c842b0d0ef1e7bb6468c141a7ad30c61`.
The new two-method result is an additive receipt and does not relabel old rows.

## Validation run

Local environment: Rust `1.98.1`, Temurin JDK `21.0.13+9-LTS`,
`JAVA_HOME=/home/vdrake6/.sdkman/candidates/java/current`, with
`$JAVA_HOME/bin` on `PATH`. The build target directory was outside the source
tree. These checks ran against the unchanged integrated main SHA above:

- `cargo fmt --all -- --check` — passed.
- `python3 tools/check-frontend-boundaries.py` — passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` — passed.
- `cargo test -p dotty-classloader --lib --locked` — 520 passed, 4 ignored.
- `cargo test -p dotty-classloader --test semantic_tasty_parity --locked` — 10 passed.
- `cargo test -p dotty-classloader --test semantic_tasty_member_completion --locked` — 8 passed.
- Strict JDK 21 gate from `AGENTS.md`, with
  `DOTTY_CLASSFILE_REQUIRE_JDK=1` — passed, including the classfile encoder,
  JVM backend, source compilation, and stateful class contract suites.
- `cargo test --workspace --all-targets --locked -- --test-threads=1` — passed.
  The first default-parallel attempt hit the host file-descriptor limit during
  JDK classpath setup; the serial required suite then passed. No suite failures
  were suppressed.
- `git diff --check` — to be recorded on the final PR candidate.

The pinned Scala secondary-constructor workflow and the full hosted required
CI jobs still need successful runs on this PR candidate. Their authentic run
URLs and exact run head SHA will be added here only after GitHub reports them.

## Provenance and limits

The trace store contains the parent run manifest and a scope-phase event only;
it has no nonempty tool-call trace for this implementation run. Shell
correlation was unavailable. No trace link, event count, or usage total is
inferred from that partial record. The execution therefore remains `PARTIAL`
until authentic tracing and required hosted checks are available.

The acceptance is limited to these three class roots, two original indexed
methods, their listed controls, and their existing loader paths. It does not
establish general Scala library completion, arbitrary aliases, other
constructor forms, combined Scala ABI output, or a complete source compiler.
