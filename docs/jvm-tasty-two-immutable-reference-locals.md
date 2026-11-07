# Two immutable reference locals in a Scala 3.9 Pair factory

This is an audit record for issue #2122. It adds oracle fixtures and tests only;
the paired JVM/TASTy producer guards and output behavior are unchanged. The
Scala files under [`tests/fixtures/jvm_tasty_two_immutable_reference_locals/`](../tests/fixtures/jvm_tasty_two_immutable_reference_locals/)
are official Scala compiler inputs and outputs. They are not Rust output and do
not establish Rust/Scala interoperability.

## Frozen source and oracle provenance

[`TwoLocals.scala`](../tests/fixtures/jvm_tasty_two_immutable_reference_locals/TwoLocals.scala)
is the exact source from issue #2122: `Counter`, `Pair`, and `PairFactory`
appear in one source unit; `PairFactory.create(firstSeed, secondSeed)` creates
two explicitly typed immutable `Counter` locals and returns
`new Pair(first, second)`. Its SHA-256 is
`e2411637c892feb474d6d70526d471b524cd7de49a3db2565923cd69736f73c3`.

The source was compiled from an empty output directory with the official Scala
3.9.0 checkout at `777528f19a58e794c9954a42f433373472ec57f8`, using the staged
checkout launcher and Eclipse Temurin 21.0.13+9 with `-release 21`. The Java
binary SHA-256 is
`9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`; the
published Scala 3.9 runtime JAR hashes and the observed local launcher/JAR
receipts are in [`oracle.json`](../tests/fixtures/jvm_tasty_two_immutable_reference_locals/oracle.json).
The local staged compiler JAR hash is an observation, not a pinned dependency.

The manifest records exactly six primary artifacts: `Counter.class` and
`Counter.tasty`, `Pair.class` and `Pair.tasty`, and `PairFactory.class` and
`PairFactory.tasty`. Every class is major version 65. Every sidecar is TASTy
28.9.0. The classfile decoder's 16-byte `TASTY` attribute matches the UUID
printed from its corresponding sidecar. Artifact paths, sizes, SHA-256 values
and UUIDs are recorded in the manifest. The full official disassembly and
`-print-tasty` output are preserved beside it.

The reproducible compiler command is:

```sh
"$SCALA39_ROOT/dist/target/universal/stage/bin/scalac" \
  -Werror -release 21 \
  -d "$OUT/primary" \
  tests/fixtures/jvm_tasty_two_immutable_reference_locals/TwoLocals.scala
```

The strict test recompiles this source, checks the six outputs against the
checked-in oracle files, decodes all three classfiles and sidecars, verifies
the classfile-to-sidecar UUID links, checks the TASTy tree, and checks the
bytecode against `javap -v -p -c`.

## Scala TASTy and JVM observations

The official `PairFactory.create` TASTy method has two `Int` `PARAM` nodes and
the result type `Pair`. Its `BLOCK` is at raw AST address 57. The final Pair
constructor `APPLY` is at address 59 and its two `TERMREFdirect` targets are
75 and 98, the distinct `first` and `second` local `VALDEF` addresses. In the
wire stream the final `APPLY` precedes both `VALDEF` nodes. The source
declaration order remains `first`, then `second`; JVM evaluation follows that
source order.

Each `VALDEF` is explicitly typed `Counter`. Its initializer is a `NEW` and
`APPLY` selecting the signed `Counter(Int)` constructor, with the first
initializer referring to the first original method formal and the second
initializer referring to the second original method formal. The final Pair
application refers to the two original local declaration addresses in
argument order. The complete offsets, name-table entries and selected signed
constructor names are visible in `PairFactory.print-tasty.txt`.

`javap` and the Rust classfile decoder agree on:

| Property | Observed value |
| --- | --- |
| Descriptor | `(II)Lexample/Pair;` |
| `max_stack`, `max_locals` | `4`, `5` |
| `this` and formal slots | `0`, `1`, `2` |
| First allocation | `new Counter`, `dup`, `iload_1`, `Counter.<init>(I)`, `astore_3` |
| Second allocation | `new Counter`, `dup`, `iload_2`, `Counter.<init>(I)`, `astore 4` |
| Pair construction | `new Pair`, `dup`, `aload_3`, `aload 4`, `Pair.<init>(Counter, Counter)` |

Each formal is loaded once for its own Counter initialization. Both initialized
references are loaded once for the final Pair constructor; the source
expressions are not evaluated again.

The independent Scala-only client reads the private Pair fields by reflection
and verifies `create(7, 3)` gives `7/3`, the two Counters are distinct, mutations
remain independent, and another call returns two fresh references. A reversed
source variant with unchanged initialization order passes with `3/7`. A
test-only `PairFactoryAlias` source variant compiles with `new Pair(first,
first)` and the client observes that the two Pair fields reference the same
Counter. These are Scala-only language and runtime controls.

## Authentic Rust source session and first reachable errors

The observations in this section record the #2122/#2123 baseline before the
#2124 ordered constructor argument proof was added.

`two_immutable_reference_locals_preserve_authentic_source_typed_and_jvm_identity`
runs the exact three-class source through the Rust lexer, parser, namer,
`SourceTyper`, and JVM backend in one session. It retains the original
`SourceId`, source arena, typed arena, source indexes, semantic store and JVM
unit.

For the observed run, `SourceId(21220)` owns method `SymbolId(60)`, formals
`SymbolId(61)` and `SymbolId(62)`, and locals `SymbolId(63)` and
`SymbolId(64)`. Both locals have `ScopeId(18)` owned by the original method.
The original local source trees 55 and 62 map through `SourceTypedIndex` to
typed trees 19 and 26, which are the exact declarations retained by their
`TypedLocalBinding`s. Their source positions are 424..467 and 472..517. The
original initializer trees 54 and 61 map to typed trees 17 and 24; both select
the same canonical `Counter` constructor `SymbolId(44)`. Their initializer
argument types retain the original ordered formal symbols. The final source
Pair `APPLY` tree 68 maps to typed tree 32, selects Pair constructor
`SymbolId(52)`, and its typed argument term references target the original
local symbols 63 and 64 in order. The test logs the canonical IDs, type IDs,
scope owner and source spans rather than recovering identity from spelling.

