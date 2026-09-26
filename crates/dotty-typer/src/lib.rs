//! Source declaration signature completion.
//!
//! The typer consumes a parsed and named untyped source tree. It does not
//! parse source, allocate declaration symbols, load classpath entries, or
//! construct a typed AST. Semantic type trees are cached per source unit.

mod source_type_index;
mod typer;

pub use source_type_index::SourceTypeIndex;
pub use typer::{SourceTyper, TyperError};
