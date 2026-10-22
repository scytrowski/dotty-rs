# Mutable reference locals: verifier and frontend contract

This document records the audit and implementation from #1454–#1459 and the
end-to-end acceptance evidence for #1460. A canonical mutable local of an exact
planned reference class keeps its declared class type and deterministic
category-one slot. Its initializer and assignments are evaluated once and
stored with `astore`; initialized mutable locals can be read with `aload`.
Assignments are supported in straight-line code, `If` branches, and `While`
bodies. Source definite-initialization uses the reachable-arm intersection for
`If` and the pre-loop set after `While`; the classfile CFG analyzer separately
joins compatible initialized local values to their checked nearest non-`Object`
superclass. Contextual sibling-`If` values require an explicit declared target
and retain the source `Type::Or` proof.

## Source evidence and current failure matrix

`tests/fixtures/jvm/MutableReferenceLocals.scala` is the real source fixture
for this increment. `Base` stores its constructor argument in a private
mutable `current: Int` field, and `Base.value` reads that field. The original
plain constructor-parameter read (`def value(): Int = seed`) remains a separate
frontend/storage concern and is outside this increment.

The PR review independently compiled the exact source with its cached Scala
3.9.0 reference compiler; its reported compiler revision begins with the
pinned hash `777528f19a58e794c9954a42f433373472ec57f8`. That oracle accepts the
source and reports the expected `Base`-declared mutable local, `If`/`While`
assignments, `current.value()` selections, and contextual sibling union. This
is reviewer-run compatibility evidence; the local environment used for this
audit did not have that compiler available.

The typed fixture retains the supported `Base` backing field but returns an
integer literal from candidate methods, isolating local CFG behavior from
virtual dispatch.

| Input | `choose` | `repeat` | `initialize` / `swap` | Reached stage |
| --- | --- | --- | --- | --- |
| `MutableReferenceLocals.scala` | Branch assignment selects `Right.value()` when true | Loop assignments select `Right.value()` after one or more iterations and `Left.value()` after zero iterations | Sibling-`If` initialization and assignment dispatch to the selected subtype | Source JVM pipeline, decoded classfiles and StackMapTable; JDK 25 `javac`, `javap`, and `java -Xverify:all` execution |
| `MutableReferenceLocalsTyped.scala`, `current.value()` replaced by `42` | Canonical `current: Base` survives a one-arm branch assignment and is returned after the join | Canonical `current: Base` survives loop assignments and remains initialized for zero or more iterations | An explicit `Base` local consumes a checked sibling-union initializer | Source JVM pipeline; branch and loop local-frame regressions |

The acceptance test observes one real typed session and checks canonical
`Base`/`Left`/`Right` identities, the mutable local owner and declared type,
and the `Base` slot used by `choose`, `repeat`, `initialize`, and `swap`. It
decodes all four classfiles, verifies exact superclass/field/constructor/method
references and verifier frames, and compares complete class bytes from two
fresh frontend sessions. Its JDK harness covers seeds `10`, `0`, and `-3`,
both branch choices, zero/one/multiple loop iterations, sibling initializer
joins, and sibling assignment joins. Separate source regressions cover
reversed branch order, nested branches, early return, exact runtime subtype,
and unsupported/maybe-initialized states.

The literal-result typed observer pins these source-backed relationships and
checks that each declared-`Base` mutable binding receives a `Base` slot while
the typed unit remains intact:

| Method | Initializer / assignment evidence | Typed control-flow shape |
| --- | --- | --- |
| `choose` | Mutable `current` is owned by `Runner.choose`, declared `Base`, initialized with canonical `Left`, and assigned canonical `Right` | The assignment to that same local symbol is inside the typed `If` then branch |
| `repeat` | Mutable `current` is declared `Base`, initialized with canonical `Left`, and assigned canonical `Right` | The assignment is inside the typed `While` body; `n = 0` is the zero-iteration path and positive `n` takes the back-edge |
| `initialize` | Mutable `current` is declared `Base`; its initializer retains `Type::Or(Left, Right)` with the canonical sibling symbols | Typed `If` inside the declaration initializer, with no later assignment |

The canonical parents of `Left` and `Right` are the same canonical `Base`
symbol. Tests obtain those IDs from the actual compilation session; no AST or
semantic identities are synthesized. Numeric arena indices are not recorded
as stable contracts.

## Bounded verifier-local lattice

Local-slot joins and operand-stack joins are separate. For local slots, the
Scala-independent classfile analysis uses only the verified JVM hierarchy
supplied to it and these bounded states:

| State | Meaning and join rule |
| --- | --- |
| `Top` | Unusable/unassigned or a prior incompatible join. Absorbing: `Top ⊔ X = Top` for every ordinary local state `X`; a later reference store on another path cannot make it definitely initialized. |
| `Integer` | The supported int-like category. Equal states remain `Integer`; a mismatch with another category becomes unusable `Top` or a typed rejection. |
| `Object(C)` | An initialized reference to the exact validated class `C`. Equal references remain exact. Two different initialized references join only to their nearest uniquely checked common **non-Object superclass**. If there is no such proof, including an Object-only common ancestor, keep `Top` or return the existing typed analysis error. |
| `Receiver` | The current receiver state. Preserve only under the existing receiver-specific exact rules; never promote it through an ordinary reference hierarchy join. |
| `UninitializedThis` / `Uninitialized(offset)` | JVM constructor/allocation states, distinct from `Top` and every initialized `Object(C)`. Preserve only under existing verifier constructor rules; do not join them as ordinary references or turn them into `Object(Base)`. |

An incoming `Top` remains absorbing even if other predecessors carry
`Object(Left)` and `Object(Right)`. `Integer` and initialized references never
join as a reference. Missing/ambiguous hierarchy evidence, unsupported slot
families, or a potentially uninitialized path cannot produce an initialized
reference state. Preserve predecessor evidence across widening so a late
incoming class cannot be accepted just because an earlier aggregate happened
to be `Base`. Requeue affected blocks to a deterministic, bounded fixed point
for loop back-edges.

For the positive sibling case, `Object(Left) ⊔ Object(Right)` may become
`Object(Base)` only when the explicit checked hierarchy proves `Base` is their
nearest unique common non-Object superclass. If no such proof exists, the
result remains unusable or is rejected. Do not select an arbitrary declared
target, use a generalized LUB, or fall back to `Object`.

JVMS 21 distinguishes initialized class verification types from
`uninitializedThis` and `uninitialized(offset)`, and specifies frame/local and
load/store constraints in §§4.10.1.2, 4.10.1.4, and 4.10.1.7: [Java Virtual
Machine Specification, Java SE 21, Chapter
4](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html).

## Source type and approved expected-Base positions

The declared Scala type `current: Base` is not a JVM flow state. It constrains
source assignment and member selection, while a JVM local can hold an actual
`Left` or `Right` value and later have a verifier state widened to
`Object(Base)`. The classfile analyzer is Scala-independent: it must derive a
join from checked internal-name hierarchy evidence and must not inspect or
infer a Scala declared type.

The Scala-aware `dotty-jvm` layer may supply an expected `Base` only at the
approved source positions proven by an explicit declared target:

- `var current: Base = new Left(seed)` uses the declaration's expected type.
- `current = new Right(seed)` uses the declared type of the mutable local on
  the assignment's left side.
- `var current: Base = if flag then new Left(seed) else new Right(seed)`
  supplies `Base` to the contextual initializer branches.
- A contextual sibling `If` used as the RHS of that same declared-`Base`
  initializer or assignment may use the existing checked sibling-union
  projection.

These contexts do not authorize an inferred `Left | Right` local, arbitrary
union erasure, or a classfile join to an unverified target. Every incoming
source value must first be proved assignable to the expected source type; the
classfile layer then independently computes its bounded verifier join.

## Source mutation and initialization subset

The implemented source-lowering subset enforces these rules before emitting a
store:

- Assignment requires the same canonical mutable local (`var`); assignment to
  a `val` remains a typed error.
- The local must already be initialized by its declaration before a read or
  reassignment. No uninitialized-local declaration is admitted by this subset.
- Evaluate the RHS exactly once, prove it assignable to the local's declared
  planned class, then emit one `astore` to the canonical slot.
- Assignment has source type `Unit`; it does not yield the stored reference.
- A checked `Child <: Base` assignment is accepted. No implicit downcast,
  narrowing, boxing, or conversion is inserted.
- An initialized reference local may be reassigned in an `If` branch or a
  `While` body. The RHS is evaluated once on the executing path and stored to
  the same canonical slot.
- Assignments in a short-circuit operand or a `While` condition remain
  rejected; those expressions have conditional or repeated evaluation rules
  outside this increment.
- Source definite initialization remains distinct from verifier type
  widening. An `If` intersection and the conservative pre-loop initialized
  set remain required; a verifier join cannot prove a source read safe by
  itself.

