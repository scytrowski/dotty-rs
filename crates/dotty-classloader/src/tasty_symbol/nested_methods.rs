use super::*;

pub(crate) struct NestedMethods {
    pub(crate) methods: Vec<DecodedTastyMethod>,
    // The unshadowed enclosing parameter in the source declaration /(A): A.
    division_parameter: Option<String>,
}

impl NestedMethods {
    pub(crate) fn matches_division(
        &self,
        flags: MethodAccessFlags,
        signature: Option<&dotty_classfile::signature::MethodSignature>,
        descriptor: &dotty_classfile::descriptor::MethodDescriptor,
    ) -> bool {
        use dotty_classfile::descriptor::FieldType;
        use dotty_classfile::signature::{
            ReferenceTypeSignature::TypeVariable, TypeSignature::Reference,
        };
        let (Some(param), Some(signature)) = (&self.division_parameter, signature) else {
            return false;
        };
        !flags.is_static()
            && !flags.is_bridge()
            && !flags.is_synthetic()
            && !flags.is_varargs()
            && signature.type_parameters.is_empty()
            && matches!(signature.parameters.as_slice(), [Reference(TypeVariable(name))] if name == param)
            && matches!(&signature.result, Some(Reference(TypeVariable(name))) if name == param)
            && matches!(descriptor.parameters.as_slice(), [FieldType::Object(name)] if name == "java/lang/Object")
            && matches!(&descriptor.return_type, Some(FieldType::Object(name)) if name == "java/lang/Object")
    }
}

/// Direct methods from the matching nested template in the enclosing class's
/// compilation unit. The JVM InnerClasses self-entry establishes the enclosing
/// owner and distinguishes members of a companion from instance members.
pub(crate) fn nested_methods(
    bytes: &[u8],
    owner: &BinaryName,
    inner: &str,
    is_static: bool,
) -> Result<Option<NestedMethods>, TastyDecodeError> {
    let mut file = parse_classloader_tasty(bytes)?;
    file.prepare_ast_address_index()?;
    let index = file.ast_address_index_view()?;
    let addresses = type_parameter_addresses(&index);
    let outer_name = if is_static {
        format!("{}$", owner.simple_name())
    } else {
        owner.simple_name().to_owned()
    };
    let mut methods = None;
    for node in index.iter().filter(|node| {
        node.tag == TYPEDEF_TAG
            && is_in_requested_top_level_package(&file, &index, node.offset, owner.package_path())
    }) {
        let StructuredNode::TypeDef(DefinitionBody::TypeDef {
            name,
            type_or_template: RawTree::LengthNode(template),
            tail,
        }) = node.decode_structured()?
        else {
            continue;
        };
        if template.tag != TEMPLATE_TAG
            || has_modifier(&tail, OBJECT_TAG) != is_static
            || module_class_wire_name(&file, name).as_deref() != Some(outer_name.as_str())
        {
            continue;
        }
        let outer = template.decode_template_structure()?;
        // Only an instance member of a unary generic outer can use this form.
        let parameter = match outer.type_params.as_slice() {
            [ParameterNode::TypeParam { name, payload, .. }] if !is_static => addresses
                .get(&(payload.as_ptr(), payload.len()))
                .copied()
                .zip(wire_name(&file, *name)),
            _ => None,
        };
        for stat in outer.stats {
            let RawTree::LengthNode(def) = stat else {
                continue;
            };
            if def.tag != TYPEDEF_TAG {
                continue;
            }
            let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                name,
                type_or_template: RawTree::LengthNode(body),
                ..
            }) = def.decode_structured()?
            else {
                continue;
            };
            if body.tag != TEMPLATE_TAG
                || module_class_wire_name(&file, name).as_deref() != Some(inner)
            {
                continue;
            }
            if methods.is_some() {
                return Err(TastyDecodeError::AmbiguousTypeDef);
            }
            let binary = BinaryName::from_internal(format!("{}${inner}", owner.as_internal()));
            let template = body.decode_template_structure()?;
            let parameter = parameter
                .as_ref()
                .filter(|_| template.type_params.is_empty());
            let class_parameters = parameter
                .map(|&(address, _)| (address, 0))
                .into_iter()
                .collect();
            let (_, decoded) = decode_members(
                &file,
                &binary,
                &template.stats,
                true,
                owner.package_path(),
                &class_parameters,
                &addresses,
                &mut |_, _| None,
            )?;
            let division_parameter = match parameter {
                Some((_, name)) if declared_outer_division(&file, &template.stats, &decoded)? => {
                    Some(name.clone())
                }
                _ => None,
            };
            methods = Some(NestedMethods {
                methods: decoded,
                division_parameter,
            });
        }
    }
    Ok(methods)
}

