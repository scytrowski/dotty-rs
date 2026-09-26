use dotty_core::diagnostics::Diagnostic;
use dotty_core::source::{TextRange, TextRangeError, is_line_break_char};
use dotty_core::token::{HardKeyword, Punctuation, ScannerEvent, Token, TokenKind, TokenSource};

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
    feedback_regions: usize,
}

impl ContextualScanner {
    /// Scans source text into parser-facing tokens.
    pub fn new(source: &str) -> Result<Self, RawLexerError> {
        let mut raw_lexer = RawLexer::new(source)?;
        let mut items = Vec::new();
        while let Some(item) = raw_lexer.next()? {
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
            feedback_regions: 0,
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
    #[allow(clippy::should_implement_trait)]
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
        let current_end = if is_layout_token(current.kind) {
            current.span.start()
        } else {
            current.span.end()
        };
        if !has_source_line_break(&self.source, current_end, next.span.start()) {
            return false;
        }
        let current_indent = line_indentation(&self.source, current.span.start());
        let next_indent = line_indentation(&self.source, next.span.start());
        current_indent.is_prefix_of(&next_indent) && current_indent != next_indent
    }

    fn insert_indent_after_current(&mut self) -> bool {
        let index = self.current_index();
        if !self.next_line_is_indented(index)
            || self
                .tokens
                .get(index + 1)
                .is_some_and(|token| token.kind == TokenKind::Indent)
        {
            return false;
        }
        let offset = self.tokens[index + 1].span.start();
        self.tokens.insert(
            index + 1,
            Token::new(
                TokenKind::Indent,
                TextRange::new(offset, offset).expect("synthetic range is valid"),
            ),
        );
        true
    }

    /// Restores statement separators inside an arrow body when the ordinary
    /// layout pass suppressed them because the body is nested in parentheses
    /// or brackets. Only peer statements at the body indentation are split;
    /// nested layout and the body-closing outdent remain parser feedback.
    fn insert_arrow_body_separators(&mut self) {
        let arrow_index = self.current_index();
        let body_index = arrow_index.saturating_add(2);
        let Some(first_body_token) = self.tokens.get(body_index) else {
            return;
        };
        let body_indent = line_indentation(&self.source, first_body_token.span.start());
        let mut previous_index = body_index;
        let mut separators = Vec::new();

        for current_index in body_index.saturating_add(1)..self.tokens.len() {
            let previous = &self.tokens[previous_index];
            let current = &self.tokens[current_index];
            if is_layout_token(current.kind) {
                continue;
            }

            let current_indent = line_indentation(&self.source, current.span.start());
            match current_indent.ordering(&body_indent) {
                IndentOrdering::Less => break,
                IndentOrdering::Equal => {}
                IndentOrdering::Greater | IndentOrdering::Incomparable => {
                    previous_index = current_index;
                    continue;
                }
            }

            let has_line_break =
                has_source_line_break(&self.source, previous.span.end(), current.span.start());
            let has_separator = self.tokens[previous_index + 1..current_index]
                .iter()
                .any(|token| matches!(token.kind, TokenKind::Newline | TokenKind::Newlines));
            let starts_unspaced_prefix_expr =
                is_unspaced_prefix_expr(&self.source, &self.tokens, current_index);
            if has_line_break
                && !has_separator
                && can_end_statement(Some(previous.kind))
                && (can_start_statement_kind(current.kind) || starts_unspaced_prefix_expr)
                && !suppresses_statement_separator_kind(current.kind)
                && !is_leading_infix_tokens(
                    &self.source,
                    &self.tokens,
                    previous_index,
                    current_index,
                )
            {
                let line_breaks = count_line_breaks(
                    &self.source[previous.span.end() as usize..current.span.start() as usize],
                );
                let kind = if line_breaks > 1 {
                    TokenKind::Newlines
                } else {
                    TokenKind::Newline
                };
                if let Ok(span) = TextRange::new(previous.span.end(), current.span.start()) {
                    separators.push((current_index, Token::new(kind, span)));
                }
            }
            previous_index = current_index;
        }

        for (index, separator) in separators.into_iter().rev() {
            self.tokens.insert(index, separator);
        }
    }

    fn insert_outdent_before_current(&mut self) -> bool {
        let index = self.current_index();
        if self
            .tokens
            .get(index)
            .is_some_and(|token| token.kind == TokenKind::Outdent)
        {
            return false;
        }
        if self.tokens[index].kind != TokenKind::Eof {
            let Some(indent_index) = self.tokens[..index]
                .iter()
                .rposition(|token| token.kind == TokenKind::Indent)
            else {
                return false;
            };
            let region_indent =
                line_indentation(&self.source, self.tokens[indent_index].span.start());
            let current_offset = if is_layout_token(self.tokens[index].kind) {
                next_real_token(&self.tokens, index)
                    .map_or(self.tokens[index].span.start(), |token| token.span.start())
            } else {
                self.tokens[index].span.start()
            };
            let current_indent = line_indentation(&self.source, current_offset);
            if !matches!(
                current_indent.ordering(&region_indent),
                IndentOrdering::Less
            ) {
                return false;
            }
        }
        let depth = self.tokens[..index]
            .iter()
            .fold(0usize, |depth, token| match token.kind {
                TokenKind::Indent => depth.saturating_add(1),
                TokenKind::Outdent => depth.saturating_sub(1),
                _ => depth,
            });
        if depth == 0 {
            return false;
        }
        let offset = self.tokens[index].span.start();
        self.tokens.insert(
            index,
            Token::new(
                TokenKind::Outdent,
                TextRange::new(offset, offset).expect("synthetic range is valid"),
            ),
        );
        true
    }

    fn remove_pending_indent_after_current(&mut self) -> bool {
        let mut index = self.current_index().saturating_add(1);
        while self
            .tokens
            .get(index)
            .is_some_and(|token| matches!(token.kind, TokenKind::Newline | TokenKind::Newlines))
        {
            index += 1;
        }
        if self
            .tokens
            .get(index)
            .is_some_and(|token| token.kind == TokenKind::Indent)
        {
            self.tokens.remove(index);
            true
        } else {
            false
        }
    }

    fn current_is_arrow(&self) -> bool {
        let current = self.current();
        current.kind == TokenKind::Operator
            && self
                .source
                .get(current.span.start() as usize..current.span.end() as usize)
                .is_some_and(|spelling| matches!(spelling, "=>" | "?=>"))
    }

    fn is_at_line_end(&self, index: usize) -> bool {
        let Some(current) = self.tokens.get(index) else {
            return false;
        };
        match next_real_token(&self.tokens, index) {
            None => true,
            Some(next) if next.kind == TokenKind::Eof => true,
            Some(next) => {
                has_source_line_break(&self.source, current.span.end(), next.span.start())
            }
        }
    }
}

impl TokenSource for ContextualScanner {
    fn current(&self) -> &Token {
        &self.tokens[self.current_index()]
    }

