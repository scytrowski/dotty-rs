# Reference-local constructor flow: Scala 3.9 and Rust boundary audit

Issue [#1849](https://github.com/scytrowski/dotty-rs/issues/1849) audits one
producer whose immutable `Counter` local is initialized from a same-instance
factory call, then passed to `CounterOwner` in the same method block. This is an
audit of existing boundaries. It does not enable or claim paired JVM/TASTy
support for the local-block expression.

## Frozen producer and oracle

The producer is the single source file
[`CounterOwnerFactory.scala`](../tests/fixtures/jvm_tasty_reference_local_constructor_flows/CounterOwnerFactory.scala).
It declares exactly `Counter`, `CounterOwner`, and `CounterOwnerFactory`, in
one package. The pinned Scala source checkout was at
`777528f19a58e794c9954a42f433373472ec57f8`; its built compiler reports
`3.9.0-bin-SNAPSHOT-git-777528f`. The producer was compiled once with
Temurin `21.0.8+9` and `-release 21` into an empty directory. The output
contains exactly three classfiles and their three TASTy sidecars. Every
classfile is major version 65, every sidecar is TASTy 28.9.0, and the
classfile's `TASTY` UUID matches its paired sidecar header. The per-file hashes,
UUIDs, compiler/runtime hashes, source hashes, and method observations are
recorded in [`oracle.json`](../tests/fixtures/jvm_tasty_reference_local_constructor_flows/oracle.json).

The compiler's `javap -v -p -c` output for
`CounterOwnerFactory.create(I)Lexample/CounterOwner;` reports `stack=3`,
`locals=3`, and these instructions:

```text
0:  aload_0
1:  iload_1
2:  invokevirtual  CounterOwnerFactory.makeCounter:(I)Lexample/Counter;
5:  astore_2
6:  new            example/CounterOwner
9:  dup
10: aload_2
11: invokespecial  CounterOwner.<init>:(Lexample/Counter;)V
14: areturn
```

The slots are `this=0`, `seed=1`, and `counter=2`. The factory call completes
and stores its initialized `Counter` before the outer `CounterOwner` allocation
starts. The Owner constructor receives the value loaded from slot 2.

The separate Scala-authored
[`Consumer.scala`](../tests/fixtures/jvm_tasty_reference_local_constructor_flows/Consumer.scala)
was compiled only against those three Scala-produced classes and the pinned
Scala libraries. It ran with `java -Xverify:all`, checked `create(7)`,
`create(-9)`, and `create(3)`, then checked two separately returned Counter
references and independent mutable state. It printed
`scala39-reference-local-constructor-flow=ok`. This verifies Scala output and
is not Rust-to-Scala interoperability.

## TASTy wire tree and cross-representation identities

`CounterOwnerFactory.tasty` decodes as TASTy 28.9.0. For `create`, the visible
addresses include:

| Address | Tag | Node |
| ---: | ---: | --- |
| 64 | 130 | `DEFDEF create` |
| 67 | 134 | `PARAM seed` |
| 80 | 140 | `BLOCK` |
| 82 | 136 | result `APPLY` for the Owner constructor |
| 84 | 176 | selected Owner constructor (`SELECTin`) |
| 96 | 129 | immutable `VALDEF counter: Counter` |
| 103 | 136 | local initializer `APPLY` |
| 105 | 176 | selected `makeCounter` (`SELECTin`) |

The block payload stores its expression before its stats. Therefore the
outer-constructor `APPLY` appears on the wire before the local `VALDEF`, even
though JVM execution evaluates the initializer first. The constructor's local
argument is a `TERMREFdirect` whose address points to the `counter` `VALDEF` at
96. The initializer remains an application of the signed `makeCounter(Int):
Counter` member. The decoder regression checks the local reference target,
same-block parent, method signature, all three TASTY UUID links, and all three
classfile major versions.

The Rust source observation uses one Lexer → Parser → Namer → `SourceTyper`
session for the unchanged fixture and `SourceId(1849)`. The IDs below are the
observed allocation in that test only; they are not stable public identifiers.

| Meaning | Source identity | Typed identity / type |
| --- | --- | --- |
| Factory class | `SymbolId(42)` | same canonical source class |
| `create` method | `SymbolId(57)` | body type is `CounterOwner` |
| `makeCounter` selection | source `Ident` in `TreeId(49)` | canonical `SymbolId(56)`; result type `Counter` |
| Local declaration | `TreeId(50)` | `TreeId(18)`, `SymbolId(60)`, owned by `create`, type `Counter` |
| Local initializer Apply | `TreeId(49)` | `TreeId(16)`; source-to-typed index retains this exact edge |
| Block method body | `TreeId(56)` | `TreeId(24)` |
| Final local use | `TreeId(54)` | `TreeId(22)`, `TermRef(SymbolId(60))` |

The frontend keeps the original block, `ValDef`, initializer call, final
constructor application, and local `Ident`. Before measuring body validation,
the test types both class-field initializers and each method body in source
member order in this same session, matching the paired planner's frontend
typing path. `Typer::typed_local_binding_at`
returns the local symbol and its exact typed declaration, keyed by the original
source `TreeId`; the typed reference in the constructor argument points to that
same `SymbolId(60)`. The helper selection retains canonical method
`SymbolId(56)`. No local symbol or typed tree is reconstructed from spelling.

| Stage | Observed result for the unchanged local-block source at the #1849 audit HEAD |
| --- | --- |
| Lexer, Parser, Namer, Typer | Succeed in the single source session; source/typed local and method links are retained. |
| Cross-class constructor proof scan | Completes without error. The scan retains the helper's existing proof but no `create` proof, because `create` returns a `Block` rather than the bounded direct constructor-call shape. |
| Same-session class-state TASTy plan | Signature/member validation succeeds from the recorded session's store, source AST/index, typed AST, and source-to-typed index. |
| Same-session TASTy method-body validation | `BodyType { tree: TreeId(24), ty: TypeId(47) }`, asserted against the exact recorded typed block and its type. These IDs are run-local. |
| Separate paired end-to-end control | A fresh `with_paired_jvm_tasty_batch_plan` run independently reaches `JVM/TASTy cross-contract validation` / `TastyPlan(BodyType { tree: TreeId(24), ty: TypeId(46) })`. The matching tree number is coincidental; its arena and type IDs are not the measured session's identities. |
| TASTy byte emitter and paired artifact publisher | Not reached in either body-validation run. A complete validated method-body plan is required first; no Rust block TASTy artifact was manufactured. |
| Separate JVM-only source backend | Succeeds for the same source shape, emits all three classes, and keeps `makeCounter:(I)Lexample/Counter;` and `CounterOwner.<init>:(Lexample/Counter;)V`. The method uses local slot 2. |
| JDK 21 verification/execution of Rust JVM classes | Succeeds with `-Xverify:all`; the Java harness checks all three seed values, distinct Counter references, and state isolation. This is separate from paired TASTy output. |

The exact same-session validation, local identity assertions, and separately
labeled paired control are in
`reference_local_constructor_flow_preserves_one_session_identity_through_body_validation`
in [`jvm_tasty_factory_result_arguments.rs`](../tests/jvm_tasty_factory_result_arguments.rs).
The independent backend check is
`reference_local_constructor_flow_emits_and_executes_on_the_jvm_only_path`
in [`jvm_source_compilation.rs`](../tests/jvm_source_compilation.rs).

## Controls and bounded conclusions

| Control | Evidence or disposition |
| --- | --- |
| Rename package, classes, and factory method while retaining the local block | Measured through the real paired planner. Frontend planning reaches the same `TastyPlan(BodyType)` gate. |
| Reorder the three class declarations while retaining the local block | Measured through the real paired planner. The first failure remains `TastyPlan(BodyType)`. |
| Direct formal, nested `new`, and direct factory-result constructor arguments | Existing linked regressions cover these distinct accepted forms, including Scala 3.9 readers and JDK execution. They do not establish block-local paired TASTy support. See [factory-result arguments](jvm-tasty-factory-result-arguments.md) and [nested constructor calls](jvm-tasty-nested-constructor-calls.md). |
| Helper shadowing, private/overloaded/generic/defaulted helper, wrong arity/result, external helper, and conditional multi-call forms | Measured by the existing first-gate controls in `jvm_tasty_factory_result_arguments.rs`; these cases fail at their asserted source or member gate. |
| Wrong source/typed call-edge binding, foreign same-named helper, wrong owner/result/receiver, missing member, malformed JVM invocation, and write failures | Existing focused proof, batch-contract, and publisher regressions check these direct-call paths. They are not local-block receipt tests. |
| Missing local initializer, mutable `var`, local declared after use, wrong local type, local shadowing, duplicated helper evaluation, nested local blocks, mixed `If`/loop blocks, foreign/cross-method local, and corrupted local source/typed bindings | Deferred at the #1849 audit HEAD. Issue #1850 adds proof-stage controls for bounded malformed local-block shapes; see below. |
| Unrelated/external classes and wider Scala ABI forms | Outside this single-source audit. No cross-unit or external-reference support is claimed. |

## Issue #1850: proof-only immutable local receipt

Starting from `fa8b0afa9a859b3f634bcafebaaf000713a63725`, the root
cross-class proof scanner now recognizes the unchanged original
`Block → ValDef → makeCounter(seed) → new CounterOwner(counter)` shape. The
receipt keeps the original source and typed `Block`, declaration, initializer,
constructor application, and final local `Ident`. Its local component retains
the `TypedLocalBinding`'s method-owned `SymbolId`, exact block `ScopeId`, typed
declaration, canonical `TypeRef(Counter)`, and source-to-typed edges. The nested
initializer receipt is the existing same-instance `PlannedCall` proof for
`makeCounter(seed)`, preserving the original seed formal and method plan.

The scanner accepts exactly one explicitly typed immutable local with an
initializer and a final direct constructor call. It checks the exact scope
membership, method owner, local kind and mutability, declaration-before-use
source positions, exact typed local reference, and initializer/result class
identity. The Typer-owned binding is supplied only by the same-session paired
planner; a disconnected source-to-typed index is rejected before scanning.
The scanner does not synthesize or rewrite trees.

`reference_local_constructor_proof_rejects_unsupported_block_shapes` runs
mutable, multiple and duplicate locals; nested blocks; branch-controlled
initializer and use; missing initializer; pre-declaration use; reassignment;
parameter shadowing; wrong declared type; a parameter used instead of the local
constructor argument; and locals from another method or nested scope through
the real frontend and paired planner. Mutable, multiple, nested, and branch
forms fail at the cross-class proof stage. Duplicate, uninitialized,
pre-declaration, reassigned, shadowing, wrong-type, parameter-substitution,
and out-of-scope cases fail earlier during method body typing or source-shape
validation. A foreign `SourceId` fails at the source-root identity check. The
positive renamed and reordered controls continue to pass the proof stage and
reach the existing later TASTy `BodyType` rejection. A cloned/disconnected
`SourceTypedIndex` is also rejected at the session-index identity check before
proof scanning.

This receipt proves source/typed constructor argument provenance only. The
unchanged paired TASTy method-body validator still rejects the original block
at `BodyType`, before byte serialization or artifact publication. No TASTy
body validation/encoder, JVM emitter, Typer implementation, or CLI behavior
was changed, and no Rust local-block artifact or emission support is claimed.

### #1850 validation receipt

The new proof regression runs the exact source through two separate frontend
sessions (`SourceId(1849)` and `SourceId(1850)`) and compares the resulting
proof relationships by source/typed roots, helper receipt, canonical names,
and method-owned scope. Both sessions independently retain the local proof;
both still reach the same downstream `BodyType` boundary.

| Validation | Result |
| --- | --- |
| `cargo test -p dotty --test jvm_tasty_factory_result_arguments --locked` | Passed; 12 tests, including unsupported local-block first gates and two-session proof relationships. |
| Pinned Scala 3.9 factory-result producer and consumer tests | Passed with source revision `777528f19a58e794c9954a42f433373472ec57f8`, verified release artifact hashes, and Temurin JDK 21.0.13. |
| Strict JDK21 classfile/JVM/source/stateful gate and local-flow JVM-only regression | Passed with `DOTTY_CLASSFILE_REQUIRE_JDK=1`. |
| Workspace formatting, frontend boundary guard, Clippy, docs, workspace tests, and `git diff --check` | Passed. |

No TASTy or class-file output is produced for the local-block case; the strict
Scala consumer above covers the pre-existing factory-result artifact path.

## Reproduction commands

Set `SCALA39_ROOT` to a clean checkout at the pinned revision and use Temurin
JDK 21. The repository CI workflow builds this checkout with
`sbt "dist/universal:stage"`; the staged `scalac` is then used as below. Keep
all outputs in the issue worktree.

```sh
test "$(git -C "$SCALA39_ROOT" rev-parse HEAD)" = \
  777528f19a58e794c9954a42f433373472ec57f8
test "$("$SCALA39_ROOT/dist/target/universal/stage/bin/scalac" -version 2>&1 | head -1)" = \
  "Scala compiler version 3.9.0-bin-SNAPSHOT-git-777528f -- Copyright 2002-2026, LAMP/EPFL"
mkdir -p target/oracle/producer target/oracle/consumer
test -z "$(find target/oracle/producer -mindepth 1 -print -quit)"
"$SCALA39_ROOT/dist/target/universal/stage/bin/scalac" -release 21 \
  -d target/oracle/producer \
  tests/fixtures/jvm_tasty_reference_local_constructor_flows/CounterOwnerFactory.scala
```

Compile the Scala client to a different output directory, with only the
producer artifacts and pinned Scala libraries on its classpath, then verify
and run it:

```sh
SCALA_LIB="$SCALA39_ROOT/dist/target/universal/stage/maven2/org/scala-lang/scala3-library_3/3.9.0-bin-SNAPSHOT/scala3-library_3-3.9.0-bin-SNAPSHOT.jar:$SCALA39_ROOT/dist/target/universal/stage/maven2/org/scala-lang/scala-library/3.9.0-bin-SNAPSHOT/scala-library-3.9.0-bin-SNAPSHOT.jar"
"$SCALA39_ROOT/dist/target/universal/stage/bin/scalac" -release 21 \
  -classpath "target/oracle/producer:$SCALA_LIB" -d target/oracle/consumer \
  tests/fixtures/jvm_tasty_reference_local_constructor_flows/Consumer.scala
java -Xverify:all -classpath "target/oracle/consumer:target/oracle/producer:$SCALA_LIB" \
  example.FactoryConsumer
javap -v -p -c -classpath target/oracle/producer example.CounterOwnerFactory
```

Run the focused Rust observations and mandatory JDK gate with JDK 21 first on
`PATH`:

```sh
cargo test -p dotty --test jvm_tasty_factory_result_arguments \
  reference_local_constructor_flow --locked -- --nocapture
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test -p dotty --test jvm_source_compilation \
  reference_local_constructor_flow_emits_and_executes_on_the_jvm_only_path \
  --locked -- --nocapture
```

The inherited strict pinned Scala consumer and JDK workflow is
`.github/workflows/jvm-tasty-factory-scala-consumer.yml`; its path filter
includes this fixture, documentation, and tests. The full hosted Scala
compatibility corpus remains out of scope.

## Validation receipt

This audit starts from `origin/main` commit
`2c9303ea65ab7c6c9d6994cc98be6620906a5bc3`. The pinned Scala source build,
fresh producer compile, six-file inventory/hash/UUID check, Scala-only client,
and `javap` inspection all completed with Temurin 21.0.8 and Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`. The consumer printed
`scala39-reference-local-constructor-flow=ok` under `-Xverify:all`.

| Validation | Result |
| --- | --- |
| Focused Rust local-flow identity, wire-tree, rename/reorder controls | Passed; source/typed links and local `TERMREFdirect → VALDEF` are asserted. |
| Same-session Rust body-validation measurement | Passed; `BodyType` is asserted against the original typed block and its exact type after field initializers and method bodies have been typed in source member order. |
| Separate paired end-to-end control | Passed; a fresh frontend session independently reaches `JVM/TASTy cross-contract validation: TastyPlan(BodyType { .. })`. Its arena IDs are not used as evidence for the recorded-session audit. |
| JVM-only local-flow backend regression | Passed; 3 classes emitted, descriptors/opcodes/stack/local bounds asserted, JDK21 `-Xverify:all` harness executed. |
| Full `jvm_tasty_factory_result_arguments` integration test with required pinned Scala reader | Passed, 11 tests. |
| Inherited strict JDK21 classfile/JVM/source/TASTy-artifact/stateful gate | Passed with `DOTTY_CLASSFILE_REQUIRE_JDK=1`. |
| `cargo fmt --all -- --check` | Passed. |
| `python3 tools/check-frontend-boundaries.py` | Passed, including self-test. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed. |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` | Passed. |
| `DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test --workspace --all-targets --locked` | Passed. |
| `git diff --check` and manifest hash/file-inventory verification | Passed. |

The root proof-planning behavior now recognizes the bounded local-block
constructor flow and retains its canonical session identities. No Rust paired
class/TASTy output or JVM emission is claimed for the local-block source. The
local-body negative matrix marked deferred above remains unmeasured because
the authentic body validator rejects before serialization. The full hosted
Scala compatibility corpus was not run.

## Issue #1851: bounded pickler body validation

Issue [#1851](https://github.com/scytrowski/dotty-rs/issues/1851) adds a
validation-only path for the exact local constructor flow. The old audit result
above records the pre-change `BodyType` gate; it is superseded for the
standalone `ClassStatePicklePlan::validate_method_bodies` API by the evidence
below.

The accepted body is one original method `Block` with exactly one immutable,
explicitly typed local `val` followed by a direct `CounterOwner` constructor
application whose sole argument is the exact local `Ident`. The original block
and final constructor `Apply` must have the same exact `TypeId`, equal to the
selected primary constructor's complete signature result: `TypeRef(NoPrefix,
CounterOwner)`. This no-prefix constructor result is intentionally distinct
from the enclosing method's package-prefixed `CounterOwner` result type. The
method result is validated separately as a canonical public same-batch class
`TypeRef`; the constructor result, method result, typed `New`, explicit type
tree, selected constructor prefix, and owner symbol must all identify that
same canonical class through the original source/typed bindings and planned
class provenance. A class target match alone is insufficient, and arbitrary
prefixes are rejected.

The local declaration, its type syntax, initializer call, and final use must
each retain their original source-to-typed binding, matching source span, and
session identity. Its method-owned local symbol and block scope come from the
same-session local binding. The local type syntax must name a canonical public
same-batch class; the initializer must be the exact helper `PlannedCall`
receipt, and its canonical result class must agree with both the local and
constructor formal. The helper's owner, symbol, source definition, result
signature and caller formal are checked by the existing planned-call validator.

The rule accepts original, class/method-renamed, and class-declaration
reordered source variants through separate real Lexer → Parser → Namer →
`SourceTyper` sessions. It does not depend on fixture names, declaration
allocation order, or structurally reconstructed trees. Existing Int locals,
primitive Int blocks, and the bounded direct-formal, nested-New, and
planned-call paths remain controls.

| Case | First pickler validation result |
| --- | --- |
| Frozen original, renamed classes/helper, and reordered class declarations with authentic same-session local and initializer receipts | `validate_method_bodies` succeeds. |
| Both block and final Apply carry the package-prefixed method result instead of the selected constructor's `TypeRef(NoPrefix, CounterOwner)` result | Rejected: the roots must exactly match the complete constructor signature result. |
| Block and final Apply result TypeIds disagree | Rejected before local/constructor body validation. |
| Typed `New` type tree points at another planned same-batch class while the source/constructor roots still identify `CounterOwner` | Rejected by the original source/typed type-tree and constructor provenance checks. |
| Initializer receipt points at another source root | Rejected during local initializer receipt matching; no successful body validation. |
| Local binding omitted | Rejected during local declaration validation; no successful body validation. |
| Local binding uses a different, valid scope owned by the factory class | `ScopeMembers { owner: create }`; no successful body validation. |
| Local binding uses a method symbol as its local symbol | Rejected by local symbol kind/owner validation. |
| Initializer receipt carries a stale typed Apply root | Rejected while matching the exact local initializer source/typed roots. |
| Paired batch adapter path without local receipt projection | `Unsupported(BodyExpression)` during TASTy body validation. The root adapter projection is outside this issue. |
| `var`, multiple/duplicate/uninitialized locals, forward use, reassignment, wrong type, shadowing, nested scopes/blocks, branch-controlled values, or foreign method/source bindings | Rejected earlier by the existing source proof and typing gates recorded under issue #1850; none yields a body-validation success. |
| Unrelated reference result, unsupported local initializer, or non-constructor tail expression | Rejected by the exact block shape/result checks. |

This result is body validation only. It does not create a complete
`ValidatedClassStatePlan` body output, run the expression serializer, produce
TASTy bytes, or authorize paired artifact publication. A regression separately
confirms that `pickle()` continues to return an error for this reference-local
body even when standalone body validation succeeds. The wire encoder,
`expression/block.rs`, root batch projection, JVM emitter, Typer, and CLI remain
unchanged. In particular, the paired end-to-end control now reaches the
explicit unsupported body boundary because its existing batch adapter does
not project the local receipt; standalone validation uses the authentic
same-session test adapter.

The approved [reactive scope decision](https://github.com/scytrowski/dotty-rs/issues/1851#issuecomment-6504865258)
clarifies this constructor-result versus method-result representation. It
supersedes the earlier scope record linked from the original issue comment.

### #1851 validation receipt

| Validation | Result |
| --- | --- |
| `cargo test -p dotty --test jvm_tasty_factory_result_arguments reference_local_constructor_flow_preserves_one_session_identity_through_body_validation --locked -- --nocapture` | Passed; authentic original and two independent source/typed sessions, renamed/reordered controls, stale initializer receipt, missing local receipt, wrong local scope, and paired adapter boundary. |
| `cargo test -p dotty-tasty-pickler --lib --locked` | Passed. |
| `cargo test -p dotty --test jvm_tasty_factory_result_arguments reference_local_constructor_flow_preserves_one_session_identity_through_body_validation --locked -- --nocapture` | Passed after review fix; authentic constructor-result TypeId is preserved, with negative package-prefix mismatch, root-TypeId mismatch, disconnected `New` type tree, and local/helper receipt corruptions. |

## Issue #1852: standalone TASTy for a reference local

Issue [#1852](https://github.com/scytrowski/dotty-rs/issues/1852) supersedes
the #1851 statement that standalone `pickle()` rejects this body. The existing
`ClassStatePicklePlan` path now serializes the validated source `Block`, its
original immutable reference `VALDEF`, the unchanged helper `PlannedCall`, and
the final constructor `Apply` with a direct reference to the local declaration.
The local's `IDENTtpt` is projected only for a canonical same-source class type
already registered by the validated plan. Primitive local type syntax keeps
its prior path and bytes. The writer keeps the constructor result's
`TypeRef(NoPrefix, Owner)` separate from the method result's package-prefixed
Owner `TypeRef`.

The standalone regression serializes Counter, CounterOwner, and
CounterOwnerFactory sidecars independently from the same original source
frontend session. Its decoder checks the Factory method result tree, block and
single local declaration, local class type tree, helper `SELECTin`/`APPLY` and
original seed parameter, Owner constructor selection, and exact local
`TERMREFdirect` address. Two independent sessions emit identical sidecars;
renamed and reordered class variants pass deterministic output checks. The
source and typed corruption controls reject missing helper members, local
bindings, wrong local addresses/types, and foreign owners before output; the
existing proof matrix continues to reject mutable, duplicate, nested, and
otherwise unsupported local shapes.

The pinned Scala 3.9.0 reader at revision
`777528f19a58e794c9954a42f433373472ec57f8` decompiles these Rust-produced
sidecars, reconstructs the typed local declaration and use, and accepts all
three with `-from-tasty`. These are standalone TASTy artifacts only: the
paired JVM/TASTy batch adapter still rejects the local receipt, and this issue
does not claim runtime acceptance of separately compiled Scala clients.
