# Mixed-origin Pair constructor arguments audit (#2011)

## Integrated release-gate ledger (#2018)

This section records the fresh integrated run required by #2018. Its baseline
is `main` at `cfaa8c1160cd190113b867b77d3644357d691a6f` (merged #2052), fetched
from GitHub on 2026-11-03. The issue worktree was clean before creating the
issue branch. All seven prerequisite merge commits are ancestors of this
baseline. The receipts below are historical prerequisite evidence; the
commands and results later in this section were rerun against the integrated
baseline and are the release-gate evidence.

| Prerequisite | PR head reviewed | Merge commit | Final review | Successful PR CI / Scala run |
| --- | --- | --- | --- | --- |
| #2011 / #2020 | `3fa7da8814cf66314987df3f6769646aaea2cb2d` | `ace9c50af2df7050846518f96464c48574c05a20` | `READY_FOR_MERGE` | `41491828053` / `41491828041` |
| #2012 / #2023 | `1dc234bd5e2a8a5d018de9085d27ee1af0754e2a` | `ac403ef0b8af2d263cd5e1680a2150e5f6517de3` | `READY_FOR_MERGE` | `41515579857` / `41515579879` |
| #2013 / #2032 | `e3778e27f74aea351d193cdaf2b40273502130f7` | `1fbc0ac64cef9d8c4b7e5a16d4fe7a1e753f2a6d` | `READY_FOR_MERGE` | `41530527659` / `41530527654` |
| #2014 / #2042 | `79fa8b7930da4db76451f10c4b489641e197abfe` | `ff2f400f19a6b805ef8c2b20766dda0cf84d02f2` | `READY_FOR_MERGE` | `41542276284` / `41542276334` |
| #2015 / #2045 | `4c54c7210b6c1cf446747a3f94e3883ef8b49d6d` | `5cb16a5868c53071e9fcbc08370df688a5378ed0` | `READY_FOR_MERGE` | `41553616952` / `41553616867` |
| #2016 / #2049 | `063f658bb7f65ca587243e5ecb8a6ae867073fa9` | `05de4d04196041b6ef442e7d8716511f8a148a0b` | `READY_FOR_MERGE` | `41591752299` / `41591752294` |
| #2017 / #2053 | `df32a8629dcbd5e2fbbff6a34bbfe9ffaa64ce39` | `2b7ccbf87c9c2b4771aeb4bc01a0236547edf572` | `READY_FOR_MERGE` | `41603443906` / `41603443829` |

All final review submissions were published on their reviewed heads. The CI
and Scala workflow runs for each prerequisite head completed successfully;
none is substituted for integrated-main validation below.

### AC1 — frozen baseline and toolchain

| Input | Integrated-run receipt |
| --- | --- |
| Rust | `rustc 1.98.1 (48a229cea 2026-09-01)`; `cargo 1.98.1 (797e8a9bc 2026-08-05)` |
| Scala source | `777528f19a58e794c9954a42f433373472ec57f8`; Scala `3.9.0-bin-SNAPSHOT-git-777528f`; TASTy `28.9.0` |
| Java | Temurin `21.0.13+9`; `java` SHA-256 `9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`; `javac` and `javap` report `21.0.13`; classfile major `65` |
| Staged compiler | `with_compiler.jar` SHA-256 `e22501460f3d2b272d2dd93666d7229950c6929aa681cc2eb47a1e2946eef161`; this identifies this local sbt build, not a deterministic compiler constant |
| Published Scala runtime | `scala3-library_3-3.9.0.jar`: `9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba`; `scala-library-3.9.0.jar`: `b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd` |

Frozen producer and independent consumer inputs used by the strict Scala run:

| Input | SHA-256 |
| --- | --- |
| Forward producer `wrap-and-create/PairFactory.scala` | `9232b0a536470a69041f399a23f2b4a0e80a9dcfeb28575f17a84d1f146f8935` |
| Reverse producer `create-and-wrap/PairFactory.scala` | `0c14002f9915683fe60371ac53ea36bec77b24b8a04781152fbae020449ac95c` |
| Java runtime harness `PairFactoryIdentityHarness.java` | `9b070f0d29c3607562b1529a49a61c10edaab9e1415e7fa03646bf1b566bdb3c` |
| Scala client `Consumer.scala` | `0694c78ae678da1575b14b136c2503b35ff038bd1f24624aa97b20c7fd83c035` |
| Scala reverse client `ConsumerReversed.scala` | `4db1392a9548a7f9f6a6376c115870bf9dde0cac0bef4e61b45f29ebf9b22c04` |

### AC2–AC3 — original frontend session, identities, and TASTy

`mixed_constructor_arguments_serialize_in_both_original_positions` emits one
session receipt per original input through
`with_paired_jvm_tasty_batch_plan`. The callback-scoped proof borrows the same
`SemanticStore`, source/typed arenas, source semantic index, source-to-typed
bindings, and complete JVM unit. The test asserts pointer identity for the
shared store/arenas/index across all three class plans, and verifies each
source argument maps to the exact typed root in the proof.

| Receipt | `wrapAndCreate` | `createAndWrap` |
| --- | --- | --- |
| `SourceId` | `201400` | `201402` |
| classes / symbols | `Counter=40`, `Pair=41`, `PairFactory=42` | same run-local IDs |
| factory method / symbol | source `TreeId(59)`, `SymbolId(60)` | source `TreeId(59)`, `SymbolId(60)` |
| method formals | source `TreeId(45), TreeId(47)`; symbols `61, 62`; types `TypeId(82), TypeId(5)` (`Counter, Int`) | same run-local IDs |
| factory descriptor | `(Lexample/Counter;I)Lexample/Pair;` | same |
| target / outer constructor | `Pair` symbol `41`; `Pair.<init>` symbol `52`, descriptor `(Lexample/Counter;Lexample/Counter;)V` | same run-local IDs |
| outer method RHS, Select, New | source/typed `58/22`, `51/15`, `50/14` | same run-local IDs |
| Pair arguments, source/typed | `52/16`, `57/21` | `56/20`, `57/21` |
| ordered origins | `Formal(existing), NestedNew(Counter(seed))` | `NestedNew(Counter(seed)), Formal(existing)` |
| nested Counter roots | Apply `57/21`, Select `55/19`, New `54/18`, seed `56/20`; ctor symbol `44`, seed formal `62` | Apply `56/20`, Select `54/18`, New `53/17`, seed `55/19`; ctor symbol `44`, seed formal `62` |

The proof kind is the existing `FormalNestedPair`; no separate mixed-proof
engine is introduced. It retains separate method and constructor formals,
source order, exact source/typed roots, canonical symbols, and types.
`PairFactory.tasty` contains the ordered
`(existing: Counter, seed: Int): Pair` method signature, signed
`Pair.<init>(Counter, Counter): Pair` selection, a direct `TERMREFdirect` to
`existing`, and a nested `Counter.<init>(Int): Counter` application whose
`TERMREFdirect` targets `seed`. The AST index reports method `PARAM` addresses
`32` (`existing`) and `41` (`seed`) in both orientations; the focused test
prints those addresses with the expected outer argument-origin order and
asserts each reference targets the corresponding formal. Reversing the
expression swaps only those two argument children. `Pair.tasty` retains
separate immutable `left`/`right` aliases; `Counter.tasty` retains state,
`get`, and `set`.

### AC4–AC5 — six linked artifacts, deterministic bytes, and JVM call shape

Each orientation emits exactly `Counter.class/.tasty`, `Pair.class/.tasty`,
and `PairFactory.class/.tasty`. The strict Scala consumer emits each original
orientation twice from fresh Rust sessions and confirms byte equality within
that orientation. The reversed Factory differs as expected; equality between
different sources is not required. All classes are major 65 and each class
`TASTY` attribute matches its sidecar UUID.

| Artifact | Shared / forward SHA-256 | Reverse SHA-256 | UUID |
| --- | --- | --- | --- |
| `Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | same | `008071c3c00968ac0000000000008193` |
| `Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | same | — |
| `Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | same | `000b7d8d7441c7f00000000000008198` |
| `Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | same | — |
| `PairFactory.class` | `4b764c8dccf4074f3df76ede0326c7da7e94af458aa21d4d8a660bc2193886d9` | `c01945fb2aa13978a44c036646bc0d29c0eb5455f44c58244f57e5f0fe9bf0cc` | forward `009425da889b07c80000000000008198`; reverse `00921f29eb774ec10000000000008198` |
| `PairFactory.tasty` | `4264dc0388a33002da2500cd178fd05c3f94199b9cac8221d3bdde67ef549009` | `5f8a3f3cf5c3f4ed83e7ea4f8498cb6498f502f90a8e988a51268b15ab9b70a9` | — |

The Rust bytecode is fully decoded and the straight-line CFG is verified. The
factory ABI in both orientations is
`wrapAndCreate/createAndWrap:(Lexample/Counter;I)Lexample/Pair;`, with
`existing` in local 1 and `seed` in local 2. Forward reports
`max_stack=6,max_locals=3`: Pair `new@0`, `aload_1@4`, Counter `new@5`,
`iload_2@9`, `Counter.<init>(I)V@10`, `Pair.<init>(Counter,Counter)V@13`,
`areturn@16`. Reverse reports `max_stack=5,max_locals=3`: Pair `new@0`,
Counter `new@4`, `iload_2@8`, Counter init `@9`, `aload_1@12`, Pair init
`@13`, `areturn@16`. Constant-pool references are decoded from the emitted
classfile; the runtime test does not assume guessed indexes or offsets.

### AC6–AC7 — strict Java and Scala consumers

The separate Java client is compiled with `javac --release 21` and runs with
`java -Xverify:all -ea` against only the original Rust `.class` files. Both
orientations pass: the original `Counter(7)` retains identity in its requested
Pair field, the other field is a fresh `Counter(seed)`, changing the original
to `42` affects only its alias, changing the new Counter to `-9` affects only
that instance, and another call allocates a fresh Counter. Original six-file
hashes are checked before and after. Runtime output records
`identity:true; first=42/-9; repeat=11; zero=0; negative=-5; fresh:true` for
both method orders.

The independently compiled Scala 3.9 client uses the same original six Rust
files plus only the two hash-pinned Scala runtime JARs. It runs on JDK 21 with
the same identity, allocation, and mutation assertions; output is
`formal-identity=true; fresh-counter=true; isolated-mutation=true` for both
argument orders. Official `-decompile`, `-print-tasty`, and `-from-tasty`
inspect the original TASTy; `-from-tasty` output is inspection-only and is
excluded from runtime resolution. Separate classfile-masked controls prove
Scala resolves Counter methods, Pair methods/constructor, and the mixed
factory method from TASTy. The six original hashes remain unchanged after
compile, inspect, and run.

### AC8 — negative first failures and publisher fault containment

These controls mutate isolated source plans, TASTy sidecars, or copied class
files. A JVM-valid positional field swap is checked by object identity instead
of being labeled a verifier error.

| Control | Observed first rejecting stage |
| --- | --- |
| Third Pair constructor argument | SourceTyper: `ApplicationArityMismatch (expected: 2, actual: 3)` |
| Deeper nested constructor / wrong function shape | Cross-class proof: `SourceShape` (the nested argument must use an original factory formal; its function must be `Select`) |
| Wrong result type | Cross-class proof: exact Pair `TypeRef` identity mismatch |
| Wrong typed constructor/formal owners, swapped or duplicate roots, forged formal, absent source-to-typed edge | TASTy body validation: observed `Unsupported BodyExpression`, `Reference { reason: Owner }`, `SourceDefinition`, or `BodyBinding ... actual: None`; exact error receipts are printed by `mixed_constructor_receipt_rejects_swapped_and_forged_provenance` |
| Wrong Counter/Pair consumer signatures or owner | Pinned Scala source typing: `[E171]` arity, `[E007]` type mismatch, `[E006]` missing package symbol, or `[E008]` missing TASTy member as appropriate |
| Corrupt formal/seed `TERMREFdirect` or malformed Pair alias address | Official Scala `-decompile`: `TreeWithoutOwner` |
| Missing/misplaced, corrupt, or mismatched-UUID sidecar | Scala binary reader: missing TASTy file, load error, or “out of sync with its TASTy file” |
| Wrong constructor Methodref, missing Counter `new`/`<init>`, wrong seed slot, extra pending allocation | JVM verifier: observed `VerifyError` at the bad operand, constructor, or stack state |
| `max_stack` too small / `max_locals` too small | JVM verifier `VerifyError: Operand stack overflow` / classfile load `ClassFormatError: Arguments can't fit into locals` |
| Same-typed but wrong Pair Fieldref | JVM/Scala accept the type-correct class; Java identity assertion reports that caller identity moved to the wrong Pair field |
| Publisher failure after each of six staged files, before promotion, and after promotion | `mixed_constructor_batch_publisher_contains_each_handled_failure` rejects each case, leaves no accepted partial destination, and preserves the preexisting sentinel. This is handled-failure containment, not crash-atomic multi-file durability. |

### AC9–AC10 — required executions, CI, and closeout boundary

The integrated run used
`DOTTY_REQUIRE_SCALA_TASTY_MIXED_ORIGIN_CONSTRUCTOR_CONSUMER=1`, every active
strict flag in `.github/workflows/jvm-tasty-factory-scala-consumer.yml`, and
`DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1`. It ran all listed Scala consumer and
reader targets, the additional alias-address, secondary-constructor rollback,
and dual-reference forwarding checks, plus the mandatory
`DOTTY_CLASSFILE_REQUIRE_JDK=1` classfile/JVM/source/artifact/stateful/identity
gate. No required Scala or JDK target skipped. A full workspace-test attempt
without `JAVA_HOME` stopped at a Java executable lookup; the full rerun with
the hash-verified JDK path passed.

On the integrated baseline, these required checks passed:

```text
cargo fmt --all -- --check
python3 tools/check-frontend-boundaries.py
cargo +1.98.1 clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo +1.98.1 doc --workspace --no-deps --locked
JAVA_HOME=<Temurin 21.0.13> RUST_TEST_THREADS=1 cargo +1.98.1 test --workspace --all-targets --locked
git diff --check
```

`RUST_TEST_THREADS=1` bounds the workspace test harness because the earlier
#2011 default-parallel run on this host reached its process file-descriptor
limit. No hosted full Scala corpus audit was run.

Deferred forms remain outside this release gate: a third constructor
argument, deeper nesting or branch-local construction, local/field/helper
produced arguments, generics, contextual/default/overloaded/secondary
constructors, cross-unit ABI, and a public compiler session or CLI. No
production behavior was changed for #2018; future missing producer behavior
returns to its owning issue.

This is an audit-only result. It records the Scala 3.9.0 representation and
the current Rust first rejection for a Pair constructor call with one existing
method formal and one nested `new Counter(seed)`. It changes no production
semantics and establishes no paired JVM/TASTy feature support.

## Rust producer / official Scala client proof (#2017)

The strict Scala consumer now emits each producer variant through one Rust
frontend session, writes the original six linked files to a clean directory,
then compiles a separately authored client against those artifacts and the two
hash-pinned Scala runtime JARs. The runtime classpath contains only the Scala
client output, the original Rust producer directory, and those runtime JARs.
`-Xverify:all -ea` is enabled. Scala `-from-tasty` output is written to a
separate inspection-only directory and is never included in runtime resolution.

The original source and client receipts from the strict run are:

| File | SHA-256 |
| --- | --- |
| `wrap-and-create/PairFactory.scala` | `9232b0a536470a69041f399a23f2b4a0e80a9dcfeb28575f17a84d1f146f8935` |
| `create-and-wrap/PairFactory.scala` | `0c14002f9915683fe60371ac53ea36bec77b24b8a04781152fbae020449ac95c` |
| `Consumer.scala` | `0694c78ae678da1575b14b136c2503b35ff038bd1f24624aa97b20c7fd83c035` |
| `ConsumerReversed.scala` | `4db1392a9548a7f9f6a6376c115870bf9dde0cac0bef4e61b45f29ebf9b22c04` |

The six original forward producer artifact hashes and linked TASTy UUIDs were:

| Artifact | SHA-256 | TASTy UUID (class files only) |
| --- | --- | --- |
| `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008071c3c00968ac0000000000008193` |
| `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | — |
| `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d7441c7f00000000000008198` |
| `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | — |
| `example/PairFactory.class` (forward) | `4b764c8dccf4074f3df76ede0326c7da7e94af458aa21d4d8a660bc2193886d9` | `009425da889b07c80000000000008198` |
| `example/PairFactory.tasty` (forward) | `4264dc0388a33002da2500cd178fd05c3f94199b9cac8221d3bdde67ef549009` | — |

The reverse producer uses the same Counter and Pair artifacts and UUIDs listed
above, and its distinct Factory receipts are:

| Artifact | SHA-256 | TASTy UUID |
| --- | --- | --- |
| `example/PairFactory.class` (reverse) | `c01945fb2aa13978a44c036646bc0d29c0eb5455f44c58244f57e5f0fe9bf0cc` | `00921f29eb774ec10000000000008198` |
| `example/PairFactory.tasty` (reverse) | `5f8a3f3cf5c3f4ed83e7ea4f8498cb6498f502f90a8e988a51268b15ab9b70a9` | — |

Each orientation is independently emitted again from a fresh Rust session; all
six bytes match within that orientation. Byte equality is not required across
changed source variants.

The accepted clients print:

```text
formal-identity=true; fresh-counter=true; isolated-mutation=true; argument-order=existing,new
formal-identity=true; fresh-counter=true; isolated-mutation=true; argument-order=new,existing
```

The assertions check original-object identity via test-only reflection, a new
Counter per nested allocation and per call, and independent mutation of the
formal and nested instances. Official Scala `-decompile`, `-print-tasty`, and
`-from-tasty` inspect all three original sidecars. `-decompile` confirms
ordered `(existing: Counter, seed: Int): Pair` formals and the direct formal
plus nested `Counter(seed)` at the expected argument index for each ordering.
The paired classfile-only controls fail source lookup after the corresponding
TASTY class attribute/sidecar is removed; the original-sidecar controls pass
for Counter `get`/`set`, Pair `leftValue`/`rightValue`/constructor, and the
factory method. All six producer hashes are rechecked after compilation,
inspection and runtime.

Toolchain provenance from the strict run: Scala source revision
`777528f19a58e794c9954a42f433373472ec57f8`, staged compiler
`3.9.0-bin-SNAPSHOT-git-777528f` (staged `with_compiler.jar` SHA-256
`172c8b8384e75ee24d327fce5ec788582bf314f66bf9983ba57e0cac6c3ff498`), and
Temurin `21.0.13+9` (`java` SHA-256
`9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`). The
published Scala 3.9.0 runtime hashes verified by the test are
`scala3-library_3-3.9.0.jar`
`9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba` and
`scala-library-3.9.0.jar`
`b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd`.

Authentic source-variant receipts from the same required run:

| Variant | Producer source SHA-256 | Independent client SHA-256 | Runtime result |
| --- | --- | --- | --- |
| Renamed package/classes/method | `60ed8c99a1476a75fa00a91a0eb7330ac531675c69f717b80d0f1d94a29f0d59` | `380cd92695817152fec92bb3a7742beafa0468daa9a5d04edf040c8784a3533e` | Pass; renamed TASTy decompile/print/from-tasty pass |
| Reordered class declarations | `ab37a91bc3a746ed2caaad9876790fa0543d6e7668cbc5d14d7d973b95c66720` | `0694c78ae678da1575b14b136c2503b35ff038bd1f24624aa97b20c7fd83c035` | Pass; official reader and runtime pass |

The renamed six artifact SHA-256 values, in `Ledger`, `Duo`, `Factory` class
then TASTy order, are `f5b5ac03d66bd01eefd42dc853b93affdb4eb1480a2e307a40beb174a3ca8a15`,
`7306118359443cf349b7c79efa72b75127bf514e31f496a832ba203f4e4a4e6e`,
`cb6ecc5dae7407d7d9dd05697832ebd387e67c2f99ed223b2f935e03a1994faf`,
`54769dc8af30e7f3c82d5fc38a2648adcfdc7331653f9b9e5d9f7d9a92f4bbba`,
`059d84133208988bf9d682a1fc0f74f07eb8a3723c70d564966277f2271d522e`, and
`dead0c42c766f3b09d9b380187d4b7e81b863ff0af292e2e628a94dfcf1155c3`. The
reordered declarations produce the same six bytes as the original forward
input; the test still verifies byte identity across a second fresh session for
each variant independently.

The isolated negative matrix records the observed authoritative first
failure: absent or misplaced sidecars fail in Scala's binary reader; corrupt
sidecars fail while loading the named class; mismatched links report the class
out of sync with TASTy; wrong Pair arity and argument type report `[E171]` and
`[E007]`; wrong source package reports `[E006]`; a foreign same-spelled
Counter reports `[E007]`; missing Counter reports `[E008]`; missing Counter
constructor reports `NoSuchMethodError`; malformed formal and seed TERMREF
addresses fail official `-decompile` with `TreeWithoutOwner`; a renamed-away
TASTy factory method reports `[E008]`; an incorrect constructor Methodref
fails at JVM verification; a changed Factory classfile descriptor fails at JVM
verification; and a same-typed left/right Fieldref swap remains JVM-legal but
is caught by the original-reference identity assertion. The tests preserve
these distinct reader, compiler, verifier/linker, and semantic identity
outcomes rather than expecting every mutation to fail at Scala source typing.

Factory signature controls separately replace only `PairFactory.tasty` with
an official Scala 3.9 sidecar for a one-formal `wrapAndCreate` or a
`wrapAndCreate` returning `Counter`. The replacement sidecar is relinked to
the original Factory class UUID; the original Rust `PairFactory.class` bytes
are asserted unchanged before and after each independently authored client
compile. Both controls fail at Scala source typing with `[E007] Type Mismatch
Error` (the one-formal case reports the supplied `(Counter, Int)` arguments
against its single `Counter` formal). These source-signature controls are
independent of the JVM classfile descriptor mutation, which continues to pass
source typing and fail later at JVM verification/linkage.

The strict flag is `DOTTY_REQUIRE_SCALA_TASTY_MIXED_ORIGIN_CONSTRUCTOR_CONSUMER=1`.
The existing pinned Scala workflow sets it, executes this consumer with the
other inherited strict Scala readers and consumers, and retains the mandatory
JDK 21 gate. With the flag set, missing compiler checkout, source revision,
runtime cache, or Java runtime fails the test instead of taking the optional
skip path.

## Frozen inputs and oracle

The audit starts from `origin/main` at
`7943e8d495815ceef2b7e4a53baee7f6fcac949b`. Scala is pinned to revision
`777528f19a58e794c9954a42f433373472ec57f8`, staged compiler
`3.9.0-bin-SNAPSHOT-git-777528f`, and Temurin JDK `21.0.13+9` with
`--release 21`. `oracle.json` retains the staged compiler JAR SHA-256 as an
informational receipt because sbt may package the same pinned sources with
different ZIP metadata. The `java` binary SHA-256 is
`9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`.
The compiler JAR receipt currently recorded is
`b56626390028bd06347e3549cf271a03ca7d2e7be491ceff630ccee94e439f4f`; the
reproduction comparison deliberately ignores only that field.
`oracle.json` records the exact producer and client source hashes, the six
producer artifact hashes for each ordering, classfile/TASTy UUID links, and
hashes for normalized snapshots. Normalization replaces only the dump's input
path and javap modification-date header; bytecode offsets, constant-pool
references, artifact checksums, and the rest of each dump remain compared.

The two producer inputs are separate frozen files under
[`tests/fixtures/jvm_tasty_mixed_origin_constructor_arguments`](../tests/fixtures/jvm_tasty_mixed_origin_constructor_arguments):

| Input | Method body | SHA-256 |
| --- | --- | --- |
| `wrap-and-create/PairFactory.scala` | `new Pair(existing, new Counter(seed))` | `9232b0a536470a69041f399a23f2b4a0e80a9dcfeb28575f17a84d1f146f8935` |
| `create-and-wrap/PairFactory.scala` | `new Pair(new Counter(seed), existing)` | `0c14002f9915683fe60371ac53ea36bec77b24b8a04781152fbae020449ac95c` |

Each order was compiled into its own empty output directory with the same
three declarations: `Counter`, `Pair`, and `PairFactory`. Each output contains
exactly `Counter`, `Pair`, and `PairFactory` `.class` files and their three
`.tasty` sidecars. All classfiles have major version 65, all sidecars are TASTy
28.9.0, and each classfile's `TASTY` attribute equals its corresponding
sidecar UUID. The binary files and `javap -v -p -c` / `-print-tasty` snapshots
are checked in as Scala oracle output only. Rebuild both variants and run
their separate Scala-only clients with:

```sh
export SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8
export JAVA_HOME=/path/to/temurin-21.0.13
export PATH="$JAVA_HOME/bin:$PATH"
export TMPDIR="$PWD/target/issue-2011/tmp"
export DOTTY_ISSUE_2011_ORACLE_DIR="$PWD/target/issue-2011/oracle"
tests/fixtures/jvm_tasty_mixed_origin_constructor_arguments/rebuild-oracle.sh
```

The rebuild first runs `test_oracle_tools.py`, which verifies that paths and
dates normalize while bytecode evidence is preserved, and that manifest
comparison ignores only the informational compiler JAR digest.

The clients compile against only the matching Scala producer output and Scala
runtime. Each creates `Counter(7)`, invokes its one factory method with seed
`3`, repeats it with seed `-9`, checks that the original formal object reaches
the requested Pair field, checks that each nested allocation is fresh, and
mutates the formal and allocated Counters independently. Both clients pass.
They do not consume Rust-produced artifacts.

## Scala classfile and TASTy observations

Both methods have descriptor
`(Lexample/Counter;I)Lexample/Pair;`; `existing` is local 1 and `seed` is
local 2. The actual `javap` snapshots record constant-pool references and
offsets. For `wrapAndCreate`, the method reports `stack=6, locals=3`:

| Offset | Instruction | Resolved constant-pool entry |
| ---: | --- | --- |
| 0 | `new #18` | `example/Pair` |
| 3 | `dup` | — |
| 4 | `aload_1` | existing `Counter` formal |
| 5 | `new #20` | `example/Counter` |
| 8 | `dup` | — |
| 9 | `iload_2` | seed `Int` formal |
| 10 | `invokespecial #23` | `Counter.<init>(I)V` |
| 13 | `invokespecial #26` | `Pair.<init>(Counter, Counter)V` |
| 16 | `areturn` | — |

For `createAndWrap`, the method reports `stack=5, locals=3`: Pair is
allocated at offset 0, then Counter at offset 4, `iload_2` at 8,
`Counter.<init>(I)V` at 9, `aload_1` at 12, and
`Pair.<init>(Counter, Counter)V` at 13, followed by `areturn` at 16.
The two methods each perform one outer Pair allocation and one inner Counter
allocation. Evaluation of the constructor arguments follows their source
order, so the `aload_1` and nested allocation exchange order between variants.
The actual constant-pool entries are listed in each checked-in `javap`
snapshot; no offset or pool index is inferred from source text.

In both TASTy files, `PairFactory` contains a `DEFDEF` with ordered
`PARAM existing: Counter`, `PARAM seed: Int`, and result `Pair`. Its body is
an outer `APPLY(SELECTin(NEW(Pair)))`. In the first input the argument children
are `TERMREFdirect(existing)` followed by
`APPLY(SELECTin(NEW(Counter)), TERMREFdirect(seed))`. In the reversed input
those children are reversed. The selected constructor signatures are
`Pair.<init>(Counter, Counter): Pair` and `Counter.<init>(Int): Counter`.
These TASTy tree children preserve semantic source order; the JVM instruction
offsets independently show evaluation order.

## One-session Rust observations

`mixed_origin_pair_arguments_reach_jvm_and_report_the_proof_boundary` runs
each frozen input through the real Lexer → Parser → Namer → SourceTyper and
normal JVM source-compilation pipeline. The two orderings use independent
sessions and `SourceId(20110)` / `SourceId(20111)`. The following arena numbers
are receipts for these runs only; equal numbers across the two independently
created stores do not denote shared identities.

| Source item | `wrapAndCreate` session | `createAndWrap` session |
| --- | --- | --- |
| `Counter`, `Pair`, `PairFactory` class symbols | 40, 41, 42 | 40, 41, 42 |
| Factory method declaration / symbol | `TreeId(59)` / `SymbolId(60)` | `TreeId(59)` / `SymbolId(60)` |
| Formals `(Counter, Int)` | `(TreeId(45), SymbolId(61), TypeId(66))`, `(TreeId(47), SymbolId(62), Definitions.int)` | same run-local values |
| Counter and Pair constructors selected by the body | `SymbolId(44)`, `SymbolId(52)` | same run-local values |
| Outer Pair `Apply` source / typed root | `TreeId(58)` / `TreeId(22)` | same run-local values |
| Outer Pair `Select` and `New`, source / typed roots | `51/15`, `50/14` | same run-local values |
| Source argument roots / typed roots | `52,57` / `16,21` | `56,57` / `20,21` |
| Nested Counter `Apply`, `Select`, `New`, seed, source / typed roots | `57/21`, `55/19`, `54/18`, `56/20` | `56/20`, `54/18`, `53/17`, `55/19` |
| Source argument spans in file order | `437..445`, `447..464` | `437..454`, `456..464` |

The `Counter` formal and `Int` formal are distinct canonical method-owned
symbols. Their declarations have no typed declaration roots. The direct
`Counter` argument remains an `Ident` with a typed `TermRef` to `SymbolId(61)`;
the inner `Counter` argument remains an `Apply` whose seed `Ident` has a typed
`TermRef` to `SymbolId(62)`. The original `SourceTypedIndex` retains each
source-to-typed edge. The outer and inner constructor selections resolve to
the actual `Pair.<init>` and `Counter.<init>` symbols in the same source unit.

The normal JVM-only path emits the three classfiles for each input. The audit
asserts the factory method descriptor, `max_locals=3`, selected constructor
references, and bytecode order with exactly one Pair and one Counter
allocation. This path emits no TASTy sidecars. It is evidence about the
existing JVM-only source backend and does not establish paired output.

| Reachable Rust stage | First observed result |
| --- | --- |
| Lexer → Parser → Namer → SourceTyper | Passes for both inputs; original source and typed roots and identities are retained. |
| JVM-only source compilation and emission | Passes for both inputs; emits `Counter`, `Pair`, and `PairFactory` classfiles. |
| Cross-class constructor-call proof | Rejects both at `SemanticIdentity("both factory formals must retain the same canonical reference class")`. The pair proof requires equal canonical formal classes, while these formals have types `Counter` and `Int`. |
| Paired JVM/TASTy batch | First rejects in phase `cross-class constructor-call identity proof` with the same semantic identity error; its callback does not run. |
| Paired signature/body validation, pickler writer, TASTy output, and paired JVM artifact publication | Masked by the earlier cross-class proof error; none is reported as an observed later failure. |

The Scala oracle describes a valid language-level call shape. The Rust
first-error result describes only the current paired planner boundary; it is
not a claim that the Rust paired path accepts or emits this shape.

## Negative and adjacent-case matrix

| Variation | Audit result |
| --- | --- |
| Swap the two constructor argument positions | Measured as the separate valid `createAndWrap` input in Scala, Scala client, and the authentic Rust source session. |
| Rename classes/package/formals or reorder declarations | Not measured in this issue audit. |
| Pass `Int` where `Counter` is required; wrong Pair constructor arity | Not measured here. No Rust behavior or Scala diagnostic is claimed. |
| Same-named foreign `Counter`; missing or duplicate method formal; different Counter owner | Not measured here. |
| Test-only mutation of the source/typed target binding | Not measured. The confirmed earlier formal-class guard would need to be accounted for before interpreting a later identity result. |
| Third constructor argument; second nesting level; helper call, local value, or branch argument | Not measured here. These shapes are outside the frozen producer. |
| Using/default/generic/overloaded/secondary constructor alternatives | Not measured here. |
| Missing/corrupt TASTy sidecar or mismatched classfile/TASTy link | Not exercised for these inputs: the paired planner stops before it emits artifacts that could be passed to the link validator. |

The established neighboring evidence remains bounded to the earlier
Formal+Formal forwarding and NestedNew+NestedNew constructor-call audits in
[`jvm-tasty-dual-reference-forwarding.md`](jvm-tasty-dual-reference-forwarding.md)
and
[`jvm-tasty-multi-argument-constructor-calls.md`](jvm-tasty-multi-argument-constructor-calls.md).
Those results do not imply support for the mixed-origin pair audited here.

## Mixed method signature projection (#2013)

The semantic pickler accepts the original ordinary method signature
`wrapAndCreate(existing: Counter, seed: Int): Pair` when the method, `Counter`,
and `Pair` have canonical declarations in the same source and package, and the
two method formals retain their exact source-owned `SymbolId`s and declaration
order. Its completed plain `MethodType` must agree with those symbols: the
first formal is the exact planned `Counter` class reference, the second is
Scala `Int`, and the result is the exact planned `Pair` class reference. The
existing planned-class proof from #2012 authenticates the referenced classes;
the validator also checks their source definitions, package prefix, names,
ownership, and `ClassInfo`.

`ValidatedClassStatePlan::method_signature_skeleton` projects the two distinct
source `PARAM` declarations and the explicit `Counter`, `Int`, and `Pair` type
syntax. The skeleton retains each original method body root in its
`omitted_bodies` metadata and emits no RHS. It is a structural signature
projection, not a complete or Scala-readable sidecar. The original body
`new Pair(existing, new Counter(seed))` remains outside this increment and is
expected to fail body serialization until #2014. Reversing the constructor
argument expressions does not change method formal order. Source class order
and package/class/method/formal spellings may vary without replacing semantic
identity with name-based matching.

The bounded new form is exactly `(same-source class, Int) => same-package
class` in one plain value clause. Flipped `(Int, class)`, an added third formal,
foreign or unplanned classes, other result classes, multiple clauses,
erased/default/contextual/generic/overloaded signatures remain rejected. The
pre-existing one-class-reference method signature and same-typed dual-formal
signatures retain their earlier acceptance rules.

## Reproduction and validation

The focused Rust audit regression is:

```sh
export TMPDIR="$PWD/target/issue-2011/tmp"
export CARGO_TARGET_DIR="$PWD/target/issue-2011/cargo"
export CARGO_BUILD_JOBS=2
cargo test -p dotty --test jvm_source_compilation \
  mixed_origin_pair_arguments_reach_jvm_and_report_the_proof_boundary \
  --locked -- --nocapture
```

The Scala rebuild script above is strict about the Scala revision and compiles
each consumer separately. This increment changes only audit tests, frozen
oracle fixtures/manifests/scripts, and documentation. Existing fixture bytes
are not modified.

## Rust TASTy implementation receipt (#2014)

The source-backed pickler bridge now projects the immutable ordered proof into
one receipt with original per-position source and typed roots and an explicit
`Formal` or `NestedNew` origin. The pickler checks each origin against the
corresponding `Pair` constructor formal and preserves the original method
parameter order. The existing constructor `APPLY` writer serializes both
children without a separate mixed-expression writer. The two supported
orientations remain `(existing, new Counter(seed))` and
`(new Counter(seed), existing)`.

[`jvm_tasty_mixed_origin_constructor_arguments.rs`](../tests/jvm_tasty_mixed_origin_constructor_arguments.rs)
decodes the original Rust output and checks the ordered
`(Counter, Int) => Pair` method parameters, the selected signed constructors,
the direct `Counter` formal reference, the nested `Counter` constructor call,
and its `seed` reference. It also exercises swapped or forged receipts,
duplicate roots, a missing source-to-typed edge, wrong result/argument types,
third arguments, deeper construction, helper results, locals, branches, and a
foreign constructor class. The rejected inputs report their first source,
typing, or proof failure in test output.

The strict pinned reader test in
[`jvm_tasty_dual_reference_forwarding_scala_consumer.rs`](../tests/jvm_tasty_dual_reference_forwarding_scala_consumer.rs)
uses the complete original Rust sidecars from one authentic source/typed
session per orientation. Scala 3.9.0 accepts all six files with `-decompile`,
`-print-tasty`, and `-from-tasty`. The last command reconstructs the classes
from the Rust sidecars while using Scala-compiled reference classes for symbol
lookup; it is not a runtime test of Rust-generated classfiles.

The strict run used Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`, runtime jar hashes
`9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba`
(`scala3-library_3-3.9.0.jar`) and
`b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd`
(`scala-library-3.9.0.jar`), and Temurin 21.0.13 `java` hash
`9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`.

The first post-#2013 failure was the expected
`Unsupported { feature: BodyExpression }` at the TASTy cross-contract gate for
both frozen orientations. After the bridge, both authentic batches validate
and the pinned readers accept the original Rust sidecars. A separate byte
comparison against baseline `1fbc0ac64cef9d8c4b7e5a16d4fe7a1e753f2a6d`
found matching hashes for all 12 Rust TASTy sidecars from the existing
one-argument, two-nested-argument, two-direct-formal, and reference-local
factory fixtures.

## Paired JVM/TASTy integration (#2015)

The integration starts at frozen baseline `ff2f400f19a6b805ef8c2b20766dda0cf84d02f2`.
The verified issue-level scope record selected one staged PR. The first paired
artifact call for both original orientations reproduced
`BatchArtifact { class_index: 2, phase: "cross-class invocation", reason:
"two-argument constructor has no canonical paired receipt" }`. The real
frontend session and JVM-only emitter remained successful, as measured above.

The cross-class contract now consumes the existing ordered
`Formal`/`NestedNew` receipt. It checks the exact source and typed argument
roots, original formal declaration order, canonical source ownership, the
`(Counter, Int) => Pair` method ABI, the selected `Pair(Counter, Counter)` and
`Counter(Int)` constructors, and the result class. The decoded factory
instructions are consumed in source argument order through `areturn`; each
allocation is initialized exactly once before the next argument. Existing
homogeneous direct-formal and nested-pair validators remain separate.

Focused Rust integration results use the frozen producer files and the same
`PairFactory.scala` name for each pair of independent sessions. Source IDs
were distinct within each pair. The hashes and UUIDs below are from the Rust
batch output, not the Scala oracle files.

| Orientation | Artifact | SHA-256 | TASTy UUID |
| --- | --- | --- | --- |
| `wrapAndCreate(existing, seed)` | `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008071c3-c009-68ac-0000-000000008193` |
| | `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | `008071c3-c009-68ac-0000-000000008193` |
| | `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d-7441-c7f0-0000-000000008198` |
| | `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | `000b7d8d-7441-c7f0-0000-000000008198` |
| | `example/PairFactory.class` | `4b764c8dccf4074f3df76ede0326c7da7e94af458aa21d4d8a660bc2193886d9` | `009425da-889b-07c8-0000-000000008198` |
| | `example/PairFactory.tasty` | `4264dc0388a33002da2500cd178fd05c3f94199b9cac8221d3bdde67ef549009` | `009425da-889b-07c8-0000-000000008198` |
| `createAndWrap(existing, seed)` | `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008071c3-c009-68ac-0000-000000008193` |
| | `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | `008071c3-c009-68ac-0000-000000008193` |
| | `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d-7441-c7f0-0000-000000008198` |
| | `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | `000b7d8d-7441-c7f0-0000-000000008198` |
| | `example/PairFactory.class` | `c01945fb2aa13978a44c036646bc0d29c0eb5455f44c58244f57e5f0fe9bf0cc` | `00921f29-eb77-4ec1-0000-000000008198` |
| | `example/PairFactory.tasty` | `5f8a3f3cf5c3f4ed83e7ea4f8498cb6498f502f90a8e988a51268b15ab9b70a9` | `00921f29-eb77-4ec1-0000-000000008198` |

The bytecode walk used decoded PCs and constant-pool entries from each Rust
classfile:

| Method | Observed instructions | Bounds |
| --- | --- | --- |
| `wrapAndCreate` | `0 new #12 Pair`; `3 dup`; `4 aload_1`; `5 new #14 Counter`; `8 dup`; `9 iload_2`; `10 invokespecial #17 Counter.<init>(I)V`; `13 invokespecial #20 Pair.<init>(Counter,Counter)V`; `16 areturn` | stack 6, locals 3 |
| `createAndWrap` | `0 new #12 Pair`; `3 dup`; `4 new #14 Counter`; `7 dup`; `8 iload_2`; `9 invokespecial #17 Counter.<init>(I)V`; `12 aload_1`; `13 invokespecial #20 Pair.<init>(Counter,Counter)V`; `16 areturn` | stack 5, locals 3 |

The integration test prints the decoded instruction PCs and operands while
walking the bytecode. It also verifies all six artifact paths, class-file
major 65, TASTy 28.9.0, per-class UUID links, Pair alias fields, method
descriptor and direct TASTy PARAM references. Independent sessions match all
six files byte for byte for each original orientation. Renamed and reordered
inputs are compared only against their own second session.

Negative controls reject a type-correct source-order swap at the mixed
contract, duplicate/swapped source and typed roots, wrong selected argument
type, absent current-source receipt, wrong local loads, an unexpected
allocation, missing nested initialization, wrong Pair constructor reference,
incorrect stack/local bounds, missing or corrupt sidecars, an opposite-order
sidecar UUID/body, duplicate or unsafe artifact paths, and each of the six
staged-write plus before/after-promotion handled publisher faults. The existing
publisher preserves an already-present destination sentinel. These tests
cover handled failures only and make no crash-atomic durability claim.

Validation performed for this increment:

```text
cargo test -p dotty --test jvm_tasty_artifacts --locked: 62 passed
cargo test -p dotty --test jvm_tasty_mixed_origin_constructor_arguments --locked: 4 passed
cargo test -p dotty --test jvm_source_compilation mixed_origin_pair_arguments_reach_jvm_and_report_the_proof_boundary --locked -- --nocapture: passed for both source sessions
```

The Rust toolchain was `rustc 1.98.1 (48a229cea 2026-09-01)`. A local
Temurin 21.0.8 installation is available at `/home/vdrake6/.jdks/jdk-21`;
its `java` SHA-256 is
`90d0a6d25da103678013f193db078b90ed8aeb27b66ebe78e0149616c9c1b811`. The
mandatory `DOTTY_CLASSFILE_REQUIRE_JDK=1` gate passed with this JDK. The
workspace suite's serial rerun reached an unrelated pinned-environment check
and failed because it requires Java 21.0.13, while this environment provides
21.0.8. The initial parallel workspace run also hit the container open-file
limit in 13 classloader tests; those tests passed in the serial rerun except
for the strict version assertion.

The pinned Scala reader was not executed in strict mode. Its Scala 3.9 source
checkout and runtime jars are not present in this environment, and the
unconfigured test invocation skips that check. The Scala oracle source remains
pinned at `777528f19a58e794c9954a42f433373472ec57f8`; its earlier #2014 strict
reader receipt verifies the byte-identical Rust sidecars, but does not replace
a strict run for this issue.

## Strict JDK runtime identity and allocation-order proof (#2016)

This runtime proof starts from integrated `main` at
`b79dc945d392a470d42997081602c6f1464fe02a`; the required #2045 merge commit
`5cb16a5868c53071e9fcbc08370df688a5378ed0` is an ancestor. It uses the two
original frozen Scala inputs above, each in its own complete Rust
Lexer/Parser/Namer/SourceTyper and paired JVM/TASTy session. It does not use
Scala-produced classes or reconstructed artifacts. The Java harness is a
separate test source, compiled with `javac --release 21`; the runtime classpath
contains the three original Rust `.class` files and the harness classes. The
six `.class`/`.tasty` bytes per orientation are hashed before compilation and
compared byte-for-byte after `javac`, successful runtime execution, and all
corrupt-copy runs.

The strict integration is
[`mixed_origin_counter_identity_and_allocation_order_on_strict_jdk21`](../tests/jvm_tasty_mixed_origin_runtime.rs).
It decodes the factory `Code` attribute with the Rust class-file reader,
resolves every allocation and constructor operand through the original
constant pool, constructs a CFG from those decoded instructions, and runs the
class-aware CFG analyzer. It also runs `javap -v -p -c` on each original
factory class. PCs below are observed from the Rust decoder and javap for
these exact artifact bytes, not assumed by the test when it walks the code.

Both methods have descriptor
`(Lexample/Counter;I)Lexample/Pair;`, argument local 1 is the reference and
argument local 2 is the `Int`, and both use `max_locals=3`. The complete
decoded instruction sequences and measured stack bounds are:

| Method | Observed instructions | `max_stack` / `max_locals` |
| --- | --- | --- |
| `wrapAndCreate` | `0 new #12 Pair`; `3 dup`; `4 aload_1`; `5 new #14 Counter`; `8 dup`; `9 iload_2`; `10 invokespecial #17 Counter.<init>(I)V`; `13 invokespecial #20 Pair.<init>(Counter,Counter)V`; `16 areturn` | `6 / 3` |
| `createAndWrap` | `0 new #12 Pair`; `3 dup`; `4 new #14 Counter`; `7 dup`; `8 iload_2`; `9 invokespecial #17 Counter.<init>(I)V`; `12 aload_1`; `13 invokespecial #20 Pair.<init>(Counter,Counter)V`; `16 areturn` | `5 / 3` |

The class-aware CFG analyzer accepts both decoded streams and reports the same
stack and local bounds as the classfiles. Each method allocates exactly one
Pair and one Counter, calls `Counter.<init>(I)V` once and
`Pair.<init>(LCounter;LCounter;)V` once, and returns the initialized Pair.
The `new Pair` allocation remains pending while the nested Counter is created
and initialized; this is the existing bounded two-live-uninitialized shape.

Under Temurin JDK `21.0.13+9` (`java` reports build `21.0.13+9-LTS`), the
separate harness prints these literal results with `java -Xverify:all -ea`:

```text
wrapAndCreate=identity:true;first=42/-9;repeat=11;zero=0;negative=-5;fresh:true
createAndWrap=identity:true;first=42/-9;repeat=11;zero=0;negative=-5;fresh:true
```

The harness reads `Pair.left` and `Pair.right` with test-only reflection. For
`wrapAndCreate`, it proves `left` is the original `Counter(7)` and `right` is
a distinct `Counter(3)`. For `createAndWrap`, those identities are reversed:
`right` is the original reference and `left` is freshly allocated. Both runs
mutate the original to 42 and the new object to -9 independently, then exercise
fresh allocations with seeds 11, 0, and -5. This verifies that method formal
order `(Counter, Int)` does not determine the constructor argument position.

Original artifact SHA-256 values measured before `javac` and unchanged through
all client and negative runs (the two `Counter` and `Pair` outputs are
identical because their source declarations are unchanged):

| Orientation | Artifact | SHA-256 |
| --- | --- | --- |
| both | `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` |
| both | `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` |
| both | `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` |
| both | `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` |
| `wrapAndCreate` | `example/PairFactory.class` | `4b764c8dccf4074f3df76ede0326c7da7e94af458aa21d4d8a660bc2193886d9` |
| `wrapAndCreate` | `example/PairFactory.tasty` | `4264dc0388a33002da2500cd178fd05c3f94199b9cac8221d3bdde67ef549009` |
| `createAndWrap` | `example/PairFactory.class` | `c01945fb2aa13978a44c036646bc0d29c0eb5455f44c58244f57e5f0fe9bf0cc` |
| `createAndWrap` | `example/PairFactory.tasty` | `5f8a3f3cf5c3f4ed83e7ea4f8498cb6498f502f90a8e988a51268b15ab9b70a9` |

The new Java harness source hash is
`9b070f0d29c3607562b1529a49a61c10edaab9e1415e7fa03646bf1b566bdb3c`.
The selected Java tools are under
`$HOME/.sdkman/candidates/java/21.0.13.0+9-tem`: `java` SHA-256
`9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`,
`javac` SHA-256
`2cc288a26beebc50ca7cd924847dffbfb38ef3314e105bba6d4ebce03a0bcb6b`, and
`javap` SHA-256
`b2146e8dcfe5662c04d14f3fad68756a1a597d01461ca475a2cdbeb44c84a678`.
The Rust toolchain is `rustc 1.98.1 (48a229cea 2026-09-01)` and `cargo
1.98.1 (797e8a9bc 2026-08-05)`.

Isolated corruption controls report the first observed failure stage. The
type-correct reversed method body fails the Rust batch's source-bound argument
order check; when tested independently by Java, it is verifier-valid and fails
the reflected identity assertion. Replacing the nested allocation and
initialization with a second `aload_1` is also verifier-valid, but fails the
fresh-object assertion. Malformed instruction and classfile copies fail at
their observed JVM boundary:

| Corrupt copy | First observed stage |
| --- | --- |
| Reversed argument orientation against the original source receipt | Rust batch contract (`cross-class invocation`, expected `aload_1`, observed `new`) |
| Reversed argument orientation run directly | Java identity assertion: caller identity is in the wrong Pair field |
| Duplicate existing Counter instead of the nested allocation | Java identity assertion: the new side aliases the caller object |
| Missing nested `NEW Counter` | JVM verifier (`VerifyError: Bad operand type when invoking <init>`) |
| Extra Counter allocation in place of `NEW Pair` | JVM verifier (`VerifyError: Call to wrong <init> method`) |
| Seed loaded from reference slot 1 | JVM verifier (`VerifyError: Bad type on operand stack`) |
| Wrong constructor owner/signature | JVM verifier (`VerifyError: Bad type on operand stack`) |
| Missing `Counter.<init>` | JVM verifier (`VerifyError: Bad type on operand stack`) |
| Invalid uninitialized Counter receiver flow | JVM verifier (`VerifyError: Bad type on operand stack`) |
| Counter `NEW` changed to Pair | JVM verifier (`VerifyError: Call to wrong <init> method`) |
| `max_stack` reduced to 1 | JVM verifier (`VerifyError: Operand stack overflow`) |
| `max_locals` reduced to 2 | JVM classfile loading (`ClassFormatError: Arguments can't fit into locals`) |
| Omit `Counter.class` or `Pair.class` | JVM linker (`NoClassDefFoundError`) |

The corruptions are written only into separate test directories under the
issue-local `TMPDIR`; they never replace the six original batch files. The
strict test prints the observed runtime failures and rechecks every original
SHA-256 and file byte after the corrupt-copy runs. This is an unchanged
original-artifact runtime acceptance result for this bounded Rust/JVM shape.
It does not claim Scala 3.9 source-consumer or TASTy runtime acceptance.

Validation on the #2016 implementation head used task-local
`TMPDIR=$PWD/target/issue-2016/tmp` and
`CARGO_TARGET_DIR=$PWD/target/issue-2016/cargo`, with SDKMAN Temurin JDK
21.0.13 on `PATH`:

```text
cargo test -p dotty --test jvm_tasty_mixed_origin_runtime mixed_origin_counter_identity_and_allocation_order_on_strict_jdk21 --locked -- --nocapture: passed
cargo test -p dotty --test jvm_tasty_artifacts --locked -- --test-threads=2: 62 passed
cargo test -p dotty --test jvm_tasty_dual_reference_identity --locked: 2 passed (#1967 direct-reference identity gate)
cargo test -p dotty --test jvm_tasty_artifacts factory_result_constructor_arguments_preserve_original_identity_on_jdk21 --locked: passed (#1880 Pair alias gate)
cargo test -p dotty --test jvm_tasty_artifacts nested_constructor_factory_executes_original_rust_artifacts_on_jdk21 --locked: passed (#1918 nested-constructor gate)
cargo test -p dotty --test jvm_tasty_artifacts sibling_constructor_batch_executes_original_rust_artifacts_with_independent_pair_identity --locked: passed
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test -p dotty-classfile --test classfile_jvm_encoder -p dotty-jvm --lib -p dotty --test jvm_source_compilation --test jvm_stateful_class_contract --locked -- --nocapture: passed; required JDK checks ran
cargo fmt --all -- --check: passed
python3 tools/check-frontend-boundaries.py: passed
cargo clippy --workspace --all-targets --locked -- -D warnings: passed
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked: passed
cargo test --workspace --all-targets --locked -- --test-threads=2: passed with DOTTY_CLASSFILE_REQUIRE_JDK=1
git diff --check: passed
```

The following repository checks passed: `cargo fmt --all -- --check`,
`python3 tools/check-frontend-boundaries.py`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`,
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`, and
`git diff --check`. The complete workspace suite is therefore not recorded as
passing; the pinned Scala/JDK environment gates remain outstanding.
