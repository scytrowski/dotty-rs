//! Method and constructor application, overload resolution, and inference.

use super::*;

mod inference;
mod overload;
pub(in crate::typer) use inference::InferenceLocation;

#[derive(Clone, Copy)]
pub(in crate::typer) struct ApplicationCandidate {
    pub(in crate::typer) symbol: SymbolId,
    pub(in crate::typer) callable: TypeId,
    pub(in crate::typer) member: Option<MemberCandidate>,
    pub(in crate::typer) rejection: Option<OverloadRejection>,
}

pub(in crate::typer) struct ResolvedApplicationFunction {
    pub(in crate::typer) typed: TreeId<Typed>,
    pub(in crate::typer) callable: TypeId,
    pub(in crate::typer) arguments: Vec<TypedArgument>,
}

#[derive(Clone, Copy)]
pub(in crate::typer) struct ApplicationRequest<'a> {
    pub(in crate::typer) function_tree: TreeId<Untyped>,
    pub(in crate::typer) argument_trees: &'a [TreeId<Untyped>],
    pub(in crate::typer) application_kind: ApplyKind,
    pub(in crate::typer) context: ExpressionContext,
    pub(in crate::typer) tree_index: u32,
}

#[derive(Clone, Copy)]
pub(in crate::typer) struct TypedArgument {
    pub(in crate::typer) typed: TreeId<Typed>,
    pub(in crate::typer) own_type: TypeId,
    pub(in crate::typer) widened_type: TypeId,
}

pub(in crate::typer) fn overload_arity_rejection(
    method: &MethodType,
    actual: usize,
) -> Option<usize> {
    match method.params.iter().position(|parameter| parameter.varargs) {
        Some(varargs_index) if varargs_index + 1 == method.params.len() => {
            (actual < varargs_index).then_some(varargs_index)
        }
        Some(_) => None,
        None => (actual != method.params.len()).then_some(method.params.len()),
    }
}
