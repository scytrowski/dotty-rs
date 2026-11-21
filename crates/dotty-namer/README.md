# dotty-namer

Source declaration naming and semantic indexing over the shared dotty-core AST.

The namer enters declarations into the caller's semantic session. It does not parse source or perform expression typing.

This is an experimental, pre-1.0 component of
[dotty-rs](https://github.com/scytrowski/dotty-rs). APIs and supported Scala
subsets are still evolving; the project is not a complete Scala compiler.
See the [component documentation](https://github.com/scytrowski/dotty-rs/blob/main/docs/namer-v0.1-compatibility.md)
and [API reference](https://docs.rs/dotty-namer) for its contracts and limitations.

Published archives contain production sources and package documentation.
Regression tests, binary fixtures and Scala oracle tools are maintained in the
repository. Licensed under Apache-2.0; the complete license is included in the
published package.
