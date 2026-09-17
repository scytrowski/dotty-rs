use crate::RawTokenKind;
use dotty_token::Punctuation;

#[derive(Debug, Clone, Copy)]
struct XmlExpression {
    brace_depth: u32,
    xml_floor: u32,
    attribute: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XmlContent {
    Text,
    Comment,
    Cdata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingTagName {
    Opening,
    Closing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XmlAttributeState {
    ExpectNameOrEnd,
    ExpectEquals,
    ExpectValue,
    InExpression,
}

impl Default for XmlContent {
    fn default() -> Self {
        Self::Text
    }
}

/// State shared by the raw and contextual XML handling stages.
#[derive(Debug, Default)]
pub(crate) struct XmlState {
    depth: u32,
    closing_tag: bool,
    expressions: Vec<XmlExpression>,
    content: XmlContent,
    cdata_brackets: u8,
    pending_error: Option<String>,
    tag_open: bool,
    open_tags: Vec<String>,
    pending_tag_name: Option<PendingTagName>,
    current_tag_name: Option<String>,
    tag_name_separator: bool,
    closing_tag_name: Option<String>,
    attribute_state: Option<XmlAttributeState>,
    attribute_name_separator: bool,
}

impl XmlState {
    pub(crate) fn can_start_literal(&self) -> bool {
        self.content == XmlContent::Text && (self.depth == 0 || !self.expressions.is_empty())
    }

    pub(crate) fn is_xml_name_separator(&self) -> bool {
        (self.pending_tag_name.is_some() && self.current_tag_name.is_some())
            || (self.tag_open
                && !self.closing_tag
                && self.attribute_state == Some(XmlAttributeState::ExpectEquals)
                && !self.attribute_name_separator)
    }

    pub(crate) fn eof_message(&self) -> Option<String> {
        match self.content {
            XmlContent::Comment => Some("unterminated XML comment".to_owned()),
            XmlContent::Cdata => Some("unterminated XML CDATA section".to_owned()),
            XmlContent::Text
                if self
                    .expressions
                    .last()
                    .is_some_and(|expression| expression.attribute) =>
            {
                Some("XML attribute expression must be closed".to_owned())
            }
            XmlContent::Text if !self.expressions.is_empty() => {
                Some("unterminated XML expression".to_owned())
            }
            XmlContent::Text if self.attribute_state == Some(XmlAttributeState::ExpectEquals) => {
                Some("XML attribute name must be followed by `=`".to_owned())
            }
            XmlContent::Text if self.attribute_state == Some(XmlAttributeState::ExpectValue) => {
                Some("XML attribute value expected after `=`".to_owned())
            }
            XmlContent::Text if self.tag_open || self.depth > 0 => {
                Some("unterminated XML tag".to_owned())
            }
            XmlContent::Text => None,
        }
    }

    pub(crate) fn take_error(&mut self) -> Option<String> {
        self.pending_error.take()
    }

    pub(crate) fn update_token(&mut self, kind: RawTokenKind, spelling: &str) -> bool {
        if self.content == XmlContent::Cdata {
            match kind {
                RawTokenKind::Punctuation(Punctuation::RightBracket) => {
                    self.cdata_brackets = self.cdata_brackets.saturating_add(1).min(2);
                    return false;
                }
                RawTokenKind::Operator if self.cdata_brackets == 2 && spelling.starts_with('>') => {
                    self.content = XmlContent::Text;
                    self.cdata_brackets = 0;
                }
                _ => {
                    self.cdata_brackets = 0;
                    return false;
                }
            }
        }

        match kind {
            RawTokenKind::XmlStart => {
                self.depth = self.depth.saturating_add(1);
                self.closing_tag = false;
                self.tag_open = true;
                self.pending_tag_name = Some(PendingTagName::Opening);
                self.current_tag_name = None;
                self.tag_name_separator = false;
            }
            RawTokenKind::Identifier | RawTokenKind::Keyword(_) => {
                if self.pending_tag_name.is_some() {
                    self.consume_tag_name(spelling);
                } else if !self.in_xml_expression_at_current_depth() {
                    self.consume_attribute_name();
                }
            }
            RawTokenKind::StringLiteral => {
                if !self.in_xml_expression_at_current_depth() {
                    self.consume_attribute_value(false);
                }
            }
            RawTokenKind::Punctuation(punctuation)
                if matches!(punctuation, Punctuation::Colon | Punctuation::Dot)
                    && self.consume_xml_name_separator(spelling) => {}
            RawTokenKind::Punctuation(Punctuation::LeftBrace) => self.update_left_brace(),
            RawTokenKind::Punctuation(Punctuation::RightBrace) => self.update_right_brace(),
            RawTokenKind::Operator => return self.update_operator(spelling),
            _ => self.report_unexpected_attribute_token(),
        }
        false
    }

    fn consume_tag_name(&mut self, spelling: &str) {
        let Some(pending) = self.pending_tag_name else {
            return;
        };

        if let Some(name) = self.current_tag_name.as_mut() {
            if self.tag_name_separator {
                name.push_str(spelling);
                self.tag_name_separator = false;
                return;
            }

            self.finalize_tag_name();
            if pending == PendingTagName::Opening {
                self.consume_attribute_name();
            }
            return;
        }

        self.current_tag_name = Some(spelling.to_owned());
    }

    fn finalize_tag_name(&mut self) {
        let Some(pending) = self.pending_tag_name.take() else {
            return;
        };
        let Some(name) = self.current_tag_name.take() else {
            return;
        };

        self.tag_name_separator = false;
        match pending {
            PendingTagName::Opening => {
                self.open_tags.push(name);
                self.attribute_state = Some(XmlAttributeState::ExpectNameOrEnd);
            }
            PendingTagName::Closing => self.closing_tag_name = Some(name),
        }
    }

    fn consume_xml_name_separator(&mut self, spelling: &str) -> bool {
        if self.pending_tag_name.is_some() && self.current_tag_name.is_some() {
            if let Some(name) = self.current_tag_name.as_mut() {
                name.push_str(spelling);
            }
            self.tag_name_separator = true;
            return true;
        }

        if self.tag_open
            && !self.closing_tag
            && self.attribute_state == Some(XmlAttributeState::ExpectEquals)
        {
            self.attribute_name_separator = true;
            return true;
        }

        false
    }

    fn consume_attribute_name(&mut self) {
        if !self.tag_open || self.closing_tag {
            return;
        }

        if self.attribute_name_separator {
            self.attribute_name_separator = false;
            return;
        }

        if self.attribute_state == Some(XmlAttributeState::ExpectEquals) {
            self.report_error("XML attribute name must be followed by `=`");
        } else if self.attribute_state == Some(XmlAttributeState::ExpectValue) {
            self.report_error("XML attribute value expected after `=`");
        }
        self.attribute_state = Some(XmlAttributeState::ExpectEquals);
    }

    fn consume_attribute_value(&mut self, expression: bool) {
        if !self.tag_open || self.closing_tag {
            return;
        }

        if expression {
            self.attribute_state = Some(XmlAttributeState::InExpression);
        } else if self.attribute_state == Some(XmlAttributeState::ExpectValue) {
            self.attribute_state = Some(XmlAttributeState::ExpectNameOrEnd);
            self.attribute_name_separator = false;
        } else {
            self.report_error("XML attribute value must follow `=`");
        }
    }

    fn report_unexpected_attribute_token(&mut self) {
        if !self.tag_open || self.closing_tag {
            return;
        }
        if self.in_xml_expression_at_current_depth() {
            return;
        }

        if self.pending_tag_name.is_some() && self.current_tag_name.is_some() {
            self.finalize_tag_name();
        }

        match self.attribute_state {
            Some(XmlAttributeState::ExpectNameOrEnd) => {
                self.report_error("XML attribute name expected")
            }
            Some(XmlAttributeState::ExpectEquals) => {
                self.report_error("XML attribute name must be followed by `=`")
            }
            Some(XmlAttributeState::ExpectValue) => {
                self.report_error("XML attribute value expected after `=`")
            }
            Some(XmlAttributeState::InExpression) | None => {}
        }
    }

    fn in_xml_expression_at_current_depth(&self) -> bool {
        self.expressions
            .last()
            .is_some_and(|expression| self.depth == expression.xml_floor)
    }

    fn report_error(&mut self, message: impl Into<String>) {
        if self.pending_error.is_none() {
            self.pending_error = Some(message.into());
        }
    }

    fn finish_opening_tag(&mut self) {
        self.finalize_tag_name();
        if self.pending_tag_name == Some(PendingTagName::Opening) {
            self.report_error("XML opening tag name expected");
            self.pending_tag_name = None;
        }
        match self.attribute_state {
            Some(XmlAttributeState::ExpectEquals) => {
                self.report_error("XML attribute name must be followed by `=`")
            }
            Some(XmlAttributeState::ExpectValue) => {
                self.report_error("XML attribute value expected after `=`")
            }
            Some(XmlAttributeState::InExpression) => {
                self.report_error("XML attribute expression must be closed")
            }
            Some(XmlAttributeState::ExpectNameOrEnd) | None => {}
        }
        self.attribute_state = None;
        self.tag_open = false;
    }

    fn finish_closing_tag(&mut self) {
        self.finalize_tag_name();
        if self.pending_tag_name == Some(PendingTagName::Closing) {
            self.report_error("XML closing tag name expected");
            self.pending_tag_name = None;
        }

        if let Some(actual) = self.closing_tag_name.take() {
            match self.open_tags.pop() {
                Some(expected) if expected == actual => {}
                Some(expected) => self.report_error(format!(
                    "mismatched XML closing tag: expected </{expected}>, found </{actual}>"
                )),
                None => self.report_error(format!(
                    "XML closing tag </{actual}> has no matching opening tag"
                )),
            }
        } else if self.open_tags.pop().is_none() {
            self.report_error("XML closing tag has no matching opening tag");
        }

        self.closing_tag = false;
        self.tag_open = false;
        self.depth = self.depth.saturating_sub(1);
    }

    fn finish_self_closing_tag(&mut self) {
        self.finish_opening_tag();
        let _ = self.open_tags.pop();
        self.depth = self.depth.saturating_sub(1);
        self.closing_tag = false;
        self.tag_open = false;
    }

    fn finish_empty_closing_tag(&mut self) {
        self.report_error("XML closing tag name expected");
        let _ = self.open_tags.pop();
        self.depth = self.depth.saturating_sub(1);
        self.closing_tag = false;
        self.tag_open = false;
    }

    fn update_left_brace(&mut self) {
        if self.depth == 0 || self.content != XmlContent::Text {
            return;
        }

        let attribute = self.tag_open
            && !self.closing_tag
            && self.attribute_state == Some(XmlAttributeState::ExpectValue);
        if self.tag_open
            && !self.closing_tag
            && !self.in_xml_expression_at_current_depth()
            && !matches!(
                self.attribute_state,
                Some(XmlAttributeState::ExpectValue | XmlAttributeState::InExpression)
            )
        {
            self.report_unexpected_attribute_token();
        }
        if attribute {
            self.consume_attribute_value(true);
        }

        let starts_xml_expression = self
            .expressions
            .last()
            .is_none_or(|expression| self.depth > expression.xml_floor);
        if starts_xml_expression {
            self.expressions.push(XmlExpression {
                brace_depth: 1,
                xml_floor: self.depth,
                attribute,
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
            let attribute = expression.attribute;
            let _ = self.expressions.pop();
            if attribute {
                self.attribute_state = Some(XmlAttributeState::ExpectNameOrEnd);
                self.attribute_name_separator = false;
            }
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

        if self.content == XmlContent::Comment {
            if spelling.contains("-->") {
                self.content = XmlContent::Text;
            } else {
                if spelling.contains("--") {
                    self.report_error("invalid `--` sequence in XML comment");
                }
                return false;
            }
        }
        if self.content == XmlContent::Cdata {
            if spelling.ends_with("</") {
                self.content = XmlContent::Text;
            } else {
                return false;
            }
        }

        if spelling.contains("<!--") {
            self.finish_opening_tag();
            self.tag_open = false;
            self.content = XmlContent::Comment;
            return false;
        }
        if spelling.contains("<!") {
            self.finish_opening_tag();
            self.tag_open = false;
            self.content = XmlContent::Cdata;
            return false;
        }

        if self.tag_open && !self.closing_tag && spelling == "=/>" {
            if self.attribute_state == Some(XmlAttributeState::ExpectEquals) {
                self.attribute_state = Some(XmlAttributeState::ExpectValue);
            } else {
                self.report_error("XML attribute `=` is unexpected");
            }
            self.finish_self_closing_tag();
            return self.depth == 0;
        }

        if matches!(spelling, "-" | ":" | ".") && self.consume_xml_name_separator(spelling) {
            return false;
        }

        if self.tag_open && !self.closing_tag && spelling == "=" {
            if self.attribute_state == Some(XmlAttributeState::ExpectEquals) {
                self.attribute_state = Some(XmlAttributeState::ExpectValue);
            } else {
                self.report_error("XML attribute `=` is unexpected");
            }
            return false;
        }

        match spelling {
            spelling if spelling.ends_with("</>") => {
                if spelling.starts_with('>') {
                    if self.closing_tag {
                        self.finish_closing_tag();
                    } else {
                        self.finish_opening_tag();
                    }
                }
                self.finish_empty_closing_tag();
                return self.depth == 0;
            }
            spelling if spelling.ends_with("</") => {
                if spelling.starts_with("/>") {
                    self.finish_self_closing_tag();
                } else if spelling.starts_with('>') {
                    if self.closing_tag {
                        self.finish_closing_tag();
                    } else {
                        self.finish_opening_tag();
                    }
                }
                self.closing_tag = true;
                self.tag_open = true;
                self.pending_tag_name = Some(PendingTagName::Closing);
                self.current_tag_name = None;
                self.tag_name_separator = false;
                self.closing_tag_name = None;
            }
            "/>" => {
                self.finish_self_closing_tag();
                return self.depth == 0;
            }
            spelling if spelling.ends_with('<') => {
                self.finish_opening_tag();
                self.depth = self.depth.saturating_add(1);
                self.closing_tag = false;
                self.tag_open = true;
                self.pending_tag_name = Some(PendingTagName::Opening);
                self.current_tag_name = None;
                self.tag_name_separator = false;
            }
            ">" if self.closing_tag => {
                self.finish_closing_tag();
                return self.depth == 0;
            }
            ">" => self.finish_opening_tag(),
            _ if self.tag_open && !self.closing_tag => self.report_unexpected_attribute_token(),
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

    #[test]
    fn closes_a_comment_before_processing_the_following_nested_tag() {
        let mut state = XmlState::default();
        state.update_token(RawTokenKind::XmlStart, "<");
        assert!(!state.update_token(RawTokenKind::Operator, "><!--"));
        assert!(!state.update_token(RawTokenKind::Operator, "--><"));
        assert!(!state.update_token(RawTokenKind::Operator, "/>"));
        assert!(!state.update_token(RawTokenKind::Operator, "</"));
        assert!(state.update_token(RawTokenKind::Operator, ">"));
    }

    #[test]
    fn keeps_cdata_contents_out_of_xml_depth_tracking() {
        let mut state = XmlState::default();
        state.update_token(RawTokenKind::XmlStart, "<");
        assert!(!state.update_token(RawTokenKind::Operator, "><!"));
        assert!(!state.update_token(RawTokenKind::Operator, "<"));
        assert!(!state.update_token(RawTokenKind::Operator, ">"));
        state.update_token(RawTokenKind::Punctuation(Punctuation::RightBracket), "]");
        state.update_token(RawTokenKind::Punctuation(Punctuation::RightBracket), "]");
        assert!(!state.update_token(RawTokenKind::Operator, "></"));
        assert!(state.update_token(RawTokenKind::Operator, ">"));
    }

    #[test]
    fn does_not_start_nested_xml_inside_comment_or_cdata() {
        let mut state = XmlState::default();
        state.update_token(RawTokenKind::XmlStart, "<");
        state.update_token(RawTokenKind::Operator, "><!--");
        assert!(!state.can_start_literal());
        state.update_token(RawTokenKind::Punctuation(Punctuation::LeftBrace), "{");
        state.update_token(RawTokenKind::Operator, "-->");
        state.update_token(RawTokenKind::Operator, "><!");
        assert!(!state.can_start_literal());
    }
}
