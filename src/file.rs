use crate::ast::{AstError, RawNodes};
use crate::header::{Header, HeaderError};
use crate::name_table::{NameRef, NameTable, NameTableError, RawName};
use crate::reader::Reader;
use crate::section::{
    Attribute, Comment, PositionSection, Section, SectionError, SectionTable, StandardSection,
};
use crate::writer::{WriteError, Writer};
use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub struct TastyFile<'a> {
    header: Header,
    names: NameTable,
    sections: SectionTable<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedTastyFile {
    bytes: Vec<u8>,
    ast_addresses: Vec<u32>,
}

impl EncodedTastyFile {
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.bytes
    }

    pub fn ast_addresses(&self) -> &[u32] {
        &self.ast_addresses
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TastyFileError {
    Header(HeaderError),
    Names(NameTableError),
    Sections(SectionError),
    Asts(AstError),
    Write(WriteError),
    MissingSection(StandardSection),
    InvalidNameReference {
        context: &'static str,
        reference: NameRef,
    },
    InvalidAstAddress {
        context: &'static str,
        address: i64,
        asts_length: usize,
    },
}

impl fmt::Display for TastyFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Header(error) => write!(formatter, "invalid TASTy header: {error}"),
            Self::Names(error) => write!(formatter, "invalid TASTy name table: {error}"),
            Self::Sections(error) => write!(formatter, "invalid TASTy section table: {error}"),
            Self::Asts(error) => write!(formatter, "invalid TASTy ASTs section: {error}"),
            Self::Write(error) => write!(formatter, "failed to encode TASTy file: {error}"),
            Self::MissingSection(section) => {
                write!(formatter, "TASTy file has no {} section", section.as_str())
            }
            Self::InvalidNameReference { context, reference } => write!(
                formatter,
                "{context} contains invalid name reference {reference}"
            ),
            Self::InvalidAstAddress {
                context,
                address,
                asts_length,
            } => write!(
                formatter,
                "{context} contains AST address {address}, outside the ASTs payload of {asts_length} bytes"
            ),
        }
    }
}

impl std::error::Error for TastyFileError {}

impl From<HeaderError> for TastyFileError {
    fn from(error: HeaderError) -> Self {
        Self::Header(error)
    }
}

impl From<NameTableError> for TastyFileError {
    fn from(error: NameTableError) -> Self {
        Self::Names(error)
    }
}

impl From<SectionError> for TastyFileError {
    fn from(error: SectionError) -> Self {
        Self::Sections(error)
    }
}

impl From<AstError> for TastyFileError {
    fn from(error: AstError) -> Self {
        Self::Asts(error)
    }
}

impl From<WriteError> for TastyFileError {
    fn from(error: WriteError) -> Self {
        Self::Write(error)
    }
}

impl<'a> TastyFile<'a> {
    pub fn from_parts(
        header: Header,
        names: NameTable,
        sections: SectionTable<'a>,
    ) -> Result<Self, TastyFileError> {
        sections.validate_references(names.len())?;
        Ok(Self {
            header,
            names,
            sections,
        })
    }

    pub fn parse(bytes: &'a [u8]) -> Result<Self, TastyFileError> {
        let mut reader = Reader::new(bytes);
        let header = Header::decode(&mut reader)?;
        let names = NameTable::decode(&mut reader)?;
        let sections = SectionTable::decode(&mut reader, names.len())?;

        Ok(Self {
            header,
            names,
            sections,
        })
    }

    pub fn parse_scala_3_9(bytes: &'a [u8]) -> Result<Self, TastyFileError> {
        let file = Self::parse(bytes)?;
        file.header.validate_scala_3_9()?;
        Ok(file)
    }

    pub fn encode(&self) -> Result<Vec<u8>, TastyFileError> {
        let mut writer = Writer::new();
        self.header.encode(&mut writer)?;
        self.names.encode(&mut writer)?;
        self.sections.encode(&mut writer)?;
        Ok(writer.into_inner())
    }

