//! The semantic unpickler driver.

use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolOrigin;
use dotty_tasty::tasty::{PACKAGE_TAG, TastyFile};

use crate::enter::AstView;
use crate::error::UnpickleError;
use crate::index::TastySemanticIndex;
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
    pub(crate) file: &'file TastyFile<'bytes>,
    pub(crate) store: &'store mut SemanticStore,
    pub(crate) origin: SymbolOrigin,
    pub(crate) index: TastySemanticIndex,
    pub(crate) packages: PackageRegistry,
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
    /// Malformed or unsupported input is a typed error, and the call is
    /// atomic: on failure the store holds exactly the symbols, scopes, types
    /// and annotations it held before the call, and the index and package
    /// registry are as they were, so nothing half-entered can be found later.
    /// Interned names and the origin registered by [`new`](Self::new) stay,
    /// which is harmless. The unpickler is usable afterwards, for example to
    /// retry.
    pub fn enter_symbols(&mut self) -> Result<&TastySemanticIndex, UnpickleError> {
        let checkpoint = self.store.checkpoint();
        let index = self.index.clone();
        let packages = self.packages.clone();

        if let Err(error) = self.enter_all() {
            // One unpickler owns its package symbols and scopes (see
            // `PackageRegistry`), so everything this call changed was
            // allocated by it, and truncating the arenas undoes it.
            self.store.rollback_to(checkpoint);
            self.index = index;
            self.packages = packages;
            return Err(error);
        }
        Ok(&self.index)
    }

    fn enter_all(&mut self) -> Result<(), UnpickleError> {
        let ast = AstView::new(self.file)?;
        let roots = self.file.asts()?;
        for node in roots.iter() {
            if node.tag == PACKAGE_TAG {
                let at = u32::try_from(node.offset).unwrap_or(u32::MAX);
                self.enter_package(&ast, at)?;
            }
        }
        Ok(())
    }
}
