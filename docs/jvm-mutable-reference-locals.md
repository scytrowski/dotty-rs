# Mutable reference locals: verifier and frontend audit

This document records the audit for issue #1454. It does **not** enable
mutable-reference-local code generation. The current backend must continue to
reject those locals until the CFG, assignment, initialization, and verifier
contracts below are implemented and checked on an executable fixture.

## Evidence from the repository frontend

`tests/fixtures/jvm/MutableReferenceLocals.scala` preserves the issue's three
candidate methods verbatim: conditional reassignment, reassignment inside a
`while`, and a conditional initializer with sibling reference types.

The exact candidate currently fails in `dotty-typer`, before a typed unit or
JVM plan exists. `Base.value` refers to its primary-constructor parameter
`seed`, and this source frontend reports `TermNameNotFound(seed)` at that
reference. Consequently, the exact issue source currently provides no evidence
about lowering, bytecode, runtime results, or Scala 3.9.0 acceptance. This is a
frontend limitation, not a finding that the source is invalid Scala.

`tests/fixtures/jvm/MutableReferenceLocalsTyped.scala` is an explicitly
adapted audit fixture. It stores the constructor argument in a field and
returns an integer literal from each candidate method so that unrelated member
selection and reference-result lowering do not prevent inspection of local
declarations. The test observes the real typed unit and pins these facts:

| Method | Initializer / assignment evidence | Typed control-flow shape |
| --- | --- | --- |
| `choose` | `current` is a mutable local owned by `Runner.choose`, declared as `Base`; initializer has canonical type `Left`; reassignment RHS has canonical type `Right` | A typed `If` whose then branch is the assignment to the same local symbol |
| `repeat` | Same declared local type and canonical `Left`/`Right` initializer/assignment identities | A typed `While`; its body contains the assignment to that local; `n = 0` denotes the zero-iteration path and positive `n` the back-edge path |
| `initialize` | `current` is still declared as `Base`; the initializer retains a `Left | Right` union with those exact canonical class symbols | A typed `If` inside the declaration initializer; there is no later assignment to `current` |

In this fixture the canonical parent of both `Left` and `Right` is the same
canonical `Base` symbol. These assertions use symbols and typed-tree IDs from
the compilation itself; they do not synthesize semantic identities. IDs are
intentionally not written here because allocation indices are not a stable
source-level contract.

The adapted fixture reaches code generation, where the first mutable reference
local is rejected by `method_local_layout` in
`crates/dotty-jvm/src/locals.rs` as
`UnsupportedMutableReferenceLocal { symbol }`. The test pins the reported
symbol to the actual `choose.current` declaration and checks that no classfile
was published. `emit_assignment` in `crates/dotty-jvm/src/expression.rs` also
contains a reference-local rejection; layout currently prevents reaching that
instruction path. Local reads through a member selection are not characterized
by the adapted fixture and remain outside this audit's observed typed result.

## Verifier contract to implement before code generation

The JVM verifier tracks a verification type for each local slot and each
operand-stack value at control-flow joins. These are related but distinct
contracts. Stack joins require equal stack heights and a compatible type at
each position. For reference values, the joined stack type must accept values
from both incoming paths. Local slots also need a safe join, but if a slot is
not initialized on every incoming path it must become unusable for loads; a
value present on only one branch cannot be treated as definitely initialized.
See JVMS 21 §§4.10.1.2, 4.10.1.4, and 4.10.1.7 for verification types, stack
map frames, and load/store constraints: [Java Virtual Machine Specification,
Java SE 21, Chapter 4](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html).

The backend's current abstract local state is `Top`, `Integer`, `Receiver`, or
an exact `Object(ClassIdentity)`. Its CFG merge currently maps any differing
local states to `Top`, while operand-stack references use a separate checked
hierarchy join (`join_flow_values`). `Top` is a safe conservative local merge,
but it loses usable type information for initialized reference locals. The
audit proposes this local lattice for a future increment:

| Incoming local states | Safe result |
| --- | --- |
| initialized compatible references `C1`, `C2` | A verified common supertype of both; prefer the declared local type when both assignments have already been checked assignable to it, otherwise use the hierarchy's nearest verified common supertype |
| same initialized primitive category | That category |
| initialized on one path and `Top`/uninitialized on another | `Top` (not definitely initialized) |
| incompatible primitive/reference categories or unusable slot shapes | `Top` or a typed analysis error, according to the existing frame contract |

Every `aload` from a local must be checked against the joined verifier type,
and every `astore` must update that slot's flow state. The worklist must
iterate to a fixed point across loop back-edges. For `repeat`, the header join
must account for both the initial `Left` value (including the path where the
loop executes zero times) and the `Right` value written by a positive
iteration. At the exit, the local is initialized because it is initialized
before the loop, but its verifier type must accept both possible values.

For `choose`, both branch paths initialize/retain the local, so the join must
accept the initial `Left` and assigned `Right`. `initialize` has no later
store: both arms of its initializer must join to a type compatible with the
declared local.

The source declaration `current: Base` and the verifier's actual type state
must not be conflated. `Base` is the declared static type and constrains legal
assignments and source member lookup. At a particular bytecode point, a local
may have the more precise verifier state `Left` or `Right`; after a control
flow join it may have `Base` (or another verified common supertype). The
declared type is a valid join candidate only after checking that every
incoming initialized value is assignable to it. A source union initializer
`Left | Right` likewise must not be mistaken for a JVM verification type: its
runtime values are references, and its verifier join must be established from
the class hierarchy and the emitted stores.

## Current join and initialization behavior

The classfile CFG analyzer already keeps local states separate from operand
stack values. In `crates/dotty-classfile/src/bytecode/analysis.rs`, local
disagreements become `FlowLocalState::Top`, while operand-stack references
join through the checked hierarchy. This conservative local rule is safe but
not sufficient to retain reference-local type information for future loads.

The source JVM emitter intersects initialized-local sets at reachable `if`
branches. For loops, it restores the initialized-local set from before the
loop at the exit, conservatively accounting for zero iterations. These rules
are relevant to definite initialization but do not yet implement reference
local assignment or CFG type propagation. Do not weaken them as part of this
audit.

## Compatibility and runtime checks not performed

The pinned Scala 3.9.0 compiler revision is
`777528f19a58e794c9954a42f433373472ec57f8`. It was not available in the
execution environment, so this audit did not confirm the exact candidate's
Scala 3.9.0 acceptance, canonical typed tree, or runtime outputs. The adapted
fixture above characterizes this repository's frontend only.

No JDK 21 tools (`java`, `javac`, or `javap`) were available either. There is
therefore no executable classfile fixture or strict verifier result in this
audit. Before enabling this feature, add a supported fixture that executes all
three control-flow shapes, including `repeat(0, seed)` and
`repeat(positive, seed)`, then compile it for the intended classfile version
and run it with strict JVM verification enabled. Assert both its runtime
results and that the verifier accepts the generated stack-map frames.

## Scope boundary

This issue is an audit. It does not implement reference stores, local
readback, branch/loop frame widening, union projection, or runtime execution.
It does not change primitive-local behavior, class hierarchy policy, or the
supported Scala language surface. A follow-up implementation must add focused
tests for local `aload`/`astore`, conditional and loop joins, definite
initialization, hierarchy joins, malformed/incompatible states, and strict
JDK 21 verification before removing `UnsupportedMutableReferenceLocal`.
