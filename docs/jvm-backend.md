# JVM backend boundary

`dotty-jvm` is the adapter boundary between a completed typed frontend and
bounded JVM lowering. It depends on `dotty-core` for canonical symbols, types,
and typed trees, and on `dotty-classfile` for class-file descriptor and output
APIs. The crate validates a borrowed `JvmCompilationUnit` contract, projects a
bounded class and method subset to JVM metadata, lowers selected typed
expressions to the class-file crate's symbolic code builder, and composes a
complete class file when the frontend supplies sufficient initialization
evidence. It does not assemble a source frontend.

The root test adapter exercises the real lexer, parser, namer and typer with
one semantic store and one `JvmCompilationUnit`; `CounterObjects.scala` emits
the cooperating `Counter` and `CounterClient` classes from one source session.
This is end-to-end test orchestration, not a public source compiler or a
classpath-aware JVM backend.

Frontend assembly remains outside `dotty-jvm`. A future compiler/session layer
is responsible for parsing, naming, typing, selecting class and method plans,
and preserving source parameter order. Backend validation does not rediscover
symbols by name or mutate or copy the frontend's semantic stores.

`JvmClassPlan::class`, `JvmMethodPlan::method`, and each parameter are canonical
`SymbolId`s from the supplied `SemanticStore`. Parameters are listed in the
order the frontend intends to lower them. The body field is a root in the exact
`TypedAst` arena supplied with the unit. The current shared semantic model does
not record an authoritative mapping from a typed method symbol to a typed body
root or a formal parameter ordering, so validation can check a parameter's
kind and owner and check that a body root exists, but it relies on the
frontend to supply the correct formal parameters, order, and method/body
association.

Each method plan may also carry `JvmLocalBinding` entries for ordinary local
`val` and `var` declarations. A binding pairs the exact typed `ValDef`
`TreeId<Typed>` with the existing canonical `SymbolId::Local` whose owner is
the planned method. The Typer source adapter obtains the tree identity from
`SourceTypedIndex` and the symbol from `SourceTyper::local_symbol_at` through
`typed_local_binding_at`; it does not search by a name, span, or tree order.
The adapter records mutability from the symbol flags and orders bindings by
typed declaration ID for deterministic plan validation. Bindings live only in
their containing method plan. Validation rejects stale or non-`ValDef` trees,
foreign owners, duplicate declaration or symbol IDs, parameter reuse,
incomplete types, type or mutability disagreement, and unsupported lazy,
given, implicit, erased, or inline flags. The semantic store and typed arena
remain borrowed and read-only.

`JvmCompilationUnit::local_layout` builds a deterministic storage layout for a
planned method after validating the complete unit. The explicit
`local_layout_for` entry point accepts that unit's projected
`JvmMethodMetadata` and exact local binding slice; stale metadata or a binding
slice that differs from the method plan is rejected. Locals are projected from
completed `Int`, `Boolean`, or exact planned-class reference types. The instance receiver stays
in slot zero, parameter slots retain their descriptor-derived positions, and
locals receive one category-one slot each in typed declaration ID order. The
allocator is monotonic and does not reuse slots for shadowed locals. Its
`max_locals` count includes the receiver and parameters and is checked against
the u16 class-file field before each slot is published. The layout carries the
canonical `SymbolId`, typed declaration ID, exact value type, and mutability;
lookup is by `SymbolId` only. Reserving a slot does not establish definite
initialization. Planned method lowering accepts ordinary local `ValDef`
statements with an initializer: it lowers the RHS once, checks the resulting
primitive type, then emits one `istore` and marks that symbol initialized.
Assignments to already initialized mutable `Int` and `Boolean` locals inspect
the exact local symbol without loading it, lower the RHS once, and emit one
`istore`; the expression leaves Unit on the operand stack. Immutable locals,
parameters, field selections outside the supported current-instance form,
unsupported types, and locals not initialized on every reachable path are
rejected.
Missing bindings, missing initializers, unsupported types and modifiers,
initializer type mismatches, and detectable reads before the store fail with
typed errors. `PatDef` remains unsupported. Initialized primitive
local reads lower through the same canonical `SymbolId` lookup to one `iload`
from the allocated slot. Local loads require the current method's validated
layout, a completed supported semantic type, and a preceding store on every
reachable path. Name-only references, foreign or unplanned locals, and reads before
initialization return typed errors. Reference slots are supported only for
initialized immutable locals of an exact planned class. Their states may cross
CFG edges when every read is definitely initialized and each stack join has
the exact same class. Mutable reference locals and hierarchy merges remain
typed errors. The classfile verifier and mandatory JDK gate check these paths.

The initial plan subset accepts `SymbolKind::Class`, `SymbolKind::Method`, and
`SymbolKind::Parameter`; other kinds return a structured unsupported-kind
error. Validation rejects duplicate class or method plans, missing semantic
info, stale IDs, and typed nodes with missing, unfilled, or sentinel types. It
checks every allocated typed AST node in arena order, then
returns the first error according to the documented plan order. It does not
walk or validate references nested inside semantic types, since that is the
responsibility of the canonical semantic model's producers. All operations
are read-only; the lifetime-bound references keep the caller as the owner.

`JvmCompilationUnit::class_hierarchy` projects a read-only proof for each
planned class in plan order. Each parent must be one canonical, unqualified
`TypeRef` to a class symbol. `java/lang/Object` is the required root; a
planned ancestry may contain at most three ordinary user classes (`Base`,
`Middle`, and `Leaf`) and two non-Object edges. The bounded graph rejects
cycles, self or multiple parents, missing plans,
unsupported/final/non-public parents, and parent self types. It requires an
accessible no-argument constructor unless a primary-constructor plan carries
a validated same-session direct-super proof for the selected public parent
primary constructor. Every edge and its parent's constructor proof is checked
independently. The proof retains child and parent `SymbolId`s alongside their
checked JVM internal names, and can also be projected to
internal-name-only edges for downstream class-file checks. Unit-aware class
metadata carries the matching direct-parent proof. Whole-class emission uses it for the class-file
`super_class` and emits either `<init>()V` or the proved parameterized
`<init>` descriptor. The supported graph is limited to the canonical Object
root and a maximum three-class same-unit chain of public ordinary classes with
public, non-final parents. A fourth user class, external chains, unavailable
constructors, and other unsupported shapes fail before bytes are returned.
The source-backed `ThreeLevelConstructorSuper.scala` regression composes this
proof with parameterized constructor forwarding and direct-super calls:
`ChainBase -> Object`, `ChainMiddle -> ChainBase`, and
`ChainLeaf -> ChainMiddle`. Each constructor invokes its immediate parent
once before its own initializers; `super.value()` uses the immediate parent as
its `invokespecial` owner. Inherited `ChainBase.doubled()` remains one method
body and uses `invokevirtual` for dynamic dispatch on a `ChainLeaf`.
The full `MultilevelInheritance.scala` source regression combines those paths
with both source-typed `Middle`/`Leaf` conditional branch orders, a `Leaf` to
`Base` call and return, decoded `Object(Middle)` frames, deterministic output,
and a JDK 21+ execution harness. CI sets `DOTTY_CLASSFILE_REQUIRE_JDK=1` for
the source compilation tests so missing JDK tools fail the gate.
Exact planned class references in unit-aware method, constructor, and field
metadata require the complete unit hierarchy proof; standalone projections
remain restricted to metadata they can validate locally. The projection also
rejects a child method with the same JVM name and descriptor as a projected
non-private final method in any validated ancestor. If an ancestor method is
not planned, the backend conservatively rejects a child method with the same
name. Override conformance and inherited dispatch are checked across this
bounded chain; source-typed ancestor reference `If` joins use that same
validated proof.

