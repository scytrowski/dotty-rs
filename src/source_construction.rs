//! Navigation from an explicit source allocation to its selected primary constructor.
use crate::core::{
    AstArena, SemanticStore, SourceSpan, SourceTypedIndex, SymbolId, TermRefTarget, TreeId,
    TreeKind, Type, TypeRefTarget, Typed, Untyped,
};
use crate::{NamedClassSource, SourceMemberTarget};

/// A committed explicit `new` of an indexed ordinary class. Identities and
/// byte ranges belong to the original naming and typing revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedSourceConstruction {
    /// Canonical class of the constructed instance.
    pub class: SymbolId,
    /// Primary constructor selected by the typed application.
    pub constructor: SymbolId,
    /// Written class-name token at the allocation site.
    pub origin: SourceSpan,
    /// Source declaration and name of the constructed class.
    pub target: SourceMemberTarget,
}

pub(crate) fn selected_constructor(
    file: &NamedClassSource,
    root: TreeId<Untyped>,
    bindings: &SourceTypedIndex,
    typed: &AstArena<Typed>,
    store: &SemanticStore,
) -> Option<(SymbolId, SymbolId)> {
    let mapped = |tree| bindings.get(file.source(), tree);
    let TreeKind::Apply(source_call) = &file.ast().try_get(root)?.kind else {
        return None;
    };
    let application = typed.try_get(mapped(root)?)?;
    let TreeKind::Apply(call) = &application.kind else {
        return None;
    };
    if mapped(source_call.function)? != call.function {
        return None;
    }
    let selection = typed.try_get(call.function)?;
    let TreeKind::Select(source_select) = &file.ast().try_get(source_call.function)?.kind else {
        return None;
    };
    let TreeKind::Select(select) = &selection.kind else {
        return None;
    };
    if mapped(source_select.qualifier)? != select.qualifier {
        return None;
    }
    let receiver = typed.try_get(select.qualifier)?;
    if !matches!(receiver.kind, TreeKind::New(_)) {
        return None;
    }
    let class_of = |ty| match store.types.try_get(ty)? {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(id),
            ..
        } => Some(*id),
        _ => None,
    };
    let class = class_of(application.ty)?;
    let Type::TermRef {
        target: TermRefTarget::Symbol(constructor),
        ..
    } = store.types.try_get(selection.ty)?
    else {
        return None;
    };
    (class_of(receiver.ty)? == class).then_some((class, *constructor))
}
