//! JVM class file decoder and encoder.
//!
//! Target format: the class file format produced and consumed by JDK 25
//! (class file major version 69), the current LTS release supported by
//! Scala 3.9.0. Implementation is not yet started; this crate is scaffolding
//! for the upcoming classfile loader used by TASTy semantic analysis.

/// JVM class file binary and structural APIs.
pub mod classfile {}
