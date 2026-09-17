//! Shared semantic foundation for the Dotty frontend and backend.
//!
//! `dotty-core` represents symbols, types, and syntax/typed trees as one
//! model shared by the source parser, the classfile loader, the TASTy
//! unpickler, and the namer/typer, instead of each of them inventing its own.
//! It knows nothing about lexical syntax, JVM classfiles, TASTy's wire
//! format, or type inference — see `docs/dotty-core-design.md` for the full
//! design and the boundary with those crates.
//!
//! This crate is under active construction; only the identity, name, and
//! source-position layers exist so far.

pub mod ids;
pub mod names;
pub mod source;

pub use ids::{
    AnnotationId, ClassfileOriginId, NameId, ScopeId, SourceId, SymbolId, TastyOriginId, TreeId,
    TypeId,
};
pub use names::{Name, NameInterner, Namespace, TermName, TypeName};
pub use source::{SourceSpan, Span};
