//! Source declaration signature completion.
//!
//! The typer consumes a parsed and named untyped source tree. It does not
//! parse source, allocate declaration symbols, load classpath entries, or
//! construct a typed AST. Semantic type trees are cached per source unit.
//!
//! Qualified type references use `ThisType` for an enclosing class prefix and
//! the canonical package `TypeRef` (with `no_prefix`) for package prefixes.
//! Call [`SourceTyper::with_resolver`] to supply semantic symbols outside the
//! entered source/session state; the default resolver is `NoResolver`.
//!
//! Type-name lookup checks lexical scopes from the innermost context outward.
//! Current-unit package definitions precede explicit imports, which precede
//! wildcard imports and package definitions from other source units. Imports
//! at the same lexical depth have equal precedence: distinct matching symbols
//! are ambiguous, while repeated bindings of the same symbol are accepted.
//! Import contexts are position-aware; a declaration sees only imports
//! preceding it in its source context chain.

mod source_type_index;
mod typer;

pub use source_type_index::SourceTypeIndex;
pub use typer::{SourceTyper, TyperError};
