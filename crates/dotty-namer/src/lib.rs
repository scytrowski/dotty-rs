//! Source-side naming infrastructure built on the shared [`dotty_core`] AST
//! and semantic model.
//!
//! This crate deliberately does not depend on the parser implementation. It
//! accepts phase-indexed trees from `dotty-core`, so parser clients can pass
//! their arena without exposing parser-private types to naming.

mod naming;

pub use dotty_core::{SourceContext, SourceContextId, SourceDefinition, SourceSemanticIndex};
pub use naming::{NamerError, name_compilation_unit};
