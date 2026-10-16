//! Lowering for the first, value-producing typed expression leaves.

use std::collections::HashSet;

use dotty_classfile::bytecode::CodeBuilder;
use dotty_core::ast::{Tree, TreeKind, Typed, TypedAst, ValDef};
use dotty_core::definitions::Definitions;
use dotty_core::ids::{SymbolId, TreeId, TypeId};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolInfo;
use dotty_core::types::{Constant, TermRefTarget, Type, TypeRefTarget};

use crate::{
    InvalidTypeReason, JVM_EXPRESSION_DEPTH_LIMIT, JVM_EXPRESSION_NODE_BUDGET, JvmBooleanIntrinsic,
    JvmCallTarget, JvmClassMetadata, JvmCodegenError, JvmCompilationUnit, JvmExpressionFlow,
    JvmIntIntrinsic, JvmInvocationKind, JvmLocalLayout, JvmLocalSlot, JvmLoweredExpression,
    JvmMethodMetadata, JvmReturnType, JvmType, JvmValueCategory, UnsupportedJvmCallShapeKind,
    resolve_int_intrinsic,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Value(JvmType),
    Unit,
    Diverges,
}

enum ExpressionShape {
    Leaf,
    Select,
    Block,
    If,
    While,
    Return,
    Apply,
    Assign,
    Unsupported(&'static str),
}

pub(super) fn lower_current_receiver(
    typed_ast: &TypedAst,
    class: &JvmClassMetadata,
    caller: &JvmMethodMetadata,
    call_id: TreeId<Typed>,
    target: &JvmCallTarget,
) -> Result<CodeBuilder, JvmCodegenError> {
    let mut code = CodeBuilder::new();
    append_current_receiver(typed_ast, class, caller, call_id, target, &mut code)?;
    Ok(code)
}

fn append_current_receiver(
    typed_ast: &TypedAst,
    class: &JvmClassMetadata,
    caller: &JvmMethodMetadata,
    call_id: TreeId<Typed>,
    target: &JvmCallTarget,
    code: &mut CodeBuilder,
) -> Result<(), JvmCodegenError> {
    let class_symbol = class.identity().symbol();
    if !class
        .methods()
        .iter()
        .any(|method| method.symbol() == caller.symbol())
    {
        return Err(JvmCodegenError::CallerMethodNotInJvmPlan {
            class: class_symbol,
            method: caller.symbol(),
        });
    }
    if !class
        .methods()
        .iter()
        .any(|method| method.symbol() == target.method_symbol())
        || target.owner_internal_name() != class.identity().internal_name()
    {
        return Err(JvmCodegenError::MethodNotInJvmPlan {
            tree: call_id,
            class: class_symbol,
            method: target.method_symbol(),
        });
    }

    let call = typed_ast
        .try_get(call_id)
        .ok_or(JvmCodegenError::UnsupportedTypedExpression {
            tree: call_id,
            node: "missing tree",
        })?;
    let TreeKind::Apply(application) = &call.kind else {
        return Err(JvmCodegenError::UnsupportedJvmCallShape {
            tree: call_id,
            kind: UnsupportedJvmCallShapeKind::NotApplication,
        });
    };
    let function = typed_ast.try_get(application.function).ok_or(
        JvmCodegenError::UnsupportedTypedExpression {
            tree: application.function,
            node: "missing tree",
        },
    )?;
    if let TreeKind::Select(select) = &function.kind {
        let receiver = typed_ast.try_get(select.qualifier).ok_or(
            JvmCodegenError::UnsupportedTypedExpression {
                tree: select.qualifier,
                node: "missing tree",
            },
        )?;
        match &receiver.kind {
            TreeKind::This(this) if this.qual.is_none() => {}
            TreeKind::This(_) => {
                return Err(JvmCodegenError::UnsupportedJvmCallShape {
                    tree: select.qualifier,
                    kind: UnsupportedJvmCallShapeKind::QualifiedThisReceiver,
                });
            }
            _ => {
                return Err(JvmCodegenError::UnsupportedJvmCallShape {
                    tree: select.qualifier,
                    kind: UnsupportedJvmCallShapeKind::NonThisReceiver,
                });
            }
        }
    } else if !matches!(function.kind, TreeKind::Ident(_)) {
        return Err(JvmCodegenError::UnsupportedJvmCallShape {
            tree: application.function,
            kind: UnsupportedJvmCallShapeKind::UnsupportedFunctionTree,
        });
    }

    code.aload(0)
        .map_err(|error| JvmCodegenError::CodeBuilder {
            tree: call_id,
            error,
        })?;
    Ok(())
}

/// Read-only semantic inputs shared by every recursive lowering step.
struct JvmLoweringContext<'a> {
    store: &'a SemanticStore,
    typed_ast: &'a TypedAst,
    definitions: &'a Definitions,
    method: &'a JvmMethodMetadata,
    local_layout: Option<&'a JvmLocalLayout>,
    initialized_locals: HashSet<SymbolId>,
    call_context: Option<(&'a JvmCompilationUnit<'a>, &'a JvmClassMetadata)>,
    control_flow_depth: usize,
    first_field_read: Option<(TreeId<Typed>, SymbolId)>,
    first_field_write: Option<(TreeId<Typed>, SymbolId)>,
    node_visits: usize,
}

impl<'a> JvmLoweringContext<'a> {
    fn new(
        store: &'a SemanticStore,
        typed_ast: &'a TypedAst,
        definitions: &'a Definitions,
        method: &'a JvmMethodMetadata,
    ) -> Self {
        Self {
            store,
            typed_ast,
            definitions,
            method,
            local_layout: None,
            initialized_locals: HashSet::new(),
            call_context: None,
            control_flow_depth: 0,
            first_field_read: None,
            first_field_write: None,
            node_visits: 0,
        }
    }

    fn tree(&self, tree: TreeId<Typed>) -> Result<&Tree<Typed>, JvmCodegenError> {
        self.typed_ast
            .try_get(tree)
            .ok_or(JvmCodegenError::UnsupportedTypedExpression {
                tree,
                node: "missing tree",
            })
    }

    fn visit(&mut self, tree: TreeId<Typed>, depth: usize) -> Result<(), JvmCodegenError> {
        let node = self.tree(tree)?;
        if depth >= JVM_EXPRESSION_DEPTH_LIMIT {
            return Err(JvmCodegenError::ExpressionDepthExceeded {
                tree,
                position: node.position,
                limit: JVM_EXPRESSION_DEPTH_LIMIT,
            });
        }
        if self.node_visits >= JVM_EXPRESSION_NODE_BUDGET {
            return Err(JvmCodegenError::ExpressionNodeBudgetExceeded {
                tree,
                limit: JVM_EXPRESSION_NODE_BUDGET,
            });
        }
        validate_tree_type(self.store, tree, node.ty)?;
        self.node_visits += 1;
        Ok(())
    }
}

pub(super) fn lower_value_expression(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    definitions: &Definitions,
    method: &JvmMethodMetadata,
    tree_id: TreeId<Typed>,
    code: &mut CodeBuilder,
) -> Result<JvmValueCategory, JvmCodegenError> {
    let mut context = JvmLoweringContext::new(store, typed_ast, definitions, method);
    context.visit(tree_id, 0)?;
    match lower_leaf(&mut context, tree_id, code)? {
        Flow::Value(value) => Ok(value.category()),
        Flow::Unit => Err(JvmCodegenError::UnsupportedTypedExpression {
            tree: tree_id,
            node: "Unit in value-producing lowering",
        }),
        Flow::Diverges => Err(JvmCodegenError::UnsupportedTypedExpression {
            tree: tree_id,
            node: "diverging leaf flow",
        }),
    }
}

pub(super) fn lower_expression(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    definitions: &Definitions,
    method: &JvmMethodMetadata,
    tree: TreeId<Typed>,
) -> Result<JvmLoweredExpression, JvmCodegenError> {
    let mut code = CodeBuilder::new();
    let mut context = JvmLoweringContext::new(store, typed_ast, definitions, method);
    let flow = emit_expression(&mut context, tree, &mut code, 0)?;
    Ok(JvmLoweredExpression {
        code,
        flow: public_flow(flow),
    })
}

pub(super) fn lower_method_body(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    definitions: &Definitions,
    method: &JvmMethodMetadata,
    body: TreeId<Typed>,
) -> Result<CodeBuilder, JvmCodegenError> {
    let mut code = CodeBuilder::new();
    let mut context = JvmLoweringContext::new(store, typed_ast, definitions, method);
    let flow = emit_expression(&mut context, body, &mut code, 0)?;
    append_method_return(method, body, flow, &mut code)?;
    validate_field_access_control_flow(&context, &code)?;
    Ok(code)
}

pub(super) fn lower_planned_method_body(
    unit: &JvmCompilationUnit<'_>,
    class: &JvmClassMetadata,
    definitions: &Definitions,
    method: &JvmMethodMetadata,
    body: TreeId<Typed>,
) -> Result<CodeBuilder, JvmCodegenError> {
    let mut code = CodeBuilder::new();
    let local_layout =
        unit.local_layout(class.identity().symbol(), method.symbol(), definitions)?;
    let mut context = JvmLoweringContext::new(unit.store, unit.typed_ast, definitions, method);
    context.local_layout = Some(&local_layout);
    context.call_context = Some((unit, class));
    let flow = emit_expression(&mut context, body, &mut code, 0)?;
    append_method_return(method, body, flow, &mut code)?;
    validate_field_access_control_flow(&context, &code)?;
    Ok(code)
}

fn validate_field_access_control_flow(
    context: &JvmLoweringContext<'_>,
    code: &CodeBuilder,
) -> Result<(), JvmCodegenError> {
    if code.has_branches() {
        if let Some((tree, field)) = context.first_field_write {
            return Err(JvmCodegenError::JvmFieldWriteInUnsupportedControlFlow {
                tree,
                method: context.method.symbol(),
                field,
            });
        }
        if let Some((tree, field)) = context.first_field_read {
            return Err(JvmCodegenError::JvmFieldReadInUnsupportedControlFlow {
                tree,
                method: context.method.symbol(),
                field,
            });
        }
    }
    Ok(())
}

fn append_method_return(
    method: &JvmMethodMetadata,
    body: TreeId<Typed>,
    flow: Flow,
    code: &mut CodeBuilder,
) -> Result<(), JvmCodegenError> {
    match (method.descriptor().result(), flow) {
        (JvmReturnType::Value(expected), Flow::Value(found)) if expected == found => {
            code.ireturn()
                .map_err(|error| JvmCodegenError::CodeBuilder { tree: body, error })?;
        }
        (JvmReturnType::Void, Flow::Unit) => {
            code.return_void()
                .map_err(|error| JvmCodegenError::CodeBuilder { tree: body, error })?;
        }
        (JvmReturnType::Value(_), Flow::Diverges) | (JvmReturnType::Void, Flow::Diverges) => {}
        (expected, Flow::Unit) => {
            return Err(JvmCodegenError::MethodBodyFlowMismatch {
                method: method.symbol(),
                tree: body,
                expected,
                found: JvmExpressionFlow::Unit,
            });
        }
        (expected, Flow::Value(found)) => {
            return Err(JvmCodegenError::MethodBodyTypeMismatch {
                method: method.symbol(),
                tree: body,
                expected,
                found: Some(found),
            });
        }
    }
    Ok(())
}

fn lower_leaf(
    context: &mut JvmLoweringContext<'_>,
    tree_id: TreeId<Typed>,
    code: &mut CodeBuilder,
) -> Result<Flow, JvmCodegenError> {
    let tree = context.tree(tree_id)?;
    match &tree.kind {
        TreeKind::Literal(literal) => {
            let (value, primitive) = match literal.value {
                Constant::Int(value) => (value, JvmType::Int),
                Constant::Boolean(value) => (i32::from(value), JvmType::Boolean),
                Constant::Unit => {
                    if !is_unit_type(context.store, tree.ty, context.definitions) {
                        return Err(JvmCodegenError::UnsupportedTypedExpression {
                            tree: tree_id,
                            node: "Unit literal with a non-Unit type",
                        });
                    }
                    return Ok(Flow::Unit);
                }
                _ => {
                    return Err(JvmCodegenError::UnsupportedTypedExpression {
                        tree: tree_id,
                        node: "non-Int/Boolean literal",
                    });
                }
            };
            let typed_primitive = primitive_type(context.store, tree.ty, context.definitions);
            let matches_constant_type = match context.store.types.try_get(tree.ty) {
                Some(Type::Constant(typed_value)) => typed_value == &literal.value,
                _ => typed_primitive == Some(primitive),
            };
            if !matches_constant_type || typed_primitive != Some(primitive) {
                return Err(JvmCodegenError::LiteralTypeMismatch {
                    tree: tree_id,
                    value: primitive,
                    typed: typed_primitive,
                });
            }
            code.iconst(value)
                .map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                })?;
            Ok(Flow::Value(primitive))
        }
        TreeKind::Ident(_) => {
            let symbol = match context.store.types.try_get(tree.ty) {
                Some(Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                }) => Some(*symbol),
                _ => None,
            };
            if let Some(local) = match symbol {
                Some(symbol) => local_reference_slot(context, tree_id, symbol)?,
                None => None,
            } {
                let symbol = local.symbol();
                if !context.initialized_locals.contains(&symbol) {
                    return Err(JvmCodegenError::UninitializedLocalRead {
                        tree: tree_id,
                        symbol,
                    });
                }
                code.iload(usize::from(local.slot())).map_err(|error| {
                    JvmCodegenError::CodeBuilder {
                        tree: tree_id,
                        error,
                    }
                })?;
                return Ok(Flow::Value(local.ty()));
            }
            let Some(parameter) = symbol.and_then(|symbol| {
                context
                    .method
                    .parameters()
                    .iter()
                    .find(|parameter| parameter.symbol() == symbol)
                    .copied()
            }) else {
                if let Some(symbol) = symbol
                    && context.store.symbols.contains(symbol)
                    && context.store.symbols.get(symbol).kind
                        == dotty_core::symbols::SymbolKind::Field
                {
                    return emit_field_read(context, tree_id, symbol, None, 0, code);
                }
                return Err(JvmCodegenError::InvalidParameterReference {
                    tree: tree_id,
                    symbol,
                });
            };
            if !context.store.symbols.contains(parameter.symbol()) {
                return Err(JvmCodegenError::InvalidParameterReference {
                    tree: tree_id,
                    symbol: Some(parameter.symbol()),
                });
            }
            let parameter_symbol = context.store.symbols.get(parameter.symbol());
            if parameter_symbol.kind != dotty_core::symbols::SymbolKind::Parameter
                || parameter_symbol.owner != Some(context.method.symbol())
            {
                return Err(JvmCodegenError::InvalidParameterReference {
                    tree: tree_id,
                    symbol: Some(parameter.symbol()),
                });
            }
            let symbol_type = match context.store.symbols.info(parameter.symbol()) {
                SymbolInfo::Complete(ty) => Some(*ty),
                SymbolInfo::Missing | SymbolInfo::Deferred(_) | SymbolInfo::Error => None,
            };
            let found =
                symbol_type.and_then(|ty| primitive_type(context.store, ty, context.definitions));
            if found != Some(parameter.ty()) {
                return Err(JvmCodegenError::ParameterReferenceTypeMismatch {
                    tree: tree_id,
                    parameter: parameter.symbol(),
                    expected: parameter.ty(),
                    found,
                });
            }
            code.iload(usize::from(parameter.slot())).map_err(|error| {
                JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                }
            })?;
            Ok(Flow::Value(parameter.ty()))
        }
        kind => Err(JvmCodegenError::UnsupportedTypedExpression {
            tree: tree_id,
            node: tree_kind_name(kind),
        }),
    }
}

