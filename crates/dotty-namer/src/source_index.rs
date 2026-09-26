//! Source-tree to semantic identity mappings produced by naming.

use std::collections::HashMap;

use dotty_core::{Packages, ScopeId, SemanticStore, SourceId, SymbolId, TreeId, Untyped};

use crate::NamerError;

/// Source tree that introduced a semantic identity.
///
/// Derived identities, such as constructor parameter copies, point back to
/// the original source tree while remaining distinct from its canonical
/// symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceDefinition {
    /// The canonical identity represented by a source definition tree.
    Canonical {
        /// Compilation source containing the tree.
        source: SourceId,
        /// Source tree that owns the canonical identity.
        tree: TreeId<Untyped>,
    },
    /// An owner-specific semantic identity derived from a source tree.
    Derived {
        /// Compilation source containing the original tree.
        source: SourceId,
        /// Original source tree from which the identity was derived.
        tree: TreeId<Untyped>,
    },
}

/// Opaque identifier for an immutable source declaration context.
///
/// IDs are meaningful only in the [`SourceSemanticIndex`] that allocated
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceContextId(u32);

impl SourceContextId {
    /// Returns the arena index backing this context identity.
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// Lexical source environment captured for a declaration site.
///
/// `import` is the unresolved source import that transitions from `parent` to
/// this context. A context with no import is a base owner/scope environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceContext {
    /// Source lexical owner, independent of a declaration's semantic owner.
    pub owner: SymbolId,
    /// Lexical scope active at this source site.
    pub lexical_scope: ScopeId,
    /// Enclosing source environment inherited by this context.
    pub parent: Option<SourceContextId>,
    /// Unresolved import tree applied after the parent context, if any.
    pub import: Option<TreeId<Untyped>>,
}

/// Canonical and owner-specific derived identities associated with source
/// trees in compilation units.
///
/// Tree IDs are arena-relative, so lookups use both the source identity and
/// the tree ID. This index belongs to one naming result; persistent symbols
/// and scopes remain in [`dotty_core::SemanticStore`].
#[derive(Debug, Default)]
pub struct SourceSemanticIndex {
    symbols_by_tree: HashMap<(SourceId, TreeId<Untyped>), SymbolId>,
    derived_symbols_by_owner_and_tree: HashMap<(SymbolId, SourceId, TreeId<Untyped>), SymbolId>,
    definitions_by_symbol: HashMap<SymbolId, SourceDefinition>,
    scopes_by_owner: HashMap<SymbolId, ScopeId>,
    extension_prefix_clauses_by_method: HashMap<SymbolId, Vec<Vec<TreeId<Untyped>>>>,
    source_contexts: Vec<SourceContext>,
    declaration_contexts_by_symbol: HashMap<SymbolId, SourceContextId>,
}

impl SourceSemanticIndex {
    /// Creates an empty source-side semantic index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the canonical symbol represented by a tree in the given source
    /// unit. Derived symbols are available through [`Self::derived_symbol_at`]
    /// and never replace this mapping.
    pub fn symbol_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<SymbolId> {
        self.symbols_by_tree.get(&(source, tree)).copied()
    }

    /// Returns the derived symbol assigned to `tree` for the given `owner`.
    ///
    /// Derived identities are kept separate from [`Self::symbol_at`], which
    /// continues to return only the canonical identity represented by a
    /// source tree.
    pub fn derived_symbol_at(
        &self,
        owner: SymbolId,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.derived_symbols_by_owner_and_tree
            .get(&(owner, source, tree))
            .copied()
    }

    /// Returns the source definition tree for a canonical or derived symbol.
    ///
    /// Lookup is O(1). Package symbols are shared across source clauses and
    /// intentionally have no authoritative reverse provenance. Purely
    /// synthetic symbols also return `None`.
    pub fn definition_of(&self, symbol: SymbolId) -> Option<SourceDefinition> {
        self.definitions_by_symbol.get(&symbol).copied()
    }

