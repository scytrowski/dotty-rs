# JVM reference CFG contract (Java 21)

This document records the verifier contract and enabled source subset for
reference-bearing control-flow graphs. The source of truth is the Java SE 21
JVM Specification (JVMS 21), not a later release.

## JVMS 21 rules used

For class-file version 51 and later, verification uses type checking and
`StackMapTable` frames; Java 21 emits class-file major version 65. A stack-map
frame describes locals and operand-stack entries at the start of a basic block,
and the initial frame comes from the method descriptor. See
[JVMS 21 §4.10.1](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.10.1),
[§4.7.4](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.7.4),
and [§4.10.1.6](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.10.1.6).

The verification type `Object(C)` names a `CONSTANT_Class_info` entry. An
uninitialized allocation is `Uninitialized(offset)`, where `offset` is the
bytecode PC of the `new` instruction; constructor `this` is the distinct
`UninitializedThis` type. `invokespecial C.<init>` for a value produced by
`new C` must name that same class, and normal completion rewrites every alias
of that allocation identity to initialized `C`. See
[JVMS 21 §4.10.1.2](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.10.1.2),
[§4.10.1.4](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.10.1.4),
and [§4.10.1.9](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.10.1.9).

JVMS permits assignment-compatible subtyping in several operations. The
class-file analyzers accept subtype assignments only when the caller supplies
an explicit, validated `ClassHierarchy`; their default remains exact and
never guesses an ancestor, interface, `java/lang/Object`, or common
superclass. The Scala adapter supplies only edges that its emission-unit
validator proved. This bounded proof enables method arguments, reference field
writes, constructor arguments, returns, and initialized reference joins on the
operand stack. A join can use the nearest shared non-`Object` superclass only
when both bounded paths are explicitly present in the graph; this is a checked
proof, not classpath inference or an `Object` fallback.

## Analysis boundaries

`ValueKind::Reference` in the context-free `LinearCodeAnalyzer` records only a
one-slot category. The class-aware paths retain initialized class identities
for supported descriptors; with no hierarchy they require exact identity,
and with an explicit hierarchy they accept only assignments proven by its
edges. The CFG path is required whenever control flow or allocation-site
initialization matters; a category-only result is not evidence of verifier
safety.

`CfgCodeAnalyzer` tracks `Top`, integer, initialized class references,
allocation-site-specific uninitialized references, and a bounded explicit
class hierarchy for assignability checks and reference joins. Stack values and
initialized object locals may widen to the nearest shared, checked non-`Object`
superclass. A local with an incoming `Top`, incompatible category, receiver,
or unsupported state remains unusable. Ordinary
instance methods seed local 0 with the exact emitting class,
while branched constructors remain unsupported. The
`StackMapFrameGenerator` maps every initialized class through a validated
`CONSTANT_Class` index and emits `Object(index)` entries.

For class-aware method analysis, ordinary `invokespecial` is separate from
constructor invocation. It requires an explicit current-class to direct-parent
edge, an exact `Methodref` owner for that parent, an `ACC_SUPER` emitting
class in `ClassFileBuilder`, and an initialized receiver whose exact class is
the emitting class. It consumes the descriptor arguments and pushes only the
supported declared result. The direct-parent method call never rewrites
uninitialized allocation aliases.

The constructor path in `ClassFileBuilder` rejects branched `<init>` bodies
because it does not track the transition from `uninitializedThis` to
initialized `this`. Class-file analysis supports exact object parameters and
results by default, and explicitly proven subtype assignments when a
`ClassHierarchy` is supplied. Source bytecode lowering supports planned
reference parameters and results, initialized mutable reference locals
reassigned in `If` branches and `While` bodies, and ordinary planned calls.
Mutable-local source initialization is intersected across reachable `If` arms
and restored to the pre-loop set after a `While`; the classfile analyzer
independently joins compatible local classes to their checked nearest
non-`Object` superclass. Source lowering still rejects uninitialized or
maybe-initialized reads, writes in short-circuit operands or repeatedly
evaluated `While` conditions, sibling result joins without an exact checked
expected parent, arrays, null, and branched constructors.

