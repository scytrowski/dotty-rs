use core::fmt;

use crate::{SourceTextError, is_line_break_char};

/// Logical line starts for a UTF-8 source buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    line_starts: Vec<u32>,
    byte_len: u32,
}

impl LineIndex {
    /// Builds an index whose first line starts at byte offset zero.
    pub fn new(source: &str) -> Result<Self, SourceTextError> {
        if source.len() > u32::MAX as usize {
            return Err(SourceTextError::TextTooLong {
                byte_len: source.len(),
            });
        }

        let mut line_starts = vec![0];
        let mut bytes = source.char_indices().peekable();

        while let Some((offset, character)) = bytes.next() {
            match character {
                '\n' => line_starts.push((offset + character.len_utf8()) as u32),
                '\r' => {
                    if matches!(bytes.peek(), Some((_, '\n'))) {
                        if let Some((newline_offset, newline)) = bytes.next() {
                            line_starts.push((newline_offset + newline.len_utf8()) as u32);
                        }
                    } else {
                        line_starts.push((offset + character.len_utf8()) as u32);
                    }
                }
                character if is_line_break_char(character) => {
                    line_starts.push((offset + character.len_utf8()) as u32);
                }
                _ => {}
            }
        }

        Ok(Self {
            line_starts,
            byte_len: source.len() as u32,
        })
    }

    /// Returns the number of logical lines, including the final empty line.
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Returns the byte offset at which `line` starts.
    pub fn line_start(&self, line: usize) -> Option<u32> {
        self.line_starts.get(line).copied()
    }

    /// Returns the zero-based line containing `offset`.
    pub fn line_of_offset(&self, offset: u32) -> Result<usize, LineIndexError> {
        if offset > self.byte_len {
            return Err(LineIndexError::OffsetOutOfBounds {
                offset,
                byte_len: self.byte_len,
            });
        }

        let line = self.line_starts.partition_point(|&start| start <= offset);
        Ok(line.saturating_sub(1))
    }

    /// Returns the zero-based UTF-8 column for a source offset.
    pub fn utf8_column(&self, source: &str, offset: u32) -> Result<u32, SourceTextError> {
        let line = self.validated_line(source, offset)?;
        let start = self.line_starts[line] as usize;
        Ok(source[start..offset as usize].chars().count() as u32)
    }

    /// Returns the zero-based UTF-16 column for a source offset.
    pub fn utf16_column(&self, source: &str, offset: u32) -> Result<u32, SourceTextError> {
        let line = self.validated_line(source, offset)?;
        let start = self.line_starts[line] as usize;
        Ok(source[start..offset as usize]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>() as u32)
    }

    fn validated_line(&self, source: &str, offset: u32) -> Result<usize, SourceTextError> {
        if source.len() != self.byte_len as usize {
            return Err(SourceTextError::OffsetOutOfBounds {
                offset,
                byte_len: self.byte_len,
            });
        }
        let source_index = Self::new(source)?;
        if source_index.line_starts != self.line_starts {
            return Err(SourceTextError::LineLayoutMismatch {
                byte_len: self.byte_len,
            });
        }
        if offset > self.byte_len {
            return Err(SourceTextError::OffsetOutOfBounds {
                offset,
                byte_len: self.byte_len,
            });
        }
        if !source.is_char_boundary(offset as usize) {
            return Err(SourceTextError::NotUtf8Boundary { offset });
        }

        let line = self.line_of_offset(offset).map_err(|error| match error {
            LineIndexError::OffsetOutOfBounds { offset, byte_len } => {
                SourceTextError::OffsetOutOfBounds { offset, byte_len }
            }
        })?;
        let line_start = self.line_starts[line];
        if !source.is_char_boundary(line_start as usize) {
            return Err(SourceTextError::NotUtf8Boundary { offset: line_start });
        }

        Ok(line)
    }
}

/// Failure while querying a line index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineIndexError {
    OffsetOutOfBounds { offset: u32, byte_len: u32 },
}

impl fmt::Display for LineIndexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OffsetOutOfBounds { offset, byte_len } => {
                write!(
                    formatter,
                    "byte offset {offset} exceeds source length {byte_len}"
                )
            }
        }
    }
}

impl std::error::Error for LineIndexError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_lf_and_crlf_as_single_line_breaks() {
        let source = "one\ntwo\r\nthree";
        let index = LineIndex::new(source).expect("valid source");

        assert_eq!(index.line_count(), 3);
        assert_eq!(index.line_start(0), Some(0));
        assert_eq!(index.line_start(1), Some(4));
        assert_eq!(index.line_start(2), Some(9));
        assert_eq!(index.line_of_offset(9), Ok(2));
    }

    #[test]
    fn indexes_form_feed_as_a_line_break() {
        let source = "one\u{000c}two";
        let index = LineIndex::new(source).expect("valid source");

        assert_eq!(index.line_count(), 2);
        assert_eq!(index.line_start(1), Some(4));
        assert_eq!(index.line_of_offset(4), Ok(1));
    }

    #[test]
    fn indexes_substitute_as_a_line_break() {
        let source = "one\u{001a}two";
        let index = LineIndex::new(source).expect("valid source");

        assert_eq!(index.line_count(), 2);
        assert_eq!(index.line_start(1), Some(4));
        assert_eq!(index.line_of_offset(4), Ok(1));
    }

    #[test]
    fn indexes_a_trailing_newline_as_a_final_empty_line() {
        let index = LineIndex::new("a\n").expect("valid source");

        assert_eq!(index.line_count(), 2);
        assert_eq!(index.line_start(1), Some(2));
        assert_eq!(index.line_of_offset(2), Ok(1));
    }

    #[test]
    fn calculates_utf8_and_utf16_columns() {
        let source = "ż😀x";
        let index = LineIndex::new(source).expect("valid source");
        let offset_after_emoji = "ż😀".len() as u32;

        assert_eq!(index.utf8_column(source, offset_after_emoji), Ok(2));
        assert_eq!(index.utf16_column(source, offset_after_emoji), Ok(3));
    }

    #[test]
    fn rejects_a_column_inside_a_utf8_character() {
        let source = "ż";
        let index = LineIndex::new(source).expect("valid source");

        assert_eq!(
            index.utf8_column(source, 1),
            Err(SourceTextError::NotUtf8Boundary { offset: 1 })
        );
    }

    #[test]
    fn rejects_a_utf8_column_for_a_different_line_layout() {
        let index = LineIndex::new("a\nb").expect("valid source");

        assert_eq!(
            index.utf8_column("a\u{0301}", 3),
            Err(SourceTextError::LineLayoutMismatch { byte_len: 3 })
        );
    }

    #[test]
    fn rejects_a_utf16_column_for_a_different_line_layout() {
        let index = LineIndex::new("a\nb").expect("valid source");

        assert_eq!(
            index.utf16_column("a\u{0301}", 3),
            Err(SourceTextError::LineLayoutMismatch { byte_len: 3 })
        );
    }

    #[test]
    fn rejects_offsets_after_the_source_end() {
        let index = LineIndex::new("abc").expect("valid source");

        assert_eq!(
            index.line_of_offset(4),
            Err(LineIndexError::OffsetOutOfBounds {
                offset: 4,
                byte_len: 3
            })
        );
    }
}
