use crate::reader::{ReadError, Reader};
use crate::writer::{WriteError, Writer};
use std::fmt;

pub type NameRef = u32;
pub type ParamSig = i32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawName {
    Utf8(String),
    Qualified {
        prefix: NameRef,
        selector: NameRef,
    },
    Expanded {
        prefix: NameRef,
        selector: NameRef,
    },
    ExpandPrefix {
        prefix: NameRef,
        selector: NameRef,
    },
    Unique {
        separator: NameRef,
        uniqid: u32,
        underlying: Option<NameRef>,
    },
    DefaultGetter {
        underlying: NameRef,
        index: u32,
    },
    SuperAccessor {
        underlying: NameRef,
    },
    InlineAccessor {
        underlying: NameRef,
    },
    ObjectClass {
        underlying: NameRef,
    },
    BodyRetainer {
        underlying: NameRef,
    },
    Signed {
        original: NameRef,
        result_signature: NameRef,
        parameter_signatures: Vec<ParamSig>,
    },
    TargetSigned {
        original: NameRef,
        target: NameRef,
        result_signature: NameRef,
        parameter_signatures: Vec<ParamSig>,
    },
    Unknown {
        tag: u8,
        payload: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameTable {
    entries: Vec<RawName>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameTableBuilder {
    entries: Vec<RawName>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameTableError {
    Read(ReadError),
    InvalidReference {
        reference: NameRef,
        entry_index: usize,
        entry_count: usize,
    },
    TrailingPayload {
        tag: u8,
        remaining: usize,
    },
    EntryOverflow {
        entry_count: usize,
    },
}

impl fmt::Display for NameTableError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::InvalidReference {
                reference,
                entry_index,
                entry_count,
            } => write!(
                formatter,
                "invalid name reference {reference} in entry {entry_index}; name table contains {entry_count} entries"
            ),
            Self::TrailingPayload { tag, remaining } => write!(
                formatter,
                "name tag {tag} has {remaining} unconsumed payload bytes"
            ),
            Self::EntryOverflow { entry_count } => write!(
                formatter,
                "cannot assign a 1-based NameRef after {entry_count} name entries"
            ),
        }
    }
}

impl std::error::Error for NameTableError {}

impl From<ReadError> for NameTableError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

impl NameTable {
    pub fn builder() -> NameTableBuilder {
        NameTableBuilder::new()
    }

    pub fn from_entries(entries: Vec<RawName>) -> Result<Self, NameTableError> {
        let table = Self { entries };
        table.validate_references()?;
        Ok(table)
    }

    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, NameTableError> {
        let table_length = reader.read_nat()? as usize;
        let mut table_reader = reader.read_sub_reader(table_length)?;
        let mut entries = Vec::new();

        while !table_reader.is_at_end() {
            let tag = table_reader.read_u8()?;
            let name = if tag == 1 {
                RawName::Utf8(table_reader.read_utf8()?)
            } else {
                let payload_length = table_reader.read_nat()? as usize;
                let mut payload = table_reader.read_sub_reader(payload_length)?;
                let name = Self::decode_composite(tag, &mut payload)?;

                if !payload.is_at_end() {
                    return Err(NameTableError::TrailingPayload {
                        tag,
                        remaining: payload.remaining(),
                    });
                }

                name
            };

            entries.push(name);
        }

        let table = Self { entries };
        table.validate_references()?;
        Ok(table)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, reference: NameRef) -> Option<&RawName> {
        reference
            .checked_sub(1)
            .and_then(|index| self.entries.get(index as usize))
    }

    pub(crate) fn get_zero_based(&self, index: NameRef) -> Option<&RawName> {
        self.entries.get(index as usize)
    }

    pub fn entries(&self) -> &[RawName] {
        &self.entries
    }

    pub fn encode(&self, writer: &mut Writer) -> Result<(), WriteError> {
        let mut table = Writer::new();
        for entry in &self.entries {
            encode_name(entry, &mut table)?;
        }
        writer.write_length_prefixed_bytes(table.as_slice())
    }

    fn decode_composite(tag: u8, reader: &mut Reader<'_>) -> Result<RawName, NameTableError> {
        let name = match tag {
            2 => RawName::Qualified {
                prefix: reader.read_nat()?,
                selector: reader.read_nat()?,
            },
            3 => RawName::Expanded {
                prefix: reader.read_nat()?,
                selector: reader.read_nat()?,
            },
            4 => RawName::ExpandPrefix {
                prefix: reader.read_nat()?,
                selector: reader.read_nat()?,
            },
            10 => RawName::Unique {
                separator: reader.read_nat()?,
                uniqid: reader.read_nat()?,
                underlying: if reader.is_at_end() {
                    None
                } else {
                    Some(reader.read_nat()?)
                },
            },
            11 => RawName::DefaultGetter {
                underlying: reader.read_nat()?,
                index: reader.read_nat()?,
            },
            20 => RawName::SuperAccessor {
                underlying: reader.read_nat()?,
            },
            21 => RawName::InlineAccessor {
                underlying: reader.read_nat()?,
            },
            22 => RawName::BodyRetainer {
                underlying: reader.read_nat()?,
            },
            23 => RawName::ObjectClass {
                underlying: reader.read_nat()?,
            },
            62 => RawName::TargetSigned {
                original: reader.read_nat()?,
                target: reader.read_nat()?,
                result_signature: reader.read_nat()?,
                parameter_signatures: read_parameter_signatures(reader)?,
            },
            63 => RawName::Signed {
                original: reader.read_nat()?,
                result_signature: reader.read_nat()?,
                parameter_signatures: read_parameter_signatures(reader)?,
            },
            _ => RawName::Unknown {
                tag,
                payload: {
                    let remaining = reader.remaining();
                    reader.read_bytes(remaining)?.to_vec()
                },
            },
        };

        Ok(name)
    }

    fn validate_references(&self) -> Result<(), NameTableError> {
        for (entry_index, entry) in self.entries.iter().enumerate() {
            let references = entry.references();
            for reference in references {
                if reference == 0 || reference as usize > self.entries.len() {
                    return Err(NameTableError::InvalidReference {
                        reference,
                        entry_index,
                        entry_count: self.entries.len(),
                    });
                }
            }
        }

        Ok(())
    }
}

impl NameTableBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, entry: RawName) -> Result<NameRef, NameTableError> {
        if let Some(index) = self
            .entries
            .iter()
            .position(|candidate| candidate == &entry)
        {
            return NameRef::try_from(index + 1).map_err(|_| NameTableError::EntryOverflow {
                entry_count: self.entries.len(),
            });
        }

