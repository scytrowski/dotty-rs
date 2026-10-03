//! Pattern typing entry points and pattern-specific type adaptation.

use super::{ExpressionContext, SourceTyper, TyperError};
use crate::typer::PatternKind;
use dotty_core::ast::{TreeKind, TypedAstBuilder, UntypedNode};
use dotty_core::types::Type;
use dotty_core::{
    Name, Namespace, SemanticStore, SourceId, SymbolFlags, SymbolId, SymbolInfo, SymbolKind,
    SymbolOrigin, TermRefTarget, TreeId, TypeId, TypeRefTarget, Typed, Untyped,
};

/// Classifies a pattern by its stable source-tree root category.
pub(super) fn pattern_kind(kind: &TreeKind<Untyped>) -> PatternKind {
    match kind {
        TreeKind::Ident(_) => PatternKind::Identifier,
        TreeKind::Select(_) => PatternKind::StableSelection,
        TreeKind::Literal(_) | TreeKind::PhaseSpecific(UntypedNode::Number(_)) => {
            PatternKind::Literal
        }
        TreeKind::Typed(_) => PatternKind::Typed,
        TreeKind::Alternative(_) => PatternKind::Alternative,
        TreeKind::Apply(_) => PatternKind::Application,
        TreeKind::TypeApply(_) => PatternKind::TypeApplication,
        TreeKind::Bind(_) => PatternKind::Binding,
        TreeKind::UnApply(_) => PatternKind::Extractor,
        TreeKind::PhaseSpecific(UntypedNode::Tuple(_)) => PatternKind::Tuple,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)) => PatternKind::Infix,
        TreeKind::PhaseSpecific(UntypedNode::Parens(_)) => PatternKind::Parenthesized,
        _ => PatternKind::Other,
    }
}

