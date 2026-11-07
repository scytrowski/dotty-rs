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

### Frozen base and prerequisite receipts

The integrated base is `89f81940fc538a7b24323b82b9415cdb6a7c5404`. Each prerequisite below has a published `READY_FOR_MERGE` review on the same head that was merged. These are stable prerequisite receipts; they do not certify this final gate's CI.

| Issue | PR | Reviewed head | READY_FOR_MERGE review | Merge commit (UTC) |
| --- | --- | --- | --- | --- |
| #2164 | [#2171](https://github.com/scytrowski/dotty-rs/pull/2171) | [`7eb7eda2b6b6eafda2b0989be907527f7035b38d`](https://github.com/scytrowski/dotty-rs/commit/7eb7eda2b6b6eafda2b0989be907527f7035b38d) | [review](https://github.com/scytrowski/dotty-rs/pull/2171#pullrequestreview-5807089636) | [`1e4d48bcaac479384fd2e830a9f928daee27d402`](https://github.com/scytrowski/dotty-rs/commit/1e4d48bcaac479384fd2e830a9f928daee27d402), 2026-11-06 21:31:26 |
| #2165 | [#2172](https://github.com/scytrowski/dotty-rs/pull/2172) | [`28e693809bd9ef82ae70cb1dc5c93da349ecd4bd`](https://github.com/scytrowski/dotty-rs/commit/28e693809bd9ef82ae70cb1dc5c93da349ecd4bd) | [review](https://github.com/scytrowski/dotty-rs/pull/2172#pullrequestreview-5807843186) | [`c1954485ac382b1d0c710d1f2e974f1c639e4ca5`](https://github.com/scytrowski/dotty-rs/commit/c1954485ac382b1d0c710d1f2e974f1c639e4ca5), 2026-11-07 00:07:50 |
| #2166 | [#2173](https://github.com/scytrowski/dotty-rs/pull/2173) | [`11d902771bd918817c3096b622d9957cb94a516d`](https://github.com/scytrowski/dotty-rs/commit/11d902771bd918817c3096b622d9957cb94a516d) | [review](https://github.com/scytrowski/dotty-rs/pull/2173#pullrequestreview-5809387128) | [`3a92a8a6e292f14ed5deebf59bf4da624ce0bcd8`](https://github.com/scytrowski/dotty-rs/commit/3a92a8a6e292f14ed5deebf59bf4da624ce0bcd8), 2026-11-07 02:08:57 |
| #2167 | [#2174](https://github.com/scytrowski/dotty-rs/pull/2174) | [`71c8c7e2a6d64d022dd49825a45f41865d1fcdfe`](https://github.com/scytrowski/dotty-rs/commit/71c8c7e2a6d64d022dd49825a45f41865d1fcdfe) | [review](https://github.com/scytrowski/dotty-rs/pull/2174#pullrequestreview-5809729576) | [`0478e877de638f8b9a27bb392f234593babcc991`](https://github.com/scytrowski/dotty-rs/commit/0478e877de638f8b9a27bb392f234593babcc991), 2026-11-07 10:34:34 |
| #2168 | [#2177](https://github.com/scytrowski/dotty-rs/pull/2177) | [`73d29488ed122b6979eade0c5d5c9fbd714f1b0f`](https://github.com/scytrowski/dotty-rs/commit/73d29488ed122b6979eade0c5d5c9fbd714f1b0f) | [review](https://github.com/scytrowski/dotty-rs/pull/2177#pullrequestreview-5812269655) | [`8c73292d054ad5df9960539718e441ecec8a7ae6`](https://github.com/scytrowski/dotty-rs/commit/8c73292d054ad5df9960539718e441ecec8a7ae6), 2026-11-07 12:57:01 |
| #2169 | [#2178](https://github.com/scytrowski/dotty-rs/pull/2178) | [`a267714ecd8c51159dcdd7c66e5f9f1ea3c74289`](https://github.com/scytrowski/dotty-rs/commit/a267714ecd8c51159dcdd7c66e5f9f1ea3c74289) | [review](https://github.com/scytrowski/dotty-rs/pull/2178#pullrequestreview-5815019092) | [`89f81940fc538a7b24323b82b9415cdb6a7c5404`](https://github.com/scytrowski/dotty-rs/commit/89f81940fc538a7b24323b82b9415cdb6a7c5404), 2026-11-07 21:33:02 |

