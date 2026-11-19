//! Term factories derived from parameterized structural given declarations.

use super::*;

impl SourceTyper<'_> {
    pub(in crate::typer) fn structural_given_class(
        &self,
        method: SymbolId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        let entry = self.store.symbols.get(method);
        let owner = entry.owner?;
        if entry.kind != SymbolKind::Method
            || !entry.flags.contains(SymbolFlags::GIVEN)
            || self.index.derived_symbol_at(owner, self.source, tree) != Some(method)
        {
            return None;
        }
        let class = self.index.symbol_at(self.source, tree)?;
        let implementation = self.store.symbols.get(class);
        (implementation.kind == SymbolKind::Class
            && implementation.owner == Some(owner)
            && implementation.flags.contains(SymbolFlags::GIVEN))
        .then_some(class)
    }

    pub(super) fn is_structural_given_parameter_copy(&self, symbol: SymbolId) -> bool {
        let entry = self.store.symbols.get(symbol);
        if !matches!(
            entry.kind,
            SymbolKind::TypeParameter | SymbolKind::Parameter
        ) {
            return false;
        }
        let Some(method) = entry.owner else {
            return false;
        };
        if self.store.symbols.get(method).kind == SymbolKind::Constructor
            && self.store.symbols.owner(method).is_some_and(|class| {
                let entry = self.store.symbols.get(class);
                entry.kind == SymbolKind::Class && entry.flags.contains(SymbolFlags::GIVEN)
            })
        {
            return matches!(self.index.definition_of(symbol), Some(SourceDefinition::Derived { source, .. }) if source == self.source);
        }
        let Some(SourceDefinition::Derived { source, tree }) = self.index.definition_of(method)
        else {
            return false;
        };
        source == self.source && self.structural_given_class(method, tree).is_some()
    }

    pub(super) fn complete_structural_given_factory(
        &mut self,
        method: SymbolId,
        class: SymbolId,
        tree: TreeId<Untyped>,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let malformed = TyperError::MalformedSourceAst {
            source: self.source,
            tree_index: tree.index(),
            symbol: method,
            kind: SymbolKind::Method,
        };
        let Some(TreeKind::TypeDef(def)) = self.arena.try_get(tree).map(|n| &n.kind) else {
            return Err(malformed);
        };
        let Some(TreeKind::Template(template)) = self.arena.try_get(def.rhs).map(|n| &n.kind)
        else {
            return Err(malformed);
        };
        let Some(TreeKind::DefDef(ctor)) =
            self.arena.try_get(template.constructor).map(|n| &n.kind)
        else {
            return Err(malformed);
        };
        let type_params =
            self.type_parameter_specs(method, &ctor.type_params, true, journal, tree.index(), 0)?;
        let prefix = self.type_symbol_prefix(class);
        let mut result = self.store.types.alloc(Type::type_ref(prefix, class));
        let mut clauses = Vec::new();
        if !type_params.is_empty() {
            let args = type_params
                .iter()
                .map(|p| {
                    self.store
                        .types
                        .alloc(Type::type_ref(self.definitions.no_prefix, p.symbol))
                })
                .collect();
            result = self.store.types.alloc(Type::Applied {
                tycon: result,
                args,
            });
            clauses.push(MethodClauseSpec::Types(type_params));
        }
        // Only written clauses belong to the factory. The extra empty clause
        // introduced during constructor completion belongs to the allocation.
        for trees in self.context_bound_clauses(ctor).iter() {
            let (params, kind) = self.method_parameter_specs(
                method,
                trees,
                true,
                tree.index(),
                clauses.len(),
                journal,
            )?;
            clauses.push(MethodClauseSpec::Terms(params, kind));
        }
        self.bind_method_clauses(clauses, result, tree.index())
    }
}