fn local_reference_slot(
    context: &JvmLoweringContext<'_>,
    tree: TreeId<Typed>,
    symbol: SymbolId,
) -> Result<Option<JvmLocalSlot>, JvmCodegenError> {
    if !context.store.symbols.contains(symbol)
        || context.store.symbols.get(symbol).kind != dotty_core::symbols::SymbolKind::Local
    {
        return Ok(None);
    }
    if context.store.symbols.get(symbol).owner != Some(context.method.symbol()) {
        return Err(JvmCodegenError::InvalidLocalReference { tree, symbol });
    }
    let local = context
        .local_layout
        .ok_or(JvmCodegenError::MissingLocalBinding { tree, symbol })?
        .local(symbol)
        .map_err(|_| JvmCodegenError::MissingLocalBinding { tree, symbol })?;
    let semantic_type = match context.store.symbols.info(symbol) {
        SymbolInfo::Complete(ty) => *ty,
        SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
            return Err(JvmCodegenError::MissingSemanticInfo { symbol });
        }
        SymbolInfo::Error => return Err(JvmCodegenError::ErrorSemanticInfo { symbol }),
    };
    let found = crate::descriptor::primitive_semantic_value_type(
        context.store,
        context.definitions,
        symbol,
        semantic_type,
    )?;
    if found != local.ty() {
        return Err(JvmCodegenError::LocalReferenceTypeMismatch {
            tree,
            symbol,
            expected: local.ty(),
            found: Some(found),
        });
    }
    Ok(Some(*local))
}

fn emit_expression(
    context: &mut JvmLoweringContext<'_>,
    tree_id: TreeId<Typed>,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    context.visit(tree_id, depth)?;
    let shape = {
        let tree = context.tree(tree_id)?;
        match &tree.kind {
            TreeKind::Literal(_) | TreeKind::Ident(_) => ExpressionShape::Leaf,
            TreeKind::Select(_) => ExpressionShape::Select,
            TreeKind::Block(_) => ExpressionShape::Block,
            TreeKind::If(_) => ExpressionShape::If,
            TreeKind::While(_) => ExpressionShape::While,
            TreeKind::Return(_) => ExpressionShape::Return,
            TreeKind::Apply(_) => ExpressionShape::Apply,
            TreeKind::Assign(_) => ExpressionShape::Assign,
            kind => ExpressionShape::Unsupported(tree_kind_name(kind)),
        }
    };
    match shape {
        ExpressionShape::Leaf => lower_leaf(context, tree_id, code),
        ExpressionShape::Select => {
            if let Some(field) = selected_field_symbol(context, tree_id)? {
                let TreeKind::Select(select) = &context.tree(tree_id)?.kind else {
                    unreachable!("select shape was observed above");
                };
                emit_field_read(context, tree_id, field, Some(select.qualifier), depth, code)
            } else {
                emit_boolean_not(context, tree_id, code, depth)
            }
        }
        ExpressionShape::Block => emit_block(context, tree_id, code, depth),
        ExpressionShape::If => emit_if(context, tree_id, code, depth),
        ExpressionShape::While => emit_while(context, tree_id, code, depth),
        ExpressionShape::Return => emit_return(context, tree_id, code, depth),
        ExpressionShape::Apply => emit_apply(context, tree_id, code, depth),
        ExpressionShape::Assign => emit_assignment(context, tree_id, code, depth),
        ExpressionShape::Unsupported(node) => Err(JvmCodegenError::UnsupportedTypedExpression {
            tree: tree_id,
            node,
        }),
    }
}

fn selected_field_symbol(
    context: &JvmLoweringContext<'_>,
    tree_id: TreeId<Typed>,
) -> Result<Option<SymbolId>, JvmCodegenError> {
    let tree = context.tree(tree_id)?;
    let Some(Type::TermRef {
        target: TermRefTarget::Symbol(symbol),
        ..
    }) = context.store.types.try_get(tree.ty)
    else {
        return Ok(None);
    };
    if !context.store.symbols.contains(*symbol) {
        return Err(JvmCodegenError::UnknownJvmFieldReference {
            tree: tree_id,
            field: *symbol,
        });
    }
    Ok(
        (context.store.symbols.get(*symbol).kind == dotty_core::symbols::SymbolKind::Field)
            .then_some(*symbol),
    )
}

fn emit_field_read(
    context: &mut JvmLoweringContext<'_>,
    tree_id: TreeId<Typed>,
    field: SymbolId,
    receiver: Option<TreeId<Typed>>,
    depth: usize,
    code: &mut CodeBuilder,
) -> Result<Flow, JvmCodegenError> {
    let Some((_, class)) = context.call_context else {
        return Err(JvmCodegenError::JvmFieldReadContextUnavailable {
            tree: tree_id,
            method: context.method.symbol(),
        });
    };
    let class_symbol = class.identity().symbol();
    if !class
        .methods()
        .iter()
        .any(|method| method.symbol() == context.method.symbol())
    {
        return Err(JvmCodegenError::JvmFieldReadContextUnavailable {
            tree: tree_id,
            method: context.method.symbol(),
        });
    }
    if context.control_flow_depth > 0 {
        return Err(JvmCodegenError::JvmFieldReadInUnsupportedControlFlow {
            tree: tree_id,
            method: context.method.symbol(),
            field,
        });
    }

    let field_symbol = context
        .store
        .symbols
        .contains(field)
        .then(|| context.store.symbols.get(field))
        .ok_or(JvmCodegenError::UnknownJvmFieldReference {
            tree: tree_id,
            field,
        })?;
    if field_symbol.kind != dotty_core::symbols::SymbolKind::Field {
        return Err(JvmCodegenError::FieldNotInJvmPlan {
            tree: tree_id,
            class: class_symbol,
            field,
        });
    }
    if field_symbol.owner != Some(class_symbol) {
        return Err(JvmCodegenError::WrongOwner {
            symbol: field,
            expected: class_symbol,
            actual: field_symbol.owner,
        });
    }
    let field_metadata = class
        .fields()
        .iter()
        .find(|metadata| metadata.symbol() == field)
        .ok_or(JvmCodegenError::FieldNotInJvmPlan {
            tree: tree_id,
            class: class_symbol,
            field,
        })?;

    let (reference_prefix, reference_target) =
        match context.store.types.try_get(context.tree(tree_id)?.ty) {
            Some(Type::TermRef { prefix, target }) => (*prefix, target.symbol()),
            _ => (context.definitions.no_prefix, None),
        };
    if reference_target != Some(field) {
        return Err(JvmCodegenError::InvalidJvmFieldReferencePrefix {
            tree: tree_id,
            field,
            class: class_symbol,
        });
    }

    if let Some(receiver) = receiver {
        context.visit(receiver, depth + 1)?;
        let receiver_tree = context.tree(receiver)?;
        let valid_receiver =
            matches!(
                receiver_tree.kind,
                TreeKind::This(dotty_core::ast::This { qual: None })
            ) && is_current_this_type(context.store, receiver_tree.ty, class_symbol);
        if !valid_receiver {
            return Err(JvmCodegenError::UnsupportedJvmFieldReceiver {
                tree: tree_id,
                field,
            });
        }
    } else if !is_current_this_type(context.store, reference_prefix, class_symbol) {
        return Err(JvmCodegenError::InvalidJvmFieldReferencePrefix {
            tree: tree_id,
            field,
            class: class_symbol,
        });
    }
    if let Some(receiver) = receiver
        && reference_prefix != context.tree(receiver)?.ty
    {
        return Err(JvmCodegenError::InvalidJvmFieldReferencePrefix {
            tree: tree_id,
            field,
            class: class_symbol,
        });
    }

    let field_type = match context.store.symbols.info(field) {
        SymbolInfo::Complete(ty) => crate::descriptor::primitive_semantic_value_type(
            context.store,
            context.definitions,
            field,
            *ty,
        )?,
        SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
            return Err(JvmCodegenError::MissingSemanticInfo { symbol: field });
        }
        SymbolInfo::Error => return Err(JvmCodegenError::ErrorSemanticInfo { symbol: field }),
    };
    context.first_field_read.get_or_insert((tree_id, field));
    code.aload(0)
        .and_then(|code| {
            code.getfield(
                class.identity().internal_name(),
                field_metadata.name(),
                field_metadata.descriptor(),
            )
        })
        .map_err(|error| JvmCodegenError::CodeBuilder {
            tree: tree_id,
            error,
        })?;
    Ok(Flow::Value(field_type))
}

fn is_current_this_type(store: &SemanticStore, ty: TypeId, class: SymbolId) -> bool {
    matches!(store.types.try_get(ty), Some(Type::ThisType { class: found }) if *found == class)
}

fn emit_assignment(
    context: &mut JvmLoweringContext<'_>,
    tree_id: TreeId<Typed>,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    let (lhs, rhs, result_type) = {
        let tree = context.tree(tree_id)?;
        let TreeKind::Assign(assignment) = &tree.kind else {
            return Err(JvmCodegenError::UnsupportedTypedExpression {
                tree: tree_id,
                node: tree_kind_name(&tree.kind),
            });
        };
        (assignment.lhs, assignment.rhs, tree.ty)
    };
    if !is_unit_type(context.store, result_type, context.definitions) {
        return Err(JvmCodegenError::AssignmentResultTypeMismatch {
            tree: tree_id,
            ty: result_type,
        });
    }

    context.visit(lhs, depth + 1)?;
    let (symbol, receiver) = match context.tree(lhs)? {
        Tree {
            kind: TreeKind::Ident(_),
            ty,
            ..
        } => (term_ref_symbol(context, tree_id, lhs, *ty)?, None),
        Tree {
            kind: TreeKind::Select(select),
            ty,
            ..
        } => (
            term_ref_symbol(context, tree_id, lhs, *ty)?,
            Some(select.qualifier),
        ),
        _ => return Err(JvmCodegenError::InvalidAssignmentTarget { tree: tree_id, lhs }),
    };
    if !context.store.symbols.contains(symbol) {
        return Err(JvmCodegenError::InvalidAssignmentTarget { tree: tree_id, lhs });
    }
    if context.store.symbols.get(symbol).kind == dotty_core::symbols::SymbolKind::Field {
        return emit_field_assignment(
            context,
            tree_id,
            lhs,
            FieldAssignmentTarget { symbol, receiver },
            rhs,
            code,
            depth,
        );
    }
    if receiver.is_some() {
        return Err(JvmCodegenError::InvalidAssignmentTarget { tree: tree_id, lhs });
    }
    let Some(local) = local_reference_slot(context, lhs, symbol)? else {
        return Err(JvmCodegenError::InvalidAssignmentTarget { tree: tree_id, lhs });
    };
    let symbol_info = context.store.symbols.get(symbol);
    let symbol_mutable = symbol_info
        .flags
        .contains(dotty_core::symbols::SymbolFlags::MUTABLE);
    if symbol_mutable != local.is_mutable() {
        return Err(JvmCodegenError::LocalMutabilityMismatch {
            declaration: local.declaration(),
            symbol,
            recorded_mutable: local.is_mutable(),
            symbol_mutable,
        });
    }
    if !symbol_mutable {
        return Err(JvmCodegenError::ImmutableLocalAssignment {
            tree: tree_id,
            symbol,
        });
    }
    if !context.initialized_locals.contains(&symbol) {
        return Err(JvmCodegenError::UninitializedLocalAssignment {
            tree: tree_id,
            symbol,
        });
    }

    let flow = emit_expression(context, rhs, code, depth + 1)?;
    match flow {
        Flow::Diverges => Ok(Flow::Diverges),
        Flow::Value(found) if found == local.ty() => {
            code.istore(usize::from(local.slot())).map_err(|error| {
                JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                }
            })?;
            Ok(Flow::Unit)
        }
        found => Err(JvmCodegenError::AssignmentValueFlowMismatch {
            tree: tree_id,
            symbol,
            expected: local.ty(),
            found: public_flow(found),
        }),
    }
}

fn term_ref_symbol(
    context: &JvmLoweringContext<'_>,
    tree: TreeId<Typed>,
    lhs: TreeId<Typed>,
    ty: TypeId,
) -> Result<SymbolId, JvmCodegenError> {
    match context.store.types.try_get(ty) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) => Ok(*symbol),
        _ => Err(JvmCodegenError::InvalidAssignmentTarget { tree, lhs }),
    }
}

struct FieldAssignmentTarget {
    symbol: SymbolId,
    receiver: Option<TreeId<Typed>>,
}

