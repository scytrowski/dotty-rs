# Mixed-origin Pair constructor arguments audit (#2011)

This is an audit-only result. It records the Scala 3.9.0 representation and
the current Rust first rejection for a Pair constructor call with one existing
method formal and one nested `new Counter(seed)`. It changes no production
semantics and establishes no paired JVM/TASTy feature support.

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
| | `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca7c87842b8dca7c6a8293045` | `008071c3-c009-68ac-0000-000000008193` |
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

The following repository checks passed: `cargo fmt --all -- --check`,
`python3 tools/check-frontend-boundaries.py`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`,
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`, and
`git diff --check`. The complete workspace suite is therefore not recorded as
passing; the pinned Scala/JDK environment gates remain outstanding.
