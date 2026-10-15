//! Read-only resolution of canonical Scala `Boolean` operations.

use dotty_core::ast::{ApplyKind, Tree, TreeKind, Typed, TypedAst};
use dotty_core::definitions::{BooleanPrimitiveOperatorSymbols, Definitions};
use dotty_core::ids::{SymbolId, TreeId, TypeId};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolFlags, SymbolInfo, SymbolKind};
use dotty_core::types::{MethodKind, TermRefTarget, Type};

/// A canonical Boolean operation and its operand-evaluation behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmBooleanIntrinsic {
    Not,
    And,
    Or,
    Xor,
    ShortCircuitAnd,
    ShortCircuitOr,
    Equal,
    NotEqual,
}

/// Evaluation behavior for a Boolean intrinsic's right operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmBooleanRhsEvaluation {
    None,
    Strict,
    ShortCircuit,
}

impl JvmBooleanIntrinsic {
    /// Returns whether and when the operation evaluates a right operand.
    pub const fn rhs_evaluation(self) -> JvmBooleanRhsEvaluation {
        match self {
            Self::Not => JvmBooleanRhsEvaluation::None,
            Self::ShortCircuitAnd | Self::ShortCircuitOr => JvmBooleanRhsEvaluation::ShortCircuit,
            Self::And | Self::Or | Self::Xor | Self::Equal | Self::NotEqual => {
                JvmBooleanRhsEvaluation::Strict
            }
        }
    }
}

/// Why a typed tree that selects a canonical Boolean member is malformed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JvmBooleanIntrinsicError {
    InvalidBuiltinType {
        ty: TypeId,
    },
    MissingTree {
        tree: TreeId<Typed>,
    },
    InvalidTreeType {
        tree: TreeId<Typed>,
        ty: TypeId,
    },
    UnsupportedTreeShape {
        tree: TreeId<Typed>,
    },
    UnsupportedFunctionShape {
        tree: TreeId<Typed>,
    },
    UnresolvedFunctionTarget {
        tree: TreeId<Typed>,
    },
    UnsupportedFunctionPrefix {
        tree: TreeId<Typed>,
        prefix: TypeId,
    },
    FunctionPrefixMismatch {
        tree: TreeId<Typed>,
        prefix: TypeId,
        receiver: TypeId,
    },
    UnknownMethod {
        tree: TreeId<Typed>,
        method: SymbolId,
    },
    MissingBuiltinRegistry {
        tree: TreeId<Typed>,
        method: SymbolId,
    },
    WrongMethodKind {
        tree: TreeId<Typed>,
        method: SymbolId,
    },
    WrongOwner {
        tree: TreeId<Typed>,
        method: SymbolId,
        expected: SymbolId,
        actual: Option<SymbolId>,
    },
    WrongSignature {
        tree: TreeId<Typed>,
        method: SymbolId,
    },
    IncompatibleMethodFlags {
        tree: TreeId<Typed>,
        method: SymbolId,
        flags: SymbolFlags,
    },
    WrongArity {
        tree: TreeId<Typed>,
        expected: usize,
        found: usize,
    },
    WrongOperandType {
        tree: TreeId<Typed>,
        operand: TreeId<Typed>,
    },
    WrongResultType {
        tree: TreeId<Typed>,
        expected: TypeId,
        found: TypeId,
    },
}

