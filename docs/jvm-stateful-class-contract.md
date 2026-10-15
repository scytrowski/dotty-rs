# JVM stateful class source contract

This document records the current Parser → Namer → Typer evidence needed
before JVM field and primary-constructor emission is added. It targets the
pinned Scala 3.9.0 reference at revision
[`777528f19a58e794c9954a42f433373472ec57f8`](https://github.com/scala/scala3/tree/777528f19a58e794c9954a42f433373472ec57f8).
This is a characterization contract; it does not claim that `dotty-jvm` emits
fields, accessors, or parameterized constructors.

## Reproducible source evidence

[`tests/jvm_stateful_class_contract.rs`](../tests/jvm_stateful_class_contract.rs)
runs actual source through the repository lexer, parser, namer, and typer. The
tests inspect canonical symbol IDs and owners, completed types, flags and
visibility, template declaration order, the primary constructor signature,
and the exact typed RHS root exposed by `SourceTypedIndex`.

| Source shape | Canonical field identity | Initializer / constructor evidence | Status |
| --- | --- | --- | --- |
| `class Plain { private val seed: Int = 42; private val enabled: Boolean = true }` | Each declaration maps to a distinct `SymbolKind::Field`, owned by `Plain`, with `Visibility::Private`; neither has `MUTABLE`. `SymbolInfo` completes to the canonical Int or Boolean type. | Template body order is `seed`, then `enabled`. Each source RHS is typed using `field_initializer_context_for`; its literal `TreeId<Typed>` is read back unchanged through `source_typed_index().get(source, rhs)`. The literal keeps its singleton `Type::Constant` type. | Frontend characterized. Backend rejects class fields today. |
| `class Counter(start: Int) { private var current: Int = start; def get(): Int = current }` | `current` is a private mutable field owned by `Counter`. The plain constructor parameter has a class-owned private `Parameter` identity and a separate constructor-owned public `Parameter` identity. The initializer reference resolves to the constructor-owned ID. | The primary constructor has one ordinary clause, one Int parameter, and a result `TypeRef` to `Counter`. The typed RHS root has `Type::TermRef(Symbol(constructorParameter))`; it is not rebound to the class-owned parameter or `current`. | Source signature and binding characterized. No JVM constructor plan or field emission exists. |
| `class C(val x: Int)` | The constructor parameter source tree maps canonically to a class-owned `Field`; `derived_symbol_at(constructor, source, tree)` returns a distinct constructor-owned `Parameter`. | The parameter order comes from the source constructor clause. | Identity audit only; public accessor ABI is out of scope. |
| `class C(var x: Int)` | The same two identities exist. `MUTABLE` is on the class-owned field only, not the derived constructor parameter. | Same clause ordering as `val`. | Identity audit only; public getter/setter ABI is out of scope. |

The `Counter.get` method body also types to a `TermRef` for the canonical
`current` field symbol. The exact typed RHS can be exported without a new API or retyping in
`dotty-jvm`: `SourceTyper::type_expression` returns the typed tree ID,
`SourceTyper::source_typed_index` is a read-only source-to-typed mapping, and
`SourceTyper::typed_ast` exposes the corresponding arena read-only. Repeating
the same successful request returns the same typed ID. A wrong Int/Boolean
initializer produces the typed `ExpectedExpressionTypeMismatch` at the Typer
stage; the expression transaction restores the semantic store, typed arena,
and source mapping. There is no partially published typed root on failure.

No adapter should look up a field or parameter by printed name. In particular,
the source tree for `start` has more than one related symbol identity, and only
the `TermRef` target identifies the parameter used by the initializer.

## Constructor order and initialization contract

For the supported future subset, constructor invocation must call
`java/lang/Object.<init>()V` first, then run supported field initializers in
`Template.body` source order, then return. A plan must pair each field's
canonical `SymbolId` with its exact typed initializer root. The field type and
typed expression's widened value type must conform before code is emitted.
There is no JVM slot for a field initializer itself. Constructor parameters
retain source clause and parameter order; in the one-clause `Counter(start)`
case, instance receiver occupies local slot 0 and `start` occupies slot 1.
That layout is a proposed lowering rule, not an API currently represented in
`dotty-core` or `dotty-jvm`.

Earlier initialized fields can be read after their initializer has executed.
The current source Typer resolves class members independent of runtime
initialization order, and explicit self/forward field reads are not rejected by
the context accessor. Inferred self and mutual cycles do fail in Typer with
`RecursiveInferredFieldType`, and rollback leaves their symbols incomplete;
the existing regression is
`inferred_field_recursion_guard_reports_direct_and_indirect_cycles` in
`crates/dotty-typer/src/typer/mod.rs`. A future constructor plan must reject
or defer self/forward reads with a contextual diagnostic before bytecode
publication. It cannot treat successful name or type resolution as proof of
safe initialization.

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

The future backend increment is limited to private instance storage fields and
ordinary user methods. It must not claim compatibility for public Scala
getters/setters, constructor parameter accessors, bridges, inheritance,
qualified access, `lazy`, generic/reference-valued members, or secondary
constructors. Scala 3.9.0 source inspection is recorded here; a generated
`javap` comparison was unavailable in this environment because the local
reference launcher requires missing `sbt`, and no Java runtime is installed.

## Stage-labeled rejection matrix

| Input shape | Observed stage / behavior | Backend contract |
| --- | --- | --- |
| `private val count: Int = true` | Typer returns `ExpectedExpressionTypeMismatch`; transaction rolls back the typed root and type allocations. | Preserve the contextual Typer error; publish no class artifact. |
| Field has no RHS | `field_initializer_context_for` returns `FieldInitializerContextInvalid(InitializerMissing)` at Typer context construction. | Do not invent a default initializer. |
| Inferred direct or mutual field cycle | `complete_symbol` returns `RecursiveInferredFieldType`; recursion state and symbol completion roll back. | Reject before class artifact publication. |
| `object` / `trait` field or non-field symbol | Typer context validation returns `FieldInitializerContextInvalid` with the actual owner/symbol kind. | This contract only covers top-level ordinary classes. |
| Plain `private` visibility on an instance field | Namer records `Visibility::Private`; this is the required supported visibility. | Backend must validate visibility from the canonical symbol. |
| Public/protected, qualified-private, lazy, static, or synthetic field | Outside the private ordinary instance field subset; not accepted as an implicit ABI shape. | Reject with a contextual plan error until an explicit ABI contract covers the form. |
| `val`/`var` constructor parameter | Namer retains separate class field and constructor parameter identities. | Do not substitute either identity for the other; public accessor emission remains unsupported. |
| Duplicate same-named field declarations | Parser and Namer currently accept both declarations without diagnostics and create distinct class-owned `Field` IDs with the same name. This is a recorded frontend gap, not valid backend input. | Plan validation must detect repeated names/identities and return a contextual error before publishing a class. |
| Secondary, `using`, defaulted, multiple-clause, generic, or otherwise unsupported constructor | These are outside the characterized plan subset. The current source frontend may accept them; backend planning must mark them unsupported rather than infer a JVM layout. | Return a typed contextual backend error before emitting any artifact. |
| Explicit self or forward field reference | Current field context can resolve the symbol; the Typer does not provide initialization-order proof for explicit field types. | Reject or defer until a separate safe-initialization analysis exists. |

The negative evidence remains executable: Typer tests
`field_initializer_context_rejects_unsupported_owners_and_non_fields` and
`inferred_field_recursion_guard_reports_direct_and_indirect_cycles`, plus the
new wrong-type rollback and duplicate-field characterization tests. The
duplicate-field result is intentionally marked as an unresolved Namer gap;
the backend must not treat the current absence of a diagnostic as evidence of
a valid field plan.

## Proposed platform-neutral adapter shapes

These are proposed data shapes, not implemented APIs:

```rust
struct JvmFieldPlan {
    symbol: SymbolId,
    initializer: TreeId<Typed>,
}

struct JvmPrimaryConstructorPlan {
    symbol: SymbolId,
    ordered_parameters: Vec<SymbolId>,
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

The current `JvmClassInitializationPlan::ImplicitNoArg` remains the only
implemented initialization proof. It is valid only for the already documented
empty no-argument class subset and must not be upgraded based on an empty
method list when source fields or constructor parameters exist.
