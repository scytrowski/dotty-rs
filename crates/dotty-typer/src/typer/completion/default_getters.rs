//! Source default getters for contextual parameters of member methods.
use super::*;

impl SourceTyper<'_> {
    pub(in crate::typer) fn contextual_getter_source(
        &self,
        getter: SymbolId,
    ) -> Option<(TreeId<Untyped>, TreeId<Untyped>, usize)> {
        let entry = self.store.symbols.get(getter);
        if entry.kind != SymbolKind::Method || !entry.flags.contains(SymbolFlags::SYNTHETIC) {
            return None;
        }
        let SourceDefinition::Derived { source, tree } = self.index.definition_of(getter)? else {
            return None;
        };
        let owner = entry.owner?;
        if source != self.source
            || self.index.derived_symbol_at(owner, source, tree) != Some(getter)
        {
            return None;
        }
        let parameter = self.index.symbol_at(source, tree)?;
        let method = self.store.symbols.owner(parameter)?;
        if self.store.symbols.get(parameter).kind != SymbolKind::Parameter
            || self.store.symbols.owner(method) != Some(owner)
        {
            return None;
        }
        let SourceDefinition::Canonical {
            tree: method_tree,
            source: method_source,
        } = self.index.definition_of(method)?
        else {
            return None;
        };
        if method_source != source {
            return None;
        }
        let TreeKind::DefDef(def) = &self.arena.try_get(method_tree)?.kind else {
            return None;
        };
        let TreeKind::ValDef(param) = &self.arena.try_get(tree)?.kind else {
            return None;
        };
        param.rhs?;
        let clause = def
            .value_param_clauses
            .iter()
            .position(|clause| clause.contains(&tree))?;
        Some((method_tree, tree, clause))
    }

    pub(super) fn complete_contextual_getter(
        &mut self,
        getter: SymbolId,
        method_tree: TreeId<Untyped>,
        parameter: TreeId<Untyped>,
        preceding: usize,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let deferred = TyperError::UsingApplicationDeferred {
            source: self.source,
            tree_index: parameter.index(),
        };
        let TreeKind::DefDef(def) = &self.arena.get(method_tree).kind else {
            return Err(deferred);
        };
        let TreeKind::ValDef(param) = &self.arena.get(parameter).kind else {
            return Err(deferred);
        };
        if self.context_bound_clauses(def).as_ref() != def.value_param_clauses.as_slice() {
            return Err(deferred);
        }
        let mut clauses = Vec::new();
        if !def.type_params.is_empty() {
            clauses.push(MethodClauseSpec::Types(self.type_parameter_specs(
                getter,
                &def.type_params,
                true,
                journal,
                parameter.index(),
                0,
            )?));
        }
        for previous in &def.value_param_clauses[..preceding] {
            let (params, kind) = self.method_parameter_specs(
                getter,
                previous,
                true,
                parameter.index(),
                clauses.len(),
                journal,
            )?;
            clauses.push(MethodClauseSpec::Terms(params, kind));
        }
        let scope = self
            .index
            .declaration_context_of(getter)
            .ok_or(TyperError::DeclarationContextMissing { symbol: getter })?;
        let result = self.type_of_tpt_inner_journaled(param.tpt, scope, journal)?;
        self.bind_method_clauses(clauses, result, parameter.index())
    }

    pub(in crate::typer) fn check_contextual_default_bodies(
        &mut self,
        method: SymbolId,
        definition: &dotty_core::ast::DefDef<Untyped>,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
        mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<(), TyperError> {
        let Some(owner) = self.store.symbols.owner(method) else {
            return Ok(());
        };
        for &tree in definition.value_param_clauses.iter().flatten() {
            let Some(node) = self.arena.try_get(tree) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: tree.index(),
                });
            };
            let TreeKind::ValDef(param) = &node.kind else {
                continue;
            };
            let Some(rhs) = param.rhs else { continue };
            let getter = self
                .index
                .derived_symbol_at(owner, self.source, tree)
                .filter(|&g| self.contextual_getter_source(g).is_some())
                .ok_or(TyperError::UsingApplicationDeferred {
                    source: self.source,
                    tree_index: tree.index(),
                })?;
            self.complete_signature_parameter(getter, journal)?;
            let context = self.expression_context_for(getter)?;
            let canonical = std::mem::take(&mut self.type_index);
            let expected = self.type_of_tpt_inner_journaled(param.tpt, context.lexical, journal);
            self.type_index = canonical;
            self.type_expression_expected_inner(rhs, context, expected?, journal, mappings)?;
        }
        Ok(())
    }
}
