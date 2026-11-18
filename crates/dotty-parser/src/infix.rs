use dotty_core::ast::{InfixOp, UntypedNode};
use dotty_core::token::characters::{is_operator_character, is_scala_letter};
use dotty_core::{
    Name, Namespace, SourceSpan, Span, TextRange, TokenSource, TreeId, TreeKind, Untyped,
};

use crate::{ExpressionIssue, ParseIssue, ParseKind, Parser, PatternIssue, TypeIssue};

/// Operand/operator state used by Dotty-style infix reduction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpInfo {
    pub operand: TreeId<Untyped>,
    pub operator: Name,
    pub offset: u32,
}

/// Returns the Scala 3 precedence bucket for a term operator.
pub fn precedence(operator: &str) -> u8 {
    precedence_in(operator, Namespace::Term)
}

fn precedence_in(operator: &str, namespace: Namespace) -> u8 {
    if namespace == Namespace::Term && is_assignment_operator(operator) {
        return 0;
    }

    match operator.chars().next() {
        Some(first) if is_scala_letter(first) => 1,
        Some('|') => 2,
        Some('^') => 3,
        Some('&') => 4,
        Some('=' | '!') => 5,
        Some('<' | '>') => 6,
        Some(':') => 7,
        Some('+' | '-') => 8,
        Some('*' | '/' | '%') => 9,
        Some(_) | None => 10,
    }
}

/// Returns whether an operator associates to the right.
pub fn is_right_associative(operator: &str) -> bool {
    operator.ends_with(':')
}

/// Returns whether two equal-precedence operators mix associativity.
fn has_mixed_associativity(left: &str, right: &str, namespace: Namespace) -> bool {
    is_right_associative(left) != is_right_associative(right)
        && precedence_in(left, namespace) == precedence_in(right, namespace)
}

/// Returns whether `operator` is a Scala assignment operator.
pub fn is_assignment_operator(operator: &str) -> bool {
    operator.ends_with('=')
        && !operator.starts_with('=')
        && !matches!(operator, "!=" | "<=" | ">=")
        && operator.chars().next().is_some_and(is_operator_character)
}

impl<S: TokenSource> Parser<'_, '_, S> {
    /// Reduces pending operators that bind before the current operator, then saves its left operand.
    pub(crate) fn push_infix_operator(
        &mut self,
        operators: &mut Vec<OpInfo>,
        mut left: TreeId<Untyped>,
        operator: Name,
        kind: ParseKind,
    ) {
        let spelling = self.names.resolve(operator.text());
        let namespace = operator.namespace();
        let incoming_precedence = precedence_in(spelling, namespace);
        let left_associative = !is_right_associative(spelling);

        // Dotty checks associativity against the initial stack top before any reductions.
        if let Some(previous) = operators.last()
            && has_mixed_associativity(
                self.names.resolve(previous.operator.text()),
                spelling,
                namespace,
            )
        {
            let left = previous.operator;
            let right = operator;
            let issue = match kind {
                ParseKind::Expr => {
                    ParseIssue::Expression(ExpressionIssue::MixedAssociativityOperators {
                        left,
                        right,
                    })
                }
                ParseKind::Type => {
                    ParseIssue::Type(TypeIssue::MixedAssociativityTypeOperators { left, right })
                }
                ParseKind::Pattern => {
                    ParseIssue::Pattern(PatternIssue::MixedAssociativityOperators { left, right })
                }
            };
            self.report_issue(issue);
        }

        while let Some(previous) = operators.pop_if(|previous| {
            let pending_precedence =
                precedence_in(self.names.resolve(previous.operator.text()), namespace);
            pending_precedence > incoming_precedence
                || left_associative && pending_precedence == incoming_precedence
        }) {
            left = self.alloc_infix(previous, left);
        }
        operators.push(OpInfo {
            operand: left,
            operator,
            offset: self.current().span.start(),
        });
    }

    pub(crate) fn finish_infix_operators(
        &mut self,
        operators: Vec<OpInfo>,
        mut right: TreeId<Untyped>,
    ) -> TreeId<Untyped> {
        for previous in operators.into_iter().rev() {
            right = self.alloc_infix(previous, right);
        }
        right
    }

