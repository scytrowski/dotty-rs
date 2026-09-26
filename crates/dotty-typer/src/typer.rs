//! Source declaration completion driver.

use std::fmt;

use dotty_core::ast::{Ident, TreeKind, UntypedNode};
use dotty_core::types::{Type, TypeRefTarget};
use dotty_core::{
    AstArena, Definitions, MemberRequest, MemberSelector, MemberSpace, NoResolver, Packages,
    ResolutionError, SemanticStore, SourceId, SourceSpan, SymbolId, SymbolInfo, SymbolKind,
    SymbolResolver, TreeId, TypeId, Untyped,
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
    /// A declaration has no source-written type and must not be inferred here.
    MissingDeclaredType {
        source: SourceId,
        tree_index: u32,
        position: Option<SourceSpan>,
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
        position: Option<SourceSpan>,
    },
    /// A source name exists, but its semantic symbol cannot denote a type.
    WrongTypeNameKind {
        source: SourceId,
        tree_index: u32,
        name: dotty_core::Name,
        symbol: SymbolId,
        kind: SymbolKind,
        position: Option<SourceSpan>,
    },
    /// The external semantic symbol resolver could not answer soundly.
    SymbolResolution {
        source: SourceId,
        tree_index: u32,
        error: ResolutionError,
    },
    /// The lexical scope exposes multiple possible type declarations.
    AmbiguousTypeName {
        source: SourceId,
        tree_index: u32,
        name: dotty_core::Name,
        position: Option<SourceSpan>,
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
    resolver: Box<dyn SymbolResolver + 'a>,
    type_index: SourceTypeIndex,
}

#[derive(Clone, Copy)]
struct SourceTreeLocation {
    tree_index: u32,
    position: Option<SourceSpan>,
}

/// Resolves source-written type names without making the type-tree dispatcher
/// depend on the details of lexical scopes, imports, or semantic lookups.
struct SourceNameResolver<'typer, 'store> {
    typer: &'typer mut SourceTyper<'store>,
}

impl SourceNameResolver<'_, '_> {
    fn resolve_type_name(
        &mut self,
        name: dotty_core::Name,
        context: SourceContextId,
        location: SourceTreeLocation,
    ) -> Result<TypeId, TyperError> {
        let target =
            self.typer
                .lookup_type_symbol(name, context, location.tree_index, location.position)?;
        if let Some(target) = target {
            let prefix = self.typer.type_symbol_prefix(target);
            return Ok(self.typer.store.types.alloc(Type::TypeRef {
                prefix,
                target: TypeRefTarget::Symbol(target),
            }));
        }
        if let Some(builtin) = self
            .typer
            .definitions
            .source_builtin_type(self.typer.store, name)
        {
            return Ok(builtin);
        }
        if let Some(symbol) = self.typer.lookup_term_candidate_for_type_name(
            name,
            context,
            location.tree_index,
            location.position,
        )? {
            return Err(TyperError::WrongTypeNameKind {
                source: self.typer.source,
                tree_index: location.tree_index,
                name,
                symbol,
                kind: self.typer.store.symbols.get(symbol).kind,
                position: location.position,
            });
        }
        Err(TyperError::TypeNameNotFound {
            source: self.typer.source,
            tree_index: location.tree_index,
            name,
            position: location.position,
        })
    }

    fn resolve_type_member(
        &mut self,
        qualifier_tree: TreeId<Untyped>,
        name: dotty_core::Name,
        context: SourceContextId,
        location: SourceTreeLocation,
    ) -> Result<TypeId, TyperError> {
        self.typer.type_of_selected_tpt(
            qualifier_tree,
            name,
            context,
            location.tree_index,
            location.position,
        )
    }
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
            resolver: Box::new(NoResolver),
            type_index: SourceTypeIndex::default(),
        }
    }

    /// Uses an external semantic resolver after source and session lookup.
    ///
    /// The resolver supplies symbols already present in the semantic store;
    /// it does not perform classpath IO through the typer.
    pub fn with_resolver(mut self, resolver: Box<dyn SymbolResolver + 'a>) -> Self {
        self.resolver = resolver;
        self
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
        self.run_atomic(|typer, _| typer.type_of_tpt_inner(tree, context))
    }

    fn type_of_tpt_inner(
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
        if matches!(&source_tree.kind, TreeKind::TypeTree(_)) {
            return Err(TyperError::MissingDeclaredType {
                source: self.source,
                tree_index: tree.index(),
                position: source_tree.position,
            });
        }
        if let TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) = &source_tree.kind {
            let ty = self.type_of_tpt_inner(parens.inner, context)?;
            if let Err(existing) = self.type_index.insert(self.source, tree, ty) {
                return Err(TyperError::DuplicateSourceTypeCacheEntry {
                    source: self.source,
                    tree_index: tree.index(),
                    existing,
                    attempted: ty,
                });
            }
            return Ok(ty);
        }
        if let TreeKind::ByNameTypeTree(by_name) = &source_tree.kind {
            let result = self.type_of_tpt_inner(by_name.result, context)?;
            let ty = self.store.types.alloc(Type::ByName { result });
            if let Err(existing) = self.type_index.insert(self.source, tree, ty) {
                return Err(TyperError::DuplicateSourceTypeCacheEntry {
                    source: self.source,
                    tree_index: tree.index(),
                    existing,
                    attempted: ty,
                });
            }
            return Ok(ty);
        }
        if let TreeKind::AppliedTypeTree(applied) = &source_tree.kind {
            let tycon = self.type_of_tpt_inner(applied.tpt, context)?;
            let mut args = Vec::with_capacity(applied.args.len());
            for argument in &applied.args {
                args.push(self.type_of_tpt_inner(*argument, context)?);
            }
            let ty = self.store.types.alloc(Type::Applied { tycon, args });
            if let Err(existing) = self.type_index.insert(self.source, tree, ty) {
                return Err(TyperError::DuplicateSourceTypeCacheEntry {
                    source: self.source,
                    tree_index: tree.index(),
                    existing,
                    attempted: ty,
                });
            }
            return Ok(ty);
        }
        if let TreeKind::Select(select) = &source_tree.kind {
            let ty = SourceNameResolver { typer: self }.resolve_type_member(
                select.qualifier,
                select.name,
                context,
                SourceTreeLocation {
                    tree_index: tree.index(),
                    position: source_tree.position,
                },
            )?;
            if let Err(existing) = self.type_index.insert(self.source, tree, ty) {
                return Err(TyperError::DuplicateSourceTypeCacheEntry {
                    source: self.source,
                    tree_index: tree.index(),
                    existing,
                    attempted: ty,
                });
            }
            return Ok(ty);
        }
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
        let position = source_tree.position;
        let ty = SourceNameResolver { typer: self }.resolve_type_name(
            *name,
            context,
            SourceTreeLocation {
                tree_index: tree.index(),
                position,
            },
        )?;
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
        match self.store.symbols.get(owner).kind {
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass => {
                self.store.types.alloc(Type::ThisType { class: owner })
            }
            SymbolKind::Package => self.package_type_prefix(owner),
            _ => self.definitions.no_prefix,
        }
    }

    fn package_type_prefix(&mut self, package: SymbolId) -> TypeId {
        self.store.types.alloc(Type::TypeRef {
            prefix: self.definitions.no_prefix,
            target: TypeRefTarget::Symbol(package),
        })
    }

    fn type_of_selected_tpt(
        &mut self,
        qualifier_tree: TreeId<Untyped>,
        name: dotty_core::Name,
        context: SourceContextId,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<TypeId, TyperError> {
        if !name.is_type() {
            return Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index,
                tree_kind: "term selection",
            });
        }
        let Some(qualifier) =
            self.resolve_qualifier_symbol(qualifier_tree, context, tree_index, position)?
        else {
            return Err(TyperError::TypeNameNotFound {
                source: self.source,
                tree_index,
                name,
                position,
            });
        };
        let scope = self.scope_of(qualifier);
        if let Some(scope) = scope {
            let candidates = self.store.scopes.get(scope).lookup_all(&name);
            if let Some(symbol) =
                self.unique_type_candidate(candidates, name, tree_index, position)?
            {
                let prefix = self.type_symbol_prefix(symbol);
                return Ok(self.store.types.alloc(Type::TypeRef {
                    prefix,
                    target: TypeRefTarget::Symbol(symbol),
                }));
            }
            let term_name = dotty_core::Name::new(name.text(), dotty_core::Namespace::Term);
            if let Some(symbol) = self.unique_symbol_candidate(
                self.store.scopes.get(scope).lookup_all(&term_name),
                name,
                tree_index,
                position,
            )? {
                return Err(TyperError::WrongTypeNameKind {
                    source: self.source,
                    tree_index,
                    name,
                    symbol,
                    kind: self.store.symbols.get(symbol).kind,
                    position,
                });
            }
        }
        let prefix = self.type_prefix_for_qualifier(qualifier);
        let request = MemberRequest {
            prefix,
            name,
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        let external = self
            .resolver
            .resolve_member(self.store, &request)
            .map_err(|error| TyperError::SymbolResolution {
                source: self.source,
                tree_index,
                error,
            })?;
        if let Some(symbol) = external {
            if !self.store.symbols.contains(symbol) {
                return Err(TyperError::UnknownSymbol { symbol });
            }
            if matches!(
                self.store.symbols.get(symbol).kind,
                SymbolKind::Class
                    | SymbolKind::Trait
                    | SymbolKind::ModuleClass
                    | SymbolKind::TypeParameter
                    | SymbolKind::TypeAlias
            ) {
                let prefix = self.type_symbol_prefix(symbol);
                return Ok(self.store.types.alloc(Type::TypeRef {
                    prefix,
                    target: TypeRefTarget::Symbol(symbol),
                }));
            }
            return Err(TyperError::WrongTypeNameKind {
                source: self.source,
                tree_index,
                name,
                symbol,
                kind: self.store.symbols.get(symbol).kind,
                position,
            });
        }
        Err(TyperError::TypeNameNotFound {
            source: self.source,
            tree_index,
            name,
            position,
        })
    }

    fn resolve_qualifier_symbol(
        &mut self,
        tree: TreeId<Untyped>,
        context: SourceContextId,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        let Some(node) = self.arena.try_get(tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: tree.index(),
            });
        };
        match &node.kind {
            TreeKind::Ident(ident) => {
                if let Some(symbol) =
                    self.lookup_context_symbol(ident.name, context, tree_index, position)?
                {
                    return Ok(Some(symbol));
                }
                let segment = self.store.names.resolve(ident.name.text()).to_owned();
                self.resolve_external_package(&[segment], tree_index)
            }
            TreeKind::Select(select) => {
                let Some(qualifier) =
                    self.resolve_qualifier_symbol(select.qualifier, context, tree_index, position)?
                else {
                    return Ok(None);
                };
                let scope = self.scope_of(qualifier);
                if let Some(scope) = scope
                    && let Some(symbol) =
                        self.unique_scoped_symbol(scope, select.name, tree_index, position)?
                {
                    return Ok(Some(symbol));
                }
                if self.store.symbols.get(qualifier).kind == SymbolKind::Package {
                    let mut path = self.package_path(qualifier);
                    path.push(self.store.names.resolve(select.name.text()).to_owned());
                    if let Some(package) = self.resolve_external_package(&path, tree_index)? {
                        return Ok(Some(package));
                    }
                }
                let prefix = self.type_prefix_for_qualifier(qualifier);
                let request = MemberRequest {
                    prefix,
                    name: select.name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                };
                self.resolver
                    .resolve_member(self.store, &request)
                    .map_err(|error| TyperError::SymbolResolution {
                        source: self.source,
                        tree_index,
                        error,
                    })
            }
            _ => Err(TyperError::UnsupportedTypeTree {
                source: self.source,
                tree_index,
                tree_kind: tree_kind_name(&node.kind),
            }),
        }
    }

    fn resolve_external_package(
        &mut self,
        path: &[String],
        tree_index: u32,
    ) -> Result<Option<SymbolId>, TyperError> {
        let segments: Vec<_> = path.iter().map(String::as_str).collect();
        let package = self
            .resolver
            .resolve_package(self.store, &segments)
            .map_err(|error| TyperError::SymbolResolution {
                source: self.source,
                tree_index,
                error,
            })?;
        if let Some(symbol) = package {
            if !self.store.symbols.contains(symbol) {
                return Err(TyperError::UnknownSymbol { symbol });
            }
            if self.store.symbols.get(symbol).kind != SymbolKind::Package {
                return Err(TyperError::SymbolResolution {
                    source: self.source,
                    tree_index,
                    error: ResolutionError::Malformed {
                        reason: "package resolution returned a non-package symbol".to_owned(),
                    },
                });
            }
        }
        Ok(package)
    }

    fn package_path(&self, package: SymbolId) -> Vec<String> {
        let mut path = Vec::new();
        let mut current = Some(package);
        while let Some(symbol) = current {
            let entry = self.store.symbols.get(symbol);
            if entry.kind != SymbolKind::Package {
                break;
            }
            let name = self.store.names.resolve(entry.name.text());
            if !name.is_empty() {
                path.push(name.to_owned());
            }
            current = entry.owner;
        }
        path.reverse();
        path
    }

    fn type_prefix_for_qualifier(&mut self, symbol: SymbolId) -> TypeId {
        match self.store.symbols.get(symbol).kind {
            SymbolKind::Package => self.package_type_prefix(symbol),
            SymbolKind::Object => {
                let owner = self.store.symbols.get(symbol).owner;
                let module_class = owner
                    .and_then(|owner| self.index.definition_of(symbol).map(|_| owner))
                    .and_then(|owner| {
                        self.index
                            .definition_of(symbol)
                            .and_then(|definition| match definition {
                                SourceDefinition::Canonical { source, tree }
                                | SourceDefinition::Derived { source, tree }
                                    if source == self.source =>
                                {
                                    self.index.derived_symbol_at(owner, source, tree)
                                }
                                _ => None,
                            })
                    });
                if let Some(module_class) = module_class {
                    self.store.types.alloc(Type::ThisType {
                        class: module_class,
                    })
                } else {
                    self.definitions.no_prefix
                }
            }
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass => {
                let prefix = self.type_symbol_prefix(symbol);
                self.store.types.alloc(Type::TypeRef {
                    prefix,
                    target: TypeRefTarget::Symbol(symbol),
                })
            }
            _ => self.definitions.no_prefix,
        }
    }

    fn lookup_type_symbol(
        &self,
        name: dotty_core::Name,
        context: SourceContextId,
        tree_index: u32,
        position: Option<SourceSpan>,
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
            if let Some(symbol) =
                self.unique_type_candidate(candidates, name, tree_index, position)?
            {
                return Ok(Some(symbol));
            }
            if let Some(import_tree) = source_context.import
                && let Some(symbol) = self.lookup_imported_symbol(
                    import_tree,
                    context_id,
                    source_context.parent,
                    name,
                    true,
                    SourceTreeLocation {
                        tree_index,
                        position,
                    },
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
        type_only: bool,
        location: SourceTreeLocation,
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
        let Some(scope) = self.import_qualifier_scope(
            import.expr,
            qualifier_context,
            import_tree.index(),
            location.position,
        )?
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
                if let Some(symbol) =
                    self.unique_scoped_symbol(scope, name, location.tree_index, location.position)?
                {
                    matches.push(symbol);
                }
            }
        }
        matches.sort_by_key(|symbol| symbol.index());
        matches.dedup();
        if type_only {
            self.unique_type_candidate(&matches, wanted, location.tree_index, location.position)
        } else {
            self.unique_symbol_candidate(&matches, wanted, location.tree_index, location.position)
        }
    }

    fn import_qualifier_scope(
        &self,
        qualifier: TreeId<Untyped>,
        context: SourceContextId,
        import_tree_index: u32,
        position: Option<SourceSpan>,
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
        let mut symbol =
            self.lookup_context_symbol(root_name, context, import_tree_index, position)?;
        for selection in selections.into_iter().rev() {
            let Some(current_symbol) = symbol else {
                return Ok(None);
            };
            let Some(scope) = self.scope_of(current_symbol) else {
                return Ok(None);
            };
            symbol = self.unique_scoped_symbol(scope, selection, import_tree_index, position)?;
        }
        Ok(symbol.and_then(|symbol| self.scope_of(symbol)))
    }

    fn lookup_context_symbol(
        &self,
        name: dotty_core::Name,
        context: SourceContextId,
        import_tree_index: u32,
        position: Option<SourceSpan>,
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
            if let Some(symbol) = self.unique_scoped_symbol(
                source_context.lexical_scope,
                name,
                import_tree_index,
                position,
            )? {
                return Ok(Some(symbol));
            }
            if let Some(import_tree) = source_context.import
                && let Some(symbol) = self.lookup_imported_symbol(
                    import_tree,
                    context_id,
                    source_context.parent,
                    name,
                    false,
                    SourceTreeLocation {
                        tree_index: import_tree_index,
                        position,
                    },
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
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        let direct = self.store.scopes.get(scope).lookup_all(&name);
        if !direct.is_empty() {
            return self.unique_symbol_candidate(direct, name, tree_index, position);
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
            position,
        )
    }

    fn lookup_term_candidate_for_type_name(
        &self,
        name: dotty_core::Name,
        context: SourceContextId,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        let term_name = dotty_core::Name::new(name.text(), dotty_core::Namespace::Term);
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
                .lookup_all(&term_name);
            if !candidates.is_empty() {
                return self.unique_symbol_candidate(candidates, name, tree_index, position);
            }
            if let Some(import_tree) = source_context.import
                && let Some(symbol) = self.lookup_imported_symbol(
                    import_tree,
                    context_id,
                    source_context.parent,
                    name,
                    false,
                    SourceTreeLocation {
                        tree_index,
                        position,
                    },
                )?
                && !matches!(
                    self.store.symbols.get(symbol).kind,
                    SymbolKind::Class
                        | SymbolKind::Trait
                        | SymbolKind::ModuleClass
                        | SymbolKind::TypeParameter
                        | SymbolKind::TypeAlias
                )
            {
                return Ok(Some(symbol));
            }
            current = source_context.parent;
        }
        Ok(None)
    }

    fn unique_type_candidate(
        &self,
        candidates: &[SymbolId],
        name: dotty_core::Name,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        let type_candidates: Vec<_> = candidates
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
        if type_candidates.is_empty() {
            match candidates {
                [symbol] => {
                    let kind = self.store.symbols.get(*symbol).kind;
                    Err(TyperError::WrongTypeNameKind {
                        source: self.source,
                        tree_index,
                        name,
                        symbol: *symbol,
                        kind,
                        position,
                    })
                }
                _ => self.unique_symbol_candidate(candidates, name, tree_index, position),
            }
        } else {
            self.unique_symbol_candidate(&type_candidates, name, tree_index, position)
        }
    }

    fn unique_symbol_candidate(
        &self,
        candidates: &[SymbolId],
        name: dotty_core::Name,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        match candidates {
            [] => Ok(None),
            [symbol] => Ok(Some(*symbol)),
            _ => Err(TyperError::AmbiguousTypeName {
                source: self.source,
                tree_index,
                name,
                position,
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
        if let SymbolInfo::Complete(ty) = self.store.symbols.get(symbol).info
            && let Type::ClassInfo(info) = self.store.types.get(ty)
        {
            return Some(info.declarations);
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
        if self.store.symbols.get(module_class).kind == SymbolKind::ModuleClass {
            return self.index.scope_of(module_class);
        }
        None
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
    use std::{cell::RefCell, rc::Rc};

    struct ScriptedResolver {
        member: Option<SymbolId>,
        package: Option<SymbolId>,
        member_requests: Rc<RefCell<Vec<dotty_core::Name>>>,
        package_requests: Rc<RefCell<Vec<Vec<String>>>>,
    }

    impl SymbolResolver for ScriptedResolver {
        fn resolve_member(
            &mut self,
            _store: &SemanticStore,
            request: &MemberRequest,
        ) -> Result<Option<SymbolId>, ResolutionError> {
            self.member_requests.borrow_mut().push(request.name);
            Ok(self.member)
        }

        fn resolve_package(
            &mut self,
            _store: &SemanticStore,
            path: &[&str],
        ) -> Result<Option<SymbolId>, ResolutionError> {
            self.package_requests
                .borrow_mut()
                .push(path.iter().map(|segment| (*segment).to_owned()).collect());
            Ok(self.package)
        }
    }

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
    fn class_type_parameter_is_resolved_in_a_field_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { val value: A = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "value");
        let type_parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "A" =>
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

        assert_eq!(type_symbol(typer.store(), projected), type_parameter);
    }

    #[test]
    fn method_type_parameter_shadows_a_class_type_parameter() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Box[A] { def f[A](value: A): A = value }");
        let method_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "f" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let TreeKind::DefDef(definition) = &parsed.ast.get(method_tree).kind else {
            unreachable!()
        };
        let method_type_parameter = index.symbol_at(source, definition.type_params[0]).unwrap();
        let value_parameter = index
            .symbol_at(source, definition.value_param_clauses[0][0])
            .unwrap();
        let context = index.declaration_context_of(value_parameter).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.type_of_tpt(definition.tpt, context).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), method_type_parameter);
    }

    #[test]
    fn unresolved_type_name_error_has_position_and_rolls_back() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("val x: MissingType = 1");
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let position = parsed.ast.get(type_tree).position;
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(value),
            Err(TyperError::TypeNameNotFound {
                source: found_source,
                tree_index,
                name,
                position: found_position,
            }) if found_source == source
                && tree_index == type_tree.index()
                && found_position == position
                && typer.store().names.resolve(name.text()) == "MissingType"
        ));
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(*typer.store().symbols.info(value), SymbolInfo::Missing);
        assert_eq!(typer.source_type_index().type_at(source, type_tree), None);
    }

    #[test]
    fn duplicate_type_candidates_return_a_positioned_ambiguity_error() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class T; class T; val x: T = 1");
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let position = parsed.ast.get(type_tree).position;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.complete_symbol(value),
            Err(TyperError::AmbiguousTypeName {
                source: found_source,
                tree_index,
                name,
                position: found_position,
            }) if found_source == source
                && tree_index == type_tree.index()
                && found_position == position
                && typer.store().names.resolve(name.text()) == "T"
        ));
    }

    #[test]
    fn object_name_alone_does_not_resolve_as_a_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("object O\nval x: O = 1");
        let (symbol, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let object = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                matches!(
                    node.kind,
                    TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(_))
                )
                .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap();
        let position = parsed.ast.get(type_tree).position;
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
            Err(TyperError::WrongTypeNameKind {
                source: found_source,
                tree_index,
                name,
                symbol: found_symbol,
                kind: SymbolKind::Object,
                position: found_position,
            }) if found_source == source
                && tree_index == type_tree.index()
                && found_symbol == object
                && found_position == position
                && typer.store().names.resolve(name.text()) == "O"
                && name.is_type()
        ));
    }

    #[test]
    fn imported_object_alias_does_not_resolve_as_a_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("object Lib { object O }\nimport Lib.O as Alias\nval x: Alias = 1");
        let (symbol, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let object = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(module)) =
                    &node.kind
                else {
                    return None;
                };
                (store.names.resolve(module.name.as_name().text()) == "O")
                    .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap();
        let position = parsed.ast.get(type_tree).position;
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
            Err(TyperError::WrongTypeNameKind {
                source: found_source,
                tree_index,
                name,
                symbol: found_symbol,
                kind: SymbolKind::Object,
                position: found_position,
            }) if found_source == source
                && tree_index == type_tree.index()
                && found_symbol == object
                && found_position == position
                && typer.store().names.resolve(name.text()) == "Alias"
                && name.is_type()
        ));
    }

    #[test]
    fn parenthesized_type_projects_the_inner_type_and_caches_both_trees() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("val x: (Int) = 1");
        let (symbol, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::Parens(parens)) =
            &parsed.ast.get(type_tree).kind
        else {
            panic!("expected the parser to preserve parentheses around the type")
        };
        let inner = parens.inner;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(symbol).unwrap();

        assert_eq!(projected, definitions.int);
        assert_eq!(
            typer.source_type_index().type_at(source, type_tree),
            Some(projected)
        );
        assert_eq!(
            typer.source_type_index().type_at(source, inner),
            Some(projected)
        );
    }

    #[test]
    fn applied_type_projects_constructor_and_arguments_in_source_order() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class F[A, B]; class A; class B; val x: F[A, B] = 1");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let mut expected = Vec::new();
        for (tree, node) in parsed.ast.iter() {
            let TreeKind::TypeDef(definition) = &node.kind else {
                continue;
            };
            let name = store.names.resolve(definition.name.as_name().text());
            if matches!(name, "F" | "A" | "B") {
                let symbol = index.symbol_at(source, tree).unwrap();
                if store.symbols.get(symbol).kind == SymbolKind::Class {
                    expected.push((name.to_owned(), symbol));
                }
            }
        }
        let find_symbol = |name| expected.iter().find(|(found, _)| found == name).unwrap().1;
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        let Type::Applied { tycon, args } = typer.store().types.get(projected) else {
            panic!("expected an applied type")
        };
        assert_eq!(type_symbol(typer.store(), *tycon), find_symbol("F"));
        assert_eq!(args.len(), 2);
        assert_eq!(type_symbol(typer.store(), args[0]), find_symbol("A"));
        assert_eq!(type_symbol(typer.store(), args[1]), find_symbol("B"));
    }

    #[test]
    fn by_name_type_projects_its_result_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("def f(x: => Int): Unit = ()");
        let (parameter, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let context = index.declaration_context_of(parameter).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.type_of_tpt(type_tree, context).unwrap();

        let Type::ByName { result } = typer.store().types.get(projected) else {
            panic!("expected a by-name type")
        };
        assert_eq!(*result, definitions.int);
    }

    #[test]
    fn failed_applied_type_projection_rolls_back_types_and_cache_entries() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class F[A]; class A; val x: F[A, Missing] = 1");
        let (value, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let TreeKind::AppliedTypeTree(applied) = &parsed.ast.get(type_tree).kind else {
            panic!("expected an applied type tree")
        };
        let constructor = applied.tpt;
        let first_argument = applied.args[0];
        let context = index.declaration_context_of(value).unwrap();
        let before = store.checkpoint();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert!(matches!(
            typer.type_of_tpt(type_tree, context),
            Err(TyperError::TypeNameNotFound { name, .. })
                if typer.store().names.resolve(name.text()) == "Missing"
        ));

        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(typer.source_type_index().type_at(source, type_tree), None);
        assert_eq!(typer.source_type_index().type_at(source, constructor), None);
        assert_eq!(
            typer.source_type_index().type_at(source, first_argument),
            None
        );
    }

    #[test]
    fn missing_declared_type_is_not_inferred_from_the_value_rhs() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name("val x = 1");
        let (symbol, type_tree) = val_symbol(&parsed, &store, &index, source, "x");
        let position = parsed.ast.get(type_tree).position;
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
            Err(TyperError::MissingDeclaredType {
                source: found_source,
                tree_index,
                position: found_position,
            }) if found_source == source
                && tree_index == type_tree.index()
                && found_position == position
        ));
        assert_eq!(typer.source_type_index().type_at(source, type_tree), None);
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
    fn qualified_inner_type_resolves_through_source_class_scope() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class Outer { class Inner }; val x: Outer.Inner = 1");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let mut outer = None;
        let mut inner = None;
        for (tree, node) in parsed.ast.iter() {
            if let TreeKind::TypeDef(definition) = &node.kind {
                match store.names.resolve(definition.name.as_name().text()) {
                    "Outer" => outer = index.symbol_at(source, tree),
                    "Inner" => inner = index.symbol_at(source, tree),
                    _ => {}
                }
            }
        }
        let outer = outer.unwrap();
        let inner = inner.unwrap();
        assert_eq!(*store.symbols.info(outer), SymbolInfo::Missing);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), inner);
        let Type::TypeRef { prefix, .. } = typer.store().types.get(projected) else {
            panic!("expected a qualified type reference")
        };
        assert_eq!(
            typer.store().types.get(*prefix),
            &Type::ThisType { class: outer }
        );
        assert_eq!(*typer.store().symbols.info(outer), SymbolInfo::Missing);
    }

    #[test]
    fn package_qualified_type_uses_the_canonical_package_prefix() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("package p { class C }; package use { val x: p.C = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let class = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "C" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let package = packages.symbol(&["p"]).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), class);
        let Type::TypeRef { prefix, .. } = typer.store().types.get(projected) else {
            panic!("expected a package-qualified type reference")
        };
        let Type::TypeRef {
            prefix: package_prefix,
            target: TypeRefTarget::Symbol(package_target),
        } = typer.store().types.get(*prefix)
        else {
            panic!("expected the canonical package reference prefix")
        };
        assert_eq!(*package_prefix, definitions.no_prefix);
        assert_eq!(*package_target, package);
    }

    #[test]
    fn external_member_resolver_runs_only_after_source_lookup_misses() {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(
            "class Outer { class Local }; val local: Outer.Local = 1; val ext: Outer.External = 1",
        );
        let (local, _) = val_symbol(&parsed, &store, &index, source, "local");
        let (external, _) = val_symbol(&parsed, &store, &index, source, "ext");
        let local_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Local" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let external_type = symbol(&mut store, SymbolKind::Class, SymbolInfo::Missing);
        let requests = Rc::new(RefCell::new(Vec::new()));
        let package_requests = Rc::new(RefCell::new(Vec::new()));
        let resolver = ScriptedResolver {
            member: Some(external_type),
            package: None,
            member_requests: Rc::clone(&requests),
            package_requests,
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        )
        .with_resolver(Box::new(resolver));

        let local_projected = typer.complete_symbol(local).unwrap();
        let external_projected = typer.complete_symbol(external).unwrap();

        assert_eq!(type_symbol(typer.store(), local_projected), local_type);
        assert_eq!(
            type_symbol(typer.store(), external_projected),
            external_type
        );
        assert_eq!(requests.borrow().len(), 1);
        assert_eq!(
            typer.store().names.resolve(requests.borrow()[0].text()),
            "External"
        );
    }

    #[test]
    fn external_package_and_member_resolvers_supply_unknown_qualifiers() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("val x: p.C = 1");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let package_name = store.names.intern("p");
        let package = store.symbols.alloc(dotty_core::Symbol {
            name: Name::new(package_name, Namespace::Term),
            owner: None,
            kind: SymbolKind::Package,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let external_type = symbol(&mut store, SymbolKind::Class, SymbolInfo::Missing);
        store.symbols.get_mut(external_type).owner = Some(package);
        let member_requests = Rc::new(RefCell::new(Vec::new()));
        let package_requests = Rc::new(RefCell::new(Vec::new()));
        let resolver = ScriptedResolver {
            member: Some(external_type),
            package: Some(package),
            member_requests: Rc::clone(&member_requests),
            package_requests: Rc::clone(&package_requests),
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        )
        .with_resolver(Box::new(resolver));

        let projected = typer.complete_symbol(value).unwrap();

        assert_eq!(type_symbol(typer.store(), projected), external_type);
        assert_eq!(*package_requests.borrow(), vec![vec!["p".to_owned()]]);
        assert_eq!(member_requests.borrow().len(), 1);
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(found_package),
            ..
        } = typer
            .store()
            .types
            .get(match typer.store().types.get(projected) {
                Type::TypeRef { prefix, .. } => *prefix,
                _ => panic!("expected a type reference"),
            })
        else {
            panic!("expected the external package prefix")
        };
        assert_eq!(*found_package, package);
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
    fn inner_term_name_does_not_shadow_an_enclosing_type_name() {
        let (parsed, mut store, packages, definitions, index, source) =
            parse_and_name("class T; class Outer { object T; val x: T = 1 }");
        let (value, _) = val_symbol(&parsed, &store, &index, source, "x");
        let enclosing_type = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "T" =>
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

        assert_eq!(type_symbol(typer.store(), projected), enclosing_type);
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
    fn primitive_boolean_annotation_uses_the_bootstrapped_type() {
        let (ty, definitions) = complete_builtin_annotation("Boolean");

        assert_eq!(ty, definitions.boolean);
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
