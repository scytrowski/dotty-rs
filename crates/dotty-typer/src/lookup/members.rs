//! Nominal member candidate discovery over completed `ClassInfo` graphs.

use std::collections::{HashMap, HashSet, VecDeque};
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
    /// Source completion for a current-unit receiver alias failed before lookup.
    SourceAliasCompletion { symbol: SymbolId, error: TyperError },
    /// A source class's declared parent view could not be instantiated.
    ParentTypeAdaptation { symbol: SymbolId, error: TyperError },
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
    /// Finds every matching declaration on `receiver` or its nominal parents.
    ///
    /// Direct declaration buckets hide inherited buckets on the same branch.
    /// Every bucket preserves its scope insertion order; candidates are ordered
    /// by inheritance depth and then by parent traversal order. Exact repeated
    /// symbols reached through a diamond are returned once. This does not
    /// choose or merge overloads. Missing classes from this source unit are
    /// completed through `complete_symbol`; external symbols are never loaded
    /// from a classpath here.
    pub fn lookup_members(
        &mut self,
        receiver: TypeId,
        name: Name,
    ) -> Result<Vec<MemberCandidate>, MemberLookupError> {
        let store_checkpoint = self.store.checkpoint();
        let resolver_checkpoint = self.resolver.checkpoint();
        let checkpoint = self.type_index.checkpoint();
        let mut journal = Vec::new();
        let result = self.lookup_members_inner(receiver, name, &mut journal, false);
        if result.is_err() {
            for (symbol, previous) in journal.into_iter().rev() {
                if self.store.symbols.contains(symbol) {
                    self.store.symbols.set_info(symbol, previous);
                }
            }
            self.resolver.rollback_to(self.store, resolver_checkpoint);
            self.store.rollback_to(store_checkpoint);
            self.type_index.restore(checkpoint);
        }
        result
    }

    pub(in crate::typer) fn lookup_members_journaled(
        &mut self,
        receiver: TypeId,
        name: Name,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Vec<MemberCandidate>, MemberLookupError> {
        self.lookup_members_inner(receiver, name, journal, false)
    }

    /// Finds matching declarations on a receiver and its nominal parents,
    /// including inherited buckets hidden by direct declarations on the path.
    /// Overload application uses this view before applicability filtering and
    /// removes only signatures overridden by a more-derived declaration.
    pub(in crate::typer) fn lookup_overload_members_journaled(
        &mut self,
        receiver: TypeId,
        name: Name,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Vec<MemberCandidate>, MemberLookupError> {
        self.lookup_members_inner(receiver, name, journal, true)
    }

    fn lookup_members_inner(
        &mut self,
        receiver: TypeId,
        name: Name,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
        include_hidden_inherited: bool,
    ) -> Result<Vec<MemberCandidate>, MemberLookupError> {
        let receiver_view = self.normalize_member_receiver(receiver, journal)?;
        let class = class_symbol_for_type(self.store, receiver_view, false)?;
        let mut queue = VecDeque::from([PendingClass {
            symbol: class,
            receiver_view,
            depth: 0,
        }]);
        let mut visited = HashSet::from([class]);
        let mut processed = Vec::new();
        let mut edges = HashMap::<SymbolId, Vec<SymbolId>>::new();
        let mut candidates = Vec::new();

        while let Some(pending) = queue.pop_front() {
            let info = match self.class_info(pending.symbol, journal) {
                Ok(info) => info,
                Err(MemberLookupError::ClassInfoUnavailable { .. })
                    if pending.symbol == self.definitions.object_class =>
                {
                    // The bootstrapped Object symbol is the terminal fallback
                    // parent in this typer's model and has no classpath metadata.
                    // Its absent class info means it contributes no modeled
                    // members, whether or not a nearer declaration matched.
                    processed.push(pending.symbol);
                    edges.insert(pending.symbol, Vec::new());
                    continue;
                }
                Err(error) => return Err(error),
            };
            let direct = self.direct_candidates(
                pending.symbol,
                pending.receiver_view,
                pending.depth,
                name,
                &info,
            )?;
            processed.push(pending.symbol);
            if !direct.is_empty() {
                candidates.extend(direct);
                if !include_hidden_inherited {
                    // Ordinary member selection treats a direct bucket as a
                    // complete override of inherited declarations.
                    edges.insert(pending.symbol, Vec::new());
                    continue;
                }
            }

            let mut parent_symbols = Vec::with_capacity(info.parents.len());
            for parent_view in info.parents {
                let parent_view = self
                    .adapt_parent_view(pending.symbol, pending.receiver_view, parent_view)
                    .map_err(|error| MemberLookupError::ParentTypeAdaptation {
                        symbol: pending.symbol,
                        error,
                    })?;
                let parent = class_symbol_for_type(self.store, parent_view, true)?;
                parent_symbols.push(parent);
                if visited.insert(parent) {
                    if pending.depth == MAX_MEMBER_LOOKUP_DEPTH {
                        return Err(MemberLookupError::TooDeep);
                    }
                    queue.push_back(PendingClass {
                        symbol: parent,
                        receiver_view: parent_view,
                        depth: pending.depth + 1,
                    });
                }
            }
            edges.insert(pending.symbol, parent_symbols);
        }

        if let Some(cycle) = find_inheritance_cycle(class, &processed, &edges) {
            return Err(MemberLookupError::InheritanceCycle { symbol: cycle });
        }

        let mut seen_candidates = HashSet::new();
        candidates.retain(|candidate| seen_candidates.insert(candidate.symbol));
        Ok(candidates)
    }

    fn normalize_member_receiver(
        &mut self,
        receiver: TypeId,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, MemberLookupError> {
        for _ in 0..crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            match TypeNormalizer::new(self.store).normalize_for_lookup(receiver) {
                Ok(normalized) => return Ok(normalized),
                Err(TypeNormalizeError::AliasInfoIncomplete {
                    symbol,
                    state: SymbolInfoState::Missing,
                }) if self.index.definition_of(symbol).is_some() => {
                    self.complete_symbol_inner(symbol, journal)
                        .map_err(|error| MemberLookupError::SourceAliasCompletion {
                            symbol,
                            error,
                        })?;
                }
                Err(error) => return Err(MemberLookupError::TypeNormalization(error)),
            }
        }
        Err(MemberLookupError::TypeNormalization(
            TypeNormalizeError::TooDeep,
        ))
    }

    pub(in crate::typer) fn class_info(
        &mut self,
        symbol: SymbolId,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<ClassInfo, MemberLookupError> {
        if !self.store.symbols.contains(symbol) {
            return Err(MemberLookupError::UnknownClassSymbol { symbol });
        }
        if matches!(*self.store.symbols.info(symbol), SymbolInfo::Missing)
            && self.is_current_source_symbol(symbol)
        {
            self.complete_symbol_inner(symbol, journal)
                .map_err(|error| MemberLookupError::SourceClassCompletion { symbol, error })?;
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

    pub(crate) fn is_current_source_symbol(&self, symbol: SymbolId) -> bool {
        self.index
            .definition_of(symbol)
            .is_some_and(|definition| match definition {
                dotty_core::SourceDefinition::Canonical { source, .. }
                | dotty_core::SourceDefinition::Derived { source, .. } => source == self.source,
            })
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

#[derive(Clone, Copy)]
struct PendingClass {
    symbol: SymbolId,
    receiver_view: TypeId,
    depth: usize,
}

pub(in crate::typer) fn class_symbol_for_type(
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
        if visited.len() > MAX_MEMBER_LOOKUP_DEPTH {
            return Err(MemberLookupError::TooDeep);
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
                // Keep the application intact while extracting its class
                // owner. In particular, do not open an applied type alias
                // after dropping the arguments needed for substitution.
                TypeNormalizer::new(store)
                    .dealias_top(current)
                    .map_err(MemberLookupError::TypeNormalization)?;
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

fn find_inheritance_cycle(
    root: SymbolId,
    processed: &[SymbolId],
    edges: &HashMap<SymbolId, Vec<SymbolId>>,
) -> Option<SymbolId> {
    let mut indegrees: HashMap<SymbolId, usize> = processed
        .iter()
        .copied()
        .map(|symbol| (symbol, 0))
        .collect();
    for symbol in processed {
        if let Some(parents) = edges.get(symbol) {
            for parent in parents {
                *indegrees.entry(*parent).or_default() += 1;
            }
        }
    }

    let mut ready: VecDeque<_> = processed
        .iter()
        .copied()
        .filter(|symbol| indegrees.get(symbol) == Some(&0))
        .collect();
    let mut removed = HashSet::new();
    while let Some(symbol) = ready.pop_front() {
        if !removed.insert(symbol) {
            continue;
        }
        if let Some(parents) = edges.get(&symbol) {
            for parent in parents {
                let Some(indegree) = indegrees.get_mut(parent) else {
                    continue;
                };
                *indegree -= 1;
                if *indegree == 0 {
                    ready.push_back(*parent);
                }
            }
        }
    }

    processed
        .iter()
        .copied()
        .find(|symbol| !removed.contains(symbol))
        .or_else(|| {
            // The root is included in `processed`; this fallback keeps the
            // parameter useful if a future caller supplies an empty list.
            (indegrees.get(&root).copied().unwrap_or(0) > 0).then_some(root)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::SourceSemanticIndex;
    use dotty_core::ast::Untyped;
    use dotty_core::types::{Annotation, ClassInfo};
    use dotty_core::{
        AstArena, Definitions, Name, Namespace, Packages, SemanticStore, SourceId, Symbol,
        SymbolFlags, SymbolLinks, SymbolOrigin, Visibility,
    };

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
    fn generic_parent_view_is_preserved_without_substitution() {
        let mut w = World::new();
        let base = w.class("Base");
        let base_scope = w.scope(base);
        let field = w.symbol(
            "value",
            Namespace::Term,
            SymbolKind::Field,
            SymbolInfo::Missing,
        );
        let name = w.store.symbols.get(field).name;
        w.store.scopes.get_mut(base_scope).enter(name, field);
        w.publish_class(base, base_scope, Vec::new());

        let child = w.class("Child");
        let child_scope = w.scope(child);
        let tycon = w.class_ref(base);
        let argument = w.store.types.alloc(Type::NoType);
        let applied_parent = w.store.types.alloc(Type::Applied {
            tycon,
            args: vec![argument],
        });
        w.publish_class(child, child_scope, vec![applied_parent]);
        let receiver = w.class_ref(child);

        let candidates = w.lookup(receiver, name).unwrap();
        assert_eq!(candidates[0].receiver_view, applied_parent);
        assert_eq!(candidates[0].inheritance_depth, 1);
    }

    #[test]
    fn transparent_parent_wrappers_keep_the_original_receiver_view() {
        let mut w = World::new();
        let base = w.class("Base");
        let base_scope = w.scope(base);
        let field = w.symbol(
            "value",
            Namespace::Term,
            SymbolKind::Field,
            SymbolInfo::Missing,
        );
        let name = w.store.symbols.get(field).name;
        w.store.scopes.get_mut(base_scope).enter(name, field);
        w.publish_class(base, base_scope, Vec::new());

        let child = w.class("Child");
        let child_scope = w.scope(child);
        let base_view = w.class_ref(base);
        let flexible_parent = w.store.types.alloc(Type::Flexible {
            underlying: base_view,
        });
        w.publish_class(child, child_scope, vec![flexible_parent]);
        let receiver = w.class_ref(child);

        let candidates = w.lookup(receiver, name).unwrap();
        assert_eq!(candidates[0].receiver_view, flexible_parent);
        assert_eq!(candidates[0].declaring_class, base);
    }

    #[test]
    fn applied_alias_parent_is_not_opened_without_substitution() {
        let mut w = World::new();
        let class = w.class("Base");
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
        let argument = w.store.types.alloc(Type::NoType);
        let applied_alias = w.store.types.alloc(Type::Applied {
            tycon: alias_ref,
            args: vec![argument],
        });
        let name = Name::new(w.store.names.intern("member"), Namespace::Term);

        assert!(matches!(
            w.lookup(applied_alias, name),
            Err(MemberLookupError::TypeNormalization(
                TypeNormalizeError::AliasRequiresSubstitution { symbol }
            )) if symbol == alias
        ));
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
    fn finds_a_member_in_the_direct_parent() {
        let mut w = World::new();
        let base = w.class("Base");
        let base_scope = w.scope(base);
        let field = w.symbol(
            "value",
            Namespace::Term,
            SymbolKind::Field,
            SymbolInfo::Missing,
        );
        let name = w.store.symbols.get(field).name;
        w.store.scopes.get_mut(base_scope).enter(name, field);
        w.publish_class(base, base_scope, Vec::new());

        let child = w.class("Child");
        let child_scope = w.scope(child);
        let base_view = w.class_ref(base);
        w.publish_class(child, child_scope, vec![base_view]);
        let receiver = w.class_ref(child);

        assert_eq!(
            w.lookup(receiver, name).unwrap(),
            vec![MemberCandidate {
                symbol: field,
                declaring_class: base,
                receiver_view: base_view,
                inheritance_depth: 1,
            }]
        );
    }

    #[test]
    fn finds_a_member_multiple_parent_levels_up() {
        let mut w = World::new();
        let root = w.class("Root");
        let root_scope = w.scope(root);
        let field = w.symbol(
            "value",
            Namespace::Term,
            SymbolKind::Field,
            SymbolInfo::Missing,
        );
        let name = w.store.symbols.get(field).name;
        w.store.scopes.get_mut(root_scope).enter(name, field);
        w.publish_class(root, root_scope, Vec::new());

        let parent = w.class("Parent");
        let parent_scope = w.scope(parent);
        let root_view = w.class_ref(root);
        w.publish_class(parent, parent_scope, vec![root_view]);
        let child = w.class("Child");
        let child_scope = w.scope(child);
        let parent_view = w.class_ref(parent);
        w.publish_class(child, child_scope, vec![parent_view]);
        let receiver = w.class_ref(child);

        let candidates = w.lookup(receiver, name).unwrap();
        assert_eq!(candidates[0].declaring_class, root);
        assert_eq!(candidates[0].receiver_view, root_view);
        assert_eq!(candidates[0].inheritance_depth, 2);
    }

    #[test]
    fn direct_member_hides_the_entire_inherited_bucket() {
        let mut w = World::new();
        let base = w.class("Base");
        let base_scope = w.scope(base);
        let inherited = w.symbol(
            "value",
            Namespace::Term,
            SymbolKind::Field,
            SymbolInfo::Missing,
        );
        let name = w.store.symbols.get(inherited).name;
        w.store.scopes.get_mut(base_scope).enter(name, inherited);
        w.publish_class(base, base_scope, Vec::new());

        let child = w.class("Child");
        let child_scope = w.scope(child);
        let direct = w.symbol(
            "value",
            Namespace::Term,
            SymbolKind::Field,
            SymbolInfo::Missing,
        );
        w.store.scopes.get_mut(child_scope).enter(name, direct);
        let base_view = w.class_ref(base);
        w.publish_class(child, child_scope, vec![base_view]);
        let receiver = w.class_ref(child);

        let candidates = w.lookup(receiver, name).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].symbol, direct);
        assert_eq!(candidates[0].declaring_class, child);
        assert_eq!(candidates[0].inheritance_depth, 0);
    }

    #[test]
    fn distinct_parent_declarations_are_returned_in_parent_order() {
        let mut w = World::new();
        let first_parent = w.class("First");
        let first_scope = w.scope(first_parent);
        let first_member = w.symbol(
            "value",
            Namespace::Term,
            SymbolKind::Field,
            SymbolInfo::Missing,
        );
        let name = w.store.symbols.get(first_member).name;
        w.store
            .scopes
            .get_mut(first_scope)
            .enter(name, first_member);
        w.publish_class(first_parent, first_scope, Vec::new());

        let second_parent = w.class("Second");
        let second_scope = w.scope(second_parent);
        let second_member = w.symbol(
            "value",
            Namespace::Term,
            SymbolKind::Field,
            SymbolInfo::Missing,
        );
        w.store
            .scopes
            .get_mut(second_scope)
            .enter(name, second_member);
        w.publish_class(second_parent, second_scope, Vec::new());

        let child = w.class("Child");
        let child_scope = w.scope(child);
        let first_view = w.class_ref(first_parent);
        let second_view = w.class_ref(second_parent);
        w.publish_class(child, child_scope, vec![first_view, second_view]);
        let receiver = w.class_ref(child);

        let candidates = w.lookup(receiver, name).unwrap();
        assert_eq!(
            candidates.iter().map(|c| c.symbol).collect::<Vec<_>>(),
            vec![first_member, second_member]
        );
        assert_eq!(
            candidates
                .iter()
                .map(|c| c.receiver_view)
                .collect::<Vec<_>>(),
            vec![first_view, second_view]
        );
        assert!(candidates.iter().all(|c| c.inheritance_depth == 1));
    }

    #[test]
    fn inherited_candidates_are_ordered_by_depth_before_parent_order() {
        let mut w = World::new();
        let deep = w.class("Deep");
        let deep_scope = w.scope(deep);
        let deep_member = w.symbol(
            "value",
            Namespace::Term,
            SymbolKind::Field,
            SymbolInfo::Missing,
        );
        let name = w.store.symbols.get(deep_member).name;
        w.store.scopes.get_mut(deep_scope).enter(name, deep_member);
        w.publish_class(deep, deep_scope, Vec::new());

        let first = w.class("First");
        let first_scope = w.scope(first);
        let deep_view = w.class_ref(deep);
        w.publish_class(first, first_scope, vec![deep_view]);
        let second = w.class("Second");
        let second_scope = w.scope(second);
        let second_member = w.symbol(
            "value",
            Namespace::Term,
            SymbolKind::Field,
            SymbolInfo::Missing,
        );
        w.store
            .scopes
            .get_mut(second_scope)
            .enter(name, second_member);
        w.publish_class(second, second_scope, Vec::new());

        let child = w.class("Child");
        let child_scope = w.scope(child);
        let first_view = w.class_ref(first);
        let second_view = w.class_ref(second);
        w.publish_class(child, child_scope, vec![first_view, second_view]);
        let receiver = w.class_ref(child);

        let candidates = w.lookup(receiver, name).unwrap();
        assert_eq!(
            candidates.iter().map(|c| c.symbol).collect::<Vec<_>>(),
            vec![second_member, deep_member]
        );
        assert_eq!(
            candidates
                .iter()
                .map(|c| c.inheritance_depth)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn diamond_inheritance_deduplicates_a_shared_declaration() {
        let mut w = World::new();
        let root = w.class("Root");
        let root_scope = w.scope(root);
        let member = w.symbol(
            "value",
            Namespace::Term,
            SymbolKind::Field,
            SymbolInfo::Missing,
        );
        let name = w.store.symbols.get(member).name;
        w.store.scopes.get_mut(root_scope).enter(name, member);
        w.publish_class(root, root_scope, Vec::new());

        let left = w.class("Left");
        let left_scope = w.scope(left);
        let root_left_view = w.class_ref(root);
        w.publish_class(left, left_scope, vec![root_left_view]);
        let right = w.class("Right");
        let right_scope = w.scope(right);
        let root_right_view = w.class_ref(root);
        w.publish_class(right, right_scope, vec![root_right_view]);

        let child = w.class("Child");
        let child_scope = w.scope(child);
        let left_view = w.class_ref(left);
        let right_view = w.class_ref(right);
        w.publish_class(child, child_scope, vec![left_view, right_view]);
        let receiver = w.class_ref(child);

        let candidates = w.lookup(receiver, name).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].symbol, member);
        assert_eq!(candidates[0].receiver_view, root_left_view);
        assert_eq!(candidates[0].inheritance_depth, 2);
    }

    #[test]
    fn inherited_cycles_are_reported() {
        let mut w = World::new();
        let left = w.class("Left");
        let right = w.class("Right");
        let left_scope = w.scope(left);
        let right_scope = w.scope(right);
        let right_view = w.class_ref(right);
        let left_view = w.class_ref(left);
        w.publish_class(left, left_scope, vec![right_view]);
        w.publish_class(right, right_scope, vec![left_view]);
        let name = Name::new(w.store.names.intern("missing"), Namespace::Term);

        assert!(matches!(
            w.lookup(left_view, name),
            Err(MemberLookupError::InheritanceCycle { .. })
        ));
    }

    #[test]
    fn inheritance_traversal_rejects_graphs_past_the_depth_limit() {
        let mut w = World::new();
        let mut classes = Vec::new();
        let mut scopes = Vec::new();
        for index in 0..=MAX_MEMBER_LOOKUP_DEPTH + 1 {
            let class = w.class(&format!("C{index}"));
            classes.push(class);
            scopes.push(w.scope(class));
        }
        for index in 0..classes.len() {
            let parents = classes
                .get(index + 1)
                .map(|parent| vec![w.class_ref(*parent)])
                .unwrap_or_default();
            w.publish_class(classes[index], scopes[index], parents);
        }
        let receiver = w.class_ref(classes[0]);
        let missing = Name::new(w.store.names.intern("missing"), Namespace::Term);

        assert!(matches!(
            w.lookup(receiver, missing),
            Err(MemberLookupError::TooDeep)
        ));
    }

    #[test]
    fn inheritance_traversal_accepts_a_graph_at_the_depth_limit() {
        let mut w = World::new();
        let mut classes = Vec::new();
        let mut scopes = Vec::new();
        for index in 0..=MAX_MEMBER_LOOKUP_DEPTH {
            let class = w.class(&format!("C{index}"));
            classes.push(class);
            scopes.push(w.scope(class));
        }
        for index in 0..classes.len() {
            let parents = classes
                .get(index + 1)
                .map(|parent| vec![w.class_ref(*parent)])
                .unwrap_or_default();
            w.publish_class(classes[index], scopes[index], parents);
        }
        let receiver = w.class_ref(classes[0]);
        let missing = Name::new(w.store.names.intern("missing"), Namespace::Term);

        assert!(w.lookup(receiver, missing).unwrap().is_empty());
    }

    #[test]
    fn union_receivers_are_explicitly_unsupported() {
        let mut w = World::new();
        let left = w.store.types.alloc(Type::NoType);
        let right = w.store.types.alloc(Type::NoType);
        let union = w.store.types.alloc(Type::Or { left, right });
        let name = Name::new(w.store.names.intern("member"), Namespace::Term);

        assert!(matches!(
            w.lookup(union, name),
            Err(MemberLookupError::UnsupportedReceiverType { ty }) if ty == union
        ));
    }

    #[test]
    fn intersection_receivers_are_explicitly_unsupported() {
        let mut w = World::new();
        let left = w.store.types.alloc(Type::NoType);
        let right = w.store.types.alloc(Type::NoType);
        let intersection = w.store.types.alloc(Type::And { left, right });
        let name = Name::new(w.store.names.intern("member"), Namespace::Term);

        assert!(matches!(
            w.lookup(intersection, name),
            Err(MemberLookupError::UnsupportedReceiverType { ty }) if ty == intersection
        ));
    }

    #[test]
    fn match_type_receivers_are_explicitly_unsupported() {
        let mut w = World::new();
        let match_type = w
            .store
            .types
            .alloc(Type::Match(dotty_core::types::MatchType {
                bound: w.definitions.no_prefix,
                scrutinee: w.definitions.no_prefix,
                cases: Vec::new(),
            }));
        let name = Name::new(w.store.names.intern("member"), Namespace::Term);

        assert!(matches!(
            w.lookup(match_type, name),
            Err(MemberLookupError::UnsupportedReceiverType { ty }) if ty == match_type
        ));
    }

    #[test]
    fn recursive_receivers_are_explicitly_unsupported() {
        let mut w = World::new();
        let recursive = w.store.types.alloc(Type::Recursive {
            parent: w.definitions.no_prefix,
        });
        let name = Name::new(w.store.names.intern("member"), Namespace::Term);

        assert!(matches!(
            w.lookup(recursive, name),
            Err(MemberLookupError::UnsupportedReceiverType { ty }) if ty == recursive
        ));
    }

    #[test]
    fn refinement_parent_is_rejected_as_malformed() {
        let mut w = World::new();
        let class = w.class("C");
        let scope = w.scope(class);
        let refined = w.store.types.alloc(Type::Refined {
            parent: w.definitions.no_prefix,
            name: Name::new(w.store.names.intern("T"), Namespace::Type),
            info: w.definitions.no_prefix,
        });
        w.publish_class(class, scope, vec![refined]);
        let receiver = w.class_ref(class);
        let name = Name::new(w.store.names.intern("missing"), Namespace::Term);

        assert!(matches!(
            w.lookup(receiver, name),
            Err(MemberLookupError::MalformedParentType { ty }) if ty == refined
        ));
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

    #[test]
    fn external_missing_class_info_is_an_explicit_error() {
        let mut w = World::new();
        let class = w.class("External");
        let receiver = w.class_ref(class);
        let name = Name::new(w.store.names.intern("member"), Namespace::Term);

        assert!(matches!(
            w.lookup(receiver, name),
            Err(MemberLookupError::ClassInfoUnavailable { symbol, state: SymbolInfoState::Missing }) if symbol == class
        ));
    }

    #[test]
    fn overload_lookup_rejects_incomplete_inherited_candidates() {
        let mut w = World::new();
        let source_class = w.class("Source");
        let parent = w.class("ExternalParent");
        let scope = w.scope(source_class);
        let method = w.symbol(
            "f",
            Namespace::Term,
            SymbolKind::Method,
            SymbolInfo::Missing,
        );
        let name = w.store.symbols.get(method).name;
        w.store.scopes.get_mut(scope).enter(name, method);
        let parent_view = w.class_ref(parent);
        w.publish_class(source_class, scope, vec![parent_view]);
        let receiver = w.class_ref(source_class);
        let mut typer = SourceTyper::new(
            &w.arena,
            w.source,
            &w.index,
            &mut w.store,
            w.definitions,
            &w.packages,
        );

        assert!(matches!(
            typer.lookup_overload_members_journaled(receiver, name, &mut Vec::new()),
            Err(MemberLookupError::ClassInfoUnavailable {
                symbol,
                state: SymbolInfoState::Missing,
            }) if symbol == parent
        ));
    }

    #[test]
    fn errored_external_class_info_is_an_explicit_error() {
        let mut w = World::new();
        let class = w.symbol(
            "External",
            Namespace::Type,
            SymbolKind::Class,
            SymbolInfo::Error,
        );
        let receiver = w.class_ref(class);
        let name = Name::new(w.store.names.intern("member"), Namespace::Term);

        assert!(matches!(
            w.lookup(receiver, name),
            Err(MemberLookupError::ClassInfoUnavailable { symbol, state: SymbolInfoState::Error }) if symbol == class
        ));
    }

    #[test]
    fn a_complete_non_class_info_is_rejected() {
        let mut w = World::new();
        let info = w.store.types.alloc(Type::NoType);
        let class = w.symbol(
            "C",
            Namespace::Type,
            SymbolKind::Class,
            SymbolInfo::Complete(info),
        );
        let receiver = w.class_ref(class);
        let name = Name::new(w.store.names.intern("member"), Namespace::Term);

        assert!(matches!(
            w.lookup(receiver, name),
            Err(MemberLookupError::ClassInfoNotClassInfo { symbol, info: found }) if symbol == class && found == info
        ));
    }

    #[test]
    fn a_class_info_with_the_wrong_class_identity_is_rejected() {
        let mut w = World::new();
        let class = w.class("C");
        let other = w.class("Other");
        let scope = w.scope(class);
        let info = w.store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: w.definitions.no_prefix,
            class: other,
            parents: Vec::new(),
            declarations: scope,
            self_type: None,
        }));
        w.store.symbols.set_info(class, SymbolInfo::Complete(info));
        let receiver = w.class_ref(class);
        let name = Name::new(w.store.names.intern("member"), Namespace::Term);

        assert!(matches!(
            w.lookup(receiver, name),
            Err(MemberLookupError::MalformedClassInfoIdentity { symbol, recorded_class }) if symbol == class && recorded_class == other
        ));
    }

    #[test]
    fn an_invalid_declaration_scope_is_rejected() {
        let mut w = World::new();
        let class = w.class("C");
        let mut foreign_store = SemanticStore::new();
        let foreign_scope = foreign_store
            .scopes
            .alloc(dotty_core::Scope::new(Some(class)));
        let info = w.store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: w.definitions.no_prefix,
            class,
            parents: Vec::new(),
            declarations: foreign_scope,
            self_type: None,
        }));
        w.store.symbols.set_info(class, SymbolInfo::Complete(info));
        let receiver = w.class_ref(class);
        let name = Name::new(w.store.names.intern("member"), Namespace::Term);

        assert!(matches!(
            w.lookup(receiver, name),
            Err(MemberLookupError::InvalidDeclarationScope { symbol, scope }) if symbol == class && scope == foreign_scope
        ));
    }
}
