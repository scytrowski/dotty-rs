# Immutable reference local passed to `Pair`

This document records the single-local constructor audit from issue #2063 and
the paired Rust producer increment from issue #2073. Scala-generated files in
[`tests/fixtures/jvm_tasty_immutable_reference_local_pair/`](../tests/fixtures/jvm_tasty_immutable_reference_local_pair/)
are Scala oracle artifacts; they are not Rust output or Rust/Scala
interoperability evidence. `oracle.json` records each source/artifact hash,
class-file major version, sidecar TASTy UUID, and the matching class-file link.

## Pinned Scala observations

The checked sources were compiled separately with Scala 3.9.0, revision
`777528f19a58e794c9954a42f433373472ec57f8`, and Temurin 21.0.13 using
`-release 21`. Each producer output directory started empty and contains three
`.class` files and three `.tasty` sidecars. The sidecars report TASTy 28.9.0;
the class files are major 65. Independent Scala consumers compiled against
each producer batch and ran with `java -Xverify:all -ea`. They checked the
existing reference's identity in the expected Pair field, a distinct fresh
reference in the other field, independent mutations, and distinct references
from repeated calls.

`javap -v -p -c` reports the primary method descriptor
`(Lexample/Counter;I)Lexample/Pair;`, `stack=4, locals=4`, and slots
`this=0`, `existing=1`, `seed=2`, `fresh=3`. Its instruction order is:

```text
new Counter; dup; iload_2; invokespecial Counter.<init>(I); astore_3;
new Pair; dup; aload_1; aload_3;
invokespecial Pair.<init>(Counter,Counter); areturn
```

The reverse source has the same descriptor, stack/locals and slots; only the
Pair argument loads reverse (`aload_3`, then `aload_1`). The recorded
`PairFactory.javap.txt` files preserve the complete disassembly. The structural
TASTy snapshots preserve raw AST addresses and report the source method's
BLOCK, one `fresh` VALDEF, its direct `new Counter(seed)` initializer, the
outer `new Pair(existing, fresh)` application, and `TERMREFdirect` links.

Reproduction used the cached Scala 3.9 compiler dependencies and this source
checkout at the pinned revision:

```sh
export SCALA39_ROOT=/home/vdrake6/dotty-rs/.worktrees/scala39-source
export JAVA_HOME=/home/vdrake6/.sdkman/candidates/java/current
export COURSIER_MAVEN_ROOT=/home/vdrake6/.codex/worktrees/a2c8/dotty-rs/.issue1876-tmp/mavenrepo
export PATH="$JAVA_HOME/bin:$PATH"
SCALA39_CP=$(find "$COURSIER_MAVEN_ROOT" -type f -name '*.jar' -print | paste -sd:)
OUT=$(mktemp -d)
mkdir -p "$OUT/primary" "$OUT/primary-consumer" "$OUT/reversed" "$OUT/reversed-consumer"
java -cp "$SCALA39_CP" dotty.tools.dotc.Main -release 21 -classpath "$SCALA39_CP" \
  -d "$OUT/primary" tests/fixtures/jvm_tasty_immutable_reference_local_pair/PairFactory.scala
java -cp "$SCALA39_CP" dotty.tools.dotc.Main -release 21 -classpath "$OUT/primary:$SCALA39_CP" \
  -d "$OUT/primary-consumer" tests/fixtures/jvm_tasty_immutable_reference_local_pair/Consumer.scala
java -Xverify:all -ea -classpath "$OUT/primary-consumer:$OUT/primary:$SCALA39_CP" example.Consumer
java -cp "$SCALA39_CP" dotty.tools.dotc.Main -release 21 -classpath "$SCALA39_CP" \
  -d "$OUT/reversed" tests/fixtures/jvm_tasty_immutable_reference_local_pair/PairFactoryReversed.scala
java -cp "$SCALA39_CP" dotty.tools.dotc.Main -release 21 -classpath "$OUT/reversed:$SCALA39_CP" \
  -d "$OUT/reversed-consumer" tests/fixtures/jvm_tasty_immutable_reference_local_pair/ConsumerReversed.scala
java -Xverify:all -ea -classpath "$OUT/reversed-consumer:$OUT/reversed:$SCALA39_CP" example.Consumer
```

## Rust first-stage observations

The focused Rust audit parses, names, types, validates the cross-class proof,
and emits JVM bytes within one callback-scoped session for each source. It
checks constructor descriptors, local slots, and emitted argument-load order.
Both JVM-only batches pass. Source and typed identity evidence is:

