use core::fmt;

use crate::{LineIndex, TextRange};

/// Returns whether a character is a Scala 3.9.0 physical line-break character.
pub const fn is_line_break_char(character: char) -> bool {
    matches!(character, '\n' | '\u{000c}' | '\r' | '\u{001a}')
}

/// A checked, borrowed view of UTF-8 source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceText<'source> {
    text: &'source str,
}

/// Failure while accessing source text by byte range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTextError {
    TextTooLong { byte_len: usize },
    RangeOutOfBounds { range: TextRange, byte_len: u32 },
    NotUtf8Boundary { offset: u32 },
    OffsetOutOfBounds { offset: u32, byte_len: u32 },
}

impl fmt::Display for SourceTextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TextTooLong { byte_len } => {
                write!(
                    formatter,
                    "source is too long for a 32-bit byte offset: {byte_len} bytes"
                )
            }
            Self::RangeOutOfBounds { range, byte_len } => write!(
                formatter,
                "source range [{}, {}) exceeds source length {byte_len}",
                range.start(),
                range.end()
            ),
            Self::NotUtf8Boundary { offset } => {
                write!(formatter, "byte offset {offset} is not a UTF-8 boundary")
            }
            Self::OffsetOutOfBounds { offset, byte_len } => {
                write!(
                    formatter,
                    "byte offset {offset} exceeds source length {byte_len}"
                )
            }
        }
    }
}

impl std::error::Error for SourceTextError {}

impl<'source> SourceText<'source> {
    /// Creates a checked view over UTF-8 source text.
    pub fn new(text: &'source str) -> Result<Self, SourceTextError> {
        if text.len() > u32::MAX as usize {
            return Err(SourceTextError::TextTooLong {
                byte_len: text.len(),
            });
        }

        Ok(Self { text })
    }

    /// Returns the original source text.
    pub const fn as_str(self) -> &'source str {
        self.text
    }

    /// Returns the source length in bytes.
    pub const fn len_bytes(self) -> u32 {
        self.text.len() as u32
    }

    /// Returns whether the source has no bytes.
    pub const fn is_empty(self) -> bool {
        self.text.is_empty()
    }

    /// Returns a checked source slice for `range`.
    pub fn slice(self, range: TextRange) -> Result<&'source str, SourceTextError> {
        if range.end() > self.len_bytes() {
            return Err(SourceTextError::RangeOutOfBounds {
                range,
                byte_len: self.len_bytes(),
            });
        }

        if !self.text.is_char_boundary(range.start() as usize) {
            return Err(SourceTextError::NotUtf8Boundary {
                offset: range.start(),
            });
        }
        if !self.text.is_char_boundary(range.end() as usize) {
            return Err(SourceTextError::NotUtf8Boundary {
                offset: range.end(),
            });
        }

        Ok(&self.text[range.start() as usize..range.end() as usize])
    }

    /// Builds a line index for this source.
    pub fn line_index(self) -> Result<LineIndex, SourceTextError> {
        LineIndex::new(self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_all_scala_line_break_characters() {
        for character in ['\n', '\u{000c}', '\r', '\u{001a}'] {
            assert!(is_line_break_char(character), "{character:?}");
        }
    }

    #[test]
    fn rejects_non_line_break_whitespace() {
        assert!(!is_line_break_char(' '));
        assert!(!is_line_break_char('\t'));
    }

    #[test]
    fn keeps_utf8_source_ranges_in_bytes() {
        let source = SourceText::new("żółw").expect("valid source");
        let range = TextRange::new(0, 2).expect("first UTF-8 character");

        assert_eq!(source.slice(range), Ok("ż"));
        assert_eq!(source.len_bytes(), 7);
    }

    #[test]
    fn rejects_a_slice_that_splits_a_utf8_character() {
        let source = SourceText::new("ż").expect("valid source");
        let range = TextRange::new(0, 1).expect("range order is valid");

        assert_eq!(
            source.slice(range),
            Err(SourceTextError::NotUtf8Boundary { offset: 1 })
        );
    }

    #[test]
    fn rejects_a_slice_past_the_source_end() {
        let source = SourceText::new("abc").expect("valid source");
        let range = TextRange::new(1, 4).expect("range order is valid");

        assert_eq!(
            source.slice(range),
            Err(SourceTextError::RangeOutOfBounds { range, byte_len: 3 })
        );
    }

    #[test]
    fn accepts_an_empty_source_and_empty_slice() {
        let source = SourceText::new("").expect("valid source");
        let range = TextRange::new(0, 0).expect("empty range");

        assert!(source.is_empty());
        assert_eq!(source.slice(range), Ok(""));
    }
}
