use dotty_core::{
    AstArena, HardKeyword, NameInterner, SourceId, SourceSpan, SourceText, SourceTextError, Span,
    TermName, TextRange, Token, TokenKind, TokenSource, Tree, TreeId, TreeKind, TypeName, Untyped,
};

use dotty_core::ScannerEvent;
use dotty_core::ast::{ErrorNode, ErrorNodeKind, UntypedNode};
use std::collections::HashSet;

use crate::ParserFeatures;
use crate::{
    Cursor, KnownNames, Location, Mark, ParamOwner, ParseContext, ParseDiagnostic,
    ParseDiagnosticKind, ParseKind, RecoverySet,
};

/// Stateful input and allocation context for the handwritten parser.
pub struct Parser<'src, 'names, S>
where
    S: TokenSource,
{
    pub(crate) cursor: Cursor<S>,
    pub(crate) source: SourceText<'src>,
    pub(crate) source_id: SourceId,
    pub(crate) names: &'names mut NameInterner,
    pub(crate) ast: AstArena<Untyped>,
    pub(crate) last_real_token_end: u32,
    pub(crate) context: ParseContext,
    /// Whether the current statement sequence is Dotty's outermost import
    /// position, where global language imports are permitted without a
    /// placement diagnostic.
    pub(crate) outermost_imports_allowed: bool,
    pub(crate) diagnostics: Vec<ParseDiagnostic>,
    pub(crate) known_names: KnownNames,
    pub(crate) next_wildcard_param: u32,
    pub(crate) next_wildcard_type_param: u32,
    pub(crate) wildcard_type_depth: u32,
    pub(crate) placeholder_params: Vec<TreeId<Untyped>>,
    pub(crate) last_advance_was_outdent: bool,
    /// The scanner suppressed a physical newline after a malformed construct.
    pub(crate) last_advance_consumed_statement_separator: bool,
    /// Indentation offset for a statement sequence opened through scanner
    /// feedback. Nested grammar such as a lambda body must keep notifying the
    /// scanner so it can emit the matching outdent at the right boundary.
    pub(crate) feedback_block_indent: Option<u32>,
    /// AST constructs already closed by an explicit Scala `end` marker.
    pub(crate) end_marked_trees: HashSet<TreeId<Untyped>>,
    /// Active enclosing constructs that can own Scala `end` markers, from
    /// outermost to innermost. Nested templates return matching markers to
    /// these owners.
    pub(crate) end_marker_owners: Vec<Option<dotty_core::Name>>,
    /// Structural expression bodies whose matching `end` marker may close
    /// their active indentation region before the complete AST node exists.
    pub(crate) active_end_marker_targets: Vec<(HardKeyword, u32)>,
    /// Active quoted expression bodies; `$` followed by `{` is a splice only
    /// while this depth is nonzero.
    pub(crate) expression_quote_depth: u32,
    /// Active quote bodies parsed from pattern position. Braced splices in
    /// these bodies contain patterns and must remain source-level pattern
    /// nodes rather than expression splices.
    pub(crate) quote_pattern_depth: u32,
    /// Active quoted type bodies; braced legacy type splices are diagnosed
    /// while this depth is nonzero.
    pub(crate) type_quote_depth: u32,
}

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: TokenSource,
{
    /// Creates a parser state over a source-backed token stream.
    pub fn new(
        source: SourceText<'src>,
        source_id: SourceId,
        tokens: S,
        names: &'names mut NameInterner,
    ) -> Self {
        let cursor = Cursor::new(tokens);
        let known_names = KnownNames::new(names);
        let last_real_token_end = if is_zero_width_synthetic(cursor.kind()) {
            cursor.current().span.start()
        } else {
            0
        };

        Self {
            cursor,
            source,
            source_id,
            names,
            ast: AstArena::new(),
            last_real_token_end,
            context: ParseContext::default(),
            outermost_imports_allowed: false,
            diagnostics: Vec::new(),
            known_names,
            next_wildcard_param: 0,
            next_wildcard_type_param: 0,
            wildcard_type_depth: 0,
            placeholder_params: Vec::new(),
            last_advance_was_outdent: false,
            last_advance_consumed_statement_separator: false,
            feedback_block_indent: None,
            end_marked_trees: HashSet::new(),
            end_marker_owners: Vec::new(),
            active_end_marker_targets: Vec::new(),
            expression_quote_depth: 0,
            quote_pattern_depth: 0,
            type_quote_depth: 0,
        }
    }

    /// Returns the source file identifier associated with this parser.
    pub const fn source_id(&self) -> SourceId {
        self.source_id
    }

    /// Returns the current parser-facing token.
    pub fn current(&self) -> &Token {
        self.cursor.current()
    }

    /// Returns the current token's source span.
    pub fn current_span(&self) -> SourceSpan {
        SourceSpan::new(self.source_id, Span::without_point(self.current().span))
    }

    /// Returns the current parser context.
    pub const fn context(&self) -> &ParseContext {
        &self.context
    }

    /// Returns the diagnostics accumulated by this parser.
    pub fn diagnostics(&self) -> &[ParseDiagnostic] {
        &self.diagnostics
    }

    /// Returns parser-known soft keyword names.
    pub const fn known_names(&self) -> &KnownNames {
        &self.known_names
    }

    /// Returns the dialect/feature policy for future contextual grammar.
    pub const fn features(&self) -> &ParserFeatures {
        &self.context.features
    }

    /// Configures feature-dependent grammar without changing tokenization.
    pub fn with_features(mut self, features: ParserFeatures) -> Self {
        self.context.features = features;
        self
    }

    /// Checks the current source spelling against a parser-known name.
    pub fn current_is_known_name(&mut self, expected: TermName) -> Result<bool, SourceTextError> {
        Ok(self.intern_current_term_name()? == expected)
    }

    pub(crate) fn current_starts_braced_splice(&mut self) -> bool {
        self.current().kind == TokenKind::Identifier
            && self.source.slice(self.current().span).ok() == Some("$")
            && self.cursor.lookahead(1).kind
                == TokenKind::Punctuation(dotty_core::Punctuation::LeftBrace)
    }

    pub(crate) fn current_starts_simple_splice(&self) -> bool {
        self.current().kind == TokenKind::Identifier
            && self
                .source
                .slice(self.current().span)
                .is_ok_and(|spelling| spelling.starts_with('$') && spelling.len() > 1)
    }

    /// Consumes the diagnostics accumulated by this parser.
    pub fn take_diagnostics(&mut self) -> Vec<ParseDiagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    /// Runs a nested parse with a temporary syntactic location.
    pub fn with_location<T>(
        &mut self,
        location: Location,
        parse: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let previous = self.context.location;
        self.context.location = location;
        let result = parse(self);
        self.context.location = previous;
        result
    }

    /// Runs a statement sequence with Dotty's outermost-import policy.
    pub(crate) fn with_outermost_imports_allowed<T>(
        &mut self,
        allowed: bool,
        parse: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let previous = self.outermost_imports_allowed;
        self.outermost_imports_allowed = allowed;
        let result = parse(self);
        self.outermost_imports_allowed = previous;
        result
    }

    /// Runs a nested type parse in a type-argument position.
    pub(crate) fn with_type_argument<T>(
        &mut self,
        wild_ok: bool,
        parse: impl FnOnce(&mut Self) -> T,
    ) -> T {
        if wild_ok {
            self.wildcard_type_depth += 1;
        }
        let result = parse(self);
        if wild_ok {
            self.wildcard_type_depth -= 1;
        }
        result
    }

    /// Parses a type position in which a wildcard type is permitted by the
    /// surrounding grammar, without treating it as a type argument.
    pub(crate) fn with_wildcard_type_allowed<T>(
        &mut self,
        parse: impl FnOnce(&mut Self) -> T,
    ) -> T {
        self.wildcard_type_depth += 1;
        let result = parse(self);
        self.wildcard_type_depth -= 1;
        result
    }

    pub(crate) const fn allows_wildcard_type(&self) -> bool {
        self.wildcard_type_depth > 0
    }

    /// Runs a nested parse with a temporary grammar category.
    pub fn with_parse_kind<T>(
        &mut self,
        parse_kind: ParseKind,
        parse: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let previous = self.context.parse_kind;
        self.context.parse_kind = parse_kind;
        let result = parse(self);
        self.context.parse_kind = previous;
        result
    }

    /// Runs a nested parse with a temporary parameter owner.
    pub fn with_param_owner<T>(
        &mut self,
        param_owner: Option<ParamOwner>,
        parse: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let previous = self.context.param_owner;
        self.context.param_owner = param_owner;
        let result = parse(self);
        self.context.param_owner = previous;
        result
    }

    /// Runs a nested parse with a temporary enclosing block delimiter.
    pub(crate) fn with_block_end<T>(
        &mut self,
        block_end: Option<TokenKind>,
        parse: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let previous = self.context.block_end;
        self.context.block_end = block_end;
        let result = parse(self);
        self.context.block_end = previous;
        result
    }

    /// Runs a nested parse with the active scanner-feedback block boundary.
    pub(crate) fn with_feedback_block_indent<T>(
        &mut self,
        indent_offset: Option<u32>,
        parse: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let previous = self.feedback_block_indent;
        self.feedback_block_indent = indent_offset;
        let result = parse(self);
        self.feedback_block_indent = previous;
        result
    }

    /// Runs a nested parse with case/catch-body boundaries enabled.
    pub(crate) fn with_case_body<T>(&mut self, parse: impl FnOnce(&mut Self) -> T) -> T {
        let previous = self.context.case_body;
        self.context.case_body = true;
        let result = parse(self);
        self.context.case_body = previous;
        result
    }

    /// Runs a nested template parse with the requested enum-case policy.
    pub(crate) fn with_enum_body<T>(
        &mut self,
        enabled: bool,
        parse: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let previous = self.context.enum_body;
        self.context.enum_body = enabled;
        let result = parse(self);
        self.context.enum_body = previous;
        result
    }

    /// Runs a nested parse with the requested secondary-constructor policy.
    pub(crate) fn with_secondary_constructor_allowed<T>(
        &mut self,
        allowed: bool,
        parse: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let previous = self.context.secondary_constructor_allowed;
        self.context.secondary_constructor_allowed = allowed;
        let result = parse(self);
        self.context.secondary_constructor_allowed = previous;
        result
    }

    /// Forwards a scanner feedback event.
    pub fn observe(&mut self, event: ScannerEvent) {
        self.cursor.observe(event);
    }

    /// Tells the scanner that a colon ended the current line.
    pub fn observe_colon_eol(&mut self, in_template: bool) {
        self.observe(ScannerEvent::ColonEol { in_template });
    }

    /// Tells the scanner that an indented region was entered.
    pub fn observe_indented(&mut self) {
        self.observe(ScannerEvent::Indented);
    }

    /// Requests an indentation token after the current body introducer and
    /// reports whether scanner feedback inserted one. An already-present
    /// eager `Indent` belongs to the scanner's ordinary layout pass.
    pub(crate) fn observe_indented_body(&mut self) -> bool {
        self.observe_indented_body_region().is_some()
    }

    /// Requests an indented body and returns the token offset that identifies
    /// the feedback region when this call opens one.
    pub(crate) fn observe_indented_body_region(&mut self) -> Option<u32> {
        self.observe_indented_body_region_with(ScannerEvent::Indented)
    }

    /// Requests an indented body relative to a grammar-owned header start.
    /// Multiline template headers can end at the same physical indentation as
    /// their bodies, so the body must be compared with the declaration line.
    pub(crate) fn observe_indented_body_region_from(
        &mut self,
        reference_offset: u32,
    ) -> Option<u32> {
        self.observe_indented_body_region_with(ScannerEvent::IndentedFrom { reference_offset })
    }

    fn observe_indented_body_region_with(&mut self, event: ScannerEvent) -> Option<u32> {
        let already_indented = self.next_indented_region_offset().is_some();
        self.observe(event);
        if already_indented {
            return None;
        }
        self.next_indented_region_offset()
    }

    fn next_indented_region_offset(&mut self) -> Option<u32> {
        let mut lookahead = 1;
        while matches!(
            self.cursor.lookahead(lookahead).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            lookahead += 1;
        }
        (1..=lookahead)
            .find(|offset| self.cursor.lookahead(*offset).kind == TokenKind::Indent)
            .map(|offset| self.cursor.lookahead(offset).span.start())
    }

    /// Closes the named active layout region if nested parsing has not already
    /// consumed its outdent.
    pub(crate) fn observe_outdented_region(&mut self, indent_offset: u32) {
        self.observe(ScannerEvent::OutdentedRegion { indent_offset });
    }

    /// Requests closure of a grammar-owned case/body region. Unlike ordinary
    /// parser feedback, this may refer to an eager scanner `Indent`.
    pub(crate) fn observe_outdented_layout_region(&mut self, indent_offset: u32) {
        self.observe(ScannerEvent::OutdentedLayoutRegion { indent_offset });
    }

    /// Closes the exact parser-requested match-case region after its grammar
    /// has stopped consuming clauses. This is distinct from a physical
    /// outdent: in braced scopes the following statement can align with the
    /// `case` clauses while still being outside the match expression.
    pub(crate) fn observe_match_cases_closed(&mut self, indent_offset: u32) {
        self.observe(ScannerEvent::MatchCasesClosed { indent_offset });
    }

    /// Identifies the match-case indentation, requesting parser feedback only
    /// when the scanner has not already emitted the region's `Indent`.
    pub(crate) fn observe_match_cases_indented(&mut self) -> Option<(u32, bool)> {
        if let Some(indent_offset) = self.next_indented_region_offset() {
            return Some((indent_offset, false));
        }
        self.observe(ScannerEvent::MatchCasesIndented);
        self.next_indented_region_offset()
            .map(|indent_offset| (indent_offset, true))
    }

    pub(crate) fn observe_case_clause_started(&mut self, case_start: u32) {
        self.observe(ScannerEvent::CaseClauseStarted { case_start });
    }

    pub(crate) fn observe_case_clause_ended(&mut self) {
        self.observe(ScannerEvent::CaseClauseEnded);
    }

    /// Tells the scanner that an indented region was exited.
    pub fn observe_outdented(&mut self) {
        self.observe(ScannerEvent::Outdented);
    }

    /// Opens a case-body region relative to the source indentation of `case`.
    /// Returns its indent offset and whether the scanner opened it via feedback.
    pub(crate) fn observe_case_body_indented(&mut self, case_start: u32) -> Option<(u32, bool)> {
        let existing_indent = self.next_indented_region_offset();
        let feedback_indent =
            self.observe_indented_body_region_with(ScannerEvent::CaseBodyIndented { case_start });
        existing_indent
            .map(|indent_offset| (indent_offset, false))
            .or_else(|| feedback_indent.map(|indent_offset| (indent_offset, true)))
    }

    /// Closes a feedback-opened layout region at a grammar boundary without
    /// asking the scanner to insert another parser-visible `Outdent` token.
    pub(crate) fn observe_outdented_by_delimiter(&mut self) {
        self.observe(ScannerEvent::OutdentedByDelimiter);
    }

    pub(crate) fn observe_outdented_by_existing_outdent(&mut self, indent_offset: u32) {
        self.observe(ScannerEvent::OutdentedByExistingOutdent { indent_offset });
    }

    /// Tells the scanner that an indented arrow body was entered.
    pub fn observe_arrow_indented(&mut self) {
        self.observe(ScannerEvent::ArrowIndented);
    }

    pub(crate) fn observe_arrow_indented_body(&mut self) -> Option<u32> {
        let existing_indent = self.next_indented_region_offset();
        self.observe_arrow_indented();
        existing_indent.or_else(|| self.next_indented_region_offset())
    }

    /// Tells the scanner that a template self-type arrow was consumed.
    pub fn observe_self_arrow(&mut self) {
        self.observe(ScannerEvent::SelfArrow);
    }

    /// Advances the parser and records the end of a real token.
    pub fn advance(&mut self) {
        let (kind, end) = {
            let token = self.current();
            (token.kind, token.span.end())
        };
        self.last_advance_was_outdent = kind == TokenKind::Outdent;
        if !is_zero_width_synthetic(kind) && kind != TokenKind::Eof {
            self.last_real_token_end = end;
        }
        self.cursor.advance();
    }

    /// Consumes the current token when it has `kind`.
    pub fn accept(&mut self, kind: TokenKind) -> bool {
        if self.cursor.at(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Reports an expected-token diagnostic without consuming unexpected input.
    pub fn expect(&mut self, kind: TokenKind) -> bool {
        if self.accept(kind) {
            return true;
        }

        self.report(
            ParseDiagnosticKind::ExpectedToken,
            format!("expected {kind:?}, found {:?}", self.current().kind),
        );
        false
    }

    /// Adds a parser diagnostic at the current token.
    pub fn report(&mut self, kind: ParseDiagnosticKind, message: impl Into<String>) {
        let span = self.current_span();
        self.report_at(kind, span, message);
    }

    /// Adds a parser diagnostic at an explicitly selected source span.
    pub(crate) fn report_at(
        &mut self,
        kind: ParseDiagnosticKind,
        span: SourceSpan,
        message: impl Into<String>,
    ) {
        self.diagnostics
            .push(ParseDiagnostic::error(kind, span, message));
    }

    /// Checks and clears placeholders that escaped a complete expression
    /// sequence, preserving any placeholder scope owned by an enclosing
    /// expression.
    pub(crate) fn with_placeholder_scope<T>(&mut self, parse: impl FnOnce(&mut Self) -> T) -> T {
        let saved = std::mem::take(&mut self.placeholder_params);
        let result = parse(self);
        self.report_escaping_placeholders();
        self.placeholder_params = saved;
        result
    }

    pub(crate) fn report_escaping_placeholders(&mut self) {
        if let Some(parameter) = self.placeholder_params.last().copied() {
            let span = self
                .ast
                .get(parameter)
                .position
                .unwrap_or_else(|| self.current_span());
            self.report_at(
                ParseDiagnosticKind::UnboundPlaceholderParameter,
                span,
                "unbound placeholder parameter",
            );
        }
        self.placeholder_params.clear();
    }

    /// Consumes input until a synchronization token or EOF is reached.
    pub fn recover_until(&mut self, set: RecoverySet) {
        while !set.contains(self.current().kind) {
            let checkpoint = self.cursor.checkpoint();
            self.advance();

            // A broken TokenSource may fail to advance. Returning here keeps
            // malformed external input from turning recovery into a hang.
            if !self.cursor.progressed_since(checkpoint) {
                return;
            }
        }
    }

    /// Creates a mark at the current parser position.
    pub fn mark(&self) -> Mark {
        let start = if is_zero_width_synthetic(self.current().kind) {
            self.last_real_token_end
        } else {
            self.current().span.start()
        };
        Mark { start }
    }

    /// Builds a source span from `mark` to the end of the last real token.
    pub fn span_from(&self, mark: Mark) -> SourceSpan {
        let end = self.last_real_token_end.max(mark.start);
        let range = TextRange::new(mark.start, end).expect("span endpoints are ordered");
        SourceSpan::new(self.source_id, Span::without_point(range))
    }

    /// Extends a parsed construct through a trailing layout marker.
    pub(crate) fn extend_tree_end(&mut self, tree: TreeId<Untyped>, end: u32) {
        let Some(position) = self.ast.get(tree).position else {
            return;
        };
        let range = position.span().range();
        let range = TextRange::new(range.start(), end).expect("tree span endpoints are ordered");
        self.ast.get_mut(tree).position =
            Some(SourceSpan::new(self.source_id, Span::without_point(range)));
    }

    /// Returns the source spelling of `token` without allocating.
    pub fn token_text(&self, token: &Token) -> Result<&'src str, SourceTextError> {
        self.source.slice(token.span)
    }

    /// Returns the source spelling of the current token without allocating.
    pub fn current_text(&self) -> Result<&'src str, SourceTextError> {
        let span = self.current().span;
        self.source.slice(span)
    }

    /// Returns whether the current token has the requested source spelling.
    pub(crate) fn current_text_is(&self, expected: &str) -> bool {
        self.current_text().ok() == Some(expected)
    }

    /// Returns whether a token is the ordinary case arrow `=>`.
    pub(crate) fn is_arrow_token(&self, token: &Token) -> bool {
        token.kind == TokenKind::Operator && self.token_text(token).ok() == Some("=>")
    }

    /// Returns whether a token is the context-function arrow `?=>`.
    pub(crate) fn is_context_arrow_token(&self, token: &Token) -> bool {
        token.kind == TokenKind::Operator && self.token_text(token).ok() == Some("?=>")
    }

    /// Returns whether the current token is the ordinary case arrow `=>`.
    pub(crate) fn current_is_arrow(&self) -> bool {
        self.is_arrow_token(self.current())
    }

    /// Returns whether the token at `offset` is the ordinary case arrow `=>`.
    pub(crate) fn lookahead_is_arrow(&mut self, offset: usize) -> bool {
        let token = self.cursor.lookahead(offset).clone();
        self.is_arrow_token(&token)
    }

    /// Returns whether the token at `offset` is the context-function arrow `?=>`.
    pub(crate) fn lookahead_is_context_arrow(&mut self, offset: usize) -> bool {
        let token = self.cursor.lookahead(offset).clone();
        self.is_context_arrow_token(&token)
    }

    /// Returns whether the current token is the context-function arrow `?=>`.
    pub(crate) fn current_is_context_arrow(&self) -> bool {
        self.is_context_arrow_token(self.current())
    }

    /// Returns whether the current token is the deferred type-lambda arrow.
    pub(crate) fn current_is_type_lambda_arrow(&self) -> bool {
        self.current().kind == TokenKind::Operator && self.current_text_is("=>>")
    }

    /// Returns whether the current operator is reserved for a later grammar layer.
    pub(crate) fn current_is_structural_operator(&self) -> bool {
        self.current().kind == TokenKind::Operator
            && (self.current_is_arrow()
                || self.current_is_context_arrow()
                || matches!(self.current_text().ok(), Some("=" | "<-")))
    }

    fn current_name_text(&self) -> Result<&'src str, SourceTextError> {
        let text = self.current_text()?;
        if self.current().kind == TokenKind::BackquotedIdentifier {
            Ok(text
                .strip_prefix('`')
                .and_then(|text| text.strip_suffix('`'))
                .unwrap_or(text))
        } else {
            Ok(text)
        }
    }

    /// Interns the current token spelling in the term namespace.
    pub fn intern_current_term_name(&mut self) -> Result<TermName, SourceTextError> {
        let text = self.current_name_text()?;
        Ok(TermName::new(self.names.intern(text)))
    }

    /// Interns the current token spelling in the type namespace.
    pub fn intern_current_type_name(&mut self) -> Result<TypeName, SourceTextError> {
        let text = self.current_name_text()?;
        Ok(TypeName::new(self.names.intern(text)))
    }

    /// Allocates an untyped AST node with an optional source position.
    pub fn alloc(
        &mut self,
        kind: TreeKind<Untyped>,
        position: Option<SourceSpan>,
    ) -> TreeId<Untyped> {
        self.ast.alloc(Tree {
            kind,
            position,
            ty: (),
        })
    }

    /// Allocates an untyped AST node spanning from `mark` to the last real token.
    pub fn alloc_from(&mut self, mark: Mark, kind: TreeKind<Untyped>) -> TreeId<Untyped> {
        let position = self.span_from(mark);
        self.alloc(kind, Some(position))
    }

    /// Allocates an expression error placeholder for parser recovery.
    pub fn error_expr(&mut self, position: SourceSpan) -> TreeId<Untyped> {
        self.alloc_error(ErrorNodeKind::MissingExpression, position)
    }

    /// Allocates a type error placeholder for parser recovery.
    pub fn error_type(&mut self, position: SourceSpan) -> TreeId<Untyped> {
        self.alloc_error(ErrorNodeKind::MissingType, position)
    }

    /// Allocates a pattern error placeholder for parser recovery.
    pub fn error_pattern(&mut self, position: SourceSpan) -> TreeId<Untyped> {
        self.alloc_error(ErrorNodeKind::MissingPattern, position)
    }

    fn alloc_error(&mut self, kind: ErrorNodeKind, position: SourceSpan) -> TreeId<Untyped> {
        self.alloc(
            TreeKind::PhaseSpecific(UntypedNode::Error(ErrorNode { kind })),
            Some(position),
        )
    }

    /// Returns the parser's untyped AST arena.
    pub fn ast(&self) -> &AstArena<Untyped> {
        &self.ast
    }

    pub(crate) fn zero_width_span(&self, start: u32) -> SourceSpan {
        let range = TextRange::new(start, start).expect("zero-width span is ordered");
        SourceSpan::new(self.source_id, Span::without_point(range))
    }
}

