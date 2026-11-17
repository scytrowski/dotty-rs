use dotty_core::TokenSource;

use crate::Parser;

impl<'src, S: TokenSource> Parser<'src, '_, S> {
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