## Method descriptor projection

`JvmCompilationUnit::metadata` supports method parameters and results that
resolve to exact classes in its validated emission unit. It retains each
canonical `SymbolId` and uses the class plan's validated internal name to build
the JVM object descriptor. Reference parameter identifiers and unqualified
current `this` can be loaded as exact reference values. Method bodies also
emit exact reference returns and planned `invokevirtual` calls with reference
receivers, arguments, and results. The audit and verifier contract are recorded in
[`jvm-reference-signatures.md`](jvm-reference-signatures.md).

The unit-aware projection reads the method's completed `Type::Method` and the
ordered parameter symbols from its plan. It supports one ordinary monomorphic
parameter clause, `Definitions.int` (`I`), `Definitions.boolean` (`Z`), exact
planned class references (`L<internal-name>;`), and result-only
`Definitions.unit` (`V`). Integer and Boolean singleton constants widen to
their corresponding primitive descriptor without changing the source type
arena. Int and Boolean both use the JVM int-like category-one operand
representation, while their descriptors remain distinct. The standalone
store-only `JvmMethodPlan::descriptor` remains primitive-only because it has no
validated emission-unit context.

The descriptor path checks class/method/parameter kinds and ownership,
completed semantic info, method/plan parameter counts, and agreement between
each parameter symbol type and its method signature type. Reference types must
be canonical `TypeRef`s to supported class symbols present in the same
validated emission unit; package prefixes and internal names are checked
against those symbols. Every accepted value occupies one slot. The descriptor
path counts the instance receiver toward the JVMS 255-slot limit; static
methods do not add a receiver slot. The result is parsed by `dotty-classfile`
before it is returned.

Polymorphic or curried methods, non-plain parameter clauses, aliases, type
parameters, match/refined/union/intersection types, Scala objects, arrays,
by-name and repeated types, non-primitive constants, Unit parameters, erased
parameters, and varargs are rejected with structured errors. Reference fields
and primary constructor parameters remain outside method-signature projection.
Unknown types never fall back to `java/lang/Object`.

## Private field metadata

`JvmCompilationUnit::metadata` projects ordinary private `val` and `var`
fields whose values are `Int`, `Boolean`, or an exact class planned in the
same unit. Reference fields retain `JvmType::Reference(SymbolId)` and receive
the descriptor built from that class identity's checked internal name. The
metadata also retains the exact value type, so later stages do not need to
recover it from descriptor text. Immutable vals use `ACC_PRIVATE | ACC_FINAL`;
mutable vars use `ACC_PRIVATE`.

A reference initializer must be a typed identifier for a parameter of a
constructor owned by the field's class, and the parameter's exact class must
match the declared field type. Classes absent from the compilation unit,
`this`, arbitrary reference expressions, arrays, applied types, and
`private[this]` remain unsupported. Reference metadata requires the complete
unit; the standalone `JvmClassPlan::metadata_with_typed_ast` entry point stays
primitive-only because it cannot prove that a referenced class is planned.

The constructor initializer subset also supports copying an exact planned
reference from an unqualified constructor-owned parameter into a private
reference field of the same class. It emits `aload` from the parameter's
validated slot and `putfield` with the field's checked `L...;` descriptor,
after the validated direct-parent constructor call. Primitive literals, parameters, approved primitive
operations, and earlier initialized primitive fields keep their existing
behavior. Reference field reads, `this` escape, nested allocations, calls, and
control flow in field initializers remain unsupported.

No general Scala type erasure, reference boxing, Unit operand-stack
representation, raw bytecode, source pipeline orchestration, TASTy integration,
or macro support is part of this contract.

## Bounded reference values

`JvmType::Reference(SymbolId)` carries the exact canonical identity of one
ordinary, top-level, non-generic class in the validated emission unit. The
checked `JvmCompilationUnit::reference_type` projection accepts only a
canonical `TypeRef` to a planned class with supported metadata and a complete
validated Object-rooted ancestry of up to two user-class edges. It rejects
`Object` itself, external or unplanned
classes, anonymous/generic classes, aliases, unions, applied types, `TermRef`s,
and classes with unvalidated or unsupported parent shapes. `local_value_type` applies the same rule
to a completed local symbol type. Both projections are read-only and do not
allocate semantic symbols or types.

References occupy one category-one slot and remain distinct from `Int`,
`Boolean`, and `Unit`. A planned method may initialize an immutable reference
`val` from a completed, validated `new C(args)`, store it with `astore`, and
read it by canonical local `SymbolId` with `aload`. A reference method
parameter can also be read with `aload` from its descriptor-assigned slot, and
unqualified `this` can be read with `aload_0` after its `ThisType` matches the
current planned class. Exact planned references may be returned with `areturn`,
passed to virtual calls, used as call results or receivers, and discarded with
`pop`. The local's checked class identity must match the initializer's
`Flow::Value`, except for an assignment proved through up to two validated
same-unit ancestry edges; the local and descriptor keep the declared class
identity. Read-before-store and mutable reference locals fail
with typed errors. Reference values may be discarded as category-one
block statements with `pop`. Planned reference parameters and results, plus
reference virtual-call arguments and results, are supported. Primary
constructor metadata retains exact parameter descriptors for classes in the
same compilation unit. Method-body constructor calls may pass a reference
parameter, an initialized immutable local of a planned class, or the current
unqualified `this` when it is assignable to the constructor parameter by exact
identity or up to two validated same-unit ancestry edges. These references may
appear between primitive arguments; each is loaded once in source order as part of
the `new`/`dup`/arguments/`invokespecial` sequence.
Mutable reference locals, fields, nested allocations, method-call results, and
branching expressions remain unsupported as constructor arguments. The
uninitialized object and argument evaluation must complete in one basic block.
Exact initialized references may stay live through `if` joins and `while`
headers/back-edges. A source-typed reference `if` may join two different
initialized classes only when the validated unit hierarchy proves that one is
an ancestor of the other and both are assignable to the `If` tree's canonical
result class. The result stays that source-selected class, with no cast or
guessed common parent. Sibling `Type::Or` results now have a separate checked
projection proof when an explicit expected class is their nearest non-`Object`
common ancestor. Method results, block results, and call arguments consume the
proof only when their canonical source signature supplies that expected type
and the two actual branches match the exact union arms. The typed source union
stays unchanged while expression lowering emits the proven common class for
the CFG verifier. Inferred unions, `Object`-only joins, or mismatched arms
remain unsupported. A local that is not initialized on every path becomes
unusable, and a later read fails.

