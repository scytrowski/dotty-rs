//! Retained declaration and usage sites for source symbols in one revision.
use std::collections::HashMap;

use crate::core::{
    AstArena, SemanticStore, SourceId, SourceSpan, Span, SymbolId, SymbolKind, Typed,
};
use crate::source_declarations::DeclarationDiagnostic;
use crate::source_field_access::TermAccess;
use crate::source_parameters::ParameterDeclaration;
use crate::{
    CheckedClassBodies, NamedClassSources, ResolvedFieldAccess, ResolvedSourceCall,
    SourceMemberTarget, SourceMethodParameter, SourceMethodTarget,
};

#[derive(Debug)]
struct IndexedFile {
    method_types: Vec<crate::ResolvedMethodType>,
    constructions: Vec<crate::ResolvedSourceConstruction>,
    parents: Vec<ResolvedSourceParent>,
    class_declarations: Vec<(SymbolId, SourceMemberTarget)>,
    locals: Vec<(SymbolId, SourceMemberTarget)>,
    local_uses: Vec<(SourceSpan, (usize, crate::ValueAccessKind))>,
    parameters: Vec<ParameterDeclaration>,
    parameter_uses: Vec<(SourceSpan, usize)>,
    calls: Vec<ResolvedSourceCall>,
    fields: Vec<ResolvedFieldAccess>,
    field_declarations: Vec<(SymbolId, SourceMemberTarget)>,
    method_declarations: Vec<(SymbolId, SourceMethodTarget)>,
}

/// One written direct parent of an ordinary class in the selected sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedSourceParent {
    /// Canonical source class whose header was completed in this session.
    pub child: SymbolId,
    /// Canonical source superclass from the completed class information.
    pub parent: SymbolId,
    /// Exact written type name following `extends`, excluding its argument list.
    pub origin: SourceSpan,
    /// Declaration and name token of the superclass in the same revision.
    pub target: SourceMemberTarget,
}

/// A supported source symbol at a declaration or committed usage in one revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceSymbolSite {
    /// Canonical identity to pass to [`ResolvedMemberIndex::references_to`].
    pub symbol: SymbolId,
    /// Semantic category: class, method, field, parameter, or local.
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