## Bounded state model

Keep locals and operand stack as separate structures. The abstract value model
distinguishes:

| Abstract value | Meaning | Stack-map representation |
| --- | --- | --- |
| `Top` | Unusable local after no assignment or a local join | `Top` |
| `Integer` | JVM int-like primitive | `Integer` |
| `Long` / `LongTail` | One category-two local value occupying a head slot and an inaccessible tail slot | One `Long` entry; the tail has no separate entry |
| `Null` | The JVM null verification type, with no class identity | `Null_variable_info` (tag 5) |
| `Initialized(class)` | Initialized reference with exact validated JVM internal class name | `Object(cp_index)` |
| `Uninitialized(allocation_site)` | Fresh instance at one specific `new` instruction | `Uninitialized(new_pc)`; prohibited at CFG joins in this increment |
| `UninitializedThis` | Constructor receiver before the required `this`/super call | `UninitializedThis`; branched constructors remain deferred |

Class-file analysis must remain Scala-independent. Its identity is a validated
JVM internal name or validated `CONSTANT_Class`, never `SymbolId`. The JVM
adapter may map its canonical source class identity to that internal name
before invoking class-file analysis.

CFG analysis also supports the bounded category-two transfer for Long
constants, descriptor parameters, local loads/stores, returns, branch joins,
and loop backedges. It stores one logical `Long` on the operand stack while
counting two physical stack slots. Locals track `Long` and `LongTail`
separately, invalidate overlapping pairs on stores, and merge an uncertain
pair to `Top`. Frame projection emits one Long verification entry and skips
the tail while preserving genuine Top holes. Class-aware instance Long field
transfer and exact virtual/approved constructor transfer are supported for
descriptor `J`. Long arithmetic, `lcmp`, `dup2`/`pop2`, conversions, static
Long calls, and branched constructors remain unsupported.

For an ordinary non-static, non-constructor method, the initial local 0 is
`Initialized(emitting_class)`. Static methods start with descriptor-derived
parameters, including exact object identities. The Scala source backend still
restricts emitted public method signatures to its existing subset. A constructor starts with
`UninitializedThis`; do not treat it as an ordinary receiver. The validated
branchless constructor path accepts a bounded sequence of supported integer
and Long constants and loads from initialized constructor parameter locals
before one direct-super `<init>` call. It checks the call's owner, descriptor,
argument categories, and exact reference identities before analyzing the
remainder as initialized `this`. Branched constructor flow remains unsupported.

## Join and termination policy

At each reachable block:

1. Operand-stack heights must match. `Integer` joins only with `Integer`.
   `Null` joins `Null` as `Null`, and `Null` with `Initialized(C)` as
   `Initialized(C)`; null contributes no class identity to retained join
   evidence. `Null` cannot join an uninitialized value.
   Equal initialized classes remain unchanged. Different initialized classes
   join at the nearest unique shared non-`Object` superclass proven by the
   explicit, validated single-parent hierarchy. Every incoming class at a
   multiway join must share that checked ancestor, including inputs accumulated
   before widening and inputs arriving on backedges. This join is commutative
   and idempotent; bounded chains converge independently of predecessor order,
   and a widened state requeues dependent blocks. Missing paths, an `Object`-
   only intersection without an actual `Object` input, and interfaces do not
   provide a join proof. An actual `Object` input may join at `Object` only
   when every other input has an explicit checked path to it. Eligibility is
   decided from the complete converged input evidence, independent of
   predecessor processing order.
   Initialized/uninitialized mismatches,
   different allocation sites, primitive/reference mismatches, and different
   stack heights return typed analysis errors. Never infer a shared ancestor
   such as `java/lang/Object` from missing edges.
2. A local slot whose incoming states are identical keeps that state. Two
   initialized object states may widen to the nearest common non-`Object`
   superclass only when every original incoming class has an explicit path in
   the checked hierarchy. Keep those original classes as evidence through
   widening and later loop inputs; do not prove a join from the current
   aggregate alone. Without a unique checked ancestor, the result is `Top`.
