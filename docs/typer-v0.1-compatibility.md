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
- method signatures with explicit parameter and result types, polymorphic
  binders, multiple term clauses, empty clauses, and `using` clauses;
- extension-method signatures whose receiver and declared signature are
  supported by the source Typer;
- aliases and bounds represented by the current semantic type model;
- class header completion as `ClassInfo`, including its parent list, declared
  members, prefix, and explicit self type where supplied.
- primary constructor signature normalization for a plain constructor.

Expression typing currently includes typed identifiers, stable term selections,
literal expressions, plain method applications, and blocks with expressions and
explicitly typed local values.
Blocks type their statements in order, preserve the typed Block shape and
source position, and use the final expression's own type without widening it.
Each block allocates an empty scope in `SemanticStore` owned by the current
semantic expression owner; nested blocks push nested scopes. The scope and all
typed nodes, local symbol mappings, and source mappings are rolled back when
any part of the block fails. A local `val` or `var` with a source-written type
is checked before it enters the block scope; `var` symbols carry the mutable
flag. Inferred local value types, missing initializers, other local declarations
and imports in block statements return an explicit deferral.

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
  application, and block subset;
- method result-type inference;
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
