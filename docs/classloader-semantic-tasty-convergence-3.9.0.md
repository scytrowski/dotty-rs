# Scala 3.9.0 semantic TASTy convergence baseline

Status: input cohort frozen before results. The original pre-session-contract
run is retained as historical evidence, not as a parity measurement. The
measurement below records the #1945 baseline before the bounded #1948 package
`TermRef` resolver change documented at the end of this file.

## Reproduction environment and policy

- Frozen cohort selection baseline: `2894d8c969e0d210b3f461d35e08d55daf6b2227`.
- Refreshed execution baseline after prerequisite #1940: `7222d2d856aa7b7cab667c9e1e30489c700164cd`.
- Rust: `rustc 1.98.1 (48a229cea 2026-09-01)`.
- JDK: Eclipse Temurin `21.0.13+9` (the Rust checks workflow installs this
  version so the real-JDK semantic audit has its declared classpath input).
- Scala reference: 3.9.0 source revision
  `777528f19a58e794c9954a42f433373472ec57f8`.
- TASTy inputs: the legacy classloader validates and accepts 28.8.0 and
  28.9.0; this cohort records the minor version per frozen input. The
  original freeze mislabeled the eight classloader fixtures as 28.8. A
  pre-measurement header check corrected only those version fields to 28.9;
  all original unit IDs, paths, hashes, requested shapes, ownership rationale
  and cohort order are unchanged. No adapter result informed this correction.
- Bootstrap policy: independently create a fresh `SemanticStore` per adapter
  and per run, call `Definitions::bootstrap` once, use the same ordered
  composite classpath for both adapters, and give the unpickler a fresh
  `TastySession` with the existing `ClasspathSymbolResolver`. This exercises
  the resolver-backed session contract added by #1940. Do not add semantic
  stand-ins for absent external dependencies; classify those resolutions
  explicitly.
- Cohort was selected and hashed before adapter results were collected. The
  exact input paths, hashes, requested shapes, and rationale are in
  [`cohort.tsv`](classloader-semantic-tasty-convergence/cohort.tsv).
- The comparison TSV schema is frozen in the #1945 historical report at
  [`comparison.tsv`](classloader-semantic-tasty-convergence/comparison.tsv).

## Comparison contract

For each selected unit, report owner path, kind, name, visibility, flags,
class hierarchy, ordered type parameters and parameter clauses,
method/result/field type structure, companion relationship, and only the
annotation forms explicitly supported by both adapters. Polymorphic
parameters retain their order, bounds, and declared variance. Fixture
provenance is repository-relative so fresh checkouts produce the same report.
Normalize type and symbol identities within each store to owner-qualified
paths and semantic shape. Never compare numeric IDs or TASTy addresses across
stores, and never resolve production identities by rendered names. Each field
receives exactly one outcome: `MATCH`, `SEMANTIC_DELTA`, `LOADER_UNSUPPORTED`,
`UNPICKLER_UNSUPPORTED`, `MALFORMED_OR_VERSION`, or `UNRESOLVED_EXTERNAL`.
Retain the first underlying error and adapter provenance. An absent or
unknown value is not `Any`, `Object`, success, or structural equality.

The audit test validates manifest uniqueness, fixture hashes, format versions
and root coverage, and emits normalized TSV twice from fresh sessions. The
checked-in TSV is the measurement from the refreshed execution baseline;
execution fails on missing/duplicate keys, skipped units, malformed rows, or
outcome mismatch. The prior resolver-free output is retained only as historical
evidence and is not used as parity evidence.

## Refreshed measurement and scope gate

The 2026-11-01 execution used Rust `1.98.1`, refreshed main
`7222d2d856aa7b7cab667c9e1e30489c700164cd`, and
`JAVA_HOME=/home/vdrake6/.sdkman/candidates/java/current`. The real
`jmods/java.base.jmod` input had SHA-256
`fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`. The
same ordered composite classpath (three classloader fixture roots, the pinned
Scala library fixtures, the path-dependent fixture root, then the JDK) was
given independently to the legacy loader and a resolver-backed `TastySession`.
Each adapter used its own fresh store and one bootstrap. The unpickler resolver
was carried through session handoffs; no semantic stand-ins were inserted.

