//! The symbol itself: identity plus metadata.

use crate::ids::{AnnotationId, SymbolId};
use crate::names::Name;
use crate::source::SourceSpan;
use crate::symbols::kind::SymbolKind;
use crate::symbols::origin::SymbolOrigin;
use crate::symbols::{SymbolFlags, SymbolInfo};

/// A class's companion object symbol, or an object's companion class symbol.
///
/// Real Dotty stores exactly one mutable link per class-like denotation
/// (`registeredCompanion`) and derives everything else (module class, source
/// module) from it plus ordinary `SymbolKind`/name navigation — it does not
/// store three separate link fields. `dotty-core` follows the same shape.
/// See `docs/dotty-core-design.md` §9, `[MAJOR 4]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SymbolLinks {
    /// Meaningful only when `kind` is `Class`, `Trait`, `Object`, or
    /// `ModuleClass`.
    pub companion: Option<SymbolId>,
}

/// A symbol: an entity's stable identity plus its metadata.
///
/// `Symbol` does **not** carry a `declarations`/`children` field — a class's
/// members are looked up through `SymbolInfo::Complete(TypeId) ->
/// Type::ClassInfo -> ScopeId`, the sole authority for that fact (see
/// `docs/dotty-core-design.md` §9, `[BLOCKER 2]`). Owner and scope
/// membership are also kept distinct: `owner` answers "who semantically
/// holds this symbol," which is not always the same as where it can be
/// found by name lookup.
#[derive(Clone, Debug, PartialEq)]
pub struct Symbol {
    pub name: Name,
    pub owner: Option<SymbolId>,
    pub kind: SymbolKind,
    pub flags: SymbolFlags,
    pub info: SymbolInfo,
    pub origin: SymbolOrigin,
    pub annotations: Vec<AnnotationId>,
    pub position: Option<SourceSpan>,
    pub links: SymbolLinks,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{NameId, TypeId};
    use crate::names::Namespace;

    fn minimal_symbol(kind: SymbolKind) -> Symbol {
        Symbol {
            name: Name::new(NameId::new(1), Namespace::Term),
            owner: None,
            kind,
            flags: SymbolFlags::EMPTY,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        }
    }

    #[test]
    fn a_fresh_symbol_has_no_companion_link_by_default() {
        let symbol = minimal_symbol(SymbolKind::Class);

        assert_eq!(symbol.links.companion, None);
    }

    #[test]
    fn class_and_companion_object_link_to_each_other() {
        let class_id = SymbolId::new(10);
        let object_id = SymbolId::new(11);

        let mut class_symbol = minimal_symbol(SymbolKind::Class);
        class_symbol.links.companion = Some(object_id);

        let mut object_symbol = minimal_symbol(SymbolKind::Object);
        object_symbol.links.companion = Some(class_id);

        assert_eq!(class_symbol.links.companion, Some(object_id));
        assert_eq!(object_symbol.links.companion, Some(class_id));
    }

    #[test]
    fn class_member_lookup_goes_through_symbol_info_not_a_symbol_field() {
        // Symbol has no `declarations` field: the only way to reach a
        // class's members from a Symbol is through its SymbolInfo.
        let mut symbol = minimal_symbol(SymbolKind::Class);
        let class_info_type = TypeId::new(42);
        symbol.info = SymbolInfo::Complete(class_info_type);

        let SymbolInfo::Complete(resolved) = symbol.info else {
            panic!("expected a complete symbol info");
        };
        assert_eq!(resolved, class_info_type);
    }
}
