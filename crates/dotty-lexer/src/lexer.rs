use core::fmt;

use dotty_diagnostics::{Diagnostic, DiagnosticSeverity};
use dotty_source::{SourceText, SourceTextError, TextRange, TextRangeError};

use crate::identifier::{is_identifier_part, is_identifier_start, is_operator_character};
use crate::{Cursor, CursorError, HardKeyword, Punctuation, RawItem, RawToken, RawTokenKind};
use crate::{Trivia, TriviaKind};

/// The first-stage raw lexer for Scala source text.
#[derive(Debug)]
pub struct RawLexer<'source> {
    source: SourceText<'source>,
    cursor: Cursor<'source>,
    diagnostics: Vec<Diagnostic>,
    emitted_eof: bool,
}

/// Failure of a raw-lexer operation that is not recoverable as a source
/// diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawLexerError {
    Cursor(CursorError),
    Source(SourceTextError),
    Range(TextRangeError),
}

impl fmt::Display for RawLexerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cursor(error) => write!(formatter, "cursor error: {error}"),
            Self::Source(error) => write!(formatter, "source error: {error}"),
            Self::Range(error) => write!(formatter, "range error: {error}"),
        }
    }
}

impl std::error::Error for RawLexerError {}

impl From<CursorError> for RawLexerError {
    fn from(error: CursorError) -> Self {
        Self::Cursor(error)
    }
}

impl From<SourceTextError> for RawLexerError {
    fn from(error: SourceTextError) -> Self {
        Self::Source(error)
    }
}

impl From<TextRangeError> for RawLexerError {
    fn from(error: TextRangeError) -> Self {
        Self::Range(error)
    }
}

impl<'source> RawLexer<'source> {
    /// Creates a raw lexer over UTF-8 source text.
    pub fn new(source: &'source str) -> Result<Self, RawLexerError> {
        let source = SourceText::new(source)?;
        let cursor = Cursor::from_source(&source);

        Ok(Self {
            source,
            cursor,
            diagnostics: Vec::new(),
            emitted_eof: false,
        })
    }

    /// Returns diagnostics collected while recovering from malformed input.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Emits the next raw token or trivia item.
    pub fn next(&mut self) -> Result<Option<RawItem>, RawLexerError> {
        if self.emitted_eof {
            return Ok(None);
        }

        let start = self.cursor.position();
        let Some(character) = self.cursor.peek() else {
            self.emitted_eof = true;
            return Ok(Some(RawItem::Token(RawToken {
                kind: RawTokenKind::Eof,
                span: self.span(start)?,
            })));
        };

        if character == ' ' {
            return Ok(Some(RawItem::Trivia(self.scan_whitespace(
                start,
                TriviaKind::Spaces,
                |character| character == ' ',
            )?)));
        }
        if character == '\t' {
            return Ok(Some(RawItem::Trivia(self.scan_whitespace(
                start,
                TriviaKind::Tabs,
                |character| character == '\t',
            )?)));
        }
        if character.is_whitespace() && character != '\n' && character != '\r' {
            return Ok(Some(RawItem::Trivia(self.scan_whitespace(
                start,
                TriviaKind::OtherWhitespace,
                |character| character.is_whitespace() && character != '\n' && character != '\r',
            )?)));
        }
        if character == '\n' || character == '\r' {
            return Ok(Some(RawItem::Trivia(self.scan_newline(start)?)));
        }
        if character == '/' && self.cursor.peek_nth(1) == Some('/') {
            return Ok(Some(RawItem::Trivia(self.scan_line_comment(start)?)));
        }
        if character == '/' && self.cursor.peek_nth(1) == Some('*') {
            return Ok(Some(RawItem::Trivia(self.scan_block_comment(start)?)));
        }
        if is_identifier_start(character) {
            return Ok(Some(RawItem::Token(self.scan_identifier(start)?)));
        }
        if character == '`' {
            return Ok(Some(RawItem::Token(
                self.scan_backquoted_identifier(start)?,
            )));
        }
        if let Some(punctuation) = punctuation(character) {
            let _ = self.cursor.bump();
            return Ok(Some(RawItem::Token(RawToken {
                kind: RawTokenKind::Punctuation(punctuation),
                span: self.span(start)?,
            })));
        }
        if is_operator_character(character) {
            return Ok(Some(RawItem::Token(self.scan_operator(start)?)));
        }

        let _ = self.cursor.bump();
        let span = self.span(start)?;
        self.diagnostics.push(Diagnostic::new(
            DiagnosticSeverity::Error,
            span,
            format!("unsupported source character in the current lexer stage: {character:?}"),
        ));
        Ok(Some(RawItem::Token(RawToken {
            kind: RawTokenKind::Error,
            span,
        })))
    }

