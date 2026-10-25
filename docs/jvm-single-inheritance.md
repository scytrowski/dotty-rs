# JVM single inheritance and virtual dispatch audit

This document audits the source-to-JVM boundary for ordinary Scala class
inheritance and records the currently emitted subset. Scala source is interpreted against the repository's pinned Scala
3.9.0 revision (`777528f19a58e794c9954a42f433373472ec57f8`). JVM rules below
refer to the Java SE 21 specification.

## Reproducible baseline

The source-to-classfile regression compiles `Inheritance.scala` through the
real lexer, parser, namer, typer, JVM adapter, and class-file encoder. It checks
all three classes (`Base`, `Derived`, and `Runner`), constructor chains,
overrides, inherited calls, subtype arguments and returns, conditional
dispatch, and byte-for-byte equality from two fresh frontend sessions. In the
mandatory JDK 21+ gate, `javap -c -v` inspects `Runner` and a Java harness runs
all behavior with `java -Xverify:all`. Override validation follows the
validated ancestry and records effective dispatch identities without copying
inherited bodies into child plans. Inherited calls keep the selected
declaration's owner in the JVM `Methodref`; the JVM performs dynamic override
selection at runtime.

The broader `MultilevelInheritance.scala` fixture verifies the bounded
three-class subset end to end: each class names its immediate JVM parent,
constructor parameters are forwarded one edge at a time, and ordinary
`Base.doubled()` dispatches through a `Leaf` override. Its `Runner` also passes
`Leaf` as `Base`, returns `Leaf` as `Base`, and joins `Middle` and `Leaf` in
both branch orders using the source-typed `Middle` result. Decoded classfiles,
deterministic independent sessions, `javap -c -v`, and the mandatory JDK 21+
`java -Xverify:all` harness pin this composition. The class-file major version
is 65. A fourth class on one ancestry path remains rejected; sibling
subclasses and exact expected-`Base` sibling reference joins are covered below.

`SiblingConstructors.scala` adds a four-class batch with two direct children
of `Base`. Each child has its own `(I)V` constructor and canonical `seed`
parameter; both direct-super plans forward that child's parameter to the same
`Base.<init>(I)V`. `Left.value` and `Right.value` keep distinct declaration
symbols and effective dispatch entries, while `Base.doubled` remains inherited
and invokes `Base.value` virtually. `Runner.fromLeft` and `fromRight` allocate
the exact sibling class and call the common `Base`-typed evaluator.
`Runner.choose` and `reverseChoose` return the exact `Left | Right` source
union under an explicitly declared `Base` result; both branch orders encode a
constant-pool-resolved `Object(Base)` stack-map entry without a cast. The test
checks exact class/member shapes, decoded headers, descriptors and method
references, byte-identical output from two fresh frontend sessions, and—when
JDK 21+ is available—runtime dispatch and both branch orders under
`java -Xverify:all`.

## Current pipeline and identity contracts

`ClassInfo.parents` carries projected parent types. For `Derived extends
Base`, the direct parent resolves to the canonical `Base` `SymbolId`. The
typer's nominal subtype check walks those completed parent records, so a
`Derived` argument can satisfy a `Base` formal without changing the argument's
own class identity or inserting a backend cast. Member lookup returns the
declaration symbol: an inherited selection such as `derived.doubled()` retains
the method's declaring owner, `Base`.

The namer records source `override` as `SymbolFlags::OVERRIDE`. This is syntax
evidence, not proof by itself. Unit metadata checks that an override has a
planned, public, nonfinal inherited declaration with the exact JVM name and
descriptor, matching plain monomorphic semantic parameter/result types, public
visibility, and a concrete body. It also rejects a same-signature public child
method without `override`. Missing, unplanned, inaccessible, mismatching, and
narrowed overrides return contextual typed errors. Final-method checks cover
every validated ancestor and remain conservative when a final declaration is
not planned.

Unit metadata also validates every inherited method-shaped accessor against
declared source methods and generated getter/setter entries across the full
planned ancestry. An exact JVM name-and-descriptor collision involving an
accessor returns `InheritedJvmAccessorCollision`, preserving the canonical
method symbol or source field plus getter/setter role on both sides. Equal
parameter descriptors with incompatible result descriptors are also rejected;
different parameter descriptors remain overloads. Ordinary method-to-method
overrides retain their existing rules. Accessor overrides remain unsupported,
and unplanned non-private inherited methods use a conservative same-name
rejection for generated accessors.

