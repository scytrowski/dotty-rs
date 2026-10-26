# Scala 3.9 reference-field object composition audit

This is the characterization-only result for [issue #1715](https://github.com/scytrowski/dotty-rs/issues/1715). It records the Scala 3.9 source/TASTy and JVM ABI for the exact private immutable reference field initialized by `new`. It does not enable Rust TASTy producer support.

## Pinned source and reproduction

The Scala source checkout was exactly `777528f19a58e794c9954a42f433373472ec57f8` (`3.9.0-bin-SNAPSHOT-git-777528f`). The checked-in oracle artifacts were independently reproduced from an empty output directory with Eclipse Temurin `21.0.8+9` and `-release 21`. The source compiler JAR used for this run has SHA-256 `b56626390028bd06347e3549cf271a03ca7d2e7be491ceff630ccee94e439f4f`; the four staged runtime/format JAR hashes are recorded below. The workspace-pinned release artifact hashes remain in [`provenance.json`](../tests/fixtures/tasty_pickler/provenance.json).

```sh
JAVA_HOME=/home/vdrake6/.jdks/jdk-21
SCALA_ROOT=/home/vdrake6/dotty-rs/.worktrees/scala39-source
export JAVA_HOME PATH="$JAVA_HOME/bin:$PATH"
SCALA_CP="$(find "$SCALA_ROOT/dist/target/universal/stage/maven2" -type f -name '*.jar' -print | paste -sd:):$SCALA_ROOT/library/target/scala-library-bootstrapped/scala-library-3.9.0-bin-SNAPSHOT.jar:$SCALA_ROOT/library/target/scala3-library-bootstrapped/scala-3.9.0-bin-SNAPSHOT-nonbootstrapped/scala3-library_3-3.9.0-bin-SNAPSHOT.jar:$SCALA_ROOT/tasty/target/tasty-core-bootstrapped/scala-3.9.0-bin-SNAPSHOT-nonbootstrapped/tasty-core_3-3.9.0-bin-SNAPSHOT.jar"
OUT="$PWD/target/issue-1715-oracle"
mkdir -p "$OUT/classes" "$OUT/client"
java -cp "$SCALA_CP" dotty.tools.MainGenericCompiler \
  -classpath "$SCALA_CP" -release 21 -d "$OUT/classes" \
  tests/fixtures/jvm_tasty_object_composition/CounterOwner.scala
java -cp "$SCALA_CP" dotty.tools.MainGenericCompiler \
  -classpath "$SCALA_CP:$OUT/classes" -release 21 -d "$OUT/client" \
  tests/fixtures/jvm_tasty_object_composition/CounterOwnerClient.scala
javap -classpath "$OUT/classes" -v -p -c example.CounterOwner
java -Xverify:all -ea \
  -classpath "$SCALA_ROOT/library/target/scala-library-bootstrapped/scala-library-3.9.0-bin-SNAPSHOT.jar:$SCALA_ROOT/library/target/scala3-library-bootstrapped/scala-3.9.0-bin-SNAPSHOT-nonbootstrapped/scala3-library_3-3.9.0-bin-SNAPSHOT.jar:$OUT/classes:$OUT/client" \
  client.CounterOwnerClient
```

The client constructs owners with values 7 and 42 and checks each `read()`. It exits successfully on Java 21 with full bytecode verification. The source is unchanged from the issue's mandatory fixture in [`CounterOwner.scala`](../tests/fixtures/jvm_tasty_object_composition/CounterOwner.scala).

| Oracle artifact | Bytes | SHA-256 | Class TASTY UUID |
| --- | ---: | --- | --- |
| `example/Counter.class` | 648 | `f89f0146ddcd4da8677c56eed0b65157b60a5e7bc5b246c2ac5256c4e31b3fc9` | `00fb1c70-e109-8de8-0062-9c42f7fe8f1b` |
| `example/Counter.tasty` | 583 | `cadecee49e2b88e47b113e69757972ab0b10f9aa76a87dfe2466285b2774c034` | same as `Counter.class` |
| `example/CounterOwner.class` | 636 | `496e195284ecd7a743955658831c926e1ae2936f3e406bcc9f4d3b59b6051a9e` | `007aa70c-1036-b5ec-0085-a60fb56b0264` |
| `example/CounterOwner.tasty` | 584 | `3515d3eb53e2644c1d019744198e5dee84f7720d5932fb12a216aabe1ee630b2` | same as `CounterOwner.class` |

Both sidecars parse as TASTy `28.9.0`; both linked classfiles are major version 65. The fixtures are under [`scala39/example`](../tests/fixtures/jvm_tasty_object_composition/scala39/example). The compiler runtime JAR hashes for this source-checkout run were:

| JAR | SHA-256 |
| --- | --- |
| `scala3-compiler_3-3.9.0-bin-SNAPSHOT.jar` | `b56626390028bd06347e3549cf271a03ca7d2e7be491ceff630ccee94e439f4f` |
| `scala3-library_3-3.9.0-bin-SNAPSHOT.jar` | `8c605e1945ae5a3c57d618d02241f7190ecbed744367473c7c3a9b61940c920c` |
| `scala-library-3.9.0-bin-SNAPSHOT.jar` | `bc87575e0099514b17f7e84384b4a9a51d7a376720af8974e04a41a202909434` |
| `tasty-core_3-3.9.0-bin-SNAPSHOT.jar` | `01e1090c1e40303e370e8c12809e238d58da23c859892ac19f17d59ef3de040e` |

## TASTy and source identity evidence

The Scala 3.9 `-print-tasty` output and the lossless Rust decoder agree on the `CounterOwner` sidecar. Addresses below are the pinned TASTy AST addresses from that sidecar:

| Address | Declaration or tree | Evidence |
| ---: | --- | --- |
| 4 | `TYPEDEF CounterOwner` | The sidecar owns this class and its `inner` field/read method. |
| 30 | primary `<init>` `DEFDEF` | Its formal at local address 1 is `PARAM seed: Int`. |
| 44 | `VALDEF inner` | Tail has `PRIVATE`, `LOCAL`, no `MUTABLE`; type tree is `IDENTtpt Counter → TYPEREF Counter → TERMREFpkg example`. |
| 53 | `inner` initializer `APPLY` | Its function is `SELECTin` address 55 for signed `<init>(scala.Int): example.Counter`; the qualifier is `NEW` address 58. |
| 59–63 | `NEW Counter` / selected constructor owner type | `IDENTtpt Counter` shares the field's TASTy type address 49. The constructor signature names the exact public `(I)V` target. |
| 65 | constructor argument term tree | The raw tree is `TERMREFsymbol` with its nested `THIS`, `TYPEREFsymbol` and `SHAREDtype` structure. The same-session Rust frontend separately proves it is the exact constructor-owned `seed` symbol; its classfile projection is `iload_1`. |
| 74 | `DEFDEF read(): Int` | The body at 82 applies signed `get(): scala.Int` at 84; its qualifier selects `inner` at 87 through `QUALTHIS`. The selected `get` declaration is in `Counter.tasty`, not duplicated into the owner sidecar. |

This distinction matters for source identity: the raw `TERMREFsymbol` argument is not a plain textual `seed` leaf. The exact source/typed relationship is checked through the real Rust frontend, and Scala's own TASTy reader successfully compiles the independent client against the sidecars.

The Rust regression [`jvm_tasty_object_composition.rs`](../tests/jvm_tasty_object_composition.rs) checks the header/version and UUID link, field modifiers and type spelling, constructor `SELECTin` signed name and descriptor, raw argument/reference form, `read` qualifier/member selection and `Counter.get` sidecar declaration. It also runs one real Lexer → Parser → Namer → Typer source session and asserts that:

- `Counter`, `Counter.<init>`, `Counter.get`, `CounterOwner`, `CounterOwner.<init>`, `seed`, `inner`, and `read` retain distinct canonical `SymbolId`s and their correct owners;
- `inner` is immutable, private, owned by `CounterOwner`, and has `TypeRef` to the canonical `Counter` class;
- the typed initializer is `Apply(Select(New(Counter), Counter.<init>), seed)`, with the selected constructor target and argument pointing to the canonical symbols;
- `read` is `Apply(Select(inner, get))`, with the receiver targeting canonical `Counter.inner` and the method targeting canonical `Counter.get`;
- source RHS roots map exactly to typed roots through `SourceTypedIndex`.

With `SourceId(1715)`, the test session produced these diagnostic snapshots: `Counter=SymbolId(39)`, `Counter.<init>=SymbolId(42)`, `Counter.get=SymbolId(45)`, `CounterOwner=SymbolId(40)`, `CounterOwner.<init>=SymbolId(49)`, `seed=SymbolId(50)`, `inner=SymbolId(51)`, `inner` declared type `TypeId(52)`, source/typed field initializer roots `TreeId(27)`/`TreeId(9)`, and `read=SymbolId(52)` with source/typed RHS roots `TreeId(32)`/`TreeId(12)`. These numbers are session-local; the asserted relationships are the contract.

## JVM ABI and execution

`javap -v -p -c example.CounterOwner` reports exactly one field and two methods. It does not synthesize a Scala getter for the private `val`:

| Member | Descriptor | Flags |
| --- | --- | --- |
| `inner` | `Lexample/Counter;` | `ACC_PRIVATE, ACC_FINAL` |
| `<init>` | `(I)V` | `ACC_PUBLIC` |
| `read` | `()I` | `ACC_PUBLIC` |

The primary constructor's code is:

```text
aload_0
invokespecial java/lang/Object.<init>:()V
aload_0
new example/Counter
dup
iload_1
invokespecial example/Counter.<init>:(I)V
putfield example/CounterOwner.inner:Lexample/Counter;
return
```

So the child `Counter` is initialized before `putfield`, after the direct super call has initialized the owner. `read()` is `aload_0; getfield CounterOwner.inner:Lexample/Counter; invokevirtual Counter.get:()I; ireturn`.

The JVM-only source path is already supported. A regression using the **exact issue fixture** was added to [`jvm_source_compilation.rs`](../tests/jvm_source_compilation.rs): it checks the emitted descriptor/flags and constructor/read instruction references, then runs a Java 21 `-Xverify:all` harness with owners holding 7 and 42. This reuses the existing bytecode/verifier work; it does not establish TASTy producer support.

## Current producer first failures

The frontend and JVM result succeed, while the paired producer stops earlier:

| Path | Observed first result for the exact fixture |
| --- | --- |
| One-session Lexer → Parser → Namer → Typer | Succeeds. The field initializer and cross-class `read` call retain canonical typed roots and exact source-index bindings. |
| JVM-only batch | Succeeds and emits the private final reference field, constructor allocation/store, and `read` field/method calls. The exact fixture passes the JDK 21 verifier/runtime regression. |
| `with_paired_jvm_tasty_batch_plan` | Returns `BatchClass { class_index: 1, declaration_index: 0, phase: "member planning" }` with `only unqualified private mutable fields are supported`. `Counter` at batch index 0 is planned first; `CounterOwner.inner` is the first rejected member. |
| Direct `ClassStatePicklePlan::validate` for `CounterOwner` | After constructing the real typed source/member plan, field validation returns `UnsupportedTypeReference { ty: TypeId(52) }`. The pickler field-type contract accepts only Int/Boolean. Field encoding is therefore not reached; `primitive_type_syntax` is a later boundary, not this run's first error. |
| JVM/TASTy compatibility contract and linked batch publication | Not reached from the paired path: the batch planner fails before producing the validated class-state TASTy plan required by the cross-contract. No invalid plan was fabricated to force later stages. |

The independent errors above locate separate bottlenecks: the root batch planner currently filters fields by mutability before its TASTy member plan, while the pickler itself independently rejects reference field types. The audit makes no producer changes and does not claim that clearing the first check would make later encoding or linkage work.

## Negative and unsupported shapes retained

These remain controls, not newly accepted producer inputs. Existing stage checks and focused coverage stay in force:

| Shape | Current rejecting boundary / evidence |
| --- | --- |
| `private[this]` field | JVM field planning requires plain unqualified `private`; visibility syntax is retained in the frontend. Covered by the field visibility controls in [`jvm_stateful_class_contract.rs`](../tests/jvm_stateful_class_contract.rs). |
| Public/protected or otherwise unplanned field | The bounded JVM source field planner rejects unsupported visibility/ownership before emission; class-state TASTy validation also requires its private field subset. |
| Private mutable reference field or uninitialized private `val` | JVM-only field behavior is separate. For paired class-state TASTy, a reference type fails `UnsupportedTypeReference`; a missing initializer fails the field-plan validation. This audit does not broaden either path. |
| Initializer type mismatch, missing `Counter`, or wrong owner/package | The Typer's expected-type/name/owner resolution or the paired same-package planned-class check rejects the source before artifact publication. No name-only class match is accepted. |
| Private/inaccessible, secondary, generic, or argument-mismatched constructor | The Typer and bounded primary-constructor planner reject these shapes; see the constructor controls in [`jvm_source_compilation.rs`](../tests/jvm_source_compilation.rs) and [`jvm-reference-fields-constructors.md`](jvm-reference-fields-constructors.md). |
| Nested `new`, factory-valued or branch-valued reference field initializer | The JVM initializer planner/codegen rejects expressions outside the one direct planned `new`; the paired pickler currently rejects the reference field type even earlier. |
| `this` escaping before the superclass call, stale/foreign field identities, mismatched TASTy UUID/package path, or incomplete class pair | Existing constructor verification, canonical-source validation and paired-artifact link/path checks reject these at their owning stages. No partial pair is published by the paired artifact validator. |

The JVM-only and TASTy producer gates are different evidence surfaces. A passing `CounterOwner` classfile test cannot be used as evidence that its `VALDEF`, `NEW`, constructor selection, or receiver call can be emitted into Rust-produced TASTy.

## Next ownership boundary

This issue is complete as an evidence-only audit. A follow-up that enables the paired source form must separately address: (1) batch collection/planning of immutable private fields; (2) read-only class-state validation for canonical reference `TypeRef`s and approved `new` initializers; (3) TASTy type/reference/initializer encoding and signed constructor identity; and (4) cross-class linked-artifact proof plus Scala 3.9 consumer verification. Any typer blocker must go to the Typer owner; this audit does not change the typer, JVM backend, TASTy format, or producer.

## Checks run

- `cargo +1.98.1 test -p dotty --test jvm_tasty_object_composition --locked -- --nocapture`
- `DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo +stable test -p dotty --test jvm_source_compilation exact_tasty_object_composition_fixture_emits_reference_field_and_executes --locked -- --nocapture`
- Exact Scala 3.9 source compilation, independent Scala client compilation, `javap -v -p -c`, and `java -Xverify:all -ea` with the pinned source checkout and Temurin JDK listed above.

These are focused checks only. The complete repository gate results are reported with the PR.
