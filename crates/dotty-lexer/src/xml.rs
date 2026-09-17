use crate::RawTokenKind;
use dotty_token::Punctuation;

#[derive(Debug, Clone, Copy)]
struct XmlExpression {
    brace_depth: u32,
    xml_floor: u32,
}

/// State shared by the raw and contextual XML handling stages.
#[derive(Debug, Default)]
pub(crate) struct XmlState {
    depth: u32,
    closing_tag: bool,
    expressions: Vec<XmlExpression>,
}

impl XmlState {
    pub(crate) fn can_start_literal(&self) -> bool {
        self.depth == 0 || !self.expressions.is_empty()
    }

    pub(crate) fn update_token(&mut self, kind: RawTokenKind, spelling: &str) -> bool {
        match kind {
            RawTokenKind::XmlStart => {
                self.depth = self.depth.saturating_add(1);
                self.closing_tag = false;
            }
            RawTokenKind::Punctuation(Punctuation::LeftBrace) => self.update_left_brace(),
            RawTokenKind::Punctuation(Punctuation::RightBrace) => self.update_right_brace(),
            RawTokenKind::Operator => return self.update_operator(spelling),
            _ => {}
        }
        false
    }

    fn update_left_brace(&mut self) {
        if self.depth == 0 {
            return;
        }

        let starts_xml_expression = self
            .expressions
            .last()
            .is_none_or(|expression| self.depth > expression.xml_floor);
        if starts_xml_expression {
            self.expressions.push(XmlExpression {
                brace_depth: 1,
                xml_floor: self.depth,
            });
        } else if let Some(expression) = self.expressions.last_mut() {
            expression.brace_depth = expression.brace_depth.saturating_add(1);
        }
    }

    fn update_right_brace(&mut self) {
        let Some(expression) = self.expressions.last_mut() else {
            return;
        };

        if expression.brace_depth > 1 {
            expression.brace_depth -= 1;
        } else {
            let _ = self.expressions.pop();
        }
    }

    fn update_operator(&mut self, spelling: &str) -> bool {
        if self.depth == 0
            || self
                .expressions
                .last()
                .is_some_and(|expression| self.depth == expression.xml_floor)
        {
            return false;
        }

        match spelling {
            spelling if spelling.ends_with("</") => {
                if (spelling.starts_with('>') || spelling.starts_with("/>")) && self.depth > 1 {
                    self.depth -= 1;
                }
                self.closing_tag = true;
            }
            "/>" => {
                self.depth = self.depth.saturating_sub(1);
                self.closing_tag = false;
                return self.depth == 0;
            }
            spelling if spelling.ends_with('<') => {
                self.depth = self.depth.saturating_add(1);
                self.closing_tag = false;
            }
            ">" if self.closing_tag => {
                self.depth = self.depth.saturating_sub(1);
                self.closing_tag = false;
                return self.depth == 0;
            }
            _ => {}
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_a_nested_xml_literal_inside_an_expression() {
        let mut state = XmlState::default();
        state.update_token(RawTokenKind::XmlStart, "<");
        state.update_token(RawTokenKind::Punctuation(Punctuation::LeftBrace), "{");
        state.update_token(RawTokenKind::XmlStart, "<");
        assert!(!state.update_token(RawTokenKind::Operator, "/>"));
        state.update_token(RawTokenKind::Punctuation(Punctuation::RightBrace), "}");
        assert!(!state.update_token(RawTokenKind::Operator, "</"));
        assert!(state.update_token(RawTokenKind::Operator, ">"));
    }

    #[test]
    fn ignores_less_than_operators_in_an_xml_expression() {
        let mut state = XmlState::default();
        state.update_token(RawTokenKind::XmlStart, "<");
        state.update_token(RawTokenKind::Punctuation(Punctuation::LeftBrace), "{");
        assert!(!state.update_token(RawTokenKind::Operator, "<"));
        state.update_token(RawTokenKind::Punctuation(Punctuation::RightBrace), "}");
        assert!(!state.update_token(RawTokenKind::Operator, "</"));
        assert!(state.update_token(RawTokenKind::Operator, ">"));
    }
}
