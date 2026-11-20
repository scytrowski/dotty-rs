//! The source-free right-hand side of a generated case-class companion factory.

use super::*;

impl SourceTyper<'_> {
    /// Types the body of a namer-generated case-class `apply` in its own
    /// parameter scope. Like source method bodies, it is requested separately
    /// from signature completion. Repeated requests return the same typed root.
    pub fn type_generated_case_class_apply_body(
        &mut self,
        method: SymbolId,
    ) -> Result<TreeId<Typed>, TyperError> {
        if !self.store.symbols.contains(method) {
            return Err(TyperError::UnknownSymbol { symbol: method });
        }
        if !matches!(self.index.generated_case_class_method_identity(method), Some(id) if id.role == dotty_core::GeneratedCaseClassMethodRole::Apply)
        {
            return Err(TyperError::SourceProvenanceMissing { symbol: method });
        }
        self.type_generated_factory_body(method)
    }

    /// Types the generated `copy` constructor call with the copy method's own parameters.
    pub fn type_generated_case_class_copy_body(
        &mut self,
        method: SymbolId,
    ) -> Result<TreeId<Typed>, TyperError> {
        if self.generated_case_class_for_copy(method).is_none() {
            return Err(TyperError::SourceProvenanceMissing { symbol: method });
        }
        self.type_generated_factory_body(method)
    }

    fn type_generated_factory_body(
        &mut self,
        method: SymbolId,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(&body) = self.generated_factory_bodies.get(&method) {
            return Ok(body);
        }
        let body =
            self.run_atomic(|typer, journal| typer.build_case_class_factory_body(method, journal))?;
        self.generated_factory_bodies.insert(method, body);
        Ok(body)
    }

    fn build_case_class_factory_body(
        &mut self,
        method: SymbolId,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let provenance = || TyperError::SourceProvenanceMissing { symbol: method };
        let Some(SourceDefinition::Derived { source, tree }) = self.index.definition_of(method)
        else {
            return Err(provenance());
        };
        let class = self
            .generated_case_class_for_factory(method, source, tree)
            .ok_or_else(provenance)?;
        let TreeKind::TypeDef(class_def) = &self.arena.get(tree).kind else {
            return Err(provenance());
        };
        let Some(TreeKind::Template(template)) = self.arena.try_get(class_def.rhs).map(|n| &n.kind)
        else {
            return Err(provenance());
        };
        let ctor_tree = template.constructor;
        let Some(TreeKind::DefDef(def)) = self.arena.try_get(ctor_tree).map(|n| &n.kind) else {
            return Err(provenance());
        };
        let constructor = self
            .index
            .symbol_at(source, ctor_tree)
            .ok_or_else(provenance)?;
        if self.store.symbols.owner(constructor) != Some(class)
            || self.store.symbols.get(constructor).kind != SymbolKind::Constructor
        {
            return Err(provenance());
        }
        let mut factory = self.complete_signature_parameter(method, journal)?;
        let mut ctor = self.complete_signature_parameter(constructor, journal)?;
        let rebind = |error| TyperError::TypeRebinding {
            source,
            tree_index: tree.index(),
            error,
        };
        if !def.type_params.is_empty() {
            let mut args = Vec::with_capacity(def.type_params.len());
            for &parameter in &def.type_params {
                let symbol = self.method_parameter_symbol(method, parameter, true)?;
                args.push(
                    self.store
                        .types
                        .alloc(Type::type_ref(self.definitions.no_prefix, symbol)),
                );
            }
            factory = dotty_core::types::instantiate_poly(self.store, factory, &args)
                .map_err(rebind)?
                .result;
            ctor = dotty_core::types::instantiate_poly(self.store, ctor, &args)
                .map_err(rebind)?
                .result;
        }
        let instance =
            self.validate_constructor_application_callable(constructor, ctor, tree.index())?;
        let position = self.arena.get(tree).position;
        let ctor_ref = self
            .store
            .types
            .alloc(Type::term_ref(instance, constructor));
        let mut builder = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types);
        let tpt = builder.type_tree(instance, position);
        let allocation = builder.new_expr(tpt, instance, position);
        let mut body = builder.select(
            allocation,
            self.store.symbols.get(constructor).name,
            false,
            ctor_ref,
            position,
        );
        for (clause_index, trees) in self.context_bound_clauses(def).iter().enumerate() {
            let malformed = || TyperError::MalformedMethodClause {
                method,
                method_tree_index: tree.index(),
                clause_index,
            };
            let Some(Type::Method(formals)) = self.store.types.try_get(ctor).cloned() else {
                return Err(malformed());
            };
            let Some(Type::Method(factory_clause)) = self.store.types.try_get(factory) else {
                return Err(malformed());
            };
            if formals.params.len() != trees.len()
                || factory_clause.params.len() != trees.len()
                || factory_clause.kind != formals.kind
            {
                return Err(malformed());
            }
            let mut args = Vec::with_capacity(trees.len());
            let mut paths = Vec::with_capacity(trees.len());
            for (index, (&param, formal)) in trees.iter().zip(formals.params).enumerate() {
                let symbol = self.method_parameter_symbol(method, param, true)?;
                let actual = self.complete_signature_parameter(symbol, journal)?;
                if !self.contextual_conforms(actual, formal.ty, param.index(), journal)? {
                    return Err(TyperError::ApplicationArgumentTypeMismatch {
                        source,
                        tree_index: tree.index(),
                        argument_index: index,
                        actual,
                        expected: formal.ty,
                    });
                }
                let path = self
                    .store
                    .types
                    .alloc(Type::term_ref(self.definitions.no_prefix, symbol));
                let arg = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).ident(
                    self.store.symbols.get(symbol).name,
                    path,
                    self.arena.get(param).position,
                );
                args.push(arg);
                paths.push(Some(path));
            }
            factory = dotty_core::types::instantiate_method_result(self.store, factory, &paths)
                .map_err(rebind)?;
            ctor = dotty_core::types::instantiate_method_result(self.store, ctor, &paths)
                .map_err(rebind)?;
            let kind = if formals.kind == MethodKind::Contextual {
                ApplyKind::Using
            } else {
                ApplyKind::Regular
            };
            body = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                .apply_with_kind(body, args, kind, ctor, position);
        }
        self.require_constructor_result_matches_instance(
            constructor,
            ctor,
            factory,
            tree.index(),
            journal,
        )?;
        Ok(body)
    }
}
