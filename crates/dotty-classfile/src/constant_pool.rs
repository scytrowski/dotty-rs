use crate::encode_error::EncodeError;
use crate::reader::{ReadError, Reader};
use crate::writer::Writer;
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
    /// Nested `Code`/`Record` attribute lists exceed their encoder/validator depth limit.
    TooDeeplyNestedAttribute,
    /// Nested annotation or array element values exceed their depth limit.
    TooDeeplyNestedAnnotation,
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
            Self::TooDeeplyNestedAttribute => {
                formatter.write_str("Code/Record attribute nesting exceeds its depth limit")
            }
            Self::TooDeeplyNestedAnnotation => {
                formatter.write_str("annotation element_value nesting exceeds its depth limit")
            }
        }
    }
}

impl std::error::Error for PoolRefError {}

impl ConstantPool {
    pub fn from_entries(entries: Vec<Option<ConstantPoolEntry>>) -> Self {
        Self { entries }
    }

    pub(crate) fn find_utf8(&self, value: &str) -> Option<ConstantPoolIndex> {
        self.entries.iter().enumerate().find_map(|(slot, entry)| {
            if matches!(entry, Some(ConstantPoolEntry::Utf8(candidate)) if candidate == value) {
                u16::try_from(slot.checked_add(1)?)
                    .ok()
                    .map(ConstantPoolIndex)
            } else {
                None
            }
        })
    }

    pub(crate) fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Encodes `constant_pool_count` and all stored entries in their current
    /// slot order (JVMS §4.4).
    ///
    /// This is structural serialization only. It preserves indices and
    /// duplicates, validates reserved `Long`/`Double` slots and constant-pool
    /// references, and does not resolve `BootstrapMethods` entries or encode
    /// any enclosing class-file structures. No bytes are returned if the
    /// model is invalid.
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        self.validate_for_encoding()?;

        let count = self
            .entries
            .len()
            .checked_add(1)
            .ok_or(EncodeError::CountOverflow {
                value: usize::MAX,
                width: crate::encode_error::LengthWidth::U16,
            })?;

