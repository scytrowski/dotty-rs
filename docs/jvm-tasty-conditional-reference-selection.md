# Conditional local reference selection: Scala 3.9 paired producer contract

This report freezes the one-source `Counter` / `Pair` / `PairFactory` input
from issue #2265 and records the pinned Scala 3.9.0/JDK 21 oracle and the
paired producer contract implemented for issue #2270. The Scala `.class` and `.tasty` files in
[`tests/fixtures/jvm_tasty_conditional_reference_selection/scala39`](../tests/fixtures/jvm_tasty_conditional_reference_selection/scala39)
are official Scala compiler outputs. They are not Rust-produced artifacts.
Source, Java consumer, binary, snapshot, control, and tool hashes are in the
adjacent [`oracle.json`](../tests/fixtures/jvm_tasty_conditional_reference_selection/oracle.json).

The `Counter` and `Pair` declarations are byte-for-byte unchanged from
[`LocalReferenceAlias.scala`](../tests/fixtures/jvm_tasty_local_reference_alias/LocalReferenceAlias.scala).
Only `PairFactory.create` is the conditional target:

```scala
def create(firstSeed: Int, secondSeed: Int, chooseFirst: Boolean, nextValue: Int): Pair =
  val first: Counter = new Counter(firstSeed)
  val second: Counter = new Counter(secondSeed)
  val selected: Counter =
    if chooseFirst then first else second
  selected.set(nextValue)
  new Pair(first, second)
```

## Official Oracle observations

The compiler source is pinned to revision
`777528f19a58e794c9954a42f433373472ec57f8` (Scala 3.9.0, TASTy 28.9.0),
built with sbt 1.12.1 and Eclipse Temurin JDK 21.0.13+9. The full `javap`,
`-print-tasty`, `-decompile`, and typer outputs are checked in beside the
fixture. The independent Java probe ran with
`java -Xverify:all -ea` and observed:

| Call | `leftValue/rightValue` | Pair fields |
| --- | --- | --- |
| `create(7, 3, true, 42)` | `42/3` | distinct Counter objects |
| `create(7, 3, false, 42)` | `7/42` | distinct Counter objects |

The class file for `create` has descriptor `(IIZI)Lexample/Pair;`, major
version 65, `max_stack=4`, and `max_locals=8`. It emits exactly two
`new Counter` instructions. The observed method locals are `first=5`,
`second=6`, and `selected=7`. At bytecode offsets 20–31, the Boolean test
branches to the original first or second local and stores that reference in
slot 7. The following setter loads slot 7; final Pair construction loads
slots 5 and 6. `StackMapTable` records the two initialized Counter locals at
the false branch and the joined Counter reference at the merge.

The TASTy `create` body is a `BLOCK` with `VALDEF first`, `VALDEF second`,
`VALDEF selected`, then the setter `APPLY`; its result is the Pair constructor
`APPLY`. The selected initializer is an `IF`: its condition references the
`chooseFirst` formal, and its branches share references to the original first
and second `VALDEF`s. The dump shows `SHAREDtype` links on the two branches,
not a `TYPED` wrapper. The final Pair constructor arguments reference those
same two original local definitions. Exact observed TASTy addresses and
UUID links are recorded in the manifest and asserted by
`scala39_conditional_reference_selection_oracle_records_if_tasty_and_jvm_join`.

The swapped-branch Scala control is valid. Its separate Java run observed
`true=7/42` and `false=42/3`, with distinct Pair fields. Renamed locals and
reversed declaration order also compile. The four-argument target, control
sources, and probe are independently hashed in the manifest.

## Rust producer contract

One real lexer → parser → namer → source typer session authenticates the
source-to-typed identities. The candidate is accepted only when its four
ordered formals have the `(Int, Int, Boolean, Int)` ABI, its first two locals
are the original `Counter(seed)` allocations, its third immutable local is
the exact `if` selecting those two identities, and its only setter mutates
that selected local with the final `Int` formal. The final Pair constructor
still receives the first two locals in source order. The proof carries source
and typed tree roots, canonical symbols, scope, and JVM local slots; spelling
is not used to recover identity.

The paired emitter returns exactly three class files and three TASTy sidecars.
The regression verifies class-file major version 65, TASTy 28.9, matching
class/sidecar UUIDs, the four-parameter descriptor, local limits, and
byte-for-byte determinism across fresh sessions. The unchanged Counter and
Pair outputs are byte-equal to the established local-reference-alias route.
The contract also checks encoded conditional branch targets and the selected
receiver slot, rejecting verifier-valid mutations that load another Counter
local.

