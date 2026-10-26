//! TASTy emission for the validated private primitive class-state slice.

use dotty_core::ast::{DefParamClauseOrder, TreeKind};
use dotty_core::{MethodKind, SymbolId, SymbolInfo, SymbolKind, SymbolOrigin, Type};
use dotty_tasty::tasty::*;

use crate::skeleton::Assembly;
use crate::{
    ClassMemberPicklePlan, EncodingContext, LimitKind, MethodPicklePlan, PickleError, PickleLimits,
    PickledTasty, SkeletonDeclaration, TypeRole, UnsupportedFeature, ValidatedClassStatePlan,
};

fn encoding(error: impl Into<TastyFileError>) -> PickleError {
    PickleError::Encoding {
        cause: Box::new(error.into()),
    }
}

fn marker(context: &mut EncodingContext<'_>, tag: u8) -> Result<AstLabel, PickleError> {
    let term = SimpleTerm::new(tag, TermValue::Tag).map_err(encoding)?;
    context.wire_node(AstNode::Leaf(term))
}

fn complete_type(
    store: &dotty_core::SemanticStore,
    symbol: SymbolId,
) -> Result<dotty_core::TypeId, PickleError> {
    if !store.symbols.contains(symbol) {
        return Err(PickleError::InvalidSymbol { symbol });
    }
    let record = store.symbols.get(symbol);
    match record.info {
        SymbolInfo::Complete(ty) => Ok(ty),
        info => Err(PickleError::IncompleteSymbol { symbol, info }),
    }
}

pub(crate) fn package_path(
    store: &dotty_core::SemanticStore,
    package: SymbolId,
    max_depth: usize,
) -> Result<String, PickleError> {
    let mut components = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut at = Some(package);
    while let Some(symbol) = at {
        if components.len() >= max_depth {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::OwnerDepth,
                limit: max_depth,
            });
        }
        if !seen.insert(symbol) {
            return Err(PickleError::OwnerCycle { symbol });
        }
        if !store.symbols.contains(symbol) {
            return Err(PickleError::InvalidSymbol { symbol });
        }
        let record = store.symbols.get(symbol);
        if record.kind != SymbolKind::Package || !record.name.is_term() {
            return Err(PickleError::Owner {
                symbol,
                expected: None,
                actual: record.owner,
            });
        }
        let text = store
            .names
            .try_resolve(record.name.text())
            .ok_or(PickleError::InvalidName {
                name: record.name.text(),
            })?;
        match record.owner {
            Some(_) => {
                if text.is_empty() || text.contains(['.', '/', '\\', '\0']) {
                    return Err(PickleError::Unsupported {
                        feature: UnsupportedFeature::Name,
                        symbol: Some(symbol),
                    });
                }
                components.push(crate::artifact_name::encode_component(text));
            }
            None if text.is_empty() && !components.is_empty() => break,
            None => {
                return Err(PickleError::Owner {
                    symbol,
                    expected: None,
                    actual: None,
                });
            }
        }
        at = record.owner;
    }
    components.reverse();
    Ok(components.join("/"))
}

pub(crate) fn intended_class_tasty_path(
    p: &crate::ClassStatePicklePlan<'_>,
    max_owner_depth: usize,
) -> Result<String, PickleError> {
    if !p.store.symbols.contains(p.class) {
        return Err(PickleError::InvalidSymbol { symbol: p.class });
    }
    let class = p.store.symbols.get(p.class);
    let package = class.owner.ok_or(PickleError::Owner {
        symbol: p.class,
        expected: None,
        actual: None,
    })?;
    let package_text = package_path(p.store, package, max_owner_depth)?;
    let class_text =
        p.store
            .names
            .try_resolve(class.name.text())
            .ok_or(PickleError::InvalidName {
                name: class.name.text(),
            })?;
    Ok(format!(
        "{package_text}/{}.tasty",
        crate::artifact_name::encode_component(class_text)
    ))
}