Allocation may occur in a branch or loop body only as one complete
`new`/`dup`/supported-argument/`invokespecial` sequence inside a basic block.
Arguments may be primitive values or the checked reference values described
above. The uninitialized object cannot cross a branch edge, enter a
local, or escape to another operation. A completed allocation becomes the
exact planned class identity. The classfile CFG analyzer and frame generator
validate generated methods before class bytes are returned; malformed or
unsupported states remain typed build errors.

| Source shape | Planned JVM lowering | CFG/verifier rule |
| --- | --- | --- |
| Immutable `val` of planned class `C` | `new`/`dup`/constructor/`astore`; later `aload` | Initialized exact `C` may cross joins and loop edges; local must be initialized on every read path |
| Private primitive or planned-reference field on current `this` | `aload(0)` with checked `getfield`/`putfield` | Exact owner and declared value descriptor; a field write may use up to two validated same-unit ancestry edges |
| Planned virtual call on `this`, a reference parameter, `new C`, an immutable local, or a completed reference-valued call | Canonical `invokevirtual` target | Receiver owner is exact or a validated ancestor; arguments and results use exact types or the bounded two-edge assignment |
| Reference parameter identifier or unqualified `this` value | Exact `aload(slot)` or `aload_0` | Parameter owner/type and `ThisType` must match the current validated plans |
| Constructor argument from a reference parameter, initialized immutable local, or current `this` | Exact `aload(slot)`/`aload_0` in source order before `invokespecial` | Argument class must match or have the verified ancestry relation of up to two edges; full allocation sequence remains in one basic block |
| Allocation in an `if` arm or loop body | Complete allocation and constructor call in one basic block | No uninitialized value may cross an edge, enter a local, or escape |
| Ancestor-related initialized references at a typed `If` join | Keep the canonical source result class | Both branch classes must be ancestor-related and assignable to that result; classfile frames verify the join |
| Sibling union at a join | Emit the checked nearest non-`Object` ancestor for an exact two-arm planned-class `Type::Or` when a canonical method result, block result, or formal parameter supplies that expected type | Actual branch classes must match both union arms; no inferred target or AST rewrite; `Object`-only, forged, malformed, nested unsupported, and non-canonical cases fail |
| Unrelated or non-canonical reference types at a join | No classfile emitted | Typed reference/branch error; no guessed superclass, `Object` widening, cast, or name-based projection |

Object-construction typing, canonical constructor and receiver identity,
uninitialized-reference verifier constraints, and the multi-class planning
boundary are recorded separately in
[`jvm-object-creation-contract.md`](jvm-object-creation-contract.md). That
contract records the verifier constraints that bound the emitted subset.

## Planned virtual instance calls

`JvmCompilationUnit::resolve_planned_instance_call` resolves a typed regular
`Apply(Select(receiver, method), args)` to the exact planned method symbol in
its declaring class and validates that owner against the receiver's class or
validated ancestry. Receivers are current `this`, a
validated direct `new C(args)`, an immutable local or parameter, or a completed
planned call returning an exact reference class. Public methods may be called across emitted
classes; plain private methods are callable only from their owning class.
Qualified access, protected/package access, static and extension methods,
bridges, traits, external classes, and name-only fallback are rejected.

Method parameters and results use primitive `Int`/`Boolean`, exact planned
references, and `Unit` represented as JVM `void`. Lowering evaluates the receiver
once, then each argument once in source order, checks their stack flows against
the selected descriptor, and emits one symbolic `invokevirtual` with the exact
owner, name, and descriptor. Primitive and reference results leave their exact
value on the stack; `Unit` leaves no value. The source typer accepts direct-new selection
for ordinary method application. Calls on exact planned reference receivers
and completed allocations are also supported in `if`, `while`, and short-
circuit branches. The target still comes from the canonical planned method
symbol; external, interface, private cross-class, and other unplanned receivers
remain rejected.

### Real-source object interaction gate

[`tests/fixtures/jvm/CounterObjects.scala`](../tests/fixtures/jvm/CounterObjects.scala)
is the bounded end-to-end object/reference fixture. One source session plans
both classes, uses the constructor-owned `start` parameter to initialize
Counter's private mutable `current` field, and types CounterClient's
constructor calls, canonical local bindings, and selected `increment`/`get`
method targets. The test checks complete classfiles, classfile major version
65, exact primitive method descriptors, and instruction order for
construction, post-constructor local stores, virtual calls, and integer
returns. It recompiles with a fresh frontend session and requires
byte-for-byte identical outputs in source order. A Java harness constructs
only CounterClient and checks `run(10) == 11`, `run(0) == 1`, `direct() == 42`,
and `independent() == 31` under `java -Xverify:all`.

The mandatory `Classfile JVM encoder gate` in `.github/workflows/ci.yml` sets
`DOTTY_CLASSFILE_REQUIRE_JDK=1` and runs the source integration test on JDK 21.
Missing `java`, `javac`, or `javap` fails that gate. This tested boundary keeps
arbitrary classpath classes, mutable reference locals, sibling/unrelated
reference joins without the exact expected-type proof, arrays, null, and
branching constructor bodies unsupported.
Reference operations remain restricted to exact planned classes and verified
calls; ancestor joins are limited to the source-typed `If` result and validated
same-unit hierarchy.

## Canonical `Int` arithmetic

Method bodies resolve typed regular applications against the canonical
`scala.Int` operator `SymbolId`s before applying the ordinary instance-call
restrictions. `+`, `-`, `*`, `/`, and `%` lower their selected receiver first,
then their sole argument, exactly once each, and emit `iadd`, `isub`, `imul`,
`idiv`, or `irem`. Both operands and the application result must have the
canonical `Int` semantic type and produce JVM `Int` values. Nested arithmetic,
local initializers and assignments, and arithmetic in the existing integer
`If`/`While` control-flow subset use the same recursive lowering path.

