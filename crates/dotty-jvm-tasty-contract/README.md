# dotty-jvm-tasty-contract

Read-only consistency validation between JVM and Scala 3 TASTy producer plans.

This coordination layer checks canonical session identities, typed roots and the bounded erased ABI shared by the backend and semantic pickler. It does not encode or publish artifacts, and does not imply general Scala ABI compatibility.

This is an experimental, pre-1.0 component of
[dotty-rs](https://github.com/scytrowski/dotty-rs). APIs and supported Scala
subsets are still evolving; the project is not a complete Scala compiler.
See the [component documentation](https://github.com/scytrowski/dotty-rs/blob/main/docs/jvm-tasty-binary-interop.md)
and [API reference](https://docs.rs/dotty-jvm-tasty-contract) for its contracts and limitations.

Published archives contain production sources and package documentation.
Regression tests, binary fixtures and Scala oracle tools are maintained in the
repository. Licensed under Apache-2.0; the complete license is included in the
published package.

See the [API usage guide](src/guide.md) for runnable examples and caller contracts.
