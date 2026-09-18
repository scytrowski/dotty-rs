//! Lazy symbol completion, modeled from day one so the namer/classloader can
//! defer computing a symbol's type without a completer engine existing yet.

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
