# Scala 3.9 cross-class factory audit

This is the audit requested by [issue #1690](https://github.com/scytrowski/dotty-rs/issues/1690). It records existing behavior; it does not add factory support to the TASTy pickler or change typer/JVM production code.

## Pinned source and oracle

The Scala source checkout was at `777528f19a58e794c9954a42f433373472ec57f8` (`3.9.0-bin-SNAPSHOT-git-777528f`), and compilation/runtime checks used Temurin JDK `21.0.8+9`. Scala was invoked with `-release 21`, producing classfile major version 65. The primary fixture is [`CounterFactory.scala`](../tests/fixtures/jvm_tasty_cross_class_factories/CounterFactory.scala); the independent identity control is [`CounterIdentity.scala`](../tests/fixtures/jvm_tasty_cross_class_factories/CounterIdentity.scala). Both preserve the issue's package, mutable `current`, `get`, `set`, factory argument `seed`, and exact `create` expression. The identity fixture is a separate class so its result cannot mask the factory result.

In this environment, the checked-out `dist/.../bin/scalac` launchers were incomplete (`sbt` was not installed and the staged Scala launcher lacked its compiler runtime). The already-built pinned compiler and libraries worked directly. This reproduces the oracle from the Scala checkout's staged Maven artifacts:

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
| `Counter` | `00b187029fc0c9e800629c56fbd58f1b` | `eb06dbe8772e356000a600654dfae0679048aacb33e6899c3ce1d00da15a5ef7` | `86cc8fcb7f3b337035e28e36e3f72a1b8c2697ee3c7341873bbdd4100f082ec7` |
| `CounterFactory` | `00acfba1d5731fed00b9210da970cb1e` | `26c1cfd5925e439a0343932e060982a3c2e586e9a2b61314db05600e5ce213dc` | `520623e9ed48eb7ee981f6a47a623d0e6a56cc677e3edaf14327fcbe47221264` |
| `CounterIdentity` | `0051b70ada9c40eb00eb1d896f7ae618` | `089030ee03931cccdb904db1020ca62bf840d996b4b86d8e372e4ce573902f6b` | `d08a460dab5bb44907b2784aedd632f90a4be91ba1d54b08b340c953556c63e0` |

The factory sidecar validates as 28.9.0 and contains the `example` package root, `CounterFactory`, `create`, `seed`, `Counter`, and `<init>` names. Its AST contains `APPLY`, `SELECTin` (tag 176), and `NEW` nodes. The `Counter` reference is a qualified `example.Counter` name-table entry; the generated classfile independently links it through the exact constructor `Methodref`. The identity sidecar/classfile is audited separately and has the reference identity descriptor but its method code is only `aload_1; areturn`.

## Canonical source-session evidence

`factory_return_and_identity_control_reach_the_real_jvm_source_pipeline` uses the real lexer, parser, namer, typer, and JVM emitter, once per source unit. The observer checks the source index's method symbol, owner, method signature result, parameter owner, exact typed root shape, and selected constructor `TermRef` target. For `create`, the root is typed `Apply(Select(New(...), <init>), [seed])`; the selected term reference names the canonical `Counter` constructor, the `New` type and result signature name the canonical `Counter` class, and the argument term reference names the canonical `seed` parameter. For `identity`, the typed root is an `Ident` whose term reference names its canonical parameter.

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

## First rejection boundary

The existing JVM-only source path accepts and emits both methods. The focused test checks the emitted descriptors and instruction sequences for `create` and `identity`; with `DOTTY_CLASSFILE_REQUIRE_JDK=1`, it also compiles a Java 21 client and executes the factory result under `java -Xverify:all`.

The paired JVM/TASTy batch planner runs the same source frontend, completes both typed bodies, and validates the complete JVM unit before it reaches the cross-contract check. The first rejection is then:

```text
phase: "JVM/TASTy cross-contract validation"
reason: TastyPlan(MalformedType { ty: TypeId(...), role: MethodResult })
```

The regression runs `create` and `identity` as separate batches. Both fail on the reference-valued method result. The class-state pickler's method-result subset currently permits only `Int`, `Boolean`, and `Unit`; the wrapper reports the error while validating the TASTy/JVM cross-contract. No typed trees, constructor symbols, or type references are fabricated to get past it. This makes identity an independent control for the ABI/result boundary, not evidence that factory construction works in paired output.

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
| Reference-valued factory or identity result | Source typing and JVM unit validation succeed; paired TASTy cross-contract reports `TastyPlan(MalformedType { role: MethodResult })`. |
| One nested `new` as a constructor argument, or sibling allocations | JVM source lowering accepts the bounded shapes and JDK verification passes; see `nested_new_constructor_arguments_lower_lifo_and_verify_initialized_values`. Deeper nesting or a third simultaneously pending allocation is rejected by source lowering / the class-aware verifier before class publication. |
| Cross-class reference field initialized by an approved direct `new` | JVM-only source path supports a planned private reference field with a bounded initializer. Factory-valued/nested/branch-valued reference field initialization is rejected by JVM codegen. The factory's paired result boundary fails earlier at `MethodResult`, so it is not evidence for field serialization. |
| Classfile-only Scala classpath or stale/missing TASTy sidecar | Pinned Scala 3.9 loader rejects the Scala class with the missing-sidecar diagnostic. |
| TASTy-only Scala classpath | Scala compiler can typecheck; execution cannot load a missing JVM class until `-from-tasty` or another compiler provides it. |

The existing Scala consumer audit also measures missing, misplaced, corrupt, and stale-UUID sidecars: [`jvm-tasty-cross-class.md`](jvm-tasty-cross-class.md) records each observed diagnostic and first stage. The factory's Rust paired producer stops at the reference-result contract for `create` and `identity`; it does not claim that the later constructor, field, owner, or sidecar forms are accepted by paired output.

## Follow-up dependency map

Issue #1690 provides a pinned source/runtime oracle and the first-boundary evidence needed by its successors. The remaining implementation work should proceed in this dependency order:

1. **#1691** — canonical typed construction proof and source-to-typed identity.
2. **#1692** — reference-valued result TASTy encoding.
3. **#1693** — constructor `New`/`SELECTin`/`<init>` TASTy encoding and exact owner/formal bindings.
4. **#1694** — batch cross-class TASTy validation and linked artifacts.
5. **#1695** — JDK verification and runtime factory client.
6. **#1696** — independent Scala consumer and sidecar/classpath behavior.
7. **#1697** — final release gate after the preceding compatibility layers pass.

The audit does not mark those successor requirements complete. The Scala oracle validates their target behavior; the current Rust paired producer still rejects the reference result before serializing the factory method body.

## Rust regression commands

```sh
cargo test --test jvm_tasty_cross_class_factories --locked
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test --test jvm_source_compilation \
  factory_return_and_identity_control_reach_the_real_jvm_source_pipeline \
  --locked -- --nocapture
```
