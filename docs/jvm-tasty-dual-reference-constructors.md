# Dual-reference constructor ABI and TASTy audit

This is an evidence-only audit of issue [#1874](https://github.com/scytrowski/dotty-rs/issues/1874). It adds no JVM or TASTy producer behavior. The Scala artifacts below are compiler-generated oracle fixtures; they are not Rust output and do not establish Rust-to-Scala interoperability.

## Pinned oracle

The producer is the one-file source [`Pair.scala`](../tests/fixtures/jvm_tasty_dual_reference_constructors/Pair.scala). It declares `Counter` followed by `Pair`; the source is SHA-256 `06266874ed9a7acdfc4d6a1c27bea3967055e474bca5a2f666af5ebfc8669c28`. The exact signature-only control is [`PairSignatureOnly.scala`](../tests/fixtures/jvm_tasty_dual_reference_constructors/PairSignatureOnly.scala). The independent Scala client is [`Consumer.scala`](../tests/fixtures/jvm_tasty_dual_reference_constructors/Consumer.scala), compiled separately against the producer output.

The compiler checkout is pinned at Scala 3.9.0 source revision `777528f19a58e794c9954a42f433373472ec57f8`; `scalac` reports `3.9.0-bin-SNAPSHOT-git-777528f`. The staged compiler JAR SHA-256 is `b56626390028bd06347e3549cf271a03ca7d2e7be491ceff630ccee94e439f4f`. The run used SDKMAN Eclipse Temurin `21.0.13+9-LTS`; classfiles use major version 65. JDK executable and Scala runtime hashes are recorded in [`oracle.json`](../tests/fixtures/jvm_tasty_dual_reference_constructors/oracle.json).

Both compilations ran into empty task-local directories with `-release 21`. The full source emitted exactly four files: `example/Counter.class`, `example/Counter.tasty`, `example/Pair.class`, and `example/Pair.tasty`. The signature-only source emitted the corresponding four-file pair in its own directory. Every classfile links its paired TASTy UUID, every sidecar is TASTy 28.9.0, and the manifest records the per-file size and SHA-256. The raw `javap -v -p -c` and official Scala `-print-tasty` snapshots are checked in under [`oracle/`](../tests/fixtures/jvm_tasty_dual_reference_constructors/oracle/).

## Observed JVM ABI

Both classes are public ordinary classes (`ACC_PUBLIC | ACC_SUPER`). `Counter` has one `private` mutable `current:I` field, a public `<init>(I)V`, public `get()I`, and public `set(I)V`. `Pair` has exactly two physical fields, both `private final Lexample/Counter;`, plus a public constructor and two public reads:

| Member | JVM descriptor | Flags |
| --- | --- | --- |
| `Pair.left` | `Lexample/Counter;` | `ACC_PRIVATE | ACC_FINAL` |
| `Pair.right` | `Lexample/Counter;` | `ACC_PRIVATE | ACC_FINAL` |
| `Pair.<init>` | `(Lexample/Counter;Lexample/Counter;)V` | `ACC_PUBLIC` |
| `Pair.leftValue` | `()I` | `ACC_PUBLIC` |
| `Pair.rightValue` | `()I` | `ACC_PUBLIC` |

The `Pair` constructor receives `this` in local 0, `first` in local 1, and `second` in local 2. After `Object.<init>`, Scala stores `aload_1` into `left`, then `aload_2` into `right`. `leftValue` reads `left` and invokes `Counter.get`; `rightValue` reads `right` and invokes the same method. The signature-only control keeps the two-reference constructor descriptor but has no fields or methods.

## TASTy structure

The official Scala 3.9 `-print-tasty` dump of `Pair.tasty` reports TASTy `28.9.0` and UUID `007fabc3-ccf4-25d3-0043-37297354695b`, matching `Pair.class`:

| Address | Declaration | Observed relationship |
| ---: | --- | --- |
| 11 | template `PARAM first: Counter` | `PRIVATE`, `LOCAL`; type is `Counter` |
| 20 | template `PARAM second: Counter` | `PRIVATE`, `LOCAL`; shares the same `Counter` type tree |
| 39 | primary constructor `DEFDEF` | Owns two distinct constructor formals |
| 42 | constructor `PARAM first: Counter` | Separate from template PARAM at 11 |
| 49 | constructor `PARAM second: Counter` | Separate from template PARAM at 20 |
| 60 | `VALDEF left` | `PRIVATE`, `LOCAL`; RHS `TERMREFsymbol` address 67 targets template PARAM 11 |
| 76 | `VALDEF right` | `PRIVATE`, `LOCAL`; RHS `TERMREFsymbol` address 83 targets template PARAM 20 |
| 89 | `DEFDEF leftValue` | Body selects `left` at 104, then signed `Counter.get(): Int` |
| 113 | `DEFDEF rightValue` | Body selects `right` at 126, then the same signed `Counter.get(): Int` |

These are per-file TASTy addresses, not stable IDs. The complete wire dump is checked in as `oracle/Pair.tasty.txt`; the binary decoder regression checks both classfile/TASTy UUID links, major versions, constructor descriptor, field flags and order, and the four constructor `PARAM` addresses.

## Independent Scala client

[`Consumer.scala`](../tests/fixtures/jvm_tasty_dual_reference_constructors/Consumer.scala) is compiled to a different output directory against only the Scala-produced classes and pinned Scala libraries. Under `java -Xverify:all -ea`, it constructs `Counter(7)`, `Counter(3)`, and `Pair(first, second)`, mutates each counter independently, and observes that only its corresponding field changes. It then constructs `Pair(second, first)` and verifies that argument positions determine `left` and `right`. Finally, `Pair(first, first)` confirms that identical inputs intentionally alias both fields. The recorded output is `scala39-dual-reference-constructor=ok`.

This validates Scala-produced output only. The separate client also confirms Scala compiler errors for wrong arity, a non-reference second argument, and a same-spelled `foreign.Counter` where `example.Counter` is required.

## One-session Rust observation and first rejection

`rust_frontend_session_keeps_two_parameter_and_field_alias_identities` runs the exact `Pair.scala` source through Lexer → Parser → Namer → SourceTyper once, under `SourceId(1874)`. It asserts canonical declaration provenance, owner scopes, distinct template and constructor parameter symbols, both field initializers’ same-session typed roots, and the exact field and `Counter.get` selections in both typed read bodies. These arena IDs are receipts from this test run only:

| Source declaration | Run-local identity | Owner / binding |
| --- | --- | --- |
| `Counter` | `SymbolId(40)` | source class |
| `Pair` | `SymbolId(41)` | source class |
| `Pair.<init>` | `SymbolId(51)` | owner is `Pair` |
| template `first`, `second` | `SymbolId(49)`, `SymbolId(50)` | owner is `Pair`; canonical source declarations |
| constructor `first`, `second` | `SymbolId(52)`, `SymbolId(53)` | owner is `Pair.<init>`; derived declarations |
| `left`, `right` fields | `SymbolId(54)`, `SymbolId(55)` | owner is `Pair`; field types point to `Counter` |
| `leftValue`, `rightValue` | `SymbolId(56)`, `SymbolId(57)` | select fields 54/55 and `Counter.get` (`SymbolId(46)`) |

The typed field initializer `TermRef`s resolve to constructor-owned formals 52 and 53; Scala's TASTy `TERMREFsymbol`s instead point to template PARAM addresses 11 and 20. The exact session's named source-to-typed roots are:

| Source member | Symbol / selected field | Source RHS | Typed RHS |
| --- | --- | ---: | ---: |
| `Counter.current` initializer | field `SymbolId(45)` | `TreeId(4)` | `TreeId(0)` |
| `Counter.get` body | method `SymbolId(46)`, reads field 45 | `TreeId(7)` | `TreeId(1)` |
| `Counter.set` body | method `SymbolId(47)` | `TreeId(14)` | `TreeId(4)` |
| `Pair.left` initializer | field `SymbolId(54)`, aliases constructor formal 52 | `TreeId(25)` | `TreeId(5)` |
| `Pair.right` initializer | field `SymbolId(55)`, aliases constructor formal 53 | `TreeId(28)` | `TreeId(6)` |
| `Pair.leftValue` body | method `SymbolId(56)`, selects field 54 and `Counter.get` 46 | `TreeId(33)` | `TreeId(9)` |
| `Pair.rightValue` body | method `SymbolId(57)`, selects field 55 and `Counter.get` 46 | `TreeId(38)` | `TreeId(12)` |

Both Pair reads retain the canonical Int result `TypeRef` (`TypeId(5)`) and their exact field/method targets. These arena IDs are run-local receipts; the named edges, not allocation numbers, explain which roots correspond.

## Ordered JVM formal proof (#1875)

`JvmPrimaryConstructorPlan::prove_ordered_reference_formals` now builds an immutable, read-only receipt for an ordinary two-formal constructor when both source formals are direct type identifiers for their validated same-batch reference targets. It resolves each original constructor `ValDef` through both index paths: `symbol_at(source, tree)` for the class-owned canonical parameter and `derived_symbol_at(constructor, source, tree)` for the distinct constructor-owned formal. It requires two unique IDs at source ordinals 0 and 1, correct source origins and owners, ordinary unannotated/no-default parameter syntax, exact type syntax for the canonical target, matching completed template and constructor signatures, and the constructor scope's exact formal membership. The canonical target must be the same public, source-owned, package-owned class for both parameters. Package prefixes are checked with the JVM identity layer's canonical package rule; separate `TypeId`s are allowed and are not treated as distinct classes.

The receipt includes both source trees, both ordered symbol pairs, the existing descriptor and its JVM parameter locals. It verifies that the complete JVM metadata remains `(Lexample/Counter;Lexample/Counter;)V`, with receiver slot 0 and category-one reference slots 1 and 2. The batch planner completes the canonical class-owned formal signatures through the existing `SourceTyper` before running this proof. It enforces the proof after whole-unit JVM metadata validation and before cross-class proof/TASTy validation. Reversed, repeated, omitted, or foreign constructor-formal IDs supplied to `JvmPrimaryConstructorPlan::from_source` fail at `ParameterIdentity`; a source with a different same-batch class in the second slot fails at the ordered formal proof stage.

The full-source regression inspects the receipt from the original one-file frontend session and compares its JVM metadata to the existing JVM-only `Pair` plan. Explicit test-only negatives mutate one source formal's type syntax or modifiers and one completed constructor signature type; each fails the proof. Reversed, duplicate, missing, and foreign-owner parameter ID sequences fail `from_source` at `ParameterIdentity`. A distinct second reference class in the same package fails at the ordered proof boundary. The signature-only control uses renamed package/class/parameter names in one source session; it passes the same proof before reaching the unchanged TASTy constructor guard. In both paired positive cases the first TASTy failure remains `Unsupported(PrimaryConstructorInput)`. That rejection still occurs before field-alias validation or TASTy lowering; no paired artifact or new TASTy bytes are produced. The emitter and the checked Scala-produced fixture snapshots are unchanged.

### Reachable JVM-only and class-state controls from the same session

The identity test builds `JvmCompilationUnit` directly from its still-live `SemanticStore`, parsed AST, `SourceSemanticIndex`, `SourceTypedIndex`, and typed roots. It calls the existing unit metadata validator and emits both classes through `dotty_jvm::emit_class`; it does not parse or type the source again. The validated order is `example/Counter` then `example/Pair`; emitted JVM-only classfiles are major 65, with the Pair constructor descriptor `(Lexample/Counter;Lexample/Counter;)V`. Its constructor formal slots are `first` (`SymbolId(52)`) at local 1 and `second` (`SymbolId(53)`) at local 2. The two fields remain in source order: `left` (`SymbolId(54)`) and `right` (`SymbolId(55)`), both `Lexample/Counter;`. The emitted test artifacts are `Counter.class` (259 bytes) and `Pair.class` (384 bytes). The Pair constructor bytecode uses `aload_1` before its first `putfield`, then `aload_2` before its second `putfield`, matching the validated parameter slots and source field order. These are Rust JVM-only classfiles and carry no TASTy sidecars.

A separate direct `ClassStatePicklePlan` in that same live session borrows the same store, definitions, source AST/index, typed AST, and source-to-typed bindings. It reaches the real primary-constructor signature guard and returns `Unsupported { feature: PrimaryConstructorInput, symbol: Some(SymbolId(52)) }`. This is the class-state constructor-signature first failure. Field-alias validation and TASTy lowering are not reached; no downstream error is manufactured.

The reachable-stage results are recorded independently:

| Entry point / stage | Result |
| --- | --- |
| Original Lexer → Parser → Namer → SourceTyper session | Passes; all seven named initializer/body roots remain bound in the original `SourceTypedIndex` |
| Borrowed JVM compilation-unit metadata | Passes for `Counter` then `Pair`; constructor slots and source field/method order are validated |
| JVM-only class emission from that unit | Passes for both classes; classfile major 65, no TASTy sidecars |
| Direct borrowed class-state constructor signature | First rejection: `Unsupported(PrimaryConstructorInput, SymbolId(52))` |
| Class-state field-alias validation | Not reached after the constructor-signature guard |
| TASTy lowering | Not reached; no downstream error is inferred |
| Paired batch convenience path | Independently reparses the source and reaches its own first TASTy plan validation rejection below |

The separately invoked paired-batch convenience path parses its own session; its IDs are not combined with the original identity receipt. For the unchanged full source, its independently measured first rejection is:

```text
BatchClass { class_index: 1, phase: "TASTy plan validation",
  reason: "Unsupported { feature: PrimaryConstructorInput, symbol: Some(SymbolId(52)) }" }
```

That path completes source typing, JVM unit metadata validation, and the cross-class constructor-call identity proof before the pickler signature rejection, then emits no paired artifact. The direct and convenience paths therefore agree on the current constructor-input boundary while retaining distinct session provenance.

## Variation matrix

All source variants are checked in under [`source-variants/`](../tests/fixtures/jvm_tasty_dual_reference_constructors/source-variants/). The strict Scala test classifies each with the pinned compiler, and the Rust matrix test asserts its actual first paired-planner stage and error class.

| Variation | Scala 3.9 result | Rust paired first result |
| --- | --- | --- |
| wrong second field type | compiler type error | field initializer typing: `ExpectedExpressionTypeMismatch` |
| one-argument `new Pair(first)` | compiler arity error | separate consumer negative; no producer call added to Rust source |
| swapped positional inputs | valid | separate Scala consumer confirms positional field behavior |
| duplicate formal name | compiler error | field initializer typing: `AmbiguousTermReference` |
| same-spelled foreign `Counter` owner | compiler type error (`foreign.Counter` vs `example.Counter`) | checked as a separate Scala client, not projected into the one-package Rust batch |
| reverse class order | valid | TASTy plan validation: `PrimaryConstructorInput` for the first planned class |
| reordered parameter names | valid | TASTy plan validation: `PrimaryConstructorInput` |
| wrong field-to-formal alias | valid Scala source | same earlier TASTy `PrimaryConstructorInput`; alias validation is masked |
| secondary constructor | valid | member planning: secondary constructor is not an owned planned method |
| generic primary constructor | valid | source collection: generic primary constructors unsupported |
| default or contextual formal | valid | constructor planning: `InvalidPrimaryConstructor(SourceShape)` |
| constructor `val` or `var` accessors | valid | complete JVM unit validation: `AccessorFieldSignature` |
| more than two reference formals | valid | TASTy plan validation: `PrimaryConstructorInput` |
| third distinct reference field | valid | TASTy plan validation rejects the extra constructor input first; field validation is masked |
| no fields | valid signature-only control | TASTy plan validation: `PrimaryConstructorInput` |
| `Pair(first, first)` | valid intentional alias | separate consumer confirms both fields share the input |
| test-only source type or completed constructor signature corruption | not a Scala source variation | ordered constructor proof: source/template/formal type mismatch |
| missing/stale typed binding | not a Scala source variation | not reached; the canonical two-formal signature rejects earlier. No typed node or foreign identity is fabricated. |
| missing TASTY link / malformed sidecar / stale UUID | binary controls | focused test observes `MissingTastyLink`, malformed TASTy decode failure, and `StaleTastyLink`; existing publisher regressions ensure incomplete batches are not published |

The existing regression `constructor_alias_batch_publication_rejects_invalid_pairs_and_partial_results` and `malformed_second_sidecar_fails_before_creating_staging_or_destination` cover missing artifact paths, corrupt sidecars, and no partial publication. The matrix and all stage distinctions are summarized in `oracle.json`.

## Issue #1876: bounded signature-only Rust producer

The producer increment starts from merged main commit `2b4f666427ec0b0eeb76c09254d5a39abd6abdb5`. It enables exactly two distinct ordered reference formals when both resolve to the same public class planned in the same source batch. The retained identity contract is the one-session proof above: class-owned template PARAM symbols and constructor-owned PARAM symbols remain distinct, source ordinals and MethodType order remain first/second, and the JVM constructor descriptor is `(Lexample/Counter;Lexample/Counter;)V` with parameter slots 1 and 2.

Rust now emits the signature-only `PairSignatureOnly.scala` sidecars with two template `PARAM`s (`PRIVATE`, `LOCAL`) and two constructor `PARAM`s. All four type trees resolve to the planned `Counter`; the class contains no fields and only its primary constructor. The decoder regression checks the ordered names and type trees. A strict Scala 3.9.0 run on those genuine Rust-produced `Counter.tasty` and `Pair.tasty` files passed both `-decompile` and `-from-tasty`; `javap` on the rebuilt Pair reports `(Lexample/Counter;Lexample/Counter;)V` and no fields. This evidence is limited to signature-only Pair and does not add dual-reference field aliases or method bodies.

The strict run used Scala source revision `777528f19a58e794c9954a42f433373472ec57f8`, `scalac 3.9.0-bin-SNAPSHOT-git-777528f`, staged compiler JAR SHA-256 `b56626390028bd06347e3549cf271a03ca7d2e7be491ceff630ccee94e439f4f`, and SDKMAN Eclipse Temurin JDK `21.0.13+9-LTS` (classfile major 65). The authoritative `PairSignatureOnly.scala` source SHA-256 is `6d58bf7b174153418b54d9bdc8991448bf1923d66be57cd226f94b29155bb5a6`. The mandatory `jvm-tasty-factory-scala-consumer` workflow runs `jvm_tasty_dual_reference_signature` with `DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_SIGNATURE=1` and the same JDK pin.

The producer's current first-rejection behavior remains explicit:

| Input | First rejection |
| --- | --- |
| Exact two-reference signature with no fields or methods | Accepted by TASTy plan validation and emitted |
| Full Pair dual-reference fields/methods source | TASTy plan validation: `PrimaryConstructorInput`; dual-reference member emission remains out of scope |
| Three reference formals | TASTy plan validation: `PrimaryConstructorInput` |
| One reference plus one `Int` formal | TASTy plan validation: `PrimaryConstructorInput` |
| Two references to distinct batch classes | Ordered constructor formal proof: `OrderedReferenceFormalIdentity` |
| Duplicate formal names | Field initializer typing: `AmbiguousTermReference` (the full source fixture reaches this before TASTy validation) |
| Wrong second type in the full field-alias fixture | Field initializer typing: `ExpectedExpressionTypeMismatch` |
| Default or contextual formal | Constructor planning: `InvalidPrimaryConstructor(SourceShape)` |
| Secondary constructor | Member planning: unsupported secondary method ownership |
| Constructor `val` or `var` accessors | Complete JVM unit validation: `AccessorFieldSignature` |

The exact positive and negative Rust producer regressions run in `jvm_tasty_dual_reference_signature`, `jvm_tasty_dual_reference_session`, and `jvm_tasty_dual_reference_matrix`. The strict Rust-side Scala gate uses `DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_SIGNATURE=1`, plus `SCALA39_ROOT` and `JAVA_HOME`; it writes to the caller's task-local `TMPDIR`.

## Reproduction

Use the pinned Scala checkout and explicit SDKMAN Temurin 21 paths. Keep temporary files and Rust outputs under the task worktree:

```sh
export SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8
export JAVA_HOME=/home/vdrake6/.sdkman/candidates/java/21.0.13.0+9-tem
export PATH="$JAVA_HOME/bin:$PATH"
export TMPDIR="$PWD/target/issue-1874/tmp"
export JAVA_TOOL_OPTIONS="-Djava.io.tmpdir=$TMPDIR -XX:-UsePerfData"
SCALAC="$SCALA39_ROOT/dist/target/universal/stage/bin/scalac"
SCALA_LIB="$SCALA39_ROOT/dist/target/universal/stage/maven2/org/scala-lang/scala3-library_3/3.9.0-bin-SNAPSHOT/scala3-library_3-3.9.0-bin-SNAPSHOT.jar:$SCALA39_ROOT/dist/target/universal/stage/maven2/org/scala-lang/scala-library/3.9.0-bin-SNAPSHOT/scala-library-3.9.0-bin-SNAPSHOT.jar"
mkdir -p target/issue-1874/oracle/producer target/issue-1874/oracle/consumer
test -z "$(find target/issue-1874/oracle/producer -mindepth 1 -print -quit)"
"$SCALAC" -release 21 -d target/issue-1874/oracle/producer \
  tests/fixtures/jvm_tasty_dual_reference_constructors/Pair.scala
"$SCALAC" -release 21 \
  -classpath "target/issue-1874/oracle/producer:$SCALA_LIB" \
  -d target/issue-1874/oracle/consumer \
  tests/fixtures/jvm_tasty_dual_reference_constructors/Consumer.scala
"$JAVA_HOME/bin/java" -Xverify:all -ea \
  -classpath "target/issue-1874/oracle/consumer:target/issue-1874/oracle/producer:$SCALA_LIB" \
  example.DualReferenceConsumer
"$JAVA_HOME/bin/javap" -v -p -c -classpath target/issue-1874/oracle/producer example.Pair
```

The Scala TASTy printer uses the staged distribution classpath, including its staged `tasty-core` JAR:

```sh
SCALA_CP="$(find "$SCALA39_ROOT/dist/target/universal/stage/maven2" -type f -name '*.jar' -print | paste -sd:):$SCALA39_ROOT/library/target/scala-library-bootstrapped/scala-library-3.9.0-bin-SNAPSHOT.jar:$SCALA39_ROOT/library/target/scala3-library-bootstrapped/scala-3.9.0-bin-SNAPSHOT-nonbootstrapped/scala3-library_3-3.9.0-bin-SNAPSHOT.jar:$SCALA39_ROOT/tasty/target/tasty-core-bootstrapped/scala-3.9.0-bin-SNAPSHOT-nonbootstrapped/tasty-core_3-3.9.0-bin-SNAPSHOT.jar"
"$JAVA_HOME/bin/java" -cp "$SCALA_CP" dotty.tools.MainGenericCompiler \
  -print-tasty target/issue-1874/oracle/producer/example/Pair.tasty
```

The strict Rust tests reproduce the Scala artifacts, consumer, and source matrix when `DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_CONSUMER=1` is set:

```sh
export CARGO_TARGET_DIR="$PWD/target/issue-1874/cargo"
DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_CONSUMER=1 \
SCALA39_ROOT="$SCALA39_ROOT" JAVA_HOME="$JAVA_HOME" \
  cargo test -p dotty \
    --test jvm_tasty_dual_reference_oracle \
    --test jvm_tasty_dual_reference_scala_consumer \
    --test jvm_tasty_dual_reference_session \
    --test jvm_tasty_dual_reference_matrix \
    --test jvm_tasty_dual_reference_variations_scala \
    --locked -- --nocapture
```

The audit also ran the JVM-only reference graph test with `DOTTY_CLASSFILE_REQUIRE_JDK=1`.

## Issue #1877: ordered two-field alias producer

Starting from merged main `811780450ade51633a5f272195aaafd3a77dec0e`, the
class-state validator now admits the exact full `Pair.scala` shape after it
authenticates two source-ordered field receipts. Each field keeps its exact
source and typed initializer, canonical same-batch `Counter` TypeRef, owner,
source position, and typed constructor-owned `TermRef`. The validator resolves
that formal through the constructor's ordered source parameter trees, then
retains the corresponding class-owned template parameter used by Scala's
TASTy field initializer. It requires the original field order to map to
`left ← first` and `right ← second`, with distinct aliases, exactly two such
fields, and exactly two methods. The existing serializer consumes each field's
receipt independently and keeps the two read bodies bound to their original
field and `Counter.get` symbols.

The Rust decoder regression checks the four distinct template/constructor
`PARAM`s, two `VALDEF`s in source order, `PRIVATE` and `LOCAL` modifiers,
`Counter` type trees, and the two distinct `TERMREFsymbol` targets. It also
walks each read method's emitted tree and confirms that `leftValue` selects
`left` before signed `get`, while `rightValue` selects `right` before `get`.
No classfile or combined Scala ABI claim is made by this producer test.

The strict producer test generated both sidecars from the original single
Rust frontend session, then the pinned Scala 3.9.0 compiler successfully ran
`-decompile` on Rust `Pair.tasty` and `-from-tasty` on Rust `Counter.tasty` and
`Pair.tasty`. `javap` on the rebuilt Pair confirmed the two private final
Counter fields and constructor descriptor
`(Lexample/Counter;Lexample/Counter;)V`. Inputs were Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`, compiler
`3.9.0-bin-SNAPSHOT-git-777528f`, Temurin JDK `21.0.13+9-LTS`, TASTy 28.9.0,
and Rust 1.98.1. The separate Java runtime/client check remains outside this
issue.

Negative paired-plan first failures are asserted at their actual gates:

| Input | First observed rejection |
| --- | --- |
| `left ← second`, `right ← first` | TASTy plan validation: `UnsafeFieldInitializer` (class-level ordered alias mismatch) |
| both fields reuse `first` | TASTy plan validation: `UnsafeFieldInitializer` (duplicate alias does not satisfy the ordered pair) |
| source fields reordered without changing their RHSs | TASTy plan validation: `UnsafeFieldInitializer` (receipt order differs from formal order) |
| third reference alias field | TASTy plan validation: `ClassMembers` (outside exact two-field shape) |
| mutable alias with reads | method body typing: `UnstableSelectionPrefix` |
| public alias field | member planning: only private fields are supported |
| stale right-field typed initializer plan binding | direct class-state validation: `FieldInitializerBinding` |
| right field declared as `Int` while source says `Counter` | direct class-state validation: `FieldType` |

The signature-only dual-reference fixture still follows the #1876 path and
preserves its previous output. Existing one-reference alias and primitive
field byte snapshots are covered by the unchanged focused class-state tests.
The strict Scala regression is part of
`.github/workflows/jvm-tasty-factory-scala-consumer.yml` under
`DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_ALIAS=1`.
