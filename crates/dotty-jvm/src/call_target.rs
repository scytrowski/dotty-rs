//! Canonical projection of supported typed calls to planned JVM targets.

use std::collections::HashSet;

use dotty_core::ast::{ApplyKind, Tree, TreeKind, Typed};
use dotty_core::definitions::Definitions;
use dotty_core::ids::{SymbolId, TreeId, TypeId};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolKind, Visibility};
use dotty_core::types::{Constant, TermRefTarget, Type, TypeRefTarget};

use crate::{
    JvmCallTarget, JvmCodegenError, JvmCompilationUnit, JvmInvocationKind, JvmReturnType, JvmType,
    SymbolRole, UnsupportedJvmCallShapeKind,
};

pub(super) fn resolve_same_class_call(
    unit: &JvmCompilationUnit<'_>,
    caller_class: SymbolId,
    caller_method: SymbolId,
    call: TreeId<Typed>,
    definitions: &Definitions,
) -> Result<JvmCallTarget, JvmCodegenError> {
    if let Some(call_tree) = unit.typed_ast.try_get(call)
        && let TreeKind::Apply(application) = &call_tree.kind
        && let Some(function) = unit.typed_ast.try_get(application.function)
        && let TreeKind::Select(select) = &function.kind
    {
        let receiver = tree(unit.store, unit.typed_ast, select.qualifier)?;
        match &receiver.kind {
            TreeKind::This(this) if this.qual.is_some() => {
                return Err(unsupported_call(
                    select.qualifier,
                    UnsupportedJvmCallShapeKind::QualifiedThisReceiver,
                ));
            }
            TreeKind::This(_) => {
                if !matches!(
                    unit.store.types.try_get(receiver.ty),
                    Some(Type::ThisType { class }) if *class == caller_class
                ) {
                    return Err(unsupported_call(
                        application.function,
                        UnsupportedJvmCallShapeKind::NonCurrentThis,
                    ));
                }
            }
            _ => {
                return Err(unsupported_call(
                    application.function,
                    UnsupportedJvmCallShapeKind::NonThisReceiver,
                ));
            }
        }
    }
    let mut target = resolve_instance_call(unit, caller_class, caller_method, call, definitions)?;
    target.receiver_tree = None;
    Ok(target)
}

pub(super) fn resolve_instance_call(
    unit: &JvmCompilationUnit<'_>,
    caller_class: SymbolId,
    caller_method: SymbolId,
    call: TreeId<Typed>,
    definitions: &Definitions,
) -> Result<JvmCallTarget, JvmCodegenError> {
    resolve_instance_call_with_kind(
        unit,
        caller_class,
        caller_method,
        call,
        definitions,
        JvmInvocationKind::Virtual,
    )
}

pub(super) fn resolve_direct_super_call(
    unit: &JvmCompilationUnit<'_>,
    caller_class: SymbolId,
    caller_method: SymbolId,
    call: TreeId<Typed>,
    definitions: &Definitions,
) -> Result<JvmCallTarget, JvmCodegenError> {
    resolve_instance_call_with_kind(
        unit,
        caller_class,
        caller_method,
        call,
        definitions,
        JvmInvocationKind::SpecialSuper,
    )
}

fn resolve_instance_call_with_kind(
    unit: &JvmCompilationUnit<'_>,
    caller_class: SymbolId,
    caller_method: SymbolId,
    call: TreeId<Typed>,
    definitions: &Definitions,
    invocation: JvmInvocationKind,
) -> Result<JvmCallTarget, JvmCodegenError> {
    let mut context = CallResolutionContext::default();
    resolve_instance_call_recursive(
        unit,
        caller_class,
        caller_method,
        call,
        definitions,
        &mut context,
        invocation,
    )
}

#[derive(Default)]
struct CallResolutionContext {
    depth: usize,
    active_calls: HashSet<TreeId<Typed>>,
}

