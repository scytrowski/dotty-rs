use dotty_core::{HardKeyword, Punctuation, TokenKind};

/// Token synchronization contexts used by parser recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoverySet {
    Statement,
    Argument,
    TypeArgument,
    Case,
}

impl RecoverySet {
    /// Returns whether `kind` is a synchronization token for this context.
    pub const fn contains(self, kind: TokenKind) -> bool {
        match self {
            Self::Statement => matches!(
                kind,
                TokenKind::Newline
                    | TokenKind::Newlines
                    | TokenKind::Punctuation(Punctuation::Semicolon)
                    | TokenKind::Outdent
                    | TokenKind::Punctuation(Punctuation::RightBrace)
                    | TokenKind::Eof
            ),
            Self::Argument => matches!(
                kind,
                TokenKind::Punctuation(Punctuation::Comma)
                    | TokenKind::Punctuation(Punctuation::RightParen)
                    | TokenKind::Punctuation(Punctuation::RightBrace)
                    | TokenKind::Outdent
                    | TokenKind::Eof
            ),
            Self::TypeArgument => matches!(
                kind,
                TokenKind::Punctuation(Punctuation::Comma)
                    | TokenKind::Punctuation(Punctuation::RightBracket)
                    | TokenKind::Eof
            ),
            Self::Case => matches!(
                kind,
                TokenKind::Keyword(HardKeyword::Case)
                    | TokenKind::Outdent
                    | TokenKind::Punctuation(Punctuation::RightBrace)
                    | TokenKind::Eof
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statement_recovery_accepts_all_statement_boundaries() {
        for kind in [
            TokenKind::Newline,
            TokenKind::Newlines,
            TokenKind::Punctuation(Punctuation::Semicolon),
            TokenKind::Outdent,
            TokenKind::Punctuation(Punctuation::RightBrace),
            TokenKind::Eof,
        ] {
            assert!(RecoverySet::Statement.contains(kind), "{kind:?}");
        }
    }

    #[test]
    fn argument_recovery_accepts_argument_boundaries() {
        for kind in [
            TokenKind::Punctuation(Punctuation::Comma),
            TokenKind::Punctuation(Punctuation::RightParen),
            TokenKind::Punctuation(Punctuation::RightBrace),
            TokenKind::Outdent,
            TokenKind::Eof,
        ] {
            assert!(RecoverySet::Argument.contains(kind), "{kind:?}");
        }
    }

    #[test]
    fn type_argument_recovery_accepts_type_argument_boundaries() {
        for kind in [
            TokenKind::Punctuation(Punctuation::Comma),
            TokenKind::Punctuation(Punctuation::RightBracket),
            TokenKind::Eof,
        ] {
            assert!(RecoverySet::TypeArgument.contains(kind), "{kind:?}");
        }
    }

    #[test]
    fn case_recovery_accepts_case_boundaries() {
        for kind in [
            TokenKind::Keyword(HardKeyword::Case),
            TokenKind::Outdent,
            TokenKind::Punctuation(Punctuation::RightBrace),
            TokenKind::Eof,
        ] {
            assert!(RecoverySet::Case.contains(kind), "{kind:?}");
        }
    }

    #[test]
    fn recovery_sets_do_not_accept_unrelated_tokens() {
        let unrelated = TokenKind::Identifier;

        assert!(!RecoverySet::Statement.contains(unrelated));
        assert!(!RecoverySet::Argument.contains(unrelated));
        assert!(!RecoverySet::TypeArgument.contains(unrelated));
        assert!(!RecoverySet::Case.contains(unrelated));
    }
}