    fn scan_whitespace(
        &mut self,
        start: u32,
        kind: TriviaKind,
        accepts: impl Fn(char) -> bool,
    ) -> Result<Trivia, RawLexerError> {
        while self.cursor.peek().is_some_and(&accepts) {
            let _ = self.cursor.bump();
        }

        Ok(Trivia {
            kind,
            span: self.span(start)?,
        })
    }

    fn scan_newline(&mut self, start: u32) -> Result<Trivia, RawLexerError> {
        match self.cursor.bump() {
            Some('\r') => {
                let _ = self.cursor.eat_if('\n');
            }
            Some('\n') => {}
            Some(_) | None => {
                return Err(RawLexerError::Cursor(CursorError::InvalidOffset {
                    offset: start,
                    byte_len: self.cursor.len_bytes(),
                }));
            }
        }

        Ok(Trivia {
            kind: TriviaKind::Newline,
            span: self.span(start)?,
        })
    }

    fn scan_line_comment(&mut self, start: u32) -> Result<Trivia, RawLexerError> {
        let _ = self.cursor.bump();
        let _ = self.cursor.bump();
        while self
            .cursor
            .peek()
            .is_some_and(|character| character != '\n' && character != '\r')
        {
            let _ = self.cursor.bump();
        }

        Ok(Trivia {
            kind: TriviaKind::LineComment,
            span: self.span(start)?,
        })
    }

    fn scan_block_comment(&mut self, start: u32) -> Result<Trivia, RawLexerError> {
        let _ = self.cursor.bump();
        let _ = self.cursor.bump();
        let mut depth = 1u32;

        while depth > 0 {
            match self.cursor.peek() {
                None => {
                    let span = self.span(start)?;
                    self.diagnostics.push(Diagnostic::new(
                        DiagnosticSeverity::Error,
                        span,
                        "unterminated block comment",
                    ));
                    break;
                }
                Some('/') if self.cursor.peek_nth(1) == Some('*') => {
                    let _ = self.cursor.bump();
                    let _ = self.cursor.bump();
                    depth = depth.saturating_add(1);
                }
                Some('*') if self.cursor.peek_nth(1) == Some('/') => {
                    let _ = self.cursor.bump();
                    let _ = self.cursor.bump();
                    depth -= 1;
                }
                Some(_) => {
                    let _ = self.cursor.bump();
                }
            }
        }

        Ok(Trivia {
            kind: TriviaKind::BlockComment,
            span: self.span(start)?,
        })
    }

    fn scan_identifier(&mut self, start: u32) -> Result<RawToken, RawLexerError> {
        let _ = self.cursor.bump();
        while let Some(character) = self.cursor.peek() {
            if character == '_' && self.cursor.peek_nth(1).is_some_and(is_operator_character) {
                let _ = self.cursor.bump();
                while self.cursor.peek().is_some_and(is_operator_character) {
                    let _ = self.cursor.bump();
                }
                break;
            }
            if is_identifier_part(character) {
                let _ = self.cursor.bump();
            } else {
                break;
            }
        }

        let span = self.span(start)?;
        let text = self.source.slice(span)?;
        let kind = classify_keyword(text)
            .map(RawTokenKind::Keyword)
            .unwrap_or(RawTokenKind::Identifier);

        Ok(RawToken { kind, span })
    }

    fn scan_backquoted_identifier(&mut self, start: u32) -> Result<RawToken, RawLexerError> {
        let _ = self.cursor.bump();
        let content_start = self.cursor.position();

        while let Some(character) = self.cursor.peek() {
            if character == '`' {
                break;
            }
            if character == '\n' || character == '\r' {
                break;
            }
            let _ = self.cursor.bump();
        }

        let closed = self.cursor.eat_if('`');
        let span = self.span(start)?;
        let content = self.source.slice(TextRange::new(
            content_start,
            if closed {
                self.cursor.position() - 1
            } else {
                self.cursor.position()
            },
        )?)?;

        if !closed {
            self.diagnostics.push(Diagnostic::new(
                DiagnosticSeverity::Error,
                span,
                "unterminated backquoted identifier",
            ));
            return Ok(RawToken {
                kind: RawTokenKind::Error,
                span,
            });
        }
        if content.is_empty() || content == "_" {
            self.diagnostics.push(Diagnostic::new(
                DiagnosticSeverity::Error,
                span,
                "backquoted identifier must not be empty or equal to `_`",
            ));
            return Ok(RawToken {
                kind: RawTokenKind::Error,
                span,
            });
        }

        Ok(RawToken {
            kind: RawTokenKind::BackquotedIdentifier,
            span,
        })
    }

