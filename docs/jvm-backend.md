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

No general Scala type erasure, boxing, Unit-as-value, raw bytecode, source
pipeline orchestration, TASTy integration, or macro support is part of this
contract.

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
The CI JVM gate writes this generated class to disk, inspects it with `javap`,
compiles a small Java caller with `javac`, and executes it with
`java -Xverify:all` on JDK 21 or newer.

## Typed expression leaves

`lower_value_expression` accepts typed `Literal` nodes containing `Constant::Int`
or `Constant::Boolean`, and `Ident` nodes whose typed `TermRefTarget::Symbol`
matches one of the method metadata's canonical parameter symbols. Literals
lower through `CodeBuilder::iconst`; parameter references use `CodeBuilder::iload`
at the metadata-assigned instance local slot. Both produce the `IntLike`
category while Boolean descriptors remain `Z`. Literal types may be the
matching singleton constant type or canonical `Definitions.int` / `.boolean`.

`lower_expression` adds nested typed `If` expressions and local explicit
`Return` nodes. A Boolean condition is evaluated once and followed by symbolic
`ifeq`/`goto` labels. Value-producing branches agree on their `Int` or Boolean
type and reach a stack-compatible join. A branch that terminates with an
explicit local return contributes no placeholder value; a `Nothing` branch
without supported return flow is rejected. Every emitted tree must be within
the fixed depth budget `JVM_EXPRESSION_DEPTH_LIMIT` (64 nodes), and depth errors
carry the offending `TreeId` and optional source span. Each lowering call also
caps expanded node visits at `JVM_EXPRESSION_NODE_BUDGET` (4,096); repeated
references to a shared subtree count each time they are expanded.

`lower_method_body` checks the final flow against the method descriptor and
adds `ireturn` for an `Int`/Boolean fallthrough. A body whose reachable paths
all return gets no extra epilogue. Unit is accepted only when the body has
supported explicit `return` flow; it emits JVM `return`. All APIs build into a
fresh method-local `CodeBuilder`, so an error discards partial instructions.
The classfile builder then validates reachable blocks, analyzes stack states,
assembles labels, and writes `StackMapTable` frames.

`Return.from = Some(...)` is rejected. `Select`, `Apply`, `TypeApply`, `Block`,
`While`, `Match`, `Try`, `New`, closures, nested `DefDef`s, String/Long and
other non-Int/Boolean literals, object/reference joins, and all other typed
nodes remain unsupported. No source name lookup, implicit boxing, boolean
conversion, constant folding, or frontend dependency is introduced.
