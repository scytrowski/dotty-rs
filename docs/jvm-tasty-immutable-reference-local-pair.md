# Immutable reference local passed to `Pair`

This document records the single-local constructor audit from issue #2063 and
the paired Rust producer increment from issue #2073. Scala-generated files in
[`tests/fixtures/jvm_tasty_immutable_reference_local_pair/`](../tests/fixtures/jvm_tasty_immutable_reference_local_pair/)
are Scala oracle artifacts; they are not Rust output or Rust/Scala
interoperability evidence. `oracle.json` records each source/artifact hash,
class-file major version, sidecar TASTy UUID, and the matching class-file link.

## Issue #2077 integrated release gate

The gate is based on merged main `7481ead4ca969b5fd897983aa7ae096e03413439`,
the merge of PR #2115. The seven predecessor PR heads below are ancestors of
that commit. Each final logical review was `READY_FOR_MERGE` on the listed PR
head, and each PR's Rust, class-file/JVM and pinned Scala workflow checks
completed successfully. This release gate adds tests and evidence only; it
does not add producer semantics.

| Issue | Merged PR | Reviewed/tested head | Merge commit | Final review |
| --- | --- | --- | --- | --- |
| #2063 | [#2081](https://github.com/scytrowski/dotty-rs/pull/2081) | `2cac3adb7a6ca9426a53f62c77a81bf744df40e3` | `91f982aa6946aec5584e700084a6acee82ee630e` | [`READY_FOR_MERGE`](https://github.com/scytrowski/dotty-rs/pull/2081#pullrequestreview-5756227050), zero blockers |
| #2065 | [#2085](https://github.com/scytrowski/dotty-rs/pull/2085) | `0c637cc3f33b50fb8dcad7b0de65988d7ec6c8b5` | `254f00c2c03f027c1b2f0da3f35e78d54bab953d` | [`READY_FOR_MERGE`](https://github.com/scytrowski/dotty-rs/pull/2085#pullrequestreview-5758791335), zero blockers |
| #2068 | [#2089](https://github.com/scytrowski/dotty-rs/pull/2089) | `356bb699888cd89bf106f3a1f012ccaa643a7112` | `caf81e06693700c9211c095f971c62ed21ef49d5` | [`READY_FOR_MERGE`](https://github.com/scytrowski/dotty-rs/pull/2089#pullrequestreview-5760659588), zero blockers |
| #2069 | [#2105](https://github.com/scytrowski/dotty-rs/pull/2105) | `4d9f7b33894b34c6963c38099cb265985c9fad64` | `b3e2a35b219959cecddee14a24ee0452e61458f7` | [`READY_FOR_MERGE`](https://github.com/scytrowski/dotty-rs/pull/2105#pullrequestreview-5761897981), zero blockers |
| #2073 | [#2107](https://github.com/scytrowski/dotty-rs/pull/2107) | `01cf0fa45b92df64ca41c638b9b442154a142ba0` | `eedcf312b989c9e5003a26a7a2576124db3ff05b` | [`READY_FOR_MERGE`](https://github.com/scytrowski/dotty-rs/pull/2107#pullrequestreview-5762653424), zero blockers |
| #2074 | [#2110](https://github.com/scytrowski/dotty-rs/pull/2110) | `68d80b84ffc22a8de1649bce7fb59c424e1682c8` | `e318bd76c638bedf0ec1ebd4b82b8ee58baaa104` | [`READY_FOR_MERGE`](https://github.com/scytrowski/dotty-rs/pull/2110#pullrequestreview-5765172055), zero blockers |
| #2075 | [#2115](https://github.com/scytrowski/dotty-rs/pull/2115) | `dbf93dec536c75ae9b792f51ae290be249c4818e` | `7481ead4ca969b5fd897983aa7ae096e03413439` | [`READY_FOR_MERGE`](https://github.com/scytrowski/dotty-rs/pull/2115#pullrequestreview-5767433762), zero blockers |

The Scala source revision is
`777528f19a58e794c9954a42f433373472ec57f8` (Scala 3.9.0 / TASTy 28.9.0);
the JDK is Temurin 21.0.13+9 (binary SHA-256
`9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`). The
published runtime JAR hashes below are pinned; the locally staged compiler
snapshot JAR hash is recorded only as an observation. The Rust toolchain is
the repository-pinned 1.98.1 from `rust-toolchain.toml`.

