# JVM backend boundary

`dotty-jvm` is the adapter boundary between a completed typed frontend and
bounded JVM lowering. It depends on `dotty-core` for canonical symbols, types,
and typed trees, and on `dotty-classfile` for class-file descriptor and output
APIs. The crate validates a borrowed `JvmCompilationUnit` contract, projects a
bounded class and method subset to JVM metadata, lowers selected typed
expressions to the class-file crate's symbolic code builder, and composes a
complete class file when the frontend supplies sufficient initialization
evidence. It does not assemble a source frontend.

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
slice that differs from the method plan is rejected. Locals are projected only
from completed `Int` and `Boolean` semantic types. The instance receiver stays
in slot zero, parameter slots retain their descriptor-derived positions, and
locals receive one category-one slot each in typed declaration ID order. The
allocator is monotonic and does not reuse slots for shadowed locals. Its
`max_locals` count includes the receiver and parameters and is checked against
the u16 class-file field before each slot is published. The layout carries the
canonical `SymbolId`, typed declaration ID, primitive type, and mutability;
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
typed errors. `PatDef` remains unsupported. Initialized
local reads lower through the same canonical `SymbolId` lookup to one `iload`
from the allocated slot. Local loads require the current method's validated
layout, a completed supported semantic type, and a preceding store on every
reachable path. Name-only references, foreign or unplanned locals, and reads before
initialization return typed errors. The existing bytecode analyzer remains
responsible for verifier local state and stack-map frames.

The initial plan subset accepts `SymbolKind::Class`, `SymbolKind::Method`, and
`SymbolKind::Parameter`; other kinds return a structured unsupported-kind
error. Validation rejects duplicate class or method plans, missing semantic
info, stale IDs, and typed nodes with missing, unfilled, or sentinel types. It
checks every allocated typed AST node in arena order, then
returns the first error according to the documented plan order. It does not
walk or validate references nested inside semantic types, since that is the
responsibility of the canonical semantic model's producers. All operations
are read-only; the lifetime-bound references keep the caller as the owner.

## Primitive descriptor subset

`JvmMethodPlan::descriptor` reads the method's completed `Type::Method` and the
ordered parameter symbols from that plan. It supports one ordinary
monomorphic parameter clause, `Definitions.int` (`I`), `Definitions.boolean`
(`Z`), and result-only `Definitions.unit` (`V`). Integer and Boolean singleton
constants widen to their corresponding primitive descriptor without changing
the source type arena. Int and Boolean both use the JVM int-like category-one
operand representation, while their descriptors remain distinct.

The descriptor path checks class/method/parameter kinds and ownership,
completed semantic info, method/plan parameter counts, and agreement between
each parameter symbol type and its method signature type. It counts the
instance receiver toward the JVMS 255-slot limit; static methods do not add a
receiver slot. The result is parsed by `dotty-classfile` before it is returned.

Polymorphic or curried methods, non-plain parameter clauses, aliases, type
parameters, match/refined/union/intersection types, Scala objects, reference
classes, arrays, by-name and repeated types, non-primitive constants, Unit
parameters, erased parameters, and varargs are rejected with structured
errors. Unknown types never fall back to `java/lang/Object`.

No general Scala type erasure, reference boxing, Unit operand-stack
representation, raw bytecode, source pipeline orchestration, TASTy integration,
or macro support is part of this contract.

Object-construction typing, canonical constructor and receiver identity,
uninitialized-reference verifier constraints, and the multi-class planning
boundary are recorded separately in
[`jvm-object-creation-contract.md`](jvm-object-creation-contract.md). That
contract is an audit only: the current backend does not emit object creation
or reference locals, and direct method selection from `new C(...)` is
currently rejected by the Typer as an unstable selection prefix.

## Canonical `Int` arithmetic

Method bodies resolve typed regular applications against the canonical
`scala.Int` operator `SymbolId`s before applying the ordinary same-class-call
restrictions. `+`, `-`, `*`, `/`, and `%` lower their selected receiver first,
then their sole argument, exactly once each, and emit `iadd`, `isub`, `imul`,
`idiv`, or `irem`. Both operands and the application result must have the
canonical `Int` semantic type and produce JVM `Int` values. Nested arithmetic,
local initializers and assignments, and arithmetic in the existing integer
`If`/`While` control-flow subset use the same recursive lowering path.

