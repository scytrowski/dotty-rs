use dotty_diagnostics::Diagnostic;
use dotty_source::{TextRange, TextRangeError};
use dotty_token::{HardKeyword, Punctuation, Token, TokenKind};

use crate::{RawItem, RawLexer, RawLexerError, RawToken, RawTokenKind, Trivia};

/// The first parser-facing scanner stage.
///
/// This stage maps the lossless raw stream to shared token kinds and performs
/// the layout rules that do not require parser feedback. It is intentionally
/// conservative around `:`: colon-triggered regions will be enabled when the
/// parser event protocol is implemented.
#[derive(Debug)]
pub struct ContextualScanner {
    tokens: Vec<Token>,
    position: usize,
    diagnostics: Vec<Diagnostic>,
}

impl ContextualScanner {
    /// Scans source text into parser-facing tokens.
    pub fn new(source: &str) -> Result<Self, RawLexerError> {
        let mut raw_lexer = RawLexer::new(source)?;
        let mut items = Vec::new();
        loop {
            let Some(item) = raw_lexer.next()? else {
                break;
            };
            let eof = matches!(
                item,
                RawItem::Token(RawToken {
                    kind: RawTokenKind::Eof,
                    ..
                })
            );
            items.push(item);
            if eof {
                break;
            }
        }

        let diagnostics = raw_lexer.diagnostics().to_vec();
        let tokens = build_tokens(source, &items)?;
        Ok(Self {
            tokens,
            position: 0,
            diagnostics,
        })
    }

    /// Returns the diagnostics collected by the raw and contextual stages.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Returns all tokens produced by this scan.
    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    /// Returns the next token and advances the scanner.
    pub fn next(&mut self) -> Option<&Token> {
        let token = self.tokens.get(self.position);
        if token.is_some() {
            self.position += 1;
        }
        token
    }
}

fn build_tokens(source: &str, items: &[RawItem]) -> Result<Vec<Token>, RawLexerError> {
    let mut tokens = Vec::new();
    let mut trivia = Vec::new();
    let mut previous_kind = None;
    let mut previous_end = 0;
    let mut indentation_stack = vec![String::new()];
    let mut paren_depth = 0u32;
    let mut bracket_depth = 0u32;
    let mut brace_depth = 0u32;

    for item in items {
        match item {
            RawItem::Trivia(current) => trivia.push(current),
            RawItem::Token(raw) => {
                let has_line_break = trivia_has_line_break(source, &trivia);
                let blank_line = trivia_line_breaks(source, &trivia) > 1;
                let indentation = line_indentation(source, raw.span.start());
                let layout_enabled = paren_depth == 0 && bracket_depth == 0;

                if has_line_break && layout_enabled {
                    let dedented = if brace_depth == 0 {
                        adjust_indentation(
                            &mut tokens,
                            &mut indentation_stack,
                            &indentation,
                            previous_kind,
                            raw.span.start(),
                        )?
                    } else {
                        false
                    };

                    if !dedented
                        && can_end_statement(previous_kind)
                        && can_start_statement(raw.kind)
                        && !matches!(raw.kind, RawTokenKind::Operator)
                    {
                        let separator = if blank_line {
                            TokenKind::Newlines
                        } else {
                            TokenKind::Newline
                        };
                        tokens.push(Token::new(
                            separator,
                            TextRange::new(previous_end, raw.span.start())?,
                        ));
                    }
                }

                let token = Token::new(to_token_kind(raw.kind), raw.span);
                update_delimiters(
                    raw.kind,
                    &mut paren_depth,
                    &mut bracket_depth,
                    &mut brace_depth,
                );
                previous_kind = Some(token.kind);
                previous_end = raw.span.end();
                tokens.push(token);
                trivia.clear();

                if raw.kind == RawTokenKind::Eof {
                    while indentation_stack.len() > 1 {
                        indentation_stack.pop();
                        tokens.insert(
                            tokens.len() - 1,
                            Token::new(
                                TokenKind::Outdent,
                                TextRange::new(raw.span.start(), raw.span.start())?,
                            ),
                        );
                    }
                }
            }
        }
    }

    Ok(tokens)
}