| Source | Canonical method / formal IDs | Local ID / scope | Source tree IDs (method, body, VALDEF, initializer, final Apply) | Typed roots for body, VALDEF, initializer, final Apply |
| --- | --- | --- | --- | --- |
| primary (`SourceId(20631)`) | `60 / existing 61 / seed 62` | `63 / ScopeId(18)` | `63, 62, 55, 54, 61` | `26:49, 19:86, 17:43, 25:49` |
| reversed (`SourceId(20633)`) | `60 / existing 61 / seed 62` | `63 / ScopeId(18)` | `63, 62, 55, 54, 61` | `26:49, 19:86, 17:43, 25:49` |

The final column is `TreeId:TypeId`. The original argument leaves map through
their `SourceTypedIndex` edges to the expected formal and local symbols in
source order. The VALDEF maps to typed tree 19, but
`SourceSemanticIndex::symbol_at(source, local_valdef)` is `None`. The existing
`SourceTyper::typed_local_binding_at` path returns its canonical local
`SymbolId(63)` and `ScopeId`; the scope is present, owned by the original
factory method, and resolves the local's interned name to exactly that symbol.
The test asserts original source spans remain on the method, block, VALDEF,
initializer and final Apply, and prints their exact ranges.

Within both sessions, nested source trees are initializer Select/New/seed
Ident `52/51/53`, then Pair Select/New/existing/fresh Idents
`58/57/59/60`. Their typed roots are `15:89/14:88/16:90` and
`22:93/21:92/23:94/24:95`, in the same order. The seed Ident's type is a
TERMREF to the original `seed` formal; the final Idents' TERMREFs resolve to
the original `existing` formal and `fresh` local in the source's argument
order. Each listed source tree has a retained source span, asserted in the
same-session test.

The same session's paired cross-class proof stage rejects the source before a
paired plan reaches the TASTy pickler. Its underlying first error is:

```text
SourceShape("factory body must be the original direct constructor Apply")
```

The batch planner wraps this as `BatchUnit { phase: "cross-class constructor-call
identity proof", ... }`. The paired TASTy validation/pickler stage is therefore
**NOT REACHED**. This does not mask the passing JVM-only result. The tests
cover both positive source orientations and the paired first error. A separate
first-error matrix changes one source property at a time and asserts the
observed Rust failure class. The pinned Scala 3.9.0 compiler accepts
package/class/method/local renames, class declaration reordering, two locals,
a mutable local, a valid local that shadows the formal (`val existing` and
`new Pair(existing, existing)`), a nested result block separated from the
initializer with a semicolon, a conditional initializer, and a
helper-initialized local. Rust reaches the bounded paired-proof shape rejection
for these; this records a gap without enabling new producer behavior. Scala
rejects missing initialization, use before initialization, a wrong local type,
a wrong constructor argument type, wrong constructor arity, a wrong
constructor owner, and a third constructor argument. Rust reports the
corresponding first typing/shape error classes in the test. These source-invalid
cases are controls, not extra acceptance criteria.
Corrupted-identity/bounds/ownership checks remain outside this bounded source
variation matrix unless stated here; no result for them is inferred. The
same-session Rust audit also swaps the two final argument typed edges, swaps
the initializer/result typed roots, and removes the local argument's typed
edge in test-only cloned indexes. All three controls return the exact same
direct-Apply `SourceShape` error as the unmodified source. Thus local identity,
typed-root validation, and downstream ownership/bounds gates are **NOT
REACHED** for this source shape; the controls establish masking order rather
than semantic validation. A stale/foreign local `SymbolId`/`ScopeId` cannot be
passed into this proof path before that shape gate, and no pair-specific
corruption result is claimed for those IDs. Existing helper-local proof tests
exercise their own local-flow contract, not this constructor-pair flow.

