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
class hierarchy for assignability checks and operand-stack joins. Stack joins
may widen initialized references to the nearest shared, checked non-`Object`
superclass. Locals with different incoming states degrade to `Top`. Ordinary
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
reference parameters and results through `if`, `while`, and ordinary planned
calls, plus the validated ancestor conversions listed above. Source lowering
still rejects mutable reference locals, sibling result joins, arrays, null,
and branched constructors.

## Bounded state model

Keep locals and operand stack as separate structures. The abstract value model
distinguishes:

| Abstract value | Meaning | Stack-map representation |
| --- | --- | --- |
| `Top` | Unusable local after no assignment or a local join | `Top` |
| `Integer` | JVM int-like primitive | `Integer` |
| `Initialized(class)` | Initialized reference with exact validated JVM internal class name | `Object(cp_index)` |
| `Uninitialized(allocation_site)` | Fresh instance at one specific `new` instruction | `Uninitialized(new_pc)`; prohibited at CFG joins in this increment |
| `UninitializedThis` | Constructor receiver before the required `this`/super call | `UninitializedThis`; branched constructors remain deferred |

Class-file analysis must remain Scala-independent. Its identity is a validated
JVM internal name or validated `CONSTANT_Class`, never `SymbolId`. The JVM
adapter may map its canonical source class identity to that internal name
before invoking class-file analysis.

For an ordinary non-static, non-constructor method, the initial local 0 is
`Initialized(emitting_class)`. Static methods start with descriptor-derived
parameters, including exact object identities. The Scala source backend still
restricts emitted public method signatures to its existing subset. A constructor starts with
`UninitializedThis`; do not treat it as an ordinary receiver. The validated
branchless constructor path accepts only a bounded sequence of integer
constants and loads from initialized constructor parameter locals before one
direct-super `<init>` call. It checks the call's owner, descriptor, argument
categories, and exact reference identities before analyzing the remainder as
initialized `this`. Branched constructor flow remains unsupported.

## Join and termination policy

At each reachable block:

1. Operand-stack heights must match. `Integer` joins only with `Integer`.
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
2. A local slot whose incoming states are identical keeps that state. Any
   disagreement degrades the local to `Top`. A later read of `Top` fails as
   uninitialized/unusable; this is the definite-assignment guard that makes
   local degradation safe. No instruction may load an uninitialized or `Top`
   local.
3. `Top` is absorbing for locals. Each local can therefore degrade at most
   once. Stack values can widen only along the validated, eight-edge-bounded
   superclass chain. The worklist reprocesses a block whenever either its
   locals or stack state changes and has a defensive work limit derived from
   the observed stack width and hierarchy depth. Traverse blocks, successors,
   and local slots in stable source/index order so results do not depend on
   hash-map order.

These rules are intentionally narrower than JVMS assignment compatibility.
Reference stack joins use only explicit superclass edges and never infer a
classpath relationship. Distinct local references become `Top` even when one
is an ancestor of the other or both share a checked superclass, so there is no
hierarchy-based local promotion. A later read of that slot fails as
uninitialized/unusable.

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
| `dup` / `pop` | Preserve or discard the same abstract value; `dup` must retain one allocation-site identity, not initialize it. |
| `getfield` / `putfield` | Validate the fieldref and descriptor. Receiver must be initialized and assignable to the field owner using the explicit hierarchy; a reference value must be assignable to the declared field class. |
| `invokevirtual` | Validate methodref and descriptor. Pop supported arguments right-to-left; each object argument and receiver must be assignable to its declared class/owner using the explicit hierarchy. Push the exact object result or int-like result. Arrays and category-two types remain unsupported. |
| `new` | Validate the `CONSTANT_Class`; push a fresh uninitialized allocation-site identity. |
| `invokespecial <init>` | The constructor owner must equal the uninitialized allocation class; constructor arguments use explicit hierarchy assignability. On normal completion rewrite all aliases of that allocation to `Initialized(owner)`. |
| `invokespecial DirectParent.method` | Require the `Methodref` owner to equal the validated direct parent and the receiver to be exactly the initialized emitting class. Validate the supported descriptor arguments, then push its exact reference or int-like result. The class-file builder requires `ACC_SUPER`; constructors keep their separate allocation transfer. |
| Integer instructions and branches | Preserve the existing integer transfer rules. Branch operands are int-like only; reference equality branches are out of scope. |
| Returns | Preserve primitive/void checks; `areturn` requires an initialized reference assignable to the planned result class using the explicit hierarchy. |

Field and method constant-pool operands must be resolved to validated entry
families, nested owner/class names, and parsed descriptors before transfer.
The exact-owner restriction applies after resolving the `CONSTANT_Fieldref` or
`CONSTANT_Methodref`; an index match alone proves nothing about a receiver.
Malformed or absent `CONSTANT_Class` references remain typed constant-pool
errors, not guessed names.

## Allocation and constructor boundary

Allow only this bounded sequence wholly inside one basic block:

```text
new C
dup
primitive argument evaluation
invokespecial C.<init>(descriptor)
```

Before the matching constructor completes normally, the uninitialized value
must not be stored in a local, returned, passed to another call, used for a
field operation, used as an ordinary receiver, or cross a branch edge. The
constructor owner must equal the allocation class. `dup` aliases must all be
rewritten to initialized `C` after the matching call. No uninitialized value
may appear in a merge state or emitted branch-target frame.

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
| Sibling initialized classes at a stack join | Join only at the nearest checked non-`Object` superclass; reject missing paths and an inferred `Object`-only intersection | Classfile analyzer and encoded-frame regressions |
| Unrelated initialized classes at a stack join | Typed incompatible-stack reference error; do not infer a common class | Classfile analyzer regression tests |
| Different initialized classes or missing assignment at a local join | Merge to unusable `Top`; reject a later read | Classfile analyzer regression tests |
| Wrong field/method/constructor owner | Typed owner mismatch before emission | Classfile analyzer and planned-target regressions |
| Uninitialized local read or uninitialized object escape | Typed uninitialized-use error; never emit the class | Classfile analyzer allocation regressions |
| Branch between `new` and matching constructor | Typed unsupported/uninitialized-on-edge error | Classfile analyzer allocation regressions |
| Wrong constructor owner for a `new` allocation | Typed allocation-owner mismatch | Classfile analyzer allocation regressions |
| Missing, wrong-kind, or malformed CP `Class` entry | Existing typed constant-pool validation error; never manufacture a frame type | Builder/decoder validation remains the current boundary |
| Branched constructor before `this`/super initialization | Keep rejected as unsupported constructor flow | `ClassFileBuilder` rejects branched `<init>` analysis |
| Exact planned reference method parameter/result | Accept; seed parameter frames and require matching initialized `areturn` values | Source integration and mandatory JDK 21 verifier gate |
| Transitive `Leaf -> Base` argument or return | Accept only with the validated two-edge ancestry; preserve the concrete value class and emit no cast | `MultilevelInheritance.scala`, decoded descriptors/references, and JDK 21 `-Xverify:all` harness |

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
  single-parent graph, arrays, `null`, interface inheritance, exception-flow
  joins, and arbitrary uninitialized values remain out of scope. For source compilation, the adapter proves the
  bounded same-unit `Base -> Object`, `Middle -> Base`, and `Leaf -> Middle`
  chain; a source-typed `If` joins distinct initialized references only when
  they are ancestor-related and both fit its canonical result class.

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