The JVM-only source path succeeds and emits all three classes. Its
`PairFactory.create` has descriptor `(II)Lexample/Pair;`, `max_stack=4`,
`max_locals=5`, and allocates local references in slots 3 and 4. The test
asserts the complete operation/offset sequence: allocate and initialize a
Counter from slot 1, store it in slot 3; allocate and initialize another
Counter from slot 2, store it in slot 4; allocate Pair, load slots 3 and 4 in
order, invoke the canonical Counter and Pair constructor references, and
return. It checks both `new` class references, all constructor owners and
descriptors, stack/local bounds, and the exact encoded local slot operands.
With the required JDK gate enabled, `java -Xverify:all` loads and executes the
Rust-produced Counter, Pair and PairFactory classes. The consumer verifies
7/3 values, distinct local references, independent mutation, and fresh
references across repeated calls.

Calling the existing paired-plan entry point on that same source records this
first error:

```text
BatchUnit {
  phase: "cross-class constructor-call identity proof",
  reason: SourceShape("factory body must be a direct Apply or one-local Block")
}
```

The method currently accepts at most one local statistic. The paired plan
therefore does not produce a source/typed TASTy plan for this method. TASTy
pickler validation/encoding and the final JVM/TASTy contract are masked and
unmeasured for this source. The tests do not bypass the one-local guard to
manufacture a later-stage observation.

## Variation matrix

The Scala compiler test generates each case deterministically from the frozen
source and records its source hash and compiler result. The paired-plan test
runs the Scala-typed cases through the actual existing Rust entry point and
asserts the exact error category, phase and diagnostic variant/payload prefix
for every case. It also asserts the paired-plan callback was not reached.
“Masked” means no later paired-plan callback, pickler or contract stage was
executed.

| Variation | Scala 3.9 | Rust first observed result |
| --- | --- | --- |
| Renamed package, classes, method locals and formals | Accepted | Paired-plan `factory body ... one-local Block` guard |
| Reordered class declarations | Accepted | Same paired-plan guard |
| Reversed `Pair(second, first)` arguments | Accepted; runtime `3/7` | Same paired-plan guard |
| `Pair(first, first)` alias arguments | Accepted; runtime fields alias | Same paired-plan guard |
| Swapped initializer-to-formal references | Accepted | Same paired-plan guard |
| Third immutable local | Accepted | Same paired-plan guard |
| Mutable `var` local | Accepted | Same paired-plan guard |
| Local helper initializer | Accepted | Same paired-plan guard |
| Conditional initializer | Accepted | Same paired-plan guard |
| Nested Counter initializer | Accepted | Same paired-plan guard |
| Branch-local declarations | Accepted | `JVM/TASTy cross-contract validation`, `TastyPlan(BodyType ...)` |
| Missing initializer | Rejected, Scala `[E067]` syntax diagnostic | `method body typing`, `LocalValueRightHandSideMissing` |
| Duplicate local name | Rejected, Scala `[E161]` naming diagnostic | `method body typing`, `DuplicateLocalValue` |
| Use before declaration | Rejected, Scala `[E039]` reference diagnostic | `method body typing`, `TermNameNotFound` |
| Local shadowing an ordinary formal | Rejected, Scala `[E007]` type mismatch | `method body typing`, `RecursiveLocalValueInitializer` |
| Wrong local type | Rejected, Scala `[E007]` type mismatch | `method body typing`, `TypeNameNotFound` |
| Wrong Pair constructor argument types | Rejected, Scala `[E007]` type mismatch | `method body typing`, `ApplicationArgumentTypeMismatch` |
| Wrong Pair constructor signature using Int formals | Rejected, Scala `[E007]` type mismatch | `method body typing`, `ApplicationArgumentTypeMismatch` |

The exact generated-source SHA-256 values and Scala observations are in
`oracle.json` and the required-mode test log. The Rust variation test also
covers renamed and reordered declarations, reversed and aliased Pair
references, swapped initializer formals, third/mutable locals, missing and
wrong local types, a wrong Pair constructor signature, helper/conditional/
nested initializers and branch-local declarations.

Two isolated mutations were applied to copies of the official Scala sidecar.
Changing the first Counter initializer's signed `SELECTin` name from the
observed `Counter(Int)` entry to the observed `Pair(Counter, Counter)` entry
preserved structural TASTy validity; Scala 3.9 `-from-tasty` **accepted** that
mutated file. This records reader behavior only; it is not a claim that the
mutation is a valid source-level tree. Truncating PairFactory TASTy is rejected
by the Rust structural TASTy parser, while Scala 3.9 `-from-tasty` fails with
`IndexOutOfBoundsException`. Both outcomes are captured in the strict test.

At that baseline, stale/foreign final argument bindings, wrong source-to-typed
edges, and Rust-produced TASTy corruption were not measured after the
paired-plan source shape error. The #2124 proof now authenticates the final
source/typed argument roots before the paired planner; later TASTy bytes and
JVM PCs remain unmeasured. No later-stage success is inferred for the Rust
pair.

## #2124 ordered LocalVal pair proof

Issue #2124 composes the two opaque initializer receipts above with the
existing ordered constructor argument proof. Each final Pair argument must be
a direct source and typed `Ident` matched to one of the original local symbols
through its exact typed `TermRef` and source-to-typed tree mapping. A nested
expression or caller formal cannot use the two-local aggregate as partial
evidence. The ordered proof retains both initializer receipts in original
declaration order, independently of the two positional argument origins. Thus
`Pair(first, first)` keeps initializer order `[0, 1]` and use order `[0, 0]`.
The proof retains the Pair constructor's two formal identities separately
from both method-local identities and does not infer distinct runtime objects
from equal Counter types.

The proof-only argument classifier reports `LocalValPair` for two local
origins. The paired-plan adapter still only translates its previously
supported origin kinds, so this classification does not authorize TASTy body
serialization or paired output. The strict source test reaches the existing
`TastyPlan(Unsupported { feature: BodyExpression, ... })` check and does not
reach the paired-plan callback.

