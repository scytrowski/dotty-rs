use crate::constant_pool::ConstantPoolIndex;

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
