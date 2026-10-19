# JVM reference fields and constructor arguments audit

This is a characterization of the current source-to-JVM boundary for
reference-valued fields and primary-constructor arguments. It does not enable
reference field emission or reference-valued constructor-call argument
lowering. The executable frontend evidence is
`tests/fixtures/jvm/ReferenceFieldsAndConstructors.scala` and
`reference_fields_and_constructor_parameters_keep_real_source_identity` in
`tests/jvm_stateful_class_contract.rs`.

## Source and typed identity findings

The test runs the real lexer, parser, namer, and typer. It checks class
`ClassInfo` declaration scopes, field and parameter owners, source ranges,
visibility, types, and canonical `TermRef` targets.

| Scala source | Typed identity and source behavior | Current JVM path and blocker |
| --- | --- | --- |
| `private val ref: Counter = initial` | A `Field` owned by `Holder`, entered in its `ClassInfo.declarations` scope, with `Visibility::Private`; the AST retains plain `private`. Its type is `TypeRef` to canonical `Class:Counter`. The typed initializer is a `TermRef` to the constructor-owned `initial` parameter. | Stops in `dotty-jvm` field metadata: `identity::field_metadata` asks for a primitive semantic value type. `getfield`/`putfield` descriptor and lowering paths currently cover primitive field values. |
| `private[this] val own: Counter = initial` | Same semantic `Visibility::Private` and field ownership/type as `ref`; the source `VisibilitySyntax` retains the `this` qualifier. | Also a reference-field metadata/type blocker. Current field metadata additionally requires source visibility `PlainPrivate`, so the syntax distinction is not lost or silently accepted. |
| `private var current: Counter = initial` | Mutable `Field` owned by `MutableHolder`, private in the semantic model, exact `Counter` `TypeRef`, initialized from the exact constructor parameter symbol. `current.get()` is rejected by the source typer with `UnstableSelectionPrefix`; direct `current` read and assignment in `get`/`replace` type. | Reference field emission is unsupported regardless. The mutable selection failure is a typer boundary, before JVM planning or classfile verification. |
| `class Holder(initial: Counter)` | The source parameter symbol belongs to `Holder` and is private. The constructor-owned parameter is a distinct canonical `Parameter` symbol owned by `<init>`, public, and its constructor signature parameter type is a `TypeRef` to `Counter`. Both preserve the source range. | `JvmCompilationUnit::metadata` accepts the signature only when `Counter` is a planned class and preserves its exact `SymbolId`; store-only constructor metadata remains primitive-only. |
| `new Holder(c)` in `Client.construct(c: Counter)` | The typed constructor selection is a `TermRef` to `Holder.<init>`, the result is `TypeRef` to `Holder`, and the argument is a `TermRef` to the exact canonical `Client.construct` parameter. | The resolver validates its exact `LCounter;` argument against the planned caller parameter and returns read-only target metadata. Constructor-call lowering still rejects reference-valued argument instructions. |
| `def read(): Int = ref.get()` | Types successfully. The selected method `TermRef` targets the canonical `Counter.get`; its qualifier is an `Ident` whose `TermRef` targets exactly `Holder.ref`. Result type is `Int`. | Source typing success is not JVM support: lowering cannot plan or emit a reference field load, and the reference field has no supported JVM field metadata. |

`Counter`, `Holder`, `MutableHolder`, and `Client` each retain a `ClassInfo`
whose class identity matches its symbol, whose declarations scope is the scope
indexed by `SourceSemanticIndex`, and whose parent is the canonical root
`Object` type. This is the source evidence required for planning; it does not
imply that those plans currently accept reference fields.

## Narrow constructor field-initializer subset to target

The smallest safe future increment is a straight-line primary constructor
which first invokes the direct superclass constructor, then initializes a
reference field from one exact reference-valued constructor parameter:

```scala
class Holder(initial: Counter):
  private val ref: Counter = initial
```

The initializer should lower to `aload <constructor-parameter-slot>` followed
by `aload_0; swap; putfield Holder.ref:LCounter;` (or an equivalent verified
stack order). The JVM adapter must resolve the canonical constructor-owned
parameter and field symbols, exact class identity, owner, and descriptor. The
field write must not read the field first, use `this` as the value, call a
method, allocate an object, branch, or let `this` escape. The field is
initialized exactly once by that assignment. First-stage support should be
private immutable fields with plain `private`, matching existing metadata
policy; `private[this]`, mutable fields, and reference reads/writes remain
separately gated until their visibility and access semantics are explicitly
planned.

