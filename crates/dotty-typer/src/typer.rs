//! Source declaration completion driver.

use std::fmt;

use dotty_core::ast::{Ident, TreeKind};
use dotty_core::types::{Type, TypeRefTarget};
use dotty_core::{
    AstArena, Definitions, Packages, SemanticStore, SourceId, SymbolId, SymbolInfo, SymbolKind,
    TreeId, TypeId, Untyped,
};
use dotty_namer::{SourceContextId, SourceDefinition, SourceSemanticIndex};

use crate::SourceTypeIndex;

/// A recoverable failure while projecting or completing source semantics.
#[derive(Debug)]
pub enum TyperError {
    /// A symbol is not present in the semantic store.
    UnknownSymbol { symbol: SymbolId },
    /// The namer did not retain source provenance for this symbol.
    SourceProvenanceMissing { symbol: SymbolId },
    /// A declaration that needs lexical lookup has no namer context.
    DeclarationContextMissing { symbol: SymbolId },
    /// Completion for this semantic declaration category is not implemented.
    UnsupportedSymbolCompletion { symbol: SymbolId, kind: SymbolKind },
    /// A source type-tree form is not supported yet.
    UnsupportedTypeTree {
        source: SourceId,
        tree_index: u32,
        tree_kind: &'static str,
    },
    /// The source tree does not have the definition shape expected by its symbol.
    MalformedSourceAst {
        source: SourceId,
        tree_index: u32,
        symbol: SymbolId,
        kind: SymbolKind,
    },
    /// The source tree's declaration category does not agree with the symbol.
    SymbolSourceKindMismatch {
        source: SourceId,
        tree_index: u32,
        symbol: SymbolId,
        kind: SymbolKind,
    },
    /// A referenced tree ID is outside the supplied arena.
    TreeOutsideArena { source: SourceId, tree_index: u32 },
    /// A simple type identifier did not resolve in its declaration scope.
    TypeNameNotFound {
        source: SourceId,
        tree_index: u32,
        name: dotty_core::Name,
    },
    /// The lexical scope exposes multiple possible type declarations.
    AmbiguousTypeName {
        source: SourceId,
        tree_index: u32,
        name: dotty_core::Name,
    },
    /// The type-tree cache was already bound to a different type.
    DuplicateSourceTypeCacheEntry {
        source: SourceId,
        tree_index: u32,
        existing: TypeId,
        attempted: TypeId,
    },
    /// A future completion operation could not rebind a semantic type.
    TypeRebinding {
        source: SourceId,
        tree_index: u32,
        error: dotty_core::TypeRebindError,
    },
}

impl fmt::Display for TyperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for TyperError {}

/// Completes source declaration signatures against a shared semantic session.
pub struct SourceTyper<'a> {
    arena: &'a AstArena<Untyped>,
    source: SourceId,
    index: &'a SourceSemanticIndex,
    store: &'a mut SemanticStore,
    definitions: Definitions,
    packages: &'a Packages,
    type_index: SourceTypeIndex,
}

impl<'a> SourceTyper<'a> {
    /// Creates a driver using caller-owned, already-bootstrapped definitions.
    pub fn new(
        arena: &'a AstArena<Untyped>,
        source: SourceId,
        index: &'a SourceSemanticIndex,
        store: &'a mut SemanticStore,
        definitions: Definitions,
        packages: &'a Packages,
    ) -> Self {
        Self {
            arena,
            source,
            index,
            store,
            definitions,
            packages,
            type_index: SourceTypeIndex::default(),
        }
    }

    /// Returns this driver's source type cache.
    pub fn source_type_index(&self) -> &SourceTypeIndex {
        &self.type_index
    }

