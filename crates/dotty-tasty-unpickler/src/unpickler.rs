//! The semantic unpickler driver.

use std::collections::HashMap;
use std::rc::Rc;

use dotty_core::ids::{SymbolId, TypeId};
use dotty_core::resolution::{NoResolver, SymbolResolver};
use dotty_core::store::SemanticStore;
use dotty_core::store::StoreCheckpoint;
use dotty_core::symbols::{SymbolInfo, SymbolOrigin};
use dotty_core::{Definitions, Packages};
use dotty_tasty::tasty::{PACKAGE_TAG, TastyFile};

use crate::ast_view::AstView;
use crate::binders::PendingBinder;
use crate::error::UnpickleError;
use crate::index::TastySemanticIndex;
use crate::packages::ScopeJournal;

/// The marks of one public call, to roll it back.
pub(crate) struct Transaction {
    checkpoint: StoreCheckpoint,
    types: usize,
    type_trees: usize,
    rec_this: usize,
    infos: usize,
}

/// Interprets one TASTy file into a `SemanticStore`.
///
/// The unpickler is multi-pass. [`enter_symbols`](Self::enter_symbols) is the
/// first pass: it allocates a symbol for every definition, with its owner,
/// kind, flags and visibility, but no type (`SymbolInfo::Missing`). Types and
/// signatures come from later passes, which resolve references through the
/// [`TastySemanticIndex`] this pass builds.
///
/// The store is borrowed mutably for the unpickler's lifetime, and one
/// unpickler enters one file. To enter several files into one store, carry the
/// [`Packages`] registry from one unpickler to the next
/// ([`with_packages`](Self::with_packages), [`into_parts`](Self::into_parts)):
/// they then share one symbol and one scope for each package they have in
/// common. Without it each file gets its own symbols for a shared package.
pub struct TastyUnpickler<'file, 'bytes, 'store> {
    pub(crate) file: &'file TastyFile<'bytes>,
    pub(crate) store: &'store mut SemanticStore,
    /// The session's canonical definitions, bootstrapped once by the caller.
    pub(crate) definitions: Definitions,
    pub(crate) origin: SymbolOrigin,
    pub(crate) index: TastySemanticIndex,
    pub(crate) packages: Packages,
    /// Asked for members the entered state does not hold. Owned, and given
    /// the store to read on each request; it never opens files here.
    pub(crate) resolver: Box<dyn SymbolResolver>,
    /// Declarations made into scopes during the current `enter_symbols`.
    pub(crate) scope_journal: ScopeJournal,
    /// Binders reserved and published but not yet filled, innermost last.
    /// Decoding state only: empty whenever a public call returns.
    pub(crate) pending_binders: Vec<PendingBinder>,
    /// Each `Recursive` binder's one canonical `RecThis`, as Dotty's `RecType`
    /// keeps one `recThis`. Survives across `unpickle_type` calls, so it is
    /// journaled: a failed call takes back the entries it added.
    pub(crate) rec_this: HashMap<TypeId, TypeId>,
    /// The binders `rec_this` gained entries for, in order, to roll back.
    pub(crate) rec_this_journal: Vec<TypeId>,
    /// The `SymbolInfo` each completed symbol held before the transaction that
    /// completed it, oldest first. Arena truncation cannot restore a field of
    /// a symbol that already existed, so a failed transaction puts these back.
    pub(crate) info_journal: Vec<(SymbolId, SymbolInfo)>,
    /// The file's AST view, built on first use and shared by both passes.
    ast: Option<Rc<AstView<'bytes>>>,
}

impl<'file, 'bytes, 'store> TastyUnpickler<'file, 'bytes, 'store> {
    /// Prepares to unpickle `file` into `store`, registering a fresh TASTy
    /// origin that every entered symbol carries.
    ///
    /// `definitions` must be the ones bootstrapped for `store`: the caller
    /// owns the session and bootstraps exactly once, so every adapter on the
    /// store shares one canonical `NoPrefix`. The unpickler never
    /// bootstraps.
    pub fn new(
        file: &'file TastyFile<'bytes>,
        store: &'store mut SemanticStore,
        definitions: Definitions,
    ) -> Self {
        Self::with_packages(file, store, definitions, Packages::new())
    }

    /// Like [`new`](Self::new), but reuses the package symbols and scopes
    /// already entered into `store` by earlier units. `packages` must come
    /// from unpicklers working on this same `store`.
    pub fn with_packages(
        file: &'file TastyFile<'bytes>,
        store: &'store mut SemanticStore,
        definitions: Definitions,
        packages: Packages,
    ) -> Self {
        let origin = SymbolOrigin::Tasty(store.origins.register_tasty());
        Self {
            file,
            store,
            definitions,
            origin,
            index: TastySemanticIndex::new(),
            packages,
            resolver: Box::new(NoResolver),
            scope_journal: Vec::new(),
            pending_binders: Vec::new(),
            rec_this: HashMap::new(),
            rec_this_journal: Vec::new(),
            info_journal: Vec::new(),
            ast: None,
        }
    }

