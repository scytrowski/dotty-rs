use crate::access_flags::FieldAccessFlags;
use crate::attribute::{Attribute, AttributeError, decode_attributes};
use crate::constant_pool::{ConstantPool, ConstantPoolIndex, read_index};
use crate::reader::Reader;

/// A `field_info` structure (JVMS §4.5).
#[derive(Debug, Clone, PartialEq)]
pub struct FieldInfo<'a> {
    pub access_flags: FieldAccessFlags,
    pub name_index: ConstantPoolIndex,
    pub descriptor_index: ConstantPoolIndex,
    pub attributes: Vec<Attribute<'a>>,
}

impl<'a> FieldInfo<'a> {
    pub fn decode(
        reader: &mut Reader<'a>,
        constant_pool: &ConstantPool,
    ) -> Result<Self, AttributeError> {
        let access_flags = FieldAccessFlags(reader.read_u16()?);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access_flags::ACC_PUBLIC;
    use crate::constant_pool::ConstantPoolEntry;
    use crate::reader::ReadError;

    fn field_info_bytes(attributes_payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&ACC_PUBLIC.to_be_bytes());
        bytes.extend_from_slice(&2u16.to_be_bytes()); // name_index = #2
        bytes.extend_from_slice(&3u16.to_be_bytes()); // descriptor_index = #3
        bytes.extend_from_slice(attributes_payload);
        bytes
    }

    #[test]
    fn decodes_a_field_with_no_attributes() {
        let pool = ConstantPool::from_entries(vec![]);
        let bytes = field_info_bytes(&[0x00, 0x00]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            FieldInfo::decode(&mut reader, &pool),
            Ok(FieldInfo {
                access_flags: FieldAccessFlags(ACC_PUBLIC),
                name_index: ConstantPoolIndex(2),
                descriptor_index: ConstantPoolIndex(3),
                attributes: vec![],
            })
        );
    }

    #[test]
    fn decodes_a_field_with_a_constant_value_attribute() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Utf8(
            "ConstantValue".to_owned(),
        ))]);
        let bytes = field_info_bytes(&[
            0x00, 0x01, // attributes_count = 1
            0x00, 0x01, // attribute_name_index = #1 ("ConstantValue")
            0x00, 0x00, 0x00, 0x02, // attribute_length = 2
            0x00, 0x05, // constantvalue_index = #5
        ]);
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            FieldInfo::decode(&mut reader, &pool),
            Ok(FieldInfo {
                access_flags: FieldAccessFlags(ACC_PUBLIC),
                name_index: ConstantPoolIndex(2),
                descriptor_index: ConstantPoolIndex(3),
                attributes: vec![Attribute::ConstantValue(ConstantPoolIndex(5))],
            })
        );
    }

    #[test]
    fn reports_a_field_truncated_before_its_descriptor_index() {
        let pool = ConstantPool::from_entries(vec![]);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&ACC_PUBLIC.to_be_bytes());
        bytes.extend_from_slice(&2u16.to_be_bytes());
        bytes.push(0x00); // descriptor_index truncated to 1 byte
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            FieldInfo::decode(&mut reader, &pool),
            Err(AttributeError::Read(ReadError::UnexpectedEof {
                offset: 4,
                needed: 2,
                remaining: 1,
            }))
        );
    }
}
