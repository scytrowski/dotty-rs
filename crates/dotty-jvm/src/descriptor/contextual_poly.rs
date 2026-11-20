//! Erasure of the unbounded contextual identity method `def id[A](using a: A): A`.

use super::*;
use crate::JvmMethodMetadata;
use dotty_classfile::bytecode::CodeBuilder;
use dotty_core::ast::{TreeKind, Typed, TypedAst};
use dotty_core::ids::TreeId;
use dotty_core::types::{PolyType, TermRefTarget};

/// Match both representations of A. In the completed public signature it is a
/// reference into the poly binder. In a term parameter's declaration it is a
/// direct reference to the type parameter owned by this method.
fn has_evidence_signature(
    store: &SemanticStore,
    definitions: &Definitions,
    method: SymbolId,
    parameters: &[SymbolId],
    binder: TypeId,
    poly: &PolyType,
) -> bool {
    let ([parameter], [type_formal]) = (parameters, poly.params.as_slice()) else {
        return false;
    };
    let Some(Type::Method(contextual)) = store.types.try_get(poly.result) else {
        return false;
    };
    let [formal] = contextual.params.as_slice() else {
        return false;
    };
    let parameter_ref = Type::ParamRef { binder, index: 0 };
    if contextual.kind != MethodKind::Contextual
        || formal.erased
        || formal.varargs
        || store.types.try_get(formal.ty) != Some(&parameter_ref)
        || store.types.try_get(contextual.result) != Some(&parameter_ref)
        || !store.symbols.contains(*parameter)
    {
        return false;
    }
    let param = store.symbols.get(*parameter);
    if param.name != *formal.name.as_name()
        || param.owner != Some(method)
        || param.kind != SymbolKind::Parameter
        || param.flags != SymbolFlags::GIVEN
    {
        return false;
    }
    let SymbolInfo::Complete(declared) = param.info else {
        return false;
    };
    let Some(Type::TypeRef {
        prefix,
        target: TypeRefTarget::Symbol(symbol),
    }) = store.types.try_get(declared)
    else {
        return false;
    };
    if *prefix != definitions.no_prefix
        || !matches!(store.types.try_get(*prefix), Some(Type::NoPrefix))
        || !store.symbols.contains(*symbol)
    {
        return false;
    }
    let tparam = store.symbols.get(*symbol);
    let SymbolInfo::Complete(bounds) = tparam.info else {
        return false;
    };
    let default_bounds = Type::Bounds {
        low: definitions.nothing_type,
        high: definitions.any_type,
    };
    tparam.kind == SymbolKind::TypeParameter
        && tparam.owner == Some(method)
        && tparam.name == *type_formal.name.as_name()
        && type_formal.declared_variance.is_none()
        && store.types.try_get(type_formal.bounds) == Some(&default_bounds)
        && store.types.try_get(bounds) == Some(&default_bounds)
}

pub(super) fn descriptor(
    plan: &JvmMethodPlan,
    store: &SemanticStore,
    definitions: &Definitions,
    binder: TypeId,
    poly: &PolyType,
) -> Result<JvmMethodDescriptor, JvmCodegenError> {
    if !has_evidence_signature(
        store,
        definitions,
        plan.method,
        &plan.parameters,
        binder,
        poly,
    ) || store.types.try_get(definitions.object_type)
        != Some(&Type::type_ref(
            definitions.no_prefix,
            definitions.object_class,
        ))
        || !store.symbols.contains(definitions.object_class)
        || store.symbols.get(definitions.object_class).origin != SymbolOrigin::Builtin
    {
        return Err(JvmCodegenError::UnsupportedMethodShape {
            method: plan.method,
            ty: binder,
            kind: UnsupportedMethodShapeKind::Polymorphic,
        });
    }
    let text = "(Ljava/lang/Object;)Ljava/lang/Object;";
    let descriptor = MethodDescriptor::parse(text).map_err(|error| {
        JvmCodegenError::InvalidGeneratedDescriptor {
            method: plan.method,
            error,
        }
    })?;
    let erased = JvmType::Reference(definitions.object_class);
    Ok(JvmMethodDescriptor {
        descriptor,
        descriptor_text: text.to_owned(),
        parameters: vec![erased],
        result: JvmReturnType::Value(erased),
    })
}

/// A generic identity returns the exact argument reference. Reusing the
/// ordinary hierarchy-based projection here would require putting java.lang.Object
/// in the source unit's user-class hierarchy.
pub(crate) fn lower_body(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    metadata: &JvmMethodMetadata,
    body: TreeId<Typed>,
    definitions: &Definitions,
) -> Result<Option<CodeBuilder>, JvmCodegenError> {
    let method = metadata.symbol();
    if !store.symbols.contains(method) {
        return Err(JvmCodegenError::UnknownMethod { method });
    }
    let signature = complete_info(store, method)?;
    let Some(Type::Poly(poly)) = store.types.try_get(signature) else {
        return Ok(None);
    };
    let [param] = metadata.parameters() else {
        return Err(JvmCodegenError::UnsupportedMethodShape {
            method,
            ty: signature,
            kind: UnsupportedMethodShapeKind::Polymorphic,
        });
    };
    let erased = JvmType::Reference(definitions.object_class);
    if metadata.descriptor().result() != JvmReturnType::Value(erased)
        || param.ty() != erased
        || !has_evidence_signature(
            store,
            definitions,
            method,
            &[param.symbol()],
            signature,
            poly,
        )
    {
        return Err(JvmCodegenError::UnsupportedMethodShape {
            method,
            ty: signature,
            kind: UnsupportedMethodShapeKind::Polymorphic,
        });
    }
    let tree = typed_ast
        .try_get(body)
        .ok_or(JvmCodegenError::UnknownBodyRoot { method, tree: body })?;
    let TreeKind::Ident(ident) = &tree.kind else {
        return Err(JvmCodegenError::UnsupportedTypedExpression {
            tree: body,
            node: "polymorphic contextual body",
        });
    };
    let Some(Type::TermRef {
        prefix,
        target: TermRefTarget::Symbol(selected),
    }) = store.types.try_get(tree.ty)
    else {
        return Err(JvmCodegenError::InvalidParameterReference {
            tree: body,
            symbol: None,
        });
    };
    if *selected != param.symbol() || ident.name != store.symbols.get(*selected).name {
        return Err(JvmCodegenError::InvalidParameterReference {
            tree: body,
            symbol: Some(*selected),
        });
    }
    if *prefix != definitions.no_prefix {
        return Err(JvmCodegenError::InvalidParameterReferencePrefix {
            tree: body,
            parameter: *selected,
            prefix: *prefix,
        });
    }
    let mut code = CodeBuilder::new();
    code.aload(usize::from(param.slot()))
        .and_then(CodeBuilder::areturn)
        .map_err(|error| JvmCodegenError::CodeBuilder { tree: body, error })?;
    Ok(Some(code))
}