fn adjust_indentation(
    tokens: &mut Vec<Token>,
    stack: &mut Vec<String>,
    indentation: &str,
    previous_kind: Option<TokenKind>,
    offset: u32,
) -> Result<bool, TextRangeError> {
    let current = stack.last().map(String::as_str).unwrap_or("");
    if current == indentation {
        return Ok(false);
    }

    if is_prefix(current, indentation) && opens_indentation(previous_kind) {
        stack.push(indentation.to_owned());
        tokens.push(Token::new(
            TokenKind::Indent,
            TextRange::new(offset, offset)?,
        ));
        return Ok(false);
    }

    let mut dedented = false;
    while stack.len() > 1 {
        let current = stack.last().map(String::as_str).unwrap_or("");
        if is_prefix(current, indentation) {
            break;
        }
        stack.pop();
        tokens.push(Token::new(
            TokenKind::Outdent,
            TextRange::new(offset, offset)?,
        ));
        dedented = true;
    }

    Ok(dedented)
}

fn opens_indentation(kind: Option<TokenKind>) -> bool {
    matches!(
        kind,
        Some(TokenKind::Keyword(
            HardKeyword::Then
                | HardKeyword::Else
                | HardKeyword::Do
                | HardKeyword::Try
                | HardKeyword::Catch
                | HardKeyword::Finally
                | HardKeyword::For
                | HardKeyword::While
                | HardKeyword::Match
                | HardKeyword::With
                | HardKeyword::Yield
        ))
    )
}

fn can_end_statement(kind: Option<TokenKind>) -> bool {
    matches!(
        kind,
        Some(
            TokenKind::Identifier
                | TokenKind::BackquotedIdentifier
                | TokenKind::CharLiteral
                | TokenKind::IntegerLiteral
                | TokenKind::DecimalLiteral
                | TokenKind::ExponentLiteral
                | TokenKind::LongLiteral
                | TokenKind::FloatLiteral
                | TokenKind::DoubleLiteral
                | TokenKind::StringLiteral
                | TokenKind::StringPart
                | TokenKind::Punctuation(
                    Punctuation::RightParen | Punctuation::RightBracket | Punctuation::RightBrace
                )
                | TokenKind::Keyword(
                    HardKeyword::This
                        | HardKeyword::Super
                        | HardKeyword::Null
                        | HardKeyword::True
                        | HardKeyword::False
                        | HardKeyword::End
                )
        )
    )
}

fn can_start_statement(kind: RawTokenKind) -> bool {
    !matches!(
        kind,
        RawTokenKind::Eof
            | RawTokenKind::Error
            | RawTokenKind::Operator
            | RawTokenKind::Punctuation(
                Punctuation::Comma
                    | Punctuation::Semicolon
                    | Punctuation::Dot
                    | Punctuation::RightParen
                    | Punctuation::RightBracket
                    | Punctuation::RightBrace
            )
    )
}

fn is_prefix(prefix: &str, value: &str) -> bool {
    value.starts_with(prefix)
}

fn update_delimiters(kind: RawTokenKind, parens: &mut u32, brackets: &mut u32, braces: &mut u32) {
    match kind {
        RawTokenKind::Punctuation(Punctuation::LeftParen) => *parens = parens.saturating_add(1),
        RawTokenKind::Punctuation(Punctuation::RightParen) => *parens = parens.saturating_sub(1),
        RawTokenKind::Punctuation(Punctuation::LeftBracket) => {
            *brackets = brackets.saturating_add(1)
        }
        RawTokenKind::Punctuation(Punctuation::RightBracket) => {
            *brackets = brackets.saturating_sub(1)
        }
        RawTokenKind::Punctuation(Punctuation::LeftBrace) => *braces = braces.saturating_add(1),
        RawTokenKind::Punctuation(Punctuation::RightBrace) => *braces = braces.saturating_sub(1),
        _ => {}
    }
}

fn trivia_has_line_break(source: &str, trivia: &[&Trivia]) -> bool {
    trivia_line_breaks(source, trivia) > 0
}

fn trivia_line_breaks(source: &str, trivia: &[&Trivia]) -> usize {
    trivia
        .iter()
        .map(|item| {
            count_line_breaks(&source[item.span.start() as usize..item.span.end() as usize])
        })
        .sum()
}

fn count_line_breaks(text: &str) -> usize {
    let mut count = 0;
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\r' => {
                count += 1;
                if chars.peek() == Some(&'\n') {
                    let _ = chars.next();
                }
            }
            '\n' => count += 1,
            _ => {}
        }
    }
    count
}

fn line_indentation(source: &str, offset: u32) -> String {
    let bytes = source.as_bytes();
    let mut line_start = offset as usize;
    while line_start > 0 && !matches!(bytes[line_start - 1], b'\n' | b'\r') {
        line_start -= 1;
    }
    source[line_start..offset as usize]
        .chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .collect()
}

