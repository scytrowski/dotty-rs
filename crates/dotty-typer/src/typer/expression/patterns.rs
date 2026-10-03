//! Pattern typing entry points and pattern-specific type adaptation.

use super::{ExpressionContext, SourceTyper, TyperError};
use crate::typer::{
    ExtractorMethodShapeIssue, ExtractorPatternArgumentIssue, ExtractorProductIssue,
    ExtractorResultMemberIssue, PatternKind, TuplePatternResolutionIssue,
};
use dotty_core::ast::{TreeKind, TypedAstBuilder, UntypedNode};
use dotty_core::types::{MethodKind, MethodType, Type};
use dotty_core::{
    Name, Namespace, SemanticStore, SourceId, SymbolFlags, SymbolId, SymbolInfo, SymbolKind,
    SymbolOrigin, TermRefTarget, TreeId, TypeId, TypeRefTarget, Typed, Untyped,
};

const MAX_REIFIABLE_TYPE_PREFIX_DEPTH: usize = 64;

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
    fn extractor_result_member_type(
        &mut self,
        result_type: TypeId,
        member_name: Name,
        pattern_index: u32,
        unapply: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let members =
            match self.lookup_overload_members_journaled(result_type, member_name, info_journal) {
                Ok(members) => members,
                Err(crate::typer::lookup::MemberLookupError::ClassInfoUnavailable { .. })
                | Err(crate::typer::lookup::MemberLookupError::UnsupportedReceiverType {
                    ..
                }) => {
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
        self.remove_overridden_overload_candidates(&mut candidates, pattern_index)?;
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
        let value_type = match self.store.types.try_get(callable) {
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
            return Err(self.tuple_pattern_resolution_error(
                pattern,
                arity,
                TuplePatternResolutionIssue::EmptyTupleNeedsUnitRule,
            ));
        }

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
        let tuple_object = match scoped_companion {
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
        let receiver = self.widen_expression_type_journaled(companion_type, info_journal, 0)?;
        let receiver = self.this_type_receiver_view(receiver)?;
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
        let Some(scala_package) = self.packages.symbol(&["scala"]) else {
            return Ok(false);
        };
        let Some(scope) = self.packages.scope_of(scala_package) else {
            return Ok(false);
        };
        let product_name = Name::new(self.store.names.intern("Product"), Namespace::Type);
        let products = self.store.scopes.get(scope).lookup_all(&product_name);
        let [product] = products else {
            return Ok(false);
        };
        let product = *product;
        if !matches!(
            self.store.symbols.get(product).kind,
            SymbolKind::Class | SymbolKind::Trait
        ) {
            return Ok(false);
        }
        let prefix = self.type_symbol_prefix(product);
        let product_type = self.store.types.alloc(Type::type_ref(prefix, product));
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
        let method = match self.store.types.try_get(callable) {
            Some(Type::Poly(_)) => {
                return Err(TyperError::ExtractorUnapplyPolymorphic {
                    source: self.source,
                    tree_index: pattern.index(),
                    unapply,
                    callable,
                });
            }
            Some(Type::Method(method)) => method.clone(),
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
                callable,
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
        let conforms = self.conforms(selector_type, input_type);
        let unapply_type = match conforms {
            Ok(true) if selector_type == self.definitions.nothing_type => input_type,
            Ok(true) => selector_type,
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
        Ok(ExtractorPlan {
            function,
            symbol: unapply,
            input_type,
            result_type,
            unapply_type,
            source_patterns: application.args.clone(),
        })
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
                            if self
                                .product_extractor_component_types(
                                    plan.result_type,
                                    pattern.index(),
                                    plan.symbol,
                                    1,
                                    true,
                                    info_journal,
                                )?
                                .is_some()
                            {
                                return Err(TyperError::ExtractorPatternArityUnsupported {
                                    source: self.source,
                                    tree_index: pattern.index(),
                                    unapply: plan.symbol,
                                    actual: 1,
                                });
                            }
                            let component_type = self.option_like_extractor_component_type(
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
