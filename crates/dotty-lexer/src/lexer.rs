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
        if character.is_ascii_digit()
            || (character == '.'
                && self
                    .cursor
                    .peek_nth(1)
                    .is_some_and(|next| next.is_ascii_digit()))
        {
            return Ok(Some(RawItem::Token(self.scan_number(start)?)));
        }
        if character == '\'' {
            return Ok(Some(RawItem::Token(self.scan_char_literal(start)?)));
        }
        if character == '"' {
            return Ok(Some(RawItem::Token(self.scan_string_literal(start)?)));
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

    fn scan_number(&mut self, start: u32) -> Result<RawToken, RawLexerError> {
        let mut kind = RawTokenKind::IntegerLiteral;
        let mut base = 10;

        if self.cursor.peek() == Some('.') {
            let _ = self.cursor.bump();
            kind = RawTokenKind::DecimalLiteral;
            self.scan_decimal_digits(start, "fractional part")?;
        } else {
            if self.cursor.peek() == Some('0')
                && matches!(self.cursor.peek_nth(1), Some('x' | 'X' | 'b' | 'B'))
            {
                let _ = self.cursor.bump();
                match self.cursor.peek() {
                    Some('x' | 'X') => {
                        base = 16;
                        let _ = self.cursor.bump();
                    }
                    Some('b' | 'B') => {
                        base = 2;
                        let _ = self.cursor.bump();
                    }
                    _ => {}
                }
            }

            self.scan_based_digits(start, base)?;

            if base == 10 && self.cursor.peek() == Some('.') {
                if self
                    .cursor
                    .peek_nth(1)
                    .is_some_and(|character| character.is_ascii_digit())
                {
                    let _ = self.cursor.bump();
                    kind = RawTokenKind::DecimalLiteral;
                    self.scan_decimal_digits(start, "fractional part")?;
                }
            }

            if base == 10 && matches!(self.cursor.peek(), Some('e' | 'E')) {
                let _ = self.cursor.bump();
                kind = RawTokenKind::ExponentLiteral;
                if matches!(self.cursor.peek(), Some('+' | '-')) {
                    let _ = self.cursor.bump();
                }
                if !self
                    .cursor
                    .peek()
                    .is_some_and(|character| character.is_ascii_digit())
                {
                    self.report(start, "exponent must contain at least one digit")?;
                } else {
                    self.scan_decimal_digits(start, "exponent")?;
                }
            }
        }

        match self.cursor.peek() {
            Some('l' | 'L') if matches!(kind, RawTokenKind::IntegerLiteral) => {
                let _ = self.cursor.bump();
                kind = RawTokenKind::LongLiteral;
            }
            Some('f' | 'F') if base == 10 => {
                let _ = self.cursor.bump();
                kind = RawTokenKind::FloatLiteral;
            }
            Some('d' | 'D') if base == 10 => {
                let _ = self.cursor.bump();
                kind = RawTokenKind::DoubleLiteral;
            }
            Some('l' | 'L') => {
                self.report(start, "long suffix is only valid on an integer literal")?;
                let _ = self.cursor.bump();
            }
            _ => {}
        }

        if self.cursor.peek().is_some_and(is_identifier_part) {
            self.report(start, "invalid literal number")?;
        }

        Ok(RawToken {
            kind,
            span: self.span(start)?,
        })
    }

    fn scan_based_digits(&mut self, start: u32, base: u32) -> Result<(), RawLexerError> {
        let mut saw_digit = false;
        let mut trailing_separator = false;

        while let Some(character) = self.cursor.peek() {
            if digit_value(character).is_some_and(|digit| digit < base) {
                saw_digit = true;
                trailing_separator = false;
                let _ = self.cursor.bump();
            } else if character == '_' {
                if !saw_digit || trailing_separator {
                    self.report(start, "invalid numeric separator")?;
                }
                trailing_separator = true;
                let _ = self.cursor.bump();
            } else {
                break;
            }
        }

        if trailing_separator {
            self.report(start, "numeric literal must not end with a separator")?;
        }
        if !saw_digit {
            self.report(start, "numeric literal must contain a digit")?;
        }
        if base != 10
            && self
                .cursor
                .peek()
                .is_some_and(|character| character.is_ascii_alphanumeric())
        {
            self.report(start, "invalid digit in non-decimal literal")?;
        }

        Ok(())
    }

    fn scan_decimal_digits(&mut self, start: u32, component: &str) -> Result<(), RawLexerError> {
        let mut saw_digit = false;
        let mut trailing_separator = false;

        while let Some(character) = self.cursor.peek() {
            if character.is_ascii_digit() {
                saw_digit = true;
                trailing_separator = false;
                let _ = self.cursor.bump();
            } else if character == '_' {
                if !saw_digit || trailing_separator {
                    self.report(start, "invalid numeric separator")?;
                }
                trailing_separator = true;
                let _ = self.cursor.bump();
            } else {
                break;
            }
        }

        if trailing_separator {
            self.report(start, "numeric literal must not end with a separator")?;
        }
        if !saw_digit {
            self.report(start, format!("{component} must contain a digit"))?;
        }

        Ok(())
    }

    fn scan_char_literal(&mut self, start: u32) -> Result<RawToken, RawLexerError> {
        let _ = self.cursor.bump();
        let mut valid = true;

        match self.cursor.peek() {
            None | Some('\n' | '\r') => {
                self.report(start, "unterminated character literal")?;
                return Ok(RawToken {
                    kind: RawTokenKind::Error,
                    span: self.span(start)?,
                });
            }
            Some('\'') => {
                let _ = self.cursor.bump();
                self.report(start, "empty character literal")?;
                return Ok(RawToken {
                    kind: RawTokenKind::Error,
                    span: self.span(start)?,
                });
            }
            Some('\\') => {
                valid &= self.scan_escape(start)?;
            }
            Some(character) => {
                let _ = self.cursor.bump();
                if character.len_utf16() != 1 {
                    self.report(start, "character literal must contain one UTF-16 code unit")?;
                    valid = false;
                }
            }
        }

        if self.cursor.peek() == Some('\'') {
            let _ = self.cursor.bump();
        } else if !self.cursor.is_eof() && !matches!(self.cursor.peek(), Some('\n' | '\r')) {
            valid = false;
            self.report(start, "character literal contains more than one character")?;
            while let Some(character) = self.cursor.peek() {
                let _ = self.cursor.bump();
                if character == '\'' || character == '\n' || character == '\r' {
                    break;
                }
            }
        } else if !self.cursor.is_eof() {
            valid = false;
            self.report(start, "unterminated character literal")?;
        }

        Ok(RawToken {
            kind: if valid {
                RawTokenKind::CharLiteral
            } else {
                RawTokenKind::Error
            },
            span: self.span(start)?,
        })
    }

    fn scan_string_literal(&mut self, start: u32) -> Result<RawToken, RawLexerError> {
        let multiline =
            self.cursor.peek_nth(1) == Some('"') && self.cursor.peek_nth(2) == Some('"');
        if multiline {
            let _ = self.cursor.bump();
            let _ = self.cursor.bump();
            let _ = self.cursor.bump();
            return self.scan_multiline_string(start);
        }

        let _ = self.cursor.bump();
        loop {
            match self.cursor.peek() {
                Some('"') => {
                    let _ = self.cursor.bump();
                    return Ok(RawToken {
                        kind: RawTokenKind::StringLiteral,
                        span: self.span(start)?,
                    });
                }
                Some('\n' | '\r') => {
                    self.report(start, "unclosed string literal")?;
                    return Ok(RawToken {
                        kind: RawTokenKind::Error,
                        span: self.span(start)?,
                    });
                }
                Some('\\') => {
                    let _ = self.scan_escape(start)?;
                }
                Some(_) => {
                    let _ = self.cursor.bump();
                }
                None => {
                    self.report(start, "unclosed string literal")?;
                    return Ok(RawToken {
                        kind: RawTokenKind::Error,
                        span: self.span(start)?,
                    });
                }
            }
        }
    }

    fn scan_multiline_string(&mut self, start: u32) -> Result<RawToken, RawLexerError> {
        loop {
            if self.cursor.peek() == Some('"')
                && self.cursor.peek_nth(1) == Some('"')
                && self.cursor.peek_nth(2) == Some('"')
            {
                let _ = self.cursor.bump();
                let _ = self.cursor.bump();
                let _ = self.cursor.bump();
                return Ok(RawToken {
                    kind: RawTokenKind::StringLiteral,
                    span: self.span(start)?,
                });
            }
            if self.cursor.bump().is_none() {
                self.report(start, "unclosed multi-line string literal")?;
                return Ok(RawToken {
                    kind: RawTokenKind::Error,
                    span: self.span(start)?,
                });
            }
        }
    }

    fn scan_escape(&mut self, start: u32) -> Result<bool, RawLexerError> {
        let _ = self.cursor.bump();
        let valid = match self.cursor.peek() {
            Some('b' | 't' | 'n' | 'f' | 'r' | '"' | '\'' | '\\') => {
                let _ = self.cursor.bump();
                true
            }
            Some('u' | 'U') => self.scan_unicode_escape(start)?,
            Some('0'..='7') => self.scan_octal_escape(start)?,
            Some(_) => {
                let _ = self.cursor.bump();
                self.report(start, "invalid escape character")?;
                false
            }
            None => {
                self.report(start, "unterminated escape sequence")?;
                false
            }
        };

        Ok(valid)
    }

    fn scan_unicode_escape(&mut self, start: u32) -> Result<bool, RawLexerError> {
        while matches!(self.cursor.peek(), Some('u' | 'U')) {
            let _ = self.cursor.bump();
        }

        for _ in 0..4 {
            match self.cursor.peek() {
                Some(character) if character.is_ascii_hexdigit() => {
                    let _ = self.cursor.bump();
                }
                Some(_) => {
                    self.report(start, "invalid character in Unicode escape sequence")?;
                    return Ok(false);
                }
                None => {
                    self.report(start, "incomplete Unicode escape sequence")?;
                    return Ok(false);
                }
            }
        }

        Ok(true)
    }

    fn scan_octal_escape(&mut self, start: u32) -> Result<bool, RawLexerError> {
        let mut count = 0;
        while count < 3
            && self
                .cursor
                .peek()
                .is_some_and(|character| ('0'..='7').contains(&character))
        {
            let _ = self.cursor.bump();
            count += 1;
        }
        self.report(
            start,
            "octal escape literals are unsupported; use a Unicode escape",
        )?;
        Ok(false)
    }

    fn report(&mut self, start: u32, message: impl Into<String>) -> Result<(), RawLexerError> {
        self.diagnostics.push(Diagnostic::new(
            DiagnosticSeverity::Error,
            self.span(start)?,
            message,
        ));
        Ok(())
    }

    fn span(&self, start: u32) -> Result<TextRange, RawLexerError> {
        TextRange::new(start, self.cursor.position()).map_err(Into::into)
    }
}