    /// Associates a derived declaration identity with its semantic owner.
    ///
    /// The same source tree may have derived identities for different owners,
    /// but a given `(owner, source, tree)` key can only be recorded once.
    /// Duplicate insertion is an internal naming error and leaves the
    /// existing mapping intact.
    pub fn record_derived_symbol(
        &mut self,
        owner: SymbolId,
        source: SourceId,
        tree: TreeId<Untyped>,
        symbol: SymbolId,
    ) -> Result<(), NamerError> {
        let key = (owner, source, tree);
        if self.derived_symbols_by_owner_and_tree.contains_key(&key) {
            return Err(NamerError::DuplicateDerivedSourceTreeSymbol {
                owner,
                source,
                tree_index: tree.index(),
            });
        }
        let definition = SourceDefinition::Derived { source, tree };
        self.ensure_source_provenance(symbol, definition)?;
        self.derived_symbols_by_owner_and_tree.insert(key, symbol);
        self.definitions_by_symbol.insert(symbol, definition);
        Ok(())
    }

    /// Returns the declaration scope owned by `symbol`, if one was indexed.
    pub fn scope_of(&self, symbol: SymbolId) -> Option<ScopeId> {
        self.scopes_by_owner.get(&symbol).copied()
    }

    /// Returns the original source prefix clauses for an extension method.
    ///
    /// The returned clauses preserve their source order and point to the
    /// original parameter trees. Ordinary methods return `None`.
    pub fn extension_prefix_clauses(&self, method: SymbolId) -> Option<&[Vec<TreeId<Untyped>>]> {
        self.extension_prefix_clauses_by_method
            .get(&method)
            .map(Vec::as_slice)
    }

    /// Returns an immutable source context owned by this index.
    ///
    /// An out-of-range ID is an internal arena invariant violation and
    /// panics, like other arena IDs. IDs are index-relative and do not carry
    /// their originating index, so passing an ID from another index that has
    /// the same numeric slot accesses this index's context at that slot; that
    /// is a caller logic error and cannot be detected here.
    pub fn source_context(&self, id: SourceContextId) -> &SourceContext {
        self.source_contexts
            .get(id.index() as usize)
            .expect("SourceContextId must be in range for this SourceSemanticIndex")
    }

    /// Returns the source declaration context recorded for `symbol`.
    pub fn declaration_context_of(&self, symbol: SymbolId) -> Option<SourceContextId> {
        self.declaration_contexts_by_symbol.get(&symbol).copied()
    }

