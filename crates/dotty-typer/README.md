# dotty-typer

Incremental Scala 3.9.0 signature completion and expression typing.

The typer consumes parsed and named source trees in a canonical semantic session. It can use the target-independent symbol resolver contract for external symbols; it does not parse source or load binary formats. Unsupported and ambiguous semantics return typed errors.

This is an experimental, pre-1.0 component of
[dotty-rs](https://github.com/scytrowski/dotty-rs). APIs and supported Scala
subsets are still evolving; the project is not a complete Scala compiler.
See the [component documentation](https://github.com/scytrowski/dotty-rs/blob/main/docs/typer-v0.1-compatibility.md)
and [API reference](https://docs.rs/dotty-typer) for its contracts and limitations.

Published archives contain production sources and package documentation.
Regression tests, binary fixtures and Scala oracle tools are maintained in the
repository. Licensed under Apache-2.0; the complete license is included in the
published package.

See the [API usage guide](src/guide.md) for runnable examples and caller contracts.
