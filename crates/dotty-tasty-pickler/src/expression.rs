//! The frozen literal slice has no recursive expression edges.

use dotty_core::ast::TypedAst;
use dotty_core::{Constant, SemanticStore, SymbolId, TermRefTarget, TreeId, TreeKind, Type, Typed};
use dotty_tasty::tasty::{AstLabel, AstNode, INTCONST_TAG, SimpleTerm, TermValue};

use crate::{EncodingContext, PickleError, UnsupportedFeature};

pub(crate) fn lower_expression(
    store: &SemanticStore,
    ast: &TypedAst,
    root: TreeId<Typed>,
    method: SymbolId,
    parameters: &[SymbolId],
    context: &mut EncodingContext<'_>,
) -> Result<AstLabel, PickleError> {
    let tree = ast
        .try_get(root)
        .ok_or(PickleError::InvalidTypedTree { tree: root })?;
    if let TreeKind::Ident(_) = &tree.kind {
        let Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(parameter),
        } = store.types.try_get(tree.ty).ok_or(PickleError::BodyType {
            tree: root,
            ty: tree.ty,
        })?
        else {
            return Err(PickleError::BodyType {
                tree: root,
                ty: tree.ty,
            });
        };
        if !parameters.contains(parameter) {
            return Err(PickleError::Reference {
                symbol: *parameter,
                reason: crate::ReferenceError::NotRegistered,
            });
        }
        if !matches!(store.types.try_get(*prefix), Some(Type::NoPrefix)) {
            return Err(PickleError::Reference {
                symbol: *parameter,
                reason: crate::ReferenceError::Owner,
            });
        }
        return context.local_reference(*parameter, None);
    }
    let TreeKind::Literal(literal) = &tree.kind else {
        return Err(PickleError::Unsupported {
            feature: UnsupportedFeature::BodyExpression,
            symbol: Some(method),
        });
    };
    // The method result is canonical Int; this expression has its own singleton
    // type. Scala's pickleConstant encodes that value directly, without a type
    // ascription, widening node, or second serialized constant-type tree.
    let (Constant::Int(value), Some(Type::Constant(Constant::Int(type_value)))) =
        (&literal.value, store.types.try_get(tree.ty))
    else {
        return Err(PickleError::BodyType {
            tree: root,
            ty: tree.ty,
        });
    };
    if value != type_value {
        return Err(PickleError::BodyType {
            tree: root,
            ty: tree.ty,
        });
    }
    let term = SimpleTerm::new(INTCONST_TAG, TermValue::Int(*value)).map_err(|cause| {
        PickleError::Encoding {
            cause: Box::new(cause.into()),
        }
    })?;
    context.wire_node(AstNode::Leaf(term))
}
