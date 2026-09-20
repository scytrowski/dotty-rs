use super::*;
use crate::compilation_unit::tests::{parser_for, token};
use dotty_core::ast::{Block, Literal, New, NumberKind, Parens, Super, This, Tuple, UntypedNode};
use dotty_core::{
    Constant, HardKeyword, NameInterner, ScannerEvent, SourceId, SourceText, TextRange, Token,
    TokenSource,
};

#[test]
fn parses_an_identifier_with_its_source_span() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "x",
        vec![
            token(TokenKind::Identifier, 0, 1),
            token(TokenKind::Eof, 1, 1),
        ],
        &mut names,
    );

    let id = parser.simple_expr();
    let tree = parser.ast().get(id).clone();
    drop(parser);

    let TreeKind::Ident(ident) = tree.kind else {
        panic!("expected identifier tree");
    };
    assert_eq!(names.resolve(ident.name.text()), "x");
    assert_eq!(
        tree.position.unwrap().span().range(),
        TextRange::new(0, 1).unwrap()
    );
    assert!(!ident.backquoted);
}

#[test]
fn postfix_expression_entry_preserves_simple_expression_behavior() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "x",
        vec![
            token(TokenKind::Identifier, 0, 1),
            token(TokenKind::Eof, 1, 1),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(parser.ast().get(id).kind, TreeKind::Ident(_)));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn expression_entry_preserves_operator_expression_behavior() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "a + b",
        vec![
            token(TokenKind::Identifier, 0, 1),
            token(TokenKind::Operator, 2, 3),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Eof, 5, 5),
        ],
        &mut names,
    );

    let id = parser.expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_if_with_then_and_else() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "if c then yes else no",
        vec![
            token(TokenKind::Keyword(HardKeyword::If), 0, 2),
            token(TokenKind::Identifier, 3, 4),
            token(TokenKind::Keyword(HardKeyword::Then), 5, 9),
            token(TokenKind::Identifier, 10, 13),
            token(TokenKind::Keyword(HardKeyword::Else), 14, 18),
            token(TokenKind::Identifier, 19, 21),
            token(TokenKind::Eof, 21, 21),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::If(if_tree) = parser.ast().get(id).kind else {
        panic!("expected if tree");
    };

    assert!(matches!(
        parser.ast().get(if_tree.cond).kind,
        TreeKind::Ident(_)
    ));
    assert!(matches!(
        parser.ast().get(if_tree.then_branch).kind,
        TreeKind::Ident(_)
    ));
    assert!(matches!(
        parser.ast().get(if_tree.else_branch).kind,
        TreeKind::Ident(_)
    ));
    assert_eq!(
        parser.ast().get(id).position.unwrap().span().range(),
        TextRange::new(0, 21).unwrap()
    );
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_if_with_parenthesized_condition() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "if (c) yes else no",
        vec![
            token(TokenKind::Keyword(HardKeyword::If), 0, 2),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
            token(TokenKind::Identifier, 7, 10),
            token(TokenKind::Keyword(HardKeyword::Else), 11, 15),
            token(TokenKind::Identifier, 16, 18),
            token(TokenKind::Eof, 18, 18),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::If(if_tree) = parser.ast().get(id).kind else {
        panic!("expected if tree");
    };

    assert!(matches!(
        parser.ast().get(if_tree.cond).kind,
        TreeKind::PhaseSpecific(UntypedNode::Parens(_))
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_else_after_a_semicolon_separator() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "if c then a; else b",
        vec![
            token(TokenKind::Keyword(HardKeyword::If), 0, 2),
            token(TokenKind::Identifier, 3, 4),
            token(TokenKind::Keyword(HardKeyword::Then), 5, 9),
            token(TokenKind::Identifier, 10, 11),
            token(TokenKind::Punctuation(Punctuation::Semicolon), 11, 12),
            token(TokenKind::Keyword(HardKeyword::Else), 13, 17),
            token(TokenKind::Identifier, 18, 19),
            token(TokenKind::Eof, 19, 19),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::If(if_tree) = parser.ast().get(id).kind else {
        panic!("expected if tree");
    };

    assert!(matches!(
        parser.ast().get(if_tree.else_branch).kind,
        TreeKind::Ident(_)
    ));
    assert_eq!(
        parser
            .ast()
            .get(if_tree.then_branch)
            .position
            .unwrap()
            .span()
            .range(),
        TextRange::new(10, 12).unwrap()
    );
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_else_after_a_separator_in_parenthesized_if() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "if (c) a; else b",
        vec![
            token(TokenKind::Keyword(HardKeyword::If), 0, 2),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
            token(TokenKind::Identifier, 7, 8),
            token(TokenKind::Punctuation(Punctuation::Semicolon), 8, 9),
            token(TokenKind::Keyword(HardKeyword::Else), 10, 14),
            token(TokenKind::Identifier, 15, 16),
            token(TokenKind::Eof, 16, 16),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::If(if_tree) = parser.ast().get(id).kind else {
        panic!("expected if tree");
    };

    assert!(matches!(
        parser.ast().get(if_tree.else_branch).kind,
        TreeKind::Ident(_)
    ));
    assert_eq!(
        parser
            .ast()
            .get(if_tree.then_branch)
            .position
            .unwrap()
            .span()
            .range(),
        TextRange::new(7, 9).unwrap()
    );
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn leaves_a_non_else_statement_separator_unconsumed() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "if c then a\nnext",
        vec![
            token(TokenKind::Keyword(HardKeyword::If), 0, 2),
            token(TokenKind::Identifier, 3, 4),
            token(TokenKind::Keyword(HardKeyword::Then), 5, 9),
            token(TokenKind::Identifier, 10, 11),
            token(TokenKind::Newline, 11, 12),
            token(TokenKind::Identifier, 12, 16),
            token(TokenKind::Eof, 16, 16),
        ],
        &mut names,
    );

    let _ = parser.expr();

    assert_eq!(parser.current().kind, TokenKind::Newline);
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_an_infix_if_condition() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "if a + b > c then x else y",
        vec![
            token(TokenKind::Keyword(HardKeyword::If), 0, 2),
            token(TokenKind::Identifier, 3, 4),
            token(TokenKind::Operator, 5, 6),
            token(TokenKind::Identifier, 7, 8),
            token(TokenKind::Operator, 9, 10),
            token(TokenKind::Identifier, 11, 12),
            token(TokenKind::Keyword(HardKeyword::Then), 13, 17),
            token(TokenKind::Identifier, 18, 19),
            token(TokenKind::Keyword(HardKeyword::Else), 20, 24),
            token(TokenKind::Identifier, 25, 26),
            token(TokenKind::Eof, 26, 26),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::If(if_tree) = parser.ast().get(id).kind else {
        panic!("expected if tree");
    };

    assert!(matches!(
        parser.ast().get(if_tree.cond).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_while_with_do_and_a_body() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "while c do body",
        vec![
            token(TokenKind::Keyword(HardKeyword::While), 0, 5),
            token(TokenKind::Identifier, 6, 7),
            token(TokenKind::Keyword(HardKeyword::Do), 8, 10),
            token(TokenKind::Identifier, 11, 15),
            token(TokenKind::Eof, 15, 15),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::While(while_tree) = parser.ast().get(id).kind else {
        panic!("expected while tree");
    };

    assert!(matches!(
        parser.ast().get(while_tree.cond).kind,
        TreeKind::Ident(_)
    ));
    assert!(matches!(
        parser.ast().get(while_tree.body).kind,
        TreeKind::Ident(_)
    ));
    assert_eq!(
        parser.ast().get(id).position.unwrap().span().range(),
        TextRange::new(0, 15).unwrap()
    );
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_while_with_a_parenthesized_condition() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "while (c) body",
        vec![
            token(TokenKind::Keyword(HardKeyword::While), 0, 5),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 6, 7),
            token(TokenKind::Identifier, 7, 8),
            token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
            token(TokenKind::Identifier, 10, 14),
            token(TokenKind::Eof, 14, 14),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::While(while_tree) = parser.ast().get(id).kind else {
        panic!("expected while tree");
    };

    assert!(matches!(
        parser.ast().get(while_tree.cond).kind,
        TreeKind::PhaseSpecific(UntypedNode::Parens(_))
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_while_with_an_infix_condition() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "while i < n do step",
        vec![
            token(TokenKind::Keyword(HardKeyword::While), 0, 5),
            token(TokenKind::Identifier, 6, 7),
            token(TokenKind::Operator, 8, 9),
            token(TokenKind::Identifier, 10, 11),
            token(TokenKind::Keyword(HardKeyword::Do), 12, 14),
            token(TokenKind::Identifier, 15, 19),
            token(TokenKind::Eof, 19, 19),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::While(while_tree) = parser.ast().get(id).kind else {
        panic!("expected while tree");
    };

    assert!(matches!(
        parser.ast().get(while_tree.cond).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_while_with_do_after_a_parenthesized_condition() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "while (c) do body",
        vec![
            token(TokenKind::Keyword(HardKeyword::While), 0, 5),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 6, 7),
            token(TokenKind::Identifier, 7, 8),
            token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
            token(TokenKind::Keyword(HardKeyword::Do), 10, 12),
            token(TokenKind::Identifier, 13, 17),
            token(TokenKind::Eof, 17, 17),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::While(while_tree) = parser.ast().get(id).kind else {
        panic!("expected while tree");
    };

    assert!(matches!(
        parser.ast().get(while_tree.body).kind,
        TreeKind::Ident(_)
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_a_single_indented_if_branch_as_its_expression() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "if c then\n  yes\nelse\n  no",
        vec![
            token(TokenKind::Keyword(HardKeyword::If), 0, 2),
            token(TokenKind::Identifier, 3, 4),
            token(TokenKind::Keyword(HardKeyword::Then), 5, 9),
            token(TokenKind::Indent, 12, 12),
            token(TokenKind::Identifier, 12, 15),
            token(TokenKind::Outdent, 16, 16),
            token(TokenKind::Keyword(HardKeyword::Else), 16, 20),
            token(TokenKind::Indent, 23, 23),
            token(TokenKind::Identifier, 23, 25),
            token(TokenKind::Outdent, 25, 25),
            token(TokenKind::Eof, 25, 25),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::If(if_tree) = parser.ast().get(id).kind else {
        panic!("expected if tree");
    };

    assert!(matches!(
        parser.ast().get(if_tree.then_branch).kind,
        TreeKind::Ident(_)
    ));
    assert!(matches!(
        parser.ast().get(if_tree.else_branch).kind,
        TreeKind::Ident(_)
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_multiple_expressions_in_an_indented_while_body() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "while c do\n  step\n  next",
        vec![
            token(TokenKind::Keyword(HardKeyword::While), 0, 5),
            token(TokenKind::Identifier, 6, 7),
            token(TokenKind::Keyword(HardKeyword::Do), 8, 10),
            token(TokenKind::Indent, 13, 13),
            token(TokenKind::Identifier, 13, 17),
            token(TokenKind::Newline, 17, 18),
            token(TokenKind::Identifier, 20, 24),
            token(TokenKind::Outdent, 24, 24),
            token(TokenKind::Eof, 24, 24),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::While(while_tree) = parser.ast().get(id).kind else {
        panic!("expected while tree");
    };
    let TreeKind::Block(ref block) = parser.ast().get(while_tree.body).kind else {
        panic!("expected an indented block body");
    };

    assert_eq!(block.stats.len(), 1);
    assert!(matches!(
        parser.ast().get(block.stats[0]).kind,
        TreeKind::Ident(_)
    ));
    assert!(matches!(
        parser.ast().get(block.expr).kind,
        TreeKind::Ident(_)
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_if_without_else_as_a_synthetic_unit() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "if c then yes",
        vec![
            token(TokenKind::Keyword(HardKeyword::If), 0, 2),
            token(TokenKind::Identifier, 3, 4),
            token(TokenKind::Keyword(HardKeyword::Then), 5, 9),
            token(TokenKind::Identifier, 10, 13),
            token(TokenKind::Eof, 13, 13),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::If(if_tree) = parser.ast().get(id).kind else {
        panic!("expected if tree");
    };

    assert!(matches!(
        parser.ast().get(if_tree.else_branch).kind,
        TreeKind::Literal(dotty_core::ast::Literal {
            value: Constant::Unit
        })
    ));
    assert_eq!(
        parser
            .ast()
            .get(if_tree.else_branch)
            .position
            .unwrap()
            .span()
            .range(),
        TextRange::new(13, 13).unwrap()
    );
    assert_eq!(
        parser.ast().get(id).position.unwrap().span().range(),
        TextRange::new(0, 13).unwrap()
    );
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn attaches_nested_if_else_to_the_nearest_if() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "if a then if b then x else y else z",
        vec![
            token(TokenKind::Keyword(HardKeyword::If), 0, 2),
            token(TokenKind::Identifier, 3, 4),
            token(TokenKind::Keyword(HardKeyword::Then), 5, 9),
            token(TokenKind::Keyword(HardKeyword::If), 10, 12),
            token(TokenKind::Identifier, 13, 14),
            token(TokenKind::Keyword(HardKeyword::Then), 15, 19),
            token(TokenKind::Identifier, 20, 21),
            token(TokenKind::Keyword(HardKeyword::Else), 22, 26),
            token(TokenKind::Identifier, 27, 28),
            token(TokenKind::Keyword(HardKeyword::Else), 29, 33),
            token(TokenKind::Identifier, 34, 35),
            token(TokenKind::Eof, 35, 35),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::If(outer) = parser.ast().get(id).kind else {
        panic!("expected outer if tree");
    };
    let TreeKind::If(inner) = parser.ast().get(outer.then_branch).kind else {
        panic!("expected nested if tree");
    };

    assert!(matches!(
        parser.ast().get(inner.else_branch).kind,
        TreeKind::Ident(_)
    ));
    assert!(matches!(
        parser.ast().get(outer.else_branch).kind,
        TreeKind::Ident(_)
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_identifier_assignment() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "x = y",
        vec![
            token(TokenKind::Identifier, 0, 1),
            token(TokenKind::Operator, 2, 3),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Eof, 5, 5),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::Assign(assignment) = parser.ast().get(id).kind else {
        panic!("expected assignment tree");
    };

    assert!(matches!(
        parser.ast().get(assignment.lhs).kind,
        TreeKind::Ident(_)
    ));
    assert!(matches!(
        parser.ast().get(assignment.rhs).kind,
        TreeKind::Ident(_)
    ));
    assert_eq!(
        parser.ast().get(id).position.unwrap().span().range(),
        TextRange::new(0, 5).unwrap()
    );
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_selection_assignment() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "obj.x = y",
        vec![
            token(TokenKind::Identifier, 0, 3),
            token(TokenKind::Punctuation(Punctuation::Dot), 3, 4),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Operator, 6, 7),
            token(TokenKind::Identifier, 8, 9),
            token(TokenKind::Eof, 9, 9),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::Assign(assignment) = parser.ast().get(id).kind else {
        panic!("expected assignment tree");
    };

    assert!(matches!(
        parser.ast().get(assignment.lhs).kind,
        TreeKind::Select(_)
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_application_assignment() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "arr(i) = y",
        vec![
            token(TokenKind::Identifier, 0, 3),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
            token(TokenKind::Operator, 7, 8),
            token(TokenKind::Identifier, 9, 10),
            token(TokenKind::Eof, 10, 10),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::Assign(assignment) = parser.ast().get(id).kind else {
        panic!("expected assignment tree");
    };

    assert!(matches!(
        parser.ast().get(assignment.lhs).kind,
        TreeKind::Apply(_)
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn normalizes_a_bare_identifier_assignment_to_a_named_argument() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "foo(x = 1)",
        vec![
            token(TokenKind::Identifier, 0, 3),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Operator, 6, 7),
            token(TokenKind::IntegerLiteral, 8, 9),
            token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
            token(TokenKind::Eof, 10, 10),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::Apply(ref application) = parser.ast().get(id).kind else {
        panic!("expected application tree");
    };
    let TreeKind::NamedArg(named) = parser.ast().get(application.args[0]).kind else {
        panic!("expected named argument tree");
    };
    let name = named.name;

    assert!(parser.diagnostics().is_empty());
    drop(parser);
    assert_eq!(names.resolve(name.text()), "x");
}

#[test]
fn named_argument_keeps_a_full_expression_rhs() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "foo(x = a + b)",
        vec![
            token(TokenKind::Identifier, 0, 3),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Operator, 6, 7),
            token(TokenKind::Identifier, 8, 9),
            token(TokenKind::Operator, 10, 11),
            token(TokenKind::Identifier, 12, 13),
            token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
            token(TokenKind::Eof, 14, 14),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::Apply(ref application) = parser.ast().get(id).kind else {
        panic!("expected application tree");
    };
    let TreeKind::NamedArg(named) = parser.ast().get(application.args[0]).kind else {
        panic!("expected named argument tree");
    };

    assert!(matches!(
        parser.ast().get(named.arg).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn normalizes_multiple_named_arguments_in_order() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "foo(x = 1, y = 2)",
        vec![
            token(TokenKind::Identifier, 0, 3),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Operator, 6, 7),
            token(TokenKind::IntegerLiteral, 8, 9),
            token(TokenKind::Punctuation(Punctuation::Comma), 9, 10),
            token(TokenKind::Identifier, 11, 12),
            token(TokenKind::Operator, 13, 14),
            token(TokenKind::IntegerLiteral, 15, 16),
            token(TokenKind::Punctuation(Punctuation::RightParen), 16, 17),
            token(TokenKind::Eof, 17, 17),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::Apply(ref application) = parser.ast().get(id).kind else {
        panic!("expected application tree");
    };
    let names_in_args: Vec<_> = application
        .args
        .iter()
        .map(|arg| match parser.ast().get(*arg).kind {
            TreeKind::NamedArg(named) => named.name,
            _ => panic!("expected named argument tree"),
        })
        .collect();

    assert!(parser.diagnostics().is_empty());
    drop(parser);
    assert_eq!(
        names_in_args
            .iter()
            .map(|name| names.resolve(name.text()))
            .collect::<Vec<_>>(),
        vec!["x", "y"]
    );
}

#[test]
fn does_not_normalize_a_selection_assignment_to_a_named_argument() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "foo(obj.x = 1)",
        vec![
            token(TokenKind::Identifier, 0, 3),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
            token(TokenKind::Identifier, 4, 7),
            token(TokenKind::Punctuation(Punctuation::Dot), 7, 8),
            token(TokenKind::Identifier, 8, 9),
            token(TokenKind::Operator, 10, 11),
            token(TokenKind::IntegerLiteral, 12, 13),
            token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
            token(TokenKind::Eof, 14, 14),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::Apply(ref application) = parser.ast().get(id).kind else {
        panic!("expected application tree");
    };

    assert!(matches!(
        parser.ast().get(application.args[0]).kind,
        TreeKind::Assign(_)
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn assignment_rhs_is_a_full_right_associative_expression() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "a = b = c",
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

    let id = parser.expr();
    let TreeKind::Assign(outer) = parser.ast().get(id).kind else {
        panic!("expected outer assignment tree");
    };
    assert!(matches!(
        parser.ast().get(outer.rhs).kind,
        TreeKind::Assign(_)
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn assignment_rhs_preserves_operator_precedence() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "x = a + b * c",
        vec![
            token(TokenKind::Identifier, 0, 1),
            token(TokenKind::Operator, 2, 3),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Operator, 6, 7),
            token(TokenKind::Identifier, 8, 9),
            token(TokenKind::Operator, 10, 11),
            token(TokenKind::Identifier, 12, 13),
            token(TokenKind::Eof, 13, 13),
        ],
        &mut names,
    );

    let id = parser.expr();
    let TreeKind::Assign(assignment) = parser.ast().get(id).kind else {
        panic!("expected assignment tree");
    };
    let TreeKind::PhaseSpecific(UntypedNode::InfixOp(outer)) =
        parser.ast().get(assignment.rhs).kind
    else {
        panic!("expected infix right-hand side");
    };
    assert!(matches!(
        parser.ast().get(outer.right).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn rejects_a_literal_assignment_target() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "1 = x",
        vec![
            token(TokenKind::IntegerLiteral, 0, 1),
            token(TokenKind::Operator, 2, 3),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Eof, 5, 5),
        ],
        &mut names,
    );

    let id = parser.expr();

    assert!(!matches!(parser.ast().get(id).kind, TreeKind::Assign(_)));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn rejects_an_infix_assignment_target() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "a + b = c",
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

    let id = parser.expr();

    assert!(!matches!(parser.ast().get(id).kind, TreeKind::Assign(_)));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_assignment_without_a_rhs() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "x =",
        vec![
            token(TokenKind::Identifier, 0, 1),
            token(TokenKind::Operator, 2, 3),
            token(TokenKind::Eof, 3, 3),
        ],
        &mut names,
    );

    let _ = parser.expr();

    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_an_if_without_a_condition() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "if",
        vec![
            token(TokenKind::Keyword(HardKeyword::If), 0, 2),
            token(TokenKind::Eof, 2, 2),
        ],
        &mut names,
    );

    let _ = parser.expr();

    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_an_if_without_a_then_branch() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "if c then",
        vec![
            token(TokenKind::Keyword(HardKeyword::If), 0, 2),
            token(TokenKind::Identifier, 3, 4),
            token(TokenKind::Keyword(HardKeyword::Then), 5, 9),
            token(TokenKind::Eof, 9, 9),
        ],
        &mut names,
    );

    let _ = parser.expr();

    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_while_without_a_body() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "while c do",
        vec![
            token(TokenKind::Keyword(HardKeyword::While), 0, 5),
            token(TokenKind::Identifier, 6, 7),
            token(TokenKind::Keyword(HardKeyword::Do), 8, 10),
            token(TokenKind::Eof, 10, 10),
        ],
        &mut names,
    );

    let _ = parser.expr();

    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_an_indented_body_without_an_outdent() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "while c do\n  step",
        vec![
            token(TokenKind::Keyword(HardKeyword::While), 0, 5),
            token(TokenKind::Identifier, 6, 7),
            token(TokenKind::Keyword(HardKeyword::Do), 8, 10),
            token(TokenKind::Indent, 13, 13),
            token(TokenKind::Identifier, 13, 17),
            token(TokenKind::Eof, 17, 17),
        ],
        &mut names,
    );

    let _ = parser.expr();

    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

fn assert_prefix_operator_parses(source: &str, operator: &str) {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        source,
        vec![
            token(TokenKind::Operator, 0, operator.len() as u32),
            token(
                TokenKind::Identifier,
                operator.len() as u32,
                source.len() as u32,
            ),
            token(TokenKind::Eof, source.len() as u32, source.len() as u32),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();
    let (operator_name, operand_is_ident) = {
        let TreeKind::PhaseSpecific(UntypedNode::PrefixOp(prefix)) = parser.ast().get(id).kind
        else {
            panic!("expected prefix operator tree");
        };
        (
            prefix.op,
            matches!(parser.ast().get(prefix.operand).kind, TreeKind::Ident(_)),
        )
    };
    assert!(operand_is_ident);
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(parser.diagnostics().is_empty());
    assert_eq!(
        parser.ast().get(id).position.unwrap().span().range(),
        TextRange::new(0, 2).unwrap()
    );
    drop(parser);
    assert_eq!(names.resolve(operator_name.text()), operator);
}

#[test]
fn parses_minus_as_a_prefix_operator() {
    assert_prefix_operator_parses("-x", "-");
}

#[test]
fn parses_plus_as_a_prefix_operator() {
    assert_prefix_operator_parses("+x", "+");
}

#[test]
fn parses_bang_as_a_prefix_operator() {
    assert_prefix_operator_parses("!x", "!");
}

#[test]
fn parses_tilde_as_a_prefix_operator() {
    assert_prefix_operator_parses("~x", "~");
}

#[test]
fn rejects_a_second_prefix_operator_as_a_nested_prefix() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "!!x",
        vec![
            token(TokenKind::Operator, 0, 1),
            token(TokenKind::Operator, 1, 2),
            token(TokenKind::Identifier, 2, 3),
            token(TokenKind::Eof, 3, 3),
        ],
        &mut names,
    );

    let _id = parser.postfix_expr();

    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn rejects_a_prefix_operator_whose_operand_starts_on_the_next_line() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "-\nx",
        vec![
            token(TokenKind::Operator, 0, 1),
            token(TokenKind::Newline, 1, 2),
            token(TokenKind::Identifier, 2, 3),
            token(TokenKind::Eof, 3, 3),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::PhaseSpecific(_)
    ));
    assert_eq!(parser.current().kind, TokenKind::Newline);
    assert_eq!(parser.diagnostics().len(), 1);
    assert_eq!(
        parser.diagnostics()[0].kind(),
        crate::ParseDiagnosticKind::ExpectedExpression
    );
}

#[test]
fn preserves_the_sign_in_a_negated_integer_literal() {
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

    let id = parser.postfix_expr();
    let tree = parser.ast().get(id).clone();
    drop(parser);

    let TreeKind::PhaseSpecific(UntypedNode::Number(number)) = tree.kind else {
        panic!("expected signed number tree");
    };
    assert_eq!(number.kind, NumberKind::Whole(10));
    assert_eq!(names.resolve(number.text), "-42");
}

#[test]
fn decodes_a_negated_long_literal_as_a_negative_constant() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "-1L",
        vec![
            token(TokenKind::Operator, 0, 1),
            token(TokenKind::LongLiteral, 1, 3),
            token(TokenKind::Eof, 3, 3),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::Literal(Literal {
            value: Constant::Long(-1)
        })
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn decodes_decimal_long_min_value() {
    let mut names = NameInterner::new();
    let source = "-9223372036854775808L";
    let mut parser = parser_for(
        source,
        vec![
            token(TokenKind::Operator, 0, 1),
            token(TokenKind::LongLiteral, 1, source.len() as u32),
            token(TokenKind::Eof, source.len() as u32, source.len() as u32),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::Literal(Literal {
            value: Constant::Long(value)
        }) if value == i64::MIN
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn decodes_hexadecimal_long_min_value() {
    let mut names = NameInterner::new();
    let source = "-0x8000000000000000L";
    let mut parser = parser_for(
        source,
        vec![
            token(TokenKind::Operator, 0, 1),
            token(TokenKind::LongLiteral, 1, source.len() as u32),
            token(TokenKind::Eof, source.len() as u32, source.len() as u32),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::Literal(Literal {
            value: Constant::Long(value)
        }) if value == i64::MIN
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn preserves_the_sign_in_a_negated_decimal_literal() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "-1.5",
        vec![
            token(TokenKind::Operator, 0, 1),
            token(TokenKind::DecimalLiteral, 1, 4),
            token(TokenKind::Eof, 4, 4),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();
    let tree = parser.ast().get(id).clone();
    drop(parser);

    let TreeKind::PhaseSpecific(UntypedNode::Number(number)) = tree.kind else {
        panic!("expected signed decimal tree");
    };
    assert_eq!(number.kind, NumberKind::Decimal);
    assert_eq!(names.resolve(number.text), "-1.5");
}

#[test]
fn decodes_a_negated_float_literal_as_a_negative_constant() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "-1.5f",
        vec![
            token(TokenKind::Operator, 0, 1),
            token(TokenKind::FloatLiteral, 1, 5),
            token(TokenKind::Eof, 5, 5),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::Literal(Literal {
            ref value
        }) if *value == Constant::float(-1.5)
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn decodes_a_negated_double_literal_as_a_negative_constant() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "-1.5d",
        vec![
            token(TokenKind::Operator, 0, 1),
            token(TokenKind::DoubleLiteral, 1, 5),
            token(TokenKind::Eof, 5, 5),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::Literal(Literal {
            ref value
        }) if *value == Constant::double(-1.5)
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn decodes_a_spaced_negated_long_literal() {
    let mut names = NameInterner::new();
    let source = "- 1L";
    let mut parser = parser_for(
        source,
        vec![
            token(TokenKind::Operator, 0, 1),
            token(TokenKind::LongLiteral, 2, source.len() as u32),
            token(TokenKind::Eof, source.len() as u32, source.len() as u32),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::Literal(Literal {
            value: Constant::Long(value)
        }) if value == -1
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn decodes_a_spaced_negated_float_literal() {
    let mut names = NameInterner::new();
    let source = "- 1.0f";
    let mut parser = parser_for(
        source,
        vec![
            token(TokenKind::Operator, 0, 1),
            token(TokenKind::FloatLiteral, 2, source.len() as u32),
            token(TokenKind::Eof, source.len() as u32, source.len() as u32),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::Literal(Literal {
            ref value
        }) if *value == Constant::float(-1.0)
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn decodes_a_spaced_negated_double_literal() {
    let mut names = NameInterner::new();
    let source = "- 1.0d";
    let mut parser = parser_for(
        source,
        vec![
            token(TokenKind::Operator, 0, 1),
            token(TokenKind::DoubleLiteral, 2, source.len() as u32),
            token(TokenKind::Eof, source.len() as u32, source.len() as u32),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::Literal(Literal {
            ref value
        }) if *value == Constant::double(-1.0)
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_a_simple_infix_expression() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "a + b",
        vec![
            token(TokenKind::Identifier, 0, 1),
            token(TokenKind::Operator, 2, 3),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Eof, 5, 5),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();
    let (left, operator, right, span) = {
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) = parser.ast().get(id).kind else {
            panic!("expected infix tree");
        };
        (
            infix.left,
            infix.op,
            infix.right,
            parser.ast().get(id).position.unwrap().span().range(),
        )
    };

    assert!(matches!(parser.ast().get(left).kind, TreeKind::Ident(_)));
    assert!(matches!(parser.ast().get(right).kind, TreeKind::Ident(_)));
    assert_eq!(span, TextRange::new(0, 5).unwrap());
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(parser.diagnostics().is_empty());
    drop(parser);
    assert_eq!(names.resolve(operator.text()), "+");
}

#[test]
fn reduces_multiplication_before_addition() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "a + b * c",
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

    let id = parser.postfix_expr();
    let TreeKind::PhaseSpecific(UntypedNode::InfixOp(outer)) = parser.ast().get(id).kind else {
        panic!("expected outer infix tree");
    };
    let TreeKind::PhaseSpecific(UntypedNode::InfixOp(inner)) = parser.ast().get(outer.right).kind
    else {
        panic!("expected multiplication on the right");
    };

    assert!(matches!(
        parser.ast().get(outer.left).kind,
        TreeKind::Ident(_)
    ));
    assert!(matches!(
        parser.ast().get(inner.left).kind,
        TreeKind::Ident(_)
    ));
    assert!(matches!(
        parser.ast().get(inner.right).kind,
        TreeKind::Ident(_)
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn reduces_colon_operators_right_associatively() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "a :: b :: c",
        vec![
            token(TokenKind::Identifier, 0, 1),
            token(TokenKind::ColonOp, 2, 4),
            token(TokenKind::Identifier, 5, 6),
            token(TokenKind::ColonOp, 7, 9),
            token(TokenKind::Identifier, 10, 11),
            token(TokenKind::Eof, 11, 11),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();
    let TreeKind::PhaseSpecific(UntypedNode::InfixOp(outer)) = parser.ast().get(id).kind else {
        panic!("expected outer infix tree");
    };
    let TreeKind::PhaseSpecific(UntypedNode::InfixOp(inner)) = parser.ast().get(outer.right).kind
    else {
        panic!("expected right-associated inner tree");
    };

    assert!(matches!(
        parser.ast().get(outer.left).kind,
        TreeKind::Ident(_)
    ));
    assert!(matches!(
        parser.ast().get(inner.left).kind,
        TreeKind::Ident(_)
    ));
    assert!(matches!(
        parser.ast().get(inner.right).kind,
        TreeKind::Ident(_)
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_an_alphabetic_infix_operator() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "a foo b",
        vec![
            token(TokenKind::Identifier, 0, 1),
            token(TokenKind::Identifier, 2, 5),
            token(TokenKind::Identifier, 6, 7),
            token(TokenKind::Eof, 7, 7),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();
    let TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) = parser.ast().get(id).kind else {
        panic!("expected alphabetic infix tree");
    };
    let operator = infix.op;

    assert!(parser.diagnostics().is_empty());
    drop(parser);
    assert_eq!(names.resolve(operator.text()), "foo");
}

#[test]
fn consumes_a_statement_newline_after_an_infix_operator() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "a +\nb",
        vec![
            token(TokenKind::Identifier, 0, 1),
            token(TokenKind::Operator, 2, 3),
            token(TokenKind::Newline, 3, 4),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Eof, 5, 5),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn keeps_a_newline_before_an_infix_operator_as_a_statement_boundary() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "a\n+ b",
        vec![
            token(TokenKind::Identifier, 0, 1),
            token(TokenKind::Newline, 1, 2),
            token(TokenKind::Operator, 2, 3),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Eof, 5, 5),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(parser.ast().get(id).kind, TreeKind::Ident(_)));
    assert_eq!(parser.current().kind, TokenKind::Newline);
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn reports_mixed_associativity_at_equal_precedence() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "a + b +: c",
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

    let id = parser.postfix_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
    assert_eq!(parser.diagnostics().len(), 1);
    assert_eq!(
        parser.diagnostics()[0].kind(),
        crate::ParseDiagnosticKind::UnexpectedToken
    );
}

#[test]
fn parses_a_postfix_operator_when_the_feature_is_enabled() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "xs reverse",
        vec![
            token(TokenKind::Identifier, 0, 2),
            token(TokenKind::Identifier, 3, 10),
            token(TokenKind::Eof, 10, 10),
        ],
        &mut names,
    )
    .with_features(crate::ParserFeatures {
        postfix_ops: true,
        ..crate::ParserFeatures::default()
    });

    let id = parser.postfix_expr();
    let TreeKind::PhaseSpecific(UntypedNode::PostfixOp(postfix)) = parser.ast().get(id).kind else {
        panic!("expected postfix tree");
    };
    let operator = postfix.op;

    assert!(matches!(
        parser.ast().get(postfix.operand).kind,
        TreeKind::Ident(_)
    ));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(parser.diagnostics().is_empty());
    assert_eq!(
        parser.ast().get(id).position.unwrap().span().range(),
        TextRange::new(0, 10).unwrap()
    );
    drop(parser);
    assert_eq!(names.resolve(operator.text()), "reverse");
}

#[test]
fn rejects_a_postfix_operator_when_the_feature_is_disabled() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "xs reverse",
        vec![
            token(TokenKind::Identifier, 0, 2),
            token(TokenKind::Identifier, 3, 10),
            token(TokenKind::Eof, 10, 10),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(!matches!(
        parser.ast().get(id).kind,
        TreeKind::PhaseSpecific(UntypedNode::PostfixOp(_))
    ));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_an_infix_operator_without_an_operand() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "a +",
        vec![
            token(TokenKind::Identifier, 0, 1),
            token(TokenKind::Operator, 2, 3),
            token(TokenKind::Eof, 3, 3),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_an_infix_operator_before_an_invalid_operand() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "a + *",
        vec![
            token(TokenKind::Identifier, 0, 1),
            token(TokenKind::Operator, 2, 3),
            token(TokenKind::Operator, 4, 5),
            token(TokenKind::Eof, 5, 5),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_an_infix_operator_before_a_closing_parenthesis() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "a + )",
        vec![
            token(TokenKind::Identifier, 0, 1),
            token(TokenKind::Operator, 2, 3),
            token(TokenKind::Punctuation(Punctuation::RightParen), 4, 5),
            token(TokenKind::Eof, 5, 5),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_an_infix_operator_before_an_opening_parenthesis() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "a + (",
        vec![
            token(TokenKind::Identifier, 0, 1),
            token(TokenKind::Operator, 2, 3),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 4, 5),
            token(TokenKind::Eof, 5, 5),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_an_incomplete_prefix_expression() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "!",
        vec![
            token(TokenKind::Operator, 0, 1),
            token(TokenKind::Eof, 1, 1),
        ],
        &mut names,
    );

    let _id = parser.postfix_expr();

    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_an_infix_expression_inside_an_application() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "foo(a + )",
        vec![
            token(TokenKind::Identifier, 0, 3),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Operator, 6, 7),
            token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
            token(TokenKind::Eof, 9, 9),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(parser.ast().get(id).kind, TreeKind::Apply(_)));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_an_infix_expression_inside_a_block() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "{ a + }",
        vec![
            token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
            token(TokenKind::Identifier, 2, 3),
            token(TokenKind::Operator, 4, 5),
            token(TokenKind::Punctuation(Punctuation::RightBrace), 6, 7),
            token(TokenKind::Eof, 7, 7),
        ],
        &mut names,
    );

    let id = parser.postfix_expr();

    assert!(matches!(parser.ast().get(id).kind, TreeKind::Block(_)));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn reports_progress_failure_for_a_stuck_infix_token_source() {
    struct StuckTokenSource {
        current: Token,
    }

    impl TokenSource for StuckTokenSource {
        fn current(&self) -> &Token {
            &self.current
        }

        fn position(&self) -> usize {
            0
        }

        fn advance(&mut self) {}

        fn lookahead(&mut self, _n: usize) -> &Token {
            &self.current
        }

        fn observe(&mut self, _event: ScannerEvent) {}
    }

    let mut names = NameInterner::new();
    let token = token(TokenKind::Operator, 0, 1);
    let mut parser = Parser::new(
        SourceText::new("+").unwrap(),
        SourceId::from_index(1),
        StuckTokenSource { current: token },
        &mut names,
    );

    let _id = parser.postfix_expr();

    assert_eq!(parser.current().kind, TokenKind::Operator);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn parses_a_backquoted_identifier_without_its_delimiters() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "`x`",
        vec![
            token(TokenKind::BackquotedIdentifier, 0, 3),
            token(TokenKind::Eof, 3, 3),
        ],
        &mut names,
    );

    let id = parser.simple_expr();
    let TreeKind::Ident(ident) = parser.ast().get(id).kind else {
        panic!("expected identifier tree");
    };

    assert!(ident.backquoted);
    assert_eq!(names.resolve(ident.name.text()), "x");
}

#[test]
fn parses_this_as_a_this_tree() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "this",
        vec![
            token(TokenKind::Keyword(HardKeyword::This), 0, 4),
            token(TokenKind::Eof, 4, 4),
        ],
        &mut names,
    );

    let id = parser.simple_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::This(This { qual: None })
    ));
}

#[test]
fn parses_parenthesized_expression_as_a_parens_node() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "(x)",
        vec![
            token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
            token(TokenKind::Identifier, 1, 2),
            token(TokenKind::Punctuation(Punctuation::RightParen), 2, 3),
            token(TokenKind::Eof, 3, 3),
        ],
        &mut names,
    );

    let id = parser.simple_expr();

    let TreeKind::PhaseSpecific(UntypedNode::Parens(Parens { inner })) = parser.ast().get(id).kind
    else {
        panic!("expected parens tree");
    };
    assert!(matches!(parser.ast().get(inner).kind, TreeKind::Ident(_)));
}

#[test]
fn parses_empty_parentheses_as_unit() {
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

    let id = parser.simple_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::Literal(Literal {
            value: Constant::Unit
        })
    ));
}

#[test]
fn parses_a_tuple_with_all_element_trees() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "(a, b)",
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

    let id = parser.simple_expr();

    let TreeKind::PhaseSpecific(UntypedNode::Tuple(Tuple { ref elements })) =
        parser.ast().get(id).kind
    else {
        panic!("expected tuple tree");
    };
    assert_eq!(elements.len(), 2);
    assert!(matches!(
        parser.ast().get(elements[0]).kind,
        TreeKind::Ident(_)
    ));
    assert!(matches!(
        parser.ast().get(elements[1]).kind,
        TreeKind::Ident(_)
    ));
}

#[test]
fn parses_a_simple_selection() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "foo.bar",
        vec![
            token(TokenKind::Identifier, 0, 3),
            token(TokenKind::Punctuation(Punctuation::Dot), 3, 4),
            token(TokenKind::Identifier, 4, 7),
            token(TokenKind::Eof, 7, 7),
        ],
        &mut names,
    );

    let id = parser.simple_expr();

    let (name, qualifier) = match parser.ast().get(id).kind {
        TreeKind::Select(selection) => (selection.name, selection.qualifier),
        _ => panic!("expected selection tree"),
    };
    let qualifier_is_ident = matches!(parser.ast().get(qualifier).kind, TreeKind::Ident(_));
    assert!(parser.diagnostics().is_empty());
    drop(parser);

    assert_eq!(names.resolve(name.text()), "bar");
    assert!(qualifier_is_ident);
}

#[test]
fn parses_a_backquoted_selection_without_its_delimiters() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "foo.`bar`",
        vec![
            token(TokenKind::Identifier, 0, 3),
            token(TokenKind::Punctuation(Punctuation::Dot), 3, 4),
            token(TokenKind::BackquotedIdentifier, 4, 9),
            token(TokenKind::Eof, 9, 9),
        ],
        &mut names,
    );

    let id = parser.simple_expr();
    let TreeKind::Select(selection) = parser.ast().get(id).kind else {
        panic!("expected selection tree");
    };

    assert!(selection.backquoted);
    assert_eq!(names.resolve(selection.name.text()), "bar");
}

#[test]
fn parses_super_selection_with_a_source_span() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "super.foo",
        vec![
            token(TokenKind::Keyword(HardKeyword::Super), 0, 5),
            token(TokenKind::Punctuation(Punctuation::Dot), 5, 6),
            token(TokenKind::Identifier, 6, 9),
            token(TokenKind::Eof, 9, 9),
        ],
        &mut names,
    );

    let id = parser.simple_expr();
    let TreeKind::Select(selection) = parser.ast().get(id).kind else {
        panic!("expected super selection tree");
    };
    let TreeKind::Super(Super { qual, mix }) = parser.ast().get(selection.qualifier).kind else {
        panic!("expected super qualifier");
    };

    assert!(mix.is_none());
    assert!(matches!(
        parser.ast().get(qual).kind,
        TreeKind::This(This { qual: None })
    ));
    assert_eq!(
        parser.ast().get(id).position.unwrap().span().range(),
        TextRange::new(0, 9).unwrap()
    );
    assert!(parser.diagnostics().is_empty());
    let selected_name = selection.name;
    drop(parser);
    assert_eq!(names.resolve(selected_name.text()), "foo");
}

#[test]
fn parses_qualified_super_with_a_mixin_qualifier() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "Outer.super[Base].foo",
        vec![
            token(TokenKind::Identifier, 0, 5),
            token(TokenKind::Punctuation(Punctuation::Dot), 5, 6),
            token(TokenKind::Keyword(HardKeyword::Super), 6, 11),
            token(TokenKind::Punctuation(Punctuation::LeftBracket), 11, 12),
            token(TokenKind::Identifier, 12, 16),
            token(TokenKind::Punctuation(Punctuation::RightBracket), 16, 17),
            token(TokenKind::Punctuation(Punctuation::Dot), 17, 18),
            token(TokenKind::Identifier, 18, 21),
            token(TokenKind::Eof, 21, 21),
        ],
        &mut names,
    );

    let id = parser.simple_expr();
    let TreeKind::Select(selection) = parser.ast().get(id).kind else {
        panic!("expected qualified super selection tree");
    };
    let TreeKind::Super(Super { qual, mix }) = parser.ast().get(selection.qualifier).kind else {
        panic!("expected qualified super tree");
    };
    let TreeKind::This(This { qual: Some(outer) }) = parser.ast().get(qual).kind else {
        panic!("expected qualified this tree");
    };

    assert!(outer.is_type());
    assert!(parser.diagnostics().is_empty());
    let outer_name = outer;
    let mix_name = mix.unwrap();
    let selected_name = selection.name;
    drop(parser);
    assert_eq!(names.resolve(outer_name.text()), "Outer");
    assert_eq!(names.resolve(mix_name.text()), "Base");
    assert_eq!(names.resolve(selected_name.text()), "foo");
}

#[test]
fn rejects_super_without_a_selector() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "super",
        vec![
            token(TokenKind::Keyword(HardKeyword::Super), 0, 5),
            token(TokenKind::Eof, 5, 5),
        ],
        &mut names,
    );

    let id = parser.simple_expr();

    assert!(matches!(parser.ast().get(id).kind, TreeKind::Super(_)));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn rejects_qualified_super_without_a_selector() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "Outer.super",
        vec![
            token(TokenKind::Identifier, 0, 5),
            token(TokenKind::Punctuation(Punctuation::Dot), 5, 6),
            token(TokenKind::Keyword(HardKeyword::Super), 6, 11),
            token(TokenKind::Eof, 11, 11),
        ],
        &mut names,
    );

    let id = parser.simple_expr();

    assert!(matches!(parser.ast().get(id).kind, TreeKind::Super(_)));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn rejects_mixin_qualified_super_without_a_selector() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "super[Base]",
        vec![
            token(TokenKind::Keyword(HardKeyword::Super), 0, 5),
            token(TokenKind::Punctuation(Punctuation::LeftBracket), 5, 6),
            token(TokenKind::Identifier, 6, 10),
            token(TokenKind::Punctuation(Punctuation::RightBracket), 10, 11),
            token(TokenKind::Eof, 11, 11),
        ],
        &mut names,
    );

    let id = parser.simple_expr();

    assert!(matches!(parser.ast().get(id).kind, TreeKind::Super(_)));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn parses_new_with_a_simple_type() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "new Foo",
        vec![
            token(TokenKind::Keyword(HardKeyword::New), 0, 3),
            token(TokenKind::Identifier, 4, 7),
            token(TokenKind::Eof, 7, 7),
        ],
        &mut names,
    );

    let id = parser.simple_expr();
    let TreeKind::Apply(application) = &parser.ast().get(id).kind else {
        panic!("expected implicit constructor application");
    };
    let TreeKind::Select(selection) = parser.ast().get(application.function).kind else {
        panic!("expected constructor selection");
    };
    let init_name = selection.name;
    let TreeKind::New(New { tpt }) = parser.ast().get(selection.qualifier).kind else {
        panic!("expected new tree");
    };
    let TreeKind::Ident(ident) = parser.ast().get(tpt).kind else {
        panic!("expected constructed type");
    };

    assert!(ident.name.is_type());
    assert_eq!(
        parser.ast().get(id).position.unwrap().span().range(),
        TextRange::new(0, 7).unwrap()
    );
    assert!(parser.diagnostics().is_empty());
    drop(parser);
    assert_eq!(names.resolve(init_name.text()), "<init>");
}

#[test]
fn parses_new_qualified_type_and_constructor_application() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "new foo.Bar(1)",
        vec![
            token(TokenKind::Keyword(HardKeyword::New), 0, 3),
            token(TokenKind::Identifier, 4, 7),
            token(TokenKind::Punctuation(Punctuation::Dot), 7, 8),
            token(TokenKind::Identifier, 8, 11),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 11, 12),
            token(TokenKind::IntegerLiteral, 12, 13),
            token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
            token(TokenKind::Eof, 14, 14),
        ],
        &mut names,
    );

    let id = parser.simple_expr();
    let TreeKind::Apply(ref application) = parser.ast().get(id).kind else {
        panic!("expected constructor application");
    };
    let TreeKind::Select(selection) = parser.ast().get(application.function).kind else {
        panic!("expected constructor selection");
    };
    let TreeKind::New(New { tpt }) = parser.ast().get(selection.qualifier).kind else {
        panic!("expected new function");
    };
    let TreeKind::Select(selection) = parser.ast().get(tpt).kind else {
        panic!("expected qualified constructed type");
    };

    assert!(selection.name.is_type());
    assert_eq!(application.args.len(), 1);
    assert_eq!(
        parser.ast().get(id).position.unwrap().span().range(),
        TextRange::new(0, 14).unwrap()
    );
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn rejects_a_second_application_after_a_new_constructor() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "new Foo(1)(2)",
        vec![
            token(TokenKind::Keyword(HardKeyword::New), 0, 3),
            token(TokenKind::Identifier, 4, 7),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 7, 8),
            token(TokenKind::IntegerLiteral, 8, 9),
            token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 10, 11),
            token(TokenKind::IntegerLiteral, 11, 12),
            token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
            token(TokenKind::Eof, 13, 13),
        ],
        &mut names,
    );

    let id = parser.simple_expr();

    let TreeKind::Apply(ref application) = parser.ast().get(id).kind else {
        panic!("expected the constructor application");
    };
    assert_eq!(application.args.len(), 1);
    assert_eq!(
        parser.current().kind,
        TokenKind::Punctuation(Punctuation::LeftParen)
    );
    assert_eq!(parser.diagnostics().len(), 1);
    assert_eq!(
        parser.diagnostics()[0].message(),
        "a constructor application cannot be applied again"
    );
}

#[test]
fn parses_a_type_application_with_multiple_arguments() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "foo[A, B]",
        vec![
            token(TokenKind::Identifier, 0, 3),
            token(TokenKind::Punctuation(Punctuation::LeftBracket), 3, 4),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
            token(TokenKind::Identifier, 7, 8),
            token(TokenKind::Punctuation(Punctuation::RightBracket), 8, 9),
            token(TokenKind::Eof, 9, 9),
        ],
        &mut names,
    );

    let id = parser.simple_expr();
    let TreeKind::TypeApply(type_apply) = &parser.ast().get(id).kind else {
        panic!("expected type application");
    };

    assert_eq!(type_apply.args.len(), 2);
    assert!(type_apply.args.iter().all(
        |arg| matches!(parser.ast().get(*arg).kind, TreeKind::Ident(ident) if ident.name.is_type())
    ));
    assert_eq!(
        parser.ast().get(id).position.unwrap().span().range(),
        TextRange::new(0, 9).unwrap()
    );
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn chains_type_application_application_and_selection() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "foo[A](1).bar",
        vec![
            token(TokenKind::Identifier, 0, 3),
            token(TokenKind::Punctuation(Punctuation::LeftBracket), 3, 4),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Punctuation(Punctuation::RightBracket), 5, 6),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 6, 7),
            token(TokenKind::IntegerLiteral, 7, 8),
            token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
            token(TokenKind::Punctuation(Punctuation::Dot), 9, 10),
            token(TokenKind::Identifier, 10, 13),
            token(TokenKind::Eof, 13, 13),
        ],
        &mut names,
    );

    let id = parser.simple_expr();
    let TreeKind::Select(selection) = &parser.ast().get(id).kind else {
        panic!("expected final selection");
    };
    let TreeKind::Apply(application) = &parser.ast().get(selection.qualifier).kind else {
        panic!("expected application before selection");
    };
    assert!(matches!(
        parser.ast().get(application.function).kind,
        TreeKind::TypeApply(_)
    ));
    assert_eq!(
        parser.ast().get(id).position.unwrap().span().range(),
        TextRange::new(0, 13).unwrap()
    );
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_type_application_on_new_before_constructor_application() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "new Foo[Int](1)",
        vec![
            token(TokenKind::Keyword(HardKeyword::New), 0, 3),
            token(TokenKind::Identifier, 4, 7),
            token(TokenKind::Punctuation(Punctuation::LeftBracket), 7, 8),
            token(TokenKind::Identifier, 8, 11),
            token(TokenKind::Punctuation(Punctuation::RightBracket), 11, 12),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 12, 13),
            token(TokenKind::IntegerLiteral, 13, 14),
            token(TokenKind::Punctuation(Punctuation::RightParen), 14, 15),
            token(TokenKind::Eof, 15, 15),
        ],
        &mut names,
    );

    let id = parser.simple_expr();
    let TreeKind::Apply(application) = &parser.ast().get(id).kind else {
        panic!("expected constructor application");
    };
    let TreeKind::Select(selection) = parser.ast().get(application.function).kind else {
        panic!("expected constructor selection");
    };
    let TreeKind::New(New { tpt }) = parser.ast().get(selection.qualifier).kind else {
        panic!("expected new tree");
    };
    assert!(matches!(parser.ast().get(tpt).kind, TreeKind::TypeApply(_)));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_an_empty_block_as_a_unit_expression() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "{}",
        vec![
            token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
            token(TokenKind::Punctuation(Punctuation::RightBrace), 1, 2),
            token(TokenKind::Eof, 2, 2),
        ],
        &mut names,
    );

    let id = parser.simple_expr();
    let TreeKind::Block(Block { ref stats, expr }) = parser.ast().get(id).kind else {
        panic!("expected block tree");
    };

    assert!(stats.is_empty());
    assert!(matches!(
        parser.ast().get(expr).kind,
        TreeKind::Literal(Literal {
            value: Constant::Unit
        })
    ));
    assert_eq!(
        parser.ast().get(expr).position.unwrap().span().range(),
        TextRange::new(1, 1).unwrap()
    );
    assert_eq!(
        parser.ast().get(id).position.unwrap().span().range(),
        TextRange::new(0, 2).unwrap()
    );
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_a_block_with_one_expression() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "{ x }",
        vec![
            token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
            token(TokenKind::Identifier, 2, 3),
            token(TokenKind::Punctuation(Punctuation::RightBrace), 4, 5),
            token(TokenKind::Eof, 5, 5),
        ],
        &mut names,
    );

    let id = parser.simple_expr();
    let TreeKind::Block(Block { ref stats, expr }) = parser.ast().get(id).kind else {
        panic!("expected block tree");
    };

    assert!(stats.is_empty());
    assert!(matches!(parser.ast().get(expr).kind, TreeKind::Ident(_)));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_block_statements_and_keeps_the_last_as_expr() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "{ x; y }",
        vec![
            token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
            token(TokenKind::Identifier, 2, 3),
            token(TokenKind::Punctuation(Punctuation::Semicolon), 3, 4),
            token(TokenKind::Identifier, 5, 6),
            token(TokenKind::Punctuation(Punctuation::RightBrace), 7, 8),
            token(TokenKind::Eof, 8, 8),
        ],
        &mut names,
    );

    let id = parser.simple_expr();
    let TreeKind::Block(Block { ref stats, expr }) = parser.ast().get(id).kind else {
        panic!("expected block tree");
    };

    assert_eq!(stats.len(), 1);
    assert!(matches!(
        parser.ast().get(stats[0]).kind,
        TreeKind::Ident(_)
    ));
    assert!(matches!(parser.ast().get(expr).kind, TreeKind::Ident(_)));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_multiline_block_statements() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "{\n x\n y\n}",
        vec![
            token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
            token(TokenKind::Newline, 1, 2),
            token(TokenKind::Identifier, 3, 4),
            token(TokenKind::Newline, 4, 5),
            token(TokenKind::Identifier, 6, 7),
            token(TokenKind::Newline, 7, 8),
            token(TokenKind::Punctuation(Punctuation::RightBrace), 8, 9),
            token(TokenKind::Eof, 9, 9),
        ],
        &mut names,
    );

    let id = parser.simple_expr();
    let TreeKind::Block(Block { ref stats, expr }) = parser.ast().get(id).kind else {
        panic!("expected block tree");
    };

    assert_eq!(stats.len(), 1);
    assert!(matches!(
        parser.ast().get(stats[0]).kind,
        TreeKind::Ident(_)
    ));
    assert!(matches!(parser.ast().get(expr).kind, TreeKind::Ident(_)));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn rejects_application_of_a_block_expression() {
    let mut names = NameInterner::new();
    let parser = parser_for(
        "{ x }(y)",
        vec![
            token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
            token(TokenKind::Identifier, 2, 3),
            token(TokenKind::Punctuation(Punctuation::RightBrace), 4, 5),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 5, 6),
            token(TokenKind::Identifier, 6, 7),
            token(TokenKind::Punctuation(Punctuation::RightParen), 7, 8),
            token(TokenKind::Eof, 8, 8),
        ],
        &mut names,
    );

    let result = parser.compilation_unit();
    let TreeKind::Block(Block { expr, .. }) = result.ast.get(result.root).kind else {
        panic!("expected compilation-unit block root");
    };

    assert!(matches!(result.ast.get(expr).kind, TreeKind::Block(_)));
    assert!(!result.diagnostics.is_empty());
}

#[test]
fn allows_application_after_selecting_from_a_block_expression() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "{ f }.foo(1)",
        vec![
            token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
            token(TokenKind::Identifier, 2, 3),
            token(TokenKind::Punctuation(Punctuation::RightBrace), 4, 5),
            token(TokenKind::Punctuation(Punctuation::Dot), 5, 6),
            token(TokenKind::Identifier, 6, 9),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
            token(TokenKind::IntegerLiteral, 10, 11),
            token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
            token(TokenKind::Eof, 12, 12),
        ],
        &mut names,
    );

    let id = parser.simple_expr();

    let TreeKind::Apply(application) = &parser.ast().get(id).kind else {
        panic!("expected application tree");
    };
    assert_eq!(application.args.len(), 1);
    let TreeKind::Select(selection) = &parser.ast().get(application.function).kind else {
        panic!("expected selection tree");
    };
    assert!(matches!(
        parser.ast().get(selection.qualifier).kind,
        TreeKind::Block(_)
    ));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_an_incomplete_new_expression() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "new",
        vec![
            token(TokenKind::Keyword(HardKeyword::New), 0, 3),
            token(TokenKind::Eof, 3, 3),
        ],
        &mut names,
    );

    let id = parser.simple_expr();

    assert!(matches!(parser.ast().get(id).kind, TreeKind::Apply(_)));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_an_incomplete_type_application() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "foo[",
        vec![
            token(TokenKind::Identifier, 0, 3),
            token(TokenKind::Punctuation(Punctuation::LeftBracket), 3, 4),
            token(TokenKind::Eof, 4, 4),
        ],
        &mut names,
    );

    let id = parser.simple_expr();

    assert!(matches!(parser.ast().get(id).kind, TreeKind::TypeApply(_)));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_a_type_application_without_a_closing_bracket() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "foo[A",
        vec![
            token(TokenKind::Identifier, 0, 3),
            token(TokenKind::Punctuation(Punctuation::LeftBracket), 3, 4),
            token(TokenKind::Identifier, 4, 5),
            token(TokenKind::Eof, 5, 5),
        ],
        &mut names,
    );

    let id = parser.simple_expr();

    assert!(matches!(parser.ast().get(id).kind, TreeKind::TypeApply(_)));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_a_block_without_a_closing_brace() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "{ x",
        vec![
            token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
            token(TokenKind::Identifier, 2, 3),
            token(TokenKind::Eof, 3, 3),
        ],
        &mut names,
    );

    let id = parser.simple_expr();

    assert!(matches!(parser.ast().get(id).kind, TreeKind::Block(_)));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_an_application_without_a_closing_parenthesis() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "foo(",
        vec![
            token(TokenKind::Identifier, 0, 3),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
            token(TokenKind::Eof, 4, 4),
        ],
        &mut names,
    );

    let id = parser.simple_expr();

    assert!(matches!(parser.ast().get(id).kind, TreeKind::Apply(_)));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn recovers_from_a_super_without_a_type_qualifier() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "super[",
        vec![
            token(TokenKind::Keyword(HardKeyword::Super), 0, 5),
            token(TokenKind::Punctuation(Punctuation::LeftBracket), 5, 6),
            token(TokenKind::Eof, 6, 6),
        ],
        &mut names,
    );

    let id = parser.simple_expr();

    assert!(matches!(parser.ast().get(id).kind, TreeKind::Super(_)));
    assert_eq!(parser.current().kind, TokenKind::Eof);
    assert!(!parser.diagnostics().is_empty());
}

