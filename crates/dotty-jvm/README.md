# dotty-jvm

Bounded JVM class-file emission from already-typed Scala semantics.

The backend validates borrowed canonical compilation plans before lowering supported classes and expressions. Current output uses class-file major version 65 (Java 21). It does not assemble a source frontend or provide a source compiler CLI.

This is an experimental, pre-1.0 component of
[dotty-rs](https://github.com/scytrowski/dotty-rs). APIs and supported Scala
subsets are still evolving; the project is not a complete Scala compiler.
See the [component documentation](https://github.com/scytrowski/dotty-rs/blob/main/docs/jvm-backend.md)
and [API reference](https://docs.rs/dotty-jvm) for its contracts and limitations.

Published archives contain production sources and package documentation.
Regression tests, binary fixtures and Scala oracle tools are maintained in the
repository. Licensed under Apache-2.0; the complete license is included in the
published package.

See the [API usage guide](src/guide.md) for runnable examples and caller contracts.
