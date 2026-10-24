//! Resolution of source expression terms.

use super::super::*;

impl SourceTyper<'_> {
    pub(in crate::typer) fn resolve_expression_term(
        &mut self,
        name: dotty_core::Name,
        context: ExpressionContext,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<SymbolId, TyperError> {
        let candidates = self.expression_term_candidates(name, context, tree_index, position)?;
        self.unique_expression_term(&candidates, name, tree_index, position)
    }

    /// Resolves the highest-precedence lexical/import bucket without choosing
    /// among method overloads. Ordinary expression references continue to use
    /// `resolve_expression_term` and defer when that bucket has several
    /// methods; Apply uses the full bucket for applicability filtering.
    pub(in crate::typer) fn expression_term_candidates(
        &mut self,
        name: dotty_core::Name,
        context: ExpressionContext,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Vec<SymbolId>, TyperError> {
        self.validate_expression_scope_stack(context.local_scopes)?;
        let mut local_scope = context.local_scopes;
        while let Some(stack) = local_scope {
            let frame = self
                .expression_scopes
                .get(stack.index())
                .cloned()
                .ok_or(TyperError::ExpressionLocalScopeStackMissing { stack })?;
            let candidates = self
                .store
                .scopes
                .get(frame.scope)
                .lookup_all(&name)
                .to_vec();
            if !candidates.is_empty() {
                return Ok(candidates);
            }

            if frame.is_block_scope && !frame.imports.is_empty() {
                for selection in [ImportSelection::Explicit, ImportSelection::Wildcard] {
                    let imported = self.lookup_local_import_candidates(
                        &frame.imports,
                        stack,
                        name,
                        false,
                        selection,
                        SourceTreeLocation {
                            tree_index,
                            position,
                        },
                    )?;
                    if imported.is_empty() {
                        continue;
                    }

                    // Scala 3 treats an import in a nested block as ambiguous
                    // with a matching declaration from the enclosing method or
                    // block. A direct declaration in this same block was
                    // already preferred above.
                    let mut combined = imported;
                    let mut outer = frame.parent;
                    while let Some(parent) = outer {
                        let parent_frame = self.expression_scopes.get(parent.index()).ok_or(
                            TyperError::ExpressionLocalScopeStackMissing { stack: parent },
                        )?;
                        let outer_candidates = self
                            .store
                            .scopes
                            .get(parent_frame.scope)
                            .lookup_all(&name)
                            .to_vec();
                        if !outer_candidates.is_empty() {
                            combined.extend(outer_candidates);
                            break;
                        }
                        outer = parent_frame.parent;
                    }
                    if let Some(outer_candidates) = self.lexical_term_candidates(
                        context.lexical,
                        context.owner,
                        name,
                        tree_index,
                    )? {
                        combined.extend(outer_candidates);
                    }
                    combined.sort_by_key(|symbol| symbol.index());
                    combined.dedup();
                    return Ok(combined);
                }
            }
            local_scope = frame.parent;
        }

        let contexts = self.source_context_chain(context.lexical, tree_index)?;
        let fallback_scope = self
            .store
            .symbols
            .contains(context.owner)
            .then(|| self.store.symbols.get(context.owner).owner)
            .flatten();
        let fallback_scope = fallback_scope
            .filter(|owner| {
                self.store.symbols.contains(*owner)
                    && matches!(
                        self.store.symbols.get(*owner).kind,
                        SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                    )
            })
            .and_then(|owner| self.index.scope_of(owner))
            .filter(|scope| {
                contexts.iter().all(|context_id| {
                    self.index.source_context(*context_id).lexical_scope != *scope
                })
            });
        for (context_position, context_id) in contexts.iter().enumerate() {
            let source_context = self.index.source_context(*context_id);
            let candidates = self
                .store
                .scopes
                .get(source_context.lexical_scope)
                .lookup_all(&name)
                .to_vec();
            if !candidates.is_empty() {
                return Ok(candidates);
            }
            // Top-level source methods are owned by their synthetic package
            // module class, while their recorded lexical context is the
            // package wrapper. Check those same-unit members before imports,
            // matching their source-declaration precedence.
            if context_position == 0
                && let Some(scope) = fallback_scope
            {
                let candidates = self.store.scopes.get(scope).lookup_all(&name).to_vec();
                if !candidates.is_empty() {
                    return Ok(candidates);
                }
            }
            for selection in [ImportSelection::Explicit, ImportSelection::Wildcard] {
                let candidates = self.lookup_import_candidates(
                    &[*context_id],
                    name,
                    false,
                    selection,
                    SourceTreeLocation {
                        tree_index,
                        position,
                    },
                )?;
                if !candidates.is_empty() {
                    return Ok(candidates);
                }
            }
        }
        if let Some(symbol) = self.lookup_default_term_import(name, tree_index, position)? {
            return Ok(vec![symbol]);
        }
        Err(TyperError::TermNameNotFound {
            source: self.source,
            tree_index,
            name,
            position,
        })
    }

    fn lookup_default_term_import(
        &mut self,
        name: dotty_core::Name,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<Option<SymbolId>, TyperError> {
        // Scala 3.9 exposes package-level terms through the Scala package
        // object. The classpath resolver validates the alias target and
        // returns its canonical module Object.
        let path = ["scala".to_owned()];
        let package = match self.packages.symbol(&path) {
            Some(package) => Some(package),
            None => self.resolve_external_package(&path, tree_index)?,
        };
        let Some(package) = package else {
            return Ok(None);
        };
        let direct_source_candidates = self
            .scopes_of(package)
            .into_iter()
            .flat_map(|scope| self.store.scopes.get(scope).lookup_all(&name))
            .copied()
            .filter(|symbol| {
                let entry = self.store.symbols.get(*symbol);
                entry.name.namespace() == dotty_core::Namespace::Term
                    && entry.origin == SymbolOrigin::Source(self.source)
            })
            .collect::<Vec<_>>();
        if let Some(symbol) =
            self.unique_symbol_candidate(&direct_source_candidates, name, tree_index, position)?
        {
            return Ok(Some(symbol));
        }
        let request = MemberRequest {
            prefix: self.package_type_prefix(package),
            name,
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        let Some(symbol) = self
            .resolver
            .resolve_member(self.store, &request)
            .map_err(|error| TyperError::SymbolResolution {
                source: self.source,
                tree_index,
                error,
            })?
        else {
            return Ok(None);
        };
        if !self.store.symbols.contains(symbol) {
            return Err(TyperError::UnknownSymbol { symbol });
        }
        Ok(
            (self.store.symbols.get(symbol).name.namespace() == dotty_core::Namespace::Term)
                .then_some(symbol),
        )
    }

    fn lexical_term_candidates(
        &self,
        context: SourceContextId,
        owner: SymbolId,
        name: dotty_core::Name,
        tree_index: u32,
    ) -> Result<Option<Vec<SymbolId>>, TyperError> {
        let contexts = self.source_context_chain(context, tree_index)?;
        let fallback_scope = self
            .store
            .symbols
            .contains(owner)
            .then(|| self.store.symbols.get(owner).owner)
            .flatten()
            .filter(|owner| {
                self.store.symbols.contains(*owner)
                    && matches!(
                        self.store.symbols.get(*owner).kind,
                        SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                    )
            })
            .and_then(|owner| self.index.scope_of(owner))
            .filter(|scope| {
                contexts.iter().all(|context_id| {
                    self.index.source_context(*context_id).lexical_scope != *scope
                })
            });
        for (context_position, context_id) in contexts.iter().enumerate() {
            let source_context = self.index.source_context(*context_id);
            let candidates = self
                .store
                .scopes
                .get(source_context.lexical_scope)
                .lookup_all(&name)
                .to_vec();
            if !candidates.is_empty() {
                return Ok(Some(candidates));
            }
            if context_position == 0
                && let Some(scope) = fallback_scope
            {
                let candidates = self.store.scopes.get(scope).lookup_all(&name).to_vec();
                if !candidates.is_empty() {
                    return Ok(Some(candidates));
                }
            }
        }
        Ok(None)
    }

    pub(in crate::typer) fn unique_expression_term(
        &self,
        candidates: &[SymbolId],
        name: dotty_core::Name,
        tree_index: u32,
        position: Option<SourceSpan>,
    ) -> Result<SymbolId, TyperError> {
        match candidates {
            [symbol] => Ok(*symbol),
            many if many
                .iter()
                .all(|symbol| self.store.symbols.get(*symbol).kind == SymbolKind::Method) =>
            {
                Err(TyperError::OverloadedReferenceDeferred {
                    source: self.source,
                    tree_index,
                    name,
                })
            }
            _ => Err(TyperError::AmbiguousTermReference {
                source: self.source,
                tree_index,
                name,
                position,
            }),
        }
    }

    pub(in crate::typer) fn source_module_class_of_object(
        &self,
        object: SymbolId,
    ) -> Result<SymbolId, TyperError> {
        if !self.store.symbols.contains(object) {
            return Err(TyperError::UnknownSymbol { symbol: object });
        }
        let declaration = self.store.symbols.get(object);
        if declaration.kind != SymbolKind::Object {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        }
        let Some(owner) = declaration.owner else {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        };
        let Some(SourceDefinition::Canonical { source, tree }) = self.index.definition_of(object)
        else {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        };
        if source != self.source {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        }
        let Some(module_class) = self.index.derived_symbol_at(owner, source, tree) else {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        };
        if !self.store.symbols.contains(module_class)
            || self.store.symbols.get(module_class).kind != SymbolKind::ModuleClass
            || self.index.definition_of(module_class)
                != Some(SourceDefinition::Derived { source, tree })
        {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        }
        Ok(module_class)
    }

    /// Returns the source-owned or resolver-validated ModuleClass for an
    /// Object. External identities are accepted only through the resolver's
    /// authoritative module cache; matching a `Foo$` name is validation of
    /// that returned identity, never a way to discover one.
    pub(in crate::typer) fn module_class_of_object(
        &self,
        object: SymbolId,
    ) -> Result<SymbolId, TyperError> {
        if let Ok(module_class) = self.source_module_class_of_object(object) {
            return Ok(module_class);
        }
        let module_class = self
            .resolver
            .module_class_of_object(self.store, object)
            .map_err(|error| TyperError::ObjectModuleClassResolution { object, error })?
            .ok_or(TyperError::ObjectModuleClassUnavailable { object })?;
        if !self.store.symbols.contains(object) || !self.store.symbols.contains(module_class) {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        }
        let object_symbol = self.store.symbols.get(object);
        let module_symbol = self.store.symbols.get(module_class);
        let expected_module_name =
            format!("{}$", self.store.names.resolve(object_symbol.name.text()));
        if object_symbol.kind != SymbolKind::Object
            || object_symbol.name.namespace() != dotty_core::Namespace::Term
            || module_symbol.kind != SymbolKind::ModuleClass
            || module_symbol.name.namespace() != dotty_core::Namespace::Type
            || module_symbol.owner != object_symbol.owner
            || object_symbol.owner.is_none()
            || self.store.names.resolve(module_symbol.name.text()) != expected_module_name
        {
            return Err(TyperError::ObjectModuleClassUnavailable { object });
        }
        Ok(module_class)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::ast::TreeKind;
    use dotty_core::{Definitions, Packages, SemanticStore, SourceId, SourceText};
    use dotty_lexer::ContextualScanner;
    use dotty_namer::name_compilation_unit;
    use dotty_parser::parse_compilation_unit;

    #[test]
    fn default_term_import_resolves_a_scala_package_term() {
        let source = SourceId::from_index(1566);
        let text = "package scala { object Nil }; package app { object Use { def run = Nil } }";
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let scanner = ContextualScanner::new(text).unwrap();
        let parsed = parse_compilation_unit(
            SourceText::new(text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let mut packages = Packages::new();
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "DefaultTermImport.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        let scala = packages.symbol(&["scala"]).unwrap();
        let nil_name =
            dotty_core::Name::new(store.names.intern("Nil"), dotty_core::Namespace::Term);
        let nil = store
            .scopes
            .get(packages.scope_of(scala).unwrap())
            .lookup_all(&nil_name)[0];
        let (run, rhs) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "run" =>
                {
                    Some((
                        index.symbol_at(source, tree).unwrap(),
                        definition.rhs.unwrap(),
                    ))
                }
                _ => None,
            })
            .unwrap();
        let lexical = index.declaration_context_of(run).unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        assert_eq!(
            typer.lookup_default_term_import(nil_name, 0, None).unwrap(),
            Some(nil)
        );
        assert_eq!(
            typer
                .expression_term_candidates(
                    nil_name,
                    ExpressionContext {
                        lexical,
                        owner: run,
                        local_scopes: None,
                    },
                    rhs.index(),
                    None,
                )
                .unwrap(),
            [nil]
        );
        let context = typer.expression_context_for(run).unwrap();
        assert!(typer.type_expression(rhs, context).is_ok());
        let missing = dotty_core::Name::new(
            typer.store.names.intern("NotAStandardTerm"),
            dotty_core::Namespace::Term,
        );
        assert_eq!(
            typer
                .lookup_default_term_import(missing, rhs.index(), None)
                .unwrap(),
            None
        );
    }
}
