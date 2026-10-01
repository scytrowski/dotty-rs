use dotty_core::{
    HardKeyword, Punctuation, SourceSpan, Span, TextRange, Token, TokenKind, TreeId, TreeKind,
    Untyped,
};

use crate::modifiers::DefinitionPrefix;
use crate::{Location, ParseDiagnosticKind, Parser, RecoverySet};

/// A parser-only classification used while building statement sequences.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ParsedStatement {
    Definition(TreeId<Untyped>),
    Expression(TreeId<Untyped>),
    Many(Vec<TreeId<Untyped>>),
}

/// The boundary that terminates a statement sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StatementSequenceBoundary {
    CompilationUnit,
    Block(TokenKind),
    FeedbackRegionBlock {
        closing: TokenKind,
        indent_offset: u32,
    },
}

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Returns whether a token can begin a statement in a block.
    ///
    /// Layout-sensitive expression bodies use this to decide whether a
    /// physical newline starts a block body rather than a single expression.
    pub(crate) fn can_start_block_stat(&self, token: &Token) -> bool {
        let kind = token.kind;
        crate::modifiers::is_hard_modifier(kind)
            || (kind == TokenKind::Operator && self.token_text(token).ok() == Some("@"))
            || matches!(
                kind,
                TokenKind::CaseClass
                    | TokenKind::CaseObject
                    | TokenKind::Keyword(
                        HardKeyword::Given
                            | HardKeyword::Val
                            | HardKeyword::Var
                            | HardKeyword::Def
                            | HardKeyword::Type
                            | HardKeyword::Enum
                            | HardKeyword::Class
                            | HardKeyword::Trait
                            | HardKeyword::Object
                            | HardKeyword::Package
                            | HardKeyword::Import
                            | HardKeyword::Export
                            | HardKeyword::Match
                    )
            )
            || crate::expr::can_start_expr(kind)
    }

    /// Returns whether `extension` is being used as the contextual
    /// extension-definition introducer.
    ///
    /// `extension` remains an ordinary identifier unless Scala's bounded
    /// lookahead sees the first parameter/type-parameter clause. This mirrors
    /// Dotty's `followingIsExtension` rule without teaching the lexer about
    /// the contextual keyword.
    pub(crate) fn starts_extension_definition(&mut self) -> bool {
        self.current().kind == TokenKind::Identifier
            && self
                .intern_current_term_name()
                .ok()
                .is_some_and(|name| name == self.known_names.extension)
            && matches!(
                self.cursor.lookahead(1).kind,
                TokenKind::Punctuation(Punctuation::LeftBracket | Punctuation::LeftParen)
            )
    }

    /// Parses one statement at the requested source location.
    pub(crate) fn parse_statement(&mut self, location: Location) -> ParsedStatement {
        if self.context.enum_body && self.current().kind == TokenKind::Keyword(HardKeyword::Case) {
            return self.parse_enum_case();
        }
        if self.context.enum_body
            && matches!(
                self.current().kind,
                TokenKind::CaseClass | TokenKind::CaseObject
            )
        {
            return self.parse_unsupported_enum_case();
        }
        if self.starts_extension_definition() {
            return self.parse_extension_definition(location);
        }
        if self.starts_definition_prefix() {
            let prefix = self.parse_definition_prefix();
            if self.context.enum_body && is_enum_case_start(self.current().kind) {
                if self.current().kind == TokenKind::Keyword(HardKeyword::Case) {
                    return self.parse_enum_case_with_prefix(prefix);
                }
                return self.parse_unsupported_enum_case();
            }
            return self.parse_prefixed_definition(location, prefix);
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Given) {
            return self.parse_given_definition(location);
        }
        if matches!(
            self.current().kind,
            TokenKind::Keyword(HardKeyword::Val | HardKeyword::Var)
        ) {
            return self.parse_value_definition(location);
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Def) {
            return self.parse_method_definition(location);
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Type) {
            return self.parse_type_definition(location);
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Enum) {
            return self.parse_enum_definition(location);
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Class) {
            return self.parse_class_definition(location);
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Trait) {
            return self.parse_trait_definition(location);
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Object) {
            return self.parse_object_definition(location);
        }
        if self.current().kind == TokenKind::CaseClass {
            return self.parse_case_class_definition(location);
        }
        if self.current().kind == TokenKind::CaseObject {
            return self.parse_case_object_definition(location);
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Package) {
            return self.parse_package_definition(location);
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Import) {
            return ParsedStatement::Many(self.parse_import_clause(location));
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Export) {
            return ParsedStatement::Many(self.parse_export_clause(location));
        }

        let tree = if is_unsupported_start(self.current().kind) {
            self.parse_unsupported_syntax()
        } else {
            self.with_location(location, |parser| parser.expr())
        };
        ParsedStatement::Expression(tree)
    }

    fn parse_prefixed_definition(
        &mut self,
        location: Location,
        prefix: DefinitionPrefix,
    ) -> ParsedStatement {
        match self.current().kind {
            TokenKind::Keyword(HardKeyword::Val | HardKeyword::Var) => {
                self.parse_value_definition_with_prefix(location, prefix)
            }
            TokenKind::Keyword(HardKeyword::Def) => {
                self.parse_method_definition_with_prefix(location, prefix)
            }
            TokenKind::Keyword(HardKeyword::Type) => {
                self.parse_type_definition_with_prefix(location, prefix)
            }
            TokenKind::Keyword(HardKeyword::Enum) => self.parse_enum_definition_with_prefix(prefix),
            TokenKind::Keyword(HardKeyword::Class) => {
                self.parse_class_definition_with_prefix(prefix)
            }
            TokenKind::Keyword(HardKeyword::Trait) => {
                self.parse_trait_definition_with_prefix(prefix)
            }
            TokenKind::Keyword(HardKeyword::Object) => {
                self.parse_object_definition_with_prefix(prefix)
            }
            TokenKind::CaseClass => self.parse_case_class_definition_with_prefix(prefix),
            TokenKind::CaseObject => self.parse_case_object_definition_with_prefix(prefix),
            TokenKind::Keyword(HardKeyword::Given) => {
                self.parse_given_definition_with_prefix(location, prefix)
            }
            _ => {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected a definition after its annotations and modifiers",
                );
                ParsedStatement::Expression(self.parse_unsupported_syntax())
            }
        }
    }

    /// Parses statements up to a compilation-unit or block boundary.
    pub(crate) fn parse_statement_sequence(
        &mut self,
        boundary: StatementSequenceBoundary,
    ) -> (Vec<TreeId<Untyped>>, TreeId<Untyped>) {
        let outermost = boundary == StatementSequenceBoundary::CompilationUnit;
        self.with_outermost_imports_allowed(outermost, |parser| {
            parser.parse_statement_sequence_inner(boundary)
        })
    }

    fn parse_statement_sequence_inner(
        &mut self,
        boundary: StatementSequenceBoundary,
    ) -> (Vec<TreeId<Untyped>>, TreeId<Untyped>) {
        let mut statements = Vec::new();
        self.consume_sequence_separators(boundary);

        while !self.sequence_ended(boundary) {
            if self.current().kind == TokenKind::EndMarker {
                let last = statements.last().and_then(last_statement_tree);
                let matches_local = self.end_marker_matches_next(last);
                if !matches_local
                    && matches!(
                        boundary,
                        StatementSequenceBoundary::Block(TokenKind::Outdent)
                            | StatementSequenceBoundary::FeedbackRegionBlock {
                                closing: TokenKind::Outdent,
                                ..
                            }
                    )
                {
                    return self.finish_statement_sequence(statements);
                }
                if !self.consume_end_marker(last) {
                    return self.finish_statement_sequence(statements);
                }
                self.consume_sequence_separators(boundary);
                continue;
            }
            let checkpoint = self.cursor.checkpoint();
            let location = match boundary {
                StatementSequenceBoundary::CompilationUnit => Location::Elsewhere,
                StatementSequenceBoundary::Block(_)
                | StatementSequenceBoundary::FeedbackRegionBlock { .. } => Location::InBlock,
            };
            statements.push(self.parse_statement(location));

            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    match boundary {
                        StatementSequenceBoundary::CompilationUnit => {
                            "parser made no progress while parsing a compilation unit"
                        }
                        StatementSequenceBoundary::Block(_) => {
                            "parser made no progress while parsing a block"
                        }
                        StatementSequenceBoundary::FeedbackRegionBlock { .. } => {
                            "parser made no progress while parsing a block"
                        }
                    },
                );
                let recovery_checkpoint = self.cursor.checkpoint();
                self.advance();
                if !self.cursor.progressed_since(recovery_checkpoint) {
                    break;
                }
            }

            if let StatementSequenceBoundary::FeedbackRegionBlock { indent_offset, .. } = boundary {
                self.observe_outdented_region(indent_offset);
            }

            if self.is_sequence_separator(boundary) {
                self.consume_sequence_separators(boundary);
            } else if self.last_advance_was_outdent {
                self.last_advance_was_outdent = false;
            } else if !self.sequence_ended(boundary) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    match boundary {
                        StatementSequenceBoundary::CompilationUnit => {
                            "expected a statement separator"
                        }
                        StatementSequenceBoundary::Block(_) => {
                            "expected a block statement separator"
                        }
                        StatementSequenceBoundary::FeedbackRegionBlock { .. } => {
                            "expected a block statement separator"
                        }
                    },
                );
                self.recover_until(RecoverySet::Statement);
                self.consume_sequence_separators(boundary);
            }

            while self.current().kind == TokenKind::EndMarker {
                let last = statements.last().and_then(last_statement_tree);
                let matches_local = self.end_marker_matches_next(last);
                if !matches_local
                    && matches!(
                        boundary,
                        StatementSequenceBoundary::Block(TokenKind::Outdent)
                            | StatementSequenceBoundary::FeedbackRegionBlock {
                                closing: TokenKind::Outdent,
                                ..
                            }
                    )
                {
                    return self.finish_statement_sequence(statements);
                }
                if !self.consume_end_marker(last) {
                    return self.finish_statement_sequence(statements);
                }
                self.consume_sequence_separators(boundary);
            }
        }

        self.finish_statement_sequence(statements)
    }

    /// Parses a compilation-unit or package statement sequence without the
    /// synthetic trailing expression used by expression blocks.
    pub(crate) fn parse_top_level_sequence(
        &mut self,
        boundary: StatementSequenceBoundary,
        location: Location,
    ) -> Vec<TreeId<Untyped>> {
        let outermost = boundary == StatementSequenceBoundary::CompilationUnit;
        self.with_outermost_imports_allowed(outermost, |parser| {
            parser.parse_top_level_sequence_inner(boundary, location)
        })
    }

    fn parse_top_level_sequence_inner(
        &mut self,
        boundary: StatementSequenceBoundary,
        location: Location,
    ) -> Vec<TreeId<Untyped>> {
        let mut statements = Vec::new();
        self.consume_sequence_separators(boundary);

        while !self.sequence_ended(boundary) {
            if self.current().kind == TokenKind::EndMarker {
                let last = statements.last().copied();
                if !self.consume_end_marker(last) {
                    return statements;
                }
                self.consume_sequence_separators(boundary);
                continue;
            }
            let checkpoint = self.cursor.checkpoint();
            match self.parse_top_level_statement(location) {
                ParsedStatement::Definition(tree) | ParsedStatement::Expression(tree) => {
                    statements.push(tree)
                }
                ParsedStatement::Many(trees) => statements.extend(trees),
            }

            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing a top-level statement",
                );
                let recovery_checkpoint = self.cursor.checkpoint();
                self.advance();
                if !self.cursor.progressed_since(recovery_checkpoint) {
                    break;
                }
            }

            if self.is_sequence_separator(boundary) {
                self.consume_sequence_separators(boundary);
            } else if self.last_advance_was_outdent {
                self.last_advance_was_outdent = false;
            } else if !self.sequence_ended(boundary) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "expected a top-level statement separator",
                );
                self.recover_until(RecoverySet::Statement);
                self.consume_sequence_separators(boundary);
            }

            while self.current().kind == TokenKind::EndMarker {
                let last = statements.last().copied();
                if !self.consume_end_marker(last) {
                    return statements;
                }
                self.consume_sequence_separators(boundary);
            }
        }

        statements
    }

    fn parse_top_level_statement(&mut self, location: Location) -> ParsedStatement {
        if self.starts_extension_definition()
            || is_top_level_statement_start(self.current().kind)
            || self.starts_definition_prefix()
        {
            return self.parse_statement(location);
        }

        let position = self.current_span();
        self.report(
            ParseDiagnosticKind::UnsupportedSyntax,
            "top-level expressions are not supported in a compilation unit",
        );
        let checkpoint = self.cursor.checkpoint();
        self.with_location(location, |parser| {
            let _ = parser.expr();
        });
        if !self.cursor.progressed_since(checkpoint) && self.current().kind != TokenKind::Eof {
            self.advance();
        }
        ParsedStatement::Expression(self.error_expr(position))
    }

    /// Places definitions in `stats` and leaves only the final expression in `expr`.
    pub(crate) fn finish_statement_sequence(
        &mut self,
        statements: Vec<ParsedStatement>,
    ) -> (Vec<TreeId<Untyped>>, TreeId<Untyped>) {
        let mut stats = Vec::new();
        let mut expression = None;

        for statement in statements {
            if let Some(previous) = expression.take() {
                stats.push(previous);
            }

            match statement {
                ParsedStatement::Definition(tree) => stats.push(tree),
                ParsedStatement::Expression(tree) => expression = Some(tree),
                ParsedStatement::Many(trees) => stats.extend(trees),
            }
        }

        (stats, expression.unwrap_or_else(|| self.synthetic_unit()))
    }

    fn sequence_ended(&self, boundary: StatementSequenceBoundary) -> bool {
        match boundary {
            StatementSequenceBoundary::CompilationUnit => self.current().kind == TokenKind::Eof,
            StatementSequenceBoundary::Block(end) => {
                self.current().kind == end
                    || self.current().kind == TokenKind::Eof
                    || (self.context.case_body && self.is_case_body_terminator())
            }
            StatementSequenceBoundary::FeedbackRegionBlock { closing: end, .. } => {
                self.current().kind == end
                    || self.current().kind == TokenKind::Eof
                    // Dotty's statement-sequence end also includes a closing
                    // parenthesis and nested delimiters. A feedback-opened
                    // lambda body may end at its enclosing delimiter without
                    // the scanner materializing a separate Outdent token.
                    || matches!(
                        self.current().kind,
                        TokenKind::Punctuation(Punctuation::RightParen | Punctuation::RightBrace)
                    )
                    || (self.context.location == Location::InArgs
                        && self.current().kind
                            == TokenKind::Punctuation(Punctuation::Comma))
                    || (self.context.case_body && self.is_case_body_terminator())
            }
        }
    }

    fn is_sequence_separator(&self, boundary: StatementSequenceBoundary) -> bool {
        match boundary {
            StatementSequenceBoundary::CompilationUnit => {
                is_statement_separator(self.current().kind)
            }
            StatementSequenceBoundary::Block(_)
            | StatementSequenceBoundary::FeedbackRegionBlock { .. } => {
                is_block_separator(self.current().kind)
            }
        }
    }

    fn consume_sequence_separators(&mut self, boundary: StatementSequenceBoundary) {
        while self.is_sequence_separator(boundary) && !self.sequence_ended(boundary) {
            if self.context.case_body
                && boundary == StatementSequenceBoundary::Block(TokenKind::Outdent)
                && matches!(
                    self.current().kind,
                    TokenKind::Newline | TokenKind::Newlines
                )
            {
                // Inside braces the scanner suppresses eager layout tokens. Let
                // it close a feedback-opened case body at a physical dedent
                // before consuming the separator that precedes the next
                // expression (or case clause).
                self.observe_outdented();
                if self.sequence_ended(boundary) {
                    break;
                }
            }
            self.advance();
        }
    }

    /// Consumes a scanner-classified Scala end marker without letting it enter
    /// expression parsing or recovery. Only the marker and its target are
    /// consumed, so an unrelated next statement remains available to the
    /// enclosing sequence.
    pub(crate) fn consume_end_marker(&mut self, last: Option<TreeId<Untyped>>) -> bool {
        let checkpoint = self.cursor.checkpoint();
        let marker = self.current().span;
        self.advance();
        let target_kind = self.current().kind;
        let target_end = self.current().span.end();
        let target_is_name = matches!(
            target_kind,
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
        );
        if !target_is_name {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected a name after `end`",
            );
            return self.cursor.progressed_since(checkpoint);
        }

        let target_text = self.marker_target_text(target_kind, self.current().span);
        let matching_tree = last.and_then(|tree| {
            self.end_marker_owner(tree, target_kind, &target_text, marker.start(), false)
        });
        if let Some(tree) = matching_tree {
            self.end_marked_trees.insert(tree);
            self.extend_tree_end(tree, target_end);
        } else if last.is_some_and(|tree| {
            self.end_marker_owner(tree, target_kind, &target_text, marker.start(), true)
                .is_some()
        }) {
            self.report_at(
                ParseDiagnosticKind::UnexpectedToken,
                SourceSpan::new(self.source_id, Span::without_point(marker)),
                "duplicate end marker",
            );
        } else {
            let range = TextRange::new(marker.start(), target_end).expect("marker span is ordered");
            self.report_at(
                ParseDiagnosticKind::UnexpectedToken,
                SourceSpan::new(self.source_id, Span::without_point(range)),
                "misaligned end marker",
            );
        }
        self.advance();
        self.cursor.progressed_since(checkpoint)
    }

    pub(crate) fn end_marker_matches_next(&mut self, tree: Option<TreeId<Untyped>>) -> bool {
        let Some(tree) = tree else {
            return false;
        };
        if self.current().kind != TokenKind::EndMarker {
            return false;
        }
        let target = self.cursor.lookahead(1);
        let target_kind = target.kind;
        let target_span = target.span;
        let target_text = self.marker_target_text(target_kind, target_span);
        self.end_marker_owner(
            tree,
            target_kind,
            &target_text,
            self.current().span.start(),
            false,
        )
        .is_some()
    }

    /// Finds the innermost still-unmarked construct represented by `tree` that
    /// matches an `end` target before the marker position. The parser's AST
    /// spans encode the nested source ownership; selecting the latest-starting
    /// eligible owner lets consecutive `end if` markers close nested controls
    /// one at a time instead of repeatedly extending the outer statement.
    fn end_marker_owner(
        &self,
        tree: TreeId<Untyped>,
        target_kind: TokenKind,
        target_text: &str,
        marker_start: u32,
        include_marked: bool,
    ) -> Option<TreeId<Untyped>> {
        let node = self.ast.get(tree);
        let children: Vec<TreeId<Untyped>> = match &node.kind {
            TreeKind::Block(block) => vec![block.expr],
            TreeKind::If(conditional) => {
                // An end marker after a nested branch closes that branch's
                // construct first; the next marker can then close this `if`.
                return [conditional.then_branch, conditional.else_branch]
                    .into_iter()
                    .filter_map(|child| {
                        self.end_marker_owner(
                            child,
                            target_kind,
                            target_text,
                            marker_start,
                            include_marked,
                        )
                    })
                    .max_by_key(|child| {
                        self.ast
                            .get(*child)
                            .position
                            .map(|position| position.span().range().start())
                    })
                    .or_else(|| {
                        self.end_marker_is_eligible(
                            tree,
                            target_kind,
                            target_text,
                            marker_start,
                            include_marked,
                        )
                        .then_some(tree)
                    });
            }
            TreeKind::Match(matching) => matching.cases.last().copied().into_iter().collect(),
            TreeKind::CaseDef(case) => vec![case.body],
            TreeKind::Try(try_expr) => vec![
                try_expr
                    .finalizer
                    .or_else(|| try_expr.cases.last().copied())
                    .unwrap_or(try_expr.expr),
            ],
            TreeKind::While(loop_expr) => vec![loop_expr.body],
            _ => Vec::new(),
        };

        children
            .into_iter()
            .filter_map(|child| {
                self.end_marker_owner(
                    child,
                    target_kind,
                    target_text,
                    marker_start,
                    include_marked,
                )
            })
            .max_by_key(|child| {
                self.ast
                    .get(*child)
                    .position
                    .map(|position| position.span().range().start())
            })
            .or_else(|| {
                self.end_marker_is_eligible(
                    tree,
                    target_kind,
                    target_text,
                    marker_start,
                    include_marked,
                )
                .then_some(tree)
            })
    }

    fn end_marker_is_eligible(
        &self,
        tree: TreeId<Untyped>,
        target_kind: TokenKind,
        target_text: &str,
        marker_start: u32,
        include_marked: bool,
    ) -> bool {
        self.ast
            .get(tree)
            .position
            .is_some_and(|position| position.span().range().end() <= marker_start)
            && (include_marked || !self.end_marked_trees.contains(&tree))
            && self.end_marker_matches(tree, target_kind, target_text)
    }

    fn marker_target_text(&self, kind: TokenKind, span: TextRange) -> String {
        let text = self.source.slice(span).unwrap_or_default();
        if kind == TokenKind::BackquotedIdentifier {
            text.strip_prefix('`')
                .and_then(|text| text.strip_suffix('`'))
                .unwrap_or(text)
                .to_owned()
        } else {
            text.to_owned()
        }
    }

    fn end_marker_matches(
        &self,
        tree: TreeId<Untyped>,
        target_kind: TokenKind,
        target_text: &str,
    ) -> bool {
        use dotty_core::ast::{Modifier, UntypedNode};

        let kind = &self.ast.get(tree).kind;
        match target_kind {
            TokenKind::Keyword(HardKeyword::If) => matches!(kind, TreeKind::If(_)),
            TokenKind::Keyword(HardKeyword::While) => matches!(kind, TreeKind::While(_)),
            TokenKind::Keyword(HardKeyword::Match) => matches!(kind, TreeKind::Match(_)),
            TokenKind::Keyword(HardKeyword::Try) => matches!(kind, TreeKind::Try(_)),
            TokenKind::Keyword(HardKeyword::New) => matches!(kind, TreeKind::New(_)),
            TokenKind::Keyword(HardKeyword::For) => matches!(
                kind,
                TreeKind::PhaseSpecific(UntypedNode::ForDo(_) | UntypedNode::ForYield(_))
            ),
            TokenKind::Keyword(HardKeyword::Val) => matches!(
                kind,
                TreeKind::ValDef(_) | TreeKind::PhaseSpecific(UntypedNode::PatDef(_))
            ),
            TokenKind::Keyword(HardKeyword::This) => matches!(
                kind,
                TreeKind::DefDef(definition)
                    if self.names.resolve(definition.name.as_name().text()) == "<init>"
            ),
            TokenKind::Keyword(HardKeyword::Given) => match kind {
                TreeKind::ValDef(definition) => {
                    definition.metadata.modifiers.contains(&Modifier::Given)
                }
                TreeKind::DefDef(definition) => {
                    definition.metadata.modifiers.contains(&Modifier::Given)
                }
                TreeKind::TypeDef(definition) => {
                    definition.metadata.modifiers.contains(&Modifier::Given)
                }
                TreeKind::PhaseSpecific(UntypedNode::ModuleDef(definition)) => {
                    definition.metadata.modifiers.contains(&Modifier::Given)
                }
                _ => false,
            },
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                let Some(target) = self.names.get(target_text) else {
                    return false;
                };
                let named_id = match kind {
                    TreeKind::ValDef(definition) => Some(definition.name.as_name().text()),
                    TreeKind::DefDef(definition) => Some(definition.name.as_name().text()),
                    TreeKind::TypeDef(definition) => Some(definition.name.as_name().text()),
                    TreeKind::PackageDef(package) => self.last_reference_name(package.name),
                    TreeKind::PhaseSpecific(UntypedNode::ModuleDef(definition)) => {
                        Some(definition.name.as_name().text())
                    }
                    TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(_)) => {
                        return target_text == "extension";
                    }
                    _ => None,
                };
                named_id == Some(target)
            }
            _ => false,
        }
    }

    fn last_reference_name(&self, tree: TreeId<Untyped>) -> Option<dotty_core::NameId> {
        match &self.ast.get(tree).kind {
            TreeKind::Ident(ident) => Some(ident.name.text()),
            TreeKind::Select(select) => Some(select.name.text()),
            _ => None,
        }
    }

    fn is_case_body_terminator(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Keyword(
                HardKeyword::Case | HardKeyword::Else | HardKeyword::Catch | HardKeyword::Finally
            ) | TokenKind::Punctuation(Punctuation::RightBrace | Punctuation::RightParen)
                | TokenKind::Outdent
        )
    }

    fn parse_unsupported_syntax(&mut self) -> TreeId<Untyped> {
        let position = self.current_span();
        self.report(
            ParseDiagnosticKind::UnsupportedSyntax,
            format!(
                "syntax beginning with {:?} is not supported by this parser milestone",
                self.current().kind
            ),
        );
        self.advance();
        self.recover_until(RecoverySet::Statement);
        self.error_expr(position)
    }

    fn parse_unsupported_enum_case(&mut self) -> ParsedStatement {
        let position = self.current_span();
        self.report(
            ParseDiagnosticKind::UnsupportedSyntax,
            "enum cases are not supported by this parser milestone",
        );
        self.advance();
        self.recover_until(RecoverySet::Statement);
        ParsedStatement::Expression(self.error_expr(position))
    }
}

