//! The one canonical set of builtin symbols/types every compilation session
//! shares.
//!
//! Descriptor/signature lowering (a JVM `I` or a source `Int`) needs a
//! stable `TypeId` for each JVM primitive plus `Object`/`Any`/`Nothing`, and
//! must not mint a fresh symbol every time one is encountered — two `Int`
//! occurrences in the same session have to resolve to the exact same
//! `TypeId`, or e.g. overload resolution and type equality would see them as
//! unrelated types. [`Definitions::bootstrap`] mints each one exactly once.

use crate::ids::{SymbolId, TypeId};
use crate::names::{Name, Namespace};
use crate::packages::Packages;
use crate::store::SemanticStore;
use crate::symbols::{
    Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
};
use crate::types::Type;

/// The builtin symbols and types every adapter (classfile/TASTy/source)
/// lowers primitives and `Object`/`Any`/`Nothing` references against.
///
/// Exactly one `Definitions` should exist per [`SemanticStore`] /
/// compilation session. `SemanticStore` itself does not own a `Definitions`
/// field — unlike `symbols`/`types`/`origins`, it is not "dumb storage" with
/// no notion of a session's dynamic state, it is a fixed, precomputed set of
/// identities *within* that storage. A caller constructs it once, right
/// after `SemanticStore::new()`, with [`Definitions::bootstrap`], and then
/// threads `&Definitions` alongside `&mut SemanticStore` everywhere lowering
/// happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Definitions {
    /// The one `Type::NoPrefix` every builtin `Type::TypeRef` here uses,
    /// and that every other session-wide `Type::TypeRef`/`Type::ClassInfo`
    /// prefix should reuse too — see this type's own doc comment: two
    /// `TypeRef`s naming the same symbol must also agree on `prefix`'s
    /// identity to be the same `TypeId`, and `NoPrefix` has no further
    /// structure to compare, so allocating more than one per session only
    /// creates spurious `TypeId` distinctions.
    pub no_prefix: TypeId,
    pub byte: TypeId,
    pub char: TypeId,
    pub double: TypeId,
    pub float: TypeId,
    pub int: TypeId,
    pub long: TypeId,
    pub short: TypeId,
    pub boolean: TypeId,
    /// What a JVM `void` return type / Scala `Unit` lowers to.
    pub unit: TypeId,
    /// The canonical source `Object` reference type.
    pub object_type: TypeId,
    /// The canonical source `Any` reference type.
    pub any_type: TypeId,
    /// The canonical source `Nothing` reference type.
    pub nothing_type: TypeId,

    /// `java.lang.Object`, the JVM upper bound (`Any`'s reference-type
    /// counterpart, `AnyRef`, is not distinguished from it yet).
    pub object_class: SymbolId,
    /// Scala's top type, the upper bound `Type::Wildcard`/`Type::Bounds`
    /// lowering uses when a generic signature does not specify one.
    pub any_class: SymbolId,
    /// Scala's bottom type, the lower bound the same lowering uses when a
    /// wildcard does not specify one (e.g. `? extends Number`).
    pub nothing_class: SymbolId,
    /// The identity of `scala.&`, Dotty's `defn.andType`: the special type
    /// alias an applied source-level intersection `A & B` names. No file
    /// declares it, so [`declare_special_aliases`](Self::declare_special_aliases)
    /// binds this very symbol as `&` in the `scala` package; a TASTy type tree applying it is normalized to
    /// [`Type::And`], as upstream's `processAppliedType` does. Identity, never
    /// the text `&`: a same-named symbol of another owner is not this one.
    pub and_type: SymbolId,
    /// The identity of `scala.|`, `defn.orType`; see [`and_type`](Self::and_type).
    pub or_type: SymbolId,
}

