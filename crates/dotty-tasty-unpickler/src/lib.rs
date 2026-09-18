//! Semantic unpickler from decoded TASTy into the shared `dotty-core` model.
//!
//! This crate interprets TASTy; it does not decode the wire format (that is
//! `dotty-tasty`), own a classpath (that is `dotty-classloader`), or define
//! the semantic model (that is `dotty-core`). It is built as a multi-pass
//! pipeline — enter symbols, build types, complete signatures — rather than a
//! one-pass tree decoder, because TASTy has forward references, shared nodes
//! and recursive binders. See `docs/tasty-semantic-unpickler.md`.
//!
//! Only the crate scaffolding and error model exist so far; no pass is
//! implemented yet.

mod error;

/// Semantic TASTy unpickling APIs.
pub mod tasty_unpickler {
    pub use crate::error::UnpickleError;
}
