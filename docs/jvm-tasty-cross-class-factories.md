# Scala 3.9 cross-class factory TASTy support

This document records the Scala/JVM audit requested by [issue #1690](https://github.com/scytrowski/dotty-rs/issues/1690) and the bounded TASTy producer slice added by [issue #1693](https://github.com/scytrowski/dotty-rs/issues/1693). The source typer and JVM bytecode writer remain unchanged.

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

## Scala consumer and classpath controls

[`FactoryClient.scala`](../tests/fixtures/jvm_tasty_cross_class_factories/FactoryClient.scala) and [`IdentityClient.scala`](../tests/fixtures/jvm_tasty_cross_class_factories/IdentityClient.scala) compile with Scala 3.9 against the oracle classfiles and run successfully on Java 21. Additional classpath probes observed:

| Classpath contents | Scala compile or runtime result |
| --- | --- |
| `.class` files without matching `.tasty` files | Scala 3.9 rejects the client with “This `.tasty` file is missing.” |
| `.tasty` sidecars without `.class` files | Scala 3.9 typechecks the client; direct JVM execution fails with `NoClassDefFoundError: example/CounterFactory`. |
| The same sidecars passed to `-from-tasty` | Scala 3.9 rebuilds `Counter` and `CounterFactory` classfiles; a client compiled against those classes passes `java -Xverify:all` and the state checks. |

The source classfile and sidecar are separate runtime/compile-time artifacts. A TASTy-only classpath can provide Scala compile-time information, while the JVM still needs generated or otherwise supplied `.class` files. A classfile with Scala metadata but no sidecar is not accepted by this pinned Scala 3.9 classpath probe.

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

The existing Scala consumer audit also measures missing, misplaced, corrupt, and stale-UUID sidecars: [`jvm-tasty-cross-class.md`](jvm-tasty-cross-class.md) records each observed diagnostic and first stage. #1692 added the reference-result Scala consumer test; #1693 adds the separate constructor decompile/reader gate. Full linked Rust factory runtime and the broader release gate remain later increments.

## Follow-up dependency map

Issues #1690–#1693 provide the pinned source/runtime oracle, canonical source/typed identity proof, reference-result TASTy and the bounded constructor application. Remaining implementation proceeds in this dependency order:

1. **#1694** — full linked artifact publication and fail-closed batch consistency.
2. **#1695** — strict JDK runtime factory client for Rust-produced artifacts.
3. **#1696** — wider independent Scala consumer and sidecar/classpath behavior.
4. **#1697** — final release gate after the preceding compatibility layers pass.

These later increments are not completed by the focused #1693 consumer test; it checks Scala decompilation/rebuild and ABI, not linked Rust factory execution.

## Rust regression commands

```sh
cargo test --test jvm_tasty_cross_class_factories --locked
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
