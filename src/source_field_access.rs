//! Checked reads and assignments of explicitly selected public source fields.
use crate::core::{
    AstArena, SemanticStore, SourceTypedIndex, SymbolId, SymbolKind, SymbolOrigin, TermRefTarget,
    TreeKind, Type, Typed, Untyped, Visibility,
};
use crate::parser::SourceSelection;

/// Operation performed on a selected source field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldAccessKind {
    /// The checked expression reads the field value.
    Read,
    /// The field selection is the target of a checked direct assignment.
    Write,
}

/// Returns the canonical public field and its access kind for an original
/// parser selection. Supply committed bindings from its owning class, both
/// original arenas and their store. Missing bindings, incompatible tree
/// shapes and non-source targets yield `None`.
pub fn selected_field_access(
    selection: SourceSelection,
    ast: &AstArena<Untyped>,
    bindings: &SourceTypedIndex,
    typed: &AstArena<Typed>,
    store: &SemanticStore,
) -> Option<(SymbolId, FieldAccessKind)> {
    let node = ast.try_get(selection.tree)?;
    let source = node.position?.source();
    let TreeKind::Select(written) = &node.kind else {
        return None;
    };
    let id = bindings.get(source, selection.tree)?;
    let node = typed.try_get(id)?;
    let TreeKind::Select(resolved) = &node.kind else {
        return None;
    };
    if bindings.get(source, written.qualifier)? != resolved.qualifier {
        return None;
    }
    let Type::TermRef {
        target: TermRefTarget::Symbol(field),
        ..
    } = store.types.try_get(node.ty)?
    else {
        return None;
    };
    if !store.symbols.contains(*field) {
        return None;
    }
    let symbol = store.symbols.get(*field);
    if symbol.kind != SymbolKind::Field
        || symbol.visibility != Visibility::Public
        || !matches!(symbol.origin, SymbolOrigin::Source(_))
    {
        return None;
    }
    let kind = if let Some(assign) = selection.assignment {
        let TreeKind::Assign(written) = &ast.try_get(assign)?.kind else {
            return None;
        };
        let TreeKind::Assign(resolved) = &typed.try_get(bindings.get(source, assign)?)?.kind else {
            return None;
        };
        if written.lhs != selection.tree || resolved.lhs != id {
            return None;
        }
        FieldAccessKind::Write
    } else {
        FieldAccessKind::Read
    };
    Some((*field, kind))
}
