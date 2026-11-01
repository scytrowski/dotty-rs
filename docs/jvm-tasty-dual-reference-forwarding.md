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
