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
    pub(super) is_case_scope: bool,
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

    /// Builds an expression context for an initialized field on an ordinary
    /// source class. The field's recorded lexical context begins at the
    /// primary-constructor parameter scope and chains to the class-body
    /// context at the declaration point. `this` is owned by the class, and no
    /// typer-local method or block scope is pushed.
    pub fn field_initializer_context_for(
        &self,
        field: SymbolId,
    ) -> Result<ExpressionContext, TyperError> {
        if !self.store.symbols.contains(field) {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index: None,
                issue: FieldInitializerContextIssue::SymbolMissing,
            });
        }
        let field_symbol = self.store.symbols.get(field);
        if field_symbol.kind != SymbolKind::Field {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index: None,
                issue: FieldInitializerContextIssue::SymbolKind(field_symbol.kind),
            });
        }
        if field_symbol.flags.contains(SymbolFlags::INLINE) {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index: None,
                issue: FieldInitializerContextIssue::InlineField,
            });
        }
        let Some(definition) = self.index.definition_of(field) else {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index: None,
                issue: FieldInitializerContextIssue::SourceDefinitionMissing,
            });
        };
        let (source, tree) = match definition {
            SourceDefinition::Canonical { source, tree } => (source, tree),
            SourceDefinition::Derived { tree, .. } => {
                return Err(TyperError::FieldInitializerContextInvalid {
                    field,
                    tree_index: Some(tree.index()),
                    issue: FieldInitializerContextIssue::SourceDefinitionMismatch,
                });
            }
        };
        let tree_index = Some(tree.index());
        if source != self.source {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::SourceDefinitionMismatch,
            });
        }
        if self.index.symbol_at(source, tree) != Some(field) {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::SourceTreeSymbolMismatch,
            });
        }
        if field_symbol.origin != SymbolOrigin::Source(self.source) {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::SourceDefinitionMismatch,
            });
        }
        let Some(source_tree) = self.arena.try_get(tree) else {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::SourceTreeMissing,
            });
        };
        let TreeKind::ValDef(definition) = &source_tree.kind else {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::SourceTreeKind,
            });
        };
        let Some(initializer) = definition.rhs else {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::InitializerMissing,
            });
        };
        if self.arena.try_get(initializer).is_none() {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::InitializerTreeMissing,
            });
        }

        let Some(owner) = field_symbol.owner else {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::OwnerMissing,
            });
        };
        if !self.store.symbols.contains(owner) {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::OwnerMissing,
            });
        }
        let owner_symbol = self.store.symbols.get(owner);
        if owner_symbol.kind != SymbolKind::Class {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::OwnerKind {
                    owner,
                    kind: owner_symbol.kind,
                },
            });
        }
        if owner_symbol.origin != SymbolOrigin::Source(self.source) {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::EnclosingClassMalformed { owner },
            });
        }
        let class_tree = match self.index.definition_of(owner) {
            Some(SourceDefinition::Canonical {
                source: class_source,
                tree,
            }) if class_source == self.source
                && self.index.symbol_at(class_source, tree) == Some(owner) =>
            {
                tree
            }
            _ => {
                return Err(TyperError::FieldInitializerContextInvalid {
                    field,
                    tree_index,
                    issue: FieldInitializerContextIssue::EnclosingClassMalformed { owner },
                });
            }
        };
        let class_definition = self.arena.try_get(class_tree).and_then(|node| {
            let TreeKind::TypeDef(definition) = &node.kind else {
                return None;
            };
            let TreeKind::Template(template) = &self.arena.try_get(definition.rhs)?.kind else {
                return None;
            };
            Some(template)
        });
        let Some(class_definition) = class_definition else {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::EnclosingClassMalformed { owner },
            });
        };
        if !class_definition.body.contains(&tree) {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::EnclosingClassMalformed { owner },
            });
        }
        let Some(class_scope) = self.index.scope_of(owner) else {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::EnclosingClassMalformed { owner },
            });
        };
        if !self.store.scopes.contains(class_scope)
            || self.store.scopes.get(class_scope).owner != Some(owner)
        {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::EnclosingClassMalformed { owner },
            });
        }

        let Some(declaration_context) = self.index.declaration_context_of(field) else {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::DeclarationContextMissing,
            });
        };
        let Some(declaration_source_context) = self.index.try_source_context(declaration_context)
        else {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::SourceContextMissing {
                    context: declaration_context,
                },
            });
        };
        if declaration_source_context.owner != owner
            || declaration_source_context.lexical_scope != class_scope
        {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::EnclosingClassMalformed { owner },
            });
        }
        let Some(lexical) = self.index.field_initializer_context_of(field) else {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::InitializerContextMissing,
            });
        };
        let Some(source_context) = self.index.try_source_context(lexical) else {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::SourceContextMissing { context: lexical },
            });
        };
        let context_owner = source_context.owner;
        let context_scope = source_context.lexical_scope;
        let actual_scope_owner = self
            .store
            .scopes
            .contains(context_scope)
            .then(|| self.store.scopes.get(context_scope).owner)
            .flatten();
        if !self.store.symbols.contains(context_owner) {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::InitializerEnvironmentMalformed {
                    context_owner,
                    context_scope,
                    actual_scope_owner,
                },
            });
        }
        let constructor = self.store.symbols.get(context_owner);
        let constructor_tree = self
            .index
            .definition_of(context_owner)
            .and_then(|definition| match definition {
                SourceDefinition::Canonical {
                    source: constructor_source,
                    tree,
                } if constructor_source == self.source => Some(tree),
                _ => None,
            });
        let constructor_matches_class = constructor.kind == SymbolKind::Constructor
            && constructor.owner == Some(owner)
            && constructor_tree == Some(class_definition.constructor)
            && self
                .index
                .symbol_at(self.source, class_definition.constructor)
                == Some(context_owner)
            && actual_scope_owner == Some(context_owner)
            && self.index.scope_of(context_owner) == Some(context_scope);
        let parent_matches_declaration = source_context.parent == Some(declaration_context);
        if !constructor_matches_class || !parent_matches_declaration {
            return Err(TyperError::FieldInitializerContextInvalid {
                field,
                tree_index,
                issue: FieldInitializerContextIssue::InitializerEnvironmentMalformed {
                    context_owner,
                    context_scope,
                    actual_scope_owner,
                },
            });
        }

        Ok(ExpressionContext {
            lexical,
            owner,
            local_scopes: None,
        })
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
            is_case_scope: false,
            imports: Vec::new(),
        });
        Ok(ExpressionContext {
            local_scopes: Some(stack),
            ..context
        })
    }

    /// Creates an isolated typer-owned scope for one Match case. Its frame is
    /// deliberately not a block scope, so block-only declarations remain
    /// unavailable while pattern bindings can still be resolved from it.
    pub(super) fn push_case_scope(
        &mut self,
        context: ExpressionContext,
    ) -> Result<ExpressionContext, TyperError> {
        self.validate_expression_scope_stack(context.local_scopes)?;
        let scope = self
            .store
            .scopes
            .alloc(dotty_core::Scope::new(Some(context.owner)));
        let case_context = self.push_local_scope(context, scope)?;
        let stack =
            case_context
                .local_scopes
                .ok_or(TyperError::ExpressionLocalScopeStackMissing {
                    stack: ExpressionScopeId::new(
                        self.expression_scope_owner,
                        self.expression_scopes.len(),
                    ),
                })?;
        let frame = self
            .expression_scopes
            .get_mut(stack.index())
            .ok_or(TyperError::ExpressionLocalScopeStackMissing { stack })?;
        frame.is_case_scope = true;
        Ok(case_context)
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
