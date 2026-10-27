# Scala 3.9 constructor-injection and reference-alias audit

This document records the contract for a plain primary-constructor reference
parameter copied into a private immutable field. It characterizes Scala 3.9
TASTy/JVM output and the bounded Rust pickler support for the exact
`CounterOwner(counter: Counter)` alias shape. It does not add general reference
fields, constructor accessors, or new JVM code generation.

## Pinned inputs and reproduction

The Scala source checkout is pinned to
`777528f19a58e794c9954a42f433373472ec57f8` (Scala 3.9.0). The audit used
Eclipse Temurin `21.0.8+9`, emitting Java 21 classfiles. The source compiler
reported `3.9.0-bin-SNAPSHOT-git-777528f`.

The required source is
[`CounterOwner.scala`](../tests/fixtures/jvm_tasty_constructor_injection/CounterOwner.scala):

```scala
package example

class Counter(start: Int):
  private var current: Int = start
  def get(): Int = current
  def set(value: Int): Unit = current = value

class CounterOwner(counter: Counter):
  private val inner: Counter = counter
  def read(): Int = inner.get()
```

The independent client is
[`Consumer.scala`](../tests/fixtures/jvm_tasty_constructor_injection/Consumer.scala).
It reads `7`, observes the supplied Counter change to `42` and `-9`, then
checks that a second Counter/Owner pair remains independent.

The compiler checkout and the staged runtime libraries used for this run have
these SHA-256 digests:

| Input | SHA-256 |
| --- | --- |
| `CounterOwner.scala` | `e86e00a64089e157a05adbb883b2b109d47d259176911927c9d62bff5f1d374e` |
| `Consumer.scala` | `529165ccda78b55402677c73b1949d60a89cc9c822ba4225a6b61db5d9b1499b` |
| `scala3-compiler_3-3.9.0-bin-SNAPSHOT.jar` | `b56626390028bd06347e3549cf271a03ca7d2e7be491ceff630ccee94e439f4f` |
| `scala3-library_3-3.9.0-bin-SNAPSHOT.jar` | `8c605e1945ae5a3c57d618d02241f7190ecbed744367473c7c3a9b61940c920c` |
| `scala-library-3.9.0-bin-SNAPSHOT.jar` | `bc87575e0099514b17f7e84384b4a9a51d7a376720af8974e04a41a202909434` |
| `tasty-core_3-3.9.0-bin-SNAPSHOT.jar` | `01e1090c1e40303e370e8c12809e238d58da23c859892ac19f17d59ef3de040e` |

Reproduce the oracle from a fresh output directory:

```sh
JAVA_HOME=/home/vdrake6/.jdks/jdk-21
SCALA_ROOT=/home/vdrake6/dotty-rs/.worktrees/scala39-source
export JAVA_HOME PATH="$JAVA_HOME/bin:$PATH"
SCALA_LIBS="$SCALA_ROOT/library/target/scala-library-bootstrapped/scala-library-3.9.0-bin-SNAPSHOT.jar:$SCALA_ROOT/library/target/scala3-library-bootstrapped/scala-3.9.0-bin-SNAPSHOT-nonbootstrapped/scala3-library_3-3.9.0-bin-SNAPSHOT.jar"
SCALA_CP="$(find "$SCALA_ROOT/dist/target/universal/stage/maven2" -type f -name '*.jar' -print | paste -sd:):$SCALA_LIBS:$SCALA_ROOT/tasty/target/tasty-core-bootstrapped/scala-3.9.0-bin-SNAPSHOT-nonbootstrapped/tasty-core_3-3.9.0-bin-SNAPSHOT.jar"
OUT="$PWD/target/issue-1751-fresh"
mkdir -p "$OUT/classes" "$OUT/client"

java -cp "$SCALA_CP" dotty.tools.MainGenericCompiler \
  -classpath "$SCALA_CP" -release 21 -d "$OUT/classes" \
  tests/fixtures/jvm_tasty_constructor_injection/CounterOwner.scala
java -cp "$SCALA_CP" dotty.tools.MainGenericCompiler \
  -classpath "$SCALA_CP:$OUT/classes" -release 21 -d "$OUT/client" \
  tests/fixtures/jvm_tasty_constructor_injection/Consumer.scala
javap -classpath "$OUT/classes" -v -p -c example.Counter
javap -classpath "$OUT/classes" -v -p -c example.CounterOwner
java -cp "$SCALA_CP" dotty.tools.MainGenericCompiler \
  -print-tasty "$OUT/classes/example/Counter.tasty"
java -cp "$SCALA_CP" dotty.tools.MainGenericCompiler \
  -print-tasty "$OUT/classes/example/CounterOwner.tasty"
java -Xverify:all -ea \
  -classpath "$SCALA_LIBS:$OUT/classes:$OUT/client" \
  client.CounterOwnerClient
```

