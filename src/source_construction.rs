//! Navigation from a source constructor invocation to its selected primary constructor.
use crate::core::{
    AstArena, SemanticStore, SourceSpan, SourceTypedIndex, SymbolId, TermRefTarget, TreeId,
    TreeKind, Type, TypeRefTarget, Typed, Untyped,
};
use crate::{NamedClassSource, SourceMemberTarget};

/// How an ordinary source primary constructor is invoked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceConstructorUse {
    /// A written allocation in a checked class body.
    Allocation,
    /// Initialization of the immediate superclass in a checked class header.
    DirectParent {
        /// Class whose constructor forwards the arguments.
        child: SymbolId,
    },
}

/// A committed constructor invocation of an indexed ordinary class. Identities and
/// byte ranges belong to the original naming and typing revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedSourceConstruction {
    /// Source context of the invocation.
    pub usage: SourceConstructorUse,
    /// Canonical class declaring the invoked constructor.
    pub class: SymbolId,
    /// Primary constructor selected by the typed application.
    pub constructor: SymbolId,
    /// Written class-name token at the invocation site.
    pub origin: SourceSpan,
    /// Source declaration and name of the constructed class.
    pub target: SourceMemberTarget,
}

/// Arguments of a checked source constructor invocation with one required primary clause.
/// Both declaration and use identities belong to the indexed revision.
#[derive(Debug)]
pub struct ResolvedConstructionArguments {
    /// Invocation site and selected primary constructor.
    pub construction: ResolvedSourceConstruction,
    /// Supplied arguments in source order, paired with constructor-owned formals
    /// and their class-owned source declarations.
    pub written: Box<[crate::ResolvedWrittenArgument<crate::SourceConstructorParameter>]>,
}

/// Checked constructor signature at a cursor in a completed argument list.
/// All references borrow the same retained revision.
#[derive(Debug)]
pub struct ResolvedConstructionSignature<'a> {
    /// Written invocation and canonical identities.
    pub construction: ResolvedSourceConstruction,
    /// Constructed source class, including its semantic name.
    pub instance: &'a crate::SourceNamedType,
    /// Strict formal parameters in declaration order.
    pub parameters: &'a [crate::SourceSignatureParameter],
    /// Source declarations in the same order as `parameters`, with field roles and formal IDs.
    pub declarations: &'a [crate::SourceConstructorParameter],
    /// Formal ordinal of the written operand at the cursor, or none for an empty list.
    pub active_parameter: Option<usize>,
}

impl ResolvedConstructionArguments {
    pub(crate) fn capture(
        construction: ResolvedSourceConstruction,
        file: &NamedClassSource,
        root: TreeId<Untyped>,
        bindings: &SourceTypedIndex,
        formals: &[crate::SourceConstructorParameter],
        store: &SemanticStore,
    ) -> Option<(u32, Self)> {
        let constructor = store.symbols.get(construction.constructor);
        if store
            .symbols
            .required_primary_clause(construction.constructor)
            .is_none()
            || constructor.visibility != crate::core::Visibility::Public
        {
            return None;
        }
        // The closing token belongs to this list even when the last operand is another call.
        let end = file
            .ast()
            .get(root)
            .position?
            .span()
            .range()
            .end()
            .checked_sub(1)?;
        let list = file.named().syntax().call_arguments.at(end)?;
        if list.application != root || !list.regular || list.arguments.len() != formals.len() {
            return None;
        }
        let binding = bindings.named_call(bindings.get(file.source(), root)?);
        let written = crate::source_call_bindings::written_arguments(list, formals, binding)?;
        Some((
            list.range.start(),
            Self {
                construction,
                written,
            },
        ))
    }
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
    let root = mapped(root)?;
    let application = typed.try_get(root)?;
    let application = match &application.kind {
        TreeKind::Block(block)
            if bindings.named_call(root)?.function == mapped(source_call.function)? =>
        {
            typed.try_get(block.expr)?
        }
        _ => application,
    };
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
