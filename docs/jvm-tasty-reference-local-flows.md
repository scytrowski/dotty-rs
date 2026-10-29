# Reference-local constructor flow: Scala 3.9 and Rust boundary audit

Issue [#1849](https://github.com/scytrowski/dotty-rs/issues/1849) audits one
producer whose immutable `Counter` local is initialized from a same-instance
factory call, then passed to `CounterOwner` in the same method block. This is an
audit of existing boundaries. It does not enable or claim paired JVM/TASTy
support for the local-block expression.

## Frozen producer and oracle

The producer is the single source file
[`CounterOwnerFactory.scala`](../tests/fixtures/jvm_tasty_reference_local_constructor_flows/CounterOwnerFactory.scala).
It declares exactly `Counter`, `CounterOwner`, and `CounterOwnerFactory`, in
one package. The pinned Scala source checkout was at
`777528f19a58e794c9954a42f433373472ec57f8`; its built compiler reports
`3.9.0-bin-SNAPSHOT-git-777528f`. The producer was compiled once with
Temurin `21.0.8+9` and `-release 21` into an empty directory. The output
contains exactly three classfiles and their three TASTy sidecars. Every
classfile is major version 65, every sidecar is TASTy 28.9.0, and the
classfile's `TASTY` UUID matches its paired sidecar header. The per-file hashes,
UUIDs, compiler/runtime hashes, source hashes, and method observations are
recorded in [`oracle.json`](../tests/fixtures/jvm_tasty_reference_local_constructor_flows/oracle.json).

The compiler's `javap -v -p -c` output for
`CounterOwnerFactory.create(I)Lexample/CounterOwner;` reports `stack=3`,
`locals=3`, and these instructions:

```text
0:  aload_0
1:  iload_1
2:  invokevirtual  CounterOwnerFactory.makeCounter:(I)Lexample/Counter;
5:  astore_2
6:  new            example/CounterOwner
9:  dup
10: aload_2
11: invokespecial  CounterOwner.<init>:(Lexample/Counter;)V
14: areturn
```

The slots are `this=0`, `seed=1`, and `counter=2`. The factory call completes
and stores its initialized `Counter` before the outer `CounterOwner` allocation
starts. The Owner constructor receives the value loaded from slot 2.

The separate Scala-authored
[`Consumer.scala`](../tests/fixtures/jvm_tasty_reference_local_constructor_flows/Consumer.scala)
was compiled only against those three Scala-produced classes and the pinned
Scala libraries. It ran with `java -Xverify:all`, checked `create(7)`,
`create(-9)`, and `create(3)`, then checked two separately returned Counter
references and independent mutable state. It printed
`scala39-reference-local-constructor-flow=ok`. This verifies Scala output and
is not Rust-to-Scala interoperability.

## TASTy wire tree and cross-representation identities

`CounterOwnerFactory.tasty` decodes as TASTy 28.9.0. For `create`, the visible
addresses include:

| Address | Tag | Node |
| ---: | ---: | --- |
| 64 | 130 | `DEFDEF create` |
| 67 | 134 | `PARAM seed` |
| 80 | 140 | `BLOCK` |
| 82 | 136 | result `APPLY` for the Owner constructor |
| 84 | 176 | selected Owner constructor (`SELECTin`) |
| 96 | 129 | immutable `VALDEF counter: Counter` |
| 103 | 136 | local initializer `APPLY` |
| 105 | 176 | selected `makeCounter` (`SELECTin`) |

The block payload stores its expression before its stats. Therefore the
outer-constructor `APPLY` appears on the wire before the local `VALDEF`, even
though JVM execution evaluates the initializer first. The constructor's local
argument is a `TERMREFdirect` whose address points to the `counter` `VALDEF` at
96. The initializer remains an application of the signed `makeCounter(Int):
Counter` member. The decoder regression checks the local reference target,
same-block parent, method signature, all three TASTY UUID links, and all three
classfile major versions.

The Rust source observation uses one Lexer → Parser → Namer → `SourceTyper`
session for the unchanged fixture and `SourceId(1849)`. The IDs below are the
observed allocation in that test only; they are not stable public identifiers.

| Meaning | Source identity | Typed identity / type |
| --- | --- | --- |
| Factory class | `SymbolId(42)` | same canonical source class |
| `create` method | `SymbolId(57)` | body type is `CounterOwner` |
| `makeCounter` selection | source `Ident` in `TreeId(49)` | canonical `SymbolId(56)`; result type `Counter` |
| Local declaration | `TreeId(50)` | `TreeId(16)`, `SymbolId(60)`, owned by `create`, type `Counter` |
| Local initializer Apply | `TreeId(49)` | `TreeId(14)`; source-to-typed index retains this exact edge |
| Block method body | `TreeId(56)` | `TreeId(22)` |
| Final local use | `TreeId(54)` | `TreeId(20)`, `TermRef(SymbolId(60))` |

The frontend keeps the original block, `ValDef`, initializer call, final
constructor application, and local `Ident`. `Typer::typed_local_binding_at`
returns the local symbol and its exact typed declaration, keyed by the original
source `TreeId`; the typed reference in the constructor argument points to that
same `SymbolId(60)`. The helper selection retains canonical method
`SymbolId(56)`. No local symbol or typed tree is reconstructed from spelling.

| Stage | Observed result for the unchanged local-block source |
| --- | --- |
| Lexer, Parser, Namer, Typer | Succeed in the single source session; source/typed local and method links are retained. |
| Cross-class constructor proof scan | Completes without error. The scan retains the helper's existing proof but no `create` proof, because `create` returns a `Block` rather than the bounded direct constructor-call shape. |
| Class-state TASTy plan | Signature/member plan validates in the paired batch before the body contract check. |
| TASTy method-body validation | First rejection: `BodyType { tree: TreeId(24), ty: TypeId(46) }`. This is surfaced as `JVM/TASTy cross-contract validation` / `TastyPlan(BodyType)`. Those IDs are run-local. |
| TASTy byte emitter and paired artifact publisher | Not reached. A complete validated method-body plan is required first; no Rust block TASTy artifact was manufactured. |
| Separate JVM-only source backend | Succeeds for the same source shape, emits all three classes, and keeps `makeCounter:(I)Lexample/Counter;` and `CounterOwner.<init>:(Lexample/Counter;)V`. The method uses local slot 2. |
| JDK 21 verification/execution of Rust JVM classes | Succeeds with `-Xverify:all`; the Java harness checks all three seed values, distinct Counter references, and state isolation. This is separate from paired TASTy output. |

The exact paired failure and local identity assertions are in
`reference_local_constructor_flow_preserves_one_session_identity_before_paired_rejection`
in [`jvm_tasty_factory_result_arguments.rs`](../tests/jvm_tasty_factory_result_arguments.rs).
The independent backend check is
`reference_local_constructor_flow_emits_and_executes_on_the_jvm_only_path`
in [`jvm_source_compilation.rs`](../tests/jvm_source_compilation.rs).

## Controls and bounded conclusions

| Control | Evidence or disposition |
| --- | --- |
| Rename package, classes, and factory method while retaining the local block | Measured through the real paired planner. Frontend planning reaches the same `TastyPlan(BodyType)` gate. |
| Reorder the three class declarations while retaining the local block | Measured through the real paired planner. The first failure remains `TastyPlan(BodyType)`. |
| Direct formal, nested `new`, and direct factory-result constructor arguments | Existing linked regressions cover these distinct accepted forms, including Scala 3.9 readers and JDK execution. They do not establish block-local paired TASTy support. See [factory-result arguments](jvm-tasty-factory-result-arguments.md) and [nested constructor calls](jvm-tasty-nested-constructor-calls.md). |
| Helper shadowing, private/overloaded/generic/defaulted helper, wrong arity/result, external helper, and conditional multi-call forms | Measured by the existing first-gate controls in `jvm_tasty_factory_result_arguments.rs`; these cases fail at their asserted source or member gate. |
| Wrong source/typed call-edge binding, foreign same-named helper, wrong owner/result/receiver, missing member, malformed JVM invocation, and write failures | Existing focused proof, batch-contract, and publisher regressions check these direct-call paths. They are not local-block receipt tests. |
| Missing local initializer, mutable `var`, local declared after use, wrong local type, local shadowing, duplicated helper evaluation, nested local blocks, mixed `If`/loop blocks, foreign/cross-method local, and corrupted local source/typed bindings | Deferred or unmeasured. The valid immutable local already reaches the TASTy body-type rejection before body serialization or artifact publication, so these controls cannot establish the local-body contract without manufacturing a downstream plan. This audit records no guessed diagnostics for them. |
| Unrelated/external classes and wider Scala ABI forms | Outside this single-source audit. No cross-unit or external-reference support is claimed. |

## Reproduction commands

Set `SCALA39_ROOT` to a clean checkout at the pinned revision and use Temurin
JDK 21. The repository CI workflow builds this checkout with
`sbt "dist/universal:stage"`; the staged `scalac` is then used as below. Keep
all outputs in the issue worktree.

```sh
test "$(git -C "$SCALA39_ROOT" rev-parse HEAD)" = \
  777528f19a58e794c9954a42f433373472ec57f8
test "$("$SCALA39_ROOT/dist/target/universal/stage/bin/scalac" -version 2>&1 | head -1)" = \
  "Scala compiler version 3.9.0-bin-SNAPSHOT-git-777528f -- Copyright 2002-2026, LAMP/EPFL"
mkdir -p target/oracle/producer target/oracle/consumer
test -z "$(find target/oracle/producer -mindepth 1 -print -quit)"
"$SCALA39_ROOT/dist/target/universal/stage/bin/scalac" -release 21 \
  -d target/oracle/producer \
  tests/fixtures/jvm_tasty_reference_local_constructor_flows/CounterOwnerFactory.scala
```

Compile the Scala client to a different output directory, with only the
producer artifacts and pinned Scala libraries on its classpath, then verify
and run it:

```sh
SCALA_LIB="$SCALA39_ROOT/dist/target/universal/stage/maven2/org/scala-lang/scala3-library_3/3.9.0-bin-SNAPSHOT/scala3-library_3-3.9.0-bin-SNAPSHOT.jar:$SCALA39_ROOT/dist/target/universal/stage/maven2/org/scala-lang/scala-library/3.9.0-bin-SNAPSHOT/scala-library-3.9.0-bin-SNAPSHOT.jar"
"$SCALA39_ROOT/dist/target/universal/stage/bin/scalac" -release 21 \
  -classpath "target/oracle/producer:$SCALA_LIB" -d target/oracle/consumer \
  tests/fixtures/jvm_tasty_reference_local_constructor_flows/Consumer.scala
java -Xverify:all -classpath "target/oracle/consumer:target/oracle/producer:$SCALA_LIB" \
  example.FactoryConsumer
javap -v -p -c -classpath target/oracle/producer example.CounterOwnerFactory
```

Run the focused Rust observations and mandatory JDK gate with JDK 21 first on
`PATH`:

```sh
cargo test -p dotty --test jvm_tasty_factory_result_arguments \
  reference_local_constructor_flow --locked -- --nocapture
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test -p dotty --test jvm_source_compilation \
  reference_local_constructor_flow_emits_and_executes_on_the_jvm_only_path \
  --locked -- --nocapture
```

The inherited strict pinned Scala consumer and JDK workflow is
`.github/workflows/jvm-tasty-factory-scala-consumer.yml`; its path filter
includes this fixture, documentation, and tests. The full hosted Scala
compatibility corpus remains out of scope.
