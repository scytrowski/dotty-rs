# Scala 3.9 reference-field object composition audit

This is the characterization-only result for [issue #1715](https://github.com/scytrowski/dotty-rs/issues/1715). It records the Scala 3.9 source/TASTy and JVM ABI for the exact private immutable reference field initialized by `new`. It does not enable Rust TASTy producer support.

## Pinned source and reproduction

The Scala source checkout was exactly `777528f19a58e794c9954a42f433373472ec57f8` (`3.9.0-bin-SNAPSHOT-git-777528f`). The checked-in oracle artifacts were independently reproduced from an empty output directory with Eclipse Temurin `21.0.8+9` and `-release 21`. The mandatory source SHA-256 is `be5898658b6077d493bb8204b624a8702bcc9b6df6ea2642a05629f03bbc1466`; the independent client SHA-256 is `7252a7574c6f35edb1f363a4819ff868ddafe41b6ef186a711646eb19482fe43`. The source compiler JAR used for this run has SHA-256 `b56626390028bd06347e3549cf271a03ca7d2e7be491ceff630ccee94e439f4f`; the four staged runtime/format JAR hashes are recorded below. The workspace-pinned release artifact hashes remain in [`provenance.json`](../tests/fixtures/tasty_pickler/provenance.json).

```sh
JAVA_HOME=/home/vdrake6/.jdks/jdk-21
SCALA_ROOT=/home/vdrake6/dotty-rs/.worktrees/scala39-source
export JAVA_HOME PATH="$JAVA_HOME/bin:$PATH"
SCALA_LIBS="$SCALA_ROOT/dist/target/universal/stage/maven2/org/scala-lang/scala3-library_3/3.9.0-bin-SNAPSHOT/scala3-library_3-3.9.0-bin-SNAPSHOT.jar:$SCALA_ROOT/dist/target/universal/stage/maven2/org/scala-lang/scala-library/3.9.0-bin-SNAPSHOT/scala-library-3.9.0-bin-SNAPSHOT.jar"
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
java -cp "$SCALA_CP" dotty.tools.MainGenericCompiler \
  -print-tasty "$OUT/classes/example/CounterOwner.tasty"
java -Xverify:all -ea \
  -classpath "$SCALA_ROOT/library/target/scala-library-bootstrapped/scala-library-3.9.0-bin-SNAPSHOT.jar:$SCALA_ROOT/library/target/scala3-library-bootstrapped/scala-3.9.0-bin-SNAPSHOT-nonbootstrapped/scala3-library_3-3.9.0-bin-SNAPSHOT.jar:$OUT/classes:$OUT/client" \
  client.CounterOwnerClient
```

The client constructs owners with values 7 and 42 and checks each `read()`. It exits successfully on Java 21 with full bytecode verification. The source is unchanged from the issue's mandatory fixture in [`CounterOwner.scala`](../tests/fixtures/jvm_tasty_object_composition/CounterOwner.scala).

Separate semantic classpath controls against this exact oracle pair produced:

| Classpath root | Scala compile | JVM runtime |
| --- | --- | --- |
| `Counter.tasty`, `CounterOwner.tasty`, no classfiles | `CounterOwnerClient.scala` typechecks successfully through the two sidecars. | Fails on first use with `NoClassDefFoundError: example/CounterOwner`. |
| `Counter.class`, `CounterOwner.class`, no sidecars | Fails at Scala class loading: `This .tasty file is missing.` | Not run because compilation fails. |

The exact probes use the staged Scala 3.9 source compiler and the client already in the fixture. `$SCALA_LIBS` is the Scala 3.9 and Scala 2.13 library JAR pair from the compiler checkout; `$SCALA_CP` is the compiler/runtime classpath in the reproduction above.

```sh
ROOT="$PWD/target/issue-1715-classpath-controls"
ORACLE="$PWD/target/issue-1715-oracle/classes/example"
mkdir -p "$ROOT/tasty-only/example" "$ROOT/classfile-only/example" \
  "$ROOT/tasty-client" "$ROOT/class-client"
cp "$ORACLE/Counter.tasty" "$ORACLE/CounterOwner.tasty" "$ROOT/tasty-only/example/"
cp "$ORACLE/Counter.class" "$ORACLE/CounterOwner.class" "$ROOT/classfile-only/example/"

java -cp "$SCALA_CP" dotty.tools.MainGenericCompiler \
  -classpath "$ROOT/tasty-only:$SCALA_LIBS" -release 21 -d "$ROOT/tasty-client" \
  tests/fixtures/jvm_tasty_object_composition/CounterOwnerClient.scala
java -Xverify:all -ea \
  -classpath "$ROOT/tasty-only:$ROOT/tasty-client:$SCALA_LIBS" \
  client.CounterOwnerClient

java -cp "$SCALA_CP" dotty.tools.MainGenericCompiler \
  -classpath "$ROOT/classfile-only:$SCALA_LIBS" -release 21 -d "$ROOT/class-client" \
  tests/fixtures/jvm_tasty_object_composition/CounterOwnerClient.scala
```

The second compiler invocation is expected to exit nonzero with the missing-sidecar diagnostic; the first compiler invocation succeeds, and its subsequent Java invocation is expected to fail because the runtime root intentionally lacks classfiles. The checked-in structural test pins both sidecars and their classfile UUID links; these controls demonstrate the separate Scala member-discovery and JVM class-loading boundaries. They do not claim Rust-produced TASTy interoperability.

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
| 9 | template parameter `PARAM seed` | Scala emits a private/local template parameter declaration; it is distinct from the primary constructor formal below. |
| 30 | primary `<init>` `DEFDEF` | Its formal is `PARAM seed: Int` at AST address 33. The template and constructor declarations share the source/name-table spelling but are distinct TASTy nodes. |
| 44 | `VALDEF inner` | Tail has `PRIVATE`, `LOCAL`, no `MUTABLE`; type tree is `IDENTtpt Counter → TYPEREF Counter → TERMREFpkg example`. |
| 53 | `inner` initializer `APPLY` | Its function is `SELECTin` address 55 for signed `<init>(scala.Int): example.Counter`; the qualifier is `NEW` address 58. |
| 59–63 | `NEW Counter` / selected constructor owner type | `IDENTtpt Counter` shares the field's TASTy type address 49. The constructor signature names the exact public `(I)V` target. |
| 65 | constructor argument term tree | The raw tree is `TERMREFsymbol 9`, so it targets the template `PARAM seed` at address 9, not the primary constructor formal at address 33. Its qualifier is `THIS(TYPEREFSYMBOL 4, SHAREDtype 51)`. The same-session Rust frontend separately proves that its source/typed argument is the exact constructor-owned `seed` symbol; Scala's field-initialization TASTy projection uses the distinct template parameter. Its classfile projection is `iload_1`. |
| 74 | `DEFDEF read(): Int` | The body at 82 applies signed `get(): scala.Int` at 84; its qualifier selects `inner` at 87 through `QUALTHIS`. The selected `get` declaration is in `Counter.tasty`, not duplicated into the owner sidecar. |

This distinction matters for source identity: the raw `TERMREFsymbol` argument is not a plain textual `seed` leaf, and its declaration address is not the constructor formal's address. The structural regression checks both seed `PARAM` addresses, the `TERMREFsymbol 9` target and its `THIS/TYPEREFSYMBOL 4/SHAREDtype 51` prefix. The exact constructor-owned source/typed relationship is checked separately through the real Rust frontend, and Scala's own TASTy reader successfully compiles the independent client against the sidecars.

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

The original #1715 source probe records the **first paired-producer** boundary
for required same-source variations. The table is historical; #1716 advances
the baseline field beyond the generic member-planning check, as described
below.

| Exact source variation | First observed result | Focused evidence |
| --- | --- | --- |
| Baseline `private val inner: Counter = new Counter(seed)` | Historical #1715 result: generic member-planning rejection. #1716 now accepts field planning and produces one read-only reference-field proof. In the original fixture, the later `read(): Int = inner.get()` still fails JVM/TASTy cross-contract validation with `TastyPlan(Reference { reason: Owner })`. | `paired_producer_advances_private_val_to_cross_contract_boundary`; direct proof and constant-method contract controls are in `jvm_tasty_plan.rs` and `jvm_tasty_contract.rs`. |
| Change `inner` to private `var`, keep `inner.get()` | Batch class 1, declaration 1, `method body typing`: `UnstableSelectionPrefix`. The typer fails before the paired field-plan check. | `paired_producer_reports_first_failure_for_required_negative_shapes`; `docs/jvm-reference-fields-constructors.md` documents the direct mutable-field receiver boundary. |
| Remove the `inner` initializer | Batch class 1, declaration 0, `member planning`: the same private mutable field check fires before a missing-initializer check. | `paired_producer_reports_first_failure_for_required_negative_shapes`. |
| Use `foreign.Counter`/`new foreign.Counter` while sibling `example.Counter` exists | Batch class 1, declaration 0, `member planning`: the same field gate fires first. This run therefore does not claim that the later planned-class/package check was reached. | `paired_producer_reports_first_failure_for_required_negative_shapes`. |
| Refer to absent `Counter` while retaining two ordinary batch classes | Batch class 1, declaration 0, `member planning`: the field gate fires before type/member resolution. | `paired_producer_reports_first_failure_for_required_negative_shapes`. |
| Wrong field reference type, duplicate `inner` field, method-local `inner` shadowing, or a new unrelated receiver in `read` | Each paired-source variant first reaches batch class 1, declaration 0, `member planning` and is rejected by the same private mutable field check. These probes do not assert behavior beyond that first failure. | `paired_producer_reports_first_failure_for_required_negative_shapes`. |
| Change `inner` visibility to `private[this]`, public, or protected | Batch class 1, declaration 0, `member planning`: the private mutable field gate rejects each form before field visibility/TASTy validation. The separate JVM-only path preserves and validates visibility syntax. | `paired_producer_reports_first_failure_for_required_negative_shapes`; `field_metadata_distinguishes_private_this_from_plain_private` in `jvm_stateful_class_contract.rs`. |
| Direct `CounterOwner` class-state pickler plan for the unchanged typed private reference field | `ClassStatePicklePlan::validate` returns `UnsupportedTypeReference { ty: TypeId(52) }`, independent of the earlier paired batch planner error. Encoding and linked-pair publication are not reached. | `one_real_source_session_preserves_object_composition_identity_and_reports_pickler_gap`. |
| Wrong initializer type, constructor argument type, nested `new`, factory-valued or branch-valued initializer, immutable field assignment, `this` escape, forward field read, or secondary/defaulted/`using` constructor | With the unchanged `private val` path, batch class 1, declaration 0, `member planning` is first and masks the later typer/codegen boundary. The existing JVM-only source suite has separate cases that assert the actual Typer or codegen error and no class artifacts. | `paired_producer_reports_first_failure_for_required_negative_shapes`; `unsupported_real_field_and_constructor_sources_fail_without_artifacts` in `jvm_source_compilation.rs`; `docs/jvm-reference-fields-constructors.md`. |
| Wrong constructor arity or argument type; inaccessible/private primary constructor | The source path reports the typed arity/argument error or reaches JVM codegen and rejects call-site visibility, respectively. | Constructor controls in `jvm_source_compilation.rs`, including `private_primary_constructor_is_rejected_without_a_public_constructor_file`. |
| Unplanned/stale constructor/field identity | Not reached through the paired private-val shape because member planning fails first. Existing JVM-only initializer and JVM/TASTy validation tests reject foreign identities before output. | `field_metadata_rejects_reference_fields_outside_the_unit_and_self_initialization` and `jvm_tasty_artifacts.rs`. |
| Incomplete class/TASTy pair, UUID mismatch, unsafe or mismatched package path | Paired artifact validation rejects malformed links/paths and does not publish a partial batch; these are post-plan artifact controls, not source-body errors. | Paired-artifact and batch-publisher controls in `jvm_tasty_artifacts.rs`. |

The historical table reports the first boundary observed before #1716. Where
that earlier private-val field gate prevented a later condition from being
tested, the JVM-only/direct-pickler evidence is labeled separately.

The table above is the historical #1715 audit snapshot. Issue #1716 advances
the baseline private immutable reference field through the batch planner and
read-only class-state proof; its current behavior is recorded below. Negative
cases remain rejected, though some now fail at the more specific TASTy proof
boundary rather than the old generic member-planning gate.

The JVM-only and TASTy producer gates are different evidence surfaces. A passing `CounterOwner` classfile test cannot be used as evidence that its `VALDEF`, `NEW`, constructor selection, or receiver call can be emitted into Rust-produced TASTy.

## Issue #1716: read-only reference-field metadata

The next increment accepts a narrow source-semantic proof for an unqualified
`private val inner: Counter` when `Counter` is a distinct public ordinary class
in the same planned source batch. The proof retains the canonical field and
target class symbols, the exact package-prefixed `TypeRef`, the owner and
visibility/immutability facts, the source declaration, and the source-to-typed
initializer roots. Type syntax projection follows the pinned Scala shape
`IDENTtpt Counter → TYPEREF Counter → TERMREFpkg example`.

Issue #1717 extends that proof through complete TASTy output for the bounded
direct initializer `new Counter(seed)`. The owner sidecar contains the actual
immutable `VALDEF`, `NEW`, selected `<init>(Int): Counter`, and a direct
reference to the owner's primary-constructor `seed`; `Counter` remains in its
own sidecar. The initializer is validated from its exact source and typed
roots, canonical constructor owner, and formal-parameter identity before
emission. Mutable reference fields, external or unplanned targets,
factory/nested/branch initializers, and mismatched source or typed identities
remain rejected.

Issue #1718 extends class-state TASTy validation and emission to the exact
same-batch immutable reference field as the receiver of a supported cross-class
no-argument `Int` method call. For `CounterOwner.read(): Int = inner.get()`,
the emitted TASTy uses `SELECT(inner, QUALTHIS(...))` as the qualifier of the
canonical `SELECTin(get)` and `APPLY`. The field must be the validated private
immutable field, its type must point at the planned sibling class, and `get`
must be the exact planned canonical method. Mutable fields, shadowed names,
unplanned or forged identities, and mismatched source/typed bindings remain
rejected.

The strict pinned Scala 3.9 regression decompiles the Rust-produced sidecars,
consumes them with `-from-tasty`, checks the rebuilt bytecode has `getfield`
and `invokevirtual`, then compiles and executes a Scala client under Java 21
verification. This establishes the TASTy producer shape and Scala consumer
behavior. The paired batch contract also accepts the resulting plan, but this
increment does not separately claim runtime correctness for Rust-emitted
`.class` files and does not expand the Typer or JVM backend.

## Issue #1719: linked JVM/TASTy object-composition batch

The paired artifact coordinator now emits the unchanged `Counter` and
`CounterOwner` source from one frontend session as exactly four ordered owned
artifacts: `Counter.class`, `Counter.tasty`, `CounterOwner.class`, and
`CounterOwner.tasty`. The owner class has only its private final
`inner:Lexample/Counter;` field. Its constructor directly calls
`Object.<init>`, allocates one `Counter` with the original `seed`, and stores
that value in `inner`; `read()I` loads `this.inner` and invokes the exact
planned `Counter.get()I` method. The binary contract checks the actual
`Fieldref` and `Methodref` owners, names, and descriptors against the canonical
source symbols and validated same-batch field proof.

The complete batch validates each classfile against its own TASTy 28.9 header
and class-level link before returning any bytes. Two fresh source sessions
produce byte-identical classfiles, sidecars, paths, and UUIDs. A nested
cross-class call and forged receiver `Fieldref` or missing final field flag
are rejected; a failure in the owner class does not return the already emitted
Counter pair.

The #1719 runtime regression establishes initial execution for owners seeded
with 7 and 42. The expanded #1720 gate below adds edge seeds, repeated reads,
publisher staging, and byte-integrity checks.

## Issue #1720: verified runtime for owned reference fields

`reference_field_owner_emits_a_complete_deterministic_linked_batch` checks
both original classfiles from the exact issue source: Java 21 class-file
version 65.0, `Object` as direct superclass, exact ordered methods/descriptors
and access flags, and the physical fields `Counter.current:I` and
`CounterOwner.inner:Lexample/Counter;` with their source visibility/finality.
For `CounterOwner.<init>(I)V`, it checks the direct-super call occurs first,
then exactly one allocation, `dup`, the original `seed` local load, the
planned `Counter.<init>(I)V` reference, and one store through the checked
`CounterOwner.inner` `Fieldref`. Its `Code` limits are `max_stack=4` and
`max_locals=2`, with no exception table or nested frame attributes. `read()I`
checks `aload_0`, the exact `inner` `Fieldref`, the exact
`Counter.get()I` `Methodref`, `ireturn`, and its `max_stack=1`/
`max_locals=1` limits.

`reference_field_owner_batch_executes_on_verified_jdk21` stages the complete
four-file Rust batch into a new directory with the validated batch publisher.
It runs `javap -v -p -c` on those classfiles, compiles a standalone Java
harness into a separate directory, then executes with `java -Xverify:all`.
The harness creates distinct owners with seeds 7, 42, 0, and -3 and performs
three interleaved reads of each; all reads return their original seed. SHA-256
digests and exact classfile bytes captured from the original Rust batch match
after both `javac` and `java`, so the runtime result cannot come from replaced
or rebuilt dependency classfiles. The local run used Eclipse Temurin 21.0.8.
The test runs under `DOTTY_CLASSFILE_REQUIRE_JDK=1` in both JVM CI gates;
missing or older-than-21 `java`, `javac`, or `javap` fails the test instead
of skipping it.

Contract mutation coverage rejects an incorrect allocation class, constructor
owner or descriptor, missing `dup`, wrong parameter local, wrong field owner
or descriptor, wrong `read` target, wrong `Fieldref` kind, missing final flag,
and missing class artifact before publication. The class-state plan also
rejects a different same-session class `SymbolId` forged as the field identity.
These mutations return typed contract, plan, or manifest errors; no invalid
artifact is accepted for execution.
The implementation reuses the existing classfile verifier and constructor
lowering. It adds no bytecode emitter or verifier-state model. The JDK harness
uses only the original Rust-generated classfiles; Scala's independent
classpath/TASTy consumer remains the separate #1721 check.

## Issue #1721: pinned Scala 3.9 object-composition consumer

The strict opt-in test `jvm_tasty_object_composition_scala_consumer` emits two
complete Rust batches from the exact `CounterOwner.scala` source above, using
distinct source sessions, and publishes each four-file batch into a new empty
root. Both batches have identical ordered paths and byte-identical classfiles
and sidecars. They decode as classfile 65.0 and TASTy 28.9.0, with a distinct
16-byte class-to-sidecar UUID for each class. Only the Rust batch root and the
pinned Scala 3.9 runtime libraries are on the independent client's compile
classpath. Pinned compiler JARs launch scalac and contain no
`example/Counter.class` or `example/CounterOwner.class` entries.

The only Scala client source is
[`Consumer.scala`](../tests/fixtures/jvm_tasty_object_composition/Consumer.scala).
It constructs owners with named seeds 7 and 42, checks the independent values,
and returns 742. The pinned compiler emits only `Consumer.class` and
`Consumer$.class` into a separate empty directory; `javap` confirms constant
pool references to `CounterOwner.<init>(I)V` and `CounterOwner.read()I`. A
separate Java driver invokes the Scala client, and `java -Xverify:all` runs it
against each original Rust batch. Both runs print `742`. SHA-256 and byte
comparisons of every Rust classfile and sidecar before and after compilation
and execution confirm that Scala did not replace or rebuild either runtime
dependency.

TASTy lookup is demonstrated independently for both methods. For each class,
the test removes `Counter.get()I` or `CounterOwner.read()I` from a copy of the
classfile while preserving its original TASTy sidecar and link; the isolated
probe still compiles. The matching classfile-only control fails with a missing
member diagnostic. These intentionally inconsistent classfile/TASTy controls
are compile-only and are never executed. Additional per-class controls reject
missing, misplaced, corrupt, and UUID-mismatched sidecars under `-Werror`.
Changing the classfile attribute name from `TASTY` to `TASTX`, removing the
sidecar, and removing the TASTy-only method makes that method unavailable.
Scala 3.9 selects a neighboring `.tasty` directly when it exists and does not
load the classfile, so its `TASTY` attribute is not consulted during member
lookup. Missing-classfile controls can still compile from TASTy, but Java
execution correctly fails with `NoClassDefFoundError`.

The Rust-produced `CounterOwner.tasty` decompiles to the private immutable
`inner: example.Counter` field with the real `new example.Counter(seed)`
initializer and `read(): Int = CounterOwner.this.inner.get()` body. The
separate `Counter.tasty` declares the source `get` method. Running the pinned
`-from-tasty` reader on an isolated sidecar-only copy reconstructs
`CounterOwner.class`; its bytecode contains the field read and virtual method
dispatch. A separate verified Java harness then runs that reconstructed owner
with only the original Rust `Counter.class` dependency and returns `742`. This
runtime is isolated from both positive client runs, whose classpaths contain
the original Rust `CounterOwner.class`. The original Rust batch validators
check the canonical constructor, field, and method owners and descriptors.

The local strict run used Scala 3.9.0 source revision
`777528f19a58e794c9954a42f433373472ec57f8` and Eclipse Temurin JDK 21.0.8.
The independent client source SHA-256 is
`0f53a121823ad917230d23cd4e9a8e3444ec150e34ace0de9e6c114d2c1ba159`. The
first Rust batch artifacts were:

| Artifact | SHA-256 | Class TASTY UUID bytes |
| --- | --- | --- |
| `Counter.class` | `15758239b1d5a6dcb73e2ea705c0ad77494ca37813a8d8cc9aa09595f62d621e` | `00d9381a49fab3ac0000000000008193` |
| `Counter.tasty` | `8a1022221260fc99d1fdd8e40b855d7931f5b958a419aac072230ddce262eaba` | same as `Counter.class` |
| `CounterOwner.class` | `2f824c65b0bc646208d33fab91c98d2fbd95852d4c7993d5a7c00c0f82f89150` | `005d992e804ee7d30000000000008197` |
| `CounterOwner.tasty` | `9954cba73d3af80e314da391b490cd76c442de21add1dea844f7575723b88b5a` | same as `CounterOwner.class` |

The pinned compiler dependencies were hash-checked before use:
`scala3-compiler_3-3.9.0.jar` `3e2c09a5ca6cef17a55f2f66e123815f9c4ea061fce20d0e9d2371989405a61a`,
`scala3-library_3-3.9.0.jar` `9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba`,
`scala-library-3.9.0.jar` `b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd`,
`tasty-core_3-3.9.0.jar` `66e12dda98bb23e864f3a31f8527c8338bf47bce9b85cc380b368c69c1ba8d0f`,
`scala3-interfaces-3.9.0.jar` `64dfde1d2c6309d65a01c5f3ea836137eb84338dcadeb16f85764561ef4e10db`,
`scala-asm-9.9.0-scala-1.jar` `75ac366e8ecb691e06a7e85041eed0f67919a646e5262fa0901225698c104375`,
`compiler-interface-1.12.0.jar` `92f487a505eafdd0a033e4ff4f38a81509a655c76257c64e6808dd873cc145c3`,
and `util-interface-1.11.5.jar` `1fc950a9e1a35385e79316e23c120d4fd184f6a7af0274e00a703b0dc76376af`.
Representative diagnostics are the Scala “out of sync with its TASTy file”
warning promoted to an error for a UUID mismatch, “This `.tasty` file is
missing” for a missing or misplaced sidecar, “could not be read … not a TASTy
file” for a corrupted sidecar, and a missing `read`/`get` member in the
classfile-only controls.

The required command is
`DOTTY_REQUIRE_SCALA_TASTY_OBJECT_COMPOSITION_CONSUMER=1 cargo test -p dotty --test jvm_tasty_object_composition_scala_consumer --locked -- --nocapture`.
The dedicated Scala 3.9 workflow sets the flag and runs this test alongside the
existing consumer gates. A missing JDK, Scala source checkout, revision, or
cached compiler dependency is therefore a test failure in CI, not a silent
skip.

## Next ownership boundary

The #1715 evidence-only audit is superseded by the #1716 metadata/type proof,
#1717 initializer serialization, #1718 TASTy receiver-call support, #1719
linked JVM/TASTy batch, #1720 verified Java runtime gate, and #1721 pinned
Scala consumer proof. Complete linked source-to-JVM/TASTy end-to-end coverage
remains in #1722. Any typer blocker still belongs to the Typer owner.

## Checks run

- `cargo +1.98.1 test -p dotty --test jvm_tasty_object_composition --locked -- --nocapture`
- `DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo +stable test -p dotty --test jvm_source_compilation exact_tasty_object_composition_fixture_emits_reference_field_and_executes --locked -- --nocapture`
- Exact Scala 3.9 source compilation, independent Scala client compilation, `javap -v -p -c`, and `java -Xverify:all -ea` with the pinned source checkout and Temurin JDK listed above.
- `DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1 cargo test --test jvm_tasty_cross_class_factories class_state_plan_emits_reference_field_receiver_and_scala_executes --locked -- --nocapture` with the pinned Scala 3.9 checkout and Java 21.
- `DOTTY_REQUIRE_SCALA_TASTY_OBJECT_COMPOSITION_CONSUMER=1 cargo test --locked --test jvm_tasty_object_composition_scala_consumer -- --nocapture` with Scala 3.9.0 revision `777528f19a58e794c9954a42f433373472ec57f8` and Temurin JDK 21.0.8.

These are focused checks only. The complete repository gate results are reported with the PR.
