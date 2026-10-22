# Mutable reference locals: verifier and frontend audit

This document records the audit for issue #1454. It does **not** enable
mutable-reference-local code generation. The backend must continue to reject
those locals until the separately owned verifier, metadata, assignment, and
control-flow work is complete.

## Source evidence and current failure matrix

`tests/fixtures/jvm/MutableReferenceLocals.scala` preserves the issue's
candidate source unchanged. The repository frontend currently stops while
typing `Base.value`: the bare constructor parameter `seed` is reported as
`TermNameNotFound` at the exact `seed` source span. No typed unit, JVM plan,
lowering, or classfile analysis is reached for this exact source. This is a
limitation of this repository's frontend, not evidence that the source is
invalid Scala.

The PR review independently compiled the exact source with its cached Scala
3.9.0 reference compiler; its reported compiler revision begins with the
pinned hash `777528f19a58e794c9954a42f433373472ec57f8`. That oracle accepts the
source and reports the expected `Base`-declared mutable local, `If`/`While`
assignments, `current.value()` selections, and contextual sibling union. This
is reviewer-run compatibility evidence; the local environment used for this
audit did not have that compiler available.

The two checked-in adaptations isolate distinct current blockers. The first
changes only `Base` to store `seed` in `currentSeed` while preserving all three
`current.value()` reads. The second keeps that storage adaptation but returns
an integer literal from candidate methods so the typed local shapes can reach
the backend without member selection. Each candidate is compiled alone from
the source fixture, so the first failure is independently observable:

| Input | `choose` | `repeat` | `initialize` | Reached stage |
| --- | --- | --- | --- | --- |
| Exact issue source | `TermNameNotFound(seed)` in `Base.value` | Same earlier unit failure | Same earlier unit failure | Typer, before candidate method completion |
| `MutableReferenceLocalsReadback.scala`, only `Base` storage adapted | `UnstableSelectionPrefix` at `current.value()` | Same | Same | Typer, before typed unit/JVM plan |
| `MutableReferenceLocalsTyped.scala`, `current.value()` replaced by `42` | `UnsupportedMutableReferenceLocal` for canonical `choose.current` | Same for `repeat.current` | Same for `initialize.current` | `dotty-jvm` local layout, `method_local_layout` in `crates/dotty-jvm/src/locals.rs` |

The readback test pins the error's source tree ID to the real `Select` node,
the direct `current` `Ident` qualifier, and both source spans. The failed
typer expression rolls back the local's temporary typed mappings and symbol,
so no canonical local `SymbolId` survives that failure path to inspect. The
literal-result fixture separately pins canonical local symbols, owners,
mutability flags, declared types, typed initializer/assignment nodes, and
class identities from a successful real typed unit. All failing runs assert
that no classfile was published. None reaches classfile CFG analysis; the
current classfile local join behavior below is established by code and its
existing tests, not by these source failures.

The literal-result typed observer pins these source-backed relationships:

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
future Scala-independent classfile analyzer should use only the verified JVM
hierarchy supplied to it and these bounded states:

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

The future source-lowering subset must enforce all of these rules before
emitting a store:

- Assignment requires the same canonical mutable local (`var`); assignment to
  a `val` remains a typed error.
- The local must already be initialized by its declaration before a read or
  reassignment. No uninitialized-local declaration is admitted by this subset.
- Evaluate the RHS exactly once, prove it assignable to the local's declared
  planned class, then emit one `astore` to the canonical slot.
- Assignment has source type `Unit`; it does not yield the stored reference.
- A checked `Child <: Base` assignment is accepted. No implicit downcast,
  narrowing, boxing, or conversion is inserted.
- Source definite initialization remains distinct from verifier type
  widening. An `If` intersection and the conservative pre-loop initialized
  set remain required; a verifier join cannot prove a source read safe by
  itself.

For `choose`, both branch paths preserve an initialized local, so the join
must accept its initial `Left` and assigned `Right`. For `repeat`, the header
must include initial `Left` (including zero iterations) and the `Right` written
by a positive iteration; at exit, the local is definitely initialized because
it was initialized before the loop, and its verifier state must accept either
class. The contextual initializer has no subsequent store, but its two
initializer paths still require the declared-target proof above.

## Current join and initialization behavior

The classfile CFG analyzer in
`crates/dotty-classfile/src/bytecode/analysis.rs` keeps local states separate
from operand-stack values. Local disagreements currently become
`FlowLocalState::Top`; operand-stack references use a separate checked
hierarchy join. This is conservative, but does not retain the checked
non-Object ancestor for initialized reference locals. The classfile layer must
remain Scala-independent.

The source JVM emitter intersects initialized-local sets at reachable `If`
branches. For `While`, it restores the pre-loop initialized-local set at the
exit, conservatively covering zero iterations. These facts do not implement
reference stores, loads, or local verifier-type propagation. Do not weaken
them in this audit.

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
| #1456 | `dotty-jvm` | Canonical mutable planned-class local metadata and deterministic slot allocation using the declared class. Does not enable reads or writes. |
| #1457 | `dotty-jvm` | Straight-line checked mutable reference assignment and `astore`; `val` rejection, initialized `var`, one RHS evaluation, assignability, `Unit` result, no implicit narrowing/boxing. |
| #1458 | `dotty-jvm` | Contextual sibling `If` result used for `Base`-typed local initialization/assignment, after the straight-line assignment contract. |
| #1459 | `dotty-jvm` + classfile integration | Compose approved writes/reads through `If` and `While`, zero and positive loop counts, while preserving source definite initialization and verifier joins. Depends on #1455–#1458 and a separately tracked `dotty-typer` prerequisite for the observed `UnstableSelectionPrefix` on mutable local receivers. |
| #1460 | End-to-end integration / CI | Real Scala source to JDK 21+ execution, decoded local frames, exact bytecode and runtime dispatch after #1455–#1459 and the typer prerequisite. |

The existing #1455–#1460 issue set does not assign ownership for fixing the
typer's unstable-prefix rule. Track that as a focused `dotty-typer` issue
before claiming the source `current.value()` readback path in #1459/#1460.
This audit records the blocker but does not change typer stability semantics.

## Compatibility and runtime validation

The pinned Scala 3.9.0 revision is
`777528f19a58e794c9954a42f433373472ec57f8`. The author environment did not
have the compiler available; the PR review reports an independent cached
compiler run at the matching hash prefix, as noted above. This audit adds no
executable mutable-reference classfile fixture because the backend correctly
rejects the feature. No strict mutable-reference runtime or frame result is
claimed. Any future executable characterization must use JDK 21+ with strict
verification and must not use a passing verifier run to replace source-level
semantic proof.
