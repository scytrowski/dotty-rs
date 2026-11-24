//! Written class names in the completed signatures of ordinary source methods and primary constructors.
use std::collections::HashMap;

use crate::core::{
    MethodKind, MethodType, SemanticStore, SourceDefinition, SourceSpan, SymbolId, SymbolInfo,
    SymbolKind, TreeId, TreeKind, Type, TypeId, TypeRefTarget, Untyped,
};
use crate::source_declarations::canonical_symbol;
use crate::{NamedClassSource, SourceMemberTarget};

/// Position of a written class type in an ordinary method signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodTypeRole {
    /// Type of this canonical method-owned value parameter.
    Parameter(SymbolId),
    /// Explicit return type of the method.
    Result,
}

/// A simple written class type in a completed signature. The symbols and UTF-8
/// ranges belong to the naming and typing revision used to build the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedMethodType {
    /// Canonical method whose completed signature supplies the semantic type.
    pub method: SymbolId,
    /// Parameter or result corresponding to this annotation.
    pub role: MethodTypeRole,
    /// Canonical ordinary source class named by the annotation.
    pub class: SymbolId,
    /// Written class-name token in the method declaration, including any backticks.
    pub origin: SourceSpan,
    /// Declaration and written name of the class in the selected sources.
    pub target: SourceMemberTarget,
}

/// A written parameter type in a completed source primary constructor.
/// All identities and half-open UTF-8 ranges belong to the indexed revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedConstructorType {
    /// Primary constructor whose completed signature supplies the parameter type.
    pub constructor: SymbolId,
    /// Class-owned source parameter, or field for a written `val` or `var`.
    pub parameter: SymbolId,
    /// Constructor-owned formal derived from the same parameter declaration.
    pub formal: SymbolId,
    /// Canonical source class named by the annotation.
    pub class: SymbolId,
    /// Exact written class-name token, including backticks.
    pub origin: SourceSpan,
    /// Definition of the referenced class within the selected sources.
    pub target: SourceMemberTarget,
}

struct WrittenSignature<'a> {
    parameters: &'a [TreeId<Untyped>],
    result: TreeId<Untyped>,
    ty: &'a MethodType,
}

fn written_signature<'a>(
    file: &'a NamedClassSource,
    store: &'a SemanticStore,
    symbol: SymbolId,
    tree: TreeId<Untyped>,
) -> Option<WrittenSignature<'a>> {
    if !store.symbols.contains(symbol) {
        return None;
    }
    let SymbolInfo::Complete(info) = store.symbols.info(symbol) else {
        return None;
    };
    let Type::Method(ty) = store.types.try_get(*info)? else {
        return None;
    };
    let TreeKind::DefDef(def) = &file.ast().try_get(tree)?.kind else {
        return None;
    };
    let [parameters] = def.value_param_clauses.as_slice() else {
        return None;
    };
    (ty.kind == MethodKind::Plain
        && def.type_params.is_empty()
        && parameters.len() == ty.params.len())
    .then_some(WrittenSignature {
        parameters,
        result: def.tpt,
        ty,
    })
}

fn class_annotation(
    file: &NamedClassSource,
    targets: &HashMap<SymbolId, SourceMemberTarget>,
    store: &SemanticStore,
    tree: TreeId<Untyped>,
    ty: TypeId,
) -> Option<(SymbolId, SourceSpan, SourceMemberTarget)> {
    let node = file.ast().get(tree);
    let TreeKind::Ident(ident) = &node.kind else {
        return None;
    };
    let origin = node.position.filter(|p| p.source() == file.source())?;
    let Type::TypeRef {
        target: TypeRefTarget::Symbol(class),
        ..
    } = store.types.try_get(ty)?
    else {
        return None;
    };
    let target = *targets.get(class)?;
    let symbol = store.symbols.get(*class);
    (symbol.kind == SymbolKind::Class && symbol.name == ident.name)
        .then_some((*class, origin, target))
}

pub(crate) fn method_types(
    file: &NamedClassSource,
    targets: &HashMap<SymbolId, SourceMemberTarget>,
    store: &SemanticStore,
) -> Vec<ResolvedMethodType> {
    let mut sites = Vec::new();
    for method in file.classes().iter().flat_map(|c| c.methods()) {
        let Some(signature) = written_signature(file, store, method.symbol, method.tree) else {
            continue;
        };
        let mut record = |tree, ty, role| {
            if let Some((class, origin, target)) = class_annotation(file, targets, store, tree, ty)
            {
                sites.push(ResolvedMethodType {
                    method: method.symbol,
                    role,
                    class,
                    origin,
                    target,
                });
            }
        };
        for (&tree, formal) in signature.parameters.iter().zip(&signature.ty.params) {
            let TreeKind::ValDef(param) = &file.ast().get(tree).kind else {
                continue;
            };
            let Some(symbol) =
                canonical_symbol(store, file.index(), file.source(), tree, method.symbol)
            else {
                continue;
            };
            if param.name == formal.name && store.symbols.get(symbol).kind == SymbolKind::Parameter
            {
                record(param.tpt, formal.ty, MethodTypeRole::Parameter(symbol));
            }
        }
        record(
            signature.result,
            signature.ty.result,
            MethodTypeRole::Result,
        );
    }
    sites
}

pub(crate) fn constructor_types(
    file: &NamedClassSource,
    targets: &HashMap<SymbolId, SourceMemberTarget>,
    store: &SemanticStore,
) -> Vec<ResolvedConstructorType> {
    let mut sites = Vec::new();
    let source = file.source();
    for owner in file.classes() {
        let constructor = owner.constructor;
        let Some(SourceDefinition::Canonical { source: unit, tree }) =
            file.index().definition_of(constructor)
        else {
            continue;
        };
        if unit != source {
            continue;
        }
        let Some(signature) = written_signature(file, store, constructor, tree) else {
            continue;
        };
        for (&tree, slot) in signature.parameters.iter().zip(&signature.ty.params) {
            let TreeKind::ValDef(param) = &file.ast().get(tree).kind else {
                continue;
            };
            if param.name != slot.name {
                continue;
            }
            let Some((class, origin, target)) =
                class_annotation(file, targets, store, param.tpt, slot.ty)
            else {
                continue;
            };
            let Some(parameter) = canonical_symbol(store, file.index(), source, tree, owner.symbol)
            else {
                continue;
            };
            let Some(formal) = file.index().derived_symbol_at(constructor, source, tree) else {
                continue;
            };
            if matches!(
                store.symbols.get(parameter).kind,
                SymbolKind::Parameter | SymbolKind::Field
            ) && store.symbols.contains(formal)
                && store.symbols.get(formal).kind == SymbolKind::Parameter
                && store.symbols.get(formal).owner == Some(constructor)
                && file.index().definition_of(formal)
                    == Some(SourceDefinition::Derived { source, tree })
            {
                sites.push(ResolvedConstructorType {
                    constructor,
                    parameter,
                    formal,
                    class,
                    origin,
                    target,
                });
            }
        }
    }
    sites
}
