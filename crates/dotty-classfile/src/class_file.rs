use crate::access_flags::ClassAccessFlags;
use crate::attribute::{
    Attribute, AttributeError, decode_attributes, read_index_list, read_optional_index,
    validate_attributes,
};
use crate::constant_pool::{
    ConstantPool, ConstantPoolError, ConstantPoolIndex, PoolRefError, read_index,
};
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

/// The lowest `major_version` the JVMS has ever defined (JDK 1.0.2;
/// `docs/classfile-format-jdk25.md` §2.1).
pub const MIN_MAJOR_VERSION: u16 = 45;

/// This project's compatibility ceiling: JDK 25, the version
/// `docs/classfile-format-jdk25.md` is written against. A `major_version`
/// above this is a JDK newer than this decoder has been validated
/// against, not necessarily an invalid file.
pub const MAX_MAJOR_VERSION: u16 = 69;

/// The `minor_version` that marks a class as compiled against **preview
/// features** of its major version (JVMS §4.1, `major_version >= 56`).
/// Only the exact JDK that introduced those preview features can load
/// such a class; this decoder never can, since it has no notion of
/// "preview features enabled".
pub const PREVIEW_MINOR_VERSION: u16 = 0xFFFF;

/// The `minor_version`/`major_version` pair of a class file (JVMS §4.1).
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

    /// Whether this decoder can safely give this version's class file a
    /// semantic reading, per `docs/classfile-format-jdk25.md` §2.1's
    /// compatible-range rules:
    ///
    /// - `major_version` must fall within the historically valid,
    ///   JDK-25-or-below range (`MIN_MAJOR_VERSION..=MAX_MAJOR_VERSION`);
    /// - `minor_version` must not be [`PREVIEW_MINOR_VERSION`] (this
    ///   decoder never has preview features enabled for any major
    ///   version, so a preview class is always incompatible);
    /// - for `major_version >= 56` (JDK 12+), `minor_version` must be `0`
    ///   (the only other value the JVMS allows there is
    ///   `PREVIEW_MINOR_VERSION`, already rejected above).
    pub fn is_compatible(&self) -> bool {
        if !(MIN_MAJOR_VERSION..=MAX_MAJOR_VERSION).contains(&self.major) {
            return false;
        }
        if self.minor == PREVIEW_MINOR_VERSION {
            return false;
        }
        if self.major >= 56 && self.minor != 0 {
            return false;
        }
        true
    }
}

