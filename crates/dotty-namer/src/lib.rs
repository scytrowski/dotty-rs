//! Source-side naming infrastructure built on the shared [`dotty_core`] AST
//! and semantic model.
//!
//! This crate deliberately does not depend on the parser implementation. It
//! accepts phase-indexed trees from `dotty-core`, so parser clients can pass
//! their arena without exposing parser-private types to naming.

mod naming;
mod source_index;

pub use naming::{NamerError, name_compilation_unit};
pub use source_index::SourceSemanticIndex;
