use crate::access_flags::MethodAccessFlags;
use crate::attribute::Attribute;
use crate::constant_pool::ConstantPoolIndex;

/// A `method_info` structure (JVMS §4.6).
#[derive(Debug, Clone, PartialEq)]
pub struct MethodInfo<'a> {
    pub access_flags: MethodAccessFlags,
    pub name_index: ConstantPoolIndex,
    pub descriptor_index: ConstantPoolIndex,
    pub attributes: Vec<Attribute<'a>>,
}
