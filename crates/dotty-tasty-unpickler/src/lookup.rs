//! Local member lookup: finds the symbol a name-based reference means, from
//! the semantic state already entered, before any external resolver is asked.
//!
//! The prefix decides where to look, never the name alone:
//!
//! ```text
//! prefix TypeId -> lookup owner -> declaration scope -> Name + Namespace
//! ```
//!
//! Only prefixes whose lookup semantics are understood are searched:
//! `ThisType`, and `TypeRef`/`TermRef` naming a class, trait, module class or
//! package. Anything else is `UnsupportedPrefix`. There is no textual
//! fallback and no search across owners. The lookup is not inheritance-aware:
//! it sees the members the prefix's own scope declares.

use dotty_core::Packages;
use dotty_core::ids::{ScopeId, SymbolId, TypeId};
use dotty_core::names::Name;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolKind};
use dotty_core::types::Type;

use crate::index::TastySemanticIndex;

/// The outcome of looking a member up in the semantic state already entered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalLookup {
    /// Exactly one symbol.
    Found(SymbolId),
    /// The prefix is understood and its scope holds no such member. Not
    /// evidence the member does not exist: another unit or a classpath may
    /// hold it, so the external resolver is asked.
    NotFound,
    /// The prefix is understood, but its declaration scope is not known here
    /// (a class of another unit that is not completed). Also for the
    /// resolver.
    ScopeUnknown,
    /// The scope holds several symbols under the name and the request names
    /// one (overloads).
    Ambiguous { candidates: usize },
    /// The prefix has no lookup semantics defined.
    UnsupportedPrefix,
}

/// The symbol whose declarations a prefix type denotes, if the prefix is a
/// form whose lookup semantics are understood.
pub(crate) fn lookup_owner(store: &SemanticStore, prefix: TypeId) -> Option<SymbolId> {
    let is_scope_owner = |symbol: SymbolId| {
        matches!(
            store.symbols.get(symbol).kind,
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass | SymbolKind::Package
        )
    };
    match store.types.get(prefix) {
        Type::ThisType { class } => Some(*class),
        Type::TypeRef { symbol, .. } if is_scope_owner(*symbol) => Some(*symbol),
        // A term reference is a searchable prefix only when it names a
        // package. An object's declarations are found through its module
        // class, which the core does not link to the object yet.
        Type::TermRef { symbol, .. } if store.symbols.get(*symbol).kind == SymbolKind::Package => {
            Some(*symbol)
        }
        _ => None,
    }
}

/// The declaration scope of `symbol`: the scope this unit entered for it, the
/// scope of the package registry, or the `declarations` of its completed
/// `ClassInfo`, in that order.
pub(crate) fn declaration_scope_of(
    store: &SemanticStore,
    index: &TastySemanticIndex,
    packages: &Packages,
    symbol: SymbolId,
) -> Option<ScopeId> {
    if let Some(scope) = index.scope_of(symbol) {
        return Some(scope);
    }
    if let Some(scope) = packages.scope_of(symbol) {
        return Some(scope);
    }
    let SymbolInfo::Complete(ty) = store.symbols.get(symbol).info else {
        return None;
    };
    match store.types.get(ty) {
        Type::ClassInfo(info) => Some(info.declarations),
        _ => None,
    }
}