These intrinsic calls do not emit `invokevirtual` or a method reference for the
Scala operator. Calls whose symbol is not in the canonical registry continue
through planned instance-call resolution and retain its receiver and
control-flow restrictions. Standalone expression lowering can emit canonical
Int and Boolean intrinsics without a compilation-unit call context; ordinary
applications still require a validated planned target. Canonical relational
operators `<`, `<=`, `>`, and `>=`
emit their signed `if_icmp*` branch to a true label, materialize `0` on the
false path and `1` on the true path, then join. The classfile builder computes
branch offsets and `StackMapTable` frames. The Boolean values work as results,
local values, and conditions in `if` and `while`. This compositional subset
includes initialized integer locals updated by arithmetic assignments across
multiple loop iterations, comparison conditions, nested Boolean branches, and
`if` branches that return arithmetic values. Loop locals must be initialized
before the loop; a local initialized only in the body is not assumed to exist
on the zero-iteration exit. Planned calls with exact reference receivers are
supported when the CFG analyzer proves their initialized class states.
Universal equality operators `Any.==` and
`Any.!=` remain unsupported. JVM overflow, signed division/remainder, and division-by-zero
behavior are preserved by emitting the corresponding JVM instruction without
constant folding or prechecks.

`tests/fixtures/jvm/Calculator.scala` validates this subset through the real
lexer, parser, namer, typer, JVM backend, and JDK 21+ runtime. Its Java harness
checks arithmetic overflow, signed comparison boundaries, divide-by-zero
exceptions, and repeated `sumTo` loop execution with `java -Xverify:all`.
General Scala library lowering, Long/Float/Double, Java classpath loading,
TASTy emission, the full Scala ABI, universal equality, and arbitrary
reference-bearing calls remain outside this subset.

## Canonical Boolean intrinsic resolution

`resolve_boolean_intrinsic` is a read-only semantic resolver. It recognizes
the eight Boolean primitive members only through their canonical `SymbolId`s
from `Definitions`, checks their owner, signature, receiver, argument, result,
and tree shape, then reports a `JvmBooleanIntrinsic`. A method on a user class
with the same name and signature, and universal `Any.==`/`Any.!=`, remain
ordinary calls. `unary_!` is a parameterless `Select`; the binary members are
regular `Apply(Select(receiver, member), [argument])` trees.

The intrinsic value reports whether the operation has no RHS, evaluates it
strictly, or short-circuits it. Resolution does not emit bytecode, mutate the
semantic store or typed tree, or route short-circuit operators through
`invokevirtual`. Backend lowering emits unary `!` as an int-like XOR with 1;
eager `&`, `|`, and `^` evaluate the selected receiver and argument once in
source order before `iand`, `ior`, or `ixor`. Canonical Boolean `==` and `!=`
compare their int-like values and materialize a normalized 0/1 result.
Canonical `&&` and `||` branch after evaluating the receiver, evaluate the
argument only on the required path, and join with a normalized 0/1 result.
Locals initialized only while evaluating the argument are not considered
initialized after the join. Short-circuit operators compose with supported
locals, assignments, blocks, and integer-like `If`/`While` flow; ordinary calls
inside branched CFGs keep their existing restriction. Universal
`Any.==`/`Any.!=` and user-defined same-spelled methods are not substituted.

## Class and method metadata subset

`JvmClassPlan::identity` derives top-level class names by following canonical
package owner symbols to the unnamed root. It supports `Answer` and
`example/api/Answer`, public concrete non-generic classes, and the JVM `final`
class flag. Nested/local classes, traits, objects, module classes, enums,
synthetic/case/sealed/abstract classes, malformed class info, and non-package
owner chains return typed errors. Names are kept unmangled; only simple
identifier names are accepted, so Scala operators and encoded names such as
`$plus` are rejected.

`JvmClassPlan::metadata` combines that identity with selected method plans.
It accepts public ordinary instance methods and maps `final` where present.
Static, abstract, synthetic, or otherwise unsupported method flags and
non-public visibility are rejected. The method name and descriptor come from
the exact method `SymbolId`; overload identity is the pair of JVM name and
descriptor, and duplicate pairs on a class are rejected. A compilation unit
also rejects distinct class symbols with the same emitted internal name.

`JvmClassPlan::fields` is an ordered frontend-issued list of
`JvmFieldPlan { field, source_visibility, initializer }` bindings. `field` is
the existing class-owned `SymbolKind::Field` ID and `initializer` is the exact
typed RHS obtained from the same frontend session's source-to-typed mapping.
The frontend records whether source visibility was plain `private` or
`private[this]`: `Visibility::Private` alone cannot distinguish those forms,
and only plain `private` is accepted. The
frontend owns that association because `dotty-jvm` must not depend on the
Typer or rediscover it from names, spans, or allocation order. The compilation
unit metadata path checks that the field belongs to the planned class and its
actual `ClassInfo::declarations` scope, that the supplied RHS exists in the
unit's typed arena, and that its widened primitive value matches the
completed field type.

Only plain-private, non-static, non-lazy, non-synthetic instance fields of
ordinary classes whose sole parent is canonical `java.lang.Object` project to
metadata. Exact `Int` and `Boolean` types produce `I` and `Z`. Immutable
fields produce `ACC_PRIVATE | ACC_FINAL`; mutable fields produce
`ACC_PRIVATE` without `ACC_FINAL`. Metadata preserves the caller's field plan
order and rejects repeated field symbols and duplicate JVM field names. Whole
class emission installs these fields only when a validated primary constructor
plan proves their initialization. `ImplicitNoArg` remains restricted to
empty-field classes and preserves the previous public `<init>()V` shape.

Method-body lowering accepts assignments to planned mutable primitive and
exact-reference fields
that resolve canonically to the current class's unqualified field identifier or
`this.field`. It validates field ownership, plan membership, mutability and
exact field value type, then emits `aload_0`, the RHS once, and `putfield`; a
successful assignment completes with Unit and no value on the operand stack.
Local assignments continue to use their canonical local slot and `istore`,
including when a local shadows a field. Immutable fields and arbitrary
receivers return typed errors; CFG validation checks temporary states across
branch edges.
`emit_class` carries these field references into the classfile after validating
the field and constructor plans.

The backend treats each field/RHS pair as frontend proof. A `TreeId<Typed>`
does not carry a session tag, so the producer must obtain the ID from the same
typed arena it passes in the compilation unit. For direct-super constructor
plans, the backend also receives the shared `dotty-core::SourceTypedIndex` and
checks that the source application and every source argument map to the exact
typed roots supplied by the frontend. This shared contract preserves the
frontend dependency boundary while rejecting mixed source/typed roots.

Formal parameter identities stay in the plan's explicit order and are checked
against the canonical method owner, completed parameter info, and descriptor
types. Instance local slot zero is reserved for `this`; supported `Int` and
`Boolean` parameters occupy one slot each, beginning at slot one. Each output
parameter records its type, first slot, and width so later category-two types
can extend the layout contract without changing identity semantics.

