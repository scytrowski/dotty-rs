# JVM multilevel inheritance and reference joins audit

This is a characterization of the Scala 3.9.0 source pipeline and Java SE 21
class-file rules. The current JVM backend accepts the bounded three-class
`Base → Middle → Leaf` chain and transitive inherited method dispatch; it does
not enable arbitrary-depth inheritance or reference class joins. The source
test `multilevel_source_audit_validates_the_three_level_hierarchy_proofs`
drives the real lexer, parser, namer, typer, and JVM adapter from one source
unit and checks canonical symbols and per-edge proofs. Separate source-to-JVM
tests verify inherited calls and runtime dispatch. Another test records how a
sibling join is typed and where the current JVM policy rejects it.

## Source evidence and current result

The source program follows the issue's `Base -> Middle -> Leaf` shape, plus
`Runner`. The test asserts canonical `SymbolId` relationships rather than
looking up a reconstructed hierarchy by printed class names:

| Source declaration | Canonical parent identity | Constructor / method evidence |
| --- | --- | --- |
| `Base` | `Definitions::object_class` | Primary constructor formal `seed` is owned by Base's constructor symbol. `doubled` contains two calls whose selected target is Base's canonical `value` method symbol. |
| `Middle` | the exact Base `SymbolId` | Its constructor formal belongs to its canonical constructor. The typed direct-parent application selects a constructor owned by Base. `super.value()` selects Base's canonical method symbol. |
| `Leaf` | the exact Middle `SymbolId` | Its constructor formal belongs to its canonical constructor. The typed direct-parent application selects a constructor owned by Middle. Both `super.value()` selections in `value` and `parentValue` target Middle's canonical `value` method symbol. |
| `Runner` | `Definitions::object_class` | The real typed method body roots are `TreeId<Typed>` values in the same compilation's typed arena. `choose` and `chooseReverse` retain the `Middle` type; `chosenValue` selects Middle's canonical `value` method symbol. |

The test does not hard-code numeric IDs: `SymbolId`, constructor parameter
ownership, selected method owners, and parent edges are compared within the
same frontend session. This preserves identity evidence without treating
allocation numbers as a cross-session contract.

For both source orders below, the canonical typed root remains `If` and its
`TypeId` resolves to the exact `Middle` `SymbolId`:

```scala
if flag then new Middle(seed) else new Leaf(seed)
if flag then new Leaf(seed) else new Middle(seed)
```

This follows from the typer's bounded, transitive nominal subtype relation:
`Leaf <: Middle`, so `join_expression_types` selects Middle whichever branch
comes first. The test's `ClassHierarchy::from_edges` control proves the
Scala-independent class-file graph also accepts the explicit edges
`Leaf -> Middle -> Base -> java/lang/Object` and reports Leaf assignable to
both Middle and Base.

The first rejection of the complete four-class source unit is during JVM
The three-class source program passes JVM codegen validation. Its exact
ancestor method symbols remain distinct from each class's declared methods;
effective dispatch metadata points to the nearest override without copying
method bodies. Ordinary calls use the canonical declaring class as the
`Methodref` owner, while explicit `super` calls use the immediate parent as the
symbolic owner so `ACC_SUPER` applies the JVMS selection rule. The
source-to-JVM regression runs the emitted classes under `java -Xverify:all`.

The source test separately uses `Root`, `Left extends Root`, and
`Right extends Root`. The typed `If` result is `Type::Or` whose arms refer to
the exact Left and Right symbols. Its declared result is Root, but lowering
currently stops with `JvmCodegenError::ReferenceTypeMismatch` because
`merge_if_flows` requires equal `JvmType::Reference` values. This is a backend
policy boundary, not a claim that the JVM has no common supertype. The same
exact-class check applies to the issue's Middle/Leaf branch after a valid unit
hierarchy could be supplied.

## Projection and dispatch boundaries