## Controls and preserved paths

Scala validity and producer support are recorded separately. Valid cases
outside this receipt shape remain rejected with the exact Rust diagnostic.

| Control | Scala 3.9 result | Rust producer result |
| --- | --- | --- |
| Swap IF branch targets | Valid; reversed runtime values | Accepted with exact branch-to-local mapping |
| Rename locals | Valid | Accepted; canonical symbols retain identity |
| Reverse the two original Counter declarations | Valid | Accepted; Pair arguments preserve source order |
| Non-Boolean IF condition | Invalid, E007 | `IfConditionTypeMismatch` during method body typing |
| Counter/Int branch mismatch | Invalid, E007 | `LocalValueTypeMismatch` during method body typing |
| Missing local reference | Invalid, E006 | `TermNameNotFound` during method body typing |
| Reference before initialization | Invalid, E039 | `TermNameNotFound` during method body typing |
| Mutable selected local | Valid, outside this receipt | Rejected at local identity validation |
| Nested IF | Valid, outside this receipt | Rejected because branches are not direct local references |
| Counter allocation inside one branch | Valid, outside this receipt | Rejected because branches are not direct local references |
| Additional independent setter | Valid, outside this receipt | Rejected by the exact block-shape gate |

The existing immutable alias fixture and zero-, one-, and two-call no-alias
fixtures remain covered by focused producer regressions. Their Scala sources
also compile with the same pinned compiler.

## Ownership boundary

- `dotty-core` source/typed indexes and the source typer own canonical
  declaration, branch, and receiver identity. The paired proof preserves
  those bindings without changing frontend semantics.
- `dotty-jvm` owns lowering already typed reference joins to JVM branch frames.
  The paired contract authenticates emitted branch targets against the same
  canonical local slots.
- `dotty-tasty-pickler` owns semantic TASTy plan validation and serialization.
  It serializes the original IF and local references from the validated
  session plan.
- The cross-class proof and paired producer own the source-to-artifact
  acceptance contract, including conditional selection and encoded linkage.

## Reproduction

Use a checkout of the pinned Scala source revision and build it in a copy in
managed scratch so sbt, Java, and compiler temporary output stay there. With
SDKMAN initialized and Temurin JDK 21 selected, the build command is:

```sh
sbt -batch 'dist-linux-x86_64/universal:stage'
```

The compile, consumer, disassembly, TASTy print/decompile, and `-from-tasty`
commands used for the receipt are listed in `oracle.json`. Run each control
with `-Werror -release 21` against the target's compiled Counter/Pair classes.
The Rust receipt is reproduced with:

```sh
cargo +1.98.1 test -p dotty --test jvm_tasty_conditional_reference_selection --locked
cargo +1.98.1 test -p dotty --test jvm_tasty_local_receiver_call conditional_reference_selection --locked -- --nocapture --test-threads=1
```

The implementation-specific producer and proof regressions are in
`jvm_tasty_local_receiver_call` and
`jvm_tasty_conditional_local_reference_proof`. The repository's required
workspace and JDK checks remain the final validation gates for this change.

## Independent consumers of the original Rust batch (#2271)

The Rust runtime proof uses the canonical producer source name `PairFactory.scala`
and the source fixture SHA-256
`c77b2b3e00bf602b86be30b29ceaa2300398feb34e80d2f76c37f391bb4d654f`. It emits
two independent fresh sessions and compares the ordered six-file paths, complete
bytes, and all three class-to-TASTy UUID links. The same canonical
`PairFactory.tasty` bytes are passed to the pinned Scala 3.9 reader, while the
independent Java client loads all six Rust outputs without compiling the
producer source.

The independent Java client is
[`RustRuntimeProbe.java`](../tests/fixtures/jvm_tasty_conditional_reference_selection/RustRuntimeProbe.java),
SHA-256 `97cfe2c30ff7cb0b07627ce83dd2392aaee898c4690bc70b40206200a37d4a1b`.
The Scala support source in the Rust consumer proof has SHA-256
`06266874ed9a7acdfc4d6a1c27bea3967055e474bca5a2f666af5ebfc8669c28`.

