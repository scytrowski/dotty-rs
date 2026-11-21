# dotty-tasty-unpickler

Semantic projection of decoded Scala 3.9.0 TASTy into the shared dotty-core model.

The unpickler enters declarations before completing symbols and types. It borrows the canonical semantic session and reports unsupported or unresolved forms explicitly. Wire decoding and classpath discovery belong to separate crates.

This is an experimental, pre-1.0 component of
[dotty-rs](https://github.com/scytrowski/dotty-rs). APIs and supported Scala
subsets are still evolving; the project is not a complete Scala compiler.
See the [component documentation](https://github.com/scytrowski/dotty-rs/blob/main/docs/tasty-semantic-unpickler.md)
and [API reference](https://docs.rs/dotty-tasty-unpickler) for its contracts and limitations.

Published archives contain production sources and package documentation.
Regression tests, binary fixtures and Scala oracle tools are maintained in the
repository. Licensed under Apache-2.0; the complete license is included in the
published package.