    fn position(&self) -> usize {
        self.current_index()
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
            ScannerEvent::ColonEol { in_template } => {
                let index = self.current_index();
                let enabled = match self.tokens[index].kind {
                    TokenKind::ColonFollow => true,
                    TokenKind::ColonOp => in_template,
                    _ => false,
                };
                if enabled && self.is_at_line_end(index) {
                    self.tokens[index].kind = TokenKind::ColonEol;
                }
            }
            ScannerEvent::Indented => {
                if self.insert_indent_after_current() {
                    self.feedback_regions += 1;
                }
            }
            ScannerEvent::Outdented => {
                if self.feedback_regions > 0 && self.insert_outdent_before_current() {
                    self.feedback_regions -= 1;
                }
            }
            ScannerEvent::ArrowIndented => {
                if self.current_is_arrow() && self.insert_indent_after_current() {
                    self.feedback_regions += 1;
                    self.insert_arrow_body_separators();
                }
            }
            ScannerEvent::SelfArrow => {
                // A template self arrow is already inside the template's
                // layout region. Unlike a lambda arrow it must not open a
                // second synthetic indentation region. The eager scanner may
                // already have inserted that region based on the raw arrow,
                // so remove only that pending synthetic token here.
                self.remove_pending_indent_after_current();
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
    let mut previous_end = 0u32;
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
                let previous_indentation = line_indentation(source, previous_end.saturating_sub(1));
                let continues_previous_region = previous_opens_indentation
                    && (previous_kind == Some(TokenKind::Operator)
                        || (previous_kind != Some(TokenKind::Operator)
                            && previous_indentation != indentation
                            && previous_indentation.is_prefix_of(&indentation)));
                let layout_enabled = paren_depth == 0 && bracket_depth == 0;
                let leading_infix = is_leading_infix(
                    source,
                    items,
                    item_index,
                    previous_kind,
                    blank_line,
                    previous_end,
                );
                let case_guard_candidate = raw.kind == RawTokenKind::Keyword(HardKeyword::If)
                    && indentation_stack
                        .last()
                        .is_some_and(|region| region.owner == LayoutRegionOwner::SameIndentCases)
                    && !current_case_has_arrow(source, &tokens);

                if has_line_break && layout_enabled {
                    let mut closed_same_indent_case = false;
                    if brace_depth == 0 {
                        if has_incomparable_indentation(&indentation_stack, &indentation) {
                            let line_start = line_start_offset(source, raw.span.start());
                            diagnostics.push(Diagnostic::error(
                                TextRange::new(line_start, raw.span.start())?,
                                "incompatible indentation prefixes",
                            ));
                        }
                        closed_same_indent_case = adjust_indentation(
                            &mut tokens,
                            &mut indentation_stack,
                            IndentationTransition {
                                indentation: &indentation,
                                previous_kind,
                                previous_opens_indentation,
                                current_kind: raw.kind,
                                leading_infix,
                                case_guard_candidate,
                                offset: raw.span.start(),
                            },
                        )?;
                    }

                    if closed_same_indent_case
                        && can_end_statement(previous_kind)
                        && !continues_previous_region
                        && !leading_infix
                        && !suppresses_statement_separator(raw.kind)
                        && can_start_statement(raw.kind)
                    {
                        let separator = if blank_line {
                            TokenKind::Newlines
                        } else {
                            TokenKind::Newline
                        };
                        let outdent_index = tokens.len().saturating_sub(1);
                        tokens.insert(
                            outdent_index,
                            Token::new(separator, TextRange::new(previous_end, raw.span.start())?),
                        );
                    }

                    let blank_line_before_operator =
                        blank_line && raw.kind == RawTokenKind::Operator;
                    if can_end_statement(previous_kind)
                        && !continues_previous_region
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

    Ok(tokens)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayoutRegionOwner {
    Root,
    Implicit,
    SameIndentCases,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IndentWidth {
    prefix: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndentOrdering {
    Less,
    Equal,
    Greater,
    Incomparable,
}

impl IndentWidth {
    fn empty() -> Self {
        Self {
            prefix: String::new(),
        }
    }

    fn from_prefix(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_owned(),
        }
    }

    fn ordering(&self, other: &Self) -> IndentOrdering {
        if self.prefix == other.prefix {
            IndentOrdering::Equal
        } else if other.prefix.starts_with(&self.prefix) {
            IndentOrdering::Less
        } else if self.prefix.starts_with(&other.prefix) {
            IndentOrdering::Greater
        } else {
            IndentOrdering::Incomparable
        }
    }

    fn is_prefix_of(&self, other: &Self) -> bool {
        matches!(
            self.ordering(other),
            IndentOrdering::Less | IndentOrdering::Equal
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LayoutRegion {
    indentation: IndentWidth,
    owner: LayoutRegionOwner,
}

impl LayoutRegion {
    fn root() -> Self {
        Self {
            indentation: IndentWidth::empty(),
            owner: LayoutRegionOwner::Root,
        }
    }

    fn implicit(indentation: &IndentWidth) -> Self {
        Self {
            indentation: indentation.clone(),
            owner: LayoutRegionOwner::Implicit,
        }
    }

    fn same_indent_cases(indentation: &IndentWidth) -> Self {
        Self {
            indentation: indentation.clone(),
            owner: LayoutRegionOwner::SameIndentCases,
        }
    }
}

struct IndentationTransition<'a> {
    indentation: &'a IndentWidth,
    previous_kind: Option<TokenKind>,
    previous_opens_indentation: bool,
    current_kind: RawTokenKind,
    leading_infix: bool,
    case_guard_candidate: bool,
    offset: u32,
}

fn adjust_indentation(
    tokens: &mut Vec<Token>,
    stack: &mut Vec<LayoutRegion>,
    transition: IndentationTransition<'_>,
) -> Result<bool, TextRangeError> {
    let IndentationTransition {
        indentation,
        previous_kind,
        previous_opens_indentation,
        current_kind,
        leading_infix,
        case_guard_candidate,
        offset,
    } = transition;
    let mut closed_same_indent_case = false;
    let current = stack
        .last()
        .map(|region| region.indentation.clone())
        .unwrap_or_else(IndentWidth::empty);
    if current == *indentation {
        if opens_same_indent_case_region(previous_kind, current_kind) {
            stack.push(LayoutRegion::same_indent_cases(indentation));
            tokens.push(Token::new(
                TokenKind::Indent,
                TextRange::new(offset, offset)?,
            ));
        } else if current_kind != RawTokenKind::Keyword(HardKeyword::Case)
            && !case_guard_candidate
            && stack
                .last()
                .is_some_and(|region| region.owner == LayoutRegionOwner::SameIndentCases)
        {
            stack.pop();
            tokens.push(Token::new(
                TokenKind::Outdent,
                TextRange::new(offset, offset)?,
            ));
            closed_same_indent_case = true;
        }
        return Ok(closed_same_indent_case);
    }

    if leading_infix {
        return Ok(false);
    }

    if current.is_prefix_of(indentation)
        && opens_indentation(previous_kind, previous_opens_indentation)
    {
        stack.push(LayoutRegion::implicit(indentation));
        tokens.push(Token::new(
            TokenKind::Indent,
            TextRange::new(offset, offset)?,
        ));
        return Ok(false);
    }

    while stack.len() > 1 {
        let current = stack
            .last()
            .map(|region| region.indentation.clone())
            .unwrap_or_else(IndentWidth::empty);
        if current.is_prefix_of(indentation) {
            if current == *indentation
                && current_kind != RawTokenKind::Keyword(HardKeyword::Case)
                && !case_guard_candidate
                && stack
                    .last()
                    .is_some_and(|region| region.owner == LayoutRegionOwner::SameIndentCases)
            {
                stack.pop();
                tokens.push(Token::new(
                    TokenKind::Outdent,
                    TextRange::new(offset, offset)?,
                ));
                closed_same_indent_case = true;
            }
            break;
        }
        stack.pop();
        tokens.push(Token::new(
            TokenKind::Outdent,
            TextRange::new(offset, offset)?,
        ));
    }

    Ok(closed_same_indent_case)
}

fn opens_same_indent_case_region(
    previous_kind: Option<TokenKind>,
    current_kind: RawTokenKind,
) -> bool {
    matches!(
        (previous_kind, current_kind),
        (
            Some(TokenKind::Keyword(HardKeyword::Match | HardKeyword::Catch)),
            RawTokenKind::Keyword(HardKeyword::Case)
        )
    )
}

fn current_case_has_arrow(source: &str, tokens: &[Token]) -> bool {
    let Some(case_index) = tokens
        .iter()
        .rposition(|token| token.kind == TokenKind::Keyword(HardKeyword::Case))
    else {
        return false;
    };

    tokens[case_index + 1..].iter().any(|token| {
        token.kind == TokenKind::Operator
            && source.get(token.span.start() as usize..token.span.end() as usize) == Some("=>")
    })
}

fn close_regions_after_delimiter(
    tokens: &mut Vec<Token>,
    stack: &mut Vec<LayoutRegion>,
    indentation: &IndentWidth,
    offset: u32,
) -> Result<(), TextRangeError> {
    while stack.len() > 1 {
        let Some(region) = stack.last() else {
            break;
        };
        if region.owner == LayoutRegionOwner::Root || region.indentation.is_prefix_of(indentation) {
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

fn has_incomparable_indentation(stack: &[LayoutRegion], indentation: &IndentWidth) -> bool {
    let Some(current) = stack.last().map(|region| &region.indentation) else {
        return false;
    };
    current.ordering(indentation) == IndentOrdering::Incomparable
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
                | HardKeyword::Return
                | HardKeyword::Throw
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
                | TokenKind::Operator
                | TokenKind::Punctuation(
                    Punctuation::RightParen | Punctuation::RightBracket | Punctuation::RightBrace
                )
                | TokenKind::Keyword(
                    HardKeyword::This
                        | HardKeyword::Super
                        | HardKeyword::Null
                        | HardKeyword::True
                        | HardKeyword::False
                        | HardKeyword::Type
                        | HardKeyword::Return
                        | HardKeyword::Given
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

fn can_start_statement_kind(kind: TokenKind) -> bool {
    !matches!(
        kind,
        TokenKind::Eof
            | TokenKind::Error
            | TokenKind::Operator
            | TokenKind::Keyword(HardKeyword::Do)
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

fn suppresses_statement_separator_kind(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Keyword(
            HardKeyword::Then
                | HardKeyword::With
                | HardKeyword::Else
                | HardKeyword::Catch
                | HardKeyword::Finally
                | HardKeyword::Yield
        )
    )
}

fn is_leading_infix_tokens(
    source: &str,
    tokens: &[Token],
    previous_index: usize,
    current_index: usize,
) -> bool {
    let previous = &tokens[previous_index];
    let current = &tokens[current_index];
    if !matches!(
        current.kind,
        TokenKind::Operator | TokenKind::BackquotedIdentifier
    ) || !can_end_statement(Some(previous.kind))
        || count_line_breaks(&source[previous.span.end() as usize..current.span.start() as usize])
            > 1
    {
        return false;
    }
    let Some(next) = next_real_token(tokens, current_index) else {
        return false;
    };
    // Dotty only treats a leading symbolic/backquoted name as an infix
    // operator when whitespace follows it. Without this check, a line such
    // as `!second` would be joined to the preceding expression instead of
    // starting a new expression with the prefix operator `!`.
    if !source[current.span.end() as usize..next.span.start() as usize]
        .chars()
        .next()
        .is_some_and(char::is_whitespace)
        || !can_start_statement_kind(next.kind)
    {
        return false;
    }
    let previous_indent = line_indentation(source, previous.span.end().saturating_sub(1));
    let operator_indent = line_indentation(source, current.span.start());
    previous_indent.is_prefix_of(&operator_indent)
}

fn is_unspaced_prefix_expr(source: &str, tokens: &[Token], operator_index: usize) -> bool {
    let operator = &tokens[operator_index];
    if operator.kind != TokenKind::Operator
        || !matches!(
            &source[operator.span.start() as usize..operator.span.end() as usize],
            "-" | "+" | "~" | "!"
        )
    {
        return false;
    }
    let Some(operand) = next_real_token(tokens, operator_index) else {
        return false;
    };
    source[operator.span.end() as usize..operand.span.start() as usize].is_empty()
        && can_start_statement_kind(operand.kind)
}

fn suppresses_statement_separator(kind: RawTokenKind) -> bool {
    matches!(
        kind,
        RawTokenKind::Keyword(
            HardKeyword::Then
                | HardKeyword::With
                | HardKeyword::Else
                | HardKeyword::Catch
                | HardKeyword::Finally
                | HardKeyword::Yield
        )
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
    previous_indent.is_prefix_of(&operator_indent)
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
                    | HardKeyword::Throw
            )
    )
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
            character if is_line_break_char(character) => count += 1,
            _ => {}
        }
    }
    count
}

fn line_indentation(source: &str, offset: u32) -> IndentWidth {
    let requested_offset = (offset as usize).min(source.len());
    let safe_offset = (0..=requested_offset)
        .rev()
        .find(|candidate| source.is_char_boundary(*candidate))
        .unwrap_or(0);
    let line_start = line_start_offset(source, safe_offset as u32) as usize;
    let prefix: String = source[line_start..safe_offset]
        .chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .collect();
    IndentWidth::from_prefix(&prefix)
}

fn line_start_offset(source: &str, offset: u32) -> u32 {
    let bytes = source.as_bytes();
    let mut line_start = offset as usize;
    while line_start > 0 && !matches!(bytes[line_start - 1], b'\n' | b'\r' | 0x0c | 0x1a) {
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
                        | TokenKind::Keyword(
                            HardKeyword::This | HardKeyword::Super | HardKeyword::New,
                        )
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

    #[test]
    fn indentation_width_orders_strict_prefixes() {
        let shallow = IndentWidth::from_prefix("  ");
        let deep = IndentWidth::from_prefix("    ");

        assert_eq!(shallow.ordering(&deep), IndentOrdering::Less);
        assert_eq!(deep.ordering(&shallow), IndentOrdering::Greater);
    }

    #[test]
    fn indentation_width_treats_equal_prefixes_as_equal() {
        let first = IndentWidth::from_prefix("\t\t");
        let second = IndentWidth::from_prefix("\t\t");

        assert_eq!(first.ordering(&second), IndentOrdering::Equal);
    }

    #[test]
    fn indentation_width_does_not_expand_tabs_to_fixed_columns() {
        let spaces = IndentWidth::from_prefix("    ");
        let tab = IndentWidth::from_prefix("\t");

        assert_eq!(spaces.ordering(&tab), IndentOrdering::Incomparable);
        assert_eq!(tab.ordering(&spaces), IndentOrdering::Incomparable);
    }

    #[test]
    fn indentation_width_orders_tab_prefixed_extensions_by_prefix() {
        let shallow = IndentWidth::from_prefix("\t\t");
        let deep = IndentWidth::from_prefix("\t\t ");

        assert_eq!(shallow.ordering(&deep), IndentOrdering::Less);
    }

    fn next_event_value(seed: &mut u64) -> u64 {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *seed
    }

    fn for_deterministic_event_sequence(
        mut assertion: impl FnMut(usize, &str, &mut ContextualScanner),
    ) {
        let sources = [
            "",
            "value",
            "root:\n  child:\nback",
            "if ready then\n  run()",
            "value + : next",
            "<root>{value}</root>",
        ];
        let mut seed = 0x5CA_77E_u64;

        for case in 0..1_024 {
            let source = sources[(next_event_value(&mut seed) as usize) % sources.len()];
            let mut scanner = ContextualScanner::new(source)
                .unwrap_or_else(|error| panic!("scanner rejected case {case}: {error}"));

            for _ in 0..64 {
                match next_event_value(&mut seed) % 7 {
                    0 => scanner.advance(),
                    1 => {
                        let _ = scanner.next();
                    }
                    2 => {
                        let _ = scanner.lookahead((next_event_value(&mut seed) as usize) % 16);
                    }
                    3 => scanner.observe(ScannerEvent::Indented),
                    4 => scanner.observe(ScannerEvent::Outdented),
                    5 => scanner.observe(ScannerEvent::ArrowIndented),
                    _ => scanner.observe(ScannerEvent::ColonEol { in_template: false }),
                }
            }

            assertion(case, source, &mut scanner);
        }
    }

    fn layout_kinds(source: &str) -> Vec<TokenKind> {
        kinds(source)
            .into_iter()
            .filter(|kind| is_layout_token(*kind))
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
    fn next_yields_eof_once_and_then_stops() {
        let mut scanner = ContextualScanner::new("value").expect("source should scan");
        let mut observed = Vec::new();

        while let Some(token) = scanner.next() {
            observed.push(token.kind);
        }

        assert_eq!(observed, vec![TokenKind::Identifier, TokenKind::Eof]);
        assert!(scanner.next().is_none());
    }

    #[test]
    fn lookahead_beyond_eof_clamps_to_the_eof_token() {
        let mut scanner = ContextualScanner::new("value").expect("source should scan");

        assert_eq!(scanner.lookahead(0).kind, TokenKind::Identifier);
        assert_eq!(scanner.lookahead(1).kind, TokenKind::Eof);
        assert_eq!(scanner.lookahead(usize::MAX).kind, TokenKind::Eof);
    }

    #[test]
    fn advance_does_not_move_past_eof() {
        let mut scanner = ContextualScanner::new("value").expect("source should scan");

        scanner.advance();
        assert_eq!(scanner.current().kind, TokenKind::Eof);
        scanner.advance();
        assert_eq!(scanner.current().kind, TokenKind::Eof);
    }

    #[test]
    fn deterministic_parser_event_sequences_always_reach_eof() {
        for_deterministic_event_sequence(|case, source, scanner| {
            while scanner.next().is_some() {}
            assert_eq!(
                scanner.current().kind,
                TokenKind::Eof,
                "event sequence {case} did not end at EOF for {source:?}"
            );
            assert!(scanner.next().is_none());
        });
    }

    #[test]
    fn deterministic_parser_event_sequences_keep_token_spans_bounded() {
        for_deterministic_event_sequence(|case, source, scanner| {
            for token in scanner.tokens() {
                assert!(
                    token.span.start() <= token.span.end()
                        && token.span.end() <= source.len() as u32,
                    "event sequence {case} produced an invalid span for {source:?}"
                );
            }
        });
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
    fn classifies_colon_after_this_as_colon_follow() {
        assert_eq!(
            kinds("this: T"),
            vec![
                TokenKind::Keyword(HardKeyword::This),
                TokenKind::ColonFollow,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn classifies_colon_after_super_as_colon_follow() {
        assert_eq!(
            kinds("super: T"),
            vec![
                TokenKind::Keyword(HardKeyword::Super),
                TokenKind::ColonFollow,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn classifies_colon_after_new_as_colon_follow() {
        assert_eq!(
            kinds("new: T"),
            vec![
                TokenKind::Keyword(HardKeyword::New),
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
    fn recognizes_an_end_marker_before_a_trailing_line_comment() {
        assert_eq!(
            kinds("value\nend if // trailing comment\nnext"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recognizes_an_end_marker_after_a_multiline_comment() {
        assert_eq!(
            kinds("value /* multiline\ncomment */\nend if\nnext"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newlines,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recognizes_an_end_marker_inside_braces() {
        assert_eq!(
            kinds("{\n  value\n  end if\n}"),
            vec![
                TokenKind::Punctuation(Punctuation::LeftBrace),
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Punctuation(Punctuation::RightBrace),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recognizes_an_end_marker_when_a_comment_splits_the_marker() {
        assert_eq!(
            kinds("value\nend /* comment */ if\nnext"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn closes_nested_regions_before_their_end_markers() {
        assert_eq!(
            kinds(
                "if outer then\n  if inner then\n    inner_value\n  end if\n  after_inner()\nend if\nafter_outer()"
            ),
            vec![
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Then),
                TokenKind::Indent,
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Then),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
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
    fn recognizes_throw_as_an_end_marker_target() {
        assert_eq!(
            kinds("value\nend throw"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::EndMarker,
                TokenKind::Keyword(HardKeyword::Throw),
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
    fn inserts_a_newline_after_a_type_keyword() {
        assert_eq!(
            kinds("value\nend type\nnext"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Type),
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn inserts_a_newline_after_return() {
        assert_eq!(
            kinds("return\nnext"),
            vec![
                TokenKind::Keyword(HardKeyword::Return),
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn inserts_a_newline_after_standalone_given() {
        assert_eq!(
            kinds("given\nnext"),
            vec![
                TokenKind::Keyword(HardKeyword::Given),
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn suppresses_a_separator_before_then_after_a_blank_line() {
        assert_eq!(
            kinds("value\n\nthen\nnext"),
            vec![
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Then),
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn suppresses_a_separator_before_with_after_a_blank_line() {
        assert_eq!(
            kinds("value\n\nwith\nnext"),
            vec![
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::With),
                TokenKind::Identifier,
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
    fn indented_feedback_after_a_newline_token_opens_a_layout_region() {
        let mut scanner =
            ContextualScanner::new("receiver\n  def method = body").expect("source scans");
        scanner.advance();
        assert_eq!(scanner.current().kind, TokenKind::Newline);

        scanner.observe(ScannerEvent::Indented);

        assert_eq!(scanner.lookahead(1).kind, TokenKind::Indent);
        scanner.advance();
        assert_eq!(scanner.current().kind, TokenKind::Indent);
    }

    #[test]
    fn parser_feedback_closes_nested_indentation_regions_in_order() {
        let mut scanner =
            ContextualScanner::new("root:\n  child:\n    leaf\nback").expect("source scans");

        scanner.advance();
        scanner.observe(ScannerEvent::ColonEol { in_template: false });
        scanner.observe(ScannerEvent::Indented);
        scanner.advance();
        scanner.advance();
        scanner.advance();
        scanner.observe(ScannerEvent::ColonEol { in_template: false });
        scanner.observe(ScannerEvent::Indented);
        scanner.advance();
        scanner.advance();
        scanner.advance();
        scanner.advance();
        scanner.observe(ScannerEvent::Outdented);
        scanner.advance();
        scanner.observe(ScannerEvent::Outdented);

        assert_eq!(
            scanner
                .tokens()
                .iter()
                .map(|token| token.kind)
                .collect::<Vec<_>>(),
            vec![
                TokenKind::Identifier,
                TokenKind::ColonEol,
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::ColonEol,
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::Outdent,
                TokenKind::Outdent,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn parser_feedback_does_not_open_indent_for_an_aligned_next_line() {
        let mut scanner = ContextualScanner::new("root:\nchild").expect("source scans");

        scanner.advance();
        scanner.observe(ScannerEvent::ColonEol { in_template: false });
        scanner.observe(ScannerEvent::Indented);

        assert_eq!(
            scanner
                .tokens()
                .iter()
                .map(|token| token.kind)
                .collect::<Vec<_>>(),
            vec![
                TokenKind::Identifier,
                TokenKind::ColonEol,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn parser_feedback_does_not_emit_outdent_at_root() {
        let mut scanner = ContextualScanner::new("root\nnext").expect("source scans");

        scanner.advance();
        scanner.observe(ScannerEvent::Outdented);

        assert_eq!(
            scanner
                .tokens()
                .iter()
                .map(|token| token.kind)
                .collect::<Vec<_>>(),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn parser_feedback_closes_a_region_when_current_is_a_newline() {
        let mut scanner = ContextualScanner::new("root:\n  child\nnext").expect("source scans");

        scanner.advance();
        scanner.observe(ScannerEvent::ColonEol { in_template: false });
        scanner.observe(ScannerEvent::Indented);
        scanner.advance();
        scanner.advance();
        scanner.advance();
        assert_eq!(scanner.current().kind, TokenKind::Newline);

        scanner.observe(ScannerEvent::Outdented);

        assert_eq!(scanner.current().kind, TokenKind::Outdent);
        scanner.advance();
        assert_eq!(scanner.current().kind, TokenKind::Newline);
    }

    #[test]
    fn parser_feedback_closes_nested_regions_before_eof() {
        let mut scanner =
            ContextualScanner::new("root:\n  child:\n    leaf").expect("source scans");

        scanner.advance();
        scanner.observe(ScannerEvent::ColonEol { in_template: false });
        scanner.observe(ScannerEvent::Indented);
        scanner.advance();
        scanner.advance();
        scanner.advance();
        scanner.observe(ScannerEvent::ColonEol { in_template: false });
        scanner.observe(ScannerEvent::Indented);
        scanner.advance();
        scanner.advance();
        scanner.advance();
        scanner.advance();
        scanner.observe(ScannerEvent::Outdented);
        scanner.advance();
        scanner.observe(ScannerEvent::Outdented);

        assert_eq!(
            scanner
                .tokens()
                .iter()
                .map(|token| token.kind)
                .collect::<Vec<_>>(),
            vec![
                TokenKind::Identifier,
                TokenKind::ColonEol,
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::ColonEol,
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Outdent,
                TokenKind::Outdent,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn colon_eol_feedback_reclassifies_colon_after_an_operator() {
        let mut scanner = ContextualScanner::new("value + : next").expect("source scans");
        scanner.advance();
        scanner.advance();

        assert_eq!(scanner.current().kind, TokenKind::ColonOp);
        scanner.observe(ScannerEvent::ColonEol { in_template: false });
        assert_eq!(scanner.current().kind, TokenKind::ColonOp);
    }

    #[test]
    fn colon_eol_feedback_does_not_reclassify_a_same_line_colon() {
        let mut scanner = ContextualScanner::new("object Foo: Int").expect("source scans");
        scanner.advance();
        scanner.advance();

        assert_eq!(scanner.current().kind, TokenKind::ColonFollow);
        scanner.observe(ScannerEvent::ColonEol { in_template: false });
        assert_eq!(scanner.current().kind, TokenKind::ColonFollow);
    }

    #[test]
    fn colon_eol_feedback_reclassifies_an_operator_colon_in_a_template() {
        let mut scanner = ContextualScanner::new("value + :\n  next").expect("source scans");
        scanner.advance();
        scanner.advance();

        assert_eq!(scanner.current().kind, TokenKind::ColonOp);
        scanner.observe(ScannerEvent::ColonEol { in_template: true });
        assert_eq!(scanner.current().kind, TokenKind::ColonEol);
    }

    #[test]
    fn repeated_indented_feedback_does_not_duplicate_an_indent_token() {
        let mut scanner = ContextualScanner::new("object Foo:\n  val x = 1").expect("source scans");
        scanner.advance();
        scanner.advance();

        scanner.observe(ScannerEvent::Indented);
        scanner.observe(ScannerEvent::Indented);

        assert_eq!(
            scanner
                .tokens()
                .iter()
                .filter(|token| token.kind == TokenKind::Indent)
                .count(),
            1
        );
    }

    #[test]
    fn repeated_outdented_feedback_does_not_duplicate_an_outdent_token() {
        let mut scanner =
            ContextualScanner::new("object Foo:\n  val x = 1\nnext").expect("source scans");
        scanner.advance();
        scanner.advance();
        scanner.observe(ScannerEvent::Indented);
        for _ in 0..7 {
            scanner.advance();
        }

        assert_eq!(scanner.current().kind, TokenKind::Identifier);
        scanner.observe(ScannerEvent::Outdented);
        scanner.observe(ScannerEvent::Outdented);

        assert_eq!(
            scanner
                .tokens()
                .iter()
                .filter(|token| token.kind == TokenKind::Outdent)
                .count(),
            1
        );
    }

    #[test]
    fn parser_outdent_does_not_close_an_automatic_region() {
        let mut scanner = ContextualScanner::new("if ready then\n  body").expect("source scans");
        scanner.advance();
        scanner.advance();
        scanner.advance();
        scanner.advance();
        assert_eq!(scanner.current().kind, TokenKind::Identifier);

        scanner.observe(ScannerEvent::Outdented);

        assert_eq!(
            scanner
                .tokens()
                .iter()
                .map(|token| token.kind)
                .collect::<Vec<_>>(),
            vec![
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Then),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Outdent,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn template_feedback_outdent_closes_before_an_outer_brace() {
        let source = "{\n  class A:\n    val x = 1\n}";
        let mut scanner = ContextualScanner::new(source).expect("source scans");
        while scanner.current().kind != TokenKind::ColonFollow {
            scanner.advance();
        }

        scanner.observe(ScannerEvent::ColonEol { in_template: true });
        scanner.observe(ScannerEvent::Indented);
        scanner.advance();
        while scanner.current().kind != TokenKind::Punctuation(Punctuation::RightBrace) {
            scanner.advance();
        }

        scanner.observe(ScannerEvent::Outdented);

        assert_eq!(scanner.current().kind, TokenKind::Outdent);
        assert_eq!(
            scanner.lookahead(1).kind,
            TokenKind::Punctuation(Punctuation::RightBrace)
        );
    }

    #[test]
    fn parser_outdent_closes_a_feedback_region_after_an_automatic_nested_region() {
        let mut scanner = ContextualScanner::new("root:\n  if ready then\n    leaf\n  back")
            .expect("source scans");
        scanner.advance();
        scanner.observe(ScannerEvent::ColonEol { in_template: false });
        scanner.observe(ScannerEvent::Indented);

        while scanner.current().kind != TokenKind::Identifier
            || scanner
                .tokens()
                .get(scanner.current_index())
                .and_then(|token| {
                    scanner
                        .source
                        .get(token.span.start() as usize..token.span.end() as usize)
                })
                != Some("back")
        {
            scanner.advance();
        }
        scanner.observe(ScannerEvent::Outdented);

        assert_eq!(
            scanner
                .tokens()
                .iter()
                .map(|token| token.kind)
                .collect::<Vec<_>>(),
            vec![
                TokenKind::Identifier,
                TokenKind::ColonEol,
                TokenKind::Indent,
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Then),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Outdent,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
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
    fn arrow_indented_feedback_restores_peer_separators_inside_parentheses() {
        let source = "(\n  value match {\n    case A =>\n      first()\n      second()\n    case B => done()\n  }\n)";
        let mut scanner = ContextualScanner::new(source).expect("source scans");
        while !(scanner.current().kind == TokenKind::Operator
            && &source
                [scanner.current().span.start() as usize..scanner.current().span.end() as usize]
                == "=>")
        {
            scanner.advance();
        }

        scanner.observe(ScannerEvent::ArrowIndented);

        let first_end = source.find("first()").unwrap() as u32 + "first()".len() as u32;
        let second_start = source.find("second()").unwrap() as u32;
        assert!(scanner.tokens().iter().any(|token| {
            token.kind == TokenKind::Newline
                && token.span.start() == first_end
                && token.span.end() == second_start
        }));
    }

    #[test]
    fn arrow_indented_feedback_keeps_a_leading_infix_line_in_the_expression() {
        let source = "(case A =>\n  start\n    + continuation\n  finish)";
        let mut scanner = ContextualScanner::new(source).expect("source scans");
        while !(scanner.current().kind == TokenKind::Operator
            && &source
                [scanner.current().span.start() as usize..scanner.current().span.end() as usize]
                == "=>")
        {
            scanner.advance();
        }

        scanner.observe(ScannerEvent::ArrowIndented);

        let start_end = source.find("start").unwrap() as u32 + "start".len() as u32;
        let operator_start = source.find('+').unwrap() as u32;
        let continuation_end =
            source.find("continuation").unwrap() as u32 + "continuation".len() as u32;
        let finish_start = source.find("finish").unwrap() as u32;
        assert!(!scanner.tokens().iter().any(|token| {
            token.kind == TokenKind::Newline
                && token.span.start() == start_end
                && token.span.end() == operator_start
        }));
        assert!(scanner.tokens().iter().any(|token| {
            token.kind == TokenKind::Newline
                && token.span.start() == continuation_end
                && token.span.end() == finish_start
        }));
    }

    #[test]
    fn arrow_indented_feedback_splits_a_leading_prefix_operator_without_spacing() {
        let source = "(case A =>\n  first\n  !second\n  finish)";
        let mut scanner = ContextualScanner::new(source).expect("source scans");
        while !(scanner.current().kind == TokenKind::Operator
            && &source
                [scanner.current().span.start() as usize..scanner.current().span.end() as usize]
                == "=>")
        {
            scanner.advance();
        }

        scanner.observe(ScannerEvent::ArrowIndented);

        let first_end = source.find("first").unwrap() as u32 + "first".len() as u32;
        let prefix_start = source.find("!second").unwrap() as u32;
        assert!(
            scanner.tokens().iter().any(|token| {
                token.kind == TokenKind::Newline
                    && token.span.start() == first_end
                    && token.span.end() == prefix_start
            }),
            "tokens: {:#?}",
            scanner.tokens()
        );
    }

    #[test]
    fn arrow_indented_feedback_does_not_split_a_deeper_assignment_continuation() {
        let source = "(case A =>\n  result =\n    value\n  next)";
        let mut scanner = ContextualScanner::new(source).expect("source scans");
        while !(scanner.current().kind == TokenKind::Operator
            && &source
                [scanner.current().span.start() as usize..scanner.current().span.end() as usize]
                == "=>")
        {
            scanner.advance();
        }

        scanner.observe(ScannerEvent::ArrowIndented);

        let assignment_end = source.rfind(" =").unwrap() as u32 + 2;
        let value_start = source.find("value").unwrap() as u32;
        let value_end = value_start + "value".len() as u32;
        let next_start = source.find("next").unwrap() as u32;
        assert!(!scanner.tokens().iter().any(|token| {
            matches!(token.kind, TokenKind::Newline | TokenKind::Newlines)
                && token.span.start() == assignment_end
                && token.span.end() == value_start
        }));
        assert!(scanner.tokens().iter().any(|token| {
            token.kind == TokenKind::Newline
                && token.span.start() == value_end
                && token.span.end() == next_start
        }));
    }

    #[test]
    fn arrow_indented_feedback_ignores_a_non_arrow_operator() {
        let mut scanner = ContextualScanner::new("value +\n  next").expect("source scans");
        scanner.advance();

        assert_eq!(scanner.current().kind, TokenKind::Operator);
        scanner.observe(ScannerEvent::ArrowIndented);

        assert_eq!(
            scanner
                .tokens()
                .iter()
                .map(|token| token.kind)
                .collect::<Vec<_>>(),
            vec![
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn self_arrow_feedback_does_not_open_a_nested_body_region() {
        let mut scanner = ContextualScanner::new("self =>\n  body").expect("source scans");
        scanner.advance();

        assert_eq!(scanner.current().kind, TokenKind::Operator);
        scanner.observe(ScannerEvent::SelfArrow);
        scanner.advance();

        assert_eq!(scanner.current().kind, TokenKind::Identifier);
        assert!(
            !scanner
                .tokens()
                .iter()
                .any(|token| token.kind == TokenKind::Indent)
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
    fn inserts_a_newline_after_a_trailing_operator() {
        assert_eq!(
            kinds("left +\n  right"),
            vec![
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn inserts_a_newline_after_a_trailing_operator_before_a_line_comment() {
        assert_eq!(
            kinds("left + // comment\n  right"),
            vec![
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Newline,
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
    fn opens_and_closes_an_indentation_region_after_assignment() {
        assert_eq!(
            kinds("val assigned =\n  value\nafter_assignment"),
            vec![
                TokenKind::Keyword(HardKeyword::Val),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn opens_and_closes_an_indentation_region_after_return() {
        assert_eq!(
            kinds("return\n  value\nafter"),
            vec![
                TokenKind::Keyword(HardKeyword::Return),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn opens_and_closes_an_indentation_region_after_throw() {
        assert_eq!(
            kinds("throw\n  failure\nafter"),
            vec![
                TokenKind::Keyword(HardKeyword::Throw),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn opens_and_closes_an_indentation_region_after_a_for_arrow() {
        assert_eq!(
            kinds("for item <-\n  items\nyield item"),
            vec![
                TokenKind::Keyword(HardKeyword::For),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Outdent,
                TokenKind::Keyword(HardKeyword::Yield),
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn opens_and_closes_an_indentation_region_after_a_standalone_operator() {
        assert_eq!(
            kinds("value =\n  next_value\nafter_expression"),
            vec![
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Identifier,
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
    fn closes_nested_if_regions_before_each_else_clause() {
        assert_eq!(
            layout_kinds(
                "if outer then\n  if inner then\n    inner\n  else\n    alternative\n  after_inner\nelse\n  fallback\nafter_if"
            ),
            vec![
                TokenKind::Indent,
                TokenKind::Indent,
                TokenKind::Outdent,
                TokenKind::Indent,
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Outdent,
                TokenKind::Indent,
                TokenKind::Outdent,
                TokenKind::Newline,
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
    fn closes_nested_try_and_case_regions_before_finally() {
        assert_eq!(
            layout_kinds(
                "try\n  risky()\ncatch\n  case error =>\n    recover()\nfinally\n  cleanup()\nafter_try"
            ),
            vec![
                TokenKind::Indent,
                TokenKind::Outdent,
                TokenKind::Indent,
                TokenKind::Indent,
                TokenKind::Outdent,
                TokenKind::Outdent,
                TokenKind::Indent,
                TokenKind::Outdent,
                TokenKind::Newline,
            ]
        );
    }

    #[test]
    fn closes_a_for_region_without_a_separator_before_yield() {
        assert_eq!(
            kinds("for item <- items do\n  item\nyield item\nafter_for()"),
            vec![
                TokenKind::Keyword(HardKeyword::For),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Do),
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Outdent,
                TokenKind::Keyword(HardKeyword::Yield),
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn opens_and_closes_a_given_with_region() {
        assert_eq!(
            kinds("given Service with\n  service_value\nafter_given()"),
            vec![
                TokenKind::Keyword(HardKeyword::Given),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::With),
                TokenKind::Indent,
                TokenKind::Identifier,
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
    fn closes_nested_given_and_then_regions_before_the_following_statement() {
        assert_eq!(
            layout_kinds(
                "given Service with\n  if ready then\n    service_value\n  service_after\nafter_given"
            ),
            vec![
                TokenKind::Indent,
                TokenKind::Indent,
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Outdent,
                TokenKind::Newline,
            ]
        );
    }

    #[test]
    fn opens_and_closes_a_while_do_region() {
        assert_eq!(
            kinds("while condition do\n  work()\nafter_while()"),
            vec![
                TokenKind::Keyword(HardKeyword::While),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Do),
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
    fn closes_nested_while_and_if_regions_before_the_following_statement() {
        assert_eq!(
            layout_kinds(
                "while condition do\n  if nested then\n    work()\n  after_work\nafter_while"
            ),
            vec![
                TokenKind::Indent,
                TokenKind::Indent,
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Outdent,
                TokenKind::Newline,
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
    fn treats_form_feed_as_a_logical_line_break() {
        assert_eq!(
            kinds("first\u{000c}second"),
            vec![
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn preserves_substitute_as_an_error_token() {
        assert_eq!(
            kinds("first\u{001a}second"),
            vec![
                TokenKind::Identifier,
                TokenKind::Error,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn uses_form_feed_as_the_start_of_an_indented_line() {
        assert_eq!(
            kinds("if ready then\u{000c}  run()"),
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
    fn does_not_use_substitute_as_the_start_of_an_indented_line() {
        assert_eq!(
            kinds("if ready then\u{001a}  run()"),
            vec![
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Then),
                TokenKind::Error,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
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
    fn opens_a_match_case_region_at_the_match_indentation() {
        assert_eq!(
            kinds("value match\ncase first =>\n  one\ncase second =>\n  two\nafter"),
            vec![
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Match),
                TokenKind::Indent,
                TokenKind::Keyword(HardKeyword::Case),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Keyword(HardKeyword::Case),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keeps_a_case_region_open_for_a_guard_at_case_indentation() {
        assert_eq!(
            kinds("value match\n  case first\n  if ready =>\n    one\n  case second => two\nafter"),
            vec![
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Match),
                TokenKind::Indent,
                TokenKind::Keyword(HardKeyword::Case),
                TokenKind::Identifier,
                TokenKind::Newline,
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Keyword(HardKeyword::Case),
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Identifier,
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn closes_a_case_region_before_an_if_after_its_body() {
        let token_kinds = kinds("value match\ncase first => done\nif ready then next");
        let outdent = token_kinds
            .iter()
            .position(|kind| *kind == TokenKind::Outdent)
            .expect("expected the case region to close");
        let if_token = token_kinds
            .iter()
            .position(|kind| *kind == TokenKind::Keyword(HardKeyword::If))
            .expect("expected following if expression");

        assert!(outdent < if_token);
    }

    #[test]
    fn opens_a_catch_case_region_at_the_catch_indentation() {
        assert_eq!(
            kinds("try\n  risky()\ncatch\ncase error =>\n  recover()\nafter"),
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
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Punctuation(Punctuation::LeftParen),
                TokenKind::Punctuation(Punctuation::RightParen),
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn leading_infix_does_not_close_an_active_assignment_region() {
        assert_eq!(
            kinds("value =\n  foo\n  + bar\nafter"),
            vec![
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Indent,
                TokenKind::Identifier,
                TokenKind::Operator,
                TokenKind::Identifier,
                TokenKind::Outdent,
                TokenKind::Newline,
                TokenKind::Identifier,
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
    fn closes_nested_match_regions_before_the_outer_case_continues() {
        assert_eq!(
            kinds(
                "value match\n  case outer =>\n    value match\n      case inner =>\n        inner()\n    after_inner()\n  case next =>\n    next()\nafter_nested_match()"
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
                TokenKind::Outdent,
                TokenKind::Newline,
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
    fn closes_two_nested_then_regions_before_a_following_statement() {
        assert_eq!(
            kinds("if outer then\n  if inner then\n    body()\n  after_inner()\nafter_outer()"),
            vec![
                TokenKind::Keyword(HardKeyword::If),
                TokenKind::Identifier,
                TokenKind::Keyword(HardKeyword::Then),
                TokenKind::Indent,
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
    fn parser_feedback_indent_is_zero_width_at_the_body_start() {
        let mut scanner = ContextualScanner::new("root:\n  child").expect("source scans");
        scanner.advance();
        scanner.observe(ScannerEvent::ColonEol { in_template: false });
        scanner.observe(ScannerEvent::Indented);

        let tokens = scanner.tokens();
        let indent_index = tokens
            .iter()
            .position(|token| token.kind == TokenKind::Indent)
            .expect("indent token");
        let indent = &tokens[indent_index];
        let child = &tokens[indent_index + 1];

        assert_eq!(child.kind, TokenKind::Identifier);
        assert_eq!(
            indent.span,
            TextRange::new(child.span.start(), child.span.start()).unwrap()
        );
    }

    #[test]
    fn parser_feedback_outdent_is_zero_width_at_the_closing_token() {
        let mut scanner = ContextualScanner::new("root:\n  child\nback").expect("source scans");
        scanner.advance();
        scanner.observe(ScannerEvent::ColonEol { in_template: false });
        scanner.observe(ScannerEvent::Indented);
        scanner.advance();
        scanner.advance();
        scanner.advance();
        scanner.advance();
        scanner.observe(ScannerEvent::Outdented);

        let tokens = scanner.tokens();
        let outdent_index = tokens
            .iter()
            .position(|token| token.kind == TokenKind::Outdent)
            .expect("outdent token");
        let outdent = &tokens[outdent_index];
        let back = &tokens[outdent_index + 1];

        assert_eq!(back.kind, TokenKind::Identifier);
        assert_eq!(
            outdent.span,
            TextRange::new(back.span.start(), back.span.start()).unwrap()
        );
    }

    #[test]
    fn parser_feedback_nested_outdents_at_eof_share_the_eof_offset() {
        let source = "root:\n  child:\n    leaf";
        let mut scanner = ContextualScanner::new(source).expect("source scans");
        scanner.advance();
        scanner.observe(ScannerEvent::ColonEol { in_template: false });
        scanner.observe(ScannerEvent::Indented);
        scanner.advance();
        scanner.advance();
        scanner.advance();
        scanner.observe(ScannerEvent::ColonEol { in_template: false });
        scanner.observe(ScannerEvent::Indented);
        scanner.advance();
        scanner.advance();
        scanner.advance();
        scanner.advance();
        scanner.observe(ScannerEvent::Outdented);
        scanner.advance();
        scanner.observe(ScannerEvent::Outdented);

        let eof = scanner
            .tokens()
            .iter()
            .find(|token| token.kind == TokenKind::Eof)
            .expect("EOF token");
        let outdents: Vec<_> = scanner
            .tokens()
            .iter()
            .filter(|token| token.kind == TokenKind::Outdent)
            .collect();

        assert_eq!(outdents.len(), 2);
        assert!(outdents.iter().all(|token| token.span == eof.span));
        assert_eq!(
            eof.span,
            TextRange::new(source.len() as u32, source.len() as u32).unwrap()
        );
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
    fn forwards_malformed_cdata_diagnostic_and_reaches_the_following_xml_literal() {
        let source = "<root><![CDATA[text]]x></root>\n<ok/>";
        let scanner = ContextualScanner::new(source).expect("source scans");
        let following_xml = source.find("<ok/>").expect("following XML literal") as u32;

        assert_eq!(scanner.diagnostics().len(), 1);
        assert_eq!(
            scanner.diagnostics()[0].message(),
            "unterminated XML CDATA section"
        );
        assert!(
            scanner
                .tokens()
                .iter()
                .any(|token| token.kind == TokenKind::XmlStart
                    && token.span.start() == following_xml)
        );
        assert_eq!(
            scanner.tokens().last().map(|token| token.kind),
            Some(TokenKind::Eof)
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
