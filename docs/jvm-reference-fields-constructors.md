# JVM reference fields and constructor arguments audit

This characterizes the bounded source-to-JVM support for reference-valued
fields and primary-constructor arguments. The executable coverage combines
`tests/fixtures/jvm/ReferenceFieldsAndConstructors.scala` and
`reference_fields_and_constructor_parameters_keep_real_source_identity` in
`tests/jvm_stateful_class_contract.rs` with the end-to-end
`tests/fixtures/jvm/ReferenceHolder.scala` source fixture.

## Source and typed identity findings

The test runs the real lexer, parser, namer, and typer. It checks class
`ClassInfo` declaration scopes, field and parameter owners, source ranges,
visibility, types, and canonical `TermRef` targets.

| Scala source | Typed identity and source behavior | Current JVM path and blocker |
| --- | --- | --- |
| `private val ref: Counter = initial` | A `Field` owned by `Holder`, entered in its `ClassInfo.declarations` scope, with `Visibility::Private`; the AST retains plain `private`. Its type is `TypeRef` to canonical `Class:Counter`. The typed initializer is a `TermRef` to the constructor-owned `initial` parameter. | Emits a private final `LCounter;` field initialized from the exact constructor parameter after the direct superclass call. |
| `private[this] val own: Counter = initial` | Same semantic `Visibility::Private` and field ownership/type as `ref`; the source `VisibilitySyntax` retains the `this` qualifier. | Also a reference-field metadata/type blocker. Current field metadata additionally requires source visibility `PlainPrivate`, so the syntax distinction is not lost or silently accepted. |
| `private var current: Counter = initial` | Mutable `Field` owned by `Holder`, private in the semantic model, exact `Counter` `TypeRef`, initialized from the exact constructor parameter symbol. Direct `current.get()` is rejected by the source typer with `UnstableSelectionPrefix`; direct reads and assignment in `value`/`replace` type. | Emits private mutable storage with exact `getfield`/`putfield` descriptors. A getter call result such as `value().get()` is supported; the mutable field itself is not a stable call prefix. |
| `class Holder(initial: Counter)` | The source parameter symbol belongs to `Holder` and is private. The constructor-owned parameter is a distinct canonical `Parameter` symbol owned by `<init>`, public, and its constructor signature parameter type is a `TypeRef` to `Counter`. Both preserve the source range. | Planned source classes accept the exact reference descriptor and retain its canonical `SymbolId` through constructor metadata. |
| `new Holder(c)` in `Client.wrap(c: Counter)` | The typed constructor selection is a `TermRef` to `Holder.<init>`, the result is `TypeRef` to `Holder`, and the argument is a `TermRef` to the exact canonical `Client.wrap` parameter. | Emits the exact `(LCounter;)V` constructor call. |
| `def read(): Int = ref.get()` | Types successfully. The selected method `TermRef` targets the canonical `Counter.get`; its qualifier is an `Ident` whose `TermRef` targets exactly `Holder.ref`. Result type is `Int`. | Supported when the exact field and target class are planned in the same unit; lowering emits the checked field read and virtual call. |
| `private val inner: Counter = new Counter(seed)` | Types successfully as a constructor application using the exact constructor-owned `seed` parameter. | `UnsupportedJvmReferenceFieldInitializer`; reference field initializers currently accept only the bounded constructor-parameter copy form. |

`Counter`, `Holder`, `MutableHolder`, and `Client` each retain a `ClassInfo`
whose class identity matches its symbol, whose declarations scope is the scope
indexed by `SourceSemanticIndex`, and whose parent is the canonical root
`Object` type. This is the source evidence required for exact
reference-field/constructor planning.

`tests/fixtures/jvm/ReferenceHolder.scala` is a separate real-source backend
fixture for the enabled exact class subset. It verifies immutable and mutable
reference field metadata, constructor arguments, and calls such as
`ref.get()` and `value().get()`. A direct application result whose type is a top-level class
declared in the current source batch may be a stable selection prefix. This
does not make a mutable reference field stable: `current.get()` remains an
`UnstableSelectionPrefix` negative case. Branch results and loop frames are
checked against exact `Object(Counter)` and `Object(Holder)` stack-map entries.

## Supported constructor field-initializer subset

The enabled source subset is a straight-line primary constructor which first
invokes the direct superclass constructor, then initializes a reference field
from one exact reference-valued constructor parameter:

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
initialized exactly once by that assignment. Plain `private` reference fields,
including mutable fields, can be planned and emitted when the constructor
initializer is this exact parameter reference. `private[this]`, arbitrary
initializer expressions, `this` escape, and unplanned reference values remain
gated until their visibility or lowering semantics are explicitly planned.

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
Reference RHS support currently allows an exact constructor parameter copy;
`new Counter(seed)` remains rejected with
`UnsupportedJvmReferenceFieldInitializer`. Future reference RHS forms must
preserve source ordering and reject forward/self-field reads. They must also
reject `this` escape and branches in `<init>` until initialization and
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
constructor completes. The supported `new Holder(existingRef)` form uses one
exact reference parameter. Nested `new Holder(new Counter(...))` and
`new Holder(factory())` currently reach codegen and fail with
`JvmConstructorCallArgumentTypeMismatch`; both need an initialized exact
reference argument contract, and the nested form additionally needs multiple
live allocation identities. Category-only linear analysis cannot establish
either proof. See the [object graph construction audit](jvm-object-graph-construction.md)
for source observer evidence and the full state contract.

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
| `dotty-typer` | Accepts immutable `ref.get()` and completed call-result receivers; rejects `current.get()` as `UnstableSelectionPrefix`. It does not emit bytecode. |
| `dotty-jvm` | Owns source visibility checks, exact field/constructor descriptors, and field-initializer plans. These plans preserve canonical symbol identities and reject unsupported source shapes. |
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
