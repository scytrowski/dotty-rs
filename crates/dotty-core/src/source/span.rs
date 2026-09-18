use core::fmt;

/// A half-open byte range into a UTF-8 source buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextRange {
    start: u32,
    end: u32,
}

/// Failure while constructing a source range.
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

    /// Returns the first byte offset in the range.
    pub const fn start(self) -> u32 {
        self.start
    }

    /// Returns the exclusive end byte offset of the range.
    pub const fn end(self) -> u32 {
        self.end
    }

    /// Returns the range length in bytes.
    pub const fn len(self) -> u32 {
        self.end - self.start
    }

    /// Returns whether the range contains no bytes.
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Returns whether `offset` is inside the range.
    pub const fn contains(self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }

    /// Returns whether two non-empty ranges overlap.
    pub const fn intersects(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }

    /// Returns the smallest range containing both ranges.
    pub const fn cover(self, other: Self) -> Self {
        Self {
            start: if self.start < other.start {
                self.start
            } else {
                other.start
            },
            end: if self.end > other.end {
                self.end
            } else {
                other.end
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_a_half_open_range() {
        let range = TextRange::new(3, 7).expect("valid range");

        assert_eq!(range.start(), 3);
        assert_eq!(range.end(), 7);
        assert_eq!(range.len(), 4);
        assert!(range.contains(3));
        assert!(range.contains(6));
        assert!(!range.contains(7));
    }

    #[test]
    fn accepts_an_empty_range() {
        let range = TextRange::new(4, 4).expect("empty range is valid");

        assert!(range.is_empty());
        assert!(!range.contains(4));
    }

    #[test]
    fn rejects_a_range_whose_end_precedes_its_start() {
        assert_eq!(
            TextRange::new(8, 2),
            Err(TextRangeError::EndBeforeStart { start: 8, end: 2 })
        );
    }

    #[test]
    fn distinguishes_adjacent_ranges_from_overlapping_ranges() {
        let first = TextRange::new(0, 2).expect("valid range");
        let adjacent = TextRange::new(2, 4).expect("valid range");
        let overlapping = TextRange::new(1, 4).expect("valid range");

        assert!(!first.intersects(adjacent));
        assert!(first.intersects(overlapping));
    }

    #[test]
    fn covers_two_ranges() {
        let first = TextRange::new(3, 5).expect("valid range");
        let second = TextRange::new(1, 9).expect("valid range");

        assert_eq!(first.cover(second), second);
    }
}