    /// Completes a source declaration, rolling back this call's mutations on failure.
    pub fn complete_symbol(&mut self, symbol: SymbolId) -> Result<TypeId, TyperError> {
        if !self.store.symbols.contains(symbol) {
            return Err(TyperError::UnknownSymbol { symbol });
        }
        if let SymbolInfo::Complete(ty) = *self.store.symbols.info(symbol) {
            return Ok(ty);
        }

        let checkpoint = self.store.checkpoint();
        let cache_checkpoint = self.type_index.checkpoint();
        let mut info_journal = Vec::new();
        let result = self.complete_symbol_inner(symbol, &mut info_journal);
        if result.is_err() {
            for (changed, previous) in info_journal.into_iter().rev() {
                if self.store.symbols.contains(changed) {
                    self.store.symbols.set_info(changed, previous);
                }
            }
            self.store.rollback_to(checkpoint);
            self.type_index.restore(cache_checkpoint);
        }
        result
    }

    fn complete_symbol_inner(
        &mut self,
        symbol: SymbolId,
        _info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let kind = self.store.symbols.get(symbol).kind;
        let definition = self
            .index
            .definition_of(symbol)
            .ok_or(TyperError::SourceProvenanceMissing { symbol })?;
        let (source, tree) = match definition {
            SourceDefinition::Canonical { source, tree }
            | SourceDefinition::Derived { source, tree } => (source, tree),
        };
        if source != self.source {
            return Err(TyperError::SourceProvenanceMissing { symbol });
        }
        if self.index.declaration_context_of(symbol).is_none() {
            return Err(TyperError::DeclarationContextMissing { symbol });
        }

        let Some((_, source_tree)) = self.arena.iter().find(|(id, _)| *id == tree) else {
            return Err(TyperError::TreeOutsideArena {
                source,
                tree_index: tree.index(),
            });
        };
        let valid_shape = match (kind, &source_tree.kind) {
            (
                SymbolKind::Field
                | SymbolKind::Value
                | SymbolKind::Variable
                | SymbolKind::Parameter,
                TreeKind::ValDef(_),
            ) => true,
            (SymbolKind::Method | SymbolKind::Constructor, TreeKind::DefDef(_)) => true,
            (
                SymbolKind::TypeParameter
                | SymbolKind::TypeAlias
                | SymbolKind::Class
                | SymbolKind::Trait
                | SymbolKind::ModuleClass,
                TreeKind::TypeDef(_),
            ) => true,
            // An object term is not a module class, even though both may share
            // the same source `TypeDef` provenance.
            (SymbolKind::Object, TreeKind::TypeDef(_)) => true,
            _ => false,
        };
        if !valid_shape {
            return Err(TyperError::SymbolSourceKindMismatch {
                source,
                tree_index: tree.index(),
                symbol,
                kind,
            });
        }

        // The source syntax is known, but signature completion is staged by
        // later typer increments. Keep this explicit category dispatch so a
        // new SymbolKind cannot accidentally become a successful placeholder.
        match kind {
            SymbolKind::Field
            | SymbolKind::Value
            | SymbolKind::Variable
            | SymbolKind::Parameter
            | SymbolKind::TypeParameter
            | SymbolKind::TypeAlias
            | SymbolKind::Method
            | SymbolKind::Constructor
            | SymbolKind::Class
            | SymbolKind::Trait
            | SymbolKind::ModuleClass => {
                Err(TyperError::UnsupportedSymbolCompletion { symbol, kind })
            }
            SymbolKind::Object => Err(TyperError::UnsupportedSymbolCompletion { symbol, kind }),
            SymbolKind::Package | SymbolKind::Local => {
                Err(TyperError::UnsupportedSymbolCompletion { symbol, kind })
            }
        }
    }

