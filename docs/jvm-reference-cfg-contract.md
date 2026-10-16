# JVM reference CFG contract audit (Java 21)

This document pins the verifier contract for the next reference-aware CFG
increment. It is a design and characterization audit only: it does not enable
any new instructions or change the class-file/JVM lowering APIs. The source of
truth is the Java SE 21 JVM Specification (JVMS 21), not a later release.

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

JVMS permits assignment-compatible subtyping in several operations. This
backend's first supported reference slice deliberately requires exact owner
identity instead: it has no hierarchy/classpath proof and must not guess an
ancestor, interface, `java/lang/Object`, or a common superclass. That is a
conservative implementation restriction, not a claim that JVMS requires exact
type equality.

## Why the existing analysis is insufficient

`ValueKind::Reference` in `LinearCodeAnalyzer` records only a one-slot category.
The linear analyzer uses it for `null`, `new`, `aload`/`astore`, fields, calls,
and reference returns. It does not retain the referenced class, distinguish
initialized from uninitialized references, track the PC of `new`, or validate
that a constructor's owner matches the allocation. A passing linear-analysis
result therefore is not evidence of verifier safety.

`CfgCodeAnalyzer` has `FlowLocalState::{Top,Integer,Receiver}` and stores a
`Vec<ValueKind>` for the operand stack. It rejects reference-producing and
reference-consuming instructions before propagation. `Receiver` means the
current instance placeholder; it is not a general reference value. Its
integer-only stack join is safe only because all admitted stack values are
integers. `StackMapFrameGenerator` can encode integer, top, and a supplied
receiver class, but it cannot turn arbitrary reference names into
`VerificationTypeInfo::Object` entries.

The current constructor path in `ClassFileBuilder` rejects branched `<init>`
bodies because it does not track the transition from `uninitializedThis` to
initialized `this`. `dotty-jvm` also rejects reference-bearing source CFGs at
its existing conservative guards. Keep those failures until a future issue
implements and validates the state model below.

## Proposed state model

Keep locals and operand stack as separate structures. A future abstract value
must at minimum distinguish:

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
`Initialized(emitting_class)`. Static methods start with only descriptor
parameters. The descriptor parameters themselves remain restricted to the
existing primitive subset; public reference parameters and results do not
become supported as a side effect. A constructor starts with
`UninitializedThis`; do not treat it as an ordinary receiver. Its branch path
and superclass-initialization flow remain unsupported.

## Join and termination policy

At each reachable block:

1. Operand-stack heights must match. Every corresponding stack value must be
   exactly equal (`Integer` with `Integer`, or `Initialized(C)` with the same
   `Initialized(C)`). A different class, initialized/uninitialized mismatch,
   different allocation site, or different stack height returns a typed
   analysis error. Never insert `java/lang/Object` or another guessed parent.
2. A local slot whose incoming states are identical keeps that state. Any
   disagreement degrades the local to `Top`. A later read of `Top` fails as
   uninitialized/unusable; this is the definite-assignment guard that makes
   local degradation safe. No instruction may load an uninitialized or `Top`
   local.
3. `Top` is absorbing for locals. Each local can therefore degrade at most
   once; stack disagreement fails immediately. For `B` blocks and `L` local
   slots, state propagation has a finite bound proportional to
   `B * (L + 1)`. Traverse blocks, successors, and local slots in stable
   source/index order so results do not depend on hash-map order.

These rules are intentionally narrower than JVMS assignment compatibility.
Reference stack joins with distinct classes are rejected even if those classes
would share a JVM superclass. Distinct local references can become `Top` only
when later bytecode does not read that slot; there is no implicit common-type
merge.

## Transfer and constant-pool checks

The first implementation should add explicit owner/type checks to the transfer
functions, rather than deriving a reference type from `ValueKind` or opcode
category alone.

| Instruction family | Required transfer/check in the first subset |
| --- | --- |
| `aload` / `astore` | Load only an initialized exact-class local. Stores preserve the exact initialized class; reject unusable or uninitialized values. |
| `dup` / `pop` | Preserve or discard the same abstract value; `dup` must retain one allocation-site identity, not initialize it. |
| `getfield` / `putfield` | Validate the fieldref and descriptor. Receiver must be initialized and its class must exactly equal the field owner. Values must match the field's supported primitive category. |
| `invokevirtual` | Validate methodref and descriptor. Receiver class must exactly equal the method owner; reference arguments/results remain unsupported. |
| `new` | Validate the `CONSTANT_Class`; push a fresh uninitialized allocation-site identity. |
| `invokespecial <init>` | Consume only the matching uninitialized allocation and exact owner/descriptor. On normal completion rewrite all aliases of that allocation to `Initialized(owner)`. |
| Integer instructions and branches | Preserve the existing integer transfer rules. Branch operands are int-like only; reference equality branches are out of scope. |
| Returns | Preserve existing primitive/void checks. Reference returns remain unsupported for backend methods. |

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