        let reference = NameRef::try_from(self.entries.len() + 1).map_err(|_| {
            NameTableError::EntryOverflow {
                entry_count: self.entries.len(),
            }
        })?;
        self.entries.push(entry);
        Ok(reference)
    }

    pub fn finish(self) -> Result<NameTable, NameTableError> {
        NameTable::from_entries(self.entries)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

fn encode_name(name: &RawName, writer: &mut Writer) -> Result<(), WriteError> {
    match name {
        RawName::Utf8(value) => {
            writer.write_u8(1);
            writer.write_utf8(value)?;
        }
        RawName::Qualified { prefix, selector } => {
            encode_composite(
                2,
                |payload| {
                    payload.write_nat(*prefix);
                    payload.write_nat(*selector);
                    Ok(())
                },
                writer,
            )?;
        }
        RawName::Expanded { prefix, selector } => {
            encode_composite(
                3,
                |payload| {
                    payload.write_nat(*prefix);
                    payload.write_nat(*selector);
                    Ok(())
                },
                writer,
            )?;
        }
        RawName::ExpandPrefix { prefix, selector } => {
            encode_composite(
                4,
                |payload| {
                    payload.write_nat(*prefix);
                    payload.write_nat(*selector);
                    Ok(())
                },
                writer,
            )?;
        }
        RawName::Unique {
            separator,
            uniqid,
            underlying,
        } => {
            encode_composite(
                10,
                |payload| {
                    payload.write_nat(*separator);
                    payload.write_nat(*uniqid);
                    if let Some(underlying) = underlying {
                        payload.write_nat(*underlying);
                    }
                    Ok(())
                },
                writer,
            )?;
        }
        RawName::DefaultGetter { underlying, index } => {
            encode_composite(
                11,
                |payload| {
                    payload.write_nat(*underlying);
                    payload.write_nat(*index);
                    Ok(())
                },
                writer,
            )?;
        }
        RawName::SuperAccessor { underlying } => encode_composite(
            20,
            |payload| {
                payload.write_nat(*underlying);
                Ok(())
            },
            writer,
        )?,
        RawName::InlineAccessor { underlying } => encode_composite(
            21,
            |payload| {
                payload.write_nat(*underlying);
                Ok(())
            },
            writer,
        )?,
        RawName::BodyRetainer { underlying } => encode_composite(
            22,
            |payload| {
                payload.write_nat(*underlying);
                Ok(())
            },
            writer,
        )?,
        RawName::ObjectClass { underlying } => encode_composite(
            23,
            |payload| {
                payload.write_nat(*underlying);
                Ok(())
            },
            writer,
        )?,
        RawName::Signed {
            original,
            result_signature,
            parameter_signatures,
        } => encode_composite(
            63,
            |payload| {
                payload.write_nat(*original);
                payload.write_nat(*result_signature);
                for signature in parameter_signatures {
                    payload.write_int(*signature);
                }
                Ok(())
            },
            writer,
        )?,
        RawName::TargetSigned {
            original,
            target,
            result_signature,
            parameter_signatures,
        } => encode_composite(
            62,
            |payload| {
                payload.write_nat(*original);
                payload.write_nat(*target);
                payload.write_nat(*result_signature);
                for signature in parameter_signatures {
                    payload.write_int(*signature);
                }
                Ok(())
            },
            writer,
        )?,
        RawName::Unknown { tag, payload } => {
            writer.write_u8(*tag);
            writer.write_length_prefixed_bytes(payload)?;
        }
    }
    Ok(())
}

fn encode_composite<F>(tag: u8, encode_payload: F, writer: &mut Writer) -> Result<(), WriteError>
where
    F: FnOnce(&mut Writer) -> Result<(), WriteError>,
{
    let mut payload = Writer::new();
    encode_payload(&mut payload)?;
    writer.write_u8(tag);
    writer.write_length_prefixed_bytes(payload.as_slice())
}

impl RawName {
    fn references(&self) -> Vec<NameRef> {
        match self {
            Self::Utf8(_) | Self::Unknown { .. } => Vec::new(),
            Self::Qualified { prefix, selector }
            | Self::Expanded { prefix, selector }
            | Self::ExpandPrefix { prefix, selector } => vec![*prefix, *selector],
            Self::Unique {
                separator,
                underlying,
                ..
            } => underlying.iter().copied().chain([*separator]).collect(),
            Self::DefaultGetter { underlying, .. }
            | Self::SuperAccessor { underlying }
            | Self::InlineAccessor { underlying }
            | Self::ObjectClass { underlying }
            | Self::BodyRetainer { underlying } => vec![*underlying],
            Self::Signed {
                original,
                result_signature,
                parameter_signatures,
            } => std::iter::once(*original)
                .chain(std::iter::once(*result_signature))
                .chain(
                    parameter_signatures
                        .iter()
                        .filter(|signature| **signature > 0)
                        .map(|signature| *signature as NameRef),
                )
                .collect(),
            Self::TargetSigned {
                original,
                target,
                result_signature,
                parameter_signatures,
            } => std::iter::once(*original)
                .chain(std::iter::once(*target))
                .chain(std::iter::once(*result_signature))
                .chain(
                    parameter_signatures
                        .iter()
                        .filter(|signature| **signature > 0)
                        .map(|signature| *signature as NameRef),
                )
                .collect(),
        }
    }
}

fn read_parameter_signatures(reader: &mut Reader<'_>) -> Result<Vec<ParamSig>, NameTableError> {
    let mut signatures = Vec::new();
    while !reader.is_at_end() {
        signatures.push(reader.read_int()?);
    }
    Ok(signatures)
}

#[cfg(test)]
mod tests {
    use super::{NameTable, NameTableError, RawName};
    use crate::reader::Reader;

    #[test]
    fn decodes_names_from_a_fixture() {
        let bytes = include_bytes!("../tests/fixtures/simple_def/SimpleDef.tasty");
        let mut reader = Reader::new(bytes);
        let _header = crate::Header::decode(&mut reader).unwrap();
        let names = NameTable::decode(&mut reader).unwrap();

        assert!(!names.is_empty());
        assert!(
            names
                .entries()
                .iter()
                .any(|name| matches!(name, RawName::Utf8(_)))
        );
    }

    #[test]
    fn rejects_a_reference_outside_the_name_table() {
        // name table length = 4, entry = QUALIFIED with refs 1 and 2
        let bytes = [0x84, 0x02, 0x82, 0x81, 0x82];
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            NameTable::decode(&mut reader),
            Err(NameTableError::InvalidReference {
                reference: 2,
                entry_index: 0,
                entry_count: 1,
            })
        );
    }

    #[test]
    fn constructs_and_validates_name_entries() {
        let names = NameTable::from_entries(vec![
            RawName::Utf8("owner".to_owned()),
            RawName::Qualified {
                prefix: 1,
                selector: 1,
            },
        ])
        .unwrap();

        assert_eq!(names.len(), 2);
        assert_eq!(names.get(2), Some(&names.entries()[1]));
    }

    #[test]
    fn rejects_invalid_references_when_constructing_name_entries() {
        assert_eq!(
            NameTable::from_entries(vec![RawName::Qualified {
                prefix: 1,
                selector: 2,
            }]),
            Err(NameTableError::InvalidReference {
                reference: 2,
                entry_index: 0,
                entry_count: 1,
            })
        );
    }

    #[test]
    fn builder_interns_duplicates_and_preserves_first_seen_order() {
        let mut builder = NameTable::builder();
        assert_eq!(builder.intern(RawName::Utf8("owner".to_owned())), Ok(1));
        assert_eq!(builder.intern(RawName::Utf8("member".to_owned())), Ok(2));
        assert_eq!(builder.intern(RawName::Utf8("owner".to_owned())), Ok(1));
        assert_eq!(builder.len(), 2);

        let names = builder.finish().unwrap();
        assert_eq!(
            names.entries(),
            &[
                RawName::Utf8("owner".to_owned()),
                RawName::Utf8("member".to_owned())
            ]
        );
    }

    #[test]
    fn builder_validates_references_when_finished() {
        let mut builder = NameTable::builder();
        builder
            .intern(RawName::Qualified {
                prefix: 1,
                selector: 2,
            })
            .unwrap();

        assert_eq!(
            builder.finish(),
            Err(NameTableError::InvalidReference {
                reference: 2,
                entry_index: 0,
                entry_count: 1,
            })
        );
    }
}