3. `Top` is absorbing for locals. Any path with an uninitialized or unusable
   slot therefore prevents that local from becoming readable after the join.
   A later read of `Top` fails as uninitialized/unusable. Stack and local
   references can widen only along the validated, eight-edge-bounded
   superclass chain. The worklist reprocesses a block whenever a local,
   operand-stack value, or retained proof changes and has a defensive work
   limit derived from graph size, state widths, evidence, and hierarchy depth.
   Traverse blocks, successors, and local slots in stable source/index order so
   results do not depend on hash-map order.

These rules are intentionally narrower than JVMS assignment compatibility.
Reference stack and initialized-local joins use only explicit superclass
edges and never infer a classpath relationship. A local is widened only when
all paths carry initialized references and the complete checked evidence
proves their common non-`Object` ancestor. Primitive/reference differences,
receiver/reference differences, missing hierarchy paths, and any incoming
`Top` remain `Top`; no later store on another path can make that slot
definitely initialized. A `Null`/initialized-object local join keeps the
object's exact class, while `Null`/`Top` remains `Top`.

The source-level `Base`-declared sibling case and its separate typer, method
descriptor, and verifier questions are characterized in the [sibling
reference joins audit](jvm-sibling-reference-joins.md). The audit keeps the
current rejection and does not infer a source result from the verifier's
ability to accept an explicit `Object(Base)` frame.

## Transfer and constant-pool checks

Transfer functions check exact class and initialization state instead of
deriving a reference type from `ValueKind` or opcode category alone.

| Instruction family | Transfer/check in the supported subset |
| --- | --- |
| `aload` / `astore` | Load only an initialized exact-class local. Stores preserve the exact initialized class; reject unusable or uninitialized values. |
| `dup` / `pop` | Preserve or discard the same abstract value; `dup` retains its allocation-site identity without initializing it. |
| `getfield` / `putfield` | Validate the fieldref and descriptor. Receiver must be initialized and assignable to the field owner using the explicit hierarchy; a reference value must be assignable to the declared field class. |
| `invokevirtual` | Validate methodref and descriptor. Pop supported arguments right-to-left; each object argument and receiver must be initialized and assignable to its declared class/owner using the explicit hierarchy. This call may execute while a distinct allocation remains pending lower on the stack. Push the exact initialized object result or int-like result. Arrays and category-two types remain unsupported. |
| `new` | Validate the `CONSTANT_Class`; push a fresh instruction-site identity. Keep at most two live sites. A third `new` before an earlier constructor completes fails with `TooManyLiveAllocations`. |
| `invokespecial <init>` | The top active uninitialized receiver's site and class must match the constructor owner; constructor arguments use explicit hierarchy assignability and must be initialized. Initialize in last-in, first-out order and rewrite only aliases of that site to `Initialized(owner)`. |
| `invokespecial DirectParent.method` | Require the `Methodref` owner to equal the validated direct parent and the receiver to be exactly the initialized emitting class. Validate the supported descriptor arguments, then push its exact reference or int-like result. The class-file builder requires `ACC_SUPER`; constructors keep their separate allocation transfer. |
| Conditional branches | `ifeq`/`ifne` pop one integer; `if_icmp*` pop two integers; `if_acmpeq`/`if_acmpne` pop two initialized references or nulls; `ifnull`/`ifnonnull` pop one initialized reference or null. Branches do not push a result. Uninitialized allocations are rejected as ordinary branch operands. |
| Returns | Preserve primitive/void checks; `areturn` requires an initialized reference assignable to the planned result class using the explicit hierarchy. |

Field and method constant-pool operands must be resolved to validated entry
families, nested owner/class names, and parsed descriptors before transfer.
The exact-owner restriction applies after resolving the `CONSTANT_Fieldref` or
`CONSTANT_Methodref`; an index match alone proves nothing about a receiver.
Malformed or absent `CONSTANT_Class` references remain typed constant-pool
errors, not guessed names.

## Allocation and constructor boundary

Allow one or two nested sequences wholly inside one straight-line basic block:

```text
new Outer
dup
new Inner
dup
primitive argument evaluation
invokespecial Inner.<init>(descriptor)
invokespecial Outer.<init>(LInner;)
```

Allocation identity is the `new` instruction position, even when two sites
allocate the same class. Each `invokespecial <init>` consumes the most recently
created still-pending site and must name that site's exact class. Normal
completion rewrites only that site's aliases; the older enclosing site stays
uninitialized until its own matching constructor completes. A third
simultaneously live site is rejected, but a later `new` is allowed after
initialization frees a slot.

Before a matching constructor completes normally, an uninitialized alias may
not be stored in a local, returned, passed as a method argument, used as a
field receiver, used as an ordinary receiver, or cross a branch edge. Calls
on a separate initialized receiver with initialized arguments may run while
an enclosing site remains pending; their initialized results can be consumed
as constructor arguments. No uninitialized value may appear in a merge state
or emitted branch-target frame. Branches and block exits with any pending
site remain rejected; exception-table edges are not modeled by this CFG
contract.

Branched `<init>` methods remain unsupported: the current linear class-file
builder validates the `UninitializedThis` to initialized-`this` transition
only in a single bounded prologue. It does not model exception edges or merge
states across the call, and no branch target may assume initialized `this`
before that transition.

## Constant-pool and frame construction

The class-file builder interns symbolic references before freezing the
constant pool. Its reference-aware path collects every class identity needed
by branch frames and interns them before `finish()`. It resolves each internal
name through that builder's name-to-index result and emits
`VerificationTypeInfo::Object(cp_index)`.
Never predict indices, mutate/reorder a frozen pool, or intern a missing frame
class after finalization.

`Uninitialized(new_pc)` must use the final byte offset from the same assembled
code/layout snapshot used for the corresponding `Code` and `StackMapTable`.
Do not count instruction indices as PCs or recompute offsets independently.
For this first allocation subset, uninitialized values cannot reach frames,
so a serialized `Uninitialized` frame item should not be needed; retaining the
typed representation in the format API is still required for later work.

## Characterization and rejection matrix

`tests/fixtures/jvm-reference-cfg/ReferenceFlow.java` is compiled with
`javac --release 21`; `javap -v` records major version 65, `StackMapTable`
entries including a local frame entry `locals = [ class Counter ]`, the
corresponding `CONSTANT_Class`, reference-bearing loop/local joins,
current-instance and field/call instructions, and object construction in
branches. The source integration test
`planned_reference_operations_verify_across_if_while_and_short_circuit_cfgs`
and `reference_parameters_and_results_compose_with_if_and_while_control_flow`
check generated classfiles and run representative methods under
`java -Xverify:all` on JDK 21+.

The latter exercises `choose(flag, first, second): Counter`, whose exact
reference result is selected from either `if` branch, a reference-returning
call inside one branch, and loops whose `Counter` receiver comes from a
reference parameter. Its frame assertions require `Object(Counter)` on the
conditional operand stack and in the loop-header locals alongside primitive
parameters.

The broader `ReferenceObjectApis.scala` fixture keeps both classes in one
source semantic session and adds `Counter.self`, `CounterService.create`,
`passthrough`, `read`, `choose`, `incrementUntil`, and `roundTrip`. Its
regression checks that each actual branch target has a decoded stack-map frame,
that `Object(Counter)` is resolved through the emitted constant pool at the
branch join and loop header, and that the loop's exact reference parameter
survives its back-edge. The independent-session bytes are compared in class
output order. On JDK 21+, `javap` and the Java identity/loop harness run the
classfiles with `-Xverify:all`; CI requires this toolchain.

`ReferenceHolder.scala` adds the real source path for field-backed reference
access: `Holder.value()` reads `current`, `initial()` reads `original`, and
`read()` chains the returned `Counter` into `Counter.get()`. Its `while` loop
calls both `value()` and `Counter.get`/`increment`; the emitted loop header
retains `Object(Holder)`. The `choose` branch join retains exact
`Object(Counter)` on the operand stack. The test checks branch targets against
decoded frames, compares complete class batches from independent frontend
sessions, and on JDK 21+ checks all three classes with `javap` and
`java -Xverify:all`.

