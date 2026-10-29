# Typer closure capture of local values (Scala 3.9.0)

`Function1` source lambdas can capture a prior, initialized ordinary immutable
block-local `val`. The closure environment stores the canonical local symbol
through the existing `Closure.env` representation. Captures follow first-use
order and repeated uses share one entry. A lambda parameter that shadows a
local value resolves to the parameter and adds no local capture.

The source local retains its typed `ValDef`, initializer and source-to-typed
mapping. Capture entries reuse the typed identifier for the source use, so the
canonical symbol, owning method and completed local type remain available
without retyping the initializer. Expected function types flow through a
nonempty block to its final expression, allowing an unannotated `Function1`
parameter in a method with a local declaration.

Mutable and lazy values, contextual values, pattern binders, and nested lambda
captures remain unsupported. Failed typing is handled by the existing
expression transaction, which rolls back local and closure capture state.

The compatibility target is Scala 3.9.0 at source revision
`777528f19a58e794c9954a42f433373472ec57f8`. The focused Rust regression is
`cargo test -p dotty-typer --locked ordinary_function1_captures_prior_initialized_local_values`.
Scala compiler acceptance should be checked against that pinned checkout with
JDK 21; the compiler only establishes the Scala source shape, while the Rust
tests establish this implementation's typed trees and symbol provenance.
