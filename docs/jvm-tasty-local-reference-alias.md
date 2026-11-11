# Immutable local reference alias: Scala 3.9 audit

This is the fixtures/tests/docs-only audit for issue #2232, based on merged
main `cd8d72f09c6e9342c0a761c8887bb2c48a52add8` and its #2194/#2222
predecessor. The frozen source is
[`LocalReferenceAlias.scala`](../tests/fixtures/jvm_tasty_local_reference_alias/LocalReferenceAlias.scala).
Its `Counter` and `Pair` declarations retain the exact members in the existing
ordered-local fixture. Only `PairFactory.create` adds the immutable
`alias: Counter = first`, then calls `alias.set(secondSeed)` and
`second.set(firstSeed)` before constructing `Pair(first, second)`.

## Pinned Scala and JDK result

The official Scala source checkout was verified at revision
`777528f19a58e794c9954a42f433373472ec57f8`. It built compiler
`3.9.0-bin-SNAPSHOT-git-777528f` with the project-declared sbt 1.12.1 and
Temurin JDK 21.0.8+9. The compiler jar SHA-256 is
`cf8ebddf01b86e563c179f72ab973dbc679092ca1d8c733ead31516cc01b9295`; the
complete compiler, runtime, JDK, source, client and generated artifact hashes
are in [`oracle.json`](../tests/fixtures/jvm_tasty_local_reference_alias/oracle.json).

[`RuntimeProbe.java`](../tests/fixtures/jvm_tasty_local_reference_alias/RuntimeProbe.java)
was compiled independently with `javac --release 21` and run with
`java -Xverify:all -ea`. It printed:

```text
3/7 distinct=true
42/-9 distinct=true
```

This verifies that the alias changes `first`, the second call changes the
distinct second counter, and the final pair stores those same two instances.
`PairFactory.create` has descriptor `(II)Lexample/Pair;`, class-file major 65,
`max_stack=4`, and `max_locals=6`. `javap -v -c -p` shows exactly two Counter
allocations: `first` in slot 3 and `second` in slot 4. The alias copies slot 3
into slot 5 (`aload_3; astore 5`); its call loads slot 5 and invokes
`Counter.set(I)V`. The next call loads slot 4, then the Pair constructor loads
slots 3 and 4. There is no third Counter allocation.

Scala emitted TASTy 28.9.0. The create `BLOCK@58` has ordered statements
`VALDEF@76` (`first`), `VALDEF@99` (`second`), `VALDEF@120` (`alias`),
`APPLY@129`, and `APPLY@140`; its result is `APPLY@60`. The alias RHS at 127
shares the term reference at 72, which points to the `first` declaration at
76. The first call receiver at 134 points to alias declaration 120. The second
receiver at 145 shares term reference 74, which points to `second` at 99. The
resulting Pair arguments use references 72 and 74, preserving the original
`first`, `second` identities. Raw outputs are checked in under `oracle/`.
`scalac -from-tasty` also consumed the generated PairFactory TASTy and emitted
both class and TASTy files; its class bytes differ from the primary compile and
are not claimed to be byte-identical.

## Rust frontend identities and measured initial paired rejection (pre-#2240)

The first implementation checkpoint recorded the focused Rust audit over the
real lexer, parser, namer and source typer in one source session
(`SourceId(22320)`). The create method was `SymbolId(60)`,
with formal symbols 61 and 62. Its immutable locals are symbols 63 (`first`),
64 (`second`) and 65 (`alias`), all owned by create and in `ScopeId(18)`. The
typed alias initializer refers to symbol 63. The two ordered call receivers
refer to symbols 65 and 64; the final typed Pair constructor arguments refer to
63 and 64. Source statements map through the shared source/typed index as
`55→31`, `62→38`, `65→41`, `69→45`, and `73→49`; the alias initializer maps
`64→39`, and the final Pair expression maps `79→55`. These are the printed
source and typed `TreeId` indexes in this exact session. The test asserts each
mapping and does not infer identities from names or allocation order.

At that checkpoint, the paired producer's first failure on the verified main
source was phase `cross-class constructor-call identity proof` with
`SourceShape("factory body must be a direct Apply or a one- or two-local
Block")`. The plan callback is not reached and
`emit_paired_jvm_tasty_batch` returns this planning error without a batch, so
there were zero paired artifacts to publish. This is the measured initial
failure before #2240, not the historical error copied from an earlier audit.
The strict zero-, one- and two-call baselines remain accepted by the paired
plan.

