//! Semantic unpickler from decoded TASTy into the shared `dotty-core` model.
//!
//! This crate interprets TASTy; it does not decode the wire format (that is
//! `dotty-tasty`), own a classpath (that is `dotty-classloader`), or define
//! the semantic model (that is `dotty-core`). It is built as a multi-pass
//! pipeline — enter symbols, build types, complete signatures — rather than a
//! one-pass tree decoder, because TASTy has forward references, shared nodes
//! and recursive binders. See `docs/tasty-semantic-unpickler.md`.
//!
//! So far only pass 1 exists: `TastyUnpickler::enter_symbols` enters packages,
//! classes, their members and parameters into the store, and records each
//! definition address in a `TastySemanticIndex`. Entered symbols have no type
//! yet (`SymbolInfo::Missing`).

mod enter;
mod error;
mod index;
mod mapping;
mod names;
mod packages;
mod unpickler;

/// Semantic TASTy unpickling APIs.
pub mod tasty_unpickler {
    pub use crate::error::UnpickleError;
    pub use crate::index::TastySemanticIndex;
    pub use crate::unpickler::TastyUnpickler;
}
