use crate::constant_pool::{
    ConstantPool, ConstantPoolEntry, ConstantPoolIndex, PoolRefError, read_index,
};
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

pub(crate) fn read_index_list(
    reader: &mut Reader<'_>,
) -> Result<Vec<ConstantPoolIndex>, ReadError> {
    let count = reader.read_u16()?;
    let mut indices = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        indices.push(read_index(reader)?);
    }
    Ok(indices)
}

/// Reads a `ConstantPoolIndex`, treating `0` as "absent" — the convention
/// several attributes use for an optional constant pool reference.
pub(crate) fn read_optional_index(
    reader: &mut Reader<'_>,
) -> Result<Option<ConstantPoolIndex>, ReadError> {
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

fn read_exception_table(reader: &mut Reader<'_>) -> Result<Vec<ExceptionTableEntry>, ReadError> {
    let count = reader.read_u16()?;
    let mut entries = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let start_pc = reader.read_u16()?;
        let end_pc = reader.read_u16()?;
        let handler_pc = reader.read_u16()?;
        let catch_type = read_optional_index(reader)?;
        entries.push(ExceptionTableEntry {
            start_pc,
            end_pc,
            handler_pc,
            catch_type,
        });
    }
    Ok(entries)
}

fn read_code<'a>(
    reader: &mut Reader<'a>,
    constant_pool: &ConstantPool,
) -> Result<CodeAttribute<'a>, AttributeError> {
    let max_stack = reader.read_u16()?;
    let max_locals = reader.read_u16()?;
    let code_length = reader.read_u32()?;
    let code = reader.read_bytes(code_length as usize)?;
    let exception_table = read_exception_table(reader)?;
    let attributes = decode_attributes(reader, constant_pool)?;
    Ok(CodeAttribute {
        max_stack,
        max_locals,
        code,
        exception_table,
        attributes,
    })
}

/// Decodes a `u2` count followed by that many `attribute_info` structures —
/// the shared shape of `ClassFile`, `field_info`, `method_info`, `Code`, and
/// `record_component_info`'s attribute lists.
pub(crate) fn decode_attributes<'a>(
    reader: &mut Reader<'a>,
    constant_pool: &ConstantPool,
) -> Result<Vec<Attribute<'a>>, AttributeError> {
    let count = reader.read_u16()?;
    let mut attributes = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        attributes.push(Attribute::decode(reader, constant_pool)?);
    }
    Ok(attributes)
}

fn read_record_components<'a>(
    reader: &mut Reader<'a>,
    constant_pool: &ConstantPool,
) -> Result<Vec<RecordComponentInfo<'a>>, AttributeError> {
    let count = reader.read_u16()?;
    let mut components = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let name_index = read_index(reader)?;
        let descriptor_index = read_index(reader)?;
        let attributes = decode_attributes(reader, constant_pool)?;
        components.push(RecordComponentInfo {
            name_index,
            descriptor_index,
            attributes,
        });
    }
    Ok(components)
}