| Rust artifact | SHA-256 | Linked TASTy UUID |
| --- | --- | --- |
| `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008071c3c00968ac0000000000008193` |
| `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | `008071c3c00968ac0000000000008193` |
| `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d7441c7f00000000000008198` |
| `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | `000b7d8d7441c7f00000000000008198` |
| `example/PairFactory.class` | `351d311b48edb38d5b86b2621cb49568df508c944929633e4bdef35ea03d159c` | `00d07c03e8e3c6dc00000000000081a1` |
| `example/PairFactory.tasty` | `327e42e222d13f21fe8fbe7f7b8ddc769abf31a85c1d97d9b6115ed5d39efe5e` | `00d07c03e8e3c6dc00000000000081a1` |

Under `javap -v -c -p`, Rust `PairFactory.create` has descriptor
`(IIZI)Lexample/Pair;`, classfile major 65, and two full StackMapTable frames.
Its `ifeq` at offset 21 targets the second local load at 29; the fallthrough
loads the first local at 24. Both paths merge into selected slot 7 at offset
31, then call `Counter.set(I)V` once. Pair construction loads original slots 5
and 6 in order. `java -Xverify:all -ea` proves both choices, signed and equal
seeds, fresh repeated allocations, distinct Pair fields, and independent
mutations of the original fields.

Negative copies distinguish failure stages: verifier-valid flipped-branch and
wrong-selected-slot classfiles fail the paired artifact contract and the Java
runtime assertions; an invalid branch target and a selected read before its
initialization fail JVM verification. Corruptions remain in isolated runtime
copies, and the six canonical originals remain unchanged. The strict Java and
Scala consumer checks are part of the CI JDK target and the pinned Scala
conditional proof target, respectively; both must run on the final PR head.

## Independent Scala 3.9 consumer of the Rust batch (#2272)

[`ScalaConsumer.scala`](../tests/fixtures/jvm_tasty_conditional_reference_selection/ScalaConsumer.scala)
is separately authored and has SHA-256
`d4668a27c5c660debd71d94a7ce117edec82b14c070dbae70c9edeb764df15c1`.
The Rust test emits the unchanged `PairFactory.scala` source (SHA-256
`c77b2b3e00bf602b86be30b29ceaa2300398feb34e80d2f76c37f391bb4d654f`) into a
fresh six-artifact batch, verifies its original hashes and UUIDs, and compiles
the client with the batch plus only the two pinned Scala runtime jars on its
classpath. The output directory is separate and contains no fallback
`Counter`, `Pair`, or `PairFactory` class. The client names and resolves all
three classes directly, calls `PairFactory.create` with both Boolean values,
negative seeds, and equal seeds, then mutates directly constructed Counter
instances to check Pair's original field references.

The test used Scala source revision
`777528f19a58e794c9954a42f433373472ec57f8`, whose staged compiler reported
`3.9.0-bin-SNAPSHOT-git-777528f`; the observed `with_compiler.jar` SHA-256 was
`e3e5db272736ffeb22a444c494bd44982fb6836da816cac61844721d639760d8`. It used
Eclipse Temurin JDK `21.0.13+9`, with `java` SHA-256
`9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`. The
published Scala 3.9 runtime jars are checked against their hashes in
[`scala39_toolchain.rs`](../tests/support/scala39_toolchain.rs). Runtime output
was `scala39-rust-conditional-reference-consumer=ok` under
`java -Xverify:all -ea`.

| Original Rust artifact | SHA-256 | Linked TASTy UUID |
| --- | --- | --- |
| `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008071c3c00968ac0000000000008193` |
| `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | `008071c3c00968ac0000000000008193` |
| `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d7441c7f00000000000008198` |
| `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | `000b7d8d7441c7f00000000000008198` |
| `example/PairFactory.class` | `351d311b48edb38d5b86b2621cb49568df508c944929633e4bdef35ea03d159c` | `00d07c03e8e3c6dc00000000000081a1` |
| `example/PairFactory.tasty` | `327e42e222d13f21fe8fbe7f7b8ddc769abf31a85c1d97d9b6115ed5d39efe5e` | `00d07c03e8e3c6dc00000000000081a1` |

On the original `PairFactory.tasty`, official `-print-tasty` reported
`chooseFirst: Boolean` at AST address 52; `first`, `second`, and `selected`
VALDEFs at 92, 117, and 140; and the selected IF at 149. Its condition
referenced formal 52, and its THEN/ELSE shared references resolved through
addresses 88/90 to the original first/second VALDEFs at 92/117. The signed
`set(Int): Unit` selection used selected address 140 as receiver and formal 61
as argument. The final signed Pair constructor used the original 92/117 locals
in order. Scala `-decompile` rendered the same body, and `-from-tasty` accepted
and reconstructed all three original sidecars into a separate directory. The
JVM descriptor was independently decoded as `(IIZI)Lexample/Pair;`.