fn resolve_instance_call_recursive(
    unit: &JvmCompilationUnit<'_>,
    caller_class: SymbolId,
    caller_method: SymbolId,
    call: TreeId<Typed>,
    definitions: &Definitions,
    context: &mut CallResolutionContext,
    invocation: JvmInvocationKind,
) -> Result<JvmCallTarget, JvmCodegenError> {
    let call_tree = tree(unit.store, unit.typed_ast, call)?;
    if context.depth >= crate::JVM_EXPRESSION_DEPTH_LIMIT {
        return Err(JvmCodegenError::ExpressionDepthExceeded {
            tree: call,
            position: call_tree.position,
            limit: crate::JVM_EXPRESSION_DEPTH_LIMIT,
        });
    }
    if !context.active_calls.insert(call) {
        return Err(unsupported_call(
            call,
            UnsupportedJvmCallShapeKind::RecursiveReceiver,
        ));
    }
    context.depth += 1;
    let result = resolve_instance_call_body(
        unit,
        caller_class,
        caller_method,
        call,
        definitions,
        context,
        invocation,
    );
    context.depth -= 1;
    context.active_calls.remove(&call);
    result
}

fn resolve_instance_call_body(
    unit: &JvmCompilationUnit<'_>,
    caller_class: SymbolId,
    caller_method: SymbolId,
    call: TreeId<Typed>,
    definitions: &Definitions,
    context: &mut CallResolutionContext,
    invocation: JvmInvocationKind,
) -> Result<JvmCallTarget, JvmCodegenError> {
    let class_plan = unit
        .classes
        .iter()
        .find(|plan| plan.class == caller_class)
        .ok_or(JvmCodegenError::MissingClassPlan {
            class: caller_class,
        })?;
    let caller_method_plan = class_plan
        .methods
        .iter()
        .find(|plan| plan.method == caller_method)
        .ok_or(JvmCodegenError::CallerMethodNotInJvmPlan {
            class: caller_class,
            method: caller_method,
        })?;

    let call_tree = tree(unit.store, unit.typed_ast, call)?;
    let (function_id, apply_kind) = match &call_tree.kind {
        TreeKind::Apply(apply) => (apply.function, apply.kind),
        TreeKind::TypeApply(_) => {
            return Err(unsupported_call(
                call,
                UnsupportedJvmCallShapeKind::TypeApplication,
            ));
        }
        _ => {
            return Err(unsupported_call(
                call,
                UnsupportedJvmCallShapeKind::NotApplication,
            ));
        }
    };
    if apply_kind != ApplyKind::Regular {
        return Err(unsupported_call(
            call,
            UnsupportedJvmCallShapeKind::UsingClause,
        ));
    }

    let function_tree = tree(unit.store, unit.typed_ast, function_id)?;
    let (receiver_tree, receiver_class, direct_super_parent) = match &function_tree.kind {
        TreeKind::Ident(_) if invocation == JvmInvocationKind::Virtual => {
            (None, caller_class, None)
        }
        TreeKind::Select(select) if invocation == JvmInvocationKind::SpecialSuper => {
            let receiver = select.qualifier;
            let parent = validate_direct_super_receiver(unit, caller_class, receiver, definitions)?;
            (Some(receiver), caller_class, Some(parent))
        }
        TreeKind::Select(select) => {
            let receiver = select.qualifier;
            let receiver_class = resolve_receiver_class(
                unit,
                caller_class,
                caller_method,
                caller_method_plan,
                receiver,
                definitions,
                context,
            )?;
            (Some(receiver), receiver_class, None)
        }
        TreeKind::TypeApply(_) => {
            return Err(unsupported_call(
                function_id,
                UnsupportedJvmCallShapeKind::TypeApplication,
            ));
        }
        TreeKind::Apply(_) => {
            return Err(unsupported_call(
                function_id,
                UnsupportedJvmCallShapeKind::NestedApplication,
            ));
        }
        _ => {
            return Err(unsupported_call(
                function_id,
                UnsupportedJvmCallShapeKind::UnsupportedFunctionTree,
            ));
        }
    };

    let (prefix, method) = match unit.store.types.try_get(function_tree.ty) {
        Some(Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(method),
        }) => (*prefix, *method),
        _ => return Err(JvmCodegenError::UnresolvedJvmCallTarget { tree: function_id }),
    };
    let receiver = receiver_tree
        .map(|receiver| tree(unit.store, unit.typed_ast, receiver))
        .transpose()?;
    let method_symbol = unit
        .store
        .symbols
        .contains(method)
        .then(|| unit.store.symbols.get(method))
        .ok_or(JvmCodegenError::UnknownMethod { method })?;
    if method_symbol.kind != SymbolKind::Method {
        return Err(JvmCodegenError::UnsupportedSymbolKind {
            symbol: method,
            expected: SymbolRole::Method,
            found: method_symbol.kind,
        });
    }
    let method_owner = method_symbol.owner.ok_or(JvmCodegenError::WrongOwner {
        symbol: method,
        expected: receiver_class,
        actual: None,
    })?;
    let supported_prefix = if invocation == JvmInvocationKind::SpecialSuper {
        receiver.is_some_and(|receiver| prefix == receiver.ty)
    } else {
        match (receiver, unit.store.types.try_get(prefix)) {
            (None, Some(Type::NoPrefix)) => true,
            (None, Some(Type::ThisType { class })) => *class == caller_class,
            (Some(_receiver), Some(Type::ThisType { class })) => {
                *class == caller_class && receiver_class == caller_class
            }
            (Some(receiver), _) => prefix_matches_receiver(
                unit,
                prefix,
                receiver.ty,
                receiver_class,
                method_owner,
                definitions,
            ),
            _ => false,
        }
    };
    if !supported_prefix {
        return Err(unsupported_call(
            function_id,
            UnsupportedJvmCallShapeKind::UnsupportedReceiverPrefix,
        ));
    }
    if let Some(expected_parent) = direct_super_parent {
        if !is_validated_ancestor(unit, expected_parent, method_owner, definitions)? {
            return Err(JvmCodegenError::WrongOwner {
                symbol: method,
                expected: expected_parent,
                actual: Some(method_owner),
            });
        }
    } else if method_owner != receiver_class
        && !is_validated_ancestor(unit, receiver_class, method_owner, definitions)?
    {
        return Err(JvmCodegenError::WrongOwner {
            symbol: method,
            expected: receiver_class,
            actual: Some(method_owner),
        });
    }
    let owner_plan = unit
        .classes
        .iter()
        .find(|plan| plan.class == method_owner)
        .ok_or(JvmCodegenError::MissingClassPlan {
            class: method_owner,
        })?;
    let Some(owner_method_plan) = owner_plan
        .methods
        .iter()
        .find(|method_plan| method_plan.method == method)
    else {
        return Err(JvmCodegenError::MethodNotInJvmPlan {
            tree: function_id,
            class: method_owner,
            method,
        });
    };
    if owner_method_plan.body.is_none() {
        return Err(JvmCodegenError::MissingTypedBody { method });
    }

    match method_symbol.visibility {
        Visibility::Public => {}
        Visibility::Private if method_owner == caller_class && receiver_class == caller_class => {}
        visibility => {
            return Err(JvmCodegenError::UnsupportedJvmVisibility {
                symbol: method,
                visibility,
            });
        }
    }

    let class_info = super::identity::class_info(unit.store, method_owner)?;
    if !unit
        .store
        .scopes
        .get(class_info.declarations)
        .entered_symbols()
        .any(|declared| declared == method)
    {
        return Err(JvmCodegenError::MethodNotInClassDeclarations {
            class: method_owner,
            method,
        });
    }

    let all_metadata = unit.metadata(definitions)?;
    let receiver_metadata = all_metadata
        .iter()
        .find(|metadata| metadata.identity().symbol() == receiver_class)
        .ok_or(JvmCodegenError::MissingClassPlan {
            class: receiver_class,
        })?;
    let class_metadata = all_metadata
        .iter()
        .find(|metadata| metadata.identity().symbol() == method_owner)
        .ok_or(JvmCodegenError::MissingClassPlan {
            class: method_owner,
        })?;
    let method_metadata = class_metadata
        .methods()
        .iter()
        .find(|metadata| metadata.symbol() == method)
        .ok_or(JvmCodegenError::MethodNotInJvmPlan {
            tree: function_id,
            class: method_owner,
            method,
        })?;
    if method_owner != receiver_class && direct_super_parent.is_none() {
        let is_effective_inherited_method =
            receiver_metadata.dispatch_methods().iter().any(|entry| {
                entry.receiver_class() == receiver_class
                    && entry.declaring_class() == method_owner
                    && entry.method() == method
                    && entry.name() == method_metadata.name()
                    && entry.descriptor().as_str() == method_metadata.descriptor().as_str()
            });
        if !is_effective_inherited_method {
            return Err(JvmCodegenError::WrongOwner {
                symbol: method,
                expected: receiver_class,
                actual: Some(method_owner),
            });
        }
    }
    if let Some(parent) = direct_super_parent
        && method_owner != parent
    {
        let parent_metadata = all_metadata
            .iter()
            .find(|metadata| metadata.identity().symbol() == parent)
            .ok_or(JvmCodegenError::MissingClassPlan { class: parent })?;
        let is_effective_parent_method = parent_metadata.dispatch_methods().iter().any(|entry| {
            entry.receiver_class() == parent
                && entry.declaring_class() == method_owner
                && entry.method() == method
                && entry.name() == method_metadata.name()
                && entry.descriptor().as_str() == method_metadata.descriptor().as_str()
        });
        if !is_effective_parent_method {
            return Err(JvmCodegenError::WrongOwner {
                symbol: method,
                expected: parent,
                actual: Some(method_owner),
            });
        }
    }
    let found = project_result_type(unit, call_tree.ty, definitions);
    let expected = method_metadata.descriptor().result();
    if found != Some(expected) {
        return Err(JvmCodegenError::JvmCallResultTypeMismatch {
            tree: call,
            method,
            expected,
            found,
        });
    }

    let invocation_owner = direct_super_parent
        .filter(|_| invocation == JvmInvocationKind::SpecialSuper)
        .and_then(|parent| {
            all_metadata
                .iter()
                .find(|metadata| metadata.identity().symbol() == parent)
        })
        .unwrap_or(class_metadata);
    Ok(JvmCallTarget {
        owner_internal_name: invocation_owner.identity().internal_name().to_owned(),
        method_name: method_metadata.name().to_owned(),
        descriptor: method_metadata.descriptor().clone(),
        method,
        invocation,
        result: expected,
        receiver_tree,
        receiver_class,
    })
}

