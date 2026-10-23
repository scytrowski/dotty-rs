# JVM stateful class source contract

This document records the current bounded Parser → Namer → Typer → JVM
contract for private primitive fields (including category-two `Long`) and
primary constructors. It targets the
pinned Scala 3.9.0 reference at revision
[`777528f19a58e794c9954a42f433373472ec57f8`](https://github.com/scala/scala3/tree/777528f19a58e794c9954a42f433373472ec57f8).
It covers the real-source `Counter` and `PrivateFields` integration fixtures;
it does not claim full Scala ABI, generated accessor, or TASTy support.

## Reproducible source evidence

[`tests/jvm_stateful_class_contract.rs`](../tests/jvm_stateful_class_contract.rs)
runs actual source through the repository lexer, parser, namer, and typer. The
tests inspect canonical symbol IDs and owners, completed types, flags and
visibility, template declaration order, the primary constructor signature,
and the exact typed RHS root exposed by `SourceTypedIndex`.

| Source shape | Canonical field identity | Initializer / constructor evidence | Status |
| --- | --- | --- | --- |
| `class Plain { private val seed: Int = 42; private val enabled: Boolean = true; private val large: Long = 9000000000L }` | Each declaration maps to a distinct `SymbolKind::Field`, owned by `Plain`, with `Visibility::Private`; none has `MUTABLE`. `SymbolInfo` completes to its canonical primitive type. | Template body order is preserved. Each source RHS is typed with `type_expression_expected(rhs, context, completed_field_type)` using `field_initializer_context_for`; its literal `TreeId<Typed>` is read back unchanged through `source_typed_index().get(source, rhs)`. The `JvmFieldPlan`s project `I`, `Z`, and `J`, private access, and immutable `ACC_FINAL`. | `PrivateFields.scala` covers `I`/`Z`; `LongFields.scala` covers category-two Long storage and initialization. Both are decoded and verified with the JDK. |
| `class Counter(start: Int) { private var current: Int = start; def get(): Int = current }` | `current` is a private mutable field owned by `Counter`. The plain constructor parameter has a class-owned private `Parameter` identity and a separate constructor-owned public `Parameter` identity. The initializer reference resolves to the constructor-owned ID. | The primary constructor has one ordinary clause, one Int parameter, and a result `TypeRef` to `Counter`. The typed RHS root has `Type::TermRef(Symbol(constructorParameter))`; it is not rebound to the class-owned parameter or `current`. | The `Counter.scala` source integration emits exactly `<init>(I)V`, initializes `current:I` from the canonical constructor parameter, and lowers `get`, `set`, and `increment`. Fresh frontend runs produce identical class bytes; JDK CI verifies independent instance state and method behavior. |
| `class LongBox(initial: Long) { private var current: Long = initial }` | The source constructor parameter and derived constructor-owned formal remain distinct identities. The initializer references the exact constructor-owned `SymbolId`. | Descriptor `(J)V`; `initial` begins at local slot 1 and occupies slots 1/2. The initializer plan contains a Long parameter load and `putfield current:J`. | `LongFields.scala` emits the category-two constructor, setter, and getter with exact `J` descriptors; a JDK harness checks signed values and independent instances. |
| `class LongConstant { private val number: Long = 9000000000L }` | `number` is a canonical private immutable Long field owned by `LongConstant`. | Its typed constant preserves the exact signed `i64` value and lowers to `lconst`/`ldc2_w` before `putfield number:J`. | `LongFields.scala` decodes the Long constant pool entry and executes the class with `java -Xverify:all`. |
| `class C(val x: Int)` | The constructor parameter source tree maps canonically to a class-owned `Field`; `derived_symbol_at(constructor, source, tree)` returns a distinct constructor-owned `Parameter`. | The parameter order comes from the source constructor clause. | Identity audit only; public accessor ABI is out of scope. |
| `class C(var x: Int)` | The same two identities exist. `MUTABLE` is on the class-owned field only, not the derived constructor parameter. | Same clause ordering as `val`. | Identity audit only; public getter/setter ABI is out of scope. |

The combined [`LongCategory2.scala`](../tests/fixtures/jvm/LongCategory2.scala)
fixture verifies these stateful shapes in the same three-class source batch as
Long calls and control flow. Its test decodes all artifacts, checks exact field
and method descriptors plus constructor initialization order, and executes
cross-class reads and writes on JDK 21.

The `Counter.get` method body also types to a `TermRef` for the canonical
`current` field symbol. The exact typed RHS can be exported without a new API or retyping in
`dotty-jvm`: `SourceTyper::type_expression_expected` returns the typed tree ID,
`SourceTyper::source_typed_index` is a read-only source-to-typed mapping, and
`SourceTyper::typed_ast` exposes the corresponding arena read-only. Repeating
the same successful request returns the same typed ID. A wrong Int/Boolean
initializer produces the typed `ExpectedExpressionTypeMismatch` at the Typer
stage when the expected type is the completed canonical field type; the
expression transaction restores the semantic store, typed arena, and source
mapping. There is no partially published typed root on failure. The
unconstrained `type_expression` API can type an RHS without checking it against
the field type, so its success alone is not initializer conformance evidence
and must not be used to build a JVM field plan.

No adapter should look up a field or parameter by printed name. In particular,
the source tree for `start` has more than one related symbol identity, and only
the `TermRef` target identifies the parameter used by the initializer.

## Constructor order and initialization contract

For the supported subset, constructor invocation must call
`java/lang/Object.<init>()V` first, then run supported field initializers in
`Template.body` source order, then return. The
`JvmCompilationUnit::lower_constructor_field_initializers` method now
produces checked fragments for Int/Boolean/Long literals and constructor
parameters, including two-slot Long loads,
approved primitive intrinsics, reads of earlier initialized fields, and one
completed direct allocation for a reference field. That allocation is limited
to a planned public class and primitive literal or exact enclosing-constructor
parameter arguments. A plan pairs each field's canonical `SymbolId` with its
exact typed initializer root.
The field type and typed expression's widened value type must conform before
instructions are returned. There is no JVM slot for a field initializer
itself. Constructor parameters retain source clause and parameter order; in the
one-clause `Counter(start)` case, instance receiver occupies local slot 0 and
`start` occupies slot 1. That parameter layout is exposed by
`JvmConstructorMetadata`. These fragments do not constitute a complete
constructor or class emitter.

Earlier initialized fields can be read by the implemented initializer
lowering. The integration characterization also confirms that explicitly
typed `this.self` and unqualified forward `later` initializers type
successfully to the canonical field symbols, including the self-reference.
The source Typer resolves class members independent of runtime initialization
order, and `field_initializer_context_for` does not itself enforce runtime
initialization order. Constructor initializer lowering rejects self and
forward reads before returning instructions. Inferred self and mutual cycles
do fail in Typer with `RecursiveInferredFieldType`, and rollback leaves their
symbols incomplete; the existing regression is
`inferred_field_recursion_guard_reports_direct_and_indirect_cycles` in
`crates/dotty-typer/src/typer/mod.rs`. Successful name or type resolution is
not proof that a field read is safe at its runtime initialization point.

## Scala field and accessor boundary

In the pinned compiler's `Getters` phase, ordinary non-private class `val`s and
`var`s become getter definitions; mutable members also receive setter
definitions. Later `Memoize` materializes backing fields for getters and
setters. Plain `private val` and `private var` are still accessors: their
getters (and a var's setter) keep private access, and storage is represented
by backing field(s). The special `private[this]` form follows a separate
no-getter path. Primary-constructor `val`/`var` parameters are parameter
accessors; a plain parameter such as `start` is not a public field declaration.

These rules are visible in the pinned compiler sources:

- [`Getters.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/transform/Getters.scala) replaces non-private vals/vars with accessor methods and explains that fields are introduced later.
- [`Memoize.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/transform/Memoize.scala) adds private backing fields and directs getters/setters to storage.
- [`SymDenotations.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/core/SymDenotations.scala) defines class parameter accessors and distinguishes getters from setters.

The current whole-class emitter supports private primitive instance fields,
including category-two Long fields,
one plain primary constructor, and planned ordinary user methods for a
top-level Object-derived class. The JDK 21 CI gate uses `javap -c -v`,
`javac`, and `java -Xverify:all` on the real fixtures. It does not claim
compatibility for public Scala getters/setters, constructor parameter
accessors, bridges, inheritance, qualified access, `lazy`,
generic/reference-valued members, or secondary constructors.

## Stage-labeled rejection matrix

| Input shape | Observed stage / behavior | Backend contract |
| --- | --- | --- |
| `private val count: Int = true` | Typer returns `ExpectedExpressionTypeMismatch`; transaction rolls back the typed root and type allocations. | Preserve the contextual Typer error; publish no class artifact. |
| Field has no RHS | `field_initializer_context_for` returns `FieldInitializerContextInvalid(InitializerMissing)` at Typer context construction. | Do not invent a default initializer. |
| Inferred direct or mutual field cycle | `complete_symbol` returns `RecursiveInferredFieldType`; recursion state and symbol completion roll back. | Reject before class artifact publication. |
| `object` / `trait` field or non-field symbol | Typer context validation returns `FieldInitializerContextInvalid` with the actual owner/symbol kind. | This contract only covers top-level ordinary classes. |
| Plain `private` visibility on an instance field | Namer records `Visibility::Private`; real-source integration projects this visibility to JVM `ACC_PRIVATE`. | Field metadata validates visibility from the canonical symbol. |
| Public, `protected`, or `private[this]` instance field | Parser and Namer accept these source forms. The integration characterization observes `Visibility::Public`, `Visibility::Protected`, and `Visibility::Private` respectively; the shared visibility value does not preserve the `private[this]` qualifier. | Public and protected fail the canonical visibility check. The frontend must label object-private plans explicitly, and metadata rejects `PrivateThis` while accepting only plain `private`. |
| Qualified-private, lazy, static, or synthetic field | Not covered by the current source integration characterization; acceptance and exact canonical representation remain unclaimed here. | Reject with a contextual plan error until an explicit ABI contract covers the form. |
| `val`/`var` constructor parameter | Namer retains separate class field and constructor parameter identities. | Do not substitute either identity for the other; public accessor emission remains unsupported. |
| Duplicate same-named field declarations | Parser and Namer currently accept both declarations without diagnostics and create distinct class-owned `Field` IDs with the same name. This is a recorded frontend gap, not valid backend input. | Metadata rejects repeated field IDs and duplicate JVM field names with structured errors. |
| Secondary constructor | Parser and Namer accept `class WithSecondary(value: Int) { def this() = this(0) }`; Typer completes the primary and secondary constructors as distinct plain method signatures with one and zero parameters respectively. | Return a typed contextual backend error until secondary constructor lowering is supported. |
| `using` primary constructor | Parser and Namer accept `class WithUsing(using context: Int)`; Typer completes its signature as one `Contextual` clause with one Int parameter. | Return a typed contextual backend error until contextual constructor lowering is supported. |
| Defaulted primary constructor parameter | Parser and Namer accept `class WithDefault(value: Int = 1)`; Typer completes its signature as one plain clause with one Int parameter. This signature alone does not characterize default-getter behavior or JVM ABI. | Return a typed contextual backend error until default arguments are explicitly supported. |
| Multiple-clause, generic, or otherwise unsupported constructor | These shapes remain outside the current source integration characterization. No frontend acceptance or canonical representation is claimed here. | Return a typed contextual backend error before emitting any artifact. |
| Explicit self or forward field reference | Integration tests show `this.self` and unqualified `later` type to the canonical `self` and `later` field IDs. This is successful name/type resolution only; Typer does not provide initialization-order proof. | Reject or defer until a separate safe-initialization analysis exists. |

The negative evidence remains executable: Typer tests
`field_initializer_context_rejects_unsupported_owners_and_non_fields` and
`inferred_field_recursion_guard_reports_direct_and_indirect_cycles`, plus the
integration tests for initializer conformance and rollback, constructor forms,
visibility, self/forward references, duplicate fields, and the real-source JVM
metadata boundary. Field metadata rejects wrong owners, non-field IDs,
incomplete types, missing/mismatched initializers, public/lazy fields,
references, duplicate symbols, and duplicate JVM names. The
duplicate-field result is intentionally marked as an unresolved Namer gap;
the backend must not treat the current absence of a diagnostic as evidence of
a valid field plan.

## Platform-neutral adapter shapes

Field plans and explicit primary-constructor metadata are implemented in
`dotty-jvm`; the whole-class emitter consumes both after frontend validation.
The constructor plan
retains private source evidence checked by `JvmPrimaryConstructorPlan::from_source`
against the shared AST/index. Its inputs include the exact constructor and
ordered derived parameter IDs. The field shape below remains unchanged:

```rust
enum JvmFieldSourceVisibility {
    PlainPrivate,
    PrivateThis,
}

struct JvmFieldPlan {
    field: SymbolId,
    source_visibility: JvmFieldSourceVisibility,
    initializer: Option<TreeId<Typed>>,
}
```

The frontend adapter owns plan construction because it has the parser, namer,
and typer identities and source order. `dotty-jvm` should consume these
platform-neutral IDs alongside only `dotty-core` and `dotty-classfile` in its
production dependencies. Validation must check canonical owner/kind,
completed types and flags, exact arena membership, parameter signature/order,
initializer conformance, source-order initialization, and the supported class
and constructor shape. The class artifact must only be returned after every
field, method, constructor, and class-file structure validates successfully.

`JvmClassInitializationPlan::PrimaryConstructor` authorizes emission of its
exact constructor descriptor/access and source-ordered private primitive field
initializers, including Long descriptor `J` and category-two parameter slots.
`ImplicitNoArg` is reserved for the empty no-argument class
subset and never implies fields or constructor parameters.

The constructor metadata regressions in `jvm_stateful_class_contract.rs` cover
`()V`, `(I)V`, `(ZI)V`, private access, canonical identity order, and the
254-parameter instance limit. `jvm_source_compilation.rs` adds whole-pipeline
fixtures and verifies complete class bytes, descriptors, flags, field
references, deterministic fresh-session output, and executable JDK harnesses.
Unsupported source shapes are rejected before projection; malformed semantic
information is also checked independently. See [the backend
contract](jvm-backend.md#explicit-primary-constructor-metadata).
