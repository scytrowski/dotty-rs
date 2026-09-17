use std::fmt;

/// A bounded, fixed-width big-endian byte reader for the class file format.
///
/// Unlike `dotty_tasty::reader::Reader`, there are no variable-length
/// integers in the class file format (`docs/classfile-format-jdk25.md` §1);
/// every field is a fixed-width `u1`/`u2`/`u4`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
    limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadError {
    InvalidRange {
        start: usize,
        end: usize,
        len: usize,
    },
    UnexpectedEof {
        offset: usize,
        needed: usize,
        remaining: usize,
    },
    InvalidModifiedUtf8 {
        offset: usize,
    },
}

impl fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRange { start, end, len } => {
                write!(
                    formatter,
                    "invalid reader range {start}..{end} for {len} bytes"
                )
            }
            Self::UnexpectedEof {
                offset,
                needed,
                remaining,
            } => write!(
                formatter,
                "unexpected end of input at offset {offset}: needed {needed} bytes, but only {remaining} remain"
            ),
            Self::InvalidModifiedUtf8 { offset } => {
                write!(formatter, "invalid modified UTF-8 at offset {offset}")
            }
        }
    }
}

impl std::error::Error for ReadError {}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            offset: 0,
            limit: bytes.len(),
        }
    }

    pub fn with_range(bytes: &'a [u8], start: usize, end: usize) -> Result<Self, ReadError> {
        if start > end || end > bytes.len() {
            return Err(ReadError::InvalidRange {
                start,
                end,
                len: bytes.len(),
            });
        }

        Ok(Self {
            bytes,
            offset: start,
            limit: end,
        })
    }

    pub fn position(&self) -> usize {
        self.offset
    }

    pub fn remaining(&self) -> usize {
        self.limit - self.offset
    }

    pub fn is_at_end(&self) -> bool {
        self.offset == self.limit
    }

    pub fn read_bytes(&mut self, length: usize) -> Result<&'a [u8], ReadError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(ReadError::UnexpectedEof {
                offset: self.offset,
                needed: length,
                remaining: self.remaining(),
            })?;

        if end > self.limit {
            return Err(ReadError::UnexpectedEof {
                offset: self.offset,
                needed: length,
                remaining: self.remaining(),
            });
        }

        let bytes = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(bytes)
    }

    pub fn read_sub_reader(&mut self, length: usize) -> Result<Reader<'a>, ReadError> {
        let start = self.offset;
        let end = start.checked_add(length).ok_or(ReadError::UnexpectedEof {
            offset: start,
            needed: length,
            remaining: self.remaining(),
        })?;

        if end > self.limit {
            return Err(ReadError::UnexpectedEof {
                offset: start,
                needed: length,
                remaining: self.remaining(),
            });
        }

        self.offset = end;
        Ok(Reader {
            bytes: self.bytes,
            offset: start,
            limit: end,
        })
    }

    pub fn read_u8(&mut self) -> Result<u8, ReadError> {
        Ok(self.read_bytes(1)?[0])
    }

    pub fn read_u16(&mut self) -> Result<u16, ReadError> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    pub fn read_u32(&mut self) -> Result<u32, ReadError> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Decodes a length-bounded modified UTF-8 byte sequence (JVMS §4.4.7):
    /// standard 1/2/3-byte sequences, the 2-byte encoding of embedded NUL,
    /// and the 6-byte encoding of a supplementary character as a surrogate
    /// pair of 3-byte sequences. Never panics on malformed input.
    pub fn read_modified_utf8(&mut self, length: usize) -> Result<String, ReadError> {
        let start = self.offset;
        let bytes = self.read_bytes(length)?;
        decode_modified_utf8(bytes, start)
    }
}

