# JVM reference method signature audit

This is a characterization of the existing source-to-JVM path, not an
implementation of reference-valued method descriptors. Candidate descriptors
are valid JVM descriptors, but this backend increment deliberately rejects
reference parameters and results before publishing any class file.

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
`SourceTyper`, then asks the actual `JvmMethodPlan` for descriptor metadata.
No typed tree is fabricated. `Counter.get(): Int` is accepted as `()I`; each
reference-signature example fails during descriptor projection with
`JvmCodegenError::UnsupportedJvmType { kind: ReferenceClass }`. The adapter
does not publish class files after that failure.

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

| Source method | Candidate descriptor | Current result |
| --- | --- | --- |
| `pass(c: Counter): Counter = c` | `(LCounter;)LCounter;` | Descriptor rejected at parameter `c` |
| `make(n: Int): Counter = new Counter(n)` | `(I)LCounter;` | Descriptor rejected at result `make` |
| `self(): Counter = this` in `Counter` | `()LCounter;` | Descriptor rejected at result `self`; typed `this` is `ThisType(Counter)` |
| `read(c: Counter): Int = c.get()` | `(LCounter;)I` | Descriptor rejected at parameter `c`; typed receiver is `Ident(c)` |
| `choose(flag: Boolean, a: Counter, b: Counter): Counter = if flag then a else b` | `(ZLCounter;LCounter;)LCounter;` | Descriptor rejected at first reference parameter `a`; typed branches both have the exact `Counter` type |

The JVM descriptor grammar accepts these forms: object types use `L` plus the
internal class name plus `;`; method parameters are field descriptors, and a
method result is a field descriptor or `V`. Instance method parameter-slot
limits include the implicit receiver slot. For a packaged class, the binary
name is used, for example `Lp/Counter;`, rather than a source spelling or
`java/lang/Object` fallback.

## JVMS contract for a future implementation

For an ordinary planned instance method `Client.read(c: Counter): Int`, the
descriptor is `(LCounter;)I`. Local 0 is the implicit `Client` receiver and
local 1 is the exact `Counter` parameter. A reference parameter occupies one
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

| Layer | Existing behavior | Work needed for reference signatures |
| --- | --- | --- |
| Parser / namer / typer | The audit fixture produces real canonical parameter, method, class, `This`, `Ident`, `Apply`, and `If` trees. Source `Counter` declarations retain a root-package `TypeRef` prefix. | Preserve these identities; do not manufacture or normalize trees to fit the backend. The current source Typer rejects `null` with `NullLiteralTypingDeferred`; it does not currently resolve source `Array` in this fixture (`TypeNameNotFound`). |
| `dotty-jvm::descriptor` | `JvmType::Reference(SymbolId)` exists for bounded expression values and immutable locals, but `primitive_value_type` accepts only `Int`/`Boolean`; `primitive_result_type` accepts result-only `Unit`. `descriptor_char` returns no descriptor for references. | Resolve reference types from canonical semantic symbols and the validated same-batch class plans, validate source prefixes, construct `L<internal-name>;`, and keep ownership, arity, modifiers, parameter-symbol order, and the JVMS slot limit checks. |
| `dotty-jvm` expression and local lowering | Planned reference locals and primitive-result calls on `this`, `new C`, or an immutable local are supported. Parameters are emitted using `iload`; reference `Ident` parameters and `This` as a value are not yet lowered. `append_method_return` emits `ireturn` for all value results. | Map reference parameters to `aload`; map only a matching canonical `ThisType` to `aload_0`; preserve receiver-first call evaluation; add typed reference `Flow` and `areturn` with exact owner/result checks. |
| Linear `dotty-classfile` analyzer | It parses full class-file descriptors and models values primarily by JVM category. Its generic invocation and return checks do not provide the planned exact source-class guarantees. | Make return and invocation checks descriptor-shaped and exact where class identity is available; preserve the generic bytecode crate's Scala independence. |
| CFG `dotty-classfile` analyzer | `Frame::new` lays out descriptor parameters by category, but `CfgCodeAnalyzer::analyze_validated_with_current_class` then rejects every non-int parameter as `UnsupportedFlowParameter`. In class-aware flow, `invokevirtual` accepts only `I`/`Z` arguments and `I`/`Z`/`V` results; result transfer currently treats any permitted non-void result as int. CFG validation rejects `areturn` up front as `UnsupportedFlowInstruction { kind: Reference }`. The shared `Frame` transfer used by linear analysis checks only the reference category for `areturn`. | Seed reference parameters with exact `ClassIdentity`; pop call arguments against descriptor types in reverse order; push exact reference results; add descriptor-exact `areturn` checking before admitting it to CFG flow; retain fail-closed exact stack joins. |
| Class-file builder / frames | The builder interns exact classes discovered in analyzed flow before freezing the pool and serializes receiver/reference CFG frames. The initial frame is implicit from the descriptor, but the analyzer does not yet admit reference parameters. | Include descriptor-derived parameter classes in pre-interning and flow state; serialize matching initial and branch-target frames; continue to reject malformed or unsupported methods before bytes are returned. |

The class-file descriptor parser already accepts reference descriptors. That is
wire-format support, not evidence that the Scala adapter, source expressions,
CFG transfer functions, or verifier frames support those descriptors.

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

| Case | Current rejection |
| --- | --- |
| Reference parameter or result on an ordinary method | `dotty-jvm` descriptor stage: `UnsupportedJvmType::ReferenceClass`; no class file is published |
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
constructor arguments; external or unplanned receiver classes; inheritance, interfaces,
subtyping and hierarchy joins; `null`; arrays; mutable reference locals;
generic erasure; boxing; and Scala ABI/TASTy changes. In particular,
`Type::Or`/`Type::And` or a printed class name must not be converted to
`java/lang/Object`.

## Recommended implementation sequence

1. Add canonical signature projection for exact same-batch planned classes,
   including validation of the source `TypeRef` package prefix. Keep field and
   constructor signatures primitive-only.
2. Initialize instance parameter metadata and slots from the projected
   descriptor. Add `aload` parameter reads and exact `ThisType` value lowering;
   carry exact reference state into CFG analysis and initial frames.
3. Add reference method results, matching typed-body flow, `areturn`, and
   descriptor-appropriate frame checks. Reject every unknown or mismatched
   class before emission.
4. Extend planned virtual calls to reference parameters/results, popping
   arguments in reverse descriptor order and checking exact receiver, owner,
   parameter, and result identities.
5. Enable only same-exact-class reference `If` joins and their precise
   `StackMapTable` frames. Keep different classes, uninitialized objects,
   external classes, hierarchy joins, reference fields, reference constructor
   arguments, arrays, and null rejected.

The dependency order is `1 -> (2, 3) -> 4 -> 5`; steps 2 and 3 both depend on
canonical signature identities from step 1. These increments should use real
source integration fixtures plus class-file structural checks and JDK 21+
`java -Xverify:all` before any reference-signature feature is enabled.
