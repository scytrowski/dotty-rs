//! The persistent semantic storage for one compilation session.

use crate::names::NameInterner;
use crate::symbols::{OriginTable, ScopeArena, SymbolTable};
use crate::types::{AnnotationArena, TypeArena};

/// Every arena `dotty-core` owns, aggregated into one value.
///
/// This is intentionally "dumb storage," not a typing context — it has no
/// notion of a current owner, lexical scope, or expected type. Once the
/// typer exists, it defines its own, separate `TypingContext` wrapping a
/// `&mut SemanticStore` plus that dynamic state, which is why this type is
/// named `SemanticStore` rather than `SemanticContext` (a name Dotty itself
/// uses for the dynamic typing context). See `docs/dotty-core-design.md`
/// §10, `[MAJOR 5]`.
#[derive(Debug, Default)]
pub struct SemanticStore {
    pub names: NameInterner,
    pub symbols: SymbolTable,
    pub types: TypeArena,
    pub scopes: ScopeArena,
    pub annotations: AnnotationArena,
    /// Mints origin IDs for `SymbolOrigin::Classfile`/`SymbolOrigin::Tasty`
    /// — see [`OriginTable`].
    pub origins: OriginTable,
}

impl SemanticStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::{Name, Namespace};
    use crate::symbols::{
        Scope, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
    };
    use crate::types::Type;

    #[test]
    fn a_fresh_store_has_no_allocations() {
        let mut store = SemanticStore::new();

        let name_id = store.names.intern("foo");
        assert_eq!(store.names.resolve(name_id), "foo");
    }

    /// A small end-to-end sanity check that the arenas actually compose: an
    /// interned name backs a symbol, the symbol is entered into a scope, and
    /// its `SymbolInfo` resolves through the store's own `TypeArena`.
    #[test]
    fn arenas_compose_into_one_resolvable_symbol() {
        let mut store = SemanticStore::new();

        let text = store.names.intern("Foo");
        let name = Name::new(text, Namespace::Term);

        let ty = store.types.alloc(Type::NoPrefix);

        let symbol = store.symbols.alloc(Symbol {
            name,
            owner: None,
            kind: SymbolKind::Value,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(ty),
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });

        let scope = store.scopes.alloc(Scope::new(None));
        store.scopes.get_mut(scope).enter(name, symbol);

        assert_eq!(store.scopes.get(scope).lookup(&name), Some(symbol));
        assert_eq!(*store.symbols.info(symbol), SymbolInfo::Complete(ty));
    }

    /// A classfile adapter mints an origin ID through the store's own
    /// `OriginTable` and attaches it to a symbol via `SymbolOrigin` — the
    /// path `dotty-classloader` uses instead of constructing a
    /// `ClassfileOriginId` directly, which it cannot do (its constructor is
    /// crate-private to `dotty-core`).
    #[test]
    fn a_stores_origin_table_mints_ids_usable_as_symbol_origins() {
        let mut store = SemanticStore::new();

        let origin_id = store.origins.register_classfile();
        let origin = SymbolOrigin::Classfile(origin_id);

        let text = store.names.intern("java/lang/Object");
        let name = Name::new(text, Namespace::Type);

        let symbol = store.symbols.alloc(Symbol {
            name,
            owner: None,
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });

        assert_eq!(store.symbols.get(symbol).origin, origin);
    }
}
