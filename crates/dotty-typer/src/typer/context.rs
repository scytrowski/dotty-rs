//! Lexical typing contexts and typer-owned expression scope stacks.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_EXPRESSION_SCOPE_OWNER: AtomicU64 = AtomicU64::new(1);

pub(super) fn next_expression_scope_owner() -> u64 {
    NEXT_EXPRESSION_SCOPE_OWNER
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
            next.checked_add(1)
        })
        .expect("expression scope owner identity space exhausted")
}

/// The source/import context, typer-local scope stack, and semantic owner used
/// to type an expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpressionContext {
    /// Immutable source lexical context for scope and import lookup.
    pub lexical: SourceContextId,
    /// Semantic declaration that owns the expression, used for `this`.
    pub owner: SymbolId,
    /// Head of the typer-owned innermost-first local scope stack.
    pub local_scopes: Option<ExpressionScopeId>,
}

/// Opaque head identity in one `SourceTyper`'s persistent local scope stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExpressionScopeId {
    owner: u64,
    index: usize,
}

impl ExpressionScopeId {
    pub(super) const fn new(owner: u64, index: usize) -> Self {
        Self { owner, index }
    }

    /// Returns the stack-frame index backing this identity.
    pub const fn index(self) -> usize {
        self.index
    }
}

#[derive(Clone)]
pub(super) struct ExpressionScopeFrame {
    pub(super) scope: dotty_core::ScopeId,
    pub(super) parent: Option<ExpressionScopeId>,
    pub(super) is_block_scope: bool,
    pub(super) imports: Vec<(TreeId<Untyped>, ExpressionContext)>,
}