The checked-in Scala 3.9 oracle files under
[`scala39/example`](../tests/fixtures/jvm_tasty_constructor_injection/scala39/example)
were produced by this run from an empty `classes` directory. They are
compiler-generated Scala artifacts, not Rust output. Their sizes, hashes, and
classfile-to-TASTy UUIDs are:

| Artifact | Bytes | SHA-256 | TASTy UUID |
| --- | ---: | --- | --- |
| `Counter.class` | 648 | `59f631c992d37b212b3da49610aeca5fb49144d0e00388fefb951228173b1d0d` | `00e0d273-b7c4-cde8-0062-9c74fcfe8f1b` |
| `Counter.tasty` | 586 | `859104941c204e1995efe077597e576c37be8986f91615180ed0e85064599049` | same as `Counter.class` |
| `CounterOwner.class` | 634 | `8fedebc2f09f3f65ce036027d3845dea12c04e86ae90bfd25f2d83fedcd8063c` | `00dc3e1f-73b2-cdee-00d1-590cd1e69566` |
| `CounterOwner.tasty` | 562 | `23c1a480010c41678ef22467c25a6e0299cbf27f97c5e7560a8011ac0faff653` | same as `CounterOwner.class` |

Both sidecars decode as TASTy `28.9.0`; both classfiles use major version 65.

## Scala wire and JVM observations

`CounterOwner.tasty` distinguishes two same-named constructor parameter
declarations:

| Address | TASTy node | Observation |
| ---: | --- | --- |
| 4 | `TYPEDEF CounterOwner` | Owns the class template, `inner`, constructor, and `read`. |
| 9 | template `PARAM counter: Counter` | Class/template-owned parameter; TASTy flags include `PRIVATE` and `LOCAL`. |
| 30 | primary constructor `DEFDEF` | Its formal is a separate `PARAM counter: Counter` at address 33. |
| 33 | constructor `PARAM counter: Counter` | Constructor signature formal; its type tree shares the template parameter's Counter type tree at address 12. |
| 44 | `VALDEF inner` | Plain private immutable field with the Counter type tree shared from address 12. |
| 51 | `TERMREFsymbol 9` | Field RHS refers to the template-owned parameter at address 9. Its prefix is `THIS` with `TYPEREFsymbol 4`, i.e. the owning `CounterOwner`. No `NEW` appears in this field initializer. |
| 60 | `DEFDEF read(): Int` | Body calls signed `Counter.get(): Int` through a selection of `CounterOwner.inner`. The `get` declaration remains in `Counter.tasty`. |

The source compiler's classfile for `CounterOwner` contains exactly one
physical field `private final inner:Lexample/Counter;`, a public constructor
`(Lexample/Counter;)V`, and public `read()I`. The constructor instruction
sequence is:

```text
aload_0
invokespecial java/lang/Object.<init>:()V
aload_0
aload_1
putfield example/CounterOwner.inner:Lexample/Counter;
return
```

There is no `new Counter` and no `Counter.<init>` call in `CounterOwner`.
`read()` is `aload_0; getfield CounterOwner.inner:Lexample/Counter;
invokevirtual Counter.get:()I; ireturn`. `Counter` separately has a private
mutable `current:I`, public `<init>(I)V`, `get()I`, and `set(I)V`.

The independent Scala client compiled against the generated class/TASTy pairs
and ran with `java -Xverify:all -ea`. It observed `7 → 42 → -9` through the
same injected Counter reference. A second Owner retained its independent
Counter(3). The checked-in structural regression pins classfile descriptors,
field flags, bytecode references, TASTy version/UUIDs, both `PARAM` addresses,
and the exact field RHS target.

Classpath controls distinguish TASTy discovery from JVM execution:

| Input root | Compile-only Scala consumer | Runtime |
| --- | --- | --- |
| Both `.class` files and both `.tasty` sidecars | Succeeds. | Succeeds under `-Xverify:all`; aliasing assertions pass. |
| Only the two `.tasty` sidecars | Succeeds, showing the compiler can discover the members from TASTy. | Fails with `NoClassDefFoundError: example/Counter`, because the JVM classfiles are absent. |
| Only the two `.class` files | Fails with “This `.tasty` file is missing.” for each Scala 3 class. | Not run after compile failure. |
| `CounterOwner.class` paired with `Counter.tasty` in its place | Fails with “CounterOwner.class is out of sync with its TASTy file”; the compiler then cannot resolve `CounterOwner`. | Not run after compile failure. |

These controls were originally captured from Scala-generated oracle artifacts.
The Rust producer regression now independently emits both sidecars, invokes
the pinned compiler's `-decompile` and `-from-tasty` paths, compiles an
independent client, and runs the reconstructed classes with Temurin 21
`-Xverify:all`. The client mutates the exact Counter supplied to the Owner and
checks that the observed sequence is `7 → 42 → -9`, then checks a second pair
remains independent. The test requires the checkout revision above and the
Temurin `21.0.8+9` toolchain; it is enabled by
`DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1` with `SCALA39_ROOT` and `JAVA_HOME`
set. It writes candidate sidecars and rebuilt classes only into an isolated
temporary directory.

The accepted Rust source/typed mapping is deliberately two-stage:

| Semantic identity | TASTy address/use |
| --- | --- |
| Canonical source parameter tree in `SourceSemanticIndex` | Class/template-owned `PARAM counter: Counter` declaration. |
| Derived typed constructor formal from `derived_symbol_at(<init>, source, parameter-tree)` | Separate constructor-owned `PARAM counter: Counter` signature declaration and the typed initializer's `TermRef` target. |
| Exact source `Ident(counter)` initializer bound to its typed `Ident` | Field `VALDEF inner` RHS encodes `TERMREFsymbol` to the canonical template `PARAM`, qualified by `THIS(TYPEREFsymbol(CounterOwner))`, matching Scala 3.9. |

The validator proves the source-to-typed root binding and span, constructor
scope and signature, the canonical/derived parameter relationship, same-batch
Counter class identity and type, and the immutable private field identity
before serialization. The alias branch does not pass a constructor-owned
symbol to the TASTy address encoder. Direct `new Counter(seed)` initializers
retain their separate existing validation and wire lowering. Mutable or
`private[this]` fields, constructor accessors, multiple/default/using
parameters, mismatched types, and non-alias reference expressions remain
rejected before complete sidecar bytes are returned.

Reproduce the classpath controls after the first compile above:

```sh
ROOT="$PWD/target/issue-1751-controls"
mkdir -p "$ROOT/tasty-only/example" "$ROOT/tasty-client" \
  "$ROOT/classfile-only/example" "$ROOT/classfile-client" \
  "$ROOT/mismatched/example" "$ROOT/mismatched-client"
cp "$OUT/classes/example/Counter.tasty" "$OUT/classes/example/CounterOwner.tasty" \
  "$ROOT/tasty-only/example/"
cp "$OUT/classes/example/Counter.class" "$OUT/classes/example/CounterOwner.class" \
  "$ROOT/classfile-only/example/"
cp "$OUT/classes/example/Counter.class" "$OUT/classes/example/Counter.tasty" \
  "$OUT/classes/example/CounterOwner.class" "$ROOT/mismatched/example/"
cp "$OUT/classes/example/Counter.tasty" "$ROOT/mismatched/example/CounterOwner.tasty"

java -cp "$SCALA_CP" dotty.tools.MainGenericCompiler \
  -classpath "$ROOT/tasty-only:$SCALA_LIBS" -release 21 -d "$ROOT/tasty-client" \
  tests/fixtures/jvm_tasty_constructor_injection/Consumer.scala
java -Xverify:all -ea -classpath "$ROOT/tasty-only:$ROOT/tasty-client:$SCALA_LIBS" \
  client.CounterOwnerClient # expected NoClassDefFoundError: example/Counter

java -cp "$SCALA_CP" dotty.tools.MainGenericCompiler \
  -classpath "$ROOT/classfile-only:$SCALA_LIBS" -release 21 -d "$ROOT/classfile-client" \
  tests/fixtures/jvm_tasty_constructor_injection/Consumer.scala
java -cp "$SCALA_CP" dotty.tools.MainGenericCompiler \
  -classpath "$ROOT/mismatched:$SCALA_LIBS" -release 21 -d "$ROOT/mismatched-client" \
  tests/fixtures/jvm_tasty_constructor_injection/Consumer.scala
```