impl SourceTyper<'_> {
    /// Enters one pattern binding into the active case scope.
    pub(super) fn enter_pattern_binding(
        &mut self,
        source_tree: TreeId<Untyped>,
        name: Name,
        binding_type: TypeId,
        case_context: ExpressionContext,
    ) -> Result<(SymbolId, TypeId), TyperError> {
        let Some(tree) = self.arena.try_get(source_tree) else {
            return Err(TyperError::MalformedPatternBinding {
                source: self.source,
                tree_index: source_tree.index(),
            });
        };
        let valid_source = match &tree.kind {
            TreeKind::Bind(binding) => binding.name == name,
            TreeKind::Ident(ident) => {
                ident.name == name
                    && !ident.backquoted
                    && is_variable_pattern_name(self.store, name)
            }
            TreeKind::Typed(typed) => self.arena.try_get(typed.expr).is_some_and(|inner| {
                matches!(
                    &inner.kind,
                    TreeKind::Ident(ident)
                        if ident.name == name
                            && !ident.backquoted
                            && is_variable_pattern_name(self.store, name)
                )
            }),
            _ => false,
        };
        if !valid_source || !name.is_term() {
            return Err(TyperError::MalformedPatternBinding {
                source: self.source,
                tree_index: source_tree.index(),
            });
        }
        if self.store.names.resolve(name.text()) == "_" {
            return Err(TyperError::WildcardPatternBindingRejected {
                source: self.source,
                tree_index: source_tree.index(),
            });
        }
        let stack =
            case_context
                .local_scopes
                .ok_or(TyperError::PatternBindingOutsideCaseScope {
                    source: self.source,
                    tree_index: source_tree.index(),
                })?;
        self.validate_expression_scope_stack(Some(stack))?;
        let frame = self
            .expression_scopes
            .get(stack.index())
            .ok_or(TyperError::ExpressionLocalScopeStackMissing { stack })?;
        if !frame.is_case_scope
            || self.store.scopes.get(frame.scope).owner != Some(case_context.owner)
        {
            return Err(TyperError::PatternBindingOutsideCaseScope {
                source: self.source,
                tree_index: source_tree.index(),
            });
        }
        let scope = frame.scope;
        if let Some(existing) = self
            .pattern_bindings
            .by_tree
            .get(&(self.source, source_tree))
            .copied()
        {
            let Some(existing_scope) = self
                .pattern_bindings
                .scope_by_symbol
                .get(&existing)
                .copied()
            else {
                return Err(TyperError::PatternBindingScopeConflict {
                    source: self.source,
                    tree_index: source_tree.index(),
                    existing_scope: scope,
                    attempted_scope: scope,
                });
            };
            if existing_scope != scope {
                return Err(TyperError::PatternBindingScopeConflict {
                    source: self.source,
                    tree_index: source_tree.index(),
                    existing_scope,
                    attempted_scope: scope,
                });
            }
            return Ok((existing, self.pattern_binding_term_ref(existing)));
        }
        if !self.store.scopes.get(scope).lookup_all(&name).is_empty() {
            return Err(TyperError::DuplicatePatternBinding {
                source: self.source,
                tree_index: source_tree.index(),
                name,
            });
        }

        let symbol = self.store.symbols.alloc(dotty_core::Symbol {
            name,
            owner: Some(case_context.owner),
            kind: SymbolKind::Local,
            flags: SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Complete(binding_type),
            origin: SymbolOrigin::Source(self.source),
            annotations: Vec::new(),
            position: tree.position,
            links: dotty_core::SymbolLinks::default(),
        });
        self.store.scopes.get_mut(scope).enter(name, symbol);
        self.pattern_bindings
            .by_tree
            .insert((self.source, source_tree), symbol);
        self.pattern_bindings.scope_by_symbol.insert(symbol, scope);
        Ok((symbol, self.pattern_binding_term_ref(symbol)))
    }

    fn pattern_binding_term_ref(&mut self, symbol: SymbolId) -> TypeId {
        self.store.types.alloc(Type::TermRef {
            prefix: self.definitions.no_prefix,
            target: TermRefTarget::Symbol(symbol),
        })
    }

    /// Types wildcard and variable-pattern roots. Other pattern families are
    /// introduced in later Match increments.
    pub(super) fn type_pattern(
        &mut self,
        pattern: TreeId<Untyped>,
        selector_type: TypeId,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(typed) = self.typed_index.get(self.source, pattern) {
            return Ok(typed);
        }
        let Some(source_tree) = self.arena.try_get(pattern).cloned() else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: pattern.index(),
            });
        };
        let typed = match &source_tree.kind {
            TreeKind::Ident(ident) => {
                if !ident.name.is_term() {
                    return Err(TyperError::MalformedVariablePattern {
                        source: self.source,
                        tree_index: pattern.index(),
                    });
                }
                let spelling = self.store.names.resolve(ident.name.text());
                if !ident.backquoted && spelling == "_" {
                    TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                        .ident_with_backquoted(
                            ident.name,
                            false,
                            selector_type,
                            source_tree.position,
                        )
                } else if !ident.backquoted && is_variable_pattern_name(self.store, ident.name) {
                    let wildcard_name = Name::new(self.store.names.intern("_"), Namespace::Term);
                    let wildcard = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                        .ident_with_backquoted(
                            wildcard_name,
                            false,
                            selector_type,
                            source_tree.position,
                        );
                    let (_symbol, binding_type) =
                        self.enter_pattern_binding(pattern, ident.name, selector_type, context)?;
                    TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).bind(
                        ident.name,
                        wildcard,
                        binding_type,
                        false,
                        source_tree.position,
                    )
                } else {
                    self.type_stable_pattern_expression(
                        pattern,
                        selector_type,
                        context,
                        info_journal,
                        new_mappings,
                    )?
                }
            }
            TreeKind::Select(selection) => {
                if !selection.name.is_term() || self.arena.try_get(selection.qualifier).is_none() {
                    return Err(TyperError::MalformedStablePatternTarget {
                        source: self.source,
                        tree_index: pattern.index(),
                    });
                }
                self.type_stable_pattern_expression(
                    pattern,
                    selector_type,
                    context,
                    info_journal,
                    new_mappings,
                )?
            }
            TreeKind::Literal(literal) => {
                let typed =
                    self.type_literal_expression(pattern, literal.clone(), source_tree.position)?;
                let actual = self.widen_expression_type_journaled(
                    self.typed_arena.get(typed).ty,
                    info_journal,
                    0,
                )?;
                self.require_literal_pattern_compatible(
                    self.typed_arena.get(typed).ty,
                    actual,
                    selector_type,
                    pattern.index(),
                )?;
                typed
            }
            TreeKind::PhaseSpecific(UntypedNode::Number(number)) => {
                let typed =
                    self.type_number_literal_expression(pattern, *number, source_tree.position)?;
                let actual = self.widen_expression_type_journaled(
                    self.typed_arena.get(typed).ty,
                    info_journal,
                    0,
                )?;
                self.require_literal_pattern_compatible(
                    self.typed_arena.get(typed).ty,
                    actual,
                    selector_type,
                    pattern.index(),
                )?;
                typed
            }
            TreeKind::Bind(binding) => {
                if binding.given || !self.bind_pattern_body_is_supported(binding.body) {
                    return Err(TyperError::UnsupportedBindPatternBody {
                        source: self.source,
                        tree_index: binding.body.index(),
                    });
                }
                let typed_body = self.type_pattern(
                    binding.body,
                    selector_type,
                    context,
                    info_journal,
                    new_mappings,
                )?;
                let binding_value_type = if self.bind_pattern_body_is_typed(binding.body) {
                    self.typed_arena.get(typed_body).ty
                } else {
                    selector_type
                };
                let (_symbol, binding_type) =
                    self.enter_pattern_binding(pattern, binding.name, binding_value_type, context)?;
                TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).bind(
                    binding.name,
                    typed_body,
                    binding_type,
                    false,
                    source_tree.position,
                )
            }
            TreeKind::Typed(typed_pattern) => self.type_typed_pattern(
                pattern,
                typed_pattern.expr,
                typed_pattern.tpt,
                selector_type,
                context,
                source_tree.position,
                info_journal,
                new_mappings,
            )?,
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => self.type_pattern(
                parens.inner,
                selector_type,
                context,
                info_journal,
                new_mappings,
            )?,
            _ => {
                return Err(TyperError::UnsupportedPattern {
                    source: self.source,
                    tree_index: pattern.index(),
                    pattern_kind: pattern_kind(&source_tree.kind),
                });
            }
        };
        if let Some(existing) = self.typed_index.get(self.source, pattern) {
            if existing != typed {
                return Err(TyperError::ConflictingTypedExpression {
                    source: self.source,
                    tree_index: pattern.index(),
                    existing: existing.index(),
                    attempted: typed.index(),
                });
            }
        } else {
            self.typed_index
                .insert(self.source, pattern, typed)
                .map_err(|conflict| TyperError::ConflictingTypedExpression {
                    source: conflict.source,
                    tree_index: conflict.untyped.index(),
                    existing: conflict.existing.index(),
                    attempted: conflict.attempted.index(),
                })?;
            new_mappings.push((self.source, pattern));
        }
        Ok(typed)
    }

    fn type_stable_pattern_expression(
        &mut self,
        pattern: TreeId<Untyped>,
        selector_type: TypeId,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let typed = self.type_expression_inner(pattern, context, info_journal, new_mappings)?;
        let reference_type = self.typed_arena.get(typed).ty;
        let symbol = match self.store.types.try_get(reference_type) {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) if self.store.symbols.contains(*symbol) => *symbol,
            _ => {
                return Err(TyperError::MalformedStablePatternTarget {
                    source: self.source,
                    tree_index: pattern.index(),
                });
            }
        };
        if self
            .require_stable_selection_prefix(reference_type, pattern.index())
            .is_err()
        {
            return Err(TyperError::UnstablePatternValue {
                source: self.source,
                tree_index: pattern.index(),
                symbol,
            });
        }
        let actual = self.widen_expression_type_journaled(reference_type, info_journal, 0)?;
        self.require_pattern_compatible(actual, selector_type, pattern.index())?;
        Ok(typed)
    }

    fn bind_pattern_body_is_supported(&self, body: TreeId<Untyped>) -> bool {
        let Some(tree) = self.arena.try_get(body) else {
            return false;
        };
        match &tree.kind {
            TreeKind::Ident(ident) if ident.name.is_term() => {
                let spelling = self.store.names.resolve(ident.name.text());
                if !ident.backquoted && spelling == "_" {
                    true
                } else {
                    ident.backquoted || !is_variable_pattern_name(self.store, ident.name)
                }
            }
            TreeKind::Literal(_) | TreeKind::PhaseSpecific(UntypedNode::Number(_)) => true,
            TreeKind::Select(selection) => selection.name.is_term(),
            TreeKind::Typed(typed) => self.is_wildcard_pattern(typed.expr),
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => {
                self.bind_pattern_body_is_supported(parens.inner)
            }
            _ => false,
        }
    }

    fn is_wildcard_pattern(&self, tree: TreeId<Untyped>) -> bool {
        self.arena.try_get(tree).is_some_and(|tree| {
            matches!(
                &tree.kind,
                TreeKind::Ident(ident)
                    if !ident.backquoted
                        && ident.name.is_term()
                        && self.store.names.resolve(ident.name.text()) == "_"
            )
        })
    }

    fn bind_pattern_body_is_typed(&self, body: TreeId<Untyped>) -> bool {
        let Some(tree) = self.arena.try_get(body) else {
            return false;
        };
        match &tree.kind {
            TreeKind::Typed(_) => true,
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => {
                self.bind_pattern_body_is_typed(parens.inner)
            }
            _ => false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn type_typed_pattern(
        &mut self,
        pattern: TreeId<Untyped>,
        expr: TreeId<Untyped>,
        tpt: TreeId<Untyped>,
        selector_type: TypeId,
        context: ExpressionContext,
        position: Option<dotty_core::SourceSpan>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let Some(expr_tree) = self.arena.try_get(expr) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: expr.index(),
            });
        };
        let (variable, source_name) = match &expr_tree.kind {
            TreeKind::Ident(ident) if ident.name.is_term() => {
                let spelling = self.store.names.resolve(ident.name.text());
                if !ident.backquoted && spelling == "_" {
                    (None, ident.name)
                } else if !ident.backquoted && is_variable_pattern_name(self.store, ident.name) {
                    (Some(ident.name), ident.name)
                } else {
                    return Err(TyperError::UnsupportedPattern {
                        source: self.source,
                        tree_index: pattern.index(),
                        pattern_kind: PatternKind::Typed,
                    });
                }
            }
            _ => {
                return Err(TyperError::UnsupportedPattern {
                    source: self.source,
                    tree_index: pattern.index(),
                    pattern_kind: PatternKind::Typed,
                });
            }
        };

        let type_context = self.expression_type_context(context)?;
        let pattern_type = self.type_of_tpt_inner(tpt, type_context)?;
        if !self.typed_pattern_runtime_test_supported(pattern_type, info_journal)? {
            return Err(TyperError::TypedPatternRuntimeTestDeferred {
                source: self.source,
                tree_index: pattern.index(),
                pattern_type,
                reason: "runtime type tests are limited to non-generic class and trait references",
            });
        }
        self.complete_typed_pattern_relation_class(selector_type, info_journal)?;
        self.require_typed_pattern_compatible(selector_type, pattern_type, pattern.index())?;
        let typed_tpt = self.reify_type_ascription_tree(tpt, pattern_type, new_mappings)?;
        let wildcard_name = if variable.is_some() {
            Name::new(self.store.names.intern("_"), Namespace::Term)
        } else {
            source_name
        };
        let child_position = expr_tree.position;
        let wildcard = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .ident_with_backquoted(wildcard_name, false, pattern_type, child_position);
        let typed_test = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).typed_expr(
            wildcard,
            typed_tpt,
            pattern_type,
            position,
        );
        let result =
            if let Some(name) = variable {
                let (_symbol, binding_type) =
                    self.enter_pattern_binding(pattern, name, pattern_type, context)?;
                let typed_bind = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                    .bind(name, typed_test, binding_type, false, position);
                self.insert_pattern_child_mapping(expr, typed_bind, new_mappings)?;
                typed_bind
            } else {
                self.insert_pattern_child_mapping(expr, wildcard, new_mappings)?;
                typed_test
            };
        Ok(result)
    }

    fn insert_pattern_child_mapping(
        &mut self,
        source_tree: TreeId<Untyped>,
        typed_tree: TreeId<Typed>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<(), TyperError> {
        if let Some(existing) = self.typed_index.get(self.source, source_tree) {
            if existing != typed_tree {
                return Err(TyperError::ConflictingTypedExpression {
                    source: self.source,
                    tree_index: source_tree.index(),
                    existing: existing.index(),
                    attempted: typed_tree.index(),
                });
            }
        } else {
            self.typed_index
                .insert(self.source, source_tree, typed_tree)
                .map_err(|conflict| TyperError::ConflictingTypedExpression {
                    source: conflict.source,
                    tree_index: conflict.untyped.index(),
                    existing: conflict.existing.index(),
                    attempted: conflict.attempted.index(),
                })?;
            new_mappings.push((self.source, source_tree));
        }
        Ok(())
    }

    fn typed_pattern_runtime_test_supported(
        &mut self,
        pattern_type: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<bool, TyperError> {
        let Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        }) = self.store.types.try_get(pattern_type)
        else {
            return Ok(false);
        };
        let symbol = *symbol;
        if !self.store.symbols.contains(symbol)
            || !matches!(
                self.store.symbols.get(symbol).kind,
                SymbolKind::Class | SymbolKind::Trait
            )
        {
            return Ok(false);
        }
        if matches!(
            self.store.types.try_get(self.definitions.nothing_type),
            Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(target),
                ..
            }) if *target == symbol
        ) {
            return Ok(false);
        }
        if self.is_builtin_type_symbol(symbol) {
            return Ok(true);
        }
        if !matches!(self.store.symbols.get(symbol).info, SymbolInfo::Complete(_)) {
            self.complete_symbol_inner(symbol, info_journal)?;
        }
        let SymbolInfo::Complete(info) = self.store.symbols.get(symbol).info else {
            return Ok(false);
        };
        let Some(Type::ClassInfo(class_info)) = self.store.types.try_get(info) else {
            return Ok(false);
        };
        Ok(!self
            .store
            .scopes
            .get(class_info.declarations)
            .entered_symbols()
            .any(|member| self.store.symbols.get(member).kind == SymbolKind::TypeParameter))
    }

    fn complete_typed_pattern_relation_class(
        &mut self,
        ty: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(), TyperError> {
        let Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        }) = self.store.types.try_get(ty)
        else {
            return Ok(());
        };
        let symbol = *symbol;
        if !self.store.symbols.contains(symbol)
            || !matches!(
                self.store.symbols.get(symbol).kind,
                SymbolKind::Class | SymbolKind::Trait
            )
            || !matches!(self.store.symbols.get(symbol).info, SymbolInfo::Missing)
            || self.is_builtin_type_symbol(symbol)
        {
            return Ok(());
        }
        self.complete_symbol_inner(symbol, info_journal)?;
        Ok(())
    }

    fn is_builtin_type_symbol(&self, symbol: SymbolId) -> bool {
        [
            self.definitions.byte,
            self.definitions.char,
            self.definitions.double,
            self.definitions.float,
            self.definitions.int,
            self.definitions.long,
            self.definitions.short,
            self.definitions.boolean,
            self.definitions.unit,
            self.definitions.object_type,
            self.definitions.any_type,
            self.definitions.nothing_type,
        ]
        .into_iter()
        .any(|builtin| {
            matches!(
                self.store.types.try_get(builtin),
                Some(Type::TypeRef {
                    target: TypeRefTarget::Symbol(target),
                    ..
                }) if *target == symbol
            )
        })
    }

    fn require_typed_pattern_compatible(
        &mut self,
        selector: TypeId,
        pattern_type: TypeId,
        tree_index: u32,
    ) -> Result<(), TyperError> {
        let pattern_conforms = self.conforms(pattern_type, selector);
        if matches!(pattern_conforms, Ok(true)) {
            return Ok(());
        }
        let selector_conforms = self.conforms(selector, pattern_type);
        if matches!(selector_conforms, Ok(true)) {
            return Ok(());
        }
        if let Err(error) = pattern_conforms {
            return Err(TyperError::TypedPatternRelationDeferred {
                source: self.source,
                tree_index,
                selector,
                pattern_type,
                error: Box::new(error),
            });
        }
        if let Err(error) = selector_conforms {
            return Err(TyperError::TypedPatternRelationDeferred {
                source: self.source,
                tree_index,
                selector,
                pattern_type,
                error: Box::new(error),
            });
        }
        Err(TyperError::TypedPatternTypeMismatch {
            source: self.source,
            tree_index,
            selector,
            pattern_type,
        })
    }

    fn require_pattern_compatible(
        &mut self,
        actual: TypeId,
        selector: TypeId,
        tree_index: u32,
    ) -> Result<(), TyperError> {
        let forward = self.conforms(actual, selector);
        if matches!(forward, Ok(true)) {
            return Ok(());
        }
        let reverse = self.conforms(selector, actual);
        if matches!(reverse, Ok(true)) {
            return Ok(());
        }
        if let Err(error) = forward {
            return Err(TyperError::PatternTypeRelationDeferred {
                source: self.source,
                tree_index,
                actual,
                selector,
                error: Box::new(error),
            });
        }
        if let Err(error) = reverse {
            return Err(TyperError::PatternTypeRelationDeferred {
                source: self.source,
                tree_index,
                actual,
                selector,
                error: Box::new(error),
            });
        }
        Err(TyperError::PatternTypeMismatch {
            source: self.source,
            tree_index,
            actual,
            selector,
        })
    }

    fn require_literal_pattern_compatible(
        &mut self,
        literal_type: TypeId,
        widened_type: TypeId,
        selector_type: TypeId,
        tree_index: u32,
    ) -> Result<(), TyperError> {
        if let Some(Type::Constant(selector_constant)) = self.store.types.try_get(selector_type) {
            let matches_selector = matches!(
                self.store.types.try_get(literal_type),
                Some(Type::Constant(literal_constant)) if literal_constant == selector_constant
            );
            return if matches_selector {
                Ok(())
            } else {
                Err(TyperError::PatternTypeMismatch {
                    source: self.source,
                    tree_index,
                    actual: literal_type,
                    selector: selector_type,
                })
            };
        }
        self.require_pattern_compatible(widened_type, selector_type, tree_index)
    }

    /// Computes the selector prototype used by patterns, preserving literal
    /// singleton types and widening other expression types as usual.
    pub(super) fn pattern_selector_type(
        &mut self,
        selector_type: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        match self.store.types.try_get(selector_type) {
            Some(Type::Constant(_)) => Ok(selector_type),
            _ => self.widen_expression_type_journaled(selector_type, info_journal, 0),
        }
    }
}

