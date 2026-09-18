//! The semantic unpickler driver.

use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolOrigin;
use dotty_tasty::tasty::{PACKAGE_TAG, RawNode, TastyFile};

use crate::error::UnpickleError;
use crate::index::TastySemanticIndex;
use crate::names::qualified_segments;
use crate::packages::PackageRegistry;

/// Interprets one TASTy file into a `SemanticStore`.
///
/// The unpickler is multi-pass. [`enter_symbols`](Self::enter_symbols) is the
/// first pass: it allocates a symbol for every definition, with its owner,
/// kind, flags and visibility, but no type (`SymbolInfo::Missing`). Types and
/// signatures come from later passes, which resolve references through the
/// [`TastySemanticIndex`] this pass builds.
///
/// The store is borrowed mutably for the unpickler's lifetime, and one
/// unpickler enters one file: two files entered into one store each get their
/// own symbols for a package they share.
pub struct TastyUnpickler<'file, 'bytes, 'store> {
    file: &'file TastyFile<'bytes>,
    store: &'store mut SemanticStore,
    origin: SymbolOrigin,
    index: TastySemanticIndex,
    packages: PackageRegistry,
}

impl<'file, 'bytes, 'store> TastyUnpickler<'file, 'bytes, 'store> {
    /// Prepares to unpickle `file` into `store`, registering a fresh TASTy
    /// origin that every entered symbol carries.
    pub fn new(file: &'file TastyFile<'bytes>, store: &'store mut SemanticStore) -> Self {
        let origin = SymbolOrigin::Tasty(store.origins.register_tasty());
        Self {
            file,
            store,
            origin,
            index: TastySemanticIndex::new(),
            packages: PackageRegistry::default(),
        }
    }

    /// The origin stamped on every symbol this unpickler enters.
    pub fn origin(&self) -> SymbolOrigin {
        self.origin
    }

    /// The addresses entered so far.
    pub fn index(&self) -> &TastySemanticIndex {
        &self.index
    }

    /// Consumes the unpickler, keeping the index for later passes.
    pub fn into_index(self) -> TastySemanticIndex {
        self.index
    }

    /// Pass 1: enters a symbol for every definition in the file.
    ///
    /// Malformed or unsupported input is a typed error; symbols entered
    /// before the failure stay in the store.
    pub fn enter_symbols(&mut self) -> Result<&TastySemanticIndex, UnpickleError> {
        let roots = self.file.asts()?;
        for node in roots.iter() {
            if node.tag == PACKAGE_TAG {
                self.enter_package(node)?;
            }
        }
        Ok(&self.index)
    }

    fn enter_package(&mut self, node: &RawNode<'_>) -> Result<(), UnpickleError> {
        let address = address_of(node);
        let package = node.decode_package()?;
        let path_name = package
            .path_name()
            .ok_or(UnpickleError::UnsupportedPackagePath { address })?;
        let path = qualified_segments(self.file.names(), path_name)?;

        let symbol = self
            .packages
            .enter(self.store, &mut self.index, self.origin, &path)?;
        self.index.insert_symbol(address, symbol)
    }
}

/// The AST address of a node. Addresses index a section that TASTy bounds to
/// `u32`, so a larger offset can only mean a corrupt node.
fn address_of(node: &RawNode<'_>) -> u32 {
    u32::try_from(node.offset).unwrap_or(u32::MAX)
}
