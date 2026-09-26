use dotty_core::ast::{
    Annotated, ByNameTypeTree, CaseDef, Function, FunctionWithMods, Ident, LambdaTypeTree,
    MatchTypeTree, Modifier, Modifiers, NamedArg, Parens, PolyFunction, RefinedTypeTree, Select,
    Super, Tuple, TypeBoundsTree, UntypedNode, ValDef,
};
use dotty_core::{
    Constant, HardKeyword, Name, Punctuation, SourceSpan, Span, TokenKind, TreeId, TreeKind,
    Untyped,
};

use crate::references::{QualifiedReferenceError, ReferenceNamespace};
use crate::statements::ParsedStatement;
use crate::{Location, ParseDiagnosticKind, ParseKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses a bracketed, comma-separated list of simple type arguments.
    ///
    /// The caller owns the tree that precedes the list; this helper is shared
    /// by term-level type applications and applied type trees so that their
    /// delimiter and recovery behavior stays identical.
    pub(crate) fn parse_type_argument_list(&mut self, wild_ok: bool) -> Vec<TreeId<Untyped>> {
        self.expect(TokenKind::Punctuation(Punctuation::LeftBracket));

        let mut args = Vec::new();
        if self.accept(TokenKind::Punctuation(Punctuation::RightBracket)) {
            self.report(
                ParseDiagnosticKind::ExpectedType,
                "expected a type argument between `[` and `]`",
            );
            return args;
        }

        loop {
            args.push(self.with_type_argument(wild_ok, |parser| parser.type_expr()));
            if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                self.expect(TokenKind::Punctuation(Punctuation::RightBracket));
                break;
            }

            if self
                .cursor
                .at(TokenKind::Punctuation(Punctuation::RightBracket))
            {
                self.report(
                    ParseDiagnosticKind::ExpectedType,
                    "expected a type argument after `,`",
                );
                self.advance();
                break;
            }
        }
        args
    }

    /// Parses the currently supported function and infix type subset.
    ///
    /// `simple_type` deliberately remains an atomic/applied type parser. The
    /// higher-level entries own function-arrow precedence.
    pub(crate) fn type_expr(&mut self) -> TreeId<Untyped> {
        self.parse_function_type()
    }

    fn parse_function_type(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        if self.starts_bracketed_function_type() {
            return self.parse_bracketed_function_type(mark);
        }
        if self.starts_empty_function_type() {
            self.advance();
            self.advance();
            self.advance();
            let body = self.type_expr();
            return self.alloc_from(
                mark,
                TreeKind::PhaseSpecific(UntypedNode::Function(Function {
                    params: Vec::new(),
                    body,
                })),
            );
        }
        if self.starts_empty_context_function_type() {
            self.advance();
            self.advance();
            self.advance();
            self.report(
                ParseDiagnosticKind::ExpectedType,
                "a context function type requires at least one parameter",
            );
            let _ = self.type_expr();
            return self.error_type(self.span_from(mark));
        }

        if let Some(arrow) = self.unnamed_erased_function_type_arrow() {
            let params = self.parse_unnamed_erased_function_params();
            self.consume_function_type_arrow(arrow);
            let body = self.type_expr();
            self.recover_missing_function_results();
            return self.alloc_function_type(
                mark,
                params.params,
                body,
                arrow,
                params.erased_params,
            );
        }

        if let Some(arrow) = self.named_function_type_arrow() {
            let allow_erased = self.features().erased_definitions;
            let params = self.parse_named_function_params(allow_erased);
            self.consume_function_type_arrow(arrow);
            let body = self.type_expr();
            self.recover_missing_function_results();
            return self.alloc_function_type(
                mark,
                params.params,
                body,
                arrow,
                params.erased_params,
            );
        }

        if let Some(arrow) = self.unnamed_by_name_function_type_arrow() {
            let params = self.parse_unnamed_function_params();
            self.consume_function_type_arrow(arrow);
            let body = self.type_expr();
            self.recover_missing_function_results();
            let erased_params = vec![false; params.len()];
            return self.alloc_function_type(mark, params, body, arrow, erased_params);
        }

        let diagnostics_before = self.diagnostics.len();
        let parameter = self.parse_infix_type();
        if self.diagnostics.len() != diagnostics_before {
            return parameter;
        }

        let arrow = if self.current_is_arrow() {
            self.advance();
            FunctionTypeArrow::Ordinary
        } else if self.current_is_context_arrow() {
            self.advance();
            FunctionTypeArrow::Context
        } else if self.current().kind == TokenKind::Keyword(HardKeyword::Match) {
            return self.parse_match_type(mark, parameter);
        } else {
            return parameter;
        };
        let body = self.type_expr();
        self.recover_missing_function_results();
        let params = self.function_type_params(parameter);
        let erased_params = vec![false; params.len()];
        self.alloc_function_type(mark, params, body, arrow, erased_params)
    }

    /// Parses the first source-level match-type form:
    /// `selector match { case InfixType => Type }`.
    ///
    /// Match-type cases deliberately do not use the expression pattern
    /// parser. Dotty parses their left-hand side as an `InfixType`, while the
    /// result remains a complete type, so the two sides have different
    /// grammar entries even though they share the `CaseDef` AST node.
    fn parse_match_type(
        &mut self,
        mark: crate::Mark,
        selector: TreeId<Untyped>,
    ) -> TreeId<Untyped> {
        self.advance();
        let indented = if self.accept(TokenKind::Punctuation(Punctuation::LeftBrace)) {
            false
        } else {
            self.consume_match_type_separators();
            if self.accept(TokenKind::Indent) {
                true
            } else {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected `{` or an indented case region after `match`",
                );
                false
            }
        };

        let cases = self.parse_match_type_cases();
        if indented {
            self.consume_match_type_separators();
            if !self.accept(TokenKind::Outdent) {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected an outdent to close match-type cases",
                );
            }
        } else if !self.accept(TokenKind::Punctuation(Punctuation::RightBrace)) {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `}` to close match-type cases",
            );
        }

        if cases.is_empty() {
            self.report(
                ParseDiagnosticKind::ExpectedType,
                "expected at least one `case` clause after match type",
            );
        }

        self.alloc_from(
            mark,
            TreeKind::MatchTypeTree(MatchTypeTree {
                bound: None,
                selector,
                cases,
            }),
        )
    }

    fn parse_match_type_cases(&mut self) -> Vec<TreeId<Untyped>> {
        let mut cases = Vec::new();
        self.consume_match_type_separators();
        while self.current().kind == TokenKind::Keyword(HardKeyword::Case) {
            let checkpoint = self.cursor.checkpoint();
            cases.push(self.parse_match_type_case());
            self.consume_match_type_separators();
            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing match-type cases",
                );
                break;
            }
        }
        cases
    }

    fn parse_match_type_case(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        self.expect(TokenKind::Keyword(HardKeyword::Case));

        let pattern = self.with_parse_kind(ParseKind::Type, |parser| {
            parser.with_location(Location::InPattern, |parser| {
                parser.parse_match_type_case_pattern()
            })
        });

        if !self.current_is_arrow() {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `=>` after match-type case pattern",
            );
            self.recover_match_type_case();
            let body = self.error_type(self.current_span());
            return self.alloc_from(
                mark,
                TreeKind::CaseDef(CaseDef {
                    pattern,
                    guard: None,
                    body,
                }),
            );
        }

        self.advance();
        let body = self.with_block_end(
            Some(TokenKind::Punctuation(Punctuation::RightBrace)),
            |parser| parser.type_expr(),
        );
        self.consume_match_type_case_end();
        self.alloc_from(
            mark,
            TreeKind::CaseDef(CaseDef {
                pattern,
                guard: None,
                body,
            }),
        )
    }

    fn parse_match_type_case_pattern(&mut self) -> TreeId<Untyped> {
        if self.current().kind == TokenKind::Identifier
            && self.current_text_is("_")
            && self.lookahead_is_arrow(1)
        {
            let mark = self.mark();
            let Ok(type_name) = self.intern_current_type_name() else {
                let position = self.current_span();
                self.report(
                    ParseDiagnosticKind::ExpectedType,
                    "expected a valid match-type wildcard",
                );
                return self.error_type(position);
            };
            self.advance();
            return self.alloc_from(
                mark,
                TreeKind::Ident(Ident {
                    name: *type_name.as_name(),
                    backquoted: false,
                }),
            );
        }

        self.parse_infix_type()
    }

    fn recover_match_type_case(&mut self) {
        while !matches!(
            self.current().kind,
            TokenKind::Keyword(HardKeyword::Case)
                | TokenKind::Punctuation(Punctuation::RightBrace)
                | TokenKind::Outdent
                | TokenKind::Eof
        ) {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn consume_match_type_separators(&mut self) {
        while matches!(
            self.current().kind,
            TokenKind::Newline
                | TokenKind::Newlines
                | TokenKind::Indent
                | TokenKind::Punctuation(Punctuation::Semicolon)
        ) {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn consume_match_type_case_end(&mut self) {
        while matches!(
            self.current().kind,
            TokenKind::Newline
                | TokenKind::Newlines
                | TokenKind::Punctuation(Punctuation::Semicolon)
        ) {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn starts_bracketed_function_type(&self) -> bool {
        self.current().kind == TokenKind::Punctuation(Punctuation::LeftBracket)
    }

    fn parse_bracketed_function_type(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let type_params = self.parse_type_param_clause(crate::ParamOwner::Type);
        if self.current_is_type_lambda_arrow() {
            self.advance();
            self.strip_type_lambda_context_bounds(&type_params);
            let body = self.type_expr();
            if type_params.is_empty() {
                self.report(
                    ParseDiagnosticKind::ExpectedType,
                    "a type lambda requires at least one type parameter",
                );
                return self.error_type(self.span_from(mark));
            }
            return self.alloc_from(
                mark,
                TreeKind::LambdaTypeTree(LambdaTypeTree { type_params, body }),
            );
        }
        if !self.current_is_arrow() {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `=>` after polymorphic function type parameters",
            );
            return self.error_type(self.span_from(mark));
        }

        self.advance();
        let body = self.type_expr();
        if type_params.is_empty() || !self.is_function_type(body) {
            self.report(
                ParseDiagnosticKind::UnexpectedToken,
                "polymorphic function types require a function type body and at least one type parameter",
            );
            return self.error_type(self.span_from(mark));
        }

        self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::PolyFunction(PolyFunction {
                type_params,
                body,
            })),
        )
    }

    fn is_function_type(&self, tree: TreeId<Untyped>) -> bool {
        match &self.ast.get(tree).kind {
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
            | TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_)) => true,
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => {
                self.is_function_type(parens.inner)
            }
            _ => false,
        }
    }

    fn starts_empty_function_type(&mut self) -> bool {
        self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen)
            && self.cursor.lookahead(1).kind == TokenKind::Punctuation(Punctuation::RightParen)
            && self.lookahead_is_arrow(2)
    }

    fn starts_empty_context_function_type(&mut self) -> bool {
        self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen)
            && self.cursor.lookahead(1).kind == TokenKind::Punctuation(Punctuation::RightParen)
            && self.lookahead_is_context_arrow(2)
    }

    /// Recognizes the disambiguating prefix of `(name: Type) => Result` or
    /// `(name: Type) ?=> Result`.
    ///
    /// Parenthesized type parsing must not treat the colon after `name` as a
    /// tuple element. We only commit when the matching right parenthesis is
    /// followed by one of the two supported function arrows. If there is no
    /// arrow, a leading `name: Type` element is parsed as a named tuple type.
    fn named_function_type_arrow(&mut self) -> Option<FunctionTypeArrow> {
        if self.current().kind != TokenKind::Punctuation(Punctuation::LeftParen) {
            return None;
        }

        let first_name = self.cursor.lookahead(1).clone();
        let erased_prefix = self.features().erased_definitions
            && first_name.kind == TokenKind::Identifier
            && self.token_text(&first_name).ok() == Some("erased");
        let name_offset = if erased_prefix { 2 } else { 1 };
        let colon_offset = if erased_prefix { 3 } else { 2 };
        let first_name = self.cursor.lookahead(name_offset).clone();
        let first_colon = self.cursor.lookahead(colon_offset).clone();
        if !erased_prefix
            && (!matches!(
                first_name.kind,
                TokenKind::Identifier | TokenKind::BackquotedIdentifier
            ) || !is_function_param_colon(first_colon.kind))
        {
            return None;
        }

        let mut depth = 1usize;
        let mut offset = 1usize;
        loop {
            let token = self.cursor.lookahead(offset).clone();
            match token.kind {
                TokenKind::Punctuation(Punctuation::LeftParen) => depth += 1,
                TokenKind::Punctuation(Punctuation::RightParen) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return if self.lookahead_is_arrow(offset + 1) {
                            Some(FunctionTypeArrow::Ordinary)
                        } else if self.lookahead_is_context_arrow(offset + 1) {
                            Some(FunctionTypeArrow::Context)
                        } else {
                            None
                        };
                    }
                }
                TokenKind::Eof => return None,
                _ => {}
            }
            offset = offset.saturating_add(1);
        }
    }

    /// Recognizes a parenthesized function parameter list containing a
    /// leading-arrow by-name parameter and followed by a function arrow.
    ///
    /// A leading arrow is only a `FunArgType` marker in this position. In
    /// particular, `(A => B) => C` is an ordinary function type whose single
    /// parameter is itself a function type, not a by-name parameter list.
    fn unnamed_by_name_function_type_arrow(&mut self) -> Option<FunctionTypeArrow> {
        if self.current().kind != TokenKind::Punctuation(Punctuation::LeftParen) {
            return None;
        }

        let mut paren_depth = 1usize;
        let mut bracket_depth = 0usize;
        let mut parameter_start = true;
        let mut has_by_name = false;
        let mut offset = 1usize;

        loop {
            let token = self.cursor.lookahead(offset).clone();
            match token.kind {
                TokenKind::Punctuation(Punctuation::LeftParen) => {
                    paren_depth += 1;
                    parameter_start = false;
                }
                TokenKind::Punctuation(Punctuation::RightParen) => {
                    paren_depth = paren_depth.saturating_sub(1);
                    if paren_depth == 0 {
                        if !has_by_name {
                            return None;
                        }
                        if self.lookahead_is_arrow(offset + 1) {
                            return Some(FunctionTypeArrow::Ordinary);
                        }
                        if self.lookahead_is_context_arrow(offset + 1) {
                            return Some(FunctionTypeArrow::Context);
                        }
                        return None;
                    }
                    parameter_start = false;
                }
                TokenKind::Punctuation(Punctuation::LeftBracket) => {
                    bracket_depth += 1;
                    parameter_start = false;
                }
                TokenKind::Punctuation(Punctuation::RightBracket) => {
                    bracket_depth = bracket_depth.saturating_sub(1);
                    parameter_start = false;
                }
                TokenKind::Punctuation(Punctuation::Comma)
                    if paren_depth == 1 && bracket_depth == 0 =>
                {
                    parameter_start = true;
                }
                TokenKind::Operator
                    if paren_depth == 1
                        && bracket_depth == 0
                        && parameter_start
                        && self.lookahead_is_arrow(offset) =>
                {
                    has_by_name = true;
                    parameter_start = false;
                }
                TokenKind::Eof => return None,
                _ => parameter_start = false,
            }
            offset = offset.saturating_add(1);
        }
    }

    /// Recognizes the Scala 3.9-only leading unnamed erased parameter form.
    ///
    /// This deliberately does not recognize erased markers after the first
    /// parameter or more than one leading marker. Those forms belong to a
    /// later grammar increment and must not be accepted accidentally here.
    fn unnamed_erased_function_type_arrow(&mut self) -> Option<FunctionTypeArrow> {
        if !self.features().erased_definitions
            || self.current().kind != TokenKind::Punctuation(Punctuation::LeftParen)
        {
            return None;
        }

        let marker = self.cursor.lookahead(1).clone();
        if marker.kind != TokenKind::Identifier || self.token_text(&marker).ok() != Some("erased") {
            return None;
        }

        let first_type = self.cursor.lookahead(2).clone();
        if first_type.kind == TokenKind::Punctuation(Punctuation::RightParen)
            || first_type.kind == TokenKind::Eof
        {
            return None;
        }

        if is_function_param_colon(first_type.kind) {
            return None;
        }

        // `(erased x: A)` is the already-supported named form. Keep it on
        // that path rather than treating `x` as an unnamed type followed by a
        // stray colon.
        if matches!(
            first_type.kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) && is_function_param_colon(self.cursor.lookahead(3).kind)
        {
            return None;
        }

        let mut depth = 1usize;
        let mut offset = 1usize;
        loop {
            let token = self.cursor.lookahead(offset).clone();
            match token.kind {
                TokenKind::Punctuation(Punctuation::LeftParen) => depth += 1,
                TokenKind::Punctuation(Punctuation::RightParen) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        if self.lookahead_is_arrow(offset + 1) {
                            return Some(FunctionTypeArrow::Ordinary);
                        }
                        if self.lookahead_is_context_arrow(offset + 1) {
                            return Some(FunctionTypeArrow::Context);
                        }
                        return None;
                    }
                }
                TokenKind::Eof => return None,
                _ => {}
            }
            offset = offset.saturating_add(1);
        }
    }

    fn parse_unnamed_function_params(&mut self) -> Vec<TreeId<Untyped>> {
        self.expect(TokenKind::Punctuation(Punctuation::LeftParen));
        let mut params = Vec::new();

        if self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
            return params;
        }

        loop {
            let mark = self.mark();
            let parameter = if self.current_is_arrow() {
                self.advance();
                let result = self.type_expr();
                self.alloc_from(mark, TreeKind::ByNameTypeTree(ByNameTypeTree { result }))
            } else {
                self.type_expr()
            };
            params.push(parameter);

            if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                if self
                    .cursor
                    .at(TokenKind::Punctuation(Punctuation::RightParen))
                {
                    self.report(
                        ParseDiagnosticKind::ExpectedType,
                        "expected a function type parameter after `,`",
                    );
                    self.advance();
                    break;
                }
                continue;
            }
            if self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
                break;
            }

            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `,` or `)` after function type parameter",
            );
            self.recover_unnamed_function_params();
            if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                continue;
            }
            self.accept(TokenKind::Punctuation(Punctuation::RightParen));
            break;
        }

        params
    }

    fn parse_unnamed_erased_function_params(&mut self) -> NamedFunctionParams {
        self.expect(TokenKind::Punctuation(Punctuation::LeftParen));
        let mut params = Vec::new();
        let mut erased_params = Vec::new();

        if !self.current_is_erased_name() {
            self.report(
                ParseDiagnosticKind::ExpectedType,
                "expected a leading `erased` function type parameter",
            );
            return NamedFunctionParams {
                params,
                erased_params,
            };
        }
        self.advance();

        params.push(self.type_expr());
        erased_params.push(true);

        loop {
            if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                if self
                    .cursor
                    .at(TokenKind::Punctuation(Punctuation::RightParen))
                {
                    self.report(
                        ParseDiagnosticKind::ExpectedType,
                        "expected a function type parameter after `,`",
                    );
                    self.advance();
                    break;
                }
                if self.current_is_erased_name() {
                    self.report(
                        ParseDiagnosticKind::ExpectedType,
                        "only the leading unnamed function type parameter may be `erased`",
                    );
                    self.recover_unnamed_function_params();
                    self.accept(TokenKind::Punctuation(Punctuation::RightParen));
                    break;
                }
                params.push(self.type_expr());
                erased_params.push(false);
                continue;
            }

            if self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
                break;
            }

            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `,` or `)` after function type parameter",
            );
            self.recover_unnamed_function_params();
            self.accept(TokenKind::Punctuation(Punctuation::RightParen));
            break;
        }

        NamedFunctionParams {
            params,
            erased_params,
        }
    }

    fn recover_unnamed_function_params(&mut self) {
        while !matches!(
            self.current().kind,
            TokenKind::Punctuation(Punctuation::Comma | Punctuation::RightParen) | TokenKind::Eof
        ) {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn parse_named_function_params(&mut self, allow_erased: bool) -> NamedFunctionParams {
        self.expect(TokenKind::Punctuation(Punctuation::LeftParen));
        let mut params = Vec::new();
        let mut erased_params = Vec::new();

        loop {
            if self.current().kind == TokenKind::Punctuation(Punctuation::RightParen) {
                self.advance();
                break;
            }
            if self.current().kind == TokenKind::Eof || self.current_is_arrow() {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected `)` after named function type parameters",
                );
                break;
            }

            let is_erased = allow_erased && self.current_is_erased_name();
            if is_erased {
                self.advance();
            }
            let parameter_mark = self.mark();
            let name = match self.intern_current_term_name() {
                Ok(name)
                    if matches!(
                        self.current().kind,
                        TokenKind::Identifier | TokenKind::BackquotedIdentifier
                    ) =>
                {
                    self.advance();
                    name
                }
                _ => {
                    self.report(
                        ParseDiagnosticKind::ExpectedToken,
                        "expected a named function type parameter",
                    );
                    self.recover_named_function_params();
                    if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                        continue;
                    }
                    self.accept(TokenKind::Punctuation(Punctuation::RightParen));
                    break;
                }
            };

            if !self.accept_function_param_colon() {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected `:` after named function type parameter",
                );
                self.recover_named_function_params();
                if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                    continue;
                }
                self.accept(TokenKind::Punctuation(Punctuation::RightParen));
                break;
            }

            let type_tree = self.type_expr();
            params.push(self.alloc_from(
                parameter_mark,
                TreeKind::ValDef(ValDef {
                    name,
                    tpt: type_tree,
                    rhs: None,
                    metadata: Modifiers::default(),
                }),
            ));
            erased_params.push(is_erased);

            if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                if self
                    .cursor
                    .at(TokenKind::Punctuation(Punctuation::RightParen))
                {
                    self.report(
                        ParseDiagnosticKind::ExpectedToken,
                        "expected a named function type parameter after `,`",
                    );
                    self.advance();
                    break;
                }
                continue;
            }
            if self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
                break;
            }

            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `,` or `)` after named function type parameter",
            );
            self.recover_named_function_params();
            if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                continue;
            }
            self.accept(TokenKind::Punctuation(Punctuation::RightParen));
            break;
        }

        NamedFunctionParams {
            params,
            erased_params,
        }
    }

    fn current_is_erased_name(&mut self) -> bool {
        if self.current().kind != TokenKind::Identifier {
            return false;
        }
        if is_function_param_colon(self.cursor.lookahead(1).kind) {
            return false;
        }
        let erased = self.known_names().erased;
        self.current_is_known_name(erased).unwrap_or(false)
    }

    fn accept_function_param_colon(&mut self) -> bool {
        if is_function_param_colon(self.current().kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn recover_named_function_params(&mut self) {
        while !matches!(
            self.current().kind,
            TokenKind::Punctuation(Punctuation::Comma | Punctuation::RightParen) | TokenKind::Eof
        ) && !self.current_is_arrow()
            && !self.current_is_context_arrow()
        {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn consume_function_type_arrow(&mut self, arrow: FunctionTypeArrow) {
        let matches_arrow = match arrow {
            FunctionTypeArrow::Ordinary => self.current_is_arrow(),
            FunctionTypeArrow::Context => self.current_is_context_arrow(),
        };
        if !matches_arrow {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                match arrow {
                    FunctionTypeArrow::Ordinary => {
                        "expected `=>` after named function type parameters"
                    }
                    FunctionTypeArrow::Context => {
                        "expected `?=>` after named context function type parameters"
                    }
                },
            );
        } else {
            self.advance();
        }
    }

    fn recover_missing_function_results(&mut self) {
        while self.current_is_arrow() || self.current_is_context_arrow() {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                return;
            }
            let _ = self.type_expr();
        }
    }

    fn alloc_function_type(
        &mut self,
        mark: crate::Mark,
        params: Vec<TreeId<Untyped>>,
        body: TreeId<Untyped>,
        arrow: FunctionTypeArrow,
        erased_params: Vec<bool>,
    ) -> TreeId<Untyped> {
        let kind = match arrow {
            FunctionTypeArrow::Ordinary if erased_params.iter().any(|erased| *erased) => {
                TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(FunctionWithMods {
                    params,
                    result: body,
                    modifiers: Modifiers::default(),
                    erased_params,
                }))
            }
            FunctionTypeArrow::Ordinary => {
                TreeKind::PhaseSpecific(UntypedNode::Function(Function { params, body }))
            }
            FunctionTypeArrow::Context => {
                TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(FunctionWithMods {
                    erased_params,
                    modifiers: Modifiers {
                        modifiers: vec![Modifier::Given],
                        ..Modifiers::default()
                    },
                    params,
                    result: body,
                }))
            }
        };
        self.alloc_from(mark, kind)
    }

    fn function_type_params(&self, parameter: TreeId<Untyped>) -> Vec<TreeId<Untyped>> {
        match &self.ast.get(parameter).kind {
            TreeKind::PhaseSpecific(UntypedNode::Parens(Parens { inner })) => vec![*inner],
            TreeKind::PhaseSpecific(UntypedNode::Tuple(Tuple { elements })) => elements.clone(),
            _ => vec![parameter],
        }
    }

    /// Parses `InfixType` with the same precedence and associativity contract
    /// as expression operators. The operand remains a refined type, so
    /// function arrows can continue to own the outer precedence level.
    pub(crate) fn parse_infix_type(&mut self) -> TreeId<Untyped> {
        self.parse_infix_type_inner(false)
    }

    /// Parses a context-bound type, where the contextual `as` alias belongs
    /// to the surrounding type-parameter grammar rather than to InfixType.
    pub(crate) fn parse_context_bound_type_expr(&mut self) -> TreeId<Untyped> {
        self.parse_infix_type_inner(true)
    }

    fn parse_infix_type_inner(&mut self, stop_at_context_bound_alias: bool) -> TreeId<Untyped> {
        let mut top = self.parse_refined_type();
        let mut operators = Vec::new();

        while let Some((operator, offset)) =
            self.current_type_infix_operator(stop_at_context_bound_alias)
        {
            let checkpoint = self.cursor.checkpoint();
            let spelling = self.names.resolve(operator.text());
            let operator_precedence = crate::infix::precedence(spelling);
            let operator_left_associative = !crate::infix::is_right_associative(spelling);

            top = self.reduce_type_operator_stack(
                &mut operators,
                top,
                operator_precedence,
                operator_left_associative,
                Some(operator),
            );
            self.advance();
            operators.push(crate::OpInfo {
                operand: top,
                operator,
                offset,
            });
            self.consume_type_infix_newlines();

            let operand_checkpoint = self.cursor.checkpoint();
            top = self.parse_refined_type();
            if !self.cursor.progressed_since(operand_checkpoint) {
                break;
            }
            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing an infix type",
                );
                break;
            }
        }

        self.reduce_type_operator_stack(&mut operators, top, 0, true, None)
    }

    fn current_type_infix_operator(
        &mut self,
        stop_at_context_bound_alias: bool,
    ) -> Option<(Name, u32)> {
        if !matches!(
            self.current().kind,
            TokenKind::Identifier
                | TokenKind::BackquotedIdentifier
                | TokenKind::Operator
                | TokenKind::ColonOp
        ) {
            return None;
        }

        // Arrows, bounds, projections, a bare colon, and contextual markers
        // belong to their own type productions rather than to InfixType.
        if self.current_is_structural_operator()
            || self.current_text_is(":")
            || self.current_text_is("<:")
            || self.current_text_is(">:")
            || self.current_text_is("#")
            || stop_at_context_bound_alias && self.current_text_is("as")
            || !self.type_operator_has_following_operand()
        {
            return None;
        }

        let operator = *self.intern_current_type_name().ok()?.as_name();
        Some((operator, self.current().span.start()))
    }

    fn type_operator_has_following_operand(&mut self) -> bool {
        let mut offset = 1;
        while matches!(
            self.cursor.lookahead(offset).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            offset += 1;
        }
        let next = self.cursor.lookahead(offset).clone();
        self.can_start_type_operand(&next)
            || matches!(
                self.current().kind,
                TokenKind::Operator | TokenKind::ColonOp
            ) && matches!(
                next.kind,
                TokenKind::Eof
                    | TokenKind::Punctuation(
                        Punctuation::Comma
                            | Punctuation::RightBracket
                            | Punctuation::RightParen
                            | Punctuation::RightBrace
                    )
            )
    }

    fn reduce_type_operator_stack(
        &mut self,
        operators: &mut Vec<crate::OpInfo>,
        mut top: TreeId<Untyped>,
        precedence: u8,
        left_associative: bool,
        next_operator: Option<Name>,
    ) -> TreeId<Untyped> {
        if let (Some(stack_top), Some(next_operator)) = (operators.last(), next_operator) {
            let stack_spelling = self.names.resolve(stack_top.operator.text()).to_owned();
            let next_spelling = self.names.resolve(next_operator.text()).to_owned();
            if crate::infix::has_mixed_associativity(&stack_spelling, &next_spelling) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    format!(
                        "mixed left- and right-associative type operators `{stack_spelling}` and `{next_spelling}`"
                    ),
                );
            }
        }

        while let Some(stack_top) = operators.last() {
            let stack_spelling = self.names.resolve(stack_top.operator.text());
            let stack_precedence = crate::infix::precedence(stack_spelling);
            if !(precedence < stack_precedence
                || left_associative && precedence == stack_precedence)
            {
                break;
            }

            let stack_top = operators.pop().expect("type operator stack was non-empty");
            top = self.alloc_infix(stack_top.operand, stack_top.operator, top);
        }
        top
    }

    /// Parses `annotatedType` followed by zero or more refinement bodies.
    ///
    /// A parentless refinement is represented with the existing zero-width
    /// `TypeTree` placeholder because `RefinedTypeTree` deliberately keeps a
    /// non-optional parent across AST phases. It remains a syntax-only
    /// placeholder; no refinement scope or semantic owner is created here.
    pub(crate) fn parse_refined_type(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let mut tree = if self.current().kind == TokenKind::Punctuation(Punctuation::LeftBrace) {
            self.synthetic_type_tree_at(mark.start())
        } else {
            self.parse_annotated_type()
        };

        while self.current().kind == TokenKind::Punctuation(Punctuation::LeftBrace) {
            let refinements = self.parse_refinement_body();
            tree = self.alloc_from(
                mark,
                TreeKind::RefinedTypeTree(RefinedTypeTree {
                    tpt: tree,
                    refinements,
                }),
            );
        }
        tree
    }

    fn parse_refinement_body(&mut self) -> Vec<TreeId<Untyped>> {
        self.advance();
        self.consume_refinement_separators();

        let mut refinements = Vec::new();
        while !matches!(
            self.current().kind,
            TokenKind::Punctuation(Punctuation::RightBrace) | TokenKind::Outdent | TokenKind::Eof
        ) {
            let checkpoint = self.cursor.checkpoint();
            let member = self.with_block_end(
                Some(TokenKind::Punctuation(Punctuation::RightBrace)),
                |parser| parser.parse_refinement_member(),
            );
            if let Some(definition) = member {
                refinements.push(definition);
            } else {
                self.recover_refinement_member();
            }

            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing a type refinement",
                );
                let recovery_checkpoint = self.cursor.checkpoint();
                self.advance();
                if !self.cursor.progressed_since(recovery_checkpoint) {
                    break;
                }
            }
            self.consume_refinement_separators();
        }

        self.expect(TokenKind::Punctuation(Punctuation::RightBrace));
        refinements
    }

    fn parse_refinement_member(&mut self) -> Option<TreeId<Untyped>> {
        let member_start = self.mark();
        let current_kind = self.current().kind;
        let statement = match current_kind {
            TokenKind::Keyword(HardKeyword::Type) => self.parse_type_definition(Location::InBlock),
            TokenKind::Keyword(HardKeyword::Val | HardKeyword::Var) => {
                self.parse_value_definition(Location::InBlock)
            }
            TokenKind::Keyword(HardKeyword::Def) => self.parse_method_definition(Location::InBlock),
            TokenKind::Keyword(
                HardKeyword::Class | HardKeyword::Trait | HardKeyword::Object | HardKeyword::Enum,
            )
            | TokenKind::CaseClass
            | TokenKind::CaseObject => {
                self.report(
                    ParseDiagnosticKind::UnsupportedSyntax,
                    "class-like definitions are not allowed in a type refinement",
                );
                return None;
            }
            _ if self.starts_definition_prefix() => {
                self.report(
                    ParseDiagnosticKind::UnsupportedSyntax,
                    "modifiers and annotations are not allowed in a type refinement",
                );
                return None;
            }
            _ => {
                self.report(
                    ParseDiagnosticKind::UnsupportedSyntax,
                    "expected a type, val, var, or def declaration in a type refinement",
                );
                return None;
            }
        };

        let ParsedStatement::Definition(definition) = statement else {
            return None;
        };
        self.validate_refinement_member(definition, member_start);
        Some(definition)
    }

    fn validate_refinement_member(&mut self, member: TreeId<Untyped>, mark: crate::Mark) {
        let has_rhs = match &self.ast.get(member).kind {
            TreeKind::ValDef(definition) => definition.rhs.is_some(),
            TreeKind::DefDef(definition) => {
                definition.rhs.is_some()
                    || definition
                        .value_param_clauses
                        .iter()
                        .flatten()
                        .any(|parameter| {
                            matches!(
                                self.ast.get(*parameter).kind,
                                TreeKind::ValDef(ref parameter) if parameter.rhs.is_some()
                            )
                        })
            }
            _ => false,
        };
        if has_rhs {
            self.report_at(
                ParseDiagnosticKind::UnsupportedSyntax,
                self.span_from(mark),
                "refinement val, var, and def declarations cannot have a right-hand side or default argument",
            );
        }
    }

    fn consume_refinement_separators(&mut self) {
        while matches!(
            self.current().kind,
            TokenKind::Newline
                | TokenKind::Newlines
                | TokenKind::Indent
                | TokenKind::Punctuation(Punctuation::Semicolon)
        ) {
            self.advance();
        }
    }

    fn recover_refinement_member(&mut self) {
        while !matches!(
            self.current().kind,
            TokenKind::Eof
                | TokenKind::Newline
                | TokenKind::Newlines
                | TokenKind::Outdent
                | TokenKind::Punctuation(Punctuation::RightBrace | Punctuation::Semicolon)
        ) {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn parse_annotated_type(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let mut tree = self.parse_type_operand();
        while self.current().kind == TokenKind::Operator && self.current_text_is("@") {
            let annotation = self.parse_annotation();
            tree = self.alloc_from(
                mark,
                TreeKind::Annotated(Annotated {
                    expr: tree,
                    annotation,
                }),
            );
        }
        tree
    }

    fn parse_type_operand(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        if self.current().kind == TokenKind::Operator && self.current_text_is("?") {
            let wildcard = self.parse_wildcard_type(mark);
            if self.allows_wildcard_type() {
                return wildcard;
            }
            let position = self
                .ast
                .get(wildcard)
                .position
                .unwrap_or_else(|| self.current_span());
            self.report(
                ParseDiagnosticKind::ExpectedType,
                "a wildcard type is only valid as a type argument",
            );
            return self.error_type(position);
        }
        if self.current().kind == TokenKind::Operator
            && self.current_text_is("-")
            && is_numeric_type_literal(self.cursor.lookahead(1).kind)
        {
            self.advance();
            let reference = self.parse_negative_number_constant(mark);
            return self.alloc_from(
                mark,
                TreeKind::SingletonTypeTree(dotty_core::ast::SingletonTypeTree { reference }),
            );
        }
        match self.current().kind {
            TokenKind::IntegerLiteral
            | TokenKind::LongLiteral
            | TokenKind::DecimalLiteral
            | TokenKind::ExponentLiteral
            | TokenKind::FloatLiteral
            | TokenKind::DoubleLiteral
            | TokenKind::StringLiteral
            | TokenKind::CharLiteral
            | TokenKind::Keyword(HardKeyword::True)
            | TokenKind::Keyword(HardKeyword::False)
            | TokenKind::Keyword(HardKeyword::Null) => {
                return self.parse_literal_singleton_type(mark);
            }
            TokenKind::Identifier
            | TokenKind::BackquotedIdentifier
            | TokenKind::Keyword(HardKeyword::This | HardKeyword::Super) => {
                return self.simple_type();
            }
            TokenKind::Punctuation(Punctuation::LeftParen) => {
                return self.parse_parenthesized_type(mark);
            }
            _ => {}
        }

        let position = self.current_span();
        self.report(ParseDiagnosticKind::ExpectedType, "expected a type operand");
        self.error_type(position)
    }

    fn parse_literal_singleton_type(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let literal = match self.current().kind {
            TokenKind::IntegerLiteral
            | TokenKind::LongLiteral
            | TokenKind::DecimalLiteral
            | TokenKind::ExponentLiteral
            | TokenKind::FloatLiteral
            | TokenKind::DoubleLiteral => self.parse_number_constant(mark),
            TokenKind::StringLiteral => self.parse_string(mark),
            TokenKind::CharLiteral => self.parse_char(mark),
            TokenKind::Keyword(HardKeyword::True) => {
                self.parse_literal(mark, Constant::Boolean(true))
            }
            TokenKind::Keyword(HardKeyword::False) => {
                self.parse_literal(mark, Constant::Boolean(false))
            }
            TokenKind::Keyword(HardKeyword::Null) => self.parse_literal(mark, Constant::Null),
            _ => {
                let position = self.current_span();
                self.report(ParseDiagnosticKind::ExpectedType, "expected a literal type");
                return self.error_type(position);
            }
        };
        self.alloc_from(
            mark,
            TreeKind::SingletonTypeTree(dotty_core::ast::SingletonTypeTree { reference: literal }),
        )
    }

    fn parse_wildcard_type(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        let low = if self.accept_type_bound_operator(">:") {
            Some(self.type_expr())
        } else {
            None
        };
        let high = if self.accept_type_bound_operator("<:") {
            Some(self.type_expr())
        } else {
            None
        };
        self.alloc_from(
            mark,
            TreeKind::TypeBoundsTree(TypeBoundsTree {
                low,
                high,
                alias: None,
            }),
        )
    }

    fn parse_parenthesized_type(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        if self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
            let position = self.current_span();
            self.report(
                ParseDiagnosticKind::ExpectedType,
                "an empty parenthesized type requires a function type",
            );
            return self.error_type(position);
        }

        if self.starts_named_tuple_element() {
            return self.parse_named_tuple_type(mark);
        }

        let inner = self.type_expr();
        if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
            self.expect(TokenKind::Punctuation(Punctuation::RightParen));
            return self.alloc_from(
                mark,
                TreeKind::PhaseSpecific(UntypedNode::Parens(Parens { inner })),
            );
        }

        let mut elements = vec![inner];
        loop {
            if self
                .cursor
                .at(TokenKind::Punctuation(Punctuation::RightParen))
            {
                let position = self.zero_width_span(self.current().span.start());
                self.report(
                    ParseDiagnosticKind::ExpectedType,
                    "expected a type after `,`",
                );
                elements.push(self.error_type(position));
                break;
            }
            if self.current().kind == TokenKind::Eof {
                break;
            }
            elements.push(self.type_expr());
            if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                break;
            }
        }
        self.expect(TokenKind::Punctuation(Punctuation::RightParen));
        self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(dotty_core::ast::Tuple { elements })),
        )
    }

    fn starts_named_tuple_element(&mut self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) && is_function_param_colon(self.cursor.lookahead(1).kind)
    }

    fn parse_named_tuple_type(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let mut elements = Vec::new();
        loop {
            let element_mark = self.mark();
            let name = match self.intern_current_term_name() {
                Ok(name)
                    if matches!(
                        self.current().kind,
                        TokenKind::Identifier | TokenKind::BackquotedIdentifier
                    ) =>
                {
                    self.advance();
                    *name.as_name()
                }
                _ => {
                    self.report(
                        ParseDiagnosticKind::ExpectedType,
                        "expected a named tuple element",
                    );
                    self.recover_named_tuple_element();
                    if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                        continue;
                    }
                    self.accept(TokenKind::Punctuation(Punctuation::RightParen));
                    break;
                }
            };

            if !self.accept_function_param_colon() {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected `:` after named tuple element",
                );
                self.recover_named_tuple_element();
                if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                    continue;
                }
                self.accept(TokenKind::Punctuation(Punctuation::RightParen));
                break;
            }

            let element_type = self.type_expr();
            elements.push(self.alloc_from(
                element_mark,
                TreeKind::NamedArg(NamedArg {
                    name,
                    arg: element_type,
                }),
            ));

            if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                if self
                    .cursor
                    .at(TokenKind::Punctuation(Punctuation::RightParen))
                {
                    self.report(
                        ParseDiagnosticKind::ExpectedType,
                        "expected a named tuple element after `,`",
                    );
                    self.advance();
                    break;
                }
                continue;
            }

            self.expect(TokenKind::Punctuation(Punctuation::RightParen));
            break;
        }

        self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(Tuple { elements })),
        )
    }

    fn recover_named_tuple_element(&mut self) {
        while !matches!(
            self.current().kind,
            TokenKind::Punctuation(Punctuation::Comma | Punctuation::RightParen) | TokenKind::Eof
        ) {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn consume_type_infix_newlines(&mut self) {
        if !matches!(
            self.current().kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            return;
        }

        let mut offset = 0;
        while matches!(
            self.cursor.lookahead(offset).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            offset += 1;
        }
        let next = self.cursor.lookahead(offset).clone();
        if !self.can_start_type_operand(&next) {
            return;
        }

        while offset > 0 {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            offset -= 1;
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn can_start_type_operand(&self, token: &dotty_core::Token) -> bool {
        match token.kind {
            TokenKind::Identifier
            | TokenKind::BackquotedIdentifier
            | TokenKind::IntegerLiteral
            | TokenKind::LongLiteral
            | TokenKind::DecimalLiteral
            | TokenKind::ExponentLiteral
            | TokenKind::FloatLiteral
            | TokenKind::DoubleLiteral
            | TokenKind::StringLiteral
            | TokenKind::CharLiteral
            | TokenKind::Keyword(HardKeyword::True)
            | TokenKind::Keyword(HardKeyword::False)
            | TokenKind::Keyword(HardKeyword::Null)
            | TokenKind::Punctuation(Punctuation::LeftParen) => true,
            TokenKind::Operator => self
                .token_text(token)
                .ok()
                .is_some_and(|text| matches!(text, "-" | "?")),
            _ => false,
        }
    }

    fn accept_type_bound_operator(&mut self, expected: &str) -> bool {
        if matches!(
            self.current().kind,
            TokenKind::Operator | TokenKind::ColonOp
        ) && self.current_text_is(expected)
        {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Parses the small simple-type subset needed by simple expressions.
    pub(crate) fn simple_type(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let mut tree = self.simple_type_reference();

        loop {
            if self
                .cursor
                .at(TokenKind::Punctuation(Punctuation::LeftBracket))
            {
                let args = self.parse_type_argument_list(true);
                tree = self.alloc_from(
                    mark,
                    dotty_core::TreeKind::AppliedTypeTree(dotty_core::ast::AppliedTypeTree {
                        tpt: tree,
                        args,
                    }),
                );
                continue;
            }

            if !self.accept_type_projection_operator() {
                break;
            }

            let Some((name, backquoted)) = self.current_type_projection_name() else {
                self.report(
                    ParseDiagnosticKind::ExpectedType,
                    "expected a type projection member after `#`",
                );
                break;
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
        tree
    }

    fn accept_type_projection_operator(&mut self) -> bool {
        if self.current().kind == TokenKind::Operator && self.current_text_is("#") {
            self.advance();
            true
        } else {
            false
        }
    }

    fn current_type_projection_name(&mut self) -> Option<(Name, bool)> {
        if !matches!(
            self.current().kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) {
            return None;
        }

        let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
        let name = *self.intern_current_type_name().ok()?.as_name();
        Some((name, backquoted))
    }

    /// Parses a qualified type reference or a path singleton type. Grammar
    /// positions whose suffixes have a distinct meaning (for example the
    /// current `derives` production) use this shared type entry point.
    pub(crate) fn simple_type_reference(&mut self) -> TreeId<Untyped> {
        if self.type_quote_depth > 0
            && (self.current_starts_braced_splice() || self.current_starts_simple_splice())
        {
            return self.parse_legacy_type_splice();
        }

        if self.starts_path_singleton_type() {
            let mark = self.mark();
            let Ok(reference) = self.parse_qualified_reference_until_keyword(
                ReferenceNamespace::Term,
                Some(HardKeyword::Type),
            ) else {
                let position = self.current_span();
                self.report(
                    ParseDiagnosticKind::ExpectedType,
                    "expected a path before `.type`",
                );
                return self.error_type(position);
            };
            self.expect(TokenKind::Punctuation(Punctuation::Dot));
            self.expect(TokenKind::Keyword(HardKeyword::Type));
            return self.alloc_from(
                mark,
                TreeKind::SingletonTypeTree(dotty_core::ast::SingletonTypeTree { reference }),
            );
        }

        if self.starts_this_or_super_type_reference() {
            let mark = self.mark();
            let Ok(reference) = self.parse_this_or_super_reference(mark) else {
                let position = self.current_span();
                self.report(
                    ParseDiagnosticKind::ExpectedType,
                    "expected a path before `.type`",
                );
                return self.error_type(position);
            };

            if self.current().kind == TokenKind::Punctuation(Punctuation::Dot)
                && self.cursor.lookahead(1).kind == TokenKind::Keyword(HardKeyword::Type)
            {
                self.advance();
                self.advance();
                return self.alloc_from(
                    mark,
                    TreeKind::SingletonTypeTree(dotty_core::ast::SingletonTypeTree { reference }),
                );
            }

            self.convert_this_or_super_reference_to_type_namespace(reference);
            return reference;
        }

        match self.parse_qualified_reference(ReferenceNamespace::Type) {
            Ok(tree) => tree,
            Err(QualifiedReferenceError::MissingInitial) => {
                let position = self.current_span();
                self.report(ParseDiagnosticKind::ExpectedType, "expected a simple type");
                if !is_type_recovery_boundary(self.current().kind) {
                    self.advance();
                }
                self.error_type(position)
            }
            Err(QualifiedReferenceError::MissingSegment) => {
                self.report(
                    ParseDiagnosticKind::ExpectedType,
                    "expected a type name after `.`",
                );
                self.error_type(self.current_span())
            }
        }
    }

    fn parse_legacy_type_splice(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        if self.current_starts_simple_splice() {
            self.advance();
        } else {
            self.advance(); // `$`
            self.expect(TokenKind::Punctuation(Punctuation::LeftBrace));
            let (_stats, _expr) =
                self.parse_expression_block_body(TokenKind::Punctuation(Punctuation::RightBrace));
            if !self.accept(TokenKind::Punctuation(Punctuation::RightBrace)) {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected `}` to close legacy type splice",
                );
            }
        }
        self.report(
            ParseDiagnosticKind::UnsupportedSyntax,
            "type splicing with `$` inside a quoted type is no longer supported",
        );
        self.error_type(self.span_from(mark))
    }

    fn parse_this_or_super_reference(
        &mut self,
        mark: crate::Mark,
    ) -> Result<TreeId<Untyped>, QualifiedReferenceError> {
        let current_kind = self.current().kind;
        let mut tree = match current_kind {
            TokenKind::Keyword(HardKeyword::This) => {
                self.advance();
                self.alloc_from(mark, TreeKind::This(dotty_core::ast::This { qual: None }))
            }
            TokenKind::Keyword(HardKeyword::Super) => {
                let mix_start = (self.cursor.lookahead(1).kind
                    == TokenKind::Punctuation(Punctuation::LeftBracket))
                .then(|| self.cursor.lookahead(2).span.start());
                let mix_position = mix_start.map(|start| self.zero_width_span_at(start));
                let tree = self.parse_super(mark, None);
                if let Some(position) = mix_position {
                    self.adjust_mixin_super_qualifier_span(tree, position);
                }
                tree
            }
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
                if self.cursor.lookahead(1).kind == TokenKind::Punctuation(Punctuation::Dot)
                    && self.cursor.lookahead(2).kind == TokenKind::Keyword(HardKeyword::This) =>
            {
                let Ok(name) = self.intern_current_type_name() else {
                    return Err(QualifiedReferenceError::MissingInitial);
                };
                self.advance();
                self.expect(TokenKind::Punctuation(Punctuation::Dot));
                self.expect(TokenKind::Keyword(HardKeyword::This));
                self.alloc_from(
                    mark,
                    TreeKind::This(dotty_core::ast::This {
                        qual: Some(*name.as_name()),
                    }),
                )
            }
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
                if self.cursor.lookahead(1).kind == TokenKind::Punctuation(Punctuation::Dot)
                    && self.cursor.lookahead(2).kind == TokenKind::Keyword(HardKeyword::Super) =>
            {
                let qualifier_start = self.current().span.start();
                let super_start = self.cursor.lookahead(2).span.start();
                let mix_position = (self.cursor.lookahead(3).kind
                    == TokenKind::Punctuation(Punctuation::LeftBracket))
                .then(|| self.span_at(qualifier_start, super_start));
                let tree = self.parse_qualified_super(mark);
                if let Some(position) = mix_position {
                    self.adjust_mixin_super_qualifier_span(tree, position);
                }
                tree
            }
            _ => return Err(QualifiedReferenceError::MissingInitial),
        };

        while self.cursor.at(TokenKind::Punctuation(Punctuation::Dot))
            && self.cursor.lookahead(1).kind != TokenKind::Keyword(HardKeyword::Type)
        {
            self.advance();
            let Some((name, backquoted)) = self.current_term_reference_name() else {
                return Err(QualifiedReferenceError::MissingSegment);
            };
            self.advance();
            tree = self.alloc_from(
                mark,
                TreeKind::Select(dotty_core::ast::Select {
                    qualifier: tree,
                    name,
                    backquoted,
                }),
            );
        }

        Ok(tree)
    }

    fn zero_width_span_at(&self, start: u32) -> SourceSpan {
        self.span_at(start, start)
    }

    fn span_at(&self, start: u32, end: u32) -> SourceSpan {
        let range = dotty_core::TextRange::new(start, end).expect("ordered source span");
        SourceSpan::new(self.source_id, Span::without_point(range))
    }

    fn adjust_mixin_super_qualifier_span(&mut self, tree: TreeId<Untyped>, position: SourceSpan) {
        let super_id = match self.ast.get(tree).kind {
            TreeKind::Select(select) => select.qualifier,
            _ => return,
        };
        let qualifier = match self.ast.get(super_id).kind {
            TreeKind::Super(Super { qual, mix: Some(_) }) => qual,
            _ => return,
        };
        self.ast.get_mut(qualifier).position = Some(position);
    }

    fn current_term_reference_name(&mut self) -> Option<(Name, bool)> {
        if !matches!(
            self.current().kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) {
            return None;
        }

        let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
        let name = *self.intern_current_term_name().ok()?.as_name();
        Some((name, backquoted))
    }

    fn convert_this_or_super_reference_to_type_namespace(&mut self, tree: TreeId<Untyped>) {
        let qualifier = match self.ast.get(tree).kind {
            TreeKind::Select(selection) => selection.qualifier,
            _ => return,
        };
        self.convert_this_or_super_reference_to_type_namespace(qualifier);
        if let TreeKind::Select(selection) = &mut self.ast.get_mut(tree).kind {
            selection.name = *dotty_core::TypeName::new(selection.name.text()).as_name();
        }
    }

    fn starts_this_or_super_type_reference(&mut self) -> bool {
        match self.current().kind {
            TokenKind::Keyword(HardKeyword::This) => {
                self.cursor.lookahead(1).kind == TokenKind::Punctuation(Punctuation::Dot)
            }
            TokenKind::Keyword(HardKeyword::Super) => {
                matches!(
                    self.cursor.lookahead(1).kind,
                    TokenKind::Punctuation(Punctuation::Dot | Punctuation::LeftBracket)
                )
            }
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                self.cursor.lookahead(1).kind == TokenKind::Punctuation(Punctuation::Dot)
                    && matches!(
                        self.cursor.lookahead(2).kind,
                        TokenKind::Keyword(HardKeyword::This | HardKeyword::Super)
                    )
            }
            _ => false,
        }
    }

    fn starts_path_singleton_type(&mut self) -> bool {
        let mut offset = 0;
        if !matches!(
            self.cursor.lookahead(offset).kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) {
            return false;
        }

        loop {
            if self.cursor.lookahead(offset + 1).kind != TokenKind::Punctuation(Punctuation::Dot) {
                return false;
            }
            if self.cursor.lookahead(offset + 2).kind == TokenKind::Keyword(HardKeyword::Type) {
                return true;
            }
            if !matches!(
                self.cursor.lookahead(offset + 2).kind,
                TokenKind::Identifier | TokenKind::BackquotedIdentifier
            ) {
                return false;
            }
            offset += 2;
        }
    }
}

const fn is_type_recovery_boundary(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Eof
            | TokenKind::Newline
            | TokenKind::Newlines
            | TokenKind::Indent
            | TokenKind::Outdent
            | TokenKind::ColonFollow
            | TokenKind::ColonOp
            | TokenKind::ColonEol
            | TokenKind::Punctuation(
                Punctuation::Comma
                    | Punctuation::Colon
                    | Punctuation::LeftBrace
                    | Punctuation::RightBrace
                    | Punctuation::RightBracket
                    | Punctuation::RightParen
            )
    )
}

const fn is_numeric_type_literal(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::IntegerLiteral
            | TokenKind::LongLiteral
            | TokenKind::DecimalLiteral
            | TokenKind::ExponentLiteral
            | TokenKind::FloatLiteral
            | TokenKind::DoubleLiteral
    )
}

const fn is_function_param_colon(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Punctuation(Punctuation::Colon)
            | TokenKind::ColonOp
            | TokenKind::ColonFollow
            | TokenKind::ColonEol
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FunctionTypeArrow {
    Ordinary,
    Context,
}

struct NamedFunctionParams {
    params: Vec<TreeId<Untyped>>,
    erased_params: Vec<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::{
        CaseDef, ContextBoundTypeTree, ContextBounds, DefDef, InfixOp, LambdaTypeTree,
        MatchTypeTree, RefinedTypeTree, Select, Super, This, TypeDef, ValDef,
    };
    use dotty_core::{HardKeyword, NameInterner, Punctuation, TextRange, TreeKind};

    #[test]
    fn parses_a_braced_match_type_with_ordered_cases() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "T match { case String => Int; case Int => Long }",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Keyword(HardKeyword::Match), 2, 7),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 8, 9),
                token(TokenKind::Keyword(HardKeyword::Case), 10, 14),
                token(TokenKind::Identifier, 15, 21),
                token(TokenKind::Operator, 22, 24),
                token(TokenKind::Identifier, 25, 28),
                token(TokenKind::Punctuation(Punctuation::Semicolon), 28, 29),
                token(TokenKind::Keyword(HardKeyword::Case), 30, 34),
                token(TokenKind::Identifier, 35, 38),
                token(TokenKind::Operator, 39, 41),
                token(TokenKind::Identifier, 42, 46),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 47, 48),
                token(TokenKind::Eof, 48, 48),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::MatchTypeTree(MatchTypeTree {
            bound,
            selector,
            ref cases,
        }) = parser.ast().get(id).kind
        else {
            panic!("expected a match type");
        };
        assert!(bound.is_none());
        assert!(matches!(
            parser.ast().get(selector).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(cases.len(), 2);
        assert!(matches!(
            parser.ast().get(cases[0]).kind,
            TreeKind::CaseDef(_)
        ));
        assert!(matches!(
            parser.ast().get(cases[1]).kind,
            TreeKind::CaseDef(_)
        ));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 48).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_match_type_wildcard_case_as_an_identifier() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "T match { case _ => C }",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Keyword(HardKeyword::Match), 2, 7),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 8, 9),
                token(TokenKind::Keyword(HardKeyword::Case), 10, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Operator, 17, 19),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 22, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::MatchTypeTree(MatchTypeTree { ref cases, .. }) = parser.ast().get(id).kind
        else {
            panic!("expected a match type");
        };
        let TreeKind::CaseDef(CaseDef { pattern, body, .. }) = parser.ast().get(cases[0]).kind
        else {
            panic!("expected a type case");
        };
        let TreeKind::Ident(wildcard) = parser.ast().get(pattern).kind else {
            panic!("expected wildcard identifier pattern");
        };
        assert_eq!(parser.names.resolve(wildcard.name.text()), "_");
        assert!(matches!(parser.ast().get(body).kind, TreeKind::Ident(_)));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_infix_types_in_match_type_selector_and_case_pattern() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A | B match { case C | D => E }",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Keyword(HardKeyword::Match), 6, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 12, 13),
                token(TokenKind::Keyword(HardKeyword::Case), 14, 18),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Operator, 21, 22),
                token(TokenKind::Identifier, 23, 24),
                token(TokenKind::Operator, 25, 27),
                token(TokenKind::Identifier, 28, 29),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 30, 31),
                token(TokenKind::Eof, 31, 31),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::MatchTypeTree(MatchTypeTree {
            selector,
            ref cases,
            ..
        }) = parser.ast().get(id).kind
        else {
            panic!("expected a match type");
        };
        assert!(matches!(
            parser.ast().get(selector).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(InfixOp { .. }))
        ));
        let TreeKind::CaseDef(CaseDef { pattern, .. }) = parser.ast().get(cases[0]).kind else {
            panic!("expected a type case");
        };
        assert!(matches!(
            parser.ast().get(pattern).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(InfixOp { .. }))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_match_type_results_as_full_types() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "T match { case _ => A => B }",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Keyword(HardKeyword::Match), 2, 7),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 8, 9),
                token(TokenKind::Keyword(HardKeyword::Case), 10, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Operator, 17, 19),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Operator, 22, 24),
                token(TokenKind::Identifier, 25, 26),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 27, 28),
                token(TokenKind::Eof, 28, 28),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::MatchTypeTree(MatchTypeTree { ref cases, .. }) = parser.ast().get(id).kind
        else {
            panic!("expected a match type");
        };
        let TreeKind::CaseDef(CaseDef { body, .. }) = parser.ast().get(cases[0]).kind else {
            panic!("expected a type case");
        };
        assert!(matches!(
            parser.ast().get(body).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_match_type_case_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "T match { case A C case B => D }",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Keyword(HardKeyword::Match), 2, 7),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 8, 9),
                token(TokenKind::Keyword(HardKeyword::Case), 10, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Keyword(HardKeyword::Case), 19, 23),
                token(TokenKind::Identifier, 24, 25),
                token(TokenKind::Operator, 26, 28),
                token(TokenKind::Identifier, 29, 30),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 31, 32),
                token(TokenKind::Eof, 32, 32),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::MatchTypeTree(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_match_type_with_a_missing_result_and_brace() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "T match { case A =>",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Keyword(HardKeyword::Match), 2, 7),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 8, 9),
                token(TokenKind::Keyword(HardKeyword::Case), 10, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Operator, 17, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::MatchTypeTree(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_parenthesized_type_as_parens() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightParen), 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Parens(Parens { inner })) =
            parser.ast().get(id).kind
        else {
            panic!("expected a parenthesized type");
        };
        assert!(matches!(parser.ast().get(inner).kind, TreeKind::Ident(_)));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 3).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_an_unbounded_wildcard_as_a_nested_type_argument() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[?]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Operator, 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::AppliedTypeTree(ref applied) = parser.ast().get(id).kind else {
            panic!("expected an applied type");
        };
        assert_eq!(applied.args.len(), 1);
        let TreeKind::TypeBoundsTree(bounds) = parser.ast().get(applied.args[0]).kind else {
            panic!("expected a wildcard type bounds tree");
        };
        assert!(bounds.low.is_none() && bounds.high.is_none() && bounds.alias.is_none());
        assert_eq!(
            parser
                .ast()
                .get(applied.args[0])
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(5, 6).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn preserves_a_backquoted_question_type_name_inside_type_arguments() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[`?`]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::BackquotedIdentifier, 5, 8),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::AppliedTypeTree(ref applied) = parser.ast().get(id).kind else {
            panic!("expected an applied type");
        };
        assert!(matches!(
            parser.ast().get(applied.args[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn rejects_a_top_level_wildcard_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "?",
            vec![
                token(TokenKind::Operator, 0, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedType)
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_wildcard_with_an_upper_bound() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[? <: Number]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Operator, 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Identifier, 10, 16),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::AppliedTypeTree(ref applied) = parser.ast().get(id).kind else {
            panic!("expected an applied type");
        };
        let TreeKind::TypeBoundsTree(bounds) = parser.ast().get(applied.args[0]).kind else {
            panic!("expected wildcard bounds");
        };
        assert!(bounds.low.is_none());
        let Some(high) = bounds.high else {
            panic!("expected an upper bound");
        };
        assert!(matches!(parser.ast().get(high).kind, TreeKind::Ident(_)));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_wildcard_with_a_lower_bound() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[? >: String]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Operator, 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Identifier, 10, 16),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::AppliedTypeTree(ref applied) = parser.ast().get(id).kind else {
            panic!("expected an applied type");
        };
        let TreeKind::TypeBoundsTree(bounds) = parser.ast().get(applied.args[0]).kind else {
            panic!("expected wildcard bounds");
        };
        let Some(low) = bounds.low else {
            panic!("expected a lower bound");
        };
        assert!(matches!(parser.ast().get(low).kind, TreeKind::Ident(_)));
        assert!(bounds.high.is_none());
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_wildcard_with_both_bounds() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[? >: Low <: High]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Operator, 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Identifier, 10, 13),
                token(TokenKind::Operator, 14, 16),
                token(TokenKind::Identifier, 17, 21),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 21, 22),
                token(TokenKind::Eof, 22, 22),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::AppliedTypeTree(ref applied) = parser.ast().get(id).kind else {
            panic!("expected an applied type");
        };
        let TreeKind::TypeBoundsTree(bounds) = parser.ast().get(applied.args[0]).kind else {
            panic!("expected wildcard bounds");
        };
        assert!(matches!(
            bounds.low.map(|id| &parser.ast().get(id).kind),
            Some(TreeKind::Ident(_))
        ));
        assert!(matches!(
            bounds.high.map(|id| &parser.ast().get(id).kind),
            Some(TreeKind::Ident(_))
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_wildcard_inside_a_nested_applied_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "Map[String, List[? <: Number]]",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 3, 4),
                token(TokenKind::Identifier, 4, 10),
                token(TokenKind::Punctuation(Punctuation::Comma), 10, 11),
                token(TokenKind::Identifier, 12, 16),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 16, 17),
                token(TokenKind::Operator, 17, 18),
                token(TokenKind::Operator, 19, 21),
                token(TokenKind::Identifier, 22, 28),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 28, 29),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 29, 30),
                token(TokenKind::Eof, 30, 30),
            ],
            &mut names,
        );

        let outer_id = parser.type_expr();
        let TreeKind::AppliedTypeTree(ref outer) = parser.ast().get(outer_id).kind else {
            panic!("expected the outer applied type");
        };
        assert_eq!(outer.args.len(), 2);
        let inner_id = outer.args[1];
        let TreeKind::AppliedTypeTree(ref inner) = parser.ast().get(inner_id).kind else {
            panic!("expected the nested applied type");
        };
        let TreeKind::TypeBoundsTree(bounds) = parser.ast().get(inner.args[0]).kind else {
            panic!("expected nested wildcard bounds");
        };
        assert!(bounds.low.is_none() && bounds.high.is_some());
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_wildcard_bound_before_the_closer() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[? <: ]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Operator, 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::AppliedTypeTree(_)
        ));
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedType)
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_wildcard_closer_at_eof() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[? >: String",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Operator, 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Identifier, 10, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let _ = parser.type_expr();
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedToken)
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn preserves_a_following_type_argument_after_a_missing_wildcard_bound() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[? <: , String]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Operator, 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Punctuation(Punctuation::Comma), 10, 11),
                token(TokenKind::Identifier, 12, 18),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::AppliedTypeTree(ref applied) = parser.ast().get(id).kind else {
            panic!("expected an applied type");
        };
        assert_eq!(applied.args.len(), 2);
        assert!(matches!(
            parser.ast().get(applied.args[1]).kind,
            TreeKind::Ident(_)
        ));
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedType)
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parenthesized_type_parses_a_full_inner_type_expression() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A | B)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Operator, 3, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Parens(Parens { inner })) =
            parser.ast().get(id).kind
        else {
            panic!("expected a parenthesized type");
        };
        assert!(matches!(
            parser.ast().get(inner).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_two_element_tuple_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A, B)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Tuple(ref tuple)) = parser.ast().get(id).kind
        else {
            panic!("expected a tuple type");
        };
        assert_eq!(tuple.elements.len(), 2);
        assert!(
            tuple
                .elements
                .iter()
                .all(|element| matches!(parser.ast().get(*element).kind, TreeKind::Ident(_)))
        );
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 6).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_named_tuple_type_elements_as_named_args() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(name: String, age: Int)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 5),
                token(TokenKind::Punctuation(Punctuation::Colon), 5, 6),
                token(TokenKind::Identifier, 7, 13),
                token(TokenKind::Punctuation(Punctuation::Comma), 13, 14),
                token(TokenKind::Identifier, 15, 18),
                token(TokenKind::Punctuation(Punctuation::Colon), 18, 19),
                token(TokenKind::Identifier, 20, 23),
                token(TokenKind::Punctuation(Punctuation::RightParen), 23, 24),
                token(TokenKind::Eof, 24, 24),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) = &parser.ast().get(id).kind else {
            panic!("expected a named tuple type");
        };
        let elements = tuple.elements.clone();
        assert_eq!(elements.len(), 2);
        let element_names = elements
            .iter()
            .map(|element| match &parser.ast().get(*element).kind {
                TreeKind::NamedArg(named) => named.name.text(),
                _ => panic!("expected a named tuple element"),
            })
            .collect::<Vec<_>>();
        assert_eq!(parser.names.resolve(element_names[0]), "name");
        assert_eq!(parser.names.resolve(element_names[1]), "age");
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn keeps_named_function_type_disambiguation() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(name: String, age: Int) => Result",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 5),
                token(TokenKind::Punctuation(Punctuation::Colon), 5, 6),
                token(TokenKind::Identifier, 7, 13),
                token(TokenKind::Punctuation(Punctuation::Comma), 13, 14),
                token(TokenKind::Identifier, 15, 18),
                token(TokenKind::Punctuation(Punctuation::Colon), 18, 19),
                token(TokenKind::Identifier, 20, 23),
                token(TokenKind::Punctuation(Punctuation::RightParen), 23, 24),
                token(TokenKind::Operator, 25, 27),
                token(TokenKind::Identifier, 28, 34),
                token(TokenKind::Eof, 34, 34),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_full_type_expressions_in_named_tuple_elements() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(items: List[A], callback: X => Y)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 6),
                token(TokenKind::Punctuation(Punctuation::Colon), 6, 7),
                token(TokenKind::Identifier, 8, 12),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 12, 13),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 14, 15),
                token(TokenKind::Punctuation(Punctuation::Comma), 15, 16),
                token(TokenKind::Identifier, 17, 25),
                token(TokenKind::Punctuation(Punctuation::Colon), 25, 26),
                token(TokenKind::Identifier, 27, 28),
                token(TokenKind::Operator, 29, 31),
                token(TokenKind::Identifier, 32, 33),
                token(TokenKind::Punctuation(Punctuation::RightParen), 33, 34),
                token(TokenKind::Eof, 34, 34),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) = &parser.ast().get(id).kind else {
            panic!("expected a named tuple type");
        };
        assert!(matches!(
            parser
                .ast()
                .get(match &parser.ast().get(tuple.elements[0]).kind {
                    TreeKind::NamedArg(named) => named.arg,
                    _ => panic!("expected a named tuple element"),
                })
                .kind,
            TreeKind::AppliedTypeTree(_)
        ));
        assert!(matches!(
            parser
                .ast()
                .get(match &parser.ast().get(tuple.elements[1]).kind {
                    TreeKind::NamedArg(named) => named.arg,
                    _ => panic!("expected a named tuple element"),
                })
                .kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_trailing_comma_in_a_named_tuple_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(name: String,)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 5),
                token(TokenKind::Punctuation(Punctuation::Colon), 5, 6),
                token(TokenKind::Identifier, 7, 13),
                token(TokenKind::Punctuation(Punctuation::Comma), 13, 14),
                token(TokenKind::Punctuation(Punctuation::RightParen), 14, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| matches!(diagnostic.kind(), ParseDiagnosticKind::ExpectedType))
        );
    }

    #[test]
    fn recovers_a_missing_named_tuple_element_colon() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(name: String, age)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 5),
                token(TokenKind::Punctuation(Punctuation::Colon), 5, 6),
                token(TokenKind::Identifier, 7, 13),
                token(TokenKind::Punctuation(Punctuation::Comma), 13, 14),
                token(TokenKind::Identifier, 15, 18),
                token(TokenKind::Punctuation(Punctuation::RightParen), 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| matches!(diagnostic.kind(), ParseDiagnosticKind::ExpectedToken))
        );
    }

    #[test]
    fn tuple_type_elements_use_full_type_expressions() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A | B, C & D)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Operator, 3, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::Comma), 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Operator, 10, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Tuple(ref tuple)) = parser.ast().get(id).kind
        else {
            panic!("expected a tuple type");
        };
        assert_eq!(tuple.elements.len(), 2);
        assert!(tuple.elements.iter().all(|element| matches!(
            parser.ast().get(*element).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        )));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_nested_tuple_types_without_flattening_them() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "((A, B), C)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::Comma), 3, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Punctuation(Punctuation::Comma), 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Tuple(ref outer)) = parser.ast().get(id).kind
        else {
            panic!("expected an outer tuple type");
        };
        assert!(matches!(
            parser.ast().get(outer.elements[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
        assert_eq!(outer.elements.len(), 2);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_tuple_types_inside_applied_type_arguments() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[(A, B)]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 5, 6),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::Comma), 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::AppliedTypeTree(ref applied) = parser.ast().get(id).kind else {
            panic!("expected an applied type");
        };
        assert!(matches!(
            parser.ast().get(applied.args[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_an_empty_parenthesized_type_without_building_an_empty_tuple() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "()",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Punctuation(Punctuation::RightParen), 1, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(!matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(matches!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedType
        ));
    }

    #[test]
    fn recovers_an_unterminated_tuple_type_at_eof() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A,",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| matches!(diagnostic.kind(), ParseDiagnosticKind::ExpectedToken))
        );
    }

    #[test]
    fn recovers_a_missing_tuple_element_without_swallowing_the_closer() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A,, B)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Punctuation(Punctuation::Comma), 3, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Tuple(ref tuple)) = parser.ast().get(id).kind
        else {
            panic!("expected a recovered tuple type");
        };
        assert_eq!(tuple.elements.len(), 3);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| matches!(diagnostic.kind(), ParseDiagnosticKind::ExpectedType))
        );
    }

    #[test]
    fn reports_a_trailing_tuple_comma_with_a_missing_element() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A,)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Punctuation(Punctuation::RightParen), 3, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Tuple(ref tuple)) = parser.ast().get(id).kind
        else {
            panic!("expected a recovered tuple type");
        };
        assert_eq!(tuple.elements.len(), 2);
        assert!(matches!(
            parser.ast().get(tuple.elements[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| matches!(diagnostic.kind(), ParseDiagnosticKind::ExpectedType))
        );
    }

    #[test]
    fn parses_a_function_type_after_a_tuple_type() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "type F = (A, B) => C",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::Comma), 11, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Punctuation(Punctuation::RightParen), 14, 15),
                token(TokenKind::Operator, 16, 18),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn parses_a_simple_type_in_the_type_namespace() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "Value",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.simple_type();
        let TreeKind::Ident(ident) = parser.ast().get(id).kind else {
            panic!("expected type identifier");
        };

        assert!(ident.name.is_type());
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 5).unwrap()
        );
        let name = ident.name;
        drop(parser);
        assert_eq!(names.resolve(name.text()), "Value");
    }

    #[test]
    fn parses_a_path_singleton_type_with_a_term_reference() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "x.type",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::Dot), 1, 2),
                token(TokenKind::Keyword(HardKeyword::Type), 2, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a singleton type tree");
        };
        let TreeKind::Ident(reference) = parser.ast().get(singleton.reference).kind else {
            panic!("expected a term reference");
        };
        assert!(reference.name.is_term());
        let reference_name = reference.name.text();
        let span = parser.ast().get(id).position.unwrap().span().range();
        let diagnostics_empty = parser.diagnostics().is_empty();
        let at_eof = parser.current().kind == TokenKind::Eof;
        drop(parser);
        assert_eq!(names.resolve(reference_name), "x");
        assert_eq!(span, TextRange::new(0, 6).unwrap());
        assert!(diagnostics_empty);
        assert!(at_eof);
    }

    #[test]
    fn parses_a_string_literal_as_a_singleton_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "\"foo\"",
            vec![
                token(TokenKind::StringLiteral, 0, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a literal singleton type");
        };
        assert!(matches!(
            parser.ast().get(singleton.reference).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: Constant::String(_)
            })
        ));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 5).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_character_literal_as_a_singleton_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "'a'",
            vec![
                token(TokenKind::CharLiteral, 0, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a literal singleton type");
        };
        assert!(matches!(
            parser.ast().get(singleton.reference).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: Constant::Char(0x61)
            })
        ));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 3).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn decodes_an_escaped_character_literal_as_a_singleton_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            r#"'\n'"#,
            vec![
                token(TokenKind::CharLiteral, 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a literal singleton type");
        };
        assert!(matches!(
            parser.ast().get(singleton.reference).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: Constant::Char(0x0a)
            })
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_integer_literal_as_a_singleton_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "42",
            vec![
                token(TokenKind::IntegerLiteral, 0, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a literal singleton type");
        };
        assert!(matches!(
            parser.ast().get(singleton.reference).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: Constant::Int(42)
            })
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_negative_integer_literal_as_a_singleton_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "-42",
            vec![
                token(TokenKind::Operator, 0, 1),
                token(TokenKind::IntegerLiteral, 1, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a literal singleton type");
        };
        assert!(matches!(
            parser.ast().get(singleton.reference).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: Constant::Int(-42)
            })
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_long_literal_as_a_singleton_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "1L",
            vec![
                token(TokenKind::LongLiteral, 0, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a literal singleton type");
        };
        assert!(matches!(
            parser.ast().get(singleton.reference).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: Constant::Long(1)
            })
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_float_literal_as_a_singleton_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "1.5f",
            vec![
                token(TokenKind::FloatLiteral, 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a literal singleton type");
        };
        assert!(matches!(
            parser.ast().get(singleton.reference).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: Constant::FloatBits(0x3fc0_0000)
            })
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_double_literal_as_a_singleton_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "1.5d",
            vec![
                token(TokenKind::DoubleLiteral, 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a literal singleton type");
        };
        assert!(matches!(
            parser.ast().get(singleton.reference).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: Constant::DoubleBits(0x3ff8_0000_0000_0000)
            })
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_true_as_a_boolean_singleton_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "true",
            vec![
                token(TokenKind::Keyword(HardKeyword::True), 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a literal singleton type");
        };
        assert!(matches!(
            parser.ast().get(singleton.reference).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: Constant::Boolean(true)
            })
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_false_as_a_boolean_singleton_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "false",
            vec![
                token(TokenKind::Keyword(HardKeyword::False), 0, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a literal singleton type");
        };
        assert!(matches!(
            parser.ast().get(singleton.reference).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: Constant::Boolean(false)
            })
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_null_as_a_singleton_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "null",
            vec![
                token(TokenKind::Keyword(HardKeyword::Null), 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a literal singleton type");
        };
        assert!(matches!(
            parser.ast().get(singleton.reference).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: Constant::Null
            })
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn composes_a_literal_singleton_type_inside_an_applied_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[42]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::IntegerLiteral, 5, 7),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::AppliedTypeTree(ref applied) = parser.ast().get(id).kind else {
            panic!("expected an applied type");
        };
        assert!(matches!(
            parser.ast().get(applied.args[0]).kind,
            TreeKind::SingletonTypeTree(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn composes_string_literal_singletons_with_a_union() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "\"a\" | \"b\"",
            vec![
                token(TokenKind::StringLiteral, 0, 3),
                token(TokenKind::Operator, 4, 5),
                token(TokenKind::StringLiteral, 6, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) = parser.ast().get(id).kind else {
            panic!("expected a union type");
        };
        assert!(matches!(
            parser.ast().get(infix.left).kind,
            TreeKind::SingletonTypeTree(_)
        ));
        assert!(matches!(
            parser.ast().get(infix.right).kind,
            TreeKind::SingletonTypeTree(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_qualified_path_singleton_type_in_the_term_namespace() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "foo.bar.type",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::Dot), 3, 4),
                token(TokenKind::Identifier, 4, 7),
                token(TokenKind::Punctuation(Punctuation::Dot), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Type), 8, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a singleton type tree");
        };
        let TreeKind::Select(selection) = parser.ast().get(singleton.reference).kind else {
            panic!("expected a qualified term reference");
        };
        assert!(selection.name.is_term());
        assert!(matches!(
            parser.ast().get(selection.qualifier).kind,
            TreeKind::Ident(ident) if ident.name.is_term()
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_this_type_as_a_singleton_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "this.type",
            vec![
                token(TokenKind::Keyword(HardKeyword::This), 0, 4),
                token(TokenKind::Punctuation(Punctuation::Dot), 4, 5),
                token(TokenKind::Keyword(HardKeyword::Type), 5, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a singleton type tree");
        };
        assert!(matches!(
            parser.ast().get(singleton.reference).kind,
            TreeKind::This(This { qual: None })
        ));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 9).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_qualified_this_type_as_a_singleton_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "Outer.this.type",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Punctuation(Punctuation::Dot), 5, 6),
                token(TokenKind::Keyword(HardKeyword::This), 6, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Keyword(HardKeyword::Type), 11, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a singleton type tree");
        };
        let TreeKind::This(This { qual: Some(outer) }) = parser.ast().get(singleton.reference).kind
        else {
            panic!("expected a qualified this reference");
        };
        assert!(parser.diagnostics().is_empty());
        assert!(outer.is_type());
        drop(parser);
        assert_eq!(names.resolve(outer.text()), "Outer");
    }

    #[test]
    fn parses_super_member_type_as_a_singleton_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "super.member.type",
            vec![
                token(TokenKind::Keyword(HardKeyword::Super), 0, 5),
                token(TokenKind::Punctuation(Punctuation::Dot), 5, 6),
                token(TokenKind::Identifier, 6, 12),
                token(TokenKind::Punctuation(Punctuation::Dot), 12, 13),
                token(TokenKind::Keyword(HardKeyword::Type), 13, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a singleton type tree");
        };
        let TreeKind::Select(Select {
            qualifier, name, ..
        }) = parser.ast().get(singleton.reference).kind
        else {
            panic!("expected a selected super member");
        };
        assert!(name.is_term());
        assert!(matches!(
            parser.ast().get(qualifier).kind,
            TreeKind::Super(Super { mix: None, .. })
        ));
        assert!(parser.diagnostics().is_empty());
        let selected_name = name;
        drop(parser);
        assert_eq!(names.resolve(selected_name.text()), "member");
    }

    #[test]
    fn parses_qualified_super_member_type_as_a_singleton_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "Outer.super.member.type",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Punctuation(Punctuation::Dot), 5, 6),
                token(TokenKind::Keyword(HardKeyword::Super), 6, 11),
                token(TokenKind::Punctuation(Punctuation::Dot), 11, 12),
                token(TokenKind::Identifier, 12, 18),
                token(TokenKind::Punctuation(Punctuation::Dot), 18, 19),
                token(TokenKind::Keyword(HardKeyword::Type), 19, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a singleton type tree");
        };
        let TreeKind::Select(Select {
            qualifier, name, ..
        }) = parser.ast().get(singleton.reference).kind
        else {
            panic!("expected a selected super member");
        };
        let TreeKind::Super(Super { qual, mix }) = parser.ast().get(qualifier).kind else {
            panic!("expected a qualified super reference");
        };
        assert!(mix.is_none());
        assert!(matches!(
            parser.ast().get(qual).kind,
            TreeKind::This(This { qual: Some(_) })
        ));
        assert!(parser.diagnostics().is_empty());
        let selected_name = name;
        drop(parser);
        assert_eq!(names.resolve(selected_name.text()), "member");
    }

    #[test]
    fn parses_mixin_qualified_super_member_type_as_a_singleton_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "super[Base].member.type",
            vec![
                token(TokenKind::Keyword(HardKeyword::Super), 0, 5),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 5, 6),
                token(TokenKind::Identifier, 6, 10),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 10, 11),
                token(TokenKind::Punctuation(Punctuation::Dot), 11, 12),
                token(TokenKind::Identifier, 12, 18),
                token(TokenKind::Punctuation(Punctuation::Dot), 18, 19),
                token(TokenKind::Keyword(HardKeyword::Type), 19, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::SingletonTypeTree(singleton) = parser.ast().get(id).kind else {
            panic!("expected a singleton type tree");
        };
        let TreeKind::Select(Select { qualifier, .. }) = parser.ast().get(singleton.reference).kind
        else {
            panic!("expected a selected super member");
        };
        let TreeKind::Super(Super { mix, .. }) = parser.ast().get(qualifier).kind else {
            panic!("expected a super reference");
        };
        assert!(mix.is_some());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_this_member_type_as_an_ordinary_selection() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "this.member",
            vec![
                token(TokenKind::Keyword(HardKeyword::This), 0, 4),
                token(TokenKind::Punctuation(Punctuation::Dot), 4, 5),
                token(TokenKind::Identifier, 5, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::Select(Select {
            qualifier, name, ..
        }) = parser.ast().get(id).kind
        else {
            panic!("expected an ordinary this selection");
        };
        assert!(matches!(
            parser.ast().get(qualifier).kind,
            TreeKind::This(This { qual: None })
        ));
        assert!(name.is_type());
        assert!(parser.diagnostics().is_empty());
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 11).unwrap()
        );
    }

    #[test]
    fn parses_super_member_type_as_an_ordinary_selection() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "super.member",
            vec![
                token(TokenKind::Keyword(HardKeyword::Super), 0, 5),
                token(TokenKind::Punctuation(Punctuation::Dot), 5, 6),
                token(TokenKind::Identifier, 6, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::Select(Select {
            qualifier, name, ..
        }) = parser.ast().get(id).kind
        else {
            panic!("expected an ordinary super selection");
        };
        assert!(matches!(
            parser.ast().get(qualifier).kind,
            TreeKind::Super(Super { mix: None, .. })
        ));
        assert!(name.is_type());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_mixin_qualified_super_member_type_as_an_ordinary_selection() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "super[Base].member",
            vec![
                token(TokenKind::Keyword(HardKeyword::Super), 0, 5),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 5, 6),
                token(TokenKind::Identifier, 6, 10),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 10, 11),
                token(TokenKind::Punctuation(Punctuation::Dot), 11, 12),
                token(TokenKind::Identifier, 12, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::Select(Select { name, .. }) = parser.ast().get(id).kind else {
            panic!("expected an ordinary mixin-qualified super selection");
        };
        assert!(name.is_type());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_missing_this_singleton_type_suffix_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "this.",
            vec![
                token(TokenKind::Keyword(HardKeyword::This), 0, 4),
                token(TokenKind::Punctuation(Punctuation::Dot), 4, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        parser.type_expr();
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(matches!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedType
        ));
    }

    #[test]
    fn composes_a_path_singleton_type_inside_an_applied_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[x.type]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::Dot), 6, 7),
                token(TokenKind::Keyword(HardKeyword::Type), 7, 11),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::AppliedTypeTree(ref applied) = parser.ast().get(id).kind else {
            panic!("expected an applied type");
        };
        assert!(matches!(
            parser.ast().get(applied.args[0]).kind,
            TreeKind::SingletonTypeTree(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn composes_a_path_singleton_type_with_a_union() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "x.type | String",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::Dot), 1, 2),
                token(TokenKind::Keyword(HardKeyword::Type), 2, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Identifier, 9, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) = parser.ast().get(id).kind else {
            panic!("expected a union type");
        };
        assert!(matches!(
            parser.ast().get(infix.left).kind,
            TreeKind::SingletonTypeTree(_)
        ));
        assert_eq!(parser.names.resolve(infix.op.text()), "|");
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn composes_a_path_singleton_type_with_an_intersection() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "x.type & String",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::Dot), 1, 2),
                token(TokenKind::Keyword(HardKeyword::Type), 2, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Identifier, 9, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) = parser.ast().get(id).kind else {
            panic!("expected an intersection type");
        };
        assert!(matches!(
            parser.ast().get(infix.left).kind,
            TreeKind::SingletonTypeTree(_)
        ));
        assert_eq!(parser.names.resolve(infix.op.text()), "&");
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_missing_path_singleton_segment_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "x.",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::Dot), 1, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        parser.type_expr();
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(matches!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedType
        ));
    }

    #[test]
    fn parses_a_union_type_with_a_type_namespace_operator() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A | B",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(infix)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a type infix tree");
        };
        assert_eq!(parser.names.resolve(infix.op.text()), "|");
        assert!(infix.op.is_type());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_ordinary_function_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A => B",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(ref function)) =
            parser.ast().get(id).kind
        else {
            panic!("expected a function type");
        };
        assert_eq!(function.params.len(), 1);
        assert!(matches!(
            parser.ast().get(function.params[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            parser.ast().get(function.body).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 6).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_single_by_name_function_type_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(=> A) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Operator, 1, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(function)) = &parser.ast().get(id).kind
        else {
            panic!("expected an ordinary function type");
        };
        assert_eq!(function.params.len(), 1);
        let parameter = function.params[0];
        let TreeKind::ByNameTypeTree(by_name) = &parser.ast().get(parameter).kind else {
            panic!("expected a by-name type tree");
        };
        assert!(matches!(
            parser.ast().get(by_name.result).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(
            parser.ast().get(parameter).position.unwrap().span().range(),
            TextRange::new(1, 5).unwrap()
        );
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 11).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn preserves_strict_and_by_name_function_type_parameter_order() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A, => B, C) => D",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Operator, 4, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::Comma), 8, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Operator, 13, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(function)) = &parser.ast().get(id).kind
        else {
            panic!("expected an ordinary function type");
        };
        assert_eq!(function.params.len(), 3);
        assert!(matches!(
            parser.ast().get(function.params[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            parser.ast().get(function.params[1]).kind,
            TreeKind::ByNameTypeTree(_)
        ));
        assert!(matches!(
            parser.ast().get(function.params[2]).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_full_type_expressions_inside_by_name_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(=> A | B) => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Operator, 1, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Operator, 11, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(function)) = &parser.ast().get(id).kind
        else {
            panic!("expected an ordinary function type");
        };
        let TreeKind::ByNameTypeTree(by_name) = &parser.ast().get(function.params[0]).kind else {
            panic!("expected a by-name type tree");
        };
        assert!(matches!(
            parser.ast().get(by_name.result).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_tuple_type_inside_a_by_name_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(=> (A, B)) => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Operator, 1, 3),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 4, 5),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::Comma), 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Operator, 12, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(function)) = &parser.ast().get(id).kind
        else {
            panic!("expected an ordinary function type");
        };
        let TreeKind::ByNameTypeTree(by_name) = &parser.ast().get(function.params[0]).kind else {
            panic!("expected a by-name type tree");
        };
        assert!(matches!(
            parser.ast().get(by_name.result).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_context_function_type_inside_a_by_name_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(=> A ?=> B) => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Operator, 1, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Operator, 13, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(function)) = &parser.ast().get(id).kind
        else {
            panic!("expected an ordinary function type");
        };
        let TreeKind::ByNameTypeTree(by_name) = &parser.ast().get(function.params[0]).kind else {
            panic!("expected a by-name type tree");
        };
        assert!(matches!(
            parser.ast().get(by_name.result).kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn rejects_a_standalone_by_name_type_prefix() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(=> A)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Operator, 1, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_missing_by_name_parameter_type_before_the_closer() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A, =>) => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Operator, 4, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Operator, 8, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_unexpected_tokens_inside_by_name_parameter_lists() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(=> A B) => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Operator, 1, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::RightParen), 7, 8),
                token(TokenKind::Operator, 9, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_by_name_context_function_type_with_given_metadata() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(=> A) ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Operator, 1, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Operator, 7, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a context function type");
        };
        assert_eq!(function.params.len(), 1);
        assert!(matches!(
            parser.ast().get(function.params[0]).kind,
            TreeKind::ByNameTypeTree(_)
        ));
        assert_eq!(function.modifiers.modifiers, vec![Modifier::Given]);
        assert_eq!(function.erased_params, vec![false]);
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 12).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn preserves_mixed_by_name_context_parameter_order_and_arity() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A, => B, C) ?=> D",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Operator, 4, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::Comma), 8, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Operator, 13, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a context function type");
        };
        assert_eq!(function.params.len(), 3);
        assert!(matches!(
            parser.ast().get(function.params[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            parser.ast().get(function.params[1]).kind,
            TreeKind::ByNameTypeTree(_)
        ));
        assert!(matches!(
            parser.ast().get(function.params[2]).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(function.erased_params, vec![false, false, false]);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_multiple_by_name_context_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(=> A, => B) ?=> C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Operator, 1, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Operator, 13, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a context function type");
        };
        assert_eq!(function.params.len(), 2);
        assert!(
            function
                .params
                .iter()
                .all(|param| matches!(parser.ast().get(*param).kind, TreeKind::ByNameTypeTree(_)))
        );
        assert_eq!(function.erased_params, vec![false, false]);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn keeps_context_by_name_results_right_associative() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(=> A) ?=> B => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Operator, 1, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Operator, 7, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Operator, 13, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected the outer context function type");
        };
        assert!(matches!(
            parser.ast().get(function.result).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn keeps_ordinary_by_name_results_right_associative() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(=> A) => B ?=> C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Operator, 1, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Operator, 12, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(function)) = &parser.ast().get(id).kind
        else {
            panic!("expected the outer ordinary function type");
        };
        assert!(matches!(
            parser.ast().get(function.body).kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_missing_context_by_name_parameter_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A, =>) ?=> C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Operator, 4, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Operator, 8, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ));
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_unexpected_context_by_name_parameter_tokens() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(=> A B) ?=> C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Operator, 1, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::RightParen), 7, 8),
                token(TokenKind::Operator, 9, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ));
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_context_function_result_after_by_name_params() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(=> A) ?=>",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Operator, 1, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Operator, 7, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn does_not_turn_a_repeated_context_arrow_into_a_type_infix() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(=> A) ?=> ?=>",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Operator, 1, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Operator, 7, 10),
                token(TokenKind::Operator, 11, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ));
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn keeps_named_by_name_parameters_unsupported() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: => A) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Operator, 4, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
                token(TokenKind::Operator, 10, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_named_function_type_parameter_as_a_val_def() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(ref function)) =
            parser.ast().get(id).kind
        else {
            panic!("expected a function type");
        };
        assert_eq!(function.params.len(), 1);
        let parameter = function.params[0];
        let TreeKind::ValDef(parameter) = &parser.ast().get(parameter).kind else {
            panic!("expected a named parameter ValDef");
        };
        assert_eq!(parser.names.resolve(parameter.name.as_name().text()), "x");
        assert!(parameter.rhs.is_none());
        assert!(parameter.metadata == Default::default());
        assert!(matches!(
            parser.ast().get(parameter.tpt).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(
            parser
                .ast()
                .get(function.params[0])
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(1, 5).unwrap()
        );
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 11).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn preserves_multiple_named_function_type_parameters_in_order() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A, y: B) => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::Colon), 8, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Operator, 13, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(ref function)) =
            parser.ast().get(id).kind
        else {
            panic!("expected a function type");
        };
        let parameter_names: Vec<_> = function
            .params
            .iter()
            .map(|parameter| {
                let TreeKind::ValDef(definition) = &parser.ast().get(*parameter).kind else {
                    panic!("expected a named parameter ValDef");
                };
                assert!(definition.rhs.is_none());
                assert!(definition.metadata == Default::default());
                parser.names.resolve(definition.name.as_name().text())
            })
            .collect();
        assert_eq!(parameter_names, ["x", "y"]);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn preserves_backquoted_named_function_parameter_names() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(`type`: A) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::BackquotedIdentifier, 1, 7),
                token(TokenKind::Punctuation(Punctuation::Colon), 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Operator, 12, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(ref function)) =
            parser.ast().get(id).kind
        else {
            panic!("expected a function type");
        };
        let TreeKind::ValDef(parameter) = &parser.ast().get(function.params[0]).kind else {
            panic!("expected a named parameter ValDef");
        };
        assert_eq!(
            parser.names.resolve(parameter.name.as_name().text()),
            "type"
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn named_function_parameter_types_use_the_full_type_parser() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A | B, y: List[C]) => (D, E)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Comma), 9, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::Colon), 12, 13),
                token(TokenKind::Identifier, 14, 18),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 18, 19),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 20, 21),
                token(TokenKind::Punctuation(Punctuation::RightParen), 21, 22),
                token(TokenKind::Operator, 23, 25),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 26, 27),
                token(TokenKind::Identifier, 27, 28),
                token(TokenKind::Punctuation(Punctuation::Comma), 28, 29),
                token(TokenKind::Identifier, 30, 31),
                token(TokenKind::Punctuation(Punctuation::RightParen), 31, 32),
                token(TokenKind::Eof, 32, 32),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(ref function)) =
            parser.ast().get(id).kind
        else {
            panic!("expected a function type");
        };
        assert_eq!(function.params.len(), 2);
        let TreeKind::ValDef(first) = &parser.ast().get(function.params[0]).kind else {
            panic!("expected the first named parameter");
        };
        assert!(matches!(
            parser.ast().get(first.tpt).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(matches!(
            parser.ast().get(function.params[1]).kind,
            TreeKind::ValDef(_)
        ));
        let TreeKind::ValDef(second) = &parser.ast().get(function.params[1]).kind else {
            unreachable!();
        };
        assert!(matches!(
            parser.ast().get(second.tpt).kind,
            TreeKind::AppliedTypeTree(_)
        ));
        assert!(matches!(
            parser.ast().get(function.body).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn nested_named_function_types_are_parsed_recursively() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(f: (x: A) => B) => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 4, 5),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::Colon), 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Operator, 11, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Punctuation(Punctuation::RightParen), 15, 16),
                token(TokenKind::Operator, 17, 19),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(ref outer)) = parser.ast().get(id).kind
        else {
            panic!("expected the outer function type");
        };
        let TreeKind::ValDef(parameter) = &parser.ast().get(outer.params[0]).kind else {
            panic!("expected the outer named parameter");
        };
        assert!(matches!(
            parser.ast().get(parameter.tpt).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn named_function_parameter_types_can_use_context_function_types() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(f: A ?=> B) => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Operator, 13, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(ref outer)) = parser.ast().get(id).kind
        else {
            panic!("expected the outer ordinary function type");
        };
        let TreeKind::ValDef(parameter) = &parser.ast().get(outer.params[0]).kind else {
            panic!("expected the named parameter");
        };
        assert!(matches!(
            parser.ast().get(parameter.tpt).kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn named_function_type_results_remain_right_associative() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A) => B => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Operator, 12, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(ref function)) =
            parser.ast().get(id).kind
        else {
            panic!("expected a function type");
        };
        assert!(matches!(
            parser.ast().get(function.body).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn reports_a_trailing_comma_in_named_function_type_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A,) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Operator, 8, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_named_function_parameter_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x:) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Punctuation(Punctuation::RightParen), 3, 4),
                token(TokenKind::Operator, 5, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_type_after_a_later_named_function_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A, y:) => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::Colon), 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Operator, 11, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn rejects_named_function_parameters_without_a_colon() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "type F = (x A) => B",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
                token(TokenKind::Operator, 15, 17),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn parses_named_tuple_types_without_an_arrow() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "type T = (x: A, y: B)",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::Colon), 11, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Punctuation(Punctuation::Comma), 14, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Punctuation(Punctuation::Colon), 17, 18),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Punctuation(Punctuation::RightParen), 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn removes_the_parentheses_wrapper_from_a_single_function_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightParen), 2, 3),
                token(TokenKind::Operator, 4, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(ref function)) =
            parser.ast().get(id).kind
        else {
            panic!("expected a function type");
        };
        assert_eq!(function.params.len(), 1);
        assert!(matches!(
            parser.ast().get(function.params[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_zero_argument_function_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "() => R",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Punctuation(Punctuation::RightParen), 1, 2),
                token(TokenKind::Operator, 3, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(ref function)) =
            parser.ast().get(id).kind
        else {
            panic!("expected a function type");
        };
        assert!(function.params.is_empty());
        assert!(matches!(
            parser.ast().get(function.body).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn reports_a_missing_function_type_result() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "type F = A =>",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Operator, 11, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn reports_a_function_type_without_a_left_operand() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "type F = => B",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Operator, 9, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn reports_a_repeated_function_arrow() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "type F = A => => B",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Operator, 11, 13),
                token(TokenKind::Operator, 14, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn reports_a_missing_result_inside_function_type_arguments() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "type F = List[A =>]",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Identifier, 9, 13),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 13, 14),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Operator, 16, 18),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn parses_a_context_function_type_with_given_metadata() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A ?=> B",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a context function type");
        };
        assert_eq!(function.params.len(), 1);
        assert!(matches!(
            parser.ast().get(function.params[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            parser.ast().get(function.result).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(function.modifiers.modifiers, vec![Modifier::Given]);
        assert_eq!(function.erased_params, vec![false]);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_an_erased_named_function_parameter_when_enabled() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased x: A) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Colon), 9, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                token(TokenKind::Operator, 14, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected an ordinary function type with erased metadata");
        };
        assert_eq!(function.erased_params, vec![true]);
        assert!(function.modifiers.modifiers.is_empty());
        assert_eq!(function.params.len(), 1);
        let parameter_name = {
            let TreeKind::ValDef(parameter) = &parser.ast().get(function.params[0]).kind else {
                panic!("expected a named function parameter");
            };
            *parameter.name.as_name()
        };
        assert_eq!(parser.names.resolve(parameter_name.text()), "x");
        assert_eq!(
            parser
                .ast()
                .get(function.params[0])
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(8, 12).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_leading_unnamed_erased_function_parameter_when_enabled() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Operator, 11, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected an ordinary function type with erased metadata");
        };
        assert_eq!(function.erased_params, vec![true]);
        assert!(function.modifiers.modifiers.is_empty());
        assert!(matches!(
            parser.ast().get(function.params[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_trailing_unnamed_function_parameters_after_one_leading_erased_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A, B) => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Comma), 9, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                token(TokenKind::Operator, 14, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected an ordinary function type with erased metadata");
        };
        assert_eq!(function.erased_params, vec![true, false]);
        assert_eq!(function.params.len(), 2);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_leading_unnamed_erased_context_function_parameter_when_enabled() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A) ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Operator, 11, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a context function type with erased metadata");
        };
        assert_eq!(function.modifiers.modifiers, vec![Modifier::Given]);
        assert_eq!(function.erased_params, vec![true]);
        assert_eq!(function.params.len(), 1);
        assert!(matches!(
            parser.ast().get(function.params[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn does_not_activate_leading_unnamed_erased_context_parameters_when_disabled() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A) ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Operator, 11, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(!matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Identifier);
    }

    #[test]
    fn preserves_mixed_leading_unnamed_erased_context_parameter_flags() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A, B) ?=> C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Comma), 9, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                token(TokenKind::Operator, 14, 17),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a context function type with erased metadata");
        };
        assert_eq!(function.modifiers.modifiers, vec![Modifier::Given]);
        assert_eq!(function.erased_params, vec![true, false]);
        assert_eq!(function.params.len(), function.erased_params.len());
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn keeps_leading_unnamed_erased_context_function_results_right_associative() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A) ?=> B ?=> C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Operator, 11, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Operator, 17, 20),
                token(TokenKind::Identifier, 21, 22),
                token(TokenKind::Eof, 22, 22),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(outer)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected the outer context function type");
        };
        assert_eq!(outer.modifiers.modifiers, vec![Modifier::Given]);
        assert_eq!(outer.erased_params, vec![true]);
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(inner)) =
            &parser.ast().get(outer.result).kind
        else {
            panic!("expected the inner context function type");
        };
        assert_eq!(inner.modifiers.modifiers, vec![Modifier::Given]);
        assert_eq!(inner.erased_params, vec![false]);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn keeps_a_context_function_with_a_leading_unnamed_erased_parameter_and_ordinary_result_right_associative()
     {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A) ?=> B => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Operator, 11, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Operator, 17, 19),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(outer)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected the outer context function type");
        };
        assert_eq!(outer.modifiers.modifiers, vec![Modifier::Given]);
        assert_eq!(outer.erased_params, vec![true]);
        assert!(matches!(
            parser.ast().get(outer.result).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn keeps_an_ordinary_function_with_a_leading_unnamed_erased_parameter_and_context_result_right_associative()
     {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A) => B ?=> C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Operator, 11, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Operator, 16, 19),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(outer)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected the outer erased ordinary function type");
        };
        assert!(outer.modifiers.modifiers.is_empty());
        assert_eq!(outer.erased_params, vec![true]);
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(inner)) =
            &parser.ast().get(outer.result).kind
        else {
            panic!("expected the inner context function type");
        };
        assert_eq!(inner.modifiers.modifiers, vec![Modifier::Given]);
        assert_eq!(inner.erased_params, vec![false]);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_leading_unnamed_erased_context_parameter_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased) ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Punctuation(Punctuation::RightParen), 7, 8),
                token(TokenKind::Operator, 9, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let _ = parser.type_expr();
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedToken
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(7, 8).unwrap()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_trailing_comma_after_a_leading_unnamed_erased_context_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A,) ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Comma), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Operator, 12, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let _ = parser.type_expr();
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedType
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(10, 11).unwrap()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn reports_an_unterminated_leading_unnamed_erased_context_parameter_list() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Operator, 10, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Parens(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedToken
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(8, 9).unwrap()
        );
        assert_eq!(parser.current().span, TextRange::new(8, 9).unwrap());
    }

    #[test]
    fn recovers_a_missing_leading_unnamed_erased_context_function_result() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A) ?=>",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Operator, 11, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let _ = parser.type_expr();
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedType
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(14, 14).unwrap()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_repeated_context_arrow_after_a_leading_unnamed_erased_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A) ?=> ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Operator, 11, 14),
                token(TokenKind::Operator, 15, 18),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedType
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(15, 18).unwrap()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn leading_unnamed_erased_parameters_keep_full_type_and_result_parsing() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased List[A]) => A => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 12),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 12, 13),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 14, 15),
                token(TokenKind::Punctuation(Punctuation::RightParen), 15, 16),
                token(TokenKind::Operator, 17, 19),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Operator, 22, 24),
                token(TokenKind::Identifier, 25, 26),
                token(TokenKind::Eof, 26, 26),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(outer)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected an outer erased function type");
        };
        assert_eq!(outer.erased_params, vec![true]);
        assert!(matches!(
            parser.ast().get(outer.params[0]).kind,
            TreeKind::AppliedTypeTree(_)
        ));
        assert!(matches!(
            parser.ast().get(outer.result).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn does_not_activate_leading_unnamed_erased_parameters_when_disabled() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Operator, 11, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(!matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Identifier);
    }

    #[test]
    fn rejects_a_second_unnamed_erased_marker_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A, erased B) => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Comma), 9, 10),
                token(TokenKind::Identifier, 11, 17),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Punctuation(Punctuation::RightParen), 19, 20),
                token(TokenKind::Operator, 21, 23),
                token(TokenKind::Identifier, 24, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_trailing_comma_after_a_leading_unnamed_erased_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A,) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Comma), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Operator, 12, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_leading_unnamed_erased_parameter_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Punctuation(Punctuation::RightParen), 7, 8),
                token(TokenKind::Operator, 9, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_leading_unnamed_erased_function_result() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A) =>",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Operator, 11, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn reports_an_unterminated_leading_unnamed_erased_parameter_list_without_consuming_following_type_tokens()
     {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Operator, 10, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Parens(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedToken
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(8, 9).unwrap()
        );
        assert_eq!(parser.current().span, TextRange::new(8, 9).unwrap());
    }

    #[test]
    fn recovers_a_repeated_arrow_after_a_leading_unnamed_erased_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased A) => => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Operator, 11, 13),
                token(TokenKind::Operator, 14, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedType
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(14, 16).unwrap()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn preserves_mixed_erased_parameter_flags_in_source_order() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased x: A, y: B, erased z: C) => D",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Colon), 9, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::Comma), 12, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Punctuation(Punctuation::Colon), 15, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Punctuation(Punctuation::Comma), 18, 19),
                token(TokenKind::Identifier, 20, 26),
                token(TokenKind::Identifier, 27, 28),
                token(TokenKind::Punctuation(Punctuation::Colon), 28, 29),
                token(TokenKind::Identifier, 30, 31),
                token(TokenKind::Punctuation(Punctuation::RightParen), 31, 32),
                token(TokenKind::Operator, 33, 35),
                token(TokenKind::Identifier, 36, 37),
                token(TokenKind::Eof, 37, 37),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected an ordinary function type with erased metadata");
        };
        assert_eq!(function.erased_params, vec![true, false, true]);
        assert_eq!(function.params.len(), function.erased_params.len());
        assert!(function.modifiers.modifiers.is_empty());
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn erased_named_parameters_keep_full_type_expression_parsing() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased x: Option[A] | B) => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Colon), 9, 10),
                token(TokenKind::Identifier, 11, 17),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 17, 18),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 19, 20),
                token(TokenKind::Operator, 21, 22),
                token(TokenKind::Identifier, 23, 24),
                token(TokenKind::Punctuation(Punctuation::RightParen), 24, 25),
                token(TokenKind::Operator, 26, 28),
                token(TokenKind::Identifier, 29, 30),
                token(TokenKind::Eof, 30, 30),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected an ordinary function type with erased metadata");
        };
        assert_eq!(function.erased_params, vec![true]);
        let TreeKind::ValDef(parameter) = &parser.ast().get(function.params[0]).kind else {
            panic!("expected a named function parameter");
        };
        assert!(matches!(
            parser.ast().get(parameter.tpt).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn treats_erased_as_an_ordinary_parameter_name_before_a_colon() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased: A) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Punctuation(Punctuation::Colon), 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Operator, 12, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(function)) = &parser.ast().get(id).kind
        else {
            panic!("expected an ordinary function type");
        };
        let TreeKind::ValDef(parameter) = &parser.ast().get(function.params[0]).kind else {
            panic!("expected a named function parameter");
        };
        assert_eq!(
            parser.names.resolve(parameter.name.as_name().text()),
            "erased"
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn treats_erased_as_an_ordinary_later_parameter_name_before_a_colon() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A, erased: B) => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 13),
                token(TokenKind::Punctuation(Punctuation::Colon), 13, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Punctuation(Punctuation::RightParen), 16, 17),
                token(TokenKind::Operator, 18, 20),
                token(TokenKind::Identifier, 21, 22),
                token(TokenKind::Eof, 22, 22),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(function)) = &parser.ast().get(id).kind
        else {
            panic!("expected an ordinary function type");
        };
        assert_eq!(function.params.len(), 2);
        let names = function
            .params
            .iter()
            .map(|parameter| {
                let TreeKind::ValDef(parameter) = &parser.ast().get(*parameter).kind else {
                    panic!("expected a named function parameter");
                };
                parser
                    .names
                    .resolve(parameter.name.as_name().text())
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(names, ["x", "erased"]);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn keeps_backquoted_erased_as_a_regular_parameter_name() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(`erased`: A) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::BackquotedIdentifier, 1, 9),
                token(TokenKind::Punctuation(Punctuation::Colon), 9, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                token(TokenKind::Operator, 14, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(function)) = &parser.ast().get(id).kind
        else {
            panic!("expected an ordinary function type");
        };
        let TreeKind::ValDef(parameter) = &parser.ast().get(function.params[0]).kind else {
            panic!("expected a named function parameter");
        };
        assert_eq!(
            parser.names.resolve(parameter.name.as_name().text()),
            "erased"
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_erased_named_context_function_parameter_when_enabled() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased x: A) ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Colon), 9, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                token(TokenKind::Operator, 14, 17),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a context function type");
        };
        assert_eq!(function.modifiers.modifiers, vec![Modifier::Given]);
        assert_eq!(function.erased_params, vec![true]);
        let TreeKind::ValDef(parameter) = &parser.ast().get(function.params[0]).kind else {
            panic!("expected a named function parameter");
        };
        assert!(parameter.metadata.modifiers.is_empty());
        assert_eq!(
            parser
                .ast()
                .get(function.params[0])
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(8, 12).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn preserves_mixed_erased_context_parameter_flags_and_given_metadata() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A, erased y: B) ?=> C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Punctuation(Punctuation::Colon), 15, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Punctuation(Punctuation::RightParen), 18, 19),
                token(TokenKind::Operator, 20, 23),
                token(TokenKind::Identifier, 24, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a context function type");
        };
        assert_eq!(function.modifiers.modifiers, vec![Modifier::Given]);
        assert_eq!(function.erased_params, vec![false, true]);
        assert!(function.params.iter().all(|parameter| {
            matches!(
                &parser.ast().get(*parameter).kind,
                TreeKind::ValDef(value) if value.metadata.modifiers.is_empty()
            )
        }));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn keeps_erased_context_function_results_right_associative() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased x: A) ?=> B => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Colon), 9, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                token(TokenKind::Operator, 14, 17),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Operator, 20, 22),
                token(TokenKind::Identifier, 23, 24),
                token(TokenKind::Eof, 24, 24),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(outer)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected the outer context function type");
        };
        assert_eq!(outer.modifiers.modifiers, vec![Modifier::Given]);
        assert_eq!(outer.erased_params, vec![true]);
        let TreeKind::PhaseSpecific(UntypedNode::Function(inner)) =
            &parser.ast().get(outer.result).kind
        else {
            panic!("expected an ordinary function result");
        };
        assert_eq!(inner.params.len(), 1);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn keeps_mixed_context_and_ordinary_erased_results_right_associative() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased x: A) => B ?=> C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Colon), 9, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                token(TokenKind::Operator, 14, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Operator, 19, 22),
                token(TokenKind::Identifier, 23, 24),
                token(TokenKind::Eof, 24, 24),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(outer)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected the outer erased function type");
        };
        assert!(outer.modifiers.modifiers.is_empty());
        assert_eq!(outer.erased_params, vec![true]);
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(inner)) =
            &parser.ast().get(outer.result).kind
        else {
            panic!("expected a context function result");
        };
        assert_eq!(inner.modifiers.modifiers, vec![Modifier::Given]);
        assert_eq!(inner.erased_params, vec![false]);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_full_type_expressions_in_erased_context_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased x: A | B) ?=> C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Colon), 9, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Operator, 13, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Punctuation(Punctuation::RightParen), 16, 17),
                token(TokenKind::Operator, 18, 21),
                token(TokenKind::Identifier, 22, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a context function type");
        };
        let TreeKind::ValDef(parameter) = &parser.ast().get(function.params[0]).kind else {
            panic!("expected a named function parameter");
        };
        assert!(matches!(
            parser.ast().get(parameter.tpt).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert_eq!(function.erased_params, vec![true]);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn keeps_erased_as_an_ordinary_context_parameter_name_before_colon() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased: A) ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Punctuation(Punctuation::Colon), 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Operator, 12, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a context function type");
        };
        assert_eq!(function.erased_params, vec![false]);
        let TreeKind::ValDef(parameter) = &parser.ast().get(function.params[0]).kind else {
            panic!("expected a named function parameter");
        };
        assert_eq!(
            parser.names.resolve(parameter.name.as_name().text()),
            "erased"
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn does_not_parse_erased_context_parameters_when_feature_is_disabled() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased x: A) ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Colon), 9, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                token(TokenKind::Operator, 14, 17),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_trailing_comma_in_erased_context_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased x: A,) ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Colon), 9, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::Comma), 12, 13),
                token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
                token(TokenKind::Operator, 15, 18),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_repeated_context_function_arrows_after_erased_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased x: A) ?=> ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Colon), 9, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                token(TokenKind::Operator, 14, 17),
                token(TokenKind::Operator, 18, 21),
                token(TokenKind::Identifier, 22, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_erased_context_parameter_name_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased) ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Punctuation(Punctuation::RightParen), 7, 8),
                token(TokenKind::Operator, 9, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_erased_context_parameter_type_without_swallowing_the_arrow() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased x:) ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Colon), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Operator, 12, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ));
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_later_erased_context_parameter_name() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A, erased) ?=> C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 13),
                token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
                token(TokenKind::Operator, 15, 18),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_erased_context_function_result() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased x: A) ?=>",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Colon), 9, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                token(TokenKind::Operator, 14, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_erased_parameter_name_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Punctuation(Punctuation::RightParen), 7, 8),
                token(TokenKind::Operator, 9, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_erased_parameter_type_without_swallowing_the_arrow() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(erased x:) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Colon), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Operator, 12, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ));
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_erased_parameter_name_after_a_valid_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A, erased) => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 13),
                token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
                token(TokenKind::Operator, 15, 17),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_multiple_context_function_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A, B) ?=> C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Operator, 7, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a context function type");
        };
        assert_eq!(function.params.len(), 2);
        assert_eq!(function.erased_params, vec![false, false]);
        assert!(
            function
                .params
                .iter()
                .all(|param| matches!(parser.ast().get(*param).kind, TreeKind::Ident(_)))
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn strips_a_direct_parentheses_wrapper_from_a_context_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A) ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightParen), 2, 3),
                token(TokenKind::Operator, 4, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a context function type");
        };
        assert_eq!(function.params.len(), 1);
        assert!(matches!(
            parser.ast().get(function.params[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_nested_tuple_context_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "((A, B), C) ?=> D",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::Comma), 3, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Punctuation(Punctuation::Comma), 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Operator, 12, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a context function type");
        };
        assert_eq!(function.params.len(), 2);
        assert!(matches!(
            parser.ast().get(function.params[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
        assert!(matches!(
            parser.ast().get(function.params[1]).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn context_function_arrows_are_right_associative() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A ?=> B ?=> C",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Operator, 8, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(outer)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected the outer context function type");
        };
        assert!(matches!(
            parser.ast().get(outer.result).kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn ordinary_and_context_function_arrows_mix_recursively() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A => B ?=> C",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(outer)) = &parser.ast().get(id).kind
        else {
            panic!("expected the outer ordinary function type");
        };
        assert!(matches!(
            parser.ast().get(outer.body).kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn context_and_ordinary_function_arrows_mix_in_the_other_direction() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A ?=> B => C",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Operator, 8, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(outer)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected the outer context function type");
        };
        assert!(matches!(
            parser.ast().get(outer.result).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_an_empty_context_function_parameter_list() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "type F = () ?=> R",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Operator, 12, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn reports_a_context_function_type_without_a_left_operand() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "type F = ?=> B",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Operator, 9, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn reports_a_repeated_context_function_arrow() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "type F = A ?=> ?=>",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Operator, 11, 14),
                token(TokenKind::Operator, 15, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn reports_a_missing_context_function_result_at_eof() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "type F = (A, B) ?=>",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::Comma), 11, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Punctuation(Punctuation::RightParen), 14, 15),
                token(TokenKind::Operator, 16, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn recovers_a_missing_context_function_result_inside_type_arguments() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "type F = List[A ?=>]",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Identifier, 9, 13),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 13, 14),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Operator, 16, 19),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn reports_a_trailing_tuple_comma_before_a_context_arrow() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "type F = (A,) ?=> B",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::Comma), 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                token(TokenKind::Operator, 14, 17),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn parses_named_context_function_parameters_with_given_on_the_function_only() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A) ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Operator, 7, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a named context function type");
        };
        assert_eq!(function.params.len(), 1);
        assert_eq!(function.erased_params, vec![false]);
        assert_eq!(function.modifiers.modifiers, vec![Modifier::Given]);
        let TreeKind::ValDef(parameter) = &parser.ast().get(function.params[0]).kind else {
            panic!("expected a named context parameter ValDef");
        };
        assert_eq!(parser.names.resolve(parameter.name.as_name().text()), "x");
        assert!(parameter.rhs.is_none());
        assert!(parameter.metadata == Default::default());
        assert_eq!(
            parser
                .ast()
                .get(function.params[0])
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(1, 5).unwrap()
        );
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 12).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn preserves_multiple_named_context_function_parameters_and_erased_arity() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A, y: B) ?=> C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::Colon), 8, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Operator, 13, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a named context function type");
        };
        let parameter_names: Vec<_> = function
            .params
            .iter()
            .map(|parameter| {
                let TreeKind::ValDef(definition) = &parser.ast().get(*parameter).kind else {
                    panic!("expected a named context parameter ValDef");
                };
                assert!(definition.metadata == Default::default());
                parser.names.resolve(definition.name.as_name().text())
            })
            .collect();
        assert_eq!(parameter_names, ["x", "y"]);
        assert_eq!(function.erased_params, vec![false, false]);
        assert_eq!(function.modifiers.modifiers, vec![Modifier::Given]);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn named_context_parameter_types_use_the_full_type_parser() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: List[A], y: A | B) ?=> C & D",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 8),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 8, 9),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 10, 11),
                token(TokenKind::Punctuation(Punctuation::Comma), 11, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Punctuation(Punctuation::Colon), 14, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Operator, 18, 19),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Punctuation(Punctuation::RightParen), 21, 22),
                token(TokenKind::Operator, 23, 26),
                token(TokenKind::Identifier, 27, 28),
                token(TokenKind::Operator, 29, 30),
                token(TokenKind::Identifier, 31, 32),
                token(TokenKind::Eof, 32, 32),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a named context function type");
        };
        let TreeKind::ValDef(first) = &parser.ast().get(function.params[0]).kind else {
            panic!("expected the first named parameter");
        };
        let TreeKind::ValDef(second) = &parser.ast().get(function.params[1]).kind else {
            panic!("expected the second named parameter");
        };
        assert!(matches!(
            parser.ast().get(first.tpt).kind,
            TreeKind::AppliedTypeTree(_)
        ));
        assert!(matches!(
            parser.ast().get(second.tpt).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(matches!(
            parser.ast().get(function.result).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn named_context_parameter_types_can_nest_context_functions() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(f: A ?=> B) ?=> C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Operator, 13, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected the outer named context function type");
        };
        let TreeKind::ValDef(parameter) = &parser.ast().get(function.params[0]).kind else {
            panic!("expected the outer named parameter");
        };
        assert!(matches!(
            parser.ast().get(parameter.tpt).kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn named_context_function_results_remain_right_associative() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A) ?=> B => C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Operator, 7, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Operator, 13, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected the outer context function type");
        };
        assert!(matches!(
            parser.ast().get(function.result).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn named_ordinary_function_results_can_be_context_functions() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A) => B ?=> C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Operator, 12, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(function)) = &parser.ast().get(id).kind
        else {
            panic!("expected the outer ordinary function type");
        };
        assert!(matches!(
            parser.ast().get(function.body).kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_missing_named_context_parameter_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x:) ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Punctuation(Punctuation::RightParen), 3, 4),
                token(TokenKind::Operator, 5, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
        ));
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_trailing_comma_in_named_context_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A,) ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Operator, 8, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        );

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_later_named_context_parameter_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A, y:) ?=> C",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::Colon), 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Operator, 11, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_named_context_function_result() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A) ?=>",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Operator, 7, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_repeated_named_context_function_arrow() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A) ?=> ?=> B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Operator, 7, 10),
                token(TokenKind::Operator, 11, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let _ = parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn converts_a_direct_tuple_lhs_into_multiple_function_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A, B, C) => D",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
                token(TokenKind::Operator, 10, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(ref function)) =
            parser.ast().get(id).kind
        else {
            panic!("expected a function type");
        };
        assert_eq!(function.params.len(), 3);
        assert!(
            function
                .params
                .iter()
                .all(|param| matches!(parser.ast().get(*param).kind, TreeKind::Ident(_)))
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_nested_tuple_parameters_without_flattening() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "((A, B), C) => D",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::Comma), 3, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Punctuation(Punctuation::Comma), 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Operator, 12, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(ref function)) =
            parser.ast().get(id).kind
        else {
            panic!("expected a function type");
        };
        assert_eq!(function.params.len(), 2);
        assert!(matches!(
            parser.ast().get(function.params[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
        assert!(matches!(
            parser.ast().get(function.params[1]).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn function_type_results_are_right_associative() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A => B => C",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(ref outer)) = parser.ast().get(id).kind
        else {
            panic!("expected the outer function type");
        };
        assert!(matches!(
            parser.ast().get(outer.body).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn function_arrow_binds_below_union_and_intersection_types() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A | B => C & D",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Operator, 11, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(ref function)) =
            parser.ast().get(id).kind
        else {
            panic!("expected a function type");
        };
        assert!(matches!(
            parser.ast().get(function.params[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(matches!(
            parser.ast().get(function.body).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn intersection_binds_tighter_than_union() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A | B & C",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(outer)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected the outer union");
        };
        assert_eq!(parser.names.resolve(outer.op.text()), "|");
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(inner)) =
            &parser.ast().get(outer.right).kind
        else {
            panic!("expected the nested intersection");
        };
        assert_eq!(parser.names.resolve(inner.op.text()), "&");
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_type_annotation_as_an_annotated_tree() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A @Ann",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 3, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::Annotated(annotated) = &parser.ast().get(id).kind else {
            panic!("expected an annotated type");
        };
        assert!(matches!(
            parser.ast().get(annotated.expr).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            parser.ast().get(annotated.annotation).kind,
            TreeKind::Apply(_)
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_repeated_type_annotations() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A @Foo @Bar",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 3, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Identifier, 8, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::Annotated(outer) = &parser.ast().get(id).kind else {
            panic!("expected an outer annotated type");
        };
        assert!(matches!(
            parser.ast().get(outer.expr).kind,
            TreeKind::Annotated(_)
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn type_annotations_bind_before_union_and_intersection() {
        let mut names = NameInterner::new();
        let mut union_parser = parser_for(
            "A @Ann | B",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 3, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );
        let union = union_parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(union)) =
            &union_parser.ast().get(union).kind
        else {
            panic!("expected a union type");
        };
        assert!(matches!(
            union_parser.ast().get(union.left).kind,
            TreeKind::Annotated(_)
        ));
        assert!(union_parser.diagnostics().is_empty());

        let mut intersection_parser = parser_for(
            "A & B @Ann",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );
        let intersection = intersection_parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(intersection)) =
            &intersection_parser.ast().get(intersection).kind
        else {
            panic!("expected an intersection type");
        };
        assert!(matches!(
            intersection_parser.ast().get(intersection.right).kind,
            TreeKind::Annotated(_)
        ));
        assert!(intersection_parser.diagnostics().is_empty());
    }

    #[test]
    fn parenthesized_type_annotations_wrap_the_grouped_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A | B) @Ann",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Operator, 3, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Operator, 8, 9),
                token(TokenKind::Identifier, 9, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::Annotated(annotated) = &parser.ast().get(id).kind else {
            panic!("expected an annotated parenthesized type");
        };
        assert!(matches!(
            parser.ast().get(annotated.expr).kind,
            TreeKind::PhaseSpecific(UntypedNode::Parens(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_missing_type_annotation_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A @",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        parser.type_expr();
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn same_precedence_type_operators_are_left_associative() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A & B & C",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(outer)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected the outer intersection");
        };
        assert_eq!(parser.names.resolve(outer.op.text()), "&");
        assert!(matches!(
            parser.ast().get(outer.left).kind,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_contextual_word_as_a_type_operator_outside_context_bounds() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A as B",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Identifier, 2, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) = &parser.ast().get(id).kind
        else {
            panic!("expected an identifier infix type");
        };
        assert_eq!(parser.names.resolve(infix.op.text()), "as");
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 6).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn type_operators_follow_shared_precedence() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A * B + C",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(outer)) = &parser.ast().get(id).kind
        else {
            panic!("expected an outer infix type");
        };
        assert_eq!(parser.names.resolve(outer.op.text()), "+");
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(inner)) =
            &parser.ast().get(outer.left).kind
        else {
            panic!("expected a nested multiplicative type");
        };
        assert_eq!(parser.names.resolve(inner.op.text()), "*");
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn colon_terminated_type_operators_are_right_associative() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A :: B :: C",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(outer)) = &parser.ast().get(id).kind
        else {
            panic!("expected an outer right-associative type");
        };
        assert_eq!(parser.names.resolve(outer.op.text()), "::");
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(inner)) =
            &parser.ast().get(outer.right).kind
        else {
            panic!("expected a nested right-associative type");
        };
        assert_eq!(parser.names.resolve(inner.op.text()), "::");
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn diagnoses_mixed_type_operator_associativity() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A + B +: C",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        parser.type_expr();
        assert!(parser.diagnostics().iter().any(|diagnostic| {
            diagnostic
                .message()
                .contains("mixed left- and right-associative")
        }));
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn consumes_a_newline_after_an_identifier_type_operator() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A op\nB",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Identifier, 2, 4),
                token(TokenKind::Newline, 4, 5),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_infix_type_operand_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A +",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        parser.type_expr();
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_union_inside_type_arguments() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[A | B]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::AppliedTypeTree(applied) = &parser.ast().get(id).kind else {
            panic!("expected an applied type");
        };
        assert_eq!(applied.args.len(), 1);
        assert!(matches!(
            parser.ast().get(applied.args[0]).kind,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_nested_applied_types_with_an_intersection_argument() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "Option[List[A | B]]",
            vec![
                token(TokenKind::Identifier, 0, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 6, 7),
                token(TokenKind::Identifier, 7, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 11, 12),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Operator, 14, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 17, 18),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let outer_id = parser.type_expr();
        let TreeKind::AppliedTypeTree(outer) = &parser.ast().get(outer_id).kind else {
            panic!("expected the outer applied type");
        };
        let inner_id = outer.args[0];
        let TreeKind::AppliedTypeTree(inner) = &parser.ast().get(inner_id).kind else {
            panic!("expected the nested applied type");
        };
        assert!(matches!(
            parser.ast().get(inner.args[0]).kind,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn consumes_a_newline_after_a_type_operator_when_an_operand_follows() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A |\nB",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Newline, 3, 4),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        parser.type_expr();
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn reports_a_missing_type_operand_without_consuming_eof() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A |",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(matches!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedType
        ));
    }

    #[test]
    fn recovers_a_missing_type_operand_before_a_closing_bracket() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[A |]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        parser.type_expr();
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(matches!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedType
        ));
    }

    #[test]
    fn parses_general_symbolic_type_operators() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A || B",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) = &parser.ast().get(id).kind
        else {
            panic!("expected a generic infix type");
        };
        assert_eq!(parser.names.resolve(infix.op.text()), "||");
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_qualified_simple_type() {
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

        let id = parser.simple_type();
        let TreeKind::Select(selection) = parser.ast().get(id).kind else {
            panic!("expected qualified type");
        };

        assert!(selection.name.is_type());
        assert!(matches!(
            parser.ast().get(selection.qualifier).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_shared_type_argument_list_in_the_type_namespace() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A, B]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let args = parser.parse_type_argument_list(true);
        assert_eq!(args.len(), 2);
        assert!(args.iter().all(|id| matches!(
            parser.ast().get(*id).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        )));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_applied_simple_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[Int]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Identifier, 5, 8),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.simple_type();
        let TreeKind::AppliedTypeTree(applied) = &parser.ast().get(id).kind else {
            panic!("expected an applied type tree");
        };
        assert!(matches!(
            parser.ast().get(applied.tpt).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        ));
        assert_eq!(applied.args.len(), 1);
        assert!(matches!(
            parser.ast().get(applied.args[0]).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_type_projection_with_type_namespace_names() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "T#Member",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 1, 2),
                token(TokenKind::Identifier, 2, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let id = parser.simple_type();
        let TreeKind::Select(selection) = &parser.ast().get(id).kind else {
            panic!("expected a type projection");
        };
        assert!(selection.name.is_type());
        assert!(matches!(
            parser.ast().get(selection.qualifier).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        ));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 8).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn composes_type_projection_after_applied_type_arguments() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "F[A]#Result",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 3, 4),
                token(TokenKind::Operator, 4, 5),
                token(TokenKind::Identifier, 5, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.simple_type();
        let TreeKind::Select(selection) = &parser.ast().get(id).kind else {
            panic!("expected an applied type projection");
        };
        assert!(selection.name.is_type());
        let TreeKind::AppliedTypeTree(applied) = &parser.ast().get(selection.qualifier).kind else {
            panic!("expected an applied type as the projection qualifier");
        };
        assert_eq!(applied.args.len(), 1);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_a_missing_type_projection_member_without_consuming_eof() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "T#",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 1, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        let id = parser.simple_type();
        assert!(matches!(parser.ast().get(id).kind, TreeKind::Ident(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| matches!(diagnostic.kind(), ParseDiagnosticKind::ExpectedType))
        );
    }

    #[test]
    fn leaves_the_following_definition_boundary_after_a_missing_projection_member() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "T#\nNext",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 1, 2),
                token(TokenKind::Newline, 2, 3),
                token(TokenKind::Identifier, 3, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        parser.simple_type();
        assert_eq!(parser.current().kind, TokenKind::Newline);
        assert_eq!(parser.current_text().unwrap(), "\n");
        assert_eq!(parser.diagnostics().len(), 1);
    }

    #[test]
    fn parses_nested_and_repeated_applied_simple_types() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "F[A][B]",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 3, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let outer_id = parser.simple_type();
        let TreeKind::AppliedTypeTree(outer) = &parser.ast().get(outer_id).kind else {
            panic!("expected the outer applied type tree");
        };
        let inner_id = outer.tpt;
        let TreeKind::AppliedTypeTree(inner) = &parser.ast().get(inner_id).kind else {
            panic!("expected the inner applied type tree");
        };
        assert_eq!(inner.args.len(), 1);
        assert_eq!(outer.args.len(), 1);
        assert_eq!(
            parser.ast().get(outer_id).position.unwrap().span().range(),
            TextRange::new(0, 7).unwrap()
        );
        assert_eq!(
            parser.ast().get(inner_id).position.unwrap().span().range(),
            TextRange::new(0, 4).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn reports_an_empty_applied_type_argument_list() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let id = parser.simple_type();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::AppliedTypeTree(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(matches!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedType
        ));
    }

    #[test]
    fn recovers_a_missing_applied_type_argument_before_a_comma() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[, Int]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.simple_type();
        let TreeKind::AppliedTypeTree(applied) = &parser.ast().get(id).kind else {
            panic!("expected an applied type tree");
        };
        assert_eq!(applied.args.len(), 2);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
    }

    #[test]
    fn recovers_a_missing_applied_type_closer_at_eof() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[Int",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Identifier, 5, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        parser.simple_type();
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(matches!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedToken
        ));
    }

    #[test]
    fn recovers_repeated_commas_in_an_applied_type_argument_list() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[Int,, String]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Identifier, 5, 8),
                token(TokenKind::Punctuation(Punctuation::Comma), 8, 9),
                token(TokenKind::Punctuation(Punctuation::Comma), 9, 10),
                token(TokenKind::Identifier, 11, 17),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        parser.simple_type();
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
    }

    #[test]
    fn parses_a_polymorphic_function_type_with_a_function_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] => A => A",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Operator, 4, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Operator, 9, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::PolyFunction(poly)) = &parser.ast().get(id).kind
        else {
            panic!("expected a polymorphic function type");
        };
        assert_eq!(poly.type_params.len(), 1);
        assert!(matches!(
            parser.ast().get(poly.type_params[0]).kind,
            TreeKind::TypeDef(_)
        ));
        assert!(matches!(
            parser.ast().get(poly.body).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 13).unwrap()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_context_bounds_in_an_ordinary_polymorphic_function_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A: Show as show] => A => A",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 8),
                token(TokenKind::Identifier, 9, 11),
                token(TokenKind::Identifier, 12, 16),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 16, 17),
                token(TokenKind::Operator, 18, 20),
                token(TokenKind::Identifier, 21, 22),
                token(TokenKind::Operator, 23, 25),
                token(TokenKind::Identifier, 26, 27),
                token(TokenKind::Eof, 27, 27),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::PolyFunction(poly)) = &parser.ast().get(id).kind
        else {
            panic!("expected a polymorphic function type");
        };
        let TreeKind::TypeDef(parameter) = &parser.ast().get(poly.type_params[0]).kind else {
            panic!("expected a type parameter definition");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ContextBounds(ContextBounds {
            bounds,
            context_bounds,
        })) = &parser.ast().get(parameter.rhs).kind
        else {
            panic!("expected preserved context bounds");
        };
        assert!(matches!(
            parser.ast().get(*bounds).kind,
            TreeKind::TypeBoundsTree(_)
        ));
        assert_eq!(context_bounds.len(), 1);
        let TreeKind::PhaseSpecific(UntypedNode::ContextBoundTypeTree(ContextBoundTypeTree {
            name,
            ..
        })) = &parser.ast().get(context_bounds[0]).kind
        else {
            panic!("expected a context-bound type tree");
        };
        let alias = *name;
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
        drop(parser);
        assert_eq!(
            alias.map(|name| names.resolve(name.as_name().text())),
            Some("show")
        );
    }

    #[test]
    fn rejects_a_polymorphic_function_type_without_a_function_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] => A",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Operator, 4, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_type_lambda_with_an_applied_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] =>> F[A]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Operator, 4, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::LambdaTypeTree(LambdaTypeTree { type_params, body }) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a type lambda");
        };
        assert_eq!(type_params.len(), 1);
        assert!(matches!(
            parser.ast().get(type_params[0]).kind,
            TreeKind::TypeDef(TypeDef { .. })
        ));
        assert!(matches!(
            parser.ast().get(*body).kind,
            TreeKind::AppliedTypeTree(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_covariance_in_a_type_lambda_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[+A] =>> Producer[A]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Operator, 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 3, 4),
                token(TokenKind::Operator, 5, 8),
                token(TokenKind::Identifier, 9, 17),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 17, 18),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::LambdaTypeTree(LambdaTypeTree { type_params, .. }) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a type lambda");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(type_params[0]).kind else {
            panic!("expected a type parameter definition");
        };
        assert_eq!(
            definition.variance,
            Some(dotty_core::types::Variance::Covariant)
        );
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn rejects_contravariance_in_a_type_lambda_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[-A] =>> Consumer[A]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Operator, 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 3, 4),
                token(TokenKind::Operator, 5, 8),
                token(TokenKind::Identifier, 9, 17),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 17, 18),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::LambdaTypeTree(LambdaTypeTree { type_params, .. }) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a type lambda");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(type_params[0]).kind else {
            panic!("expected a type parameter definition");
        };
        assert_eq!(
            definition.variance,
            Some(dotty_core::types::Variance::Contravariant)
        );
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_nested_type_lambda_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] =>> [B] =>> Either[A, B]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Operator, 4, 7),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 8, 9),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 10, 11),
                token(TokenKind::Operator, 12, 15),
                token(TokenKind::Identifier, 16, 22),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 22, 23),
                token(TokenKind::Identifier, 23, 24),
                token(TokenKind::Punctuation(Punctuation::Comma), 24, 25),
                token(TokenKind::Identifier, 26, 27),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 27, 28),
                token(TokenKind::Eof, 28, 28),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::LambdaTypeTree(LambdaTypeTree { body, .. }) = &parser.ast().get(id).kind
        else {
            panic!("expected an outer type lambda");
        };
        assert!(matches!(
            parser.ast().get(*body).kind,
            TreeKind::LambdaTypeTree(_)
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn reports_and_strips_a_context_bound_in_a_type_lambda() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A: Show] =>> X",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 8),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 8, 9),
                token(TokenKind::Operator, 10, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::LambdaTypeTree(LambdaTypeTree { type_params, .. }) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a type lambda");
        };
        let parameter = parser.ast().get(type_params[0]);
        assert_eq!(
            parameter.position.unwrap().span().range(),
            TextRange::new(4, 4).unwrap()
        );
        let TreeKind::TypeDef(definition) = &parameter.kind else {
            panic!("expected a type parameter definition");
        };
        assert!(definition.metadata.modifiers.is_empty());
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(4, 8).unwrap()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn reports_and_strips_a_context_bound_after_an_upper_bound() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A <: Base: Show] =>> A",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Operator, 3, 5),
                token(TokenKind::Identifier, 6, 10),
                token(TokenKind::ColonFollow, 10, 11),
                token(TokenKind::Identifier, 12, 16),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 16, 17),
                token(TokenKind::Operator, 18, 21),
                token(TokenKind::Identifier, 22, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::LambdaTypeTree(LambdaTypeTree { type_params, .. }) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a type lambda");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(type_params[0]).kind else {
            panic!("expected a type parameter definition");
        };
        assert!(matches!(
            parser.ast().get(definition.rhs).kind,
            TreeKind::TypeBoundsTree(_)
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn reports_and_strips_multiple_braced_context_bounds() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A: {Show, Ord}] =>> A",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 4, 5),
                token(TokenKind::Identifier, 5, 9),
                token(TokenKind::Punctuation(Punctuation::Comma), 9, 10),
                token(TokenKind::Identifier, 11, 14),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 14, 15),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 15, 16),
                token(TokenKind::Operator, 17, 20),
                token(TokenKind::Identifier, 21, 22),
                token(TokenKind::Eof, 22, 22),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::LambdaTypeTree(_)
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn preserves_type_parameter_order_in_a_type_lambda() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A, B] =>> Either[A, B]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 5, 6),
                token(TokenKind::Operator, 7, 10),
                token(TokenKind::Identifier, 11, 17),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 17, 18),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Punctuation(Punctuation::Comma), 19, 20),
                token(TokenKind::Identifier, 21, 22),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 22, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::LambdaTypeTree(LambdaTypeTree { type_params, .. }) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a type lambda");
        };
        let names = type_params
            .iter()
            .map(|param| {
                let TreeKind::TypeDef(definition) = &parser.ast().get(*param).kind else {
                    panic!("expected a type parameter definition");
                };
                parser.names.resolve(definition.name.as_name().text())
            })
            .collect::<Vec<_>>();
        assert_eq!(names, ["A", "B"]);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_type_lambda_with_an_ordinary_function_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] =>> A => A",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Operator, 4, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Operator, 10, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::LambdaTypeTree(LambdaTypeTree { body, .. }) = &parser.ast().get(id).kind
        else {
            panic!("expected a type lambda");
        };
        assert!(matches!(
            parser.ast().get(*body).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_type_lambda_with_a_context_function_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] =>> A ?=> A",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Operator, 4, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Operator, 10, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::LambdaTypeTree(LambdaTypeTree { body, .. }) = &parser.ast().get(id).kind
        else {
            panic!("expected a type lambda");
        };
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(*body).kind
        else {
            panic!("expected a context function body");
        };
        assert_eq!(function.modifiers.modifiers, vec![Modifier::Given]);
        assert_eq!(function.erased_params, vec![false]);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_an_empty_type_lambda_parameter_clause() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[] =>> X",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 1, 2),
                token(TokenKind::Operator, 3, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_leading_type_lambda_parameter_comma() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[, A] =>> X",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Punctuation(Punctuation::Comma), 1, 2),
                token(TokenKind::Identifier, 3, 4),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 4, 5),
                token(TokenKind::Operator, 6, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::LambdaTypeTree(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
    }

    #[test]
    fn recovers_a_missing_type_lambda_parameter_closer() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A =>> X",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Operator, 3, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::LambdaTypeTree(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_missing_type_lambda_body_at_eof() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] =>>",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Operator, 4, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::LambdaTypeTree(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_missing_nested_type_lambda_parameter_closer() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] =>> [B =>> X",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Operator, 4, 7),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 8, 9),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Operator, 11, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::LambdaTypeTree(LambdaTypeTree { body, .. }) = &parser.ast().get(id).kind
        else {
            panic!("expected an outer type lambda");
        };
        assert!(matches!(
            parser.ast().get(*body).kind,
            TreeKind::LambdaTypeTree(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
    }

    #[test]
    fn recovers_a_missing_context_bound_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A:] =>> X",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 3, 4),
                token(TokenKind::Operator, 5, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::LambdaTypeTree(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 2);
    }

    #[test]
    fn preserves_a_context_function_body_inside_a_polymorphic_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] => A ?=> A",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Operator, 4, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Operator, 9, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::PolyFunction(poly)) = &parser.ast().get(id).kind
        else {
            panic!("expected a polymorphic function type");
        };
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(poly.body).kind
        else {
            panic!("expected a context function body");
        };
        assert_eq!(function.modifiers.modifiers, vec![Modifier::Given]);
        assert_eq!(function.erased_params, vec![false]);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_a_by_name_function_body_inside_a_polymorphic_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] => (=> A) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Operator, 4, 6),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 7, 8),
                token(TokenKind::Operator, 8, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                token(TokenKind::Operator, 14, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::PolyFunction(poly)) = &parser.ast().get(id).kind
        else {
            panic!("expected a polymorphic function type");
        };
        let TreeKind::PhaseSpecific(UntypedNode::Function(function)) =
            &parser.ast().get(poly.body).kind
        else {
            panic!("expected an ordinary function body");
        };
        assert!(matches!(
            parser.ast().get(function.params[0]).kind,
            TreeKind::ByNameTypeTree(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_an_erased_function_body_inside_a_polymorphic_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] => (erased x: A) => B",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Operator, 4, 6),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 7, 8),
                token(TokenKind::Identifier, 8, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Punctuation(Punctuation::Colon), 16, 17),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Punctuation(Punctuation::RightParen), 19, 20),
                token(TokenKind::Operator, 21, 23),
                token(TokenKind::Identifier, 24, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        });

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::PolyFunction(poly)) = &parser.ast().get(id).kind
        else {
            panic!("expected a polymorphic function type");
        };
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) =
            &parser.ast().get(poly.body).kind
        else {
            panic!("expected an erased function body");
        };
        assert!(function.modifiers.modifiers.is_empty());
        assert_eq!(function.erased_params, vec![true]);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_missing_polymorphic_function_body_at_eof() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] =>",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Operator, 4, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_missing_polymorphic_type_parameter_closer_before_the_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A => A => A",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Operator, 3, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Operator, 8, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::PolyFunction(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_an_empty_polymorphic_type_parameter_clause_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[] => A => A",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 1, 2),
                token(TokenKind::Operator, 3, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Operator, 8, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn does_not_consume_a_context_arrow_as_a_polymorphic_type_arrow() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] ?=> A => A",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Operator, 4, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Operator, 10, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(parser.current_text(), Ok("?=>"));
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_a_directly_nested_polymorphic_function_type_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] => [B] => B => A",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Operator, 4, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 7, 8),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 9, 10),
                token(TokenKind::Operator, 11, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Operator, 16, 18),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_abstract_type_member_refinement() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A { type X }",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 2, 3),
                token(TokenKind::Keyword(HardKeyword::Type), 4, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::RefinedTypeTree(RefinedTypeTree { tpt, refinements }) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a refined type");
        };
        assert!(matches!(parser.ast().get(*tpt).kind, TreeKind::Ident(_)));
        assert_eq!(refinements.len(), 1);
        let TreeKind::TypeDef(TypeDef { rhs, .. }) = parser.ast().get(refinements[0]).kind else {
            panic!("expected a type member");
        };
        assert!(matches!(
            parser.ast().get(rhs).kind,
            TreeKind::TypeBoundsTree(_)
        ));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 12).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_type_alias_member_refinement() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A { type X = String }",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 2, 3),
                token(TokenKind::Keyword(HardKeyword::Type), 4, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Operator, 11, 12),
                token(TokenKind::Identifier, 13, 19),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::RefinedTypeTree(RefinedTypeTree { refinements, .. }) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a refined type");
        };
        let TreeKind::TypeDef(TypeDef { rhs, .. }) = parser.ast().get(refinements[0]).kind else {
            panic!("expected a type member");
        };
        assert!(matches!(parser.ast().get(rhs).kind, TreeKind::Ident(_)));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_the_source_order_of_multiple_type_member_refinements() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A { type X; type Y = Int }",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 2, 3),
                token(TokenKind::Keyword(HardKeyword::Type), 4, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::Semicolon), 10, 11),
                token(TokenKind::Keyword(HardKeyword::Type), 12, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Operator, 19, 20),
                token(TokenKind::Identifier, 21, 24),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 25, 26),
                token(TokenKind::Eof, 26, 26),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::RefinedTypeTree(RefinedTypeTree { refinements, .. }) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a refined type");
        };
        assert_eq!(refinements.len(), 2);
        let TreeKind::TypeDef(first) = &parser.ast().get(refinements[0]).kind else {
            panic!("expected the first type member");
        };
        let TreeKind::TypeDef(second) = &parser.ast().get(refinements[1]).kind else {
            panic!("expected the second type member");
        };
        assert_eq!(parser.names.resolve(first.name.as_name().text()), "X");
        assert_eq!(parser.names.resolve(second.name.as_name().text()), "Y");
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_parentless_refinement_with_a_synthetic_parent() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{ type X }",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Keyword(HardKeyword::Type), 2, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::RefinedTypeTree(RefinedTypeTree { tpt, refinements }) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a parentless refined type");
        };
        assert!(matches!(parser.ast().get(*tpt).kind, TreeKind::TypeTree(_)));
        assert_eq!(
            parser.ast().get(*tpt).position.unwrap().span().range(),
            TextRange::new(0, 0).unwrap()
        );
        assert_eq!(refinements.len(), 1);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_missing_refinement_closer_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A { type X",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 2, 3),
                token(TokenKind::Keyword(HardKeyword::Type), 4, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::RefinedTypeTree(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn leaves_an_enclosing_outdent_after_a_missing_refinement_closer() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A { type X",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 2, 3),
                token(TokenKind::Keyword(HardKeyword::Type), 4, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Outdent, 10, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::RefinedTypeTree(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Outdent);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_val_declaration_member_in_a_refinement() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A { val x: Int }",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 2, 3),
                token(TokenKind::Keyword(HardKeyword::Val), 4, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::ColonFollow, 9, 10),
                token(TokenKind::Identifier, 11, 14),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::RefinedTypeTree(refined) = &parser.ast().get(id).kind else {
            panic!("expected a refined type");
        };
        let TreeKind::ValDef(ValDef { rhs, .. }) = &parser.ast().get(refined.refinements[0]).kind
        else {
            panic!("expected a val declaration");
        };
        assert!(rhs.is_none());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_var_declaration_member_in_a_refinement() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A { var y: String }",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 2, 3),
                token(TokenKind::Keyword(HardKeyword::Var), 4, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::ColonFollow, 9, 10),
                token(TokenKind::Identifier, 11, 17),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::RefinedTypeTree(refined) = &parser.ast().get(id).kind else {
            panic!("expected a refined type");
        };
        let TreeKind::ValDef(definition) = &parser.ast().get(refined.refinements[0]).kind else {
            panic!("expected a var declaration");
        };
        assert!(
            definition
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::Var)
        );
        assert!(definition.rhs.is_none());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_def_declaration_member_in_a_refinement() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A { def value: Result }",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 2, 3),
                token(TokenKind::Keyword(HardKeyword::Def), 4, 7),
                token(TokenKind::Identifier, 8, 13),
                token(TokenKind::ColonFollow, 13, 14),
                token(TokenKind::Identifier, 15, 21),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 22, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::RefinedTypeTree(refined) = &parser.ast().get(id).kind else {
            panic!("expected a refined type");
        };
        let TreeKind::DefDef(DefDef { rhs, .. }) = &parser.ast().get(refined.refinements[0]).kind
        else {
            panic!("expected a def declaration");
        };
        assert!(rhs.is_none());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn diagnoses_a_rhs_and_preserves_a_following_type_member() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A { val x: Int = 1; type Y = String }",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 2, 3),
                token(TokenKind::Keyword(HardKeyword::Val), 4, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::ColonFollow, 9, 10),
                token(TokenKind::Identifier, 11, 14),
                token(TokenKind::Operator, 15, 16),
                token(TokenKind::IntegerLiteral, 17, 18),
                token(TokenKind::Punctuation(Punctuation::Semicolon), 18, 19),
                token(TokenKind::Keyword(HardKeyword::Type), 20, 24),
                token(TokenKind::Identifier, 25, 26),
                token(TokenKind::Operator, 27, 28),
                token(TokenKind::Identifier, 29, 35),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 36, 37),
                token(TokenKind::Eof, 37, 37),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::RefinedTypeTree(refined) = &parser.ast().get(id).kind else {
            panic!("expected a refined type");
        };
        assert_eq!(refined.refinements.len(), 2);
        assert!(matches!(
            parser.ast().get(refined.refinements[1]).kind,
            TreeKind::TypeDef(_)
        ));
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn diagnoses_a_default_argument_in_a_refinement_method() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A { def f(x: Int = 1): String }",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 2, 3),
                token(TokenKind::Keyword(HardKeyword::Def), 4, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::ColonFollow, 11, 12),
                token(TokenKind::Identifier, 13, 16),
                token(TokenKind::Operator, 17, 18),
                token(TokenKind::IntegerLiteral, 19, 20),
                token(TokenKind::Punctuation(Punctuation::RightParen), 20, 21),
                token(TokenKind::ColonFollow, 21, 22),
                token(TokenKind::Identifier, 23, 29),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 30, 31),
                token(TokenKind::Eof, 31, 31),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::RefinedTypeTree(_)
        ));
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_a_class_member_without_swallowing_a_following_type_member() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A { class C; type Y }",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 2, 3),
                token(TokenKind::Keyword(HardKeyword::Class), 4, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::Semicolon), 11, 12),
                token(TokenKind::Keyword(HardKeyword::Type), 13, 17),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::RefinedTypeTree(refined) = &parser.ast().get(id).kind else {
            panic!("expected a refined type");
        };
        assert_eq!(refined.refinements.len(), 1);
        assert!(matches!(
            parser.ast().get(refined.refinements[0]).kind,
            TreeKind::TypeDef(_)
        ));
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_a_modifier_without_swallowing_a_following_type_member() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A { private val x: Int; type Y }",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 2, 3),
                token(TokenKind::Keyword(HardKeyword::Private), 4, 11),
                token(TokenKind::Keyword(HardKeyword::Val), 12, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::ColonFollow, 17, 18),
                token(TokenKind::Identifier, 19, 22),
                token(TokenKind::Punctuation(Punctuation::Semicolon), 22, 23),
                token(TokenKind::Keyword(HardKeyword::Type), 24, 28),
                token(TokenKind::Identifier, 29, 30),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 31, 32),
                token(TokenKind::Eof, 32, 32),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::RefinedTypeTree(refined) = &parser.ast().get(id).kind else {
            panic!("expected a refined type");
        };
        assert_eq!(refined.refinements.len(), 1);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_malformed_refinement_method_at_the_closing_brace() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A { def f( }",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 2, 3),
                token(TokenKind::Keyword(HardKeyword::Def), 4, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::RefinedTypeTree(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }
}
