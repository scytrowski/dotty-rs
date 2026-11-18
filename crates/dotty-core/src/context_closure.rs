//! Canonical links in a source-typed unary context-function literal.

use crate::ast::{TreeKind, Typed, TypedAst};
use crate::{
    Definitions, MethodKind, SemanticStore, Symbol, SymbolFlags, SymbolId, SymbolInfo, SymbolKind,
    SymbolOrigin, TermRefTarget, TreeId, Type, TypeId, TypeRefTarget, Visibility,
};

/// Exact declarations and types retained by a unary contextual closure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CheckedUnaryContextClosure {
    pub definition: TreeId<Typed>,
    pub parameter_tree: TreeId<Typed>,
    pub body: TreeId<Typed>,
    pub method: SymbolId,
    pub parameter: SymbolId,
    pub constructor: SymbolId,
    pub input: TypeId,
    pub result: TypeId,
}

fn symbol(store: &SemanticStore, id: SymbolId) -> Option<&Symbol> {
    store.symbols.contains(id).then(|| store.symbols.get(id))
}

/// Read the canonically owned source-synthesized `scala.ContextFunction1`.
/// A separately allocated reference to the same package and class is valid.
fn application(
    store: &SemanticStore,
    defs: &Definitions,
    ty: TypeId,
) -> Option<(SymbolId, TypeId, TypeId)> {
    let Type::Applied { tycon, args } = store.types.try_get(ty)? else {
        return None;
    };
    let [input, result] = args.as_slice() else {
        return None;
    };
    store.types.try_get(*input)?;
    store.types.try_get(*result)?;
    let Type::TypeRef {
        prefix,
        target: TypeRefTarget::Symbol(class),
    } = store.types.try_get(*tycon)?
    else {
        return None;
    };
    let entry = symbol(store, *class)?;
    let SymbolInfo::Complete(info) = entry.info else {
        return None;
    };
    let Type::ClassInfo(info) = store.types.try_get(info)? else {
        return None;
    };
    if info.class != *class
        || info.parents.as_slice() != [defs.object_type]
        || !store.scopes.contains(info.declarations)
        || store.scopes.get(info.declarations).owner != Some(*class)
    {
        return None;
    }
    let package = entry.owner?;
    let owner = symbol(store, package)?;
    let root = symbol(store, owner.owner?)?;
    (entry.kind == SymbolKind::Trait
        && entry.origin == SymbolOrigin::Synthetic
        && entry.flags == (SymbolFlags::SYNTHETIC | SymbolFlags::ABSTRACT)
        && store.names.try_resolve(entry.name.text()) == Some("ContextFunction1")
        && store.types.try_get(info.prefix) == store.types.try_get(*prefix)
        && owner.kind == SymbolKind::Package
        && store.names.try_resolve(owner.name.text()) == Some("scala")
        && root.kind == SymbolKind::Package
        && root.owner.is_none()
        && store.names.try_resolve(root.name.text()) == Some("")
        && store.types.try_get(*prefix) == Some(&Type::type_ref(defs.no_prefix, package)))
    .then_some((*class, *input, *result))
}

/// Validates the source closure's reference, method definition and single
/// formal against the declared result's type constructor and arguments.
/// Consumers check body and capture support for their respective output.
pub fn checked_unary_context_closure(
    store: &SemanticStore,
    defs: &Definitions,
    ast: &TypedAst,
    owner: SymbolId,
    root: TreeId<Typed>,
    declared_result: TypeId,
) -> Option<CheckedUnaryContextClosure> {
    symbol(store, owner)?;
    let tree = ast.try_get(root)?;
    let TreeKind::Closure(closure) = &tree.kind else {
        return None;
    };
    let expected = application(store, defs, declared_result)?;
    if closure.tpt.is_some() || application(store, defs, tree.ty)? != expected {
        return None;
    }
    let (constructor, input, result) = expected;
    let implementation = closure.implementation.as_ref()?;
    let [parameter] = implementation.parameters.as_slice() else {
        return None;
    };
    let reference = ast.try_get(closure.method)?;
    let definition = ast.try_get(implementation.definition)?;
    let TreeKind::Ident(ident) = &reference.kind else {
        return None;
    };
    let TreeKind::DefDef(def) = &definition.kind else {
        return None;
    };
    let Type::TermRef {
        prefix,
        target: TermRefTarget::Symbol(method),
    } = store.types.try_get(reference.ty)?
    else {
        return None;
    };
    let entry = symbol(store, *method)?;
    if *prefix != defs.no_prefix
        || reference.ty != definition.ty
        || !def.type_params.is_empty()
        || def.source_param_clause_order.is_some()
        || ident.name != entry.name
        || def.name.as_name() != &entry.name
        || entry.kind != SymbolKind::Method
        || entry.origin != SymbolOrigin::Synthetic
        || entry.owner != Some(owner)
        || entry.visibility != Visibility::Private
        || entry.flags != (SymbolFlags::SYNTHETIC | SymbolFlags::FINAL)
        || ast.try_get(def.tpt)?.ty != result
    {
        return None;
    }
    let SymbolInfo::Complete(info) = entry.info else {
        return None;
    };
    let Type::Method(signature) = store.types.try_get(info)? else {
        return None;
    };
    let [formal] = signature.params.as_slice() else {
        return None;
    };
    let [clause] = def.value_param_clauses.as_slice() else {
        return None;
    };
    let [param_tree] = clause.as_slice() else {
        return None;
    };
    let val = ast.try_get(*param_tree)?;
    let TreeKind::ValDef(param) = &val.kind else {
        return None;
    };
    let param_entry = symbol(store, *parameter)?;
    if signature.kind != MethodKind::Contextual
        || signature.result != result
        || formal.ty != input
        || formal.erased
        || formal.varargs
        || formal.name != param.name
        || param.name.as_name() != &param_entry.name
        || val.ty != input
        || ast.try_get(param.tpt)?.ty != input
        || param.rhs.is_some()
        || param_entry.kind != SymbolKind::Parameter
        || param_entry.owner != Some(*method)
        || param_entry.flags != SymbolFlags::GIVEN
        || param_entry.origin != SymbolOrigin::Synthetic
        || param_entry.info != SymbolInfo::Complete(input)
    {
        return None;
    }
    let body = def.rhs?;
    if body == root || body == implementation.definition {
        return None;
    }
    ast.try_get(body)?;
    Some(CheckedUnaryContextClosure {
        definition: implementation.definition,
        parameter_tree: *param_tree,
        body,
        method: *method,
        parameter: *parameter,
        constructor,
        input,
        result,
    })
}
