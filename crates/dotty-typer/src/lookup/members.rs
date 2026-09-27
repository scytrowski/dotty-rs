//! Nominal member candidate discovery over completed `ClassInfo` graphs.

use std::fmt;

use dotty_core::types::{ClassInfo, Type, TypeRefTarget};
use dotty_core::{Name, ScopeId, SemanticStore, SymbolId, SymbolInfo, SymbolKind, TypeId};

use crate::types::{SymbolInfoState, TypeNormalizeError, TypeNormalizer};
use crate::{SourceTyper, TyperError};

/// Maximum number of parent edges followed by one member lookup.
pub const MAX_MEMBER_LOOKUP_DEPTH: usize = 256;

/// A declaration found on the receiver or one of its nominal parents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberCandidate {
    /// The declaration selected from a class declaration scope.
    pub symbol: SymbolId,
    /// The class or trait that directly declares `symbol`.
    pub declaring_class: SymbolId,
    /// The type through which `declaring_class` was reached.
    pub receiver_view: TypeId,
    /// Number of parent edges from the normalized receiver to the declaration.
    pub inheritance_depth: u32,
}

/// A malformed graph, incomplete class declaration, or unsupported receiver
/// encountered while searching for member candidates.
#[derive(Debug)]
pub enum MemberLookupError {
    /// Receiver normalization stopped at a typed limit/error.
    TypeNormalization(TypeNormalizeError),
    /// The receiver does not designate a class, trait, or module class.
    ReceiverNotClassLike { ty: TypeId },
    /// The receiver's type shape is not supported by nominal member lookup.
    UnsupportedReceiverType { ty: TypeId },
    /// A parent type does not designate a class-like declaration.
    MalformedParentType { ty: TypeId },
    /// A referenced class symbol does not exist in the semantic store.
    UnknownClassSymbol { symbol: SymbolId },
    /// The class has not been completed (or its completion is in progress).
    ClassInfoUnavailable {
        symbol: SymbolId,
        state: SymbolInfoState,
    },
    /// A completed class symbol points to a type outside the type arena.
    InvalidClassInfoType { symbol: SymbolId, info: TypeId },
    /// A completed class symbol does not point to `Type::ClassInfo`.
    ClassInfoNotClassInfo { symbol: SymbolId, info: TypeId },
    /// The recorded class identity inside `ClassInfo` does not match its owner.
    MalformedClassInfoIdentity {
        symbol: SymbolId,
        recorded_class: SymbolId,
    },
    /// The class declaration scope does not exist in this semantic store.
    InvalidDeclarationScope { symbol: SymbolId, scope: ScopeId },
    /// A declaration scope contains a symbol ID absent from the store.
    UnknownMemberSymbol {
        declaring_class: SymbolId,
        symbol: SymbolId,
    },
    /// Source completion for a current-unit class failed before lookup.
    SourceClassCompletion { symbol: SymbolId, error: TyperError },
    /// A parent path revisits a class already on that path.
    InheritanceCycle { symbol: SymbolId },
    /// The parent graph exceeds [`MAX_MEMBER_LOOKUP_DEPTH`].
    TooDeep,
}

impl fmt::Display for MemberLookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for MemberLookupError {}

impl From<TypeNormalizeError> for MemberLookupError {
    fn from(error: TypeNormalizeError) -> Self {
        Self::TypeNormalization(error)
    }
}

