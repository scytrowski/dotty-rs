//! Shared parsing of dotted source references.

use dotty_core::ast::{Ident, Select};
use dotty_core::{HardKeyword, Name, TokenKind, TreeId, TreeKind, Untyped};

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
    /// Returns the term name accepted after a selection dot. Scala's scanner
    /// presents ordinary symbolic method names as identifiers; our scanner
    /// keeps them as operators, so bridge those spellings here while leaving
    /// grammar tokens such as `=`, arrows, and bounds reserved.
    pub(crate) fn current_selector_name(&mut self) -> Option<(Name, bool)> {
        let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
        let is_name = match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => true,
            TokenKind::Operator => !["=", "=>", "<-", "<:", ">:", "<%", "@", "?=>"]
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
        if !matches!(
            self.current().kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) {
            return None;
        }

        let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
        let name = match namespace {
            ReferenceNamespace::Term => *self.intern_current_term_name().ok()?.as_name(),
            ReferenceNamespace::Type => *self.intern_current_type_name().ok()?.as_name(),
        };
        Some((name, backquoted))
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
}