`JvmClassMetadata::methods()` continues to contain only methods declared in
that class's plan. Its read-only `dispatch_methods()` view carries effective
public method identities for the receiver: an inherited entry keeps the
parent's method symbol and declaring class, while an exact override points to
the child's distinct symbol. This metadata does not copy method bodies or
change JVM `invokevirtual` selection.

Generated accessor methods use a separate read-only `dispatch_accessors()`
view. Each entry retains the source `Field` symbol and a getter/setter role,
along with the receiver, declaring class, JVM name, descriptor, and flags.
Keeping these entries separate preserves `dispatch_methods()` compatibility
and leaves same-signature collisions visible without inventing source method
symbols or treating an accessor as an override. Emission now accepts the
validated Object-rooted hierarchy subset when descendants add no new accessors;
the parent keeps physical storage and getter/setter methods while descendants
retain inherited entries only in effective dispatch metadata.

The JVM adapter accepts a canonical Object root and up to two same-unit edges
between public ordinary classes, so a planned `Base → Middle → Leaf` chain is
validated without admitting a fourth class in the ancestry. Each edge is
checked independently, and each child must have a public accessible parent
constructor with a supported descriptor. Parameterized calls additionally require
the canonical source/typed parent-argument proof; accepted arguments are bounded
literals or exact child constructor parameters. It rejects cycles, a fourth
planned user class in the chain, external or missing parents, and unsupported
class shapes before output. Emission uses the proven
parent's canonical symbol and checked internal name. Instance-call resolution
accepts the receiver's exact planned class or a class on its validated
ancestry as the selected method owner. For inherited calls, the resolver
confirms the canonical method symbol is declared and planned by that owner
and appears unchanged in the receiver's effective dispatch metadata. Reference
assignability accepts a `Leaf` value where `Middle` or `Base` is required only
when the same validated canonical ancestry proof establishes that assignment
within two user-class edges. The bounded conversion applies to method and
constructor arguments, immutable local initializers, reference field writes,
and returns; it retains each value's
exact source/JVM class identity and emits no cast. It does not enable mutable
reference locals, arbitrary `Object` upcasts, or implicit reference
conversions without the unit proof. Exact planned class references and
transitive assignments may be projected through the complete unit hierarchy
proof. Inherited method lookup follows the
validated ancestry and retains the canonical declaring symbol. These checks do
not use name-based class identity.

Keep the source and class-file identities distinct:

- Within `dotty-jvm`, use canonical `SymbolId` keys and read-only records
  derived from completed `ClassInfo` and class flags. Validate the graph
  before projection; reject cycles, final parents, missing or duplicate
  parents, deep chains, and unsupported trait or multi-parent forms. Do not
  allocate symbols, repair parent lists, or infer ancestry from names.
- At the `dotty-classfile` boundary, project each validated source symbol to
  its checked JVM internal name. Class-file analysis stays Scala-independent
  and reasons about internal names / constant-pool entries, not `SymbolId`s.

The implemented emission slice is a public, concrete, non-generic same-unit
chain of up to three classes: `Base extends Object`, `Middle extends Base`, and
`Leaf extends Middle`. Each class has an accessible public parent constructor
and a checked argument list. The no-argument case uses `()V`; parameterized
construction uses the actual parent descriptor, including `(I)V` in
`SuperCalls.scala` and `MultilevelInheritance.scala`. Keep traits, multiple or
four-or-more-class inheritance, generic and
external parents, inaccessible constructors, bridges, and other forms
rejected until separately specified and tested.

## JVM contracts to preserve

The JVM class file's `super_class` denotes the direct parent. For the bounded
slice above, `Derived.class` must name `Base` there, while `Base.class` names
`java/lang/Object`. Constructor initialization must call the direct parent's
accessible `<init>` using `invokespecial` with its exact descriptor and ordered
arguments. The verifier begins a constructor
with `UninitializedThis`; normal completion of that legal superclass call
transitions it to initialized `this`. No constructor branch may cross this
transition unless the analyzer explicitly models it. See [JVMS 21 §4.1 and
§4.10](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html) and
[§6.5 `invokespecial`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.invokespecial).

