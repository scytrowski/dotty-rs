//! The semantic unpickler driver.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use dotty_core::ids::{AnnotationId, ScopeId, SymbolId, TypeId};
use dotty_core::resolution::{NoResolver, SymbolResolver};
use dotty_core::store::SemanticStore;
use dotty_core::store::StoreCheckpoint;
use dotty_core::symbols::{SymbolInfo, SymbolKind, SymbolOrigin};
use dotty_core::{Definitions, Packages};
use dotty_tasty::tasty::{PACKAGE_TAG, TastyFile};

use crate::ast_view::AstView;
use crate::binders::PendingBinder;
use crate::error::UnpickleError;
use crate::index::TastySemanticIndex;
use crate::packages::ScopeJournal;
use crate::session::TastySession;

/// The marks of one public call, to roll it back.
pub(crate) struct Transaction {
    checkpoint: StoreCheckpoint,
    types: usize,
    type_trees: usize,
    term_trees: usize,
    rec_this: usize,
    infos: usize,
    annotations: usize,
    pending_opaque_aliases: usize,
}

/// Interprets one TASTy file into a `SemanticStore`.
///
/// The unpickler is multi-pass. [`enter_symbols`](Self::enter_symbols) is the
/// first pass: it allocates a symbol for every definition, with its owner,
/// kind, flags and visibility, but no type (`SymbolInfo::Missing`). Types and
/// signatures come from later passes, which resolve references through the
/// [`TastySemanticIndex`] this pass builds: `unpickle_type` decodes type nodes,
/// `unpickle_type_tree_type` projects type trees (their `tpe`, not a typed
/// AST), and `complete_symbol` gives the simple definitions their
/// `SymbolInfo::Complete`, one atomic transaction per symbol.
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
    /// Declaration scopes of class-like owners in the shared TASTy session.
    pub(crate) shared_scopes: HashMap<SymbolId, ScopeId>,
    /// Owners whose scopes were added to `shared_scopes`, for enter rollback.
    pub(crate) shared_scope_order: Vec<SymbolId>,
    /// Opaque aliases whose owners do not have a completed ClassInfo yet.
    pub(crate) pending_opaque_aliases: Vec<(SymbolId, dotty_core::names::Name, TypeId)>,
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
    /// Symbols whose `Symbol.annotations` completed (Milestone 5e1), with any
    /// annotations, including zero. `Vec::is_empty()` on
    /// `Symbol.annotations` cannot tell "not completed" from "completed with
    /// none", so this adapter-local set is the state
    /// [`complete_symbol_annotations`](Self::complete_symbol_annotations)
    /// checks, never a new field on `dotty-core::Symbol`.
    pub(crate) annotations_completed: HashSet<SymbolId>,
    /// The `(old Symbol.annotations, was it completed before)` each symbol's
    /// annotation state held before a call changed it, oldest first — mirrors
    /// `info_journal`: arena truncation frees a newly allocated `Annotation`
    /// but cannot restore a mutated field, or this set's own membership, for
    /// a symbol that already existed.
    pub(crate) annotations_journal: Vec<(SymbolId, Vec<AnnotationId>, bool)>,
    /// Whether `scala.&` / `scala.|` were declared in the package registry.
    special_aliases_declared: bool,
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
            shared_scopes: HashMap::new(),
            shared_scope_order: Vec::new(),
            pending_opaque_aliases: Vec::new(),
            resolver: Box::new(NoResolver),
            scope_journal: Vec::new(),
            pending_binders: Vec::new(),
            rec_this: HashMap::new(),
            rec_this_journal: Vec::new(),
            info_journal: Vec::new(),
            annotations_completed: HashSet::new(),
            annotations_journal: Vec::new(),
            special_aliases_declared: false,
            ast: None,
        }
    }

    /// Uses `resolver` for the name-based references the entered state cannot
    /// resolve. Without one, [`NoResolver`] leaves them unresolved.
    pub fn with_resolver(mut self, resolver: Box<dyn SymbolResolver>) -> Self {
        self.resolver = resolver;
        self
    }

    /// Prepares to enter a file in a session that shares package and class
    /// scopes with previously entered TASTy units.
    pub fn with_session(
        file: &'file TastyFile<'bytes>,
        store: &'store mut SemanticStore,
        definitions: Definitions,
        session: TastySession,
    ) -> Self {
        let mut unpickler = Self::with_packages(file, store, definitions, session.packages);
        unpickler.shared_scopes = session.owner_scopes;
        unpickler.shared_scope_order = session.scope_order;
        unpickler.pending_opaque_aliases = session.pending_opaque_aliases;
        unpickler
    }

    /// Resumes completion for a unit whose symbols were already entered with
    /// [`with_session`](Self::with_session), preserving that unit's address
    /// index while carrying the session updated by other entered units.
    ///
    /// This supports a whole-classpath entry pass followed by a completion
    /// pass: every unit's definitions are visible before any signature is
    /// projected, without re-entering definitions or losing their addresses.
    pub fn with_session_and_index(
        file: &'file TastyFile<'bytes>,
        store: &'store mut SemanticStore,
        definitions: Definitions,
        session: TastySession,
        index: TastySemanticIndex,
    ) -> Self {
        let mut unpickler = Self::with_session(file, store, definitions, session);
        unpickler.index = index;
        unpickler
    }

    /// The origin stamped on every symbol this unpickler enters.
    pub fn origin(&self) -> SymbolOrigin {
        self.origin
    }

    /// The addresses entered so far.
    pub fn index(&self) -> &TastySemanticIndex {
        &self.index
    }

    /// The kind and current info of the symbol entered for the definition at
    /// `address`, for measuring completion without reaching into the store.
    pub fn symbol_state_at(&self, address: u32) -> Option<(SymbolKind, SymbolInfo)> {
        let symbol = self.index.symbol_at(address)?;
        let entered = self.store.symbols.get(symbol);
        Some((entered.kind, entered.info))
    }

    /// Consumes the unpickler, keeping the index for later passes.
    pub fn into_index(self) -> TastySemanticIndex {
        self.index
    }

    /// Consumes the unpickler, keeping the index for later passes and the
    /// package registry. Use [`into_session_parts`](Self::into_session_parts)
    /// when the next unit must also reuse entered class scopes.
    pub fn into_parts(self) -> (TastySemanticIndex, Packages) {
        (self.index, self.packages)
    }

    /// Consumes the unpickler, keeping the index and the complete session for
    /// the next unit.
    pub fn into_session_parts(self) -> (TastySemanticIndex, TastySession) {
        (
            self.index,
            TastySession {
                packages: self.packages,
                owner_scopes: self.shared_scopes,
                scope_order: self.shared_scope_order,
                pending_opaque_aliases: self.pending_opaque_aliases,
            },
        )
    }

    /// Pass 1: enters a symbol for every definition in the file.
    ///
    /// Malformed or unsupported input is a typed error, and the call is
    /// atomic: on failure the store holds exactly the symbols, scopes, types
    /// and annotations it held before the call, every scope holds the
    /// declarations it held before, and the index, package registry and
    /// shared owner-scope registry are as they were, so nothing half-entered
    /// can be found later. Interned names
    /// and the origin registered by [`new`](Self::new) stay, which is
    /// harmless. The unpickler is usable afterwards, for example to retry.
    pub fn enter_symbols(&mut self) -> Result<&TastySemanticIndex, UnpickleError> {
        let checkpoint = self.store.checkpoint();
        let index = self.index.clone();
        let packages = self.packages.mark();
        let shared_scopes = self.shared_scope_order.len();
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
            while self.shared_scope_order.len() > shared_scopes {
                if let Some(owner) = self.shared_scope_order.pop() {
                    self.shared_scopes.remove(&owner);
                }
            }
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
        self.declare_special_aliases();
        let transaction = self.begin_transaction();
        let result = self.type_of_tpt(&ast, address, address, 0);
        self.finish_transaction(transaction, result)
    }

    /// Pass 5b: the semantic type of the *term* tree at `address`, for the
    /// paths a `SELECTtpt` qualifier or a `SINGLETONtpt` reference is made of.
    /// See the `term_type` module. Cached by term address, apart from both
    /// other caches. Atomic like the others.
    pub fn unpickle_term_type(&mut self, address: u32) -> Result<TypeId, UnpickleError> {
        let ast = self.ast_view()?;
        self.declare_special_aliases();
        let transaction = self.begin_transaction();
        let result = self.type_of_term(&ast, address, address, 0);
        self.finish_transaction(transaction, result)
    }

    /// Declares the compiler-defined `scala.&` / `scala.|` in the package
    /// registry, once, before a decoding call starts (so a failed call never
    /// takes them back). No file declares them, and an applied `&` in a type
    /// tree resolves to them by name and is recognized by identity. Deferred
    /// to the first decode so that merely entering a unit adds no package.
    pub(crate) fn declare_special_aliases(&mut self) {
        if !self.special_aliases_declared {
            self.special_aliases_declared = true;
            self.definitions
                .declare_special_aliases(self.store, &mut self.packages);
        }
    }

    /// Everything a public call may change that is not truncated with the
    /// arenas, taken at the start of the call.
    pub(crate) fn begin_transaction(&self) -> Transaction {
        Transaction {
            checkpoint: self.store.checkpoint(),
            types: self.index.mark_types(),
            type_trees: self.index.mark_type_trees(),
            term_trees: self.index.mark_term_trees(),
            rec_this: self.rec_this_journal.len(),
            infos: self.info_journal.len(),
            annotations: self.annotations_journal.len(),
            pending_opaque_aliases: self.pending_opaque_aliases.len(),
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
            self.pending_opaque_aliases
                .truncate(transaction.pending_opaque_aliases);
            // Infos and annotations first: they name symbols that exist
            // before the call, and truncating the arenas does not touch them.
            while self.info_journal.len() > transaction.infos {
                if let Some((symbol, info)) = self.info_journal.pop() {
                    self.store.symbols.set_info(symbol, info);
                }
            }
            while self.annotations_journal.len() > transaction.annotations {
                if let Some((symbol, annotations, was_completed)) = self.annotations_journal.pop() {
                    self.store.symbols.get_mut(symbol).annotations = annotations;
                    if was_completed {
                        self.annotations_completed.insert(symbol);
                    } else {
                        self.annotations_completed.remove(&symbol);
                    }
                }
            }
            self.store.rollback_to(transaction.checkpoint);
            self.index.roll_back_types(transaction.types);
            self.index.roll_back_type_trees(transaction.type_trees);
            self.index.roll_back_term_trees(transaction.term_trees);
            self.roll_back_rec_this(transaction.rec_this);
        } else {
            // Committed and outermost: nothing left to undo.
            if transaction.infos == 0 {
                self.info_journal.clear();
            }
            if transaction.annotations == 0 {
                self.annotations_journal.clear();
            }
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

    /// Keeps a declaration scope available to later TASTy units in this
    /// session, without completing its owner.
    pub(crate) fn share_owner_scope(
        &mut self,
        owner: SymbolId,
        scope: ScopeId,
    ) -> Result<(), UnpickleError> {
        match self.shared_scopes.get(&owner) {
            Some(existing) if *existing != scope => {
                Err(UnpickleError::DuplicateScope { symbol: owner })
            }
            Some(_) => Ok(()),
            None => {
                self.shared_scopes.insert(owner, scope);
                self.shared_scope_order.push(owner);
                Ok(())
            }
        }
    }

    /// Sets a symbol's annotations and marks its annotation completion
    /// state complete (Milestone 5e1), remembering the old annotations and
    /// completion state for rollback.
    pub(crate) fn set_symbol_annotations(
        &mut self,
        symbol: SymbolId,
        annotations: Vec<AnnotationId>,
    ) {
        let old = self.store.symbols.get(symbol).annotations.clone();
        let was_completed = self.annotations_completed.contains(&symbol);
        self.annotations_journal.push((symbol, old, was_completed));
        self.store.symbols.get_mut(symbol).annotations = annotations;
        self.annotations_completed.insert(symbol);
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
        self.link_companions()?;
        Ok(())
    }
}
