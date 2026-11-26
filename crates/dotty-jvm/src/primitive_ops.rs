//! Read-only resolution of the bounded canonical Scala `Int` operators.

use dotty_core::ast::{ApplyKind, Tree, TreeKind, Typed, TypedAst};
use dotty_core::definitions::{Definitions, IntPrimitiveOperatorSymbols};
use dotty_core::ids::{SymbolId, TreeId, TypeId};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolKind};
use dotty_core::types::{Constant, MethodKind, TermRefTarget, Type, TypeRefTarget};

/// A canonical Scala `Int` operation that can later map to JVM int-like ops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmIntIntrinsic {
    /// 32-bit integer addition with JVM wrapping semantics.
    IntAdd,
    /// 32-bit integer subtraction with JVM wrapping semantics.
    IntSubtract,
    /// 32-bit integer multiplication with JVM wrapping semantics.
    IntMultiply,
    /// 32-bit integer division using JVM idiv semantics.
    IntDivide,
    /// 32-bit integer remainder using JVM irem semantics.
    IntRemainder,
    /// Bitwise conjunction of two 32-bit integer values.
    IntBitwiseAnd,
    /// Signed integer less-than comparison producing Boolean.
    IntLessThan,
    /// Signed integer less-than-or-equal comparison producing Boolean.
    IntLessOrEqual,
    /// Signed integer greater-than comparison producing Boolean.
    IntGreaterThan,
    /// Signed integer greater-than-or-equal comparison producing Boolean.
    IntGreaterOrEqual,
}