fn method_input(
    p: &crate::ClassStatePicklePlan<'_>,
    symbol: SymbolId,
    source_tree: dotty_core::TreeId<dotty_core::Untyped>,
) -> Result<(MethodPicklePlan, Vec<crate::LocalValueBinding>), PickleError> {
    let TreeKind::DefDef(definition) = &p
        .source_ast
        .try_get(source_tree)
        .ok_or(PickleError::InvalidSourceTree { tree: source_tree })?
        .kind
    else {
        return Err(PickleError::SourceShape {
            tree: source_tree,
            expected: crate::SourceRole::Method,
        });
    };
    let parameter_clauses = definition
        .value_param_clauses
        .iter()
        .map(|clause| {
            clause
                .iter()
                .map(|&tree| {
                    let param = p
                        .source_index
                        .symbol_at(p.source.id, tree)
                        .ok_or(PickleError::SourceDefinition { symbol })?;
                    Ok(param)
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let body = definition
        .rhs
        .map(|rhs| {
            p.typed_bindings
                .typed_at(p.source.id, rhs)
                .ok_or(PickleError::MissingBody { method: symbol })
        })
        .transpose()?;
    let method = MethodPicklePlan {
        symbol,
        source_tree,
        parameter_clauses,
        body,
    };
    let local_values = p.typed_bindings.local_value_bindings(p.source.id, symbol);
    Ok((method, local_values))
}

pub(crate) fn validate_class_state_method_bodies(
    p: &crate::ClassStatePicklePlan<'_>,
) -> Result<(), PickleError> {
    let method_inputs = p
        .members
        .iter()
        .filter_map(|member| match *member {
            ClassMemberPicklePlan::Method {
                symbol,
                source_tree,
            } => Some((symbol, source_tree)),
            ClassMemberPicklePlan::Field(_) => None,
        })
        .map(|(symbol, source_tree)| method_input(p, symbol, source_tree))
        .collect::<Result<Vec<_>, _>>()?;
    let methods = method_inputs
        .iter()
        .map(|(method, _)| method.clone())
        .collect::<Vec<_>>();
    let local_values = method_inputs
        .iter()
        .flat_map(|(_, locals)| locals.iter().copied())
        .collect::<Vec<_>>();
    crate::plan::validate_class_state_method(p, &methods, &local_values)
}

impl ValidatedClassStatePlan<'_, '_> {
    /// Validate every method's source and typed body against the complete
    /// class-state TASTy subset without emitting bytes.
    pub fn validate_method_bodies(&self) -> Result<(), PickleError> {
        validate_class_state_method_bodies(self.plan())
    }

    /// Emit complete Scala 3.9 TASTy for the validated private primitive fields.
    pub fn pickle(&self) -> Result<PickledTasty, PickleError> {
        self.pickle_with_limits(PickleLimits::default())
    }

    /// Apply the standard projection and whole-file limits to class-state output.
    pub fn pickle_with_limits(&self, limits: PickleLimits) -> Result<PickledTasty, PickleError> {
        let output = self.assemble_class_state(limits)?;
        Ok(PickledTasty::from_assembly(output))
    }

    fn assemble_class_state(&self, limits: PickleLimits) -> Result<Assembly, PickleError> {
        let p = self.plan();
        let constructor_tree = p.source_ast.try_get(p.constructor.source_tree).ok_or(
            PickleError::InvalidSourceTree {
                tree: p.constructor.source_tree,
            },
        )?;
        let TreeKind::DefDef(constructor_definition) = &constructor_tree.kind else {
            return Err(PickleError::SourceShape {
                tree: p.constructor.source_tree,
                expected: crate::SourceRole::Constructor,
            });
        };
        let mut template_parameters = Vec::new();
        let mut constructor_parameters = Vec::new();
        for (source_clause, symbols) in constructor_definition
            .value_param_clauses
            .iter()
            .zip(&p.constructor.parameter_clauses)
        {
            for (&tree, &parameter) in source_clause.iter().zip(symbols) {
                let template_parameter = p
                    .source_index
                    .symbol_at(p.source.id, tree)
                    .ok_or(PickleError::SourceDefinition { symbol: parameter })?;
                template_parameters.push((template_parameter, parameter));
                constructor_parameters.push(parameter);
            }
        }

        let mut method_inputs = Vec::new();
        for member in p.members {
            if let ClassMemberPicklePlan::Method {
                symbol,
                source_tree,
            } = *member
            {
                method_inputs.push(method_input(p, symbol, source_tree)?);
            }
        }
        let methods = method_inputs
            .iter()
            .map(|(method, _)| method.clone())
            .collect::<Vec<_>>();
        let local_values = method_inputs
            .iter()
            .flat_map(|(_, locals)| locals.iter().copied())
            .collect::<Vec<_>>();
        crate::plan::validate_class_state_method(p, &methods, &local_values)?;

        let mut context = EncodingContext::new(
            p.store,
            p.definitions,
            p.source_index,
            p.source.id,
            limits.projection,
        )?;
        context.reserve_declaration(p.class)?;
        for member in p.members {
            let ClassMemberPicklePlan::Method { symbol, .. } = *member else {
                continue;
            };
            let signature_id = complete_type(p.store, symbol)?;
            let parameters = match p.store.types.try_get(signature_id) {
                Some(Type::Method(signature)) => signature.params.as_slice(),
                Some(_) => &[],
                None => return Err(PickleError::InvalidType { ty: signature_id }),
            };
            let result_ty = match p.store.types.try_get(signature_id) {
                Some(Type::Method(signature)) => signature.result,
                Some(_) => signature_id,
                None => return Err(PickleError::InvalidType { ty: signature_id }),
            };
            for parameter_ty in parameters
                .iter()
                .map(|parameter| parameter.ty)
                .chain(std::iter::once(result_ty))
            {
                let Some(Type::TypeRef {
                    target: dotty_core::TypeRefTarget::Symbol(class),
                    ..
                }) = p.store.types.try_get(parameter_ty)
                else {
                    continue;
                };
                if !p.typed_bindings.planned_class_symbols().contains(class) || *class == p.class {
                    continue;
                }
                let Some(dotty_core::SourceDefinition::Canonical { source, tree }) =
                    p.source_index.definition_of(*class)
                else {
                    return Err(PickleError::SourceDefinition { symbol: *class });
                };
                if source != p.source.id {
                    return Err(PickleError::SourceDefinition { symbol: *class });
                }
                context.register_planned_source_class(
                    *class,
                    tree,
                    p.typed_bindings.planned_class_symbols(),
                )?;
            }
        }
        context.reserve_declaration(p.constructor.symbol)?;
        for (source_clause, symbols) in constructor_definition
            .value_param_clauses
            .iter()
            .zip(&p.constructor.parameter_clauses)
        {
            for (&tree, &parameter) in source_clause.iter().zip(symbols) {
                let template_parameter = p
                    .source_index
                    .symbol_at(p.source.id, tree)
                    .ok_or(PickleError::SourceDefinition { symbol: parameter })?;
                context.reserve_template_parameter(
                    template_parameter,
                    p.class,
                    p.source.id,
                    tree,
                )?;
                context.reserve_primary_constructor_parameter(
                    parameter,
                    p.constructor.symbol,
                    p.source.id,
                    tree,
                )?;
            }
        }
        let mut method_parameters = Vec::new();
        let mut method_local_symbols = Vec::new();
        for member in p.members {
            match member {
                ClassMemberPicklePlan::Field(field) => {
                    context.reserve_class_field(field.symbol, p.class, field.source_tree)?;
                }
                ClassMemberPicklePlan::Method {
                    symbol,
                    source_tree: _,
                } => {
                    context.reserve_declaration(*symbol)?;
                    let (method, locals) = method_inputs
                        .iter()
                        .find(|(method, _)| method.symbol == *symbol)
                        .ok_or(PickleError::SourceDefinition { symbol: *symbol })?;
                    for parameter in method.parameter_clauses.iter().flatten().copied() {
                        context.reserve_declaration(parameter)?;
                        method_parameters.push(parameter);
                    }
                    for local in locals {
                        context.reserve_local_binding(*local)?;
                        method_local_symbols.push(local.symbol);
                    }
                }
            }
        }

        if !p.store.symbols.contains(p.class) {
            return Err(PickleError::InvalidSymbol { symbol: p.class });
        }
        let class_record = p.store.symbols.get(p.class);
        let package = class_record.owner.ok_or(PickleError::Owner {
            symbol: p.class,
            expected: None,
            actual: None,
        })?;
        let package_reference = context.package_reference(package)?;
        let class_ty = complete_type(p.store, p.class)?;
        let Some(Type::ClassInfo(class_info)) = p.store.types.try_get(class_ty) else {
            return Err(PickleError::MalformedType {
                ty: class_ty,
                role: TypeRole::ClassInfo,
            });
        };
        let parent = *class_info
            .parents
            .first()
            .ok_or(PickleError::MalformedType {
                ty: class_ty,
                role: TypeRole::ClassInfo,
            })?;
        if class_info.parents.len() != 1 || parent != p.definitions.object_type {
            return Err(PickleError::MalformedType {
                ty: class_ty,
                role: TypeRole::ClassInfo,
            });
        }
        let parent_type = context.lower_type(parent)?;
        let new_parent = context.wire_node(AstNode::Ast {
            tag: NEW_TAG,
            child: parent_type,
        })?;
        let constructor_name = context.object_constructor_name()?;
        let owner_type = context.lower_type(parent)?;
        let select = context.wire_node(AstNode::Length {
            tag: SELECTIN_TAG,
            fields: vec![
                AstField::Nat(constructor_name),
                AstField::Tree(new_parent),
                AstField::Tree(owner_type),
            ],
        })?;
        let parent_application = context.wire_node(AstNode::Length {
            tag: APPLY_TAG,
            fields: vec![AstField::Tree(select)],
        })?;

        let mut template_parameter_labels = Vec::new();
        let mut constructor_parameter_labels = Vec::new();
        for (source_clause, symbols) in constructor_definition
            .value_param_clauses
            .iter()
            .zip(&p.constructor.parameter_clauses)
        {
            for (&tree, &parameter) in source_clause.iter().zip(symbols) {
                let template_parameter = p
                    .source_index
                    .symbol_at(p.source.id, tree)
                    .ok_or(PickleError::SourceDefinition { symbol: parameter })?;
                let template_record = p.store.symbols.get(template_parameter);
                let template_name = context.intern_name(template_record.name)?;
                let template_type = context.lower_type(p.definitions.int)?;
                let private = marker(&mut context, PRIVATE_TAG)?;
                let local = marker(&mut context, LOCAL_TAG)?;
                context.bind_declaration(
                    template_parameter,
                    AstNode::Length {
                        tag: PARAM_TAG,
                        fields: vec![
                            AstField::Nat(template_name),
                            AstField::Tree(template_type),
                            AstField::Tree(private),
                            AstField::Tree(local),
                        ],
                    },
                )?;
                template_parameter_labels.push(
                    context.declaration_label(template_parameter).ok_or(
                        PickleError::Reference {
                            symbol: template_parameter,
                            reason: crate::ReferenceError::NotRegistered,
                        },
                    )?,
                );

                let parameter_record = p.store.symbols.get(parameter);
                let parameter_name = context.intern_name(parameter_record.name)?;
                let parameter_type =
                    context.primitive_type_syntax(complete_type(p.store, parameter)?)?;
                context.bind_declaration(
                    parameter,
                    AstNode::Length {
                        tag: PARAM_TAG,
                        fields: vec![
                            AstField::Nat(parameter_name),
                            AstField::Tree(parameter_type),
                        ],
                    },
                )?;
                constructor_parameter_labels.push(context.declaration_label(parameter).ok_or(
                    PickleError::Reference {
                        symbol: parameter,
                        reason: crate::ReferenceError::NotRegistered,
                    },
                )?);
            }
        }

        let constructor_name =
            context.intern_name(p.store.symbols.get(p.constructor.symbol).name)?;
        let constructor_result = context.constructor_return_type()?;
        let mut constructor_fields = vec![AstField::Nat(constructor_name)];
        if constructor_parameter_labels.is_empty() {
            constructor_fields.push(AstField::Tree(marker(&mut context, EMPTYCLAUSE_TAG)?));
        } else {
            constructor_fields.extend(
                constructor_parameter_labels
                    .iter()
                    .copied()
                    .map(AstField::Tree),
            );
        }
        constructor_fields.push(AstField::Tree(constructor_result));
        if !constructor_parameter_labels.is_empty() {
            constructor_fields.push(AstField::Tree(marker(&mut context, STABLE_TAG)?));
        }
        context.bind_declaration(
            p.constructor.symbol,
            AstNode::Length {
                tag: DEFDEF_TAG,
                fields: constructor_fields,
            },
        )?;
        let constructor_label =
            context
                .declaration_label(p.constructor.symbol)
                .ok_or(PickleError::Reference {
                    symbol: p.constructor.symbol,
                    reason: crate::ReferenceError::NotRegistered,
                })?;
        let mut stats = template_parameter_labels
            .into_iter()
            .map(AstField::Tree)
            .collect::<Vec<_>>();
        stats.extend([
            AstField::Tree(parent_application),
            AstField::Tree(constructor_label),
        ]);
        let mut bodies = Vec::new();

        for member in p.members {
            match *member {
                ClassMemberPicklePlan::Field(field) => {
                    if !p.store.symbols.contains(field.symbol) {
                        return Err(PickleError::InvalidSymbol {
                            symbol: field.symbol,
                        });
                    }
                    let record = p.store.symbols.get(field.symbol);
                    let name = context.intern_name(record.name)?;
                    let type_syntax = context.primitive_type_syntax(field.declared_type)?;
                    let initializer = crate::expression::lower_expression(
                        p.store,
                        p.typed_ast,
                        field.typed_initializer,
                        field.symbol,
                        &constructor_parameters,
                        limits.projection,
                        &mut context,
                    )?;
                    let mut fields = vec![
                        AstField::Nat(name),
                        AstField::Tree(type_syntax),
                        AstField::Tree(initializer),
                        AstField::Tree(marker(&mut context, PRIVATE_TAG)?),
                        AstField::Tree(marker(&mut context, LOCAL_TAG)?),
                    ];
                    if record.flags == dotty_core::SymbolFlags::MUTABLE {
                        fields.push(AstField::Tree(marker(&mut context, MUTABLE_TAG)?));
                    }
                    context.bind_declaration(
                        field.symbol,
                        AstNode::Length {
                            tag: VALDEF_TAG,
                            fields,
                        },
                    )?;
                    stats.push(AstField::Tree(
                        context
                            .declaration_label(field.symbol)
                            .ok_or(PickleError::Reference {
                                symbol: field.symbol,
                                reason: crate::ReferenceError::NotRegistered,
                            })?,
                    ));
                }
                ClassMemberPicklePlan::Method {
                    symbol,
                    source_tree,
                } => {
                    let TreeKind::DefDef(definition) = &p.source_ast.get(source_tree).kind else {
                        return Err(PickleError::SourceShape {
                            tree: source_tree,
                            expected: crate::SourceRole::Method,
                        });
                    };
                    if !definition.type_params.is_empty()
                        || definition.value_param_clauses.len() > 1
                        || definition
                            .source_param_clause_order
                            .as_ref()
                            .is_some_and(|order| {
                                order.as_slice() != [DefParamClauseOrder::ValueParams(0)]
                            })
                    {
                        return Err(PickleError::Unsupported {
                            feature: UnsupportedFeature::ClassMembers,
                            symbol: Some(symbol),
                        });
                    }
                    let signature_id = complete_type(p.store, symbol)?;
                    let (parameter_types, result_ty) = match p.store.types.try_get(signature_id) {
                        Some(Type::Method(signature)) if signature.kind == MethodKind::Plain => {
                            (signature.params.clone(), signature.result)
                        }
                        Some(Type::Method(_)) => {
                            return Err(PickleError::MalformedType {
                                ty: signature_id,
                                role: TypeRole::MethodResult,
                            });
                        }
                        Some(_) => (Vec::new(), signature_id),
                        None => return Err(PickleError::InvalidType { ty: signature_id }),
                    };
                    let source_clauses = &definition.value_param_clauses;
                    let source_parameter_count = source_clauses.iter().map(Vec::len).sum::<usize>();
                    if source_parameter_count != parameter_types.len() {
                        return Err(PickleError::MalformedType {
                            ty: signature_id,
                            role: TypeRole::MethodResult,
                        });
                    }
                    let mut parameter_ids = Vec::new();
                    let mut method_fields = vec![AstField::Nat(
                        context.intern_name(p.store.symbols.get(symbol).name)?,
                    )];
                    let mut signature_index = 0;
                    for (clause_index, clause) in source_clauses.iter().enumerate() {
                        if clause.is_empty() {
                            method_fields
                                .push(AstField::Tree(marker(&mut context, EMPTYCLAUSE_TAG)?));
                            continue;
                        }
                        if clause_index != 0 {
                            return Err(PickleError::Unsupported {
                                feature: UnsupportedFeature::ClassMembers,
                                symbol: Some(symbol),
                            });
                        }
                        for &parameter_tree in clause {
                            let TreeKind::ValDef(parameter) =
                                &p.source_ast.get(parameter_tree).kind
                            else {
                                return Err(PickleError::SourceShape {
                                    tree: parameter_tree,
                                    expected: crate::SourceRole::Parameter,
                                });
                            };
                            let parameter_symbol = p
                                .source_index
                                .symbol_at(p.source.id, parameter_tree)
                                .ok_or(PickleError::SourceDefinition { symbol })?;
                            let parameter_ty = complete_type(p.store, parameter_symbol)?;
                            let expected = parameter_types[signature_index].ty;
                            let parameter_record = p.store.symbols.get(parameter_symbol);
                            let TreeKind::Ident(parameter_type) =
                                &p.source_ast.get(parameter.tpt).kind
                            else {
                                return Err(PickleError::SourceShape {
                                    tree: parameter.tpt,
                                    expected: crate::SourceRole::TypeSyntax,
                                });
                            };
                            let primitive_parameter =
                                expected == p.definitions.int || expected == p.definitions.boolean;
                            let planned_class_parameter = match p.store.types.try_get(expected) {
                                Some(Type::TypeRef {
                                    target: dotty_core::TypeRefTarget::Symbol(class),
                                    ..
                                }) if *class != p.class
                                    && p.typed_bindings.planned_class_symbols().contains(class) =>
                                {
                                    Some(*class)
                                }
                                _ => None,
                            };
                            if parameter_ty != expected
                                || (!primitive_parameter && planned_class_parameter.is_none())
                                || (primitive_parameter
                                    && !matches!(p.store.types.try_get(expected), Some(Type::TypeRef { prefix, target: dotty_core::TypeRefTarget::Symbol(_) }) if *prefix == p.definitions.no_prefix))
                                || parameter_record.kind != SymbolKind::Parameter
                                || parameter_record.owner != Some(symbol)
                                || parameter_record.origin != SymbolOrigin::Source(p.source.id)
                                || parameter_record.name != *parameter.name.as_name()
                                || parameter_record.visibility != dotty_core::Visibility::Public
                                || !parameter_record.flags.is_empty()
                                || !parameter_record.annotations.is_empty()
                                || p.source_index.symbol_at(p.source.id, parameter_tree)
                                    != Some(parameter_symbol)
                                || parameter_types[signature_index].name.as_name()
                                    != parameter.name.as_name()
                                || parameter_types[signature_index].erased
                                || parameter_types[signature_index].varargs
                                || (primitive_parameter
                                    && p.store.names.try_resolve(parameter_type.name.text())
                                        != Some(if expected == p.definitions.int {
                                            "Int"
                                        } else {
                                            "Boolean"
                                        }))
                                || planned_class_parameter.is_some_and(|class| {
                                    p.store.symbols.get(class).name != parameter_type.name
                                })
                            {
                                return Err(PickleError::UnsupportedTypeReference { ty: expected });
                            }
                            signature_index += 1;
                            parameter_ids.push(parameter_symbol);
                            let parameter_name = context.intern_name(*parameter.name.as_name())?;
                            let parameter_type = if primitive_parameter {
                                context.primitive_type_syntax(expected)?
                            } else {
                                let name = context.intern_name(parameter_type.name)?;
                                let child = context.lower_type(expected)?;
                                context.wire_node(AstNode::NatAst {
                                    tag: IDENTTPT_TAG,
                                    value: AstOperand::Nat(name),
                                    child,
                                })?
                            };
                            context.bind_declaration(
                                parameter_symbol,
                                AstNode::Length {
                                    tag: PARAM_TAG,
                                    fields: vec![
                                        AstField::Nat(parameter_name),
                                        AstField::Tree(parameter_type),
                                    ],
                                },
                            )?;
                            method_fields.push(AstField::Tree(
                                context.declaration_label(parameter_symbol).ok_or(
                                    PickleError::Reference {
                                        symbol: parameter_symbol,
                                        reason: crate::ReferenceError::NotRegistered,
                                    },
                                )?,
                            ));
                        }
                    }
                    let result_type = if result_ty == p.definitions.int
                        || result_ty == p.definitions.boolean
                        || result_ty == p.definitions.unit
                    {
                        context.primitive_type_syntax(result_ty)?
                    } else {
                        let TreeKind::Ident(result_syntax) = &p.source_ast.get(definition.tpt).kind
                        else {
                            return Err(PickleError::SourceShape {
                                tree: definition.tpt,
                                expected: crate::SourceRole::TypeSyntax,
                            });
                        };
                        let Some(Type::TypeRef {
                            target: dotty_core::TypeRefTarget::Symbol(result_class),
                            ..
                        }) = p.store.types.try_get(result_ty)
                        else {
                            return Err(PickleError::UnsupportedTypeReference { ty: result_ty });
                        };
                        let Some(dotty_core::SourceDefinition::Canonical { source, tree }) =
                            p.source_index.definition_of(*result_class)
                        else {
                            return Err(PickleError::SourceDefinition {
                                symbol: *result_class,
                            });
                        };
                        let TreeKind::TypeDef(class_definition) = &p.source_ast.get(tree).kind
                        else {
                            return Err(PickleError::SourceShape {
                                tree,
                                expected: crate::SourceRole::Class,
                            });
                        };
                        if source != p.source.id
                            || p.source_index.symbol_at(source, tree) != Some(*result_class)
                            || *result_class == p.class
                            || !p
                                .typed_bindings
                                .planned_class_symbols()
                                .contains(result_class)
                            || class_definition.name.as_name() != &result_syntax.name
                        {
                            return Err(PickleError::SourceDefinition {
                                symbol: *result_class,
                            });
                        }
                        let name = context.intern_name(result_syntax.name)?;
                        let child = context.lower_type(result_ty)?;
                        context.wire_node(AstNode::NatAst {
                            tag: IDENTTPT_TAG,
                            value: AstOperand::Nat(name),
                            child,
                        })?
                    };
                    method_fields.push(AstField::Tree(result_type));
                    let source_rhs = definition
                        .rhs
                        .ok_or(PickleError::MissingBody { method: symbol })?;
                    let actual_body = p.typed_bindings.typed_at(p.source.id, source_rhs);
                    let body_root =
                        actual_body.ok_or(PickleError::MissingBody { method: symbol })?;
                    // `method_input` ran the shared recursive source/typed
                    // validator before any body is lowered, including nested
                    // bindings, literal payloads, spans, and local identities.
                    let body = crate::expression::lower_expression(
                        p.store,
                        p.typed_ast,
                        body_root,
                        symbol,
                        &parameter_ids,
                        limits.projection,
                        &mut context,
                    )?;
                    method_fields.push(AstField::Tree(body));
                    context.bind_declaration(
                        symbol,
                        AstNode::Length {
                            tag: DEFDEF_TAG,
                            fields: method_fields,
                        },
                    )?;
                    stats.push(AstField::Tree(context.declaration_label(symbol).ok_or(
                        PickleError::Reference {
                            symbol,
                            reason: crate::ReferenceError::NotRegistered,
                        },
                    )?));
                    bodies.push((symbol, body_root));
                }
            }
        }

        let template = context.wire_node(AstNode::Length {
            tag: TEMPLATE_TAG,
            fields: stats,
        })?;
        let class_name = context.intern_name(class_record.name)?;
        context.bind_declaration(
            p.class,
            AstNode::Length {
                tag: TYPEDEF_TAG,
                fields: vec![AstField::Nat(class_name), AstField::Tree(template)],
            },
        )?;
        let class_label = context
            .declaration_label(p.class)
            .ok_or(PickleError::Reference {
                symbol: p.class,
                reason: crate::ReferenceError::NotRegistered,
            })?;
        let package_tree = context.wire_node(AstNode::Length {
            tag: PACKAGE_TAG,
            fields: vec![
                AstField::Tree(package_reference),
                AstField::Tree(class_label),
            ],
        })?;
        let asts_name = context.intern_wire_text("ASTs")?;
        let attributes_name = context.intern_wire_text("Attributes")?;
        let source_name = context.intern_wire_text(&p.source.name)?;
        let fragment = context.finish(&[package_tree])?;
        let mut declarations = Vec::new();
        for id in std::iter::once(p.class)
            .chain(std::iter::once(p.constructor.symbol))
            .chain(p.members.iter().map(|member| match member {
                ClassMemberPicklePlan::Field(field) => field.symbol,
                ClassMemberPicklePlan::Method { symbol, .. } => *symbol,
            }))
            .chain(method_parameters.iter().copied())
            .chain(method_local_symbols.iter().copied())
            .chain(
                template_parameters
                    .iter()
                    .map(|(template_parameter, _)| *template_parameter),
            )
            .chain(constructor_parameters.iter().copied())
        {
            let label = context
                .declaration_label(id)
                .ok_or(PickleError::Reference {
                    symbol: id,
                    reason: crate::ReferenceError::NotRegistered,
                })?;
            let address = fragment.ast.address(label).ok_or(PickleError::Reference {
                symbol: id,
                reason: crate::ReferenceError::NotRegistered,
            })?;
            let owner = p.store.symbols.get(id).owner.ok_or(PickleError::Owner {
                symbol: id,
                expected: None,
                actual: None,
            })?;
            declarations.push(SkeletonDeclaration {
                symbol: id,
                owner,
                address,
            });
        }
        let asts = EncodedSection::raw(asts_name, fragment.ast.bytes());
        let attributes =
            EncodedSection::attributes(attributes_name, &[Attribute::SourceFile(source_name)])
                .map_err(encoding)?;
        let mut header = Header {
            major_version: SCALA_3_9_MAJOR_VERSION,
            minor_version: SCALA_3_9_MINOR_VERSION,
            experimental_version: SCALA_3_9_EXPERIMENTAL_VERSION,
            tooling_version: "dotty-rs class-state pickler".into(),
            uuid: [0; 16],
        };
        crate::output::prepare_header(
            &mut header,
            &fragment.names,
            &asts,
            &attributes,
            limits.max_file_bytes,
        )?;
        let builder = TastyFileBuilder::new(header, fragment.names)
            .with_section(asts)
            .with_section(attributes);
        let bytes = builder.encode_validated().map_err(encoding)?;
        if bytes.len() > limits.max_file_bytes {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::FileBytes,
                limit: limits.max_file_bytes,
            });
        }
        let intended_tasty_path = crate::class_state_output::intended_class_tasty_path(
            p,
            limits.projection.max_owner_depth,
        )?;
        Ok(Assembly {
            bytes,
            source: p.source.clone(),
            intended_tasty_path,
            declarations,
            bodies,
        })
    }
}
