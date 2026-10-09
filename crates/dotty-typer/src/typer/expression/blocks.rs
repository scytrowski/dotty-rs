//! Block scopes and local value/method expression typing.

use super::super::{ExpressionContext, ExpressionScopeId, SourceTyper, TyperError};
use super::super::{local_block_declaration_kind, source_method_flags, tree_kind_name};
use dotty_core::ast::*;
use dotty_core::types::*;
use dotty_core::*;
use std::collections::HashMap;

/// The single typed identity for a source block statement and the ordered
/// typed statements emitted into the enclosing block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::typer) struct TypedStatExpansion {
    pub(in crate::typer) anchor: TreeId<Typed>,
    pub(in crate::typer) emitted: Vec<TreeId<Typed>>,
}

impl TypedStatExpansion {
    fn one(typed: TreeId<Typed>) -> Self {
        Self {
            anchor: typed,
            emitted: vec![typed],
        }
    }

    pub(in crate::typer) fn append_to(self, stats: &mut Vec<TreeId<Typed>>) {
        debug_assert!(self.emitted.contains(&self.anchor));
        stats.extend(self.emitted);
    }
}

/// The typed result recorded for one source PatDef. User-visible binder
/// symbols remain in `SourceTyper::local_symbols`; this record owns only the
/// statement expansion and compiler-generated temporaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::typer) struct PatDefStatExpansion {
    pub(in crate::typer) anchor: TreeId<Typed>,
    pub(in crate::typer) emitted: Vec<TreeId<Typed>>,
    pub(in crate::typer) synthetic_symbols: Vec<SymbolId>,
}

#[derive(Clone, Default)]
pub(in crate::typer) struct PatDefExpansionIndex {
    by_tree: HashMap<(SourceId, TreeId<Untyped>), PatDefStatExpansion>,
}

impl PatDefExpansionIndex {
    pub(in crate::typer) fn get(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<&PatDefStatExpansion> {
        self.by_tree.get(&(source, tree))
    }

    fn insert(
        &mut self,
        source: SourceId,
        tree: TreeId<Untyped>,
        expansion: PatDefStatExpansion,
    ) -> Result<(), ()> {
        if let Some(previous) = self.get(source, tree) {
            return if previous == &expansion {
                Ok(())
            } else {
                Err(())
            };
        }
        self.by_tree.insert((source, tree), expansion);
        Ok(())
    }
}

#[derive(Clone, Default)]
pub(in crate::typer) struct LocalMethodIndex {
    pub(in crate::typer) symbols_by_tree: HashMap<(SourceId, TreeId<Untyped>), SymbolId>,
    pub(in crate::typer) definitions: HashMap<SymbolId, (SourceId, TreeId<Untyped>)>,
    pub(in crate::typer) scopes: HashMap<SymbolId, ScopeId>,
    pub(in crate::typer) declaration_contexts: HashMap<SymbolId, ExpressionContext>,
    pub(in crate::typer) parameter_symbols_by_tree: HashMap<(SourceId, TreeId<Untyped>), SymbolId>,
    pub(in crate::typer) parameter_definitions: HashMap<SymbolId, (SourceId, TreeId<Untyped>)>,
    pub(in crate::typer) parameter_contexts: HashMap<SymbolId, SourceContextId>,
    pub(in crate::typer) type_parameter_symbols_by_tree:
        HashMap<(SourceId, TreeId<Untyped>), SymbolId>,
    pub(in crate::typer) type_parameter_definitions: HashMap<SymbolId, (SourceId, TreeId<Untyped>)>,
    pub(in crate::typer) type_parameter_contexts: HashMap<SymbolId, SourceContextId>,
    extension_prefix_clauses_by_method: HashMap<SymbolId, Vec<Vec<TreeId<Untyped>>>>,
    extension_receiver_parameters_by_method_and_tree:
        HashMap<(SymbolId, TreeId<Untyped>), SymbolId>,
}

impl LocalMethodIndex {
    pub(in crate::typer) fn insert(
        &mut self,
        source: SourceId,
        tree: TreeId<Untyped>,
        symbol: SymbolId,
        scope: ScopeId,
        declaration_context: ExpressionContext,
        extension_prefix_clauses: Option<&[Vec<TreeId<Untyped>>]>,
    ) {
        self.symbols_by_tree.insert((source, tree), symbol);
        self.definitions.insert(symbol, (source, tree));
        self.scopes.insert(symbol, scope);
        self.declaration_contexts
            .insert(symbol, declaration_context);
        if let Some(clauses) = extension_prefix_clauses {
            self.extension_prefix_clauses_by_method
                .insert(symbol, clauses.to_vec());
        }
    }

    pub(in crate::typer) fn symbol_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.symbols_by_tree.get(&(source, tree)).copied()
    }

    pub(in crate::typer) fn definition(
        &self,
        symbol: SymbolId,
    ) -> Option<(SourceId, TreeId<Untyped>)> {
        self.definitions.get(&symbol).copied()
    }

    pub(in crate::typer) fn scope(&self, symbol: SymbolId) -> Option<ScopeId> {
        self.scopes.get(&symbol).copied()
    }

    pub(in crate::typer) fn declaration_context(
        &self,
        symbol: SymbolId,
    ) -> Option<ExpressionContext> {
        self.declaration_contexts.get(&symbol).copied()
    }

    pub(in crate::typer) fn extension_prefix_clauses(
        &self,
        method: SymbolId,
    ) -> Option<&[Vec<TreeId<Untyped>>]> {
        self.extension_prefix_clauses_by_method
            .get(&method)
            .map(Vec::as_slice)
    }

    pub(in crate::typer) fn insert_extension_receiver_parameter(
        &mut self,
        source: SourceId,
        method: SymbolId,
        tree: TreeId<Untyped>,
        symbol: SymbolId,
        context: SourceContextId,
    ) {
        self.extension_receiver_parameters_by_method_and_tree
            .insert((method, tree), symbol);
        self.parameter_definitions.insert(symbol, (source, tree));
        self.parameter_contexts.insert(symbol, context);
    }

    pub(in crate::typer) fn extension_receiver_parameter_symbol(
        &self,
        method: SymbolId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.extension_receiver_parameters_by_method_and_tree
            .get(&(method, tree))
            .copied()
    }

    pub(in crate::typer) fn contains_symbol(&self, symbol: SymbolId) -> bool {
        self.definitions.contains_key(&symbol)
    }

    pub(in crate::typer) fn insert_parameter(
        &mut self,
        source: SourceId,
        tree: TreeId<Untyped>,
        symbol: SymbolId,
        context: SourceContextId,
    ) {
        self.parameter_symbols_by_tree
            .insert((source, tree), symbol);
        self.parameter_definitions.insert(symbol, (source, tree));
        self.parameter_contexts.insert(symbol, context);
    }

