use crate::name_table::NameRef;
use crate::reader::{ReadError, Reader};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: NameRef,
    pub offset: usize,
    pub length: usize,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionTable {
    sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionError {
    Read(ReadError),
    InvalidNameReference {
        reference: NameRef,
        name_count: usize,
        offset: usize,
    },
}

impl fmt::Display for SectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::InvalidNameReference {
                reference,
                name_count,
                offset,
            } => write!(
                formatter,
                "invalid section name reference {reference} at offset {offset}; name table contains {name_count} entries"
            ),
        }
    }
}

impl std::error::Error for SectionError {}

impl From<ReadError> for SectionError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

impl SectionTable {
    pub fn decode(reader: &mut Reader<'_>, name_count: usize) -> Result<Self, SectionError> {
        let mut sections = Vec::new();

        while !reader.is_at_end() {
            let name_offset = reader.position();
            let name = reader.read_nat()?;
            // Section names are encoded as zero-based indexes in emitted
            // Scala 3.9.0 files. This differs from the one-based NameRef
            // convention used by names referenced from AST nodes.
            if name as usize >= name_count {
                return Err(SectionError::InvalidNameReference {
                    reference: name,
                    name_count,
                    offset: name_offset,
                });
            }

            let length = reader.read_nat()? as usize;
            let offset = reader.position();
            let payload = reader.read_bytes(length)?.to_vec();

            sections.push(Section {
                name,
                offset,
                length,
                payload,
            });
        }

        Ok(Self { sections })
    }

    pub fn len(&self) -> usize {
        self.sections.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&Section> {
        self.sections.get(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Section> {
        self.sections.iter()
    }

    pub fn entries(&self) -> &[Section] {
        &self.sections
    }
}

#[cfg(test)]
mod tests {
    use super::{SectionError, SectionTable};
    use crate::reader::{ReadError, Reader};

    #[test]
    fn decodes_length_delimited_sections() {
        // zero-based section name index 0, payload length 3, payload "abc"
        let bytes = [0x80, 0x83, b'a', b'b', b'c'];
        let mut reader = Reader::new(&bytes);
        let sections = SectionTable::decode(&mut reader, 1).unwrap();

        assert_eq!(sections.len(), 1);
        assert_eq!(sections.get(0).unwrap().name, 0);
        assert_eq!(sections.get(0).unwrap().offset, 2);
        assert_eq!(sections.get(0).unwrap().length, 3);
        assert_eq!(sections.get(0).unwrap().payload, b"abc");
        assert!(reader.is_at_end());
    }

    #[test]
    fn rejects_a_section_name_outside_the_name_table() {
        let mut reader = Reader::new(&[0x81]);

        assert_eq!(
            SectionTable::decode(&mut reader, 1),
            Err(SectionError::InvalidNameReference {
                reference: 1,
                name_count: 1,
                offset: 0,
            })
        );
    }

    #[test]
    fn reports_a_truncated_section_payload() {
        let mut reader = Reader::new(&[0x80, 0x83, b'a']);

        assert_eq!(
            SectionTable::decode(&mut reader, 1),
            Err(SectionError::Read(ReadError::UnexpectedEof {
                offset: 2,
                needed: 3,
                remaining: 1,
            }))
        );
    }
}
