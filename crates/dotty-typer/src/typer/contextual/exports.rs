//! Contextual forwarders declared by a source object's export clauses.

use super::*;

impl SourceTyper<'_> {
    pub(in crate::typer) fn enter_contextual_exports(
        &mut self,
        owner: SymbolId,
        scope: ScopeId,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(), TyperError> {
        for site in self.index.export_sites_for_owner(owner) {
            if site.source != self.source || site.extension_prefix_clauses.is_some() {
                continue;
            }
            let TreeKind::Export(export) = &self.arena.get(site.tree).kind else {
                continue;
            };
            let position = self.arena.get(site.tree).position;
            let Some(object) = self.resolve_qualifier_symbol(
                export.expr,
                site.context,
                site.tree.index(),
                position,
            )?
            else {
                continue;
            };
            if self.store.symbols.get(object).kind != SymbolKind::Object
                || !self.is_current_source_symbol(object)
                || !self
                    .store
                    .symbols
                    .owner(object)
                    .is_some_and(|p| self.store.symbols.get(p).kind == SymbolKind::Package)
            {
                continue;
            }
            let module = self.source_module_class_of_object(object)?;
            let members = self.synthetic_module_scope(module)?;
            for selector in &export.selectors {
                if selector.renamed.is_some() || selector.bound.is_some() {
                    continue;
                }
                let targets = self
                    .store
                    .scopes
                    .get(members)
                    .lookup_all(&selector.imported)
                    .to_vec();
                for target in targets {
                    let entry = self.store.symbols.get(target);
                    if entry.visibility != Visibility::Public
                        || !entry.flags.contains(SymbolFlags::GIVEN)
                        || entry.kind != SymbolKind::Field
                    {
                        continue;
                    }
                    let name = entry.name;
                    let prefix =
                        self.term_reference_type(object, None, owner, site.tree.index(), journal)?;
                    let result = self.term_reference_type(
                        target,
                        Some(prefix),
                        owner,
                        site.tree.index(),
                        journal,
                    )?;
                    let decl = dotty_core::Symbol {
                        name,
                        owner: Some(owner),
                        kind: SymbolKind::Method,
                        flags: SymbolFlags::EXPORTED | SymbolFlags::FINAL | SymbolFlags::GIVEN,
                        visibility: Visibility::Public,
                        info: SymbolInfo::Complete(result),
                        origin: SymbolOrigin::Synthetic,
                        position,
                        annotations: vec![],
                        links: Default::default(),
                    };
                    self.store
                        .enter_export_forwarder(scope, decl, result)
                        .ok_or(TyperError::AmbiguousTermReference {
                            source: self.source,
                            tree_index: site.tree.index(),
                            name,
                            position,
                        })?;
                }
            }
        }
        Ok(())
    }

    /// Materializes a source object's exported contextual method and its delegation
    /// in this typer’s arena. The symbol must be a completed export from the
    /// same semantic store. Repeated requests return the same typed definition.
    /// Unknown or unsupported forwarders return an error without publishing a tree.
    pub fn type_export_forwarder_definition(
        &mut self,
        method: SymbolId,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(&tree) = self.export_definitions.get(&method) {
            return Ok(tree);
        }
        let tree = self.run_atomic(|typer, _| typer.build_export_definition(method))?;
        self.export_definitions.insert(method, tree);
        Ok(tree)
    }

    fn build_export_definition(&mut self, method: SymbolId) -> Result<TreeId<Typed>, TyperError> {
        let invalid = TyperError::SourceProvenanceMissing { symbol: method };
        let Some(target) = self.store.export_target(method) else {
            return Err(invalid);
        };
        let Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(member),
        } = *self.store.types.get(target)
        else {
            return Err(invalid);
        };
        let Some(object) = self
            .store
            .types
            .get(prefix)
            .reference_symbol()
            .filter(|&id| self.store.symbols.contains(id))
        else {
            return Err(invalid);
        };
        let entry = self.store.symbols.get(method);
        let position = entry.position;
        let name = dotty_core::TermName::new(entry.name.text());
        let mut builder = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types);
        let receiver = builder.ident(self.store.symbols.get(object).name, prefix, position);
        let rhs = builder.select(
            receiver,
            self.store.symbols.get(member).name,
            false,
            target,
            position,
        );
        let tpt = builder.type_tree(target, position);
        Ok(self.typed_index.alloc_synthetic_definition(
            &mut self.typed_arena,
            method,
            Tree {
                ty: target,
                position,
                kind: TreeKind::DefDef(dotty_core::ast::DefDef {
                    name,
                    type_params: vec![],
                    value_param_clauses: vec![],
                    source_param_clause_order: None,
                    tpt,
                    rhs: Some(rhs),
                    metadata: (),
                }),
            },
        ))
    }
}