/// Validates the constant pool references inside a single attribute (JVMS
/// §4.7), recursing into nested attribute lists (`Record`, `Code`).
fn validate_attribute(attribute: &Attribute<'_>, pool: &ConstantPool) -> Result<(), PoolRefError> {
    match attribute {
        Attribute::ConstantValue(index) => pool.check_constant_value(*index),
        Attribute::Signature(index) | Attribute::SourceFile(index) => pool.utf8(*index).map(|_| ()),
        Attribute::Exceptions(indices)
        | Attribute::NestMembers(indices)
        | Attribute::PermittedSubclasses(indices) => {
            for index in indices {
                pool.class_name(*index)?;
            }
            Ok(())
        }
        Attribute::InnerClasses(entries) => {
            for entry in entries {
                pool.class_name(entry.inner_class_info_index)?;
                if let Some(outer_class_info_index) = entry.outer_class_info_index {
                    pool.class_name(outer_class_info_index)?;
                }
                if let Some(inner_name_index) = entry.inner_name_index {
                    pool.utf8(inner_name_index)?;
                }
            }
            Ok(())
        }
        Attribute::EnclosingMethod {
            class_index,
            method_index,
        } => {
            pool.class_name(*class_index)?;
            if let Some(method_index) = method_index {
                pool.name_and_type(*method_index)?;
            }
            Ok(())
        }
        Attribute::Deprecated => Ok(()),
        Attribute::NestHost(index) => pool.class_name(*index).map(|_| ()),
        Attribute::Record(components) => {
            for component in components {
                pool.utf8(component.name_index)?;
                pool.utf8(component.descriptor_index)?;
                validate_attributes(&component.attributes, pool)?;
            }
            Ok(())
        }
        Attribute::Code(code) => {
            for entry in &code.exception_table {
                if let Some(catch_type) = entry.catch_type {
                    pool.class_name(catch_type)?;
                }
            }
            validate_attributes(&code.attributes, pool)
        }
        Attribute::BootstrapMethods(methods) => {
            for method in methods {
                pool.method_handle(method.method_ref)?;
                for argument in &method.arguments {
                    pool.check_loadable(*argument)?;
                }
            }
            Ok(())
        }
        Attribute::MethodParameters(parameters) => {
            for parameter in parameters {
                if let Some(name_index) = parameter.name_index {
                    pool.utf8(name_index)?;
                }
            }
            Ok(())
        }
        Attribute::Other(_) => Ok(()),
    }
}

