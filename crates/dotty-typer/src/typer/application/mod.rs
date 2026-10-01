//! Method and constructor application, overload resolution, and inference.

use super::*;

pub(super) mod constructors;
mod inference;
pub(in crate::typer) mod overload;
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
