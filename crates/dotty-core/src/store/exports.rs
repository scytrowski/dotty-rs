//! Publication of source export forwarders in an existing declaration scope.

use crate::{ScopeId, SemanticStore, Symbol, SymbolFlags, SymbolId, SymbolKind, Type, TypeId};

#[derive(Debug)]
pub(super) struct ExportEntry {
    pub scope: ScopeId,
    pub method: SymbolId,
    pub target: TypeId,
}

impl SemanticStore {
    /// Publishes a fresh exported method in its owner's declaration scope and
    /// retains the qualified term reference that its body must delegate to.
    /// All IDs must belong to this store. Returns `None` without publication for
    /// an invalid scope, owner, method, reference or occupied declaration name.
    /// The caller must verify the target's visibility and the declaration's
    /// signature in the export site's lexical context. Rolling back past this
    /// call also removes the entry from the scope.
    ///
    /// # Panics
    ///
    /// Panics if allocating the method exhausts the store's symbol ID space.
    pub fn enter_export_forwarder(
        &mut self,
        scope: ScopeId,
        declaration: Symbol,
        target: TypeId,
    ) -> Option<SymbolId> {
        let Type::TermRef {
            prefix,
            target: crate::types::TermRefTarget::Symbol(member),
        } = self.types.try_get(target)?
        else {
            return None;
        };
        let receiver = self.types.try_get(*prefix)?.reference_symbol()?;
        if !self.symbols.contains(receiver)
            || self.symbols.get(receiver).kind != SymbolKind::Object
            || !self.symbols.contains(*member)
            || !self.scopes.contains(scope)
            || !declaration
                .owner
                .is_some_and(|owner| self.symbols.contains(owner))
            || declaration.owner != self.scopes.get(scope).owner
            || declaration.kind != SymbolKind::Method
            || !declaration.flags.contains(SymbolFlags::EXPORTED)
            || !self
                .scopes
                .get(scope)
                .lookup_all(&declaration.name)
                .is_empty()
        {
            return None;
        }
        let name = declaration.name;
        let method = self.symbols.alloc(declaration);
        self.scopes.get_mut(scope).enter(name, method);
        self.exports.push(ExportEntry {
            scope,
            method,
            target,
        });
        Some(method)
    }

    /// Returns the original qualified member reference of a live source export
    /// forwarder. The prefix identifies the receiver even for a renamed export.
    pub fn export_target(&self, method: SymbolId) -> Option<TypeId> {
        let i = self
            .exports
            .binary_search_by_key(&method.index(), |entry| entry.method.index())
            .ok()?;
        Some(self.exports[i].target)
    }
}