#[test]
fn parses_a_simple_application_with_an_argument() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "foo(42)",
        vec![
            token(TokenKind::Identifier, 0, 3),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
            token(TokenKind::IntegerLiteral, 4, 6),
            token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
            token(TokenKind::Eof, 7, 7),
        ],
        &mut names,
    );

    let id = parser.simple_expr();

    let TreeKind::Apply(application) = &parser.ast().get(id).kind else {
        panic!("expected application tree");
    };
    assert_eq!(application.args.len(), 1);
    assert!(matches!(
        parser.ast().get(application.function).kind,
        TreeKind::Ident(_)
    ));
    assert!(matches!(
        parser.ast().get(application.args[0]).kind,
        TreeKind::PhaseSpecific(UntypedNode::Number(_))
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_an_empty_application_with_no_arguments() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "foo()",
        vec![
            token(TokenKind::Identifier, 0, 3),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
            token(TokenKind::Punctuation(Punctuation::RightParen), 4, 5),
            token(TokenKind::Eof, 5, 5),
        ],
        &mut names,
    );

    let id = parser.simple_expr();

    let TreeKind::Apply(application) = &parser.ast().get(id).kind else {
        panic!("expected application tree");
    };
    assert!(application.args.is_empty());
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn parses_an_application_with_multiple_arguments_in_order() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "foo(1, 2)",
        vec![
            token(TokenKind::Identifier, 0, 3),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
            token(TokenKind::IntegerLiteral, 4, 5),
            token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
            token(TokenKind::IntegerLiteral, 7, 8),
            token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
            token(TokenKind::Eof, 9, 9),
        ],
        &mut names,
    );

    let id = parser.simple_expr();

    let TreeKind::Apply(application) = &parser.ast().get(id).kind else {
        panic!("expected application tree");
    };
    assert_eq!(application.args.len(), 2);
    assert!(matches!(
        parser.ast().get(application.args[0]).kind,
        TreeKind::PhaseSpecific(UntypedNode::Number(_))
    ));
    assert!(matches!(
        parser.ast().get(application.args[1]).kind,
        TreeKind::PhaseSpecific(UntypedNode::Number(_))
    ));
    assert!(parser.diagnostics().is_empty());
}

