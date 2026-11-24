//! Checked reads and assignments of written source fields.
use crate::core::ast::{Modifier, VisibilitySyntax};
use crate::core::{
    AstArena, SemanticStore, SourceTypedIndex, SymbolId, SymbolKind, SymbolOrigin, TermRefTarget,
    TreeKind, Type, Typed, Untyped, Visibility,
};
use crate::core::{SourceDefinition, SourceId, SourceSpan, Span};
use crate::parser::{SourceSelection, SourceTermName};
use crate::source_declarations::DeclarationError;
use crate::{
    CheckedClassBodies, NamedClassSource, NamedClassSources, SourceFileCalls, SourceMemberTarget,
};

/// Operation performed on a source field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldAccessKind {
    /// The checked expression reads the field value.
    Read,
    /// The field name is the target of a checked direct assignment.
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
    field_access(selection, None, ast, bindings, typed, store)
}

fn field_access(
    selection: SourceTermName,
    enclosing_class: Option<SymbolId>,
    ast: &AstArena<Untyped>,
    bindings: &SourceTypedIndex,
    typed: &AstArena<Typed>,
    store: &SemanticStore,
) -> Option<(SymbolId, FieldAccessKind)> {
    let written = ast.try_get(selection.tree)?;
    let source = written.position?.source();
    let id = bindings.get(source, selection.tree)?;
    let node = typed.try_get(id)?;
    let own_receiver = match (&written.kind, &node.kind) {
        (TreeKind::Select(w), TreeKind::Select(r))
            if bindings.get(source, w.qualifier) == Some(r.qualifier) =>
        {
            matches!(&ast.try_get(w.qualifier)?.kind, TreeKind::This(t) if t.qual.is_none())
        }
        (TreeKind::Ident(_), TreeKind::Ident(_)) if enclosing_class.is_some() => true,
        _ => return None,
    };
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
    let own_field = own_receiver && enclosing_class.is_some() && symbol.owner == enclosing_class;
    if symbol.kind != SymbolKind::Field
        || (matches!(written.kind, TreeKind::Ident(_)) && !own_field)
        || !(symbol.visibility == Visibility::Public
            || own_field && symbol.visibility == Visibility::Private)
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

/// A committed use of a field declared in the selected sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedFieldAccess {
    /// Written name at the usage site, in UTF-8 bytes.
    pub origin: SourceSpan,
    /// Canonical field of the defining class, including constructor accessors.
    pub field: SymbolId,
    /// Read or direct assignment seen by the typer.
    pub kind: FieldAccessKind,
    /// Declaration and name token in the defining source.
    pub target: SourceMemberTarget,
}

impl NamedClassSources<'_> {
    /// Resolves a byte within a public field selection or a bare use of the enclosing
    /// class’s field. An ordinary private body field is supported when written
    /// as a bare name or selected on unqualified `this` inside its declaring class.
    /// Use committed bodies, typed arena and store from the same revision.
    /// Missing files, uncommitted uses and unsupported targets yield `Ok(None)`.
    /// Caller or provider navigation failures preserve their typed error.
    pub fn resolved_field_at(
        &self,
        source: SourceId,
        offset: u32,
        bodies: &CheckedClassBodies,
        typed: &AstArena<Typed>,
        store: &SemanticStore,
    ) -> Result<Option<ResolvedFieldAccess>, DeclarationError> {
        let Some(file) = self.file(source) else {
            return Ok(None);
        };
        let navigation = file.navigation()?;
        let names = &file.named().syntax().member_names;
        let Some(selection) = names
            .selection_at(offset)
            .or_else(|| names.identifier_at(offset))
        else {
            return Ok(None);
        };
        let Some((field, kind)) = field_binding(file, navigation, selection, bodies, typed, store)
        else {
            return Ok(None);
        };
        let Some((provider, decl)) = self.declaration(field) else {
            return Ok(None);
        };
        let target = field_target(provider, provider.navigation()?, decl.class, field, store)?;
        Ok(target.map(|target| ResolvedFieldAccess {
            origin: SourceSpan::new(file.source(), Span::without_point(selection.name)),
            field,
            kind,
            target,
        }))
    }
}

pub(crate) fn field_binding(
    caller: &NamedClassSource,
    navigation: &SourceFileCalls,
    selection: SourceSelection,
    bodies: &CheckedClassBodies,
    typed: &AstArena<Typed>,
    store: &SemanticStore,
) -> Option<(SymbolId, FieldAccessKind)> {
    let owner = navigation.class_at(selection.name.start())?;
    let bindings = bodies.bindings(owner)?;
    field_access(selection, Some(owner), caller.ast(), bindings, typed, store)
}

pub(crate) fn field_target(
    file: &NamedClassSource,
    navigation: &SourceFileCalls,
    owner: SymbolId,
    field: SymbolId,
    store: &SemanticStore,
) -> Result<Option<SourceMemberTarget>, DeclarationError> {
    let visibility = store.symbols.get(field).visibility;
    if !matches!(visibility, Visibility::Public | Visibility::Private) {
        return Ok(None);
    }
    let Some(SourceDefinition::Canonical { source, tree }) = file.index().definition_of(field)
    else {
        return Ok(None);
    };
    let Some(name) = file.named().syntax().member_names.value_name(tree) else {
        return Ok(None);
    };
    let invalid = DeclarationError::InvalidBinding { source, tree };
    let node = file.ast().try_get(tree).ok_or(invalid)?;
    let TreeKind::ValDef(value) = &node.kind else {
        return Err(invalid);
    };
    if visibility == Visibility::Private
        && (value.metadata.visibility != Some(VisibilitySyntax::Private { qualifier: None })
            || value.metadata.modifiers.contains(&Modifier::ParamAccessor))
    {
        return Ok(None);
    }
    let declaration = node
        .position
        .filter(|p| p.source() == file.source())
        .ok_or(invalid)?
        .span()
        .range();
    if source != file.source()
        || name.start() < declaration.start()
        || name.end() > declaration.end()
        || navigation.class_at(name.start()) != Some(owner)
    {
        return Err(invalid);
    }
    Ok(Some(SourceMemberTarget {
        source,
        declaration,
        name,
    }))
}