The original single-unit candidate `visibility-open` and two-unit candidate
`animal` + `dog` were retained. The legacy loader returned all nine fields for
`visibility-open`; all nine unpickler fields stopped at its first actual
`UnresolvedMember` (`java.lang.Object`, TASTy address 15). For both `animal`
and `dog`, the unpickler likewise stopped at the first `UnresolvedMember`
(`java.lang.Object`, addresses 9 and 15). The legacy loader completed the
hierarchy for both units, while each had one explicitly unsupported member
signature caused by the unresolved `scala/Predef/String` reference. Thus the
single-unit and two-unit cases do not qualify as interoperable integration
candidates under the frozen contract.

Across all 14 frozen rows, 13 files have a matching entered root definition;
the `visibility-holder` file has no root named `Holder` in either adapter.
This leaves more than the required 12 real root units without substituting a
candidate. The remaining cohort also yields explicit unsupported or unresolved
outcomes, including `scala.Nothing`, `scala.Int`, and the applied `scala.Tuple2`
parent shape. The per-field report contains 126 rows and is byte-identical
across two fresh sessions. During review correction, the report was regenerated
with repository-relative provenance, class type parameters from the ordered
constructor `PolyType` vector before member sorting, and polymorphic method
parameter bounds plus declared variance. The normalized comparison TSV SHA-256
is `34097e61b691ff6ee9c16089336c495decc31241a10e448c1a1776849888aa4e`. The
pre-correction resolver-backed raw trace is retained as historical evidence
with SHA-256
`1b4280c931e0d53e8d4fd03c8e4e2fc4934d14a34491013c7e55b9982983b2ac`.

Neither frozen integration candidate qualifies, as permitted by the issue's
DoD when the report explains why no candidate qualifies. The negative controls
reject a one-byte truncation of a real TASTy fixture, confirm a missing JDK
dependency is explicit and rolls back classloader semantic allocations, then
successfully load an unrelated real fixture with a complete classpath in the
same store. The existing unpickler identity regression
`references_to_same_named_classes_resolve_by_address_not_by_name` verifies two
real `Inner` classes remain owned by distinct `Left` and `Right` classes; the
unpickler's `unmodelled_types_are_explicitly_unsupported` regression covers an
unknown semantic type shape. The frozen `scala/Tuple2` applied-parent case
retains the legacy loader's `UnresolvedSupertype` and the unpickler's first
unresolved `scala.Nothing` reference, neither of which counts as equality or a
supported shape. The unpickler regression
`a_failed_decode_takes_back_everything_it_allocated` checks its store rollback.
The two-adapter matrix runs twice in independent fresh sessions and requires
byte-identical normalized output.
Making the frozen integration candidates complete would require changes to
external semantic resolution or its bootstrap contracts, beyond this audit's
tests/tooling/docs scope. The original pre-resolver run remains preserved at
`target/parity-run.txt` in the prior #1927 worktree with SHA-256
`61a7f0e069c46ee0b492da9e8b78ed7347f7439ab2f9f93afe7e2771a7c9c448`.
This boundary is also explicit in the
[`#1940 session contract`](classloader-semantic-tasty-session-contract.md),
which leaves external aliases and full semantic completion of the `Open` and
Animal/Dog graph unsupported by the resolver handoff.

## Package `TermRef` resolver follow-up (#1948)