/// Resolves a typed Boolean expression only when its selected method is one
/// of the canonical Boolean symbols and its tree, signature, and semantic
/// types agree. It reads the store and typed AST without mutation or bytecode
/// emission. Ordinary calls to user-defined methods return `Ok(None)`.
pub fn resolve_boolean_intrinsic(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    expression: TreeId<Typed>,
    definitions: &Definitions,
) -> Result<Option<JvmBooleanIntrinsic>, JvmBooleanIntrinsicError> {
    let boolean_class = builtin_type_symbol(store, definitions.boolean).ok_or(
        JvmBooleanIntrinsicError::InvalidBuiltinType {
            ty: definitions.boolean,
        },
    )?;
    if !store.symbols.contains(boolean_class) {
        return Err(JvmBooleanIntrinsicError::InvalidBuiltinType {
            ty: definitions.boolean,
        });
    }

    let root = get_tree(store, typed_ast, expression)?;
    let (function_id, args, apply_kind) = match &root.kind {
        TreeKind::Apply(apply) => (
            Some(apply.function),
            apply.args.as_slice(),
            Some(apply.kind),
        ),
        TreeKind::Select(_) => (None, &[][..], None),
        TreeKind::TypeApply(apply) => {
            let function = get_tree(store, typed_ast, apply.function)?;
            let Some((method, _)) = selected_method(store, function.ty) else {
                return Err(JvmBooleanIntrinsicError::UnresolvedFunctionTarget {
                    tree: apply.function,
                });
            };
            let Some(_intrinsic) =
                resolve_registered_method(store, definitions, boolean_class, expression, method)?
            else {
                return Ok(None);
            };
            return Err(match function.kind {
                TreeKind::Select(_) => {
                    JvmBooleanIntrinsicError::UnsupportedTreeShape { tree: expression }
                }
                _ => JvmBooleanIntrinsicError::UnsupportedFunctionShape {
                    tree: apply.function,
                },
            });
        }
        _ => return Ok(None),
    };

    let (method, prefix, function_id) = if let Some(function_id) = function_id {
        let function = get_tree(store, typed_ast, function_id)?;
        let Some((method, prefix)) = selected_method(store, function.ty) else {
            return Err(JvmBooleanIntrinsicError::UnresolvedFunctionTarget { tree: function_id });
        };
        if !matches!(function.kind, TreeKind::Select(_)) {
            let Some(_intrinsic) =
                resolve_registered_method(store, definitions, boolean_class, expression, method)?
            else {
                return Ok(None);
            };
            return Err(JvmBooleanIntrinsicError::UnsupportedFunctionShape { tree: function_id });
        }
        (method, prefix, Some(function_id))
    } else {
        let TreeKind::Select(_) = root.kind else {
            unreachable!("only a Select has no function id")
        };
        let Some((method, prefix)) = selected_method(store, root.ty) else {
            return Err(JvmBooleanIntrinsicError::UnresolvedFunctionTarget { tree: expression });
        };
        (method, prefix, None)
    };

    let Some(intrinsic) =
        resolve_registered_method(store, definitions, boolean_class, expression, method)?
    else {
        return Ok(None);
    };

    let expected_arity = if intrinsic == JvmBooleanIntrinsic::Not {
        0
    } else {
        1
    };
    if args.len() != expected_arity {
        return Err(JvmBooleanIntrinsicError::WrongArity {
            tree: expression,
            expected: expected_arity,
            found: args.len(),
        });
    }
    if apply_kind.is_some_and(|kind| kind != ApplyKind::Regular) {
        return Err(JvmBooleanIntrinsicError::UnsupportedTreeShape { tree: expression });
    }
    if function_id.is_none() && intrinsic != JvmBooleanIntrinsic::Not {
        return Err(JvmBooleanIntrinsicError::WrongArity {
            tree: expression,
            expected: 1,
            found: 0,
        });
    }
    if function_id.is_some() && intrinsic == JvmBooleanIntrinsic::Not {
        return Err(JvmBooleanIntrinsicError::WrongArity {
            tree: expression,
            expected: 0,
            found: args.len(),
        });
    }

    let Some(function_id) = function_id else {
        let TreeKind::Select(select) = &root.kind else {
            unreachable!("a unary Boolean intrinsic is a Select")
        };
        validate_receiver(
            store,
            typed_ast,
            definitions,
            expression,
            expression,
            select.qualifier,
            prefix,
        )?;
        return Ok(Some(intrinsic));
    };

    let TreeKind::Select(select) = &get_tree(store, typed_ast, function_id)?.kind else {
        unreachable!("selected function checked above")
    };
    validate_receiver(
        store,
        typed_ast,
        definitions,
        expression,
        function_id,
        select.qualifier,
        prefix,
    )?;
    let argument = get_tree(store, typed_ast, args[0])?;
    if primitive_semantic_type(store, argument.ty, definitions) != Some(definitions.boolean) {
        return Err(JvmBooleanIntrinsicError::WrongOperandType {
            tree: expression,
            operand: args[0],
        });
    }
    if root.ty != definitions.boolean {
        return Err(JvmBooleanIntrinsicError::WrongResultType {
            tree: expression,
            expected: definitions.boolean,
            found: root.ty,
        });
    }
    Ok(Some(intrinsic))
}

