//! TASTy emission for the validated private primitive class-state slice.

use dotty_core::ast::{DefParamClauseOrder, TreeKind};
use dotty_core::{Constant, MethodKind, SymbolId, SymbolInfo, SymbolKind, SymbolOrigin, Type};
use dotty_tasty::tasty::*;

use crate::skeleton::Assembly;
use crate::{
    ClassMemberPicklePlan, EncodingContext, LimitKind, PickleError, PickleLimits, PickledTasty,
    SkeletonDeclaration, TypeRole, UnsupportedFeature, ValidatedClassStatePlan,
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

impl ValidatedClassStatePlan<'_, '_> {
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
        if p.constructor
            .parameter_clauses
            .iter()
            .any(|clause| !clause.is_empty())
        {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::PrimaryConstructorInput,
                symbol: Some(p.constructor.symbol),
            });
        }

        let mut context = EncodingContext::new(
            p.store,
            p.definitions,
            p.source_index,
            p.source.id,
            limits.projection,
        )?;
        context.reserve_declaration(p.class)?;
        context.reserve_declaration(p.constructor.symbol)?;
        let mut method_parameters = Vec::new();
        for member in p.members {
            match member {
                ClassMemberPicklePlan::Field(field) => {
                    context.reserve_class_field(field.symbol, p.class, field.source_tree)?;
                }
                ClassMemberPicklePlan::Method {
                    symbol,
                    source_tree,
                } => {
                    context.reserve_declaration(*symbol)?;
                    let TreeKind::DefDef(definition) = &p.source_ast.get(*source_tree).kind else {
                        return Err(PickleError::SourceShape {
                            tree: *source_tree,
                            expected: crate::SourceRole::Method,
                        });
                    };
                    for clause in &definition.value_param_clauses {
                        for &tree in clause {
                            let parameter = p
                                .source_index
                                .derived_symbol_at(*symbol, p.source.id, tree)
                                .ok_or(PickleError::SourceDefinition { symbol: *symbol })?;
                            context.reserve_declaration(parameter)?;
                            method_parameters.push(parameter);
                        }
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
        let package_path = context.package_reference(package)?;
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

        let constructor_name =
            context.intern_name(p.store.symbols.get(p.constructor.symbol).name)?;
        let empty_clause = marker(&mut context, EMPTYCLAUSE_TAG)?;
        let constructor_result = context.constructor_return_type()?;
        context.bind_declaration(
            p.constructor.symbol,
            AstNode::Length {
                tag: DEFDEF_TAG,
                fields: vec![
                    AstField::Nat(constructor_name),
                    AstField::Tree(empty_clause),
                    AstField::Tree(constructor_result),
                ],
            },
        )?;
        let constructor_label =
            context
                .declaration_label(p.constructor.symbol)
                .ok_or(PickleError::Reference {
                    symbol: p.constructor.symbol,
                    reason: crate::ReferenceError::NotRegistered,
                })?;
        let mut stats = vec![
            AstField::Tree(parent_application),
            AstField::Tree(constructor_label),
        ];
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
                        &[],
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
                                .derived_symbol_at(symbol, p.source.id, parameter_tree)
                                .ok_or(PickleError::SourceDefinition { symbol })?;
                            let parameter_ty = complete_type(p.store, parameter_symbol)?;
                            let expected = parameter_types[signature_index].ty;
                            let parameter_record = p.store.symbols.get(parameter_symbol);
                            let canonical_parameter =
                                p.source_index.symbol_at(p.source.id, parameter_tree);
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
                            if parameter_ty != expected
                                || !primitive_parameter
                                || !matches!(p.store.types.try_get(expected), Some(Type::TypeRef { prefix, target: dotty_core::TypeRefTarget::Symbol(_) }) if *prefix == p.definitions.no_prefix)
                                || parameter_record.kind != SymbolKind::Parameter
                                || parameter_record.owner != Some(symbol)
                                || parameter_record.origin != SymbolOrigin::Source(p.source.id)
                                || parameter_record.name != *parameter.name.as_name()
                                || parameter_record.visibility != dotty_core::Visibility::Public
                                || !parameter_record.flags.is_empty()
                                || !parameter_record.annotations.is_empty()
                                || canonical_parameter.is_none_or(|canonical| {
                                    canonical == parameter_symbol
                                        || p.store.symbols.get(canonical).name
                                            != *parameter.name.as_name()
                                })
                                || parameter_types[signature_index].name.as_name()
                                    != parameter.name.as_name()
                                || parameter_types[signature_index].erased
                                || parameter_types[signature_index].varargs
                                || p.store.names.try_resolve(parameter_type.name.text())
                                    != Some(if expected == p.definitions.int {
                                        "Int"
                                    } else {
                                        "Boolean"
                                    })
                            {
                                return Err(PickleError::UnsupportedTypeReference { ty: expected });
                            }
                            signature_index += 1;
                            parameter_ids.push(parameter_symbol);
                            let parameter_name = context.intern_name(*parameter.name.as_name())?;
                            let parameter_type = context.primitive_type_syntax(expected)?;
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
                    let result_type = context.primitive_type_syntax(result_ty)?;
                    method_fields.push(AstField::Tree(result_type));
                    let source_rhs = definition
                        .rhs
                        .ok_or(PickleError::MissingBody { method: symbol })?;
                    let actual_body = p.typed_bindings.typed_at(p.source.id, source_rhs);
                    let body_root =
                        actual_body.ok_or(PickleError::MissingBody { method: symbol })?;
                    let typed_body = p
                        .typed_ast
                        .try_get(body_root)
                        .ok_or(PickleError::InvalidTypedTree { tree: body_root })?;
                    let body_type_matches = typed_body.ty == result_ty
                        || match p.store.types.try_get(typed_body.ty) {
                            Some(Type::Constant(value)) => {
                                let expected = match value {
                                    Constant::Int(_) => p.definitions.int,
                                    Constant::Boolean(_) => p.definitions.boolean,
                                    Constant::Unit => p.definitions.unit,
                                    _ => {
                                        return Err(PickleError::BodyType {
                                            tree: body_root,
                                            ty: typed_body.ty,
                                        });
                                    }
                                };
                                result_ty == expected
                                    && matches!(
                                        (&typed_body.kind, value),
                                        (TreeKind::Literal(literal), value)
                                            if &literal.value == value
                                    )
                            }
                            _ => false,
                        };
                    if !body_type_matches {
                        return Err(PickleError::BodyType {
                            tree: body_root,
                            ty: typed_body.ty,
                        });
                    }
                    let source_position = p
                        .source_ast
                        .try_get(source_rhs)
                        .ok_or(PickleError::InvalidSourceTree { tree: source_rhs })?
                        .position;
                    let typed_position = p
                        .typed_ast
                        .try_get(body_root)
                        .ok_or(PickleError::InvalidTypedTree { tree: body_root })?
                        .position;
                    if source_position.is_none()
                        || source_position != typed_position
                        || source_position.is_some_and(|position| position.source() != p.source.id)
                    {
                        return Err(PickleError::BodySpan { tree: body_root });
                    }
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
            fields: vec![AstField::Tree(package_path), AstField::Tree(class_label)],
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
        let package_text = p
            .store
            .names
            .try_resolve(p.store.symbols.get(package).name.text())
            .ok_or(PickleError::InvalidName {
                name: p.store.symbols.get(package).name.text(),
            })?;
        let class_text = p.store.names.try_resolve(class_record.name.text()).ok_or(
            PickleError::InvalidName {
                name: class_record.name.text(),
            },
        )?;
        let intended_tasty_path = format!(
            "{}/{}.tasty",
            crate::artifact_name::encode_component(package_text),
            crate::artifact_name::encode_component(class_text)
        );
        Ok(Assembly {
            bytes,
            source: p.source.clone(),
            intended_tasty_path,
            declarations,
            bodies,
        })
    }
}
