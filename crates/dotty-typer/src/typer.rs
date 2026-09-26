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
    /// The type name may come from an import form not supported by this pass.
    UnsupportedImportContext {
        source: SourceId,
        context_index: u32,
        import_tree_index: u32,
    },
    /// An import qualifier does not resolve to an entered package or scope.
    ImportQualifierNotFound {
        source: SourceId,
        import_tree_index: u32,
    },
    /// An import tree has an unexpected AST shape or stale child reference.
    MalformedSourceImport {
        source: SourceId,
        import_tree_index: u32,
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
        self.run_atomic(|typer, info_journal| {
            if !typer.store.symbols.contains(symbol) {
                return Err(TyperError::UnknownSymbol { symbol });
            }
            match *typer.store.symbols.info(symbol) {
                SymbolInfo::Complete(ty) => return Ok(ty),
                SymbolInfo::Missing => {}
                SymbolInfo::Deferred(_) => {
                    return Err(TyperError::DeferredSymbolCompletion { symbol });
                }
                SymbolInfo::Error => return Err(TyperError::SymbolAlreadyErrored { symbol }),
            }
            typer.complete_symbol_inner(symbol, info_journal)
        })
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
        if self.index.try_source_context(context).is_none() {
            return Err(TyperError::SourceContextMissing {
                source: self.source,
                tree_index: tree.index(),
                context_index: context.index(),
            });
        }
        let target = self.lookup_type_symbol(*name, context, tree.index())?;
        let ty = match target {
            Some(target) => {
                let prefix = self.type_symbol_prefix(target);
                self.store.types.alloc(Type::TypeRef {
                    prefix,
                    target: TypeRefTarget::Symbol(target),
                })
            }
            None => self
                .definitions
                .source_builtin_type(self.store, *name)
                .ok_or(TyperError::TypeNameNotFound {
                    source: self.source,
                    tree_index: tree.index(),
                    name: *name,
                })?,
        };
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

    fn type_symbol_prefix(&mut self, symbol: SymbolId) -> TypeId {
        let Some(owner) = self.store.symbols.get(symbol).owner else {
            return self.definitions.no_prefix;
        };
        if matches!(
            self.store.symbols.get(owner).kind,
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
        ) {
            self.store.types.alloc(Type::ThisType { class: owner })
        } else {
            self.definitions.no_prefix
        }
    }

    fn lookup_type_symbol(
        &self,
        name: dotty_core::Name,
        context: SourceContextId,
        tree_index: u32,
    ) -> Result<Option<SymbolId>, TyperError> {
        let mut current = Some(context);
        while let Some(context_id) = current {
            let Some(source_context) = self.index.try_source_context(context_id) else {
                return Err(TyperError::SourceContextMissing {
                    source: self.source,
                    tree_index,
                    context_index: context_id.index(),
                });
            };
            let candidates = self
                .store
                .scopes
                .get(source_context.lexical_scope)
                .lookup_all(&name);
            if let Some(symbol) = self.unique_type_candidate(candidates, name, tree_index)? {
                return Ok(Some(symbol));
            }
            if let Some(import_tree) = source_context.import
                && let Some(symbol) = self.lookup_imported_symbol(
                    import_tree,
                    context_id,
                    source_context.parent,
                    name,
                    tree_index,
                    true,
                )?
            {
                return Ok(Some(symbol));
            }
            current = source_context.parent;
        }
        Ok(None)
    }

    fn lookup_imported_symbol(
        &self,
        import_tree: TreeId<Untyped>,
        context_id: SourceContextId,
        parent_context: Option<SourceContextId>,
        wanted: dotty_core::Name,
        tree_index: u32,
        type_only: bool,
    ) -> Result<Option<SymbolId>, TyperError> {
        let Some(node) = self.arena.try_get(import_tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: import_tree.index(),
            });
        };
        let TreeKind::Import(import) = &node.kind else {
            return Err(TyperError::MalformedSourceImport {
                source: self.source,
                import_tree_index: import_tree.index(),
            });
        };

        let wildcard = self.store.names.get("*");
        let hidden = self.store.names.get("_");
        let mut relevant = Vec::new();
        for selector in &import.selectors {
            if Some(selector.imported.text()) == wildcard {
                relevant.push((selector.imported, true));
                continue;
            }
            if selector.bound.is_some() {
                continue;
            }
            let public_name = if let Some(renamed) = selector.renamed {
                let Some(rename_tree) = self.arena.try_get(renamed) else {
                    return Err(TyperError::MalformedSourceImport {
                        source: self.source,
                        import_tree_index: import_tree.index(),
                    });
                };
                let TreeKind::Ident(ident) = &rename_tree.kind else {
                    return Err(TyperError::MalformedSourceImport {
                        source: self.source,
                        import_tree_index: import_tree.index(),
                    });
                };
                ident.name.text()
            } else {
                selector.imported.text()
            };
            if public_name == wanted.text() && Some(public_name) != hidden {
                relevant.push((selector.imported, false));
            }
        }
        if relevant.is_empty() {
            return Ok(None);
        }

        let qualifier_context = parent_context.unwrap_or(context_id);
        let Some(scope) =
            self.import_qualifier_scope(import.expr, qualifier_context, import_tree.index())?
        else {
            return Err(TyperError::ImportQualifierNotFound {
                source: self.source,
                import_tree_index: import_tree.index(),
            });
        };
        let mut matches = Vec::new();
        for (imported, is_wildcard) in relevant {
            if type_only {
                let candidates = if is_wildcard {
                    self.store.scopes.get(scope).lookup_all(&wanted)
                } else {
                    let imported_type =
                        dotty_core::Name::new(imported.text(), dotty_core::Namespace::Type);
                    self.store.scopes.get(scope).lookup_all(&imported_type)
                };
                matches.extend_from_slice(candidates);
            } else {
                let name = if is_wildcard { wanted } else { imported };
                if let Some(symbol) = self.unique_scoped_symbol(scope, name, tree_index)? {
                    matches.push(symbol);
                }
            }
        }
        matches.sort_by_key(|symbol| symbol.index());
        matches.dedup();
        if type_only {
            self.unique_type_candidate(&matches, wanted, tree_index)
        } else {
            self.unique_symbol_candidate(&matches, wanted, tree_index)
        }
    }

    fn import_qualifier_scope(
        &self,
        qualifier: TreeId<Untyped>,
        context: SourceContextId,
        import_tree_index: u32,
    ) -> Result<Option<dotty_core::ScopeId>, TyperError> {
        let mut selections = Vec::new();
        let mut cursor = qualifier;
        let root_name = loop {
            let Some(node) = self.arena.try_get(cursor) else {
                return Err(TyperError::MalformedSourceImport {
                    source: self.source,
                    import_tree_index: cursor.index(),
                });
            };
            match &node.kind {
                TreeKind::Ident(ident) => break ident.name,
                TreeKind::Select(select) => {
                    selections.push(select.name);
                    cursor = select.qualifier;
                }
                _ => {
                    return Err(TyperError::UnsupportedImportContext {
                        source: self.source,
                        context_index: context.index(),
                        import_tree_index: cursor.index(),
                    });
                }
            }
        };
        let mut symbol = self.lookup_context_symbol(root_name, context, import_tree_index)?;
        for selection in selections.into_iter().rev() {
            let Some(current_symbol) = symbol else {
                return Ok(None);
            };
            let Some(scope) = self.scope_of(current_symbol) else {
                return Ok(None);
            };
            symbol = self.unique_scoped_symbol(scope, selection, import_tree_index)?;
        }
        Ok(symbol.and_then(|symbol| self.scope_of(symbol)))
    }

    fn lookup_context_symbol(
        &self,
        name: dotty_core::Name,
        context: SourceContextId,
        import_tree_index: u32,
    ) -> Result<Option<SymbolId>, TyperError> {
        let mut current = Some(context);
        while let Some(context_id) = current {
            let Some(source_context) = self.index.try_source_context(context_id) else {
                return Err(TyperError::SourceContextMissing {
                    source: self.source,
                    tree_index: import_tree_index,
                    context_index: context_id.index(),
                });
            };
            if let Some(symbol) =
                self.unique_scoped_symbol(source_context.lexical_scope, name, import_tree_index)?
            {
                return Ok(Some(symbol));
            }
            if let Some(import_tree) = source_context.import
                && let Some(symbol) = self.lookup_imported_symbol(
                    import_tree,
                    context_id,
                    source_context.parent,
                    name,
                    import_tree_index,
                    false,
                )?
            {
                return Ok(Some(symbol));
            }
            current = source_context.parent;
        }
        Ok(None)
    }

    fn unique_scoped_symbol(
        &self,
        scope: dotty_core::ScopeId,
        name: dotty_core::Name,
        tree_index: u32,
    ) -> Result<Option<SymbolId>, TyperError> {
        let direct = self.store.scopes.get(scope).lookup_all(&name);
        if !direct.is_empty() {
            return self.unique_symbol_candidate(direct, name, tree_index);
        }
        let alternate = dotty_core::Name::new(
            name.text(),
            match name.namespace() {
                dotty_core::Namespace::Term => dotty_core::Namespace::Type,
                dotty_core::Namespace::Type => dotty_core::Namespace::Term,
            },
        );
        self.unique_symbol_candidate(
            self.store.scopes.get(scope).lookup_all(&alternate),
            name,
            tree_index,
        )
    }

    fn unique_type_candidate(
        &self,
        candidates: &[SymbolId],
        name: dotty_core::Name,
        tree_index: u32,
    ) -> Result<Option<SymbolId>, TyperError> {
        let candidates: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|symbol| {
                matches!(
                    self.store.symbols.get(*symbol).kind,
                    SymbolKind::Class
                        | SymbolKind::Trait
                        | SymbolKind::ModuleClass
                        | SymbolKind::TypeParameter
                        | SymbolKind::TypeAlias
                )
            })
            .collect();
        self.unique_symbol_candidate(&candidates, name, tree_index)
    }

    fn unique_symbol_candidate(
        &self,
        candidates: &[SymbolId],
        name: dotty_core::Name,
        tree_index: u32,
    ) -> Result<Option<SymbolId>, TyperError> {
        match candidates {
            [] => Ok(None),
            [symbol] => Ok(Some(*symbol)),
            _ => Err(TyperError::AmbiguousTypeName {
                source: self.source,
                tree_index,
                name,
            }),
        }
    }

    fn scope_of(&self, symbol: SymbolId) -> Option<dotty_core::ScopeId> {
        if let Some(scope) = self
            .packages
            .scope_of(symbol)
            .or_else(|| self.index.scope_of(symbol))
        {
            return Some(scope);
        }
        let semantic = self.store.symbols.get(symbol);
        if semantic.kind != SymbolKind::Object {
            return None;
        }
        let owner = semantic.owner?;
        let SourceDefinition::Canonical { source, tree } = self.index.definition_of(symbol)? else {
            return None;
        };
        if source != self.source {
            return None;
        }
        let module_class = self.index.derived_symbol_at(owner, source, tree)?;
        (self.store.symbols.get(module_class).kind == SymbolKind::ModuleClass)
            .then(|| self.index.scope_of(module_class))
            .flatten()
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

    fn type_symbol(store: &SemanticStore, ty: TypeId) -> SymbolId {
        match store.types.get(ty) {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("expected a symbol TypeRef, got {other:?}"),
        }
    }

    fn complete_builtin_annotation(name: &str) -> (TypeId, Definitions) {
        let text = format!("val x: {name} = 1");
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(&text);
        let (symbol, _) = val_symbol(&parsed, &store, &index, source, "x");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let ty = typer.complete_symbol(symbol).unwrap();
        (ty, definitions)
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
    fn object_name_alone_does_not_resolve_as_a_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("object O\nval x: O = 1");
        let (symbol, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(symbol),
            Err(TyperError::TypeNameNotFound {
                source: found_source,
                tree_index,
                name,
            }) if found_source == source
                && tree_index == type_tree.index()
                && typer.store().names.resolve(name.text()) == "O"
                && name.is_type()
        ));
    }

    #[test]
    fn nested_type_lookup_walks_to_the_enclosing_declaration_context() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer { class Inner { val x: Outer = 1 } }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let outer = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Outer" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), outer);
    }

    #[test]
    fn member_type_reference_uses_the_enclosing_class_this_type_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer { class Inner; val x: Inner = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let inner = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Inner" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let owner = store.symbols.get(inner).owner.unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        let Type::TypeRef { prefix, target } = typer.store().types.get(projected) else {
            panic!("expected a TypeRef for the member type")
        };
        assert_eq!(*target, TypeRefTarget::Symbol(inner));
        assert_eq!(
            typer.store().types.get(*prefix),
            &Type::ThisType { class: owner }
        );
    }

    #[test]
    fn member_type_reference_in_an_object_uses_its_module_class_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("object O { class Inner; val x: Inner = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let inner = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Inner" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let owner = store.symbols.get(inner).owner.unwrap();
        assert_eq!(store.symbols.get(owner).kind, SymbolKind::ModuleClass);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        let Type::TypeRef { prefix, target } = typer.store().types.get(projected) else {
            panic!("expected a TypeRef for the member type")
        };
        assert_eq!(*target, TypeRefTarget::Symbol(inner));
        assert_eq!(
            typer.store().types.get(*prefix),
            &Type::ThisType { class: owner }
        );
    }

    #[test]
    fn innermost_type_declaration_shadows_an_enclosing_name() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class T; class Outer { class T; val x: T = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let outer = store.symbols.get(value).owner.unwrap();
        let inner_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "T" =>
                {
                    let symbol = index.symbol_at(source, tree)?;
                    (store.symbols.get(symbol).owner == Some(outer)).then_some(symbol)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), inner_type);
    }

    #[test]
    fn explicit_package_import_resolves_a_type_name() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Imported }\npackage app { import lib.Imported; val x: Imported = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let imported_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Imported" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), imported_type);
    }

    #[test]
    fn wildcard_package_import_resolves_a_type_name() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Imported }\npackage app { import lib.*; val x: Imported = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let imported_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Imported" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), imported_type);
    }

    #[test]
    fn renamed_package_import_resolves_the_alias() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Imported }\npackage app { import lib.{Imported as Alias}; val x: Alias = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let imported_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Imported" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), imported_type);
    }

    #[test]
    fn later_import_qualifier_can_use_an_earlier_type_alias() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "package lib { class Imported { class Nested } }\npackage app { import lib.Imported as Alias; import Alias.Nested; val x: Nested = 1 }",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let nested_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Nested" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), nested_type);
    }

    #[test]
    fn later_import_qualifier_can_use_an_earlier_object_alias() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "object Lib { object Nested { class X } }\nimport Lib.Nested as Alias\nimport Alias.X\nval x: X = 1",
        );
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let nested_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "X" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), nested_type);
    }

    #[test]
    fn wildcard_import_from_an_object_uses_its_namer_module_class_scope() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("object Lib { class Imported }\nimport Lib.*\nval x: Imported = 1");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let imported_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Imported" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), imported_type);
    }

    #[test]
    fn primitive_int_annotation_uses_the_bootstrapped_type() {
        let (ty, definitions) = complete_builtin_annotation("Int");

        assert_eq!(ty, definitions.int);
    }

    #[test]
    fn lexical_type_named_int_shadows_the_builtin() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Int; val x: Int = 1");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let local_int = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Int" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), local_int);
    }

    #[test]
    fn unit_annotation_uses_the_bootstrapped_type() {
        let (ty, definitions) = complete_builtin_annotation("Unit");

        assert_eq!(ty, definitions.unit);
    }

    #[test]
    fn any_annotation_uses_the_bootstrapped_type() {
        let (ty, definitions) = complete_builtin_annotation("Any");

        assert_eq!(ty, definitions.any_type);
    }

    #[test]
    fn nothing_annotation_uses_the_bootstrapped_type() {
        let (ty, definitions) = complete_builtin_annotation("Nothing");

        assert_eq!(ty, definitions.nothing_type);
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