## Control matrix

Each alias control has its own source file and was independently compiled
against the frozen fixture's unchanged `Counter`/`Pair` classes with the pinned
Scala compiler and `-Werror -release 21`. Valid controls recorded at the
initial checkpoint were renamed locals/alias, reversed local declarations
(alias still after both declarations), aliasing `second`, `var alias`, and an
alias chain. These all reached the measured `SourceShape` rejection then. The
#2240 integration accepts renaming and reversed declarations for the bounded
alias-to-first shape; aliasing `second`, `var alias`, and alias chains remain
outside the accepted shape. The alias-of-second case is valid Scala.
`AliasBeforeInitialization.scala` is invalid with Scala E039 (forward
reference). `WrongAliasType.scala` is invalid with E007 (Counter supplied
where Pair is required) and E008 (`set` is not a Pair member). The paired
source typer reported `TermNameNotFound` for the forward reference and
`LocalValueTypeMismatch` for the wrong type. The old no-alias 0/1/2-call source
fixtures were recompiled with the pinned Scala compiler and remain accepted by
the Rust paired plan.

At this initial checkpoint, no production compiler, paired publisher,
acceptance condition, or CI shard had changed. The bounded producer integration
and its artifact contract are implemented below in #2240.

## Paired JVM/TASTy producer integration (#2240)

Issue #2240 integrates the exact frozen alias source above with the existing
one-session three-class producer. The source and typed receipts now admit only
the two original `new Counter(seed)` locals, a third immutable direct alias of
the local passed as the first Pair argument, the two ordered original
`Counter.set(I)V` calls, and the final Pair construction from the two original
locals. The paired JVM contract derives every local slot from the original
`JvmCompilationUnit` layout and checks the alias load/store before the setter
calls; the TASTy plan serializes the alias as its own `VALDEF` with the original
local's `TERMREF` target.

The Rust paired-artifact regression compares all six paths, bytes, and UUIDs
across two fresh source sessions; verifies class-file major 65, TASTy 28.9.0,
and each class-file/sidecar UUID link; and executes the generated batch under
JDK 21 `-Xverify:all` for the frozen values and distinct Pair fields. Renamed
and reversed original declarations are accepted. Mutable aliases, aliases of
the second local, alias chains, extra locals or allocations, nested conditional
calls, duplicate setters, and Pair arguments changed to the alias are rejected.
Artifact-level negatives mutate the alias copy load/store, setter receiver,
setter order/count, extra allocation, and final Pair loads; the validator
rejects each at `cross-class invocation` before a batch can be published. A
TASTy alias `TERMREFdirect` mutation to the second local keeps the original
UUID and class-file link valid, then fails at `TASTy plan identity`. The
same-typed wrong JVM alias load also passes `-Xverify:all` and reaches the
runtime harness's `AssertionError`, rather than a verifier or linkage error.
The original zero-, one-, and two-call no-alias producer paths remain on their
existing branches and keep their focused output tests.

The Scala evidence above remains the pinned-language oracle for this source
shape. The integration adds no separate Java or Scala consumer feature.

## Independent Rust artifact runtime acceptance (#2243)

`jvm_tasty_local_receiver_call_runtime::aliased_counter_identity_and_ordered_calls_execute_under_strict_jdk21`
emits the unchanged `LocalReferenceAlias.scala` source through the paired Rust
producer, then compiles the separate
[`RustRuntimeProbe.java`](../tests/fixtures/jvm_tasty_local_reference_alias/RustRuntimeProbe.java)
with `javac --release 21` against those six files. Temurin JDK 21 runs the
client with `java -Xverify:all -ea`; it observes `3/7`, `42/-9`, and `7/7`,
checks distinct Counter and Pair identities for equal values and repeated
factory calls, and independently mutates both Counter fields by reflection.
The probe source SHA-256 is
`fd9b6bbd2b2d4ea1c02c8568341414a75e501a310e17a8d688adf8f87e2e5946`.

The Rust artifacts are pinned by path and SHA-256 in the regression. Their
class-file TASTY links point to these sidecar UUIDs:

