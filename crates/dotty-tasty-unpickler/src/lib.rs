//! Semantic unpickler from decoded TASTy into the shared `dotty-core` model.
//!
//! This crate interprets TASTy; it does not decode the wire format (that is
//! `dotty-tasty`), own a classpath (that is `dotty-classloader`), or define
//! the semantic model (that is `dotty-core`). It is built as a multi-pass
//! pipeline — enter symbols, build types, complete signatures — rather than a
//! one-pass tree decoder, because TASTy has forward references, shared nodes
//! and recursive binders. See `docs/tasty-semantic-unpickler.md`.
//!
//! Two passes exist so far. Pass 1, `TastyUnpickler::enter_symbols`, enters
//! packages, classes, their members and parameters into the store, and records
//! each definition address in a `TastySemanticIndex`; entered symbols have no
//! type yet (`SymbolInfo::Missing`). Pass 2a, `TastyUnpickler::unpickle_type`,
//! gives each type node address at most one `TypeId` and resolves reference
//! types (`TypeRef`, `TermRef`, `ThisType`, `SHAREDtype`) through the index by
//! address, never by name; other type forms are an explicit
//! `UnpickleError::UnsupportedType`. Pass 5a projects type trees
//! (`unpickle_type_tree_type`) and completes simple symbols
//! (`complete_symbol`): `VALDEF`, `PARAM`, `TYPEPARAM` and plain `TYPEDEF`
//! become `SymbolInfo::Complete`, and (5c/5d1/5d2a) ordinary methods, classes,
//! traits, module classes and constructors do too (`Poly`/`Method`/`ClassInfo`).

mod annotated;
mod ast_view;
mod binders;
mod class;
mod completion;
mod constructor;
mod enter;
mod error;
mod index;
mod lookup;
mod mapping;
mod method;
mod names;
mod packages;
mod recursive;
mod refined;
mod term_type;
mod type_tree;
mod types;
mod unpickler;

/// Semantic TASTy unpickling APIs.
pub mod tasty_unpickler {
    pub use crate::error::UnpickleError;
    pub use crate::index::TastySemanticIndex;
    pub use crate::unpickler::TastyUnpickler;
}