fn to_token_kind(kind: RawTokenKind) -> TokenKind {
    match kind {
        RawTokenKind::Error => TokenKind::Error,
        RawTokenKind::Eof => TokenKind::Eof,
        RawTokenKind::Identifier => TokenKind::Identifier,
        RawTokenKind::BackquotedIdentifier => TokenKind::BackquotedIdentifier,
        RawTokenKind::Operator => TokenKind::Operator,
        RawTokenKind::Keyword(keyword) => TokenKind::Keyword(keyword),
        RawTokenKind::Punctuation(punctuation) => TokenKind::Punctuation(punctuation),
        RawTokenKind::CharLiteral => TokenKind::CharLiteral,
        RawTokenKind::IntegerLiteral => TokenKind::IntegerLiteral,
        RawTokenKind::DecimalLiteral => TokenKind::DecimalLiteral,
        RawTokenKind::ExponentLiteral => TokenKind::ExponentLiteral,
        RawTokenKind::LongLiteral => TokenKind::LongLiteral,
        RawTokenKind::FloatLiteral => TokenKind::FloatLiteral,
        RawTokenKind::DoubleLiteral => TokenKind::DoubleLiteral,
        RawTokenKind::StringLiteral => TokenKind::StringLiteral,
        RawTokenKind::InterpolationId => TokenKind::InterpolationId,
        RawTokenKind::StringPart => TokenKind::StringPart,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        ContextualScanner::new(source)
            .expect("source should scan")
            .tokens()
            .iter()
            .map(|token| token.kind)
            .collect()
    }

    #[test]
    fn maps_raw_tokens_to_shared_parser_kinds() {
        assert_eq!(
            kinds("val answer = 42"),
            vec![
                TokenKind::Keyword(HardKeyword::Val),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::IntegerLiteral,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn inserts_a_newline_between_simple_statements() {
        assert_eq!(
            kinds("val first = 1\nval second = 2"),
            vec![
                TokenKind::Keyword(HardKeyword::Val),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::IntegerLiteral,
                TokenKind::Newline,
                TokenKind::Keyword(HardKeyword::Val),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::IntegerLiteral,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn inserts_newlines_after_a_blank_line() {
        assert!(kinds("val first = 1\n\nval second = 2").contains(&TokenKind::Newlines));
    }

    #[test]
    fn opens_and_closes_an_indentation_region_after_then() {
        assert_eq!(
            kinds("if ready then\n  run()\nfinish()"),
            vec![
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Then),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn suppresses_layout_inside_parentheses() {
        assert!(!kinds("call(\n  first,\n  second\n)").iter().any(|kind| {
            matches!(
                kind,
                TokenKind::Newline | TokenKind::Newlines | TokenKind::Indent
            )
        }));
    }

    #[test]
    fn closes_open_regions_before_eof() {
        assert_eq!(
            kinds("if ready then\n  run()"),
            vec![
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Then),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn treats_crlf_and_comment_lines_as_logical_line_breaks() {
        assert_eq!(
            kinds("val first = 1\r\n// comment\r\nval second = 2"),
            vec![
                TokenKind::Keyword(HardKeyword::Val),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::IntegerLiteral,
                TokenKind::Newlines,
                TokenKind::Keyword(HardKeyword::Val),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::IntegerLiteral,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keeps_newline_separators_in_braces_but_disables_indentation() {
        let token_kinds = kinds("{\n  first\n  second\n}");

        assert!(token_kinds.contains(&TokenKind::Newline));
        assert!(!token_kinds.contains(&TokenKind::Indent));
        assert!(!token_kinds.contains(&TokenKind::Outdent));
    }

    #[test]
    fn synthetic_layout_tokens_are_zero_width_at_the_following_token() {
        let scanner = ContextualScanner::new("if ready then\n  run()").expect("source scans");
        let indent = scanner
            .tokens()
            .iter()
            .find(|token| token.kind == TokenKind::Indent)
            .expect("indent token");
        let run = scanner
            .tokens()
            .iter()
            .find(|token| {
                token.kind == TokenKind::Identifier && token.span.start() >= indent.span.start()
            })
            .expect("run identifier");

        assert_eq!(indent.span.start(), run.span.start());
        assert!(indent.span.is_empty());
    }

    #[test]
    fn forwards_recoverable_raw_diagnostics() {
        let scanner = ContextualScanner::new("\"unclosed").expect("source scans");

        assert_eq!(scanner.diagnostics().len(), 1);
    }
}
