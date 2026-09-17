use crate::reader::{ReadError, Reader};
use crate::writer::{WriteError, Writer};
use std::fmt;

pub type NameRef = u32;
pub type ParamSig = i32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamSigValue {
    TypeParameterSectionLength(u32),
    TermParameter(NameRef),
}

/// Interprets a raw `ParamSig` without changing its lossless representation.
///
/// Negative values encode the length of a type-parameter section, while
/// positive values encode a name-table reference for a term parameter. Zero
/// and `i32::MIN` are invalid encodings and return `None`.
pub fn interpret_param_sig(value: ParamSig) -> Option<ParamSigValue> {
    match value {
        i32::MIN | 0 => None,
        value if value < 0 => Some(ParamSigValue::TypeParameterSectionLength(
            value.unsigned_abs(),
        )),
        value => Some(ParamSigValue::TermParameter(value as NameRef)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameSignature {
    pub result: NameRef,
    pub parameters: Vec<ParamSigValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignedName {
    Signed {
        original: NameRef,
        signature: NameSignature,
    },
    TargetSigned {
        original: NameRef,
        target: NameRef,
        signature: NameSignature,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderedParamSig {
    TypeParameterSectionLength(u32),
    TermParameter(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedNameSignature {
    pub result: String,
    pub parameters: Vec<RenderedParamSig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderedSignedName {
    Signed {
        original: String,
        signature: RenderedNameSignature,
    },
    TargetSigned {
        original: String,
        target: String,
        signature: RenderedNameSignature,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawNameKind {
    Utf8,
    Qualified,
    Expanded,
    ExpandPrefix,
    Unique,
    DefaultGetter,
    SuperAccessor,
    InlineAccessor,
    ObjectClass,
    BodyRetainer,
    Signed,
    TargetSigned,
    Unknown,
}

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
    InvalidParamSig {
        value: ParamSig,
        entry_index: usize,
    },
    InvalidTag {
        tag: u8,
        entry_index: usize,
    },
    CyclicReference {
        entry_index: usize,
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
            Self::InvalidParamSig { value, entry_index } => write!(
                formatter,
                "invalid parameter signature {value} in name entry {entry_index}; expected a non-zero signed value"
            ),
            Self::InvalidTag { tag, entry_index } => write!(
                formatter,
                "unknown name entry {entry_index} uses reserved name tag {tag}"
            ),
            Self::CyclicReference { entry_index } => write!(
                formatter,
                "cyclic name reference involving name entry {entry_index}"
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameRenderError {
    InvalidReference {
        reference: NameRef,
    },
    Unsupported {
        reference: NameRef,
        kind: RawNameKind,
    },
}

impl fmt::Display for NameRenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidReference { reference } => {
                write!(formatter, "invalid name reference {reference}")
            }
            Self::Unsupported { reference, kind } => write!(
                formatter,
                "cannot render name reference {reference} of kind {kind:?}"
            ),
        }
    }
}

impl std::error::Error for NameRenderError {}

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

    /// Returns the UTF-8 text stored directly at a name-table reference.
    ///
    /// Composite names intentionally return `None`; callers that need their
    /// structure can inspect [`NameTable::get`] and the public `RawName`
    /// variants instead.
    pub fn get_utf8(&self, reference: NameRef) -> Option<&str> {
        self.get(reference).and_then(RawName::as_utf8)
    }

    /// Returns the first one-based [`NameRef`] whose entry stores `value`
    /// directly as UTF-8.
    ///
    /// Composite names are deliberately not matched by their rendered form.
    /// Callers that need to inspect or resolve those entries should use
    /// [`NameTable::get`] and [`NameTable::dependency_order`].
    pub fn find_utf8(&self, value: &str) -> Option<NameRef> {
        self.entries
            .iter()
            .position(|entry| entry.as_utf8() == Some(value))
            .and_then(|index| NameRef::try_from(index + 1).ok())
    }

    /// Returns the root name and all of its transitive dependencies in
    /// dependency-first order, visiting each name reference at most once.
    ///
    /// The result is structural: it contains [`NameRef`] values rather than
    /// guessing how Scala should render composite names. `None` is returned
    /// when `reference` does not identify an entry in this table.
    ///
    /// Traversal is iterative so a valid but deeply chained name table cannot
    /// overflow the process stack.
    pub fn dependency_order(&self, reference: NameRef) -> Option<Vec<NameRef>> {
        let root = reference.checked_sub(1)? as usize;
        self.entries.get(root)?;

        let mut visited = vec![false; self.entries.len()];
        let mut order = Vec::new();
        let mut stack = vec![(reference, false)];

        while let Some((current, expanded)) = stack.pop() {
            let index = (current - 1) as usize;
            if expanded {
                order.push(current);
                continue;
            }
            if visited[index] {
                continue;
            }

            visited[index] = true;
            stack.push((current, true));

            let mut references = Vec::new();
            self.entries[index].visit_references(&mut |dependency| references.push(dependency));
            for dependency in references.into_iter().rev() {
                let dependency_index = (dependency - 1) as usize;
                if !visited[dependency_index] {
                    stack.push((dependency, false));
                }
            }
        }

        Some(order)
    }

    /// Renders a name entry using the conventional Scala spelling for all
    /// non-signature name kinds.
    ///
    /// Rendering is iterative and therefore safe for deeply chained names.
    /// `SIGNED`, `TARGETSIGNED`, and unknown entries remain structural raw data
    /// and return [`NameRenderError::Unsupported`], because their complete
    /// textual meaning depends on signature-specific compiler semantics.
    pub fn render(&self, reference: NameRef) -> Result<String, NameRenderError> {
        self.render_text(reference, false)
    }

    fn render_text(
        &self,
        reference: NameRef,
        include_signature_entries: bool,
    ) -> Result<String, NameRenderError> {
        let root_index = reference
            .checked_sub(1)
            .and_then(|index| usize::try_from(index).ok())
            .filter(|index| *index < self.entries.len())
            .ok_or(NameRenderError::InvalidReference { reference })?;
        let order = self
            .dependency_order(reference)
            .ok_or(NameRenderError::InvalidReference { reference })?;
        let mut rendered = vec![None; self.entries.len()];

        for current in order {
            let index = (current - 1) as usize;
            let kind = self.entries[index].kind();
            let value = match &self.entries[index] {
                RawName::Utf8(value) => value.clone(),
                RawName::Qualified { prefix, selector } => format!(
                    "{}.{}",
                    rendered_dependency(&rendered, *prefix, current, kind)?,
                    rendered_dependency(&rendered, *selector, current, kind)?
                ),
                RawName::Expanded { prefix, selector } => format!(
                    "{}$${}",
                    rendered_dependency(&rendered, *prefix, current, kind)?,
                    rendered_dependency(&rendered, *selector, current, kind)?
                ),
                RawName::ExpandPrefix { prefix, selector } => format!(
                    "{}${}",
                    rendered_dependency(&rendered, *prefix, current, kind)?,
                    rendered_dependency(&rendered, *selector, current, kind)?
                ),
                RawName::Unique {
                    separator,
                    uniqid,
                    underlying,
                } => {
                    let separator = rendered_dependency(&rendered, *separator, current, kind)?;
                    let underlying = underlying
                        .map(|reference| rendered_dependency(&rendered, reference, current, kind))
                        .transpose()?
                        .unwrap_or_default();
                    if underlying.is_empty() && separator == "$" {
                        format!("${uniqid}$")
                    } else {
                        format!("{underlying}{separator}{uniqid}")
                    }
                }
                RawName::DefaultGetter { underlying, index } => format!(
                    "{}$default${}",
                    rendered_dependency(&rendered, *underlying, current, kind)?,
                    u64::from(*index) + 1
                ),
                RawName::SuperAccessor { underlying } => format!(
                    "super${}",
                    rendered_dependency(&rendered, *underlying, current, kind)?
                ),
                RawName::InlineAccessor { underlying } => format!(
                    "inline${}",
                    rendered_dependency(&rendered, *underlying, current, kind)?
                ),
                RawName::ObjectClass { underlying } => format!(
                    "{}$",
                    rendered_dependency(&rendered, *underlying, current, kind)?
                ),
                RawName::BodyRetainer { underlying } => format!(
                    "{}$retainedBody",
                    rendered_dependency(&rendered, *underlying, current, kind)?
                ),
                RawName::Signed {
                    original,
                    result_signature,
                    parameter_signatures,
                } if include_signature_entries => {
                    let original = rendered_dependency(&rendered, *original, current, kind)?;
                    let signature = rendered_signature_text(
                        &rendered,
                        *result_signature,
                        parameter_signatures,
                        current,
                        kind,
                    )?;
                    format!("{original}[with sig {signature}]")
                }
                RawName::TargetSigned {
                    original,
                    target,
                    result_signature,
                    parameter_signatures,
                } if include_signature_entries => {
                    let original = rendered_dependency(&rendered, *original, current, kind)?;
                    let target = rendered_dependency(&rendered, *target, current, kind)?;
                    let signature = rendered_signature_text(
                        &rendered,
                        *result_signature,
                        parameter_signatures,
                        current,
                        kind,
                    )?;
                    format!("{original}[with sig {signature} @ {target}]")
                }
                RawName::Signed { .. } | RawName::TargetSigned { .. } | RawName::Unknown { .. } => {
                    return Err(NameRenderError::Unsupported {
                        reference: current,
                        kind,
                    });
                }
            };
            rendered[index] = Some(value);
        }

        rendered[root_index]
            .clone()
            .ok_or(NameRenderError::InvalidReference { reference })
    }

    /// Renders a signature-bearing name and all name references contained in
    /// its signature. Returns `Ok(None)` for non-signature entries.
    pub fn render_signed_name(
        &self,
        reference: NameRef,
    ) -> Result<Option<RenderedSignedName>, NameRenderError> {
        let entry = self
            .get(reference)
            .ok_or(NameRenderError::InvalidReference { reference })?;
        let signed = match entry.signed_name() {
            Some(signed) => signed,
            None => return Ok(None),
        };

        let rendered = match signed {
            SignedName::Signed {
                original,
                signature,
            } => RenderedSignedName::Signed {
                original: self.render_text(original, true)?,
                signature: self.render_name_signature(signature, true)?,
            },
            SignedName::TargetSigned {
                original,
                target,
                signature,
            } => RenderedSignedName::TargetSigned {
                original: self.render_text(original, true)?,
                target: self.render_text(target, true)?,
                signature: self.render_name_signature(signature, true)?,
            },
        };

        Ok(Some(rendered))
    }

    fn render_name_signature(
        &self,
        signature: NameSignature,
        include_signature_entries: bool,
    ) -> Result<RenderedNameSignature, NameRenderError> {
        let parameters = signature
            .parameters
            .into_iter()
            .map(|parameter| match parameter {
                ParamSigValue::TypeParameterSectionLength(length) => {
                    Ok(RenderedParamSig::TypeParameterSectionLength(length))
                }
                ParamSigValue::TermParameter(reference) => Ok(RenderedParamSig::TermParameter(
                    self.render_text(reference, include_signature_entries)?,
                )),
            })
            .collect::<Result<Vec<_>, NameRenderError>>()?;

        Ok(RenderedNameSignature {
            result: self.render_text(signature.result, include_signature_entries)?,
            parameters,
        })
    }

    pub(crate) fn get_zero_based(&self, index: NameRef) -> Option<&RawName> {
        self.entries.get(index as usize)
    }

    pub fn entries(&self) -> &[RawName] {
        &self.entries
    }

    /// Iterates over name entries together with their one-based references.
    ///
    /// The order is the original wire order. This avoids requiring callers
    /// to derive a `NameRef` from a zero-based slice index themselves.
    pub fn iter(&self) -> impl Iterator<Item = (NameRef, &RawName)> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                index
                    .checked_add(1)
                    .and_then(|value| NameRef::try_from(value).ok())
                    .map(|reference| (reference, entry))
            })
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
            if let RawName::Unknown { tag, .. } = entry
                && is_known_name_tag(*tag)
            {
                return Err(NameTableError::InvalidTag {
                    tag: *tag,
                    entry_index,
                });
            }

            let parameter_signatures: &[ParamSig] = match entry {
                RawName::Signed {
                    parameter_signatures,
                    ..
                }
                | RawName::TargetSigned {
                    parameter_signatures,
                    ..
                } => parameter_signatures,
                _ => &[],
            };
            for signature in parameter_signatures {
                if interpret_param_sig(*signature).is_none() {
                    return Err(NameTableError::InvalidParamSig {
                        value: *signature,
                        entry_index,
                    });
                }
            }

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

        let mut states = vec![0u8; self.entries.len()];
        for entry_index in 0..self.entries.len() {
            if let Some(cycle_entry) = detect_name_cycle(&self.entries, entry_index, &mut states) {
                return Err(NameTableError::CyclicReference {
                    entry_index: cycle_entry,
                });
            }
        }

        Ok(())
    }
}

fn rendered_dependency(
    rendered: &[Option<String>],
    reference: NameRef,
    owner: NameRef,
    owner_kind: RawNameKind,
) -> Result<String, NameRenderError> {
    let index = reference
        .checked_sub(1)
        .and_then(|index| usize::try_from(index).ok())
        .ok_or(NameRenderError::InvalidReference { reference })?;
    rendered
        .get(index)
        .and_then(Clone::clone)
        .ok_or(NameRenderError::Unsupported {
            reference: owner,
            kind: owner_kind,
        })
}

fn rendered_signature_text(
    rendered: &[Option<String>],
    result: NameRef,
    parameters: &[ParamSig],
    owner: NameRef,
    owner_kind: RawNameKind,
) -> Result<String, NameRenderError> {
    let result = rendered_dependency(rendered, result, owner, owner_kind)?;
    let mut text = result;
    text.push('(');

    for (index, parameter) in parameters.iter().enumerate() {
        if index > 0 {
            text.push_str(", ");
        }

        match interpret_param_sig(*parameter) {
            Some(ParamSigValue::TypeParameterSectionLength(length)) => {
                text.push_str("type-params(");
                text.push_str(&length.to_string());
                text.push(')');
            }
            Some(ParamSigValue::TermParameter(reference)) => {
                text.push_str(&rendered_dependency(
                    rendered, reference, owner, owner_kind,
                )?);
            }
            None => {
                return Err(NameRenderError::Unsupported {
                    reference: owner,
                    kind: owner_kind,
                });
            }
        }
    }

    text.push(')');
    Ok(text)
}

fn detect_name_cycle(entries: &[RawName], entry_index: usize, states: &mut [u8]) -> Option<usize> {
    if states[entry_index] != 0 {
        return (states[entry_index] == 1).then_some(entry_index);
    }

    // Name entries form a graph, and the input is untrusted. Keep the DFS
    // stack on the heap so a long but valid name chain cannot overflow the
    // process stack while it is being validated.
    let mut stack = vec![(entry_index, entries[entry_index].references(), 0usize)];
    states[entry_index] = 1;

    while let Some((current, references, next_reference)) = stack.last_mut() {
        if *next_reference == references.len() {
            states[*current] = 2;
            stack.pop();
            continue;
        }

        let target_index = (references[*next_reference] - 1) as usize;
        *next_reference += 1;
        match states[target_index] {
            0 => {
                states[target_index] = 1;
                stack.push((target_index, entries[target_index].references(), 0));
            }
            1 => return Some(target_index),
            2 => {}
            _ => unreachable!("name validation states are only 0, 1, or 2"),
        }
    }

    None
}

fn is_known_name_tag(tag: u8) -> bool {
    matches!(tag, 1 | 2 | 3 | 4 | 10 | 11 | 20..=23 | 62..=63)
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

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
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
    /// Returns the wire-level variant of this name entry without exposing its
    /// payload. Use the matching [`RawName`] variant to inspect references or
    /// preserve the entry's complete data.
    pub fn kind(&self) -> RawNameKind {
        match self {
            Self::Utf8(_) => RawNameKind::Utf8,
            Self::Qualified { .. } => RawNameKind::Qualified,
            Self::Expanded { .. } => RawNameKind::Expanded,
            Self::ExpandPrefix { .. } => RawNameKind::ExpandPrefix,
            Self::Unique { .. } => RawNameKind::Unique,
            Self::DefaultGetter { .. } => RawNameKind::DefaultGetter,
            Self::SuperAccessor { .. } => RawNameKind::SuperAccessor,
            Self::InlineAccessor { .. } => RawNameKind::InlineAccessor,
            Self::ObjectClass { .. } => RawNameKind::ObjectClass,
            Self::BodyRetainer { .. } => RawNameKind::BodyRetainer,
            Self::Signed { .. } => RawNameKind::Signed,
            Self::TargetSigned { .. } => RawNameKind::TargetSigned,
            Self::Unknown { .. } => RawNameKind::Unknown,
        }
    }

    /// Returns a typed view of a `SIGNED` or `TARGETSIGNED` entry.
    ///
    /// The raw signed parameter values remain available in the enum itself.
    /// `None` means that this entry is not signature-bearing or contains an
    /// invalid raw `ParamSig` value.
    pub fn signed_name(&self) -> Option<SignedName> {
        let signature = |result, parameter_signatures: &[ParamSig]| {
            Some(NameSignature {
                result,
                parameters: parameter_signatures
                    .iter()
                    .copied()
                    .map(interpret_param_sig)
                    .collect::<Option<Vec<_>>>()?,
            })
        };

        match self {
            Self::Signed {
                original,
                result_signature,
                parameter_signatures,
            } => Some(SignedName::Signed {
                original: *original,
                signature: signature(*result_signature, parameter_signatures)?,
            }),
            Self::TargetSigned {
                original,
                target,
                result_signature,
                parameter_signatures,
            } => Some(SignedName::TargetSigned {
                original: *original,
                target: *target,
                signature: signature(*result_signature, parameter_signatures)?,
            }),
            _ => None,
        }
    }

    /// Returns the text of a direct UTF-8 name entry.
    pub fn as_utf8(&self) -> Option<&str> {
        match self {
            Self::Utf8(value) => Some(value),
            _ => None,
        }
    }

    /// Visits name-table references contained in this entry in wire order.
    ///
    /// This is useful for dependency inspection without allocating a
    /// temporary collection. Positive `ParamSig` values are included because
    /// they are name references; negative values encode type-parameter
    /// section lengths and are therefore omitted.
    pub fn visit_references(&self, visitor: &mut impl FnMut(NameRef)) {
        match self {
            Self::Utf8(_) | Self::Unknown { .. } => {}
            Self::Qualified { prefix, selector }
            | Self::Expanded { prefix, selector }
            | Self::ExpandPrefix { prefix, selector } => {
                visitor(*prefix);
                visitor(*selector);
            }
            Self::Unique {
                separator,
                underlying,
                ..
            } => {
                visitor(*separator);
                if let Some(underlying) = underlying {
                    visitor(*underlying);
                }
            }
            Self::DefaultGetter { underlying, .. }
            | Self::SuperAccessor { underlying }
            | Self::InlineAccessor { underlying }
            | Self::ObjectClass { underlying }
            | Self::BodyRetainer { underlying } => visitor(*underlying),
            Self::Signed {
                original,
                result_signature,
                parameter_signatures,
            } => {
                visitor(*original);
                visitor(*result_signature);
                for signature in parameter_signatures {
                    if let Some(ParamSigValue::TermParameter(reference)) =
                        interpret_param_sig(*signature)
                    {
                        visitor(reference);
                    }
                }
            }
            Self::TargetSigned {
                original,
                target,
                result_signature,
                parameter_signatures,
            } => {
                visitor(*original);
                visitor(*target);
                visitor(*result_signature);
                for signature in parameter_signatures {
                    if let Some(ParamSigValue::TermParameter(reference)) =
                        interpret_param_sig(*signature)
                    {
                        visitor(reference);
                    }
                }
            }
        }
    }

    fn references(&self) -> Vec<NameRef> {
        let mut references = Vec::new();
        self.visit_references(&mut |reference| references.push(reference));
        references
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
    use super::{
        NameRenderError, NameSignature, NameTable, NameTableError, ParamSigValue, RawName,
        RawNameKind, RenderedNameSignature, RenderedParamSig, RenderedSignedName, SignedName,
        interpret_param_sig,
    };
    use crate::reader::{ReadError, Reader};

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
    fn name_table_length_counts_the_complete_utf8_byte_region() {
        let mut reader = Reader::new(&[
            0x8c, // name-table payload length: tag + UTF-8 length + 10 bytes
            1, 0x8a, b'z', b'a', 0xc5, 0xbc, 0xc3, 0xb3, 0xc5, 0x82, 0xc4, 0x87,
            0xa5, // bytes after the bounded name table, e.g. a section header
        ]);

        let names = NameTable::decode(&mut reader).unwrap();

        assert_eq!(names.get_utf8(1), Some("zażółć"));
        assert_eq!(reader.position(), 13);
        assert_eq!(reader.read_u8().unwrap(), 0xa5);
        assert!(reader.is_at_end());
    }

    #[test]
    fn classifies_utf8_names() {
        assert_eq!(RawName::Utf8("name".to_owned()).kind(), RawNameKind::Utf8);
    }

    #[test]
    fn classifies_qualified_names() {
        assert_eq!(
            RawName::Qualified {
                prefix: 1,
                selector: 1,
            }
            .kind(),
            RawNameKind::Qualified
        );
    }

    #[test]
    fn classifies_expanded_names() {
        assert_eq!(
            RawName::Expanded {
                prefix: 1,
                selector: 1,
            }
            .kind(),
            RawNameKind::Expanded
        );
    }

    #[test]
    fn classifies_expand_prefix_names() {
        assert_eq!(
            RawName::ExpandPrefix {
                prefix: 1,
                selector: 1,
            }
            .kind(),
            RawNameKind::ExpandPrefix
        );
    }

    #[test]
    fn classifies_unique_names() {
        assert_eq!(
            RawName::Unique {
                separator: 1,
                uniqid: 1,
                underlying: None,
            }
            .kind(),
            RawNameKind::Unique
        );
    }

    #[test]
    fn classifies_default_getter_names() {
        assert_eq!(
            RawName::DefaultGetter {
                underlying: 1,
                index: 1,
            }
            .kind(),
            RawNameKind::DefaultGetter
        );
    }

    #[test]
    fn classifies_super_accessor_names() {
        assert_eq!(
            RawName::SuperAccessor { underlying: 1 }.kind(),
            RawNameKind::SuperAccessor
        );
    }

    #[test]
    fn classifies_inline_accessor_names() {
        assert_eq!(
            RawName::InlineAccessor { underlying: 1 }.kind(),
            RawNameKind::InlineAccessor
        );
    }

    #[test]
    fn classifies_object_class_names() {
        assert_eq!(
            RawName::ObjectClass { underlying: 1 }.kind(),
            RawNameKind::ObjectClass
        );
    }

    #[test]
    fn classifies_body_retainer_names() {
        assert_eq!(
            RawName::BodyRetainer { underlying: 1 }.kind(),
            RawNameKind::BodyRetainer
        );
    }

    #[test]
    fn classifies_signed_names() {
        assert_eq!(
            RawName::Signed {
                original: 1,
                result_signature: 1,
                parameter_signatures: vec![],
            }
            .kind(),
            RawNameKind::Signed
        );
    }

    #[test]
    fn classifies_target_signed_names() {
        assert_eq!(
            RawName::TargetSigned {
                original: 1,
                target: 1,
                result_signature: 1,
                parameter_signatures: vec![],
            }
            .kind(),
            RawNameKind::TargetSigned
        );
    }

    #[test]
    fn classifies_unknown_names() {
        assert_eq!(
            RawName::Unknown {
                tag: 99,
                payload: vec![],
            }
            .kind(),
            RawNameKind::Unknown
        );
    }

    #[test]
    fn interprets_a_positive_parameter_signature_as_a_term_parameter() {
        assert_eq!(
            interpret_param_sig(17),
            Some(ParamSigValue::TermParameter(17))
        );
    }

    #[test]
    fn interprets_the_largest_positive_parameter_signature() {
        assert_eq!(
            interpret_param_sig(i32::MAX),
            Some(ParamSigValue::TermParameter(i32::MAX as u32))
        );
    }

    #[test]
    fn interprets_a_negative_parameter_signature_as_a_type_section_length() {
        assert_eq!(
            interpret_param_sig(-17),
            Some(ParamSigValue::TypeParameterSectionLength(17))
        );
    }

    #[test]
    fn interprets_minus_one_as_a_one_element_type_section_length_marker() {
        assert_eq!(
            interpret_param_sig(-1),
            Some(ParamSigValue::TypeParameterSectionLength(1))
        );
    }

    #[test]
    fn rejects_zero_as_a_parameter_signature() {
        assert_eq!(interpret_param_sig(0), None);
    }

    #[test]
    fn rejects_the_minimum_i32_as_a_parameter_signature() {
        assert_eq!(interpret_param_sig(i32::MIN), None);
    }

    #[test]
    fn exposes_a_typed_signed_name_without_parameters() {
        let name = RawName::Signed {
            original: 7,
            result_signature: 11,
            parameter_signatures: vec![],
        };

        assert_eq!(
            name.signed_name(),
            Some(SignedName::Signed {
                original: 7,
                signature: NameSignature {
                    result: 11,
                    parameters: vec![],
                },
            })
        );
    }

    #[test]
    fn exposes_typed_parameter_sections_and_term_parameters_in_wire_order() {
        let name = RawName::Signed {
            original: 7,
            result_signature: 11,
            parameter_signatures: vec![-2, 13, -1, 17],
        };

        assert_eq!(
            name.signed_name(),
            Some(SignedName::Signed {
                original: 7,
                signature: NameSignature {
                    result: 11,
                    parameters: vec![
                        ParamSigValue::TypeParameterSectionLength(2),
                        ParamSigValue::TermParameter(13),
                        ParamSigValue::TypeParameterSectionLength(1),
                        ParamSigValue::TermParameter(17),
                    ],
                },
            })
        );
    }

    #[test]
    fn exposes_the_target_of_a_target_signed_name() {
        let name = RawName::TargetSigned {
            original: 7,
            target: 11,
            result_signature: 13,
            parameter_signatures: vec![17],
        };

        assert_eq!(
            name.signed_name(),
            Some(SignedName::TargetSigned {
                original: 7,
                target: 11,
                signature: NameSignature {
                    result: 13,
                    parameters: vec![ParamSigValue::TermParameter(17)],
                },
            })
        );
    }

    #[test]
    fn does_not_expose_a_signature_for_another_name_kind() {
        assert_eq!(RawName::Utf8("name".to_owned()).signed_name(), None);
    }

    #[test]
    fn rejects_a_typed_view_of_a_malformed_signed_name() {
        let name = RawName::Signed {
            original: 1,
            result_signature: 1,
            parameter_signatures: vec![0],
        };

        assert_eq!(name.signed_name(), None);
    }

    #[test]
    fn renders_a_signed_name_signature_and_term_parameter() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("method".to_owned()),
            RawName::Utf8("result".to_owned()),
            RawName::Utf8("parameter".to_owned()),
            RawName::Signed {
                original: 1,
                result_signature: 2,
                parameter_signatures: vec![-2, 3],
            },
        ])
        .unwrap();

        assert_eq!(
            table.render_signed_name(4),
            Ok(Some(RenderedSignedName::Signed {
                original: "method".to_owned(),
                signature: RenderedNameSignature {
                    result: "result".to_owned(),
                    parameters: vec![
                        RenderedParamSig::TypeParameterSectionLength(2),
                        RenderedParamSig::TermParameter("parameter".to_owned()),
                    ],
                },
            }))
        );
    }

    #[test]
    fn renders_a_target_signed_name_target_and_signature() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("method".to_owned()),
            RawName::Utf8("target".to_owned()),
            RawName::Utf8("result".to_owned()),
            RawName::TargetSigned {
                original: 1,
                target: 2,
                result_signature: 3,
                parameter_signatures: vec![],
            },
        ])
        .unwrap();

        assert_eq!(
            table.render_signed_name(4),
            Ok(Some(RenderedSignedName::TargetSigned {
                original: "method".to_owned(),
                target: "target".to_owned(),
                signature: RenderedNameSignature {
                    result: "result".to_owned(),
                    parameters: vec![],
                },
            }))
        );
    }

    #[test]
    fn renders_a_signed_name_with_a_signed_original() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("method".to_owned()),
            RawName::Utf8("result".to_owned()),
            RawName::Signed {
                original: 1,
                result_signature: 2,
                parameter_signatures: vec![],
            },
            RawName::Signed {
                original: 3,
                result_signature: 2,
                parameter_signatures: vec![],
            },
        ])
        .unwrap();

        assert_eq!(
            table.render_signed_name(4),
            Ok(Some(RenderedSignedName::Signed {
                original: "method[with sig result()]".to_owned(),
                signature: RenderedNameSignature {
                    result: "result".to_owned(),
                    parameters: vec![],
                },
            }))
        );
    }

    #[test]
    fn renders_a_target_signed_name_with_a_signed_target() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("method".to_owned()),
            RawName::Utf8("target".to_owned()),
            RawName::Utf8("result".to_owned()),
            RawName::Signed {
                original: 1,
                result_signature: 3,
                parameter_signatures: vec![],
            },
            RawName::TargetSigned {
                original: 1,
                target: 4,
                result_signature: 3,
                parameter_signatures: vec![],
            },
        ])
        .unwrap();

        assert_eq!(
            table.render_signed_name(5),
            Ok(Some(RenderedSignedName::TargetSigned {
                original: "method".to_owned(),
                target: "method[with sig result()]".to_owned(),
                signature: RenderedNameSignature {
                    result: "result".to_owned(),
                    parameters: vec![],
                },
            }))
        );
    }

    #[test]
    fn renders_a_deep_signed_name_chain_without_recursion() {
        let depth = 256;
        let mut entries = vec![
            RawName::Utf8("method".to_owned()),
            RawName::Utf8("result".to_owned()),
        ];

        let mut root = 1;
        for _ in 0..depth {
            let reference = entries.len() as u32 + 1;
            entries.push(RawName::Signed {
                original: root,
                result_signature: 2,
                parameter_signatures: vec![],
            });
            root = reference;
        }

        let table = NameTable::from_entries(entries).unwrap();
        let rendered = table.render_signed_name(root).unwrap().unwrap();

        let RenderedSignedName::Signed { original, .. } = rendered else {
            panic!("expected a signed rendered name");
        };
        assert_eq!(original.matches("[with sig result()]").count(), depth - 1);
    }

    #[test]
    fn returns_no_rendered_signature_for_a_non_signature_name() {
        let table = NameTable::from_entries(vec![RawName::Utf8("name".to_owned())]).unwrap();

        assert_eq!(table.render_signed_name(1), Ok(None));
    }

    #[test]
    fn renders_a_direct_utf8_name() {
        let table = NameTable::from_entries(vec![RawName::Utf8("name".to_owned())]).unwrap();

        assert_eq!(table.render(1), Ok("name".to_owned()));
    }

    #[test]
    fn renders_a_qualified_name() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("owner".to_owned()),
            RawName::Utf8("member".to_owned()),
            RawName::Qualified {
                prefix: 1,
                selector: 2,
            },
        ])
        .unwrap();

        assert_eq!(table.render(3), Ok("owner.member".to_owned()));
    }

    #[test]
    fn renders_an_expanded_name() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("owner".to_owned()),
            RawName::Utf8("member".to_owned()),
            RawName::Expanded {
                prefix: 1,
                selector: 2,
            },
        ])
        .unwrap();

        assert_eq!(table.render(3), Ok("owner$$member".to_owned()));
    }

    #[test]
    fn renders_an_expand_prefix_name() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("owner".to_owned()),
            RawName::Utf8("member".to_owned()),
            RawName::ExpandPrefix {
                prefix: 1,
                selector: 2,
            },
        ])
        .unwrap();

        assert_eq!(table.render(3), Ok("owner$member".to_owned()));
    }

    #[test]
    fn renders_a_unique_name_with_an_underlying_name() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("owner".to_owned()),
            RawName::Utf8("$".to_owned()),
            RawName::Unique {
                separator: 2,
                uniqid: 7,
                underlying: Some(1),
            },
        ])
        .unwrap();

        assert_eq!(table.render(3), Ok("owner$7".to_owned()));
    }

    #[test]
    fn renders_an_empty_unique_name_with_scala_dollar_delimiters() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("$".to_owned()),
            RawName::Unique {
                separator: 1,
                uniqid: 7,
                underlying: None,
            },
        ])
        .unwrap();

        assert_eq!(table.render(2), Ok("$7$".to_owned()));
    }

    #[test]
    fn renders_a_default_getter_name_with_a_one_based_index() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("method".to_owned()),
            RawName::DefaultGetter {
                underlying: 1,
                index: 0,
            },
        ])
        .unwrap();

        assert_eq!(table.render(2), Ok("method$default$1".to_owned()));
    }

    #[test]
    fn renders_a_super_accessor_name() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("method".to_owned()),
            RawName::SuperAccessor { underlying: 1 },
        ])
        .unwrap();

        assert_eq!(table.render(2), Ok("super$method".to_owned()));
    }

    #[test]
    fn renders_an_inline_accessor_name() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("method".to_owned()),
            RawName::InlineAccessor { underlying: 1 },
        ])
        .unwrap();

        assert_eq!(table.render(2), Ok("inline$method".to_owned()));
    }

    #[test]
    fn renders_a_body_retainer_name() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("method".to_owned()),
            RawName::BodyRetainer { underlying: 1 },
        ])
        .unwrap();

        assert_eq!(table.render(2), Ok("method$retainedBody".to_owned()));
    }

    #[test]
    fn renders_an_object_class_name() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("module".to_owned()),
            RawName::ObjectClass { underlying: 1 },
        ])
        .unwrap();

        assert_eq!(table.render(2), Ok("module$".to_owned()));
    }

    #[test]
    fn rejects_rendering_a_signed_name() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("method".to_owned()),
            RawName::Signed {
                original: 1,
                result_signature: 1,
                parameter_signatures: vec![],
            },
        ])
        .unwrap();

        assert_eq!(
            table.render(2),
            Err(NameRenderError::Unsupported {
                reference: 2,
                kind: RawNameKind::Signed,
            })
        );
    }

    #[test]
    fn rejects_rendering_a_target_signed_name() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("method".to_owned()),
            RawName::TargetSigned {
                original: 1,
                target: 1,
                result_signature: 1,
                parameter_signatures: vec![],
            },
        ])
        .unwrap();

        assert_eq!(
            table.render(2),
            Err(NameRenderError::Unsupported {
                reference: 2,
                kind: RawNameKind::TargetSigned,
            })
        );
    }

    #[test]
    fn rejects_rendering_an_unknown_name() {
        let table = NameTable::from_entries(vec![RawName::Unknown {
            tag: 99,
            payload: vec![],
        }])
        .unwrap();

        assert_eq!(
            table.render(1),
            Err(NameRenderError::Unsupported {
                reference: 1,
                kind: RawNameKind::Unknown,
            })
        );
    }

    #[test]
    fn rejects_rendering_an_invalid_name_reference() {
        let table = NameTable::from_entries(vec![RawName::Utf8("name".to_owned())]).unwrap();

        assert_eq!(
            table.render(0),
            Err(NameRenderError::InvalidReference { reference: 0 })
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
    fn rejects_a_truncated_name_table_payload() {
        assert_unexpected_eof(&[0x82]);
    }

    #[test]
    fn rejects_a_maximal_name_table_length_without_allocating_it() {
        let bytes = [0x0f, 0x7f, 0x7f, 0x7f, 0xff];
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            NameTable::decode(&mut reader),
            Err(NameTableError::Read(ReadError::UnexpectedEof {
                offset: 5,
                needed: u32::MAX as usize,
                remaining: 0,
            }))
        );
    }

    #[test]
    fn rejects_a_truncated_utf8_name_payload() {
        assert_rejects_truncated_entry(RawName::Utf8("nested".to_owned()));
    }

    #[test]
    fn rejects_a_truncated_qualified_name_payload() {
        assert_rejects_truncated_entry(RawName::Qualified {
            prefix: 1,
            selector: 2,
        });
    }

    #[test]
    fn rejects_a_truncated_expanded_name_payload() {
        assert_rejects_truncated_entry(RawName::Expanded {
            prefix: 1,
            selector: 2,
        });
    }

    #[test]
    fn rejects_a_truncated_expand_prefix_name_payload() {
        assert_rejects_truncated_entry(RawName::ExpandPrefix {
            prefix: 1,
            selector: 2,
        });
    }

    #[test]
    fn rejects_a_truncated_unique_name_payload() {
        assert_rejects_truncated_entry(RawName::Unique {
            separator: 2,
            uniqid: 7,
            underlying: None,
        });
    }

    #[test]
    fn rejects_a_truncated_default_getter_name_payload() {
        assert_rejects_truncated_entry(RawName::DefaultGetter {
            underlying: 1,
            index: 2,
        });
    }

    #[test]
    fn rejects_a_truncated_super_accessor_name_payload() {
        assert_rejects_truncated_entry(RawName::SuperAccessor { underlying: 1 });
    }

    #[test]
    fn rejects_a_truncated_inline_accessor_name_payload() {
        assert_rejects_truncated_entry(RawName::InlineAccessor { underlying: 1 });
    }

    #[test]
    fn rejects_a_truncated_body_retainer_name_payload() {
        assert_rejects_truncated_entry(RawName::BodyRetainer { underlying: 1 });
    }

    #[test]
    fn rejects_a_truncated_object_class_name_payload() {
        assert_rejects_truncated_entry(RawName::ObjectClass { underlying: 1 });
    }

    #[test]
    fn rejects_a_truncated_signed_name_payload() {
        assert_rejects_truncated_entry(RawName::Signed {
            original: 1,
            result_signature: 1,
            parameter_signatures: vec![2],
        });
    }

    #[test]
    fn rejects_a_truncated_target_signed_name_payload() {
        assert_rejects_truncated_entry(RawName::TargetSigned {
            original: 1,
            target: 1,
            result_signature: 1,
            parameter_signatures: vec![2],
        });
    }

    #[test]
    fn rejects_a_truncated_unknown_name_payload() {
        assert_rejects_truncated_entry(RawName::Unknown {
            tag: 99,
            payload: vec![0x42],
        });
    }

    #[test]
    fn rejects_trailing_payload_after_a_qualified_name() {
        let bytes = [0x85, 2, 0x83, 0x81, 0x82, 0x80];
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            NameTable::decode(&mut reader),
            Err(NameTableError::TrailingPayload {
                tag: 2,
                remaining: 1,
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
    fn iterates_name_entries_with_one_based_references() {
        let names = NameTable::from_entries(vec![
            RawName::Utf8("foo".to_owned()),
            RawName::Utf8("bar".to_owned()),
        ])
        .unwrap();

        assert_eq!(
            names
                .iter()
                .map(|(reference, entry)| (reference, entry.as_utf8().unwrap()))
                .collect::<Vec<_>>(),
            vec![(1, "foo"), (2, "bar")]
        );
    }

    #[test]
    fn iterates_no_entries_for_an_empty_name_table() {
        let names = NameTable::from_entries(Vec::new()).unwrap();

        assert_eq!(names.iter().count(), 0);
    }

    #[test]
    fn resolves_direct_utf8_name_references() {
        let table = NameTable::from_entries(vec![RawName::Utf8("member".to_owned())]).unwrap();

        assert_eq!(table.get_utf8(1), Some("member"));
        assert_eq!(table.get_utf8(0), None);
        assert_eq!(table.get_utf8(2), None);
    }

    #[test]
    fn finds_a_direct_utf8_name_reference() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("owner".to_owned()),
            RawName::Utf8("member".to_owned()),
        ])
        .unwrap();

        assert_eq!(table.find_utf8("member"), Some(2));
    }

    #[test]
    fn returns_none_when_a_utf8_name_is_missing() {
        let table = NameTable::from_entries(vec![RawName::Utf8("owner".to_owned())]).unwrap();

        assert_eq!(table.find_utf8("missing"), None);
    }

    #[test]
    fn returns_the_first_reference_for_duplicate_utf8_names() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("member".to_owned()),
            RawName::Utf8("member".to_owned()),
        ])
        .unwrap();

        assert_eq!(table.find_utf8("member"), Some(1));
    }

    #[test]
    fn does_not_find_composite_names_by_rendered_text() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("owner".to_owned()),
            RawName::Utf8("member".to_owned()),
            RawName::Qualified {
                prefix: 1,
                selector: 2,
            },
        ])
        .unwrap();

        assert_eq!(table.find_utf8("owner.member"), None);
    }

    #[test]
    fn visits_qualified_name_references_in_wire_order_without_allocating() {
        let name = RawName::Qualified {
            prefix: 7,
            selector: 3,
        };
        let mut references = Vec::new();

        name.visit_references(&mut |reference| references.push(reference));

        assert_eq!(references, vec![7, 3]);
    }

    #[test]
    fn visits_no_references_for_a_direct_utf8_name() {
        let name = RawName::Utf8("member".to_owned());
        let mut visited = false;

        name.visit_references(&mut |_| visited = true);

        assert!(!visited);
    }

    #[test]
    fn visits_no_references_for_an_unknown_name() {
        let name = RawName::Unknown {
            tag: 99,
            payload: vec![1, 2, 3],
        };
        let mut references = Vec::new();

        name.visit_references(&mut |reference| references.push(reference));

        assert!(references.is_empty());
    }

    #[test]
    fn visits_expanded_name_references_in_wire_order() {
        let name = RawName::Expanded {
            prefix: 7,
            selector: 3,
        };
        let mut references = Vec::new();

        name.visit_references(&mut |reference| references.push(reference));

        assert_eq!(references, vec![7, 3]);
    }

    #[test]
    fn visits_expand_prefix_name_references_in_wire_order() {
        let name = RawName::ExpandPrefix {
            prefix: 7,
            selector: 3,
        };
        let mut references = Vec::new();

        name.visit_references(&mut |reference| references.push(reference));

        assert_eq!(references, vec![7, 3]);
    }

    #[test]
    fn visits_a_unique_name_separator_before_its_underlying_name() {
        let name = RawName::Unique {
            separator: 7,
            uniqid: 3,
            underlying: Some(11),
        };
        let mut references = Vec::new();

        name.visit_references(&mut |reference| references.push(reference));

        assert_eq!(references, vec![7, 11]);
    }

    #[test]
    fn visits_a_unique_name_separator_without_an_underlying_name() {
        let name = RawName::Unique {
            separator: 7,
            uniqid: 3,
            underlying: None,
        };
        let mut references = Vec::new();

        name.visit_references(&mut |reference| references.push(reference));

        assert_eq!(references, vec![7]);
    }

    #[test]
    fn visits_default_getter_underlying_reference() {
        let name = RawName::DefaultGetter {
            underlying: 7,
            index: 3,
        };
        let mut references = Vec::new();

        name.visit_references(&mut |reference| references.push(reference));

        assert_eq!(references, vec![7]);
    }

    #[test]
    fn visits_super_accessor_underlying_reference() {
        let name = RawName::SuperAccessor { underlying: 7 };
        let mut references = Vec::new();

        name.visit_references(&mut |reference| references.push(reference));

        assert_eq!(references, vec![7]);
    }

    #[test]
    fn visits_inline_accessor_underlying_reference() {
        let name = RawName::InlineAccessor { underlying: 7 };
        let mut references = Vec::new();

        name.visit_references(&mut |reference| references.push(reference));

        assert_eq!(references, vec![7]);
    }

    #[test]
    fn visits_body_retainer_underlying_reference() {
        let name = RawName::BodyRetainer { underlying: 7 };
        let mut references = Vec::new();

        name.visit_references(&mut |reference| references.push(reference));

        assert_eq!(references, vec![7]);
    }

    #[test]
    fn visits_object_class_underlying_reference() {
        let name = RawName::ObjectClass { underlying: 7 };
        let mut references = Vec::new();

        name.visit_references(&mut |reference| references.push(reference));

        assert_eq!(references, vec![7]);
    }

    #[test]
    fn visits_signed_name_positive_parameter_references_only() {
        let name = RawName::Signed {
            original: 7,
            result_signature: 11,
            parameter_signatures: vec![-2, 13, -1, 17],
        };
        let mut references = Vec::new();

        name.visit_references(&mut |reference| references.push(reference));

        assert_eq!(references, vec![7, 11, 13, 17]);
    }

    #[test]
    fn visits_target_signed_name_references_in_wire_order() {
        let name = RawName::TargetSigned {
            original: 7,
            target: 11,
            result_signature: 13,
            parameter_signatures: vec![-2, 17],
        };
        let mut references = Vec::new();

        name.visit_references(&mut |reference| references.push(reference));

        assert_eq!(references, vec![7, 11, 13, 17]);
    }

    #[test]
    fn does_not_resolve_a_composite_name_as_direct_utf8() {
        let table = NameTable::from_entries(vec![
            RawName::Utf8("owner".to_owned()),
            RawName::Utf8("member".to_owned()),
            RawName::Qualified {
                prefix: 1,
                selector: 2,
            },
        ])
        .unwrap();

        assert_eq!(table.get_utf8(3), None);
        assert_eq!(table.get(3).and_then(RawName::as_utf8), None);
    }

    #[test]
    fn returns_the_root_for_a_direct_name_dependency_order() {
        let table = NameTable::from_entries(vec![RawName::Utf8("member".to_owned())]).unwrap();

        assert_eq!(table.dependency_order(1), Some(vec![1]));
    }

    #[test]
    fn returns_unique_name_dependencies_before_the_root() {
        let table = NameTable::from_entries(vec![
            RawName::Qualified {
                prefix: 2,
                selector: 3,
            },
            RawName::Qualified {
                prefix: 4,
                selector: 4,
            },
            RawName::Utf8("member".to_owned()),
            RawName::Utf8("owner".to_owned()),
        ])
        .unwrap();

        assert_eq!(table.dependency_order(1), Some(vec![4, 2, 3, 1]));
    }

    #[test]
    fn returns_none_for_an_unknown_name_dependency_order_root() {
        let table = NameTable::from_entries(vec![RawName::Utf8("member".to_owned())]).unwrap();

        assert_eq!(table.dependency_order(0), None);
        assert_eq!(table.dependency_order(2), None);
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
    fn rejects_cyclic_name_references_when_constructing_name_entries() {
        assert_eq!(
            NameTable::from_entries(vec![RawName::Qualified {
                prefix: 1,
                selector: 1,
            }]),
            Err(NameTableError::CyclicReference { entry_index: 0 })
        );
    }

    #[test]
    fn rejects_cyclic_name_references_when_decoding() {
        let mut reader = Reader::new(&[0x84, 2, 0x82, 0x81, 0x81]);

        assert_eq!(
            NameTable::decode(&mut reader),
            Err(NameTableError::CyclicReference { entry_index: 0 })
        );
    }

    #[test]
    fn rejects_a_multi_entry_name_cycle_when_decoding() {
        let mut reader = Reader::new(&[0x88, 2, 0x82, 0x82, 0x81, 2, 0x82, 0x81, 0x82]);

        assert_eq!(
            NameTable::decode(&mut reader),
            Err(NameTableError::CyclicReference { entry_index: 0 })
        );
    }

    #[test]
    fn rejects_a_cycle_reached_through_a_signed_parameter_reference() {
        assert_eq!(
            NameTable::from_entries(vec![
                RawName::Utf8("owner".to_owned()),
                RawName::Signed {
                    original: 1,
                    result_signature: 1,
                    parameter_signatures: vec![3],
                },
                RawName::Qualified {
                    prefix: 2,
                    selector: 2,
                },
            ]),
            Err(NameTableError::CyclicReference { entry_index: 1 })
        );
    }

    #[test]
    fn accepts_an_acyclic_forward_name_reference() {
        let names = NameTable::from_entries(vec![
            RawName::Qualified {
                prefix: 2,
                selector: 2,
            },
            RawName::Utf8("later".to_owned()),
        ])
        .unwrap();

        assert_eq!(names.len(), 2);
    }

    #[test]
    fn validates_a_long_acyclic_name_chain_without_using_recursion() {
        const CHAIN_LENGTH: usize = 4096;
        let mut entries = Vec::with_capacity(CHAIN_LENGTH);

        for index in 0..CHAIN_LENGTH - 1 {
            let next = u32::try_from(index + 2).unwrap();
            entries.push(RawName::Qualified {
                prefix: next,
                selector: next,
            });
        }
        entries.push(RawName::Utf8("end".to_owned()));

        let names = NameTable::from_entries(entries).unwrap();

        assert_eq!(names.len(), CHAIN_LENGTH);
        assert_eq!(names.get_utf8(CHAIN_LENGTH as u32), Some("end"));
    }

    #[test]
    fn rejects_an_unknown_name_entry_that_uses_the_utf8_tag() {
        assert_eq!(
            NameTable::from_entries(vec![RawName::Unknown {
                tag: 1,
                payload: vec![0x80],
            }]),
            Err(NameTableError::InvalidTag {
                tag: 1,
                entry_index: 0,
            })
        );
    }

    #[test]
    fn rejects_an_unknown_name_entry_that_uses_a_composite_tag() {
        assert_eq!(
            NameTable::from_entries(vec![RawName::Unknown {
                tag: 63,
                payload: vec![],
            }]),
            Err(NameTableError::InvalidTag {
                tag: 63,
                entry_index: 0,
            })
        );
    }

    #[test]
    fn rejects_a_zero_parameter_signature_when_decoding() {
        let bytes = [0x85, 63, 0x83, 0x81, 0x81, 0x80];
        let mut reader = Reader::new(&bytes);

        assert_eq!(
            NameTable::decode(&mut reader),
            Err(NameTableError::InvalidParamSig {
                value: 0,
                entry_index: 0,
            })
        );
    }

    #[test]
    fn rejects_i32_min_parameter_signature_when_constructing_name_entries() {
        assert_eq!(
            NameTable::from_entries(vec![RawName::Signed {
                original: 1,
                result_signature: 1,
                parameter_signatures: vec![i32::MIN],
            }]),
            Err(NameTableError::InvalidParamSig {
                value: i32::MIN,
                entry_index: 0,
            })
        );
    }

    #[test]
    fn builder_interns_duplicates_and_preserves_first_seen_order() {
        let mut builder = NameTable::builder();
        assert!(builder.is_empty());
        assert_eq!(builder.intern(RawName::Utf8("owner".to_owned())), Ok(1));
        assert_eq!(builder.intern(RawName::Utf8("member".to_owned())), Ok(2));
        assert_eq!(builder.intern(RawName::Utf8("owner".to_owned())), Ok(1));
        assert_eq!(builder.len(), 2);
        assert!(!builder.is_empty());

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

    fn assert_round_trips_name_entry(entry: RawName) {
        let names = NameTable::from_entries(vec![
            RawName::Utf8("owner".to_owned()),
            RawName::Utf8("member".to_owned()),
            entry,
        ])
        .unwrap();
        let mut writer = crate::Writer::new();
        names.encode(&mut writer).unwrap();

        let mut reader = Reader::new(writer.as_slice());
        assert_eq!(NameTable::decode(&mut reader).unwrap(), names);
        assert!(reader.is_at_end());
    }

    fn assert_rejects_truncated_entry(entry: RawName) {
        let names = NameTable::from_entries(vec![
            RawName::Utf8("owner".to_owned()),
            RawName::Utf8("separator".to_owned()),
            entry,
        ])
        .unwrap();
        let mut writer = crate::Writer::new();
        names.encode(&mut writer).unwrap();
        let mut bytes = writer.into_inner();
        bytes.pop();

        assert_unexpected_eof(&bytes);
    }

    fn assert_unexpected_eof(bytes: &[u8]) {
        let mut reader = Reader::new(bytes);
        assert!(matches!(
            NameTable::decode(&mut reader),
            Err(NameTableError::Read(ReadError::UnexpectedEof { .. }))
        ));
    }

    #[test]
    fn round_trips_a_utf8_name_entry() {
        assert_round_trips_name_entry(RawName::Utf8("nested".to_owned()));
    }

    #[test]
    fn round_trips_a_qualified_name_entry() {
        assert_round_trips_name_entry(RawName::Qualified {
            prefix: 1,
            selector: 2,
        });
    }

    #[test]
    fn round_trips_an_expanded_name_entry() {
        assert_round_trips_name_entry(RawName::Expanded {
            prefix: 1,
            selector: 2,
        });
    }

    #[test]
    fn round_trips_an_expand_prefix_name_entry() {
        assert_round_trips_name_entry(RawName::ExpandPrefix {
            prefix: 2,
            selector: 1,
        });
    }

    #[test]
    fn round_trips_a_unique_name_entry() {
        assert_round_trips_name_entry(RawName::Unique {
            separator: 1,
            uniqid: 7,
            underlying: Some(2),
        });
    }

    #[test]
    fn round_trips_a_unique_name_entry_without_an_underlying_name() {
        assert_round_trips_name_entry(RawName::Unique {
            separator: 1,
            uniqid: 7,
            underlying: None,
        });
    }

    #[test]
    fn round_trips_a_default_getter_name_entry() {
        assert_round_trips_name_entry(RawName::DefaultGetter {
            underlying: 1,
            index: 2,
        });
    }

    #[test]
    fn round_trips_a_super_accessor_name_entry() {
        assert_round_trips_name_entry(RawName::SuperAccessor { underlying: 1 });
    }

    #[test]
    fn round_trips_an_inline_accessor_name_entry() {
        assert_round_trips_name_entry(RawName::InlineAccessor { underlying: 1 });
    }

    #[test]
    fn round_trips_an_object_class_name_entry() {
        assert_round_trips_name_entry(RawName::ObjectClass { underlying: 1 });
    }

    #[test]
    fn round_trips_a_body_retainer_name_entry() {
        assert_round_trips_name_entry(RawName::BodyRetainer { underlying: 1 });
    }

    #[test]
    fn round_trips_a_signed_name_entry() {
        assert_round_trips_name_entry(RawName::Signed {
            original: 1,
            result_signature: 2,
            parameter_signatures: vec![1, -1],
        });
    }

    #[test]
    fn round_trips_a_target_signed_name_entry() {
        assert_round_trips_name_entry(RawName::TargetSigned {
            original: 1,
            target: 2,
            result_signature: 1,
            parameter_signatures: vec![2, -2],
        });
    }

    #[test]
    fn round_trips_an_unknown_name_entry() {
        assert_round_trips_name_entry(RawName::Unknown {
            tag: 99,
            payload: vec![1, 2, 3],
        });
    }
}
