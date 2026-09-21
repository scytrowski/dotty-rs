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
//! `ThisType`, `TypeRef` naming a class, trait, module class or package, and
//! `TermRef` naming a package or an object (through its module class), and
//! `Flexible` and `Annotated` around any of these (looked through, never
//! stripped: both are proxies whose members are their underlying type's, as
//! Dotty's `FlexibleType` and `AnnotatedType` are `CachedProxyType`s).
//! Anything else is `UnsupportedPrefix`. There is no textual fallback and no search across owners. The lookup is not inheritance-aware:
//! it sees the members the prefix's own scope declares.
//!
//! `TYPEREFin` / `TERMREFin` name their declaration through an explicit owner
//! space instead: the space type goes through the same [`lookup_owner`], and
//! [`lookup_declaration`] then reads that owner's own scope. That is Scala's
//! `ownerSpace.decl(name)`. The reference's prefix takes no part in the search.

use dotty_core::Packages;
use dotty_core::ids::{ScopeId, SymbolId, TypeId};
use dotty_core::names::{Name, Namespace};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolFlags, SymbolInfo, SymbolKind};
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

/// How many proxy wrappers ([`proxy_underlying`]) are looked through before
/// the prefix is given up as unsearchable. Real wrappers nest a handful deep;
/// a graph that keeps going is malformed, and the walk must end.
pub(crate) const MAX_PROXY_DEPTH: usize = 64;

/// The type a proxy prefix forwards its members to: the `underlying` of a
/// `Flexible` or `Annotated` type. Nothing else is a proxy here.
pub(crate) fn proxy_underlying(store: &SemanticStore, ty: TypeId) -> Option<TypeId> {
    match store.types.get(ty) {
        Type::Flexible { underlying } | Type::Annotated { underlying, .. } => Some(*underlying),
        _ => None,
    }
}

/// `prefix` with every proxy wrapper looked through, or `None` if there are
/// more than [`MAX_PROXY_DEPTH`] of them. The wrappers stay in the graph.
fn look_through_proxies(store: &SemanticStore, mut prefix: TypeId) -> Option<TypeId> {
    for _ in 0..=MAX_PROXY_DEPTH {
        match proxy_underlying(store, prefix) {
            Some(underlying) => prefix = underlying,
            None => return Some(prefix),
        }
    }
    None
}

