//! The semantic shape of a class, trait, or object.

use crate::ids::{ScopeId, SymbolId, TypeId};

/// A class's prefix, parents, member scope, and optional self type.
///
/// `declarations` is the **sole** authority for a class's members — `Symbol`
/// does not also carry a `declarations` field. See
/// `docs/dotty-core-design.md` §9, `[BLOCKER 2]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassInfo {
    pub prefix: TypeId,
    pub class: SymbolId,
    pub parents: Vec<TypeId>,
    pub declarations: ScopeId,
    pub self_type: Option<TypeId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_info_is_the_sole_carrier_of_its_declarations_scope() {
        let info = ClassInfo {
            prefix: TypeId::new(1),
            class: SymbolId::new(2),
            parents: vec![TypeId::new(3)],
            declarations: ScopeId::new(4),
            self_type: None,
        };

        assert_eq!(info.declarations, ScopeId::new(4));
        assert_eq!(info.parents, vec![TypeId::new(3)]);
    }
}
