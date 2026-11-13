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