#[test]
fn unsupported_expression_input_produces_an_error_tree_and_diagnostic() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "@",
        vec![token(TokenKind::Error, 0, 1), token(TokenKind::Eof, 1, 1)],
        &mut names,
    );

    let id = parser.simple_expr();

    assert!(matches!(
        parser.ast().get(id).kind,
        TreeKind::PhaseSpecific(UntypedNode::Error(_))
    ));
    assert_eq!(parser.diagnostics().len(), 1);
}

#[test]
fn wraps_an_infix_placeholder_expression_in_a_function() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "_ + 1",
        vec![
            token(TokenKind::Identifier, 0, 1),
            Token {
                kind: TokenKind::Operator,
                span: TextRange::new(2, 3).unwrap(),
                value: dotty_core::TokenValue::None,
            },
            token(TokenKind::IntegerLiteral, 4, 5),
            token(TokenKind::Eof, 5, 5),
        ],
        &mut names,
    );

    let tree = parser.expr();
    let TreeKind::PhaseSpecific(UntypedNode::Function(function)) = &parser.ast().get(tree).kind
    else {
        panic!("expected a placeholder function");
    };
    assert_eq!(function.params.len(), 1);
    assert!(matches!(
        parser.ast().get(function.body).kind,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
    ));
    assert_eq!(
        parser.ast().get(tree).position.unwrap().span().range(),
        TextRange::new(0, 5).unwrap()
    );
    assert!(parser.placeholder_params.is_empty());
}

#[test]
fn leaves_a_bare_placeholder_unwrapped_until_its_parent_expression_finishes() {
    let mut names = NameInterner::new();
    let mut parser = parser_for(
        "_",
        vec![
            token(TokenKind::Identifier, 0, 1),
            token(TokenKind::Eof, 1, 1),
        ],
        &mut names,
    );

    let tree = parser.expr();
    assert!(matches!(parser.ast().get(tree).kind, TreeKind::Ident(_)));
    assert_eq!(parser.placeholder_params.len(), 1);
}
