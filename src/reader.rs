use std::fmt;

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
    UnterminatedInteger {
        offset: usize,
    },
    IntegerOverflow {
        offset: usize,
    },
    InvalidUtf8 {
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
            Self::UnterminatedInteger { offset } => {
                write!(
                    formatter,
                    "unterminated base-128 integer at offset {offset}"
                )
            }
            Self::IntegerOverflow { offset } => {
                write!(formatter, "base-128 integer overflows at offset {offset}")
            }
            Self::InvalidUtf8 { offset } => {
                write!(formatter, "invalid UTF-8 at offset {offset}")
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

    pub fn read_u8(&mut self) -> Result<u8, ReadError> {
        if self.is_at_end() {
            return Err(ReadError::UnexpectedEof {
                offset: self.offset,
                needed: 1,
                remaining: 0,
            });
        }

        let byte = self.bytes[self.offset];
        self.offset += 1;
        Ok(byte)
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

    pub fn read_nat(&mut self) -> Result<u32, ReadError> {
        let (value, _) = self.read_base128()?;
        u32::try_from(value).map_err(|_| ReadError::IntegerOverflow {
            offset: self.offset,
        })
    }

    pub fn read_long_nat(&mut self) -> Result<u64, ReadError> {
        let (value, _) = self.read_base128()?;
        u64::try_from(value).map_err(|_| ReadError::IntegerOverflow {
            offset: self.offset,
        })
    }

    pub fn read_int(&mut self) -> Result<i32, ReadError> {
        let value = self.read_signed()?;
        i32::try_from(value).map_err(|_| ReadError::IntegerOverflow {
            offset: self.offset,
        })
    }

    pub fn read_long_int(&mut self) -> Result<i64, ReadError> {
        self.read_signed()
    }

    pub fn read_utf8(&mut self) -> Result<String, ReadError> {
        let start = self.offset;
        let length = usize::try_from(self.read_nat()?)
            .map_err(|_| ReadError::IntegerOverflow { offset: start })?;
        let bytes = self.read_bytes(length)?;

        String::from_utf8(bytes.to_vec()).map_err(|_| ReadError::InvalidUtf8 { offset: start })
    }

    fn read_base128(&mut self) -> Result<(u128, usize), ReadError> {
        let start = self.offset;
        let mut value = 0u128;

        for group_count in 0..10 {
            let byte = match self.read_u8() {
                Ok(byte) => byte,
                Err(error) if group_count > 0 => {
                    return match error {
                        ReadError::UnexpectedEof { .. } => {
                            Err(ReadError::UnterminatedInteger { offset: start })
                        }
                        other => Err(other),
                    };
                }
                Err(error) => return Err(error),
            };

            value = (value << 7) | u128::from(byte & 0x7f);

            if byte & 0x80 != 0 {
                return Ok((value, group_count + 1));
            }
        }

        Err(ReadError::IntegerOverflow { offset: start })
    }

    fn read_signed(&mut self) -> Result<i64, ReadError> {
        let start = self.offset;
        let (raw, group_count) = self.read_base128()?;
        let bit_count = group_count * 7;
        let sign_bit = 1u128 << (bit_count - 1);
        let signed = if raw & sign_bit != 0 {
            raw as i128 - (1i128 << bit_count)
        } else {
            raw as i128
        };

        i64::try_from(signed).map_err(|_| ReadError::IntegerOverflow { offset: start })
    }
}

#[cfg(test)]
mod tests {
    use super::{ReadError, Reader};

    #[test]
    fn reads_bytes_and_tracks_position() {
        let mut reader = Reader::new(&[0x10, 0x20, 0x30]);

        assert_eq!(reader.position(), 0);
        assert_eq!(reader.read_u8().unwrap(), 0x10);
        assert_eq!(reader.read_bytes(2).unwrap(), &[0x20, 0x30]);
        assert_eq!(reader.position(), 3);
        assert!(reader.is_at_end());
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
    }

    #[test]
    fn reads_base128_natural_numbers() {
        let mut reader = Reader::new(&[0x80, 0xff, 0x01, 0x80]);

        assert_eq!(reader.read_nat().unwrap(), 0);
        assert_eq!(reader.read_nat().unwrap(), 127);
        assert_eq!(reader.read_nat().unwrap(), 128);
    }

    #[test]
    fn reads_signed_two_complement_numbers() {
        let mut reader = Reader::new(&[0xff, 0x00, 0xff, 0x01, 0x80]);

        assert_eq!(reader.read_long_int().unwrap(), -1);
        assert_eq!(reader.read_long_int().unwrap(), 127);
        assert_eq!(reader.read_long_int().unwrap(), 128);
    }

    #[test]
    fn reports_eof_and_unterminated_numbers() {
        let mut reader = Reader::new(&[]);
        assert!(matches!(
            reader.read_u8(),
            Err(ReadError::UnexpectedEof { .. })
        ));

        let mut reader = Reader::new(&[0x00]);
        assert_eq!(
            reader.read_nat(),
            Err(ReadError::UnterminatedInteger { offset: 0 })
        );
    }

    #[test]
    fn reads_length_prefixed_utf8() {
        let mut reader = Reader::new(&[
            0x8b, b'S', b'c', b'a', b'l', b'a', b' ', b'3', b'.', b'9', b'.', b'0',
        ]);

        assert_eq!(reader.read_utf8().unwrap(), "Scala 3.9.0");
    }

    #[test]
    fn rejects_invalid_utf8() {
        let mut reader = Reader::new(&[0x82, 0xff, 0xfe]);

        assert_eq!(
            reader.read_utf8(),
            Err(ReadError::InvalidUtf8 { offset: 0 })
        );
    }
}
