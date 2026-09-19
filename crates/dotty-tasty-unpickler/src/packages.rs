//! Scope bookkeeping for pass 1.
//!
//! Package symbols are not entered here: they are session identities, kept
//! by `dotty_core::Packages` and shared with every other adapter.

use dotty_core::ids::{ScopeId, SymbolId};
use dotty_core::names::Name;
use dotty_core::store::SemanticStore;

/// Every declaration made into a scope, in order, so that a failed unit can
/// take its declarations back out of scopes that existed before it.
pub(crate) type ScopeJournal = Vec<(ScopeId, SymbolId)>;

/// Declares `symbol` under `name` in `scope`, journaling the entry.
pub(crate) fn enter_in_scope(
    store: &mut SemanticStore,
    journal: &mut ScopeJournal,
    scope: ScopeId,
    name: Name,
    symbol: SymbolId,
) {
    store.scopes.get_mut(scope).enter(name, symbol);
    journal.push((scope, symbol));
}