fn emit_field_assignment(
    context: &mut JvmLoweringContext<'_>,
    tree_id: TreeId<Typed>,
    lhs: TreeId<Typed>,
    target: FieldAssignmentTarget,
    rhs: TreeId<Typed>,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    let FieldAssignmentTarget {
        symbol: field,
        receiver,
    } = target;
    let Some((_, class)) = context.call_context else {
        return Err(JvmCodegenError::JvmFieldWriteContextUnavailable {
            tree: tree_id,
            method: context.method.symbol(),
        });
    };
    let class_symbol = class.identity().symbol();
    if !class
        .methods()
        .iter()
        .any(|method| method.symbol() == context.method.symbol())
    {
        return Err(JvmCodegenError::JvmFieldWriteContextUnavailable {
            tree: tree_id,
            method: context.method.symbol(),
        });
    }
    if context.control_flow_depth > 0 {
        return Err(JvmCodegenError::JvmFieldWriteInUnsupportedControlFlow {
            tree: tree_id,
            method: context.method.symbol(),
            field,
        });
    }

    let field_symbol = context
        .store
        .symbols
        .contains(field)
        .then(|| context.store.symbols.get(field))
        .ok_or(JvmCodegenError::UnknownJvmFieldReference { tree: lhs, field })?;
    if field_symbol.kind != dotty_core::symbols::SymbolKind::Field {
        return Err(JvmCodegenError::FieldNotInJvmPlan {
            tree: lhs,
            class: class_symbol,
            field,
        });
    }
    if field_symbol.owner != Some(class_symbol) {
        return Err(JvmCodegenError::WrongOwner {
            symbol: field,
            expected: class_symbol,
            actual: field_symbol.owner,
        });
    }
    let field_metadata = class
        .fields()
        .iter()
        .find(|candidate| candidate.symbol() == field)
        .ok_or(JvmCodegenError::FieldNotInJvmPlan {
            tree: lhs,
            class: class_symbol,
            field,
        })?;

    let reference_prefix = match context.store.types.try_get(context.tree(lhs)?.ty) {
        Some(Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(target),
        }) if *target == field => *prefix,
        _ => {
            return Err(JvmCodegenError::InvalidJvmFieldReferencePrefix {
                tree: lhs,
                field,
                class: class_symbol,
            });
        }
    };
    if let Some(receiver) = receiver {
        context.visit(receiver, depth + 1)?;
        let receiver_tree = context.tree(receiver)?;
        let valid_receiver =
            matches!(
                receiver_tree.kind,
                TreeKind::This(dotty_core::ast::This { qual: None })
            ) && is_current_this_type(context.store, receiver_tree.ty, class_symbol);
        if !valid_receiver {
            return Err(JvmCodegenError::UnsupportedJvmFieldReceiver { tree: lhs, field });
        }
        if reference_prefix != receiver_tree.ty {
            return Err(JvmCodegenError::InvalidJvmFieldReferencePrefix {
                tree: lhs,
                field,
                class: class_symbol,
            });
        }
    } else if !is_current_this_type(context.store, reference_prefix, class_symbol) {
        return Err(JvmCodegenError::InvalidJvmFieldReferencePrefix {
            tree: lhs,
            field,
            class: class_symbol,
        });
    }

    let symbol_mutable = field_symbol
        .flags
        .contains(dotty_core::symbols::SymbolFlags::MUTABLE);
    let metadata_mutable = !field_metadata.access_flags().is_final();
    if symbol_mutable != metadata_mutable {
        return Err(JvmCodegenError::JvmFieldMutabilityMismatch {
            tree: lhs,
            field,
            symbol_mutable,
            metadata_mutable,
        });
    }
    if !symbol_mutable {
        return Err(JvmCodegenError::ImmutableFieldAssignment {
            tree: tree_id,
            field,
        });
    }

    let field_type = match context.store.symbols.info(field) {
        SymbolInfo::Complete(ty) => crate::descriptor::primitive_semantic_value_type(
            context.store,
            context.definitions,
            field,
            *ty,
        )?,
        SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
            return Err(JvmCodegenError::MissingSemanticInfo { symbol: field });
        }
        SymbolInfo::Error => return Err(JvmCodegenError::ErrorSemanticInfo { symbol: field }),
    };

    context.first_field_write.get_or_insert((tree_id, field));
    let Some(local_layout) = context.local_layout else {
        return Err(JvmCodegenError::JvmFieldWriteContextUnavailable {
            tree: tree_id,
            method: context.method.symbol(),
        });
    };
    let receiver_slot = usize::from(local_layout.max_locals());
    let value_slot = receiver_slot
        .checked_add(1)
        .filter(|slot| *slot < usize::from(u16::MAX))
        .ok_or(JvmCodegenError::TooManyLocalSlots {
            method: context.method.symbol(),
        })?;
    code.aload(0)
        .and_then(|code| code.astore(receiver_slot))
        .map_err(|error| JvmCodegenError::CodeBuilder {
            tree: tree_id,
            error,
        })?;
    match emit_expression(context, rhs, code, depth + 1)? {
        Flow::Diverges => Ok(Flow::Diverges),
        Flow::Value(found) if found == field_type => {
            code.istore(value_slot)
                .and_then(|code| code.aload(receiver_slot))
                .and_then(|code| code.iload(value_slot))
                .map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                })?;
            code.putfield(
                class.identity().internal_name(),
                field_metadata.name(),
                field_metadata.descriptor(),
            )
            .map_err(|error| JvmCodegenError::CodeBuilder {
                tree: tree_id,
                error,
            })?;
            Ok(Flow::Unit)
        }
        found => Err(JvmCodegenError::AssignmentValueFlowMismatch {
            tree: tree_id,
            symbol: field,
            expected: field_type,
            found: public_flow(found),
        }),
    }
}

fn emit_block(
    context: &mut JvmLoweringContext<'_>,
    tree_id: TreeId<Typed>,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    let (declared_type, stats, expr) = {
        let tree = context.tree(tree_id)?;
        let TreeKind::Block(block) = &tree.kind else {
            return Err(JvmCodegenError::UnsupportedTypedExpression {
                tree: tree_id,
                node: tree_kind_name(&tree.kind),
            });
        };
        (tree.ty, block.stats.clone(), block.expr)
    };

    for stat in stats {
        if matches!(context.tree(stat)?.kind, TreeKind::ValDef(_)) {
            emit_local_declaration(context, stat, code, depth + 1)?;
            continue;
        }
        match emit_expression(context, stat, code, depth + 1)? {
            Flow::Value(_) => {
                code.pop()
                    .map_err(|error| JvmCodegenError::CodeBuilder { tree: stat, error })?;
            }
            Flow::Unit => {
                let stat_type = context.tree(stat)?.ty;
                if !is_unit_type(context.store, stat_type, context.definitions) {
                    return Err(JvmCodegenError::UnitFlowTypeMismatch {
                        tree: stat,
                        ty: stat_type,
                    });
                }
            }
            Flow::Diverges => return Ok(Flow::Diverges),
        }
    }

    let expression_type = context.tree(expr)?.ty;
    let flow = emit_expression(context, expr, code, depth + 1)?;
    match flow {
        Flow::Value(value)
            if block_types_match(context, tree_id, expr, declared_type, expression_type)?
                && block_value_type(context, tree_id, declared_type, Some(expr))?
                    == Some(value) =>
        {
            Ok(flow)
        }
        Flow::Unit
            if is_unit_type(context.store, declared_type, context.definitions)
                && block_types_match(context, tree_id, expr, declared_type, expression_type)? =>
        {
            Ok(flow)
        }
        Flow::Diverges
            if block_types_match(context, tree_id, expr, declared_type, expression_type)? =>
        {
            Ok(flow)
        }
        _ => Err(JvmCodegenError::BlockResultTypeMismatch {
            tree: tree_id,
            declared: declared_type,
            found: public_flow(flow),
        }),
    }
}

fn emit_local_declaration(
    context: &mut JvmLoweringContext<'_>,
    declaration: TreeId<Typed>,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<(), JvmCodegenError> {
    context.visit(declaration, depth)?;
    let method = context.method.symbol();
    let local = context
        .local_layout
        .and_then(|layout| {
            layout
                .locals()
                .iter()
                .find(|local| local.declaration() == declaration)
                .copied()
        })
        .ok_or(JvmCodegenError::LocalDeclarationNotInPlan {
            method,
            declaration,
        })?;
    let rhs = match &context.tree(declaration)?.kind {
        TreeKind::ValDef(ValDef { rhs, .. }) => *rhs,
        _ => {
            return Err(JvmCodegenError::InvalidLocalDeclaration {
                method,
                declaration,
            });
        }
    }
    .ok_or(JvmCodegenError::MissingLocalInitializer {
        method,
        declaration,
    })?;

    let flow = emit_expression(context, rhs, code, depth + 1)?;
    if flow != Flow::Value(local.ty()) {
        return Err(JvmCodegenError::LocalInitializerFlowMismatch {
            method,
            declaration,
            symbol: local.symbol(),
            expected: local.ty(),
            found: public_flow(flow),
        });
    }

    code.istore(usize::from(local.slot()))
        .map_err(|error| JvmCodegenError::CodeBuilder {
            tree: declaration,
            error,
        })?;
    context.initialized_locals.insert(local.symbol());
    Ok(())
}

fn block_types_match(
    context: &JvmLoweringContext<'_>,
    tree: TreeId<Typed>,
    expression_tree: TreeId<Typed>,
    declared: TypeId,
    expression: TypeId,
) -> Result<bool, JvmCodegenError> {
    if declared == expression || is_nothing_type(context.store, expression, context.definitions) {
        return Ok(true);
    }
    if is_unit_type(context.store, declared, context.definitions)
        && is_unit_type(context.store, expression, context.definitions)
    {
        return Ok(true);
    }

    let declared = block_value_type(context, tree, declared, Some(expression_tree))?;
    let expression = block_value_type(context, tree, expression, Some(expression_tree))?;
    Ok(declared.is_some() && declared == expression)
}

fn block_value_type(
    context: &JvmLoweringContext<'_>,
    tree: TreeId<Typed>,
    ty: TypeId,
    intrinsic_expression: Option<TreeId<Typed>>,
) -> Result<Option<JvmType>, JvmCodegenError> {
    if let Some(value_type) = primitive_type(context.store, ty, context.definitions) {
        return Ok(Some(value_type));
    }
    let Some(Type::TermRef {
        target: TermRefTarget::Symbol(symbol),
        ..
    }) = context.store.types.try_get(ty)
    else {
        return Ok(None);
    };
    if let Some(local) = local_reference_slot(context, tree, *symbol)? {
        return Ok(Some(local.ty()));
    }
    if let Some(expression) = intrinsic_expression
        && is_boolean_not_block_result(context, expression, ty)?
    {
        return Ok(Some(JvmType::Boolean));
    }
    if context.store.symbols.contains(*symbol)
        && context.store.symbols.get(*symbol).kind == dotty_core::symbols::SymbolKind::Field
    {
        let Some((_, class)) = context.call_context else {
            return Err(JvmCodegenError::JvmFieldReadContextUnavailable {
                tree,
                method: context.method.symbol(),
            });
        };
        let class_symbol = class.identity().symbol();
        if context.control_flow_depth > 0 {
            return Err(JvmCodegenError::JvmFieldReadInUnsupportedControlFlow {
                tree,
                method: context.method.symbol(),
                field: *symbol,
            });
        }
        if context.store.symbols.get(*symbol).owner != Some(class_symbol) {
            return Err(JvmCodegenError::WrongOwner {
                symbol: *symbol,
                expected: class_symbol,
                actual: context.store.symbols.get(*symbol).owner,
            });
        }
        if !class.fields().iter().any(|field| field.symbol() == *symbol) {
            return Err(JvmCodegenError::FieldNotInJvmPlan {
                tree,
                class: class_symbol,
                field: *symbol,
            });
        }
        let symbol_type = match context.store.symbols.info(*symbol) {
            SymbolInfo::Complete(symbol_type) => *symbol_type,
            SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
                return Err(JvmCodegenError::MissingSemanticInfo { symbol: *symbol });
            }
            SymbolInfo::Error => {
                return Err(JvmCodegenError::ErrorSemanticInfo { symbol: *symbol });
            }
        };
        return crate::descriptor::primitive_semantic_value_type(
            context.store,
            context.definitions,
            *symbol,
            symbol_type,
        )
        .map(Some);
    }
    let Some(parameter) = context
        .method
        .parameters()
        .iter()
        .find(|parameter| parameter.symbol() == *symbol)
        .copied()
    else {
        return Err(JvmCodegenError::InvalidParameterReference {
            tree,
            symbol: Some(*symbol),
        });
    };
    if !context.store.symbols.contains(*symbol) {
        return Err(JvmCodegenError::InvalidParameterReference {
            tree,
            symbol: Some(*symbol),
        });
    }
    let parameter_symbol = context.store.symbols.get(*symbol);
    if parameter_symbol.kind != dotty_core::symbols::SymbolKind::Parameter
        || parameter_symbol.owner != Some(context.method.symbol())
    {
        return Err(JvmCodegenError::InvalidParameterReference {
            tree,
            symbol: Some(*symbol),
        });
    }
    let symbol_type = match context.store.symbols.info(*symbol) {
        SymbolInfo::Complete(ty) => Some(*ty),
        SymbolInfo::Missing | SymbolInfo::Deferred(_) | SymbolInfo::Error => None,
    };
    let found = symbol_type.and_then(|ty| primitive_type(context.store, ty, context.definitions));
    if found != Some(parameter.ty()) {
        return Err(JvmCodegenError::ParameterReferenceTypeMismatch {
            tree,
            parameter: *symbol,
            expected: parameter.ty(),
            found,
        });
    }
    Ok(found)
}

fn is_boolean_not_block_result(
    context: &JvmLoweringContext<'_>,
    expression: TreeId<Typed>,
    expected_type: TypeId,
) -> Result<bool, JvmCodegenError> {
    let mut candidate = expression;
    for _ in 0..=JVM_EXPRESSION_DEPTH_LIMIT {
        let expression_tree = context.tree(candidate)?;
        if expression_tree.ty != expected_type {
            return Ok(false);
        }
        if let TreeKind::Block(block) = &expression_tree.kind {
            candidate = block.expr;
            continue;
        }

        let intrinsic = crate::resolve_boolean_intrinsic(
            context.store,
            context.typed_ast,
            candidate,
            context.definitions,
        )
        .map_err(|error| JvmCodegenError::BooleanIntrinsicResolution {
            tree: candidate,
            error,
        })?;
        return Ok(intrinsic == Some(JvmBooleanIntrinsic::Not));
    }
    Ok(false)
}

fn emit_apply(
    context: &mut JvmLoweringContext<'_>,
    tree_id: TreeId<Typed>,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    let (function, arguments) = {
        let tree = context.tree(tree_id)?;
        let TreeKind::Apply(application) = &tree.kind else {
            return Err(JvmCodegenError::UnsupportedTypedExpression {
                tree: tree_id,
                node: tree_kind_name(&tree.kind),
            });
        };
        (application.function, application.args.clone())
    };
    let intrinsic = crate::resolve_boolean_intrinsic(
        context.store,
        context.typed_ast,
        tree_id,
        context.definitions,
    )
    .map_err(|error| JvmCodegenError::BooleanIntrinsicResolution {
        tree: tree_id,
        error,
    })?;
    if let Some(intrinsic) = intrinsic {
        if matches!(
            intrinsic,
            JvmBooleanIntrinsic::ShortCircuitAnd | JvmBooleanIntrinsic::ShortCircuitOr
        ) {
            return emit_short_circuit_boolean_intrinsic(
                context, tree_id, function, &arguments, intrinsic, code, depth,
            );
        }
        return emit_boolean_intrinsic(
            context, tree_id, function, &arguments, intrinsic, code, depth,
        );
    }

    let intrinsic = resolve_int_intrinsic(
        context.store,
        context.typed_ast,
        tree_id,
        context.definitions,
    )
    .map_err(|error| JvmCodegenError::IntIntrinsicResolution {
        tree: tree_id,
        error,
    })?;
    if let Some(intrinsic) = intrinsic {
        return emit_int_intrinsic(
            context, tree_id, function, &arguments, intrinsic, code, depth,
        );
    }

    let Some((unit, class)) = context.call_context else {
        return Err(JvmCodegenError::JvmCallContextUnavailable {
            tree: tree_id,
            method: context.method.symbol(),
        });
    };

    if context.control_flow_depth > 0 {
        return Err(JvmCodegenError::JvmCallInUnsupportedControlFlow {
            tree: tree_id,
            method: context.method.symbol(),
        });
    }
    let caller_class = class.identity().symbol();
    let caller_method = context.method.symbol();
    let target =
        unit.resolve_same_class_call(caller_class, caller_method, tree_id, context.definitions)?;
    let parameters = target.descriptor().parameters().to_vec();
    if arguments.len() != parameters.len() {
        return Err(JvmCodegenError::JvmCallArityMismatch {
            tree: tree_id,
            method: target.method_symbol(),
            expected: parameters.len(),
            found: arguments.len(),
        });
    }

    context.visit(function, depth + 1)?;
    if let TreeKind::Select(select) = &context.tree(function)?.kind {
        context.visit(select.qualifier, depth + 2)?;
    }
    append_current_receiver(
        context.typed_ast,
        class,
        context.method,
        tree_id,
        &target,
        code,
    )?;

    for (index, (argument, expected)) in arguments.into_iter().zip(parameters).enumerate() {
        let flow = emit_expression(context, argument, code, depth + 1)?;
        if flow != Flow::Value(expected) {
            return Err(JvmCodegenError::JvmCallArgumentMismatch {
                tree: tree_id,
                argument,
                method: target.method_symbol(),
                index,
                expected,
                found: public_flow(flow),
            });
        }
    }

    match target.invocation_kind() {
        JvmInvocationKind::Virtual => code
            .invokevirtual(
                target.owner_internal_name(),
                target.method_name(),
                target.descriptor().as_str(),
            )
            .map_err(|error| JvmCodegenError::CodeBuilder {
                tree: tree_id,
                error,
            })?,
    };
    Ok(match target.result() {
        JvmReturnType::Value(value) => Flow::Value(value),
        JvmReturnType::Void => Flow::Unit,
    })
}