const fn is_zero_width_synthetic(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Indent | TokenKind::Outdent | TokenKind::Newline | TokenKind::Newlines
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::{Punctuation, TextRange, TokenKind, TokenValue};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct SingleTokenSource {
        token: Token,
    }

    struct SequenceTokenSource {
        tokens: Vec<Token>,
        index: usize,
    }

    struct RecordingTokenSource {
        token: Token,
        observed: Rc<RefCell<Vec<ScannerEvent>>>,
    }

    impl TokenSource for SingleTokenSource {
        fn current(&self) -> &Token {
            &self.token
        }

        fn position(&self) -> usize {
            0
        }

        fn advance(&mut self) {}

        fn lookahead(&mut self, _n: usize) -> &Token {
            &self.token
        }

        fn observe(&mut self, _event: dotty_core::ScannerEvent) {}
    }

    impl TokenSource for SequenceTokenSource {
        fn current(&self) -> &Token {
            &self.tokens[self.index]
        }

        fn position(&self) -> usize {
            self.index
        }

        fn advance(&mut self) {
            if self.index + 1 < self.tokens.len() {
                self.index += 1;
            }
        }

        fn lookahead(&mut self, n: usize) -> &Token {
            let index = self
                .index
                .saturating_add(n)
                .min(self.tokens.len().saturating_sub(1));
            &self.tokens[index]
        }

        fn observe(&mut self, _event: dotty_core::ScannerEvent) {}
    }

    impl TokenSource for RecordingTokenSource {
        fn current(&self) -> &Token {
            &self.token
        }

        fn position(&self) -> usize {
            0
        }

        fn advance(&mut self) {}

        fn lookahead(&mut self, _n: usize) -> &Token {
            &self.token
        }

        fn observe(&mut self, event: ScannerEvent) {
            self.observed.borrow_mut().push(event);
        }
    }

    fn token(kind: TokenKind, start: u32, end: u32) -> Token {
        Token {
            kind,
            span: TextRange::new(start, end).expect("valid test range"),
            value: TokenValue::None,
        }
    }

    fn parser_for<'src, 'names>(
        text: &'src str,
        span: TextRange,
        names: &'names mut NameInterner,
    ) -> Parser<'src, 'names, SingleTokenSource> {
        let source = SourceText::new(text).expect("valid source");
        let token = Token {
            kind: TokenKind::Identifier,
            span,
            value: TokenValue::None,
        };
        Parser::new(
            source,
            SourceId::from_index(1),
            SingleTokenSource { token },
            names,
        )
    }

    fn parser_with_tokens<'src, 'names>(
        text: &'src str,
        tokens: Vec<Token>,
        names: &'names mut NameInterner,
    ) -> Parser<'src, 'names, SequenceTokenSource> {
        Parser::new(
            SourceText::new(text).expect("valid source"),
            SourceId::from_index(1),
            SequenceTokenSource { tokens, index: 0 },
            names,
        )
    }

    #[test]
    fn current_text_uses_the_source_backed_token_span() {
        let mut names = NameInterner::new();
        let parser = parser_for("żółw", TextRange::new(0, 2).unwrap(), &mut names);

        assert_eq!(parser.current_text(), Ok("ż"));
    }

    #[test]
    fn structural_operator_helpers_use_source_spelling() {
        let mut names = NameInterner::new();
        let parser = parser_with_tokens(
            "=>",
            vec![
                token(TokenKind::Operator, 0, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );
        assert!(parser.current_text_is("=>"));
        assert!(parser.current_is_arrow());
        assert!(!parser.current_is_context_arrow());
        assert!(!parser.current_is_type_lambda_arrow());
        assert!(parser.current_is_structural_operator());
        drop(parser);

        let parser = parser_with_tokens(
            "?=>",
            vec![
                token(TokenKind::Operator, 0, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );
        assert!(!parser.current_is_arrow());
        assert!(parser.current_is_context_arrow());
        assert!(!parser.current_is_type_lambda_arrow());
        assert!(parser.current_is_structural_operator());
        drop(parser);

        let parser = parser_with_tokens(
            "=>>",
            vec![
                token(TokenKind::Operator, 0, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );
        assert!(!parser.current_is_arrow());
        assert!(!parser.current_is_context_arrow());
        assert!(parser.current_is_type_lambda_arrow());
        assert!(!parser.current_is_structural_operator());
        drop(parser);

        let parser = parser_with_tokens(
            "<-",
            vec![
                token(TokenKind::Operator, 0, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );
        assert!(parser.current_is_structural_operator());
    }

    #[test]
    fn scoped_block_end_is_restored_after_nested_parsing() {
        let mut names = NameInterner::new();
        let mut parser = parser_for("x", TextRange::new(0, 1).unwrap(), &mut names);

        assert_eq!(parser.context().block_end, None);
        let inner = parser.with_block_end(Some(TokenKind::Outdent), |parser| {
            assert_eq!(parser.context().block_end, Some(TokenKind::Outdent));
            parser.with_block_end(
                Some(TokenKind::Punctuation(Punctuation::RightBrace)),
                |parser| {
                    assert_eq!(
                        parser.context().block_end,
                        Some(TokenKind::Punctuation(Punctuation::RightBrace))
                    );
                },
            );
            parser.context().block_end
        });

        assert_eq!(inner, Some(TokenKind::Outdent));
        assert_eq!(parser.context().block_end, None);
    }

    #[test]
    fn token_text_rejects_a_range_outside_the_source() {
        let mut names = NameInterner::new();
        let parser = parser_for("abc", TextRange::new(1, 4).unwrap(), &mut names);

        assert_eq!(
            parser.token_text(parser.current()),
            Err(SourceTextError::RangeOutOfBounds {
                range: TextRange::new(1, 4).unwrap(),
                byte_len: 3,
            })
        );
    }

    #[test]
    fn current_term_name_is_interned_from_source_text() {
        let mut names = NameInterner::new();
        let mut parser = parser_for("value", TextRange::new(0, 5).unwrap(), &mut names);
        let name = parser.intern_current_term_name().expect("valid token span");
        drop(parser);

        assert_eq!(names.resolve(name.as_name().text()), "value");
    }

    #[test]
    fn current_type_name_uses_the_type_namespace() {
        let mut names = NameInterner::new();
        let mut parser = parser_for("Value", TextRange::new(0, 5).unwrap(), &mut names);
        let name = parser.intern_current_type_name().expect("valid token span");
        drop(parser);

        assert!(name.as_name().is_type());
        assert_eq!(names.resolve(name.as_name().text()), "Value");
    }

    #[test]
    fn current_type_name_strips_backquoted_delimiters() {
        let mut names = NameInterner::new();
        let mut parser = parser_with_tokens(
            "`Value`",
            vec![
                token(TokenKind::BackquotedIdentifier, 0, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );
        let name = parser.intern_current_type_name().expect("valid token span");
        drop(parser);

        assert!(name.as_name().is_type());
        assert_eq!(names.resolve(name.as_name().text()), "Value");
    }

    #[test]
    fn parser_recognizes_a_soft_keyword_by_interned_name() {
        let mut names = NameInterner::new();
        let mut parser = parser_for("using", TextRange::new(0, 5).unwrap(), &mut names);
        let expected = parser.known_names().using;

        assert_eq!(parser.current().kind, TokenKind::Identifier);
        assert!(
            parser
                .current_is_known_name(expected)
                .expect("valid token span")
        );
    }

    #[test]
    fn feature_dependent_names_remain_identifiers_until_grammar_uses_policy() {
        let mut names = NameInterner::new();
        let mut parser = parser_for("into", TextRange::new(0, 4).unwrap(), &mut names)
            .with_features(ParserFeatures {
                capture_checking: false,
                erased_definitions: false,
                into: true,
                postfix_ops: false,
                sub_cases: false,
            });
        let expected = parser.known_names().into;

        assert_eq!(parser.current().kind, TokenKind::Identifier);
        assert!(parser.features().into);
        assert!(
            parser
                .current_is_known_name(expected)
                .expect("valid token span")
        );
    }

    #[test]
    fn allocation_preserves_kind_and_source_position() {
        let mut names = NameInterner::new();
        let mut parser = parser_for("value", TextRange::new(0, 5).unwrap(), &mut names);
        let span = SourceSpan::new(
            parser.source_id(),
            dotty_core::Span::without_point(TextRange::new(0, 1).unwrap()),
        );
        let name = *parser
            .intern_current_term_name()
            .expect("valid token span")
            .as_name();
        let id = parser.alloc(
            TreeKind::Ident(dotty_core::ast::Ident {
                name,
                backquoted: false,
            }),
            Some(span),
        );

        assert!(matches!(parser.ast().get(id).kind, TreeKind::Ident(_)));
        assert_eq!(parser.ast().get(id).position, Some(span));
    }

    #[test]
    fn span_from_covers_one_real_token() {
        let mut names = NameInterner::new();
        let mut parser = parser_with_tokens(
            "x",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );
        let mark = parser.mark();

        parser.advance();

        assert_eq!(
            parser.span_from(mark).span().range(),
            TextRange::new(0, 1).unwrap()
        );
    }

    #[test]
    fn span_from_covers_parenthesized_input_through_the_last_real_token() {
        let mut names = NameInterner::new();
        let mut parser = parser_with_tokens(
            "(x)",
            vec![
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    0,
                    1,
                ),
                token(TokenKind::Identifier, 1, 2),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    2,
                    3,
                ),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );
        let mark = parser.mark();

        parser.advance();
        parser.advance();
        parser.advance();

        assert_eq!(
            parser.span_from(mark).span().range(),
            TextRange::new(0, 3).unwrap()
        );
    }

    #[test]
    fn synthetic_outdent_does_not_extend_the_last_real_token_end() {
        let mut names = NameInterner::new();
        let mut parser = parser_with_tokens(
            "x",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Outdent, 1, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );

        parser.advance();
        let mark = parser.mark();

        assert_eq!(mark.start(), 1);
        assert_eq!(
            parser.span_from(mark).span().range(),
            TextRange::new(1, 1).unwrap()
        );
    }

    #[test]
    fn eof_mark_is_a_zero_width_span_after_the_last_real_token() {
        let mut names = NameInterner::new();
        let mut parser = parser_with_tokens(
            "x",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );

        parser.advance();

        assert_eq!(
            parser.span_from(parser.mark()).span().range(),
            TextRange::new(1, 1).unwrap()
        );
    }

    #[test]
    fn alloc_from_assigns_the_span_built_from_its_mark() {
        let mut names = NameInterner::new();
        let mut parser = parser_with_tokens(
            "x",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );
        let mark = parser.mark();
        let name = *parser
            .intern_current_term_name()
            .expect("valid token span")
            .as_name();

        parser.advance();
        let id = parser.alloc_from(
            mark,
            TreeKind::Ident(dotty_core::ast::Ident {
                name,
                backquoted: false,
            }),
        );

        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 1).unwrap()
        );
    }

    #[test]
    fn with_location_restores_the_previous_location() {
        let mut names = NameInterner::new();
        let mut parser = parser_for("x", TextRange::new(0, 1).unwrap(), &mut names);

        let observed_inside =
            parser.with_location(Location::InPattern, |parser| parser.context().location);

        assert_eq!(observed_inside, Location::InPattern);
        assert_eq!(parser.context().location, Location::Elsewhere);
    }

    #[test]
    fn with_parse_kind_restores_the_previous_kind() {
        let mut names = NameInterner::new();
        let mut parser = parser_for("x", TextRange::new(0, 1).unwrap(), &mut names);

        let observed_inside =
            parser.with_parse_kind(ParseKind::Type, |parser| parser.context().parse_kind);

        assert_eq!(observed_inside, ParseKind::Type);
        assert_eq!(parser.context().parse_kind, ParseKind::Expr);
    }

    #[test]
    fn with_param_owner_restores_the_previous_owner() {
        let mut names = NameInterner::new();
        let mut parser = parser_for("x", TextRange::new(0, 1).unwrap(), &mut names);

        let observed_inside = parser.with_param_owner(Some(ParamOwner::Given), |parser| {
            parser.context().param_owner
        });

        assert_eq!(observed_inside, Some(ParamOwner::Given));
        assert_eq!(parser.context().param_owner, None);
    }

    #[test]
    fn parser_scanner_helpers_forward_the_existing_feedback_protocol() {
        let observed = Rc::new(RefCell::new(Vec::new()));
        let mut names = NameInterner::new();
        let token = token(TokenKind::Eof, 0, 0);
        let mut parser = Parser::new(
            SourceText::new("").expect("valid source"),
            SourceId::from_index(1),
            RecordingTokenSource {
                token,
                observed: Rc::clone(&observed),
            },
            &mut names,
        );

        parser.observe_colon_eol(true);
        parser.observe_indented();
        parser.observe_outdented();
        parser.observe_arrow_indented();
        parser.observe_self_arrow();

        assert_eq!(
            *observed.borrow(),
            vec![
                ScannerEvent::ColonEol { in_template: true },
                ScannerEvent::Indented,
                ScannerEvent::Outdented,
                ScannerEvent::ArrowIndented,
                ScannerEvent::SelfArrow,
            ]
        );
    }

    #[test]
    fn expect_reports_an_exact_diagnostic_without_consuming_unexpected_input() {
        let mut names = NameInterner::new();
        let mut parser = parser_for("x", TextRange::new(0, 1).unwrap(), &mut names);

        assert!(!parser.expect(TokenKind::Eof));
        assert_eq!(parser.current().kind, TokenKind::Identifier);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedToken
        );
        assert_eq!(parser.diagnostics()[0].source(), SourceId::from_index(1));
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(0, 1).unwrap()
        );
        assert_eq!(
            parser.diagnostics()[0].severity(),
            dotty_core::DiagnosticSeverity::Error
        );
    }

    #[test]
    fn expect_consumes_the_expected_token_without_diagnostic() {
        let mut names = NameInterner::new();
        let mut parser = parser_for("x", TextRange::new(0, 1).unwrap(), &mut names);

        assert!(parser.expect(TokenKind::Identifier));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovery_stops_at_a_statement_boundary() {
        let mut names = NameInterner::new();
        let mut parser = parser_with_tokens(
            "x;",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::Semicolon),
                    1,
                    2,
                ),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        parser.recover_until(RecoverySet::Statement);

        assert_eq!(
            parser.current().kind,
            TokenKind::Punctuation(dotty_core::Punctuation::Semicolon)
        );
    }

    #[test]
    fn recovery_returns_when_the_token_source_does_not_advance() {
        let mut names = NameInterner::new();
        let mut parser = parser_for("x", TextRange::new(0, 1).unwrap(), &mut names);

        parser.recover_until(RecoverySet::Statement);

        assert_eq!(parser.current().kind, TokenKind::Identifier);
    }

    #[test]
    fn error_helpers_allocate_the_requested_error_node_kind() {
        let mut names = NameInterner::new();
        let mut parser = parser_for("x", TextRange::new(0, 1).unwrap(), &mut names);
        let position = SourceSpan::new(
            SourceId::from_index(1),
            Span::without_point(TextRange::new(0, 0).unwrap()),
        );

        let expression = parser.error_expr(position);
        let type_tree = parser.error_type(position);
        let pattern = parser.error_pattern(position);

        assert!(matches!(
            parser.ast().get(expression).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(ErrorNode {
                kind: ErrorNodeKind::MissingExpression
            }))
        ));
        assert!(matches!(
            parser.ast().get(type_tree).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(ErrorNode {
                kind: ErrorNodeKind::MissingType
            }))
        ));
        assert!(matches!(
            parser.ast().get(pattern).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(ErrorNode {
                kind: ErrorNodeKind::MissingPattern
            }))
        ));
    }
}
