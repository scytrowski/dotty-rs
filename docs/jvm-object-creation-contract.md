# JVM object creation source contract

This document records the current source and semantic boundary for object
creation, immutable reference locals, and bounded virtual calls.

## Typed source evidence

The focused tests in
[`crates/dotty-typer/tests/refactor_contract.rs`](../crates/dotty-typer/tests/refactor_contract.rs)
run source through the real lexer, parser, namer, and `SourceTyper` with one
`SemanticStore`, `Definitions`, and `SourceSemanticIndex`.

For `new Counter(42, true)`, the typed expression is:

```text
Apply(
  Select(New(type-tree), <init>),
  [42, true]
)
```

The selected function's type is a `TermRef` whose target is the canonical
constructor `SymbolId`; that symbol is a `Constructor` owned by the `Counter`
class and entered in its declaration scope. The application and `New` tree
each carry a `TypeId` that projects to `TypeRefTarget::Symbol(Counter)`. Their
`TypeId`s need not be the same arena ID; compare the canonical class target,
not the printed type or arena allocation order. Constructor arguments remain
in source order and preserve their own typed parameter or literal identities.

The namer creates the class's zero-argument primary constructor when the
source omits a parameter list. Both `new Empty()` and the accepted bare
`new Empty` form become a zero-argument constructor `Apply` with a selected,
class-owned constructor symbol. The typer does not ask the JVM backend to
invent a constructor.

For a local declaration such as `val counter = new Counter(seed, true)`,
`SourceTyper::typed_local_binding_at(source, source_val_tree)` returns the
exact typed `ValDef` and the typer-owned local `SymbolId`. The local symbol is
owned by the containing method. The typed `Ident` in `counter.get()` keeps
`TermRefTarget::Symbol(local)`; widening that term reference gives the
`TypeRefTarget::Symbol(Counter)` class type. This mapping is the source
adapter's evidence for a future `JvmLocalBinding`; a backend must not find the
local again by name, source span, or tree allocation order.

For ordinary parameter or local receivers, method calls remain
`Apply(Select(receiver, method), args)`. The selected method `TermRef` names
the exact method `SymbolId`, whose owner is the declaring class. The widened
receiver type separately identifies the runtime class. The audit uses two
classes with the same `read` spelling and distinct `Int`/`Boolean` signatures;
selection follows the receiver's class identity and preserves the matching
result type.

Direct calls on an initialized application result are accepted by the source
typer. Both `new Counter(42, true).get()` and `makeCounter(...).get()` retain
their selected constructor/method identities and are covered by the source
compilation regressions. The earlier `UnstableSelectionPrefix` observation
applies to unstable mutable-field prefixes such as `current.get()`, not to a
completed `new` or method application. See the
[object graph construction audit](jvm-object-graph-construction.md).

## Source failure and access observations

The source Typer currently reports wrong constructor arguments before backend
planning:

| Source shape | Observed stage and result |
| --- | --- |
| `new C()` when `C` requires one argument | Typer: `ApplicationArityMismatch { expected: 1, actual: 0 }` |
| `new C(true)` when `C` requires `Int` | Typer: `ApplicationArgumentTypeMismatch` |
| `new C(1)` where `C` has a private primary constructor and the caller is another class | Currently accepted by the Typer; constructor candidate selection does not filter access |
| Equal applicable constructor overloads | Typer: `AmbiguousConstructorApplication`; it does not guess |
| Direct method call on a completed `new C(...)` result | Supported for the bounded planned class and method subset |

When source typing has succeeded and a JVM plan reaches existing validation,
the relevant typed backend failures include:

| Invalid plan or selected shape | Backend error |
| --- | --- |
| Constructor symbol is not the source-proven primary constructor or has an unsupported source shape | `InvalidPrimaryConstructor { issue: SourceIdentity/SourceShape/... }` |
| Target class has no plan in this unit | `MissingClassPlan` |
| Abstract, generic, trait, object, nested, or otherwise unsupported class form | `UnsupportedJvmClass` |
| Reference class appears in the store-only `JvmMethodPlan::descriptor` or a constructor signature | `UnsupportedJvmType { kind: ReferenceClass }` |
| Exact planned reference appears in `JvmCompilationUnit::metadata` | Valid method descriptor metadata; class emission remains unsupported |
| Call uses an arbitrary non-`this` selection in the current same-class resolver | `UnsupportedJvmCallShape { kind: NonThisReceiver }` |
| Distinct class symbols have the same emitted internal name | `DuplicateJvmClassIdentity` |

