# Mutable local reference rebinding: Scala 3.9 oracle

This report records issue #2298's narrow fixture audit. The source and Java
probe are in [`tests/fixtures/jvm_tasty_mutable_reference_rebinding`](../tests/fixtures/jvm_tasty_mutable_reference_rebinding).
The six `.class` and `.tasty` files under `scala39/` are outputs from the pinned
Scala compiler, not Rust artifacts. The adjacent `oracle.json` records hashes,
commands, and observed control results.

The source keeps `Counter` and `Pair` declaration shapes fixed. In
`PairFactory.create`, it allocates two counters, initializes mutable `selected`
from `first`, reassigns it to `second`, calls `selected.set(nextValue)`, then
constructs `Pair(first, second)`.

```scala
def create(firstSeed: Int, secondSeed: Int, nextValue: Int): Pair =
  val first: Counter = new Counter(firstSeed)
  val second: Counter = new Counter(secondSeed)
  var selected: Counter = first
  selected = second
  selected.set(nextValue)
  new Pair(first, second)
```

## Pinned Scala and JVM evidence

The Scala source revision is
`777528f19a58e794c9954a42f433373472ec57f8` (Scala 3.9.0, TASTy 28.9.0),
compiled with sbt 1.12.1 and Eclipse Temurin JDK 21.0.13+9. The compiler emits
class-file major version 65 and the `create` descriptor
`(III)Lexample/Pair;`. The method allocates exactly two `Counter` instances.
Its local slots are `first=4`, `second=5`, and `selected=6`. Bytecode stores
first and then second into slot 6; the setter loads slot 6 once. Pair
construction loads slots 4 and 5, preserving both original references.

The PairFactory TASTy block begins at address 65. Its statements are `first`
at 83, `second` at 106, mutable `selected` at 127, assignment at 137, and the
setter call at 144; the block result constructs Pair at 67. The assignment
left side is a direct reference to `selected` at 127. Its right side is a
shared reference resolving to the `second` local at 106. The initializer for
`selected` resolves to `first` at 83. The final constructor arguments resolve
to `first` and `second` in that order.

The Java 21 probe runs under `java -Xverify:all -ea`. It verifies that all
three calls preserve distinct Pair fields and produce `(left,right)` values
`(7,42)`, `(-9,11)`, and `(5,-13)`. Equal seeds still produce distinct
objects, so value equality cannot substitute for reference identity.

## Rust paired producer integration

The fixture passes through the real contextual scanner, parser, namer, and
source typer. The regression follows canonical symbols through source-to-typed
bindings: the two constructor arguments refer to the respective seed formals;
the mutable local's initializer refers to `first`; the typed assignment maps
`selected` to `second`; the setter receiver maps back to `selected` and its
argument to `nextValue`; the Pair call still receives `first` and `second`.
The paired regression emits exactly `Counter`, `Pair`, and `PairFactory` class
files with their TASTy sidecars from that one source session. It decodes every
class as major 65, every sidecar as TASTy 28.9.0, verifies the real UUID links,
and compares ordered paths, bytes, and UUIDs across two independent sessions.
The test also exercises all six staged-file fault points and both promotion
fault points, asserting that no failed destination is visible.

At the merged base, the paired planning callback was not reached. The recorded
first failure was
`SourceShape("candidate method must have one plain formal clause and a concrete public body")`
in the cross-class constructor-call identity proof, before any artifact
publication. The integrated route now admits only the original mutable
declaration/assignment/setter receipts and the five-statement
`ValDef, ValDef, ValDef, Assign, Apply` shape. The classifier keeps that route
separate from legacy alias and ordered-call shapes. Renamed locals and reversed
declarations remain accepted when their canonical identities preserve the
same contract; assigning `selected` back to `first`, duplicate writes,
allocation right sides, and nested branch/loop shapes remain rejected.

The frontend owns canonical local and assignment identities; `dotty-jvm`
owns lowering the initialized mutable reference local and assignment; the
paired TASTy planner and artifact contract join those identities to the
complete TASTy block and validate the emitted instructions before the batch is
accepted. No source retyping, synthetic AST, new writer, CLI, or standalone
publisher is involved.

