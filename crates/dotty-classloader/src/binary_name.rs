use std::fmt;

/// A binary class/interface name in JVMS internal form (`java/lang/Object`),
/// per JVMS §4.2.1 and `docs/classfile-format-jdk25.md` §8.
///
/// `$` is preserved as-is: it is a naming convention for nested classes,
/// not a delimiter this type parses. Nesting is reconstructed elsewhere
/// from `InnerClasses`/`EnclosingMethod`, not by splitting on `$`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BinaryName(String);

impl BinaryName {
    /// Wraps a name already in internal form (`java/lang/Object`).
    pub fn from_internal(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// Converts a qualified (dotted) name (`java.lang.Object`) to internal
    /// form.
    pub fn from_qualified(name: &str) -> Self {
        Self(name.replace('.', "/"))
    }

    /// The name in internal form (`java/lang/Object`).
    pub fn as_internal(&self) -> &str {
        &self.0
    }

    /// The name in qualified, dotted form (`java.lang.Object`).
    pub fn to_qualified(&self) -> String {
        self.0.replace('/', ".")
    }
}

impl fmt::Display for BinaryName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_internal_preserves_the_name_as_is() {
        let name = BinaryName::from_internal("java/lang/Object");
        assert_eq!(name.as_internal(), "java/lang/Object");
    }

    #[test]
    fn from_qualified_replaces_dots_with_slashes() {
        let name = BinaryName::from_qualified("java.lang.Object");
        assert_eq!(name.as_internal(), "java/lang/Object");
    }

    #[test]
    fn to_qualified_replaces_slashes_with_dots() {
        let name = BinaryName::from_internal("java/lang/Object");
        assert_eq!(name.to_qualified(), "java.lang.Object");
    }

    #[test]
    fn dollar_is_preserved_as_a_naming_convention_not_parsed() {
        let name = BinaryName::from_internal("Outer$Inner");
        assert_eq!(name.as_internal(), "Outer$Inner");
        assert_eq!(name.to_qualified(), "Outer$Inner");
    }

    #[test]
    fn equal_names_hash_equal() {
        use std::collections::HashSet;

        let mut set = HashSet::new();
        set.insert(BinaryName::from_internal("java/lang/Object"));
        assert!(set.contains(&BinaryName::from_qualified("java.lang.Object")));
    }

    #[test]
    fn display_renders_internal_form() {
        let name = BinaryName::from_internal("java/lang/Runnable");
        assert_eq!(name.to_string(), "java/lang/Runnable");
    }
}