| Variation | Pinned Scala 3.9 | Rust first paired-batch result |
| --- | --- | --- |
| package/class/method/local rename | compiles | cross-class proof: original direct constructor Apply shape required |
| class declaration reorder | compiles | cross-class proof: original direct constructor Apply shape required |
| missing initializer | rejects | method body typing: `LocalValueRightHandSideMissing` |
| two immutable locals | compiles | cross-class proof: original direct constructor Apply shape required |
| mutable local | compiles | cross-class proof: original direct constructor Apply shape required |
| shadowed formal name, with both arguments using the local | compiles | cross-class proof: original direct constructor Apply shape required |
| use before initialization | rejects | method body typing: `TermNameNotFound` |
| wrong local type | rejects | method body typing: `TypeNameNotFound` |
| wrong constructor argument type | rejects | method body typing: `ApplicationArgumentTypeMismatch` |
| wrong constructor arity | rejects | method body typing: `ApplicationArityMismatch` |
| nested result block, separated from initializer by `;` | compiles | cross-class proof: original direct constructor Apply shape required |
| conditional initializer | compiles | cross-class proof: original direct constructor Apply shape required |
| helper initializer | compiles | cross-class proof: original direct constructor Apply shape required |
| wrong constructor owner | rejects | cross-class proof: original direct constructor Apply shape required |
| third constructor argument | rejects | method body typing: `ApplicationArityMismatch` |

The Rust result is the first returned stage/error only; it does not imply
downstream plan, TASTy, or class-file validation ran. The source-invalid cases
were compiled with the pinned Scala 3.9.0/JDK 21 toolchain and fresh temporary
output directories; compiler rejection was observed for each.

Artifact-link controls use the frozen Scala primary `PairFactory.class` and
`PairFactory.tasty`: the intact UUID pair passes, while a missing class-file
`TASTY` attribute, a zeroed stale UUID, and a truncated TASTy header each fail
with their corresponding typed contract error. This validates the existing
class-file/TASTy link checker against the issue's real oracle artifact; it does
not claim the paired Rust TASTy producer ran.

## Proof-only direct-New local receipt

Issue #2065 adds `validate_immutable_local_initialization_proof` beside the
existing cross-class proof boundary. It consumes a `TypedLocalBinding` captured
from `typed_local_binding_at` in the original Typer session and returns an
opaque, borrowed receipt for the original Block, ValDef, explicit type,
`new Counter(seed)` initializer, final local Ident, selected Counter primary
constructor, and canonical IDs/types. It checks the one-entry method-owned
local scope, immutable flag, exact source-to-typed edges and positions, the
two-formal `(Counter, Int)` method signature, Counter and Pair constructor
signatures, and the final Pair application in either argument order. Renamed
classes/method/local and reordered class declarations pass through canonical
identities without fixed-spelling dispatch.

The receipt is not an ordered constructor argument origin and is not consumed
by `ConstructorArgumentProof`, TASTy planning, or either producer. The earlier
helper-initialized `LocalValConstructorArgumentProof` retains its
`PlannedCallProof` requirement and first failures. Primary and reversed direct
initializer sessions prove the receipt independently; test-only corruption of
the original initializer/use mapping and seed typed edge is rejected at the
corresponding typed-binding gate. This proof does not claim validation of
conditional, nested, helper, mutable, shadowed, or multi-local initializers;
those remain unsupported. No class-file or TASTy output bytes change.

## Issue #2068 ordered formal/local proof

The follow-up proof reuses the opaque initializer receipt above for exactly
one `val` in a method-owned Block. It records that receipt as a distinct
ordered LocalVal origin beside the existing Formal and NestedNew origins; it
does not reinterpret the helper-local `LocalValConstructorArgumentProof` or
change the helper's `PlannedCallProof` contract. The ordered proof kind is
`FormalLocalPair`, with each argument view retaining its actual Pair parameter
position.

The focused proof test runs the primary and reversed source fixtures through
one original parser/namer/typer session each. Both prove the same
`(Counter, Int) => Pair` method and `Pair(Counter, Counter)` constructor
signatures. The receipt keeps the original Block/declaration/initializer/use
source and typed roots, local symbol and scope, and selected Counter
constructor. In the reversed case the local occupies Pair argument zero; the
formal occupies argument one. The seed remains only the initializer input.
The proof API does not authorize a paired plan or change JVM/TASTy bytes.

The earlier Rust first-error table is historical evidence from before #2068:
its valid Block cases reached the direct-Apply shape rejection. Current focused
positive coverage accepts only the one-local direct-New initializer with one
other original Counter formal. Multi-local, mutable, shadowed, helper,
conditional, nested-initializer, branch and other origins remain unsupported.

The updated cross-contract first-error matrix records the now-reached boundary:

| Control | First current result |
| --- | --- |
| package/class/method/local rename | ordered proof succeeds; paired TASTy planning rejects `BodyExpression` |
| class declaration reorder | ordered proof succeeds; paired TASTy planning rejects `BodyExpression` |
| multiple local declarations | `SourceShape`: one-local Block required |
| mutable local | `SourceIdentity`: original immutable method-owned local required |
| local shadows a method formal | `SourceIdentity`: local source name cannot shadow a formal |
| nested final result Block | `SourceShape`: final Block expression must be the Pair constructor Apply |
| conditional initializer | `SourceShape`: local initializer must be the direct constructor Apply |
| helper initializer | `SourceShape`: initializer function must be the constructor Select |
| wrong constructor owner | `TypedBinding`: final Apply must retain the method's canonical Pair result type |

The two positive rows record the proof-only status at the end of #2068. Issue
#2069 added paired TASTy body serialization, and issue #2073 completes the
bounded paired JVM/TASTy artifact path as recorded below.

## Paired Rust producer evidence for issue #2073

The paired planner now projects the exact source-typed local binding from the
`FormalLocalPair` proof into the existing `JvmLocalBinding` and checked local
layout. The whole-batch contract consumes the same pair and direct initializer
receipts. It checks the local's source/typed Block, ValDef, initializer and
ordered use roots, and validates emitted symbolic bytecode against that
identity: initialize one Counter from seed slot 2, store the initialized value
in local slot 3, then allocate Pair and load the original arguments in their
source order. The reversed method retains slots 1/2/3 and reverses only those
Pair argument loads. Existing helper-local checks remain covered separately.

`tests/jvm_tasty_artifacts.rs::immutable_local_pair_batches_emit_deterministic_authenticated_artifacts`
emits the complete six-file batch from each frozen source using the real paired
frontend pipeline. Independent sessions for each identical source produce
byte-identical artifacts and the same UUIDs. These hashes identify the Rust
output from that regression at this revision; the identical Counter and Pair
declarations produce identical files in both orientations.

| Variant | Artifact | SHA-256 | TASTy UUID |
| --- | --- | --- | --- |
| both | `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008047c3c00968ac0000000000008193` |
| both | `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | `008047c3c00968ac0000000000008193` |
| both | `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d7441c7f00000000000008198` |
| both | `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | `000b7d8d7441c7f00000000000008198` |
| primary | `example/PairFactory.class` | `f5f98b7460958398e1871f473261cffd947c59abf6f3be4718f39d0cb6133b26` | `00aa404076fa2cc80000000000008199` |
| primary | `example/PairFactory.tasty` | `243a9bf981e8fbc8403d6f6da9108798a8615446cbc8d1b608b8c60100cd36ca` | `00aa404076fa2cc80000000000008199` |
| reversed | `example/PairFactory.class` | `a8074b8b4fbb5e822eb38213ac789629462e734faf7623fbf52c0c1caf741493` | `00cc322d08c23fc80000000000008199` |
| reversed | `example/PairFactory.tasty` | `e3126007b1309789e45a7fedef0b9f8d43bec44a15f3271367074a22a27b1735` | `00cc322d08c23fc80000000000008199` |

The Rust classfiles are major 65, and the validator checks the class-file TASTy
UUID links against each sidecar header before accepting the batch. The
contract's bytecode check uses resolved class/method references and decoded
local instructions; it does not rely on fixed PCs or constant-pool indexes.

The strict pinned Scala 3.9 reader passed for both argument orders through
`formal_local_pair_pickles_both_original_argument_orders` with
`DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1`. The compiler was staged from source
revision `777528f19a58e794c9954a42f433373472ec57f8` under Temurin 21.0.13 and
reported `3.9.0-bin-SNAPSHOT-git-777528f`. For each Rust-produced factory
sidecar, the test checks Scala decompilation and `-print-tasty`, then uses
`-from-tasty -release 21` to reconstruct the factory class. This establishes
the required pinned reader result for primary and reversed argument order; it
does not claim that Scala compiled or executed a client against the Rust JVM
classfiles.

## Original Rust runtime proof for issue #2074

`tests/jvm_tasty_mixed_origin_runtime.rs::immutable_reference_local_pair_executes_original_rust_classes_on_strict_jdk21`
drives both frozen sources through one authentic Rust paired producer session
each. It snapshots each six-file batch, validates class/TASTy versions and
links, hashes files before compiling a separate Java harness, and places only
the three original Rust classfiles on the runtime classpath. The test checks
all six original bytes and hashes after compilation, execution, and the
isolated corruption runs. The test binary is included in the mandatory JDK
workflow target list.

