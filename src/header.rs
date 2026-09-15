use crate::reader::{ReadError, Reader};
use std::fmt;

pub const TASTY_MAGIC: [u8; 4] = [0x5c, 0xa1, 0xab, 0x1f];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub major_version: u32,
    pub minor_version: u32,
    pub experimental_version: u32,
    pub tooling_version: String,
    pub uuid: [u8; 16],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderError {
    Read(ReadError),
    InvalidMagic { actual: [u8; 4] },
}

impl fmt::Display for HeaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::InvalidMagic { actual } => write!(
                formatter,
                "invalid TASTy magic header: {:02x} {:02x} {:02x} {:02x}",
                actual[0], actual[1], actual[2], actual[3]
            ),
        }
    }
}

impl std::error::Error for HeaderError {}

impl From<ReadError> for HeaderError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

impl Header {
    pub fn parse(bytes: &[u8]) -> Result<Self, HeaderError> {
        let mut reader = Reader::new(bytes);
        Self::decode(&mut reader)
    }

    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, HeaderError> {
        let magic_bytes = reader.read_bytes(TASTY_MAGIC.len())?;
        let mut actual = [0u8; TASTY_MAGIC.len()];
        actual.copy_from_slice(magic_bytes);

        if actual != TASTY_MAGIC {
            return Err(HeaderError::InvalidMagic { actual });
        }

        let major_version = reader.read_nat()?;
        let minor_version = reader.read_nat()?;
        let experimental_version = reader.read_nat()?;
        let tooling_version = reader.read_utf8()?;
        let uuid_bytes = reader.read_bytes(16)?;
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(uuid_bytes);

        Ok(Self {
            major_version,
            minor_version,
            experimental_version,
            tooling_version,
            uuid,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Header, HeaderError, TASTY_MAGIC};
    use crate::reader::{ReadError, Reader};

    #[test]
    fn decodes_a_scala_3_9_header() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let header = Header::parse(bytes).unwrap();

        assert_eq!(header.major_version, 28);
        assert_eq!(header.minor_version, 9);
        assert_eq!(header.experimental_version, 0);
        assert_eq!(header.tooling_version, "Scala 3.9.0");
        assert_eq!(header.uuid.len(), 16);
    }

    #[test]
    fn rejects_an_invalid_magic_header() {
        let bytes = [0, 0, 0, 0];

        assert_eq!(
            Header::parse(&bytes),
            Err(HeaderError::InvalidMagic { actual: bytes })
        );
    }

    #[test]
    fn reports_a_truncated_header() {
        let mut bytes = TASTY_MAGIC.to_vec();
        bytes.push(0x9c);

        assert_eq!(
            Header::parse(&bytes),
            Err(HeaderError::Read(ReadError::UnexpectedEof {
                offset: 5,
                needed: 1,
                remaining: 0,
            }))
        );
    }

    #[test]
    fn decodes_from_a_bounded_reader() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let mut reader = Reader::with_range(bytes, 0, 35).unwrap();

        let header = Header::decode(&mut reader).unwrap();

        assert_eq!(header.tooling_version, "Scala 3.9.0");
        assert_eq!(reader.position(), 35);
    }
}
