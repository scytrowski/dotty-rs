use dotty_diagnostics::Diagnostic;
use dotty_source::{TextRange, TextRangeError};
use dotty_token::{HardKeyword, Punctuation, ScannerEvent, Token, TokenKind, TokenSource};

use crate::xml::XmlState;
use crate::{RawItem, RawLexer, RawLexerError, RawToken, RawTokenKind, Trivia};

/// The first parser-facing scanner stage.
///
/// This stage maps the lossless raw stream to shared token kinds and performs
/// the layout rules that do not require parser feedback. Colon-triggered
/// regions are enabled only after the parser sends the corresponding scanner
/// event.
#[derive(Debug)]
pub struct ContextualScanner {
    source: String,
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

        let mut diagnostics = raw_lexer.diagnostics().to_vec();
        let tokens = build_tokens(source, &items, &mut diagnostics)?;
        Ok(Self {
            source: source.to_owned(),
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

    fn current_index(&self) -> usize {
        self.position.min(self.tokens.len().saturating_sub(1))
    }

    fn next_line_is_indented(&self, current_index: usize) -> bool {
        let Some(current) = self.tokens.get(current_index) else {
            return false;
        };
        let Some(next) = self.tokens.get(current_index + 1) else {
            return false;
        };
        if next.kind == TokenKind::Eof {
            return false;
        }
        if !has_source_line_break(&self.source, current.span.end(), next.span.start()) {
            return false;
        }
        let current_indent = line_indentation(&self.source, current.span.start());
        let next_indent = line_indentation(&self.source, next.span.start());
        is_prefix(&current_indent, &next_indent) && current_indent != next_indent
    }

    fn insert_indent_after_current(&mut self) {
        let index = self.current_index();
        if !self.next_line_is_indented(index)
            || self
                .tokens
                .get(index + 1)
                .is_some_and(|token| token.kind == TokenKind::Indent)
        {
            return;
        }
        let offset = self.tokens[index + 1].span.start();
        self.tokens.insert(
            index + 1,
            Token::new(
                TokenKind::Indent,
                TextRange::new(offset, offset).expect("synthetic range is valid"),
            ),
        );
    }

    fn insert_outdent_before_current(&mut self) {
        let index = self.current_index();
        if self
            .tokens
            .get(index)
            .is_some_and(|token| token.kind == TokenKind::Outdent)
        {
            return;
        }
        let depth = self.tokens[..index]
            .iter()
            .fold(0usize, |depth, token| match token.kind {
                TokenKind::Indent => depth.saturating_add(1),
                TokenKind::Outdent => depth.saturating_sub(1),
                _ => depth,
            });
        if depth == 0 {
            return;
        }
        let offset = self.tokens[index].span.start();
        self.tokens.insert(
            index,
            Token::new(
                TokenKind::Outdent,
                TextRange::new(offset, offset).expect("synthetic range is valid"),
            ),
        );
    }
}

impl TokenSource for ContextualScanner {
    fn current(&self) -> &Token {
        &self.tokens[self.current_index()]
    }

    fn advance(&mut self) {
        if self.position + 1 < self.tokens.len() {
            self.position += 1;
        }
    }

    fn lookahead(&mut self, n: usize) -> &Token {
        let index = self
            .current_index()
            .saturating_add(n)
            .min(self.tokens.len().saturating_sub(1));
        &self.tokens[index]
    }

    fn observe(&mut self, event: ScannerEvent) {
        match event {
            ScannerEvent::ColonEol { .. } => {
                let index = self.current_index();
                if self.tokens[index].kind == TokenKind::ColonFollow
                    || self.tokens[index].kind == TokenKind::ColonOp
                {
                    self.tokens[index].kind = TokenKind::ColonEol;
                }
            }
            ScannerEvent::Indented => self.insert_indent_after_current(),
            ScannerEvent::Outdented => self.insert_outdent_before_current(),
            ScannerEvent::ArrowIndented => {
                if self.current().kind == TokenKind::Operator {
                    self.insert_indent_after_current();
                }
            }
        }
    }
}

fn build_tokens(
    source: &str,
    items: &[RawItem],
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Vec<Token>, RawLexerError> {
    let mut tokens = Vec::new();
    let mut trivia = Vec::new();
    let mut previous_kind = None;
    let mut previous_opens_indentation = false;
    let mut previous_end = 0;
    let mut indentation_stack = vec![LayoutRegion::root()];
    let mut paren_depth = 0u32;
    let mut bracket_depth = 0u32;
    let mut brace_depth = 0u32;
    let mut xml = XmlState::default();

    for (item_index, item) in items.iter().enumerate() {
        match item {
            RawItem::Trivia(current) => trivia.push(current),
            RawItem::Token(raw) => {
                let has_line_break = trivia_has_line_break(source, &trivia);
                let blank_line = trivia_line_breaks(source, &trivia) > 1;
                let indentation = line_indentation(source, raw.span.start());
                let layout_enabled = paren_depth == 0 && bracket_depth == 0;
                let leading_infix = is_leading_infix(
                    source,
                    items,
                    item_index,
                    previous_kind,
                    blank_line,
                    previous_end,
                );

                if has_line_break && layout_enabled {
                    if brace_depth == 0 {
                        if has_incomparable_indentation(&indentation_stack, &indentation) {
                            let line_start = line_start_offset(source, raw.span.start());
                            diagnostics.push(Diagnostic::error(
                                TextRange::new(line_start, raw.span.start())?,
                                "incompatible indentation prefixes",
                            ));
                        }
                        adjust_indentation(
                            &mut tokens,
                            &mut indentation_stack,
                            &indentation,
                            previous_kind,
                            previous_opens_indentation,
                            raw.span.start(),
                        )?;
                    }

                    let blank_line_before_operator =
                        blank_line && raw.kind == RawTokenKind::Operator;
                    if can_end_statement(previous_kind)
                        && !leading_infix
                        && !suppresses_statement_separator(raw.kind)
                        && (can_start_statement(raw.kind) || blank_line_before_operator)
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

                let token = Token::new(to_token_kind(raw.kind, previous_kind), raw.span);
                update_delimiters(
                    raw.kind,
                    &mut paren_depth,
                    &mut bracket_depth,
                    &mut brace_depth,
                );
                previous_kind = Some(token.kind);
                previous_opens_indentation = opens_indentation(
                    Some(token.kind),
                    is_layout_operator(
                        token.kind,
                        &source[raw.span.start() as usize..raw.span.end() as usize],
                    ),
                );
                previous_end = raw.span.end();
                tokens.push(token);

                let xml_literal_ended = xml.update_token(
                    raw.kind,
                    &source[raw.span.start() as usize..raw.span.end() as usize],
                );
                if xml_literal_ended {
                    previous_kind = Some(TokenKind::Identifier);
                    previous_opens_indentation = false;
                }

                if has_line_break && is_closing_delimiter(raw.kind) {
                    close_regions_after_delimiter(
                        &mut tokens,
                        &mut indentation_stack,
                        &indentation,
                        raw.span.end(),
                    )?;
                }
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

    fuse_case_declarations(source, &mut tokens)?;
    classify_end_markers(source, &mut tokens);
    insert_end_marker_separators(source, &mut tokens)?;

    Ok(tokens)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayoutRegionOwner {
    Root,
    Implicit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LayoutRegion {
    indentation: String,
    owner: LayoutRegionOwner,
}

impl LayoutRegion {
    fn root() -> Self {
        Self {
            indentation: String::new(),
            owner: LayoutRegionOwner::Root,
        }
    }

    fn implicit(indentation: &str) -> Self {
        Self {
            indentation: indentation.to_owned(),
            owner: LayoutRegionOwner::Implicit,
        }
    }
}

fn adjust_indentation(
    tokens: &mut Vec<Token>,
    stack: &mut Vec<LayoutRegion>,
    indentation: &str,
    previous_kind: Option<TokenKind>,
    previous_opens_indentation: bool,
    offset: u32,
) -> Result<(), TextRangeError> {
    let current = stack
        .last()
        .map(|region| region.indentation.as_str())
        .unwrap_or("");
    if current == indentation {
        return Ok(());
    }

    if is_prefix(current, indentation)
        && opens_indentation(previous_kind, previous_opens_indentation)
    {
        stack.push(LayoutRegion::implicit(indentation));
        tokens.push(Token::new(
            TokenKind::Indent,
            TextRange::new(offset, offset)?,
        ));
        return Ok(());
    }

    while stack.len() > 1 {
        let current = stack
            .last()
            .map(|region| region.indentation.as_str())
            .unwrap_or("");
        if is_prefix(current, indentation) {
            break;
        }
        stack.pop();
        tokens.push(Token::new(
            TokenKind::Outdent,
            TextRange::new(offset, offset)?,
        ));
    }

    Ok(())
}

fn close_regions_after_delimiter(
    tokens: &mut Vec<Token>,
    stack: &mut Vec<LayoutRegion>,
    indentation: &str,
    offset: u32,
) -> Result<(), TextRangeError> {
    while stack.len() > 1 {
        let Some(region) = stack.last() else {
            break;
        };
        if region.owner != LayoutRegionOwner::Implicit
            || is_prefix(&region.indentation, indentation)
        {
            break;
        }
        stack.pop();
        tokens.push(Token::new(
            TokenKind::Outdent,
            TextRange::new(offset, offset)?,
        ));
    }
    Ok(())
}

fn has_incomparable_indentation(stack: &[LayoutRegion], indentation: &str) -> bool {
    let Some(current) = stack.last().map(|region| region.indentation.as_str()) else {
        return false;
    };
    !current.is_empty()
        && !indentation.is_empty()
        && !is_prefix(current, indentation)
        && !is_prefix(indentation, current)
}

fn opens_indentation(kind: Option<TokenKind>, operator: bool) -> bool {
    if operator {
        return true;
    }
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

fn is_layout_operator(kind: TokenKind, spelling: &str) -> bool {
    matches!(kind, TokenKind::Operator) && matches!(spelling, "=" | "=>" | "<-")
}

fn can_end_statement(kind: Option<TokenKind>) -> bool {
    matches!(
        kind,
        Some(
            TokenKind::Identifier
                | TokenKind::BackquotedIdentifier
                | TokenKind::QuoteId
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
                | TokenKind::EndMarker
        )
    )
}

fn can_start_statement(kind: RawTokenKind) -> bool {
    !matches!(
        kind,
        RawTokenKind::Eof
            | RawTokenKind::Error
            | RawTokenKind::Keyword(HardKeyword::Do)
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

fn suppresses_statement_separator(kind: RawTokenKind) -> bool {
    matches!(
        kind,
        RawTokenKind::Keyword(HardKeyword::Else | HardKeyword::Catch | HardKeyword::Finally)
    )
}

fn is_leading_infix(
    source: &str,
    items: &[RawItem],
    item_index: usize,
    previous_kind: Option<TokenKind>,
    blank_line: bool,
    previous_end: u32,
) -> bool {
    if blank_line || !can_end_statement(previous_kind) {
        return false;
    }
    let RawItem::Token(current) = &items[item_index] else {
        return false;
    };
    if !matches!(
        current.kind,
        RawTokenKind::Operator | RawTokenKind::BackquotedIdentifier
    ) {
        return false;
    }
    let Some(next) = next_raw_token(items, item_index) else {
        return false;
    };
    if !can_start_statement(next.kind) {
        return false;
    }

    let previous_indent = line_indentation(source, previous_end.saturating_sub(1));
    let operator_indent = line_indentation(source, current.span.start());
    is_prefix(&previous_indent, &operator_indent)
}

fn next_raw_token(items: &[RawItem], item_index: usize) -> Option<&RawToken> {
    items[item_index + 1..].iter().find_map(|item| match item {
        RawItem::Token(token) => Some(token),
        RawItem::Trivia(_) => None,
    })
}

fn is_layout_token(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Newline | TokenKind::Newlines | TokenKind::Indent | TokenKind::Outdent
    )
}

fn previous_real_token(tokens: &[Token], index: usize) -> Option<&Token> {
    tokens[..index]
        .iter()
        .rev()
        .find(|token| !is_layout_token(token.kind))
}

fn next_real_token(tokens: &[Token], index: usize) -> Option<&Token> {
    tokens[index + 1..]
        .iter()
        .find(|token| !is_layout_token(token.kind))
}

fn next_real_token_index(tokens: &[Token], index: usize) -> Option<usize> {
    (index + 1..tokens.len()).find(|&candidate| !is_layout_token(tokens[candidate].kind))
}

fn fuse_case_declarations(source: &str, tokens: &mut Vec<Token>) -> Result<(), TextRangeError> {
    let mut index = 0;
    while index + 1 < tokens.len() {
        if has_source_line_break(
            source,
            tokens[index].span.end(),
            tokens[index + 1].span.start(),
        ) {
            index += 1;
            continue;
        }
        let Some(fused_kind) = (match (tokens[index].kind, tokens[index + 1].kind) {
            (TokenKind::Keyword(HardKeyword::Case), TokenKind::Keyword(HardKeyword::Class)) => {
                Some(TokenKind::CaseClass)
            }
            (TokenKind::Keyword(HardKeyword::Case), TokenKind::Keyword(HardKeyword::Object)) => {
                Some(TokenKind::CaseObject)
            }
            _ => None,
        }) else {
            index += 1;
            continue;
        };

        let span = TextRange::new(tokens[index].span.start(), tokens[index + 1].span.end())?;
        tokens[index] = Token::new(fused_kind, span);
        tokens.remove(index + 1);
    }
    Ok(())
}

fn classify_end_markers(source: &str, tokens: &mut [Token]) {
    for index in 0..tokens.len() {
        if tokens[index].kind != TokenKind::Keyword(HardKeyword::End) {
            continue;
        }

        let starts_line = previous_real_token(tokens, index).is_some_and(|previous| {
            has_source_line_break(source, previous.span.end(), tokens[index].span.start())
        });
        let Some(next) = next_real_token(tokens, index) else {
            tokens[index].kind = TokenKind::Identifier;
            continue;
        };

        let same_line = !has_source_line_break(source, tokens[index].span.end(), next.span.start());
        if !starts_line || !same_line || !is_end_marker_target(next.kind) {
            tokens[index].kind = TokenKind::Identifier;
            continue;
        }

        let line_ends = match next_real_token_after(tokens, index, next) {
            None => true,
            Some(following) if following.kind == TokenKind::Eof => true,
            Some(following) => {
                has_source_line_break(source, next.span.end(), following.span.start())
            }
        };

        tokens[index].kind = if line_ends {
            TokenKind::EndMarker
        } else {
            TokenKind::Identifier
        };
    }
}

fn insert_end_marker_separators(
    source: &str,
    tokens: &mut Vec<Token>,
) -> Result<(), TextRangeError> {
    let mut index = 0;
    while index < tokens.len() {
        if tokens[index].kind != TokenKind::EndMarker {
            index += 1;
            continue;
        }

        let Some(target_index) = next_real_token_index(tokens, index) else {
            index += 1;
            continue;
        };
        if !end_marker_can_end_statement(tokens[target_index].kind) {
            index += 1;
            continue;
        }
        let Some(following_index) = next_real_token_index(tokens, target_index) else {
            index += 1;
            continue;
        };
        let following = &tokens[following_index];
        if following.kind == TokenKind::Eof
            || !can_start_statement_token(following.kind)
            || !has_source_line_break(
                source,
                tokens[target_index].span.end(),
                following.span.start(),
            )
        {
            index += 1;
            continue;
        }

        let target_end = tokens[target_index].span.end();
        let separator_kind =
            if count_line_breaks(&source[target_end as usize..following.span.start() as usize]) > 1
            {
                TokenKind::Newlines
            } else {
                TokenKind::Newline
            };
        tokens.insert(
            following_index,
            Token::new(
                separator_kind,
                TextRange::new(target_end, following.span.start())?,
            ),
        );
        index = following_index + 1;
    }
    Ok(())
}

fn can_start_statement_token(kind: TokenKind) -> bool {
    !matches!(
        kind,
        TokenKind::Error
            | TokenKind::Eof
            | TokenKind::Operator
            | TokenKind::Punctuation(
                Punctuation::Comma
                    | Punctuation::Semicolon
                    | Punctuation::Dot
                    | Punctuation::RightParen
                    | Punctuation::RightBracket
                    | Punctuation::RightBrace
            )
    )
}

fn end_marker_can_end_statement(kind: TokenKind) -> bool {
    matches!(kind, TokenKind::Keyword(HardKeyword::Given))
}

fn next_real_token_after<'tokens>(
    tokens: &'tokens [Token],
    index: usize,
    token: &Token,
) -> Option<&'tokens Token> {
    let token_index = tokens[index + 1..]
        .iter()
        .position(|candidate| std::ptr::eq(candidate, token))?;
    next_real_token(tokens, index + 1 + token_index)
}

fn is_end_marker_target(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Identifier
            | TokenKind::BackquotedIdentifier
            | TokenKind::Keyword(
                HardKeyword::If
                    | HardKeyword::For
                    | HardKeyword::While
                    | HardKeyword::Match
                    | HardKeyword::Try
                    | HardKeyword::New
                    | HardKeyword::This
                    | HardKeyword::Given
                    | HardKeyword::Val
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

fn is_closing_delimiter(kind: RawTokenKind) -> bool {
    matches!(
        kind,
        RawTokenKind::Punctuation(
            Punctuation::RightParen | Punctuation::RightBracket | Punctuation::RightBrace
        )
    )
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
    let requested_offset = (offset as usize).min(source.len());
    let safe_offset = (0..=requested_offset)
        .rev()
        .find(|candidate| source.is_char_boundary(*candidate))
        .unwrap_or(0);
    let line_start = line_start_offset(source, safe_offset as u32) as usize;
    source[line_start..safe_offset]
        .chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .collect()
}

fn line_start_offset(source: &str, offset: u32) -> u32 {
    let bytes = source.as_bytes();
    let mut line_start = offset as usize;
    while line_start > 0 && !matches!(bytes[line_start - 1], b'\n' | b'\r') {
        line_start -= 1;
    }
    line_start as u32
}

fn has_source_line_break(source: &str, start: u32, end: u32) -> bool {
    start < end && count_line_breaks(&source[start as usize..end as usize]) > 0
}

fn to_token_kind(kind: RawTokenKind, previous: Option<TokenKind>) -> TokenKind {
    match kind {
        RawTokenKind::Error => TokenKind::Error,
        RawTokenKind::Eof => TokenKind::Eof,
        RawTokenKind::Identifier => TokenKind::Identifier,
        RawTokenKind::BackquotedIdentifier => TokenKind::BackquotedIdentifier,
        RawTokenKind::Quote => TokenKind::Quote,
        RawTokenKind::QuoteId => TokenKind::QuoteId,
        RawTokenKind::XmlStart => TokenKind::XmlStart,
        RawTokenKind::Operator => TokenKind::Operator,
        RawTokenKind::Keyword(keyword) => TokenKind::Keyword(keyword),
        RawTokenKind::Punctuation(Punctuation::Colon) => {
            if matches!(
                previous,
                Some(
                    TokenKind::Identifier
                        | TokenKind::BackquotedIdentifier
                        | TokenKind::Punctuation(
                            Punctuation::RightParen | Punctuation::RightBracket
                        )
                )
            ) {
                TokenKind::ColonFollow
            } else {
                TokenKind::ColonOp
            }
        }
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

    fn assert_newline_after_xml_literal(source: &str) {
        let tokens = kinds(source);
        let newline = tokens
            .iter()
            .position(|kind| *kind == TokenKind::Newline)
            .expect("XML literal should end with a newline separator");

        assert_eq!(
            tokens.get(newline + 1),
            Some(&TokenKind::Keyword(HardKeyword::Val))
        );
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
    fn separates_statements_after_a_legacy_quoted_identifier() {
        assert_eq!(
            kinds("val first = 'foo\nval second = 1"),
            vec![
                TokenKind::Keyword(HardKeyword::Val),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::QuoteId,
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
    fn maps_quote_markers_to_parser_facing_tokens() {
        assert_eq!(
            kinds("'{ 1 }"),
            vec![
                TokenKind::Quote,
                TokenKind::Punctuation(Punctuation::LeftBrace),
                TokenKind::IntegerLiteral,
                TokenKind::Punctuation(Punctuation::RightBrace),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn maps_xml_start_to_a_parser_facing_token() {
        assert_eq!(
            kinds("val xml = <tag>"),
            vec![
                TokenKind::Keyword(HardKeyword::Val),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::XmlStart,
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keeps_xml_namespace_separators_as_operators() {
        assert_eq!(
            kinds(r#"val xml = <ns:item xml:lang="en"/>"#),
            vec![
                TokenKind::Keyword(HardKeyword::Val),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::XmlStart,
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Identifier,
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::StringLiteral,
                TokenKind::Operator,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn separates_after_a_closed_xml_literal() {
        assert_eq!(
            kinds("val xml = <tag></tag>\nval next = 1"),
            vec![
                TokenKind::Keyword(HardKeyword::Val),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::XmlStart,
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Identifier,
                TokenKind::Operator,
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
    fn separates_after_a_self_closing_xml_literal() {
        assert_eq!(
            kinds("val xml = <tag/>\nval next = 1"),
            vec![
                TokenKind::Keyword(HardKeyword::Val),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::XmlStart,
                TokenKind::Identifier,
                TokenKind::Operator,
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
    fn separates_after_a_nested_xml_literal() {
        assert_eq!(
            kinds("val xml = <root><child/></root>\nval next = 1"),
            vec![
                TokenKind::Keyword(HardKeyword::Val),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::XmlStart,
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Identifier,
                TokenKind::Operator,
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
    fn separates_after_xml_with_attributes() {
        assert_newline_after_xml_literal(
            "val xml = <item id=\"x\" enabled={flag}>text</item>\nval next = 1",
        );
    }

    #[test]
    fn separates_after_xml_with_a_comment() {
        assert_newline_after_xml_literal(
            "val xml = <root><!-- comment --><x></x></root>\nval next = 1",
        );
    }

    #[test]
    fn separates_after_xml_with_cdata() {
        assert_newline_after_xml_literal("val xml = <root><![CDATA[text]]></root>\nval next = 1");
    }

    #[test]
    fn classifies_colon_after_an_identifier_as_colon_follow() {
        assert_eq!(
            kinds("value: Int"),
            vec![
                TokenKind::Identifier,
                TokenKind::ColonFollow,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn classifies_colon_after_an_operator_as_colon_op() {
        assert_eq!(
            kinds("+ : Int"),
            vec![
                TokenKind::Operator,
                TokenKind::ColonOp,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn fuses_case_class_and_case_object_declarations() {
        assert_eq!(
            kinds("case class Foo\ncase object Bar"),
            vec![
                TokenKind::CaseClass,
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::CaseObject,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn does_not_fuse_case_and_class_across_a_line_break() {
        assert_eq!(
            kinds("case\nclass Foo"),
            vec![
                TokenKind::Keyword(HardKeyword::Case),
                TokenKind::Keyword(HardKeyword::Class),
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn does_not_fuse_case_and_object_across_a_line_break() {
        assert_eq!(
            kinds("case\nobject Foo"),
            vec![
                TokenKind::Keyword(HardKeyword::Case),
                TokenKind::Keyword(HardKeyword::Object),
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn does_not_fuse_case_and_class_across_a_multiline_comment() {
        assert_eq!(
            kinds("case /* comment\n*/ class Foo"),
            vec![
                TokenKind::Keyword(HardKeyword::Case),
                TokenKind::Keyword(HardKeyword::Class),
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recognizes_an_end_marker_at_the_start_of_a_line() {
        assert_eq!(
            kinds("if ready then\n  run()\nend if"),
            vec![
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Then),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keeps_end_as_an_identifier_at_the_start_of_the_source() {
        assert_eq!(
            kinds("end if"),
            vec![
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keeps_end_as_an_identifier_after_leading_blank_lines() {
        assert_eq!(
            kinds("\n\nend if"),
            vec![
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recognizes_an_end_marker_for_a_given_declaration() {
        assert_eq!(
            kinds("value\nend given"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::Given),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recognizes_an_identifier_as_an_end_marker_target() {
        assert_eq!(
            kinds("value\nend method"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recognizes_for_as_an_end_marker_target() {
        assert_eq!(
            kinds("value\nend for"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::For),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recognizes_while_as_an_end_marker_target() {
        assert_eq!(
            kinds("value\nend while"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::While),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recognizes_match_as_an_end_marker_target() {
        assert_eq!(
            kinds("value\nend match"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::Match),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recognizes_try_as_an_end_marker_target() {
        assert_eq!(
            kinds("value\nend try"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::Try),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn inserts_a_newline_after_an_end_given_before_the_next_statement() {
        assert_eq!(
            kinds("value\nend given\nnext"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::Given),
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn does_not_insert_a_newline_after_an_end_new_before_the_next_statement() {
        assert_eq!(
            kinds("value\nend new\nnext"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::New),
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recognizes_new_as_an_end_marker_target() {
        assert_eq!(
            kinds("value\nend new"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::New),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recognizes_this_as_an_end_marker_target() {
        assert_eq!(
            kinds("value\nend this"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::This),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recognizes_val_as_an_end_marker_target() {
        assert_eq!(
            kinds("value\nend val"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::Val),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn does_not_recognize_an_enum_keyword_as_an_end_marker_target() {
        assert_eq!(
            kinds("value\nend enum"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Enum),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keeps_end_as_an_identifier_when_used_in_a_declaration() {
        assert_eq!(
            kinds("val end = 1"),
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
    fn keeps_a_standalone_end_as_an_identifier() {
        assert_eq!(
            kinds("end\nnext"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn colon_eol_feedback_reclassifies_colon_and_can_open_indent() {
        let mut scanner = ContextualScanner::new("object Foo:\n  val x = 1").expect("source scans");
        scanner.advance();
        scanner.advance();

        assert_eq!(scanner.current().kind, TokenKind::ColonFollow);
        scanner.observe(ScannerEvent::ColonEol { in_template: false });
        assert_eq!(scanner.current().kind, TokenKind::ColonEol);

        scanner.observe(ScannerEvent::Indented);
        assert_eq!(scanner.lookahead(1).kind, TokenKind::Indent);
        scanner.advance();
        assert_eq!(scanner.current().kind, TokenKind::Indent);
        scanner.advance();
        assert_eq!(scanner.current().kind, TokenKind::Keyword(HardKeyword::Val));
    }

    #[test]
    fn arrow_indented_feedback_opens_a_body_region() {
        let mut scanner = ContextualScanner::new("case 1 =>\n  body").expect("source scans");
        scanner.advance();
        scanner.advance();

        assert_eq!(scanner.current().kind, TokenKind::Operator);
        scanner.observe(ScannerEvent::ArrowIndented);
        scanner.advance();
        assert_eq!(scanner.current().kind, TokenKind::Indent);
        scanner.advance();
        assert_eq!(scanner.current().kind, TokenKind::Identifier);
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
    fn keeps_a_leading_infix_operator_on_the_previous_statement() {
        assert_eq!(
            kinds("value\n  + other\n\nnext"),
            vec![
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Identifier,
                TokenKind::Newlines,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keeps_a_leading_backquoted_operator_on_the_previous_statement() {
        assert_eq!(
            kinds("value\n  `op` other\n\nnext"),
            vec![
                TokenKind::Identifier,
                TokenKind::BackquotedIdentifier,
                TokenKind::Identifier,
                TokenKind::Newlines,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn separates_a_backquoted_operator_after_a_blank_line() {
        assert_eq!(
            kinds("value\n\n  `op` other"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newlines,
                TokenKind::BackquotedIdentifier,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn separates_a_leading_operator_after_a_blank_line() {
        assert_eq!(
            kinds("value\n\n  + other"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newlines,
                TokenKind::Operator,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
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
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn closes_a_then_region_without_a_separator_before_else() {
        assert_eq!(
            kinds("if condition then\n  first()\nelse\n  second()\nafter()"),
            vec![
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Then),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Keyword(HardKeyword::Else),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn supports_an_else_if_chain_without_separators_before_clauses() {
        assert_eq!(
            kinds("if first then\n  one()\nelse if second then\n  two()\nelse\n  three()\ndone()"),
            vec![
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Then),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Keyword(HardKeyword::Else),
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Then),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Keyword(HardKeyword::Else),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn closes_try_regions_without_separators_before_catch_and_finally() {
        assert_eq!(
            kinds(
                "try\n  risky()\ncatch\n  case error => recover()\nfinally\n  cleanup()\nafter_try()"
            ),
            vec![
                TokenKind::Keyword(HardKeyword::Try),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Keyword(HardKeyword::Catch),
                TokenKind::Indent,
                TokenKind::Keyword(HardKeyword::Case),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Keyword(HardKeyword::Finally),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keeps_do_as_a_continuation_before_its_body() {
        assert_eq!(
            kinds("value\n\ndo\n  body()\nwhile condition"),
            vec![
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Do),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Keyword(HardKeyword::While),
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn opens_a_do_region_and_closes_it_before_while() {
        assert_eq!(
            kinds("do\n  body()\nwhile condition"),
            vec![
                TokenKind::Keyword(HardKeyword::Do),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Keyword(HardKeyword::While),
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn closes_an_indentation_region_after_a_dedented_parenthesis() {
        assert_eq!(
            kinds("if ready then\n  call(\n    value\n)\nnext"),
            vec![
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Then),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keeps_an_indentation_region_open_before_an_aligned_parenthesis() {
        assert_eq!(
            kinds("if ready then\n  call(\n    value\n  )\n  next"),
            vec![
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Then),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Outdent,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn closes_an_indentation_region_after_a_dedented_bracket() {
        assert_eq!(
            kinds("if ready then\n  values[\n    0\n]\nnext"),
            vec![
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Then),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftBracket),
                TokenKind::IntegerLiteral,
                TokenKind::Punctuation(Punctuation::RightBracket),
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn closes_an_indentation_region_after_a_dedented_brace() {
        assert_eq!(
            kinds("if ready then\n  block {\n    value\n}\nnext"),
            vec![
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Then),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftBrace),
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::RightBrace),
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Identifier,
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
    fn treats_a_standalone_cr_as_a_logical_line_break() {
        assert_eq!(
            kinds("first\rsecond"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::Identifier,
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
    fn opens_nested_case_bodies_after_match_and_arrow_tokens() {
        assert_eq!(
            kinds("value match\n  case 1 =>\n    one()\n  case _ =>\n    other()"),
            vec![
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Match),
                TokenKind::Indent,
                TokenKind::Keyword(HardKeyword::Case),
                TokenKind::IntegerLiteral,
                TokenKind::Operator,
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Keyword(HardKeyword::Case),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Outdent,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn closes_match_case_regions_before_a_following_statement() {
        assert_eq!(
            kinds(
                "value match\n  case first =>\n    one()\n  case second =>\n    two()\nafter_match()"
            ),
            vec![
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Match),
                TokenKind::Indent,
                TokenKind::Keyword(HardKeyword::Case),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Keyword(HardKeyword::Case),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Eof,
            ]
        );
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

    #[test]
    fn forwards_unterminated_xml_element_diagnostic() {
        let source = "<item>text";
        let scanner = ContextualScanner::new(source).expect("source scans");

        assert_eq!(scanner.diagnostics().len(), 1);
        assert_eq!(scanner.diagnostics()[0].message(), "unterminated XML tag");
        assert_eq!(
            scanner.diagnostics()[0].span(),
            TextRange::new(source.len() as u32, source.len() as u32).expect("valid range")
        );
    }

    #[test]
    fn forwards_an_incomplete_xml_attribute_diagnostic() {
        let scanner = ContextualScanner::new("<item id=").expect("source scans");

        assert_eq!(scanner.diagnostics().len(), 1);
        assert_eq!(
            scanner.diagnostics()[0].message(),
            "XML attribute value expected after `=`"
        );
    }

    #[test]
    fn forwards_an_invalid_xml_attribute_name_diagnostic() {
        let source = "<item 123/>";
        let scanner = ContextualScanner::new(source).expect("source scans");

        assert_eq!(scanner.diagnostics().len(), 1);
        assert_eq!(
            scanner.diagnostics()[0].message(),
            "XML attribute name expected"
        );
        assert_eq!(
            scanner.diagnostics()[0].span(),
            TextRange::new(6, 9).expect("valid range")
        );
    }

    #[test]
    fn accepts_string_literals_inside_xml_attribute_expressions() {
        let scanner = ContextualScanner::new(r#"<item title={"hello"}/>"#).expect("source scans");

        assert!(scanner.diagnostics().is_empty());
    }

    #[test]
    fn forwards_an_unterminated_xml_attribute_expression_diagnostic() {
        let source = "<item enabled={flag";
        let scanner = ContextualScanner::new(source).expect("source scans");

        assert_eq!(scanner.diagnostics().len(), 1);
        assert_eq!(
            scanner.diagnostics()[0].message(),
            "XML attribute expression must be closed"
        );
        assert_eq!(
            scanner.diagnostics()[0].span(),
            TextRange::new(source.len() as u32, source.len() as u32).expect("valid range")
        );
    }

    #[test]
    fn forwards_an_invalid_xml_closing_tag_diagnostic() {
        let source = "<root></root id>";
        let scanner = ContextualScanner::new(source).expect("source scans");

        assert_eq!(scanner.diagnostics().len(), 1);
        assert_eq!(
            scanner.diagnostics()[0].message(),
            "XML closing tag cannot contain attributes"
        );
        assert_eq!(
            scanner.diagnostics()[0].span(),
            TextRange::new(13, 15).expect("valid range")
        );
    }

    #[test]
    fn diagnoses_incomparable_indentation_prefixes() {
        let scanner =
            ContextualScanner::new("if ready then\n  first\n\tsecond").expect("source scans");

        assert_eq!(scanner.diagnostics().len(), 1);
        assert_eq!(
            scanner.diagnostics()[0].message(),
            "incompatible indentation prefixes"
        );
    }

    #[test]
    fn handles_unicode_before_a_trailing_newline() {
        let scanner = ContextualScanner::new("val café = \"żółw\"\n").expect("source scans");

        assert_eq!(
            scanner.tokens().last().map(|token| token.kind),
            Some(TokenKind::Eof)
        );
    }
}
