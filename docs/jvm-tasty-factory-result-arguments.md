# Factory-result constructor arguments: Scala 3.9 evidence and Rust proof

Issue #1835's batch and bytecode validation regressions are based on
`04376dc23f92feb423f112e8d3e6c1d4d908414e`, using the repository-pinned Rust
1.98.1 toolchain. The strict JDK 21 gate and the pinned Scala 3.9 reader were
rerun for this source. The inherited nested-constructor test also rechecked all
six recorded Rust SHA-256 values in
[`jvm-tasty-nested-constructor-calls.md`](jvm-tasty-nested-constructor-calls.md).

Issue [#1832](https://github.com/scytrowski/dotty-rs/issues/1832) pins the
Scala 3.9 oracle and records the original Rust layer observations for
`new CounterOwner(makeCounter(seed))`. Issue
[#1833](https://github.com/scytrowski/dotty-rs/issues/1833) extends the
read-only source/typed proof to that exact same-instance planned call. Issue
[#1834](https://github.com/scytrowski/dotty-rs/issues/1834) carries that receipt
through genuine Rust JVM/TASTy batch production, preserves the original method
bodies in TASTy, and checks the resulting sidecars with the pinned Scala reader.
The JVM method bodies and ABI remain the existing output of the JVM backend.

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
The paired batch regression now repeats the exact source through two
independent frontend sessions with distinct `SourceId`s and compares all six
relative paths, class bytes, sidecar bytes, and UUIDs. It also checks that the
Factory retains both `(I)Lexample/Counter;` and
`(I)Lexample/CounterOwner;` methods.

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

The method has `stack=4`, `locals=2`, and `ACC_PUBLIC`. The batch contract
consumes this supported instruction sequence one instruction at a time,
checks each constant-pool operand, and requires complete method-body
consumption. The check does not assume bytecode program counters. The JVM
allocates the outer Owner first, calls the instance factory on `this` with the
original `seed`, passes that returned `Counter` to the Owner constructor, and
returns the initialized Owner. In `create`, the method reference owner is
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
| Paired JVM/TASTy planner | Accepts the original source after validating the proof-backed `makeCounter(seed)` expression and the helper's original constructor body. |
| Expression-body TASTy plan/encoding | Emits both original `DEFDEF` bodies. The `create` body keeps the outer Owner constructor application, nested Factory `APPLY`, signed `SELECTin`, Factory `this` receiver, and a direct reference to `create.seed`; no source tree or name-based identity is reconstructed. |
| Paired artifact contract/publisher | Validates the receipt against the same source/typed roots, canonical helper method, JVM descriptors and method plan. The genuine paired batch contains all three linked classfile/TASTy artifacts. |
| Pinned Scala reader | Scala 3.9.0 `-decompile` displays the Factory call and Owner construction. `-from-tasty` accepts all three Rust sidecars and emits `CounterOwnerFactory.class`. |
| JVM-only emission | The exact original packaged source succeeds through the JVM-only source pipeline with its canonical three-class bindings, field plans (`Counter.current`, `CounterOwner.inner`), and supported initialization plans (primary constructors for Counter and Owner; implicit no-arg for Factory). The emitted Factory `create` method has `new`, `dup`, `aload_0`, `iload_1`, `invokevirtual CounterOwnerFactory.makeCounter:(I)Lexample/Counter;`, `invokespecial CounterOwner.<init>:(Lexample/Counter;)V`, `areturn`; its invocation owners and descriptors are asserted from the emitted classfile. Under the strict JDK 21 gate, a Java harness verifies all three generated classes and checks `create(7)`, `create(-9)`, distinct mutable Counters, and state isolation. This is separate from paired TASTy production. |

## Controls and unsupported cases

| Control | Evidence / first result |
| --- | --- |
| Independent original-source sessions | `jvm_tasty_artifacts::factory_result_argument_emits_six_identical_artifacts_from_independent_sessions` emits the exact source with distinct `SourceId`s and compares all six paths, bytes, and UUIDs. Both class and TASTy versions and each UUID link are decoded. |
| Malformed Factory `create` bytecode | `jvm_tasty_artifacts::factory_result_batch_contract_rejects_corrupt_create_bytecode_before_publication` first accepts the complete canonical batch, then rejects a missing or additional `invokevirtual`, a Factory invocation redirected to `Counter.<init>`, an outer constructor redirected to `makeCounter`, a missing `create` method, a Factory result descriptor changed to `CounterOwner`, and truncated class bytes. Instruction corruption fails at `cross-class invocation`; missing or ABI-mismatched methods fail at `member ABI`; truncation fails during class-file decoding. No rejected artifact set is passed to the publisher. |
| Factory-result publisher negatives and handled failures | `jvm_tasty_artifacts::factory_result_batch_publisher_rejects_corruption_and_all_six_write_failures` uses the mandatory source. Missing/corrupt class or sidecar, wrong source identity, stale UUID, duplicate/unsafe paths, each failure after writes 1–6, and failures before/after promotion are rejected. Every handled fault leaves no destination or staging tree; a preexisting destination sentinel is unchanged. These are handled failures, not crash-atomic durability claims. |
| Rename package, classes, and factory method | Genuine renamed source emits a complete batch; a fresh identical source session produces equal artifacts. |
| Reorder class declarations | Genuine reordered source emits a complete batch; a fresh identical source session produces equal artifacts. Helper proof linkage does not depend on declaration order. |
| Explicit `this.makeCounter(seed)` | Emits the same bounded call form with an explicit TASTy `THIS` receiver; the implicit source form emits `QUALTHIS`. |
| Legacy direct formal argument | Existing reference-constructor regression accepts and serializes the exact caller formal. |
| Nested `new Counter(seed)` argument | Existing pinned nested-constructor regression accepts the source proof and decodes the two constructor applications. |
| Previous JVM-only object-graph fixture | Remains a separate regression for `new Holder(makeCounter(seed))`; it is not used as evidence for the original packaged source. |
| Factory call with wrong arity | Source typing rejects with `ApplicationArityMismatch` before proof. |
| Factory declared with a mismatching result | Source typing rejects with `ApplicationArgumentTypeMismatch` when that result is supplied to the Owner constructor. |
| Field argument | Source proof rejects: `constructor argument must resolve to the exact caller formal`. |
| Null argument | Source proof rejects: `constructor argument must be the exact caller formal, one nested New, or one planned call`. |
| Conditional argument | Source proof rejects at the bounded argument-shape check. |
| Foreign same-named helper owner | Test-only corruption replaces the selected method `TermRef` with a real same-session method from another planned class; the proof rejects with `SourceIdentity` before returning receipts. |
| Disconnected explicit receiver | Test-only same-session corruption changes only the typed helper `Select.qualifier`; the proof rejects with `TypedBinding` even though the separately mapped `this`, method `TermRef`, spans, and source mappings remain intact. |
| Missing planned-call function binding | Test-only removal of the original source-to-typed edge rejects with `TypedBinding("planned helper function has no source-to-typed binding")`. |
| Missing helper member in the TASTy plan | With a real same-session call receipt, omitting `makeCounter` from Factory members rejects first as `SourceOrder { class: Factory }` during validation; no complete `PickledTasty` is returned. |
| Missing call-edge mapping at the pickler boundary | With a real receipt, removing its `source_apply` mapping rejects first as `BodyBinding { actual: None }`; no complete output is returned. |
| Stale call-edge mapping at the pickler boundary | Mapping `source_apply` to the real `typed_function` tree rejects first as `BodyBinding` with that mismatched actual tree; no complete output is returned. |
| Wrong selected member or owner in the receipt | Replacing the selected helper with `create`, or replacing its owner with `CounterOwner`, rejects first as `Reference(Owner)`; no complete output is returned. |
| Wrong result type in the receipt | Replacing the result with the real `Int` formal type rejects first as `Unsupported(BodyExpression)` when it disagrees with the typed Apply result; no complete output is returned. |
| Disconnected typed receiver at the pickler boundary | Replacing the receipt's typed `this` receiver with the real `seed` tree rejects first as `Unsupported(BodyExpression)`; no complete output is returned. |
| Expression depth and visit budgets | A zero projection depth rejects the nested factory Apply as `LimitExceeded(ExpressionDepth, 0)`; depth 1 succeeds. A one-item visit budget rejects with `LimitExceeded(Items)` before output. |
| Parameter shadows helper method | A real source parameter named `makeCounter` rejects first during method body typing with `ApplicationCalleeNotMethod`; no paired plan or TASTy batch is returned. |
| Private helper metadata | A real private `makeCounter` rejects first in the cross-class constructor-call identity proof with `SourceIdentity("candidate method is not the public canonical method owned by its source class")`; no paired plan or TASTy batch is returned. |
| Local/block argument | A real block that stores `makeCounter(seed)` in a local before constructing the Owner rejects first during JVM/TASTy cross-contract validation with `TastyPlan(BodyType)`; no paired plan or TASTy batch is returned. |
| Conditional multi-call argument | Two `makeCounter` calls in the constructor argument reject first in the source proof with `SourceShape("constructor argument must be the exact caller formal, one nested New, or one planned call")`; no paired plan or TASTy batch is returned. |
| Overloaded helper | A real Boolean overload rejects first in the source proof with `SourceShape("overloaded planned helper methods are unsupported")`; no paired plan or TASTy batch is returned. |
| Generic helper | A generic `makeCounter[A]` rejects first during member planning with `generic methods are unsupported`; no paired plan or TASTy batch is returned. |
| Defaulted helper parameter | A default value on `makeCounter`'s formal rejects first in the source proof with `SourceShape("candidate method formal cannot use a default value")`; no paired plan or TASTy batch is returned. |
| Using helper parameter | A contextual `using` helper called without an ordinary argument rejects first during method body typing with `ApplicationArgumentConformanceUnsupported`; no paired plan or TASTy batch is returned. |
| External-owner helper | Calling `CounterMaker.makeCounter` rejects first in the source proof with `SourceIdentity("planned helper must be a public canonical instance method owned by the caller class")`; no paired plan or TASTy batch is returned. |
| Super helper call | A factory inheriting `makeCounter` through `super` rejects first during source collection because explicit parents remain outside this batch plan; no paired plan or TASTy batch is returned. |
| Wrong constructor | Rejected by the complete Factory-result batch contract when `create`'s Owner constructor operand is redirected to the valid Factory helper Methodref; first failure is `cross-class invocation`. |
| Missing/corrupt Factory sidecar | Rejected by the test-only batch publisher before staging. Missing path fails manifest validation; empty/corrupt TASTy fails TASTy decoding. |

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

## Rust-produced TASTy and compatibility evidence

The paired artifact test inspects the Rust-produced bytes rather than only the
Scala oracle. It checks the three output classes, Factory classfile/TASTy UUID
link, both original method descriptors, and source-order method bodies. In
`makeCounter`, the constructor argument's `TERMREFdirect` points to that
method's own `seed` `PARAM`. In `create`, the outer constructor still receives
one nested `APPLY`; its selected helper retains the signed type
`scala.Int => example.Counter`, its receiver is `QUALTHIS` (or explicit
`THIS`), and its `TERMREFdirect` points to `create.seed`. The decoded tree thus
preserves the separate formal identities of both methods.

The strict reader regression is opt-in for routine test runs. With
`DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1`, it verifies checkout revision
`777528f19a58e794c9954a42f433373472ec57f8` and compiler version
`3.9.0-bin-SNAPSHOT-git-777528f`, runs `scalac -decompile` on the Rust
`CounterOwnerFactory.tasty`, checks the reconstructed `makeCounter` body and
the `create` body calling `CounterOwnerFactory.this.makeCounter` with its own
source formal inside `new CounterOwner`, then runs `scalac -from-tasty` over all
three Rust sidecars. Its scratch directory is supplied through
`DOTTY_TASTY_PICKLER_ORACLE_TMP` and kept under the issue worktree's `target/`.
The strict `jvm-tasty-factory-scala-consumer` workflow includes this test
target and creates its scratch directory before running the oracle.
The Scala reader accepts the actual Rust artifacts; this is interoperability
evidence, distinct from the Scala-produced oracle above.

Existing direct-formal and nested-constructor artifact fixtures remain covered
by regression tests. Their encoded bytes are not changed by the new result-call
form; the legacy fixtures and their existing byte expectations continue to
pass.

## Strict JDK 21 original Rust runtime and rejection controls (#1836)

The issue #1836 acceptance test uses the unchanged packaged
[`CounterOwnerFactory.scala`](../tests/fixtures/jvm_tasty_factory_result_arguments/CounterOwnerFactory.scala)
source and one `emit_paired_jvm_tasty_batch` session. It writes the six Rust
class/TASTy files to an isolated class directory, then compiles only a separate
Java harness with `javac --release 21`. Neither a Scala producer nor Scala
runtime libraries are on the execution classpath.

On base commit `699b20d493` (merged #1844 included), Rust 1.98.1 produced these
original artifact hashes. The test checks all six before harness compilation,
after `javac`, and after `java -Xverify:all -ea`, and compares the final files
byte-for-byte with the returned Rust artifacts.

| Artifact | SHA-256 |
| --- | --- |
| `example/Counter.class` | `b38143cee8571a6602865ff6062eca92dd771582f539b156100ffc9647af94ae` |
| `example/Counter.tasty` | `a0e78329d60aba318de825eaddb7668d34838197158653c1b7384188143099c7` |
| `example/CounterOwner.class` | `d914c2dfc5b8d22079be304fc7c463be171394a528b71603de43342a79db91b2` |
| `example/CounterOwner.tasty` | `9bcfbbb9d12f71a50122c6b2af960fe76ffd7f629a57738f61f4d384821f7f13` |
| `example/CounterOwnerFactory.class` | `ecbebc8a5433f559a995364195a6276d85dc736b10ffc8076c3273f805e25981` |
| `example/CounterOwnerFactory.tasty` | `c3ca6a461041bc590d3c6d51fba86b0633fe652a0a55de72f84374e8e2840e16` |

The decoded `Factory.create(I)Lexample/CounterOwner;` code is consumed in full:
one `CounterOwner` allocation, its `dup`, `aload_0`, `iload_1`, exactly one
`invokevirtual CounterOwnerFactory.makeCounter:(I)Lexample/Counter;`, exactly
one `invokespecial CounterOwner.<init>:(Lexample/Counter;)V`, then `areturn`.
The helper body contains the `Counter` allocation and its own constructor
call; `create` does not allocate a `Counter`. `CounterOwner.<init>` stores its
received `Counter` in the private `inner` field. The class-aware CFG analyzer
accepts this flow with max stack 4 and two locals while the outer Owner
allocation remains pending; `javap -v -p -c` reports major version 65 and the
same owners and descriptors. The exact one-call claim is static bytecode
inspection, not a dynamic invocation-count claim.

Temurin JDK `21.0.13+9` compiled and executed the harness under
`java -Xverify:all -ea`. Calls on one factory returned distinct Owners and
distinct private `inner` Counters for seeds `7`, `-9`, and `3`; a second
factory's seed `11` graph was also independent. Reflection retrieved each
private field. Mutating one Counter to `42`, then `-9`, then mutating another
to `100` changed only its associated Owner. The harness printed:
`factory-result=7,-9,3,11; distinct=true; mutation=42,-9,100`.

Each corruption starts from a copy of the accepted batch. The first observed
rejection stages are:

| Control | First authoritative rejection |
| --- | --- |
| Factory call operand redirected to the `Counter.<init>(I)V` reference (wrong helper owner/descriptor) | Paired batch contract, `cross-class invocation` |
| Declared helper result changed to `CounterOwner` | Paired batch contract, `member ABI` |
| Owner constructor operand redirected to the Factory helper reference | Paired batch contract, `cross-class invocation` |
| `create` passes an integer as the virtual receiver | Java 21 verifier, `VerifyError` |
| `create` discards the initialized helper result and returns the still-uninitialized Owner | Java 21 verifier, `VerifyError` |
| `create.max_stack` lowered below the required depth | Java 21 verifier, `VerifyError` |
| Factory class omits `makeCounter` while retaining `create` | JVM linkage, `NoSuchMethodError` |

The first three controls are rejected before an accepted batch or publication
destination is produced. The four runtime probes use private mutated class
trees; they do not alter the canonical six files or the successful runtime
classpath. The focused strict test was run as
`cargo +1.98.1 test --test jvm_tasty_artifacts factory_result_constructor_arguments_preserve_original_identity_on_jdk21 --locked -- --nocapture`
with `DOTTY_CLASSFILE_REQUIRE_JDK=1`, Temurin 21 on `PATH`, and task-local
`TMPDIR`/`CARGO_TARGET_DIR`. The full affected artifact test target and
`cargo +1.98.1 test --workspace --all-targets --locked` also passed. The
mandatory JDK gate passed with:

```sh
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo +1.98.1 test \
  -p dotty-classfile --test classfile_jvm_encoder \
  -p dotty-jvm --lib \
  -p dotty --test jvm_source_compilation \
  --test jvm_stateful_class_contract \
  --locked -- --nocapture
```

The gate environment used Temurin 21.0.13+9 and task-local Cargo and Java temp
directories. `cargo +1.98.1 fmt --all -- --check`, the frontend boundary
guard, Clippy with `-D warnings`, warning-free workspace docs, and
`git diff --check` passed. No Scala producer/oracle run is part of this
Rust-artifact-only acceptance; Scala TASTy consumer interoperability remains
the separate #1837 scope.

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

## Independent Scala 3.9 consumer evidence (#1837)

At base `2f138ecb86a834318a82c527ce3f7944507c627d`, the new strict target
`jvm_tasty_factory_result_scala_consumer` ran with
`DOTTY_REQUIRE_SCALA_TASTY_FACTORY_RESULT_CONSUMER=1`. It uses the exact source
above to produce two independent Rust six-artifact batches, a separately
compiled Scala client, and a renamed/reordered Rust batch. The strict flag
turns unavailable pinned tools into a failing test. The Scala consumer source
is `FactoryResultConsumer.scala`, SHA-256
`6f40cd586653aefbf735f5d511428399b9f71b12c6550ff820cec8d79a5a7d4a`.

The run used Scala source revision
`777528f19a58e794c9954a42f433373472ec57f8` (compiler reports 3.9.0) and
Eclipse Temurin `21.0.13+9` (`x86_64`), with compiler and consumer flags
`-release 21`.
The consumer compile classpath was the original Rust batch root followed by
only `scala3-library_3-3.9.0.jar` and `scala-library-3.9.0.jar`. The compiler
inputs were those libraries plus Scala 3.9.0 compiler, interfaces, TASTy core,
Scala ASM 9.9.0-scala-1, compiler-interface 1.12.0, and util-interface 1.11.5;
all eight artifacts matched the SHA-256 manifest in
`.github/workflows/jvm-tasty-factory-scala-consumer.yml`. The compiler JARs
were checked for producer classes before compilation. Runtime used the same
six original Rust artifacts, the independent client and Java driver, and the
two pinned Scala runtime libraries. No Scala-produced producer classes were
on either classpath.

| Compiler/library artifact | SHA-256 |
| --- | --- |
| `scala3-library_3-3.9.0.jar` | `9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba` |
| `scala-library-3.9.0.jar` | `b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd` |
| `scala3-compiler_3-3.9.0.jar` | `3e2c09a5ca6cef17a55f2f66e123815f9c4ea061fce20d0e9d2371989405a61a` |
| `tasty-core_3-3.9.0.jar` | `66e12dda98bb23e864f3a31f8527c8338bf47bce9b85cc380b368c69c1ba8d0f` |
| `scala3-interfaces-3.9.0.jar` | `64dfde1d2c6309d65a01c5f3ea836137eb84338dcadeb16f85764561ef4e10db` |
| `scala-asm-9.9.0-scala-1.jar` | `75ac366e8ecb691e06a7e85041eed0f67919a646e5262fa0901225698c104375` |
| `compiler-interface-1.12.0.jar` | `92f487a505eafdd0a033e4ff4f38a81509a655c76257c64e6808dd873cc145c3` |
| `util-interface-1.11.5.jar` | `1fc950a9e1a35385e79316e23c120d4fd184f6a7af0274e00a703b0dc76376af` |

| Rust producer class | Class SHA-256 | TASTy SHA-256 | TASTY UUID |
| --- | --- | --- | --- |
| `example.Counter` | `b38143cee8571a6602865ff6062eca92dd771582f539b156100ffc9647af94ae` | `a0e78329d60aba318de825eaddb7668d34838197158653c1b7384188143099c7` | `00da82c5ca4ebdac0000000000008193` |
| `example.CounterOwner` | `d914c2dfc5b8d22079be304fc7c463be171394a528b71603de43342a79db91b2` | `9bcfbbb9d12f71a50122c6b2af960fe76ffd7f629a57738f61f4d384821f7f13` | `004ebc559d6864ef0000000000008195` |
| `example.CounterOwnerFactory` | `ecbebc8a5433f559a995364195a6276d85dc736b10ffc8076c3273f805e25981` | `c3ca6a461041bc590d3c6d51fba86b0633fe652a0a55de72f84374e8e2840e16` | `00ae6e0b2a42b1a10000000000008199` |

The report verified all three classfiles are major version 65, each class's
TASTY attribute equals its sidecar UUID, and fresh Rust sessions produce equal
bytes. Before/after SHA-256 inventories were identical across client compile
and runtime. Running the separate client on the unchanged Rust artifacts with
`java -Xverify:all -ea` printed `7,-9,3` for both independent batches. The
client annotates each `create` result as `CounterOwner` and each direct
`makeCounter` result as `Counter`; it checks the returned values, fresh object
identity, and independent mutable Counter state.

The pinned compiler separately accepted four TASTy-only source lookups after
the selected classfile method was removed. The paired classfile-only controls
then removed that class's sidecar and failed with Scala E008 at member lookup:

| Classfile method hidden | TASTy-only witness | Classfile-only first failure |
| --- | --- | --- |
| `Counter.get()I` | accepted | `value get is not a member of example.Counter` |
| `CounterOwner.read()I` | accepted | `value read is not a member of example.CounterOwner` |
| `CounterOwnerFactory.create(I)CounterOwner` | accepted | `value create is not a member of example.CounterOwnerFactory` |
| `CounterOwnerFactory.makeCounter(I)Counter` | accepted | `value makeCounter is not a member of example.CounterOwnerFactory` |

These method-stripped classfiles are compile-only witnesses and never enter a
runtime classpath. Pinned `-decompile` on the three original Rust sidecars
showed separate Counter and Owner declarations, `makeCounter(seed): Counter`
with `new Counter(seed)`, and `create(seed): CounterOwner` with
`new CounterOwner(CounterOwnerFactory.this.makeCounter(seed₂))` (Scala's
decompiler alpha-renames the second binder). `-from-tasty` accepted all three
original sidecars and emitted exactly Counter, CounterOwner, and
CounterOwnerFactory. Those rebuilt classes were not used by the client or
runtime.

For each of Counter, CounterOwner, and CounterOwnerFactory, the harness
separately tested missing, misplaced, corrupt, and UUID-mismatched sidecars.
Missing and misplaced sidecars were rejected by the reader with
“This `.tasty` file is missing”; corrupt sidecars failed while loading with
“not a TASTy file”; UUID mismatches warned that the classfile was out of sync
with its TASTy, and `-Werror` made that warning fatal. Additional isolated
controls reached these first failures:

- In the retained #1837 run, a classfile-only `makeCounter` result descriptor
  changed from Counter to CounterOwner produced the reader's “This `.tasty`
  file is missing” diagnostic first, followed by E007 (`Found:
  example.CounterOwner`, `Required: example.Counter`) when the consumer
  required Counter. The missing-helper control likewise reported the missing
  sidecar before E008, “value makeCounter is not a member”. The Owner
  constructor control reported the missing sidecar before E173,
  “constructor CounterOwner cannot be accessed”. These were secondary ABI
  diagnostics, not the first failures.
- Removing both Counter artifacts and relocating both under another package
  failed with E008, “type Counter is not a member of example”.
- Changing Counter's classfile-internal owner to `example.Coupler` while leaving
  the artifact at `example/Counter.class` failed during Scala classfile loading:
  “location not matching its contents: contains class example.Coupler”.
- A precompiled client against a verifier-valid Factory class whose
  `create` method entry was renamed reached `NoSuchMethodError`. Omitting
  Counter at runtime reached `NoClassDefFoundError`. The renamed-method
  linkage control retains the method body and descriptor; no method-stripped
  class is run.

The independently emitted batch renamed the package and all three classes and
reordered the declarations. Scala compiled and ran its own client against each
of its two deterministic Rust batches; both runs printed `7,-9,3`. The
following per-class class/TASTy UUID links and hashes were equal across those
independent emissions:

| Renamed class | Class SHA-256 | TASTy SHA-256 | TASTY UUID |
| --- | --- | --- | --- |
| `renamed.Widget` | `f6d53c786231f6c88282932558cfe2cd2469eb5d1bc778ee31eee96f4ecb6fd5` | `a66023e403b8a5a32febb4d189b734bb9cb2378d4caf1f07877a9d5f9a127c04` | `00d39e8d55af90ac0000000000008193` |
| `renamed.WidgetHolder` | `bc4c2ef76458d79e2c63a10100c9d090ac7ae0002240d1377bba522b22f36ff6` | `775ff29f9ad4579f1711adf01668988defac7f1267e4993f0607ef7d8726ca33` | `001dae5ea91964ef0000000000008195` |
| `renamed.WidgetBuilder` | `8d159b7be3cb87a130f3e85270757bfcc41bc713801c6c4a62da4462cdcbd350` | `b22546343121af2945592fb63f4727ddec7bfb751c433fdc413990696c64fc68` | `0084d2f370bd3ea10000000000008199` |

The Rust producer source hash is
`daecae35c324a26522f13f459d14d5cc730ab22d5260f17384802f20572a550c`; the
consumer hash is
`6f40cd586653aefbf735f5d511428399b9f71b12c6550ff820cec8d79a5a7d4a`. The
gate emits a JSON evidence report with the compiler revision/version, JDK
release, eight compiler artifact hashes, all original and renamed hashes and
UUIDs, compile classpath order, runtime outputs, control diagnostics, and
first failing stages.

## Integrated release gate (#1838)

The final integration run started from clean `origin/main` at
`2c9303ea65ab7c6c9d6994cc98be6620906a5bc3`, containing merged prerequisites
#1832–#1837. Their independently reviewed `READY_FOR_MERGE` receipts and merge
commits are [#1832/#1840](https://github.com/scytrowski/dotty-rs/pull/1840)
(`55ad298dd3863e00ce3a80d8d20d6b2ae781b552),
[#1833/#1841](https://github.com/scytrowski/dotty-rs/pull/1841)
(`a34c50ffc397b052edee67e4ec9321b9a0121849),
[#1834/#1843](https://github.com/scytrowski/dotty-rs/pull/1843)
(`04376dc23f92feb423f112e8d3e6c1d4d908414e),
[#1835/#1844](https://github.com/scytrowski/dotty-rs/pull/1844)
(`a316d967ec6c4c698ee9e28329c50fd7636772cc),
[#1836/#1845](https://github.com/scytrowski/dotty-rs/pull/1845)
(`2f138ecb86a834318a82c527ce3f7944507c627d), and
[#1837/#1847](https://github.com/scytrowski/dotty-rs/pull/1847)
(`2c9303ea65ab7c6c9d6994cc98be6620906a5bc3).

The strict Scala consumer and producer suite were rerun against that tree with
Rust/Cargo 1.98.1, Scala checkout
`777528f19a58e794c9954a42f433373472ec57f8` (compiler 3.9.0), and Eclipse
Temurin 21.0.13+9 on x86_64. The consumer used `-release 21`; all eight
compiler JARs matched the workflow SHA-256 manifest. Build outputs, temporary
files, Scala reader output and dependency cache were isolated under the issue
worktree. The producer source SHA-256 is
`daecae35c324a26522f13f459d14d5cc730ab22d5260f17384802f20572a550c`; the
separate Scala client SHA-256 is
`6f40cd586653aefbf735f5d511428399b9f71b12c6550ff820cec8d79a5a7d4a`.

The strict consumer reported all six original Rust outputs byte-identical to
the recorded inventory above, class-file major version 65, TASTy 28.9.0, and
the following classfile-to-sidecar links:

| Class | Classfile SHA-256 | TASTy SHA-256 | TASTY UUID |
| --- | --- | --- | --- |
| `example/Counter` | `b38143cee8571a6602865ff6062eca92dd771582f539b156100ffc9647af94ae` | `a0e78329d60aba318de825eaddb7668d34838197158653c1b7384188143099c7` | `00da82c5ca4ebdac0000000000008193` |
| `example/CounterOwner` | `d914c2dfc5b8d22079be304fc7c463be171394a528b71603de43342a79db91b2` | `9bcfbbb9d12f71a50122c6b2af960fe76ffd7f629a57738f61f4d384821f7f13` | `004ebc559d6864ef0000000000008195` |
| `example/CounterOwnerFactory` | `ecbebc8a5433f559a995364195a6276d85dc736b10ffc8076c3273f805e25981` | `c3ca6a461041bc590d3c6d51fba86b0633fe652a0a55de72f84374e8e2840e16` | `00ae6e0b2a42b1a10000000000008199` |

Two fresh sessions emitted identical bytes for all six original files. The
renamed/reordered producer also emitted deterministic six-file batches. The
official Scala client compiled against each original Rust batch and ran
unchanged producer classes with `java -Xverify:all -ea`, printing `7,-9,3`;
each renamed/reordered client printed the same result. The strict JDK gate
decoded exactly one `CounterOwnerFactory.makeCounter:(I)Lexample/Counter;`
invoke in `create` and the `CounterOwner.<init>:(Lexample/Counter;)V` call,
then executed the original Rust classes with Owner/Counter identity and
mutation-isolation assertions. The Scala reader's TASTy-only lookups and their
classfile-only member controls ran for
Counter, Owner, Factory.create, and Factory.makeCounter. Before/after hashes
matched around compilation and runtime. Scala `-decompile` preserved both
Factory method bodies and the selected `makeCounter(seed)` call inside the
Owner constructor argument; `-from-tasty` accepted the three original Rust
sidecars. The independent original source session and shared Formal,
NestedNew, and PlannedCall proof regressions passed without compiler or
artifact changes.

The #1847 ABI note is closed by paired classfile-only probes. Each probe
renames the classfile `TASTY` attribute and removes the sidecar; the unchanged
classfile baseline must compile before the mutated classfile is tested:

| Probe | Unchanged classfile-only baseline | Mutated first failure |
| --- | --- | --- |
| `makeCounter` result descriptor changed from `(I)Lexample/Counter;` to `(I)Lexample/CounterOwner;` | Compiles with the original descriptor | E007 only: `Found: example.CounterOwner`, `Required: example.Counter`; one error, no missing-sidecar diagnostic |
| Owner reference constructor removed from the classfile | Compiles with `(Lexample/Counter;)V` present | E173 only: `constructor CounterOwner cannot be accessed as a member`; one error, no missing-sidecar diagnostic |

Other sidecar controls intentionally keep the Scala 3 `TASTY` marker. Their
first result remains the Scala reader's missing, corrupt, misplaced, or UUID
out-of-sync diagnostic; any subsequent E007/E008/E173 is recorded as a
secondary diagnostic. The source/proof/pickler/batch negatives and handled
fault injection after each of six writes, before promotion, and after
promotion all passed. The publisher preserves preexisting sentinels and makes
no crash-atomic durability claim.

The following strict suites executed with their required flags set (not
skipped): the Scala/oracle suite passed 30 tests across 12 target summaries;
the JDK 21 gate passed 838 tests across seven target summaries. Neither strict
suite ignored a test.

```sh
DOTTY_REQUIRE_SCALA_TASTY_CONSUMER=1 \
DOTTY_REQUIRE_SCALA_TASTY_MULTICLASS_CONSUMER=1 \
DOTTY_REQUIRE_SCALA_TASTY_CROSS_CLASS_CONSUMER=1 \
DOTTY_REQUIRE_SCALA_TASTY_FACTORY_CONSUMER=1 \
DOTTY_REQUIRE_SCALA_TASTY_OBJECT_COMPOSITION_CONSUMER=1 \
DOTTY_REQUIRE_SCALA_TASTY_CONSTRUCTOR_INJECTION_CONSUMER=1 \
DOTTY_REQUIRE_SCALA_TASTY_REFERENCE_CONSTRUCTOR_CONSUMER=1 \
DOTTY_REQUIRE_SCALA_TASTY_NESTED_CONSTRUCTOR_CONSUMER=1 \
DOTTY_REQUIRE_SCALA_TASTY_FACTORY_RESULT_CONSUMER=1 \
DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1 \
cargo +1.98.1 test \
  --test jvm_tasty_scala_consumer \
  --test jvm_tasty_multiclass_scala_consumer \
  --test jvm_tasty_cross_class_scala_consumer \
  --test jvm_tasty_factory_scala_consumer \
  --test jvm_tasty_object_composition_scala_consumer \
  --test jvm_tasty_constructor_injection_scala_consumer \
  --test jvm_tasty_reference_constructor_scala_consumer \
  --test jvm_tasty_nested_constructor_scala_consumer \
  --test jvm_tasty_factory_result_scala_consumer \
  --test jvm_tasty_cross_class_factories \
  --test jvm_tasty_factory_result_arguments \
  --test tasty_source_pickling --locked -- --nocapture

DOTTY_CLASSFILE_REQUIRE_JDK=1 \
cargo +1.98.1 test \
  -p dotty-classfile --test classfile_jvm_encoder \
  -p dotty-jvm --lib \
  -p dotty --test jvm_source_compilation \
  --test jvm_tasty_artifacts \
  --test jvm_stateful_class_contract --locked -- --nocapture
```

The remaining workspace release checks passed on the issue tree: `cargo
+1.98.1 fmt --all -- --check`, `python3 tools/check-frontend-boundaries.py`,
workspace Clippy with `-D warnings`, workspace rustdoc with
`RUSTDOCFLAGS="-D warnings"`, and `cargo +1.98.1 test --workspace
--all-targets --locked`. Workspace tests reported 7,191 passed, no failures,
and 18 explicitly ignored tests across 157 target summaries. The separate
strict Scala and JDK suites above had zero ignored tests. `git diff --check`
also passed. These checks were run on implementation tree
`4d0105a50f11d1f13266f77e50ac973be12451de`; this ledger update adds no source
or test behavior. The hosted full Scala corpus audit is outside this release
gate.

This bounded result supports one public same-class factory call with one
ordinary argument, a public instance helper whose result is the exact
reference type expected by the Owner constructor, and the already covered
direct-formal and nested-`new` forms. External owners, static/super calls,
overloads, generic or contextual/using helpers, multi-argument or generic
constructors, arbitrary method expressions, cross-unit imports, and a public
source compiler/session API remain deferred. Producer or proof failures belong
to their owning predecessor and are not hidden by this release ledger.
