//! Public facade for the Dotty compiler libraries.
//!
//! The [`tasty`] module exposes the structural Scala 3.9.0 TASTy codec. It
//! deliberately does not resolve Scala symbols or load JVM class files;
//! those concerns belong to higher-level compiler components.

/// Scala 3 TASTy encoding and structural APIs.
pub use dotty_tasty::tasty;

/// JVM class file decoding and encoding APIs.
pub use dotty_classfile::classfile;

/// Shared compiler foundation (source text, diagnostics, tokens, symbols,
/// types, and phase-indexed trees) for the source parser, classfile loader,
/// TASTy unpickler, and namer/typer.
pub use dotty_core::core;

/// Scala 3.9.0 source parser infrastructure and parser-facing AST APIs.
pub use dotty_parser as parser;

/// Source declaration signature completion.
pub use dotty_typer as typer;

/// Classpath loading APIs, unifying TASTy and class file entries.
pub use dotty_classloader::classloader;

/// Semantic unpickling of TASTy into the shared `dotty-core` model.
pub use dotty_tasty_unpickler::tasty_unpickler;