The class-file unit tests exercise exact local/stack transfer, allocation
tracking, owner and argument checks, joins, and frame projection. The JDK
encoder gate checks virtual reference calls, chaining, and exact result frames
under `java -Xverify:all`. The context-free linear analyzer may accept
category-shaped invalid sequences; class-aware linear analysis is used for
straight-line builder methods.

| Case | Required behavior | Evidence or explicit boundary |
| --- | --- | --- |
| `Counter` reference local through a loop header and join | Same `Initialized(Counter)` state at each compatible stack/local join; emit `Object(Counter)` frame entry | Covered by source integration and JDK `-Xverify:all` |
| Reference parameter used as a call receiver through `while` back-edges | Preserve descriptor-seeded `Object(Counter)` in each loop-header local frame; leave no operand values at the header | Focused source integration and JDK `-Xverify:all` |
| Exact reference-valued `if` result | Join only identical initialized classes and return with `areturn` | Source integration checks the `Object(Counter)` stack frame and JDK object identity |
| Reference-returning call in an `if` arm | Preserve call result class through the exact branch stack join | Source integration and class-aware CFG analysis |
| Reference local initialized only in one branch or loop body | Do not mark it definitely initialized after the join or zero-iteration exit; reject a later read | Typed-body `dotty-jvm` regression and class-aware local-state analysis |
| Reference-bearing conditional and current `this` | Seed local 0 as emitting class; preserve exact receiver state at branch target | Covered by field/call branch and loop cases under JDK verifier |
| `getfield`, `putfield`, `invokevirtual` with exact owner | Validate CP operands and require exact receiver owner; virtual calls also check supported exact object arguments and results | Covered by classfile analyzer tests and JDK verifier gate |
| Ancestor-related initialized classes at a stack join | Widen to the already-present ancestor and emit its `Object` frame entry | Classfile analyzer and builder regressions; JDK gate |
| Real `Middle`/`Leaf` source branches returning `Middle` | Keep the canonical source result and encode `Object(Middle)` at both branch-order joins | `MultilevelInheritance.scala`, decoded `StackMapTable`, and mandatory JDK 21+ source gate |
| Exact `Left`/`Right` source branches returning `Base` | Preserve the source `Type::Or`, then encode the checked `Object(Base)` frame at both branch-order joins; do not infer `Object` or insert a cast | `SiblingConstructors.scala`, decoded `StackMapTable` with CP-resolved `Base`, byte-identical fresh frontend sessions, and mandatory JDK 21+ `-Xverify:all` gate |
| Other sibling initialized classes at a stack join | Join only at the nearest checked non-`Object` superclass; reject missing paths and inferred `Object`-only intersections | Classfile analyzer regressions; source lowering also requires the exact expected-parent proof |
| Unrelated initialized classes at a stack join | Typed incompatible-stack reference error; do not infer a common class | Classfile analyzer regression tests |
| Different initialized classes at a local join | Join only at their nearest checked non-`Object` superclass, using complete predecessor evidence | Classfile analyzer and local-frame regressions |
| Initialized `Base` local assigned `Right` in only one `If` arm | Keep the local definitely initialized after the source arm intersection; encode `Object(Base)` at the verifier join | Source integration decodes the local frame and runs both branches with `java -Xverify:all` |
| Initialized `Base` local assigned in a `While` body | Preserve the pre-loop source initialization proof for zero iterations; join preheader and back-edge classes to `Object(Base)` at the loop header | Source integration checks the Base and primitive-index frame entries and runs zero, one, and multiple iterations under `java -Xverify:all` |
| Missing assignment or unproven ancestry at a local join | Merge to unusable `Top`; reject a later read | Classfile analyzer regression tests |
| Wrong field/method/constructor owner | Typed owner mismatch before emission | Classfile analyzer and planned-target regressions |
| Uninitialized local read or uninitialized object escape | Typed uninitialized-use error; never emit the class | Classfile analyzer allocation regressions |
| Branch between `new` and matching constructor | Typed unsupported/uninitialized-on-edge error | Classfile analyzer allocation regressions |
| Wrong constructor owner for a `new` allocation | Typed allocation-owner mismatch | Classfile analyzer allocation regressions |
| Missing, wrong-kind, or malformed CP `Class` entry | Existing typed constant-pool validation error; never manufacture a frame type | Builder/decoder validation remains the current boundary |
| Branched constructor before `this`/super initialization | Keep rejected as unsupported constructor flow | `ClassFileBuilder` rejects branched `<init>` analysis |
| Exact planned reference method parameter/result | Accept; seed parameter frames and require matching initialized `areturn` values | Source integration and mandatory JDK 21 verifier gate |
| Transitive `Leaf -> Base` argument or return | Accept only with the validated two-edge ancestry; preserve the concrete value class and emit no cast | `MultilevelInheritance.scala`, decoded descriptors/references, and JDK 21 `-Xverify:all` harness |
| Canonical source `Null` in planned results, locals, and supported `If` joins | Preserve verifier `Null`; join with `Object(Box)` only as the checked `Object(Box)` frame | `NullReferenceIdentity.scala`, decoded CP-resolved frames, independent-session byte comparison, strict JDK 21 acceptance harness |