    fn alloc_infix(&mut self, pending: OpInfo, right: TreeId<Untyped>) -> TreeId<Untyped> {
        let left = pending.operand;
        let start = self
            .ast
            .get(left)
            .position
            .map(|position| position.span().range().start())
            .unwrap_or_else(|| self.mark().start());
        let end = self
            .ast
            .get(right)
            .position
            .map(|position| position.span().range().end())
            .unwrap_or(self.last_real_token_end);
        let range = TextRange::new(start, end).expect("infix child spans are ordered");
        self.alloc(
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(InfixOp {
                left,
                op: pending.operator,
                right,
            })),
            Some(SourceSpan::new(self.source_id, Span::without_point(range))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::ast::Ident;
    use dotty_core::{AstArena, NameInterner, Tree, TreeKind};

    #[test]
    fn letter_operators_have_the_lowest_non_assignment_precedence() {
        for op in ["foo", "foo=", "_op", "$op", "Ⅳ"] {
            assert_eq!(precedence(op), 1, "{op}");
        }
        assert_eq!(precedence("==="), 5);
        assert_eq!(precedence("ʰ"), 10);
    }

    #[test]
    fn pipe_operators_have_precedence_two() {
        assert_eq!(precedence("|"), 2);
    }

    #[test]
    fn caret_operators_have_precedence_three() {
        assert_eq!(precedence("^"), 3);
    }

    #[test]
    fn ampersand_operators_have_precedence_four() {
        assert_eq!(precedence("&"), 4);
    }

    #[test]
    fn equals_and_bang_operators_have_precedence_five() {
        for operator in ["=", "==", "===", "=:="] {
            assert_eq!(precedence(operator), 5, "{operator}");
        }
        assert_eq!(precedence("!"), 5);
    }

    #[test]
    fn angle_operators_have_precedence_six() {
        assert_eq!(precedence("<"), 6);
        assert_eq!(precedence(">>"), 6);
    }

    #[test]
    fn colon_operators_have_precedence_seven() {
        assert_eq!(precedence(":"), 7);
    }

    #[test]
    fn plus_and_minus_operators_have_precedence_eight() {
        assert_eq!(precedence("+"), 8);
        assert_eq!(precedence("-"), 8);
    }

    #[test]
    fn multiplicative_operators_have_precedence_nine() {
        assert_eq!(precedence("*"), 9);
        assert_eq!(precedence("/"), 9);
        assert_eq!(precedence("%"), 9);
    }

    #[test]
    fn other_and_empty_operators_have_precedence_ten() {
        assert_eq!(precedence("?"), 10);
        assert_eq!(precedence(""), 10);
    }

    #[test]
    fn assignment_operators_have_precedence_zero() {
        for operator in ["+=", "::=", "⇒=", "🂡=", "!==", "⊕="] {
            assert_eq!(precedence(operator), 0, "{operator}");
            assert!(is_assignment_operator(operator));
        }
    }

    #[test]
    fn comparison_and_equality_operators_are_not_assignments() {
        for operator in [
            "=", "==", "===", "=:=", "!=", "<=", ">=", "foo=", "_=", "$=", "1=", ".=", "_x=", "",
        ] {
            assert!(!is_assignment_operator(operator), "{operator}");
        }
    }

    #[test]
    fn colon_terminated_operators_are_right_associative() {
        assert!(is_right_associative("::"));
        assert!(is_right_associative(":"));
        assert!(!is_right_associative("+"));
    }

    #[test]
    fn detects_mixed_associativity_at_equal_precedence() {
        for namespace in [Namespace::Term, Namespace::Type] {
            assert!(has_mixed_associativity("+", "+:", namespace));
            assert!(has_mixed_associativity("+:", "+", namespace));
            assert!(!has_mixed_associativity("+", "-", namespace));
            assert!(!has_mixed_associativity("+", "*", namespace));
        }
        assert!(has_mixed_associativity("+=", "+:", Namespace::Type));
        assert!(!has_mixed_associativity("+=", "+:", Namespace::Term));
    }

    #[test]
    fn shared_reduction_uses_scala_unicode_categories_in_every_grammar() {
        use crate::compilation_unit::tests::{parser_for, token};
        use dotty_core::TokenKind;

        for kind in [ParseKind::Expr, ParseKind::Type, ParseKind::Pattern] {
            for (operator, outer_name) in [("Ⅳ", "Ⅳ"), ("ʰ", "+")] {
                let source = format!("a {operator} b + c");
                let operator_end = 2 + operator.len() as u32;
                let end = source.len() as u32;
                let mut names = NameInterner::new();
                let mut parser = parser_for(
                    &source,
                    vec![
                        token(TokenKind::Identifier, 0, 1),
                        token(TokenKind::Identifier, 2, operator_end),
                        token(TokenKind::Identifier, operator_end + 1, operator_end + 2),
                        token(TokenKind::Operator, operator_end + 3, operator_end + 4),
                        token(TokenKind::Identifier, end - 1, end),
                        token(TokenKind::Eof, end, end),
                    ],
                    &mut names,
                );
                let root = match kind {
                    ParseKind::Expr => parser.expr(),
                    ParseKind::Type => parser.type_expr(),
                    ParseKind::Pattern => parser.pattern(),
                };
                let tree = parser.ast().get(root);
                let TreeKind::PhaseSpecific(UntypedNode::InfixOp(outer)) = &tree.kind else {
                    panic!("expected an infix tree for {kind:?} {source}");
                };
                assert_eq!(parser.names.resolve(outer.op.text()), outer_name);
                assert_eq!(outer.op.is_type(), kind == ParseKind::Type);
                assert_eq!(
                    tree.position.unwrap().span().range(),
                    TextRange::new(0, end).unwrap()
                );
                let (nested, inner_name) = if outer_name == operator {
                    (outer.right, "+")
                } else {
                    (outer.left, operator)
                };
                let TreeKind::PhaseSpecific(UntypedNode::InfixOp(inner)) =
                    &parser.ast().get(nested).kind
                else {
                    panic!("expected a nested infix tree for {kind:?} {source}");
                };
                assert_eq!(parser.names.resolve(inner.op.text()), inner_name);
                assert_eq!(inner.op.is_type(), kind == ParseKind::Type);
                assert_eq!(parser.current().kind, TokenKind::Eof);
                assert!(parser.diagnostics().is_empty(), "{kind:?} {source}");
            }
        }
    }

    #[test]
    fn op_info_preserves_operand_operator_and_offset() {
        let mut names = NameInterner::new();
        let operator = names.intern("+");
        let term = dotty_core::TermName::new(operator);
        let mut ast = AstArena::new();
        let operand = ast.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: *term.as_name(),
                backquoted: false,
            }),
            position: None,
            ty: (),
        });
        let info = OpInfo {
            operand,
            operator: *term.as_name(),
            offset: 12,
        };

        assert_eq!(info.operand, operand);
        assert_eq!(info.operator.text(), operator);
        assert_eq!(info.offset, 12);
    }
}
