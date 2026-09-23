//! Serialized symbol annotations (Milestone 5e1): `Symbol.annotations`.
//!
//! A definition's or parameter's tail may carry `ANNOTATION` entries
//! ([`crate::mapping`] and [`crate::enter`] leave them unread; pass 1 only
//! indexes their addresses, in wire order, on
//! [`TastySemanticIndex::annotation_tail_at`](crate::index::TastySemanticIndex::annotation_tail_at)).
//! This is the completion that turns those indexed addresses into semantic
//! `AnnotationId`s and attaches them to `Symbol.annotations`, in the same
//! order.
//!
//! ## Independent of `SymbolInfo` completion
//!
//! [`complete_symbol_annotations`](TastyUnpickler::complete_symbol_annotations)
//! and [`complete_symbol`](crate::unpickler::TastyUnpickler::complete_symbol)
//! are separate semantic dimensions of one symbol: either may be called
//! first, or alone, or not at all. Annotation completion never inspects or
//! sets `SymbolInfo`, and `complete_symbol` never decodes an annotation tail;
//! a class whose parent is still external, or an opaque alias deferred by
//! `complete_symbol`, can still have its own annotations completed.
//!
//! ## The `ANNOTATION` wrapper, not `ANNOTATEDtype`
//!
//! ```text
//! ANNOTATION Length tycon_Type full_annotation_Type
//! ```
//!
//! This is not the `{underlying, annotation}` pair `ANNOTATEDtype`/
//! `ANNOTATEDtpt` read ([`crate::annotated`]): a tail entry's payload splits
//! out a `tycon` field alongside the same kind of annotation tree
//! (`full_annotation`). The `full_annotation` tree is decoded by the very
//! same lower-level decoder those two use,
//! [`decode_annotation_payload`](crate::unpickler::TastyUnpickler::decode_annotation_payload)
//! (compact or full, Milestone 5e1's own "commit 2" refactor) — the
//! `AnnotationId` this produces is exactly the same representation
//! `Type::Annotated` uses, from the one shared `AnnotationArena`.
//!
//! `tycon` is read here only far enough to validate the wrapper's shape
//! (`RawNode::decode_annotation`); it is not given independent semantic
//! weight. The full annotation tree's own root already carries the
//! constructor's type (a compact tag is a type outright; a full
//! constructor's `NEW`/`APPLY` spine resolves its own class), so nothing is
//! lost by not reading `tycon` a second time. If a real annotation is ever
//! found where the two disagree, that is future evidence to revisit this
//! choice, not something this milestone tries to detect.
//!
//! ## Identity, idempotence and atomicity
//!
//! Each `ANNOTATION` tail occurrence gets its own `AnnotationId`: two
//! occurrences that happen to decode to equal payloads are never merged, and
//! a repeated call for an already-completed symbol returns the same ids
//! again without reallocating (`annotations_completed`, an adapter-local set
//! — `Symbol.annotations` alone cannot tell "not completed" from "completed
//! with zero annotations"). A symbol's annotations complete atomically: if
//! any one tail entry fails to decode, none of that symbol's annotations for
//! this call are attached and its completion state stays incomplete, exactly
//! as if the call had not been made; a retry then behaves like the first
//! attempt. [`complete_symbols_annotations`](TastyUnpickler::complete_symbols_annotations)
//! extends this to a whole batch: if any address in it fails, none of them
//! stay completed, mirroring
//! [`complete_symbols`](crate::unpickler::TastyUnpickler::complete_symbols).

use dotty_core::ids::AnnotationId;

use crate::ast_view::{AstView, address};
use crate::error::UnpickleError;
use crate::unpickler::TastyUnpickler;

