# JVM reference method signature audit

This document records source-to-JVM characterization and bounded reference
support. `JvmCompilationUnit::metadata` projects method parameters and results
that reference exact planned classes. Method bodies can load a planned
reference parameter and unqualified current `this` as expression values.
Method bodies also emit exact planned reference returns and ordinary planned
`invokevirtual` calls with reference receivers, arguments, and results.

The audit is pinned to the Java Virtual Machine Specification, Java SE 21:
[§4.3.2 Field Descriptors](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.3.2),
[§4.3.3 Method Descriptors](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.3.3),
[§4.7.4 `StackMapTable`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.7.4),
[§4.10.1.6 method initial frames](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.10.1.6),
[§4.10.1.9 `invokevirtual`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.10.1.9.invokevirtual),
and [§4.10.1.9 `areturn`](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.10.1.9.areturn).

## Real source and typed-tree observations

`reference_method_signature_audit_characterizes_real_typed_source` in
[`tests/jvm_source_compilation.rs`](../tests/jvm_source_compilation.rs)
runs these declarations through the repository's lexer, parser, namer, and
`SourceTyper`, then asks the unit-aware metadata projector for method
descriptors. No typed tree is fabricated. `Counter.get(): Int` is accepted as
`()I`, and each exact planned-class example receives its candidate descriptor.
The standalone store-only `JvmMethodPlan::descriptor` still rejects reference
types because that API has no validated emission-unit context. Source methods
with reference results are emitted when their bodies stay within the supported
expression subset. Exact class identity is preserved through returns and
calls; hierarchy conversions remain unsupported.

For these root-package declarations, a declared source type `Counter` is
`Type::TypeRef` targeting the canonical `Class:Counter` symbol with a
`TypeRef(Package:<root>)` prefix. `Int` and `Boolean` use `TypeRef(NoPrefix,
Class:Int/Boolean)`. A parameter read such as `c` is a typed `Ident` whose
type is `TermRef(NoPrefix, Parameter:c)`. A `new Counter(n)` expression has
`TypeRef(NoPrefix, Class:Counter)`, whereas the declared method result uses
the root-package-prefixed `Counter` type. `this` remains a `TreeKind::This`
with `Type::ThisType` naming its exact enclosing class.

The selected call `c.get()` preserves the receiver as the prefix of the
selected method `TermRef`: its prefix is `TermRef(Parameter:c)` and its target
is the canonical `Method:get` symbol. The constructor selection in
`new Counter(n)` has a `TypeRef(Class:Counter)` prefix and targets the
canonical `Constructor:<init>` symbol. Those paths carry enough identity for
planned-call validation; lowering must not recover the receiver or owner from
printed source names.

| Source method | Unit metadata descriptor | Emission status |
| --- | --- | --- |
| `pass(c: Counter): Counter = c` | `(LCounter;)LCounter;` | Emits `aload` and exact `areturn` |
| `make(n: Int): Counter = new Counter(n)` | `(I)LCounter;` | Emits initialized allocation then `areturn` |
| `self(): Counter = this` in `Counter` | `()LCounter;` | Emits `aload_0` and `areturn`; typed `this` is `ThisType(Counter)` |
| `read(c: Counter): Int = c.get()` | `(LCounter;)I` | Resolves the parameter receiver by its canonical symbol and exact class |
| `forward(c: Counter): Counter = pass(c)` | `(LCounter;)LCounter;` | Checks reference argument and call result against the planned descriptor |
| `readCreated(n: Int): Int = read(make(n))` | `(I)I` | Emits each call once in receiver/argument source order |
| `choose(flag: Boolean, a: Counter, b: Counter): Counter = if flag then a else b` | `(ZLCounter;LCounter;)LCounter;` | Exact `Object(Counter)` branch-stack join followed by `areturn` |
| `chooseThroughCall(flag, a, b): Counter = if flag then pass(a) else b` | `(ZLCounter;LCounter;)LCounter;` | Planned call result keeps its exact class through the branch join |
| `incrementUntil(counter: Counter, limit: Int): Int` | `(LCounter;I)I` | Parameter `Object(Counter)` remains in loop-header frames while receiver calls repeat |
| `incrementWhile(enabled: Boolean, counter: Counter, limit: Int): Int` | `(ZLCounter;I)I` | Short-circuit condition preserves exact parameter locals and an empty loop-header stack |
| `mix(flag: Boolean, count: Int, c: Counter, tail: Boolean): Counter` | `(ZILCounter;Z)LCounter;` | Mixed category-one slots are retained in source order |