| Concern | Current behavior | Follow-up requirement |
| --- | --- | --- |
| Unit hierarchy | `identity::unit_hierarchy` verifies canonical one-parent `ClassInfo`, planned/public parent, cycles, `final`, constructor access, and the bounded three-class source chain. | Keep the proof keyed by canonical symbols and bounded; validate every edge independently. |
| Reference projection | Unit-aware `reference::project_type_with_metadata` consumes the validated hierarchy proof. Standalone projection still rejects class references it cannot prove locally. | Reuse the unit's ancestry proof; do not create a string- or name-driven second ancestor registry. |
| Assignability | `reference::is_validated_subtype` accepts equality and up to two canonical parent edges from the validated unit graph. Method/constructor arguments, results, immutable locals, and approved field values retain their declared JVM descriptors without casts. | Preserve the two-edge bound and exact `SymbolId` identity; do not widen this relation into a branch join or external/classpath proof. |
| Typed joins | `join_expression_types` returns Middle for Middle/Leaf in either order. Unrelated sibling classes become `Type::Or`; `Or` remains a semantic union and is not silently projected as one JVM class. | Keep nominal join policy explicit: support only the planned ancestor relation; reject unrelated/sibling `Or` values unless a separate issue defines a safe policy. |
| If lowering | `merge_if_flows` requires equal branch flows. Different reference classes produce `ReferenceTypeMismatch`; it does not widen to Object or a guessed ancestor. | Add a commutative, deterministic reference stack join using only a supplied validated hierarchy proof. Preserve primitive and uninitialized-state errors. |
| Override and dispatch | Effective metadata walks the validated ancestry. Override checks resolve the nearest inherited declaration, and final-method checks cover every ancestor, including unplanned final declarations. | Preserve canonical method identities and verify missing-override and final-ancestor errors without partial output. |
| Direct `super` calls | Typer lookup starts at the immediate parent and can select a declaration inherited through it. Lowering retains the selected symbol but emits `invokespecial` with the immediate parent as the symbolic owner. | Preserve direct-parent selection start and verify the JVMS `ACC_SUPER` behavior. |
| Ordinary virtual calls | `Base.doubled` selects Base.value twice. `Leaf` can select `Base.doubled` or another inherited method while keeping Base as the `Methodref` owner. The JVM receiver dispatches to the nearest override. | Preserve the canonical declaring owner in constant-pool references and verify runtime dispatch. |
| Parameterized constructor chain | The source adapter retains each constructor's canonical formal symbols and the typed parent-constructor application. The intended calls are `Leaf.<init>(I)V -> Middle.<init>(I)V -> Base.<init>(I)V -> Object.<init>()V`. | Validate and emit one direct-super constructor call per class, in order, with each call's exact parent owner, descriptor, access, and initialized/uninitialized state. No `this` escape or branch through constructor initialization. |

### Class-file hierarchy versus CFG join

`dotty-classfile::bytecode::ClassHierarchy::from_edges` accepts an explicit
single-parent graph, rejects duplicate/conflicting edges and cycles, and
validates paths up to its current eight-edge bound. `is_assignable` follows
those edges transitively. It is intentionally Scala-independent and operates
on validated JVM internal class names. This existing object is the right proof
to extend and share with CFG analysis; `dotty-jvm` should first validate
canonical source symbols and then project those exact edges to JVM names.

The class-file CFG analyzer uses the hierarchy for typed transfers, including
method arguments, fields, and returns. Its block merge is a different rule:
operand stacks must have the same height and exactly equal values at every
position. `Object(Left)` and `Object(Right)` fail with
`AnalysisError::IncompatibleStackValue` even when both are assignable to Root.
Conflicting locals degrade to `Top`, and a later load fails as an unusable
local. Keep that conservative local rule in the first stack-join increment;
never promote an uninitialized allocation or `uninitializedThis` to an
initialized ancestor. Branching constructors remain unsupported.

## JVMS 21 contract

For class-file major version 65, `StackMapTable` frames describe verification
types for local variables and operand-stack entries at bytecode offsets. An
`Object_variable_info` names its class through a `CONSTANT_Class_info`; it is
not a source-language type expression and does not request a cast. See [JVMS
21 §4.7.4](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.7.4)
and [§4.10.1](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.10.1).