fn validate_receiver(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    definitions: &Definitions,
    expression: TreeId<Typed>,
    function: TreeId<Typed>,
    qualifier: TreeId<Typed>,
    prefix: TypeId,
) -> Result<(), JvmBooleanIntrinsicError> {
    let receiver = get_tree(store, typed_ast, qualifier)?;
    if primitive_semantic_type(store, receiver.ty, definitions) != Some(definitions.boolean) {
        return Err(JvmBooleanIntrinsicError::WrongOperandType {
            tree: expression,
            operand: qualifier,
        });
    }
    if store
        .types
        .try_get(prefix)
        .is_none_or(|ty| matches!(ty, Type::NoType | Type::NoPrefix | Type::Error(_)))
    {
        return Err(JvmBooleanIntrinsicError::UnsupportedFunctionPrefix {
            tree: function,
            prefix,
        });
    }
    if prefix != receiver.ty {
        return Err(JvmBooleanIntrinsicError::FunctionPrefixMismatch {
            tree: function,
            prefix,
            receiver: receiver.ty,
        });
    }
    Ok(())
}

fn get_tree<'a>(
    store: &SemanticStore,
    typed_ast: &'a TypedAst,
    id: TreeId<Typed>,
) -> Result<&'a Tree<Typed>, JvmBooleanIntrinsicError> {
    let tree = typed_ast
        .try_get(id)
        .ok_or(JvmBooleanIntrinsicError::MissingTree { tree: id })?;
    if store
        .types
        .try_get(tree.ty)
        .is_none_or(|ty| matches!(ty, Type::NoType | Type::NoPrefix | Type::Error(_)))
    {
        return Err(JvmBooleanIntrinsicError::InvalidTreeType {
            tree: id,
            ty: tree.ty,
        });
    }
    Ok(tree)
}

fn selected_method(store: &SemanticStore, ty: TypeId) -> Option<(SymbolId, TypeId)> {
    match store.types.try_get(ty)? {
        Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(method),
        } => Some((*method, *prefix)),
        _ => None,
    }
}

fn builtin_type_symbol(store: &SemanticStore, ty: TypeId) -> Option<SymbolId> {
    match store.types.try_get(ty)? {
        Type::TypeRef {
            target: dotty_core::types::TypeRefTarget::Symbol(symbol),
            ..
        } => Some(*symbol),
        _ => None,
    }
}

fn resolve_registered_method(
    store: &SemanticStore,
    definitions: &Definitions,
    boolean_class: SymbolId,
    tree: TreeId<Typed>,
    method: SymbolId,
) -> Result<Option<JvmBooleanIntrinsic>, JvmBooleanIntrinsicError> {
    if !store.symbols.contains(method) {
        return Err(JvmBooleanIntrinsicError::UnknownMethod { tree, method });
    }
    let Some(intrinsic) =
        registered_operation(definitions.boolean_primitive_operator_ids(), method)
    else {
        return Ok(None);
    };
    validate_method_signature(store, definitions, boolean_class, tree, method, intrinsic)?;
    if definitions
        .boolean_primitive_operator_symbols(store)
        .is_none()
    {
        return Err(JvmBooleanIntrinsicError::MissingBuiltinRegistry { tree, method });
    }
    Ok(Some(intrinsic))
}