The whole-class emitter currently puts `Object.<init>()V` before the planned
field initializer fragments (`dotty-jvm/src/emission.rs`). Thus the bytecode
point where constructor `this` becomes initialized is the normal completion of
that `invokespecial java/lang/Object.<init>()V`. The classfile CFG model does
not currently represent or verify that `UninitializedThis` transition:
constructor flow is explicitly outside its supported state model
(`dotty-classfile/src/bytecode/stack_map_frames.rs`). A branched `<init>` must
remain rejected until a constructor-aware CFG transfer validates the legal
first `this` or direct-super call and the restricted pre-super `putfield`
exception. Do not treat constructor local 0 as an ordinary initialized
reference before the transition.

The current constructor initializer planner accepts a bounded expression
subset over primitive values and fields already initialized in source order.
Reference RHS support must preserve this ordering and reject forward/self-field
reads. It must also reject `this` escape, arbitrary `new` expressions in field
initializers, branches in `<init>`, and calls until their initialization and
exception behavior have a verifier-backed plan. A failed lowering must leave
semantic and plan state valid.

## Exact bytecode and verifier contract

For a future field descriptor `LCounter;`, `getfield` and `putfield` require
resolved `CONSTANT_Fieldref` entries and parsed field descriptors. In this
compiler's bounded subset:

- `getfield Holder.ref:LCounter;` consumes only an initialized exact
  `Object(Holder)` receiver and pushes exact `Object(Counter)`.
- `putfield Holder.ref:LCounter;` consumes an initialized exact
  `Object(Holder)` receiver and a value of exact `Object(Counter)`.
- No check may accept a value because it is merely category `Reference`, or
  guess `Object`, a superclass, or an interface. The JVMS allows broader
  assignment compatibility in operations where the hierarchy is known; exact
  identity here is the deliberate no-hierarchy restriction.
- A constructor call `invokespecial Counter.<init>(LDep;)V` requires the
  uninitialized allocation identity for exactly `Counter`, an exact initialized
  `Object(Dep)` argument, and normal completion before the allocation aliases
  become `Object(Counter)`. A subtype, different class, missing plan, or
  name-only match is insufficient.
- For the supported constructor-field write, the receiver is exact initialized
  `Object(Holder)` after the direct-super call, and the stored value is exact
  initialized `Object(Counter)` from the constructor parameter.

Current class-aware CFG analysis already records exact identities for ordinary
reference calls and tracks allocation-site initialization for a bounded
straight-line `new; dup; arguments; invokespecial` shape. It has a single
`pending_allocation` slot and rejects a second `new` before the first
constructor completes. Therefore `new Holder(existingRef)` is the only
reference-argument shape to consider after constructor descriptor/planning
support exists. Nested `new Holder(new Counter(...))` needs nested allocation
tracking and is unsupported; `new Holder(factory())` needs an exact planned
reference-return/argument composition and is unsupported. Neither case should
be accepted through category-only linear analysis.

The class-aware linear analyzer can validate straight-line exact descriptors,
but cannot prove allocation-site alias transitions, branch joins, or
`UninitializedThis`. Control-flow-sensitive constructor support needs the CFG
analyzer. The existing constructor classfile builder rejects branched
constructors. A passing linear analysis alone is not evidence that a branched
constructor is verifier-safe.

## Pipeline ownership and retained negative cases

| Stage | Current responsibility / boundary |
| --- | --- |
| `dotty-core` | Owns canonical symbols, `ClassInfo`, source scopes, types, and AST identity. No JVM lowering policy belongs here. |
| `dotty-typer` | Accepts immutable `ref.get()`; rejects `current.get()` as `UnstableSelectionPrefix`. It does not emit bytecode. |
| `dotty-jvm` | Owns source visibility checks, primitive field metadata, constructor signatures and field-initializer plans. Extend these plans only with exact canonical identities and checked descriptors. |
| `dotty-classfile` | Owns JVM descriptors, constant-pool reference validation, exact verifier values, allocation-site tracking, and stack-map frames. It must not depend on Scala symbols. |

Keep regression coverage for wrong `Counter`/`Other` values, missing class or
member plans, wrong field or constructor owners, uninitialized references,
`null`, arrays, generic/unresolved types, nested allocations, and constructor
CFG branches. Tests must assert the stage and typed failure; do not add ignored
or expected-failure tests that imply unsupported emission works. Existing
source-pipeline rejection coverage is in
`tests/jvm_source_compilation.rs`; lower-level exact-owner, uninitialized, and
allocation checks are in `dotty-jvm` and `dotty-classfile` tests.

Related contracts: [JVM reference signatures](jvm-reference-signatures.md),
[JVM reference CFG contract](jvm-reference-cfg-contract.md), and
[JVM stateful class contract](jvm-stateful-class-contract.md).