Private constructor access is not source-checked by the current Typer, and
`JvmPrimaryConstructorPlan::from_source` is not a call-site access check: it
accepts a private primary constructor and preserves its visibility as
`ACC_PRIVATE` metadata. The existing
[`ordinary_primary_constructor_metadata_uses_derived_parameters_and_void_result`](../tests/jvm_stateful_class_contract.rs)
regression covers `class C private (x: Int)` and verifies that the plan and
class metadata can be produced. A future constructor-call resolver must check
visibility against the canonical caller and constructor symbols before
producing executable code. An accepted typed tree or constructor metadata is
not proof that a particular caller may invoke it.

The typer also resolves a declared secondary constructor when it is the unique
applicable constructor. That fact is not permission for the JVM subset in
this sprint: the object-creation plan is limited to a proven plain primary
constructor. A backend must return a typed unsupported-constructor error for
secondary, generic, deferred, external, abstract, trait, or unplanned class
targets; it must not fall back to a name or guessed descriptor.

## Receiver, class, and emission identity

The source semantic index enters each top-level class once in its declaration
scope. Multiple classes in one source unit share the same semantic store and
`SourceTyper`; constructor and method selections carry canonical symbol
identities across class boundaries. The JVM integration adapter must collect
each class from the source package's declaration order and use those same
symbols and typed trees to build one `JvmCompilationUnit`.

`JvmCompilationUnit::classes` is a caller-ordered `Vec<JvmClassPlan>`. The
backend validates plans and rejects duplicate class symbols and distinct
class symbols with duplicate emitted internal names. It does not discover the
source class set or reconstruct source declaration order. The source
orchestrator must keep a deterministic source-order list and finish every
class plan before emitting any class.

`tests/jvm_source_compilation.rs` now has a multi-class source adapter and
retains `compile_source` as the one-class/one-artifact wrapper used by existing
fixtures. The batch adapter lexes, parses, and names once, keeps source-order
class plans in one `JvmCompilationUnit`, and completes class, constructor,
field, and method signatures before typing dependent bodies. It emits every
class into memory before staging any output, then publishes under validated
internal-name paths in source order. A semantic, typing, lowering, or encoder
failure publishes no class. Filesystem renames across multiple outputs are
not atomic; on a staging or later rename error, the adapter makes a best-effort
rollback of staged and already-published outputs. This is the bounded test
orchestration increment in #1221, not a production multi-file compiler.

The future backend value type must carry the exact planned class `SymbolId`,
for example `Reference(Counter)`. It may not collapse references to
`java/lang/Object`, infer identity from a type's printed name, or infer the
selected method from its spelling. Keep reference expressions separate from
public method parameter/result descriptors until those descriptors are
explicitly supported.

## JVM initialization safety boundary

The JVM verifier models the result of each `new` as a distinct
`uninitialized(offset)` verification type. `dup` duplicates that same
uninitialized identity. The matching `invokespecial C.<init>` must consume
that instance and name the constructor of the class created by that `new`;
only after successful initialization does the verifier treat the remaining
duplicate as an ordinary `C` reference. The JVM Specification describes the
canonical sequence as `new; dup; [arguments]; invokespecial <init>` and rejects
uses of the object before initialization or multiple initialization.
[JVMS 21 §4.10.1.4 and §4.10.1.9](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.10.1.4)
defines the verifier's uninitialized-object types and their rewrite after
`invokespecial`.

The approved constructor and immutable-reference-local lowering is restricted
to a straight-line, branch-free sequence:

```text
new C
dup
evaluate each supported primitive constructor argument once, left to right
invokespecial C.<init>(descriptor)
astore <local-slot>       # only after invokespecial completes
aload <local-slot>        # later reads use the same canonical local identity
```

For `val counter = new Counter(seed); counter` as a discarded block statement
in an instance method, the emitted instruction order is:

```text
new Counter
dup
iload <seed-slot>
invokespecial Counter.<init>(I)V
astore <counter-slot>
aload <counter-slot>
pop
```

This sequence is covered by real-source classfile regressions and a mandatory
JDK `-Xverify:all` CI gate. A completed direct `new C(args)` and an initialized
immutable local of `C` can each serve as the receiver of one planned
`invokevirtual` call with primitive arguments and primitive/Unit results.

Before the matching constructor returns normally, the fresh reference must
not be stored, returned, passed as an ordinary method argument, used as an
ordinary receiver, exposed through a field, or carried across a branch. The
class-aware CFG verifier supports up to two nested `new` sites in one straight-
line block and can call an approved method on an initialized receiver while a
separate outer allocation remains pending. Source lowering supports one
planned nested constructor application per outer constructor argument:
`new Holder(new Counter(seed))`. Multiple sibling inner allocations are
completed one at a time. It rejects deeper nested constructor applications and
conditional or short-circuit expressions while an allocation remains pending.
Reference operations in `If`/`While` remain outside this constructor-argument
contract.