fn registered_operation(
    registry: Option<BooleanPrimitiveOperatorSymbols>,
    method: SymbolId,
) -> Option<JvmBooleanIntrinsic> {
    let symbols = registry?;
    symbols
        .all()
        .iter()
        .position(|candidate| *candidate == method)
        .map(|index| match index {
            0 => JvmBooleanIntrinsic::Not,
            1 => JvmBooleanIntrinsic::ShortCircuitAnd,
            2 => JvmBooleanIntrinsic::ShortCircuitOr,
            3 => JvmBooleanIntrinsic::And,
            4 => JvmBooleanIntrinsic::Or,
            5 => JvmBooleanIntrinsic::Xor,
            6 => JvmBooleanIntrinsic::Equal,
            7 => JvmBooleanIntrinsic::NotEqual,
            _ => unreachable!("the Boolean operator registry has exactly eight entries"),
        })
}

fn validate_method_signature(
    store: &SemanticStore,
    definitions: &Definitions,
    boolean_class: SymbolId,
    tree: TreeId<Typed>,
    method: SymbolId,
    intrinsic: JvmBooleanIntrinsic,
) -> Result<(), JvmBooleanIntrinsicError> {
    let symbol = store.symbols.get(method);
    if symbol.kind != SymbolKind::Method {
        return Err(JvmBooleanIntrinsicError::WrongMethodKind { tree, method });
    }
    if symbol.owner != Some(boolean_class) {
        return Err(JvmBooleanIntrinsicError::WrongOwner {
            tree,
            method,
            expected: boolean_class,
            actual: symbol.owner,
        });
    }
    let SymbolInfo::Complete(info) = symbol.info else {
        return Err(JvmBooleanIntrinsicError::WrongSignature { tree, method });
    };
    let Some(ty) = store.types.try_get(info) else {
        return Err(JvmBooleanIntrinsicError::WrongSignature { tree, method });
    };

    const INCOMPATIBLE_FLAGS: SymbolFlags = SymbolFlags::STATIC
        .union(SymbolFlags::EXTENSION)
        .union(SymbolFlags::ERASED);
    if !symbol.flags.intersection(INCOMPATIBLE_FLAGS).is_empty() {
        return Err(JvmBooleanIntrinsicError::IncompatibleMethodFlags {
            tree,
            method,
            flags: symbol.flags,
        });
    }
    if intrinsic == JvmBooleanIntrinsic::Not && info == definitions.boolean {
        return Ok(());
    }
    let Type::Method(signature) = ty else {
        return Err(JvmBooleanIntrinsicError::WrongSignature { tree, method });
    };
    let expected_arity = if intrinsic == JvmBooleanIntrinsic::Not {
        0
    } else {
        1
    };
    if signature.kind != MethodKind::Plain
        || signature.params.len() != expected_arity
        || signature
            .params
            .iter()
            .any(|param| param.ty != definitions.boolean || param.erased || param.varargs)
        || signature.result != definitions.boolean
    {
        return Err(JvmBooleanIntrinsicError::WrongSignature { tree, method });
    }
    Ok(())
}

fn primitive_semantic_type(
    store: &SemanticStore,
    ty: TypeId,
    definitions: &Definitions,
) -> Option<TypeId> {
    match store.types.try_get(ty)? {
        Type::Constant(dotty_core::types::Constant::Boolean(_)) => Some(definitions.boolean),
        Type::TypeRef {
            target: dotty_core::types::TypeRefTarget::Symbol(symbol),
            ..
        } if Some(*symbol) == builtin_type_symbol(store, definitions.boolean) => {
            Some(definitions.boolean)
        }
        Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        } if store.symbols.contains(*symbol) => match store.symbols.info(*symbol) {
            SymbolInfo::Complete(info) => primitive_boolean_value_type(store, *info, definitions),
            _ => None,
        },
        _ => None,
    }
}

fn primitive_boolean_value_type(
    store: &SemanticStore,
    ty: TypeId,
    definitions: &Definitions,
) -> Option<TypeId> {
    match store.types.try_get(ty)? {
        Type::Constant(dotty_core::types::Constant::Boolean(_)) => Some(definitions.boolean),
        Type::TypeRef {
            target: dotty_core::types::TypeRefTarget::Symbol(symbol),
            ..
        } if Some(*symbol) == builtin_type_symbol(store, definitions.boolean) => {
            Some(definitions.boolean)
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "boolean_ops_tests.rs"]
mod tests;