fn emit_short_circuit_boolean_intrinsic(
    context: &mut JvmLoweringContext<'_>,
    tree_id: TreeId<Typed>,
    function: TreeId<Typed>,
    arguments: &[TreeId<Typed>],
    intrinsic: JvmBooleanIntrinsic,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    let receiver = match &context.tree(function)?.kind {
        TreeKind::Select(select) => select.qualifier,
        _ => {
            return Err(JvmCodegenError::BooleanIntrinsicResolution {
                tree: tree_id,
                error: crate::JvmBooleanIntrinsicError::UnsupportedFunctionShape { tree: function },
            });
        }
    };
    let [argument] = arguments else {
        return Err(JvmCodegenError::BooleanIntrinsicResolution {
            tree: tree_id,
            error: crate::JvmBooleanIntrinsicError::WrongArity {
                tree: tree_id,
                expected: 1,
                found: arguments.len(),
            },
        });
    };
    let short_circuit_and = match intrinsic {
        JvmBooleanIntrinsic::ShortCircuitAnd => true,
        JvmBooleanIntrinsic::ShortCircuitOr => false,
        _ => {
            return Err(JvmCodegenError::BooleanIntrinsicResolution {
                tree: tree_id,
                error: crate::JvmBooleanIntrinsicError::UnsupportedTreeShape { tree: tree_id },
            });
        }
    };

    let receiver_flow = emit_expression(context, receiver, code, depth + 1)?;
    if receiver_flow != Flow::Value(JvmType::Boolean) {
        return Err(JvmCodegenError::BooleanIntrinsicOperandFlowMismatch {
            tree: tree_id,
            operand: receiver,
            index: 0,
            found: public_flow(receiver_flow),
        });
    }

    let short_circuit = code
        .new_label()
        .map_err(|error| JvmCodegenError::CodeBuilder {
            tree: tree_id,
            error,
        })?;
    let end = code
        .new_label()
        .map_err(|error| JvmCodegenError::CodeBuilder {
            tree: tree_id,
            error,
        })?;
    if short_circuit_and {
        code.ifeq(short_circuit.clone())
    } else {
        code.ifne(short_circuit.clone())
    }
    .map_err(|error| JvmCodegenError::CodeBuilder {
        tree: tree_id,
        error,
    })?;

    let initialized_on_skip = context.initialized_locals.clone();
    context.control_flow_depth += 1;
    let argument_flow = emit_expression(context, *argument, code, depth + 1);
    context.control_flow_depth -= 1;
    let argument_flow = argument_flow?;
    if argument_flow != Flow::Value(JvmType::Boolean) {
        return Err(JvmCodegenError::BooleanIntrinsicOperandFlowMismatch {
            tree: tree_id,
            operand: *argument,
            index: 1,
            found: public_flow(argument_flow),
        });
    }
    let initialized_after_argument = context.initialized_locals.clone();
    if short_circuit_and {
        code.ifeq(short_circuit.clone())
            .and_then(|code| code.iconst(1))
    } else {
        code.ifne(short_circuit.clone())
            .and_then(|code| code.iconst(0))
    }
    .and_then(|code| code.goto(end.clone()))
    .and_then(|code| code.mark(short_circuit))
    .and_then(|code| code.iconst(i32::from(!short_circuit_and)))
    .and_then(|code| code.mark(end))
    .map_err(|error| JvmCodegenError::CodeBuilder {
        tree: tree_id,
        error,
    })?;
    context.initialized_locals = initialized_on_skip
        .intersection(&initialized_after_argument)
        .copied()
        .collect();
    Ok(Flow::Value(JvmType::Boolean))
}

fn emit_boolean_not(
    context: &mut JvmLoweringContext<'_>,
    tree_id: TreeId<Typed>,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    let intrinsic = crate::resolve_boolean_intrinsic(
        context.store,
        context.typed_ast,
        tree_id,
        context.definitions,
    )
    .map_err(|error| JvmCodegenError::BooleanIntrinsicResolution {
        tree: tree_id,
        error,
    })?;
    if intrinsic != Some(JvmBooleanIntrinsic::Not) {
        return Err(JvmCodegenError::UnsupportedTypedExpression {
            tree: tree_id,
            node: "non-intrinsic selection",
        });
    }
    let TreeKind::Select(select) = &context.tree(tree_id)?.kind else {
        return Err(JvmCodegenError::BooleanIntrinsicResolution {
            tree: tree_id,
            error: crate::JvmBooleanIntrinsicError::UnsupportedTreeShape { tree: tree_id },
        });
    };
    let operand = select.qualifier;
    let flow = emit_expression(context, operand, code, depth + 1)?;
    if flow != Flow::Value(JvmType::Boolean) {
        return Err(JvmCodegenError::BooleanIntrinsicOperandFlowMismatch {
            tree: tree_id,
            operand,
            index: 0,
            found: public_flow(flow),
        });
    }
    code.iconst(1)
        .and_then(|code| code.ixor())
        .map_err(|error| JvmCodegenError::CodeBuilder {
            tree: tree_id,
            error,
        })?;
    Ok(Flow::Value(JvmType::Boolean))
}

fn emit_boolean_intrinsic(
    context: &mut JvmLoweringContext<'_>,
    tree_id: TreeId<Typed>,
    function: TreeId<Typed>,
    arguments: &[TreeId<Typed>],
    intrinsic: JvmBooleanIntrinsic,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    let receiver = match &context.tree(function)?.kind {
        TreeKind::Select(select) => select.qualifier,
        _ => {
            return Err(JvmCodegenError::BooleanIntrinsicResolution {
                tree: tree_id,
                error: crate::JvmBooleanIntrinsicError::UnsupportedFunctionShape { tree: function },
            });
        }
    };
    let [argument] = arguments else {
        return Err(JvmCodegenError::BooleanIntrinsicResolution {
            tree: tree_id,
            error: crate::JvmBooleanIntrinsicError::WrongArity {
                tree: tree_id,
                expected: 1,
                found: arguments.len(),
            },
        });
    };
    if matches!(
        intrinsic,
        JvmBooleanIntrinsic::Not
            | JvmBooleanIntrinsic::ShortCircuitAnd
            | JvmBooleanIntrinsic::ShortCircuitOr
    ) {
        return Err(JvmCodegenError::BooleanIntrinsicResolution {
            tree: tree_id,
            error: crate::JvmBooleanIntrinsicError::UnsupportedTreeShape { tree: tree_id },
        });
    }
    for (index, operand) in [receiver, *argument].into_iter().enumerate() {
        let flow = emit_expression(context, operand, code, depth + 1)?;
        if flow != Flow::Value(JvmType::Boolean) {
            return Err(JvmCodegenError::BooleanIntrinsicOperandFlowMismatch {
                tree: tree_id,
                operand,
                index,
                found: public_flow(flow),
            });
        }
    }

    match intrinsic {
        JvmBooleanIntrinsic::And => code.iand(),
        JvmBooleanIntrinsic::Or => code.ior(),
        JvmBooleanIntrinsic::Xor => code.ixor(),
        JvmBooleanIntrinsic::Equal | JvmBooleanIntrinsic::NotEqual => {
            let truthy = code
                .new_label()
                .map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                })?;
            let end = code
                .new_label()
                .map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                })?;
            if intrinsic == JvmBooleanIntrinsic::Equal {
                code.if_icmpeq(truthy.clone())
            } else {
                code.if_icmpne(truthy.clone())
            }
            .map_err(|error| JvmCodegenError::CodeBuilder {
                tree: tree_id,
                error,
            })?;
            code.iconst(0)
                .and_then(|code| code.goto(end.clone()))
                .map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                })?;
            code.mark(truthy)
                .and_then(|code| code.iconst(1))
                .and_then(|code| code.mark(end))
                .map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                })?;
            return Ok(Flow::Value(JvmType::Boolean));
        }
        JvmBooleanIntrinsic::Not
        | JvmBooleanIntrinsic::ShortCircuitAnd
        | JvmBooleanIntrinsic::ShortCircuitOr => {
            unreachable!("unary and short-circuit Boolean operators use dedicated lowering paths")
        }
    }
    .map_err(|error| JvmCodegenError::CodeBuilder {
        tree: tree_id,
        error,
    })?;
    Ok(Flow::Value(JvmType::Boolean))
}

fn emit_int_intrinsic(
    context: &mut JvmLoweringContext<'_>,
    tree_id: TreeId<Typed>,
    function: TreeId<Typed>,
    arguments: &[TreeId<Typed>],
    intrinsic: JvmIntIntrinsic,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    let receiver = match &context.tree(function)?.kind {
        TreeKind::Select(select) => select.qualifier,
        _ => {
            return Err(JvmCodegenError::IntIntrinsicResolution {
                tree: tree_id,
                error: crate::JvmIntIntrinsicError::UnsupportedFunctionShape { tree: function },
            });
        }
    };
    let [argument] = arguments else {
        return Err(JvmCodegenError::IntIntrinsicResolution {
            tree: tree_id,
            error: crate::JvmIntIntrinsicError::WrongArity {
                tree: tree_id,
                expected: 1,
                found: arguments.len(),
            },
        });
    };

    for (index, operand) in [receiver, *argument].into_iter().enumerate() {
        let flow = emit_expression(context, operand, code, depth + 1)?;
        if flow != Flow::Value(JvmType::Int) {
            return Err(JvmCodegenError::IntIntrinsicOperandFlowMismatch {
                tree: tree_id,
                operand,
                index,
                found: public_flow(flow),
            });
        }
    }

    match intrinsic {
        JvmIntIntrinsic::IntAdd => code.iadd(),
        JvmIntIntrinsic::IntSubtract => code.isub(),
        JvmIntIntrinsic::IntMultiply => code.imul(),
        JvmIntIntrinsic::IntDivide => code.idiv(),
        JvmIntIntrinsic::IntRemainder => code.irem(),
        JvmIntIntrinsic::IntLessThan
        | JvmIntIntrinsic::IntLessOrEqual
        | JvmIntIntrinsic::IntGreaterThan
        | JvmIntIntrinsic::IntGreaterOrEqual => {
            let truthy = code
                .new_label()
                .map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                })?;
            let end = code
                .new_label()
                .map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                })?;
            match intrinsic {
                JvmIntIntrinsic::IntLessThan => code.if_icmplt(truthy.clone()),
                JvmIntIntrinsic::IntLessOrEqual => code.if_icmple(truthy.clone()),
                JvmIntIntrinsic::IntGreaterThan => code.if_icmpgt(truthy.clone()),
                JvmIntIntrinsic::IntGreaterOrEqual => code.if_icmpge(truthy.clone()),
                _ => unreachable!("arithmetic operations returned above"),
            }
            .map_err(|error| JvmCodegenError::CodeBuilder {
                tree: tree_id,
                error,
            })?;
            code.iconst(0)
                .map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                })?;
            code.goto(end.clone())
                .map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                })?;
            code.mark(truthy)
                .map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                })?;
            code.iconst(1)
                .map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                })?;
            code.mark(end)
                .map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                })?;
            return Ok(Flow::Value(JvmType::Boolean));
        }
    }
    .map_err(|error| JvmCodegenError::CodeBuilder {
        tree: tree_id,
        error,
    })?;
    Ok(Flow::Value(JvmType::Int))
}

fn emit_if(
    context: &mut JvmLoweringContext<'_>,
    tree_id: TreeId<Typed>,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    context.control_flow_depth += 1;
    let result = emit_if_body(context, tree_id, code, depth);
    context.control_flow_depth -= 1;
    result
}

fn emit_if_body(
    context: &mut JvmLoweringContext<'_>,
    tree_id: TreeId<Typed>,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    let (result_ty, condition, then_branch, else_branch) = {
        let tree = context.tree(tree_id)?;
        let TreeKind::If(if_tree) = &tree.kind else {
            return Err(JvmCodegenError::UnsupportedTypedExpression {
                tree: tree_id,
                node: tree_kind_name(&tree.kind),
            });
        };
        (
            tree.ty,
            if_tree.cond,
            if_tree.then_branch,
            if_tree.else_branch,
        )
    };
    let condition_flow = emit_expression(context, condition, code, depth + 1)?;
    if condition_flow != Flow::Value(JvmType::Boolean) {
        return Err(JvmCodegenError::InvalidIfCondition {
            tree: condition,
            found: match condition_flow {
                Flow::Value(value_type) => Some(value_type),
                Flow::Unit | Flow::Diverges => None,
            },
        });
    }

    let else_label = code
        .new_label()
        .map_err(|error| JvmCodegenError::CodeBuilder {
            tree: tree_id,
            error,
        })?;
    code.ifeq(else_label.clone())
        .map_err(|error| JvmCodegenError::CodeBuilder {
            tree: tree_id,
            error,
        })?;
    let initialized_before_branches = context.initialized_locals.clone();
    let then_flow = emit_expression(context, then_branch, code, depth + 1)?;
    let initialized_then = context.initialized_locals.clone();
    let join_label = if matches!(then_flow, Flow::Value(_) | Flow::Unit) {
        let join_label = code
            .new_label()
            .map_err(|error| JvmCodegenError::CodeBuilder {
                tree: tree_id,
                error,
            })?;
        code.goto(join_label.clone())
            .map_err(|error| JvmCodegenError::CodeBuilder {
                tree: tree_id,
                error,
            })?;
        Some(join_label)
    } else {
        None
    };

    code.mark(else_label)
        .map_err(|error| JvmCodegenError::CodeBuilder {
            tree: tree_id,
            error,
        })?;
    context.initialized_locals = initialized_before_branches.clone();
    let else_flow = emit_expression(context, else_branch, code, depth + 1)?;
    let initialized_else = context.initialized_locals.clone();
    let flow = merge_if_flows(
        context,
        tree_id,
        result_ty,
        then_branch,
        then_flow,
        else_branch,
        else_flow,
    )?;
    context.initialized_locals = match (then_flow, else_flow) {
        (Flow::Value(_) | Flow::Unit, Flow::Value(_) | Flow::Unit) => initialized_then
            .intersection(&initialized_else)
            .copied()
            .collect(),
        (Flow::Value(_) | Flow::Unit, Flow::Diverges) => initialized_then,
        (Flow::Diverges, Flow::Value(_) | Flow::Unit) => initialized_else,
        (Flow::Diverges, Flow::Diverges) => initialized_before_branches,
    };
    if let Some(join_label) = join_label {
        code.mark(join_label)
            .map_err(|error| JvmCodegenError::CodeBuilder {
                tree: tree_id,
                error,
            })?;
    }
    Ok(flow)
}

