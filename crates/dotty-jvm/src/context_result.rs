//! Integration of a direct unary context-function result into its enclosing JVM method.

use dotty_classfile::descriptor::MethodDescriptor;
use dotty_core::ast::{TreeKind, Typed, TypedAst};
use dotty_core::{
    CheckedUnaryContextClosure, Definitions, MethodKind, SemanticStore, SymbolFlags, SymbolId,
    SymbolInfo, SymbolKind, TermRefTarget, TreeId, Type, checked_unary_context_closure,
};

use crate::{
    JvmCodegenError, JvmIntIntrinsic, JvmMethodDescriptor, JvmMethodPlan, JvmReturnType, JvmType,
    resolve_int_intrinsic,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ContextResult {
    pub root: TreeId<Typed>,
    pub closure: CheckedUnaryContextClosure,
    outer: Option<SymbolId>,
}

impl ContextResult {
    pub(super) fn descriptor(
        self,
        method: SymbolId,
    ) -> Result<JvmMethodDescriptor, JvmCodegenError> {
        let outer_count = usize::from(self.outer.is_some());
        let text = if outer_count == 0 { "(I)I" } else { "(II)I" };
        let descriptor = MethodDescriptor::parse(text)
            .map_err(|error| JvmCodegenError::InvalidGeneratedDescriptor { method, error })?;
        Ok(JvmMethodDescriptor {
            descriptor,
            descriptor_text: text.to_owned(),
            parameters: vec![JvmType::Int; outer_count + 1],
            result: JvmReturnType::Value(JvmType::Int),
        })
    }
    /// The standalone lowering entry point may be passed previously built metadata.
    /// Recheck the closure's links against its actual arena before following the body.
    pub(super) fn standalone_body(
        self,
        store: &SemanticStore,
        ast: &TypedAst,
        method: SymbolId,
        root: TreeId<Typed>,
        defs: &Definitions,
    ) -> Result<TreeId<Typed>, JvmCodegenError> {
        let invalid = JvmCodegenError::InvalidContextFunctionResult { method, tree: root };
        if root != self.root || !store.symbols.contains(method) {
            return Err(invalid);
        }
        let Some(owner) = store.symbols.get(method).owner else {
            return Err(invalid);
        };
        let plan = JvmMethodPlan {
            method,
            parameters: self.outer.into_iter().collect(),
            body: Some(root),
            locals: Vec::new(),
        };
        if project(store, ast, &plan, owner, defs)? != Some(self) {
            return Err(invalid);
        }
        Ok(self.closure.body)
    }
}

pub(super) fn project(
    store: &SemanticStore,
    ast: &TypedAst,
    plan: &JvmMethodPlan,
    owner: SymbolId,
    defs: &Definitions,
) -> Result<Option<ContextResult>, JvmCodegenError> {
    let Some(root) = plan.body else {
        return Ok(None);
    };
    let Some(tree) = ast.try_get(root) else {
        return Ok(None);
    };
    let TreeKind::Closure(literal) = &tree.kind else {
        return Ok(None);
    };
    let invalid = || JvmCodegenError::InvalidContextFunctionResult {
        method: plan.method,
        tree: root,
    };
    if !store.symbols.contains(plan.method) {
        return Err(JvmCodegenError::UnknownMethod {
            method: plan.method,
        });
    }
    let method = store.symbols.get(plan.method);
    let SymbolInfo::Complete(info) = method.info else {
        return Err(invalid());
    };
    let (formals, declared) = match store.types.try_get(info) {
        Some(Type::Method(signature)) if signature.kind == MethodKind::Plain => {
            (signature.params.as_slice(), signature.result)
        }
        Some(Type::Applied { .. }) => (&[][..], info),
        _ => return Ok(None),
    };
    if method.owner != Some(owner) || method.kind != SymbolKind::Method {
        return Err(invalid());
    }
    let outer = match (plan.parameters.as_slice(), formals) {
        ([], []) => None,
        ([id], [formal])
            if !formal.erased
                && !formal.varargs
                && formal.ty == defs.int
                && store.symbols.contains(*id) =>
        {
            let symbol = store.symbols.get(*id);
            if symbol.owner != Some(plan.method)
                || symbol.kind != SymbolKind::Parameter
                || symbol.flags != SymbolFlags::EMPTY
                || symbol.name != *formal.name.as_name()
                || symbol.info != SymbolInfo::Complete(defs.int)
            {
                return Err(invalid());
            }
            Some(*id)
        }
        _ => return Err(invalid()),
    };
    let closure = checked_unary_context_closure(store, defs, ast, plan.method, root, declared)
        .ok_or_else(invalid)?;
    if closure.input != defs.int || closure.result != defs.int {
        return Err(invalid());
    }
    let parameter_reference = |tree| {
        let node = ast.try_get(tree)?;
        let TreeKind::Ident(id) = &node.kind else {
            return None;
        };
        let Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(symbol),
        } = store.types.try_get(node.ty)?
        else {
            return None;
        };
        (*prefix == defs.no_prefix
            && store.symbols.contains(*symbol)
            && id.name == store.symbols.get(*symbol).name)
            .then_some(*symbol)
    };
    let capture = match literal.env.as_slice() {
        [] => None,
        [capture] => {
            let captured = parameter_reference(*capture).ok_or_else(invalid)?;
            if Some(captured) != outer {
                return Err(invalid());
            }
            Some(captured)
        }
        _ => return Err(invalid()),
    };
    let mut uses_capture = false;
    let mut check_operand = |operand| {
        let id = parameter_reference(operand).ok_or_else(invalid)?;
        if id != closure.parameter {
            if Some(id) != capture {
                return Err(invalid());
            }
            uses_capture = true;
        }
        Ok(())
    };
    let body = ast.get(closure.body);
    match &body.kind {
        TreeKind::Ident(_) => check_operand(closure.body)?,
        TreeKind::Apply(app) => {
            let intrinsic =
                resolve_int_intrinsic(store, ast, closure.body, defs).map_err(|error| {
                    JvmCodegenError::IntIntrinsicResolution {
                        tree: closure.body,
                        error,
                    }
                })?;
            if intrinsic != Some(JvmIntIntrinsic::IntAdd) {
                return Err(invalid());
            }
            let [arg] = app.args.as_slice() else {
                return Err(invalid());
            };
            let TreeKind::Select(sel) = &ast.get(app.function).kind else {
                return Err(invalid());
            };
            check_operand(sel.qualifier)?;
            check_operand(*arg)?;
        }
        _ => return Err(invalid()),
    }
    if uses_capture != capture.is_some() {
        return Err(invalid());
    }
    Ok(Some(ContextResult {
        root,
        closure,
        outer,
    }))
}
