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
    /// A source context ID is outside the naming result that produced it.
    SourceContextMissing {
        source: SourceId,
        tree_index: u32,
        context_index: u32,
    },
    /// Completion for this semantic declaration category is not implemented.
    UnsupportedSymbolCompletion { symbol: SymbolId, kind: SymbolKind },
    /// A deferred completion belongs to a future completion engine.
    DeferredSymbolCompletion { symbol: SymbolId },
    /// A previous fatal semantic failure has already been recorded.
    SymbolAlreadyErrored { symbol: SymbolId },
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
        match *self.store.symbols.info(symbol) {
            SymbolInfo::Complete(ty) => return Ok(ty),
            SymbolInfo::Missing => {}
            SymbolInfo::Deferred(_) => {
                return Err(TyperError::DeferredSymbolCompletion { symbol });
            }
            SymbolInfo::Error => return Err(TyperError::SymbolAlreadyErrored { symbol }),
        }

        self.run_atomic(|typer, info_journal| typer.complete_symbol_inner(symbol, info_journal))
    }

    fn run_atomic<T>(
        &mut self,
        operation: impl FnOnce(&mut Self, &mut Vec<(SymbolId, SymbolInfo)>) -> Result<T, TyperError>,
    ) -> Result<T, TyperError> {
        let checkpoint = self.store.checkpoint();
        let cache_checkpoint = self.type_index.checkpoint();
        let mut info_journal = Vec::new();
        let result = operation(self, &mut info_journal);
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
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
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

        let Some(source_tree) = self.arena.try_get(tree) else {
            return Err(TyperError::TreeOutsideArena {
                source,
                tree_index: tree.index(),
            });
        };
        let type_tree = match (kind, &source_tree.kind) {
            (
                SymbolKind::Field
                | SymbolKind::Value
                | SymbolKind::Variable
                | SymbolKind::Parameter,
                TreeKind::ValDef(definition),
            ) => Some(definition.tpt),
            (SymbolKind::Method | SymbolKind::Constructor, TreeKind::DefDef(_)) => None,
            (
                SymbolKind::TypeParameter
                | SymbolKind::TypeAlias
                | SymbolKind::Class
                | SymbolKind::Trait,
                TreeKind::TypeDef(_),
            ) => None,
            // Namer records both object term and derived module class against
            // the original parser-only `ModuleDef`; keep their semantic
            // dispatch distinct even though their source tree is shared.
            (
                SymbolKind::Object | SymbolKind::ModuleClass,
                TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(_)),
            ) => None,
            _ => {
                return Err(TyperError::SymbolSourceKindMismatch {
                    source,
                    tree_index: tree.index(),
                    symbol,
                    kind,
                });
            }
        };

        match kind {
            SymbolKind::Field
            | SymbolKind::Value
            | SymbolKind::Variable
            | SymbolKind::Parameter => {
                let Some(tpt) = type_tree else {
                    return Err(TyperError::MalformedSourceAst {
                        source,
                        tree_index: tree.index(),
                        symbol,
                        kind,
                    });
                };
                let context = self
                    .index
                    .declaration_context_of(symbol)
                    .ok_or(TyperError::DeclarationContextMissing { symbol })?;
                let ty = self.type_of_tpt(tpt, context)?;
                let previous = *self.store.symbols.info(symbol);
                info_journal.push((symbol, previous));
                self.store
                    .symbols
                    .set_info(symbol, SymbolInfo::Complete(ty));
                Ok(ty)
            }
            SymbolKind::TypeParameter
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
        let Some(source_tree) = self.arena.try_get(tree) else {
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
        let Some(source_context) = self.index.try_source_context(context) else {
            return Err(TyperError::SourceContextMissing {
                source: self.source,
                tree_index: tree.index(),
                context_index: context.index(),
            });
        };
        let lexical_scope = source_context.lexical_scope;
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
    use dotty_core::{
        Name, Namespace, SourceText, SymbolFlags, SymbolLinks, SymbolOrigin, Visibility,
    };
    use dotty_lexer::ContextualScanner;
    use dotty_namer::name_compilation_unit;

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

    fn parse_and_name(
        text: &str,
    ) -> (
        dotty_parser::ParseResult,
        SemanticStore,
        Packages,
        Definitions,
        SourceSemanticIndex,
        SourceId,
    ) {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let source = SourceId::from_index(11);
        let scanner = ContextualScanner::new(text).unwrap();
        let parsed = dotty_parser::parse_compilation_unit(
            SourceText::new(text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let mut packages = Packages::new();
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "Typer.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        (parsed, store, packages, definitions, index, source)
    }

    fn val_symbol(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
        target: &str,
    ) -> (SymbolId, TreeId<Untyped>) {
        for (tree, node) in parsed.ast.iter() {
            if let TreeKind::ValDef(definition) = &node.kind
                && store.names.resolve(definition.name.as_name().text()) == target
            {
                return (index.symbol_at(source, tree).unwrap(), definition.tpt);
            }
        }
        panic!("source val `{target}` not found");
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

    #[test]
    fn named_value_completion_projects_and_caches_its_type_tree() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C\nval x: C = 1");
        let (symbol, tpt) = val_symbol(&parsed, &store, &index, source, "x");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let completed = typer.complete_symbol(symbol).unwrap();
        let context = index.declaration_context_of(symbol).unwrap();
        let projected_again = typer.type_of_tpt(tpt, context).unwrap();

        assert_eq!(completed, projected_again);
        assert_eq!(
            typer.source_type_index().type_at(source, tpt),
            Some(completed)
        );
        assert_eq!(
            *typer.store().symbols.info(symbol),
            SymbolInfo::Complete(completed)
        );
    }

    #[test]
    fn unsupported_method_signature_fails_without_store_changes() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def method: Int = 1");
        let before = store.checkpoint();
        let method = index
            .symbol_at(
                source,
                parsed
                    .ast
                    .iter()
                    .find_map(|(id, node)| matches!(node.kind, TreeKind::DefDef(_)).then_some(id))
                    .unwrap(),
            )
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(method),
            Err(TyperError::UnsupportedSymbolCompletion {
                symbol,
                kind: SymbolKind::Method
            }) if symbol == method
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(method), SymbolInfo::Missing);
    }

    #[test]
    fn type_projection_rejects_a_context_from_another_naming_index() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C\nval x: C = 1");
        let (symbol, tpt) = val_symbol(&parsed, &store, &index, source, "x");
        let context = index.declaration_context_of(symbol).unwrap();
        let foreign_index = SourceSemanticIndex::new();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &foreign_index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_of_tpt(tpt, context),
            Err(TyperError::SourceContextMissing { tree_index, .. }) if tree_index == tpt.index()
        ));
    }

    #[test]
    fn completion_does_not_find_a_definition_by_scanning_the_ast() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("val x: Int = 1");
        let (symbol, _) = val_symbol(&parsed, &store, &index, source, "x");
        let empty_index = SourceSemanticIndex::new();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &empty_index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(symbol),
            Err(TyperError::SourceProvenanceMissing { symbol: found }) if found == symbol
        ));
    }

    #[test]
    fn a_later_failed_completion_keeps_an_earlier_success() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class C\nval x: C = 1\ndef method: C = 1");
        let (value, value_tpt) = val_symbol(&parsed, &store, &index, source, "x");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(id, node)| {
                matches!(&node.kind, TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "method")
                .then_some(id)
            })
            .unwrap();
        let method = index.symbol_at(source, method_tree).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let value_type = typer.complete_symbol(value).unwrap();
        assert!(matches!(
            typer.complete_symbol(method),
            Err(TyperError::UnsupportedSymbolCompletion {
                kind: SymbolKind::Method,
                ..
            })
        ));

        assert_eq!(
            *typer.store().symbols.info(value),
            SymbolInfo::Complete(value_type)
        );
        assert_eq!(*typer.store().symbols.info(method), SymbolInfo::Missing);
        assert_eq!(
            typer.source_type_index().type_at(source, value_tpt),
            Some(value_type)
        );
    }

    #[test]
    fn object_term_and_derived_module_class_keep_distinct_dispatch() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("object O");
        let (tree, object) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                matches!(
                    node.kind,
                    TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(_))
                )
                .then(|| (tree, index.symbol_at(source, tree).unwrap()))
            })
            .unwrap();
        let owner = store.symbols.get(object).owner.unwrap();
        let module_class = index.derived_symbol_at(owner, source, tree).unwrap();
        assert_eq!(store.symbols.get(object).kind, SymbolKind::Object);
        assert_eq!(
            store.symbols.get(module_class).kind,
            SymbolKind::ModuleClass
        );
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(object),
            Err(TyperError::UnsupportedSymbolCompletion {
                kind: SymbolKind::Object,
                ..
            })
        ));
        assert!(matches!(
            typer.complete_symbol(module_class),
            Err(TyperError::UnsupportedSymbolCompletion {
                kind: SymbolKind::ModuleClass,
                ..
            })
        ));
    }

    #[test]
    fn failed_transaction_restores_store_symbol_info_and_type_cache() {
        let (_unused_arena, mut store, packages, definitions) = setup();
        let source = SourceId::from_index(7);
        let tree = {
            let mut arena = AstArena::<Untyped>::new();
            let tree = arena.alloc(dotty_core::Tree {
                kind: TreeKind::TypeTree(dotty_core::ast::TypeTree),
                position: None,
                ty: (),
            });
            // The transaction only indexes an ID; the caller-owned arena
            // below must outlive the driver, so return the arena with the ID.
            (arena, tree)
        };
        let (arena, tree_id) = tree;
        let index = SourceSemanticIndex::new();
        let symbol = symbol(&mut store, SymbolKind::Value, SymbolInfo::Missing);
        let before = store.checkpoint();
        let attempted_type = store.types.alloc(Type::NoType);
        store.rollback_to(before);
        let mut typer =
            SourceTyper::new(&arena, source, &index, &mut store, definitions, &packages);

        let result: Result<(), TyperError> = typer.run_atomic(|typer, journal| {
            let new_type = typer.store.types.alloc(Type::Error(dotty_core::ErrorType {
                message: typer.store.names.intern("temporary"),
            }));
            journal.push((symbol, *typer.store.symbols.info(symbol)));
            typer
                .store
                .symbols
                .set_info(symbol, SymbolInfo::Complete(new_type));
            typer.type_index.insert(source, tree_id, new_type).unwrap();
            Err(TyperError::UnsupportedSymbolCompletion {
                symbol,
                kind: SymbolKind::Value,
            })
        });

        assert!(matches!(
            result,
            Err(TyperError::UnsupportedSymbolCompletion { .. })
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(symbol), SymbolInfo::Missing);
        assert_eq!(typer.source_type_index().type_at(source, tree_id), None);
        drop(typer);
        assert_eq!(store.types.alloc(Type::NoType), attempted_type);
    }
}