fn emit_while(
    context: &mut JvmLoweringContext<'_>,
    tree_id: TreeId<Typed>,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    context.control_flow_depth += 1;
    let result = emit_while_body(context, tree_id, code, depth);
    context.control_flow_depth -= 1;
    result
}

fn emit_while_body(
    context: &mut JvmLoweringContext<'_>,
    tree_id: TreeId<Typed>,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    let (result_type, condition, body) = {
        let tree = context.tree(tree_id)?;
        let TreeKind::While(loop_tree) = &tree.kind else {
            return Err(JvmCodegenError::UnsupportedTypedExpression {
                tree: tree_id,
                node: tree_kind_name(&tree.kind),
            });
        };
        (tree.ty, loop_tree.cond, loop_tree.body)
    };
    if !is_unit_type(context.store, result_type, context.definitions) {
        return Err(JvmCodegenError::WhileResultTypeMismatch {
            tree: tree_id,
            ty: result_type,
        });
    }

    let header = code
        .new_label()
        .map_err(|error| JvmCodegenError::CodeBuilder {
            tree: tree_id,
            error,
        })?;
    let exit = code
        .new_label()
        .map_err(|error| JvmCodegenError::CodeBuilder {
            tree: tree_id,
            error,
        })?;
    // Give a loop at method offset zero a non-entry frame target for its back-edge.
    code.goto(header.clone())
        .map_err(|error| JvmCodegenError::CodeBuilder {
            tree: tree_id,
            error,
        })?;
    code.mark(header.clone())
        .map_err(|error| JvmCodegenError::CodeBuilder {
            tree: tree_id,
            error,
        })?;

    let initialized_before_loop = context.initialized_locals.clone();
    let condition_flow = emit_expression(context, condition, code, depth + 1)?;
    if condition_flow != Flow::Value(JvmType::Boolean) {
        return Err(JvmCodegenError::InvalidWhileCondition {
            tree: condition,
            found: match condition_flow {
                Flow::Value(value) => Some(value),
                Flow::Unit | Flow::Diverges => None,
            },
        });
    }
    code.ifeq(exit.clone())
        .map_err(|error| JvmCodegenError::CodeBuilder {
            tree: tree_id,
            error,
        })?;

    let body_flow = emit_expression(context, body, code, depth + 1)?;
    match body_flow {
        Flow::Value(_) => {
            code.pop()
                .map_err(|error| JvmCodegenError::CodeBuilder { tree: body, error })?;
            code.goto(header)
                .map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                })?;
        }
        Flow::Unit => {
            code.goto(header)
                .map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                })?;
        }
        Flow::Diverges => {}
    }
    code.mark(exit)
        .map_err(|error| JvmCodegenError::CodeBuilder {
            tree: tree_id,
            error,
        })?;
    context.initialized_locals = initialized_before_loop;
    Ok(Flow::Unit)
}

fn merge_if_flows(
    context: &JvmLoweringContext<'_>,
    tree: TreeId<Typed>,
    result_ty: TypeId,
    then_tree: TreeId<Typed>,
    then_flow: Flow,
    else_tree: TreeId<Typed>,
    else_flow: Flow,
) -> Result<Flow, JvmCodegenError> {
    validate_branch(context, then_tree, then_flow)?;
    validate_branch(context, else_tree, else_flow)?;
    let merged = match (then_flow, else_flow) {
        (Flow::Value(left), Flow::Value(right)) => {
            if left != right {
                return Err(JvmCodegenError::IfBranchTypeMismatch {
                    tree,
                    branch: else_tree,
                    expected: left,
                    found: Some(right),
                });
            }
            Flow::Value(left)
        }
        (Flow::Value(value), Flow::Diverges) | (Flow::Diverges, Flow::Value(value)) => {
            Flow::Value(value)
        }
        (Flow::Unit, Flow::Unit | Flow::Diverges) | (Flow::Diverges, Flow::Unit) => Flow::Unit,
        (Flow::Diverges, Flow::Diverges) => Flow::Diverges,
        (Flow::Value(value), Flow::Unit) | (Flow::Unit, Flow::Value(value)) => {
            return Err(JvmCodegenError::IfResultTypeMismatch {
                tree,
                expected: primitive_type(context.store, result_ty, context.definitions),
                found: Some(value),
            });
        }
    };
    let expected = primitive_type(context.store, result_ty, context.definitions);
    match merged {
        Flow::Value(found) if expected == Some(found) => Ok(Flow::Value(found)),
        Flow::Unit if is_unit_type(context.store, result_ty, context.definitions) => Ok(Flow::Unit),
        Flow::Diverges => Ok(Flow::Diverges),
        found => Err(JvmCodegenError::IfResultTypeMismatch {
            tree,
            expected,
            found: match found {
                Flow::Value(value) => Some(value),
                Flow::Unit | Flow::Diverges => None,
            },
        }),
    }
}

fn validate_branch(
    context: &JvmLoweringContext<'_>,
    tree: TreeId<Typed>,
    flow: Flow,
) -> Result<(), JvmCodegenError> {
    let tree_ty = context.tree(tree)?.ty;
    match flow {
        Flow::Diverges => {
            // Recursive lowering proves terminal flow before these Unit-valued
            // forms can return Diverges; other Unit nodes remain unsupported.
            let diverges_with_supported_type =
                is_nothing_type(context.store, tree_ty, context.definitions)
                    || (is_unit_type(context.store, tree_ty, context.definitions)
                        && matches!(
                            &context.tree(tree)?.kind,
                            TreeKind::If(_) | TreeKind::Assign(_) | TreeKind::Block(_)
                        ));
            if diverges_with_supported_type {
                Ok(())
            } else {
                Err(JvmCodegenError::UnsupportedBottomFlow { tree })
            }
        }
        Flow::Unit if is_unit_type(context.store, tree_ty, context.definitions) => Ok(()),
        Flow::Unit => Err(JvmCodegenError::UnitFlowTypeMismatch { tree, ty: tree_ty }),
        Flow::Value(_) if is_nothing_type(context.store, tree_ty, context.definitions) => {
            Err(JvmCodegenError::UnsupportedBottomFlow { tree })
        }
        Flow::Value(_) => Ok(()),
    }
}

fn emit_return(
    context: &mut JvmLoweringContext<'_>,
    tree_id: TreeId<Typed>,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    let (return_ty, expr, from) = {
        let tree = context.tree(tree_id)?;
        let TreeKind::Return(return_tree) = &tree.kind else {
            return Err(JvmCodegenError::UnsupportedTypedExpression {
                tree: tree_id,
                node: tree_kind_name(&tree.kind),
            });
        };
        (tree.ty, return_tree.expr, return_tree.from)
    };
    if let Some(from) = from {
        return Err(JvmCodegenError::UnsupportedReturnTarget {
            tree: tree_id,
            from,
        });
    }
    if !is_nothing_type(context.store, return_ty, context.definitions) {
        return Err(JvmCodegenError::UnsupportedBottomFlow { tree: tree_id });
    }
    match (expr, context.method.descriptor().result()) {
        (Some(value), expected) => match emit_expression(context, value, code, depth + 1)? {
            Flow::Diverges => Ok(Flow::Diverges),
            Flow::Unit if expected == JvmReturnType::Void => {
                code.return_void()
                    .map_err(|error| JvmCodegenError::CodeBuilder {
                        tree: tree_id,
                        error,
                    })?;
                Ok(Flow::Diverges)
            }
            Flow::Unit => Err(JvmCodegenError::ReturnValueTypeMismatch {
                tree: tree_id,
                expected,
                found: None,
            }),
            Flow::Value(found) if expected == JvmReturnType::Value(found) => {
                code.ireturn()
                    .map_err(|error| JvmCodegenError::CodeBuilder {
                        tree: tree_id,
                        error,
                    })?;
                Ok(Flow::Diverges)
            }
            Flow::Value(found) => Err(JvmCodegenError::ReturnValueTypeMismatch {
                tree: tree_id,
                expected,
                found: Some(found),
            }),
        },
        (None, JvmReturnType::Void) => {
            code.return_void()
                .map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                })?;
            Ok(Flow::Diverges)
        }
        (None, expected) => Err(JvmCodegenError::ReturnValueTypeMismatch {
            tree: tree_id,
            expected,
            found: None,
        }),
    }
}

fn public_flow(flow: Flow) -> JvmExpressionFlow {
    match flow {
        Flow::Value(value_type) => JvmExpressionFlow::Value {
            value_type,
            category: value_type.category(),
        },
        Flow::Unit => JvmExpressionFlow::Unit,
        Flow::Diverges => JvmExpressionFlow::Diverges,
    }
}

fn is_unit_type(store: &SemanticStore, ty: TypeId, definitions: &Definitions) -> bool {
    if matches!(
        store.types.try_get(ty),
        Some(Type::Constant(Constant::Unit))
    ) {
        return true;
    }
    type_symbol(store, definitions.unit).is_some_and(|unit| type_symbol(store, ty) == Some(unit))
}

fn is_nothing_type(store: &SemanticStore, ty: TypeId, definitions: &Definitions) -> bool {
    match (
        type_symbol(store, ty),
        type_symbol(store, definitions.nothing_type),
    ) {
        (Some(found), Some(nothing)) => found == nothing,
        _ => false,
    }
}

pub(super) fn validate_tree_type(
    store: &SemanticStore,
    tree: TreeId<Typed>,
    ty: TypeId,
) -> Result<(), JvmCodegenError> {
    let reason = match store.types.try_get(ty) {
        None => Some(InvalidTypeReason::MissingOrUnfilled),
        Some(Type::NoType) => Some(InvalidTypeReason::NoType),
        Some(Type::Error(_)) => Some(InvalidTypeReason::Error),
        Some(Type::NoPrefix) => Some(InvalidTypeReason::NoPrefix),
        Some(_) => None,
    };
    match reason {
        Some(reason) => Err(JvmCodegenError::InvalidExpressionType { tree, ty, reason }),
        None => Ok(()),
    }
}

fn primitive_type(store: &SemanticStore, ty: TypeId, definitions: &Definitions) -> Option<JvmType> {
    match store.types.try_get(ty)? {
        Type::Constant(Constant::Int(_)) => Some(JvmType::Int),
        Type::Constant(Constant::Boolean(_)) => Some(JvmType::Boolean),
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } if *symbol == type_symbol(store, definitions.int)? => Some(JvmType::Int),
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } if *symbol == type_symbol(store, definitions.boolean)? => Some(JvmType::Boolean),
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

