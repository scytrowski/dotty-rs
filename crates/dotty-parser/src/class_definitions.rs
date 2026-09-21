//! Initial source-level class, trait, and object definitions.
//!
//! This module deliberately stops before constructor clauses, parents, and
//! layout bodies.  It establishes the shared `TypeDef`/`Template` and
//! `ModuleDef` shapes so those grammar pieces can be added without changing
//! statement dispatch again.

use dotty_core::ast::{DefDef, Modifier, Modifiers, ModuleDef, Template, TypeDef};
use dotty_core::{HardKeyword, TermName, TokenKind, TreeId, TreeKind, TypeName, Untyped};

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
        let body = self.parse_optional_template_body();
        let constructor = self.synthetic_primary_constructor(mark.start());
        let template = self.alloc_from(
            mark,
            TreeKind::Template(Template {
                constructor,
                parents: Vec::new(),
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
        let body = self.parse_optional_template_body();
        let constructor = self.synthetic_primary_constructor(mark.start());
        let template = self.alloc_from(
            mark,
            TreeKind::Template(Template {
                constructor,
                parents: Vec::new(),
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
        match self.current().kind {
            TokenKind::Punctuation(dotty_core::Punctuation::LeftBrace) => {
                self.parse_template_body(TemplateBody::Braced)
            }
            TokenKind::Indent => self.parse_template_body(TemplateBody::Indented),
            _ => Vec::new(),
        }
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

    fn synthetic_primary_constructor(&mut self, start: u32) -> TreeId<Untyped> {
        let name = TermName::new(self.names.intern("<init>"));
        let tpt = self.synthetic_type_tree_at(start);
        let position = self.zero_width_span(start);
        self.alloc(
            TreeKind::DefDef(DefDef {
                name,
                type_params: Vec::new(),
                value_param_clauses: Vec::new(),
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
}