fn is_validated_ancestor(
    unit: &JvmCompilationUnit<'_>,
    class: SymbolId,
    ancestor: SymbolId,
    definitions: &Definitions,
) -> Result<bool, JvmCodegenError> {
    let hierarchy = unit.class_hierarchy(definitions)?;
    let mut current = class;
    for _ in 0..=hierarchy.superclasses().len() {
        if current == ancestor {
            return Ok(true);
        }
        let Some(superclass) = hierarchy
            .superclasses()
            .iter()
            .find(|superclass| superclass.class() == current)
        else {
            return Ok(false);
        };
        current = superclass.parent();
    }
    Ok(current == ancestor)
}

fn validate_direct_super_receiver(
    unit: &JvmCompilationUnit<'_>,
    caller_class: SymbolId,
    receiver_id: TreeId<Typed>,
    definitions: &Definitions,
) -> Result<SymbolId, JvmCodegenError> {
    let receiver = tree(unit.store, unit.typed_ast, receiver_id)?;
    let TreeKind::Super(super_tree) = &receiver.kind else {
        return Err(unsupported_call(
            receiver_id,
            UnsupportedJvmCallShapeKind::UnsupportedSuperQualifier,
        ));
    };
    if super_tree.mix.is_some() {
        return Err(unsupported_call(
            receiver_id,
            UnsupportedJvmCallShapeKind::UnsupportedSuperQualifier,
        ));
    }
    let qualifier = tree(unit.store, unit.typed_ast, super_tree.qual)?;
    if !matches!(
        qualifier.kind,
        TreeKind::This(dotty_core::ast::This { qual: None })
    ) || !matches!(
        unit.store.types.try_get(qualifier.ty),
        Some(Type::ThisType { class }) if *class == caller_class
    ) {
        return Err(unsupported_call(
            receiver_id,
            UnsupportedJvmCallShapeKind::UnsupportedSuperQualifier,
        ));
    }

    let metadata = unit.metadata(definitions)?;
    let class = metadata
        .iter()
        .find(|class| class.identity().symbol() == caller_class)
        .ok_or(JvmCodegenError::MissingClassPlan {
            class: caller_class,
        })?;
    let parent = class
        .superclass()
        .filter(|superclass| superclass.class() == caller_class)
        .map(|superclass| superclass.parent())
        .ok_or(JvmCodegenError::UnsupportedJvmSuperclass {
            class: caller_class,
            parent: None,
        })?;

    let type_matches = matches!(
        unit.store.types.try_get(receiver.ty),
        Some(Type::SuperType { this_type, super_type })
            if matches!(
                unit.store.types.try_get(*this_type),
                Some(Type::ThisType { class }) if *class == caller_class
            ) && matches!(
                unit.store.types.try_get(*super_type),
                Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == parent
            )
    );
    if !type_matches {
        return Err(unsupported_call(
            receiver_id,
            UnsupportedJvmCallShapeKind::SuperTypeDoesNotMatchHierarchy,
        ));
    }
    Ok(parent)
}