| Final Pair arguments | Authenticated local positions | Proof classification |
| --- | --- | --- |
| `Pair(first, second)` | `[0, 1]` | `LocalValPair` |
| `Pair(second, first)` | `[1, 0]` | `LocalValPair` |
| `Pair(first, first)` | `[0, 0]` | `LocalValPair` |

The aggregate still requires two distinct immutable declarations in original
Block order, one method-owned scope, direct `new Counter(IntFormal)`
initializers, and the exact selected public `Pair(Counter, Counter)` primary
constructor. A third local, mutable local, helper/conditional/nested
initializer, wrong formal or constructor ABI, stale typed root, or source name
that disagrees with its typed local target fails before a constructor-argument
pair receipt is returned. No TASTy bytes, JVM classes, or published artifacts
are produced by this proof increment.

The proof validation matrix records the first measured layer and does not
continue into later checks after a failure:

| Mutation | First observed rejection |
| --- | --- |
| Third local or helper initializer | Cross-class proof source-shape check |
| Mutable local | Cross-class proof local identity check |
| Swapped seed formal, conditional initializer, or nested seed expression | Cross-class proof initializer shape/identity check |
| Missing declaration initializer, duplicate local name, or use before declaration | Source typer diagnostic |
| Wrong local type or Pair constructor argument ABI | Source typer diagnostic |
| Valid reversed or repeat-first Pair arguments | Ordered `LocalValPair` proof with `[1, 0]` or `[0, 0]` |
| Valid two-local Pair body sent to paired producer | TASTy body plan rejects `BodyExpression`; callback is not reached |

## Strict target inventory and validation

At the issue implementation base (`7ca0538d519b5eeb4addfc3609a99040a5b4c9d9`),
the closed #2095 manifest contains 24 targets and already includes
`jvm_tasty_immutable_reference_local_pair_scala_consumer`. The issue body and
#2103 epic still describe a 23-target inventory that predates this manifest.
This PR adds tests to that existing required target and does not add or remove a
Cargo test target or edit the manifest. The manifest remains the authority for
its current 24-target union; #2103 inventory drift is reported to its owner.

The required-mode Scala tests use
`DOTTY_REQUIRE_SCALA_TASTY_IMMUTABLE_REFERENCE_LOCAL_PAIR_CONSUMER=1`, the
pinned Scala checkout and JDK, approved runtime JAR hashes, and a task-local
`TMPDIR`. No hosted Scala corpus audit is run.

## #2125 Rust TASTy two-local BLOCK producer

Issue #2125 extends only the authenticated PairFactory body to its two original
immutable locals. The producer borrows the same `SourceId`, source and typed
arenas, semantic store, source index, method/formal/local symbols, and shared
method `ScopeId` recorded above. It consumes both ordered initializer receipts
independently of the final Pair argument origins. The two local declarations
remain in source order (`first`, `second`) when the final arguments are reversed
or both target `first`.

The focused Rust TASTy test inspects the emitted address index. Its deterministic
`create` method is at address 29, its `BLOCK` at 54, the result `APPLY` at 56,
and the two `VALDEF`s at 72 and 97. The wire order is result expression first,
then the VALDEFs, while the BLOCK statistics retain source declaration order.
Each VALDEF retains a separate `APPLY(SELECTin(NEW(...)))` Counter initializer.
The test verifies that each seed points directly to its original method
`PARAM`. The result's direct local references target `[72, 97]` for
`Pair(first, second)`, `[97, 72]` for the reversed source, and `[72, 72]` for
`Pair(first, first)`. The repeated-first case still emits VALDEF 97 and its
initializer; reusing the first local does not erase the separately
authenticated second declaration.

The pickler rejects typed-only mutations of the second final Pair argument and
the second Counter initializer while the original source roots remain fixed.
The inherited one-local and #2077 paired-local tests still pass. The existing
`jvm_tasty_factory_result_arguments` strict Scala shard now reads the emitted
two-local sidecar with pinned Scala 3.9 `-decompile`, `-print-tasty`, and an
isolated `-from-tasty` invocation. This establishes TASTy reader compatibility;
it does not claim a Scala runtime consumer for linked Rust classfiles.

## #2126 linked six-artifact paired batch

Issue #2126 integrates the two authenticated initializer receipts, ordered Pair
constructor argument proof, and existing TASTy writer through the existing
paired JVM/TASTy plan and batch publisher. The plan borrows the same canonical
session identity and projects both `TypedLocalBinding`s in original Block
order; it does not reconstruct locals from names. The shared JVM local allocator
assigns slots 3 and 4 after `this` and the two `Int` formals. Both the source
and typed roots are retained through contract validation, and the existing
artifact validator checks the six class/sidecar outputs and all three classfile
`TASTY` UUID links before publication.

The focused Rust regression produces actual batches for `Pair(first, second)`,
`Pair(second, first)`, and `Pair(first, first)`. Each case runs in two fresh
sessions and compares paths, bytes, and UUID links across sessions. The batch
contains exactly the six expected class/sidecar paths; classfiles decode as
major version 65, sidecars as TASTy 28.9.0, and the emitted PairFactory body
uses `(II)Lexample/Pair;`, `max_stack=4`, `max_locals=5`. The local declaration
sequence remains first then second for all variants, while final reference
loads follow the requested argument order. Under the required Scala 3.9 mode,
each produced PairFactory sidecar is separately accepted by `-decompile`,
`-print-tasty`, and `-from-tasty`.

The batch-level mutation regression rejects changes to the second seed load,
second local store slot, second Pair argument load, Counter allocation owner,
Pair constructor owner/reference, and stack/local bounds. The existing publisher
fault matrix now also runs against the six-file two-local batch and verifies
that staged write, promotion, and unsafe-path failures do not expose a partial
batch or alter sentinel destinations. The pair remains subject to the existing
transactional publisher and is not exposed through a new public publisher API.

Required validation completed on Rust 1.98.1 and Temurin 21.0.13+9 with a
task-local Cargo target directory and `TMPDIR`: the full workspace test suite,
strict JDK backend gate, pinned Scala 3.9 sidecar checks, strict TASTy target
manifest validator (24 targets), frontend boundary guard, formatting, Clippy,
and rustdoc warnings gate. The strict manifest is unchanged. Hosted strict
shard aggregation remains a CI workflow result and is not represented as a
local run.

