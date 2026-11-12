# Conditional local reference selection: Scala 3.9 audit

This report freezes the one-source `Counter` / `Pair` / `PairFactory` input
from issue #2265 and records the pinned Scala 3.9.0/JDK 21 oracle and current
Rust first failure. The Scala `.class` and `.tasty` files in
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

## Current Rust first failure

One real lexer → parser → namer → source typer session authenticates the
source-to-typed identities. With source `SourceId(22650)`, `create` is
`SymbolId(60)` and its formals are `61`–`64`. The two allocated local
bindings are `SymbolId(65)` and `SymbolId(66)` in `ScopeId(18)`; `selected`
is `SymbolId(67)` in the same method-owned scope. The source-to-typed receipt
records both Counter declarations, selected declaration, setter, IF condition
and branches, and final Pair application. The IF branches retain symbols 65
and 66, the setter receiver is 67, and the Pair arguments remain 65 and 66.
These identities are read from the shared session indexes and typed
references, not inferred from local names.

The current paired producer's first failure is **before the body shape is
examined**:

```text
phase: cross-class constructor-call identity proof
SourceShape("candidate method must have one plain formal clause and a concrete public body")
```

The failure is caused by the current candidate-method contract rejecting this
four-formal method shape. It does not measure a later paired proof for a
conditional local join. The callback is not reached, `emit_paired_jvm_tasty_batch`
returns an error, and no accepted artifact batch or partial artifacts are
returned. No compiler support claim for the conditional body follows from this
earlier rejection.

## Controls and preserved paths

Scala validity and Rust first failure are recorded separately; valid deferred
inputs are not treated as invalid Scala. The focused Rust regression asserts
the exact Rust diagnostic for each measured case.

| Control | Scala 3.9 result | Current Rust first result |
| --- | --- | --- |
| Swap IF branch targets | Valid; reversed runtime values | Candidate-method shape `SourceShape` |
| Rename locals | Valid | Candidate-method shape `SourceShape` |
| Reverse the two original Counter declarations | Valid | Candidate-method shape `SourceShape` |
| Non-Boolean IF condition | Invalid, E007 | `IfConditionTypeMismatch` during method body typing |
| Counter/Int branch mismatch | Invalid, E007 | `LocalValueTypeMismatch` during method body typing |
| Missing local reference | Invalid, E006 | `TermNameNotFound` during method body typing |
| Reference before initialization | Invalid, E039 | `TermNameNotFound` during method body typing |
| Mutable selected local | Valid, deferred | Candidate-method shape `SourceShape` |
| Nested IF | Valid, deferred | Candidate-method shape `SourceShape` |
| Counter allocation inside one branch | Valid, deferred | Candidate-method shape `SourceShape` |
| Additional independent setter | Valid, deferred | Candidate-method shape `SourceShape` |

The existing pre-sprint immutable alias fixture and zero-, one-, and two-call
no-alias fixtures still pass their focused Rust producer regression. Their
Scala sources also compile with the same pinned compiler. This audit changes
no accepted language shape or producer behavior.

## Ownership boundary

- `dotty-core` source/typed indexes and the source typer own canonical
  declaration, branch, and receiver identity. The audit authenticates those
  bindings without changing frontend semantics.
- `dotty-jvm` owns lowering already typed reference joins to JVM branch frames.
  Its standalone backend capability does not establish a paired
  JVM/TASTy producer contract.
- `dotty-tasty-pickler` owns semantic TASTy plan validation and serialization.
  This audit records the official IF/VALDEF wire shape and does not change the
  pickler's accepted forms.
- The cross-class proof and paired producer own the source-to-artifact
  acceptance contract. Their current early method-shape rejection is the
  measured result; any future expansion belongs in a separately scoped
  paired-contract/proof increment with its own producer and publication
  regressions.

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

On the final implementation branch, the following local checks also passed
with Rust 1.98.1 and the JDK 21 toolchain on `PATH`:

```text
cargo fmt --all -- --check
python3 tools/check-frontend-boundaries.py
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test --workspace --all-targets --locked
git diff --check
```
