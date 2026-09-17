use core::fmt;

use dotty_source::{SourceText, SourceTextError, TextRange};

/// A UTF-8 source cursor with byte-based positions and character lookahead.
#[derive(Debug, Clone, Copy)]
pub struct Cursor<'source> {
    source: &'source str,
    offset: u32,
}

/// Failure while moving a source cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorError {
    SourceTooLong { byte_len: usize },
    InvalidOffset { offset: u32, byte_len: u32 },
    NotUtf8Boundary { offset: u32 },
}

impl fmt::Display for CursorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceTooLong { byte_len } => {
                write!(
                    formatter,
                    "source is too long for a 32-bit byte offset: {byte_len} bytes"
                )
            }
            Self::InvalidOffset { offset, byte_len } => {
                write!(
                    formatter,
                    "byte offset {offset} exceeds source length {byte_len}"
                )
            }
            Self::NotUtf8Boundary { offset } => {
                write!(formatter, "byte offset {offset} is not a UTF-8 boundary")
            }
        }
    }
}

impl std::error::Error for CursorError {}

impl<'source> Cursor<'source> {
    /// Creates a cursor at the beginning of a UTF-8 source buffer.
    pub fn new(source: &'source str) -> Result<Self, CursorError> {
        if source.len() > u32::MAX as usize {
            return Err(CursorError::SourceTooLong {
                byte_len: source.len(),
            });
        }

        Ok(Self { source, offset: 0 })
    }

    /// Creates a cursor over checked source text.
    pub fn from_source(source: &SourceText<'source>) -> Self {
        Self {
            source: source.as_str(),
            offset: 0,
        }
    }

    /// Returns the current byte offset.
    pub const fn position(self) -> u32 {
        self.offset
    }

    /// Returns the source length in bytes.
    pub const fn len_bytes(self) -> u32 {
        self.source.len() as u32
    }

    /// Returns whether the cursor has reached EOF.
    pub const fn is_eof(self) -> bool {
        self.offset == self.len_bytes()
    }

    /// Returns the remaining source text.
    pub fn remaining(self) -> &'source str {
        &self.source[self.offset as usize..]
    }

    /// Returns the next Unicode scalar value without advancing.
    pub fn peek(self) -> Option<char> {
        self.remaining().chars().next()
    }

    /// Returns the `n`th Unicode scalar value without advancing.
    pub fn peek_nth(self, n: usize) -> Option<char> {
        self.remaining().chars().nth(n)
    }

    /// Consumes and returns the next Unicode scalar value.
    pub fn bump(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.offset += character.len_utf8() as u32;
        Some(character)
    }

    /// Consumes `expected` if it is the next character.
    pub fn eat_if(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            let _ = self.bump();
            true
        } else {
            false
        }
    }

    /// Saves the current byte offset for later restoration.
    pub const fn checkpoint(self) -> u32 {
        self.offset
    }

    /// Restores the cursor to a UTF-8 boundary within the source.
    pub fn reset(&mut self, offset: u32) -> Result<(), CursorError> {
        if offset > self.len_bytes() {
            return Err(CursorError::InvalidOffset {
                offset,
                byte_len: self.len_bytes(),
            });
        }
        if !self.source.is_char_boundary(offset as usize) {
            return Err(CursorError::NotUtf8Boundary { offset });
        }

        self.offset = offset;
        Ok(())
    }

    /// Returns the source range between `start` and the current cursor.
    pub fn range_from(&self, start: u32) -> Result<TextRange, CursorError> {
        if start > self.offset {
            return Err(CursorError::InvalidOffset {
                offset: start,
                byte_len: self.offset,
            });
        }

        TextRange::new(start, self.offset).map_err(|_| CursorError::InvalidOffset {
            offset: start,
            byte_len: self.offset,
        })
    }

    /// Returns a checked source slice for the range from `start` to the cursor.
    pub fn slice_from(&self, start: u32) -> Result<&'source str, SourceTextError> {
        let range = self.range_from(start).map_err(|error| match error {
            CursorError::SourceTooLong { byte_len } => SourceTextError::TextTooLong { byte_len },
            CursorError::InvalidOffset { offset, byte_len } => {
                SourceTextError::OffsetOutOfBounds { offset, byte_len }
            }
            CursorError::NotUtf8Boundary { offset } => SourceTextError::NotUtf8Boundary { offset },
        })?;

        SourceText::new(self.source)?.slice(range)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_source::SourceText;

    #[test]
    fn tracks_utf8_positions_in_bytes() {
        let mut cursor = Cursor::new("żx").expect("valid source");

        assert_eq!(cursor.position(), 0);
        assert_eq!(cursor.bump(), Some('ż'));
        assert_eq!(cursor.position(), 2);
        assert_eq!(cursor.bump(), Some('x'));
        assert_eq!(cursor.position(), 3);
        assert!(cursor.is_eof());
    }

    #[test]
    fn lookahead_does_not_advance_the_cursor() {
        let cursor = Cursor::new("abc").expect("valid source");

        assert_eq!(cursor.peek(), Some('a'));
        assert_eq!(cursor.peek_nth(2), Some('c'));
        assert_eq!(cursor.position(), 0);
    }

    #[test]
    fn consumes_a_character_only_when_it_matches() {
        let mut cursor = Cursor::new("ab").expect("valid source");

        assert!(!cursor.eat_if('b'));
        assert_eq!(cursor.position(), 0);
        assert!(cursor.eat_if('a'));
        assert_eq!(cursor.position(), 1);
    }

    #[test]
    fn restores_a_checkpoint() {
        let mut cursor = Cursor::new("abc").expect("valid source");
        let checkpoint = cursor.checkpoint();
        assert_eq!(cursor.bump(), Some('a'));

        cursor.reset(checkpoint).expect("checkpoint is valid");
        assert_eq!(cursor.position(), 0);
        assert_eq!(cursor.peek(), Some('a'));
    }

    #[test]
    fn rejects_an_offset_inside_a_utf8_character() {
        let mut cursor = Cursor::new("ż").expect("valid source");

        assert_eq!(
            cursor.reset(1),
            Err(CursorError::NotUtf8Boundary { offset: 1 })
        );
    }

    #[test]
    fn returns_the_range_and_slice_since_a_checkpoint() {
        let source = SourceText::new("abc").expect("valid source");
        let mut cursor = Cursor::from_source(&source);
        let start = cursor.checkpoint();
        assert_eq!(cursor.bump(), Some('a'));
        assert_eq!(cursor.bump(), Some('b'));

        assert_eq!(
            cursor.range_from(start),
            Ok(TextRange::new(0, 2).expect("valid range"))
        );
        assert_eq!(cursor.slice_from(start), Ok("ab"));
    }

    #[test]
    fn reaches_eof_after_crlf_without_special_cursor_state() {
        let mut cursor = Cursor::new("a\r\nb").expect("valid source");
        while cursor.bump().is_some() {}

        assert!(cursor.is_eof());
        assert_eq!(cursor.position(), 4);
    }
}