impl SourceTyper<'_> {
    /// Finds every direct declaration matching `name` on `receiver`.
    ///
    /// This initial lookup step preserves every symbol from the exact scope
    /// bucket, in insertion order; it does not choose or merge overloads.
    pub fn lookup_members(
        &mut self,
        receiver: TypeId,
        name: Name,
    ) -> Result<Vec<MemberCandidate>, MemberLookupError> {
        let receiver_view = TypeNormalizer::new(self.store)
            .normalize_for_lookup(receiver)
            .map_err(MemberLookupError::TypeNormalization)?;
        let class = class_symbol_for_type(self.store, receiver_view, false)?;
        let info = self.class_info(class)?;
        self.direct_candidates(class, receiver_view, 0, name, &info)
    }

    fn class_info(&mut self, symbol: SymbolId) -> Result<ClassInfo, MemberLookupError> {
        if !self.store.symbols.contains(symbol) {
            return Err(MemberLookupError::UnknownClassSymbol { symbol });
        }
        let info_type = match *self.store.symbols.info(symbol) {
            SymbolInfo::Complete(info) => info,
            SymbolInfo::Missing => {
                return Err(MemberLookupError::ClassInfoUnavailable {
                    symbol,
                    state: SymbolInfoState::Missing,
                });
            }
            SymbolInfo::Deferred(_) => {
                return Err(MemberLookupError::ClassInfoUnavailable {
                    symbol,
                    state: SymbolInfoState::Deferred,
                });
            }
            SymbolInfo::Error => {
                return Err(MemberLookupError::ClassInfoUnavailable {
                    symbol,
                    state: SymbolInfoState::Error,
                });
            }
        };
        let Some(ty) = self.store.types.try_get(info_type) else {
            return Err(MemberLookupError::InvalidClassInfoType {
                symbol,
                info: info_type,
            });
        };
        let Type::ClassInfo(info) = ty else {
            return Err(MemberLookupError::ClassInfoNotClassInfo {
                symbol,
                info: info_type,
            });
        };
        if info.class != symbol {
            return Err(MemberLookupError::MalformedClassInfoIdentity {
                symbol,
                recorded_class: info.class,
            });
        }
        if !self.store.scopes.contains(info.declarations) {
            return Err(MemberLookupError::InvalidDeclarationScope {
                symbol,
                scope: info.declarations,
            });
        }
        Ok(info.clone())
    }

    fn direct_candidates(
        &self,
        class: SymbolId,
        receiver_view: TypeId,
        inheritance_depth: usize,
        name: Name,
        info: &ClassInfo,
    ) -> Result<Vec<MemberCandidate>, MemberLookupError> {
        let bucket = self.store.scopes.get(info.declarations).lookup_all(&name);
        bucket
            .iter()
            .map(|symbol| {
                if !self.store.symbols.contains(*symbol) {
                    return Err(MemberLookupError::UnknownMemberSymbol {
                        declaring_class: class,
                        symbol: *symbol,
                    });
                }
                Ok(MemberCandidate {
                    symbol: *symbol,
                    declaring_class: class,
                    receiver_view,
                    inheritance_depth: inheritance_depth as u32,
                })
            })
            .collect()
    }
}

fn class_symbol_for_type(
    store: &SemanticStore,
    ty: TypeId,
    is_parent: bool,
) -> Result<SymbolId, MemberLookupError> {
    let normalized = TypeNormalizer::new(store)
        .normalize_for_lookup(ty)
        .map_err(MemberLookupError::TypeNormalization)?;
    let mut current = normalized;
    let mut visited = Vec::new();

    loop {
        if visited.contains(&current) {
            return Err(if is_parent {
                MemberLookupError::MalformedParentType { ty }
            } else {
                MemberLookupError::UnsupportedReceiverType { ty }
            });
        }
        visited.push(current);
        let Some(ty_node) = store.types.try_get(current) else {
            return Err(MemberLookupError::TypeNormalization(
                if store.types.contains(current) {
                    TypeNormalizeError::UnfilledType { ty: current }
                } else {
                    TypeNormalizeError::InvalidType { ty: current }
                },
            ));
        };
        match ty_node {
            Type::ThisType { class } => {
                return validate_class_symbol(store, *class, ty, is_parent);
            }
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => return validate_class_symbol(store, *symbol, ty, is_parent),
            Type::Applied { tycon, .. } => {
                current = *tycon;
                let dealiased = TypeNormalizer::new(store)
                    .normalize_for_lookup(current)
                    .map_err(MemberLookupError::TypeNormalization)?;
                current = dealiased;
            }
            Type::TypeRef {
                target: TypeRefTarget::Name(_),
                ..
            } => {
                return Err(if is_parent {
                    MemberLookupError::MalformedParentType { ty }
                } else {
                    MemberLookupError::UnsupportedReceiverType { ty }
                });
            }
            _ => {
                return Err(if is_parent {
                    MemberLookupError::MalformedParentType { ty }
                } else {
                    MemberLookupError::UnsupportedReceiverType { ty }
                });
            }
        }
    }
}