fn resolve_receiver_class(
    unit: &JvmCompilationUnit<'_>,
    caller_class: SymbolId,
    caller_method: SymbolId,
    method_plan: &crate::JvmMethodPlan,
    receiver_id: TreeId<Typed>,
    definitions: &Definitions,
    context: &mut CallResolutionContext,
) -> Result<SymbolId, JvmCodegenError> {
    let receiver = tree(unit.store, unit.typed_ast, receiver_id)?;
    match &receiver.kind {
        TreeKind::This(this) => {
            if this.qual.is_some() {
                return Err(unsupported_call(
                    receiver_id,
                    UnsupportedJvmCallShapeKind::QualifiedThisReceiver,
                ));
            }
            match unit.store.types.try_get(receiver.ty) {
                Some(Type::ThisType { class }) if *class == caller_class => Ok(*class),
                _ => Err(unsupported_call(
                    receiver_id,
                    UnsupportedJvmCallShapeKind::NonCurrentThis,
                )),
            }
        }
        TreeKind::Ident(_) => {
            let Some(Type::TermRef {
                prefix,
                target: TermRefTarget::Symbol(local),
            }) = unit.store.types.try_get(receiver.ty)
            else {
                return Err(unsupported_call(
                    receiver_id,
                    UnsupportedJvmCallShapeKind::UnsupportedFunctionTree,
                ));
            };
            if unit.store.symbols.contains(*local)
                && unit.store.symbols.get(*local).kind == SymbolKind::Field
            {
                return planned_current_field_receiver_class(
                    unit,
                    caller_class,
                    receiver_id,
                    *local,
                    *prefix,
                    definitions,
                );
            }
            let symbol = unit
                .store
                .symbols
                .contains(*local)
                .then(|| unit.store.symbols.get(*local))
                .ok_or(JvmCodegenError::InvalidLocalReference {
                    tree: receiver_id,
                    symbol: *local,
                })?;
            let prefix_is_canonical = *prefix == definitions.no_prefix
                && matches!(unit.store.types.try_get(*prefix), Some(Type::NoPrefix));
            if !prefix_is_canonical {
                return Err(unsupported_call(
                    receiver_id,
                    UnsupportedJvmCallShapeKind::UnsupportedReceiverPrefix,
                ));
            }
            let local_type = match unit.store.symbols.info(*local) {
                SymbolInfo::Complete(ty) => *ty,
                SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
                    return Err(JvmCodegenError::MissingSemanticInfo { symbol: *local });
                }
                SymbolInfo::Error => {
                    return Err(JvmCodegenError::ErrorSemanticInfo { symbol: *local });
                }
            };
            if symbol.kind == SymbolKind::Parameter && symbol.owner == Some(caller_method) {
                let Some(parameter_index) = method_plan
                    .parameters
                    .iter()
                    .position(|parameter| parameter == local)
                else {
                    return Err(JvmCodegenError::InvalidParameterReference {
                        tree: receiver_id,
                        symbol: Some(*local),
                    });
                };
                let all_metadata = unit.metadata(definitions)?;
                let class_metadata = all_metadata
                    .iter()
                    .find(|metadata| metadata.identity().symbol() == caller_class)
                    .ok_or(JvmCodegenError::MissingClassPlan {
                        class: caller_class,
                    })?;
                let method_metadata = class_metadata
                    .methods()
                    .iter()
                    .find(|metadata| metadata.symbol() == caller_method)
                    .ok_or(JvmCodegenError::CallerMethodNotInJvmPlan {
                        class: caller_class,
                        method: caller_method,
                    })?;
                let parameter = method_metadata
                    .parameters()
                    .get(parameter_index)
                    .filter(|parameter| parameter.symbol() == *local)
                    .ok_or(JvmCodegenError::InvalidParameterReference {
                        tree: receiver_id,
                        symbol: Some(*local),
                    })?;
                let class_identities = all_metadata
                    .iter()
                    .map(|metadata| metadata.identity().clone())
                    .collect::<Vec<_>>();
                let found = crate::reference::project_method_signature_type(
                    unit.store,
                    local_type,
                    definitions,
                    &class_identities,
                    Some(unit),
                )?;
                if found != parameter.ty() {
                    return Err(JvmCodegenError::ParameterReferenceTypeMismatch {
                        tree: receiver_id,
                        parameter: *local,
                        expected: parameter.ty(),
                        found: Some(found),
                    });
                }
                return match found {
                    JvmType::Reference(class) => Ok(class),
                    JvmType::Int | JvmType::Boolean => Err(unsupported_call(
                        receiver_id,
                        UnsupportedJvmCallShapeKind::UnsupportedReferenceReceiver,
                    )),
                };
            }
            let binding = method_plan
                .locals
                .iter()
                .find(|binding| binding.symbol == *local)
                .ok_or(JvmCodegenError::InvalidLocalReference {
                    tree: receiver_id,
                    symbol: *local,
                })?;
            if symbol.kind != SymbolKind::Local
                || symbol.owner != Some(caller_method)
                || binding.mutable
                || symbol
                    .flags
                    .contains(dotty_core::symbols::SymbolFlags::MUTABLE)
            {
                return Err(unsupported_call(
                    receiver_id,
                    UnsupportedJvmCallShapeKind::MutableOrUnplannedReferenceReceiver,
                ));
            }
            project_reference_class(unit, local_type, definitions).ok_or_else(|| {
                unsupported_call(
                    receiver_id,
                    UnsupportedJvmCallShapeKind::UnsupportedReferenceReceiver,
                )
            })
        }
        TreeKind::Select(select) => {
            let qualifier = tree(unit.store, unit.typed_ast, select.qualifier)?;
            if !matches!(
                qualifier.kind,
                TreeKind::This(dotty_core::ast::This { qual: None })
            ) || !matches!(
                unit.store.types.try_get(qualifier.ty),
                Some(Type::ThisType { class }) if *class == caller_class
            ) {
                return Err(unsupported_call(
                    receiver_id,
                    UnsupportedJvmCallShapeKind::UnsupportedReferenceReceiver,
                ));
            }
            let Some(Type::TermRef {
                prefix,
                target: TermRefTarget::Symbol(field),
            }) = unit.store.types.try_get(receiver.ty)
            else {
                return Err(unsupported_call(
                    receiver_id,
                    UnsupportedJvmCallShapeKind::UnsupportedReferenceReceiver,
                ));
            };
            planned_current_field_receiver_class(
                unit,
                caller_class,
                receiver_id,
                *field,
                *prefix,
                definitions,
            )
        }
        TreeKind::Apply(apply) if constructor_application(unit, apply.function)? => unit
            .resolve_constructor_call(caller_class, caller_method, receiver_id, definitions)
            .map(|target| target.owner_class()),
        TreeKind::Apply(_) => {
            let target = resolve_instance_call_recursive(
                unit,
                caller_class,
                caller_method,
                receiver_id,
                definitions,
                context,
                JvmInvocationKind::Virtual,
            )?;
            match target.result() {
                JvmReturnType::Value(JvmType::Reference(class)) => Ok(class),
                JvmReturnType::Value(JvmType::Int | JvmType::Boolean) | JvmReturnType::Void => {
                    Err(unsupported_call(
                        receiver_id,
                        UnsupportedJvmCallShapeKind::UnsupportedReferenceReceiver,
                    ))
                }
            }
        }
        _ => Err(unsupported_call(
            receiver_id,
            UnsupportedJvmCallShapeKind::UnsupportedReferenceReceiver,
        )),
    }
}

