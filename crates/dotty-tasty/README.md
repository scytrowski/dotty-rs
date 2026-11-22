# dotty-tasty

Borrowed and owned structural codecs for Scala 3.9.0 TASTy format 28.9.0.

The crate exposes binary primitives, names, raw and structured ASTs, sections and complete files. Semantic symbol interpretation and classpath loading belong to separate crates. Compatibility with other format versions must be requested explicitly.

This is an experimental, pre-1.0 component of
[dotty-rs](https://github.com/scytrowski/dotty-rs). APIs and supported Scala
subsets are still evolving; the project is not a complete Scala compiler.
See the [component documentation](https://github.com/scytrowski/dotty-rs/blob/main/docs/tasty-api.md)
and [API reference](https://docs.rs/dotty-tasty) for its contracts and limitations.

Published archives contain production sources and package documentation.
Regression tests, binary fixtures and Scala oracle tools are maintained in the
repository. Licensed under Apache-2.0; the complete license is included in the
published package.

See the [API usage guide](src/guide.md) for runnable examples and caller contracts.
