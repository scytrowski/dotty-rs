# Semantic TASTy member completion audit (issue #1995)

This supplemental audit explicitly enters and completes each frozen `DEFDEF`
at its address in that unit's original TASTy index. Every method runs with a
new `SemanticStore`, exactly one `Definitions::bootstrap`, the real fixture
classpath, and Temurin OpenJDK `21.0.13+9-LTS` (`java.base.jmod` SHA-256
`fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`). The integration test repeats
the whole four-row audit twice and compares its normalized TSV byte-for-byte.
The fixture paths, owner names, method names, and addresses are fixed in the
test; a missing or duplicate method is a test failure, never a replacement
candidate.

The four TASTy inputs are format 28.9 and their byte hashes are recorded in
[`member-completion.tsv`](member-completion.tsv). The Scala 3.9.0 reference
compiler source fixtures in this repository are pinned to revision
`777528f19a58e794c9954a42f433373472ec57f8`. Option's already-reviewed source
contract is `getOrElse[B >: A](default: => B): B`; its output retains the
method-local bound, by-name clause, and `B` result; each `ParamRef` in that
signature is normalized with a stable traversal-relative binder identity.
`InfoWrapped.m` retains
its method-local `F` application and returns canonical `F[Int]` semantics.

## Issue #1997 integration freeze