    /// Checks the source index and the semantic objects it refers to.
    ///
    /// This is intended for tests, corpus audits, and debug tooling. Naming
    /// does not call it automatically, so production source naming does not
    /// pay for a second graph traversal.
    pub fn validate(&self, store: &SemanticStore, packages: &Packages) -> Vec<String> {
        let mut violations = Vec::new();
        for ((source, tree), symbol) in &self.symbols_by_tree {
            if !store.symbols.contains(*symbol) {
                violations.push(format!(
                    "canonical source {}/tree {} refers to missing symbol {}",
                    source.index(),
                    tree.index(),
                    symbol.index()
                ));
                continue;
            }
            let semantic = store.symbols.get(*symbol);
            if matches!(
                semantic.kind,
                dotty_core::SymbolKind::Class
                    | dotty_core::SymbolKind::Trait
                    | dotty_core::SymbolKind::ModuleClass
            ) && !self.scopes_by_owner.contains_key(symbol)
            {
                violations.push(format!(
                    "class-like symbol {} has no indexed declaration scope",
                    symbol.index()
                ));
            }
            if semantic.kind == dotty_core::SymbolKind::Package {
                let indexed_scope = self.scopes_by_owner.get(symbol).copied();
                match (packages.scope_of(*symbol), indexed_scope) {
                    (Some(package_scope), Some(indexed_scope))
                        if package_scope == indexed_scope => {}
                    (registered_scope, indexed_scope) => {
                        violations.push(format!(
                            "package symbol {} does not reuse its Packages scope (registry: {:?}, index: {:?})",
                            symbol.index(),
                            registered_scope.map(ScopeId::index),
                            indexed_scope.map(ScopeId::index)
                        ));
                    }
                }
            }
        }

        for ((owner, source, tree), symbol) in &self.derived_symbols_by_owner_and_tree {
            if !store.symbols.contains(*owner) {
                violations.push(format!(
                    "derived source {}/tree {} has missing owner {}",
                    source.index(),
                    tree.index(),
                    owner.index()
                ));
            }
            if !store.symbols.contains(*symbol) {
                violations.push(format!(
                    "derived source {}/tree {} for owner {} refers to missing symbol {}",
                    source.index(),
                    tree.index(),
                    owner.index(),
                    symbol.index()
                ));
            }
            if self.symbols_by_tree.get(&(*source, *tree)) == Some(symbol) {
                violations.push(format!(
                    "derived source {}/tree {} overwrites its canonical identity",
                    source.index(),
                    tree.index()
                ));
            }
        }

        for (owner, scope) in &self.scopes_by_owner {
            if !store.symbols.contains(*owner) {
                violations.push(format!("scope owner {} is missing", owner.index()));
                continue;
            }
            if !store.scopes.contains(*scope) {
                violations.push(format!(
                    "scope {} indexed for owner {} is missing",
                    scope.index(),
                    owner.index()
                ));
                continue;
            }
            let scope_data = store.scopes.get(*scope);
            if scope_data.owner != Some(*owner) {
                violations.push(format!(
                    "scope {} indexed for owner {} declares owner {:?}",
                    scope.index(),
                    owner.index(),
                    scope_data.owner
                ));
            }
            for member in scope_data.entered_symbols() {
                if !store.symbols.contains(member) {
                    violations.push(format!(
                        "scope {} contains missing symbol {}",
                        scope.index(),
                        member.index()
                    ));
                } else if scope_data
                    .owner
                    .is_some_and(|scope_owner| store.symbols.get(member).owner != Some(scope_owner))
                {
                    violations.push(format!(
                        "symbol {} entered in scope {} is not owned by scope owner {}",
                        member.index(),
                        scope.index(),
                        scope_data.owner.unwrap().index()
                    ));
                }
            }
        }

        for context in &self.source_contexts {
            if !store.symbols.contains(context.owner) {
                violations.push(format!(
                    "source context owner {} is missing",
                    context.owner.index()
                ));
            }
            if !store.scopes.contains(context.lexical_scope) {
                violations.push(format!(
                    "source context scope {} is missing",
                    context.lexical_scope.index()
                ));
            } else if store.scopes.get(context.lexical_scope).owner != Some(context.owner) {
                violations.push(format!(
                    "source context scope {} is not owned by source context owner {}",
                    context.lexical_scope.index(),
                    context.owner.index()
                ));
            }
            if let Some(parent) = context.parent
                && self.source_contexts.get(parent.index() as usize).is_none()
            {
                violations.push(format!(
                    "source context refers to missing parent context {}",
                    parent.index()
                ));
            }
        }

        for (symbol, context) in &self.declaration_contexts_by_symbol {
            if !store.symbols.contains(*symbol) {
                violations.push(format!(
                    "declaration context refers to missing symbol {}",
                    symbol.index()
                ));
            }
            if self.source_contexts.get(context.index() as usize).is_none() {
                violations.push(format!(
                    "declaration symbol {} refers to missing source context {}",
                    symbol.index(),
                    context.index()
                ));
            }
        }

        violations
    }

    /// Records one immutable source context node and returns its index-local
    /// identity.
    pub(crate) fn alloc_source_context(&mut self, context: SourceContext) -> SourceContextId {
        let id = SourceContextId(
            u32::try_from(self.source_contexts.len())
                .unwrap_or_else(|_| panic!("source context arena exceeded u32::MAX entries")),
        );
        self.source_contexts.push(context);
        id
    }

    /// Associates a declaration symbol with its source lexical context.
    ///
    /// Repeating the same association is idempotent. Assigning a different
    /// context to the same symbol is an internal naming error and preserves
    /// the original association.
    pub(crate) fn record_declaration_context(
        &mut self,
        symbol: SymbolId,
        context: SourceContextId,
    ) -> Result<(), NamerError> {
        match self.declaration_contexts_by_symbol.get(&symbol) {
            Some(existing) if *existing == context => Ok(()),
            Some(existing) => Err(NamerError::DuplicateDeclarationContext {
                symbol,
                existing: *existing,
                attempted: context,
            }),
            None => {
                self.declaration_contexts_by_symbol.insert(symbol, context);
                Ok(())
            }
        }
    }

    /// Records the original extension prefix clauses for one method.
    ///
    /// Metadata is source structure, not a semantic parameter list. A method
    /// can only be registered once; duplicate registration leaves the first
    /// value intact.
    pub fn record_extension_prefix_clauses(
        &mut self,
        method: SymbolId,
        clauses: &[Vec<TreeId<Untyped>>],
    ) -> Result<(), NamerError> {
        if self
            .extension_prefix_clauses_by_method
            .contains_key(&method)
        {
            return Err(NamerError::DuplicateExtensionPrefixClauses { method });
        }
        self.extension_prefix_clauses_by_method
            .insert(method, clauses.to_vec());
        Ok(())
    }