The last two compiler commands are expected to fail. The classfile-only run
reports the missing sidecar for `Counter` and `CounterOwner`; the mismatched
run reports that `CounterOwner.class` is out of sync with the loaded TASTy,
then fails member lookup.

## Initial audit boundary and first failures

The first-failure matrix below records the repository state during the #1751
audit, before later increments added class-valued constructor TASTy formals,
the field alias wire shape, and complete linked artifact emission. The current
bounded linked-batch contract is described in the next section.

The audit regression parses, names, and types the exact fixture in one real
Rust frontend session. It checks the relationships rather than relying on
session-local numeric IDs:

- `Counter` and `CounterOwner` are the original top-level source classes in
  the same package. `CounterOwner.counter` has a class-owned canonical
  parameter and a distinct constructor-owned derived parameter; both
  completed types point to the same canonical `Counter` class.
- `inner` is the original private immutable `CounterOwner` field. Its declared
  `TypeRef` points to `Counter`. The original initializer source tree maps to
  the typed `Ident`; that typed `TermRef` has `NoPrefix` and targets the exact
  constructor-owned `CounterOwner.<init>` formal.
- The typed `read` body selects the original `inner` field and the original
  `Counter.get` method. Its source body retains the exact source-to-typed root
  binding.

Observed first results for this exact fixture:

| Path | First observed result |
| --- | --- |
| Pinned Scala 3.9 compiler and independent client | Succeeds; the classfile, TASTy, and runtime observations above are from the exact input. |
| Rust Lexer → Parser → Namer → Typer | Succeeds in one session and preserves the canonical parameter/field/method identities above. |
| Rust JVM-only source batch | Succeeds. The issue's JDK-backed regression verifies the incoming local is stored directly and checks reference identity plus mutation-visible reads. |
| Direct `ClassStatePicklePlan::validate` | Returns `UnsupportedTypeReference` while validating the class-owned primary-constructor parameter type. The class-state parameter contract currently accepts only `Int`. Field-type and initializer validators are later and are not reached by this call. |
| Paired JVM/TASTy batch planner | Fails for batch class 1 at phase `TASTy plan validation`, with `UnsupportedTypeReference`. The current constructor-formal contract rejects the completed class-reference parameter before a field-alias plan can be accepted. |
| Constructor parameter projection, field initializer serialization, JVM/TASTy ABI receipt, linked emission | Not reached from the exact paired-plan path. No plan, symbol, or AST was forged to probe later stages. |

The two `UnsupportedTypeReference` reports above are distinct from the
reference field validator: in both paths the constructor-parameter validation
is the observed first failure. A later issue in the approved sprint sequence
owns enabling the bounded TASTy constructor formal and then the alias field.

## Bounded negative matrix

The audit-only regression runs each source variant through the real paired
batch planner and asserts its first rejection phase and error family. It stops
at that first result; no later producer stage is inferred from a masked case.

