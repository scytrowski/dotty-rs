use crate::ast::{
    AstAddressIndex, AstError, AstReference, NameReference, RawNodes, StructuredNode,
};
use crate::header::{Header, HeaderError};
use crate::name_table::{
    NameRef, NameRenderError, NameTable, NameTableError, RawName, RawNameKind, RenderedSignedName,
};
use crate::reader::Reader;
use crate::section::{
    Attribute, Comment, EncodedSection, PositionSection, ResolvedPosition, Section, SectionError,
    SectionTable, StandardSection,
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

/// A visible AST node paired with its resolved source span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AstTreePosition {
    pub node: AstTreeNode,
    pub position: ResolvedPosition,
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
    InvalidSourceFileName {
        reference: NameRef,
        kind: RawNameKind,
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
            Self::InvalidSourceFileName { reference, kind } => write!(
                formatter,
                "SOURCEFILE attribute reference {reference} resolves to {kind:?}, expected a UTF-8 name"
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
        self.validate_with_max_ast_index_depth(crate::DEFAULT_MAX_AST_INDEX_DEPTH)
    }

    /// Validate all supported file contents with an explicit AST nesting
    /// limit.
    pub fn validate_with_max_ast_index_depth(
        &self,
        max_depth: usize,
    ) -> Result<(), TastyFileError> {
        self.build()?.validate_with_max_ast_index_depth(max_depth)
    }

    /// Validate the file contents and require the Scala 3.9.0 format version.
    pub fn validate_scala_3_9(&self) -> Result<(), TastyFileError> {
        self.validate_scala_3_9_with_max_ast_index_depth(crate::DEFAULT_MAX_AST_INDEX_DEPTH)
    }

    /// Validate the Scala 3.9.0 file contents with an explicit AST nesting
    /// limit.
    pub fn validate_scala_3_9_with_max_ast_index_depth(
        &self,
        max_depth: usize,
    ) -> Result<(), TastyFileError> {
        let file = self.build()?;
        file.header.validate_scala_3_9()?;
        file.validate_with_max_ast_index_depth(max_depth)
    }

    /// Validate the file against a compiler version using TASTy's
    /// compatibility relation.
    pub fn validate_compatible_with(
        &self,
        compiler_major: u32,
        compiler_minor: u32,
        compiler_experimental: u32,
    ) -> Result<(), TastyFileError> {
        self.validate_compatible_with_max_ast_index_depth(
            compiler_major,
            compiler_minor,
            compiler_experimental,
            crate::DEFAULT_MAX_AST_INDEX_DEPTH,
        )
    }

    /// Validate the file against a compiler version and with an explicit AST
    /// nesting limit.
    pub fn validate_compatible_with_max_ast_index_depth(
        &self,
        compiler_major: u32,
        compiler_minor: u32,
        compiler_experimental: u32,
        max_depth: usize,
    ) -> Result<(), TastyFileError> {
        let file = self.build()?;
        file.validate_compatible_with_max_ast_index_depth(
            compiler_major,
            compiler_minor,
            compiler_experimental,
            max_depth,
        )
    }

    pub fn encode(&self) -> Result<Vec<u8>, TastyFileError> {
        self.build()?.encode()
    }

    pub fn encode_with_ast_addresses(&self) -> Result<EncodedTastyFile, TastyFileError> {
        self.build()?.encode_with_ast_addresses()
    }

    /// Validate all supported file contents before encoding.
    pub fn encode_validated(&self) -> Result<Vec<u8>, TastyFileError> {
        self.encode_validated_with_max_ast_index_depth(crate::DEFAULT_MAX_AST_INDEX_DEPTH)
    }

    /// Validate all supported file contents before encoding with an explicit
    /// AST nesting limit.
    pub fn encode_validated_with_max_ast_index_depth(
        &self,
        max_depth: usize,
    ) -> Result<Vec<u8>, TastyFileError> {
        let file = self.build()?;
        file.encode_validated_with_max_ast_index_depth(max_depth)
    }

    /// Validate all supported file contents against a compiler version before
    /// encoding.
    pub fn encode_validated_compatible_with(
        &self,
        compiler_major: u32,
        compiler_minor: u32,
        compiler_experimental: u32,
    ) -> Result<Vec<u8>, TastyFileError> {
        self.encode_validated_compatible_with_max_ast_index_depth(
            compiler_major,
            compiler_minor,
            compiler_experimental,
            crate::DEFAULT_MAX_AST_INDEX_DEPTH,
        )
    }

    /// Validate compiler compatibility and file contents before encoding with
    /// an explicit AST nesting limit.
    pub fn encode_validated_compatible_with_max_ast_index_depth(
        &self,
        compiler_major: u32,
        compiler_minor: u32,
        compiler_experimental: u32,
        max_depth: usize,
    ) -> Result<Vec<u8>, TastyFileError> {
        let file = self.build()?;
        file.encode_validated_compatible_with_max_ast_index_depth(
            compiler_major,
            compiler_minor,
            compiler_experimental,
            max_depth,
        )
    }

    /// Validate all supported file contents before encoding and allocating AST addresses.
    pub fn encode_validated_with_ast_addresses(&self) -> Result<EncodedTastyFile, TastyFileError> {
        self.encode_validated_with_max_ast_index_depth_and_ast_addresses(
            crate::DEFAULT_MAX_AST_INDEX_DEPTH,
        )
    }

    /// Validate all supported file contents before encoding and allocating AST
    /// addresses with an explicit AST nesting limit.
    pub fn encode_validated_with_max_ast_index_depth_and_ast_addresses(
        &self,
        max_depth: usize,
    ) -> Result<EncodedTastyFile, TastyFileError> {
        let file = self.build()?;
        file.encode_validated_with_max_ast_index_depth_and_ast_addresses(max_depth)
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
        Self::parse_and_validate_scala_3_9_with_max_ast_index_depth(
            bytes,
            crate::DEFAULT_MAX_AST_INDEX_DEPTH,
        )
    }

    /// Parse and eagerly validate a Scala 3.9.0 file with an explicit AST
    /// nesting limit.
    pub fn parse_and_validate_scala_3_9_with_max_ast_index_depth(
        bytes: &'a [u8],
        max_depth: usize,
    ) -> Result<Self, TastyFileError> {
        let file = Self::parse_scala_3_9(bytes)?;
        file.validate_with_max_ast_index_depth(max_depth)?;
        Ok(file)
    }

    /// Parse a file, validate its version against a compiler version, and
    /// eagerly validate all supported file contents.
    pub fn parse_and_validate_compatible_with(
        bytes: &'a [u8],
        compiler_major: u32,
        compiler_minor: u32,
        compiler_experimental: u32,
    ) -> Result<Self, TastyFileError> {
        Self::parse_and_validate_compatible_with_max_ast_index_depth(
            bytes,
            compiler_major,
            compiler_minor,
            compiler_experimental,
            crate::DEFAULT_MAX_AST_INDEX_DEPTH,
        )
    }

    /// Parse, validate compiler compatibility, and eagerly validate a file
    /// with an explicit AST nesting limit.
    pub fn parse_and_validate_compatible_with_max_ast_index_depth(
        bytes: &'a [u8],
        compiler_major: u32,
        compiler_minor: u32,
        compiler_experimental: u32,
        max_depth: usize,
    ) -> Result<Self, TastyFileError> {
        let file = Self::parse(bytes)?;
        file.validate_compatible_with_max_ast_index_depth(
            compiler_major,
            compiler_minor,
            compiler_experimental,
            max_depth,
        )?;
        Ok(file)
    }

    /// Validate the ASTs and every supported standard section in this file.
    ///
    /// Parsing keeps section payloads lazy so callers can inspect only the
    /// parts they need. This method provides an explicit eager-validation
    /// boundary for applications that need the complete file checked.
    pub fn validate(&self) -> Result<(), TastyFileError> {
        self.validate_with_max_ast_index_depth(crate::DEFAULT_MAX_AST_INDEX_DEPTH)
    }

    /// Validate the file using an explicit maximum AST nesting depth.
    ///
    /// This is useful when eager validation is performed on input whose size
    /// or nesting depth is controlled by an untrusted source.
    pub fn validate_with_max_ast_index_depth(
        &self,
        max_depth: usize,
    ) -> Result<(), TastyFileError> {
        self.validate_name_references()?;
        self.validate_ast_reference_targets_with_max_depth(max_depth)?;
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
        self.validate_compatible_with_max_ast_index_depth(
            compiler_major,
            compiler_minor,
            compiler_experimental,
            crate::DEFAULT_MAX_AST_INDEX_DEPTH,
        )
    }

    /// Validate compiler compatibility and file contents with an explicit AST
    /// nesting limit.
    pub fn validate_compatible_with_max_ast_index_depth(
        &self,
        compiler_major: u32,
        compiler_minor: u32,
        compiler_experimental: u32,
        max_depth: usize,
    ) -> Result<(), TastyFileError> {
        self.header.validate_compatible_with(
            compiler_major,
            compiler_minor,
            compiler_experimental,
        )?;
        self.validate_with_max_ast_index_depth(max_depth)
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
        self.encode_validated_with_max_ast_index_depth(crate::DEFAULT_MAX_AST_INDEX_DEPTH)
    }

    /// Validate all supported file contents before encoding with an explicit
    /// AST nesting limit.
    pub fn encode_validated_with_max_ast_index_depth(
        &self,
        max_depth: usize,
    ) -> Result<Vec<u8>, TastyFileError> {
        self.validate_with_max_ast_index_depth(max_depth)?;
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
        self.encode_validated_compatible_with_max_ast_index_depth(
            compiler_major,
            compiler_minor,
            compiler_experimental,
            crate::DEFAULT_MAX_AST_INDEX_DEPTH,
        )
    }

    /// Validate compiler compatibility and file contents before encoding with
    /// an explicit AST nesting limit.
    pub fn encode_validated_compatible_with_max_ast_index_depth(
        &self,
        compiler_major: u32,
        compiler_minor: u32,
        compiler_experimental: u32,
        max_depth: usize,
    ) -> Result<Vec<u8>, TastyFileError> {
        self.validate_compatible_with_max_ast_index_depth(
            compiler_major,
            compiler_minor,
            compiler_experimental,
            max_depth,
        )?;
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
        self.encode_validated_with_max_ast_index_depth_and_ast_addresses(
            crate::DEFAULT_MAX_AST_INDEX_DEPTH,
        )
    }

    /// Validate all supported file contents before encoding and allocating AST
    /// addresses with an explicit AST nesting limit.
    pub fn encode_validated_with_max_ast_index_depth_and_ast_addresses(
        &self,
        max_depth: usize,
    ) -> Result<EncodedTastyFile, TastyFileError> {
        self.validate_with_max_ast_index_depth(max_depth)?;
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

    /// Render a non-signature name using the conventional Scala spelling.
    ///
    /// This is a convenience delegation to [`NameTable::render`]. Signature
    /// bearing and unknown names return [`NameRenderError::Unsupported`].
    pub fn render_name(&self, reference: NameRef) -> Result<String, NameRenderError> {
        self.names.render(reference)
    }

    /// Resolve a signature-bearing name through the file's name table.
    ///
    /// This is a convenience delegation to [`NameTable::render_signed_name`].
    /// Non-signature names return `Ok(None)`.
    pub fn render_signed_name(
        &self,
        reference: NameRef,
    ) -> Result<Option<RenderedSignedName>, NameRenderError> {
        self.names.render_signed_name(reference)
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
        self.ast_address_index_with_max_depth(crate::DEFAULT_MAX_AST_INDEX_DEPTH)
    }

    /// Index every visible AST node using an explicit nesting limit.
    ///
    /// The limit applies to category-one through category-five nodes reached
    /// while traversing the ASTs section. A zero limit rejects even the first
    /// top-level node.
    pub fn ast_address_index_with_max_depth(
        &self,
        max_depth: usize,
    ) -> Result<AstAddressIndex<'a>, TastyFileError> {
        let section = self
            .section(StandardSection::Asts)
            .ok_or(TastyFileError::MissingSection(StandardSection::Asts))?;
        let mut reader = section.reader();
        let nodes = RawNodes::decode(&mut reader)?;
        Ok(nodes.deep_address_index_with_source_and_max_depth(section.payload, max_depth)?)
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

    /// Collect visible AST nodes with `tag` in absolute address order.
    ///
    /// The query includes nested nodes and is independent of the semantic
    /// Scala model. An unknown tag returns an empty vector.
    pub fn ast_nodes_with_tag(&self, tag: u8) -> Result<Vec<AstTreeNode>, TastyFileError> {
        self.ast_nodes_with_tag_with_max_depth(tag, crate::DEFAULT_MAX_AST_INDEX_DEPTH)
    }

    /// Collect visible AST nodes with `tag` using an explicit nesting limit.
    ///
    /// The limit has the same meaning as
    /// [`TastyFile::ast_address_index_with_max_depth`].
    pub fn ast_nodes_with_tag_with_max_depth(
        &self,
        tag: u8,
        max_depth: usize,
    ) -> Result<Vec<AstTreeNode>, TastyFileError> {
        Ok(self
            .ast_address_index_with_max_depth(max_depth)?
            .iter_nodes_with_tag(tag)
            .collect())
    }

    /// Collect visible AST nodes whose absolute addresses are in
    /// `[start, end)`, retaining address order.
    pub fn ast_nodes_in_address_range(
        &self,
        start: u32,
        end: u32,
    ) -> Result<Vec<AstTreeNode>, TastyFileError> {
        self.ast_nodes_in_address_range_with_max_depth(
            start,
            end,
            crate::DEFAULT_MAX_AST_INDEX_DEPTH,
        )
    }

    /// Collect visible AST nodes in `[start, end)` using an explicit nesting
    /// limit.
    pub fn ast_nodes_in_address_range_with_max_depth(
        &self,
        start: u32,
        end: u32,
        max_depth: usize,
    ) -> Result<Vec<AstTreeNode>, TastyFileError> {
        Ok(self
            .ast_address_index_with_max_depth(max_depth)?
            .iter_nodes_in_address_range(start, end)
            .collect())
    }

    /// Return the structural parent of a visible AST node.
    pub fn ast_parent_of(&self, child_address: u32) -> Result<Option<AstTreeNode>, TastyFileError> {
        self.ast_parent_of_with_max_depth(child_address, crate::DEFAULT_MAX_AST_INDEX_DEPTH)
    }

    /// Return the structural parent using an explicit AST traversal limit.
    pub fn ast_parent_of_with_max_depth(
        &self,
        child_address: u32,
        max_depth: usize,
    ) -> Result<Option<AstTreeNode>, TastyFileError> {
        Ok(self
            .ast_address_index_with_max_depth(max_depth)?
            .parent_of(child_address))
    }

    /// Collect the direct children of a visible AST node in wire order.
    pub fn ast_children_of(&self, parent_address: u32) -> Result<Vec<AstTreeNode>, TastyFileError> {
        self.ast_children_of_with_max_depth(parent_address, crate::DEFAULT_MAX_AST_INDEX_DEPTH)
    }

    /// Collect direct children using an explicit AST traversal limit.
    pub fn ast_children_of_with_max_depth(
        &self,
        parent_address: u32,
        max_depth: usize,
    ) -> Result<Vec<AstTreeNode>, TastyFileError> {
        Ok(self
            .ast_address_index_with_max_depth(max_depth)?
            .children_of(parent_address)
            .collect())
    }

    /// Collect all structural AST parent-to-child edges in traversal order.
    pub fn ast_tree_edges(&self) -> Result<Vec<crate::AstTreeEdge>, TastyFileError> {
        self.ast_tree_edges_with_max_depth(crate::DEFAULT_MAX_AST_INDEX_DEPTH)
    }

    /// Collect structural AST edges using an explicit traversal limit.
    pub fn ast_tree_edges_with_max_depth(
        &self,
        max_depth: usize,
    ) -> Result<Vec<crate::AstTreeEdge>, TastyFileError> {
        Ok(self
            .ast_address_index_with_max_depth(max_depth)?
            .iter_tree_edges()
            .collect())
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

    /// Collect AST references owned by the top-level node at `owner_address`.
    ///
    /// The returned references retain their original wire order. An address
    /// without references produces an empty vector.
    pub fn ast_references_from(
        &self,
        owner_address: u32,
    ) -> Result<Vec<AstReference>, TastyFileError> {
        Ok(self
            .ast_references()?
            .into_iter()
            .filter(|reference| reference.owner_address == owner_address)
            .collect())
    }

    /// Collect AST references targeting `address` in their original wire
    /// order.
    pub fn ast_references_to(&self, address: u32) -> Result<Vec<AstReference>, TastyFileError> {
        Ok(self
            .ast_references()?
            .into_iter()
            .filter(|reference| reference.reference.address == address)
            .collect())
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

    /// Collect name-table references owned by the top-level node at
    /// `owner_address`, preserving wire order.
    pub fn name_references_from(
        &self,
        owner_address: u32,
    ) -> Result<Vec<NameReference>, TastyFileError> {
        Ok(self
            .name_references()?
            .into_iter()
            .filter(|reference| reference.owner_address == owner_address)
            .collect())
    }

    /// Collect name-table references to `name`, preserving wire order.
    pub fn name_references_to(&self, name: NameRef) -> Result<Vec<NameReference>, TastyFileError> {
        Ok(self
            .name_references()?
            .into_iter()
            .filter(|reference| reference.reference == name)
            .collect())
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
        self.validate_ast_reference_targets_with_max_depth(crate::DEFAULT_MAX_AST_INDEX_DEPTH)
    }

    /// Validate AST references against a global index built with an explicit
    /// maximum nesting depth.
    pub fn validate_ast_reference_targets_with_max_depth(
        &self,
        max_depth: usize,
    ) -> Result<(), TastyFileError> {
        let index = self.ast_address_index_with_max_depth(max_depth)?;
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
                    self.resolve_source_file(*reference)?;
                }
            }
        }

        Ok(attributes)
    }

    /// Resolve the compiler-emitted `SOURCEFILE` attribute.
    ///
    /// Unlike ordinary `NameRef` values, the attribute stores a zero-based
    /// name-table index. The raw index remains available through
    /// [`TastyFile::attributes`] so encoding can remain lossless.
    pub fn source_file(&self) -> Result<Option<&str>, TastyFileError> {
        let Some(attributes) = self.attributes()? else {
            return Ok(None);
        };

        let Some(reference) = attributes.iter().find_map(|attribute| match attribute {
            Attribute::SourceFile(reference) => Some(*reference),
            _ => None,
        }) else {
            return Ok(None);
        };

        self.resolve_source_file(reference).map(Some)
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

    /// Resolve position associations when the file contains a `Positions`
    /// section.
    ///
    /// `None` means that the section is absent. The returned associations carry
    /// the active source reference and omit standalone source-change events.
    pub fn resolved_position_associations(
        &self,
    ) -> Result<Option<Vec<ResolvedPosition>>, TastyFileError> {
        self.positions()?
            .map(|positions| positions.resolved_associations())
            .transpose()
            .map_err(TastyFileError::from)
    }

    /// Find the resolved source position associated with an AST address.
    ///
    /// `None` means that the file has no `Positions` section or that no
    /// association uses the requested address.
    pub fn resolved_position_at(
        &self,
        address: u32,
    ) -> Result<Option<ResolvedPosition>, TastyFileError> {
        self.positions()?
            .map(|positions| positions.resolved_association_at(i64::from(address)))
            .transpose()
            .map(|position| position.flatten())
            .map_err(TastyFileError::from)
    }

    /// Pair resolved source positions with visible AST nodes.
    ///
    /// The result keeps the order of the `Positions` associations. An
    /// association whose address is inside the AST section but does not start
    /// a visible node is omitted. `None` means that the file has no
    /// `Positions` section.
    pub fn ast_node_positions(&self) -> Result<Option<Vec<AstTreePosition>>, TastyFileError> {
        self.ast_node_positions_with_max_depth(crate::DEFAULT_MAX_AST_INDEX_DEPTH)
    }

    /// Pair resolved source positions with visible AST nodes using an
    /// explicit AST traversal limit.
    pub fn ast_node_positions_with_max_depth(
        &self,
        max_depth: usize,
    ) -> Result<Option<Vec<AstTreePosition>>, TastyFileError> {
        let Some(positions) = self.positions()? else {
            return Ok(None);
        };
        let index = self.ast_address_index_with_max_depth(max_depth)?;
        let mut mapped = Vec::new();
        for position in positions.resolved_associations()? {
            let Ok(address) = u32::try_from(position.address) else {
                continue;
            };
            if let Some(node) = index.get_node(address) {
                mapped.push(AstTreePosition { node, position });
            }
        }
        Ok(Some(mapped))
    }

    /// Find visible AST nodes whose resolved source positions intersect
    /// `[start, end)`.
    ///
    /// Non-empty spans use half-open interval overlap. Point positions use
    /// their resolved `point` coordinate instead. The result keeps Positions
    /// wire order. `None` means that the file has no `Positions` section;
    /// an empty or inverted range returns an empty vector.
    pub fn ast_nodes_in_source_range(
        &self,
        start: i64,
        end: i64,
    ) -> Result<Option<Vec<AstTreePosition>>, TastyFileError> {
        self.ast_nodes_in_source_range_with_max_depth(
            start,
            end,
            crate::DEFAULT_MAX_AST_INDEX_DEPTH,
        )
    }

    /// Find visible AST nodes whose resolved source positions intersect
    /// `[start, end)` using an explicit AST traversal limit.
    pub fn ast_nodes_in_source_range_with_max_depth(
        &self,
        start: i64,
        end: i64,
        max_depth: usize,
    ) -> Result<Option<Vec<AstTreePosition>>, TastyFileError> {
        if start >= end {
            return Ok(self.positions()?.map(|_| Vec::new()));
        }

        let Some(mapped) = self.ast_node_positions_with_max_depth(max_depth)? else {
            return Ok(None);
        };

        Ok(Some(
            mapped
                .into_iter()
                .filter(|entry| {
                    if entry.position.start < entry.position.end {
                        entry.position.start < end && start < entry.position.end
                    } else {
                        entry.position.point >= start && entry.position.point < end
                    }
                })
                .collect(),
        ))
    }

    fn asts_length(&self) -> usize {
        self.section(StandardSection::Asts)
            .map(|section| section.payload.len())
            .unwrap_or(0)
    }

    fn resolve_source_file(&self, reference: NameRef) -> Result<&str, TastyFileError> {
        match self.names.get_zero_based(reference) {
            Some(RawName::Utf8(name)) => Ok(name),
            Some(name) => Err(TastyFileError::InvalidSourceFileName {
                reference,
                kind: name.kind(),
            }),
            None => Err(TastyFileError::InvalidNameReference {
                context: "SOURCEFILE attribute",
                reference,
            }),
        }
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
    use crate::section::{
        EncodedSection, PositionEntry, PositionSection, Section, StandardSection,
    };

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
    fn renders_a_fixture_name_through_the_file_api() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();
        let reference = file.names().find_utf8("ASTs").unwrap();

        assert_eq!(file.render_name(reference), Ok("ASTs".to_owned()));
    }

    #[test]
    fn resolves_a_fixture_source_file_using_a_zero_based_name_index() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();

        assert_eq!(
            file.source_file(),
            Ok(Some(
                "IdeaProjects/TastyFixtures/src/main/scala/me/cytrowski/tastyfixtures/SimpleDef.scala",
            ))
        );
    }

    #[test]
    fn rejects_a_source_file_index_outside_the_name_table() {
        let attributes = [crate::SOURCEFILE_ATTR, 0x82];
        let names = NameTable::from_entries(vec![
            crate::RawName::Utf8("Attributes".to_owned()),
            crate::RawName::Utf8("Example.scala".to_owned()),
        ])
        .unwrap();
        let sections = crate::SectionTable::from_sections(vec![Section::new(0, &attributes)]);
        let file = TastyFile::from_parts(
            Header {
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
            file.source_file(),
            Err(TastyFileError::InvalidNameReference {
                context: "SOURCEFILE attribute",
                reference: 2,
            })
        );
    }

    #[test]
    fn returns_no_source_file_when_the_attributes_section_is_absent() {
        let file = TastyFile::from_parts(
            Header {
                major_version: 28,
                minor_version: 9,
                experimental_version: 0,
                tooling_version: "Scala 3.9.0".to_owned(),
                uuid: [0; 16],
            },
            NameTable::from_entries(vec![crate::RawName::Utf8("Attributes".to_owned())]).unwrap(),
            crate::SectionTable::from_sections(Vec::new()),
        )
        .unwrap();

        assert_eq!(file.source_file(), Ok(None));
    }

    #[test]
    fn rejects_a_source_file_index_that_targets_a_composite_name() {
        let attributes = [crate::SOURCEFILE_ATTR, 0x81];
        let names = NameTable::from_entries(vec![
            crate::RawName::Utf8("Attributes".to_owned()),
            crate::RawName::Qualified {
                prefix: 1,
                selector: 1,
            },
        ])
        .unwrap();
        let sections = crate::SectionTable::from_sections(vec![Section::new(0, &attributes)]);
        let file = TastyFile::from_parts(
            Header {
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
            file.attributes(),
            Err(TastyFileError::InvalidSourceFileName {
                reference: 1,
                kind: crate::RawNameKind::Qualified,
            })
        );
    }

    #[test]
    fn propagates_invalid_name_references_from_the_file_api() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();

        assert_eq!(
            file.render_name(0),
            Err(crate::NameRenderError::InvalidReference { reference: 0 })
        );
    }

    #[test]
    fn queries_nested_fixture_nodes_by_tag() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();
        let expected = file
            .ast_address_index()
            .unwrap()
            .iter_nodes()
            .filter(|node| node.tag == crate::DEFDEF_TAG)
            .collect::<Vec<_>>();

        assert_eq!(
            file.ast_nodes_with_tag(crate::DEFDEF_TAG).unwrap(),
            expected
        );
    }

    #[test]
    fn exposes_fixture_ast_parent_and_children_through_the_file_api() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();
        let edge = file
            .ast_tree_edges()
            .unwrap()
            .into_iter()
            .next()
            .expect("fixture should contain a nested AST tree");

        assert_eq!(
            file.ast_parent_of(edge.child.offset as u32).unwrap(),
            Some(edge.parent)
        );
        assert_eq!(
            file.ast_children_of(edge.parent.offset as u32)
                .unwrap()
                .first(),
            Some(&edge.child)
        );
    }

    #[test]
    fn applies_a_configured_depth_limit_to_ast_parent_queries() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();

        assert!(matches!(
            file.ast_parent_of_with_max_depth(0, 0),
            Err(TastyFileError::Asts(crate::AstError::RecursionLimit {
                limit: 0,
                ..
            }))
        ));
        assert!(matches!(
            file.ast_children_of_with_max_depth(0, 0),
            Err(TastyFileError::Asts(crate::AstError::RecursionLimit {
                limit: 0,
                ..
            }))
        ));
    }

    #[test]
    fn applies_a_configured_depth_limit_to_ast_edge_queries() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();

        assert!(matches!(
            file.ast_tree_edges_with_max_depth(0),
            Err(TastyFileError::Asts(crate::AstError::RecursionLimit {
                limit: 0,
                ..
            }))
        ));
    }

    #[test]
    fn returns_no_fixture_nodes_for_an_unknown_tag() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();

        assert!(file.ast_nodes_with_tag(255).unwrap().is_empty());
    }

    #[test]
    fn applies_a_configured_depth_limit_to_tag_queries() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();

        assert!(matches!(
            file.ast_nodes_with_tag_with_max_depth(crate::DEFDEF_TAG, 0),
            Err(TastyFileError::Asts(crate::AstError::RecursionLimit {
                limit: 0,
                ..
            }))
        ));
    }

    #[test]
    fn queries_fixture_nodes_in_an_absolute_address_range() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();
        let all_nodes = file
            .ast_address_index()
            .unwrap()
            .node_addresses()
            .collect::<Vec<_>>();
        let end = all_nodes.last().copied().unwrap() + 1;

        assert_eq!(
            file.ast_nodes_in_address_range(0, end).unwrap().len(),
            all_nodes.len()
        );
    }

    #[test]
    fn applies_a_configured_depth_limit_to_address_range_queries() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();

        assert!(matches!(
            file.ast_nodes_in_address_range_with_max_depth(0, u32::MAX, 0),
            Err(TastyFileError::Asts(crate::AstError::RecursionLimit {
                limit: 0,
                ..
            }))
        ));
    }

    #[test]
    fn resolves_fixture_position_associations_through_the_file_api() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();

        assert!(
            !file
                .resolved_position_associations()
                .unwrap()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn finds_a_fixture_position_by_its_ast_address() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();
        let expected = file
            .resolved_position_associations()
            .unwrap()
            .unwrap()
            .into_iter()
            .next()
            .unwrap();

        assert_eq!(
            file.resolved_position_at(expected.address as u32).unwrap(),
            Some(expected)
        );
    }

    #[test]
    fn joins_fixture_positions_with_visible_ast_nodes_in_position_order() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();
        let mapped = file.ast_node_positions().unwrap().unwrap();

        assert!(!mapped.is_empty());
        for entry in mapped {
            assert_eq!(
                file.resolved_position_at(entry.node.offset as u32).unwrap(),
                Some(entry.position.clone())
            );
            assert_eq!(entry.position.address, entry.node.offset as i64);
        }
    }

    #[test]
    fn returns_no_ast_position_mapping_when_positions_are_absent() {
        let bytes = compatible_file_bytes();
        let file = TastyFile::parse(&bytes).unwrap();

        assert_eq!(file.ast_node_positions().unwrap(), None);
    }

    #[test]
    fn applies_a_configured_depth_limit_to_ast_position_mapping() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();

        assert!(matches!(
            file.ast_node_positions_with_max_depth(0),
            Err(TastyFileError::Asts(crate::AstError::RecursionLimit {
                limit: 0,
                ..
            }))
        ));
    }

    #[test]
    fn filters_fixture_ast_positions_by_a_half_open_source_range() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();
        let mapped = file.ast_node_positions().unwrap().unwrap();
        let first = mapped.first().unwrap();
        let start = first.position.start;
        let end = first.position.end.max(start + 1);

        let filtered = file.ast_nodes_in_source_range(start, end).unwrap().unwrap();

        assert!(filtered.iter().any(|entry| entry.node == first.node));
        assert!(filtered.iter().all(|entry| {
            if entry.position.start < entry.position.end {
                entry.position.start < end && start < entry.position.end
            } else {
                entry.position.point >= start && entry.position.point < end
            }
        }));
    }

    #[test]
    fn treats_an_empty_source_range_as_empty_without_indexing_asts() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();

        assert_eq!(
            file.ast_nodes_in_source_range_with_max_depth(10, 10, 0)
                .unwrap(),
            Some(Vec::new())
        );
    }

    #[test]
    fn returns_no_source_range_matches_when_positions_are_absent() {
        let bytes = compatible_file_bytes();
        let file = TastyFile::parse(&bytes).unwrap();

        assert_eq!(file.ast_nodes_in_source_range(0, 1).unwrap(), None);
    }

    #[test]
    fn applies_a_configured_depth_limit_to_source_range_queries() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();

        assert!(matches!(
            file.ast_nodes_in_source_range_with_max_depth(0, i64::MAX, 0),
            Err(TastyFileError::Asts(crate::AstError::RecursionLimit {
                limit: 0,
                ..
            }))
        ));
    }

    #[test]
    fn applies_a_configured_depth_limit_when_indexing_file_asts() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse(bytes).unwrap();

        assert!(matches!(
            file.ast_address_index_with_max_depth(0),
            Err(TastyFileError::Asts(crate::AstError::RecursionLimit {
                limit: 0,
                ..
            }))
        ));
    }

    #[test]
    fn applies_a_configured_depth_limit_during_file_validation() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse(bytes).unwrap();

        assert!(matches!(
            file.validate_with_max_ast_index_depth(0),
            Err(TastyFileError::Asts(crate::AstError::RecursionLimit {
                limit: 0,
                ..
            }))
        ));
    }

    #[test]
    fn applies_a_configured_depth_limit_during_compatible_validation() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse(bytes).unwrap();

        assert!(matches!(
            file.validate_compatible_with_max_ast_index_depth(28, 10, 0, 0),
            Err(TastyFileError::Asts(crate::AstError::RecursionLimit {
                limit: 0,
                ..
            }))
        ));
    }

    #[test]
    fn applies_a_configured_depth_limit_during_scala_versioned_parsing() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");

        assert!(matches!(
            TastyFile::parse_and_validate_scala_3_9_with_max_ast_index_depth(bytes, 0),
            Err(TastyFileError::Asts(crate::AstError::RecursionLimit {
                limit: 0,
                ..
            }))
        ));
    }

    #[test]
    fn applies_a_configured_depth_limit_during_compatible_parsing() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");

        assert!(matches!(
            TastyFile::parse_and_validate_compatible_with_max_ast_index_depth(bytes, 28, 10, 0, 0),
            Err(TastyFileError::Asts(crate::AstError::RecursionLimit {
                limit: 0,
                ..
            }))
        ));
    }

    #[test]
    fn applies_a_configured_depth_limit_during_validated_encoding() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse(bytes).unwrap();

        assert!(matches!(
            file.encode_validated_with_max_ast_index_depth(0),
            Err(TastyFileError::Asts(crate::AstError::RecursionLimit {
                limit: 0,
                ..
            }))
        ));
    }

    #[test]
    fn applies_a_configured_depth_limit_during_address_encoding() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse(bytes).unwrap();

        assert!(matches!(
            file.encode_validated_with_max_ast_index_depth_and_ast_addresses(0),
            Err(TastyFileError::Asts(crate::AstError::RecursionLimit {
                limit: 0,
                ..
            }))
        ));
    }

    #[test]
    fn applies_a_configured_depth_limit_to_builder_encoding() {
        let builder = super::TastyFileBuilder::new(
            crate::Header {
                major_version: 28,
                minor_version: 9,
                experimental_version: 0,
                tooling_version: "Scala 3.9.0".to_owned(),
                uuid: [0; 16],
            },
            crate::NameTable::from_entries(vec![
                crate::RawName::Utf8("ASTs".to_owned()),
                crate::RawName::Utf8("value".to_owned()),
            ])
            .unwrap(),
        )
        .with_section(EncodedSection::raw(0, [crate::VALDEF_TAG, 0x82, 0x82, 3]));

        assert!(matches!(
            builder.encode_validated_with_max_ast_index_depth(0),
            Err(TastyFileError::Asts(crate::AstError::RecursionLimit {
                limit: 0,
                ..
            }))
        ));
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
    fn parses_and_validates_a_fixture_against_a_newer_compatible_minor_version() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_and_validate_compatible_with(bytes, 28, 10, 0).unwrap();

        assert_eq!(file.header().major_version, 28);
        assert_eq!(file.header().minor_version, 9);
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
    fn applies_a_configured_depth_limit_to_builder_validation() {
        let builder = super::TastyFileBuilder::new(
            crate::Header {
                major_version: 28,
                minor_version: 9,
                experimental_version: 0,
                tooling_version: "Scala 3.9.0".to_owned(),
                uuid: [0; 16],
            },
            crate::NameTable::from_entries(vec![
                crate::RawName::Utf8("ASTs".to_owned()),
                crate::RawName::Utf8("value".to_owned()),
            ])
            .unwrap(),
        )
        .with_section(EncodedSection::raw(0, [crate::VALDEF_TAG, 0x82, 0x82, 3]));

        assert!(matches!(
            builder.validate_with_max_ast_index_depth(0),
            Err(TastyFileError::Asts(crate::AstError::RecursionLimit {
                limit: 0,
                ..
            }))
        ));
    }

    #[test]
    fn applies_a_configured_depth_limit_to_builder_compatible_validation() {
        let builder = super::TastyFileBuilder::new(
            crate::Header {
                major_version: 28,
                minor_version: 9,
                experimental_version: 0,
                tooling_version: "Scala 3.9.0".to_owned(),
                uuid: [0; 16],
            },
            crate::NameTable::from_entries(vec![
                crate::RawName::Utf8("ASTs".to_owned()),
                crate::RawName::Utf8("value".to_owned()),
            ])
            .unwrap(),
        )
        .with_section(EncodedSection::raw(0, [crate::VALDEF_TAG, 0x82, 0x82, 3]));

        assert!(matches!(
            builder.validate_compatible_with_max_ast_index_depth(28, 10, 0, 0),
            Err(TastyFileError::Asts(crate::AstError::RecursionLimit {
                limit: 0,
                ..
            }))
        ));
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
        crate::Attribute::encode_all(&[crate::Attribute::SourceFile(4)], &mut attributes).unwrap();
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
        .with_section(EncodedSection::attributes(1, &[crate::Attribute::SourceFile(4)]).unwrap())
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
    fn filters_ast_references_by_owner_and_target_address() {
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
        let references = file.ast_references().unwrap();

        assert_eq!(file.ast_references_from(0).unwrap(), references);
        assert_eq!(file.ast_references_to(5).unwrap(), vec![references[0]]);
        assert_eq!(file.ast_references_to(99).unwrap(), Vec::new());
    }

    #[test]
    fn filters_name_references_by_owner_address() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();
        let references = file.name_references().unwrap();
        let owner = references.first().unwrap().owner_address;

        assert_eq!(
            file.name_references_from(owner).unwrap(),
            references
                .iter()
                .copied()
                .filter(|reference| reference.owner_address == owner)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn filters_name_references_by_name_reference() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();
        let references = file.name_references().unwrap();
        let name = references.first().unwrap().reference;

        assert_eq!(
            file.name_references_to(name).unwrap(),
            references
                .iter()
                .copied()
                .filter(|reference| reference.reference == name)
                .collect::<Vec<_>>()
        );
        assert!(file.name_references_to(u32::MAX).unwrap().is_empty());
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