Acceptance evidence map: AC1 is this provenance ledger and the source/client,
toolchain and artifact hashes below; AC2–AC3 are the original-session and
ordered proof sections; AC4–AC5 are the TASTy address and six-artifact
determinism sections; AC6–AC7 are the decoded local instruction evidence and
independent Java/Scala clients; AC8 is the observed first-failure matrix plus
the immutable-local publisher fault regression; AC9 is the integrated strict
gate receipt below; AC10 is this document's combined ledger. Historical audit
failures are marked as such and are not used as current acceptance evidence.

This gate is limited to one immutable local initialized by direct
`new Counter(seed)` and passed as one of two `Pair` constructor arguments. It
does not cover multiple locals, `var`, nested or branch-local scopes,
helper-produced locals, a third constructor argument, cross-unit/imported
classes, generic/contextual/default/secondary constructors, a public compiler
CLI, or broader Scala ABI support. Publisher tests cover handled injected
failures and do not claim crash-atomic multi-file durability.

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

## Rust-authentic Scala 3.9 consumer

`tests/jvm_tasty_immutable_reference_local_pair_scala_consumer.rs` now emits
the primary and reversed producers through `emit_paired_jvm_tasty_batch` and
compiles only the separate `Consumer.scala` client with the pinned official
Scala compiler. The classes and sidecars used for runtime are the same six
files inspected by `-decompile`, `-print-tasty`, and `-from-tasty`; reconstructed
classes are written to a separate inspection directory and never enter the
runtime classpath. The client runs with `java -Xverify:all -ea` against the
unchanged Rust files plus the two approved published Scala runtime jars.

The consumer checks the original identity on the expected Pair field, a fresh
Counter on the other field, getter and setter source lookup, independent
mutations, fresh instances for seeds `11`, `0`, and `-5`, and reversed Pair
field ownership. Both Rust sessions repeat byte-for-byte with all six paths
and TASTy links equal. SHA-256 values and UUID links below are from that strict
run; shared Counter and Pair artifacts are identical between orientations.

| Source | Artifact | SHA-256 | TASTy UUID |
| --- | --- | --- | --- |
| primary | `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008071c3-c009-68ac-0000-000000008193` |
| primary | `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | — |
| primary | `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d-7441-c7f0-0000-000000008198` |
| primary | `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | — |
| primary | `example/PairFactory.class` | `f5f98b7460958398e1871f473261cffd947c59abf6f3be4718f39d0cb6133b26` | `00aa4040-76fa-2cc8-0000-000000008199` |
| primary | `example/PairFactory.tasty` | `243a9bf981e8fbc8403d6f6da9108798a8615446cbc8d1b608b8c60100cd36ca` | — |
| reversed | `example/Counter.class` | same as primary | same as primary |
| reversed | `example/Counter.tasty` | same as primary | — |
| reversed | `example/Pair.class` | same as primary | same as primary |
| reversed | `example/Pair.tasty` | same as primary | — |
| reversed | `example/PairFactory.class` | `a8074b8b4fbb5e822eb38213ac789629462e734faf7623fbf52c0c1caf741493` | `00cc322d-08c2-3fc8-0000-000000008199` |
| reversed | `example/PairFactory.tasty` | `e3126007b1309789e45a7fedef0b9f8d43bec44a15f3271367074a22a27b1735` | — |

The original source hashes are `9072a8c50bf0c9b62568187e7152618d4ea5567b2bed412c76cdcaf192af18cb`
(primary) and `81ee9e087f89eb071444244c4b2c8ca16c83711d89f07236ddfe35f331bc31db`
(reversed). The independent consumer hashes are
`10fd772dfd42d4ceaadf61373d543a2da01ec8b5d33f24d7aed26c650527f991`
(primary) and
`745f5779f8932aaa96d42230bcd3dea579b828e84a93bb130cff591f002d7087`
(reversed). The Scala source revision is
`777528f19a58e794c9954a42f433373472ec57f8`; Java is Temurin `21.0.13`, binary
SHA-256 `9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`.
Published `scala3-library_3-3.9.0.jar` and `scala-library-3.9.0.jar` SHA-256
values are respectively
`9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba` and
`b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd`.

### Scala lookup controls

For each orientation, compilation succeeds with the original TASTy sidecar
present after the corresponding classfile method/constructor entry is removed.
The matched control removes the same entry, the TASTY class attribute, and the
sidecar. The actual first Scala diagnostics are:

| Lookup | TASTy-backed | Classfile-only first failure |
| --- | --- | --- |
| `Counter(Int)` | accepted | `[E173] Reference Error` |
| `Counter.get(): Int` | accepted | `[E008] Not Found Error` |
| `Counter.set(Int): Unit` | accepted | `[E008] Not Found Error` |
| `Pair.leftValue(): Int` | accepted | `[E008] Not Found Error` |
| `Pair.rightValue(): Int` | accepted | `[E008] Not Found Error` |
| `Pair(Counter, Counter)` | accepted | `[E173] Reference Error` |
| `PairFactory.create/createReversed(Counter, Int): Pair` | accepted | `[E008] Not Found Error` |

In the official `-print-tasty` output, `PairFactory.create` has one `BLOCK`,
two `PARAM` nodes, a single `VALDEF fresh` at address 74, and direct references
to method formal address 32, local address 74, and seed formal address 41.
Scala 3.9's BLOCK stream lists its final Pair expression before the local
declaration, so target addresses and the decoded initializer are asserted
separately from source evaluation order. The reversed producer swaps the two
Pair-constructor reference targets while retaining the formal types and local
initializer. `-from-tasty` reconstructs all three classes into the
inspection-only directory.

The negative matrix starts from fresh copies of the original Rust files.
Removing or moving any one of `Counter.tasty`, `Pair.tasty`, or
`PairFactory.tasty` fails with Scala's missing `.tasty` loader error;
truncation fails in the TASTy reader as a broken file; and changing the
classfile TASTY UUID fails with Scala's out-of-sync error. A dangling local
`TERMREFdirect` is intentionally lazy: ordinary client signature lookup still
passes, then official `-decompile` fails in phase `readTasty` with
`TreeWithoutOwner`. It is recorded at that observed stage rather than treated
as an eager lookup rejection. Three additional isolated Factory-sidecar copies
exercise the local body. Repointing the local use at the enclosing BLOCK passes
signature lookup and fails in `readTasty` with `TreeWithoutOwner`; this
BLOCK-target probe is an invalid-reference control, not a declaration-owner
mutation. Changing the
initializer's `SHAREDTYPE` target from `Counter` to `Pair` passes lookup and
`-decompile`, then fails in `-from-tasty` code generation with Scala's
`Unexpected New(...Pair...) reached GenBCode` assertion. Changing the local
VALDEF type from `Counter` to the valid `Pair` type passes signature lookup,
`-decompile`, and `-from-tasty`; the decompiled local is explicitly asserted as
`fresh: example.Pair = new example.Counter(seed)`. This is Scala's observed
acceptance of a well-formed but semantically inconsistent lazy body annotation,
not a claimed rejection. The source-bound first-error matrix separately
rejects a wrong local type during method body typing with `TypeNameNotFound`;
the pair proof rejects an incorrect constructor owner before TASTy planning.
Separate source-bound ownership controls preserve the local declaration,
typed binding, references, and proof plan, then mutate the canonical local
`SymbolId` owner or its canonical `ScopeId` owner to the valid constructor
class `SymbolId`. Each test asserts that exact mutation before validation and
checks the corresponding first `SourceIdentity` failure. This exercises actual
declaration ownership independently of the malformed BLOCK-target reference.

The wrong-package control emits a separate genuine Rust batch in
`renamed.example`, leaves the consumer correctly qualified as `example`, and
fails to resolve `example.PairFactory` with `[E006] Not Found Error`. This
exercises a producer-package mismatch; it does not manufacture the diagnostic
by moving the client to an unrelated package. Existing UUID mismatch cases
cover stale sidecars independently.

Additional isolated signature controls replace only the Factory sidecar with
a pinned-Scala-generated sidecar linked to the unchanged Rust Factory
classfile. Wrong formal count, formal type, and result type each fail at
independent Scala client compilation with `[E007] Type Mismatch Error`. Wrong
package references fail with `[E006] Not Found Error`. Invalid `get`,
`leftValue`, Factory `create`, and Pair-constructor descriptors are changed
only in negative classfile copies; Scala compilation reads the valid sidecars,
then JDK 21 `-Xverify:all` rejects the incompatible binaries with `VerifyError`
(`Method does not expect a return value` for `get`/`leftValue`, `Bad local
variable type` for Factory `create` and Pair constructor). These failures are
observed in negative copies; the six runtime inputs remain unchanged.