### Consumer-to-producer mapping

Both consumers compile the unchanged source bytes from `tests/fixtures/jvm_tasty_local_receiver_call/LocalReceiverCall.scala` (SHA-256 `d00efdafda78682532599fbd9017a2037d1579241fa388c744f8f58d13b42eb8`). They call the producer with different source filenames and session IDs, so their emitted six-file batches and UUIDs are different. `RuntimeProbe.java` (`d02400bbd69140197a8948c3074cd61d38a077e9b2283102a6f00573c8ecfc2d`) is a historical Scala-oracle fixture only; it is not the Java client executed by the current Rust runtime test.

| Consumer run | Producer filename / session | Actual client source |
| --- | --- | --- |
| Java verifier/runtime | `LocalReceiverCall.scala`, primary `SourceId(21681)` and reversed `SourceId(21682)` | Embedded `HARNESS` in `tests/jvm_tasty_local_receiver_call_runtime.rs`, compiled as `JavaConsumer.java`; SHA-256 `ac193ace840cff262983583ebbcae0f87f90151abf1021390b7c3ec7273e4cef` |
| Scala source consumer | `PairFactory.scala`, primary `SourceId(21690)` and reversed `SourceId(21691)` | `tests/fixtures/jvm_tasty_two_local_counter_set/Consumer.scala`; SHA-256 `5d9d77abad4111fd49e135b9bdd044e14a177d35440852de89440d8c6d50e61e` |

The Java primary and reversed output batches are emitted and hashed by `local_counter_set_runs_on_unchanged_rust_batch_under_strict_jdk21`. The Scala primary and reversed batches are emitted and consumed by `official_scala_consumes_two_local_counter_set_rust_artifacts`. In each table, the class entry's UUID is the UUID in its matching sidecar header; the tests validate this link directly.

| Java consumer, `LocalReceiverCall.scala` | SHA-256 | Linked TASTy UUID |
| --- | --- | --- |
| Primary `example/Counter.class` | `00d8c580a7de082187963ef68da3ed2c0fc434c8ffd7eb74fc3b4fc284d15f4b` | `00c921ab9a680cac0000000000008193` |
| Primary `example/Counter.tasty` | `a810aae114aa178ae6d46ea0186cad24b2accbeb1f60a261ff86bc0ca6bddebc` | `00c921ab9a680cac0000000000008193` |
| Primary `example/Pair.class` | `7c013db083dec06742cb25c9d5e8d624a056f6f7d76d9b19ab1691b99db9cb37` | `00422de52e20a3f00000000000008198` |
| Primary `example/Pair.tasty` | `0f4ba6c53d4b71b058db4a606753ad2afdbc854a05f2a57d356e51f3066a8652` | `00422de52e20a3f00000000000008198` |
| Primary `example/PairFactory.class` | `b9cb66f944343b13bd166d2ac85025d2cd203608830658e6fe3a410100570286` | `008977b0069599cb000000000000819d` |
| Primary `example/PairFactory.tasty` | `18c9f230a0c519810e13e50dcae8adff351c20348a30aef903078df231e8559a` | `008977b0069599cb000000000000819d` |
| Reversed `example/Counter.class` | `00d8c580a7de082187963ef68da3ed2c0fc434c8ffd7eb74fc3b4fc284d15f4b` | `00c921ab9a680cac0000000000008193` |
| Reversed `example/Counter.tasty` | `a810aae114aa178ae6d46ea0186cad24b2accbeb1f60a261ff86bc0ca6bddebc` | `00c921ab9a680cac0000000000008193` |
| Reversed `example/Pair.class` | `7c013db083dec06742cb25c9d5e8d624a056f6f7d76d9b19ab1691b99db9cb37` | `00422de52e20a3f00000000000008198` |
| Reversed `example/Pair.tasty` | `0f4ba6c53d4b71b058db4a606753ad2afdbc854a05f2a57d356e51f3066a8652` | `00422de52e20a3f00000000000008198` |
| Reversed `example/PairFactory.class` | `8e1a18a0a329c6a44165ec3a1e5b93c090423f5d631482df35f49dc1dcc09406` | `008e2c12fc87a4cb000000000000819d` |
| Reversed `example/PairFactory.tasty` | `3b57917fe8a1966fc4af671aa620693c8c8fdc18b202a5be1e2ee2a0ca6c99e3` | `008e2c12fc87a4cb000000000000819d` |

