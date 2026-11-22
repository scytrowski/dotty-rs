//! Ordered access to the named class sources in a caller-selected batch.
use std::{collections::HashMap, fmt};

use crate::core::{SemanticStore, SourceId, SymbolId};
use crate::source_declarations::SourceClass;
use crate::{
    NamedClassSource, SourceDeclaration, SourceDeclarationIndex, SourceDeclarationIndexError,
};

/// Failure to assemble a batch from named files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedClassBatchError {
    /// Two inputs have the same source ID, including files without declarations.
    DuplicateSource(SourceId),
    /// The session store and the supplied declarations have inconsistent bindings.
    Declaration(SourceDeclarationIndexError),
}

impl fmt::Display for NamedClassBatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateSource(source) => write!(f, "duplicate named source {source:?}"),
            Self::Declaration(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for NamedClassBatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Declaration(error) => Some(error),
            Self::DuplicateSource(_) => None,
        }
    }
}

/// Borrows ordinary-class projections from a single naming session.
/// Files retain their input order, independent of the numeric source IDs.
#[derive(Debug)]
pub struct NamedClassBatch<'a> {
    declarations: SourceDeclarationIndex,
    files: Vec<&'a NamedClassSource>,
    by_source: HashMap<SourceId, &'a NamedClassSource>,
}

impl<'a> NamedClassBatch<'a> {
    /// Collects the supplied files in order. A repeated source ID rejects the
    /// whole batch. Validates declarations against their naming store and owns
    /// the resulting index, so the store remains available for mutable typing.
    /// The supplied files and store must belong to the same session: numeric
    /// symbol IDs alone cannot distinguish arenas. Navigation is left lazy.
    pub fn new(
        files: impl IntoIterator<Item = &'a NamedClassSource>,
        store: &SemanticStore,
    ) -> Result<Self, NamedClassBatchError> {
        let mut by_source = HashMap::new();
        let mut ordered = Vec::new();
        for file in files {
            if by_source.insert(file.source(), file).is_some() {
                return Err(NamedClassBatchError::DuplicateSource(file.source()));
            }
            ordered.push(file);
        }
        let declarations =
            SourceDeclarationIndex::from_classes(store, ordered.iter().flat_map(|f| f.classes()))
                .map_err(NamedClassBatchError::Declaration)?;
        Ok(Self {
            declarations,
            files: ordered,
            by_source,
        })
    }

    /// Iterates over the borrowed files in the order supplied at construction.
    pub fn files(&self) -> impl DoubleEndedIterator<Item = &'a NamedClassSource> + '_ {
        self.files.iter().copied()
    }

    /// Iterates by input file and then by class declaration order within each file.
    pub fn classes(
        &self,
    ) -> impl DoubleEndedIterator<Item = (&'a NamedClassSource, &'a SourceClass)> + '_ {
        self.files()
            .flat_map(|file| file.classes().iter().map(move |class| (file, class)))
    }

    /// Returns the batch's validated index, including constructors and their fields.
    pub fn declarations(&self) -> &SourceDeclarationIndex {
        &self.declarations
    }

    /// Resolves a same-session symbol to its declaring file and canonical owner.
    /// A symbol outside this batch has no entry.
    pub fn declaration(
        &self,
        symbol: SymbolId,
    ) -> Option<(&'a NamedClassSource, SourceDeclaration)> {
        let declaration = self.declarations.get(symbol)?;
        Some((self.file(declaration.source)?, declaration))
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
            let batch = NamedClassBatch::new(files, &store).unwrap();
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
            NamedClassBatch::new([&empty, &a, &empty], &store).unwrap_err(),
            NamedClassBatchError::DuplicateSource(empty.source())
        );
    }
    #[test]
    fn same_named_fields_resolve_to_their_exact_classes_and_files() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();
        let ab = named(
            42,
            "class A { val seed = 1 }; class B",
            &mut store,
            &mut packages,
        );
        let c = named(7, "class C { val seed = 7 }", &mut store, &mut packages);
        let omitted = named(3, "class D", &mut store, &mut packages);
        let batch = NamedClassBatch::new([&c, &ab], &store).unwrap();
        assert_eq!(
            batch
                .classes()
                .map(|(f, c)| (f.source(), c.symbol))
                .collect::<Vec<_>>(),
            [
                (c.source(), c.classes()[0].symbol),
                (ab.source(), ab.classes()[0].symbol),
                (ab.source(), ab.classes()[1].symbol)
            ]
        );
        for file in [&ab, &c] {
            let class = &file.classes()[0];
            let seed = class.fields().next().unwrap().symbol;
            let (owner_file, d) = batch.declaration(seed).unwrap();
            assert!(std::ptr::eq(owner_file, file));
            assert_eq!(
                (d.class, d.source, d.kind),
                (class.symbol, file.source(), crate::core::SymbolKind::Field)
            );
        }
        assert!(batch.declaration(omitted.classes()[0].symbol).is_none());
    }
}