| Variant | First rejection | What it establishes |
| --- | --- | --- |
| Constructor formal changed from `Counter` to `Int` | `field initializer typing`: `ExpectedExpressionTypeMismatch` | The source formal completes; assigning it to the declared `Counter` field fails during expected-type checking. |
| Same-package `OtherCounter` formal with `Counter` field | `field initializer typing`: `ExpectedExpressionTypeMismatch` | The field does not accept another planned class identity. |
| `Counter` removed from the source batch | `constructor typing`: `TypeNameNotFound` | An unrelated class in the batch does not satisfy the exact planned-reference requirement. |
| `java.lang.String` or unresolved `foreign.Counter` formal | `constructor typing`: `TypeNameNotFound` | These external/foreign type spellings do not resolve in this batch run; no producer plan is made. |
| Private `Counter` | complete JVM unit validation: `UnsupportedJvmVisibility` | Frontend typing reaches unit validation; visibility rejects the target class. |
| Abstract `Counter` | complete JVM unit validation: `UnsupportedJvmClass` | The target's abstract class kind is rejected at whole-unit validation. |
| Trait replacing `Counter` | `source collection`: top-level declaration is not an ordinary class | The batch API rejects a trait before constructor identity planning. |
| `private[this]` alias field | `member planning`: bounded field visibility check | The field is not accepted as the required unqualified private field. |
| Mutable `var` alias field | `method body typing`: `UnstableSelectionPrefix` | The later `read()` body is the first rejection; mutable field selection is outside this shape. |
| Constructor `val` or `var` accessor | complete JVM unit validation: `InvalidPrimaryConstructor(AccessorFieldSignature)` | Accessor symbols remain distinct and are rejected by constructor metadata validation. |
| Generic constructor | `source collection`: generic primary constructors are unsupported | Generic class identity does not enter the paired plan. |
| Defaulted formal | `constructor planning`: `InvalidPrimaryConstructor(SourceShape)` | Default arguments are rejected before TASTy planning. |
| Extra `Int` formal, extra clause, or `using` formal | `TASTy plan validation: UnsupportedTypeReference` on the existing `Counter` formal, or `constructor planning: InvalidPrimaryConstructor(SourceShape)` for unsupported clauses | The extra `Int` formal is masked by the earlier reference-formal rejection; clause forms reach source-shape validation first. |
| Secondary constructor | `member planning`: method is not owned by its declaring class | The bounded member collector rejects it before primary-constructor TASTy validation. |
| Field declared as `Int` | `member planning`: private immutable fields require a canonical same-batch class `TypeRef` | Wrong field type is rejected before initializer serialization. |
| Alias initialized from a different same-typed constructor formal | `TASTy plan validation: UnsupportedTypeReference` | The earlier constructor type blocker masks any later comparison of the actual initializer target. |
| Class-owned template PARAM substituted for the derived constructor formal in a real `ClassStatePicklePlan` | direct class-state validation: `ScopeMembers` for the primary constructor | A real but wrong-owner symbol is rejected; no symbol or tree is fabricated. |
| Same-spelled class nested in a foreign package block | `source collection`: top-level siblings must all be ordinary classes | The paired batch API rejects this package-block shape before the foreign class identity can be resolved; the unresolved `foreign.Counter` case above separately pins constructor typing. |

The missing-sidecar, classfile-only, and stale/mismatched-link rows are
measured by the Scala oracle controls above. These negative outcomes are
characterization baselines, not added producer support. The audit also leaves
erased/repeated parameters and arbitrary initializer expressions for later
work, as required by the bounded scope.

## Linked Rust JVM/TASTy batch support (#1756)

The exact two-class source above now passes through one original frontend
session and produces a complete in-memory set, in source order:

```text
example/Counter.class
example/Counter.tasty
example/CounterOwner.class
example/CounterOwner.tasty
```

The batch planner retains the same `SourceId`, `SemanticStore`, source AST,
`SourceSemanticIndex`, typed AST, and source-to-typed bindings for both
classes. `CounterOwner` keeps distinct class-owned/template and
constructor-owned `counter` `SymbolId`s derived from the same source parameter
tree. Its `inner` field and `read` body remain the original source declarations;
the typed field initializer targets the constructor formal, and its TASTy
initializer encodes the canonical template parameter established by the
pickler proof.

Before returning plans, the cross-producer contract checks the complete
source-ordered JVM/TASTy class set, per-class constructor and member identities,
same-package `Counter` `TypeRef`, descriptors, visibility, and field flags. The
Owner constructor ABI is exactly `(Lexample/Counter;)V`; `inner` is the
`private final` field `Lexample/Counter;`. Its classfile code is checked as the
exact superclass call followed by `aload_0; aload_1; putfield; return`.
Consequently, this bounded constructor path stores the incoming reference once
and contains no `new Counter` or `Counter.<init>` call. The `read()I` bytecode
is checked for `getfield CounterOwner.inner:Lexample/Counter;`,
`invokevirtual Counter.get:()I`, and `ireturn` against the canonical planned
symbols.

Each classfile is linked to the UUID from its own validated TASTy 28.9.0
header. The in-memory emitter encodes both pairs, then validates all four paths
and bytes as one owned batch. A missing or extra path, malformed second
classfile, stale UUID, or invalid class/member link rejects the entire result;
the emitter does not return a successful prefix. The regression compares all
four byte arrays from two fresh frontend sessions and verifies per-class
headers, classfile versions, paths, and UUID links. Existing direct
`new Counter(seed)` batches retain their original validation and output.

This issue proves source-to-TASTy/JVM consistency and complete in-memory batch
validation. The separate #1757 runtime gate proves alias identity on original
Rust-produced classfiles; #1758 owns the independent Scala 3.9 consumer of the
Rust-produced pair. Reconstructing classes from TASTy or passing the existing
JVM-only test is not evidence for either successor gate.