| Scala consumer, `PairFactory.scala` | SHA-256 | Linked TASTy UUID |
| --- | --- | --- |
| Primary `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008071c3c00968ac0000000000008193` |
| Primary `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | `008071c3c00968ac0000000000008193` |
| Primary `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d7441c7f00000000000008198` |
| Primary `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | `000b7d8d7441c7f00000000000008198` |
| Primary `example/PairFactory.class` | `db036a44f33a34661e2849215ededc022c4f8fb01bb89c7bc02009a4d7125f0d` | `00c027d85cf4fdcb000000000000819d` |
| Primary `example/PairFactory.tasty` | `3193a268004e9b6481789c34b4fcd70d03c94324c511dafdd5a62d7abbc3a2ca` | `00c027d85cf4fdcb000000000000819d` |
| Reversed `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008071c3c00968ac0000000000008193` |
| Reversed `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | `008071c3c00968ac0000000000008193` |
| Reversed `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d7441c7f00000000000008198` |
| Reversed `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | `000b7d8d7441c7f00000000000008198` |
| Reversed `example/PairFactory.class` | `59bf043cf27d266a5b23de4f99be1d031d219a02a649b62d9bddb56560415034` | `00c77c7aa6e6c0cb000000000000819d` |
| Reversed `example/PairFactory.tasty` | `7086eb507918f67003edd23a18c362ac47d695eeaa8e2d468efe853e5243ebc4` | `00c77c7aa6e6c0cb000000000000819d` |

### Measured negative-control first failures

These are the observed first rejection stages from the integrated Java and Scala harnesses, not inferred producer guarantees. The byte-level tamper tests separately assert that Rust artifact bytes are unchanged after external consumers run.

| Control | First observed failure |
| --- | --- |
| Java batch: wrong receiver, wrong seed, skipped setter or duplicate setter | Runtime semantic assertion: `create(7,3) expected 3/3` |
| Java batch: wrong opcode/owner/descriptor, undersized stack/locals or call before receiver initialization | JVM verification under `-Xverify:all`: `VerifyError` |
| Java batch: missing setter method / missing `Counter.class` | Method resolution `NoSuchMethodError` / class resolution `NoClassDefFoundError` |
| Scala consumer: missing or misplaced `.tasty` sidecar | Scala binary loader: class file expected its `.tasty` sidecar; sidecar missing at the required path |
| Scala consumer: corrupt sidecar / mismatched classfile UUID | Scala TASTy parser reports broken file (`ArrayIndexOutOfBoundsException`) / rejects out-of-sync TASTy UUID |
| Scala consumer: invalid BLOCK target `TERMREF` | `readTasty`: `TreeWithoutOwner` |
| Rust source: `first.set()` / `first.set(true)` | Method body typing: `ApplicationArityMismatch` / `ApplicationArgumentTypeMismatch` |
| Rust source: extra setter, wrong formal or moved call | Cross-class identity proof: `SourceShape` for unsupported statement/argument; or JVM/TASTy contract validation: `TastyPlan(BodyType)` for the moved call |
| Rust source: non-Unit setter expression | Cross-class identity proof: `Counter.set statement must be one regular one-argument Apply` |

### Acceptance checkpoint matrix

