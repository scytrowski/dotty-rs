//! Resolution of the canonical source `AnyRef.eq/ne` identity operations.

use dotty_core::ast::{ApplyKind, TreeKind, Typed, TypedAst};
use dotty_core::definitions::Definitions;
use dotty_core::ids::{TreeId, TypeId};
use dotty_core::store::SemanticStore;
use dotty_core::types::{TermRefTarget, Type};

/// One canonical source reference-identity operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmReferenceIdentityOperator {
    /// Reference equality, lowered with `if_acmpeq`.
    Eq,
    /// Reference inequality, lowered with `if_acmpne`.
    Ne,
}

/// Why a typed application claiming a canonical `AnyRef.eq/ne` is malformed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JvmReferenceIdentityError {
    MissingTree { tree: TreeId<Typed> },
    InvalidTreeType { tree: TreeId<Typed>, ty: TypeId },
    UnsupportedFunctionShape { tree: TreeId<Typed> },
    InvalidBuiltinRegistry { tree: TreeId<Typed> },
    WrongApplyKind { tree: TreeId<Typed> },
    WrongArity { tree: TreeId<Typed>, found: usize },
    WrongResultType { tree: TreeId<Typed>, found: TypeId },
}

/// Resolve only a Typer-selected canonical `AnyRef.eq` or `AnyRef.ne` call.
///
/// Ordinary methods that happen to be named `eq` or `ne` return `Ok(None)`;
/// the method's spelling is never used to establish intrinsic identity.
pub fn resolve_reference_identity_operator(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    expression: TreeId<Typed>,
    definitions: &Definitions,
) -> Result<Option<JvmReferenceIdentityOperator>, JvmReferenceIdentityError> {
    let root = get_tree(store, typed_ast, expression)?;
    let TreeKind::Apply(application) = &root.kind else {
        return Ok(None);
    };

    let function = get_tree(store, typed_ast, application.function)?;
    let method = match store.types.try_get(function.ty) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(method),
            ..
        }) => *method,
        _ => return Ok(None),
    };

    let Some(ids) = definitions.any_ref_identity_operator_ids() else {
        return Ok(None);
    };
    let operator = if method == ids.eq {
        JvmReferenceIdentityOperator::Eq
    } else if method == ids.ne {
        JvmReferenceIdentityOperator::Ne
    } else {
        return Ok(None);
    };
    if definitions.any_ref_identity_operator_symbols(store) != Some(ids) {
        return Err(JvmReferenceIdentityError::InvalidBuiltinRegistry { tree: expression });
    }
    let TreeKind::Select(selection) = &function.kind else {
        return Err(JvmReferenceIdentityError::UnsupportedFunctionShape {
            tree: application.function,
        });
    };
    get_tree(store, typed_ast, selection.qualifier)?;
    for argument in &application.args {
        get_tree(store, typed_ast, *argument)?;
    }
    if application.kind != ApplyKind::Regular {
        return Err(JvmReferenceIdentityError::WrongApplyKind { tree: expression });
    }
    if application.args.len() != 1 {
        return Err(JvmReferenceIdentityError::WrongArity {
            tree: expression,
            found: application.args.len(),
        });
    }
    if root.ty != definitions.boolean {
        return Err(JvmReferenceIdentityError::WrongResultType {
            tree: expression,
            found: root.ty,
        });
    }
    Ok(Some(operator))
}

fn get_tree<'a>(
    store: &SemanticStore,
    typed_ast: &'a TypedAst,
    tree: TreeId<Typed>,
) -> Result<&'a dotty_core::ast::Tree<Typed>, JvmReferenceIdentityError> {
    let node = typed_ast
        .try_get(tree)
        .ok_or(JvmReferenceIdentityError::MissingTree { tree })?;
    if store
        .types
        .try_get(node.ty)
        .is_none_or(|ty| matches!(ty, Type::NoType | Type::NoPrefix | Type::Error(_)))
    {
        return Err(JvmReferenceIdentityError::InvalidTreeType { tree, ty: node.ty });
    }
    Ok(node)
}
