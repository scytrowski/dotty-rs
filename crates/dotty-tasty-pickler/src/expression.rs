//! The frozen literal slice has no recursive expression edges.

use dotty_core::ast::{ApplyKind, TypedAst};
use dotty_core::{Constant, SemanticStore, SymbolId, TermRefTarget, TreeId, TreeKind, Type, Typed};
use dotty_tasty::tasty::{
    APPLY_TAG, AstField, AstLabel, AstNode, INTCONST_TAG, SimpleTerm, TermValue,
};

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
    if let TreeKind::Apply(application) = &tree.kind {
        if application.kind != ApplyKind::Regular || application.args.len() != 1 {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::BodyExpression,
                symbol: Some(method),
            });
        }
        let function = ast
            .try_get(application.function)
            .ok_or(PickleError::InvalidTypedTree {
                tree: application.function,
            })?;
        let Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(target),
        } = store
            .types
            .try_get(function.ty)
            .ok_or(PickleError::BodyType {
                tree: application.function,
                ty: function.ty,
            })?
        else {
            return Err(PickleError::BodyType {
                tree: application.function,
                ty: function.ty,
            });
        };
        if matches!(
            ast.try_get(application.args[0])
                .map(|argument| &argument.kind),
            Some(TreeKind::Apply(_))
        ) {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::BodyExpression,
                symbol: Some(method),
            });
        }
        let explicit_this = matches!(function.kind, TreeKind::Select(_));
        let class = store
            .symbols
            .get(method)
            .owner
            .ok_or(PickleError::InvalidSymbol { symbol: method })?;
        if !matches!(store.types.try_get(*prefix), Some(Type::ThisType { class: prefix_class }) if *prefix_class == class)
        {
            return Err(PickleError::Reference {
                symbol: *target,
                reason: crate::ReferenceError::Owner,
            });
        }
        let function_label = context.same_class_method_select_in(class, *target, explicit_this)?;
        let argument =
            lower_expression(store, ast, application.args[0], method, parameters, context)?;
        return context.wire_node(AstNode::Length {
            tag: APPLY_TAG,
            fields: vec![AstField::Tree(function_label), AstField::Tree(argument)],
        });
    }
    if let TreeKind::Ident(_) = &tree.kind {
        let Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(target),
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
        if parameters.contains(target) {
            if !matches!(store.types.try_get(*prefix), Some(Type::NoPrefix)) {
                return Err(PickleError::Reference {
                    symbol: *target,
                    reason: crate::ReferenceError::Owner,
                });
            }
            return context.local_reference(*target, None);
        }
        let class = store
            .symbols
            .contains(method)
            .then(|| store.symbols.get(method).owner)
            .flatten()
            .ok_or(PickleError::InvalidSymbol { symbol: method })?;
        if store.symbols.contains(*target)
            && store.symbols.get(*target).kind == dotty_core::SymbolKind::Method
            && store.symbols.get(*target).owner == Some(class)
            && matches!(store.types.try_get(*prefix), Some(Type::ThisType { class: prefix_class }) if *prefix_class == class)
        {
            return context.same_class_method_select(class, *target);
        }
        if !parameters.contains(target) {
            return Err(PickleError::Reference {
                symbol: *target,
                reason: crate::ReferenceError::NotRegistered,
            });
        }
        return context.local_reference(*target, None);
    }
    if let TreeKind::Select(selection) = &tree.kind {
        let qualifier = ast
            .try_get(selection.qualifier)
            .ok_or(PickleError::InvalidTypedTree {
                tree: selection.qualifier,
            })?;
        if !matches!(qualifier.kind, TreeKind::This(this) if this.qual.is_none()) {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::BodyExpression,
                symbol: Some(method),
            });
        }
        let Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(target),
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
        let class = store
            .symbols
            .contains(method)
            .then(|| store.symbols.get(method).owner)
            .flatten()
            .ok_or(PickleError::InvalidSymbol { symbol: method })?;
        if !store.symbols.contains(*target)
            || store.symbols.get(*target).kind != dotty_core::SymbolKind::Method
            || store.symbols.get(*target).owner != Some(class)
            || !matches!(store.types.try_get(*prefix), Some(Type::ThisType { class: prefix_class }) if *prefix_class == class)
            || !matches!(store.types.try_get(qualifier.ty), Some(Type::ThisType { class: qualifier_class }) if *qualifier_class == class)
        {
            return Err(PickleError::Reference {
                symbol: *target,
                reason: crate::ReferenceError::Owner,
            });
        }
        return context.same_class_method_select_on_this(class, *target);
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
