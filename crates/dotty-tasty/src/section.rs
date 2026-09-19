use crate::ast::{
    AstAddressIndex, AstError, DEFAULT_MAX_AST_INDEX_DEPTH, RawNodes, StructuredNode,
};
use crate::name_table::NameRef;
use crate::reader::{ReadError, Reader};
use crate::term::TermEncodeError;
use crate::writer::{WriteError, Writer};
use std::collections::BTreeMap;
use std::fmt;

/// A borrowed section payload and its file-level location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section<'a> {
    /// Zero-based name-table index naming the section.
    pub name: NameRef,
    /// Absolute offset of the payload in the source file.
    pub offset: usize,
    /// Payload length in bytes.
    pub length: usize,
    /// Borrowed section payload.
    pub payload: &'a [u8],
}

/// Borrowed collection of sections in their original wire order.
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
    ast_address_map: BTreeMap<u32, u32>,
}

/// Identifies one of the standard TASTy sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardSection {
    /// The top-level AST section.
    Asts,
    /// Source-position section.
    Positions,
    /// Comment section.
    Comments,
    /// File-attributes section.
    Attributes,
}

/// Attribute tag for the Scala 2 standard library marker.
pub const SCALA2STANDARDLIBRARY_ATTR: u8 = 1;
/// Attribute tag for explicit-nulls mode.
pub const EXPLICITNULLS_ATTR: u8 = 2;
/// Attribute tag for capture checking.
pub const CAPTURECHECKED_ATTR: u8 = 3;
/// Attribute tag for pure-function checking.
pub const WITHPUREFUNS_ATTR: u8 = 4;
/// Attribute tag for Java-defined sources.
pub const JAVA_ATTR: u8 = 5;
/// Attribute tag for outline files.
pub const OUTLINE_ATTR: u8 = 6;
/// Attribute tag for the source-file name reference.
pub const SOURCEFILE_ATTR: u8 = 129;

/// Decoded standard TASTy file attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attribute {
    /// The file uses the Scala 2 standard library.
    Scala2StandardLibrary,
    /// The file was compiled with explicit nulls enabled.
    ExplicitNulls,
    /// The file was compiled with capture checking enabled.
    CaptureChecked,
    /// The file was compiled with pure-function checking enabled.
    WithPureFuns,
    /// The source originated from Java.
    Java,
    /// The file is an outline.
    Outline,
    /// Raw zero-based name-table index emitted by Scala's `SOURCEFILEattr`.
    SourceFile(NameRef),
}

/// A source comment associated with an AST address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// Absolute AST address of the definition carrying this comment.
    pub address: u32,
    /// Comment text.
    pub text: String,
    /// Encoded source coordinate associated with the comment.
    pub coordinates: i64,
}

/// Lossless position-section event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PositionEntry {
    /// A delta-encoded AST/source position association.
    Association {
        /// Delta from the previous AST address.
        address_delta: i64,
        /// Optional delta for the start coordinate.
        start_delta: Option<i64>,
        /// Optional delta for the end coordinate.
        end_delta: Option<i64>,
        /// Optional delta for the point coordinate.
        point_delta: Option<i64>,
    },
    /// Changes the active source-file name reference.
    Source(NameRef),
}

/// Coordinate affected by a position delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionCoordinate {
    /// AST address coordinate.
    Address,
    /// Source start coordinate.
    Start,
    /// Source end coordinate.
    End,
    /// Source point coordinate.
    Point,
}

/// Position event after address and coordinate deltas are resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedPositionEntry {
    /// Changes the active source-file name reference.
    Source(NameRef),
    /// Absolute position association.
    Association {
        /// Absolute AST address.
        address: i64,
        /// Absolute source start coordinate.
        start: i64,
        /// Absolute source end coordinate.
        end: i64,
        /// Absolute source point coordinate.
        point: i64,
    },
}

