//! Completion of typer-owned local method signatures and results.

use super::*;

impl SourceTyper<'_> {
    pub(super) fn complete_local_method_signature(
        &mut self,
        method: SymbolId,
        method_tree_index: u32,
        definition: &dotty_core::ast::DefDef<Untyped>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let method_scope = self.method_scope(method)?;
        let existing_symbols = self
            .store
            .scopes
            .get(method_scope)
            .entered_symbols()
            .collect::<HashSet<_>>();
        let result = self.complete_local_method_signature_inner(
            method,
            method_tree_index,
            definition,
            info_journal,
        );
        if result.is_err() {
            let new_symbols = self
                .store
                .scopes
                .get(method_scope)
                .entered_symbols()
                .filter(|symbol| !existing_symbols.contains(symbol))
                .collect::<Vec<_>>();
            for symbol in new_symbols {
                self.store.scopes.get_mut(method_scope).remove(symbol);
            }
        }
        result
    }

    fn complete_local_method_signature_inner(
        &mut self,
        method: SymbolId,
        method_tree_index: u32,
        definition: &dotty_core::ast::DefDef<Untyped>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let deferred = |feature| TyperError::LocalMethodSignatureDeferred {
            symbol: method,
            tree_index: method_tree_index,
            feature,
        };
        if self
            .store
            .symbols
            .get(method)
            .flags
            .contains(SymbolFlags::EXTENSION)
        {
            return Err(deferred("extension methods"));
        }
        let result_node =
            self.arena
                .try_get(definition.tpt)
                .ok_or(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: definition.tpt.index(),
                })?;
        let infer_result = matches!(result_node.kind, TreeKind::TypeTree(_));
        if infer_result && !self.inferred_method_results_in_progress.insert(method) {
            return Err(TyperError::RecursiveInferredMethodResult { symbol: method });
        }

        let declaration_context = self
            .local_methods
            .declaration_context(method)
            .ok_or(TyperError::DeclarationContextMissing { symbol: method })?;
        let method_scope = self.method_scope(method)?;
        let mut parameters_to_enter = Vec::new();
        for type_parameter_tree in &definition.type_params {
            let Some(parameter_node) = self.arena.try_get(*type_parameter_tree) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: type_parameter_tree.index(),
                });
            };
            let TreeKind::TypeDef(parameter) = &parameter_node.kind else {
                return Err(deferred("non-type parameter"));
            };
            if self
                .local_methods
                .type_parameter_symbol_at(self.source, *type_parameter_tree)
                .is_some()
            {
                continue;
            }
            let symbol = self.store.symbols.alloc(dotty_core::Symbol {
                name: *parameter.name.as_name(),
                owner: Some(method),
                kind: SymbolKind::TypeParameter,
                flags: SymbolFlags::EMPTY,
                visibility: dotty_core::Visibility::Public,
                info: SymbolInfo::Missing,
                origin: SymbolOrigin::Source(self.source),
                annotations: Vec::new(),
                position: parameter_node.position,
                links: dotty_core::SymbolLinks::default(),
            });
            self.store
                .scopes
                .get_mut(method_scope)
                .enter(*parameter.name.as_name(), symbol);
            self.local_methods.insert_type_parameter(
                self.source,
                *type_parameter_tree,
                symbol,
                declaration_context.lexical,
            );
        }
        for clause in &definition.value_param_clauses {
            for parameter_tree in clause {
                let Some(parameter_node) = self.arena.try_get(*parameter_tree) else {
                    return Err(TyperError::TreeOutsideArena {
                        source: self.source,
                        tree_index: parameter_tree.index(),
                    });
                };
                let TreeKind::ValDef(parameter) = &parameter_node.kind else {
                    return Err(deferred("non-value parameter"));
                };
                if parameter.metadata.modifiers.contains(&Modifier::Erased) {
                    return Err(deferred("erased parameters"));
                }
                if parameter.metadata.modifiers.iter().any(|modifier| {
                    !matches!(
                        modifier,
                        Modifier::Param | Modifier::Given | Modifier::Implicit
                    )
                }) {
                    return Err(deferred("parameter modifiers"));
                }
                let parameter_type =
                    self.arena
                        .try_get(parameter.tpt)
                        .ok_or(TyperError::TreeOutsideArena {
                            source: self.source,
                            tree_index: parameter.tpt.index(),
                        })?;
                if matches!(parameter_type.kind, TreeKind::ByNameTypeTree(_))
                    || matches!(
                        &parameter_type.kind,
                        TreeKind::PhaseSpecific(UntypedNode::PostfixOp(postfix))
                            if self.store.names.resolve(postfix.op.text()) == "*"
                    )
                {
                    return Err(deferred("by-name or repeated parameters"));
                }
                if self
                    .local_methods
                    .parameter_symbol_at(self.source, *parameter_tree)
                    .is_some()
                {
                    continue;
                }
                let symbol = self.store.symbols.alloc(dotty_core::Symbol {
                    name: *parameter.name.as_name(),
                    owner: Some(method),
                    kind: SymbolKind::Parameter,
                    flags: source_method_flags(&parameter.metadata.modifiers),
                    visibility: dotty_core::Visibility::Public,
                    info: SymbolInfo::Missing,
                    origin: SymbolOrigin::Source(self.source),
                    annotations: Vec::new(),
                    position: parameter_node.position,
                    links: dotty_core::SymbolLinks::default(),
                });
                parameters_to_enter.push((*parameter.name.as_name(), symbol));
                self.local_methods.insert_parameter(
                    self.source,
                    *parameter_tree,
                    symbol,
                    declaration_context.lexical,
                );
            }
        }
        let parameter_names = parameters_to_enter
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>();
        if self.local_result_depends_on_parameters(definition.tpt, &parameter_names) {
            return Err(deferred("dependent result types"));
        }
        for (name, parameter) in parameters_to_enter {
            self.store
                .scopes
                .get_mut(method_scope)
                .enter(name, parameter);
        }
        self.active_local_type_scopes
            .push((declaration_context.lexical, method_scope));
        let signature = self.complete_method_signature_body(
            method,
            method_tree_index,
            definition,
            infer_result,
            info_journal,
        );
        self.active_local_type_scopes.pop();
        if infer_result {
            self.inferred_method_results_in_progress.remove(&method);
        }
        signature
    }

    fn local_result_depends_on_parameters(
        &self,
        result_tree: TreeId<Untyped>,
        parameter_names: &[dotty_core::Name],
    ) -> bool {
        let mut pending = vec![result_tree];
        let mut visited = HashSet::new();
        while let Some(tree) = pending.pop() {
            if !visited.insert(tree) {
                continue;
            }
            let Some(node) = self.arena.try_get(tree) else {
                continue;
            };
            match &node.kind {
                TreeKind::SingletonTypeTree(singleton) => {
                    if self.local_parameter_path(singleton.reference, parameter_names) {
                        return true;
                    }
                }
                TreeKind::Select(selection) => {
                    if self.local_parameter_path(selection.qualifier, parameter_names) {
                        return true;
                    }
                }
                TreeKind::AppliedTypeTree(applied) => {
                    pending.push(applied.tpt);
                    pending.extend(applied.args.iter().copied());
                }
                TreeKind::ByNameTypeTree(by_name) => pending.push(by_name.result),
                TreeKind::RefinedTypeTree(refined) => {
                    pending.push(refined.tpt);
                    pending.extend(refined.refinements.iter().copied());
                }
                TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => {
                    pending.push(parens.inner);
                }
                _ => {}
            }
        }
        false
    }

    fn local_parameter_path(
        &self,
        mut tree: TreeId<Untyped>,
        parameter_names: &[dotty_core::Name],
    ) -> bool {
        let mut visited = HashSet::new();
        loop {
            if !visited.insert(tree) {
                return false;
            }
            let Some(node) = self.arena.try_get(tree) else {
                return false;
            };
            match &node.kind {
                TreeKind::Ident(ident) => return parameter_names.contains(&ident.name),
                TreeKind::Select(selection) => tree = selection.qualifier,
                TreeKind::SingletonTypeTree(singleton) => tree = singleton.reference,
                TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => tree = parens.inner,
                _ => return false,
            }
        }
    }
}
