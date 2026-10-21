# JVM sibling reference joins audit

This document records the audit for issue #1433. It does not enable sibling
reference joins or change emitted bytecode. The source-language reference is
Scala 3.9.0 at pinned revision
`777528f19a58e794c9954a42f433373472ec57f8`; the class-file contract is JVMS
21.

## Source and observed frontend result

The integration test
`sibling_reference_join_audit_uses_the_real_parameterized_source_program`
runs this source through the actual lexer, parser, namer, typer, source
semantic index, and JVM adapter in one semantic store:

```scala
class Base(seed: Int):
  private var current: Int = seed
  def value(): Int = current
  def doubled(): Int = value() + value()

class Left(seed: Int) extends Base(seed):
  override def value(): Int = super.value() + 1

class Right(seed: Int) extends Base(seed):
  override def value(): Int = super.value() + 2

class Runner:
  def choose(flag: Boolean, seed: Int): Base =
    if flag then new Left(seed) else new Right(seed)
  def chooseReverse(flag: Boolean, seed: Int): Base =
    if flag then new Right(seed) else new Left(seed)
  def run(flag: Boolean, seed: Int): Int =
    choose(flag, seed).doubled()
  def accept(value: Base): Int = value.doubled()
  def viaArgument(flag: Boolean, seed: Int): Int = accept(choose(flag, seed))
  def chooseNested(first: Boolean, second: Boolean, seed: Int): Base =
    if first then new Left(seed)
    else if second then new Right(seed)
    else new Left(seed)
```

The test compares identities from the same semantic store, without asserting
allocation-order-dependent numeric IDs:

- `Left` and `Right` each have the canonical `Base` parent symbol.
- The typed method signatures of `choose`, `chooseReverse`, and
  `chooseNested` return the canonical `Base` type.
- In both branch orders, the typed `If` result remains `Type::Or` over the
  exact `Left` and `Right` symbols. The nested source form also reaches a
  typed `If` and retains those two union arms.
- The real typed call in `run` selects `Runner.choose` and `Base.doubled`.
  `viaArgument` selects `Runner.choose` and `Runner.accept`, whose parameter
  type is the canonical `Base` type.

The Scala source requests a `Base` result and both alternatives conform to
that declared parent. The JVM method descriptor expected from that declaration
is `Base`, and the two displayed concrete evaluations would produce 22 and 24
for seed 10 if code generation supported the join. This audit does not run
those evaluations or claim that dotty-rs emits them.

## Current rejection and ownership

The test's frontend observer runs after typing and before emission. It verifies
the typed trees and canonical identities above, then the real compilation
pipeline returns `JvmCodegenError::ReferenceTypeMismatch` during codegen and
publishes no class files. This pins the first failing layer: the parser,
namer, typer, and source signature construction accept and preserve the
program; the JVM adapter refuses to lower the sibling-valued `If`.

A separate minimal inferred-result characterization confirms that an
unannotated `choose` method keeps `Type::Or` in both the typed `If` and method
signature. Its first backend rejection is
`UnsupportedJvmType { kind: Union }` while projecting that inferred signature.
This is a different failure path from the explicitly Base-typed source above.

## Pinned Scala 3.9.0 comparison

The inferred-result source shape was also run against the pinned Scala
compiler:

```scala
class Base(seed: Int):
  private var current: Int = seed
  def value(): Int = current
  def doubled(): Int = value() + value()

class Left(seed: Int) extends Base(seed):
  override def value(): Int = super.value() + 1

class Right(seed: Int) extends Base(seed):
  override def value(): Int = super.value() + 2

class Runner:
  def chooseInferred(flag: Boolean, seed: Int) =
    if flag then new Left(seed) else new Right(seed)
```

To reproduce with JDK 21 and Coursier, save that block as
`SiblingReferenceJoinAudit.scala`, then run:

```sh
SCALA_CP="$(cs fetch --classpath org.scala-lang:scala3-compiler_3:3.9.0)"
COMPILER_JAR="$(printf '%s' "$SCALA_CP" | tr ':' '\n' | grep '/scala3-compiler_3-3.9.0.jar$')"
unzip -p "$COMPILER_JAR" compiler.properties | grep -Fx 'version.number=3.9.0'
unzip -p "$COMPILER_JAR" compiler.properties | grep -Fx 'git.hash=777528f'
mkdir -p out
java -cp "$SCALA_CP" dotty.tools.dotc.Main \
  -classpath "$SCALA_CP" -d out -Xprint:typer SiblingReferenceJoinAudit.scala
javap -classpath out -s Runner
```

