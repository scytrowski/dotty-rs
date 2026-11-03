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
a mutable local, a conditional initializer, and a helper-initialized local.
Rust reaches the bounded paired-proof shape rejection for these; this records a
gap without enabling new producer behavior. Scala rejects missing
initialization, name shadowing, use before initialization, a wrong local type,
a wrong constructor argument type, wrong constructor arity, a nested result
block, a wrong constructor owner, and a third constructor argument. Rust
reports the corresponding first typing/shape error classes in the test. These
source-invalid cases are controls, not extra acceptance criteria.
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
| shadowed formal name | rejects | method body typing: `TermNameNotFound` |
| use before initialization | rejects | method body typing: `TermNameNotFound` |
| wrong local type | rejects | method body typing: `TypeNameNotFound` |
| wrong constructor argument type | rejects | method body typing: `ApplicationArgumentTypeMismatch` |
| wrong constructor arity | rejects | method body typing: `ApplicationArityMismatch` |
| nested result block | rejects | method body typing: `AnonymousClassInstantiationDeferred` |
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