    /// Uses `resolver` for the name-based references the entered state cannot
    /// resolve. Without one, [`NoResolver`] leaves them unresolved.
    pub fn with_resolver(mut self, resolver: Box<dyn SymbolResolver>) -> Self {
        self.resolver = resolver;
        self
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

    /// Consumes the unpickler, keeping the index for later passes and the
    /// package registry to hand to the unpickler of the next unit.
    pub fn into_parts(self) -> (TastySemanticIndex, Packages) {
        (self.index, self.packages)
    }

    /// Pass 1: enters a symbol for every definition in the file.
    ///
    /// Malformed or unsupported input is a typed error, and the call is
    /// atomic: on failure the store holds exactly the symbols, scopes, types
    /// and annotations it held before the call, every scope holds the
    /// declarations it held before, and the index and package registry are as
    /// they were, so nothing half-entered can be found later. Interned names
    /// and the origin registered by [`new`](Self::new) stay, which is
    /// harmless. The unpickler is usable afterwards, for example to retry.
    pub fn enter_symbols(&mut self) -> Result<&TastySemanticIndex, UnpickleError> {
        let checkpoint = self.store.checkpoint();
        let index = self.index.clone();
        let packages = self.packages.mark();
        self.scope_journal.clear();

        if let Err(error) = self.enter_all() {
            // Truncating the arenas frees what this call allocated. Packages
            // may be shared with earlier units, so their scopes, which
            // survive the truncation, also lose what this call declared in
            // them.
            for &(scope, symbol) in &self.scope_journal {
                self.store.scopes.get_mut(scope).remove(symbol);
            }
            self.packages.roll_back_to(self.store, packages);
            self.store.rollback_to(checkpoint);
            self.index = index;
            self.scope_journal.clear();
            return Err(error);
        }
        self.scope_journal.clear();
        Ok(&self.index)
    }

    /// The AST view of the file, built the first time it is needed.
    pub(crate) fn ast_view(&mut self) -> Result<Rc<AstView<'bytes>>, UnpickleError> {
        if let Some(ast) = &self.ast {
            return Ok(Rc::clone(ast));
        }
        let ast = Rc::new(AstView::new(self.file)?);
        self.ast = Some(Rc::clone(&ast));
        Ok(ast)
    }

    /// Pass 2a: the semantic type of the type node at `address`.
    ///
    /// `address` is the absolute AST address of a type node, such as one
    /// reported by `dotty-tasty`'s address index. The result is cached in the
    /// index by address, so decoding the same address again, or reaching it
    /// through a `SHAREDtype` link, returns the same `TypeId`. A
    /// `SHAREDtype` node is not a type of its own: decoding it returns the
    /// `TypeId` of the node it names.
    ///
    /// Decoding is lazy: only the addressed node and the prefixes it refers to
    /// are decoded, and only decoded nodes enter the index. The forms decoded
    /// so far are the reference forms; anything else is an
    /// `UnsupportedType` error. Symbols must have been entered first
    /// ([`enter_symbols`](Self::enter_symbols)), because references resolve
    /// through the index. See `docs/tasty-semantic-unpickler.md`.
    ///
    /// The call is atomic: on failure every type allocated and every address
    /// recorded by this call is taken back, so the store and the index are as
    /// they were.
    pub fn unpickle_type(&mut self, address: u32) -> Result<TypeId, UnpickleError> {
        let ast = self.ast_view()?;
        let transaction = self.begin_transaction();
        let result = self.type_at(&ast, address, address, 0);
        self.finish_transaction(transaction, result)
    }

    /// Pass 5a: the semantic type of the type *tree* at `address`.
    ///
    /// This is the tree's `tpe`, not a typed AST node: see the `type_tree`
    /// module. It is cached by tree address, apart from the type-node cache
    /// of [`unpickle_type`](Self::unpickle_type). Atomic like it.
    pub fn unpickle_type_tree_type(&mut self, address: u32) -> Result<TypeId, UnpickleError> {
        let ast = self.ast_view()?;
        let transaction = self.begin_transaction();
        let result = self.type_of_tpt(&ast, address, address);
        self.finish_transaction(transaction, result)
    }

    /// Everything a public call may change that is not truncated with the
    /// arenas, taken at the start of the call.
    pub(crate) fn begin_transaction(&self) -> Transaction {
        Transaction {
            checkpoint: self.store.checkpoint(),
            types: self.index.mark_types(),
            type_trees: self.index.mark_type_trees(),
            rec_this: self.rec_this_journal.len(),
            infos: self.info_journal.len(),
        }
    }

    /// Ends a public call: on failure puts everything back as
    /// [`begin_transaction`](Self::begin_transaction) found it.
    pub(crate) fn finish_transaction<T>(
        &mut self,
        transaction: Transaction,
        result: Result<T, UnpickleError>,
    ) -> Result<T, UnpickleError> {
        if result.is_err() {
            // Infos first: they name symbols that exist before the call, and
            // truncating the arenas does not touch them.
            while self.info_journal.len() > transaction.infos {
                if let Some((symbol, info)) = self.info_journal.pop() {
                    self.store.symbols.set_info(symbol, info);
                }
            }
            self.store.rollback_to(transaction.checkpoint);
            self.index.roll_back_types(transaction.types);
            self.index.roll_back_type_trees(transaction.type_trees);
            self.roll_back_rec_this(transaction.rec_this);
        } else if transaction.infos == 0 {
            // Committed and outermost: nothing left to undo.
            self.info_journal.clear();
        }
        // Nothing is pending outside a call, and every binder unregisters
        // itself; this only guarantees the invariant for the next call.
        debug_assert!(self.pending_binders.is_empty());
        self.pending_binders.clear();
        result
    }

    /// Sets a symbol's info, remembering the old one for rollback.
    pub(crate) fn set_symbol_info(&mut self, symbol: SymbolId, info: SymbolInfo) {
        let old = self.store.symbols.get(symbol).info;
        self.info_journal.push((symbol, old));
        self.store.symbols.set_info(symbol, info);
    }

    fn enter_all(&mut self) -> Result<(), UnpickleError> {
        let ast = self.ast_view()?;
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
