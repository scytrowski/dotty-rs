//! Name lookup, distinct from symbol ownership.
//!
//! Owner (on [`super::Symbol`]) answers "who semantically holds this
//! symbol"; [`Scope`] answers "where can this symbol be found by name
//! lookup." They usually agree but are not the same concept — e.g. an
//! imported name is in scope somewhere it isn't owned. `Symbol` never gets a
//! `children` field; child membership is reconstructed via scopes.

use std::collections::HashMap;

use crate::ids::{ScopeId, SymbolId, checked_index};
use crate::names::Name;

/// A name-lookup table for one owner (a class body, a block, ...).
///
/// Each name maps to a `Vec<SymbolId>` rather than a single symbol because
/// overloads (`def foo(x: Int)` / `def foo(x: String)`) are ordinary, not an
/// edge case.
#[derive(Debug, Default)]
pub struct Scope {
    pub owner: Option<SymbolId>,
    entries: HashMap<Name, Vec<SymbolId>>,
}

impl Scope {
    pub fn new(owner: Option<SymbolId>) -> Self {
        Self {
            owner,
            entries: HashMap::new(),
        }
    }

    /// Enters `symbol` under `name`, appending to any existing overloads.
    pub fn enter(&mut self, name: Name, symbol: SymbolId) {
        self.entries.entry(name).or_default().push(symbol);
    }

    /// Removes `symbol` from every name it was entered under.
    pub fn remove(&mut self, symbol: SymbolId) {
        for bucket in self.entries.values_mut() {
            bucket.retain(|&entered| entered != symbol);
        }
    }

    /// Returns the first symbol entered under `name`, if any.
    pub fn lookup(&self, name: &Name) -> Option<SymbolId> {
        self.entries
            .get(name)
            .and_then(|bucket| bucket.first())
            .copied()
    }

    /// Returns every symbol entered under `name`, in insertion order.
    pub fn lookup_all(&self, name: &Name) -> &[SymbolId] {
        self.entries.get(name).map_or(&[], Vec::as_slice)
    }
}

/// Owns every [`Scope`] for one compilation session.
#[derive(Debug, Default)]
pub struct ScopeArena {
    scopes: Vec<Scope>,
}

impl ScopeArena {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alloc(&mut self, scope: Scope) -> ScopeId {
        let id = ScopeId::new(checked_index(self.scopes.len()));
        self.scopes.push(scope);
        id
    }

    /// The number of scopes allocated so far.
    pub(crate) fn len(&self) -> usize {
        self.scopes.len()
    }

    /// Drops every scope allocated after the arena held `len` scopes.
    pub(crate) fn truncate(&mut self, len: usize) {
        self.scopes.truncate(len);
    }

    /// Panics if `id` was not allocated by this arena — see
    /// `docs/dotty-core-design.md`, "Error handling policy."
    pub fn get(&self, id: ScopeId) -> &Scope {
        &self.scopes[id.index() as usize]
    }

    pub fn get_mut(&mut self, id: ScopeId) -> &mut Scope {
        &mut self.scopes[id.index() as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::NameId;
    use crate::names::Namespace;

    fn name(raw: u32) -> Name {
        Name::new(NameId::new(raw), Namespace::Term)
    }

    #[test]
    fn lookup_finds_nothing_in_an_empty_scope() {
        let scope = Scope::new(None);

        assert_eq!(scope.lookup(&name(1)), None);
        assert_eq!(scope.lookup_all(&name(1)), &[]);
    }

    #[test]
    fn enter_and_lookup_round_trip_a_single_symbol() {
        let mut scope = Scope::new(None);
        let symbol = SymbolId::new(1);

        scope.enter(name(1), symbol);

        assert_eq!(scope.lookup(&name(1)), Some(symbol));
    }

    #[test]
    fn overloaded_names_keep_every_entry_in_insertion_order() {
        let mut scope = Scope::new(None);
        let first = SymbolId::new(1);
        let second = SymbolId::new(2);

        scope.enter(name(1), first);
        scope.enter(name(1), second);

        assert_eq!(scope.lookup_all(&name(1)), &[first, second]);
        assert_eq!(scope.lookup(&name(1)), Some(first));
    }

    #[test]
    fn remove_drops_one_overload_without_affecting_the_other() {
        let mut scope = Scope::new(None);
        let first = SymbolId::new(1);
        let second = SymbolId::new(2);
        scope.enter(name(1), first);
        scope.enter(name(1), second);

        scope.remove(first);

        assert_eq!(scope.lookup_all(&name(1)), &[second]);
        assert_eq!(scope.lookup(&name(1)), Some(second));
    }

    #[test]
    fn scope_arena_allocates_distinct_ids() {
        let mut arena = ScopeArena::new();
        let first = arena.alloc(Scope::new(None));
        let second = arena.alloc(Scope::new(None));

        assert_ne!(first, second);
    }

    #[test]
    fn scope_arena_get_mut_allows_entering_symbols_after_allocation() {
        let mut arena = ScopeArena::new();
        let id = arena.alloc(Scope::new(None));
        let symbol = SymbolId::new(1);

        arena.get_mut(id).enter(name(1), symbol);

        assert_eq!(arena.get(id).lookup(&name(1)), Some(symbol));
    }
}
