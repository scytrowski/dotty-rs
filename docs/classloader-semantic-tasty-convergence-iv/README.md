# Semantic TASTy convergence IV closeout

## Verdict

Functional acceptance: **MET** on PR head
`016a7a843de900fb45dd7b5274c23f65fce2e93c`. All four pinned class roots pass
through the same private semantic loader on both required orders and fresh
stores; the selected `Open` resolver route passes; negative rollback and
historical compatibility checks pass. Agent tracing: **BLOCKED / UNAVAILABLE**
because the original issue-level tracer telemetry is not observable in this
report. Sprint verdict: **PARTIAL**, solely because the requested tracer
provenance remains unavailable. This does not change the functional verdict or
imply a compiler correctness failure.

This closeout integrates the separately reviewed immutable-field and explicit
resolver-request increments. It adds no production semantics. Public/default
`ClassLoader::load_class` and ordinary resolver requests remain on the legacy
path.

## Integrated baseline and prerequisites

This branch starts from refreshed `main` SHA
`e309e1f950ffe9dc0cda25647086f64f6c9b7b60`. Each prerequisite had a final
`READY_FOR_MERGE` review, is merged, and its merge commit is an ancestor of the
baseline:

| Stream | Issue / PR | PR head | Merge commit |
| --- | --- | --- | --- |
| A1 field audit | #2059 / #2083 | `3ffd1f005b66bfa8c8a1ef68ed7d04327821bc71` | `481ddef0527d5a40e065b23102641c265e912dc4` |
| B1 resolver audit | #2060 / #2087 | `87505646fc0688a578f35ca2de937392f84f997a` | `2535e4c8d3f3f40c3c65db48a05ef3c9ebec5c6e` |
| A2 field projection | #2062 / #2090 | `9d0ade991128292355c263c1fd5ae9c580143381` | `0c57f2eff55ca502181a36d4ed15f941a6426cb4` |
| B2 resolver opt-in | #2064 / #2106 | `dd4136afbd34b721372970d590501007faba5335` | `ad02dbfd6e18cb5aa86a78f8f0faaa3ff60b0224` |
| A3 whole-class field | #2067 / #2111 | `05c92b672739e935e7d8b76165971df8b0c8661e` | `36711d0bac27af5956f42e0be7bfc09699ef0d8a` |

## Frozen inputs and classpath

Scala source compatibility is pinned to revision
`777528f19a58e794c9954a42f433373472ec57f8`, Scala 3.9.0, TASTy 28.9.
Rust is `1.98.1 (48a229cea 2026-09-01)`. The local runtime is Temurin
`21.0.13+9-LTS`; real JDK input is `$JAVA_HOME/jmods` and `java.base.jmod`
SHA-256 is `fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`.

| Input | Repository path | SHA-256 |
| --- | --- | --- |
| `Constructors.scala` | `crates/dotty-tasty-unpickler/tests/fixtures/semantic/Constructors.scala` | `ee054964fa24c1e2084350b257b0875d2c9c38038b73750b7eefb12b70ffcd63` |
| `CtorWithVal.tasty` | `crates/dotty-tasty-unpickler/tests/fixtures/semantic/CtorWithVal.tasty` | `53d335f857224d38559335eaf1efba92006a55ad9ed8e74d1f58a9e77f8a0192` |
| `CtorPlain.tasty` | `crates/dotty-tasty-unpickler/tests/fixtures/semantic/CtorPlain.tasty` | `0d2f01cc5444bda30ae6a920162b3eb89fd2a5a7aefc84a1e22bef8a35120ddb` |
| `Open.tasty` | `crates/dotty-classloader/tests/fixtures/tasty_visibility/me/cytrowski/tastyfixtures/visibility/Open.tasty` | `49286e74a3d3693b08d2a29575f23089f6be5c486231390059f43c19a8929894` |
| `PlainPrivate.tasty` | `crates/dotty-classloader/tests/fixtures/tasty_visibility/me/cytrowski/tastyfixtures/visibility/PlainPrivate.tasty` | `db045c091f6bc71cdee1fef6c81cd09fe1f520f740f09fec008dd5f1fd98c4ba` |
| `Predef.tasty` | `crates/dotty-tasty/tests/fixtures/scala3-library/scala/Predef.tasty` | `2faef7420109c4e44074ab0385dce2c31e751fd992621fd1de312eed6c362fb2` |
| `Animal.tasty` | `crates/dotty-classloader/tests/fixtures/tasty_sample/me/cytrowski/tastyfixtures/Animal.tasty` | `9f205fe92b667f4f83a6a3d03d6dd45ec8f22f979e5acd73608e75ba1f2af9b3` |
| `Dog.tasty` | `crates/dotty-classloader/tests/fixtures/tasty_sample/Dog.tasty` | `9fb989680c0fbc7811ca1343437f223f4fd60b368d95fc8a0e1fbc49023cd13a` |
| `java.base.jmod` | `$JAVA_HOME/jmods/java.base.jmod` | `fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91` |

