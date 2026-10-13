# JVM backend boundary

`dotty-jvm` is the adapter boundary between a completed typed frontend and
future JVM lowering. It depends on `dotty-core` for canonical symbols, types,
and typed trees, and on `dotty-classfile` for class-file descriptor and output
APIs. The crate validates a borrowed `JvmCompilationUnit` contract and
projects one bounded method subset to JVM descriptors and lowers a first
subset of typed expression leaves to the class-file crate's symbolic code
builder. It does not assemble a source frontend or emit complete class files.

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

## Typed expression leaves

`lower_value_expression` accepts only typed `Literal` nodes containing
`Constant::Int` or `Constant::Boolean`, and `Ident` nodes whose typed
`TermRefTarget::Symbol` matches one of the method metadata's canonical
parameter symbols. Int and Boolean literals lower through `CodeBuilder::iconst`
and parameter references through `CodeBuilder::iload` at the metadata-assigned
instance local slot. The result category is `IntLike` for both, while Boolean
method descriptors remain `Z`. Literal constant types may be the matching
singleton constant type or the canonical `Definitions.int` / `.boolean` type.

The caller owns the `CodeBuilder` and is responsible for appending returns or
other method epilogues. A rejected tree or failed instruction append leaves the
builder unchanged. `Select`, `Apply`, `TypeApply`, `Block`, `If`, `While`,
`Match`, `New`, closures, nested `DefDef`s, String/Long and other non-Int/
Boolean literals, and all other typed nodes return a structured
unsupported-expression error. No source name lookup, implicit boxing, boolean
conversion, branch generation, or frontend dependency is performed.
