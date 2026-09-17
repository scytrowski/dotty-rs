use crate::reader::{ReadError, Reader};
use std::fmt;

pub const CONSTANT_UTF8_TAG: u8 = 1;
pub const CONSTANT_INTEGER_TAG: u8 = 3;
pub const CONSTANT_FLOAT_TAG: u8 = 4;
pub const CONSTANT_LONG_TAG: u8 = 5;
pub const CONSTANT_DOUBLE_TAG: u8 = 6;
pub const CONSTANT_CLASS_TAG: u8 = 7;
pub const CONSTANT_STRING_TAG: u8 = 8;
pub const CONSTANT_FIELDREF_TAG: u8 = 9;
pub const CONSTANT_METHODREF_TAG: u8 = 10;
pub const CONSTANT_INTERFACE_METHODREF_TAG: u8 = 11;
pub const CONSTANT_NAME_AND_TYPE_TAG: u8 = 12;
pub const CONSTANT_METHOD_HANDLE_TAG: u8 = 15;
pub const CONSTANT_METHOD_TYPE_TAG: u8 = 16;
pub const CONSTANT_DYNAMIC_TAG: u8 = 17;
pub const CONSTANT_INVOKE_DYNAMIC_TAG: u8 = 18;
pub const CONSTANT_MODULE_TAG: u8 = 19;
pub const CONSTANT_PACKAGE_TAG: u8 = 20;

/// A 1-based index into a [`ConstantPool`]. Index `0` is always invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConstantPoolIndex(pub u16);

/// An index into a `BootstrapMethods` attribute's `bootstrap_methods` array
/// (JVMS §4.7.23) — a distinct index space from [`ConstantPoolIndex`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BootstrapMethodIndex(pub u16);

/// The kind of a `CONSTANT_MethodHandle_info` reference (JVMS §4.4.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodHandleKind {
    GetField,
    GetStatic,
    PutField,
    PutStatic,
    InvokeVirtual,
    InvokeStatic,
    InvokeSpecial,
    NewInvokeSpecial,
    InvokeInterface,
}

impl MethodHandleKind {
    /// Maps a raw `reference_kind` byte (JVMS §4.4.8, values `1`-`9`) to the
    /// corresponding variant.
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::GetField),
            2 => Some(Self::GetStatic),
            3 => Some(Self::PutField),
            4 => Some(Self::PutStatic),
            5 => Some(Self::InvokeVirtual),
            6 => Some(Self::InvokeStatic),
            7 => Some(Self::InvokeSpecial),
            8 => Some(Self::NewInvokeSpecial),
            9 => Some(Self::InvokeInterface),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConstantPoolEntry {
    Utf8(String),
    Integer(i32),
    Float(f32),
    Long(i64),
    Double(f64),
    Class {
        name_index: ConstantPoolIndex,
    },
    String {
        string_index: ConstantPoolIndex,
    },
    Fieldref {
        class_index: ConstantPoolIndex,
        name_and_type_index: ConstantPoolIndex,
    },
    Methodref {
        class_index: ConstantPoolIndex,
        name_and_type_index: ConstantPoolIndex,
    },
    InterfaceMethodref {
        class_index: ConstantPoolIndex,
        name_and_type_index: ConstantPoolIndex,
    },
    NameAndType {
        name_index: ConstantPoolIndex,
        descriptor_index: ConstantPoolIndex,
    },
    MethodHandle {
        reference_kind: MethodHandleKind,
        reference_index: ConstantPoolIndex,
    },
    MethodType {
        descriptor_index: ConstantPoolIndex,
    },
    Dynamic {
        bootstrap_method_attr_index: BootstrapMethodIndex,
        name_and_type_index: ConstantPoolIndex,
    },
    InvokeDynamic {
        bootstrap_method_attr_index: BootstrapMethodIndex,
        name_and_type_index: ConstantPoolIndex,
    },
    Module {
        name_index: ConstantPoolIndex,
    },
    Package {
        name_index: ConstantPoolIndex,
    },
}

/// A class file's constant pool.
///
/// `Long`/`Double` entries occupy two consecutive slots (JVMS §4.4.5); the
/// second slot is represented as `None` rather than a repeated entry.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConstantPool {
    entries: Vec<Option<ConstantPoolEntry>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstantPoolError {
    Read(ReadError),
    UnknownTag { index: u16, tag: u8 },
    InvalidMethodHandleReferenceKind { index: u16, kind: u8 },
}