`dotty-classfile::bytecode::LinearCodeAnalyzer` is not that verifier. It
represents `New` as the broad `ValueKind::Reference`, lets `Dup` copy any
category-one value, and checks `InvokeSpecial` as a descriptor-shaped call;
it does not remember allocation offsets, distinguish initialized from
uninitialized references, or match constructors to allocation sites. The
class-aware CFG analyzer tracks up to two pending allocation-site identities,
requires last-in, first-out constructor initialization, and rejects aliases
that escape or cross CFG edges. Passing the linear analyzer alone is not
evidence that a generated constructor sequence verifies. Any emitted object
code must run under JDK 21+ `java -Xverify:all`.

## Bounded support matrix

| Form | Current status |
| --- | --- |
| Top-level ordinary class in the same validated emission unit | Supported in source order by the root test adapter's one-session batch path |
| Plain public primary constructor with primitive `Int`/`Boolean` arguments | Supported with canonical class, constructor, and constructor-owned parameter identities |
| Public implicit no-argument primary constructor | Supported when the source shape proves the backend's exact `()V` constructor |
| `val c = new C(args)`; discard or read `c` in a straight-line method | Supported with exact class identity, `astore`/`aload`, definite initialization, and category-one `pop` |
| `val c = new C(args)` and later `c.method()` | Supported for an exact planned class and bounded primitive/Unit method signature |
| Direct `new C(args).method()` | Supported after constructor initialization for the bounded method subset |
| `new C(parameter)` or an exact immutable local constructor argument | Supported when the reference class and constructor are planned in the same unit |
| `new Holder(new Counter(seed))` | Supported for one nested planned constructor application; each allocation site is initialized by its own exact constructor in LIFO order |
| `new Pair(new Counter(first), new Counter(second))` | Supported; sibling child allocations are initialized sequentially while Pair remains the one separate pending allocation |
| `new C(this)` from a method of `C` | Supported for the exact current-class reference and planned constructor |
| Method receiver when planned classes declare same-spelled methods | Selected method and receiver `SymbolId`/`TypeId` preserve the exact owner |
| Private constructor call, wrong constructor arity/type, or missing external class metadata | Rejected at the originating Typer or backend stage; no artifact is published |
| Initialized mutable reference local, planned factory result, approved private current-instance field, or canonical null for a reference formal used as a constructor argument | Supported by the bounded initialized-reference whitelist |
| Reference field initialized by `new` in the primary constructor | `UnsupportedJvmReferenceFieldInitializer` |
| `var` reference local as a receiver, unsupported reference method descriptor, arbitrary classpath receiver | Unsupported; no descriptor fallback |
| Generic, abstract, trait, external, secondary, or unplanned constructor | Unsupported for JVM emission |
| Third simultaneously live allocation, deeper source nesting, branch-valued constructor argument with a pending allocation, or an uninitialized allocation crossing a CFG edge | Unsupported; the source emitter accepts only one direct nested constructor level and the class-aware verifier rejects pending-site control-flow edges |
| Duplicate emitted internal class name | `JvmCompilationUnit` validation error |

`tests/fixtures/jvm/CounterObjects.scala` is the end-to-end source gate for this
slice. One real lexer/parser/namer/typer session builds plans for `Counter` and
`CounterClient`; the backend emits both classfiles in memory before the root
adapter publishes either. Decoded descriptors and instructions cover the
constructor-owned `start` parameter, private mutable field initialization,
reference local stores/loads, exact `invokevirtual` targets, and primitive
results. A Java-only harness checks `run(10) == 11`, `run(0) == 1`,
`direct() == 42`, and `independent() == 31` under `java -Xverify:all`. Fresh
frontends must produce byte-identical classfiles in source order.

The JDK 21 CI gate sets `DOTTY_CLASSFILE_REQUIRE_JDK=1`, so missing `java`,
`javac`, or `javap` fails the required test instead of skipping it. Rejected
source forms assert their actual Typer or backend error stage and publish no
classfiles. Forged, stale, wrong-owner, and unplanned typed receiver targets
are also covered by `dotty-jvm` unit tests without manufacturing source typed
trees in the end-to-end harness.

Reference-aware CFG merging remains the next required JVM backend increment.
Reference-valued method descriptors, mutable reference locals, reference
results in `If`, and reference-bearing `While` remain unsupported.
