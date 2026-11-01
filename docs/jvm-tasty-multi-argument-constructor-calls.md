# Two-argument sibling constructor-call audit

This is an evidence-only audit for [issue #1913](https://github.com/scytrowski/dotty-rs/issues/1913). It adds no source typing, JVM, TASTy, paired artifact, or public API support. The checked-in Scala artifacts are output from Scala 3.9.0; they are not Rust output and do not establish Rust-to-Scala interoperability.

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
