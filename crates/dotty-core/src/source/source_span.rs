//! Source positions shared by the semantic model.
//!
//! A [`Span`] is a byte range plus an optional diagnostic point, with no
//! notion of *which* source file it belongs to. [`SourceSpan`] adds that by
//! pairing a [`Span`] with a [`SourceId`]. Keeping the two separate lets a
//! tree's position be `Option<SourceSpan>` instead of requiring every tree —
//! including compiler-generated ones with no real source — to carry a
//! sentinel `SourceId`.

use core::fmt;

use super::TextRange;
use crate::ids::SourceId;

/// A byte range plus an optional diagnostic point, independent of which
/// source file it is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    range: TextRange,
    point: Option<u32>,
}

/// Failure while constructing a [`Span`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanError {
    /// `point` fell outside `range`. `point` must satisfy
    /// `range.start() <= point <= range.end()` — **inclusive of `end`**,
    /// even though [`TextRange`] itself is the half-open `[start, end)`.
    /// See [`Span::new`] for why the upper bound is not tightened to `<`.
    PointOutOfRange { point: u32, range: TextRange },
}

impl fmt::Display for SpanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PointOutOfRange { point, range } => write!(
                formatter,
                "diagnostic point {point} lies outside range [{}, {}]",
                range.start(),
                range.end()
            ),
        }
    }
}

impl std::error::Error for SpanError {}

impl Span {
    /// Creates a span covering `range`, with `point` identifying the primary
    /// diagnostic offset inside it (e.g. the start of `bar` in `foo.bar`,
    /// where `range` covers the whole selection).
    ///
    /// Fails if `point` is given but does not satisfy
    /// `range.start() <= point <= range.end()`.
    ///
    /// The upper bound is **inclusive of `end`**, deliberately, even though
    /// [`TextRange`] itself is the half-open `[start, end)`: `point` is not a
    /// byte offset "inside" the range in that sense, it is an insertion
    /// position — "where a single `^` would be logically placed" for a
    /// diagnostic (matching real Dotty's `Span.point`, which allows
    /// `point == end`, e.g. for a zero-width span or a caret placed right
    /// after the range's last byte). Rejecting `point == end` would make it
    /// impossible to point at the end of a token, which is a legitimate,
    /// common diagnostic position, not an out-of-range one.
    pub fn new(range: TextRange, point: Option<u32>) -> Result<Self, SpanError> {
        if let Some(point) = point
            && !(range.start()..=range.end()).contains(&point)
        {
            return Err(SpanError::PointOutOfRange { point, range });
        }

        Ok(Self { range, point })
    }

    /// Creates a span with no distinguished diagnostic point.
    pub const fn without_point(range: TextRange) -> Self {
        Self { range, point: None }
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
    fn span_without_point_has_no_diagnostic_point() {
        let range = TextRange::new(0, 3).expect("valid range");
        let span = Span::without_point(range);

        assert_eq!(span.range(), range);
        assert_eq!(span.point(), None);
    }

    #[test]
    fn span_keeps_its_diagnostic_point_distinct_from_the_range() {
        let range = TextRange::new(0, 7).expect("valid range");
        let span = Span::new(range, Some(4)).expect("point inside range");

        assert_eq!(span.range(), range);
        assert_eq!(span.point(), Some(4));
    }

    #[test]
    fn span_accepts_a_point_at_the_start_boundary() {
        let range = TextRange::new(3, 9).expect("valid range");

        assert!(Span::new(range, Some(3)).is_ok());
    }

    #[test]
    fn span_accepts_a_point_at_the_end_boundary() {
        // Deliberately inclusive of `end` despite `TextRange` being
        // half-open: `point` is an insertion position (a caret placed after
        // the range's last byte), not a byte offset "inside" the range in
        // the `TextRange::contains` sense. See `Span::new`'s doc comment.
        let range = TextRange::new(3, 9).expect("valid range");

        assert!(Span::new(range, Some(9)).is_ok());
    }

    #[test]
    fn span_rejects_a_point_before_the_range_start() {
        let range = TextRange::new(10, 20).expect("valid range");

        assert_eq!(
            Span::new(range, Some(9)),
            Err(SpanError::PointOutOfRange { point: 9, range })
        );
    }

    #[test]
    fn span_rejects_a_point_after_the_range_end() {
        let range = TextRange::new(10, 20).expect("valid range");

        assert_eq!(
            Span::new(range, Some(21)),
            Err(SpanError::PointOutOfRange { point: 21, range })
        );
    }

    #[test]
    fn span_rejects_a_point_outside_the_range_even_when_zero() {
        let range = TextRange::new(10, 20).expect("valid range");

        assert_eq!(
            Span::new(range, Some(0)),
            Err(SpanError::PointOutOfRange { point: 0, range })
        );
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