| Artifact path | SHA-256 | TASTy UUID |
| --- | --- | --- |
| `example/Counter.class` | `68d6a0af84c82311ce06b38b1e98ea69493643c5c99acde2cc4b377d31913634` | `00edafe3-c150-85ac-0000-000000008193` |
| `example/Counter.tasty` | `ff7a6a332352b38dd2022f9a5c5b5de3a40d336eef1b2859bc226d43383dcc6e` | same |
| `example/Pair.class` | `4ff943e4a8310b37c7010a3901b5d75979823524d240902cc25a67347953a9b0` | `00cda16b-8275-dff0-0000-000000008198` |
| `example/Pair.tasty` | `5d9083ae32ee7aec20702d6220c7f7fc3c6be3dca2019a93963ae33c1323f770` | same |
| `example/PairFactory.class` | `4458dadc7c09d16e3868872e135b80233597a7d567fbc28964cf9468b96b5d7b` | `0021e620-2123-f1c2-0000-00000000819e` |
| `example/PairFactory.tasty` | `8b1468fb6948a583210407f30a664a7dba2cf574e0bb8b114a04eb153fefd5bb` | same |

The test checks `create(II)Lexample/Pair;`, class-file major 65,
`max_stack=4`, `max_locals=6`, exactly two Counter allocations, and the
derived first, second and alias local slots. `javap -v -c -p` and decoded
bytecode both show the alias copied from the initialized first local, the
alias receiver with `secondSeed`, the original second receiver with
`firstSeed`, and Pair construction from the original first and second locals.
All six artifact bytes, paths and UUID links are rechecked after compilation,
disassembly and execution.

Bounded mutations observe the first actual failure: a valid wrong alias slot,
omitted setter, and swapped Pair arguments reach the Java runtime assertions;
loading the alias before its store and wrong setter owner/descriptor fail with
`VerifyError`; missing `Counter.class` fails with `NoClassDefFoundError`.
Reduced stack/local bounds also fail verification. Duplicate-setter and extra
allocation controls are rejected by the observed bytecode contract at three
setter sites and three Counter allocations, respectively. These controls use
the existing runtime harness and mutation helpers; the test does not add a
workflow or change the producer.

## Reproduction

Build from a copy of the pinned source checkout so sbt outputs stay in the
task's managed scratch directory. With SDKMAN initialized, this checkout
selects sbt 1.12.1; `XDG_RUNTIME_DIR=.` keeps sbt's Unix socket short and in
that scratch copy.

```sh
source "$HOME/.sdkman/bin/sdkman-init.sh"
SCRATCH=/path/to/managed-scratch
SCALA_SOURCE=/path/to/scala3-at-777528f
cp -a "$SCALA_SOURCE" "$SCRATCH/scala39-source"
cd "$SCRATCH/scala39-source"
export TMPDIR="$SCRATCH" TMP="$SCRATCH" TEMP="$SCRATCH"
export XDG_RUNTIME_DIR=.
export JAVA_HOME=/path/to/temurin-21
export PATH="$JAVA_HOME/bin:$PATH"
export JAVA_TOOL_OPTIONS="-XX:-UsePerfData -Djava.io.tmpdir=$SCRATCH"
export SBT_OPTS="-Dsbt.boot.directory=$SCRATCH/sbt/boot -Dsbt.global.base=$SCRATCH/sbt/global -Dsbt.ivy.home=$SCRATCH/ivy2"
sbt -batch 'dist-linux-x86_64/universal:stage'

SCALAC="$SCRATCH/scala39-source/dist/linux-x86_64/target/universal/stage/bin/scalac"
FIXTURES=/path/to/dotty-rs/tests/fixtures
OUT="$SCRATCH/local-reference-alias"
mkdir -p "$OUT/classes" "$OUT/typed" "$OUT/from-tasty" "$OUT/baselines"
"$SCALAC" -Werror -release 21 -d "$OUT/classes" \
  "$FIXTURES/jvm_tasty_local_reference_alias/LocalReferenceAlias.scala"
javac --release 21 -cp "$OUT/classes" -d "$OUT/classes" \
  "$FIXTURES/jvm_tasty_local_reference_alias/RuntimeProbe.java"
java -Xverify:all -ea -cp "$OUT/classes" example.RuntimeProbe
javap -v -c -p -classpath "$OUT/classes" example.PairFactory > "$OUT/PairFactory.javap.txt"
"$SCALAC" -color:never -print-tasty "$OUT/classes/example/PairFactory.tasty" \
  > "$OUT/PairFactory.print-tasty.txt"
"$SCALAC" -Werror -release 21 -Vprint:typer -d "$OUT/typed" \
  "$FIXTURES/jvm_tasty_local_reference_alias/LocalReferenceAlias.scala" \
  > "$OUT/typer.txt" 2>&1
"$SCALAC" -from-tasty -classpath "$OUT/classes" -d "$OUT/from-tasty" \
  "$OUT/classes/example/PairFactory.tasty"

for source in "$FIXTURES"/jvm_tasty_local_reference_alias/controls/*.scala; do
  name=$(basename "$source" .scala)
  mkdir -p "$OUT/controls/$name"
  if "$SCALAC" -Werror -release 21 -classpath "$OUT/classes" \
      -d "$OUT/controls/$name" "$source"; then
    printf '%s: Scala-valid\n' "$name"
  else
    printf '%s: Scala-invalid\n' "$name"
  fi
done

for entry in \
  'zero:jvm_tasty_two_immutable_reference_locals/TwoLocals.scala' \
  'one:jvm_tasty_local_receiver_call/LocalReceiverCall.scala' \
  'two:jvm_tasty_ordered_local_unit_calls/OrderedLocalUnitCalls.scala'; do
  name=${entry%%:*}
  source=${entry#*:}
  mkdir -p "$OUT/baselines/$name"
  "$SCALAC" -Werror -release 21 -d "$OUT/baselines/$name" "$FIXTURES/$source"
done
```

