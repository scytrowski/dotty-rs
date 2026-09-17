use crate::access_flags::ClassAccessFlags;
use crate::attribute::{
    Attribute, AttributeError, decode_attributes, read_index_list, read_optional_index,
};
use crate::constant_pool::{ConstantPool, ConstantPoolError, ConstantPoolIndex, read_index};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassFileError {
    Read(ReadError),
    Header(ClassFileHeaderError),
    ConstantPool(ConstantPoolError),
    Attribute(AttributeError),
}

impl fmt::Display for ClassFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::Header(error) => error.fmt(formatter),
            Self::ConstantPool(error) => error.fmt(formatter),
            Self::Attribute(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ClassFileError {}

impl From<ReadError> for ClassFileError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

impl From<ClassFileHeaderError> for ClassFileError {
    fn from(error: ClassFileHeaderError) -> Self {
        Self::Header(error)
    }
}

impl From<ConstantPoolError> for ClassFileError {
    fn from(error: ConstantPoolError) -> Self {
        Self::ConstantPool(error)
    }
}

impl From<AttributeError> for ClassFileError {
    fn from(error: AttributeError) -> Self {
        Self::Attribute(error)
    }
}

impl<'a> ClassFile<'a> {
    /// Decodes a complete `ClassFile` (JVMS §4.1): header, constant pool,
    /// access flags, `this_class`/`super_class`/interfaces, fields, methods,
    /// and top-level attributes.
    pub fn decode(reader: &mut Reader<'a>) -> Result<Self, ClassFileError> {
        let version = ClassFileVersion::decode(reader)?;
        let constant_pool = ConstantPool::decode(reader)?;

        let access_flags = ClassAccessFlags(reader.read_u16()?);
        let this_class = read_index(reader)?;
        let super_class = read_optional_index(reader)?;
        let interfaces = read_index_list(reader)?;

        let fields_count = reader.read_u16()?;
        let mut fields = Vec::with_capacity(usize::from(fields_count));
        for _ in 0..fields_count {
            fields.push(FieldInfo::decode(reader, &constant_pool)?);
        }

        let methods_count = reader.read_u16()?;
        let mut methods = Vec::with_capacity(usize::from(methods_count));
        for _ in 0..methods_count {
            methods.push(MethodInfo::decode(reader, &constant_pool)?);
        }

        let attributes = decode_attributes(reader, &constant_pool)?;

        Ok(Self {
            version,
            constant_pool,
            access_flags,
            this_class,
            super_class,
            interfaces,
            fields,
            methods,
            attributes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribute::AttributeError;
    use crate::constant_pool::ConstantPoolEntry;

    /// A minimal but complete class file: one class named via constant pool
    /// entries `#1` (`Utf8 "Foo"`) / `#2` (`Class -> #1`), `PUBLIC | SUPER`
    /// access flags, `super_class = 0` (`java.lang.Object`), and no
    /// interfaces/fields/methods/attributes.
    const MINIMAL_CLASS_FILE: &[u8] = &[
        0xCA, 0xFE, 0xBA, 0xBE, // magic
        0x00, 0x00, // minor
        0x00, 0x45, // major = 69
        0x00, 0x03, // constant_pool_count = 3
        0x01, 0x00, 0x03, b'F', b'o', b'o', // #1 Utf8 "Foo"
        0x07, 0x00, 0x01, // #2 Class -> #1
        0x00, 0x21, // access_flags = PUBLIC | SUPER
        0x00, 0x02, // this_class = #2
        0x00, 0x00, // super_class = 0 (Object)
        0x00, 0x00, // interfaces_count = 0
        0x00, 0x00, // fields_count = 0
        0x00, 0x00, // methods_count = 0
        0x00, 0x00, // attributes_count = 0
    ];

    #[test]
    fn decodes_a_minimal_complete_class_file() {
        let mut reader = Reader::new(MINIMAL_CLASS_FILE);

        let class_file = ClassFile::decode(&mut reader).unwrap();

        assert_eq!(
            class_file.version,
            ClassFileVersion {
                major: 69,
                minor: 0
            }
        );
        assert_eq!(class_file.access_flags, ClassAccessFlags(0x0021));
        assert_eq!(class_file.this_class, ConstantPoolIndex(2));
        assert_eq!(class_file.super_class, None);
        assert_eq!(class_file.interfaces, Vec::<ConstantPoolIndex>::new());
        assert_eq!(class_file.fields, Vec::new());
        assert_eq!(class_file.methods, Vec::new());
        assert_eq!(class_file.attributes, Vec::new());
        assert_eq!(
            class_file.constant_pool.get(ConstantPoolIndex(1)),
            Some(&ConstantPoolEntry::Utf8("Foo".to_owned()))
        );
        assert!(reader.is_at_end());
    }

    #[test]
    fn reports_an_error_from_a_class_file_truncated_before_attributes_count() {
        let mut reader = Reader::new(&MINIMAL_CLASS_FILE[..31]);

        assert_eq!(
            ClassFile::decode(&mut reader),
            Err(ClassFileError::Attribute(AttributeError::Read(
                ReadError::UnexpectedEof {
                    offset: 31,
                    needed: 2,
                    remaining: 0,
                }
            )))
        );
    }

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
