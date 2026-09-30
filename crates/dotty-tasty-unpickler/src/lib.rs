//! Semantic unpickler from decoded TASTy into the shared `dotty-core` model.
//!
//! This crate interprets TASTy; it does not decode the wire format (that is
//! `dotty-tasty`), own a classpath (that is `dotty-classloader`), or define
//! the semantic model (that is `dotty-core`). It uses an enter-before-complete
//! pipeline because TASTy has forward references, shared nodes and recursive
//! binders. Entry records symbols and metadata in a `TastySemanticIndex`;
//! type decoding and `complete_symbol` project supported forms into the
//! canonical core model. Completion can remain pending when a dependency is
//! outside the entered state and no `SymbolResolver` can supply it. See
//! `docs/tasty-semantic-unpickler.md` for the compatibility gate and the
//! supported/deferred boundary.

mod annotated;
mod ast_view;
mod binders;
mod class;
mod companions;
mod completion;
mod constructor;
mod discovery;
mod enter;
mod error;
mod index;
mod lookup;
mod mapping;
mod method;
mod names;
mod opaque;
mod packages;
mod reachability;
mod recursive;
mod refined;
mod refinement;
mod session;
mod symbol_annotations;
mod term_type;
mod type_tree;
mod types;
mod unpickler;

/// Semantic TASTy unpickling APIs.
pub mod tasty_unpickler {
    pub use crate::error::UnpickleError;
    pub use crate::index::TastySemanticIndex;
    pub use crate::reachability::{IdentityNode, IdentityOutcome, identity_reachability};
    pub use crate::session::TastySession;
    pub use crate::unpickler::TastyUnpickler;
}