Measured with Temurin 21.0.8+9 (`javac --release 21`,
`java -Xverify:all -ea`), both methods have descriptor
`(Lexample/Counter;I)Lexample/Pair;`, `stack=4`, and `locals=4`. The observed
PCs are the same in both orientations: `new Counter` at 0,
`Counter.<init>(I)V` at 5, `astore_3` at 8, `new Pair` at 9,
`Pair.<init>(Counter,Counter)V` at 15, and `areturn` at 18. Slots are
`this=0`, `existing=1`, `seed=2`, and `fresh=3`. The primary method loads
`existing` at PC 13 and initialized `fresh` at PC 14; the reversed method
loads `fresh` and then `existing`. The Rust classfile CFG analysis accepts both
methods with `(max_stack, max_locals) = (4, 4)` and observes the initialized
slot store before the Pair call. The test checks original `javap -v -p -c`
output as well.

The Java harness reported
`primary=identity:true;mutations=isolated;repeat=fresh;seeds=0,-5` and
`reversed=identity:true;mutations=isolated;repeat=fresh;seeds=0,-5`. It checks
the reflected Pair fields, both mutation directions, a distinct fresh object
on a repeated call, and seed values 0 and -5.

The runtime snapshots reproduce the artifact hashes and UUIDs recorded in the
preceding #2073 table. The primary and reversed factory class/TASTy hashes are
`f5f98b7460958398e1871f473261cffd947c59abf6f3be4718f39d0cb6133b26` /
`243a9bf981e8fbc8403d6f6da9108798a8615446cbc8d1b608b8c60100cd36ca` and
`a8074b8b4fbb5e822eb38213ac789629462e734faf7623fbf52c0c1caf741493` /
`e3126007b1309789e45a7fedef0b9f8d43bec44a15f3271367074a22a27b1735`.
This test executes Rust-produced classfiles. It makes no claim about Scala
source TASTy interoperability.

The isolated controls report their first observed rejecting stage:

| Control | First observed failure |
| --- | --- |
| Swap same-typed Pair argument loads | Java identity assertion (`caller Counter identity moved`) |
| Store fresh reference into existing slot 1 with `astore_1` | JVM `VerifyError` at the later `aload_3`; local 3 remains `top` |
| Load slot 3 before its store | JVM `VerifyError` |
| Load the wrong local slot (`aload_0`) | JVM `VerifyError` |
| Load existing slot twice | Java identity assertion (`fresh local aliases the caller Counter`) |
| Remove `Counter.new` or `Counter.<init>` | JVM `VerifyError` |
| Replace the seed load with `pop` and remove `Counter.<init>` while retaining the `new` reference in local 3 | JVM `VerifyError` when the uninitialized reference is passed to `Pair.<init>` |
| Duplicate `Counter.new` or `Counter.<init>` | JVM `VerifyError` |
| Use the Counter constructor reference at the Pair call | JVM `VerifyError` |
| Use the Counter constructor descriptor at the Pair call | JVM `VerifyError` |
| Invoke the Pair constructor at the Counter initialization site | JVM `VerifyError` |
| Reduce `max_stack` or `max_locals` | JVM `VerifyError` |
| Omit `Counter.class` or `Pair.class` | JVM `NoClassDefFoundError` |

The existing dual-reference runtime regression still covers valid
`wrap(a, a)` aliasing. The new identity controls reject only incorrect wiring
of this issue's required fresh local.

The `astore_1` control is decoded back from the mutated classfile and asserted
to be the reference store opcode at the original store PC. Its verifier output
identifies a bad local-variable type at the later fresh-local load. The
uninitialized-reference control preserves bytecode length and the Pair
argument shape by changing the seed load to `pop` and replacing the Counter
constructor call with `nop`s. The duplicate uninitialized Counter left on the
stack is stored in slot 3 and reaches the Pair constructor argument. The
harness observes `VerifyError` whose diagnostic identifies the uninitialized
type. The independent missing-constructor case remains in the matrix.

For PR #2110, the focused
`immutable_reference_local_pair_executes_original_rust_classes_on_strict_jdk21`
test passed with `DOTTY_CLASSFILE_REQUIRE_JDK=1` on local Temurin 21.0.8 and
ran the external verifier/runtime controls. The full selected strict-JDK
integration command did not pass locally: its existing mixed-origin test
requires exactly Temurin 21.0.13. GitHub's strict-JDK CI job is configured for
21.0.13 and remains the authoritative full-gate result; no local full-gate
pass is claimed here.
