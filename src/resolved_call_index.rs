//! Retained observations of direct source calls in one revision.
use std::collections::HashMap;

use crate::core::{AstArena, SemanticStore, SourceId, SourceSpan, SymbolId, Typed};
use crate::source_declarations::DeclarationDiagnostic;
use crate::{CheckedClassBodies, NamedClassSources, ResolvedSourceCall};

/// Index of direct source calls with committed body bindings in one session.
/// The index owns its locations and borrows no arenas. Symbols and byte ranges
/// are meaningful only for the original session and document versions.
#[derive(Debug, Default)]
pub struct ResolvedCallIndex {
    calls: HashMap<SourceId, Vec<ResolvedSourceCall>>,
    references: HashMap<SymbolId, Vec<SourceSpan>>,
    diagnostics: Vec<DeclarationDiagnostic>,
}

impl ResolvedCallIndex {
    /// Reads the selected files in input order using their original bodies,
    /// store and typed arena. Skips calls without a committed source target.
    /// An invalid navigation index yields one diagnostic for its file; calls
    /// between other valid files remain available. Performs no further typing.
    pub fn build(
        sources: &NamedClassSources<'_>,
        bodies: &CheckedClassBodies,
        typed_ast: &AstArena<Typed>,
        store: &SemanticStore,
    ) -> Self {
        let mut index = Self::default();
        for file in sources.files() {
            let navigation = match file.navigation() {
                Ok(navigation) => navigation,
                Err(error) => {
                    index.diagnostics.push(error.diagnostic(file.ast()));
                    continue;
                }
            };
            let mut calls = Vec::new();
            // A target navigation error is reported once when visiting its file.
            for call in sources
                .calls_in_file(file, navigation, bodies, typed_ast, store)
                .flatten()
            {
                index
                    .references
                    .entry(call.method)
                    .or_default()
                    .push(call.origin);
                calls.push(call);
            }
            index.calls.insert(file.source(), calls);
        }
        index
    }

    /// Invalid file navigation indices, once per file in input order.
    pub fn diagnostics(&self) -> &[DeclarationDiagnostic] {
        &self.diagnostics
    }

    /// Looks up a written callee token by half-open UTF-8 byte offset.
    /// Missing files, unchecked calls and offsets outside a name yield `None`.
    pub fn call_at(&self, source: SourceId, offset: u32) -> Option<&ResolvedSourceCall> {
        let calls = self.calls.get(&source)?;
        let i = calls.partition_point(|c| c.origin.span().range().end() <= offset);
        calls
            .get(i)
            .filter(|c| c.origin.span().range().contains(offset))
    }

    /// Written direct call sites of this exact method, in input file order then
    /// token order. Omits the definition. Unknown and unused symbols yield an
    /// empty slice. Supply a symbol from the original session.
    pub fn references_to(&self, method: SymbolId) -> &[SourceSpan] {
        self.references.get(&method).map_or(&[], Vec::as_slice)
    }
}
