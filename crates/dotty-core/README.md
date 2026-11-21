# dotty-core

Shared names, typed IDs, symbols, scopes, types, diagnostics and phase-indexed ASTs for the dotty-rs compiler libraries.

This crate is target-independent and does not parse source or decode binaries. IDs belong to their allocating session; callers must retain the canonical stores and respect rollback invalidation.

This is an experimental, pre-1.0 component of
[dotty-rs](https://github.com/scytrowski/dotty-rs). APIs and supported Scala
subsets are still evolving; the project is not a complete Scala compiler.
See the [component documentation](https://github.com/scytrowski/dotty-rs/blob/main/docs/dotty-core-design.md)
and [API reference](https://docs.rs/dotty-core) for its contracts and limitations.

Published archives contain production sources and package documentation.
Regression tests, binary fixtures and Scala oracle tools are maintained in the
repository. Licensed under Apache-2.0; the complete license is included in the
published package.