fn tree_kind_name(kind: &TreeKind<Typed>) -> &'static str {
    match kind {
        TreeKind::Ident(_) => "Ident",
        TreeKind::Select(_) => "Select",
        TreeKind::This(_) => "This",
        TreeKind::Super(_) => "Super",
        TreeKind::Literal(_) => "Literal",
        TreeKind::Apply(_) => "Apply",
        TreeKind::TypeApply(_) => "TypeApply",
        TreeKind::New(_) => "New",
        TreeKind::Typed(_) => "Typed",
        TreeKind::NamedArg(_) => "NamedArg",
        TreeKind::Assign(_) => "Assign",
        TreeKind::Block(_) => "Block",
        TreeKind::If(_) => "If",
        TreeKind::Match(_) => "Match",
        TreeKind::CaseDef(_) => "CaseDef",
        TreeKind::Return(_) => "Return",
        TreeKind::While(_) => "While",
        TreeKind::Try(_) => "Try",
        TreeKind::Closure(_) => "Closure",
        TreeKind::ValDef(_) => "ValDef",
        TreeKind::DefDef(_) => "DefDef",
        TreeKind::TypeDef(_) => "TypeDef",
        TreeKind::Template(_) => "Template",
        TreeKind::PackageDef(_) => "PackageDef",
        TreeKind::Import(_) => "Import",
        TreeKind::Export(_) => "Export",
        TreeKind::TypeTree(_) => "TypeTree",
        TreeKind::SingletonTypeTree(_) => "SingletonTypeTree",
        TreeKind::AppliedTypeTree(_) => "AppliedTypeTree",
        TreeKind::RefinedTypeTree(_) => "RefinedTypeTree",
        TreeKind::LambdaTypeTree(_) => "LambdaTypeTree",
        TreeKind::MatchTypeTree(_) => "MatchTypeTree",
        TreeKind::ByNameTypeTree(_) => "ByNameTypeTree",
        TreeKind::TypeBoundsTree(_) => "TypeBoundsTree",
        TreeKind::Bind(_) => "Bind",
        TreeKind::Alternative(_) => "Alternative",
        TreeKind::UnApply(_) => "UnApply",
        TreeKind::Annotated(_) => "Annotated",
        TreeKind::Quote(_) => "Quote",
        TreeKind::Splice(_) => "Splice",
        TreeKind::QuotePattern(_) => "QuotePattern",
        TreeKind::SplicePattern(_) => "SplicePattern",
        TreeKind::Inlined(_) => "Inlined",
        TreeKind::PhaseSpecific(_) => "PhaseSpecific",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_classfile::attribute::Attribute;
    use dotty_classfile::bytecode::CodeBuilder;
    use dotty_classfile::class_file::ClassFile;
    use dotty_classfile::class_file::ClassFileVersion;
    use dotty_classfile::class_file_builder::ClassFileBuilder;
    use dotty_classfile::descriptor::MethodDescriptor;
    use dotty_classfile::reader::Reader;
    use dotty_core::ast::{Ident, Tree};
    use dotty_core::ids::TypeId;
    use dotty_core::names::{Name, Namespace};
    use dotty_core::symbols::{
        Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
    };
    use dotty_core::types::{TermRefTarget, Type};

    use crate::{JvmMethodDescriptor, JvmParameterLocal};

    struct Fixture {
        store: SemanticStore,
        definitions: Definitions,
        ast: TypedAst,
        method: JvmMethodMetadata,
        parameters: Vec<SymbolId>,
    }

    impl Fixture {
        fn new(parameter_types: &[JvmType]) -> Self {
            let mut store = SemanticStore::new();
            let definitions = Definitions::bootstrap(&mut store);
            let class = symbol(&mut store, SymbolKind::Class, None, definitions.int);
            let method_symbol =
                symbol(&mut store, SymbolKind::Method, Some(class), definitions.int);
            let mut parameters = Vec::new();
            let mut locals = Vec::new();
            for (slot, ty) in (1u16..).zip(parameter_types.iter()) {
                let param_type = match ty {
                    JvmType::Int => definitions.int,
                    JvmType::Boolean => definitions.boolean,
                };
                let parameter = symbol(
                    &mut store,
                    SymbolKind::Parameter,
                    Some(method_symbol),
                    param_type,
                );
                parameters.push(parameter);
                locals.push(JvmParameterLocal {
                    symbol: parameter,
                    ty: *ty,
                    slot,
                    width: 1,
                });
            }
            let descriptor_text = format!(
                "({})I",
                parameter_types
                    .iter()
                    .map(|ty| match ty {
                        JvmType::Int => 'I',
                        JvmType::Boolean => 'Z',
                    })
                    .collect::<String>()
            );
            let descriptor = MethodDescriptor::parse(&descriptor_text).unwrap();
            let method = JvmMethodMetadata {
                symbol: method_symbol,
                name: "value".to_owned(),
                descriptor: JvmMethodDescriptor {
                    descriptor,
                    descriptor_text,
                    parameters: parameter_types.to_vec(),
                    result: crate::JvmReturnType::Value(JvmType::Int),
                },
                access_flags: dotty_classfile::access_flags::MethodAccessFlags(
                    dotty_classfile::access_flags::ACC_PUBLIC,
                ),
                parameters: locals,
            };
            Self {
                store,
                definitions,
                ast: TypedAst::new(),
                method,
                parameters,
            }
        }

        fn literal(&mut self, value: Constant, ty: TypeId) -> TreeId<Typed> {
            self.ast.alloc(Tree {
                kind: TreeKind::Literal(dotty_core::ast::Literal { value }),
                position: None,
                ty,
            })
        }

        fn ident(&mut self, ty: TypeId) -> TreeId<Typed> {
            self.ast.alloc(Tree {
                kind: TreeKind::Ident(Ident {
                    name: Name::new(self.store.names.intern("sameName"), Namespace::Term),
                    backquoted: false,
                }),
                position: None,
                ty,
            })
        }

        fn if_expr(
            &mut self,
            condition: TreeId<Typed>,
            then_branch: TreeId<Typed>,
            else_branch: TreeId<Typed>,
            ty: TypeId,
        ) -> TreeId<Typed> {
            self.ast.alloc(Tree {
                kind: TreeKind::If(dotty_core::ast::If {
                    cond: condition,
                    then_branch,
                    else_branch,
                }),
                position: None,
                ty,
            })
        }

        fn block(
            &mut self,
            stats: Vec<TreeId<Typed>>,
            expr: TreeId<Typed>,
            ty: TypeId,
        ) -> TreeId<Typed> {
            self.ast.alloc(Tree {
                kind: TreeKind::Block(dotty_core::ast::Block { stats, expr }),
                position: None,
                ty,
            })
        }

        fn return_expr(
            &mut self,
            expr: Option<TreeId<Typed>>,
            from: Option<TreeId<Typed>>,
        ) -> TreeId<Typed> {
            self.ast.alloc(Tree {
                kind: TreeKind::Return(dotty_core::ast::Return { expr, from }),
                position: None,
                ty: self.definitions.nothing_type,
            })
        }

        fn set_result(&mut self, result: crate::JvmReturnType) {
            self.method.descriptor.result = result;
            let parameters = self
                .method
                .descriptor
                .parameters
                .iter()
                .map(|ty| match ty {
                    JvmType::Int => 'I',
                    JvmType::Boolean => 'Z',
                })
                .collect::<String>();
            let suffix = match result {
                JvmReturnType::Value(JvmType::Int) => "I",
                JvmReturnType::Value(JvmType::Boolean) => "Z",
                JvmReturnType::Void => "V",
            };
            self.method.descriptor.descriptor_text = format!("({parameters}){suffix}");
            self.method.descriptor.descriptor =
                MethodDescriptor::parse(&self.method.descriptor.descriptor_text).unwrap();
        }
    }

    fn symbol(
        store: &mut SemanticStore,
        kind: SymbolKind,
        owner: Option<SymbolId>,
        ty: TypeId,
    ) -> SymbolId {
        store.symbols.alloc(Symbol {
            name: Name::new(store.names.intern("sameName"), Namespace::Term),
            owner,
            kind,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(ty),
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        })
    }

    fn code_bytes(fixture: &Fixture, code: &CodeBuilder) -> Vec<u8> {
        let mut encodable = code.clone();
        encodable.ireturn().unwrap();
        encoded_method_code(fixture, encodable)
    }

    fn encoded_method_code(fixture: &Fixture, encodable: CodeBuilder) -> Vec<u8> {
        let bytes = encoded_class_bytes(fixture, encodable);
        let mut reader = Reader::new(&bytes);
        let decoded = ClassFile::decode(&mut reader).unwrap();
        decoded.methods[0]
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::Code(code) => Some(code.code.to_vec()),
                _ => None,
            })
            .unwrap()
    }

    fn encoded_class_bytes(fixture: &Fixture, encodable: CodeBuilder) -> Vec<u8> {
        let mut class = ClassFileBuilder::new(
            "Example",
            ClassFileVersion {
                minor: 0,
                major: 65,
            },
        )
        .unwrap();
        class
            .add_instance_method(
                "value",
                fixture.method.descriptor.descriptor_text.as_str(),
                |output| {
                    *output = encodable;
                    Ok(())
                },
            )
            .unwrap();
        class.encode().unwrap()
    }

    fn has_stack_map(bytes: &[u8]) -> bool {
        let mut reader = Reader::new(bytes);
        let decoded = ClassFile::decode(&mut reader).unwrap();
        decoded.methods[0].attributes.iter().any(|attribute| {
            let Attribute::Code(code) = attribute else {
                return false;
            };
            code.attributes.iter().any(|attribute| {
                let Attribute::Other(raw) = attribute else {
                    return false;
                };
                decoded.constant_pool.utf8(raw.name_index).ok() == Some("StackMapTable")
            })
        })
    }

    fn method_body_bytes(fixture: &Fixture, code: CodeBuilder) -> Vec<u8> {
        encoded_method_code(fixture, code)
    }

    fn lower_and_return(fixture: &Fixture, tree: TreeId<Typed>) -> (JvmValueCategory, Vec<u8>) {
        let mut code = CodeBuilder::new();
        let category = lower_value_expression(
            &fixture.store,
            &fixture.ast,
            &fixture.definitions,
            &fixture.method,
            tree,
            &mut code,
        )
        .unwrap();
        (category, code_bytes(fixture, &code))
    }

    #[test]
    fn lowers_integer_and_boolean_literals_to_int_like_values() {
        let mut fixture = Fixture::new(&[]);
        let int_value = fixture.literal(Constant::Int(42), fixture.definitions.int);
        let true_value = fixture.literal(Constant::Boolean(true), fixture.definitions.boolean);
        let false_value = fixture.literal(Constant::Boolean(false), fixture.definitions.boolean);

        assert_eq!(
            lower_and_return(&fixture, int_value),
            (JvmValueCategory::IntLike, vec![0x10, 42, 0xac])
        );
        assert_eq!(
            lower_and_return(&fixture, true_value),
            (JvmValueCategory::IntLike, vec![0x04, 0xac])
        );
        assert_eq!(
            lower_and_return(&fixture, false_value),
            (JvmValueCategory::IntLike, vec![0x03, 0xac])
        );
    }

    #[test]
    fn lowers_parameter_references_by_symbol_identity_and_ordered_slot() {
        let mut fixture = Fixture::new(&[JvmType::Int, JvmType::Int]);
        let int_type = fixture.definitions.int;
        let first = fixture.parameters[0];
        let second = fixture.parameters[1];
        let first_ref = fixture.store.types.alloc(Type::TermRef {
            prefix: fixture.definitions.no_prefix,
            target: TermRefTarget::Symbol(first),
        });
        let second_ref = fixture.store.types.alloc(Type::TermRef {
            prefix: fixture.definitions.no_prefix,
            target: TermRefTarget::Symbol(second),
        });
        let first_tree = fixture.ident(first_ref);
        let second_tree = fixture.ident(second_ref);
        assert_eq!(
            lower_and_return(&fixture, first_tree),
            (JvmValueCategory::IntLike, vec![0x1b, 0xac])
        );
        assert_eq!(
            lower_and_return(&fixture, second_tree),
            (JvmValueCategory::IntLike, vec![0x1c, 0xac])
        );

        let other = symbol(
            &mut fixture.store,
            SymbolKind::Parameter,
            Some(fixture.method.symbol()),
            int_type,
        );
        let other_ref = fixture.store.types.alloc(Type::TermRef {
            prefix: fixture.definitions.no_prefix,
            target: TermRefTarget::Symbol(other),
        });
        let other_tree = fixture.ident(other_ref);
        let mut code = CodeBuilder::new();
        code.iconst(0).unwrap();
        let before = code_bytes(&fixture, &code);
        assert_eq!(
            lower_value_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                other_tree,
                &mut code,
            ),
            Err(JvmCodegenError::InvalidParameterReference {
                tree: other_tree,
                symbol: Some(other),
            })
        );
        assert_eq!(code_bytes(&fixture, &code), before);
    }

    #[test]
    fn boolean_and_integer_parameters_use_their_ordered_int_like_slots() {
        let mut fixture = Fixture::new(&[JvmType::Boolean, JvmType::Int, JvmType::Int]);
        let parameter_symbols = fixture.parameters.clone();
        let references = parameter_symbols
            .iter()
            .copied()
            .map(|symbol| {
                let ty = fixture.store.types.alloc(Type::TermRef {
                    prefix: fixture.definitions.no_prefix,
                    target: TermRefTarget::Symbol(symbol),
                });
                fixture.ident(ty)
            })
            .collect::<Vec<_>>();
        for (tree, expected) in
            references
                .into_iter()
                .zip([vec![0x1b, 0xac], vec![0x1c, 0xac], vec![0x1d, 0xac]])
        {
            assert_eq!(
                lower_and_return(&fixture, tree),
                (JvmValueCategory::IntLike, expected)
            );
        }
    }

    #[test]
    fn rejects_parameter_metadata_that_no_longer_matches_store_ownership() {
        let mut fixture = Fixture::new(&[JvmType::Int]);
        let parameter = fixture.parameters[0];
        let reference_type = fixture.store.types.alloc(Type::TermRef {
            prefix: fixture.definitions.no_prefix,
            target: TermRefTarget::Symbol(parameter),
        });
        let tree = fixture.ident(reference_type);
        let other_method = symbol(
            &mut fixture.store,
            SymbolKind::Method,
            None,
            fixture.definitions.int,
        );
        fixture.store.symbols.get_mut(parameter).owner = Some(other_method);

        let mut code = CodeBuilder::new();
        code.iconst(0).unwrap();
        let before = code_bytes(&fixture, &code);
        assert_eq!(
            lower_value_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                tree,
                &mut code,
            ),
            Err(JvmCodegenError::InvalidParameterReference {
                tree,
                symbol: Some(parameter),
            })
        );
        assert_eq!(code_bytes(&fixture, &code), before);
    }

    #[test]
    fn source_independent_lowering_rejects_locals_without_a_validated_layout() {
        let mut fixture = Fixture::new(&[]);
        let local = symbol(
            &mut fixture.store,
            SymbolKind::Local,
            Some(fixture.method.symbol()),
            fixture.definitions.int,
        );
        let reference_type = fixture.store.types.alloc(Type::TermRef {
            prefix: fixture.definitions.no_prefix,
            target: TermRefTarget::Symbol(local),
        });
        let tree = fixture.ident(reference_type);
        let mut code = CodeBuilder::new();
        code.iconst(0).unwrap();
        let before = code_bytes(&fixture, &code);

        assert_eq!(
            lower_value_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                tree,
                &mut code,
            ),
            Err(JvmCodegenError::MissingLocalBinding {
                tree,
                symbol: local
            })
        );
        assert_eq!(code_bytes(&fixture, &code), before);

        let name_only_type = fixture.store.types.alloc(Type::TermRef {
            prefix: fixture.definitions.no_prefix,
            target: TermRefTarget::Name(dotty_core::names::TermName::new(
                fixture.store.names.intern("sameName"),
            )),
        });
        let name_only = fixture.ident(name_only_type);
        assert_eq!(
            lower_value_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                name_only,
                &mut code,
            ),
            Err(JvmCodegenError::InvalidParameterReference {
                tree: name_only,
                symbol: None,
            })
        );
        assert_eq!(code_bytes(&fixture, &code), before);
    }

    #[test]
    fn rejects_mismatched_and_unsupported_leaves_without_mutating_code() {
        let mut fixture = Fixture::new(&[]);
        let mismatch = fixture.literal(Constant::Int(7), fixture.definitions.boolean);
        let singleton_type = fixture.store.types.alloc(Type::Constant(Constant::Int(5)));
        let mismatched_singleton = fixture.literal(Constant::Int(7), singleton_type);
        let unsupported = fixture.literal(Constant::Long(7), fixture.definitions.long);
        for (tree, expected) in [
            (
                mismatch,
                JvmCodegenError::LiteralTypeMismatch {
                    tree: mismatch,
                    value: JvmType::Int,
                    typed: Some(JvmType::Boolean),
                },
            ),
            (
                mismatched_singleton,
                JvmCodegenError::LiteralTypeMismatch {
                    tree: mismatched_singleton,
                    value: JvmType::Int,
                    typed: Some(JvmType::Int),
                },
            ),
            (
                unsupported,
                JvmCodegenError::UnsupportedTypedExpression {
                    tree: unsupported,
                    node: "non-Int/Boolean literal",
                },
            ),
        ] {
            let mut code = CodeBuilder::new();
            code.iconst(0).unwrap();
            let before = code_bytes(&fixture, &code);
            assert_eq!(
                lower_value_expression(
                    &fixture.store,
                    &fixture.ast,
                    &fixture.definitions,
                    &fixture.method,
                    tree,
                    &mut code,
                ),
                Err(expected)
            );
            assert_eq!(code_bytes(&fixture, &code), before);
        }
    }

    #[test]
    fn rejects_a_missing_or_sentinel_typed_node() {
        let mut fixture = Fixture::new(&[]);
        let tree = fixture.literal(Constant::Int(1), fixture.definitions.no_prefix);
        let mut code = CodeBuilder::new();
        code.iconst(0).unwrap();
        let before = code_bytes(&fixture, &code);
        assert_eq!(
            lower_value_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                tree,
                &mut code,
            ),
            Err(JvmCodegenError::InvalidExpressionType {
                tree,
                ty: fixture.definitions.no_prefix,
                reason: InvalidTypeReason::NoPrefix,
            })
        );
        assert_eq!(code_bytes(&fixture, &code), before);
    }

    #[test]
    fn lowers_value_if_branches_to_a_deterministic_join_with_stack_maps() {
        let mut fixture = Fixture::new(&[JvmType::Boolean, JvmType::Int, JvmType::Int]);
        let ids = fixture.parameters.clone();
        let refs = ids
            .iter()
            .copied()
            .map(|symbol| {
                let ty = fixture.store.types.alloc(Type::TermRef {
                    prefix: fixture.definitions.no_prefix,
                    target: TermRefTarget::Symbol(symbol),
                });
                fixture.ident(ty)
            })
            .collect::<Vec<_>>();
        let root = fixture.if_expr(refs[0], refs[1], refs[2], fixture.definitions.int);

        let first = lower_method_body(
            &fixture.store,
            &fixture.ast,
            &fixture.definitions,
            &fixture.method,
            root,
        )
        .unwrap();
        let first_class = encoded_class_bytes(&fixture, first.clone());
        assert_eq!(
            encoded_method_code(&fixture, first.clone()),
            vec![0x1b, 0x99, 0x00, 0x07, 0x1c, 0xa7, 0x00, 0x04, 0x1d, 0xac]
        );
        assert!(has_stack_map(&first_class));

        let second = lower_method_body(
            &fixture.store,
            &fixture.ast,
            &fixture.definitions,
            &fixture.method,
            root,
        )
        .unwrap();
        assert_eq!(first_class, encoded_class_bytes(&fixture, second));
    }

    #[test]
    fn unit_literal_reports_no_stack_value_and_gets_a_void_epilogue() {
        let mut fixture = Fixture::new(&[]);
        let constant_unit_type = fixture.store.types.alloc(Type::Constant(Constant::Unit));
        let unit = fixture.literal(Constant::Unit, constant_unit_type);
        let lowered = lower_expression(
            &fixture.store,
            &fixture.ast,
            &fixture.definitions,
            &fixture.method,
            unit,
        )
        .unwrap();
        assert_eq!(lowered.flow(), JvmExpressionFlow::Unit);
        fixture.set_result(JvmReturnType::Void);
        let mut expression_code = lowered.into_code();
        expression_code.return_void().unwrap();
        assert_eq!(encoded_method_code(&fixture, expression_code), vec![0xb1]);

        fixture.set_result(JvmReturnType::Value(JvmType::Int));
        assert_eq!(
            lower_method_body(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                unit,
            )
            .unwrap_err(),
            JvmCodegenError::MethodBodyFlowMismatch {
                method: fixture.method.symbol(),
                tree: unit,
                expected: JvmReturnType::Value(JvmType::Int),
                found: JvmExpressionFlow::Unit,
            }
        );

        fixture.set_result(JvmReturnType::Void);
        assert_eq!(
            method_body_bytes(
                &fixture,
                lower_method_body(
                    &fixture.store,
                    &fixture.ast,
                    &fixture.definitions,
                    &fixture.method,
                    unit,
                )
                .unwrap(),
            ),
            vec![0xb1]
        );
    }

    #[test]
    fn blocks_discard_value_stats_keep_unit_empty_and_preserve_final_result() {
        let mut fixture = Fixture::new(&[]);
        let int_stat = fixture.literal(Constant::Int(7), fixture.definitions.int);
        let unit_stat = fixture.literal(Constant::Unit, fixture.definitions.unit);
        let boolean_stat = fixture.literal(Constant::Boolean(true), fixture.definitions.boolean);
        let result = fixture.literal(Constant::Int(42), fixture.definitions.int);
        let block = fixture.block(
            vec![int_stat, unit_stat, boolean_stat],
            result,
            fixture.definitions.int,
        );

        assert_eq!(
            method_body_bytes(
                &fixture,
                lower_method_body(
                    &fixture.store,
                    &fixture.ast,
                    &fixture.definitions,
                    &fixture.method,
                    block,
                )
                .unwrap(),
            ),
            vec![0x10, 7, 0x57, 0x04, 0x57, 0x10, 42, 0xac]
        );
    }

    #[test]
    fn nested_blocks_discard_each_intermediate_result_once() {
        let mut fixture = Fixture::new(&[]);
        let first = fixture.literal(Constant::Int(20), fixture.definitions.int);
        let inner_result = fixture.literal(Constant::Int(21), fixture.definitions.int);
        let inner = fixture.block(vec![first], inner_result, fixture.definitions.int);
        let outer_result = fixture.literal(Constant::Int(42), fixture.definitions.int);
        let outer = fixture.block(vec![inner], outer_result, fixture.definitions.int);

        assert_eq!(
            method_body_bytes(
                &fixture,
                lower_method_body(
                    &fixture.store,
                    &fixture.ast,
                    &fixture.definitions,
                    &fixture.method,
                    outer,
                )
                .unwrap(),
            ),
            vec![0x10, 20, 0x57, 0x10, 21, 0x57, 0x10, 42, 0xac]
        );
    }

    #[test]
    fn block_with_unit_result_gets_only_a_void_return() {
        let mut fixture = Fixture::new(&[]);
        fixture.set_result(JvmReturnType::Void);
        let int_stat = fixture.literal(Constant::Int(7), fixture.definitions.int);
        let unit_stat = fixture.literal(Constant::Unit, fixture.definitions.unit);
        let unit_result = fixture.literal(Constant::Unit, fixture.definitions.unit);
        let block = fixture.block(
            vec![int_stat, unit_stat],
            unit_result,
            fixture.definitions.unit,
        );

        assert_eq!(
            method_body_bytes(
                &fixture,
                lower_method_body(
                    &fixture.store,
                    &fixture.ast,
                    &fixture.definitions,
                    &fixture.method,
                    block,
                )
                .unwrap(),
            ),
            vec![0x10, 7, 0x57, 0xb1]
        );
    }

    #[test]
    fn block_result_type_must_match_the_final_expression_flow() {
        let mut fixture = Fixture::new(&[]);
        let result = fixture.literal(Constant::Int(42), fixture.definitions.int);
        let block = fixture.block(Vec::new(), result, fixture.definitions.boolean);
        assert_eq!(
            lower_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                block,
            )
            .unwrap_err(),
            JvmCodegenError::BlockResultTypeMismatch {
                tree: block,
                declared: fixture.definitions.boolean,
                found: JvmExpressionFlow::Value {
                    value_type: JvmType::Int,
                    category: JvmValueCategory::IntLike,
                },
            }
        );

        let mut wrong_method_result = Fixture::new(&[]);
        wrong_method_result.set_result(JvmReturnType::Value(JvmType::Boolean));
        let value =
            wrong_method_result.literal(Constant::Int(42), wrong_method_result.definitions.int);
        let block =
            wrong_method_result.block(Vec::new(), value, wrong_method_result.definitions.int);
        assert_eq!(
            lower_method_body(
                &wrong_method_result.store,
                &wrong_method_result.ast,
                &wrong_method_result.definitions,
                &wrong_method_result.method,
                block,
            )
            .unwrap_err(),
            JvmCodegenError::MethodBodyTypeMismatch {
                method: wrong_method_result.method.symbol(),
                tree: block,
                expected: JvmReturnType::Value(JvmType::Boolean),
                found: Some(JvmType::Int),
            }
        );

        let mut wrong_unit = Fixture::new(&[]);
        let unit = wrong_unit.literal(Constant::Unit, wrong_unit.definitions.unit);
        let block = wrong_unit.block(Vec::new(), unit, wrong_unit.definitions.int);
        assert_eq!(
            lower_expression(
                &wrong_unit.store,
                &wrong_unit.ast,
                &wrong_unit.definitions,
                &wrong_unit.method,
                block,
            )
            .unwrap_err(),
            JvmCodegenError::BlockResultTypeMismatch {
                tree: block,
                declared: wrong_unit.definitions.int,
                found: JvmExpressionFlow::Unit,
            }
        );

        let mut invalid_literal = Fixture::new(&[]);
        let unit = invalid_literal.literal(Constant::Unit, invalid_literal.definitions.int);
        let block = invalid_literal.block(Vec::new(), unit, invalid_literal.definitions.int);
        assert_eq!(
            lower_expression(
                &invalid_literal.store,
                &invalid_literal.ast,
                &invalid_literal.definitions,
                &invalid_literal.method,
                block,
            )
            .unwrap_err(),
            JvmCodegenError::UnsupportedTypedExpression {
                tree: unit,
                node: "Unit literal with a non-Unit type",
            }
        );
    }

    #[test]
    fn block_result_accepts_a_validated_parameter_term_reference() {
        let mut fixture = Fixture::new(&[JvmType::Int]);
        let parameter = fixture.parameters[0];
        let term_reference = fixture.store.types.alloc(Type::TermRef {
            prefix: fixture.definitions.no_prefix,
            target: TermRefTarget::Symbol(parameter),
        });
        let discarded = fixture.literal(Constant::Int(1), fixture.definitions.int);
        let result = fixture.ident(term_reference);
        let block = fixture.block(vec![discarded], result, term_reference);

        assert_eq!(
            method_body_bytes(
                &fixture,
                lower_method_body(
                    &fixture.store,
                    &fixture.ast,
                    &fixture.definitions,
                    &fixture.method,
                    block,
                )
                .unwrap(),
            ),
            vec![0x04, 0x57, 0x1b, 0xac]
        );
    }

    #[test]
    fn block_stops_after_a_diverging_stat_and_emits_no_unreachable_suffix() {
        let mut fixture = Fixture::new(&[]);
        let value = fixture.literal(Constant::Int(4), fixture.definitions.int);
        let returned = fixture.return_expr(Some(value), None);
        let condition = fixture.literal(Constant::Boolean(true), fixture.definitions.boolean);
        let unreachable = fixture.ast.alloc(Tree {
            kind: TreeKind::While(dotty_core::ast::While {
                cond: condition,
                body: value,
            }),
            position: None,
            ty: fixture.definitions.unit,
        });
        let block = fixture.block(vec![returned], unreachable, fixture.definitions.int);

        assert_eq!(
            method_body_bytes(
                &fixture,
                lower_method_body(
                    &fixture.store,
                    &fixture.ast,
                    &fixture.definitions,
                    &fixture.method,
                    block,
                )
                .unwrap(),
            ),
            vec![0x07, 0xac]
        );
    }

    #[test]
    fn nested_value_typed_block_preserves_a_diverging_final_flow() {
        let mut fixture = Fixture::new(&[]);
        let value = fixture.literal(Constant::Int(4), fixture.definitions.int);
        let returned = fixture.return_expr(Some(value), None);
        let unreachable = fixture.literal(Constant::Int(5), fixture.definitions.int);
        let inner = fixture.block(vec![returned], unreachable, fixture.definitions.int);
        let outer = fixture.block(Vec::new(), inner, fixture.definitions.int);

        assert_eq!(
            method_body_bytes(
                &fixture,
                lower_method_body(
                    &fixture.store,
                    &fixture.ast,
                    &fixture.definitions,
                    &fixture.method,
                    outer,
                )
                .unwrap(),
            ),
            vec![0x07, 0xac]
        );
    }

    #[test]
    fn block_final_return_and_early_return_if_have_no_unreachable_epilogue() {
        let mut fixture = Fixture::new(&[]);
        let value = fixture.literal(Constant::Int(4), fixture.definitions.int);
        let returned = fixture.return_expr(Some(value), None);
        let block = fixture.block(Vec::new(), returned, fixture.definitions.nothing_type);
        assert_eq!(
            method_body_bytes(
                &fixture,
                lower_method_body(
                    &fixture.store,
                    &fixture.ast,
                    &fixture.definitions,
                    &fixture.method,
                    block,
                )
                .unwrap(),
            ),
            vec![0x07, 0xac]
        );

        let mut conditional = Fixture::new(&[JvmType::Boolean, JvmType::Int]);
        let condition_type = conditional.store.types.alloc(Type::TermRef {
            prefix: conditional.definitions.no_prefix,
            target: TermRefTarget::Symbol(conditional.parameters[0]),
        });
        let result_type = conditional.store.types.alloc(Type::TermRef {
            prefix: conditional.definitions.no_prefix,
            target: TermRefTarget::Symbol(conditional.parameters[1]),
        });
        let condition = conditional.ident(condition_type);
        let result = conditional.ident(result_type);
        let return_value = conditional.literal(Constant::Int(4), conditional.definitions.int);
        let return_then = conditional.return_expr(Some(return_value), None);
        let if_expr =
            conditional.if_expr(condition, return_then, result, conditional.definitions.int);
        let block = conditional.block(Vec::new(), if_expr, conditional.definitions.int);
        assert_eq!(
            method_body_bytes(
                &conditional,
                lower_method_body(
                    &conditional.store,
                    &conditional.ast,
                    &conditional.definitions,
                    &conditional.method,
                    block,
                )
                .unwrap(),
            ),
            vec![0x1b, 0x99, 0x00, 0x05, 0x07, 0xac, 0x1c, 0xac]
        );
    }

    #[test]
    fn block_nesting_obeys_the_existing_expression_depth_limit() {
        let mut fixture = Fixture::new(&[]);
        let leaf = fixture.literal(Constant::Int(1), fixture.definitions.int);
        let mut nested = leaf;
        for _ in 0..JVM_EXPRESSION_DEPTH_LIMIT {
            nested = fixture.block(Vec::new(), nested, fixture.definitions.int);
        }

        assert_eq!(
            lower_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                nested,
            )
            .unwrap_err(),
            JvmCodegenError::ExpressionDepthExceeded {
                tree: leaf,
                position: None,
                limit: JVM_EXPRESSION_DEPTH_LIMIT,
            }
        );
    }

    #[test]
    fn block_rejects_unhandled_statements_and_missing_children() {
        let mut fixture = Fixture::new(&[]);
        let value = fixture.literal(Constant::Int(1), fixture.definitions.int);
        let unsupported = [
            (
                fixture.ast.alloc(Tree {
                    kind: TreeKind::ValDef(dotty_core::ast::ValDef {
                        name: dotty_core::names::TermName::new(fixture.store.names.intern("local")),
                        tpt: value,
                        rhs: Some(value),
                        metadata: (),
                    }),
                    position: None,
                    ty: fixture.definitions.int,
                }),
                "ValDef",
            ),
            (
                fixture.ast.alloc(Tree {
                    kind: TreeKind::Assign(dotty_core::ast::Assign {
                        lhs: value,
                        rhs: value,
                    }),
                    position: None,
                    ty: fixture.definitions.unit,
                }),
                "Assign",
            ),
        ];
        for (stat, node) in unsupported {
            let block = fixture.block(vec![stat], value, fixture.definitions.int);
            let expected = if node == "ValDef" {
                JvmCodegenError::LocalDeclarationNotInPlan {
                    method: fixture.method.symbol(),
                    declaration: stat,
                }
            } else if node == "Assign" {
                JvmCodegenError::InvalidAssignmentTarget {
                    tree: stat,
                    lhs: value,
                }
            } else {
                JvmCodegenError::UnsupportedTypedExpression { tree: stat, node }
            };
            assert_eq!(
                lower_expression(
                    &fixture.store,
                    &fixture.ast,
                    &fixture.definitions,
                    &fixture.method,
                    block,
                )
                .unwrap_err(),
                expected
            );
        }

        let mut foreign_ast = TypedAst::new();
        let missing = (0..100)
            .map(|_| {
                foreign_ast.alloc(Tree {
                    kind: TreeKind::Literal(dotty_core::ast::Literal {
                        value: Constant::Int(9),
                    }),
                    position: None,
                    ty: fixture.definitions.int,
                })
            })
            .last()
            .unwrap();
        let block = fixture.block(vec![missing], value, fixture.definitions.int);
        assert_eq!(
            lower_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                block,
            )
            .unwrap_err(),
            JvmCodegenError::UnsupportedTypedExpression {
                tree: missing,
                node: "missing tree",
            }
        );
    }

    #[test]
    fn unit_if_branches_join_with_an_empty_operand_stack() {
        let mut fixture = Fixture::new(&[JvmType::Boolean]);
        fixture.set_result(JvmReturnType::Void);
        let parameter = fixture.parameters[0];
        let condition_type = fixture.store.types.alloc(Type::TermRef {
            prefix: fixture.definitions.no_prefix,
            target: TermRefTarget::Symbol(parameter),
        });
        let condition = fixture.ident(condition_type);
        let then_unit = fixture.literal(Constant::Unit, fixture.definitions.unit);
        let else_unit = fixture.literal(Constant::Unit, fixture.definitions.unit);
        let root = fixture.if_expr(condition, then_unit, else_unit, fixture.definitions.unit);

        assert_eq!(
            lower_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                root,
            )
            .unwrap()
            .flow(),
            JvmExpressionFlow::Unit
        );
        assert_eq!(
            method_body_bytes(
                &fixture,
                lower_method_body(
                    &fixture.store,
                    &fixture.ast,
                    &fixture.definitions,
                    &fixture.method,
                    root,
                )
                .unwrap(),
            ),
            vec![0x1b, 0x99, 0x00, 0x06, 0xa7, 0x00, 0x03, 0xb1]
        );
    }

    #[test]
    fn explicit_void_return_consumes_unit_literal_or_if_flow() {
        let mut fixture = Fixture::new(&[JvmType::Boolean]);
        fixture.set_result(JvmReturnType::Void);
        let unit_literal = fixture.literal(Constant::Unit, fixture.definitions.unit);
        let return_unit = fixture.return_expr(Some(unit_literal), None);
        assert_eq!(
            method_body_bytes(
                &fixture,
                lower_method_body(
                    &fixture.store,
                    &fixture.ast,
                    &fixture.definitions,
                    &fixture.method,
                    return_unit,
                )
                .unwrap(),
            ),
            vec![0xb1]
        );

        let condition_type = fixture.store.types.alloc(Type::TermRef {
            prefix: fixture.definitions.no_prefix,
            target: TermRefTarget::Symbol(fixture.parameters[0]),
        });
        let condition = fixture.ident(condition_type);
        let then_unit = fixture.literal(Constant::Unit, fixture.definitions.unit);
        let else_unit = fixture.literal(Constant::Unit, fixture.definitions.unit);
        let unit_if = fixture.if_expr(condition, then_unit, else_unit, fixture.definitions.unit);
        let return_unit_if = fixture.return_expr(Some(unit_if), None);
        assert_eq!(
            method_body_bytes(
                &fixture,
                lower_method_body(
                    &fixture.store,
                    &fixture.ast,
                    &fixture.definitions,
                    &fixture.method,
                    return_unit_if,
                )
                .unwrap(),
            ),
            vec![0x1b, 0x99, 0x00, 0x06, 0xa7, 0x00, 0x03, 0xb1]
        );
    }

    #[test]
    fn unit_typed_if_with_only_returns_still_diverges_without_an_epilogue() {
        let mut fixture = Fixture::new(&[JvmType::Boolean, JvmType::Int, JvmType::Int]);
        let ids = fixture.parameters.clone();
        let references = ids
            .iter()
            .copied()
            .map(|symbol| {
                let ty = fixture.store.types.alloc(Type::TermRef {
                    prefix: fixture.definitions.no_prefix,
                    target: TermRefTarget::Symbol(symbol),
                });
                fixture.ident(ty)
            })
            .collect::<Vec<_>>();
        let then_return = fixture.return_expr(Some(references[1]), None);
        let else_return = fixture.return_expr(Some(references[2]), None);
        let root = fixture.if_expr(
            references[0],
            then_return,
            else_return,
            fixture.definitions.unit,
        );

        assert_eq!(
            lower_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                root,
            )
            .unwrap()
            .flow(),
            JvmExpressionFlow::Diverges
        );
        assert_eq!(
            method_body_bytes(
                &fixture,
                lower_method_body(
                    &fixture.store,
                    &fixture.ast,
                    &fixture.definitions,
                    &fixture.method,
                    root,
                )
                .unwrap(),
            ),
            vec![0x1b, 0x99, 0x00, 0x05, 0x1c, 0xac, 0x1d, 0xac]
        );
    }

    #[test]
    fn unit_typed_diverging_if_is_valid_as_a_nested_unit_branch() {
        let mut fixture = Fixture::new(&[JvmType::Boolean, JvmType::Boolean]);
        fixture.set_result(JvmReturnType::Void);
        let parameters = fixture.parameters.clone();
        let references = parameters
            .into_iter()
            .map(|symbol| {
                let ty = fixture.store.types.alloc(Type::TermRef {
                    prefix: fixture.definitions.no_prefix,
                    target: TermRefTarget::Symbol(symbol),
                });
                fixture.ident(ty)
            })
            .collect::<Vec<_>>();
        let inner_then_return = fixture.return_expr(None, None);
        let inner_else_return = fixture.return_expr(None, None);
        let inner = fixture.if_expr(
            references[1],
            inner_then_return,
            inner_else_return,
            fixture.definitions.unit,
        );
        let outer_unit = fixture.literal(Constant::Unit, fixture.definitions.unit);
        let root = fixture.if_expr(references[0], inner, outer_unit, fixture.definitions.unit);

        assert_eq!(
            lower_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                root,
            )
            .unwrap()
            .flow(),
            JvmExpressionFlow::Unit
        );
        assert_eq!(
            method_body_bytes(
                &fixture,
                lower_method_body(
                    &fixture.store,
                    &fixture.ast,
                    &fixture.definitions,
                    &fixture.method,
                    root,
                )
                .unwrap(),
            ),
            vec![
                0x1b, 0x99, 0x00, 0x09, 0x1c, 0x99, 0x00, 0x04, 0xb1, 0xb1, 0xb1
            ]
        );
    }

    #[test]
    fn lowers_if_with_an_early_return_without_dead_join_code() {
        let mut fixture = Fixture::new(&[JvmType::Boolean, JvmType::Int, JvmType::Int]);
        let ids = fixture.parameters.clone();
        let refs = ids
            .iter()
            .copied()
            .map(|symbol| {
                let ty = fixture.store.types.alloc(Type::TermRef {
                    prefix: fixture.definitions.no_prefix,
                    target: TermRefTarget::Symbol(symbol),
                });
                fixture.ident(ty)
            })
            .collect::<Vec<_>>();
        let return_value = fixture.return_expr(Some(refs[1]), None);
        let root = fixture.if_expr(refs[0], return_value, refs[2], fixture.definitions.int);
        let code = lower_method_body(
            &fixture.store,
            &fixture.ast,
            &fixture.definitions,
            &fixture.method,
            root,
        )
        .unwrap();
        assert_eq!(
            encoded_method_code(&fixture, code),
            vec![0x1b, 0x99, 0x00, 0x05, 0x1c, 0xac, 0x1d, 0xac]
        );
    }

    #[test]
    fn lowers_nested_conditions_and_two_explicit_returns_without_epilogue() {
        let mut fixture = Fixture::new(&[JvmType::Boolean, JvmType::Int, JvmType::Int]);
        let ids = fixture.parameters.clone();
        let refs = ids
            .iter()
            .copied()
            .map(|symbol| {
                let ty = fixture.store.types.alloc(Type::TermRef {
                    prefix: fixture.definitions.no_prefix,
                    target: TermRefTarget::Symbol(symbol),
                });
                fixture.ident(ty)
            })
            .collect::<Vec<_>>();
        let return_x = fixture.return_expr(Some(refs[1]), None);
        let return_y = fixture.return_expr(Some(refs[2]), None);
        let inner = fixture.if_expr(
            refs[0],
            return_x,
            return_y,
            fixture.definitions.nothing_type,
        );
        let true_condition = fixture.literal(Constant::Boolean(true), fixture.definitions.boolean);
        let outer_return = fixture.return_expr(Some(refs[1]), None);
        let root = fixture.if_expr(
            true_condition,
            inner,
            outer_return,
            fixture.definitions.nothing_type,
        );
        let lowered = lower_expression(
            &fixture.store,
            &fixture.ast,
            &fixture.definitions,
            &fixture.method,
            root,
        )
        .unwrap();
        assert_eq!(lowered.flow(), JvmExpressionFlow::Diverges);
        assert_eq!(
            encoded_method_code(&fixture, lowered.into_code()),
            vec![
                0x04, 0x99, 0x00, 0x0b, 0x1b, 0x99, 0x00, 0x05, 0x1c, 0xac, 0x1d, 0xac, 0x1c, 0xac
            ]
        );
    }

    #[test]
    fn rejects_unsupported_control_flow_and_type_mismatches() {
        let mut fixture = Fixture::new(&[JvmType::Int]);
        let int_ref = fixture.store.types.alloc(Type::TermRef {
            prefix: fixture.definitions.no_prefix,
            target: TermRefTarget::Symbol(fixture.parameters[0]),
        });
        let int_ident = fixture.ident(int_ref);
        let bool_literal = fixture.literal(Constant::Boolean(true), fixture.definitions.boolean);
        let int_literal = fixture.literal(Constant::Int(3), fixture.definitions.int);
        let invalid_condition =
            fixture.if_expr(int_ident, int_literal, int_literal, fixture.definitions.int);
        assert_eq!(
            lower_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                invalid_condition,
            )
            .unwrap_err(),
            JvmCodegenError::InvalidIfCondition {
                tree: int_ident,
                found: Some(JvmType::Int),
            }
        );

        let mismatched_branches = fixture.if_expr(
            bool_literal,
            int_literal,
            bool_literal,
            fixture.definitions.int,
        );
        assert_eq!(
            lower_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                mismatched_branches,
            )
            .unwrap_err(),
            JvmCodegenError::IfBranchTypeMismatch {
                tree: mismatched_branches,
                branch: bool_literal,
                expected: JvmType::Int,
                found: Some(JvmType::Boolean),
            }
        );
    }

    #[test]
    fn while_requires_a_boolean_condition_and_unit_result() {
        let mut fixture = Fixture::new(&[]);
        let int_condition = fixture.literal(Constant::Int(1), fixture.definitions.int);
        let unit_condition = fixture.literal(Constant::Unit, fixture.definitions.unit);
        let boolean_condition =
            fixture.literal(Constant::Boolean(false), fixture.definitions.boolean);
        let unit_body = fixture.literal(Constant::Unit, fixture.definitions.unit);

        let invalid_condition = fixture.ast.alloc(Tree {
            kind: TreeKind::While(dotty_core::ast::While {
                cond: int_condition,
                body: unit_body,
            }),
            position: None,
            ty: fixture.definitions.unit,
        });
        assert_eq!(
            lower_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                invalid_condition,
            )
            .unwrap_err(),
            JvmCodegenError::InvalidWhileCondition {
                tree: int_condition,
                found: Some(JvmType::Int),
            }
        );

        let unit_condition_loop = fixture.ast.alloc(Tree {
            kind: TreeKind::While(dotty_core::ast::While {
                cond: unit_condition,
                body: unit_body,
            }),
            position: None,
            ty: fixture.definitions.unit,
        });
        assert_eq!(
            lower_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                unit_condition_loop,
            )
            .unwrap_err(),
            JvmCodegenError::InvalidWhileCondition {
                tree: unit_condition,
                found: None,
            }
        );

        let wrong_result_type = fixture.ast.alloc(Tree {
            kind: TreeKind::While(dotty_core::ast::While {
                cond: boolean_condition,
                body: unit_body,
            }),
            position: None,
            ty: fixture.definitions.int,
        });
        assert_eq!(
            lower_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                wrong_result_type,
            )
            .unwrap_err(),
            JvmCodegenError::WhileResultTypeMismatch {
                tree: wrong_result_type,
                ty: fixture.definitions.int,
            }
        );
    }

    #[test]
    fn rejects_unsupported_bottom_nodes_and_excessive_expression_depth() {
        let mut fixture = Fixture::new(&[]);
        let condition = fixture.literal(Constant::Boolean(true), fixture.definitions.boolean);
        let value = fixture.literal(Constant::Int(1), fixture.definitions.int);
        let unsupported_bottom = fixture.ast.alloc(Tree {
            kind: TreeKind::While(dotty_core::ast::While {
                cond: condition,
                body: value,
            }),
            position: None,
            ty: fixture.definitions.nothing_type,
        });
        let root = fixture.if_expr(
            condition,
            unsupported_bottom,
            value,
            fixture.definitions.int,
        );
        assert_eq!(
            lower_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                root,
            )
            .unwrap_err(),
            JvmCodegenError::WhileResultTypeMismatch {
                tree: unsupported_bottom,
                ty: fixture.definitions.nothing_type,
            }
        );

        let mut nested = value;
        for _ in 0..JVM_EXPRESSION_DEPTH_LIMIT {
            nested = fixture.if_expr(condition, nested, value, fixture.definitions.int);
        }
        assert_eq!(
            lower_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                nested,
            )
            .unwrap_err(),
            JvmCodegenError::ExpressionDepthExceeded {
                tree: condition,
                position: None,
                limit: JVM_EXPRESSION_DEPTH_LIMIT,
            }
        );
    }

    #[test]
    fn shared_typed_tree_dag_obeys_the_total_node_visit_budget() {
        let mut fixture = Fixture::new(&[]);
        let condition = fixture.literal(Constant::Boolean(true), fixture.definitions.boolean);
        let value = fixture.literal(Constant::Int(1), fixture.definitions.int);
        let mut shared = value;
        for _ in 0..12 {
            shared = fixture.if_expr(condition, shared, shared, fixture.definitions.int);
        }

        assert!(matches!(
            lower_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                shared,
            ),
            Err(JvmCodegenError::ExpressionNodeBudgetExceeded {
                limit: JVM_EXPRESSION_NODE_BUDGET,
                ..
            })
        ));
    }

    #[test]
    fn supports_only_local_returns_and_descriptor_compatible_method_results() {
        let mut fixture = Fixture::new(&[]);
        let value = fixture.literal(Constant::Int(4), fixture.definitions.int);
        let return_value = fixture.return_expr(Some(value), None);
        let wrong_target = fixture.literal(Constant::Int(1), fixture.definitions.int);
        let targeted_return = fixture.return_expr(Some(value), Some(wrong_target));
        assert_eq!(
            lower_method_body(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                targeted_return,
            )
            .unwrap_err(),
            JvmCodegenError::UnsupportedReturnTarget {
                tree: targeted_return,
                from: wrong_target,
            }
        );

        fixture.set_result(JvmReturnType::Value(JvmType::Boolean));
        assert_eq!(
            lower_method_body(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                value,
            )
            .unwrap_err(),
            JvmCodegenError::MethodBodyTypeMismatch {
                method: fixture.method.symbol(),
                tree: value,
                expected: JvmReturnType::Value(JvmType::Boolean),
                found: Some(JvmType::Int),
            }
        );
        fixture.set_result(JvmReturnType::Value(JvmType::Int));
        let code = lower_method_body(
            &fixture.store,
            &fixture.ast,
            &fixture.definitions,
            &fixture.method,
            return_value,
        )
        .unwrap();
        assert_eq!(method_body_bytes(&fixture, code), vec![0x07, 0xac]);

        fixture.set_result(JvmReturnType::Void);
        let void_return = fixture.return_expr(None, None);
        let code = lower_method_body(
            &fixture.store,
            &fixture.ast,
            &fixture.definitions,
            &fixture.method,
            void_return,
        )
        .unwrap();
        assert_eq!(method_body_bytes(&fixture, code), vec![0xb1]);
    }
}
