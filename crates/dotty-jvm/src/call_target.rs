//! Canonical projection of supported typed calls to same-class JVM targets.

use dotty_core::ast::{ApplyKind, Tree, TreeKind, Typed};
use dotty_core::definitions::Definitions;
use dotty_core::ids::{SymbolId, TreeId, TypeId};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolKind;
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
    let class_plan = unit
        .classes
        .iter()
        .find(|plan| plan.class == caller_class)
        .ok_or(JvmCodegenError::MissingClassPlan {
            class: caller_class,
        })?;
    if !class_plan
        .methods
        .iter()
        .any(|plan| plan.method == caller_method)
    {
        return Err(JvmCodegenError::CallerMethodNotInJvmPlan {
            class: caller_class,
            method: caller_method,
        });
    }

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
    let is_ident = match &function_tree.kind {
        TreeKind::Ident(_) => true,
        TreeKind::Select(select) => {
            let receiver = tree(unit.store, unit.typed_ast, select.qualifier)?;
            if !matches!(receiver.kind, TreeKind::This(_)) {
                return Err(unsupported_call(
                    function_id,
                    UnsupportedJvmCallShapeKind::NonThisReceiver,
                ));
            }
            if !matches!(
                unit.store.types.try_get(receiver.ty),
                Some(Type::ThisType { class }) if *class == caller_class
            ) {
                return Err(unsupported_call(
                    function_id,
                    UnsupportedJvmCallShapeKind::NonCurrentThis,
                ));
            }
            false
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
    let supported_prefix = match unit.store.types.try_get(prefix) {
        Some(Type::NoPrefix) => is_ident,
        Some(Type::ThisType { class }) => *class == caller_class,
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
    if method_symbol.owner != Some(caller_class) {
        return Err(JvmCodegenError::WrongOwner {
            symbol: method,
            expected: caller_class,
            actual: method_symbol.owner,
        });
    }
    if !class_plan
        .methods
        .iter()
        .any(|method_plan| method_plan.method == method)
    {
        return Err(JvmCodegenError::MethodNotInJvmPlan {
            tree: function_id,
            class: caller_class,
            method,
        });
    }

    let class_info = super::identity::class_info(unit.store, caller_class)?;
    if !unit
        .store
        .scopes
        .get(class_info.declarations)
        .entered_symbols()
        .any(|declared| declared == method)
    {
        return Err(JvmCodegenError::MethodNotInClassDeclarations {
            class: caller_class,
            method,
        });
    }

    let class_metadata = class_plan.metadata(unit.store, definitions)?;
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
    })
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