/// A resolved source position with the active source-file reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPosition {
    /// Active source-file name reference, if a source event preceded it.
    pub source: Option<NameRef>,
    /// Absolute AST address.
    pub address: i64,
    /// Absolute source start coordinate.
    pub start: i64,
    /// Absolute source end coordinate.
    pub end: i64,
    /// Absolute source point coordinate.
    pub point: i64,
}

/// Decoded source-position section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionSection {
    /// Size of each source line in bytes.
    pub line_sizes: Vec<u32>,
    /// Position events in wire order.
    pub entries: Vec<PositionEntry>,
}

impl StandardSection {
    /// Returns the canonical section name used by TASTy.
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
    /// Encodes attributes in the required strictly increasing tag order.
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
    /// Encodes comments in their supplied order.
    pub fn encode_all(comments: &[Self], writer: &mut Writer) -> Result<(), WriteError> {
        for comment in comments {
            writer.write_nat(comment.address);
            writer.write_utf8(&comment.text)?;
            writer.write_long_int(comment.coordinates);
        }
        Ok(())
    }

    pub(crate) fn relocate_ast_addresses(
        comments: &[Self],
        address_map: &BTreeMap<u32, u32>,
    ) -> Vec<Self> {
        comments
            .iter()
            .map(|comment| Self {
                address: address_map
                    .get(&comment.address)
                    .copied()
                    .unwrap_or(comment.address),
                text: comment.text.clone(),
                coordinates: comment.coordinates,
            })
            .collect()
    }
}

impl PositionSection {
    /// Encodes line sizes and position events in TASTy wire format.
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

    /// Rebuild address deltas after AST nodes have moved within the AST
    /// section. Source and source-coordinate entries retain their wire order
    /// and values; only association address deltas are changed.
    pub(crate) fn relocate_ast_addresses(
        &self,
        address_map: &BTreeMap<u32, u32>,
    ) -> Result<Self, SectionError> {
        let mut old_address = 0i64;
        let mut new_address = 0i64;
        let mut entries = Vec::with_capacity(self.entries.len());

        for entry in &self.entries {
            match entry {
                PositionEntry::Source(reference) => {
                    entries.push(PositionEntry::Source(*reference));
                }
                PositionEntry::Association {
                    address_delta,
                    start_delta,
                    end_delta,
                    point_delta,
                } => {
                    old_address = add_position_delta(
                        old_address,
                        *address_delta,
                        PositionCoordinate::Address,
                    )?;
                    let old_address_u32 = u32::try_from(old_address).map_err(|_| {
                        SectionError::InvalidAstAddress {
                            address: old_address,
                        }
                    })?;
                    let relocated = address_map
                        .get(&old_address_u32)
                        .copied()
                        .unwrap_or(old_address_u32);
                    let relocated = i64::from(relocated);
                    let relocated_delta = relocated.checked_sub(new_address).ok_or(
                        SectionError::PositionOverflow {
                            coordinate: PositionCoordinate::Address,
                            previous: new_address,
                            delta: relocated,
                        },
                    )?;
                    new_address = relocated;
                    entries.push(PositionEntry::Association {
                        address_delta: relocated_delta,
                        start_delta: *start_delta,
                        end_delta: *end_delta,
                        point_delta: *point_delta,
                    });
                }
            }
        }

        Ok(Self {
            line_sizes: self.line_sizes.clone(),
            entries,
        })
    }
}

