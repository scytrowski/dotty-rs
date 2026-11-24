//! Written argument lists for cursor queries in one source revision.

use dotty_core::TextRange;

/// A closed parenthesized application, including nested and indirect applications.
#[derive(Debug)]
pub struct CallArgumentList {
    /// Written callee token for a direct named call. Other applications have no name here.
    pub callee: Option<TextRange>,
    /// Half-open UTF-8 byte range from the opening through the closing parenthesis.
    pub range: TextRange,
    /// Expression ranges in written order, excluding delimiters and surrounding trivia.
    pub arguments: Box<[TextRange]>,
    /// True for an ordinary clause containing only positional arguments.
    pub positional: bool,
    pub(crate) comma_ends: Box<[u32]>,
    pub(crate) parent: Option<usize>,
}

impl CallArgumentList {
    /// Index of the written argument at a UTF-8 insertion position. Trivia before
    /// a comma belongs to the previous argument; immediately after a comma the
    /// next argument becomes active. Empty lists, trailing commas without a next
    /// argument, and positions outside the parentheses yield `None`.
    pub fn argument_index(&self, offset: u32) -> Option<usize> {
        if offset <= self.range.start() || offset >= self.range.end() {
            return None;
        }
        let slot = self.comma_ends.partition_point(|end| *end <= offset);
        (slot < self.arguments.len()).then_some(slot)
    }
}

/// Index of completed parenthesized applications in a single parse.
/// It owns only byte ranges and can be retained after dropping the syntax arena.
#[derive(Debug)]
pub struct SourceCallArguments(Vec<CallArgumentList>);

impl SourceCallArguments {
    pub(crate) fn new(mut lists: Vec<CallArgumentList>) -> Self {
        lists.sort_unstable_by_key(|list| list.range.start());
        let mut parent: Option<usize> = None;
        for i in 0..lists.len() {
            while let Some(p) = parent {
                if lists[p].range.contains(lists[i].range.start()) {
                    break;
                }
                parent = lists[p].parent;
            }
            lists[i].parent = parent;
            parent = Some(i);
        }
        Self(lists)
    }

    /// Finds the innermost list containing a cursor between the parentheses,
    /// including immediately before the closing token. The positions before
    /// the opening and after the closing token are outside. Supply a UTF-8
    /// byte offset from the original revision. Unclosed lists have no entry.
    pub fn at(&self, offset: u32) -> Option<&CallArgumentList> {
        let next = self.0.partition_point(|list| list.range.start() < offset);
        let mut list = self.0.get(next.checked_sub(1)?)?;
        while offset >= list.range.end() {
            list = &self.0[list.parent?];
        }
        Some(list)
    }
}

#[cfg(test)]
mod tests {
    use dotty_core::{NameInterner, SourceId, SourceText};
    use dotty_lexer::ContextualScanner;

    fn parse(text: &str) -> crate::ParseResult {
        crate::parse_expression_fragment(
            SourceText::new(text).unwrap(),
            SourceId::from_index(1),
            ContextualScanner::new(text).unwrap(),
            &mut NameInterner::new(),
        )
    }

    #[test]
    fn nested_lists_use_written_delimiters_and_argument_ranges() {
        let text = "outer(inner(1 /* , */, 2), /* 😀 , */ 3)";
        let parsed = parse(text);
        assert!(parsed.diagnostics.is_empty());
        let pos = |token: &str| text.find(token).unwrap() as u32;
        let at = |offset| parsed.call_arguments.at(offset).unwrap();
        let slice = |r: dotty_core::TextRange| &text[r.start() as usize..r.end() as usize];
        let outer = at(pos("inner"));
        let inner = at(pos("1"));
        assert_eq!(slice(outer.range), &text[5..]);
        assert_eq!(slice(inner.callee.unwrap()), "inner");
        assert_eq!(
            inner
                .arguments
                .iter()
                .copied()
                .map(slice)
                .collect::<Vec<_>>(),
            ["1", "2"]
        );
        assert_eq!(
            outer
                .arguments
                .iter()
                .copied()
                .map(slice)
                .collect::<Vec<_>>(),
            ["inner(1 /* , */, 2)", "3"]
        );
        for (offset, callee, slot) in [
            (pos(","), "inner", 0),
            (pos(", 2"), "inner", 0),
            (pos(", 2") + 1, "inner", 1),
            (pos("),"), "inner", 1),
            (pos("),") + 1, "outer", 0),
            (pos("),") + 2, "outer", 1),
            (pos("😀"), "outer", 1),
            (pos("3)") + 1, "outer", 1),
        ] {
            let list = at(offset);
            assert_eq!(
                (slice(list.callee.unwrap()), list.argument_index(offset)),
                (callee, Some(slot))
            );
        }
        assert!(parsed.call_arguments.at(5).is_none());
        assert!(parsed.call_arguments.at(text.len() as u32).is_none());
    }

    #[test]
    fn empty_trailing_named_and_unclosed_lists_remain_distinct() {
        for (text, positional, count, slot) in [
            ("f()", true, 0, None),
            ("f(1,)", true, 1, None),
            ("f(n = 1)", false, 1, Some(0)),
            ("f(using x)", false, 1, Some(0)),
        ] {
            let parsed = parse(text);
            assert!(
                parsed.diagnostics.is_empty(),
                "{text}: {:?}",
                parsed.diagnostics
            );
            let offset = text.len() as u32 - 1;
            let list = parsed.call_arguments.at(offset).unwrap();
            assert_eq!(
                (
                    list.positional,
                    list.arguments.len(),
                    list.argument_index(offset)
                ),
                (positional, count, slot)
            );
        }
        assert!(parse("f(1").call_arguments.at(2).is_none());
    }
}
