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
        self.section(StandardSection::Comments)
            .map(|section| section.decode_comments().map_err(TastyFileError::Sections))
            .transpose()
    }

    pub fn positions(&self) -> Result<Option<PositionSection>, TastyFileError> {
        self.section(StandardSection::Positions)
            .map(|section| section.decode_positions().map_err(TastyFileError::Sections))
            .transpose()
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
}