`JvmCompilationUnit::resolve_same_class_call` accepts a complete typed
`Apply` tree plus the planned caller class and method. It resolves only a
canonical `TermRefTarget::Symbol` carried by an `Ident` or a `Select` on that
class's `this`. Ident references must have either `NoPrefix` or that class's
`ThisType` as their canonical term-reference prefix; selected references must
have that class's `ThisType`. Other prefixes return a typed unsupported-shape
error. The resolver then requires the target to belong to the same class's
explicit emission plan and declaration scope. The result carries the exact
owner internal name, JVM method name and descriptor, canonical method symbol,
`Virtual` invocation kind, and result contract. The typed call result must
agree with the projected descriptor. Contextual `using` calls, type
applications, other receivers, constructors, static or extension methods,
unresolved name-designed references, and parameterless methods without an
explicit empty clause return typed errors. This API resolves metadata only:
it does not check argument arity or types, evaluate arguments, load `this`, or
emit an invocation instruction.

`JvmCompilationUnit::lower_same_class_call_receiver` composes that validated
call context with receiver lowering. A `Select` receiver must be the current
class's unqualified `this`; an `Ident` is accepted only after the resolver has
proved it names a planned instance method. The method returns a fresh builder
containing exactly one `aload(0)`. Callers append argument evaluation after
that builder so the receiver precedes the arguments. Qualified `this`,
`super`, other receiver expressions, and static caller methods return typed
errors. This API emits neither arguments nor the invocation and does not
introduce general JVM reference-value lowering.

Planned method-body lowering in `emit_class` accepts a regular `Apply` whose
target passes `resolve_planned_instance_call`. It evaluates the selected
receiver once before lowering each argument once in source order, checks their
stack values against the exact descriptor, and emits one symbolic
`invokevirtual`. Int/Boolean results leave one int-like value; Unit leaves no
value. Arity, receiver-flow, argument-flow, target, and typed-result mismatches
return structured errors, and a failed method does not produce a class
artifact. Standalone expression/body lowering has no emission plan, so it
cannot resolve ordinary `Apply` targets.

An explicit unqualified `super.method(args)` follows a separate checked path.
`resolve_direct_super_call` requires the typed receiver to be
`Super(This)` with no mixin qualifier, verifies its `SuperType` against the
current class and planned direct parent, and retains the selected canonical
method symbol. The selected method must be public, concrete, planned, and
effective on the direct parent. Lowering pushes `aload(0)`, evaluates each
argument once in source order against the exact descriptor, and emits one
`invokespecial` using the direct parent's JVM owner and the selected method
name and descriptor. The result uses the selected method's validated return type.
Qualified/mixin super, mismatched semantic types or owners, unplanned methods,
and unsupported method signatures fail with typed errors before a class is
published. Ordinary calls continue through the virtual resolver and emit
`invokevirtual`; an explicit super call cannot fall back to virtual dispatch.
The source integration test checks primitive and reference results, Boolean
arguments/results, a branched body with stack-map frames, deterministic class
bytes, and JDK 21 `-Xverify:all` execution in CI.

Planned method-body lowering allows initialized primitive local references in
the final expression or other value positions, and immutable planned-class
reference locals in supported straight-line blocks. Standalone expression APIs have
no local layout and return `MissingLocalBinding` for local references. Typed
`Block` lowering evaluates supported stats from left to right, then
lowers the final expression once. An unused Int/Boolean value is discarded
with one `pop`; Unit already leaves an empty operand stack and needs no pop.
The final value remains available to its parent expression or method return,
including exact planned reference results. Unused initialized references are
discarded with one `pop`.
If a stat definitely returns, lowering stops there and omits the unreachable
suffix, so dead instructions do not reach CFG analysis. A normally completing
Unit method receives one trailing `return`; explicit returns remain terminal.
The block's declared type is checked against its final expression flow.
Planned method lowering supports initialized primitive `ValDef` statements,
immutable reference `ValDef` statements initialized by validated object
creation, assignments to initialized mutable primitive locals, and typed
`While` loops with Boolean conditions. A loop evaluates its condition at the symbolic header
on every iteration, branches to its exit when false, and emits a back-edge only
when the body can fall through. Unit bodies leave the stack empty; Int/Boolean
body values are discarded once. The condition and body share the existing
depth/node budgets. Since a loop may execute
zero times, initialization performed only in its body is not considered
definite after the loop; the class-file CFG analyzer verifies header/back-edge
local states and generates stack-map frames. Nested loops and `If`/`Block`/
`Assign` bodies are supported when their verifier states merge. Exact planned
reference locals, current-instance private fields, planned calls on
reference receivers, and complete branch-local allocations use the same CFG
validation. `PatDef` and other unsupported stats return structured errors.
Standalone expression lowering remains limited by its lack of a validated
class plan and local layout.

`JvmCompilationUnit::lower_method_body(class, method, definitions)` also lowers
reads of fields in that class's validated field plan. A canonical unqualified
field `Ident` or a `Select` whose qualifier is that class's unqualified
`ThisType` emits `aload(0)` followed by symbolic `getfield` using the validated
owner, field name, and exact `I`/`Z`/`L...;` descriptor. Reference reads
produce the exact planned `JvmType::Reference(SymbolId)` flow. Assignments to
mutable reference fields evaluate the RHS once, spill and reload it with
`astore`/`aload`, then emit checked `putfield`; immutable fields remain
rejected. A planned reference field can also be a receiver for a canonical
planned `invokevirtual` method. Resolution uses the `TermRefTarget::Symbol`
identity, field owner, current-instance prefix, and planned metadata;
same-spelling parameters and locals keep their own loads. Other receivers,
fields absent from the plan, stale targets, and prefixes that do not identify
the current instance return contextual errors. Field reads and writes may
occur in `If` and `While`; the exact field type and temporary locals are
checked at branch targets and loop edges. Straight-line `Block` results may
carry the type of a planned field into this same validated read path.
Whole-class emission
uses this lowering path for planned methods and emits no artifact if any
method fails.

This metadata layer does not encode Scala operator names, nested-class `$`
names, companion/module classes, trait forwarders, bridges, private-access
semantics, constructor bodies, classfile hierarchy, or bytecode. Unsupported ABI
forms must be handled by a separate contract before they can be emitted.

## Whole-class emission

`emit_class` composes one planned class into a class file at
`JVM_CLASSFILE_MAJOR_VERSION` (65). The artifact contains an ordinary class
with the direct parent from its validated unit proof, validated fields in
frontend source order, one constructor, and planned instance methods in plan
order. `ImplicitNoArg` emits the public `<init>()V` and accepts no fields.
`PrimaryConstructor` emits the exact validated descriptor and public/private
access from canonical constructor metadata. Both constructor forms begin with
one `aload_0; invokespecial <direct-parent>.<init>()V`; primary constructors
then append each source-ordered field initializer and return. No parameterized
class gets an extra `()V`. Method bodies use the same typed lowering path as
before. The file is encoded into an owned `GeneratedClass` only after every
method and class-file structure has been built successfully.

