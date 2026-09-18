//! Allocates and looks up [`Symbol`] values.

use crate::ids::{SymbolId, checked_index};
use crate::symbols::completion::SymbolInfo;
use crate::symbols::symbol::Symbol;

/// Owns every [`Symbol`] for one compilation session.
#[derive(Debug, Default)]
pub struct SymbolTable {
    symbols: Vec<Symbol>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alloc(&mut self, symbol: Symbol) -> SymbolId {
        let id = SymbolId::new(checked_index(self.symbols.len()));
        self.symbols.push(symbol);
        id
    }

    /// Panics if `id` was not allocated by this table — see
    /// `docs/dotty-core-design.md`, "Error handling policy."
    pub fn get(&self, id: SymbolId) -> &Symbol {
        &self.symbols[id.index() as usize]
    }

    pub fn get_mut(&mut self, id: SymbolId) -> &mut Symbol {
        &mut self.symbols[id.index() as usize]
    }

    pub fn owner(&self, id: SymbolId) -> Option<SymbolId> {
        self.get(id).owner
    }

    pub fn info(&self, id: SymbolId) -> &SymbolInfo {
        &self.get(id).info
    }

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
    fn alloc_and_get_round_trip_a_symbol() {
        let mut table = SymbolTable::new();
        let id = table.alloc(minimal_symbol(None));

        assert_eq!(table.get(id).kind, SymbolKind::Value);
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
