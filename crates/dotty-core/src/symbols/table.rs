//! Allocates and looks up [`Symbol`] values.

use crate::ids::{SymbolId, checked_index};
use crate::symbols::completion::SymbolInfo;
use crate::symbols::symbol::Symbol;

/// Owns every [`Symbol`] for one compilation session.
#[derive(Debug, Default)]
pub struct SymbolTable {
    symbols: Vec<Symbol>,
    // Sorted by identity. Naming registers constructors in allocation order.
    plain_primary_clauses: Vec<SymbolId>,
}

impl SymbolTable {
    /// Creates an empty canonical symbol table for one compilation session.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a declaration and returns its stable table-local identity. Ownership, name, and
    /// signature relationships must be valid in the same session.
    ///
    /// # Panics
    ///
    /// Panics if the next identity exceeds the 32-bit ID space.
    pub fn alloc(&mut self, symbol: Symbol) -> SymbolId {
        let id = SymbolId::new(checked_index(self.symbols.len()));
        self.symbols.push(symbol);
        id
    }

    /// The number of symbols allocated so far.
    pub(crate) fn len(&self) -> usize {
        self.symbols.len()
    }

    /// Drops every symbol allocated after the table held `len` symbols.
    pub(crate) fn truncate(&mut self, len: usize) {
        self.symbols.truncate(len);
        self.plain_primary_clauses.truncate(
            self.plain_primary_clauses
                .partition_point(|id| (id.index() as usize) < len),
        );
    }

    /// Records a source primary constructor with no type parameters and one
    /// clause of required, strict, plain parameters. The source frontend must
    /// establish this shape in the original declaration before registration.
    /// Repeated registration is idempotent. Truncating a constructor also drops its record.
    ///
    /// # Panics
    ///
    /// Panics unless the ID identifies a source constructor in this table.
    pub fn record_plain_primary_clause(&mut self, constructor: SymbolId) {
        let symbol = self.get(constructor);
        assert!(
            symbol.kind == crate::SymbolKind::Constructor
                && matches!(symbol.origin, crate::SymbolOrigin::Source(_))
        );
        if let Err(slot) = self
            .plain_primary_clauses
            .binary_search_by_key(&constructor.index(), |id| id.index())
        {
            self.plain_primary_clauses.insert(slot, constructor);
        }
    }

    /// Whether naming established a plain primary clause for this constructor
    /// in this session. Binary and unregistered symbols return `false`.
    pub fn has_plain_primary_clause(&self, constructor: SymbolId) -> bool {
        self.plain_primary_clauses
            .binary_search_by_key(&constructor.index(), |id| id.index())
            .is_ok()
    }

    /// Returns the symbol for an ID from this session.
    ///
    /// # Panics
    ///
    /// Panics for an out-of-range index. IDs from another arena with the same
    /// index cannot be detected and violate the caller’s session contract.
    pub fn get(&self, id: SymbolId) -> &Symbol {
        &self.symbols[id.index() as usize]
    }

    /// Returns whether `id` names a symbol allocated by this table.
    pub fn contains(&self, id: SymbolId) -> bool {
        (id.index() as usize) < self.symbols.len()
    }

    /// Borrows a symbol for in-place mutation; callers must preserve ownership and completion
    /// invariants.
    ///
    /// # Panics
    ///
    /// Panics for an out-of-range index. A same-slot ID from another table cannot be detected.
    pub fn get_mut(&mut self, id: SymbolId) -> &mut Symbol {
        &mut self.symbols[id.index() as usize]
    }

    /// Returns the declaration's semantic owner, independently of lookup-scope membership.
    ///
    /// # Panics
    ///
    /// Panics if the symbol index is outside this table.
    pub fn owner(&self, id: SymbolId) -> Option<SymbolId> {
        self.get(id).owner
    }

    /// Borrows the declaration's current signature-completion state.
    ///
    /// # Panics
    ///
    /// Panics if the symbol index is outside this table.
    pub fn info(&self, id: SymbolId) -> &SymbolInfo {
        &self.get(id).info
    }

    /// Replaces the signature-completion state without validating the supplied type or completer.
    /// All referenced identities must belong to this session.
    ///
    /// # Panics
    ///
    /// Panics if the symbol index is outside this table.
    pub fn set_info(&mut self, id: SymbolId, info: SymbolInfo) {
        self.get_mut(id).info = info;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{NameId, TypeId};
    use crate::names::Name;
    use crate::names::Namespace;
    use crate::symbols::flags::SymbolFlags;
    use crate::symbols::kind::SymbolKind;
    use crate::symbols::origin::SymbolOrigin;
    use crate::symbols::symbol::SymbolLinks;
    use crate::symbols::visibility::Visibility;

    fn minimal_symbol(owner: Option<SymbolId>) -> Symbol {
        Symbol {
            name: Name::new(NameId::new(1), Namespace::Term),
            owner,
            kind: SymbolKind::Value,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        }
    }

    #[test]
    fn truncated_constructor_identity_loses_its_clause() {
        let mut table = SymbolTable::new();
        let mut entry = minimal_symbol(None);
        entry.kind = SymbolKind::Constructor;
        entry.origin = SymbolOrigin::Source(crate::SourceId::from_index(0));
        let first = table.alloc(entry.clone());
        table.record_plain_primary_clause(first);
        let last = table.alloc(entry.clone());
        table.record_plain_primary_clause(last);
        table.truncate(1);
        assert_eq!(table.alloc(entry), last);
        assert!(!table.has_plain_primary_clause(last));
        assert!(table.has_plain_primary_clause(first));
    }

    #[test]
    fn alloc_and_get_round_trip_a_symbol() {
        let mut table = SymbolTable::new();
        let id = table.alloc(minimal_symbol(None));

        assert_eq!(table.get(id).kind, SymbolKind::Value);
    }

    #[test]
    fn contains_checks_symbol_ids_without_indexing_the_table() {
        let mut table = SymbolTable::new();
        let id = table.alloc(minimal_symbol(None));

        assert!(table.contains(id));
        assert!(!table.contains(SymbolId::new(1)));
    }

    #[test]
    fn owner_reads_the_symbols_owner() {
        let mut table = SymbolTable::new();
        let owner_id = table.alloc(minimal_symbol(None));
        let child_id = table.alloc(minimal_symbol(Some(owner_id)));

        assert_eq!(table.owner(child_id), Some(owner_id));
        assert_eq!(table.owner(owner_id), None);
    }

    #[test]
    fn set_info_updates_the_symbols_info() {
        let mut table = SymbolTable::new();
        let id = table.alloc(minimal_symbol(None));

        table.set_info(id, SymbolInfo::Complete(TypeId::new(7)));

        assert_eq!(*table.info(id), SymbolInfo::Complete(TypeId::new(7)));
    }
}
