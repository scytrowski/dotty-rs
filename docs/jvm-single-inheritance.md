# JVM single inheritance and virtual dispatch audit

This document audits the source-to-JVM boundary for ordinary Scala class
inheritance. It records the current contract; it does not enable inheritance
emission. Scala source is interpreted against the repository's pinned Scala
3.9.0 revision (`777528f19a58e794c9954a42f433373472ec57f8`). JVM rules below
refer to the Java SE 21 specification.

## Reproducible baseline

`tests/fixtures/jvm/SingleInheritanceAudit.scala` is the small end-to-end
fixture. It contains `Base.value`, `Base.doubled`, an override in `Derived`, a
method taking `Base`, and calls with `new Base()` and `new Derived()`. The
typer unit test confirms the source frontend preserves `Derived`'s direct
parent as the canonical `Base` symbol, retains the `OVERRIDE` source flag on
the declared override, keeps `Base.value` owned by `Base` when typing the call
inside `Base.doubled`, and accepts `Derived` where `Base` is expected.

The end-to-end test runs lexer → parser → namer → typer → JVM codegen. Its
current result is specifically
`PipelineFailure::Codegen(JvmCodegenError::UnsupportedJvmSuperclass { .. })`
while emitting `Derived`; no `.class` file is published. Thus the typer
contracts are present, but neither execution result (20 for `Base`, 42 for
`Derived`) is currently produced by this backend. A passing audit test means
that this failure boundary remains explicit and stable, not that the fixture
runs.

## Current pipeline and identity contracts

`ClassInfo.parents` carries projected parent types. For `Derived extends
Base`, the direct parent resolves to the canonical `Base` `SymbolId`. The
typer's nominal subtype check walks those completed parent records, so a
`Derived` argument can satisfy a `Base` formal without changing the argument's
own class identity or inserting a backend cast. Member lookup returns the
declaration symbol: an inherited selection such as `base.doubled()` retains
the method's declaring owner, `Base`.

The namer records source `override` as `SymbolFlags::OVERRIDE`. This is only
syntax evidence: the current source namer/typer does not validate that a
matching parent member exists or that the override's signature and
visibility are compatible. Do not treat that flag as a validated override
relationship.

The JVM adapter currently accepts only a class whose sole parent is the
canonical `Object` definition. A source subclass is rejected with
`UnsupportedJvmSuperclass` before class bytes are published. Its instance-call
resolver also requires the selected method's owner to equal the receiver
class, and reference argument projection requires exact planned class
identity. Those checks are not a hierarchy model and must not be weakened by
name comparison or by treating every reference as `Object`.

For a future validated hierarchy, keep two identities distinct:

- Within `dotty-jvm`, use canonical `SymbolId` keys and read-only records
  derived from completed `ClassInfo` and class flags. Validate the graph
  before projection; reject cycles, final parents, missing or duplicate
  parents, and unsupported trait or multi-parent forms. Do not allocate
  symbols, repair parent lists, or infer ancestry from names.
- At the `dotty-classfile` boundary, project each validated source symbol to
  its checked JVM internal name. Class-file analysis stays Scala-independent
  and reasons about internal names / constant-pool entries, not `SymbolId`s.

The first future implementation slice should be only a public, concrete,
non-generic `Base extends Object` and `Derived extends Base`, emitted together
in one compilation unit, with public compatible methods and an accessible
no-argument `Base` constructor. Keep traits, multiple or chained inheritance,
generic and external parents, inaccessible constructors, bridges, and other
forms rejected until separately specified and tested.

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
shape. Any resolved class with a non-`Object` parent that reaches current JVM
emission is rejected at `Codegen` with
`UnsupportedJvmSuperclass { class, parent }`; the audit fixture locks this
exact baseline. Rows whose source validity is not yet checked by the frontend
are intentionally still blocked at that same backend gate. Constructor and
call-specific diagnostics below describe the next required gate after a
future hierarchy implementation, not behavior already implemented.

