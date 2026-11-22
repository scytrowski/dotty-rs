# dotty-parser

A handwritten Scala 3.9.0 source parser with diagnostics and recovery.

The parser consumes the `dotty-core::TokenSource` contract and produces shared untyped trees. It does not depend on the concrete lexer. Grammar support is incremental.

This is an experimental, pre-1.0 component of
[dotty-rs](https://github.com/scytrowski/dotty-rs). APIs and supported Scala
subsets are still evolving; the project is not a complete Scala compiler.
See the [component documentation](https://github.com/scytrowski/dotty-rs/blob/main/docs/parser-design-3.9.0.md)
and [API reference](https://docs.rs/dotty-parser) for its contracts and limitations.

Published archives contain production sources and package documentation.
Regression tests, binary fixtures and Scala oracle tools are maintained in the
repository. Licensed under Apache-2.0; the complete license is included in the
published package.

See the [API usage guide](src/guide.md) for runnable examples and caller contracts.