The pinned artifact reports `version.number=3.9.0` and `git.hash=777528f`,
matching issue #1433's Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`. The Scala 3.9.0 typer output
reports `chooseInferred(flag: Boolean, seed: Int): Base`; `javap -s Runner`
reports descriptor `(ZI)LBase;`.

This is a typer result followed by its emitted JVM descriptor. It does not
show Scala retaining a union and erasing that union to Base later. In this
case Scala 3.9.0 infers Base as the method result; dotty-rs currently retains
`Type::Or` for the analogous inferred method and rejects the union during JVM
signature projection. That is a source-typing/inference difference, distinct
from the explicitly Base-typed sibling `If` that reaches dotty-rs's
`ReferenceTypeMismatch` lowering check. The pinned comparison is recorded
from the PR review's independent compiler run; this work environment still
cannot execute Scala or JDK commands itself. The audit does not change typer
behavior or claim the two paths are the same.

The typer's `Type::Or` is source semantic information. It must not be replaced
with `Base` merely because `Base` is a known JVM ancestor. Source conformance,
method erasure/descriptors, and verifier stack-map joins are separate
questions. A later implementation needs an explicit source-result policy
before it can tell the CFG analyzer that `Base` is the intended join type.

Independently, the JVMS 21 verifier permits a stack-map entry
`Object(Base)` when both incoming initialized values are assignable to Base.
The class-file analyzer currently supports joins only when one incoming class
is the other class's proven ancestor; it rejects Left/Right even though both
have the explicit edge to Base. The verifier rule does not choose a Scala
conditional type and is not a reason to reinterpret arbitrary `Type::Or`
values. Stack-map `Object(Base)` is about compatible verifier state, not
source type inference.

The Scala 3.9.0 revision is recorded as the compatibility target, but this
environment did not have a Scala compiler/JDK available to execute the source
through the pinned Scala oracle. The integration test is a dotty-rs frontend
characterization; it is not reported as an independent Scala oracle run.

## Conservative scope for follow-up work

Any later support should require a unique, explicitly proven common
superclass from the current planned hierarchy, and should keep source typing
separate from class-file dataflow. This audit does not authorize a global
union-to-class projection, inferred fallback to `java/lang/Object`, a cast,
or a change to the typer.

`JvmClassHierarchy::nearest_common_non_object_ancestor(left, right)` now
answers that source-independent proof query from its validated canonical
`SymbolId` edges. It returns the nearest shared planned class for siblings
(`Left`, `Right` → `Base`), ancestor pairs (`Leaf`, `Middle` → `Middle`), and
identity pairs (`Left`, `Left` → `Left`). The answer is symmetric and
independent of class-plan order. It returns `None` for an unplanned input or
when `java/lang/Object` is the only common ancestor. The public edge and proof
queries remain read-only; no names are used to recover source identity.

For the real source graph, `JvmClassHierarchy::internal_name_edges()` still
projects the exact `Left → Base` and `Right → Base` edges. These checked JVM
names are accepted by `dotty_classfile::bytecode::ClassHierarchy`; the source
proof itself remains in `dotty-jvm` and does not pass `SymbolId` values into
the class-file crate. The returned common ancestor is evidence only: it does
not change a typed `If`, method signature, or code-generation policy.

Keep these cases outside the accepted subset until separately specified and
tested:

| Case | Required conservative behavior |
| --- | --- |
| Unrelated roots or an Object-only common ancestor | Reject; do not infer Object as a source join. |
| Missing, unplanned, foreign, or name-colliding parent identity | Reject unless canonical identity and hierarchy proof are available. |
| Cyclic, final, inaccessible, or invalid superclass path | Reject during hierarchy validation. |
| A fourth planned class on the ancestry path | Keep outside the current bounded source hierarchy. |
| Traits/interfaces, generic or abstract parents, and external parents | Defer until their metadata and JVM representation are explicitly supported. |
| Ambiguous context, inferred union without an approved target, or incompatible declared result | Reject rather than guess a class. |
| `null`, arrays, uninitialized allocations, or `uninitializedThis` at a branch | Preserve existing unsupported/error behavior; never widen uninitialized values. |
| Incompatible stack heights or a later read of a local degraded to `Top` | Keep the class-file analyzer's existing typed error. |

The class-file layer must continue to consume JVM internal names and explicit
validated edges, never Scala `SymbolId` or `Type::Or`. The source adapter owns
canonical source hierarchy validation and any future policy selecting a
declared join. See [the multilevel inheritance audit](jvm-multilevel-inheritance.md)
and [the reference CFG contract](jvm-reference-cfg-contract.md).