    /// Associates a definition tree with its symbol.
    ///
    /// A source tree can own at most one symbol. Duplicate insertion is an
    /// internal naming error and leaves the existing mapping intact.
    pub fn record_symbol(
        &mut self,
        source: SourceId,
        tree: TreeId<Untyped>,
        symbol: SymbolId,
    ) -> Result<(), NamerError> {
        let key = (source, tree);
        if self.symbols_by_tree.contains_key(&key) {
            return Err(NamerError::DuplicateSourceTreeSymbol {
                source,
                tree_index: tree.index(),
            });
        }
        let definition = SourceDefinition::Canonical { source, tree };
        self.ensure_source_provenance(symbol, definition)?;
        self.symbols_by_tree.insert(key, symbol);
        self.definitions_by_symbol.insert(symbol, definition);
        Ok(())
    }

    /// Records a shared package identity without assigning it reverse source
    /// provenance. Package symbols may be reached from multiple package
    /// clauses and compilation units, so no one clause is authoritative.
    pub(crate) fn record_package_symbol(
        &mut self,
        source: SourceId,
        tree: TreeId<Untyped>,
        symbol: SymbolId,
    ) -> Result<(), NamerError> {
        let key = (source, tree);
        if self.symbols_by_tree.contains_key(&key) {
            return Err(NamerError::DuplicateSourceTreeSymbol {
                source,
                tree_index: tree.index(),
            });
        }
        self.symbols_by_tree.insert(key, symbol);
        Ok(())
    }

    fn ensure_source_provenance(
        &self,
        symbol: SymbolId,
        attempted: SourceDefinition,
    ) -> Result<(), NamerError> {
        if let Some(existing) = self.definitions_by_symbol.get(&symbol)
            && *existing != attempted
        {
            return Err(NamerError::ConflictingSourceProvenance {
                symbol,
                existing: *existing,
                attempted,
            });
        }
        Ok(())
    }