impl SourceTyper<'_> {
    pub(super) fn method_declaration_context(&self, method: SymbolId) -> Option<ExpressionContext> {
        self.local_methods.declaration_context(method).or_else(|| {
            self.index
                .declaration_context_of(method)
                .map(|lexical| ExpressionContext {
                    lexical,
                    owner: method,
                    local_scopes: None,
                })
        })
    }

    pub(super) fn method_scope(&self, method: SymbolId) -> Result<ScopeId, TyperError> {
        match self.local_methods.scope(method) {
            Some(scope) => Ok(scope),
            None => Self::indexed_method_scope(method, self.index.scope_of(method)),
        }
    }

    pub(super) fn parameter_source_context(&self, parameter: SymbolId) -> Option<SourceContextId> {
        self.local_methods
            .parameter_context(parameter)
            .or_else(|| self.local_methods.type_parameter_context(parameter))
            .or_else(|| self.index.declaration_context_of(parameter))
    }

    /// Builds the lexical context for a method or constructor body from the
    /// authoritative source identities in the semantic index.
    pub fn expression_context_for(
        &mut self,
        owner: SymbolId,
    ) -> Result<ExpressionContext, TyperError> {
        if !self.store.symbols.contains(owner) {
            return Err(TyperError::ExpressionOwnerMissing { owner });
        }
        let kind = self.store.symbols.get(owner).kind;
        if !matches!(kind, SymbolKind::Method | SymbolKind::Constructor) {
            return Err(TyperError::ExpressionOwnerKindUnsupported { owner, kind });
        }
        let declaration_context = self
            .method_declaration_context(owner)
            .ok_or(TyperError::ExpressionOwnerDeclarationContextMissing { owner })?;
        let lexical = declaration_context.lexical;
        if self.index.try_source_context(lexical).is_none() {
            return Err(TyperError::ExpressionOwnerSourceContextMissing {
                owner,
                context: lexical,
            });
        }
        let scope = self.method_scope(owner)?;
        if !self.store.scopes.contains(scope) {
            return Err(TyperError::ExpressionMethodScopeMissing { owner });
        }
        let actual_owner = self.store.scopes.get(scope).owner;
        if actual_owner != Some(owner) {
            return Err(TyperError::ExpressionMethodScopeOwnerMismatch {
                owner,
                scope,
                actual: actual_owner,
            });
        }
        self.push_local_scope(
            ExpressionContext {
                owner,
                ..declaration_context
            },
            scope,
        )
    }

    pub(super) fn expression_type_context(
        &self,
        context: ExpressionContext,
    ) -> Result<SourceContextId, TyperError> {
        let Some(SourceDefinition::Canonical { tree, .. }) =
            self.index.definition_of(context.owner)
        else {
            return Ok(context.lexical);
        };
        let Some(source_tree) = self.arena.try_get(tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: tree.index(),
            });
        };
        let TreeKind::DefDef(definition) = &source_tree.kind else {
            return Ok(context.lexical);
        };
        let extension_type_parameter = self
            .index
            .extension_prefix_clauses(context.owner)
            .into_iter()
            .flatten()
            .flatten()
            .find(|tree| {
                self.arena
                    .try_get(**tree)
                    .is_some_and(|node| matches!(node.kind, TreeKind::TypeDef(_)))
            })
            .copied()
            .map(|tree| (tree, true));
        let type_parameter = extension_type_parameter.or_else(|| {
            definition
                .type_params
                .first()
                .copied()
                .map(|tree| (tree, false))
        });
        let Some((tree, derived)) = type_parameter else {
            return Ok(context.lexical);
        };
        let symbol = if derived {
            self.index
                .derived_symbol_at(context.owner, self.source, tree)
        } else {
            self.index.symbol_at(self.source, tree)
        }
        .ok_or(TyperError::MethodTypeParameterSymbolMissing {
            method: context.owner,
            parameter_tree_index: tree.index(),
        })?;
        self.index
            .declaration_context_of(symbol)
            .ok_or(TyperError::DeclarationContextMissing { symbol })
    }

    /// Returns a context with `scope` pushed at the innermost lexical depth.
    /// This lets later expression forms such as `Block` add local scopes
    /// without changing the immutable source-context model.
    pub fn push_local_scope(
        &mut self,
        context: ExpressionContext,
        scope: dotty_core::ScopeId,
    ) -> Result<ExpressionContext, TyperError> {
        if !self.store.scopes.contains(scope) {
            return Err(TyperError::ExpressionLocalScopeMissing { scope });
        }
        if let Some(parent) = context.local_scopes {
            self.validate_expression_scope_stack(Some(parent))?;
        }
        let stack = ExpressionScopeId {
            owner: self.expression_scope_owner,
            index: self.expression_scopes.len(),
        };
        self.expression_scopes.push(ExpressionScopeFrame {
            scope,
            parent: context.local_scopes,
            is_block_scope: false,
            imports: Vec::new(),
        });
        Ok(ExpressionContext {
            local_scopes: Some(stack),
            ..context
        })
    }

    pub(super) fn indexed_method_scope(
        owner: SymbolId,
        scope: Option<dotty_core::ScopeId>,
    ) -> Result<dotty_core::ScopeId, TyperError> {
        scope.ok_or(TyperError::ExpressionMethodScopeMissing { owner })
    }

    pub(super) fn validate_expression_scope_stack(
        &self,
        mut stack: Option<ExpressionScopeId>,
    ) -> Result<(), TyperError> {
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = stack {
            if id.owner != self.expression_scope_owner {
                return Err(TyperError::ExpressionLocalScopeStackForeign { stack: id });
            }
            if !seen.insert(id) {
                return Err(TyperError::ExpressionLocalScopeStackMissing { stack: id });
            }
            let Some(frame) = self.expression_scopes.get(id.index()) else {
                return Err(TyperError::ExpressionLocalScopeStackMissing { stack: id });
            };
            if !self.store.scopes.contains(frame.scope) {
                return Err(TyperError::ExpressionLocalScopeMissing { scope: frame.scope });
            }
            stack = frame.parent;
        }
        Ok(())
    }
}
