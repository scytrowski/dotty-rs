//! The persistent semantic storage for a compilation session.

mod default_getters;
mod semantic_store;

pub use default_getters::DefaultGetterError;

pub use semantic_store::{SemanticStore, StoreCheckpoint};
