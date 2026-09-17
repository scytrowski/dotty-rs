use crate::access_flags::MethodAccessFlags;
use crate::attribute::{Attribute, AttributeError, decode_attributes};
use crate::constant_pool::{ConstantPool, ConstantPoolIndex, read_index};
use crate::descriptor::{MethodDescriptor, ResolveDescriptorError, resolve_method_descriptor};
use crate::reader::Reader;

/// A `method_info` structure (JVMS §4.6).
#[derive(Debug, Clone, PartialEq)]
pub struct MethodInfo<'a> {
    pub access_flags: MethodAccessFlags,
    pub name_index: ConstantPoolIndex,
    pub descriptor_index: ConstantPoolIndex,
    pub attributes: Vec<Attribute<'a>>,
}

impl<'a> MethodInfo<'a> {
    pub fn decode(
        reader: &mut Reader<'a>,
        constant_pool: &ConstantPool,
    ) -> Result<Self, AttributeError> {
        let access_flags = MethodAccessFlags(reader.read_u16()?);
        let name_index = read_index(reader)?;
        let descriptor_index = read_index(reader)?;
        let attributes = decode_attributes(reader, constant_pool)?;

        Ok(Self {
            access_flags,
            name_index,
            descriptor_index,
            attributes,
        })
    }

    /// Resolves and parses this method's descriptor (JVMS §4.3.3).
    pub fn descriptor(
        &self,
        constant_pool: &ConstantPool,
    ) -> Result<MethodDescriptor, ResolveDescriptorError> {
        resolve_method_descriptor(constant_pool, self.descriptor_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access_flags::ACC_PUBLIC;
    use crate::constant_pool::ConstantPoolEntry;
    use crate::reader::ReadError;

    fn method_info_bytes(attributes_payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&ACC_PUBLIC.to_be_bytes());
        bytes.extend_from_slice(&2u16.to_be_bytes()); // name_index = #2
        bytes.extend_from_slice(&3u16.to_be_bytes()); // descriptor_index = #3
        bytes.extend_from_slice(attributes_payload);
        bytes
    }

    #[test]
    fn decodes_a_method_with_no_attributes() {
        let pool = ConstantPool::from_entries(vec![]);
        let bytes = method_info_bytes(&[0x00, 0x00]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            MethodInfo::decode(&mut reader, &pool),
            Ok(MethodInfo {
                access_flags: MethodAccessFlags(ACC_PUBLIC),
                name_index: ConstantPoolIndex(2),
                descriptor_index: ConstantPoolIndex(3),
                attributes: vec![],
            })
        );
    }

    #[test]
    fn decodes_a_method_with_a_deprecated_attribute() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Utf8(
            "Deprecated".to_owned(),
        ))]);
        let bytes = method_info_bytes(&[
            0x00, 0x01, // attributes_count = 1
            0x00, 0x01, // attribute_name_index = #1 ("Deprecated")
            0x00, 0x00, 0x00, 0x00, // attribute_length = 0
        ]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            MethodInfo::decode(&mut reader, &pool),
            Ok(MethodInfo {
                access_flags: MethodAccessFlags(ACC_PUBLIC),
                name_index: ConstantPoolIndex(2),
                descriptor_index: ConstantPoolIndex(3),
                attributes: vec![Attribute::Deprecated],
            })
        );
    }

    #[test]
    fn reports_a_method_truncated_before_its_attributes_count() {
        let pool = ConstantPool::from_entries(vec![]);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&ACC_PUBLIC.to_be_bytes());
        bytes.extend_from_slice(&2u16.to_be_bytes());
        bytes.extend_from_slice(&3u16.to_be_bytes());
        bytes.push(0x00); // attributes_count truncated to 1 byte
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            MethodInfo::decode(&mut reader, &pool),
            Err(AttributeError::Read(ReadError::UnexpectedEof {
                offset: 6,
                needed: 2,
                remaining: 1,
            }))
        );
    }

    fn method_with_descriptor(descriptor: ConstantPoolIndex) -> MethodInfo<'static> {
        MethodInfo {
            access_flags: MethodAccessFlags(ACC_PUBLIC),
            name_index: ConstantPoolIndex(2),
            descriptor_index: descriptor,
            attributes: vec![],
        }
    }

    #[test]
    fn resolves_and_parses_a_method_descriptor() {
        let pool = ConstantPool::from_entries(vec![
            None,
            None,
            Some(ConstantPoolEntry::Utf8("()I".to_owned())),
        ]);

        assert_eq!(
            method_with_descriptor(ConstantPoolIndex(3)).descriptor(&pool),
            Ok(MethodDescriptor {
                parameters: vec![],
                return_type: Some(crate::descriptor::FieldType::Int),
            })
        );
    }

    #[test]
    fn reports_a_method_descriptor_that_does_not_resolve_to_utf8() {
        let pool =
            ConstantPool::from_entries(vec![None, None, Some(ConstantPoolEntry::Integer(1))]);

        assert_eq!(
            method_with_descriptor(ConstantPoolIndex(3)).descriptor(&pool),
            Err(ResolveDescriptorError::NotUtf8 {
                index: ConstantPoolIndex(3)
            })
        );
    }

    #[test]
    fn reports_a_method_descriptor_that_fails_to_parse() {
        let pool = ConstantPool::from_entries(vec![
            None,
            None,
            Some(ConstantPoolEntry::Utf8("I)V".to_owned())),
        ]);

        assert_eq!(
            method_with_descriptor(ConstantPoolIndex(3)).descriptor(&pool),
            Err(ResolveDescriptorError::Parse(
                crate::descriptor::DescriptorError::MissingOpenParen { offset: 0 }
            ))
        );
    }
}