| Source shape | Frontend / current stage and diagnostic | JVM metadata and verifier state | Next explicit blocker / owner |
| --- | --- | --- | --- |
| `class Base { def value(): Int = 20; def doubled(): Int = value() }` | Frontend succeeds; current Object-only class emission is supported for the shape | `Base.super_class = java/lang/Object`; `doubled` uses `invokevirtual Base.value:()I`; ordinary method receiver is initialized `Base` | Baseline source/JVM integration test |
| `class Derived extends Base { override def value(): Int = 21 }` | Typer preserves canonical parent and override syntax flag; JVM codegen reports `UnsupportedJvmSuperclass` | Intended `Derived.super_class = Base`; constructor begins with `UninitializedThis` and must invoke `Base.<init>()V` via `invokespecial` before ordinary use | Hierarchy validation and constructor lowering in `dotty-jvm`; direct-super verification in `dotty-classfile` |
| `def evaluate(base: Base) = base.doubled(); evaluate(new Derived())` | Typer accepts the assignment-compatible argument; class emission is still blocked by `UnsupportedJvmSuperclass` | Formal descriptor remains `(LBase;)I`; `Derived` argument retains identity `Derived`, with no source-level cast | Hierarchy-aware call-argument assignability in `dotty-jvm` and verifier assignability in `dotty-classfile` |
| Unknown/wrong parent type | If name/type resolution cannot resolve it, reject in the frontend with its unresolved-type diagnostic; never lower by name guess | No class file should be produced | Source resolution in `dotty-typer`; hierarchy projection must require a completed canonical class symbol |
| Parent cycle (`A extends B`, `B extends A`) | No cycle-specific source/JVM hierarchy diagnostic exists; if parent records complete, current emission stops at `Codegen::UnsupportedJvmSuperclass` | No `super_class` chain may be emitted | Validated hierarchy graph in `dotty-jvm`, keyed by canonical symbols, must reject the cycle before projection |
| Extending a `final` class | Current override/final conformance is not established by `OVERRIDE` flags; when the subclass reaches emission, current gate is `UnsupportedJvmSuperclass` | JVMS forbids a class from directly or indirectly extending a final class | Parent flag validation in the future `dotty-jvm` hierarchy check |
| `override` with no matching parent method | The namer records `OVERRIDE` syntax but does not prove an overridden declaration; current non-Object parent still reaches `UnsupportedJvmSuperclass` | No dispatch entry may be invented | Override conformance in the source typer/namer contract, before JVM projection |
| Override with incompatible parameter/result descriptor or reduced visibility | No validated override relation is recorded; current non-Object parent is rejected at `Codegen::UnsupportedJvmSuperclass` | Same JVM name+descriptor must identify the virtual entry; access cannot be narrowed incompatibly | Source override checking plus JVM descriptor/visibility validation; bridge generation remains unsupported |
| Trait parent or multiple parents | Parser/typer preserve source parents (traits are represented separately); current JVM emission rejects the non-Object parent list with `Codegen::UnsupportedJvmSuperclass` | JVM has one direct `super_class`; interfaces require distinct interface metadata and dispatch rules | Explicitly reject traits/multiple parents in the first hierarchy validator; later interface support is separate |
| Generic or external parent | Source projection may retain type arguments or external symbols, but current JVM source emission requires exact planned local classes and Object-only ancestry | Erased descriptor and external hierarchy facts are unavailable to this adapter | `dotty-jvm` projection/classpath boundary; keep unsupported until erasure and validated external metadata exist |
| Missing or inaccessible parent `()V` constructor | Current backend rejects the subclass earlier at `Codegen::UnsupportedJvmSuperclass`, before proving constructor accessibility | `invokespecial Base.<init>()V` is invalid if absent or inaccessible; `UninitializedThis` stays uninitialized on failure | Constructor metadata/access validation in `dotty-jvm`; constructor transfer in `dotty-classfile` |
| `def read(): Int = super.value()` | Real source fails earlier at `PipelineFailure::Typer(TyperError::UnsupportedExpression { expression_kind: "expression or declaration tree", .. })`; method bodies are typed before JVM emission | No class file is produced, so no invocation instruction or constant-pool owner is emitted | First add source AST/typer support with the canonical selected parent declaration; only then add the separate checked `invokespecial` lowering path in `dotty-jvm` and analyzer support in `dotty-classfile` |

The end-to-end tests guarantee the baseline superclass rejection and the
earlier typer rejection for `super.method()`, both without partial output.
Rows that name future validation gates are design constraints for follow-up
increments and must gain a focused exact-diagnostic regression test when that
gate is implemented.
