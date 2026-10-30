# Lexical `Function1` closure Typer acceptance closeout

**SPRINT GOAL UNMET.** The prerequisite capture mechanisms are integrated on
this branch, and the focused Typer regressions pass. This closeout does not
claim the full #1865 acceptance gate: the exact combined member-call-plus-outer-
parameter source root is not in the pinned oracle fixture, and the six-root
two-session acceptance/report with complete store-rollback snapshots has not
been run. No backend, IR, TASTy, or runtime closure behavior is covered here.

## Pinned inputs and provenance

The closure fixture set pins Scala 3.9.0 source revision
`777528f19a58e794c9954a42f433373472ec57f8`, Eclipse Temurin JDK 21.0.8+9,
and SHA-256 digests for all eight compiler artifacts in
[`scala-artifacts.sha256.tsv`](../crates/dotty-typer/tests/fixtures/lexical-closure-contract/scala-artifacts.sha256.tsv).
The fixture runner compiles each positive source with `-Vprint:typer`, diffs
the normalized output against the checked-in snapshot, and checks wrong-result
and unresolved-name controls. Existing snapshots are provenance for the
individual shapes listed below; they do not establish the missing combined
shape or the integrated two-session gate.

## Positive source roots

| Group | Pinned source and method | Rust whole-method evidence | Capture result | Closeout |
| --- | --- | --- | --- | --- |
| No capture | `lexical-closure-contract/Positive.scala`; strict source test uses `class C { def identity: Int => Int = x => x }` | `lexical_closure_contract::lexical_closure_identity_and_mapping_are_session_local` | Empty `Closure.env` | TYPED individually |
| Enclosing parameter | `MethodParameters.scala`; `CaptureParameter.addBy` | `method_parameter_capture::ordinary_function1_captures_enclosing_method_parameters_by_first_use` | Canonical `delta`, once | TYPED individually |
| Immutable local | `LocalValues.scala`; `LocalValues.offset` | `local_value_capture::ordinary_function1_captures_prior_initialized_local_values` | Canonical initialized `bias`, once | TYPED individually |
| Enclosing `this` | `ThisCapture.scala`; `Outer.self` | `enclosing_this_capture::ordinary_function1_captures_canonical_enclosing_class_and_trait_this` | Canonical `Outer.this` | TYPED individually |
| Same-owner member call | `SameOwnerMembers.scala`; `Calculator.lifted` | `enclosing_this_capture::ordinary_function1_captures_implicit_same_owner_members` | Canonical `Calculator.this` | TYPED individually |
| Combined member call and parameter | Required shape: `def fn(delta: Int): Int => Int = x => inc(x) + delta` | Existing tests cover parameter-plus-`this` and member calls separately; no exact combined source root is asserted | Expected `[this, delta]` in first-use order | **UNMET**; add the pinned Scala oracle root and whole-`DefDef` assertion before claiming acceptance |

The focused Typer suite run for this closeout was:

```text
CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p dotty-typer --locked --lib ordinary_function1_
5 passed; 0 failed
```

This selects five existing whole-method regressions. It is not a replay of all
six roots in two fresh source sessions. Their detailed symbol, owner, type,
closure-env, and `SourceTypedIndex` assertions remain in the linked test
modules; no arena-local typed tree numbers are compared across sessions.

## Negative controls and limits

Existing focused tests reject or roll back these representative cases:

- same-name lambda/local shadowing, unknown or forward references, by-name
  parameters, mutable/lazy/contextual/pattern locals, and wrong local types;
- wrong expected function result, non-enclosing `Other.this`, module `this`,
  inherited member lookup, invalid member-call arguments, and mutable member
  updates;
- failed capture attempts leave no typed method/RHS mapping or lambda method,
  and valid methods remain typeable afterward. Parameter/local fresh-session
  tests compare semantic identity shape.

The complete issue matrix remains **UNMET**: there is no single test that runs
every negative root against the full six-positive acceptance harness twice,
and there is no assertion covering rollback of every listed store, scope,
typed arena, environment, and typed index component for every failure. `super`
capture is excluded by the supported contract, but this closeout does not
claim a pinned Scala negative oracle for it.

## Standard-library follow-up and regression baseline

The pinned, deterministic two-root receipt is
[`typer-closure-instance-members-roots-3.9.0.tsv`](typer-closure-instance-members-roots-3.9.0.tsv):
`Function1.andThen` (`#tree=111`) is fully typed; `Function1.compose`
(`#tree=86`) remains incomplete at
`ApplicationArgumentConformanceUnsupported` at `Function1.scala#tree=81`.
This is a whole-method result, not partial capture credit. PR #1884 records
that the frozen 25-method ordinary cohort is 15/25 and retains its prior 14
required successes. The separate 15-member analysis cohort has not been
re-audited as part of this closeout, so its preservation is **UNMET** here.
Historical corpus percentages are not inferred from these focused receipts.

## Reproduction

Run the pinned source oracle with the checkout, artifact cache, and JDK
specified in the fixture runner, then run the focused whole-method regressions:

```sh
SCALA39_ROOT=/path/to/scala3-source \
JAVA_HOME=/path/to/temurin-21 SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=/path/to/coursier/repo1.maven.org/maven2 \
crates/dotty-typer/tests/fixtures/lexical-closure-contract/run

CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 \
cargo test -p dotty-typer --locked --lib ordinary_function1_
```

The current environment could not reproduce the pinned Scala oracle: the
Scala source checkout and required Temurin 21.0.8+9 are unavailable. Its
checked-in source snapshots and artifact manifest were inspected, not
regenerated. A first blocker moving in `compose` is not counted as full method
acceptance.