/// Why a typed tree that claims a canonical `Int` call is malformed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JvmIntIntrinsicError {
    /// The supplied builtin type does not identify the required canonical class.
    InvalidBuiltinType {
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// A referenced tree is absent from the supplied typed arena.
    MissingTree {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
    },
    /// A tree has a missing, unfilled, or sentinel semantic type.
    InvalidTreeType {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Semantic type identity in the same session type arena.
        ty: TypeId,
    },
    /// The call is outside the supported intrinsic application shape.
    UnsupportedCallShape {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
    },
    /// The callee is not a supported canonical member selection.
    UnsupportedFunctionShape {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
    },
    /// The callee does not designate a canonical method symbol.
    UnresolvedFunctionTarget {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
    },
    /// The selected method has an unsupported semantic prefix.
    UnsupportedFunctionPrefix {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Semantic prefix through which the member is selected.
        prefix: TypeId,
    },
    /// The method prefix disagrees with the typed receiver.
    FunctionPrefixMismatch {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Semantic prefix through which the member is selected.
        prefix: TypeId,
        /// Semantic receiver type for the member or intrinsic call.
        receiver: TypeId,
    },
    /// The selected method is absent from the supplied semantic store.
    UnknownMethod {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// The canonical operator registry is missing or inconsistent.
    MissingBuiltinRegistry {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// The selected builtin identity is not a method declaration.
    WrongMethodKind {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// The method is owned by a class other than the required builtin owner.
    WrongOwner {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
        /// Required declaration identity in the caller’s symbol table.
        expected: SymbolId,
        /// Observed declaration identity in the caller’s symbol table. None records a missing
        /// binding or owner.
        actual: Option<SymbolId>,
    },
    /// The method signature differs from the canonical intrinsic signature.
    WrongSignature {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Method declaration identity being inspected in the supplied session.
        method: SymbolId,
    },
    /// The application has the wrong number of operands.
    WrongArity {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Required argument or parameter count.
        expected: usize,
        /// Observed argument or parameter count.
        found: usize,
    },
    /// An operand does not have the required canonical primitive type.
    WrongOperandType {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Operand of the prefix or postfix operator.
        operand: TreeId<Typed>,
    },
    /// The application result differs from the intrinsic's required result type.
    WrongResultType {
        /// Typed tree identity in its original session arena.
        tree: TreeId<Typed>,
        /// Required semantic type identity in the caller’s type arena.
        expected: TypeId,
        /// Observed semantic type identity in the caller’s type arena.
        found: TypeId,
    },
}

/// Resolves a typed application only when it selects one of the registered
/// canonical `scala.Int` operator symbols and all of its semantic types agree.
/// This function only reads the store and typed AST; it does not emit code.
/// Ordinary calls to user-defined or otherwise non-registered methods return
/// `Ok(None)`.
pub fn resolve_int_intrinsic(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    call: TreeId<Typed>,
    definitions: &Definitions,
) -> Result<Option<JvmIntIntrinsic>, JvmIntIntrinsicError> {
    let int_class = builtin_type_symbol(store, definitions.int).ok_or(
        JvmIntIntrinsicError::InvalidBuiltinType {
            ty: definitions.int,
        },
    )?;
    if !store.symbols.contains(int_class) {
        return Err(JvmIntIntrinsicError::InvalidBuiltinType {
            ty: definitions.int,
        });
    }
    let boolean_class = builtin_type_symbol(store, definitions.boolean);
    if boolean_class.is_none_or(|class| !store.symbols.contains(class)) {
        return Err(JvmIntIntrinsicError::InvalidBuiltinType {
            ty: definitions.boolean,
        });
    }
    let call_tree = get_tree(store, typed_ast, call)?;
    let (function_id, args, kind) = match &call_tree.kind {
        TreeKind::Apply(apply) => (apply.function, apply.args.as_slice(), apply.kind),
        TreeKind::TypeApply(_) => {
            return Err(JvmIntIntrinsicError::UnsupportedCallShape { tree: call });
        }
        _ => return Ok(None),
    };

    let function = get_tree(store, typed_ast, function_id)?;
    let (method, prefix) = match function.kind {
        TreeKind::Select(_) => selected_method(store, function.ty)
            .ok_or(JvmIntIntrinsicError::UnresolvedFunctionTarget { tree: function_id })?,
        _ => {
            if let Some((method, _)) = selected_method(store, function.ty)
                && registered_operation(definitions.int_primitive_operator_symbols(store), method)
                    .is_some()
            {
                return Err(JvmIntIntrinsicError::UnsupportedFunctionShape { tree: function_id });
            }
            return Ok(None);
        }
    };

    let registry = definitions.int_primitive_operator_symbols(store);
    let Some(intrinsic) = registered_operation(registry, method) else {
        if !store.symbols.contains(method) {
            return Err(JvmIntIntrinsicError::UnknownMethod { tree: call, method });
        }
        let symbol = store.symbols.get(method);
        // A builtin method whose registry has become incomplete cannot be
        // treated as an ordinary source call: its identity was compiler-owned.
        if symbol.origin == dotty_core::symbols::SymbolOrigin::Builtin {
            if symbol.owner != Some(int_class) {
                return Err(JvmIntIntrinsicError::WrongOwner {
                    tree: call,
                    method,
                    expected: int_class,
                    actual: symbol.owner,
                });
            }
            return Err(JvmIntIntrinsicError::MissingBuiltinRegistry { tree: call, method });
        }
        return Ok(None);
    };

    if !store.symbols.contains(method) {
        return Err(JvmIntIntrinsicError::UnknownMethod { tree: call, method });
    }
    if kind != ApplyKind::Regular {
        return Err(JvmIntIntrinsicError::UnsupportedCallShape { tree: call });
    }
    let method_symbol = store.symbols.get(method);
    if method_symbol.kind != SymbolKind::Method {
        return Err(JvmIntIntrinsicError::WrongMethodKind { tree: call, method });
    }
    if method_symbol.owner != Some(int_class) {
        return Err(JvmIntIntrinsicError::WrongOwner {
            tree: call,
            method,
            expected: int_class,
            actual: method_symbol.owner,
        });
    }

    let SymbolInfo::Complete(method_type) = method_symbol.info else {
        return Err(JvmIntIntrinsicError::WrongSignature { tree: call, method });
    };
    let Some(Type::Method(signature)) = store.types.try_get(method_type) else {
        return Err(JvmIntIntrinsicError::WrongSignature { tree: call, method });
    };
    let expected_result = match intrinsic {
        JvmIntIntrinsic::IntLessThan
        | JvmIntIntrinsic::IntLessOrEqual
        | JvmIntIntrinsic::IntGreaterThan
        | JvmIntIntrinsic::IntGreaterOrEqual => definitions.boolean,
        _ => definitions.int,
    };
    if signature.kind != MethodKind::Plain
        || signature.params.len() != 1
        || signature.params[0].ty != definitions.int
        || signature.params[0].erased
        || signature.params[0].varargs
        || signature.result != expected_result
    {
        return Err(JvmIntIntrinsicError::WrongSignature { tree: call, method });
    }
    if args.len() != 1 {
        return Err(JvmIntIntrinsicError::WrongArity {
            tree: call,
            expected: 1,
            found: args.len(),
        });
    }

    let TreeKind::Select(select) = function.kind else {
        unreachable!("selected method checked above")
    };
    let receiver = get_tree(store, typed_ast, select.qualifier)?;
    if primitive_semantic_type(store, receiver.ty, definitions) != Some(definitions.int) {
        return Err(JvmIntIntrinsicError::WrongOperandType {
            tree: call,
            operand: select.qualifier,
        });
    }
    if store
        .types
        .try_get(prefix)
        .is_none_or(|ty| matches!(ty, Type::NoType | Type::NoPrefix | Type::Error(_)))
    {
        return Err(JvmIntIntrinsicError::UnsupportedFunctionPrefix {
            tree: function_id,
            prefix,
        });
    }
    if prefix != receiver.ty {
        return Err(JvmIntIntrinsicError::FunctionPrefixMismatch {
            tree: function_id,
            prefix,
            receiver: receiver.ty,
        });
    }
    let argument = get_tree(store, typed_ast, args[0])?;
    if primitive_semantic_type(store, argument.ty, definitions) != Some(definitions.int) {
        return Err(JvmIntIntrinsicError::WrongOperandType {
            tree: call,
            operand: args[0],
        });
    }
    if primitive_semantic_type(store, call_tree.ty, definitions) != Some(expected_result) {
        return Err(JvmIntIntrinsicError::WrongResultType {
            tree: call,
            expected: expected_result,
            found: call_tree.ty,
        });
    }
    Ok(Some(intrinsic))
}

fn get_tree<'a>(
    store: &SemanticStore,
    typed_ast: &'a TypedAst,
    id: TreeId<Typed>,
) -> Result<&'a Tree<Typed>, JvmIntIntrinsicError> {
    let tree = typed_ast
        .try_get(id)
        .ok_or(JvmIntIntrinsicError::MissingTree { tree: id })?;
    if store
        .types
        .try_get(tree.ty)
        .is_none_or(|ty| matches!(ty, Type::NoType | Type::NoPrefix | Type::Error(_)))
    {
        return Err(JvmIntIntrinsicError::InvalidTreeType {
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
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => Some(*symbol),
        _ => None,
    }
}

fn registered_operation(
    registry: Option<IntPrimitiveOperatorSymbols>,
    method: SymbolId,
) -> Option<JvmIntIntrinsic> {
    let symbols = registry?;
    if method == symbols.bitwise_and {
        return Some(JvmIntIntrinsic::IntBitwiseAnd);
    }
    symbols
        .all()
        .iter()
        .position(|candidate| *candidate == method)
        .map(|i| match i {
            0 => JvmIntIntrinsic::IntAdd,
            1 => JvmIntIntrinsic::IntSubtract,
            2 => JvmIntIntrinsic::IntMultiply,
            3 => JvmIntIntrinsic::IntDivide,
            4 => JvmIntIntrinsic::IntRemainder,
            5 => JvmIntIntrinsic::IntLessThan,
            6 => JvmIntIntrinsic::IntLessOrEqual,
            7 => JvmIntIntrinsic::IntGreaterThan,
            8 => JvmIntIntrinsic::IntGreaterOrEqual,
            _ => unreachable!("the operator registry has exactly nine entries"),
        })
}

fn primitive_semantic_type(
    store: &SemanticStore,
    ty: TypeId,
    definitions: &Definitions,
) -> Option<TypeId> {
    match store.types.try_get(ty)? {
        Type::Constant(Constant::Int(_)) => Some(definitions.int),
        Type::Constant(Constant::Boolean(_)) => Some(definitions.boolean),
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } if Some(*symbol) == builtin_type_symbol(store, definitions.int) => Some(definitions.int),
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } if Some(*symbol) == builtin_type_symbol(store, definitions.boolean) => {
            Some(definitions.boolean)
        }
        Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        } if store.symbols.contains(*symbol) => match store.symbols.info(*symbol) {
            SymbolInfo::Complete(info) => primitive_value_type(store, *info, definitions),
            _ => None,
        },
        _ => None,
    }
}

fn primitive_value_type(
    store: &SemanticStore,
    ty: TypeId,
    definitions: &Definitions,
) -> Option<TypeId> {
    match store.types.try_get(ty)? {
        Type::Constant(Constant::Int(_)) => Some(definitions.int),
        Type::Constant(Constant::Boolean(_)) => Some(definitions.boolean),
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } if Some(*symbol) == builtin_type_symbol(store, definitions.int) => Some(definitions.int),
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } if Some(*symbol) == builtin_type_symbol(store, definitions.boolean) => {
            Some(definitions.boolean)
        }
        _ => None,
    }
}