### Rust producer artifact identity

The focused paired-batch producer regression emitted these six Rust artifacts
from the frozen `TwoLocals.scala` source (SHA-256
`e2411637c892feb474d6d70526d471b524cd7de49a3db2565923cd69736f73c3`) at
source/test revision `bfeaf9a72119131a6b472cd99493a8f168fbae1d`. This is the
exact committed source/test revision used for the two-thread strict Scala 3.9
producer run that captured these values.

| Rust-produced path | SHA-256 |
| --- | --- |
| `example/Counter.class` | `518831fb3bc939f249037f5a14f0e8c02c182681a38ee8f26314d74414411641` |
| `example/Counter.tasty` | `504092e378b58d111e81169711cc86a4b4967ff47d3673646e96760ec1fc18de` |
| `example/Pair.class` | `4e895557ebb06761402668cda14f93479cdbbc45199b3af4a67cbbe65a1c215b` |
| `example/Pair.tasty` | `f7d29974104da060bc1a598d0b3043df6f43f136e72cb095f672548cdc5b4f13` |
| `example/PairFactory.class` | `b9954a37aafc6a84a391c82b001865fa5b688b56cab4245f801d0e7dba351ffa` |
| `example/PairFactory.tasty` | `a1812f47bad089c47ec125a887ddc7403f78124b4de396d87e262ccfd755e495` |

The three decoded classfile `TASTY` links equal the UUIDs in their corresponding
Rust-produced sidecars:

| Classfile | Sidecar UUID |
| --- | --- |
| `example/Counter.class` | `009ce4f47b6e18ac0000000000008193` |
| `example/Pair.class` | `007a1d11cde040f00000000000008198` |
| `example/PairFactory.class` | `002d5a1b50b7d0c8000000000000819a` |

The artifacts were emitted by Rust 1.98.1 and checked with Scala 3.9.0 at
`777528f19a58e794c9954a42f433373472ec57f8` and Temurin 21.0.13+9. These are
producer observations; they are not the Scala oracle hashes recorded earlier
in this document.

## #2127 Rust JVM runtime identity gate

The #2127 gate extends the existing JDK runtime integration target. It emits
three independent six-artifact batches through the genuine Rust source
frontend and paired JVM/TASTy producer: the primary `new Pair(first, second)`
source, the reversed `new Pair(second, first)` source, and the deliberate
`new Pair(first, first)` alias source. Each batch has its own `SourceId` and
canonical emission session. The Java 21 harness is compiled separately with
`javac --release 21`; runtime classpaths contain the original Rust `.class`
files and that harness only. No Scala-produced classes or sidecars are used by
the runtime client. The run used Temurin 21.0.13+9, Java class-file major 65,
and `-Xverify:all -ea`.

For primary `PairFactory.create(II)Lexample/Pair;`, Rust decoding and
`javap -v -p -c` agree on `max_stack=4`, `max_locals=5`, formal slots 1 and 2,
and this measured instruction flow (PCs are bytecode offsets):

| PC | Instruction | Constant-pool reference / local effect |
| ---: | --- | --- |
| 0 | `new Counter` | Class #12 |
| 3 | `dup` | |
| 4 | `iload_1` | first seed formal, slot 1 |
| 5 | `invokespecial Counter.<init>(I)V` | Methodref #15 |
| 8 | `astore_3` | initialized first Counter, slot 3 |
| 9 | `new Counter` | Class #12 |
| 12 | `dup` | |
| 13 | `iload_2` | second seed formal, slot 2 |
| 14 | `invokespecial Counter.<init>(I)V` | Methodref #15 |
| 17 | `astore 4` | initialized second Counter, slot 4 |
| 19 | `new Pair` | Class #17 |
| 22 | `dup` | |
| 23 | `aload_3` | first Pair argument, slot 3 |
| 24 | `aload 4` | second Pair argument, slot 4 |
| 26 | `invokespecial Pair.<init>(Counter,Counter)V` | Methodref #20 |
| 29 | `areturn` | |

The CFG analyzer accepts the original method and confirms both initialized
reference locals are available at their Pair loads. The reversed source retains
both initializer evaluations and stores but loads slots 4 then 3 at PCs 23 and
25. The alias source still initializes/stores the second Counter in slot 4,
then loads slot 3 twice at PCs 23 and 24. `javap` confirms the explicit-index
slot-4 instructions and their PCs for all variants.

The independent Java harness observes primary results `7/3`, `3/7`, and
`-9/42`, and makes an actual `create(7, 7)` call. Reflection confirms those
equal-seed Pair fields refer to distinct Counters; after mutating the left
Counter, the right remains `7`, and after mutating the right, the left remains
`100`. Repeated calls allocate fresh objects. It observes reversed field
ownership for the reversed source. For the alias source it observes
`left == right`, sees a mutation through either reference in both getter
results, and confirms repeated calls allocate a fresh aliased object pair.

The six original Rust artifacts remain unchanged before and after harness
compilation, disassembly, runtime, and isolated negative copies. Primary SHA-256
values are:

| Artifact | SHA-256 |
| --- | --- |
| `Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` |
| `Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` |
| `Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` |
| `Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` |
| `PairFactory.class` | `16f734ec97d054a2ff1ec40400b6583b9d60476babcf0ba8617fd98dc0feebea` |
| `PairFactory.tasty` | `9415f52a7476d72554ba8943b9b657d588385fc339b3ff639a3c7d51e3d685e4` |

The reversed and alias sessions retain the same Counter and Pair artifact
hashes. Reversed `PairFactory.class` / `.tasty` hashes are
`e04948fc761273bd68ea698e20ed9af574bc2f4f10f8361c4a6b089cc5f72dde` /
`5eb6397aa8561be0e710c8af2087b7eb4561d002da89df540446cc4cd1dc1377`; alias
hashes are
`3064553d2e44ec26c265490b6b0ffad08f46ae929634f1c60ddfa999e081ccc2` /
`7a5332c3b2ee991624a4219a7347f3e928357c29f2f44d50ad027f6feae8a879`.

