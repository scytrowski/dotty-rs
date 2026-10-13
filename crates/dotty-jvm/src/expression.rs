//! Lowering for the first, value-producing typed expression leaves.

use dotty_classfile::bytecode::CodeBuilder;
use dotty_core::ast::{TreeKind, Typed, TypedAst};
use dotty_core::definitions::Definitions;
use dotty_core::ids::{SymbolId, TreeId, TypeId};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolInfo;
use dotty_core::types::{Constant, TermRefTarget, Type, TypeRefTarget};

use crate::{
    InvalidTypeReason, JVM_EXPRESSION_DEPTH_LIMIT, JvmCodegenError, JvmExpressionFlow,
    JvmLoweredExpression, JvmMethodMetadata, JvmReturnType, JvmType, JvmValueCategory,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Value(JvmType),
    Diverges,
}

pub(super) fn lower_value_expression(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    definitions: &Definitions,
    method: &JvmMethodMetadata,
    tree_id: TreeId<Typed>,
    code: &mut CodeBuilder,
) -> Result<JvmValueCategory, JvmCodegenError> {
    let tree = typed_ast
        .try_get(tree_id)
        .ok_or(JvmCodegenError::UnsupportedTypedExpression {
            tree: tree_id,
            node: "missing tree",
        })?;
    validate_tree_type(store, tree_id, tree.ty)?;

    lower_leaf(store, typed_ast, definitions, method, tree_id, code)?;
    Ok(JvmValueCategory::IntLike)
}

pub(super) fn lower_expression(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    definitions: &Definitions,
    method: &JvmMethodMetadata,
    tree: TreeId<Typed>,
) -> Result<JvmLoweredExpression, JvmCodegenError> {
    let mut code = CodeBuilder::new();
    let flow = emit_expression(store, typed_ast, definitions, method, tree, &mut code, 0)?;
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
    let flow = emit_expression(store, typed_ast, definitions, method, body, &mut code, 0)?;
    match (method.descriptor().result(), flow) {
        (JvmReturnType::Value(expected), Flow::Value(found)) if expected == found => {
            code.ireturn()
                .map_err(|error| JvmCodegenError::CodeBuilder { tree: body, error })?;
        }
        (JvmReturnType::Value(_), Flow::Diverges) | (JvmReturnType::Void, Flow::Diverges) => {}
        (expected, found) => {
            return Err(JvmCodegenError::MethodBodyTypeMismatch {
                method: method.symbol(),
                tree: body,
                expected,
                found: match found {
                    Flow::Value(value_type) => Some(value_type),
                    Flow::Diverges => None,
                },
            });
        }
    }
    Ok(code)
}

fn lower_leaf(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    definitions: &Definitions,
    method: &JvmMethodMetadata,
    tree_id: TreeId<Typed>,
    code: &mut CodeBuilder,
) -> Result<JvmType, JvmCodegenError> {
    let tree = typed_ast
        .try_get(tree_id)
        .ok_or(JvmCodegenError::UnsupportedTypedExpression {
            tree: tree_id,
            node: "missing tree",
        })?;
    match &tree.kind {
        TreeKind::Literal(literal) => {
            let (value, primitive) = match literal.value {
                Constant::Int(value) => (value, JvmType::Int),
                Constant::Boolean(value) => (i32::from(value), JvmType::Boolean),
                _ => {
                    return Err(JvmCodegenError::UnsupportedTypedExpression {
                        tree: tree_id,
                        node: "non-Int/Boolean literal",
                    });
                }
            };
            let typed_primitive = primitive_type(store, tree.ty, definitions);
            let matches_constant_type = match store.types.try_get(tree.ty) {
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
            Ok(primitive)
        }
        TreeKind::Ident(_) => {
            let symbol = match store.types.try_get(tree.ty) {
                Some(Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                }) => Some(*symbol),
                _ => None,
            };
            let Some(parameter) = symbol.and_then(|symbol| {
                method
                    .parameters()
                    .iter()
                    .find(|parameter| parameter.symbol() == symbol)
                    .copied()
            }) else {
                return Err(JvmCodegenError::InvalidParameterReference {
                    tree: tree_id,
                    symbol,
                });
            };
            if !store.symbols.contains(parameter.symbol()) {
                return Err(JvmCodegenError::InvalidParameterReference {
                    tree: tree_id,
                    symbol: Some(parameter.symbol()),
                });
            }
            let parameter_symbol = store.symbols.get(parameter.symbol());
            if parameter_symbol.kind != dotty_core::symbols::SymbolKind::Parameter
                || parameter_symbol.owner != Some(method.symbol())
            {
                return Err(JvmCodegenError::InvalidParameterReference {
                    tree: tree_id,
                    symbol: Some(parameter.symbol()),
                });
            }
            let symbol_type = match store.symbols.info(parameter.symbol()) {
                SymbolInfo::Complete(ty) => Some(*ty),
                SymbolInfo::Missing | SymbolInfo::Deferred(_) | SymbolInfo::Error => None,
            };
            let found = symbol_type.and_then(|ty| primitive_type(store, ty, definitions));
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
            Ok(parameter.ty())
        }
        kind => Err(JvmCodegenError::UnsupportedTypedExpression {
            tree: tree_id,
            node: tree_kind_name(kind),
        }),
    }
}