/// Index of ordinary source classes, methods, fields, named method and constructor parameters, and ordinary method locals in one session.
/// A parent name requires completed class information. Other uses require committed body or header bindings.
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
        Self::build_with_headers(
            sources,
            bodies,
            &crate::ClassHeaderBindings::default(),
            typed_ast,
            store,
        )
    }

    /// Includes constructor parameter reads from committed parent arguments as well
    /// as body uses. Supply the headers, bodies, final typed arena and store of
    /// the same session. Blocked and failed header attempts contribute no uses.
    /// References are ordered by input file and written token across both phases.
    pub fn build_with_headers(
        sources: &NamedClassSources<'_>,
        bodies: &CheckedClassBodies,
        headers: &crate::ClassHeaderBindings,
        typed_ast: &AstArena<Typed>,
        store: &SemanticStore,
    ) -> Self {
        let headers = (!headers.index.is_empty()).then_some(headers);
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
            let mut class_declarations = Vec::new();
            let mut field_declarations = Vec::new();
            for class in file.classes() {
                if let Some(name) = file.named().syntax().member_names.class_name(class.tree) {
                    // The navigation index has validated this canonical tree and its class range.
                    let declaration = file.ast().get(class.tree).position.unwrap().span().range();
                    if name.start() < declaration.start() || name.end() > declaration.end() {
                        let error = crate::source_declarations::DeclarationError::InvalidBinding {
                            source: file.source(),
                            tree: class.tree,
                        };
                        index.diagnostics.push(error.diagnostic(file.ast()));
                    } else {
                        let target = SourceMemberTarget {
                            source: file.source(),
                            declaration,
                            name,
                        };
                        targets.insert(class.symbol, target);
                        class_declarations.push((class.symbol, target));
                    }
                }
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
            valid_files.push((
                file,
                navigation,
                class_declarations,
                field_declarations,
                parameters,
            ));
        }
        for (file, navigation, class_declarations, field_declarations, parameters) in valid_files {
            let parents: Vec<_> = file
                .classes()
                .iter()
                .filter_map(|class| {
                    let [tree] = class.parents.as_slice() else {
                        return None;
                    };
                    let name = file.named().syntax().member_names.parent_name(*tree)?;
                    let parent =
                        crate::source_declarations::direct_source_parent(store, class.symbol)?;
                    Some(ResolvedSourceParent {
                        child: class.symbol,
                        parent,
                        origin: SourceSpan::new(file.source(), Span::without_point(name)),
                        target: *targets.get(&parent)?,
                    })
                })
                .collect();
            let constructions: Vec<_> = file
                .named()
                .syntax()
                .member_names
                .constructions()
                .iter()
                .filter_map(|&(root, name)| {
                    let owner = navigation.class_at(name.start())?;
                    let bindings = bodies.bindings(owner)?;
                    let (class, constructor) = crate::source_construction::selected_constructor(
                        file, root, bindings, typed_ast, store,
                    )?;
                    let (_, declaration) = sources.class(class)?;
                    (declaration.constructor == constructor).then_some(
                        crate::ResolvedSourceConstruction {
                            class,
                            constructor,
                            origin: SourceSpan::new(file.source(), Span::without_point(name)),
                            target: *targets.get(&class)?,
                        },
                    )
                })
                .collect();
            // Both lists are in token order, but a construction can precede a later class header.
            let mut record = |class, span| index.references.entry(class).or_default().push(span);
            let mut allocations = constructions.iter().peekable();
            for parent in &parents {
                while let Some(site) = allocations.next_if(|s| {
                    s.origin.span().range().start() < parent.origin.span().range().start()
                }) {
                    record(site.class, site.origin);
                }
                record(parent.parent, parent.origin);
            }
            for site in allocations {
                record(site.class, site.origin);
            }
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
            let mut slots: HashMap<_, _> = parameters
                .iter()
                .enumerate()
                .map(|(i, p)| (p.symbol(), i))
                .chain(locals.iter().enumerate().map(|(i, (id, _))| (*id, i)))
                .collect();
            for (slot, declaration) in parameters.iter().enumerate() {
                if let ParameterDeclaration::Constructor(p) = declaration {
                    slots.insert(p.formal, slot);
                }
            }
            let mut fields = Vec::new();
            let mut parameter_uses = Vec::new();
            for name in file.named().syntax().member_names.term_names() {
                let Some(access) = crate::source_field_access::term_binding(
                    file, navigation, name, bodies, headers, typed_ast, store,
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
                        parameters[slot].symbol()
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
                    method_types: crate::source_method_types::method_types(file, &targets, store),
                    constructions,
                    parents,
                    class_declarations,
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

    /// Resolves a byte within a simple written class type in a method parameter
    /// or result. Requires a completed ordinary, non-generic method signature
    /// with one plain value list, even when empty. Both files must be indexed.
    /// Body checking is independent. Inferred results, aliases, qualified or
    /// compound annotations and offsets outside the name yield `None`.
    pub fn method_type_at(
        &self,
        source: SourceId,
        offset: u32,
    ) -> Option<&crate::ResolvedMethodType> {
        let sites = &self.files.get(&source)?.method_types;
        let i = sites.partition_point(|s| s.origin.span().range().end() <= offset);
        sites
            .get(i)
            .filter(|s| s.origin.span().range().contains(offset))
    }

    /// Looks up a byte within the unqualified class name of a written `new` in
    /// a checked class body. Returns the selected primary constructor when its
    /// class is in the indexed sources. Missing bindings, unsupported forms and
    /// positions outside the half-open name token yield `None`.
    pub fn construction_at(
        &self,
        source: SourceId,
        offset: u32,
    ) -> Option<&crate::ResolvedSourceConstruction> {
        let sites = &self.files.get(&source)?.constructions;
        let i = sites.partition_point(|s| s.origin.span().range().end() <= offset);
        sites
            .get(i)
            .filter(|s| s.origin.span().range().contains(offset))
    }

    /// Resolves a UTF-8 byte within the name of a single, unqualified, explicit
    /// source superclass. Both index builders use completed class information,
    /// including for a bare parent without expression bindings. Missing targets,
    /// unfinished headers and positions outside the name yield `None`.
    pub fn parent_at(&self, source: SourceId, offset: u32) -> Option<&ResolvedSourceParent> {
        let parents = &self.files.get(&source)?.parents;
        let i = parents.partition_point(|p| p.origin.span().range().end() <= offset);
        parents
            .get(i)
            .filter(|p| p.origin.span().range().contains(offset))
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
        match self.parameter_site_at(source, offset)?.1 {
            ParameterDeclaration::Method(p) => Some(p),
            ParameterDeclaration::Constructor(_) => None,
        }
    }

    /// Looks up a plain primary constructor parameter at its declaration or a committed read. Parent argument reads require [`Self::build_with_headers`].
    /// Includes unused parameters. Explicit constructor fields, contextual and
    /// multiple lists, by-name, repeated and defaulted parameters are outside this query.
    pub fn constructor_parameter_at(
        &self,
        source: SourceId,
        offset: u32,
    ) -> Option<crate::SourceConstructorParameter> {
        match self.parameter_site_at(source, offset)?.1 {
            ParameterDeclaration::Constructor(p) => Some(p),
            ParameterDeclaration::Method(_) => None,
        }
    }

    fn parameter_site_at(
        &self,
        source: SourceId,
        offset: u32,
    ) -> Option<(Option<SourceSpan>, ParameterDeclaration)> {
        if let Some((origin, p)) = self.parameter_use_at(source, offset) {
            Some((Some(origin), p))
        } else {
            Some((None, self.parameter_declaration_at(source, offset)?))
        }
    }

    fn parameter_declaration_at(
        &self,
        source: SourceId,
        offset: u32,
    ) -> Option<ParameterDeclaration> {
        let parameters = &self.files.get(&source)?.parameters;
        let i = parameters.partition_point(|p| p.target().name.end() <= offset);
        parameters
            .get(i)
            .copied()
            .filter(|p| p.target().name.contains(offset))
    }

    fn parameter_use_at(
        &self,
        source: SourceId,
        offset: u32,
    ) -> Option<(SourceSpan, ParameterDeclaration)> {
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

    /// Classifies a committed use of a field, ordinary local, or parameter
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

    /// Looks up an ordinary class, method, field, parameter or method local by half-open
    /// UTF-8 byte offset. Parent names require completed class information. Local declarations
    /// and term uses require committed bindings from their phase. Other supported declarations are available before body checking.
    /// Unsupported declarations yield `None`. The result can be retained with the index.
    pub fn symbol_at(&self, source: SourceId, offset: u32) -> Option<SourceSymbolSite> {
        let file = self.files.get(&source)?;
        let (symbol, kind, target, usage) =
            if let Some((id, target)) = declaration_at(&file.class_declarations, offset) {
                (id, SymbolKind::Class, target, None)
            } else if let Some(parent) = self.parent_at(source, offset) {
                (
                    parent.parent,
                    SymbolKind::Class,
                    parent.target,
                    Some(parent.origin),
                )
            } else if let Some(site) = self.construction_at(source, offset) {
                (
                    site.class,
                    SymbolKind::Class,
                    site.target,
                    Some(site.origin),
                )
            } else if let Some(c) = self.call_at(source, offset) {
                (c.method, SymbolKind::Method, c.target, Some(c.origin))
            } else if let Some(f) = self.field_at(source, offset) {
                (f.field, SymbolKind::Field, f.target, Some(f.origin))
            } else if let Some((origin, (slot, _))) = use_at(&file.local_uses, offset) {
                let (id, target) = file.locals[slot];
                (id, SymbolKind::Local, target, Some(origin))
            } else if let Some((id, target)) = declaration_at(&file.locals, offset) {
                (id, SymbolKind::Local, target, None)
            } else if let Some((origin, p)) = self.parameter_use_at(source, offset) {
                (p.symbol(), SymbolKind::Parameter, p.target(), Some(origin))
            } else if let Some((id, target)) = declaration_at(&file.method_declarations, offset) {
                (id, SymbolKind::Method, target, None)
            } else if let Some((id, target)) = declaration_at(&file.field_declarations, offset) {
                (id, SymbolKind::Field, target, None)
            } else {
                let p = self.parameter_declaration_at(source, offset)?;
                (p.symbol(), SymbolKind::Parameter, p.target(), None)
            };
        Some(SourceSymbolSite {
            symbol,
            kind,
            target,
            usage,
        })
    }

    /// Committed uses of this exact symbol, in input file order then
    /// token order. For classes, covers single unqualified explicit parents and committed
    /// simple `new` expressions. Omits type annotations and declarations. Unknown and unused symbols yield an
    /// empty slice. Supply a symbol from the original session.
    pub fn references_to(&self, symbol: SymbolId) -> &[SourceSpan] {
        self.references.get(&symbol).map_or(&[], Vec::as_slice)
    }
}