fn constructor_application(
    unit: &JvmCompilationUnit<'_>,
    function_id: TreeId<Typed>,
) -> Result<bool, JvmCodegenError> {
    let function = tree(unit.store, unit.typed_ast, function_id)?;
    let TreeKind::Select(select) = &function.kind else {
        return Ok(false);
    };
    Ok(matches!(
        tree(unit.store, unit.typed_ast, select.qualifier)?.kind,
        TreeKind::New(_)
    ))
}

fn planned_current_field_receiver_class(
    unit: &JvmCompilationUnit<'_>,
    caller_class: SymbolId,
    receiver: TreeId<Typed>,
    field: SymbolId,
    prefix: TypeId,
    definitions: &Definitions,
) -> Result<SymbolId, JvmCodegenError> {
    let Some(field_symbol) = unit
        .store
        .symbols
        .contains(field)
        .then(|| unit.store.symbols.get(field))
    else {
        return Err(JvmCodegenError::UnknownJvmFieldReference {
            tree: receiver,
            field,
        });
    };
    if field_symbol.kind != SymbolKind::Field || field_symbol.owner != Some(caller_class) {
        return Err(unsupported_call(
            receiver,
            UnsupportedJvmCallShapeKind::UnsupportedReferenceReceiver,
        ));
    }
    if !matches!(
        unit.store.types.try_get(prefix),
        Some(Type::ThisType { class }) if *class == caller_class
    ) {
        return Err(unsupported_call(
            receiver,
            UnsupportedJvmCallShapeKind::UnsupportedReceiverPrefix,
        ));
    }
    let class_metadata = unit
        .metadata(definitions)?
        .into_iter()
        .find(|metadata| metadata.identity().symbol() == caller_class)
        .ok_or(JvmCodegenError::MissingClassPlan {
            class: caller_class,
        })?;
    let field_metadata = class_metadata
        .fields()
        .iter()
        .find(|metadata| metadata.symbol() == field)
        .ok_or(JvmCodegenError::FieldNotInJvmPlan {
            tree: receiver,
            class: caller_class,
            field,
        })?;
    match field_metadata.value_type() {
        JvmType::Reference(class) => Ok(class),
        JvmType::Int | JvmType::Boolean => Err(unsupported_call(
            receiver,
            UnsupportedJvmCallShapeKind::UnsupportedReferenceReceiver,
        )),
    }
}