The frozen `Open.tasty` bytes remain unchanged (SHA-256
`49286e74a3d3693b08d2a29575f23089f6be5c486231390059f43c19a8929894`). The
new classloader integration regression unpickles address 15 with the real
JDK jmods classpath and captures the resolver callback before any error
rollback. The request is the unique type member `Object` in prefix space. Its
prefix is a `TermRef` to the same canonical `java.lang` package symbol held by
the session package registry, and the resolver's prefix-classification
counter confirms it reaches `prefix_target_bounded`. Before the fix, this
request produced the exact typed failure
`UnresolvedMember { address: 15, name: "Object", namespace: Type }`.

The package `TermRef` and existing package `TypeRef` forms now select the same
package target. Address 15 resolves to the existing `Definitions::object_class`
identity, with complete `ClassInfo`, owner `java.lang`, and classfile origin
from the selected JDK resource. The regression also repeats the lookup through
a package `TypeRef` in the same resolver session and confirms the same symbol
identity. Missing Object bytes remain a clean unresolved result; malformed
Object bytes remain a typed resolver error. A failure injected after real
Object materialization restores the new `java.lang` package, the builtin
Object's missing info and builtin origin, then the same unpickler session
retries successfully with stable identity.

Local diagnostics used Eclipse Temurin `21.0.8+9-LTS` (`bin/java` SHA-256
`90d0a6d25da103678013f193db078b90ed8aeb27b66ebe78e0149616c9c1b811`;
`java.base.jmod` SHA-256
`7e05214a446061c5543f3732f653f514d4e05fa6fd2545aa792c205b24362791`). This
is recorded as local evidence only. The required Rust CI workflow installs
Eclipse Temurin `21.0.13+9`; the regression reads `JAVA_HOME/jmods`, so that
workflow supplies the exact audit JDK and classpath version. The historical
#1945 comparison TSV, frozen cohort, and prior measurements are unchanged.
The separate #1948 live comparison is checked in as
[`comparison-1948.tsv`](classloader-semantic-tasty-convergence/comparison-1948.tsv)
(SHA-256 `a4b14bdf2ba921363358f568b8bce636e0e24453c2a3c6e57bd61478c231e6e0`).
The live comparator masks store-local `TypeId` values in diagnostic text, and
a focused regression verifies equivalent failures with independently
allocated IDs produce identical normalized diagnostics. It records the
post-fix classloader/unpickler run over the same frozen inputs;
the Open address-15 Object dependency now resolves, while the report still
marks its type parameters and parameter clauses `UNPICKLER_UNSUPPORTED` and
Animal/Dog member signatures unresolved on `scala/Predef/String`. Resolving
this one Object reference does not establish semantic completion or candidate
qualification for the full `Open` / Animal / Dog cohort; the #1928 gate remains
separate.

## Private classloader adapter proof (#1928)

The opt-in, crate-private `ClassLoader::load_open_with_tasty_unpickler` path
uses the unchanged Scala 3.9.0 `Open.tasty` fixture
(`crates/dotty-classloader/tests/fixtures/tasty_visibility/me/cytrowski/tastyfixtures/visibility/Open.tasty`,
SHA-256 `49286e74a3d3693b08d2a29575f23089f6be5c486231390059f43c19a8929894`).
It resolves canonical `java.lang.Object` through the loader's configured
classpath and existing `ClasspathSymbolResolver`, carries that resolver's
`LoadingSession` back with `into_session`, then lends only the session's
`Packages` to `TastyUnpickler::with_packages`. The unpickler receives no
attached resolver. It enters the class at `TYPEDEF@4`, validates/completes its
class info, and completes the constructor at `DEFDEF@21`.

The serialized constructor result tree references `scala.Unit` at TASTy
address 25. Before completing the constructor, the adapter binds the existing
`Definitions::unit` symbol into the canonical `scala` type scope. The symbol
retains its bootstrap identity, `Class` kind, `Builtin` origin, and missing
class info; its owner is explicitly set to the `scala` package. The session's
symbol-update journal and package-scope undo journal make this binding part of
the same transaction as TASTy projection. An already committed canonical
binding is reused without adding another undo entry. The serialized address-25
reference resolves to that canonical Unit symbol. Constructor completion
validates the serialized result tree, then publishes the effective constructor
result as the owning `Open` class reference.