Fresh Rust sessions with renamed package/classes/method/local compile and run
the matching independent client. Reordered class declarations also compile
and run. Their producer source SHA-256 values are
`35d686fd9ca9dbaa2242122c8e26ed31c348dfbb364903257899e6fc7a2cfd3f`
(renamed) and
`67745bd2818a9c99315cfd36df19351a9933b1a5946d21c4cf851957de86e26f`
(reordered); consumer hashes are
`e40aeca5b9609a7322f9d0b391917c645c3ed55abe7c0f0eeb5427de6182a840`
(renamed) and the primary consumer hash above (reordered).

The strict opt-in is
`DOTTY_REQUIRE_SCALA_TASTY_IMMUTABLE_REFERENCE_LOCAL_PAIR_CONSUMER=1` in the
existing pinned Scala consumer workflow. Required mode fails on absent paths,
wrong source revision, runtime hash mismatch, missing compiler, or JDK version
and hash mismatch; its test target is included in the workflow's strict target
list.

The locally staged compiler distribution SHA-256 was
`048f38662a47f66dc0d7c5e0d2308c20d0419ab1bf4018ef09a9e3831a13e2c9`. This is
an observation for this staged build only and is not a pinned compatibility
value.

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
its valid Block cases reached the direct-Apply shape rejection. The ordered
formal/local argument proof still accepts only the one-local direct-New
initializer with one other original Counter formal. Its paired-planning path
does not consume the later two-initializer receipt.

## Issue #2123 proof-only two-initializer receipt

`validate_immutable_local_initializer_pair_proof` composes the shared
canonical declaration/initializer validator for exactly two distinct
immutable `Counter` ValDefs in one original straight-line Block. It requires
two ordinary Int method formals, one direct `new Counter(formal)` initializer
per local, distinct declarations/symbols, one exact method-owned scope with
both entries, original source-to-typed mappings, and declaration/initializer
order before the final Block expression. The pair is returned only after both
individual receipts and the shared Block/scope/order checks succeed.

| Receipt field | Preserved original identity |
| --- | --- |
| Block roots | source and typed Block, plus final expression roots |
| declaration | source and typed ValDef, local SymbolId, ScopeId, declared TypeId |
| initializer | source and typed Apply, Select, New, New type tree, and Int argument roots |
| constructor | selected original Counter primary constructor and its Int formal |
| caller input | exact corresponding original method formal SymbolId and Int type |

The focused authentic frontend test proves the frozen source, reversed
argument source, and reversed local declaration order in independent sessions,
plus consistently renamed and class-reordered sessions. Receipts retain source
declaration order while each initializer's original typed `TermRef` retains its
actual method formal; independently-authored sources may assign the two
distinct formals in either order.
Negative controls cover stale and swapped typed edges, a missing Typer binding,
mutable locals, duplicate use of one formal, a conditional initializer, and a
third local. This aggregate receipt does not prove Pair argument order or
origins. The later paired planner applies its separate final-argument proof;
on the current base, both reversed-declaration and swapped-initializer
variants pass that downstream planner without changes to its contract. This
follow-up does not change TASTy planning, class emission, or a CLI, and it does
not change emitted bytes.

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
pass was claimed at the #2110 checkpoint. The #2077 integrated run below uses
Temurin 21.0.13 and records the later full-gate result.

## Issue #2077 integrated execution

The implementation delta was tested from frozen main
`7481ead4ca969b5fd897983aa7ae096e03413439`; the publisher regression is commit
`faacce7f3f631f26cba6af8ba59d3849de10ae81`. The full inherited pinned Scala
3.9 workflow target list in
[`jvm-tasty-factory-scala-consumer.yml`](../.github/workflows/jvm-tasty-factory-scala-consumer.yml)
was executed with every listed `DOTTY_REQUIRE_SCALA_*` opt-in enabled and
`DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1`; it passed with no required skips.
This includes the immutable-local consumer test, both source orientations,
Scala `-decompile`/`-print-tasty`/`-from-tasty`, lookup controls, and original
Rust artifact hash checks.

The mandatory JDK 21 CI suite passed with
`DOTTY_CLASSFILE_REQUIRE_JDK=1` on Temurin `21.0.13+9`. It included the
class-file encoder, JVM backend, integrated source compilation, paired artifact
publisher, immutable-local Java runtime and corruption controls, dual-reference
identity, and stateful class contract targets. The issue-specific publisher
regression also passed, including six staged-write faults, both promotion
faults, and pre-existing sentinel preservation. Each staged-write case asserts
the `staging write` phase, injected error kind/message, and exact relative
artifact path; promotion cases assert their distinct phase, injected message,
and destination. The regression also checks missing class data/path, unsafe
path, and duplicate path rejection.