fn prefix_matches_receiver(
    unit: &JvmCompilationUnit<'_>,
    prefix: TypeId,
    receiver_type: TypeId,
    receiver_class: SymbolId,
    method_owner: SymbolId,
    definitions: &Definitions,
) -> bool {
    if prefix == receiver_type {
        return true;
    }
    matches!(
        project_reference_class(unit, prefix, definitions),
        Some(class) if class == receiver_class || class == method_owner
    )
}

fn project_reference_class(
    unit: &JvmCompilationUnit<'_>,
    ty: TypeId,
    definitions: &Definitions,
) -> Option<SymbolId> {
    let projected = crate::reference::project_type(unit, ty, definitions)
        .or_else(|_| crate::reference::project_constructor_type(unit, ty, definitions))
        .ok()?;
    match projected {
        JvmType::Reference(class) => Some(class),
        _ => None,
    }
}

fn tree<'a>(
    store: &SemanticStore,
    typed_ast: &'a dotty_core::ast::TypedAst,
    id: TreeId<Typed>,
) -> Result<&'a Tree<Typed>, JvmCodegenError> {
    let tree = typed_ast
        .try_get(id)
        .ok_or(JvmCodegenError::UnsupportedTypedExpression {
            tree: id,
            node: "missing tree",
        })?;
    super::validate_tree_type(store, id, tree)?;
    Ok(tree)
}