For this fixture the semantic adapter yields one public `Open` class owned by
`me.cytrowski.tastyfixtures.visibility`, one `java.lang.Object` parent, no
class type parameters, no companion or annotations, and one public zero-arg
constructor owned by `Open`. These fields match the legacy row in
[`comparison-1948.tsv`](classloader-semantic-tasty-convergence/comparison-1948.tsv):
its legacy constructor is `()Plain:Unit`. The adapter's constructor semantic
result is `Open`, as specified by constructor completion; its separately
validated serialized return tree is `Unit`. The default `load_class` path and
TASTy-first resource preference remain unchanged.

The adapter preflights the pinned unit shape before entering symbols: exactly
one package with one top-level class, rooted at `TYPEDEF@4`, with its
constructor at `DEFDEF@21`. A one-class unit rooted elsewhere, such as
`SymbolAnnotated.tasty` at `TYPEDEF@5`, is rejected with a typed
`UnsupportedShape` before any package-scope mutation. This prevents a
noncanonical root from being entered and then lost during store rollback.
After projection, a root that does not match the requested full binary name
(including package) is also rejected with a typed `UnsupportedShape` and rolls
back class/package entries, Object state, caches, origins, and the new Unit
binding. Missing resources, malformed TASTy, class-file resources, duplicate
legacy identities, and multi-declaration units are likewise rejected. The
supported boundary remains one self-contained class, its root and constructor
only; a type declaration already present in the canonical package scope is
also rejected before entry even when a fresh loader session has empty name
caches. Additional external semantic dependencies or graph expansion are not
resolved by this adapter.

## Issue #1929 fixture ledger

| Role | Fixture | SHA-256 | Root | Expected semantic edge |
|---|---|---|---|---|
| Parent | `crates/dotty-tasty-unpickler/tests/fixtures/semantic/InfoBase.tasty` | `e4d58a733e8cc9a516b5d248dd6fb97acb2cb19d763e1e390cfcd1249bd7f5f9` | `TYPEDEF@4`, constructor `DEFDEF@28`, `InfoBase[A]`, package `me.cytrowski.tastyfixtures.semantic` | No ordinary members |
| Child | `crates/dotty-tasty-unpickler/tests/fixtures/semantic/InfoWrapped.tasty` | `07cca416641dda27a333df99603bf7b119d0bc5660098c5d378a09064e1630fd` | `TYPEDEF@5`, constructor `DEFDEF@80`, `InfoWrapped[F[_]]`, same package | Applied parent `InfoBase[F[Int]]`; direct member `m` at `DEFDEF@115`, type `F[Int]` |

The pair has one nonbootstrap cross-unit TASTy dependency. JVM `Object` and
`Int` use existing canonical bootstrap/classpath behavior. `InfoBase` has no
ordinary members: this fixture demonstrates the applied parent edge and the
child's directly declared `m`; it does not demonstrate inherited-member
traversal.

The fixture source is pinned by the repository's Scala 3.9.0 semantic fixture
policy. The root, constructor, and member addresses above are original TASTy addresses;
tests must locate definitions through each unit's own retained semantic index.
The address ledger was checked against both fixture indexes: `InfoBase` is
`TYPEDEF@4` / `DEFDEF@28`, while `InfoWrapped` is `TYPEDEF@5` / `DEFDEF@80` /
`DEFDEF@115`. The semantic loader regression pins those per-unit addresses.

## Issue #1929 first-failure ledger

