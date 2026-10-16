//! Owned complete output for the minimal semantic slice.

use dotty_core::{SymbolId, TreeId, Typed};
use dotty_tasty::tasty::*;

use crate::{
    LimitKind, OmittedMethodBody, PickleError, ProjectionLimits, SkeletonDeclaration,
    SourceMetadata, ValidatedPicklePlan,
};

/// Whole-file budget in addition to projection and wire depth/size budgets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickleLimits {
    pub projection: ProjectionLimits,
    pub max_file_bytes: usize,
}

impl Default for PickleLimits {
    fn default() -> Self {
        Self {
            projection: ProjectionLimits::default(),
            max_file_bytes: 16 * 1024 * 1024,
        }
    }
}

/// A complete owned file for the supported slice. Does not write to disk.
/// Rust structural validation alone does not establish Scala interoperability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickledTasty {
    bytes: Vec<u8>,
    source: SourceMetadata,
    intended_tasty_path: String,
    declarations: Vec<SkeletonDeclaration>,
    bodies: Vec<OmittedMethodBody>,
}

impl PickledTasty {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn source(&self) -> &SourceMetadata {
        &self.source
    }
    pub fn intended_tasty_path(&self) -> &str {
        &self.intended_tasty_path
    }
    /// Canonical identities and AST-relative declaration addresses.
    pub fn declarations(&self) -> &[SkeletonDeclaration] {
        &self.declarations
    }
    /// Exact existing typed root serialized for a canonical method identity.
    pub fn body(&self, method: SymbolId) -> Option<TreeId<Typed>> {
        self.bodies
            .iter()
            .find(|b| b.method == method)
            .map(|b| b.typed_root)
    }
}

impl ValidatedPicklePlan<'_, '_> {
    /// Serializes actual typed literal bodies and validates an owned 28.9.0 file.
    pub fn pickle(&self) -> Result<PickledTasty, PickleError> {
        self.pickle_with_limits(PickleLimits::default())
    }

    /// Limits apply to the entire file as well as names, types and wire layout.
    /// Unsupported nodes fail; traversal is never truncated to fit a budget.
    pub fn pickle_with_limits(&self, limits: PickleLimits) -> Result<PickledTasty, PickleError> {
        let output = self.assemble(limits.projection, Some(limits.max_file_bytes))?;
        Ok(PickledTasty {
            bytes: output.bytes,
            source: output.source,
            intended_tasty_path: output.intended_tasty_path,
            declarations: output.declarations,
            bodies: output.omitted_bodies,
        })
    }
}

// Pinned TastyHash.pjwHash64: wrapping unsigned byte arithmetic, not a
// cryptographic digest. Header identity is assembled as in TastyPickler.scala.
fn hash(bytes: &[u8]) -> u64 {
    let mut h = 0u64;
    for &byte in bytes {
        h = h.wrapping_shl(8).wrapping_add(u64::from(byte));
        let high = h & 0xff00_0000_0000_0000;
        h ^= high >> 48;
        h &= !high;
    }
    h
}

fn encoding(cause: impl Into<TastyFileError>) -> PickleError {
    PickleError::Encoding {
        cause: Box::new(cause.into()),
    }
}

pub(crate) fn prepare_header(
    header: &mut Header,
    names: &NameTable,
    asts: &EncodedSection,
    attributes: &EncodedSection,
    max_file_bytes: usize,
) -> Result<(), PickleError> {
    let mut writer = Writer::new();
    names.encode(&mut writer).map_err(encoding)?;
    let name_bytes = writer.as_slice();
    let mut reader = Reader::new(name_bytes);
    let size = reader
        .read_nat()
        .map_err(NameTableError::from)
        .map_err(encoding)? as usize;
    let payload = reader
        .read_bytes(size)
        .map_err(NameTableError::from)
        .map_err(encoding)?;
    header.uuid[..8].copy_from_slice(&(hash(payload) ^ hash(asts.payload())).to_be_bytes());
    header.uuid[8..].copy_from_slice(&hash(attributes.payload()).to_be_bytes());

    // Measure before the full-file encoder allocates its output. Section framing
    // is written by the existing Writer; only payload lengths are added here.
    header.encode(&mut writer).map_err(encoding)?;
    for section in [asts, attributes] {
        writer.write_nat(section.name());
        writer.write_nat(u32::try_from(section.payload().len()).map_err(|_| {
            PickleError::LimitExceeded {
                kind: LimitKind::FileBytes,
                limit: max_file_bytes,
            }
        })?);
    }
    let total = writer
        .as_slice()
        .len()
        .checked_add(asts.payload().len())
        .and_then(|n| n.checked_add(attributes.payload().len()));
    if total.is_none_or(|n| n > max_file_bytes) {
        return Err(PickleError::LimitExceeded {
            kind: LimitKind::FileBytes,
            limit: max_file_bytes,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_hash_matches_the_frozen_scala_producer() {
        let bytes =
            include_bytes!("../../../tests/fixtures/tasty_pickler/tasty/example/Answer.tasty");
        let file = TastyFile::parse_and_validate_scala_3_9(bytes).unwrap();
        let mut writer = Writer::new();
        file.names().encode(&mut writer).unwrap();
        let mut reader = Reader::new(writer.as_slice());
        let size = reader.read_nat().unwrap() as usize;
        let names_hash = hash(reader.read_bytes(size).unwrap());
        let asts_hash = hash(file.section(StandardSection::Asts).unwrap().payload);
        assert_eq!(
            &file.header().uuid[..8],
            &(names_hash ^ asts_hash).to_be_bytes()
        );
        let other_hash = file
            .sections()
            .iter()
            .filter(|s| s.name != file.names().find_utf8("ASTs").unwrap())
            .fold(0u64, |h, s| h ^ hash(s.payload));
        assert_eq!(&file.header().uuid[8..], &other_hash.to_be_bytes());
    }
}