Both commands used Rust/Cargo 1.98.1 and task-local `.tmp` and `.cargo-target`
directories because `/tmp` had no free space. The JVM options disabled shared
perf data and redirected Java temporary files into `.tmp`; the required JDK
version and tools remained enabled. The standard repository gates also passed:
`cargo +1.98.1 fmt --all -- --check`,
`python3 tools/check-frontend-boundaries.py`,
`cargo +1.98.1 clippy --workspace --all-targets --locked -- -D warnings`,
`RUSTDOCFLAGS="-D warnings" cargo +1.98.1 doc --workspace --no-deps --locked`,
and `DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo +1.98.1 test --workspace --all-targets
--locked -- --test-threads=2`. `git diff --check` passed after the evidence
ledger update.

## #2128 official Scala source consumer for the Rust two-local batch

The strict linked Scala consumer now also runs with
`DOTTY_REQUIRE_SCALA_TASTY_TWO_IMMUTABLE_REFERENCE_LOCALS_CONSUMER=1`. It
produces the original six `Counter`, `Pair`, and `PairFactory` class/TASTy
artifacts through one Rust source session, then compiles an independent Scala
client against those outputs and the approved Scala libraries. The client
classpath and runtime classpath contain the original Rust outputs; Scala
`-from-tasty` classes are written to a separate inspection-only directory.

The primary source SHA-256 is
`e2411637c892feb474d6d70526d471b524cd7de49a3db2565923cd69736f73c3`; the
independent primary Scala client SHA-256 is
`d0ecf8a1183b95508da88f75a06175c2b01dacc575c9b44498d00b030572d00c`. The
successful local strict run used Scala source revision
`777528f19a58e794c9954a42f433373472ec57f8` (`3.9.0-bin-SNAPSHOT-git-777528f`)
and Eclipse Temurin `21.0.13+9`, Java binary SHA-256
`9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`. The
published runtime JARs were checked against the approved hashes recorded in
the preceding oracle section. The Rust output classfile/TASTy fingerprints
were:

| Rust output | SHA-256 | Paired TASTy UUID |
| --- | --- | --- |
| `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008071c3-c009-68ac-0000-000000008193` |
| `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | — |
| `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d-7441-c7f0-0000-000000008198` |
| `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | — |
| `example/PairFactory.class` | `16f734ec97d054a2ff1ec40400b6583b9d60476babcf0ba8617fd98dc0feebea` | `000a7216-f3d1-05c8-0000-00000000819a` |
| `example/PairFactory.tasty` | `9415f52a7476d72554ba8943b9b657d588385fc339b3ff639a3c7d51e3d685e4` | — |

The classfile UUID is the corresponding sidecar UUID. `PairFactory.create`
has two original formal references, two declaration-ordered local addresses
`[72, 97]`, and final Pair argument references `[72, 97]`; the final `APPLY`
may precede the `VALDEF` wire entries. Reversed arguments resolve to the same
original locals in reverse order, and repeated-first arguments point twice to
the first local. Runtime output confirmed `7/3`, `3/7`, independent mutation,
fresh references and deliberate repeated-first aliasing. Every primary
class/sidecar hash matched after client compilation, execution and sidecar
inspection.

The original source, reversed Pair argument source, repeated-first-local
source, renamed package/class/method/local source, and reordered declarations
each receive their own Rust producer session and matching Scala client. The
client covers independent seeds, equal-seed reference identity, separate
mutation, fresh references across repeated calls, positional reversal, and
the intentional alias produced by `new Pair(first, first)`. Re-emitting each
source in a fresh Rust session compares every class and sidecar byte and UUID.
The six artifact hashes are checked after Scala compilation and runtime; the
three original sidecars are also hashed around Scala inspection and
reconstruction.

