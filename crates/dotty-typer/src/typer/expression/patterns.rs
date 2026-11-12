//! Pattern typing entry points and pattern-specific type adaptation.

use std::collections::HashSet;

use super::{ExpressionContext, SourceTyper, TyperError};
use crate::TypeRelationError;
use crate::typer::{
    ExtractorMethodShapeIssue, ExtractorPatternArgumentIssue, ExtractorProductIssue,
    ExtractorResultMemberIssue, PatternKind, TuplePatternResolutionIssue,
};
use dotty_core::ast::{TreeKind, TypedAstBuilder, UntypedNode};
use dotty_core::types::{MethodKind, MethodType, Type};
use dotty_core::{
    Name, Namespace, SemanticStore, SourceDefinition, SourceId, SymbolFlags, SymbolId, SymbolInfo,
    SymbolKind, SymbolOrigin, TermRefTarget, TreeId, TypeId, TypeRefTarget, Typed, Untyped,
};

const MAX_REIFIABLE_TYPE_PREFIX_DEPTH: usize = 64;
pub(super) const MAX_CANONICAL_TUPLE_ARITY: usize = 22;

#[derive(Clone, Copy)]
pub(super) struct CanonicalTupleSymbols {
    pub(super) class: SymbolId,
    pub(super) companion: SymbolId,
}

