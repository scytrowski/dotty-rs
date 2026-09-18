use core::fmt;

use dotty_diagnostics::{Diagnostic, DiagnosticSeverity};
use dotty_source::{SourceText, SourceTextError, TextRange, TextRangeError};

use crate::identifier::{is_identifier_part, is_identifier_start, is_operator_character};
use crate::xml::XmlState;
use crate::{Cursor, CursorError, HardKeyword, Punctuation, RawItem, RawToken, RawTokenKind};
use crate::{Trivia, TriviaKind};

#[derive(Debug, Clone)]
enum LexMode {
    Normal,
    InterpolatedString(StringState),
    InterpolationExpression { brace_depth: u32 },
    SimpleSplice,
}

#[derive(Debug, Clone, Copy)]
struct StringState {
    part_start: u32,
    multiline: bool,
    started: bool,
}

/// The first-stage raw lexer for Scala source text.
#[derive(Debug)]
pub struct RawLexer<'source> {
    source: SourceText<'source>,
    cursor: Cursor<'source>,
    diagnostics: Vec<Diagnostic>,
    emitted_eof: bool,
    modes: Vec<LexMode>,
    pending: Option<RawItem>,
    xml: XmlState,
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
            modes: vec![LexMode::Normal],
            pending: None,
            xml: XmlState::default(),
        })
    }

    /// Returns diagnostics collected while recovering from malformed input.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Emits the next raw token or trivia item.
    pub fn next(&mut self) -> Result<Option<RawItem>, RawLexerError> {
        if let Some(item) = self.pending.take() {
            return Ok(Some(item));
        }

        match self.modes.last().cloned() {
            Some(LexMode::Normal) => self.next_normal(),
            Some(LexMode::InterpolatedString(_)) => self.next_string_part(),
            Some(LexMode::InterpolationExpression { .. }) => self.next_expression(),
            Some(LexMode::SimpleSplice) => self.next_simple_splice(),
            None => Ok(None),
        }
    }

    fn next_normal(&mut self) -> Result<Option<RawItem>, RawLexerError> {
        if self.emitted_eof {
            return Ok(None);
        }

        let start = self.cursor.position();
        let Some(character) = self.cursor.peek() else {
            if let Some(message) = self.xml.eof_message() {
                self.report(start, message)?;
            }
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
            let token = self.scan_number(start)?;
            self.update_xml_token(token.kind, token.span)?;
            return Ok(Some(RawItem::Token(token)));
        }
        if character == '\'' {
            if matches!(self.cursor.peek_nth(1), Some('{' | '[')) {
                return Ok(Some(RawItem::Token(self.scan_quote(start)?)));
            }
            if self.looks_like_quote_id() {
                return Ok(Some(RawItem::Token(self.scan_quote_id(start)?)));
            }
            if !self.looks_like_char_literal() {
                let _ = self.cursor.bump();
                return Ok(Some(RawItem::Token(RawToken {
                    kind: RawTokenKind::Quote,
                    span: self.span(start)?,
                })));
            }
            let token = self.scan_char_literal(start)?;
            self.update_xml_token(token.kind, token.span)?;
            return Ok(Some(RawItem::Token(token)));
        }
        if character == '"' {
            let token = self.scan_string_literal(start)?;
            self.update_xml_token(token.kind, token.span)?;
            return Ok(Some(RawItem::Token(token)));
        }
        if self.xml.can_start_literal()
            && character == '<'
            && self
                .cursor
                .peek_nth(1)
                .is_some_and(crate::identifier::is_identifier_start)
        {
            let _ = self.cursor.bump();
            let span = self.span(start)?;
            self.update_xml_token(RawTokenKind::XmlStart, span)?;
            return Ok(Some(RawItem::Token(RawToken {
                kind: RawTokenKind::XmlStart,
                span,
            })));
        }
        if is_identifier_start(character) {
            return Ok(Some(RawItem::Token(self.scan_identifier(start)?)));
        }
        if character == '`' {
            return Ok(Some(RawItem::Token(
                self.scan_backquoted_identifier(start)?,
            )));
        }
        if character == ':' && self.cursor.peek_nth(1).is_some_and(is_operator_character) {
            return Ok(Some(RawItem::Token(self.scan_operator(start)?)));
        }
        if character == ':' && self.xml.is_xml_name_separator() {
            let _ = self.cursor.bump();
            let kind = RawTokenKind::Operator;
            let span = self.span(start)?;
            self.update_xml_token(kind, span)?;
            return Ok(Some(RawItem::Token(RawToken { kind, span })));
        }
        if let Some(punctuation) = punctuation(character) {
            let _ = self.cursor.bump();
            let kind = RawTokenKind::Punctuation(punctuation);
            let span = self.span(start)?;
            self.update_xml_token(kind, span)?;
            return Ok(Some(RawItem::Token(RawToken { kind, span })));
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

    fn next_expression(&mut self) -> Result<Option<RawItem>, RawLexerError> {
        if self.cursor.is_eof() {
            let start = self.cursor.position();
            self.report(start, "unterminated interpolation expression")?;
            let _ = self.modes.pop();
            return self.next();
        }

        if self.cursor.peek() == Some('}')
            && matches!(
                self.modes.last(),
                Some(LexMode::InterpolationExpression { brace_depth: 1 })
            )
        {
            let start = self.cursor.position();
            let _ = self.cursor.bump();
            let _ = self.modes.pop();
            self.resume_string_after_expression();
            return Ok(Some(RawItem::Token(RawToken {
                kind: RawTokenKind::Punctuation(Punctuation::RightBrace),
                span: self.span(start)?,
            })));
        }

        let Some(item) = self.next_normal()? else {
            return Ok(None);
        };
        if let RawItem::Token(token) = &item {
            match token.kind {
                RawTokenKind::Punctuation(Punctuation::LeftBrace) => {
                    if let Some(LexMode::InterpolationExpression { brace_depth }) = self
                        .modes
                        .iter_mut()
                        .rev()
                        .find(|mode| matches!(mode, LexMode::InterpolationExpression { .. }))
                    {
                        *brace_depth = brace_depth.saturating_add(1);
                    }
                }
                RawTokenKind::Punctuation(Punctuation::RightBrace) => {
                    if let Some(LexMode::InterpolationExpression { brace_depth }) = self
                        .modes
                        .iter_mut()
                        .rev()
                        .find(|mode| matches!(mode, LexMode::InterpolationExpression { .. }))
                    {
                        *brace_depth = brace_depth.saturating_sub(1);
                    }
                }
                _ => {}
            }
        }

        Ok(Some(item))
    }

    fn next_string_part(&mut self) -> Result<Option<RawItem>, RawLexerError> {
        let Some(LexMode::InterpolatedString(state)) = self.modes.last().cloned() else {
            return Ok(None);
        };

        if !state.started {
            let _ = self.cursor.bump();
            if state.multiline {
                let _ = self.cursor.bump();
                let _ = self.cursor.bump();
            }
            if let Some(LexMode::InterpolatedString(current)) = self.modes.last_mut() {
                current.started = true;
            }
        }

        loop {
            if self.cursor.is_eof() {
                let start = state.part_start;
                let _ = self.modes.pop();
                self.report(start, "unclosed interpolated string literal")?;
                return Ok(Some(RawItem::Token(RawToken {
                    kind: RawTokenKind::Error,
                    span: self.span(start)?,
                })));
            }

            if !state.multiline && matches!(self.cursor.peek(), Some('\n' | '\r')) {
                let start = state.part_start;
                let _ = self.modes.pop();
                self.report(start, "unclosed interpolated string literal")?;
                return Ok(Some(RawItem::Token(RawToken {
                    kind: RawTokenKind::Error,
                    span: self.span(start)?,
                })));
            }

            if (state.multiline
                && self.cursor.peek() == Some('"')
                && self.cursor.peek_nth(1) == Some('"')
                && self.cursor.peek_nth(2) == Some('"'))
                || (!state.multiline && self.cursor.peek() == Some('"'))
            {
                if state.multiline {
                    self.consume_multiline_terminator();
                } else {
                    let _ = self.cursor.bump();
                }
                let _ = self.modes.pop();
                return Ok(Some(RawItem::Token(RawToken {
                    kind: RawTokenKind::StringPart,
                    span: self.span(state.part_start)?,
                })));
            }

            if self.cursor.peek() == Some('$') {
                match self.cursor.peek_nth(1) {
                    Some('$' | '"') => {
                        let _ = self.cursor.bump();
                        let _ = self.cursor.bump();
                    }
                    Some('{') => {
                        let _ = self.cursor.bump();
                        let brace_start = self.cursor.position();
                        let _ = self.cursor.bump();
                        self.modes
                            .push(LexMode::InterpolationExpression { brace_depth: 1 });
                        self.pending = Some(RawItem::Token(RawToken {
                            kind: RawTokenKind::Punctuation(Punctuation::LeftBrace),
                            span: self.span(brace_start)?,
                        }));
                        return Ok(Some(RawItem::Token(RawToken {
                            kind: RawTokenKind::StringPart,
                            span: self.span(state.part_start)?,
                        })));
                    }
                    Some(character) if is_identifier_start(character) => {
                        let _ = self.cursor.bump();
                        self.modes.push(LexMode::SimpleSplice);
                        return Ok(Some(RawItem::Token(RawToken {
                            kind: RawTokenKind::StringPart,
                            span: self.span(state.part_start)?,
                        })));
                    }
                    _ => {
                        let _ = self.cursor.bump();
                        if self.cursor.is_eof()
                            || (!state.multiline && matches!(self.cursor.peek(), Some('\n' | '\r')))
                        {
                            return self
                                .recover_interpolated_string(state.part_start, state.multiline);
                        }
                        self.report(state.part_start, "invalid string interpolation splice")?;
                        return self.recover_interpolated_string(state.part_start, state.multiline);
                    }
                }
                continue;
            }

            if !state.multiline && self.cursor.peek() == Some('\\') {
                let _ = self.scan_escape(state.part_start)?;
            } else {
                let _ = self.cursor.bump();
            }
        }
    }

    fn recover_interpolated_string(
        &mut self,
        start: u32,
        multiline: bool,
    ) -> Result<Option<RawItem>, RawLexerError> {
        loop {
            if self.cursor.is_eof() {
                let _ = self.modes.pop();
                self.report(start, "unclosed interpolated string literal")?;
                return Ok(Some(RawItem::Token(RawToken {
                    kind: RawTokenKind::Error,
                    span: self.span(start)?,
                })));
            }

            if (multiline
                && self.cursor.peek() == Some('"')
                && self.cursor.peek_nth(1) == Some('"')
                && self.cursor.peek_nth(2) == Some('"'))
                || (!multiline && self.cursor.peek() == Some('"'))
            {
                if multiline {
                    self.consume_multiline_terminator();
                } else {
                    let _ = self.cursor.bump();
                }
                let _ = self.modes.pop();
                return Ok(Some(RawItem::Token(RawToken {
                    kind: RawTokenKind::StringLiteral,
                    span: self.span(start)?,
                })));
            }

            if !multiline && matches!(self.cursor.peek(), Some('\n' | '\r')) {
                let _ = self.modes.pop();
                self.report(start, "unclosed interpolated string literal")?;
                return Ok(Some(RawItem::Token(RawToken {
                    kind: RawTokenKind::Error,
                    span: self.span(start)?,
                })));
            }

            if !multiline && self.cursor.peek() == Some('\\') {
                let _ = self.scan_escape(start)?;
            } else {
                let _ = self.cursor.bump();
            }
        }
    }

    fn next_simple_splice(&mut self) -> Result<Option<RawItem>, RawLexerError> {
        let start = self.cursor.position();
        if !self.cursor.peek().is_some_and(is_identifier_start) {
            let _ = self.modes.pop();
            self.report(start, "interpolation identifier expected after `$`")?;
            return Ok(Some(RawItem::Token(RawToken {
                kind: RawTokenKind::Error,
                span: self.span(start)?,
            })));
        }

        let token = self.scan_identifier_token(start, false)?;
        let _ = self.modes.pop();
        self.resume_string_after_expression();
        Ok(Some(RawItem::Token(token)))
    }

    fn resume_string_after_expression(&mut self) {
        if let Some(LexMode::InterpolatedString(state)) = self.modes.last_mut() {
            state.part_start = self.cursor.position();
        }
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
        let token = self.scan_identifier_token(start, true)?;
        self.update_xml_token(token.kind, token.span)?;
        Ok(token)
    }

    fn scan_identifier_token(
        &mut self,
        start: u32,
        allow_interpolation: bool,
    ) -> Result<RawToken, RawLexerError> {
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
        if allow_interpolation && self.cursor.peek() == Some('"') {
            let multiline =
                self.cursor.peek_nth(1) == Some('"') && self.cursor.peek_nth(2) == Some('"');
            self.modes.push(LexMode::InterpolatedString(StringState {
                part_start: self.cursor.position(),
                multiline,
                started: false,
            }));
            return Ok(RawToken {
                kind: RawTokenKind::InterpolationId,
                span,
            });
        }
        let kind = if !allow_interpolation {
            RawTokenKind::Identifier
        } else {
            classify_keyword(text)
                .map(RawTokenKind::Keyword)
                .unwrap_or(RawTokenKind::Identifier)
        };

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

        let token = RawToken {
            kind: RawTokenKind::Operator,
            span: self.span(start)?,
        };
        self.update_xml_token(token.kind, token.span)?;
        Ok(token)
    }

    fn update_xml_token(
        &mut self,
        kind: RawTokenKind,
        span: TextRange,
    ) -> Result<(), RawLexerError> {
        let spelling = self.source.slice(span)?;
        self.xml.update_token(kind, spelling);
        if let Some(message) = self.xml.take_error() {
            self.report(span.start(), message)?;
        }
        Ok(())
    }

    fn looks_like_quote_id(&self) -> bool {
        if !self
            .cursor
            .peek_nth(1)
            .is_some_and(crate::identifier::is_identifier_start)
        {
            return false;
        }

        let mut lookahead = 2;
        while self
            .cursor
            .peek_nth(lookahead)
            .is_some_and(crate::identifier::is_identifier_part)
        {
            lookahead += 1;
        }

        let has_additional_identifier_start = (2..lookahead)
            .any(|index| self.cursor.peek_nth(index).is_some_and(is_identifier_start));
        self.cursor.peek_nth(lookahead) != Some('\'') || has_additional_identifier_start
    }

    fn looks_like_char_literal(&self) -> bool {
        match self.cursor.peek_nth(1) {
            Some('\\' | '\'') => true,
            Some(character) if is_identifier_start(character) => true,
            Some('\n' | '\r') => true,
            Some(character) if character.is_whitespace() => false,
            Some(character) if is_operator_character(character) => {
                self.cursor.peek_nth(2) == Some('\'')
            }
            Some(_) => self.cursor.peek_nth(2) == Some('\''),
            None => true,
        }
    }

    fn scan_quote(&mut self, start: u32) -> Result<RawToken, RawLexerError> {
        let _ = self.cursor.bump();
        Ok(RawToken {
            kind: RawTokenKind::Quote,
            span: self.span(start)?,
        })
    }

    fn scan_quote_id(&mut self, start: u32) -> Result<RawToken, RawLexerError> {
        let _ = self.cursor.bump();
        let _ = self.cursor.bump();
        while self
            .cursor
            .peek()
            .is_some_and(crate::identifier::is_identifier_part)
        {
            let _ = self.cursor.bump();
        }
        Ok(RawToken {
            kind: RawTokenKind::QuoteId,
            span: self.span(start)?,
        })
    }

    fn scan_number(&mut self, start: u32) -> Result<RawToken, RawLexerError> {
        let mut kind = RawTokenKind::IntegerLiteral;
        let mut base = 10;
        let mut invalid_suffix = false;

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
                kind = RawTokenKind::Error;
                invalid_suffix = true;
            }
            _ => {}
        }

        let invalid_non_decimal_digit = base != 10
            && self
                .cursor
                .peek()
                .is_some_and(|character| character.is_ascii_alphanumeric());
        if self.cursor.peek().is_some_and(is_identifier_part)
            && !invalid_non_decimal_digit
            && !invalid_suffix
        {
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
        let invalid_non_decimal_digit = base != 10
            && self.cursor.peek().is_some_and(|character| {
                character.is_ascii_alphanumeric() && !(saw_digit && matches!(character, 'l' | 'L'))
            });
        if invalid_non_decimal_digit {
            self.report(start, "invalid digit in non-decimal literal")?;
        } else if !saw_digit {
            self.report(start, "numeric literal must contain a digit")?;
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
                self.consume_multiline_terminator();
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

    fn consume_multiline_terminator(&mut self) {
        while self.cursor.peek() == Some('"') {
            let _ = self.cursor.bump();
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
        Ok(true)
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
    use dotty_token::TokenKind;

    use super::*;

    fn visit_short_ascii_inputs(
        alphabet: &[char],
        current: &mut String,
        remaining: usize,
        visit: &mut impl FnMut(&str),
    ) {
        visit(current);
        if remaining == 0 {
            return;
        }
        for character in alphabet {
            current.push(*character);
            visit_short_ascii_inputs(alphabet, current, remaining - 1, visit);
            current.pop();
        }
    }

    fn visit_short_unicode_inputs(
        alphabet: &[char],
        current: &mut String,
        remaining: usize,
        visit: &mut impl FnMut(&str),
    ) {
        visit(current);
        if remaining == 0 {
            return;
        }
        for character in alphabet {
            current.push(*character);
            visit_short_unicode_inputs(alphabet, current, remaining - 1, visit);
            current.pop();
        }
    }

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
    fn accepts_combining_marks_in_identifier_parts() {
        let source = "val a\u{0301} = 1";
        let (items, diagnostics) = scan(source);

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::Keyword(HardKeyword::Val), 0, 3),
                trivia(TriviaKind::Spaces, 3, 4),
                token(RawTokenKind::Identifier, 4, 7),
                trivia(TriviaKind::Spaces, 7, 8),
                token(RawTokenKind::Operator, 8, 9),
                trivia(TriviaKind::Spaces, 9, 10),
                token(RawTokenKind::IntegerLiteral, 10, 11),
                token(RawTokenKind::Eof, 11, 11),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn accepts_supplementary_unicode_identifier_starts() {
        let source = "val 𐐀 = 1";
        let (items, diagnostics) = scan(source);

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::Keyword(HardKeyword::Val), 0, 3),
                trivia(TriviaKind::Spaces, 3, 4),
                token(RawTokenKind::Identifier, 4, 8),
                trivia(TriviaKind::Spaces, 8, 9),
                token(RawTokenKind::Operator, 9, 10),
                trivia(TriviaKind::Spaces, 10, 11),
                token(RawTokenKind::IntegerLiteral, 11, 12),
                token(RawTokenKind::Eof, 12, 12),
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
    fn keeps_soft_modifier_words_as_identifiers() {
        let (items, diagnostics) = scan("using extension inline");
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
                RawTokenKind::Identifier,
                RawTokenKind::Identifier,
                RawTokenKind::Identifier,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn keeps_soft_type_modifier_words_as_identifiers() {
        let (items, diagnostics) = scan("opaque open transparent");
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
                RawTokenKind::Identifier,
                RawTokenKind::Identifier,
                RawTokenKind::Identifier,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn keeps_soft_context_words_as_identifiers() {
        let (items, diagnostics) = scan("as derives infix");
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
                RawTokenKind::Identifier,
                RawTokenKind::Identifier,
                RawTokenKind::Identifier,
                RawTokenKind::Eof,
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
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn emits_a_supplementary_unicode_symbol_as_an_operator() {
        let (items, diagnostics) = scan("left 🂡 right");
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
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn emits_a_non_ascii_symbol_as_an_operator() {
        let (items, diagnostics) = scan("left © right");
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
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
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
    fn preserves_form_feed_as_other_whitespace() {
        let (items, diagnostics) = scan("a\u{000c}b");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::Identifier, 0, 1),
                trivia(TriviaKind::OtherWhitespace, 1, 2),
                token(RawTokenKind::Identifier, 2, 3),
                token(RawTokenKind::Eof, 3, 3),
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
    fn recognizes_case_variants_of_numeric_suffixes_and_exponent_separators() {
        let (items, diagnostics) = scan("1l 1F 1D 1.0e1_0");
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
                RawTokenKind::LongLiteral,
                RawTokenKind::FloatLiteral,
                RawTokenKind::DoubleLiteral,
                RawTokenKind::ExponentLiteral,
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
    fn reports_missing_exponent_digits_and_keeps_the_numeric_token() {
        let (items, diagnostics) = scan("1e+ next");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::ExponentLiteral, 0, 3),
                trivia(TriviaKind::Spaces, 3, 4),
                token(RawTokenKind::Identifier, 4, 8),
                token(RawTokenKind::Eof, 8, 8),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(0, 3).expect("valid range")
        );
        assert!(diagnostics[0].message().contains("exponent"));
    }

    #[test]
    fn reports_missing_digits_after_hex_and_binary_prefixes() {
        let (items, diagnostics) = scan("0x 0b");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::IntegerLiteral, 0, 2),
                trivia(TriviaKind::Spaces, 2, 3),
                token(RawTokenKind::IntegerLiteral, 3, 5),
                token(RawTokenKind::Eof, 5, 5),
            ]
        );
        assert_eq!(diagnostics.len(), 2);
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.message().contains("digit"))
        );
    }

    #[test]
    fn reports_invalid_digits_after_hex_and_binary_prefixes() {
        let (items, diagnostics) = scan("0xg 0b2");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::IntegerLiteral, 0, 2),
                token(RawTokenKind::Identifier, 2, 3),
                trivia(TriviaKind::Spaces, 3, 4),
                token(RawTokenKind::IntegerLiteral, 4, 6),
                token(RawTokenKind::IntegerLiteral, 6, 7),
                token(RawTokenKind::Eof, 7, 7),
            ]
        );
        assert_eq!(diagnostics.len(), 2);
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.message().contains("invalid digit"))
        );
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
    fn recognizes_float_suffix_after_an_exponent() {
        let (items, diagnostics) = scan("1e2f");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::FloatLiteral, 0, 4),
                token(RawTokenKind::Eof, 4, 4),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn recognizes_double_suffix_after_an_exponent() {
        let (items, diagnostics) = scan("1e2d");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::DoubleLiteral, 0, 4),
                token(RawTokenKind::Eof, 4, 4),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn recognizes_a_long_suffix_after_an_uppercase_hex_literal() {
        let (items, diagnostics) = scan("0XFFL");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::LongLiteral, 0, 5),
                token(RawTokenKind::Eof, 5, 5),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn reports_a_long_suffix_on_a_decimal_literal_as_an_error_token() {
        let (items, diagnostics) = scan("1.0L");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::Error, 0, 3),
                token(RawTokenKind::Identifier, 3, 4),
                token(RawTokenKind::Eof, 4, 4),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message().contains("long suffix"));
    }

    #[test]
    fn reports_a_long_suffix_on_an_exponent_literal_as_an_error_token() {
        let (items, diagnostics) = scan("1e2L");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::Error, 0, 3),
                token(RawTokenKind::Identifier, 3, 4),
                token(RawTokenKind::Eof, 4, 4),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message().contains("long suffix"));
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
    fn accepts_repeated_unicode_escape_prefixes_in_strings() {
        let source = r#""\uuuu0041""#;
        let (items, diagnostics) = scan(source);

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::StringLiteral, 0, source.len() as u32),
                token(RawTokenKind::Eof, source.len() as u32, source.len() as u32),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn accepts_uppercase_unicode_escape_prefixes_in_strings() {
        let source = r#""\U0041""#;
        let (items, diagnostics) = scan(source);

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::StringLiteral, 0, source.len() as u32),
                token(RawTokenKind::Eof, source.len() as u32, source.len() as u32),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn recognizes_term_and_type_quote_markers() {
        let (items, diagnostics) = scan("'{ '[List[Int]]");
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
                RawTokenKind::Quote,
                RawTokenKind::Punctuation(Punctuation::LeftBrace),
                RawTokenKind::Quote,
                RawTokenKind::Punctuation(Punctuation::LeftBracket),
                RawTokenKind::Identifier,
                RawTokenKind::Punctuation(Punctuation::LeftBracket),
                RawTokenKind::Identifier,
                RawTokenKind::Punctuation(Punctuation::RightBracket),
                RawTokenKind::Punctuation(Punctuation::RightBracket),
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn recognizes_a_legacy_quoted_identifier() {
        let (items, diagnostics) = scan("'foo");
        let kinds: Vec<_> = items
            .into_iter()
            .filter_map(|item| match item {
                RawItem::Token(token) => Some(token.kind),
                RawItem::Trivia(_) => None,
            })
            .collect();

        assert_eq!(kinds, vec![RawTokenKind::QuoteId, RawTokenKind::Eof]);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn keeps_a_legacy_quoted_identifier_bounded_by_a_line_break() {
        let (items, diagnostics) = scan("'foo\nbar");
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
                RawTokenKind::QuoteId,
                RawTokenKind::Identifier,
                RawTokenKind::Eof
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn recognizes_multiple_legacy_quoted_identifiers_on_one_line() {
        let (items, diagnostics) = scan("'foo + 'bar");
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
                RawTokenKind::QuoteId,
                RawTokenKind::Operator,
                RawTokenKind::QuoteId,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn recognizes_a_legacy_quoted_identifier_before_a_character_literal() {
        let (items, diagnostics) = scan("'foo 'a'");
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
                RawTokenKind::QuoteId,
                RawTokenKind::CharLiteral,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn recognizes_a_legacy_quoted_identifier_with_a_trailing_quote() {
        let (items, diagnostics) = scan("'name'");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::QuoteId, 0, 5),
                token(RawTokenKind::Error, 5, 6),
                token(RawTokenKind::Eof, 6, 6),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].span(), TextRange::new(5, 6).unwrap());
        assert!(diagnostics[0].message().contains("unterminated character"));
    }

    #[test]
    fn recognizes_a_bare_quote_after_an_identifier() {
        let (items, diagnostics) = scan("x' = 1");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::Identifier, 0, 1),
                token(RawTokenKind::Quote, 1, 2),
                trivia(TriviaKind::Spaces, 2, 3),
                token(RawTokenKind::Operator, 3, 4),
                trivia(TriviaKind::Spaces, 4, 5),
                token(RawTokenKind::IntegerLiteral, 5, 6),
                token(RawTokenKind::Eof, 6, 6),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn recovers_an_unclosed_character_literal_after_an_identifier_at_line_end() {
        let source = "value'\nnext";
        let (items, diagnostics) = scan(source);

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::Identifier, 0, 5),
                token(RawTokenKind::Error, 5, 6),
                trivia(TriviaKind::Newline, 6, 7),
                token(RawTokenKind::Identifier, 7, 11),
                token(RawTokenKind::Eof, 11, 11),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].span(), TextRange::new(5, 6).unwrap());
        assert!(diagnostics[0].message().contains("unterminated character"));
    }

    #[test]
    fn recovers_an_unclosed_character_literal_after_an_operator_at_line_end() {
        let source = "+'\nnext";
        let (items, diagnostics) = scan(source);

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::Operator, 0, 1),
                token(RawTokenKind::Error, 1, 2),
                trivia(TriviaKind::Newline, 2, 3),
                token(RawTokenKind::Identifier, 3, 7),
                token(RawTokenKind::Eof, 7, 7),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].span(), TextRange::new(1, 2).unwrap());
        assert!(diagnostics[0].message().contains("unterminated character"));
    }

    #[test]
    fn recovers_a_trailing_quote_after_a_legacy_identifier_at_line_end() {
        let source = "'name'\nnext";
        let (items, diagnostics) = scan(source);

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::QuoteId, 0, 5),
                token(RawTokenKind::Error, 5, 6),
                trivia(TriviaKind::Newline, 6, 7),
                token(RawTokenKind::Identifier, 7, 11),
                token(RawTokenKind::Eof, 11, 11),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].span(), TextRange::new(5, 6).unwrap());
        assert!(diagnostics[0].message().contains("unterminated character"));
    }

    #[test]
    fn keeps_splice_syntax_as_identifier_and_brace_tokens() {
        let (items, diagnostics) = scan("${value}");
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
                RawTokenKind::Identifier,
                RawTokenKind::Punctuation(Punctuation::LeftBrace),
                RawTokenKind::Identifier,
                RawTokenKind::Punctuation(Punctuation::RightBrace),
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn recognizes_xml_start_before_a_closed_tag() {
        let (items, diagnostics) = scan("<tag></tag>");
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
                RawTokenKind::XmlStart,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Eof
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn accepts_matching_nested_xml_tag_names() {
        let (_, diagnostics) = scan("<root><child/></root>");

        assert!(diagnostics.is_empty());
    }

    #[test]
    fn diagnoses_a_mismatched_xml_closing_tag_name() {
        let source = "<root></wrong>";
        let (_, diagnostics) = scan(source);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].message(),
            "mismatched XML closing tag: expected </root>, found </wrong>"
        );
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new((source.len() - 1) as u32, source.len() as u32).expect("valid range")
        );
    }

    #[test]
    fn diagnoses_an_xml_closing_tag_without_a_name() {
        let source = "<root></>";
        let (_, diagnostics) = scan(source);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message(), "XML closing tag name expected");
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new((source.len() - 4) as u32, source.len() as u32).expect("valid range")
        );
    }

    #[test]
    fn diagnoses_an_attribute_name_on_an_xml_closing_tag() {
        let source = "<root></root id>";
        let (_, diagnostics) = scan(source);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].message(),
            "XML closing tag cannot contain attributes"
        );
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(13, 15).expect("valid range")
        );
    }

    #[test]
    fn diagnoses_a_string_value_on_an_xml_closing_tag() {
        let source = r#"<root></root "value">"#;
        let (_, diagnostics) = scan(source);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].message(),
            "XML closing tag cannot contain attributes"
        );
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(13, 20).expect("valid range")
        );
    }

    #[test]
    fn accepts_hard_keywords_as_xml_tag_names() {
        let (_, diagnostics) = scan("<if></if>");

        assert!(diagnostics.is_empty());
    }

    #[test]
    fn accepts_hyphenated_xml_tag_names() {
        let (_, diagnostics) = scan("<data-item></data-item>");

        assert!(diagnostics.is_empty());
    }

    #[test]
    fn accepts_namespaced_xml_attribute_names() {
        let (_, diagnostics) = scan(r#"<item xml:lang="en"/>"#);

        assert!(diagnostics.is_empty());
    }

    #[test]
    fn emits_xml_namespace_separators_as_operators() {
        let (items, diagnostics) = scan(r#"<ns:item xml:lang="en"/>"#);
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
                RawTokenKind::XmlStart,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::StringLiteral,
                RawTokenKind::Operator,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn accepts_hyphenated_xml_attribute_names() {
        let (_, diagnostics) = scan("<item data-id={id}/>");

        assert!(diagnostics.is_empty());
    }

    #[test]
    fn accepts_a_string_literal_inside_an_expression_xml_attribute_value() {
        let (_, diagnostics) = scan(r#"<item title={"hello"}/>"#);

        assert!(diagnostics.is_empty());
    }

    #[test]
    fn accepts_a_character_literal_inside_an_expression_xml_attribute_value() {
        let (_, diagnostics) = scan("<item marker={'x'}/>");

        assert!(diagnostics.is_empty());
    }

    #[test]
    fn accepts_a_number_inside_an_expression_xml_attribute_value() {
        let (_, diagnostics) = scan("<item count={42}/>");

        assert!(diagnostics.is_empty());
    }

    #[test]
    fn diagnoses_an_unterminated_simple_xml_tag_at_eof() {
        let source = "<item";
        let (_, diagnostics) = scan(source);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message(), "unterminated XML tag");
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(source.len() as u32, source.len() as u32).expect("valid range")
        );
    }

    #[test]
    fn diagnoses_an_unterminated_xml_closing_tag_at_eof() {
        let source = "<item></item";
        let (_, diagnostics) = scan(source);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message(), "unterminated XML tag");
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(source.len() as u32, source.len() as u32).expect("valid range")
        );
    }

    #[test]
    fn diagnoses_an_unterminated_xml_closing_tag_without_a_name_at_eof() {
        let source = "<item></";
        let (_, diagnostics) = scan(source);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message(), "unterminated XML tag");
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(source.len() as u32, source.len() as u32).expect("valid range")
        );
    }

    #[test]
    fn diagnoses_an_unterminated_xml_attribute_expression_at_eof() {
        let source = "<item enabled={flag";
        let (_, diagnostics) = scan(source);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].message(),
            "XML attribute expression must be closed"
        );
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(source.len() as u32, source.len() as u32).expect("valid range")
        );
    }

    #[test]
    fn diagnoses_an_unterminated_nested_xml_attribute_expression_at_eof() {
        let source = "<item enabled={flag {nested}";
        let (_, diagnostics) = scan(source);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].message(),
            "XML attribute expression must be closed"
        );
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(source.len() as u32, source.len() as u32).expect("valid range")
        );
    }

    #[test]
    fn diagnoses_an_unterminated_empty_xml_element_at_eof() {
        let source = "<item>";
        let (_, diagnostics) = scan(source);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message(), "unterminated XML tag");
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(source.len() as u32, source.len() as u32).expect("valid range")
        );
    }

    #[test]
    fn diagnoses_an_unterminated_xml_element_with_text_at_eof() {
        let source = "<item>text";
        let (_, diagnostics) = scan(source);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message(), "unterminated XML tag");
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(source.len() as u32, source.len() as u32).expect("valid range")
        );
    }

    #[test]
    fn diagnoses_an_unterminated_nested_xml_element_at_eof() {
        let source = "<root><child/>";
        let (_, diagnostics) = scan(source);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message(), "unterminated XML tag");
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(source.len() as u32, source.len() as u32).expect("valid range")
        );
    }

    #[test]
    fn keeps_comparison_operators_inside_xml_expressions() {
        let (items, diagnostics) = scan("<root>{a < b}</root>");
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
                RawTokenKind::XmlStart,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Punctuation(Punctuation::LeftBrace),
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Punctuation(Punctuation::RightBrace),
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn recognizes_nested_xml_inside_an_xml_expression() {
        let (items, diagnostics) = scan("<root>{<inner/>}</root>");
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
                RawTokenKind::XmlStart,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Punctuation(Punctuation::LeftBrace),
                RawTokenKind::XmlStart,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Punctuation(Punctuation::RightBrace),
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn diagnoses_an_unterminated_xml_comment_at_eof() {
        let source = "<root><!--";
        let (_, diagnostics) = scan(source);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message(), "unterminated XML comment");
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(source.len() as u32, source.len() as u32).expect("valid range")
        );
    }

    #[test]
    fn diagnoses_an_unterminated_xml_cdata_section_at_eof() {
        let source = "<root><![CDATA[text";
        let (_, diagnostics) = scan(source);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message(), "unterminated XML CDATA section");
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(source.len() as u32, source.len() as u32).expect("valid range")
        );
    }

    #[test]
    fn diagnoses_an_unterminated_xml_expression_at_eof() {
        let source = "<root>{flag";
        let (_, diagnostics) = scan(source);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message(), "unterminated XML expression");
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(source.len() as u32, source.len() as u32).expect("valid range")
        );
    }

    #[test]
    fn emits_xml_attribute_tokens_with_exact_spans() {
        let source = r#"<item id="x" enabled={flag}>text</item>"#;
        let (items, diagnostics) = scan(source);
        let tokens: Vec<_> = items
            .into_iter()
            .filter_map(|item| match item {
                RawItem::Token(token) => Some(token),
                RawItem::Trivia(_) => None,
            })
            .collect();

        assert_eq!(
            tokens.iter().map(|token| token.kind).collect::<Vec<_>>(),
            vec![
                RawTokenKind::XmlStart,
                RawTokenKind::Identifier,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::StringLiteral,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Punctuation(Punctuation::LeftBrace),
                RawTokenKind::Identifier,
                RawTokenKind::Punctuation(Punctuation::RightBrace),
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Eof,
            ]
        );
        assert_eq!(
            tokens
                .iter()
                .map(|token| &source[token.span.start() as usize..token.span.end() as usize])
                .collect::<Vec<_>>(),
            vec![
                "<", "item", "id", "=", "\"x\"", "enabled", "=", "{", "flag", "}", ">", "text",
                "</", "item", ">", ""
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn accepts_a_quoted_xml_attribute_value() {
        let (_, diagnostics) = scan(r#"<item id="x"/>"#);

        assert!(diagnostics.is_empty());
    }

    #[test]
    fn accepts_an_expression_xml_attribute_value() {
        let (_, diagnostics) = scan("<item enabled={flag}/>");

        assert!(diagnostics.is_empty());
    }

    #[test]
    fn accepts_multiple_xml_attributes() {
        let (_, diagnostics) = scan(r#"<item id="x" enabled={flag}/>"#);

        assert!(diagnostics.is_empty());
    }

    #[test]
    fn diagnoses_an_xml_attribute_without_equals() {
        let (_, diagnostics) = scan("<item id/>");

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].message(),
            "XML attribute name must be followed by `=`"
        );
    }

    #[test]
    fn diagnoses_an_xml_attribute_without_a_value() {
        let (_, diagnostics) = scan("<item id=/>");

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].message(),
            "XML attribute value expected after `=`"
        );
    }

    #[test]
    fn diagnoses_a_missing_xml_attribute_equals_at_eof() {
        let (_, diagnostics) = scan("<item id");

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].message(),
            "XML attribute name must be followed by `=`"
        );
    }

    #[test]
    fn diagnoses_a_missing_xml_attribute_value_at_eof() {
        let (_, diagnostics) = scan("<item id=");

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].message(),
            "XML attribute value expected after `=`"
        );
    }

    #[test]
    fn diagnoses_an_unexpected_xml_attribute_value() {
        let (_, diagnostics) = scan(r#"<item "x"/>"#);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].message(),
            "XML attribute value must follow `=`"
        );
    }

    #[test]
    fn diagnoses_an_xml_attribute_without_a_name() {
        let (_, diagnostics) = scan("<item 123/>");

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message(), "XML attribute name expected");
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(6, 9).expect("valid range")
        );
    }

    #[test]
    fn diagnoses_an_xml_attribute_expression_without_a_name() {
        let (_, diagnostics) = scan("<item {flag}/>");

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message(), "XML attribute name expected");
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(6, 7).expect("valid range")
        );
    }

    #[test]
    fn diagnoses_an_unexpected_operator_in_an_xml_attribute_list() {
        let (_, diagnostics) = scan("<item @ />");

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message(), "XML attribute name expected");
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(6, 7).expect("valid range")
        );
    }

    #[test]
    fn emits_xml_comment_tokens_with_exact_spans() {
        let source = "<root><!-- comment --><x></x></root>";
        let (items, diagnostics) = scan(source);
        let tokens: Vec<_> = items
            .into_iter()
            .filter_map(|item| match item {
                RawItem::Token(token) => Some(token),
                RawItem::Trivia(_) => None,
            })
            .collect();

        assert_eq!(
            tokens.iter().map(|token| token.kind).collect::<Vec<_>>(),
            vec![
                RawTokenKind::XmlStart,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Eof,
            ]
        );
        assert_eq!(
            tokens
                .iter()
                .map(|token| &source[token.span.start() as usize..token.span.end() as usize])
                .collect::<Vec<_>>(),
            vec![
                "<", "root", "><!--", "comment", "--><", "x", "></", "x", "></", "root", ">", ""
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn emits_xml_cdata_tokens_with_exact_spans() {
        let source = "<root><![CDATA[text]]></root>";
        let (items, diagnostics) = scan(source);
        let tokens: Vec<_> = items
            .into_iter()
            .filter_map(|item| match item {
                RawItem::Token(token) => Some(token),
                RawItem::Trivia(_) => None,
            })
            .collect();

        assert_eq!(
            tokens.iter().map(|token| token.kind).collect::<Vec<_>>(),
            vec![
                RawTokenKind::XmlStart,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Punctuation(Punctuation::LeftBracket),
                RawTokenKind::Identifier,
                RawTokenKind::Punctuation(Punctuation::LeftBracket),
                RawTokenKind::Identifier,
                RawTokenKind::Punctuation(Punctuation::RightBracket),
                RawTokenKind::Punctuation(Punctuation::RightBracket),
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Eof,
            ]
        );
        assert_eq!(
            tokens
                .iter()
                .map(|token| &source[token.span.start() as usize..token.span.end() as usize])
                .collect::<Vec<_>>(),
            vec![
                "<", "root", "><!", "[", "CDATA", "[", "text", "]", "]", "></", "root", ">", ""
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn diagnoses_an_invalid_double_hyphen_inside_an_xml_comment() {
        let source = "<root><!-- bad -- text --></root>";
        let (_, diagnostics) = scan(source);
        let invalid_start = 10 + source[10..].find("--").expect("invalid sequence");

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].message(),
            "invalid `--` sequence in XML comment"
        );
        assert_eq!(
            diagnostics[0].span(),
            TextRange::new(invalid_start as u32, (invalid_start + 2) as u32).expect("valid range")
        );
    }

    #[test]
    fn keeps_spaced_less_than_as_an_operator() {
        let (items, diagnostics) = scan("a < b");
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
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn preserves_xml_greedy_tag_operator_boundaries() {
        let (items, diagnostics) = scan("<tag></tag>");
        let tokens: Vec<_> = items
            .into_iter()
            .filter_map(|item| match item {
                RawItem::Token(token) => Some(token),
                RawItem::Trivia(_) => None,
            })
            .collect();

        assert_eq!(
            tokens.iter().map(|token| token.kind).collect::<Vec<_>>(),
            vec![
                RawTokenKind::XmlStart,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Eof,
            ]
        );
        assert_eq!(
            tokens
                .iter()
                .map(|token| token.span)
                .map(|span| &"<tag></tag>"[span.start() as usize..span.end() as usize])
                .collect::<Vec<_>>(),
            vec!["<", "tag", "></", "tag", ">", ""]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn preserves_xml_self_closing_and_closing_tag_operators() {
        let (items, diagnostics) = scan("<root><child/></root>");
        let tokens: Vec<_> = items
            .into_iter()
            .filter_map(|item| match item {
                RawItem::Token(token) => Some(token),
                RawItem::Trivia(_) => None,
            })
            .collect();

        assert_eq!(
            tokens.iter().map(|token| token.kind).collect::<Vec<_>>(),
            vec![
                RawTokenKind::XmlStart,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Eof,
            ]
        );
        assert_eq!(
            tokens
                .iter()
                .map(|token| token.span)
                .map(|span| &"<root><child/></root>"[span.start() as usize..span.end() as usize])
                .collect::<Vec<_>>(),
            vec!["<", "root", "><", "child", "/></", "root", ">", ""]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn diagnoses_empty_invalid_and_unterminated_character_literals() {
        let (items, diagnostics) = scan("'' '\\q' '");
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

        assert_eq!(error_count, 3);
        assert_eq!(diagnostics.len(), 3);
    }

    #[test]
    fn rejects_a_supplementary_codepoint_in_a_character_literal() {
        let source = "'𐐀'";
        let (items, diagnostics) = scan(source);

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::Error, 0, source.len() as u32),
                token(RawTokenKind::Eof, source.len() as u32, source.len() as u32),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message().contains("UTF-16 code unit"));
    }

    #[test]
    fn rejects_a_combining_mark_as_an_extra_character() {
        let source = "'á'";
        let (items, diagnostics) = scan(source);

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::Error, 0, source.len() as u32),
                token(RawTokenKind::Eof, source.len() as u32, source.len() as u32),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message().contains("more than one character"));
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
    fn accepts_quotes_before_a_multiline_string_terminator() {
        let source = "\"\"\"text\"\"\"\"";
        let (items, diagnostics) = scan(source);

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::StringLiteral, 0, source.len() as u32),
                token(RawTokenKind::Eof, source.len() as u32, source.len() as u32),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn accepts_quotes_before_an_interpolated_string_terminator() {
        let source = "s\"\"\"text $value\"\"\"\"";
        let (items, diagnostics) = scan(source);
        let kinds: Vec<_> = items
            .iter()
            .filter_map(|item| match item {
                RawItem::Token(token) => Some(token.kind),
                RawItem::Trivia(_) => None,
            })
            .collect();

        assert_eq!(
            kinds,
            vec![
                RawTokenKind::InterpolationId,
                RawTokenKind::StringPart,
                RawTokenKind::Identifier,
                RawTokenKind::StringPart,
                RawTokenKind::Eof,
            ]
        );
        assert_eq!(
            items.last(),
            Some(&token(
                RawTokenKind::Eof,
                source.len() as u32,
                source.len() as u32
            ))
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

    #[test]
    fn reports_an_incomplete_unicode_escape_without_losing_the_string() {
        let source = r#""\u12""#;
        let (items, diagnostics) = scan(source);

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::StringLiteral, 0, source.len() as u32),
                token(RawTokenKind::Eof, source.len() as u32, source.len() as u32),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message().contains("Unicode escape"));
    }

    #[test]
    fn recovers_a_backslash_before_a_line_break_inside_a_string() {
        let source = "\"left\\\nright\"";
        let (items, diagnostics) = scan(source);

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::StringLiteral, 0, source.len() as u32),
                token(RawTokenKind::Eof, source.len() as u32, source.len() as u32),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message().contains("escape"));
    }

    #[test]
    fn recovers_an_octal_character_escape_as_a_literal_with_a_diagnostic() {
        let (items, diagnostics) = scan(r#"'\101'"#);

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::CharLiteral, 0, 6),
                token(RawTokenKind::Eof, 6, 6),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message().contains("octal escape"));
    }

    #[test]
    fn recovers_an_octal_string_escape_as_a_literal_with_a_diagnostic() {
        let (items, diagnostics) = scan(r#""\101""#);

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::StringLiteral, 0, 6),
                token(RawTokenKind::Eof, 6, 6),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message().contains("octal escape"));
    }

    #[test]
    fn recognizes_simple_interpolation_and_preserves_part_spans() {
        let (items, diagnostics) = scan("s\"hello $name!\"");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::InterpolationId, 0, 1),
                token(RawTokenKind::StringPart, 1, 9),
                token(RawTokenKind::Identifier, 9, 13),
                token(RawTokenKind::StringPart, 13, 15),
                token(RawTokenKind::Eof, 15, 15),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn recognizes_braced_interpolation_expressions() {
        let (items, diagnostics) = scan("s\"${foo + bar}\"");
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
                RawTokenKind::InterpolationId,
                RawTokenKind::StringPart,
                RawTokenKind::Punctuation(Punctuation::LeftBrace),
                RawTokenKind::Identifier,
                RawTokenKind::Operator,
                RawTokenKind::Identifier,
                RawTokenKind::Punctuation(Punctuation::RightBrace),
                RawTokenKind::StringPart,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn treats_double_dollar_as_string_content() {
        let (items, diagnostics) = scan("s\"$$$x\"");
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
                RawTokenKind::InterpolationId,
                RawTokenKind::StringPart,
                RawTokenKind::Identifier,
                RawTokenKind::StringPart,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn keeps_an_escaped_dollar_without_a_splice_as_string_content() {
        let (items, diagnostics) = scan("s\"cost $$5\"");
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
                RawTokenKind::InterpolationId,
                RawTokenKind::StringPart,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn keeps_an_underscore_simple_splice_as_one_identifier() {
        let (items, diagnostics) = scan("s\"$_value\"");
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
                RawTokenKind::InterpolationId,
                RawTokenKind::StringPart,
                RawTokenKind::Identifier,
                RawTokenKind::StringPart,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn keeps_a_unicode_simple_splice_as_one_identifier() {
        let (items, diagnostics) = scan("s\"$λ\"");
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
                RawTokenKind::InterpolationId,
                RawTokenKind::StringPart,
                RawTokenKind::Identifier,
                RawTokenKind::StringPart,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn preserves_multiline_interpolation_parts_around_simple_and_braced_splices() {
        let source = "s\"\"\"first\n$name\n${value}\nlast\"\"\"";
        let (items, diagnostics) = scan(source);
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
                RawTokenKind::InterpolationId,
                RawTokenKind::StringPart,
                RawTokenKind::Identifier,
                RawTokenKind::StringPart,
                RawTokenKind::Punctuation(Punctuation::LeftBrace),
                RawTokenKind::Identifier,
                RawTokenKind::Punctuation(Punctuation::RightBrace),
                RawTokenKind::StringPart,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn keeps_simple_splice_names_as_identifiers_even_when_they_are_keywords() {
        let (items, diagnostics) = scan("s\"$if\"");
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
                RawTokenKind::InterpolationId,
                RawTokenKind::StringPart,
                RawTokenKind::Identifier,
                RawTokenKind::StringPart,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn recovers_an_invalid_splice_as_a_string_literal_when_whitespace_follows() {
        let (items, diagnostics) = scan("s\"$ name\"");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::InterpolationId, 0, 1),
                token(RawTokenKind::StringLiteral, 1, 9),
                token(RawTokenKind::Eof, 9, 9),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message().contains("interpolation splice"));
    }

    #[test]
    fn recovers_an_invalid_splice_as_a_string_literal_when_a_digit_follows() {
        let (items, diagnostics) = scan("s\"$1\"");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::InterpolationId, 0, 1),
                token(RawTokenKind::StringLiteral, 1, 5),
                token(RawTokenKind::Eof, 5, 5),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message().contains("interpolation splice"));
    }

    #[test]
    fn recovers_an_unclosed_simple_splice_at_eof() {
        let (items, diagnostics) = scan("s\"$\"");

        assert_eq!(
            items,
            vec![
                token(RawTokenKind::InterpolationId, 0, 1),
                token(RawTokenKind::Error, 1, 4),
                token(RawTokenKind::Eof, 4, 4),
            ]
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message().contains("unclosed"));
    }

    #[test]
    fn supports_arbitrary_and_multiline_interpolator_names() {
        let (items, diagnostics) = scan("foo\"$x\" s\"\"\"$y\"\"\"");
        let interpolators = items
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    RawItem::Token(RawToken {
                        kind: RawTokenKind::InterpolationId,
                        ..
                    })
                )
            })
            .count();

        assert_eq!(interpolators, 2);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn supports_nested_interpolation_inside_a_braced_expression() {
        let (items, diagnostics) = scan("s\"${s\"$x\"}\"");
        let interpolation_count = items
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    RawItem::Token(RawToken {
                        kind: RawTokenKind::InterpolationId,
                        ..
                    })
                )
            })
            .count();

        assert_eq!(interpolation_count, 2);
        assert!(items.iter().any(|item| {
            matches!(
                item,
                RawItem::Token(RawToken {
                    kind: RawTokenKind::Punctuation(Punctuation::RightBrace),
                    ..
                })
            )
        }));
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn does_not_start_interpolation_after_whitespace() {
        let (items, diagnostics) = scan("s \"text\"");
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
                RawTokenKind::Identifier,
                RawTokenKind::StringLiteral,
                RawTokenKind::Eof,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn recovers_from_an_unclosed_interpolation_expression() {
        let (items, diagnostics) = scan("s\"${foo\"");

        assert!(items.iter().any(|item| {
            matches!(
                item,
                RawItem::Token(RawToken {
                    kind: RawTokenKind::Error,
                    ..
                })
            )
        }));
        assert!(!diagnostics.is_empty());
    }

    #[test]
    fn raw_lexer_preserves_contiguous_spans_for_short_ascii_inputs() {
        let alphabet = [
            ' ', '\t', '\n', '\r', 'a', '1', '\'', '"', '$', '{', '}', '/', '*',
        ];
        let mut input = String::new();
        let mut visited = 0;

        visit_short_ascii_inputs(&alphabet, &mut input, 3, &mut |source| {
            visited += 1;

            let (items, _) = scan(source);
            let mut offset = 0;
            for item in items {
                let span = match item {
                    RawItem::Token(token) => token.span,
                    RawItem::Trivia(trivia) => trivia.span,
                };
                assert_eq!(span.start(), offset, "raw span gap for {source:?}");
                offset = span.end();
            }
            assert_eq!(offset, source.len() as u32, "raw span gap for {source:?}");
        });

        let alphabet_size = alphabet.len();
        assert_eq!(
            visited,
            1 + alphabet_size + alphabet_size.pow(2) + alphabet_size.pow(3)
        );
    }

    #[test]
    fn contextual_scanner_reaches_eof_for_short_ascii_inputs() {
        let alphabet = [
            ' ', '\t', '\n', '\r', 'a', '1', '\'', '"', '$', '{', '}', '/', '*',
        ];
        let mut input = String::new();
        let mut visited = 0;

        visit_short_ascii_inputs(&alphabet, &mut input, 3, &mut |source| {
            visited += 1;

            let scanner = crate::ContextualScanner::new(source)
                .unwrap_or_else(|error| panic!("scanner rejected {source:?}: {error}"));
            assert_eq!(
                scanner.tokens().last().map(|token| token.kind),
                Some(TokenKind::Eof),
                "scanner did not reach EOF for {source:?}"
            );
        });

        let alphabet_size = alphabet.len();
        assert_eq!(
            visited,
            1 + alphabet_size + alphabet_size.pow(2) + alphabet_size.pow(3)
        );
    }

    #[test]
    fn contextual_scanner_keeps_token_spans_within_source_for_short_ascii_inputs() {
        let alphabet = [
            ' ', '\t', '\n', '\r', 'a', '1', '\'', '"', '$', '{', '}', '/', '*',
        ];
        let mut input = String::new();
        let mut visited = 0;

        visit_short_ascii_inputs(&alphabet, &mut input, 3, &mut |source| {
            visited += 1;
            let scanner = crate::ContextualScanner::new(source)
                .unwrap_or_else(|error| panic!("scanner rejected {source:?}: {error}"));

            for token in scanner.tokens() {
                assert!(
                    token.span.start() <= token.span.end(),
                    "invalid scanner span for {source:?}"
                );
                assert!(
                    token.span.end() <= source.len() as u32,
                    "scanner span exceeds source for {source:?}"
                );
            }
        });

        let alphabet_size = alphabet.len();
        assert_eq!(
            visited,
            1 + alphabet_size + alphabet_size.pow(2) + alphabet_size.pow(3)
        );
    }

    #[test]
    fn raw_lexer_preserves_contiguous_spans_for_short_unicode_inputs() {
        let alphabet = ['é', '𐐀', '\u{0301}', ' ', '\n', '\'', '"', '$', '{', '}'];
        let mut input = String::new();
        let mut visited = 0;

        visit_short_unicode_inputs(&alphabet, &mut input, 3, &mut |source| {
            visited += 1;

            let (items, _) = scan(source);
            let mut offset = 0;
            for item in items {
                let span = match item {
                    RawItem::Token(token) => token.span,
                    RawItem::Trivia(trivia) => trivia.span,
                };
                assert_eq!(span.start(), offset, "raw span gap for {source:?}");
                offset = span.end();
            }
            assert_eq!(offset, source.len() as u32, "raw span gap for {source:?}");
        });

        let alphabet_size = alphabet.len();
        assert_eq!(
            visited,
            1 + alphabet_size + alphabet_size.pow(2) + alphabet_size.pow(3)
        );
    }

    #[test]
    fn contextual_scanner_reaches_eof_for_short_unicode_inputs() {
        let alphabet = ['é', '𐐀', '\u{0301}', ' ', '\n', '\'', '"', '$', '{', '}'];
        let mut input = String::new();
        let mut visited = 0;

        visit_short_unicode_inputs(&alphabet, &mut input, 3, &mut |source| {
            visited += 1;

            let scanner = crate::ContextualScanner::new(source)
                .unwrap_or_else(|error| panic!("scanner rejected {source:?}: {error}"));
            assert_eq!(
                scanner.tokens().last().map(|token| token.kind),
                Some(TokenKind::Eof),
                "scanner did not reach EOF for {source:?}"
            );
        });

        let alphabet_size = alphabet.len();
        assert_eq!(
            visited,
            1 + alphabet_size + alphabet_size.pow(2) + alphabet_size.pow(3)
        );
    }

    #[test]
    fn contextual_scanner_keeps_token_spans_within_short_unicode_inputs() {
        let alphabet = ['é', '𐐀', '\u{0301}', ' ', '\n', '\'', '"', '$', '{', '}'];
        let mut input = String::new();
        let mut visited = 0;

        visit_short_unicode_inputs(&alphabet, &mut input, 3, &mut |source| {
            visited += 1;
            let scanner = crate::ContextualScanner::new(source)
                .unwrap_or_else(|error| panic!("scanner rejected {source:?}: {error}"));

            for token in scanner.tokens() {
                assert!(
                    token.span.start() <= token.span.end(),
                    "invalid scanner span for {source:?}"
                );
                assert!(
                    token.span.end() <= source.len() as u32,
                    "scanner span exceeds source for {source:?}"
                );
            }
        });

        let alphabet_size = alphabet.len();
        assert_eq!(
            visited,
            1 + alphabet_size + alphabet_size.pow(2) + alphabet_size.pow(3)
        );
    }

    #[test]
    fn recovers_truncated_interpolation_inputs_at_eof() {
        for source in ["s\"", "s\"$", "s\"${", "s\"${value", "s\"${value + other"] {
            let (items, diagnostics) = scan(source);
            assert_eq!(
                items.last().and_then(|item| match item {
                    RawItem::Token(token) => Some(token.kind),
                    RawItem::Trivia(_) => None,
                }),
                Some(RawTokenKind::Eof),
                "interpolation did not reach EOF for {source:?}"
            );
            assert!(
                !diagnostics.is_empty(),
                "truncated interpolation produced no diagnostic for {source:?}"
            );
        }
    }

    #[test]
    fn recovers_truncated_xml_inputs_at_eof() {
        for source in [
            "<root",
            "<root>",
            "<root>text",
            "<root>{value",
            "<root><!--",
        ] {
            let scanner = crate::ContextualScanner::new(source)
                .unwrap_or_else(|error| panic!("scanner rejected {source:?}: {error}"));
            assert_eq!(
                scanner.tokens().last().map(|token| token.kind),
                Some(TokenKind::Eof),
                "XML did not reach EOF for {source:?}"
            );
            assert!(
                !scanner.diagnostics().is_empty(),
                "truncated XML produced no diagnostic for {source:?}"
            );
        }
    }

    #[test]
    fn recovers_truncated_comments_and_literals_at_eof() {
        for source in ["/*", "/* outer /* inner", "`name", "\"hello", "'\\"] {
            let (items, diagnostics) = scan(source);
            assert_eq!(
                items.last().and_then(|item| match item {
                    RawItem::Token(token) => Some(token.kind),
                    RawItem::Trivia(_) => None,
                }),
                Some(RawTokenKind::Eof),
                "truncated input did not reach EOF for {source:?}"
            );
            assert!(
                !diagnostics.is_empty(),
                "truncated input produced no diagnostic for {source:?}"
            );
        }
    }
}
