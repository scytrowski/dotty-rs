//! Source-level class, trait, and object definitions.
//!
//! This module owns the initial definition grammar: constructor clauses,
//! parent applications, and braced or indented template bodies are preserved
//! in the shared `TypeDef`/`Template` and `ModuleDef` shapes. Semantic class
//! and template processing remains outside the parser.

use dotty_core::ast::{
    DefDef, Ident, Modifier, Modifiers, ModuleDef, New, Template, TypeDef, UntypedTemplateMetadata,
};
use dotty_core::{
    HardKeyword, Punctuation, SourceSpan, Span, TermName, TokenKind, TreeId, TreeKind, TypeName,
    Untyped,
};

use crate::modifiers::DefinitionPrefix;
use crate::statements::ParsedStatement;
use crate::templates::{TemplateBody, TemplateBodyResult};
use crate::{Location, ParseDiagnosticKind, Parser};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParentSeparator {
    Comma,
    With,
}

#[derive(Clone, Copy)]
struct ConstructorBoundary {
    parameter_start: Option<u32>,
    constructor_metadata_start: Option<u32>,
    parent_start: Option<u32>,
    body_start: Option<u32>,
}

struct TemplateTail {
    parents: Vec<TreeId<Untyped>>,
    self_val: Option<TreeId<Untyped>>,
    body: Vec<TreeId<Untyped>>,
    metadata: UntypedTemplateMetadata,
}

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(crate) fn parse_class_definition(&mut self, _location: Location) -> ParsedStatement {
        self.parse_class_definition_with_prefix(DefinitionPrefix::empty(self.mark().start()))
    }

    pub(crate) fn parse_class_definition_with_prefix(
        &mut self,
        prefix: DefinitionPrefix,
    ) -> ParsedStatement {
        self.parse_type_like_definition(false, false, false, prefix)
    }

    pub(crate) fn parse_enum_definition(&mut self, _location: Location) -> ParsedStatement {
        self.parse_enum_definition_with_prefix(DefinitionPrefix::empty(self.mark().start()))
    }

    pub(crate) fn parse_enum_definition_with_prefix(
        &mut self,
        prefix: DefinitionPrefix,
    ) -> ParsedStatement {
        self.parse_type_like_definition(false, false, true, prefix)
    }

    pub(crate) fn parse_case_class_definition(&mut self, _location: Location) -> ParsedStatement {
        self.parse_case_class_definition_with_prefix(DefinitionPrefix::empty(self.mark().start()))
    }

    pub(crate) fn parse_case_class_definition_with_prefix(
        &mut self,
        prefix: DefinitionPrefix,
    ) -> ParsedStatement {
        self.parse_type_like_definition(false, true, false, prefix)
    }

    pub(crate) fn parse_trait_definition(&mut self, _location: Location) -> ParsedStatement {
        self.parse_trait_definition_with_prefix(DefinitionPrefix::empty(self.mark().start()))
    }

    pub(crate) fn parse_trait_definition_with_prefix(
        &mut self,
        prefix: DefinitionPrefix,
    ) -> ParsedStatement {
        self.parse_type_like_definition(true, false, false, prefix)
    }

    pub(crate) fn parse_object_definition(&mut self, _location: Location) -> ParsedStatement {
        self.parse_object_definition_with_prefix(DefinitionPrefix::empty(self.mark().start()))
    }

    pub(crate) fn parse_object_definition_with_prefix(
        &mut self,
        prefix: DefinitionPrefix,
    ) -> ParsedStatement {
        self.parse_object_definition_with_case(prefix, false)
    }

    pub(crate) fn parse_case_object_definition(&mut self, _location: Location) -> ParsedStatement {
        self.parse_case_object_definition_with_prefix(DefinitionPrefix::empty(self.mark().start()))
    }

    pub(crate) fn parse_case_object_definition_with_prefix(
        &mut self,
        prefix: DefinitionPrefix,
    ) -> ParsedStatement {
        self.parse_object_definition_with_case(prefix, true)
    }

    fn parse_object_definition_with_case(
        &mut self,
        prefix: DefinitionPrefix,
        is_case: bool,
    ) -> ParsedStatement {
        let mark = crate::Mark {
            start: prefix.start,
        };
        self.advance();
        let name = self.parse_object_name();
        let mut metadata = prefix.metadata;
        if is_case {
            metadata.modifiers.push(Modifier::Case);
        }
        self.parse_module_definition_after_name(mark, name, metadata)
    }

    fn parse_module_definition_after_name(
        &mut self,
        mark: crate::Mark,
        name: TermName,
        metadata: Modifiers,
    ) -> ParsedStatement {
        let tail = self.with_secondary_constructor_allowed(false, |parser| {
            parser.with_enum_body(false, |parser| parser.parse_template_tail(false))
        });
        self.build_module_definition(mark, name, metadata, tail)
    }

    fn parse_enum_case_module_after_name(
        &mut self,
        mark: crate::Mark,
        name: TermName,
        metadata: Modifiers,
    ) -> ParsedStatement {
        // A singleton enum case uses Dotty's `caseTemplate`, not the general
        // template tail. In particular, it must not consume a body, `derives`,
        // or `uses` after the case name.
        self.build_module_definition(
            mark,
            name,
            metadata,
            TemplateTail {
                parents: Vec::new(),
                self_val: None,
                body: Vec::new(),
                metadata: UntypedTemplateMetadata {
                    derives: Vec::new(),
                    uses: Vec::new(),
                },
            },
        )
    }

    fn build_module_definition(
        &mut self,
        mark: crate::Mark,
        name: TermName,
        metadata: Modifiers,
        tail: TemplateTail,
    ) -> ParsedStatement {
        let parent_start = self.template_tail_start(&tail);
        let body_start = tail.body.first().and_then(|member| {
            self.ast
                .get(*member)
                .position
                .map(|position| position.span().range().start())
        });
        let (constructor, constructor_start) = self.synthetic_primary_constructor(
            mark.start(),
            Vec::new(),
            Vec::new(),
            mark.start(),
            ConstructorBoundary {
                parameter_start: None,
                constructor_metadata_start: None,
                parent_start,
                body_start,
            },
            Modifiers::default(),
        );
        let template_position = self.template_position(constructor, constructor_start, &tail);
        let template = self.alloc_from(
            mark,
            TreeKind::Template(Template {
                constructor,
                parents: tail.parents,
                self_val: tail.self_val,
                body: tail.body,
                metadata: tail.metadata,
            }),
        );
        self.ast.get_mut(template).position = Some(template_position);

        ParsedStatement::Definition(self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(ModuleDef {
                name,
                template,
                metadata,
            })),
        ))
    }

    pub(crate) fn parse_enum_case(&mut self) -> ParsedStatement {
        self.parse_enum_case_with_prefix(DefinitionPrefix::empty(self.mark().start()))
    }

    pub(crate) fn parse_enum_case_with_prefix(
        &mut self,
        prefix: DefinitionPrefix,
    ) -> ParsedStatement {
        let mark = crate::Mark {
            start: prefix.start,
        };
        let case_position = self.current_span();
        let metadata = self.enum_case_metadata(prefix.metadata);
        self.advance();

        let name_mark = self.mark();
        let (type_name, backquoted, case_name_end) = match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
                let name_end = self.current().span.end();
                match self.intern_current_type_name() {
                    Ok(name) => {
                        self.advance();
                        (name, backquoted, name_end)
                    }
                    Err(_) => return self.malformed_enum_case(case_position),
                }
            }
            _ => return self.malformed_enum_case(case_position),
        };

        let term_name = TermName::new(type_name.as_name().text());
        if matches!(
            self.current().kind,
            TokenKind::Punctuation(Punctuation::LeftBracket | Punctuation::LeftParen)
        ) || self.current_is_modifier()
        {
            return self.parse_parameterized_enum_case(
                mark,
                case_position,
                case_name_end,
                type_name,
                metadata,
            );
        }

        if let Some(body_start) = self.enum_case_body_start() {
            return self.reject_enum_case_body(case_position, body_start);
        }

        if self.enum_case_requires_unsupported_recovery() {
            return self.unsupported_enum_case(case_position);
        }

        if self.current().kind == TokenKind::Punctuation(Punctuation::Comma) {
            let first = self.alloc_from(
                name_mark,
                TreeKind::Ident(Ident {
                    name: *term_name.as_name(),
                    backquoted,
                }),
            );
            return self.parse_enum_case_group(mark, case_position, first, metadata);
        }

        self.parse_enum_case_module_after_name(mark, term_name, metadata)
    }

    fn parse_parameterized_enum_case(
        &mut self,
        mark: crate::Mark,
        case_position: dotty_core::SourceSpan,
        case_name_end: u32,
        name: TypeName,
        metadata: Modifiers,
    ) -> ParsedStatement {
        let diagnostics_before_type_params = self.diagnostics.len();
        let type_params = if self.current().kind == TokenKind::Punctuation(Punctuation::LeftBracket)
        {
            self.parse_type_param_clause(crate::ParamOwner::CaseClass)
        } else {
            Vec::new()
        };
        let type_params_are_valid = self.diagnostics.len() == diagnostics_before_type_params;
        let constructor_metadata_start = self.current_is_modifier().then_some(case_name_end);
        let constructor_metadata = if constructor_metadata_start.is_some() {
            self.parse_constructor_modifiers()
        } else {
            Modifiers::default()
        };
        if self.current_is_modifier() {
            return self.unsupported_enum_case(case_position);
        }
        self.consume_newlines_before_parameter_clause(TokenKind::Punctuation(
            Punctuation::LeftParen,
        ));
        let (value_param_clauses, parameter_start) =
            if self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen) {
                let parameter_start = self.current().span.start();
                (
                    self.parse_term_param_clauses(crate::ParamOwner::CaseClass),
                    Some(parameter_start),
                )
            } else if (type_params.is_empty() && constructor_metadata_start.is_none())
                || !type_params_are_valid
            {
                if type_params_are_valid {
                    if self.enum_case_requires_unsupported_recovery() {
                        return self.unsupported_enum_case(case_position);
                    } else {
                        self.report(
                            ParseDiagnosticKind::ExpectedToken,
                            "expected an enum case constructor parameter clause",
                        );
                    }
                }
                self.recover_until(crate::RecoverySet::Case);
                return ParsedStatement::Expression(self.error_expr(case_position));
            } else {
                (Vec::new(), None)
            };
        let constructor_end = self.last_real_token_end;
        let parents = self.parse_parent_clause();
        let parent_start = parents.first().and_then(|parent| {
            self.ast
                .get(*parent)
                .position
                .map(|position| position.span().range().start())
        });
        if let Some(body_start) = self.enum_case_body_start() {
            return self.reject_enum_case_body(case_position, body_start);
        }
        if self.enum_case_requires_unsupported_recovery() {
            return self.unsupported_enum_case(case_position);
        }

        let tail = TemplateTail {
            parents: parents.clone(),
            self_val: None,
            body: Vec::new(),
            metadata: UntypedTemplateMetadata {
                derives: Vec::new(),
                uses: Vec::new(),
            },
        };
        let (constructor, constructor_start) = self.synthetic_primary_constructor(
            mark.start(),
            type_params,
            value_param_clauses,
            constructor_end,
            ConstructorBoundary {
                parameter_start,
                constructor_metadata_start,
                parent_start,
                body_start: None,
            },
            constructor_metadata,
        );
        let template_position = self.template_position(constructor, constructor_start, &tail);
        let template = self.alloc_from(
            mark,
            TreeKind::Template(Template {
                constructor,
                parents: tail.parents,
                self_val: None,
                body: Vec::new(),
                metadata: tail.metadata,
            }),
        );
        self.ast.get_mut(template).position = Some(template_position);

        ParsedStatement::Definition(self.alloc_from(
            mark,
            TreeKind::TypeDef(TypeDef {
                name,
                rhs: template,
                metadata,
                variance: None,
            }),
        ))
    }

    fn parse_enum_case_group(
        &mut self,
        mark: crate::Mark,
        case_position: dotty_core::SourceSpan,
        first: TreeId<Untyped>,
        metadata: Modifiers,
    ) -> ParsedStatement {
        let mut patterns = vec![first];
        while self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
            match self.current().kind {
                TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                    let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
                    let Ok(name) = self.intern_current_term_name() else {
                        self.report(
                            ParseDiagnosticKind::ExpectedPattern,
                            "expected an identifier after `,` in an enum case",
                        );
                        self.recover_until(crate::RecoverySet::Case);
                        break;
                    };
                    let name = *name.as_name();
                    let pattern_mark = self.mark();
                    self.advance();
                    patterns.push(
                        self.alloc_from(pattern_mark, TreeKind::Ident(Ident { name, backquoted })),
                    );
                }
                _ => {
                    self.report(
                        ParseDiagnosticKind::ExpectedPattern,
                        "expected an identifier after `,` in an enum case",
                    );
                    self.recover_until(crate::RecoverySet::Case);
                    break;
                }
            }
        }

        if let Some(body_start) = self.enum_case_body_start() {
            return self.reject_enum_case_body(case_position, body_start);
        }

        let tpt = self.synthetic_type_tree_at(self.last_real_token_end);
        ParsedStatement::Definition(self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::PatDef(
                dotty_core::ast::PatDef {
                    modifiers: metadata,
                    patterns,
                    tpt,
                    rhs: None,
                },
            )),
        ))
    }

    fn enum_case_requires_unsupported_recovery(&mut self) -> bool {
        let immediate = matches!(
            self.current().kind,
            TokenKind::Keyword(HardKeyword::Extends)
                | TokenKind::Punctuation(
                    Punctuation::LeftBracket | Punctuation::LeftBrace | Punctuation::LeftParen,
                )
                | TokenKind::ColonFollow
                | TokenKind::ColonEol
                | TokenKind::ColonOp
        ) || (self.current().kind == TokenKind::Operator
            && self.current_text_is("@"));
        if immediate || self.current_is_modifier() {
            return true;
        }

        let mut offset = 0;
        while matches!(
            self.cursor.lookahead(offset).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            offset += 1;
        }
        let follows = self.cursor.lookahead(offset);
        matches!(
            follows.kind,
            TokenKind::Indent
                | TokenKind::Punctuation(Punctuation::LeftBrace)
                | TokenKind::Keyword(HardKeyword::Extends)
        ) || (follows.kind == TokenKind::Identifier
            && matches!(
                self.source.slice(follows.span).ok(),
                Some("derives" | "uses")
            ))
    }

    fn enum_case_body_start(&mut self) -> Option<SourceSpan> {
        let mut offset = 0;
        while matches!(
            self.cursor.lookahead(offset).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            offset += 1;
        }
        matches!(
            self.cursor.lookahead(offset).kind,
            TokenKind::Punctuation(Punctuation::LeftBrace)
                | TokenKind::Indent
                | TokenKind::ColonFollow
                | TokenKind::ColonEol
                | TokenKind::ColonOp
        )
        .then(|| {
            SourceSpan::new(
                self.source_id,
                Span::without_point(self.cursor.lookahead(offset).span),
            )
        })
    }

    fn reject_enum_case_body(
        &mut self,
        position: dotty_core::SourceSpan,
        body_start: dotty_core::SourceSpan,
    ) -> ParsedStatement {
        self.report_at(
            ParseDiagnosticKind::UnexpectedToken,
            body_start,
            "an enum case cannot have a template body in Scala 3.9",
        );
        self.recover_enum_case_body();
        ParsedStatement::Expression(self.error_expr(position))
    }

    /// Skips a rejected case-body attempt without treating its internal
    /// separators or closing delimiter as boundaries of the enclosing enum.
    fn recover_enum_case_body(&mut self) {
        let mut closers = Vec::new();
        loop {
            let kind = self.current().kind;
            if closers.is_empty() {
                match kind {
                    TokenKind::Newline | TokenKind::Newlines => {
                        if !self.advance_enum_case_recovery() {
                            return;
                        }
                        continue;
                    }
                    TokenKind::ColonFollow | TokenKind::ColonOp => {
                        self.observe_colon_eol(true);
                        if !self.advance_enum_case_recovery() {
                            return;
                        }
                        continue;
                    }
                    TokenKind::ColonEol => {
                        self.observe_indented();
                        if !self.advance_enum_case_recovery() {
                            return;
                        }
                        continue;
                    }
                    TokenKind::Punctuation(Punctuation::LeftBrace) => {
                        closers.push(TokenKind::Punctuation(Punctuation::RightBrace));
                    }
                    TokenKind::Indent => closers.push(TokenKind::Outdent),
                    _ => return,
                }
            } else if matches!(
                kind,
                TokenKind::Punctuation(Punctuation::LeftBrace) | TokenKind::Indent
            ) {
                closers.push(if kind == TokenKind::Indent {
                    TokenKind::Outdent
                } else {
                    TokenKind::Punctuation(Punctuation::RightBrace)
                });
            } else if matches!(
                kind,
                TokenKind::Punctuation(Punctuation::RightBrace) | TokenKind::Outdent
            ) {
                if closers.last().copied() != Some(kind) {
                    return;
                }
                closers.pop();
                let body_complete = closers.is_empty();
                if !self.advance_enum_case_recovery() || body_complete {
                    return;
                }
                continue;
            }

            if !self.advance_enum_case_recovery() {
                return;
            }
        }
    }

    fn advance_enum_case_recovery(&mut self) -> bool {
        let checkpoint = self.cursor.checkpoint();
        self.advance();
        self.cursor.progressed_since(checkpoint)
    }

    fn enum_case_metadata(&mut self, mut metadata: Modifiers) -> Modifiers {
        metadata.modifiers.retain(|modifier| {
            self.report(
                ParseDiagnosticKind::UnsupportedSyntax,
                format!("modifier `{modifier:?}` is not allowed on an enum case"),
            );
            false
        });
        metadata.modifiers.push(Modifier::EnumCase);
        metadata
    }

    fn malformed_enum_case(&mut self, position: dotty_core::SourceSpan) -> ParsedStatement {
        self.report(
            ParseDiagnosticKind::ExpectedPattern,
            "expected an identifier after `case`",
        );
        self.recover_until(crate::RecoverySet::Case);
        ParsedStatement::Expression(self.error_expr(position))
    }

    fn unsupported_enum_case(&mut self, position: dotty_core::SourceSpan) -> ParsedStatement {
        self.report(
            ParseDiagnosticKind::UnsupportedSyntax,
            "unsupported enum case syntax",
        );
        self.recover_until(crate::RecoverySet::Case);
        ParsedStatement::Expression(self.error_expr(position))
    }

    fn validate_enum_modifiers(&mut self, mut metadata: Modifiers) -> Modifiers {
        metadata.modifiers.retain(|modifier| {
            if matches!(modifier, Modifier::Infix) {
                return true;
            }
            self.report(
                ParseDiagnosticKind::UnsupportedSyntax,
                format!("modifier `{modifier:?}` is not allowed on an enum"),
            );
            false
        });
        metadata
    }

    fn parse_type_like_definition(
        &mut self,
        is_trait: bool,
        is_case: bool,
        is_enum: bool,
        prefix: DefinitionPrefix,
    ) -> ParsedStatement {
        let mark = crate::Mark {
            start: prefix.start,
        };
        self.advance();
        let name = self.parse_type_name();
        let owner = if is_case {
            crate::ParamOwner::CaseClass
        } else {
            crate::ParamOwner::Class
        };
        let type_params = if self.current().kind
            == TokenKind::Punctuation(dotty_core::Punctuation::LeftBracket)
        {
            self.parse_type_param_clause(owner)
        } else {
            Vec::new()
        };
        self.consume_newlines_before_parameter_clause(TokenKind::Punctuation(
            Punctuation::LeftParen,
        ));
        let parameter_start = (self.current().kind
            == TokenKind::Punctuation(Punctuation::LeftParen))
        .then_some(self.current().span.start());
        let value_param_clauses = self.parse_term_param_clauses(owner);
        let constructor_end = self.last_real_token_end;
        let tail = self.with_secondary_constructor_allowed(!is_trait && !is_enum, |parser| {
            if is_enum {
                parser.with_enum_body(true, |parser| parser.parse_template_tail(true))
            } else {
                parser.with_enum_body(false, |parser| parser.parse_template_tail(false))
            }
        });
        let parent_start = self.template_tail_start(&tail);
        let body_start = tail.body.first().and_then(|member| {
            self.ast
                .get(*member)
                .position
                .map(|position| position.span().range().start())
        });
        let (constructor, constructor_start) = self.synthetic_primary_constructor(
            mark.start(),
            type_params,
            value_param_clauses,
            constructor_end,
            ConstructorBoundary {
                parameter_start,
                constructor_metadata_start: None,
                parent_start,
                body_start,
            },
            Modifiers::default(),
        );
        let template_position = self.template_position(constructor, constructor_start, &tail);
        let template = self.alloc_from(
            mark,
            TreeKind::Template(Template {
                constructor,
                parents: tail.parents,
                self_val: tail.self_val,
                body: tail.body,
                metadata: tail.metadata,
            }),
        );
        self.ast.get_mut(template).position = Some(template_position);
        let mut metadata = prefix.metadata;
        if is_enum {
            metadata = self.validate_enum_modifiers(metadata);
        }
        if is_case {
            metadata.modifiers.push(Modifier::Case);
        }
        if is_trait {
            metadata.modifiers.push(Modifier::Trait);
        }
        if is_enum {
            metadata.modifiers.push(Modifier::Enum);
        }

        ParsedStatement::Definition(self.alloc_from(
            mark,
            TreeKind::TypeDef(TypeDef {
                name,
                rhs: template,
                metadata,
                variance: None,
            }),
        ))
    }

    fn parse_template_tail(&mut self, required_body: bool) -> TemplateTail {
        let parents = self.parse_parent_clause();
        let derives = self.parse_derives_clause();
        let uses = self.parse_uses_clause();
        let body = if required_body {
            self.parse_required_template_body()
        } else {
            self.parse_optional_template_body()
        };
        TemplateTail {
            parents,
            self_val: body.self_val,
            body: body.members,
            metadata: UntypedTemplateMetadata { derives, uses },
        }
    }

    fn parse_required_template_body(&mut self) -> TemplateBodyResult {
        let checkpoint = self.cursor.checkpoint();
        let body = self.parse_optional_template_body();
        if !self.cursor.progressed_since(checkpoint) {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected an enum body after its header",
            );
        }
        body
    }

    fn template_tail_start(&self, tail: &TemplateTail) -> Option<u32> {
        tail.parents
            .iter()
            .chain(tail.metadata.derives.iter())
            .chain(tail.metadata.uses.iter().map(|use_ref| &use_ref.reference))
            .copied()
            .chain(tail.self_val)
            .chain(tail.body.first().copied())
            .filter_map(|tree| {
                self.ast
                    .get(tree)
                    .position
                    .map(|position| position.span().range().start())
            })
            .min()
    }

    fn parse_derives_clause(&mut self) -> Vec<TreeId<Untyped>> {
        self.consume_newlines_before_template_name(self.known_names.derives);
        if !self.current_is_template_name(self.known_names.derives) {
            return Vec::new();
        }
        self.advance();

        let mut derives = Vec::new();
        loop {
            let derive = self.with_parse_kind(crate::ParseKind::Type, |parser| {
                parser.simple_type_reference()
            });
            if self
                .cursor
                .at(TokenKind::Punctuation(Punctuation::LeftBracket))
            {
                self.report(
                    ParseDiagnosticKind::UnsupportedSyntax,
                    "type applications are not supported in `derives` clauses",
                );
                self.recover_unsupported_derives_type_application();
            }
            derives.push(derive);
            if self.current().kind == TokenKind::Operator
                && (self.current_text_is("|") || self.current_text_is("&"))
            {
                self.report(
                    ParseDiagnosticKind::UnsupportedSyntax,
                    "infix type expressions are not supported in `derives` clauses",
                );
                self.recover_unsupported_derives_type_expression();
            }
            if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                break;
            }
        }
        derives
    }

    fn recover_unsupported_derives_type_application(&mut self) {
        let mut depth = 0u32;
        while self.current().kind != TokenKind::Eof {
            match self.current().kind {
                TokenKind::Punctuation(Punctuation::LeftBracket) => depth += 1,
                TokenKind::Punctuation(Punctuation::RightBracket) => {
                    depth = depth.saturating_sub(1);
                }
                _ => {}
            }
            let closes_application = depth == 0;
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) || closes_application {
                break;
            }
        }
    }

    fn recover_unsupported_derives_type_expression(&mut self) {
        while !matches!(
            self.current().kind,
            TokenKind::Eof
                | TokenKind::Newline
                | TokenKind::Newlines
                | TokenKind::Indent
                | TokenKind::Outdent
                | TokenKind::ColonFollow
                | TokenKind::ColonEol
                | TokenKind::Punctuation(
                    Punctuation::Comma
                        | Punctuation::LeftBrace
                        | Punctuation::RightBrace
                        | Punctuation::Semicolon
                )
        ) {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn parse_uses_clause(&mut self) -> Vec<dotty_core::ast::UseRef> {
        self.consume_newlines_before_template_name(self.known_names.uses);
        if !self.current_is_template_name(self.known_names.uses) {
            return Vec::new();
        }
        self.advance();

        let mut uses = Vec::new();
        loop {
            let reference = self.parse_capture_reference();
            let initially = self.current_is_template_name(self.known_names.initially);
            if initially {
                self.advance();
            }
            uses.push(dotty_core::ast::UseRef {
                reference,
                initially,
            });
            if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                break;
            }
        }
        uses
    }

    fn parse_capture_reference(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        match self.current().kind {
            TokenKind::Keyword(HardKeyword::This) => {
                let position = self.current_span();
                self.advance();
                let this = self.alloc(
                    TreeKind::This(dotty_core::ast::This { qual: None }),
                    Some(position),
                );
                self.parse_capture_selection(mark, this)
            }
            TokenKind::Keyword(HardKeyword::Super) => self.parse_super(mark, None),
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
                let Ok(name) = self.intern_current_term_name() else {
                    return self.error_expr(self.current_span());
                };
                self.advance();
                let ident = self.alloc_from(
                    mark,
                    TreeKind::Ident(dotty_core::ast::Ident {
                        name: *name.as_name(),
                        backquoted,
                    }),
                );
                if !self.accept(TokenKind::Punctuation(Punctuation::Dot)) {
                    return ident;
                }

                if self.accept(TokenKind::Keyword(HardKeyword::This)) {
                    let qualifier = self.alloc_from(
                        mark,
                        TreeKind::This(dotty_core::ast::This {
                            qual: Some(*name.as_name()),
                        }),
                    );
                    return self.parse_capture_selection(mark, qualifier);
                }

                if self.current().kind == TokenKind::Keyword(HardKeyword::Super) {
                    let qualifier_position = self.span_from(mark);
                    self.advance();
                    let qualifier = self.alloc(
                        TreeKind::This(dotty_core::ast::This {
                            qual: Some(*name.as_name()),
                        }),
                        Some(qualifier_position),
                    );
                    return self.parse_super_tail(mark, qualifier);
                }

                self.parse_capture_selection_after_dot(mark, ident)
            }
            _ => {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected a capture reference after `uses`",
                );
                self.error_expr(self.current_span())
            }
        }
    }

    fn parse_capture_selection(
        &mut self,
        mark: crate::Mark,
        qualifier: TreeId<Untyped>,
    ) -> TreeId<Untyped> {
        if !self.accept(TokenKind::Punctuation(Punctuation::Dot)) {
            return qualifier;
        }
        self.parse_capture_selection_after_dot(mark, qualifier)
    }

    fn parse_capture_selection_after_dot(
        &mut self,
        mark: crate::Mark,
        mut qualifier: TreeId<Untyped>,
    ) -> TreeId<Untyped> {
        loop {
            if !matches!(
                self.current().kind,
                TokenKind::Identifier | TokenKind::BackquotedIdentifier
            ) {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected a name after `.` in a capture reference",
                );
                return qualifier;
            }
            let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
            let Ok(name) = self.intern_current_term_name() else {
                return qualifier;
            };
            self.advance();
            qualifier = self.alloc_from(
                mark,
                TreeKind::Select(dotty_core::ast::Select {
                    qualifier,
                    name: *name.as_name(),
                    backquoted,
                }),
            );
            if !self.accept(TokenKind::Punctuation(Punctuation::Dot)) {
                break;
            }
        }
        qualifier
    }

    fn current_is_template_name(&mut self, expected: TermName) -> bool {
        self.current().kind == TokenKind::Identifier
            && self.current_is_known_name(expected).unwrap_or(false)
    }

    fn consume_newlines_before_template_name(&mut self, expected: TermName) {
        let mut count = 0;
        while matches!(
            self.cursor.lookahead(count).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            count += 1;
        }
        let follows = self.cursor.lookahead(count);
        let expected_text = self.names.resolve(expected.as_name().text());
        if follows.kind == TokenKind::Identifier
            && self.source.slice(follows.span).ok() == Some(expected_text)
        {
            for _ in 0..count {
                self.advance();
            }
        }
    }

    pub(crate) fn parse_optional_template_body(&mut self) -> TemplateBodyResult {
        self.consume_newlines_before_template_body();
        if matches!(
            self.current().kind,
            TokenKind::ColonFollow | TokenKind::ColonOp | TokenKind::ColonEol
        ) {
            if self.current().kind != TokenKind::ColonEol {
                self.observe_colon_eol(true);
            }
            if self.current().kind == TokenKind::ColonEol {
                self.observe_indented();
                self.advance();
                if self.current().kind == TokenKind::Indent {
                    return self.parse_template_body(TemplateBody::Indented);
                }
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected an indented template body after `:`",
                );
                return TemplateBodyResult {
                    self_val: None,
                    members: Vec::new(),
                };
            }
        }

        match self.current().kind {
            TokenKind::Punctuation(dotty_core::Punctuation::LeftBrace) => {
                self.parse_template_body(TemplateBody::Braced)
            }
            TokenKind::Indent => self.parse_template_body(TemplateBody::Indented),
            _ => TemplateBodyResult {
                self_val: None,
                members: Vec::new(),
            },
        }
    }

    fn consume_newlines_before_template_body(&mut self) {
        let mut count = 0;
        while matches!(
            self.cursor.lookahead(count).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            count += 1;
        }
        if matches!(
            self.cursor.lookahead(count).kind,
            TokenKind::Indent | TokenKind::Punctuation(Punctuation::LeftBrace)
        ) {
            for _ in 0..count {
                self.advance();
            }
        }
    }

    fn template_position(
        &self,
        constructor: TreeId<Untyped>,
        start: u32,
        tail: &TemplateTail,
    ) -> dotty_core::SourceSpan {
        let constructor_range = self.ast.get(constructor).position.map(|position| {
            (
                position.span().range().start(),
                position.span().range().end(),
            )
        });
        let body_start = tail.body.first().and_then(|tree| {
            self.ast
                .get(*tree)
                .position
                .map(|position| position.span().range().start())
        });
        let body_end = tail.body.last().and_then(|tree| {
            self.ast
                .get(*tree)
                .position
                .map(|position| position.span().range().end())
        });
        let parent_end = tail.parents.last().and_then(|tree| {
            self.ast
                .get(*tree)
                .position
                .map(|position| position.span().range().end())
        });
        let template_start = if tail.parents.is_empty()
            && constructor_range.is_some_and(|(constructor_start, constructor_end)| {
                constructor_start == constructor_end
            })
            && body_start.is_none_or(|body_start| body_start == start)
        {
            body_start.unwrap_or(start)
        } else {
            start
        };
        let metadata_end = tail
            .metadata
            .derives
            .iter()
            .chain(tail.metadata.uses.iter().map(|use_ref| &use_ref.reference))
            .chain(tail.self_val.iter())
            .filter_map(|tree| {
                self.ast
                    .get(*tree)
                    .position
                    .map(|position| position.span().range().end())
            })
            .max();
        let uses_end = tail
            .metadata
            .uses
            .iter()
            .filter_map(|use_ref| self.use_ref_end(use_ref))
            .max();
        let template_end = [
            body_end,
            metadata_end,
            uses_end,
            parent_end,
            constructor_range.map(|(_, end)| end),
        ]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(template_start);
        dotty_core::SourceSpan::new(
            self.source_id,
            dotty_core::Span::without_point(
                dotty_core::TextRange::new(template_start, template_end)
                    .expect("template span is ordered"),
            ),
        )
    }

    fn use_ref_end(&self, use_ref: &dotty_core::ast::UseRef) -> Option<u32> {
        let position = self.ast.get(use_ref.reference).position?;
        let end = position.span().range().end() as usize;
        if !use_ref.initially {
            return Some(end as u32);
        }
        let remainder = self.source.as_str().get(end..)?;
        let limit = remainder
            .find([',', ':', '\n', '\r', '}'])
            .unwrap_or(remainder.len());
        remainder[..limit]
            .find("initially")
            .map(|offset| end.saturating_add(offset + "initially".len()) as u32)
            .or(Some(end as u32))
    }

    fn parse_parent_clause(&mut self) -> Vec<TreeId<Untyped>> {
        self.consume_newlines_before_parent_keyword();
        if !self.accept(TokenKind::Keyword(HardKeyword::Extends)) {
            return Vec::new();
        }

        let mut parents = vec![self.parse_parent()];
        let mut separator_mode = None;
        loop {
            self.consume_newlines_before_parent_separator();
            let separator = match self.current().kind {
                TokenKind::Punctuation(Punctuation::Comma) => Some(ParentSeparator::Comma),
                TokenKind::Keyword(HardKeyword::With) => Some(ParentSeparator::With),
                _ => None,
            };
            let Some(separator) = separator else {
                break;
            };
            if separator_mode.is_some_and(|mode| mode != separator) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "cannot mix `,` and `with` in an extends clause",
                );
                break;
            }
            separator_mode = Some(separator);
            self.advance();
            if self.at_enum_body_parent_boundary() {
                self.report(
                    ParseDiagnosticKind::ExpectedType,
                    "expected a parent after the extends separator",
                );
                break;
            }
            parents.push(self.parse_parent());
        }
        parents
    }

    fn consume_newlines_before_parent_keyword(&mut self) {
        let count = self.newlines_before(|kind| kind == TokenKind::Keyword(HardKeyword::Extends));
        for _ in 0..count {
            self.advance();
        }
    }

    fn consume_newlines_before_parent_separator(&mut self) {
        let count = self.newlines_before(|kind| {
            matches!(
                kind,
                TokenKind::Punctuation(Punctuation::Comma) | TokenKind::Keyword(HardKeyword::With)
            )
        });
        for _ in 0..count {
            self.advance();
        }
    }

    fn newlines_before(&mut self, follows: impl Fn(TokenKind) -> bool) -> usize {
        let mut count = 0;
        while matches!(
            self.cursor.lookahead(count).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            count += 1;
        }
        if follows(self.cursor.lookahead(count).kind) {
            count
        } else {
            0
        }
    }

    pub(crate) fn parse_parent(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let mut parent =
            self.with_parse_kind(crate::ParseKind::Type, |parser| parser.simple_type());
        if self.current().kind == TokenKind::Punctuation(Punctuation::LeftBracket) {
            parent = self.parse_type_application(mark, parent);
        }
        if self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen) {
            let new_tree = self.alloc(
                TreeKind::New(New { tpt: parent }),
                self.ast.get(parent).position,
            );
            parent = self.parse_application(mark, new_tree);
            while self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen) {
                parent = self.parse_application(mark, parent);
            }
        }
        parent
    }

    pub(crate) fn at_enum_body_parent_boundary(&self) -> bool {
        self.context.enum_body
            && matches!(
                self.current().kind,
                TokenKind::Newline
                    | TokenKind::Newlines
                    | TokenKind::Indent
                    | TokenKind::Outdent
                    | TokenKind::Keyword(HardKeyword::Case)
                    | TokenKind::Punctuation(Punctuation::RightBrace)
                    | TokenKind::Eof
            )
    }

    fn parse_type_name(&mut self) -> TypeName {
        match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                match self.intern_current_type_name() {
                    Ok(name) => {
                        self.advance();
                        name
                    }
                    Err(_) => self.missing_type_name(),
                }
            }
            _ => {
                self.report(
                    ParseDiagnosticKind::ExpectedType,
                    "expected a type name after class or trait",
                );
                self.missing_type_name()
            }
        }
    }

    fn parse_object_name(&mut self) -> TermName {
        match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                match self.intern_current_term_name() {
                    Ok(name) => {
                        self.advance();
                        name
                    }
                    Err(_) => self.missing_object_name(),
                }
            }
            _ => {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected an object name after `object`",
                );
                self.missing_object_name()
            }
        }
    }

    fn missing_type_name(&mut self) -> TypeName {
        TypeName::new(self.names.intern("$missing_type_definition"))
    }

    fn missing_object_name(&mut self) -> TermName {
        TermName::new(self.names.intern("$missing_object"))
    }

    fn synthetic_primary_constructor(
        &mut self,
        start: u32,
        type_params: Vec<TreeId<Untyped>>,
        value_param_clauses: Vec<Vec<TreeId<Untyped>>>,
        parameter_end: u32,
        boundary: ConstructorBoundary,
        metadata: Modifiers,
    ) -> (TreeId<Untyped>, u32) {
        let name = TermName::new(self.names.intern("<init>"));
        let type_param_start = type_params
            .first()
            .and_then(|child| self.ast.get(*child).position)
            .map(|position| position.span().range().start());
        let value_param_start = value_param_clauses
            .iter()
            .flat_map(|clause| clause.iter())
            .next()
            .and_then(|child| self.ast.get(*child).position)
            .map(|position| position.span().range().start());
        let constructor_start = type_param_start
            .map(|child_start| child_start.saturating_sub(1))
            .or(boundary.constructor_metadata_start)
            .or(boundary.parameter_start)
            .or_else(|| value_param_start.map(|child_start| child_start.saturating_sub(1)))
            .or(boundary.parent_start)
            .or(boundary.body_start)
            .unwrap_or(start);
        let has_constructor_parameters = type_param_start.is_some()
            || value_param_start.is_some()
            || boundary.parameter_start.is_some();
        let constructor_end = boundary
            .parameter_start
            .map(|_| parameter_end)
            .or(has_constructor_parameters.then_some(parameter_end))
            .or(boundary.constructor_metadata_start.map(|_| parameter_end))
            .or(boundary.parent_start)
            .or(boundary.body_start)
            .unwrap_or(start);
        let tpt_start = value_param_clauses
            .last()
            .and_then(|clause| clause.last())
            .and_then(|parameter| match &self.ast.get(*parameter).kind {
                TreeKind::ValDef(parameter) => parameter
                    .rhs
                    .and_then(|rhs| self.ast.get(rhs).position)
                    .or_else(|| self.ast.get(parameter.tpt).position)
                    .map(|position| position.span().range().end()),
                _ => None,
            })
            .or_else(|| {
                type_params
                    .last()
                    .and_then(|parameter| self.ast.get(*parameter).position)
                    .map(|position| position.span().range().end())
            })
            .or(boundary.parameter_start)
            .unwrap_or(constructor_start);
        let tpt = self.synthetic_type_tree_at(tpt_start);
        let position = if has_constructor_parameters
            || boundary.constructor_metadata_start.is_some()
            || boundary.parent_start.is_some()
            || boundary.body_start.is_some()
        {
            dotty_core::SourceSpan::new(
                self.source_id,
                dotty_core::Span::without_point(
                    dotty_core::TextRange::new(constructor_start, constructor_end)
                        .expect("constructor span is ordered"),
                ),
            )
        } else {
            self.zero_width_span(start)
        };
        let constructor = self.alloc(
            TreeKind::DefDef(DefDef {
                name,
                type_params,
                value_param_clauses,
                tpt,
                rhs: None,
                metadata,
            }),
            Some(position),
        );
        (constructor, constructor_start)
    }
    /// Builds the existing source-level template shape used by a structural
    /// given. A given has no class name of its own, but its parent and body use
    /// exactly the same template machinery as class-like definitions.
    pub(crate) fn allocate_given_template(
        &mut self,
        start: u32,
        type_params: Vec<TreeId<Untyped>>,
        value_param_clauses: Vec<Vec<TreeId<Untyped>>>,
        parents: Vec<TreeId<Untyped>>,
        body: Vec<TreeId<Untyped>>,
    ) -> TreeId<Untyped> {
        let (constructor, constructor_start) = self.synthetic_given_constructor(
            start,
            type_params,
            value_param_clauses,
            &parents,
            &body,
        );
        let tail = TemplateTail {
            parents,
            self_val: None,
            body,
            metadata: UntypedTemplateMetadata::default(),
        };
        let position = self.template_position(constructor, constructor_start, &tail);
        let template = self.alloc_from(
            crate::Mark { start },
            TreeKind::Template(Template {
                constructor,
                parents: tail.parents,
                self_val: tail.self_val,
                body: tail.body,
                metadata: tail.metadata,
            }),
        );
        self.ast.get_mut(template).position = Some(position);
        template
    }

    fn synthetic_given_constructor(
        &mut self,
        start: u32,
        type_params: Vec<TreeId<Untyped>>,
        value_param_clauses: Vec<Vec<TreeId<Untyped>>>,
        parents: &[TreeId<Untyped>],
        body: &[TreeId<Untyped>],
    ) -> (TreeId<Untyped>, u32) {
        for type_param in &type_params {
            if let TreeKind::TypeDef(definition) = &mut self.ast.get_mut(*type_param).kind
                && !definition
                    .metadata
                    .modifiers
                    .contains(&Modifier::PrivateLocal)
            {
                definition.metadata.modifiers.push(Modifier::PrivateLocal);
            }
        }
        for parameter in value_param_clauses.iter().flatten() {
            if let TreeKind::ValDef(definition) = &mut self.ast.get_mut(*parameter).kind
                && !definition
                    .metadata
                    .modifiers
                    .contains(&Modifier::ParamAccessor)
            {
                definition.metadata.modifiers.push(Modifier::ParamAccessor);
            }
        }

        let type_param_start = type_params.first().and_then(|child| {
            self.ast
                .get(*child)
                .position
                .map(|position| position.span().range().start())
        });
        let value_param_start = value_param_clauses
            .iter()
            .flat_map(|clause| clause.iter())
            .next()
            .and_then(|child| {
                self.ast
                    .get(*child)
                    .position
                    .map(|position| position.span().range().start())
            });
        let parent_start = parents.first().and_then(|parent| {
            self.ast
                .get(*parent)
                .position
                .map(|position| position.span().range().start())
        });
        let body_start = body.first().and_then(|member| {
            self.ast
                .get(*member)
                .position
                .map(|position| position.span().range().start())
        });
        let constructor_start = type_param_start
            .or(value_param_start)
            .or(parent_start)
            .or(body_start)
            .unwrap_or(start);
        let constructor_end = value_param_clauses
            .last()
            .and_then(|clause| clause.last())
            .and_then(|parameter| self.ast.get(*parameter).position)
            .map(|position| position.span().range().end())
            .or_else(|| {
                type_params
                    .last()
                    .and_then(|parameter| self.ast.get(*parameter).position)
                    .map(|position| position.span().range().end())
            })
            .or(parent_start)
            .or(body_start)
            .unwrap_or(constructor_start);
        let tpt = self.synthetic_type_tree_at(constructor_end);
        let constructor_name = TermName::new(self.names.intern("<init>"));
        let constructor = self.alloc(
            TreeKind::DefDef(DefDef {
                name: constructor_name,
                type_params,
                value_param_clauses,
                tpt,
                rhs: None,
                metadata: Modifiers::default(),
            }),
            Some(dotty_core::SourceSpan::new(
                self.source_id,
                dotty_core::Span::without_point(
                    dotty_core::TextRange::new(constructor_start, constructor_end)
                        .expect("given constructor span is ordered"),
                ),
            )),
        );
        (constructor, constructor_start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::{TypeDef, UntypedNode};
    use dotty_core::{NameInterner, Punctuation};

    #[test]
    fn parses_an_empty_class_as_a_type_def_with_a_template() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class C",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(TypeDef { rhs, metadata, .. }) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        assert!(metadata.modifiers.is_empty());
        let TreeKind::Template(template) = &parser.ast().get(*rhs).kind else {
            panic!("expected Template");
        };
        assert!(template.parents.is_empty());
        assert!(template.body.is_empty());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn reports_a_missing_enum_body_without_losing_the_definition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(TypeDef { rhs, metadata, .. }) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        assert_eq!(metadata.modifiers, vec![Modifier::Enum]);
        assert!(matches!(parser.ast().get(*rhs).kind, TreeKind::Template(_)));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedToken
        );
        assert!(parser.diagnostics()[0].message().contains("enum body"));
    }

    #[test]
    fn rejects_a_disallowed_enum_modifier_without_losing_the_definition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "final enum E",
            vec![
                token(TokenKind::Keyword(HardKeyword::Final), 0, 5),
                token(TokenKind::Keyword(HardKeyword::Enum), 6, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        assert_eq!(definition.metadata.modifiers, vec![Modifier::Enum]);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| { diagnostic.kind() == ParseDiagnosticKind::UnsupportedSyntax })
        );
    }

    #[test]
    fn preserves_allowed_enum_visibility_and_infix_modifiers() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "private infix enum E {}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Private), 0, 7),
                token(TokenKind::Identifier, 8, 13),
                token(TokenKind::Keyword(HardKeyword::Enum), 14, 18),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 21, 22),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 22, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        assert_eq!(
            definition.metadata.modifiers,
            vec![Modifier::Infix, Modifier::Enum]
        );
        assert!(definition.metadata.visibility.is_some());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_singleton_enum_case_but_preserves_a_following_member() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case A\ndef f = x }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Newline, 15, 16),
                token(TokenKind::Keyword(HardKeyword::Def), 16, 19),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Operator, 22, 23),
                token(TokenKind::Identifier, 24, 25),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 26, 27),
                token(TokenKind::Eof, 27, 27),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(ref module))
                if module.metadata.modifiers == vec![Modifier::EnumCase]
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::DefDef(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_prefix_metadata_on_all_enum_case_definition_shapes() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { @A case Red, Green\nprivate[pkg] case Some(value: A)\nprotected case Child(value: A) extends Parent\n}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Operator, 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Keyword(HardKeyword::Case), 12, 16),
                token(TokenKind::Identifier, 17, 20),
                token(TokenKind::Punctuation(Punctuation::Comma), 20, 21),
                token(TokenKind::Identifier, 22, 27),
                token(TokenKind::Newline, 27, 28),
                token(TokenKind::Keyword(HardKeyword::Private), 28, 35),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 35, 36),
                token(TokenKind::Identifier, 36, 39),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 39, 40),
                token(TokenKind::Keyword(HardKeyword::Case), 41, 45),
                token(TokenKind::Identifier, 46, 50),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 50, 51),
                token(TokenKind::Identifier, 51, 56),
                token(TokenKind::ColonFollow, 56, 57),
                token(TokenKind::Identifier, 58, 59),
                token(TokenKind::Punctuation(Punctuation::RightParen), 59, 60),
                token(TokenKind::Newline, 60, 61),
                token(TokenKind::Keyword(HardKeyword::Protected), 61, 70),
                token(TokenKind::Keyword(HardKeyword::Case), 71, 75),
                token(TokenKind::Identifier, 76, 81),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 81, 82),
                token(TokenKind::Identifier, 82, 87),
                token(TokenKind::ColonFollow, 87, 88),
                token(TokenKind::Identifier, 89, 90),
                token(TokenKind::Punctuation(Punctuation::RightParen), 90, 91),
                token(TokenKind::Keyword(HardKeyword::Extends), 92, 99),
                token(TokenKind::Identifier, 100, 106),
                token(TokenKind::Newline, 106, 107),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 107, 108),
                token(TokenKind::Eof, 108, 108),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        assert_eq!(template.body.len(), 3);

        let TreeKind::PhaseSpecific(UntypedNode::PatDef(group)) =
            &parser.ast().get(template.body[0]).kind
        else {
            panic!("expected a comma-separated enum case group");
        };
        assert_eq!(group.modifiers.modifiers, vec![Modifier::EnumCase]);
        assert_eq!(group.modifiers.annotations.len(), 1);
        assert!(group.modifiers.visibility.is_none());
        assert_eq!(
            parser
                .ast()
                .get(template.body[0])
                .position
                .expect("enum case group span")
                .span()
                .range()
                .start(),
            9
        );

        let TreeKind::TypeDef(parameterized) = &parser.ast().get(template.body[1]).kind else {
            panic!("expected a parameterized enum case");
        };
        assert_eq!(parameterized.metadata.modifiers, vec![Modifier::EnumCase]);
        let Some(dotty_core::ast::VisibilitySyntax::Private {
            qualifier: Some(qualifier),
        }) = parameterized.metadata.visibility
        else {
            panic!("expected qualified private visibility");
        };
        assert_eq!(parser.names.resolve(qualifier.text()), "pkg");
        assert!(parameterized.metadata.annotations.is_empty());

        let TreeKind::TypeDef(parented) = &parser.ast().get(template.body[2]).kind else {
            panic!("expected a parented enum case");
        };
        assert_eq!(parented.metadata.modifiers, vec![Modifier::EnumCase]);
        assert_eq!(
            parented.metadata.visibility,
            Some(dotty_core::ast::VisibilitySyntax::Protected { qualifier: None })
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_comma_separated_enum_case_as_one_pattern_definition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case Red, Green\ndef after = x }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 17),
                token(TokenKind::Punctuation(Punctuation::Comma), 17, 18),
                token(TokenKind::Identifier, 19, 24),
                token(TokenKind::Newline, 24, 25),
                token(TokenKind::Keyword(HardKeyword::Def), 25, 28),
                token(TokenKind::Identifier, 29, 34),
                token(TokenKind::Operator, 35, 36),
                token(TokenKind::Identifier, 37, 38),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 39, 40),
                token(TokenKind::Eof, 40, 40),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        let TreeKind::PhaseSpecific(UntypedNode::PatDef(pattern_definition)) =
            &parser.ast().get(template.body[0]).kind
        else {
            panic!("expected a PatDef");
        };
        assert_eq!(
            pattern_definition.modifiers.modifiers,
            vec![Modifier::EnumCase]
        );
        assert_eq!(pattern_definition.patterns.len(), 2);
        assert!(matches!(
            parser.ast().get(pattern_definition.patterns[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            parser.ast().get(pattern_definition.patterns[1]).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(
            parser
                .ast()
                .get(pattern_definition.patterns[0])
                .position
                .expect("first pattern span")
                .span()
                .range()
                .start(),
            14
        );
        assert_eq!(
            parser
                .ast()
                .get(pattern_definition.patterns[1])
                .position
                .expect("second pattern span")
                .span()
                .range()
                .start(),
            19
        );
        assert!(matches!(
            parser.ast().get(pattern_definition.tpt).kind,
            TreeKind::TypeTree(_)
        ));
        assert!(pattern_definition.rhs.is_none());
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::DefDef(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_prefixed_enum_cases_but_preserves_a_following_member() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { private case class C\ndef f = x }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Private), 9, 16),
                token(TokenKind::CaseClass, 17, 27),
                token(TokenKind::Identifier, 28, 29),
                token(TokenKind::Newline, 29, 30),
                token(TokenKind::Keyword(HardKeyword::Def), 30, 33),
                token(TokenKind::Identifier, 34, 35),
                token(TokenKind::Operator, 36, 37),
                token(TokenKind::Identifier, 38, 39),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 40, 41),
                token(TokenKind::Eof, 41, 41),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::DefDef(_)
        ));
        assert_eq!(
            parser
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::UnsupportedSyntax)
                .count(),
            1
        );
        assert!(parser.diagnostics()[0].message().contains("enum cases"));
    }

    #[test]
    fn parses_a_prefixed_singleton_enum_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { private case Red\ndef after = 1 }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Private), 9, 16),
                token(TokenKind::Keyword(HardKeyword::Case), 17, 21),
                token(TokenKind::Identifier, 22, 25),
                token(TokenKind::Newline, 25, 26),
                token(TokenKind::Keyword(HardKeyword::Def), 26, 29),
                token(TokenKind::Identifier, 30, 35),
                token(TokenKind::Operator, 36, 37),
                token(TokenKind::IntegerLiteral, 38, 39),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 40, 41),
                token(TokenKind::Eof, 41, 41),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) =
            &parser.ast().get(template.body[0]).kind
        else {
            panic!("expected a singleton enum case module");
        };
        assert_eq!(
            module.metadata.visibility,
            Some(dotty_core::ast::VisibilitySyntax::Private { qualifier: None })
        );
        assert_eq!(module.metadata.modifiers, vec![Modifier::EnumCase]);
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::DefDef(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn diagnoses_a_hard_modifier_before_an_enum_case_without_losing_the_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { final case Red\ndef after = 1 }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Final), 9, 14),
                token(TokenKind::Keyword(HardKeyword::Case), 15, 19),
                token(TokenKind::Identifier, 20, 23),
                token(TokenKind::Newline, 23, 24),
                token(TokenKind::Keyword(HardKeyword::Def), 24, 27),
                token(TokenKind::Identifier, 28, 33),
                token(TokenKind::Operator, 34, 35),
                token(TokenKind::IntegerLiteral, 36, 37),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 38, 39),
                token(TokenKind::Eof, 39, 39),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(case)) =
            &parser.ast().get(template.body[0]).kind
        else {
            panic!("expected the invalidly prefixed case to remain a ModuleDef");
        };
        assert_eq!(case.metadata.modifiers, vec![Modifier::EnumCase]);
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::DefDef(_)
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::UnsupportedSyntax
        );
        assert!(
            parser.diagnostics()[0]
                .message()
                .contains("not allowed on an enum case")
        );
    }

    #[test]
    fn diagnoses_a_soft_modifier_before_an_enum_case_without_treating_it_as_valid() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { inline case Red\ndef after = 1 }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Identifier, 9, 15),
                token(TokenKind::Keyword(HardKeyword::Case), 16, 20),
                token(TokenKind::Identifier, 21, 24),
                token(TokenKind::Newline, 24, 25),
                token(TokenKind::Keyword(HardKeyword::Def), 25, 28),
                token(TokenKind::Identifier, 29, 34),
                token(TokenKind::Operator, 35, 36),
                token(TokenKind::IntegerLiteral, 37, 38),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 39, 40),
                token(TokenKind::Eof, 40, 40),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(case)) =
            &parser.ast().get(template.body[0]).kind
        else {
            panic!("expected the invalidly prefixed case to remain a ModuleDef");
        };
        assert_eq!(case.metadata.modifiers, vec![Modifier::EnumCase]);
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::DefDef(_)
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(
            parser.diagnostics()[0]
                .message()
                .contains("not allowed on an enum case")
        );
    }

    #[test]
    fn recovers_from_a_missing_annotation_type_before_an_enum_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { @ case Red\ndef after = 1 }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Operator, 9, 10),
                token(TokenKind::Keyword(HardKeyword::Case), 11, 15),
                token(TokenKind::Identifier, 16, 19),
                token(TokenKind::Newline, 19, 20),
                token(TokenKind::Keyword(HardKeyword::Def), 20, 23),
                token(TokenKind::Identifier, 24, 29),
                token(TokenKind::Operator, 30, 31),
                token(TokenKind::IntegerLiteral, 32, 33),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 34, 35),
                token(TokenKind::Eof, 35, 35),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::DefDef(_)
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedType
        );
    }

    #[test]
    fn recovers_from_a_missing_qualified_visibility_closer_before_an_enum_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { private[pkg case Red\ndef after = 1 }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Private), 9, 16),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 16, 17),
                token(TokenKind::Identifier, 17, 20),
                token(TokenKind::Keyword(HardKeyword::Case), 21, 25),
                token(TokenKind::Identifier, 26, 29),
                token(TokenKind::Newline, 29, 30),
                token(TokenKind::Keyword(HardKeyword::Def), 30, 33),
                token(TokenKind::Identifier, 34, 39),
                token(TokenKind::Operator, 40, 41),
                token(TokenKind::IntegerLiteral, 42, 43),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 44, 45),
                token(TokenKind::Eof, 45, 45),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::DefDef(_)
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedToken
        );
    }

    #[test]
    fn recovers_from_a_prefixed_enum_case_without_a_name() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { private case\ndef after = 1 }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Private), 9, 16),
                token(TokenKind::Keyword(HardKeyword::Case), 17, 21),
                token(TokenKind::Newline, 21, 22),
                token(TokenKind::Keyword(HardKeyword::Def), 22, 25),
                token(TokenKind::Identifier, 26, 31),
                token(TokenKind::Operator, 32, 33),
                token(TokenKind::IntegerLiteral, 34, 35),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 36, 37),
                token(TokenKind::Eof, 37, 37),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::DefDef(_)
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedPattern
        );
    }

    #[test]
    fn diagnoses_a_soft_modifier_before_qualified_enum_visibility() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { inline private[pkg] case Broken\ncase Good }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Identifier, 9, 15),
                token(TokenKind::Keyword(HardKeyword::Private), 16, 23),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 23, 24),
                token(TokenKind::Identifier, 24, 27),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 27, 28),
                token(TokenKind::Keyword(HardKeyword::Case), 29, 33),
                token(TokenKind::Identifier, 34, 40),
                token(TokenKind::Newline, 40, 41),
                token(TokenKind::Keyword(HardKeyword::Case), 41, 45),
                token(TokenKind::Identifier, 46, 50),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 51, 52),
                token(TokenKind::Eof, 52, 52),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        assert_eq!(template.body.len(), 2);
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(first)) =
            &parser.ast().get(template.body[0]).kind
        else {
            panic!("expected the prefixed case to remain a ModuleDef");
        };
        assert_eq!(first.metadata.modifiers, vec![Modifier::EnumCase]);
        let Some(dotty_core::ast::VisibilitySyntax::Private {
            qualifier: Some(qualifier),
        }) = first.metadata.visibility
        else {
            panic!("expected qualified private visibility");
        };
        assert_eq!(parser.names.resolve(qualifier.text()), "pkg");
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(
            parser.diagnostics()[0]
                .message()
                .contains("not allowed on an enum case")
        );
    }

    #[test]
    fn diagnoses_a_soft_modifier_before_this_qualified_enum_visibility() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { inline private[this] case Broken\ncase Good }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Identifier, 9, 15),
                token(TokenKind::Keyword(HardKeyword::Private), 16, 23),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 23, 24),
                token(TokenKind::Keyword(HardKeyword::This), 24, 28),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 28, 29),
                token(TokenKind::Keyword(HardKeyword::Case), 30, 34),
                token(TokenKind::Identifier, 35, 41),
                token(TokenKind::Newline, 41, 42),
                token(TokenKind::Keyword(HardKeyword::Case), 42, 46),
                token(TokenKind::Identifier, 47, 51),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 52, 53),
                token(TokenKind::Eof, 53, 53),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        assert_eq!(template.body.len(), 2);
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(first)) =
            &parser.ast().get(template.body[0]).kind
        else {
            panic!("expected the prefixed case to remain a ModuleDef");
        };
        assert_eq!(first.metadata.modifiers, vec![Modifier::EnumCase]);
        let Some(dotty_core::ast::VisibilitySyntax::Private {
            qualifier: Some(qualifier),
        }) = first.metadata.visibility
        else {
            panic!("expected private[this] visibility");
        };
        assert_eq!(parser.names.resolve(qualifier.text()), "this");
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(
            parser.diagnostics()[0]
                .message()
                .contains("not allowed on an enum case")
        );
    }

    #[test]
    fn diagnoses_a_soft_modifier_across_a_prefix_newline() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { inline\ncase Broken\ndef after = 1 }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Identifier, 9, 15),
                token(TokenKind::Newline, 15, 16),
                token(TokenKind::Keyword(HardKeyword::Case), 16, 20),
                token(TokenKind::Identifier, 21, 27),
                token(TokenKind::Newline, 27, 28),
                token(TokenKind::Keyword(HardKeyword::Def), 28, 31),
                token(TokenKind::Identifier, 32, 37),
                token(TokenKind::Operator, 38, 39),
                token(TokenKind::IntegerLiteral, 40, 41),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 42, 43),
                token(TokenKind::Eof, 43, 43),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::DefDef(_)
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(
            parser.diagnostics()[0]
                .message()
                .contains("not allowed on an enum case")
        );
    }

    #[test]
    fn parses_a_parameterized_enum_case_as_a_type_definition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case Some(value: A)\ndef after = 1 }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 18),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 18, 19),
                token(TokenKind::Identifier, 19, 24),
                token(TokenKind::ColonFollow, 24, 25),
                token(TokenKind::Identifier, 26, 27),
                token(TokenKind::Punctuation(Punctuation::RightParen), 27, 28),
                token(TokenKind::Newline, 28, 29),
                token(TokenKind::Keyword(HardKeyword::Def), 29, 32),
                token(TokenKind::Identifier, 33, 38),
                token(TokenKind::Operator, 39, 40),
                token(TokenKind::IntegerLiteral, 41, 42),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 43, 44),
                token(TokenKind::Eof, 44, 44),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        let TreeKind::TypeDef(case_definition) = &parser.ast().get(template.body[0]).kind else {
            panic!("expected a parameterized enum case TypeDef");
        };
        assert_eq!(case_definition.metadata.modifiers, vec![Modifier::EnumCase]);
        let TreeKind::Template(case_template) = &parser.ast().get(case_definition.rhs).kind else {
            panic!("expected a parameterized enum case Template");
        };
        let TreeKind::DefDef(constructor) = &parser.ast().get(case_template.constructor).kind
        else {
            panic!("expected a synthetic enum case constructor");
        };
        assert_eq!(constructor.value_param_clauses.len(), 1);
        assert_eq!(constructor.value_param_clauses[0].len(), 1);
        assert!(matches!(
            parser
                .ast()
                .get(constructor.value_param_clauses[0][0])
                .kind,
            TreeKind::ValDef(ref parameter)
                if parameter.metadata.modifiers == vec![Modifier::ParamAccessor]
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::DefDef(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_post_name_annotation_and_visibility_on_enum_case_constructor() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case C @Ann private[pkg](x: Int) }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Operator, 16, 17),
                token(TokenKind::Identifier, 17, 20),
                token(TokenKind::Keyword(HardKeyword::Private), 21, 28),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 28, 29),
                token(TokenKind::Identifier, 29, 32),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 32, 33),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 33, 34),
                token(TokenKind::Identifier, 34, 35),
                token(TokenKind::ColonFollow, 35, 36),
                token(TokenKind::Identifier, 37, 40),
                token(TokenKind::Punctuation(Punctuation::RightParen), 40, 41),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 42, 43),
                token(TokenKind::Eof, 43, 43),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected an enum TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected the enum Template");
        };
        let TreeKind::TypeDef(case) = &parser.ast().get(template.body[0]).kind else {
            panic!("expected a parameterized enum-case TypeDef");
        };
        let TreeKind::Template(case_template) = &parser.ast().get(case.rhs).kind else {
            panic!("expected the enum-case Template");
        };
        let TreeKind::DefDef(constructor) = &parser.ast().get(case_template.constructor).kind
        else {
            panic!("expected the enum-case constructor");
        };
        assert_eq!(constructor.metadata.annotations.len(), 1);
        assert!(matches!(
            constructor.metadata.visibility,
            Some(dotty_core::ast::VisibilitySyntax::Private { qualifier: Some(_) })
        ));
        assert!(case.metadata.visibility.is_none());
        let constructor_span = parser
            .ast()
            .get(case_template.constructor)
            .position
            .expect("constructor should include its source metadata")
            .span()
            .range();
        assert_eq!((constructor_span.start(), constructor_span.end()), (15, 41));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_annotation_on_type_only_enum_case_constructor() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case Empty[T] @Ann }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 19),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 19, 20),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 21, 22),
                token(TokenKind::Operator, 23, 24),
                token(TokenKind::Identifier, 24, 27),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 28, 29),
                token(TokenKind::Eof, 29, 29),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected the enum TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected the enum Template");
        };
        let TreeKind::TypeDef(case) = &parser.ast().get(template.body[0]).kind else {
            panic!("expected a type-only enum-case TypeDef");
        };
        let TreeKind::Template(case_template) = &parser.ast().get(case.rhs).kind else {
            panic!("expected the enum-case Template");
        };
        let TreeKind::DefDef(constructor) = &parser.ast().get(case_template.constructor).kind
        else {
            panic!("expected the enum-case constructor");
        };
        assert!(constructor.value_param_clauses.is_empty());
        assert_eq!(constructor.metadata.annotations.len(), 1);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_a_non_access_constructor_modifier_without_losing_the_next_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case Bad final(x: Int)\ncase Good }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 17),
                token(TokenKind::Keyword(HardKeyword::Final), 18, 23),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 23, 24),
                token(TokenKind::Identifier, 24, 25),
                token(TokenKind::ColonFollow, 25, 26),
                token(TokenKind::Identifier, 27, 30),
                token(TokenKind::Punctuation(Punctuation::RightParen), 30, 31),
                token(TokenKind::Newline, 31, 32),
                token(TokenKind::Keyword(HardKeyword::Case), 32, 36),
                token(TokenKind::Identifier, 37, 41),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 42, 43),
                token(TokenKind::Eof, 43, 43),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected the enum TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected the enum Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(
            parser.diagnostics()[0]
                .message()
                .contains("unsupported enum case syntax")
        );
    }

    #[test]
    fn parses_generic_enum_case_type_parameters_with_case_class_owner() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case Packed[A](value: A)",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 11, 12),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 13, 14),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 14, 15),
                token(TokenKind::Identifier, 15, 20),
                token(TokenKind::ColonFollow, 20, 21),
                token(TokenKind::Identifier, 22, 23),
                token(TokenKind::Punctuation(Punctuation::RightParen), 23, 24),
                token(TokenKind::Eof, 24, 24),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_enum_case() else {
            panic!("expected a parameterized enum case definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        assert!(definition.name.as_name().is_type());
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        let TreeKind::DefDef(constructor) = &parser.ast().get(template.constructor).kind else {
            panic!("expected a constructor");
        };
        assert_eq!(constructor.type_params.len(), 1);
        let TreeKind::TypeDef(type_param) = &parser.ast().get(constructor.type_params[0]).kind
        else {
            panic!("expected a type parameter");
        };
        assert_eq!(
            type_param.metadata.modifiers,
            vec![Modifier::Param, Modifier::PrivateLocal]
        );
        assert_eq!(constructor.value_param_clauses.len(), 1);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_type_only_enum_case_without_value_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case Empty[T]",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 10),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 10, 11),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 12, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_enum_case() else {
            panic!("expected a parameterized enum case definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        let TreeKind::DefDef(constructor) = &parser.ast().get(template.constructor).kind else {
            panic!("expected a constructor");
        };
        assert_eq!(constructor.type_params.len(), 1);
        assert!(constructor.value_param_clauses.is_empty());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_empty_enum_case_constructor_clause_and_type_span() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case Empty()",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 10),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_enum_case() else {
            panic!("expected a parameterized enum case definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        let TreeKind::DefDef(constructor) = &parser.ast().get(template.constructor).kind else {
            panic!("expected a constructor");
        };
        assert_eq!(constructor.value_param_clauses, vec![Vec::new()]);
        let type_tree = parser.ast().get(constructor.tpt);
        assert!(matches!(type_tree.kind, TreeKind::TypeTree(_)));
        assert_eq!(
            type_tree.position.map(|position| position.span().range()),
            Some(dotty_core::TextRange::new(10, 10).unwrap())
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_multiple_enum_case_constructor_clauses() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case Curried(a: A)(b: B)",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 12),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 12, 13),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::ColonFollow, 14, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Punctuation(Punctuation::RightParen), 17, 18),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 18, 19),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::ColonFollow, 20, 21),
                token(TokenKind::Identifier, 22, 23),
                token(TokenKind::Punctuation(Punctuation::RightParen), 23, 24),
                token(TokenKind::Eof, 24, 24),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_enum_case() else {
            panic!("expected a parameterized enum case definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        let TreeKind::DefDef(constructor) = &parser.ast().get(template.constructor).kind else {
            panic!("expected a constructor");
        };
        assert_eq!(constructor.value_param_clauses.len(), 2);
        assert_eq!(constructor.value_param_clauses[0].len(), 1);
        assert_eq!(constructor.value_param_clauses[1].len(), 1);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_enum_case_parent_clauses_and_preserves_following_members() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case Some(value: A) extends Parent\ndef after = 1 }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 18),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 18, 19),
                token(TokenKind::Identifier, 19, 24),
                token(TokenKind::ColonFollow, 24, 25),
                token(TokenKind::Identifier, 26, 27),
                token(TokenKind::Punctuation(Punctuation::RightParen), 27, 28),
                token(TokenKind::Keyword(HardKeyword::Extends), 29, 36),
                token(TokenKind::Identifier, 37, 43),
                token(TokenKind::Newline, 43, 44),
                token(TokenKind::Keyword(HardKeyword::Def), 44, 47),
                token(TokenKind::Identifier, 48, 53),
                token(TokenKind::Operator, 54, 55),
                token(TokenKind::IntegerLiteral, 56, 57),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 58, 59),
                token(TokenKind::Eof, 59, 59),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.body.len(), 2);
        let TreeKind::TypeDef(case_definition) = &parser.ast().get(template.body[0]).kind else {
            panic!("expected a parameterized enum case TypeDef");
        };
        assert_eq!(case_definition.metadata.modifiers, vec![Modifier::EnumCase]);
        let TreeKind::Template(case_template) = &parser.ast().get(case_definition.rhs).kind else {
            panic!("expected an enum case Template");
        };
        assert_eq!(case_template.parents.len(), 1);
        assert!(matches!(
            parser.ast().get(case_template.parents[0]).kind,
            TreeKind::Ident(identifier) if identifier.name.is_type()
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::DefDef(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_enum_case_parent_constructor_applications() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case Child(value: A) extends Parent(value)",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 10),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 10, 11),
                token(TokenKind::Identifier, 11, 16),
                token(TokenKind::ColonFollow, 16, 17),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Punctuation(Punctuation::RightParen), 19, 20),
                token(TokenKind::Keyword(HardKeyword::Extends), 21, 28),
                token(TokenKind::Identifier, 29, 35),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 35, 36),
                token(TokenKind::Identifier, 36, 41),
                token(TokenKind::Punctuation(Punctuation::RightParen), 41, 42),
                token(TokenKind::Eof, 42, 42),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_enum_case() else {
            panic!("expected a parameterized enum case definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        let TreeKind::Apply(application) = &parser.ast().get(template.parents[0]).kind else {
            panic!("expected a parent constructor application");
        };
        assert_eq!(application.args.len(), 1);
        assert!(matches!(
            parser.ast().get(application.args[0]).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(
            parser
                .ast()
                .get(template.parents[0])
                .position
                .expect("parent application span")
                .span()
                .range(),
            dotty_core::TextRange::new(29, 42).unwrap()
        );
        assert_eq!(
            parser
                .ast()
                .get(definition.rhs)
                .position
                .expect("enum case template span")
                .span()
                .range(),
            dotty_core::TextRange::new(10, 42).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_repeated_enum_case_parent_argument_clauses_as_nested_applications() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case Child(value: A) extends Parent(first)(second)",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 10),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 10, 11),
                token(TokenKind::Identifier, 11, 16),
                token(TokenKind::ColonFollow, 16, 17),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Punctuation(Punctuation::RightParen), 19, 20),
                token(TokenKind::Keyword(HardKeyword::Extends), 21, 28),
                token(TokenKind::Identifier, 29, 35),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 35, 36),
                token(TokenKind::Identifier, 36, 41),
                token(TokenKind::Punctuation(Punctuation::RightParen), 41, 42),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 42, 43),
                token(TokenKind::Identifier, 43, 49),
                token(TokenKind::Punctuation(Punctuation::RightParen), 49, 50),
                token(TokenKind::Eof, 50, 50),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_enum_case() else {
            panic!("expected a parameterized enum case definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        let TreeKind::Apply(outer) = &parser.ast().get(template.parents[0]).kind else {
            panic!("expected the outer parent application");
        };
        let TreeKind::Apply(inner) = &parser.ast().get(outer.function).kind else {
            panic!("expected the inner parent application");
        };
        assert_eq!(inner.args.len(), 1);
        assert_eq!(outer.args.len(), 1);
        assert_eq!(
            parser
                .ast()
                .get(template.parents[0])
                .position
                .expect("curried parent span")
                .span()
                .range(),
            dotty_core::TextRange::new(29, 50).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_enum_case_parent_using_arguments_with_using_apply_kind() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case Child() extends Parent(using ctx)",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 10),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Keyword(HardKeyword::Extends), 13, 20),
                token(TokenKind::Identifier, 21, 27),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 27, 28),
                token(TokenKind::Identifier, 28, 33),
                token(TokenKind::Identifier, 34, 37),
                token(TokenKind::Punctuation(Punctuation::RightParen), 37, 38),
                token(TokenKind::Eof, 38, 38),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_enum_case() else {
            panic!("expected a parameterized enum case definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        let TreeKind::Apply(application) = &parser.ast().get(template.parents[0]).kind else {
            panic!("expected a parent application");
        };
        assert_eq!(application.kind, dotty_core::ast::ApplyKind::Using);
        assert_eq!(application.args.len(), 1);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn normalizes_enum_case_parent_named_arguments() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case Child() extends Parent(value = x)",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 10),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Keyword(HardKeyword::Extends), 13, 20),
                token(TokenKind::Identifier, 21, 27),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 27, 28),
                token(TokenKind::Identifier, 28, 33),
                token(TokenKind::Operator, 34, 35),
                token(TokenKind::Identifier, 36, 37),
                token(TokenKind::Punctuation(Punctuation::RightParen), 37, 38),
                token(TokenKind::Eof, 38, 38),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_enum_case() else {
            panic!("expected a parameterized enum case definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        let TreeKind::Apply(application) = &parser.ast().get(template.parents[0]).kind else {
            panic!("expected a parent application");
        };
        let TreeKind::NamedArg(named) = &parser.ast().get(application.args[0]).kind else {
            panic!("expected a named parent argument");
        };
        let name = named.name;
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(names.resolve(name.text()), "value");
    }

    #[test]
    fn preserves_enum_case_parent_order_for_multiple_parents() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case Child(value: A) extends Parent(value), Marker",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 10),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 10, 11),
                token(TokenKind::Identifier, 11, 16),
                token(TokenKind::ColonFollow, 16, 17),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Punctuation(Punctuation::RightParen), 19, 20),
                token(TokenKind::Keyword(HardKeyword::Extends), 21, 28),
                token(TokenKind::Identifier, 29, 35),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 35, 36),
                token(TokenKind::Identifier, 36, 41),
                token(TokenKind::Punctuation(Punctuation::RightParen), 41, 42),
                token(TokenKind::Punctuation(Punctuation::Comma), 42, 43),
                token(TokenKind::Identifier, 44, 50),
                token(TokenKind::Eof, 50, 50),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_enum_case() else {
            panic!("expected a parameterized enum case definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.parents.len(), 2);
        assert!(matches!(
            parser.ast().get(template.parents[0]).kind,
            TreeKind::Apply(_)
        ));
        assert!(matches!(
            parser.ast().get(template.parents[1]).kind,
            TreeKind::Ident(identifier) if identifier.name.is_type()
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_missing_enum_case_constructor_parenthesis_at_the_next_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case Some(\ncase None }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 18),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 18, 19),
                token(TokenKind::Newline, 19, 20),
                token(TokenKind::Keyword(HardKeyword::Case), 20, 24),
                token(TokenKind::Identifier, 25, 29),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 30, 31),
                token(TokenKind::Eof, 31, 31),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::TypeDef(_)
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_missing_enum_case_parent_and_preserves_following_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case Child(x: A) extends\ncase None }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 19),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 19, 20),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::ColonFollow, 21, 22),
                token(TokenKind::Identifier, 23, 24),
                token(TokenKind::Punctuation(Punctuation::RightParen), 24, 25),
                token(TokenKind::Keyword(HardKeyword::Extends), 26, 33),
                token(TokenKind::Newline, 33, 34),
                token(TokenKind::Keyword(HardKeyword::Case), 34, 38),
                token(TokenKind::Identifier, 39, 43),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 44, 45),
                token(TokenKind::Eof, 45, 45),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_missing_enum_case_parent_closer_at_the_next_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case Child(x: A) extends Parent(\ncase None }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 19),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 19, 20),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::ColonFollow, 21, 22),
                token(TokenKind::Identifier, 23, 24),
                token(TokenKind::Punctuation(Punctuation::RightParen), 24, 25),
                token(TokenKind::Keyword(HardKeyword::Extends), 26, 33),
                token(TokenKind::Identifier, 34, 40),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 40, 41),
                token(TokenKind::Newline, 41, 42),
                token(TokenKind::Keyword(HardKeyword::Case), 42, 46),
                token(TokenKind::Identifier, 47, 51),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 52, 53),
                token(TokenKind::Eof, 53, 53),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_trailing_enum_case_parent_separator_at_the_next_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case Child(x: A) extends Parent(x),\ncase None }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 19),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 19, 20),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::ColonFollow, 21, 22),
                token(TokenKind::Identifier, 23, 24),
                token(TokenKind::Punctuation(Punctuation::RightParen), 24, 25),
                token(TokenKind::Keyword(HardKeyword::Extends), 26, 33),
                token(TokenKind::Identifier, 34, 40),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 40, 41),
                token(TokenKind::Identifier, 41, 42),
                token(TokenKind::Punctuation(Punctuation::RightParen), 42, 43),
                token(TokenKind::Punctuation(Punctuation::Comma), 43, 44),
                token(TokenKind::Newline, 44, 45),
                token(TokenKind::Keyword(HardKeyword::Case), 45, 49),
                token(TokenKind::Identifier, 50, 54),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 55, 56),
                token(TokenKind::Eof, 56, 56),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.body.len(), 2);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_missing_enum_case_type_bracket_at_the_next_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case Packed[A\ncase None }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 20),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 20, 21),
                token(TokenKind::Identifier, 21, 22),
                token(TokenKind::Newline, 22, 23),
                token(TokenKind::Keyword(HardKeyword::Case), 23, 27),
                token(TokenKind::Identifier, 28, 32),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 33, 34),
                token(TokenKind::Eof, 34, 34),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_a_newline_template_body_after_a_singleton_enum_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case A\n{ def leaked = 1 }\ncase B }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Newline, 15, 16),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 16, 17),
                token(TokenKind::Keyword(HardKeyword::Def), 18, 21),
                token(TokenKind::Identifier, 22, 28),
                token(TokenKind::Operator, 29, 30),
                token(TokenKind::IntegerLiteral, 31, 32),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 33, 34),
                token(TokenKind::Newline, 34, 35),
                token(TokenKind::Keyword(HardKeyword::Case), 35, 39),
                token(TokenKind::Identifier, 40, 41),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 42, 43),
                token(TokenKind::Eof, 43, 43),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::UnexpectedToken
        );
        assert_eq!(parser.diagnostics()[0].span().start(), 16);
        assert!(
            parser.diagnostics()[0]
                .message()
                .contains("cannot have a template body")
        );
    }

    #[test]
    fn rejects_a_braced_template_body_after_a_grouped_enum_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case A, B { def leaked = 1 }\ncase C }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Punctuation(Punctuation::Comma), 15, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 19, 20),
                token(TokenKind::Keyword(HardKeyword::Def), 21, 24),
                token(TokenKind::Identifier, 25, 31),
                token(TokenKind::Operator, 32, 33),
                token(TokenKind::IntegerLiteral, 34, 35),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 36, 37),
                token(TokenKind::Newline, 37, 38),
                token(TokenKind::Keyword(HardKeyword::Case), 38, 42),
                token(TokenKind::Identifier, 43, 44),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 45, 46),
                token(TokenKind::Eof, 46, 46),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::UnexpectedToken
        );
        assert_eq!(parser.diagnostics()[0].span().start(), 19);
        assert!(
            parser.diagnostics()[0]
                .message()
                .contains("cannot have a template body")
        );
    }

    #[test]
    fn rejects_a_braced_template_body_after_a_parameterized_enum_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case A(x: Int)\n{ def leaked = 1 }\ncase B }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 15, 16),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Punctuation(Punctuation::Colon), 17, 18),
                token(TokenKind::Identifier, 19, 22),
                token(TokenKind::Punctuation(Punctuation::RightParen), 22, 23),
                token(TokenKind::Newline, 23, 24),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 24, 25),
                token(TokenKind::Keyword(HardKeyword::Def), 26, 29),
                token(TokenKind::Identifier, 30, 36),
                token(TokenKind::Operator, 37, 38),
                token(TokenKind::IntegerLiteral, 39, 40),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 41, 42),
                token(TokenKind::Newline, 42, 43),
                token(TokenKind::Keyword(HardKeyword::Case), 43, 47),
                token(TokenKind::Identifier, 48, 49),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 50, 51),
                token(TokenKind::Eof, 51, 51),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(
            parser.diagnostics()[0]
                .message()
                .contains("cannot have a template body")
        );
    }

    #[test]
    fn rejects_an_indented_template_body_after_a_singleton_enum_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case A:\n  def leaked = 1\ncase B }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::ColonEol, 15, 16),
                token(TokenKind::Indent, 16, 16),
                token(TokenKind::Keyword(HardKeyword::Def), 18, 21),
                token(TokenKind::Identifier, 22, 28),
                token(TokenKind::Operator, 29, 30),
                token(TokenKind::IntegerLiteral, 31, 32),
                token(TokenKind::Outdent, 32, 32),
                token(TokenKind::Newline, 32, 33),
                token(TokenKind::Keyword(HardKeyword::Case), 33, 37),
                token(TokenKind::Identifier, 38, 39),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 40, 41),
                token(TokenKind::Eof, 41, 41),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(
            parser.diagnostics()[0]
                .message()
                .contains("cannot have a template body")
        );
    }

    #[test]
    fn rejects_an_indented_template_body_after_a_parameterized_enum_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case A(x: Int):\n  def leaked = x\ncase B }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 15, 16),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Punctuation(Punctuation::Colon), 17, 18),
                token(TokenKind::Identifier, 19, 22),
                token(TokenKind::Punctuation(Punctuation::RightParen), 22, 23),
                token(TokenKind::ColonEol, 23, 24),
                token(TokenKind::Indent, 24, 24),
                token(TokenKind::Keyword(HardKeyword::Def), 27, 30),
                token(TokenKind::Identifier, 31, 37),
                token(TokenKind::Operator, 38, 39),
                token(TokenKind::Identifier, 40, 41),
                token(TokenKind::Outdent, 41, 41),
                token(TokenKind::Newline, 41, 42),
                token(TokenKind::Keyword(HardKeyword::Case), 42, 46),
                token(TokenKind::Identifier, 47, 48),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 49, 50),
                token(TokenKind::Eof, 50, 50),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(
            parser.diagnostics()[0]
                .message()
                .contains("cannot have a template body")
        );
    }

    #[test]
    fn keeps_singleton_enum_case_parent_clauses_deferred() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case Red extends Parent\ncase Blue }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 17),
                token(TokenKind::Keyword(HardKeyword::Extends), 18, 25),
                token(TokenKind::Identifier, 26, 32),
                token(TokenKind::Newline, 32, 33),
                token(TokenKind::Keyword(HardKeyword::Case), 33, 37),
                token(TokenKind::Identifier, 38, 42),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 43, 44),
                token(TokenKind::Eof, 44, 44),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| { diagnostic.message().contains("unsupported enum case") })
        );
    }

    #[test]
    fn parses_access_only_enum_case_constructor_without_value_params() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case A private\ndef after = 1 }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Keyword(HardKeyword::Private), 16, 23),
                token(TokenKind::Newline, 23, 24),
                token(TokenKind::Keyword(HardKeyword::Def), 24, 27),
                token(TokenKind::Identifier, 28, 33),
                token(TokenKind::Operator, 34, 35),
                token(TokenKind::IntegerLiteral, 36, 37),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 38, 39),
                token(TokenKind::Eof, 39, 39),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        let TreeKind::TypeDef(case) = &parser.ast().get(template.body[0]).kind else {
            panic!("expected the access-modified case to be a TypeDef");
        };
        assert!(case.metadata.visibility.is_none());
        let TreeKind::Template(case_template) = &parser.ast().get(case.rhs).kind else {
            panic!("expected the enum-case Template");
        };
        let TreeKind::DefDef(constructor) = &parser.ast().get(case_template.constructor).kind
        else {
            panic!("expected the enum-case constructor");
        };
        assert!(constructor.value_param_clauses.is_empty());
        assert_eq!(
            parser
                .ast()
                .get(constructor.tpt)
                .position
                .expect("synthetic constructor type tree span")
                .span()
                .range(),
            dotty_core::TextRange::new(15, 15).unwrap()
        );
        assert_eq!(
            constructor.metadata.visibility,
            Some(dotty_core::ast::VisibilitySyntax::Private { qualifier: None })
        );
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::DefDef(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn keeps_enum_case_prefix_visibility_separate_from_constructor_visibility() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { protected case C private(x: Int) }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Protected), 9, 18),
                token(TokenKind::Keyword(HardKeyword::Case), 19, 23),
                token(TokenKind::Identifier, 24, 25),
                token(TokenKind::Keyword(HardKeyword::Private), 26, 33),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 33, 34),
                token(TokenKind::Identifier, 34, 35),
                token(TokenKind::ColonFollow, 35, 36),
                token(TokenKind::Identifier, 37, 40),
                token(TokenKind::Punctuation(Punctuation::RightParen), 40, 41),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 42, 43),
                token(TokenKind::Eof, 43, 43),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        let TreeKind::TypeDef(case) = &parser.ast().get(template.body[0]).kind else {
            panic!("expected the enum case TypeDef");
        };
        assert_eq!(
            case.metadata.visibility,
            Some(dotty_core::ast::VisibilitySyntax::Protected { qualifier: None })
        );
        let TreeKind::Template(case_template) = &parser.ast().get(case.rhs).kind else {
            panic!("expected the enum-case Template");
        };
        let TreeKind::DefDef(constructor) = &parser.ast().get(case_template.constructor).kind
        else {
            panic!("expected the enum-case constructor");
        };
        assert_eq!(
            constructor.metadata.visibility,
            Some(dotty_core::ast::VisibilitySyntax::Private { qualifier: None })
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_derives_and_uses_after_singleton_enum_cases() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case A derives Base\ncase B uses Ref }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Identifier, 16, 23),
                token(TokenKind::Identifier, 24, 28),
                token(TokenKind::Newline, 28, 29),
                token(TokenKind::Keyword(HardKeyword::Case), 29, 33),
                token(TokenKind::Identifier, 34, 35),
                token(TokenKind::Identifier, 36, 40),
                token(TokenKind::Identifier, 41, 44),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 45, 46),
                token(TokenKind::Eof, 46, 46),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(template.body.iter().all(|tree| matches!(
            parser.ast().get(*tree).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        )));
        assert_eq!(
            parser
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.message().contains("unsupported enum case"))
                .count(),
            2
        );
    }

    #[test]
    fn rejects_case_local_derives_after_a_parameterized_enum_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case Bad(x: Int) derives Eq\ncase Good }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 17),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 17, 18),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::ColonFollow, 19, 20),
                token(TokenKind::Identifier, 21, 24),
                token(TokenKind::Punctuation(Punctuation::RightParen), 24, 25),
                token(TokenKind::Identifier, 26, 33),
                token(TokenKind::Identifier, 34, 36),
                token(TokenKind::Newline, 36, 37),
                token(TokenKind::Keyword(HardKeyword::Case), 37, 41),
                token(TokenKind::Identifier, 42, 46),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 47, 48),
                token(TokenKind::Eof, 48, 48),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(
            parser.diagnostics()[0]
                .message()
                .contains("unsupported enum case syntax")
        );
    }

    #[test]
    fn rejects_soft_and_deferred_modifiers_after_singleton_enum_cases() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case A inline\ncase B opaque\ndef after = 1 }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Identifier, 16, 22),
                token(TokenKind::Newline, 22, 23),
                token(TokenKind::Keyword(HardKeyword::Case), 23, 27),
                token(TokenKind::Identifier, 28, 29),
                token(TokenKind::Identifier, 30, 36),
                token(TokenKind::Newline, 36, 37),
                token(TokenKind::Keyword(HardKeyword::Def), 37, 40),
                token(TokenKind::Identifier, 41, 46),
                token(TokenKind::Operator, 47, 48),
                token(TokenKind::IntegerLiteral, 49, 50),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 51, 52),
                token(TokenKind::Eof, 52, 52),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.body.len(), 3);
        assert!(template.body[..2].iter().all(|tree| matches!(
            parser.ast().get(*tree).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        )));
        assert!(matches!(
            parser.ast().get(template.body[2]).kind,
            TreeKind::DefDef(_)
        ));
        assert_eq!(
            parser
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.message().contains("unsupported enum case"))
                .count(),
            2
        );
    }

    #[test]
    fn recovers_from_missing_enum_case_names_and_trailing_commas() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { case\ncase Red, ,\ndef after = 1 }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Case), 9, 13),
                token(TokenKind::Newline, 13, 14),
                token(TokenKind::Keyword(HardKeyword::Case), 14, 18),
                token(TokenKind::Identifier, 19, 22),
                token(TokenKind::Punctuation(Punctuation::Comma), 22, 23),
                token(TokenKind::Punctuation(Punctuation::Comma), 24, 25),
                token(TokenKind::Newline, 25, 26),
                token(TokenKind::Keyword(HardKeyword::Def), 26, 29),
                token(TokenKind::Identifier, 30, 35),
                token(TokenKind::Operator, 36, 37),
                token(TokenKind::IntegerLiteral, 38, 39),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 40, 41),
                token(TokenKind::Eof, 41, 41),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        assert_eq!(template.body.len(), 3);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::PatDef(_))
        ));
        assert!(matches!(
            parser.ast().get(template.body[2]).kind,
            TreeKind::DefDef(_)
        ));
        assert_eq!(
            parser
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedPattern)
                .count(),
            2
        );
    }

    #[test]
    fn does_not_treat_case_classes_in_nested_templates_as_enum_cases() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { class Inner { case class C } }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Class), 9, 14),
                token(TokenKind::Identifier, 15, 20),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 21, 22),
                token(TokenKind::CaseClass, 23, 33),
                token(TokenKind::Identifier, 34, 35),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 36, 37),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 38, 39),
                token(TokenKind::Eof, 39, 39),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a TypeDef");
        };
        let TreeKind::Template(enum_template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected the enum template");
        };
        let TreeKind::TypeDef(inner) = &parser.ast().get(enum_template.body[0]).kind else {
            panic!("expected the nested class");
        };
        let TreeKind::Template(inner_template) = &parser.ast().get(inner.rhs).kind else {
            panic!("expected the nested class template");
        };
        let TreeKind::TypeDef(case_class) = &parser.ast().get(inner_template.body[0]).kind else {
            panic!("expected the nested case class");
        };
        assert!(case_class.metadata.modifiers.contains(&Modifier::Case));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn allows_local_case_definitions_inside_an_enum_method_block() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "enum E { def f = { case class Local\ncase object Other } }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Def), 9, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Operator, 15, 16),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 17, 18),
                token(TokenKind::CaseClass, 19, 29),
                token(TokenKind::Identifier, 30, 35),
                token(TokenKind::Newline, 35, 36),
                token(TokenKind::CaseObject, 36, 47),
                token(TokenKind::Identifier, 48, 53),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 54, 55),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 56, 57),
                token(TokenKind::Eof, 57, 57),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected an enum definition");
        };
        let TreeKind::TypeDef(enum_definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(enum_template) = &parser.ast().get(enum_definition.rhs).kind else {
            panic!("expected the enum template");
        };
        let TreeKind::DefDef(method) = &parser.ast().get(enum_template.body[0]).kind else {
            panic!("expected the method definition");
        };
        let TreeKind::Block(body) = &parser.ast().get(method.rhs.expect("method body")).kind else {
            panic!("expected the method body block");
        };
        assert_eq!(body.stats.len(), 2);
        assert!(matches!(
            parser.ast().get(body.stats[0]).kind,
            TreeKind::TypeDef(ref definition)
                if definition.metadata.modifiers.contains(&Modifier::Case)
        ));
        assert!(matches!(
            parser.ast().get(body.stats[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(ref definition))
                if definition.metadata.modifiers.contains(&Modifier::Case)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_qualified_derives_in_template_metadata() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class C derives A, pkg.B",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Identifier, 8, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Punctuation(Punctuation::Comma), 17, 18),
                token(TokenKind::Identifier, 19, 22),
                token(TokenKind::Punctuation(Punctuation::Dot), 22, 23),
                token(TokenKind::Identifier, 23, 24),
                token(TokenKind::Eof, 24, 24),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };

        assert_eq!(template.metadata.derives.len(), 2);
        assert_eq!(
            parser
                .ast()
                .get(definition.rhs)
                .position
                .expect("template span")
                .span()
                .range()
                .start(),
            16
        );
        assert!(matches!(
            parser.ast().get(template.metadata.derives[0]).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        ));
        assert!(matches!(
            parser.ast().get(template.metadata.derives[1]).kind,
            TreeKind::Select(select) if select.name.is_type()
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_type_applications_in_derives_clauses() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class C derives Foo[Bar]",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Identifier, 8, 15),
                token(TokenKind::Identifier, 16, 19),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 19, 20),
                token(TokenKind::Identifier, 20, 23),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 23, 24),
                token(TokenKind::Eof, 24, 24),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };

        assert!(matches!(
            parser.ast().get(template.metadata.derives[0]).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::UnsupportedSyntax
        );
    }

    #[test]
    fn keeps_derives_as_a_qualified_identifier_list() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class C derives Eq | Show",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Identifier, 8, 15),
                token(TokenKind::Identifier, 16, 18),
                token(TokenKind::Operator, 19, 20),
                token(TokenKind::Identifier, 21, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };

        assert_eq!(template.metadata.derives.len(), 1);
        assert!(matches!(
            parser.ast().get(template.metadata.derives[0]).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_following_derives_after_unsupported_infix_type_recovery() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class C derives A | B, C",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Identifier, 8, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Operator, 18, 19),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Punctuation(Punctuation::Comma), 21, 22),
                token(TokenKind::Identifier, 23, 24),
                token(TokenKind::Eof, 24, 24),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };

        assert_eq!(template.metadata.derives.len(), 2);
        assert!(matches!(
            parser.ast().get(template.metadata.derives[0]).kind,
            TreeKind::Ident(ident) if parser.names.resolve(ident.name.text()) == "A"
        ));
        assert!(matches!(
            parser.ast().get(template.metadata.derives[1]).kind,
            TreeKind::Ident(ident) if parser.names.resolve(ident.name.text()) == "C"
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_uses_references_and_initially_markers() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class C uses cap initially, outer.cap",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Identifier, 8, 12),
                token(TokenKind::Identifier, 13, 16),
                token(TokenKind::Identifier, 17, 26),
                token(TokenKind::Punctuation(Punctuation::Comma), 26, 27),
                token(TokenKind::Identifier, 28, 33),
                token(TokenKind::Punctuation(Punctuation::Dot), 33, 34),
                token(TokenKind::Identifier, 34, 37),
                token(TokenKind::Eof, 37, 37),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };

        assert_eq!(template.metadata.uses.len(), 2);
        assert_eq!(
            parser
                .ast()
                .get(definition.rhs)
                .position
                .expect("template span")
                .span()
                .range()
                .end(),
            37
        );
        assert!(template.metadata.uses[0].initially);
        assert!(!template.metadata.uses[1].initially);
        assert!(matches!(
            parser.ast().get(template.metadata.uses[0].reference).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            parser.ast().get(template.metadata.uses[1].reference).kind,
            TreeKind::Select(_)
        ));
        let TreeKind::Select(select) = &parser.ast().get(template.metadata.uses[1].reference).kind
        else {
            panic!("expected qualified capture reference");
        };
        assert_eq!(
            parser
                .ast()
                .get(select.qualifier)
                .position
                .expect("capture qualifier span")
                .span()
                .range(),
            dotty_core::TextRange::new(28, 33).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn includes_initially_marker_in_template_span() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class C uses cap initially",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Identifier, 8, 12),
                token(TokenKind::Identifier, 13, 16),
                token(TokenKind::Identifier, 17, 26),
                token(TokenKind::Eof, 26, 26),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };

        assert!(template.metadata.uses[0].initially);
        assert_eq!(
            parser
                .ast()
                .get(definition.rhs)
                .position
                .expect("template span")
                .span()
                .range()
                .end(),
            26
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn accepts_terminal_this_capture_reference() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class C uses this",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Identifier, 8, 12),
                token(TokenKind::Keyword(HardKeyword::This), 13, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };

        assert!(matches!(
            parser.ast().get(template.metadata.uses[0].reference).kind,
            TreeKind::This(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn accepts_qualified_this_capture_reference() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class C uses Outer.this",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Identifier, 8, 12),
                token(TokenKind::Identifier, 13, 18),
                token(TokenKind::Punctuation(Punctuation::Dot), 18, 19),
                token(TokenKind::Keyword(HardKeyword::This), 19, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };

        let reference = template.metadata.uses[0].reference;
        assert!(
            matches!(parser.ast().get(reference).kind, TreeKind::This(this) if this.qual.is_some())
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn accepts_super_capture_reference() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class C uses super.cap",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Identifier, 8, 12),
                token(TokenKind::Keyword(HardKeyword::Super), 13, 18),
                token(TokenKind::Punctuation(Punctuation::Dot), 18, 19),
                token(TokenKind::Identifier, 19, 22),
                token(TokenKind::Eof, 22, 22),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };

        let reference = template.metadata.uses[0].reference;
        assert!(matches!(
            parser.ast().get(reference).kind,
            TreeKind::Select(select) if matches!(parser.ast().get(select.qualifier).kind, TreeKind::Super(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_case_class_with_case_metadata_and_case_parameter_policy() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case class C(x: A)",
            vec![
                token(TokenKind::CaseClass, 0, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 12, 13),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Punctuation(Punctuation::Colon), 14, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Punctuation(Punctuation::RightParen), 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a case class definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a TypeDef");
        };
        assert_eq!(definition.metadata.modifiers, vec![Modifier::Case]);
        assert_eq!(
            parser
                .ast()
                .get(id)
                .position
                .unwrap()
                .span()
                .range()
                .start(),
            0
        );
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        let TreeKind::DefDef(constructor) = &parser.ast().get(template.constructor).kind else {
            panic!("expected a synthetic constructor");
        };
        let TreeKind::ValDef(parameter) =
            &parser.ast().get(constructor.value_param_clauses[0][0]).kind
        else {
            panic!("expected a constructor parameter");
        };
        assert_eq!(parameter.metadata.modifiers, vec![Modifier::ParamAccessor]);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn keeps_var_on_the_constructor_parameter_only() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case class C(var x: Int)",
            vec![
                token(TokenKind::CaseClass, 0, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 12, 13),
                token(TokenKind::Keyword(HardKeyword::Var), 13, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Punctuation(Punctuation::Colon), 18, 19),
                token(TokenKind::Identifier, 20, 23),
                token(TokenKind::Punctuation(Punctuation::RightParen), 23, 24),
                token(TokenKind::Eof, 24, 24),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a case class definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a TypeDef");
        };
        assert_eq!(definition.metadata.modifiers, vec![Modifier::Case]);

        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        let TreeKind::DefDef(constructor) = &parser.ast().get(template.constructor).kind else {
            panic!("expected a synthetic constructor");
        };
        assert!(constructor.metadata.modifiers.is_empty());

        let TreeKind::ValDef(parameter) =
            &parser.ast().get(constructor.value_param_clauses[0][0]).kind
        else {
            panic!("expected a constructor parameter");
        };
        assert_eq!(
            parameter.metadata.modifiers,
            vec![Modifier::ParamAccessor, Modifier::Var]
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_case_object_as_a_module_with_case_metadata() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case object Empty",
            vec![
                token(TokenKind::CaseObject, 0, 11),
                token(TokenKind::Identifier, 12, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a case object definition");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) = &parser.ast().get(id).kind
        else {
            panic!("expected a ModuleDef");
        };
        assert_eq!(module.metadata.modifiers, vec![Modifier::Case]);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn composes_a_prefix_modifier_with_a_case_class() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "final case class C",
            vec![
                token(TokenKind::Keyword(HardKeyword::Final), 0, 5),
                token(TokenKind::CaseClass, 6, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a case class definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a TypeDef");
        };
        assert_eq!(
            definition.metadata.modifiers,
            vec![Modifier::Final, Modifier::Case]
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn marks_a_trait_without_inferencing_it_from_constructor_shape() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "trait T",
            vec![
                token(TokenKind::Keyword(HardKeyword::Trait), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_trait_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        assert_eq!(definition.metadata.modifiers, vec![Modifier::Trait]);
    }

    #[test]
    fn parses_an_object_as_a_module_def_with_a_template() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "object O {}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Object), 0, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_object_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) = &parser.ast().get(id).kind
        else {
            panic!("expected ModuleDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(module.template).kind else {
            panic!("expected Template");
        };
        assert!(template.body.is_empty());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_class_type_parameters_in_the_synthetic_constructor() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class Box[A, B]",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 9),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::Comma), 11, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 14, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        let TreeKind::DefDef(constructor) = &parser.ast().get(template.constructor).kind else {
            panic!("expected synthetic constructor");
        };
        assert_eq!(constructor.type_params.len(), 2);
        assert!(constructor.value_param_clauses.is_empty());
        assert!(
            constructor
                .type_params
                .iter()
                .all(|param| matches!(parser.ast().get(*param).kind, TreeKind::TypeDef(_)))
        );
        assert_eq!(
            parser
                .ast()
                .get(constructor.tpt)
                .position
                .map(|position| position.span().range()),
            Some(dotty_core::TextRange::new(14, 14).unwrap())
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_multiple_class_constructor_parameter_clauses() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class Pair(x: X)(y: Y)",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 10),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 10, 11),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::Colon), 12, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Punctuation(Punctuation::RightParen), 15, 16),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 16, 17),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Punctuation(Punctuation::Colon), 18, 19),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Punctuation(Punctuation::RightParen), 21, 22),
                token(TokenKind::Eof, 22, 22),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        let TreeKind::DefDef(constructor) = &parser.ast().get(template.constructor).kind else {
            panic!("expected synthetic constructor");
        };
        assert_eq!(constructor.value_param_clauses.len(), 2);
        assert_eq!(constructor.value_param_clauses[0].len(), 1);
        assert_eq!(constructor.value_param_clauses[1].len(), 1);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn spans_a_constructor_from_an_empty_first_parameter_clause() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class C()(value: X)",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Identifier, 10, 15),
                token(TokenKind::Punctuation(Punctuation::Colon), 15, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Punctuation(Punctuation::RightParen), 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a class definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected a Template");
        };
        let constructor = parser.ast().get(template.constructor);
        assert_eq!(
            constructor.position.unwrap().span().range(),
            dotty_core::TextRange::new(7, 19).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_single_parent_in_an_extends_clause() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class Child extends Parent",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 11),
                token(TokenKind::Keyword(HardKeyword::Extends), 12, 19),
                token(TokenKind::Identifier, 20, 26),
                token(TokenKind::Eof, 26, 26),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        assert_eq!(template.parents.len(), 1);
        assert!(matches!(
            parser.ast().get(template.parents[0]).kind,
            TreeKind::Ident(identifier) if identifier.name.is_type()
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_parent_constructor_arguments_and_parent_lists() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class Child extends Parent(x), Other, Mixin",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 11),
                token(TokenKind::Keyword(HardKeyword::Extends), 12, 19),
                token(TokenKind::Identifier, 20, 26),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 26, 27),
                token(TokenKind::Identifier, 27, 28),
                token(TokenKind::Punctuation(Punctuation::RightParen), 28, 29),
                token(TokenKind::Punctuation(Punctuation::Comma), 29, 30),
                token(TokenKind::Identifier, 31, 36),
                token(TokenKind::Punctuation(Punctuation::Comma), 36, 37),
                token(TokenKind::Identifier, 38, 43),
                token(TokenKind::Eof, 43, 43),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        assert_eq!(template.parents.len(), 3);
        assert!(matches!(
            parser.ast().get(template.parents[0]).kind,
            TreeKind::Apply(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn reports_mixed_parent_separators_without_accepting_the_second_mode() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class Child extends Parent(x), Other with Mixin",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 11),
                token(TokenKind::Keyword(HardKeyword::Extends), 12, 19),
                token(TokenKind::Identifier, 20, 26),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 26, 27),
                token(TokenKind::Identifier, 27, 28),
                token(TokenKind::Punctuation(Punctuation::RightParen), 28, 29),
                token(TokenKind::Punctuation(Punctuation::Comma), 29, 30),
                token(TokenKind::Identifier, 31, 36),
                token(TokenKind::Keyword(HardKeyword::With), 37, 41),
                token(TokenKind::Identifier, 42, 47),
                token(TokenKind::Eof, 47, 47),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        assert_eq!(template.parents.len(), 2);
        assert_eq!(parser.current().kind, TokenKind::Keyword(HardKeyword::With));
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| { diagnostic.kind() == ParseDiagnosticKind::UnexpectedToken })
        );
    }

    #[test]
    fn represents_parent_constructor_arguments_as_new_select_apply() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class Child extends Parent(x)",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 11),
                token(TokenKind::Keyword(HardKeyword::Extends), 12, 19),
                token(TokenKind::Identifier, 20, 26),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 26, 27),
                token(TokenKind::Identifier, 27, 28),
                token(TokenKind::Punctuation(Punctuation::RightParen), 28, 29),
                token(TokenKind::Eof, 29, 29),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        let TreeKind::Apply(application) = &parser.ast().get(template.parents[0]).kind else {
            panic!("expected parent constructor application");
        };
        let TreeKind::Select(constructor) = &parser.ast().get(application.function).kind else {
            panic!("expected constructor selection");
        };
        let TreeKind::New(new_tree) = &parser.ast().get(constructor.qualifier).kind else {
            panic!("expected New parent tree");
        };
        assert!(matches!(
            parser.ast().get(new_tree.tpt).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(application.args.len(), 1);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_indented_template_body_after_colon_feedback() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class A:\n  value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::ColonEol, 7, 8),
                token(TokenKind::Indent, 8, 8),
                token(TokenKind::Identifier, 11, 16),
                token(TokenKind::Outdent, 16, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        assert_eq!(template.body.len(), 1);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn nested_class_like_definitions_stay_in_the_outer_template_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class Outer:\n  class Inner\n  object Companion",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 11),
                token(TokenKind::ColonEol, 11, 12),
                token(TokenKind::Indent, 12, 12),
                token(TokenKind::Keyword(HardKeyword::Class), 15, 20),
                token(TokenKind::Identifier, 21, 26),
                token(TokenKind::Newline, 26, 27),
                token(TokenKind::Keyword(HardKeyword::Object), 29, 35),
                token(TokenKind::Identifier, 36, 45),
                token(TokenKind::Outdent, 45, 45),
                token(TokenKind::Eof, 45, 45),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected outer TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected outer Template");
        };
        assert_eq!(template.body.len(), 2);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::TypeDef(_)
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn missing_class_name_recovers_at_eof() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let _ = parser.parse_class_definition(Location::Elsewhere);

        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
    }

    #[test]
    fn malformed_constructor_parameter_recovers_at_the_closing_parenthesis() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class A(x:)",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 7, 8),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Colon), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let _ = parser.parse_class_definition(Location::Elsewhere);

        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn missing_parent_recovers_at_eof() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class A extends",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Keyword(HardKeyword::Extends), 8, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let _ = parser.parse_class_definition(Location::Elsewhere);

        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn missing_template_closer_reports_a_diagnostic_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class A { value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 8, 9),
                token(TokenKind::Identifier, 10, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let _ = parser.parse_class_definition(Location::Elsewhere);

        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn malformed_parent_application_stops_at_the_outer_body_boundary() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class A extends Parent(}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Keyword(HardKeyword::Extends), 8, 15),
                token(TokenKind::Identifier, 16, 22),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 22, 23),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 23, 24),
                token(TokenKind::Eof, 24, 24),
            ],
            &mut names,
        );

        let _ = parser.parse_class_definition(Location::Elsewhere);

        assert_eq!(
            parser.current().kind,
            TokenKind::Punctuation(Punctuation::RightBrace)
        );
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn malformed_parent_type_preserves_a_following_template_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class A extends { value }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Keyword(HardKeyword::Extends), 8, 15),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 16, 17),
                token(TokenKind::Identifier, 18, 23),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 24, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected Template");
        };
        assert_eq!(template.body.len(), 1);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_class_accessor_parameter_clause() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "class A(val x: X)",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 7, 8),
                token(TokenKind::Keyword(HardKeyword::Val), 8, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Punctuation(Punctuation::Colon), 13, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Punctuation(Punctuation::RightParen), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let _ = parser.parse_class_definition(Location::Elsewhere);

        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn nested_indented_case_definitions_stay_in_the_outer_template_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{\n  class Outer:\n    case class Inner(value: A):\n      def get = value\n    case object Empty\n    val done = true\n}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 4, 9),
                token(TokenKind::Identifier, 10, 15),
                token(TokenKind::ColonEol, 15, 16),
                token(TokenKind::Indent, 16, 16),
                token(TokenKind::CaseClass, 21, 31),
                token(TokenKind::Identifier, 32, 37),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 37, 38),
                token(TokenKind::Identifier, 38, 43),
                token(TokenKind::ColonFollow, 43, 44),
                token(TokenKind::Identifier, 45, 46),
                token(TokenKind::Punctuation(Punctuation::RightParen), 46, 47),
                token(TokenKind::ColonEol, 47, 48),
                token(TokenKind::Indent, 48, 48),
                token(TokenKind::Keyword(HardKeyword::Def), 55, 58),
                token(TokenKind::Identifier, 59, 62),
                token(TokenKind::Operator, 63, 64),
                token(TokenKind::Identifier, 65, 69),
                token(TokenKind::Newline, 69, 70),
                token(TokenKind::Outdent, 70, 70),
                token(TokenKind::CaseObject, 75, 86),
                token(TokenKind::Identifier, 87, 92),
                token(TokenKind::Newline, 92, 93),
                token(TokenKind::Keyword(HardKeyword::Val), 97, 100),
                token(TokenKind::Identifier, 101, 105),
                token(TokenKind::Operator, 106, 107),
                token(TokenKind::Keyword(HardKeyword::True), 108, 112),
                token(TokenKind::Outdent, 113, 113),
                token(TokenKind::Eof, 113, 113),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_class_definition(Location::Elsewhere)
        else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected outer TypeDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(definition.rhs).kind else {
            panic!("expected outer Template");
        };
        assert_eq!(template.body.len(), 3);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::TypeDef(_)
        ));
        assert!(matches!(
            parser.ast().get(template.body[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ));
        assert!(matches!(
            parser.ast().get(template.body[2]).kind,
            TreeKind::ValDef(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }
}
