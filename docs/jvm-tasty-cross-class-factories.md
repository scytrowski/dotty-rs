# Scala 3.9 cross-class factory TASTy support

This document records the Scala/JVM audit requested by [issue #1690](https://github.com/scytrowski/dotty-rs/issues/1690), the bounded TASTy producer slice added by [issue #1693](https://github.com/scytrowski/dotty-rs/issues/1693), and its complete linked batch integration from [issue #1694](https://github.com/scytrowski/dotty-rs/issues/1694). The source typer and JVM bytecode writer remain unchanged.

## Pinned source and oracle

The Scala source checkout was at `777528f19a58e794c9954a42f433373472ec57f8` (`3.9.0-bin-SNAPSHOT-git-777528f`), and compilation/runtime checks used Temurin JDK `21.0.8+9`. Scala was invoked with `-release 21`, producing classfile major version 65. The source-pipeline fixture [`CounterFactory.scala`](../tests/fixtures/jvm_tasty_cross_class_factories/CounterFactory.scala) now contains both the issue's `create` method and its independent `identity` method in the same class. The pinned Scala oracle artifacts retain `CounterIdentity.scala` as a separate identity-only control, allowing the existing ABI and TASTy evidence to remain independently inspectable.

The Scala 3.9 compiler revision and Java runtime were verified directly for the #1693 consumer gate:

```text
Scala compiler: 3.9.0-bin-SNAPSHOT-git-777528f
Scala revision: 777528f19a58e794c9954a42f433373472ec57f8
Java runtime:   Temurin 21.0.8+9 (2025-07-15)
```

The pinned source-oracle evidence below remains reproducible from the staged Maven artifacts:

```sh
SCALA_ROOT=/home/vdrake6/dotty-rs/.worktrees/scala39-source
JAVA_HOME=/home/vdrake6/.jdks/jdk-21
export JAVA_HOME
export PATH="$JAVA_HOME/bin:$PATH"
SCALA_CP="$(find "$SCALA_ROOT/dist/target/universal/stage/maven2" -type f -name '*.jar' -print | paste -sd:)"
SCALA_CP="$SCALA_CP:$SCALA_ROOT/library/target/scala-library-bootstrapped/scala-library-3.9.0-bin-SNAPSHOT.jar"
SCALA_CP="$SCALA_CP:$SCALA_ROOT/library/target/scala3-library-bootstrapped/scala-3.9.0-bin-SNAPSHOT-nonbootstrapped/scala3-library_3-3.9.0-bin-SNAPSHOT.jar"
SCALA_CP="$SCALA_CP:$SCALA_ROOT/tasty/target/tasty-core-bootstrapped/scala-3.9.0-bin-SNAPSHOT-nonbootstrapped/tasty-core_3-3.9.0-bin-SNAPSHOT.jar"
OUT=/tmp/issue-1690-oracle
mkdir -p "$OUT/classes" "$OUT/client"
java -cp "$SCALA_CP" dotty.tools.MainGenericCompiler \
  -classpath "$SCALA_CP" -release 21 -d "$OUT/classes" \
  tests/fixtures/jvm_tasty_cross_class_factories/CounterFactory.scala \
  tests/fixtures/jvm_tasty_cross_class_factories/CounterIdentity.scala
java -cp "$SCALA_CP" dotty.tools.MainGenericCompiler \
  -classpath "$SCALA_CP:$OUT/classes" -release 21 -d "$OUT/client" \
  tests/fixtures/jvm_tasty_cross_class_factories/FactoryClient.scala \
  tests/fixtures/jvm_tasty_cross_class_factories/IdentityClient.scala
javap -classpath "$OUT/classes" -v -p -c example.CounterFactory
java -Xverify:all -ea -classpath "$SCALA_ROOT/library/target/scala-library-bootstrapped/scala-library-3.9.0-bin-SNAPSHOT.jar:$SCALA_ROOT/library/target/scala3-library-bootstrapped/scala-3.9.0-bin-SNAPSHOT-nonbootstrapped/scala3-library_3-3.9.0-bin-SNAPSHOT.jar:$OUT/classes:$OUT/client" client.FactoryClient
java -Xverify:all -ea -classpath "$SCALA_ROOT/library/target/scala-library-bootstrapped/scala-library-3.9.0-bin-SNAPSHOT.jar:$SCALA_ROOT/library/target/scala3-library-bootstrapped/scala-3.9.0-bin-SNAPSHOT-nonbootstrapped/scala3-library_3-3.9.0-bin-SNAPSHOT.jar:$OUT/classes:$OUT/client" client.IdentityClient
```

Both clients completed successfully. `FactoryClient` called `create(7)`, observed `get() == 7`, called `set(42)`, and observed `get() == 42`. `IdentityClient` asserted that `identity(counter) eq counter`, proving it returns the original runtime object. These are Scala-produced oracle classes, not Rust-produced paired artifacts.

## JVM ABI and construction

`javap -v -p -c example.CounterFactory` reports a public class (`ACC_PUBLIC, ACC_SUPER`) and public instance methods:

| Method | Descriptor | Access | Code offsets and instructions |
| --- | --- | --- | --- |
| `create` | `(I)Lexample/Counter;` | `ACC_PUBLIC` | `0 new example/Counter`; `3 dup`; `4 iload_1`; `5 invokespecial example/Counter.<init>:(I)V`; `8 areturn` |
| `identity` (separate control class) | `(Lexample/Counter;)Lexample/Counter;` | `ACC_PUBLIC` | `0 aload_1`; `1 areturn` |

The factory call has one `CONSTANT_Methodref` to owner `example/Counter`, name `<init>`, descriptor `(I)V`. `MethodParameters` names the argument `seed`; `LocalVariableTable` maps it to slot 1, matching `iload_1`. `Counter` itself is public, its `(I)V` constructor is public, `current:I` is private, and `get`/`set` are public instance methods. The issue's exact descriptor and `new; dup; invokespecial; areturn` contract is present.

The checked-in Scala output is pinned at [`scala39/example`](../tests/fixtures/jvm_tasty_cross_class_factories/scala39/example). Its three classfiles are Java 21 major 65 and their sidecars parse as TASTy 28.9.0. Classfile TASTY links exactly match the corresponding sidecar UUIDs:

| Pair | TASTY UUID | Class SHA-256 | TASTy SHA-256 |
| --- | --- | --- | --- |
| `Counter` | `00a5635c9fca05e800deb738f3edb71b` | `eb06dbe8772e356000a600654dfae0679048aacb33e6899c3ce1d00da15a5ef7` | `86cc8fcb7f3b337035e28e36e3f72a1b8c2697ee3c7341873bbdd4100f082ec7` |
| `CounterFactory` | `00c419a25977ffef005fc36c1872301c` | `26c1cfd5925e439a0343932e060982a3c2e586e9a2b61314db05600e5ce213dc` | `520623e9ed48eb7ee981f6a47a623d0e6a56cc677e3edaf14327fcbe47221264` |
| `CounterIdentity` | `0051b70ada9c40eb00eb1d896f7ae618` | `089030ee03931cccdb904db1020ca62bf840d996b4b86d8e372e4ce573902f6b` | `d08a460dab5bb44907b2784aedd632f90a4be91ba1d54b08b340c953556c63e0` |

The method-specific TASTy walk pins the cross-class relationships, rather than just finding tags somewhere in the sidecar. In `CounterFactory.tasty`, `create` is `DEFDEF` at AST address 30 and its result type is the exact `IDENTtpt → TYPEREF → TERMREFpkg` wire shape. The name references are `Counter` qualified by `example`. The body is `APPLY(SELECTin(NEW(...)), [seed])`: the `SELECTin` signed name is `<init>` with parameter `scala.Int` and result `example.Counter`; its owner type and the `NEW` type both share the `TYPEREF` at address 44. The sole argument is `TERMREFdirect` to the `seed` `PARAM` at address 33. The classfile independently pins the same owner/name/descriptor as `Methodref example/Counter.<init>:(I)V`.

`CounterIdentity.tasty` is decoded separately. Its `identity` `DEFDEF` is at address 30, the `Counter` result type shares the `TYPEREF` at address 38, and its `counter` formal has the same exact qualified type. The body is a direct `TERMREFdirect` to that method's `counter` `PARAM` at address 33; it contains no `NEW` or constructor `APPLY`. In both sidecars the linked type address resolves to a `TYPEREF` AST node and the name table supplies the qualified `example.Counter` entry. The pinned regression asserts these method bodies, reference kinds, target tags/addresses, constructor signed name and signature, and the corrected classfile/sidecar UUID pairs. This identity control remains separate from construction evidence.

## Canonical source-session evidence

`factory_return_and_identity_control_reach_the_real_jvm_source_pipeline` uses the real lexer, parser, namer, typer, and JVM emitter. It proves both methods from the same `CounterFactory` source unit, and also retains a separate `CounterIdentity` control fixture. The observer checks the source index's method symbol, owner, method signature result, parameter owner, exact typed root shape, and selected constructor `TermRef` target. For `create`, the root is typed `Apply(Select(New(...), <init>), [seed])`; the selected term reference names the canonical `Counter` constructor, the `New` type and result signature name the canonical `Counter` class, and the argument term reference names the canonical `seed` parameter. For `identity`, the typed root is an `Ident` whose term reference names its canonical parameter.

Issue #1691 adds `validate_cross_class_factory_proof` in the source-to-output coordination layer. It retains the borrowed semantic store, source and typed arenas, both source indexes, and complete JVM unit in `ValidatedCrossClassFactoryProof`; its getters expose canonical class/constructor/method/formal symbols, exact source and typed RHS roots, and the JVM descriptor. The factory check requires the exact source `Apply(Select(New(Counter), <init>), [seed])`, every mapped source expression root, the typed canonical constructor/formal targets, the public primary `(I)V` plan, and the JVM method descriptor `(I)Lexample/Counter;`. It checks declared result `TypeRefTarget::Symbol(Counter)` and the typed factory result target independently because the current typer can allocate equivalent reference types at distinct `TypeId`s. The independent identity proof accepts `identity` beside `create` in `CounterFactory` (and retains the standalone `CounterIdentity` fixture), requires a typed `Ident` targeting its own canonical formal, and checks descriptor `(Lexample/Counter;)Lexample/Counter;`. Its method body proof does not require a constructor call.

The original #1691 proof consumed already-produced frontend/JVM state and was read-only; at that stage the paired TASTy producer rejected reference-valued results. #1692 added reference-result serialization, and #1693 adds the bounded constructor body described below. The proof still reports failures at the source-shape, source-identity, semantic-identity, typed-binding, or JVM-plan stage. Focused regressions supply an empty source-to-typed index, a mismatched `SourceId`, a JVM batch with `Counter` or `create` removed, the wrong identity/factory source shape, and a private primary constructor; each is rejected by its corresponding typed error category.

The following IDs are from one test session with `SourceId(1690)` and are diagnostic snapshots only. Arena IDs are session-local; semantic relationships are the contract, not these numbers.

| Item | Snapshot |
| --- | --- |
| `Counter` class | `SymbolId(39)` |
| `Counter` primary constructor | `SymbolId(42)` |
| `CounterFactory` class | `SymbolId(40)` |
| `create` method / `seed` formal | `SymbolId(49)` / `SymbolId(50)` |
| source `create` RHS / exact typed body root | `TreeId(27)` / `TreeId(9)` |
| `New` type / result type | `TypeId(53)` / `TypeId(39)` |
| selected constructor target / argument target | `SymbolId(42)` / `SymbolId(50)` |

An independent identity-only session with `SourceId(1691)` also produced `CounterIdentity=SymbolId(40)`, `identity=SymbolId(49)`, parameter `SymbolId(50)`, source RHS `TreeId(23)`, typed body root `TreeId(5)`, and result `TypeId(54)`. The two result TypeIds differ even though both canonical signatures identify `Counter`.

## Rust TASTy constructor output (#1693)

The Rust paired producer now emits the actual typed `create(seed)` body for the complete `Counter`/`CounterFactory` source batch. Validation accepts only `Apply(Select(New(Counter), Counter.<init>), [seed])`: both source and typed expression roots and spans must correspond, the selected term reference must be the canonical primary constructor owned by the planned public sibling class, its completed signature must be `(Int)Counter`, and the argument must be the original Int formal owned by `create`. The sibling class must be an ordinary public class in the same package with one accessible primary constructor; secondary, defaulted, using, varargs, generic, abstract, trait/object, private, nested and external constructor cases remain unsupported.

The sidecar encodes the method body as `APPLY(SELECTin(NEW(Counter)), [seed])`. The signed constructor name is `<init>` with result `example.Counter` and one `scala.Int` term parameter. The `NEW` type and `SELECTin` owner type refer to the same canonical class type; the argument `TERMREFdirect` targets the method's own `seed` `PARAM`. The factory sidecar contains no cloned `Counter` or constructor declaration. The paired classfile retains descriptor `(I)Lexample/Counter;` and the verifier checks `new/dup/iload_1/invokespecial/areturn` plus both constant-pool owners against the same planned sibling class.

`paired_tasty_planning_emits_the_canonical_factory_new_body` checks the genuine one-session source pipeline, decoded Rust sidecar tree, constructor signature, exact argument formal, JVM descriptor and bytecode. With `DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1`, it also invokes the pinned Scala 3.9 decompiler and `-from-tasty` reader with both Rust-produced sidecars, then checks the rebuilt factory descriptor with `javap`. This is a TASTy interoperability gate; linked Rust artifact runtime is tracked separately by #1695.

The existing identity control remains separate: `identity(counter: Counter): Counter = counter` emits a direct formal reference and no `NEW` or constructor `APPLY`. Both result forms use the same exact planned class type and remain covered by the pinned Scala consumer checks.

## Linked JVM/TASTy source batch (#1694)

The complete paired producer now retains the validated factory identity proof in `PairedJvmTastyBatchPlan` for the canonical `Counter`/`CounterFactory` source pair. Before returning that plan, it checks that the proof's exact source RHS, typed binding, and JVM method body are the same roots used by the Factory TASTy plan. A mismatch fails batch planning with a typed `BatchUnit` error. The ordinary artifact validator then validates the complete ordered output set and each classfile's link to its own TASTy header before returning the batch.

`factory_emits_four_complete_linked_artifacts_from_one_source_session` checks the two `.class` and two `.tasty` outputs, class names and order, Java 21 classfile version, TASTy 28.9 headers, per-class UUID links, exact method descriptors, and factory allocation bytecode. It repeats emission in a fresh source session and compares all four byte outputs and both links for determinism. `factory_batch_contract_rejects_wrong_result_abi_target_and_partial_pairs` proves the artifact gate rejects an incorrect result descriptor, allocation targeting the factory itself, and a missing class/TASTy pair. The existing Counter/Gauge and Counter/Reader artifact tests remain unchanged.

## Rust-produced factory runtime (#1695)

`rust_factory_counter_batch_executes_with_verified_reference_returns_on_jdk21` compiles the exact two-class source above in two fresh frontend sessions, compares each class/TASTy pair byte-for-byte, and runs the first session's original Rust classfiles. The temporary runtime classpath contains only `example/Counter.class`, `example/CounterFactory.class`, and their sidecars; `javac` writes the throwing Java harness to a separate classpath root. The test decodes both classfiles and checks their public class names, major version 65, exact declared methods and fields, public constructors, `(I)Lexample/Counter;` factory descriptor, public `(I)V` Counter constructor, and absence of Factory backing fields or `get` methods.

The decoded `Factory.create` Code has `max_stack=3` and `max_locals=2` (`this` in local 0, `seed` in local 1). Its instruction flow is `new Counter; dup; iload_1; invokespecial Counter.<init>:(I)V; areturn`; both allocation and constructor references resolve to the exact `example/Counter` class. `javap -v -p -c` reports the same descriptor and Methodref. JDK execution uses Temurin `21.0.8+9` and `java -Xverify:all`. The harness checks values 7 and 2 from two Factory instances, mutates the first Counter to 13 and then -42 while the second remains 2, and confirms a further `create(0)` returns a fresh initialized instance. Each original classfile's SHA-256 and exact bytes are compared before and after harness compilation/execution, so javac/java cannot silently replace the Rust artifacts.

`factory_batch_contract_rejects_missing_dup_wrong_seed_slot_and_constructor_ref` mutates the emitted class bytes to remove `dup`, load the wrong local, or target the Factory's own Object constructor; each malformed batch is rejected by the artifact contract. Existing source regressions reject wrong constructor arity/type and a private sibling constructor before returning a batch. These checks establish runtime behavior for Rust-produced JVM classes only; Scala's use of the Rust TASTy remains the separate #1696 gate.

## Scala consumer and classpath controls

[`FactoryClient.scala`](../tests/fixtures/jvm_tasty_cross_class_factories/FactoryClient.scala) and [`IdentityClient.scala`](../tests/fixtures/jvm_tasty_cross_class_factories/IdentityClient.scala) compile with Scala 3.9 against the oracle classfiles and run successfully on Java 21. Additional classpath probes observed:

| Classpath contents | Scala compile or runtime result |
| --- | --- |
| `.class` files without matching `.tasty` files | Scala 3.9 rejects the client with “This `.tasty` file is missing.” |
| `.tasty` sidecars without `.class` files | Scala 3.9 typechecks the client; direct JVM execution fails with `NoClassDefFoundError: example/CounterFactory`. |
| The same sidecars passed to `-from-tasty` | Scala 3.9 rebuilds `Counter` and `CounterFactory` classfiles; a client compiled against those classes passes `java -Xverify:all` and the state checks. |

The source classfile and sidecar are separate runtime/compile-time artifacts. A TASTy-only classpath can provide Scala compile-time information, while the JVM still needs generated or otherwise supplied `.class` files. A classfile with Scala metadata but no sidecar is not accepted by this pinned Scala 3.9 classpath probe.

## Pinned Scala 3.9 factory consumer (#1696)

`DOTTY_REQUIRE_SCALA_TASTY_FACTORY_CONSUMER` makes `jvm_tasty_factory_scala_consumer` mandatory when present; an unset flag keeps the integration test opt-in. The dedicated `JVM TASTy factory Scala consumer` workflow installs Temurin JDK `21.0.8+9`, checks out Scala revision `777528f19a58e794c9954a42f433373472ec57f8`, verifies all eight compiler/runtime jar hashes against [`provenance.json`](../tests/fixtures/tasty_pickler/provenance.json), and runs this gate together with the existing strict Counter/Reader Scala consumer regression.

The independent [`FactoryConsumer.scala`](../tests/fixtures/jvm_tasty_cross_class_factories/FactoryConsumer.scala) is the only Scala source compiled by the positive gate. For each of two fresh Rust frontend sessions, Scala 3.9.0 compiles it to an empty output directory with compile classpath order `<Rust batch root>`, `scala3-library_3-3.9.0.jar`, and `scala-library-3.9.0.jar`. The compiler process itself uses the eight pinned compiler/runtime jars (Scala 3 library, Scala library, compiler, TASTy core, interfaces, scala-asm, compiler-interface, util-interface). The runtime order is `<Rust batch root>`, `<client output>`, `scala3-library_3-3.9.0.jar`, and `scala-library-3.9.0.jar`; compiler-only jars are excluded. The client uses named `seed` and `value` arguments, observes the `Counter` result through `get` and `set`, checks state independence, and prints `42`. Its output is exactly `consumer/Consumer$.class` and `consumer/Consumer.class`; all eight compiler/runtime dependency jars are checked to contain neither `example/Counter.class` nor `example/CounterFactory.class`. `javap -verbose consumer.Consumer$` must include `CounterFactory.<init>:()V`, `CounterFactory.create:(I)Lexample/Counter;`, `Counter.get:()I`, and `Counter.set:(I)V`, and may not widen the factory result to `Object`.

The test independently decodes `CounterFactory.create` and checks `new Counter; dup; iload_1; invokespecial Counter.<init>:(I)V; areturn`, verifies the allocation and constructor constant-pool owners, classfile major 65, TASTy 28.9.0, and per-class TASTY links. The fresh Rust batches match byte-for-byte across all four files and have equal producer-owned links. Each Scala compile and runtime uses the original classfiles from its batch; their SHA-256 and bytes are unchanged afterward.

The method-discovery controls are isolated. Removing `CounterFactory.create` from its classfile while retaining its matching sidecar still lets the probe compile through `CounterFactory.tasty`; removing `Counter.get` while retaining `Counter.tasty` still lets its probe compile. In both matched classfile-only roots (the altered classfile without its sidecar), the method lookup fails with Scala E008 (`create`/`get` is not a member). A separate missing-classfile control for each class still compiles against its sidecar but fails at execution with `NoClassDefFoundError: example/Counter` or `NoClassDefFoundError: example/CounterFactory`. These incomplete roots are never used by the successful runtime. Independent Counter and CounterFactory UUID mismatches fail with `out of sync with its TASTy file` under `-Werror`. Missing and misplaced sidecars fail at the Scala loader with `This .tasty file is missing`; corrupt sidecars fail while loading with `could not be read ... not a TASTy file`. Scala can emit a further source-level error after the first diagnostic, so the tests match the specific first-stage message. These controls distinguish TASTy lookup from runtime class availability.

The successful local run used Rust `1.98.1`, Scala `3.9.0`, revision `777528f19a58e794c9954a42f433373472ec57f8`, and Temurin `21.0.8+9`. The two emitted batches shared these hashes and UUIDs:

| Class | Class SHA-256 | TASTy SHA-256 | TASTY UUID |
| --- | --- | --- | --- |
| `Counter` | `827ae5af5932752105e82918df61cbac2d9fc9eeae4e451b71bc3f5f4641b7c6` | `711b949785f948786bb2a796f646cdaf53121476f0e5ee11db4f556505e2635f` | `00a3d0f572713eac0000000000008193` |
| `CounterFactory` | `6708e19958c8c8f085a4916ff298c7d1cc4c8e47f406bfb5aa0991a78d7cae38` | `f7ca66c9cd8f3df9467f5dc19686f3958025e6bb5de7b0ba971358198f5dfb32` | `00557cce1fd3e1c10000000000008194` |

Run the strict Scala consumer and the existing strict Scala regression with the pinned local inputs:

```sh
DOTTY_REQUIRE_SCALA_TASTY_FACTORY_CONSUMER=1 \
SCALA39_ROOT=/home/vdrake6/dotty-rs/.worktrees/scala39-source \
JAVA_HOME=/home/vdrake6/.jdks/jdk-21 \
COURSIER_MAVEN_ROOT=/home/vdrake6/dotty-rs/target-fs2/coursier/https/repo1.maven.org/maven2 \
cargo +1.98.1 test --test jvm_tasty_factory_scala_consumer \
  --locked -- --nocapture

DOTTY_REQUIRE_SCALA_TASTY_CROSS_CLASS_CONSUMER=1 \
SCALA39_ROOT=/home/vdrake6/dotty-rs/.worktrees/scala39-source \
JAVA_HOME=/home/vdrake6/.jdks/jdk-21 \
COURSIER_MAVEN_ROOT=/home/vdrake6/dotty-rs/target-fs2/coursier/https/repo1.maven.org/maven2 \
cargo +1.98.1 test --test jvm_tasty_cross_class_scala_consumer \
  --locked -- --nocapture
```

The oracle report includes the exact compiler and Java command arguments, compile/runtime classpath order, all eight jar hashes, source hash, Rust artifact hashes, UUIDs, runtime output, and the diagnostics observed for each negative control. Missing Scala sources, JDK tools, pinned jars, revision or matching artifact hashes fail the test instead of turning the strict gate into a skip.

## Negative and unsupported cases

This matrix combines observed failure-stage probes in the focused tests with the existing object creation contract in [`jvm-object-creation-contract.md`](jvm-object-creation-contract.md). It records the first boundary known from those contracts; it does not claim that every unsupported form reaches the factory method-result check.

| Case | First observed boundary |
| --- | --- |
| Wrong constructor arity, e.g. `new C()` for one required parameter | Typer: `ApplicationArityMismatch` |
| Wrong constructor argument type, e.g. `new C(true)` for `Int` | Typer: `ApplicationArgumentTypeMismatch` |
| Equal applicable overloads | Typer: `AmbiguousConstructorApplication`; no candidate is guessed |
| Private primary constructor called from a different class | Typer resolves it; JVM codegen then rejects it with `InaccessibleJvmConstructor` before publishing either class. `private_factory_constructor_is_rejected_by_jvm_call_site_visibility` exercises the exact `CounterFactory.create` shape. |
| Secondary, defaulted-parameter, or `using` constructor | Source frontend reaches JVM planning; codegen reports `UnsupportedConstructor` or `InvalidPrimaryConstructor`. `unsupported_real_field_and_constructor_sources_fail_without_artifacts` covers each form. |
| Generic or abstract class, or trait/object class plan | JVM class validation reports `UnsupportedJvmClass` with the class kind; it does not invent a concrete constructor. See `dotty-jvm`'s `rejects_unsupported_class_shapes_and_generic_declarations` and `rejects_traits_objects_and_module_classes_as_emittable_classes`. |
| Unplanned constructor target or stale/foreign constructor symbol | JVM validation reports `MissingClassPlan` or rejects the source-proven constructor identity (`InvalidPrimaryConstructor` / constructor-owner validation). See `reference_projection_rejects_unplanned_stale_and_noncanonical_targets` and `rejects_a_constructor_reference_owned_by_another_class`. |
| External class without a planned class artifact | JVM projection rejects the external/unplanned target; it does not guess a class plan from a descriptor. Missing source class symbols instead fail at typer lookup. |
| Wrong-owner TypeRef, malformed class info, or invalid package chain | JVM metadata/projection rejects the class/type identity before emission (`UnsupportedJvmReferenceType`, `InvalidJvmClassInfo`, or `InvalidPackageOwnerChain`/`InvalidJvmName`). See `method_signature_projection_requires_exact_class_and_package_identity`, `rejects_malformed_class_info_instead_of_guessing_class_shape`, and `rejects_nested_and_invalid_package_owner_chains`. |
| Identity returning its exact planned class-valued formal | Source and JVM/TASTy validation accept the reference result; `new` is not involved. The opt-in Scala consumer check is described above. |
| Factory result from `new Counter(seed)` | The exact public planned primary `(Int)Counter` constructor and direct `seed` argument are serialized and validated against the linked JVM descriptor and instructions. |
| Private/unplanned/stale constructor target or a wrong sibling class | Source/typed expression validation rejects the owner, canonical source declaration, or planned-class proof before TASTy bytes are returned. |
| Wrong result type, constructor argument expression, or nested `new` | TASTy expression validation rejects the shape before complete sidecars are published. |
| One nested `new` as a constructor argument, or sibling allocations | JVM source lowering accepts the bounded shapes and JDK verification passes; see `nested_new_constructor_arguments_lower_lifo_and_verify_initialized_values`. Deeper nesting or a third simultaneously pending allocation is rejected by source lowering / the class-aware verifier before class publication. |
| Cross-class reference field initialized by an approved direct `new` | JVM-only source path supports a planned private reference field with a bounded initializer. Factory-valued/nested/branch-valued reference field initialization is rejected by JVM codegen. The identity result proof does not claim support for serializing factory-valued field initializers. |
| Classfile-only Scala classpath or stale/missing TASTy sidecar | Pinned Scala 3.9 loader rejects the Scala class with the missing-sidecar diagnostic. |
| TASTy-only Scala classpath | Scala compiler can typecheck; execution cannot load a missing JVM class until `-from-tasty` or another compiler provides it. |

The existing Scala consumer audit also measures missing, misplaced, corrupt, and stale-UUID sidecars: [`jvm-tasty-cross-class.md`](jvm-tasty-cross-class.md) records each observed diagnostic and first stage. #1692 added the reference-result Scala consumer test; #1693 adds the separate constructor decompile/reader gate; #1695 verifies the original Rust classfiles on Java 21. The strict #1696 consumer uses the Rust-produced linked TASTy and original Rust classes.

## Follow-up dependency map

Issues #1690–#1696 provide the pinned Scala source oracle, canonical source/typed identity proof, reference-result TASTy, bounded constructor application, complete linked batch publication, strict Java 21 execution, and the independent Scala consumer. Issue #1697 closes the release gate by running these layers together and preserving prior consumer gates.

1. **#1697** — strict linked factory release gate.

### Strict linked factory release gate (#1697)

The `JVM TASTy factory Scala consumer` workflow is the aggregate gate. It checks out Scala revision `777528f19a58e794c9954a42f433373472ec57f8`, stages that source compiler for the factory constructor `-decompile`/`-from-tasty` oracle, verifies the eight Scala 3.9.0 Maven artifact hashes in [`provenance.json`](../tests/fixtures/tasty_pickler/provenance.json), and uses Temurin JDK `21.0.8+9` with Rust `1.98.1`. It makes the one-class Counter, independent Counter/Gauge, Reader/Counter, and CounterFactory/Counter Scala consumers mandatory. The same strict step runs the paired factory constructor oracle and the semantic pickler `-from-tasty` oracle. A second step sets `DOTTY_CLASSFILE_REQUIRE_JDK=1` and runs the classfile, JVM source, linked artifact publisher, and stateful class JDK gate. Workspace formatting, lint, rustdoc, tests, and frontend dependency checks remain enforced by the general CI workflow.

The mandatory producer input is one Scala source session, [`CounterFactoryRuntime.scala`](../tests/fixtures/jvm_tasty_cross_class_factories/CounterFactoryRuntime.scala); it declares mutable `Counter` state and `CounterFactory.create(seed)` returning `new Counter(seed)`. The independent Scala client is [`FactoryConsumer.scala`](../tests/fixtures/jvm_tasty_cross_class_factories/FactoryConsumer.scala). Their SHA-256 provenance is respectively `d8f39e95d3a8503409eb39e692aa3c0eda3123428ec5a1d7e6cf56b9aa400a00` and `eac836d574ab779256dae5f87161366976d1374e97ee1b0b9596ca71ad913bff`. The Rust test creates two fresh semantic sessions, validates four complete linked artifacts and their ordered paths, and compares all bytes and UUIDs. The publisher rejects duplicate and ASCII case-insensitive colliding paths before staging; malformed sidecars, incorrect links, unsafe paths, preexisting output, and injected staged failures have focused controls. Cleanup assertions cover handled errors only; the publisher does not promise crash-atomic multi-file I/O.

The strict Scala client compiles against only each Rust batch plus Scala runtime libraries. It resolves `Factory.create` and `Counter.get` from their matching Rust TASTy sidecars; classfile-only controls reject those members, while UUID mismatch and missing/corrupt/misplaced sidecar controls fail at the Scala loader. The compiled client runs against the original Rust classfiles with `java -Xverify:all`, reads initial values 7 and 2, mutates the first Counter to 42, leaves the second at 2, and reports 42. The test compares SHA-256 and bytes of both original Rust classfiles before and after Scala/JVM execution. The separate #1695 Java-only harness checks 7, 13, and -42.

Pinned consumer evidence recorded by #1696 for the Rust batch is:

| Class | Class SHA-256 | TASTy SHA-256 | TASTY UUID |
| --- | --- | --- | --- |
| `Counter` | `827ae5af5932752105e82918df61cbac2d9fc9eeae4e451b71bc3f5f4641b7c6` | `711b949785f948786bb2a796f646cdaf53121476f0e5ee11db4f556505e2635f` | `00a3d0f572713eac0000000000008193` |
| `CounterFactory` | `6708e19958c8c8f085a4916ff298c7d1cc4c8e47f406bfb5aa0991a78d7cae38` | `f7ca66c9cd8f3df9467f5dc19686f3958025e6bb5de7b0ba971358198f5dfb32` | `00557cce1fd3e1c10000000000008194` |

This is a bounded compatibility slice: no typer changes, secondary/overloaded or generic constructors, nested/reference-valued constructor arguments, external members, cross-class inheritance, general Scala ABI, public compilation API/CLI, or full hosted Scala corpus claim. Existing unsupported shapes fail through the documented source/TASTy/JVM validation layer rather than being synthesized.

The local aggregate consumer/oracle command uses Scala source revision `777528f19a58e794c9954a42f433373472ec57f8`, Scala compiler artifacts pinned by `provenance.json`, Java `21.0.8`, and Rust `1.98.1`:

```sh
DOTTY_REQUIRE_SCALA_TASTY_CONSUMER=1 \
DOTTY_REQUIRE_SCALA_TASTY_MULTICLASS_CONSUMER=1 \
DOTTY_REQUIRE_SCALA_TASTY_CROSS_CLASS_CONSUMER=1 \
DOTTY_REQUIRE_SCALA_TASTY_FACTORY_CONSUMER=1 \
DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1 \
SCALA39_ROOT=/home/vdrake6/dotty-rs/.worktrees/scala39-source \
JAVA_HOME=/home/vdrake6/.jdks/jdk-21 \
COURSIER_MAVEN_ROOT=/home/vdrake6/dotty-rs/target-fs2/coursier/https/repo1.maven.org/maven2 \
TMPDIR="$PWD/target/tmp" \
cargo +1.98.1 test -p dotty \
  --test jvm_tasty_scala_consumer \
  --test jvm_tasty_multiclass_scala_consumer \
  --test jvm_tasty_cross_class_scala_consumer \
  --test jvm_tasty_factory_scala_consumer \
  --test jvm_tasty_cross_class_factories \
  --test tasty_source_pickling --locked -- --nocapture

DOTTY_CLASSFILE_REQUIRE_JDK=1 \
JAVA_HOME=/home/vdrake6/.jdks/jdk-21 \
PATH="/home/vdrake6/.jdks/jdk-21/bin:$PATH" \
TMPDIR="$PWD/target/tmp" \
cargo +stable test \
  -p dotty-classfile --test classfile_jvm_encoder \
  -p dotty-jvm --lib \
  -p dotty --test jvm_source_compilation \
  --test jvm_tasty_artifacts \
  --test jvm_stateful_class_contract --locked -- --nocapture
```

## Rust regression commands

```sh
cargo test --test jvm_tasty_cross_class_factories --locked
cargo test --test jvm_tasty_plan --test jvm_tasty_contract --test jvm_tasty_artifacts --locked
DOTTY_CLASSFILE_REQUIRE_JDK=1 \
JAVA_HOME=/home/vdrake6/.jdks/jdk-21 \
PATH="/home/vdrake6/.jdks/jdk-21/bin:$PATH" \
cargo test -p dotty --test jvm_tasty_artifacts \
  rust_factory_counter_batch_executes_with_verified_reference_returns_on_jdk21 \
  --locked -- --nocapture
SCALA39_ROOT=/home/vdrake6/dotty-rs/.worktrees/scala39-source \
DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1 \
JAVA_HOME=/home/vdrake6/.jdks/jdk-21 \
PATH="/home/vdrake6/.jdks/jdk-21/bin:$PATH" \
cargo test --test jvm_tasty_cross_class_factories \
  paired_tasty_planning_emits_the_canonical_factory_new_body \
  --locked -- --nocapture
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test --test jvm_source_compilation \
  factory_return_and_identity_control_reach_the_real_jvm_source_pipeline \
  --locked -- --nocapture
```
