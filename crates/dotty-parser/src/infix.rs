use dotty_core::ast::{InfixOp, UntypedNode};
use dotty_core::{Name, SourceSpan, Span, TextRange, TokenSource, TreeId, TreeKind, Untyped};

use crate::{ExpressionIssue, ParseIssue, ParseKind, Parser, PatternIssue, TypeIssue};

/// Operand/operator state used by Dotty-style infix reduction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpInfo {
    pub operand: TreeId<Untyped>,
    pub operator: Name,
    pub offset: u32,
}

/// Returns the Scala 3 operator precedence bucket for `operator`.
pub fn precedence(operator: &str) -> u8 {
    if is_assignment_operator(operator) {
        return 0;
    }

    match operator.chars().next() {
        Some(first) if first.is_alphabetic() || matches!(first, '_' | '$') => 1,
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
fn has_mixed_associativity(left: &str, right: &str) -> bool {
    precedence(left) == precedence(right)
        && is_right_associative(left) != is_right_associative(right)
}

/// Returns whether `operator` is a Scala assignment operator.
pub fn is_assignment_operator(operator: &str) -> bool {
    operator.ends_with('=')
        && !operator.starts_with('=')
        && !matches!(operator, "!=" | "<=" | ">=")
        && operator
            .chars()
            .next()
            .is_some_and(dotty_core::token::is_operator_character)
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
        let incoming_precedence = precedence(spelling);
        let left_associative = !is_right_associative(spelling);

        // Dotty checks associativity against the initial stack top before any reductions.
        if let Some(previous) = operators.last()
            && has_mixed_associativity(self.names.resolve(previous.operator.text()), spelling)
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
            let pending_precedence = precedence(self.names.resolve(previous.operator.text()));
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
        for operator in ["foo", "foo=", "_op", "$op"] {
            assert_eq!(precedence(operator), 1, "{operator}");
        }
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
        for operator in ["+=", "::=", "⇒=", "🂡="] {
            assert_eq!(precedence(operator), 0, "{operator}");
            assert!(is_assignment_operator(operator));
        }
    }

    #[test]
    fn comparison_and_equality_operators_are_not_assignments() {
        for operator in [
            "=", "==", "===", "=:=", "!=", "<=", ">=", "foo=", "_=", "$=", "1=", ".=",
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
        assert!(has_mixed_associativity("+", "+:"));
        assert!(has_mixed_associativity("+:", "+"));
        assert!(!has_mixed_associativity("+", "-"));
        assert!(!has_mixed_associativity("+", "*"));
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