| Isolated negative control | First observed failure |
| --- | --- |
| Missing `PairFactory.tasty` | Scala source compilation rejects the classfile because its TASTy sidecar is missing. |
| Wrong sidecar UUID | Scala source compilation reports the classfile and sidecar are out of sync. |
| Changed JVM descriptor with original TASTy | Scala compilation follows the TASTy signature; JVM execution first fails with `NoSuchMethodError`. |
| Stale IF arm reference | Scala `-decompile` cannot unpickle the factory body. |
| Selected local receiver reference changed to a non-node address | Scala `-decompile` cannot unpickle the factory body. |
| Forged source-bound setter receiver | The existing `jvm_tasty_conditional_local_reference_proof` test rejects it at the typed-binding stage. |

All corruptions use isolated copies; the original six artifact hashes are
checked again after consumer and control execution. The focused strict target
is `jvm_tasty_conditional_reference_scala_consumer`; it is assigned to shard 4
while the previous forged-receiver proof remains required. Reproduce the Scala
consumer and controls with the pinned source checkout, JDK, and runtime cache:

```sh
DOTTY_REQUIRE_SCALA_TASTY_CONDITIONAL_REFERENCE_SCALA_CONSUMER=1 \
SCALA39_ROOT=/path/to/scala39-source \
COURSIER_MAVEN_ROOT=/path/to/repo1.maven.org/maven2 \
cargo +1.98.1 test --test jvm_tasty_conditional_reference_scala_consumer \
  --locked -- --nocapture
```

## Integrated acceptance ledger (#2273)

This final receipt freezes the reviewed prerequisites at integration base
`ed65fc4d5fdad078afcee4d15a2d25337636e9b7`. Each row records the reviewed PR
head, its merge commit in that integration base, and the published ready review
and required CI runs. All eight merge commits are ancestors of the integration
base. Review links identify the exact reviewed heads; the Rust run includes the
workspace checks and mandatory JDK gate, while the Scala run includes all four
strict shards and the required pinned-consumer aggregate.