impl fmt::Display for ConstantPoolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::UnknownTag { index, tag } => write!(
                formatter,
                "unknown constant pool tag {tag} at index {index}"
            ),
            Self::InvalidMethodHandleReferenceKind { index, kind } => write!(
                formatter,
                "invalid method handle reference_kind {kind} at index {index}"
            ),
        }
    }
}

impl std::error::Error for ConstantPoolError {}

impl From<ReadError> for ConstantPoolError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

/// The expected shape of a constant pool entry, used by [`PoolRefError`] to
/// report what a reference *should* have resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Utf8,
    Class,
    NameAndType,
    MethodHandle,
    Integer,
    Long,
    Float,
    Double,
    /// One of the "loadable constant" tags (JVMS §4.4): `Integer`, `Float`,
    /// `Long`, `Double`, `String`, `Class`, `MethodHandle`, `MethodType`, or
    /// `Dynamic`.
    Loadable,
    /// One of the tags allowed for a `ConstantValue` attribute (JVMS
    /// §4.7.2): `Integer`, `Float`, `Long`, `Double`, or `String`.
    ConstantValue,
}

impl fmt::Display for EntryKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let description = match self {
            Self::Utf8 => "a Utf8 entry",
            Self::Class => "a Class entry",
            Self::NameAndType => "a NameAndType entry",
            Self::MethodHandle => "a MethodHandle entry",
            Self::Integer => "an Integer entry",
            Self::Long => "a Long entry",
            Self::Float => "a Float entry",
            Self::Double => "a Double entry",
            Self::Loadable => "a loadable constant",
            Self::ConstantValue => "a ConstantValue-compatible constant",
        };
        formatter.write_str(description)
    }
}

/// A failure to resolve a [`ConstantPoolIndex`] to the expected kind of
/// entry — either nothing is there, or something is, but not the right
/// shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolRefError {
    InvalidIndex {
        index: ConstantPoolIndex,
    },
    WrongKind {
        index: ConstantPoolIndex,
        expected: EntryKind,
    },
}

impl fmt::Display for PoolRefError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIndex { index } => write!(
                formatter,
                "constant pool index {} does not resolve to any entry",
                index.0
            ),
            Self::WrongKind { index, expected } => write!(
                formatter,
                "constant pool index {} does not resolve to {expected}",
                index.0
            ),
        }
    }
}

impl std::error::Error for PoolRefError {}

impl ConstantPool {
    pub fn from_entries(entries: Vec<Option<ConstantPoolEntry>>) -> Self {
        Self { entries }
    }

    pub fn get(&self, index: ConstantPoolIndex) -> Option<&ConstantPoolEntry> {
        let position = usize::from(index.0).checked_sub(1)?;
        self.entries.get(position)?.as_ref()
    }

    /// Resolves `index` to a `Utf8` entry's string (JVMS §4.4.7).
    pub fn utf8(&self, index: ConstantPoolIndex) -> Result<&str, PoolRefError> {
        match self.get(index) {
            Some(ConstantPoolEntry::Utf8(value)) => Ok(value.as_str()),
            Some(_) => Err(PoolRefError::WrongKind {
                index,
                expected: EntryKind::Utf8,
            }),
            None => Err(PoolRefError::InvalidIndex { index }),
        }
    }

    /// Resolves `index` to a `Class` entry, then that entry's `name_index`
    /// to a `Utf8` string (the class's internal binary name, JVMS §4.4.1).
    pub fn class_name(&self, index: ConstantPoolIndex) -> Result<&str, PoolRefError> {
        match self.get(index) {
            Some(ConstantPoolEntry::Class { name_index }) => self.utf8(*name_index),
            Some(_) => Err(PoolRefError::WrongKind {
                index,
                expected: EntryKind::Class,
            }),
            None => Err(PoolRefError::InvalidIndex { index }),
        }
    }

    /// Resolves `index` to a `NameAndType` entry, then both of its indices
    /// to `Utf8` strings (name, descriptor) (JVMS §4.4.6).
    pub fn name_and_type(&self, index: ConstantPoolIndex) -> Result<(&str, &str), PoolRefError> {
        match self.get(index) {
            Some(ConstantPoolEntry::NameAndType {
                name_index,
                descriptor_index,
            }) => Ok((self.utf8(*name_index)?, self.utf8(*descriptor_index)?)),
            Some(_) => Err(PoolRefError::WrongKind {
                index,
                expected: EntryKind::NameAndType,
            }),
            None => Err(PoolRefError::InvalidIndex { index }),
        }
    }

