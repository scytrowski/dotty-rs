# Reference-valued constructor calls: Scala 3.9 audit

Issue [#1793](https://github.com/scytrowski/dotty-rs/issues/1793) is an audit
of a three-class source unit. It records the pinned Scala/JVM behavior and the
current Rust producer boundary; it does not enable a new producer feature.

## Oracle inputs and outputs

The unchanged source is
[`CounterOwnerFactory.scala`](../tests/fixtures/jvm_tasty_reference_constructor_calls/CounterOwnerFactory.scala).
The pinned Scala checkout is revision
`777528f19a58e794c9954a42f433373472ec57f8` (3.9.0-bin-SNAPSHOT-git-777528f),
and the compiler ran on Temurin 21.0.8+9 with `-release 21`. The independent
client is in
[`Consumer.scala`](../tests/fixtures/jvm_tasty_reference_constructor_calls/Consumer.scala).
The checked-in oracle consists of exactly these six files, all under
`scala39/example/`:

| Class | Classfile | TASTy | TASTY attribute / header UUID |
| --- | --- | --- | --- |
| `example.Counter` | `Counter.class` | `Counter.tasty` | `00d64806c49b36e8007989815a7b291b` |
| `example.CounterOwner` | `CounterOwner.class` | `CounterOwner.tasty` | `0035b89b8dc3a7ee0044f9921cbe0966` |
| `example.CounterOwnerFactory` | `CounterOwnerFactory.class` | `CounterOwnerFactory.tasty` | `00a528c19ba3d2ef00878881da6d9f1c` |

Each classfile has major version 65 and a public class declaration. The
decoded TASTy headers are 28.9.0. Each class's `TASTY` link matches its own
sidecar UUID; the test asserts this per pair. SHA-256 hashes for the six
checked-in oracle files are recorded in
[`oracle.json`](../tests/fixtures/jvm_tasty_reference_constructor_calls/oracle.json).

`CounterOwner.<init>` is public with descriptor
`(Lexample/Counter;)V`. `CounterOwnerFactory.wrap` is public with descriptor
`(Lexample/Counter;)Lexample/CounterOwner;`. `javap -v -p -c` reports the body:

```text
0: new           #17  // class example/CounterOwner
3: dup
4: aload_1
5: invokespecial #20  // CounterOwner.<init>:(Lexample/Counter;)V
8: areturn
```

The checked-in Rust test verifies the corresponding exact code bytes
`bb 00 11 59 2b b7 00 14 b0`, one `new` instruction, and a constructor
Methodref owned by `CounterOwner`. `Counter` is passed through; `wrap` does not
allocate another `Counter`.

## TASTy shape

The `wrap` DEFDEF has one source formal named `counter`, whose type syntax
resolves to `example.Counter`; its result syntax resolves to the distinct
`example.CounterOwner`. The body is `APPLY(SELECTin(NEW(CounterOwner),
<init>), counter)`. The selected signed name is `<init>` with term parameter
`example.Counter` and result `example.CounterOwner`. The APPLY has one argument,
and its `TERMREFdirect` points to the address of `wrap`'s actual PARAM node.
The `NEW` and selected constructor type contain AST references to the Owner
type; they do not refer to the `Counter` class. The Owner sidecar remains the
separate compiler-produced `CounterOwner.tasty`.

This differs from the earlier `CounterFactory.create(seed: Int): Counter`
shape: both its formal and constructor argument are `Int`, and the method
returns the newly constructed class. It also differs from constructor
injection: there, `CounterOwner` itself declares the `Counter` formal and alias
field. Here `CounterOwnerFactory.wrap` takes a reference, constructs a
different class, and returns it.

The TASTy decoder test checks the exact tags, signed constructor signature,
argument address identity, and classfile-to-sidecar UUID association. TASTy
address offsets are local to the pinned compiler output and are not public
identity values; the regression asserts the reference relationship instead of
freezing offsets.

## Independent Scala consumer

`Consumer.scala` was compiled in a separate output directory against only the
Scala-produced classes and the staged Scala 3.9 standard libraries. It creates
`Counter(7)`, calls `wrap`, observes 7, mutates the original object to 42, and
observes 42 through `CounterOwner.read()`. It creates a second pair, mutates
the first counter again to -9, and confirms the second owner still reads 3.
The client ran with `java -Xverify:all` and printed
`scala39-reference-constructor-alias=ok`. This establishes the Scala oracle's
object-identity behavior; it says nothing about Rust-produced interoperability.

## Rust one-session identity and current failures

`tests/jvm_tasty_reference_constructor_calls.rs` runs the original one-source
fixture through Lexer → Parser → Namer → Typer with one semantic store. On this
fixture/source ID the canonical symbols are:

| Declaration | SymbolId |
| --- | ---: |
| `Counter` | 39 |
| `Counter.<init>` | 43 |
| `CounterOwner` | 40 |
| `CounterOwner.<init>` | 50 |
| `CounterOwnerFactory` | 41 |
| `CounterOwnerFactory.wrap` | 55 |
| `wrap.counter` | 56 |

These are recorded as the observed allocation in this test, not stable API
identifiers. The test checks owners and exact symbol relationships. Typer
completes `wrap` to a method with parameter `TypeRef(Counter)` and result
`TypeRef(CounterOwner)`. It types the source body as `Apply(Select(New(...)),
counter)`, resolves the selected constructor to the canonical
`CounterOwner.<init>` symbol, and resolves the argument to the actual `wrap`
formal. SourceTypedIndex contains edges for the Apply, Select, New, New type
syntax, and argument trees. The expression and type identities survive naming
and typing without reconstructing them from spelling.

| Layer | Observed result |
| --- | --- |
| Lexer / parser / namer / typer | Succeeds on the original one-session, three-class unit. |
| Factory `ClassStatePicklePlan::validate` | Succeeds with actual source and typed bindings plus all three planned class symbols. |
| Distinct-class method signature proof/projection | Succeeds independently: `PARAM counter` refers to canonical `Counter`; result `IDENTtpt` refers to canonical `CounterOwner`; the two class SymbolIds differ. The proof retains the actual source RHS and typed RHS IDs. |
| Factory class-state TASTy `pickle()` | Succeeds and emits the original `wrap` signature plus `APPLY(SELECTin(NEW(CounterOwner)), TERMREFdirect(wrap.counter))`. |
| Paired JVM/TASTy batch validation | Succeeds for all three source classes, including the generic constructor-call receipt and exact descriptor checks. |
| Artifact integration | Batched classfile plus TASTy publication and runtime execution remain in #1797/#1798. |

The direct class-state `validate()` call accepts the actual signature and
source/typed plan, including the distinct `Counter` parameter and `CounterOwner`
result. Body validation checks the source and typed Apply, selected constructor,
New type, and argument bindings against their original spans. The target
constructor's source-derived formal and method signature must both be exactly
one plain class-reference parameter whose class is planned in the same package;
the caller formal must have that same canonical class TypeRef and be owned by
the calling method. The target constructor result remains the separate
`CounterOwner` class. No reference-category widening, name search, or class
copying is used. No Typer changes were needed.

## TASTy encoding of the reference argument (#1796)

For `wrap(counter: Counter): CounterOwner = new CounterOwner(counter)`, the
Factory method signature keeps its original `PARAM counter` with
`TYPEREF(TERMREFpkg(example), Counter)` and its distinct result
`IDENTtpt(TYPEREF(TERMREFpkg(example), CounterOwner))`. The body is the bounded
wire tree `APPLY(SELECTin(NEW(IDENTtpt(CounterOwner))),
TERMREFdirect(wrap.counter))`. The `SELECTin` signed name is `<init>` with
result signature type `example.CounterOwner` and one term parameter signature
type `example.Counter`. Its qualifier and underlying type reference
CounterOwner's own source declaration; the constructor declaration remains in
the Owner sidecar, and the argument reference targets the Factory method's
original `PARAM` address. The decoder regression verifies both that signed name
and the direct-reference address.

The source validator requires one public, concrete, ordinary, non-generic
planned target class and its accessible public primary constructor with exactly
one plain formal. That formal must be a canonical planned sibling class type;
the caller method must have one ordinary parameter of that exact class type,
and its argument expression must be the corresponding original parameter
identifier. The input class and result class must differ. `Int` constructor
calls retain the previous primitive signature `(scala.Int)C` encoding. Nested
allocation, extra arguments, fields, factory values, null, foreign or
unplanned classes, generic or secondary constructors, and other argument
expressions remain rejected before complete TASTy bytes are returned.

The JVM-only backend's existing `new Holder(counter)` lowering remains a
separate output path. This increment validates the paired plans and serializes
the TASTy sidecars, while six-file publication and runtime identity checks
remain in #1797/#1798.

## Generic source/typed constructor-call proof (#1794)

The batch planner now scans concrete source method RHS roots structurally for
`Apply(Select(New(target), <init>), argument)`. Once that shape is present, the
proof fails closed unless the source method, formal, target class, selected
primary constructor, argument `TermRef`, typed roots, and JVM metadata all have
canonical same-session identities. Every source tree in the proof must carry a
position owned by the proof's `SourceId`, and each mapped expression tree must
retain the exact same source position after typing. Stale typed annotation
bindings return a typed proof error. The caller and constructed class must be
different public ordinary classes in the same package and ordered source/JVM
batch. A reference-valued formal must itself be another planned same-batch
class. The exact erased method and constructor descriptors are retained in an
immutable `ValidatedCrossClassConstructorCallProof` receipt.

This route handles both the earlier `CounterFactory.create(seed: Int):
Counter = new Counter(seed)` fixture and the `CounterOwnerFactory.wrap`
reference-argument source without consulting class names, method names, or
class positions. The test also renames/reorders the three classes and adds an
unrelated same-named method as a decoy; the receipt still points to the
original canonical Builder method and Owner constructor. A source unit with
no direct matching constructor application receives an empty receipt list.

For the three-class fixture, the same receipt now feeds the bounded TASTy
body validator and emitter. The Factory sidecar serializes its original
method-owned parameter reference and keeps the constructor declaration in the
Owner sidecar. The paired planner validates all three plans, but full
classfile-plus-TASTy artifact publication remains a separate integration
increment.

Negative regressions verify rejection for a mismatched `SourceId`, an omitted
target sibling, a stale typed RHS binding, and an argument mapped to the wrong
typed tree. The existing paired planner also rejects non-formal constructor
arguments before returning a completed batch. These are validation-only
checks; they do not claim forged foreign symbols are constructible through the
real typer.

## Variants and negative controls

The focused Rust regressions rename the package and all three class names, and
separately reverse the `Counter`/`CounterOwner` source declaration order. Both
variants pass the same generic source proof and TASTy validation. The original
source has a positive decoded Factory body assertion; the legacy primitive
constructor test also remains covered by its byte-stability regression.

The focused rejection tests exercise these source-valid or source-typed
controls through the real paired API. The stage and error below are the first
observed rejection; these are current bounded behavior, not general Scala
semantics:

| Variation | First measured rejection |
| --- | --- |
| `new CounterOwner()` | `class_index=2`, `method body typing` / `ApplicationArityMismatch` |
| `new CounterOwner(1)` | `class_index=2`, `method body typing` / `ApplicationArgumentTypeMismatch` |
| Declare `wrap` as returning `Counter` while its body returns `CounterOwner` | `class_index=2`, `JVM/TASTy cross-contract validation` / `BodyType` |
| Make `CounterOwner`'s primary constructor private | `class_index=1`, `TASTy plan validation` / `PrimaryConstructorInput` |
| Make `CounterOwner` abstract | `class_index=2`, `method body typing` / `AbstractClassInstantiation` |
| Add a type parameter to `CounterOwner` | `class_index=1`, `source collection` / generic primary constructors unsupported |
| Change constructor input to `val counter` | `complete JVM unit validation` / `InvalidPrimaryConstructor(AccessorFieldSignature)` |
| Change constructor input to `var counter` | `complete JVM unit validation` / `InvalidPrimaryConstructor(AccessorFieldSignature)` |
| Nest `new Counter(1)` as the Owner argument | cross-class constructor-call identity proof / argument must be the exact caller formal identifier |
| Add a default to `CounterOwner`'s constructor parameter | `class_index=1`, `constructor planning` / `InvalidPrimaryConstructor(SourceShape)` |
| Add a secondary constructor (including an overload) | `class_index=1`, `member planning` / method not owned by declaring class |
| Make the primary constructor contextual (`using`) | `class_index=1`, `constructor planning` / `InvalidPrimaryConstructor(SourceShape)` |
| Rename package and all three classes; reverse Counter/Owner declaration order | Generic constructor-call proof and TASTy body validation succeed through semantic identities |

Some requested controls cannot be authoritatively constructed through this
source-level paired API without violating the one-session identity invariant:

| Control | Why it remains unmeasured here |
| --- | --- |
| Foreign same-name class or a selected constructor owned by another class | The typer resolves the source selection to its canonical constructor. Producing a foreign same-slot `SymbolId` would require fabricating typed identities, explicitly excluded by the issue. |
| Stale/foreign source IDs | The paired API creates and consumes one source ID internally. A stale binding requires substituting caller-forged source/typed indexes, which this audit does not do. |
| External constructor | This fixture uses only source-local classes; an external definition needs a separate classpath/TASTy resolver setup and is not reached by the current constructor-call gate. |
| Missing sidecar, altered TASTY link UUID, or Java classfile-only fallback | The three-class linked artifact batch and its missing/corrupt Factory controls are covered by #1797; Java classfile-only fallback remains outside this source-batch contract. |

No rejection stage is claimed for the unmeasured cases. In particular, the
Scala TASTy consumption is required before claiming interoperability. The
negative tests preserve the current boundaries so successor issues can move
only the intended gate.

## Reproduction

Set `SCALA39_ROOT` to a checkout at the pinned revision. The prebuilt staged
compiler is used because it carries the pinned compiler and standard-library
artifacts; this does not require rebuilding Scala. Start from an empty output
directory:

```sh
export JAVA_HOME=/path/to/jdk-21
export PATH="$JAVA_HOME/bin:$PATH"
export SCALA39_ROOT=/path/to/scala39-source
SCALAC="$SCALA39_ROOT/dist/target/universal/stage/bin/scalac"
rm -rf target/issue1796-oracle
mkdir -p target/issue1796-oracle/classes target/issue1796-oracle/client
"$SCALAC" -release 21 \
  -sourceroot tests/fixtures/jvm_tasty_reference_constructor_calls \
  -d target/issue1796-oracle/classes \
  tests/fixtures/jvm_tasty_reference_constructor_calls/CounterOwnerFactory.scala
find target/issue1796-oracle/classes -type f | sort
"$JAVA_HOME/bin/javap" -v -p -c -classpath target/issue1796-oracle/classes \
  example.Counter example.CounterOwner example.CounterOwnerFactory
```

The strict Rust-produced sidecar reader gate regenerates all three source
sidecars in one frontend session, invokes pinned Scala `-decompile` on the
Factory sidecar, then compiles all three with `-from-tasty`:

```sh
DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1 \
SCALA39_ROOT="$SCALA39_ROOT" JAVA_HOME="$JAVA_HOME" \
cargo test -p dotty --test jvm_tasty_reference_constructor_calls --locked \
  pinned_scala_reads_all_three_rust_reference_constructor_sidecars -- --exact
```

Compile `Consumer.scala` separately with the classes and staged
`scala3-library_3` / `scala-library` jars on its compile classpath, then run
`consumer.CounterOwnerFactoryConsumer` with `java -Xverify:all` and those jars
on the runtime classpath. The exact classfile/TASTy decoders and runtime
assertions are also available as the focused Rust test:

```sh
cargo test -p dotty --test jvm_tasty_reference_constructor_calls --locked
```

## Complete linked batch (#1797)

The paired artifact emitter now completes this same source as one ordered batch
of six owned Rust outputs: `Counter.class` / `.tasty`, `CounterOwner.class` /
`.tasty`, and `CounterOwnerFactory.class` / `.tasty`. Each classfile targets
major version 65 and links to the UUID in its own Scala 3.9 TASTy 28.9.0
sidecar. The batch validator checks the exact source-ordered classes, all six
package-relative paths, every class ABI, and each classfile-to-sidecar pair
before returning the batch.

For the cross-class edge, the source proof retains the canonical
`CounterOwnerFactory.wrap` method and its actual typed RHS, the canonical
`CounterOwner` class and primary constructor, and the exact method-owned
`counter` formal. The contract derives the caller and constructor descriptors
from their source signatures and requires the same single planned-class
parameter and a result descriptor for the allocated Owner. Its bytecode check
requires `new`, `dup`, the correct reference load, the exact Owner constructor
`Methodref`, and `areturn`. The class-specific TASTy plan is emitted from the
same source and typed roots. No Counter allocation occurs in `wrap`.

The focused artifact regression emits the original source in two fresh
semantic sessions and compares every classfile, sidecar, path, and UUID. It
also repeats the structural proof and byte comparison after renaming the
package/classes/method and after reversing the class declaration order. The
contract regression rejects a missing third classfile, a damaged Factory
sidecar, and a Factory allocation/constructor edge changed to `Object`.
These checks establish deterministic Rust output and complete in-memory
validation. The strict runtime proof is described below; Scala consumer
interoperability remains a separate gate.

## Strict Rust classfile runtime proof (#1798)

`tests/jvm_tasty_artifacts.rs::reference_constructor_factory_executes_original_rust_object_graph_on_verified_jvm`
publishes the complete six-artifact batch into a fresh directory, then stages
only the three original Rust-produced `.class` files into the runtime
classpath. It decodes all three classfiles (major version 65), checks the
source ABI, and inspects the Factory allocation and constructor call through
their actual constant-pool references. The verified Factory sequence is
`new CounterOwner; dup; aload_1; invokespecial CounterOwner.<init>(Counter);
areturn`, with `max_stack = 3` and `max_locals = 2`. The Owner constructor
calls `Object.<init>` before storing its exact parameter in the sole
`private final inner: Counter` field; `read` loads that field and calls
`Counter.get()I`. `javap -p -v -c` checks these generated methods and their
descriptors.

The test compiles a separate Java harness against those original Rust classes
and runs it with `java -Xverify:all`. Reflection proves `Owner.inner == counter`
by reference identity. Two calls to `wrap` with the same Counter produce
distinct Owner objects retaining that same Counter. Mutations 7 → 42 → −9 are
observed through both owners; a separate Counter/Owner pair remains
independent. SHA-256 and byte comparisons before and after `javac` and Java
execution ensure the toolchain did not replace Rust output. Repeated source
emission with distinct `SourceId`s still compares byte-for-byte. The test is in
the required JDK job and fails when JDK 21+ tools are absent under
`DOTTY_CLASSFILE_REQUIRE_JDK=1`.

A second required JDK test renames the package, classes, and factory method to
`renamed.Widget`, `WidgetHolder`, and `WidgetBuilder.build`, then moves
`WidgetHolder` before `Widget` in source order. It resolves and stages each
class by the emitted artifact path and checks source identity, reflected alias
identity, distinct holders from repeated calls, and independent mutation under
`java -Xverify:all`. The uninitialized-return negative control replaces the
whole constructor invocation with stack-balanced `pop; pop; nop` instructions,
leaving one uninitialized Owner for `areturn`; the JVM rejects that return.

Negative runtime controls remove either linked class and require the JVM to
report the missing class. Mutated bytecode that skips the constructor call,
omits `dup`, loads an integer instead of the reference parameter, returns an
integer in place of the Owner, or uses the Counter as the Owner receiver must
fail `-Xverify:all` with `VerifyError`. The publisher separately rejects a
corrupted third class's TASTy link before promotion. Source-level negative
tests reject a wrong Factory result type, wrong constructor argument type,
inaccessible or generic constructors, nested allocation, and invalid identity
provenance before a complete batch is returned.

This lane proves execution of the bounded Rust classfile producer. It does not
consume the Rust TASTy sidecars with Scala, and it adds no production API or
public alias accessor. Other constructor shapes, reference widening, arbitrary
constructor expressions, extra arguments, generic/secondary constructors,
and broader object-graph lowering remain unsupported. Scala TASTy
interoperability remains the separate #1799 lane.

## Strict Scala reference-constructor consumer proof (#1799)

`tests/jvm_tasty_reference_constructor_scala_consumer.rs` adds an opt-in gate
under `DOTTY_REQUIRE_SCALA_TASTY_REFERENCE_CONSTRUCTOR_CONSUMER=1`. The test
emits two independent Rust frontend sessions (`SourceId` 17991 and 17992),
checks their three ordered class artifacts retain the same canonical class
symbols and source-member order, and confirms each artifact belongs to its
session. The class symbols are distinct across Counter, CounterOwner, and
CounterOwnerFactory. The independent Scala 3.9 client is compiled only from
`tests/fixtures/jvm_tasty_reference_constructor_calls/ScalaConsumer.scala` against
the six files in a fresh Rust batch and the pinned Scala runtime libraries.
Compiler jars are used only to launch scalac and are checked for shadow copies
of the three example classes. The Scala output directory is separate and
contains only `consumer/Consumer.class` and `consumer/Consumer$.class`.

The consumer calls `CounterOwnerFactory.wrap(counter)` and has no
`CounterOwner.<init>` Methodref. Its compiled references include
`Counter.<init>(I)V`, `CounterOwnerFactory.<init>()V`,
`CounterOwnerFactory.wrap(Lexample/Counter;)Lexample/CounterOwner;`,
`CounterOwner.read()I`, and `Counter.set(I)V`. Both fresh Rust batches execute
unchanged with `java -Xverify:all`; `Consumer.run()` returns `-9` after its
internal checks observe 7, then 42 through the original Counter alias, keep a
second graph at 3, and finally observe -9 without mutating that second graph.
The gate hashes all six artifacts before and after scalac and runtime.

The test also repeats the Scala consumer using independently emitted
`renamed.Widget`, `WidgetHolder`, and `WidgetBuilder.build` artifacts, with
`WidgetHolder` declared before `Widget` in the Rust source. Scala sees the
`WidgetBuilder.build(Lrenamed/Widget;)Lrenamed/WidgetHolder;` signature, does
not construct a holder directly, and returns `-9` on both independent batches.
This verifies the producer behavior follows source identities and package
paths instead of relying on the example class spellings.

The independent lookup controls operate on each class separately. The test
removes `get()I`, `read()I`, or `wrap(Counter):CounterOwner` from that classfile
while retaining its matching TASTy link and sidecar; a Scala probe still
typechecks each member from TASTy. Removing the corresponding sidecar makes
that probe fail because the method is no longer in the classfile. Moving or
corrupting each sidecar and flipping each classfile link UUID are also tested
under `-Werror`. The observed first failures are Scala's missing `.tasty`
diagnostic for absent/moved sidecars, “not a TASTy file” while reading a
corrupt sidecar, and “out of sync with its TASTy file” for a mismatched UUID.
Renaming each `TASTY` attribute and removing its sidecar falls back to the
classfile and fails at the missing method lookup. The suite also confirms a
wrong Owner constructor formal and wrong Factory result are rejected by Scala
typechecking, and a wrong-package Owner reference does not resolve; deleting the Owner classfile fails at JVM linkage with
`NoClassDefFoundError`, and deleting its `(Counter)V` constructor fails with
`NoSuchMethodError` at `Factory.wrap`.

Pinned Scala 3.9.0 decompilation of the Rust-produced sidecars confirms the
Factory method has the distinct CounterOwner result and source body
`new example.CounterOwner(counter)`, the Owner has its original
`counter: Counter` constructor parameter and `private val inner: Counter =
counter`, and Counter retains `get` and `set`. An isolated `-from-tasty` run
reconstructs all three semantic declarations into a separate output directory;
those Scala classfiles are not used by the positive runtime checks.

The observed strict run used Scala source revision
`777528f19a58e794c9954a42f433373472ec57f8`, the SHA-pinned 3.9.0 compiler
dependencies in `tests/fixtures/tasty_pickler/provenance.json`, and Amazon
Corretto JDK 25.0.4 (release 21 or newer). For the `SourceId(17991)` producer
batch, the six files staged at `example/{Counter,CounterOwner,
CounterOwnerFactory}.{class,tasty}` had these SHA-256 hashes and linked TASTy
UUIDs:

| Class | Classfile SHA-256 | TASTy SHA-256 | TASTy UUID |
| --- | --- | --- | --- |
| `example.Counter` | `b38143cee8571a6602865ff6062eca92dd771582f539b156100ffc9647af94ae` | `a0e78329d60aba318de825eaddb7668d34838197158653c1b7384188143099c7` | `00da82c5ca4ebdac0000000000008193` |
| `example.CounterOwner` | `d914c2dfc5b8d22079be304fc7c463be171394a528b71603de43342a79db91b2` | `9bcfbbb9d12f71a50122c6b2af960fe76ffd7f629a57738f61f4d384821f7f13` | `004ebc559d6864ef0000000000008195` |
| `example.CounterOwnerFactory` | `2891207c0907224a87da6ea95d0610b022572e5bb055faefa5349fbdfd0d13e7` | `fa91a9f9410e5fc876fd27d3f4e324371b9237b36df21f8297bb785fb5a792d2` | `006568821180adc10000000000008194` |

Each classfile is major version 65 and each TASTy header is 28.9.0. The second
session produced byte-identical classfile and TASTy bytes at the same ordered
paths. Both batches' dependency hashes matched before and after Scala
compilation and JVM execution. If `DOTTY_REQUIRE_SCALA_TASTY_REFERENCE_CONSUMER`
is set, missing or mismatched Scala 3.9.0 revision/dependencies or JDK 21+
tools fail the test rather than skip it.

## Final three-class release gate (#1800)

The final strict runs used Rust `1.98.1`, Scala compiler 3.9.0 from source
revision `777528f19a58e794c9954a42f433373472ec57f8`, and Eclipse Temurin
`21.0.8+9` (`java`, `javac`, and `javap` all report 21.0.8). The pinned Maven
artifacts are read from
`target-fs2/coursier/https/repo1.maven.org/maven2`; the oracle checks their
SHA-256 values against `tests/fixtures/tasty_pickler/provenance.json`. The
standalone Scala client source SHA-256 is
`86f6f693cbe3fd2e7cf7f3a51f7cf36ce7eb310981fc0b435e2082514ea349c9`.

`DOTTY_REQUIRE_SCALA_TASTY_REFERENCE_CONSTRUCTOR_CONSUMER=1 cargo test -p
dotty --test jvm_tasty_reference_constructor_scala_consumer --locked --
--nocapture` passed. Its compile classpath puts the six-file Rust output first,
followed only by `scala3-library_3-3.9.0.jar` and
`scala-library-3.9.0.jar`. The Scala compiler launcher uses the eight
SHA-pinned jars listed in `tests/fixtures/tasty_pickler/provenance.json`:
those two runtime libraries, `scala3-compiler_3-3.9.0.jar`,
`tasty-core_3-3.9.0.jar`, `scala3-interfaces-3.9.0.jar`,
`scala-asm-9.9.0-scala-1.jar`, `compiler-interface-1.12.0.jar`, and
`util-interface-1.11.5.jar`. Runtime classpath order is the unchanged Rust
six-file root, the separate consumer output, then the two Scala runtime jars.
Three separate probes resolved `Counter.get`, `CounterOwner.read`, and
`CounterOwnerFactory.wrap` with each member removed from its classfile but
present in TASTy; their classfile-only controls failed. Missing, misplaced,
corrupt, and UUID-mismatched sidecars failed with pinned Scala diagnostics.
The client compiled against Rust-only output, and both fresh producer sessions
ran with `java -Xverify:all`, returning `-9`. Runtime used the unchanged
original Rust classes. The separate `-from-tasty` output was used only for
semantic reconstruction and never replaced runtime dependencies.

The strict run recorded these six Rust artifact digests and linked TASTy UUIDs
for the first fresh frontend session:

| Class | Classfile SHA-256 | TASTy SHA-256 | TASTy UUID |
| --- | --- | --- | --- |
| `example.Counter` | `b38143cee8571a6602865ff6062eca92dd771582f539b156100ffc9647af94ae` | `a0e78329d60aba318de825eaddb7668d34838197158653c1b7384188143099c7` | `00da82c5ca4ebdac0000000000008193` |
| `example.CounterOwner` | `d914c2dfc5b8d22079be304fc7c463be171394a528b71603de43342a79db91b2` | `9bcfbbb9d12f71a50122c6b2af960fe76ffd7f629a57738f61f4d384821f7f13` | `004ebc559d6864ef0000000000008195` |
| `example.CounterOwnerFactory` | `2891207c0907224a87da6ea95d0610b022572e5bb055faefa5349fbdfd0d13e7` | `fa91a9f9410e5fc876fd27d3f4e324371b9237b36df21f8297bb785fb5a792d2` | `006568821180adc10000000000008194` |

The second independent session emitted byte-identical files at the same six
paths. The strict Rust JDK command passed with
`DOTTY_CLASSFILE_REQUIRE_JDK=1` for the classfile encoder, JVM backend, source
compilation, stateful class contract, and paired artifact tests. The direct
Factory harness used `java -Xverify:all` on the original Rust classes,
verified that `Owner.inner` is the same input object, checked distinct Owners
for repeated calls, observed `7 → 42 → -9` after mutating the original
Counter, and kept a second `Counter(3)` graph independent. The harness and
Scala consumer both verify that Rust classfile hashes remain unchanged.

The command used was:

```sh
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test \
  -p dotty-classfile --test classfile_jvm_encoder \
  -p dotty-jvm --lib \
  -p dotty --test jvm_source_compilation \
  --test jvm_tasty_artifacts \
  --test jvm_stateful_class_contract \
  --locked -- --nocapture
```

The other strict Scala 3.9 consumers passed with their required flags:
`DOTTY_REQUIRE_SCALA_TASTY_CONSTRUCTOR_INJECTION_CONSUMER`,
`DOTTY_REQUIRE_SCALA_TASTY_OBJECT_COMPOSITION_CONSUMER`,
`DOTTY_REQUIRE_SCALA_TASTY_FACTORY_CONSUMER`,
`DOTTY_REQUIRE_SCALA_TASTY_CROSS_CLASS_CONSUMER`,
`DOTTY_REQUIRE_SCALA_TASTY_MULTICLASS_CONSUMER`, and
`DOTTY_REQUIRE_SCALA_TASTY_CONSUMER`. The pinned source pickler oracle passed
with `DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1`. Rust checks passed for workspace
formatting, the frontend dependency-boundary guard, Clippy with warnings
denied, rustdoc with warnings denied, and all workspace target tests. Scratch
compiler files used the worktree's `target/tmp` because the shared `/tmp`
filesystem was full; no unrelated cache files were removed.

The six-file publisher regression rejects a missing Factory sidecar or
classfile, forged per-class source provenance, corrupt Owner or Factory TASTy,
and a mismatched Factory link UUID before publication. Injected handled errors
after each of the six staged files, before promotion, and after promotion
leave no final output tree and clean staging. A pre-existing destination and
sentinel remain intact. This covers handled failures and does not claim
crash-atomic filesystem durability. The supported producer shape remains
bounded: nested constructor expressions, fields, null, factory-result
arguments, secondary or generic constructors, multiple reference formals,
and other unproven constructor forms fail before a complete batch is returned.
