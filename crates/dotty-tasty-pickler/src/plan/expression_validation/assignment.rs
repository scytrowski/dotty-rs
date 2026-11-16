use super::*;

impl Validator<'_, '_> {
    pub(crate) fn validate_mutable_counter_reference_assignment_fragment(
        &mut self,
        declaration: crate::MutableCounterLocalInitializerBinding,
        assignment: crate::MutableCounterReferenceAssignmentBinding,
    ) -> Result<(), PickleError> {
        let p = self.plan;
        let source_block = self.source_tree(declaration.source_block)?;
        let typed_block =
            p.typed_ast
                .try_get(declaration.typed_block)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: declaration.typed_block,
                })?;
        let (TreeKind::Block(source_block), TreeKind::Block(typed_block)) =
            (&source_block.kind, &typed_block.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: declaration.source_block,
                expected: SourceRole::Body,
            });
        };
        if assignment.source_id != p.source.id
            || assignment.owner != p.class
            || assignment.method != declaration.method
            || assignment.scope != declaration.scope
            || assignment.source_block != declaration.source_block
            || assignment.typed_block != declaration.typed_block
            || assignment.local != declaration.local
            || !declaration.prefix_locals.contains(&assignment.second_local)
            || assignment.rhs_local != assignment.second_local
            || assignment.rhs_local == assignment.local
            || source_block.stats.get(3) != Some(&assignment.source_assignment)
            || typed_block.stats.get(3) != Some(&assignment.typed_assignment)
            || source_block
                .stats
                .iter()
                .filter(|&&stat| {
                    matches!(
                        self.source_tree(stat).map(|tree| &tree.kind),
                        Ok(TreeKind::Assign(_))
                    )
                })
                .count()
                != 1
        {
            return Err(
                self.unsupported(UnsupportedFeature::BodyExpression, Some(assignment.method))
            );
        }
        let assignment_source = self.source_tree(assignment.source_assignment)?;
        let assignment_typed = p.typed_ast.try_get(assignment.typed_assignment).ok_or(
            PickleError::InvalidTypedTree {
                tree: assignment.typed_assignment,
            },
        )?;
        let (TreeKind::Assign(source_assign), TreeKind::Assign(typed_assign)) =
            (&assignment_source.kind, &assignment_typed.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: assignment.source_assignment,
                expected: SourceRole::Body,
            });
        };
        if source_assign.lhs != assignment.source_lhs
            || source_assign.rhs != assignment.source_rhs
            || typed_assign.lhs != assignment.typed_lhs
            || typed_assign.rhs != assignment.typed_rhs
        {
            return Err(PickleError::BodyBinding {
                source: p.source.id,
                tree: assignment.source_assignment,
                expected: assignment.typed_assignment,
                actual: p
                    .typed_bindings
                    .typed_at(p.source.id, assignment.source_assignment),
            });
        }
        let before = |tree: TreeId<Untyped>| {
            source_block
                .stats
                .iter()
                .position(|&stat| stat == tree)
                .zip(
                    source_block
                        .stats
                        .iter()
                        .position(|&stat| stat == assignment.source_assignment),
                )
                .is_some_and(|(declaration, assignment)| declaration < assignment)
        };
        if !before(declaration.prefix_source_declarations[0])
            || !before(declaration.prefix_source_declarations[1])
            || !before(declaration.source_declaration)
        {
            return Err(
                self.unsupported(UnsupportedFeature::BodyExpression, Some(assignment.method))
            );
        }
        self.validate_assignment_statement(
            assignment.source_assignment,
            assignment.typed_assignment,
            self.plan
                .methods
                .iter()
                .find(|method| method.symbol == assignment.method)
                .ok_or(PickleError::SourceDefinition {
                    symbol: assignment.method,
                })?,
            0,
            &mut HashSet::new(),
        )
    }

    pub(super) fn validate_assignment_statement(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<(), PickleError> {
        let depth_limit = self.limits.expression_depth_limit();
        if depth > depth_limit {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: depth_limit,
            });
        }
        self.charge(1)?;
        if !visited.insert(typed_id) {
            return Err(PickleError::RepeatedExpression { tree: typed_id });
        }

        let source = self.source_tree(source_id)?;
        let typed = self
            .plan
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        self.source_typed_binding(source_id, typed_id)?;
        self.paired_source_typed_span(source.position, typed.position, typed_id)?;
        let (TreeKind::Assign(source_assign), TreeKind::Assign(typed_assign)) =
            (&source.kind, &typed.kind)
        else {
            return Err(PickleError::SourceShape {
                tree: source_id,
                expected: SourceRole::Body,
            });
        };
        if typed.ty != self.plan.definitions.unit {
            return Err(PickleError::BodyType {
                tree: typed_id,
                ty: typed.ty,
            });
        }

        for (source_child, typed_child) in [
            (source_assign.lhs, typed_assign.lhs),
            (source_assign.rhs, typed_assign.rhs),
        ] {
            let actual = self
                .plan
                .typed_bindings
                .typed_at(self.plan.source.id, source_child);
            if actual != Some(typed_child) {
                return Err(PickleError::BodyBinding {
                    source: self.plan.source.id,
                    tree: source_child,
                    expected: typed_child,
                    actual,
                });
            }
        }

        let lhs =
            self.plan
                .typed_ast
                .try_get(typed_assign.lhs)
                .ok_or(PickleError::InvalidTypedTree {
                    tree: typed_assign.lhs,
                })?;
        let Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(target),
        } = self.ty(lhs.ty)?
        else {
            return Err(PickleError::BodyType {
                tree: typed_assign.lhs,
                ty: lhs.ty,
            });
        };
        let target = *target;
        let assignment_binding = self.mutable_counter_assignment;
        let target_type = if self.registered_local_symbols.contains(&target) {
            let complete_type = self.complete(target)?;
            let authenticated_counter_target = assignment_binding.is_some_and(|binding| {
                binding.source_assignment == source_id
                    && binding.typed_assignment == typed_id
                    && binding.local == target
                    && binding.local_type == complete_type
                    && binding.method == method.symbol
            });
            if !self.visible_locals.contains(&target)
                || self.symbol(target)?.flags != SymbolFlags::MUTABLE
                || (complete_type != self.plan.definitions.int && !authenticated_counter_target)
                || *prefix != self.plan.definitions.no_prefix
            {
                return Err(PickleError::Reference {
                    symbol: target,
                    reason: crate::ReferenceError::NotRegistered,
                });
            }
            complete_type
        } else if self.class_members.is_some()
            && self.plan.store.symbols.contains(target)
            && self.plan.store.symbols.get(target).kind == SymbolKind::Field
        {
            let source_lhs = self.source_tree(source_assign.lhs)?;
            let typed_lhs = self.plan.typed_ast.try_get(typed_assign.lhs).ok_or(
                PickleError::InvalidTypedTree {
                    tree: typed_assign.lhs,
                },
            )?;
            let (TreeKind::Ident(source_ident), TreeKind::Ident(typed_ident)) =
                (&source_lhs.kind, &typed_lhs.kind)
            else {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            };
            if source_ident.name != typed_ident.name
                || source_ident.backquoted != typed_ident.backquoted
            {
                return Err(PickleError::SourceShape {
                    tree: source_assign.lhs,
                    expected: SourceRole::Body,
                });
            }
            let field = self
                .class_members
                .and_then(|members| {
                    members.iter().find_map(|member| match member {
                        ClassMemberPicklePlan::Field(field) if field.symbol == target => {
                            Some(*field)
                        }
                        _ => None,
                    })
                })
                .ok_or(PickleError::Reference {
                    symbol: target,
                    reason: crate::ReferenceError::NotRegistered,
                })?;
            if self.symbol(target)?.flags != SymbolFlags::MUTABLE {
                return Err(PickleError::Reference {
                    symbol: target,
                    reason: crate::ReferenceError::Metadata,
                });
            }
            let field_type = self.complete(target)?;
            if field_type != field.declared_type
                || (field_type != self.plan.definitions.int
                    && field_type != self.plan.definitions.boolean)
            {
                return Err(PickleError::UnsupportedTypeReference { ty: field_type });
            }
            self.same_class_field_target(
                target,
                source_ident.name,
                *prefix,
                typed_assign.lhs,
                field_type,
            )?;
            field_type
        } else {
            return Err(PickleError::Reference {
                symbol: target,
                reason: crate::ReferenceError::NotRegistered,
            });
        };

        self.validate_method_expression_inner(
            Some(source_assign.lhs),
            Some(typed_assign.lhs),
            method,
            target_type,
            depth,
            (Some(visited), false),
        )?;
        if target_type == self.plan.definitions.int {
            self.validate_int_expression(
                source_assign.rhs,
                typed_assign.rhs,
                method,
                depth,
                visited,
            )
        } else if target_type == self.plan.definitions.boolean {
            self.validate_if_condition(source_assign.rhs, typed_assign.rhs, method, depth, visited)
        } else {
            self.validate_mutable_counter_assignment_rhs(
                assignment_binding.ok_or(PickleError::Reference {
                    symbol: target,
                    reason: crate::ReferenceError::NotRegistered,
                })?,
                source_assign.rhs,
                typed_assign.rhs,
                method,
                visited,
            )
        }
    }

    fn validate_mutable_counter_assignment_rhs(
        &mut self,
        binding: crate::MutableCounterReferenceAssignmentBinding,
        source_rhs: TreeId<Untyped>,
        typed_rhs: TreeId<Typed>,
        method: &MethodPicklePlan,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<(), PickleError> {
        let p = self.plan;
        let source = self.source_tree(source_rhs)?;
        let typed = p
            .typed_ast
            .try_get(typed_rhs)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_rhs })?;
        let (TreeKind::Ident(source_ident), TreeKind::Ident(typed_ident)) =
            (&source.kind, &typed.kind)
        else {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        };
        let local = self.symbol(binding.rhs_local)?;
        let local_type = self.complete(binding.rhs_local)?;
        let Type::TermRef { prefix, target } = self.ty(typed.ty)? else {
            return Err(PickleError::BodyType {
                tree: typed_rhs,
                ty: typed.ty,
            });
        };
        let TermRefTarget::Symbol(rhs_target) = target else {
            return Err(PickleError::BodyType {
                tree: typed_rhs,
                ty: typed.ty,
            });
        };
        let binding_local = p
            .local_values
            .iter()
            .find(|candidate| candidate.symbol == binding.rhs_local);
        let same_local_type = self.same_reference_type(local_type, binding.local_type)?;
        if binding.source_rhs != source_rhs
            || binding.typed_rhs != typed_rhs
            || binding.method != method.symbol
            || binding.rhs_local != binding.second_local
            || *rhs_target != binding.second_local
            || *prefix != p.definitions.no_prefix
            || !same_local_type
            || local.kind != SymbolKind::Local
            || local.owner != Some(method.symbol)
            || local.origin != SymbolOrigin::Source(p.source.id)
            || local.flags != SymbolFlags::EMPTY
            || !self.visible_locals.contains(&binding.rhs_local)
            || binding_local.is_none_or(|candidate| {
                candidate.scope != binding.scope || candidate.source_tree == binding.source_block
            })
            || source_ident.name != local.name
            || typed_ident.name != local.name
        {
            return Err(self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol)));
        }
        self.source_typed_binding(source_rhs, typed_rhs)?;
        self.paired_source_typed_span(source.position, typed.position, typed_rhs)?;
        if !visited.insert(typed_rhs) {
            return Err(PickleError::RepeatedExpression { tree: typed_rhs });
        }
        Ok(())
    }
}
