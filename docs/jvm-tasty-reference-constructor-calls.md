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
| Factory class-state TASTy `pickle()` | Fails with `UnsupportedTypeReference` for the built-in `Int` type at the selected primary-constructor-call validation. |
| Paired JVM/TASTy batch cross-contract validation | First batch failure is class index 2, phase `JVM/TASTy cross-contract validation`, caused by the same existing Int-only constructor formal/argument gate. |
| Artifact publication | Not reached: paired producer validation fails before an artifact batch is returned. No emitted artifact is claimed. |

The direct class-state `validate()` call accepts the actual signature and
source/typed plan, including the distinct `Counter` parameter and `CounterOwner`
result. The first lowering failure arises while serializing the real method
body: `validate_cross_class_constructor_application` delegates to
`validate_planned_primary_constructor`, whose formal/argument path still
requires the constructor formal to be `Int`. The paired batch wraps that same
failure in its `JVM/TASTy cross-contract validation` stage. This pins the
specific constructor-call blocker, rather than treating the already accepted
method signature as the failure. The issue's implementation steps must
replace the Int-only check with source-identity-based reference validation. No
Typer or production code was changed for this audit.

The JVM-only backend's existing `new Holder(counter)` support is outside this
Rust producer failure. The paired plan does not produce a JVM-only artifact
before the TASTy cross-contract failure, so this audit does not claim the
three-class JVM artifact batch succeeds.

## Variants and negative controls

The focused Rust regression renames the package and all three class names, and
separately reverses the `Counter`/`CounterOwner` source declaration order. Both
still reach the same measured TASTy cross-contract `UnsupportedTypeReference`
failure. The test deliberately records the limitation, not a proposed
implementation based on those names or positions.

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
| Nest `new Counter(1)` as the Owner argument | `class_index=2`, `JVM/TASTy cross-contract validation` / same Int-only constructor gate |
| Add a default to `CounterOwner`'s constructor parameter | `class_index=1`, `constructor planning` / `InvalidPrimaryConstructor(SourceShape)` |
| Add a secondary constructor (including an overload) | `class_index=1`, `member planning` / method not owned by declaring class |
| Make the primary constructor contextual (`using`) | `class_index=1`, `constructor planning` / `InvalidPrimaryConstructor(SourceShape)` |
| Rename package and all three classes; reverse Counter/Owner declaration order | `class_index=2`, `JVM/TASTy cross-contract validation` / same Int-only constructor gate |

Some requested controls cannot be authoritatively constructed through this
source-level paired API without violating the one-session identity invariant:

| Control | Why it remains unmeasured here |
| --- | --- |
| Foreign same-name class or a selected constructor owned by another class | The typer resolves the source selection to its canonical constructor. Producing a foreign same-slot `SymbolId` would require fabricating typed identities, explicitly excluded by the issue. |
| Stale/foreign source IDs | The paired API creates and consumes one source ID internally. A stale binding requires substituting caller-forged source/typed indexes, which this audit does not do. |
| External constructor | This fixture uses only source-local classes; an external definition needs a separate classpath/TASTy resolver setup and is not reached by the current constructor-call gate. |
| Missing sidecar, altered TASTY link UUID, or Java classfile-only fallback | The paired producer returns no artifact batch after its measured TASTy failure, so there is no Rust-produced classfile set to exercise those artifact-integration controls against. |

No rejection stage is claimed for the unmeasured cases. In particular, the
positive Scala oracle is not evidence that a Rust artifact batch passes those
controls. The negative tests preserve the current boundaries so successor
issues can move only the intended gate.

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
rm -rf target/issue1793-oracle
mkdir -p target/issue1793-oracle/classes target/issue1793-oracle/client
"$SCALAC" -release 21 \
  -sourceroot tests/fixtures/jvm_tasty_reference_constructor_calls \
  -d target/issue1793-oracle/classes \
  tests/fixtures/jvm_tasty_reference_constructor_calls/CounterOwnerFactory.scala
find target/issue1793-oracle/classes -type f | sort
"$JAVA_HOME/bin/javap" -v -p -c -classpath target/issue1793-oracle/classes \
  example.Counter example.CounterOwner example.CounterOwnerFactory
```

Compile `Consumer.scala` separately with the classes and staged
`scala3-library_3` / `scala-library` jars on its compile classpath, then run
`consumer.CounterOwnerFactoryConsumer` with `java -Xverify:all` and those jars
on the runtime classpath. The exact classfile/TASTy decoders and runtime
assertions are also available as the focused Rust test:

```sh
cargo test -p dotty --test jvm_tasty_reference_constructor_calls --locked
```
