use crate::ast::{
    AstAddressIndex, AstError, AstReference, NameReference, RawNodes, StructuredNode,
};
use crate::header::{Header, HeaderError};
use crate::name_table::{NameRef, NameTable, NameTableError, RawName};
use crate::reader::Reader;
use crate::section::{
    Attribute, Comment, EncodedSection, PositionSection, Section, SectionError, SectionTable,
    StandardSection,
};
use crate::term::{AstRef, AstTreeNode};
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

/// Owns the pieces of a TASTy file while it is being assembled for encoding.
///
/// The parsed [`TastyFile`] type borrows section payloads, which is useful for
/// zero-copy decoding but awkward for programmatic construction. This builder
/// keeps encoded sections owned and creates a short-lived borrowed view only
/// while validating or serializing the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TastyFileBuilder {
    header: Header,
    names: NameTable,
    sections: Vec<EncodedSection>,
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
    InvalidAstNodeAddress {
        context: &'static str,
        address: u32,
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
            Self::InvalidAstNodeAddress { context, address } => write!(
                formatter,
                "{context} contains AST address {address}, which is not the start of a visible AST node"
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

impl TastyFileBuilder {
    pub fn new(header: Header, names: NameTable) -> Self {
        Self {
            header,
            names,
            sections: Vec::new(),
        }
    }

    /// Add a section and return the builder for fluent construction.
    pub fn with_section(mut self, section: EncodedSection) -> Self {
        self.sections.push(section);
        self
    }

    pub fn push_section(&mut self, section: EncodedSection) {
        self.sections.push(section);
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn names(&self) -> &NameTable {
        &self.names
    }

    pub fn sections(&self) -> &[EncodedSection] {
        &self.sections
    }

    /// Build a borrowed file view after validating section-name references.
    pub fn build(&self) -> Result<TastyFile<'_>, TastyFileError> {
        let sections = SectionTable::from_sections(
            self.sections.iter().map(EncodedSection::section).collect(),
        );
        TastyFile::from_parts(self.header.clone(), self.names.clone(), sections)
    }

    /// Validate all supported file contents without serializing them.
    pub fn validate(&self) -> Result<(), TastyFileError> {
        self.build()?.validate()
    }

    /// Validate the file contents and require the Scala 3.9.0 format version.
    pub fn validate_scala_3_9(&self) -> Result<(), TastyFileError> {
        let file = self.build()?;
        file.header.validate_scala_3_9()?;
        file.validate()
    }

    /// Validate the file against a compiler version using TASTy's
    /// compatibility relation.
    pub fn validate_compatible_with(
        &self,
        compiler_major: u32,
        compiler_minor: u32,
        compiler_experimental: u32,
    ) -> Result<(), TastyFileError> {
        let file = self.build()?;
        file.validate_compatible_with(compiler_major, compiler_minor, compiler_experimental)
    }

    pub fn encode(&self) -> Result<Vec<u8>, TastyFileError> {
        self.build()?.encode()
    }

    pub fn encode_with_ast_addresses(&self) -> Result<EncodedTastyFile, TastyFileError> {
        self.build()?.encode_with_ast_addresses()
    }

    /// Validate all supported file contents before encoding.
    pub fn encode_validated(&self) -> Result<Vec<u8>, TastyFileError> {
        self.build()?.encode_validated()
    }

    /// Validate all supported file contents against a compiler version before
    /// encoding.
    pub fn encode_validated_compatible_with(
        &self,
        compiler_major: u32,
        compiler_minor: u32,
        compiler_experimental: u32,
    ) -> Result<Vec<u8>, TastyFileError> {
        let file = self.build()?;
        file.validate_compatible_with(compiler_major, compiler_minor, compiler_experimental)?;
        file.encode()
    }

    /// Validate all supported file contents before encoding and allocating AST addresses.
    pub fn encode_validated_with_ast_addresses(&self) -> Result<EncodedTastyFile, TastyFileError> {
        self.build()?.encode_validated_with_ast_addresses()
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

    /// Parse a file after validating its version against a compiler version.
    /// The original file version remains available through [`TastyFile::header`].
    pub fn parse_compatible_with(
        bytes: &'a [u8],
        compiler_major: u32,
        compiler_minor: u32,
        compiler_experimental: u32,
    ) -> Result<Self, TastyFileError> {
        let file = Self::parse(bytes)?;
        file.header.validate_compatible_with(
            compiler_major,
            compiler_minor,
            compiler_experimental,
        )?;
        Ok(file)
    }

    /// Parse a Scala 3.9.0 file and eagerly validate all supported sections.
    pub fn parse_and_validate_scala_3_9(bytes: &'a [u8]) -> Result<Self, TastyFileError> {
        let file = Self::parse_scala_3_9(bytes)?;
        file.validate()?;
        Ok(file)
    }

    /// Validate the ASTs and every supported standard section in this file.
    ///
    /// Parsing keeps section payloads lazy so callers can inspect only the
    /// parts they need. This method provides an explicit eager-validation
    /// boundary for applications that need the complete file checked.
    pub fn validate(&self) -> Result<(), TastyFileError> {
        self.validate_name_references()?;
        self.validate_ast_reference_targets()?;
        self.attributes()?;
        self.comments()?;
        self.positions()?;
        Ok(())
    }

    /// Validate the file contents and its version against a compiler version.
    pub fn validate_compatible_with(
        &self,
        compiler_major: u32,
        compiler_minor: u32,
        compiler_experimental: u32,
    ) -> Result<(), TastyFileError> {
        self.header.validate_compatible_with(
            compiler_major,
            compiler_minor,
            compiler_experimental,
        )?;
        self.validate()
    }

    /// Validate that every name-table reference used by a supported AST
    /// payload resolves to an entry in this file's name table.
    pub fn validate_name_references(&self) -> Result<(), TastyFileError> {
        for reference in self.name_references()? {
            if self.name(reference.reference).is_none() {
                return Err(TastyFileError::InvalidNameReference {
                    context: "AST name reference",
                    reference: reference.reference,
                });
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, TastyFileError> {
        let mut writer = Writer::new();
        self.header.encode(&mut writer)?;
        self.names.encode(&mut writer)?;
        self.sections.encode(&mut writer)?;
        Ok(writer.into_inner())
    }

    /// Validate all supported file contents before encoding.
    pub fn encode_validated(&self) -> Result<Vec<u8>, TastyFileError> {
        self.validate()?;
        self.encode()
    }

    /// Validate the file contents and its version against a compiler version
    /// before encoding.
    pub fn encode_validated_compatible_with(
        &self,
        compiler_major: u32,
        compiler_minor: u32,
        compiler_experimental: u32,
    ) -> Result<Vec<u8>, TastyFileError> {
        self.validate_compatible_with(compiler_major, compiler_minor, compiler_experimental)?;
        self.encode()
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

    /// Validate all supported file contents before encoding and allocating AST addresses.
    pub fn encode_validated_with_ast_addresses(&self) -> Result<EncodedTastyFile, TastyFileError> {
        self.validate()?;
        self.encode_with_ast_addresses()
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

    pub fn structured_asts(&self) -> Result<Vec<StructuredNode<'a>>, TastyFileError> {
        Ok(self
            .asts()?
            .iter()
            .map(|node| node.decode_structured())
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn ast_at(&self, address: u32) -> Result<Option<crate::RawNode<'a>>, TastyFileError> {
        Ok(self.asts()?.address_index().get(address).cloned())
    }

    /// Index every visible AST node in the ASTs section, including nodes
    /// nested inside expression, type, package, and template payloads.
    pub fn ast_address_index(&self) -> Result<AstAddressIndex<'a>, TastyFileError> {
        let section = self
            .section(StandardSection::Asts)
            .ok_or(TastyFileError::MissingSection(StandardSection::Asts))?;
        let mut reader = section.reader();
        let nodes = RawNodes::decode(&mut reader)?;
        Ok(nodes.deep_address_index_with_source(section.payload)?)
    }

    /// Resolve an AST reference against the global AST node index.
    ///
    /// `None` means that the address is inside the ASTs section but does not
    /// identify the start of a visible AST node.
    pub fn resolve_ast_reference(
        &self,
        reference: AstRef,
    ) -> Result<Option<AstTreeNode>, TastyFileError> {
        Ok(self.ast_address_index()?.resolve_node(reference))
    }

    /// Collect AST references from every top-level node in wire order.
    ///
    /// The owner address identifies the top-level node containing the
    /// reference. The target can point to a nested AST node; resolving that
    /// target is intentionally separate from this collection step.
    pub fn ast_references(&self) -> Result<Vec<AstReference>, TastyFileError> {
        let mut references = Vec::new();
        for node in self.asts()?.iter() {
            for reference in node.ast_refs()? {
                references.push(AstReference {
                    owner_address: node.offset as u32,
                    reference,
                });
            }
        }
        Ok(references)
    }

    /// Collects name-table references from every top-level AST node in wire
    /// order. The owner address identifies the top-level node containing the
    /// reference.
    pub fn name_references(&self) -> Result<Vec<NameReference>, TastyFileError> {
        let mut references = Vec::new();
        for node in self.asts()?.iter() {
            for reference in node.name_refs()? {
                references.push(NameReference {
                    owner_address: node.offset as u32,
                    reference,
                });
            }
        }
        Ok(references)
    }

    /// Validate that every collected AST reference points inside the ASTs
    /// section payload.
    ///
    /// This checks the address range only. Use
    /// [`TastyFile::validate_ast_reference_targets`] when references must also
    /// resolve to visible node starts.
    pub fn validate_ast_references(&self) -> Result<(), TastyFileError> {
        for reference in self.ast_references()? {
            self.validate_ast_address("AST reference", i64::from(reference.reference.address))?;
        }
        Ok(())
    }

    /// Validate that every collected AST reference points to the start of a
    /// visible AST node, not merely somewhere inside the ASTs section.
    pub fn validate_ast_reference_targets(&self) -> Result<(), TastyFileError> {
        let index = self.ast_address_index()?;
        for reference in self.ast_references()? {
            self.validate_ast_address("AST reference", i64::from(reference.reference.address))?;
            if index.resolve_node(reference.reference).is_none() {
                return Err(TastyFileError::InvalidAstNodeAddress {
                    context: "AST reference",
                    address: reference.reference.address,
                });
            }
        }
        Ok(())
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
    use crate::ast::StructuredNode;
    use crate::header::Header;
    use crate::name_table::NameTable;
    use crate::section::{EncodedSection, PositionEntry, PositionSection, StandardSection};

    fn compatible_file_bytes() -> Vec<u8> {
        super::TastyFileBuilder::new(
            Header {
                major_version: 28,
                minor_version: 8,
                experimental_version: 0,
                tooling_version: "Scala 3.8.0".to_owned(),
                uuid: [0; 16],
            },
            NameTable::from_entries(vec![
                crate::RawName::Utf8("ASTs".to_owned()),
                crate::RawName::Utf8("value".to_owned()),
            ])
            .unwrap(),
        )
        .with_section(EncodedSection::raw(0, [crate::VALDEF_TAG, 0x82, 0x82, 3]))
        .encode()
        .unwrap()
    }

    #[test]
    fn decodes_a_complete_scala_3_9_fixture_as_one_file_model() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_and_validate_scala_3_9(bytes).unwrap();

        assert_eq!(file.header().tooling_version, "Scala 3.9.0");
        assert!(!file.names().is_empty());
        assert!(file.section(StandardSection::Asts).is_some());
        assert!(!file.asts().unwrap().is_empty());
        assert!(file.attributes().unwrap().is_some());
        assert!(file.comments().unwrap().is_some());
        assert!(file.positions().unwrap().is_some());
    }

    #[test]
    fn decodes_fixture_asts_through_the_structured_file_api() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_and_validate_scala_3_9(bytes).unwrap();
        let nodes = file.structured_asts().unwrap();

        assert!(!nodes.is_empty());
        assert!(matches!(nodes.first(), Some(StructuredNode::Package(_))));
    }

    #[test]
    fn parses_a_file_with_an_older_compatible_minor_version() {
        let bytes = compatible_file_bytes();
        let file = TastyFile::parse_compatible_with(&bytes, 28, 9, 0).unwrap();

        assert_eq!(file.header().minor_version, 8);
    }

    #[test]
    fn rejects_a_file_with_an_incompatible_version_during_compatible_parse() {
        let bytes = compatible_file_bytes();

        assert_eq!(
            TastyFile::parse_compatible_with(&bytes, 27, 9, 0),
            Err(TastyFileError::Header(
                crate::HeaderError::IncompatibleVersion {
                    major: 28,
                    minor: 8,
                    experimental: 0,
                    compiler_major: 27,
                    compiler_minor: 9,
                    compiler_experimental: 0,
                }
            ))
        );
    }

    #[test]
    fn validates_a_fixture_against_a_newer_compatible_minor_version() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse(bytes).unwrap();

        assert_eq!(file.validate_compatible_with(28, 10, 0), Ok(()));
    }

    #[test]
    fn validates_a_complete_file_from_parts() {
        let names = crate::NameTable::from_entries(vec![
            crate::RawName::Utf8("ASTs".to_owned()),
            crate::RawName::Utf8("value".to_owned()),
        ])
        .unwrap();
        let sections = crate::SectionTable::from_sections(vec![crate::Section::new(
            0,
            &[crate::VALDEF_TAG, 0x82, 0x82, 3],
        )]);
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

        assert_eq!(file.validate(), Ok(()));
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
    fn builds_and_reparses_a_file_from_owned_sections() {
        let names = crate::NameTable::from_entries(vec![
            crate::RawName::Utf8("ASTs".to_owned()),
            crate::RawName::Utf8("Attributes".to_owned()),
            crate::RawName::Utf8("Comments".to_owned()),
            crate::RawName::Utf8("Positions".to_owned()),
            crate::RawName::Utf8("Example.scala".to_owned()),
        ])
        .unwrap();
        let builder = super::TastyFileBuilder::new(
            crate::Header {
                major_version: 28,
                minor_version: 9,
                experimental_version: 0,
                tooling_version: "Scala 3.9.0".to_owned(),
                uuid: [8; 16],
            },
            names,
        )
        .with_section(EncodedSection::raw(0, [crate::VALDEF_TAG, 0x82, 0x82, 3]))
        .with_section(EncodedSection::attributes(1, &[crate::Attribute::SourceFile(5)]).unwrap())
        .with_section(
            EncodedSection::comments(
                2,
                &[crate::Comment {
                    text: "example".to_owned(),
                    coordinates: 0,
                }],
            )
            .unwrap(),
        )
        .with_section(
            EncodedSection::positions(
                3,
                &PositionSection {
                    line_sizes: vec![7],
                    entries: vec![PositionEntry::Source(5)],
                },
            )
            .unwrap(),
        );

        assert_eq!(builder.validate(), Ok(()));
        let bytes = builder.encode_validated().unwrap();
        let reparsed = TastyFile::parse_and_validate_scala_3_9(&bytes).unwrap();

        assert_eq!(reparsed.header().uuid, [8; 16]);
        assert_eq!(reparsed.asts().unwrap().len(), 1);
        assert_eq!(reparsed.attributes().unwrap().unwrap().len(), 1);
        assert_eq!(reparsed.comments().unwrap().unwrap().len(), 1);
        assert_eq!(reparsed.positions().unwrap().unwrap().line_sizes, vec![7]);
    }

    #[test]
    fn encodes_a_file_after_compatible_version_validation() {
        let bytes = compatible_file_bytes();
        let file = TastyFile::parse(&bytes).unwrap();

        assert_eq!(
            file.encode_validated_compatible_with(28, 9, 0).unwrap(),
            bytes
        );
    }

    #[test]
    fn validates_and_encodes_owned_sections_with_an_older_compatible_version() {
        let names = NameTable::from_entries(vec![
            crate::RawName::Utf8("ASTs".to_owned()),
            crate::RawName::Utf8("value".to_owned()),
        ])
        .unwrap();
        let builder = super::TastyFileBuilder::new(
            Header {
                major_version: 28,
                minor_version: 8,
                experimental_version: 0,
                tooling_version: "Scala 3.8.0".to_owned(),
                uuid: [0; 16],
            },
            names,
        )
        .with_section(EncodedSection::raw(0, [crate::VALDEF_TAG, 0x82, 0x82, 3]));

        assert_eq!(builder.validate_compatible_with(28, 9, 0), Ok(()));
        assert_eq!(
            builder.encode_validated_compatible_with(28, 9, 0),
            Ok(compatible_file_bytes())
        );
    }

    #[test]
    fn exposes_ast_addresses_when_encoding_from_owned_sections() {
        let names = crate::NameTable::from_entries(vec![
            crate::RawName::Utf8("ASTs".to_owned()),
            crate::RawName::Utf8("value".to_owned()),
        ])
        .unwrap();
        let builder = super::TastyFileBuilder::new(
            crate::Header {
                major_version: 28,
                minor_version: 9,
                experimental_version: 0,
                tooling_version: "Scala 3.9.0".to_owned(),
                uuid: [0; 16],
            },
            names,
        )
        .with_section(EncodedSection::raw(0, [crate::VALDEF_TAG, 0x82, 0x82, 3]));

        let encoded = builder.encode_validated_with_ast_addresses().unwrap();

        assert_eq!(encoded.ast_addresses(), &[0]);
        assert_eq!(
            TastyFile::parse(encoded.as_slice())
                .unwrap()
                .asts()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn rejects_invalid_section_names_when_encoding_from_owned_sections() {
        let builder = super::TastyFileBuilder::new(
            crate::Header {
                major_version: 28,
                minor_version: 9,
                experimental_version: 0,
                tooling_version: "Scala 3.9.0".to_owned(),
                uuid: [0; 16],
            },
            crate::NameTable::from_entries(vec![crate::RawName::Utf8("ASTs".to_owned())]).unwrap(),
        )
        .with_section(EncodedSection::raw(1, []));

        assert!(matches!(
            builder.encode(),
            Err(TastyFileError::Sections(
                crate::SectionError::InvalidNameReference {
                    reference: 1,
                    name_count: 1,
                    ..
                }
            ))
        ));
    }

    #[test]
    fn rejects_a_non_scala_3_9_version_during_builder_validation() {
        let builder = super::TastyFileBuilder::new(
            crate::Header {
                major_version: 29,
                minor_version: 0,
                experimental_version: 0,
                tooling_version: "future compiler".to_owned(),
                uuid: [0; 16],
            },
            crate::NameTable::from_entries(vec![
                crate::RawName::Utf8("ASTs".to_owned()),
                crate::RawName::Utf8("value".to_owned()),
            ])
            .unwrap(),
        )
        .with_section(EncodedSection::raw(0, [crate::VALDEF_TAG, 0x82, 0x82, 3]));

        assert_eq!(
            builder.validate_scala_3_9(),
            Err(TastyFileError::Header(
                crate::HeaderError::UnsupportedVersion {
                    major: 29,
                    minor: 0,
                    experimental: 0,
                }
            ))
        );
    }

    #[test]
    fn validated_encoders_preserve_a_complete_fixture() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();

        assert_eq!(file.encode_validated().unwrap(), bytes);
        assert_eq!(
            file.encode_validated_with_ast_addresses()
                .unwrap()
                .as_slice(),
            bytes
        );
    }

    #[test]
    fn rejects_invalid_name_references_during_validated_encoding() {
        let names =
            crate::NameTable::from_entries(vec![crate::RawName::Utf8("ASTs".to_owned())]).unwrap();
        let sections = crate::SectionTable::from_sections(vec![crate::Section::new(
            0,
            &[crate::APPLY_TAG, 0x82, crate::TERMREFPKG_TAG, 0x83],
        )]);
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

        assert!(file.encode().is_ok());
        assert_eq!(
            file.encode_validated(),
            Err(TastyFileError::InvalidNameReference {
                context: "AST name reference",
                reference: 3,
            })
        );
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

    #[test]
    fn looks_up_a_top_level_ast_node_by_address() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();
        let first = file.asts().unwrap().get(0).unwrap().clone();

        assert_eq!(file.ast_at(first.offset as u32).unwrap(), Some(first));
        assert_eq!(file.ast_at(1).unwrap(), None);
    }

    #[test]
    fn collects_file_references_with_their_top_level_owner_address() {
        let names =
            crate::NameTable::from_entries(vec![crate::RawName::Utf8("ASTs".to_owned())]).unwrap();
        let sections = crate::SectionTable::from_sections(vec![crate::Section::new(
            0,
            &[
                crate::APPLY_TAG,
                0x84,
                crate::TERMREFDIRECT_TAG,
                0x85,
                crate::SHAREDTYPE_TAG,
                0x83,
            ],
        )]);
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
            file.ast_references().unwrap(),
            vec![
                crate::AstReference {
                    owner_address: 0,
                    reference: crate::AstRef {
                        kind: crate::AstRefKind::TermRefDirect,
                        address: 5,
                    },
                },
                crate::AstReference {
                    owner_address: 0,
                    reference: crate::AstRef {
                        kind: crate::AstRefKind::SharedType,
                        address: 3,
                    },
                },
            ]
        );
    }

    #[test]
    fn rejects_a_file_ast_reference_outside_the_asts_payload() {
        let names =
            crate::NameTable::from_entries(vec![crate::RawName::Utf8("ASTs".to_owned())]).unwrap();
        let sections = crate::SectionTable::from_sections(vec![crate::Section::new(
            0,
            &[crate::APPLY_TAG, 0x82, crate::TERMREFDIRECT_TAG, 0xff],
        )]);
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
            file.validate_ast_references(),
            Err(TastyFileError::InvalidAstAddress {
                context: "AST reference",
                address: 127,
                asts_length: 4,
            })
        );
    }

    #[test]
    fn rejects_a_file_ast_name_reference_outside_the_name_table() {
        let names =
            crate::NameTable::from_entries(vec![crate::RawName::Utf8("ASTs".to_owned())]).unwrap();
        let sections = crate::SectionTable::from_sections(vec![crate::Section::new(
            0,
            &[crate::APPLY_TAG, 0x82, crate::TERMREFPKG_TAG, 0x83],
        )]);
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
            file.validate_name_references(),
            Err(TastyFileError::InvalidNameReference {
                context: "AST name reference",
                reference: 3,
            })
        );
        assert_eq!(
            file.validate(),
            Err(TastyFileError::InvalidNameReference {
                context: "AST name reference",
                reference: 3,
            })
        );
    }

    #[test]
    fn rejects_a_file_ast_reference_that_is_not_a_node_start() {
        let names =
            crate::NameTable::from_entries(vec![crate::RawName::Utf8("ASTs".to_owned())]).unwrap();
        let sections = crate::SectionTable::from_sections(vec![crate::Section::new(
            0,
            &[crate::APPLY_TAG, 0x82, crate::TERMREFDIRECT_TAG, 0x81],
        )]);
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
            file.validate_ast_reference_targets(),
            Err(TastyFileError::InvalidAstNodeAddress {
                context: "AST reference",
                address: 1,
            })
        );
        assert_eq!(
            file.validate(),
            Err(TastyFileError::InvalidAstNodeAddress {
                context: "AST reference",
                address: 1,
            })
        );
    }
}