/// Errors returned while decoding or resolving a section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionError {
    /// A bounded binary read failed.
    Read(ReadError),
    /// A section name reference is outside the name table.
    InvalidNameReference {
        /// Invalid zero-based name index.
        reference: NameRef,
        /// Number of available name entries.
        name_count: usize,
        /// Offset where the reference was read.
        offset: usize,
    },
    /// An attribute tag is not supported by this format version.
    InvalidAttributeTag {
        /// Invalid tag.
        tag: u8,
        /// Offset where the tag was read.
        offset: usize,
    },
    /// Attribute tags were not strictly increasing.
    AttributesNotOrdered {
        /// Previous attribute tag.
        previous: u8,
        /// Current attribute tag.
        current: u8,
        /// Offset of the current tag.
        offset: usize,
    },
    /// A source event used a negative name reference.
    NegativePositionSource {
        /// Invalid signed reference value.
        value: i32,
    },
    /// A position delta overflowed its coordinate.
    PositionOverflow {
        /// Coordinate that overflowed.
        coordinate: PositionCoordinate,
        /// Previous absolute value.
        previous: i64,
        /// Delta that could not be added.
        delta: i64,
    },
    /// An AST address could not be represented as an unsigned address.
    InvalidAstAddress {
        /// Invalid absolute address.
        address: i64,
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
            Self::InvalidAstAddress { address } => {
                write!(
                    formatter,
                    "AST address {address} does not fit in a TASTy address"
                )
            }
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
    /// Creates a section view over an already bounded payload.
    pub fn new(name: NameRef, payload: &'a [u8]) -> Self {
        Self {
            name,
            offset: 0,
            length: payload.len(),
            payload,
        }
    }

    /// Creates a reader over this section's payload.
    pub fn reader(&self) -> Reader<'a> {
        Reader::new(self.payload)
    }

    /// Decodes an `Attributes` section.
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

    /// Decodes a `Comments` section.
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

    /// Decodes a `Positions` section.
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

    /// Resolves this section's zero-based name index to a standard section.
    pub fn standard_kind(&self, names: &crate::name_table::NameTable) -> Option<StandardSection> {
        let name = match names.get(self.name)? {
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

    /// Re-encode structured AST nodes while relocating references from an
    /// original AST-section layout to the newly emitted layout.
    ///
    /// The original payload is required because structured nodes retain AST
    /// addresses but not the complete address index from which they came.
    /// Relocation is iterative because changing the width of an address can
    /// change the address of a later node. Opaque unknown nodes are rejected
    /// when their position changes because their hidden reference grammar is
    /// not available to the structural encoder.
    pub fn structured_asts_relocated(
        name: NameRef,
        original_payload: &[u8],
        nodes: &[StructuredNode<'_>],
    ) -> Result<EncodedAstSection, TermEncodeError> {
        EncodedAstSection::from_structured_relocated(name, original_payload, nodes)
    }

    /// Returns the zero-based section name index.
    pub fn name(&self) -> NameRef {
        self.name
    }

    /// Borrows the serialized section payload.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Borrow this owned payload as a regular section.
    pub fn section(&self) -> Section<'_> {
        Section::new(self.name, &self.payload)
    }

    /// Consumes the section and returns its name index and payload.
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
            ast_address_map: BTreeMap::new(),
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
            ast_address_map: BTreeMap::new(),
        })
    }

    fn from_structured_relocated(
        name: NameRef,
        original_payload: &[u8],
        nodes: &[StructuredNode<'_>],
    ) -> Result<Self, TermEncodeError> {
        let original_index = ast_index(original_payload)?;
        let original_top_level = top_level_nodes(original_payload)?;
        if original_top_level.len() != nodes.len() {
            return Err(AstError::AstNodeCountMismatch {
                expected: original_top_level.len(),
                actual: nodes.len(),
            }
            .into());
        }

        let original_visible = original_index.iter_nodes().collect::<Vec<_>>();
        let mut address_map = BTreeMap::new();

        for iteration in 0..MAX_AST_RELOCATION_ITERATIONS {
            let bytes = encode_structured_nodes(nodes, &address_map)?;
            let (encoded_nodes, encoded_index) = ast_index_with_nodes(&bytes)?;
            let encoded_visible = encoded_index.iter_nodes().collect::<Vec<_>>();

            if encoded_visible.len() != original_visible.len() {
                return Err(AstError::AstNodeCountMismatch {
                    expected: original_visible.len(),
                    actual: encoded_visible.len(),
                }
                .into());
            }

            for (expected, actual) in original_visible.iter().zip(&encoded_visible) {
                if expected.tag != actual.tag {
                    return Err(AstError::UnexpectedTag {
                        expected: expected.tag,
                        actual: actual.tag,
                        offset: actual.offset,
                    }
                    .into());
                }
            }

            let next_map = original_visible
                .iter()
                .zip(&encoded_visible)
                .filter_map(|(old, new)| {
                    (old.offset != new.offset).then_some((old.offset, new.offset))
                })
                .map(|(old, new)| {
                    Ok((ast_address_from_offset(old)?, ast_address_from_offset(new)?))
                })
                .collect::<Result<BTreeMap<_, _>, TermEncodeError>>()?;

            for node in original_index.iter() {
                let old_address = ast_address_from_offset(node.offset)?;
                let new_address = next_map.get(&old_address).copied().unwrap_or(old_address);
                if !node.is_known() && new_address != old_address {
                    return Err(AstError::UnsupportedRelocation {
                        tag: node.tag,
                        offset: node.offset,
                    }
                    .into());
                }
            }

            if next_map == address_map {
                validate_structured_ast_payload(&bytes)?;
                let ast_addresses = encoded_nodes
                    .iter()
                    .map(|node| ast_address_from_offset(node.offset))
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(Self {
                    section: EncodedSection::raw(name, bytes),
                    ast_addresses,
                    ast_address_map: address_map,
                });
            }

            address_map = next_map;
            if iteration + 1 == MAX_AST_RELOCATION_ITERATIONS {
                return Err(AstError::RelocationDidNotConverge {
                    iterations: MAX_AST_RELOCATION_ITERATIONS,
                }
                .into());
            }
        }

        unreachable!("AST relocation loop always returns or reaches its limit")
    }

    /// Returns the zero-based section name index.
    pub fn name(&self) -> NameRef {
        self.section.name()
    }

    /// Borrows the serialized AST payload.
    pub fn payload(&self) -> &[u8] {
        self.section.payload()
    }

    /// Returns addresses of the emitted top-level AST nodes.
    pub fn ast_addresses(&self) -> &[u32] {
        &self.ast_addresses
    }

    pub(crate) fn ast_address_map(&self) -> &BTreeMap<u32, u32> {
        &self.ast_address_map
    }

    /// Borrows the owned section representation.
    pub fn section(&self) -> &EncodedSection {
        &self.section
    }

    /// Consumes the address-aware section and drops its address map.
    pub fn into_section(self) -> EncodedSection {
        self.section
    }

    /// Consumes the section and returns its payload owner and node addresses.
    pub fn into_parts(self) -> (EncodedSection, Vec<u32>) {
        (self.section, self.ast_addresses)
    }
}

