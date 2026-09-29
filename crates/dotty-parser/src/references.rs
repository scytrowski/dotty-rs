//! Shared parsing of dotted source references.

use dotty_core::ast::{Ident, Select, This};
use dotty_core::{HardKeyword, Name, Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::Parser;

/// Namespace used when constructing the terminal names of a reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReferenceNamespace {
    Term,
    Type,
}

/// Why a qualified reference could not be completed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QualifiedReferenceError {
    MissingInitial,
    MissingSegment,
}

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses the shared `[id '.'] this` reference form when qualified.
    /// Dotty stores the qualifier in the type-name namespace.
    pub(crate) fn parse_qualified_this_reference(
        &mut self,
        mark: crate::Mark,
    ) -> Option<TreeId<Untyped>> {
        if !self.current_starts_qualified_this() {
            return None;
        }

        let qualifier = *self.intern_current_type_name().ok()?.as_name();
        self.advance();
        self.advance();
        self.advance();
        Some(self.alloc_from(
            mark,
            TreeKind::This(This {
                qual: Some(qualifier),
            }),
        ))
    }

    pub(crate) fn current_starts_qualified_this(&mut self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) && self.cursor.lookahead(1).kind == TokenKind::Punctuation(Punctuation::Dot)
            && self.cursor.lookahead(2).kind == TokenKind::Keyword(HardKeyword::This)
    }

    /// Returns a source term name accepted in a name position. Scala's scanner
    /// presents ordinary symbolic names as identifiers; our scanner keeps
    /// them as operators, so bridge those spellings while leaving grammar
    /// tokens such as `=`, arrows, and bounds reserved.
    pub(crate) fn current_selector_name(&mut self) -> Option<(Name, bool)> {
        self.current_term_name()
    }

    pub(crate) fn current_term_name(&mut self) -> Option<(Name, bool)> {
        let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
        let is_name = match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => true,
            TokenKind::Operator => !["=", "=>", "<-", "<:", ">:", "<%", "@", "?=>", "#", "=>>"]
                .iter()
                .any(|reserved| self.current_text_is(reserved)),
            _ => false,
        };
        if !is_name {
            return None;
        }

        self.intern_current_term_name()
            .ok()
            .map(|name| (*name.as_name(), backquoted))
    }

    pub(crate) fn current_is_type_reference_name(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) || self.current_is_symbolic_type_name()
    }

    /// Parses `id { '.' id }`, preserving the requested name namespace and
    /// the source-level backquoted marker on every segment.
    pub(crate) fn parse_qualified_reference(
        &mut self,
        namespace: ReferenceNamespace,
    ) -> Result<TreeId<Untyped>, QualifiedReferenceError> {
        self.parse_qualified_reference_until_keyword(namespace, None)
    }

    /// Parses a qualified reference while leaving a final `.<keyword>` suffix
    /// for the caller. This is used by type syntax whose final selector is a
    /// keyword rather than a term/type name, such as `x.type`.
    pub(crate) fn parse_qualified_reference_until_keyword(
        &mut self,
        namespace: ReferenceNamespace,
        terminal: Option<HardKeyword>,
    ) -> Result<TreeId<Untyped>, QualifiedReferenceError> {
        let mark = self.mark();
        let Some((name, backquoted)) = self.current_reference_name(namespace) else {
            return Err(QualifiedReferenceError::MissingInitial);
        };
        self.advance();

        let mut tree = self.alloc_from(mark, TreeKind::Ident(Ident { name, backquoted }));
        while self
            .cursor
            .at(TokenKind::Punctuation(dotty_core::Punctuation::Dot))
        {
            if terminal
                .is_some_and(|keyword| self.cursor.lookahead(1).kind == TokenKind::Keyword(keyword))
            {
                break;
            }
            self.advance();
            let Some((name, backquoted)) = self.current_reference_name(namespace) else {
                return Err(QualifiedReferenceError::MissingSegment);
            };
            self.advance();
            tree = self.alloc_from(
                mark,
                TreeKind::Select(Select {
                    qualifier: tree,
                    name,
                    backquoted,
                }),
            );
        }

        Ok(tree)
    }

    fn current_reference_name(&mut self, namespace: ReferenceNamespace) -> Option<(Name, bool)> {
        let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
        let is_name = match namespace {
            ReferenceNamespace::Term => matches!(
                self.current().kind,
                TokenKind::Identifier | TokenKind::BackquotedIdentifier
            ),
            ReferenceNamespace::Type => self.current_is_type_reference_name(),
        };
        if !is_name {
            return None;
        }

        let name = match namespace {
            ReferenceNamespace::Term => *self.intern_current_term_name().ok()?.as_name(),
            ReferenceNamespace::Type => *self.intern_current_type_name().ok()?.as_name(),
        };
        Some((name, backquoted))
    }

    fn current_is_symbolic_type_name(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Operator | TokenKind::ColonOp
        ) && ![
            "=", "=>", "=>>", "<-", "<:", "<%", ">:", "?=>", ":", "@", "#",
        ]
        .iter()
        .any(|reserved| self.current_text_is(reserved))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::{NameInterner, Punctuation, TokenKind, TreeKind};

    #[test]
    fn parses_a_qualified_term_reference_with_backquoted_segments() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "pkg.`value`",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::Dot), 3, 4),
                token(TokenKind::BackquotedIdentifier, 4, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let reference = parser
            .parse_qualified_reference(ReferenceNamespace::Term)
            .expect("expected a qualified reference");
        let TreeKind::Select(selection) = parser.ast().get(reference).kind else {
            panic!("expected a selection");
        };
        assert!(selection.backquoted);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn qualified_type_reference_keeps_the_type_namespace() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "pkg.Value",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::Dot), 3, 4),
                token(TokenKind::Identifier, 4, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let reference = parser
            .parse_qualified_reference(ReferenceNamespace::Type)
            .expect("expected a qualified type reference");
        let TreeKind::Select(selection) = parser.ast().get(reference).kind else {
            panic!("expected a selection");
        };
        assert!(selection.name.is_type());
        assert!(matches!(
            parser.ast().get(selection.qualifier).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn qualified_type_references_accept_symbolic_names_in_the_type_namespace() {
        for (source, kind) in [("::", TokenKind::ColonOp), ("=:=", TokenKind::Operator)] {
            let mut names = NameInterner::new();
            let end = source.len() as u32;
            let mut parser = parser_for(
                source,
                vec![token(kind, 0, end), token(TokenKind::Eof, end, end)],
                &mut names,
            );

            let reference = parser
                .parse_qualified_reference(ReferenceNamespace::Type)
                .expect("symbolic spelling should be accepted as a type name");
            let TreeKind::Ident(ident) = parser.ast().get(reference).kind else {
                panic!("expected a type identifier");
            };
            assert!(ident.name.is_type());
            let name = ident.name;
            assert!(parser.diagnostics().is_empty());
            drop(parser);
            assert_eq!(names.resolve(name.text()), source);
        }
    }

    #[test]
    fn qualified_type_references_do_not_accept_reserved_operators_as_names() {
        for source in ["=", "<:", ">:", "@", "#"] {
            let mut names = NameInterner::new();
            let end = source.len() as u32;
            let mut parser = parser_for(
                source,
                vec![
                    token(TokenKind::Operator, 0, end),
                    token(TokenKind::Eof, end, end),
                ],
                &mut names,
            );

            assert_eq!(
                parser.parse_qualified_reference(ReferenceNamespace::Type),
                Err(QualifiedReferenceError::MissingInitial),
                "reserved spelling {source:?} must stay in its grammar"
            );
        }
    }
}
