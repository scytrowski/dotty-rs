use crate::access_flags::FieldAccessFlags;
use crate::attribute::Attribute;
use crate::constant_pool::ConstantPoolIndex;

/// A `field_info` structure (JVMS §4.5).
#[derive(Debug, Clone, PartialEq)]
pub struct FieldInfo<'a> {
    pub access_flags: FieldAccessFlags,
    pub name_index: ConstantPoolIndex,
    pub descriptor_index: ConstantPoolIndex,
    pub attributes: Vec<Attribute<'a>>,
}