| Issue / PR | Reviewed PR head | Merge commit | Review | Rust + JDK run | Strict Scala run |
| --- | --- | --- | --- | --- | --- |
| #2265 / [#2275](https://github.com/scytrowski/dotty-rs/pull/2275) | `b2fd6bb09765d041fdf11d9b209179dc218a9127` | `96c26b9b9a968b12851b04ef5ecde16092e93016` | [ready](https://github.com/scytrowski/dotty-rs/pull/2275#pullrequestreview-5872725313) | [success](https://github.com/scytrowski/dotty-rs/actions/runs/43198423955) | [success](https://github.com/scytrowski/dotty-rs/actions/runs/43198424052) |
| #2266 / [#2277](https://github.com/scytrowski/dotty-rs/pull/2277) | `c46749aebcbbde6f09d1867dfa16d8622a281735` | `0ad5b6e39f927c49ef2784530765f82b040678d0` | [ready](https://github.com/scytrowski/dotty-rs/pull/2277#pullrequestreview-5874070129) | [success](https://github.com/scytrowski/dotty-rs/actions/runs/43212843498) | [success](https://github.com/scytrowski/dotty-rs/actions/runs/43212843474) |
| #2267 / [#2279](https://github.com/scytrowski/dotty-rs/pull/2279) | `7d5a7243e1c3f9322397dbc6f5c49b0e71b19dc4` | `e19a7dac62b90920d1c59c5e7f78ae184ce5974c` | [ready](https://github.com/scytrowski/dotty-rs/pull/2279#pullrequestreview-5877546727) | [success](https://github.com/scytrowski/dotty-rs/actions/runs/43251545383) | [success](https://github.com/scytrowski/dotty-rs/actions/runs/43251545332) |
| #2268 / [#2281](https://github.com/scytrowski/dotty-rs/pull/2281) | `a6f5e747cf98167f01e9b985ed6e7dc710cf00fb` | `88152ded31393cb08252305014c6d22ee49a07d1` | [ready](https://github.com/scytrowski/dotty-rs/pull/2281#pullrequestreview-5879711882) | [success](https://github.com/scytrowski/dotty-rs/actions/runs/43277196605) | [success](https://github.com/scytrowski/dotty-rs/actions/runs/43277196602) |
| #2269 / [#2283](https://github.com/scytrowski/dotty-rs/pull/2283) | `9f3848822a9dd11e7d7cec0ca3cba31abbd6ca31` | `b566b84eb5e3df5c23fd1f345a9273d967ee4274` | [ready](https://github.com/scytrowski/dotty-rs/pull/2283#pullrequestreview-5882810230) | [success](https://github.com/scytrowski/dotty-rs/actions/runs/43315676365) | [success](https://github.com/scytrowski/dotty-rs/actions/runs/43315676326) |
| #2270 / [#2285](https://github.com/scytrowski/dotty-rs/pull/2285) | `643b8f115a3ffdd42ad281f621362aa2214cabf5` | `75c4b94a38e8dfedfdae5580d26d66024dc758bc` | [ready](https://github.com/scytrowski/dotty-rs/pull/2285#pullrequestreview-5892495344) | [success](https://github.com/scytrowski/dotty-rs/actions/runs/43439850564) | [success](https://github.com/scytrowski/dotty-rs/actions/runs/43439850596) |
| #2271 / [#2286](https://github.com/scytrowski/dotty-rs/pull/2286) | `ab4d3909d651d310ae49da14671b73fd7b2c2fbb` | `58e355f2b390bc9da19fb78458026a5c89e2467b` | [ready](https://github.com/scytrowski/dotty-rs/pull/2286#pullrequestreview-5894644253) | [success](https://github.com/scytrowski/dotty-rs/actions/runs/43463232221) | [success](https://github.com/scytrowski/dotty-rs/actions/runs/43463233399) |
| #2272 / [#2293](https://github.com/scytrowski/dotty-rs/pull/2293) | `24c46dd8906e4c43cfaabe3d219db6a28ca80be2` | `ed65fc4d5fdad078afcee4d15a2d25337636e9b7` | [ready](https://github.com/scytrowski/dotty-rs/pull/2293#pullrequestreview-5897947648) | [success](https://github.com/scytrowski/dotty-rs/actions/runs/43502387361) | [success](https://github.com/scytrowski/dotty-rs/actions/runs/43502387292) |

The frozen producer is the unchanged `PairFactory.scala` source
(`c77b2b3e00bf602b86be30b29ceaa2300398feb34e80d2f76c37f391bb4d654f`). The
integrated strict test emits that source through the ordinary paired producer
into one canonical directory, then runs `javac --release 21`, `javap -v -c
-p`, and the Java 21 runtime probe before the separately authored Scala client.
Both classpaths point to that same directory; all six original hashes are
checked before each consumer and again after each run. The existing frontend
identity proof and producer-plan regression assert the original `SourceId`,
source/typed `TreeId`s, symbols, scope, Boolean formal, IF arms, selected local,
setter receiver, and final Pair arguments. The paired producer validates those
borrowed session bindings before returning its JVM/TASTy artifacts.

The integrated test additionally decodes each emitted class and sidecar,
requires class-file major 65 and TASTy 28.9, verifies all three class/TASTy UUID
pairs, and compares paths, complete bytes, hashes, and UUIDs across fresh Rust
sessions. `javap` observes descriptor `(IIZI)Lexample/Pair;`, two Counter
allocations, the conditional branch and merge, one `Counter.set(I)V` call, and
the two original Pair argument loads. The exact Rust output hashes and UUIDs
remain the six-entry table above. The Java output is
`conditional-rust-runtime=true:42/3,false:7/42,negative:0/42,-9/0,equal:42/7,identity=original-distinct,fresh=true`;
the official Scala client prints
`scala39-rust-conditional-reference-consumer=ok`. Both ran with
`-Xverify:all -ea` / the official pinned compiler against the same immutable
batch. The TASTy reader's IF/VALDEF/TERMREF and `-from-tasty` evidence is the
official output recorded above.

### Negative and failure-stage ledger

