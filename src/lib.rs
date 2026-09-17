//! Public facade for the Dotty compiler libraries.

/// Scala 3 TASTy encoding and structural APIs.
pub use dotty_tasty::tasty;

/// JVM class file decoding and encoding APIs.
pub use dotty_classfile::classfile;

/// Classpath loading APIs, unifying TASTy and class file entries.
pub use dotty_classloader::classloader;
