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
`777528f19a58e794c9954a42f433373472ec57f8`. The pinned closure fixture runner
compiles `LocalValues.scala` with Eclipse Temurin JDK 21.0.8+9 and compares its
typer output with `LocalValues.scala39-typed-tree.txt`. The source includes the
three issue acceptance members and a nested-block shadowing control. The
`Typer focused audit` workflow provisions these exact inputs and runs the same
script:

```sh
SCALA39_ROOT=/path/to/scala3-source \
JAVA_HOME=/path/to/temurin-21 \
SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=/path/to/coursier/repo1.maven.org/maven2 \
crates/dotty-typer/tests/fixtures/lexical-closure-contract/run
```

The Rust regression is
`cargo test -p dotty-typer --locked local_value_capture`. Scala's snapshot
establishes source acceptance and inferred lambda parameter types; the Rust
tests establish this implementation's typed trees and symbol provenance.