impl Definitions {
    /// Mints every builtin's `Symbol`/`TypeId` in `store`, once.
    ///
    /// Each primitive becomes a synthetic `SymbolKind::Class` symbol
    /// (`SymbolOrigin::Builtin`) with a `Type::TypeRef` pointing at it — the
    /// same shape a loaded JVM class ultimately takes — so descriptor/
    /// signature lowering never has to special-case "is this a primitive."
    pub fn bootstrap(store: &mut SemanticStore) -> Self {
        let no_prefix = store.types.alloc(Type::NoPrefix);

        let byte = Self::primitive(store, no_prefix, "Byte");
        let char = Self::primitive(store, no_prefix, "Char");
        let double = Self::primitive(store, no_prefix, "Double");
        let float = Self::primitive(store, no_prefix, "Float");
        let int = Self::primitive(store, no_prefix, "Int");
        let long = Self::primitive(store, no_prefix, "Long");
        let short = Self::primitive(store, no_prefix, "Short");
        let boolean = Self::primitive(store, no_prefix, "Boolean");
        let unit = Self::primitive(store, no_prefix, "Unit");

        let object_class = Self::builtin_symbol(store, "Object");
        let any_class = Self::builtin_symbol(store, "Any");
        let nothing_class = Self::builtin_symbol(store, "Nothing");
        let and_type = Self::builtin_alias(store, "&");
        let or_type = Self::builtin_alias(store, "|");
        let object_type = store.types.alloc(Type::type_ref(no_prefix, object_class));
        let any_type = store.types.alloc(Type::type_ref(no_prefix, any_class));
        let nothing_type = store.types.alloc(Type::type_ref(no_prefix, nothing_class));

        Self {
            no_prefix,
            byte,
            char,
            double,
            float,
            int,
            long,
            short,
            boolean,
            unit,
            object_type,
            any_type,
            nothing_type,
            object_class,
            any_class,
            nothing_class,
            and_type,
            or_type,
        }
    }

    /// Returns the session-canonical type for a source-level builtin name.
    ///
    /// Lookup compares interned names and does not depend on builtin symbols
    /// being entered in a source lexical scope.
    pub fn source_builtin_type(&self, store: &SemanticStore, name: Name) -> Option<TypeId> {
        [
            self.byte,
            self.char,
            self.double,
            self.float,
            self.int,
            self.long,
            self.short,
            self.boolean,
            self.unit,
            self.object_type,
            self.any_type,
            self.nothing_type,
        ]
        .into_iter()
        .find(|ty| {
            let Type::TypeRef {
                target: crate::types::TypeRefTarget::Symbol(symbol),
                ..
            } = store.types.get(*ty)
            else {
                return false;
            };
            store.symbols.get(*symbol).name == name
        })
    }

    /// Declares [`and_type`](Self::and_type) and [`or_type`](Self::or_type) as
    /// `&` and `|` in the `scala` package of `packages`, entering that package
    /// if needed. No TASTy or class file declares them (the compiler does), so
    /// a session must; `TastyUnpickler` calls this for its package registry.
    /// Idempotent: a symbol already declared is left alone.
    pub fn declare_special_aliases(&self, store: &mut SemanticStore, packages: &mut Packages) {
        let chain = packages.enter(store, SymbolOrigin::Builtin, &["scala"]);
        let scala = chain
            .last()
            .unwrap_or_else(|| unreachable!("a non-empty path enters a package"));
        for (text, symbol) in [("&", self.and_type), ("|", self.or_type)] {
            let name = Name::new(store.names.intern(text), Namespace::Type);
            if store
                .scopes
                .get(scala.scope)
                .lookup_all(&name)
                .contains(&symbol)
            {
                continue;
            }
            store.symbols.get_mut(symbol).owner = Some(scala.symbol);
            store.scopes.get_mut(scala.scope).enter(name, symbol);
        }
    }

    fn builtin_alias(store: &mut SemanticStore, name: &str) -> SymbolId {
        let id = Self::builtin_symbol(store, name);
        store.symbols.get_mut(id).kind = SymbolKind::TypeAlias;
        id
    }

    fn builtin_symbol(store: &mut SemanticStore, name: &str) -> SymbolId {
        let text = store.names.intern(name);
        let name = Name::new(text, Namespace::Type);

        store.symbols.alloc(Symbol {
            name,
            owner: None,
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Builtin,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        })
    }

