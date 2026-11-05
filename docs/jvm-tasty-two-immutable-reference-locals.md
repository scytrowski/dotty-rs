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
