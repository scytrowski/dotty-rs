//! Lazy symbol completion, modeled from day one so the namer/classloader can
//! defer computing a symbol's type without a completer engine existing yet.
//!
//! `CompletionId`'s constructor stays `pub(crate)`, unlike
//! `ClassfileOriginId`/`TastyOriginId` (see [`super::OriginTable`]): no
//! completer engine exists yet to resolve a `Deferred` back to a
//! `Complete`, so handing out real `CompletionId`s today would let external
//! code create completions that can never complete. `dotty-classloader`'s
//! first migration onto this model uses only `Missing -> Complete`
//! (`Symbol` enters with `SymbolInfo::Missing`, then the loader eagerly
//! resolves it to `SymbolInfo::Complete(TypeId)` once its members/parents
//! are known — see `docs/classloader.md`'s enter-before-complete section).
//! `Deferred` is not removed and this module's shape does not change when a
//! real completer engine lands; only that engine gets to mint
//! `CompletionId`s, the same controlled-front-door pattern `OriginTable`
//! already uses.

use crate::ids::{CompletionId, TypeId};

/// A symbol's type/info, possibly not computed yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolInfo {
    Missing,
    Deferred(CompletionId),
    Complete(TypeId),
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_carries_a_completion_id() {
        let info = SymbolInfo::Deferred(CompletionId::new(1));

        assert_eq!(info, SymbolInfo::Deferred(CompletionId::new(1)));
        assert_ne!(info, SymbolInfo::Deferred(CompletionId::new(2)));
    }

    #[test]
    fn complete_carries_a_type_id() {
        let info = SymbolInfo::Complete(TypeId::new(3));

        assert_eq!(info, SymbolInfo::Complete(TypeId::new(3)));
    }

    #[test]
    fn missing_and_error_are_distinct_unit_variants() {
        assert_ne!(SymbolInfo::Missing, SymbolInfo::Error);
    }
}
