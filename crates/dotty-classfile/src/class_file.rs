use crate::access_flags::ClassAccessFlags;
use crate::attribute::Attribute;
use crate::constant_pool::{ConstantPool, ConstantPoolIndex};
use crate::field::FieldInfo;
use crate::method::MethodInfo;
use crate::reader::{ReadError, Reader};
use std::fmt;

/// The `ClassFile` magic number (JVMS §4.1).
pub const CLASS_MAGIC: [u8; 4] = [0xCA, 0xFE, 0xBA, 0xBE];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassFileHeaderError {
    Read(ReadError),
    InvalidMagic { actual: [u8; 4] },
}

impl fmt::Display for ClassFileHeaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::InvalidMagic { actual } => write!(
                formatter,
                "invalid class file magic: {:02x} {:02x} {:02x} {:02x}",
                actual[0], actual[1], actual[2], actual[3]
            ),
        }
    }
}

impl std::error::Error for ClassFileHeaderError {}

impl From<ReadError> for ClassFileHeaderError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

/// The `minor_version`/`major_version` pair of a class file (JVMS §4.1).
/// Compatibility-range checks land alongside the header decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassFileVersion {
    pub major: u16,
    pub minor: u16,
}

impl ClassFileVersion {
    /// Decodes the class file's magic number and version fields (the first
    /// eight bytes of a `ClassFile`, JVMS §4.1).
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, ClassFileHeaderError> {
        let magic_bytes = reader.read_bytes(CLASS_MAGIC.len())?;
        let mut actual = [0u8; CLASS_MAGIC.len()];
        actual.copy_from_slice(magic_bytes);

        if actual != CLASS_MAGIC {
            return Err(ClassFileHeaderError::InvalidMagic { actual });
        }

        let minor = reader.read_u16()?;
        let major = reader.read_u16()?;

        Ok(Self { major, minor })
    }
}

/// The full `ClassFile` structure (JVMS §4.1).
#[derive(Debug, Clone, PartialEq)]
pub struct ClassFile<'a> {
    pub version: ClassFileVersion,
    pub constant_pool: ConstantPool,
    pub access_flags: ClassAccessFlags,
    pub this_class: ConstantPoolIndex,
    /// `None` only for `java.lang.Object`.
    pub super_class: Option<ConstantPoolIndex>,
    pub interfaces: Vec<ConstantPoolIndex>,
    pub fields: Vec<FieldInfo<'a>>,
    pub methods: Vec<MethodInfo<'a>>,
    pub attributes: Vec<Attribute<'a>>,
}

#[cfg(test)]
mod tests {
    use super::{CLASS_MAGIC, ClassFileHeaderError, ClassFileVersion};
    use crate::reader::{ReadError, Reader};

    #[test]
    fn decodes_a_valid_magic_and_version() {
        let bytes = [0xCA, 0xFE, 0xBA, 0xBE, 0x00, 0x00, 0x00, 0x45];
        let mut reader = Reader::new(&bytes);

        let version = ClassFileVersion::decode(&mut reader).unwrap();

        assert_eq!(
            version,
            ClassFileVersion {
                major: 69,
                minor: 0,
            }
        );
        assert_eq!(reader.position(), 8);
    }

    #[test]
    fn rejects_an_invalid_magic() {
        let bytes = [0x00, 0x00, 0x00, 0x00];
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            ClassFileVersion::decode(&mut reader),
            Err(ClassFileHeaderError::InvalidMagic { actual: bytes })
        );
    }

    #[test]
    fn reports_a_magic_truncated_mid_sequence() {
        let bytes = [0xCA, 0xFE];
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            ClassFileVersion::decode(&mut reader),
            Err(ClassFileHeaderError::Read(ReadError::UnexpectedEof {
                offset: 0,
                needed: CLASS_MAGIC.len(),
                remaining: 2,
            }))
        );
    }

    #[test]
    fn reports_a_version_truncated_after_a_valid_magic() {
        let bytes = [0xCA, 0xFE, 0xBA, 0xBE, 0x00];
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            ClassFileVersion::decode(&mut reader),
            Err(ClassFileHeaderError::Read(ReadError::UnexpectedEof {
                offset: 4,
                needed: 2,
                remaining: 1,
            }))
        );
    }
}
