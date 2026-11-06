# Scala oracle and paired-path audit: local receiver `Unit` call

This is an evidence-only audit for [issue #2164](https://github.com/scytrowski/dotty-rs/issues/2164). The target source is preserved verbatim in [`LocalReceiverCall.scala`](../tests/fixtures/jvm_tasty_local_receiver_call/LocalReceiverCall.scala), SHA-256 `d00efdafda78682532599fbd9017a2037d1579241fa388c744f8f58d13b42eb8`. It adds exactly `first.set(secondSeed)` between the original two immutable local declarations and `new Pair(first, second)`. No production compiler code or accepted source boundary changed.

## Oracle provenance and reproduction

The baseline is `3fba164caaa5efac5994cee6d7c1e865dd1d767a`. Scala is 3.9.0 and the clean source checkout was verified at revision `777528f19a58e794c9954a42f433373472ec57f8`. The executable compiler was the official published `org.scala-lang:scala3-compiler_3:3.9.0` artifact (SHA-256 `3e2c09a5ca6cef17a55f2f66e123815f9c4ea061fce20d0e9d2371989405a61a`) with the published Scala 3.9.0 runtime jars. A local source build was attempted from the pinned checkout, but the sbt launcher could not download sbt 1.12.1 under the host's restricted network policy; the checked-in oracle therefore makes no claim that the compiler jar was built locally from that checkout. As a parity check, compiling the existing no-call source with this official compiler reproduced the existing Scala fixture's three classfile hashes exactly. The artifact, runtime and tool hashes are in [`oracle.json`](../tests/fixtures/jvm_tasty_local_receiver_call/oracle.json).

The oracle command used Java 21.0.8+9 (Temurin), `-release 21`, and an empty output directory. With the official Scala jars available locally:

```sh
SCALA39_MAVEN_ROOT=/path/to/scala-3.9.0-maven-cache
SCALA39_CP="$(find "$SCALA39_MAVEN_ROOT" -name '*.jar' -type f | sort | paste -sd:)"
SCALA39_LIBS="$SCALA39_MAVEN_ROOT/org/scala-lang/scala3-library_3/3.9.0/scala3-library_3-3.9.0.jar:$SCALA39_MAVEN_ROOT/org/scala-lang/scala-library/3.9.0/scala-library-3.9.0.jar"
OUT=/path/to/empty-output
"$JAVA_HOME/bin/java" -cp "$SCALA39_CP" dotty.tools.dotc.Main \
  -classpath "$SCALA39_LIBS" -Werror -release 21 -d "$OUT" \
  tests/fixtures/jvm_tasty_local_receiver_call/LocalReceiverCall.scala
"$JAVA_HOME/bin/java" -cp "$SCALA39_CP" dotty.tools.MainGenericCompiler \
  -color:never -print-tasty "$OUT/example/PairFactory.tasty"
"$JAVA_HOME/bin/javap" -v -p -c -classpath "$OUT" example.PairFactory
"$JAVA_HOME/bin/javac" --release 21 -cp "$OUT" -d "$OUT" \
  tests/fixtures/jvm_tasty_local_receiver_call/RuntimeProbe.java
"$JAVA_HOME/bin/java" -Xverify:all -cp "$OUT" example.RuntimeProbe
```

The six independently compiled `.class` and `.tasty` files are checked in under [`scala39/primary/example`](../tests/fixtures/jvm_tasty_local_receiver_call/scala39/primary/example). The actual `javap -v -p -c` and `-print-tasty` snapshots are under [`oracle/`](../tests/fixtures/jvm_tasty_local_receiver_call/oracle/). Scala accepted the target. All classfiles are major version 65, each classfile TASTY UUID link matches its sidecar, and the sidecars parse as TASTy 28.9.0.

## Scala observations

`PairFactory.create` has descriptor `(II)Lexample/Pair;`, `max_stack=4`, and `max_locals=5`. `javap` reports this instruction order:

```text
new Counter; dup; iload_1; invokespecial Counter.<init>(I)V; astore_3
new Counter; dup; iload_2; invokespecial Counter.<init>(I)V; astore 4
aload_3; iload_2; invokevirtual Counter.set:(I)V
new Pair; dup; aload_3; aload 4; invokespecial Pair.<init>(Counter,Counter)V; areturn
```

The TASTy reader observes a `BLOCK` at address 57. Its source-ordered stats are `VALDEF` at 75 (`first`), `VALDEF` at 98 (`second`), and `APPLY` at 119 for the call. The result expression is the Pair-constructor `APPLY` at 59. The call selects the actual signed `Counter.set(Int): Unit` member through `SELECTin`; these are observations from the emitted `-print-tasty` tree, not assumed tags. The Java 21 harness calls `create(7, 3)` under `-Xverify:all` and prints `3/3`, confirming that the receiver mutation executes before Pair construction.

## Rust paired-path result

The focused audit test sends the same source through the real lexer, parser, namer, and source typer, then through `with_paired_jvm_tasty_batch_plan`. The frontend successfully types the method body. One real source-typing session (`SourceId(21640)`) records two original formal symbols, two distinct method-owned local symbols, the original source stat order, and a typed call stat whose source span is `[522, 543)`. The typed receiver's `TermRef` resolves to the canonical symbol returned for the first local declaration; the selected method `TermRef` matches the canonical `Counter.set` declaration. Each constructor argument resolves to its corresponding canonical formal symbol. The test compares these source/typed bindings directly; it does not infer identity from names or fixed arena offsets.

The paired planner's measured first failure is:

```text
BatchUnit {
  phase: "cross-class constructor-call identity proof",
  reason: "SourceShape(\"factory body must be a direct Apply or a one- or two-local Block\")"
}
```

This is a paired source-adapter identity-proof rejection after frontend typing. TASTy pickler validation/encoding, JVM/TASTy contract validation, paired emission and publisher behavior are not reached; no later failure is inferred. The next issue should own the bounded cross-class constructor-call identity proof surface that currently recognizes only direct or one-/two-local factory bodies. This audit does not propose widening that proof or altering downstream producers.

## Bounded controls

Each control was compiled with Scala 3.9.0 and separately sent through the real Rust paired planner. The Rust result below is the observed first gate for that exact variant.

| Control | Scala 3.9.0 | Rust paired path |
| --- | --- | --- |
| Existing two-local source with no call | Accepted | Paired plan succeeds; existing two-local pickle regression passes |
| `first.set()` | Rejected, `[E171]` missing argument | Method-body typing: `ApplicationArityMismatch` |
| `first.set(true)` | Rejected, `[E007]` Boolean where `Int` is required | Method-body typing: `ApplicationArgumentTypeMismatch` |
| `second.set(secondSeed)` | Accepted | Cross-class constructor-call identity proof: same `SourceShape` rejection |
| Reverse the two local declarations, keep the call and Pair arguments | Accepted | Cross-class constructor-call identity proof: same `SourceShape` rejection |
| Add `second.set(firstSeed)` as a second call | Accepted | Cross-class constructor-call identity proof: same `SourceShape` rejection |

The matrix keeps wrong-call diagnostics at their measured frontend stages and confirms that ordinary Scala acceptance of the other controls does not imply paired Rust support. The complete fixture, artifact snapshots and paired identity assertions are focused audit coverage; no general statement framework or production acceptance change was added.