The JVMS type-inference algorithm specifies merging two non-array reference
stack values to their first common supertype, which exists because Object is a
supertype of classes, interfaces, and arrays. That rule is stated in
[§4.10.2.2](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.10.2.2)
for inference. It must not be misread as the JVM choosing a source-level
conditional type or rewriting bytecode. For this explicitly validated chain,
`Object(Middle) ⊔ Object(Leaf) = Object(Middle)`. A classfile producer must
emit a compatible stack-map type and verify all incoming values against it.
The compiler's policy may continue to reject unrelated and same-base sibling
joins even though the JVMS has a common supertype (at least Object); that is a
deliberate scope boundary, not a JVM limitation.

At a conditional join, both paths must arrive with compatible stack heights
and types for the emitted frame. No reference cast is needed for the
Middle/Leaf join: Middle is already the verifier type. A future CFG change
must preserve exact allocation-site identities and must not merge
initialized references with `Uninitialized(offset)` or
`UninitializedThis`. For locals, this audit recommends leaving the current
`Top` degradation unchanged until a separate source-local policy is specified.

Constructor and super-call bytecode have distinct semantics. Each constructor
must invoke the direct superclass constructor before using initialized
`this`. For a non-constructor superclass method call, `invokespecial` uses the
direct-parent selection rules; ordinary `invokevirtual` continues to select
according to the runtime class. See [JVMS 21 §6 `invokespecial`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-invokespecial)
and [§6 `invokevirtual`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-invokevirtual).

## Negative audit matrix

The current hierarchy validator or established conservative policy must keep
these cases rejected. Only the three-level source chain, reversed typed join,
and unrelated typed `Or` plus exact backend rejection are executed by the new
characterization tests; the other rows below are constraints for the focused
follow-up tests, not newly claimed test results.

| Case | Audit boundary |
| --- | --- |
| Cycles | Reject during canonical hierarchy graph validation. |
| `final Middle` or another final ancestor | Reject a child edge; inherited final methods are checked transitively before emission. |
| Unplanned or nonpublic parent | Reject before emission; no name-based fallback or partial output. |
| A fourth hierarchy level | Keep outside this bounded implementation; enforce the selected explicit depth bound. |
| Traits/interfaces or multiple parents | Keep separate from single-superclass metadata and dispatch. |
| Generic, abstract, or external parent | Reject until erasure, abstract dispatch, or validated external hierarchy metadata is explicitly supported. |
| Grandparent final override | `FinalJvmMethodOverride` after walking validated ancestors, even when the direct parent does not redeclare the method. |
| Unrelated `Counter` vs `Middle`, and same-base siblings | Preserve semantic `Or` where typer creates it, but do not reinterpret it as an arbitrary JVM class join. |
| `null` and arrays | Remain outside this reference projection and join increment. |
| Invalid `super` owner | Reject unless the selected declaration is canonical and effective on the validated direct parent; the emitted `invokespecial` owner remains that direct parent. |
| Uninitialized references and constructor branches | Never widen at joins; branched constructor initialization remains unsupported. |

## Follow-up ownership and validation

Keep changes at the owning boundary:

1. `dotty-core` remains the authority for `SymbolId`, `TypeId`, typed AST and
   `ClassInfo.parents`; `dotty-typer` owns transitive source conformance and
   `If` type joins. The typer should not learn JVM names or bytecode policy.
2. `dotty-jvm` owns source-unit hierarchy validation, projections from
   canonical symbols to class-file names, inherited method/override checks,
   constructor plans and typed expression lowering. It must not mutate the
   semantic store or invent missing declarations.
3. `dotty-classfile` owns the bounded, Scala-independent name-edge proof,
   verifier flow state, stack-map types and CFG joins. It receives validated
   names and method descriptors, never Scala symbols or semantic `Or` types.
4. Source-backed tests should separately assert canonical parent and member
   identities, direct-super owners, constructor sequence, both branch orders,
   stack joins, locals degrading to `Top`, and all negative cases. The final
   executable fixture must use Java 21+ `java -Xverify:all` and inspect
   `javap -c -v`; do not treat a successful Rust codegen test as JVM execution.

The exact-source audit and current rejection are covered in
`tests/jvm_source_compilation.rs`. Existing two-class inheritance, explicit
super-call, reference-holder, and classfile CFG tests remain the regression
baseline. See also [single inheritance](jvm-single-inheritance.md),
[super calls](jvm-super-calls.md), and the
[reference CFG contract](jvm-reference-cfg-contract.md).