Isolated primary-batch controls observed their first failure at the actual
layer. The source-authenticated Rust paired contract rejects the swapped local
loads in its `cross-class invocation` phase. A balanced extra Counter
allocation and constructor is inserted at PC 19, before `Pair NEW` with an
empty operand stack. Its peak stack depth is 3, within the unchanged
`max_stack=4`. The isolated clone independently passes `java -Xverify:all`,
executes the harness successfully, and prints the same result as the original.
The paired contract rejects it in `cross-class invocation` because the extra
constructor seed `iload_1` (opcode `0x1b`) appears where the validated first
Pair argument `aload_3` is expected. An isolated JVM-valid swapped clone and
same-local-twice clone reach the Java identity assertion.
Wrong or missing slot-4 stores, missing second allocation or constructor calls,
load-before-store, storing an uninitialized Counter that is later passed to
`Pair.<init>`, wrong Pair constructor owner or descriptor, and `max_stack=3` or
`max_locals=4` fail with JVM `VerifyError`. The uninitialized escape reports
`VerifyError: Bad type on operand stack` at the Pair construction path.
Renaming Pair's `leftValue` method in a classfile clone yields
`NoSuchMethodError`; omitting Counter, Pair, or PairFactory yields
`NoClassDefFoundError`. Each case runs from an isolated class directory and
records its observed first failure layer; the six original hashes are
rechecked. Existing one-local strict runtime tests remain inherited regressions.

The strict two-local runtime gate checks that `java -version` reports Temurin
21.0.13+9 and that `java`, `javac`, and `javap` all report 21.0.13. Required
mode rejects a same-version `OtherVendor-21.0.13+9` runtime; optional mode
skips it. The observed local receipt was `java: OpenJDK Runtime Environment
Temurin-21.0.13+9 (build 21.0.13+9-LTS)`, `javac 21.0.13`, and `javap 21.0.13`.

The strict two-local runtime test, the full `jvm_tasty_mixed_origin_runtime`
target, mandatory `DOTTY_CLASSFILE_REQUIRE_JDK=1` gate, and complete
`cargo test --workspace --all-targets --locked` run were performed on code/test
commit `883bb2867405f2acd92ba17c153ba1bf591cc4b9`. The workspace run completed with 538 passed and 4 ignored; the ignored
tests require separate optional JDK/classpath fixtures and are outside this
JVM-only issue gate. `cargo fmt --all -- --check`, the frontend boundary guard,
workspace Clippy with warnings denied, workspace rustdoc with warnings denied,
and `git diff --check` also passed. No Scala source-consumer or TASTy
interoperability claim is made here.

### Review-fix evidence

The final reviewed test tree at
`882c80ee6835854bd88c93efa791aea49e89d9cb` ran the five-test
`jvm_tasty_mixed_origin_runtime` target on Temurin `21.0.13+9`. The strict
JDK guard recorded `Temurin-21.0.13+9 (build 21.0.13+9-LTS)`, `javac 21.0.13`,
and `javap 21.0.13`; isolated required-mode controls reject both a same-patch
other vendor and Temurin `+10`. Equal-seed `create(7, 7)` was executed and the
reflected Counters were mutated independently.

The extra balanced Counter allocation and constructor are inserted at PC 19,
before `Pair NEW` with an empty operand stack. The clone's peak stack depth is
3 within `max_stack=4`; it independently passes `java -Xverify:all`, executes
the harness, and prints the same result as the original. The authenticated
paired contract rejects it in phase `cross-class invocation` with
`expected aload_3 in bounded call bytecode, found opcode 0x1b`, identifying the
extra constructor seed load where the validated Pair argument load was
expected. Load-before-store, uninitialized Counter escape into `Pair.<init>`,
and wrong Pair constructor descriptor controls fail with observed JVM
`VerifyError`.

At this test SHA, the mandatory classfile/JVM/source JDK gate and
`cargo test --workspace --all-targets --locked -- --test-threads=2` passed
with `DOTTY_CLASSFILE_REQUIRE_JDK=1`. Formatting, frontend-boundary, Clippy,
rustdoc, and `git diff --check` also passed. The evidence update followed
these checks.

## #2129 integrated evidence gate

The final evidence-only increment starts from merged main
`ee90e7f9c013c85623ee1fe4ea7dbd4b8ac27d2a`, which includes the initializer
formal-binding correction from #2157. The prerequisite PR receipts list the
merged PR head followed by its merge commit. Their independent reviews and
required hosted checks are recorded in those PR histories.