The four-root order test uses the semantic constructor fixture first, then the
visibility fixture directory, then real JDK 21 `jmods`; Open and PlainPrivate
are selected from the visibility directory, and CtorPlain/CtorWithVal from the
semantic fixture path. The selected resolver request uses the visibility
directory followed by `jmods`. The supplemental member audit orders
`tasty_sample`, Scala library fixtures, path-dependent fixtures, semantic
fixtures, then `jmods`; Animal/Dog and Predef resolve from their recorded
original paths above. The accepted class roots and exact resource hashes are in
[`acceptance.tsv`](acceptance.tsv).

## Acceptance and negative evidence

The TSV has exactly four class roots plus one separately labeled resolver
opt-in proof. Existing tests exercise both `CtorPlain`/`CtorWithVal` orders in
fresh stores, same-loader repeats, origin/cache/repository identity, canonical
Object and Int, class/package and constructor/field/formal ownership, and the
distinct indexed `CtorWithVal` field and formal. They compare normalized roles,
not numeric IDs across stores.

Negative controls cover malformed or missing TASTy, unsupported shapes and
minor versions, wrong package/owner, duplicate scope identity, unsupported
fields/constructor forms, missing real Object, resolver collision, late
rollback, and same-session retry. Selected resolver failures remain typed and
never fall back to legacy. A source-authentic mutable-field fixture is not
available; mutable flags are tested by structural substitution. The source
generated secondary-constructor control runs in the existing pinned Scala/JDK
consumer workflow with `DOTTY_REQUIRE_SCALA39_SECONDARY_CONSTRUCTOR_TEST=1`.

Before #2067, the first CtorWithVal rejection was
`UnsupportedShape("the primary constructor must have one empty clause or one ordinary Int clause")`;
the integrated loader now accepts that audited shape. Malformed selected
TASTy remains `ClassLoad(InvalidTastyFile)`. At the resolver trait boundary,
the typed semantic error maps to existing `ResolutionError::Malformed` only
after rollback.

## Historical baselines (unchanged)

| Artifact | Scope | SHA-256 |
| --- | --- | --- |
| convergence-I `cohort.tsv` | 14 TASTy units | `bfa365d830a627ce46bb5682a93cad90e26dbc9a90ead76bcaa65a48539a8a95` |
| convergence-I `comparison.tsv` | historical class-root report | `34097e61b691ff6ee9c16089336c495decc31241a10e448c1a1776849888aa4e` |
| `comparison-1948.tsv` | 126 rows | `a4b14bdf2ba921363358f568b8bce636e0e24453c2a3c6e57bd61478c231e6e0` |
| convergence-II `member-completion.tsv` | original four-method attempt | `5bc658a2c13e35c7751b5ab3a29ed6f0c842b0d0ef1e7bb6468c141a7ad30c61` |
| convergence-III `acceptance.tsv` | earlier 3-class / 2-method receipt | `70e3dca0e4e32c82ef83912291ac971e2894bcd2af04451107a42e540bf14fd3` |
| `animal-dog-method-acceptance.tsv` | supplemental Animal/Dog 2/2 result | `9e6e671db1daa1cd90d8c6b9b1b2a90b9b46af072c96e753d9d76f379bc5c8ae` |

The four-method rows remain Option.getOrElse and InfoWrapped.m complete, and
Animal.sound / Dog.sound unresolved at original addresses 27 and 41. The
separate supplemental report retains Animal/Dog 2/2 completion. The earlier
three-class baseline remains Open, PlainPrivate, and CtorPlain; CtorWithVal is
additive.

## Reproduction and CI

Focused semantic checks and both frozen comparators are run twice on this
checkout; each comparator itself also compares two fresh normalized runs.
Workspace temp/build paths are isolated because shared `/tmp` is full.