    /// Resolves `index` to a `MethodHandle` entry (JVMS §4.4.8).
    pub fn method_handle(
        &self,
        index: ConstantPoolIndex,
    ) -> Result<&ConstantPoolEntry, PoolRefError> {
        match self.get(index) {
            Some(entry @ ConstantPoolEntry::MethodHandle { .. }) => Ok(entry),
            Some(_) => Err(PoolRefError::WrongKind {
                index,
                expected: EntryKind::MethodHandle,
            }),
            None => Err(PoolRefError::InvalidIndex { index }),
        }
    }

    /// Checks that `index` resolves to a "loadable constant" (JVMS §4.4),
    /// as required for e.g. `BootstrapMethods` arguments.
    pub fn check_loadable(&self, index: ConstantPoolIndex) -> Result<(), PoolRefError> {
        match self.get(index) {
            Some(
                ConstantPoolEntry::Integer(_)
                | ConstantPoolEntry::Float(_)
                | ConstantPoolEntry::Long(_)
                | ConstantPoolEntry::Double(_)
                | ConstantPoolEntry::String { .. }
                | ConstantPoolEntry::Class { .. }
                | ConstantPoolEntry::MethodHandle { .. }
                | ConstantPoolEntry::MethodType { .. }
                | ConstantPoolEntry::Dynamic { .. },
            ) => Ok(()),
            Some(_) => Err(PoolRefError::WrongKind {
                index,
                expected: EntryKind::Loadable,
            }),
            None => Err(PoolRefError::InvalidIndex { index }),
        }
    }

    /// Checks that `index` resolves to an `Integer` entry (JVMS §4.4.4) —
    /// also the required kind for annotation `element_value`s tagged `B`,
    /// `C`, `I`, `S`, or `Z` (JVMS §4.7.16.1).
    pub fn check_integer(&self, index: ConstantPoolIndex) -> Result<(), PoolRefError> {
        match self.get(index) {
            Some(ConstantPoolEntry::Integer(_)) => Ok(()),
            Some(_) => Err(PoolRefError::WrongKind {
                index,
                expected: EntryKind::Integer,
            }),
            None => Err(PoolRefError::InvalidIndex { index }),
        }
    }

    /// Checks that `index` resolves to a `Long` entry (JVMS §4.4.5).
    pub fn check_long(&self, index: ConstantPoolIndex) -> Result<(), PoolRefError> {
        match self.get(index) {
            Some(ConstantPoolEntry::Long(_)) => Ok(()),
            Some(_) => Err(PoolRefError::WrongKind {
                index,
                expected: EntryKind::Long,
            }),
            None => Err(PoolRefError::InvalidIndex { index }),
        }
    }

    /// Checks that `index` resolves to a `Float` entry (JVMS §4.4.4).
    pub fn check_float(&self, index: ConstantPoolIndex) -> Result<(), PoolRefError> {
        match self.get(index) {
            Some(ConstantPoolEntry::Float(_)) => Ok(()),
            Some(_) => Err(PoolRefError::WrongKind {
                index,
                expected: EntryKind::Float,
            }),
            None => Err(PoolRefError::InvalidIndex { index }),
        }
    }

    /// Checks that `index` resolves to a `Double` entry (JVMS §4.4.5).
    pub fn check_double(&self, index: ConstantPoolIndex) -> Result<(), PoolRefError> {
        match self.get(index) {
            Some(ConstantPoolEntry::Double(_)) => Ok(()),
            Some(_) => Err(PoolRefError::WrongKind {
                index,
                expected: EntryKind::Double,
            }),
            None => Err(PoolRefError::InvalidIndex { index }),
        }
    }

    /// Checks that `index` resolves to a tag allowed for a `ConstantValue`
    /// attribute (JVMS §4.7.2).
    pub fn check_constant_value(&self, index: ConstantPoolIndex) -> Result<(), PoolRefError> {
        match self.get(index) {
            Some(
                ConstantPoolEntry::Integer(_)
                | ConstantPoolEntry::Float(_)
                | ConstantPoolEntry::Long(_)
                | ConstantPoolEntry::Double(_)
                | ConstantPoolEntry::String { .. },
            ) => Ok(()),
            Some(_) => Err(PoolRefError::WrongKind {
                index,
                expected: EntryKind::ConstantValue,
            }),
            None => Err(PoolRefError::InvalidIndex { index }),
        }
    }

    /// Decodes `constant_pool_count` followed by that many constant pool
    /// entries (JVMS §4.4). Purely structural: indices are stored as-is,
    /// without checking that they are in range or point at the expected
    /// entry kind.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, ConstantPoolError> {
        let count = usize::from(reader.read_u16()?);
        let mut entries: Vec<Option<ConstantPoolEntry>> = Vec::new();
        let mut index = 1usize;

