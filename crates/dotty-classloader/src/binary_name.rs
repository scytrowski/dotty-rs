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

    /// The unqualified simple name: the part after the last `/`, or the
    /// whole name if there is none (a class in the unnamed package).
    pub fn simple_name(&self) -> &str {
        self.0
            .rsplit_once('/')
            .map_or(self.0.as_str(), |(_, simple)| simple)
    }

    /// The containing package, in internal form (`java/util`), or empty
    /// for the unnamed package.
    pub fn package_path(&self) -> &str {
        self.0.rsplit_once('/').map_or("", |(package, _)| package)
    }

    /// Whether this name is safe to turn verbatim into a relative
    /// filesystem or archive path.
    ///
    /// `from_internal`/`from_qualified` accept any string — the JVMS
    /// §4.2.1 grammar alone doesn't guarantee this is safe, and neither
    /// constructor's input is always compiler-driven: a class's own
    /// superclass/interface/member-type name is read straight out of
    /// another class file's constant pool, which is untrusted input once
    /// the classpath itself may contain attacker-supplied bytes. Every
    /// `ClassPathEntry` that maps a `BinaryName` onto a filesystem path or
    /// archive entry name (`DirectoryClassPath`, `JarClassPath`,
    /// `JmodClassPath`) must reject a name this returns `false` for,
    /// rather than resolving it — an unchecked name could otherwise escape
    /// a `DirectoryClassPath`'s root entirely (`../../etc/passwd`, or an
    /// absolute path, which `Path::join` does not confine to the root at
    /// all).
    ///
    /// Rejects: an empty name; a name containing a NUL byte, `\`
    /// (Windows path-separator smuggling), or `:` (a Windows drive
    /// letter, or an NTFS alternate-data-stream marker); and any
    /// `/`-separated segment that is empty (a leading, trailing, or
    /// doubled `/`), `.`, or `..` (path traversal).
    pub fn is_path_safe(&self) -> bool {
        if self.0.is_empty()
            || self.0.contains('\0')
            || self.0.contains('\\')
            || self.0.contains(':')
        {
            return false;
        }

        self.0
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
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

    #[test]
    fn simple_name_is_the_part_after_the_last_slash() {
        let name = BinaryName::from_internal("java/lang/Object");
        assert_eq!(name.simple_name(), "Object");
    }

    #[test]
    fn simple_name_is_the_whole_name_in_the_unnamed_package() {
        let name = BinaryName::from_internal("PoolSample");
        assert_eq!(name.simple_name(), "PoolSample");
    }

    #[test]
    fn package_path_is_everything_before_the_last_slash() {
        let name = BinaryName::from_internal("java/lang/Object");
        assert_eq!(name.package_path(), "java/lang");
    }

    #[test]
    fn package_path_is_empty_in_the_unnamed_package() {
        let name = BinaryName::from_internal("PoolSample");
        assert_eq!(name.package_path(), "");
    }

    #[test]
    fn nested_class_names_keep_dollar_in_their_simple_name() {
        let name = BinaryName::from_internal("com/example/Outer$Inner");
        assert_eq!(name.simple_name(), "Outer$Inner");
        assert_eq!(name.package_path(), "com/example");
    }

    #[test]
    fn ordinary_names_are_path_safe() {
        assert!(BinaryName::from_internal("java/lang/Object").is_path_safe());
        assert!(BinaryName::from_internal("PoolSample").is_path_safe());
        assert!(BinaryName::from_internal("com/example/Outer$Inner").is_path_safe());
    }

    #[test]
    fn an_empty_name_is_not_path_safe() {
        assert!(!BinaryName::from_internal("").is_path_safe());
    }

    #[test]
    fn a_dot_dot_segment_is_not_path_safe() {
        assert!(!BinaryName::from_internal("../../etc/passwd").is_path_safe());
        assert!(!BinaryName::from_internal("a/../../b").is_path_safe());
        assert!(!BinaryName::from_internal("a/..").is_path_safe());
    }

    #[test]
    fn a_leading_slash_is_not_path_safe() {
        assert!(!BinaryName::from_internal("/etc/passwd").is_path_safe());
    }

    #[test]
    fn a_doubled_or_trailing_slash_is_not_path_safe() {
        assert!(!BinaryName::from_internal("java//Object").is_path_safe());
        assert!(!BinaryName::from_internal("java/lang/").is_path_safe());
    }

    #[test]
    fn a_bare_dot_segment_is_not_path_safe() {
        assert!(!BinaryName::from_internal("./Object").is_path_safe());
        assert!(!BinaryName::from_internal(".").is_path_safe());
    }

    #[test]
    fn a_backslash_is_not_path_safe() {
        assert!(!BinaryName::from_internal("..\\..\\Windows\\System32").is_path_safe());
    }

    #[test]
    fn a_colon_is_not_path_safe() {
        assert!(!BinaryName::from_internal("C:/Windows/System32").is_path_safe());
        assert!(!BinaryName::from_internal("Object:hidden").is_path_safe());
    }

    #[test]
    fn a_nul_byte_is_not_path_safe() {
        assert!(!BinaryName::from_internal("Object\0.class").is_path_safe());
    }
}
