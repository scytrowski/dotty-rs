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
parameters, fields, selections, unsupported types, and locals not initialized
on every reachable path are rejected.
Missing bindings, missing initializers, unsupported types and modifiers,
initializer type mismatches, and detectable reads before the store fail with
typed errors. Loops and `PatDef` remain unsupported. Initialized
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
Planned method lowering supports initialized primitive `ValDef` statements
and assignments to initialized mutable primitive locals; standalone expression
lowering, loops, and other unsupported stats return structured errors.

This metadata layer does not encode Scala operator names, nested-class `$`
names, companion/module classes, trait forwarders, bridges, private-access
semantics, constructors, classfile hierarchy, or bytecode. Unsupported ABI
forms must be handled by a separate contract before they can be emitted.

## Whole-class emission

`emit_class` composes one planned class into a class file at
`JVM_CLASSFILE_MAJOR_VERSION` (65). The artifact contains a public ordinary
class extending canonical `java/lang/Object`, one synthesized public
`<init>()V`, and the public instance methods listed in `JvmClassPlan::methods`
in plan order. Method bodies are lowered through the same typed expression
path described above. The file is encoded into an owned `GeneratedClass` only
after every method and class-file structure has been built successfully.

The frontend must set `JvmClassInitializationPlan::ImplicitNoArg` only after
proving the source class has no constructor parameters or body, superclass
arguments, fields, or template initialization expressions. The backend checks
that the class's completed `ClassInfo` has exactly its canonical `Object`
parent, has no self type, declares no non-method members, and has at most one
no-argument primary constructor. A missing plan, a plan marked `Unsupported`,
or a class shape outside those checks returns a typed error; the backend does
not infer initialization safety from an empty method list. The synthesized
constructor calls `java/lang/Object.<init>()V` and returns.

The emitted class has no fields, interfaces, class initializer, Scala/TASTy
metadata, or unplanned methods. Constructors with parameters, additional
constructors, field initializers, nested types, explicit superclass behavior,
and other class initialization semantics remain unsupported. The API returns
only after encoding succeeds, so a failure does not expose a partial artifact.

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
trees or constant-pool indexes. A Java harness then checks each result under
`java -Xverify:all`.

CI sets `DOTTY_CLASSFILE_REQUIRE_JDK=1`, so missing JDK tools fail the gate.
Negative source tests verify that field initialization, local `val`, `while`,
private construction, and parser diagnostics fail at their actual pipeline
stage without leaving a `.class` file. Existing backend typed-tree tests cover
unsupported receivers, `using` calls, static/extension methods, mismatched
method references and arguments, and other unsupported call shapes. Run the
test locally with `cargo test --test jvm_source_compilation --locked`; without
the JDK it still runs structural and negative checks and explicitly skips Java
verification.

This path targets the repository's pinned Scala 3.9.0 source compatibility
reference at revision `777528f19a58e794c9954a42f433373472ec57f8`. It demonstrates
only the explicitly supported frontend and JVM subset; the class has no
`.tasty` payload and does not claim full Scala ABI or `scalac` compatibility.

## Typed expression leaves

`lower_value_expression` accepts typed `Literal` nodes containing `Constant::Int`
or `Constant::Boolean`, and `Ident` nodes whose typed `TermRefTarget::Symbol`
matches one of the method metadata's canonical parameter symbols. Literals
lower through `CodeBuilder::iconst`; parameter references use `CodeBuilder::iload`
at the metadata-assigned instance local slot. Both produce the `IntLike`
category while Boolean descriptors remain `Z`. Literal types may be the
matching singleton constant type or canonical `Definitions.int` / `.boolean`.

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

`Return.from = Some(...)` is rejected. Arbitrary `Select`, `TypeApply`, `Block`,
`While`, `Match`, `Try`, `New`, closures, nested `DefDef`s, String/Long and
other non-Int/Boolean literals, object/reference joins, and all other typed
nodes remain unsupported. No source name lookup, implicit boxing, boolean
conversion, constant folding, or frontend dependency is introduced.