fn digit_value(character: char) -> Option<u32> {
    match character {
        '0'..='9' => Some(character as u32 - '0' as u32),
        'a'..='f' => Some(character as u32 - 'a' as u32 + 10),
        'A'..='F' => Some(character as u32 - 'A' as u32 + 10),
        _ => None,
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
    fn recognizes_integer_bases_and_numeric_separators() {
        let (items, diagnostics) = scan("0 42 1_000 0xff 0b1010");
        let kinds: Vec<_> = items
            .into_iter()
            .filter_map(|item| match item {
                RawItem::Token(token) if token.kind != RawTokenKind::Eof => Some(token.kind),
                _ => None,
            })
            .collect();

        assert_eq!(
            kinds,
            vec![
                RawTokenKind::IntegerLiteral,
                RawTokenKind::IntegerLiteral,
                RawTokenKind::IntegerLiteral,
                RawTokenKind::IntegerLiteral,
                RawTokenKind::IntegerLiteral,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn recognizes_decimal_exponent_and_suffix_forms() {
        let (items, diagnostics) = scan("1.0 .5 1e10 1e-10 1f 1d 1L 1.");
        let kinds: Vec<_> = items
            .into_iter()
            .filter_map(|item| match item {
                RawItem::Token(token) => Some(token.kind),
                RawItem::Trivia(_) => None,
            })
            .collect();

        assert_eq!(
            kinds,
            vec![
                RawTokenKind::DecimalLiteral,
                RawTokenKind::DecimalLiteral,
                RawTokenKind::ExponentLiteral,
                RawTokenKind::ExponentLiteral,
                RawTokenKind::FloatLiteral,
                RawTokenKind::DoubleLiteral,
                RawTokenKind::LongLiteral,
                RawTokenKind::IntegerLiteral,
                RawTokenKind::Punctuation(Punctuation::Dot),
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn keeps_unary_minus_outside_the_numeric_literal() {
        let (items, diagnostics) = scan("-123");
        let kinds: Vec<_> = items
            .into_iter()
            .filter_map(|item| match item {
                RawItem::Token(token) => Some(token.kind),
                RawItem::Trivia(_) => None,
            })
            .collect();

        assert_eq!(
            kinds,
            vec![
                RawTokenKind::Operator,
                RawTokenKind::IntegerLiteral,
                RawTokenKind::Eof
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn diagnoses_malformed_numeric_separators() {
        let (items, diagnostics) = scan("1_ 0x_1 1.0_");

        assert!(items.iter().all(|item| !matches!(
            item,
            RawItem::Token(RawToken {
                kind: RawTokenKind::Error,
                ..
            })
        )));
        assert_eq!(diagnostics.len(), 3);
    }

    #[test]
    fn does_not_accept_decimal_suffixes_on_non_decimal_literals() {
        let (items, diagnostics) = scan("0b1010f 0xffd");
        let kinds: Vec<_> = items
            .into_iter()
            .filter_map(|item| match item {
                RawItem::Token(token) => Some(token.kind),
                RawItem::Trivia(_) => None,
            })
            .collect();

        assert_eq!(
            kinds,
            vec![
                RawTokenKind::IntegerLiteral,
                RawTokenKind::Identifier,
                RawTokenKind::IntegerLiteral,
                RawTokenKind::Eof,
            ]
        );
        assert!(!diagnostics.is_empty());
    }

    #[test]
    fn recognizes_character_literals_and_unicode_escapes() {
        let (items, diagnostics) = scan(r#"'a' '\n' '\t' '\'' '\\' '\u0041'"#);
        let kinds: Vec<_> = items
            .into_iter()
            .filter_map(|item| match item {
                RawItem::Token(token) => Some(token.kind),
                RawItem::Trivia(_) => None,
            })
            .collect();

        assert_eq!(
            kinds,
            vec![
                RawTokenKind::CharLiteral,
                RawTokenKind::CharLiteral,
                RawTokenKind::CharLiteral,
                RawTokenKind::CharLiteral,
                RawTokenKind::CharLiteral,
                RawTokenKind::CharLiteral,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn diagnoses_empty_invalid_and_unterminated_character_literals() {
        let (items, diagnostics) = scan("'' 'ab' '\\q' '");
        let error_count = items
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    RawItem::Token(RawToken {
                        kind: RawTokenKind::Error,
                        ..
                    })
                )
            })
            .count();

        assert_eq!(error_count, 4);
        assert_eq!(diagnostics.len(), 4);
    }

    #[test]
    fn recognizes_ordinary_and_multiline_strings() {
        let (items, diagnostics) = scan("\"hello\\nworld\" \"\"\"hello\nworld\"\"\"");
        let kinds: Vec<_> = items
            .into_iter()
            .filter_map(|item| match item {
                RawItem::Token(token) => Some(token.kind),
                RawItem::Trivia(_) => None,
            })
            .collect();

        assert_eq!(
            kinds,
            vec![
                RawTokenKind::StringLiteral,
                RawTokenKind::StringLiteral,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn keeps_multiline_string_backslashes_raw() {
        let (items, diagnostics) = scan("\"\"\"\\n\"\"\"");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::StringLiteral, 0, 8),
                token(RawTokenKind::Eof, 8, 8),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn diagnoses_invalid_escapes_and_unclosed_strings() {
        let (items, diagnostics) = scan("\"bad\\q\" \"unclosed\nnext");
        let error_count = items
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    RawItem::Token(RawToken {
                        kind: RawTokenKind::Error,
                        ..
                    })
                )
            })
            .count();

        assert_eq!(error_count, 1);
        assert_eq!(diagnostics.len(), 2);
    }
}