    /// Projects a source type tree using the declaration context from the namer.
    pub fn type_of_tpt(
        &mut self,
        tree: TreeId<Untyped>,
        context: SourceContextId,
    ) -> Result<TypeId, TyperError> {
        if let Some(ty) = self.type_index.type_at(self.source, tree) {
            return Ok(ty);
        }
        let Some((_, source_tree)) = self.arena.iter().find(|(id, _)| *id == tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: tree.index(),
            });
        };
        let TreeKind::Ident(Ident { name, .. }) = &source_tree.kind else {
            return Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index: tree.index(),
                tree_kind: tree_kind_name(&source_tree.kind),
            });
        };
        if !name.is_type() {
            return Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index: tree.index(),
                tree_kind: "term identifier",
            });
        }
        let lexical_scope = self.index.source_context(context).lexical_scope;
        let candidates = self.store.scopes.get(lexical_scope).lookup_all(name);
        let target = match candidates {
            [] => {
                return Err(TyperError::TypeNameNotFound {
                    source: self.source,
                    tree_index: tree.index(),
                    name: *name,
                });
            }
            [target] => *target,
            _ => {
                return Err(TyperError::AmbiguousTypeName {
                    source: self.source,
                    tree_index: tree.index(),
                    name: *name,
                });
            }
        };
        let ty = self.store.types.alloc(Type::TypeRef {
            prefix: self.definitions.no_prefix,
            target: TypeRefTarget::Symbol(target),
        });
        if let Err(existing) = self.type_index.insert(self.source, tree, ty) {
            return Err(TyperError::DuplicateSourceTypeCacheEntry {
                source: self.source,
                tree_index: tree.index(),
                existing,
                attempted: ty,
            });
        }
        Ok(ty)
    }

    /// Exposes the shared semantic store after the driver is no longer needed.
    pub fn store(&self) -> &SemanticStore {
        self.store
    }

    /// Exposes the package registry used by this driver.
    pub fn packages(&self) -> &Packages {
        self.packages
    }
}

fn tree_kind_name(kind: &TreeKind<Untyped>) -> &'static str {
    match kind {
        TreeKind::Ident(_) => "identifier",
        TreeKind::TypeTree(_) => "type tree",
        TreeKind::AppliedTypeTree(_) => "applied type tree",
        TreeKind::SingletonTypeTree(_) => "singleton type tree",
        TreeKind::RefinedTypeTree(_) => "refined type tree",
        TreeKind::LambdaTypeTree(_) => "lambda type tree",
        TreeKind::MatchTypeTree(_) => "match type tree",
        TreeKind::ByNameTypeTree(_) => "by-name type tree",
        TreeKind::TypeBoundsTree(_) => "type bounds tree",
        _ => "expression or declaration tree",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::{Name, Namespace, SymbolFlags, SymbolLinks, SymbolOrigin, Visibility};

    fn setup() -> (AstArena<Untyped>, SemanticStore, Packages, Definitions) {
        let arena = AstArena::new();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        (arena, store, Packages::new(), definitions)
    }

    fn symbol(store: &mut SemanticStore, kind: SymbolKind, info: SymbolInfo) -> SymbolId {
        let name = store.names.intern("member");
        store.symbols.alloc(dotty_core::Symbol {
            name: Name::new(name, Namespace::Term),
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

    #[test]
    fn constructor_uses_caller_bootstrapped_definitions() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let source = SourceId::from_index(0);

        let typer = SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        assert_eq!(typer.definitions.int, definitions.int);
    }

    #[test]
    fn complete_returns_an_existing_type_without_requiring_provenance() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let symbol = symbol(
            &mut store,
            SymbolKind::Value,
            SymbolInfo::Complete(definitions.int),
        );
        let mut typer = SourceTyper::new(
            &arena,
            SourceId::from_index(0),
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(symbol),
            Ok(ty) if ty == definitions.int
        ));
    }

    #[test]
    fn missing_provenance_is_reported_without_mutation() {
        let (arena, mut store, packages, definitions) = setup();
        let index = SourceSemanticIndex::new();
        let symbol = symbol(&mut store, SymbolKind::Value, SymbolInfo::Missing);
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &arena,
            SourceId::from_index(0),
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(symbol),
            Err(TyperError::SourceProvenanceMissing { symbol: found }) if found == symbol
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(symbol), SymbolInfo::Missing);
    }
}