For an ordinary virtual call, the symbolic method reference owner comes from
the selected declaration, not from the static receiver type. Thus
`Base.doubled` calling `Base.value` must reference owner `Base` with the exact
method name and descriptor and use `invokevirtual`. At runtime the JVM resolves
that virtual call against the receiver class, so `new Derived()` dispatches to
`Derived.value`. `Runner.evaluate(base: Base)` must carry a `Base` formal
descriptor; passing a typed `Leaf` value, initializing an immutable `Base`
local, or returning a `Leaf` value from a method declared `Base` is accepted
only through up to two validated parent edges in the same unit. The JVM
verifier receives that same hierarchy proof, so no `checkcast` is needed. This
must not be confused
with merging two possible values at a control-flow join. A hierarchy-aware
merge requires a separately defined least upper-bound operation; do not reuse
the one-directional assignability check.

This initial slice does not emit bridge methods. A covariant result or any
override whose JVM name/descriptor does not match the inherited entry point
must remain unsupported until erasure and bridge generation are specified.
Access checks must use the declaring symbol's visibility and the actual
inheritance relationship, not just the receiver's class.

## Characterization and rejection matrix

“Frontend” means the first lexer/parser/namer/typer stage that can diagnose the
shape. The matrix distinguishes emitted hierarchy shapes from related source
semantics that remain unsupported. Backend failures are typed and must not
publish partial output.

| Source shape | Frontend / current stage and diagnostic | JVM metadata and verifier state | Next explicit blocker / owner |
| --- | --- | --- | --- |
| `class Base { def value(): Int = 20; def doubled(): Int = value() }` | Frontend and root-class emission support the shape | `Base.super_class = java/lang/Object`; `doubled` uses `invokevirtual Base.value:()I`; ordinary method receiver is initialized `Base` | Root-class source/JVM integration test |
| `class Derived extends Base {}` | Source pipeline emits the batch when `Base` is a planned public no-arg class with Object as its parent | `Derived.super_class = Base`; constructor invokes `Base.<init>()V` exactly once before ordinary use | Covered by source-backed classfile regression and JDK verifier gate |
| `def evaluate(base: Base) = base.doubled(); evaluate(new Leaf())` | Accepted when the validated `Leaf → Middle → Base` proof is present in the emission unit | `evaluate` keeps descriptor `Base`; `invokevirtual Base.doubled` accepts the `Base` receiver, and no cast or merge is emitted | Other subtype depths and external hierarchy facts remain unsupported |
| `Inheritance.scala` end-to-end fixture | Real Scala frontend compiles `Base`, `Derived`, and `Runner`; two fresh sessions emit byte-identical batches | Decoded class metadata and method references are asserted; `javap -c -v` and a Java 21+ `-Xverify:all` harness validate descriptors, frames, constructors, and dynamic dispatch | Mandatory JDK 21+ CI gate; local JDK is optional |
| `SiblingConstructors.scala` batch | Real source compiles `Base`, `Left`, `Right`, and `Runner`; child constructors forward distinct canonical parameters to the same parent constructor, overrides retain distinct symbols, and `choose`/`reverseChoose` preserve their exact union under a declared `Base` result | All classes and exact member shapes are decoded; constructors use `invokespecial Base.<init>(I)V`, overrides use `invokespecial Base.value()I`, and `Base.doubled`/Runner use virtual calls; both join orders have CP-resolved `Object(Base)` frames and no cast; two fresh frontend sessions emit identical bytes; JDK 21+ checks runtime identity and dispatch | Exact context-proven sibling joins are supported; inferred, mismatched, nested, and `Object`-only joins remain unsupported; see [the sibling reference join audit](jvm-sibling-reference-joins.md) |
| Unknown/wrong parent type | If name/type resolution cannot resolve it, reject in the frontend; otherwise hierarchy validation rejects noncanonical or non-class parents | No class file should be produced | Source resolution and canonical hierarchy projection; never lower by name guess |
| Parent cycle (`A extends B`, `B extends A`) | `JvmSuperclassIssue::InheritanceCycle` | No `super_class` chain is emitted | Validated hierarchy graph in `dotty-jvm`, keyed by canonical symbols |
| Extending a `final` class | `JvmSuperclassIssue::ParentIsFinal` | JVMS forbids extending a final class | Parent flag validation in the `dotty-jvm` hierarchy check |
| Child method with the same JVM signature as a planned non-private final ancestor method | `FinalJvmMethodOverride` before any class files are published; name-only conservative rejection if the ancestor method is unplanned | JVMS forbids overriding the final method | Exact JVM name-and-descriptor collision check across validated ancestors in `dotty-jvm` |
| Exact public `override def value(): Int` for public nonfinal `Base.value(): Int` | Accepted only with distinct canonical symbols, matching `()I` descriptors and a concrete child body | Child dispatch points to the child symbol; inherited methods retain the parent owner and are not re-emitted | `JvmClassMetadata::dispatch_methods()`; exact override validation in `dotty-jvm` |
| `Leaf` receiver selects inherited `Base.doubled()` | Accepted only with the complete validated ancestry and exact planned `Base.doubled` symbol | Emits `invokevirtual Base.doubled:()I`; `Base.doubled` calls `invokevirtual Base.value:()I`, so a `Leaf` runtime receiver reaches its nearest override | `JvmClassMetadata::dispatch_methods()` plus the explicit class-file verification hierarchy |
| `override` with no matching or planned inherited method | `InvalidJvmOverride` before any class files are published | No dispatch entry may be invented | Canonical ancestor declaration and plan validation in `dotty-jvm` |
| Override with incompatible parameter/result descriptor, semantic signature, or reduced visibility | `InvalidJvmOverride` before any class files are published; final methods retain `FinalJvmMethodOverride` | Same JVM name+descriptor identifies the virtual entry; access cannot be narrowed incompatibly | Exact descriptor, semantic MethodType, and visibility checks in `dotty-jvm`; bridge generation remains unsupported |
| Trait parent or multiple parents | The hierarchy validator rejects traits and unsupported parent-list shapes before output | JVM has one direct `super_class`; interfaces require distinct metadata and dispatch rules | Explicitly reject traits/multiple parents in `dotty-jvm`; later interface support is separate |
| Generic or external parent | The hierarchy validator rejects unsupported class shapes and unplanned/external parents | Erased descriptor and external hierarchy facts are unavailable to this adapter | `dotty-jvm` projection/classpath boundary; keep unsupported until erasure and validated external metadata exist |
| Missing or inaccessible parent `()V` constructor | `JvmSuperclassIssue::ParentConstructorUnavailable` | No constructor invocation or class output is emitted | Constructor metadata/access validation in `dotty-jvm`; constructor transfer in `dotty-classfile` |
| `def read(): Int = super.value()` | Supported for an explicit unqualified Super(This) selecting a planned public concrete method reachable through the direct parent | Emits checked `invokespecial` with the immediate parent as symbolic owner; ordinary calls keep `invokevirtual` | See [super-call contract](jvm-super-calls.md) and `SuperCalls.scala` for combined parameterized construction and runtime verification; multilevel inheritance is characterized in the [multilevel inheritance audit](jvm-multilevel-inheritance.md) |

