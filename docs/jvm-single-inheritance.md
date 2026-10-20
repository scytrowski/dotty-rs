# JVM single inheritance and virtual dispatch audit

This document audits the source-to-JVM boundary for ordinary Scala class
inheritance and records the currently emitted subset. Scala source is interpreted against the repository's pinned Scala
3.9.0 revision (`777528f19a58e794c9954a42f433373472ec57f8`). JVM rules below
refer to the Java SE 21 specification.

## Reproducible baseline

The source-to-classfile regression compiles a same-batch `Base` and
`Derived extends Base`, decodes `Derived.super_class`, checks the exact
`Base.<init>()V` constructor target, and preserves `Base`'s Object-root
constructor. In the mandatory JDK 21+ gate, `javap -c -v` inspects the child
class and `java -Xverify:all` instantiates it and invokes inherited methods.
The broader `SingleInheritanceAudit.scala` fixture remains a characterization
of override and subtype behavior. Exact direct-parent override validation now
records effective dispatch identities without copying inherited bodies into a
child plan; inherited-reference compatibility remains a later increment.

## Current pipeline and identity contracts

`ClassInfo.parents` carries projected parent types. For `Derived extends
Base`, the direct parent resolves to the canonical `Base` `SymbolId`. The
typer's nominal subtype check walks those completed parent records, so a
`Derived` argument can satisfy a `Base` formal without changing the argument's
own class identity or inserting a backend cast. Member lookup returns the
declaration symbol: an inherited selection such as `base.doubled()` retains
the method's declaring owner, `Base`.

The namer records source `override` as `SymbolFlags::OVERRIDE`. This is syntax
evidence, not proof by itself. Unit metadata checks that an override has a
planned, public, nonfinal direct-parent declaration with the exact JVM name
and descriptor, matching plain monomorphic semantic parameter/result types,
public visibility, and a concrete body. It also rejects a same-signature
public child method without `override`. Missing, unplanned, inaccessible,
mismatching, and narrowed overrides return contextual typed errors. The
existing final-method check remains conservative when the parent's final
method is not planned.

`JvmClassMetadata::methods()` continues to contain only methods declared in
that class's plan. Its read-only `dispatch_methods()` view carries effective
public method identities for the receiver: an inherited entry keeps the
parent's method symbol and declaring class, while an exact override points to
the child's distinct symbol. This metadata does not copy method bodies or
change JVM `invokevirtual` selection.

The JVM adapter accepts a canonical Object root and one same-unit edge from a
public ordinary class to a public, non-final ordinary parent whose own parent
is Object. Its checked hierarchy proof requires a public accessible no-arg
parent constructor and rejects cycles, deeper chains, external or missing
parents, and unsupported class shapes before output. Emission uses the proven
parent's canonical symbol and checked internal name. Instance-call resolution
and reference argument projection still require exact planned class identity;
they are not generalized by this increment and must not be weakened by name
comparison or by treating every reference as Object.

Keep the source and class-file identities distinct:

- Within `dotty-jvm`, use canonical `SymbolId` keys and read-only records
  derived from completed `ClassInfo` and class flags. Validate the graph
  before projection; reject cycles, final parents, missing or duplicate
  parents, deep chains, and unsupported trait or multi-parent forms. Do not
  allocate symbols, repair parent lists, or infer ancestry from names.
- At the `dotty-classfile` boundary, project each validated source symbol to
  its checked JVM internal name. Class-file analysis stays Scala-independent
  and reasons about internal names / constant-pool entries, not `SymbolId`s.

The implemented emission slice is a public, concrete, non-generic `Base
extends Object` and `Derived extends Base`, emitted together in one
compilation unit, with an accessible public no-argument `Base` constructor.
The `Derived` proof requires its own parent to be `Object`, so a third class
cannot extend it in the same unit. Keep traits, multiple or deeper
inheritance, generic and external parents, inaccessible constructors, bridges,
and other forms rejected until separately specified and tested.

## JVM contracts to preserve