| Control | First rejecting stage / observed result | Regression |
| --- | --- | --- |
| Missing or forged source/typed IF edges, stale mappings, wrong local/receiver/argument identity | Source-to-typed binding validation; no completed TASTy/batch | `missing_or_forged_source_typed_if_edges_reject_at_the_typed_binding_stage`; `conditional_shape_reaches_the_authenticated_paired_producer_plan` |
| Non-Boolean condition, incompatible branch/result types, missing or uninitialized local | Source typing; exact typed diagnostics (`IfConditionTypeMismatch`, `LocalValueTypeMismatch`, or `TermNameNotFound`) | `frontend_invalid_condition_branch_and_result_inputs_fail_during_typing` |
| Mutable, nested, allocation-in-arm or extra-setter shape | Bounded source-shape proof or exact paired BLOCK contract; rejected before publication | `mutable_or_nested_selections_are_outside_the_receipt_contract`; `conditional_reference_selection_controls_record_scala_and_rust_outcomes` |
| Reversed or forged proof receipts, owner/slot mismatch, verifier-valid wrong branch or selected receiver | Paired artifact contract rejects; no accepted artifact batch | `conditional_reference_selection_controls_record_scala_and_rust_outcomes`; `independent_java21_client_consumes_the_original_conditional_rust_batch` |
| Invalid branch target / malformed or incompatible frame | Class-file decoder/verifier or `java -Xverify:all` rejects before runtime | `independent_java21_client_consumes_the_original_conditional_rust_batch` |
| Missing sidecar / wrong UUID | Scala classpath loading reports missing TASTy / classfile and sidecar “out of sync” | `scala39_compiles_and_runs_against_the_original_rust_conditional_batch` |
| Stale IF arm / invalid selected VALDEF reference | Official Scala `-decompile` rejects the TASTy body as unpicklable | `scala39_compiles_and_runs_against_the_original_rust_conditional_batch` |
| Changed JVM method descriptor with original TASTy | Scala client compiles from TASTy; JVM linkage fails first with `NoSuchMethodError` | `scala39_compiles_and_runs_against_the_original_rust_conditional_batch` |
| Failure during any of six writes or directory promotion | Publisher returns the injected stage error; prior sentinel state remains and no partial batch is accepted | `conditional_six_artifact_publication_is_atomic_under_staging_and_promotion_failures` |

The canonical batch passed two-session byte equality and legacy alias/no-alias
regressions unchanged. The earlier alias receipt remains intact in
[`jvm-tasty-local-reference-alias.md`](jvm-tasty-local-reference-alias.md),
including the independent receipt-only #2246 successor at integration base
`b7ad6d2599183864166f1e72ebe9ab50893c79cf`. Its reviewed executable head was
`5a258460a352516b9201a991e7fe09a69ec19bad`; its final receipt head was
`7a3ce14e6328331c5d4f0da7da80fee914a45f12`. Review
[#2263](https://github.com/scytrowski/dotty-rs/pull/2263#pullrequestreview-5861720409)
records successful exact-head Rust checks, mandatory JDK, four strict Scala
shards and aggregate, with run/job links; no earlier alias receipt was
rewritten here.

This evidence is pinned to Rust `1.98.1`, Scala 3.9.0 source revision
`777528f19a58e794c9954a42f433373472ec57f8`, official TASTy `28.9`, and Eclipse
Temurin JDK `21.0.13+9`. The staged compiler reported
`3.9.0-bin-SNAPSHOT-git-777528f`; `with_compiler.jar` SHA-256 is
`e3e5db272736ffeb22a444c494bd44982fb6836da816cac61844721d639760d8`, and the
Java launcher SHA-256 is
`9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`. Source,
Java client, Scala client, and artifact digests are asserted from the fixtures
and support receipt. The current strict TASTy inventory contains 27 targets in
four shards; its validator and shard assignment are checked by
`tools/ci/validate_strict_tasty_targets.py`. The evidence document path is now
in the existing strict workflow filter, so a receipt-only edit invokes the
same four shards and aggregate without enabling the hosted full Scala corpus or
changing a gate.

The only behavior added by this final increment is test orchestration: Java
and Scala now prove independent use of the same physical Rust batch. No
production compiler, pickler, encoder, paired contract, or publisher code is
changed. The required final-head hosted run/job URLs are recorded in the PR's
final verification receipt after the final content head is fixed.

The accepted source shape remains the exact four-formal method with two
initialized immutable Counter locals, one immutable local whose IF directly
references those locals, one selected setter call, and the final ordered Pair
construction. Renamed locals, reversed original declarations, and swapped IF
arms are accepted through canonical symbol bindings. Mutable selected locals,
nested IFs, allocations or side effects inside an arm, unrelated branch types,
and additional statements or setter calls remain deferred and fail closed.