The backend converts exact planned reference-CFG cases and explicitly proven
direct-parent assignments from negative to positive coverage. Remaining
negative cases continue to pin owner, definite-initialization,
constructor-safety, hierarchy-join, and descriptor boundaries.

## Ownership for follow-up work

- `dotty-classfile` owns JVM names, allocation sites, validated hierarchy
  assignability and bounded checked superclass joins, dataflow, constant pool
  checks, and frame encoding; it does not depend on Scala symbols.
- `dotty-jvm` maps validated plan identities to classfile names and emits only
  the bounded source forms described above.
- Branched `<init>` and `UninitializedThis` remain a separate constructor-flow
  problem. General least-upper-bound inference beyond the explicitly checked
  single-parent graph, arrays and null contexts outside the bounded default-null
  paths, interface inheritance, exception-flow
  joins, and arbitrary uninitialized values remain out of scope. For source compilation, the adapter proves the
  bounded same-unit `Base -> Object`, `Middle -> Base`, and `Leaf -> Middle`
  chain; a source-typed `If` joins distinct initialized references only when
  they are ancestor-related and both fit its canonical result class.
  The current `null` and reference-identity boundary, including the distinct
  JVMS `Null` verifier value, is audited in
  [jvm-null-reference-identity.md](jvm-null-reference-identity.md).

## Combined constructor and dispatch evidence

The unchanged `SuperCalls.scala` source fixture exercises parameterized
`Derived.<init>(I)V -> Base.<init>(I)V` and initialized direct-super method
calls in the same frontend/emission batch. Decoded classfiles check exact
Methodref owners/descriptors and max_stack/max_locals; JDK 21 verifies all
classes with `-Xverify:all`. These branchless methods carry no StackMapTable.
The additional Branching client checks exact Derived reference entries and
requires frames at decoded branch target PCs, then executes both branches.
Frame generation still belongs to dotty-classfile; the test does not construct
or patch frames or their bytecode offsets.

Focused builder tests reject an extra `aload_0`, allocation, or effectful call
before superclass initialization, incorrect parent owners, and branched
constructors. The CodeBuilder rejects `invokevirtual <init>` before encoding;
the class-aware method path continues to reject constructor calls on already
initialized receivers. Source integration tests assert their actual frontend
or JVM adapter errors and an empty published batch.

This evidence does not broaden the verifier/source contract: nested allocation
arguments, arbitrary effectful pre-super evaluation, branched constructors,
qualified/trait-qualified `super`, explicit grandparent-qualified super,
general reference LUB inference, and Scala ABI/TASTy emission remain deferred.
The two-live-allocation state required by nested constructor arguments, and the
Scala 3.9 source evidence for that boundary, are detailed in the
[object graph construction audit](jvm-object-graph-construction.md).
