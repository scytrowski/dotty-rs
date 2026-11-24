//! Retained direct calls and field accesses in one revision.
use std::collections::HashMap;

use crate::core::{AstArena, SemanticStore, SourceId, SourceSpan, Span, SymbolId, Typed};
use crate::source_declarations::DeclarationDiagnostic;
use crate::{
    CheckedClassBodies, NamedClassSources, ResolvedFieldAccess, ResolvedSourceCall,
    SourceMemberTarget, SourceMethodTarget,
};

#[derive(Debug)]
struct IndexedFile {
    calls: Vec<ResolvedSourceCall>,
    fields: Vec<ResolvedFieldAccess>,
    field_declarations: Vec<(SymbolId, SourceMemberTarget)>,
    method_declarations: Vec<(SymbolId, SourceMethodTarget)>,
}

/// Index of direct source calls and source field accesses with committed body bindings in one session.
/// The index owns its locations and borrows no arenas. Symbols and byte ranges
/// are meaningful only for the original session and document versions.
#[derive(Debug, Default)]
pub struct ResolvedMemberIndex {
    files: HashMap<SourceId, IndexedFile>,
    references: HashMap<SymbolId, Vec<SourceSpan>>,
    diagnostics: Vec<DeclarationDiagnostic>,
}

/// Original name for the revision index used by direct-call clients.
pub type ResolvedCallIndex = ResolvedMemberIndex;

impl ResolvedMemberIndex {
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
        let mut targets = HashMap::new();
        let mut valid_files = Vec::new();
        for file in sources.files() {
            let navigation = match file.navigation() {
                Ok(navigation) => navigation,
                Err(error) => {
                    index.diagnostics.push(error.diagnostic(file.ast()));
                    continue;
                }
            };
            let mut field_declarations = Vec::new();
            for class in file.classes() {
                for field in class
                    .constructor_fields
                    .iter()
                    .copied()
                    .chain(class.fields().map(|f| f.symbol))
                {
                    match crate::source_field_access::field_target(
                        file,
                        navigation,
                        class.symbol,
                        field,
                        store,
                    ) {
                        Ok(Some(target)) => {
                            targets.insert(field, target);
                            field_declarations.push((field, target));
                        }
                        Ok(None) => {}
                        Err(error) => index.diagnostics.push(error.diagnostic(file.ast())),
                    }
                }
            }
            field_declarations.sort_unstable_by_key(|(_, target)| target.name.start());
            valid_files.push((file, navigation, field_declarations));
        }
        for (file, navigation, field_declarations) in valid_files {
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
            let mut fields = Vec::new();
            for selection in file.named().syntax().member_names.term_names() {
                let Some((field, kind)) = crate::source_field_access::field_binding(
                    file, navigation, selection, bodies, typed_ast, store,
                ) else {
                    continue;
                };
                if let Some(&target) = targets.get(&field) {
                    let origin =
                        SourceSpan::new(file.source(), Span::without_point(selection.name));
                    index.references.entry(field).or_default().push(origin);
                    fields.push(ResolvedFieldAccess {
                        origin,
                        field,
                        kind,
                        target,
                    });
                }
            }
            index.files.insert(
                file.source(),
                IndexedFile {
                    calls,
                    fields,
                    field_declarations,
                    method_declarations: navigation.method_targets(),
                },
            );
        }
        index
    }

    /// File indexing and field declaration errors in input file order.
    pub fn diagnostics(&self) -> &[DeclarationDiagnostic] {
        &self.diagnostics
    }

    /// Looks up a written callee token by half-open UTF-8 byte offset.
    /// Missing files, unchecked calls and offsets outside a name yield `None`.
    pub fn call_at(&self, source: SourceId, offset: u32) -> Option<&ResolvedSourceCall> {
        let calls = &self.files.get(&source)?.calls;
        let i = calls.partition_point(|c| c.origin.span().range().end() <= offset);
        calls
            .get(i)
            .filter(|c| c.origin.span().range().contains(offset))
    }

    /// Identifies a source method from its written declaration name or a resolved
    /// direct call. Returns the original symbol and its declaration ranges.
    /// Includes named methods without call sites; a position outside both
    /// kinds of half-open name tokens yields `None`.
    pub fn method_at(
        &self,
        source: SourceId,
        offset: u32,
    ) -> Option<(SymbolId, SourceMethodTarget)> {
        if let Some(call) = self.call_at(source, offset) {
            return Some((call.method, call.target));
        }
        let declarations = &self.files.get(&source)?.method_declarations;
        let i = declarations.partition_point(|(_, target)| target.name.end() <= offset);
        declarations
            .get(i)
            .copied()
            .filter(|(_, target)| target.name.contains(offset))
    }

    /// Gets the committed read or write at a written field name. The
    /// receiver, the dot and the exclusive token end are outside the usage.
    pub fn field_at(&self, source: SourceId, offset: u32) -> Option<&ResolvedFieldAccess> {
        let fields = &self.files.get(&source)?.fields;
        let i = fields.partition_point(|f| f.origin.span().range().end() <= offset);
        fields
            .get(i)
            .filter(|f| f.origin.span().range().contains(offset))
    }

    /// Source usage and definition ranges at a committed call or field access.
    pub fn definition_at(
        &self,
        source: SourceId,
        offset: u32,
    ) -> Option<(SourceSpan, SourceMemberTarget)> {
        self.call_at(source, offset)
            .map(|c| (c.origin, c.target))
            .or_else(|| self.field_at(source, offset).map(|f| (f.origin, f.target)))
    }

    /// Identifies the method or supported field at its declaration name or one of
    /// its committed, explicitly written usages. Includes unused declarations.
    pub fn member_at(
        &self,
        source: SourceId,
        offset: u32,
    ) -> Option<(SymbolId, SourceMemberTarget)> {
        if let Some(method) = self.method_at(source, offset) {
            return Some(method);
        }
        if let Some(access) = self.field_at(source, offset) {
            return Some((access.field, access.target));
        }
        let declarations = &self.files.get(&source)?.field_declarations;
        let i = declarations.partition_point(|(_, target)| target.name.end() <= offset);
        declarations
            .get(i)
            .copied()
            .filter(|(_, target)| target.name.contains(offset))
    }

    /// Direct calls or explicit reads and writes of this exact member, in input file order then
    /// token order. Omits the definition. Unknown and unused symbols yield an
    /// empty slice. Supply a symbol from the original session.
    pub fn references_to(&self, member: SymbolId) -> &[SourceSpan] {
        self.references.get(&member).map_or(&[], Vec::as_slice)
    }
}