/// The symbol whose declarations a prefix type denotes, if the prefix is a
/// form whose lookup semantics are understood.
pub(crate) fn lookup_owner(
    store: &SemanticStore,
    index: &TastySemanticIndex,
    packages: &Packages,
    prefix: TypeId,
) -> Option<SymbolId> {
    let is_scope_owner = |symbol: SymbolId| {
        matches!(
            store.symbols.get(symbol).kind,
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass | SymbolKind::Package
        )
    };
    // A flexible or annotated type has the members of its underlying type;
    // nothing else is looked through.
    let prefix = look_through_proxies(store, prefix)?;
    let ty_symbol = store.types.get(prefix).reference_symbol();
    match store.types.get(prefix) {
        Type::ThisType { class } => Some(*class),
        // A name-designated reference has no symbol, so no declaration scope.
        Type::TypeRef { .. } => ty_symbol.filter(|symbol| is_scope_owner(*symbol)),
        // A term reference is a searchable prefix when it names a package, or
        // an object, whose declarations are those of its module class.
        Type::TermRef { .. } => {
            let symbol = ty_symbol?;
            match store.symbols.get(symbol).kind {
                SymbolKind::Package => Some(symbol),
                SymbolKind::Object => module_class_of(store, index, packages, symbol),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The module class of an object: the `ModuleClass` its owner declares under
/// the object's name with the object-class suffix (`Foo` and `Foo$`), which is
/// how TASTy names the pair. Derived from the owner's scope, as `dotty-core`
/// derives it, not stored, since `SymbolLinks::companion` links a class to its
/// companion object instead. `None` unless exactly one such class is declared.
fn module_class_of(
    store: &SemanticStore,
    index: &TastySemanticIndex,
    packages: &Packages,
    object: SymbolId,
) -> Option<SymbolId> {
    let object = store.symbols.get(object);
    let scope = declaration_scope_of(store, index, packages, object.owner?)?;
    let class_text = format!("{}$", store.names.resolve(object.name.text()));
    let class_name = Name::new(store.names.get(&class_text)?, Namespace::Type);
    match store.scopes.get(scope).lookup_all(&class_name) {
        [only] if store.symbols.get(*only).kind == SymbolKind::ModuleClass => Some(*only),
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
    let Some(owner) = lookup_owner(store, index, packages, prefix) else {
        return LocalLookup::UnsupportedPrefix;
    };
    lookup_declaration(store, index, packages, owner, name)
}

/// Looks `name` up among the declarations of `owner` itself: exact
/// `Name` + `Namespace`, no inheritance. `UnsupportedPrefix` is never the
/// answer here; the caller has already turned its type into an owner.
pub(crate) fn lookup_declaration(
    store: &SemanticStore,
    index: &TastySemanticIndex,
    packages: &Packages,
    owner: SymbolId,
    name: &Name,
) -> LocalLookup {
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

/// Whether `prefix` (with proxies looked through) is not a legal prefix as
/// Dotty's `TypeOps.isLegalPrefix` sees it: a singleton that is not stable, of
/// which a `TermRef` to a method or a mutable member is what the semantic graph can
/// show. Dotty wraps such a prefix in a `QualSkolemType`, which `dotty-core`
/// does not model, so it must not be passed on as if it were legal. A prefix
/// whose proxy chain is too deep is reported as illegal too.
pub(crate) fn is_illegal_prefix(store: &SemanticStore, prefix: TypeId) -> bool {
    let Some(prefix) = look_through_proxies(store, prefix) else {
        return true;
    };
    match store.types.get(prefix) {
        ty @ Type::TermRef { .. } => {
            // A name-designated reference has no symbol to be unstable.
            let Some(symbol) = ty.reference_symbol() else {
                return false;
            };
            let symbol = store.symbols.get(symbol);
            // A `var` member is a `Field` with the `MUTABLE` flag (see
            // `val_def_kind`), so the flag decides, not the kind.
            symbol.flags.contains(SymbolFlags::MUTABLE)
                || matches!(
                    symbol.kind,
                    SymbolKind::Method | SymbolKind::Constructor | SymbolKind::Variable
                )
        }
        _ => false,
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
            self.store
                .types
                .alloc(Type::type_ref(self.no_prefix, symbol))
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
    fn a_flexible_prefix_is_searched_through_its_underlying_type() {
        let mut world = World::new();
        let (class, scope) = world.class("C");
        let inner = world.name("Inner", Namespace::Type);
        let member = world.declare(scope, class, inner, SymbolKind::Class);
        let underlying = world.type_ref(class);
        let flexible = world.store.types.alloc(Type::Flexible { underlying });
        let twice = world.store.types.alloc(Type::Flexible {
            underlying: flexible,
        });

        assert_eq!(world.lookup(flexible, &inner), LocalLookup::Found(member));
        assert_eq!(world.lookup(twice, &inner), LocalLookup::Found(member));
    }

    fn annotated(world: &mut World, underlying: TypeId) -> TypeId {
        let annotation = world
            .store
            .annotations
            .alloc(dotty_core::types::Annotation::new(underlying, None));
        world.store.types.alloc(Type::Annotated {
            underlying,
            annotation,
        })
    }

    #[test]
    fn an_annotated_prefix_is_searched_through_its_underlying_type() {
        let mut world = World::new();
        let (class, scope) = world.class("C");
        let inner = world.name("Inner", Namespace::Type);
        let member = world.declare(scope, class, inner, SymbolKind::Class);
        let underlying = world.type_ref(class);
        let once = annotated(&mut world, underlying);
        let twice = annotated(&mut world, once);

        assert_eq!(world.lookup(once, &inner), LocalLookup::Found(member));
        assert_eq!(world.lookup(twice, &inner), LocalLookup::Found(member));
    }

    #[test]
    fn lookup_looks_through_mixed_flexible_and_annotated_wrappers_without_stripping_them() {
        let mut world = World::new();
        let (class, scope) = world.class("C");
        let inner = world.name("Inner", Namespace::Type);
        let member = world.declare(scope, class, inner, SymbolKind::Class);
        let base = world.type_ref(class);

        let annotated_base = annotated(&mut world, base);
        let flexible_of_annotated = world.store.types.alloc(Type::Flexible {
            underlying: annotated_base,
        });
        let flexible_base = world.store.types.alloc(Type::Flexible { underlying: base });
        let annotated_of_flexible = annotated(&mut world, flexible_base);

        assert_eq!(
            world.lookup(flexible_of_annotated, &inner),
            LocalLookup::Found(member)
        );
        assert_eq!(
            world.lookup(annotated_of_flexible, &inner),
            LocalLookup::Found(member)
        );
        // Lookup reads the graph; the wrappers are still there afterwards.
        assert!(matches!(
            world.store.types.get(flexible_of_annotated),
            Type::Flexible { underlying } if *underlying == annotated_base
        ));
        assert!(matches!(
            world.store.types.get(annotated_of_flexible),
            Type::Annotated { underlying, .. } if *underlying == flexible_base
        ));
    }

    #[test]
    fn an_annotated_wrapper_around_an_unsearchable_type_stays_unsupported() {
        let mut world = World::new();
        let inner = world.name("Inner", Namespace::Type);
        let no_prefix = world.no_prefix;
        let wrapped = annotated(&mut world, no_prefix);

        assert_eq!(
            world.lookup(wrapped, &inner),
            LocalLookup::UnsupportedPrefix
        );
    }

    #[test]
    fn a_proxy_chain_deeper_than_the_bound_is_unsearchable_not_a_walk_without_end() {
        let mut world = World::new();
        let (class, scope) = world.class("C");
        let inner = world.name("Inner", Namespace::Type);
        let member = world.declare(scope, class, inner, SymbolKind::Class);
        let mut wrapped = world.type_ref(class);
        for depth in 0..MAX_PROXY_DEPTH + 2 {
            wrapped = annotated(&mut world, wrapped);
            let expected = if depth < MAX_PROXY_DEPTH {
                LocalLookup::Found(member)
            } else {
                LocalLookup::UnsupportedPrefix
            };
            assert_eq!(world.lookup(wrapped, &inner), expected, "depth {depth}");
        }
    }

    #[test]
    fn a_flexible_wrapper_around_an_unsearchable_type_stays_unsupported() {
        let mut world = World::new();
        let inner = world.name("Inner", Namespace::Type);
        let flexible = world.store.types.alloc(Type::Flexible {
            underlying: world.no_prefix,
        });

        assert_eq!(
            world.lookup(flexible, &inner),
            LocalLookup::UnsupportedPrefix
        );
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
        let term = world
            .store
            .types
            .alloc(Type::term_ref(world.no_prefix, chain[0].symbol));
        let ty = world.type_ref(chain[0].symbol);

        // Neither this index nor `ClassInfo` knows the package.
        assert_eq!(world.index.scope_of(chain[0].symbol), None);
        assert_eq!(world.lookup(term, &class_name), LocalLookup::Found(member));
        assert_eq!(world.lookup(ty, &class_name), LocalLookup::Found(member));
    }

    #[test]
    fn an_object_prefix_is_searched_through_its_module_class() {
        let mut world = World::new();
        let (owner, owner_scope) = world.class("Outer");
        let object_name = world.name("Obj", Namespace::Term);
        let class_name = world.name("Obj$", Namespace::Type);
        let object = world.declare(owner_scope, owner, object_name, SymbolKind::Object);
        let module_class = world.declare(owner_scope, owner, class_name, SymbolKind::ModuleClass);
        let module_scope = world.store.scopes.alloc(Scope::new(Some(module_class)));
        world
            .index
            .insert_scope(module_class, module_scope)
            .unwrap();
        let inner = world.name("Inner", Namespace::Type);
        let member = world.declare(module_scope, module_class, inner, SymbolKind::Class);
        let prefix = world
            .store
            .types
            .alloc(Type::term_ref(world.no_prefix, object));

        assert_eq!(world.lookup(prefix, &inner), LocalLookup::Found(member));
    }

    #[test]
    fn an_object_without_a_module_class_is_not_searched() {
        let mut world = World::new();
        let (owner, owner_scope) = world.class("Outer");
        let object_name = world.name("Obj", Namespace::Term);
        let object = world.declare(owner_scope, owner, object_name, SymbolKind::Object);
        let inner = world.name("Inner", Namespace::Type);
        let prefix = world
            .store
            .types
            .alloc(Type::term_ref(world.no_prefix, object));

        assert_eq!(world.lookup(prefix, &inner), LocalLookup::UnsupportedPrefix);
    }

    #[test]
    fn prefixes_without_lookup_semantics_are_unsupported() {
        let mut world = World::new();
        let field_name = world.name("v", Namespace::Term);
        let field = world.symbol(field_name, SymbolKind::Field, None);
        let inner = world.name("Inner", Namespace::Type);
        let term_of_field = world
            .store
            .types
            .alloc(Type::term_ref(world.no_prefix, field));

        assert_eq!(
            world.lookup(world.no_prefix, &inner),
            LocalLookup::UnsupportedPrefix
        );
        assert_eq!(
            world.lookup(term_of_field, &inner),
            LocalLookup::UnsupportedPrefix
        );
    }

    fn declaration(world: &World, owner: SymbolId, name: &Name) -> LocalLookup {
        lookup_declaration(&world.store, &world.index, &world.packages, owner, name)
    }

    #[test]
    fn a_declaration_lookup_sees_only_the_owners_own_declarations() {
        let mut world = World::new();
        let (left, left_scope) = world.class("Left");
        let (right, right_scope) = world.class("Right");
        let secret = world.name("secret", Namespace::Term);
        let left_secret = world.declare(left_scope, left, secret, SymbolKind::Value);
        let right_secret = world.declare(right_scope, right, secret, SymbolKind::Value);

        assert_eq!(
            declaration(&world, left, &secret),
            LocalLookup::Found(left_secret)
        );
        assert_eq!(
            declaration(&world, right, &secret),
            LocalLookup::Found(right_secret)
        );
    }

    #[test]
    fn a_declaration_lookup_is_exact_in_namespace_and_reports_misses_and_overloads() {
        let mut world = World::new();
        let (class, scope) = world.class("C");
        let term = world.name("x", Namespace::Term);
        let ty = world.name("x", Namespace::Type);
        let only = world.declare(scope, class, term, SymbolKind::Value);
        let (unfilled, _) = (world.class("Unfilled").0, ());
        // A class with no scope entered here.
        let no_scope_name = world.name("Elsewhere", Namespace::Type);
        let elsewhere = world.symbol(no_scope_name, SymbolKind::Class, None);

        assert_eq!(declaration(&world, class, &term), LocalLookup::Found(only));
        assert_eq!(declaration(&world, class, &ty), LocalLookup::NotFound);
        assert_eq!(declaration(&world, unfilled, &term), LocalLookup::NotFound);
        assert_eq!(
            declaration(&world, elsewhere, &term),
            LocalLookup::ScopeUnknown
        );

        world.declare(scope, class, term, SymbolKind::Method);
        assert_eq!(
            declaration(&world, class, &term),
            LocalLookup::Ambiguous { candidates: 2 }
        );
    }

    #[test]
    fn only_a_method_or_variable_term_is_an_illegal_prefix() {
        let mut world = World::new();
        let (class, _) = world.class("C");
        let no_prefix = world.no_prefix;
        let term_ref = |world: &mut World, text: &str, kind| {
            let name = world.name(text, Namespace::Term);
            let symbol = world.symbol(name, kind, None);
            world.store.types.alloc(Type::term_ref(no_prefix, symbol))
        };
        let method = term_ref(&mut world, "m", SymbolKind::Method);
        let variable = term_ref(&mut world, "v", SymbolKind::Variable);
        let value = term_ref(&mut world, "x", SymbolKind::Value);
        let object = term_ref(&mut world, "o", SymbolKind::Object);
        // A class `var` is a `Field` with the `MUTABLE` flag, as pass 1 enters it.
        let var_name = world.name("w", Namespace::Term);
        let var_symbol = world.symbol(var_name, SymbolKind::Field, None);
        world.store.symbols.get_mut(var_symbol).flags = SymbolFlags::MUTABLE;
        let mutable_field = world
            .store
            .types
            .alloc(Type::term_ref(no_prefix, var_symbol));
        let plain_field = term_ref(&mut world, "g", SymbolKind::Field);
        let class_ref = world.type_ref(class);
        let wrapped_method = annotated(&mut world, method);

        assert!(is_illegal_prefix(&world.store, method));
        assert!(is_illegal_prefix(&world.store, variable));
        assert!(is_illegal_prefix(&world.store, mutable_field));
        assert!(!is_illegal_prefix(&world.store, plain_field));
        assert!(is_illegal_prefix(&world.store, wrapped_method));
        assert!(!is_illegal_prefix(&world.store, value));
        assert!(!is_illegal_prefix(&world.store, object));
        assert!(!is_illegal_prefix(&world.store, class_ref));
        assert!(!is_illegal_prefix(&world.store, no_prefix));
    }
}
