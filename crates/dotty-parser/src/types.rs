use dotty_core::ast::{
    ByNameTypeTree, Function, FunctionWithMods, Modifier, Modifiers, Parens, Tuple, UntypedNode,
    ValDef,
};
use dotty_core::{Name, Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::references::{QualifiedReferenceError, ReferenceNamespace};
use crate::{ParseDiagnosticKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses a bracketed, comma-separated list of simple type arguments.
    ///
    /// The caller owns the tree that precedes the list; this helper is shared
    /// by term-level type applications and applied type trees so that their
    /// delimiter and recovery behavior stays identical.
    pub(crate) fn parse_type_argument_list(&mut self) -> Vec<TreeId<Untyped>> {
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
            args.push(self.type_expr());
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
    /// higher-level entries own function-arrow and union/intersection
    /// precedence.
    pub(crate) fn type_expr(&mut self) -> TreeId<Untyped> {
        self.parse_function_type()
    }

    fn parse_function_type(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
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

        if let Some(arrow) = self.named_function_type_arrow() {
            let allow_erased =
                matches!(arrow, FunctionTypeArrow::Ordinary) && self.features().erased_definitions;
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
        let parameter = self.parse_union_type();
        if self.diagnostics.len() != diagnostics_before {
            return parameter;
        }

        let arrow = if self.current_is_arrow() {
            self.advance();
            FunctionTypeArrow::Ordinary
        } else if self.current_is_context_arrow() {
            self.advance();
            FunctionTypeArrow::Context
        } else {
            return parameter;
        };
        let body = self.type_expr();
        self.recover_missing_function_results();
        let params = self.function_type_params(parameter);
        let erased_params = vec![false; params.len()];
        self.alloc_function_type(mark, params, body, arrow, erased_params)
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
    /// followed by one of the two supported function arrows; named tuple
    /// types remain on the existing recovery path for now.
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
        if !matches!(
            first_name.kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) || !is_function_param_colon(first_colon.kind)
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

    pub(crate) fn parse_union_type(&mut self) -> TreeId<Untyped> {
        let mut tree = self.parse_intersection_type();
        while let Some(operator) = self.accept_type_infix_operator("|") {
            self.consume_type_infix_newlines();
            let right = self.parse_intersection_type();
            tree = self.alloc_infix(tree, operator, right);
        }
        tree
    }

    fn parse_intersection_type(&mut self) -> TreeId<Untyped> {
        let mut tree = self.parse_type_operand();
        while let Some(operator) = self.accept_type_infix_operator("&") {
            self.consume_type_infix_newlines();
            let right = self.parse_type_operand();
            tree = self.alloc_infix(tree, operator, right);
        }
        tree
    }

    fn parse_type_operand(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
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
        if !matches!(
            self.cursor.lookahead(offset).kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) {
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

    fn accept_type_infix_operator(&mut self, expected: &str) -> Option<Name> {
        if !matches!(
            self.current().kind,
            TokenKind::Operator | TokenKind::ColonOp
        ) || !self.current_text_is(expected)
        {
            return None;
        }

        let operator = *self.intern_current_type_name().ok()?.as_name();
        self.advance();
        Some(operator)
    }

    /// Parses the small simple-type subset needed by simple expressions.
    pub(crate) fn simple_type(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let mut tree = self.simple_type_reference();

        while self
            .cursor
            .at(TokenKind::Punctuation(Punctuation::LeftBracket))
        {
            let args = self.parse_type_argument_list();
            tree = self.alloc_from(
                mark,
                dotty_core::TreeKind::AppliedTypeTree(dotty_core::ast::AppliedTypeTree {
                    tpt: tree,
                    args,
                }),
            );
        }
        tree
    }

    /// Parses only `id { '.' id }`, for grammar positions whose suffixes have
    /// a distinct meaning (for example the current `derives` production).
    pub(crate) fn simple_type_reference(&mut self) -> TreeId<Untyped> {
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
    use dotty_core::{HardKeyword, NameInterner, Punctuation, TextRange, TreeKind};

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
    fn keeps_named_tuple_types_deferred_without_an_arrow() {
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
        assert!(!result.diagnostics.is_empty());
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
    fn leaves_unimplemented_type_operators_unconsumed() {
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

        parser.type_expr();
        assert_eq!(parser.current().kind, TokenKind::Operator);
        assert_eq!(parser.current_text().unwrap(), "||");
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

        let args = parser.parse_type_argument_list();
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
}
