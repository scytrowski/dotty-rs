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
literal expressions, and plain monomorphic method applications. Application
sites can resolve lexical, imported, and selected overload buckets for supported
methods. Candidate filtering uses exact arity and nominal conformance; when
several candidates apply, only a unique method whose formal types are strictly
more specific than every other applicable candidate is selected. Selected
members use receiver-adapted signatures, including inherited generic methods.
Standalone overloaded identifiers and selections remain deferred. Generic
overloads and matching-arity candidates with unsupported semantics return typed
errors rather than being selected speculatively.

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

- expression typing and result-type inference;
- result-type inference beyond the current explicit method signatures;
- enum semantics, case-class synthetic APIs, and `derives`;
- context-bound evidence synthesis;
- default imports and general standard-library member lookup;
- annotations not represented by the current source typer;
- advanced parent feasibility and compiler-generated wrapper members.

Scala 3.9 TASTy records inferred/default module self references that source
`ClassInfo` completion does not yet synthesize. For module classes only, the
normalizer treats a missing self type and the matching compiler-generated
module self reference as the same default. Explicit non-default self types are
still compared structurally. The next Typer phase should build typed trees and
expression types after closing the remaining declaration-level parity gaps.
Do not treat this gate as evidence of expression-typing or full Scala source
compatibility.