fn emit_expression(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    definitions: &Definitions,
    method: &JvmMethodMetadata,
    tree_id: TreeId<Typed>,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    let tree = typed_ast
        .try_get(tree_id)
        .ok_or(JvmCodegenError::UnsupportedTypedExpression {
            tree: tree_id,
            node: "missing tree",
        })?;
    if depth >= JVM_EXPRESSION_DEPTH_LIMIT {
        return Err(JvmCodegenError::ExpressionDepthExceeded {
            tree: tree_id,
            position: tree.position,
            limit: JVM_EXPRESSION_DEPTH_LIMIT,
        });
    }
    validate_tree_type(store, tree_id, tree.ty)?;
    match &tree.kind {
        TreeKind::Literal(_) | TreeKind::Ident(_) => Ok(Flow::Value(lower_leaf(
            store,
            typed_ast,
            definitions,
            method,
            tree_id,
            code,
        )?)),
        TreeKind::If(if_tree) => emit_if(
            store,
            typed_ast,
            definitions,
            method,
            tree_id,
            tree.ty,
            if_tree.cond,
            if_tree.then_branch,
            if_tree.else_branch,
            code,
            depth,
        ),
        TreeKind::Return(return_tree) => emit_return(
            store,
            typed_ast,
            definitions,
            method,
            tree_id,
            tree.ty,
            return_tree.expr,
            return_tree.from,
            code,
            depth,
        ),
        kind => Err(JvmCodegenError::UnsupportedTypedExpression {
            tree: tree_id,
            node: tree_kind_name(kind),
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_if(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    definitions: &Definitions,
    method: &JvmMethodMetadata,
    tree_id: TreeId<Typed>,
    result_ty: TypeId,
    condition: TreeId<Typed>,
    then_branch: TreeId<Typed>,
    else_branch: TreeId<Typed>,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    let condition_flow = emit_expression(
        store,
        typed_ast,
        definitions,
        method,
        condition,
        code,
        depth + 1,
    )?;
    if condition_flow != Flow::Value(JvmType::Boolean) {
        return Err(JvmCodegenError::InvalidIfCondition {
            tree: condition,
            found: match condition_flow {
                Flow::Value(value_type) => Some(value_type),
                Flow::Diverges => None,
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
    let then_flow = emit_expression(
        store,
        typed_ast,
        definitions,
        method,
        then_branch,
        code,
        depth + 1,
    )?;
    let join_label = if matches!(then_flow, Flow::Value(_)) {
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
    let else_flow = emit_expression(
        store,
        typed_ast,
        definitions,
        method,
        else_branch,
        code,
        depth + 1,
    )?;
    let flow = merge_if_flows(
        store,
        typed_ast,
        definitions,
        tree_id,
        result_ty,
        then_branch,
        then_flow,
        else_branch,
        else_flow,
    )?;
    if let Some(join_label) = join_label {
        code.mark(join_label)
            .map_err(|error| JvmCodegenError::CodeBuilder {
                tree: tree_id,
                error,
            })?;
    }
    Ok(flow)
}

#[allow(clippy::too_many_arguments)]
fn merge_if_flows(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    definitions: &Definitions,
    tree: TreeId<Typed>,
    result_ty: TypeId,
    then_tree: TreeId<Typed>,
    then_flow: Flow,
    else_tree: TreeId<Typed>,
    else_flow: Flow,
) -> Result<Flow, JvmCodegenError> {
    validate_bottom_branch(store, typed_ast, definitions, then_tree, then_flow)?;
    validate_bottom_branch(store, typed_ast, definitions, else_tree, else_flow)?;
    let branch_value = match (then_flow, else_flow) {
        (Flow::Value(left), Flow::Value(right)) => {
            if left != right {
                return Err(JvmCodegenError::IfBranchTypeMismatch {
                    tree,
                    branch: else_tree,
                    expected: left,
                    found: Some(right),
                });
            }
            Some(left)
        }
        (Flow::Value(value), Flow::Diverges) | (Flow::Diverges, Flow::Value(value)) => Some(value),
        (Flow::Diverges, Flow::Diverges) => None,
    };
    let expected = primitive_type(store, result_ty, definitions);
    match branch_value {
        Some(found) if expected == Some(found) => Ok(Flow::Value(found)),
        None if is_nothing_type(store, result_ty, definitions) => Ok(Flow::Diverges),
        found => Err(JvmCodegenError::IfResultTypeMismatch {
            tree,
            expected,
            found,
        }),
    }
}

fn validate_bottom_branch(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    definitions: &Definitions,
    tree: TreeId<Typed>,
    flow: Flow,
) -> Result<(), JvmCodegenError> {
    let tree_ty = typed_ast.try_get(tree).map(|tree| tree.ty).ok_or(
        JvmCodegenError::UnsupportedTypedExpression {
            tree,
            node: "missing tree",
        },
    )?;
    match flow {
        Flow::Diverges if is_nothing_type(store, tree_ty, definitions) => Ok(()),
        Flow::Diverges => Err(JvmCodegenError::UnsupportedBottomFlow { tree }),
        Flow::Value(_) if is_nothing_type(store, tree_ty, definitions) => {
            Err(JvmCodegenError::UnsupportedBottomFlow { tree })
        }
        Flow::Value(_) => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_return(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    definitions: &Definitions,
    method: &JvmMethodMetadata,
    tree_id: TreeId<Typed>,
    return_ty: TypeId,
    expr: Option<TreeId<Typed>>,
    from: Option<TreeId<Typed>>,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    if let Some(from) = from {
        return Err(JvmCodegenError::UnsupportedReturnTarget {
            tree: tree_id,
            from,
        });
    }
    if !is_nothing_type(store, return_ty, definitions) {
        return Err(JvmCodegenError::UnsupportedBottomFlow { tree: tree_id });
    }
    match (expr, method.descriptor().result()) {
        (Some(value), JvmReturnType::Value(expected)) => {
            match emit_expression(
                store,
                typed_ast,
                definitions,
                method,
                value,
                code,
                depth + 1,
            )? {
                Flow::Value(found) if found == expected => {
                    code.ireturn()
                        .map_err(|error| JvmCodegenError::CodeBuilder {
                            tree: tree_id,
                            error,
                        })?;
                    Ok(Flow::Diverges)
                }
                Flow::Diverges => Ok(Flow::Diverges),
                Flow::Value(found) => Err(JvmCodegenError::ReturnValueTypeMismatch {
                    tree: tree_id,
                    expected: method.descriptor().result(),
                    found: Some(found),
                }),
            }
        }
        (Some(_), expected) => Err(JvmCodegenError::ReturnValueTypeMismatch {
            tree: tree_id,
            expected,
            found: None,
        }),
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
        Flow::Diverges => JvmExpressionFlow::Diverges,
    }
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

fn validate_tree_type(
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
            JvmCodegenError::UnsupportedTypedExpression {
                tree: unsupported_bottom,
                node: "While",
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
