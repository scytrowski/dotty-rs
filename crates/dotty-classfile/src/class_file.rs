use crate::access_flags::ClassAccessFlags;
use crate::attribute::Attribute;
use crate::constant_pool::{ConstantPool, ConstantPoolIndex};
use crate::field::FieldInfo;
use crate::method::MethodInfo;

/// The `minor_version`/`major_version` pair of a class file (JVMS §4.1).
/// Compatibility-range checks land alongside the header decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassFileVersion {
    pub major: u16,
    pub minor: u16,
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
