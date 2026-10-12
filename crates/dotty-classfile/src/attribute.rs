use crate::constant_pool::{
    ConstantPool, ConstantPoolEntry, ConstantPoolIndex, PoolRefError, read_index,
};
use crate::encode_error::EncodeError;
use crate::reader::{ReadError, Reader};
use crate::writer::{Writer, checked_u8_len, checked_u16_len, checked_u32_len};
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

/// An `annotation` structure (JVMS §4.7.16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Annotation {
    /// A `Utf8` entry holding the annotation interface's field descriptor
    /// (e.g. `Ljava/lang/Deprecated;`).
    pub type_index: ConstantPoolIndex,
    pub element_value_pairs: Vec<(ConstantPoolIndex, ElementValue)>,
}

/// An `element_value` structure (JVMS §4.7.16.1). One variant per `tag`
/// byte, rather than collapsing the primitive/string tags into one shape,
/// since each tag mandates a distinct constant pool entry kind — most
/// notably `String`/`Class`, which point at a `Utf8` entry directly rather
/// than a `CONSTANT_String`/`CONSTANT_Class` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElementValue {
    Byte(ConstantPoolIndex),
    Char(ConstantPoolIndex),
    Double(ConstantPoolIndex),
    Float(ConstantPoolIndex),
    Int(ConstantPoolIndex),
    Long(ConstantPoolIndex),
    Short(ConstantPoolIndex),
    Boolean(ConstantPoolIndex),
    /// Points at a `Utf8` entry directly (JVMS §4.7.16.1), not a
    /// `CONSTANT_String`.
    String(ConstantPoolIndex),
    Enum {
        type_name_index: ConstantPoolIndex,
        const_name_index: ConstantPoolIndex,
    },
    /// Points at a `Utf8` entry holding a return descriptor (JVMS
    /// §4.7.16.1), not a `CONSTANT_Class`.
    Class(ConstantPoolIndex),
    Annotation(Box<Annotation>),
    Array(Vec<ElementValue>),
}

/// Bounds recursive `element_value` decode (via the `annotation` and
/// `array` tags) against adversarial nesting — implementation-defined,
/// like `signature::DEFAULT_MAX_SIGNATURE_DEPTH`.
pub const DEFAULT_MAX_ANNOTATION_DEPTH: usize = 128;