/// Resolved extractor metadata retained for later result-protocol and nested
/// pattern typing increments.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ExtractorPlan {
    function: TreeId<Typed>,
    symbol: SymbolId,
    input_type: TypeId,
    result_type: TypeId,
    unapply_type: TypeId,
    source_patterns: Vec<TreeId<Untyped>>,
}

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
    fn tuple_companion_receiver(
        &mut self,
        companion_type: TypeId,
        pattern: TreeId<Untyped>,
        arity: usize,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let receiver = self
            .widen_expression_type_journaled(companion_type, info_journal, 0)
            .map_err(|error| match error {
                TyperError::ObjectModuleClassUnavailable { .. } => self
                    .tuple_pattern_resolution_error(
                        pattern,
                        arity,
                        TuplePatternResolutionIssue::CompanionModuleClassUnavailable,
                    ),
                error => error,
            })?;
        self.this_type_receiver_view(receiver)
    }

    fn extractor_result_member_type(
        &mut self,
        result_type: TypeId,
        member_name: Name,
        pattern_index: u32,
        unapply: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        // Extractor result protocols select value members using ordinary
        // receiver precedence. Overload application must discover hidden
        // inherited buckets before it can test applicability; a protocol
        // member such as Tuple2._1 instead follows the direct member bucket,
        // which hides the matching inherited Product2._1 declaration.
        let members = match self.lookup_members_journaled(result_type, member_name, info_journal) {
            Ok(members) => members,
            Err(crate::typer::lookup::MemberLookupError::ClassInfoUnavailable { .. })
            | Err(crate::typer::lookup::MemberLookupError::UnsupportedReceiverType { .. }) => {
                return Err(TyperError::UnsupportedExtractorResultProtocol {
                    source: self.source,
                    tree_index: pattern_index,
                    unapply,
                    result: result_type,
                });
            }
            Err(error) => return Err(TyperError::MemberLookup(Box::new(error))),
        };
        if members.is_empty() {
            return Err(TyperError::ExtractorResultMemberNotFound {
                source: self.source,
                tree_index: pattern_index,
                result: result_type,
                member: member_name,
            });
        }
        let candidates = members
            .into_iter()
            .map(|member| {
                let callable = self.member_type_on_journaled(&member, info_journal)?;
                Ok(crate::typer::application::ApplicationCandidate {
                    symbol: member.symbol,
                    callable,
                    member: Some(member),
                    rejection: None,
                })
            })
            .collect::<Result<Vec<_>, TyperError>>()?;
        let candidate = match candidates.as_slice() {
            [] => {
                return Err(TyperError::ExtractorResultMemberNotFound {
                    source: self.source,
                    tree_index: pattern_index,
                    result: result_type,
                    member: member_name,
                });
            }
            [candidate] => *candidate,
            _ => {
                return Err(TyperError::ExtractorResultMemberOverloaded {
                    source: self.source,
                    tree_index: pattern_index,
                    result: result_type,
                    member: member_name,
                    candidates: candidates
                        .iter()
                        .map(|candidate| candidate.symbol)
                        .collect(),
                });
            }
        };
        let callable = candidate.callable;
        let authenticated_product_selector = self
            .source_case_class_product_selector_owner(candidate.symbol)
            .is_some();
        let value_type = match self.store.types.try_get(callable) {
            Some(Type::Method(method))
                if method.kind == MethodKind::Plain
                    && method.params.is_empty()
                    && authenticated_product_selector =>
            {
                method.result
            }
            Some(Type::Method(_)) => {
                return Err(TyperError::ExtractorResultMemberUnsupported {
                    source: self.source,
                    tree_index: pattern_index,
                    result: result_type,
                    member: member_name,
                    member_type: callable,
                    issue: ExtractorResultMemberIssue::NotParameterless,
                });
            }
            Some(Type::Poly(_)) => {
                return Err(TyperError::ExtractorResultMemberUnsupported {
                    source: self.source,
                    tree_index: pattern_index,
                    result: result_type,
                    member: member_name,
                    member_type: callable,
                    issue: ExtractorResultMemberIssue::NotValueType,
                });
            }
            Some(Type::ByName { result })
                if self.store.symbols.get(candidate.symbol).kind == SymbolKind::Method
                    && matches!(
                        self.store.symbols.get(candidate.symbol).origin,
                        SymbolOrigin::Tasty(_)
                    ) =>
            {
                // Semantic TASTy represents a no-term-clause method as
                // ExprType. At a value-member boundary consume its result
                // type while retaining the canonical stored signature.
                *result
            }
            Some(_) => callable,
            None => {
                return Err(TyperError::ExtractorResultMemberUnsupported {
                    source: self.source,
                    tree_index: pattern_index,
                    result: result_type,
                    member: member_name,
                    member_type: callable,
                    issue: ExtractorResultMemberIssue::NotValueType,
                });
            }
        };
        if !self.is_supported_extractor_value_type(value_type) {
            return Err(TyperError::ExtractorResultMemberUnsupported {
                source: self.source,
                tree_index: pattern_index,
                result: result_type,
                member: member_name,
                member_type: value_type,
                issue: ExtractorResultMemberIssue::NotValueType,
            });
        }
        Ok(value_type)
    }

    fn is_supported_extractor_value_type(&self, ty: TypeId) -> bool {
        !matches!(
            self.store.types.try_get(ty),
            None | Some(
                Type::NoType
                    | Type::Error(_)
                    | Type::NoPrefix
                    | Type::Bounds { .. }
                    | Type::AliasingBounds { .. }
                    | Type::ByName { .. }
                    | Type::Method(_)
                    | Type::Poly(_)
                    | Type::TypeLambda(_)
                    | Type::Wildcard { .. }
                    | Type::ClassInfo(_)
            )
        )
    }

    fn option_like_extractor_component_type(
        &mut self,
        result_type: TypeId,
        pattern_index: u32,
        unapply: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let is_empty_name = Name::new(self.store.names.intern("isEmpty"), Namespace::Term);
        let is_empty_type = self.extractor_result_member_type(
            result_type,
            is_empty_name,
            pattern_index,
            unapply,
            info_journal,
        )?;
        let widened_is_empty =
            self.widen_expression_type_journaled(is_empty_type, info_journal, 0)?;
        if widened_is_empty != self.definitions.boolean {
            return Err(TyperError::ExtractorResultMemberUnsupported {
                source: self.source,
                tree_index: pattern_index,
                result: result_type,
                member: is_empty_name,
                member_type: is_empty_type,
                issue: ExtractorResultMemberIssue::IsEmptyNotBoolean,
            });
        }

        let get_name = Name::new(self.store.names.intern("get"), Namespace::Term);
        self.extractor_result_member_type(
            result_type,
            get_name,
            pattern_index,
            unapply,
            info_journal,
        )
    }

    /// Reads numbered product selectors in source order. The source arity
    /// bounds traversal; probing the next selector detects wider products
    /// without imposing a fixed maximum arity.
    fn product_extractor_component_types(
        &mut self,
        result_type: TypeId,
        pattern_index: u32,
        unapply: SymbolId,
        expected_arity: usize,
        require_product_subtype: bool,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<Vec<TypeId>>, TyperError> {
        if expected_arity == 0 {
            return Ok(None);
        }
        if require_product_subtype
            && !self.is_product_subtype(result_type, pattern_index, unapply, info_journal)?
        {
            return Ok(None);
        }

        let mut selectors = Vec::new();
        let mut component_types = Vec::new();
        for index in 1..=expected_arity.saturating_add(1) {
            let spelling = format!("_{index}");
            let name = Name::new(self.store.names.intern(&spelling), Namespace::Term);
            match self.extractor_result_member_type(
                result_type,
                name,
                pattern_index,
                unapply,
                info_journal,
            ) {
                Ok(component_type) => {
                    selectors.push(name);
                    component_types.push((index, component_type));
                }
                Err(TyperError::ExtractorResultMemberNotFound { .. })
                | Err(TyperError::UnsupportedExtractorResultProtocol { .. }) => {}
                Err(TyperError::ExtractorResultMemberUnsupported {
                    issue:
                        ExtractorResultMemberIssue::NotParameterless
                        | ExtractorResultMemberIssue::NotValueType,
                    ..
                }) if index == expected_arity.saturating_add(1) => {}
                Err(error) => return Err(error),
            }
        }
        if selectors.is_empty() {
            return Ok(None);
        }
        if component_types.len() != expected_arity
            || component_types
                .iter()
                .enumerate()
                .any(|(position, (index, _))| *index != position + 1)
        {
            return Err(TyperError::ExtractorProductSelectorCountMismatch {
                source: self.source,
                tree_index: pattern_index,
                unapply,
                result: result_type,
                selectors,
            });
        }
        Ok(Some(
            component_types
                .into_iter()
                .map(|(_, component_type)| component_type)
                .collect(),
        ))
    }

    fn tuple_pattern_resolution_error(
        &self,
        pattern: TreeId<Untyped>,
        arity: usize,
        issue: TuplePatternResolutionIssue,
    ) -> TyperError {
        TyperError::TuplePatternResolutionDeferred {
            source: self.source,
            tree_index: pattern.index(),
            arity,
            issue,
        }
    }

    pub(super) fn resolve_canonical_tuple_symbols(
        &mut self,
        pattern: TreeId<Untyped>,
        arity: usize,
    ) -> Result<CanonicalTupleSymbols, TyperError> {
        let tree_index = pattern.index();
        let tuple_name_text = format!("Tuple{arity}");
        let tuple_type_name = Name::new(self.store.names.intern(&tuple_name_text), Namespace::Type);
        let tuple_term_name = Name::new(tuple_type_name.text(), Namespace::Term);
        let scala_package = match self.packages.symbol(&["scala"]) {
            Some(package) => Some(package),
            None => self.resolve_external_package(&["scala".to_owned()], tree_index)?,
        }
        .ok_or_else(|| {
            self.tuple_pattern_resolution_error(
                pattern,
                arity,
                TuplePatternResolutionIssue::TupleClassNotFound,
            )
        })?;
        let package_prefix = self.package_type_prefix(scala_package);
        let local_type_candidates = self
            .packages
            .scope_of(scala_package)
            .map(|scope| {
                self.store
                    .scopes
                    .get(scope)
                    .lookup_all(&tuple_type_name)
                    .iter()
                    .copied()
                    .filter(|symbol| self.store.symbols.get(*symbol).kind == SymbolKind::Class)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let tuple_class = match local_type_candidates.as_slice() {
            [class] => *class,
            [] => {
                let request = dotty_core::MemberRequest {
                    prefix: package_prefix,
                    name: tuple_type_name,
                    selector: dotty_core::MemberSelector::Unique,
                    space: dotty_core::MemberSpace::Prefix,
                };
                self.resolver
                    .resolve_member(self.store, &request)
                    .map_err(|error| TyperError::SymbolResolution {
                        source: self.source,
                        tree_index,
                        error,
                    })?
                    .filter(|symbol| {
                        self.store.symbols.contains(*symbol)
                            && self.store.symbols.get(*symbol).kind == SymbolKind::Class
                    })
                    .ok_or_else(|| {
                        self.tuple_pattern_resolution_error(
                            pattern,
                            arity,
                            TuplePatternResolutionIssue::TupleClassNotFound,
                        )
                    })?
            }
            _ => {
                return Err(self.tuple_pattern_resolution_error(
                    pattern,
                    arity,
                    TuplePatternResolutionIssue::TupleClassNotFound,
                ));
            }
        };
        let linked_companion = self.store.symbols.get(tuple_class).links.companion;
        let scoped_companion = linked_companion
            .filter(|symbol| {
                self.store.symbols.contains(*symbol)
                    && self.store.symbols.get(*symbol).kind == SymbolKind::Object
            })
            .or_else(|| {
                self.packages.scope_of(scala_package).and_then(|scope| {
                    let objects = self
                        .store
                        .scopes
                        .get(scope)
                        .lookup_all(&tuple_term_name)
                        .iter()
                        .copied()
                        .filter(|symbol| self.store.symbols.get(*symbol).kind == SymbolKind::Object)
                        .collect::<Vec<_>>();
                    match objects.as_slice() {
                        [object] => Some(*object),
                        _ => None,
                    }
                })
            });
        let companion = match scoped_companion {
            Some(object) => object,
            None => {
                let request = dotty_core::MemberRequest {
                    prefix: package_prefix,
                    name: tuple_term_name,
                    selector: dotty_core::MemberSelector::Unique,
                    space: dotty_core::MemberSpace::Prefix,
                };
                self.resolver
                    .resolve_member(self.store, &request)
                    .map_err(|error| TyperError::SymbolResolution {
                        source: self.source,
                        tree_index,
                        error,
                    })?
                    .filter(|symbol| {
                        self.store.symbols.contains(*symbol)
                            && self.store.symbols.get(*symbol).kind == SymbolKind::Object
                    })
                    .ok_or_else(|| {
                        self.tuple_pattern_resolution_error(
                            pattern,
                            arity,
                            TuplePatternResolutionIssue::CompanionNotFound,
                        )
                    })?
            }
        };
        Ok(CanonicalTupleSymbols {
            class: tuple_class,
            companion,
        })
    }

    pub(super) fn construct_canonical_tuple_value(
        &mut self,
        source_tree: TreeId<Untyped>,
        component_types: &[TypeId],
        component_values: Vec<TreeId<Typed>>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(TreeId<Typed>, TypeId), TyperError> {
        let arity = component_types.len();
        if !(2..=MAX_CANONICAL_TUPLE_ARITY).contains(&arity) || component_values.len() != arity {
            return Err(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: source_tree.index(),
                kind: "pattern definition aggregate tuple arity",
            });
        }
        let symbols = self.resolve_canonical_tuple_symbols(source_tree, arity)?;
        let tuple_prefix = self.type_symbol_prefix(symbols.class);
        let tuple_constructor = self
            .store
            .types
            .alloc(Type::type_ref(tuple_prefix, symbols.class));
        let tuple_type = self.store.types.alloc(Type::Applied {
            tycon: tuple_constructor,
            args: component_types.to_vec(),
        });
        let companion_prefix = self.type_symbol_prefix(symbols.companion);
        let companion_type = self.store.types.alloc(Type::TermRef {
            prefix: companion_prefix,
            target: TermRefTarget::Symbol(symbols.companion),
        });
        let receiver =
            self.tuple_companion_receiver(companion_type, source_tree, arity, info_journal)?;
        let apply_name = Name::new(self.store.names.intern("apply"), Namespace::Term);
        let members = self
            .lookup_overload_members_journaled(receiver, apply_name, info_journal)
            .map_err(|_| TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: source_tree.index(),
                kind: "pattern definition aggregate tuple constructor overload",
            })?;
        let mut candidates = members
            .into_iter()
            .map(|member| {
                let callable = self.member_type_on_journaled(&member, info_journal)?;
                Ok(crate::typer::application::ApplicationCandidate {
                    symbol: member.symbol,
                    callable,
                    member: Some(member),
                    rejection: None,
                })
            })
            .collect::<Result<Vec<_>, TyperError>>()?;
        self.remove_overridden_overload_candidates(&mut candidates, source_tree.index())?;
        let [candidate] = candidates.as_slice() else {
            return Err(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: source_tree.index(),
                kind: "pattern definition aggregate tuple constructor selection",
            });
        };
        let Some(Type::Poly(poly)) = self.store.types.try_get(candidate.callable) else {
            return Err(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: source_tree.index(),
                kind: "pattern definition aggregate tuple constructor type parameters",
            });
        };
        if poly.params.len() != arity {
            return Err(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: source_tree.index(),
                kind: "pattern definition aggregate tuple constructor",
            });
        }
        let instantiated =
            dotty_core::types::instantiate_poly(self.store, candidate.callable, component_types)
                .map_err(|_| TyperError::LocalBlockDeclarationDeferred {
                    source: self.source,
                    tree_index: source_tree.index(),
                    kind: "pattern definition aggregate tuple constructor instantiation",
                })?;
        for (index, (argument, bounds)) in component_types
            .iter()
            .copied()
            .zip(instantiated.bounds.iter().copied())
            .enumerate()
        {
            self.check_explicit_type_argument_bounds(
                argument,
                bounds,
                index,
                source_tree.index(),
                info_journal,
            )?;
        }
        let Some(Type::Method(method)) = self.store.types.try_get(instantiated.result) else {
            return Err(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: source_tree.index(),
                kind: "pattern definition aggregate tuple constructor method type",
            });
        };
        if method.kind != MethodKind::Plain
            || method.params.iter().any(|parameter| {
                parameter.erased
                    || parameter.varargs
                    || matches!(
                        self.store.types.try_get(parameter.ty),
                        Some(Type::ByName { .. } | Type::Repeated { .. })
                    )
            })
            || matches!(
                self.store.types.try_get(method.result),
                Some(Type::Method(_) | Type::Poly(_))
            )
        {
            return Err(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: source_tree.index(),
                kind: "pattern definition aggregate tuple constructor method shape",
            });
        }
        let result_matches_tuple = match self.store.types.try_get(method.result) {
            Some(Type::Applied { tycon, args }) if args == component_types => {
                matches!(
                    self.store.types.try_get(*tycon),
                    Some(Type::TypeRef {
                        target: TypeRefTarget::Symbol(symbol),
                        ..
                    }) if *symbol == symbols.class
                )
            }
            _ => false,
        };
        if method.params.len() != arity
            || method
                .params
                .iter()
                .zip(component_types)
                .any(|(parameter, component)| parameter.ty != *component)
            || !result_matches_tuple
        {
            return Err(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: source_tree.index(),
                kind: "pattern definition aggregate tuple constructor signature",
            });
        }
        let function_type = self.store.types.alloc(Type::TermRef {
            prefix: companion_type,
            target: TermRefTarget::Symbol(candidate.symbol),
        });
        let position = self
            .arena
            .try_get(source_tree)
            .and_then(|tree| tree.position);
        let function = {
            let mut builder = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types);
            let qualifier = builder.ident(
                self.store.symbols.get(symbols.companion).name,
                companion_type,
                position,
            );
            let selected = builder.select(qualifier, apply_name, false, function_type, position);
            let arguments = component_types
                .iter()
                .map(|component| builder.type_tree(*component, position))
                .collect();
            builder.type_apply(selected, arguments, instantiated.result, position)
        };
        let tuple_value = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).apply(
            function,
            component_values,
            tuple_type,
            position,
        );
        Ok((tuple_value, tuple_type))
    }

    pub(super) fn select_canonical_tuple_component(
        &mut self,
        source_tree: TreeId<Untyped>,
        tuple_value: TreeId<Typed>,
        tuple_type: TypeId,
        component_index: usize,
        expected_type: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let spelling = format!("_{}", component_index + 1);
        let name = Name::new(self.store.names.intern(&spelling), Namespace::Term);
        let members = self
            .lookup_overload_members_journaled(tuple_type, name, info_journal)
            .map_err(|_| TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: source_tree.index(),
                kind: "pattern definition aggregate tuple selector",
            })?;
        let mut candidates = members
            .into_iter()
            .map(|member| {
                let callable = self.member_type_on_journaled(&member, info_journal)?;
                Ok(crate::typer::application::ApplicationCandidate {
                    symbol: member.symbol,
                    callable,
                    member: Some(member),
                    rejection: None,
                })
            })
            .collect::<Result<Vec<_>, TyperError>>()?;
        self.remove_overridden_overload_candidates(&mut candidates, source_tree.index())?;
        let [candidate] = candidates.as_slice() else {
            return Err(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: source_tree.index(),
                kind: "pattern definition aggregate tuple selector",
            });
        };
        let member_conforms = self.conforms(candidate.callable, expected_type);
        let expected_conforms = self.conforms(expected_type, candidate.callable);
        if !matches!(member_conforms, Ok(true)) || !matches!(expected_conforms, Ok(true)) {
            return Err(TyperError::LocalBlockDeclarationDeferred {
                source: self.source,
                tree_index: source_tree.index(),
                kind: "pattern definition aggregate tuple selector type",
            });
        }
        let selected_type = self.store.types.alloc(Type::TermRef {
            prefix: self.typed_arena.get(tuple_value).ty,
            target: TermRefTarget::Symbol(candidate.symbol),
        });
        let position = self
            .arena
            .try_get(source_tree)
            .and_then(|tree| tree.position);
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).select(
                tuple_value,
                name,
                false,
                selected_type,
                position,
            ),
        )
    }

    fn type_tuple_pattern(
        &mut self,
        pattern: TreeId<Untyped>,
        tuple: &dotty_core::ast::Tuple,
        selector_type: TypeId,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let arity = tuple.elements.len();
        if arity == 0 {
            let literal = self.type_literal_expression(
                pattern,
                dotty_core::ast::Literal {
                    value: dotty_core::Constant::Unit,
                },
                self.arena.get(pattern).position,
            )?;
            let literal_type = self.typed_arena.get(literal).ty;
            let widened_type =
                self.widen_expression_type_journaled(literal_type, info_journal, 0)?;
            self.require_literal_pattern_compatible(
                literal_type,
                widened_type,
                selector_type,
                pattern.index(),
            )?;
            return Ok(literal);
        }

        let tree_index = pattern.index();
        let tuple_symbols = self.resolve_canonical_tuple_symbols(pattern, arity)?;
        let tuple_class = tuple_symbols.class;
        let tuple_object = tuple_symbols.companion;

        let type_arguments = match self.store.types.try_get(selector_type) {
            Some(Type::Applied { tycon, args }) => {
                let tycon_symbol = match self.store.types.try_get(*tycon) {
                    Some(Type::TypeRef {
                        target: TypeRefTarget::Symbol(symbol),
                        ..
                    }) => Some(*symbol),
                    _ => None,
                };
                if tycon_symbol == Some(tuple_class) && args.len() == arity {
                    args.clone()
                } else {
                    vec![self.definitions.any_type; arity]
                }
            }
            _ => vec![self.definitions.any_type; arity],
        };

        let companion_prefix = self.type_symbol_prefix(tuple_object);
        let companion_type = self.store.types.alloc(Type::TermRef {
            prefix: companion_prefix,
            target: TermRefTarget::Symbol(tuple_object),
        });
        let unapply_name = Name::new(self.store.names.intern("unapply"), Namespace::Term);
        let receiver =
            self.tuple_companion_receiver(companion_type, pattern, arity, info_journal)?;
        self.complete_relation_type(
            receiver,
            info_journal,
            &mut std::collections::HashSet::new(),
            0,
        )?;
        let members = self
            .lookup_overload_members_journaled(receiver, unapply_name, info_journal)
            .map_err(|_| {
                self.tuple_pattern_resolution_error(
                    pattern,
                    arity,
                    TuplePatternResolutionIssue::UnapplyNotFound,
                )
            })?;
        let mut candidates = members
            .into_iter()
            .map(|member| {
                let callable = self.member_type_on_journaled(&member, info_journal)?;
                Ok(crate::typer::application::ApplicationCandidate {
                    symbol: member.symbol,
                    callable,
                    member: Some(member),
                    rejection: None,
                })
            })
            .collect::<Result<Vec<_>, TyperError>>()?;
        self.remove_overridden_overload_candidates(&mut candidates, tree_index)?;
        let [candidate] = candidates.as_slice() else {
            return Err(self.tuple_pattern_resolution_error(
                pattern,
                arity,
                TuplePatternResolutionIssue::UnapplyNotFound,
            ));
        };
        let callable = candidate.callable;
        let Some(Type::Poly(poly)) = self.store.types.try_get(callable) else {
            return Err(self.tuple_pattern_resolution_error(
                pattern,
                arity,
                TuplePatternResolutionIssue::UnapplyShapeUnsupported,
            ));
        };
        if poly.params.len() != arity {
            return Err(self.tuple_pattern_resolution_error(
                pattern,
                arity,
                TuplePatternResolutionIssue::UnapplyShapeUnsupported,
            ));
        }
        let instantiated =
            dotty_core::types::instantiate_poly(self.store, callable, &type_arguments).map_err(
                |_| {
                    self.tuple_pattern_resolution_error(
                        pattern,
                        arity,
                        TuplePatternResolutionIssue::UnapplyShapeUnsupported,
                    )
                },
            )?;
        let Some(Type::Method(method)) = self.store.types.try_get(instantiated.result) else {
            return Err(self.tuple_pattern_resolution_error(
                pattern,
                arity,
                TuplePatternResolutionIssue::UnapplyShapeUnsupported,
            ));
        };
        if self.extractor_method_shape_issue(method).is_some() {
            return Err(self.tuple_pattern_resolution_error(
                pattern,
                arity,
                TuplePatternResolutionIssue::UnapplyShapeUnsupported,
            ));
        }
        let result_type = method.result;
        let tuple_type = method.params[0].ty;
        let selector_conforms = self.conforms(selector_type, tuple_type);
        if matches!(&selector_conforms, Ok(true)) {
            // The selector already has a compatible tuple input type.
        } else {
            let tuple_conforms = self.conforms(tuple_type, selector_type);
            if matches!(&tuple_conforms, Ok(true)) {
                // A broad selector such as Any can still match this tuple.
            } else if let Err(error) = selector_conforms {
                return Err(TyperError::TuplePatternRelationDeferred {
                    source: self.source,
                    tree_index,
                    selector: selector_type,
                    tuple_type,
                    error: Box::new(error),
                });
            } else if let Err(error) = tuple_conforms {
                return Err(TyperError::TuplePatternRelationDeferred {
                    source: self.source,
                    tree_index,
                    selector: selector_type,
                    tuple_type,
                    error: Box::new(error),
                });
            } else {
                return Err(TyperError::TuplePatternTypeMismatch {
                    source: self.source,
                    tree_index,
                    selector: selector_type,
                    tuple_type,
                });
            }
        }
        let component_types = self
            .product_component_types(
                result_type,
                tree_index,
                candidate.symbol,
                arity,
                info_journal,
            )
            .map_err(|_| {
                self.tuple_pattern_resolution_error(
                    pattern,
                    arity,
                    TuplePatternResolutionIssue::TupleTypeUnsupported,
                )
            })?;
        let unapply_type = self.store.types.alloc(Type::TermRef {
            prefix: companion_type,
            target: TermRefTarget::Symbol(candidate.symbol),
        });
        let position = self.arena.get(pattern).position;
        let function = {
            let mut builder = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types);
            let qualifier = builder.ident(
                self.store.symbols.get(tuple_object).name,
                companion_type,
                position,
            );
            let selected = builder.select(qualifier, unapply_name, false, unapply_type, position);
            let arguments = type_arguments
                .iter()
                .map(|argument| builder.type_tree(*argument, position))
                .collect();
            builder.type_apply(selected, arguments, instantiated.result, position)
        };
        let mut patterns = Vec::with_capacity(arity);
        for (element, component_type) in tuple.elements.iter().zip(component_types) {
            patterns.push(self.type_pattern(
                *element,
                component_type,
                context,
                info_journal,
                new_mappings,
            )?);
        }
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).unapply(
                function,
                Vec::new(),
                patterns,
                tuple_type,
                position,
            ),
        )
    }

    fn is_product_subtype(
        &mut self,
        result_type: TypeId,
        pattern_index: u32,
        unapply: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<bool, TyperError> {
        let product_type = match self.canonical_scala_product_type(pattern_index) {
            Ok(Some(product_type)) => product_type,
            Ok(None) => return Ok(false),
            Err(TyperError::TypeNameNotFound { name, .. })
                if self.store.names.resolve(name.text()) == "Product" =>
            {
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        let result_tycon = match self.store.types.try_get(result_type) {
            Some(Type::Applied { tycon, .. }) => *tycon,
            _ => result_type,
        };
        self.complete_typed_pattern_relation_class(result_tycon, info_journal)?;
        self.complete_typed_pattern_relation_class(product_type, info_journal)?;
        let conforms = self.conforms(result_type, product_type).map_err(|_| {
            TyperError::UnsupportedExtractorProductProtocol {
                source: self.source,
                tree_index: pattern_index,
                unapply,
                result: result_type,
                issue: ExtractorProductIssue::ProductRelationUnsupported,
            }
        })?;
        Ok(conforms)
    }

    fn product_component_types(
        &mut self,
        result_type: TypeId,
        pattern_index: u32,
        unapply: SymbolId,
        expected_arity: usize,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Vec<TypeId>, TyperError> {
        let direct_product = self.product_extractor_component_types(
            result_type,
            pattern_index,
            unapply,
            expected_arity,
            true,
            info_journal,
        );
        let direct_arity_error = match direct_product {
            Ok(Some(component_types)) => return Ok(component_types),
            Ok(None) => None,
            Err(error @ TyperError::ExtractorProductSelectorCountMismatch { .. }) => Some(error),
            Err(error) => return Err(error),
        };

        let get_type = match self.option_like_extractor_component_type(
            result_type,
            pattern_index,
            unapply,
            info_journal,
        ) {
            Ok(get_type) => get_type,
            Err(error) => return Err(direct_arity_error.unwrap_or(error)),
        };
        self.product_extractor_component_types(
            get_type,
            pattern_index,
            unapply,
            expected_arity,
            false,
            info_journal,
        )?
        .ok_or(TyperError::UnsupportedExtractorProductProtocol {
            source: self.source,
            tree_index: pattern_index,
            unapply,
            result: get_type,
            issue: ExtractorProductIssue::SelectorShape,
        })
    }

    fn unary_extractor_component_type(
        &mut self,
        result_type: TypeId,
        pattern_index: u32,
        unapply: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let direct_product = self.product_extractor_component_types(
            result_type,
            pattern_index,
            unapply,
            1,
            true,
            info_journal,
        );
        let direct_arity_error = match direct_product {
            Ok(Some(mut component_types)) => return Ok(component_types.remove(0)),
            Ok(None) => None,
            Err(error @ TyperError::ExtractorProductSelectorCountMismatch { .. }) => Some(error),
            Err(error) => return Err(error),
        };

        match self.option_like_extractor_component_type(
            result_type,
            pattern_index,
            unapply,
            info_journal,
        ) {
            Ok(component_type) => Ok(component_type),
            Err(error) => Err(direct_arity_error.unwrap_or(error)),
        }
    }

    fn resolve_extractor_pattern_plan(
        &mut self,
        pattern: TreeId<Untyped>,
        application: &dotty_core::ast::Apply<Untyped>,
        selector_type: TypeId,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<ExtractorPlan, TyperError> {
        let Some(function_tree) = self.arena.try_get(application.function) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: application.function.index(),
            });
        };
        let (qualifier_name, selected_qualifier) = match &function_tree.kind {
            TreeKind::Ident(qualifier) if qualifier.name.is_term() => (qualifier.name, false),
            TreeKind::Select(selection) if selection.name.is_term() => (selection.name, true),
            _ => {
                return Err(TyperError::ExtractorQualifierShapeUnsupported {
                    source: self.source,
                    tree_index: pattern.index(),
                });
            }
        };
        if application.kind != dotty_core::ast::ApplyKind::Regular {
            return Err(TyperError::ExtractorQualifierShapeUnsupported {
                source: self.source,
                tree_index: pattern.index(),
            });
        }
        for argument in &application.args {
            let Some(argument_tree) = self.arena.try_get(*argument) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: argument.index(),
                });
            };
            if matches!(&argument_tree.kind, TreeKind::NamedArg(_)) {
                return Err(TyperError::ExtractorPatternArgumentUnsupported {
                    source: self.source,
                    tree_index: pattern.index(),
                    argument_tree_index: argument.index(),
                    issue: ExtractorPatternArgumentIssue::Named,
                });
            }
            if self.is_sequence_wildcard_pattern(*argument) {
                return Err(TyperError::ExtractorPatternArgumentUnsupported {
                    source: self.source,
                    tree_index: pattern.index(),
                    argument_tree_index: argument.index(),
                    issue: ExtractorPatternArgumentIssue::SequenceWildcard,
                });
            }
        }

        let typed_qualifier = match self.type_expression_inner(
            application.function,
            context,
            info_journal,
            new_mappings,
        ) {
            Ok(typed) => typed,
            Err(TyperError::TermNameNotFound { .. }) if !selected_qualifier => {
                return Err(TyperError::ExtractorQualifierNotFound {
                    source: self.source,
                    tree_index: pattern.index(),
                    name: qualifier_name,
                });
            }
            Err(
                TyperError::OverloadedReferenceDeferred { .. }
                | TyperError::UnsupportedTermReference { .. }
                | TyperError::ObjectTermReferenceDeferred { .. },
            ) => {
                return Err(TyperError::ExtractorQualifierNotValueLike {
                    source: self.source,
                    tree_index: pattern.index(),
                    qualifier_type: None,
                });
            }
            Err(TyperError::MemberNotFound { .. }) if selected_qualifier => {
                return Err(TyperError::ExtractorQualifierMemberNotFound {
                    source: self.source,
                    tree_index: pattern.index(),
                    name: qualifier_name,
                });
            }
            Err(TyperError::OverloadedSelectionDeferred { .. }) if selected_qualifier => {
                return Err(TyperError::ExtractorQualifierMemberAmbiguous {
                    source: self.source,
                    tree_index: pattern.index(),
                    name: qualifier_name,
                });
            }
            Err(TyperError::AmbiguousTermReference { .. }) if selected_qualifier => {
                return Err(TyperError::ExtractorQualifierMemberAmbiguous {
                    source: self.source,
                    tree_index: pattern.index(),
                    name: qualifier_name,
                });
            }
            Err(TyperError::UnstableSelectionPrefix { qualifier_type, .. })
                if selected_qualifier =>
            {
                return Err(TyperError::ExtractorQualifierNotStable {
                    source: self.source,
                    tree_index: pattern.index(),
                    qualifier_type,
                });
            }
            Err(error) => return Err(error),
        };
        let qualifier_type = self.typed_arena.get(typed_qualifier).ty;
        let extractor = match self.store.types.try_get(qualifier_type) {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) if self.store.symbols.contains(*symbol) => *symbol,
            _ => {
                return Err(TyperError::ExtractorQualifierNotValueLike {
                    source: self.source,
                    tree_index: pattern.index(),
                    qualifier_type: Some(qualifier_type),
                });
            }
        };
        if self.store.symbols.get(extractor).kind == SymbolKind::Package {
            return Err(TyperError::ExtractorQualifierNotValueLike {
                source: self.source,
                tree_index: pattern.index(),
                qualifier_type: Some(qualifier_type),
            });
        }
        if self
            .require_stable_selection_prefix(qualifier_type, pattern.index())
            .is_err()
        {
            return Err(TyperError::ExtractorQualifierNotStable {
                source: self.source,
                tree_index: pattern.index(),
                qualifier_type,
            });
        }
        let receiver = self.widen_expression_type_journaled(qualifier_type, info_journal, 0)?;
        let receiver = self.this_type_receiver_view(receiver)?;
        self.complete_relation_type(
            receiver,
            info_journal,
            &mut std::collections::HashSet::new(),
            0,
        )?;
        let unapply_name = Name::new(self.store.names.intern("unapply"), Namespace::Term);
        let members = self
            .lookup_overload_members_journaled(receiver, unapply_name, info_journal)
            .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
        let mut candidates = members
            .into_iter()
            .map(|member| {
                let callable = self.member_type_on_journaled(&member, info_journal)?;
                self.complete_relation_type(
                    callable,
                    info_journal,
                    &mut std::collections::HashSet::new(),
                    0,
                )?;
                Ok(crate::typer::application::ApplicationCandidate {
                    symbol: member.symbol,
                    callable,
                    member: Some(member),
                    rejection: None,
                })
            })
            .collect::<Result<Vec<_>, TyperError>>()?;
        self.remove_overridden_overload_candidates(&mut candidates, pattern.index())?;
        let candidate = match candidates.as_slice() {
            [] => {
                return Err(TyperError::ExtractorUnapplyNotFound {
                    source: self.source,
                    tree_index: pattern.index(),
                    extractor,
                });
            }
            [candidate] => *candidate,
            _ => {
                return Err(TyperError::ExtractorUnapplyOverloaded {
                    source: self.source,
                    tree_index: pattern.index(),
                    extractor,
                    candidates: candidates
                        .iter()
                        .map(|candidate| candidate.symbol)
                        .collect(),
                });
            }
        };
        let unapply = candidate.symbol;
        let callable = candidate.callable;
        let mut unapply_arguments = None;
        let (method, instantiated_callable) = match self.store.types.try_get(callable).cloned() {
            Some(Type::Poly(_)) => {
                let Some((arguments, instantiated)) = self
                    .instantiate_source_case_class_either_unapply(
                        unapply,
                        callable,
                        selector_type,
                        pattern.index(),
                        info_journal,
                    )?
                else {
                    return Err(TyperError::ExtractorUnapplyPolymorphic {
                        source: self.source,
                        tree_index: pattern.index(),
                        unapply,
                        callable,
                    });
                };
                unapply_arguments = Some(arguments);
                let Some(Type::Method(method)) = self.store.types.try_get(instantiated) else {
                    return Err(TyperError::ExtractorUnapplyShapeUnsupported {
                        source: self.source,
                        tree_index: pattern.index(),
                        unapply,
                        callable: instantiated,
                        issue: ExtractorMethodShapeIssue::NotMethod,
                    });
                };
                (method.clone(), instantiated)
            }
            Some(Type::Method(method)) => (method, callable),
            _ => {
                return Err(TyperError::ExtractorUnapplyShapeUnsupported {
                    source: self.source,
                    tree_index: pattern.index(),
                    unapply,
                    callable,
                    issue: ExtractorMethodShapeIssue::NotMethod,
                });
            }
        };
        if let Some(issue) = self.extractor_method_shape_issue(&method) {
            return Err(TyperError::ExtractorUnapplyShapeUnsupported {
                source: self.source,
                tree_index: pattern.index(),
                unapply,
                callable: instantiated_callable,
                issue,
            });
        }
        let input_type = method.params[0].ty;
        let result_type = method.result;
        if matches!(
            self.store.types.try_get(result_type),
            None | Some(Type::NoType | Type::Error(_) | Type::Method(_) | Type::Poly(_))
        ) {
            return Err(TyperError::UnsupportedExtractorResultProtocol {
                source: self.source,
                tree_index: pattern.index(),
                unapply,
                result: result_type,
            });
        }
        let generic_reverse_applicability = unapply_arguments.is_some();
        let conforms = if generic_reverse_applicability {
            // The generic case-class helper authenticated the exact applied
            // Either parent after substituting its ordered class parameters.
            Ok(true)
        } else {
            self.extractor_selector_conforms_to_input(selector_type, input_type, info_journal)
        };
        let unapply_type = match conforms {
            Ok(true) if generic_reverse_applicability => input_type,
            Ok(true) if selector_type == self.definitions.nothing_type => input_type,
            Ok(true) => selector_type,
            Ok(false) => {
                match self.extractor_input_narrows_selector(input_type, selector_type, info_journal)
                {
                    Ok(true) => input_type,
                    Ok(false) => {
                        return Err(TyperError::ExtractorPatternConstraintDeferred {
                            source: self.source,
                            tree_index: pattern.index(),
                            selector: selector_type,
                            input: input_type,
                            error: None,
                        });
                    }
                    Err(error) => {
                        return Err(TyperError::ExtractorPatternConstraintDeferred {
                            source: self.source,
                            tree_index: pattern.index(),
                            selector: selector_type,
                            input: input_type,
                            error: Some(Box::new(error)),
                        });
                    }
                }
            }
            Err(error) => {
                return Err(TyperError::ExtractorPatternConstraintDeferred {
                    source: self.source,
                    tree_index: pattern.index(),
                    selector: selector_type,
                    input: input_type,
                    error: Some(Box::new(error)),
                });
            }
        };
        let function_type = self.store.types.alloc(Type::TermRef {
            prefix: qualifier_type,
            target: TermRefTarget::Symbol(unapply),
        });
        let function = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).select(
            typed_qualifier,
            unapply_name,
            false,
            function_type,
            function_tree.position,
        );
        let function = if let Some(arguments) = unapply_arguments {
            let type_arguments = arguments
                .iter()
                .map(|argument| {
                    TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                        .type_tree(*argument, function_tree.position)
                })
                .collect();
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).type_apply(
                function,
                type_arguments,
                instantiated_callable,
                function_tree.position,
            )
        } else {
            function
        };
        Ok(ExtractorPlan {
            function,
            symbol: unapply,
            input_type,
            result_type,
            unapply_type,
            source_patterns: application.args.clone(),
        })
    }

    /// Instantiate only an authenticated source case-class `unapply` whose
    /// class declares a canonical, two-parameter `Either` parent. The parent
    /// arguments determine the positional class arguments; names are never
    /// used to identify `Left` or `Right`.
    fn instantiate_source_case_class_either_unapply(
        &mut self,
        unapply: SymbolId,
        callable: TypeId,
        selector_type: TypeId,
        pattern_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<(Vec<TypeId>, TypeId)>, TyperError> {
        let selector_type = self.this_type_receiver_view(selector_type)?;
        let Some(identity) = self.index.generated_case_class_method_identity(unapply) else {
            return Ok(None);
        };
        if identity.role != dotty_core::GeneratedCaseClassMethodRole::Unapply {
            return Ok(None);
        }
        let Some(Type::Poly(poly)) = self.store.types.try_get(callable).cloned() else {
            return Ok(None);
        };
        let Some(SourceDefinition::Canonical { source, tree }) =
            self.index.definition_of(identity.class)
        else {
            return Ok(None);
        };
        if source != self.source
            || self.index.symbol_at(source, tree) != Some(identity.class)
            || self.generated_case_class_for_unapply(unapply, source, tree) != Some(identity.class)
        {
            return Ok(None);
        }
        let class_parameters = self.source_class_type_parameter_metadata(identity.class)?;
        if class_parameters.len() != 2
            || poly.params.len() != 2
            || class_parameters
                .iter()
                .any(|parameter| parameter.variance != Some(dotty_core::Variance::Covariant))
        {
            return Ok(None);
        }
        let Some(Type::Applied {
            tycon: selector_tycon,
            args: selector_args,
        }) = self.store.types.try_get(selector_type).cloned()
        else {
            return Ok(None);
        };
        let parent_tree = {
            let Some(TreeKind::TypeDef(definition)) =
                self.arena.try_get(tree).map(|node| &node.kind)
            else {
                return Ok(None);
            };
            let Some(TreeKind::Template(template)) =
                self.arena.try_get(definition.rhs).map(|node| &node.kind)
            else {
                return Ok(None);
            };
            let [parent_tree] = template.parents.as_slice() else {
                return Ok(None);
            };
            *parent_tree
        };
        let Some(context) = self.index.declaration_context_of(identity.class) else {
            return Ok(None);
        };
        let parent_type = self.project_parent_type(parent_tree, context, 0, info_journal)?;
        let Some(Type::Applied {
            tycon: parent_tycon,
            args: parent_args,
        }) = self.store.types.try_get(parent_type).cloned()
        else {
            return Ok(None);
        };
        let symbol_of = |store: &SemanticStore, ty: TypeId| match store.types.try_get(ty) {
            Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            }) => Some(*symbol),
            _ => None,
        };
        let (Some(parent_symbol), Some(selector_symbol)) = (
            symbol_of(self.store, parent_tycon),
            symbol_of(self.store, selector_tycon),
        ) else {
            return Ok(None);
        };
        if parent_symbol != selector_symbol
            || selector_args.len() != 2
            || parent_args.len() != 2
            || self.source_case_class_either_parent(identity.class, parent_type, info_journal)?
                != Some(parent_symbol)
        {
            return Ok(None);
        }
        let parent_parameters = self.source_class_type_parameter_metadata(parent_symbol)?;
        if parent_parameters.len() != 2
            || parent_parameters
                .iter()
                .any(|parameter| parameter.variance != Some(dotty_core::Variance::Covariant))
        {
            return Ok(None);
        }
        let mut class_arguments = vec![None; 2];
        for (parent_argument, selector_argument) in parent_args.iter().zip(selector_args) {
            let Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(parameter),
                ..
            }) = self.store.types.try_get(*parent_argument)
            else {
                return Ok(None);
            };
            let Some(index) = class_parameters
                .iter()
                .position(|candidate| candidate.symbol == *parameter)
            else {
                return Ok(None);
            };
            if class_arguments[index].replace(selector_argument).is_some() {
                return Ok(None);
            }
        }
        let Some(arguments) = class_arguments.into_iter().collect::<Option<Vec<_>>>() else {
            return Ok(None);
        };
        let instantiated = dotty_core::types::instantiate_poly(self.store, callable, &arguments)
            .map_err(TyperError::TypeSubstitution)?;
        for (index, (argument, bounds)) in arguments
            .iter()
            .copied()
            .zip(instantiated.bounds)
            .enumerate()
        {
            self.check_explicit_type_argument_bounds(
                argument,
                bounds,
                index,
                pattern_index,
                info_journal,
            )?;
        }
        Ok(Some((arguments, instantiated.result)))
    }

    /// Allows the bounded reverse relation needed by nominal extractor patterns
    /// only when both the extractor input and selector are direct class or
    /// trait references. Global conformance deliberately remains unchanged;
    /// applied, union, intersection, and other non-nominal shapes stay deferred.
    fn extractor_input_narrows_selector(
        &mut self,
        input_type: TypeId,
        selector_type: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<bool, TypeRelationError> {
        let direct_nominal_symbol = |ty: TypeId| match self.store.types.try_get(ty) {
            Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            }) => Some(*symbol),
            _ => None,
        };
        let (Some(input), Some(selector)) = (
            direct_nominal_symbol(input_type),
            direct_nominal_symbol(selector_type),
        ) else {
            return Err(TypeRelationError::UnsupportedType {
                found: input_type,
                expected: selector_type,
            });
        };
        for symbol in [input, selector] {
            if self.store.symbols.contains(symbol)
                && !matches!(
                    self.store.symbols.get(symbol).kind,
                    SymbolKind::Class | SymbolKind::Trait
                )
            {
                return Err(TypeRelationError::UnsupportedType {
                    found: input_type,
                    expected: selector_type,
                });
            }
        }
        self.complete_typed_pattern_relation_class(input_type, info_journal)
            .map_err(|error| TypeRelationError::ParentTypeAdaptation {
                symbol: input,
                error,
            })?;
        self.complete_typed_pattern_relation_class(selector_type, info_journal)
            .map_err(|error| TypeRelationError::ParentTypeAdaptation {
                symbol: selector,
                error,
            })?;
        self.is_subtype(input_type, selector_type)
    }

    fn extractor_selector_conforms_to_input(
        &mut self,
        selector_type: TypeId,
        input_type: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<bool, TypeRelationError> {
        let relation = self.conforms(selector_type, input_type);
        if !matches!(
            relation,
            Err(TypeRelationError::ClassInfoUnavailable {
                state: crate::types::SymbolInfoState::Missing,
                ..
            })
        ) {
            return relation;
        }
        for ty in [selector_type, input_type] {
            let Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            }) = self.store.types.try_get(ty)
            else {
                return relation;
            };
            let symbol = *symbol;
            if !self.store.symbols.contains(symbol)
                || !matches!(
                    self.store.symbols.get(symbol).kind,
                    SymbolKind::Class | SymbolKind::Trait
                )
            {
                return relation;
            }
            self.complete_typed_pattern_relation_class(ty, info_journal)
                .map_err(|error| TypeRelationError::ParentTypeAdaptation { symbol, error })?;
        }
        self.conforms(selector_type, input_type)
    }

    fn is_sequence_wildcard_pattern(&self, pattern: TreeId<Untyped>) -> bool {
        let Some(tree) = self.arena.try_get(pattern) else {
            return false;
        };
        match &tree.kind {
            TreeKind::Typed(typed) => self.arena.try_get(typed.tpt).is_some_and(|tpt| {
                matches!(
                    &tpt.kind,
                    TreeKind::Ident(ident) if self.store.names.resolve(ident.name.text()) == "_*"
                )
            }),
            TreeKind::Bind(binding) => self.is_sequence_wildcard_pattern(binding.body),
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => {
                self.is_sequence_wildcard_pattern(parens.inner)
            }
            _ => false,
        }
    }

    fn extractor_method_shape_issue(
        &self,
        method: &MethodType,
    ) -> Option<ExtractorMethodShapeIssue> {
        if method.kind != MethodKind::Plain {
            Some(ExtractorMethodShapeIssue::MethodKind)
        } else if method.params.len() != 1 {
            Some(ExtractorMethodShapeIssue::ParameterArity)
        } else if method.params[0].erased {
            Some(ExtractorMethodShapeIssue::ErasedParameter)
        } else if method.params[0].varargs
            || matches!(
                self.store.types.try_get(method.params[0].ty),
                Some(Type::Repeated { .. })
            )
        {
            Some(ExtractorMethodShapeIssue::RepeatedParameter)
        } else if matches!(
            self.store.types.try_get(method.params[0].ty),
            Some(Type::ByName { .. })
        ) {
            Some(ExtractorMethodShapeIssue::ByNameParameter)
        } else if matches!(
            self.store.types.try_get(method.result),
            Some(Type::Method(_) | Type::Poly(_))
        ) {
            Some(ExtractorMethodShapeIssue::TrailingClause)
        } else {
            None
        }
    }

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

    pub(super) fn pattern_binding_term_ref(&mut self, symbol: SymbolId) -> TypeId {
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
            TreeKind::Apply(application) => {
                let plan = self.resolve_extractor_pattern_plan(
                    pattern,
                    application,
                    selector_type,
                    context,
                    info_journal,
                    new_mappings,
                )?;
                let patterns = if plan.result_type == self.definitions.boolean {
                    if !plan.source_patterns.is_empty() {
                        return Err(TyperError::BooleanExtractorPatternArityUnsupported {
                            source: self.source,
                            tree_index: pattern.index(),
                            unapply: plan.symbol,
                            actual: plan.source_patterns.len(),
                        });
                    }
                    Vec::new()
                } else {
                    match plan.source_patterns.as_slice() {
                        [source_pattern] => {
                            let component_type = self.unary_extractor_component_type(
                                plan.result_type,
                                pattern.index(),
                                plan.symbol,
                                info_journal,
                            )?;
                            vec![self.type_pattern(
                                *source_pattern,
                                component_type,
                                context,
                                info_journal,
                                new_mappings,
                            )?]
                        }
                        _ if !plan.source_patterns.is_empty() => {
                            let expected_arity = plan.source_patterns.len();
                            let component_types = match self.product_component_types(
                                plan.result_type,
                                pattern.index(),
                                plan.symbol,
                                expected_arity,
                                info_journal,
                            ) {
                                Ok(component_types) => component_types,
                                Err(
                                    error @ TyperError::UnsupportedExtractorProductProtocol {
                                        issue: ExtractorProductIssue::ProductRelationUnsupported,
                                        ..
                                    },
                                ) => return Err(error),
                                Err(TyperError::UnsupportedExtractorProductProtocol { .. }) => {
                                    return Err(TyperError::ExtractorPatternArityUnsupported {
                                        source: self.source,
                                        tree_index: pattern.index(),
                                        unapply: plan.symbol,
                                        actual: expected_arity,
                                    });
                                }
                                Err(error) => return Err(error),
                            };
                            let mut patterns = Vec::with_capacity(expected_arity);
                            for (source_pattern, component_type) in
                                plan.source_patterns.iter().zip(component_types)
                            {
                                patterns.push(self.type_pattern(
                                    *source_pattern,
                                    component_type,
                                    context,
                                    info_journal,
                                    new_mappings,
                                )?);
                            }
                            patterns
                        }
                        _ => {
                            return Err(TyperError::ExtractorPatternArityUnsupported {
                                source: self.source,
                                tree_index: pattern.index(),
                                unapply: plan.symbol,
                                actual: plan.source_patterns.len(),
                            });
                        }
                    }
                };
                TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).unapply(
                    plan.function,
                    Vec::new(),
                    patterns,
                    plan.unapply_type,
                    source_tree.position,
                )
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
            TreeKind::Alternative(alternative) => {
                if alternative.alternatives.is_empty() {
                    return Err(TyperError::UnsupportedPattern {
                        source: self.source,
                        tree_index: pattern.index(),
                        pattern_kind: PatternKind::Alternative,
                    });
                }
                let mut typed_alternatives = Vec::with_capacity(alternative.alternatives.len());
                let mut joined_type = None;
                for branch in &alternative.alternatives {
                    let branch = *branch;
                    if let Some(binding) = self.pattern_binding_in(branch) {
                        return Err(TyperError::PatternBindingInAlternative {
                            source: self.source,
                            tree_index: pattern.index(),
                            branch_tree_index: branch.index(),
                            binding_tree_index: binding.index(),
                        });
                    }
                    let typed_branch = self.type_pattern(
                        branch,
                        selector_type,
                        context,
                        info_journal,
                        new_mappings,
                    )?;
                    let branch_type = self.typed_arena.get(typed_branch).ty;
                    let branch_type =
                        self.widen_expression_type_journaled(branch_type, info_journal, 0)?;
                    joined_type = Some(match joined_type {
                        None => branch_type,
                        Some(previous) => self
                            .join_expression_types(previous, branch_type)
                            .map_err(|error| TyperError::PatternAlternativeJoinUnsupported {
                                source: self.source,
                                tree_index: pattern.index(),
                                left: previous,
                                right: branch_type,
                                error: Box::new(error),
                            })?,
                    });
                    typed_alternatives.push(typed_branch);
                }
                let ty = joined_type.expect("a non-empty alternative has a joined type");
                TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).alternative(
                    typed_alternatives,
                    ty,
                    source_tree.position,
                )
            }
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => self.type_pattern(
                parens.inner,
                selector_type,
                context,
                info_journal,
                new_mappings,
            )?,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) => self.type_tuple_pattern(
                pattern,
                tuple,
                selector_type,
                context,
                info_journal,
                new_mappings,
            )?,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)) => {
                return Err(TyperError::InfixPatternDeferred {
                    source: self.source,
                    tree_index: pattern.index(),
                });
            }
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

    /// Finds a binding form before typing an alternative branch. This keeps
    /// rejected bindings out of the case scope and reports their source node.
    fn pattern_binding_in(&self, pattern: TreeId<Untyped>) -> Option<TreeId<Untyped>> {
        let tree = self.arena.try_get(pattern)?;
        match &tree.kind {
            TreeKind::Bind(_) => Some(pattern),
            TreeKind::Ident(ident)
                if !ident.backquoted
                    && ident.name.is_term()
                    && self.store.names.resolve(ident.name.text()) != "_"
                    && is_variable_pattern_name(self.store, ident.name) =>
            {
                Some(pattern)
            }
            TreeKind::Typed(typed) => self.pattern_binding_in(typed.expr),
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => {
                self.pattern_binding_in(parens.inner)
            }
            TreeKind::Apply(application) => application
                .args
                .iter()
                .find_map(|argument| self.pattern_binding_in(*argument)),
            TreeKind::UnApply(extractor) => extractor
                .patterns
                .iter()
                .find_map(|argument| self.pattern_binding_in(*argument)),
            TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) => tuple
                .elements
                .iter()
                .find_map(|element| self.pattern_binding_in(*element)),
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) => self
                .pattern_binding_in(infix.left)
                .or_else(|| self.pattern_binding_in(infix.right)),
            TreeKind::NamedArg(argument) => self.pattern_binding_in(argument.arg),
            TreeKind::Alternative(alternative) => alternative
                .alternatives
                .iter()
                .find_map(|branch| self.pattern_binding_in(*branch)),
            _ => None,
        }
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
            // An explicit alias may wrap a refutable extractor pattern. The
            // recursive pattern typer below remains responsible for deciding
            // whether that extractor shape is supported.
            TreeKind::Apply(_) | TreeKind::TypeApply(_) | TreeKind::UnApply(_) => true,
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
        let pattern_type = self.type_of_tpt_inner_journaled(tpt, type_context, info_journal)?;
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
            prefix,
            target: TypeRefTarget::Symbol(symbol),
        }) = self.store.types.try_get(pattern_type)
        else {
            return Ok(false);
        };
        if !self.typed_pattern_prefix_is_reifiable(*prefix) {
            return Ok(false);
        }
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

    fn typed_pattern_prefix_is_reifiable(&self, mut prefix: TypeId) -> bool {
        for _ in 0..MAX_REIFIABLE_TYPE_PREFIX_DEPTH {
            match self.store.types.try_get(prefix) {
                Some(Type::NoPrefix) => return true,
                Some(Type::TypeRef {
                    prefix: parent,
                    target: TypeRefTarget::Symbol(package),
                }) if self.store.symbols.contains(*package)
                    && self.store.symbols.get(*package).kind == SymbolKind::Package =>
                {
                    prefix = *parent;
                }
                _ => return false,
            }
        }
        false
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
        let mut selector_type = selector_type;
        let mut seen = HashSet::new();
        for _ in 0..crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            if !seen.insert(selector_type) {
                return Err(TyperError::TypeNormalization(
                    crate::types::TypeNormalizeError::NormalizationCycle { ty: selector_type },
                ));
            }
            let Some(selector_node) = self.store.types.try_get(selector_type) else {
                return Err(TyperError::TypeNormalization(
                    if self.store.types.contains(selector_type) {
                        crate::types::TypeNormalizeError::UnfilledType { ty: selector_type }
                    } else {
                        crate::types::TypeNormalizeError::InvalidType { ty: selector_type }
                    },
                ));
            };
            match selector_node {
                Type::Annotated { underlying, .. } | Type::Flexible { underlying } => {
                    selector_type = *underlying;
                }
                _ => break,
            }
        }
        if self
            .store
            .types
            .try_get(selector_type)
            .is_some_and(|selector_node| {
                matches!(
                    selector_node,
                    Type::Annotated { .. } | Type::Flexible { .. }
                )
            })
        {
            return Err(TyperError::TypeNormalization(
                crate::types::TypeNormalizeError::TooDeep,
            ));
        }
        match self.store.types.try_get(selector_type) {
            Some(Type::Constant(_)) => Ok(selector_type),
            _ => self.widen_expression_type_journaled(selector_type, info_journal, 0),
        }
    }
}