// Inspect the original definitions: unsupported overloads can be absent
// from the decoded list. An annotated operator may have a different @targetName.
fn declared_outer_division(
    file: &TastyFile<'_>,
    stats: &[RawTree<'_>],
    decoded: &[DecodedTastyMethod],
) -> Result<bool, TastyDecodeError> {
    let mut division = None;
    for stat in stats {
        let RawTree::LengthNode(def) = stat else {
            continue;
        };
        if def.tag != DEFDEF_TAG {
            continue;
        }
        let StructuredNode::DefDef(body) = def.decode_structured()? else {
            continue;
        };
        let name = wire_name(file, body.name);
        if name.as_deref() == Some("$div") {
            return Ok(false);
        }
        if name.as_deref() != Some("/") {
            continue;
        }
        if division.is_some()
            || term_clauses(&body.header_items)?.as_slice() != [(MethodKind::Plain, 1)]
        {
            return Ok(false);
        }
        division = Some(def.offset);
    }
    Ok(decoded.iter().find(|m| Some(m.offset) == division).is_some_and(|m| {
        !m.has_annotations
            && m.type_parameters.is_empty()
            && matches!(m.parameters.as_slice(), [arg] if !arg.erased && arg.declared_type == Some(DecodedTastyType::ClassParamRef(0)))
            && m.return_type == Some(DecodedTastyType::ClassParamRef(0))
    }))
}

/// Correlates the source declaration `[A](x: A)(implicit ev: C[A]): C[A]#M`
/// with the exact generic JVM signature before restoring its two clauses.
pub(crate) fn matches_contextual_projection(
    method: &DecodedTastyMethod,
    sig: &dotty_classfile::signature::MethodSignature,
    descriptor: &dotty_classfile::descriptor::MethodDescriptor,
) -> bool {
    use DecodedTastyType::{Applied, Named, ParamRef};
    use dotty_classfile::signature::{ReferenceTypeSignature as Ref, TypeArgument, TypeSignature};
    let (
        [param],
        [x, ev],
        Some(DecodedTastyType::Projected {
            qualifier: prefix,
            member,
        }),
    ) = (
        method.type_parameters.as_slice(),
        method.parameters.as_slice(),
        &method.return_type,
    )
    else {
        return false;
    };
    let Some(Applied { tycon, arguments }) = &ev.declared_type else {
        return false;
    };
    let Named(evidence) = tycon.as_ref() else {
        return false;
    };
    let (
        [jparam],
        [
            TypeSignature::Reference(Ref::TypeVariable(var)),
            TypeSignature::Reference(Ref::Class(goal)),
        ],
        Some(TypeSignature::Reference(Ref::Class(result))),
    ) = (
        sig.type_parameters.as_slice(),
        sig.parameters.as_slice(),
        &sig.result,
    )
    else {
        return false;
    };
    let [inner] = result.suffix.as_slice() else {
        return false;
    };
    let param_argument = [TypeArgument::Exact(Ref::TypeVariable(var.clone()))];
    let named = |ty: &Option<DecodedTastyType>, binary: &str| matches!(ty, Some(Named(name)) if name.as_internal() == binary);
    let [
        dotty_classfile::descriptor::FieldType::Object(input),
        dotty_classfile::descriptor::FieldType::Object(context),
    ] = descriptor.parameters.as_slice()
    else {
        return false;
    };
    let Some(dotty_classfile::descriptor::FieldType::Object(erased_result)) =
        &descriptor.return_type
    else {
        return false;
    };
    input == "java/lang/Object"
        && context == evidence.as_internal()
        && *erased_result == format!("{}${member}", evidence.as_internal())
        && !method.has_annotations
        && method.semantic_flags.contains(SymbolFlags::IMPLICIT)
        && method.term_clauses.as_deref()
            == Some(&[(MethodKind::Plain, 1), (MethodKind::Implicit, 1)])
        && !x.erased
        && !ev.erased
        && param.bounds_supported
        && named(&param.lower_bound, "scala/Nothing")
        && named(&param.upper_bound, "scala/Any")
        && x.declared_type == Some(ParamRef(0))
        && arguments.as_slice() == [ParamRef(0)]
        && ev.declared_type.as_ref() == Some(prefix.as_ref())
        && jparam.name == *var
        && jparam.interface_bounds.is_empty()
        && matches!(&jparam.class_bound, Some(Ref::Class(bound)) if bound.package == ["java","lang"] && bound.simple_name == "Object" && bound.type_arguments.is_empty() && bound.suffix.is_empty())
        && goal.suffix.is_empty()
        && goal.type_arguments == param_argument
        && crate::loader::class_type_signature_binary_name(goal) == *evidence
        && result.simple_name == goal.simple_name
        && result.package == goal.package
        && result.type_arguments == param_argument
        && &inner.name == member
        && inner.type_arguments.is_empty()
}
