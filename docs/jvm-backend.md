# JVM backend boundary

`dotty-jvm` is the adapter boundary between a completed typed frontend and
future JVM lowering. It depends on `dotty-core` for canonical symbols, types,
and typed trees, and on `dotty-classfile` for class-file descriptor and output
APIs. The crate validates a borrowed `JvmCompilationUnit` contract and
projects one bounded method subset to JVM descriptors; it does not lower trees
or emit class files.

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