const MAX_AST_RELOCATION_ITERATIONS: usize = 16;

fn encode_structured_nodes(
    nodes: &[StructuredNode<'_>],
    address_map: &BTreeMap<u32, u32>,
) -> Result<Vec<u8>, TermEncodeError> {
    let mut writer = Writer::with_ast_address_map(address_map.clone());
    for node in nodes {
        node.encode(&mut writer)?;
    }
    Ok(writer.into_inner())
}

fn ast_address_from_offset(offset: usize) -> Result<u32, TermEncodeError> {
    u32::try_from(offset).map_err(|_| {
        let value = u64::try_from(offset).unwrap_or(u64::MAX);
        WriteError::NatOverflow { value }.into()
    })
}

fn ast_index<'a>(bytes: &'a [u8]) -> Result<AstAddressIndex<'a>, TermEncodeError> {
    Ok(ast_index_with_nodes(bytes)?.1)
}

fn ast_index_with_nodes<'a>(
    bytes: &'a [u8],
) -> Result<(RawNodes<'a>, AstAddressIndex<'a>), TermEncodeError> {
    let mut reader = Reader::new(bytes);
    let nodes = RawNodes::decode(&mut reader)?;
    if !reader.is_at_end() {
        return Err(AstError::UnsupportedCategory {
            tag: reader.peek_u8().map_err(AstError::from)?,
            offset: reader.position(),
        }
        .into());
    }
    let index =
        nodes.deep_address_index_with_source_and_max_depth(bytes, DEFAULT_MAX_AST_INDEX_DEPTH)?;
    Ok((nodes, index))
}