The frontend must set `JvmClassInitializationPlan::ImplicitNoArg` only after
proving the source class has no constructor parameters or body, superclass
arguments, fields, or template initialization expressions. A direct parent is
allowed only when the unit hierarchy proves its canonical no-argument
constructor chain. The backend checks that the class's completed `ClassInfo`
has exactly the parent from that proof and has no self type. It checks that every field and method declaration
is represented by the supplied plan. A missing plan, a plan marked
`Unsupported`, an unplanned member, or a class shape outside the supported
subset returns a typed error; the backend does not infer initialization
safety from an empty method list.

A parameterized direct-super call is supported only when the source frontend
provides the exact original parent `Apply`, its same-session typed roots, the
canonical parent class and primary-constructor symbols, and source-ordered
argument bindings. The backend accepts one plain argument clause containing
only Int/Boolean literals or exact primary-constructor parameter identifiers
whose types map to planned JVM classes or primitives. It emits `aload_0`, the
arguments in source order, and `invokespecial Parent.<init>(descriptor)V`
before child field initializers. Other expressions, side effects, extra clauses,
additional constructors, and unplanned reference classes fail before bytes
are returned.

The emitted class has no interfaces, class initializer, Scala/TASTy metadata,
or unplanned methods. Deeper or external parent chains, nested types, qualified
or trait super, and other class initialization semantics remain unsupported. The API
returns only after encoding succeeds, so a failure does not expose a partial
artifact.

The lower-level `dotty-classfile::CodeBuilder::pop` operation discards one
category-one JVM stack value. Linear analysis accepts represented
category-one values and rejects category-two values; the current CFG analyzer
supports it within its integer-only stack model. This bytecode primitive does
not infer Scala expression effects or add source block lowering to `dotty-jvm`.

The CI JVM gate writes this generated class to disk, inspects it with `javap`,
compiles a small Java caller with `javac`, and executes it with
`java -Xverify:all` on JDK 21 or newer.

## Source frontend integration gate

The root crate's `jvm_source_compilation` integration test is the bounded
source-to-execution path. It reads `tests/fixtures/jvm/Answer.scala` and
`Delegator.scala`, runs each through the lexer, parser, namer, and typer, and
builds JVM plans from canonical `SourceSemanticIndex` identities while
preserving source parameter order. `Answer` covers primitive results and
conditional lowering. `Delegator` covers same-class direct and explicit
`this` calls, nested calls, explicit empty-clause calls, and a statement block
that discards the first call result and returns the second. Its output is
compared byte-for-byte across independent frontend runs; decoded class-file
references and instruction order are checked without hand-authored typed
trees or constant-pool indexes. `WhileLocals` exercises zero and one iteration,
mutable Boolean conditions, nested `If`/`While`, returning bodies, and discarded
primitive body values. `LocalState.scala` is compiled twice through fresh
frontend sessions and output directories; its complete class bytes must match.
It covers parameter-copying `val`, reassignment of a mutable `var`, a source
`while`, a local shadowing a parameter, and nested blocks with same-named
locals. The test asserts method descriptors, slot-specific loads and stores,
`max_stack`, `max_locals`, branch targets, sorted decoded `StackMapTable` frame
PCs, class-file version 65, full class-file decoding, and reference validity.
Java harnesses check all local-state results under `java -Xverify:all`.
`Counter`, `Other`, and `Client` cover direct-new and immutable-local receivers,
same-named methods owned by different classes, ordered primitive arguments,
Boolean and Unit results, and private calls from the owning class. Negative
source cases reject cross-class private access without publishing class files.
`BooleanControlFlow` exercises canonical short-circuit intrinsics through the
source frontend in `if` and `while` conditions, nested eager/short-circuit
expressions, Boolean local initialization and assignment, and side-effecting
operands. Its JVM harness checks zero- and multi-iteration loops, verifies
that short-circuit RHS effects occur only when selected, and confirms that
strict `&`, `|`, and `^` evaluate both operands. Generated methods are checked
against the classfile CFG's stack-map frame targets and run with
`java -Xverify:all` in the JDK gate. `Logic.scala` is the focused complete
operator fixture: two independent source frontend runs must emit identical
class bytes, all decoded references and public instance method descriptors are
checked, and `javap -c -v` verifies the descriptors, branch/Boolean opcodes,
and `StackMapTable`. The Java harness checks short-circuit versus eager
divide-by-zero behavior, Boolean `==`/`!=` truth tables, and side-effect counts
for short-circuit and strict operators.

`planned_reference_operations_verify_across_if_while_and_short_circuit_cfgs`
is the focused reference-CFG integration gate. It covers private Int/Boolean
field reads and writes in branches and loops, immutable reference locals across
loop edges, exact planned virtual calls in conditions and bodies, short-circuit
calls, same-class reference joins, and complete allocations in conditional
arms. It checks generated `StackMapTable` attributes and exercises zero-, one-,
and multi-iteration results with JDK `-Xverify:all`.

`reference_parameters_and_results_compose_with_if_and_while_control_flow`
checks the method boundary cases directly: `Counter` parameters are receivers
through repeated loop iterations, methods return the exact reference selected
by either `if` branch, and a reference-returning call can supply one branch.
The test asserts exact `Object(Counter)` stack joins, parameter locals at loop
back-edge frames, deterministic class bytes, and zero-, one-, and
multi-iteration behavior under the JDK verifier. A typed-body regression also
checks that reference locals initialized in only one branch or loop iteration
remain unreadable after the join or loop exit.

`CounterControlFlow.scala` is the end-to-end reference-CFG fixture. Two
independent lexer-to-backend sessions emit `Counter.class` and
`CounterClient.class` in source order with byte-identical output. The test
decodes both complete classfiles, checks major version 65, exact descriptors,
field and invocation owners, and resolves every branch target to a frame. At
the `incrementUntil` and `run` loop headers it resolves each `Object` frame
entry through the actual constant pool and asserts the exact `Counter` and
`CounterClient` local types. For allocations in `choose`, it checks each
`new`/`dup`/argument/`invokespecial` sequence completes within one basic block.
The JDK gate inspects `javap -c -v` output and runs zero-, one-, and
multi-iteration loops, both allocation branches, and equal/over-limit starting
values under `java -Xverify:all`.