fn validate_class_symbol(
    store: &SemanticStore,
    symbol: SymbolId,
    ty: TypeId,
    is_parent: bool,
) -> Result<SymbolId, MemberLookupError> {
    if !store.symbols.contains(symbol) {
        return Err(MemberLookupError::UnknownClassSymbol { symbol });
    }
    if matches!(
        store.symbols.get(symbol).kind,
        SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
    ) {
        return Ok(symbol);
    }
    Err(if is_parent {
        MemberLookupError::MalformedParentType { ty }
    } else {
        MemberLookupError::ReceiverNotClassLike { ty }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::ast::Untyped;
    use dotty_core::types::{Annotation, ClassInfo};
    use dotty_core::{
        AstArena, Definitions, Name, Namespace, Packages, SemanticStore, SourceId, Symbol,
        SymbolFlags, SymbolLinks, SymbolOrigin, Visibility,
    };
    use dotty_namer::SourceSemanticIndex;

    struct World {
        arena: AstArena<Untyped>,
        index: SourceSemanticIndex,
        store: SemanticStore,
        packages: Packages,
        definitions: Definitions,
        source: SourceId,
    }

    impl World {
        fn new() -> Self {
            let mut store = SemanticStore::new();
            let definitions = Definitions::bootstrap(&mut store);
            Self {
                arena: AstArena::new(),
                index: SourceSemanticIndex::new(),
                store,
                packages: Packages::new(),
                definitions,
                source: SourceId::from_index(0),
            }
        }

        fn symbol(
            &mut self,
            text: &str,
            namespace: Namespace,
            kind: SymbolKind,
            info: SymbolInfo,
        ) -> SymbolId {
            let name = Name::new(self.store.names.intern(text), namespace);
            self.store.symbols.alloc(Symbol {
                name,
                owner: None,
                kind,
                flags: SymbolFlags::EMPTY,
                visibility: Visibility::Public,
                info,
                origin: SymbolOrigin::Synthetic,
                annotations: Vec::new(),
                position: None,
                links: SymbolLinks::default(),
            })
        }

        fn class(&mut self, name: &str) -> SymbolId {
            self.symbol(
                name,
                Namespace::Type,
                SymbolKind::Class,
                SymbolInfo::Missing,
            )
        }

        fn scope(&mut self, class: SymbolId) -> dotty_core::ScopeId {
            self.store.scopes.alloc(dotty_core::Scope::new(Some(class)))
        }

        fn publish_class(
            &mut self,
            class: SymbolId,
            scope: dotty_core::ScopeId,
            parents: Vec<TypeId>,
        ) -> TypeId {
            let info = self.store.types.alloc(Type::ClassInfo(ClassInfo {
                prefix: self.definitions.no_prefix,
                class,
                parents,
                declarations: scope,
                self_type: None,
            }));
            self.store
                .symbols
                .set_info(class, SymbolInfo::Complete(info));
            info
        }

        fn class_ref(&mut self, class: SymbolId) -> TypeId {
            self.store.types.alloc(Type::TypeRef {
                prefix: self.definitions.no_prefix,
                target: TypeRefTarget::Symbol(class),
            })
        }

        fn lookup(
            &mut self,
            receiver: TypeId,
            name: Name,
        ) -> Result<Vec<MemberCandidate>, MemberLookupError> {
            let mut typer = SourceTyper::new(
                &self.arena,
                self.source,
                &self.index,
                &mut self.store,
                self.definitions,
                &self.packages,
            );
            typer.lookup_members(receiver, name)
        }
    }

    #[test]
    fn finds_a_direct_field_declaration() {
        let mut w = World::new();
        let class = w.class("C");
        let scope = w.scope(class);
        let field = w.symbol(
            "value",
            Namespace::Term,
            SymbolKind::Field,
            SymbolInfo::Missing,
        );
        let name = w.store.symbols.get(field).name;
        w.store.scopes.get_mut(scope).enter(name, field);
        w.publish_class(class, scope, Vec::new());
        let receiver = w.class_ref(class);

        assert_eq!(
            w.lookup(receiver, name).unwrap(),
            vec![MemberCandidate {
                symbol: field,
                declaring_class: class,
                receiver_view: receiver,
                inheritance_depth: 0,
            }]
        );
    }

    #[test]
    fn direct_method_lookup_preserves_the_complete_overload_bucket_order() {
        let mut w = World::new();
        let class = w.class("C");
        let scope = w.scope(class);
        let first = w.symbol(
            "f",
            Namespace::Term,
            SymbolKind::Method,
            SymbolInfo::Missing,
        );
        let second = w.symbol(
            "f",
            Namespace::Term,
            SymbolKind::Method,
            SymbolInfo::Missing,
        );
        let name = w.store.symbols.get(first).name;
        w.store.scopes.get_mut(scope).enter(name, first);
        w.store.scopes.get_mut(scope).enter(name, second);
        w.publish_class(class, scope, Vec::new());
        let receiver = w.class_ref(class);

        let candidates = w.lookup(receiver, name).unwrap();
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.symbol)
                .collect::<Vec<_>>(),
            vec![first, second]
        );
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.declaring_class == class
                    && candidate.receiver_view == receiver)
        );
    }

    #[test]
    fn direct_lookup_keeps_term_and_type_namespaces_distinct() {
        let mut w = World::new();
        let class = w.class("C");
        let scope = w.scope(class);
        let term = w.symbol("T", Namespace::Term, SymbolKind::Field, SymbolInfo::Missing);
        let ty = w.symbol(
            "T",
            Namespace::Type,
            SymbolKind::TypeAlias,
            SymbolInfo::Missing,
        );
        let term_name = w.store.symbols.get(term).name;
        let type_name = w.store.symbols.get(ty).name;
        w.store.scopes.get_mut(scope).enter(term_name, term);
        w.store.scopes.get_mut(scope).enter(type_name, ty);
        w.publish_class(class, scope, Vec::new());
        let receiver = w.class_ref(class);

        assert_eq!(w.lookup(receiver, term_name).unwrap()[0].symbol, term);
        assert_eq!(w.lookup(receiver, type_name).unwrap()[0].symbol, ty);
    }

    #[test]
    fn lookup_returns_an_empty_success_for_a_missing_member() {
        let mut w = World::new();
        let class = w.class("C");
        let scope = w.scope(class);
        w.publish_class(class, scope, Vec::new());
        let receiver = w.class_ref(class);
        let missing = Name::new(w.store.names.intern("missing"), Namespace::Term);

        assert!(w.lookup(receiver, missing).unwrap().is_empty());
    }

    #[test]
    fn applied_receiver_is_preserved_in_the_direct_candidate() {
        let mut w = World::new();
        let class = w.class("C");
        let scope = w.scope(class);
        let field = w.symbol(
            "value",
            Namespace::Term,
            SymbolKind::Field,
            SymbolInfo::Missing,
        );
        let name = w.store.symbols.get(field).name;
        w.store.scopes.get_mut(scope).enter(name, field);
        w.publish_class(class, scope, Vec::new());
        let tycon = w.class_ref(class);
        let argument = w.store.types.alloc(Type::NoType);
        let receiver = w.store.types.alloc(Type::Applied {
            tycon,
            args: vec![argument],
        });

        let candidates = w.lookup(receiver, name).unwrap();
        assert_eq!(candidates[0].receiver_view, receiver);
    }

    #[test]
    fn this_type_looks_up_its_class_declarations() {
        let mut w = World::new();
        let class = w.class("C");
        let scope = w.scope(class);
        let field = w.symbol(
            "value",
            Namespace::Term,
            SymbolKind::Field,
            SymbolInfo::Missing,
        );
        let name = w.store.symbols.get(field).name;
        w.store.scopes.get_mut(scope).enter(name, field);
        w.publish_class(class, scope, Vec::new());
        let receiver = w.store.types.alloc(Type::ThisType { class });

        let candidates = w.lookup(receiver, name).unwrap();
        assert_eq!(candidates[0].symbol, field);
        assert_eq!(candidates[0].receiver_view, receiver);
    }

    #[test]
    fn aliases_annotations_and_flexible_types_are_transparent_for_receivers() {
        let mut w = World::new();
        let class = w.class("C");
        let scope = w.scope(class);
        let field = w.symbol(
            "value",
            Namespace::Term,
            SymbolKind::Field,
            SymbolInfo::Missing,
        );
        let name = w.store.symbols.get(field).name;
        w.store.scopes.get_mut(scope).enter(name, field);
        w.publish_class(class, scope, Vec::new());
        let class_ref = w.class_ref(class);
        let bounds = w
            .store
            .types
            .alloc(Type::AliasingBounds { alias: class_ref });
        let alias = w.symbol(
            "Alias",
            Namespace::Type,
            SymbolKind::TypeAlias,
            SymbolInfo::Complete(bounds),
        );
        let alias_ref = w.class_ref(alias);
        let annotation = w.store.annotations.alloc(Annotation::new(class_ref, None));
        let annotated = w.store.types.alloc(Type::Annotated {
            underlying: alias_ref,
            annotation,
        });
        let receiver = w.store.types.alloc(Type::Flexible {
            underlying: annotated,
        });

        let candidates = w.lookup(receiver, name).unwrap();
        assert_eq!(candidates[0].symbol, field);
        assert_eq!(candidates[0].receiver_view, class_ref);
    }

    #[test]
    fn non_class_type_references_are_rejected_as_receivers() {
        let mut w = World::new();
        let value = w.symbol(
            "value",
            Namespace::Type,
            SymbolKind::Value,
            SymbolInfo::Missing,
        );
        let receiver = w.class_ref(value);
        let name = Name::new(w.store.names.intern("f"), Namespace::Term);

        assert!(matches!(
            w.lookup(receiver, name),
            Err(MemberLookupError::ReceiverNotClassLike { ty }) if ty == receiver
        ));
    }
}
