# Nested constructor arguments: Scala 3.9/JVM 21 audit

Issue [#1815](https://github.com/scytrowski/dotty-rs/issues/1815) freezes the
pinned Scala output for a factory that nests `new Counter(seed)` inside
`new CounterOwner(...)`, then measures the current Rust boundaries. This is an
audit only. It does not enable paired JVM/TASTy emission for this body.

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
| `ValidatedClassStatePicklePlan::pickle` | Fails with `Unsupported { feature: BodyExpression, symbol: Some(SymbolId(55)) }`; no complete Factory TASTy is returned. |
| Paired JVM/TASTy batch | The bounded source/typed constructor proof succeeds. The batch then fails at `JVM/TASTy cross-contract validation` with `TastyPlan(Unsupported { feature: BodyExpression, symbol: Some(SymbolId(55)) })`; no paired batch or bytes are returned. |
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

This increment changes the structural proof only. It does not serialize the
nested body into TASTy, emit JVM bytes, change Typer behavior, or claim
Rust-produced artifact interoperability. The pinned Scala fixtures and their
hashes above remain the Scala 3.9.0 oracle and are byte-identical.
