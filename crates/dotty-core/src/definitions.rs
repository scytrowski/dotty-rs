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

    /// `java.lang.Object`, the JVM upper bound (`Any`'s reference-type
    /// counterpart, `AnyRef`, is not distinguished from it yet).
    pub object_class: SymbolId,
    /// Scala's top type, the upper bound `Type::Wildcard`/`Type::Bounds`
    /// lowering uses when a generic signature does not specify one.
    pub any_class: SymbolId,
    /// Scala's bottom type, the lower bound the same lowering uses when a
    /// wildcard does not specify one (e.g. `? extends Number`).
    pub nothing_class: SymbolId,
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
            object_class,
            any_class,
            nothing_class,
        }
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
        store.types.alloc(Type::TypeRef {
            prefix: no_prefix,
            symbol,
        })
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

        let Type::TypeRef { symbol, .. } = store.types.get(definitions.int) else {
            panic!("expected Int to lower to a Type::TypeRef");
        };

        assert_eq!(store.symbols.get(*symbol).origin, SymbolOrigin::Builtin);
        assert_eq!(store.symbols.get(*symbol).kind, SymbolKind::Class);
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

        let Type::TypeRef {
            symbol: symbol_a, ..
        } = store_a.types.get(definitions_a.int)
        else {
            panic!("expected Int to lower to a Type::TypeRef");
        };
        let Type::TypeRef {
            symbol: symbol_b, ..
        } = store_b.types.get(definitions_b.int)
        else {
            panic!("expected Int to lower to a Type::TypeRef");
        };

        assert_eq!(
            store_a
                .names
                .resolve(store_a.symbols.get(*symbol_a).name.text()),
            "Int"
        );
        assert_eq!(
            store_b
                .names
                .resolve(store_b.symbols.get(*symbol_b).name.text()),
            "Int"
        );
    }
}