## Independent Java 21 client for Rust output

Issue #2309 adds a separately authored client at
[`tests/fixtures/jvm_tasty_mutable_reference_rebinding/RustRuntimeProbe.java`](../tests/fixtures/jvm_tasty_mutable_reference_rebinding/RustRuntimeProbe.java)
(SHA-256 `e0679bf4a786026af8dbefa4fbbbe33fc253749d81d81502cf7fcc66ba4bd238`).
The focused `jvm_tasty_artifacts` test emits the unchanged one-source batch
from one Rust frontend session, publishes the six files to a fresh isolated
class directory, compiles only this Java client with `javac --release 21`,
and runs it against that directory with `java -Xverify:all -ea`. The oracle
classes under `scala39/` are not on this test's classpath. It records the
ordered paths, hashes and shared class/TASTy link UUIDs before compilation,
then checks the same bytes and hashes after the positive run and all isolated
negative copies.

The Rust output receipt from the pinned workspace test is:

| Ordered artifact path | SHA-256 | Class/TASTy UUID |
| --- | --- | --- |
| `example/Counter.class` | `b439e752205735537790a02963f67602cc0ff233d40ca2ef22f542064bf7e0b9` | `009ce3b8-b80c-cbac-0000-000000008193` |
| `example/Counter.tasty` | `41510454f72931da9915aa0d4316fab0f163c9c5a134ecd5a3a735c5946a030a` | `009ce3b8-b80c-cbac-0000-000000008193` |
| `example/Pair.class` | `f38e139eccf8bcd6a68c448898410f5ad9dee51856a53d65685b6b71393a4e02` | `00bced30-fb29-91f0-0000-000000008198` |
| `example/Pair.tasty` | `3b016c54db4e3e52d9399a0ac0fe8cd26ea5d78031de2ac4c5bd0865336a2605` | `00bced30-fb29-91f0-0000-000000008198` |
| `example/PairFactory.class` | `63d3525870cb42013171712825cc471f6f09f5bf153dd17793373441af215f6f` | `005eb921-38c3-47d2-0000-00000000819f` |
| `example/PairFactory.tasty` | `8335b18001bdf9dd0ae02e62737c10f54cf96885594a7b121e00be900faf14fb` | `005eb921-38c3-47d2-0000-00000000819f` |

`javap -v -c -p` and the repository classfile decoder inspect the actual Rust
`PairFactory.class`: class major 65; `create` descriptor
`(III)Lexample/Pair;`; `max_stack=4`, `max_locals=7`; two Counter allocations;
stores of `first` then `second` into selected slot 6; one `set(I)V` call with
slot 6 as receiver; and Pair arguments loaded from original slots 4 and 5.
The test reconstructs the decoded instruction sequence with the actual
constant-pool operands and validates its typed CFG and stack/local bounds.
This straight-line method has no `StackMapTable` attribute; Java 21 accepts
and verifies the class with `-Xverify:all`, and the decoder/CFG checks validate
the emitted code and bounds without asserting a frame table that is absent.

The independent probe checks `(7,3,42)`, `(-9,11,42)`, and `(5,5,-13)`.
It reads Pair's supported public values and independently checks that the two
reflected Pair fields hold distinct Counter objects. Mutating each held object
after construction changes only its corresponding Pair value. The positive
output is `rust mutable rebinding: signed/equal seeds, original identities
and independent mutation passed`.

Six isolated classpath copies exercise negatives. Rebinding selected to the
first local, calling the setter on the first local, and reversing Pair's
constructor arguments all remain JVM-verifier-valid and are rejected by the
Java semantic witness. A descriptor changed from `I` to `Z` reaches
`NoSuchMethodError`, a zeroed method stack bound reaches `VerifyError`, and a
missing Counter class reaches `NoClassDefFoundError`. The existing paired
contract corruption test additionally checks TASTy UUID mismatch and rejects
verifier-valid wrong-slot, receiver, and Pair-argument edits before batch
acceptance. No negative copy is used for the positive run.

Run the focused consumer test with pinned Rust 1.98.1 and Temurin JDK 21.0.13+9:

```sh
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo +1.98.1 test --test jvm_tasty_artifacts \
  mutable_reference_rebinding_java_21_consumes_original_rust_artifacts \
  --locked -- --nocapture
```

The test uses the registered scratch directory through `TMPDIR` and the
task-specific `CARGO_TARGET_DIR`; it does not remove or rebuild shared caches.

## Controls

The pinned Scala compiler accepts renamed locals, reversed declarations,
assignment back to `first`, duplicate assignment, allocation on the assignment
right side, nested conditional assignment, and nested loop assignment. The
paired producer accepts only renamed and reversed variants that preserve the
exact selected-to-second contract; the other valid source controls remain
outside this bounded route. The Rust regression separately rejects an
immutable write, unknown or forward assignment target, uninitialized local,
and assignment of `Int` to `Counter`. The Scala and Rust outcomes are recorded
separately in the tests and `oracle.json`.

## Strict Scala gate status

The pinned Scala source was staged with `sbt "dist/universal:stage"` using
sbt 1.12.1 and Eclipse Temurin JDK 21.0.13+9. `scalac -version` reported
`3.9.0-bin-SNAPSHOT-git-777528f`; the observed generated
`with_compiler.jar` SHA-256 was
`0ef4231a0cceac40a1bb9424f9a7aba90714ef691e45167985daafbe7393891f`.
As with other local source builds, this generated JAR hash is provenance, not
a stable identifier for the pinned Scala revision.

With `SCALA39_ROOT` pointing at the staged checkout,
`DOTTY_REQUIRE_SCALA_TASTY_MUTABLE_COUNTER_FRAGMENT=1`, the pinned runtime
artifacts from the approved Maven cache, and Temurin 21.0.13+9, the strict
`jvm_tasty_mutable_local_reference_reassignment_proof` consumer passed all
8 tests. In addition to the declaration and assignment fragments, it passes
the actual complete `Counter`, `Pair`, and `PairFactory` class/TASTy artifacts
emitted by the Rust one-session paired route to the pinned Scala reader. The
reader accepts `PairFactory.tasty` with TASTy 28.9.0; `-print-tasty` identities
are checked for the ordered `first`, `second`, and mutable `selected` VALDEFs,
the `selected = second` ASSIGN, the `selected.set(nextValue)` receiver and
argument, and the final `Pair(first, second)` result. `-decompile` independently
checks the complete source statement order.

The focused semantic corruption matrix is run by
`cargo +1.98.1 test --test jvm_tasty_artifacts mutable_reference_rebinding_contract_rejects_semantic_corruption --locked`.
It verifies first rejection and absence of a validated or published batch for
same-session `SourceTypedBindings` faults that remove the assignment RHS mapping
or forge the assignment receipt. Those faults retain the original source and
typed trees, build the complete PairFactory class-state plan, and are rejected
by `validate_jvm_tasty_batch` at class 2's TASTy body validation stage. The
artifact cases verify rejection for wrong JVM store/load slots, a
verifier-valid wrong setter receiver, swapped Pair arguments, malformed
verifier stack/local bounds, and swapped TASTy UUID links. The well-formed
TASTy mutation changes the actual ASSIGN LHS from `selected` to the `second`
VALDEF, leaves its RHS at `second`, reparses the complete file, and is rejected
at `TASTy plan identity`. Extra setter effects remain covered by a separate
unsupported-source control. The full `jvm_tasty_artifacts` target passed all
69 tests. The prior attempted run without a staged compiler stopped at
`scala39_toolchain` preflight before compilation. The pinned Rust gates and
JDK 21 verifier passed as recorded above.

## Initial toolchain blocker

The first pinned compiler launch failed before Scala compilation because sbt's
default Unix-domain socket path exceeded the platform socket path limit under
the long managed scratch directory. Setting sbt's supported
`XDG_RUNTIME_DIR` to a short alias located inside that same registered scratch
resolved the path-length failure. The sandbox then reported `EPERM` while
binding the Unix socket; the same bounded build command completed under the
host-approved execution. The Scala source revision, compiler dependencies,
JDK 21, and Rust 1.98.1 pins were unchanged. This host setup issue is resolved.
