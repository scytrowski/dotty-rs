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

    pub(in crate::typer) fn resolve_expression_term_with_inherited(
        &mut self,
        name: dotty_core::Name,
        context: ExpressionContext,
        tree_index: u32,
        position: Option<SourceSpan>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<SymbolId, TyperError> {
        let candidates = self.expression_term_candidates_inner(
            name,
            context,
            tree_index,
            position,
            true,
            info_journal,
        )?;
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
        let mut info_journal = Vec::new();
        self.expression_term_candidates_inner(
            name,
            context,
            tree_index,
            position,
            false,
            &mut info_journal,
        )
    }

    pub(in crate::typer) fn expression_term_candidates_with_inherited(
        &mut self,
        name: dotty_core::Name,
        context: ExpressionContext,
        tree_index: u32,
        position: Option<SourceSpan>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Vec<SymbolId>, TyperError> {
        self.expression_term_candidates_inner(
            name,
            context,
            tree_index,
            position,
            true,
            info_journal,
        )
    }

    fn expression_term_candidates_inner(
        &mut self,
        name: dotty_core::Name,
        context: ExpressionContext,
        tree_index: u32,
        position: Option<SourceSpan>,
        include_inherited_members: bool,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Vec<SymbolId>, TyperError> {
        self.validate_expression_scope_stack(context.local_scopes)?;
        let enclosing_class = self
            .store
            .symbols
            .contains(context.owner)
            .then(|| self.store.symbols.get(context.owner))
            .and_then(|owner| match owner.kind {
                SymbolKind::Class | SymbolKind::Trait => Some(context.owner),
                _ => owner.owner.filter(|symbol| {
                    self.store.symbols.contains(*symbol)
                        && matches!(
                            self.store.symbols.get(*symbol).kind,
                            SymbolKind::Class | SymbolKind::Trait
                        )
                }),
            });
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
                    if include_inherited_members && let Some(child) = enclosing_class {
                        combined.extend(self.direct_source_inherited_immutable_fields(
                            child,
                            name,
                            info_journal,
                        )?);
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
        let mut deferred_parent_error = None;
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
            if include_inherited_members && enclosing_class == Some(source_context.owner) {
                let this_type = self.store.types.alloc(Type::ThisType {
                    class: source_context.owner,
                });
                let inherited =
                    match self.lookup_members_journaled(this_type, name, info_journal) {
                        Ok(inherited) => inherited,
                        Err(error)
                            if matches!(
                                &error,
                                MemberLookupError::ParentTypeAdaptation {
                                    error: TyperError::ExternalGenericInstantiationDeferred { .. },
                                    ..
                                }
                            ) =>
                        {
                            // A matching inherited field still precedes outer
                            // lexical/import scopes. Generic adaptation may
                            // be unavailable for this external parent, so
                            // inspect its raw hierarchy before continuing.
                            let inherited = self
                                .lookup_unadapted_members_journaled(this_type, name, info_journal)
                                .map_err(|error| TyperError::MemberLookup(Box::new(error)))?
                                .into_iter()
                                .filter(|candidate| candidate.inheritance_depth > 0)
                                .filter(|candidate| {
                                    self.store.symbols.contains(candidate.symbol)
                                        && self.store.symbols.get(candidate.symbol).kind
                                            == SymbolKind::Field
                                        && self.store.symbols.get(candidate.symbol).visibility
                                            == dotty_core::symbols::Visibility::Public
                                        && !self
                                            .store
                                            .symbols
                                            .get(candidate.symbol)
                                            .flags
                                            .contains(SymbolFlags::STATIC)
                                })
                                .map(|candidate| candidate.symbol)
                                .collect::<Vec<_>>();
                            if !inherited.is_empty() {
                                return Ok(inherited);
                            }
                            deferred_parent_error.get_or_insert(error);
                            continue;
                        }
                        Err(error) => return Err(TyperError::MemberLookup(Box::new(error))),
                    }
                    .into_iter()
                    .filter(|candidate| candidate.inheritance_depth > 0)
                    .filter(|candidate| {
                        self.store.symbols.contains(candidate.symbol)
                            && self.store.symbols.get(candidate.symbol).kind == SymbolKind::Field
                            && self.store.symbols.get(candidate.symbol).visibility
                                == dotty_core::symbols::Visibility::Public
                            && !self
                                .store
                                .symbols
                                .get(candidate.symbol)
                                .flags
                                .contains(SymbolFlags::STATIC)
                    })
                    .map(|candidate| candidate.symbol)
                    .collect::<Vec<_>>();
                if !inherited.is_empty() {
                    return Ok(inherited);
                }

                // Keep this first source-method increment deliberately
                // narrow: only a canonical method declared directly on
                // the single source parent is admitted. The shared member
                // lookup remains responsible for hierarchy discovery and
                // receiver adaptation. Once an inherited public method name
                // bucket exists, an unsupported bucket must not fall through
                // to an unrelated outer lexical declaration.
                let parent =
                    self.single_direct_source_parent(source_context.owner, info_journal)?;
                if let Some(parent) = parent {
                    let raw = self
                        .lookup_members_journaled(this_type, name, info_journal)
                        .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
                    let methods = raw
                        .into_iter()
                        .filter(|candidate| candidate.inheritance_depth > 0)
                        .filter(|candidate| self.store.symbols.contains(candidate.symbol))
                        .filter(|candidate| {
                            let symbol = self.store.symbols.get(candidate.symbol);
                            symbol.kind == SymbolKind::Method
                                && symbol.visibility == dotty_core::symbols::Visibility::Public
                        })
                        .collect::<Vec<_>>();
                    if !methods.is_empty() {
                        if methods.len() != 1 {
                            return Err(TyperError::OverloadedSelectionDeferred {
                                source: self.source,
                                tree_index,
                                name,
                            });
                        }
                        let candidate = methods[0];
                        let symbol = self.store.symbols.get(candidate.symbol);
                        if candidate.inheritance_depth != 1
                            || candidate.declaring_class != parent
                            || symbol.owner != Some(parent)
                            || symbol.flags.contains(SymbolFlags::STATIC)
                            || symbol.flags.contains(SymbolFlags::ABSTRACT)
                            || !self.is_current_source_symbol(candidate.symbol)
                        {
                            return Err(TyperError::OverloadedSelectionDeferred {
                                source: self.source,
                                tree_index,
                                name,
                            });
                        }
                        let info =
                            self.completed_expression_symbol_info(candidate.symbol, info_journal)?;
                        let Some(Type::Method(method)) = self.store.types.try_get(info) else {
                            return Err(TyperError::OverloadedSelectionDeferred {
                                source: self.source,
                                tree_index,
                                name,
                            });
                        };
                        let ordinary_clause = method.kind == dotty_core::types::MethodKind::Plain
                            && method.params.iter().all(|parameter| {
                                !parameter.erased
                                    && !parameter.varargs
                                    && !matches!(
                                        self.store.types.try_get(parameter.ty),
                                        Some(Type::ByName { .. })
                                    )
                            });
                        if !ordinary_clause
                            || matches!(
                                self.store.types.try_get(method.result),
                                Some(Type::Method(_) | Type::Poly(_))
                            )
                        {
                            return Err(TyperError::OverloadedSelectionDeferred {
                                source: self.source,
                                tree_index,
                                name,
                            });
                        }
                        return Ok(vec![candidate.symbol]);
                    }
                }
            }
        }
        if let Some(deferred_error) = deferred_parent_error {
            let scala_package = self.packages.symbol(&["scala".to_owned()]);
            if let Some(symbol) = self.lookup_default_term_import(name, tree_index, position)?
                && self.store.symbols.contains(symbol)
                && self.store.symbols.get(symbol).kind == SymbolKind::Object
                && self.store.symbols.get(symbol).owner == scala_package
                && scala_package.is_some()
            {
                self.module_class_of_object(symbol)?;
                return Ok(vec![symbol]);
            }
            return Err(TyperError::MemberLookup(Box::new(deferred_error)));
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

    fn single_direct_source_parent(
        &mut self,
        child: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<SymbolId>, TyperError> {
        if !self.is_current_source_symbol(child)
            || !self.store.symbols.contains(child)
            || !matches!(
                self.store.symbols.get(child).kind,
                SymbolKind::Class | SymbolKind::Trait
            )
        {
            return Ok(None);
        }
        let info = self
            .class_info(child, info_journal)
            .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
        if info.parents.len() != 1 {
            return Ok(None);
        }
        let parent_type = info.parents[0];
        let Type::TypeRef {
            target: dotty_core::types::TypeRefTarget::Symbol(parent),
            ..
        } = self.store.types.get(parent_type)
        else {
            return Ok(None);
        };
        Ok((self.is_current_source_symbol(*parent)
            && self.store.symbols.contains(*parent)
            && self.store.symbols.get(*parent).kind == SymbolKind::Class)
            .then_some(*parent))
    }

    fn direct_source_inherited_immutable_fields(
        &mut self,
        child: SymbolId,
        name: dotty_core::Name,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Vec<SymbolId>, TyperError> {
        let Some(parent) = self.single_direct_source_parent(child, info_journal)? else {
            return Ok(Vec::new());
        };
        let receiver = self.store.types.alloc(Type::ThisType { class: child });
        let members = self
            .lookup_members_journaled(receiver, name, info_journal)
            .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
        Ok(members
            .into_iter()
            .filter(|candidate| {
                candidate.inheritance_depth == 1
                    && candidate.declaring_class == parent
                    && self.store.symbols.contains(candidate.symbol)
                    && self.is_current_source_symbol(candidate.symbol)
            })
            .filter(|candidate| {
                let field = self.store.symbols.get(candidate.symbol);
                field.kind == SymbolKind::Field
                    && field.owner == Some(parent)
                    && field.visibility == dotty_core::symbols::Visibility::Public
                    && !field.flags.contains(SymbolFlags::MUTABLE)
                    && !field.flags.contains(SymbolFlags::STATIC)
            })
            .map(|candidate| candidate.symbol)
            .collect())
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
            return self
                .generated_case_class_module_class_of_object(object, owner)
                .ok_or(TyperError::ObjectModuleClassUnavailable { object });
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

    fn generated_case_class_module_class_of_object(
        &self,
        object: SymbolId,
        owner: SymbolId,
    ) -> Option<SymbolId> {
        let object_symbol = self.store.symbols.get(object);
        if object_symbol.origin != SymbolOrigin::Synthetic
            || object_symbol.owner != Some(owner)
            || !matches!(
                self.store.symbols.get(owner).kind,
                SymbolKind::ModuleClass | SymbolKind::Package
            )
        {
            return None;
        }
        let class = object_symbol.links.companion?;
        if !self.store.symbols.contains(class) {
            return None;
        }
        let class_symbol = self.store.symbols.get(class);
        if class_symbol.kind != SymbolKind::Class
            || !class_symbol.flags.contains(SymbolFlags::CASE)
            || class_symbol.origin != SymbolOrigin::Source(self.source)
            || class_symbol.owner != Some(owner)
            || class_symbol.links.companion != Some(object)
        {
            return None;
        }
        let Some(SourceDefinition::Canonical { source, tree }) = self.index.definition_of(class)
        else {
            return None;
        };
        if source != self.source
            || self.index.symbol_at(source, tree) != Some(class)
            || !matches!(
                self.arena.try_get(tree).map(|node| &node.kind),
                Some(TreeKind::TypeDef(definition))
                    if definition.metadata.modifiers.contains(&dotty_core::ast::Modifier::Case)
                        && !definition
                            .metadata
                            .modifiers
                            .contains(&dotty_core::ast::Modifier::Abstract)
            )
        {
            return None;
        }
        let module_class = self.index.derived_symbol_at(owner, source, tree)?;
        let module_symbol = self.store.symbols.get(module_class);
        if module_symbol.kind != SymbolKind::ModuleClass
            || module_symbol.origin != SymbolOrigin::Synthetic
            || module_symbol.owner != Some(owner)
            || self.index.definition_of(module_class)
                != Some(SourceDefinition::Derived { source, tree })
            || self.index.scope_of(module_class).is_none()
        {
            return None;
        }
        let owner_scope = self.index.scope_of(owner)?;
        let scope = self.store.scopes.get(owner_scope);
        (scope.lookup_all(&object_symbol.name).contains(&object)
            && scope
                .lookup_all(&module_symbol.name)
                .contains(&module_class))
        .then_some(module_class)
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
    fn generated_nested_case_class_companion_uses_its_source_module_class_identity() {
        let source = SourceId::from_index(1773);
        let text = "object Owners { case class Node[A](value: A) }";
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
            "GeneratedCaseCompanion.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
            panic!("expected a package root");
        };
        let owner_tree = package.stats[0];
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(owner_definition)) =
            &parsed.ast.get(owner_tree).kind
        else {
            panic!("expected the Owners object");
        };
        let TreeKind::Template(owner_template) = &parsed.ast.get(owner_definition.template).kind
        else {
            panic!("expected the Owners template");
        };
        let class_tree = owner_template.body[0];
        let class = index.symbol_at(source, class_tree).unwrap();
        let owner = store.symbols.get(class).owner.unwrap();
        let owner_tree_symbol = index.symbol_at(source, owner_tree).unwrap();
        assert_eq!(
            store.symbols.get(owner_tree_symbol).kind,
            SymbolKind::Object
        );
        let owner_module_class = index
            .derived_symbol_at(
                store.symbols.get(owner_tree_symbol).owner.unwrap(),
                source,
                owner_tree,
            )
            .unwrap();
        assert_eq!(owner, owner_module_class);

        let companion = store.symbols.get(class).links.companion.unwrap();
        assert_eq!(store.symbols.get(companion).kind, SymbolKind::Object);
        assert_eq!(index.definition_of(companion), None);
        let companion_module_class = index
            .derived_symbol_at(owner, source, class_tree)
            .expect("the generated ModuleClass must retain its class-tree provenance");

        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        assert_eq!(
            typer.module_class_of_object(companion).unwrap(),
            companion_module_class
        );
        let class_info_id = typer.complete_symbol(companion_module_class).unwrap();
        let dotty_core::Type::ClassInfo(class_info) = typer.store().types.get(class_info_id) else {
            panic!("expected a generated companion ClassInfo");
        };
        assert_eq!(class_info.class, companion_module_class);
        assert_eq!(class_info.parents, [definitions.object_type]);
        assert_eq!(
            class_info.declarations,
            index.scope_of(companion_module_class).unwrap()
        );

        let apply = index
            .derived_symbol_at(companion_module_class, source, class_tree)
            .expect("the generated apply must retain its class-tree provenance");
        let apply_info = typer.complete_symbol(apply).unwrap();
        let dotty_core::Type::Poly(poly) = typer.store().types.get(apply_info) else {
            panic!("expected a polymorphic generated apply signature");
        };
        assert_eq!(poly.params.len(), 1);
        let dotty_core::Type::Method(method) = typer.store().types.get(poly.result) else {
            panic!("expected a generated apply value clause");
        };
        assert_eq!(method.kind, dotty_core::MethodKind::Plain);
        assert_eq!(method.params.len(), 1);
        let dotty_core::Type::Applied { tycon, args } = typer.store().types.get(method.result)
        else {
            panic!("expected the result to be the applied nested class type");
        };
        assert_eq!(args.len(), 1);
        assert!(matches!(
            typer.store().types.get(*tycon),
            dotty_core::Type::TypeRef {
                prefix,
                target: dotty_core::TypeRefTarget::Symbol(symbol),
            } if *prefix != definitions.no_prefix && *symbol == class
        ));
    }

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
