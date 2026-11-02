# Dual-reference formal forwarding audit (#1960)

This is an evidence-only audit for [issue #1960](https://github.com/scytrowski/dotty-rs/issues/1960). It does not enable paired JVM/TASTy production for two reference formals or change producer semantics. Scala-produced files under [`scala39/example`](../tests/fixtures/jvm_tasty_dual_reference_forwarding/scala39/example) are kept separate from Rust-generated JVM output. The Rust session test emits only classfiles; no Rust TASTy artifact is claimed for this source.

## Pinned baseline and Scala oracle

The audit started from `origin/main` at `fd090895e587852f9feca5599d4ddc55211ce81b`. Its only producer source unit is [`PairFactory.scala`](../tests/fixtures/jvm_tasty_dual_reference_forwarding/PairFactory.scala), SHA-256 `48e4ab7b3038eda26f9070a5879a9cd71d4eb51f69277d1eafcff53bda28610e`. Scala is pinned to revision `777528f19a58e794c9954a42f433373472ec57f8`, staged compiler `3.9.0-bin-SNAPSHOT-git-777528f` (compiler JAR SHA-256 `b56626390028bd06347e3549cf271a03ca7d2e7be491ceff630ccee94e439f4f`), and Temurin JDK `21.0.13+9` (`java` SHA-256 `9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`). The single producer file was compiled with `-release 21` into an empty directory. The output contains exactly three `.class` files and three `.tasty` files; all classfiles are major version 65 and all TASTy files are 28.9.0. Per-file byte sizes, hashes, UUIDs, and links are recorded in [`oracle.json`](../tests/fixtures/jvm_tasty_dual_reference_forwarding/oracle.json). The `javap -v -p -c` and official `scalac -print-tasty` snapshots are in [`oracle/`](../tests/fixtures/jvm_tasty_dual_reference_forwarding/oracle/).

`PairFactory.wrap` has descriptor:

```text
(Lexample/Counter;Lexample/Counter;)Lexample/Pair;
```

The observed method code reports `stack=4`, `locals=3`, `args_size=3`. Symbolic decoding of the actual code gives:

1. `new example/Pair`
2. `dup`
3. `aload_1`
4. `aload_2`
5. `invokespecial example/Pair.<init>(Lexample/Counter;Lexample/Counter;)V`
6. `areturn`

There are no `Counter` allocations or `Counter.<init>` invocations in `wrap`. The Rust source-session test separately decodes the normal backend artifact and asserts this descriptor, stack/locals, opcode order, exact selected constructor owner and descriptor, and equality with the artifact published by the source pipeline. No constant-pool indexes or program counters are part of the asserted contract.

The Scala `PairFactory.tasty` decoder snapshot reports `DEFDEF wrap` at address 30; its distinct `PARAM first` and `PARAM second` declarations are at addresses 33 and 42. Their types are `example.Counter` and the result type is `Pair`. The body is `APPLY(SELECTin(NEW(Pair)), TERMREFdirect(33), TERMREFdirect(42))`; the selected signed constructor name is `<init>[Signed Signature(List(example.Counter, example.Counter),example.Pair) @<init>]`. The outer `APPLY` is at address 55, selected function at 57, `NEW` at 60, and direct references at 67 and 69. The two direct-reference targets are distinct formal declaration addresses.

`Pair.tasty` is a separate callee declaration: its primary constructor owns `first` and `second` `PARAM` nodes at addresses 42 and 49; `left` and `right` are separate `VALDEF` nodes at addresses 60 and 76. These classfile and TASTy observations are from Scala-produced oracle files, not from the Rust producer.

The independent Scala-only [`Consumer.scala`](../tests/fixtures/jvm_tasty_dual_reference_forwarding/Consumer.scala), SHA-256 `ee3c2e2a5f519d796868831a8a8d6b40d4dfcef06ebf2ec3e87ad788a44f63e8`, compiles against only the Scala-produced producer directory and Scala runtime. Under `java -Xverify:all -ea` it checks `wrap(a,b)` identity and independent mutation, `wrap(b,a)` positional reversal and identity, and intentional `wrap(a,a)` aliasing. This is an oracle-consumer test, not Rust/Scala interoperability.

## One canonical Rust frontend session

`dual_reference_factory_wrap_records_the_genuine_source_session_and_first_proof_error` runs the fixture through the real lexer, parser, namer, source typer, and normal JVM source backend. One `SourceId(19601)` retains the source AST, `SourceSemanticIndex`, typed AST, `SourceTypedIndex`, `SemanticStore`, selected symbols, and output classfiles. The test records and asserts original declaration/reference relationships; these run-local arena IDs are evidence for this one session only.

| Source item | Identity and relationship |
| --- | --- |
| Classes in source order | `Counter=SymbolId(40)`, `Pair=SymbolId(41)`, `PairFactory=SymbolId(42)` |
| `PairFactory.wrap` | source `TreeId(55)`, `SymbolId(60)`, method type `TypeId(71)`, owner `PairFactory`; result is `Pair` |
| Caller formals | `first`: source `TreeId(45)`, `SymbolId(61)`, `TypeId(66)`, span `377..391`; `second`: `TreeId(47)`, `SymbolId(62)`, `TypeId(68)`, span `393..408`. Both are canonical method-owned declarations with distinct `TypeRef`s to the same `Counter` symbol and have no typed declaration roots. |
| Pair constructor | selected constructor `SymbolId(52)`, owned by `Pair`; class-template formals are `SymbolId(50)`/`(51)`, while derived constructor-owned formals are `SymbolId(53)`/`(54)` |
| Outer constructor expression | source/typed `Apply` `TreeId(54)`/`TreeId(18)`; source/typed `Select` `TreeId(51)`/`TreeId(15)`; source/typed `New` `TreeId(50)`/`TreeId(14)` |
| First original argument | source/typed `Ident` `TreeId(52)`/`TreeId(16)`, span `431..436`, resolves to original caller formal `SymbolId(61)`, typed `TypeId(89)` |
| Second original argument | source/typed `Ident` `TreeId(53)`/`TreeId(17)`, span `438..444`, resolves to original caller formal `SymbolId(62)`, typed `TypeId(90)` |

The caller formals and Pair constructor formals are distinct identities even though both pairs have the same two `Counter` parameter types. The formal declaration trees have no typed-root binding; both call-site Ident trees have their original typed-root bindings. The test also clones the actual `SourceTypedIndex`, deliberately maps argument one to the other argument's typed `Ident`, and keeps the original store and arenas unchanged for the normal compilation result.

## Measured first failures and reachable output

| Entry point | First observed result |
| --- | --- |
| Read-only cross-class constructor-call proof on the live session | `SemanticIdentity("both factory formals must have the canonical Int type")` |
| Same proof with a cloned, deliberately stale argument-to-typed edge | The same `SemanticIdentity` error occurs before the bad edge is inspected; the corrupted binding is masked. No later typed-binding result is claimed. |
| Standalone pickler class-state validation on the same active `SourceTyper` session | Class-state validation passes after completing the source-owned default constructor. The distinct-reference method-signature projection then returns `Unsupported { feature: ClassMembers, symbol: Some(SymbolId(60)) }`: its signature projection requires one formal and this source declares two. |
| Paired JVM/TASTy batch plan | `BatchUnit` at phase `cross-class constructor-call identity proof`, wrapping the same `SemanticIdentity` error. The paired-plan callback does not run. |
| Pickler expression validation/writer in the paired path | Masked by the earlier proof error. In the independent pickler path, method-signature projection rejects before an expression receipt is available. No expression-writer failure is inferred. |
| JVM-only source output | Passes. It emits `Counter`, `Pair`, and `PairFactory` classes from the same session; `wrap` has the exact reference descriptor and forwarding bytecode listed above. No TASTy sidecar is emitted by this JVM-only path. |

The inspected `dotty-tasty-pickler::method` planned-class-parameter predicate is an inspection candidate; it is not reported as an executed result for this source because the paired planner stops earlier. The standalone signature projection above is the independently executed pickler result. No producer guard was bypassed, no node or type was forged, and no output path was made to accept the source.

## Method signature increment (#1963)

Issue [#1963](https://github.com/scytrowski/dotty-rs/issues/1963) uses the same original `PairFactory.scala` source and one frontend session described above. Before the increment, its ordinary method signature was rejected in two bounded pickler paths: `PicklePlan::method` returned `MalformedType { role: Parameter }` for its two planned-class formals, while the read-only class-state signature proof returned `Unsupported { feature: ClassMembers }` because that proof accepted only one formal.

The signature contract now accepts exactly one existing or two same-class planned sibling reference formals in one ordinary plain clause. Both formals retain their distinct method-owned source `SymbolId`s, canonical `ValueParam` declarations, original order, matching `MethodType` entries, public visibility and exact same-source class `TypeRef`; the result must be a different canonical planned sibling class with the correct package prefix. Mixed primitive/reference pairs, different class formals, third formals, and non-plain or exotic clauses remain unsupported. The standalone projection emits separate ordered `PARAM first: Counter` and `PARAM second: Counter` roots followed by `IDENTtpt Pair`, matching the formal/type shape and ordering shown in the pinned Scala snapshot above.

The focused Rust source-session regression verifies the two distinct identities, their source ownership and order, the `Counter` parameter class, the `Pair` result class, the separate PARAM addresses/names, and the projected `Counter`/`Pair` type names. Issue #1964 extends that signature support through the exact original `new Pair(first, second)` body: its paired-plan adapter forwards the ordered source/typed proof, the pickler validates both direct formal references, and the writer emits two direct `TERMREF` leaves in order. The Rust output passed the pinned Scala 3.9 `-decompile` and `-from-tasty` reader checks. See the [two-argument constructor-call implementation record](jvm-tasty-multi-argument-constructor-calls.md#direct-formal-pairfactory-body-tasty-1964) for receipt checks and the exact wire shape.

## Joint JVM/TASTy forwarding batch (#1966)

Issue [#1966](https://github.com/scytrowski/dotty-rs/issues/1966) joins the already-proved source receipt and direct-reference TASTy body to the JVM backend. It starts from current `main` at `4591bcab5388fb21b1630f87884a6e46affff7bb` and compiles the unchanged source above through one original frontend session. The adapter returns exactly `Counter.class/.tasty`, `Pair.class/.tasty`, and `PairFactory.class/.tasty`; each classfile is major 65, each sidecar is TASTy 28.9.0, and every classfile `TASTY` attribute matches its own sidecar UUID.

The source-bound classfile check requires `PairFactory.wrap` to retain the method-owned `first` and `second` symbols in local slots 1 and 2, load those references in source order, allocate only `Pair`, call the selected `Pair.<init>(Counter, Counter)`, and return it. It consumes the whole method body, checks `max_stack=4` and `max_locals=3`, and rejects swapped same-typed `aload` instructions before the batch is returned. The paired artifact regression also checks that the emitted `wrap` TASTy has two distinct ordered PARAM declarations and two `TERMREFdirect` arguments targeting those exact PARAM addresses.

| Rust output | SHA-256 | TASTy UUID |
| --- | --- | --- |
| `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008071c3c00968ac0000000000008193` |
| `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | `008071c3c00968ac0000000000008193` |
| `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d7441c7f00000000000008198` |
| `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | `000b7d8d7441c7f00000000000008198` |
| `example/PairFactory.class` | `13adef3d34ffbf9af3bf39cc996a29e1a45d8e76c0082012fcee898efd323dae` | `00f0b42f44fbadc80000000000008195` |
| `example/PairFactory.tasty` | `4df88f35f03867ae637ed6131e4fca4e7c264bf10db2b9165c213ee85eec0fdb` | `00f0b42f44fbadc80000000000008195` |

Two fresh sessions produce identical bytes and UUIDs for all six original outputs. A renamed package/class variant and a class-declaration reorder each produce stable output across fresh sessions. The source-bound swapped-load negative first fails during `cross-class invocation` with `expected aload_1 in bounded call bytecode, found opcode 0x2c`. The test-only stage/promotion publisher rejects handled writes after each of the six staged artifacts and before/after promotion, leaves no accepted destination on failure, and preserves an existing destination sentinel. These checks establish handled-failure safety only; they do not claim crash-atomic multi-file durability.

The strict batch-reader regression verifies the Scala checkout at revision `777528f19a58e794c9954a42f433373472ec57f8` and compiler version `3.9.0-bin-SNAPSHOT-git-777528f`. Scala `-decompile` and `-print-tasty` read each of the three Rust sidecars; Scala `-from-tasty -release 21` reconstructs all three classes from that batch. The gate is enabled by `DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_FORWARDING_BATCH=1` in the pinned Scala consumer workflow.

Reproduce the batch gate from a persistent issue worktree with its own scratch and build directories:

```sh
export SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8
export JAVA_HOME=/path/to/temurin-21
export PATH="$JAVA_HOME/bin:$PATH"
export TMPDIR="$PWD/.issue1966-tmp"
export CARGO_TARGET_DIR="$PWD/.issue1966-target"
export CARGO_BUILD_JOBS=2
export DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_FORWARDING_BATCH=1

cargo test -p dotty --test jvm_tasty_artifacts \
  dual_reference_forwarding_emits_six_linked_artifacts_with_ordered_formal_loads \
  --locked -- --nocapture
```

## Scala validity and Rust first-stage controls

The reproducible control compiler is [`verify-controls.py`](../tests/fixtures/jvm_tasty_dual_reference_forwarding/verify-controls.py), run by [`rebuild-oracle.sh`](../tests/fixtures/jvm_tasty_dual_reference_forwarding/rebuild-oracle.sh). The Rust test creates an independent paired-planner source session for each control and asserts that no batch callback publishes output.

| Control | Pinned Scala 3.9 | First Rust result |
| --- | --- | --- |
| Rename package, classes, method, and formal names | valid | cross-class proof: canonical-`Int` formal guard |
| Reorder the three declarations | valid | cross-class proof: canonical-`Int` formal guard |
| `wrap(first, first)` intentional alias | valid | cross-class proof: canonical-`Int` formal guard |
| Reverse arguments or method formal order | valid | cross-class proof: canonical-`Int` formal guard |
| Add an unused third method formal | valid | cross-class proof: one plain clause/body shape guard |
| Contextual method formal | valid | complete JVM unit validation: `UnsupportedMethodShape::ParameterCountMismatch` |
| Defaulted method formal | valid | cross-class proof: canonical-`Int` formal guard |
| Generic method | valid | member planning: generic methods unsupported |
| Overload `wrap` | valid | cross-class proof: canonical-`Int` formal guard |
| Mixed direct reference and `new Counter(...)` expression | valid | cross-class proof: canonical-`Int` formal guard; mixed expression shape remains masked |
| Use a `Pair` subclass as the returned outer constructor | valid | source collection: explicit parent classes unsupported |
| Unused erased formal (with `-language:experimental.erasedDefinitions`) | valid | parser recovery on erased formal syntax |
| Third argument to Pair constructor | invalid Scala arity | method body typing: `ApplicationArityMismatch { expected: 2, actual: 3 }` |
| Int passed where Pair requires Counter | invalid Scala type | method body typing: `ApplicationArgumentTypeMismatch` |
| One argument to Pair constructor | invalid Scala arity | method body typing: `ApplicationArityMismatch { expected: 2, actual: 1 }` |
| Duplicate method formal name | invalid Scala declaration | method body typing: `AmbiguousTermReference` |
| Foreign same-spelled `Counter` formal passed to `example.Pair` | invalid Scala type ownership | source collection rejects the extra top-level non-class sibling before typing the mismatch |
| Parseable sidecar with altered header UUID | binary corruption | classfile/TASTy link validator returns `StaleTastyLink` |
| Factory descriptor with a same-length class-name mutation | binary corruption | decoded descriptor differs from required wrap ABI |
| Stale source-to-typed argument binding | semantic adapter corruption | proof returns earlier canonical-`Int` guard; stale edge is explicitly masked |

Valid Scala variations are not treated as invalid source merely because the Rust subset rejects or masks them. Invalid source controls are retained separately. The Rust test does not invent a foreign symbol or rewrite the canonical semantic index to manufacture a constructor target.

## Reproduction and validation

Use the pinned Scala source checkout and JDK 21, and keep all task-local output outside crowded `/tmp`:

```sh
export SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8
export JAVA_HOME=/path/to/temurin-21
export PATH="$JAVA_HOME/bin:$PATH"
export TMPDIR=/var/tmp/dotty-rs-issue-1960-tmp
export CARGO_TARGET_DIR=/var/tmp/dotty-rs-issue-1960-target
export DOTTY_ISSUE_1960_ORACLE_DIR=/var/tmp/dotty-rs-issue-1960-oracle/rebuild
export DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_FORWARDING_ORACLE=1

tests/fixtures/jvm_tasty_dual_reference_forwarding/rebuild-oracle.sh
cargo test -p dotty --test jvm_tasty_dual_reference_forwarding --locked
cargo test -p dotty --test jvm_source_compilation dual_reference_factory_wrap_records_the_genuine_source_session_and_first_proof_error --locked
cargo test -p dotty --test jvm_tasty_dual_reference_forwarding source_variants_record_first_reachable_paired_stage_without_forging_ids --locked
```

The strict Rust test reruns the Scala producer build, independent consumer, all Scala validity controls, and byte-for-byte artifact comparisons. The fixture tests also verify class/TASTy UUID links, version numbers, exact constructor descriptors and source/typed identity relationships. The checked-in oracle artifacts are Scala outputs only.

Issue #1960's required final gates remain the pinned Scala/JDK gates plus `cargo fmt --all -- --check`, `python3 tools/check-frontend-boundaries.py`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`, `cargo test --workspace --all-targets --locked`, the mandatory `DOTTY_CLASSFILE_REQUIRE_JDK=1` test command, and `git diff --check`.

## Original Rust factory identity proof (#1967)

Issue [#1967](https://github.com/scytrowski/dotty-rs/issues/1967) checks the runtime behavior of the unchanged source above, from one call to `emit_paired_jvm_tasty_batch` through the genuine Rust frontend. This run starts from `origin/main` `3a854831b9c074261e31e82d325c0350f606b95a`, uses source SHA-256 `48e4ab7b3038eda26f9070a5879a9cd71d4eb51f69277d1eafcff53bda28610e`, Rust `1.98.1`, and Temurin `21.0.13+9`. The isolated batch contains only the three original producer classes and their three linked TASTy sidecars. A separate Java harness is compiled with `javac --release 21`; runtime classpath order is harness classes followed by that Rust batch. There are no Scala-produced artifacts in the JVM path. The TASTy files are included and hashed as producer outputs, but this test makes no independent Scala interoperability claim.

The Rust classfile decoder and `javap -v -p -c` both observe `PairFactory.wrap(LCounter;LCounter;)LPair;` with `max_stack=4`, `max_locals=3`, classfile major 65, and exactly this code. Program counters below are parsed from the actual `javap` output and asserted by the focused test:

| PC | Instruction |
| ---: | --- |
| 0 | `new example/Pair` |
| 3 | `dup` |
| 4 | `aload_1` |
| 5 | `aload_2` |
| 6 | `invokespecial example/Pair.<init>(Lexample/Counter;Lexample/Counter;)V` |
| 9 | `areturn` |

The Rust CFG analyzer independently accepts that sequence and reports the same stack and local maxima. The selected invocation is the exact two-reference `Pair.<init>` Methodref. `wrap` contains one `new`, no `Counter` allocation, and no call to a `Counter` constructor.

Under `java -Xverify:all -ea`, the separate harness reflects `Pair.left` and `Pair.right` and compares them by `==` to the original `Counter` arguments. It observed `7/3`, then `42/3`, then `42/-9`; `wrap(b,a)` retained positional reversal; repeated calls through multiple `PairFactory` instances returned separate `Pair` objects while retaining the same passed `Counter`s. For the intentional alias case, `wrap(a,a)` stored that identical object in both fields, and after `a.set(100)` both reads were `100`. The harness output was:

```text
forward=7/3,42/3,42/-9;reverse=-9/42;repeat=true;alias=100/100
```

All six Rust artifacts were SHA-256 hashed before the Java compile and remained byte-for-byte unchanged after both compilation and execution. The digests from this source-session run are:

| Artifact | SHA-256 |
| --- | --- |
| `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` |
| `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` |
| `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` |
| `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` |
| `example/PairFactory.class` | `13adef3d34ffbf9af3bf39cc996a29e1a45d8e76c0082012fcee898efd323dae` |
| `example/PairFactory.tasty` | `4df88f35f03867ae637ed6131e4fca4e7c264bf10db2b9165c213ee85eec0fdb` |

Negative tests use isolated modified copies and report the first actual rejecting layer. In particular, swapped same-typed loads are valid bytecode and pass verification; they fail only when the runtime reflection identity assertion checks the positional contract.

| Modified copy | First observed rejection |
| --- | --- |
| Swap `aload_1` and `aload_2` | Runtime identity assertion: `left` is not the original first argument |
| Load local 1 twice | Runtime identity assertion: `right` is not the original second argument |
| Change the selected constructor owner to `java/lang/Object` | JVM verifier: `VerifyError: Call to wrong <init> method` |
| Change the selected constructor descriptor to `()V` | JVM verifier: `VerifyError: Bad operand type when invoking <init>` |
| Remove `Pair.new` | JVM verifier: `VerifyError: Bad type on operand stack` |
| Insert a second `Pair.new` before return | JVM verifier: `VerifyError: Bad return type` |
| Remove the selected `invokespecial` | JVM verifier: `VerifyError: Operand stack overflow` |
| Insert a second selected `invokespecial` after initialization | JVM verifier: `VerifyError: Bad type on operand stack` |
| Remove `dup` before constructor initialization | JVM verifier: `VerifyError: Operand stack underflow` |
| Set `max_stack` to 3 | JVM verifier: `VerifyError: Operand stack overflow` |
| Set `max_locals` to 2 | JVM classfile loading: `ClassFormatError: Arguments can't fit into locals` |
| Omit `Counter.class` | JVM linkage: `NoClassDefFoundError: example/Counter` |
| Omit `Pair.class` | JVM linkage: `NoClassDefFoundError: example/Pair` |

Reproduce the runtime proof from the repository root with the pinned JDK. `TMPDIR` and `CARGO_TARGET_DIR` must point to persistent worktree-local directories:

```sh
export JAVA_HOME=/path/to/temurin-21.0.13
export PATH="$JAVA_HOME/bin:$PATH"
export TMPDIR="$PWD/.issue1967-tmp"
export CARGO_TARGET_DIR="$PWD/.issue1967-target"
export CARGO_BUILD_JOBS=2
export DOTTY_CLASSFILE_REQUIRE_JDK=1
export JAVA_TOOL_OPTIONS="-Djava.io.tmpdir=$TMPDIR -XX:-UsePerfData"

cargo test -p dotty --test jvm_tasty_dual_reference_identity \
  dual_reference_factory_preserves_forwarded_argument_identity_on_strict_jdk21 \
  --locked -- --nocapture
```

## Official Scala source consumer of the original Rust batch (#1969)

[`Consumer.scala`](../tests/fixtures/jvm_tasty_dual_reference_forwarding/rust-consumer/Consumer.scala) is compiled separately from the unchanged Rust producer source and against the exact six-file batch from #1966. It checks `PairFactory.wrap(first, second)`, the original `Counter` and `Pair` members, values `7/3`, independent mutation to `42/-9`, positional reversal, and intentional `wrap(a, a)` identity through reflection of the existing private `left` and `right` fields. The Java process uses `-Xverify:all -ea`; its output is `rust-dual-reference-forwarding=7/3,42/-9,-9/42; alias=true`.

The Rust producer source SHA-256 is `48e4ab7b3038eda26f9070a5879a9cd71d4eb51f69277d1eafcff53bda28610e`; the separate Scala consumer source SHA-256 is `eee2e18901dfa37d29570d81c92098e93ab2f1a36534970241013e06feea66ec`. Two fresh source sessions emit byte-identical classfiles and sidecars. The strict test records every original artifact hash before compiling the client and checks all six hashes again after compilation, runtime, Scala inspection, member lookup probes, and negative controls.

| Original Rust artifact | SHA-256 | Linked TASTy UUID |
| --- | --- | --- |
| `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `008071c3-c009-68ac-0000-000000008193` |
| `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | `008071c3-c009-68ac-0000-000000008193` |
| `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `000b7d8d-7441-c7f0-0000-000000008198` |
| `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | `000b7d8d-7441-c7f0-0000-000000008198` |
| `example/PairFactory.class` | `13adef3d34ffbf9af3bf39cc996a29e1a45d8e76c0082012fcee898efd323dae` | `00f0b42f-44fb-adc8-0000-000000008195` |
| `example/PairFactory.tasty` | `4df88f35f03867ae637ed6131e4fca4e7c264bf10db2b9165c213ee85eec0fdb` | `00f0b42f-44fb-adc8-0000-000000008195` |

The strict Scala gate used source revision `777528f19a58e794c9954a42f433373472ec57f8`, compiler `3.9.0-bin-SNAPSHOT-git-777528f`, and Temurin `21.0.13+9`. The local staged compiler JAR hash observed during this run was `172c8b8384e75ee24d327fce5ec788582bf314f66bf9983ba57e0cac6c3ff498`; staged sbt artifacts are build outputs and this hash is provenance, not a reproducibility assertion. The approved published runtime jars were:

| Published runtime | SHA-256 |
| --- | --- |
| `scala3-library_3-3.9.0.jar` | `9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba` |
| `scala-library-3.9.0.jar` | `b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd` |

The client runtime classpath contained, in order, the compiled `Consumer.scala` output, the original Rust batch directory, `scala3-library_3-3.9.0.jar`, and `scala-library-3.9.0.jar`. It excluded every masked control and `-from-tasty` reconstruction directory. Official `-decompile`, `-print-tasty`, and `-from-tasty` all accepted the original sidecars. The Factory TASTy has distinct `PARAM first` and `PARAM second` nodes (addresses 32 and 41 as direct declaration roots); its original `APPLY` invokes the signed `Pair.<init>(Counter, Counter)` and its two direct `TERMREF`s target those corresponding method parameters. No synthetic `Counter` allocation appears in the body.

Compilation-only member probes masked each selected classfile member while preserving its genuine sidecar, then paired it with a classfile-only copy whose matching `.tasty` and `TASTY` link were removed. Scala accepted TASTy lookup for `Counter.get`, `Counter.set`, `Pair.leftValue`, `Pair.rightValue`, `Pair.<init>(Counter, Counter)`, and `PairFactory.wrap(Counter, Counter): Pair`. The matched classfile-only controls rejected the methods with `[E008] Not Found Error`; the constructor-only control rejected with `[E173] Reference Error`. Masked files never entered the runtime classpath.

The strict negative controls observed these first failures: each class's missing or misplaced sidecar failed with Scala's “`.tasty` file is missing” binary-reader diagnostic; each truncated sidecar failed at `error while loading <class>,`; each UUID mismatch was reported as that classfile being out of sync with its TASTy file (promoted to failure by `-Werror`). Wrong Pair-constructor and original-batch `wrap` arities failed with `[E171] Type Error`; foreign same-spelled `Counter` ownership and a wrong `wrap` result type failed with `[E007] Type Mismatch Error`; an absent package/class failed with `[E006] Not Found Error`; and an absent method or renamed TASTy member failed with `[E008] Not Found Error`. A separate pinned-Scala producer source variant removes `wrap`'s `second` formal while keeping the `Pair(first, second)` producer unchanged by forwarding `first` twice. Its TASTy `wrap` declaration contains exactly one `PARAM`. With the corresponding `wrap` method removed from the classfile but its TASTy sidecar retained, a focused two-argument call fails first with `[E007] Type Mismatch Error` (the `(Counter, Counter)` actual tuple does not match the sole `Counter` formal); the matched classfile-only control (without the TASTY link or sidecar) fails first with `[E006] Not Found Error` because the Scala-produced class is no longer a source class. This producer-declaration control is separate from the original-batch wrong-call-arity control. The malformed direct formal target was accepted during lazy signature lookup but official `-decompile` forced the body read and failed with `TreeWithoutOwner`. A type-correct swap of the two TASTy formal references was accepted by Scala and decompiled as `new example.Pair(second, first)`, so the original-source proof and positional runtime assertions remain the evidence for order.

Changing the Factory constructor `Methodref` to the existing `java.lang.Object.<init>()V` reference and changing the Pair constructor descriptor were compilation-only classfile mutations: the genuine TASTy still supplied source lookup, while the runtime controls failed with `VerifyError: Bad operand type when invoking <init>` and `VerifyError: Bad local variable type`, respectively. Removing the original Counter classfile still allowed TASTy source compilation and then failed at runtime with `NoClassDefFoundError: example/Counter`. Original hashes remained unchanged. A separate package/class/method/formal rename and a class declaration reorder each compiled and ran; fresh sessions for each variant byte-matched all six outputs. The renamed producer and consumer source hashes were `6127fcfacf89bd83c666119b0a32ba507f8bf0ce3b3659ece1d228f1fb1e044c` and `a1bbdb00316bf1ae7b35317d499e315af929379c5d435db88652ba6699cbe9c6`; the reordered producer and consumer hashes were `090c49424ce60229bd5370cd24728ba021037766f04d7b194cedc18f738fdf6d` and `eee2e18901dfa37d29570d81c92098e93ab2f1a36534970241013e06feea66ec`.

The pinned workflow requires `DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_FORWARDING_CONSUMER=1` and runs `jvm_tasty_dual_reference_forwarding_scala_consumer` with the inherited Scala, JDK, and cache checks. Required mode fails when the compiler checkout, JDK, runtime cache, or dedicated `TMPDIR` is absent; task output stays outside `/tmp`. Reproduce the strict test with:

```sh
export SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8
export JAVA_HOME=/path/to/temurin-21
export PATH="$JAVA_HOME/bin:$PATH"
export COURSIER_MAVEN_ROOT=/path/to/pinned-maven-cache
export TMPDIR="$PWD/.issue1969-tmp"
export CARGO_TARGET_DIR="$PWD/.issue1969-target"
export CARGO_BUILD_JOBS=2
export DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_FORWARDING_CONSUMER=1

cargo +1.98.1 test --test jvm_tasty_dual_reference_forwarding_scala_consumer \
  --locked -- --nocapture --test-threads=1
```

## Final integrated release gate (#1971)

The final gate started from clean merged `main` at `bd90c1a82d89f315c7613570d6d5f518e0e76d8b`. The seven reviewed prerequisites are ancestors of that commit. Their final PR heads had exact-head `READY_FOR_MERGE` receipts and successful CI and pinned Scala workflow runs:

| Issue / PR | Reviewed head | Merge commit | CI run | Pinned Scala run |
| --- | --- | --- | --- | --- |
| [#1960 / #1974](https://github.com/scytrowski/dotty-rs/pull/1974) | `bc5a87f2ea59b2053da5a2083889330dee4a3bb5` | `148883c222d86cdc2a90bb50ccc7114acd69c99d` | [41333012628](https://github.com/scytrowski/dotty-rs/actions/runs/41333012628) | [41333012689](https://github.com/scytrowski/dotty-rs/actions/runs/41333012689) |
| [#1961 / #1976](https://github.com/scytrowski/dotty-rs/pull/1976) | `93883f7d90416b4860073f92668e370785d4e9a6` | `f24adf9b7bfaccabd6c8085a76b4aa63aba8b7e5` | [41338223952](https://github.com/scytrowski/dotty-rs/actions/runs/41338223952) | [41338223943](https://github.com/scytrowski/dotty-rs/actions/runs/41338223943) |
| [#1963 / #1979](https://github.com/scytrowski/dotty-rs/pull/1979) | `e8277c49bc714a3f3e904f3f1c6dfcef6f075b5e` | `15a8558c59f07df95c4291662f2baa73ea8919d0` | [41343829750](https://github.com/scytrowski/dotty-rs/actions/runs/41343829750) | [41343829748](https://github.com/scytrowski/dotty-rs/actions/runs/41343829748) |
| [#1964 / #1981](https://github.com/scytrowski/dotty-rs/pull/1981) | `0b13ea24761f27272f316f2823446d73437c9c33` | `4591bcab5388fb21b1630f87884a6e46affff7bb` | [41354616230](https://github.com/scytrowski/dotty-rs/actions/runs/41354616230) | [41354616180](https://github.com/scytrowski/dotty-rs/actions/runs/41354616180) |
| [#1966 / #1984](https://github.com/scytrowski/dotty-rs/pull/1984) | `a35f5556827b89aad12487941b04cd9381f7ba2d` | `52411c8c5ca96e031c99e18ae8d3c7c3e7205f50` | [41362348601](https://github.com/scytrowski/dotty-rs/actions/runs/41362348601) | [41362348633](https://github.com/scytrowski/dotty-rs/actions/runs/41362348633) |
| [#1967 / #1988](https://github.com/scytrowski/dotty-rs/pull/1988) | `0c654d1cda7cac11b79a2703bc2a087f0a7189af` | `8b638784b57a02bfbd8f2a467d472029cb3657f9` | [41366981803](https://github.com/scytrowski/dotty-rs/actions/runs/41366981803) | [41366981799](https://github.com/scytrowski/dotty-rs/actions/runs/41366981799) |
| [#1969 / #2001](https://github.com/scytrowski/dotty-rs/pull/2001) | `8c4ec2356f68ee53dbaaf8a8833b1912f7b59d0d` | `1c23213e3716fc137a2fee16d6f4d0541961fa70` | [41414695839](https://github.com/scytrowski/dotty-rs/actions/runs/41414695839) | [41414695978](https://github.com/scytrowski/dotty-rs/actions/runs/41414695978) |

Issue #1961 remains administratively open, but PR #1976 is merged and its reviewed head and workflow receipts are recorded above. These prerequisite receipts are not counted as the fresh integrated-main runs below.

### AC1–AC10 integrated evidence

| AC | Integrated evidence at `bd90c1a` |
| --- | --- |
| 1. Provenance | Original Rust source [`PairFactory.scala`](../tests/fixtures/jvm_tasty_dual_reference_forwarding/PairFactory.scala), SHA-256 `48e4ab7b3038eda26f9070a5879a9cd71d4eb51f69277d1eafcff53bda28610e`. Separate Java client [`JavaConsumer.java`](../tests/fixtures/jvm_tasty_dual_reference_forwarding/JavaConsumer.java), SHA-256 `d6d09ce17fc8b2e4d048dd03bb34e05bb439b08365f0932d7843045e75f16d82`. Separate Scala client `rust-consumer/Consumer.scala`, SHA-256 `eee2e18901dfa37d29570d81c92098e93ab2f1a36534970241013e06feea66ec`. Rust/Cargo `1.98.1`; Scala source revision `777528f19a58e794c9954a42f433373472ec57f8`, compiler reports `3.9.0-bin-SNAPSHOT-git-777528f`; the staged `with_compiler.jar` hash observed in this run is `8f41217ebb685b05084242c6664f3543c573f08f20cc0638714101c8ed92e614` (provenance only, not a stable pin). Temurin `21.0.13+9`, Java SHA-256 `9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`. `-release 21` yields major version 65; sidecars are TASTy 28.9.0. Approved runtime hashes are listed above. |
| 2. One authentic frontend session | `dual_reference_forwarding_emits_six_linked_artifacts_with_ordered_formal_loads` emitted the unchanged source in two fresh sessions with `SourceId(19660)` and `SourceId(19661)`. `dual_reference_factory_wrap_records_the_genuine_source_session_and_first_proof_error` retains the source and typed roots, semantic indexes, original method-owned `first` and `second` symbols, selected `Pair` constructor, and direct argument `Ident` trees. The caller formals remain distinct from `Pair`'s constructor-owned formals. |
| 3. TASTy proof | `rust_pair_factory_tasty_emits_ordered_direct_formal_constructor_arguments` and the strict Scala reader assert two ordered `PARAM` roots in `wrap`, one `APPLY(SELECTin(NEW(Pair)))`, and direct `TERMREF`s to the corresponding original method formals. `Pair.tasty` retains its two constructor parameters and separate immutable aliases. Scala `-decompile`, `-print-tasty`, and `-from-tasty` read the original Rust sidecars. |
| 4. Six artifacts and determinism | The two fresh sessions produced the same paths, all six byte arrays and UUIDs. The source-bound test also checks classfile major 65, TASTy 28.9, sidecar UUID links, descriptors, method body and exact source-order loads. The current integrated inventory follows. |
| 5. JVM lowering | `PairFactory.wrap(LCounter;LCounter;)LPair;` has `max_stack=4`, `max_locals=3`, and symbolic instructions `new Pair; dup; aload_1; aload_2; invokespecial Pair.<init>(LCounter;LCounter;)V; areturn`. The wrapper creates no `Counter`. `Pair.<init>` stores its own first and second formals into distinct `left` and `right` fields. |
| 6. Java 21 runtime | The separate Java client compiled with `javac --release 21` against only the original Rust batch and ran with `java -Xverify:all -ea`. Output: `forward=7/3,42/3,42/-9;reverse=-9/42;repeat=true;alias=100/100`. It checks reflected identity, separate mutations, reversed positions, repeated calls, distinct factory instances and intentional aliasing. The six hashes below were unchanged before compilation, after compilation and after runtime. |
| 7. Scala 3.9 runtime and lookup | The strict Scala client compiled against the six original Rust artifacts and the two approved runtime jars, then ran the unchanged Rust classes with output `rust-dual-reference-forwarding=7/3,42/-9,-9/42; alias=true`. It verifies `Counter.get/set`, `Pair.leftValue/rightValue` and its constructor, and `PairFactory.wrap` using TASTy-only lookup plus matched classfile-only failures. The six hashes remained unchanged after compilation, runtime, decompile and negative probes. `-from-tasty` outputs are inspection-only and absent from the positive runtime classpath. |
| 8. First-failure and publisher negatives | Proof tests reject stale provenance, wrong types and arity at the source/proof layer. A same-typed positional swap is valid bytecode and fails the runtime identity assertion. Wrong constructor owners/descriptors, missing or duplicate allocation/calls, invalid stack/local bounds and missing classes fail at the JVM verifier, classfile loader or linker. Scala controls cover malformed, missing, misplaced and UUID-mismatched TASTy, wrong owner/type/result, wrong arity, missing members, malformed TERMREF ownership, and the one-formal producer with its matched classfile-only control. The six-file publisher test injects handled failures after every staged write and before/after promotion; no partial destination is accepted and an existing sentinel is preserved. This is not a crash-atomicity claim. |
| 9. Required gates | The inherited strict Scala group passed with every required consumer variable set to `1`, including `DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_FORWARDING_CONSUMER=1`, `DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_FORWARDING_BATCH=1`, and `DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1`. The separate pickler alias-oracle test passed. The mandatory `DOTTY_CLASSFILE_REQUIRE_JDK=1` verifier/JVM/source/paired/publisher gate passed. `cargo fmt`, the frontend boundary guard, Clippy, Rustdoc, all workspace tests and `git diff --check` also passed. These are integrated-baseline reruns, distinct from predecessor workflow receipts. |
| 10. Evidence and limits | This ledger records the common baseline, prerequisite receipts, pinned source/client/runtime inputs, artifacts, runtime results, first failures and strict commands. No producer semantics were added. Mixed argument expressions, more than two arguments, generic/default/overloaded/contextual constructors, external ABI composition and public compiler APIs remain outside this release gate. |

| Rust artifact path | Classfile SHA-256 | TASTy SHA-256 | Linked UUID |
| --- | --- | --- | --- |
| `example/Counter.class` / `.tasty` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | `008071c3-c009-68ac-0000-000000008193` |
| `example/Pair.class` / `.tasty` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | `000b7d8d-7441-c7f0-0000-000000008198` |
| `example/PairFactory.class` / `.tasty` | `13adef3d34ffbf9af3bf39cc996a29e1a45d8e76c0082012fcee898efd323dae` | `4df88f35f03867ae637ed6131e4fca4e7c264bf10db2b9165c213ee85eec0fdb` | `00f0b42f-44fb-adc8-0000-000000008195` |

`PairFactory.tasty` has distinct direct roots for `PARAM first` and `PARAM second` (addresses 32 and 41 in the checked Rust batch); the `APPLY` arguments are direct `TERMREF`s targeting those respective roots. The classfile `TASTY` attribute and sidecar header share the UUID listed for each class.

### Integrated reproduction

`TMPDIR`, Cargo outputs, Scala compiler targets, Maven artifacts and oracle outputs were task-local and outside shared `/tmp`. All published Scala compiler/runtime dependency JARs were checked against the pinned workflow SHA-256 manifest. The strict Scala group used the pinned workflow's 22 test targets and required variables, plus `--locked -- --nocapture --test-threads=1`. The mandatory JDK command was:

```sh
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo +1.98.1 test \
  -p dotty-classfile --test classfile_jvm_encoder \
  -p dotty-jvm --lib \
  -p dotty --test jvm_source_compilation \
  -p dotty --test jvm_tasty_artifacts \
  -p dotty --test jvm_stateful_class_contract \
  --locked -- --nocapture
```

The focused integrated gates were:

```sh
DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_FORWARDING_BATCH=1 \
cargo +1.98.1 test -p dotty --test jvm_tasty_artifacts \
  dual_reference_forwarding_emits_six_linked_artifacts_with_ordered_formal_loads \
  --locked -- --nocapture --test-threads=1

DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo +1.98.1 test -p dotty \
  --test jvm_tasty_dual_reference_identity \
  dual_reference_factory_preserves_forwarded_argument_identity_on_strict_jdk21 \
  --locked -- --nocapture --test-threads=1
```
