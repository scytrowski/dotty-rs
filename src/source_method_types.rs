//! Written class names in the completed signatures of ordinary source methods.
use std::collections::HashMap;

use crate::core::{
    MethodKind, SemanticStore, SourceSpan, SymbolId, SymbolInfo, SymbolKind, TreeKind, Type,
    TypeRefTarget,
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

pub(crate) fn method_types(
    file: &NamedClassSource,
    targets: &HashMap<SymbolId, SourceMemberTarget>,
    store: &SemanticStore,
) -> Vec<ResolvedMethodType> {
    let mut sites = Vec::new();
    for method in file.classes().iter().flat_map(|c| c.methods()) {
        if !store.symbols.contains(method.symbol) {
            continue;
        }
        let SymbolInfo::Complete(info) = store.symbols.info(method.symbol) else {
            continue;
        };
        let Some(Type::Method(signature)) = store.types.try_get(*info) else {
            continue;
        };
        let TreeKind::DefDef(def) = &file.ast().get(method.tree).kind else {
            continue;
        };
        let [clause] = def.value_param_clauses.as_slice() else {
            continue;
        };
        if signature.kind != MethodKind::Plain
            || !def.type_params.is_empty()
            || clause.len() != signature.params.len()
        {
            continue;
        }
        let mut record = |tree, ty, role| {
            let node = file.ast().get(tree);
            let TreeKind::Ident(ident) = &node.kind else {
                return;
            };
            let Some(origin) = node.position.filter(|p| p.source() == file.source()) else {
                return;
            };
            let Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(class),
                ..
            }) = store.types.try_get(ty)
            else {
                return;
            };
            let Some(&target) = targets.get(class) else {
                return;
            };
            let class_symbol = store.symbols.get(*class);
            if class_symbol.kind == SymbolKind::Class && class_symbol.name == ident.name {
                sites.push(ResolvedMethodType {
                    method: method.symbol,
                    role,
                    class: *class,
                    origin,
                    target,
                });
            }
        };
        for (&tree, formal) in clause.iter().zip(&signature.params) {
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
        record(def.tpt, signature.result, MethodTypeRole::Result);
    }
    sites
}