/// Looks `name` up among the members of `prefix`. `name` carries the
/// namespace, so a term never matches a type of the same text.
pub(crate) fn lookup_member(
    store: &SemanticStore,
    index: &TastySemanticIndex,
    packages: &Packages,
    prefix: TypeId,
    name: &Name,
) -> LocalLookup {
    let Some(owner) = lookup_owner(store, prefix) else {
        return LocalLookup::UnsupportedPrefix;
    };
    let Some(scope) = declaration_scope_of(store, index, packages, owner) else {
        return LocalLookup::ScopeUnknown;
    };
    match store.scopes.get(scope).lookup_all(name) {
        [] => LocalLookup::NotFound,
        [only] => LocalLookup::Found(*only),
        several => LocalLookup::Ambiguous {
            candidates: several.len(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::names::Namespace;
    use dotty_core::symbols::{Scope, Symbol, SymbolFlags, SymbolLinks, SymbolOrigin, Visibility};
    use dotty_core::types::ClassInfo;

    struct World {
        store: SemanticStore,
        index: TastySemanticIndex,
        packages: Packages,
        no_prefix: TypeId,
    }

    impl World {
        fn new() -> Self {
            let mut store = SemanticStore::new();
            let no_prefix = store.types.alloc(Type::NoPrefix);
            Self {
                store,
                index: TastySemanticIndex::new(),
                packages: Packages::new(),
                no_prefix,
            }
        }

        fn name(&mut self, text: &str, namespace: Namespace) -> Name {
            Name::new(self.store.names.intern(text), namespace)
        }

        fn symbol(&mut self, name: Name, kind: SymbolKind, owner: Option<SymbolId>) -> SymbolId {
            self.store.symbols.alloc(Symbol {
                name,
                owner,
                kind,
                flags: SymbolFlags::EMPTY,
                visibility: Visibility::Public,
                info: SymbolInfo::Missing,
                origin: SymbolOrigin::Synthetic,
                annotations: Vec::new(),
                position: None,
                links: SymbolLinks::default(),
            })
        }

        /// A class with an index-recorded scope.
        fn class(&mut self, text: &str) -> (SymbolId, ScopeId) {
            let name = self.name(text, Namespace::Type);
            let class = self.symbol(name, SymbolKind::Class, None);
            let scope = self.store.scopes.alloc(Scope::new(Some(class)));
            self.index.insert_scope(class, scope).unwrap();
            (class, scope)
        }

        fn declare(
            &mut self,
            scope: ScopeId,
            owner: SymbolId,
            name: Name,
            kind: SymbolKind,
        ) -> SymbolId {
            let member = self.symbol(name, kind, Some(owner));
            self.store.scopes.get_mut(scope).enter(name, member);
            member
        }

        fn type_ref(&mut self, symbol: SymbolId) -> TypeId {
            self.store.types.alloc(Type::TypeRef {
                prefix: self.no_prefix,
                symbol,
            })
        }

        fn lookup(&self, prefix: TypeId, name: &Name) -> LocalLookup {
            lookup_member(&self.store, &self.index, &self.packages, prefix, name)
        }
    }

    #[test]
    fn the_prefix_decides_which_same_named_member_is_found() {
        let mut world = World::new();
        let (left, left_scope) = world.class("Left");
        let (right, right_scope) = world.class("Right");
        let inner = world.name("Inner", Namespace::Type);
        let left_inner = world.declare(left_scope, left, inner, SymbolKind::Class);
        let right_inner = world.declare(right_scope, right, inner, SymbolKind::Class);
        let left_prefix = world.type_ref(left);
        let right_prefix = world.type_ref(right);

        assert_eq!(
            world.lookup(left_prefix, &inner),
            LocalLookup::Found(left_inner)
        );
        assert_eq!(
            world.lookup(right_prefix, &inner),
            LocalLookup::Found(right_inner)
        );
    }

    #[test]
    fn a_this_type_prefix_searches_its_class() {
        let mut world = World::new();
        let (class, scope) = world.class("C");
        let inner = world.name("Inner", Namespace::Type);
        let member = world.declare(scope, class, inner, SymbolKind::Class);
        let prefix = world.store.types.alloc(Type::ThisType { class });

        assert_eq!(world.lookup(prefix, &inner), LocalLookup::Found(member));
    }

    #[test]
    fn the_namespace_of_the_name_is_respected() {
        let mut world = World::new();
        let (class, scope) = world.class("C");
        let as_type = world.name("X", Namespace::Type);
        let as_term = world.name("X", Namespace::Term);
        let type_member = world.declare(scope, class, as_type, SymbolKind::Class);
        let term_member = world.declare(scope, class, as_term, SymbolKind::Field);
        let prefix = world.type_ref(class);

        assert_eq!(
            world.lookup(prefix, &as_type),
            LocalLookup::Found(type_member)
        );
        assert_eq!(
            world.lookup(prefix, &as_term),
            LocalLookup::Found(term_member)
        );
    }

    #[test]
    fn overloads_are_ambiguous_not_first_wins() {
        let mut world = World::new();
        let (class, scope) = world.class("Owner");
        let f = world.name("f", Namespace::Term);
        world.declare(scope, class, f, SymbolKind::Method);
        world.declare(scope, class, f, SymbolKind::Method);
        let prefix = world.type_ref(class);

        assert_eq!(
            world.lookup(prefix, &f),
            LocalLookup::Ambiguous { candidates: 2 }
        );
    }

    #[test]
    fn a_missing_member_is_not_found_and_not_a_guess() {
        let mut world = World::new();
        let (class, _) = world.class("C");
        let (other, other_scope) = world.class("Other");
        let inner = world.name("Inner", Namespace::Type);
        world.declare(other_scope, other, inner, SymbolKind::Class);
        let prefix = world.type_ref(class);

        // `Other` has an `Inner`; `C` does not, and no global search happens.
        assert_eq!(world.lookup(prefix, &inner), LocalLookup::NotFound);
    }

    #[test]
    fn a_completed_class_is_searched_through_its_class_info() {
        let mut world = World::new();
        let name = world.name("Loaded", Namespace::Type);
        let class = world.symbol(name, SymbolKind::Class, None);
        let scope = world.store.scopes.alloc(Scope::new(Some(class)));
        let inner = world.name("Inner", Namespace::Type);
        let member = world.declare(scope, class, inner, SymbolKind::Class);
        let info = world.store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: world.no_prefix,
            class,
            parents: Vec::new(),
            declarations: scope,
            self_type: None,
        }));
        world.store.symbols.get_mut(class).info = SymbolInfo::Complete(info);
        let prefix = world.type_ref(class);

        // Not in this unit's index: found through `ClassInfo.declarations`.
        assert_eq!(world.index.scope_of(class), None);
        assert_eq!(world.lookup(prefix, &inner), LocalLookup::Found(member));
    }

    #[test]
    fn a_class_with_no_known_scope_is_scope_unknown() {
        let mut world = World::new();
        let name = world.name("Elsewhere", Namespace::Type);
        let class = world.symbol(name, SymbolKind::Class, None);
        let inner = world.name("Inner", Namespace::Type);
        let prefix = world.type_ref(class);

        assert_eq!(world.lookup(prefix, &inner), LocalLookup::ScopeUnknown);
    }

    #[test]
    fn a_package_prefix_is_searched_through_the_registry() {
        let mut world = World::new();
        let chain = world
            .packages
            .enter(&mut world.store, SymbolOrigin::Synthetic, &["a"]);
        let class_name = world.name("C", Namespace::Type);
        let member = world.declare(
            chain[0].scope,
            chain[0].symbol,
            class_name,
            SymbolKind::Class,
        );
        let term = world.store.types.alloc(Type::TermRef {
            prefix: world.no_prefix,
            symbol: chain[0].symbol,
        });
        let ty = world.type_ref(chain[0].symbol);

        // Neither this index nor `ClassInfo` knows the package.
        assert_eq!(world.index.scope_of(chain[0].symbol), None);
        assert_eq!(world.lookup(term, &class_name), LocalLookup::Found(member));
        assert_eq!(world.lookup(ty, &class_name), LocalLookup::Found(member));
    }

    #[test]
    fn prefixes_without_lookup_semantics_are_unsupported() {
        let mut world = World::new();
        let field_name = world.name("v", Namespace::Term);
        let field = world.symbol(field_name, SymbolKind::Field, None);
        let inner = world.name("Inner", Namespace::Type);
        let term_of_field = world.store.types.alloc(Type::TermRef {
            prefix: world.no_prefix,
            symbol: field,
        });

        assert_eq!(
            world.lookup(world.no_prefix, &inner),
            LocalLookup::UnsupportedPrefix
        );
        assert_eq!(
            world.lookup(term_of_field, &inner),
            LocalLookup::UnsupportedPrefix
        );
    }
}
