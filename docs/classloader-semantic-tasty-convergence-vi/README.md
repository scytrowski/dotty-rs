# Semantic TASTy convergence VI: generic classloader acceptance

This is supplemental C1 acceptance evidence for the source-authentic Scala 3.9
class `CtorGeneric[A](x: A)`. It closes the original generic single-root load
through the private classloader and explicitly selected `ClasspathSymbolResolver`
on integrated `main`. The PR changes tests and evidence only; production
semantics, fixture bytes, and earlier comparator artifacts are unchanged.

## Frozen revision and inputs

The integrated base is `fe4a4270ded53cb0240d7feba9ad2365634209d4`. It contains
the reviewed prerequisite merges below; each merge is an ancestor of the tested
revision `30c32f0aad32a7811f5b9904fa772841a2fa5c67`.

| Issue | Merged PR | PR-head SHA | Merge commit | PR-head Rust / JDK run | PR-head strict Scala run |
| --- | --- | --- | --- | --- | --- |
| #2287 | [#2296](https://github.com/scytrowski/dotty-rs/pull/2296) | `4265af922f8896f4192e4e3ce353de843d4f4749` | `353e7de5138976bf5ef0dcc60b044f7fcc3adfc4` | [success, run 43598628289](https://github.com/scytrowski/dotty-rs/actions/runs/43598628289) | [success, run 43598628321](https://github.com/scytrowski/dotty-rs/actions/runs/43598628321) |
| #2288 | [#2312](https://github.com/scytrowski/dotty-rs/pull/2312) | `8b78787154734bef69fae4defd03cf9ea560a6de` | `6b6eb51ee16bcb9c0bf300438448c991235e74eb` | [success, run 43611753546](https://github.com/scytrowski/dotty-rs/actions/runs/43611753546) | [success, run 43611753688](https://github.com/scytrowski/dotty-rs/actions/runs/43611753688) |
| #2289 | [#2315](https://github.com/scytrowski/dotty-rs/pull/2315) | `d4d2b339428a7e0d2d372ada723396c108f5c4aa` | `1571658b9dfa90e00b040049cdd8ce461dedc31e` | [success, run 43624465900](https://github.com/scytrowski/dotty-rs/actions/runs/43624465900) | [success, run 43624465874](https://github.com/scytrowski/dotty-rs/actions/runs/43624465874) |
| #2290 | [#2318](https://github.com/scytrowski/dotty-rs/pull/2318) | `e864cdfa538ac80338fce8b75b407850cfa39b50` | `e03a2fbe8465088d51082d79d093c9dbba9c1803` | [success, run 43696506748](https://github.com/scytrowski/dotty-rs/actions/runs/43696506748) | [success, run 43696506757](https://github.com/scytrowski/dotty-rs/actions/runs/43696506757) |
| #2291 | [#2319](https://github.com/scytrowski/dotty-rs/pull/2319) | `ac620e6ef21475508d673b420b94d444254c1f32` | `fe4a4270ded53cb0240d7feba9ad2365634209d4` | [success, run 43749375457](https://github.com/scytrowski/dotty-rs/actions/runs/43749375457) | [success, run 43749375540](https://github.com/scytrowski/dotty-rs/actions/runs/43749375540) |

The reviewed PR-head workflows for these prerequisites completed successfully;
the linked Rust runs include the required JDK 21 classfile/JVM job, and the
Scala runs include four strict shards and their aggregate. These are prerequisite
receipts, not checks on the integrated C1 revision.

The compiler and fixture provenance is frozen as follows:

| Item | Value |
| --- | --- |
| Rust | `rustc 1.98.1 (48a229cea 2026-09-01)`, Cargo 1.98.1 |
| Scala reference | 3.9.0 source SHA `777528f19a58e794c9954a42f433373472ec57f8` |
| TASTy format | 28.9 |
| JDK | Temurin 21.0.13+9-LTS; `$JAVA_HOME/jmods/java.base.jmod` SHA-256 `fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91` |
| Source fixture | `Constructors.scala`, SHA-256 `ee054964fa24c1e2084350b257b0875d2c9c38038b73750b7eefb12b70ffcd63` |
| Authentic generic fixture | `CtorGeneric.tasty`, SHA-256 `a95d3bd7267b9ef1abac1d575b749c88b3bdcaaa5f2b80df66d8191e6c12cf71` |

Direct loading places the semantic fixture directory before the JDK `jmods`
entry. Selected resolver tests order the semantic fixture entry, the visibility
fixtures, and JDK `jmods`; `CtorGeneric.tasty` therefore wins from the semantic
directory, while `java/lang/Object` resolves from the actual JDK. Both routes
assert `ClassFormat::Tasty` and `ClassOrigin::Directory` for the selected
fixture. The selected coexistence controls also record `Open.tasty` from the
visibility directory and `CtorWithVal.tasty` from the semantic directory.

## Semantic acceptance and regressions