fn decode_modified_utf8(bytes: &[u8], base_offset: usize) -> Result<String, ReadError> {
    let invalid_at = |offset: usize| ReadError::InvalidModifiedUtf8 { offset };
    let mut result = String::new();
    let mut index = 0;

    while index < bytes.len() {
        let lead = bytes[index];

        if lead & 0x80 == 0 {
            if lead == 0 {
                return Err(invalid_at(base_offset + index));
            }
            result.push(lead as char);
            index += 1;
        } else if lead & 0xE0 == 0xC0 {
            let continuation = *bytes
                .get(index + 1)
                .ok_or_else(|| invalid_at(base_offset + index))?;
            if continuation & 0xC0 != 0x80 {
                return Err(invalid_at(base_offset + index));
            }

            let value = (u32::from(lead & 0x1F) << 6) | u32::from(continuation & 0x3F);
            // JVMS §4.4.7: the 2-byte form encodes 0x0000 (the special NUL
            // case) or 0x0080-0x07FF; 0x0001-0x007F must use the 1-byte form.
            if (1..0x80).contains(&value) {
                return Err(invalid_at(base_offset + index));
            }
            result.push(char::from_u32(value).ok_or_else(|| invalid_at(base_offset + index))?);
            index += 2;
        } else if lead & 0xF0 == 0xE0 {
            let value = read_three_byte_sequence(bytes, index, base_offset)?;

            if (0xD800..=0xDBFF).contains(&value) {
                let low = read_three_byte_sequence(bytes, index + 3, base_offset)?;
                if !(0xDC00..=0xDFFF).contains(&low) {
                    return Err(invalid_at(base_offset + index));
                }
                let code_point = 0x10000 + ((value - 0xD800) << 10) + (low - 0xDC00);
                result.push(
                    char::from_u32(code_point).ok_or_else(|| invalid_at(base_offset + index))?,
                );
                index += 6;
            } else if (0xDC00..=0xDFFF).contains(&value) {
                return Err(invalid_at(base_offset + index));
            } else {
                // JVMS §4.4.7: the plain 3-byte form encodes 0x0800-0xFFFF;
                // anything below 0x0800 must use the 1- or 2-byte form.
                if value < 0x800 {
                    return Err(invalid_at(base_offset + index));
                }
                result.push(char::from_u32(value).ok_or_else(|| invalid_at(base_offset + index))?);
                index += 3;
            }
        } else {
            return Err(invalid_at(base_offset + index));
        }
    }

    Ok(result)
}

fn read_three_byte_sequence(
    bytes: &[u8],
    index: usize,
    base_offset: usize,
) -> Result<u32, ReadError> {
    let invalid = ReadError::InvalidModifiedUtf8 {
        offset: base_offset + index,
    };
    let lead = *bytes.get(index).ok_or(invalid)?;
    let byte1 = *bytes.get(index + 1).ok_or(invalid)?;
    let byte2 = *bytes.get(index + 2).ok_or(invalid)?;

    if lead & 0xF0 != 0xE0 || byte1 & 0xC0 != 0x80 || byte2 & 0xC0 != 0x80 {
        return Err(invalid);
    }

    Ok((u32::from(lead & 0x0F) << 12) | (u32::from(byte1 & 0x3F) << 6) | u32::from(byte2 & 0x3F))
}

#[cfg(test)]
mod tests {
    use super::{ReadError, Reader};

    #[test]
    fn reads_fixed_width_big_endian_integers() {
        let mut reader = Reader::new(&[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07]);

        assert_eq!(reader.read_u8().unwrap(), 0x01);
        assert_eq!(reader.read_u16().unwrap(), 0x0203);
        assert_eq!(reader.read_u32().unwrap(), 0x0405_0607);
        assert!(reader.is_at_end());
    }

    #[test]
    fn reports_eof_at_each_width() {
        assert_eq!(
            Reader::new(&[]).read_u8(),
            Err(ReadError::UnexpectedEof {
                offset: 0,
                needed: 1,
                remaining: 0,
            })
        );
        assert_eq!(
            Reader::new(&[0x01]).read_u16(),
            Err(ReadError::UnexpectedEof {
                offset: 0,
                needed: 2,
                remaining: 1,
            })
        );
        assert_eq!(
            Reader::new(&[0x01, 0x02, 0x03]).read_u32(),
            Err(ReadError::UnexpectedEof {
                offset: 0,
                needed: 4,
                remaining: 3,
            })
        );
    }

    #[test]
    fn respects_bounded_ranges() {
        let bytes = [0x10, 0x20, 0x30];
        let mut reader = Reader::with_range(&bytes, 1, 3).unwrap();

        assert_eq!(reader.read_bytes(2).unwrap(), &[0x20, 0x30]);
        assert_eq!(reader.position(), 3);
    }

