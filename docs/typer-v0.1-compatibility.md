# Typer v0.1 source-to-TASTy semantic gate

This document records the supported boundary for the first source signature
Typer milestone. The parity tests compare structurally normalized semantic
snapshots from the source parser/namer/Typer and the TASTy unpickler. They do
not compare arena IDs, scope IDs, TASTy addresses, source positions, or
allocation order.

## Reproduction and oracle

Run the deterministic parity suite with:

```text
cargo test -p dotty-typer --test semantic_parity
```

The checked-in semantic `.tasty` fixtures are real compiler output from Scala
3.9.0. Their source files are adjacent in
[`crates/dotty-tasty-unpickler/tests/fixtures/semantic`](../crates/dotty-tasty-unpickler/tests/fixtures/semantic),
and `generate.sh` documents how to regenerate them using the Scala 3.9.0
compiler artifacts. The Scala compiler source checkout used by this repository
is pinned to revision
`777528f19a58e794c9954a2f433373472ec57f8` (branch `3.9.0`); the fixture
compiler version is pinned to the exact Maven artifact version `3.9.0`.

The suite currently compares plain and generic classes, traits, class
parameters/accessors, explicit superclass and applied generic parents, explicit
self types, nested classes, aliases, and explicit method signatures. Method
cases cover polymorphic binders and `ParamRef`, empty and multiple clauses,
contextual clauses, overloads, and an extension method. Module classes include
nested modules; the constructor fixture compares a completed primary-
constructor signature. An ID-independence test constructs the same class graph
with different arena and scope allocation, and negative tests prove that parent
and parameter-reference changes are detected.

A mismatch reports the normalized semantic path and the first differing
normalized component from each frontend. Builtin package-prefix normalization
is deliberately limited to named Scala and `java.lang` builtin targets.

## Supported source contract

For the tested subset, source Typer completion currently supports:

- declared simple, qualified, and applied type references;
- source declarations entered by the namer, including classes, traits,
  parameters, fields, methods, and type members;
- explicit class parents and explicit self types;
- class parameters with explicit types, including `val` accessors;
- method signatures with explicit parameter types and inferred result types,
  polymorphic binders, multiple term clauses, empty clauses, and `using`
  clauses. Inferred results come from the typed method body after expression
  widening, and the typed body is retained for later consumers. Recursive
  inferred results are rejected explicitly, including self and mutual cycles;
- extension-method signatures whose receiver and declared signature are
  supported by the source Typer;
- aliases and bounds represented by the current semantic type model;
- class header completion as `ClassInfo`, including its parent list, declared
  members, prefix, and explicit self type where supplied.
- primary constructor signature normalization for a plain constructor.

Expression typing currently includes typed identifiers, stable term selections,
literal expressions, plain method applications, and blocks with expressions
and local `val`/`var` definitions with or without explicit types. It also
supports expected-type conformance checks, source type ascriptions, direct
assignments to mutable locals and fields, ordinary `if` expressions over the
supported expression subset, condition-bearing `while` expressions, and local
`return` expressions in methods with an explicit result type.
Expected typing widens the expression type for the existing conformance
relation without changing the child tree's own type; a typed ascription node
carries the projected source type. Assignment requires an exact mutable symbol
reference, checks the right-hand side through expected typing, and produces
`Unit`. Selected field types are adapted to the receiver.

