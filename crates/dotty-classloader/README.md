# dotty-classloader

Directory, JAR, JMOD and JDK metadata loading for the dotty-rs compiler libraries.

The loader combines class-file and TASTy metadata with the shared semantic model. It owns classpath discovery and binary loading, separately from source parsing and typing. JDK metadata operations require the relevant JDK files.

This is an experimental, pre-1.0 component of
[dotty-rs](https://github.com/scytrowski/dotty-rs). APIs and supported Scala
subsets are still evolving; the project is not a complete Scala compiler.
See the [component documentation](https://github.com/scytrowski/dotty-rs/blob/main/docs/classloader.md)
and [API reference](https://docs.rs/dotty-classloader) for its contracts and limitations.

Published archives contain production sources and package documentation.
Regression tests, binary fixtures and Scala oracle tools are maintained in the
repository. Licensed under Apache-2.0; the complete license is included in the
published package.

See the [API usage guide](src/guide.md) for runnable examples and caller contracts.