The acceptance baseline is integrated `main` at
`53dd490de7acbc19120a75d74db8887c7181a6c1` (PR #2019 merge). The required
prerequisites are present on that commit:

| Issue | PR | Reviewed PR head | Merge commit |
| --- | --- | --- | --- |
| #1989 | #2002 | `8eb61572070a8344fc10b1050a59bcbfd0f593cb` | `bd90c1a82d89f315c7613570d6d5f518e0e76d8b` |
| #1990 | #2004 | `e3ba851849becc29b6f42ad76d64455b6491ba12` | `02f666dcf794a24fd4ba3676380444461c8e0f72` |
| #1991 | #2007 | `795d9106c72e459a9a45201c26aa3b0dbe43e0de` | `7943e8d495815ceef2b7e4a53baee7f6fcac949b` |
| #1993 | #2009 | `bbc2d51671ebd0629963a54a17b21936b41efa0f` | `c7a9ad9894d9e7146c9fd934305243c12098d44f` |
| #1995 | #2019 | `824e7bef2f18f69241addb3a1021cbcb727ff60b` | `53dd490de7acbc19120a75d74db8887c7181a6c1` |

The pinned Scala source revision remains
`777528f19a58e794c9954a42f433373472ec57f8`; both visibility TASTy fixtures
are format 28.9, within the classloader's accepted 28.8/28.9 policy. The
fixture receipt is:

| Input | SHA-256 |
| --- | --- |
| `Visibility.scala` | `48e37c29b951d908ea60d031cd0bf0f2b909f860b142b47e33f7975a58d7d59d` |
| `Open.tasty` | `49286e74a3d3693b08d2a29575f23089f6be5c486231390059f43c19a8929894` |
| `PlainPrivate.tasty` | `db045c091f6bc71cdee1fef6c81cd09fe1f520f740f09fec008dd5f1fd98c4ba` |

The test classpath is ordered as the pinned visibility fixture directory,
then the configured JDK `jmods`. Within a directory or JAR, `.tasty` remains
preferred over a collocated `.class`; the existing directory, JAR, and
Info-pair loader regressions cover that selection rule. Expected class roles
are `visibility.Open` (public class) and `visibility.PlainPrivate` (private
within its source package); both have the canonical `java.lang.Object` parent,
one zero-argument primary constructor returning the canonical class identity,
and a TASTy origin from the visibility fixture directory.

The local Rust toolchain is pinned by `rust-toolchain.toml` to `1.98.1`
(`rustc 1.98.1`, `cargo 1.98.1`). The CI workflow installs Temurin
`21.0.13+9.0.LTS`; the existing member audit records `java.base.jmod`
SHA-256 `fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`
for that runtime. The CI JDK install and mandatory JVM gate already exercise
the workspace test path, so this issue adds no workflow wiring.

Before this closeout test, `loader.rs` called the same private operation for
both visibility roots in one store. Fresh-store tests exercised the direct
unpickler path, not the reusable loader. The frozen parity test already ran
its 14-unit/126-row comparison twice and checked determinism; the separate
four-method audit already attempted exactly four members. The new loader test
will close the two-session gap while keeping numeric arena IDs local to each
store. The negative rollback cases remain separate, directly observable
regressions in the loader suite; their final run receipts are recorded below.

The frozen comparator artifacts remain untouched at these hashes:

| Artifact | SHA-256 |
| --- | --- |
| `cohort.tsv` (14 units) | `bfa365d830a627ce46bb5682a93cad90e26dbc9a90ead76bcaa65a48539a8a95` |
| `comparison.tsv` | `34097e61b691ff6ee9c16089336c495decc31241a10e448c1a1776849888aa4e` |
| `comparison-1948.tsv` (126 comparison rows) | `a4b14bdf2ba921363358f568b8bce636e0e24453c2a3c6e57bd61478c231e6e0` |

The configured nonempty issue trace is run
`113f2c35-6b19-45e7-ab5c-1fab1ba58ba7`; it is preserved in place and is not
copied into the PR. The historical #1931 `PARTIAL` report remains a historical
measurement and is not rewritten by this integration closeout.

## Outcomes

| Member | Explicit completion | Normalized finding |
| --- | --- | --- |
| `scala.Option.getOrElse` | `COMPLETE` at `DEFDEF@666` | Canonical and current legacy member signatures are equal, including `B >: Option.A`, the by-name parameter, and both parameter/result references to the indexed method-local `B` binder. This agrees with the reviewed #1930 source/typed contract. |
| `InfoWrapped.m` | `COMPLETE` at `DEFDEF@115` | Canonical and legacy projections are semantically equal. The normalized result is `ByName(InfoWrapped.F[Int])`, preserving the wrapper carried by this compiled TASTy; the source member is declared `m: F[Int]`. For the legacy comparison only, the test copies the original `InfoWrapped.tasty` and `InfoBase.tasty` bytes into a temporary package-relative classpath layout because the fixture generator stores them flat. No TASTy bytes are rewritten. |
| `Animal.sound` | `UNRESOLVED_EXTERNAL` at `DEFDEF@22` | Canonical resolver request is `Predef` in prefix `scala`; `scala/Predef.tasty` is selected from the Scala library fixture path. The test-only classpath trace then records both `scala/Predef/String` and `scala/Predef$String` as missing before the first typed error, `UnresolvedMember` for `String` at TASTy address 27. |
| `Dog.sound` | `UNRESOLVED_EXTERNAL` at `DEFDEF@36` | Independent original unit and owner; the same measured canonical request selects `scala/Predef.tasty`, and both String resource paths are missing before its distinct first typed error at address 41. The failure is recorded separately and was not inferred from Animal. |

The legacy outcomes are obtained by loading the owner through the existing
`ClassLoader` member projection in a separate fresh store. `MATCH` means the
legacy member symbol exists with a complete normalized signature. The
historical class-root reports, their frozen 14-unit / 126-field denominator,
and their hashes are unchanged: `cohort.tsv` SHA-256
`bfa365d830a627ce46bb5682a93cad90e26dbc9a90ead76bcaa65a48539a8a95`,
`comparison.tsv` SHA-256
`34097e61b691ff6ee9c16089336c495decc31241a10e448c1a1776849888aa4e`, and
`comparison-1948.tsv` SHA-256
`a4b14bdf2ba921363358f568b8bce636e0e24453c2a3c6e57bd61478c231e6e0`. The
Option row here reflects the current direct legacy projection; the historical
class-root-only report remains a separate immutable artifact.

The canonical request and classpath trace distinguish selecting the Predef
TASTy resource from the subsequent missing `String` resource lookups. They do
not establish whether the resolver should add alias loading or whether those
resources are expected to exist as standalone classpath entries. The
first typed cause remains the recorded `UnresolvedMember`; follow-up should
trace the Scala 3.9.0 alias/dependency chain before changing resolver behavior.

## Negative controls

The focused test checks that a one-byte-truncated original `InfoWrapped.tasty`
is rejected before it can mutate the bootstrapped store; a mismatched
`Animal.sound` request for the real `Dog.sound` address produces a typed
`ForeignOwner` rejection through the audit request path, preserves the pending
method state, and permits a valid-owner retry; and a dangling method address
produces the exact typed
`MissingEnteredSymbol { address: u32::MAX }`. The dangling-address control
leaves the real method pending and that same method completes successfully on
retry. `InfoWrapped.m` completes successfully after the malformed-input
control; the foreign-owner check leaves its committed semantic state intact.

## Ranked follow-up

1. **Bounded external alias resolution for `scala/Predef.String`** — owner:
   `dotty-classloader` resolver. Reproduce with either frozen
   `Animal.sound@22` (`UnresolvedMember` at 27) or `Dog.sound@36` (at 41) using
   the recorded JDK 21 and classpath. Determine the exact alias/dependency
   chain from Scala 3.9.0 inputs before changing resolver behavior. This audit
   does not implement that change or claim broader Scala standard-library
   support.

## Issue #1997 closeout

**Verdict: SPRINT GOAL MET.** The opt-in reusable loader met the two-class
positive acceptance in two fresh stores, and all required Rust/JDK checks
passed. The negative matrix has typed rollback and retry coverage for wrong
package, module roots, parameterized class templates, a real Scala 3.9
secondary-constructor class, malformed and unsupported-version TASTy, and a late
failure after a prior class has committed. The secondary-constructor fixture is
compiled by the pinned Scala compiler and required by the JDK 21 consumer
workflow; the same private loader returns `UnsupportedShape` and leaves store,
session, package, and cache checkpoints unchanged.

The integrated base was `53dd490de7acbc19120a75d74db8887c7181a6c1`; tested
code head was `031bb7fe5c80947f74015df40c1049a9a7351994` in PR
[#2022](https://github.com/scytrowski/dotty-rs/pull/2022). The implementation
changes only tests and documentation; production loader behavior is unchanged.
CI adds a required command in the pinned Scala/JDK workflow for the dynamic
secondary-constructor fixture.

### Positive acceptance

| Class role | Fresh session 1 | Fresh session 2 | Assertions |
| --- | --- | --- | --- |
| `visibility.Open` | accepted | accepted | canonical package/class/constructor identities in the store; class and constructor kinds/owners; public visibility; canonical `java.lang.Object` parent; zero-argument constructor returning the class; fixture-directory TASTy origin; exact selected TASTy bytes; stable same-loader repeat and all three cache entries agree |
| `visibility.PlainPrivate` | accepted | accepted | same identity, owner, parent, constructor, origin, selected resource, and repeat-cache checks; `PrivateWithin` points to its source package |

Both roles use `load_semantic_class_with_tasty_unpickler` and the same
`ClassLoader` instance within each session. IDs are compared only inside their
own store; the two stores are checked by semantic roles. `Open.tasty` and
`PlainPrivate.tasty` SHA-256 values are recorded above.

### Negative controls and boundaries

`semantic_class_loader_rejects_wrong_package_and_module_before_mutation`
asserts exact `UnsupportedShape` errors for a wrong requested package, a
module-only root, and `CtorPlain.tasty`'s parameterized primary-constructor
shape, and checks store, package/session checkpoints, and caches remain
unchanged. Existing loader regressions cover unsupported TASTy versions
(`semantic_adapter_rejects_unsupported_tasty_version_without_mutation_and_retries`), malformed input, absent classpath resources, duplicate/pre-existing semantic identities and scopes, and the late partial-load rollback/retry path
(`semantic_class_late_failure_preserves_prior_class_and_retries`). The existing
nested-constructor structural regression verifies that a nested constructor is
not mistaken for a direct template declaration. The new
`semantic_class_loader_rejects_secondary_constructor_with_rollback` test
compiles `tasty_secondary_constructor/SecondaryConstructor.scala` with the
pinned Scala compiler, then asserts typed shape rejection and unchanged store,
session, package, and cache checkpoints. The Scala consumer workflow invokes it
with the required gate enabled; it passed with one test run, zero failures, and
zero ignored tests.

The default `ClassLoader::load_class` path is unchanged. Existing directory,
JAR, and Info-pair tests continue to assert `.tasty` priority over a collocated
`.class`. The adapter suite also retains the earlier `Open`, `InfoBase` /
`InfoWrapped`, and `Option.getOrElse` coverage. Normal CI runs the non-ignored
classloader and unpickler tests; the pinned Scala consumer workflow additionally
requires the dynamic secondary-constructor regression.

### Frozen comparator and member audit

The original 14-unit and 126-row comparator artifacts were not modified. Their
hashes remain the values recorded above; the deterministic comparator test ran
twice in the CI audit. The two new class acceptances are not added to that
historical denominator and do not establish global semantic parity.

The separate four-member report remains four attempts: `Option.getOrElse` and
`InfoWrapped.m` are `COMPLETE`; `Animal.sound` and `Dog.sound` are
`UNRESOLVED_EXTERNAL`, with their distinct first errors at TASTy addresses 27
and 41. Those unresolved method rows are not class-loader acceptance failures
and are not counted as accepted members.

No cold/warm timing was recorded: the existing bounded harness does not time
both target classes as a pair, and no separate benchmark was added. The
historical #1931 `PARTIAL` remains unchanged.

### Validation receipts

All checks below were run against final code head
`031bb7fe5c80947f74015df40c1049a9a7351994`:

- Local: `cargo test -p dotty-classloader --lib --no-run --locked`, focused
  semantic-loader negative/visibility tests, `cargo fmt --all -- --check`,
  `python3 tools/check-frontend-boundaries.py`, Clippy, rustdoc, and
  `git diff --check` passed. The local host has no JDK, so full JDK-dependent
  tests were delegated to the required hosted JDK 21 gates.
- [CI](https://github.com/scytrowski/dotty-rs/actions/runs/41502299594):
  formatting, boundaries, Clippy, rustdoc, full
  `cargo test --workspace --all-targets --locked`, diff check, and the mandatory
  JDK 21 classfile/JVM gate passed.
- [Focused semantic regression gate](https://github.com/scytrowski/dotty-rs/actions/runs/41502299646):
  passed; it ran the focused acceptance matrices, frozen comparator/audit, and
  uploaded artifact `typer-focused-audit-f6ea7e251f6d55ba072e6292e4c47938f4ed313e`
  (SHA-256 `760cfa00517fe0fecf17fb452f1e72de1d39f9275f68265ca9a0450d2963e4e4`).
- [Pinned Scala 3.9 factory TASTy consumer](https://github.com/scytrowski/dotty-rs/actions/runs/41502299774):
  passed, including the non-skipped secondary-constructor rejection test and
  the mandatory Java 21 verifier/publisher gate.

The configured trace `113f2c35-6b19-45e7-ab5c-1fab1ba58ba7` remains preserved
in its original location and was not published separately.
