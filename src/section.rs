use crate::name_table::NameRef;
use crate::reader::{ReadError, Reader};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section<'a> {
    pub name: NameRef,
    pub offset: usize,
    pub length: usize,
    pub payload: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionTable<'a> {
    sections: Vec<Section<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardSection {
    Asts,
    Positions,
    Comments,
    Attributes,
}

pub const SCALA2STANDARDLIBRARY_ATTR: u8 = 1;
pub const EXPLICITNULLS_ATTR: u8 = 2;
pub const CAPTURECHECKED_ATTR: u8 = 3;
pub const WITHPUREFUNS_ATTR: u8 = 4;
pub const JAVA_ATTR: u8 = 5;
pub const OUTLINE_ATTR: u8 = 6;
pub const SOURCEFILE_ATTR: u8 = 129;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attribute {
    Scala2StandardLibrary,
    ExplicitNulls,
    CaptureChecked,
    WithPureFuns,
    Java,
    Outline,
    SourceFile(NameRef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub text: String,
    pub coordinates: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PositionEntry {
    Association {
        address_delta: u32,
        start_delta: Option<i64>,
        end_delta: Option<i64>,
        point_delta: Option<i64>,
    },
    Source(NameRef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionSection {
    pub line_sizes: Vec<u32>,
    pub entries: Vec<PositionEntry>,
}

impl StandardSection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Asts => "ASTs",
            Self::Positions => "Positions",
            Self::Comments => "Comments",
            Self::Attributes => "Attributes",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionError {
    Read(ReadError),
    InvalidNameReference {
        reference: NameRef,
        name_count: usize,
        offset: usize,
    },
    InvalidAttributeTag {
        tag: u8,
        offset: usize,
    },
    AttributesNotOrdered {
        previous: u8,
        current: u8,
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
            Self::InvalidAttributeTag { tag, offset } => {
                write!(formatter, "invalid attribute tag {tag} at offset {offset}")
            }
            Self::AttributesNotOrdered {
                previous,
                current,
                offset,
            } => write!(
                formatter,
                "attribute tag {current} at offset {offset} follows tag {previous}"
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

impl<'a> Section<'a> {
    pub fn reader(&self) -> Reader<'a> {
        Reader::new(self.payload)
    }

    pub fn decode_attributes(&self) -> Result<Vec<Attribute>, SectionError> {
        let mut reader = self.reader();
        let mut attributes = Vec::new();
        let mut previous = None;

        while !reader.is_at_end() {
            let offset = reader.position();
            let tag = reader.read_u8()?;
            if previous.is_some_and(|previous| tag <= previous) {
                return Err(SectionError::AttributesNotOrdered {
                    previous: previous.unwrap(),
                    current: tag,
                    offset,
                });
            }
            previous = Some(tag);

            let attribute = match tag {
                SCALA2STANDARDLIBRARY_ATTR => Attribute::Scala2StandardLibrary,
                EXPLICITNULLS_ATTR => Attribute::ExplicitNulls,
                CAPTURECHECKED_ATTR => Attribute::CaptureChecked,
                WITHPUREFUNS_ATTR => Attribute::WithPureFuns,
                JAVA_ATTR => Attribute::Java,
                OUTLINE_ATTR => Attribute::Outline,
                SOURCEFILE_ATTR => Attribute::SourceFile(reader.read_nat()?),
                _ => return Err(SectionError::InvalidAttributeTag { tag, offset }),
            };
            attributes.push(attribute);
        }

        Ok(attributes)
    }

    pub fn decode_comments(&self) -> Result<Vec<Comment>, SectionError> {
        let mut reader = self.reader();
        let mut comments = Vec::new();

        while !reader.is_at_end() {
            comments.push(Comment {
                text: reader.read_utf8()?,
                coordinates: reader.read_long_int()?,
            });
        }

        Ok(comments)
    }

    pub fn decode_positions(&self) -> Result<PositionSection, SectionError> {
        let mut reader = self.reader();
        let line_count = reader.read_nat()? as usize;
        let mut line_sizes = Vec::with_capacity(line_count);
        for _ in 0..line_count {
            line_sizes.push(reader.read_nat()?);
        }

        let mut entries = Vec::new();
        while !reader.is_at_end() {
            let header = reader.read_nat()?;
            if header == 4 {
                entries.push(PositionEntry::Source(reader.read_nat()?));
                continue;
            }

            entries.push(PositionEntry::Association {
                address_delta: header >> 3,
                start_delta: if header & 0b100 != 0 {
                    Some(reader.read_int()? as i64)
                } else {
                    None
                },
                end_delta: if header & 0b010 != 0 {
                    Some(reader.read_int()? as i64)
                } else {
                    None
                },
                point_delta: if header & 0b001 != 0 {
                    Some(reader.read_int()? as i64)
                } else {
                    None
                },
            });
        }

        Ok(PositionSection {
            line_sizes,
            entries,
        })
    }

    pub fn standard_kind(&self, names: &crate::name_table::NameTable) -> Option<StandardSection> {
        let name = match names.get_zero_based(self.name)? {
            crate::name_table::RawName::Utf8(name) => name.as_str(),
            _ => return None,
        };

        match name {
            "ASTs" => Some(StandardSection::Asts),
            "Positions" => Some(StandardSection::Positions),
            "Comments" => Some(StandardSection::Comments),
            "Attributes" => Some(StandardSection::Attributes),
            _ => None,
        }
    }
}

impl<'a> SectionTable<'a> {
    pub fn decode(reader: &mut Reader<'a>, name_count: usize) -> Result<Self, SectionError> {
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
            let payload = reader.read_bytes(length)?;

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

    pub fn get(&self, index: usize) -> Option<&Section<'a>> {
        self.sections.get(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Section<'a>> {
        self.sections.iter()
    }

    pub fn entries(&self) -> &[Section<'a>] {
        &self.sections
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Attribute, Comment, PositionEntry, PositionSection, Section, SectionError, SectionTable,
    };
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
        assert_eq!(sections.get(0).unwrap().payload, b"abc".as_slice());
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

    #[test]
    fn decodes_ordered_attributes_and_source_file_reference() {
        let section = Section {
            name: 0,
            offset: 0,
            length: 0,
            payload: &[1, 2, 3, 4, 5, 6, 129, 0x85],
        };

        assert_eq!(
            section.decode_attributes().unwrap(),
            vec![
                Attribute::Scala2StandardLibrary,
                Attribute::ExplicitNulls,
                Attribute::CaptureChecked,
                Attribute::WithPureFuns,
                Attribute::Java,
                Attribute::Outline,
                Attribute::SourceFile(5),
            ]
        );
    }

    #[test]
    fn rejects_duplicate_or_out_of_order_attributes() {
        let section = Section {
            name: 0,
            offset: 0,
            length: 0,
            payload: &[2, 1],
        };

        assert_eq!(
            section.decode_attributes(),
            Err(SectionError::AttributesNotOrdered {
                previous: 2,
                current: 1,
                offset: 1,
            })
        );
    }

    #[test]
    fn rejects_unknown_attribute_tags() {
        let section = Section {
            name: 0,
            offset: 0,
            length: 0,
            payload: &[7],
        };

        assert_eq!(
            section.decode_attributes(),
            Err(SectionError::InvalidAttributeTag { tag: 7, offset: 0 })
        );
    }

    #[test]
    fn decodes_comments_with_utf8_text_and_coordinates() {
        let section = Section {
            name: 0,
            offset: 0,
            length: 0,
            payload: &[0x82, b'h', b'i', 0x83],
        };

        assert_eq!(
            section.decode_comments().unwrap(),
            vec![Comment {
                text: "hi".to_owned(),
                coordinates: 3,
            }]
        );
    }

    #[test]
    fn reports_truncated_comment_coordinates() {
        let section = Section {
            name: 0,
            offset: 0,
            length: 0,
            payload: &[0x81, b'x'],
        };

        assert_eq!(
            section.decode_comments(),
            Err(SectionError::Read(ReadError::UnexpectedEof {
                offset: 2,
                needed: 1,
                remaining: 0,
            }))
        );
    }

    #[test]
    fn decodes_line_sizes_and_position_associations() {
        let section = Section {
            name: 0,
            offset: 0,
            length: 0,
            payload: &[0x82, 0x8a, 0x94, 0x8f, 0x82, 0x83, 0x84, 0x84, 0x85],
        };

        assert_eq!(
            section.decode_positions().unwrap(),
            PositionSection {
                line_sizes: vec![10, 20],
                entries: vec![
                    PositionEntry::Association {
                        address_delta: 1,
                        start_delta: Some(2),
                        end_delta: Some(3),
                        point_delta: Some(4),
                    },
                    PositionEntry::Source(5),
                ],
            }
        );
    }

    #[test]
    fn preserves_signed_position_deltas() {
        let section = Section {
            name: 0,
            offset: 0,
            length: 0,
            payload: &[0x80, 0x8e, 0xfe, 0x82],
        };

        assert_eq!(
            section.decode_positions().unwrap().entries,
            vec![PositionEntry::Association {
                address_delta: 1,
                start_delta: Some(-2),
                end_delta: Some(2),
                point_delta: None,
            }]
        );
    }
}