## Rust one-session proof boundary (#1752)

`dotty::jvm_tasty_constructor_alias::with_constructor_reference_alias_proof`
now parses, names, and types the exact two-class source once and invokes a
callback with an immutable `ValidatedConstructorReferenceAlias`. The receipt
retains the original `Counter`, `CounterOwner`, constructor, distinct
class-owned/template and constructor-derived `counter` parameter, `inner`,
`read`, and `Counter.get` identities. It also retains the original source and
typed initializer/read roots and exposes borrowed views of the same semantic
store, source AST, typed AST, source index, and source-to-typed index through
`session()`.

The proof checks the full package/class source shape, class-scope membership,
symbol ownership and provenance, the exact source-index relationship between
the two same-spelled parameter symbols, their completed package-qualified
`Counter` types, the immutable unqualified private field and its typed
constructor-parameter reference, and the `read` → `inner` → `Counter.get`
typed edge. It records the required JVM descriptors
`(Lexample/Counter;)V`, `Lexample/Counter;`, and `()I` from those canonical
source types and owners.

Scala 3.9 TASTy has two `PARAM counter` declarations: the class template
parameter and the primary-constructor formal. The Rust source index preserves
that phase distinction as one canonical class-owned symbol plus a distinct
constructor-owned derived symbol for the same original formal tree. The proof
does not allocate a replacement symbol to make those phases appear identical.

This receipt is only a source/typed identity contract for successor planning.
It does not validate a serialized constructor `PARAM`, field alias TASTy
encoding, linked class/TASTy artifacts, or Scala consumer behavior. No
production TASTy bytes, JVM bytes, or CLI are produced by this proof.

## Original Rust JVM alias proof (#1757)

The linked four-artifact batch is emitted twice from fresh Rust frontend
sessions. Both `Counter.class` and `CounterOwner.class` bytes, TASTy sidecars,
and class-to-sidecar UUIDs are identical across the sessions. The test stages
all four artifacts, then places only the two original Rust-produced classfiles
in a separate runtime classpath alongside the Java harness. `javac` and
`java -Xverify:all` never see Scala-generated or `-from-tasty` rebuilt
classes.

The strict run used Eclipse Temurin `21.0.8+9` (`javac 21.0.8`), producing
classfile major version 65. Before `javac`, the Rust classfile SHA-256 digests
were:

| Original Rust classfile | SHA-256 before `javac` and after `java -Xverify:all` |
| --- | --- |
| `Counter.class` | `a3d893c45a56e187fc1c19ce1e2160d035fd7965bff3cfaab3834d41e20a5e4f` |
| `CounterOwner.class` | `83f9bed85e3b93d6a6c5591ee771408f4b2b350d574ebacca38d21adaae60c54` |

`javap -v -p -c` and the classfile decoder show the public Owner constructor
descriptor `(Lexample/Counter;)V`, `max_locals = 2`, and the exact instruction
sequence `aload_0; invokespecial java/lang/Object.<init>()V; aload_0;
aload_1; putfield CounterOwner.inner:Lexample/Counter;; return`. The referenced
field is private final. Owner's constructor contains no `new` instruction or
`Counter.<init>` call. `read()I` loads that field and invokes
`example/Counter.get:()I`. `javap` confirms `Counter` has the private mutable
`current:I` field and the public `<init>(I)V`, `get()I`, and `set(I)V` members.

The harness receives `Counter(7)`, observes reads of `7`, `42`, and `-9` as
that same object is mutated, and uses reflection to assert
`CounterOwner.inner == counter` by reference identity. A second
`Counter(3)`/Owner pair remains independent while both objects are mutated.
The explicit harness checks finish with `identity=true; first=100; second=13`.
The test verifies both original classfile hashes are unchanged after
`javac` and after JVM execution.

Negative controls cover the observed contract boundaries: the linked-batch
validator rejects an invalid Owner constructor descriptor, field descriptor
or final flag, wrong `Counter.get` dispatch target, forged argument local,
an allocation opcode in place of the argument load, and a field store reordered
before direct-super initialization. The alias contract suite also rejects a missing `CounterOwner`
TASTy sidecar and a corrupt second classfile. With `Counter.class` omitted from
the runtime classpath, `java -Xverify:all` reports
`NoClassDefFoundError: example/Counter`. These are invalid artifacts or
incomplete inputs; none is treated as a valid Rust-produced classfile.

Reproduce the strict JDK runtime proof from the repository root:

