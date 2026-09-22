use dotty_core::{NameInterner, TermName};

/// Parser-known Scala 3.9.0 soft keywords.
///
/// The lexer deliberately leaves these words as identifiers. The parser
/// compares their interned names only in grammar positions where they have a
/// contextual meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnownNames {
    pub as_: TermName,
    pub derives: TermName,
    pub extension: TermName,
    pub infix: TermName,
    pub inline: TermName,
    pub opaque: TermName,
    pub open: TermName,
    pub transparent: TermName,
    pub using: TermName,
    /// Contextual name used by template `uses` clauses.
    pub uses: TermName,
    /// Contextual name used by the `initially` clause in capture-aware syntax.
    pub initially: TermName,
    /// Feature-dependent contextual name; its syntax is controlled by
    /// [`crate::ParserFeatures::into`].
    pub into: TermName,
    /// Feature-dependent contextual name; its syntax is controlled by
    /// [`crate::ParserFeatures::erased_definitions`].
    pub erased: TermName,
    /// Name reserved for future capture-checking grammar.
    pub tracked: TermName,
    /// Name reserved for future capture-checking grammar.
    pub update: TermName,
}

impl KnownNames {
    /// Interns the Scala 3.9.0 soft-keyword set in the shared name table.
    pub fn new(names: &mut NameInterner) -> Self {
        Self {
            as_: term_name(names, "as"),
            derives: term_name(names, "derives"),
            extension: term_name(names, "extension"),
            infix: term_name(names, "infix"),
            inline: term_name(names, "inline"),
            opaque: term_name(names, "opaque"),
            open: term_name(names, "open"),
            transparent: term_name(names, "transparent"),
            using: term_name(names, "using"),
            uses: term_name(names, "uses"),
            initially: term_name(names, "initially"),
            into: term_name(names, "into"),
            erased: term_name(names, "erased"),
            tracked: term_name(names, "tracked"),
            update: term_name(names, "update"),
        }
    }
}

fn term_name(names: &mut NameInterner, text: &str) -> TermName {
    TermName::new(names.intern(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_names_cover_the_scala_390_soft_keyword_set() {
        let mut names = NameInterner::new();
        let known = KnownNames::new(&mut names);
        let entries = [
            (known.as_, "as"),
            (known.derives, "derives"),
            (known.extension, "extension"),
            (known.infix, "infix"),
            (known.inline, "inline"),
            (known.opaque, "opaque"),
            (known.open, "open"),
            (known.transparent, "transparent"),
            (known.using, "using"),
            (known.uses, "uses"),
            (known.initially, "initially"),
            (known.into, "into"),
            (known.erased, "erased"),
            (known.tracked, "tracked"),
            (known.update, "update"),
        ];

        for (name, expected) in entries {
            assert_eq!(names.resolve(name.as_name().text()), expected);
        }
    }

    #[test]
    fn known_names_are_term_names() {
        let mut names = NameInterner::new();
        let known = KnownNames::new(&mut names);

        assert!(known.as_.as_name().is_term());
        assert!(known.using.as_name().is_term());
        assert!(known.uses.as_name().is_term());
        assert!(known.initially.as_name().is_term());
        assert!(!known.using.as_name().is_type());
    }

    #[test]
    fn constructing_known_names_reuses_existing_interned_text() {
        let mut names = NameInterner::new();
        let first = KnownNames::new(&mut names);
        let second = KnownNames::new(&mut names);

        assert_eq!(first, second);
        assert_eq!(names.resolve(first.using.as_name().text()), "using");
    }
}