/// Mirrors the parser's pattern-variable first-character rule. Backquotes are
/// checked by the caller; literal keywords and the wildcard are never binders.
pub(super) fn is_variable_pattern_name(store: &SemanticStore, name: Name) -> bool {
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
        Definitions, MemberRequest, Name, Namespace, Packages, ResolutionError, ResolverCheckpoint,
        SemanticStore, SourceSemanticIndex, SourceText, Symbol, SymbolFlags, SymbolInfo,
        SymbolKind, SymbolLinks, SymbolOrigin, SymbolResolver, TermName, Visibility,
    };
    use dotty_lexer::ContextualScanner;
    use dotty_namer::name_compilation_unit;
    use std::cell::Cell;
    use std::rc::Rc;

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

    #[test]
    fn extractor_result_protocol_uses_ordinary_selection_overload_discovery() {
        let source_text = "class Parent { def _1: Int = 0 }; class Tuple extends Parent { override def _1: Int = 1 }; class Direct extends Parent { override def _1: Int = 2; def _1(value: Int): Int = value }; class Left { def _1: Int = 0 }; class Right { def _1: Int = 1 }; class Incomparable extends Left with Right";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let class_symbol = |name: &str, store: &SemanticStore| {
            parsed
                .ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::TypeDef(definition)
                        if store.names.resolve(definition.name.as_name().text()) == name =>
                    {
                        index.symbol_at(source, tree)
                    }
                    _ => None,
                })
                .expect("fixture class")
        };
        let parent = class_symbol("Parent", &store);
        let tuple = class_symbol("Tuple", &store);
        let parent_selector = store
            .scopes
            .get(index.scope_of(parent).unwrap())
            .lookup(&Name::new(store.names.intern("_1"), Namespace::Term))
            .unwrap();
        let tuple_selector = store
            .scopes
            .get(index.scope_of(tuple).unwrap())
            .lookup(&Name::new(store.names.intern("_1"), Namespace::Term))
            .unwrap();
        let receiver = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, tuple));
        let selector_name = Name::new(store.names.intern("_1"), Namespace::Term);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let mut ordinary_journal = Vec::new();
        let ordinary = typer
            .lookup_members_journaled(receiver, selector_name, &mut ordinary_journal)
            .unwrap();
        assert_eq!(ordinary.len(), 1);
        assert_eq!(ordinary[0].symbol, tuple_selector);
        assert_eq!(ordinary[0].declaring_class, tuple);
        let ordinary_type = typer
            .member_type_on_journaled(&ordinary[0], &mut ordinary_journal)
            .unwrap();
        assert_eq!(ordinary_type, definitions.int);

        let mut overload_journal = Vec::new();
        let overloads = typer
            .lookup_overload_members_journaled(receiver, selector_name, &mut overload_journal)
            .unwrap();
        assert_eq!(
            overloads
                .iter()
                .map(|candidate| candidate.symbol)
                .collect::<Vec<_>>(),
            [tuple_selector, parent_selector]
        );

        let direct = class_symbol("Direct", typer.store());
        let direct_receiver = typer
            .store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, direct));
        let mut direct_journal = Vec::new();
        let direct_bucket = typer
            .lookup_members_journaled(direct_receiver, selector_name, &mut direct_journal)
            .unwrap();
        assert_eq!(
            direct_bucket.len(),
            2,
            "direct overloads remain in one ambiguous selection bucket"
        );
        assert!(
            direct_bucket
                .iter()
                .all(|candidate| candidate.declaring_class == direct)
        );
        let mut direct_protocol_journal = Vec::new();
        let direct_error = typer
            .extractor_result_member_type(
                direct_receiver,
                selector_name,
                0,
                direct_bucket[0].symbol,
                &mut direct_protocol_journal,
            )
            .unwrap_err();
        assert!(matches!(
            direct_error,
            TyperError::ExtractorResultMemberOverloaded { .. }
        ));

        let incomparable = class_symbol("Incomparable", typer.store());
        let incomparable_receiver = typer
            .store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, incomparable));
        let mut incomparable_journal = Vec::new();
        let inherited = typer
            .lookup_members_journaled(
                incomparable_receiver,
                selector_name,
                &mut incomparable_journal,
            )
            .unwrap();
        assert_eq!(
            inherited.len(),
            2,
            "incomparable inherited buckets remain ambiguous"
        );
        let mut inherited_protocol_journal = Vec::new();
        let inherited_error = typer
            .extractor_result_member_type(
                incomparable_receiver,
                selector_name,
                0,
                inherited[0].symbol,
                &mut inherited_protocol_journal,
            )
            .unwrap_err();
        assert!(matches!(
            inherited_error,
            TyperError::ExtractorResultMemberOverloaded { .. }
        ));
    }

    struct ProductResolver {
        product: dotty_core::SymbolId,
        scala_package: Option<dotty_core::SymbolId>,
        should_fail: Rc<Cell<bool>>,
        should_resolve: Rc<Cell<bool>>,
        should_resolve_package: Rc<Cell<bool>>,
    }

    impl SymbolResolver for ProductResolver {
        fn checkpoint(&self) -> ResolverCheckpoint {
            ResolverCheckpoint::new(0)
        }

        fn rollback_to(&mut self, _store: &mut SemanticStore, _checkpoint: ResolverCheckpoint) {}

        fn resolve_member(
            &mut self,
            store: &mut SemanticStore,
            request: &MemberRequest,
        ) -> Result<Option<dotty_core::SymbolId>, ResolutionError> {
            if self.should_fail.get() {
                return Err(ResolutionError::Malformed {
                    reason: "scripted Product resolution failure".to_owned(),
                });
            }
            if !self.should_resolve.get() {
                return Ok(None);
            }
            Ok((store.names.resolve(request.name.text()) == "Product").then_some(self.product))
        }

        fn resolve_package(
            &mut self,
            _store: &mut SemanticStore,
            path: &[&str],
        ) -> Result<Option<dotty_core::SymbolId>, ResolutionError> {
            Ok((path == ["scala"] && self.should_resolve_package.get())
                .then_some(self.scala_package)
                .flatten())
        }
    }

    #[test]
    fn one_field_source_case_class_projects_its_canonical_product_selector() {
        let source_text = "package matching { case class Box(value: Int); object Matcher { def unwrap(box: Box): Int = box match { case Box(value) => value } } }; package scala { trait Product }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let class = parsed
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
            .expect("Box source class");
        let unapply = index
            .generated_case_class_method(class, dotty_core::GeneratedCaseClassMethodRole::Unapply)
            .expect("generated Box.unapply");
        let selector_name = Name::new(store.names.intern("_1"), Namespace::Term);
        let product_name = dotty_core::TypeName::new(store.names.intern("Product"));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let unapply_info = typer
            .complete_symbol(unapply)
            .expect("complete generated unapply");
        let result = match typer.store().types.try_get(unapply_info) {
            Some(Type::Method(method)) => method.result,
            other => panic!("unexpected generated unapply signature: {other:?}"),
        };
        let class_info = typer.complete_symbol(class).expect("complete Box class");
        let (product, product_edges, parent_count) = match typer.store().types.try_get(class_info) {
            Some(Type::ClassInfo(info)) => {
                let scala = packages.symbol(&["scala"]).expect("scala package");
                let scala_scope = packages.scope_of(scala).expect("scala package scope");
                let product = typer
                    .store()
                    .scopes
                    .get(scala_scope)
                    .lookup(product_name.as_name())
                    .expect("canonical Product");
                let edges = info
                    .parents
                    .iter()
                    .filter(|parent| match typer.store().types.try_get(**parent) {
                        Some(Type::TypeRef {
                            target: TypeRefTarget::Symbol(symbol),
                            ..
                        }) => *symbol == product,
                        _ => false,
                    })
                    .count();
                (product, edges, info.parents.len())
            }
            other => panic!("unexpected Box class info: {other:?}"),
        };
        assert_eq!(
            product_edges, 1,
            "Product appears exactly once in Box parents"
        );
        assert_eq!(parent_count, 2, "Box keeps Object and adds only Product");
        assert_eq!(
            typer.store().symbols.get(product).owner,
            packages.symbol(&["scala"]),
            "selector ancestry uses the canonical scala.Product owner"
        );
        let members = typer
            .lookup_overload_members_journaled(result, selector_name, &mut Vec::new())
            .expect("normal member lookup for Box._1");
        assert_eq!(members.len(), 1);
        let selector = members[0].symbol;
        assert_eq!(typer.store().symbols.get(selector).owner, Some(class));
        let SymbolInfo::Complete(selector_info) = *typer.store().symbols.info(selector) else {
            panic!("class-first completion should publish the generated selector");
        };
        let Some(Type::Method(selector_method)) = typer.store().types.try_get(selector_info) else {
            panic!("expected the generated selector method type");
        };
        assert!(selector_method.params.is_empty());
        assert_eq!(selector_method.result, definitions.int);
        let component = typer
            .run_expression_transaction(|typer, journal, _| {
                typer.unary_extractor_component_type(result, parsed.root.index(), unapply, journal)
            })
            .expect("existing unary product extractor path");
        assert_eq!(component, definitions.int);
        assert_eq!(typer.complete_symbol(class).unwrap(), class_info);
    }

    #[test]
    fn recursive_source_product1_projects_its_canonical_trait_reference() {
        let source_text = "package scala { trait Product }; package matching { sealed trait Expr; final case class Succ(inner: Expr) extends Expr }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let (class, class_tree, unapply) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Succ" =>
                {
                    let class = index.symbol_at(source, tree)?;
                    let unapply = index.generated_case_class_method(
                        class,
                        dotty_core::GeneratedCaseClassMethodRole::Unapply,
                    )?;
                    Some((class, tree, unapply))
                }
                _ => None,
            })
            .expect("source Succ class and generated unapply");
        let expr = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Expr" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .expect("source Expr trait");
        let class_type = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class));
        let expr_type = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, expr));
        let selector_name = Name::new(store.names.intern("_1"), Namespace::Term);
        let second_selector_name = Name::new(store.names.intern("_2"), Namespace::Term);
        let unapply_formal = index
            .derived_symbol_at(unapply, source, class_tree)
            .expect("generated unapply formal retains its distinct source-derived identity");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let class_info = typer
            .complete_symbol(class)
            .expect("complete bounded recursive Product1 class");
        let parents = match typer.store().types.get(class_info) {
            Type::ClassInfo(info) => {
                let parent_symbols = info
                    .parents
                    .iter()
                    .map(|parent| type_ref_symbol(&typer, *parent))
                    .collect::<Vec<_>>();
                assert_eq!(parent_symbols.len(), 3);
                assert_eq!(parent_symbols[0], definitions.object_class);
                assert_eq!(parent_symbols[1], expr);
                let product = parent_symbols[2];
                let product_symbol = typer.store().symbols.get(product);
                assert_eq!(product_symbol.kind, SymbolKind::Trait);
                assert_eq!(
                    typer.store().names.resolve(product_symbol.name.text()),
                    "Product"
                );
                assert_eq!(
                    product_symbol.owner,
                    typer.packages.symbol(&["scala"]),
                    "canonical scala.Product edge"
                );
                parent_symbols
            }
            other => panic!("unexpected class completion: {other:?}"),
        };
        assert_eq!(
            parents.len(),
            3,
            "Object, Expr, and Product are unique edges"
        );
        typer
            .complete_symbol(expr)
            .expect("complete canonical source Expr parent");
        typer
            .complete_symbol(parents[2])
            .expect("complete canonical scala.Product parent");
        assert!(typer.is_subtype(class_type, expr_type).unwrap());
        assert!(!typer.is_subtype(expr_type, class_type).unwrap());
        let selector = typer
            .store()
            .scopes
            .get(index.scope_of(class).unwrap())
            .lookup_all(&selector_name);
        assert_eq!(selector.len(), 1, "exactly one class-owned _1 is present");
        let selector = selector[0];
        assert!(
            typer
                .store()
                .scopes
                .get(index.scope_of(class).unwrap())
                .lookup_all(&second_selector_name)
                .is_empty()
        );
        assert_eq!(typer.store().symbols.get(selector).kind, SymbolKind::Method);
        assert_eq!(typer.store().symbols.get(selector).owner, Some(class));
        let field_tree = match &parsed.ast.get(class_tree).kind {
            TreeKind::TypeDef(definition) => match &parsed.ast.get(definition.rhs).kind {
                TreeKind::Template(template) => match &parsed.ast.get(template.constructor).kind {
                    TreeKind::DefDef(constructor) => constructor.value_param_clauses[0][0],
                    other => panic!("unexpected constructor: {other:?}"),
                },
                other => panic!("unexpected template: {other:?}"),
            },
            other => panic!("unexpected class declaration: {other:?}"),
        };
        let field = index.symbol_at(source, field_tree).unwrap();
        assert_eq!(typer.store().symbols.get(field).kind, SymbolKind::Field);
        assert_eq!(typer.store().symbols.get(field).owner, Some(class));
        assert_ne!(selector, field);
        assert_ne!(selector, unapply_formal);
        assert_ne!(field, unapply_formal);
        let mut journal = Vec::new();
        let projected = typer
            .unary_extractor_component_type(class_type, class_tree.index(), unapply, &mut journal)
            .expect("Product1 projection reads completed Succ._1");
        assert_eq!(type_ref_symbol(&typer, projected), expr);
        let selector_info = typer.complete_symbol(selector).unwrap();
        let Type::Method(selector_method) = typer.store().types.get(selector_info) else {
            panic!("Succ._1 is a parameterless method")
        };
        assert!(selector_method.params.is_empty());
        assert_eq!(type_ref_symbol(&typer, selector_method.result), expr);
    }

    #[test]
    fn recursive_source_product1_rejects_stale_class_info_with_duplicate_product_edge() {
        let source_text = "package scala { trait Product }; package matching { sealed trait Expr; final case class Succ(inner: Expr) extends Expr }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let class = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Succ" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .expect("source Succ class");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let canonical_info = typer
            .complete_symbol(class)
            .expect("valid source Product1 class completes");
        assert_eq!(
            typer
                .complete_symbol(class)
                .expect("unchanged source Product1 class completes repeatedly"),
            canonical_info,
            "revalidation accepts the metadata this typer just published"
        );
        let Type::ClassInfo(mut stale) = typer.store().types.get(canonical_info).clone() else {
            panic!("Succ ClassInfo")
        };
        let product = type_ref_symbol(&typer, stale.parents[2]);
        assert_eq!(
            typer.store().symbols.get(product).origin,
            SymbolOrigin::Source(source),
            "the source-owned scala.Product fixture remains supported"
        );
        assert_eq!(
            stale
                .parents
                .iter()
                .filter(|parent| type_ref_symbol(&typer, **parent) == product)
                .count(),
            1,
            "canonical ClassInfo has exactly one Product edge before corruption"
        );
        stale.parents.push(stale.parents[2]);
        assert_eq!(
            stale
                .parents
                .iter()
                .filter(|parent| type_ref_symbol(&typer, **parent) == product)
                .count(),
            2,
            "the test corruption introduces only a duplicate Product edge"
        );
        let stale_info = typer.store.types.alloc(Type::ClassInfo(stale));
        typer
            .store
            .symbols
            .set_info(class, SymbolInfo::Complete(stale_info));

        assert!(matches!(
            typer.complete_symbol(class),
            Err(TyperError::MalformedClassInfo { symbol, .. }) if symbol == class
        ));
    }

    fn type_ref_symbol(typer: &SourceTyper<'_>, ty: dotty_core::TypeId) -> dotty_core::SymbolId {
        match typer.store.types.get(ty) {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("expected canonical TypeRef, found {other:?}"),
        }
    }

    #[test]
    fn product_selector_completes_first_and_retries_resolver_transactionally() {
        let source_text = "package scala {}; package matching { case class Box(value: Int) }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let class_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Box" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let class = index.symbol_at(source, class_tree).unwrap();
        let TreeKind::TypeDef(class_definition) = &parsed.ast.get(class_tree).kind else {
            panic!("expected source Box declaration");
        };
        let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
            panic!("expected source Box template");
        };
        let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
            panic!("expected Box primary constructor");
        };
        let parameter_tree = constructor.value_param_clauses[0][0];
        let selector = index
            .derived_symbol_at(class, source, parameter_tree)
            .unwrap();
        let scala = packages.symbol(&["scala"]).unwrap();
        let product = store.symbols.alloc(Symbol {
            name: Name::new(store.names.intern("Product"), Namespace::Type),
            owner: Some(scala),
            kind: SymbolKind::Trait,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Builtin,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let should_fail = Rc::new(Cell::new(true));
        let should_resolve = Rc::new(Cell::new(false));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        )
        .with_resolver(Box::new(ProductResolver {
            product,
            scala_package: None,
            should_fail: should_fail.clone(),
            should_resolve: should_resolve.clone(),
            should_resolve_package: Rc::new(Cell::new(false)),
        }));

        let checkpoint = typer.store.checkpoint();
        assert!(matches!(
            typer.complete_symbol(selector),
            Err(TyperError::SymbolResolution { .. })
        ));
        assert_eq!(typer.store.checkpoint(), checkpoint);
        assert_eq!(*typer.store.symbols.info(class), SymbolInfo::Missing);
        assert_eq!(*typer.store.symbols.info(selector), SymbolInfo::Missing);

        should_fail.set(false);
        let checkpoint = typer.store.checkpoint();
        assert!(matches!(
            typer.complete_symbol(selector),
            Err(TyperError::TypeNameNotFound { name, .. })
                if typer.store.names.resolve(name.text()) == "Product"
        ));
        assert_eq!(typer.store.checkpoint(), checkpoint);
        assert_eq!(*typer.store.symbols.info(class), SymbolInfo::Missing);
        assert_eq!(*typer.store.symbols.info(selector), SymbolInfo::Missing);

        should_resolve.set(true);
        let selector_info = typer
            .complete_symbol(selector)
            .expect("selector-first completion retries when Product resolves");
        let Some(Type::Method(method)) = typer.store.types.try_get(selector_info) else {
            panic!("expected a parameterless generated selector");
        };
        assert!(method.params.is_empty());
        assert_eq!(method.result, definitions.int);

        let SymbolInfo::Complete(class_info_id) = *typer.store.symbols.info(class) else {
            panic!("selector-first completion should complete Box");
        };
        let Some(Type::ClassInfo(class_info)) = typer.store.types.try_get(class_info_id) else {
            panic!("expected Box class metadata");
        };
        assert!(class_info.parents.iter().any(|parent| {
            matches!(typer.store.types.try_get(*parent), Some(Type::TypeRef { target: TypeRefTarget::Symbol(found), .. }) if *found == product)
        }));
        assert_eq!(typer.complete_symbol(class).unwrap(), class_info_id);
    }

    #[test]
    fn missing_scala_package_defers_case_class_completion_until_canonical_retry() {
        let source_text = "package matching { case class Box(value: Int); object Matcher { def unwrap(box: Box): Int = box match { case Box(value) => value } } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let class = parsed
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
            .expect("Box source class");
        let unwrap = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "unwrap" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .expect("source unwrap method");
        let unapply = index
            .generated_case_class_method(class, dotty_core::GeneratedCaseClassMethodRole::Unapply)
            .expect("generated Box.unapply");
        let scala = store.symbols.alloc(Symbol {
            name: Name::new(store.names.intern("scala"), Namespace::Term),
            owner: None,
            kind: SymbolKind::Package,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Builtin,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let product = store.symbols.alloc(Symbol {
            name: Name::new(store.names.intern("Product"), Namespace::Type),
            owner: Some(scala),
            kind: SymbolKind::Trait,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Builtin,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let product_declarations = store.scopes.alloc(dotty_core::Scope::new(Some(product)));
        let product_info = store
            .types
            .alloc(Type::ClassInfo(dotty_core::types::ClassInfo {
                prefix: definitions.no_prefix,
                class: product,
                parents: Vec::new(),
                declarations: product_declarations,
                self_type: None,
            }));
        store
            .symbols
            .set_info(product, SymbolInfo::Complete(product_info));
        let should_resolve_package = Rc::new(Cell::new(false));
        let should_resolve_product = Rc::new(Cell::new(false));
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        )
        .with_resolver(Box::new(ProductResolver {
            product,
            scala_package: Some(scala),
            should_fail: Rc::new(Cell::new(false)),
            should_resolve: should_resolve_product.clone(),
            should_resolve_package: should_resolve_package.clone(),
        }));

        let checkpoint = typer.store.checkpoint();
        assert!(matches!(
            typer.complete_symbol(class),
            Err(TyperError::TypeNameNotFound { name, .. })
                if typer.store.names.resolve(name.text()) == "Product"
        ));
        assert_eq!(typer.store.checkpoint(), checkpoint);
        assert_eq!(*typer.store.symbols.info(class), SymbolInfo::Missing);
        let selector = index
            .scope_of(class)
            .and_then(|scope| {
                typer
                    .store
                    .scopes
                    .get(scope)
                    .lookup(&Name::new(typer.store.names.intern("_1"), Namespace::Term))
            })
            .expect("derived Box._1");
        assert_eq!(*typer.store.symbols.info(selector), SymbolInfo::Missing);

        should_resolve_package.set(true);
        should_resolve_product.set(true);
        let class_info_id = typer
            .complete_symbol(class)
            .expect("completion retries after canonical scala becomes available");
        let Some(Type::ClassInfo(class_info)) = typer.store.types.try_get(class_info_id) else {
            panic!("expected Box class info");
        };
        assert!(class_info.parents.iter().any(|parent| {
            matches!(typer.store.types.try_get(*parent), Some(Type::TypeRef { target: TypeRefTarget::Symbol(found), .. }) if *found == product)
        }));
        assert!(matches!(
            *typer.store.symbols.info(selector),
            SymbolInfo::Complete(_)
        ));
        let unapply_info = typer
            .complete_symbol(unapply)
            .expect("generated unapply completes through resolver-only Product");
        let result_type = match typer.store.types.try_get(unapply_info) {
            Some(Type::Method(method)) => method.result,
            other => panic!("expected generated unapply method info, found {other:?}"),
        };
        let product_component = typer
            .run_expression_transaction(|typer, journal, _| {
                typer.unary_extractor_component_type(result_type, unwrap.index(), unapply, journal)
            })
            .expect("resolver-only Product ancestry authorizes unary projection");
        assert_eq!(product_component, definitions.int);
        let typed_unwrap = typer
            .type_method_definition(unwrap)
            .expect("resolver-only Product supports the source extractor method");
        assert_eq!(
            typer.source_typed_index().get(source, unwrap),
            Some(typed_unwrap)
        );
    }

    #[test]
    fn noncanonical_resolved_scala_package_defers_case_class_completion() {
        let source_text = "package matching { case class Box(value: Int) }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let class = parsed
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
            .expect("Box source class");
        let foreign = store.symbols.alloc(Symbol {
            name: Name::new(store.names.intern("foreign"), Namespace::Term),
            owner: None,
            kind: SymbolKind::Package,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Builtin,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let product = store.symbols.alloc(Symbol {
            name: Name::new(store.names.intern("Product"), Namespace::Type),
            owner: Some(foreign),
            kind: SymbolKind::Trait,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Builtin,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        )
        .with_resolver(Box::new(ProductResolver {
            product,
            scala_package: Some(foreign),
            should_fail: Rc::new(Cell::new(false)),
            should_resolve: Rc::new(Cell::new(true)),
            should_resolve_package: Rc::new(Cell::new(true)),
        }));
        let checkpoint = typer.store.checkpoint();
        assert!(matches!(
            typer.complete_symbol(class),
            Err(TyperError::SymbolResolution {
                error: ResolutionError::Malformed { .. },
                ..
            })
        ));
        assert_eq!(typer.store.checkpoint(), checkpoint);
        assert_eq!(*typer.store.symbols.info(class), SymbolInfo::Missing);
        let selector = index
            .scope_of(class)
            .and_then(|scope| {
                typer
                    .store
                    .scopes
                    .get(scope)
                    .lookup(&Name::new(typer.store.names.intern("_1"), Namespace::Term))
            })
            .expect("derived Box._1");
        assert_eq!(*typer.store.symbols.info(selector), SymbolInfo::Missing);
    }

    #[test]
    fn ordinary_and_multiclause_classes_do_not_receive_case_product_projection() {
        let source_text = "package scala { trait Product }; package matching { class Ordinary(val value: Int); case class Pair(left: Int, right: Int); case class Curried(value: Int)(extra: Int); case class Repeated(values: Int*) }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let selector_name = Name::new(store.names.intern("_1"), Namespace::Term);
        let selector2_name = Name::new(store.names.intern("_2"), Namespace::Term);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        for (tree, node) in parsed.ast.iter() {
            let TreeKind::TypeDef(definition) = &node.kind else {
                continue;
            };
            let name = typer
                .store()
                .names
                .resolve(definition.name.as_name().text())
                .to_owned();
            if !matches!(name.as_str(), "Ordinary" | "Pair" | "Curried" | "Repeated") {
                continue;
            }
            let class = index
                .symbol_at(source, tree)
                .expect("source class identity");
            let class_scope = index.scope_of(class).expect("class scope");
            let selector = typer.store().scopes.get(class_scope).lookup(&selector_name);
            if name == "Pair" {
                assert!(selector.is_some(), "structural product-2 Pair gets _1");
                assert!(
                    typer
                        .store()
                        .scopes
                        .get(class_scope)
                        .lookup(&selector2_name)
                        .is_some(),
                    "structural product-2 Pair gets _2"
                );
            } else {
                assert!(
                    selector.is_none(),
                    "{name} must not acquire a synthesized _1"
                );
            }
            if name == "Pair" {
                let error = typer
                    .complete_symbol(class)
                    .expect_err("unsupported Product2 field types remain a typer error");
                assert!(matches!(
                    error,
                    TyperError::UnsupportedProduct2FieldTypes { class: actual, .. }
                        if actual == class
                ));
                assert_eq!(
                    *typer.store().symbols.info(selector.unwrap()),
                    SymbolInfo::Missing
                );
                continue;
            }
            let class_info = typer
                .complete_symbol(class)
                .expect("complete negative control");
            let Type::ClassInfo(info) = typer.store().types.get(class_info) else {
                panic!("class completion must produce ClassInfo");
            };
            assert!(
                !info
                    .parents
                    .iter()
                    .any(|parent| match typer.store().types.try_get(*parent) {
                        Some(Type::TypeRef {
                            target: TypeRefTarget::Symbol(parent),
                            ..
                        }) => {
                            typer
                                .store()
                                .names
                                .resolve(typer.store().symbols.get(*parent).name.text())
                                == "Product"
                        }
                        _ => false,
                    }),
                "{name} must not gain Product ancestry"
            );
        }
    }

    #[test]
    fn malformed_ambiguous_and_foreign_product_identities_defer_transactionally() {
        let source_text =
            "package scala { class Product }; package matching { case class Box(value: Int) }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let class = parsed
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
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let checkpoint = typer.store.checkpoint();
        assert!(matches!(
            typer.complete_symbol(class),
            Err(TyperError::SymbolResolution { .. })
        ));
        assert_eq!(typer.store.checkpoint(), checkpoint);
        assert_eq!(*typer.store.symbols.info(class), SymbolInfo::Missing);

        let source_text =
            "package scala { trait Product }; package matching { case class Box(value: Int) }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let scala = packages.symbol(&["scala"]).unwrap();
        let scala_scope = packages.scope_of(scala).unwrap();
        let duplicate = store.symbols.alloc(Symbol {
            name: Name::new(store.names.intern("Product"), Namespace::Type),
            owner: Some(scala),
            kind: SymbolKind::Trait,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Builtin,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let duplicate_name = store.symbols.get(duplicate).name;
        store
            .scopes
            .get_mut(scala_scope)
            .enter(duplicate_name, duplicate);
        let class = parsed
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
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let checkpoint = typer.store.checkpoint();
        assert!(matches!(
            typer.complete_symbol(class),
            Err(TyperError::SymbolResolution { .. })
        ));
        assert_eq!(typer.store.checkpoint(), checkpoint);
        assert_eq!(*typer.store.symbols.info(class), SymbolInfo::Missing);

        let source_text =
            "package scala {}; package foreign {}; package matching { case class Box(value: Int) }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let class = parsed
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
        let foreign = packages.symbol(&["foreign"]).unwrap();
        let product = store.symbols.alloc(Symbol {
            name: Name::new(store.names.intern("Product"), Namespace::Type),
            owner: Some(foreign),
            kind: SymbolKind::Trait,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Builtin,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        )
        .with_resolver(Box::new(ProductResolver {
            product,
            scala_package: None,
            should_fail: Rc::new(Cell::new(false)),
            should_resolve: Rc::new(Cell::new(true)),
            should_resolve_package: Rc::new(Cell::new(false)),
        }));
        let checkpoint = typer.store.checkpoint();
        assert!(matches!(
            typer.complete_symbol(class),
            Err(TyperError::SymbolResolution { .. })
        ));
        assert_eq!(typer.store.checkpoint(), checkpoint);
        assert_eq!(*typer.store.symbols.info(class), SymbolInfo::Missing);
    }

    #[test]
    fn user_defined_selector_is_not_replaced_by_case_class_projection() {
        let source_text = "package scala { trait Product }; package matching { case class Custom(value: Int) { def _1: Boolean = true } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let selector_name = Name::new(store.names.intern("_1"), Namespace::Term);
        let (custom, class_tree) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Custom" =>
                {
                    index.symbol_at(source, tree).map(|class| (class, tree))
                }
                _ => None,
            })
            .expect("Custom case class");
        let class_scope = index.scope_of(custom).expect("Custom class scope");
        let user_selector = store
            .scopes
            .get(class_scope)
            .lookup(&selector_name)
            .expect("the explicit _1 remains available");
        let TreeKind::TypeDef(definition) = &parsed.ast.get(class_tree).kind else {
            panic!("Custom source tree must be a class");
        };
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            panic!("Custom RHS must be a template");
        };
        let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
            panic!("Custom primary constructor must be a method");
        };
        let parameter_tree = constructor.value_param_clauses[0][0];
        assert_eq!(
            index.derived_symbol_at(custom, source, parameter_tree),
            None
        );
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let class_info = typer.complete_symbol(custom).expect("complete Custom");
        let Type::ClassInfo(info) = typer.store().types.get(class_info) else {
            panic!("class completion must produce ClassInfo");
        };
        assert!(
            !info
                .parents
                .iter()
                .any(|parent| match typer.store().types.try_get(*parent) {
                    Some(Type::TypeRef {
                        target: TypeRefTarget::Symbol(parent),
                        ..
                    }) => {
                        typer
                            .store()
                            .names
                            .resolve(typer.store().symbols.get(*parent).name.text())
                            == "Product"
                    }
                    _ => false,
                })
        );
        assert_eq!(
            typer
                .store()
                .scopes
                .get(class_scope)
                .lookup_all(&selector_name),
            &[user_selector],
            "completion does not add an overloaded synthetic selector"
        );
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

    fn type_match_error(source_text: &str) -> (TyperError, bool, bool) {
        let (parsed, mut store, packages, _definitions, index, source) = setup(source_text);
        let method = method_symbol(&parsed, &store, &index, source);
        let match_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::Match(_)).then_some(tree))
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            _definitions,
            &index,
            source,
            method,
        );
        let checkpoint = typer.store.checkpoint();
        let error = typer.type_expression(match_tree, context).unwrap_err();
        (
            error,
            typer.store.checkpoint() == checkpoint,
            typer.typed_arena.iter().next().is_none() && typer.typed_index.is_empty(),
        )
    }

    fn extractor_plan_error(
        source_text: &str,
        selector_is_boolean: bool,
    ) -> (TyperError, bool, bool) {
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let TreeKind::Apply(application) = &parsed.ast.get(pattern).kind else {
            panic!("expected source extractor Apply")
        };
        let application = application.clone();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let checkpoint = typer.store.checkpoint();
        let result = typer.run_expression_transaction(|typer, journal, mappings| {
            typer.resolve_extractor_pattern_plan(
                pattern,
                &application,
                if selector_is_boolean {
                    definitions.boolean
                } else {
                    definitions.int
                },
                context,
                journal,
                mappings,
            )
        });
        let error = match result {
            Err(error) => error,
            Ok(plan) => panic!("expected extractor failure for {source_text:?}, got {plan:?}"),
        };
        (
            error,
            typer.store.checkpoint() == checkpoint,
            typer.typed_arena.iter().next().is_none() && typer.typed_index.is_empty(),
        )
    }

    #[test]
    fn generic_either_case_extractors_instantiate_in_parent_parameter_order() {
        let source_text = r#"
package scala { trait Product }
package scala.util {
  sealed abstract class Either[+A, +B] extends Product
  final case class Left[+X, +Y](value: X) extends Either[X, Y]
  final case class Right[+X, +Y](value: Y) extends Either[X, Y]
  final case class Reversed[+X, +Y](value: X) extends Either[Y, X]
  final case class Unrelated[+X, +Y](value: X) extends Product
  def choose[A, B](value: Either[A, B]): Int = value match {
    case Left(a) => 1
    case Right(b) => 2
    case Reversed(x) => 3
    case Unrelated(y) => 4
    case Left(a, b) => 5
  }
}
"#;
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let method = method_symbol(&parsed, &store, &index, source);
        let method_type_parameters = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition) if index.symbol_at(source, tree) == Some(method) => {
                    Some(
                        definition
                            .type_params
                            .iter()
                            .map(|parameter| index.symbol_at(source, *parameter).unwrap())
                            .collect::<Vec<_>>(),
                    )
                }
                _ => None,
            })
            .unwrap();
        let cases = parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| matches!(node.kind, TreeKind::CaseDef(_)).then_some(tree))
            .collect::<Vec<_>>();
        assert_eq!(cases.len(), 5);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let value_tpt = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition) if index.symbol_at(source, tree) == Some(method) => {
                    let TreeKind::ValDef(parameter) =
                        &parsed.ast.get(definition.value_param_clauses[0][0]).kind
                    else {
                        panic!("method parameter")
                    };
                    Some(parameter.tpt)
                }
                _ => None,
            })
            .unwrap();
        let selector_context = typer.expression_type_context(context).unwrap();
        let selector_type = typer
            .type_of_tpt_inner_journaled(value_tpt, selector_context, &mut Vec::new())
            .unwrap();
        let mut binders = Vec::new();
        let source_class = |name: &str| {
            parsed
                .ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::TypeDef(definition)
                        if typer.store.names.resolve(definition.name.as_name().text()) == name =>
                    {
                        index.symbol_at(source, tree)
                    }
                    _ => None,
                })
                .unwrap()
        };
        let left_class = source_class("Left");
        let right_class = source_class("Right");
        for (case_index, case_tree) in cases.iter().copied().take(2).enumerate() {
            let typed_case = typer
                .run_expression_transaction(|typer, journal, mappings| {
                    typer.type_case_def(case_tree, selector_type, context, journal, mappings)
                })
                .expect("the generic extractor case types");
            let TreeKind::CaseDef(case_def) = &typer.typed_arena.get(typed_case).kind else {
                panic!("typed CaseDef")
            };
            let TreeKind::UnApply(unapply) = &typer.typed_arena.get(case_def.pattern).kind else {
                panic!("typed UnApply")
            };
            let prototype = typer.typed_arena.get(case_def.pattern).ty;
            let Type::Applied { tycon, args } = typer.store.types.get(prototype) else {
                panic!(
                    "generic extractor prototype remains applied: {:?}",
                    typer.store.types.get(prototype)
                )
            };
            let Type::TypeRef {
                target: TypeRefTarget::Symbol(prototype_class),
                ..
            } = typer.store.types.get(*tycon)
            else {
                panic!("applied prototype has canonical class tycon")
            };
            assert_eq!(
                *prototype_class,
                if case_index == 0 {
                    left_class
                } else {
                    right_class
                }
            );
            assert_eq!(
                args,
                &match typer.store.types.get(selector_type) {
                    Type::Applied { args, .. } => args.clone(),
                    other => panic!("generic Either selector: {other:?}"),
                }
            );
            assert_eq!(unapply.patterns.len(), 1);
            let binder_tree = match &parsed
                .ast
                .get(match &parsed.ast.get(case_tree).kind {
                    TreeKind::CaseDef(def) => def.pattern,
                    _ => unreachable!(),
                })
                .kind
            {
                TreeKind::Apply(application) => application.args[0],
                _ => unreachable!(),
            };
            let symbol = typer
                .pattern_binding_symbol_at(source, binder_tree)
                .expect("case-local binder");
            let info = *typer.store.symbols.info(symbol);
            binders.push((symbol, info));
            assert!(matches!(
                typer
                    .store
                    .types
                    .get(typer.typed_arena.get(unapply.function).ty),
                Type::Method(_)
            ));
        }
        assert_ne!(binders[0].0, binders[1].0);
        let (SymbolInfo::Complete(left_ty), SymbolInfo::Complete(right_ty)) =
            (binders[0].1, binders[1].1)
        else {
            panic!("complete binder types")
        };
        let left_parameter = match typer.store.types.get(left_ty) {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("Left(a) should retain A, got {other:?}"),
        };
        let right_parameter = match typer.store.types.get(right_ty) {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("Right(b) should retain B, got {other:?}"),
        };
        assert_ne!(left_parameter, right_parameter);
        assert_eq!(left_parameter, method_type_parameters[0]);
        assert_eq!(right_parameter, method_type_parameters[1]);

        for (negative_index, case_tree) in cases.iter().copied().enumerate().skip(2) {
            let store_checkpoint = typer.store.checkpoint();
            let typed_checkpoint = typer.typed_arena.iter().count();
            let error = typer
                .run_expression_transaction(|typer, journal, mappings| {
                    typer.type_case_def(case_tree, selector_type, context, journal, mappings)
                })
                .expect_err("reversed and unrelated parents are not inferred from spelling");
            if negative_index == 4 {
                assert!(
                    matches!(
                        &error,
                        TyperError::ExtractorPatternArityUnsupported { .. }
                            | TyperError::ExtractorProductSelectorCountMismatch { .. }
                    ),
                    "wrong arity rejection: {error:?}"
                );
            }
            let _typed_rejection = error;
            assert_eq!(typer.store.checkpoint(), store_checkpoint);
            assert_eq!(typer.typed_arena.iter().count(), typed_checkpoint);
        }
        typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_case_def(cases[0], selector_type, context, journal, mappings)
            })
            .expect("a valid same-session retry succeeds after negative rollback");
        let first_binder_tree = match &parsed.ast.get(cases[0]).kind {
            TreeKind::CaseDef(case_def) => match &parsed.ast.get(case_def.pattern).kind {
                TreeKind::Apply(application) => application.args[0],
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        assert_eq!(
            typer.pattern_binding_symbol_at(source, first_binder_tree),
            Some(binders[0].0)
        );
    }

    #[test]
    fn simple_extractor_plan_retains_unapply_identity_types_and_source_arguments() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "object SimpleExtractor { def unapply(value: Any): Any = value }; object ExtractorFoundation { def choose(value: Any): Int = value match { case SimpleExtractor(_) => 1; case _ => 0 } }",
        );
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let TreeKind::Apply(application) = &parsed.ast.get(pattern).kind else {
            panic!("expected source extractor Apply");
        };
        let application = application.clone();
        let unapply_symbol = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "unapply" =>
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
        let plan = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.resolve_extractor_pattern_plan(
                    pattern,
                    &application,
                    definitions.any_type,
                    context,
                    journal,
                    mappings,
                )
            })
            .unwrap();
        assert_eq!(plan.symbol, unapply_symbol);
        assert_eq!(plan.unapply_type, definitions.any_type);
        assert_eq!(plan.source_patterns, application.args);
        let SymbolInfo::Complete(unapply_signature) = typer.store.symbols.get(unapply_symbol).info
        else {
            panic!("the unapply declaration should be completed")
        };
        let Some(Type::Method(method_type)) = typer.store.types.try_get(unapply_signature) else {
            panic!("expected a method signature")
        };
        assert_eq!(method_type.params.len(), 1);
        assert_eq!(plan.input_type, method_type.params[0].ty);
        assert_eq!(plan.result_type, method_type.result);
        let TreeKind::Select(function) = &typer.typed_arena.get(plan.function).kind else {
            panic!("expected selected unapply function")
        };
        assert_eq!(
            function.qualifier,
            typer.typed_index.get(source, application.function).unwrap()
        );
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(plan.function).ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == unapply_symbol
        ));
        let forward_supertype_plan = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.resolve_extractor_pattern_plan(
                    pattern,
                    &application,
                    definitions.int,
                    context,
                    journal,
                    mappings,
                )
            })
            .unwrap();
        assert_eq!(forward_supertype_plan.input_type, plan.input_type);
        assert_eq!(
            forward_supertype_plan.unapply_type, definitions.int,
            "selector-to-unapply-supertype cases retain the original selector prototype"
        );
    }

    #[test]
    fn nominal_subtype_extractor_patterns_narrow_only_the_pattern_prototype() {
        let source_text = "package scala { trait Product }; package matching { trait Event; case class Created(id: Int) extends Event; case class Deleted(code: Int) extends Event; object Matcher { def extract(event: Event): Int = event match { case Created(id) => id; case Deleted(code) => code } } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let symbol_named = |name: &str, kind: SymbolKind| {
            parsed
                .ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::TypeDef(definition)
                        if store.names.resolve(definition.name.as_name().text()) == name
                            && store
                                .symbols
                                .get(index.symbol_at(source, tree).unwrap())
                                .kind
                                == kind =>
                    {
                        index.symbol_at(source, tree)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing {kind:?} {name}"))
        };
        let product = symbol_named("Product", SymbolKind::Trait);
        let event = symbol_named("Event", SymbolKind::Trait);
        let created = symbol_named("Created", SymbolKind::Class);
        let deleted = symbol_named("Deleted", SymbolKind::Class);
        let method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "extract" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let match_selector = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::Match(expression) => Some(expression.selector),
                _ => None,
            })
            .unwrap();
        let cases = parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| match &node.kind {
                TreeKind::CaseDef(_) => Some(tree),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(cases.len(), 2);
        let event_type = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(event),
        });
        let created_type = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(created),
        });
        let deleted_type = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(deleted),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        typer.complete_symbol(product).unwrap();
        let typed_selector = typer.type_expression(match_selector, context).unwrap();
        let selector_parameter = match typer
            .store
            .types
            .get(typer.typed_arena.get(typed_selector).ty)
        {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("unexpected source Match selector type {other:?}"),
        };
        let SymbolInfo::Complete(selector_parameter_type) =
            *typer.store.symbols.info(selector_parameter)
        else {
            panic!("source Match selector parameter has a completed type")
        };
        assert!(matches!(
            typer.store.types.try_get(selector_parameter_type),
            Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. })
                if *symbol == event
        ));
        let created_unapply = index
            .generated_case_class_method(created, dotty_core::GeneratedCaseClassMethodRole::Unapply)
            .unwrap();
        let deleted_unapply = index
            .generated_case_class_method(deleted, dotty_core::GeneratedCaseClassMethodRole::Unapply)
            .unwrap();
        let canonical_int = match typer.store.types.get(definitions.int) {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("unexpected canonical Int type {other:?}"),
        };
        let selector_name = Name::new(typer.store.names.intern("_1"), Namespace::Term);
        let mut binder_symbols = Vec::new();

        for (case_tree, expected_class, expected_unapply, binder) in [
            (cases[0], created, created_unapply, "id"),
            (cases[1], deleted, deleted_unapply, "code"),
        ] {
            let typed_case = typer
                .run_expression_transaction(|typer, journal, mappings| {
                    typer.type_case_def(case_tree, event_type, context, journal, mappings)
                })
                .unwrap();
            let TreeKind::CaseDef(case_def) = &typer.typed_arena.get(typed_case).kind else {
                panic!("expected typed case definition")
            };
            let typed = case_def.pattern;
            let node = typer.typed_arena.get(typed);
            assert!(
                matches!(
                    typer.store.types.try_get(node.ty),
                    Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. })
                        if *symbol == expected_class
                ),
                "the pattern prototype is the authentic narrowed variant"
            );
            let TreeKind::UnApply(unapply) = &node.kind else {
                panic!("expected typed UnApply, found {:?}", node.kind)
            };
            let TreeKind::Select(selection) = &typer.typed_arena.get(unapply.function).kind else {
                panic!("expected selected unapply method")
            };
            let selected_name = selection.name;
            assert!(matches!(
                typer.store.types.try_get(typer.typed_arena.get(unapply.function).ty),
                Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. })
                    if *symbol == expected_unapply
            ));
            let owner = typer.store.symbols.get(expected_unapply).owner.unwrap();
            assert_eq!(typer.store.symbols.get(owner).kind, SymbolKind::ModuleClass);
            assert_eq!(
                index
                    .generated_case_class_method_identity(expected_unapply)
                    .unwrap()
                    .class,
                expected_class,
                "the source-generated extractor owner remains variant-specific"
            );
            assert_eq!(unapply.patterns.len(), 1);
            let TreeKind::Bind(binding) = &typer.typed_arena.get(unapply.patterns[0]).kind else {
                panic!("expected nested binder")
            };
            assert_eq!(typer.store.names.resolve(binding.name.text()), binder);
            let binder_type = typer.typed_arena.get(unapply.patterns[0]).ty;
            let selector_symbol = match typer.store.types.get(binder_type) {
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } => *symbol,
                other => panic!("unexpected extractor component type {other:?}"),
            };
            assert_eq!(typer.store.symbols.get(selector_symbol).owner, Some(method));
            binder_symbols.push(selector_symbol);
            let SymbolInfo::Complete(binding_info) = *typer.store.symbols.info(selector_symbol)
            else {
                panic!("nested binder has a completed semantic type")
            };
            let binding_int = match typer.store.types.get(binding_info) {
                Type::TypeRef {
                    target: TypeRefTarget::Symbol(symbol),
                    ..
                } => *symbol,
                other => panic!("unexpected nested binder type {other:?}"),
            };
            assert_eq!(binding_int, canonical_int);
            let unapply_info = typer.complete_symbol(expected_unapply).unwrap();
            let Type::Method(unapply_method) = typer.store.types.get(unapply_info) else {
                panic!("generated unapply method has a method type")
            };
            let selectors = typer
                .lookup_overload_members_journaled(
                    unapply_method.result,
                    selector_name,
                    &mut Vec::new(),
                )
                .unwrap();
            assert_eq!(selectors.len(), 1);
            let product_selector = selectors[0].symbol;
            assert_eq!(
                typer.store.symbols.get(product_selector).owner,
                Some(expected_class),
                "the selected `_1` declaration belongs to the matching variant"
            );
            let SymbolInfo::Complete(selector_info) = *typer.store.symbols.info(product_selector)
            else {
                panic!("the matching variant's `_1` selector is complete")
            };
            let Type::Method(selector_method) = typer.store.types.get(selector_info) else {
                panic!("the matching `_1` selector is a method")
            };
            assert!(selector_method.params.is_empty());
            assert_eq!(selector_method.result, definitions.int);
            assert_eq!(typer.store.names.resolve(selected_name.text()), "unapply");
        }
        assert_ne!(binder_symbols[0], binder_symbols[1]);
        assert!(!typer.conforms(event_type, created_type).unwrap());
        assert!(typer.conforms(created_type, event_type).unwrap());
        assert!(!typer.conforms(event_type, deleted_type).unwrap());
        assert!(typer.conforms(deleted_type, event_type).unwrap());
    }

    #[test]
    fn unrelated_extractor_input_stays_deferred_after_nominal_narrowing_attempt() {
        let source_text = "package scala { trait Product }; package matching { trait Event; class Foreign; object ForeignExtractor { def unapply(value: Foreign): Int = 1 }; object Matcher { def choose(event: Event): Int = event match { case ForeignExtractor(_) => 1 } } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let class_symbol = |name: &str| {
            parsed
                .ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::TypeDef(definition)
                        if store.names.resolve(definition.name.as_name().text()) == name =>
                    {
                        index.symbol_at(source, tree)
                    }
                    _ => None,
                })
                .unwrap()
        };
        let event = class_symbol("Event");
        let product = class_symbol("Product");
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
        let case_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::CaseDef(_)).then_some(tree))
            .unwrap();
        let event_type = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(event),
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        typer.complete_symbol(product).unwrap();
        let checkpoint = typer.store.checkpoint();
        let error = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_case_def(case_tree, event_type, context, journal, mappings)
            })
            .unwrap_err();
        assert!(matches!(
            error,
            TyperError::ExtractorPatternConstraintDeferred {
                selector,
                input: _,
                error: None,
                ..
            } if selector == event_type
        ));
        assert_eq!(typer.store.checkpoint(), checkpoint);
        assert!(typer.typed_arena.iter().next().is_none());
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn failure_after_narrowing_rolls_back_and_same_session_retry_succeeds() {
        let source_text = "package scala { trait Product }; package matching { trait Event; case class Created(id: Int) extends Event; object Matcher { def choose(event: Event): Int = event match { case Created(id) if id => 0; case Created(value) if true => value } } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let symbol_named = |name: &str, kind: SymbolKind| {
            parsed
                .ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::TypeDef(definition)
                        if store.names.resolve(definition.name.as_name().text()) == name
                            && store
                                .symbols
                                .get(index.symbol_at(source, tree).unwrap())
                                .kind
                                == kind =>
                    {
                        index.symbol_at(source, tree)
                    }
                    _ => None,
                })
                .unwrap()
        };
        let event = symbol_named("Event", SymbolKind::Trait);
        let product = symbol_named("Product", SymbolKind::Trait);
        let created = symbol_named("Created", SymbolKind::Class);
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
        let cases = parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| matches!(node.kind, TreeKind::CaseDef(_)).then_some(tree))
            .collect::<Vec<_>>();
        assert_eq!(cases.len(), 2);
        let created_unapply = index
            .generated_case_class_method(created, dotty_core::GeneratedCaseClassMethodRole::Unapply)
            .unwrap();
        let method_definition = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition) if index.symbol_at(source, tree) == Some(method) => {
                    Some(definition)
                }
                _ => None,
            })
            .unwrap();
        let event_tpt = match &parsed
            .ast
            .get(method_definition.value_param_clauses[0][0])
            .kind
        {
            TreeKind::ValDef(parameter) => parameter.tpt,
            _ => unreachable!(),
        };
        let TreeKind::CaseDef(first_case) = &parsed.ast.get(cases[0]).kind else {
            unreachable!()
        };
        let TreeKind::CaseDef(retry_case) = &parsed.ast.get(cases[1]).kind else {
            unreachable!()
        };
        let first_case_pattern = first_case.pattern;
        let first_case_guard = first_case.guard.unwrap();
        let retry_case_pattern = retry_case.pattern;
        let TreeKind::Apply(first_extractor) = &parsed.ast.get(first_case_pattern).kind else {
            panic!("expected an extractor pattern")
        };
        let first_binder_tree = first_extractor.args[0];
        let TreeKind::Apply(retry_extractor) = &parsed.ast.get(retry_case_pattern).kind else {
            panic!("expected a retry extractor pattern")
        };
        let retry_binder_tree = retry_extractor.args[0];
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        typer.complete_symbol(product).unwrap();
        let selector_context = typer.expression_type_context(context).unwrap();
        let event_type = typer
            .type_of_tpt_inner_journaled(event_tpt, selector_context, &mut Vec::new())
            .unwrap();
        let store_checkpoint = typer.store.checkpoint();
        let type_index_checkpoint = typer.type_index.checkpoint();
        let symbol_snapshots = [event, product, created, created_unapply]
            .map(|symbol| (symbol, *typer.store.symbols.info(symbol)));
        let scope_ids = [event, created, method]
            .map(|symbol| index.scope_of(symbol).expect("source symbol scope"));
        let scope_snapshots = scope_ids.map(|scope| {
            let mut entries = typer
                .store
                .scopes
                .get(scope)
                .entered_symbols()
                .collect::<Vec<_>>();
            entries.sort_by_key(|symbol| symbol.index());
            entries
        });
        let pattern_binding_checkpoint = typer.pattern_bindings.clone();
        let error = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_case_def(cases[0], event_type, context, journal, mappings)
            })
            .unwrap_err();
        assert!(
            matches!(
                error,
                TyperError::ExpectedExpressionTypeMismatch {
                    source: error_source,
                    tree_index,
                    actual,
                    expected,
                } if error_source == source
                    && tree_index == first_case_guard.index()
                    && actual == definitions.int
                    && expected == definitions.boolean
            ),
            "expected the bound Int value to fail as a Boolean guard, got {error:?}"
        );
        assert_eq!(typer.store.checkpoint(), store_checkpoint);
        for (symbol, expected_info) in symbol_snapshots {
            assert_eq!(*typer.store.symbols.info(symbol), expected_info);
        }
        for (tree, _) in parsed.ast.iter() {
            assert_eq!(
                typer.type_index.type_at(source, tree),
                type_index_checkpoint.type_at(source, tree),
                "source type projection changed for {tree:?}"
            );
        }
        for (scope, expected_entries) in scope_ids.into_iter().zip(scope_snapshots) {
            let mut actual_entries = typer
                .store
                .scopes
                .get(scope)
                .entered_symbols()
                .collect::<Vec<_>>();
            actual_entries.sort_by_key(|symbol| symbol.index());
            assert_eq!(
                actual_entries, expected_entries,
                "pre-existing scope {scope:?} changed"
            );
        }
        assert_eq!(
            typer.pattern_bindings.by_tree,
            pattern_binding_checkpoint.by_tree
        );
        assert_eq!(
            typer.pattern_bindings.scope_by_symbol,
            pattern_binding_checkpoint.scope_by_symbol
        );
        assert_eq!(
            typer.pattern_binding_symbol_at(source, first_binder_tree),
            None
        );
        assert!(typer.typed_arena.iter().next().is_none());
        assert!(typer.typed_index.is_empty());

        let retried = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_case_def(cases[1], event_type, context, journal, mappings)
            })
            .expect("a valid same-session retry succeeds after rollback");
        let TreeKind::CaseDef(retried_case) = &typer.typed_arena.get(retried).kind else {
            panic!("same-session retry returns a typed case")
        };
        let TreeKind::UnApply(retried_unapply) = &typer.typed_arena.get(retried_case.pattern).kind
        else {
            panic!("retry pattern retains typed UnApply")
        };
        assert!(matches!(
            typer.store.types.get(typer.typed_arena.get(retried_unapply.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if *symbol == created_unapply
        ));
        assert_eq!(retried_unapply.patterns.len(), 1);
        let typed_binder = retried_unapply.patterns[0];
        let TreeKind::Bind(binding) = &typer.typed_arena.get(typed_binder).kind else {
            panic!("retry retains its source binder")
        };
        assert_eq!(
            binding.name,
            Name::new(typer.store.names.intern("value"), Namespace::Term)
        );
        let binder_symbol = typer
            .pattern_binding_symbol_at(source, retry_binder_tree)
            .expect("retry publishes canonical binder identity");
        let binder_scope = typer
            .pattern_binding_scope(binder_symbol)
            .expect("retry publishes the binder's case scope");
        assert_eq!(
            typer.store.scopes.get(binder_scope).lookup(&binding.name),
            Some(binder_symbol)
        );
        assert_eq!(
            typer.store.symbols.get(binder_symbol).info,
            SymbolInfo::Complete(definitions.int)
        );
        assert!(matches!(
            typer.store.types.get(typer.typed_arena.get(typed_binder).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if *symbol == binder_symbol
        ));
        assert!(matches!(
            typer.store.types.get(typer.typed_arena.get(retried_case.body).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if *symbol == binder_symbol
        ));
        let unapply_info = typer.complete_symbol(created_unapply).unwrap();
        assert_eq!(
            index
                .generated_case_class_method_identity(created_unapply)
                .unwrap()
                .class,
            created
        );
        let Type::Method(unapply_method) = typer.store.types.get(unapply_info) else {
            panic!("canonical generated unapply has a method type")
        };
        let selector_name = Name::new(typer.store.names.intern("_1"), Namespace::Term);
        let selectors = typer
            .lookup_overload_members_journaled(
                unapply_method.result,
                selector_name,
                &mut Vec::new(),
            )
            .unwrap();
        assert_eq!(selectors.len(), 1);
        let product_selector = selectors[0].symbol;
        assert_eq!(
            typer.store.symbols.get(product_selector).owner,
            Some(created)
        );
        let SymbolInfo::Complete(selector_info) = *typer.store.symbols.info(product_selector)
        else {
            panic!("canonical Created._1 selector is complete after retry")
        };
        let Type::Method(selector_method) = typer.store.types.get(selector_info) else {
            panic!("canonical Created._1 selector has a method type")
        };
        assert_eq!(selector_method.result, definitions.int);
        assert!(typer.typed_index.get(source, retry_binder_tree).is_some());
    }

    #[test]
    fn extractor_resolution_collapses_a_real_inherited_override() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "trait Base { def unapply(value: Any): Any = value }; object Extractor extends Base { override def unapply(value: Any): Any = value }; class C { def choose(value: Any): Int = value match { case Extractor(_) => 1 } }",
        );
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let TreeKind::Apply(application) = &parsed.ast.get(pattern).kind else {
            panic!("expected source extractor Apply");
        };
        let application = application.clone();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );

        let plan = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.resolve_extractor_pattern_plan(
                    pattern,
                    &application,
                    definitions.any_type,
                    context,
                    journal,
                    mappings,
                )
            })
            .unwrap();
        drop(typer);

        let declarations = parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "unapply" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(declarations.len(), 2);
        assert!(declarations.contains(&plan.symbol));
    }

    #[test]
    fn extractor_resolution_reports_focused_qualifier_and_unapply_errors() {
        let cases = [
            (
                "object Extractor { def unapply(value: Any): Any = value }; class C { def choose(value: Any): Int = value match { case Missing(_) => 1 } }",
                0,
            ),
            (
                "object Extractor {}; class C { def choose(value: Any): Int = value match { case Extractor(_) => 1 } }",
                1,
            ),
            (
                "object Extractor { def unapplySeq(value: Any): Any = value }; class C { def choose(value: Any): Int = value match { case Extractor(_) => 1 } }",
                1,
            ),
            (
                "object Extractor { def unapply(value: Any): Any = value; def unapply(value: Int): Any = value }; class C { def choose(value: Any): Int = value match { case Extractor(_) => 1 } }",
                2,
            ),
            (
                "trait Base { def unapply(value: Int): Any = value }; object Extractor extends Base { def unapply(value: Any): Any = value }; class C { def choose(value: Any): Int = value match { case Extractor(_) => 1 } }",
                5,
            ),
            (
                "object Extractor { def unapply[A](value: A): Any = value }; class C { def choose(value: Any): Int = value match { case Extractor(_) => 1 } }",
                3,
            ),
            (
                "object Extractor { def unapply(value: Boolean): Any = value }; class C { def choose(value: Int): Int = value match { case Extractor(_) => 1 } }",
                4,
            ),
        ];
        for (source_text, expected) in cases {
            let (error, store_rolled_back, typed_state_rolled_back) =
                extractor_plan_error(source_text, false);
            let matched = match (expected, error) {
                (0, TyperError::ExtractorQualifierNotFound { .. })
                | (1, TyperError::ExtractorUnapplyNotFound { .. })
                | (2, TyperError::ExtractorUnapplyOverloaded { .. })
                | (5, TyperError::ExtractorUnapplyOverloaded { .. })
                | (3, TyperError::ExtractorUnapplyPolymorphic { .. })
                | (4, TyperError::ExtractorPatternConstraintDeferred { .. }) => true,
                (_, other) => panic!("unexpected extractor error: {other:?}"),
            };
            assert!(matched);
            assert!(store_rolled_back);
            assert!(typed_state_rolled_back);
        }
    }

    #[test]
    fn extractor_resolution_rejects_unstable_selected_named_and_sequence_forms() {
        let unstable = extractor_plan_error(
            "class C { var Extractor: Any = 1; def choose(value: Any): Int = value match { case Extractor(_) => 1 } }",
            false,
        );
        assert!(matches!(
            unstable.0,
            TyperError::ExtractorQualifierNotStable { .. }
        ));
        assert!(unstable.1 && unstable.2);

        let selected = extractor_plan_error(
            "object Extractor { def unapply(value: Any): Any = value }; class C { def choose(value: Any): Int = value match { case Extractor.unapply(_) => 1 } }",
            false,
        );
        assert!(matches!(
            selected.0,
            TyperError::ExtractorQualifierNotStable { .. }
        ));
        assert!(selected.1 && selected.2);

        for (source_text, issue) in [
            (
                "object Extractor { def unapply(value: Any): Any = value }; class C { def choose(value: Any): Int = value match { case Extractor(arg = _) => 1 } }",
                ExtractorPatternArgumentIssue::Named,
            ),
            (
                "object Extractor { def unapply(value: Any): Any = value }; class C { def choose(value: Any): Int = value match { case Extractor(values*) => 1 } }",
                ExtractorPatternArgumentIssue::SequenceWildcard,
            ),
        ] {
            let (error, store_rolled_back, typed_state_rolled_back) =
                extractor_plan_error(source_text, false);
            assert!(matches!(
                error,
                TyperError::ExtractorPatternArgumentUnsupported { issue: found, .. }
                    if found == issue
            ));
            assert!(store_rolled_back);
            assert!(typed_state_rolled_back);
        }
    }

    #[test]
    fn extractor_plan_rejects_source_arguments_outside_the_arena() {
        let (mut parsed, mut store, packages, definitions, index, source) = setup(
            "object Extractor { def unapply(value: Any): Any = value }; class C { def choose(value: Any): Int = value match { case Extractor(_) => 1 } }",
        );
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let TreeKind::Apply(application) = &parsed.ast.get(pattern).kind else {
            panic!("expected source extractor Apply");
        };
        let mut application = application.clone();
        let arena_checkpoint = parsed.ast.checkpoint();
        let invalid_argument = parsed.ast.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: Name::new(store.names.intern("orphan"), Namespace::Term),
                backquoted: false,
            }),
            position: None,
            ty: (),
        });
        parsed.ast.rollback_to(arena_checkpoint);
        application.args[0] = invalid_argument;
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let checkpoint = typer.store.checkpoint();

        let result = typer.run_expression_transaction(|typer, journal, mappings| {
            typer.resolve_extractor_pattern_plan(
                pattern,
                &application,
                definitions.any_type,
                context,
                journal,
                mappings,
            )
        });

        assert!(matches!(
            result,
            Err(TyperError::TreeOutsideArena { tree_index, .. })
                if tree_index == invalid_argument.index()
        ));
        assert_eq!(typer.store.checkpoint(), checkpoint);
        assert!(typer.typed_arena.iter().next().is_none());
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn extractor_unapply_callable_shapes_are_rejected_explicitly() {
        let cases = [
            (
                "def unapply(using context: Any)(value: Any): Any = value",
                ExtractorMethodShapeIssue::MethodKind,
            ),
            (
                "def unapply(): Any = 1",
                ExtractorMethodShapeIssue::ParameterArity,
            ),
            (
                "def unapply(left: Any, right: Any): Any = left",
                ExtractorMethodShapeIssue::ParameterArity,
            ),
            (
                "def unapply(value: => Any): Any = value",
                ExtractorMethodShapeIssue::ByNameParameter,
            ),
            (
                "def unapply(values: Any*): Any = values",
                ExtractorMethodShapeIssue::RepeatedParameter,
            ),
            (
                "def unapply(value: Any)(using context: Any): Any = value",
                ExtractorMethodShapeIssue::TrailingClause,
            ),
        ];
        for (declaration, expected_issue) in cases {
            let source_text = format!(
                "object Extractor {{ {declaration} }}; class C {{ def choose(value: Any): Int = value match {{ case Extractor(_) => 1 }} }}"
            );
            let (error, store_rolled_back, typed_state_rolled_back) =
                extractor_plan_error(&source_text, false);
            assert!(matches!(
                error,
                TyperError::ExtractorUnapplyShapeUnsupported { issue, .. }
                    if issue == expected_issue
            ));
            assert!(store_rolled_back);
            assert!(typed_state_rolled_back);
        }
    }

    #[test]
    fn erased_unapply_parameters_are_rejected_even_when_source_parsing_lacks_erased_params() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(value: Any): Int = value match { case _ => 1 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let (typer, _) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let erased_method = MethodType {
            params: vec![dotty_core::types::MethodParam {
                name: dotty_core::TermName::new(typer.store.names.intern("value")),
                ty: definitions.any_type,
                erased: true,
                varargs: false,
            }],
            result: definitions.any_type,
            kind: MethodKind::Plain,
        };
        assert_eq!(
            typer.extractor_method_shape_issue(&erased_method),
            Some(ExtractorMethodShapeIssue::ErasedParameter)
        );
    }

    #[test]
    fn extractor_apply_dispatch_types_a_unary_option_like_pattern() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object Extractor { def unapply(value: Any): MaybeInt = value }; class C { def choose(value: Any): Int = value match { case Extractor(_) => 1 } }",
        );
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
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
                typer.type_pattern(pattern, definitions.any_type, context, journal, mappings)
            })
            .unwrap();
        assert_eq!(typer.typed_arena.get(typed).ty, definitions.any_type);
        let TreeKind::UnApply(unapply) = &typer.typed_arena.get(typed).kind else {
            panic!("expected a typed UnApply node");
        };
        assert_eq!(unapply.patterns.len(), 1);
        assert_eq!(
            typer.typed_arena.get(unapply.patterns[0]).ty,
            definitions.int
        );
    }

    fn flagged_product_extractor_receipt(siblings: &str) -> String {
        let source_text = format!(
            "package scala {{ trait Product }}; package app {{ {siblings} case class Flagged(value: Int, enabled: Boolean); class C {{ def choose(value: Any): Int = value match {{ case Flagged(_, _) => 1; case _ => 0 }} }} }}"
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
            .expect("choose method");
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::CaseDef(case_def) => match &parsed.ast.get(case_def.pattern).kind {
                    TreeKind::Apply(application)
                        if matches!(
                            &parsed.ast.get(application.function).kind,
                            TreeKind::Ident(name)
                                if store.names.resolve(name.name.text()) == "Flagged"
                        ) =>
                    {
                        Some(case_def.pattern)
                    }
                    _ => None,
                },
                _ => None,
            })
            .expect("Flagged(_, _) pattern");
        let flagged = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Flagged" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .expect("Flagged class");
        let TreeKind::TypeDef(class_definition) = &parsed
            .ast
            .get(
                index
                    .definition_of(flagged)
                    .and_then(|definition| match definition {
                        dotty_core::SourceDefinition::Canonical { tree, .. } => Some(tree),
                        _ => None,
                    })
                    .expect("Flagged canonical source tree"),
            )
            .kind
        else {
            panic!("Flagged source definition must be a class");
        };
        let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
            panic!("Flagged must have a template");
        };
        let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
            panic!("Flagged must have a primary constructor");
        };
        let fields = &constructor.value_param_clauses[0];

        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        typer
            .complete_symbol(flagged)
            .expect("source Flagged class and Product ancestry must complete first");
        let flagged_type = typer.store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(flagged),
        });
        let typed = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, flagged_type, context, journal, mappings)
            })
            .expect("ordinary generated unapply must project Flagged's Product selectors");
        let TreeKind::UnApply(unapply) = &typer.typed_arena.get(typed).kind else {
            panic!("expected a typed extractor pattern");
        };
        assert_eq!(unapply.patterns.len(), 2);
        assert_eq!(
            typer.typed_arena.get(unapply.patterns[0]).ty,
            definitions.int
        );
        assert_eq!(
            typer.typed_arena.get(unapply.patterns[1]).ty,
            definitions.boolean
        );
        let Some(Type::TermRef {
            target: TermRefTarget::Symbol(unapply_symbol),
            ..
        }) = typer
            .store
            .types
            .try_get(typer.typed_arena.get(unapply.function).ty)
        else {
            panic!("generated Flagged.unapply must be the selected function");
        };
        assert_eq!(
            typer.index.generated_case_class_method(
                flagged,
                dotty_core::GeneratedCaseClassMethodRole::Unapply,
            ),
            Some(*unapply_symbol)
        );
        let class_scope = typer.index.scope_of(flagged).unwrap();
        for (position, (spelling, expected_type)) in
            [("_1", definitions.int), ("_2", definitions.boolean)]
                .into_iter()
                .enumerate()
        {
            let selector_name = TermName::new(typer.store.names.intern(spelling));
            let selector = typer
                .store
                .scopes
                .get(class_scope)
                .lookup(selector_name.as_name())
                .expect("canonical source selector");
            let original_field = typer
                .index
                .symbol_at(source, fields[position])
                .expect("original constructor field");
            assert_ne!(selector, original_field);
            assert_eq!(
                typer.source_case_class_product_selector_owner(selector),
                Some(flagged)
            );
            let SymbolInfo::Complete(selector_info) = *typer.store.symbols.info(selector) else {
                panic!("selector must have completed SymbolInfo");
            };
            let Type::Method(method) = typer.store.types.get(selector_info) else {
                panic!("selector must have completed MethodType");
            };
            assert_eq!(method.result, expected_type);
        }
        let selector_3 = TermName::new(typer.store.names.intern("_3"));
        assert_eq!(
            typer
                .store
                .scopes
                .get(class_scope)
                .lookup(selector_3.as_name()),
            None
        );
        "class=Flagged;fields=value:Int,enabled:Boolean;selectors=_1:Int@Flagged,_2:Boolean@Flagged;parents=Object,scala.Product;unapply=Flagged$:Flagged;components=Int,Boolean;_3=absent".to_owned()
    }

    #[test]
    fn recursive_source_product2_generated_unapply_projects_two_distinct_expr_selectors() {
        let source_text = "package scala { trait Product }; package app { sealed trait Expr; final case class Num(value: Int) extends Expr; final case class Add(left: Expr, right: Expr) extends Expr; class C { def choose(value: Expr): Int = value match { case Add(_, _) => 1 } } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
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
            .expect("choose method");
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::CaseDef(case_def) => match &parsed.ast.get(case_def.pattern).kind {
                    TreeKind::Apply(application)
                        if matches!(
                            &parsed.ast.get(application.function).kind,
                            TreeKind::Ident(name)
                                if store.names.resolve(name.name.text()) == "Add"
                        ) =>
                    {
                        Some(case_def.pattern)
                    }
                    _ => None,
                },
                _ => None,
            })
            .expect("Add(_, _) extractor pattern");
        let expr = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Expr" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .expect("Expr trait");
        let product = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Product" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .expect("canonical scala.Product fixture");
        let add = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "Add" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .expect("Add class");
        let class_tree = match index.definition_of(add).unwrap() {
            dotty_core::SourceDefinition::Canonical { tree, .. } => tree,
            other => panic!("unexpected Add definition {other:?}"),
        };
        let TreeKind::TypeDef(definition) = &parsed.ast.get(class_tree).kind else {
            panic!("Add is a class")
        };
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            panic!("Add has a template")
        };
        let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
            panic!("Add has a constructor")
        };
        let fields = constructor.value_param_clauses[0].clone();
        let class_scope = index.scope_of(add).unwrap();
        let selector_1 = store
            .scopes
            .get(class_scope)
            .lookup(&Name::new(store.names.intern("_1"), Namespace::Term))
            .unwrap();
        let selector_2 = store
            .scopes
            .get(class_scope)
            .lookup(&Name::new(store.names.intern("_2"), Namespace::Term))
            .unwrap();
        assert_ne!(selector_1, selector_2);
        assert_eq!(
            store
                .scopes
                .get(class_scope)
                .lookup(&Name::new(store.names.intern("_3"), Namespace::Term),),
            None
        );
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        typer
            .complete_symbol(add)
            .expect("Add and its selectors complete");
        typer
            .complete_symbol(product)
            .expect("canonical Product ClassInfo");
        let expr_type = typer.store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(expr),
        });
        let typed = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, expr_type, context, journal, mappings)
            })
            .expect("ordinary generated unapply projects Add's selectors");
        let TreeKind::UnApply(unapply) = &typer.typed_arena.get(typed).kind else {
            panic!("typed extractor pattern")
        };
        assert_eq!(unapply.patterns.len(), 2);
        let component_types = unapply
            .patterns
            .iter()
            .map(|pattern| typer.typed_arena.get(*pattern).ty)
            .collect::<Vec<_>>();
        assert_eq!(component_types.len(), 2);
        for component in component_types {
            assert!(matches!(
                typer.store.types.try_get(component),
                Some(Type::TypeRef {
                    target: TypeRefTarget::Symbol(symbol),
                    ..
                }) if *symbol == expr
            ));
        }
        let Some(Type::TermRef {
            target: TermRefTarget::Symbol(selected_unapply),
            ..
        }) = typer
            .store
            .types
            .try_get(typer.typed_arena.get(unapply.function).ty)
        else {
            panic!("generated unapply is selected by ordinary lookup")
        };
        assert_eq!(
            Some(*selected_unapply),
            typer.index.generated_case_class_method(
                add,
                dotty_core::GeneratedCaseClassMethodRole::Unapply
            )
        );
        let fields = fields
            .iter()
            .map(|tree| index.symbol_at(source, *tree).unwrap())
            .collect::<Vec<_>>();
        for ((field_tree, field), selector) in constructor.value_param_clauses[0]
            .iter()
            .zip(fields)
            .zip([selector_1, selector_2])
        {
            assert_eq!(typer.store.symbols.get(field).owner, Some(add));
            assert_eq!(
                typer.index.definition_of(field),
                Some(dotty_core::SourceDefinition::Canonical {
                    source,
                    tree: *field_tree
                })
            );
            assert_eq!(
                typer.index.derived_symbol_at(add, source, *field_tree),
                Some(selector)
            );
            assert_eq!(typer.store.symbols.get(selector).owner, Some(add));
            assert_eq!(
                typer.index.definition_of(selector),
                Some(dotty_core::SourceDefinition::Derived {
                    source,
                    tree: *field_tree
                })
            );
        }
    }

    #[test]
    fn real_flagged_product_extractor_reads_ordered_source_selectors() {
        let ordered = "case class Earlier(flag: Boolean); case class Later(code: Int);";
        let expected = flagged_product_extractor_receipt(ordered);
        assert_eq!(flagged_product_extractor_receipt(ordered), expected);
        assert_eq!(
            flagged_product_extractor_receipt(
                "case class Later(code: Int); case class Earlier(flag: Boolean);"
            ),
            expected,
            "reordering sibling declarations must preserve the normalized projection receipt"
        );
    }

    #[test]
    fn binary_product_extractors_type_ordered_components_for_direct_and_get_results() {
        let source_text = "package scala { trait Product }; package app { class PairResult extends scala.Product { def _1: Int = 1; def _2: Boolean = true; def _3(index: Int): Int = index }; class MaybePair extends scala.Product { def _1: Boolean = true; def _2: Int = 2; def _3: Int = 3; def isEmpty: Boolean = false; def get: PairResult = new PairResult }; object DirectPair { def unapply(value: Any): PairResult = new PairResult }; object GetPair { def unapply(value: Any): MaybePair = new MaybePair }; class C { def direct(value: Any): Boolean = value match { case DirectPair(a, b) if accepts(a, b) => b; case _ => false }; def throughGet(value: Any): Boolean = value match { case GetPair(_, b) => b; case _ => false }; def accepts(a: Int, b: Boolean): Boolean = true } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);

        for (method_name, extractor_name, expected_component_types) in [
            (
                "direct",
                "DirectPair",
                [definitions.int, definitions.boolean],
            ),
            (
                "throughGet",
                "GetPair",
                [definitions.int, definitions.boolean],
            ),
        ] {
            let (method, rhs) = parsed
                .ast
                .iter()
                .find_map(|(tree, node)| {
                    let TreeKind::DefDef(definition) = &node.kind else {
                        return None;
                    };
                    (store.names.resolve(definition.name.as_name().text()) == method_name).then(
                        || {
                            let match_tree = definition.rhs?;
                            let TreeKind::Match(_) = &parsed.ast.get(match_tree).kind else {
                                return None;
                            };
                            Some((index.symbol_at(source, tree)?, match_tree))
                        },
                    )?
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
            let typed_match = typer
                .type_expression(rhs, context)
                .unwrap_or_else(|error| panic!("{method_name}: {error:?}"));
            let TreeKind::Match(matching) = &typer.typed_arena.get(typed_match).kind else {
                panic!("expected a Typed Match");
            };
            let TreeKind::CaseDef(case_def) = &typer.typed_arena.get(matching.cases[0]).kind else {
                panic!("expected a Typed CaseDef");
            };
            let typed = case_def.pattern;
            let TreeKind::UnApply(unapply) = &typer.typed_arena.get(typed).kind else {
                panic!("expected a Typed UnApply node");
            };
            assert_eq!(unapply.patterns.len(), 2);
            assert_eq!(unapply.implicits.len(), 0);
            for (child, expected_type) in unapply.patterns.iter().zip(expected_component_types) {
                let child_type = typer.typed_arena.get(*child).ty;
                let actual_type = match typer.store.types.try_get(child_type) {
                    Some(Type::TermRef {
                        target: TermRefTarget::Symbol(symbol),
                        ..
                    }) => match *typer.store.symbols.info(*symbol) {
                        SymbolInfo::Complete(binding_type) => binding_type,
                        info => panic!("pattern binder has incomplete info: {info:?}"),
                    },
                    _ => child_type,
                };
                assert_eq!(
                    typer.store.types.try_get(actual_type),
                    typer.store.types.try_get(expected_type),
                    "component type mismatch: actual {actual_type:?}, expected {expected_type:?}"
                );
            }
            let Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) = typer
                .store
                .types
                .try_get(typer.typed_arena.get(unapply.function).ty)
            else {
                panic!("expected exact unapply function reference");
            };
            assert_eq!(
                typer
                    .store
                    .names
                    .resolve(typer.store.symbols.get(*symbol).name.text()),
                "unapply"
            );
            let owner = typer.store.symbols.get(*symbol).owner.unwrap();
            let owner_name = typer
                .store
                .names
                .resolve(typer.store.symbols.get(owner).name.text());
            assert_eq!(
                owner_name.strip_suffix('$').unwrap_or(owner_name),
                extractor_name
            );
        }

        let non_product = "package scala { trait Product }; package app { class PairResult { def _1: Int = 1; def _2: Boolean = true }; object Extractor { def unapply(value: Any): PairResult = new PairResult }; class C { def choose(value: Any): Int = value match { case Extractor(a, b) => a; case _ => 0 } } }";
        let (error, store_rolled_back, typed_state_rolled_back) = type_match_error(non_product);
        assert!(matches!(
            error,
            TyperError::ExtractorResultMemberNotFound { .. }
                | TyperError::UnsupportedExtractorResultProtocol { .. }
        ));
        assert!(store_rolled_back);
        assert!(typed_state_rolled_back);
    }

    #[test]
    fn binary_product_extractors_reject_wrong_selector_and_source_arities() {
        for (product, expected_selectors) in [
            (
                "class ProductResult extends scala.Product { def _1: Int = 1 }",
                1,
            ),
            (
                "class ProductResult extends scala.Product { def _1: Int = 1; def _2: Int = 2; def _3: Int = 3 }",
                3,
            ),
        ] {
            let source_text = format!(
                "package scala {{ trait Product }}; package app {{ {product}; object Extractor {{ def unapply(value: Any): ProductResult = new ProductResult }}; class C {{ def choose(value: Any): Int = value match {{ case Extractor(a, b) => 1; case _ => 0 }} }} }}"
            );
            let (error, store_rolled_back, typed_state_rolled_back) =
                type_match_error(&source_text);
            assert!(matches!(
                error,
                TyperError::ExtractorProductSelectorCountMismatch { selectors, .. }
                    if selectors.len() == expected_selectors
            ));
            assert!(store_rolled_back);
            assert!(typed_state_rolled_back);
        }

        for source_pattern in ["Extractor(a)", "Extractor(a, b, c)"] {
            let source_text = format!(
                "package scala {{ trait Product }}; package app {{ class ProductResult extends scala.Product {{ def _1: Int = 1; def _2: Int = 2 }}; object Extractor {{ def unapply(value: Any): ProductResult = new ProductResult }}; class C {{ def choose(value: Any): Int = value match {{ case {source_pattern} => 1; case _ => 0 }} }} }}"
            );
            let (error, store_rolled_back, typed_state_rolled_back) =
                type_match_error(&source_text);
            assert!(matches!(
                error,
                TyperError::ExtractorPatternArityUnsupported { actual: 1, .. }
                    | TyperError::ExtractorProductSelectorCountMismatch { .. }
            ));
            assert!(store_rolled_back);
            assert!(typed_state_rolled_back);
        }
    }

    #[test]
    fn unary_product_extractor_types_its_single_component() {
        let source_text = "package scala { trait Product }; package app { class ProductResult extends scala.Product { def _1: Int = 1 }; object Extractor { def unapply(value: Any): ProductResult = new ProductResult }; class C { def choose(value: Any): Int = value match { case Extractor(number) => number; case _ => 0 } } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let method = method_symbol(&parsed, &store, &index, source);
        let match_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::Match(_)).then_some(tree))
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

        let typed_match = typer.type_expression(match_tree, context).unwrap();
        let TreeKind::Match(matching) = &typer.typed_arena.get(typed_match).kind else {
            panic!("expected a Typed Match");
        };
        let TreeKind::CaseDef(case) = &typer.typed_arena.get(matching.cases[0]).kind else {
            panic!("expected a Typed CaseDef");
        };
        let TreeKind::UnApply(unapply) = &typer.typed_arena.get(case.pattern).kind else {
            panic!("expected a Typed UnApply");
        };
        assert_eq!(unapply.patterns.len(), 1);
        let child = unapply.patterns[0];
        let typed_binding = typer.typed_arena.get(child);
        let actual_type = match typer.store.types.try_get(typed_binding.ty) {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) => match *typer.store.symbols.info(*symbol) {
                SymbolInfo::Complete(binding_type) => binding_type,
                info => panic!("pattern binder has incomplete info: {info:?}"),
            },
            _ => typed_binding.ty,
        };
        assert_eq!(actual_type, definitions.int);
    }

    #[test]
    fn unary_extractor_falls_back_to_get_after_product_arity_mismatch() {
        let source_text = "package scala { trait Product }; package app { class ProductResult extends scala.Product { def _1: Boolean = true; def _2: Boolean = false; def isEmpty: Boolean = false; def get: Int = 1 }; object Extractor { def unapply(value: Any): ProductResult = new ProductResult }; class C { def choose(value: Any): Int = value match { case Extractor(number) => number; case _ => 0 } } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let method = method_symbol(&parsed, &store, &index, source);
        let match_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::Match(_)).then_some(tree))
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

        let typed_match = typer.type_expression(match_tree, context).unwrap();
        let TreeKind::Match(matching) = &typer.typed_arena.get(typed_match).kind else {
            panic!("expected a Typed Match");
        };
        let TreeKind::CaseDef(case) = &typer.typed_arena.get(matching.cases[0]).kind else {
            panic!("expected a Typed CaseDef");
        };
        let TreeKind::UnApply(unapply) = &typer.typed_arena.get(case.pattern).kind else {
            panic!("expected a Typed UnApply");
        };
        assert_eq!(unapply.patterns.len(), 1);
        let child_type = typer.typed_arena.get(unapply.patterns[0]).ty;
        let actual_type = match typer.store.types.try_get(child_type) {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) => match *typer.store.symbols.info(*symbol) {
                SymbolInfo::Complete(binding_type) => binding_type,
                info => panic!("pattern binder has incomplete info: {info:?}"),
            },
            _ => child_type,
        };
        assert_eq!(
            typer.store.types.try_get(actual_type),
            typer.store.types.try_get(definitions.int)
        );
    }

    #[test]
    fn nary_product_extractors_type_components_recursively_in_order() {
        let source_text = "package scala { trait Product }; package app { class PairResult extends scala.Product { def _1: Int = 1; def _2: Boolean = true }; class TripleResult extends scala.Product { def _1: PairResult = new PairResult; def _2: Boolean = true; def _3: Int = 3 }; object Pair { def unapply(value: Any): PairResult = new PairResult }; object Triple { def unapply(value: Any): TripleResult = new TripleResult }; class C { def choose(value: Any): Int = value match { case Triple(Pair(first, flag), enabled, last) if enabled => first; case _ => 0 } } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let method = method_symbol(&parsed, &store, &index, source);
        let match_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::Match(_)).then_some(tree))
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
        let typed_match = typer.type_expression(match_tree, context).unwrap();
        let TreeKind::Match(matching) = &typer.typed_arena.get(typed_match).kind else {
            panic!("expected a Typed Match");
        };
        let TreeKind::CaseDef(case) = &typer.typed_arena.get(matching.cases[0]).kind else {
            panic!("expected a Typed CaseDef");
        };
        let TreeKind::UnApply(triple) = &typer.typed_arena.get(case.pattern).kind else {
            panic!("expected a Typed UnApply for Triple");
        };
        assert_eq!(triple.patterns.len(), 3);
        assert!(matches!(
            typer.typed_arena.get(triple.patterns[0]).kind,
            TreeKind::UnApply(_)
        ));
        for (child, expected_type) in [
            (triple.patterns[1], definitions.boolean),
            (triple.patterns[2], definitions.int),
        ] {
            let child_type = typer.typed_arena.get(child).ty;
            let actual_type = match typer.store.types.try_get(child_type) {
                Some(Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                }) => match *typer.store.symbols.info(*symbol) {
                    SymbolInfo::Complete(binding_type) => binding_type,
                    info => panic!("pattern binder has incomplete info: {info:?}"),
                },
                _ => child_type,
            };
            assert_eq!(actual_type, expected_type);
        }
    }

    #[test]
    fn tuple_patterns_lower_through_canonical_source_tuple_extractors() {
        let source_text = "package scala { trait Product; class Tuple2[A, B](val _1: A, val _2: B) extends Product; class MaybeTuple2[A, B](val value: Tuple2[A, B]) { def isEmpty: Boolean = false; def get: Tuple2[A, B] = value }; object Tuple2 { def unapply[A, B](value: Tuple2[A, B]): MaybeTuple2[A, B] = new MaybeTuple2(value) }; class Tuple3[A, B, C](val _1: A, val _2: B, val _3: C) extends Product; class MaybeTuple3[A, B, C](val value: Tuple3[A, B, C]) { def isEmpty: Boolean = false; def get: Tuple3[A, B, C] = value }; object Tuple3 { def unapply[A, B, C](value: Tuple3[A, B, C]): MaybeTuple3[A, B, C] = new MaybeTuple3(value) } }; package app { class PairResult extends scala.Product { def _1: Int = 1; def _2: Boolean = true }; object Pair { def unapply(value: Any): PairResult = new PairResult }; class C { def choose2(value: scala.Tuple2[Int, Boolean]): Any = value match { case (first, second) => first; case _ => 0 }; def choose3(value: scala.Tuple3[PairResult, Int, Boolean]): Any = value match { case (Pair(nested, _), 2 | 3, last) => nested; case _ => 0 } } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);

        for (method_name, arity, expected_extractor) in
            [("choose2", 2, "Tuple2"), ("choose3", 3, "Tuple3")]
        {
            let (method, match_tree) = parsed
                .ast
                .iter()
                .find_map(|(tree, node)| {
                    let TreeKind::DefDef(definition) = &node.kind else {
                        return None;
                    };
                    if store.names.resolve(definition.name.as_name().text()) != method_name {
                        return None;
                    }
                    let match_tree = definition.rhs?;
                    if !matches!(parsed.ast.get(match_tree).kind, TreeKind::Match(_)) {
                        return None;
                    }
                    Some((index.symbol_at(source, tree)?, match_tree))
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
            let typed_match = typer
                .type_expression(match_tree, context)
                .unwrap_or_else(|error| panic!("{method_name}: {error:?}"));
            let TreeKind::Match(matching) = &typer.typed_arena.get(typed_match).kind else {
                panic!("expected a Typed Match");
            };
            let TreeKind::CaseDef(case) = &typer.typed_arena.get(matching.cases[0]).kind else {
                panic!("expected a Typed CaseDef");
            };
            let TreeKind::UnApply(tuple_unapply) = &typer.typed_arena.get(case.pattern).kind else {
                panic!("tuple patterns lower to Typed UnApply");
            };
            assert_eq!(tuple_unapply.patterns.len(), arity);
            let TreeKind::TypeApply(type_apply) =
                &typer.typed_arena.get(tuple_unapply.function).kind
            else {
                panic!("tuple extractor retains its inferred type arguments");
            };
            assert_eq!(type_apply.args.len(), arity);
            let TreeKind::Select(extractor) = &typer.typed_arena.get(type_apply.function).kind
            else {
                panic!("tuple extractor function is selected from its companion");
            };
            assert_eq!(typer.store.names.resolve(extractor.name.text()), "unapply");
            let TreeKind::Ident(companion) = &typer.typed_arena.get(extractor.qualifier).kind
            else {
                panic!("tuple companion is an identifier");
            };
            assert_eq!(
                typer.store.names.resolve(companion.name.text()),
                expected_extractor
            );
            if arity == 3 {
                assert!(matches!(
                    typer.typed_arena.get(tuple_unapply.patterns[0]).kind,
                    TreeKind::UnApply(_)
                ));
                assert!(matches!(
                    typer.typed_arena.get(tuple_unapply.patterns[1]).kind,
                    TreeKind::Alternative(_)
                ));
            }
        }
    }

    #[test]
    fn tuple_patterns_defer_when_the_canonical_tuple_class_is_unresolved() {
        let source_text = "package scala { trait Product }; package app { class C { def choose(value: Any): Int = value match { case (first, second) => first; case _ => 0 } } }";
        let (error, store_rolled_back, typed_state_rolled_back) = type_match_error(source_text);
        assert!(matches!(
            error,
            TyperError::TuplePatternResolutionDeferred {
                arity: 2,
                issue: TuplePatternResolutionIssue::TupleClassNotFound,
                ..
            }
        ));
        assert!(store_rolled_back);
        assert!(typed_state_rolled_back);
    }

    #[test]
    fn classpath_tuple_companions_without_module_classes_defer_resolution() {
        let source_text = "package scala { trait Product }; package app { class C { def choose(value: Any): Int = value match { case (first, second) => first; case _ => 0 } } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                matches!(node.kind, TreeKind::PhaseSpecific(UntypedNode::Tuple(_))).then_some(tree)
            })
            .unwrap();
        let object = store.symbols.alloc(Symbol {
            name: Name::new(store.names.intern("Tuple2"), Namespace::Term),
            owner: None,
            kind: SymbolKind::Object,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Classfile(store.origins.register_classfile()),
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let companion_type = store.types.alloc(Type::TermRef {
            prefix: definitions.no_prefix,
            target: TermRefTarget::Symbol(object),
        });
        let (mut typer, _) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );

        let error = typer
            .tuple_companion_receiver(companion_type, pattern, 2, &mut Vec::new())
            .unwrap_err();
        assert!(matches!(
            error,
            TyperError::TuplePatternResolutionDeferred {
                arity: 2,
                issue: TuplePatternResolutionIssue::CompanionModuleClassUnavailable,
                ..
            }
        ));
    }

    #[test]
    fn tuple_patterns_reject_a_selector_disjoint_from_the_canonical_tuple_type() {
        let source_text = "package scala { trait Product; class Tuple2[A, B](val _1: A, val _2: B) extends Product; class MaybeTuple2[A, B](val value: Tuple2[A, B]) { def isEmpty: Boolean = false; def get: Tuple2[A, B] = value }; object Tuple2 { def unapply[A, B](value: Tuple2[A, B]): MaybeTuple2[A, B] = new MaybeTuple2(value) } }; package app { class Unrelated {}; class C { def choose(value: Unrelated): Any = value match { case (first, second) => first; case _ => value } } }";
        let (error, store_rolled_back, typed_state_rolled_back) = type_match_error(source_text);

        assert!(
            matches!(
                &error,
                TyperError::TuplePatternTypeMismatch { .. }
                    | TyperError::TuplePatternRelationDeferred { .. }
            ),
            "{error:?}"
        );
        assert!(store_rolled_back);
        assert!(typed_state_rolled_back);
    }

    #[test]
    fn unit_pattern_is_typed_as_a_literal_without_tuple_resolution() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class C { def choose(value: Any): Int = value match { case () => 1; case _ => 0 } }",
        );
        let method = method_symbol(&parsed, &store, &index, source);
        let match_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::Match(_)).then_some(tree))
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

        typer
            .type_expression(match_tree, context)
            .unwrap_or_else(|error| panic!("{error:?}"));
    }

    #[test]
    fn infix_patterns_return_a_focused_deferred_error() {
        let source_text = "class C { def choose(value: Any): Any = value match { case left op right => left; case _ => value } }";
        let (error, store_rolled_back, typed_state_rolled_back) = type_match_error(source_text);

        assert!(matches!(error, TyperError::InfixPatternDeferred { .. }));
        assert!(store_rolled_back);
        assert!(typed_state_rolled_back);
    }

    #[test]
    fn binary_product_patterns_recurse_and_rollback_when_the_second_component_fails() {
        let nested = "package scala { trait Product }; package app { class PairResult extends scala.Product { def _1: Int = 1; def _2: Boolean = true }; class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object Pair { def unapply(value: Any): PairResult = new PairResult }; object SomeInt { def unapply(value: Int): MaybeInt = new MaybeInt }; class C { def choose(value: Any): Int = value match { case Pair(SomeInt(x), _) => x; case _ => 0 } } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(nested);
        let method = method_symbol(&parsed, &store, &index, source);
        let match_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::Match(_)).then_some(tree))
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
        assert!(typer.type_expression(match_tree, context).is_ok());

        let failure = "package scala { trait Product }; package app { class PairResult extends scala.Product { def _1: Int = 1; def _2: Boolean = true }; object Pair { def unapply(value: Any): PairResult = new PairResult }; object Even { def unapply(value: Boolean): Boolean = true }; class C { def choose(value: Any): Int = value match { case Pair(first, Even(_)) => first; case _ => 0 } } }";
        let (error, store_rolled_back, typed_state_rolled_back) = type_match_error(failure);
        assert!(matches!(
            error,
            TyperError::BooleanExtractorPatternArityUnsupported { actual: 1, .. }
        ));
        assert!(store_rolled_back);
        assert!(typed_state_rolled_back);
    }

    #[test]
    fn binary_product_patterns_accept_literals_and_reject_duplicate_bindings() {
        let literal = "package scala { trait Product }; package app { class PairResult extends scala.Product { def _1: Int = 1; def _2: Boolean = true }; object Pair { def unapply(value: Any): PairResult = new PairResult }; class C { def choose(value: Any): Boolean = value match { case Pair(1, second) => second; case _ => false } } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(literal);
        let method = method_symbol(&parsed, &store, &index, source);
        let match_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::Match(_)).then_some(tree))
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
        assert!(typer.type_expression(match_tree, context).is_ok());

        let duplicate = "package scala { trait Product }; package app { class PairResult extends scala.Product { def _1: Int = 1; def _2: Boolean = true }; object Pair { def unapply(value: Any): PairResult = new PairResult }; class C { def choose(value: Any): Int = value match { case Pair(same, same) => 1; case _ => 0 } } }";
        let (error, store_rolled_back, typed_state_rolled_back) = type_match_error(duplicate);
        assert!(matches!(error, TyperError::DuplicatePatternBinding { .. }));
        assert!(store_rolled_back);
        assert!(typed_state_rolled_back);
    }

    #[test]
    fn boolean_extractor_types_zero_patterns_and_preserves_unapply_identity() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "object Even { def unapply(value: Int): Boolean = true }; class C { def choose(value: Int): Int = value match { case Even() => 1; case _ => 0 } }",
        );
        let method = method_symbol(&parsed, &store, &index, source);
        let (pattern, unapply_symbol) = parsed
            .ast
            .iter()
            .find_map(|(_tree, node)| match &node.kind {
                TreeKind::CaseDef(case_def) => {
                    let unapply = parsed
                        .ast
                        .iter()
                        .find_map(|(tree, node)| match &node.kind {
                            TreeKind::DefDef(definition)
                                if store.names.resolve(definition.name.as_name().text())
                                    == "unapply" =>
                            {
                                index.symbol_at(source, tree)
                            }
                            _ => None,
                        })?;
                    Some((case_def.pattern, unapply))
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
        assert_eq!(typer.typed_arena.get(typed).ty, definitions.int);
        let TreeKind::UnApply(unapply) = &typer.typed_arena.get(typed).kind else {
            panic!("expected a typed UnApply node");
        };
        assert!(unapply.patterns.is_empty());
        assert!(unapply.implicits.is_empty());
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(unapply.function).ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. })
                if *symbol == unapply_symbol
        ));
        assert_eq!(typer.typed_index.get(source, pattern), Some(typed));
    }

    #[test]
    fn boolean_extractors_reject_nested_patterns_with_a_focused_error() {
        for source_text in [
            "object Even { def unapply(value: Int): Boolean = true }; class C { def choose(value: Int): Int = value match { case Even(_) => 1; case _ => 0 } }",
            "object Even { def unapply(value: Int): Boolean = true }; class C { def choose(value: Int): Int = value match { case Even(_, _) => 1; case _ => 0 } }",
        ] {
            let (error, store_rolled_back, typed_state_rolled_back) = type_match_error(source_text);
            assert!(matches!(
                error,
                TyperError::BooleanExtractorPatternArityUnsupported { actual: 1 | 2, .. }
            ));
            assert!(store_rolled_back);
            assert!(typed_state_rolled_back);
        }
    }

    #[test]
    fn selected_option_and_boolean_extractors_reuse_stable_selection_typing() {
        let source_text = "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object extractors { object SomeInt { def unapply(value: Any): MaybeInt = value }; object Even { def unapply(value: Int): Boolean = true } }; class C { def option(value: Any): Int = value match { case extractors.SomeInt(x) => x; case _ => 0 }; def boolean(value: Int): Int = value match { case extractors.Even() => 1; case _ => 0 } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);

        for (method_name, selected_name, expected_result) in [
            ("option", "SomeInt", None),
            ("boolean", "Even", Some(definitions.boolean)),
        ] {
            let (method, match_tree, pattern) = parsed
                .ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::DefDef(definition)
                        if store.names.resolve(definition.name.as_name().text()) == method_name =>
                    {
                        let match_tree = definition.rhs?;
                        let TreeKind::Match(matching) = &parsed.ast.get(match_tree).kind else {
                            return None;
                        };
                        let pattern = matching.cases.iter().find_map(|case| {
                            let TreeKind::CaseDef(case) = &parsed.ast.get(*case).kind else {
                                return None;
                            };
                            matches!(parsed.ast.get(case.pattern).kind, TreeKind::Apply(_))
                                .then_some(case.pattern)
                        })?;
                        Some((index.symbol_at(source, tree)?, match_tree, pattern))
                    }
                    _ => None,
                })
                .unwrap();
            let TreeKind::Apply(application) = &parsed.ast.get(pattern).kind else {
                panic!("expected source extractor application");
            };
            let source_function = application.function;
            let TreeKind::Select(source_selection) = &parsed.ast.get(source_function).kind else {
                panic!("expected a selected extractor qualifier");
            };
            let source_qualifier = source_selection.qualifier;
            let (mut typer, context) = context_for(
                &parsed,
                &mut store,
                &packages,
                definitions,
                &index,
                source,
                method,
            );
            let typed_match = typer.type_expression(match_tree, context).unwrap();
            let TreeKind::Match(matching) = &typer.typed_arena.get(typed_match).kind else {
                panic!("expected typed Match");
            };
            let TreeKind::CaseDef(case) = &typer.typed_arena.get(matching.cases[0]).kind else {
                panic!("expected typed CaseDef");
            };
            let typed_pattern = case.pattern;
            let TreeKind::UnApply(unapply) = &typer.typed_arena.get(typed_pattern).kind else {
                panic!("expected typed UnApply");
            };
            let TreeKind::Select(typed_function) = &typer.typed_arena.get(unapply.function).kind
            else {
                panic!("expected exact selected unapply function");
            };
            assert_eq!(
                typer.store.names.resolve(typed_function.name.text()),
                "unapply"
            );
            let unapply_symbol = typed_function_symbol(&typer, unapply.function);
            let selected_extractor_symbol = match typer
                .store
                .types
                .try_get(typer.typed_arena.get(typed_function.qualifier).ty)
            {
                Some(Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                }) => *symbol,
                other => panic!("expected selected extractor value, found {other:?}"),
            };
            let module_class = typer
                .source_module_class_of_object(selected_extractor_symbol)
                .unwrap();
            assert_eq!(
                typer.store.symbols.get(unapply_symbol).owner,
                Some(module_class)
            );
            let SymbolInfo::Complete(signature) = typer.store.symbols.get(unapply_symbol).info
            else {
                panic!("selected unapply symbol should have a complete signature");
            };
            let Some(Type::Method(signature)) = typer.store.types.try_get(signature) else {
                panic!("selected unapply should be a method");
            };
            if let Some(result_type) = expected_result {
                assert_eq!(signature.result, result_type);
                assert!(unapply.patterns.is_empty());
            } else {
                assert_eq!(
                    typer.store.names.resolve(source_selection.name.text()),
                    selected_name
                );
                assert_eq!(signature.params.len(), 1);
                let source_binder = match &parsed.ast.get(application.args[0]).kind {
                    TreeKind::Ident(ident) => ident.name,
                    other => panic!("expected nested binder, found {other:?}"),
                };
                let binder = typer
                    .pattern_binding_symbol_at(source, application.args[0])
                    .unwrap();
                assert_eq!(
                    typer.store.symbols.get(binder).info,
                    SymbolInfo::Complete(definitions.int),
                );
                assert_eq!(
                    typer.store.symbols.get(binder).name.text(),
                    source_binder.text()
                );
            }
            assert_eq!(
                typer.typed_index.get(source, source_function),
                Some(typed_function.qualifier)
            );
            assert!(typer.typed_index.get(source, source_qualifier).is_some());
            assert_eq!(typer.typed_index.get(source, pattern), Some(typed_pattern));
            assert_ne!(
                typer.typed_index.get(source, pattern),
                Some(unapply.function)
            );
        }
    }

    #[test]
    fn package_qualified_boolean_extractor_uses_source_package_resolution() {
        let source_text = "package p { object Extractor { def unapply(value: Int): Boolean = true } }; package client { class C { def choose(value: Int): Int = value match { case p.Extractor() => 1; case _ => 0 } } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let (method, match_tree, pattern) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "choose" =>
                {
                    let match_tree = definition.rhs?;
                    let TreeKind::Match(matching) = &parsed.ast.get(match_tree).kind else {
                        return None;
                    };
                    let TreeKind::CaseDef(case) = &parsed.ast.get(matching.cases[0]).kind else {
                        return None;
                    };
                    Some((index.symbol_at(source, tree)?, match_tree, case.pattern))
                }
                _ => None,
            })
            .unwrap();
        let TreeKind::Apply(application) = &parsed.ast.get(pattern).kind else {
            panic!("expected extractor application");
        };
        let source_function = application.function;
        let TreeKind::Select(selection) = &parsed.ast.get(source_function).kind else {
            panic!("expected package-selected extractor");
        };
        let package_tree = selection.qualifier;
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );

        let typed_match = typer.type_expression(match_tree, context).unwrap();
        let TreeKind::Match(matching) = &typer.typed_arena.get(typed_match).kind else {
            panic!("expected typed Match");
        };
        let TreeKind::CaseDef(case) = &typer.typed_arena.get(matching.cases[0]).kind else {
            panic!("expected typed CaseDef");
        };
        let typed_pattern = case.pattern;
        let TreeKind::UnApply(unapply) = &typer.typed_arena.get(typed_pattern).kind else {
            panic!("expected typed UnApply");
        };
        assert!(unapply.patterns.is_empty());
        assert_eq!(typer.typed_arena.get(typed_pattern).ty, definitions.int);
        let unapply_symbol = typed_function_symbol(&typer, unapply.function);
        let TreeKind::Select(function) = &typer.typed_arena.get(unapply.function).kind else {
            panic!("expected selected unapply function");
        };
        let extractor_symbol = match typer
            .store
            .types
            .try_get(typer.typed_arena.get(function.qualifier).ty)
        {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) => *symbol,
            other => panic!("expected selected unapply reference, found {other:?}"),
        };
        let module_class = typer
            .source_module_class_of_object(extractor_symbol)
            .unwrap();
        assert_eq!(
            typer.store.symbols.get(unapply_symbol).owner,
            Some(module_class)
        );
        let SymbolInfo::Complete(signature) = typer.store.symbols.get(unapply_symbol).info else {
            panic!("expected completed unapply signature");
        };
        let Some(Type::Method(signature)) = typer.store.types.try_get(signature) else {
            panic!("expected unapply method");
        };
        assert_eq!(signature.result, definitions.boolean);
        let TreeKind::Select(extractor_selection) = &typer.typed_arena.get(function.qualifier).kind
        else {
            panic!("expected package-qualified extractor selection");
        };
        let typed_package = extractor_selection.qualifier;
        assert_eq!(
            typer.typed_index.get(source, package_tree),
            Some(typed_package)
        );
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(typed_package).ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(package), .. })
                if typer.store.symbols.get(*package).kind == SymbolKind::Package
        ));
        assert!(matches!(
            typer.type_expression(package_tree, context),
            Err(TyperError::UnsupportedTermReference {
                kind: SymbolKind::Package,
                ..
            })
        ));
        let typed_qualifier = typer.typed_index.get(source, source_function).unwrap();
        assert!(matches!(
            &typer.typed_arena.get(typed_qualifier).kind,
            TreeKind::Select(selection)
                if typer.store.names.resolve(selection.name.text()) == "Extractor"
        ));
        assert_eq!(typer.typed_index.get(source, pattern), Some(typed_pattern));
    }

    #[test]
    fn package_name_is_not_a_standalone_expression() {
        let source_text = "package p { object Extractor { def unapply(value: Int): Boolean = true } }; package client { class C { def read: Any = p } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let (method, rhs) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "read" =>
                {
                    Some((index.symbol_at(source, tree)?, definition.rhs?))
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

        let error = typer.type_expression(rhs, context).unwrap_err();
        assert!(matches!(
            error,
            TyperError::UnsupportedTermReference {
                kind: SymbolKind::Package,
                ..
            }
        ));
    }

    #[test]
    fn nested_package_path_is_only_a_qualifier() {
        let source_text = "package p { package q { class C; object Extractor { def unapply(value: Int): Boolean = true } } }; package client { class C { def choose(value: Int): Int = value match { case p.q.Extractor() => 1; case _ => 0 }; def read: Any = p.q; def readParen: Any = (p.q); def classAsTerm: Any = p.q.C; def classAsTermParen: Any = (p.q.C); def blockRead: Int = { p.q; 1 }; def whileRead: Unit = while true do p.q } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
        let (choose, match_tree, package_path) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "choose" =>
                {
                    let match_tree = definition.rhs?;
                    let TreeKind::Match(matching) = &parsed.ast.get(match_tree).kind else {
                        return None;
                    };
                    let TreeKind::CaseDef(case) = &parsed.ast.get(matching.cases[0]).kind else {
                        return None;
                    };
                    let TreeKind::Apply(application) = &parsed.ast.get(case.pattern).kind else {
                        return None;
                    };
                    let TreeKind::Select(selection) = &parsed.ast.get(application.function).kind
                    else {
                        return None;
                    };
                    Some((
                        index.symbol_at(source, tree)?,
                        match_tree,
                        selection.qualifier,
                    ))
                }
                _ => None,
            })
            .unwrap();
        let (mut typer, choose_context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            choose,
        );

        typer.type_expression(match_tree, choose_context).unwrap();
        let typed_path = typer.typed_index.get(source, package_path).unwrap();
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(typed_path).ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(package), .. })
                if typer.store.symbols.get(*package).kind == SymbolKind::Package
        ));

        let (read, rhs) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if typer.store.names.resolve(definition.name.as_name().text()) == "read" =>
                {
                    Some((index.symbol_at(source, tree)?, definition.rhs?))
                }
                _ => None,
            })
            .unwrap();
        let read_context = typer.expression_context_for(read).unwrap();
        assert!(matches!(
            typer.type_expression(rhs, read_context),
            Err(TyperError::UnsupportedTermReference {
                kind: SymbolKind::Package,
                ..
            })
        ));

        let (read_paren, read_paren_rhs) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if typer.store.names.resolve(definition.name.as_name().text())
                        == "readParen" =>
                {
                    Some((index.symbol_at(source, tree)?, definition.rhs?))
                }
                _ => None,
            })
            .unwrap();
        let read_paren_context = typer.expression_context_for(read_paren).unwrap();
        assert!(matches!(
            typer.type_expression(read_paren_rhs, read_paren_context),
            Err(TyperError::UnsupportedTermReference {
                kind: SymbolKind::Package,
                ..
            })
        ));

        let (class_as_term, class_rhs) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if typer.store.names.resolve(definition.name.as_name().text())
                        == "classAsTerm" =>
                {
                    Some((index.symbol_at(source, tree)?, definition.rhs?))
                }
                _ => None,
            })
            .unwrap();
        let class_context = typer.expression_context_for(class_as_term).unwrap();
        assert!(matches!(
            typer.type_expression(class_rhs, class_context),
            Err(TyperError::UnsupportedTermReference {
                kind: SymbolKind::Class,
                ..
            })
        ));

        let (class_paren, class_paren_rhs) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if typer.store.names.resolve(definition.name.as_name().text())
                        == "classAsTermParen" =>
                {
                    Some((index.symbol_at(source, tree)?, definition.rhs?))
                }
                _ => None,
            })
            .unwrap();
        let class_paren_context = typer.expression_context_for(class_paren).unwrap();
        assert!(matches!(
            typer.type_expression(class_paren_rhs, class_paren_context),
            Err(TyperError::UnsupportedTermReference {
                kind: SymbolKind::Class,
                ..
            })
        ));

        let (block_read, block_read_rhs) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if typer.store.names.resolve(definition.name.as_name().text())
                        == "blockRead" =>
                {
                    Some((index.symbol_at(source, tree)?, definition.rhs?))
                }
                _ => None,
            })
            .unwrap();
        let block_context = typer.expression_context_for(block_read).unwrap();
        assert!(matches!(
            typer.type_expression(block_read_rhs, block_context),
            Err(TyperError::UnsupportedTermReference {
                kind: SymbolKind::Package,
                ..
            })
        ));

        let (while_read, while_read_rhs) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if typer.store.names.resolve(definition.name.as_name().text())
                        == "whileRead" =>
                {
                    Some((index.symbol_at(source, tree)?, definition.rhs?))
                }
                _ => None,
            })
            .unwrap();
        let while_context = typer.expression_context_for(while_read).unwrap();
        assert!(matches!(
            typer.type_expression(while_read_rhs, while_context),
            Err(TyperError::UnsupportedTermReference {
                kind: SymbolKind::Package,
                ..
            })
        ));
    }

    fn typed_function_symbol(typer: &SourceTyper<'_>, function: TreeId<Typed>) -> SymbolId {
        match typer
            .store
            .types
            .try_get(typer.typed_arena.get(function).ty)
        {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) => *symbol,
            other => panic!("expected symbol-backed selected function, found {other:?}"),
        }
    }

    #[test]
    fn selected_extractor_qualifier_errors_are_focused_and_transaction_clean() {
        let (error, store_rolled_back, typed_state_rolled_back) = type_match_error(
            "class C { def choose(value: Any): Int = value match { case unknown.Missing(_) => 1; case _ => 0 } }",
        );
        assert!(matches!(error, TyperError::TermNameNotFound { .. }));
        assert!(store_rolled_back && typed_state_rolled_back);

        let (error, store_rolled_back, typed_state_rolled_back) = type_match_error(
            "object extractors {}; class C { def choose(value: Any): Int = value match { case extractors.Missing(_) => 1; case _ => 0 } }",
        );
        assert!(matches!(
            error,
            TyperError::ExtractorQualifierMemberNotFound { .. }
        ));
        assert!(store_rolled_back && typed_state_rolled_back);

        let (error, store_rolled_back, typed_state_rolled_back) = type_match_error(
            "object extractors { def Missing(): Any = 1; def Missing(value: Int): Any = value }; class C { def choose(value: Any): Int = value match { case extractors.Missing(_) => 1; case _ => 0 } }",
        );
        assert!(matches!(
            error,
            TyperError::ExtractorQualifierMemberAmbiguous { .. }
        ));
        assert!(store_rolled_back && typed_state_rolled_back);

        let (error, store_rolled_back, typed_state_rolled_back) = type_match_error(
            "object extractors { var SomeInt: Any = 1 }; class C { def choose(value: Any): Int = value match { case extractors.SomeInt(_) => 1; case _ => 0 } }",
        );
        assert!(matches!(
            error,
            TyperError::ExtractorQualifierNotStable { .. }
        ));
        assert!(store_rolled_back && typed_state_rolled_back);
    }

    #[test]
    fn extractor_binding_uses_get_type_and_is_visible_to_guard_and_body() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object SomeInt { def unapply(value: Any): MaybeInt = value }; class C { def positive(value: Int): Boolean = true; def choose(value: Any): Int = value match { case SomeInt(x) if positive(x) => x; case _ => 0 } }",
        );
        let method = method_symbol(&parsed, &store, &index, source);
        let match_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::Match(_)).then_some(tree))
            .unwrap();
        let (source_pattern, source_binder, unapply_symbol) = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::CaseDef(case_def) => {
                    let TreeKind::Apply(application) = &parsed.ast.get(case_def.pattern).kind
                    else {
                        return None;
                    };
                    let unapply_symbol =
                        parsed
                            .ast
                            .iter()
                            .find_map(|(tree, node)| match &node.kind {
                                TreeKind::DefDef(definition)
                                    if store.names.resolve(definition.name.as_name().text())
                                        == "unapply" =>
                                {
                                    index.symbol_at(source, tree)
                                }
                                _ => None,
                            })?;
                    Some((case_def.pattern, application.args[0], unapply_symbol))
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
        let typed_match_id = typer.type_expression(match_tree, context).unwrap();
        assert_eq!(
            typer.type_expression(match_tree, context).unwrap(),
            typed_match_id
        );
        let TreeKind::Match(typed_match) = &typer.typed_arena.get(typed_match_id).kind else {
            panic!("expected typed Match");
        };
        let TreeKind::CaseDef(typed_case) = &typer.typed_arena.get(typed_match.cases[0]).kind
        else {
            panic!("expected typed CaseDef");
        };
        let TreeKind::UnApply(typed_unapply) = &typer.typed_arena.get(typed_case.pattern).kind
        else {
            panic!("expected typed UnApply");
        };
        assert_eq!(
            typer.typed_arena.get(typed_case.pattern).ty,
            definitions.any_type
        );
        assert!(typed_case.guard.is_some());
        assert_eq!(typed_unapply.implicits.len(), 0);
        assert_eq!(typed_unapply.patterns.len(), 1);
        assert_eq!(
            typer.typed_index.get(source, source_pattern),
            Some(typed_case.pattern)
        );
        let binder = typer
            .pattern_binding_symbol_at(source, source_binder)
            .unwrap();
        assert_eq!(
            typer.store.symbols.get(binder).info,
            SymbolInfo::Complete(definitions.int)
        );
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(typed_unapply.function).ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. })
                if *symbol == unapply_symbol
        ));
    }

    #[test]
    fn literal_typed_and_nested_option_like_extractors_type_recursively() {
        let source_text = "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; class MaybeAny { def isEmpty: Boolean = false; def get: Any = 1 }; object SomeInt { def unapply(value: Any): MaybeInt = value }; object Outer { def unapply(value: Any): MaybeAny = value }; class C { def literal(value: Any): Int = value match { case SomeInt(1) => 1; case _ => 0 }; def typed(value: Any): Int = value match { case SomeInt(x: Int) => x; case _ => 0 }; def nested(value: Any): Int = value match { case Outer(SomeInt(x)) => x; case _ => 0 } }";
        let (parsed, mut store, packages, definitions, index, source) = setup(source_text);

        for method_name in ["literal", "typed", "nested"] {
            let (method, match_tree) = parsed
                .ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::DefDef(definition)
                        if store.names.resolve(definition.name.as_name().text()) == method_name =>
                    {
                        let match_tree = definition.rhs?;
                        if !matches!(parsed.ast.get(match_tree).kind, TreeKind::Match(_)) {
                            return None;
                        }
                        Some((index.symbol_at(source, tree)?, match_tree))
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
            assert!(
                typer.type_expression(match_tree, context).is_ok(),
                "{method_name}"
            );
        }
    }

    #[test]
    fn option_like_result_protocol_and_pattern_arity_fail_with_focused_errors() {
        let cases = [
            (
                "class NoGet { def isEmpty: Boolean = false }; object Extractor { def unapply(value: Any): NoGet = value }; class C { def choose(value: Any): Int = value match { case Extractor(_) => 1; case _ => 0 } }",
                0,
            ),
            (
                "class BadIsEmpty { def isEmpty: Int = 0; def get: Int = 1 }; object Extractor { def unapply(value: Any): BadIsEmpty = value }; class C { def choose(value: Any): Int = value match { case Extractor(_) => 1; case _ => 0 } }",
                1,
            ),
            (
                "class BadGet { def isEmpty: Boolean = false; def get(index: Int): Int = index }; object Extractor { def unapply(value: Any): BadGet = value }; class C { def choose(value: Any): Int = value match { case Extractor(_) => 1; case _ => 0 } }",
                4,
            ),
            (
                "class EmptyClauseGet { def isEmpty(): Boolean = false; def get(): Int = 1 }; object Extractor { def unapply(value: Any): EmptyClauseGet = value }; class C { def choose(value: Any): Int = value match { case Extractor(_) => 1; case _ => 0 } }",
                5,
            ),
            (
                "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object Extractor { def unapply(value: Any): MaybeInt = value }; class C { def choose(value: Any): Int = value match { case Extractor() => 1; case _ => 0 } }",
                2,
            ),
            (
                "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object Extractor { def unapply(value: Any): MaybeInt = value }; class C { def choose(value: Any): Int = value match { case Extractor(_, _) => 1; case _ => 0 } }",
                3,
            ),
        ];
        for (source_text, expected) in cases {
            let (error, store_rolled_back, typed_state_rolled_back) = type_match_error(source_text);
            assert!(
                match (expected, error) {
                    (0, TyperError::ExtractorResultMemberNotFound { .. }) => true,
                    (
                        1,
                        TyperError::ExtractorResultMemberUnsupported {
                            issue: ExtractorResultMemberIssue::IsEmptyNotBoolean,
                            ..
                        },
                    ) => true,
                    (
                        4 | 5,
                        TyperError::ExtractorResultMemberUnsupported {
                            issue: ExtractorResultMemberIssue::NotParameterless,
                            ..
                        },
                    ) => true,
                    (2, TyperError::ExtractorPatternArityUnsupported { actual: 0, .. }) => true,
                    (3, TyperError::ExtractorPatternArityUnsupported { actual: 2, .. }) => true,
                    (_, other) => panic!("unexpected Option-like extractor error: {other:?}"),
                },
                "case {expected}"
            );
            assert!(store_rolled_back);
            assert!(typed_state_rolled_back);
        }
    }

    #[test]
    fn unresolved_nested_tuple_pattern_rolls_back_the_whole_match() {
        let source_text = "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object SomeInt { def unapply(value: Any): MaybeInt = value }; class C { def choose(value: Any): Int = value match { case SomeInt((left, right)) => 1; case _ => 0 } }";
        let (error, store_rolled_back, typed_state_rolled_back) = type_match_error(source_text);
        assert!(matches!(
            error,
            TyperError::TuplePatternResolutionDeferred {
                issue: TuplePatternResolutionIssue::TupleClassNotFound,
                ..
            }
        ));
        assert!(store_rolled_back);
        assert!(typed_state_rolled_back);
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
            .type_of_tpt_inner_journaled(parameter_tpt, selector_context, &mut Vec::new())
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
    fn typed_pattern_defers_path_dependent_and_applied_type_prefixes() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class Outer[A] { class Inner }; class C { def choose(value: Any): Int = value match { case _: Outer.Inner => 1 } }",
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
        let mut outer = None;
        let mut inner = None;
        for (tree, node) in parsed.ast.iter() {
            if let TreeKind::TypeDef(definition) = &node.kind {
                match store.names.resolve(definition.name.as_name().text()) {
                    "Outer" => outer = index.symbol_at(source, tree),
                    "Inner" => inner = index.symbol_at(source, tree),
                    _ => {}
                }
            }
        }
        let outer = outer.unwrap();
        let inner = inner.unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let generic_outer = typer
            .store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, outer));
        let applied_outer = typer.store.types.alloc(Type::Applied {
            tycon: generic_outer,
            args: vec![definitions.int],
        });
        let applied_inner = typer
            .store
            .types
            .alloc(Type::type_ref(applied_outer, inner));
        assert!(
            !typer
                .typed_pattern_runtime_test_supported(applied_inner, &mut Vec::new())
                .unwrap()
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
    fn literal_alternatives_keep_order_join_their_types_and_reuse_identity() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case 1 | 2 => 0 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::Alternative(_)).then_some(tree))
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
        let TreeKind::Alternative(alternative) = &typer.typed_arena.get(typed).kind else {
            panic!("expected typed alternative")
        };
        assert_eq!(alternative.alternatives.len(), 2);
        assert!(matches!(
            typer.typed_arena.get(alternative.alternatives[0]).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Int(1)
            })
        ));
        assert!(matches!(
            typer.typed_arena.get(alternative.alternatives[1]).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Int(2)
            })
        ));
        assert_eq!(typer.typed_arena.get(typed).ty, definitions.int);
        assert_eq!(typer.typed_index.get(source, pattern), Some(typed));
        let TreeKind::Alternative(source_alternative) = &parsed.ast.get(pattern).kind else {
            panic!("expected source alternative")
        };
        for (source_branch, typed_branch) in source_alternative
            .alternatives
            .iter()
            .zip(alternative.alternatives.iter())
        {
            assert_eq!(
                typer.typed_index.get(source, *source_branch),
                Some(*typed_branch)
            );
        }

        let repeated = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.int, context, journal, mappings)
            })
            .unwrap();
        assert_eq!(repeated, typed);
    }

    #[test]
    fn stable_value_alternatives_keep_the_source_branch_order() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class C { val First: Int = 1; val Second: Int = 2; def choose(x: Int): Int = x match { case First | Second => 0 } }",
        );
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::Alternative(_)).then_some(tree))
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
        let TreeKind::Alternative(alternative) = &typer.typed_arena.get(typed).kind else {
            panic!("expected typed alternative")
        };
        assert!(matches!(
            typer.typed_arena.get(alternative.alternatives[0]).kind,
            TreeKind::Ident(Ident { name, .. }) if typer.store.names.resolve(name.text()) == "First"
        ));
        assert!(matches!(
            typer.typed_arena.get(alternative.alternatives[1]).kind,
            TreeKind::Ident(Ident { name, .. }) if typer.store.names.resolve(name.text()) == "Second"
        ));
        assert_eq!(typer.typed_arena.get(typed).ty, definitions.int);
    }

    #[test]
    fn bindings_in_alternative_branches_are_rejected_without_partial_state() {
        for source_text in [
            "class C { def choose(x: Int): Int = x match { case item | 1 => 0 } }",
            "class C { def choose(x: Int): Int = x match { case item @ _ | 1 => 0 } }",
            "class C { def choose(x: Any): Int = x match { case Some(item) | 1 => 0 } }",
        ] {
            let (parsed, mut store, packages, definitions, index, source) = setup(source_text);
            let method = method_symbol(&parsed, &store, &index, source);
            let pattern = parsed
                .ast
                .iter()
                .find_map(|(tree, node)| {
                    matches!(node.kind, TreeKind::Alternative(_)).then_some(tree)
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
                matches!(
                    error,
                    TyperError::PatternBindingInAlternative {
                        tree_index,
                        ..
                    } if tree_index == pattern.index()
                ),
                "unexpected error: {error:?}"
            );
            assert!(typer.typed_arena.iter().next().is_none());
            assert!(typer.typed_index.is_empty());
            assert!(typer.pattern_bindings.by_tree.is_empty());
        }
    }

    #[test]
    fn failed_later_alternative_branch_rolls_back_earlier_branch_mapping() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case 1 | true => 0 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::Alternative(_)).then_some(tree))
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
            Err(TyperError::PatternTypeMismatch { .. })
        ));
        assert!(typer.typed_arena.iter().next().is_none());
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn parenthesized_alternatives_are_typed_transparently() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case (1 | 2) => 0 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_tree, node)| match &node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
                _ => None,
            })
            .unwrap();
        let alternative = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::Alternative(_)).then_some(tree))
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
            TreeKind::Alternative(_)
        ));
        assert_eq!(typer.typed_index.get(source, alternative), Some(typed));
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