    /// Associates a declaration scope with its semantic owner.
    ///
    /// Duplicate scope registration is an internal naming error and leaves
    /// the existing mapping intact.
    pub fn record_scope(&mut self, symbol: SymbolId, scope: ScopeId) -> Result<(), NamerError> {
        match self.scopes_by_owner.get(&symbol) {
            Some(existing) if *existing == scope => Ok(()),
            Some(_) => Err(NamerError::DuplicateDeclarationScope { symbol }),
            None => {
                self.scopes_by_owner.insert(symbol, scope);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use dotty_core::ast::{ErrorNode, ErrorNodeKind, UntypedNode};
    use dotty_core::{
        Scope, SemanticStore, SourceId, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks,
        SymbolOrigin, Tree, TreeKind, Untyped, Visibility,
    };

    use super::*;

    fn symbol(store: &mut SemanticStore) -> SymbolId {
        let name_id = store.names.intern("test");
        let name = *dotty_core::TermName::new(name_id).as_name();
        store.symbols.alloc(Symbol {
            name,
            owner: None,
            kind: SymbolKind::Value,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        })
    }

    fn tree_id() -> TreeId<Untyped> {
        let mut arena = dotty_core::AstArena::<Untyped>::new();
        arena.alloc(Tree {
            kind: TreeKind::PhaseSpecific(UntypedNode::Error(ErrorNode {
                kind: ErrorNodeKind::UnexpectedToken,
            })),
            position: None,
            ty: (),
        })
    }

    #[test]
    fn symbol_identity_distinguishes_source_units_with_the_same_tree_id() {
        let mut store = SemanticStore::new();
        let first = symbol(&mut store);
        let second = symbol(&mut store);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index
            .record_symbol(SourceId::from_index(1), tree, first)
            .unwrap();
        index
            .record_symbol(SourceId::from_index(2), tree, second)
            .unwrap();

        assert_eq!(index.symbol_at(SourceId::from_index(1), tree), Some(first));
        assert_eq!(index.symbol_at(SourceId::from_index(2), tree), Some(second));
    }

    #[test]
    fn canonical_symbol_records_reverse_source_definition() {
        let mut store = SemanticStore::new();
        let symbol = symbol(&mut store);
        let source = SourceId::from_index(3);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index.record_symbol(source, tree, symbol).unwrap();

        assert_eq!(index.symbol_at(source, tree), Some(symbol));
        assert_eq!(
            index.definition_of(symbol),
            Some(SourceDefinition::Canonical { source, tree })
        );
    }

    #[test]
    fn conflicting_canonical_provenance_is_rejected_without_partial_mapping() {
        let mut store = SemanticStore::new();
        let symbol = symbol(&mut store);
        let first_source = SourceId::from_index(4);
        let attempted_source = SourceId::from_index(5);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();
        index.record_symbol(first_source, tree, symbol).unwrap();

        assert_eq!(
            index.record_symbol(attempted_source, tree, symbol),
            Err(NamerError::ConflictingSourceProvenance {
                symbol,
                existing: SourceDefinition::Canonical {
                    source: first_source,
                    tree,
                },
                attempted: SourceDefinition::Canonical {
                    source: attempted_source,
                    tree,
                },
            })
        );
        assert_eq!(index.symbol_at(first_source, tree), Some(symbol));
        assert_eq!(index.symbol_at(attempted_source, tree), None);
        assert_eq!(
            index.definition_of(symbol),
            Some(SourceDefinition::Canonical {
                source: first_source,
                tree,
            })
        );
    }

    #[test]
    fn derived_symbol_records_original_source_definition() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let symbol = symbol(&mut store);
        let source = SourceId::from_index(6);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index
            .record_derived_symbol(owner, source, tree, symbol)
            .unwrap();

        assert_eq!(index.derived_symbol_at(owner, source, tree), Some(symbol));
        assert_eq!(
            index.definition_of(symbol),
            Some(SourceDefinition::Derived { source, tree })
        );
    }

    #[test]
    fn canonical_symbol_cannot_also_be_recorded_as_derived() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let symbol = symbol(&mut store);
        let source = SourceId::from_index(7);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();
        index.record_symbol(source, tree, symbol).unwrap();

        assert_eq!(
            index.record_derived_symbol(owner, source, tree, symbol),
            Err(NamerError::ConflictingSourceProvenance {
                symbol,
                existing: SourceDefinition::Canonical { source, tree },
                attempted: SourceDefinition::Derived { source, tree },
            })
        );
        assert_eq!(index.derived_symbol_at(owner, source, tree), None);
        assert_eq!(
            index.definition_of(symbol),
            Some(SourceDefinition::Canonical { source, tree })
        );
    }

    #[test]
    fn shared_package_mappings_keep_forward_lookup_and_skip_reverse_provenance() {
        let mut store = SemanticStore::new();
        let package = symbol(&mut store);
        let first_source = SourceId::from_index(8);
        let second_source = SourceId::from_index(9);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index
            .record_package_symbol(first_source, tree, package)
            .unwrap();
        index
            .record_package_symbol(second_source, tree, package)
            .unwrap();

        assert_eq!(index.symbol_at(first_source, tree), Some(package));
        assert_eq!(index.symbol_at(second_source, tree), Some(package));
        assert_eq!(index.definition_of(package), None);
    }

    #[test]
    fn source_contexts_are_stored_and_retrievable_by_opaque_id() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let lexical_scope = store.scopes.alloc(Scope::new(Some(owner)));
        let mut index = SourceSemanticIndex::new();
        let context = SourceContext {
            owner,
            lexical_scope,
            parent: None,
            import: None,
        };

        let id = index.alloc_source_context(context);

        assert_eq!(id.index(), 0);
        assert_eq!(index.source_context(id), &context);
    }

    #[test]
    #[should_panic(expected = "SourceContextId must be in range for this SourceSemanticIndex")]
    fn looking_up_an_out_of_range_context_is_an_internal_invariant_violation() {
        let index = SourceSemanticIndex::new();
        let _ = index.source_context(SourceContextId(0));
    }

    #[test]
    fn a_symbol_gets_one_declaration_context_and_repeated_same_context_is_idempotent() {
        let mut store = SemanticStore::new();
        let declaration = symbol(&mut store);
        let owner = symbol(&mut store);
        let lexical_scope = store.scopes.alloc(Scope::new(Some(owner)));
        let mut index = SourceSemanticIndex::new();
        let context = index.alloc_source_context(SourceContext {
            owner,
            lexical_scope,
            parent: None,
            import: None,
        });

        index
            .record_declaration_context(declaration, context)
            .unwrap();
        index
            .record_declaration_context(declaration, context)
            .unwrap();

        assert_eq!(index.declaration_context_of(declaration), Some(context));
    }

