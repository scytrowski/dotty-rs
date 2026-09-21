//! Initial source-level class, trait, and object definitions.
//!
//! This module deliberately stops before constructor clauses, parents, and
//! layout bodies.  It establishes the shared `TypeDef`/`Template` and
//! `ModuleDef` shapes so those grammar pieces can be added without changing
//! statement dispatch again.

use dotty_core::ast::{
    Apply, ApplyKind, DefDef, Modifier, Modifiers, ModuleDef, Template, TypeDef,
};
use dotty_core::{
    HardKeyword, Punctuation, TermName, TokenKind, TreeId, TreeKind, TypeName, Untyped,
};

use crate::statements::ParsedStatement;
use crate::templates::TemplateBody;
use crate::{Location, ParseDiagnosticKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(crate) fn parse_class_definition(&mut self, _location: Location) -> ParsedStatement {
        self.parse_type_like_definition(false)
    }

    pub(crate) fn parse_trait_definition(&mut self, _location: Location) -> ParsedStatement {
        self.parse_type_like_definition(true)
    }

    pub(crate) fn parse_object_definition(&mut self, _location: Location) -> ParsedStatement {
        let mark = self.mark();
        self.advance();
        let name = self.parse_object_name();
        let parents = self.parse_parent_clause();
        let body = self.parse_optional_template_body();
        let constructor = self.synthetic_primary_constructor(mark.start(), Vec::new(), Vec::new());
        let template = self.alloc_from(
            mark,
            TreeKind::Template(Template {
                constructor,
                parents,
                self_val: None,
                body,
                metadata: dotty_core::ast::UntypedTemplateMetadata::default(),
            }),
        );

        ParsedStatement::Definition(self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(ModuleDef {
                name,
                template,
            })),
        ))
    }

    fn parse_type_like_definition(&mut self, is_trait: bool) -> ParsedStatement {
        let mark = self.mark();
        self.advance();
        let name = self.parse_type_name();
        let type_params = if self.current().kind
            == TokenKind::Punctuation(dotty_core::Punctuation::LeftBracket)
        {
            self.parse_type_param_clause(crate::ParamOwner::Class)
        } else {
            Vec::new()
        };
        let value_param_clauses = self.parse_term_param_clauses(crate::ParamOwner::Class);
        let parents = self.parse_parent_clause();
        let body = self.parse_optional_template_body();
        let constructor =
            self.synthetic_primary_constructor(mark.start(), type_params, value_param_clauses);
        let template = self.alloc_from(
            mark,
            TreeKind::Template(Template {
                constructor,
                parents,
                self_val: None,
                body,
                metadata: dotty_core::ast::UntypedTemplateMetadata::default(),
            }),
        );
        let mut metadata = Modifiers::default();
        if is_trait {
            metadata.modifiers.push(Modifier::Trait);
        }

        ParsedStatement::Definition(self.alloc_from(
            mark,
            TreeKind::TypeDef(TypeDef {
                name,
                rhs: template,
                metadata,
            }),
        ))
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

    fn parse_parent_clause(&mut self) -> Vec<TreeId<Untyped>> {
        self.consume_newlines_before_parent_keyword();
        if !self.accept(TokenKind::Keyword(HardKeyword::Extends)) {
            return Vec::new();
        }

        let mut parents = vec![self.parse_parent()];
        loop {
            self.consume_newlines_before_parent_separator();
            let separator = self.accept(TokenKind::Punctuation(Punctuation::Comma))
                || self.accept(TokenKind::Keyword(HardKeyword::With));
            if !separator {
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
        follows(self.cursor.lookahead(count).kind)
            .then_some(count)
            .unwrap_or(0)
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
                args.push(self.with_location(crate::Location::InArgs, |parser| parser.expr()));
                if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                    self.expect(TokenKind::Punctuation(Punctuation::RightParen));
                    break;
                }
                if self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
                    break;
                }
            }
        }
        self.alloc_from(
            mark,
            TreeKind::Apply(Apply {
                function: parent,
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
    ) -> TreeId<Untyped> {
        let name = TermName::new(self.names.intern("<init>"));
        let tpt = self.synthetic_type_tree_at(start);
        let position = self.zero_width_span(start);
        self.alloc(
            TreeKind::DefDef(DefDef {
                name,
                type_params,
                value_param_clauses,
                tpt,
                rhs: None,
                metadata: Modifiers::default(),
            }),
            Some(position),
        )
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
        assert_eq!(template.parents.len(), 3);
        assert!(matches!(
            parser.ast().get(template.parents[0]).kind,
            TreeKind::Apply(_)
        ));
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
}