| Boundary | Expected behavior | Regression evidence |
|---|---|---|
| One-class `Open` adapter | Remains available as the existing #1928 opt-in regression | Existing `loader.rs` adapter tests |
| Two-unit opt-in integration | Enter both units before completing parent/member references, reusing canonical package and class identity | #1929 loader integration tests |
| Ordinary loader | Keeps legacy default TASTy output and resource priority | `tasty_loading.rs` and legacy parity tests |
| Pair resource selection | First classpath entry wins; a directory's TASTy resource wins over a competing class file | `info_pair_uses_first_tasty_resource_and_prefers_tasty_over_classfile` |
| Declared root identity | Verify the decoded package and root name against the requested binary name before either declaration is entered | `info_pair_rejects_mismatched_root_before_entering_preexisting_package` |
| Unit failure provenance | Keep failing binary name, selected `ClassOrigin`, `ClassFormat`, and nested typed cause/address | Base and wrapped malformed-resource tests; late child failure test |
| Unsupported or malformed unit | Return typed error and roll back only unpublished work | `info_pair_fails_closed_for_an_unsupported_parent_reference`; malformed and late-failure controls |
| Late second-unit failure and retry | Preserve a previously completed Object cache/origin and package/alias state; remove newly entered pair declarations and allow valid bytes to retry | `info_pair_late_child_failure_rolls_back_and_retries_with_original_bytes` |
| Constructor and parameter projection | Normalize both classes' type parameters and constructor parameter types in both request orders | Pair graph comparison: `A`/empty constructor and `F`/`x:Int` |
| Canonical Scala aliases during paired completion | Bind existing `Unit`, `Int`, `Any`, `Nothing`, and `Matchable` symbols; restore prior owners and package entries on failure | #1929 rollback control |

The pinned pair has one directed cross-unit edge and no cycle. The unsupported
dependency control mutates the child's parent reference to a same-length,
unavailable name and verifies the loader fails closed with the selected child
origin and no partial pair publication. A cyclic multi-unit fixture is outside
this one-edge contract; no general cycle traversal is claimed.

A first failure inside unpickler projection beyond the supported fixture
contract is a prerequisite boundary. Record its original file, address, and
error; do not broaden this loader increment into unrelated unpickler semantics.

## Deferred graph shape

The next graph increment is deferred. This issue covers only the two files and
single `InfoWrapped -> InfoBase` edge above. It makes no claim about general
classpath traversal, arbitrary dependency fan-out, or inherited-member lookup.

## Selected `Option.getOrElse` member projection (#1930)

The bounded classloader member decoder preserves the `BYNAMETPT` wrapper for
the frozen Scala 3.9 `Option.getOrElse` default parameter when its child is the
method's own address-resolved `ParamRef`. The result remains that same
method-local `ParamRef`, yielding
`[B:None:bounds(scala.Option.A,Any)]=>(by-name(ParamRef[0]))Plain:ParamRef[0]`.
The focused report records the fixture hash, original AST addresses, pinned
source declaration, and negative binder cases:
[`option-get-or-else-1930.md`](classloader-semantic-tasty-convergence/option-get-or-else-1930.md).

The frozen 14-unit/126-row cohort, `comparison.tsv`, and
`comparison-1948.tsv` are unchanged. A live run of the existing comparator
twice in fresh sessions retained all 126 row keys; compared with the
historical #1948 report, only the `option/member_signatures` legacy value
changed. That row remains `LOADER_UNSUPPORTED` overall because other Option
member signatures remain unsupported. This selected member improvement does
not establish whole-class Option parity or general Option loading.

## Integrated gate and closeout (#1931)