These intrinsic calls do not emit `invokevirtual` or a method reference for the
Scala operator. Calls whose symbol is not in the canonical registry continue
through ordinary same-class call resolution and retain its receiver and
control-flow restrictions. Standalone expression lowering can emit canonical
Int and Boolean intrinsics without a compilation-unit call context; ordinary
applications still require a validated same-class target. Canonical relational
operators `<`, `<=`, `>`, and `>=`
emit their signed `if_icmp*` branch to a true label, materialize `0` on the
false path and `1` on the true path, then join. The classfile builder computes
branch offsets and `StackMapTable` frames. The Boolean values work as results,
local values, and conditions in `if` and `while`. This compositional subset
includes initialized integer locals updated by arithmetic assignments across
multiple loop iterations, comparison conditions, nested Boolean branches, and
`if` branches that return arithmetic values. Loop locals must be initialized
before the loop; a local initialized only in the body is not assumed to exist
on the zero-iteration exit. Calls with reference receivers remain unsupported
in control-flow graphs because the current analyzer still models only
integer-like stack states there. Universal equality operators `Any.==` and
`Any.!=` remain unsupported. JVM overflow, signed division/remainder, and division-by-zero
behavior are preserved by emitting the corresponding JVM instruction without
constant folding or prechecks.

`tests/fixtures/jvm/Calculator.scala` validates this subset through the real
lexer, parser, namer, typer, JVM backend, and JDK 21+ runtime. Its Java harness
checks arithmetic overflow, signed comparison boundaries, divide-by-zero
exceptions, and repeated `sumTo` loop execution with `java -Xverify:all`.
General Scala library lowering, Long/Float/Double, Java classpath loading,
TASTy emission, the full Scala ABI, universal equality, and arbitrary
reference-bearing calls inside branched CFGs remain outside this subset.

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

Method-body lowering accepts assignments to planned mutable primitive fields
that resolve canonically to the current class's unqualified field identifier or
`this.field`. It validates field ownership, plan membership, mutability and
primitive type, then emits `aload_0`, the RHS once, and `putfield`; a successful
assignment completes with Unit and no value on the operand stack. Local
assignments continue to use their canonical local slot and `istore`, including
when a local shadows a field. Immutable fields, arbitrary receivers, and field
writes in methods whose emitted code contains branches return typed errors.
`emit_class` carries these field references into the classfile after validating
the field and constructor plans.

The backend treats each field/RHS pair as frontend proof. A `TreeId<Typed>`
does not carry a session tag, so the producer must obtain the ID from the same
typed arena it passes in the compilation unit. The backend checks arena
membership and type conformance, but it cannot independently reconstruct the
source-to-typed mapping without violating the frontend dependency boundary.

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
target passes `resolve_same_class_call`. It emits one current receiver, then
lowers each argument once in source order and checks its primitive stack value
against the selected descriptor. It finally emits symbolic `invokevirtual`
with the target's exact owner, name, and descriptor. Int/Boolean results leave
one int-like value; Unit leaves no value. Arity, argument-flow, target, and
typed-result mismatches return structured errors, and a failed method does not
produce a class artifact. Standalone expression/body lowering has no emission
plan, so it cannot resolve `Apply` targets.

Planned method-body lowering allows initialized primitive local references in
the final expression or other value positions. Standalone expression APIs have
no local layout and return `MissingLocalBinding` for local references. Typed
`Block` lowering evaluates supported stats from left to right, then
lowers the final expression once. An unused Int/Boolean value is discarded
with one `pop`; Unit already leaves an empty operand stack and needs no pop.
The final value remains available to its parent expression or method return.
If a stat definitely returns, lowering stops there and omits the unreachable
suffix, so dead instructions do not reach CFG analysis. A normally completing
Unit method receives one trailing `return`; explicit returns remain terminal.
The block's declared type is checked against its final expression flow.
Planned method lowering supports initialized primitive `ValDef` statements,
assignments to initialized mutable primitive locals, and typed `While` loops
with Boolean conditions. A loop evaluates its condition at the symbolic header
on every iteration, branches to its exit when false, and emits a back-edge only
when the body can fall through. Unit bodies leave the stack empty; Int/Boolean
body values are discarded once. The condition and body share the existing
depth/node budgets and control-flow call restriction. Since a loop may execute
zero times, initialization performed only in its body is not considered
definite after the loop; the class-file CFG analyzer verifies header/back-edge
local states and generates stack-map frames. Nested loops and integer-only
`If`/`Block`/`Assign` bodies are supported when their verifier states merge.
Same-class calls in loop control flow, reference-bearing states, `PatDef`, and
general comparison/arithmetic remain unsupported. Standalone expression
lowering and other unsupported stats return structured errors.

`JvmCompilationUnit::lower_method_body(class, method, definitions)` also lowers
reads of fields in that class's validated field plan. A canonical unqualified
field `Ident` or a `Select` whose qualifier is that class's unqualified
`ThisType` emits `aload(0)` followed by symbolic `getfield` using the validated
owner, field name, and `I`/`Z` descriptor. Both forms produce the matching
int-like `Int` or Boolean flow. Resolution uses the `TermRefTarget::Symbol`
identity, the field owner, and its planned metadata; same-spelling parameters
and locals keep their existing loads. Other receivers, fields absent from the
plan, stale targets, and prefixes that do not identify the current instance
return contextual errors. Field reads within `If`, `While`, or other branching
CFG regions are rejected explicitly until the stack-map analyzer can track the
temporary reference operand. Straight-line `Block` results may carry the type
of a planned field into this same validated read path. Whole-class emission
uses this lowering path for planned methods and emits no artifact if any
method fails.

