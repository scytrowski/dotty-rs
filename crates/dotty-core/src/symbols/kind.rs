//! Stable symbol categories.

/// A symbol's stable category. Orthogonal properties (visibility,
/// mutability, ...) live in [`super::SymbolFlags`] instead of being folded
/// into this enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Package,
    Class,
    Trait,
    Object,
    ModuleClass,
    Method,
    Constructor,
    Field,
    Value,
    Variable,
    Parameter,
    TypeParameter,
    TypeAlias,
    Local,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_kinds_are_not_equal() {
        assert_ne!(SymbolKind::Class, SymbolKind::Trait);
        assert_eq!(SymbolKind::Method, SymbolKind::Method);
    }
}
