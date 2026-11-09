# Semantic TASTy convergence V closeout

This is supplemental acceptance evidence for the selected semantic `Open` /
`CtorWithVal` roots and the `InfoBase` / `InfoWrapped` resolver pair. It makes no
production semantic changes. Public `ClassLoader::load_class`, ordinary
`MemberRequest` resolution, the frozen reports, and the earlier convergence-IV
closeout remain unchanged.

## Frozen integrated inputs

The tested base is `0eb707fe3f6684c60b83c6a1cac2e3a2c61924ae`, merge PR #2228.
It contains the reviewed and merged prerequisites:

| Issue | Merge PR | Merge commit |
| --- | --- | --- |
| #2202 | #2211 | `c11060e87d3fef6fd6b55cd205a77124294e5f24` |
| #2203 | #2213 | `a20d676364f627754638c2b87ca3c94601524aa6` |
| #2204 | #2216 | `188314f88b45cc2669720a9d3f0b4ac73bb5078d` |
| #2205 | #2218 | `a3319e43dac3aa08021d0cf4bb9b6fc65a75e7fc` |
| #2206 | #2223 | `27c72be638d08a521e860a51fa320bc00c388f79` |
| #2207 | #2224 | `925fce2b0ec73f4803e5597a70cf6fefb66d0586` |
| #2208 | #2228 | `0eb707fe3f6684c60b83c6a1cac2e3a2c61924ae` |

Scala compatibility is Scala 3.9.0 at source revision
`777528f19a58e794c9954a42f433373472ec57f8`; the fixtures use TASTy 28.9.
The pinned Rust toolchain is `1.98.1 (48a229cea 2026-09-01)`. Local JDK is
Temurin `21.0.13+9-LTS`; `java.base.jmod` SHA-256 is
`fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`.
`JAVA_HOME/jmods` supplies canonical JDK classes in the focused resolver tests.

| Original input | Checked-in path | SHA-256 |
| --- | --- | --- |
| `InfoBase.tasty` | `crates/dotty-tasty-unpickler/tests/fixtures/semantic/InfoBase.tasty` | `e4d58a733e8cc9a516b5d248dd6fb97acb2cb19d763e1e390cfcd1249bd7f5f9` |
| `InfoWrapped.tasty` | `crates/dotty-tasty-unpickler/tests/fixtures/semantic/InfoWrapped.tasty` | `07cca416641dda27a333df99603bf7b119d0bc5660098c5d378a09064e1630fd` |
| `Open.tasty` | `crates/dotty-classloader/tests/fixtures/tasty_visibility/me/cytrowski/tastyfixtures/visibility/Open.tasty` | `49286e74a3d3693b08d2a29575f23089f6be5c486231390059f43c19a8929894` |
| `CtorWithVal.tasty` | `crates/dotty-tasty-unpickler/tests/fixtures/semantic/CtorWithVal.tasty` | `53d335f857224d38559335eaf1efba92006a55ad9ed8e74d1f58a9e77f8a0192` |

The resolver fixture uses these original checked-in bytes. Pair tests resolve
the two semantic fixtures by their named resources; selected class tests assert
the selected resource directory and its `ClassOrigin`. The issue acceptance
does not regenerate or replace any positive fixture.

The combined `Open` / `CtorWithVal` resolver classpath order is the semantic
fixture entry, the `tasty_visibility` directory, then `$JAVA_HOME/jmods`.
`CtorWithVal` is selected from the first entry; `Open` is selected from the
second; JDK classes such as `java.lang.Object` come from the JDK entry. The
pair resolver path orders the semantic fixture directory before the JDK
`jmods` entry. Its positive pair origin is that fixture directory; the
controlled failure cases use an explicit test origin and retry with original
bytes.

## Acceptance test inventory

The initial integration inventory and exact executing test names are in
[`acceptance.tsv`](acceptance.tsv). The pair-order test already checks the
trait/class distinction, package ownership, parent and direct member shape.
This closeout adds a normalized fresh-store graph comparison to that same
positive test, so semantic equivalence is checked without comparing raw arena
IDs. Negative coverage maps to existing non-ignored `#[test]` functions in
`crates/dotty-classloader/src/resolver.rs`.

The existing required Rust workflow runs
`cargo test --workspace --all-targets --locked`; this includes the classloader
unit tests. The required JDK 21 workflow runs its class-file/JVM test gate. The
pinned Scala 3.9 consumer workflow includes `crates/**` in its pull-request
path filters, so resolver acceptance-test changes trigger it. The strict Scala
consumer runs its checked-in consumer tests against pinned Scala 3.9.0 and JDK
21; it does not substitute for the classloader's Rust tests.

## Functional and telemetry verdict

Functional verdict: **PENDING** until the listed focused tests, historical
comparators, and required local and hosted CI gates execute on the final
revision. Telemetry status: **PARTIAL** as recorded by the historical
convergence-IV trace. The trace is historical and is not a compiler-failure
verdict; no new telemetry evidence is claimed here.

## Reproduction

Run from the repository root with Rust 1.98.1 and a JDK 21 installation:

```sh
export JAVA_HOME=/path/to/jdk-21
export PATH="$JAVA_HOME/bin:$PATH"
export TMPDIR="$PWD/target/convergence-v-tmp"
export CARGO_TARGET_DIR="$PWD/target/convergence-v-cargo"
mkdir -p "$TMPDIR"
cargo test -p dotty-classloader --lib explicit_info_pair_requests_select_both_roots_in_both_orders --locked -- --nocapture --test-threads=1
cargo test -p dotty-classloader --lib two_explicit_semantic_class_requests_coexist_in_one_resolver_session --locked -- --nocapture --test-threads=1
cargo test -p dotty-classloader --lib selected_pair_late_completion_failure_rolls_back_and_retries_in_both_orders --locked -- --nocapture --test-threads=1
cargo test -p dotty-classloader --lib semantic_pair_proof_and_package_adoption_roll_back_with_enclosing_resolver_checkpoint --locked -- --nocapture --test-threads=1
cargo test -p dotty-classloader --lib independent_selected_open_resolvers_have_the_same_normalized_graph --locked -- --nocapture --test-threads=1
cargo test -p dotty-classloader --test semantic_tasty_parity frozen_cohort_is_complete_and_both_fresh_adapter_runs_are_deterministic --locked -- --nocapture --test-threads=1
cargo test -p dotty-classloader --test semantic_tasty_member_completion four_frozen_members_are_explicitly_completed_in_fresh_deterministic_runs --locked -- --nocapture --test-threads=1
```

The two historical comparator commands must each pass twice; their frozen
artifacts must retain the hashes recorded in `acceptance.tsv`. Full required
commands and actual hosted check URLs will be appended after they have run.
