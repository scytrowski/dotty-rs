//! Retained declaration and usage sites for source symbols in one revision.
use std::collections::HashMap;

use crate::core::{
    AstArena, SemanticStore, SourceId, SourceSpan, Span, SymbolId, SymbolKind, Typed,
};
use crate::source_declarations::DeclarationDiagnostic;
use crate::source_field_access::TermAccess;
use crate::{
    CheckedClassBodies, NamedClassSources, ResolvedFieldAccess, ResolvedSourceCall,
    SourceMemberTarget, SourceMethodParameter, SourceMethodTarget,
};

#[derive(Debug)]
struct IndexedFile {
    locals: Vec<(SymbolId, SourceMemberTarget)>,
    local_uses: Vec<(SourceSpan, (usize, crate::ValueAccessKind))>,
    parameters: Vec<SourceMethodParameter>,
    parameter_uses: Vec<(SourceSpan, usize)>,
    calls: Vec<ResolvedSourceCall>,
    fields: Vec<ResolvedFieldAccess>,
    field_declarations: Vec<(SymbolId, SourceMemberTarget)>,
    method_declarations: Vec<(SymbolId, SourceMethodTarget)>,
}

/// A supported source symbol at a declaration or committed usage in one revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceSymbolSite {
    /// Canonical identity to pass to [`ResolvedMemberIndex::references_to`].
    pub symbol: SymbolId,
    /// Semantic category: method, field, method parameter, or local.
    pub kind: SymbolKind,
    /// Declaration range and exact written name token.
    pub target: SourceMemberTarget,
    /// Written use token, or `None` when the query hit the declaration.
    pub usage: Option<SourceSpan>,
}

fn declaration_at(
    declarations: &[(SymbolId, SourceMemberTarget)],
    offset: u32,
) -> Option<(SymbolId, SourceMemberTarget)> {
    let i = declarations.partition_point(|(_, target)| target.name.end() <= offset);
    declarations
        .get(i)
        .copied()
        .filter(|(_, target)| target.name.contains(offset))
}

fn use_at<T: Copy>(uses: &[(SourceSpan, T)], offset: u32) -> Option<(SourceSpan, T)> {
    let i = uses.partition_point(|(span, _)| span.span().range().end() <= offset);
    uses.get(i)
        .copied()
        .filter(|(span, _)| span.span().range().contains(offset))
}