This metadata layer does not encode Scala operator names, nested-class `$`
names, companion/module classes, trait forwarders, bridges, private-access
semantics, constructor bodies, classfile hierarchy, or bytecode. Unsupported ABI
forms must be handled by a separate contract before they can be emitted.

## Whole-class emission

`emit_class` composes one planned class into a class file at
`JVM_CLASSFILE_MAJOR_VERSION` (65). The artifact contains an ordinary class extending canonical
`java/lang/Object`, validated fields in frontend source order, one constructor,
and the planned instance methods in plan order. `ImplicitNoArg` emits the
existing public `<init>()V` and accepts no fields. `PrimaryConstructor` emits
the exact validated descriptor and public/private access from canonical
constructor metadata; its code calls `Object.<init>()V`, then appends each
source-ordered primitive field initializer, then returns. No parameterized
class gets an extra `()V`. Method bodies use the same typed lowering path as
before. The file is encoded into an owned `GeneratedClass` only after every
method and class-file structure has been built successfully.

The frontend must set `JvmClassInitializationPlan::ImplicitNoArg` only after
proving the source class has no constructor parameters or body, superclass
arguments, fields, or template initialization expressions. The backend checks
that the class's completed `ClassInfo` has exactly its canonical `Object`
parent and has no self type. It checks that every field and method declaration
is represented by the supplied plan. A missing plan, a plan marked
`Unsupported`, an unplanned member, or a class shape outside the supported
subset returns a typed error; the backend does not infer initialization
safety from an empty method list.

The emitted class has no interfaces, class initializer, Scala/TASTy metadata,
or unplanned methods. Additional constructors, nested types, explicit
superclass behavior, and other class initialization semantics remain
unsupported. The API returns only after encoding succeeds, so a failure does
not expose a partial artifact.

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

`Counter.scala` runs through the same real frontend assembly with a canonical
constructor parameter, private mutable `current:I`, and public `get`, `set`,
and `increment` methods. `PrivateFields.scala` covers private final `I` and
`Z` fields, including true and false Boolean getters. Both stateful fixtures
are compiled in two fresh frontend sessions and their complete class bytes
must match. Their decoded constructor descriptors, field flags and references,
and method instructions are checked structurally; the JDK gate uses
`javap -c -v` and Java harnesses under `-Xverify:all`. Negative real-source
cases cover wrong initializer types, immutable assignment, unsupported
constructor forms, and field reads inside branches; failed compilation must
leave no class artifact. See
[`jvm-stateful-class-contract.md`](jvm-stateful-class-contract.md).

These fixtures test the bounded emitted class shape. They do not establish
Scala public accessor, bridge, or TASTy compatibility.

CI sets `DOTTY_CLASSFILE_REQUIRE_JDK=1`, so missing JDK tools fail the gate.
Negative source tests verify that field initialization, invalid while
conditions, same-class calls inside loop control flow, private
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
skips Java verification. Primitive arithmetic, relational operators, and
general reference-bearing control-flow states remain deferred.

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

Calls inside an `If` expression are rejected with a typed error because the
current classfile stack-map analyzer cannot represent their temporary
reference receiver in that control-flow graph. Calls at ordinary expression
roots, as arguments to other calls, and under supported local returns remain
within the current subset when their stack flow is primitive or Unit.

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
one slot each starting at 1, with slot 0 reserved for `this`. At most 254
parameters are supported. Exact constructor-owned parameter identities are
preserved; class-owned parameter copies and `val`/`var` accessor fields never
substitute for them.

Projection checks the live constructor kind, source origin, owner, declaration
scope, complete plain monomorphic signature, primitive parameter types, and
proven public/private visibility. Private constructors stay private. Generic,
secondary, contextual, defaulted, by-name, erased and repeated parameters,
parameter accessors, annotations, qualified visibility, and non-primitive
parameters fail with structured errors. Source proof rejection does not
publish partial metadata or mutate semantic state. Emission creates only the
proved constructor, with its exact descriptor and access flags; it does not add
a fallback constructor, Scala accessors, field stores outside the approved
initializer path, or `new` expressions.

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
field fragment starts with `aload_0`, lowers one primitive value, and ends with
`putfield` for that canonical field. Supported RHS forms are Int/Boolean
literals, constructor-owned Int/Boolean parameters, canonical `Int.+`,
canonical Boolean negation, empty-statement blocks, and reads of fields whose
initializer fragment precedes the current one. Self and forward field reads,
user method calls, `this` escape, control-flow expressions, and other
unsupported typed nodes fail with contextual `JvmCodegenError`s.

`JvmConstructorFieldInitializerPlan::append_to` appends only those field
instructions. `emit_class` emits `aload_0; invokespecial
java/lang/Object.<init>()V` before the plan and appends `return` afterward. It
also installs the validated private field_info entries and uses the exact
constructor descriptor and access flags from metadata.