fn last_statement_tree(statement: &ParsedStatement) -> Option<TreeId<Untyped>> {
    match statement {
        ParsedStatement::Definition(tree) | ParsedStatement::Expression(tree) => Some(*tree),
        ParsedStatement::Many(trees) => trees.last().copied(),
    }
}

const fn is_statement_separator(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Newline
            | TokenKind::Newlines
            | TokenKind::Punctuation(Punctuation::Semicolon)
            | TokenKind::Outdent
    )
}

pub(crate) const fn is_block_separator(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Newline
            | TokenKind::Newlines
            | TokenKind::Punctuation(Punctuation::Semicolon)
            | TokenKind::Indent
            | TokenKind::Outdent
    )
}

const fn is_unsupported_start(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Keyword(HardKeyword::Match | HardKeyword::Val | HardKeyword::Var)
    )
}

const fn is_enum_case_start(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Keyword(HardKeyword::Case) | TokenKind::CaseClass | TokenKind::CaseObject
    )
}

const fn is_top_level_statement_start(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Keyword(
            HardKeyword::Val
                | HardKeyword::Var
                | HardKeyword::Def
                | HardKeyword::Type
                | HardKeyword::Class
                | HardKeyword::Trait
                | HardKeyword::Object
                | HardKeyword::Package
                | HardKeyword::Import
                | HardKeyword::Export
                | HardKeyword::Match
                | HardKeyword::Enum
                | HardKeyword::Given
        ) | TokenKind::CaseClass
            | TokenKind::CaseObject
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::{HardKeyword, NameInterner, Punctuation, TextRange, TokenKind, TreeKind};

    #[test]
    fn case_body_sequence_leaves_an_enclosing_else_for_the_if_parser() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "first else fallback",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Keyword(HardKeyword::Else), 6, 10),
                token(TokenKind::Identifier, 11, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let (_, expr) = parser.with_case_body(|parser| {
            parser.parse_statement_sequence(StatementSequenceBoundary::Block(TokenKind::Outdent))
        });

        assert!(matches!(parser.ast.get(expr).kind, TreeKind::Ident(_)));
        assert_eq!(parser.current().kind, TokenKind::Keyword(HardKeyword::Else));
        assert!(parser.diagnostics.is_empty());
    }

    #[test]
    fn case_body_sequence_leaves_an_enclosing_catch_for_the_try_parser() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "first catch fallback",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Keyword(HardKeyword::Catch), 6, 11),
                token(TokenKind::Identifier, 12, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let (_, expr) = parser.with_case_body(|parser| {
            parser.parse_statement_sequence(StatementSequenceBoundary::Block(TokenKind::Outdent))
        });

        assert!(matches!(parser.ast.get(expr).kind, TreeKind::Ident(_)));
        assert_eq!(
            parser.current().kind,
            TokenKind::Keyword(HardKeyword::Catch)
        );
        assert!(parser.diagnostics.is_empty());
    }

    #[test]
    fn case_body_sequence_leaves_an_enclosing_finally_for_the_try_parser() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "first finally cleanup",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Keyword(HardKeyword::Finally), 6, 13),
                token(TokenKind::Identifier, 14, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let (_, expr) = parser.with_case_body(|parser| {
            parser.parse_statement_sequence(StatementSequenceBoundary::Block(TokenKind::Outdent))
        });

        assert!(matches!(parser.ast.get(expr).kind, TreeKind::Ident(_)));
        assert_eq!(
            parser.current().kind,
            TokenKind::Keyword(HardKeyword::Finally)
        );
        assert!(parser.diagnostics.is_empty());
    }

    #[test]
    fn case_body_sequence_still_reports_a_missing_statement_separator() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "first if",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Keyword(HardKeyword::If), 6, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        parser.with_case_body(|parser| {
            parser.parse_statement_sequence(StatementSequenceBoundary::Block(TokenKind::Outdent))
        });

        assert!(parser.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind() == ParseDiagnosticKind::UnexpectedToken
                && diagnostic.message() == "expected a block statement separator"
        }));
    }

    #[test]
    fn matching_if_end_marker_is_consumed_and_extends_the_if_span() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "if c then x\nend if\ny",
            vec![
                token(TokenKind::Keyword(HardKeyword::If), 0, 2),
                token(TokenKind::Identifier, 3, 4),
                token(TokenKind::Keyword(HardKeyword::Then), 5, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Newline, 11, 12),
                token(TokenKind::EndMarker, 12, 15),
                token(TokenKind::Keyword(HardKeyword::If), 16, 18),
                token(TokenKind::Newline, 18, 19),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let (stats, result) =
            parser.parse_statement_sequence(StatementSequenceBoundary::CompilationUnit);

        assert_eq!(stats.len(), 1);
        assert_eq!(
            parser.ast.get(stats[0]).position.unwrap().span().range(),
            TextRange::new(0, 18).unwrap()
        );
        let TreeKind::Ident(result) = parser.ast.get(result).kind else {
            panic!("expected the following statement to remain available");
        };
        assert_eq!(parser.names.resolve(result.name.text()), "y");
        assert!(parser.diagnostics.is_empty());
    }

    #[test]
    fn end_marker_owner_does_not_reach_an_earlier_block_sibling() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{ if a then x; y }",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Keyword(HardKeyword::If), 2, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Keyword(HardKeyword::Then), 7, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Punctuation(Punctuation::Semicolon), 13, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );
        let block = parser.expr();

        assert!(matches!(parser.ast.get(block).kind, TreeKind::Block(_)));
        assert!(
            parser
                .end_marker_owner(block, TokenKind::Keyword(HardKeyword::If), "if", 18, false,)
                .is_none()
        );
    }

    #[test]
    fn misaligned_end_marker_reports_an_error_without_consuming_the_next_statement() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "if c then x\nend while\ny",
            vec![
                token(TokenKind::Keyword(HardKeyword::If), 0, 2),
                token(TokenKind::Identifier, 3, 4),
                token(TokenKind::Keyword(HardKeyword::Then), 5, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Newline, 11, 12),
                token(TokenKind::EndMarker, 12, 15),
                token(TokenKind::Keyword(HardKeyword::While), 16, 21),
                token(TokenKind::Newline, 21, 22),
                token(TokenKind::Identifier, 22, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let (stats, result) =
            parser.parse_statement_sequence(StatementSequenceBoundary::CompilationUnit);

        assert_eq!(stats.len(), 1);
        assert_eq!(parser.diagnostics.len(), 1);
        assert_eq!(
            parser.diagnostics[0].kind(),
            ParseDiagnosticKind::UnexpectedToken
        );
        let TreeKind::Ident(result) = parser.ast.get(result).kind else {
            panic!("expected the statement after the marker");
        };
        assert_eq!(parser.names.resolve(result.name.text()), "y");
    }

    #[test]
    fn throw_end_marker_is_misaligned_and_does_not_extend_the_throw_span() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "throw x\nend throw\ny",
            vec![
                token(TokenKind::Keyword(HardKeyword::Throw), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Newline, 7, 8),
                token(TokenKind::EndMarker, 8, 11),
                token(TokenKind::Keyword(HardKeyword::Throw), 12, 17),
                token(TokenKind::Newline, 17, 18),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let (stats, result) =
            parser.parse_statement_sequence(StatementSequenceBoundary::CompilationUnit);

        assert_eq!(stats.len(), 1);
        assert_eq!(
            parser.ast.get(stats[0]).position.unwrap().span().range(),
            TextRange::new(0, 7).unwrap()
        );
        assert_eq!(parser.diagnostics.len(), 1);
        assert_eq!(parser.diagnostics[0].message(), "misaligned end marker");
        let TreeKind::Ident(result) = parser.ast.get(result).kind else {
            panic!("expected the statement after the misaligned marker");
        };
        assert_eq!(parser.names.resolve(result.name.text()), "y");
    }

    #[test]
    fn duplicate_matching_end_marker_is_diagnosed_but_does_not_swallow_the_next_statement() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "if c then x\nend if\nend if\ny",
            vec![
                token(TokenKind::Keyword(HardKeyword::If), 0, 2),
                token(TokenKind::Identifier, 3, 4),
                token(TokenKind::Keyword(HardKeyword::Then), 5, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Newline, 11, 12),
                token(TokenKind::EndMarker, 12, 15),
                token(TokenKind::Keyword(HardKeyword::If), 16, 18),
                token(TokenKind::Newline, 18, 19),
                token(TokenKind::EndMarker, 19, 22),
                token(TokenKind::Keyword(HardKeyword::If), 23, 25),
                token(TokenKind::Newline, 25, 26),
                token(TokenKind::Identifier, 26, 27),
                token(TokenKind::Eof, 27, 27),
            ],
            &mut names,
        );

        let (stats, result) =
            parser.parse_statement_sequence(StatementSequenceBoundary::CompilationUnit);

        assert_eq!(stats.len(), 1);
        assert_eq!(parser.diagnostics.len(), 1);
        assert_eq!(parser.diagnostics[0].message(), "duplicate end marker");
        let TreeKind::Ident(result) = parser.ast.get(result).kind else {
            panic!("expected the statement after the duplicate marker");
        };
        assert_eq!(parser.names.resolve(result.name.text()), "y");
    }

    #[test]
    fn recognizes_extension_only_when_a_parameter_clause_follows() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "extension (x)",
            vec![
                token(TokenKind::Identifier, 0, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 10, 11),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        );

        assert!(parser.starts_extension_definition());
    }

    #[test]
    fn keeps_extension_as_an_expression_identifier_without_a_clause() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "extension + 1",
            vec![
                token(TokenKind::Identifier, 0, 9),
                token(TokenKind::Operator, 10, 11),
                token(TokenKind::IntegerLiteral, 12, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        );

        assert!(!parser.starts_extension_definition());
    }

    #[test]
    fn accepts_an_extension_as_a_top_level_definition_start() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "extension (x: X) def f = x",
            vec![
                token(TokenKind::Identifier, 0, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 10, 11),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::ColonFollow, 12, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Punctuation(Punctuation::RightParen), 15, 16),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Def), 17, 20),
                token(TokenKind::Identifier, 21, 22),
                token(TokenKind::Operator, 23, 24),
                token(TokenKind::Identifier, 25, 26),
                token(TokenKind::Eof, 26, 26),
            ],
            &mut names,
        );

        assert!(matches!(
            parser.parse_top_level_statement(Location::Elsewhere),
            ParsedStatement::Definition(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn statement_sequence_keeps_the_final_expression_as_the_result() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "a\nb",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Newline, 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let (stats, expr) =
            parser.parse_statement_sequence(StatementSequenceBoundary::CompilationUnit);

        assert_eq!(stats.len(), 1);
        assert!(matches!(parser.ast.get(stats[0]).kind, TreeKind::Ident(_)));
        assert!(matches!(parser.ast.get(expr).kind, TreeKind::Ident(_)));
    }

    #[test]
    fn definition_statements_are_always_block_stats() {
        let mut names = NameInterner::new();
        let x_name = names.intern("x");
        let mut parser = parser_for(
            "x",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );
        let definition = parser.alloc(
            dotty_core::TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Unit,
            }),
            Some(parser.current_span()),
        );
        let expression = parser.alloc(
            dotty_core::TreeKind::Ident(dotty_core::ast::Ident {
                name: *dotty_core::TermName::new(x_name).as_name(),
                backquoted: false,
            }),
            Some(parser.current_span()),
        );

        let (stats, expr) = parser.finish_statement_sequence(vec![
            ParsedStatement::Definition(definition),
            ParsedStatement::Expression(expression),
        ]);

        assert_eq!(stats, vec![definition]);
        assert_eq!(expr, expression);
        assert_eq!(
            parser.ast.get(expr).position.unwrap().span().range(),
            TextRange::new(0, 1).unwrap()
        );
    }

    #[test]
    fn preserves_source_order_when_a_definition_follows_an_expression() {
        let mut names = NameInterner::new();
        let first_name = names.intern("first");
        let last_name = names.intern("last");
        let mut parser = parser_for(
            "first\nval x = 1\nlast",
            vec![token(TokenKind::Eof, 21, 21)],
            &mut names,
        );
        let first = parser.alloc(
            dotty_core::TreeKind::Ident(dotty_core::ast::Ident {
                name: *dotty_core::TermName::new(first_name).as_name(),
                backquoted: false,
            }),
            Some(parser.current_span()),
        );
        let definition = parser.alloc(
            dotty_core::TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Unit,
            }),
            Some(parser.current_span()),
        );
        let last = parser.alloc(
            dotty_core::TreeKind::Ident(dotty_core::ast::Ident {
                name: *dotty_core::TermName::new(last_name).as_name(),
                backquoted: false,
            }),
            Some(parser.current_span()),
        );

        let (stats, expr) = parser.finish_statement_sequence(vec![
            ParsedStatement::Expression(first),
            ParsedStatement::Definition(definition),
            ParsedStatement::Expression(last),
        ]);

        assert_eq!(stats, vec![first, definition]);
        assert_eq!(expr, last);
    }
}