For `choose`, both branch paths preserve an initialized local, so the join
must accept its initial `Left` and assigned `Right`. For `repeat`, the header
must include initial `Left` (including zero iterations) and the `Right` written
by a positive iteration; at exit, the local is definitely initialized because
it was initialized before the loop, and its verifier state must accept either
class. Reverse branch order, nested `If` writes, and a terminal branch exercise
the same source-state and verifier-state separation. The contextual initializer
has no subsequent store, but its two initializer paths still require the
declared-target proof above.

## Current join and initialization behavior

The classfile CFG analyzer in
`crates/dotty-classfile/src/bytecode/analysis.rs` keeps local states separate
from operand-stack values. At the time of this audit, every local disagreement
became `FlowLocalState::Top`. Follow-up #1455 now allows two or more
definitely initialized object inputs to join at their nearest uniquely checked
non-`Object` superclass, retaining the original class evidence through CFG
widening. A `Top`, uninitialized, primitive/reference, receiver/reference, or
unproven input still prevents an initialized local join. The classfile layer
remains Scala-independent.

The source JVM emitter intersects initialized-local sets at reachable `If`
branches. For `While`, it restores the pre-loop initialized-local set at the
exit, conservatively covering zero iterations. Reference assignments use
those facts to permit writes only to locals initialized on the current path.
The classfile analyzer separately propagates their actual stored classes,
joining `Left`/`Right` to `Object(Base)` at branch joins and loop headers.
Neither analysis substitutes for the other.

## Explicit exclusions

This audit and the initial successor subset exclude:

- inferred reference-union locals without an explicit declared/approved
  expected class;
- `null`, arrays, traits/interfaces, external/classpath parents, arbitrary
  generalized LUB or erasure, reference equality, and implicit
  downcasts/conversions;
- exception/`try` edges, unsafe/branched constructor CFG, uninitialized
  allocations crossing control-flow edges, and new source syntax constructs;
- promoting `Receiver`, `UninitializedThis`, `Uninitialized(offset)`, or
  `Top` to an initialized ordinary reference.

Existing single/multilevel/sibling bytecode, constructor guards, and primitive
local behavior must remain unchanged.

## Successor ownership

| Issue | Owning layer | Scope |
| --- | --- | --- |
| #1454 | Audit / cross-layer contract | Source evidence, current failure matrix, bounded verifier and source-write rules, exclusions, and ownership below. No codegen feature. |
| #1455 | `dotty-classfile` | Scala-independent checked initialized-reference local joins and `StackMapTable` local frames; nearest unique non-Object superclass; absorbing `Top`; preserve receiver/constructor/allocation guards. |
| #1456 | `dotty-jvm` | Canonical mutable planned-class local metadata, deterministic slot allocation, and initialized declaration lowering to `astore` using the declared class. |
| #1457 | `dotty-jvm` | Initialized mutable reference `aload`, straight-line checked assignment and `astore`; `val` rejection, one RHS evaluation, assignability, `Unit` result, no implicit narrowing/boxing. |
| #1458 | `dotty-jvm` | Contextual sibling `If` result used for `Base`-typed local initialization/assignment, after the straight-line assignment contract. Implemented by forwarding the canonical local symbol's completed declared type to the existing checked projection. |
| #1459 | `dotty-typer`, `dotty-jvm` + classfile integration | Permit direct applicable method calls on planned mutable local receivers, then compose reference writes and calls through `If` and `While` while preserving source definite initialization and verifier joins. Bare field selections and overloaded mutable-local calls keep the stable-prefix restriction. |
| #1460 | End-to-end integration / CI | Real Scala source to JDK 21+ execution, decoded local frames, exact bytecode and runtime dispatch after #1455–#1459. |

The candidate fixture exercises `current.value()` end to end. The original
plain constructor-parameter read in `Base.value` remains a separate frontend
and storage concern and is not covered by this mutable-reference-local
increment.

## Compatibility and runtime validation

The pinned Scala 3.9.0 revision is
`777528f19a58e794c9954a42f433373472ec57f8`. The author environment did not
have the compiler available; the PR review reports an independent cached
compiler run at the matching hash prefix, as noted above. Regressions now check
the real frontend's declared-`Base` mutable slot, shadowed local identities,
stable layouts across fresh sessions, initialized mutable-local loads,
straight-line same-class and subtype assignments, branch/loop local joins, and
runtime virtual dispatch through `current.value()`. The narrow Typer allowance
applies only to a direct mutable local used by one applicable method call;
field selections and overloaded calls remain rejected as unstable. Executable
coverage uses JDK 21+ strict verification and does not replace source-level
semantic proof.
