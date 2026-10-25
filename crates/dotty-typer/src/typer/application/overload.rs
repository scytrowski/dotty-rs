//! Candidate discovery, filtering, and selection for callable overloads.

use super::super::*;
use dotty_core::types::MethodParam;
use dotty_core::{Namespace, Symbol, SymbolLinks, TermName};

#[derive(Clone, Copy)]
enum ApplicationFunctionShape {
    Ident(Ident),
    Select {
        selection: dotty_core::ast::Select<Untyped>,
        qualifier: TreeId<Typed>,
        receiver_type: TypeId,
        allow_mutable_local_receiver: bool,
    },
}

fn supported_override_signature(method: &MethodType, store: &SemanticStore) -> bool {
    method.kind == MethodKind::Plain
        && method.params.iter().all(|parameter| {
            !parameter.erased
                && !parameter.varargs
                && !matches!(store.types.try_get(parameter.ty), Some(Type::ByName { .. }))
        })
}

impl SourceTyper<'_> {
    pub(in crate::typer) fn resolve_infix_application_function(
        &mut self,
        request: InfixApplicationRequest,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<ResolvedApplicationFunction, TyperError> {
        let InfixApplicationRequest {
            operator,
            qualifier,
            receiver_type,
            argument_tree,
            context,
            tree_index,
            position,
        } = request;
        if !operator.is_term() {
            return Err(TyperError::InfixOperatorMustBeTerm {
                source: self.source,
                tree_index,
                operator,
            });
        }
        let receiver = self.widen_expression_type_journaled(receiver_type, info_journal, 0)?;
        let receiver = if self.any_equality_operator_name(operator) {
            self.dealias_source_aliases(receiver, info_journal)?
        } else {
            receiver
        };
        if let Err(error) = self.require_stable_selection_prefix(receiver_type, tree_index) {
            // A left-associated infix chain selects on the result of the
            // preceding application. Its widened nominal type is enough to
            // resolve a member without retaining a dependent term prefix.
            if !matches!(
                self.store.types.try_get(receiver),
                Some(Type::TypeRef { .. } | Type::Applied { .. })
            ) {
                return Err(error);
            }
        }
        let receiver = self.this_type_receiver_view(receiver)?;
        let mut candidates = if self.any_ref_identity_operator_name(operator) {
            let Some(symbol) = self.any_ref_identity_symbol_for_receiver(receiver, operator) else {
                return Err(TyperError::MemberNotFound {
                    source: self.source,
                    tree_index,
                    receiver,
                    name: operator,
                });
            };
            let source_members = self
                .lookup_overload_members_journaled(receiver, operator, info_journal)
                .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
            // A same-spelled source or trait declaration cannot stand in for
            // the compiler's final AnyRef identity symbol.
            if !source_members.is_empty() {
                return Err(TyperError::MemberNotFound {
                    source: self.source,
                    tree_index,
                    receiver,
                    name: operator,
                });
            }
            let SymbolInfo::Complete(callable) = self.store.symbols.get(symbol).info else {
                return Err(TyperError::MemberNotFound {
                    source: self.source,
                    tree_index,
                    receiver,
                    name: operator,
                });
            };
            self.validate_overload_callable(symbol, callable)?;
            vec![ApplicationCandidate {
                symbol,
                callable,
                member: None,
                rejection: None,
            }]
        } else if self.any_equality_operator_name(operator)
            && self.is_canonical_any_receiver(receiver)
        {
            let Some(symbol) = self.any_equality_operator_symbol(operator) else {
                return Err(TyperError::MemberNotFound {
                    source: self.source,
                    tree_index,
                    receiver,
                    name: operator,
                });
            };
            let SymbolInfo::Complete(callable) = self.store.symbols.get(symbol).info else {
                return Err(TyperError::MemberNotFound {
                    source: self.source,
                    tree_index,
                    receiver,
                    name: operator,
                });
            };
            self.validate_overload_callable(symbol, callable)?;
            vec![ApplicationCandidate {
                symbol,
                callable,
                member: None,
                rejection: None,
            }]
        } else {
            let mut members = self
                .lookup_overload_members_journaled(receiver, operator, info_journal)
                .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
            if members.is_empty()
                && self.store.names.resolve(operator.text()) == "+"
                && let Some(string_class) = self.canonical_java_lang_string_class(receiver)
                && self.ensure_java_lang_string_plus(string_class, receiver)?
            {
                members = self
                    .lookup_overload_members_journaled(receiver, operator, info_journal)
                    .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
            }
            if members.is_empty() && !self.any_equality_operator_name(operator) {
                return Err(TyperError::MemberNotFound {
                    source: self.source,
                    tree_index,
                    receiver,
                    name: operator,
                });
            }
            if members
                .iter()
                .any(|member| self.store.symbols.get(member.symbol).kind != SymbolKind::Method)
            {
                return Err(TyperError::MixedApplicationCandidateKinds {
                    source: self.source,
                    tree_index,
                    candidates: members.iter().map(|member| member.symbol).collect(),
                });
            }
            let mut candidates = members
                .into_iter()
                .map(|member| {
                    let callable = self.member_type_on_journaled(&member, info_journal)?;
                    self.validate_overload_callable(member.symbol, callable)?;
                    Ok(ApplicationCandidate {
                        symbol: member.symbol,
                        callable,
                        member: Some(member),
                        rejection: None,
                    })
                })
                .collect::<Result<Vec<_>, TyperError>>()?;
            if self.any_equality_operator_name(operator) {
                let Some(symbol) = self.any_equality_operator_symbol(operator) else {
                    return Err(TyperError::MemberNotFound {
                        source: self.source,
                        tree_index,
                        receiver,
                        name: operator,
                    });
                };
                let SymbolInfo::Complete(callable) = self.store.symbols.get(symbol).info else {
                    return Err(TyperError::MemberNotFound {
                        source: self.source,
                        tree_index,
                        receiver,
                        name: operator,
                    });
                };
                self.validate_overload_callable(symbol, callable)?;
                candidates.push(ApplicationCandidate {
                    symbol,
                    callable,
                    member: None,
                    rejection: None,
                });
            }
            candidates
        };

        let typed =
            self.type_value_expression_inner(argument_tree, context, info_journal, new_mappings)?;
        let own_type = self.typed_arena.get(typed).ty;
        let widened_type = self.widen_expression_type_journaled(own_type, info_journal, 0)?;
        let widened_type =
            if candidates.len() == 1 && self.is_any_ref_identity_symbol(candidates[0].symbol) {
                self.any_ref_identity_argument_view(widened_type)
                    .unwrap_or(widened_type)
            } else {
                widened_type
            };
        let widened_type = if self.any_equality_operator_name(operator) {
            self.dealias_source_aliases(widened_type, info_journal)?
        } else {
            widened_type
        };
        let arguments = [TypedArgument {
            typed,
            own_type,
            widened_type,
        }];

        let mut completed_relation_types = std::collections::HashSet::new();
        self.complete_relation_type(widened_type, info_journal, &mut completed_relation_types, 0)?;
        for candidate in &candidates {
            if let Some(Type::Method(method)) = self.store.types.try_get(candidate.callable) {
                let parameter_types: Vec<_> = method.params.iter().map(|param| param.ty).collect();
                for parameter_type in parameter_types {
                    self.complete_relation_type(
                        parameter_type,
                        info_journal,
                        &mut completed_relation_types,
                        0,
                    )?;
                    if self.any_equality_operator_name(operator) {
                        self.dealias_source_aliases(parameter_type, info_journal)?;
                    }
                }
            }
        }

        let universal_candidate = if self.any_equality_operator_name(operator) {
            candidates
                .iter()
                .position(|candidate| self.is_any_equality_symbol(candidate.symbol))
                .map(|index| candidates.remove(index))
        } else {
            None
        };
        self.remove_overridden_overload_candidates(&mut candidates, tree_index)?;
        if let Some(universal_candidate) = universal_candidate {
            candidates.push(universal_candidate);
        }
        let winner = if self.any_equality_operator_name(operator) {
            let universal_candidate = candidates
                .iter()
                .position(|candidate| self.is_any_equality_symbol(candidate.symbol))
                .map(|index| candidates.remove(index));
            if let Some(universal_candidate) = universal_candidate {
                if candidates.is_empty() {
                    self.require_compatible_universal_equality(
                        receiver,
                        widened_type,
                        tree_index,
                        info_journal,
                    )?;
                    universal_candidate
                } else {
                    match self.choose_method_overload_candidate(
                        &mut candidates,
                        &arguments,
                        tree_index,
                        ApplyKind::Regular,
                        info_journal,
                    ) {
                        Ok(winner) => winner,
                        Err(TyperError::OverloadApplicationNoApplicable { .. }) => {
                            self.require_compatible_universal_equality(
                                receiver,
                                widened_type,
                                tree_index,
                                info_journal,
                            )?;
                            universal_candidate
                        }
                        Err(error) => return Err(error),
                    }
                }
            } else {
                self.choose_method_overload_candidate(
                    &mut candidates,
                    &arguments,
                    tree_index,
                    ApplyKind::Regular,
                    info_journal,
                )?
            }
        } else {
            self.choose_method_overload_candidate(
                &mut candidates,
                &arguments,
                tree_index,
                ApplyKind::Regular,
                info_journal,
            )?
        };
        if winner
            .member
            .is_some_and(|member| member.symbol != winner.symbol)
        {
            return Err(TyperError::MalformedOverloadCandidate {
                symbol: winner.symbol,
                callable: winner.callable,
            });
        }
        let function_type = self.store.types.alloc(Type::TermRef {
            prefix: receiver_type,
            target: TermRefTarget::Symbol(winner.symbol),
        });
        let function = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).select(
            qualifier,
            operator,
            false,
            function_type,
            position,
        );
        Ok(ResolvedApplicationFunction {
            typed: function,
            callable: winner.callable,
            arguments: arguments.into(),
            argument_trees: None,
        })
    }

    /// Returns the class identity for the JDK's `java.lang.String`, without
    /// accepting a same-spelled source or TASTy class as the compiler intrinsic.
    fn canonical_java_lang_string_class(&self, receiver: TypeId) -> Option<SymbolId> {
        let Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(class),
            ..
        }) = self.store.types.try_get(receiver)
        else {
            return None;
        };
        if !self.store.symbols.contains(*class) {
            return None;
        }
        let string = self.store.symbols.get(*class);
        if string.kind != SymbolKind::Class
            || !matches!(string.origin, SymbolOrigin::Classfile(_))
            || !string.name.is_type()
            || self.store.names.resolve(string.name.text()) != "String"
        {
            return None;
        }
        let lang = string.owner?;
        if !self.store.symbols.contains(lang) {
            return None;
        }
        let lang_symbol = self.store.symbols.get(lang);
        if lang_symbol.kind != SymbolKind::Package
            || !lang_symbol.name.is_term()
            || self.store.names.resolve(lang_symbol.name.text()) != "lang"
        {
            return None;
        }
        let java = lang_symbol.owner?;
        if !self.store.symbols.contains(java) {
            return None;
        }
        let java_symbol = self.store.symbols.get(java);
        if java_symbol.kind != SymbolKind::Package
            || !java_symbol.name.is_term()
            || self.store.names.resolve(java_symbol.name.text()) != "java"
        {
            return None;
        }
        let root = java_symbol.owner?;
        if !self.store.symbols.contains(root) {
            return None;
        }
        let root_symbol = self.store.symbols.get(root);
        (root_symbol.kind == SymbolKind::Package
            && root_symbol.name.is_term()
            && self.store.names.resolve(root_symbol.name.text()).is_empty()
            && root_symbol.owner.is_none())
        .then_some(*class)
    }

    /// Installs Scala 3.9's compiler-synthesized `String.+(Any): String` on
    /// the already-validated JDK class when the classfile member index omits it.
    fn ensure_java_lang_string_plus(
        &mut self,
        string_class: SymbolId,
        string_type: TypeId,
    ) -> Result<bool, TyperError> {
        let name = Name::new(self.store.names.intern("+"), Namespace::Term);
        let SymbolInfo::Complete(class_info_id) = self.store.symbols.get(string_class).info else {
            return Ok(false);
        };
        let Some(Type::ClassInfo(class_info)) = self.store.types.try_get(class_info_id) else {
            return Ok(false);
        };
        if class_info.class != string_class || !self.store.scopes.contains(class_info.declarations)
        {
            return Ok(false);
        }
        let declarations = class_info.declarations;
        let declaration_scope = self.store.scopes.get(declarations);
        if declaration_scope.owner != Some(string_class) {
            return Ok(false);
        }
        if declaration_scope.lookup_all(&name).iter().any(|symbol| {
            self.store.symbols.contains(*symbol)
                && self.store.symbols.get(*symbol).kind == SymbolKind::Method
        }) {
            return Ok(false);
        }
        if !declaration_scope.lookup_all(&name).is_empty() {
            return Ok(false);
        }

        let method_info = self.store.types.alloc(Type::Method(MethodType {
            params: vec![MethodParam {
                name: TermName::new(self.store.names.intern("other")),
                ty: self.definitions.any_type,
                erased: false,
                varargs: false,
            }],
            result: string_type,
            kind: MethodKind::Plain,
        }));
        let symbol = self.store.symbols.alloc(Symbol {
            name,
            owner: Some(string_class),
            kind: SymbolKind::Method,
            flags: SymbolFlags::FINAL,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(method_info),
            origin: SymbolOrigin::Builtin,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        self.store.scopes.get_mut(declarations).enter(name, symbol);
        self.synthetic_class_member_entries
            .push((declarations, symbol));
        Ok(true)
    }

    fn require_compatible_universal_equality(
        &mut self,
        left: TypeId,
        right: TypeId,
        tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(), TyperError> {
        if self.universal_equality_operands_compatible(left, right, info_journal)? {
            Ok(())
        } else {
            Err(TyperError::IncompatibleEqualityOperands {
                source: self.source,
                tree_index,
                left,
                right,
            })
        }
    }

    pub(in crate::typer) fn resolve_overloaded_application_function(
        &mut self,
        request: ApplicationRequest<'_>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<Option<ResolvedApplicationFunction>, TyperError> {
        let ApplicationRequest {
            function_tree,
            argument_trees,
            application_kind,
            context,
            tree_index: application_tree_index,
        } = request;
        let Some(function_node) = self.arena.try_get(function_tree).cloned() else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: function_tree.index(),
            });
        };
        let (mut candidates, function_shape) = match function_node.kind {
            TreeKind::Ident(ident) => {
                let candidates = self.expression_term_candidates(
                    ident.name,
                    context,
                    function_tree.index(),
                    function_node.position,
                )?;
                if candidates.len() <= 1 {
                    return Ok(None);
                }
                if candidates
                    .iter()
                    .any(|symbol| self.store.symbols.get(*symbol).kind != SymbolKind::Method)
                {
                    return Err(TyperError::MixedApplicationCandidateKinds {
                        source: self.source,
                        tree_index: application_tree_index,
                        candidates,
                    });
                }
                (
                    candidates
                        .into_iter()
                        .map(|symbol| {
                            let callable =
                                self.completed_expression_symbol_info(symbol, info_journal)?;
                            self.validate_overload_callable(symbol, callable)?;
                            Ok(ApplicationCandidate {
                                symbol,
                                callable,
                                member: None,
                                rejection: None,
                            })
                        })
                        .collect::<Result<Vec<_>, TyperError>>()?,
                    ApplicationFunctionShape::Ident(ident),
                )
            }
            TreeKind::Select(selection) if selection.name.is_term() => {
                let qualifier_candidate = self.type_selection_qualifier(
                    selection.qualifier,
                    context,
                    function_tree.index(),
                    function_node.position,
                    info_journal,
                    new_mappings,
                )?;
                let qualifier = if let Some(qualifier) = qualifier_candidate {
                    qualifier
                } else {
                    self.type_expression_inner(
                        selection.qualifier,
                        context,
                        info_journal,
                        new_mappings,
                    )?
                };
                let receiver_type = self.typed_arena.get(qualifier).ty;
                let receiver =
                    self.widen_expression_type_journaled(receiver_type, info_journal, 0)?;
                let receiver_view = self.this_type_receiver_view(receiver)?;
                let java_class_qualifier = self.is_java_class_qualifier(qualifier);
                if !java_class_qualifier && self.any_ref_identity_operator_name(selection.name) {
                    let Some(symbol) =
                        self.any_ref_identity_symbol_for_receiver(receiver_view, selection.name)
                    else {
                        return Err(TyperError::MemberNotFound {
                            source: self.source,
                            tree_index: function_tree.index(),
                            receiver: receiver_view,
                            name: selection.name,
                        });
                    };
                    let source_members = self
                        .lookup_overload_members_journaled(
                            receiver_view,
                            selection.name,
                            info_journal,
                        )
                        .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
                    // Do not let an overload set hide a source declaration
                    // that shadows the compiler-owned AnyRef identity member.
                    if !source_members.is_empty() {
                        return Err(TyperError::MemberNotFound {
                            source: self.source,
                            tree_index: function_tree.index(),
                            receiver: receiver_view,
                            name: selection.name,
                        });
                    }
                    let SymbolInfo::Complete(callable) = self.store.symbols.get(symbol).info else {
                        return Err(TyperError::MemberNotFound {
                            source: self.source,
                            tree_index: function_tree.index(),
                            receiver: receiver_view,
                            name: selection.name,
                        });
                    };
                    self.validate_overload_callable(symbol, callable)?;
                    let allow_mutable_local_receiver =
                        self.is_mutable_local_selection_receiver(qualifier, receiver_type);
                    if !allow_mutable_local_receiver {
                        self.require_stable_application_selection_prefix(
                            qualifier,
                            receiver_type,
                            function_tree.index(),
                        )?;
                    }
                    (
                        vec![ApplicationCandidate {
                            symbol,
                            callable,
                            member: None,
                            rejection: None,
                        }],
                        ApplicationFunctionShape::Select {
                            selection,
                            qualifier,
                            receiver_type,
                            allow_mutable_local_receiver,
                        },
                    )
                } else {
                    let mut members = match self.lookup_overload_members_journaled(
                        receiver_view,
                        selection.name,
                        info_journal,
                    ) {
                        Ok(members) => members,
                        Err(error) => {
                            return Err(TyperError::MemberLookup(Box::new(error)));
                        }
                    };
                    if members.is_empty()
                        && !java_class_qualifier
                        && self.store.names.resolve(selection.name.text()) == "+"
                        && let Some(string_class) =
                            self.canonical_java_lang_string_class(receiver_view)
                        && self.ensure_java_lang_string_plus(string_class, receiver_view)?
                    {
                        members = match self.lookup_overload_members_journaled(
                            receiver_view,
                            selection.name,
                            info_journal,
                        ) {
                            Ok(members) => members,
                            Err(error) => {
                                return Err(TyperError::MemberLookup(Box::new(error)));
                            }
                        };
                    }
                    if java_class_qualifier {
                        members.retain(|member| {
                            self.store.symbols.contains(member.symbol)
                                && self
                                    .store
                                    .symbols
                                    .get(member.symbol)
                                    .flags
                                    .contains(SymbolFlags::STATIC)
                        });
                    }
                    if members.is_empty() {
                        if java_class_qualifier {
                            return Err(TyperError::MemberNotFound {
                                source: self.source,
                                tree_index: function_tree.index(),
                                receiver: receiver_view,
                                name: selection.name,
                            });
                        }
                        if let Some(resolved) = self.resolve_local_extension_application(
                            selection,
                            qualifier,
                            argument_trees,
                            application_kind,
                            context,
                            function_tree,
                            application_tree_index,
                            info_journal,
                            new_mappings,
                        )? {
                            return Ok(Some(resolved));
                        }
                        self.require_stable_application_selection_prefix(
                            qualifier,
                            receiver_type,
                            function_tree.index(),
                        )?;
                        return Ok(None);
                    }
                    if members.len() == 1 {
                        // Only a call to one already-applicable method may read
                        // a mutable local receiver; selecting a member as a value
                        // still requires the ordinary stable-prefix check.
                        let mutable_local_method_call =
                            if self.store.symbols.get(members[0].symbol).kind == SymbolKind::Method
                            {
                                let applicability = self.single_member_application_applicability(
                                    &members[0],
                                    argument_trees,
                                    context,
                                    application_kind,
                                    info_journal,
                                    new_mappings,
                                )?;
                                if applicability == Some(false)
                                    && let Some(resolved) = self
                                        .resolve_local_extension_application(
                                            selection,
                                            qualifier,
                                            argument_trees,
                                            application_kind,
                                            context,
                                            function_tree,
                                            application_tree_index,
                                            info_journal,
                                            new_mappings,
                                        )?
                                {
                                    return Ok(Some(resolved));
                                }
                                applicability == Some(true)
                                    && self.is_mutable_local_selection_receiver(
                                        qualifier,
                                        receiver_type,
                                    )
                            } else {
                                false
                            };
                        if !mutable_local_method_call && !java_class_qualifier {
                            self.require_stable_application_selection_prefix(
                                qualifier,
                                receiver_type,
                                function_tree.index(),
                            )?;
                            return Ok(None);
                        }
                    }
                    if members.iter().any(|member| {
                        self.store.symbols.get(member.symbol).kind != SymbolKind::Method
                    }) {
                        self.require_stable_application_selection_prefix(
                            qualifier,
                            receiver_type,
                            function_tree.index(),
                        )?;
                        return Err(TyperError::MixedApplicationCandidateKinds {
                            source: self.source,
                            tree_index: application_tree_index,
                            candidates: members.iter().map(|member| member.symbol).collect(),
                        });
                    }
                    let allow_mutable_local_receiver = members.len() == 1
                        && self.is_mutable_local_selection_receiver(qualifier, receiver_type);
                    let candidates = members
                        .into_iter()
                        .map(|member| {
                            let callable = self.member_type_on_journaled(&member, info_journal)?;
                            self.validate_overload_callable(member.symbol, callable)?;
                            Ok(ApplicationCandidate {
                                symbol: member.symbol,
                                callable,
                                member: Some(member),
                                rejection: None,
                            })
                        })
                        .collect::<Result<Vec<_>, TyperError>>()?;
                    (
                        candidates,
                        ApplicationFunctionShape::Select {
                            selection,
                            qualifier,
                            receiver_type,
                            allow_mutable_local_receiver,
                        },
                    )
                }
            }
            _ => return Ok(None),
        };

        let canonical_identity_application =
            candidates.len() == 1 && self.is_any_ref_identity_symbol(candidates[0].symbol);
        let mut arguments = Vec::with_capacity(argument_trees.len());
        for argument_tree in argument_trees {
            let typed = self.type_value_expression_inner(
                *argument_tree,
                context,
                info_journal,
                new_mappings,
            )?;
            let own_type = self.typed_arena.get(typed).ty;
            // String constants have exact singleton relations but no modeled
            // underlying String type to widen to. Keep the raw type until a
            // candidate's parameter type determines whether the constant is
            // needed.
            let widened_type = if canonical_identity_application {
                let widened = self.widen_expression_type_journaled(own_type, info_journal, 0)?;
                self.any_ref_identity_argument_view(widened)
                    .unwrap_or(widened)
            } else if matches!(
                self.store.types.try_get(own_type),
                Some(Type::Constant(dotty_core::Constant::String(_)))
            ) {
                own_type
            } else {
                self.widen_expression_type_journaled(own_type, info_journal, 0)?
            };
            arguments.push(TypedArgument {
                typed,
                own_type,
                widened_type,
            });
        }

        let mut completed_relation_types = std::collections::HashSet::new();
        for argument in &arguments {
            self.complete_relation_type(
                argument.widened_type,
                info_journal,
                &mut completed_relation_types,
                0,
            )?;
        }
        for candidate in &candidates {
            if let Some(Type::Method(method)) = self.store.types.try_get(candidate.callable) {
                let parameter_types: Vec<_> = method.params.iter().map(|param| param.ty).collect();
                for parameter_type in parameter_types {
                    self.complete_relation_type(
                        parameter_type,
                        info_journal,
                        &mut completed_relation_types,
                        0,
                    )?;
                }
            }
        }

        self.remove_overridden_overload_candidates(&mut candidates, application_tree_index)?;

        let overload_store_checkpoint = self.store.checkpoint();
        let overload_resolver_checkpoint = self.resolver.checkpoint();
        let overload_type_index_checkpoint = self.type_index.checkpoint();
        let overload_journal_checkpoint = info_journal.len();
        let winner = match self.choose_method_overload_candidate(
            &mut candidates,
            &arguments,
            application_tree_index,
            application_kind,
            info_journal,
        ) {
            Ok(winner) => winner,
            Err(error @ TyperError::OverloadApplicationNoApplicable { .. }) => {
                for (symbol, previous) in info_journal[overload_journal_checkpoint..]
                    .iter()
                    .rev()
                    .copied()
                {
                    if self.store.symbols.contains(symbol) {
                        self.store.symbols.set_info(symbol, previous);
                    }
                }
                info_journal.truncate(overload_journal_checkpoint);
                self.resolver
                    .rollback_to(self.store, overload_resolver_checkpoint);
                self.store.rollback_to(overload_store_checkpoint);
                self.type_index.restore(overload_type_index_checkpoint);
                if let ApplicationFunctionShape::Select {
                    selection,
                    qualifier,
                    ..
                } = &function_shape
                    && let Some(resolved) = self.resolve_local_extension_application(
                        *selection,
                        *qualifier,
                        argument_trees,
                        application_kind,
                        context,
                        function_tree,
                        application_tree_index,
                        info_journal,
                        new_mappings,
                    )?
                {
                    return Ok(Some(resolved));
                }
                if let ApplicationFunctionShape::Select {
                    qualifier,
                    receiver_type,
                    ..
                } = &function_shape
                    && !self.is_java_class_qualifier(*qualifier)
                {
                    self.require_stable_application_selection_prefix(
                        *qualifier,
                        *receiver_type,
                        function_tree.index(),
                    )?;
                }
                return Err(error);
            }
            Err(error) => {
                if let ApplicationFunctionShape::Select {
                    qualifier,
                    receiver_type,
                    ..
                } = &function_shape
                    && !self.is_java_class_qualifier(*qualifier)
                {
                    self.require_stable_application_selection_prefix(
                        *qualifier,
                        *receiver_type,
                        function_tree.index(),
                    )?;
                }
                return Err(error);
            }
        };
        let function_type = match function_shape {
            ApplicationFunctionShape::Ident(ident) => {
                let ty = self.expression_type_of_symbol(
                    winner.symbol,
                    context.owner,
                    function_tree.index(),
                    info_journal,
                )?;
                TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                    .ident_with_backquoted(ident.name, ident.backquoted, ty, function_node.position)
            }
            ApplicationFunctionShape::Select {
                selection,
                qualifier,
                receiver_type,
                allow_mutable_local_receiver,
            } => {
                if !allow_mutable_local_receiver && !self.is_java_class_qualifier(qualifier) {
                    self.require_stable_application_selection_prefix(
                        qualifier,
                        receiver_type,
                        function_tree.index(),
                    )?;
                }
                if winner
                    .member
                    .is_some_and(|member| member.symbol != winner.symbol)
                {
                    return Err(TyperError::MalformedOverloadCandidate {
                        symbol: winner.symbol,
                        callable: winner.callable,
                    });
                }
                let ty = self.store.types.alloc(Type::TermRef {
                    prefix: receiver_type,
                    target: TermRefTarget::Symbol(winner.symbol),
                });
                TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).select(
                    qualifier,
                    selection.name,
                    selection.backquoted,
                    ty,
                    function_node.position,
                )
            }
        };
        self.typed_index
            .insert(self.source, function_tree, function_type)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        new_mappings.push((self.source, function_tree));
        Ok(Some(ResolvedApplicationFunction {
            typed: function_type,
            callable: winner.callable,
            arguments,
            argument_trees: None,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_local_extension_application(
        &mut self,
        selection: dotty_core::ast::Select<Untyped>,
        qualifier: TreeId<Typed>,
        argument_trees: &[TreeId<Untyped>],
        application_kind: ApplyKind,
        context: ExpressionContext,
        function_tree: TreeId<Untyped>,
        application_tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<Option<ResolvedApplicationFunction>, TyperError> {
        let names = match self.expression_term_candidates(
            selection.name,
            context,
            function_tree.index(),
            self.arena.get(function_tree).position,
        ) {
            Ok(names) => names,
            Err(TyperError::TermNameNotFound { .. }) => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut extensions = names.into_iter().filter(|symbol| {
            self.local_methods.contains_symbol(*symbol)
                && self
                    .store
                    .symbols
                    .get(*symbol)
                    .flags
                    .contains(SymbolFlags::EXTENSION)
        });
        let Some(symbol) = extensions.next() else {
            return Ok(None);
        };
        if extensions.next().is_some() {
            return Err(TyperError::OverloadedSelectionDeferred {
                source: self.source,
                tree_index: application_tree_index,
                name: selection.name,
            });
        }
        let Some(prefix_clauses) = self.local_methods.extension_prefix_clauses(symbol) else {
            return Ok(None);
        };
        if prefix_clauses.len() != 1 || prefix_clauses[0].len() != 1 {
            return Ok(None);
        }
        if application_kind != ApplyKind::Regular {
            return Ok(None);
        }
        let signature = self.completed_expression_symbol_info(symbol, info_journal)?;
        let Some(Type::Method(receiver_clause)) = self.store.types.try_get(signature).cloned()
        else {
            return Ok(None);
        };
        if receiver_clause.kind != MethodKind::Plain || receiver_clause.params.len() != 1 {
            return Ok(None);
        }
        let Some(Type::Method(method)) = self.store.types.try_get(receiver_clause.result).cloned()
        else {
            return Ok(None);
        };
        if !supported_override_signature(&method, self.store)
            || self.type_contains_param_ref(method.result, receiver_clause.result)?
            || matches!(
                self.store.types.try_get(method.result),
                Some(Type::Method(_) | Type::Poly(_))
            )
            || argument_trees.len() != method.params.len()
        {
            return Ok(None);
        }
        let receiver_parameter = receiver_clause.params[0];
        let receiver_own_type = self.typed_arena.get(qualifier).ty;
        let receiver_widened_type =
            self.widen_expression_type_journaled(receiver_own_type, info_journal, 0)?;
        match self.conforms(receiver_widened_type, receiver_parameter.ty) {
            Ok(true) => {}
            Ok(false) => return Ok(None),
            Err(error) => {
                return Err(TyperError::ApplicationArgumentConformanceUnsupported {
                    source: self.source,
                    tree_index: application_tree_index,
                    argument_index: 0,
                    actual: receiver_widened_type,
                    expected: receiver_parameter.ty,
                    error: Box::new(error),
                });
            }
        }
        let receiver = TypedArgument {
            typed: qualifier,
            own_type: receiver_own_type,
            widened_type: receiver_widened_type,
        };
        let mut arguments = Vec::with_capacity(argument_trees.len());
        for argument_tree in argument_trees {
            let typed = self.type_value_expression_inner(
                *argument_tree,
                context,
                info_journal,
                new_mappings,
            )?;
            let own_type = self.typed_arena.get(typed).ty;
            let widened_type = self.widen_expression_type_journaled(own_type, info_journal, 0)?;
            arguments.push(TypedArgument {
                typed,
                own_type,
                widened_type,
            });
        }
        for (index, argument) in arguments.iter().enumerate() {
            let expected = method.params[index].ty;
            let actual =
                self.adapt_expression_type_to_expected(argument.own_type, expected, info_journal)?;
            match self.conforms(actual, expected) {
                Ok(true) => {}
                Ok(false) => return Ok(None),
                Err(error) => {
                    return Err(TyperError::ApplicationArgumentConformanceUnsupported {
                        source: self.source,
                        tree_index: application_tree_index,
                        argument_index: index,
                        actual,
                        expected,
                        error: Box::new(error),
                    });
                }
            }
        }
        let reference_type = self.store.types.alloc(Type::TermRef {
            prefix: self.definitions.no_prefix,
            target: TermRefTarget::Symbol(symbol),
        });
        let typed_ident = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .ident_with_backquoted(
                selection.name,
                selection.backquoted,
                reference_type,
                self.arena.get(function_tree).position,
            );
        let typed = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).apply_with_kind(
            typed_ident,
            vec![receiver.typed],
            ApplyKind::Regular,
            receiver_clause.result,
            self.arena.get(function_tree).position,
        );
        self.typed_index
            .insert(self.source, function_tree, typed)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        new_mappings.push((self.source, function_tree));
        Ok(Some(ResolvedApplicationFunction {
            typed,
            callable: receiver_clause.result,
            arguments,
            argument_trees: None,
        }))
    }

    fn single_member_application_applicability(
        &mut self,
        member: &MemberCandidate,
        argument_trees: &[TreeId<Untyped>],
        context: ExpressionContext,
        application_kind: ApplyKind,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<Option<bool>, TyperError> {
        let callable = self.member_type_on_journaled(member, info_journal)?;
        let Some(Type::Method(method)) = self.store.types.try_get(callable).cloned() else {
            return Ok(None);
        };
        if !application_kind_accepts(application_kind, method.kind) {
            return Ok(Some(false));
        }
        if argument_trees.len() != method.params.len() {
            return Ok(Some(false));
        }
        if method.params.iter().any(|parameter| {
            parameter.erased
                || parameter.varargs
                || matches!(
                    self.store.types.try_get(parameter.ty),
                    Some(Type::ByName { .. })
                )
        }) || self.type_contains_param_ref(method.result, callable)?
        {
            return Ok(None);
        }
        for (tree, parameter) in argument_trees.iter().zip(&method.params) {
            let typed =
                self.type_value_expression_inner(*tree, context, info_journal, new_mappings)?;
            let actual = self.adapt_expression_type_to_expected(
                self.typed_arena.get(typed).ty,
                parameter.ty,
                info_journal,
            )?;
            match self.conforms(actual, parameter.ty) {
                Ok(true) => {}
                Ok(false) => return Ok(Some(false)),
                Err(_) => return Ok(None),
            }
        }
        Ok(Some(true))
    }

    pub(in crate::typer) fn choose_method_overload_candidate(
        &mut self,
        candidates: &mut [ApplicationCandidate],
        arguments: &[TypedArgument],
        tree_index: u32,
        application_kind: ApplyKind,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<ApplicationCandidate, TyperError> {
        let store_checkpoint = self.store.checkpoint();
        let resolver_checkpoint = self.resolver.checkpoint();
        let type_index_checkpoint = self.type_index.checkpoint();
        let journal_checkpoint = info_journal.len();
        let original_candidates = candidates.to_vec();

        let resolution = (|| {
            self.instantiate_generic_overload_candidates(
                candidates,
                arguments,
                tree_index,
                application_kind,
                info_journal,
            )?;
            self.choose_overload_candidate(candidates, arguments, tree_index, application_kind)
        })();
        let selected = resolution?;

        for (symbol, previous) in info_journal[journal_checkpoint..].iter().rev().copied() {
            if self.store.symbols.contains(symbol) {
                self.store.symbols.set_info(symbol, previous);
            }
        }
        info_journal.truncate(journal_checkpoint);
        self.resolver.rollback_to(self.store, resolver_checkpoint);
        self.store.rollback_to(store_checkpoint);
        self.type_index.restore(type_index_checkpoint);
        candidates.copy_from_slice(&original_candidates);

        let selected_index = candidates
            .iter()
            .position(|candidate| candidate.symbol == selected.symbol)
            .ok_or(TyperError::MalformedOverloadCandidate {
                symbol: selected.symbol,
                callable: selected.callable,
            })?;
        self.instantiate_generic_overload_candidates(
            &mut candidates[selected_index..=selected_index],
            arguments,
            tree_index,
            application_kind,
            info_journal,
        )?;
        Ok(candidates[selected_index])
    }

    pub(in crate::typer) fn choose_constructor_overload_candidate(
        &mut self,
        candidates: &mut [ApplicationCandidate],
        arguments: &[TypedArgument],
        tree_index: u32,
        application_kind: ApplyKind,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<ApplicationCandidate, TyperError> {
        let store_checkpoint = self.store.checkpoint();
        let resolver_checkpoint = self.resolver.checkpoint();
        let type_index_checkpoint = self.type_index.checkpoint();
        let journal_checkpoint = info_journal.len();
        let original_candidates = candidates.to_vec();

        let resolution = (|| {
            self.instantiate_generic_overload_candidates(
                candidates,
                arguments,
                tree_index,
                application_kind,
                info_journal,
            )?;
            self.choose_overload_candidate_with_policy(
                candidates,
                arguments,
                tree_index,
                application_kind,
                true,
            )
        })();
        let selected = resolution?;

        for (symbol, previous) in info_journal[journal_checkpoint..].iter().rev().copied() {
            if self.store.symbols.contains(symbol) {
                self.store.symbols.set_info(symbol, previous);
            }
        }
        info_journal.truncate(journal_checkpoint);
        self.resolver.rollback_to(self.store, resolver_checkpoint);
        self.store.rollback_to(store_checkpoint);
        self.type_index.restore(type_index_checkpoint);
        candidates.copy_from_slice(&original_candidates);

        let selected_index = candidates
            .iter()
            .position(|candidate| candidate.symbol == selected.symbol)
            .ok_or(TyperError::MalformedOverloadCandidate {
                symbol: selected.symbol,
                callable: selected.callable,
            })?;
        self.instantiate_generic_overload_candidates(
            &mut candidates[selected_index..=selected_index],
            arguments,
            tree_index,
            application_kind,
            info_journal,
        )?;
        Ok(candidates[selected_index])
    }

    pub(in crate::typer) fn instantiate_generic_overload_candidates(
        &mut self,
        candidates: &mut [ApplicationCandidate],
        arguments: &[TypedArgument],
        tree_index: u32,
        application_kind: ApplyKind,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(), TyperError> {
        for candidate in candidates {
            let Some(Type::Poly(poly)) = self.store.types.try_get(candidate.callable).cloned()
            else {
                continue;
            };
            let Some(Type::Method(method)) = self.store.types.try_get(poly.result).cloned() else {
                continue;
            };
            if !application_kind_accepts(application_kind, method.kind) {
                candidate.rejection = Some(OverloadRejection::ApplicationKindMismatch {
                    application_kind,
                    method_kind: method.kind,
                });
                continue;
            }
            if overload_arity_rejection(&method, arguments.len()).is_some() {
                continue;
            }

            let type_arguments = match self.infer_poly_application_arguments(
                candidate.callable,
                &poly,
                arguments,
                application_kind,
                tree_index,
                info_journal,
            ) {
                Ok(type_arguments) => type_arguments,
                Err(TyperError::ApplicationArgumentTypeMismatch {
                    argument_index,
                    actual,
                    expected,
                    ..
                }) => {
                    candidate.rejection = Some(OverloadRejection::ArgumentNonConformance {
                        argument_index,
                        actual,
                        expected,
                    });
                    continue;
                }
                Err(TyperError::ConflictingInferenceConstraints {
                    parameter_index, ..
                }) => {
                    candidate.rejection = Some(OverloadRejection::TypeArgumentInferenceFailure {
                        parameter_index: Some(parameter_index),
                    });
                    continue;
                }
                Err(TyperError::ApplicationArityMismatch { .. }) => continue,
                Err(
                    TyperError::UnconstrainedTypeParameter { .. }
                    | TyperError::UnsupportedInferenceShape { .. }
                    | TyperError::UnsupportedPolymorphicApplicationShape { .. },
                ) => continue,
                Err(error) => return Err(error),
            };
            let instantiated = dotty_core::types::instantiate_poly(
                self.store,
                candidate.callable,
                &type_arguments,
            )
            .map_err(TyperError::PolyInstantiation)?;

            let mut supported_bounds = true;
            for (parameter_index, (argument, bounds)) in type_arguments
                .iter()
                .copied()
                .zip(instantiated.bounds.iter().copied())
                .enumerate()
            {
                match self.check_explicit_type_argument_bounds(
                    argument,
                    bounds,
                    parameter_index,
                    tree_index,
                    info_journal,
                ) {
                    Ok(()) => {}
                    Err(TyperError::ExplicitTypeArgumentBoundViolation { side, .. }) => {
                        candidate.rejection = Some(OverloadRejection::TypeArgumentBoundViolation {
                            parameter_index,
                            side,
                        });
                        supported_bounds = false;
                        break;
                    }
                    Err(
                        TyperError::UnsupportedExplicitTypeArgumentBounds { .. }
                        | TyperError::ExplicitTypeArgumentBoundCheckUnsupported { .. },
                    ) => {
                        supported_bounds = false;
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
            if supported_bounds {
                candidate.callable = instantiated.result;
            }
        }
        Ok(())
    }

    pub(in crate::typer) fn validate_overload_callable(
        &self,
        symbol: SymbolId,
        callable: TypeId,
    ) -> Result<(), TyperError> {
        match self.store.types.try_get(callable) {
            Some(Type::Method(_) | Type::Poly(_)) => Ok(()),
            _ => Err(TyperError::MalformedOverloadCandidate { symbol, callable }),
        }
    }

    pub(in crate::typer) fn complete_relation_type(
        &mut self,
        ty: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        seen: &mut std::collections::HashSet<TypeId>,
        depth: usize,
    ) -> Result<(), TyperError> {
        if depth >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            return Err(TyperError::TypeNormalization(
                crate::types::TypeNormalizeError::TooDeep,
            ));
        }
        if !seen.insert(ty) {
            return Ok(());
        }
        let Some(node) = self.store.types.try_get(ty).cloned() else {
            return Err(TyperError::TypeNormalization(
                if self.store.types.contains(ty) {
                    crate::types::TypeNormalizeError::UnfilledType { ty }
                } else {
                    crate::types::TypeNormalizeError::InvalidType { ty }
                },
            ));
        };
        let mut children = Vec::new();
        match node {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                prefix,
            } => {
                if !self.store.symbols.contains(symbol) {
                    return Err(TyperError::UnknownSymbol { symbol });
                }
                children.push(prefix);
                if matches!(
                    self.store.symbols.get(symbol).kind,
                    SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                ) && self.is_current_source_symbol(symbol)
                    && matches!(*self.store.symbols.info(symbol), SymbolInfo::Missing)
                {
                    self.complete_symbol_inner(symbol, info_journal)?;
                }
                if let SymbolInfo::Complete(info) = *self.store.symbols.info(symbol)
                    && let Some(Type::ClassInfo(class_info)) = self.store.types.try_get(info)
                {
                    children.extend(class_info.parents.iter().copied());
                }
            }
            Type::Applied { tycon, args } => {
                children.push(tycon);
                children.extend(args);
            }
            Type::TermRef { prefix, .. } => children.push(prefix),
            Type::SuperType {
                this_type,
                super_type,
            } => children.extend([this_type, super_type]),
            Type::Bounds { low, high } => children.extend([low, high]),
            Type::AliasingBounds { alias }
            | Type::ByName { result: alias }
            | Type::Flexible { underlying: alias }
            | Type::Recursive { parent: alias }
            | Type::Wildcard { bounds: alias }
            | Type::JavaArray { element: alias }
            | Type::Repeated { element: alias }
            | Type::Annotated {
                underlying: alias, ..
            } => children.push(alias),
            Type::And { left, right } | Type::Or { left, right } => {
                children.extend([left, right]);
            }
            _ => {}
        }
        for child in children {
            self.complete_relation_type(child, info_journal, seen, depth + 1)?;
        }
        Ok(())
    }

    pub(in crate::typer) fn remove_overridden_overload_candidates(
        &mut self,
        candidates: &mut Vec<ApplicationCandidate>,
        tree_index: u32,
    ) -> Result<(), TyperError> {
        let mut overridden = vec![false; candidates.len()];
        for derived_index in 0..candidates.len() {
            let Some(derived_member) = candidates[derived_index].member else {
                continue;
            };
            let Some(Type::Method(derived_method)) = self
                .store
                .types
                .try_get(candidates[derived_index].callable)
                .cloned()
            else {
                continue;
            };
            if !supported_override_signature(&derived_method, self.store)
                || self.type_contains_param_ref(
                    derived_method.result,
                    candidates[derived_index].callable,
                )?
            {
                continue;
            }

            for base_index in 0..candidates.len() {
                if base_index == derived_index || overridden[base_index] {
                    continue;
                }
                let Some(base_member) = candidates[base_index].member else {
                    continue;
                };
                if derived_member.declaring_class == base_member.declaring_class {
                    continue;
                }
                let Some(Type::Method(base_method)) = self
                    .store
                    .types
                    .try_get(candidates[base_index].callable)
                    .cloned()
                else {
                    continue;
                };
                if !supported_override_signature(&base_method, self.store)
                    || self.type_contains_param_ref(
                        base_method.result,
                        candidates[base_index].callable,
                    )?
                    || derived_method.params.len() != base_method.params.len()
                {
                    continue;
                }

                let mut same_parameters = true;
                for (derived, base) in derived_method.params.iter().zip(&base_method.params) {
                    let derived_subtype = self.is_subtype(derived.ty, base.ty).map_err(|_| {
                        TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                            source: self.source,
                            tree_index,
                            candidate: candidates[derived_index].symbol,
                        }
                    })?;
                    if !derived_subtype {
                        same_parameters = false;
                        break;
                    }
                    let base_subtype = self.is_subtype(base.ty, derived.ty).map_err(|_| {
                        TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                            source: self.source,
                            tree_index,
                            candidate: candidates[base_index].symbol,
                        }
                    })?;
                    if !base_subtype {
                        same_parameters = false;
                        break;
                    }
                }
                if !same_parameters {
                    continue;
                }

                let derived_view = derived_member.receiver_view;
                let base_view = base_member.receiver_view;
                let derived_is_subtype =
                    self.is_subtype(derived_view, base_view).map_err(|_| {
                        TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                            source: self.source,
                            tree_index,
                            candidate: candidates[derived_index].symbol,
                        }
                    })?;
                if !derived_is_subtype {
                    continue;
                }
                let base_is_subtype = self.is_subtype(base_view, derived_view).map_err(|_| {
                    TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                        source: self.source,
                        tree_index,
                        candidate: candidates[base_index].symbol,
                    }
                })?;
                if !base_is_subtype {
                    overridden[base_index] = true;
                }
            }
        }
        let mut index = 0;
        candidates.retain(|_| {
            let keep = !overridden[index];
            index += 1;
            keep
        });
        Ok(())
    }

    pub(in crate::typer) fn choose_overload_candidate(
        &mut self,
        candidates: &mut [ApplicationCandidate],
        arguments: &[TypedArgument],
        tree_index: u32,
        application_kind: ApplyKind,
    ) -> Result<ApplicationCandidate, TyperError> {
        self.choose_overload_candidate_with_policy(
            candidates,
            arguments,
            tree_index,
            application_kind,
            false,
        )
    }

    pub(in crate::typer) fn choose_overload_candidate_with_policy(
        &mut self,
        candidates: &mut [ApplicationCandidate],
        arguments: &[TypedArgument],
        tree_index: u32,
        application_kind: ApplyKind,
        retain_unsupported_rejections: bool,
    ) -> Result<ApplicationCandidate, TyperError> {
        let mut applicable = Vec::new();
        let mut rejected = Vec::new();
        let mut unsupported_candidates = Vec::new();
        for candidate in candidates.iter().copied() {
            if let Some(rejection) = candidate.rejection {
                rejected.push((candidate.symbol, rejection));
                continue;
            }
            let method = match self.store.types.try_get(candidate.callable) {
                Some(Type::Method(method)) => method.clone(),
                Some(Type::Poly(poly)) => {
                    let rejection = match self.store.types.try_get(poly.result) {
                        Some(Type::Method(method))
                            if let Some(expected) =
                                overload_arity_rejection(method, arguments.len()) =>
                        {
                            OverloadRejection::WrongArity {
                                expected,
                                actual: arguments.len(),
                            }
                        }
                        _ => {
                            if retain_unsupported_rejections {
                                rejected.push((
                                    candidate.symbol,
                                    OverloadRejection::UnsupportedSemantics,
                                ));
                                unsupported_candidates.push(candidate.symbol);
                                continue;
                            }
                            return Err(
                                TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                                    source: self.source,
                                    tree_index,
                                    candidate: candidate.symbol,
                                },
                            );
                        }
                    };
                    rejected.push((candidate.symbol, rejection));
                    continue;
                }
                _ => {
                    return Err(TyperError::MalformedOverloadCandidate {
                        symbol: candidate.symbol,
                        callable: candidate.callable,
                    });
                }
            };
            if !application_kind_accepts(application_kind, method.kind) {
                rejected.push((
                    candidate.symbol,
                    OverloadRejection::ApplicationKindMismatch {
                        application_kind,
                        method_kind: method.kind,
                    },
                ));
                continue;
            }
            if let Some(expected) = overload_arity_rejection(&method, arguments.len()) {
                rejected.push((
                    candidate.symbol,
                    OverloadRejection::WrongArity {
                        expected,
                        actual: arguments.len(),
                    },
                ));
                continue;
            }
            let mut argument_rejection = None;
            let mut unsupported_relation = false;
            for (argument_index, (argument, parameter)) in
                arguments.iter().zip(&method.params).enumerate()
            {
                let actual = self.adapt_pre_widened_expression_type_to_expected(
                    argument.own_type,
                    argument.widened_type,
                    parameter.ty,
                )?;
                match self.conforms(actual, parameter.ty) {
                    Ok(true) => {}
                    Ok(false) => {
                        argument_rejection = Some(OverloadRejection::ArgumentNonConformance {
                            argument_index,
                            actual,
                            expected: parameter.ty,
                        });
                        break;
                    }
                    Err(_) => {
                        unsupported_relation = true;
                        break;
                    }
                }
            }
            if let Some(rejection) = argument_rejection {
                rejected.push((candidate.symbol, rejection));
                continue;
            }
            if unsupported_relation {
                if retain_unsupported_rejections {
                    rejected.push((candidate.symbol, OverloadRejection::UnsupportedSemantics));
                    unsupported_candidates.push(candidate.symbol);
                    continue;
                }
                return Err(TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                    source: self.source,
                    tree_index,
                    candidate: candidate.symbol,
                });
            }
            let unsupported = method.params.iter().any(|param| {
                param.erased
                    || param.varargs
                    || matches!(
                        self.store.types.try_get(param.ty),
                        Some(Type::ByName { .. })
                    )
            }) || self
                .type_contains_param_ref(method.result, candidate.callable)?;
            if unsupported {
                if retain_unsupported_rejections {
                    rejected.push((candidate.symbol, OverloadRejection::UnsupportedSemantics));
                    unsupported_candidates.push(candidate.symbol);
                    continue;
                }
                return Err(TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                    source: self.source,
                    tree_index,
                    candidate: candidate.symbol,
                });
            }
            applicable.push(candidate);
        }
        if !unsupported_candidates.is_empty()
            && (!applicable.is_empty() || !retain_unsupported_rejections)
        {
            return Err(TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                source: self.source,
                tree_index,
                candidate: unsupported_candidates[0],
            });
        }
        match applicable.as_slice() {
            [candidate] => Ok(*candidate),
            [] => Err(TyperError::OverloadApplicationNoApplicable {
                source: self.source,
                tree_index,
                candidates: rejected,
            }),
            many => {
                let mut most_specific = Vec::new();
                for candidate in many {
                    let Some(Type::Method(method)) = self.store.types.try_get(candidate.callable)
                    else {
                        return Err(TyperError::MalformedOverloadCandidate {
                            symbol: candidate.symbol,
                            callable: candidate.callable,
                        });
                    };
                    let candidate_params: Vec<_> =
                        method.params.iter().map(|parameter| parameter.ty).collect();
                    let mut dominates_all = true;
                    for other in many {
                        if candidate.symbol == other.symbol {
                            continue;
                        }
                        let Some(Type::Method(other_method)) =
                            self.store.types.try_get(other.callable)
                        else {
                            return Err(TyperError::MalformedOverloadCandidate {
                                symbol: other.symbol,
                                callable: other.callable,
                            });
                        };
                        let other_params: Vec<_> = other_method
                            .params
                            .iter()
                            .map(|parameter| parameter.ty)
                            .collect();
                        let mut strictly_more_specific = false;
                        for (candidate_parameter, other_parameter) in
                            candidate_params.iter().zip(&other_params)
                        {
                            let candidate_is_subtype = self
                                .is_subtype(*candidate_parameter, *other_parameter)
                                .map_err(|_| {
                                    TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                                        source: self.source,
                                        tree_index,
                                        candidate: candidate.symbol,
                                    }
                                })?;
                            if !candidate_is_subtype {
                                dominates_all = false;
                                break;
                            }
                            let other_is_subtype = self
                                .is_subtype(*other_parameter, *candidate_parameter)
                                .map_err(|_| {
                                    TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                                        source: self.source,
                                        tree_index,
                                        candidate: other.symbol,
                                    }
                                })?;
                            strictly_more_specific |= !other_is_subtype;
                        }
                        if !dominates_all || !strictly_more_specific {
                            dominates_all = false;
                            break;
                        }
                    }
                    if dominates_all {
                        most_specific.push(*candidate);
                    }
                }
                match most_specific.as_slice() {
                    [candidate] => Ok(*candidate),
                    _ => Err(TyperError::AmbiguousOverloadApplication {
                        source: self.source,
                        tree_index,
                        candidates: many.iter().map(|candidate| candidate.symbol).collect(),
                    }),
                }
            }
        }
    }
}

fn overload_arity_rejection(method: &MethodType, actual: usize) -> Option<usize> {
    match method.params.iter().position(|parameter| parameter.varargs) {
        Some(varargs_index) if varargs_index + 1 == method.params.len() => {
            (actual < varargs_index).then_some(varargs_index)
        }
        Some(_) => None,
        None => (actual != method.params.len()).then_some(method.params.len()),
    }
}
