use std::fmt;

/// A bounded, fixed-width little-endian byte reader for the ZIP file
/// format (used by JAR archives).
///
/// Unlike `dotty_classfile::reader::Reader` (big-endian, for the class
/// file format), every multi-byte ZIP field is little-endian per the ZIP
/// APPNOTE.TXT specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZipReader<'a> {
    bytes: &'a [u8],
    offset: usize,
    limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZipReadError {
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
}

impl fmt::Display for ZipReadError {
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
        }
    }
}

impl std::error::Error for ZipReadError {}

impl<'a> ZipReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            offset: 0,
            limit: bytes.len(),
        }
    }

    pub fn with_range(bytes: &'a [u8], start: usize, end: usize) -> Result<Self, ZipReadError> {
        if start > end || end > bytes.len() {
            return Err(ZipReadError::InvalidRange {
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

    pub fn read_bytes(&mut self, length: usize) -> Result<&'a [u8], ZipReadError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(ZipReadError::UnexpectedEof {
                offset: self.offset,
                needed: length,
                remaining: self.remaining(),
            })?;

        if end > self.limit {
            return Err(ZipReadError::UnexpectedEof {
                offset: self.offset,
                needed: length,
                remaining: self.remaining(),
            });
        }

        let bytes = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(bytes)
    }

    pub fn read_u8(&mut self) -> Result<u8, ZipReadError> {
        Ok(self.read_bytes(1)?[0])
    }

    pub fn read_u16(&mut self) -> Result<u16, ZipReadError> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub fn read_u32(&mut self) -> Result<u32, ZipReadError> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_fixed_width_little_endian_integers() {
        let mut reader = ZipReader::new(&[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07]);

        assert_eq!(reader.read_u8().unwrap(), 0x01);
        assert_eq!(reader.read_u16().unwrap(), 0x0302);
        assert_eq!(reader.read_u32().unwrap(), 0x0706_0504);
        assert!(reader.is_at_end());
    }

    #[test]
    fn reports_eof_at_each_width() {
        assert_eq!(
            ZipReader::new(&[]).read_u8(),
            Err(ZipReadError::UnexpectedEof {
                offset: 0,
                needed: 1,
                remaining: 0,
            })
        );
        assert_eq!(
            ZipReader::new(&[0x01]).read_u16(),
            Err(ZipReadError::UnexpectedEof {
                offset: 0,
                needed: 2,
                remaining: 1,
            })
        );
        assert_eq!(
            ZipReader::new(&[0x01, 0x02, 0x03]).read_u32(),
            Err(ZipReadError::UnexpectedEof {
                offset: 0,
                needed: 4,
                remaining: 3,
            })
        );
    }

    #[test]
    fn respects_bounded_ranges() {
        let bytes = [0x10, 0x20, 0x30];
        let mut reader = ZipReader::with_range(&bytes, 1, 3).unwrap();

        assert_eq!(reader.read_bytes(2).unwrap(), &[0x20, 0x30]);
        assert_eq!(reader.position(), 3);
    }

    #[test]
    fn rejects_invalid_ranges() {
        let bytes = [0x10, 0x20, 0x30];

        assert_eq!(
            ZipReader::with_range(&bytes, 3, 2),
            Err(ZipReadError::InvalidRange {
                start: 3,
                end: 2,
                len: 3,
            })
        );
        assert_eq!(
            ZipReader::with_range(&bytes, 0, 4),
            Err(ZipReadError::InvalidRange {
                start: 0,
                end: 4,
                len: 3,
            })
        );
    }
}