The direct-loader test
`authentic_ctor_generic_loads_through_the_private_loader_and_repeats_canonically`
runs the original fixture in two independent stores and compares normalized
graphs. It checks class-owned header `A` separately from constructor-owned
`Poly[A]`, `Nothing..Any` bounds, `x: A` as a constructor-binder `ParamRef(0)`,
the applied class result, canonical `Object`, and retained class origin. Repeat
loads reuse the same intra-store IDs and leave store/session checkpoints intact.

The selected-resolver test
`ctor_generic_coexists_with_selected_open_and_ctor_with_val_in_both_orders`
does the same normalized comparison in two fresh stores while changing request
order across selected `Open`, `CtorWithVal`, and `CtorGeneric`. It checks the
class and constructor binders and bounds, the formal and result binder
references, selected origin, and intra-store selected/ordinary repeat identity.
`explicit_semantic_ctor_generic_repeats_preserve_canonical_binders_and_origin`
adds direct positive and foreign-origin authentication assertions.

`acceptance.tsv` maps every positive, selected-name mismatch, legacy/default,
classpath-preference, B4 malformed/foreign/collision/late-failure/outer-rollback
and retry contract to an executing non-ignored test. Existing controls retain
`Open`, `CtorPlain`, `CtorWithVal`, the genuine `InfoBase`/`InfoWrapped` pair in
both request orders, legacy `load_class`, and selected-name mismatches. No
generic fallback or production resolver cutover is introduced.

## Frozen comparators

Both existing comparators passed twice on the integrated C1 implementation
revision. The historical checked-in files remain byte-identical:

| Artifact | SHA-256 |
| --- | --- |
| 14-unit cohort `cohort.tsv` | `bfa365d830a627ce46bb5682a93cad90e26dbc9a90ead76bcaa65a48539a8a95` |
| prior comparison `comparison.tsv` | `34097e61b691ff6ee9c16089336c495decc31241a10e448c1a1776849888aa4e` |
| 126-row comparison `comparison-1948.tsv` | `a4b14bdf2ba921363358f568b8bce636e0e24453c2a3c6e57bd61478c231e6e0` |
| original four-member `member-completion.tsv` | `5bc658a2c13e35c7751b5ab3a29ed6f0c842b0d0ef1e7bb6468c141a7ad30c61` |

The parity test validates the frozen 14-unit cohort and its original outcome
vocabulary; the member-completion test attempts exactly the four listed
members. No unsupported row was rewritten or converted to success. The exact
commands are:

```sh
for _ in 1 2; do
  cargo +1.98.1 test -p dotty-classloader --test semantic_tasty_parity \
    frozen_cohort_is_complete_and_both_fresh_adapter_runs_are_deterministic \
    --locked -- --nocapture --test-threads=1
done
for _ in 1 2; do
  cargo +1.98.1 test -p dotty-classloader --test semantic_tasty_member_completion \
    four_frozen_members_are_explicitly_completed_in_fresh_deterministic_runs \
    --locked -- --nocapture --test-threads=1
done
```

## Functional and telemetry verdict

**Functional: MET** on implementation/test revision
`30c32f0aad32a7811f5b9904fa772841a2fa5c67`, with the local Rust, classloader,
unpickler, comparator, and strict JDK 21 checks listed in `acceptance.tsv`.
Production semantic files and all historical artifacts are unchanged.

**Telemetry: PARTIAL.** C1 semantic and hosted-check receipts are recorded, but
no C1-specific issue-level trace was captured. The [historical convergence-IV
trace](https://github.com/scytrowski/dotty-rs/issues/2070#issuecomment-6627776023)
remains historical evidence and is not recast as a compiler failure or a newly
captured trace. This status describes telemetry coverage only.

The tested implementation revision is recorded in `acceptance.tsv`. The
receipt-document revision is the final documentation commit in
[#2321](https://github.com/scytrowski/dotty-rs/pull/2321); its exact SHA is
reported in the PR handoff. Documentation-only receipt edits do not change the
implementation under test.

## Required local validation

With `TMPDIR`, `TMP`, and `TEMP` set to registered managed scratch, the
following passed on the implementation/test revision:

```sh
cargo +1.98.1 fmt --all -- --check
python3 tools/check-frontend-boundaries.py
cargo +1.98.1 clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo +1.98.1 doc --workspace --no-deps --locked
cargo +1.98.1 test --workspace --all-targets --locked
cargo +1.98.1 test -p dotty-classloader --locked
cargo +1.98.1 test -p dotty-tasty-unpickler --locked
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo +stable test \
  -p dotty-classfile --test classfile_jvm_encoder \
  -p dotty-jvm --lib \
  -p dotty --test jvm_source_compilation \
  --test jvm_stateful_class_contract --locked -- --nocapture
git diff --check
```

The hosted Rust, strict JDK 21, focused semantic, and pinned Scala 3.9 shard and
aggregate job URLs for the tested revision are listed in `acceptance.tsv`.