For the primary source, `-decompile`, `-print-tasty`, and inspection-only
`-from-tasty` run on all original sidecars. The Rust-produced `PairFactory`
sidecar is checked for its two formal parameters, source-ordered `first` and
`second` local declarations and initializers, and final `Pair` argument
references to the corresponding local AST addresses. Masked classfile members
are compiled once with the valid Rust TASTy sidecar and once without the TASTy
marker/sidecar; all seven TASTy-backed required lookups succeeded while their
matched classfile-only controls failed. The observed classfile-only first
diagnostics were `[E173]` for Counter and Pair constructors and `[E008]` for
Counter `get`/`set`, Pair `leftValue`/`rightValue`, and Factory `create`.
Isolated missing/misplaced sidecars first failed as “.tasty file is missing”;
truncated sidecars first failed as broken `ArrayIndexOutOfBoundsException`
TASTy; mismatched classfile UUIDs first failed as “out of sync with its TASTy
file”. A dangling first or second local reference and a local-to-BLOCK
reference were accepted during lazy signature lookup, then failed with
`TreeWithoutOwner` during `-decompile`. Scala accepted wrong first/second local
types and a constructor-type mutation through lazy lookup, `-from-tasty`, and
`-decompile`. The test checks each local's decoded declared type, records the
observed decompile form, and requires the source-authenticated Rust batch
contract to reject each mutation against the original two-local source. The
initializer mutation targets the `Pair` type reference in the first local's
constructor graph. The single-reference alias and duplicate-address mutations
are also checked against source-authenticated contracts. The duplicate-address
mutation decodes as `new Pair(first, first)` and
is rejected by the source-authenticated Rust batch contract against the
original `new Pair(first, second)` input at TASTy plan identity. A separately
produced `new Pair(first, first)` source remains the positive source-contract
and runtime alias control. Matching clients against the wrong factory formal
count/type/result controls first fail compilation with `[E007]`; all four
malformed JVM descriptor clients compile against the unchanged sidecars and
then first fail Java `-Xverify:all` with `VerifyError`. These controls never
replace the accepted runtime artifacts.

The target remains registered once in the existing four-shard strict Scala
workflow and its fail-closed aggregate. The required environment name above is
included in each shard declaration and in the aggregate's required-environment
inventory. Run locally with:

```sh
DOTTY_REQUIRE_SCALA_TASTY_TWO_IMMUTABLE_REFERENCE_LOCALS_CONSUMER=1 \
  cargo test -p dotty --test jvm_tasty_immutable_reference_local_pair_scala_consumer \
  --locked -- --nocapture
```

The strict log records the variant producer/client hashes, all original Rust
artifact hashes and UUIDs, compiler/JDK versions, observed Scala outcomes, and
elapsed test time. The focused strict Rust-produced two-local consumer test
completed in 175.83 seconds in the recorded local run. This was the full six-test target run, including both the existing one-local consumer and the new two-local strict matrix.

## Independent `Counter.set` consumer

The separate `jvm_tasty_two_local_counter_set_scala_consumer` target compiles
`tests/fixtures/jvm_tasty_two_local_counter_set/Consumer.scala` with the pinned
Scala 3.9.0 compiler against only the original Rust-produced `Counter`, `Pair`,
and `PairFactory` class/TASTy pairs plus the hash-checked official Scala
runtime. It executes both the original source declaration order and the
fully reversed `first`/`second` declaration order. The client observes
`create(7, 3) == 3/3` and `create(-9, 42) == 42/42`, reads both `Counter`
objects by reflection, confirms they are distinct, and mutates each object to
verify the pair exposes the expected independent values. The assertions record
runtime-observed identity; they do not infer identity from equal values.

For the original six Rust artifacts, official `-print-tasty`, `-decompile`, and
inspection-only `-from-tasty` verify the two local bindings, the direct
`Counter.set(Int): Unit` call on `first` with `secondSeed`, and the final
`Pair(first, second)` constructor call. The target also runs the existing masked
classfile/TASTy lookup controls, including `set`, isolated sidecar negatives,
malformed local-reference checks, and source-authenticated negative checks.
Each Scala compile and execution is followed by a byte-for-byte check of the
six producer artifacts.

For stat negatives, Scala's reader accepts an artifact produced from a source
without the required `set` call, while Rust's source-authenticated plan check
rejects those artifacts against the required source. A non-Unit `get()` call in
that stat position is rejected by the Rust producer before artifact emission,
with the observed `Counter.set statement must be one regular one-argument
Apply` source-shape diagnostic; no Scala outcome is claimed for that
unproduced variant.

The new target's observed local runtime was 86.14 seconds. It is in strict
shard 1 and is required by the distinct
`DOTTY_REQUIRE_SCALA_TASTY_TWO_LOCAL_COUNTER_SET_CONSUMER=1` flag in every
shard and the aggregate fail-closed inventory. Run it locally with:

```sh
DOTTY_REQUIRE_SCALA_TASTY_TWO_LOCAL_COUNTER_SET_CONSUMER=1 \
  cargo test -p dotty --test jvm_tasty_two_local_counter_set_scala_consumer \
  --locked -- --nocapture
```

## Issue #2244: aliased local with two ordered setters

The strict target `jvm_tasty_two_local_counter_set_scala_consumer` reuses the
existing two-local Scala producer gate and adds one alias-bearing producer in
`tests/fixtures/jvm_tasty_two_local_counter_set/AliasTwoCalls.scala`. The
source declares `first`, `second`, and `alias = first`, then calls
`alias.set(secondSeed)` followed by `second.set(firstSeed)` before constructing
`Pair(first, second)`. Its pinned Scala client checks `(7, 3) -> (3, 7)` and
`(-9, 42) -> (42, -9)`, distinct Pair fields and fresh results, then mutates
each returned object independently. It also calls `create(11, 11)`, asserts the
equal-valued fields hold distinct Counter references, and mutates the left
Counter while the right remains `11`. The same client runs against original,
renamed-alias, and reversed-local producers. Fixture SHA-256 values are
`b1b200af5108685cf58b769b3ed0654c189d2fb28fa1c8eaaba5f02580d2b79f`
(producer) and
`da97e28cb22f2405872ada8ab38922444144038bcfb5253012293c334905ca4f`
(client).

The strict run uses Scala 3.9.0 at revision
`777528f19a58e794c9954a42f433373472ec57f8`, JDK 21.0.13, and Rust 1.98.1.
The Rust-authored producer batch has exactly the six files below. The original
and Scala-consumed copies were hash-checked, and each classfile TASTy link was
checked against its sidecar UUID.

| Artifact | SHA-256 | Classfile TASTy UUID |
| --- | --- | --- |
| `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008071c3-c009-68ac-0000-000000008193` |
| `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | — |
| `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d-7441-c7f0-0000-000000008198` |
| `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | — |
| `example/PairFactory.class` | `68027bf87468fd724f034cc4fd67d4e57220e389fb4130f9e3a33bcd2cea98e1` | `00c9bae5-ea1a-54c2-0000-00000000819e` |
| `example/PairFactory.tasty` | `e56be0c7ff675f7d05a34914cb924c13fac030a54e72e06612de03b286a570ed` | — |

The Rust structural reader and Scala `-print-tasty`, `-decompile`, and
`-from-tasty` inspect the same batch. The decoded method has a BLOCK, three
local VALDEFs, two ordered setter applications, and a final Pair application.
`alias` is a direct `TERMREFdirect` to `first`; the setter receivers resolve to
`alias` then `second`, and constructor arguments resolve to `first` then
`second`. The test also repeats the positive client with a renamed alias and
reversed initial local declarations, asserting semantic links by names rather
than relying on allocation order. Runtime prints
`rust-alias-two-call-consumer passed` and runs with `java -Xverify:all -ea`.

First-failure controls cover a missing, misplaced, truncated, or UUID-mismatched
PairFactory sidecar; classfile-only factory lookup; a missing declared Pair
class and sidecar; and omission of the second setter call. The missing Pair
control asserts Scala's first diagnostic is `[E008] Not Found Error` with
`value Pair is not a member of example`. Missing/misplaced sidecars assert the
missing-sidecar loader message, truncation asserts the broken-reader message,
and UUID corruption asserts the out-of-sync message. The missing-second-call
producer control asserts the first Rust failure is at `cross-class
constructor-call identity proof` with the unsupported bounded local block
shape. Scala 3.9 accepts both the corrupted alias TERMREF and forged signed
setter in isolated modified TASTy copies; the tests assert successful Scala
compilation for each while the source-bound Rust contract rejects those exact
modified artifacts at TASTy plan identity because their sidecar bytes differ
from the validated plan output. The producer and consumer assertions use
isolated copies for all mutations; the six positive artifacts remain
unchanged.

The test runs under the existing
`DOTTY_REQUIRE_SCALA_TASTY_TWO_LOCAL_COUNTER_SET_CONSUMER=1` flag, manifest
entry, and strict Scala consumer workflow shard 1/4. No new workflow, target,
manifest entry, or production code is introduced for this increment.
