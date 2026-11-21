# dotty-classfile

JVM class-file decoding and encoding, symbolic bytecode construction and bounded verification analysis.

The structural codec supports its documented class-file versions independently of the JVM backend's output target. It does not compile Scala source or provide general JVM verifier coverage. Java tools are not required to build the Rust library.

This is an experimental, pre-1.0 component of
[dotty-rs](https://github.com/scytrowski/dotty-rs). APIs and supported Scala
subsets are still evolving; the project is not a complete Scala compiler.
See the [component documentation](https://github.com/scytrowski/dotty-rs/blob/main/docs/classfile-encoder.md)
and [API reference](https://docs.rs/dotty-classfile) for its contracts and limitations.

Published archives contain production sources and package documentation.
Regression tests, binary fixtures and Scala oracle tools are maintained in the
repository. Licensed under Apache-2.0; the complete license is included in the
published package.
