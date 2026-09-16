use crate::ast::{AstError, DEFAULT_MAX_AST_INDEX_DEPTH, RawNodes, StructuredNode};
use crate::name_table::NameRef;
use crate::reader::{ReadError, Reader};
use crate::term::TermEncodeError;
use crate::writer::{WriteError, Writer};
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

/// Owns an encoded section payload while it is being assembled into a file.
///
/// [`EncodedSection::section`] provides a borrowed [`Section`] view that can
/// be passed to [`SectionTable::from_sections`]. Keeping the owner alive also
/// keeps that view valid, which makes typed section encoding convenient for
/// programmatically built files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedSection {
    name: NameRef,
    payload: Vec<u8>,
}

/// Owns an encoded `ASTs` section together with the addresses allocated to its
/// top-level nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedAstSection {
    section: EncodedSection,
    ast_addresses: Vec<u32>,
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
    /// Raw zero-based name-table index emitted by Scala's `SOURCEFILEattr`.
    SourceFile(NameRef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// Absolute AST address of the definition carrying this comment.
    pub address: u32,
    pub text: String,
    pub coordinates: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PositionEntry {
    Association {
        address_delta: i64,
        start_delta: Option<i64>,
        end_delta: Option<i64>,
        point_delta: Option<i64>,
    },
    Source(NameRef),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionCoordinate {
    Address,
    Start,
    End,
    Point,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedPositionEntry {
    Source(NameRef),
    Association {
        address: i64,
        start: i64,
        end: i64,
        point: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPosition {
    pub source: Option<NameRef>,
    pub address: i64,
    pub start: i64,
    pub end: i64,
    pub point: i64,
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

impl Attribute {
    pub fn encode_all(attributes: &[Self], writer: &mut Writer) -> Result<(), WriteError> {
        let mut previous = None;
        for attribute in attributes {
            let tag = attribute.tag();
            if previous.is_some_and(|previous| tag <= previous) {
                return Err(WriteError::AttributeOrder {
                    previous: previous.unwrap(),
                    current: tag,
                });
            }
            previous = Some(tag);
            writer.write_u8(tag);
            if let Self::SourceFile(reference) = attribute {
                writer.write_nat(*reference);
            }
        }
        Ok(())
    }

    fn tag(&self) -> u8 {
        match self {
            Self::Scala2StandardLibrary => SCALA2STANDARDLIBRARY_ATTR,
            Self::ExplicitNulls => EXPLICITNULLS_ATTR,
            Self::CaptureChecked => CAPTURECHECKED_ATTR,
            Self::WithPureFuns => WITHPUREFUNS_ATTR,
            Self::Java => JAVA_ATTR,
            Self::Outline => OUTLINE_ATTR,
            Self::SourceFile(_) => SOURCEFILE_ATTR,
        }
    }
}

impl Comment {
    pub fn encode_all(comments: &[Self], writer: &mut Writer) -> Result<(), WriteError> {
        for comment in comments {
            writer.write_nat(comment.address);
            writer.write_utf8(&comment.text)?;
            writer.write_long_int(comment.coordinates);
        }
        Ok(())
    }
}

impl PositionSection {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), WriteError> {
        writer.write_nat(u32::try_from(self.line_sizes.len()).map_err(|_| {
            WriteError::LengthOverflow {
                length: self.line_sizes.len(),
            }
        })?);
        for line_size in &self.line_sizes {
            writer.write_nat(*line_size);
        }

        for entry in &self.entries {
            match entry {
                PositionEntry::Source(reference) => {
                    writer.write_int(4);
                    writer.write_int(i32::try_from(*reference).map_err(|_| {
                        WriteError::IntOverflow {
                            value: i64::from(*reference),
                        }
                    })?);
                }
                PositionEntry::Association {
                    address_delta,
                    start_delta,
                    end_delta,
                    point_delta,
                } => {
                    let flags = u8::from(start_delta.is_some()) << 2
                        | u8::from(end_delta.is_some()) << 1
                        | u8::from(point_delta.is_some());
                    let header = address_delta
                        .checked_mul(8)
                        .and_then(|value| value.checked_add(i64::from(flags)))
                        .ok_or(WriteError::IntOverflow {
                            value: *address_delta,
                        })?;
                    if header == 4 {
                        return Err(WriteError::PositionHeaderCollision {
                            address_delta: *address_delta,
                            flags,
                        });
                    }
                    writer.write_int(
                        i32::try_from(header)
                            .map_err(|_| WriteError::IntOverflow { value: header })?,
                    );
                    for delta in [start_delta, end_delta, point_delta].into_iter().flatten() {
                        writer.write_int(
                            i32::try_from(*delta)
                                .map_err(|_| WriteError::IntOverflow { value: *delta })?,
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// Resolves position deltas into absolute coordinates while retaining
    /// source-change events in their original order.
    ///
    /// The raw [`PositionEntry`] values remain available through `entries`;
    /// this derived view is intended for consumers that need coordinates
    /// rather than the lossless wire representation.
    pub fn resolved_entries(&self) -> Result<Vec<ResolvedPositionEntry>, SectionError> {
        let mut address = 0;
        let mut start = 0;
        let mut end = 0;
        let mut point = 0;
        let mut resolved = Vec::with_capacity(self.entries.len());

        for entry in &self.entries {
            match entry {
                PositionEntry::Source(reference) => {
                    resolved.push(ResolvedPositionEntry::Source(*reference));
                }
                PositionEntry::Association {
                    address_delta,
                    start_delta,
                    end_delta,
                    point_delta,
                } => {
                    address =
                        add_position_delta(address, *address_delta, PositionCoordinate::Address)?;
                    if let Some(delta) = start_delta {
                        start = add_position_delta(start, *delta, PositionCoordinate::Start)?;
                    }
                    if let Some(delta) = end_delta {
                        end = add_position_delta(end, *delta, PositionCoordinate::End)?;
                    }
                    if let Some(delta) = point_delta {
                        point = add_position_delta(point, *delta, PositionCoordinate::Point)?;
                    }
                    resolved.push(ResolvedPositionEntry::Association {
                        address,
                        start,
                        end,
                        point,
                    });
                }
            }
        }

        Ok(resolved)
    }

    /// Resolves position deltas and returns only associations with their
    /// currently active source reference.
    ///
    /// An association before the first `SOURCE` event has `source = None`.
    /// Source-change events are omitted from this derived view; use
    /// [`Self::resolved_entries`] when their ordering must be preserved.
    pub fn resolved_associations(&self) -> Result<Vec<ResolvedPosition>, SectionError> {
        let mut source = None;
        let mut associations = Vec::new();

        for entry in self.resolved_entries()? {
            match entry {
                ResolvedPositionEntry::Source(reference) => source = Some(reference),
                ResolvedPositionEntry::Association {
                    address,
                    start,
                    end,
                    point,
                } => associations.push(ResolvedPosition {
                    source,
                    address,
                    start,
                    end,
                    point,
                }),
            }
        }

        Ok(associations)
    }

    /// Finds the first resolved position association for an absolute AST
    /// address.
    pub fn resolved_association_at(
        &self,
        address: i64,
    ) -> Result<Option<ResolvedPosition>, SectionError> {
        Ok(self
            .resolved_associations()?
            .into_iter()
            .find(|association| association.address == address))
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
    NegativePositionSource {
        value: i32,
    },
    PositionOverflow {
        coordinate: PositionCoordinate,
        previous: i64,
        delta: i64,
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
            Self::NegativePositionSource { value } => {
                write!(
                    formatter,
                    "negative source name reference {value} in a TASTy position entry"
                )
            }
            Self::PositionOverflow {
                coordinate,
                previous,
                delta,
            } => write!(
                formatter,
                "position {coordinate:?} overflows when adding delta {delta} to {previous}"
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

fn add_position_delta(
    previous: i64,
    delta: i64,
    coordinate: PositionCoordinate,
) -> Result<i64, SectionError> {
    previous
        .checked_add(delta)
        .ok_or(SectionError::PositionOverflow {
            coordinate,
            previous,
            delta,
        })
}

impl<'a> Section<'a> {
    pub fn new(name: NameRef, payload: &'a [u8]) -> Self {
        Self {
            name,
            offset: 0,
            length: payload.len(),
            payload,
        }
    }

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
                address: reader.read_nat()?,
                text: reader.read_utf8()?,
                coordinates: reader.read_long_int()?,
            });
        }

        Ok(comments)
    }

    pub fn decode_positions(&self) -> Result<PositionSection, SectionError> {
        let mut reader = self.reader();
        let line_count = reader.read_nat()? as usize;

        // Every line size occupies at least one byte. Check that minimum
        // before reserving capacity so a malformed count cannot trigger an
        // allocation unrelated to the bounded section payload.
        if line_count > reader.remaining() {
            return Err(ReadError::UnexpectedEof {
                offset: reader.position(),
                needed: line_count,
                remaining: reader.remaining(),
            }
            .into());
        }

        let mut line_sizes = Vec::with_capacity(line_count);
        for _ in 0..line_count {
            line_sizes.push(reader.read_nat()?);
        }

        let mut entries = Vec::new();
        while !reader.is_at_end() {
            let header = reader.read_int()?;
            if header == 4 {
                let reference = reader.read_int()?;
                if reference < 0 {
                    return Err(SectionError::NegativePositionSource { value: reference });
                }
                entries.push(PositionEntry::Source(reference as u32));
                continue;
            }

            entries.push(PositionEntry::Association {
                address_delta: i64::from(header >> 3),
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

impl EncodedSection {
    /// Create an encoded section from an already serialized payload.
    pub fn raw(name: NameRef, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            name,
            payload: payload.into(),
        }
    }

    /// Encode an `Attributes` section payload and retain ownership of it.
    pub fn attributes(name: NameRef, attributes: &[Attribute]) -> Result<Self, WriteError> {
        let mut writer = Writer::new();
        Attribute::encode_all(attributes, &mut writer)?;
        Ok(Self::raw(name, writer.into_inner()))
    }

    /// Encode a `Comments` section payload and retain ownership of it.
    pub fn comments(name: NameRef, comments: &[Comment]) -> Result<Self, WriteError> {
        let mut writer = Writer::new();
        Comment::encode_all(comments, &mut writer)?;
        Ok(Self::raw(name, writer.into_inner()))
    }

    /// Encode a `Positions` section payload and retain ownership of it.
    pub fn positions(name: NameRef, positions: &PositionSection) -> Result<Self, WriteError> {
        let mut writer = Writer::new();
        positions.encode(&mut writer)?;
        Ok(Self::raw(name, writer.into_inner()))
    }

    /// Encode a raw top-level AST list and retain ownership of its payload.
    pub fn asts(name: NameRef, nodes: &RawNodes<'_>) -> Result<Self, WriteError> {
        let mut writer = Writer::new();
        nodes.encode(&mut writer)?;
        Ok(Self::raw(name, writer.into_inner()))
    }

    /// Encode structured top-level AST nodes and retain ownership of their payload.
    ///
    /// The emitted AST section is validated before it is returned. In
    /// particular, an AST reference that no longer targets a visible node
    /// after a structural size change is reported as an error instead of
    /// producing an invalid section.
    pub fn structured_asts(
        name: NameRef,
        nodes: &[StructuredNode<'_>],
    ) -> Result<Self, TermEncodeError> {
        let mut writer = Writer::new();
        for node in nodes {
            node.encode(&mut writer)?;
        }
        validate_structured_ast_payload(writer.as_slice())?;
        Ok(Self::raw(name, writer.into_inner()))
    }

    /// Encode raw top-level AST nodes and retain their allocated addresses.
    pub fn asts_with_addresses(
        name: NameRef,
        nodes: &RawNodes<'_>,
    ) -> Result<EncodedAstSection, WriteError> {
        EncodedAstSection::from_raw(name, nodes)
    }

    /// Encode structured top-level AST nodes and retain their allocated addresses.
    ///
    /// The returned addresses describe the newly emitted layout. Existing
    /// `ASTRef` values in the structured nodes are not inferred or relocated;
    /// stale references are rejected during encoding.
    pub fn structured_asts_with_addresses(
        name: NameRef,
        nodes: &[StructuredNode<'_>],
    ) -> Result<EncodedAstSection, TermEncodeError> {
        EncodedAstSection::from_structured(name, nodes)
    }

    pub fn name(&self) -> NameRef {
        self.name
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Borrow this owned payload as a regular section.
    pub fn section(&self) -> Section<'_> {
        Section::new(self.name, &self.payload)
    }

    pub fn into_parts(self) -> (NameRef, Vec<u8>) {
        (self.name, self.payload)
    }
}

impl EncodedAstSection {
    fn from_raw(name: NameRef, nodes: &RawNodes<'_>) -> Result<Self, WriteError> {
        let encoded = nodes.encode_with_addresses()?;
        Ok(Self {
            section: EncodedSection::raw(name, encoded.as_slice().to_vec()),
            ast_addresses: encoded.addresses().to_vec(),
        })
    }

    fn from_structured(
        name: NameRef,
        nodes: &[StructuredNode<'_>],
    ) -> Result<Self, TermEncodeError> {
        let mut writer = Writer::new();
        let mut ast_addresses = Vec::with_capacity(nodes.len());

        for node in nodes {
            ast_addresses.push(u32::try_from(writer.position()).map_err(|_| {
                WriteError::NatOverflow {
                    value: writer.position() as u64,
                }
            })?);
            node.encode(&mut writer)?;
        }

        validate_structured_ast_payload(writer.as_slice())?;

        Ok(Self {
            section: EncodedSection::raw(name, writer.into_inner()),
            ast_addresses,
        })
    }

    pub fn name(&self) -> NameRef {
        self.section.name()
    }

    pub fn payload(&self) -> &[u8] {
        self.section.payload()
    }

    pub fn ast_addresses(&self) -> &[u32] {
        &self.ast_addresses
    }

    pub fn section(&self) -> &EncodedSection {
        &self.section
    }

    pub fn into_section(self) -> EncodedSection {
        self.section
    }

    pub fn into_parts(self) -> (EncodedSection, Vec<u32>) {
        (self.section, self.ast_addresses)
    }
}

fn validate_structured_ast_payload(bytes: &[u8]) -> Result<(), TermEncodeError> {
    let mut reader = Reader::new(bytes);
    let nodes = RawNodes::decode(&mut reader).map_err(AstError::from)?;
    let index =
        nodes.deep_address_index_with_source_and_max_depth(bytes, DEFAULT_MAX_AST_INDEX_DEPTH)?;

    for node in nodes.iter() {
        for reference in node.ast_refs()? {
            if index.resolve_node(reference).is_none() {
                return Err(AstError::InvalidAstReference {
                    address: reference.address,
                }
                .into());
            }
        }
    }

    Ok(())
}

impl<'a> SectionTable<'a> {
    pub fn from_sections(sections: Vec<Section<'a>>) -> Self {
        Self { sections }
    }

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

    pub(crate) fn validate_references(&self, name_count: usize) -> Result<(), SectionError> {
        for section in &self.sections {
            if section.name as usize >= name_count {
                return Err(SectionError::InvalidNameReference {
                    reference: section.name,
                    name_count,
                    offset: section.offset,
                });
            }
        }
        Ok(())
    }

    pub fn encode(&self, writer: &mut Writer) -> Result<(), WriteError> {
        for section in &self.sections {
            writer.write_nat(section.name);
            writer.write_length_prefixed_bytes(section.payload)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Attribute, Comment, EncodedSection, PositionCoordinate, PositionEntry, PositionSection,
        ResolvedPosition, ResolvedPositionEntry, Section, SectionError, SectionTable,
    };
    use crate::ast::{AstError, PackageNode, RawNode, RawNodes, ReturnNode, StructuredNode};
    use crate::reader::{ReadError, Reader};
    use crate::term::{AstRef, AstRefKind, RawTree, TermEncodeError, TermValue};
    use crate::writer::{WriteError, Writer};

    #[test]
    fn encodes_all_attributes_in_wire_order() {
        let attributes = vec![
            Attribute::Scala2StandardLibrary,
            Attribute::ExplicitNulls,
            Attribute::CaptureChecked,
            Attribute::WithPureFuns,
            Attribute::Java,
            Attribute::Outline,
            Attribute::SourceFile(5),
        ];
        let mut writer = Writer::new();
        Attribute::encode_all(&attributes, &mut writer).unwrap();

        let section = Section {
            name: 0,
            offset: 0,
            length: writer.as_slice().len(),
            payload: writer.as_slice(),
        };
        assert_eq!(section.decode_attributes().unwrap(), attributes);
    }

    #[test]
    fn builds_an_owned_attributes_section() {
        let encoded = EncodedSection::attributes(3, &[Attribute::ExplicitNulls]).unwrap();

        assert_eq!(encoded.name(), 3);
        assert_eq!(encoded.payload(), &[2]);
        assert_eq!(
            encoded.section().decode_attributes().unwrap(),
            vec![Attribute::ExplicitNulls]
        );
    }

    #[test]
    fn rejects_unordered_attributes_when_encoding() {
        let mut writer = Writer::new();
        assert_eq!(
            Attribute::encode_all(
                &[Attribute::ExplicitNulls, Attribute::Scala2StandardLibrary],
                &mut writer
            ),
            Err(WriteError::AttributeOrder {
                previous: 2,
                current: 1,
            })
        );
    }

    #[test]
    fn encodes_comments_with_unicode_and_signed_coordinates() {
        let comments = vec![
            Comment {
                address: 17,
                text: "zażółć".to_owned(),
                coordinates: -129,
            },
            Comment {
                address: 255,
                text: "ok".to_owned(),
                coordinates: 16_384,
            },
        ];
        let mut writer = Writer::new();
        Comment::encode_all(&comments, &mut writer).unwrap();

        let section = Section {
            name: 0,
            offset: 0,
            length: writer.as_slice().len(),
            payload: writer.as_slice(),
        };
        assert_eq!(section.decode_comments().unwrap(), comments);
    }

    #[test]
    fn builds_an_owned_comments_section() {
        let comments = [Comment {
            address: 4,
            text: "comment".to_owned(),
            coordinates: -1,
        }];
        let encoded = EncodedSection::comments(2, &comments).unwrap();

        assert_eq!(encoded.name(), 2);
        assert_eq!(encoded.section().decode_comments().unwrap(), comments);
    }

    #[test]
    fn builds_an_owned_positions_section() {
        let positions = PositionSection {
            line_sizes: vec![12, 8],
            entries: vec![PositionEntry::Association {
                address_delta: 1,
                start_delta: Some(2),
                end_delta: None,
                point_delta: Some(-1),
            }],
        };
        let encoded = EncodedSection::positions(1, &positions).unwrap();

        assert_eq!(encoded.name(), 1);
        assert_eq!(encoded.section().decode_positions().unwrap(), positions);
    }

    #[test]
    fn resolves_position_deltas_and_keeps_source_events_in_order() {
        let positions = PositionSection {
            line_sizes: vec![],
            entries: vec![
                PositionEntry::Source(4),
                PositionEntry::Association {
                    address_delta: 10,
                    start_delta: Some(2),
                    end_delta: Some(5),
                    point_delta: None,
                },
                PositionEntry::Association {
                    address_delta: 3,
                    start_delta: None,
                    end_delta: Some(-1),
                    point_delta: Some(7),
                },
                PositionEntry::Source(8),
            ],
        };

        assert_eq!(
            positions.resolved_entries(),
            Ok(vec![
                ResolvedPositionEntry::Source(4),
                ResolvedPositionEntry::Association {
                    address: 10,
                    start: 2,
                    end: 5,
                    point: 0,
                },
                ResolvedPositionEntry::Association {
                    address: 13,
                    start: 2,
                    end: 4,
                    point: 7,
                },
                ResolvedPositionEntry::Source(8),
            ])
        );
    }

    #[test]
    fn resolves_position_associations_with_the_active_source() {
        let positions = PositionSection {
            line_sizes: vec![],
            entries: vec![
                PositionEntry::Source(4),
                PositionEntry::Association {
                    address_delta: 10,
                    start_delta: Some(2),
                    end_delta: Some(5),
                    point_delta: None,
                },
                PositionEntry::Source(8),
                PositionEntry::Association {
                    address_delta: 3,
                    start_delta: None,
                    end_delta: None,
                    point_delta: Some(7),
                },
            ],
        };

        assert_eq!(
            positions.resolved_associations(),
            Ok(vec![
                ResolvedPosition {
                    source: Some(4),
                    address: 10,
                    start: 2,
                    end: 5,
                    point: 0,
                },
                ResolvedPosition {
                    source: Some(8),
                    address: 13,
                    start: 2,
                    end: 5,
                    point: 7,
                },
            ])
        );
    }

    #[test]
    fn resolves_a_position_association_without_a_source_as_unattributed() {
        let positions = PositionSection {
            line_sizes: vec![],
            entries: vec![PositionEntry::Association {
                address_delta: 1,
                start_delta: None,
                end_delta: None,
                point_delta: None,
            }],
        };

        assert_eq!(
            positions.resolved_associations(),
            Ok(vec![ResolvedPosition {
                source: None,
                address: 1,
                start: 0,
                end: 0,
                point: 0,
            }])
        );
    }

    #[test]
    fn finds_a_resolved_position_association_by_address() {
        let positions = PositionSection {
            line_sizes: vec![],
            entries: vec![
                PositionEntry::Association {
                    address_delta: 4,
                    start_delta: Some(1),
                    end_delta: Some(3),
                    point_delta: Some(2),
                },
                PositionEntry::Association {
                    address_delta: 6,
                    start_delta: Some(4),
                    end_delta: Some(5),
                    point_delta: Some(6),
                },
            ],
        };

        assert_eq!(
            positions.resolved_association_at(10),
            Ok(Some(ResolvedPosition {
                source: None,
                address: 10,
                start: 5,
                end: 8,
                point: 8,
            }))
        );
    }

    #[test]
    fn returns_no_resolved_position_for_an_absent_address() {
        let positions = PositionSection {
            line_sizes: vec![],
            entries: vec![PositionEntry::Association {
                address_delta: 4,
                start_delta: None,
                end_delta: None,
                point_delta: None,
            }],
        };

        assert_eq!(positions.resolved_association_at(5), Ok(None));
    }

    #[test]
    fn reports_position_coordinate_overflow_when_resolving_deltas() {
        let positions = PositionSection {
            line_sizes: vec![],
            entries: vec![
                PositionEntry::Association {
                    address_delta: i64::MAX,
                    start_delta: None,
                    end_delta: None,
                    point_delta: None,
                },
                PositionEntry::Association {
                    address_delta: 1,
                    start_delta: None,
                    end_delta: None,
                    point_delta: None,
                },
            ],
        };

        assert_eq!(
            positions.resolved_entries(),
            Err(SectionError::PositionOverflow {
                coordinate: PositionCoordinate::Address,
                previous: i64::MAX,
                delta: 1,
            })
        );
    }

    #[test]
    fn rejects_a_maximal_position_line_count_before_allocating_line_sizes() {
        let bytes = [0x0f, 0x7f, 0x7f, 0x7f, 0xff];
        let section = Section::new(0, &bytes);

        assert_eq!(
            section.decode_positions(),
            Err(SectionError::Read(ReadError::UnexpectedEof {
                offset: 5,
                needed: u32::MAX as usize,
                remaining: 0,
            }))
        );
    }

    #[test]
    fn builds_an_owned_raw_section() {
        let encoded = EncodedSection::raw(9, vec![1, 2, 3]);

        assert_eq!(encoded.into_parts(), (9, vec![1, 2, 3]));
    }

    #[test]
    fn builds_an_owned_asts_section_from_raw_nodes() {
        let nodes = RawNodes::from_entries(vec![
            RawNode::new(crate::VALDEF_TAG, &[0x85, 2, 3, 17]).unwrap(),
        ])
        .unwrap();
        let encoded = EncodedSection::asts(4, &nodes).unwrap();

        assert_eq!(encoded.name(), 4);
        assert_eq!(
            encoded.payload(),
            &[crate::VALDEF_TAG, 0x84, 0x85, 2, 3, 17]
        );
    }

    #[test]
    fn builds_an_owned_asts_section_from_structured_nodes() {
        let nodes = RawNodes::from_entries(vec![
            RawNode::new(crate::VALDEF_TAG, &[0x85, 2, 3, 17]).unwrap(),
        ])
        .unwrap();
        let structured = vec![nodes.get(0).unwrap().decode_structured().unwrap()];
        let encoded = EncodedSection::structured_asts(4, &structured).unwrap();

        assert_eq!(
            encoded.payload(),
            &[crate::VALDEF_TAG, 0x84, 0x85, 2, 3, 17]
        );
    }

    #[test]
    fn builds_an_owned_asts_section_with_raw_node_addresses() {
        let nodes = RawNodes::from_entries(vec![
            RawNode::new(crate::VALDEF_TAG, &[0x85, 2, 3, 17]).unwrap(),
            RawNode::new(crate::VALDEF_TAG, &[0x85, 2, 3, 17]).unwrap(),
        ])
        .unwrap();
        let encoded = EncodedSection::asts_with_addresses(4, &nodes).unwrap();

        assert_eq!(encoded.name(), 4);
        assert_eq!(encoded.ast_addresses(), &[0, 6]);
        assert_eq!(
            encoded.payload(),
            &[
                crate::VALDEF_TAG,
                0x84,
                0x85,
                2,
                3,
                17,
                crate::VALDEF_TAG,
                0x84,
                0x85,
                2,
                3,
                17
            ]
        );
    }

    #[test]
    fn builds_an_owned_asts_section_with_structured_node_addresses() {
        let nodes = RawNodes::from_entries(vec![
            RawNode::new(crate::VALDEF_TAG, &[0x85, 2, 3, 17]).unwrap(),
            RawNode::new(crate::VALDEF_TAG, &[0x85, 2, 3, 17]).unwrap(),
        ])
        .unwrap();
        let structured = vec![
            nodes.get(0).unwrap().decode_structured().unwrap(),
            nodes.get(1).unwrap().decode_structured().unwrap(),
        ];
        let encoded = EncodedSection::structured_asts_with_addresses(4, &structured).unwrap();

        assert_eq!(encoded.ast_addresses(), &[0, 6]);
        assert_eq!(encoded.section().name(), 4);
    }

    #[test]
    fn rejects_structured_encoding_with_a_stale_ast_reference_after_node_growth() {
        let mut reader = Reader::new(&[crate::APPLY_TAG, 0x81, crate::UNITCONST_TAG]);
        let enlarged_expression = RawTree::decode(&mut reader).unwrap();
        let first = StructuredNode::Return(ReturnNode {
            target: AstRef {
                kind: AstRefKind::ReturnTarget,
                address: 4,
            },
            expression: Some(enlarged_expression),
        });
        let second = StructuredNode::Package(PackageNode {
            path: RawTree::leaf(crate::TERMREFPKG_TAG, TermValue::NameRef(1)).unwrap(),
            stats: RawNodes::from_entries(Vec::new()).unwrap(),
        });

        assert_eq!(
            EncodedSection::structured_asts(4, &[first, second]),
            Err(TermEncodeError::Ast(AstError::InvalidAstReference {
                address: 4,
            }))
        );
    }

    #[test]
    fn encodes_position_lines_associations_and_sources() {
        let positions = PositionSection {
            line_sizes: vec![10, 20],
            entries: vec![
                PositionEntry::Association {
                    address_delta: 1,
                    start_delta: Some(-2),
                    end_delta: Some(2),
                    point_delta: Some(4),
                },
                PositionEntry::Source(5),
            ],
        };
        let mut writer = Writer::new();
        positions.encode(&mut writer).unwrap();

        let section = Section {
            name: 0,
            offset: 0,
            length: writer.as_slice().len(),
            payload: writer.as_slice(),
        };
        assert_eq!(section.decode_positions().unwrap(), positions);
    }

    #[test]
    fn rejects_position_deltas_outside_tasty_int() {
        let positions = PositionSection {
            line_sizes: Vec::new(),
            entries: vec![PositionEntry::Association {
                address_delta: 1,
                start_delta: Some(i64::from(i32::MAX) + 1),
                end_delta: None,
                point_delta: None,
            }],
        };
        let mut writer = Writer::new();

        assert_eq!(
            positions.encode(&mut writer),
            Err(WriteError::IntOverflow {
                value: i64::from(i32::MAX) + 1,
            })
        );
    }

    #[test]
    fn rejects_position_headers_that_are_not_representable() {
        let positions = PositionSection {
            line_sizes: Vec::new(),
            entries: vec![PositionEntry::Association {
                address_delta: i64::from(u32::MAX),
                start_delta: None,
                end_delta: None,
                point_delta: None,
            }],
        };
        let mut writer = Writer::new();

        assert_eq!(
            positions.encode(&mut writer),
            Err(WriteError::IntOverflow {
                value: i64::from(u32::MAX) * 8,
            })
        );
    }

    #[test]
    fn rejects_the_reserved_source_position_header_for_associations() {
        let positions = PositionSection {
            line_sizes: Vec::new(),
            entries: vec![PositionEntry::Association {
                address_delta: 0,
                start_delta: Some(1),
                end_delta: None,
                point_delta: None,
            }],
        };
        let mut writer = Writer::new();

        assert_eq!(
            positions.encode(&mut writer),
            Err(WriteError::PositionHeaderCollision {
                address_delta: 0,
                flags: 4,
            })
        );
    }

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
    fn constructs_sections_with_derived_lengths() {
        let section = Section::new(3, b"abc");

        assert_eq!(section.name, 3);
        assert_eq!(section.offset, 0);
        assert_eq!(section.length, 3);
        assert_eq!(section.payload, b"abc");
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
            payload: &[0x81, 0x82, b'h', b'i', 0x83],
        };

        assert_eq!(
            section.decode_comments().unwrap(),
            vec![Comment {
                address: 1,
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
            payload: &[0x81, 0x81, b'x'],
        };

        assert_eq!(
            section.decode_comments(),
            Err(SectionError::Read(ReadError::UnexpectedEof {
                offset: 3,
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
    fn rejects_a_position_line_count_larger_than_the_remaining_payload() {
        let section = Section {
            name: 0,
            offset: 0,
            length: 0,
            payload: &[0x82],
        };

        assert_eq!(
            section.decode_positions(),
            Err(SectionError::Read(ReadError::UnexpectedEof {
                offset: 1,
                needed: 2,
                remaining: 0,
            }))
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
