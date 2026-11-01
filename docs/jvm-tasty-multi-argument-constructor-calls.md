# Two-argument sibling constructor-call audit

This is an evidence-only audit for [issue #1913](https://github.com/scytrowski/dotty-rs/issues/1913). It adds no source typing, JVM, TASTy, paired artifact, or public API support. The checked-in Scala artifacts are output from Scala 3.9.0; they are not Rust output and do not establish Rust-to-Scala interoperability.

## Ordered direct-formal proof baseline (#1961 checkpoint 1)

Issue [#1961](https://github.com/scytrowski/dotty-rs/issues/1961) starts from
current `main` at `36d987fe262b18a61e9082216a759aeac3f991b3`, which contains
the merged #1960 audit. Before extending the proof, the focused legacy
constructor-proof tests and the #1960 source-session regression passed:

```text
cargo test -p dotty --test jvm_tasty_constructor_call_proofs --locked: 4 passed
cargo test -p dotty --test jvm_tasty_dual_reference_forwarding --locked: 3 passed
cargo test -p dotty --test jvm_source_compilation dual_reference_factory_wrap_records_the_genuine_source_session_and_first_proof_error --locked: passed
```

The existing immutable proof kinds remain `Formal`, `NestedNew`,
`NestedNewPair`, `PlannedCall`, and `LocalVal`. Their read-only projections
(`formal`, `nested_pair`, `nested_pair_context`, `nested_argument`,
`planned_call`, and `local_val`) are the compatibility baseline. The authentic
one-session `PairFactory.wrap(first, second)` source still emits its ordinary
JVM classes and preserves their bytes; the read-only proof currently stops at
`SemanticIdentity("both factory formals must have the canonical Int type")`.
Corrupting the first argument's typed-index edge to the second argument's root
currently produces that same earlier error, so the corruption is masked at
this baseline. The paired planner reports that error at its
`cross-class constructor-call identity proof` phase and does not invoke its
callback.

The independent Scala 3.9 oracle files are unchanged baseline fixtures. Their
SHA-256 values are recorded here so the proof-only work does not replace them
with Rust output:

| Artifact | SHA-256 |
| --- | --- |
| `Counter.class` | `35afad87cca23efbf1185229d0fbed6b689aea7649ef69a3cf14232779763395` |
| `Counter.tasty` | `f3cb320b58199798c7ab1d31f017476059506f52349fb9fef927a593aa3dce7e` |
| `Pair.class` | `e6db9d87df09fcd961b932ab7bc66b0b73fcdf30bb50ab5895043f1a7e7c614c` |
| `Pair.tasty` | `bd7d14f448a1cd4a64c94b34f8416d41816c1c9a57978750639f1e70404bc617` |
| `PairFactory.class` | `7727e0f390d3dc4109decaffe1a87610c76253b950829292499f1643229dd0b2` |
| `PairFactory.tasty` | `c7fe6a0e8334e8518b5f467c6253f47c531fc9ccb8831c8e51b29d185747d8e5` |

## Pinned source and oracle

[`PairFactory.scala`](../tests/fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactory.scala) is the issue's unchanged one-file producer source. It contains the original `Counter`, `Pair`, and `PairFactory` declarations in that order. Its SHA-256 is `7e40de1afb6272dbcf481baab7148801a055210216a19bdeaff85fafb40ff1bf`. The independent Scala consumer is [`Consumer.scala`](../tests/fixtures/jvm_tasty_multi_argument_constructor_calls/Consumer.scala); it is compiled separately and is not used to produce the oracle classes.

The oracle uses Scala 3.9.0 source revision `777528f19a58e794c9954a42f433373472ec57f8`, staged compiler `3.9.0-bin-SNAPSHOT-git-777528f`, compiler JAR SHA-256 `b56626390028bd06347e3549cf271a03ca7d2e7be491ceff630ccee94e439f4f`, and SDKMAN Temurin `21.0.13+9-LTS` (JDK `21.0.13`, classfile major 65). `oracle.json` records tool versions, hashes, all six producer artifact paths, sizes and hashes, and each classfile TASTY UUID linked to its sidecar UUID. `oracle/*.javap.txt` and `oracle/*.tasty.txt` are readable snapshots from those exact artifacts.

The producer was compiled from the repository root into an empty directory with `-release 21`. It emitted exactly three classfiles and three sidecars:

| Class | `.class` SHA-256 | `.tasty` SHA-256 | linked UUID | TASTy |
| --- | --- | --- | --- | --- |
| `example.Counter` | `0e4175f68b99781aab85d04a867243747d7fe6f11687dcd7a7d2f55ca0eed012` | `f3c1adc42db4979c529c3566d88652c24fed026dd0d8550295592e16d3945a89` | `00c33321-22a9-b8e8-00dd-1db900712c1b` | 28.9.0 |
| `example.Pair` | `83cba46ced18fbb02ef353009827479c7e68030b866e23e08122e3f3a2020ce9` | `df8217316bd32e21b15eccf27b5e2019c736e49e514bb544f465ef7227f70c8b` | `001186c6-f799-45d3-001a-ce81c7f69d5b` | 28.9.0 |
| `example.PairFactory` | `4c84cdbe1ff94ed7708d2c16015c28e492add3deb366755ea2d7030f352b0e8f` | `2232b74c794b64107dc96b9535230378721f0d3274709fcc47437af89e2f1077` | `00d30365-3129-c6d3-00f1-d428bbe23920` | 28.9.0 |

The exact observed `PairFactory.create` ABI is `create(II)Lexample/Pair;`; its constructor invocation is `example/Pair.<init>(Lexample/Counter;Lexample/Counter;)V`. `javap -v -p -c` reports `stack=6`, `locals=3`, and `args_size=3`. Decoding by instruction and symbolic reference (without relying on constant-pool indexes or byte offsets) gives:

1. `new example/Pair`, `dup`
2. `new example/Counter`, `dup`, `iload_1`, `Counter.<init>(I)V`
3. `new example/Counter`, `dup`, `iload_2`, `Counter.<init>(I)V`
4. `Pair.<init>(LCounter;LCounter;)V`, `areturn`

Thus the outer Pair allocation is emitted before either argument, while the sibling Counter expressions run left to right and read the `first` and `second` instance-method locals respectively. The Rust fixture test resolves each instruction's actual constant-pool owner, name, and descriptor.

The `PairFactory.tasty` tree has `DEFDEF create` at address 30, with two distinct `PARAM` nodes: `first` at address 33 and `second` at address 42. Both formal types are `scala.Int`; the second parameter shares the first `Int` type tree through `SHAREDtype` while preserving its own PARAM identity. Its RHS at address 55 is `APPLY(SELECTin(NEW(Pair)), arg1, arg2)`. `arg1` is a separate `APPLY(SELECTin(NEW(Counter)), TERMREFdirect(33))` rooted at address 67; `arg2` is a separate sibling `APPLY(SELECTin(NEW(Counter)), TERMREFdirect(42))` rooted at address 83. The selected signatures are `Pair.<init>(example.Counter, example.Counter): example.Pair` and, separately in each branch, `Counter.<init>(scala.Int): example.Counter`. TASTy stores the outer function before its argument children; the bytecode observation above establishes runtime allocation/evaluation order independently.

### Reproduction

Use a pinned Scala source checkout and JDK 21. Keep all temporary and build output under the issue worktree; for example:

```sh
export SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8
export JAVA_HOME=/path/to/temurin-21
export PATH="$JAVA_HOME/bin:$PATH"
export TMPDIR="$PWD/target/issue-1913/tmp"
export JAVA_TOOL_OPTIONS="-Djava.io.tmpdir=$TMPDIR -XX:-UsePerfData"
SCALAC="$SCALA39_ROOT/dist/target/universal/stage/bin/scalac"
SCALA_LIB="$SCALA39_ROOT/dist/target/universal/stage/maven2/org/scala-lang/scala3-library_3/3.9.0-bin-SNAPSHOT/scala3-library_3-3.9.0-bin-SNAPSHOT.jar:$SCALA39_ROOT/dist/target/universal/stage/maven2/org/scala-lang/scala-library/3.9.0-bin-SNAPSHOT/scala-library-3.9.0-bin-SNAPSHOT.jar"
ORACLE=target/issue-1913/oracle
mkdir -p "$ORACLE/producer" "$ORACLE/consumer"
test -z "$(find "$ORACLE/producer" -mindepth 1 -print -quit)"
"$SCALAC" -release 21 -d "$ORACLE/producer" \
  tests/fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactory.scala
"$SCALAC" -release 21 -classpath "$ORACLE/producer:$SCALA_LIB" \
  -d "$ORACLE/consumer" \
  tests/fixtures/jvm_tasty_multi_argument_constructor_calls/Consumer.scala
java -Xverify:all -ea \
  -classpath "$ORACLE/consumer:$ORACLE/producer:$SCALA_LIB" \
  example.PairFactoryConsumer
javap -v -p -c -classpath "$ORACLE/producer" example.PairFactory
```

The independent consumer passed under `java -Xverify:all -ea`: `create(7,3)` returned values `7/3`, `create(3,7)` returned `3/7`, and reflection confirmed distinct `left` and `right` Counter objects. Mutating the left Counter left the right value unchanged, and mutating the right Counter left the left value unchanged. This is an official Scala-only oracle baseline; it does not test or claim Rust/Scala interoperation.

## One-session Rust evidence

The `multi_argument_sibling_factory_records_rust_identity_and_first_proof_rejection` test sends that unchanged source through the real lexer, parser, namer, source typer, and JVM-only backend in one session (`SourceId(1913)`). The session preserves declaration order `Counter`, `Pair`, `PairFactory`; the JVM member order is `Counter.current/get/set`, `Pair.left/right/leftValue/rightValue`, then `PairFactory.create`.

Run-local identity receipt from the test:

| Source item | Source identity | Typed/semantic identity | Relationship |
| --- | --- | --- | --- |
| `Counter`, `Pair`, `PairFactory` classes | canonical `SymbolId(40)`, `(41)`, `(42)` | same canonical class symbols in the JVM unit | source declaration order |
| `PairFactory.create` | `SymbolId(60)` | method signature returns `TypeId(68)` (`Pair`) | owner is `SymbolId(42)` |
| `first`, `second` formals | source trees `TreeId(45)`, `TreeId(47)`; symbols `SymbolId(61)`, `(62)` | completed method parameter types `TypeId(5)`, `(5)` (`Int`); formal `SourceTypedIndex` entries are absent because formals are signature declarations, not typed expression roots | distinct canonical parameters owned by `create`; source spans are `379..389` and `391..402` |
| `create` body / outer Pair call | source `TreeId(62)` | typed `TreeId(26)`, result `Pair` | same source and typed root |
| outer Pair constructor selection and allocation | source Select `TreeId(51)`, New `TreeId(50)` | typed Select `TreeId(15)`, New `TreeId(14)` | selected constructor `SymbolId(52)`, owned by Pair `SymbolId(41)` |
| first inner Counter call | source Apply/Select/New/argument `TreeId(56/54/53/55)` | typed roots `TreeId(20/18/17/19)` | selected constructor `SymbolId(44)`, owned by Counter `SymbolId(40)`; argument type is the `first` formal reference |
| second inner Counter call | source Apply/Select/New/argument `TreeId(61/59/58/60)` | typed roots `TreeId(25/23/22/24)` | the same Counter constructor; argument type is the distinct `second` formal reference |

All expression nodes listed above retain their exact source-to-typed `TreeId` mappings. The two outer `Apply` arguments remain distinct roots with selected Counter constructor ownership and argument-specific `TermRef`s. The JVM-only source test emits exactly `Counter.class`, `Pair.class`, and `PairFactory.class` from this live session; it does not publish TASTy.

The read-only cross-class constructor-call proof is reached against that same live store, AST, typed AST, indexes, and JVM unit. Its first actual error is:

```text
SourceShape("candidate method must have one plain formal clause and a concrete public body")
```

This one-formal method guard is reached before the proof inspects the two-argument outer `Apply`. The paired-batch convenience API reparses in a separate session and reports its first error at `BatchUnit { phase: "cross-class constructor-call identity proof", ... }` with the same source-shape reason. Because this guard rejects first, class-state pickler plan validation and Rust TASTy encoding are unreachable through this paired path. No downstream pickler/TASTy error is claimed or fabricated. The successful JVM-only compilation is a separate output path and supplies no TASTy sidecars.

## Variant and rejection matrix

Scala classifications below were measured with the pinned Scala 3.9 compiler. Rust first results were measured by the paired-batch test; the original positive Rust session above is kept separate from the independent variant sessions.

| Control | Scala 3.9 | Rust first paired outcome |
| --- | --- | --- |
| Rename package, classes, and factory method | valid | cross-class proof: one-formal method source-shape guard |
| Reorder the three class declarations; preserve `first` then `second` expressions | valid | cross-class proof: same guard |
| Reverse factory formal declaration order | valid | cross-class proof: same guard |
| Swap the two nested Counter expressions | valid | cross-class proof: same guard |
| Add a third nested Counter allocation in the first branch | valid | cross-class proof: same guard; deeper allocation is not analyzed |
| Put a conditional expression in a Counter argument | valid | cross-class proof: same guard; branch shape is not analyzed |
| Add a default Pair constructor argument | valid | constructor planning: `InvalidPrimaryConstructor { issue: SourceShape }` |
| Add a generic Pair constructor | valid (`Pair[Any]` result in control) | source collection: generic primary constructors unsupported |
| Add a secondary Pair constructor | valid | member planning: secondary constructor method ownership unsupported |
| Make the second Pair constructor clause contextual (`using`) | valid | constructor planning: `InvalidPrimaryConstructor { issue: SourceShape }` |
| Call `Pair` with one argument | invalid Scala arity | method body typing: `ApplicationArityMismatch { expected: 2, actual: 1 }` |
| Pass a String to `Counter(Int)` | invalid Scala argument type | method body typing: `StringLiteralTypingDeferred` |
| Duplicate `create` formal name | invalid Scala declaration | method body typing: `AmbiguousTermReference` |
| Pass a same-spelled `foreign.Counter` where `example.Counter` is required | invalid Scala type ownership | Rust foreign-owner semantic mutation not measured |
| Wrong/foreign constructor symbol, duplicate identity, missing or stale typed-tree binding | not a source-language Scala variation | unmeasured and masked: the one-formal proof rejection occurs first; no foreign ID or replacement typed node was fabricated |
| Corrupt sidecar header | binary corruption control | issue fixture test observes TASTy parse rejection |
| Remove TASTY class attribute | binary corruption control | issue fixture test observes `MissingTastyLink` |
| Change a parseable sidecar UUID | binary corruption control | issue fixture test observes `StaleTastyLink` |

The Rust variation test creates separate planner sessions for its controls; their run-local IDs are not mixed into the original `SourceId(1913)` identity receipt. Source-invalid and Scala-valid-but-unsupported cases are kept distinct. Cases masked by the earlier guard are explicitly unmeasured at their downstream semantic boundary.

## Checks

The focused test names are:

```sh
cargo test -p dotty --test jvm_tasty_multi_argument_constructor_calls --locked
cargo test -p dotty --test jvm_source_compilation multi_argument --locked
cargo test -p dotty --test jvm_tasty_multi_argument_constructor_matrix --locked
```

The strict checked-in Scala oracle regression recompiles the producer, compares all six artifacts byte for byte, then runs the Scala-only consumer:

```sh
export JAVA_HOME=/path/to/temurin-21
export SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8
export DOTTY_REQUIRE_SCALA_TASTY_MULTI_ARGUMENT_CONSUMER=1
cargo test -p dotty --test jvm_tasty_multi_argument_constructor_calls --locked
```

The issue run also executed the inherited nested- and cross-class Scala consumer gates with `DOTTY_REQUIRE_SCALA_TASTY_NESTED_CONSTRUCTOR_CONSUMER=1` and `DOTTY_REQUIRE_SCALA_TASTY_CROSS_CLASS_CONSUMER=1`. It ran the mandatory JDK suite with `DOTTY_CLASSFILE_REQUIRE_JDK=1`, then the repository format, frontend boundary, Clippy, rustdoc, workspace-test, and `git diff --check` gates. Set `TMPDIR` and `CARGO_TARGET_DIR` beneath the task worktree, and set `JAVA_TOOL_OPTIONS=-Djava.io.tmpdir=target/java-tmp -XX:-UsePerfData` so JVM temporary files remain in its ignored `target/` directory.

The #1913 section above records the initial one-formal guard as measured at that revision. The #1914 implementation and current measurements follow.

## Ordered nested argument proof (#1914)

Issue [#1914](https://github.com/scytrowski/dotty-rs/issues/1914) extends the read-only source proof for the original `PairFactory.create(first: Int, second: Int)` shape. It uses the same authentic single-session lexer, parser, namer, source typer, source/typed indexes, and JVM compilation unit. It does not build source trees or typed roots for the proof and does not change JVM or TASTy output.

The new opaque `NestedConstructorArgumentPairProof` preserves both ordered contexts: the factory's two original formal source trees, symbols, and types; the Pair primary constructor's two original formal source trees, symbols, and types; and one `NestedConstructorArgumentProof` per outer argument position. Each nested receipt links the original outer argument and `Apply`, inner Counter `Apply`, selected constructor, `New`, and caller-formal leaf to their exact typed roots, original source positions, and resolved `Int` type. The two calls may share the canonical Counter class and constructor symbols, but their Apply/Select/New/leaf identities remain separate. Repeating `first` as both arguments is accepted when both source and typed roots prove that same symbol; the receipt still retains the declared `second` formal in its ordered method context.

At the #1914 revision, the paired planner ran this identity proof before any producer callback, then rejected a valid pair at `BatchUnit { phase: "two-argument constructor receipt integration", ... }` because the producer only carried singleton constructor argument bindings. That was the first paired-planner failure; body validation was not reached in that flow. The pre-existing validator also had explicit one-argument guards in both the outer constructor `Apply` and the planned primary-constructor/formal checks.

Focused tests cover the original fixture; swapped formal and expression order; repeated caller-formal use; renamed package/classes/method; declaration reordering; missing nested Select binding; and a forged leaf mapping that attempts to substitute the sibling's typed root. The latter two fail closed with `TypedBinding` errors. Source-invalid arity/type/duplicate-formal controls still fail in source typing. Unsupported deeper allocation and branch expressions fail because the leaf is not an original factory formal. Defaulted and generic factory methods are rejected by the earliest source/planning boundary. Secondary constructors remain rejected by existing method ownership checks. A source variation adding an overload currently fails earlier during method typing (`TypeNameNotFound`) and therefore does not exercise the proof's dedicated overloaded-method guard; no end-to-end overload-proof result is claimed.

Current paired proof checks:

```sh
cargo test -p dotty --test jvm_tasty_constructor_call_proofs --locked
cargo test -p dotty --test jvm_tasty_multi_argument_constructor_matrix --locked
```

Run all build, format, documentation, boundary, workspace, Scala-oracle, and mandatory JDK checks from the issue worker handoff; set `TMPDIR`, `CARGO_TARGET_DIR`, and JVM temporary storage to paths under the issue worktree.

## Ordered nested constructor body validation (#1915)

The one-session planner now adapts the immutable ordered proof into two
`NestedConstructorCallBinding` receipts. Each retains its own outer argument
root, nested `Apply`/`Select`/`New`/type tree/leaf bindings, inner constructor
formal, and actual caller formal named by that leaf. The pickler shares one
visited set and item budget across both branches, checks the outer constructor
and factory method arities, and validates the outer constructor's two
source-ordered reference formals as the same planned sibling class. Each inner
receipt remains a one-argument `Counter(Int)` constructor. The caller formal is
read from the proof instead of inferred from argument position, so formal
reordering, expression reordering, and authentic repeated use retain their
original identities.

The first body-validator failure after connecting the batch adapter was the
outer `Pair` primary-constructor guard: it required exactly one source formal
and reported `UnsupportedFeature::PrimaryConstructorInput` for the selected
constructor. After making that check use the observed `Apply` arity, the next
failure was the constructor signature/formal validator's one-formal predicate
(`MalformedType { role: ConstructorSignature }`). Both checks now accept two
formals only when each is a planned sibling reference to the same class and
the completed signature matches their source order. The exact original source,
renamed and declaration-reordered controls, swapped formal and expression
orders, and repeated-formal source all pass direct `validate_method_bodies()`.

The root matrix also corrupts the second receipt by removing the batch pair,
duplicating or reversing call-site identities, substituting a constructor or
caller formal, and reusing the first typed leaf. These forged receipts fail
validation. Third/deeper constructor expressions, branch arguments, invalid
outer arity, and unsupported default, generic, overloaded, secondary, or
contextual shapes remain rejected at their earlier measured gates. The prior
single-argument and source-local body validators remain covered by the focused
factory and pickler regressions.

This change validates bounded source/typed plan identity only. It does not add
expression wire serialization, change JVM code generation, or establish a
Scala-readable Rust-produced sidecar. The Scala 3.9 oracle artifacts and
independent consumer remain the unchanged baseline; class and TASTy emission
was not modified.

## Integrated JVM/TASTy batch acceptance (#1917)

Issue [#1917](https://github.com/scytrowski/dotty-rs/issues/1917) connects the
existing one-session ordered source proof and genuine TASTy writer to the JVM
artifact batch. The integration baseline is `6c4ec4609d8d76f30f69cf911caa4d927d00740d`.
Before this change, `emit_paired_jvm_tasty_batch` reached
`BatchArtifact { class_index: 2, phase: "cross-class invocation" }` for the
unchanged `PairFactory.scala` source and returned no artifact manifest.

The contract now admits exactly two sibling constructor arguments when the
original pair receipt proves their source and typed roots. It checks the outer
`Pair.<init>(LCounter;LCounter;)V`, two distinct `Counter.<init>(I)V` calls,
each original leaf formal and its JVM local slot, and the generated
`PairFactory.create(II)Lexample/Pair;` instructions. The bytecode reader
consumes the full straight-line sequence symbolically through `areturn`; it
checks both formal loads in order, all three allocations and constructor
Methodrefs, `max_stack=6`, `max_locals=3`, and empty exception metadata. The
source/TASTy plans and all class/TASTy links validate before the batch returns.
The new bytecode code is a validator for this bounded shape; class emission
continues to use the existing JVM backend.

The regression emits `Counter`, `Pair`, and `PairFactory` from one real source
frontend session and compares all six files across fresh sessions. Rust
artifact SHA-256 and linked TASTy UUIDs for the checked-in fixture source are:

| Artifact | SHA-256 | Linked UUID |
| --- | --- | --- |
| `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008071c3-c009-68ac-0000-000000008193` |
| `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | `008071c3-c009-68ac-0000-000000008193` |
| `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d-7441-c7f0-0000-000000008198` |
| `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | `000b7d8d-7441-c7f0-0000-000000008198` |
| `example/PairFactory.class` | `e4212a30e382a609ec64aa54e002900448e7323e68839ebb25fa2fccf054e1f4` | `0036155a-30ac-b6c8-0000-000000008198` |
| `example/PairFactory.tasty` | `ef6cc72805a9f3846d0cf31721cc06e88bb35716da1b0500eb6fad233b254836` | `0036155a-30ac-b6c8-0000-000000008198` |

The focused integration confirms classfile 65.0, TASTy 28.9.0, each
classfile-to-sidecar UUID, Pair's two private `Counter` fields and ordered
two-reference constructor ABI, and the exact factory method descriptor and
instructions. The existing TASTy regression checks separate `first` and
`second` PARAM references in its nested leaves. Renamed and
declaration-reordered producers remain deterministic across fresh sessions;
the authentic repeated-formal case also remains accepted.

Corruption controls reject a missing second inner initialization, swapped or
duplicated formal loads, wrong inner owner, wrong outer descriptor, truncated
class bytes, incorrect stack/local limits, missing class/sidecar paths,
duplicate or unsafe paths, malformed TASTy, and a stale UUID before any
accepted manifest. Paired-receipt regressions reject missing, duplicate,
reversed, or forged constructor/formal/typed-leaf bindings. The reused
test-only publisher injects failures after each of six staged writes, before
promotion, and after promotion; each handled failure leaves no final tree and
preserves an existing destination sentinel. These checks cover handled
failures and do not claim crash-atomic multi-file durability.

At the implementation checkpoint, `rustc --version` reported
`1.98.1 (48a229cea 2026-09-01)`. Focused passing commands were:

```sh
cargo test -p dotty --test jvm_tasty_artifacts sibling_constructor_batch --locked
cargo test -p dotty --test jvm_tasty_artifacts sibling_constructor_batches_are_deterministic_after_renaming_and_reordering --locked
cargo test -p dotty --test jvm_tasty_multi_argument_constructor_calls rust_batch_writer_emits_two_distinct_ordered_nested_constructor_receipts --locked
cargo test -p dotty --test jvm_tasty_multi_argument_constructor_matrix --locked
```

The standalone JDK 21 runtime and independent Scala reader remain separate
compatibility gates. Both passed in this worktree with Temurin `21.0.13+9`
and the pinned Scala checkout at `777528f19a58e794c9954a42f433373472ec57f8`.
The full workspace suite enabled `DOTTY_CLASSFILE_REQUIRE_JDK=1` and the
multi-argument, nested-constructor, and cross-class strict Scala consumer
requirements. It passed with `SCALA39_ROOT` set to that checkout and
`COURSIER_MAVEN_ROOT` set to the repository's cached Scala 3.9.0 dependencies.
The final `cargo fmt --all -- --check`, frontend boundary guard, workspace
Clippy with `-D warnings`, rustdoc with `-D warnings`, full workspace tests,
and `git diff --check` all passed. Neither compatibility gate substitutes
for the source, bytecode, or artifact proofs above.

## Strict sibling factory runtime identity (#1918)

Issue [#1918](https://github.com/scytrowski/dotty-rs/issues/1918) runs the
unchanged `PairFactory.scala` batch produced in one Rust source/typed session.
The runtime test is
`sibling_constructor_batch_executes_original_rust_artifacts_with_independent_pair_identity`
in `tests/jvm_tasty_artifacts.rs`; it writes the original six artifacts to one
class directory and compiles only the separate Java assertion harness. The
harness class directory precedes the producer directory on the classpath.
Neither Scala-produced classes nor replacement producer output participates.

The accepted artifacts remain classfile 65.0 and TASTy 28.9.0. SHA-256 values
and classfile-to-sidecar UUIDs observed before the Java client compile were:

| Artifact | SHA-256 | TASTy UUID |
| --- | --- | --- |
| `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008071c3-c009-68ac-0000-000000008193` |
| `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | `008071c3-c009-68ac-0000-000000008193` |
| `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d-7441-c7f0-0000-000000008198` |
| `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | `000b7d8d-7441-c7f0-0000-000000008198` |
| `example/PairFactory.class` | `e4212a30e382a609ec64aa54e002900448e7323e68839ebb25fa2fccf054e1f4` | `0036155a-30ac-b6c8-0000-000000008198` |
| `example/PairFactory.tasty` | `ef6cc72805a9f3846d0cf31721cc06e88bb35716da1b0500eb6fad233b254836` | `0036155a-30ac-b6c8-0000-000000008198` |

The independent Java runtime client's exact source is
[`PairFactoryIdentityHarness.java`](../tests/fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactoryIdentityHarness.java),
with SHA-256
`e3f173a04d86ebc746af15e779fe0f3a4b2d9a382b291f488a5606fced5bea90`.
`tests/jvm_tasty_artifacts.rs::sibling_constructor_batch_executes_original_rust_artifacts_with_independent_pair_identity`
copies these fixture bytes to its generated
`PairFactoryIdentityHarness.java` before invoking `javac`; the test checks and
prints that generated-file digest. Reproduce the source digest with
`sha256sum tests/fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactoryIdentityHarness.java`.
This client-source receipt is separate from the six Rust artifact hashes
above, the Scala consumer-source hash below, and the Java executable hash.

The test hashes those six files again after `javac`, after the JVM run, and
after all isolated negative copies. Every hash and original file byte matched
the pre-client value. `javap -v -p -c` reported major version 65 and the
expected `(II)Lexample/Pair;` factory descriptor.

The bytecode walker consumes the complete decoded `PairFactory.create` body;
the reported PCs come from that byte array, not constants in the test. It
observed `Pair` allocation at PC 0, separate `Counter` allocation sites at PCs
4 and 12, and the outer `Pair.<init>` call at PC 20. The exact order is
`new Pair; dup; new Counter; dup; iload_1; invokespecial Counter.<init>(I)V;
new Counter; dup; iload_2; invokespecial Counter.<init>(I)V; invokespecial
Pair.<init>(LCounter;LCounter;)V; areturn`. Both inner calls use the same
Counter constructor Methodref, while their `new` PCs remain distinct. The
decoded class-aware CFG analyzer accepts this sequence with the emitted
`max_stack=6` and `max_locals=3`, completing each inner allocation before the
outer constructor call.

Under `java -Xverify:all -ea`, the original Rust classes returned `7/3`,
`3/7`, and `-9/42` for the three input pairs. Test-only reflection confirmed
that each Pair's `left` and `right` fields reference distinct Counter objects,
and that all six Counters across the three calls are independent. Mutating the
first Pair's left then right Counters preserved the other field and both other
Pairs; mutating the second Pair's left Counter preserved the first and third
Pairs. No production getter was added.

### Negative first-failure matrix

The accepted manifest was kept separate from each mutated copy. Contract
corruptions of the two factory `iload`s, either inner `new`, either inner
`invokespecial`, outer-constructor order/descriptor, constructor Methodref
owner, and stack/local bounds fail at `BatchArtifact { class_index: 2, phase:
"cross-class invocation" }`; truncated Factory bytes fail earlier at `phase:
"class-file decode"`. The CFG analyzer independently reports
`TooManyLiveAllocations { instruction: 4, limit: 2 }` for a third pending
allocation and `UninitializedReferenceUse { instruction: 1, allocation_site:
0 }` for an uninitialized value stored to a local.

Separate JDK-only copies classify failures after the Rust contract test:

| Test-only copy | First observed failure | Classification |
| --- | --- | --- |
| Remove the second inner constructor instruction | `VerifyError` under `java -Xverify:all` | JVM verifier rejection of an uninitialized argument |
| Retarget `Pair.leftValue`'s `Counter.get:()I` Methodref owner to `java/lang/Object` | `NoSuchMethodError` | JVM linkage failure; the descriptor and verifier-compatible receiver shape remain valid |
| Swap the two `iload` operands | The JVM verifies and runs; the harness fails with `AssertionError: expected 7, got 3` | Runtime argument-order assertion, not `VerifyError` or `LinkageError` |

The swapped-load control is deliberately a runtime semantic failure. The
contract validator also rejects that copy before accepting an artifact
manifest, but it is not described as a JVM verifier error. These isolated
copies do not alter the six hashed producer artifacts.

The strict issue test is part of `.github/workflows/ci.yml` through the
existing `jvm_tasty_artifacts` target. With
`DOTTY_CLASSFILE_REQUIRE_JDK=1`, the test's exact-JDK-21 guard fails closed if
`java`, `javac`, or `javap` is missing or reports a different major version.
The run used Temurin `21.0.13+9-LTS`, classfile major 65, and Rust `1.98.1`.
The repository's Scala compatibility baseline remains pinned to revision
`777528f19a58e794c9954a42f433373472ec57f8`; no Scala producer or consumer ran
in this runtime-only acceptance. The independent Scala TASTy consumer remains
a separate compatibility gate.

The issue worktree ran the focused acceptance and preserved Pair-identity
tests, then the required strict JDK gate and repository checks. With
`JAVA_HOME=/home/vdrake6/.sdkman/candidates/java/21.0.13.0+9-tem`, its `bin`
on `PATH`, and `TMPDIR`/`CARGO_TARGET_DIR` beneath the issue worktree, the
commands were:

```sh
cargo test -p dotty --test jvm_tasty_artifacts sibling_constructor_batch --locked -- --nocapture
cargo test -p dotty --test jvm_tasty_dual_reference_identity --locked -- --nocapture
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo +stable test \
  -p dotty-classfile --test classfile_jvm_encoder \
  -p dotty-jvm --lib \
  -p dotty --test jvm_source_compilation \
  --test jvm_tasty_artifacts \
  --test jvm_tasty_dual_reference_identity \
  --test jvm_stateful_class_contract \
  --locked
cargo fmt --all -- --check
python3 tools/check-frontend-boundaries.py
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test --workspace --all-targets --locked
git diff --check
```

All commands passed with Rust `1.98.1`, Temurin JDK `21.0.13+9-LTS`, and
`DOTTY_CLASSFILE_REQUIRE_JDK=1` for the strict and workspace suites. The
full workspace test run bounded build and test concurrency to two jobs.

## Strict official Scala consumer of the Rust batch (#1919)

`tests/jvm_tasty_multi_argument_constructor_scala_consumer.rs` adds the
independent source-consumer gate. It sends the unchanged
[`PairFactory.scala`](../tests/fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactory.scala)
through one Rust frontend/typed session, writes its three classfiles and three
linked sidecars, then compiles only the separate
[`RustArtifactConsumer.scala`](../tests/fixtures/jvm_tasty_multi_argument_constructor_calls/RustArtifactConsumer.scala)
with official Scala 3.9.0. The producer source SHA-256 is
`7e40de1afb6272dbcf481baab7148801a055210216a19bdeaff85fafb40ff1bf`; the
separate consumer source SHA-256 is
`a773a39695f9e4b93abd1760dd75fb2a7854c64186d5e299ef2a92fd62b054e9`.

The run used Scala source revision
`777528f19a58e794c9954a42f433373472ec57f8`, reporting
`3.9.0-bin-SNAPSHOT-git-777528f`. The staged `with_compiler.jar` SHA-256 was
`172c8b8384e75ee24d327fce5ec788582bf314f66bf9983ba57e0cac6c3ff498`. It used
Temurin `21.0.13+9-LTS`, classfile major 65, and the Java executable SHA-256
`9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`. The two
published runtime libraries were SHA-256 verified before use:

| Runtime library | SHA-256 |
| --- | --- |
| `scala3-library_3-3.9.0.jar` | `9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba` |
| `scala-library-3.9.0.jar` | `b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd` |

The same JDK installation reported `javac 21.0.13` and `javap 21.0.13`; their
executable SHA-256 values were `2cc288a26beebc50ca7cd924847dffbfb38ef3314e105bba6d4ebce03a0bcb6b`
and `b2146e8dcfe5662c04d14f3fad68756a1a597d01461ca475a2cdbeb44c84a678`.

Consumer compilation used the original Rust output directory followed by those
two runtime libraries. Runtime used the separate Scala consumer output first,
then the original Rust output and the same libraries. The Scala `-from-tasty`
directory was inspection-only and never appeared on either classpath. Before
and after compile, execution, `-decompile`, `-print-tasty`, `-from-tasty`, and
the isolated mutation probes, all six original SHA-256 values matched:

| Rust artifact | SHA-256 | Linked TASTy UUID |
| --- | --- | --- |
| `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008071c3-c009-68ac-0000-000000008193` |
| `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | `008071c3-c009-68ac-0000-000000008193` |
| `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d-7441-c7f0-0000-000000008198` |
| `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | `000b7d8d-7441-c7f0-0000-000000008198` |
| `example/PairFactory.class` | `e4212a30e382a609ec64aa54e002900448e7323e68839ebb25fa2fccf054e1f4` | `0036155a-30ac-b6c8-0000-000000008198` |
| `example/PairFactory.tasty` | `ef6cc72805a9f3846d0cf31721cc06e88bb35716da1b0500eb6fad233b254836` | `0036155a-30ac-b6c8-0000-000000008198` |

With `java -Xverify:all -ea`, the Scala client called `create(7,3)`,
`create(3,7)`, and `create(-9,42)`, and checked all six resulting `Counter`
references were distinct. It mutated both members of one Pair and one member
of another Pair while checking the other values remained unchanged. Separate
direct `Counter` and `Pair` construction checked source-visible members. The
runtime output was
`rust-artifact-scala-consumer=7/3,3/7,-9/42; independent=true`.

The three original sidecars each passed official Scala `-decompile`,
`-print-tasty`, and the combined `-from-tasty` inspection. Factory decompilation
showed `create(first, second)` returning `Pair` with the ordered body
`new Pair(new Counter(first), new Counter(second))`. Per-class declaration
masking also proved that Scala's source lookup succeeds for `Counter.get` and
`set`, `Pair.leftValue`, `rightValue` and its two-reference constructor, and
`PairFactory.create(Int, Int): Pair`. Matched controls removed the TASTy marker
and sidecar; these failed with the corresponding source-level member or
constructor lookup errors. The Factory result probe also called a Pair member,
so its return type came through the selected source signature.

Official Pair decompilation is checked for the ordered constructor
`Pair(first: example.Counter, second: example.Counter)` and for both exact
`private[this] val` aliases: `left = Pair.this.first` and
`right = Pair.this.second`. This verifies the field mutability, visibility,
type, and source-formal binding rather than only the presence of the names.

### Scala consumer and reader negative results

The strict gate records the first observable rejection. Each sidecar control
was run separately for Counter, Pair, and PairFactory.

| Mutation | First rejecting stage and result |
| --- | --- |
| Missing or misplaced sidecar | First diagnostic names the selected `.class`, says it should have been loaded from a `.tasty` file, and reports that this `.tasty` file is missing |
| Truncated/corrupt sidecar | First diagnostic is `error while loading <class>,`; the following reader diagnostic names `<class>.tasty` and reports `is broken, reading aborted with class java.lang.ArrayIndexOutOfBoundsException` |
| Mismatched classfile/TASTy UUID | First diagnostic says the selected `.class` is out of sync with its TASTy and that TASTy was loaded; `-Werror` then reports `No warnings can be incurred under -Werror` |
| Wrong Pair constructor arity | Scala typechecker `[E171] Type Error` |
| Wrong second constructor argument type | Scala typechecker `[E007] Type Mismatch Error` |
| Consumer calls an absent method; a separate TASTy copy renames `create` to `delete` | Scala source lookup `[E008] Not Found Error` |
| Missing Counter classfile with authentic TASTy | Scala source compile succeeds; runtime first fails with `NoClassDefFoundError: example/Counter` |
| Pair constructor descriptor changed in a classfile copy | Scala source compile succeeds using TASTy; Java `-Xverify:all` first reports `VerifyError: Bad local variable type` |
| Factory Pair constructor Methodref changed to Counter's `(I)V` constructor | Scala source compile succeeds using TASTy; Java `-Xverify:all` first reports `VerifyError: Bad type on operand stack` |
| Nested Factory formal address points to an absent AST node | Consumer signature compilation stays lazy; official Scala `-decompile` forces `readTasty` and first throws `TreeWithoutOwner` |
| Two type-correct Factory formal target addresses swapped in a wire copy | Scala accepts it and decompiles `Counter(second), Counter(first)`; the test records this as accepted corruption, not a claimed rejection |

The last case demonstrates why accepting two `Int`-typed references does not
prove that their source identities were preserved. The original Rust TASTy
regression independently checks the nested leaves against the original
`create` formals in source order. Mutated sidecars and classfiles are isolated
copies; the six originals remain byte-identical.

The same strict run compiled and executed two independently emitted source
variants against their own Rust artifacts. The renamed variant changed the
package, class names, and factory method; producer source SHA-256 was
`133f7fe18adf9b8130f66292fad0b3648e766e8feabb896ca50893f9177a7057`, and
consumer SHA-256 was
`148a5a330ecc95430cc25f2b03466aad1fb9bcecbae702572e469068fb44fa74`. The
declaration-reordered producer source SHA-256 was
`9b83d2a63b2034e8ad81a955910864dff1c2e271ab900a991e27972a366420bb`; its
separate client used SHA-256
`a773a39695f9e4b93abd1760dd75fb2a7854c64186d5e299ef2a92fd62b054e9`. Both
Scala clients ran under the same verifier and identity assertions. Two fresh
Rust sessions of the unchanged source also matched byte-for-byte across all
six output files.

The strict test is wired into
`.github/workflows/jvm-tasty-factory-scala-consumer.yml` with
`DOTTY_REQUIRE_SCALA_TASTY_MULTI_ARGUMENT_CONSTRUCTOR_CONSUMER=1`; the flag
requires the pinned compiler, JDK, source revision, and runtime hashes instead
of skipping. That workflow still runs the inherited strict Scala consumers,
the standalone #1916 reader suite, the #1918 JDK runtime gate, and the
mandatory JDK 21 verification gate.

## Integrated release evidence for issue #1920

This closeout's final release gates were rerun from integrated `main` baseline
`e7f1941d072b705207f8e3606f50b9592c6ea826`. It contains prerequisites
#1913–#1919 and the intervening merged PRs #1950, #1953, and #1954. The initial
integrated verification used `d313e26facea1c7640272f535155e418fb59339c`;
its validation logs are not evidence for the final baseline. This issue adds
only a strict runtime
receipt normalization and this evidence ledger; it changes no compiler
semantics, ABI, or code generation. The release gate used the Scala 3.9.0 source revision
`777528f19a58e794c9954a42f433373472ec57f8`, staged compiler JAR SHA-256
`172c8b8384e75ee24d327fce5ec788582bf314f66bf9983ba57e0cac6c3ff498`, Temurin
JDK `21.0.13`, and the two published runtime library hashes recorded above.

The preceding issue evidence was reviewed and merged on these distinct heads:

| Prerequisite | Merged PR | Tested PR head | Merge commit |
| --- | --- | --- | --- |
| #1913 | #1923 | `da8ae07a1690af39f36df52625eb39061a46a009` | `b85bcd291d9af425bd3e439476b2a2c7fc03fe16` |
| #1914 | #1925 | `f41ff248b94aa340bdc02e6ae77ba378b4aa9dbe` | `2894d8c969e0d210b3f461d35e08d55daf6b2227` |
| #1915 | #1932 | `3db89c996e470e053ef1b08d8d3774224d3ce401` | `0e66ab92d5ec115236691a1ef6b2adb72f0e42fa` |
| #1916 | #1939 | `be24cc9478552684184b7326d72209d206dd3601` | `6c4ec4609d8d76f30f69cf911caa4d927d00740d` |
| #1917 | #1943 | `7cac3392a3541f1911f32fc85363552ad2f1c2ae` | `bf637fb9000914beee58038880f57dab5ab23154` |
| #1918 | #1947 | `3841713ea32feb1fa7e364e8972e3185da2eef2b` | `fd31a4f3dad7d0edac926997763e18637c323d0b` |
| #1919 | #1949 | `95e45e3d55b3b6a32d5987e79aa3a499843d3360` | `9198d8c8541cbb95930d89ba8521ef744ca13edc` |

Those head receipts belong to their prerequisite reviews. This section records
the integrated #1920 revalidation at `e7f1941d072b705207f8e3606f50b9592c6ea826`;
the final strict run reproduced the six original artifact hashes and UUID
links shown in the #1919 evidence above.

The acceptance criteria reconcile to the following evidence:

| Criterion | Evidence and result |
| --- | --- |
| AC-1 provenance | This ledger records the producer, independent Java and Scala client source hashes, pinned Scala source revision, compiler/JDK identity, published runtime library hashes, and six artifact receipts; the Java test checks the generated client against its recorded source digest. |
| AC-2 canonical semantic session | The existing same-session Rust regression checks source-to-typed roots, symbols, ordered parameters, and the exact supported proof boundary. It passed in the strict run. |
| AC-3 genuine original TASTy | The strict Scala consumer reads the six unmodified Rust-produced files; `-decompile`, `-print-tasty`, `-from-tasty`, selected source lookup, and runtime use the original artifacts. Inspection output is not substituted into the consumer classpath. |
| AC-4 artifact set, identity, and reproducibility | The three class/sidecar pairs, UUID links, paths, and hashes are recorded above. The gate checks two fresh Rust sessions produce identical six-file output and exercises renamed and declaration-reordered source variants. |
| AC-5 ordered bytecode contract | The symbolic bytecode contract and independent `javap` observations above cover `create(II)Lexample/Pair;`, left-to-right sibling construction, constructor descriptor, stack, locals, and argument count. The JDK verifier/runtime gate passed. |
| AC-6 strict Java runtime and mutation isolation | `java -Xverify:all -ea` ran the independent client and mutation probes. Original artifact hashes are checked before and after probes; runtime object identity and independent mutation assertions passed. |
| AC-7 official Scala source consumer and TASTy tools | The pinned Scala source consumer compiled against original Rust output, ran successfully, and each original sidecar passed official decompile, print, and from-TASTy inspection. Positive member lookup and matched classfile-only failures are recorded above. |
| AC-8 negative and publisher-fault matrix | First-stage sidecar, lookup, descriptor, and address failures are recorded above. The strict Rust tests cover producer rejection stages and injected publication faults, including preservation of an existing destination and absence of partial accepted output. |
| AC-9 strict toolchain and repository gates | All required Scala-consumer opt-ins, the oracle, JDK 21 gate, formatting, frontend boundary guard, Clippy, rustdoc, and `cargo test --workspace --all-targets --locked` ran with no required opt-in gate skipped. The regular suites retain their explicitly ignored tests. |
| AC-10 evidence and deferred boundaries | This ledger separates observed behavior from unsupported scope. The supported contract remains the exact two-argument direct-sibling fixture and bounded pending-object shape covered by the linked tests; the limits listed below remain deferred. |

The ordered checkpoints in the issue scope record completed against the final
baseline as follows: (1) integrated `main` and the prerequisite history were
reconciled; (2) the canonical producer session and six artifacts were verified;
(3) negative inputs and publication faults were exercised; (4) independent
Java and pinned Scala acceptance passed; (5) strict Scala, JDK, and repository
gates passed with all required opt-ins enabled; and (6) this ledger records the
evidence and scope limits. A runtime receipt initially included the JVM's `Picked up
JAVA_TOOL_OPTIONS:` banner before the expected linkage error. The harness now
removes that environment banner from only the two linkage-error receipts before
comparing and recording them; the underlying `NoSuchMethodError` and
`NoClassDefFoundError` assertions remain required.

This evidence does not expand support to arbitrary third arguments, conditional
branches, deeper pending-object nesting, generic or contextual constructors,
defaults, secondary constructors, cross-unit compilation, or a public compiler
session/CLI. A production semantic, ABI, or codegen failure is outside this
release closeout and must be returned to its owning prerequisite rather than
masked by changing this evidence.