/// Index of source methods, fields, named method parameters and ordinary method locals in one session.
/// Usage sites require committed body bindings.
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
            let parameters =
                crate::source_parameters::declarations(file, store, &mut index.diagnostics);
            valid_files.push((file, navigation, field_declarations, parameters));
        }
        for (file, navigation, field_declarations, parameters) in valid_files {
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
            let locals = crate::source_locals::declarations(file, navigation, bodies, store);
            let mut local_uses = Vec::new();
            let slots: HashMap<_, _> = parameters
                .iter()
                .enumerate()
                .map(|(i, p)| (p.parameter, i))
                .chain(locals.iter().enumerate().map(|(i, (id, _))| (*id, i)))
                .collect();
            let mut fields = Vec::new();
            let mut parameter_uses = Vec::new();
            for name in file.named().syntax().member_names.term_names() {
                let Some(access) = crate::source_field_access::term_binding(
                    file, navigation, name, bodies, typed_ast, store,
                ) else {
                    continue;
                };
                let origin = SourceSpan::new(file.source(), Span::without_point(name.name));
                let symbol = match access {
                    TermAccess::Parameter(id) => {
                        let Some(&slot) = slots.get(&id) else {
                            continue;
                        };
                        parameter_uses.push((origin, slot));
                        id
                    }
                    TermAccess::Local(id, kind) => {
                        let Some(&slot) = slots.get(&id) else {
                            continue;
                        };
                        local_uses.push((origin, (slot, kind)));
                        id
                    }
                    TermAccess::Field(field, kind) => {
                        let Some(&target) = targets.get(&field) else {
                            continue;
                        };
                        fields.push(ResolvedFieldAccess {
                            origin,
                            field,
                            kind,
                            target,
                        });
                        field
                    }
                };
                index.references.entry(symbol).or_default().push(origin);
            }
            index.files.insert(
                file.source(),
                IndexedFile {
                    locals,
                    local_uses,
                    parameters,
                    parameter_uses,
                    calls,
                    fields,
                    field_declarations,
                    method_declarations: navigation.method_targets(),
                },
            );
        }
        index
    }

    /// File indexing and declaration errors in input file order.
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
        declaration_at(&self.files.get(&source)?.method_declarations, offset)
    }

    /// Finds a named parameter at its declaration or a committed bare read.
    /// Includes unused declarations and successful members of a partial body.
    /// Only ordinary parameters of class methods with one value list are indexed.
    /// Supply a UTF-8 byte offset from this revision; the token end is exclusive.
    pub fn parameter_at(&self, source: SourceId, offset: u32) -> Option<SourceMethodParameter> {
        if let Some((_, parameter)) = self.parameter_use_at(source, offset) {
            return Some(parameter);
        }
        self.parameter_declaration_at(source, offset)
    }

    fn parameter_declaration_at(
        &self,
        source: SourceId,
        offset: u32,
    ) -> Option<SourceMethodParameter> {
        let parameters = &self.files.get(&source)?.parameters;
        let i = parameters.partition_point(|p| p.target.name.end() <= offset);
        parameters
            .get(i)
            .copied()
            .filter(|p| p.target.name.contains(offset))
    }

    fn parameter_use_at(
        &self,
        source: SourceId,
        offset: u32,
    ) -> Option<(SourceSpan, SourceMethodParameter)> {
        let file = self.files.get(&source)?;
        let (origin, slot) = use_at(&file.parameter_uses, offset)?;
        Some((origin, file.parameters[slot]))
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

    /// Classifies a committed use of a field, ordinary local, or method parameter
    /// at its written name. Direct assignment targets are writes; other supported
    /// value uses are reads. Declarations and method calls yield `None`.
    pub fn value_access_at(&self, source: SourceId, offset: u32) -> Option<crate::ValueAccessKind> {
        let file = self.files.get(&source)?;
        if let Some((_, (_, kind))) = use_at(&file.local_uses, offset) {
            return Some(kind);
        }
        self.field_at(source, offset)
            .map(|f| f.kind)
            .or_else(|| use_at(&file.parameter_uses, offset).map(|_| crate::ValueAccessKind::Read))
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
        declaration_at(&self.files.get(&source)?.field_declarations, offset)
    }

    /// Looks up a written method, field, ordinary parameter or method local by half-open
    /// UTF-8 byte offset. Local declarations and all usage sites require committed body
    /// bindings. Other supported declarations are available before body checking.
    /// Unsupported declarations yield `None`. The result can be retained with the index.
    pub fn symbol_at(&self, source: SourceId, offset: u32) -> Option<SourceSymbolSite> {
        let file = self.files.get(&source)?;
        let (symbol, kind, target, usage) = if let Some(c) = self.call_at(source, offset) {
            (c.method, SymbolKind::Method, c.target, Some(c.origin))
        } else if let Some(f) = self.field_at(source, offset) {
            (f.field, SymbolKind::Field, f.target, Some(f.origin))
        } else if let Some((origin, (slot, _))) = use_at(&file.local_uses, offset) {
            let (id, target) = file.locals[slot];
            (id, SymbolKind::Local, target, Some(origin))
        } else if let Some((id, target)) = declaration_at(&file.locals, offset) {
            (id, SymbolKind::Local, target, None)
        } else if let Some((origin, p)) = self.parameter_use_at(source, offset) {
            (p.parameter, SymbolKind::Parameter, p.target, Some(origin))
        } else if let Some((id, target)) = declaration_at(&file.method_declarations, offset) {
            (id, SymbolKind::Method, target, None)
        } else if let Some((id, target)) = declaration_at(&file.field_declarations, offset) {
            (id, SymbolKind::Field, target, None)
        } else {
            let p = self.parameter_declaration_at(source, offset)?;
            (p.parameter, SymbolKind::Parameter, p.target, None)
        };
        Some(SourceSymbolSite {
            symbol,
            kind,
            target,
            usage,
        })
    }

    /// Committed uses of this exact symbol, in input file order then
    /// token order. Omits the definition. Unknown and unused symbols yield an
    /// empty slice. Supply a symbol from the original session.
    pub fn references_to(&self, symbol: SymbolId) -> &[SourceSpan] {
        self.references.get(&symbol).map_or(&[], Vec::as_slice)
    }
}