    pub fn encode_with_ast_addresses(&self) -> Result<EncodedTastyFile, TastyFileError> {
        let mut writer = Writer::new();
        self.header.encode(&mut writer)?;
        self.names.encode(&mut writer)?;
        let mut ast_addresses = Vec::new();

        for section in self.sections.iter() {
            writer.write_nat(section.name);
            if section.standard_kind(&self.names) == Some(StandardSection::Asts) {
                let mut reader = section.reader();
                let asts = RawNodes::decode(&mut reader)?;
                let encoded_asts = asts.encode_with_addresses()?;
                ast_addresses = encoded_asts.addresses().to_vec();
                writer.write_length_prefixed_bytes(encoded_asts.as_slice())?;
            } else {
                writer.write_length_prefixed_bytes(section.payload)?;
            }
        }

        Ok(EncodedTastyFile {
            bytes: writer.into_inner(),
            ast_addresses,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn names(&self) -> &NameTable {
        &self.names
    }

    pub fn sections(&self) -> &SectionTable<'a> {
        &self.sections
    }

    pub fn section(&self, kind: StandardSection) -> Option<&Section<'a>> {
        self.sections
            .iter()
            .find(|section| section.standard_kind(&self.names) == Some(kind))
    }

    pub fn name(&self, reference: NameRef) -> Option<&RawName> {
        self.names.get(reference)
    }

    pub fn asts(&self) -> Result<RawNodes<'a>, TastyFileError> {
        let section = self
            .section(StandardSection::Asts)
            .ok_or(TastyFileError::MissingSection(StandardSection::Asts))?;
        let mut reader = section.reader();
        let nodes = RawNodes::decode(&mut reader)?;
        debug_assert!(reader.is_at_end());
        Ok(nodes)
    }

    pub fn attributes(&self) -> Result<Option<Vec<Attribute>>, TastyFileError> {
        let attributes = self
            .section(StandardSection::Attributes)
            .map(|section| {
                section
                    .decode_attributes()
                    .map_err(TastyFileError::Sections)
            })
            .transpose()?;

        if let Some(attributes) = &attributes {
            for attribute in attributes {
                if let Attribute::SourceFile(reference) = attribute {
                    if self.name(*reference).is_none() {
                        return Err(TastyFileError::InvalidNameReference {
                            context: "SOURCEFILE attribute",
                            reference: *reference,
                        });
                    }
                }
            }
        }

        Ok(attributes)
    }

    pub fn comments(&self) -> Result<Option<Vec<Comment>>, TastyFileError> {
        let comments = self
            .section(StandardSection::Comments)
            .map(|section| section.decode_comments().map_err(TastyFileError::Sections))
            .transpose()?;

        if let Some(comments) = &comments {
            for comment in comments {
                self.validate_ast_address("comment", i64::from(comment.address))?;
            }
        }

        Ok(comments)
    }

    pub fn positions(&self) -> Result<Option<PositionSection>, TastyFileError> {
        let positions = self
            .section(StandardSection::Positions)
            .map(|section| section.decode_positions().map_err(TastyFileError::Sections))
            .transpose()?;

        if let Some(positions) = &positions {
            let mut address = 0i64;
            for entry in &positions.entries {
                match entry {
                    crate::PositionEntry::Source(reference) => {
                        if self.name(*reference).is_none() {
                            return Err(TastyFileError::InvalidNameReference {
                                context: "SOURCE position",
                                reference: *reference,
                            });
                        }
                    }
                    crate::PositionEntry::Association { address_delta, .. } => {
                        address = address.checked_add(*address_delta).ok_or(
                            TastyFileError::InvalidAstAddress {
                                context: "position association",
                                address: *address_delta,
                                asts_length: self.asts_length(),
                            },
                        )?;
                        self.validate_ast_address("position association", address)?;
                    }
                }
            }
        }

        Ok(positions)
    }

    fn asts_length(&self) -> usize {
        self.section(StandardSection::Asts)
            .map(|section| section.payload.len())
            .unwrap_or(0)
    }