An `if` condition is checked against canonical `Boolean`. Both branches are
typed in the same lexical context and retain their own types in the typed AST.
The join widens branch value types, chooses an equivalent/supertype branch
when the supported nominal relation can prove it, and otherwise produces
`Type::Or`. Union subtyping implements only the two recursive rules required by
that join and is bounded by depth and comparison limits. Intersections and
other advanced relations remain explicit unsupported cases. Scala 3.9.0's
`TypeAssigner.assignType(If, ...)` combines the typed branch types with `|`
([pinned source](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/typer/TypeAssigner.scala#L428-L429));
the focused tests pin this subset's literal widening, nominal supertype, and
unrelated-class union behavior. An omitted source `else` uses the parser's
synthetic Unit tree and follows the same branch typing and join path.

A `while` condition is checked against canonical `Boolean`; its body is typed
as an ordinary expression in the same lexical context, so a block creates its
scope through the existing block path. The body's value is discarded without
a Unit conformance check, and the typed loop has canonical `Unit` type. This
result type matches Scala 3.9.0's `TypeAssigner.assignType(WhileDo)`
([pinned source](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/typer/TypeAssigner.scala#L476-L477)).
The parser currently represents ordinary loops with a condition; conditionless
loops and `Nothing` results are not part of this source typing contract.

A local `return` follows the semantic owner chain to its enclosing source
method and projects that method's explicit result type through normal method
completion. Its expression is checked against that type, and the typed Return
node has canonical `Nothing` type, matching Scala 3.9.0's
`TypeAssigner.assignType(Return)`
([pinned source](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/typer/TypeAssigner.scala#L473-L474)).
For a bare source `return`, the parser preserves an absent operand and Scala
3.9.0's `typedReturn` supplies a synthetic Unit literal before expected-type
checking
([pinned source](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/typer/Typer.scala#L2445-L2473)); the source typer mirrors that behavior. Returns in methods with inferred results and valid explicit `Return.from` targets remain deferred; malformed targets return a focused error. Constructors are outside this contract. Since `Nothing` conforms as the bottom type, a return in one `if` branch leaves the other branch's ordinary result type intact. If the existing bounded conformance relation cannot decide whether the return expression matches the declared result, typing reports a focused unsupported-relation error.

An inferred local
uses the already typed RHS and `widen_expression_type`; it does not repeat
identifier, member, or application lookup. The pinned Scala 3.9.0 source, TASTy,
and normalized typed-tree oracle are in
[`local-value-inference`](../crates/dotty-typer/tests/fixtures/local-value-inference).
They show `val n = 1` as `val n: Int = 1`, while the RHS remains a literal.
Inferred declarations publish the widened type and `var` retains its mutable
flag. The synthetic missing annotation's source `TypeTree` maps to the typed
`TypeTree` that carries the inferred type.
Blocks type their statements in order, preserve the typed Block shape and
source position, and use the final expression's own type without widening it.
Each block allocates an empty scope in `SemanticStore` owned by the current
semantic expression owner; nested blocks push nested scopes. The scope and all
typed nodes, local symbol mappings, and source mappings are rolled back when
any part of the block fails. A local `val` or `var` with a source-written type
shadows outer bindings throughout the statement sequence, including its own
initializer; a reference to the local while it is being initialized reports a
recursive initializer error. Explicitly typed initializers are checked against
their declared types. Inferred initializers are typed before the new local is
entered, so they resolve an outer binding of the same name or report ordinary
name lookup failure. Unsupported methodic, bounds, incomplete, and other
non-value inferred infos return a focused error. Both typed explicit and
inferred locals are entered with one stable symbol identity; a later failure in
the block rolls back local symbols, source mappings, and typed nodes.

Application sites can resolve lexical, imported, and selected overload buckets
for supported monomorphic methods. Candidate filtering uses exact arity and
nominal conformance; when several candidates apply, only a unique method whose
formal types are strictly more specific than every other applicable candidate
is selected. Selected members use receiver-adapted signatures, including
inherited generic methods. Standalone overloaded identifiers and selections
remain deferred. Generic overload competition remains deferred.

`SourceTyper::expression_context_for` builds a method or constructor body
context from the namer's declaration context and owned scope. Term lookup checks
typer-local scopes from innermost to outermost before source scopes and their
import rules, preserving each matching overload bucket. The method scope is
reused as indexed, so parameters are not entered a second time. Constructor
contexts expose constructor-owned parameters for lookup, but constructor-body
typing is not otherwise implemented. `push_local_scope` adds empty block scopes
without changing the namer's source-context graph.

Explicit positional type applications are supported for one resolved `Poly`
callee. Type arguments use the expression's lexical context, require exact
arity, and are checked against ordinary lower and upper bounds before the Poly
binder is instantiated. Typed `TypeApply` nodes retain the exact function
reference and carry projected type arguments as typed `TypeTree` nodes.
Overloaded type applications, unsupported bounds, and unsupported source type
argument forms return typed deferrals/errors.

Plain applications of one resolved `Poly` whose result is one immediate plain
`Method` infer type arguments from the method's term arguments. Inference is
binder/index based and supports direct `ParamRef` formals and matching nested
`Applied` type constructors. Repeated equivalent constraints are accepted;
conflicts do not compute a least upper bound. Concrete formal fragments still
require conformance, and inferred arguments are checked against instantiated
ordinary bounds. Unconstrained parameters, unsupported shapes, and generic
overload competition return typed errors. Expected-result inference, variance
solving, inherited-constructor matching, unions/intersections, wildcard capture,
match-type reduction, type-lambda unification, implicit search, and numeric
weak conformance remain deferred.

Source and TASTy snapshots intentionally normalize adapter-specific identity
and provenance. TASTy's package prefixes on the built-in Scala and
`java.lang.Object` references normalize to the source Typer's canonical
no-prefix form. The TASTy class scope also contains a synthetic `writeReplace`
method for serializable compiler output; that exact synthetic method is omitted
from class member equality. These are named, narrow normalizations; other
members and type components remain part of equality.

## Deferred work

The parity gate does not claim support for arbitrary Scala 3.9 source. These
parts remain deferred until their source semantic implementation and fixtures
are ready:

- expression forms outside the supported identifier, selection, literal,
  application, block, ordinary `if`, condition-bearing `while`, and local
  `return` subset, including `match`, `try`, lambdas, and other forms that are
  not currently handled by the expression typer;
- local `def`, class, type, and pattern declarations, plus imports in block
  statements;
- generic overload inference, `using`/implicit argument insertion, dependent
  result application, and right-associative extension normalization;
- enum semantics, case-class synthetic APIs, and `derives`;
- context-bound evidence synthesis;
- default imports and general standard-library member lookup;
- annotations not represented by the current source typer;
- advanced parent feasibility and compiler-generated wrapper members.

Scala 3.9 TASTy records inferred/default module self references that source
`ClassInfo` completion does not yet synthesize. For module classes only, the
normalizer treats a missing self type and the matching compiler-generated
module self reference as the same default. Explicit non-default self types are
still compared structurally. Do not treat this gate as evidence of general
expression typing or full Scala source compatibility.
