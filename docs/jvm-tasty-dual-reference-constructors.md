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

The typed alias `TermRef`s resolve to constructor-owned formals 52 and 53; the Scala TASTy `TERMREFsymbol`s instead point to template PARAM addresses 11 and 20. The source and typed expression roots are retained together in the test and manifest: `left` RHS `TreeId(7) → TreeId(0)`, `right` RHS `TreeId(25) → TreeId(1)`, `leftValue` RHS `TreeId(28) → TreeId(5)`, and `rightValue` RHS `TreeId(33) → TreeId(8)`. Both read roots retain the canonical Int result `TypeRef` and their exact field/method targets.

The separately invoked paired-batch convenience path parses its own session; its IDs are not combined with the preceding receipt. For the unchanged full source, its first rejection is:

```text
BatchClass { class_index: 1, phase: "TASTy plan validation",
  reason: "Unsupported { feature: PrimaryConstructorInput, symbol: Some(SymbolId(52)) }" }
```

The batch has already completed source typing, assembled and validated the JVM unit metadata, and passed its cross-class constructor-call identity proof before this pickler signature rejection. It emits no paired artifact. The field-alias-specific pickler check and TASTy lowering are not reached, so this audit does not invent downstream errors. The existing JVM-only regression `object_graph_construction_audit_emits_after_frontend_identity_validation` independently compiles the bounded two-reference object graph and executes its separate-field behavior under the required JDK gate; it does not claim paired Rust output.

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
| constructor `val` or `var` accessors | valid | JVM unit validation: `AccessorFieldSignature` |
| more than two reference formals | valid | TASTy plan validation: `PrimaryConstructorInput` |
| third distinct reference field | valid | TASTy plan validation rejects the extra constructor input first; field validation is masked |
| no fields | valid signature-only control | TASTy plan validation: `PrimaryConstructorInput` |
| `Pair(first, first)` | valid intentional alias | separate consumer confirms both fields share the input |
| missing/stale typed binding | not a Scala source variation | not reached; the canonical two-formal signature rejects earlier. No typed node or foreign identity is fabricated. |
| missing TASTY link / malformed sidecar / stale UUID | binary controls | focused test observes `MissingTastyLink`, malformed TASTy decode failure, and `StaleTastyLink`; existing publisher regressions ensure incomplete batches are not published |

The existing regression `constructor_alias_batch_publication_rejects_invalid_pairs_and_partial_results` and `malformed_second_sidecar_fails_before_creating_staging_or_destination` cover missing artifact paths, corrupt sidecars, and no partial publication. The matrix and all stage distinctions are summarized in `oracle.json`.

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

The audit also ran the JVM-only reference graph test with `DOTTY_CLASSFILE_REQUIRE_JDK=1`. No producer implementation or enablement is included in this audit.