    pub(in crate::typer) fn parameter_symbol_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.parameter_symbols_by_tree.get(&(source, tree)).copied()
    }

    pub(in crate::typer) fn parameter_definition(
        &self,
        symbol: SymbolId,
    ) -> Option<(SourceId, TreeId<Untyped>)> {
        self.parameter_definitions.get(&symbol).copied()
    }

    pub(in crate::typer) fn parameter_context(&self, symbol: SymbolId) -> Option<SourceContextId> {
        self.parameter_contexts.get(&symbol).copied()
    }

    pub(in crate::typer) fn insert_type_parameter(
        &mut self,
        source: SourceId,
        tree: TreeId<Untyped>,
        symbol: SymbolId,
        context: SourceContextId,
    ) {
        self.type_parameter_symbols_by_tree
            .insert((source, tree), symbol);
        self.type_parameter_definitions
            .insert(symbol, (source, tree));
        self.type_parameter_contexts.insert(symbol, context);
    }

    pub(in crate::typer) fn type_parameter_symbol_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.type_parameter_symbols_by_tree
            .get(&(source, tree))
            .copied()
    }

    pub(in crate::typer) fn type_parameter_definition(
        &self,
        symbol: SymbolId,
    ) -> Option<(SourceId, TreeId<Untyped>)> {
        self.type_parameter_definitions.get(&symbol).copied()
    }

    pub(in crate::typer) fn type_parameter_context(
        &self,
        symbol: SymbolId,
    ) -> Option<SourceContextId> {
        self.type_parameter_contexts.get(&symbol).copied()
    }
}

