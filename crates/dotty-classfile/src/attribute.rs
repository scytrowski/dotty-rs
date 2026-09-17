use crate::constant_pool::{ConstantPool, ConstantPoolEntry, ConstantPoolIndex, read_index};
use crate::reader::{ReadError, Reader};
use std::fmt;

/// The lossless, borrowed byte-level view of any `attribute_info` (JVMS
/// §4.7.1). Every attribute has this shape regardless of whether its name is
/// recognized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawAttribute<'a> {
    pub name_index: ConstantPoolIndex,
    pub bytes: &'a [u8],
}

/// One entry of the `InnerClasses` attribute (JVMS §4.7.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InnerClassEntry {
    pub inner_class_info_index: ConstantPoolIndex,
    pub outer_class_info_index: Option<ConstantPoolIndex>,
    pub inner_name_index: Option<ConstantPoolIndex>,
    /// Source-level modifiers of the inner class; may differ from the
    /// inner class's own `access_flags` as compiled.
    pub inner_class_access_flags: u16,
}

/// One component of a `Record` attribute (JVMS §4.7.30). Not the same shape
/// as `field_info`/`method_info`.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordComponentInfo<'a> {
    pub name_index: ConstantPoolIndex,
    pub descriptor_index: ConstantPoolIndex,
    pub attributes: Vec<Attribute<'a>>,
}

/// One entry of a `Code` attribute's exception table (JVMS §4.7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExceptionTableEntry {
    pub start_pc: u16,
    pub end_pc: u16,
    pub handler_pc: u16,
    /// `None` matches any exception (used for `finally` blocks).
    pub catch_type: Option<ConstantPoolIndex>,
}

/// The `Code` attribute (JVMS §4.7.3). `code` is raw bytecode, opaque to
/// this crate; nested attributes such as `LineNumberTable` and
/// `StackMapTable` are not needed for semantic analysis and are expected to
/// surface as [`Attribute::Other`].
#[derive(Debug, Clone, PartialEq)]
pub struct CodeAttribute<'a> {
    pub max_stack: u16,
    pub max_locals: u16,
    pub code: &'a [u8],
    pub exception_table: Vec<ExceptionTableEntry>,
    pub attributes: Vec<Attribute<'a>>,
}

/// One entry of the `BootstrapMethods` attribute (JVMS §4.7.23).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapMethodEntry {
    pub method_ref: ConstantPoolIndex,
    pub arguments: Vec<ConstantPoolIndex>,
}

/// One entry of the `MethodParameters` attribute (JVMS §4.7.24).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodParameterEntry {
    /// `None` means the parameter has no name (`name_index == 0`).
    pub name_index: Option<ConstantPoolIndex>,
    pub access_flags: u16,
}

/// A structured attribute, resolved one level above [`RawAttribute`].
///
/// Covers the attributes needed for symbol-level semantic analysis
/// (`docs/classfile-format-jdk25.md` §7). Annotation attributes and other
/// standard attributes not yet needed (`StackMapTable`, `LineNumberTable`,
/// `Module*`, ...) fall through to [`Attribute::Other`] rather than being
/// dropped.
#[derive(Debug, Clone, PartialEq)]
pub enum Attribute<'a> {
    ConstantValue(ConstantPoolIndex),
    /// Raw index into a `Utf8` entry holding the signature grammar
    /// (`crate::signature`); parsing that string is decoder work.
    Signature(ConstantPoolIndex),
    Exceptions(Vec<ConstantPoolIndex>),
    InnerClasses(Vec<InnerClassEntry>),
    EnclosingMethod {
        class_index: ConstantPoolIndex,
        method_index: Option<ConstantPoolIndex>,
    },
    SourceFile(ConstantPoolIndex),
    Deprecated,
    NestHost(ConstantPoolIndex),
    NestMembers(Vec<ConstantPoolIndex>),
    Record(Vec<RecordComponentInfo<'a>>),
    PermittedSubclasses(Vec<ConstantPoolIndex>),
    Code(CodeAttribute<'a>),
    BootstrapMethods(Vec<BootstrapMethodEntry>),
    MethodParameters(Vec<MethodParameterEntry>),
    /// An attribute name not (yet) modeled above, kept as raw bytes.
    Other(RawAttribute<'a>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttributeError {
    Read(ReadError),
    /// `attribute_name_index` didn't resolve to a `Utf8` constant pool entry.
    InvalidNameIndex {
        name_index: ConstantPoolIndex,
    },
    /// A recognized attribute's fields didn't consume the whole
    /// `attribute_length`-bounded payload.
    TrailingBytes {
        name: String,
        unread: usize,
    },
}

impl fmt::Display for AttributeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::InvalidNameIndex { name_index } => write!(
                formatter,
                "attribute name_index {} does not resolve to a Utf8 entry",
                name_index.0
            ),
            Self::TrailingBytes { name, unread } => write!(
                formatter,
                "{unread} unread trailing byte(s) in attribute {name:?}"
            ),
        }
    }
}

