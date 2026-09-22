//! Source-level class, trait, and object definitions.
//!
//! This module owns the initial definition grammar: constructor clauses,
//! parent applications, and braced or indented template bodies are preserved
//! in the shared `TypeDef`/`Template` and `ModuleDef` shapes. Semantic class
//! and template processing remains outside the parser.

use dotty_core::ast::{
    Apply, ApplyKind, DefDef, Modifier, Modifiers, ModuleDef, New, Select, Template, TypeDef,
    UntypedTemplateMetadata,
};
use dotty_core::{
    HardKeyword, Punctuation, TermName, TokenKind, TreeId, TreeKind, TypeName, Untyped,
};

use crate::modifiers::DefinitionPrefix;
use crate::statements::ParsedStatement;
use crate::templates::TemplateBody;
use crate::{Location, ParseDiagnosticKind, Parser};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParentSeparator {
    Comma,
    With,
}

#[derive(Clone, Copy)]
struct ConstructorBoundary {
    parameter_start: Option<u32>,
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
        self.parse_type_like_definition(false, false, prefix)
    }

    pub(crate) fn parse_case_class_definition(&mut self, _location: Location) -> ParsedStatement {
        self.parse_case_class_definition_with_prefix(DefinitionPrefix::empty(self.mark().start()))
    }

    pub(crate) fn parse_case_class_definition_with_prefix(
        &mut self,
        prefix: DefinitionPrefix,
    ) -> ParsedStatement {
        self.parse_type_like_definition(false, true, prefix)
    }

    pub(crate) fn parse_trait_definition(&mut self, _location: Location) -> ParsedStatement {
        self.parse_trait_definition_with_prefix(DefinitionPrefix::empty(self.mark().start()))
    }

    pub(crate) fn parse_trait_definition_with_prefix(
        &mut self,
        prefix: DefinitionPrefix,
    ) -> ParsedStatement {
        self.parse_type_like_definition(true, false, prefix)
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
        let tail = self.parse_template_tail();
        let parent_start = tail.parents.first().and_then(|parent| {
            self.ast
                .get(*parent)
                .position
                .map(|position| position.span().range().start())
        });
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
                parent_start,
                body_start,
            },
        );
        let template_position =
            self.template_position(constructor, constructor_start, &tail.parents, &tail.body);
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
        if is_case {
            metadata.modifiers.push(Modifier::Case);
        }

        ParsedStatement::Definition(self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(ModuleDef {
                name,
                template,
                metadata,
            })),
        ))
    }

    fn parse_type_like_definition(
        &mut self,
        is_trait: bool,
        is_case: bool,
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
        let tail = self.parse_template_tail();
        let parent_start = tail.parents.first().and_then(|parent| {
            self.ast
                .get(*parent)
                .position
                .map(|position| position.span().range().start())
        });
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
                parent_start,
                body_start,
            },
        );
        let template_position =
            self.template_position(constructor, constructor_start, &tail.parents, &tail.body);
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
        if is_case {
            metadata.modifiers.push(Modifier::Case);
        }
        if is_trait {
            metadata.modifiers.push(Modifier::Trait);
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

    fn parse_template_tail(&mut self) -> TemplateTail {
        let parents = self.parse_parent_clause();
        let derives = self.parse_derives_clause();
        let uses = self.parse_uses_clause();
        let body = self.parse_optional_template_body();
        TemplateTail {
            parents,
            self_val: None,
            body,
            metadata: UntypedTemplateMetadata { derives, uses },
        }
    }

    fn parse_derives_clause(&mut self) -> Vec<TreeId<Untyped>> {
        self.consume_newlines_before_template_name(self.known_names.derives);
        if !self.current_is_template_name(self.known_names.derives) {
            return Vec::new();
        }
        self.advance();

        let mut derives = Vec::new();
        loop {
            let mark = self.mark();
            let mut derive =
                self.with_parse_kind(crate::ParseKind::Type, |parser| parser.simple_type());
            if self.current().kind == TokenKind::Punctuation(Punctuation::LeftBracket) {
                derive = self.parse_type_application(mark, derive);
            }
            derives.push(derive);
            if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                break;
            }
        }
        derives
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
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
                let Ok(name) = self.intern_current_term_name() else {
                    return self.error_expr(self.current_span());
                };
                self.advance();
                if !self.accept(TokenKind::Punctuation(Punctuation::Dot)) {
                    return self.alloc_from(
                        mark,
                        TreeKind::Ident(dotty_core::ast::Ident {
                            name: *name.as_name(),
                            backquoted,
                        }),
                    );
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

                let mut reference = self.alloc_from(
                    mark,
                    TreeKind::Ident(dotty_core::ast::Ident {
                        name: *name.as_name(),
                        backquoted,
                    }),
                );
                reference = self.parse_capture_selection_after_dot(mark, reference);
                reference
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
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `.` in a capture reference",
            );
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
            let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
            let Ok(name) = self.intern_current_term_name() else {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected a name after `.` in a capture reference",
                );
                return qualifier;
            };
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

    fn parse_optional_template_body(&mut self) -> Vec<TreeId<Untyped>> {
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
                return Vec::new();
            }
        }

        match self.current().kind {
            TokenKind::Punctuation(dotty_core::Punctuation::LeftBrace) => {
                self.parse_template_body(TemplateBody::Braced)
            }
            TokenKind::Indent => self.parse_template_body(TemplateBody::Indented),
            _ => Vec::new(),
        }
    }

    fn template_position(
        &self,
        constructor: TreeId<Untyped>,
        start: u32,
        parents: &[TreeId<Untyped>],
        body: &[TreeId<Untyped>],
    ) -> dotty_core::SourceSpan {
        let constructor_range = self.ast.get(constructor).position.map(|position| {
            (
                position.span().range().start(),
                position.span().range().end(),
            )
        });
        let body_start = body.first().and_then(|tree| {
            self.ast
                .get(*tree)
                .position
                .map(|position| position.span().range().start())
        });
        let body_end = body.last().and_then(|tree| {
            self.ast
                .get(*tree)
                .position
                .map(|position| position.span().range().end())
        });
        let parent_end = parents.last().and_then(|tree| {
            self.ast
                .get(*tree)
                .position
                .map(|position| position.span().range().end())
        });
        let template_start = if parents.is_empty()
            && constructor_range.is_some_and(|(constructor_start, constructor_end)| {
                constructor_start == constructor_end
            }) {
            body_start.unwrap_or(start)
        } else {
            start
        };
        let template_end = body_end
            .or(parent_end)
            .or_else(|| constructor_range.map(|(_, end)| end))
            .unwrap_or(template_start);
        dotty_core::SourceSpan::new(
            self.source_id,
            dotty_core::Span::without_point(
                dotty_core::TextRange::new(template_start, template_end)
                    .expect("template span is ordered"),
            ),
        )
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

    fn parse_parent(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let mut parent =
            self.with_parse_kind(crate::ParseKind::Type, |parser| parser.simple_type());
        if self.current().kind == TokenKind::Punctuation(Punctuation::LeftBracket) {
            parent = self.parse_type_application(mark, parent);
        }
        if self.current().kind != TokenKind::Punctuation(Punctuation::LeftParen) {
            return parent;
        }

        self.advance();
        let mut args = Vec::new();
        if !self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
            loop {
                if matches!(
                    self.current().kind,
                    TokenKind::Punctuation(Punctuation::RightBrace)
                        | TokenKind::Outdent
                        | TokenKind::Eof
                ) {
                    self.report(
                        ParseDiagnosticKind::ExpectedExpression,
                        "expected a parent constructor argument",
                    );
                    args.push(self.error_expr(self.current_span()));
                } else {
                    args.push(self.with_location(crate::Location::InArgs, |parser| parser.expr()));
                }
                if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                    self.expect(TokenKind::Punctuation(Punctuation::RightParen));
                    break;
                }
                if self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
                    break;
                }
            }
        }
        let new_tree = self.alloc(
            TreeKind::New(New { tpt: parent }),
            self.ast.get(parent).position,
        );
        let constructor_name = TermName::new(self.names.intern("<init>"));
        let constructor = self.alloc(
            TreeKind::Select(Select {
                qualifier: new_tree,
                name: *constructor_name.as_name(),
                backquoted: false,
            }),
            self.ast.get(new_tree).position,
        );
        self.alloc_from(
            mark,
            TreeKind::Apply(Apply {
                function: constructor,
                args,
                kind: ApplyKind::Regular,
            }),
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
            .unwrap_or(constructor_end);
        let tpt = self.synthetic_type_tree_at(tpt_start);
        let position = if has_constructor_parameters
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
                metadata: Modifiers::default(),
            }),
            Some(position),
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
