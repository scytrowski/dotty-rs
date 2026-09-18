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
//! [`TextRange`] here is a **temporary stand-in** for `dotty_source::TextRange`,
//! not a permanent parallel implementation.
//!
//! `dotty-core` was branched from `main` while the `dotty-source` crate still
//! lives only on the not-yet-merged `feature/lexer` branch (still 70+ commits
//! ahead of `main`, still under active development by another agent as of
//! this writing), so it cannot be a path dependency yet without coupling
//! `dotty-core` to another team's in-flight branch.
//!
//! `new`/`start`/`end` here match `dotty_source::TextRange`'s signatures
//! field-for-field and byte-for-byte (verified against
//! `feature/lexer`'s `crates/dotty-source/src/span.rs`; pinned by
//! `local_text_range_matches_dotty_sources_public_constructor_shape` below),
//! so every call site in this crate keeps compiling unchanged after the swap.
//! The real type additionally has `len`, `is_empty`, `contains`,
//! `intersects`, and `cover`; this stand-in deliberately does not replicate
//! them because nothing in `dotty-core` needs them yet — if a caller needs
//! one before the swap happens, add it here too so the two stay in lockstep.
//!
//! **Required once `feature/lexer` merges into `main`:**
//! 1. Add `dotty-source = { path = "../dotty-source" }` to
//!    `crates/dotty-core/Cargo.toml`.
//! 2. Delete `TextRange`/`TextRangeError` from this module and replace them
//!    with `pub use dotty_source::{TextRange, TextRangeError};`.
//! 3. Delete the shape-pinning test below (it becomes redundant — the real
//!    type is now used directly).
//!
//! See also `docs/dotty-core-design.md`, "§2 deviations."

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

    /// Pins this module's `TextRange::new`/`start`/`end` to the exact
    /// signatures `dotty_source::TextRange` exposes, so this file's module
    /// documentation stays true and the eventual swap (see above) is a pure
    /// deletion, not a call-site rewrite. Delete this test as part of that
    /// swap — see step 3 in the module documentation.
    #[test]
    fn local_text_range_matches_dotty_sources_public_constructor_shape() {
        fn assert_shape(_: fn(u32, u32) -> Result<TextRange, TextRangeError>) {}
        assert_shape(TextRange::new);

        let range = TextRange::new(5, 12).expect("valid range");
        let start: u32 = range.start();
        let end: u32 = range.end();
        assert_eq!((start, end), (5, 12));
    }

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