The JVM descriptor grammar accepts these forms: object types use `L` plus the
internal class name plus `;`; method parameters are field descriptors, and a
method result is a field descriptor or `V`. Instance method parameter-slot
limits include the implicit receiver slot. For a packaged class, the binary
name is used, for example `Lp/Counter;`, rather than a source spelling or
`java/lang/Object` fallback.

## Verifier contract for emitted reference values

For an ordinary planned instance method `Client.read(c: Counter): Int`, the
descriptor is `(LCounter;)I`. Local 0 is the implicit `Client` receiver and
local 1 is the exact `Counter` parameter. Reference parameters and `this` used
as expression values load with `aload`; a reference parameter occupies one
local slot. The initial stack-map frame is derived from the method descriptor;
it must retain exact object types for `this` and each reference parameter.
The current backend emits only ordinary instance methods, but a generic class
file analyzer must also respect static-method initialization without a
receiver if that API is later used for static methods.

`invokevirtual Counter.get:()I` consumes the declared arguments in reverse
stack order, then the receiver, checking the receiver against the exact
resolved owner. It pushes the descriptor's exact result type. For
`read(c: Counter): Int`, the first local load must be `aload 1`; name-based
parameter lookup is insufficient. `areturn` must return a value assignable to
the exact declared reference result; checking only that both values are in the
reference category is insufficient for this compiler's no-hierarchy subset.

CFG joins retain an exact class only when incoming stack references agree.
`Counter` joined with `Counter` is valid; `Counter` joined with `Other` must
fail closed. A local disagreement may become unusable (`Top`) only if later
code does not read it. No join may silently select `Object` or infer a common
superclass. Every branch target frame must encode the exact descriptor-derived
or flow-derived `Object(CONSTANT_Class)` values.

`this` as a value can denote the current planned class only when its canonical
`ThisType` class is the current method's owner. A parameter receiver such as
`c.get()` must resolve the `TermRef` to that method's canonical parameter
symbol, then use its descriptor-derived slot and exact planned class. Neither
case may fall back to a printed name when a symbol, owner, class plan, or
prefix is missing.

## Current responsibility and limitation map

| Layer | Existing behavior | Invariant to preserve |
| --- | --- | --- |
| Parser / namer / typer | The audit fixture produces real canonical parameter, method, class, `This`, `Ident`, `Apply`, and `If` trees. Source `Counter` declarations retain a root-package `TypeRef` prefix. | Preserve these identities; do not manufacture or normalize trees to fit the backend. The current source Typer rejects `null` with `NullLiteralTypingDeferred`; it does not currently resolve source `Array` in this fixture (`TypeNameNotFound`). |
| `dotty-jvm::descriptor` | Unit metadata projects primitive types and exact planned-class references to checked JVM method descriptors. The standalone store-only `JvmMethodPlan::descriptor`, fields, and constructors remain primitive-only. | Keep canonical class identity, source package-prefix, owner, arity, modifiers, parameter-symbol order, and JVMS slot-limit validation. |
| `dotty-jvm` expression and local lowering | Planned reference locals, parameters, current `this`, returns, and virtual calls compose through exact `if` joins, `while` back-edges, nested blocks, and short-circuit conditions. | Keep exact parameter/class identity checks; evaluate receivers then arguments once in source order; preserve the validated owner and descriptor. |
| Linear `dotty-classfile` analyzer | Class-aware analysis seeds object parameters and the instance receiver with exact descriptor/current-class identities. In that mode, `areturn` and straight-line `invokevirtual` arguments/results require exact initialized classes. The context-free analyzer retains category-only behavior. | Preserve generic JVM validation and explicit rejection of unsupported descriptor forms. |
| CFG `dotty-classfile` analyzer | Class-aware flow seeds object method parameters with exact `ClassIdentity`, retains exact stack joins, and checks `areturn` and `invokevirtual` arguments/results against exact descriptor classes. | Preserve fail-closed joins and unsupported descriptor checks; do not infer hierarchy conversions. |
| Class-file builder / frames | Linear and CFG methods with supported object descriptors pre-intern descriptor/current-class identities before the pool is frozen. Initial and branch-target frames preserve exact object types produced by virtual calls. Unsupported descriptor shapes and mismatched or uninitialized values fail before bytes are returned. | Keep verifier-backed coverage for call results carried through control flow. |

