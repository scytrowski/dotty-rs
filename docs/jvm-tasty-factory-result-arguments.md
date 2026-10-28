# Factory-result constructor arguments: Scala 3.9 evidence and Rust proof

Issue [#1832](https://github.com/scytrowski/dotty-rs/issues/1832) pins the
Scala 3.9 oracle and records the original Rust layer observations for
`new CounterOwner(makeCounter(seed))`. Issue
[#1833](https://github.com/scytrowski/dotty-rs/issues/1833) extends the
read-only source/typed proof to that exact same-instance planned call. It does
not add expression-body TASTy serialization or change JVM byte generation.

## Pinned source and oracle provenance

The positive source is
[`CounterOwnerFactory.scala`](../tests/fixtures/jvm_tasty_factory_result_arguments/CounterOwnerFactory.scala),
compiled alone as one package and one Scala frontend compilation. Its separate
client is
[`Consumer.scala`](../tests/fixtures/jvm_tasty_factory_result_arguments/Consumer.scala);
it was compiled later against only the Scala-produced classes and the pinned
Scala 3.9 libraries.

The initially visible checkout was at
`8ddf0f3bd3e3e51b99267159aa5b973bbb97da47`; the audit was isolated on
`origin/main` at `b62fbfda7421e2afac423ccc1251d31886e784c7` (merge of #1830)
before observations were recorded. Both SHAs are in the manifest. The invocation
uses Scala checkout revision
`777528f19a58e794c9954a42f433373472ec57f8`, reporting
`3.9.0-bin-SNAPSHOT-git-777528f`, with `-release 21`. The runtime is Temurin
21.0.8+9, and the generated classfiles have major version 65. The exact
compiler/library/runtime input hashes and six output hashes are in
[`oracle.json`](../tests/fixtures/jvm_tasty_factory_result_arguments/oracle.json).
The oracle output directory was empty before compilation.

The compiler produced exactly these six files:

| Class | Classfile | TASTy sidecar | TASTY UUID (classfile ↔ header) | Class SHA-256 | TASTy SHA-256 |
| --- | --- | --- | --- | --- | --- |
| `example.Counter` | `Counter.class` | `Counter.tasty` | `00d64806c49b36e800ec22da2f7eb01b` | `231c429c58b0a4bf03321179d03f9a1e7c3d91e7907c0ac9d65060f43d687cec` | `27a05a5fb499ea17cfa3066c44e28f87cecff3ec45fb8f59b82d2e27771404dc` |
| `example.CounterOwner` | `CounterOwner.class` | `CounterOwner.tasty` | `0035b89b8dc3a7ee00dd6c3944864066` | `f4abfe5b6ff8581af8971fdf8b72f3ead5b820cff4cf001fec81172cfbf01df9` | `394b13c4eb677b8a0c320975bf96dbe602fab5df6b74804fac09094ef0d7b2bc` |
| `example.CounterOwnerFactory` | `CounterOwnerFactory.class` | `CounterOwnerFactory.tasty` | `00526cc942642dd2001c7c921db70921` | `989454c0d6a5f207c23d40965456a46949d99b55cbc68b3f5070313bc4ed948a` | `6e8fb6697c053c4b0fe84d7ad802fb96f6018ea5e27835703a5166b8e122f923` |

The Rust classfile and TASTy decoders assert class names, major version 65,
TASTy 28.9.0, each class's own UUID link, descriptors, and Factory bytecode.

## JVM ABI and instruction order

`CounterOwner.<init>` is public with descriptor
`(Lexample/Counter;)V`. The Factory methods are public with descriptors
`makeCounter(I)Lexample/Counter;` and
`create(I)Lexample/CounterOwner;`.

Pinned `javap -v -p -c` reports `create` as follows:

```text
0:  new           #25  // class example/CounterOwner
3:  dup
4:  aload_0
5:  iload_1
6:  invokevirtual #27  // Methodref example/CounterOwnerFactory.makeCounter:(I)Lexample/Counter;
9:  invokespecial #30  // Methodref example/CounterOwner.<init>:(Lexample/Counter;)V
12: areturn
```

The method has `stack=4`, `locals=2`, and `ACC_PUBLIC`. The JVM allocates the
outer Owner first, calls the instance factory on `this` with the original
`seed`, passes that returned `Counter` to the Owner constructor, and returns
the initialized Owner. In `create`, the method reference owner is
`CounterOwnerFactory`, and the outer constructor reference owner is
`CounterOwner`. The separate constructor reference in `makeCounter` targets
`Counter.<init>`. No instruction offsets, owner or descriptor were inferred
from the Rust output.

## Decoded Factory TASTy

The pinned Scala sidecar contains both `DEFDEF` nodes:

- `makeCounter(seed: scala.Int): example.Counter`, whose RHS is
  `new example.Counter(seed)`.
- `create(seed: scala.Int): example.CounterOwner`, whose RHS is
  `new example.CounterOwner(CounterOwnerFactory.this.makeCounter(seed))`.

The Rust decoder observes TASTy version 28.9.0 and these wire tags and
addresses in `CounterOwnerFactory.tasty`:

| Node/reference | Address | Wire tag |
| --- | ---: | ---: |
| `makeCounter` `DEFDEF` | 30 | 130 (`DEFDEF`) |
| `makeCounter.seed` `PARAM` | 33 | 134 (`PARAM`) |
| `makeCounter` constructor `APPLY` / `<init>` `SELECTin` | 48 / 50 | 136 / 176 |
| `create` `DEFDEF` | 62 | 130 (`DEFDEF`) |
| `create.seed` `PARAM` | 65 | 134 (`PARAM`) |
| `create` outer constructor `APPLY` | 78 | 136 (`APPLY`) |
| selected Owner constructor | 80 | 176 (`SELECTin`) |
| factory invocation `APPLY` | 90 | 136 (`APPLY`) |
| selected `makeCounter` | 92 | 176 (`SELECTin`) |
| factory receiver | in the `SELECTin` tree | 91 (`QUALTHIS`) |
| `create.seed` reference argument | points to address 65 | 62 (`TERMREFdirect`) |
| `makeCounter.seed` argument | points to address 33 | 62 (`TERMREFdirect`) |
| Owner allocation tree | in the constructor `SELECTin` tree | 95 (`NEW`) |

The constructor selection's signed name has the term signature
`example.Counter => example.CounterOwner`. The factory selection's signed name
has `scala.Int => example.Counter`. Its qualifier is the Factory's qualified
`this`; the argument's `TERMREFdirect` points to `create.seed` at address 65,
not `makeCounter.seed` at 33. Thus the two method definitions remain distinct,
and the inner call consumes the original `create` formal.

For comparison, the existing pinned nested-constructor oracle represents
`new CounterOwner(new Counter(seed))` with two constructor applications and
two `NEW` trees under `create`: its recorded addresses are `DEFDEF=30`,
`PARAM=33`, `APPLY=48/60`, `SELECTin=50/62`, `NEW=53/65`, and the direct seed
reference points to 33. The new oracle instead has a Factory method
`SELECTin`/`APPLY` between the outer Owner construction and the seed reference.
This comparison comes from decoding both genuine Scala-produced sidecars.

## Independent Scala consumer

The consumer was separately compiled against the three oracle classfiles plus
only `scala3-library_3` and `scala-library` from the pinned checkout. It runs
with `java -Xverify:all`, calls `create(7)`, `create(-9)`, and `create(3)`, and
checks all three reads. It also calls `makeCounter(7)` twice, checks the
returned references differ, mutates one to 41, and confirms the other still
reads 7. It prints `scala39-factory-result-arguments=ok`.

This is a Scala-produced oracle baseline only. It is not Rust-to-Scala
interoperability evidence.

## Rust one-session identity map and first failures

The focused Rust test runs the original source through one Lexer → Parser →
Namer → `SourceTyper` session. These IDs are the observed allocation in that
test, not stable identifiers:

| Declaration | Canonical identity |
| --- | ---: |
| `Counter` / `Counter.<init>` / `Counter.start` | `SymbolId(39)` / `SymbolId(43)` / `SymbolId(44)` |
| `CounterOwner` / `CounterOwner.<init>` / `counter` | `SymbolId(40)` / `SymbolId(50)` / `SymbolId(51)` |
| `CounterOwnerFactory` / `makeCounter` / `makeCounter.seed` | `SymbolId(41)` / `SymbolId(55)` / `SymbolId(57)` |
| `CounterOwnerFactory.create` / `create.seed` | `SymbolId(56)` / `SymbolId(58)` |

`makeCounter` retains a one-parameter `Int` method type and canonical
`Counter` result. In the typed `create` tree, the factory function is a
`TermRef` for `SymbolId(55)` with prefix `ThisType(SymbolId(41))`; its call
result is `TypeRef(SymbolId(39))`. The original seed expression is a
`TermRef(SymbolId(58))`. The outer selected constructor is
`SymbolId(50)`, and both the typed `New` and complete RHS have
`TypeRef(SymbolId(40))`.

The test also records these source→typed roots from that session:

| Source tree | Typed tree | Meaning |
| --- | --- | --- |
| `TreeId(52)` | `TreeId(18)` | complete `create` RHS |
| `TreeId(48)` | `TreeId(14)` | selected `CounterOwner.<init>` |
| `TreeId(47)` | `TreeId(13)` | original Owner `New` |
| `TreeId(46)` | `TreeId(12)` | Owner type syntax |
| `TreeId(51)` | `TreeId(17)` | `makeCounter(seed)` application |
| `TreeId(49)` | `TreeId(15)` | original unqualified `makeCounter` reference |
| `TreeId(50)` | `TreeId(16)` | original `seed` reference |

The layer results below are independent observations; downstream stages are
not inferred from the first rejection.

| Layer | First observed result |
| --- | --- |
| Lexer, parser, namer, typer | Succeed on the original one-file source; all listed bindings are present. |
| Class-state TASTy source plan | `ClassStatePicklePlan::validate` succeeds for the real Factory methods and same-session bindings. This validates class-state/source facts; it is not expression-body encoding. |
| Source/typed cross-class constructor proof | Accepts `Formal`, `NestedNew`, and `PlannedCall` receipts. `create` retains the exact original seed, `makeCounter` selection and body, same-class `this` receiver, canonical `Counter` result, outer Owner constructor, and one-session source/typed roots. |
| Paired JVM/TASTy planner | Reaches `JVM/TASTy cross-contract validation` and rejects with `TastyPlan(Unsupported { feature: BodyExpression, ... })`. The proof no longer fails at source shape; body serialization remains unsupported. |
| Expression-body TASTy plan/encoding | The paired path stops at its existing unsupported expression contract. The standalone class-state API still validates only signatures/state. No complete Rust TASTy bytes were produced for this source. |
| Paired artifact contract/publisher | Unreachable: paired planning rejects before returning a complete Rust classfile/TASTy batch, so no artifact contract or publisher input is available. No rejection is attributed to these later layers. |
| JVM-only emission | The exact original packaged source succeeds through the JVM-only source pipeline with its canonical three-class bindings, field plans (`Counter.current`, `CounterOwner.inner`), and supported initialization plans (primary constructors for Counter and Owner; implicit no-arg for Factory). The emitted Factory `create` method has `new`, `dup`, `aload_0`, `iload_1`, `invokevirtual CounterOwnerFactory.makeCounter:(I)Lexample/Counter;`, `invokespecial CounterOwner.<init>:(Lexample/Counter;)V`, `areturn`; its invocation owners and descriptors are asserted from the emitted classfile. Under the strict JDK 21 gate, a Java harness verifies all three generated classes and checks `create(7)`, `create(-9)`, distinct mutable Counters, and state isolation. This is separate from paired TASTy production. |

## Controls and unsupported cases

| Control | Evidence / first result |
| --- | --- |
| Rename package, classes, and factory method | Genuine renamed source receives both helper and planned-call proofs; paired planning then reaches the unchanged body-expression rejection. |
| Reorder class declarations | Genuine reordered source receives both proofs; helper proof linkage is checked after the source-ordered scan, so it does not depend on declaration order. |
| Explicit `this.makeCounter(seed)` | Accepted as `PlannedCall`; the receipt retains the original source and typed `This` receiver. |
| Legacy direct formal argument | Existing reference-constructor regression accepts and serializes the exact caller formal. |
| Nested `new Counter(seed)` argument | Existing pinned nested-constructor regression accepts the source proof and decodes the two constructor applications. |
| Previous JVM-only object-graph fixture | Remains a separate regression for `new Holder(makeCounter(seed))`; it is not used as evidence for the original packaged source. |
| Factory call with wrong arity | Source typing rejects with `ApplicationArityMismatch` before proof. |
| Factory declared with a mismatching result | Source typing rejects with `ApplicationArgumentTypeMismatch` when that result is supplied to the Owner constructor. |
| Field argument | Source proof rejects: `constructor argument must resolve to the exact caller formal`. |
| Null argument | Source proof rejects: `constructor argument must be the exact caller formal, one nested New, or one planned call`. |
| Conditional argument | Source proof rejects at the bounded argument-shape check. |
| Overloaded same-class helper | A real source with a Boolean overload rejects at `SourceShape("overloaded planned helper methods are unsupported")`. |
| Foreign same-named helper owner | Test-only corruption replaces the selected method `TermRef` with a real same-session method from another planned class; the proof rejects with `SourceIdentity` before returning receipts. |
| Disconnected explicit receiver | Test-only same-session corruption changes only the typed helper `Select.qualifier`; the proof rejects with `TypedBinding` even though the separately mapped `this`, method `TermRef`, spans, and source mappings remain intact. |
| Missing planned-call function binding | Test-only removal of the original source-to-typed edge rejects with `TypedBinding("planned helper function has no source-to-typed binding")`. |
| Local/block argument | Not measured in this increment. The issue allows deferred cases; the matrix uses the direct call path only and does not make a claim about block-root scanning. |
| Private/static/super/implicit/using/generic/external factory method | Not accepted by the proof contract: the helper must be a public, monomorphic, plain instance method with one ordinary formal, a canonical source definition, and an exact same-class `this` receiver. Source typing or earlier planning can reject some of these before the proof stage. |
| Wrong constructor | Unmeasured. No alternate constructor shape is needed to observe the current factory-call boundary. |
| Missing/stale typed binding or wrong typed constructor owner | Unmeasured. The issue prohibits forged canonical identities, and the one-session source index supplies the actual mappings. |
| Missing/corrupt sidecar | Unmeasured for this source. No Rust batch was emitted to serve as a publisher input; the existing linked-batch regression has separate corrupt-sidecar controls. |

Unmeasured controls have no attributed rejection stage.

## Validated proof model and boundary

`ValidatedCrossClassConstructorCallProof` contains one opaque
`ConstructorArgumentProof`. Its private sum type is `Formal`, `NestedNew`, or
`PlannedCall`; callers cannot construct a partial variant. The old
`nested_argument()` accessor remains available and returns a value only for
`NestedNew`.

Each receipt keeps canonical source and typed tree IDs, the original caller
formal, expected and result `TypeId`s, and selected owner/member identities.
For `Formal`, the variant's `result_type` is the value type of the original
argument expression; the enclosing constructor proof separately retains the
method's result type. This keeps primitive and reference formals distinct from
the class constructed by the method.
`PlannedCall` additionally keeps the original helper `Apply` and function,
optional explicit receiver, helper method definition and body roots, selected
helper symbol, and helper formal. Validation checks exact `SourceId`-scoped
source/typed mappings and matching spans; canonical source definitions and
owners; the caller's original formal; one plain `(A)R` helper method; the
same-class `this` prefix (including unqualified selections); its exact class
result; exact `Ident`/`Select` shape parity and the mapped typed `Select`
qualifier; and its source/JVM method body. A post-scan link requires that helper's
own direct-formal constructor body have passed the established proof in the
same `JvmCompilationUnit`, independent of class declaration order.

The proof scan remains read-only and all-or-error: no partial receipts escape
when any candidate fails. It does not infer identity from fixture names or JVM
descriptors, synthesize or retype trees, traverse general call graphs, or
authenticate foreign same-slot arena IDs. The caller must keep the existing
single-session invariant.

Proof acceptance is separate from expression serialization. In this issue the
original source reaches `BodyExpression` rejection in the existing
JVM/TASTy cross-contract validator, before any paired batch or artifact
publisher result exists. The direct-formal, nested-constructor, and fixture
artifact hashes remain covered by the focused regression tests.

## Reproduction

Set `SCALA39_ROOT` to the pinned Scala checkout and `JAVA_HOME` to JDK 21.
From the repository root, the following starts with an empty task-specific
output directory, compiles the source alone, then compiles and runs the client
in its separate output directory:

```sh
export SCALA39_ROOT=/path/to/scala39-source
export JAVA_HOME=/path/to/jdk-21
export PATH="$JAVA_HOME/bin:$PATH"
SCALAC="$SCALA39_ROOT/dist/target/universal/stage/bin/scalac"
SCALA3_LIBRARY="$SCALA39_ROOT/dist/target/universal/stage/maven2/org/scala-lang/scala3-library_3/3.9.0-bin-SNAPSHOT/scala3-library_3-3.9.0-bin-SNAPSHOT.jar"
SCALA_LIBRARY="$SCALA39_ROOT/dist/target/universal/stage/maven2/org/scala-lang/scala-library/3.9.0-bin-SNAPSHOT/scala-library-3.9.0-bin-SNAPSHOT.jar"
ORACLE=target/issue1832-oracle
rm -rf "$ORACLE"
mkdir -p "$ORACLE/classes" "$ORACLE/client"
"$SCALAC" -release 21 \
  -sourceroot tests/fixtures/jvm_tasty_factory_result_arguments \
  -d "$ORACLE/classes" \
  tests/fixtures/jvm_tasty_factory_result_arguments/CounterOwnerFactory.scala
find "$ORACLE/classes" -type f | sort
"$JAVA_HOME/bin/javap" -v -p -c -classpath "$ORACLE/classes" \
  example.CounterOwnerFactory example.CounterOwner
"$SCALAC" -release 21 \
  -classpath "$ORACLE/classes:$SCALA3_LIBRARY:$SCALA_LIBRARY" \
  -d "$ORACLE/client" \
  tests/fixtures/jvm_tasty_factory_result_arguments/Consumer.scala
"$JAVA_HOME/bin/java" -Xverify:all \
  -cp "$ORACLE/classes:$ORACLE/client:$SCALA3_LIBRARY:$SCALA_LIBRARY" \
  consumer.FactoryAuditConsumer
sha256sum "$ORACLE/classes/example/"*
```

The pinned fixture decoder and Rust source-session audit are run with:

```sh
cargo test -p dotty --test jvm_tasty_factory_result_arguments --locked
cargo test -p dotty --test jvm_tasty_reference_constructor_calls --locked
```

The strict JDK 21 test for this original source's JVM-only emission is:

```sh
DOTTY_CLASSFILE_REQUIRE_JDK=1 \
PATH="$JAVA_HOME/bin:$PATH" \
cargo test -p dotty --test jvm_source_compilation \
  original_factory_result_source_emits_with_complete_state_plans_and_runs_on_jdk \
  --locked -- --exact
```