| Checkpoint | Integrated test/evidence | Observed result |
| --- | --- | --- |
| #2170.2: canonical source/typed proof | `jvm_tasty_local_receiver_call::local_receiver_call_audit_keeps_the_real_frontend_identities_and_first_failure` | Real frontend session `SourceId(21640)` preserves two local symbols and ordered stats; typed receiver resolves to `first`, selected member to `Counter.set`, argument to `secondSeed`, final Pair arguments to their original formals; call span `[522, 543)`. Passed locally. |
| #2170.3: authentic Rust TASTy and JVM statement order | `jvm_tasty_local_receiver_call::local_receiver_unit_call_emits_one_linked_six_artifact_batch` | Exactly six linked files; Rust TASTy `BLOCK` stats are `VALDEF, VALDEF, APPLY`, followed by final constructor `APPLY`. `PairFactory.create(II)Lexample/Pair;` initializes/stores both Counters, invokes `Counter.set(I)V` once on `first` with `secondSeed`, then constructs `Pair(first, second)`; `max_stack=4`, `max_locals=5`, class major 65 and TASTy 28.9. Passed locally. |
| #2170.4–5: linked outputs, determinism and inherited no-call bytes | `jvm_tasty_local_receiver_call_runtime::local_counter_set_runs_on_unchanged_rust_batch_under_strict_jdk21`; the two six-file consumer tables above | Each Java source variant is emitted twice in fresh sessions with byte-identical six-file output; Scala primary/reversed outputs and UUIDs are separately recorded. The original no-call `Counter` and `Pair` class/TASTy bytes remain identical to their inherited batch. Passed locally. |
| #2170.6: independent external consumers | Java `JavaConsumer` harness and Scala `Consumer.scala`, mapped to their producer filename/session and output hashes above | Java 21 `-Xverify:all -ea` and pinned Scala 3.9 compile/runtime, readers and lookups passed locally for primary and reversed declarations. Current-head hosted consumer gate remains pending. |
| #2170.7: negative controls and accepted-artifact publisher | Negative first-failure table above; `jvm_tasty_artifacts::immutable_local_pair_batch_publisher_rejects_partial_and_unsafe_outputs` | Existing source/typed/TASTy/JVM/Scala/Java negatives retain their observed first stages. Publisher checks inject failure after each of six staged artifact writes, before promotion and after promotion; each failure leaves no staging/final tree. The pre-existing destination sentinel remains byte-identical and is the only file left. Passed in the locked workspace suite. |
| #2170.8: current strict inventory and fail-closed execution | `tools/ci/validate_strict_tasty_targets.py`; PR runs linked in the exact-head evidence comment | Inventory validates 25 unique targets over four shards. Final-head hosted shard/aggregate outcomes are pending; historical prerequisite checks are not substituted. |
| #2170.9: reproducible provenance and final-head receipt | Prerequisite review/head/merge table above; [exact-head evidence comment](https://github.com/scytrowski/dotty-rs/pull/2185#issuecomment-6713923017) | Inputs, both consumer clients, producer filenames/sessions, artifact hashes/UUIDs, and measured negative failures are recorded here. The linked comment binds workflow/job outcomes to the final PR SHA and tree; hosted checks remain pending until completion. |

### Tool and final-head status

The Scala 3.9 source checkout was built from revision `777528f19a58e794c9954a42f433373472ec57f8`. The staged compiler reported `3.9.0-bin-SNAPSHOT-git-777528f` with SHA-256 `8972d966eae9b43346b89e4f3cf608ce9ff90a20764dca8f8d5c13d98587c2e6`; the official compiler artifact used by the pinned strict workflow is SHA-256 `3e2c09a5ca6cef17a55f2f66e123815f9c4ea061fce20d0e9d2371989405a61a`. Rust is 1.98.1. The Scala runtime JAR hashes are `9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba` (`scala3-library_3`) and `b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd` (`scala-library`). Temurin JDK 21.0.13 `java` SHA-256 is `9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`.

The local workspace run on review head `27d52766c856b5214c9291ebd117c56f124e2b9c` passed the locked workspace tests, Clippy, rustdoc, formatting, frontend boundary guard, strict inventory (25 unique targets/four shards), mandatory Java 21 gate and pinned Scala consumer (9/9 tests). Those local checks precede this documentation correction and are not final-head CI receipts. The current final-head check outcomes and direct workflow/job URLs are recorded in the [exact-head PR evidence comment](https://github.com/scytrowski/dotty-rs/pull/2185#issuecomment-6713923017); mandatory hosted checks stay pending until their actual completion. No earlier prerequisite or historical CI pass is counted as final release success.

Unsupported language forms remain those in the bounded controls above. This PR adds acceptance evidence only and changes no production semantic, JVM, TASTy or publisher behavior.
