# Immutable reference local passed to `Pair`

This is an audit of the single-local constructor flow from issue #2063. It
does not enable JVM or TASTy producer behavior. Scala-generated files in
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

The two positive rows stop downstream because this issue only adds proof
acceptance: the current paired TASTy path still reports unsupported
`BodyExpression`, and no JVM/TASTy output is enabled by this change.