| Prerequisite | PR | Reviewed head | Merge commit |
| --- | --- | --- | --- |
| #2122 | [#2130](https://github.com/scytrowski/dotty-rs/pull/2130) | `3d2cdaea24e2f01f6f339c23c2b5754aa72a2eee` | `29d0bbe0788171dcc723c88ca9f61ea680302a6a` |
| #2123 | [#2140](https://github.com/scytrowski/dotty-rs/pull/2140) | `646eca9231ebc216182e8a3cb72c30ff89fec056` | `72d80d1fd37dca11fc98fd950124750a917ffcff` |
| #2124 | [#2144](https://github.com/scytrowski/dotty-rs/pull/2144) | `3d463c1dde0f50756c10124858bd17d110accbee` | `0fc33e567ac2cb9d35021361707edc4d617a3994` |
| #2125 | [#2146](https://github.com/scytrowski/dotty-rs/pull/2146) | `6c86588e9c55b3ead50f55e30b792480cf60f0d3` | `8ef927277ed7693e84fe8d1864b77df9a4797ecd` |
| #2126 | [#2148](https://github.com/scytrowski/dotty-rs/pull/2148) | `274838246fe4e3c0b2fdbd447127fecb429348c7` | `83bb5e41526b84291c829c6afd9632629e74c790` |
| #2127 | [#2149](https://github.com/scytrowski/dotty-rs/pull/2149) | `9555cf60d95582499a3e5bfbd32d0a11cb9cbd43` | `ce11c0b801e04dc8a7b75ad2484604410fa287f4` |
| #2128 | [#2152](https://github.com/scytrowski/dotty-rs/pull/2152) | `8b473151841cb11fe61f360fb6cf43e249769d6c` | `9d34564b05dd36dd3a5c5eb9603e3641292857f9` |
| Declaration-order correction | [#2155](https://github.com/scytrowski/dotty-rs/pull/2155) | `0340713c589d28baf4545691f7d8e91260990b9b` | `bc5650551c2362a29310bf6292c8d77cf7ad32f0` |
| Formal-identity correction in baseline | [#2157](https://github.com/scytrowski/dotty-rs/pull/2157) | `cf9009b7a1a2b9f4db81da0010f27c18fb746378` | `ee90e7f9c013c85623ee1fe4ea7dbd4b8ac27d2a` |

The reversed-declaration variation swaps both complete local declaration
lines while retaining `second = new Counter(secondSeed)`,
`first = new Counter(firstSeed)`, and final `new Pair(first, second)`. This is
separate from class-declaration reordering and reversed Pair argument order.
The paired producer tests verify source-ordered local receipts, each local's
actual formal identity, TASTy local declarations and direct Pair references,
all six paths and UUID links, and byte-identical output across fresh sessions.
The Scala consumer suite compiles and runs the independent client against this
Rust-produced batch and inspects its TASTy. The strict JDK runtime suite also
includes this source variant with the existing Java `-Xverify:all -ea`
identity harness.

The current strict manifest contains 25 targets and registers both
`jvm_tasty_immutable_reference_local_pair_scala_consumer` and
`jvm_tasty_two_local_counter_set_scala_consumer` once. Existing workflow wiring
runs the required Scala 3.9 consumer/oracle gates and JDK 21 runtime gate; the
four-shard count and aggregate failure behavior are unchanged. The Scala
source revision, runtime JAR hashes, Rust pin, TASTy version, and class-file
target remain as recorded above.

The original source SHA-256 is
`e2411637c892feb474d6d70526d471b524cd7de49a3db2565923cd69736f73c3`; the
reversed-declaration source generated by swapping the two complete lines is
`3f95b763e6fd5b05c5e21951e4428e3c7b00dd508078caf547fa50341fc4a0e1`. The
independent Scala client `Consumer.scala` is
`d0ecf8a1183b95508da88f75a06175c2b01dacc575c9b44498d00b030572d00c`. The Java
identity harness source is the existing
`tests/fixtures/jvm_tasty_immutable_reference_local_pair/PairFactoryTwoLocalIdentityHarness.java`;
its SHA-256 is
`38310c03464a835034b628c500324f7c3676c5bea4eef43965660b40cc6f8f69`.

At the current local producer test run, the reversed-declaration six-file
batch has the following observations. Counter and Pair artifacts are identical
to the original batch; only PairFactory changes. The classfile UUID link is
shown beside its sidecar.

| Artifact | SHA-256 / UUID |
| --- | --- |
| `example/Counter.class` | `518831fb3bc939f249037f5a14f0e8c02c182681a38ee8f26314d74414411641` |
| `example/Counter.tasty` | `504092e378b58d111e81169711cc86a4b4967ff47d3673646e96760ec1fc18de` |
| `example/Pair.class` | `4e895557ebb06761402668cda14f93479cdbbc45199b3af4a67cbbe65a1c215b` |
| `example/Pair.tasty` | `f7d29974104da060bc1a598d0b3043df6f43f136e72cb095f672548cdc5b4f13` |
| `example/PairFactory.class` | `6eb506efce8f0ef03a28f51114822c89bc7e377bf9745a82155b304ca5a41d8f`; TASTy UUID `0086a9094699abc1000000000000819a` |
| `example/PairFactory.tasty` | `23d218100c13d0291f00a5b91ae614353ec3acfc9f3554d520286ccb13c9d1e6` |

This producer hash receipt comes from the focused Rust test on the PR tree;
the same test compares a second fresh session byte for byte. Java runtime,
Scala 3.9 consumer/reader, publisher fault matrix and full repository checks
remain subject to the required integrated CI run.

At implementation start, this host did not provide `java`, `javac`, or
`javap`. The local run therefore does not claim Java/Scala runtime or strict
oracle success for this increment. Those required gates must pass in hosted CI;
the exact integrated artifact hashes, UUID receipts, failure matrix and CI run
links will be added after the required run completes. Local focused producer
tests are run on Rust 1.98.1 with the registered task scratch directory.

## Final integrated AC1–AC10 release ledger

This ledger distinguishes evidence measured by this final PR from receipts
inherited from prerequisites. The frozen integrated base is
`ee90e7f9c013c85623ee1fe4ea7dbd4b8ac27d2a`; it contains the seven prerequisite
merges listed above. The implementation and local producer/negative tests were
run at PR head `75ddb1c4e655377f5200218b0e448005890a4d8a`. The CI runs at that
head are [Rust/JVM CI run 42176124273](https://github.com/scytrowski/dotty-rs/actions/runs/42176124273)
and [strict Scala 3.9 run 42176124274](https://github.com/scytrowski/dotty-rs/actions/runs/42176124274).
Both jobs in Rust/JVM CI run 42176124273 are green, including 1,014 workspace
tests (one ignored) and the mandatory JDK 21 job. The four shards in strict
Scala 3.9 run 42176124274 are still running. These hosted checks are current
integrated receipts, not inherited prerequisite results. This document-only
correction will itself trigger fresh PR checks, which must also finish green.

| AC | Integrated observation and evidence | Result at this ledger revision |
| --- | --- | --- |
| **1. Baseline and provenance** | Base `ee90e7f9c013c85623ee1fe4ea7dbd4b8ac27d2a`; prerequisite reviewed heads and merge SHAs are in the preceding table. Original `TwoLocals.scala`: `e2411637c892feb474d6d70526d471b524cd7de49a3db2565923cd69736f73c3`; reversed-declaration source: `3f95b763e6fd5b05c5e21951e4428e3c7b00dd508078caf547fa50341fc4a0e1`; Scala `Consumer.scala`: `d0ecf8a1183b95508da88f75a06175c2b01dacc575c9b44498d00b030572d00c`; Java identity harness: `38310c03464a835034b628c500324f7c3676c5bea4eef43965660b40cc6f8f69`. Rust is 1.98.1 (`48a229cea`), Scala is 3.9.0 at `777528f19a58e794c9954a42f433373472ec57f8`, TASTy 28.9.0, Temurin 21.0.13+9 with `--release 21` / class-file 65. The pinned checkout launcher is `dist/target/universal/stage/bin/scalac` (observed SHA-256 `2981fa689c25ac031e1d923387c07ca65506ed7d8d88454e5d4dcd8e11726120`); the staged compiler JAR is recorded as an observation only, not a reproducible pin. Published runtime JAR hashes: Scala 3 library `9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba`, Scala library `b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd`; full oracle provenance is in `oracle.json`. | Both Rust/JVM run 42176124273 and strict Scala run 42176124274 passed at the recorded test head. |
| **2. One source/typed session** | `two_immutable_counter_locals_pickle_in_source_order_for_local_pair_arguments` exercises the whole source unit in one session and asserts both original initializer/formal relationships and final Pair local identities. It passes on the recorded PR tree. The assertion is identity-based and does not treat spelling or arena slot numbers as proof. | Focused test passed locally and was included in green Rust CI run 42176124273. |
| **3. Authentic TASTy graph** | The producer test checks two immutable `Counter` `VALDEF`s, separate constructor applications and seed `PARAM` references, and final direct `TERMREF`s to the corresponding local declarations. The Rust graph regression reports `BLOCK=54`, declaration addresses `[72, 97]`, and Pair references `[72, 97]` for the original and `[97, 72]` when declarations are reversed but Pair use remains `Pair(first, second)`. The official Scala 3.9 oracle observes method 32, block 57, final apply 59, local VALDEF addresses `[75,98]`, direct Pair targets `[75,98]`; `-decompile`, `-print-tasty`, and `-from-tasty` ran on Rust sidecars. | Focused producer and Scala 3.9 shard passed in run 42176124274. |
| **4. Six-file batches** | Focused deterministic producer coverage emits exactly `Counter.class/.tasty`, `Pair.class/.tasty`, and `PairFactory.class/.tasty` in two fresh sessions. Original-source SHA-256 values are listed below. The complete declaration-reversal batch retains Counter and Pair artifacts and changes PairFactory; its six hashes and UUID are recorded above. The source-order and final-argument controls are tested separately. | Focused six-artifact test passed locally. |
| **5. JVM bytecode/proof** | The strict runtime test decodes `create(II)Lexample/Pair;`, checks `max_stack=4`, `max_locals=5`, two Counter allocations/constructors and distinct stores. Original Pair use loads slots `[3,4]`; declaration reversal with unchanged `Pair(first, second)` loads `[4,3]`; final-argument reversal also loads `[4,3]`; repeat-first loads `[3,3]`. The new runtime matrix asserts these per variant. | The required hosted Classfile JVM encoder gate passed on this tested head with `DOTTY_CLASSFILE_REQUIRE_JDK=1` and includes `jvm_tasty_mixed_origin_runtime`; see [job 127754877388](https://github.com/scytrowski/dotty-rs/actions/runs/42176124273/job/127754877388). |
| **6. Strict Java 21 runtime** | Temurin 21.0.13+9 ran the Java harness with `-Xverify:all -ea`. The original, reversed-final-argument and reversed-declaration variants each reported `identity:true;mutations=isolated;repeat=fresh;seeds=7/3,3/7,-9/42;equalSeeds=distinct-isolated`; the alias control reported the same seed/mutation/fresh-call result with `equalSeeds=n/a`. The reversed-declaration disassembly retained two allocations and stores to slots 3/4, then loaded `[4,3]` for source Pair(first, second). All six runtime artifacts remained hash-identical before and after checks. | Passed in JVM job 127754877388 at tested head `75ddb1c4e655377f5200218b0e448005890a4d8a`; run output records the runtime strings and actual flow. |
| **7. Strict Scala 3.9 consumer** | The independent client is `tests/fixtures/jvm_tasty_two_immutable_reference_locals/Consumer.scala`; it compiles against the Rust six-file batch and the two approved runtime JARs. It reports primary `7/3`, reversed arguments `3/7`, and alias `left-equals-right`; independent mutation and fresh allocations pass. Declaration reversal is separately compiled and run with the unchanged `Pair(first, second)` client, and the six original outputs stay unchanged after each run. TASTy-only lookup succeeds for Counter constructor/get/set, Pair fields/constructor, and factory method; matching classfile-only controls fail at Scala `[E008]`/`[E173]` lookup. | All four required shards and aggregate check passed in run 42176124274. |
| **8. Negative and publisher first-failure matrix** | The JVM matrix rejected swapped Pair loads at the paired contract (`cross-class invocation`, expected `aload_3`, found `0x19`); a JVM-valid swapped clone failed the identity assertion (`left value: expected 7 got 3`); same-local-twice failed distinct-local identity; invalid local slots/stores, missing allocations/constructors, use-before-store, uninitialized escape, wrong constructor references/descriptors and low stack/local limits failed at JVM verification. Missing classes produced `NoClassDefFoundError`; missing `leftValue` produced `NoSuchMethodError`. Scala-reader controls observed missing/misplaced sidecars, corruption and UUID mismatch at compiler loading; dangling local TERMREFs failed `-decompile` with `TreeWithoutOwner`. Wrong local types/initializer constructor were accepted by Scala's reader but rejected against the Rust source-bound TASTy plan at `TASTy plan identity`; wrong signed constructor-name mutation was likewise recorded as accepted by Scala, not claimed as a Scala rejection. The paired publisher matrix injects failures across six staged writes and promotion and checks partial-output/sentinel containment. | JVM negative matrix and publisher test passed in run 42176124273; Scala-reader negative matrix passed in run 42176124274, both at tested head `75ddb1c4e655377f5200218b0e448005890a4d8a`. |
| **9. Strict CI and repository gates** | Current strict manifest validates 25 targets / four shards and includes the immutable-local-pair and two-local Counter.set consumer targets once each. The required shard flags include `DOTTY_REQUIRE_SCALA_TASTY_TWO_IMMUTABLE_REFERENCE_LOCALS_CONSUMER=1` and `DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1`; the JVM workflow sets `DOTTY_CLASSFILE_REQUIRE_JDK=1`. Local fmt, frontend-boundary, manifest, Clippy (`-D warnings`), rustdoc (`-D warnings`) and diff checks pass; the full workspace suite was not run locally; Java/Scala tools are absent locally. | Rust/JVM CI and all strict Scala 3.9 shards passed on PR head `75ddb1c4e655377f5200218b0e448005890a4d8a`; both workflows must run again and pass at the final documentation-correction head. |
| **10. Final ledger and boundary** | This section records each AC, tested source/client pins, artifacts, UUIDs, runtime and first-failure evidence with explicit provenance. Supported scope is the two original immutable `Counter` locals, their authenticated initializers, ordered two-reference Pair construction, six-file Rust output and the pinned Java/Scala verification gate. Third/mutable locals, helper/conditional/nested initializers, broader constructor shapes, cross-unit ABI, public compiler API/CLI and unrelated compiler features remain deferred. A newly exposed production defect goes to its owning prerequisite. | Evidence document updated; required final-head CI receipts remain outstanding. |

Original-source artifacts emitted by the focused Rust producer on the recorded
PR tree (SHA-256). The artifact test additionally decodes class-file major 65,
TASTy 28.9 and each classfile-to-sidecar UUID link.

| Original path | SHA-256 |
| --- | --- |
| `example/Counter.class` | `518831fb3bc939f249037f5a14f0e8c02c182681a38ee8f26314d74414411641` |
| `example/Counter.tasty` | `504092e378b58d111e81169711cc86a4b4967ff47d3673646e96760ec1fc18de` |
| `example/Pair.class` | `4e895557ebb06761402668cda14f93479cdbbc45199b3af4a67cbbe65a1c215b` |
| `example/Pair.tasty` | `f7d29974104da060bc1a598d0b3043df6f43f136e72cb095f672548cdc5b4f13` |
| `example/PairFactory.class` | `b9954a37aafc6a84a391c82b001865fa5b688b56cab4245f801d0e7dba351ffa` |
| `example/PairFactory.tasty` | `a1812f47bad089c47ec125a887ddc7403f78124b4de396d87e262ccfd755e495` |

| Classfile | Sidecar UUID |
| --- | --- |
| `example/Counter.class` | `009ce4f47b6e18ac0000000000008193` |
| `example/Pair.class` | `007a1d11cde040f00000000000008198` |
| `example/PairFactory.class` | `002d5a1b50b7d0c8000000000000819a` |

For declaration reversal, the unchanged Counter and Pair hashes/UUIDs are
above. `PairFactory.class` is
`6eb506efce8f0ef03a28f51114822c89bc7e377bf9745a82155b304ca5a41d8f`, its
sidecar is `23d218100c13d0291f00a5b91ae614353ec3acfc9f3554d520286ccb13c9d1e6`,
and its UUID is `0086a9094699abc1000000000000819a`. The final-argument-reversed
variant has PairFactory hashes
`dddd308354ea7d7948ff85a8a54e491e475db9c59b8bc1974bdb4eec01c70309` /
`aee575b295a7f5e86c5afb0ab29da575f62230eab4cd415f17cb90987edb1bed` and UUID
`00045a1b509ed0c8000000000000819a`. The existing repeat-first variant has
UUID `00045a1b50b7d0c8000000000000819a`. These variant receipts are from
`two_local_pair_emits_deterministic_six_artifact_batches_for_all_argument_orders`
at PR head `75ddb1c4e655377f5200218b0e448005890a4d8a`; the test compares fresh
session outputs byte for byte.

### Strict Scala-consumer runtime classpaths

The strict Scala workflow also emitted the classpaths actually consumed by its
independently compiled client. These use the runtime test's source IDs, so their
PairFactory hashes/UUIDs are distinct from the paired-batch producer receipts
above. The first set is the original source (`source_id=21280`); the second is
the complete local-declaration reversal with final `Pair(first, second)` held
fixed (`source_id=21300`). The workflow recorded all six files unchanged after
Scala compile/runtime for each case.

| Artifact | Original source, source ID 21280 SHA-256 | Reversed declarations, source ID 21300 SHA-256 |
| --- | --- | --- |
| `example/Counter.class` | `fe3d5bc2169b6e02e28fec36271518d476e1b3aea7ac65077a8d382958f7ef6b` | same |
| `example/Counter.tasty` | `3d6289dd35b38698347de93f679605547ac61ca1c87842b8dca96ca7a8293045` | same |
| `example/Pair.class` | `77257daed358c452f0291b26bcc7d4a2778483b47df0b1861948b13e43b8e317` | same |
| `example/Pair.tasty` | `4a7dcc31a6f771af5685afa6464bad85acdc869e7fec2feb9b3d41ee98dfd284` | same |
| `example/PairFactory.class` | `16f734ec97d054a2ff1ec40400b6583b9d60476babcf0ba8617fd98dc0feebea` | `c4730db60612fa1968533389c9afe1a94720ffd344366bc1805e677aaa4767c7` |
| `example/PairFactory.tasty` | `9415f52a7476d72554ba8943b9b657d588385fc339b3ff639a3c7d51e3d685e4` | `199dab4895187fed40e1384050fcf111c80bfac9bf50241c4058967dc5bb9387` |

| Classfile | Original source UUID | Reversed-declaration UUID |
| --- | --- | --- |
| `example/Counter.class` | `008071c3c00968ac0000000000008193` | same |
| `example/Pair.class` | `000b7d8d7441c7f00000000000008198` | same |
| `example/PairFactory.class` | `000a7216f3d105c8000000000000819a` | `00316d11887affc1000000000000819a` |

These strict-consumer receipts are from [Scala shard 2/4, job
127754877279](https://github.com/scytrowski/dotty-rs/actions/runs/42176124274/job/127754877279)
at tested PR head `75ddb1c4e655377f5200218b0e448005890a4d8a`. The companion
strict shards and aggregate required check also passed in [run
42176124274](https://github.com/scytrowski/dotty-rs/actions/runs/42176124274).