/// Mirrors the parser's pattern-variable first-character rule. Backquotes are
/// checked by the caller; literal keywords and the wildcard are never binders.
fn is_variable_pattern_name(store: &SemanticStore, name: Name) -> bool {
    let spelling = store.names.resolve(name.text());
    if matches!(spelling, "_" | "false" | "true" | "null") {
        return false;
    }
    spelling
        .chars()
        .next()
        .is_some_and(|first| first == '_' || (first.is_alphabetic() && first.is_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typer::{ExpressionContext, SourceTyper};
    use dotty_core::ast::{Ident, Tree};
    use dotty_core::{
        Definitions, Name, Namespace, Packages, SemanticStore, SourceSemanticIndex, SourceText,
        Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
    };
    use dotty_lexer::ContextualScanner;
    use dotty_namer::name_compilation_unit;

    fn setup(
        source_text: &str,
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
        let source = SourceId::from_index(7);
        let mut packages = Packages::new();
        let scanner = ContextualScanner::new(source_text).unwrap();
        let parsed = dotty_parser::parse_compilation_unit(
            SourceText::new(source_text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "Patterns.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        (parsed, store, packages, definitions, index, source)
    }

    fn method_and_pattern(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
        method_name: &str,
    ) -> (dotty_core::SymbolId, TreeId<Untyped>) {
        let mut method = None;
        let mut pattern = None;
        for (tree, node) in parsed.ast.iter() {
            match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == method_name =>
                {
                    method = Some(index.symbol_at(source, tree).unwrap());
                }
                TreeKind::Ident(ident) if store.names.resolve(ident.name.text()) == "_" => {
                    pattern = Some(tree);
                }
                _ => {}
            }
        }
        (method.unwrap(), pattern.unwrap())
    }

    fn method_symbol(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
    ) -> dotty_core::SymbolId {
        parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "choose" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap()
    }

    fn context_for<'a>(
        parsed: &'a dotty_parser::ParseResult,
        store: &'a mut SemanticStore,
        packages: &'a Packages,
        definitions: Definitions,
        index: &'a SourceSemanticIndex,
        source: SourceId,
        method: dotty_core::SymbolId,
    ) -> (SourceTyper<'a>, ExpressionContext) {
        let mut typer = SourceTyper::new(&parsed.ast, source, index, store, definitions, packages);
        let context = typer.expression_context_for(method).unwrap();
        (typer, context)
    }

    #[test]
    fn wildcard_uses_selector_type_and_repeated_typing_keeps_identity() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case _ => 1 } }");
        let (method, pattern) = method_and_pattern(&parsed, &store, &index, source, "choose");
        let method_scope = index.scope_of(method).unwrap();
        let underscore = Name::new(store.names.intern("_"), Namespace::Term);
        let shadowed_wildcard = store.symbols.alloc(Symbol {
            name: underscore,
            owner: Some(method),
            kind: SymbolKind::Local,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(definitions.object_type),
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        store
            .scopes
            .get_mut(method_scope)
            .enter(underscore, shadowed_wildcard);
        let second_pattern = parsed.ast.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: underscore,
                backquoted: false,
            }),
            position: None,
            ty: (),
        });
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let first = typer
            .run_expression_transaction(|typer, journal, mappings| {
                let typed =
                    typer.type_pattern(pattern, definitions.int, context, journal, mappings)?;
                Ok(typed)
            })
            .unwrap();
        let typed_tree = typer.typed_arena.get(first);
        assert!(matches!(typed_tree.kind, TreeKind::Ident(ident) if !ident.backquoted));
        assert_eq!(typed_tree.ty, definitions.int);
        assert_eq!(typer.typed_index.get(source, pattern), Some(first));

        let repeated = typer
            .run_expression_transaction(|typer, journal, mappings| {
                let before = mappings.len();
                let typed =
                    typer.type_pattern(pattern, definitions.int, context, journal, mappings)?;
                assert_eq!(mappings.len(), before);
                Ok(typed)
            })
            .unwrap();
        assert_eq!(repeated, first);

        let reference_pattern = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(
                    second_pattern,
                    definitions.object_type,
                    context,
                    journal,
                    mappings,
                )
            })
            .unwrap();
        assert_eq!(
            typer.typed_arena.get(reference_pattern).ty,
            definitions.object_type
        );
    }

    #[test]
    fn typed_wildcard_projects_and_reifies_its_type_tree() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case _: Int => 1 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let (pattern, source_tpt) = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::CaseDef(case) => match &parsed.ast.get(case.pattern).kind {
                    TreeKind::Typed(typed) => Some((case.pattern, typed.tpt)),
                    _ => None,
                },
                _ => None,
            })
            .unwrap();
        let wildcard_name = Name::new(store.names.intern("_"), Namespace::Term);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.int, context, journal, mappings)
            })
            .unwrap();
        let TreeKind::Typed(typed_pattern) = typer.typed_arena.get(typed).kind else {
            panic!("typed wildcard pattern should retain a Typed node")
        };
        assert!(matches!(
            typer.typed_arena.get(typed_pattern.expr).kind,
            TreeKind::Ident(ident) if ident.name == wildcard_name
        ));
        assert!(matches!(
            typer.typed_arena.get(typed_pattern.tpt).kind,
            TreeKind::TypeTree(_)
        ));
        assert_eq!(typer.typed_arena.get(typed_pattern.tpt).ty, definitions.int);
        assert_eq!(typer.typed_arena.get(typed).ty, definitions.int);
        assert_eq!(
            typer.typed_index.get(source, source_tpt),
            Some(typed_pattern.tpt)
        );
        assert!(typer.pattern_bindings.by_tree.is_empty());
        let repeated = typer
            .run_expression_transaction(|typer, journal, mappings| {
                let mapped_before = mappings.len();
                let repeated =
                    typer.type_pattern(pattern, definitions.int, context, journal, mappings)?;
                assert_eq!(mappings.len(), mapped_before);
                Ok(repeated)
            })
            .unwrap();
        assert_eq!(repeated, typed);
    }

    #[test]
    fn typed_wildcard_accepts_a_supertype_of_the_selector() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case _: Any => 1 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.int, context, journal, mappings)
            })
            .unwrap();
        assert_eq!(typer.typed_arena.get(typed).ty, definitions.any_type);
    }

    #[test]
    fn typed_pattern_accepts_source_class_subtyping_after_completing_both_sides() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class Parent; class Child extends Parent; class C { def choose(value: Parent): Int = value match { case _: Child => 1 } }",
        );
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let parent = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Parent" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let parameter_tpt = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition) if index.symbol_at(source, tree) == Some(method) => {
                    let parameter = definition.value_param_clauses[0][0];
                    match &parsed.ast.get(parameter).kind {
                        TreeKind::ValDef(parameter) => Some(parameter.tpt),
                        _ => None,
                    }
                }
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let selector_context = typer.expression_type_context(context).unwrap();
        let selector = typer
            .type_of_tpt_inner(parameter_tpt, selector_context)
            .unwrap();
        let typed = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, selector, context, journal, mappings)
            })
            .unwrap();
        assert!(matches!(
            typer.typed_arena.get(typed).kind,
            TreeKind::Typed(_)
        ));
        assert!(matches!(
            typer.store.symbols.get(parent).info,
            SymbolInfo::Complete(_)
        ));
    }

    #[test]
    fn typed_variable_binds_once_and_exposes_its_type_to_guard_and_body() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class C { def predicate(value: Int): Boolean = true; def choose(x: Any): Int = x match { case item: Int if predicate(item) => item } }",
        );
        let method = method_symbol(&parsed, &store, &index, source);
        let case_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::CaseDef(_)).then_some(tree))
            .unwrap();
        let TreeKind::CaseDef(source_case) = parsed.ast.get(case_tree).kind else {
            panic!("expected source CaseDef")
        };
        let TreeKind::Typed(source_pattern) = parsed.ast.get(source_case.pattern).kind else {
            panic!("expected source typed pattern")
        };
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed_case = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_case_def(case_tree, definitions.any_type, context, journal, mappings)
            })
            .unwrap();
        let TreeKind::CaseDef(case) = typer.typed_arena.get(typed_case).kind.clone() else {
            panic!("expected typed CaseDef")
        };
        let TreeKind::Bind(binding) = typer.typed_arena.get(case.pattern).kind.clone() else {
            panic!("typed variable pattern should lower to Bind")
        };
        let symbol = typer
            .pattern_binding_symbol_at(source, source_case.pattern)
            .unwrap();
        assert_eq!(typer.pattern_bindings.by_tree.len(), 1);
        assert_eq!(
            typer.store.symbols.get(symbol).info,
            SymbolInfo::Complete(definitions.int)
        );
        let TreeKind::Typed(typed_pattern) = typer.typed_arena.get(binding.body).kind else {
            panic!("Bind body should retain the typed test")
        };
        assert_eq!(
            typer.typed_index.get(source, source_pattern.tpt),
            Some(typed_pattern.tpt)
        );
        let guard = case.guard.unwrap();
        let TreeKind::Apply(application) = typer.typed_arena.get(guard).kind.clone() else {
            panic!("guard should be an application")
        };
        assert!(matches!(
            typer
                .store
                .types
                .try_get(typer.typed_arena.get(application.args[0]).ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(actual), .. }) if *actual == symbol
        ));
        assert!(matches!(
            typer
                .store
                .types
                .try_get(typer.typed_arena.get(case.body).ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(actual), .. }) if *actual == symbol
        ));
    }

    #[test]
    fn unrelated_typed_pattern_reports_a_focused_mismatch_and_rolls_back() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case _: Boolean => 1 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let error = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.int, context, journal, mappings)
            })
            .unwrap_err();
        assert!(
            matches!(error, TyperError::TypedPatternTypeMismatch { .. }),
            "unexpected error: {error:?}"
        );
        assert_eq!(typer.typed_arena.iter().count(), 0);
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn generic_typed_pattern_defers_runtime_test_without_partial_state() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class Box[A] {}; class C { def choose(x: Any): Int = x match { case _: Box[Int] => 1 } }",
        );
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let typed_tpt = match &parsed.ast.get(pattern).kind {
            TreeKind::Typed(typed) => typed.tpt,
            _ => unreachable!(),
        };
        let box_symbol = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Box" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let error = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.any_type, context, journal, mappings)
            })
            .unwrap_err();
        assert!(matches!(
            error,
            TyperError::TypedPatternRuntimeTestDeferred { .. }
        ));
        assert_eq!(typer.typed_arena.iter().count(), 0);
        assert!(typer.typed_index.is_empty());
        assert!(matches!(
            typer.store.symbols.get(box_symbol).info,
            SymbolInfo::Missing
        ));
        assert_eq!(typer.source_type_index().type_at(source, typed_tpt), None);
    }

    #[test]
    fn nothing_typed_pattern_defers_missing_runtime_test_representation() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Any): Int = x match { case _: Nothing => 1 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        assert!(matches!(
            typer.run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.any_type, context, journal, mappings)
            }),
            Err(TyperError::TypedPatternRuntimeTestDeferred { .. })
        ));
        assert_eq!(typer.typed_arena.iter().count(), 0);
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn unsupported_typed_pattern_relation_is_deferred_without_partial_state() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case _: Int => 1 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let selector = typer.store.types.alloc(Type::And {
            left: definitions.int,
            right: definitions.object_type,
        });
        assert!(matches!(
            typer.run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, selector, context, journal, mappings)
            }),
            Err(TyperError::TypedPatternRelationDeferred { .. })
        ));
        assert_eq!(typer.typed_arena.iter().count(), 0);
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn explicit_bind_around_parenthesized_typed_wildcard_binds_the_narrow_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Any): Int = x match { case item @ (_: Int) => item } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let case_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::CaseDef(_)).then_some(tree))
            .unwrap();
        let TreeKind::CaseDef(source_case) = &parsed.ast.get(case_tree).kind else {
            unreachable!()
        };
        let source_pattern = source_case.pattern;
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed_case = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_case_def(case_tree, definitions.any_type, context, journal, mappings)
            })
            .unwrap();
        let TreeKind::CaseDef(case) = typer.typed_arena.get(typed_case).kind else {
            panic!("expected typed CaseDef")
        };
        let TreeKind::Bind(bind) = typer.typed_arena.get(case.pattern).kind else {
            panic!("expected explicit Bind around typed wildcard")
        };
        let symbol = typer
            .pattern_binding_symbol_at(source, source_pattern)
            .unwrap();
        assert_eq!(
            typer.store.symbols.get(symbol).info,
            SymbolInfo::Complete(definitions.int)
        );
        assert_eq!(typer.typed_arena.get(bind.body).ty, definitions.int);
    }

    #[test]
    fn uppercase_identifier_pattern_uses_stable_value_resolution() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case Value => 1 } }");
        let method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "choose" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(ident) if store.names.resolve(ident.name.text()) == "Value" => {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let error = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.int, context, journal, mappings)
            })
            .unwrap_err();
        assert!(matches!(error, TyperError::TermNameNotFound { .. }));
        assert_eq!(typer.typed_arena.iter().count(), 0);
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn unresolved_backquoted_identifier_uses_stable_value_resolution() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case `value` => 1 } }");
        let method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "choose" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(ident)
                    if store.names.resolve(ident.name.text()) == "value" && ident.backquoted =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let error = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.int, context, journal, mappings)
            })
            .unwrap_err();
        assert!(matches!(error, TyperError::TermNameNotFound { .. }));
        assert!(typer.pattern_bindings.by_tree.is_empty());
    }

    #[test]
    fn literal_patterns_keep_constant_types_and_check_selector_compatibility() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case 1 => 1 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.int, context, journal, mappings)
            })
            .unwrap();
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(typed).ty),
            Some(Type::Constant(dotty_core::Constant::Int(1)))
        ));
        assert!(matches!(
            typer.typed_arena.get(typed).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Int(1)
            })
        ));

        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case 1 => 1 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        assert!(matches!(
            typer.run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.boolean, context, journal, mappings)
            }),
            Err(TyperError::PatternTypeMismatch { actual, selector, .. })
                if actual == definitions.int && selector == definitions.boolean
        ));
        assert!(typer.typed_arena.iter().next().is_none());
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn boolean_literal_pattern_uses_boolean_selector_prototype() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Boolean): Int = x match { case true => 1 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.boolean, context, journal, mappings)
            })
            .unwrap();
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(typed).ty),
            Some(Type::Constant(dotty_core::Constant::Boolean(true)))
        ));
    }

    #[test]
    fn unsupported_pattern_selector_relation_is_deferred() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case 1 => 1 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let unsupported_selector = typer.store.types.alloc(Type::And {
            left: definitions.int,
            right: definitions.object_type,
        });
        assert!(matches!(
            typer.run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, unsupported_selector, context, journal, mappings)
            }),
            Err(TyperError::PatternTypeRelationDeferred { .. })
        ));
        assert_eq!(typer.typed_arena.iter().count(), 0);
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn stable_identifier_pattern_preserves_its_term_reference_without_binding() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class C { val Stable: Int = 1; def choose(value: Int): Int = value match { case Stable => 1 } }",
        );
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let expected_symbol = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::ValDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Stable" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.int, context, journal, mappings)
            })
            .unwrap();
        assert!(matches!(
            typer.typed_arena.get(typed).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(typed).ty),
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) if *symbol == expected_symbol
        ));
        assert_eq!(
            typer.store.symbols.get(expected_symbol).kind,
            SymbolKind::Field
        );
        assert!(typer.pattern_bindings.by_tree.is_empty());
    }

    #[test]
    fn object_identifier_pattern_resolves_the_stable_object_term() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "object Foo; class C { def choose(value: Any): Int = value match { case Foo => 1 } }",
        );
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let object_symbol = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::PhaseSpecific(UntypedNode::ModuleDef(definition))
                    if store.names.resolve(definition.name.as_name().text()) == "Foo" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.any_type, context, journal, mappings)
            })
            .unwrap();
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(typed).ty),
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) if *symbol == object_symbol
        ));
        assert_eq!(
            typer.store.symbols.get(object_symbol).kind,
            SymbolKind::Object
        );
        assert!(typer.pattern_bindings.by_tree.is_empty());
    }

    #[test]
    fn stable_pattern_mismatch_rolls_back_reference_and_symbol_state() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class C { val Stable: Int = 1; def choose(value: Boolean): Int = value match { case Stable => 1 } }",
        );
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let stable_symbol = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::ValDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Stable" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let previous_info = store.symbols.get(stable_symbol).info;
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        assert!(matches!(
            typer.run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.boolean, context, journal, mappings)
            }),
            Err(TyperError::PatternTypeMismatch {
                actual, selector, ..
            }) if actual == definitions.int && selector == definitions.boolean
        ));
        assert_eq!(typer.store.symbols.get(stable_symbol).info, previous_info);
        assert!(typer.typed_index.is_empty());
        assert_eq!(typer.typed_arena.iter().count(), 0);
        assert!(typer.pattern_bindings.by_tree.is_empty());
    }

    #[test]
    fn mutable_stable_looking_pattern_is_rejected() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class C { var Stable: Int = 1; def choose(value: Int): Int = value match { case Stable => 1 } }",
        );
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let expected_symbol = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::ValDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Stable" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        assert!(matches!(
            typer.run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.int, context, journal, mappings)
            }),
            Err(TyperError::UnstablePatternValue { symbol, .. })
                if symbol == expected_symbol
        ));
        assert!(typer.pattern_bindings.by_tree.is_empty());
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn method_reference_is_not_a_stable_pattern_value() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class C { def Stable: Int = 1; def choose(value: Int): Int = value match { case Stable => 1 } }",
        );
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let expected_symbol = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Stable" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        assert!(matches!(
            typer.run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.int, context, journal, mappings)
            }),
            Err(TyperError::UnstablePatternValue { symbol, .. })
                if symbol == expected_symbol
        ));
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn stable_selection_pattern_preserves_the_selected_symbol_and_prefix() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "object Values { val Stable: Int = 1 }; class C { def choose(value: Int): Int = value match { case Values.Stable => 1 } }",
        );
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let expected_symbol = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::ValDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Stable" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.int, context, journal, mappings)
            })
            .unwrap();
        assert!(matches!(
            typer.typed_arena.get(typed).kind,
            TreeKind::Select(_)
        ));
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(typed).ty),
            Some(Type::TermRef {
                prefix,
                target: TermRefTarget::Symbol(symbol),
            }) if *symbol == expected_symbol && *prefix != definitions.no_prefix
        ));
        assert_eq!(
            typer.store.symbols.get(expected_symbol).kind,
            SymbolKind::Field
        );
    }

    #[test]
    fn backquoted_lowercase_stable_parameter_resolves_without_binding() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(stable: Int): Int = stable match { case `stable` => 1 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(ident)
                    if ident.backquoted && store.names.resolve(ident.name.text()) == "stable" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let expected_symbol = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::ValDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "stable" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.int, context, journal, mappings)
            })
            .unwrap();
        assert!(matches!(
            typer.typed_arena.get(typed).kind,
            TreeKind::Ident(ident) if ident.backquoted
        ));
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(typed).ty),
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) if *symbol == expected_symbol
        ));
        assert!(typer.pattern_bindings.by_tree.is_empty());
    }

    #[test]
    fn true_false_and_null_are_not_variable_patterns() {
        for keyword in ["true", "false", "null"] {
            let source_text = format!(
                "class C {{ def choose(x: Int): Int = x match {{ case {keyword} => 1 }} }}"
            );
            let (parsed, mut store, packages, definitions, index, source) = setup(&source_text);
            let method = parsed
                .ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::DefDef(definition)
                        if store.names.resolve(definition.name.as_name().text()) == "choose" =>
                    {
                        index.symbol_at(source, tree)
                    }
                    _ => None,
                })
                .unwrap();
            let pattern = parsed
                .ast
                .iter()
                .find_map(|(_tree, node)| match node.kind {
                    TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                    _ => None,
                })
                .unwrap();
            let (mut typer, context) = context_for(
                &parsed,
                &mut store,
                &packages,
                definitions,
                &index,
                source,
                method,
            );
            let error = typer
                .run_expression_transaction(|typer, journal, mappings| {
                    typer.type_pattern(pattern, definitions.int, context, journal, mappings)
                })
                .unwrap_err();
            assert!(matches!(
                error,
                TyperError::UnsupportedPattern { .. }
                    | TyperError::PatternTypeMismatch { .. }
                    | TyperError::NullLiteralTypingDeferred { .. }
            ));
            assert!(typer.pattern_bindings.by_tree.is_empty());
            assert_eq!(typer.typed_arena.iter().count(), 0);
        }
    }

    #[test]
    fn backquoted_underscore_is_not_a_wildcard() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case _ => 1 } }");
        let (method, pattern) = method_and_pattern(&parsed, &store, &index, source, "choose");
        if let TreeKind::Ident(ident) = &mut parsed.ast.get_mut(pattern).kind {
            ident.backquoted = true;
        }
        let source_pattern = parsed.ast.get(pattern);
        assert!(matches!(source_pattern.kind, TreeKind::Ident(ident) if ident.backquoted));
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let error = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.int, context, journal, mappings)
            })
            .unwrap_err();
        assert!(matches!(error, TyperError::TermNameNotFound { .. }));
        assert_eq!(typer.typed_arena.iter().count(), 0);
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn pattern_selector_preserves_constant_types_and_widens_other_types() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case _ => 1 } }");
        let (method, pattern) = method_and_pattern(&parsed, &store, &index, source, "choose");
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let singleton = typer
            .store
            .types
            .alloc(Type::Constant(dotty_core::Constant::Int(42)));
        let mut journal = Vec::new();
        assert_eq!(
            typer
                .pattern_selector_type(singleton, &mut journal)
                .unwrap(),
            singleton
        );
        assert_eq!(
            typer
                .pattern_selector_type(definitions.int, &mut journal)
                .unwrap(),
            definitions.int
        );
        let typed_pattern = typer
            .run_expression_transaction(|typer, info_journal, mappings| {
                typer.type_pattern(pattern, singleton, context, info_journal, mappings)
            })
            .unwrap();
        assert_eq!(typer.typed_arena.get(typed_pattern).ty, singleton);
    }

    #[test]
    fn pattern_root_classifier_uses_stable_categories() {
        let mut store = SemanticStore::new();
        let underscore = store.names.intern("_");
        assert_eq!(
            pattern_kind(&TreeKind::Ident(dotty_core::ast::Ident {
                name: Name::new(underscore, Namespace::Term),
                backquoted: false,
            })),
            PatternKind::Identifier
        );
        assert_eq!(PatternKind::Identifier.as_str(), "identifier");
    }

    #[test]
    fn pattern_root_classifier_covers_common_unsupported_shapes() {
        let (parsed, _store, _packages, _definitions, _index, _source) = setup(
            "class C { def choose(x: Any): Int = x match { case 1 => 1; case y: Int => y; case Some(y) => 2; case 2 | 3 => 3 } }",
        );
        let categories = parsed
            .ast
            .iter()
            .map(|(_, tree)| pattern_kind(&tree.kind))
            .collect::<std::collections::HashSet<_>>();
        assert!(categories.contains(&PatternKind::Literal));
        assert!(categories.contains(&PatternKind::Typed));
        assert!(categories.contains(&PatternKind::Application));
        assert!(categories.contains(&PatternKind::Alternative));
    }
}
