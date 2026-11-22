# dotty-lexer

Scala 3.9.0 source tokenization and contextual scanning.

The lexer produces the shared token contracts from `dotty-core`. It does not parse or type source programs.

This is an experimental, pre-1.0 component of
[dotty-rs](https://github.com/scytrowski/dotty-rs). APIs and supported Scala
subsets are still evolving; the project is not a complete Scala compiler.
See the [component documentation](https://github.com/scytrowski/dotty-rs/blob/main/docs/lexer-design-3.9.0.md)
and [API reference](https://docs.rs/dotty-lexer) for its contracts and limitations.

Published archives contain production sources and package documentation.
Regression tests, binary fixtures and Scala oracle tools are maintained in the
repository. Licensed under Apache-2.0; the complete license is included in the
published package.

See the [API usage guide](src/guide.md) for runnable examples and caller contracts.
