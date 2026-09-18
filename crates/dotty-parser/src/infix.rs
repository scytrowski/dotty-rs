use dotty_core::{Name, TreeId, Untyped};

/// Operand/operator state used by future Dotty-style infix reduction.
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
        Some(first) if first.is_alphabetic() => 1,
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

/// Returns whether `operator` is a Scala assignment operator.
pub fn is_assignment_operator(operator: &str) -> bool {
    operator.ends_with('=') && !matches!(operator, "==" | "!=" | "<=" | ">=")
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::ast::Ident;
    use dotty_core::{AstArena, NameInterner, Tree, TreeKind};

    #[test]
    fn letter_operators_have_the_lowest_non_assignment_precedence() {
        assert_eq!(precedence("foo"), 1);
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
        assert_eq!(precedence("=="), 5);
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
        for operator in ["=", "+=", "foo="] {
            assert_eq!(precedence(operator), 0, "{operator}");
            assert!(is_assignment_operator(operator));
        }
    }

    #[test]
    fn comparison_and_equality_operators_are_not_assignments() {
        for operator in ["==", "!=", "<=", ">="] {
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
    fn op_info_preserves_operand_operator_and_offset() {
        let mut names = NameInterner::new();
        let operator = names.intern("+");
        let term = dotty_core::TermName::new(operator);
        let mut ast = AstArena::new();
        let operand = ast.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: *term.as_name(),
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
