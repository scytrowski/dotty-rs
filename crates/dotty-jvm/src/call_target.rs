//! Canonical projection of supported typed calls to planned JVM targets.

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
    let (receiver_tree, receiver_class) = match &function_tree.kind {
        TreeKind::Ident(_) => (None, caller_class),
        TreeKind::Select(select) => {
            let receiver = select.qualifier;
            let receiver_class = resolve_receiver_class(
                unit,
                caller_class,
                caller_method,
                caller_method_plan,
                receiver,
                definitions,
            )?;
            (Some(receiver), receiver_class)
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
    let supported_prefix = match (receiver, unit.store.types.try_get(prefix)) {
        (None, Some(Type::NoPrefix)) => true,
        (None, Some(Type::ThisType { class })) => *class == caller_class,
        (Some(_receiver), Some(Type::ThisType { class })) => {
            *class == caller_class && receiver_class == caller_class
        }
        (Some(receiver), _) => {
            prefix_matches_receiver(unit, prefix, receiver.ty, receiver_class, definitions)
        }
        _ => false,
    };
    if !supported_prefix {
        return Err(unsupported_call(
            function_id,
            UnsupportedJvmCallShapeKind::UnsupportedReceiverPrefix,
        ));
    }
    if !unit.store.symbols.contains(method) {
        return Err(JvmCodegenError::UnknownMethod { method });
    }
    let method_symbol = unit.store.symbols.get(method);
    if method_symbol.kind != SymbolKind::Method {
        return Err(JvmCodegenError::UnsupportedSymbolKind {
            symbol: method,
            expected: SymbolRole::Method,
            found: method_symbol.kind,
        });
    }
    if method_symbol.owner != Some(receiver_class) {
        return Err(JvmCodegenError::WrongOwner {
            symbol: method,
            expected: receiver_class,
            actual: method_symbol.owner,
        });
    }
    let owner_plan = unit
        .classes
        .iter()
        .find(|plan| plan.class == receiver_class)
        .ok_or(JvmCodegenError::MissingClassPlan {
            class: receiver_class,
        })?;
    if !owner_plan
        .methods
        .iter()
        .any(|method_plan| method_plan.method == method)
    {
        return Err(JvmCodegenError::MethodNotInJvmPlan {
            tree: function_id,
            class: receiver_class,
            method,
        });
    }

    match method_symbol.visibility {
        Visibility::Public => {}
        Visibility::Private if receiver_class == caller_class => {}
        visibility => {
            return Err(JvmCodegenError::UnsupportedJvmVisibility {
                symbol: method,
                visibility,
            });
        }
    }

    let class_info = super::identity::class_info(unit.store, receiver_class)?;
    if !unit
        .store
        .scopes
        .get(class_info.declarations)
        .entered_symbols()
        .any(|declared| declared == method)
    {
        return Err(JvmCodegenError::MethodNotInClassDeclarations {
            class: receiver_class,
            method,
        });
    }

    let class_metadata =
        owner_plan.metadata_with_typed_ast(unit.store, unit.typed_ast, definitions)?;
    let method_metadata = class_metadata
        .methods()
        .iter()
        .find(|metadata| metadata.symbol() == method)
        .ok_or(JvmCodegenError::MethodNotInJvmPlan {
            tree: function_id,
            class: caller_class,
            method,
        })?;
    let found = project_result_type(unit.store, call_tree.ty, definitions);
    let expected = method_metadata.descriptor().result();
    if found != Some(expected) {
        return Err(JvmCodegenError::JvmCallResultTypeMismatch {
            tree: call,
            method,
            expected,
            found,
        });
    }

    Ok(JvmCallTarget {
        owner_internal_name: class_metadata.identity().internal_name().to_owned(),
        method_name: method_metadata.name().to_owned(),
        descriptor: method_metadata.descriptor().clone(),
        method,
        invocation: JvmInvocationKind::Virtual,
        result: expected,
        receiver_tree,
        receiver_class,
    })
}

fn resolve_receiver_class(
    unit: &JvmCompilationUnit<'_>,
    caller_class: SymbolId,
    caller_method: SymbolId,
    method_plan: &crate::JvmMethodPlan,
    receiver_id: TreeId<Typed>,
    definitions: &Definitions,
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
                target: TermRefTarget::Symbol(local),
                ..
            }) = unit.store.types.try_get(receiver.ty)
            else {
                return Err(unsupported_call(
                    receiver_id,
                    UnsupportedJvmCallShapeKind::UnsupportedFunctionTree,
                ));
            };
            let symbol = unit
                .store
                .symbols
                .contains(*local)
                .then(|| unit.store.symbols.get(*local))
                .ok_or(JvmCodegenError::InvalidLocalReference {
                    tree: receiver_id,
                    symbol: *local,
                })?;
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
            let local_type = match unit.store.symbols.info(*local) {
                SymbolInfo::Complete(ty) => *ty,
                SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
                    return Err(JvmCodegenError::MissingSemanticInfo { symbol: *local });
                }
                SymbolInfo::Error => {
                    return Err(JvmCodegenError::ErrorSemanticInfo { symbol: *local });
                }
            };
            project_reference_class(unit, local_type, definitions).ok_or_else(|| {
                unsupported_call(
                    receiver_id,
                    UnsupportedJvmCallShapeKind::UnsupportedReferenceReceiver,
                )
            })
        }
        TreeKind::Apply(apply) if constructor_application(unit, apply.function)? => unit
            .resolve_constructor_call(caller_class, caller_method, receiver_id, definitions)
            .map(|target| target.owner_class()),
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

fn prefix_matches_receiver(
    unit: &JvmCompilationUnit<'_>,
    prefix: TypeId,
    receiver_type: TypeId,
    receiver_class: SymbolId,
    definitions: &Definitions,
) -> bool {
    if prefix == receiver_type {
        return true;
    }
    project_reference_class(unit, prefix, definitions) == Some(receiver_class)
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
    store: &SemanticStore,
    ty: TypeId,
    definitions: &Definitions,
) -> Option<JvmReturnType> {
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