/// Validates the constant pool references of every attribute in `attributes`
/// (JVMS §4.7), stopping at the first bad reference.
pub(crate) fn validate_attributes(
    attributes: &[Attribute<'_>],
    pool: &ConstantPool,
) -> Result<(), PoolRefError> {
    for attribute in attributes {
        validate_attribute(attribute, pool)?;
    }
    Ok(())
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
            "Record" => Attribute::Record(read_record_components(&mut sub_reader, constant_pool)?),
            "Code" => Attribute::Code(read_code(&mut sub_reader, constant_pool)?),
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
    fn decodes_a_record_attribute_with_no_nested_attributes() {
        let pool = pool_with_utf8_name("Record");
        let bytes = attribute_info_bytes(
            1,
            &[
                0x00, 0x01, // components_count = 1
                0x00, 0x03, // name_index = #3
                0x00, 0x04, // descriptor_index = #4
                0x00, 0x00, // attributes_count = 0
            ],
        );
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::Record(vec![RecordComponentInfo {
                name_index: ConstantPoolIndex(3),
                descriptor_index: ConstantPoolIndex(4),
                attributes: vec![],
            }]))
        );
    }

    #[test]
    fn decodes_a_record_attribute_with_a_nested_signature_attribute() {
        let pool = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("Record".to_owned())),
            Some(ConstantPoolEntry::Utf8("Signature".to_owned())),
        ]);
        let bytes = attribute_info_bytes(
            1,
            &[
                0x00, 0x01, // components_count = 1
                0x00, 0x03, // name_index = #3
                0x00, 0x04, // descriptor_index = #4
                0x00, 0x01, // attributes_count = 1
                0x00, 0x02, // nested attribute_name_index = #2 ("Signature")
                0x00, 0x00, 0x00, 0x02, // nested attribute_length = 2
                0x00, 0x05, // signature_index = #5
            ],
        );
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::Record(vec![RecordComponentInfo {
                name_index: ConstantPoolIndex(3),
                descriptor_index: ConstantPoolIndex(4),
                attributes: vec![Attribute::Signature(ConstantPoolIndex(5))],
            }]))
        );
    }

    #[test]
    fn decodes_a_code_attribute_with_no_exceptions_or_nested_attributes() {
        let pool = pool_with_utf8_name("Code");
        let bytes = attribute_info_bytes(
            1,
            &[
                0x00, 0x01, // max_stack = 1
                0x00, 0x02, // max_locals = 2
                0x00, 0x00, 0x00, 0x01, // code_length = 1
                0xB1, // code: return
                0x00, 0x00, // exception_table_length = 0
                0x00, 0x00, // attributes_count = 0
            ],
        );
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::Code(CodeAttribute {
                max_stack: 1,
                max_locals: 2,
                code: &[0xB1],
                exception_table: vec![],
                attributes: vec![],
            }))
        );
    }

    #[test]
    fn decodes_a_code_attribute_with_exception_table_entries() {
        let pool = pool_with_utf8_name("Code");
        let bytes = attribute_info_bytes(
            1,
            &[
                0x00, 0x02, // max_stack = 2
                0x00, 0x01, // max_locals = 1
                0x00, 0x00, 0x00, 0x00, // code_length = 0
                0x00, 0x02, // exception_table_length = 2
                0x00, 0x00, 0x00, 0x05, 0x00, 0x08, 0x00, 0x03, // caught: catch_type=#3
                0x00, 0x05, 0x00, 0x08, 0x00, 0x0B, 0x00, 0x00, // finally: catch_type=0
                0x00, 0x00, // attributes_count = 0
            ],
        );
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::Code(CodeAttribute {
                max_stack: 2,
                max_locals: 1,
                code: &[],
                exception_table: vec![
                    ExceptionTableEntry {
                        start_pc: 0,
                        end_pc: 5,
                        handler_pc: 8,
                        catch_type: Some(ConstantPoolIndex(3)),
                    },
                    ExceptionTableEntry {
                        start_pc: 5,
                        end_pc: 8,
                        handler_pc: 11,
                        catch_type: None,
                    },
                ],
                attributes: vec![],
            }))
        );
    }

    #[test]
    fn decodes_a_code_attribute_with_an_unrecognized_nested_attribute() {
        let pool = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("Code".to_owned())),
            Some(ConstantPoolEntry::Utf8("LineNumberTable".to_owned())),
        ]);
        let bytes = attribute_info_bytes(
            1,
            &[
                0x00, 0x00, // max_stack = 0
                0x00, 0x00, // max_locals = 0
                0x00, 0x00, 0x00, 0x00, // code_length = 0
                0x00, 0x00, // exception_table_length = 0
                0x00, 0x01, // attributes_count = 1
                0x00, 0x02, // nested attribute_name_index = #2 ("LineNumberTable")
                0x00, 0x00, 0x00, 0x02, // nested attribute_length = 2
                0xCA, 0xFE, // opaque nested payload
            ],
        );
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::Code(CodeAttribute {
                max_stack: 0,
                max_locals: 0,
                code: &[],
                exception_table: vec![],
                attributes: vec![Attribute::Other(RawAttribute {
                    name_index: ConstantPoolIndex(2),
                    bytes: &[0xCA, 0xFE],
                })],
            }))
        );
    }

    #[test]
    fn reports_a_truncated_code_array() {
        let pool = pool_with_utf8_name("Code");
        let bytes = attribute_info_bytes(
            1,
            &[
                0x00, 0x00, // max_stack = 0
                0x00, 0x00, // max_locals = 0
                0x00, 0x00, 0x00, 0x05, // code_length = 5, but only 1 byte follows
                0xB1,
            ],
        );
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Err(AttributeError::Read(ReadError::UnexpectedEof {
                offset: 14,
                needed: 5,
                remaining: 1,
            }))
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
    fn validate_attributes_accepts_an_empty_list() {
        let pool = ConstantPool::from_entries(vec![]);

        assert_eq!(validate_attributes(&[], &pool), Ok(()));
    }

    #[test]
    fn validate_constant_value_accepts_a_matching_kind() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(1))]);
        let attribute = Attribute::ConstantValue(ConstantPoolIndex(1));

        assert_eq!(validate_attribute(&attribute, &pool), Ok(()));
    }

    #[test]
    fn validate_constant_value_rejects_a_bad_kind() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Class {
            name_index: ConstantPoolIndex(1),
        })]);
        let attribute = Attribute::ConstantValue(ConstantPoolIndex(1));

        assert_eq!(
            validate_attribute(&attribute, &pool),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: crate::constant_pool::EntryKind::ConstantValue,
            })
        );
    }

    #[test]
    fn validate_signature_and_source_file_accept_utf8() {
        let pool =
            ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Utf8("LFoo;".to_owned()))]);

        assert_eq!(
            validate_attribute(&Attribute::Signature(ConstantPoolIndex(1)), &pool),
            Ok(())
        );
        assert_eq!(
            validate_attribute(&Attribute::SourceFile(ConstantPoolIndex(1)), &pool),
            Ok(())
        );
    }

    #[test]
    fn validate_source_file_rejects_a_non_utf8_index() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(1))]);

        assert_eq!(
            validate_attribute(&Attribute::SourceFile(ConstantPoolIndex(1)), &pool),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: crate::constant_pool::EntryKind::Utf8,
            })
        );
    }

    fn class_pool_with_one_class() -> ConstantPool {
        ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("java/lang/Exception".to_owned())),
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(1),
            }),
        ])
    }

    #[test]
    fn validate_exceptions_nest_members_and_permitted_subclasses_accept_class_indices() {
        let pool = class_pool_with_one_class();

        assert_eq!(
            validate_attribute(&Attribute::Exceptions(vec![ConstantPoolIndex(2)]), &pool),
            Ok(())
        );
        assert_eq!(
            validate_attribute(&Attribute::NestMembers(vec![ConstantPoolIndex(2)]), &pool),
            Ok(())
        );
        assert_eq!(
            validate_attribute(
                &Attribute::PermittedSubclasses(vec![ConstantPoolIndex(2)]),
                &pool
            ),
            Ok(())
        );
    }

    #[test]
    fn validate_exceptions_rejects_a_non_class_index() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(1))]);

        assert_eq!(
            validate_attribute(&Attribute::Exceptions(vec![ConstantPoolIndex(1)]), &pool),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: crate::constant_pool::EntryKind::Class,
            })
        );
    }

    #[test]
    fn validate_inner_classes_accepts_valid_references() {
        let pool = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("Outer$Inner".to_owned())),
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(1),
            }),
            Some(ConstantPoolEntry::Utf8("Outer".to_owned())),
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(3),
            }),
            Some(ConstantPoolEntry::Utf8("Inner".to_owned())),
        ]);
        let attribute = Attribute::InnerClasses(vec![InnerClassEntry {
            inner_class_info_index: ConstantPoolIndex(2),
            outer_class_info_index: Some(ConstantPoolIndex(4)),
            inner_name_index: Some(ConstantPoolIndex(5)),
            inner_class_access_flags: 0x0009,
        }]);

        assert_eq!(validate_attribute(&attribute, &pool), Ok(()));
    }

    #[test]
    fn validate_inner_classes_rejects_a_bad_outer_class_info_index() {
        let pool = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("Outer$Inner".to_owned())),
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(1),
            }),
            Some(ConstantPoolEntry::Integer(1)),
        ]);
        let attribute = Attribute::InnerClasses(vec![InnerClassEntry {
            inner_class_info_index: ConstantPoolIndex(2),
            outer_class_info_index: Some(ConstantPoolIndex(3)),
            inner_name_index: None,
            inner_class_access_flags: 0,
        }]);

        assert_eq!(
            validate_attribute(&attribute, &pool),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(3),
                expected: crate::constant_pool::EntryKind::Class,
            })
        );
    }

    #[test]
    fn validate_enclosing_method_accepts_a_class_and_name_and_type() {
        let pool = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("Outer".to_owned())),
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(1),
            }),
            Some(ConstantPoolEntry::Utf8("run".to_owned())),
            Some(ConstantPoolEntry::Utf8("()V".to_owned())),
            Some(ConstantPoolEntry::NameAndType {
                name_index: ConstantPoolIndex(3),
                descriptor_index: ConstantPoolIndex(4),
            }),
        ]);
        let attribute = Attribute::EnclosingMethod {
            class_index: ConstantPoolIndex(2),
            method_index: Some(ConstantPoolIndex(5)),
        };

        assert_eq!(validate_attribute(&attribute, &pool), Ok(()));
    }

    #[test]
    fn validate_enclosing_method_rejects_a_method_index_that_is_not_name_and_type() {
        let pool = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("Outer".to_owned())),
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(1),
            }),
            Some(ConstantPoolEntry::Utf8("run".to_owned())),
        ]);
        let attribute = Attribute::EnclosingMethod {
            class_index: ConstantPoolIndex(2),
            method_index: Some(ConstantPoolIndex(3)),
        };

        assert_eq!(
            validate_attribute(&attribute, &pool),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(3),
                expected: crate::constant_pool::EntryKind::NameAndType,
            })
        );
    }

    #[test]
    fn validate_deprecated_and_other_are_always_ok() {
        let pool = ConstantPool::from_entries(vec![]);

        assert_eq!(validate_attribute(&Attribute::Deprecated, &pool), Ok(()));
        assert_eq!(
            validate_attribute(
                &Attribute::Other(RawAttribute {
                    name_index: ConstantPoolIndex(1),
                    bytes: &[0xFF],
                }),
                &pool
            ),
            Ok(())
        );
    }

    #[test]
    fn validate_nest_host_accepts_and_rejects() {
        let good_pool = class_pool_with_one_class();
        assert_eq!(
            validate_attribute(&Attribute::NestHost(ConstantPoolIndex(2)), &good_pool),
            Ok(())
        );

        let bad_pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(1))]);
        assert_eq!(
            validate_attribute(&Attribute::NestHost(ConstantPoolIndex(1)), &bad_pool),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: crate::constant_pool::EntryKind::Class,
            })
        );
    }

    #[test]
    fn validate_record_accepts_valid_components_and_recurses_into_nested_attributes() {
        let pool = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("radius".to_owned())),
            Some(ConstantPoolEntry::Utf8("D".to_owned())),
        ]);
        let attribute = Attribute::Record(vec![RecordComponentInfo {
            name_index: ConstantPoolIndex(1),
            descriptor_index: ConstantPoolIndex(2),
            attributes: vec![Attribute::Signature(ConstantPoolIndex(2))],
        }]);

        assert_eq!(validate_attribute(&attribute, &pool), Ok(()));
    }

    #[test]
    fn validate_record_propagates_a_bad_reference_in_a_nested_attribute() {
        let pool = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("radius".to_owned())),
            Some(ConstantPoolEntry::Utf8("D".to_owned())),
        ]);
        let attribute = Attribute::Record(vec![RecordComponentInfo {
            name_index: ConstantPoolIndex(1),
            descriptor_index: ConstantPoolIndex(2),
            attributes: vec![Attribute::ConstantValue(ConstantPoolIndex(1))],
        }]);

        assert_eq!(
            validate_attribute(&attribute, &pool),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: crate::constant_pool::EntryKind::ConstantValue,
            })
        );
    }

    #[test]
    fn validate_code_accepts_a_valid_catch_type_and_recurses_into_nested_attributes() {
        let pool = class_pool_with_one_class();
        let attribute = Attribute::Code(CodeAttribute {
            max_stack: 1,
            max_locals: 1,
            code: &[],
            exception_table: vec![ExceptionTableEntry {
                start_pc: 0,
                end_pc: 1,
                handler_pc: 2,
                catch_type: Some(ConstantPoolIndex(2)),
            }],
            attributes: vec![],
        });

        assert_eq!(validate_attribute(&attribute, &pool), Ok(()));
    }

    #[test]
    fn validate_code_rejects_a_bad_catch_type() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(1))]);
        let attribute = Attribute::Code(CodeAttribute {
            max_stack: 0,
            max_locals: 0,
            code: &[],
            exception_table: vec![ExceptionTableEntry {
                start_pc: 0,
                end_pc: 1,
                handler_pc: 2,
                catch_type: Some(ConstantPoolIndex(1)),
            }],
            attributes: vec![],
        });

        assert_eq!(
            validate_attribute(&attribute, &pool),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: crate::constant_pool::EntryKind::Class,
            })
        );
    }

    #[test]
    fn validate_code_propagates_a_bad_reference_in_a_nested_attribute() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(1))]);
        let attribute = Attribute::Code(CodeAttribute {
            max_stack: 0,
            max_locals: 0,
            code: &[],
            exception_table: vec![],
            attributes: vec![Attribute::SourceFile(ConstantPoolIndex(1))],
        });

        assert_eq!(
            validate_attribute(&attribute, &pool),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: crate::constant_pool::EntryKind::Utf8,
            })
        );
    }

    #[test]
    fn validate_bootstrap_methods_accepts_a_method_handle_and_loadable_arguments() {
        let pool = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("value".to_owned())),
            Some(method_handle_entry()),
            Some(ConstantPoolEntry::Integer(1)),
        ]);
        let attribute = Attribute::BootstrapMethods(vec![BootstrapMethodEntry {
            method_ref: ConstantPoolIndex(2),
            arguments: vec![ConstantPoolIndex(3)],
        }]);

        assert_eq!(validate_attribute(&attribute, &pool), Ok(()));
    }

    #[test]
    fn validate_bootstrap_methods_rejects_a_method_ref_that_is_not_a_method_handle() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(1))]);
        let attribute = Attribute::BootstrapMethods(vec![BootstrapMethodEntry {
            method_ref: ConstantPoolIndex(1),
            arguments: vec![],
        }]);

        assert_eq!(
            validate_attribute(&attribute, &pool),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: crate::constant_pool::EntryKind::MethodHandle,
            })
        );
    }

    #[test]
    fn validate_bootstrap_methods_rejects_a_non_loadable_argument() {
        let pool = ConstantPool::from_entries(vec![
            Some(method_handle_entry()),
            Some(ConstantPoolEntry::NameAndType {
                name_index: ConstantPoolIndex(1),
                descriptor_index: ConstantPoolIndex(1),
            }),
        ]);
        let attribute = Attribute::BootstrapMethods(vec![BootstrapMethodEntry {
            method_ref: ConstantPoolIndex(1),
            arguments: vec![ConstantPoolIndex(2)],
        }]);

        assert_eq!(
            validate_attribute(&attribute, &pool),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(2),
                expected: crate::constant_pool::EntryKind::Loadable,
            })
        );
    }

    fn method_handle_entry() -> ConstantPoolEntry {
        ConstantPoolEntry::MethodHandle {
            reference_kind: crate::constant_pool::MethodHandleKind::InvokeStatic,
            reference_index: ConstantPoolIndex(1),
        }
    }

    #[test]
    fn validate_method_parameters_accepts_a_named_and_an_unnamed_parameter() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Utf8("a".to_owned()))]);
        let attribute = Attribute::MethodParameters(vec![
            MethodParameterEntry {
                name_index: Some(ConstantPoolIndex(1)),
                access_flags: 0,
            },
            MethodParameterEntry {
                name_index: None,
                access_flags: 0,
            },
        ]);

        assert_eq!(validate_attribute(&attribute, &pool), Ok(()));
    }

    #[test]
    fn validate_method_parameters_rejects_a_non_utf8_name_index() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(1))]);
        let attribute = Attribute::MethodParameters(vec![MethodParameterEntry {
            name_index: Some(ConstantPoolIndex(1)),
            access_flags: 0,
        }]);

        assert_eq!(
            validate_attribute(&attribute, &pool),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: crate::constant_pool::EntryKind::Utf8,
            })
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