`ReferenceObjectApis.scala` closes the two-class API path from actual Scala
source through the lexer, parser, namer, typer, canonical JVM plans, and class
file encoder. Its `Counter` and `CounterService` methods cover `new`, exact
reference parameters/results, `this`, `if` joins, virtual calls in `while`,
and a reference-returning call passed to another planned call. The source test
checks descriptors, owners, decoded `Object(Counter)` frame entries and branch
PCs; two fresh sessions must emit byte-identical ordered class batches. Its
JDK harness checks reference identity and loop edge cases with
`java -Xverify:all`. See
[`jvm-reference-signatures.md`](jvm-reference-signatures.md) for the fixture's
precise coverage and remaining limits.

`typed_super_method_calls_lower_to_checked_invokespecial` uses actual Scala
source for `Base`, `Derived`, and `Runner` in two independent frontend
sessions. It checks exact `Methodref` owners/descriptors for direct `super`
calls with Int and Boolean arguments, Unit and reference results, and both
branches of an `if`; the branch has a `StackMapTable`. It separately confirms
that `Base.doubled` and `Runner.evaluate` retain `invokevirtual`, rejects
qualified and root-class super shapes before publishing classes, and
requires byte-identical ordered class output. The JDK gate inspects these
instructions with `javap` and executes the methods using JDK 21
`java -Xverify:all`.

Multi-class compilation encodes every planned class in memory before staging or
publishing output files, so semantic, codegen, and encoding failures publish no
classes. If a filesystem write or rename fails during publication, cleanup and
restoration of earlier files are best-effort; the batch is not an atomic OS
filesystem transaction.

`Counter.scala` runs through the same real frontend assembly with a canonical
constructor parameter, private mutable `current:I`, and public `get`, `set`,
and `increment` methods. `PrivateFields.scala` covers private final `I` and
`Z` fields, including true and false Boolean getters. Both stateful fixtures
are compiled in two fresh frontend sessions and their complete class bytes
must match. Their decoded constructor descriptors, field flags and references,
and method instructions are checked structurally; the JDK gate uses
`javap -c -v` and Java harnesses under `-Xverify:all`. Negative real-source
cases cover wrong initializer types, immutable assignment, unsupported
constructor forms, and failed compilation without partial class artifacts. See
[`jvm-stateful-class-contract.md`](jvm-stateful-class-contract.md).

`ReferenceHolder.scala` exercises a three-class source batch with private
reference fields, constructor reference arguments, a reference-returning method
chain (`value().get()`), a `while` loop, and an `if` result join. Its source
integration test compares ordered class bytes from two independent frontend
sessions, checks exact field flags and descriptors, constructor field-store
order, call owners, and `Object(Holder)`/`Object(Counter)` frames. On JDK 21+
the test also runs all three classes through `javap` and a Java identity and
loop harness with `-Xverify:all`. The typer accepts a direct application
returning a top-level class from the current source batch as a selection
prefix; mutable reference-field prefixes such as `current.get()` remain
rejected.

These fixtures test the bounded emitted class shape. They do not establish
Scala public accessor, bridge, or TASTy compatibility.

CI sets `DOTTY_CLASSFILE_REQUIRE_JDK=1`, so missing JDK tools fail the gate.
Negative source tests verify that field initialization, mutable reference
locals, invalid while conditions, wrong exact reference call values, private
construction, immutable-local assignment, missing local references, mismatched
local initializers and assignments, Unit locals, deferred reference locals,
pattern locals, and parser diagnostics fail at their actual pipeline stage
without leaving a `.class` file. Backend unit tests reject missing, foreign, or
wrong-owner local bindings.
Existing backend typed-tree tests cover unsupported receivers, `using` calls,
static/extension methods, mismatched method references and arguments, and other
unsupported call shapes. Run the test locally with:

```sh
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test --test jvm_source_compilation --locked
```

Without the JDK it still runs structural and negative checks and explicitly
skips Java verification. Reference method results outside the exact current
source class subset, reference call arguments outside the exact supported
class subset, sibling joins without an explicit checked expected parent,
unrelated or `Object`-only class joins, arrays, null, mutable reference locals,
and branched constructors remain deferred. Ancestor-related and proven sibling
source-typed `If` results are supported only within the validated same-unit
hierarchy and still require JVM frame verification. The exact `Left`/`Right`
join under a declared `Base` result is covered end to end: both branch orders
emit a CP-resolved `Object(Base)` frame, fresh frontend sessions produce
identical class bytes, and the mandatory JDK 21+ harness verifies runtime class
identity and virtual dispatch with `-Xverify:all`.

This path targets the repository's pinned Scala 3.9.0 source compatibility
reference at revision `777528f19a58e794c9954a42f433373472ec57f8`. It demonstrates
only the explicitly supported frontend and JVM subset; the class has no
`.tasty` payload and does not claim full Scala ABI or `scalac` compatibility.

## Typed expression leaves

`lower_value_expression` accepts typed `Literal` nodes containing `Constant::Int`
or `Constant::Boolean`, and `Ident` nodes whose typed `TermRefTarget::Symbol`
matches one of the method metadata's canonical parameter symbols. Literals
lower through `CodeBuilder::iconst`; parameter references use
`CodeBuilder::iload` at the metadata-assigned instance local slot. These
primitive values use the `IntLike` category while Boolean field descriptors
remain `Z`. Literal types may be the matching singleton constant type or
canonical `Definitions.int` / `.boolean`. Same-class field `Ident` and
current-`this` `Select` reads are available only through the validated
`JvmCompilationUnit::lower_method_body` path described above; standalone
expression APIs do not carry class field metadata.

`lower_expression` also supports a typed `Constant::Unit` literal. It emits no
instruction and reports `JvmExpressionFlow::Unit`, distinct from an int-like
value on the operand stack and from `Diverges`. Nested typed `If` expressions
may join two Unit branches with an empty stack. A Boolean condition is evaluated
once and followed by symbolic `ifeq`/`goto` labels. Value-producing branches
agree on their `Int` or Boolean type and reach a stack-compatible join. A
branch that terminates with an explicit local return contributes no placeholder
value; a `Nothing` branch without supported return flow is rejected. Every
emitted tree must be within the fixed depth budget
`JVM_EXPRESSION_DEPTH_LIMIT` (64 nodes), and depth errors carry the offending
`TreeId` and optional source span. The internal `JvmLoweringContext` shares the
read-only semantic inputs and one expanded-node counter through recursive
lowering. Each call caps expanded visits at `JVM_EXPRESSION_NODE_BUDGET` (4,096);
repeated references to a shared subtree count each time they are expanded.

`lower_method_body` checks the final flow against the method descriptor and
adds `ireturn` for an `Int`/Boolean value fallthrough. A Unit body that
completes normally gets JVM `return`; a body whose reachable paths all return
gets no extra epilogue. Unit never pushes a placeholder value. A Unit body is a
typed flow mismatch for an `Int`/Boolean method. All APIs build into a fresh
method-local `CodeBuilder`, so an error discards partial instructions. The
classfile builder then validates reachable blocks, analyzes stack states,
assembles labels, and writes `StackMapTable` frames.