    #[test]
    fn validation_accepts_a_well_formed_symbol_scope_and_context_graph() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let scope = store.scopes.alloc(Scope::new(Some(owner)));
        let member_name = store.names.intern("member");
        let member = store.symbols.alloc(Symbol {
            name: *dotty_core::TermName::new(member_name).as_name(),
            owner: Some(owner),
            kind: SymbolKind::Value,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        store
            .scopes
            .get_mut(scope)
            .enter(store.symbols.get(member).name, member);
        let mut index = SourceSemanticIndex::new();
        let tree = tree_id();
        index
            .record_symbol(SourceId::from_index(1), tree, member)
            .unwrap();
        index.record_scope(owner, scope).unwrap();
        let context = index.alloc_source_context(SourceContext {
            owner,
            lexical_scope: scope,
            parent: None,
            import: None,
        });
        index.record_declaration_context(member, context).unwrap();

        assert!(index.validate(&store, &Packages::new()).is_empty());
    }

    #[test]
    fn validation_reports_scope_owner_and_member_owner_mismatches() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let other_owner = symbol(&mut store);
        let scope = store.scopes.alloc(Scope::new(Some(other_owner)));
        let member = symbol(&mut store);
        store
            .scopes
            .get_mut(scope)
            .enter(store.symbols.get(member).name, member);
        let mut index = SourceSemanticIndex::new();
        index.record_scope(owner, scope).unwrap();

        let violations = index.validate(&store, &Packages::new());

        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("declares owner"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("is not owned by scope owner"))
        );
    }

    #[test]
    fn validation_requires_class_like_symbols_to_have_a_declaration_scope() {
        let mut store = SemanticStore::new();
        let class_name = store.names.intern("C");
        let class = store.symbols.alloc(Symbol {
            name: *dotty_core::TypeName::new(class_name).as_name(),
            owner: None,
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let mut index = SourceSemanticIndex::new();
        index
            .record_symbol(SourceId::from_index(1), tree_id(), class)
            .unwrap();

        let violations = index.validate(&store, &Packages::new());

        assert!(violations.iter().any(|violation| {
            violation.contains("class-like symbol")
                && violation.contains("no indexed declaration scope")
        }));
    }

    #[test]
    fn validation_requires_package_scopes_to_come_from_the_shared_registry() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();
        let package = packages.enter(&mut store, SymbolOrigin::Synthetic, &["p"])[0];
        let unrelated_scope = store.scopes.alloc(Scope::new(Some(package.symbol)));
        let mut index = SourceSemanticIndex::new();
        index
            .record_symbol(SourceId::from_index(1), tree_id(), package.symbol)
            .unwrap();
        index.record_scope(package.symbol, unrelated_scope).unwrap();

        let violations = index.validate(&store, &packages);

        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("does not reuse its Packages scope"))
        );
    }

    #[test]
    fn validation_rejects_unregistered_package_symbols_without_scopes() {
        let mut store = SemanticStore::new();
        let package_name = store.names.intern("p");
        let package = store.symbols.alloc(Symbol {
            name: *dotty_core::TypeName::new(package_name).as_name(),
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
        let mut index = SourceSemanticIndex::new();
        index
            .record_symbol(SourceId::from_index(1), tree_id(), package)
            .unwrap();

        let violations = index.validate(&store, &Packages::new());

        assert!(
            violations
                .iter()
                .any(|violation| { violation.contains("does not reuse its Packages scope") })
        );
    }

    #[test]
    fn validation_rejects_derived_identities_with_missing_owners() {
        let mut store = SemanticStore::new();
        let checkpoint = store.checkpoint();
        let _filler = symbol(&mut store);
        let owner = symbol(&mut store);
        store.rollback_to(checkpoint);
        let derived = symbol(&mut store);
        let mut index = SourceSemanticIndex::new();
        index
            .record_derived_symbol(owner, SourceId::from_index(1), tree_id(), derived)
            .unwrap();

        let violations = index.validate(&store, &Packages::new());

        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("has missing owner 1"))
        );
    }

    #[test]
    fn assigning_a_different_declaration_context_is_rejected_without_replacement() {
        let mut store = SemanticStore::new();
        let declaration = symbol(&mut store);
        let first_owner = symbol(&mut store);
        let second_owner = symbol(&mut store);
        let first_scope = store.scopes.alloc(Scope::new(Some(first_owner)));
        let second_scope = store.scopes.alloc(Scope::new(Some(second_owner)));
        let mut index = SourceSemanticIndex::new();
        let first = index.alloc_source_context(SourceContext {
            owner: first_owner,
            lexical_scope: first_scope,
            parent: None,
            import: None,
        });
        let second = index.alloc_source_context(SourceContext {
            owner: second_owner,
            lexical_scope: second_scope,
            parent: Some(first),
            import: None,
        });
        index
            .record_declaration_context(declaration, first)
            .unwrap();

        assert_eq!(
            index.record_declaration_context(declaration, second),
            Err(NamerError::DuplicateDeclarationContext {
                symbol: declaration,
                existing: first,
                attempted: second,
            })
        );
        assert_eq!(index.declaration_context_of(declaration), Some(first));
    }

    #[test]
    fn duplicate_source_tree_symbol_insertion_is_rejected_without_replacement() {
        let mut store = SemanticStore::new();
        let first = symbol(&mut store);
        let second = symbol(&mut store);
        let source = SourceId::from_index(1);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index.record_symbol(source, tree, first).unwrap();
        assert!(matches!(
            index.record_symbol(source, tree, second),
            Err(NamerError::DuplicateSourceTreeSymbol {
                source: duplicate_source,
                tree_index: 0
            }) if duplicate_source == source
        ));
        assert_eq!(index.symbol_at(source, tree), Some(first));
    }

    #[test]
    fn duplicate_derived_source_tree_symbol_is_rejected_without_replacement() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let first = symbol(&mut store);
        let second = symbol(&mut store);
        let source = SourceId::from_index(1);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index
            .record_derived_symbol(owner, source, tree, first)
            .unwrap();
        assert!(matches!(
            index.record_derived_symbol(owner, source, tree, second),
            Err(NamerError::DuplicateDerivedSourceTreeSymbol {
                owner: duplicate_owner,
                source: duplicate_source,
                tree_index: 0
            }) if duplicate_owner == owner && duplicate_source == source
        ));
        assert_eq!(index.derived_symbol_at(owner, source, tree), Some(first));
    }

    #[test]
    fn canonical_and_derived_source_identities_coexist() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let canonical = symbol(&mut store);
        let derived = symbol(&mut store);
        let source = SourceId::from_index(3);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index.record_symbol(source, tree, canonical).unwrap();
        index
            .record_derived_symbol(owner, source, tree, derived)
            .unwrap();

        assert_eq!(index.symbol_at(source, tree), Some(canonical));
        assert_eq!(index.derived_symbol_at(owner, source, tree), Some(derived));
    }

    #[test]
    fn derived_identity_is_not_returned_by_canonical_lookup() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let derived = symbol(&mut store);
        let source = SourceId::from_index(4);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index
            .record_derived_symbol(owner, source, tree, derived)
            .unwrap();

        assert_eq!(index.symbol_at(source, tree), None);
        assert_eq!(index.derived_symbol_at(owner, source, tree), Some(derived));
    }

    #[test]
    fn one_source_tree_can_have_derived_identities_for_multiple_owners() {
        let mut store = SemanticStore::new();
        let first_owner = symbol(&mut store);
        let second_owner = symbol(&mut store);
        let first_derived = symbol(&mut store);
        let second_derived = symbol(&mut store);
        let source = SourceId::from_index(5);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index
            .record_derived_symbol(first_owner, source, tree, first_derived)
            .unwrap();
        index
            .record_derived_symbol(second_owner, source, tree, second_derived)
            .unwrap();

        assert_eq!(
            index.derived_symbol_at(first_owner, source, tree),
            Some(first_derived)
        );
        assert_eq!(
            index.derived_symbol_at(second_owner, source, tree),
            Some(second_derived)
        );
    }

    #[test]
    fn derived_lookup_requires_the_exact_owner() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let other_owner = symbol(&mut store);
        let derived = symbol(&mut store);
        let source = SourceId::from_index(6);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index
            .record_derived_symbol(owner, source, tree, derived)
            .unwrap();

        assert_eq!(index.derived_symbol_at(owner, source, tree), Some(derived));
        assert_eq!(index.derived_symbol_at(other_owner, source, tree), None);
    }

    #[test]
    fn derived_identity_distinguishes_sources_with_the_same_tree_id() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let first = symbol(&mut store);
        let second = symbol(&mut store);
        let tree = tree_id();
        let first_source = SourceId::from_index(7);
        let second_source = SourceId::from_index(8);
        let mut index = SourceSemanticIndex::new();

        index
            .record_derived_symbol(owner, first_source, tree, first)
            .unwrap();
        index
            .record_derived_symbol(owner, second_source, tree, second)
            .unwrap();

        assert_eq!(
            index.derived_symbol_at(owner, first_source, tree),
            Some(first)
        );
        assert_eq!(
            index.derived_symbol_at(owner, second_source, tree),
            Some(second)
        );
    }

    #[test]
    fn registering_a_derived_identity_does_not_change_scope_lookup() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let derived = symbol(&mut store);
        let scope = store.scopes.alloc(Scope::new(Some(owner)));
        let source = SourceId::from_index(9);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index.record_scope(owner, scope).unwrap();
        index
            .record_derived_symbol(owner, source, tree, derived)
            .unwrap();

        assert_eq!(index.scope_of(owner), Some(scope));
        assert_eq!(index.scope_of(derived), None);
    }

    #[test]
    fn registering_the_same_scope_for_a_symbol_is_idempotent() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let scope = store.scopes.alloc(Scope::new(Some(owner)));
        let mut index = SourceSemanticIndex::new();

        index.record_scope(owner, scope).unwrap();
        index.record_scope(owner, scope).unwrap();

        assert_eq!(index.scope_of(owner), Some(scope));
    }

    #[test]
    fn duplicate_scope_registration_for_a_symbol_is_rejected_without_replacement() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let first_scope = store.scopes.alloc(Scope::new(Some(owner)));
        let second_scope = store.scopes.alloc(Scope::new(Some(owner)));
        let mut index = SourceSemanticIndex::new();

        index.record_scope(owner, first_scope).unwrap();
        assert!(matches!(
            index.record_scope(owner, second_scope),
            Err(NamerError::DuplicateDeclarationScope { symbol }) if symbol == owner
        ));
        assert_eq!(index.scope_of(owner), Some(first_scope));
    }

    #[test]
    fn extension_prefix_clauses_preserve_the_original_clause_and_tree_order() {
        let mut store = SemanticStore::new();
        let method = symbol(&mut store);
        let mut arena = dotty_core::AstArena::<Untyped>::new();
        let first = arena.alloc(Tree {
            kind: TreeKind::PhaseSpecific(UntypedNode::Error(ErrorNode {
                kind: ErrorNodeKind::UnexpectedToken,
            })),
            position: None,
            ty: (),
        });
        let second = arena.alloc(Tree {
            kind: TreeKind::PhaseSpecific(UntypedNode::Error(ErrorNode {
                kind: ErrorNodeKind::UnexpectedToken,
            })),
            position: None,
            ty: (),
        });
        let clauses = vec![vec![first], vec![second, first]];
        let mut index = SourceSemanticIndex::new();

        index
            .record_extension_prefix_clauses(method, &clauses)
            .unwrap();

        assert_eq!(
            index.extension_prefix_clauses(method),
            Some(clauses.as_slice())
        );
        assert_eq!(index.extension_prefix_clauses(symbol(&mut store)), None);
    }

    #[test]
    fn duplicate_extension_prefix_clause_registration_is_rejected_without_replacement() {
        let mut store = SemanticStore::new();
        let method = symbol(&mut store);
        let mut arena = dotty_core::AstArena::<Untyped>::new();
        let first = arena.alloc(Tree {
            kind: TreeKind::PhaseSpecific(UntypedNode::Error(ErrorNode {
                kind: ErrorNodeKind::UnexpectedToken,
            })),
            position: None,
            ty: (),
        });
        let second = arena.alloc(Tree {
            kind: TreeKind::PhaseSpecific(UntypedNode::Error(ErrorNode {
                kind: ErrorNodeKind::UnexpectedToken,
            })),
            position: None,
            ty: (),
        });
        let first_clauses = vec![vec![first]];
        let second_clauses = vec![vec![second]];
        let mut index = SourceSemanticIndex::new();

        index
            .record_extension_prefix_clauses(method, &first_clauses)
            .unwrap();
        assert_eq!(
            index.record_extension_prefix_clauses(method, &second_clauses),
            Err(NamerError::DuplicateExtensionPrefixClauses { method })
        );
        assert_eq!(
            index.extension_prefix_clauses(method),
            Some(first_clauses.as_slice())
        );
    }
}