The implementation baseline for this closeout is merged `main`
`339f6f08203358a2dff566df566a7d9b4557d898` (PR #1982). Its reviewed
predecessors are PR #1945 at `49bd2702ff284edcbf30ceb6df51f5e09dddf598`, PR
#1957 at `fd090895e587852f9feca5599d4ddc55211ce81b`, PR #1975 at
`771f74670d5deca3f800874571601c447886c0c1`, and PR #1982 at
`339f6f08203358a2dff566df566a7d9b4557d898`. The earlier #1945 baseline and
frozen selection and refreshed parity provenance remain as recorded above.
The reference remains Scala 3.9.0 at
`777528f19a58e794c9954a42f433373472ec57f8`, TASTy 28.8/28.9 under the cohort's
per-fixture manifest, Rust `1.98.1 (48a229cea 2026-09-01)`, and Temurin
`21.0.13+9` (the same pinned JDK as required Rust CI).

The frozen denominator is unchanged: 14 input units and 126 per-field rows.
Their immutable input manifest and measured-report hashes are:

| Artifact | SHA-256 |
|---|---|
| `cohort.tsv` | `bfa365d830a627ce46bb5682a93cad90e26dbc9a90ead76bcaa65a48539a8a95` |
| `comparison.tsv` | `34097e61b691ff6ee9c16089336c495decc31241a10e448c1a1776849888aa4e` |
| `comparison-1948.tsv` | `a4b14bdf2ba921363358f568b8bce636e0e24453c2a3c6e57bd61478c231e6e0` |
| `Open.tasty` | `49286e74a3d3693b08d2a29575f23089f6be5c486231390059f43c19a8929894` |

The `frozen_cohort_is_complete_and_both_fresh_adapter_runs_are_deterministic`
test was invoked twice independently with dump mode unset. Each invocation
checked the pinned fixture hashes and versions, all 126 keys and the checked-in
normalized result; the report hash remained identical. The test itself runs
both adapters in fresh sessions and compares normalized output byte-for-byte.

| Gate shape | Current evidence and boundary |
|---|---|
| One-class Open | Private opt-in semantic adapter, canonical Object dependency, class origin, constructor completion and repeat identity; legacy public loader remains the default. |
| Two-unit graph | Private Info pair adapter enters the pinned InfoBase/InfoWrapped units in both request orders and checks the applied parent, direct member, canonical owner and cache identity. |
| Selected member type | #1930 preserves the Option `getOrElse` by-name method binder with its address-resolved `ParamRef`; reported separately from the unchanged frozen denominator. |
| Negative controls | Malformed, wrong TASTy version, unsupported, missing and mismatched resources/owners, selected origin and typed causes, late rollback, retry, and existing identities are covered by non-ignored loader unit tests. |
| Frozen compatibility | 14 units / 126 rows; report remains byte-identical over two independent test invocations. Unsupported/unresolved fields remain explicitly classified. |
| CI wiring | `cargo test --workspace --all-targets --locked` already runs these crate unit and integration tests as a required CI job; no extra workflow switch is needed. |
| Performance | Test-local matched Open legacy/semantic cold and warm API measurements; coverage and sample statistics follow. |

### Matched Open profile

The mandatory unit test
`loader::tests::open_legacy_and_semantic_loads_have_matched_cold_warm_profile_receipt`
uses the same pinned `Open.tasty`, composite classpath, Rust process and
Temurin 21.0.13+9 environment for both paths. It takes seven samples per
mode/phase. Each cold sample creates a fresh store, loader and bootstrapped
session; each warm sample repeats the same request through that loader and
asserts the returned canonical symbol identity. Each of the 28 timed requests
succeeded. The test emits attempt, success, failure and median wall-time
receipts; medians from this run were:

| Adapter | Phase | Attempts / successes / failures | Median wall time |
|---|---|---:|---:|
| Legacy | Cold | 7 / 7 / 0 | 0.776 ms |
| Legacy | Warm | 7 / 7 / 0 | 0.0006 ms |
| Semantic opt-in | Cold | 7 / 7 / 0 | 0.797 ms |
| Semantic opt-in | Warm | 7 / 7 / 0 | 0.0002 ms |

These are local wall-clock observations from one run, not a speedup claim or
service-level threshold. They cover the public legacy `load_class` call and
the private semantic adapter call end-to-end, including resource lookup and
session cache behavior. Existing `ClassLoadProfile` exposes the legacy
dependency profile; it does not instrument the private adapter's full
projection, so no internal phase attribution is inferred from these values.
No failed-load profile is included in the timing sample; fail-closed behavior
and rollback are separately exercised by the negative controls above.
The test captures the legacy root's available `ClassLoadProfile` counters and
asserts that the semantic adapter does not publish those legacy decode
profiles; both paths' top-level API attempts are counted independently.
Across the seven legacy cold samples, the root profile consistently reported
six resource lookups (all found), 109,766 resource bytes, one TASTy decode and
five class-file decodes, including its Object dependency work.

The mandatory
`semantic_adapter_rejects_unsupported_tasty_version_without_mutation_and_retries`
control starts with the pinned real `Open.tasty` 28.9.0 bytes, changes only
the minor-version header to 28.7.0, and submits those bytes through
`load_open_with_tasty_unpickler`. It asserts the exact typed
`InvalidTastyFile(Open, UnsupportedClassloaderVersion { major: 28, minor: 7,
experimental: 0 })` cause, unchanged store and session checkpoints, absent
`scala`, `java.lang`, and requested Open package state, and no loader caches or
repository entry. The same in-memory resource is then replaced with the
original valid bytes; retry succeeds, preserves the fixture's selected origin,
and a repeated request returns the cached canonical symbol. This test is not
ignored and runs under the required workspace test job.

### Verdict and limitations

**PARTIAL — implementation evidence complete; independent PR review is
pending.** The cohort is frozen, tested twice with deterministic normalized
output, and remains explicitly honest about unsupported rows. The supported
Open, Info pair, and selected Option shapes have focused crate tests;
malformed and transaction-failure cases remain mandatory in the existing
workspace test job. The small local profile establishes reproducible matched
measurement coverage, not a general performance conclusion. Promote this to
`SPRINT GOAL MET` only after the PR review and required checks are complete.

The opt-in semantic adapters remain crate-private and bounded to their pinned
fixture shapes. The legacy loader remains the default. Full semantic parity
for Open's other forms, general dependency-graph traversal, inherited-member
lookup, the frozen Animal/Dog external signatures, and the remaining
unsupported cohort fields are not claimed. Any new semantic failure belongs
in a separate focused follow-up rather than this test/report gate.

The implementation run retained the supplied original trace at
`target/issue-traces/a888fa69-b490-4e0c-91f7-ef3319378999/manifest.json`, whose
run ID is `1acdbac2-fecf-4070-b126-0711cade31c7`. No replacement trace was
created; final checks were run in a dedicated worktree rooted at the integrated
main listed above.

Implementation validation on that integrated main included:

```text
cargo test -p dotty-classloader --locked
cargo test -p dotty-tasty-unpickler --locked
cargo fmt --all -- --check
python3 tools/check-frontend-boundaries.py
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked
cargo test --workspace --all-targets --locked
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo +stable test \
  -p dotty-classfile --test classfile_jvm_encoder \
  -p dotty-jvm --lib \
  -p dotty --test jvm_source_compilation \
  --test jvm_tasty_artifacts \
  --test jvm_tasty_dual_reference_identity \
  --test jvm_stateful_class_contract --locked -- --nocapture
git diff --check
```

This host exposes an open-file soft limit of 1024, which caused intermittent
`TooManyOpenFiles` failures when parallel tests opened JDK modules. The full
workspace and focused crate suites passed with the local soft limit raised to
8192; the original failing case also passed in isolation. The separate JDK
gate passed with installed Temurin
`21.0.13+9`; local `JAVA_TOOL_OPTIONS` redirected Java's temporary files from
the host's full `/tmp` tmpfs. The classloader-specific ignored
`jdk21_classspecializer_speciesdata_resolves_its_outer_k_parameter` check was
also run explicitly against the same JDK and passed. Frozen comparator
reproduction used two separate invocations of
`cargo test -p dotty-classloader --test semantic_tasty_parity
frozen_cohort_is_complete_and_both_fresh_adapter_runs_are_deterministic
--locked`; both passed.
