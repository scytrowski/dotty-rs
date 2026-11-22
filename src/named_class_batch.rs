//! Ordered access to the named class sources in a caller-selected batch.
use std::{collections::HashMap, fmt};

use crate::NamedClassSource;
use crate::core::SourceId;

/// Failure to assemble a batch from named files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedClassBatchError {
    /// Two inputs have the same source ID, including files without declarations.
    DuplicateSource(SourceId),
}

impl fmt::Display for NamedClassBatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateSource(source) => write!(f, "duplicate named source {source:?}"),
        }
    }
}

impl std::error::Error for NamedClassBatchError {}

/// Borrows ordinary-class projections from a single naming session.
/// Files retain their input order, independent of the numeric source IDs.
#[derive(Debug)]
pub struct NamedClassBatch<'a> {
    files: Vec<&'a NamedClassSource>,
    by_source: HashMap<SourceId, &'a NamedClassSource>,
}

impl<'a> NamedClassBatch<'a> {
    /// Collects the supplied files in order. A repeated source ID rejects the
    /// whole batch. Construction does not prepare call navigation.
    pub fn new(
        files: impl IntoIterator<Item = &'a NamedClassSource>,
    ) -> Result<Self, NamedClassBatchError> {
        let mut by_source = HashMap::new();
        let mut ordered = Vec::new();
        for file in files {
            if by_source.insert(file.source(), file).is_some() {
                return Err(NamedClassBatchError::DuplicateSource(file.source()));
            }
            ordered.push(file);
        }
        Ok(Self {
            files: ordered,
            by_source,
        })
    }

    /// Iterates over the borrowed files in the order supplied at construction.
    pub fn files(&self) -> impl DoubleEndedIterator<Item = &'a NamedClassSource> + '_ {
        self.files.iter().copied()
    }

    /// Finds the original named file by its session source ID, including empty files.
    pub fn file(&self, source: SourceId) -> Option<&'a NamedClassSource> {
        self.by_source.get(&source).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Packages, SemanticStore};

    fn named(
        id: u32,
        text: &str,
        store: &mut SemanticStore,
        packages: &mut Packages,
    ) -> NamedClassSource {
        crate::parse_source(SourceId::from_index(id), text, Default::default())
            .unwrap()
            .bind(&mut store.names)
            .name(&format!("File{id}.scala"), store, packages)
            .unwrap()
            .into_classes(store)
            .unwrap()
    }

    #[test]
    fn sparse_sources_preserve_input_order_and_reject_repeated_empty_files() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();
        let a = named(42, "class A", &mut store, &mut packages);
        let empty = named(7, "", &mut store, &mut packages);
        for files in [[&a, &empty], [&empty, &a]] {
            let batch = NamedClassBatch::new(files).unwrap();
            assert_eq!(
                batch
                    .files()
                    .map(NamedClassSource::source)
                    .collect::<Vec<_>>(),
                files.map(NamedClassSource::source)
            );
            assert!(std::ptr::eq(batch.file(a.source()).unwrap(), &a));
            assert!(std::ptr::eq(batch.file(empty.source()).unwrap(), &empty));
            assert!(batch.file(SourceId::from_index(0)).is_none());
        }
        assert_eq!(
            NamedClassBatch::new([&empty, &a, &empty]).unwrap_err(),
            NamedClassBatchError::DuplicateSource(empty.source())
        );
    }
}