```sh
export JAVA_HOME=/home/vdrake6/.sdkman/candidates/java/current
export PATH="$JAVA_HOME/bin:$PATH"
export TMPDIR="$PWD/target/issue-2070/tmp"
export CARGO_TARGET_DIR="$PWD/target/issue-2070/cargo"
export JAVA_TOOL_OPTIONS="-Djava.io.tmpdir=$TMPDIR -XX:-UsePerfData"
mkdir -p "$TMPDIR"
cargo test -p dotty-classloader --lib semantic_adapter_loads_one_plain_int_constructor_between_visibility_roots --locked -- --nocapture --test-threads=1
cargo test -p dotty-classloader --lib authentic_ctor_with_val_loads_and_unsupported_neighbors_preserve_state --locked -- --nocapture --test-threads=1
cargo test -p dotty-classloader --lib explicit_semantic_open_request_reuses_identity_and_rolls_back_cleanly --locked -- --nocapture --test-threads=1
cargo test -p dotty-classloader --test semantic_tasty_parity frozen_cohort_is_complete_and_both_fresh_adapter_runs_are_deterministic --locked -- --nocapture --test-threads=1
cargo test -p dotty-classloader --test semantic_tasty_member_completion four_frozen_members_are_explicitly_completed_in_fresh_deterministic_runs --locked -- --nocapture --test-threads=1
```

Repeat the two comparator commands and recheck historical hashes. Required
checks are workspace format, frontend boundaries, Clippy, rustdoc, locked
workspace tests, strict JDK 21 verification and `git diff --check`. The existing
pinned Scala/JDK consumer workflow is a required gate.

Local checks ran against frozen integrated source baseline
`e309e1f950ffe9dc0cda25647086f64f6c9b7b60`. Hosted checks below ran on PR head
`0651ba9b899c5349f25d76f56cf9550de19ed094`; that PR commit adds only this
closeout report and the workflow path trigger, with no production changes.

- focused four-root sequence and selected resolver acceptance — passed;
- CtorWithVal positive/unsupported-shape regression — passed;
- frozen 14-unit/126-row comparator — passed twice;
- original four-method comparator — passed twice;
- full `dotty-classloader` and `dotty-tasty-unpickler` suites — passed;
- format, frontend boundary, workspace Clippy and rustdoc checks — passed;
- locked workspace all-target tests — passed;
- mandatory JDK 21 gate with `DOTTY_CLASSFILE_REQUIRE_JDK=1` — passed;
- `git diff --check` — passed.

| Required hosted check | Result | Tested PR head | Run |
| --- | --- | --- | --- |
| Rust checks (format, frontend boundaries, Clippy, rustdoc, workspace tests, diff) | passed | `0651ba9b899c5349f25d76f56cf9550de19ed094` | [job 126515157285](https://github.com/scytrowski/dotty-rs/actions/runs/41812556451/job/126515157285) |
| Classfile/JVM encoder gate | passed | `0651ba9b899c5349f25d76f56cf9550de19ed094` | [job 126515158913](https://github.com/scytrowski/dotty-rs/actions/runs/41812556451/job/126515158913) |
| Pinned Scala 3.9 consumer and strict TASTy gates | passed | `0651ba9b899c5349f25d76f56cf9550de19ed094` | [job 126515155853](https://github.com/scytrowski/dotty-rs/actions/runs/41812556499/job/126515155853) |
| Rust checks (review-fix evidence rerun) | passed | `016a7a843de900fb45dd7b5274c23f65fce2e93c` | [job 126530617177](https://github.com/scytrowski/dotty-rs/actions/runs/41817093455/job/126530617177) |
| Classfile/JVM encoder gate (review-fix evidence rerun) | passed | `016a7a843de900fb45dd7b5274c23f65fce2e93c` | [job 126530485872](https://github.com/scytrowski/dotty-rs/actions/runs/41817093455/job/126530485872) |
| Pinned Scala 3.9 consumer and strict TASTy gates (review-fix evidence rerun) | passed | `016a7a843de900fb45dd7b5274c23f65fce2e93c` | [job 126530657193](https://github.com/scytrowski/dotty-rs/actions/runs/41817142961/job/126530657193) |

Both pinned Scala runs completed the required source-generated secondary-
constructor rejection step successfully. Review-fix SHA `016a7a843de900fb45dd7b5274c23f65fce2e93c` changes this closeout README only;
the implementation and acceptance tests are unchanged. The current PR is
[#2114](https://github.com/scytrowski/dotty-rs/pull/2114).

## Trace provenance

The original issue-level tracer result remains **BLOCKED / UNAVAILABLE**; this
report contains no original trace artifact and prerequisite PR traces are not
substitutes. Functional acceptance is **MET**. The sprint verdict is **PARTIAL**
because tracer provenance remains unmet. Missing telemetry is not treated as a
compiler correctness failure and is not reported as complete.