The end-to-end tests guarantee the supported three-class ancestry path,
including sibling construction and shared-base runtime dispatch, transitive
method lookup, direct and two-edge subtype
argument/return paths, and explicit `super.method()` dispatch through the
immediate parent. Rejected hierarchy rows must remain typed
failures without partial output. Rows that describe other future behavior are
design constraints for follow-up increments and need focused exact-diagnostic
regression coverage when implemented.

## Parameterized constructor and explicit super integration

`SuperCalls.scala` combines `Base(seed: Int)`, `Derived(start: Int) extends
Base(start)`, and Runner in a single canonical frontend batch. Base and Derived
have only `(I)V` constructors. Derived forwards `start` before any child
initialization; its explicit calls name Base's canonical methods and emit
`invokespecial`. Base.doubled and Runner.evaluate retain `invokevirtual`, so
Derived's override remains active for ordinary dynamic dispatch.

The test decodes every class and compares ordered bytes from two fresh sessions.
The JDK 21 harness checks constructor initialization and mutation plus the exact
20/22/10/11/12 results, zero, and negative seeds with `java -Xverify:all`.
An additional branching client validates exact frame reference identities and
branch target PCs. Existing Inheritance, ReferenceHolder, and ReferenceObjectApis
fixtures and their deterministic/bytecode/runtime checks remain unchanged.

The validated hierarchy permits at most three planned user classes on one
ancestry path; separate sibling branches may be included in the same batch.
Exact sibling reference joins with a checked expected parent are supported as
described above. General inferred hierarchy-aware reference joins, nested
allocation arguments, arbitrary effectful pre-super evaluation,
branched constructors, qualified/trait super, and Scala ABI/TASTy emission
remain deferred. Invalid
source/plan/verification inputs fail
before the source batch publishes classfiles; later OS failures retain the
existing best-effort rollback contract.