```sh
PATH=/home/vdrake6/.jdks/jdk-21/bin:$PATH DOTTY_CLASSFILE_REQUIRE_JDK=1 \
  cargo test -p dotty --test jvm_tasty_artifacts \
  constructor_injected_reference_alias_executes_on_verified_jdk21 \
  --locked -- --nocapture
cargo test -p dotty --test jvm_tasty_contract \
  constructor_reference_alias --locked
```

This proves the JVM behavior of the original Rust classfiles. It does not show
that Scala consumed their TASTy; the pinned Scala consumer is verified below.

## Pinned Scala 3.9 consumer proof (#1758)

The strict consumer test emits the complete `Counter`/`CounterOwner` linked
batch in two fresh source sessions, then compiles only the separate
`tests/fixtures/jvm_tasty_constructor_injection/Consumer.scala` against each
Rust batch. It runs the Scala client with `java -Xverify:all` on the unchanged
Rust classfiles. Reflection in the Scala client checks that each private
`Owner.inner` field is reference-identical to the `Counter` passed to that
Owner; mutations `7 → 42 → -9` remain visible and the independent second pair
still reads `3`. Both original Rust batches and the isolated Scala `-from-tasty`
Owner reconstruction return `-9`.

The oracle run used Scala compiler `3.9.0` from source revision
`777528f19a58e794c9954a42f433373472ec57f8`, and Eclipse Temurin
`21.0.8+9` (`java 21.0.8`, `javac 21.0.8`, `javap 21.0.8`). The producer and
consumer source SHA-256 values were:

| Source | SHA-256 |
| --- | --- |
| `CounterOwner.scala` | `e86e00a64089e157a05adbb883b2b109d47d259176911927c9d62bff5f1d374e` |
| `Consumer.scala` | `374f0c2aba8576b73db08f3c72c742641adb3b28fb6d6afe767e0391f86b8b8d` |

Each Rust classfile has major version 65, each TASTy sidecar is version 28.9.0,
and each classfile's `TASTY` link equals that sidecar's header UUID. The first
batch SHA-256 values and UUIDs were:

| Artifact | SHA-256 | TASTy UUID |
| --- | --- | --- |
| `example/Counter.class` | `15758239b1d5a6dcb73e2ea705c0ad77494ca37813a8d8cc9aa09595f62d621e` | `00d9381a49fab3ac0000000000008193` |
| `example/Counter.tasty` | `8a1022221260fc99d1fdd8e40b855d7931f5b958a419aac072230ddce262eaba` | `00d9381a49fab3ac0000000000008193` |
| `example/CounterOwner.class` | `c06cb0314d7baa2ff4ad2fea0ec136b041b5d097a6e0bfef608f44e9f3982934` | `00ab49827d097def0000000000008195` |
| `example/CounterOwner.tasty` | `05add331ff9fda55596016af151f6c79fbef5bcebb5d1aa8c369ce941487181c` | `00ab49827d097def0000000000008195` |

All four artifact byte arrays and UUIDs match across the fresh sessions. Their
hashes are checked again after Scala compilation and execution. The compiler
classpath puts the Rust batch before the pinned Scala runtime libraries; the
runtime classpath contains that Rust batch, only the Scala consumer and its
small Java launcher, and the pinned Scala runtime libraries. Compiler jars are
also checked not to contain either `example.Counter` or
`example.CounterOwner`. The consumer bytecode references exactly
`Counter.<init>(I)V`, `CounterOwner.<init>(Lexample/Counter;)V`,
`CounterOwner.read()I`, and `Counter.set(I)V`.

Scala's decompiler reads the original Rust `CounterOwner.tasty` as a
`CounterOwner(counter: example.Counter)` with the private `inner` initializer
selecting `CounterOwner.this.counter`, and `read()` selecting
`CounterOwner.this.inner.get()`. The matching TASTy wire test checks the two
distinct template/constructor `PARAM counter` addresses and confirms the field
RHS targets the template parameter. An isolated `-from-tasty` reconstruction
preserves the `(Lexample/Counter;)V` constructor and alias field and runs
against the original Rust `Counter.class`; that reconstructed Owner is never
used by the positive consumer runtime.

