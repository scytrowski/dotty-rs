//! The one canonical set of builtin symbols/types every compilation session
//! shares.
//!
//! Descriptor/signature lowering (a JVM `I` or a source `Int`) needs a
//! stable `TypeId` for each JVM primitive plus `Object`/`Any`/`Nothing`, and
//! must not mint a fresh symbol every time one is encountered — two `Int`
//! occurrences in the same session have to resolve to the exact same
//! `TypeId`, or e.g. overload resolution and type equality would see them as
//! unrelated types. [`Definitions::bootstrap`] mints each one exactly once.

use crate::ids::{ScopeId, SymbolId, TypeId};
use crate::names::{Name, Namespace, TermName};
use crate::packages::Packages;
use crate::store::SemanticStore;
use crate::symbols::{
    Scope, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
};
use crate::types::{ClassInfo, MethodKind, MethodParam, MethodType, Type};

/// The canonical semantic identities for the bounded binary `Int` operators.
///
/// A backend can use these IDs to recognize typed calls without depending on
/// `dotty-typer` or inferring an intrinsic from an operator's spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntPrimitiveOperatorSymbols {
    pub plus: SymbolId,
    pub minus: SymbolId,
    pub multiply: SymbolId,
    pub divide: SymbolId,
    pub remainder: SymbolId,
    pub less_than: SymbolId,
    pub less_or_equal: SymbolId,
    pub greater_than: SymbolId,
    pub greater_or_equal: SymbolId,
}

impl IntPrimitiveOperatorSymbols {
    /// Returns the nine supported method identities in the audit order.
    pub const fn all(self) -> [SymbolId; 9] {
        [
            self.plus,
            self.minus,
            self.multiply,
            self.divide,
            self.remainder,
            self.less_than,
            self.less_or_equal,
            self.greater_than,
            self.greater_or_equal,
        ]
    }
}

/// A conflict or malformed class/member encountered while registering the
/// bounded canonical `Int` operators.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntPrimitiveOperatorRegistrationError {
    ClassInfoUnavailable {
        state: IntPrimitiveOperatorInfoState,
    },
    InvalidClassInfoType {
        info: TypeId,
    },
    ClassInfoNotClassInfo {
        info: TypeId,
    },
    MalformedClassInfoIdentity {
        recorded_class: SymbolId,
    },
    MalformedClassInfo {
        info: TypeId,
    },
    InvalidDeclarationScope {
        scope: ScopeId,
    },
    DeclarationScopeOwnerMismatch {
        actual: Option<SymbolId>,
    },
    UnknownMemberSymbol {
        symbol: SymbolId,
    },
    OperatorNameIsNotMethod {
        operator: &'static str,
        symbol: SymbolId,
    },
    OperatorMethodOwnerMismatch {
        operator: &'static str,
        symbol: SymbolId,
        owner: Option<SymbolId>,
    },
    OperatorMethodInfoUnavailable {
        operator: &'static str,
        symbol: SymbolId,
    },
    OperatorMethodInfoMalformed {
        operator: &'static str,
        symbol: SymbolId,
    },
    DuplicateOperatorSignature {
        operator: &'static str,
    },
    IncompatibleOperatorSignature {
        operator: &'static str,
        symbol: SymbolId,
    },
    IntTypeIsNotCanonical,
    BooleanTypeIsNotCanonical,
}

/// The completion state reported for the canonical `Int` class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntPrimitiveOperatorInfoState {
    Deferred,
    Error,
}

impl std::fmt::Display for IntPrimitiveOperatorRegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for IntPrimitiveOperatorRegistrationError {}

