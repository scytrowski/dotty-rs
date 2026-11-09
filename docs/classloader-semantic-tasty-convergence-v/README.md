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

Functional verdict: **MET** on tested commit
`d1eefc5674db8076bbecf7f4af009c41ace91fa9`. The focused resolver matrix,
classloader/unpickler suites, historical comparators, local required Rust/JDK
checks, all eight hosted checks, and the pinned Scala 3.9 consumer shards
passed. The hosted job URLs and tested SHA are recorded in `acceptance.tsv`.

Telemetry status: **PARTIAL** as recorded by the historical convergence-IV
trace. The trace is historical and is not a compiler-failure verdict; no new
telemetry evidence is claimed here.

Local validation on `f907485d6a` used Rust 1.98.1 and Temurin JDK 21.0.13:

- `cargo test -p dotty-classloader --locked` — passed, including all resolver
  unit and integration tests.
- `cargo test -p dotty-tasty-unpickler --locked` — passed.
- The frozen 14-unit/126-row comparator and original four-member comparator
  each passed twice. Their checked-in SHA-256 values match `acceptance.tsv`.
- `cargo fmt --all -- --check`, `python3 tools/check-frontend-boundaries.py`,
  workspace Clippy with `-D warnings`, and workspace rustdoc with warnings
  denied — passed.
- `cargo test --workspace --all-targets --locked` — passed.
- The strict JDK 21 gate with `DOTTY_CLASSFILE_REQUIRE_JDK=1` — passed.
- `git diff --check` — passed.

The final receipt-only documentation update was checked with
`cargo fmt --all -- --check` and `git diff --check`. It does not change tests,
source, or workflow behavior. The hosted Rust CI job also ran the workflow
lint step successfully.

## Hosted required checks

All eight PR check runs completed successfully on
`d1eefc5674db8076bbecf7f4af009c41ace91fa9`:

| Check | Result | Job |
| --- | --- | --- |
| Rust checks (format, boundary guard, Clippy, rustdoc, workspace tests, diff) | success | [job 129461622577](https://github.com/scytrowski/dotty-rs/actions/runs/42705652022/job/129461622577) |
| Classfile JVM encoder gate (JDK required) | success | [job 129461622204](https://github.com/scytrowski/dotty-rs/actions/runs/42705652022/job/129461622204) |
| Focused semantic regression gate | success | [job 129461615925](https://github.com/scytrowski/dotty-rs/actions/runs/42705652046/job/129461615925) |
| Strict Scala 3.9 TASTy shard 1/4 | success | [job 129461609616](https://github.com/scytrowski/dotty-rs/actions/runs/42705651574/job/129461609616) |
| Strict Scala 3.9 TASTy shard 2/4 | success | [job 129461609978](https://github.com/scytrowski/dotty-rs/actions/runs/42705651574/job/129461609978) |
| Strict Scala 3.9 TASTy shard 3/4 | success | [job 129461610704](https://github.com/scytrowski/dotty-rs/actions/runs/42705651574/job/129461610704) |
| Strict Scala 3.9 TASTy shard 4/4 | success | [job 129461610204](https://github.com/scytrowski/dotty-rs/actions/runs/42705651574/job/129461610204) |
| Pinned Scala 3.9 factory TASTy consumer aggregate | success | [job 129472600137](https://github.com/scytrowski/dotty-rs/actions/runs/42705651574/job/129472600137) |

The hosted Scala workflow checked out Scala 3.9.0 at
`777528f19a58e794c9954a42f433373472ec57f8`, used Temurin JDK 21.0.13 and Rust
1.98.1, validated the strict target manifest, and ran all four shards plus the
required semantic-loader secondary-constructor rejection. The exact workflow
and reproducible command forms are recorded below.

Review receipt: PR #2229 received a logical `CHANGES_REQUESTED` finding for
this missing receipt. This update addresses that blocking documentation
finding; it does not alter the previously published review or claim a new
review verdict. See the [review summary](https://github.com/scytrowski/dotty-rs/pull/2229#pullrequestreview-5833315772)
and [blocking inline thread](https://github.com/scytrowski/dotty-rs/pull/2229#discussion_r4518406340).

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
artifacts must retain the hashes recorded in `acceptance.tsv`.

## Required gate commands

From the repository root, with pinned Rust 1.98.1 and a Temurin JDK 21
installation, run the Rust and strict JDK jobs as follows:

```sh
cargo fmt --all -- --check
python3 tools/check-frontend-boundaries.py
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
export TMPDIR="$PWD/target/rust-tests-tmp"
mkdir -p "$TMPDIR"
cargo test --workspace --all-targets --locked
git diff --check

export JAVA_HOME=/path/to/temurin-21.0.13
export PATH="$JAVA_HOME/bin:$PATH"
export TMPDIR="$PWD/target/classfile-jvm-tmp"
mkdir -p "$TMPDIR"
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo +stable test \
  -p dotty-classfile --test classfile_jvm_encoder \
  -p dotty-jvm --lib \
  -p dotty --test jvm_source_compilation \
  -p dotty --test jvm_stateful_class_contract \
  --locked -- --nocapture
```

For the pinned Scala consumer, prepare the Scala 3.9.0 source checkout and
verified dependencies at the revisions/pins specified by the workflow, then run
its strict target manifest for all four shards. The workflow sets the strict
consumer/oracle requirement variables and provisions JDK 21 before running
this manifest-driven command:

```sh
export SCALA39_SOURCE_SHA=777528f19a58e794c9954a42f433373472ec57f8
python3 tools/ci/validate_strict_tasty_targets.py
for shard in shard-1 shard-2 shard-3 shard-4; do
  mapfile -t strict_targets < <(python3 tools/ci/validate_strict_tasty_targets.py --shard "$shard")
  test "${#strict_targets[@]}" -gt 0
  test_args=()
  for target in "${strict_targets[@]}"; do test_args+=(--test "$target"); done
  cargo +1.98.1 test "${test_args[@]}" --locked -- --nocapture --test-threads=1
done
export DOTTY_REQUIRE_SCALA39_SECONDARY_CONSTRUCTOR_TEST=1
cargo +1.98.1 test -p dotty-classloader --lib \
  semantic_class_loader_rejects_secondary_constructor_with_rollback \
  --locked -- --nocapture --test-threads=1
```

The hosted run used the complete environment and setup in
[the pinned Scala consumer workflow](../../.github/workflows/jvm-tasty-factory-scala-consumer.yml).
Its four shard job URLs and aggregate result are in `acceptance.tsv`.

Re-run both frozen comparators twice each with:

```sh
for _ in 1 2; do
  cargo test -p dotty-classloader --test semantic_tasty_parity \
    frozen_cohort_is_complete_and_both_fresh_adapter_runs_are_deterministic \
    --locked -- --nocapture --test-threads=1
done
for _ in 1 2; do
  cargo test -p dotty-classloader --test semantic_tasty_member_completion \
    four_frozen_members_are_explicitly_completed_in_fresh_deterministic_runs \
    --locked -- --nocapture --test-threads=1
done
```
