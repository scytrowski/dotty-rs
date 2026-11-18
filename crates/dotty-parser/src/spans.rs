use dotty_core::TokenSource;

use crate::Parser;

impl<'src, S: TokenSource> Parser<'src, '_, S> {
    /// Whether the source between token boundaries contains a Scala line break.
    /// An invalid byte range leaves the layout decision to the caller.
    pub(crate) fn line_break_in_gap(&self, start: u32, end: u32) -> Option<bool> {
        self.source
            .as_str()
            .get(start as usize..end as usize)
            .map(|gap| gap.contains(dotty_core::is_line_break_char))
    }

    /// Source on the current physical line up to a token's byte offset.
    pub(crate) fn line_prefix_at(&self, offset: u32) -> Option<&'src str> {
        self.source
            .as_str()
            .get(..offset as usize)?
            .rsplit(dotty_core::is_line_break_char)
            .next()
    }

    pub(crate) fn source_line_indent_prefix(&self, offset: u32) -> &'src str {
        let prefix = self
            .line_prefix_at(offset.min(self.source.len_bytes()))
            .unwrap_or_default();
        let end = prefix
            .find(|ch| !matches!(ch, ' ' | '\t'))
            .unwrap_or(prefix.len());
        &prefix[..end]
    }
}

/// A parser position used to build a source span after consuming tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub(crate) start: u32,
}

impl Mark {
    /// Returns the byte offset at which this mark was created.
    pub const fn start(self) -> u32 {
        self.start
    }
}

#[cfg(test)]
mod tests {
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::{NameInterner, TokenKind};

    #[test]
    fn physical_gaps_distinguish_line_breaks_from_invalid_token_offsets() {
        let source = "é\r\n\u{c}\u{1a}\u{2028}";
        let end = source.len() as u32;
        let mut names = NameInterner::new();
        let parser = parser_for(source, vec![token(TokenKind::Eof, end, end)], &mut names);
        for i in 2..6 {
            assert_eq!(parser.line_break_in_gap(i, i + 1), Some(true));
        }
        assert_eq!(parser.line_break_in_gap(0, 2), Some(false));
        assert_eq!(parser.line_break_in_gap(6, end), Some(false));
        assert_eq!(parser.line_break_in_gap(end, end), Some(false));
        assert_eq!(parser.line_break_in_gap(1, 2), None);
        assert_eq!(parser.line_break_in_gap(2, 0), None);
        assert_eq!(parser.line_break_in_gap(0, end + 1), None);
    }
}