    fn primitive(store: &mut SemanticStore, no_prefix: TypeId, name: &str) -> TypeId {
        let symbol = Self::builtin_symbol(store, name);
        store.types.alloc(Type::type_ref(no_prefix, symbol))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_gives_every_primitive_a_distinct_type_id() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);

        let ids = [
            definitions.byte,
            definitions.char,
            definitions.double,
            definitions.float,
            definitions.int,
            definitions.long,
            definitions.short,
            definitions.boolean,
            definitions.unit,
        ];

        for (i, a) in ids.iter().enumerate() {
            for (j, b) in ids.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "primitives {i} and {j} share a TypeId");
                }
            }
        }
    }

    #[test]
    fn bootstrap_mints_the_special_intersection_and_union_aliases() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);

        assert_ne!(definitions.and_type, definitions.or_type);
        for (symbol, text) in [(definitions.and_type, "&"), (definitions.or_type, "|")] {
            let symbol = store.symbols.get(symbol);
            assert_eq!(symbol.kind, SymbolKind::TypeAlias);
            assert_eq!(symbol.origin, SymbolOrigin::Builtin);
            assert_eq!(store.names.resolve(symbol.name.text()), text);
        }
    }

    #[test]
    fn declaring_the_special_aliases_binds_them_in_the_scala_package_once() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();

        definitions.declare_special_aliases(&mut store, &mut packages);
        definitions.declare_special_aliases(&mut store, &mut packages);

        let scala = packages.enter(&mut store, SymbolOrigin::Synthetic, &["scala"]);
        let scope = scala.last().unwrap().scope;
        for (text, symbol) in [("&", definitions.and_type), ("|", definitions.or_type)] {
            let name = Name::new(store.names.intern(text), Namespace::Type);
            assert_eq!(store.scopes.get(scope).lookup_all(&name), [symbol]);
            assert_eq!(
                store.symbols.get(symbol).owner,
                Some(scala.last().unwrap().symbol)
            );
        }
    }

    #[test]
    fn bootstrap_gives_object_any_and_nothing_distinct_symbol_ids() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);

        assert_ne!(definitions.object_class, definitions.any_class);
        assert_ne!(definitions.object_class, definitions.nothing_class);
        assert_ne!(definitions.any_class, definitions.nothing_class);
    }

    #[test]
    fn each_primitive_type_id_resolves_to_a_type_ref_with_a_builtin_origin_symbol() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);

        let int = store.types.get(definitions.int);
        let (Type::TypeRef { .. }, Some(symbol)) = (int, int.reference_symbol()) else {
            panic!("expected Int to lower to a symbol-designated Type::TypeRef");
        };

        assert_eq!(store.symbols.get(symbol).origin, SymbolOrigin::Builtin);
        assert_eq!(store.symbols.get(symbol).kind, SymbolKind::Class);
    }

    /// `bootstrap` mints real identities against whatever store it is given,
    /// rather than sharing one global set — each store's `Definitions` must
    /// only be resolved against that same store.
    #[test]
    fn bootstrap_mints_independent_identities_per_store() {
        let mut store_a = SemanticStore::new();
        let mut store_b = SemanticStore::new();

        let definitions_a = Definitions::bootstrap(&mut store_a);
        let definitions_b = Definitions::bootstrap(&mut store_b);

        let Some(symbol_a) = store_a.types.get(definitions_a.int).reference_symbol() else {
            panic!("expected Int to lower to a Type::TypeRef");
        };
        let Some(symbol_b) = store_b.types.get(definitions_b.int).reference_symbol() else {
            panic!("expected Int to lower to a Type::TypeRef");
        };

        assert_eq!(
            store_a
                .names
                .resolve(store_a.symbols.get(symbol_a).name.text()),
            "Int"
        );
        assert_eq!(
            store_b
                .names
                .resolve(store_b.symbols.get(symbol_b).name.text()),
            "Int"
        );
    }
}