The JVM class file's `super_class` denotes the direct parent. For the bounded
slice above, `Derived.class` must name `Base` there, while `Base.class` names
`java/lang/Object`. Constructor initialization must call the direct parent's
accessible `<init>()V` using `invokespecial`. The verifier begins a constructor
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
`Derived.value`. `Runner.evaluate(base: Base)` should carry a `Base` formal
descriptor; passing a typed `Derived` value is a source assignment-compatibility
check. It must not be confused with merging two possible values at a control
flow join. A hierarchy-aware merge requires a separately defined least upper
bound operation; do not reuse the one-directional argument subtype check.

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
| `def evaluate(base: Base) = base.doubled(); evaluate(new Derived())` | Typer accepts the source subtype argument, but exact-class JVM argument checks still reject it | The method descriptor can name `Base`; no cast or merge is emitted | Hierarchy-aware call-argument assignability is a separate `dotty-jvm` increment |
| Unknown/wrong parent type | If name/type resolution cannot resolve it, reject in the frontend; otherwise hierarchy validation rejects noncanonical or non-class parents | No class file should be produced | Source resolution and canonical hierarchy projection; never lower by name guess |
| Parent cycle (`A extends B`, `B extends A`) | `JvmSuperclassIssue::InheritanceCycle` | No `super_class` chain is emitted | Validated hierarchy graph in `dotty-jvm`, keyed by canonical symbols |
| Extending a `final` class | `JvmSuperclassIssue::ParentIsFinal` | JVMS forbids extending a final class | Parent flag validation in the `dotty-jvm` hierarchy check |
| Child method with the same JVM signature as a planned non-private final direct-parent method | `FinalJvmMethodOverride` before any class files are published; name-only conservative rejection if the parent method is unplanned | JVMS forbids overriding the final method | Exact JVM name-and-descriptor collision check in `dotty-jvm` |
| Exact public `override def value(): Int` for public nonfinal `Base.value(): Int` | Accepted only with distinct canonical symbols, matching `()I` descriptors and a concrete child body | Child dispatch points to the child symbol; inherited methods retain the parent owner and are not re-emitted | `JvmClassMetadata::dispatch_methods()`; exact override validation in `dotty-jvm` |
| `override` with no matching or planned direct-parent method | `InvalidJvmOverride` before any class files are published | No dispatch entry may be invented | Canonical parent declaration and plan validation in `dotty-jvm` |
| Override with incompatible parameter/result descriptor, semantic signature, or reduced visibility | `InvalidJvmOverride` before any class files are published; final methods retain `FinalJvmMethodOverride` | Same JVM name+descriptor identifies the virtual entry; access cannot be narrowed incompatibly | Exact descriptor, semantic MethodType, and visibility checks in `dotty-jvm`; bridge generation remains unsupported |
| Trait parent or multiple parents | The hierarchy validator rejects traits and unsupported parent-list shapes before output | JVM has one direct `super_class`; interfaces require distinct metadata and dispatch rules | Explicitly reject traits/multiple parents in `dotty-jvm`; later interface support is separate |
| Generic or external parent | The hierarchy validator rejects unsupported class shapes and unplanned/external parents | Erased descriptor and external hierarchy facts are unavailable to this adapter | `dotty-jvm` projection/classpath boundary; keep unsupported until erasure and validated external metadata exist |
| Missing or inaccessible parent `()V` constructor | `JvmSuperclassIssue::ParentConstructorUnavailable` | No constructor invocation or class output is emitted | Constructor metadata/access validation in `dotty-jvm`; constructor transfer in `dotty-classfile` |
| `def read(): Int = super.value()` | Real source fails earlier at `PipelineFailure::Typer(TyperError::UnsupportedExpression { expression_kind: "expression or declaration tree", .. })`; method bodies are typed before JVM emission | No class file is produced, so no invocation instruction or constant-pool owner is emitted | First add source AST/typer support with the canonical selected parent declaration; only then add the separate checked `invokespecial` lowering path in `dotty-jvm` and analyzer support in `dotty-classfile` |

The end-to-end tests guarantee the supported direct-parent constructor chain
and the earlier typer rejection for `super.method()`. Rejected hierarchy rows
must remain typed failures without partial output. Rows that describe other
future behavior are design constraints for follow-up increments and need
focused exact-diagnostic regression coverage when implemented.
