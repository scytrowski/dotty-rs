# JVM object creation source contract

This document records the current source and semantic boundary for object
creation. Issue #1215 is a characterization audit. It adds no JVM object
allocation, reference-local, or cross-class call lowering.

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

There is a current typer blocker for direct chaining: `new Counter(42, true).get()`
fails in `SourceTyper` with `TyperError::UnstableSelectionPrefix`. The
constructor application itself and a subsequent call on a local initialized
from it type successfully. This issue records the blocker; it does not change
selection stability rules or lower around the failed source typing step.

## Source failure and access observations

The source Typer currently reports wrong constructor arguments before backend
planning:

| Source shape | Observed stage and result |
| --- | --- |
| `new C()` when `C` requires one argument | Typer: `ApplicationArityMismatch { expected: 1, actual: 0 }` |
| `new C(true)` when `C` requires `Int` | Typer: `ApplicationArgumentTypeMismatch` |
| `new C(1)` where `C` has a private primary constructor and the caller is another class | Currently accepted by the Typer; constructor candidate selection does not filter access |
| Equal applicable constructor overloads | Typer: `AmbiguousConstructorApplication`; it does not guess |
| Direct method call on `new C(...)` | Typer: `UnstableSelectionPrefix` before JVM planning |

When source typing has succeeded and a JVM plan reaches existing validation,
the relevant typed backend failures include:

| Invalid plan or selected shape | Backend error |
| --- | --- |
| Constructor symbol is not the source-proven primary constructor or has an unsupported source shape | `InvalidPrimaryConstructor { issue: SourceIdentity/SourceShape/... }` |
| Target class has no plan in this unit | `MissingClassPlan` |
| Abstract, generic, trait, object, nested, or otherwise unsupported class form | `UnsupportedJvmClass` |
| Reference class appears in a public primitive-only method descriptor | `UnsupportedJvmType { kind: ReferenceClass }` |
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

The current `tests/jvm_source_compilation.rs` harness deliberately accepts one
top-level class and publishes one `GeneratedClass`; it is not a multi-class
source compiler. Multi-class object emission needs a root-level orchestration
step: build and validate all canonical plans from one frontend session, emit
all class bytes in memory, and only then stage/publish the complete batch. A
semantic, lowering, or encoder failure must publish no class. Filesystem
renames across multiple outputs are not atomic; if staging or a later rename
fails, remove staged and already-published outputs before reporting failure.
This work is outside #1215.

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

The first approved lowering, in a future issue, must be restricted to a
straight-line, branch-free sequence:

```text
new C
dup
evaluate each supported primitive constructor argument once, left to right
invokespecial C.<init>(descriptor)
```

For `val counter = new Counter(seed); counter.get()` in an instance method,
the expected instruction order is conceptually:

```text
new Counter
dup
iload <seed-slot>
invokespecial Counter.<init>(I)V
astore <counter-slot>
aload <counter-slot>
invokevirtual Counter.get()I
ireturn
```

The `Counter.get()` result stays primitive. An `Int`/`Boolean` call used as a
statement must be followed by `pop`; a Unit-returning call has JVM result
descriptor `V` and leaves nothing to pop. The call target and receiver are
still resolved from typed symbol identity.
These are expected sequences for future lowering, not emitted bytes: this
issue deliberately adds no object instructions. The direct-new receiver
sequence cannot be produced from source until the Typer selection-prefix
blocker is resolved.

Before the matching constructor returns normally, the fresh reference must
not be stored, returned, passed to another method, used as an ordinary
receiver, exposed through a field, or carried across a branch. Constructor
argument lowering must itself be primitive-only and branch-free. Nested
`new`, conditional or short-circuit arguments, and reference operations in
`If`/`While` remain rejected until the whole-method verifier state can track
initialized and uninitialized references.

`dotty-classfile::bytecode::LinearCodeAnalyzer` is not that verifier. It
represents `New` as the broad `ValueKind::Reference`, lets `Dup` copy any
category-one value, and checks `InvokeSpecial` as a descriptor-shaped call;
it does not remember the allocation offset, distinguish initialized from
uninitialized references, or match a constructor to its allocation. The CFG
analyzer currently rejects reference-flow instructions such as `ALoad`,
`AStore`, `InvokeSpecial`, `InvokeVirtual`, and `AReturn` as unsupported
reference operations. Passing the linear analyzer alone is not evidence that
a generated constructor sequence verifies. Any future emitted object code
must run under JDK 21+ `java -Xverify:all`.

## Bounded support matrix

| Form | Characterization or future contract |
| --- | --- |
| Top-level ordinary class in the same validated emission unit | Candidate for the bounded subset |
| Plain primary constructor with primitive `Int`/`Boolean` arguments | Required future constructor target |
| Explicit or syntactic zero-argument primary constructor | Canonical class-owned constructor symbol required |
| `val c = new C(args)` and later `c.method()` | Typed identities are observed; future straight-line local and receiver support only |
| Method parameter/local receiver with same-spelled methods on different classes | Resolve by selected method and receiver `SymbolId`/`TypeId` |
| Direct `new C(args).method()` | Currently blocked by `UnstableSelectionPrefix` in Typer |
| `var` reference local, reference method parameter/result, arbitrary classpath receiver | Unsupported; no descriptor fallback |
| Generic, abstract, trait, external, secondary, or unplanned constructor | Unsupported for JVM emission |
| Nested allocation or allocation/reference inside branch or short-circuit CFG | Unsupported until verifier-aware reference CFG exists |
| Duplicate emitted internal class name | `JvmCompilationUnit` validation error |

The audit adds no `new`, `astore`, `aload`, `invokevirtual`, or reference
descriptor lowering. That remains the work of dependent implementation
issues.