    fn scan_operator(&mut self, start: u32) -> Result<RawToken, RawLexerError> {
        while let Some(character) = self.cursor.peek() {
            if character == '/' && matches!(self.cursor.peek_nth(1), Some('/') | Some('*')) {
                break;
            }
            if !is_operator_character(character) {
                break;
            }
            let _ = self.cursor.bump();
        }

        Ok(RawToken {
            kind: RawTokenKind::Operator,
            span: self.span(start)?,
        })
    }

    fn span(&self, start: u32) -> Result<TextRange, RawLexerError> {
        TextRange::new(start, self.cursor.position()).map_err(Into::into)
    }
}

fn punctuation(character: char) -> Option<Punctuation> {
    Some(match character {
        ',' => Punctuation::Comma,
        ';' => Punctuation::Semicolon,
        '.' => Punctuation::Dot,
        ':' => Punctuation::Colon,
        '(' => Punctuation::LeftParen,
        ')' => Punctuation::RightParen,
        '[' => Punctuation::LeftBracket,
        ']' => Punctuation::RightBracket,
        '{' => Punctuation::LeftBrace,
        '}' => Punctuation::RightBrace,
        _ => return None,
    })
}

fn classify_keyword(text: &str) -> Option<HardKeyword> {
    Some(match text {
        "if" => HardKeyword::If,
        "for" => HardKeyword::For,
        "else" => HardKeyword::Else,
        "this" => HardKeyword::This,
        "null" => HardKeyword::Null,
        "new" => HardKeyword::New,
        "super" => HardKeyword::Super,
        "abstract" => HardKeyword::Abstract,
        "final" => HardKeyword::Final,
        "private" => HardKeyword::Private,
        "protected" => HardKeyword::Protected,
        "override" => HardKeyword::Override,
        "extends" => HardKeyword::Extends,
        "true" => HardKeyword::True,
        "false" => HardKeyword::False,
        "class" => HardKeyword::Class,
        "import" => HardKeyword::Import,
        "package" => HardKeyword::Package,
        "do" => HardKeyword::Do,
        "sealed" => HardKeyword::Sealed,
        "throw" => HardKeyword::Throw,
        "try" => HardKeyword::Try,
        "catch" => HardKeyword::Catch,
        "finally" => HardKeyword::Finally,
        "while" => HardKeyword::While,
        "return" => HardKeyword::Return,
        "with" => HardKeyword::With,
        "case" => HardKeyword::Case,
        "val" => HardKeyword::Val,
        "implicit" => HardKeyword::Implicit,
        "var" => HardKeyword::Var,
        "def" => HardKeyword::Def,
        "type" => HardKeyword::Type,
        "object" => HardKeyword::Object,
        "yield" => HardKeyword::Yield,
        "trait" => HardKeyword::Trait,
        "match" => HardKeyword::Match,
        "lazy" => HardKeyword::Lazy,
        "then" => HardKeyword::Then,
        "forSome" => HardKeyword::ForSome,
        "enum" => HardKeyword::Enum,
        "given" => HardKeyword::Given,
        "export" => HardKeyword::Export,
        "macro" => HardKeyword::Macro,
        "end" => HardKeyword::End,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(source: &str) -> (Vec<RawItem>, Vec<Diagnostic>) {
        let mut lexer = RawLexer::new(source).expect("valid source");
        let mut items = Vec::new();
        while let Some(item) = lexer.next().expect("lexing succeeds") {
            items.push(item);
        }
        (items, lexer.diagnostics().to_vec())
    }

    fn token(kind: RawTokenKind, start: u32, end: u32) -> RawItem {
        RawItem::Token(RawToken {
            kind,
            span: TextRange::new(start, end).expect("valid range"),
        })
    }

    fn trivia(kind: TriviaKind, start: u32, end: u32) -> RawItem {
        RawItem::Trivia(Trivia {
            kind,
            span: TextRange::new(start, end).expect("valid range"),
        })
    }

    #[test]
    fn emits_identifiers_hard_keywords_and_eof() {
        let (items, diagnostics) = scan("class Foo");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::Keyword(HardKeyword::Class), 0, 5),
                trivia(TriviaKind::Spaces, 5, 6),
                token(RawTokenKind::Identifier, 6, 9),
                token(RawTokenKind::Eof, 9, 9),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn keeps_soft_keywords_as_identifiers() {
        let (items, diagnostics) = scan("using inline extension end");
        let kinds: Vec<_> = items
            .iter()
            .filter_map(|item| match item {
                RawItem::Token(token) if token.kind != RawTokenKind::Eof => Some(token.kind),
                _ => None,
            })
            .collect();

        assert_eq!(
            kinds,
            vec![
                RawTokenKind::Identifier,
                RawTokenKind::Identifier,
                RawTokenKind::Identifier,
                RawTokenKind::Keyword(HardKeyword::End),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn emits_a_valid_backquoted_identifier() {
        let (items, diagnostics) = scan("`foo-bar`");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::BackquotedIdentifier, 0, 9),
                token(RawTokenKind::Eof, 9, 9),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn emits_structural_punctuation_and_greedy_operators() {
        let (items, diagnostics) = scan("(x ++ y)::z");
        let tokens: Vec<_> = items
            .into_iter()
            .filter_map(|item| match item {
                RawItem::Token(token) => Some(token.kind),
                RawItem::Trivia(_) => None,
            })
            .collect();

        assert_eq!(
            tokens,
            vec![
                RawTokenKind::Punctuation(Punctuation::LeftParen),
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Punctuation(Punctuation::RightParen),
                RawTokenKind::Punctuation(Punctuation::Colon),
                RawTokenKind::Punctuation(Punctuation::Colon),
                RawTokenKind::Identifier,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn preserves_spaces_tabs_and_physical_newlines() {
        let (items, diagnostics) = scan("a \t\r\nb");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::Identifier, 0, 1),
                trivia(TriviaKind::Spaces, 1, 2),
                trivia(TriviaKind::Tabs, 2, 3),
                trivia(TriviaKind::Newline, 3, 5),
                token(RawTokenKind::Identifier, 5, 6),
                token(RawTokenKind::Eof, 6, 6),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn preserves_line_comments_without_consuming_the_newline() {
        let (items, diagnostics) = scan("a // comment\nb");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::Identifier, 0, 1),
                trivia(TriviaKind::Spaces, 1, 2),
                trivia(TriviaKind::LineComment, 2, 12),
                trivia(TriviaKind::Newline, 12, 13),
                token(RawTokenKind::Identifier, 13, 14),
                token(RawTokenKind::Eof, 14, 14),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn supports_nested_block_comments() {
        let (items, diagnostics) = scan("a/* outer /* inner */ outer */b");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::Identifier, 0, 1),
                trivia(TriviaKind::BlockComment, 1, 30),
                token(RawTokenKind::Identifier, 30, 31),
                token(RawTokenKind::Eof, 31, 31),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn reports_an_unterminated_block_comment_and_recovers_at_eof() {
        let (items, diagnostics) = scan("/* comment");

        assert_eq!(
            items,
            vec![
                trivia(TriviaKind::BlockComment, 0, 10),
                token(RawTokenKind::Eof, 10, 10),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(0, 10).expect("valid range")
        );
    }

    #[test]
    fn rejects_empty_and_underscore_backquoted_identifiers() {
        let (items, diagnostics) = scan("`` `_`");
        let errors: Vec<_> = items
            .iter()
            .filter_map(|item| match item {
                RawItem::Token(token) if token.kind == RawTokenKind::Error => Some(token.span),
                _ => None,
            })
            .collect();

        assert_eq!(
            errors,
            vec![TextRange::new(0, 2).unwrap(), TextRange::new(3, 6).unwrap()]
        );
        assert_eq!(diagnostics.len(), 2);
    }

    #[test]
    fn keeps_identifier_operator_suffixes_together() {
        let (items, diagnostics) = scan("foo_::");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::Identifier, 0, 6),
                token(RawTokenKind::Eof, 6, 6),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn stops_an_operator_before_a_comment() {
        let (items, diagnostics) = scan("+/* comment */+");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::Operator, 0, 1),
                trivia(TriviaKind::BlockComment, 1, 14),
                token(RawTokenKind::Operator, 14, 15),
                token(RawTokenKind::Eof, 15, 15),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn raw_items_cover_the_source_without_gaps() {
        let source = "class Foo/* comment */\r\nfoo_+";
        let (items, diagnostics) = scan(source);
        let mut offset = 0;

        for item in items {
            let span = match item {
                RawItem::Token(token) if token.kind == RawTokenKind::Eof => token.span,
                RawItem::Token(token) => token.span,
                RawItem::Trivia(trivia) => trivia.span,
            };
            assert_eq!(span.start(), offset);
            offset = span.end();
        }

        assert_eq!(offset, source.len() as u32);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn emits_an_error_token_for_an_unsupported_character() {
        let (items, diagnostics) = scan("1");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::Error, 0, 1),
                token(RawTokenKind::Eof, 1, 1),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
    }
}
