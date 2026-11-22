//! Publication of source export forwarders in an existing declaration scope.

use crate::types::TermRefTarget;
use crate::{
    ScopeId, SemanticStore, Symbol, SymbolFlags, SymbolId, SymbolInfo, SymbolKind, Type, TypeId,
};

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
    /// The caller must verify membership in the receiver, visibility, and the method's
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
            target: TermRefTarget::Symbol(member),
        } = self.types.try_get(target)?
        else {
            return None;
        };
        let Type::TermRef {
            target: TermRefTarget::Symbol(receiver),
            ..
        } = *self.types.try_get(*prefix)?
        else {
            return None;
        };
        if !matches!(declaration.info, SymbolInfo::Complete(ty) if self.types.try_get(ty).is_some())
            || !self.symbols.contains(receiver)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Name, Namespace, Scope, SymbolOrigin, Visibility};

    #[test]
    fn only_an_object_term_path_can_be_published_and_publication_rolls_back() {
        let mut store = SemanticStore::new();
        let name = Name::new(store.names.intern("member"), Namespace::Term);
        let no_prefix = store.types.alloc(Type::NoPrefix);
        let mut decl = Symbol {
            name,
            owner: None,
            kind: SymbolKind::Object,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(no_prefix),
            origin: SymbolOrigin::Synthetic,
            annotations: vec![],
            position: None,
            links: Default::default(),
        };
        let receiver = store.symbols.alloc(decl.clone());
        decl.kind = SymbolKind::Field;
        decl.owner = Some(receiver);
        let member = store.symbols.alloc(decl.clone());
        let scope = store.scopes.alloc(Scope::new(Some(receiver)));
        decl.kind = SymbolKind::Method;
        decl.flags = SymbolFlags::EXPORTED;
        let wrong = store.types.alloc(Type::type_ref(no_prefix, receiver));
        let path = store.types.alloc(Type::term_ref(no_prefix, receiver));
        let invalid = store.types.alloc(Type::term_ref(wrong, member));
        let target = store.types.alloc(Type::term_ref(path, member));
        let checkpoint = store.checkpoint();
        assert_eq!(
            store.enter_export_forwarder(scope, decl.clone(), invalid),
            None
        );
        assert_eq!(store.checkpoint(), checkpoint);
        let method = store
            .enter_export_forwarder(scope, decl.clone(), target)
            .unwrap();
        assert_eq!(store.export_target(method), Some(target));
        assert_eq!(store.enter_export_forwarder(scope, decl, target), None);
        assert_eq!(store.scopes.get(scope).lookup_all(&name), &[method]);
        store.rollback_to(checkpoint);
        assert_eq!(store.export_target(method), None);
        assert!(store.scopes.get(scope).lookup_all(&name).is_empty());
    }
}