        let mut writer = Writer::new();
        writer.write_u2_length(count)?;
        for entry in self.entries.iter().flatten() {
            write_entry(&mut writer, entry)?;
        }
        Ok(writer.into_bytes())
    }

    pub(crate) fn validate_for_encoding(&self) -> Result<(), EncodeError> {
        let count = self
            .entries
            .len()
            .checked_add(1)
            .ok_or(EncodeError::CountOverflow {
                value: usize::MAX,
                width: crate::encode_error::LengthWidth::U16,
            })?;
        crate::writer::checked_u16_len(count)?;

        let mut slot = 0;
        while slot < self.entries.len() {
            match self.entries.get(slot) {
                Some(Some(ConstantPoolEntry::Long(_) | ConstantPoolEntry::Double(_))) => {
                    if !matches!(self.entries.get(slot + 1), Some(None)) {
                        return Err(EncodeError::InvalidLayout);
                    }
                    slot += 2;
                }
                Some(Some(_)) => slot += 1,
                Some(None) | None => return Err(EncodeError::InvalidLayout),
            }
        }

        for entry in &self.entries {
            let Some(entry) = entry else { continue };
            match entry {
                ConstantPoolEntry::Utf8(value) => {
                    // Check the complete modified UTF-8 payload before output
                    // is started; `Writer::write_modified_utf8` repeats this
                    // while producing the final bytes.
                    crate::modified_utf8::encode_modified_utf8(value)?;
                }
                ConstantPoolEntry::Integer(_)
                | ConstantPoolEntry::Float(_)
                | ConstantPoolEntry::Long(_)
                | ConstantPoolEntry::Double(_) => {}
                ConstantPoolEntry::Class { name_index }
                | ConstantPoolEntry::Module { name_index }
                | ConstantPoolEntry::Package { name_index } => {
                    self.validate_reference(*name_index, is_utf8)?;
                }
                ConstantPoolEntry::String { string_index } => {
                    self.validate_reference(*string_index, is_utf8)?;
                }
                ConstantPoolEntry::Fieldref {
                    class_index,
                    name_and_type_index,
                }
                | ConstantPoolEntry::Methodref {
                    class_index,
                    name_and_type_index,
                }
                | ConstantPoolEntry::InterfaceMethodref {
                    class_index,
                    name_and_type_index,
                } => {
                    self.validate_reference(*class_index, is_class)?;
                    self.validate_reference(*name_and_type_index, is_name_and_type)?;
                }
                ConstantPoolEntry::NameAndType {
                    name_index,
                    descriptor_index,
                } => {
                    self.validate_reference(*name_index, is_utf8)?;
                    self.validate_reference(*descriptor_index, is_utf8)?;
                }
                ConstantPoolEntry::MethodHandle {
                    reference_kind,
                    reference_index,
                } => {
                    self.validate_method_handle_reference(*reference_kind, *reference_index)?;
                }
                ConstantPoolEntry::MethodType { descriptor_index } => {
                    self.validate_reference(*descriptor_index, is_utf8)?;
                }
                ConstantPoolEntry::Dynamic {
                    name_and_type_index,
                    ..
                }
                | ConstantPoolEntry::InvokeDynamic {
                    name_and_type_index,
                    ..
                } => {
                    self.validate_reference(*name_and_type_index, is_name_and_type)?;
                }
            }
        }

        Ok(())
    }

    fn validate_reference(
        &self,
        index: ConstantPoolIndex,
        expected: fn(&ConstantPoolEntry) -> bool,
    ) -> Result<(), EncodeError> {
        let raw_index = usize::from(index.0);
        let slot = raw_index
            .checked_sub(1)
            .and_then(|slot| self.entries.get(slot))
            .ok_or(EncodeError::InvalidIndex {
                index: raw_index,
                count: self.entries.len(),
            })?;

        match slot {
            Some(entry) if expected(entry) => Ok(()),
            Some(_) | None => Err(EncodeError::InvalidReference { index: raw_index }),
        }
    }

    fn validate_method_handle_reference(
        &self,
        kind: MethodHandleKind,
        index: ConstantPoolIndex,
    ) -> Result<(), EncodeError> {
        match kind {
            MethodHandleKind::GetField
            | MethodHandleKind::GetStatic
            | MethodHandleKind::PutField
            | MethodHandleKind::PutStatic => self.validate_reference(index, is_fieldref),
            MethodHandleKind::InvokeVirtual | MethodHandleKind::NewInvokeSpecial => {
                self.validate_reference(index, is_methodref)
            }
            MethodHandleKind::InvokeStatic | MethodHandleKind::InvokeSpecial => self
                .validate_reference(index, |entry| {
                    is_methodref(entry) || is_interface_methodref(entry)
                }),
            MethodHandleKind::InvokeInterface => {
                self.validate_reference(index, is_interface_methodref)
            }
        }?;

        if matches!(
            kind,
            MethodHandleKind::GetField
                | MethodHandleKind::GetStatic
                | MethodHandleKind::PutField
                | MethodHandleKind::PutStatic
        ) {
            return Ok(());
        }

        let target_index = usize::from(index.0);
        let target = self.get(index).ok_or(EncodeError::InvalidIndex {
            index: target_index,
            count: self.entries.len(),
        })?;
        let name_and_type_index = match target {
            ConstantPoolEntry::Methodref {
                name_and_type_index,
                ..
            }
            | ConstantPoolEntry::InterfaceMethodref {
                name_and_type_index,
                ..
            } => *name_and_type_index,
            _ => {
                return Err(EncodeError::InvalidReference {
                    index: target_index,
                });
            }
        };
        self.validate_reference(name_and_type_index, is_name_and_type)?;
        let (name_index, _) = match self.get(name_and_type_index) {
            Some(ConstantPoolEntry::NameAndType {
                name_index,
                descriptor_index,
            }) => (*name_index, *descriptor_index),
            _ => {
                return Err(EncodeError::InvalidReference {
                    index: usize::from(name_and_type_index.0),
                });
            }
        };
        self.validate_reference(name_index, is_utf8)?;
        let name = match self.get(name_index) {
            Some(ConstantPoolEntry::Utf8(name)) => name.as_str(),
            _ => {
                return Err(EncodeError::InvalidReference {
                    index: usize::from(name_index.0),
                });
            }
        };

        let valid_name = match kind {
            MethodHandleKind::NewInvokeSpecial => name == "<init>",
            MethodHandleKind::InvokeVirtual
            | MethodHandleKind::InvokeStatic
            | MethodHandleKind::InvokeSpecial
            | MethodHandleKind::InvokeInterface => name != "<init>" && name != "<clinit>",
            MethodHandleKind::GetField
            | MethodHandleKind::GetStatic
            | MethodHandleKind::PutField
            | MethodHandleKind::PutStatic => true,
        };
        if valid_name {
            Ok(())
        } else {
            Err(EncodeError::InvalidMethodHandleName {
                index: target_index,
            })
        }
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

fn is_utf8(entry: &ConstantPoolEntry) -> bool {
    matches!(entry, ConstantPoolEntry::Utf8(_))
}

fn is_class(entry: &ConstantPoolEntry) -> bool {
    matches!(entry, ConstantPoolEntry::Class { .. })
}

fn is_name_and_type(entry: &ConstantPoolEntry) -> bool {
    matches!(entry, ConstantPoolEntry::NameAndType { .. })
}

fn is_fieldref(entry: &ConstantPoolEntry) -> bool {
    matches!(entry, ConstantPoolEntry::Fieldref { .. })
}

fn is_methodref(entry: &ConstantPoolEntry) -> bool {
    matches!(entry, ConstantPoolEntry::Methodref { .. })
}

fn is_interface_methodref(entry: &ConstantPoolEntry) -> bool {
    matches!(entry, ConstantPoolEntry::InterfaceMethodref { .. })
}

fn write_entry(writer: &mut Writer, entry: &ConstantPoolEntry) -> Result<(), EncodeError> {
    match entry {
        ConstantPoolEntry::Utf8(value) => {
            writer.write_u1(CONSTANT_UTF8_TAG);
            writer.write_modified_utf8(value)?;
        }
        ConstantPoolEntry::Integer(value) => {
            writer.write_u1(CONSTANT_INTEGER_TAG);
            writer.write_bytes(&value.to_be_bytes());
        }
        ConstantPoolEntry::Float(value) => {
            writer.write_u1(CONSTANT_FLOAT_TAG);
            writer.write_u4(value.to_bits());
        }
        ConstantPoolEntry::Long(value) => {
            writer.write_u1(CONSTANT_LONG_TAG);
            writer.write_bytes(&value.to_be_bytes());
        }
        ConstantPoolEntry::Double(value) => {
            writer.write_u1(CONSTANT_DOUBLE_TAG);
            writer.write_bytes(&value.to_bits().to_be_bytes());
        }
        ConstantPoolEntry::Class { name_index } => {
            writer.write_u1(CONSTANT_CLASS_TAG);
            writer.write_u2(name_index.0);
        }
        ConstantPoolEntry::String { string_index } => {
            writer.write_u1(CONSTANT_STRING_TAG);
            writer.write_u2(string_index.0);
        }
        ConstantPoolEntry::Fieldref {
            class_index,
            name_and_type_index,
        } => {
            writer.write_u1(CONSTANT_FIELDREF_TAG);
            writer.write_u2(class_index.0);
            writer.write_u2(name_and_type_index.0);
        }
        ConstantPoolEntry::Methodref {
            class_index,
            name_and_type_index,
        } => {
            writer.write_u1(CONSTANT_METHODREF_TAG);
            writer.write_u2(class_index.0);
            writer.write_u2(name_and_type_index.0);
        }
        ConstantPoolEntry::InterfaceMethodref {
            class_index,
            name_and_type_index,
        } => {
            writer.write_u1(CONSTANT_INTERFACE_METHODREF_TAG);
            writer.write_u2(class_index.0);
            writer.write_u2(name_and_type_index.0);
        }
        ConstantPoolEntry::NameAndType {
            name_index,
            descriptor_index,
        } => {
            writer.write_u1(CONSTANT_NAME_AND_TYPE_TAG);
            writer.write_u2(name_index.0);
            writer.write_u2(descriptor_index.0);
        }
        ConstantPoolEntry::MethodHandle {
            reference_kind,
            reference_index,
        } => {
            writer.write_u1(CONSTANT_METHOD_HANDLE_TAG);
            writer.write_u1(method_handle_kind_tag(*reference_kind));
            writer.write_u2(reference_index.0);
        }
        ConstantPoolEntry::MethodType { descriptor_index } => {
            writer.write_u1(CONSTANT_METHOD_TYPE_TAG);
            writer.write_u2(descriptor_index.0);
        }
        ConstantPoolEntry::Dynamic {
            bootstrap_method_attr_index,
            name_and_type_index,
        } => {
            writer.write_u1(CONSTANT_DYNAMIC_TAG);
            writer.write_u2(bootstrap_method_attr_index.0);
            writer.write_u2(name_and_type_index.0);
        }
        ConstantPoolEntry::InvokeDynamic {
            bootstrap_method_attr_index,
            name_and_type_index,
        } => {
            writer.write_u1(CONSTANT_INVOKE_DYNAMIC_TAG);
            writer.write_u2(bootstrap_method_attr_index.0);
            writer.write_u2(name_and_type_index.0);
        }
        ConstantPoolEntry::Module { name_index } => {
            writer.write_u1(CONSTANT_MODULE_TAG);
            writer.write_u2(name_index.0);
        }
        ConstantPoolEntry::Package { name_index } => {
            writer.write_u1(CONSTANT_PACKAGE_TAG);
            writer.write_u2(name_index.0);
        }
    }
    Ok(())
}

fn method_handle_kind_tag(kind: MethodHandleKind) -> u8 {
    match kind {
        MethodHandleKind::GetField => 1,
        MethodHandleKind::GetStatic => 2,
        MethodHandleKind::PutField => 3,
        MethodHandleKind::PutStatic => 4,
        MethodHandleKind::InvokeVirtual => 5,
        MethodHandleKind::InvokeStatic => 6,
        MethodHandleKind::InvokeSpecial => 7,
        MethodHandleKind::NewInvokeSpecial => 8,
        MethodHandleKind::InvokeInterface => 9,
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

    fn serialization_matrix_pool() -> ConstantPool {
        ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Integer(-2)),
            Some(ConstantPoolEntry::Float(f32::from_bits(0x8000_0000))),
            Some(ConstantPoolEntry::Long(i64::MIN)),
            None,
            Some(ConstantPoolEntry::Double(f64::from_bits(
                0x7FF8_0000_0000_0042,
            ))),
            None,
            Some(ConstantPoolEntry::Utf8("C".to_owned())),
            Some(ConstantPoolEntry::Utf8("n".to_owned())),
            Some(ConstantPoolEntry::Utf8("()V".to_owned())),
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(7),
            }),
            Some(ConstantPoolEntry::String {
                string_index: ConstantPoolIndex(8),
            }),
            Some(ConstantPoolEntry::NameAndType {
                name_index: ConstantPoolIndex(8),
                descriptor_index: ConstantPoolIndex(9),
            }),
            Some(ConstantPoolEntry::Fieldref {
                class_index: ConstantPoolIndex(10),
                name_and_type_index: ConstantPoolIndex(12),
            }),
            Some(ConstantPoolEntry::Methodref {
                class_index: ConstantPoolIndex(10),
                name_and_type_index: ConstantPoolIndex(12),
            }),
            Some(ConstantPoolEntry::InterfaceMethodref {
                class_index: ConstantPoolIndex(10),
                name_and_type_index: ConstantPoolIndex(12),
            }),
            Some(ConstantPoolEntry::MethodHandle {
                reference_kind: MethodHandleKind::GetField,
                reference_index: ConstantPoolIndex(13),
            }),
            Some(ConstantPoolEntry::MethodHandle {
                reference_kind: MethodHandleKind::GetStatic,
                reference_index: ConstantPoolIndex(13),
            }),
            Some(ConstantPoolEntry::MethodHandle {
                reference_kind: MethodHandleKind::PutField,
                reference_index: ConstantPoolIndex(13),
            }),
            Some(ConstantPoolEntry::MethodHandle {
                reference_kind: MethodHandleKind::PutStatic,
                reference_index: ConstantPoolIndex(13),
            }),
            Some(ConstantPoolEntry::MethodHandle {
                reference_kind: MethodHandleKind::InvokeVirtual,
                reference_index: ConstantPoolIndex(14),
            }),
            Some(ConstantPoolEntry::MethodHandle {
                reference_kind: MethodHandleKind::InvokeStatic,
                reference_index: ConstantPoolIndex(14),
            }),
            Some(ConstantPoolEntry::MethodHandle {
                reference_kind: MethodHandleKind::InvokeSpecial,
                reference_index: ConstantPoolIndex(14),
            }),
            Some(ConstantPoolEntry::MethodHandle {
                reference_kind: MethodHandleKind::NewInvokeSpecial,
                reference_index: ConstantPoolIndex(33),
            }),
            Some(ConstantPoolEntry::MethodHandle {
                reference_kind: MethodHandleKind::InvokeInterface,
                reference_index: ConstantPoolIndex(15),
            }),
            Some(ConstantPoolEntry::MethodType {
                descriptor_index: ConstantPoolIndex(9),
            }),
            Some(ConstantPoolEntry::Dynamic {
                bootstrap_method_attr_index: BootstrapMethodIndex(0x1234),
                name_and_type_index: ConstantPoolIndex(12),
            }),
            Some(ConstantPoolEntry::InvokeDynamic {
                bootstrap_method_attr_index: BootstrapMethodIndex(0xABCD),
                name_and_type_index: ConstantPoolIndex(12),
            }),
            Some(ConstantPoolEntry::Module {
                name_index: ConstantPoolIndex(7),
            }),
            Some(ConstantPoolEntry::Package {
                name_index: ConstantPoolIndex(8),
            }),
            Some(ConstantPoolEntry::Utf8("u\0😀".to_owned())),
            Some(ConstantPoolEntry::Utf8("<init>".to_owned())),
            Some(ConstantPoolEntry::NameAndType {
                name_index: ConstantPoolIndex(31),
                descriptor_index: ConstantPoolIndex(9),
            }),
            Some(ConstantPoolEntry::Methodref {
                class_index: ConstantPoolIndex(10),
                name_and_type_index: ConstantPoolIndex(32),
            }),
        ])
    }

    fn assert_pool_eq_bitwise(expected: &ConstantPool, actual: &ConstantPool) {
        assert_eq!(expected.entries.len(), actual.entries.len());
        for (expected, actual) in expected.entries.iter().zip(&actual.entries) {
            match (expected, actual) {
                (
                    Some(ConstantPoolEntry::Float(expected)),
                    Some(ConstantPoolEntry::Float(actual)),
                ) => {
                    assert_eq!(expected.to_bits(), actual.to_bits());
                }
                (
                    Some(ConstantPoolEntry::Double(expected)),
                    Some(ConstantPoolEntry::Double(actual)),
                ) => {
                    assert_eq!(expected.to_bits(), actual.to_bits());
                }
                (expected, actual) => assert_eq!(expected, actual),
            }
        }
    }

    #[test]
    fn encode_writes_the_tag_and_payload_for_every_entry_variant() {
        let pool = serialization_matrix_pool();
        let encoded = pool.encode().unwrap();

        assert_eq!(
            encoded,
            [
                0x00, 0x22, // constant_pool_count: 33 slots + slot zero
                0x03, 0xFF, 0xFF, 0xFF, 0xFE, // Integer(-2)
                0x04, 0x80, 0x00, 0x00, 0x00, // Float(-0.0)
                0x05, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Long(i64::MIN)
                0x06, 0x7F, 0xF8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x42, // Double(NaN payload)
                0x01, 0x00, 0x01, b'C', // Utf8
                0x01, 0x00, 0x01, b'n', // Utf8
                0x01, 0x00, 0x03, b'(', b')', b'V', // Utf8
                0x07, 0x00, 0x07, // Class
                0x08, 0x00, 0x08, // String
                0x0C, 0x00, 0x08, 0x00, 0x09, // NameAndType
                0x09, 0x00, 0x0A, 0x00, 0x0C, // Fieldref
                0x0A, 0x00, 0x0A, 0x00, 0x0C, // Methodref
                0x0B, 0x00, 0x0A, 0x00, 0x0C, // InterfaceMethodref
                0x0F, 0x01, 0x00, 0x0D, // GetField
                0x0F, 0x02, 0x00, 0x0D, // GetStatic
                0x0F, 0x03, 0x00, 0x0D, // PutField
                0x0F, 0x04, 0x00, 0x0D, // PutStatic
                0x0F, 0x05, 0x00, 0x0E, // InvokeVirtual
                0x0F, 0x06, 0x00, 0x0E, // InvokeStatic
                0x0F, 0x07, 0x00, 0x0E, // InvokeSpecial
                0x0F, 0x08, 0x00, 0x21, // NewInvokeSpecial
                0x0F, 0x09, 0x00, 0x0F, // InvokeInterface
                0x10, 0x00, 0x09, // MethodType
                0x11, 0x12, 0x34, 0x00, 0x0C, // Dynamic
                0x12, 0xAB, 0xCD, 0x00, 0x0C, // InvokeDynamic
                0x13, 0x00, 0x07, // Module
                0x14, 0x00, 0x08, // Package
                0x01, 0x00, 0x09, b'u', 0xC0, 0x80, 0xED, 0xA0, 0xBD, 0xED, 0xB8,
                0x80, // Utf8 MUTF-8
                0x01, 0x00, 0x06, b'<', b'i', b'n', b'i', b't', b'>', // Utf8 <init>
                0x0C, 0x00, 0x1F, 0x00, 0x09, // NameAndType <init>
                0x0A, 0x00, 0x0A, 0x00, 0x20, // Methodref <init>
            ]
        );
    }

    #[test]
    fn encode_decode_preserves_every_slot_and_float_bit_pattern() {
        let pool = serialization_matrix_pool();
        let encoded = pool.encode().unwrap();
        let mut reader = Reader::new(&encoded);
        let decoded = ConstantPool::decode(&mut reader).unwrap();

        assert!(reader.is_at_end());
        assert_pool_eq_bitwise(&pool, &decoded);
    }

    #[test]
    fn encode_preserves_forward_references_duplicates_and_original_order() {
        let pool = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(2),
            }),
            Some(ConstantPoolEntry::Utf8("duplicate".to_owned())),
            Some(ConstantPoolEntry::Utf8("duplicate".to_owned())),
            Some(ConstantPoolEntry::String {
                string_index: ConstantPoolIndex(3),
            }),
        ]);

        let encoded = pool.encode().unwrap();
        assert_eq!(
            encoded,
            [
                0x00, 0x05, 0x07, 0x00, 0x02, // forward Class reference
                0x01, 0x00, 0x09, b'd', b'u', b'p', b'l', b'i', b'c', b'a', b't', b'e', 0x01, 0x00,
                0x09, b'd', b'u', b'p', b'l', b'i', b'c', b'a', b't', b'e', 0x08, 0x00, 0x03,
            ]
        );
        let mut reader = Reader::new(&encoded);
        let decoded = ConstantPool::decode(&mut reader).unwrap();
        assert_pool_eq_bitwise(&pool, &decoded);
    }

    #[test]
    fn encode_rejects_orphaned_or_misplaced_two_slot_entries() {
        let invalid_pools = [
            ConstantPool::from_entries(vec![None]),
            ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Long(1))]),
            ConstantPool::from_entries(vec![
                Some(ConstantPoolEntry::Long(1)),
                Some(ConstantPoolEntry::Integer(2)),
            ]),
            ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Long(1)), None, None]),
        ];

        for pool in invalid_pools {
            assert_eq!(pool.encode(), Err(EncodeError::InvalidLayout));
        }
    }

    #[test]
    fn encode_rejects_malformed_references_and_method_handle_targets() {
        let invalid_index = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Class {
            name_index: ConstantPoolIndex(0),
        })]);
        assert_eq!(
            invalid_index.encode(),
            Err(EncodeError::InvalidIndex { index: 0, count: 1 })
        );

        let out_of_range = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Class {
            name_index: ConstantPoolIndex(2),
        })]);
        assert_eq!(
            out_of_range.encode(),
            Err(EncodeError::InvalidIndex { index: 2, count: 1 })
        );

        let wrong_kind = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Integer(1)),
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(1),
            }),
        ]);
        assert_eq!(
            wrong_kind.encode(),
            Err(EncodeError::InvalidReference { index: 1 })
        );

        let bad_handle_target = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Integer(1)),
            Some(ConstantPoolEntry::MethodHandle {
                reference_kind: MethodHandleKind::GetField,
                reference_index: ConstantPoolIndex(1),
            }),
        ]);
        assert_eq!(
            bad_handle_target.encode(),
            Err(EncodeError::InvalidReference { index: 1 })
        );

        let reserved_slot_reference = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Long(1)),
            None,
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(2),
            }),
            Some(ConstantPoolEntry::Utf8("C".to_owned())),
        ]);
        assert_eq!(
            reserved_slot_reference.encode(),
            Err(EncodeError::InvalidReference { index: 2 })
        );
    }

    fn pool_with_method_handle(kind: MethodHandleKind, method_name: &str) -> ConstantPool {
        ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("C".to_owned())),
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(1),
            }),
            Some(ConstantPoolEntry::Utf8(method_name.to_owned())),
            Some(ConstantPoolEntry::Utf8("()V".to_owned())),
            Some(ConstantPoolEntry::NameAndType {
                name_index: ConstantPoolIndex(3),
                descriptor_index: ConstantPoolIndex(4),
            }),
            Some(ConstantPoolEntry::Methodref {
                class_index: ConstantPoolIndex(2),
                name_and_type_index: ConstantPoolIndex(5),
            }),
            Some(ConstantPoolEntry::InterfaceMethodref {
                class_index: ConstantPoolIndex(2),
                name_and_type_index: ConstantPoolIndex(5),
            }),
            Some(ConstantPoolEntry::MethodHandle {
                reference_kind: kind,
                reference_index: ConstantPoolIndex(if kind == MethodHandleKind::InvokeInterface {
                    7
                } else {
                    6
                }),
            }),
        ])
    }

    #[test]
    fn encode_validates_method_handle_target_names() {
        for kind in [
            MethodHandleKind::InvokeVirtual,
            MethodHandleKind::InvokeStatic,
            MethodHandleKind::InvokeSpecial,
            MethodHandleKind::InvokeInterface,
        ] {
            for method_name in ["<init>", "<clinit>"] {
                assert_eq!(
                    pool_with_method_handle(kind, method_name).encode(),
                    Err(EncodeError::InvalidMethodHandleName {
                        index: if kind == MethodHandleKind::InvokeInterface {
                            7
                        } else {
                            6
                        }
                    }),
                    "{kind:?} must reject {method_name}"
                );
            }
        }

        assert_eq!(
            pool_with_method_handle(MethodHandleKind::NewInvokeSpecial, "ordinary").encode(),
            Err(EncodeError::InvalidMethodHandleName { index: 6 })
        );
        assert!(
            pool_with_method_handle(MethodHandleKind::NewInvokeSpecial, "<init>")
                .encode()
                .is_ok()
        );
        assert!(
            pool_with_method_handle(MethodHandleKind::InvokeVirtual, "ordinary")
                .encode()
                .is_ok()
        );
    }

    #[test]
    fn encode_rejects_a_modified_utf8_payload_that_exceeds_u2() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Utf8(
            "a".repeat(usize::from(u16::MAX) + 1),
        ))]);
        assert_eq!(
            pool.encode(),
            Err(EncodeError::Utf8LengthOverflow {
                byte_len: usize::from(u16::MAX) + 1,
                max: u16::MAX,
            })
        );
    }

    #[test]
    fn encode_preserves_long_and_double_slots_at_pool_boundaries() {
        let pools = [
            ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Long(1)), None]),
            ConstantPool::from_entries(vec![
                Some(ConstantPoolEntry::Integer(1)),
                Some(ConstantPoolEntry::Double(2.0)),
                None,
            ]),
            ConstantPool::from_entries(vec![
                Some(ConstantPoolEntry::Integer(1)),
                Some(ConstantPoolEntry::Long(2)),
                None,
                Some(ConstantPoolEntry::Double(3.0)),
                None,
            ]),
        ];

        for pool in pools {
            let encoded = pool.encode().unwrap();
            let mut reader = Reader::new(&encoded);
            let decoded = ConstantPool::decode(&mut reader).unwrap();
            assert_pool_eq_bitwise(&pool, &decoded);
            assert!(reader.is_at_end());
        }
    }

    #[test]
    fn encode_preserves_signed_zero_and_noncanonical_nan_payloads() {
        let pool = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Float(f32::from_bits(0x7FC0_0042))),
            Some(ConstantPoolEntry::Float(-0.0)),
            Some(ConstantPoolEntry::Double(f64::from_bits(
                0x7FF0_0000_0000_0042,
            ))),
            None,
            Some(ConstantPoolEntry::Double(-0.0)),
            None,
        ]);
        let encoded = pool.encode().unwrap();
        let mut reader = Reader::new(&encoded);
        let decoded = ConstantPool::decode(&mut reader).unwrap();

        assert_pool_eq_bitwise(&pool, &decoded);
    }

    #[test]
    fn encode_checks_maximum_constant_pool_count_without_repairing_layout() {
        let maximum = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Integer(0));
            usize::from(u16::MAX) - 1
        ]);
        let bytes = maximum.encode().unwrap();
        assert_eq!(&bytes[..2], &u16::MAX.to_be_bytes());

        let too_many = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Integer(0));
            usize::from(u16::MAX)
        ]);
        assert_eq!(
            too_many.encode(),
            Err(EncodeError::CountOverflow {
                value: usize::from(u16::MAX) + 1,
                width: crate::encode_error::LengthWidth::U16,
            })
        );
    }

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
