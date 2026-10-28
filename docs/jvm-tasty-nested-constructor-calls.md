# Nested constructor arguments: Scala 3.9/JVM 21 audit

Issue [#1815](https://github.com/scytrowski/dotty-rs/issues/1815) freezes the
pinned Scala output for a factory that nests `new Counter(seed)` inside
`new CounterOwner(...)`. Issue #1817 adds bounded TASTy serialization for that
source-proven shape; this document records both the oracle baseline and the
Rust producer contract. The separate JVM output path is unchanged.

The exact one-file source is
[`NestedConstructorCalls.scala`](../tests/fixtures/jvm_tasty_nested_constructor_calls/NestedConstructorCalls.scala)
(SHA-256 `8433d71767dc73587e859b1bff982d0c699c1b008b40d26497f7f068c27c7fa4`).
The Scala-only consumer is
[`Consumer.scala`](../tests/fixtures/jvm_tasty_nested_constructor_calls/Consumer.scala).

## Rebuild the oracle

The oracle uses Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`, reporting
`3.9.0-bin-SNAPSHOT-git-777528f`, and Temurin `21.0.8+9` with `-release 21`.
The staged Scala compiler and its dependency cache must already exist. The
rebuild script fails if the source revision or JDK differs, compiles into a
caller-supplied empty directory, byte-compares all six compiler outputs with
the checked-in fixture, then compiles and executes the independent client
against those Scala-produced classes:

```sh
SCALA39_ROOT=/path/to/scala3-at-777528f \
JAVA_HOME=/path/to/temurin-21.0.8+9 \
tests/fixtures/jvm_tasty_nested_constructor_calls/rebuild-oracle.sh \
  "$PWD/target/nested-constructor-oracle"
```

It prints `scala39-nested-constructor-oracle=ok`. This is an official Scala
client reading official Scala artifacts; it is not Rust interoperability
acceptance. `javap` inspection used:

```sh
"$JAVA_HOME/bin/javap" -classpath target/nested-constructor-oracle/oracle \
  -v -p -c example.CounterOwnerFactory
"$JAVA_HOME/bin/javap" -classpath target/nested-constructor-oracle/oracle \
  -p -s example.Counter example.CounterOwner example.CounterOwnerFactory
```

`oracle.json` records SHA-256 for each artifact and the UUID found both in the
classfile `TASTY` attribute and its sidecar header. The three classfiles are
public and major version 65; their TASTy headers are 28.9.0.

| Class | Constructor / method ABI | Classfile SHA-256 | TASTy SHA-256 | TASTY UUID |
| --- | --- | --- | --- | --- |
| `example.Counter` | `<init>(I)V`, `get()I`, `set(I)V` | `b41dcbdb2d2e1c9f41d1e10466aed184e0a560a57414f0c71daae45b63a7bd91` | `29af379e401449734e343c5c651d709c5266d5030b40bed24316ff3661660de0` | `007c0a58da84f5e80079899c5c41291b` |
| `example.CounterOwner` | `<init>(Lexample/Counter;)V`, `read()I` | `0917f71cc66418dc8161e6caff35024a5b9abc6913f3dc74ba9614499b2b904f` | `a2456d97066929e4767633fde6daefd451f77f9f0dc1a210a8237dc1c9184ee1` | `001861af74153cee0044f99201b53e66` |
| `example.CounterOwnerFactory` | `create(I)Lexample/CounterOwner;` | `cd7fc772311f755e93448c9316c36c6141c31e5df29eb4536225ebee469ed584` | `4bcc567fd594488544eb56863e4577319ec7221fa9c32bd68d45b881ccdf4f87` | `006478f6eca9f0ec00704e7ef9f10d1f` |

The compiler emits these `Factory.create` instructions. The two allocation
sites are simultaneously live until their constructors initialize them:

| PC | Instruction | Meaning |
| ---: | --- | --- |
| 0 | `new #17 // example/CounterOwner` | Allocate the result object |
| 3 | `dup` | Keep the Owner receiver for its constructor |
| 4 | `new #19 // example/Counter` | Allocate the nested argument |
| 7 | `dup` | Keep the Counter receiver for its constructor |
| 8 | `iload_1` | Load `seed` |
| 9 | `invokespecial #22 // Counter.<init>:(I)V` | Initialize Counter |
| 12 | `invokespecial #25 // CounterOwner.<init>:(Lexample/Counter;)V` | Initialize Owner with that Counter |
| 15 | `areturn` | Return Owner |

The bytecode PCs, constant-pool references, and exact bytes are asserted by
`scala39_nested_constructor_oracle_pins_abi_bytecode_and_tasty_shape` in
[`jvm_tasty_reference_constructor_calls.rs`](../tests/jvm_tasty_reference_constructor_calls.rs).

## Genuine Factory TASTy body

The checked-in Factory sidecar is decoded by the Rust TASTy reader; no body or
wire nodes are synthesized. `create` has source formal `seed: Int` and result
`CounterOwner`. Its body is:

```text
APPLY(SELECTin(NEW(CounterOwner), <init>),
  APPLY(SELECTin(NEW(Counter), <init>), TERMREFdirect(create.seed)))
```

The measured offsets in this exact sidecar are local TASTy addresses:

| Node | Address | Wire tag (decimal / hex) |
| --- | ---: | ---: |
| `create` DEFDEF | 30 | 130 / `0x82` |
| `seed` PARAM | 33 | 134 / `0x86` |
| outer Owner APPLY | 48 | 136 / `0x88` |
| outer Owner SELECTin | 50 | 176 / `0xb0` |
| outer Owner NEW | 53 | 95 / `0x5f` |
| inner Counter APPLY | 60 | 136 / `0x88` |
| inner Counter SELECTin | 62 | 176 / `0xb0` |
| inner Counter NEW | 65 | 95 / `0x5f` |
| inner seed reference | inline in argument tree | TERMREFdirect 62 / `0x3e`, address 33 |

The selected signed constructor names decode to
`<init>(example.Counter)example.CounterOwner` and
`<init>(scala.Int)example.Counter`. The `seed` PARAM type syntax resolves to
`scala.Int`; the result type syntax resolves to `example.CounterOwner`. The
Rust test asserts both nested application shapes, both selected constructor
signatures, both `NEW` nodes, and the direct reference to the method's actual
`PARAM` address. Offsets are documented to reproduce this artifact only; they
are not semantic identities or stable API values.

## Rust single-session evidence and first failures

The test `nested_constructor_rust_first_failures_preserve_the_one_session_trees`
feeds the unchanged fixture once through Lexer → Parser → Namer → SourceTyper.
For the recorded source ID 181501, the exact original declarations resolve to:

| Identity | Observed ID |
| --- | ---: |
| `Counter`, `CounterOwner`, `CounterOwnerFactory` | `SymbolId(39)`, `SymbolId(40)`, `SymbolId(41)` |
| Their primary constructors in that order | `SymbolId(43)`, `SymbolId(50)`, `SymbolId(54)` |
| `CounterOwnerFactory.create`, `create.seed` | `SymbolId(55)`, `SymbolId(56)` |
| Package source root / Factory RHS | `TreeId(51)` / `TreeId(45)` |
| Owner `New` / nested Counter `New` | `TreeId(38)` / `TreeId(41)` |
| Typed Factory RHS / original seed argument | `TreeId(8)` / `TreeId(6)` |

The source-to-typed map is asserted for the original outer and inner APPLY,
SELECT, NEW, type trees, and seed argument. The typed constructor selections
resolve to the exact session constructors `SymbolId(50)` and `SymbolId(43)`;
the innermost argument resolves to `create.seed` (`SymbolId(56)`). These arena
numbers describe the regression's deterministic allocation, not a public ID
contract.

The layers reject at different points:

| Operation on this source/session | First observed result |
| --- | --- |
| Lexer, parser, namer, typer | Succeeds; the nested body and canonical constructor/parameter identities are preserved. |
| `ClassStatePicklePlan::validate` | Succeeds; the supplied source, typed trees, and source-to-typed bindings validate. |
| `ValidatedClassStatePicklePlan::pickle` before #1817 | Failed with `Unsupported { feature: BodyExpression, symbol: Some(SymbolId(55)) }`; no complete Factory TASTy was returned. |
| Paired JVM/TASTy batch before #1817 | The bounded source/typed constructor proof succeeded, then the batch failed at `JVM/TASTy cross-contract validation` with `TastyPlan(Unsupported { feature: BodyExpression, symbol: Some(SymbolId(55)) })`. |
| JVM-only source emission | The established separate source/JVM path succeeds for a two-site nested constructor argument; it does not create TASTy or relax the paired proof. |

For the final row, the test
`exact_tasty_audit_source_emits_nested_owner_factory_on_the_jvm_only_path`
in [`jvm_source_compilation.rs`](../tests/jvm_source_compilation.rs) compiles
the exact fixture through the separate JVM-only pipeline, checks both
constructor references and executes it under `java -Xverify:all`. The existing
`nested_new_constructor_arguments_lower_lifo_and_verify_initialized_values`
test additionally asserts LIFO constructor invocation, two separate
allocations, deterministic output, and Java verification/execution for the
general nested-allocation control. The proof change does not alter that JVM
path or allow TASTy bytes to be published.

## Control matrix

The controls below distinguish this nested source from established controls
for the prior direct-reference path and JVM-only backend. The nested Factory
source now reaches the TASTy cross-contract boundary after the source proof;
it still does not produce TASTy bytes.

| Control | Evidence / first status |
| --- | --- |
| Rename package and all classes | The new real-session proof test accepts the structurally identical shape with renamed package/classes/method. |
| Reorder Counter and CounterOwner declarations | The new real-session proof test accepts the same shape when declarations are reordered; source identity is not inferred from declaration order. |
| Direct reference-formal constructor argument | Existing positive paired/TASTy and artifact tests in `jvm_tasty_reference_constructor_calls.rs`; this remains supported. |
| Constructor arity/type mismatch | This exact source's Owner arity mismatch, Counter argument type mismatch, and Owner argument type mismatch are rejected during method-body typing. Existing direct-path tests `zero_argument_constructor_call_is_rejected_by_method_body_typing` and `wrong_constructor_argument_type_is_rejected_by_method_body_typing` cover the same failure classes. JVM-only equivalents are also covered in `unsupported_counter_object_source_cases_fail_at_their_pipeline_stage`. |
| Private, abstract, generic, secondary, contextual (`using`), and defaulted constructors | Existing paired/direct-path tests record rejection at cross-class proof, method typing, source collection, member planning, or constructor planning as appropriate. No such case is accepted by this audit. |
| Stale source-to-typed RHS mapping | Existing `distinct_signature_rejects_a_rolled_back_source_rhs_id` and related provenance tests reject stale/missing/foreign/mismatched roots. These are proof-integrity tests on the supported direct shape. |
| Third concurrently live uninitialized allocation | Existing `third-simultaneous-nested-allocation` case is rejected in JVM code generation with `UnsupportedJvmConstructorArgument`. |
| Branch-valued constructor argument | Existing nested branch control is rejected in JVM code generation with `UnsupportedJvmConstructorArgument`; the source proof accepts only the direct nested application shape and rejects a conditional at `SourceShape`. |
| Deeper nested allocation | Existing two-allocation JVM-only control remains supported. The source proof rejects a third constructor depth at `SourceShape`; backend behavior remains independently bounded. |
| Missing or corrupt sidecar | Artifact publisher tests reject malformed sidecar input before publishing a partial destination; this is independent of constructor-expression acceptance. |
| Foreign same-name class / constructor from another owner | Not synthesized or measured: doing so would forge semantic identity outside the required single real frontend session. Name equality is not evidence of same-session ownership. |

## Bounded source proof added by #1816

The cross-class constructor-call proof now accepts either its established
direct argument (`Apply(Select(New(Owner), <init>), formal)`) or exactly one
nested application as that argument:

```text
Apply(Select(New(Owner), Owner.<init>),
  Apply(Select(New(Counter), Counter.<init>), Factory.create.seed))
```

The validator reads the original source and typed arenas and requires every
node to retain the exact `SourceTypedIndex` mapping, source position, and
session identity. It proves the caller method's signed `Int` parameter and
`Owner` result; the outer constructor's canonical `Counter` formal; the inner
constructor's canonical `Int` formal; both selected primary-constructor
symbols and owners; and the original `Factory.create.seed` `TermRef`. It also
requires the inner class to be another ordinary class in the exact package and
ordered JVM batch. Class names, file order, and allocation order do not select
the shape.

`ValidatedCrossClassConstructorCallProof::nested_argument()` returns a
copyable read-only receipt for the inner `Select`, `New`, leaf argument,
constructor, class, formal, and canonical formal/result types. The existing
`source_argument()` and `typed_argument()` remain the original outer argument
roots; for a nested proof these are the inner `Apply` itself. The proof's
`formal_type()` is the factory's `Int` parameter type, while
`constructor_formal_type()` is the outer constructor's `Counter` parameter
type. These are distinct semantic roles and are not collapsed.

The generic proof continues to accept the direct reference-formal constructor
call and the `Factory(Int)` constructor-return control. Real-session tests also
rename the package/classes/method and reorder class declarations. The narrow
proof rejects malformed nested shapes at their first relevant stage:

| Invalid input | First error type / stage |
| --- | --- |
| Literal or non-formal nested leaf | `CrossClassConstructorCallProofError::SourceShape` |
| Nested apply with wrong arity or non-primary function | `SourceShape` |
| Missing or mismatched nested source/typed binding | `TypedBinding` |
| Foreign/stale inner class, constructor, or formal identity | `SourceIdentity` or `SemanticIdentity` |
| Non-canonical nested or outer result/formal type | `TypedBinding` or `SemanticIdentity` |
| More than one nested allocation level, conditional, or method-result argument | `SourceShape` |

Existing stale/mismatched direct-root mapping regressions continue to exercise
the fail-closed source/typed binding checks. Unsupported expression shapes do
not produce a proof receipt.

## Bounded TASTy serialization added by #1817

The paired batch passes the immutable nested receipt into the pickler's
source/typed binding contract. Before serialization, the pickler rechecks the
original outer Apply, inner Apply/Select/New nodes, the inner constructor
owner and one-`Int` formal, the nested result class, the outer constructor's
matching class formal, the caller method's exact `seed` TermRef, and each
source-to-typed mapping and span. The writer then recursively emits those
original nodes as `APPLY(SELECTin(NEW(Owner)),
APPLY(SELECTin(NEW(Counter)), TERMREFdirect(create.seed)))`. It registers the
inner class as a same-source sibling for the signed constructor selection; the
constructor declaration remains in its own sidecar.

`nested_constructor_proof_serializes_the_original_nested_tasty_expression`
asserts both applications and the direct reference to the actual `PARAM` node.
The renamed/reordered source regression and existing direct-reference and
primitive constructor cases remain covered. With
`DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1`,
`pinned_scala_reads_and_from_tasty_compiles_the_rust_nested_factory_sidecar`
runs the pinned Scala 3.9 `-decompile` and `-from-tasty` checks against all
three Rust-produced sidecars.

The accepted expression is exactly one nested primary constructor whose
single argument is the original factory's `Int` formal. A third constructor
level, branch or other computed argument, field/local/factory/null input,
generic/default/using/secondary constructors, and forged or stale identities
remain rejected before complete sidecar output. The change does not alter
Typer or JVM emission and does not claim client runtime interoperability for
Rust-produced artifacts. Scala oracle fixture hashes above remain unchanged.

## Six-artifact paired integration (#1818)

The paired source-batch emitter now accepts this exact source shape after both
producer outputs have been encoded. Its post-encoding cross-class invocation
validator uses the already validated one-session nested constructor proof and
the checked JVM metadata to verify the Factory method's complete straight-line
sequence:

```text
new CounterOwner; dup; new Counter; dup; iload_1;
invokespecial Counter.<init>:(I)V;
invokespecial CounterOwner.<init>:(LCounter;)V; areturn
```

The byte-level check requires both allocation class references and both
constructor `Methodref`s in evaluation order, with descriptors matching the
Counter, Owner, and Factory plans. Existing direct constructor forms continue
through their previous validator. Batch artifact validation checks all
expected package-relative paths, class-file major version 65, TASTy 28.9.0,
each class's own `TASTY` UUID link, and sidecar bytes against the validated
class plan before returning the six-artifact batch.

`nested_constructor_factory_emits_six_linked_artifacts_from_one_session`
checks all three genuine classfile/sidecar pairs, their paths and UUIDs, the
two constructor references in Factory bytecode, and byte-for-byte determinism
across fresh source sessions. The publisher regression reuses the six-file
staging and promotion fault matrix for this nested batch. A contract-level
corrupted-byte regression changes the inner constructor call to the outer
constructor's `(LCounter;)V` Methodref and verifies that the artifact
validator rejects the batch at `cross-class invocation` before a manifest can
be returned. A second corruption sets the Factory method's `Code.max_stack`
from five to zero; the validator rejects it because the proven instruction
sequence requires five operand-stack slots at its peak.

The publisher tests cover handled staging and promotion failures; they do not
claim multi-file crash-atomic durability.

## Strict JDK 21 runtime and identity acceptance (#1819)

`nested_constructor_factory_executes_original_rust_artifacts_on_jdk21` emits
the six files from the unchanged `NestedConstructorCalls.scala` source through
one `emit_paired_jvm_tasty_batch` session. It writes the resulting three class
files and three sidecars to a Rust-only classpath; `javac` writes only the
separate Java harness directory. The test checks classfile major version 65,
the exact `create(I)Lexample/CounterOwner;` descriptor, `max_stack = 5`, and
the decoded two-site instruction and constant-pool sequence before launching
`javap -v -p -c` and `java -Xverify:all -ea`. The JDK check requires all three
tools to report major version 21 exactly; a newer JDK is not accepted.
For the Rust-side verifier pass, the test reconstructs the typed instruction
list from the decoded `Factory.create` bytes and their constant-pool operands,
then calls `dotty_classfile::bytecode::analyze`; it asserts the required stack
and local bounds are five and two.

The harness calls `create` with `7`, `-9`, and `3`, checks the corresponding
`read()` values, and uses reflection only to read private `CounterOwner.inner`.
It confirms three distinct Owner objects each hold a distinct initialized
`Counter`, then mutates the first Counter to `42` and `-9` and the second to
`100`, checking that the other two object graphs keep their values. The
expected harness line is:

```text
nested-runtime=7,-9,3; independent=true; mutation=42,-9,100
```

Observed on Temurin `21.0.13+9` (`javac 21.0.13`, `javap 21.0.13`): the test
passed under `java -Xverify:all -ea` and printed the expected harness line.
The six producer artifacts had these SHA-256 digests before harness compilation;
the test confirmed the same values after compilation and after runtime:

| Original Rust artifact | SHA-256 |
| --- | --- |
| `example/Counter.class` | `c26c7cc678521d1497c2fa7b6c0071380d347f63635081a05bc7a5abd2d20851` |
| `example/Counter.tasty` | `ee4e2afc9c7afea1acadf178722ee80f8662c5fd6259a946174b7ec268178dd8` |
| `example/CounterOwner.class` | `05ac0006ccb7720073eb1aade6ca932d22bc0af1f1aa76f353a2484332071c28` |
| `example/CounterOwner.tasty` | `198a377a4f7e00b38c5a5173f6a8381abbbf8dd394fa4030ecb20fe96909b50d` |
| `example/CounterOwnerFactory.class` | `1e4495ec5abfb53ada5f93c6d561c6a288cc1d8b46ab7f90a46169de47d0ae8c` |
| `example/CounterOwnerFactory.tasty` | `bb5b3d8fc1ac369139a1660b2c06aebf614effb81bcddd035c680b405268f997` |

The test records SHA-256 for each producer artifact before compiling the
harness, then checks all six hashes after `javac` and again after the strict
runtime. `javap` only inspects the Rust classfiles. This acceptance does not
compile, copy, or load Scala-produced classes and makes no Scala TASTy client
interoperability claim.

Reproduce it with a JDK 21 SDKMAN candidate selected explicitly. For example,
after installing Temurin 21.0.13, run from the repository root:

```sh
source "$HOME/.sdkman/bin/sdkman-init.sh"
sdk use java 21.0.13.0+9-tem
export TMPDIR="$PWD/target/issue-1819-tmp"
export CARGO_TARGET_DIR="$PWD/target/issue-1819"
mkdir -p "$TMPDIR"
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test --test jvm_tasty_artifacts \
  nested_constructor_factory_executes_original_rust_artifacts_on_jdk21 \
  --locked -- --nocapture
```

The required environment flag makes the test fail if any tool is missing or
is not JDK 21. The test intentionally reports the six source-artifact hashes
so a review receipt can record the exact run. The Scala oracle tree and its
compiled classes are not on either classpath.

Negative cases remain separated by the component that rejects them:

| Control | First rejection | Evidence |
| --- | --- | --- |
| Inner constructor `Methodref` changed to the Owner constructor reference, giving the wrong owner and descriptor for the `Counter` allocation | Paired artifact contract, phase `cross-class invocation`, before a batch manifest is returned | `nested_constructor_batch_contract_rejects_wrong_constructor_reference_and_stack_bound` in `tests/jvm_tasty_contract.rs`. |
| Constructor argument descriptor does not match the initialized class formal | Classfile analysis (`WrongExactMethodArgument`) before a classfile is encoded | `rejects_wrong_reference_field_value_and_constructor_argument` in `crates/dotty-classfile/tests/class_file_builder.rs`. |
| Incomplete initialization or an uninitialized alias escaping to ordinary bytecode use | Classfile analysis rejects malformed plans; the deliberately malformed runtime class is rejected by the JVM as `VerifyError` | `exact_reference_returns_reject_wrong_classes_and_uninitialized_values` in `crates/dotty-classfile/tests/class_file_builder.rs`; `reference_constructor_factory_executes_original_rust_object_graph_on_verified_jvm` mutates a linked class and asserts JVM `VerifyError`. |
| Unauthorized use of `UninitializedThis` | Classfile analysis returns `ConstructorReceiverNotUninitialized` before encoding | `class_builder_verifies_direct_super_invokespecial_and_requires_acc_super` in `crates/dotty-classfile/tests/class_file_builder.rs`. |
| More than two simultaneously live `new` sites | JVM codegen returns `UnsupportedJvmConstructorArgument`; no class batch is returned | `third-simultaneous-nested-allocation` in `tests/jvm_source_compilation.rs`. |
| Wrong `Fieldref` value type or invalid reference method argument | Classfile analysis returns `WrongExactFieldValue` or `WrongExactMethodArgument` | `rejects_wrong_reference_field_value_and_constructor_argument` and `linear_virtual_calls_use_exact_reference_analysis_without_reference_signature` in `crates/dotty-classfile/tests/class_file_builder.rs`. |
| Required Factory stack bound changed from five to zero | Paired artifact contract, phase `cross-class invocation`, before a batch manifest is returned | `nested_constructor_batch_contract_rejects_wrong_constructor_reference_and_stack_bound` in `tests/jvm_tasty_contract.rs`. |
| Partial class/TASTy batch publication | Publisher rejects before promotion and removes staging output | `reference_constructor_batch_publisher_contains_all_six_artifact_failures` in `tests/jvm_tasty_artifacts.rs`. |

These stages are intentionally not conflated: `AnalysisError` and paired
contract errors are Rust-side pre-publication rejections; `VerifyError` is the
JVM's result after loading a deliberately corrupted classfile. None of these
negative controls changes producer behavior or publishes a partial artifact
batch. Branch-valued arguments, deeper nested `New`, foreign classes, generic
or secondary constructors, and unplanned classpath members remain unsupported
at their earlier documented validation stages.
