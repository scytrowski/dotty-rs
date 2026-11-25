//! Provisional signatures for unfinished calls in a disposable editor revision.
use std::{collections::HashMap, sync::Arc};

use crate::core::{
    AstArena, Definitions, NoResolver, Packages, SemanticStore, SourceId, SymbolId, TreeId,
    TreeKind, TypeId, Untyped, Visibility,
};
use crate::parser::{CallArgumentList, SourceCallArguments};
use crate::typer::{ExpressionContext, SourceTyper};
use crate::{
    ClassHeaderBindings, ClassHeaderPhase, MemberSignaturePhase, NamedClassSource,
    NamedClassSources, ParsedSource, SourceMemberTarget, SourceNamedType, SourceSignatureParameter,
    SourceSymbolInfo, SourceSymbolSignature, SyntaxDiagnostic,
};

/// A source overload compatible with the written prefix. This is a declaration
/// from this revision, even when it is the only remaining candidate.
#[derive(Debug)]
pub struct CallSignatureCandidate {
    /// Method identity in the original semantic store.
    pub method: SymbolId,
    /// Source declaration and its written name.
    pub target: SourceMemberTarget,
    /// Unquoted semantic method name.
    pub name: Box<str>,
    /// Ordered formals of the ordinary value clause.
    pub parameters: Box<[SourceSignatureParameter]>,
    /// Declared or inferred result of the candidate.
    pub result: SourceNamedType,
}

/// Alternatives at an insertion point, distinct from a committed
/// [`crate::ResolvedCallSignature`].
#[derive(Debug)]
pub struct CallSignatureCandidates<'a> {
    /// Zero-based slot after the opening parenthesis or most recent comma.
    pub active_parameter: usize,
    /// Compatible overloads in semantic member lookup order. Always nonempty.
    pub candidates: Vec<&'a CallSignatureCandidate>,
}

#[derive(Debug)]
struct FileCandidates {
    syntax: Arc<SourceCallArguments>,
    // For each recovered application, number of consecutive written arguments
    // compatible with each candidate. Earlier cursor positions reuse the prefix.
    matches: HashMap<TreeId<Untyped>, Vec<(CallSignatureCandidate, usize)>>,
}

/// Arena-independent candidate information for a single editor revision.
/// No provisional application is added to the compiler's checked call index.
#[derive(Debug, Default)]
pub struct IncompleteCallIndex {
    files: HashMap<SourceId, FileCandidates>,
}

impl IncompleteCallIndex {
    /// Finishes a disposable session after checking its clean source providers.
    /// Pass syntax-error files with their original filenames, bound in the same
    /// interner. File IDs must be unique and disjoint from the clean providers.
    /// Retain syntax diagnostics for publication before passing the parses here.
    ///
    /// Recovered naming and signature preparation use the consumed store and
    /// packages. Only owned candidate data escapes. Unavailable declarations,
    /// rejected contexts and unsupported calls contribute no candidates.
    /// Clean provider signatures must already be complete. Public source methods
    /// with one plain positional list and named parameter and result types are
    /// supported. The receiver and prefix are typed in the enclosing method
    /// context, including preceding ordinary eager local `val` and `var`
    /// statements in enclosing blocks. Unsupported scopes or ill-typed
    /// preceding statements suppress candidates for that call.
    /// Filtering uses value-type conformance and requires a formal at the active
    /// slot. The probe uses source declarations and types available in this
    /// session and performs no external classpath loading.
    pub fn build<'a>(
        providers: &NamedClassSources<'_>,
        unfinished: impl IntoIterator<Item = (&'a str, ParsedSource<SyntaxDiagnostic>)>,
        mut store: SemanticStore,
        defs: Definitions,
        mut packages: Packages,
    ) -> Self {
        let mut files = Vec::new();
        for (filename, parsed) in unfinished {
            if parsed.diagnostics.is_empty()
                || parsed
                    .syntax
                    .call_arguments
                    .unfinished_lists()
                    .next()
                    .is_none()
            {
                continue;
            }
            if let Ok(named) = parsed.name_with_status(
                filename,
                &mut store,
                &mut packages,
                crate::namer::SourceParseStatus::Recovered,
            ) && let Ok(projected) = named.project_classes(&store)
            {
                files.push(projected.file);
            }
        }
        let mut index = Self::default();
        if files.is_empty() {
            return index;
        }
        let Ok(recovered) = NamedClassSources::new(&files, &store) else {
            return index;
        };
        let mut arena = AstArena::new();
        let mut bindings = ClassHeaderBindings::default();
        let mut headers = ClassHeaderPhase::prepare(
            &recovered,
            &mut store,
            defs,
            &packages,
            &mut arena,
            &mut bindings,
        );
        headers.complete_all();
        let ready = headers.finish().ready;
        if let Ok(selection) = recovered.member_signatures(ready, &store) {
            MemberSignaturePhase::new(&selection, &mut store, defs, &packages)
                .complete_all(&mut NoResolver);
        }
        let Ok(all) = NamedClassSources::new(providers.files().chain(&files), &store) else {
            return index;
        };
        for file in &files {
            let syntax = &file.named().syntax().call_arguments;
            // Both iterators are in source order. Use the callee token to
            // locate a method even when the closing parenthesis is missing.
            // The typer checks ancestry using the actual application tree.
            let mut methods = file
                .classes()
                .iter()
                .flat_map(|c| c.methods())
                .filter_map(|m| Some((file.ast().get(m.tree).position?.span().range(), m.symbol)))
                .peekable();
            let mut typer = SourceTyper::new(
                file.ast(),
                file.source(),
                file.index(),
                &mut store,
                defs,
                &packages,
            );
            let mut matches = HashMap::new();
            for list in syntax.unfinished_lists().filter(|list| list.positional) {
                let Some(tree) = list.unfinished else {
                    continue;
                };
                let Some(callee) = list.callee else { continue };
                while methods
                    .peek()
                    .is_some_and(|(range, _)| range.end() <= callee.start())
                {
                    methods.next();
                }
                let Some(&(range, owner)) = methods.peek() else {
                    break;
                };
                if !range.contains(callee.start()) {
                    continue;
                }
                if let Ok(Some(Some(found))) =
                    typer.inspect_method_expression(owner, tree, |probe, context| {
                        prefix_candidates(file, tree, list, &all, probe, context)
                    })
                {
                    matches.insert(tree, found);
                }
            }
            index.files.insert(
                file.source(),
                FileCandidates {
                    syntax: Arc::clone(syntax),
                    matches,
                },
            );
        }
        index
    }