The class-file descriptor parser and verifier enforce the exact reference
types passed between planned methods. This does not enable arbitrary classpath
targets, hierarchy conversions, reference fields, or reference-valued
constructor parameters.

## Rejection characterization and scope boundaries

The current source and lower-layer behavior is pinned by the audit test,
`unsupported_counter_object_source_cases_fail_at_their_pipeline_stage`, and
the focused `dotty-jvm` / `dotty-classfile` tests. Focused commands:

```text
cargo test --test jvm_source_compilation --locked reference_method_signature_audit_characterizes_real_typed_source
cargo test --test jvm_source_compilation --locked unsupported_counter_object_source_cases_fail_at_their_pipeline_stage
cargo test -p dotty-jvm --locked
cargo test -p dotty-classfile --locked
```

| Case | Current behavior |
| --- | --- |
| Reference parameter used as a value or unqualified `this` used as a value | Lowered with `aload(slot)` or `aload_0`, after exact planned-class and owner validation |
| Exact planned reference method result or argument to a planned call | Emits/accepts `areturn` and `invokevirtual` values only when canonical class identities match |
| Reference constructor parameter | `dotty-jvm` constructor signature stage: `UnsupportedJvmType::ReferenceClass` |
| `null` source expression | `dotty-typer`: `NullLiteralTypingDeferred` |
| `Array[Int]` source parameter | `dotty-typer`: `TypeNameNotFound`; the JVM descriptor layer is not reached |
| `Counter` / `Other` reference join stored in a local | `dotty-jvm` local-type projection: `UnsupportedJvmReferenceType::NotCanonicalTypeRef` |
| Source `new Object()` outside the planned batch | `dotty-typer`: `NewClassInfoUnavailable` |
| Canonical but unplanned or external class projected by the JVM adapter | `ReferenceClassNotInJvmPlan` or `UnsupportedJvmReferenceType::ExternalClass` |
| Wrong virtual-call owner in class-aware class-file flow | `AnalysisError::MethodOwnerMismatch` |
| Two distinct exact classes at a class-file stack join | `AnalysisError::IncompatibleStackValue` |

The `dotty-classfile` analyzer tests construct instruction graphs and constant
pools to check verifier-level owner and join errors; these tests do not replace
the real-source characterization and do not model Scala symbols. The
`dotty-jvm` call-target tests check canonical method/constructor ownership and
reject foreign, unresolved, or inconsistent symbols. They likewise do not
fabricate a source typed tree in the source-to-JVM audit.

Still excluded after this audit: reference fields; reference-valued
constructor arguments; external or unplanned receiver classes; general
inheritance, interfaces, subtyping and hierarchy joins (apart from the JVM rule
that every reference is assignable to `java/lang/Object`); `null`; arrays; mutable reference locals;
generic erasure; boxing; and Scala ABI/TASTy changes. In particular,
`Type::Or`/`Type::And` or a printed class name must not be converted to
`java/lang/Object`.

The canonical method signature metadata and source package-prefix checks are
implemented by #1270. Exact descriptor parameter seeding and reference-return
verification in the class-file builder are implemented by #1271. Reference
returns and planned virtual-call values are covered by real-source and typed
tree fixtures, with the JDK 21+ `java -Xverify:all` gate checking emitted
classfiles.