impl fmt::Display for ClassFileVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
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

    /// Validates that every constant pool reference reachable from this
    /// `ClassFile` — `this_class`, `super_class`, `interfaces`, and every
    /// field/method/attribute's indices, recursively — resolves to the
    /// expected entry kind (JVMS §4.1). Stops at the first bad reference.
    pub fn validate_references(&self) -> Result<(), PoolRefError> {
        let pool = &self.constant_pool;

        pool.class_name(self.this_class)?;
        if let Some(super_class) = self.super_class {
            pool.class_name(super_class)?;
        }
        for interface in &self.interfaces {
            pool.class_name(*interface)?;
        }
        for field in &self.fields {
            field.validate_references(pool)?;
        }
        for method in &self.methods {
            method.validate_references(pool)?;
        }
        validate_attributes(&self.attributes, pool)
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
    fn validate_references_accepts_the_minimal_class_file() {
        let mut reader = Reader::new(MINIMAL_CLASS_FILE);
        let class_file = ClassFile::decode(&mut reader).unwrap();

        assert_eq!(class_file.validate_references(), Ok(()));
    }

    #[test]
    fn validate_references_rejects_a_this_class_that_is_not_a_class_entry() {
        let class_file = ClassFile {
            version: ClassFileVersion {
                major: 69,
                minor: 0,
            },
            constant_pool: ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Utf8(
                "Foo".to_owned(),
            ))]),
            access_flags: ClassAccessFlags(0),
            this_class: ConstantPoolIndex(1),
            super_class: None,
            interfaces: vec![],
            fields: vec![],
            methods: vec![],
            attributes: vec![],
        };

        assert_eq!(
            class_file.validate_references(),
            Err(crate::constant_pool::PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: crate::constant_pool::EntryKind::Class,
            })
        );
    }

    #[test]
    fn validate_references_rejects_a_super_class_that_is_not_a_class_entry() {
        let class_file = ClassFile {
            version: ClassFileVersion {
                major: 69,
                minor: 0,
            },
            constant_pool: ConstantPool::from_entries(vec![
                Some(ConstantPoolEntry::Utf8("Foo".to_owned())),
                Some(ConstantPoolEntry::Class {
                    name_index: ConstantPoolIndex(1),
                }),
                Some(ConstantPoolEntry::Integer(1)),
            ]),
            access_flags: ClassAccessFlags(0),
            this_class: ConstantPoolIndex(2),
            super_class: Some(ConstantPoolIndex(3)),
            interfaces: vec![],
            fields: vec![],
            methods: vec![],
            attributes: vec![],
        };

        assert_eq!(
            class_file.validate_references(),
            Err(crate::constant_pool::PoolRefError::WrongKind {
                index: ConstantPoolIndex(3),
                expected: crate::constant_pool::EntryKind::Class,
            })
        );
    }

    #[test]
    fn validate_references_rejects_an_interface_that_is_not_a_class_entry() {
        let class_file = ClassFile {
            version: ClassFileVersion {
                major: 69,
                minor: 0,
            },
            constant_pool: ConstantPool::from_entries(vec![
                Some(ConstantPoolEntry::Utf8("Foo".to_owned())),
                Some(ConstantPoolEntry::Class {
                    name_index: ConstantPoolIndex(1),
                }),
                Some(ConstantPoolEntry::Integer(1)),
            ]),
            access_flags: ClassAccessFlags(0),
            this_class: ConstantPoolIndex(2),
            super_class: None,
            interfaces: vec![ConstantPoolIndex(3)],
            fields: vec![],
            methods: vec![],
            attributes: vec![],
        };

        assert_eq!(
            class_file.validate_references(),
            Err(crate::constant_pool::PoolRefError::WrongKind {
                index: ConstantPoolIndex(3),
                expected: crate::constant_pool::EntryKind::Class,
            })
        );
    }

    #[test]
    fn validate_references_propagates_a_bad_field_reference() {
        let class_file = ClassFile {
            version: ClassFileVersion {
                major: 69,
                minor: 0,
            },
            constant_pool: ConstantPool::from_entries(vec![
                Some(ConstantPoolEntry::Utf8("Foo".to_owned())),
                Some(ConstantPoolEntry::Class {
                    name_index: ConstantPoolIndex(1),
                }),
                Some(ConstantPoolEntry::Integer(1)),
            ]),
            access_flags: ClassAccessFlags(0),
            this_class: ConstantPoolIndex(2),
            super_class: None,
            interfaces: vec![],
            fields: vec![FieldInfo {
                access_flags: crate::access_flags::FieldAccessFlags(0),
                name_index: ConstantPoolIndex(3),
                descriptor_index: ConstantPoolIndex(1),
                attributes: vec![],
            }],
            methods: vec![],
            attributes: vec![],
        };

        assert_eq!(
            class_file.validate_references(),
            Err(crate::constant_pool::PoolRefError::WrongKind {
                index: ConstantPoolIndex(3),
                expected: crate::constant_pool::EntryKind::Utf8,
            })
        );
    }

    #[test]
    fn validate_references_propagates_a_bad_top_level_attribute_reference() {
        let class_file = ClassFile {
            version: ClassFileVersion {
                major: 69,
                minor: 0,
            },
            constant_pool: ConstantPool::from_entries(vec![
                Some(ConstantPoolEntry::Utf8("Foo".to_owned())),
                Some(ConstantPoolEntry::Class {
                    name_index: ConstantPoolIndex(1),
                }),
                Some(ConstantPoolEntry::Integer(1)),
            ]),
            access_flags: ClassAccessFlags(0),
            this_class: ConstantPoolIndex(2),
            super_class: None,
            interfaces: vec![],
            fields: vec![],
            methods: vec![],
            attributes: vec![Attribute::SourceFile(ConstantPoolIndex(3))],
        };

        assert_eq!(
            class_file.validate_references(),
            Err(crate::constant_pool::PoolRefError::WrongKind {
                index: ConstantPoolIndex(3),
                expected: crate::constant_pool::EntryKind::Utf8,
            })
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

    #[test]
    fn a_jdk_25_version_is_compatible() {
        assert!(
            ClassFileVersion {
                major: 69,
                minor: 0
            }
            .is_compatible()
        );
    }

    #[test]
    fn the_oldest_historical_version_is_compatible() {
        assert!(
            ClassFileVersion {
                major: 45,
                minor: 3
            }
            .is_compatible()
        );
    }

    #[test]
    fn a_major_version_below_the_historical_minimum_is_incompatible() {
        assert!(
            !ClassFileVersion {
                major: 44,
                minor: 0
            }
            .is_compatible()
        );
    }

    #[test]
    fn a_major_version_above_the_jdk_25_ceiling_is_incompatible() {
        assert!(
            !ClassFileVersion {
                major: 70,
                minor: 0
            }
            .is_compatible()
        );
    }

    #[test]
    fn a_preview_minor_version_is_incompatible() {
        assert!(
            !ClassFileVersion {
                major: 69,
                minor: PREVIEW_MINOR_VERSION,
            }
            .is_compatible()
        );
    }

    #[test]
    fn a_nonzero_non_preview_minor_version_is_incompatible_from_jdk_12_onward() {
        assert!(
            !ClassFileVersion {
                major: 56,
                minor: 1
            }
            .is_compatible()
        );
    }

    #[test]
    fn any_minor_version_is_compatible_before_jdk_12() {
        assert!(
            ClassFileVersion {
                major: 55,
                minor: 1234
            }
            .is_compatible()
        );
    }

    #[test]
    fn version_displays_as_major_dot_minor() {
        let version = ClassFileVersion {
            major: 69,
            minor: 0,
        };

        assert_eq!(version.to_string(), "69.0");
    }
}
