//! Arena-independent signature data for indexed source symbols.

use crate::core::{
    MethodKind, SemanticStore, SymbolId, SymbolInfo, SymbolKind, Type, TypeId, TypeRefTarget,
};

/// A reference to a class or trait by its canonical identity within one revision.
/// Covers builtins and top-level class types with no prefix or a package prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceNamedType {
    /// Exact semantic type, usable with the original type arena.
    pub ty: TypeId,
    /// Class or trait designated by the type reference in the original session.
    pub symbol: SymbolId,
    /// Simple semantic name, retained for clients that release the arena.
    pub name: Box<str>,
}

impl SourceNamedType {
    fn capture(store: &SemanticStore, ty: TypeId) -> Option<Self> {
        let Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(symbol),
        } = store.types.try_get(ty)?
        else {
            return None;
        };
        match store.types.try_get(*prefix)? {
            Type::NoPrefix => {}
            Type::TypeRef {
                target: TypeRefTarget::Symbol(package),
                ..
            } if store.symbols.contains(*package)
                && store.symbols.get(*package).kind == SymbolKind::Package => {}
            _ => return None,
        }
        if !store.symbols.contains(*symbol) {
            return None;
        }
        let target = store.symbols.get(*symbol);
        matches!(target.kind, SymbolKind::Class | SymbolKind::Trait).then(|| Self {
            ty,
            symbol: *symbol,
            name: store.names.resolve(target.name.text()).into(),
        })
    }
}

/// One formal of a completed ordinary method clause, in declaration order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSignatureParameter {
    /// Getter registered for this formal on this exact declaration, in the original session.
    /// `None` denotes a required parameter.
    pub default_getter: Option<SymbolId>,
    /// Semantic name from the selected method's formal parameter.
    pub name: Box<str>,
    /// Completed type of this formal.
    pub ty: SourceNamedType,
}

/// Declaration information supported by the retained source index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceSymbolSignature {
    /// An ordinary source class. Its identity is the symbol at the queried site.
    Class,
    /// Completed declared or inferred type of a field, parameter or method local.
    Value(SourceNamedType),
    /// A completed method with a named result and at most one plain value clause.
    Method {
        /// Original declaration type for clients retaining the semantic store.
        ty: TypeId,
        /// Ordered formals. `None` means a parameterless method; an empty slice means `()`.
        parameters: Option<Box<[SourceSignatureParameter]>>,
        /// Completed explicit or inferred result type of this declaration.
        result: SourceNamedType,
    },
}

/// Spelling and typed declaration information shared by all sites of one symbol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSymbolInfo {
    /// Semantic name of the declaration, without source quoting.
    pub name: Box<str>,
    /// Captured declaration signature in this revision.
    pub signature: SourceSymbolSignature,
}

impl SourceSymbolInfo {
    pub(crate) fn capture(store: &SemanticStore, id: SymbolId) -> Option<Self> {
        let symbol = store.symbols.get(id);
        let signature = match symbol.kind {
            SymbolKind::Class => SourceSymbolSignature::Class,
            SymbolKind::Field | SymbolKind::Parameter | SymbolKind::Local => {
                let SymbolInfo::Complete(ty) = symbol.info else {
                    return None;
                };
                SourceSymbolSignature::Value(SourceNamedType::capture(store, ty)?)
            }
            SymbolKind::Method => {
                let SymbolInfo::Complete(ty) = symbol.info else {
                    return None;
                };
                let (parameters, result) = match store.types.try_get(ty)? {
                    Type::Method(method) if method.kind == MethodKind::Plain => {
                        let parameters = method
                            .params
                            .iter()
                            .enumerate()
                            .map(|(i, p)| {
                                if p.erased || p.varargs {
                                    return None;
                                }
                                Some(SourceSignatureParameter {
                                    default_getter: store.default_getter(id, i),
                                    name: store.names.resolve(p.name.as_name().text()).into(),
                                    ty: SourceNamedType::capture(store, p.ty)?,
                                })
                            })
                            .collect::<Option<Box<[_]>>>()?;
                        (Some(parameters), method.result)
                    }
                    _ => (None, ty),
                };
                SourceSymbolSignature::Method {
                    ty,
                    parameters,
                    result: SourceNamedType::capture(store, result)?,
                }
            }
            _ => return None,
        };
        Some(Self {
            name: store.names.resolve(symbol.name.text()).into(),
            signature,
        })
    }
}