        while index < count {
            let tag = reader.read_u8()?;
            let entry = decode_entry(reader, tag, index as u16)?;
            let occupies_two_slots = matches!(
                entry,
                ConstantPoolEntry::Long(_) | ConstantPoolEntry::Double(_)
            );

            entries.push(Some(entry));
            index += 1;

            if occupies_two_slots {
                entries.push(None);
                index += 1;
            }
        }

        Ok(Self { entries })
    }
}

pub(crate) fn read_index(reader: &mut Reader<'_>) -> Result<ConstantPoolIndex, ReadError> {
    Ok(ConstantPoolIndex(reader.read_u16()?))
}

fn decode_entry(
    reader: &mut Reader<'_>,
    tag: u8,
    index: u16,
) -> Result<ConstantPoolEntry, ConstantPoolError> {
    match tag {
        CONSTANT_UTF8_TAG => {
            let length = usize::from(reader.read_u16()?);
            Ok(ConstantPoolEntry::Utf8(reader.read_modified_utf8(length)?))
        }
        CONSTANT_INTEGER_TAG => Ok(ConstantPoolEntry::Integer(reader.read_u32()? as i32)),
        CONSTANT_FLOAT_TAG => Ok(ConstantPoolEntry::Float(f32::from_bits(reader.read_u32()?))),
        CONSTANT_LONG_TAG => {
            let high = reader.read_u32()?;
            let low = reader.read_u32()?;
            Ok(ConstantPoolEntry::Long(
                ((u64::from(high) << 32) | u64::from(low)) as i64,
            ))
        }
        CONSTANT_DOUBLE_TAG => {
            let high = reader.read_u32()?;
            let low = reader.read_u32()?;
            Ok(ConstantPoolEntry::Double(f64::from_bits(
                (u64::from(high) << 32) | u64::from(low),
            )))
        }
        CONSTANT_CLASS_TAG => Ok(ConstantPoolEntry::Class {
            name_index: read_index(reader)?,
        }),
        CONSTANT_STRING_TAG => Ok(ConstantPoolEntry::String {
            string_index: read_index(reader)?,
        }),
        CONSTANT_FIELDREF_TAG => Ok(ConstantPoolEntry::Fieldref {
            class_index: read_index(reader)?,
            name_and_type_index: read_index(reader)?,
        }),
        CONSTANT_METHODREF_TAG => Ok(ConstantPoolEntry::Methodref {
            class_index: read_index(reader)?,
            name_and_type_index: read_index(reader)?,
        }),
        CONSTANT_INTERFACE_METHODREF_TAG => Ok(ConstantPoolEntry::InterfaceMethodref {
            class_index: read_index(reader)?,
            name_and_type_index: read_index(reader)?,
        }),
        CONSTANT_NAME_AND_TYPE_TAG => Ok(ConstantPoolEntry::NameAndType {
            name_index: read_index(reader)?,
            descriptor_index: read_index(reader)?,
        }),
        CONSTANT_METHOD_HANDLE_TAG => {
            let kind_tag = reader.read_u8()?;
            let reference_kind = MethodHandleKind::from_tag(kind_tag).ok_or(
                ConstantPoolError::InvalidMethodHandleReferenceKind {
                    index,
                    kind: kind_tag,
                },
            )?;
            Ok(ConstantPoolEntry::MethodHandle {
                reference_kind,
                reference_index: read_index(reader)?,
            })
        }
        CONSTANT_METHOD_TYPE_TAG => Ok(ConstantPoolEntry::MethodType {
            descriptor_index: read_index(reader)?,
        }),
        CONSTANT_DYNAMIC_TAG => Ok(ConstantPoolEntry::Dynamic {
            bootstrap_method_attr_index: BootstrapMethodIndex(reader.read_u16()?),
            name_and_type_index: read_index(reader)?,
        }),
        CONSTANT_INVOKE_DYNAMIC_TAG => Ok(ConstantPoolEntry::InvokeDynamic {
            bootstrap_method_attr_index: BootstrapMethodIndex(reader.read_u16()?),
            name_and_type_index: read_index(reader)?,
        }),
        CONSTANT_MODULE_TAG => Ok(ConstantPoolEntry::Module {
            name_index: read_index(reader)?,
        }),
        CONSTANT_PACKAGE_TAG => Ok(ConstantPoolEntry::Package {
            name_index: read_index(reader)?,
        }),
        _ => Err(ConstantPoolError::UnknownTag { index, tag }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::{ReadError, Reader};

    #[test]
    fn constant_pool_get_returns_none_for_index_zero() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(1))]);

        assert_eq!(pool.get(ConstantPoolIndex(0)), None);
    }

    #[test]
    fn constant_pool_get_returns_the_entry_at_a_valid_index() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(42))]);

        assert_eq!(
            pool.get(ConstantPoolIndex(1)),
            Some(&ConstantPoolEntry::Integer(42))
        );
    }

    #[test]
    fn constant_pool_get_returns_none_for_an_out_of_range_index() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(42))]);

        assert_eq!(pool.get(ConstantPoolIndex(2)), None);
    }

    #[test]
    fn constant_pool_get_returns_none_for_the_slot_after_a_long_or_double() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Long(1)), None]);

        assert_eq!(pool.get(ConstantPoolIndex(2)), None);
    }

    /// Wraps a single tag + payload as a one-entry constant pool
    /// (`constant_pool_count = 2`) and decodes the entry at index 1.
    fn decode_single_entry(
        tag: u8,
        payload: &[u8],
    ) -> Result<ConstantPoolEntry, ConstantPoolError> {
        let mut bytes = vec![0x00, 0x02, tag];
        bytes.extend_from_slice(payload);
        let mut reader = Reader::new(&bytes);
        let pool = ConstantPool::decode(&mut reader)?;
        Ok(pool.get(ConstantPoolIndex(1)).unwrap().clone())
    }

    #[test]
    fn decodes_a_utf8_entry() {
        let payload = [0x00, 0x05, b'h', b'e', b'l', b'l', b'o'];

        assert_eq!(
            decode_single_entry(CONSTANT_UTF8_TAG, &payload),
            Ok(ConstantPoolEntry::Utf8("hello".to_owned()))
        );
    }

    #[test]
    fn decodes_signed_integer_entries() {
        assert_eq!(
            decode_single_entry(CONSTANT_INTEGER_TAG, &[0x00, 0x00, 0x00, 0x2A]),
            Ok(ConstantPoolEntry::Integer(42))
        );
        assert_eq!(
            decode_single_entry(CONSTANT_INTEGER_TAG, &[0xFF, 0xFF, 0xFF, 0xFF]),
            Ok(ConstantPoolEntry::Integer(-1))
        );
    }

    #[test]
    fn decodes_a_float_entry() {
        assert_eq!(
            decode_single_entry(CONSTANT_FLOAT_TAG, &1.5f32.to_bits().to_be_bytes()),
            Ok(ConstantPoolEntry::Float(1.5))
        );
    }

    #[test]
    fn decodes_a_long_entry_and_reserves_the_next_slot() {
        let value: i64 = 42_000_000_000;
        let mut bytes = vec![0x00, 0x02, CONSTANT_LONG_TAG];
        bytes.extend_from_slice(&(value as u64).to_be_bytes());
        let mut reader = Reader::new(&bytes);

        let pool = ConstantPool::decode(&mut reader).unwrap();

        assert_eq!(
            pool.get(ConstantPoolIndex(1)),
            Some(&ConstantPoolEntry::Long(value))
        );
        assert_eq!(pool.get(ConstantPoolIndex(2)), None);
    }

    #[test]
    fn decodes_a_double_entry_and_reserves_the_next_slot() {
        let value: f64 = 12345.6789;
        let mut bytes = vec![0x00, 0x02, CONSTANT_DOUBLE_TAG];
        bytes.extend_from_slice(&value.to_bits().to_be_bytes());
        let mut reader = Reader::new(&bytes);

        let pool = ConstantPool::decode(&mut reader).unwrap();

        assert_eq!(
            pool.get(ConstantPoolIndex(1)),
            Some(&ConstantPoolEntry::Double(value))
        );
        assert_eq!(pool.get(ConstantPoolIndex(2)), None);
    }

    #[test]
    fn decodes_a_class_entry() {
        assert_eq!(
            decode_single_entry(CONSTANT_CLASS_TAG, &[0x00, 0x01]),
            Ok(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(1),
            })
        );
    }

    #[test]
    fn decodes_a_string_entry() {
        assert_eq!(
            decode_single_entry(CONSTANT_STRING_TAG, &[0x00, 0x01]),
            Ok(ConstantPoolEntry::String {
                string_index: ConstantPoolIndex(1),
            })
        );
    }

    #[test]
    fn decodes_a_fieldref_entry() {
        assert_eq!(
            decode_single_entry(CONSTANT_FIELDREF_TAG, &[0x00, 0x01, 0x00, 0x02]),
            Ok(ConstantPoolEntry::Fieldref {
                class_index: ConstantPoolIndex(1),
                name_and_type_index: ConstantPoolIndex(2),
            })
        );
    }

    #[test]
    fn decodes_a_methodref_entry() {
        assert_eq!(
            decode_single_entry(CONSTANT_METHODREF_TAG, &[0x00, 0x01, 0x00, 0x02]),
            Ok(ConstantPoolEntry::Methodref {
                class_index: ConstantPoolIndex(1),
                name_and_type_index: ConstantPoolIndex(2),
            })
        );
    }

    #[test]
    fn decodes_an_interface_methodref_entry() {
        assert_eq!(
            decode_single_entry(CONSTANT_INTERFACE_METHODREF_TAG, &[0x00, 0x01, 0x00, 0x02]),
            Ok(ConstantPoolEntry::InterfaceMethodref {
                class_index: ConstantPoolIndex(1),
                name_and_type_index: ConstantPoolIndex(2),
            })
        );
    }

    #[test]
    fn decodes_a_name_and_type_entry() {
        assert_eq!(
            decode_single_entry(CONSTANT_NAME_AND_TYPE_TAG, &[0x00, 0x01, 0x00, 0x02]),
            Ok(ConstantPoolEntry::NameAndType {
                name_index: ConstantPoolIndex(1),
                descriptor_index: ConstantPoolIndex(2),
            })
        );
    }

    #[test]
    fn decodes_every_method_handle_reference_kind() {
        let cases = [
            (1u8, MethodHandleKind::GetField),
            (2, MethodHandleKind::GetStatic),
            (3, MethodHandleKind::PutField),
            (4, MethodHandleKind::PutStatic),
            (5, MethodHandleKind::InvokeVirtual),
            (6, MethodHandleKind::InvokeStatic),
            (7, MethodHandleKind::InvokeSpecial),
            (8, MethodHandleKind::NewInvokeSpecial),
            (9, MethodHandleKind::InvokeInterface),
        ];

        for (kind_tag, expected_kind) in cases {
            assert_eq!(
                decode_single_entry(CONSTANT_METHOD_HANDLE_TAG, &[kind_tag, 0x00, 0x01]),
                Ok(ConstantPoolEntry::MethodHandle {
                    reference_kind: expected_kind,
                    reference_index: ConstantPoolIndex(1),
                }),
                "reference_kind {kind_tag} should decode to {expected_kind:?}"
            );
        }
    }

    #[test]
    fn rejects_an_invalid_method_handle_reference_kind() {
        assert_eq!(
            decode_single_entry(CONSTANT_METHOD_HANDLE_TAG, &[0x00, 0x00, 0x01]),
            Err(ConstantPoolError::InvalidMethodHandleReferenceKind { index: 1, kind: 0 })
        );
    }

    #[test]
    fn decodes_a_method_type_entry() {
        assert_eq!(
            decode_single_entry(CONSTANT_METHOD_TYPE_TAG, &[0x00, 0x01]),
            Ok(ConstantPoolEntry::MethodType {
                descriptor_index: ConstantPoolIndex(1),
            })
        );
    }

    #[test]
    fn decodes_a_dynamic_entry() {
        assert_eq!(
            decode_single_entry(CONSTANT_DYNAMIC_TAG, &[0x00, 0x00, 0x00, 0x01]),
            Ok(ConstantPoolEntry::Dynamic {
                bootstrap_method_attr_index: BootstrapMethodIndex(0),
                name_and_type_index: ConstantPoolIndex(1),
            })
        );
    }

    #[test]
    fn decodes_an_invoke_dynamic_entry() {
        assert_eq!(
            decode_single_entry(CONSTANT_INVOKE_DYNAMIC_TAG, &[0x00, 0x00, 0x00, 0x01]),
            Ok(ConstantPoolEntry::InvokeDynamic {
                bootstrap_method_attr_index: BootstrapMethodIndex(0),
                name_and_type_index: ConstantPoolIndex(1),
            })
        );
    }

    #[test]
    fn decodes_a_module_entry() {
        assert_eq!(
            decode_single_entry(CONSTANT_MODULE_TAG, &[0x00, 0x01]),
            Ok(ConstantPoolEntry::Module {
                name_index: ConstantPoolIndex(1),
            })
        );
    }

    #[test]
    fn decodes_a_package_entry() {
        assert_eq!(
            decode_single_entry(CONSTANT_PACKAGE_TAG, &[0x00, 0x01]),
            Ok(ConstantPoolEntry::Package {
                name_index: ConstantPoolIndex(1),
            })
        );
    }

    #[test]
    fn rejects_an_unknown_tag() {
        assert_eq!(
            decode_single_entry(2, &[]),
            Err(ConstantPoolError::UnknownTag { index: 1, tag: 2 })
        );
    }

    fn method_handle_entry() -> ConstantPoolEntry {
        ConstantPoolEntry::MethodHandle {
            reference_kind: MethodHandleKind::InvokeStatic,
            reference_index: ConstantPoolIndex(1),
        }
    }

    #[test]
    fn utf8_resolves_a_utf8_entry() {
        let pool =
            ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Utf8("hello".to_owned()))]);

        assert_eq!(pool.utf8(ConstantPoolIndex(1)), Ok("hello"));
    }

    #[test]
    fn utf8_reports_an_invalid_index() {
        let pool = ConstantPool::from_entries(vec![]);

        assert_eq!(
            pool.utf8(ConstantPoolIndex(1)),
            Err(PoolRefError::InvalidIndex {
                index: ConstantPoolIndex(1)
            })
        );
    }

    #[test]
    fn utf8_reports_the_wrong_kind() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(1))]);

        assert_eq!(
            pool.utf8(ConstantPoolIndex(1)),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: EntryKind::Utf8,
            })
        );
    }

    #[test]
    fn class_name_resolves_a_class_entry() {
        let pool = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("java/lang/Object".to_owned())),
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(1),
            }),
        ]);

        assert_eq!(
            pool.class_name(ConstantPoolIndex(2)),
            Ok("java/lang/Object")
        );
    }

    #[test]
    fn class_name_reports_an_invalid_index() {
        let pool = ConstantPool::from_entries(vec![]);

        assert_eq!(
            pool.class_name(ConstantPoolIndex(1)),
            Err(PoolRefError::InvalidIndex {
                index: ConstantPoolIndex(1)
            })
        );
    }

    #[test]
    fn class_name_reports_the_wrong_kind() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(1))]);

        assert_eq!(
            pool.class_name(ConstantPoolIndex(1)),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: EntryKind::Class,
            })
        );
    }

    #[test]
    fn class_name_propagates_a_bad_name_index() {
        let pool = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Integer(1)),
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(1),
            }),
        ]);

        assert_eq!(
            pool.class_name(ConstantPoolIndex(2)),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: EntryKind::Utf8,
            })
        );
    }

    #[test]
    fn name_and_type_resolves_both_strings() {
        let pool = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("run".to_owned())),
            Some(ConstantPoolEntry::Utf8("()V".to_owned())),
            Some(ConstantPoolEntry::NameAndType {
                name_index: ConstantPoolIndex(1),
                descriptor_index: ConstantPoolIndex(2),
            }),
        ]);

        assert_eq!(pool.name_and_type(ConstantPoolIndex(3)), Ok(("run", "()V")));
    }

    #[test]
    fn name_and_type_reports_an_invalid_index() {
        let pool = ConstantPool::from_entries(vec![]);

        assert_eq!(
            pool.name_and_type(ConstantPoolIndex(1)),
            Err(PoolRefError::InvalidIndex {
                index: ConstantPoolIndex(1)
            })
        );
    }

    #[test]
    fn name_and_type_reports_the_wrong_kind() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(1))]);

        assert_eq!(
            pool.name_and_type(ConstantPoolIndex(1)),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: EntryKind::NameAndType,
            })
        );
    }

    #[test]
    fn method_handle_resolves_a_method_handle_entry() {
        let pool = ConstantPool::from_entries(vec![Some(method_handle_entry())]);

        assert_eq!(
            pool.method_handle(ConstantPoolIndex(1)),
            Ok(&method_handle_entry())
        );
    }

    #[test]
    fn method_handle_reports_an_invalid_index() {
        let pool = ConstantPool::from_entries(vec![]);

        assert_eq!(
            pool.method_handle(ConstantPoolIndex(1)),
            Err(PoolRefError::InvalidIndex {
                index: ConstantPoolIndex(1)
            })
        );
    }

    #[test]
    fn method_handle_reports_the_wrong_kind() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(1))]);

        assert_eq!(
            pool.method_handle(ConstantPoolIndex(1)),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: EntryKind::MethodHandle,
            })
        );
    }

    #[test]
    fn check_integer_accepts_and_rejects() {
        let good_pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(1))]);
        assert_eq!(good_pool.check_integer(ConstantPoolIndex(1)), Ok(()));

        let bad_pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Long(1))]);
        assert_eq!(
            bad_pool.check_integer(ConstantPoolIndex(1)),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: EntryKind::Integer,
            })
        );

        let empty_pool = ConstantPool::from_entries(vec![]);
        assert_eq!(
            empty_pool.check_integer(ConstantPoolIndex(1)),
            Err(PoolRefError::InvalidIndex {
                index: ConstantPoolIndex(1)
            })
        );
    }

    #[test]
    fn check_long_accepts_and_rejects() {
        let good_pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Long(1))]);
        assert_eq!(good_pool.check_long(ConstantPoolIndex(1)), Ok(()));

        let bad_pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(1))]);
        assert_eq!(
            bad_pool.check_long(ConstantPoolIndex(1)),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: EntryKind::Long,
            })
        );
    }

    #[test]
    fn check_float_accepts_and_rejects() {
        let good_pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Float(1.0))]);
        assert_eq!(good_pool.check_float(ConstantPoolIndex(1)), Ok(()));

        let bad_pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Double(1.0))]);
        assert_eq!(
            bad_pool.check_float(ConstantPoolIndex(1)),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: EntryKind::Float,
            })
        );
    }

    #[test]
    fn check_double_accepts_and_rejects() {
        let good_pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Double(1.0))]);
        assert_eq!(good_pool.check_double(ConstantPoolIndex(1)), Ok(()));

        let bad_pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Float(1.0))]);
        assert_eq!(
            bad_pool.check_double(ConstantPoolIndex(1)),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: EntryKind::Double,
            })
        );
    }

    #[test]
    fn check_loadable_accepts_every_loadable_kind() {
        let entries = vec![
            ConstantPoolEntry::Integer(1),
            ConstantPoolEntry::Float(1.0),
            ConstantPoolEntry::Long(1),
            ConstantPoolEntry::Double(1.0),
            ConstantPoolEntry::String {
                string_index: ConstantPoolIndex(1),
            },
            ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(1),
            },
            method_handle_entry(),
            ConstantPoolEntry::MethodType {
                descriptor_index: ConstantPoolIndex(1),
            },
            ConstantPoolEntry::Dynamic {
                bootstrap_method_attr_index: BootstrapMethodIndex(0),
                name_and_type_index: ConstantPoolIndex(1),
            },
        ];

        for entry in entries {
            let pool = ConstantPool::from_entries(vec![Some(entry.clone())]);
            assert_eq!(
                pool.check_loadable(ConstantPoolIndex(1)),
                Ok(()),
                "{entry:?} should be loadable"
            );
        }
    }

    #[test]
    fn check_loadable_rejects_a_non_loadable_kind() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::NameAndType {
            name_index: ConstantPoolIndex(1),
            descriptor_index: ConstantPoolIndex(1),
        })]);

        assert_eq!(
            pool.check_loadable(ConstantPoolIndex(1)),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: EntryKind::Loadable,
            })
        );
    }

    #[test]
    fn check_loadable_reports_an_invalid_index() {
        let pool = ConstantPool::from_entries(vec![]);

        assert_eq!(
            pool.check_loadable(ConstantPoolIndex(1)),
            Err(PoolRefError::InvalidIndex {
                index: ConstantPoolIndex(1)
            })
        );
    }

    #[test]
    fn check_constant_value_accepts_every_constant_value_kind() {
        let entries = vec![
            ConstantPoolEntry::Integer(1),
            ConstantPoolEntry::Float(1.0),
            ConstantPoolEntry::Long(1),
            ConstantPoolEntry::Double(1.0),
            ConstantPoolEntry::String {
                string_index: ConstantPoolIndex(1),
            },
        ];

        for entry in entries {
            let pool = ConstantPool::from_entries(vec![Some(entry.clone())]);
            assert_eq!(
                pool.check_constant_value(ConstantPoolIndex(1)),
                Ok(()),
                "{entry:?} should be a valid ConstantValue"
            );
        }
    }

    #[test]
    fn check_constant_value_rejects_a_non_constant_value_kind() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Class {
            name_index: ConstantPoolIndex(1),
        })]);

        assert_eq!(
            pool.check_constant_value(ConstantPoolIndex(1)),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: EntryKind::ConstantValue,
            })
        );
    }

    #[test]
    fn check_constant_value_reports_an_invalid_index() {
        let pool = ConstantPool::from_entries(vec![]);

        assert_eq!(
            pool.check_constant_value(ConstantPoolIndex(1)),
            Err(PoolRefError::InvalidIndex {
                index: ConstantPoolIndex(1)
            })
        );
    }

    #[test]
    fn reports_a_truncated_entry_payload() {
        // Fieldref needs 4 payload bytes; only 1 is provided.
        assert_eq!(
            decode_single_entry(CONSTANT_FIELDREF_TAG, &[0x00]),
            Err(ConstantPoolError::Read(ReadError::UnexpectedEof {
                offset: 3,
                needed: 2,
                remaining: 1,
            }))
        );
    }
}