The Scala controls and the unchanged 0/1/2-call baseline sources are listed
with SHA-256 and outcome in `oracle.json`. To reproduce the Rust audit, run
`cargo +1.98.1 test -p dotty --test jvm_tasty_local_receiver_call
local_reference_alias_audit_authenticates_frontend_identities_and_first_paired_failure
--locked -- --nocapture --test-threads=1`.

## Integrated Java and Scala consumer provenance (#2246 checkpoint)

The final-gate consumers now emit the same canonical source through the paired
producer. Both `LocalReferenceAlias.scala` and
`jvm_tasty_two_local_counter_set/AliasTwoCalls.scala` have SHA-256
`b1b200af5108685cf58b769b3ed0654c189d2fb28fa1c8eaaba5f02580d2b79f` and
contain identical bytes. Both consumer tests pass the producer label
`PairFactory.scala`; they share the expected artifact table in
[`tests/support/immutable_alias_artifacts.rs`](../tests/support/immutable_alias_artifacts.rs).
The earlier Java receipt used `LocalReferenceAlias.scala`, so its former
hashes and UUIDs remain a separate historical receipt and are not combined
with this canonical batch.

The seven prerequisite PRs are merged in dependency order into integration
base `b7ad6d2599183864166f1e72ebe9ab50893c79cf`. Each row records its
reviewed head, published READY_FOR_MERGE review, merge commit, and successful
PR-head Rust/JDK and strict Scala workflow runs:

| Issue / PR | Reviewed PR head | Review | Merge commit | Rust/JDK checks | Strict Scala shards / aggregate |
| --- | --- | --- | --- | --- | --- |
| #2232 / [#2251](https://github.com/scytrowski/dotty-rs/pull/2251) | `0c57b66b639823ba06f16542e21f57c0d1d7a9a2` | [review](https://github.com/scytrowski/dotty-rs/pull/2251#pullrequestreview-5841472980) | `55b3e1a776da32ddcd0afd970779d1887708135a` | [run](https://github.com/scytrowski/dotty-rs/actions/runs/42811783778) | [run](https://github.com/scytrowski/dotty-rs/actions/runs/42811783764) |
| #2235 / [#2252](https://github.com/scytrowski/dotty-rs/pull/2252) | `82858a0bf3aecebb0d2c8979e60345c246830462` | [review](https://github.com/scytrowski/dotty-rs/pull/2252#pullrequestreview-5842598495) | `36bcd961ad00ad9af82f518baa32885c0df253d6` | [run](https://github.com/scytrowski/dotty-rs/actions/runs/42823834695) | [run](https://github.com/scytrowski/dotty-rs/actions/runs/42823834758) |
| #2236 / [#2254](https://github.com/scytrowski/dotty-rs/pull/2254) | `94769b1e24d785605987f254a1784dd5b899518e` | [review](https://github.com/scytrowski/dotty-rs/pull/2254#pullrequestreview-5844055843) | `3aa9f719b7f79d80ebe46e47a1b390afdde9f15e` | [run](https://github.com/scytrowski/dotty-rs/actions/runs/42839522026) | [run](https://github.com/scytrowski/dotty-rs/actions/runs/42839521991) |
| #2239 / [#2255](https://github.com/scytrowski/dotty-rs/pull/2255) | `7254558f46394dd0b7c53640b0c1c72eb47c5950` | [review](https://github.com/scytrowski/dotty-rs/pull/2255#pullrequestreview-5846046962) | `0238061ccf674c0e15b8f055c971ed1cb6db5f51` | [run](https://github.com/scytrowski/dotty-rs/actions/runs/42860587954) | [run](https://github.com/scytrowski/dotty-rs/actions/runs/42860587879) |
| #2240 / [#2257](https://github.com/scytrowski/dotty-rs/pull/2257) | `c9b4ea2e3083dc71c302e43f1116a6a721aed2cd` | [review](https://github.com/scytrowski/dotty-rs/pull/2257#pullrequestreview-5850555866) | `f200b6fb6b86438886c6d4f6447ca1077da94b7b` | [run](https://github.com/scytrowski/dotty-rs/actions/runs/42915032747) | [run](https://github.com/scytrowski/dotty-rs/actions/runs/42915032757) |
| #2243 / [#2259](https://github.com/scytrowski/dotty-rs/pull/2259) | `19f2b7880c78ac7fe6e66ee773156286ba2d3258` | [review](https://github.com/scytrowski/dotty-rs/pull/2259#pullrequestreview-5855686149) | `333ae382e04884850b88090f15e1efe557cb3b60` | [run](https://github.com/scytrowski/dotty-rs/actions/runs/42984305118) | [run](https://github.com/scytrowski/dotty-rs/actions/runs/42984305306) |
| #2244 / [#2261](https://github.com/scytrowski/dotty-rs/pull/2261) | `3ae605c8771bfa13b2e3bb355e18eb7b7615e580` | [review](https://github.com/scytrowski/dotty-rs/pull/2261#pullrequestreview-5857261839) | `b7ad6d2599183864166f1e72ebe9ab50893c79cf` | [run](https://github.com/scytrowski/dotty-rs/actions/runs/43002287174) | [run](https://github.com/scytrowski/dotty-rs/actions/runs/43002287257) |

The integration-base push checks also passed: [Rust checks](https://github.com/scytrowski/dotty-rs/actions/runs/43042366536/job/130597261524)
and [the mandatory classfile/JVM gate](https://github.com/scytrowski/dotty-rs/actions/runs/43042366536/job/130597260951).
The final-gate PR still needs its own final-head hosted checks; predecessor
results are provenance, not a substitute for those checks.

The canonical six Rust artifacts are:

| Artifact path | SHA-256 | TASTy UUID |
| --- | --- | --- |
| `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008071c3-c009-68ac-0000-000000008193` |
| `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | same |
| `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d-7441-c7f0-0000-000000008198` |
| `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | same |
| `example/PairFactory.class` | `68027bf87468fd724f034cc4fd67d4e57220e389fb4130f9e3a33bcd2cea98e1` | `00c9bae5-ea1a-54c2-0000-00000000819e` |
| `example/PairFactory.tasty` | `e56be0c7ff675f7d05a34914cb924c13fac030a54e72e06612de03b286a570ed` | same |

The independent Java runtime probe fixture is SHA-256
`fd9b6bbd2b2d4ea1c02c8568341414a75e501a310e17a8d688adf8f87e2e5946`;
the independent Scala client fixture is SHA-256
`50857d2606e3d90296ea6a2fe4b766bca0c9ecd0574a44014f53523c9372e7af`.
The Java strict-JDK test passed with Temurin 21.0.13 using
`DOTTY_CLASSFILE_REQUIRE_JDK=1`. The Scala alias consumer passed using Rust
1.98.1, official Scala source revision
`777528f19a58e794c9954a42f433373472ec57f8` and Temurin 21.0.13 with
`DOTTY_REQUIRE_SCALA_TASTY_TWO_LOCAL_COUNTER_SET_CONSUMER=1`. Its output
reports the same six hashes and UUID links, the unchanged original artifacts
after Scala compilation/execution and the original, renamed-alias and
reversed-local variants. The Scala client runs against the producer's original
Rust artifacts; `-print-tasty`, `-decompile` and `-from-tasty` outputs remain
inspection-only.

These local focused runs authenticate the common input and both consumer
paths. They do not complete the final release gate: the strict four-shard
workflow, its aggregate, mandatory JDK workflow and standard locked workspace
checks must still pass on the final PR head, and their exact hosted run/job
links must be added after that head is fixed. The checked-in strict manifest
currently validates 25 targets across four shards; this receipt does not claim
hosted checks or final acceptance.