impl std::error::Error for AttributeError {}

impl From<ReadError> for AttributeError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

fn read_index_list(reader: &mut Reader<'_>) -> Result<Vec<ConstantPoolIndex>, ReadError> {
    let count = reader.read_u16()?;
    let mut indices = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        indices.push(read_index(reader)?);
    }
    Ok(indices)
}

/// Reads a `ConstantPoolIndex`, treating `0` as "absent" — the convention
/// several attributes use for an optional constant pool reference.
fn read_optional_index(reader: &mut Reader<'_>) -> Result<Option<ConstantPoolIndex>, ReadError> {
    let index = read_index(reader)?;
    Ok(if index.0 == 0 { None } else { Some(index) })
}

fn read_bootstrap_methods(reader: &mut Reader<'_>) -> Result<Vec<BootstrapMethodEntry>, ReadError> {
    let count = reader.read_u16()?;
    let mut methods = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let method_ref = read_index(reader)?;
        let arguments = read_index_list(reader)?;
        methods.push(BootstrapMethodEntry {
            method_ref,
            arguments,
        });
    }
    Ok(methods)
}

fn read_inner_classes(reader: &mut Reader<'_>) -> Result<Vec<InnerClassEntry>, ReadError> {
    let count = reader.read_u16()?;
    let mut classes = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let inner_class_info_index = read_index(reader)?;
        let outer_class_info_index = read_optional_index(reader)?;
        let inner_name_index = read_optional_index(reader)?;
        let inner_class_access_flags = reader.read_u16()?;
        classes.push(InnerClassEntry {
            inner_class_info_index,
            outer_class_info_index,
            inner_name_index,
            inner_class_access_flags,
        });
    }
    Ok(classes)
}

fn read_method_parameters(reader: &mut Reader<'_>) -> Result<Vec<MethodParameterEntry>, ReadError> {
    let count = reader.read_u8()?;
    let mut parameters = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let name_index = read_optional_index(reader)?;
        let access_flags = reader.read_u16()?;
        parameters.push(MethodParameterEntry {
            name_index,
            access_flags,
        });
    }
    Ok(parameters)
}

impl<'a> Attribute<'a> {
    /// Decodes one `attribute_info` (JVMS §4.7.1), resolving its name
    /// through `constant_pool` to pick a grammar. An unrecognized name
    /// always decodes to [`Attribute::Other`] rather than erroring.
    pub fn decode(
        reader: &mut Reader<'a>,
        constant_pool: &ConstantPool,
    ) -> Result<Self, AttributeError> {
        let name_index = read_index(reader)?;
        let attribute_length = reader.read_u32()?;
        let mut sub_reader = reader.read_sub_reader(attribute_length as usize)?;

        let name = match constant_pool.get(name_index) {
            Some(ConstantPoolEntry::Utf8(name)) => name.as_str(),
            _ => return Err(AttributeError::InvalidNameIndex { name_index }),
        };

        let attribute = match name {
            "ConstantValue" => Attribute::ConstantValue(read_index(&mut sub_reader)?),
            "Signature" => Attribute::Signature(read_index(&mut sub_reader)?),
            "SourceFile" => Attribute::SourceFile(read_index(&mut sub_reader)?),
            "NestHost" => Attribute::NestHost(read_index(&mut sub_reader)?),
            "Deprecated" => Attribute::Deprecated,
            "Exceptions" => Attribute::Exceptions(read_index_list(&mut sub_reader)?),
            "NestMembers" => Attribute::NestMembers(read_index_list(&mut sub_reader)?),
            "PermittedSubclasses" => {
                Attribute::PermittedSubclasses(read_index_list(&mut sub_reader)?)
            }
            "BootstrapMethods" => {
                Attribute::BootstrapMethods(read_bootstrap_methods(&mut sub_reader)?)
            }
            "MethodParameters" => {
                Attribute::MethodParameters(read_method_parameters(&mut sub_reader)?)
            }
            "InnerClasses" => Attribute::InnerClasses(read_inner_classes(&mut sub_reader)?),
            "EnclosingMethod" => {
                let class_index = read_index(&mut sub_reader)?;
                let method_index = read_optional_index(&mut sub_reader)?;
                Attribute::EnclosingMethod {
                    class_index,
                    method_index,
                }
            }
            _ => {
                let bytes = sub_reader.read_bytes(sub_reader.remaining())?;
                return Ok(Attribute::Other(RawAttribute { name_index, bytes }));
            }
        };

        if sub_reader.is_at_end() {
            Ok(attribute)
        } else {
            Err(AttributeError::TrailingBytes {
                name: name.to_owned(),
                unread: sub_reader.remaining(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attribute_info_bytes(name_index: u16, payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&name_index.to_be_bytes());
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    fn pool_with_utf8_name(name: &str) -> ConstantPool {
        ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Utf8(name.to_owned()))])
    }

    #[test]
    fn decodes_a_constant_value_attribute() {
        let pool = pool_with_utf8_name("ConstantValue");
        let bytes = attribute_info_bytes(1, &[0x00, 0x02]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::ConstantValue(ConstantPoolIndex(2)))
        );
    }

    #[test]
    fn decodes_a_signature_attribute() {
        let pool = pool_with_utf8_name("Signature");
        let bytes = attribute_info_bytes(1, &[0x00, 0x03]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::Signature(ConstantPoolIndex(3)))
        );
    }