Branched `<init>` methods remain unsupported: tracking
`UninitializedThis`, the first legal `this`/direct-super constructor call, and
the allowed pre-super current-class `putfield` exception is a separate
constructor-flow problem. No branch target may assume initialized `this`
before that transition.

## Constant-pool and frame construction

The class-file builder interns symbolic references before freezing the
constant pool. A future reference-aware path must collect every class identity
needed by branch frames, including identities not otherwise used by an
instruction, and call the existing typed class interning API before
`finish()`. It then resolves each internal name through that builder's
name-to-index result and emits `VerificationTypeInfo::Object(cp_index)`.
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
entries, the `Counter` class reference, reference-bearing loop/local joins,
current-instance and field/call instructions, and object construction in
branches. The integration test runs it under `java -Xverify:all` and checks
the observable values. This is a JDK 21 positive verifier oracle; it does not
prove the repository's builder supports these methods.

The class-file unit characterization pins the current failure boundary:
`validate_flow_instruction` returns
`UnsupportedFlowInstruction { kind: Reference }` for each currently excluded
reference opcode before CFG type flow. This is a generic implementation
boundary, not a claim that all malformed reference programs have already been
diagnosed precisely. The linear analyzer may accept category-shaped invalid
sequences because it is not a verifier; do not use its success as a positive
test for any row below.

| Case | Required future behavior | Current characterization |
| --- | --- | --- |
| `Counter` reference local through a loop header and join | Same `Initialized(Counter)` state at each compatible stack/local join; emit `Object(Counter)` frame entry | JVMS 21 oracle passes; repository CFG rejects reference opcodes |
| Reference-bearing conditional and current `this` | Seed local 0 as emitting class; preserve exact receiver state at branch target | JVMS 21 oracle passes; no reference CFG acceptance |
| `getfield`, `putfield`, `invokevirtual` with exact owner | Validate CP operands and require exact receiver owner in this subset | JVMS 21 oracle passes; repository CFG rejects these opcodes |
| Different initialized classes at a stack join | Typed incompatible-stack reference error; do not merge to `Object` | Producers are rejected before repository CFG reaches the join |
| Different initialized classes or missing assignment at a local join | Merge to unusable `Top`; reject a later read | Reference producers are rejected before local dataflow |
| Wrong field/method/constructor owner | Typed owner mismatch before emission | `LinearCodeAnalyzer` has no class identity check; CFG rejects the opcode family |
| Uninitialized local read or uninitialized object escape | Typed uninitialized-use error; never emit the class | Not modeled by the linear analyzer; CFG rejects reference operations |
| Branch between `new` and matching constructor | Typed unsupported/uninitialized-on-edge error | CFG rejects `new` / `invokespecial` before edge transfer |
| Wrong constructor owner for a `new` allocation | Typed allocation-owner mismatch | Not modeled by the linear analyzer; CFG rejects `invokespecial` |
| Missing, wrong-kind, or malformed CP `Class` entry | Existing typed constant-pool validation error; never manufacture a frame type | Builder/decoder validation remains the current boundary |
| Branched constructor before `this`/super initialization | Keep rejected as unsupported constructor flow | `ClassFileBuilder` rejects branched `<init>` analysis |
| Reference method parameter/result | Keep rejected by JVM source backend | Existing backend returns `UnsupportedJvmType` |

No old negative tests should be removed to make future reference tests pass.
When a later issue enables one row, replace only that row's obsolete rejection
assertion with positive coverage and retain all adjacent negative cases.

## Ownership for follow-up work

- The next class-file analyzer change owns the abstract reference values,
  transfer checks, exact joins, and deterministic fixed-point policy.
- The class-file builder/frame change owns pre-freeze class interning and
  resolving internal names to real `VerificationTypeInfo::Object` indices.
- A later bounded-control-flow backend issue owns the Scala `SymbolId` to
  validated JVM internal-name mapping and enabling reference operations in
  `dotty-jvm` only after class-file evidence exists.
- A separate constructor-flow issue owns branched `<init>` and
  `UninitializedThis`. General inheritance/interface merges, arrays, `null`,
  reference signatures, exception-flow joins, and arbitrary uninitialized
  values remain out of scope.