impl TastyUnpickler<'_, '_, '_> {
    /// Completes the symbol annotations of the definition/parameter entered
    /// at `address`: decodes every `ANNOTATION` tail entry pass 1 indexed for
    /// it, in wire order, into `Symbol.annotations`, and returns them.
    ///
    /// `address` must name a definition/parameter pass 1 entered a symbol
    /// for (`UnpickleError::MissingEnteredSymbol` otherwise, matching
    /// [`complete_symbol`](crate::unpickler::TastyUnpickler::complete_symbol)),
    /// and one that carries an indexed `ANNOTATION` tail: every `TYPEDEF`/
    /// `VALDEF`/`DEFDEF`/`TYPEPARAM`/`PARAM` does, even with zero entries, but
    /// a package symbol never does (`UnpickleError::UnsupportedAnnotationCompletion`).
    /// Already-completed annotations (including a symbol completed with
    /// none) are returned again with no further work. The call is atomic:
    /// see the module docs.
    pub fn complete_symbol_annotations(
        &mut self,
        address: u32,
    ) -> Result<Vec<AnnotationId>, UnpickleError> {
        let ast = self.ast_view()?;
        self.declare_special_aliases();
        let transaction = self.begin_transaction();
        let result = self.complete_annotations_in(&ast, address, 0);
        self.finish_transaction(transaction, result)
    }

    /// Completes the symbol annotations of each of `addresses`, in order, as
    /// one transaction: if any fails, none of them stay completed. Mirrors
    /// [`complete_symbols`](crate::unpickler::TastyUnpickler::complete_symbols).
    pub fn complete_symbols_annotations(
        &mut self,
        addresses: &[u32],
    ) -> Result<Vec<Vec<AnnotationId>>, UnpickleError> {
        let ast = self.ast_view()?;
        self.declare_special_aliases();
        let transaction = self.begin_transaction();
        let result = addresses
            .iter()
            .map(|address| self.complete_annotations_in(&ast, *address, 0))
            .collect();
        self.finish_transaction(transaction, result)
    }

    /// Completes the symbol annotations of the definition/parameter at `at`;
    /// `depth` bounds the annotation-tree walk the way every other
    /// completion's does.
    fn complete_annotations_in(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        depth: usize,
    ) -> Result<Vec<AnnotationId>, UnpickleError> {
        let symbol = self
            .index
            .symbol_at(at)
            .ok_or(UnpickleError::MissingEnteredSymbol { address: at })?;
        if self.annotations_completed.contains(&symbol) {
            return Ok(self.store.symbols.get(symbol).annotations.clone());
        }
        // Every `TYPEDEF`/`VALDEF`/`DEFDEF`/`TYPEPARAM`/`PARAM` symbol is
        // entered through `enter_symbol`, which always indexes an annotation
        // tail alongside it (even an empty one). A package symbol is the one
        // exception: `enter_package` inserts it directly and never indexes a
        // tail, because a `PACKAGE` node has no `ANNOTATION` tail in the wire
        // format at all. `annotation_tail_at` returning `None` here means
        // `at` names one of those, not a bug to panic on.
        let Some(tails) = self.index.annotation_tail_at(at) else {
            let kind = self.store.symbols.get(symbol).kind;
            return Err(UnpickleError::UnsupportedAnnotationCompletion { address: at, kind });
        };
        let tails = tails.to_vec();

        let mut annotations = Vec::with_capacity(tails.len());
        for annotation_node_at in tails {
            let full_annotation_at = self.full_annotation_address(ast, at, annotation_node_at)?;
            annotations.push(self.decode_annotation_payload(ast, at, full_annotation_at, depth)?);
        }
        self.set_symbol_annotations(symbol, annotations.clone());
        Ok(annotations)
    }

    /// The absolute address of the `full_annotation` tree of the `ANNOTATION`
    /// tail node at `annotation_node_at`, owned by the definition/parameter
    /// at `at`. Shape-validates the wrapper (`RawNode::decode_annotation`);
    /// `tycon`, the wrapper's first child, is not otherwise read here (see
    /// the module docs).
    fn full_annotation_address(
        &self,
        ast: &AstView<'_>,
        at: u32,
        annotation_node_at: u32,
    ) -> Result<u32, UnpickleError> {
        ast.node(annotation_node_at)?.decode_annotation()?;
        let children: Vec<u32> = ast
            .children(annotation_node_at)
            .iter()
            .map(|child| address(child.offset))
            .collect();
        let [_tycon_at, full_annotation_at] = children[..] else {
            return Err(UnpickleError::MalformedType {
                address: at,
                reason: "an ANNOTATION tail entry has a tycon and a full annotation",
            });
        };
        Ok(full_annotation_at)
    }
}