    #[test]
    fn decodes_a_source_file_attribute() {
        let pool = pool_with_utf8_name("SourceFile");
        let bytes = attribute_info_bytes(1, &[0x00, 0x04]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::SourceFile(ConstantPoolIndex(4)))
        );
    }

    #[test]
    fn decodes_a_nest_host_attribute() {
        let pool = pool_with_utf8_name("NestHost");
        let bytes = attribute_info_bytes(1, &[0x00, 0x05]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::NestHost(ConstantPoolIndex(5)))
        );
    }

    #[test]
    fn decodes_a_deprecated_attribute() {
        let pool = pool_with_utf8_name("Deprecated");
        let bytes = attribute_info_bytes(1, &[]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::Deprecated)
        );
    }

    #[test]
    fn decodes_an_unrecognized_attribute_as_other() {
        let pool = pool_with_utf8_name("CustomAttr");
        let bytes = attribute_info_bytes(1, &[0xAA, 0xBB, 0xCC]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::Other(RawAttribute {
                name_index: ConstantPoolIndex(1),
                bytes: &[0xAA, 0xBB, 0xCC],
            }))
        );
    }

    #[test]
    fn rejects_a_name_index_that_does_not_resolve_to_utf8() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(1))]);
        let bytes = attribute_info_bytes(1, &[]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Err(AttributeError::InvalidNameIndex {
                name_index: ConstantPoolIndex(1),
            })
        );
    }

    #[test]
    fn rejects_trailing_bytes_in_a_recognized_attribute() {
        let pool = pool_with_utf8_name("ConstantValue");
        // ConstantValue must be exactly 2 bytes; 2 extra bytes follow.
        let bytes = attribute_info_bytes(1, &[0x00, 0x02, 0xFF, 0xFF]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Err(AttributeError::TrailingBytes {
                name: "ConstantValue".to_owned(),
                unread: 2,
            })
        );
    }

    #[test]
    fn decodes_an_exceptions_attribute() {
        let pool = pool_with_utf8_name("Exceptions");
        let bytes = attribute_info_bytes(1, &[0x00, 0x02, 0x00, 0x03, 0x00, 0x04]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::Exceptions(vec![
                ConstantPoolIndex(3),
                ConstantPoolIndex(4)
            ]))
        );
    }

    #[test]
    fn decodes_an_empty_exceptions_attribute() {
        let pool = pool_with_utf8_name("Exceptions");
        let bytes = attribute_info_bytes(1, &[0x00, 0x00]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::Exceptions(vec![]))
        );
    }

    #[test]
    fn decodes_a_nest_members_attribute() {
        let pool = pool_with_utf8_name("NestMembers");
        let bytes = attribute_info_bytes(1, &[0x00, 0x01, 0x00, 0x02]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::NestMembers(vec![ConstantPoolIndex(2)]))
        );
    }

    #[test]
    fn decodes_a_permitted_subclasses_attribute() {
        let pool = pool_with_utf8_name("PermittedSubclasses");
        let bytes = attribute_info_bytes(1, &[0x00, 0x02, 0x00, 0x05, 0x00, 0x06]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::PermittedSubclasses(vec![
                ConstantPoolIndex(5),
                ConstantPoolIndex(6)
            ]))
        );
    }

    #[test]
    fn decodes_a_bootstrap_methods_attribute() {
        let pool = pool_with_utf8_name("BootstrapMethods");
        let bytes = attribute_info_bytes(
            1,
            &[
                0x00, 0x01, // num_bootstrap_methods = 1
                0x00, 0x02, // bootstrap_method_ref = #2
                0x00, 0x02, // num_bootstrap_arguments = 2
                0x00, 0x03, 0x00, 0x04, // arguments: #3, #4
            ],
        );
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::BootstrapMethods(vec![BootstrapMethodEntry {
                method_ref: ConstantPoolIndex(2),
                arguments: vec![ConstantPoolIndex(3), ConstantPoolIndex(4)],
            }]))
        );
    }

    #[test]
    fn decodes_a_bootstrap_method_with_no_arguments() {
        let pool = pool_with_utf8_name("BootstrapMethods");
        let bytes = attribute_info_bytes(
            1,
            &[
                0x00, 0x01, // num_bootstrap_methods = 1
                0x00, 0x02, // bootstrap_method_ref = #2
                0x00, 0x00, // num_bootstrap_arguments = 0
            ],
        );
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::BootstrapMethods(vec![BootstrapMethodEntry {
                method_ref: ConstantPoolIndex(2),
                arguments: vec![],
            }]))
        );
    }

    #[test]
    fn decodes_a_method_parameters_attribute() {
        let pool = pool_with_utf8_name("MethodParameters");
        let bytes = attribute_info_bytes(
            1,
            &[
                0x02, // parameters_count = 2 (u1!)
                0x00, 0x05, 0x00, 0x10, // name_index=#5, access_flags=0x0010 (FINAL)
                0x00, 0x00, 0x80,
                0x00, // name_index=0 (unnamed), access_flags=0x8000 (MANDATED)
            ],
        );
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::MethodParameters(vec![
                MethodParameterEntry {
                    name_index: Some(ConstantPoolIndex(5)),
                    access_flags: 0x0010,
                },
                MethodParameterEntry {
                    name_index: None,
                    access_flags: 0x8000,
                },
            ]))
        );
    }

    #[test]
    fn decodes_an_inner_classes_attribute() {
        let pool = pool_with_utf8_name("InnerClasses");
        let bytes = attribute_info_bytes(
            1,
            &[
                0x00, 0x01, // number_of_classes = 1
                0x00, 0x02, // inner_class_info_index = #2
                0x00, 0x00, // outer_class_info_index = 0 (anonymous)
                0x00, 0x00, // inner_name_index = 0 (anonymous)
                0x00, 0x19, // inner_class_access_flags = PUBLIC|STATIC|FINAL
            ],
        );
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::InnerClasses(vec![InnerClassEntry {
                inner_class_info_index: ConstantPoolIndex(2),
                outer_class_info_index: None,
                inner_name_index: None,
                inner_class_access_flags: 0x0019,
            }]))
        );
    }

    #[test]
    fn decodes_an_inner_classes_attribute_with_a_named_member_class() {
        let pool = pool_with_utf8_name("InnerClasses");
        let bytes = attribute_info_bytes(
            1,
            &[
                0x00, 0x01, // number_of_classes = 1
                0x00, 0x02, // inner_class_info_index = #2
                0x00, 0x03, // outer_class_info_index = #3
                0x00, 0x04, // inner_name_index = #4
                0x00, 0x09, // inner_class_access_flags = PUBLIC|STATIC
            ],
        );
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::InnerClasses(vec![InnerClassEntry {
                inner_class_info_index: ConstantPoolIndex(2),
                outer_class_info_index: Some(ConstantPoolIndex(3)),
                inner_name_index: Some(ConstantPoolIndex(4)),
                inner_class_access_flags: 0x0009,
            }]))
        );
    }

    #[test]
    fn decodes_an_enclosing_method_attribute_with_a_method() {
        let pool = pool_with_utf8_name("EnclosingMethod");
        let bytes = attribute_info_bytes(1, &[0x00, 0x02, 0x00, 0x03]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::EnclosingMethod {
                class_index: ConstantPoolIndex(2),
                method_index: Some(ConstantPoolIndex(3)),
            })
        );
    }

    #[test]
    fn decodes_an_enclosing_method_attribute_without_a_method() {
        let pool = pool_with_utf8_name("EnclosingMethod");
        let bytes = attribute_info_bytes(1, &[0x00, 0x02, 0x00, 0x00]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::EnclosingMethod {
                class_index: ConstantPoolIndex(2),
                method_index: None,
            })
        );
    }

    #[test]
    fn reports_a_truncated_index_list_attribute() {
        let pool = pool_with_utf8_name("Exceptions");
        // count says 2 entries, but only one index (2 bytes) follows.
        let bytes = attribute_info_bytes(1, &[0x00, 0x02, 0x00, 0x03]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Err(AttributeError::Read(ReadError::UnexpectedEof {
                offset: 10,
                needed: 2,
                remaining: 0,
            }))
        );
    }

    #[test]
    fn reports_a_truncated_recognized_attribute() {
        let pool = pool_with_utf8_name("ConstantValue");
        // ConstantValue needs 2 payload bytes; only 1 is declared/provided.
        let bytes = attribute_info_bytes(1, &[0x00]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Err(AttributeError::Read(ReadError::UnexpectedEof {
                offset: 6,
                needed: 2,
                remaining: 1,
            }))
        );
    }
}
