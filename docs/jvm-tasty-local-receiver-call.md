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

## Integrated release-gate evidence for issue #2170

This appendix closes the final tests/docs gate against integration base
`89f81940fc538a7b24323b82b9415cdb6a7c5404`, which contains reviewed merges of
#2164 through #2169. Those prerequisite review and merge receipts remain
historical provenance; the checks in this table are the candidate's own
integrated-head evidence. The tested Scala input is the unchanged
`LocalReceiverCall.scala` above (SHA-256
`d00efdafda78682532599fbd9017a2037d1579241fa388c744f8f58d13b42eb8`), and the
Java fixture client is `RuntimeProbe.java` (SHA-256
`d02400bbd69140197a8948c3074cd61d38a077e9b2283102a6f00573c8ecfc2d`). The
strict consumer job pins Scala 3.9.0 source revision
`777528f19a58e794c9954a42f433373472ec57f8`, its compiler artifact SHA-256
`3e2c09a5ca6cef17a55f2f66e123815f9c4ea061fce20d0e9d2371989405a61a`, Rust
1.98.1 and Temurin JDK 21.0.13+9.0.LTS.

| Acceptance contract | Integrated regression/evidence | Result |
| --- | --- | --- |
| One real source/typed session preserves two immutable local initializers, `first.set(secondSeed)` and final `Pair(first, second)` order | `local_receiver_call_audit_keeps_the_real_frontend_identities_and_first_failure`; canonical receiver symbol is `first`, selected method is `Counter.set`, and call source span is `[522, 543)` | Passed on the candidate worktree |
| Authentic TASTy and JVM output retain the call and both original locals | `local_receiver_unit_call_emits_one_linked_six_artifact_batch`; TASTy `BLOCK` has `VALDEF, VALDEF, APPLY` stats and final `APPLY`; JVM `create(II)Lexample/Pair;` has one `Counter.set(I)V` invocation between local initialization and Pair construction, classfile major 65 and TASTy 28.9 links | Passed on the candidate worktree |
| Fresh sessions reproduce all six artifacts for original and reversed declarations | `local_counter_set_runs_on_unchanged_rust_batch_under_strict_jdk21`; each source variant is emitted twice in independent sessions and all class/TASTy bytes compare equal | Passed on the candidate worktree |
| The actual Rust output passes Java verification/runtime, identity, mutation-isolation and reversed-order controls | Same runtime test, with `DOTTY_CLASSFILE_REQUIRE_JDK=1`, Temurin 21.0.13; both primary and reversed variants report verifier/runtime success | Passed locally |
| Existing no-call artifact bytes and bounded negative first failures remain intact | The six-artifact test compares `Counter` and `Pair` bytes with the inherited no-call batch; paired-path controls retain frontend, identity-proof and cross-contract rejection stages; runtime test retains verifier/linkage/publisher tamper controls | Passed in focused local tests |
| Official Scala reader/consumer and all current strict shards execute fail-closed | `jvm_tasty_two_local_counter_set_scala_consumer`, strict inventory and aggregate job | Scala consumer passed locally; hosted four-shard workflow is required on the PR head. The current manifest validates 25 unique targets across four shards |

The focused local run used the exact Java gate with `DOTTY_CLASSFILE_REQUIRE_JDK=1`
and the source-built Scala consumer with `DOTTY_REQUIRE_SCALA_TASTY_TWO_LOCAL_COUNTER_SET_CONSUMER=1`
and `DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1`. The compiler source checkout was
verified at `777528f19a58e794c9954a42f433373472ec57f8`; its staged compiler
reported `3.9.0-bin-SNAPSHOT-git-777528f`, SHA-256
`8972d966eae9b43346b89e4f3cf608ce9ff90a20764dca8f8d5c13d98587c2e6`. The
runtime consumer used Temurin 21.0.13 (`java` SHA-256
`9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`), the
Scala producer source SHA above and generated client SHA-256
`5d9d77abad4111fd49e135b9bdd044e14a177d35440852de89440d8c6d50e61e`. The
strict inventory validator passed for all 25 registered targets and four
shards. The final PR description links workflow run and job IDs for the final
tested head; no historical prerequisite check is substituted for those
results.

For the runtime regression's original source session (`SourceId(21681)`),
SHA-256 hashes and linked TASTy UUIDs are emitted by the test itself. The
observed primary batch was:

| Artifact | SHA-256 | Linked TASTy UUID |
| --- | --- | --- |
| `example/Counter.class` | `00d8c580a7de082187963ef68da3ed2c0fc434c8ffd7eb74fc3b4fc284d15f4b` | `00c921ab9a680cac0000000000008193` |
| `example/Counter.tasty` | `a810aae114aa178ae6d46ea0186cad24b2accbeb1f60a261ff86bc0ca6bddebc` | `00c921ab9a680cac0000000000008193` |
| `example/Pair.class` | `7c013db083dec06742cb25c9d5e8d624a056f6f7d76d9b19ab1691b99db9cb37` | `00422de52e20a3f00000000000008198` |
| `example/Pair.tasty` | `0f4ba6c53d4b71b058db4a606753ad2afdbc854a05f2a57d356e51f3066a8652` | `00422de52e20a3f00000000000008198` |
| `example/PairFactory.class` | `b9cb66f944343b13bd166d2ac85025d2cd203608830658e6fe3a410100570286` | `008977b0069599cb000000000000819d` |
| `example/PairFactory.tasty` | `18c9f230a0c519810e13e50dcae8adff351c20348a30aef903078df231e8559a` | `008977b0069599cb000000000000819d` |

These hashes are measurements from the final gate test, not checked-in golden
outputs. The reversed declaration variant keeps the `Counter` and `Pair`
artifacts identical and produces its own deterministic `PairFactory` class and
TASTy hashes, printed by the same test. Unsupported forms remain those listed
in the bounded controls above; this release gate adds no producer behavior.
