//! Byte offsets and editor positions in one immutable version of a document.
use super::{LineIndex, SourceTextError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Unit used to count columns when converting source byte offsets to editor positions.
pub enum PositionEncoding {
    /// Columns count UTF-8 bytes.
    Utf8,
    /// Columns count UTF-16 code units, as used by common editor protocols.
    Utf16,
}

/// Owns a text buffer and indexes its LF, CR and CRLF line endings once.
#[derive(Debug)]
pub struct DocumentText {
    text: String,
    lines: LineIndex,
}

impl DocumentText {
    /// Owns a UTF-8 buffer and builds its line index. Returns SourceTextError if the text exceeds
    /// the supported 32-bit offset range.
    pub fn new(text: String) -> Result<Self, SourceTextError> {
        let lines = LineIndex::build(&text, |_| false)?;
        Ok(Self { text, lines })
    }

    /// Borrows the unchanged UTF-8 document contents, including original line endings.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    fn line_text(&self, line: u32) -> Option<(u32, &str)> {
        let start = self.lines.line_start(line as usize)?;
        let end = (line as usize)
            .checked_add(1)
            .and_then(|next| self.lines.line_start(next))
            .unwrap_or(self.text.len() as u32);
        Some((
            start,
            self.text[start as usize..end as usize].trim_end_matches(['\r', '\n']),
        ))
    }

    /// Maps a scalar boundary to a zero-based line and code-unit column.
    /// The interior of a CRLF sequence has no editor position.
    pub fn position(&self, offset: u32, encoding: PositionEncoding) -> Option<(u32, u32)> {
        let line = self.lines.line_of_offset(offset).ok()? as u32;
        let (start, text) = self.line_text(line)?;
        let prefix = text.get(..(offset - start) as usize)?;
        let column = match encoding {
            PositionEncoding::Utf8 => prefix.len(),
            PositionEncoding::Utf16 => prefix.encode_utf16().count(),
        };
        Some((line, column as u32))
    }

    /// Maps a code-unit column to UTF-8 bytes. Overlong columns select the line end.
    /// An unknown line or the interior of a multi-unit character yields `None`.
    pub fn offset(&self, line: u32, column: u32, encoding: PositionEncoding) -> Option<u32> {
        let (start, text) = self.line_text(line)?;
        if encoding == PositionEncoding::Utf8 {
            let byte = (column as usize).min(text.len());
            return text.is_char_boundary(byte).then_some(start + byte as u32);
        }
        let mut units = 0;
        for (byte, c) in text.char_indices() {
            if units == column {
                return Some(start + byte as u32);
            }
            units += c.len_utf16() as u32;
            if units > column {
                return None;
            }
        }
        Some(start + text.len() as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editor_positions_round_trip_across_unicode_and_crlf() {
        let doc = DocumentText::new("ż😀\r\nx\u{c}y\r".into()).unwrap();
        for (enc, width) in [(PositionEncoding::Utf8, 6), (PositionEncoding::Utf16, 3)] {
            for (offset, line, column) in
                [(0, 0, 0), (6, 0, width), (8, 1, 0), (11, 1, 3), (12, 2, 0)]
            {
                assert_eq!(doc.position(offset, enc), Some((line, column)));
                assert_eq!(doc.offset(line, column, enc), Some(offset));
            }
            assert_eq!(doc.offset(0, 999, enc), Some(6));
            assert_eq!(doc.position(7, enc), None);
            assert_eq!(doc.offset(3, 0, enc), None);
        }
        assert_eq!(doc.offset(0, 2, PositionEncoding::Utf16), None);
        assert_eq!(doc.offset(0, 1, PositionEncoding::Utf8), None);
    }
}
