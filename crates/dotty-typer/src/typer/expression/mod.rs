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
pub(super) mod function;
mod match_expr;
mod new;
mod patterns;
mod references;
pub(super) use blocks::LocalMethodIndex;
pub(super) use function::FunctionLiteralIndex;

impl SourceTyper<'_> {
    /// Returns the canonical widened type for supported constants.
    pub(in crate::typer) fn constant_underlying_type(
        &mut self,
        constant: &Constant,
    ) -> Result<Option<TypeId>, ResolutionError> {
        match constant {
            Constant::Unit => Ok(Some(self.definitions.unit)),
            Constant::Boolean(_) => Ok(Some(self.definitions.boolean)),
            Constant::Byte(_) => Ok(Some(self.definitions.byte)),
            Constant::Short(_) => Ok(Some(self.definitions.short)),
            Constant::Char(_) => Ok(Some(self.definitions.char)),
            Constant::Int(_) => Ok(Some(self.definitions.int)),
            Constant::Long(_) => Ok(Some(self.definitions.long)),
            Constant::FloatBits(_) => Ok(Some(self.definitions.float)),
            Constant::DoubleBits(_) => Ok(Some(self.definitions.double)),
            Constant::String(_) | Constant::StringUtf16(_) => {
                self.canonical_java_lang_string_type()
            }
            Constant::Null | Constant::Class(_) => Ok(None),
        }
    }

    fn canonical_java_lang_string_type(&mut self) -> Result<Option<TypeId>, ResolutionError> {
        let store_checkpoint = self.store.checkpoint();
        let resolver_checkpoint = self.resolver.checkpoint();
        let result = self.resolve_canonical_java_lang_string_type();
        if !matches!(result, Ok(Some(_))) {
            self.resolver.rollback_to(self.store, resolver_checkpoint);
            self.store.rollback_to(store_checkpoint);
        }
        result
    }

    fn resolve_canonical_java_lang_string_type(
        &mut self,
    ) -> Result<Option<TypeId>, ResolutionError> {
        let path = ["java", "lang"];
        let package = if let Some(package) = self.packages.symbol(&path) {
            package
        } else {
            let Some(package) = self.resolver.resolve_package(self.store, &path)? else {
                return Ok(None);
            };
            if !self.is_canonical_java_lang_package(package) {
                return Err(ResolutionError::Malformed {
                    reason: "java.lang package resolution returned a non-canonical package"
                        .to_owned(),
                });
            }
            package
        };
        if !self.is_canonical_java_lang_package(package) {
            return Err(ResolutionError::Malformed {
                reason: "registered java.lang package has a non-canonical owner chain".to_owned(),
            });
        }

        let name = Name::new(self.store.names.intern("String"), Namespace::Type);
        if let Some(scope) = self.packages.scope_of(package)
            && self.store.scopes.contains(scope)
        {
            let candidates = self.store.scopes.get(scope).lookup_all(&name);
            if !candidates.is_empty() {
                if candidates.len() > 1 {
                    return Err(ResolutionError::Ambiguous {
                        candidates: candidates.len(),
                    });
                }
                let canonical = candidates
                    .iter()
                    .copied()
                    .filter(|symbol| self.is_canonical_java_lang_string_symbol(*symbol))
                    .collect::<Vec<_>>();
                return match canonical.as_slice() {
                    [symbol] => Ok(Some(self.string_type_reference(*symbol))),
                    [] => Ok(None),
                    _ => Err(ResolutionError::Ambiguous {
                        candidates: canonical.len(),
                    }),
                };
            }
        }

        let prefix = self.package_type_prefix(package);
        let request = MemberRequest {
            prefix,
            name,
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        let Some(symbol) = self.resolver.resolve_member(self.store, &request)? else {
            return Ok(None);
        };
        if !self.store.symbols.contains(symbol) {
            return Err(ResolutionError::Malformed {
                reason: "java.lang.String resolution returned an unknown symbol".to_owned(),
            });
        }
        if !self.is_canonical_java_lang_string_symbol(symbol) {
            return Ok(None);
        }
        Ok(Some(self.string_type_reference(symbol)))
    }

    fn is_canonical_java_lang_package(&self, package: SymbolId) -> bool {
        if !self.store.symbols.contains(package) {
            return false;
        }
        let lang = self.store.symbols.get(package);
        if lang.kind != SymbolKind::Package
            || !lang.name.is_term()
            || self.store.names.resolve(lang.name.text()) != "lang"
        {
            return false;
        }
        let Some(java) = lang.owner else {
            return false;
        };
        if !self.store.symbols.contains(java) {
            return false;
        }
        let java_symbol = self.store.symbols.get(java);
        if java_symbol.kind != SymbolKind::Package
            || !java_symbol.name.is_term()
            || self.store.names.resolve(java_symbol.name.text()) != "java"
        {
            return false;
        }
        let Some(root) = java_symbol.owner else {
            return false;
        };
        if !self.store.symbols.contains(root) {
            return false;
        }
        let root_symbol = self.store.symbols.get(root);
        root_symbol.kind == SymbolKind::Package
            && root_symbol.name.is_term()
            && self.store.names.resolve(root_symbol.name.text()).is_empty()
            && root_symbol.owner.is_none()
    }

    fn string_type_reference(&mut self, symbol: SymbolId) -> TypeId {
        let prefix = self.type_symbol_prefix(symbol);
        self.store.types.alloc(Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(symbol),
        })
    }

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
        let operator_text = self.store.names.resolve(infix.op.text()).to_owned();
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
        let resolved = match self.resolve_infix_application_function(
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
        ) {
            Ok(resolved) => resolved,
            Err(error @ TyperError::MemberNotFound { .. }) if operator_text == "+=" => {
                if let Some(assignment) = self.type_mutable_local_plus_assign_fallback(
                    tree,
                    infix,
                    position,
                    left,
                    receiver_type,
                    context,
                    info_journal,
                    new_mappings,
                )? {
                    return Ok(assignment);
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        };
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

    #[allow(clippy::too_many_arguments)]
    fn type_mutable_local_plus_assign_fallback(
        &mut self,
        tree: TreeId<Untyped>,
        infix: dotty_core::ast::InfixOp,
        position: Option<SourceSpan>,
        left: TreeId<Typed>,
        receiver_type: TypeId,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<Option<TreeId<Typed>>, TyperError> {
        if !matches!(
            self.arena.try_get(infix.left).map(|node| &node.kind),
            Some(TreeKind::Ident(_))
        ) || !self.is_mutable_local_selection_receiver(left, receiver_type)
        {
            return Ok(None);
        }
        let widened_receiver_type =
            self.widen_expression_type_journaled(receiver_type, info_journal, 0)?;
        if !matches!(
            self.conforms(widened_receiver_type, self.definitions.int),
            Ok(true)
        ) {
            return Ok(None);
        }

        // A lexical extension named `+=` has precedence over the assignment
        // rewrite. Infix extension resolution is still unsupported, so leave
        // that case visible instead of silently rewriting around it.
        let name_candidates =
            match self.expression_term_candidates(infix.op, context, tree.index(), position) {
                Ok(candidates) => candidates,
                Err(TyperError::TermNameNotFound { .. }) => Vec::new(),
                Err(_) => return Ok(None),
            };
        if name_candidates.iter().any(|symbol| {
            self.store
                .symbols
                .get(*symbol)
                .flags
                .contains(SymbolFlags::EXTENSION)
        }) {
            return Ok(None);
        }

        let plus = dotty_core::Name::new(self.store.names.intern("+"), dotty_core::Namespace::Term);
        let sum_function = self.resolve_infix_application_function(
            InfixApplicationRequest {
                operator: plus,
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
        let sum = self.type_resolved_application(
            ResolvedApplication {
                tree_index: tree.index(),
                application_kind: ApplyKind::Regular,
                argument_trees: vec![infix.right],
                position,
                context,
                function: sum_function.typed,
                callable: sum_function.callable,
                typed_arguments: Some(sum_function.arguments),
                is_constructor_application: false,
            },
            info_journal,
            new_mappings,
        )?;
        if !matches!(
            self.conforms(self.typed_arena.get(sum).ty, widened_receiver_type),
            Ok(true)
        ) {
            return Ok(None);
        }

        Ok(Some(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).assign(
                left,
                sum,
                self.definitions.unit,
                position,
            ),
        ))
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
        let mut actual =
            self.adapt_expression_type_to_expected(expression_type, expected, info_journal)?;
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

    /// Preserves an exact constant only when the expected type contains a
    /// singleton alternative that needs the literal's identity. Ordinary
    /// expectations continue through normal expression widening.
    pub(in crate::typer) fn adapt_expression_type_to_expected(
        &mut self,
        expression_type: TypeId,
        expected: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        if matches!(
            self.store.types.try_get(expression_type),
            Some(Type::Constant(_))
        ) && self.expected_type_contains_constant(expected, 0)?
        {
            Ok(expression_type)
        } else {
            self.widen_expression_type_journaled(expression_type, info_journal, 0)
        }
    }

    /// Selects between an expression's exact and already computed widened
    /// types without performing additional semantic mutations.
    pub(in crate::typer) fn adapt_pre_widened_expression_type_to_expected(
        &mut self,
        expression_type: TypeId,
        widened_type: TypeId,
        expected: TypeId,
    ) -> Result<TypeId, TyperError> {
        if matches!(
            self.store.types.try_get(expression_type),
            Some(Type::Constant(_))
        ) && self.expected_type_contains_constant(expected, 0)?
        {
            Ok(expression_type)
        } else {
            Ok(widened_type)
        }
    }

    fn expected_type_contains_constant(
        &mut self,
        expected: TypeId,
        depth: usize,
    ) -> Result<bool, TyperError> {
        if depth >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            return Err(TyperError::TypeNormalization(
                crate::types::TypeNormalizeError::TooDeep,
            ));
        }
        let expected = crate::types::TypeNormalizer::new(self.store)
            .normalize_for_lookup(expected)
            .map_err(TyperError::TypeNormalization)?;
        let Some(node) = self.store.types.try_get(expected).cloned() else {
            return Err(TyperError::TypeNormalization(
                if self.store.types.contains(expected) {
                    crate::types::TypeNormalizeError::UnfilledType { ty: expected }
                } else {
                    crate::types::TypeNormalizeError::InvalidType { ty: expected }
                },
            ));
        };
        match node {
            Type::Constant(_) => Ok(true),
            Type::Or { left, right } => Ok(self
                .expected_type_contains_constant(left, depth + 1)?
                || self.expected_type_contains_constant(right, depth + 1)?),
            _ => Ok(false),
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
        match expression_type {
            Type::Constant(value) => self
                .constant_underlying_type(&value)
                .map_err(|error| TyperError::StringTypeResolution { error })?
                .ok_or(TyperError::ExpressionTypeCannotBeWidened { ty }),
            Type::TermRef { prefix, target } => {
                let TermRefTarget::Symbol(symbol) = target else {
                    return Err(TyperError::ExpressionTypeCannotBeWidened { ty });
                };
                if !self.store.symbols.contains(symbol) {
                    return Err(TyperError::UnknownSymbol { symbol });
                }
                let kind = self.store.symbols.get(symbol).kind;
                if kind == SymbolKind::Object {
                    let module_class = self.module_class_of_object(symbol)?;
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
                // Canonical AnyRef identity members deliberately do not live
                // in ordinary class member scopes. Preserve the verified
                // selection through application widening instead of looking
                // the compiler-owned symbol up a second time.
                if self.is_any_ref_identity_symbol(symbol) {
                    let receiver =
                        self.widen_expression_type_journaled(prefix, info_journal, depth + 1)?;
                    let receiver = self.this_type_receiver_view(receiver)?;
                    let name = self.store.symbols.get(symbol).name;
                    if self.any_ref_identity_symbol_for_receiver(receiver, name) == Some(symbol)
                        && self
                            .lookup_overload_members_journaled(receiver, name, info_journal)
                            .map_err(|error| TyperError::MemberLookup(Box::new(error)))?
                            .is_empty()
                    {
                        return self.completed_expression_symbol_info(symbol, info_journal);
                    }
                    return Err(TyperError::TermReferencePrefixMismatch {
                        symbol,
                        prefix: receiver,
                    });
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
                    let info = self.completed_expression_symbol_info(symbol, info_journal)?;
                    if kind == SymbolKind::Parameter
                        && let Some(Type::ByName { result }) = self.store.types.try_get(info)
                    {
                        return Ok(*result);
                    }
                    Ok(info)
                } else {
                    let super_type = match self.store.types.try_get(prefix) {
                        Some(Type::SuperType { super_type, .. }) => Some(*super_type),
                        _ => None,
                    };
                    if let Some(super_type) = super_type {
                        let name = self.store.symbols.get(symbol).name;
                        let candidates = self
                            .lookup_members_journaled(super_type, name, info_journal)
                            .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
                        let candidate = candidates
                            .into_iter()
                            .find(|candidate| candidate.symbol == symbol)
                            .ok_or(TyperError::TermReferencePrefixMismatch {
                                symbol,
                                prefix: super_type,
                            })?;
                        return self.member_type_on_journaled(&candidate, info_journal);
                    }
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
            TreeKind::SingletonTypeTree(_) => true,
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
