//! Source positions shared by the semantic model.
//!
//! A [`Span`] is a byte range plus an optional diagnostic point, with no
//! notion of *which* source file it belongs to. [`SourceSpan`] adds that by
//! pairing a [`Span`] with a [`SourceId`]. Keeping the two separate lets a
//! tree's position be `Option<SourceSpan>` instead of requiring every tree —
//! including compiler-generated ones with no real source — to carry a
//! sentinel `SourceId` (see `docs/dotty-core-design.md`, "source positions
//! need an explicit no-source/synthetic state").
//!
//! [`TextRange`] here is a **temporary stand-in** for `dotty_source::TextRange`.
//! `dotty-core` was branched from `main` while the `dotty-source` crate still
//! lives only on the not-yet-merged `feature/lexer` branch, so it cannot be a
//! path dependency yet without coupling `dotty-core` to another team's
//! in-flight branch. Once `feature/lexer` merges into `main`, replace this
//! module's `TextRange`/`TextRangeError` with a re-export of
//! `dotty_source::TextRange` and drop the local definition — see
//! `docs/dotty-core-design.md`, "§2 deviations."

use core::fmt;

use crate::ids::SourceId;

/// A half-open byte range into a UTF-8 source buffer.
///
/// Temporary local stand-in for `dotty_source::TextRange` — see the module
/// documentation above.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextRange {
    start: u32,
    end: u32,
}

/// Failure while constructing a [`TextRange`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextRangeError {
    EndBeforeStart { start: u32, end: u32 },
}

impl fmt::Display for TextRangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EndBeforeStart { start, end } => {
                write!(formatter, "range end {end} precedes start {start}")
            }
        }
    }
}

impl std::error::Error for TextRangeError {}

impl TextRange {
    /// Creates a half-open range `[start, end)`.
    pub fn new(start: u32, end: u32) -> Result<Self, TextRangeError> {
        if end < start {
            return Err(TextRangeError::EndBeforeStart { start, end });
        }

        Ok(Self { start, end })
    }

    pub const fn start(self) -> u32 {
        self.start
    }

    pub const fn end(self) -> u32 {
        self.end
    }
}

/// A byte range plus an optional diagnostic point, independent of which
/// source file it is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    range: TextRange,
    point: Option<u32>,
}

impl Span {
    /// Creates a span covering `range`, with `point` identifying the primary
    /// diagnostic offset inside it (e.g. the start of `bar` in `foo.bar`,
    /// where `range` covers the whole selection).
    pub const fn new(range: TextRange, point: Option<u32>) -> Self {
        Self { range, point }
    }

    /// Creates a span with no distinguished diagnostic point.
    pub const fn without_point(range: TextRange) -> Self {
        Self::new(range, None)
    }

    pub const fn range(self) -> TextRange {
        self.range
    }

    pub const fn point(self) -> Option<u32> {
        self.point
    }
}

/// A [`Span`] located in a specific source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceSpan {
    source: SourceId,
    span: Span,
}

impl SourceSpan {
    pub const fn new(source: SourceId, span: Span) -> Self {
        Self { source, span }
    }

    pub const fn source(self) -> SourceId {
        self.source
    }

    pub const fn span(self) -> Span {
        self.span
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_range_exposes_its_start_and_end() {
        let range = TextRange::new(3, 9).expect("valid range");

        assert_eq!(range.start(), 3);
        assert_eq!(range.end(), 9);
    }

    #[test]
    fn text_range_rejects_a_range_whose_end_precedes_its_start() {
        assert_eq!(
            TextRange::new(5, 2),
            Err(TextRangeError::EndBeforeStart { start: 5, end: 2 })
        );
    }

    #[test]
    fn span_without_point_has_no_diagnostic_point() {
        let range = TextRange::new(0, 3).expect("valid range");
        let span = Span::without_point(range);

        assert_eq!(span.range(), range);
        assert_eq!(span.point(), None);
    }

    #[test]
    fn span_keeps_its_diagnostic_point_distinct_from_the_range() {
        let range = TextRange::new(0, 7).expect("valid range");
        let span = Span::new(range, Some(4));

        assert_eq!(span.range(), range);
        assert_eq!(span.point(), Some(4));
    }

    #[test]
    fn source_span_pairs_a_span_with_its_source() {
        let range = TextRange::new(2, 5).expect("valid range");
        let span = Span::without_point(range);
        let source = SourceId::new(1);
        let source_span = SourceSpan::new(source, span);

        assert_eq!(source_span.source(), source);
        assert_eq!(source_span.span(), span);
    }
}