fn top_level_nodes<'a>(bytes: &'a [u8]) -> Result<RawNodes<'a>, TermEncodeError> {
    Ok(ast_index_with_nodes(bytes)?.0)
}

fn validate_structured_ast_payload(bytes: &[u8]) -> Result<(), TermEncodeError> {
    let mut reader = Reader::new(bytes);
    let nodes = RawNodes::decode(&mut reader)?;
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
    /// Creates a section table from sections already decoded or constructed.
    pub fn from_sections(sections: Vec<Section<'a>>) -> Self {
        Self { sections }
    }

    /// Decodes all sections remaining in the reader.
    pub fn decode(reader: &mut Reader<'a>, name_count: usize) -> Result<Self, SectionError> {
        let mut sections = Vec::new();

        while !reader.is_at_end() {
            let name_offset = reader.position();
            let name = reader.read_nat()?;
            // Like every other name reference in emitted Scala 3.9.0 files,
            // a section name is the zero-based index of a name-table entry.
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

    /// Returns the number of sections.
    pub fn len(&self) -> usize {
        self.sections.len()
    }

    /// Returns whether the table contains no sections.
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// Returns a section by its zero-based table index.
    pub fn get(&self, index: usize) -> Option<&Section<'a>> {
        self.sections.get(index)
    }

    /// Iterates over sections in wire order.
    pub fn iter(&self) -> impl Iterator<Item = &Section<'a>> {
        self.sections.iter()
    }

    /// Borrows all sections in wire order.
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

    /// Encodes all sections in their current order.
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
    use crate::ast::{
        AstError, DEFAULT_MAX_AST_INDEX_DEPTH, PackageNode, RawNode, RawNodes, ReturnNode,
        StructuredNode,
    };
    use crate::reader::{ReadError, Reader};
    use crate::term::{AstRef, AstRefKind, RawTree, TermEncodeError, TermValue};
    use crate::writer::{WriteError, Writer};
    use std::collections::BTreeMap;

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
    fn relocates_position_association_addresses_and_preserves_coordinates() {
        let positions = PositionSection {
            line_sizes: vec![20],
            entries: vec![
                PositionEntry::Source(7),
                PositionEntry::Association {
                    address_delta: 10,
                    start_delta: Some(4),
                    end_delta: Some(9),
                    point_delta: None,
                },
                PositionEntry::Association {
                    address_delta: 3,
                    start_delta: Some(2),
                    end_delta: None,
                    point_delta: Some(-1),
                },
            ],
        };
        let address_map = BTreeMap::from([(10, 100), (13, 109)]);

        let relocated = positions.relocate_ast_addresses(&address_map).unwrap();

        assert_eq!(
            relocated.entries,
            vec![
                PositionEntry::Source(7),
                PositionEntry::Association {
                    address_delta: 100,
                    start_delta: Some(4),
                    end_delta: Some(9),
                    point_delta: None,
                },
                PositionEntry::Association {
                    address_delta: 9,
                    start_delta: Some(2),
                    end_delta: None,
                    point_delta: Some(-1),
                },
            ]
        );
        assert_eq!(
            relocated.resolved_entries().unwrap(),
            vec![
                ResolvedPositionEntry::Source(7),
                ResolvedPositionEntry::Association {
                    address: 100,
                    start: 4,
                    end: 9,
                    point: 0,
                },
                ResolvedPositionEntry::Association {
                    address: 109,
                    start: 6,
                    end: 9,
                    point: -1,
                },
            ]
        );
    }

    #[test]
    fn relocates_comment_ast_addresses_without_changing_comment_metadata() {
        let comments = vec![
            Comment {
                address: 10,
                text: "pierwszy".to_owned(),
                coordinates: -4,
            },
            Comment {
                address: 13,
                text: "drugi".to_owned(),
                coordinates: 19,
            },
        ];
        let address_map = BTreeMap::from([(10, 100), (13, 109)]);

        assert_eq!(
            Comment::relocate_ast_addresses(&comments, &address_map),
            vec![
                Comment {
                    address: 100,
                    text: "pierwszy".to_owned(),
                    coordinates: -4,
                },
                Comment {
                    address: 109,
                    text: "drugi".to_owned(),
                    coordinates: 19,
                },
            ]
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
    fn relocates_structured_ast_references_after_node_growth() {
        fn select_outer_tree(levels: u32) -> RawTree<'static> {
            let mut payload = Writer::new();
            payload.write_nat(levels);
            RawTree::leaf(crate::TERMREFPKG_TAG, TermValue::NameRef(1))
                .unwrap()
                .encode(&mut payload)
                .unwrap();
            RawTree::leaf(crate::TERMREFPKG_TAG, TermValue::NameRef(1))
                .unwrap()
                .encode(&mut payload)
                .unwrap();

            let mut bytes = Writer::new();
            bytes.write_u8(crate::SELECTOUTER_TAG);
            bytes
                .write_length_prefixed_bytes(payload.as_slice())
                .unwrap();
            let leaked = Box::leak(bytes.into_inner().into_boxed_slice());
            let mut reader = Reader::new(leaked);
            RawTree::decode(&mut reader).unwrap()
        }

        let expression = select_outer_tree(127);
        let second = StructuredNode::Package(PackageNode {
            path: RawTree::leaf(crate::TERMREFPKG_TAG, TermValue::NameRef(1)).unwrap(),
            stats: RawNodes::from_entries(Vec::new()).unwrap(),
        });
        let placeholder = StructuredNode::Return(ReturnNode {
            target: AstRef {
                kind: AstRefKind::ReturnTarget,
                address: 0,
            },
            expression: Some(expression),
        });
        let mut placeholder_writer = Writer::new();
        placeholder.encode(&mut placeholder_writer).unwrap();
        let second_address = placeholder_writer.position() as u32;
        second.encode(&mut placeholder_writer).unwrap();

        let original = StructuredNode::Return(ReturnNode {
            target: AstRef {
                kind: AstRefKind::ReturnTarget,
                address: second_address,
            },
            expression: Some(select_outer_tree(127)),
        });
        let mut original_writer = Writer::new();
        original.encode(&mut original_writer).unwrap();
        second.encode(&mut original_writer).unwrap();
        let original_payload = original_writer.into_inner();

        let mut structured = {
            let mut reader = Reader::new(&original_payload);
            let original_nodes = RawNodes::decode(&mut reader).unwrap();
            original_nodes
                .iter()
                .map(|node| node.decode_structured().unwrap())
                .collect::<Vec<_>>()
        };
        let StructuredNode::Return(return_node) = &mut structured[0] else {
            panic!("expected a return node");
        };
        return_node.expression = Some(select_outer_tree(128));

        let encoded =
            EncodedSection::structured_asts_relocated(4, &original_payload, &structured).unwrap();
        assert_eq!(encoded.ast_addresses()[0], 0);
        assert!(encoded.ast_addresses()[1] > second_address);

        let mut reparsed_reader = Reader::new(encoded.payload());
        let reparsed_nodes = RawNodes::decode(&mut reparsed_reader).unwrap();
        let return_node = reparsed_nodes.get(0).unwrap().decode_return().unwrap();
        assert_eq!(return_node.target.address, encoded.ast_addresses()[1]);
        let reparsed_index = reparsed_nodes
            .deep_address_index_with_source_and_max_depth(
                encoded.payload(),
                DEFAULT_MAX_AST_INDEX_DEPTH,
            )
            .unwrap();
        assert!(reparsed_index.resolve_node(return_node.target).is_some());
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