    #[test]
    fn rejects_invalid_ranges() {
        let bytes = [0x10, 0x20, 0x30];

        assert_eq!(
            Reader::with_range(&bytes, 3, 2),
            Err(ReadError::InvalidRange {
                start: 3,
                end: 2,
                len: 3,
            })
        );
        assert_eq!(
            Reader::with_range(&bytes, 0, 4),
            Err(ReadError::InvalidRange {
                start: 0,
                end: 4,
                len: 3,
            })
        );
    }

    #[test]
    fn sub_readers_are_bounded_and_advance_the_parent() {
        let bytes = [0x10, 0x20, 0x30, 0x40];
        let mut reader = Reader::new(&bytes);

        let mut sub_reader = reader.read_sub_reader(2).unwrap();
        assert_eq!(sub_reader.read_bytes(2).unwrap(), &[0x10, 0x20]);
        assert!(sub_reader.is_at_end());
        assert_eq!(reader.position(), 2);

        assert_eq!(
            reader.read_sub_reader(3),
            Err(ReadError::UnexpectedEof {
                offset: 2,
                needed: 3,
                remaining: 2,
            })
        );
    }

    #[test]
    fn decodes_plain_ascii_modified_utf8() {
        let mut reader = Reader::new(b"Scala 3.9.0");

        assert_eq!(reader.read_modified_utf8(11).unwrap(), "Scala 3.9.0");
    }

    #[test]
    fn decodes_the_two_byte_encoding_of_embedded_nul() {
        let mut reader = Reader::new(&[0xC0, 0x80]);

        assert_eq!(reader.read_modified_utf8(2).unwrap(), "\0");
    }

    #[test]
    fn decodes_the_six_byte_surrogate_pair_encoding_of_a_supplementary_character() {
        let mut reader = Reader::new(&[0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80]);

        assert_eq!(reader.read_modified_utf8(6).unwrap(), "\u{1F600}");
    }

    #[test]
    fn rejects_a_truncated_multi_byte_sequence() {
        let mut reader = Reader::new(&[0xE0, 0x80]);

        assert_eq!(
            reader.read_modified_utf8(2),
            Err(ReadError::InvalidModifiedUtf8 { offset: 0 })
        );
    }

    #[test]
    fn rejects_a_stray_continuation_byte() {
        let mut reader = Reader::new(&[0x80]);

        assert_eq!(
            reader.read_modified_utf8(1),
            Err(ReadError::InvalidModifiedUtf8 { offset: 0 })
        );
    }

    #[test]
    fn rejects_a_literal_nul_byte() {
        let mut reader = Reader::new(&[0x41, 0x00]);

        assert_eq!(
            reader.read_modified_utf8(2),
            Err(ReadError::InvalidModifiedUtf8 { offset: 1 })
        );
    }

    #[test]
    fn rejects_an_overlong_two_byte_encoding_of_an_ascii_character() {
        // 0xC1 0x81 encodes 'A' (0x41) via the 2-byte form; JVMS §4.4.7
        // requires 0x0001-0x007F to use the 1-byte form.
        let mut reader = Reader::new(&[0xC1, 0x81]);

        assert_eq!(
            reader.read_modified_utf8(2),
            Err(ReadError::InvalidModifiedUtf8 { offset: 0 })
        );
    }

    #[test]
    fn rejects_an_overlong_three_byte_encoding_of_an_ascii_character() {
        // 0xE0 0x81 0x81 encodes 'A' (0x41) via the 3-byte form; JVMS
        // §4.4.7 requires 0x0000-0x07FF to use the 1- or 2-byte form.
        let mut reader = Reader::new(&[0xE0, 0x81, 0x81]);

        assert_eq!(
            reader.read_modified_utf8(3),
            Err(ReadError::InvalidModifiedUtf8 { offset: 0 })
        );
    }

    #[test]
    fn rejects_a_disallowed_lead_byte() {
        let mut reader = Reader::new(&[0xF0, 0x80, 0x80, 0x80]);

        assert_eq!(
            reader.read_modified_utf8(4),
            Err(ReadError::InvalidModifiedUtf8 { offset: 0 })
        );
    }

    #[test]
    fn rejects_an_unpaired_high_surrogate() {
        let mut reader = Reader::new(&[0xED, 0xA0, 0xBD, 0x41]);

        assert_eq!(
            reader.read_modified_utf8(4),
            Err(ReadError::InvalidModifiedUtf8 { offset: 3 })
        );
    }
}