The strict negative controls confirm that TASTy-only member lookup finds
`Counter.get` and `CounterOwner.read` after each method is removed from a copy
of its Rust classfile, while the matched classfile-only inputs fail. Additional
controls reject wrong `CounterOwner` formal types, missing classfiles at JVM
runtime, classfiles moved outside their package path (TASTy compile still
succeeds, then JVM loading fails), missing or misplaced sidecars, corrupt
sidecars, a renamed TASTY marker, and class/TASTy UUID mismatches under
`-Werror`. Observed diagnostics
include `value get/read is not a member`, `.tasty file is missing`, `not a
TASTy file`, `out of sync with its TASTy file`, Scala's `Found Other / Required
example.Counter`, and JVM `NoClassDefFoundError: example/Counter` or
`NoClassDefFoundError: example/CounterOwner` at the respective first failing
layer.

## Combined release gate (#1759)

The strict Scala consumer gate carries one emitted `Counter`/`CounterOwner`
batch through staging, Scala compilation, and execution. Its separate
`Consumer.scala` is compiled by pinned Scala 3.9, then a small Java launcher
runs that client with `java -Xverify:all` against the original Rust classfiles.
The Scala client checks reference identity for both owners, observes mutations
`7 → 42 → -9`, and confirms the independent `Counter(3)` owner remains at `3`.
The consumer harness verifies both classes' pre/post hashes and classpath order;
Scala-rebuilt owner output is used only for the isolated `-from-tasty` check.

The alias batch now also has a publisher failure matrix using the same exact
producer source. It rejects an incomplete pair list, foreign source session,
duplicate class identity, duplicate/unsafe paths, malformed Owner classfile or
sidecar, and a stale Owner TASTy link before creating the destination. Injected
write failures after each of the four staged artifacts, before promotion, and
after promotion leave no accepted destination. A pre-existing destination and
its sentinel are preserved. These are handled-error guarantees; they do not
claim cross-file crash atomicity.

The release gates ran on PR #1790's merged tree (`04dbdd3`). GitHub Actions
passed the strict Scala 3.9 release step, the pinned PARAM-address oracle, the
JDK 21 verifier/publisher gate, formatting, frontend boundaries, Clippy,
rustdoc, the full workspace test suite, and diff checks. The pinned toolchain
was Scala `3.9.0` at revision
`777528f19a58e794c9954a42f433373472ec57f8`, Eclipse Temurin `21.0.8+9`, and
Rust/Cargo `1.98.1` (`rustc` commit `48a229cea`). The constructor-alias
consumer's original Rust batch returned `-9`; its second independent owner
returned `3` after the first Counter's mutations.

Reproduce the strict independent Scala consumer with the pinned Scala checkout,
JDK 21, and SHA-verified Maven cache configured:

```sh
SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8 \
JAVA_HOME=/path/to/temurin-21 \
COURSIER_MAVEN_ROOT=/path/to/verified-maven-cache \
DOTTY_REQUIRE_SCALA_TASTY_CONSTRUCTOR_INJECTION_CONSUMER=1 \
  cargo test -p dotty --test jvm_tasty_constructor_injection_scala_consumer \
  --locked -- --nocapture
```

## Validation

The audit was reproduced with the pinned source compiler and JDK above. The
focused repository checks are:

```sh
cargo test -p dotty --test jvm_tasty_object_composition --locked
cargo test -p dotty --test jvm_tasty_object_composition \
  constructor_injection_negative_matrix_pins_first_batch_rejection --locked
cargo test -p dotty --test jvm_tasty_artifacts \
  constructor_alias_batch_publication_rejects_invalid_pairs_and_partial_results \
  --locked -- --nocapture
SCALA39_ROOT=/path/to/pinned-scala JAVA_HOME=/path/to/temurin-21 \
COURSIER_MAVEN_ROOT=/path/to/verified-maven-cache \
DOTTY_REQUIRE_SCALA_TASTY_CONSTRUCTOR_INJECTION_CONSUMER=1 \
  cargo test -p dotty --test jvm_tasty_constructor_injection_scala_consumer \
  --locked -- --nocapture
PATH=/home/vdrake6/.jdks/jdk-21/bin:$PATH DOTTY_CLASSFILE_REQUIRE_JDK=1 \
  cargo test -p dotty --test jvm_source_compilation \
  constructor_injected_reference_alias_emits_and_preserves_object_identity \
  --locked -- --nocapture
```

The first test checks the Scala oracle and the single-session source/TASTy
boundaries. The second emits the exact source through the Rust frontend/JVM
path, verifies the direct `aload_1` field store, and executes a Java 21
identity/mutation harness with full bytecode verification.