#[derive(Clone, Copy)]
struct IntOperatorSpec {
    spelling: &'static str,
    result: TypeId,
}

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

        let definitions = Self {
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
        };

        // `Int` is a compiler-owned class shell rather than a classfile
        // loaded from a classpath. Install its bounded source operator set
        // while the session's canonical definitions are created.
        definitions
            .ensure_int_primitive_operators(store)
            .expect("fresh canonical Int class should accept builtin operators");
        definitions
    }

    /// Ensures the supported primitive operators live in the existing
    /// canonical `Int` class declaration scope. Existing class information
    /// and unrelated overloads are preserved; malformed or conflicting
    /// operator entries are reported without partial registration.
    pub fn ensure_int_primitive_operators(
        &self,
        store: &mut SemanticStore,
    ) -> Result<IntPrimitiveOperatorSymbols, IntPrimitiveOperatorRegistrationError> {
        let int_class = store
            .types
            .get(self.int)
            .reference_symbol()
            .ok_or(IntPrimitiveOperatorRegistrationError::IntTypeIsNotCanonical)?;
        let boolean_class = store
            .types
            .get(self.boolean)
            .reference_symbol()
            .ok_or(IntPrimitiveOperatorRegistrationError::BooleanTypeIsNotCanonical)?;
        let _ = boolean_class;
        let specs = [
            IntOperatorSpec {
                spelling: "+",
                result: self.int,
            },
            IntOperatorSpec {
                spelling: "-",
                result: self.int,
            },
            IntOperatorSpec {
                spelling: "*",
                result: self.int,
            },
            IntOperatorSpec {
                spelling: "/",
                result: self.int,
            },
            IntOperatorSpec {
                spelling: "%",
                result: self.int,
            },
            IntOperatorSpec {
                spelling: "<",
                result: self.boolean,
            },
            IntOperatorSpec {
                spelling: "<=",
                result: self.boolean,
            },
            IntOperatorSpec {
                spelling: ">",
                result: self.boolean,
            },
            IntOperatorSpec {
                spelling: ">=",
                result: self.boolean,
            },
        ];

        let info = store.symbols.get(int_class).info;
        let (scope, needs_class_info) = match info {
            SymbolInfo::Missing => (None, true),
            SymbolInfo::Deferred(_) => {
                return Err(
                    IntPrimitiveOperatorRegistrationError::ClassInfoUnavailable {
                        state: IntPrimitiveOperatorInfoState::Deferred,
                    },
                );
            }
            SymbolInfo::Error => {
                return Err(
                    IntPrimitiveOperatorRegistrationError::ClassInfoUnavailable {
                        state: IntPrimitiveOperatorInfoState::Error,
                    },
                );
            }
            SymbolInfo::Complete(info_id) => {
                let Some(Type::ClassInfo(class_info)) = store.types.try_get(info_id) else {
                    return Err(if store.types.contains(info_id) {
                        IntPrimitiveOperatorRegistrationError::ClassInfoNotClassInfo {
                            info: info_id,
                        }
                    } else {
                        IntPrimitiveOperatorRegistrationError::InvalidClassInfoType {
                            info: info_id,
                        }
                    });
                };
                if class_info.class != int_class {
                    return Err(
                        IntPrimitiveOperatorRegistrationError::MalformedClassInfoIdentity {
                            recorded_class: class_info.class,
                        },
                    );
                }
                if store.types.try_get(class_info.prefix).is_none()
                    || class_info
                        .parents
                        .iter()
                        .any(|parent| store.types.try_get(*parent).is_none())
                    || class_info
                        .self_type
                        .is_some_and(|self_type| store.types.try_get(self_type).is_none())
                {
                    return Err(IntPrimitiveOperatorRegistrationError::MalformedClassInfo {
                        info: info_id,
                    });
                }
                if !store.scopes.contains(class_info.declarations) {
                    return Err(
                        IntPrimitiveOperatorRegistrationError::InvalidDeclarationScope {
                            scope: class_info.declarations,
                        },
                    );
                }
                let declaration_scope = store.scopes.get(class_info.declarations);
                if declaration_scope.owner != Some(int_class) {
                    return Err(
                        IntPrimitiveOperatorRegistrationError::DeclarationScopeOwnerMismatch {
                            actual: declaration_scope.owner,
                        },
                    );
                }
                (Some(class_info.declarations), false)
            }
        };

        let mut found = [None; 9];
        if let Some(scope) = scope {
            for (index, spec) in specs.iter().enumerate() {
                let name = Name::new(store.names.intern(spec.spelling), Namespace::Term);
                let mut matching_signature = None;
                for &symbol in store.scopes.get(scope).lookup_all(&name) {
                    if !store.symbols.contains(symbol) {
                        return Err(IntPrimitiveOperatorRegistrationError::UnknownMemberSymbol {
                            symbol,
                        });
                    }
                    let candidate = store.symbols.get(symbol);
                    if candidate.kind != SymbolKind::Method {
                        return Err(
                            IntPrimitiveOperatorRegistrationError::OperatorNameIsNotMethod {
                                operator: spec.spelling,
                                symbol,
                            },
                        );
                    }
                    if candidate.owner != Some(int_class) {
                        return Err(
                            IntPrimitiveOperatorRegistrationError::OperatorMethodOwnerMismatch {
                                operator: spec.spelling,
                                symbol,
                                owner: candidate.owner,
                            },
                        );
                    }
                    let SymbolInfo::Complete(method_info) = candidate.info else {
                        return Err(
                            IntPrimitiveOperatorRegistrationError::OperatorMethodInfoUnavailable {
                                operator: spec.spelling,
                                symbol,
                            },
                        );
                    };
                    let Some(Type::Method(method)) = store.types.try_get(method_info) else {
                        return Err(
                            IntPrimitiveOperatorRegistrationError::OperatorMethodInfoMalformed {
                                operator: spec.spelling,
                                symbol,
                            },
                        );
                    };
                    let same_parameter_signature =
                        method.params.len() == 1 && method.params[0].ty == self.int;
                    if same_parameter_signature {
                        if method.kind != MethodKind::Plain
                            || method.params[0].erased
                            || method.params[0].varargs
                            || method.result != spec.result
                        {
                            return Err(IntPrimitiveOperatorRegistrationError::IncompatibleOperatorSignature {
                                operator: spec.spelling,
                                symbol,
                            });
                        }
                        if matching_signature.replace(symbol).is_some() {
                            return Err(
                                IntPrimitiveOperatorRegistrationError::DuplicateOperatorSignature {
                                    operator: spec.spelling,
                                },
                            );
                        }
                    }
                }
                found[index] = matching_signature;
            }
        }

        let scope = if let Some(scope) = scope {
            scope
        } else {
            store.scopes.alloc(Scope::new(Some(int_class)))
        };
        let mut ids = [None; 9];
        for (index, spec) in specs.into_iter().enumerate() {
            if let Some(symbol) = found[index] {
                ids[index] = Some(symbol);
                continue;
            }
            let name_id = store.names.intern(spec.spelling);
            let name = Name::new(name_id, Namespace::Term);
            let param_name = TermName::new(store.names.intern("x"));
            let method_info = store.types.alloc(Type::Method(MethodType {
                params: vec![MethodParam {
                    name: param_name,
                    ty: self.int,
                    erased: false,
                    varargs: false,
                }],
                result: spec.result,
                kind: MethodKind::Plain,
            }));
            let symbol = store.symbols.alloc(Symbol {
                name,
                owner: Some(int_class),
                kind: SymbolKind::Method,
                flags: SymbolFlags::EMPTY,
                visibility: Visibility::Public,
                info: SymbolInfo::Complete(method_info),
                origin: SymbolOrigin::Builtin,
                annotations: Vec::new(),
                position: None,
                links: SymbolLinks::default(),
            });
            store.scopes.get_mut(scope).enter(name, symbol);
            ids[index] = Some(symbol);
        }
        let ids = ids.map(Option::unwrap);

        if needs_class_info {
            let info = store.types.alloc(Type::ClassInfo(ClassInfo {
                prefix: self.no_prefix,
                class: int_class,
                parents: Vec::new(),
                declarations: scope,
                self_type: None,
            }));
            store.symbols.get_mut(int_class).info = SymbolInfo::Complete(info);
        }
        Ok(IntPrimitiveOperatorSymbols {
            plus: ids[0],
            minus: ids[1],
            multiply: ids[2],
            divide: ids[3],
            remainder: ids[4],
            less_than: ids[5],
            less_or_equal: ids[6],
            greater_than: ids[7],
            greater_or_equal: ids[8],
        })
    }

    /// Reads the registered builtin operator identities without changing
    /// semantic state. Returns `None` until registration succeeds.
    pub fn int_primitive_operator_symbols(
        &self,
        store: &SemanticStore,
    ) -> Option<IntPrimitiveOperatorSymbols> {
        let int_class = store.types.get(self.int).reference_symbol()?;
        let SymbolInfo::Complete(info_id) = store.symbols.get(int_class).info else {
            return None;
        };
        let Type::ClassInfo(info) = store.types.try_get(info_id)? else {
            return None;
        };
        if info.class != int_class
            || !store.scopes.contains(info.declarations)
            || store.scopes.get(info.declarations).owner != Some(int_class)
        {
            return None;
        }
        let specs = [
            IntOperatorSpec {
                spelling: "+",
                result: self.int,
            },
            IntOperatorSpec {
                spelling: "-",
                result: self.int,
            },
            IntOperatorSpec {
                spelling: "*",
                result: self.int,
            },
            IntOperatorSpec {
                spelling: "/",
                result: self.int,
            },
            IntOperatorSpec {
                spelling: "%",
                result: self.int,
            },
            IntOperatorSpec {
                spelling: "<",
                result: self.boolean,
            },
            IntOperatorSpec {
                spelling: "<=",
                result: self.boolean,
            },
            IntOperatorSpec {
                spelling: ">",
                result: self.boolean,
            },
            IntOperatorSpec {
                spelling: ">=",
                result: self.boolean,
            },
        ];
        let mut ids = [None; 9];
        for (index, spec) in specs.iter().enumerate() {
            let name = Name::new(store.names.get(spec.spelling)?, Namespace::Term);
            for &symbol in store.scopes.get(info.declarations).lookup_all(&name) {
                if !store.symbols.contains(symbol) {
                    return None;
                }
                let member = store.symbols.get(symbol);
                let SymbolInfo::Complete(member_info) = member.info else {
                    continue;
                };
                let Some(Type::Method(method)) = store.types.try_get(member_info) else {
                    continue;
                };
                if member.kind == SymbolKind::Method
                    && member.owner == Some(int_class)
                    && method.kind == MethodKind::Plain
                    && method.params.len() == 1
                    && method.params[0].ty == self.int
                    && !method.params[0].erased
                    && !method.params[0].varargs
                    && method.result == spec.result
                    && ids[index].replace(symbol).is_some()
                {
                    return None;
                }
            }
            ids[index]?;
        }
        Some(IntPrimitiveOperatorSymbols {
            plus: ids[0]?,
            minus: ids[1]?,
            multiply: ids[2]?,
            divide: ids[3]?,
            remainder: ids[4]?,
            less_than: ids[5]?,
            less_or_equal: ids[6]?,
            greater_than: ids[7]?,
            greater_or_equal: ids[8]?,
        })
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

    #[test]
    fn int_operator_registration_is_complete_canonical_and_idempotent() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let int_symbol = store.types.get(definitions.int).reference_symbol().unwrap();
        let boolean = definitions.boolean;
        let registered = definitions.int_primitive_operator_symbols(&store).unwrap();
        assert_eq!(
            definitions
                .ensure_int_primitive_operators(&mut store)
                .unwrap(),
            registered
        );
        assert_eq!(registered.all().len(), 9);

        for (symbol, spelling, result) in [
            (registered.plus, "+", definitions.int),
            (registered.minus, "-", definitions.int),
            (registered.multiply, "*", definitions.int),
            (registered.divide, "/", definitions.int),
            (registered.remainder, "%", definitions.int),
            (registered.less_than, "<", boolean),
            (registered.less_or_equal, "<=", boolean),
            (registered.greater_than, ">", boolean),
            (registered.greater_or_equal, ">=", boolean),
        ] {
            let method = store.symbols.get(symbol);
            assert_eq!(method.kind, SymbolKind::Method);
            assert_eq!(method.origin, SymbolOrigin::Builtin);
            assert_eq!(method.owner, Some(int_symbol));
            assert_eq!(store.names.resolve(method.name.text()), spelling);
            let SymbolInfo::Complete(info) = method.info else {
                panic!("registered operator should have completed info");
            };
            let Type::Method(method_type) = store.types.get(info) else {
                panic!("registered operator should be an ordinary method");
            };
            assert_eq!(method_type.kind, MethodKind::Plain);
            assert_eq!(method_type.params.len(), 1);
            assert_eq!(method_type.params[0].ty, definitions.int);
            assert_eq!(method_type.result, result);
        }
    }

    #[test]
    fn malformed_int_class_info_is_reported_without_replacing_it() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let int_symbol = store.types.get(definitions.int).reference_symbol().unwrap();
        let original = store.symbols.get(int_symbol).info;
        store.symbols.get_mut(int_symbol).info = SymbolInfo::Error;

        assert_eq!(
            definitions.ensure_int_primitive_operators(&mut store),
            Err(
                IntPrimitiveOperatorRegistrationError::ClassInfoUnavailable {
                    state: IntPrimitiveOperatorInfoState::Error,
                }
            )
        );
        assert_eq!(store.symbols.get(int_symbol).info, SymbolInfo::Error);
        assert_ne!(original, SymbolInfo::Error);
    }

    #[test]
    fn missing_int_operator_is_registered_again_in_the_same_scope() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let before = definitions.int_primitive_operator_symbols(&store).unwrap();
        let int_symbol = store.types.get(definitions.int).reference_symbol().unwrap();
        let SymbolInfo::Complete(info) = store.symbols.get(int_symbol).info else {
            unreachable!();
        };
        let Type::ClassInfo(info) = store.types.get(info) else {
            unreachable!();
        };
        let scope = info.declarations;
        store.scopes.get_mut(scope).remove(before.plus);

        assert!(definitions.int_primitive_operator_symbols(&store).is_none());
        let after = definitions
            .ensure_int_primitive_operators(&mut store)
            .unwrap();
        assert_ne!(after.plus, before.plus);
        assert_eq!(after.minus, before.minus);
        assert_eq!(
            store.scopes.get(scope).owner,
            Some(int_symbol),
            "registration should preserve the existing declaration scope"
        );
    }

    #[test]
    fn duplicate_int_operator_signatures_are_rejected_without_mutation() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let int_symbol = store.types.get(definitions.int).reference_symbol().unwrap();
        let SymbolInfo::Complete(info) = store.symbols.get(int_symbol).info else {
            unreachable!();
        };
        let Type::ClassInfo(info) = store.types.get(info) else {
            unreachable!();
        };
        let scope = info.declarations;
        let name = Name::new(store.names.get("+").unwrap(), Namespace::Term);
        let plus = definitions
            .int_primitive_operator_symbols(&store)
            .unwrap()
            .plus;
        store.scopes.get_mut(scope).enter(name, plus);

        assert_eq!(
            definitions.ensure_int_primitive_operators(&mut store),
            Err(
                IntPrimitiveOperatorRegistrationError::DuplicateOperatorSignature { operator: "+" }
            )
        );
        assert_eq!(store.scopes.get(scope).lookup_all(&name), &[plus, plus]);
    }

    #[test]
    fn invalid_int_class_info_type_is_rejected() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let int_symbol = store.types.get(definitions.int).reference_symbol().unwrap();
        let invalid = TypeId::new(u32::MAX);
        store.symbols.get_mut(int_symbol).info = SymbolInfo::Complete(invalid);

        assert_eq!(
            definitions.ensure_int_primitive_operators(&mut store),
            Err(IntPrimitiveOperatorRegistrationError::InvalidClassInfoType { info: invalid })
        );
        assert_eq!(
            store.symbols.get(int_symbol).info,
            SymbolInfo::Complete(invalid)
        );
    }

    #[test]
    fn malformed_int_class_info_references_are_rejected() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let int_symbol = store.types.get(definitions.int).reference_symbol().unwrap();
        let SymbolInfo::Complete(info) = store.symbols.get(int_symbol).info else {
            unreachable!();
        };
        let Type::ClassInfo(mut class_info) = store.types.get(info).clone() else {
            unreachable!();
        };
        class_info.prefix = TypeId::new(u32::MAX);
        let malformed = store.types.alloc(Type::ClassInfo(class_info));
        store.symbols.get_mut(int_symbol).info = SymbolInfo::Complete(malformed);

        assert_eq!(
            definitions.ensure_int_primitive_operators(&mut store),
            Err(IntPrimitiveOperatorRegistrationError::MalformedClassInfo { info: malformed })
        );
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
