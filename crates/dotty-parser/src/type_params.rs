use dotty_core::ast::{
    ContextBoundTypeTree, ContextBounds, LambdaTypeTree, Modifiers, TypeBoundsTree, TypeDef,
    UntypedNode,
};
use dotty_core::types::Variance;
use dotty_core::{
    Punctuation, SourceSpan, Span, TextRange, TokenKind, TreeId, TreeKind, TypeName, Untyped,
};

use crate::{ParamOwner, ParseIssue, ParseKind, Parser, TypeParamIssue};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses the initial reusable subset of a Scala type-parameter clause.
    ///
    /// The owner is kept explicit even though the first consumer is a
    /// polymorphic function literal. Future definition parsers can reuse this
    /// entry point without introducing a second square-bracket grammar.
    pub(crate) fn parse_type_param_clause(&mut self, owner: ParamOwner) -> Vec<TreeId<Untyped>> {
        self.with_param_owner(Some(owner), |parser| parser.type_param_clause())
    }

    /// Removes context-bound wrappers after a `=>>` arrow has established a
    /// type lambda. Ordinary polymorphic function types retain these wrappers
    /// in their source tree.
    pub(crate) fn strip_type_lambda_context_bounds(&mut self, params: &[TreeId<Untyped>]) {
        for &param in params {
            let (bounds, first_context_bound, context_bounds_span) = match &self.ast.get(param).kind
            {
                TreeKind::TypeDef(definition) => match &self.ast.get(definition.rhs).kind {
                    TreeKind::PhaseSpecific(UntypedNode::ContextBounds(context_bounds)) => (
                        Some(context_bounds.bounds),
                        context_bounds.context_bounds.first().copied(),
                        self.ast.get(definition.rhs).position,
                    ),
                    _ => (None, None, None),
                },
                _ => (None, None, None),
            };
            let Some(bounds) = bounds else {
                continue;
            };

            self.report_issue_at(
                context_bounds_span.unwrap_or_else(|| self.current_span()),
                ParseIssue::TypeParameter(
                    TypeParamIssue::ContextBoundsNotAllowedForPolyFunctionParameter,
                ),
            );

            if let TreeKind::TypeDef(definition) = &mut self.ast.get_mut(param).kind {
                definition.rhs = bounds;
                definition
                    .metadata
                    .modifiers
                    .retain(|modifier| !matches!(modifier, dotty_core::ast::Modifier::Param));
            }

            let start = first_context_bound
                .and_then(|bound| {
                    self.ast
                        .get(bound)
                        .position
                        .map(|position| position.span().range().start())
                })
                .or_else(|| {
                    self.ast
                        .get(param)
                        .position
                        .map(|position| position.span().range().end())
                });
            if let Some(start) = start {
                let range = TextRange::new(start, start).expect("zero-width context bound span");
                self.ast.get_mut(param).position =
                    Some(SourceSpan::new(self.source_id, Span::without_point(range)));
            }
        }
    }

    fn type_param_clause(&mut self) -> Vec<TreeId<Untyped>> {
        self.expect(TokenKind::Punctuation(Punctuation::LeftBracket));
        let mut params = Vec::new();

        if self.current().kind == TokenKind::Punctuation(Punctuation::RightBracket) {
            self.report_issue(ParseIssue::TypeParameter(
                TypeParamIssue::EmptyParameterClause {
                    found: self.current().kind,
                },
            ));
        }

        while self.current().kind != TokenKind::Eof
            && self.current().kind != TokenKind::Punctuation(Punctuation::RightBracket)
            && !self.type_param_recovery_definition_boundary()
            && !(self.context.enum_body
                && matches!(
                    self.current().kind,
                    TokenKind::Newline
                        | TokenKind::Newlines
                        | TokenKind::Indent
                        | TokenKind::Outdent
                        | TokenKind::Keyword(dotty_core::HardKeyword::Case)
                        | TokenKind::Punctuation(Punctuation::RightBrace)
                ))
        {
            if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                self.report_issue(ParseIssue::TypeParameter(
                    TypeParamIssue::ExpectedParameterAfterComma {
                        found: self.current().kind,
                    },
                ));
                continue;
            }

            let checkpoint = self.cursor.checkpoint();
            params.push(self.type_param());
            if !self.cursor.progressed_since(checkpoint) {
                self.report_issue(ParseIssue::TypeParameter(
                    TypeParamIssue::ParameterNoProgress {
                        found: self.current().kind,
                    },
                ));
                break;
            }

            if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                if self.current().kind == TokenKind::Punctuation(Punctuation::RightBracket) {
                    self.report_issue(ParseIssue::TypeParameter(
                        TypeParamIssue::ExpectedParameterAfterComma {
                            found: self.current().kind,
                        },
                    ));
                }
                continue;
            }
            if self.current().kind == TokenKind::Punctuation(Punctuation::RightBracket)
                || self.current().kind == TokenKind::Eof
                || self.current_is_arrow()
                || self.current_is_type_lambda_arrow()
            {
                break;
            }

            if self.type_param_recovery_definition_boundary() {
                self.report_issue(ParseIssue::TypeParameter(
                    TypeParamIssue::ExpectedParameterSeparator {
                        found: self.current().kind,
                    },
                ));
                break;
            }

            self.report_issue(ParseIssue::TypeParameter(
                TypeParamIssue::ExpectedParameterSeparator {
                    found: self.current().kind,
                },
            ));
            self.recover_type_param_clause();
            if self.current_is_arrow() || self.current_is_type_lambda_arrow() {
                break;
            }
        }

        self.expect(TokenKind::Punctuation(Punctuation::RightBracket));
        params
    }

    fn type_param_recovery_definition_boundary(&mut self) -> bool {
        let mut offset = 0;
        while matches!(
            self.cursor.lookahead(offset).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            offset += 1;
        }
        if offset > 0 && is_recovery_definition_keyword(self.cursor.lookahead(offset).kind) {
            return true;
        }
        (crate::modifiers::is_hard_modifier(self.current().kind) && self.starts_definition_prefix())
            || is_recovery_definition_keyword(self.current().kind)
    }

    fn type_param(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let mut metadata = Modifiers::default();
        while self.current().kind == TokenKind::Operator && self.current_text_is("@") {
            metadata.annotations.push(self.parse_annotation());
        }
        let is_synthetic_wildcard_name = self.current().kind == TokenKind::BackquotedIdentifier
            && self
                .current_text()
                .ok()
                .is_some_and(|text| text.trim_matches('`').starts_with("$type_wildcard_"));
        if !is_synthetic_wildcard_name {
            metadata.modifiers.push(dotty_core::ast::Modifier::Param);
            if matches!(
                self.context.param_owner,
                Some(ParamOwner::Class | ParamOwner::CaseClass)
            ) {
                metadata
                    .modifiers
                    .push(dotty_core::ast::Modifier::PrivateLocal);
            }
        }
        let variance = if self.current().kind == TokenKind::Operator {
            match self.current_text().ok() {
                Some("+") => {
                    self.advance();
                    Some(Variance::Covariant)
                }
                Some("-") => {
                    self.advance();
                    Some(Variance::Contravariant)
                }
                _ => None,
            }
        } else {
            None
        };

        let name = self.parse_type_param_name();
        if variance.is_some() && self.context.param_owner == Some(ParamOwner::Type) {
            self.report_issue(ParseIssue::TypeParameter(
                TypeParamIssue::VarianceNotAllowedForPolyFunctionParameter,
            ));
        }

        let nested_params = if self
            .cursor
            .at(TokenKind::Punctuation(Punctuation::LeftBracket))
        {
            Some(self.parse_type_param_clause(ParamOwner::Hk))
        } else {
            None
        };

        let explicit_bounds_start = self.current().span.start();
        let low = if self.accept_operator(">:") {
            Some(self.parse_bound_type())
        } else {
            None
        };
        let high = if self.accept_operator("<:") {
            Some(self.parse_bound_type())
        } else {
            None
        };
        let has_explicit_bounds = low.is_some() || high.is_some();
        let empty_bounds_start = nested_params
            .as_ref()
            .and_then(|params| {
                params.last().and_then(|param| {
                    self.ast
                        .get(*param)
                        .position
                        .map(|position| position.span().range().end())
                })
            })
            .unwrap_or(mark.start);
        let bounds = if has_explicit_bounds {
            self.alloc_from(
                crate::Mark {
                    start: explicit_bounds_start,
                },
                TreeKind::TypeBoundsTree(TypeBoundsTree {
                    low,
                    high,
                    alias: None,
                }),
            )
        } else {
            self.synthetic_type_bounds(empty_bounds_start)
        };
        let context_bounds = self.parse_context_bounds(name);
        let has_context_bounds = context_bounds.is_some();
        if !has_explicit_bounds
            && let Some(start) = context_bounds.as_ref().and_then(|bounds| {
                bounds.first().and_then(|bound| {
                    self.ast
                        .get(*bound)
                        .position
                        .map(|position| position.span().range().start())
                })
            })
        {
            let range = TextRange::new(start, start).expect("zero-width context bound span");
            self.ast.get_mut(bounds).position =
                Some(SourceSpan::new(self.source_id, Span::without_point(range)));
        }
        let rhs = if let Some(context_bounds) =
            context_bounds.filter(|_| context_bounds_are_allowed(self.context.param_owner))
        {
            self.alloc_from(
                crate::Mark {
                    start: self
                        .ast
                        .get(bounds)
                        .position
                        .map(|position| position.span().range().start())
                        .unwrap_or(mark.start),
                },
                TreeKind::PhaseSpecific(UntypedNode::ContextBounds(ContextBounds {
                    bounds,
                    context_bounds,
                })),
            )
        } else {
            if has_context_bounds {
                self.report_issue(ParseIssue::TypeParameter(
                    TypeParamIssue::ContextBoundsNotAllowedForOwner {
                        owner: self.context.param_owner,
                    },
                ));
            }
            bounds
        };
        let rhs = if let Some(nested_params) = nested_params {
            let lambda_start = nested_params
                .first()
                .and_then(|param| {
                    self.ast
                        .get(*param)
                        .position
                        .map(|position| position.span().range().start())
                })
                .unwrap_or(mark.start);
            let lambda = self.alloc_from(
                crate::Mark {
                    start: lambda_start,
                },
                TreeKind::LambdaTypeTree(LambdaTypeTree {
                    type_params: nested_params,
                    body: rhs,
                }),
            );
            if !has_explicit_bounds {
                let range = TextRange::new(lambda_start, empty_bounds_start)
                    .expect("higher-kinded lambda span is ordered");
                self.ast.get_mut(lambda).position =
                    Some(SourceSpan::new(self.source_id, Span::without_point(range)));
            }
            lambda
        } else {
            rhs
        };
        self.alloc_from(
            mark,
            TreeKind::TypeDef(TypeDef {
                name,
                rhs,
                metadata,
                variance,
            }),
        )
    }

    fn parse_type_param_name(&mut self) -> TypeName {
        if self.current().kind == TokenKind::Identifier && self.current_text_is("_") {
            self.advance();
            return self.fresh_wildcard_type_name();
        }

        match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                match self.intern_current_type_name() {
                    Ok(name) => {
                        self.advance();
                        name
                    }
                    Err(_) => self.missing_type_param_name(),
                }
            }
            _ => {
                self.report_issue(ParseIssue::TypeParameter(
                    TypeParamIssue::ExpectedParameterName {
                        found: self.current().kind,
                    },
                ));
                self.missing_type_param_name()
            }
        }
    }

    fn missing_type_param_name(&mut self) -> TypeName {
        if !matches!(
            self.current().kind,
            TokenKind::Eof | TokenKind::Punctuation(Punctuation::Comma | Punctuation::RightBracket)
        ) && !(self.context.enum_body
            && matches!(
                self.current().kind,
                TokenKind::Newline
                    | TokenKind::Newlines
                    | TokenKind::Indent
                    | TokenKind::Outdent
                    | TokenKind::Keyword(dotty_core::HardKeyword::Case)
                    | TokenKind::Punctuation(Punctuation::RightBrace)
            ))
            && !self.current_is_arrow()
            && !self.current_is_type_lambda_arrow()
        {
            self.advance();
        }
        self.fresh_wildcard_type_name()
    }

    fn parse_bound_type(&mut self) -> TreeId<Untyped> {
        self.with_parse_kind(ParseKind::Type, |parser| parser.type_expr())
    }

    fn accept_operator(&mut self, expected: &str) -> bool {
        if self.current().kind == TokenKind::Operator && self.current_text_is(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn accept_context_bound_colon(&mut self) -> bool {
        let is_context_bound_colon = matches!(
            self.current().kind,
            TokenKind::Punctuation(Punctuation::Colon)
                | TokenKind::ColonFollow
                | TokenKind::ColonOp
        ) && self.current_text_is(":");
        if is_context_bound_colon {
            self.advance();
        }
        is_context_bound_colon
    }

    fn parse_context_bounds(&mut self, parameter: TypeName) -> Option<Vec<TreeId<Untyped>>> {
        let mut context_bounds = Vec::new();
        while self.accept_context_bound_colon() {
            if self.accept(TokenKind::Punctuation(Punctuation::LeftBrace)) {
                if self.accept(TokenKind::Punctuation(Punctuation::RightBrace)) {
                    self.report_issue(ParseIssue::TypeParameter(
                        TypeParamIssue::EmptyBracedContextBoundList {
                            found: self.current().kind,
                        },
                    ));
                } else {
                    loop {
                        if self.current().kind == TokenKind::Punctuation(Punctuation::Comma) {
                            self.report_issue(ParseIssue::TypeParameter(
                                TypeParamIssue::ExpectedContextBoundAtListStart {
                                    found: self.current().kind,
                                },
                            ));
                            self.advance();
                            continue;
                        }
                        let checkpoint = self.cursor.checkpoint();
                        context_bounds.push(self.parse_context_bound_type(parameter));
                        if !self.cursor.progressed_since(checkpoint) {
                            self.report_issue(ParseIssue::TypeParameter(
                                TypeParamIssue::ContextBoundNoProgress {
                                    found: self.current().kind,
                                },
                            ));
                            self.recover_context_bound_list();
                        }
                        if self.current().kind != TokenKind::Punctuation(Punctuation::Comma) {
                            break;
                        }
                        self.advance();
                        if self.current().kind == TokenKind::Punctuation(Punctuation::RightBrace) {
                            self.report_issue(ParseIssue::TypeParameter(
                                TypeParamIssue::ExpectedContextBoundAfterComma {
                                    found: self.current().kind,
                                },
                            ));
                            break;
                        }
                    }
                    self.expect(TokenKind::Punctuation(Punctuation::RightBrace));
                }
            } else {
                context_bounds.push(self.parse_context_bound_type(parameter));
            }
        }
        (!context_bounds.is_empty()).then_some(context_bounds)
    }

    fn recover_context_bound_list(&mut self) {
        while !matches!(
            self.current().kind,
            TokenKind::Eof | TokenKind::Punctuation(Punctuation::Comma | Punctuation::RightBrace)
        ) {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn parse_context_bound_type(&mut self, parameter: TypeName) -> TreeId<Untyped> {
        let mark = self.mark();
        let bound = if self.context_bound_type_is_missing() {
            self.report_issue(ParseIssue::TypeParameter(
                TypeParamIssue::ExpectedContextBoundType {
                    found: self.current().kind,
                },
            ));
            self.error_type(self.span_from(mark))
        } else {
            self.with_parse_kind(ParseKind::Type, |parser| {
                parser.parse_context_bound_type_expr()
            })
        };
        let name = self.parse_context_bound_alias();
        self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::ContextBoundTypeTree(ContextBoundTypeTree {
                bound,
                parameter,
                name,
            })),
        )
    }

    fn parse_context_bound_alias(&mut self) -> Option<dotty_core::names::TermName> {
        if !(self.current().kind == TokenKind::Identifier && self.current_text_is("as")) {
            return None;
        }
        self.advance();
        match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                let name = self.intern_current_term_name().ok();
                self.advance();
                name
            }
            _ => {
                self.report_issue(ParseIssue::TypeParameter(
                    TypeParamIssue::ExpectedContextBoundAlias {
                        found: self.current().kind,
                    },
                ));
                None
            }
        }
    }

    fn context_bound_type_is_missing(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Punctuation(Punctuation::Comma | Punctuation::RightBracket) | TokenKind::Eof
        ) || self.current_is_arrow()
            || self.current_is_type_lambda_arrow()
    }

    fn recover_type_param_clause(&mut self) {
        while !matches!(
            self.current().kind,
            TokenKind::Eof | TokenKind::Punctuation(Punctuation::Comma | Punctuation::RightBracket)
        ) && !(self.context.enum_body
            && matches!(
                self.current().kind,
                TokenKind::Newline
                    | TokenKind::Newlines
                    | TokenKind::Indent
                    | TokenKind::Outdent
                    | TokenKind::Keyword(dotty_core::HardKeyword::Case)
                    | TokenKind::Punctuation(Punctuation::RightBrace)
            ))
            && !self.current_is_arrow()
            && !self.current_is_type_lambda_arrow()
        {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn fresh_wildcard_type_name(&mut self) -> TypeName {
        let index = self.next_wildcard_type_param;
        self.next_wildcard_type_param = self.next_wildcard_type_param.saturating_add(1);
        let name = format!("$type_wildcard_{index}");
        TypeName::new(self.names.intern(&name))
    }

    pub(crate) fn synthetic_type_bounds(&mut self, start: u32) -> TreeId<Untyped> {
        let range = TextRange::new(start, start).expect("zero-width type bounds range");
        self.alloc(
            TreeKind::TypeBoundsTree(TypeBoundsTree {
                low: None,
                high: None,
                alias: None,
            }),
            Some(SourceSpan::new(self.source_id, Span::without_point(range))),
        )
    }
}

fn context_bounds_are_allowed(owner: Option<ParamOwner>) -> bool {
    matches!(
        owner,
        Some(
            ParamOwner::Class
                | ParamOwner::CaseClass
                | ParamOwner::Def
                | ParamOwner::Type
                | ParamOwner::Given
                | ParamOwner::ExtensionPrefix
                | ParamOwner::ExtensionFollow
        )
    )
}

pub(crate) fn is_recovery_definition_keyword(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Keyword(
            dotty_core::HardKeyword::Given
                | dotty_core::HardKeyword::Val
                | dotty_core::HardKeyword::Var
                | dotty_core::HardKeyword::Def
                | dotty_core::HardKeyword::Type
                | dotty_core::HardKeyword::Enum
                | dotty_core::HardKeyword::Class
                | dotty_core::HardKeyword::Trait
                | dotty_core::HardKeyword::Object
                | dotty_core::HardKeyword::Package
                | dotty_core::HardKeyword::Import
                | dotty_core::HardKeyword::Export
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use crate::{ParseDiagnosticKind, ParseIssue};
    use dotty_core::ast::{
        ContextBoundTypeTree, ContextBounds, TypeBoundsTree, TypeDef, UntypedNode,
    };
    use dotty_core::{NameInterner, TextRange, Token, TokenValue};

    #[test]
    fn parses_an_unbounded_type_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        assert_eq!(params.len(), 1);
        let TreeKind::TypeDef(TypeDef { name, rhs, .. }) = parser.ast().get(params[0]).kind else {
            panic!("expected a type definition");
        };
        assert!(name.as_name().is_type());
        let TreeKind::TypeBoundsTree(TypeBoundsTree { low, high, alias }) =
            parser.ast().get(rhs).kind
        else {
            panic!("expected type bounds");
        };
        assert!(low.is_none() && high.is_none() && alias.is_none());
        assert_eq!(
            parser.ast().get(rhs).position.unwrap().span().range(),
            TextRange::new(1, 1).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_context_bound_on_a_method_type_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A: Show]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 8),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Def);

        let TreeKind::TypeDef(TypeDef { name, rhs, .. }) = &parser.ast().get(params[0]).kind else {
            panic!("expected a type parameter");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ContextBounds(ContextBounds {
            bounds,
            context_bounds,
        })) = &parser.ast().get(*rhs).kind
        else {
            panic!("expected context bounds");
        };
        assert_eq!(context_bounds.len(), 1);
        assert!(matches!(
            parser.ast().get(*bounds).kind,
            TreeKind::TypeBoundsTree(TypeBoundsTree {
                low: None,
                high: None,
                alias: None
            })
        ));
        let TreeKind::PhaseSpecific(UntypedNode::ContextBoundTypeTree(ContextBoundTypeTree {
            parameter,
            ..
        })) = &parser.ast().get(context_bounds[0]).kind
        else {
            panic!("expected a context-bound type tree");
        };
        assert_eq!(*parameter, *name);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_context_bounds_for_the_polyfunction_type_owner_in_scala_39() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A: Show]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 8),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);

        assert!(matches!(
            parser.ast().get(params[0]).kind,
            TreeKind::TypeDef(TypeDef { rhs, .. })
                if matches!(parser.ast().get(rhs).kind, TreeKind::PhaseSpecific(UntypedNode::ContextBounds(_)))
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_braced_multiple_context_bounds_in_source_order() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A: {Ord, Show}]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 4, 5),
                token(TokenKind::Identifier, 5, 8),
                token(TokenKind::Punctuation(Punctuation::Comma), 8, 9),
                token(TokenKind::Identifier, 10, 14),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 14, 15),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Class);

        let TreeKind::TypeDef(TypeDef { rhs, .. }) = &parser.ast().get(params[0]).kind else {
            panic!("expected a type parameter");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ContextBounds(ContextBounds {
            context_bounds,
            ..
        })) = &parser.ast().get(*rhs).kind
        else {
            panic!("expected context bounds");
        };
        let bound_names = context_bounds
            .iter()
            .map(|bound| match &parser.ast().get(*bound).kind {
                TreeKind::PhaseSpecific(UntypedNode::ContextBoundTypeTree(context_bound)) => {
                    match &parser.ast().get(context_bound.bound).kind {
                        TreeKind::Ident(ident) => parser.names.resolve(ident.name.text()),
                        _ => panic!("expected a type identifier"),
                    }
                }
                _ => panic!("expected a context-bound type tree"),
            })
            .collect::<Vec<_>>();
        assert_eq!(bound_names, ["Ord", "Show"]);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_from_a_missing_braced_context_bound_and_keeps_the_next_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A: {, Show}, B]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 11),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 11, 12),
                token(TokenKind::Punctuation(Punctuation::Comma), 12, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Def);
        let parsed_names = params
            .iter()
            .map(|param| match &parser.ast().get(*param).kind {
                TreeKind::TypeDef(definition) => {
                    parser.names.resolve(definition.name.as_name().text())
                }
                _ => panic!("expected a type parameter"),
            })
            .collect::<Vec<_>>();

        assert_eq!(parsed_names, ["A", "B"]);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].issue(),
            &ParseIssue::TypeParameter(TypeParamIssue::ExpectedContextBoundAtListStart {
                found: TokenKind::Punctuation(Punctuation::Comma),
            })
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(5, 6).unwrap()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn reports_an_empty_braced_context_bound_list_structurally() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A: {}]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        parser.parse_type_param_clause(ParamOwner::Def);

        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].issue(),
            &ParseIssue::TypeParameter(TypeParamIssue::EmptyBracedContextBoundList {
                found: TokenKind::Punctuation(Punctuation::RightBracket),
            })
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(6, 7).unwrap()
        );
    }

    #[test]
    fn reports_a_missing_context_bound_alias_structurally() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A: Show as]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 8),
                token(TokenKind::Identifier, 9, 11),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        parser.parse_type_param_clause(ParamOwner::Def);

        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].issue(),
            &ParseIssue::TypeParameter(TypeParamIssue::ExpectedContextBoundAlias {
                found: TokenKind::Punctuation(Punctuation::RightBracket),
            })
        );
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedExpression
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(11, 12).unwrap()
        );
    }

    #[test]
    fn reports_context_bounds_stripped_from_a_polyfunction_structurally() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A: Show]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 8),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );
        let params = parser.parse_type_param_clause(ParamOwner::Type);

        parser.strip_type_lambda_context_bounds(&params);

        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].issue(),
            &ParseIssue::TypeParameter(
                TypeParamIssue::ContextBoundsNotAllowedForPolyFunctionParameter,
            )
        );
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::UnsupportedSyntax
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(4, 8).unwrap()
        );
    }

    #[test]
    fn rejects_context_bounds_for_higher_kinded_parameter_owner() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A: Show]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 8),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Hk);

        assert!(matches!(
            parser.ast().get(params[0]).kind,
            TreeKind::TypeDef(TypeDef { rhs, .. })
                if matches!(parser.ast().get(rhs).kind, TreeKind::TypeBoundsTree(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].issue(),
            &ParseIssue::TypeParameter(TypeParamIssue::ContextBoundsNotAllowedForOwner {
                owner: Some(ParamOwner::Hk),
            })
        );
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::UnsupportedSyntax
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(8, 9).unwrap()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn preserves_declared_type_parameter_variance() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[+A, -B]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(1, 2).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::Comma), 3, 4),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(5, 6).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Class);
        let variances = params
            .iter()
            .map(|param| match parser.ast().get(*param).kind {
                TreeKind::TypeDef(TypeDef { variance, .. }) => variance,
                _ => panic!("expected a type definition"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            variances,
            vec![Some(Variance::Covariant), Some(Variance::Contravariant)]
        );
        let TreeKind::TypeDef(TypeDef { ref metadata, .. }) = parser.ast().get(params[0]).kind
        else {
            panic!("expected a type definition");
        };
        assert_eq!(
            metadata.modifiers,
            vec![
                dotty_core::ast::Modifier::Param,
                dotty_core::ast::Modifier::PrivateLocal,
            ]
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_a_context_bound_until_the_arrow_kind_is_known() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A: Show]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Colon), 2, 3),
                token(TokenKind::Identifier, 4, 8),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        let TreeKind::TypeDef(TypeDef { rhs, .. }) = parser.ast().get(params[0]).kind else {
            panic!("expected a type parameter definition");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ContextBounds(ContextBounds {
            bounds,
            context_bounds,
        })) = &parser.ast().get(rhs).kind
        else {
            panic!("expected context bounds");
        };
        assert!(matches!(
            parser.ast().get(*bounds).kind,
            TreeKind::TypeBoundsTree(TypeBoundsTree {
                low: None,
                high: None,
                alias: None
            })
        ));
        assert_eq!(context_bounds.len(), 1);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn reports_a_trailing_type_lambda_parameter_comma() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A,]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 3, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let _ = parser.parse_type_param_clause(ParamOwner::Type);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].issue(),
            &ParseIssue::TypeParameter(TypeParamIssue::ExpectedParameterAfterComma {
                found: TokenKind::Punctuation(Punctuation::RightBracket),
            })
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(3, 4).unwrap()
        );
    }

    #[test]
    fn polymorphic_type_parameters_do_not_get_private_local_role() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        let TreeKind::TypeDef(TypeDef { ref metadata, .. }) = parser.ast().get(params[0]).kind
        else {
            panic!("expected a type definition");
        };
        assert_eq!(metadata.modifiers, vec![dotty_core::ast::Modifier::Param]);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn wildcard_type_parameters_get_parameter_role() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[_]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        let TreeKind::TypeDef(TypeDef { ref metadata, .. }) = parser.ast().get(params[0]).kind
        else {
            panic!("expected a type definition");
        };
        assert_eq!(metadata.modifiers, vec![dotty_core::ast::Modifier::Param]);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn wildcard_type_parameters_with_bounds_get_parameter_role() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[_ <: Foo]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(3, 5).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 6, 9),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        let TreeKind::TypeDef(TypeDef { ref metadata, .. }) = parser.ast().get(params[0]).kind
        else {
            panic!("expected a type definition");
        };
        assert_eq!(metadata.modifiers, vec![dotty_core::ast::Modifier::Param]);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_dotty_wildcard_name_collision_without_parameter_roles() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[`$type_wildcard_0`]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::BackquotedIdentifier, 1, 19),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        let TreeKind::TypeDef(TypeDef { ref metadata, .. }) = parser.ast().get(params[0]).kind
        else {
            panic!("expected a type definition");
        };
        assert!(metadata.modifiers.is_empty());
        assert!(metadata.visibility.is_none());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn class_wildcard_name_collision_has_no_parameter_roles() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[`$type_wildcard_0`]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::BackquotedIdentifier, 1, 19),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Class);
        let TreeKind::TypeDef(TypeDef { ref metadata, .. }) = parser.ast().get(params[0]).kind
        else {
            panic!("expected a type definition");
        };
        assert!(metadata.modifiers.is_empty());
        assert!(metadata.visibility.is_none());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn malformed_variance_token_text_reports_instead_of_panicking() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Operator, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Class);

        assert_eq!(params.len(), 1);
        assert_eq!(parser.diagnostics().len(), 1);
        let diagnostic = &parser.diagnostics()[0];
        assert_eq!(diagnostic.kind(), ParseDiagnosticKind::ExpectedType);
        assert_eq!(
            diagnostic.issue(),
            &ParseIssue::TypeParameter(TypeParamIssue::ExpectedParameterName {
                found: TokenKind::Operator,
            })
        );
        assert_eq!(diagnostic.span(), TextRange::new(1, 2).unwrap());
    }

    #[test]
    fn parses_multiple_type_parameters_in_source_order() {
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

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        assert_eq!(params.len(), 2);
        let parameter_names = params
            .iter()
            .map(|param| match parser.ast().get(*param).kind {
                TreeKind::TypeDef(TypeDef { name, .. }) => name,
                _ => panic!("expected a type definition"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            parser.names.resolve(parameter_names[0].as_name().text()),
            "A"
        );
        assert_eq!(
            parser.names.resolve(parameter_names[1].as_name().text()),
            "B"
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_simple_lower_and_upper_type_bounds() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A >: Lower <: Upper]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(3, 5).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 6, 11),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(12, 14).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 15, 20),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        let TreeKind::TypeDef(TypeDef { rhs, .. }) = parser.ast().get(params[0]).kind else {
            panic!("expected a type definition");
        };
        let TreeKind::TypeBoundsTree(TypeBoundsTree { low, high, .. }) = parser.ast().get(rhs).kind
        else {
            panic!("expected type bounds");
        };
        assert!(low.is_some() && high.is_some());
        assert_eq!(
            parser.ast().get(rhs).position.unwrap().span().range(),
            TextRange::new(3, 20).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn context_bounds_span_includes_explicit_lower_and_upper_bounds() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A >: Low <: High: Show]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(3, 5).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 6, 9),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(10, 12).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 13, 17),
                token(TokenKind::ColonFollow, 17, 18),
                token(TokenKind::Identifier, 19, 23),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 23, 24),
                token(TokenKind::Eof, 24, 24),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Def);
        let TreeKind::TypeDef(TypeDef { rhs, .. }) = parser.ast().get(params[0]).kind else {
            panic!("expected a type parameter");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ContextBounds(ref context_bounds)) =
            parser.ast().get(rhs).kind
        else {
            panic!("expected context bounds");
        };

        assert_eq!(
            parser.ast().get(rhs).position.unwrap().span().range(),
            TextRange::new(3, 23).unwrap()
        );
        assert_eq!(
            parser
                .ast()
                .get(context_bounds.bounds)
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(3, 17).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_intersection_type_in_a_bound_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A <: Foo & Bar]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(3, 5).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 6, 9),
                token(TokenKind::Operator, 10, 11),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);

        assert_eq!(params.len(), 1);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        let TreeKind::TypeDef(TypeDef { rhs, .. }) = parser.ast().get(params[0]).kind else {
            panic!("expected a type definition");
        };
        let TreeKind::TypeBoundsTree(TypeBoundsTree { high, .. }) = parser.ast().get(rhs).kind
        else {
            panic!("expected type bounds");
        };
        assert!(matches!(
            parser
                .ast()
                .get(high.expect("expected an upper bound"))
                .kind,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn malformed_type_parameter_clause_recovers_at_the_closing_bracket() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[,A]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Punctuation(Punctuation::Comma), 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 3, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        assert_eq!(params.len(), 1);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].issue(),
            &ParseIssue::TypeParameter(TypeParamIssue::ExpectedParameterAfterComma {
                found: TokenKind::Identifier,
            })
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(2, 3).unwrap()
        );
    }

    #[test]
    fn stops_a_type_parameter_clause_before_a_type_lambda_arrow() {
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

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        assert_eq!(params.len(), 1);
        assert_eq!(parser.current_text(), Ok("=>>"));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_higher_kinded_type_parameter_clause() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[G[_]]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 2, 3),
                token(TokenKind::Identifier, 3, 4),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        let TreeKind::TypeDef(definition) = &parser.ast().get(params[0]).kind else {
            panic!("expected an outer type parameter");
        };
        let TreeKind::LambdaTypeTree(lambda) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a higher-kinded lambda type");
        };
        assert_eq!(lambda.type_params.len(), 1);
        let TreeKind::TypeDef(nested) = &parser.ast().get(lambda.type_params[0]).kind else {
            panic!("expected a nested type parameter");
        };
        assert_eq!(
            parser.names.resolve(nested.name.as_name().text()),
            "$type_wildcard_0"
        );
        assert!(matches!(
            parser.ast().get(nested.rhs).kind,
            TreeKind::TypeBoundsTree(TypeBoundsTree {
                low: None,
                high: None,
                alias: None,
            })
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn keeps_bounds_in_a_higher_kinded_type_parameter_lambda() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[G[_] <: Bound]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 2, 3),
                token(TokenKind::Identifier, 3, 4),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 4, 5),
                token(TokenKind::Operator, 6, 8),
                token(TokenKind::Identifier, 9, 14),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 14, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        let TreeKind::TypeDef(definition) = &parser.ast().get(params[0]).kind else {
            panic!("expected an outer type parameter");
        };
        let TreeKind::LambdaTypeTree(lambda) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a higher-kinded lambda type");
        };
        assert_eq!(
            parser
                .ast()
                .get(definition.rhs)
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(3, 14).unwrap()
        );
        let TreeKind::TypeBoundsTree(bounds) = &parser.ast().get(lambda.body).kind else {
            panic!("expected higher-kinded bounds");
        };
        assert!(bounds.low.is_none());
        assert!(matches!(
            bounds.high.map(|high| &parser.ast().get(high).kind),
            Some(TreeKind::Ident(_))
        ));
        assert_eq!(
            parser
                .ast()
                .get(lambda.body)
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(6, 14).unwrap()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_an_annotation_on_a_type_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[@ann A]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Operator, 1, 2),
                token(TokenKind::Identifier, 2, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        let TreeKind::TypeDef(definition) = &parser.ast().get(params[0]).kind else {
            panic!("expected a type parameter");
        };
        assert_eq!(definition.metadata.annotations.len(), 1);
        assert!(matches!(
            parser.ast().get(definition.metadata.annotations[0]).kind,
            TreeKind::Apply(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_an_annotation_on_a_wildcard_type_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[@ann _]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Operator, 1, 2),
                token(TokenKind::Identifier, 2, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        let TreeKind::TypeDef(definition) = &parser.ast().get(params[0]).kind else {
            panic!("expected a wildcard type parameter");
        };
        assert_eq!(definition.metadata.annotations.len(), 1);
        assert_eq!(
            parser.names.resolve(definition.name.as_name().text()),
            "$type_wildcard_0"
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn type_parameter_recovery_outside_enum_body_consumes_newlines() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A B\n]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Identifier, 3, 4),
                token(TokenKind::Newline, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);

        assert_eq!(params.len(), 1);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].issue(),
            &ParseIssue::TypeParameter(TypeParamIssue::ExpectedParameterSeparator {
                found: TokenKind::Identifier,
            })
        );
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedToken
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(3, 4).unwrap()
        );
    }

    #[test]
    fn empty_type_parameter_clause_reports_a_missing_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 1, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        assert!(params.is_empty());
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].issue(),
            &ParseIssue::TypeParameter(TypeParamIssue::EmptyParameterClause {
                found: TokenKind::Punctuation(Punctuation::RightBracket),
            })
        );
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedType
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(1, 2).unwrap()
        );
    }
}
