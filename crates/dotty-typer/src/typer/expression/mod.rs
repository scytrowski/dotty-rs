//! Expression typing orchestration and shared expression helpers.

use super::application::{
    InfixApplicationRequest, ResolvedApplication, ZeroArgumentSelectionError,
};
use super::{ExpressionContext, SourceTyper, TyperError, tree_kind_name};
use dotty_core::ast::*;
use dotty_core::types::*;
use dotty_core::*;

mod annotated;
mod assignment;
pub(super) mod blocks;
mod control_flow;
mod match_expr;
mod new;
mod patterns;
mod references;
pub(super) use blocks::LocalMethodIndex;

impl SourceTyper<'_> {
    pub(super) fn type_prefix_expression(
        &mut self,
        tree: TreeId<Untyped>,
        prefix: dotty_core::ast::PrefixOp,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let unary_name = match self.store.names.resolve(prefix.op.text()) {
            "!" => "unary_!",
            "~" => "unary_~",
            "+" => "unary_+",
            "-" => "unary_-",
            _ => {
                return Err(TyperError::UnsupportedPrefixOperator {
                    source: self.source,
                    tree_index: tree.index(),
                    operator: prefix.op,
                });
            }
        };
        if !prefix.op.is_term() {
            return Err(TyperError::UnsupportedPrefixOperator {
                source: self.source,
                tree_index: tree.index(),
                operator: prefix.op,
            });
        }
        let unary_name = Name::new(self.store.names.intern(unary_name), Namespace::Term);
        let operand =
            self.type_value_expression_inner(prefix.operand, context, info_journal, new_mappings)?;
        self.type_zero_argument_selected_call(
            tree.index(),
            operand,
            unary_name,
            position,
            info_journal,
        )
        .map_err(|error| match error {
            ZeroArgumentSelectionError::Selection(error) => error,
            ZeroArgumentSelectionError::MethodNeedsArgumentList => {
                TyperError::PrefixMethodNeedsArgumentList {
                    source: self.source,
                    tree_index: tree.index(),
                    name: unary_name,
                }
            }
            ZeroArgumentSelectionError::PolymorphicDeferred => {
                TyperError::PrefixPolymorphicDeferred {
                    source: self.source,
                    tree_index: tree.index(),
                    name: unary_name,
                }
            }
        })
    }

    pub(super) fn type_infix_expression(
        &mut self,
        tree: TreeId<Untyped>,
        infix: dotty_core::ast::InfixOp,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let operator_text = self.store.names.resolve(infix.op.text());
        if operator_text.ends_with(':') {
            return Err(TyperError::RightAssociativeInfixDeferred {
                source: self.source,
                tree_index: tree.index(),
                operator: infix.op,
            });
        }

        let left =
            self.type_value_expression_inner(infix.left, context, info_journal, new_mappings)?;
        let receiver_type = self.typed_arena.get(left).ty;
        let resolved = self.resolve_infix_application_function(
            InfixApplicationRequest {
                operator: infix.op,
                qualifier: left,
                receiver_type,
                argument_tree: infix.right,
                context,
                tree_index: tree.index(),
                position,
            },
            info_journal,
            new_mappings,
        )?;
        self.type_resolved_application(
            ResolvedApplication {
                tree_index: tree.index(),
                application_kind: ApplyKind::Regular,
                argument_trees: vec![infix.right],
                position,
                context,
                function: resolved.typed,
                callable: resolved.callable,
                typed_arguments: Some(resolved.arguments),
                is_constructor_application: false,
            },
            info_journal,
            new_mappings,
        )
    }

    pub(super) fn type_parenthesized_expression(
        &mut self,
        inner: TreeId<Untyped>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        // Parens is source-only. Dotty's typed tree keeps the enclosed
        // expression, including its position and own type, as the result.
        // `type_expression_inner` records the inner source mapping; the
        // dispatcher records the wrapper against this same typed identity.
        self.type_expression_inner(inner, context, info_journal, new_mappings)
    }

    pub(in crate::typer) fn type_expression_expected_inner(
        &mut self,
        tree: TreeId<Untyped>,
        context: ExpressionContext,
        expected: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let typed = self.type_value_expression_inner(tree, context, info_journal, new_mappings)?;
        let expression_type = self.typed_arena.get(typed).ty;
        let mut actual = self.widen_expression_type_journaled(expression_type, info_journal, 0)?;
        for (binder, parameters) in self.active_local_type_binders.iter().rev() {
            if !self.type_contains_param_ref(expected, *binder)? {
                continue;
            }
            let substitutions = parameters
                .iter()
                .enumerate()
                .map(|(index, symbol)| {
                    (
                        *symbol,
                        self.store.types.alloc(Type::ParamRef {
                            binder: *binder,
                            index: index as u32,
                        }),
                    )
                })
                .collect::<Vec<_>>();
            actual = dotty_core::types::substitute_type_symbols(self.store, actual, &substitutions)
                .map_err(|error| TyperError::TypeRebinding {
                    source: self.source,
                    tree_index: tree.index(),
                    error,
                })?;
        }
        match self.conforms(actual, expected) {
            Ok(true) => Ok(typed),
            Ok(false) => Err(TyperError::ExpectedExpressionTypeMismatch {
                source: self.source,
                tree_index: tree.index(),
                actual,
                expected,
            }),
            Err(error) => Err(TyperError::ExpectedExpressionConformanceUnsupported {
                source: self.source,
                tree_index: tree.index(),
                actual,
                expected,
                error: Box::new(error),
            }),
        }
    }

    pub(in crate::typer) fn widen_expression_type_journaled(
        &mut self,
        ty: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        depth: usize,
    ) -> Result<TypeId, TyperError> {
        if depth >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            return Err(TyperError::TypeNormalization(
                crate::types::TypeNormalizeError::TooDeep,
            ));
        }
        let Some(expression_type) = self.store.types.try_get(ty).cloned() else {
            return Err(TyperError::TypeNormalization(
                if self.store.types.contains(ty) {
                    crate::types::TypeNormalizeError::UnfilledType { ty }
                } else {
                    crate::types::TypeNormalizeError::InvalidType { ty }
                },
            ));
        };
        use dotty_core::Constant;
        match expression_type {
            Type::Constant(value) => match value {
                Constant::Unit => Ok(self.definitions.unit),
                Constant::Boolean(_) => Ok(self.definitions.boolean),
                Constant::Byte(_) => Ok(self.definitions.byte),
                Constant::Short(_) => Ok(self.definitions.short),
                Constant::Char(_) => Ok(self.definitions.char),
                Constant::Int(_) => Ok(self.definitions.int),
                Constant::Long(_) => Ok(self.definitions.long),
                Constant::FloatBits(_) => Ok(self.definitions.float),
                Constant::DoubleBits(_) => Ok(self.definitions.double),
                Constant::String(_)
                | Constant::StringUtf16(_)
                | Constant::Null
                | Constant::Class(_) => Err(TyperError::ExpressionTypeCannotBeWidened { ty }),
            },
            Type::TermRef { prefix, target } => {
                let TermRefTarget::Symbol(symbol) = target else {
                    return Err(TyperError::ExpressionTypeCannotBeWidened { ty });
                };
                if !self.store.symbols.contains(symbol) {
                    return Err(TyperError::UnknownSymbol { symbol });
                }
                let kind = self.store.symbols.get(symbol).kind;
                if kind == SymbolKind::Object {
                    let module_class = self.source_module_class_of_object(symbol)?;
                    let module_prefix = if prefix == self.definitions.no_prefix {
                        self.type_symbol_prefix(module_class)
                    } else {
                        prefix
                    };
                    return Ok(self.store.types.alloc(Type::TypeRef {
                        prefix: module_prefix,
                        target: TypeRefTarget::Symbol(module_class),
                    }));
                }
                if !matches!(
                    kind,
                    SymbolKind::Field
                        | SymbolKind::Value
                        | SymbolKind::Variable
                        | SymbolKind::Parameter
                        | SymbolKind::Local
                        | SymbolKind::Method
                ) {
                    return Err(TyperError::TermReferenceCannotBeWidened { symbol, kind });
                }
                if prefix == self.definitions.no_prefix {
                    self.completed_expression_symbol_info(symbol, info_journal)
                } else {
                    let receiver =
                        self.widen_expression_type_journaled(prefix, info_journal, depth + 1)?;
                    let receiver = self.this_type_receiver_view(receiver)?;
                    let name = self.store.symbols.get(symbol).name;
                    let candidates = self
                        .lookup_members_journaled(receiver, name, info_journal)
                        .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
                    let candidate = match candidates
                        .into_iter()
                        .find(|candidate| candidate.symbol == symbol)
                    {
                        Some(candidate) => candidate,
                        None => self
                            .lookup_overload_members_journaled(receiver, name, info_journal)
                            .map_err(|error| TyperError::MemberLookup(Box::new(error)))?
                            .into_iter()
                            .find(|candidate| candidate.symbol == symbol)
                            .ok_or(TyperError::TermReferencePrefixMismatch {
                                symbol,
                                prefix: receiver,
                            })?,
                    };
                    self.member_type_on_journaled(&candidate, info_journal)
                }
            }
            Type::NoType | Type::NoPrefix | Type::Error(_) => {
                Err(TyperError::ExpressionTypeCannotBeWidened { ty })
            }
            Type::Repeated { .. } => Err(TyperError::ExpressionTypeCannotBeWidened { ty }),
            _ => Ok(ty),
        }
    }

    pub(in crate::typer) fn reify_type_argument(
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
        let supported = match &source_node.kind {
            TreeKind::Ident(ident) => ident.name.is_type(),
            TreeKind::Select(selection) => selection.name.is_type(),
            TreeKind::AppliedTypeTree(_) => true,
            _ => false,
        };
        if !supported {
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

    pub(in crate::typer) fn reify_type_ascription_tree(
        &mut self,
        source_tree: TreeId<Untyped>,
        ty: TypeId,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(typed) = self.typed_index.get(self.source, source_tree) {
            let node = self.typed_arena.get(typed);
            if matches!(node.kind, TreeKind::TypeTree(_)) && node.ty == ty {
                return Ok(typed);
            }
            let source_node = self.arena.try_get(source_tree);
            return Err(TyperError::TypeAscriptionTreeCannotBeReified {
                source: self.source,
                tree_index: source_tree.index(),
                tree_kind: source_node.map_or("tree outside source arena", |node| {
                    tree_kind_name(&node.kind)
                }),
            });
        }
        let Some(source_node) = self.arena.try_get(source_tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: source_tree.index(),
            });
        };
        let supported = match &source_node.kind {
            TreeKind::Ident(ident) => ident.name.is_type(),
            TreeKind::Select(selection) => selection.name.is_type(),
            TreeKind::AppliedTypeTree(_) | TreeKind::ByNameTypeTree(_) => true,
            TreeKind::PhaseSpecific(UntypedNode::Parens(_)) => true,
            _ => false,
        };
        if !supported {
            return Err(TyperError::TypeAscriptionTreeCannotBeReified {
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
    pub(in crate::typer) fn type_ascription_expression(
        &mut self,
        _tree: TreeId<Untyped>,
        ascription: dotty_core::ast::TypedExpr<Untyped>,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let type_context = self.expression_type_context(context)?;
        let expected =
            self.type_of_tpt_inner_journaled(ascription.tpt, type_context, info_journal)?;
        let expr = self.type_expression_expected_inner(
            ascription.expr,
            context,
            expected,
            info_journal,
            new_mappings,
        )?;
        let tpt = self.reify_type_ascription_tree(ascription.tpt, expected, new_mappings)?;
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                .typed_expr(expr, tpt, expected, position),
        )
    }
}