Calls inside `If`, `While`, and short-circuit expressions are allowed when
their targets and receiver classes come from the validated plan and the
classfile CFG accepts the resulting exact stack/local states. Calls at ordinary
expression roots, as arguments to other calls, and under supported local
returns follow the same receiver and result restrictions.

`Return.from = Some(...)` is rejected. Non-intrinsic `Select`, `TypeApply`,
`Match`, `Try`, `New`, closures, nested `DefDef`s, String/Long and other
non-Int/Boolean literals, object/reference joins, and all other typed nodes
remain unsupported. No source name lookup, implicit boxing, general-purpose
coercion, constant folding, or frontend dependency is introduced.

### Explicit primary constructor metadata

`JvmClassInitializationPlan::PrimaryConstructor` carries a
`JvmPrimaryConstructorPlan` that authorizes emission only after its constructor,
field, class, and typed initializer metadata all validate. Existing frontend
proofs of `ImplicitNoArg` retain their prior empty public `<init>()V` emission
and do not require a new constructor plan.

The frontend collects the constructor through `Template.constructor` and its
parameters through `SourceSemanticIndex::derived_symbol_at(constructor, source,
parameter_tree)`, in source clause order. A constructor with no value-parameter
clause is treated as the zero-parameter form; one ordinary clause is also
supported. `JvmPrimaryConstructorPlan::from_source`
checks those supplied identities against the shared core AST and semantic
index, then retains a private, owned proof. It does not tokenize, parse,
complete symbols, type expressions, or depend on the parser/namer/typer crates.
This source check is necessary because default arguments and parameter
accessor syntax cannot be recovered safely from the completed method type.
The producer must supply the same frontend session's AST, index, and store;
typed IDs themselves do not encode session identity.

`JvmClassMetadata::constructor()` exposes the explicit constructor's canonical
symbol, owner internal name, descriptor, access flags, and ordered
`JvmParameterLocal` values. Semantic constructor results are class types; JVM
constructor results are always `V`. Plain `Int`/`Boolean` parameters occupy
one slot each starting at 1, with slot 0 reserved for `this`. A reference
parameter occupies one slot and is accepted only when its exact class has a
plan in the same `JvmCompilationUnit`; its descriptor uses that class's
validated internal name. Mixed primitive/reference clauses preserve source
order and canonical constructor-owned parameter symbols. At most 254
category-one parameters are supported. Class-owned parameter copies and
`val`/`var` accessor fields never substitute for constructor parameters.

Projection checks the live constructor kind, source origin, owner, declaration
scope, complete plain monomorphic signature, primitive or exact planned-class
parameter types, and
proven public/private visibility. Private constructors stay private. Generic,
secondary, contextual, defaulted, by-name, erased and repeated parameters,
parameter accessors, annotations, qualified visibility, arrays, applied
references, and references outside the unit fail with structured errors.
Store-only `JvmClassPlan::metadata` entry points remain primitive-only because
they cannot prove a complete emission unit. Source proof rejection does not
publish partial metadata or mutate semantic state. Emission creates only the
proved constructor, with its exact descriptor and access flags; it does not add
a fallback constructor, Scala accessors, field stores outside the approved
initializer path, or `new` expressions.

`JvmCompilationUnit::resolve_constructor_call` resolves the exact typed shape
`Apply(Select(New(TypeTree(C)), <init>), args)` to a planned class and its
approved primary constructor metadata. It accepts the proved explicit primary
constructor or a declared public empty `ImplicitNoArg` constructor, requires
the class/result/prefix identities and primitive arguments or exact planned
reference arguments to match, and rejects private cross-class calls. The
returned target is read-only metadata. Constructor-call lowering still rejects
reference-valued arguments; this increment does not emit those argument
instructions. Other application forms and unsupported argument types remain
rejected.

### Constructor field initializer fragments

`JvmCompilationUnit::lower_constructor_field_initializers` consumes the
validated `PrimaryConstructor` metadata and the ordered `JvmFieldPlan` list.
The frontend obtains each field's source RHS from its canonical source
declaration, calls `field_initializer_context_for(field)`, types that RHS once,
and supplies the exact `TreeId<Typed>` retained in `SourceTypedIndex`. It
collects fields by source template order and constructor parameters through
`Template.constructor` plus their derived `SymbolId`s; equal printed names do
not substitute one identity for another.

The result is a checked instruction plan, not a complete constructor. Each
field fragment starts with `aload_0`, lowers one value, and ends with `putfield`
for that canonical field. Supported RHS forms are Int/Boolean literals,
constructor-owned primitive parameters, an exact planned-reference constructor
parameter, canonical `Int.+`, canonical Boolean negation, empty-statement
blocks, and reads of earlier initialized primitive fields. The reference
parameter is matched by its canonical `SymbolId`, constructor owner, exact
class identity, and validated local slot; its load uses `aload`. Self and
forward field reads, reference field reads, user method calls, `this` escape,
control-flow expressions, and other unsupported typed nodes fail with
contextual `JvmCodegenError`s.

`JvmConstructorFieldInitializerPlan::append_to` appends only those field
instructions. `emit_class` emits `aload_0; invokespecial
<direct-parent>.<init>()V` before the plan and appends `return` afterward. It
also installs the validated private field_info entries and uses the exact
constructor descriptor and access flags from metadata.

### Combined super-call fixture

`SuperCalls.scala` covers parameterized Base/Derived constructors together with
explicit super method calls in one real frontend batch. The source integration
test decodes every class, checks exact constructor and method references,
stack/local bounds, absence of phantom no-arg constructors, and two independent
byte-identical builds in Base/Derived/Runner order. A Java 21 harness verifies
constructor forwarding, special parent implementations, mutation, ordinary
virtual override dispatch, and zero/negative inputs with `-Xverify:all`.
The mandatory `jvm_source_compilation` CI suite includes this test; a missing
JDK fails when `DOTTY_CLASSFILE_REQUIRE_JDK=1`.

`super_calls_keep_canonical_targets_and_exact_frames_in_ordinary_branches`
uses the same fixture with an additional ordinary branching client. It checks
selected method symbols against their direct-parent owners and verifies exact
Derived reference frames and branch target PCs before JDK execution.
The source/classfile negative tests retain typed rejection and empty source
output batches; the existing publisher still stages only fully encoded batches.
This increment extends hierarchy validation, transitive inherited method
lookup, direct-parent `super` selection, and two-edge reference assignability
through the bounded chain. Sibling joins without explicit checked context,
unrelated/`Object`-only reference joins, nested allocation arguments, arbitrary
effectful pre-super evaluation,
branched constructors, qualified/trait super, and Scala ABI/TASTy emission also
remain deferred.