impl SourceTyper<'_> {
    fn patdef_deferred(&self, tree: TreeId<Untyped>) -> TyperError {
        TyperError::LocalBlockDeclarationDeferred {
            source: self.source,
            tree_index: tree.index(),
            kind: "pattern definition",
        }
    }

    fn patdef_binders(&self, patterns: &[TreeId<Untyped>]) -> Vec<(TreeId<Untyped>, Name)> {
        let mut binders = Vec::new();
        let mut names = std::collections::HashSet::new();
        let mut pending = patterns.iter().rev().copied().collect::<Vec<_>>();
        while let Some(tree) = pending.pop() {
            let Some(node) = self.arena.try_get(tree) else {
                continue;
            };
            match &node.kind {
                TreeKind::Ident(ident)
                    if !ident.backquoted
                        && super::patterns::is_variable_pattern_name(self.store, ident.name) =>
                {
                    if names.insert(ident.name) {
                        binders.push((tree, ident.name));
                    }
                }
                TreeKind::Bind(binding) => {
                    if self.store.names.resolve(binding.name.text()) != "_"
                        && names.insert(binding.name)
                    {
                        binders.push((tree, binding.name));
                    }
                    pending.push(binding.body);
                }
                TreeKind::NamedArg(argument) => pending.push(argument.arg),
                TreeKind::Typed(typed) => pending.push(typed.expr),
                TreeKind::Apply(application) => {
                    pending.extend(application.args.iter().rev().copied());
                }
                TreeKind::Alternative(alternative) => {
                    if let Some(first) = alternative.alternatives.first() {
                        pending.push(*first);
                    }
                }
                TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => pending.push(parens.inner),
                TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) => {
                    pending.extend(tuple.elements.iter().rev().copied());
                }
                TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) => {
                    pending.push(infix.right);
                    pending.push(infix.left);
                }
                TreeKind::UnApply(unapply) => {
                    pending.extend(unapply.patterns.iter().rev().copied());
                }
                _ => {}
            }
        }
        binders
    }

    fn type_local_patdef(
        &mut self,
        tree: TreeId<Untyped>,
        definition: &dotty_core::ast::PatDef,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TypedStatExpansion, TyperError> {
        let outcome = self.type_local_patdef_inner(
            tree,
            definition,
            position,
            context,
            info_journal,
            new_mappings,
        );
        let recorded = outcome
            .as_ref()
            .map(|_| ())
            .map_err(Self::patdef_attempt_failure);
        let key = (self.source, tree);
        match self.patdef_typing_attempts.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if entry.get().is_err() || recorded.is_ok() {
                    let _ = entry.insert(recorded);
                }
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(recorded);
            }
        }
        outcome
    }

    fn patdef_attempt_failure(error: &TyperError) -> String {
        match error {
            TyperError::LocalPatDefDeferred { kind, .. } => {
                format!("LocalPatDefDeferred::{kind}")
            }
            TyperError::PatDefAggregateArityDeferred {
                arity,
                max_supported,
                ..
            } => format!("PatDefAggregateArityDeferred::{arity}>{max_supported}"),
            _ => format!("{error:?}")
                .split([' ', '{', '('])
                .next()
                .unwrap_or("TyperError")
                .to_owned(),
        }
    }

    fn type_local_patdef_inner(
        &mut self,
        tree: TreeId<Untyped>,
        definition: &dotty_core::ast::PatDef,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TypedStatExpansion, TyperError> {
        use dotty_core::ast::Modifier;

        if let Some(expansion) = self.patdef_expansion_at(self.source, tree).cloned() {
            let binders = self.patdef_binders(&definition.patterns);
            if binders.is_empty() {
                return Ok(TypedStatExpansion {
                    anchor: expansion.anchor,
                    emitted: expansion.emitted,
                });
            }
            if binders.len() > super::patterns::MAX_CANONICAL_TUPLE_ARITY {
                return Err(TyperError::PatDefAggregateArityDeferred {
                    source: self.source,
                    tree_index: tree.index(),
                    arity: binders.len(),
                    max_supported: super::patterns::MAX_CANONICAL_TUPLE_ARITY,
                });
            }
            let Some(local_stack) = context.local_scopes else {
                return Err(TyperError::ExpressionLocalScopeStackMissing {
                    stack: ExpressionScopeId::new(
                        self.expression_scope_owner,
                        self.expression_scopes.len(),
                    ),
                });
            };
            let frame = self
                .expression_scopes
                .get(local_stack.index())
                .ok_or(TyperError::ExpressionLocalScopeStackMissing { stack: local_stack })?;
            if !frame.is_block_scope {
                return Err(TyperError::LocalValueOutsideBlock {
                    source: self.source,
                    tree_index: tree.index(),
                });
            }
            let scope = frame.scope;
            let mut bindings = Vec::with_capacity(binders.len());
            for (binder_tree, binder_name) in binders {
                let final_symbol = self
                    .local_symbols
                    .get(&(self.source, binder_tree))
                    .copied()
                    .ok_or_else(|| self.patdef_deferred(tree))?;
                let existing = self.store.scopes.get(scope).lookup_all(&binder_name);
                if !existing.is_empty() && !existing.contains(&final_symbol) {
                    return Err(TyperError::DuplicateLocalValue {
                        source: self.source,
                        tree_index: tree.index(),
                        name: binder_name,
                    });
                }
                bindings.push((binder_name, final_symbol, existing.is_empty()));
            }
            for (binder_name, final_symbol, should_enter) in bindings {
                if should_enter {
                    self.store
                        .scopes
                        .get_mut(scope)
                        .enter(binder_name, final_symbol);
                }
            }
            return Ok(TypedStatExpansion {
                anchor: expansion.anchor,
                emitted: expansion.emitted,
            });
        }
        let modifiers = &definition.modifiers.modifiers;
        if modifiers.contains(&Modifier::Lazy) {
            return Err(TyperError::LocalPatDefDeferred {
                source: self.source,
                tree_index: tree.index(),
                kind: "lazy",
            });
        }
        let is_mutable = modifiers.contains(&Modifier::Var);
        if modifiers.iter().any(|modifier| *modifier != Modifier::Var)
            || definition.modifiers.visibility.is_some()
            || !definition.modifiers.annotations.is_empty()
        {
            return Err(TyperError::LocalPatDefDeferred {
                source: self.source,
                tree_index: tree.index(),
                kind: "modifiers",
            });
        }
        if definition.patterns.len() != 1 {
            return Err(TyperError::LocalPatDefDeferred {
                source: self.source,
                tree_index: tree.index(),
                kind: "multiple source patterns",
            });
        }
        if definition.rhs.is_none() {
            return Err(TyperError::LocalPatDefDeferred {
                source: self.source,
                tree_index: tree.index(),
                kind: "missing right-hand side",
            });
        }
        if !self
            .arena
            .try_get(definition.tpt)
            .is_some_and(|tpt| matches!(tpt.kind, TreeKind::TypeTree(_)))
        {
            return Err(TyperError::LocalPatDefDeferred {
                source: self.source,
                tree_index: tree.index(),
                kind: "explicit type",
            });
        }
        let binders = self.patdef_binders(&definition.patterns);
        if binders.len() > super::patterns::MAX_CANONICAL_TUPLE_ARITY {
            return Err(TyperError::PatDefAggregateArityDeferred {
                source: self.source,
                tree_index: tree.index(),
                arity: binders.len(),
                max_supported: super::patterns::MAX_CANONICAL_TUPLE_ARITY,
            });
        }
        let scope = if !binders.is_empty() {
            let Some(local_stack) = context.local_scopes else {
                return Err(TyperError::ExpressionLocalScopeStackMissing {
                    stack: ExpressionScopeId::new(
                        self.expression_scope_owner,
                        self.expression_scopes.len(),
                    ),
                });
            };
            let frame = self
                .expression_scopes
                .get(local_stack.index())
                .ok_or(TyperError::ExpressionLocalScopeStackMissing { stack: local_stack })?;
            if !frame.is_block_scope {
                return Err(TyperError::LocalValueOutsideBlock {
                    source: self.source,
                    tree_index: tree.index(),
                });
            }
            let scope = frame.scope;
            for (_, binder_name) in &binders {
                if !self
                    .store
                    .scopes
                    .get(scope)
                    .lookup_all(binder_name)
                    .is_empty()
                {
                    return Err(TyperError::DuplicateLocalValue {
                        source: self.source,
                        tree_index: tree.index(),
                        name: *binder_name,
                    });
                }
            }
            Some(scope)
        } else {
            None
        };
        let rhs = definition.rhs.expect("checked above");
        let typed_selector =
            self.type_value_expression_inner(rhs, context, info_journal, new_mappings)?;
        let selector_type = self.typed_arena.get(typed_selector).ty;
        let selector_type = self
            .pattern_selector_type(selector_type, info_journal)
            .map_err(|error| TyperError::MatchSelectorTypeCannotBeAdapted {
                source: self.source,
                tree_index: tree.index(),
                error: Box::new(error),
            })?;

        // The source pattern engine owns the temporary case binding and its
        // SourceTypedIndex mapping. The scope is discarded before the final
        // block-local symbol is entered.
        let expression_scope_depth = self.expression_scopes.len();
        let prior_pattern_binding_count = self.pattern_bindings.by_tree.len();
        let case_context = self.push_case_scope(context)?;
        let typed_pattern = self.type_pattern(
            definition.patterns[0],
            selector_type,
            case_context,
            info_journal,
            new_mappings,
        );
        self.expression_scopes.truncate(expression_scope_depth);
        let typed_pattern = typed_pattern?;
        if binders.is_empty() {
            let actual_pattern_binding_count = self
                .pattern_bindings
                .by_tree
                .len()
                .saturating_sub(prior_pattern_binding_count);
            if actual_pattern_binding_count != 0 {
                return Err(TyperError::PatDefBinderInventoryConflict {
                    source: self.source,
                    tree_index: tree.index(),
                    expected: 0,
                    actual: actual_pattern_binding_count,
                });
            }
            let unit_body = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).literal(
                dotty_core::Constant::Unit,
                self.definitions.unit,
                None,
            );
            let case = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).case_def(
                typed_pattern,
                None,
                unit_body,
                self.definitions.unit,
                position,
            );
            let extraction = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                .match_expr(typed_selector, vec![case], self.definitions.unit, position);
            let expansion = TypedStatExpansion::one(extraction);
            self.record_patdef_expansion(
                tree,
                context,
                expansion.clone(),
                Vec::new(),
                new_mappings,
            )?;
            return Ok(expansion);
        }
        let actual_pattern_binding_count = self
            .pattern_bindings
            .by_tree
            .len()
            .saturating_sub(prior_pattern_binding_count);
        if actual_pattern_binding_count != binders.len() {
            return Err(TyperError::PatDefBinderInventoryConflict {
                source: self.source,
                tree_index: tree.index(),
                expected: binders.len(),
                actual: actual_pattern_binding_count,
            });
        }
        if binders.len() > 1 {
            let mut component_types = Vec::with_capacity(binders.len());
            let mut component_values = Vec::with_capacity(binders.len());
            for (binder_tree, binder_name) in &binders {
                let temporary = self
                    .pattern_bindings
                    .by_tree
                    .get(&(self.source, *binder_tree))
                    .copied()
                    .ok_or_else(|| self.patdef_deferred(tree))?;
                if !self.store.symbols.contains(temporary) {
                    return Err(self.patdef_deferred(tree));
                }
                let SymbolInfo::Complete(binder_type) = self.store.symbols.get(temporary).info
                else {
                    return Err(self.patdef_deferred(tree));
                };
                let body_type = self.pattern_binding_term_ref(temporary);
                let binder_position = self
                    .arena
                    .try_get(*binder_tree)
                    .and_then(|node| node.position);
                let value = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).ident(
                    *binder_name,
                    body_type,
                    binder_position,
                );
                component_types.push(binder_type);
                component_values.push(value);
            }
            let (aggregate_body, aggregate_type) = self.construct_canonical_tuple_value(
                tree,
                &component_types,
                component_values,
                info_journal,
            )?;
            let case = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).case_def(
                typed_pattern,
                None,
                aggregate_body,
                aggregate_type,
                position,
            );
            let extraction = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                .match_expr(typed_selector, vec![case], aggregate_type, position);
            let synthetic_name = Name::new(
                self.store
                    .names
                    .intern(&format!("$patdef${}", tree.index())),
                Namespace::Term,
            );
            let synthetic_symbol = self.store.symbols.alloc(dotty_core::Symbol {
                name: synthetic_name,
                owner: Some(context.owner),
                kind: SymbolKind::Local,
                flags: SymbolFlags::SYNTHETIC,
                visibility: dotty_core::Visibility::Public,
                info: SymbolInfo::Complete(aggregate_type),
                origin: SymbolOrigin::Synthetic,
                annotations: Vec::new(),
                position,
                links: dotty_core::SymbolLinks::default(),
            });
            let synthetic_tpt = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                .type_tree(aggregate_type, position);
            let synthetic_val = self.typed_arena.alloc(Tree {
                kind: TreeKind::ValDef(ValDef {
                    name: TermName::new(synthetic_name.text()),
                    tpt: synthetic_tpt,
                    rhs: Some(extraction),
                    metadata: (),
                }),
                position,
                ty: aggregate_type,
            });
            let synthetic_ref = self.pattern_binding_term_ref(synthetic_symbol);
            let synthetic_value = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                .ident(synthetic_name, synthetic_ref, position);
            let scope = scope.ok_or_else(|| self.patdef_deferred(tree))?;
            let mut emitted = Vec::with_capacity(binders.len() + 1);
            emitted.push(synthetic_val);
            for (component_index, ((binder_tree, binder_name), binder_type)) in binders
                .iter()
                .zip(component_types.iter().copied())
                .enumerate()
            {
                let selection = self.select_canonical_tuple_component(
                    tree,
                    synthetic_value,
                    aggregate_type,
                    component_index,
                    binder_type,
                    info_journal,
                )?;
                let binder_position = self
                    .arena
                    .try_get(*binder_tree)
                    .and_then(|node| node.position);
                let final_symbol = self.store.symbols.alloc(dotty_core::Symbol {
                    name: *binder_name,
                    owner: Some(context.owner),
                    kind: SymbolKind::Local,
                    flags: if is_mutable {
                        SymbolFlags::MUTABLE
                    } else {
                        SymbolFlags::EMPTY
                    },
                    visibility: dotty_core::Visibility::Public,
                    info: SymbolInfo::Complete(binder_type),
                    origin: SymbolOrigin::Source(self.source),
                    annotations: Vec::new(),
                    position: binder_position,
                    links: dotty_core::SymbolLinks::default(),
                });
                self.store
                    .scopes
                    .get_mut(scope)
                    .enter(*binder_name, final_symbol);
                self.local_symbols
                    .insert((self.source, *binder_tree), final_symbol);
                let typed_tpt = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                    .type_tree(binder_type, binder_position);
                let typed_val = self.typed_arena.alloc(Tree {
                    kind: TreeKind::ValDef(ValDef {
                        name: TermName::new(binder_name.text()),
                        tpt: typed_tpt,
                        rhs: Some(selection),
                        metadata: (),
                    }),
                    position: binder_position,
                    ty: binder_type,
                });
                emitted.push(typed_val);
            }
            let expansion = TypedStatExpansion {
                anchor: synthetic_val,
                emitted,
            };
            self.record_patdef_expansion(
                tree,
                context,
                expansion.clone(),
                vec![synthetic_symbol],
                new_mappings,
            )?;
            return Ok(expansion);
        }
        let (binder_tree, binder_name) = binders
            .first()
            .copied()
            .ok_or_else(|| self.patdef_deferred(tree))?;
        let temporary = self
            .pattern_bindings
            .by_tree
            .get(&(self.source, binder_tree))
            .copied()
            .ok_or_else(|| self.patdef_deferred(tree))?;
        if !self.store.symbols.contains(temporary) {
            return Err(self.patdef_deferred(tree));
        }
        let binder_type = match self.store.symbols.get(temporary).info {
            SymbolInfo::Complete(ty) => ty,
            _ => return Err(self.patdef_deferred(tree)),
        };
        let body_type = self.pattern_binding_term_ref(temporary);
        let typed_body = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).ident(
            binder_name,
            body_type,
            self.arena
                .try_get(binder_tree)
                .and_then(|node| node.position),
        );
        let case = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).case_def(
            typed_pattern,
            None,
            typed_body,
            body_type,
            position,
        );
        let match_type = self.widen_expression_type_journaled(body_type, info_journal, 0)?;
        let extraction = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).match_expr(
            typed_selector,
            vec![case],
            match_type,
            position,
        );
        if match_type != binder_type {
            return Err(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: tree.index(),
                kind: "pattern binding result type",
            });
        }
        self.validate_inferred_local_value_type(binder_type, tree.index())?;

        let binder_position = self
            .arena
            .try_get(binder_tree)
            .and_then(|node| node.position);
        let final_symbol = self.store.symbols.alloc(dotty_core::Symbol {
            name: binder_name,
            owner: Some(context.owner),
            kind: SymbolKind::Local,
            flags: if is_mutable {
                SymbolFlags::MUTABLE
            } else {
                SymbolFlags::EMPTY
            },
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Complete(binder_type),
            origin: SymbolOrigin::Source(self.source),
            annotations: Vec::new(),
            position: binder_position,
            links: dotty_core::SymbolLinks::default(),
        });
        let scope = scope.ok_or_else(|| self.patdef_deferred(tree))?;
        self.store
            .scopes
            .get_mut(scope)
            .enter(binder_name, final_symbol);
        self.local_symbols
            .insert((self.source, binder_tree), final_symbol);
        let typed_tpt =
            self.reify_inferred_local_type_tree(definition.tpt, binder_type, new_mappings)?;
        let typed_val = self.typed_arena.alloc(Tree {
            kind: TreeKind::ValDef(ValDef {
                name: TermName::new(binder_name.text()),
                tpt: typed_tpt,
                rhs: Some(extraction),
                metadata: (),
            }),
            position,
            ty: binder_type,
        });
        let expansion = TypedStatExpansion::one(typed_val);
        self.record_patdef_expansion(tree, context, expansion.clone(), Vec::new(), new_mappings)?;
        Ok(expansion)
    }

    /// Records the single source mapping for a successfully lowered PatDef.
    /// Callers run this inside an expression transaction so the expansion,
    /// source mapping, and any local symbols commit or roll back together.
    #[allow(dead_code)] // The PatDef lowering issues consume this entry point.
    pub(in crate::typer) fn record_patdef_expansion(
        &mut self,
        source_tree: TreeId<Untyped>,
        context: ExpressionContext,
        expansion: TypedStatExpansion,
        synthetic_symbols: Vec<SymbolId>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<(), TyperError> {
        let Some(source_node) = self.arena.try_get(source_tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: source_tree.index(),
            });
        };
        if !matches!(
            source_node.kind,
            TreeKind::PhaseSpecific(UntypedNode::PatDef(_))
        ) || !expansion.emitted.contains(&expansion.anchor)
            || expansion
                .emitted
                .iter()
                .any(|tree| self.typed_arena.try_get(*tree).is_none())
            || synthetic_symbols.iter().any(|symbol| {
                !self.store.symbols.contains(*symbol)
                    || {
                        let entry = self.store.symbols.get(*symbol);
                        entry.kind != SymbolKind::Local
                            || entry.owner != Some(context.owner)
                            || entry.origin != SymbolOrigin::Synthetic
                    }
                    || self.local_symbols.values().any(|local| local == symbol)
            })
        {
            return Err(TyperError::PatDefExpansionConflict {
                source: self.source,
                tree_index: source_tree.index(),
            });
        }

        if let Some(existing) = self.typed_index.get(self.source, source_tree)
            && existing != expansion.anchor
        {
            return Err(TyperError::ConflictingTypedExpression {
                source: self.source,
                tree_index: source_tree.index(),
                existing: existing.index(),
                attempted: expansion.anchor.index(),
            });
        }
        let record = PatDefStatExpansion {
            anchor: expansion.anchor,
            emitted: expansion.emitted,
            synthetic_symbols,
        };
        if self
            .patdef_expansions
            .get(self.source, source_tree)
            .is_some_and(|existing| existing != &record)
        {
            return Err(TyperError::PatDefExpansionConflict {
                source: self.source,
                tree_index: source_tree.index(),
            });
        }

        let is_new_mapping = self.typed_index.get(self.source, source_tree).is_none();
        self.typed_index
            .insert(self.source, source_tree, record.anchor)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        self.patdef_expansions
            .insert(self.source, source_tree, record)
            .map_err(|()| TyperError::PatDefExpansionConflict {
                source: self.source,
                tree_index: source_tree.index(),
            })?;
        if is_new_mapping {
            new_mappings.push((self.source, source_tree));
        }
        Ok(())
    }

    #[allow(dead_code)] // Tests and the following PatDef issues inspect this record.
    pub(in crate::typer) fn patdef_expansion_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<&PatDefStatExpansion> {
        self.patdef_expansions.get(source, tree)
    }

    pub(in crate::typer) fn preindex_local_methods(
        &mut self,
        stats: &[TreeId<Untyped>],
        declaration_context: ExpressionContext,
        block_scope: ScopeId,
    ) -> Result<(), TyperError> {
        for tree in stats {
            let Some(source_tree) = self.arena.try_get(*tree) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: tree.index(),
                });
            };
            match &source_tree.kind {
                TreeKind::DefDef(definition) => self.preindex_local_method(
                    *tree,
                    definition,
                    source_tree.position,
                    declaration_context,
                    block_scope,
                    None,
                ),
                TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) => {
                    if extension.methods.len() != 1 || extension.param_clauses.len() != 1 {
                        continue;
                    }
                    let method_tree = extension.methods[0];
                    let Some(method_node) = self.arena.try_get(method_tree) else {
                        return Err(TyperError::TreeOutsideArena {
                            source: self.source,
                            tree_index: method_tree.index(),
                        });
                    };
                    let TreeKind::DefDef(definition) = &method_node.kind else {
                        continue;
                    };
                    let receiver_clause = &extension.param_clauses[0];
                    let supported_receiver_clause = if receiver_clause.len() == 1 {
                        let parameter_tree = receiver_clause[0];
                        let Some(parameter_node) = self.arena.try_get(parameter_tree) else {
                            return Err(TyperError::TreeOutsideArena {
                                source: self.source,
                                tree_index: parameter_tree.index(),
                            });
                        };
                        matches!(
                            &parameter_node.kind,
                            TreeKind::ValDef(parameter)
                                if !parameter.metadata.modifiers.iter().any(|modifier| {
                                    matches!(modifier, Modifier::Given | Modifier::Implicit)
                                })
                        )
                    } else {
                        false
                    };
                    if !supported_receiver_clause
                        || !definition.type_params.is_empty()
                        || definition.rhs.is_none()
                        || self
                            .store
                            .names
                            .resolve(definition.name.as_name().text())
                            .ends_with(':')
                    {
                        continue;
                    }
                    self.preindex_local_method(
                        method_tree,
                        definition,
                        method_node.position,
                        declaration_context,
                        block_scope,
                        Some(&extension.param_clauses),
                    );
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn preindex_local_method(
        &mut self,
        tree: TreeId<Untyped>,
        definition: &dotty_core::ast::DefDef<Untyped>,
        position: Option<SourceSpan>,
        declaration_context: ExpressionContext,
        block_scope: ScopeId,
        extension_prefix_clauses: Option<&[Vec<TreeId<Untyped>>]>,
    ) {
        let name = *definition.name.as_name();
        if let Some(symbol) = self.local_methods.symbol_at(self.source, tree) {
            self.store.scopes.get_mut(block_scope).enter(name, symbol);
            return;
        }
        let mut flags = source_method_flags(&definition.metadata.modifiers);
        if extension_prefix_clauses.is_some() {
            flags = flags | SymbolFlags::EXTENSION;
        }
        let symbol = self.store.symbols.alloc(dotty_core::Symbol {
            name,
            owner: Some(declaration_context.owner),
            kind: SymbolKind::Method,
            flags,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Source(self.source),
            annotations: Vec::new(),
            position,
            links: dotty_core::SymbolLinks::default(),
        });
        let method_scope = self
            .store
            .scopes
            .alloc(dotty_core::Scope::new(Some(symbol)));
        self.store.scopes.get_mut(block_scope).enter(name, symbol);
        self.local_methods.insert(
            self.source,
            tree,
            symbol,
            method_scope,
            declaration_context,
            extension_prefix_clauses,
        );
        if let Some(clauses) = extension_prefix_clauses {
            for parameter_tree in clauses.iter().flatten().copied() {
                let Some(parameter_node) = self.arena.try_get(parameter_tree) else {
                    continue;
                };
                let TreeKind::ValDef(parameter) = &parameter_node.kind else {
                    continue;
                };
                let parameter_name = *parameter.name.as_name();
                let parameter = self.store.symbols.alloc(dotty_core::Symbol {
                    name: parameter_name,
                    owner: Some(symbol),
                    kind: SymbolKind::Parameter,
                    flags: source_method_flags(&parameter.metadata.modifiers),
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
                    .enter(parameter_name, parameter);
                self.local_methods.insert_extension_receiver_parameter(
                    self.source,
                    symbol,
                    parameter_tree,
                    parameter,
                    declaration_context.lexical,
                );
            }
        }
    }

    pub(in crate::typer) fn type_local_value(
        &mut self,
        tree: TreeId<Untyped>,
        definition: &dotty_core::ast::ValDef<Untyped>,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let Some(local_stack) = context.local_scopes else {
            return Err(TyperError::ExpressionLocalScopeStackMissing {
                stack: ExpressionScopeId::new(
                    self.expression_scope_owner,
                    self.expression_scopes.len(),
                ),
            });
        };
        let frame = self
            .expression_scopes
            .get(local_stack.index())
            .ok_or(TyperError::ExpressionLocalScopeStackMissing { stack: local_stack })?;
        if !frame.is_block_scope {
            return Err(TyperError::LocalValueOutsideBlock {
                source: self.source,
                tree_index: tree.index(),
            });
        }
        let scope = frame.scope;
        let name = *definition.name.as_name();
        if !self.store.scopes.get(scope).lookup_all(&name).is_empty() {
            return Err(TyperError::DuplicateLocalValue {
                source: self.source,
                tree_index: tree.index(),
                name,
            });
        }
        let Some(source_tpt) = self.arena.try_get(definition.tpt) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: definition.tpt.index(),
            });
        };
        let inferred = matches!(source_tpt.kind, TreeKind::TypeTree(_));
        let rhs = definition
            .rhs
            .ok_or(TyperError::LocalValueRightHandSideMissing {
                source: self.source,
                tree_index: tree.index(),
            })?;
        let declared_type = if inferred {
            None
        } else {
            self.active_local_import_scopes
                .push((context.lexical, context.local_scopes));
            let projected =
                self.type_of_tpt_inner_journaled(definition.tpt, context.lexical, info_journal);
            self.active_local_import_scopes.pop();
            Some(projected?)
        };

        // Scala 3 gives a local definition scope over the entire statement
        // sequence, including its own initializer. Enter the binding now so it
        // shadows outer names, and diagnose a reference to it while initializing.
        let symbol = self.store.symbols.alloc(dotty_core::Symbol {
            name,
            owner: Some(context.owner),
            kind: SymbolKind::Local,
            flags: if definition
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::Var)
            {
                SymbolFlags::MUTABLE
            } else {
                SymbolFlags::EMPTY
            },
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Source(self.source),
            annotations: Vec::new(),
            position,
            links: dotty_core::SymbolLinks::default(),
        });
        if !inferred {
            self.store.scopes.get_mut(scope).enter(name, symbol);
            self.initializing_local_symbols.insert(symbol);
        }
        let typed_rhs_result =
            self.type_value_expression_inner(rhs, context, info_journal, new_mappings);
        if !inferred {
            self.initializing_local_symbols.remove(&symbol);
        }
        let typed_rhs = typed_rhs_result?;
        let rhs_type = self.typed_arena.get(typed_rhs).ty;
        let actual = match declared_type {
            Some(expected) => {
                self.adapt_expression_type_to_expected(rhs_type, expected, info_journal)?
            }
            None => self.widen_expression_type_journaled(rhs_type, info_journal, 0)?,
        };
        let declared_type = if let Some(declared_type) = declared_type {
            match self.conforms(actual, declared_type) {
                Ok(true) => {}
                Ok(false) => {
                    return Err(TyperError::LocalValueTypeMismatch {
                        source: self.source,
                        tree_index: tree.index(),
                        actual,
                        expected: declared_type,
                    });
                }
                Err(error) => {
                    return Err(TyperError::LocalValueConformanceUnsupported {
                        source: self.source,
                        tree_index: tree.index(),
                        actual,
                        expected: declared_type,
                        error: Box::new(error),
                    });
                }
            }
            declared_type
        } else {
            self.validate_inferred_local_value_type(actual, tree.index())?;
            actual
        };
        self.store
            .symbols
            .set_info(symbol, SymbolInfo::Complete(declared_type));
        if inferred {
            self.store.scopes.get_mut(scope).enter(name, symbol);
        }
        self.local_symbols.insert((self.source, tree), symbol);

        let typed_tpt = if inferred {
            self.reify_inferred_local_type_tree(definition.tpt, declared_type, new_mappings)?
        } else {
            self.reify_type_argument(definition.tpt, declared_type, new_mappings)?
        };
        Ok(self.typed_arena.alloc(Tree {
            kind: TreeKind::ValDef(ValDef {
                name: definition.name,
                tpt: typed_tpt,
                rhs: Some(typed_rhs),
                metadata: (),
            }),
            position,
            ty: declared_type,
        }))
    }

    pub(in crate::typer) fn type_local_method_definition(
        &mut self,
        tree: TreeId<Untyped>,
        definition: &dotty_core::ast::DefDef<Untyped>,
        position: Option<SourceSpan>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(typed) = self.typed_index.get(self.source, tree) {
            return Ok(typed);
        }
        let method = self.local_methods.symbol_at(self.source, tree).ok_or(
            TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: tree.index(),
                kind: "method definition",
            },
        )?;
        let signature = match *self.store.symbols.info(method) {
            SymbolInfo::Complete(signature) => signature,
            SymbolInfo::Missing => self.complete_symbol_inner(method, info_journal)?,
            SymbolInfo::Deferred(_) => {
                return Err(TyperError::DeferredSymbolCompletion { symbol: method });
            }
            SymbolInfo::Error => return Err(TyperError::SymbolAlreadyErrored { symbol: method }),
        };
        let is_extension = self
            .store
            .symbols
            .get(method)
            .flags
            .contains(SymbolFlags::EXTENSION);
        let mut result = signature;
        if !definition.type_params.is_empty() {
            result = match self.store.types.try_get(result) {
                Some(Type::Poly(poly)) if poly.params.len() == definition.type_params.len() => {
                    poly.result
                }
                _ => {
                    return Err(TyperError::LocalBlockDeclarationDeferred {
                        source: self.source,
                        tree_index: tree.index(),
                        kind: "method definition",
                    });
                }
            };
        }
        let extension_prefix_count = usize::from(is_extension);
        for _ in 0..extension_prefix_count + definition.value_param_clauses.len() {
            result = match self.store.types.try_get(result) {
                Some(Type::Method(method_type)) => method_type.result,
                _ => {
                    return Err(TyperError::LocalBlockDeclarationDeferred {
                        source: self.source,
                        tree_index: tree.index(),
                        kind: "method definition",
                    });
                }
            };
        }
        let mut typed_type_params = Vec::with_capacity(definition.type_params.len());
        for type_parameter_tree in &definition.type_params {
            let Some(parameter_node) = self.arena.try_get(*type_parameter_tree).cloned() else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: type_parameter_tree.index(),
                });
            };
            let TreeKind::TypeDef(parameter) = parameter_node.kind else {
                return Err(TyperError::LocalBlockDeclarationDeferred {
                    source: self.source,
                    tree_index: type_parameter_tree.index(),
                    kind: "method type parameter",
                });
            };
            let parameter_symbol = self
                .local_methods
                .type_parameter_symbol_at(self.source, *type_parameter_tree)
                .ok_or(TyperError::MethodParameterSymbolMissing {
                    method,
                    parameter_tree_index: type_parameter_tree.index(),
                })?;
            let bounds = match *self.store.symbols.info(parameter_symbol) {
                SymbolInfo::Complete(bounds) => bounds,
                _ => {
                    return Err(TyperError::MethodParameterSymbolMissing {
                        method,
                        parameter_tree_index: type_parameter_tree.index(),
                    });
                }
            };
            let (low, high) = match self.store.types.try_get(bounds).cloned() {
                Some(Type::Bounds { low, high }) => (low, high),
                _ => {
                    return Err(TyperError::LocalBlockDeclarationDeferred {
                        source: self.source,
                        tree_index: type_parameter_tree.index(),
                        kind: "method type parameter bounds",
                    });
                }
            };
            let Some(rhs_node) = self.arena.try_get(parameter.rhs) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: parameter.rhs.index(),
                });
            };
            let TreeKind::TypeBoundsTree(source_bounds) = rhs_node.kind else {
                return Err(TyperError::LocalBlockDeclarationDeferred {
                    source: self.source,
                    tree_index: parameter.rhs.index(),
                    kind: "method type parameter bounds",
                });
            };
            let typed_low = match source_bounds.low {
                Some(source_tree) => {
                    Some(self.reify_constructor_type_tree(source_tree, low, new_mappings)?)
                }
                None => None,
            };
            let typed_high = match source_bounds.high {
                Some(source_tree) => {
                    Some(self.reify_constructor_type_tree(source_tree, high, new_mappings)?)
                }
                None => None,
            };
            let typed_rhs = self.typed_arena.alloc(Tree {
                kind: TreeKind::TypeBoundsTree(TypeBoundsTree {
                    low: typed_low,
                    high: typed_high,
                    alias: None,
                }),
                position: rhs_node.position,
                ty: bounds,
            });
            self.typed_index
                .insert(self.source, parameter.rhs, typed_rhs)
                .map_err(|error| TyperError::ConflictingTypedExpression {
                    source: error.source,
                    tree_index: error.untyped.index(),
                    existing: error.existing.index(),
                    attempted: error.attempted.index(),
                })?;
            new_mappings.push((self.source, parameter.rhs));
            let typed_parameter = self.typed_arena.alloc(Tree {
                kind: TreeKind::TypeDef(TypeDef {
                    name: parameter.name,
                    rhs: typed_rhs,
                    metadata: (),
                    variance: parameter.variance,
                }),
                position: parameter_node.position,
                ty: self.store.types.alloc(Type::TypeRef {
                    prefix: self.definitions.no_prefix,
                    target: TypeRefTarget::Symbol(parameter_symbol),
                }),
            });
            self.typed_index
                .insert(self.source, *type_parameter_tree, typed_parameter)
                .map_err(|error| TyperError::ConflictingTypedExpression {
                    source: error.source,
                    tree_index: error.untyped.index(),
                    existing: error.existing.index(),
                    attempted: error.attempted.index(),
                })?;
            new_mappings.push((self.source, *type_parameter_tree));
            typed_type_params.push(typed_parameter);
        }

        let mut typed_clauses =
            Vec::with_capacity(definition.value_param_clauses.len() + extension_prefix_count);
        if is_extension {
            let receiver_tree = self
                .extension_prefix_clauses(method)
                .and_then(|clauses| {
                    (clauses.len() == 1 && clauses[0].len() == 1).then_some(clauses[0][0])
                })
                .ok_or(TyperError::LocalBlockDeclarationDeferred {
                    source: self.source,
                    tree_index: tree.index(),
                    kind: "extension methods",
                })?;
            let Some(receiver_node) = self.arena.try_get(receiver_tree).cloned() else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: receiver_tree.index(),
                });
            };
            let TreeKind::ValDef(receiver) = receiver_node.kind else {
                return Err(TyperError::LocalBlockDeclarationDeferred {
                    source: self.source,
                    tree_index: receiver_tree.index(),
                    kind: "extension receiver",
                });
            };
            let receiver_symbol = self
                .local_methods
                .extension_receiver_parameter_symbol(method, receiver_tree)
                .ok_or(TyperError::MethodParameterSymbolMissing {
                    method,
                    parameter_tree_index: receiver_tree.index(),
                })?;
            let receiver_type = match *self.store.symbols.info(receiver_symbol) {
                SymbolInfo::Complete(ty) => ty,
                _ => {
                    return Err(TyperError::MethodParameterSymbolMissing {
                        method,
                        parameter_tree_index: receiver_tree.index(),
                    });
                }
            };
            let typed_tpt =
                self.reify_constructor_type_tree(receiver.tpt, receiver_type, new_mappings)?;
            let typed_receiver = self.typed_arena.alloc(Tree {
                kind: TreeKind::ValDef(ValDef {
                    name: receiver.name,
                    tpt: typed_tpt,
                    rhs: None,
                    metadata: (),
                }),
                position: receiver_node.position,
                ty: receiver_type,
            });
            self.typed_index
                .insert(self.source, receiver_tree, typed_receiver)
                .map_err(|error| TyperError::ConflictingTypedExpression {
                    source: error.source,
                    tree_index: error.untyped.index(),
                    existing: error.existing.index(),
                    attempted: error.attempted.index(),
                })?;
            new_mappings.push((self.source, receiver_tree));
            typed_clauses.push(vec![typed_receiver]);
        }
        for clause in &definition.value_param_clauses {
            let mut typed_parameters = Vec::with_capacity(clause.len());
            for parameter_tree in clause {
                let Some(parameter_node) = self.arena.try_get(*parameter_tree).cloned() else {
                    return Err(TyperError::TreeOutsideArena {
                        source: self.source,
                        tree_index: parameter_tree.index(),
                    });
                };
                let TreeKind::ValDef(parameter) = parameter_node.kind else {
                    return Err(TyperError::LocalBlockDeclarationDeferred {
                        source: self.source,
                        tree_index: parameter_tree.index(),
                        kind: "method parameter",
                    });
                };
                if let Some(typed) = self.typed_index.get(self.source, *parameter_tree) {
                    typed_parameters.push(typed);
                    continue;
                }
                let parameter_symbol = self
                    .local_methods
                    .parameter_symbol_at(self.source, *parameter_tree)
                    .ok_or(TyperError::MethodParameterSymbolMissing {
                        method,
                        parameter_tree_index: parameter_tree.index(),
                    })?;
                let parameter_type = match *self.store.symbols.info(parameter_symbol) {
                    SymbolInfo::Complete(ty) => ty,
                    _ => {
                        return Err(TyperError::MethodParameterSymbolMissing {
                            method,
                            parameter_tree_index: parameter_tree.index(),
                        });
                    }
                };
                let typed_tpt =
                    self.reify_constructor_type_tree(parameter.tpt, parameter_type, new_mappings)?;
                let typed_parameter = self.typed_arena.alloc(Tree {
                    kind: TreeKind::ValDef(ValDef {
                        name: parameter.name,
                        tpt: typed_tpt,
                        rhs: None,
                        metadata: (),
                    }),
                    position: parameter_node.position,
                    ty: parameter_type,
                });
                self.typed_index
                    .insert(self.source, *parameter_tree, typed_parameter)
                    .map_err(|error| TyperError::ConflictingTypedExpression {
                        source: error.source,
                        tree_index: error.untyped.index(),
                        existing: error.existing.index(),
                        attempted: error.attempted.index(),
                    })?;
                new_mappings.push((self.source, *parameter_tree));
                typed_parameters.push(typed_parameter);
            }
            typed_clauses.push(typed_parameters);
        }

        let typed_result =
            self.reify_constructor_type_tree(definition.tpt, result, new_mappings)?;
        let rhs = definition
            .rhs
            .ok_or(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: tree.index(),
                kind: "method definition without body",
            })?;
        let method_context = self.expression_context_for(method)?;
        let declaration_context = self
            .local_methods
            .declaration_context(method)
            .ok_or(TyperError::DeclarationContextMissing { symbol: method })?;
        let method_scope = self.method_scope(method)?;
        if !definition.type_params.is_empty() {
            let type_parameters = definition
                .type_params
                .iter()
                .map(|parameter_tree| {
                    self.local_methods
                        .type_parameter_symbol_at(self.source, *parameter_tree)
                        .ok_or(TyperError::MethodParameterSymbolMissing {
                            method,
                            parameter_tree_index: parameter_tree.index(),
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            self.active_local_type_binders
                .push((signature, type_parameters));
        }
        self.active_local_type_scopes
            .push((declaration_context.lexical, method_scope));
        self.active_local_import_scopes.push((
            declaration_context.lexical,
            declaration_context.local_scopes,
        ));
        let typed_rhs = self.type_expression_expected_inner(
            rhs,
            method_context,
            result,
            info_journal,
            new_mappings,
        );
        self.active_local_import_scopes.pop();
        self.active_local_type_scopes.pop();
        if !definition.type_params.is_empty() {
            self.active_local_type_binders.pop();
        }
        let typed_rhs = typed_rhs?;
        let definition_type = self.store.types.alloc(Type::TermRef {
            prefix: self.definitions.no_prefix,
            target: TermRefTarget::Symbol(method),
        });
        let source_param_clause_order =
            definition.source_param_clause_order.as_ref().map(|order| {
                let mut lowered = Vec::with_capacity(order.len() + extension_prefix_count);
                if is_extension {
                    lowered.push(DefParamClauseOrder::ValueParams(0));
                }
                lowered.extend(order.iter().map(|entry| match entry {
                    DefParamClauseOrder::TypeParams(range) => {
                        DefParamClauseOrder::TypeParams(range.clone())
                    }
                    DefParamClauseOrder::ValueParams(index) => {
                        DefParamClauseOrder::ValueParams(index + extension_prefix_count)
                    }
                }));
                lowered
            });
        let typed = self.typed_arena.alloc(Tree {
            kind: TreeKind::DefDef(DefDef {
                name: definition.name,
                type_params: typed_type_params,
                value_param_clauses: typed_clauses,
                source_param_clause_order,
                tpt: typed_result,
                rhs: Some(typed_rhs),
                metadata: (),
            }),
            position,
            ty: definition_type,
        });
        self.typed_index
            .insert(self.source, tree, typed)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        new_mappings.push((self.source, tree));
        Ok(typed)
    }

    pub(in crate::typer) fn validate_inferred_local_value_type(
        &self,
        inferred: TypeId,
        tree_index: u32,
    ) -> Result<(), TyperError> {
        if !self.is_valid_inferred_value_type(inferred) {
            return Err(TyperError::InvalidInferredLocalValueType {
                source: self.source,
                tree_index,
                inferred,
            });
        }
        Ok(())
    }

    pub(in crate::typer) fn is_valid_inferred_value_type(&self, inferred: TypeId) -> bool {
        !matches!(
            self.store.types.try_get(inferred),
            None | Some(
                Type::NoType
                    | Type::NoPrefix
                    | Type::Error(_)
                    | Type::Bounds { .. }
                    | Type::AliasingBounds { .. }
                    | Type::ByName { .. }
                    | Type::Repeated { .. }
                    | Type::Method(_)
                    | Type::Poly(_)
                    | Type::TypeLambda(_)
                    | Type::RecThis { .. }
                    | Type::Wildcard { .. }
                    | Type::MatchCase { .. }
                    | Type::ClassInfo(_)
            )
        )
    }

    /// The parser uses an empty source `TypeTree` for a missing local annotation.
    /// In typed trees that source identity maps to a concrete type tree carrying
    /// the inferred local symbol info.
    pub(in crate::typer) fn reify_inferred_local_type_tree(
        &mut self,
        source_tree: TreeId<Untyped>,
        ty: TypeId,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(typed) = self.typed_index.get(self.source, source_tree) {
            return Ok(typed);
        }
        let Some(source_node) = self.arena.try_get(source_tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: source_tree.index(),
            });
        };
        if !matches!(source_node.kind, TreeKind::TypeTree(_)) {
            return Err(TyperError::TypeArgumentTreeCannotBeReified {
                source: self.source,
                tree_index: source_tree.index(),
                tree_kind: tree_kind_name(&source_node.kind),
            });
        }
        let typed = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .type_tree(ty, source_node.position);
        self.typed_index
            .insert(self.source, source_tree, typed)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        new_mappings.push((self.source, source_tree));
        Ok(typed)
    }
    fn type_block_stat_expansion(
        &mut self,
        stat: TreeId<Untyped>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TypedStatExpansion, TyperError> {
        let Some(source_stat) = self.arena.try_get(stat).cloned() else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: stat.index(),
            });
        };
        if let TreeKind::Import(import) = source_stat.kind {
            let typed = self.type_local_import_statement(
                stat,
                import,
                source_stat.position,
                context,
                info_journal,
                new_mappings,
            )?;
            let Some(scope_stack) = context.local_scopes else {
                return Err(TyperError::LocalBlockDeclarationDeferred {
                    source: self.source,
                    tree_index: stat.index(),
                    kind: "import scope",
                });
            };
            self.expression_scopes
                .get_mut(scope_stack.index())
                .ok_or(TyperError::ExpressionLocalScopeStackMissing { stack: scope_stack })?
                .imports
                .push((stat, context));
            return Ok(TypedStatExpansion::one(typed));
        }

        if let TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) = &source_stat.kind
        {
            if extension.methods.len() != 1
                || extension.param_clauses.len() != 1
                || extension.param_clauses[0].len() != 1
            {
                return Err(TyperError::LocalBlockDeclarationDeferred {
                    source: self.source,
                    tree_index: stat.index(),
                    kind: "extension methods",
                });
            }
            let method_tree = extension.methods[0];
            let Some(method_node) = self.arena.try_get(method_tree).cloned() else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: method_tree.index(),
                });
            };
            let TreeKind::DefDef(definition) = method_node.kind else {
                return Err(TyperError::LocalBlockDeclarationDeferred {
                    source: self.source,
                    tree_index: stat.index(),
                    kind: "extension methods",
                });
            };
            let Some(method) = self.local_methods.symbol_at(self.source, method_tree) else {
                return Err(TyperError::LocalBlockDeclarationDeferred {
                    source: self.source,
                    tree_index: stat.index(),
                    kind: "extension methods",
                });
            };
            if !self
                .store
                .symbols
                .get(method)
                .flags
                .contains(SymbolFlags::EXTENSION)
            {
                return Err(TyperError::LocalBlockDeclarationDeferred {
                    source: self.source,
                    tree_index: stat.index(),
                    kind: "extension methods",
                });
            }
            let typed = self.type_local_method_definition(
                method_tree,
                &definition,
                method_node.position,
                info_journal,
                new_mappings,
            )?;
            self.typed_index
                .insert(self.source, stat, typed)
                .map_err(|error| TyperError::ConflictingTypedExpression {
                    source: error.source,
                    tree_index: error.untyped.index(),
                    existing: error.existing.index(),
                    attempted: error.attempted.index(),
                })?;
            new_mappings.push((self.source, stat));
            return Ok(TypedStatExpansion::one(typed));
        }

        if let Some(kind) = local_block_declaration_kind(&source_stat.kind) {
            if let TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) = &source_stat.kind {
                return self.type_local_patdef(
                    stat,
                    definition,
                    source_stat.position,
                    context,
                    info_journal,
                    new_mappings,
                );
            }
            if let TreeKind::DefDef(definition) = &source_stat.kind
                && definition.rhs.is_some()
            {
                let typed = self.type_local_method_definition(
                    stat,
                    definition,
                    source_stat.position,
                    info_journal,
                    new_mappings,
                )?;
                return Ok(TypedStatExpansion::one(typed));
            }
            if let TreeKind::ValDef(definition) = &source_stat.kind {
                if definition
                    .metadata
                    .modifiers
                    .iter()
                    .any(|modifier| *modifier != dotty_core::ast::Modifier::Var)
                {
                    return Err(TyperError::LocalBlockDeclarationDeferred {
                        source: self.source,
                        tree_index: stat.index(),
                        kind,
                    });
                }
                let typed =
                    self.type_value_expression_inner(stat, context, info_journal, new_mappings)?;
                return Ok(TypedStatExpansion::one(typed));
            }
            // Concrete PatDef typing remains deferred until the following
            // PatDef increments consume the expansion contract above.
            return Err(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: stat.index(),
                kind,
            });
        }

        let typed = self.type_value_expression_inner(stat, context, info_journal, new_mappings)?;
        Ok(TypedStatExpansion::one(typed))
    }

    pub(in crate::typer) fn type_block_expression(
        &mut self,
        tree: TreeId<Untyped>,
        block: dotty_core::ast::Block<Untyped>,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let contains_local_method = block.stats.iter().any(|stat| {
            self.arena.try_get(*stat).is_some_and(|node| {
                matches!(
                    node.kind,
                    TreeKind::DefDef(_) | TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(_))
                )
            })
        });
        let block_scope = self
            .store
            .scopes
            .alloc(dotty_core::Scope::new(Some(context.owner)));
        let block_context = self.push_local_scope(context, block_scope)?;
        if let Some(frame) = block_context
            .local_scopes
            .and_then(|stack| self.expression_scopes.get_mut(stack.index()))
        {
            frame.is_block_scope = true;
        }
        self.preindex_local_methods(&block.stats, block_context, block_scope)?;
        let mut stats = Vec::with_capacity(block.stats.len());
        for stat in block.stats {
            self.type_block_stat_expansion(stat, block_context, info_journal, new_mappings)?
                .append_to(&mut stats);
        }
        let expr = self.type_value_expression_inner(
            block.expr,
            block_context,
            info_journal,
            new_mappings,
        )?;
        let ty = self.typed_arena.get(expr).ty;
        let typed = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .block(stats, expr, ty, position);
        if contains_local_method {
            self.typed_index
                .insert(self.source, tree, typed)
                .map_err(|error| TyperError::ConflictingTypedExpression {
                    source: error.source,
                    tree_index: error.untyped.index(),
                    existing: error.existing.index(),
                    attempted: error.attempted.index(),
                })?;
            new_mappings.push((self.source, tree));
        }
        Ok(typed)
    }
}