fn project_result_type(
    unit: &JvmCompilationUnit<'_>,
    ty: TypeId,
    definitions: &Definitions,
) -> Option<JvmReturnType> {
    let store = unit.store;
    match store.types.try_get(ty)? {
        Type::Constant(Constant::Int(_)) => Some(JvmReturnType::Value(JvmType::Int)),
        Type::Constant(Constant::Boolean(_)) => Some(JvmReturnType::Value(JvmType::Boolean)),
        Type::Constant(Constant::Unit) => Some(JvmReturnType::Void),
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } if Some(*symbol) == type_symbol(store, definitions.int) => {
            Some(JvmReturnType::Value(JvmType::Int))
        }
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } if Some(*symbol) == type_symbol(store, definitions.boolean) => {
            Some(JvmReturnType::Value(JvmType::Boolean))
        }
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } if Some(*symbol) == type_symbol(store, definitions.unit) => Some(JvmReturnType::Void),
        Type::TypeRef { .. } => match crate::reference::project_type(unit, ty, definitions)
            .or_else(|_| crate::reference::project_constructor_type(unit, ty, definitions))
            .ok()?
        {
            JvmType::Reference(class) => Some(JvmReturnType::Value(JvmType::Reference(class))),
            JvmType::Int | JvmType::Boolean => None,
        },
        _ => None,
    }
}

fn type_symbol(store: &SemanticStore, ty: TypeId) -> Option<SymbolId> {
    match store.types.try_get(ty)? {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => Some(*symbol),
        _ => None,
    }
}

fn unsupported_call(tree: TreeId<Typed>, kind: UnsupportedJvmCallShapeKind) -> JvmCodegenError {
    JvmCodegenError::UnsupportedJvmCallShape { tree, kind }
}
