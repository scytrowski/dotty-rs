//! Stable symbol categories.

/// A symbol's stable category. Orthogonal properties (visibility,
/// mutability, ...) live in [`super::SymbolFlags`] instead of being folded
/// into this enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    /// Package identity shared by qualified declarations.
    Package,
    /// Ordinary class declaration.
    Class,
    /// Trait declaration.
    Trait,
    /// Module value, distinct from its module class.
    Object,
    /// Class implementing an object or module value.
    ModuleClass,
    /// Method declaration.
    Method,
    /// Class constructor declaration.
    Constructor,
    /// Class or module storage member.
    Field,
    /// Immutable value declaration.
    Value,
    /// Mutable variable declaration.
    Variable,
    /// Value parameter declaration.
    Parameter,
    /// Type parameter declaration.
    TypeParameter,
    /// Named type alias declaration.
    TypeAlias,
    /// Local declaration identity in an expression scope.
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
