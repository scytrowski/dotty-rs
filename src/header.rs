use crate::reader::{ReadError, Reader};
use crate::writer::{WriteError, Writer};
use std::fmt;

pub const TASTY_MAGIC: [u8; 4] = [0x5c, 0xa1, 0xab, 0x1f];
pub const SCALA_3_9_MAJOR_VERSION: u32 = 28;
pub const SCALA_3_9_MINOR_VERSION: u32 = 9;
pub const SCALA_3_9_EXPERIMENTAL_VERSION: u32 = 0;

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
    InvalidMagic {
        actual: [u8; 4],
    },
    UnsupportedVersion {
        major: u32,
        minor: u32,
        experimental: u32,
    },
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
            Self::UnsupportedVersion {
                major,
                minor,
                experimental,
            } => write!(
                formatter,
                "unsupported TASTy version {major}.{minor}.{experimental}; expected Scala 3.9.0 format version 28.9.0"
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

    pub fn encode(&self, writer: &mut Writer) -> Result<(), WriteError> {
        writer.write_bytes(&TASTY_MAGIC);
        writer.write_nat(self.major_version);
        writer.write_nat(self.minor_version);
        writer.write_nat(self.experimental_version);
        writer.write_utf8(&self.tooling_version)?;
        writer.write_bytes(&self.uuid);
        Ok(())
    }

    pub fn is_scala_3_9(&self) -> bool {
        self.major_version == SCALA_3_9_MAJOR_VERSION
            && self.minor_version == SCALA_3_9_MINOR_VERSION
            && self.experimental_version == SCALA_3_9_EXPERIMENTAL_VERSION
    }

    /// Returns whether this file version is compatible with a compiler
    /// version according to the TASTy compatibility relation.
    ///
    /// A file is compatible when all version components match exactly, or
    /// when it has the same major version, an older minor version, and no
    /// experimental component.
    pub fn is_compatible_with(
        &self,
        compiler_major: u32,
        compiler_minor: u32,
        compiler_experimental: u32,
    ) -> bool {
        (self.major_version == compiler_major
            && self.minor_version == compiler_minor
            && self.experimental_version == compiler_experimental)
            || (self.major_version == compiler_major
                && self.minor_version < compiler_minor
                && self.experimental_version == 0)
    }

    pub fn validate_scala_3_9(&self) -> Result<(), HeaderError> {
        if self.is_scala_3_9() {
            Ok(())
        } else {
            Err(HeaderError::UnsupportedVersion {
                major: self.major_version,
                minor: self.minor_version,
                experimental: self.experimental_version,
            })
        }
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

    #[test]
    fn recognizes_and_validates_the_scala_3_9_format_version() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let header = Header::parse(bytes).unwrap();

        assert!(header.is_scala_3_9());
        assert_eq!(header.validate_scala_3_9(), Ok(()));
    }

    #[test]
    fn accepts_an_exactly_matching_compiler_version() {
        let header = Header {
            major_version: 28,
            minor_version: 9,
            experimental_version: 1,
            tooling_version: String::new(),
            uuid: [0; 16],
        };

        assert!(header.is_compatible_with(28, 9, 1));
    }

    #[test]
    fn accepts_an_older_stable_minor_version() {
        let header = Header {
            major_version: 28,
            minor_version: 8,
            experimental_version: 0,
            tooling_version: String::new(),
            uuid: [0; 16],
        };

        assert!(header.is_compatible_with(28, 9, 0));
    }

    #[test]
    fn rejects_a_different_experimental_version_for_the_same_minor() {
        let header = Header {
            major_version: 28,
            minor_version: 9,
            experimental_version: 1,
            tooling_version: String::new(),
            uuid: [0; 16],
        };

        assert!(!header.is_compatible_with(28, 9, 0));
    }

    #[test]
    fn rejects_a_different_major_version() {
        let header = Header {
            major_version: 27,
            minor_version: 9,
            experimental_version: 0,
            tooling_version: String::new(),
            uuid: [0; 16],
        };

        assert!(!header.is_compatible_with(28, 9, 0));
    }

    #[test]
    fn rejects_a_different_format_version_only_when_validation_is_requested() {
        let header = Header {
            major_version: 29,
            minor_version: 0,
            experimental_version: 0,
            tooling_version: "future compiler".to_owned(),
            uuid: [0; 16],
        };

        assert!(!header.is_scala_3_9());
        assert_eq!(
            header.validate_scala_3_9(),
            Err(HeaderError::UnsupportedVersion {
                major: 29,
                minor: 0,
                experimental: 0,
            })
        );
    }
}