/// A structured attribute, resolved one level above [`RawAttribute`].
///
/// Covers the attributes needed for symbol-level semantic analysis
/// (`docs/classfile-format-jdk25.md` §7). `Code`, `Record`, and runtime
/// annotation payloads are represented structurally; unmodeled nested
/// attributes such as `StackMapTable` and `LineNumberTable` fall through to
/// [`Attribute::Other`] so their bytes can be preserved.
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
    Synthetic,
    NestHost(ConstantPoolIndex),
    NestMembers(Vec<ConstantPoolIndex>),
    Record(Vec<RecordComponentInfo<'a>>),
    PermittedSubclasses(Vec<ConstantPoolIndex>),
    Code(CodeAttribute<'a>),
    BootstrapMethods(Vec<BootstrapMethodEntry>),
    MethodParameters(Vec<MethodParameterEntry>),
    RuntimeVisibleAnnotations(Vec<Annotation>),
    RuntimeInvisibleAnnotations(Vec<Annotation>),
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
    /// An `element_value`'s `tag` byte (JVMS §4.7.16.1) wasn't one of the
    /// twelve recognized tags.
    UnknownElementValueTag {
        tag: u8,
    },
    /// An `element_value` nested (via `annotation`/`array`) past
    /// [`DEFAULT_MAX_ANNOTATION_DEPTH`].
    TooDeeplyNestedAnnotation,
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
            Self::UnknownElementValueTag { tag } => {
                write!(formatter, "unknown element_value tag {tag:#04x}")
            }
            Self::TooDeeplyNestedAnnotation => write!(
                formatter,
                "annotation element_value nested past depth {DEFAULT_MAX_ANNOTATION_DEPTH}"
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

fn read_element_value(
    reader: &mut Reader<'_>,
    depth: usize,
) -> Result<ElementValue, AttributeError> {
    let tag = reader.read_u8()?;
    match tag {
        b'B' => Ok(ElementValue::Byte(read_index(reader)?)),
        b'C' => Ok(ElementValue::Char(read_index(reader)?)),
        b'D' => Ok(ElementValue::Double(read_index(reader)?)),
        b'F' => Ok(ElementValue::Float(read_index(reader)?)),
        b'I' => Ok(ElementValue::Int(read_index(reader)?)),
        b'J' => Ok(ElementValue::Long(read_index(reader)?)),
        b'S' => Ok(ElementValue::Short(read_index(reader)?)),
        b'Z' => Ok(ElementValue::Boolean(read_index(reader)?)),
        b's' => Ok(ElementValue::String(read_index(reader)?)),
        b'e' => Ok(ElementValue::Enum {
            type_name_index: read_index(reader)?,
            const_name_index: read_index(reader)?,
        }),
        b'c' => Ok(ElementValue::Class(read_index(reader)?)),
        b'@' => {
            if depth >= DEFAULT_MAX_ANNOTATION_DEPTH {
                return Err(AttributeError::TooDeeplyNestedAnnotation);
            }
            Ok(ElementValue::Annotation(Box::new(read_annotation(
                reader,
                depth + 1,
            )?)))
        }
        b'[' => {
            if depth >= DEFAULT_MAX_ANNOTATION_DEPTH {
                return Err(AttributeError::TooDeeplyNestedAnnotation);
            }
            let count = reader.read_u16()?;
            let mut values = Vec::with_capacity(usize::from(count));
            for _ in 0..count {
                values.push(read_element_value(reader, depth + 1)?);
            }
            Ok(ElementValue::Array(values))
        }
        _ => Err(AttributeError::UnknownElementValueTag { tag }),
    }
}

fn read_annotation(reader: &mut Reader<'_>, depth: usize) -> Result<Annotation, AttributeError> {
    let type_index = read_index(reader)?;
    let count = reader.read_u16()?;
    let mut element_value_pairs = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let element_name_index = read_index(reader)?;
        let value = read_element_value(reader, depth)?;
        element_value_pairs.push((element_name_index, value));
    }
    Ok(Annotation {
        type_index,
        element_value_pairs,
    })
}

fn read_annotations(reader: &mut Reader<'_>) -> Result<Vec<Annotation>, AttributeError> {
    let count = reader.read_u16()?;
    let mut annotations = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        annotations.push(read_annotation(reader, 0)?);
    }
    Ok(annotations)
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
        Attribute::Deprecated | Attribute::Synthetic => Ok(()),
        Attribute::RuntimeVisibleAnnotations(annotations)
        | Attribute::RuntimeInvisibleAnnotations(annotations) => {
            for annotation in annotations {
                validate_annotation(annotation, pool)?;
            }
            Ok(())
        }
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

/// Validates an `annotation`'s constant pool references (JVMS §4.7.16):
/// `type_index`, each `element_name_index`, and each paired `element_value`.
fn validate_annotation(annotation: &Annotation, pool: &ConstantPool) -> Result<(), PoolRefError> {
    pool.utf8(annotation.type_index)?;
    for (element_name_index, value) in &annotation.element_value_pairs {
        pool.utf8(*element_name_index)?;
        validate_element_value(value, pool)?;
    }
    Ok(())
}

/// Validates an `element_value`'s constant pool reference(s) against the
/// exact kind its `tag` mandates (JVMS §4.7.16.1) — notably `String`/`Class`
/// against `Utf8`, not `CONSTANT_String`/`CONSTANT_Class`.
fn validate_element_value(value: &ElementValue, pool: &ConstantPool) -> Result<(), PoolRefError> {
    match value {
        ElementValue::Byte(index)
        | ElementValue::Char(index)
        | ElementValue::Int(index)
        | ElementValue::Short(index)
        | ElementValue::Boolean(index) => pool.check_integer(*index),
        ElementValue::Long(index) => pool.check_long(*index),
        ElementValue::Float(index) => pool.check_float(*index),
        ElementValue::Double(index) => pool.check_double(*index),
        ElementValue::String(index) | ElementValue::Class(index) => pool.utf8(*index).map(|_| ()),
        ElementValue::Enum {
            type_name_index,
            const_name_index,
        } => {
            pool.utf8(*type_name_index)?;
            pool.utf8(*const_name_index)?;
            Ok(())
        }
        ElementValue::Annotation(annotation) => validate_annotation(annotation, pool),
        ElementValue::Array(values) => {
            for value in values {
                validate_element_value(value, pool)?;
            }
            Ok(())
        }
    }
}

fn encode_pool_ref_error(pool: &ConstantPool, error: PoolRefError) -> EncodeError {
    match error {
        PoolRefError::InvalidIndex { index } => EncodeError::InvalidIndex {
            index: usize::from(index.0),
            count: pool.entry_count(),
        },
        PoolRefError::WrongKind { index, .. } => EncodeError::InvalidReference {
            index: usize::from(index.0),
        },
    }
}

fn attribute_name(attribute: &Attribute<'_>) -> Option<&'static str> {
    match attribute {
        Attribute::ConstantValue(_) => Some("ConstantValue"),
        Attribute::Signature(_) => Some("Signature"),
        Attribute::Exceptions(_) => Some("Exceptions"),
        Attribute::InnerClasses(_) => Some("InnerClasses"),
        Attribute::EnclosingMethod { .. } => Some("EnclosingMethod"),
        Attribute::SourceFile(_) => Some("SourceFile"),
        Attribute::Deprecated => Some("Deprecated"),
        Attribute::Synthetic => Some("Synthetic"),
        Attribute::NestHost(_) => Some("NestHost"),
        Attribute::NestMembers(_) => Some("NestMembers"),
        Attribute::PermittedSubclasses(_) => Some("PermittedSubclasses"),
        Attribute::BootstrapMethods(_) => Some("BootstrapMethods"),
        Attribute::MethodParameters(_) => Some("MethodParameters"),
        Attribute::Record(_) => Some("Record"),
        Attribute::Code(_) => Some("Code"),
        Attribute::RuntimeVisibleAnnotations(_) => Some("RuntimeVisibleAnnotations"),
        Attribute::RuntimeInvisibleAnnotations(_) => Some("RuntimeInvisibleAnnotations"),
        Attribute::Other(_) => None,
    }
}

fn write_index_list(writer: &mut Writer, indices: &[ConstantPoolIndex]) -> Result<(), EncodeError> {
    writer.write_u2_length(indices.len())?;
    for index in indices {
        writer.write_u2(index.0);
    }
    Ok(())
}

fn write_annotation(
    writer: &mut Writer,
    annotation: &Annotation,
    depth: usize,
) -> Result<(), EncodeError> {
    writer.write_u2(annotation.type_index.0);
    writer.write_u2_length(annotation.element_value_pairs.len())?;
    for (name_index, value) in &annotation.element_value_pairs {
        writer.write_u2(name_index.0);
        write_element_value(writer, value, depth)?;
    }
    Ok(())
}

fn write_element_value(
    writer: &mut Writer,
    value: &ElementValue,
    depth: usize,
) -> Result<(), EncodeError> {
    match value {
        ElementValue::Byte(index) => write_tagged_index(writer, b'B', *index),
        ElementValue::Char(index) => write_tagged_index(writer, b'C', *index),
        ElementValue::Double(index) => write_tagged_index(writer, b'D', *index),
        ElementValue::Float(index) => write_tagged_index(writer, b'F', *index),
        ElementValue::Int(index) => write_tagged_index(writer, b'I', *index),
        ElementValue::Long(index) => write_tagged_index(writer, b'J', *index),
        ElementValue::Short(index) => write_tagged_index(writer, b'S', *index),
        ElementValue::Boolean(index) => write_tagged_index(writer, b'Z', *index),
        ElementValue::String(index) => write_tagged_index(writer, b's', *index),
        ElementValue::Class(index) => write_tagged_index(writer, b'c', *index),
        ElementValue::Enum {
            type_name_index,
            const_name_index,
        } => {
            writer.write_u1(b'e');
            writer.write_u2(type_name_index.0);
            writer.write_u2(const_name_index.0);
            Ok(())
        }
        ElementValue::Annotation(annotation) => {
            if depth >= DEFAULT_MAX_ANNOTATION_DEPTH {
                return Err(EncodeError::TooDeeplyNestedAnnotation);
            }
            writer.write_u1(b'@');
            write_annotation(writer, annotation, depth + 1)
        }
        ElementValue::Array(values) => {
            if depth >= DEFAULT_MAX_ANNOTATION_DEPTH {
                return Err(EncodeError::TooDeeplyNestedAnnotation);
            }
            writer.write_u1(b'[');
            writer.write_u2_length(values.len())?;
            for value in values {
                write_element_value(writer, value, depth + 1)?;
            }
            Ok(())
        }
    }
}

fn write_tagged_index(
    writer: &mut Writer,
    tag: u8,
    index: ConstantPoolIndex,
) -> Result<(), EncodeError> {
    writer.write_u1(tag);
    writer.write_u2(index.0);
    Ok(())
}

fn encode_attribute_info(
    name_index: ConstantPoolIndex,
    payload_length: usize,
    write_payload: impl FnOnce(&mut Writer) -> Result<(), EncodeError>,
) -> Result<Vec<u8>, EncodeError> {
    let payload_length = checked_u32_len(payload_length)?;
    let mut writer = Writer::new();
    writer.write_u2(name_index.0);
    writer.write_u4(payload_length);
    write_payload(&mut writer)?;
    Ok(writer.into_bytes())
}

fn add_payload_length(length: usize, additional: usize) -> Result<usize, EncodeError> {
    length
        .checked_add(additional)
        .ok_or(EncodeError::CountOverflow {
            value: usize::MAX,
            width: crate::encode_error::LengthWidth::U32,
        })
}

fn repeated_payload_length(count: usize, element_width: usize) -> Result<usize, EncodeError> {
    count
        .checked_mul(element_width)
        .ok_or(EncodeError::CountOverflow {
            value: usize::MAX,
            width: crate::encode_error::LengthWidth::U32,
        })
}

fn bootstrap_methods_payload_length(
    method_count: usize,
    argument_counts: impl IntoIterator<Item = usize>,
) -> Result<usize, EncodeError> {
    checked_u16_len(method_count)?;
    let mut length = 2;
    for argument_count in argument_counts {
        checked_u16_len(argument_count)?;
        let entry_length = add_payload_length(4, repeated_payload_length(argument_count, 2)?)?;
        length = add_payload_length(length, entry_length)?;
    }
    checked_u32_len(length)?;
    Ok(length)
}

fn attribute_list_payload_length(attributes: &[Attribute<'_>]) -> Result<usize, EncodeError> {
    checked_u16_len(attributes.len())?;
    let mut length = 2;
    for attribute in attributes {
        length = add_payload_length(length, 6)?;
        length = add_payload_length(length, encoded_payload_length(attribute)?)?;
    }
    checked_u32_len(length)?;
    Ok(length)
}

fn code_payload_length(code: &CodeAttribute<'_>) -> Result<usize, EncodeError> {
    if !(1..=usize::from(u16::MAX)).contains(&code.code.len()) {
        return Err(EncodeError::InvalidCodeLength {
            length: code.code.len(),
        });
    }
    checked_u16_len(code.exception_table.len())?;
    for (entry_index, entry) in code.exception_table.iter().enumerate() {
        let code_length = code.code.len() as u32;
        if u32::from(entry.start_pc) >= u32::from(entry.end_pc)
            || u32::from(entry.end_pc) > code_length
            || u32::from(entry.handler_pc) >= code_length
        {
            return Err(EncodeError::InvalidExceptionTableRange { entry_index });
        }
    }
    let mut length = 10;
    length = add_payload_length(length, code.code.len())?;
    length = add_payload_length(
        length,
        repeated_payload_length(code.exception_table.len(), 8)?,
    )?;
    length = add_payload_length(length, attribute_list_payload_length(&code.attributes)?)?;
    checked_u32_len(length)?;
    Ok(length)
}

fn record_payload_length(components: &[RecordComponentInfo<'_>]) -> Result<usize, EncodeError> {
    checked_u16_len(components.len())?;
    let mut length = 2;
    for component in components {
        length = add_payload_length(length, 4)?;
        length = add_payload_length(
            length,
            attribute_list_payload_length(&component.attributes)?,
        )?;
    }
    checked_u32_len(length)?;
    Ok(length)
}

fn annotation_payload_length(annotations: &[Annotation]) -> Result<usize, EncodeError> {
    checked_u16_len(annotations.len())?;
    let mut length = 2;
    for annotation in annotations {
        length = add_payload_length(length, annotation_length(annotation, 0)?)?;
    }
    checked_u32_len(length)?;
    Ok(length)
}

fn annotation_length(annotation: &Annotation, depth: usize) -> Result<usize, EncodeError> {
    checked_u16_len(annotation.element_value_pairs.len())?;
    let mut length = 4;
    for (_, value) in &annotation.element_value_pairs {
        length = add_payload_length(length, 2)?;
        length = add_payload_length(length, element_value_length(value, depth)?)?;
    }
    Ok(length)
}

fn element_value_length(value: &ElementValue, depth: usize) -> Result<usize, EncodeError> {
    match value {
        ElementValue::Byte(_)
        | ElementValue::Char(_)
        | ElementValue::Double(_)
        | ElementValue::Float(_)
        | ElementValue::Int(_)
        | ElementValue::Long(_)
        | ElementValue::Short(_)
        | ElementValue::Boolean(_)
        | ElementValue::String(_)
        | ElementValue::Class(_) => Ok(3),
        ElementValue::Enum { .. } => Ok(5),
        ElementValue::Annotation(annotation) => {
            if depth >= DEFAULT_MAX_ANNOTATION_DEPTH {
                return Err(EncodeError::TooDeeplyNestedAnnotation);
            }
            add_payload_length(1, annotation_length(annotation, depth + 1)?)
        }
        ElementValue::Array(values) => {
            if depth >= DEFAULT_MAX_ANNOTATION_DEPTH {
                return Err(EncodeError::TooDeeplyNestedAnnotation);
            }
            checked_u16_len(values.len())?;
            let mut length = 3;
            for value in values {
                length = add_payload_length(length, element_value_length(value, depth + 1)?)?;
            }
            Ok(length)
        }
    }
}

fn encoded_payload_length(attribute: &Attribute<'_>) -> Result<usize, EncodeError> {
    let length = match attribute {
        Attribute::ConstantValue(_)
        | Attribute::Signature(_)
        | Attribute::SourceFile(_)
        | Attribute::NestHost(_) => 2,
        Attribute::Deprecated | Attribute::Synthetic => 0,
        Attribute::Exceptions(indices)
        | Attribute::NestMembers(indices)
        | Attribute::PermittedSubclasses(indices) => {
            checked_u16_len(indices.len())?;
            add_payload_length(2, repeated_payload_length(indices.len(), 2)?)?
        }
        Attribute::InnerClasses(entries) => {
            checked_u16_len(entries.len())?;
            add_payload_length(2, repeated_payload_length(entries.len(), 8)?)?
        }
        Attribute::EnclosingMethod { .. } => 4,
        Attribute::BootstrapMethods(methods) => bootstrap_methods_payload_length(
            methods.len(),
            methods.iter().map(|method| method.arguments.len()),
        )?,
        Attribute::MethodParameters(parameters) => {
            checked_u8_len(parameters.len())?;
            add_payload_length(1, repeated_payload_length(parameters.len(), 4)?)?
        }
        Attribute::Other(raw) => raw.bytes.len(),
        Attribute::Record(components) => record_payload_length(components)?,
        Attribute::Code(code) => code_payload_length(code)?,
        Attribute::RuntimeVisibleAnnotations(annotations)
        | Attribute::RuntimeInvisibleAnnotations(annotations) => {
            annotation_payload_length(annotations)?
        }
    };
    checked_u32_len(length)?;
    Ok(length)
}

/// Encodes a `u2` count followed by that many `attribute_info` structures.
///
/// Attribute payloads are structurally encoded. Known attributes use the
/// first matching UTF-8 name entry in `constant_pool`, so their bytes may use
/// a different duplicate name index than the original input. Opaque
/// [`Attribute::Other`] values retain their original name index and payload
/// bytes exactly. The pool is never modified or reindexed.
pub fn encode_attributes(
    attributes: &[Attribute<'_>],
    constant_pool: &ConstantPool,
) -> Result<Vec<u8>, EncodeError> {
    let mut writer = Writer::new();
    writer.write_u2_length(attributes.len())?;
    for attribute in attributes {
        writer.write_bytes(&attribute.encode(constant_pool)?);
    }
    Ok(writer.into_bytes())
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

/// The information carried by a `@java.lang.Deprecated` annotation (JVMS
/// §9.6.4.6), which the legacy `Deprecated` attribute can't express.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeprecationInfo {
    pub since: Option<String>,
    pub for_removal: bool,
}

/// Looks for a `java.lang.Deprecated` annotation among `attributes`'s
/// `RuntimeVisibleAnnotations`/`RuntimeInvisibleAnnotations` and, if found,
/// reads its `since`/`forRemoval` elements. A best-effort lookup rather than
/// a decoder: an element that doesn't resolve or has an unexpected shape is
/// skipped rather than erroring, since a malformed `@Deprecated` shouldn't
/// fail the whole class load.
pub fn find_deprecated_annotation(
    attributes: &[Attribute<'_>],
    pool: &ConstantPool,
) -> Option<DeprecationInfo> {
    for attribute in attributes {
        let annotations = match attribute {
            Attribute::RuntimeVisibleAnnotations(annotations)
            | Attribute::RuntimeInvisibleAnnotations(annotations) => annotations,
            _ => continue,
        };
        for annotation in annotations {
            if pool.utf8(annotation.type_index) != Ok("Ljava/lang/Deprecated;") {
                continue;
            }

            let mut info = DeprecationInfo::default();
            for (name_index, value) in &annotation.element_value_pairs {
                let Ok(name) = pool.utf8(*name_index) else {
                    continue;
                };
                match (name, value) {
                    ("since", ElementValue::String(index)) => {
                        info.since = pool.utf8(*index).ok().map(str::to_owned);
                    }
                    ("forRemoval", ElementValue::Boolean(index)) => {
                        info.for_removal =
                            matches!(pool.get(*index), Some(ConstantPoolEntry::Integer(1)));
                    }
                    _ => {}
                }
            }
            return Some(info);
        }
    }
    None
}

impl<'a> Attribute<'a> {
    /// Encodes one `attribute_info` (JVMS §4.7.1).
    ///
    /// Known variants use the first matching UTF-8 name entry in the supplied
    /// pool. This is a structural round trip: duplicate name entries may
    /// produce different bytes while decoding to the same model. An opaque
    /// [`Attribute::Other`] preserves its original name index and payload.
    /// Code and nested attribute payloads remain opaque where represented by
    /// [`Attribute::Other`]; annotation structures are encoded recursively
    /// with [`DEFAULT_MAX_ANNOTATION_DEPTH`].
    pub fn encode(&self, constant_pool: &ConstantPool) -> Result<Vec<u8>, EncodeError> {
        let payload_length = encoded_payload_length(self)?;
        let name_index = if let Some(name) = attribute_name(self) {
            validate_attribute(self, constant_pool)
                .map_err(|error| encode_pool_ref_error(constant_pool, error))?;
            constant_pool
                .find_utf8(name)
                .ok_or(EncodeError::MissingAttributeName { name })?
        } else if let Attribute::Other(raw) = self {
            constant_pool
                .utf8(raw.name_index)
                .map_err(|error| encode_pool_ref_error(constant_pool, error))?;
            raw.name_index
        } else {
            return Err(EncodeError::UnsupportedAttribute);
        };

        encode_attribute_info(name_index, payload_length, |writer| {
            match self {
                Attribute::ConstantValue(index)
                | Attribute::Signature(index)
                | Attribute::SourceFile(index)
                | Attribute::NestHost(index) => writer.write_u2(index.0),
                Attribute::Deprecated | Attribute::Synthetic => {}
                Attribute::Exceptions(indices)
                | Attribute::NestMembers(indices)
                | Attribute::PermittedSubclasses(indices) => {
                    write_index_list(writer, indices)?;
                }
                Attribute::InnerClasses(entries) => {
                    writer.write_u2_length(entries.len())?;
                    for entry in entries {
                        writer.write_u2(entry.inner_class_info_index.0);
                        writer.write_u2(entry.outer_class_info_index.map_or(0, |index| index.0));
                        writer.write_u2(entry.inner_name_index.map_or(0, |index| index.0));
                        writer.write_u2(entry.inner_class_access_flags);
                    }
                }
                Attribute::EnclosingMethod {
                    class_index,
                    method_index,
                } => {
                    writer.write_u2(class_index.0);
                    writer.write_u2(method_index.map_or(0, |index| index.0));
                }
                Attribute::BootstrapMethods(methods) => {
                    writer.write_u2_length(methods.len())?;
                    for method in methods {
                        writer.write_u2(method.method_ref.0);
                        write_index_list(writer, &method.arguments)?;
                    }
                }
                Attribute::MethodParameters(parameters) => {
                    writer.write_u1(checked_u8_len(parameters.len())?);
                    for parameter in parameters {
                        writer.write_u2(parameter.name_index.map_or(0, |index| index.0));
                        writer.write_u2(parameter.access_flags);
                    }
                }
                Attribute::Record(components) => {
                    writer.write_u2_length(components.len())?;
                    for component in components {
                        writer.write_u2(component.name_index.0);
                        writer.write_u2(component.descriptor_index.0);
                        writer
                            .write_bytes(&encode_attributes(&component.attributes, constant_pool)?);
                    }
                }
                Attribute::Code(code) => {
                    writer.write_u2(code.max_stack);
                    writer.write_u2(code.max_locals);
                    writer.write_u4_length(code.code.len())?;
                    writer.write_bytes(code.code);
                    writer.write_u2_length(code.exception_table.len())?;
                    for entry in &code.exception_table {
                        writer.write_u2(entry.start_pc);
                        writer.write_u2(entry.end_pc);
                        writer.write_u2(entry.handler_pc);
                        writer.write_u2(entry.catch_type.map_or(0, |index| index.0));
                    }
                    writer.write_bytes(&encode_attributes(&code.attributes, constant_pool)?);
                }
                Attribute::RuntimeVisibleAnnotations(annotations)
                | Attribute::RuntimeInvisibleAnnotations(annotations) => {
                    writer.write_u2_length(annotations.len())?;
                    for annotation in annotations {
                        write_annotation(writer, annotation, 0)?;
                    }
                }
                Attribute::Other(raw) => writer.write_bytes(raw.bytes),
            }
            Ok(())
        })
    }

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
            "Synthetic" => Attribute::Synthetic,
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
            "RuntimeVisibleAnnotations" => {
                Attribute::RuntimeVisibleAnnotations(read_annotations(&mut sub_reader)?)
            }
            "RuntimeInvisibleAnnotations" => {
                Attribute::RuntimeInvisibleAnnotations(read_annotations(&mut sub_reader)?)
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

    fn attribute_encoding_pool() -> ConstantPool {
        ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("ConstantValue".to_owned())), // #1
            Some(ConstantPoolEntry::Utf8("Signature".to_owned())),     // #2, first duplicate
            Some(ConstantPoolEntry::Utf8("Signature".to_owned())),     // #3
            Some(ConstantPoolEntry::Utf8("Exceptions".to_owned())),    // #4
            Some(ConstantPoolEntry::Utf8("InnerClasses".to_owned())),  // #5
            Some(ConstantPoolEntry::Utf8("EnclosingMethod".to_owned())), // #6
            Some(ConstantPoolEntry::Utf8("SourceFile".to_owned())),    // #7
            Some(ConstantPoolEntry::Utf8("Deprecated".to_owned())),    // #8
            Some(ConstantPoolEntry::Utf8("Synthetic".to_owned())),     // #9
            Some(ConstantPoolEntry::Utf8("NestHost".to_owned())),      // #10
            Some(ConstantPoolEntry::Utf8("NestMembers".to_owned())),   // #11
            Some(ConstantPoolEntry::Utf8("PermittedSubclasses".to_owned())), // #12
            Some(ConstantPoolEntry::Utf8("BootstrapMethods".to_owned())), // #13
            Some(ConstantPoolEntry::Utf8("MethodParameters".to_owned())), // #14
            Some(ConstantPoolEntry::Utf8("StackMapTable".to_owned())), // #15
            Some(ConstantPoolEntry::Utf8("StackMapTable".to_owned())), // #16
            Some(ConstantPoolEntry::Integer(7)),                       // #17
            Some(ConstantPoolEntry::Utf8("Lsig;".to_owned())),         // #18
            Some(ConstantPoolEntry::Utf8("Demo.java".to_owned())),     // #19
            Some(ConstantPoolEntry::Utf8("C".to_owned())),             // #20
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(20),
            }), // #21
            Some(ConstantPoolEntry::Utf8("Outer".to_owned())),         // #22
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(22),
            }), // #23
            Some(ConstantPoolEntry::Utf8("Inner".to_owned())),         // #24
            Some(ConstantPoolEntry::Utf8("run".to_owned())),           // #25
            Some(ConstantPoolEntry::Utf8("()V".to_owned())),           // #26
            Some(ConstantPoolEntry::NameAndType {
                name_index: ConstantPoolIndex(25),
                descriptor_index: ConstantPoolIndex(26),
            }), // #27
            Some(ConstantPoolEntry::Methodref {
                class_index: ConstantPoolIndex(21),
                name_and_type_index: ConstantPoolIndex(27),
            }), // #28
            Some(ConstantPoolEntry::MethodHandle {
                reference_kind: crate::constant_pool::MethodHandleKind::InvokeStatic,
                reference_index: ConstantPoolIndex(28),
            }), // #29
            Some(ConstantPoolEntry::Long(9)),                          // #30
            None,                                                      // #31, reserved long slot
            Some(ConstantPoolEntry::Float(1.5)),                       // #32
            Some(ConstantPoolEntry::Double(2.5)),                      // #33
        ])
    }

    #[test]
    fn encodes_and_decodes_every_supported_attribute_variant() {
        let pool = attribute_encoding_pool();
        let attributes = vec![
            Attribute::ConstantValue(ConstantPoolIndex(17)),
            Attribute::Signature(ConstantPoolIndex(18)),
            Attribute::Exceptions(vec![ConstantPoolIndex(21), ConstantPoolIndex(23)]),
            Attribute::InnerClasses(vec![
                InnerClassEntry {
                    inner_class_info_index: ConstantPoolIndex(21),
                    outer_class_info_index: Some(ConstantPoolIndex(23)),
                    inner_name_index: Some(ConstantPoolIndex(24)),
                    inner_class_access_flags: 0x0019,
                },
                InnerClassEntry {
                    inner_class_info_index: ConstantPoolIndex(23),
                    outer_class_info_index: None,
                    inner_name_index: None,
                    inner_class_access_flags: 0x0001,
                },
            ]),
            Attribute::EnclosingMethod {
                class_index: ConstantPoolIndex(21),
                method_index: Some(ConstantPoolIndex(27)),
            },
            Attribute::EnclosingMethod {
                class_index: ConstantPoolIndex(23),
                method_index: None,
            },
            Attribute::SourceFile(ConstantPoolIndex(19)),
            Attribute::Deprecated,
            Attribute::Synthetic,
            Attribute::NestHost(ConstantPoolIndex(23)),
            Attribute::NestMembers(vec![]),
            Attribute::PermittedSubclasses(vec![ConstantPoolIndex(21), ConstantPoolIndex(23)]),
            Attribute::BootstrapMethods(vec![
                BootstrapMethodEntry {
                    method_ref: ConstantPoolIndex(29),
                    arguments: vec![ConstantPoolIndex(17), ConstantPoolIndex(21)],
                },
                BootstrapMethodEntry {
                    method_ref: ConstantPoolIndex(29),
                    arguments: vec![],
                },
            ]),
            Attribute::BootstrapMethods(vec![]),
            Attribute::MethodParameters(vec![
                MethodParameterEntry {
                    name_index: Some(ConstantPoolIndex(25)),
                    access_flags: 0x0010,
                },
                MethodParameterEntry {
                    name_index: None,
                    access_flags: 0x8000,
                },
            ]),
            Attribute::Other(RawAttribute {
                name_index: ConstantPoolIndex(16),
                bytes: &[0xCA, 0xFE, 0x00, 0xFF],
            }),
        ];
        let original_pool = pool.clone();

        for attribute in &attributes {
            let bytes = attribute.encode(&pool).unwrap();
            let mut reader = Reader::new(&bytes);
            assert_eq!(Attribute::decode(&mut reader, &pool), Ok(attribute.clone()));
            assert!(reader.is_at_end());
        }

        let encoded = encode_attributes(&attributes, &pool).unwrap();
        assert_eq!(encoded, encode_attributes(&attributes, &pool).unwrap());
        let mut reader = Reader::new(&encoded);
        assert_eq!(decode_attributes(&mut reader, &pool).unwrap(), attributes);
        assert!(reader.is_at_end());
        assert_eq!(pool, original_pool);

        // The structured Signature model has no name index, so encoding uses
        // the first duplicate Utf8 name in the original constant pool.
        assert_eq!(attributes[1].encode(&pool).unwrap()[..2], [0, 2]);
        // Opaque attributes retain their original name index and payload.
        assert_eq!(
            attributes.last().unwrap().encode(&pool).unwrap(),
            [0, 16, 0, 0, 0, 4, 0xCA, 0xFE, 0, 0xFF]
        );
    }

    #[test]
    fn encode_attributes_writes_a_checked_u2_count() {
        let pool = pool_with_utf8_name("Deprecated");
        assert_eq!(encode_attributes(&[], &pool).unwrap(), [0, 0]);

        let attributes = vec![Attribute::Deprecated; usize::from(u16::MAX) + 1];
        assert_eq!(
            encode_attributes(&attributes, &pool),
            Err(EncodeError::CountOverflow {
                value: usize::from(u16::MAX) + 1,
                width: crate::encode_error::LengthWidth::U16,
            })
        );
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn bootstrap_methods_preflight_aggregate_payload_length_without_allocating() {
        let method_count = 32_768;
        let argument_count = usize::from(u16::MAX);
        let expected_length = 2 + method_count * (4 + argument_count * 2);

        assert_eq!(
            bootstrap_methods_payload_length(
                method_count,
                std::iter::repeat_n(argument_count, method_count),
            ),
            Err(EncodeError::CountOverflow {
                value: expected_length,
                width: crate::encode_error::LengthWidth::U32,
            })
        );
    }

    #[test]
    fn encode_checks_names_references_and_nested_lengths() {
        assert_eq!(
            Attribute::Signature(ConstantPoolIndex(1)).encode(&ConstantPool::from_entries(vec![
                Some(ConstantPoolEntry::Utf8("Lsig;".to_owned())),
            ])),
            Err(EncodeError::MissingAttributeName { name: "Signature" })
        );

        let wrong_other_name = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("StackMapTable".to_owned())),
            Some(ConstantPoolEntry::Integer(1)),
        ]);
        assert_eq!(
            Attribute::Other(RawAttribute {
                name_index: ConstantPoolIndex(2),
                bytes: &[],
            })
            .encode(&wrong_other_name),
            Err(EncodeError::InvalidReference { index: 2 })
        );

        let bad_reference_pool = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("ConstantValue".to_owned())),
            Some(ConstantPoolEntry::Utf8("not a constant".to_owned())),
        ]);
        assert_eq!(
            Attribute::ConstantValue(ConstantPoolIndex(2)).encode(&bad_reference_pool),
            Err(EncodeError::InvalidReference { index: 2 })
        );
        assert_eq!(
            Attribute::SourceFile(ConstantPoolIndex(3)).encode(&bad_reference_pool),
            Err(EncodeError::InvalidIndex { index: 3, count: 2 })
        );

        let exceptions_pool = ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("Exceptions".to_owned())),
            Some(ConstantPoolEntry::Utf8("E".to_owned())),
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(2),
            }),
        ]);
        let too_many_classes =
            Attribute::Exceptions(vec![ConstantPoolIndex(3); usize::from(u16::MAX) + 1]);
        assert_eq!(
            too_many_classes.encode(&exceptions_pool),
            Err(EncodeError::CountOverflow {
                value: usize::from(u16::MAX) + 1,
                width: crate::encode_error::LengthWidth::U16,
            })
        );

        let parameters_pool = pool_with_utf8_name("MethodParameters");
        let too_many_parameters = Attribute::MethodParameters(vec![
            MethodParameterEntry {
                name_index: None,
                access_flags: 0,
            };
            usize::from(u8::MAX) + 1
        ]);
        assert_eq!(
            too_many_parameters.encode(&parameters_pool),
            Err(EncodeError::CountOverflow {
                value: usize::from(u8::MAX) + 1,
                width: crate::encode_error::LengthWidth::U8,
            })
        );
    }

    fn complex_attribute_pool() -> ConstantPool {
        ConstantPool::from_entries(vec![
            Some(ConstantPoolEntry::Utf8("RuntimeVisibleAnnotations".into())), // #1
            Some(ConstantPoolEntry::Utf8(
                "RuntimeInvisibleAnnotations".into(),
            )), // #2
            Some(ConstantPoolEntry::Utf8("Code".into())),                      // #3
            Some(ConstantPoolEntry::Utf8("Record".into())),                    // #4
            Some(ConstantPoolEntry::Utf8("StackMapTable".into())),             // #5
            Some(ConstantPoolEntry::Utf8("LExample;".into())),                 // #6
            Some(ConstantPoolEntry::Utf8("value".into())),                     // #7
            Some(ConstantPoolEntry::Integer(1)),                               // #8
            Some(ConstantPoolEntry::Float(1.5)),                               // #9
            Some(ConstantPoolEntry::Long(9)),                                  // #10
            None,                                           // #11, reserved long slot
            Some(ConstantPoolEntry::Double(2.5)),           // #12
            None,                                           // #13, reserved double slot
            Some(ConstantPoolEntry::Utf8("text".into())),   // #14
            Some(ConstantPoolEntry::Utf8("LKind;".into())), // #15
            Some(ConstantPoolEntry::Utf8("ONE".into())),    // #16
            Some(ConstantPoolEntry::Utf8("Ljava/lang/String;".into())), // #17
            Some(ConstantPoolEntry::Utf8("component".into())), // #18
            Some(ConstantPoolEntry::Utf8("I".into())),      // #19
            Some(ConstantPoolEntry::Utf8("C".into())),      // #20
            Some(ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(20),
            }), // #21
            Some(ConstantPoolEntry::Utf8("Signature".into())), // #22
            Some(ConstantPoolEntry::Utf8("I".into())),      // #23
            Some(ConstantPoolEntry::Utf8("component2".into())), // #24
            Some(ConstantPoolEntry::Utf8("J".into())),      // #25
        ])
    }

    #[test]
    fn code_attribute_roundtrips_raw_code_exceptions_and_nested_opaque_attributes() {
        let pool = complex_attribute_pool();
        let nested = Attribute::Other(RawAttribute {
            name_index: ConstantPoolIndex(5),
            bytes: &[0xCA, 0xFE, 0x00],
        });
        let attribute = Attribute::Code(CodeAttribute {
            max_stack: 3,
            max_locals: 2,
            code: &[0xFF, 0x00, 0x7A, 0xB1, 0x42],
            exception_table: vec![
                ExceptionTableEntry {
                    start_pc: 0,
                    end_pc: 2,
                    handler_pc: 2,
                    catch_type: Some(ConstantPoolIndex(21)),
                },
                ExceptionTableEntry {
                    start_pc: 2,
                    end_pc: 5,
                    handler_pc: 4,
                    catch_type: None,
                },
            ],
            attributes: vec![nested],
        });

        let bytes = attribute.encode(&pool).unwrap();
        let mut reader = Reader::new(&bytes);
        assert_eq!(Attribute::decode(&mut reader, &pool), Ok(attribute));
        assert!(reader.is_at_end());
    }

    #[test]
    fn code_attribute_rejects_invalid_lengths_ranges_and_catch_types() {
        let pool = complex_attribute_pool();
        fn code<'a>(bytes: &'a [u8], entries: Vec<ExceptionTableEntry>) -> Attribute<'a> {
            Attribute::Code(CodeAttribute {
                max_stack: 0,
                max_locals: 0,
                code: bytes,
                exception_table: entries,
                attributes: vec![],
            })
        }
        assert_eq!(
            code(&[], vec![]).encode(&pool),
            Err(EncodeError::InvalidCodeLength { length: 0 })
        );
        let oversized = vec![0; usize::from(u16::MAX) + 1];
        assert_eq!(
            code(&oversized, vec![]).encode(&pool),
            Err(EncodeError::InvalidCodeLength {
                length: oversized.len()
            })
        );
        assert_eq!(
            code(
                &[0, 0],
                vec![ExceptionTableEntry {
                    start_pc: 1,
                    end_pc: 1,
                    handler_pc: 1,
                    catch_type: None,
                }]
            )
            .encode(&pool),
            Err(EncodeError::InvalidExceptionTableRange { entry_index: 0 })
        );
        assert_eq!(
            code(
                &[0, 0],
                vec![ExceptionTableEntry {
                    start_pc: 0,
                    end_pc: 2,
                    handler_pc: 2,
                    catch_type: None,
                }]
            )
            .encode(&pool),
            Err(EncodeError::InvalidExceptionTableRange { entry_index: 0 })
        );
        assert_eq!(
            code(
                &[0],
                vec![ExceptionTableEntry {
                    start_pc: 0,
                    end_pc: 1,
                    handler_pc: 0,
                    catch_type: Some(ConstantPoolIndex(1)),
                }]
            )
            .encode(&pool),
            Err(EncodeError::InvalidReference { index: 1 })
        );
    }

    #[test]
    fn record_attribute_roundtrips_empty_and_nested_components() {
        let pool = complex_attribute_pool();
        let empty = Attribute::Record(vec![]);
        let components = Attribute::Record(vec![
            RecordComponentInfo {
                name_index: ConstantPoolIndex(18),
                descriptor_index: ConstantPoolIndex(19),
                attributes: vec![Attribute::Signature(ConstantPoolIndex(23))],
            },
            RecordComponentInfo {
                name_index: ConstantPoolIndex(24),
                descriptor_index: ConstantPoolIndex(25),
                attributes: vec![Attribute::Other(RawAttribute {
                    name_index: ConstantPoolIndex(5),
                    bytes: &[0x01, 0x02],
                })],
            },
        ]);

        for attribute in [empty, components] {
            let bytes = attribute.encode(&pool).unwrap();
            let mut reader = Reader::new(&bytes);
            assert_eq!(Attribute::decode(&mut reader, &pool), Ok(attribute));
            assert!(reader.is_at_end());
        }
    }

    #[test]
    fn annotation_attributes_roundtrip_every_element_value_tag() {
        let pool = complex_attribute_pool();
        let annotation = Annotation {
            type_index: ConstantPoolIndex(6),
            element_value_pairs: vec![
                (
                    ConstantPoolIndex(7),
                    ElementValue::Byte(ConstantPoolIndex(8)),
                ),
                (
                    ConstantPoolIndex(7),
                    ElementValue::Char(ConstantPoolIndex(8)),
                ),
                (
                    ConstantPoolIndex(7),
                    ElementValue::Double(ConstantPoolIndex(12)),
                ),
                (
                    ConstantPoolIndex(7),
                    ElementValue::Float(ConstantPoolIndex(9)),
                ),
                (
                    ConstantPoolIndex(7),
                    ElementValue::Int(ConstantPoolIndex(8)),
                ),
                (
                    ConstantPoolIndex(7),
                    ElementValue::Long(ConstantPoolIndex(10)),
                ),
                (
                    ConstantPoolIndex(7),
                    ElementValue::Short(ConstantPoolIndex(8)),
                ),
                (
                    ConstantPoolIndex(7),
                    ElementValue::Boolean(ConstantPoolIndex(8)),
                ),
                (
                    ConstantPoolIndex(7),
                    ElementValue::String(ConstantPoolIndex(14)),
                ),
                (
                    ConstantPoolIndex(7),
                    ElementValue::Enum {
                        type_name_index: ConstantPoolIndex(15),
                        const_name_index: ConstantPoolIndex(16),
                    },
                ),
                (
                    ConstantPoolIndex(7),
                    ElementValue::Class(ConstantPoolIndex(17)),
                ),
                (
                    ConstantPoolIndex(7),
                    ElementValue::Annotation(Box::new(Annotation {
                        type_index: ConstantPoolIndex(6),
                        element_value_pairs: vec![],
                    })),
                ),
                (
                    ConstantPoolIndex(7),
                    ElementValue::Array(vec![
                        ElementValue::Int(ConstantPoolIndex(8)),
                        ElementValue::Annotation(Box::new(Annotation {
                            type_index: ConstantPoolIndex(6),
                            element_value_pairs: vec![],
                        })),
                    ]),
                ),
            ],
        };
        let attributes = [
            Attribute::RuntimeVisibleAnnotations(vec![annotation.clone()]),
            Attribute::RuntimeInvisibleAnnotations(vec![]),
        ];

        for attribute in attributes {
            let bytes = attribute.encode(&pool).unwrap();
            let mut reader = Reader::new(&bytes);
            assert_eq!(Attribute::decode(&mut reader, &pool), Ok(attribute));
            assert!(reader.is_at_end());
        }
    }

    #[test]
    fn annotation_encoder_enforces_depth_limit_before_reference_walk() {
        let pool = pool_with_utf8_name("RuntimeVisibleAnnotations");
        let mut value = ElementValue::Int(ConstantPoolIndex(1));
        for _ in 0..=DEFAULT_MAX_ANNOTATION_DEPTH {
            value = ElementValue::Array(vec![value]);
        }
        let attribute = Attribute::RuntimeVisibleAnnotations(vec![Annotation {
            type_index: ConstantPoolIndex(1),
            element_value_pairs: vec![(ConstantPoolIndex(1), value)],
        }]);

        assert_eq!(
            attribute.encode(&pool),
            Err(EncodeError::TooDeeplyNestedAnnotation)
        );
    }

    #[test]
    fn complex_attribute_counts_reject_u2_overflow() {
        let over_u2 = usize::from(u16::MAX) + 1;
        let pool = complex_attribute_pool();
        let component = RecordComponentInfo {
            name_index: ConstantPoolIndex(18),
            descriptor_index: ConstantPoolIndex(19),
            attributes: vec![],
        };
        assert_eq!(
            Attribute::Record(vec![component; over_u2]).encode(&pool),
            Err(EncodeError::CountOverflow {
                value: over_u2,
                width: crate::encode_error::LengthWidth::U16,
            })
        );

        let exception = ExceptionTableEntry {
            start_pc: 0,
            end_pc: 1,
            handler_pc: 0,
            catch_type: None,
        };
        assert_eq!(
            Attribute::Code(CodeAttribute {
                max_stack: 0,
                max_locals: 0,
                code: &[0],
                exception_table: vec![exception; over_u2],
                attributes: vec![],
            })
            .encode(&pool),
            Err(EncodeError::CountOverflow {
                value: over_u2,
                width: crate::encode_error::LengthWidth::U16,
            })
        );

        let pair = (
            ConstantPoolIndex(7),
            ElementValue::Int(ConstantPoolIndex(8)),
        );
        assert_eq!(
            Attribute::RuntimeVisibleAnnotations(vec![Annotation {
                type_index: ConstantPoolIndex(6),
                element_value_pairs: vec![pair; over_u2],
            }])
            .encode(&pool),
            Err(EncodeError::CountOverflow {
                value: over_u2,
                width: crate::encode_error::LengthWidth::U16,
            })
        );

        assert_eq!(
            Attribute::RuntimeVisibleAnnotations(vec![Annotation {
                type_index: ConstantPoolIndex(6),
                element_value_pairs: vec![(
                    ConstantPoolIndex(7),
                    ElementValue::Array(vec![ElementValue::Int(ConstantPoolIndex(8)); over_u2]),
                )],
            }])
            .encode(&pool),
            Err(EncodeError::CountOverflow {
                value: over_u2,
                width: crate::encode_error::LengthWidth::U16,
            })
        );
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
    fn decodes_a_synthetic_attribute() {
        let pool = pool_with_utf8_name("Synthetic");
        let bytes = attribute_info_bytes(1, &[]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::Synthetic)
        );
    }

    #[test]
    fn validate_synthetic_is_always_ok() {
        let pool = ConstantPool::from_entries(vec![]);

        assert_eq!(validate_attribute(&Attribute::Synthetic, &pool), Ok(()));
    }

    fn element_value_bytes(tag: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![tag];
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn reads_every_primitive_and_string_element_value_tag() {
        let cases: &[(u8, ElementValue)] = &[
            (b'B', ElementValue::Byte(ConstantPoolIndex(1))),
            (b'C', ElementValue::Char(ConstantPoolIndex(1))),
            (b'D', ElementValue::Double(ConstantPoolIndex(1))),
            (b'F', ElementValue::Float(ConstantPoolIndex(1))),
            (b'I', ElementValue::Int(ConstantPoolIndex(1))),
            (b'J', ElementValue::Long(ConstantPoolIndex(1))),
            (b'S', ElementValue::Short(ConstantPoolIndex(1))),
            (b'Z', ElementValue::Boolean(ConstantPoolIndex(1))),
            (b's', ElementValue::String(ConstantPoolIndex(1))),
            (b'c', ElementValue::Class(ConstantPoolIndex(1))),
        ];

        for (tag, expected) in cases {
            let bytes = element_value_bytes(*tag, &[0x00, 0x01]);
            let mut reader = Reader::new(&bytes);

            assert_eq!(
                read_element_value(&mut reader, 0),
                Ok(expected.clone()),
                "tag {tag:#04x}"
            );
        }
    }

    #[test]
    fn reads_an_enum_element_value() {
        let bytes = element_value_bytes(b'e', &[0x00, 0x01, 0x00, 0x02]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            read_element_value(&mut reader, 0),
            Ok(ElementValue::Enum {
                type_name_index: ConstantPoolIndex(1),
                const_name_index: ConstantPoolIndex(2),
            })
        );
    }

    #[test]
    fn reads_a_nested_annotation_element_value() {
        let bytes = element_value_bytes(
            b'@',
            &[
                0x00, 0x01, // type_index = #1
                0x00, 0x00, // num_element_value_pairs = 0
            ],
        );
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            read_element_value(&mut reader, 0),
            Ok(ElementValue::Annotation(Box::new(Annotation {
                type_index: ConstantPoolIndex(1),
                element_value_pairs: vec![],
            })))
        );
    }

    #[test]
    fn reads_an_array_element_value() {
        let bytes = element_value_bytes(
            b'[',
            &[
                0x00, 0x02, // num_values = 2
                b'I', 0x00, 0x01, // Int(#1)
                b'I', 0x00, 0x02, // Int(#2)
            ],
        );
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            read_element_value(&mut reader, 0),
            Ok(ElementValue::Array(vec![
                ElementValue::Int(ConstantPoolIndex(1)),
                ElementValue::Int(ConstantPoolIndex(2)),
            ]))
        );
    }

    #[test]
    fn rejects_an_unknown_element_value_tag() {
        let bytes = element_value_bytes(b'?', &[]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            read_element_value(&mut reader, 0),
            Err(AttributeError::UnknownElementValueTag { tag: b'?' })
        );
    }

    #[test]
    fn rejects_annotation_nesting_past_the_depth_guard() {
        let bytes = element_value_bytes(b'@', &[0x00, 0x01, 0x00, 0x00]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            read_element_value(&mut reader, DEFAULT_MAX_ANNOTATION_DEPTH),
            Err(AttributeError::TooDeeplyNestedAnnotation)
        );
    }

    #[test]
    fn rejects_array_nesting_past_the_depth_guard() {
        let bytes = element_value_bytes(b'[', &[0x00, 0x00]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            read_element_value(&mut reader, DEFAULT_MAX_ANNOTATION_DEPTH),
            Err(AttributeError::TooDeeplyNestedAnnotation)
        );
    }

    #[test]
    fn decodes_a_runtime_visible_annotations_attribute() {
        let pool = pool_with_utf8_name("RuntimeVisibleAnnotations");
        let bytes = attribute_info_bytes(
            1,
            &[
                0x00, 0x01, // num_annotations = 1
                0x00, 0x02, // type_index = #2
                0x00, 0x01, // num_element_value_pairs = 1
                0x00, 0x03, // element_name_index = #3
                b's', 0x00, 0x04, // String(#4)
            ],
        );
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::RuntimeVisibleAnnotations(vec![Annotation {
                type_index: ConstantPoolIndex(2),
                element_value_pairs: vec![(
                    ConstantPoolIndex(3),
                    ElementValue::String(ConstantPoolIndex(4))
                )],
            }]))
        );
    }

    #[test]
    fn decodes_a_runtime_invisible_annotations_attribute_with_no_annotations() {
        let pool = pool_with_utf8_name("RuntimeInvisibleAnnotations");
        let bytes = attribute_info_bytes(1, &[0x00, 0x00]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Ok(Attribute::RuntimeInvisibleAnnotations(vec![]))
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

    fn pool_with(entries: Vec<ConstantPoolEntry>) -> ConstantPool {
        ConstantPool::from_entries(entries.into_iter().map(Some).collect())
    }

    #[test]
    fn validate_element_value_accepts_every_integer_backed_tag() {
        let pool = pool_with(vec![ConstantPoolEntry::Integer(1)]);
        let index = ConstantPoolIndex(1);

        for value in [
            ElementValue::Byte(index),
            ElementValue::Char(index),
            ElementValue::Int(index),
            ElementValue::Short(index),
            ElementValue::Boolean(index),
        ] {
            assert_eq!(validate_element_value(&value, &pool), Ok(()));
        }
    }

    #[test]
    fn validate_element_value_rejects_an_integer_backed_tag_pointing_elsewhere() {
        let pool = pool_with(vec![ConstantPoolEntry::Long(1)]);

        assert_eq!(
            validate_element_value(&ElementValue::Int(ConstantPoolIndex(1)), &pool),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: crate::constant_pool::EntryKind::Integer,
            })
        );
    }

    #[test]
    fn validate_element_value_accepts_long_float_and_double() {
        let pool = pool_with(vec![
            ConstantPoolEntry::Long(1),
            ConstantPoolEntry::Float(1.0),
            ConstantPoolEntry::Double(1.0),
        ]);

        assert_eq!(
            validate_element_value(&ElementValue::Long(ConstantPoolIndex(1)), &pool),
            Ok(())
        );
        assert_eq!(
            validate_element_value(&ElementValue::Float(ConstantPoolIndex(2)), &pool),
            Ok(())
        );
        assert_eq!(
            validate_element_value(&ElementValue::Double(ConstantPoolIndex(3)), &pool),
            Ok(())
        );
    }

    #[test]
    fn validate_element_value_string_requires_utf8_not_a_string_constant() {
        let utf8_pool = pool_with(vec![ConstantPoolEntry::Utf8("hi".to_owned())]);
        assert_eq!(
            validate_element_value(&ElementValue::String(ConstantPoolIndex(1)), &utf8_pool),
            Ok(())
        );

        let string_constant_pool = pool_with(vec![
            ConstantPoolEntry::Utf8("hi".to_owned()),
            ConstantPoolEntry::String {
                string_index: ConstantPoolIndex(1),
            },
        ]);
        assert_eq!(
            validate_element_value(
                &ElementValue::String(ConstantPoolIndex(2)),
                &string_constant_pool
            ),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(2),
                expected: crate::constant_pool::EntryKind::Utf8,
            })
        );
    }

    #[test]
    fn validate_element_value_class_requires_utf8_not_a_class_entry() {
        let utf8_pool = pool_with(vec![ConstantPoolEntry::Utf8(
            "Ljava/lang/String;".to_owned(),
        )]);
        assert_eq!(
            validate_element_value(&ElementValue::Class(ConstantPoolIndex(1)), &utf8_pool),
            Ok(())
        );

        let class_entry_pool = pool_with(vec![
            ConstantPoolEntry::Utf8("java/lang/String".to_owned()),
            ConstantPoolEntry::Class {
                name_index: ConstantPoolIndex(1),
            },
        ]);
        assert_eq!(
            validate_element_value(
                &ElementValue::Class(ConstantPoolIndex(2)),
                &class_entry_pool
            ),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(2),
                expected: crate::constant_pool::EntryKind::Utf8,
            })
        );
    }

    #[test]
    fn validate_element_value_enum_accepts_and_rejects() {
        let pool = pool_with(vec![
            ConstantPoolEntry::Utf8("Ljava/time/DayOfWeek;".to_owned()),
            ConstantPoolEntry::Utf8("MONDAY".to_owned()),
        ]);
        let good = ElementValue::Enum {
            type_name_index: ConstantPoolIndex(1),
            const_name_index: ConstantPoolIndex(2),
        };
        assert_eq!(validate_element_value(&good, &pool), Ok(()));

        let bad = ElementValue::Enum {
            type_name_index: ConstantPoolIndex(1),
            const_name_index: ConstantPoolIndex(99),
        };
        assert_eq!(
            validate_element_value(&bad, &pool),
            Err(PoolRefError::InvalidIndex {
                index: ConstantPoolIndex(99)
            })
        );
    }

    #[test]
    fn validate_element_value_recurses_into_a_nested_annotation() {
        let good_pool = pool_with(vec![
            ConstantPoolEntry::Utf8("Ljava/lang/Deprecated;".to_owned()),
            ConstantPoolEntry::Utf8("since".to_owned()),
            ConstantPoolEntry::Utf8("1.0".to_owned()),
        ]);
        let good = ElementValue::Annotation(Box::new(Annotation {
            type_index: ConstantPoolIndex(1),
            element_value_pairs: vec![(
                ConstantPoolIndex(2),
                ElementValue::String(ConstantPoolIndex(3)),
            )],
        }));
        assert_eq!(validate_element_value(&good, &good_pool), Ok(()));

        let bad_pool = pool_with(vec![ConstantPoolEntry::Integer(1)]);
        let bad = ElementValue::Annotation(Box::new(Annotation {
            type_index: ConstantPoolIndex(1),
            element_value_pairs: vec![],
        }));
        assert_eq!(
            validate_element_value(&bad, &bad_pool),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: crate::constant_pool::EntryKind::Utf8,
            })
        );
    }

    #[test]
    fn validate_element_value_recurses_into_an_array() {
        let good_pool = pool_with(vec![ConstantPoolEntry::Integer(1)]);
        let good = ElementValue::Array(vec![
            ElementValue::Int(ConstantPoolIndex(1)),
            ElementValue::Int(ConstantPoolIndex(1)),
        ]);
        assert_eq!(validate_element_value(&good, &good_pool), Ok(()));

        let bad_pool = pool_with(vec![ConstantPoolEntry::Long(1)]);
        let bad = ElementValue::Array(vec![ElementValue::Int(ConstantPoolIndex(1))]);
        assert_eq!(
            validate_element_value(&bad, &bad_pool),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: crate::constant_pool::EntryKind::Integer,
            })
        );
    }

    #[test]
    fn validate_attribute_accepts_a_well_formed_runtime_visible_annotations_attribute() {
        let pool = pool_with(vec![
            ConstantPoolEntry::Utf8("Ljava/lang/Deprecated;".to_owned()),
            ConstantPoolEntry::Utf8("forRemoval".to_owned()),
            ConstantPoolEntry::Integer(1),
        ]);
        let attribute = Attribute::RuntimeVisibleAnnotations(vec![Annotation {
            type_index: ConstantPoolIndex(1),
            element_value_pairs: vec![(
                ConstantPoolIndex(2),
                ElementValue::Boolean(ConstantPoolIndex(3)),
            )],
        }]);

        assert_eq!(validate_attribute(&attribute, &pool), Ok(()));
    }

    #[test]
    fn validate_attribute_propagates_a_bad_reference_in_an_annotation() {
        let pool = pool_with(vec![ConstantPoolEntry::Integer(1)]);
        let attribute = Attribute::RuntimeInvisibleAnnotations(vec![Annotation {
            type_index: ConstantPoolIndex(1),
            element_value_pairs: vec![],
        }]);

        assert_eq!(
            validate_attribute(&attribute, &pool),
            Err(PoolRefError::WrongKind {
                index: ConstantPoolIndex(1),
                expected: crate::constant_pool::EntryKind::Utf8,
            })
        );
    }

    fn deprecated_annotation(
        type_index: ConstantPoolIndex,
        element_value_pairs: Vec<(ConstantPoolIndex, ElementValue)>,
    ) -> Attribute<'static> {
        Attribute::RuntimeVisibleAnnotations(vec![Annotation {
            type_index,
            element_value_pairs,
        }])
    }

    #[test]
    fn find_deprecated_annotation_reads_since_and_for_removal() {
        let pool = pool_with(vec![
            ConstantPoolEntry::Utf8("Ljava/lang/Deprecated;".to_owned()),
            ConstantPoolEntry::Utf8("since".to_owned()),
            ConstantPoolEntry::Utf8("1.0".to_owned()),
            ConstantPoolEntry::Utf8("forRemoval".to_owned()),
            ConstantPoolEntry::Integer(1),
        ]);
        let attributes = vec![deprecated_annotation(
            ConstantPoolIndex(1),
            vec![
                (
                    ConstantPoolIndex(2),
                    ElementValue::String(ConstantPoolIndex(3)),
                ),
                (
                    ConstantPoolIndex(4),
                    ElementValue::Boolean(ConstantPoolIndex(5)),
                ),
            ],
        )];

        assert_eq!(
            find_deprecated_annotation(&attributes, &pool),
            Some(DeprecationInfo {
                since: Some("1.0".to_owned()),
                for_removal: true,
            })
        );
    }

    #[test]
    fn find_deprecated_annotation_defaults_for_removal_to_false_when_absent() {
        let pool = pool_with(vec![ConstantPoolEntry::Utf8(
            "Ljava/lang/Deprecated;".to_owned(),
        )]);
        let attributes = vec![deprecated_annotation(ConstantPoolIndex(1), vec![])];

        assert_eq!(
            find_deprecated_annotation(&attributes, &pool),
            Some(DeprecationInfo {
                since: None,
                for_removal: false,
            })
        );
    }

    #[test]
    fn find_deprecated_annotation_ignores_an_unrelated_annotation_type() {
        let pool = pool_with(vec![ConstantPoolEntry::Utf8(
            "Ljava/lang/FunctionalInterface;".to_owned(),
        )]);
        let attributes = vec![deprecated_annotation(ConstantPoolIndex(1), vec![])];

        assert_eq!(find_deprecated_annotation(&attributes, &pool), None);
    }

    #[test]
    fn find_deprecated_annotation_returns_none_without_any_annotations() {
        let pool = ConstantPool::from_entries(vec![]);

        assert_eq!(find_deprecated_annotation(&[], &pool), None);
    }

    #[test]
    fn reports_a_truncated_element_value_inside_an_annotation() {
        let pool = pool_with_utf8_name("RuntimeVisibleAnnotations");
        let bytes = attribute_info_bytes(
            1,
            &[
                0x00, 0x01, // num_annotations = 1
                0x00, 0x02, // type_index = #2
                0x00, 0x01, // num_element_value_pairs = 1
                0x00, 0x03, // element_name_index = #3
                b'B', 0x00, // Int element_value's index truncated to 1 byte
            ],
        );
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            Attribute::decode(&mut reader, &pool),
            Err(AttributeError::Read(ReadError::UnexpectedEof {
                offset: 15,
                needed: 2,
                remaining: 1,
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