    /// Returns the innermost unfinished call's alternatives at a UTF-8 insertion
    /// offset. Only complete argument expressions ending at or before the cursor
    /// constrain the candidates. A failed prefix or exhausted arity yields
    /// `None`; completing the list transfers the query to
    /// [`crate::ResolvedMemberIndex::call_signature_at`].
    pub fn at(&self, source: SourceId, offset: u32) -> Option<CallSignatureCandidates<'_>> {
        let file = self.files.get(&source)?;
        let list = file.syntax.unfinished_at(offset)?;
        let active_parameter = list.slot_at(offset)?;
        let written = list
            .arguments
            .partition_point(|range| range.end() <= offset);
        let candidates: Vec<_> = file
            .matches
            .get(&list.unfinished?)?
            .iter()
            .filter(|(c, prefix)| *prefix >= written && active_parameter < c.parameters.len())
            .map(|(candidate, _)| candidate)
            .collect();
        (!candidates.is_empty()).then_some(CallSignatureCandidates {
            active_parameter,
            candidates,
        })
    }
}

fn value_type(
    typer: &mut SourceTyper<'_>,
    tree: TreeId<Untyped>,
    context: ExpressionContext,
) -> Option<TypeId> {
    let typed = typer.type_expression(tree, context).ok()?;
    typer
        .widen_expression_type(typer.typed_ast().get(typed).ty)
        .ok()
}

fn prefix_candidates(
    file: &NamedClassSource,
    tree: TreeId<Untyped>,
    list: &CallArgumentList,
    all: &NamedClassSources<'_>,
    typer: &mut SourceTyper<'_>,
    context: ExpressionContext,
) -> Option<Vec<(CallSignatureCandidate, usize)>> {
    list.callee?;
    let TreeKind::Apply(app) = &file.ast().get(tree).kind else {
        return None;
    };
    let TreeKind::Select(select) = &file.ast().get(app.function).kind else {
        return None;
    };
    let receiver = value_type(typer, select.qualifier, context)?;
    let methods = typer.lookup_members(receiver, select.name).ok()?;
    let prefix: Vec<_> = app
        .args
        .iter()
        .take(list.arguments.len())
        .map_while(|&arg| value_type(typer, arg, context))
        .collect();
    let mut found = Vec::new();
    for member in methods {
        let method = member.symbol;
        if typer.store().symbols.get(method).visibility != Visibility::Public {
            continue;
        }
        let Some((provider, _)) = all.declaration(method) else {
            continue;
        };
        let Some(target) = provider
            .navigation()
            .ok()
            .and_then(|n| n.target(provider.source(), method))
        else {
            continue;
        };
        let Some(SourceSymbolInfo {
            name,
            signature:
                SourceSymbolSignature::Method {
                    parameters: Some(parameters),
                    result,
                    ..
                },
        }) = SourceSymbolInfo::capture(typer.store(), method)
        else {
            continue;
        };
        let compatible = prefix
            .iter()
            .zip(&parameters)
            .take_while(|(arg, formal)| typer.conforms(**arg, formal.ty.ty).unwrap_or(false))
            .count();
        found.push((
            CallSignatureCandidate {
                method,
                target,
                name,
                parameters,
                result,
            },
            compatible,
        ));
    }
    Some(found)
}