    fn validate_ast_address(
        &self,
        context: &'static str,
        address: i64,
    ) -> Result<(), TastyFileError> {
        if address < 0
            || u64::try_from(address).map_or(true, |value| value >= self.asts_length() as u64)
        {
            return Err(TastyFileError::InvalidAstAddress {
                context,
                address,
                asts_length: self.asts_length(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{TastyFile, TastyFileError};
    use crate::section::StandardSection;

    #[test]
    fn decodes_a_complete_scala_3_9_fixture_as_one_file_model() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();

        assert_eq!(file.header().tooling_version, "Scala 3.9.0");
        assert!(!file.names().is_empty());
        assert!(file.section(StandardSection::Asts).is_some());
        assert!(!file.asts().unwrap().is_empty());
        assert!(file.attributes().unwrap().is_some());
        assert!(file.comments().unwrap().is_some());
        assert!(file.positions().unwrap().is_some());
    }

    #[test]
    fn reports_a_missing_asts_section() {
        let file = TastyFile {
            header: crate::header::Header {
                major_version: 28,
                minor_version: 9,
                experimental_version: 0,
                tooling_version: "Scala 3.9.0".to_owned(),
                uuid: [0; 16],
            },
            names: crate::name_table::NameTable::decode(&mut crate::reader::Reader::new(&[0x80]))
                .unwrap(),
            sections: crate::section::SectionTable::decode(&mut crate::reader::Reader::new(&[]), 0)
                .unwrap(),
        };

        assert_eq!(
            file.asts(),
            Err(TastyFileError::MissingSection(StandardSection::Asts))
        );
    }

    #[test]
    fn builds_and_reparses_a_complete_file_from_parts() {
        let names = crate::NameTable::from_entries(vec![
            crate::RawName::Utf8("ASTs".to_owned()),
            crate::RawName::Utf8("Attributes".to_owned()),
            crate::RawName::Utf8("Comments".to_owned()),
            crate::RawName::Utf8("Positions".to_owned()),
            crate::RawName::Utf8("Example.scala".to_owned()),
        ])
        .unwrap();

        let mut attributes = crate::Writer::new();
        crate::Attribute::encode_all(&[crate::Attribute::SourceFile(5)], &mut attributes).unwrap();
        let mut comments = crate::Writer::new();
        crate::Comment::encode_all(
            &[crate::Comment {
                address: 0,
                text: "example".to_owned(),
                coordinates: 0,
            }],
            &mut comments,
        )
        .unwrap();
        let mut positions = crate::Writer::new();
        crate::PositionSection {
            line_sizes: vec![7],
            entries: vec![crate::PositionEntry::Source(5)],
        }
        .encode(&mut positions)
        .unwrap();

        let sections = crate::SectionTable::from_sections(vec![
            crate::Section::new(0, &[crate::VALDEF_TAG, 0x80]),
            crate::Section::new(1, attributes.as_slice()),
            crate::Section::new(2, comments.as_slice()),
            crate::Section::new(3, positions.as_slice()),
        ]);
        let file = TastyFile::from_parts(
            crate::Header {
                major_version: 28,
                minor_version: 9,
                experimental_version: 0,
                tooling_version: "Scala 3.9.0".to_owned(),
                uuid: [7; 16],
            },
            names,
            sections,
        )
        .unwrap();

        let bytes = file.encode().unwrap();
        let reparsed = TastyFile::parse_scala_3_9(&bytes).unwrap();
        assert_eq!(reparsed.header().uuid, [7; 16]);
        assert_eq!(reparsed.asts().unwrap().len(), 1);
        assert_eq!(reparsed.attributes().unwrap().unwrap().len(), 1);
        assert_eq!(reparsed.comments().unwrap().unwrap().len(), 1);
        assert_eq!(reparsed.positions().unwrap().unwrap().line_sizes, vec![7]);
    }

    #[test]
    fn rejects_a_position_source_reference_outside_the_name_table() {
        let names =
            crate::NameTable::from_entries(vec![crate::RawName::Utf8("Positions".to_owned())])
                .unwrap();
        let sections =
            crate::SectionTable::from_sections(vec![crate::Section::new(0, &[0x80, 0x84, 0x82])]);
        let file = TastyFile::from_parts(
            crate::Header {
                major_version: 28,
                minor_version: 9,
                experimental_version: 0,
                tooling_version: "Scala 3.9.0".to_owned(),
                uuid: [0; 16],
            },
            names,
            sections,
        )
        .unwrap();

        assert_eq!(
            file.positions(),
            Err(TastyFileError::InvalidNameReference {
                context: "SOURCE position",
                reference: 2,
            })
        );
    }

    #[test]
    fn rejects_a_comment_address_outside_the_asts_payload() {
        let names = crate::NameTable::from_entries(vec![
            crate::RawName::Utf8("ASTs".to_owned()),
            crate::RawName::Utf8("Comments".to_owned()),
        ])
        .unwrap();
        let sections = crate::SectionTable::from_sections(vec![
            crate::Section::new(0, &[crate::VALDEF_TAG, 0x80]),
            crate::Section::new(1, &[0x85, 0x80, 0x80]),
        ]);
        let file = TastyFile::from_parts(
            crate::Header {
                major_version: 28,
                minor_version: 9,
                experimental_version: 0,
                tooling_version: "Scala 3.9.0".to_owned(),
                uuid: [0; 16],
            },
            names,
            sections,
        )
        .unwrap();

        assert_eq!(
            file.comments(),
            Err(TastyFileError::InvalidAstAddress {
                context: "comment",
                address: 5,
                asts_length: 2,
            })
        );
    }

    #[test]
    fn rejects_a_section_reference_when_building_from_parts() {
        let sections = crate::SectionTable::from_sections(vec![crate::Section::new(1, &[])]);
        let error = TastyFile::from_parts(
            crate::Header {
                major_version: 28,
                minor_version: 9,
                experimental_version: 0,
                tooling_version: "Scala 3.9.0".to_owned(),
                uuid: [0; 16],
            },
            crate::NameTable::from_entries(vec![crate::RawName::Utf8("ASTs".to_owned())]).unwrap(),
            sections,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            TastyFileError::Sections(crate::SectionError::InvalidNameReference {
                reference: 1,
                name_count: 1,
                ..
            })
        ));
    }
}
