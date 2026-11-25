//! Provisional signatures for unfinished calls in a disposable editor revision.
mod constructor;
use constructor::ConstructorCandidate;
pub use constructor::ProvisionalConstructorSignature;
use std::{collections::HashMap, sync::Arc};

use crate::core::{
    AstArena, Definitions, NoResolver, Packages, SemanticStore, SourceId, SymbolId, SymbolKind,
    TreeId, TreeKind, TypeId, Untyped,
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
    /// Written slot after the opening parenthesis or most recent comma. For named
    /// arguments, use each candidate's formal ordinal in [`ActiveCallSignature`].
    pub active_parameter: usize,
    /// Compatible overloads in semantic member lookup order. Always nonempty.
    pub candidates: Vec<ActiveCallSignature<'a>>,
}

/// One viable signature and the formal selected by the cursor in that signature.
#[derive(Debug)]
pub struct ActiveCallSignature<'a> {
    /// Borrowed method declaration from this revision.
    pub signature: &'a CallSignatureCandidate,
    /// Zero-based ordinal in this candidate's declaration.
    pub active_parameter: usize,
}

impl std::ops::Deref for ActiveCallSignature<'_> {
    type Target = CallSignatureCandidate;
    fn deref(&self) -> &Self::Target {
        self.signature
    }
}

#[derive(Debug)]
struct CandidatePrefix {
    signature: CallSignatureCandidate,
    prefix: PrefixMatch,
}

#[derive(Debug)]
struct PrefixMatch {
    compatible: usize,
    // Empty for positional lists, whose mapping is the identity.
    named_slots: Vec<SlotHint>,
}

#[derive(Debug)]
struct SlotHint {
    before: Option<usize>,
    bound: Option<usize>,
}

#[derive(Debug)]
struct FileCandidates {
    syntax: Arc<SourceCallArguments>,
    // For each recovered application, number of consecutive written arguments
    // compatible with each candidate. Earlier cursor positions reuse the prefix.
    constructors: HashMap<TreeId<Untyped>, ConstructorCandidate>,
    matches: HashMap<TreeId<Untyped>, Vec<CandidatePrefix>>,
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
    /// Clean provider signatures must already be complete. Accessible source methods
    /// with one plain argument list and named parameter and result types are
    /// supported for unqualified calls and explicit receivers. The written
    /// arguments and any receiver are typed in their lexical
    /// context, including inside the initializer of an ordinary local `val` or
    /// `var`. Earlier statements in each enclosing block are typed in order.
    /// Unsupported scopes or ill-typed preceding statements suppress candidates.
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
            let mut constructors = HashMap::new();
            let mut matches = HashMap::new();
            for list in syntax.unfinished_lists().filter(|list| list.regular) {
                let Some(tree) = list.unfinished else {
                    continue;
                };
                let allocation = match list.constructor {
                    Some(crate::parser::ConstructorSyntax::Allocation(name)) => Some(name),
                    _ => None,
                };
                let Some(callee) = allocation.or(list.callee) else {
                    continue;
                };
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
                if allocation.is_some() {
                    if let Ok(Some(Some(candidate))) =
                        typer.inspect_method_expression(owner, tree, |probe, context| {
                            ConstructorCandidate::capture(
                                file,
                                list,
                                &all,
                                probe,
                                context,
                                crate::SourceConstructorUse::Allocation,
                            )
                        })
                    {
                        constructors.insert(tree, candidate);
                    }
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
                    constructors,
                    matches,
                },
            );
        }
        index
    }

    /// Looks up the source primary constructor at a UTF-8 insertion point after
    /// an unfinished `new C(` inside a method. Requires a public constructor with
    /// a single required parameter list and simple named types in this revision.
    /// Completed operands constrain the prefix up to the cursor. Named labels
    /// select their declared ordinal, including before a value has been entered.
    pub fn primary_constructor_at(
        &self,
        source: SourceId,
        offset: u32,
    ) -> Option<ProvisionalConstructorSignature<'_>> {
        let file = self.files.get(&source)?;
        let list = file.syntax.unfinished_at(offset)?;
        let candidate = file.constructors.get(&list.application)?;
        let cursor = PrefixCursor::new(list, offset)?;
        Some(candidate.at(cursor.formal(&candidate.prefix, candidate.parameters.len())?))
    }

    /// Returns the innermost unfinished call's alternatives at a UTF-8 insertion
    /// offset. Only complete argument expressions ending at or before the cursor
    /// constrain the candidates. A failed prefix or exhausted arity yields
    /// `None`; completing the list transfers the query to
    /// [`crate::ResolvedMemberIndex::call_signature_at`].
    pub fn at(&self, source: SourceId, offset: u32) -> Option<CallSignatureCandidates<'_>> {
        let file = self.files.get(&source)?;
        let list = file.syntax.unfinished_at(offset)?;
        let cursor = PrefixCursor::new(list, offset)?;
        let candidates: Vec<_> = file
            .matches
            .get(&list.unfinished?)?
            .iter()
            .filter_map(|c| {
                let formal = cursor.formal(&c.prefix, c.signature.parameters.len())?;
                Some(ActiveCallSignature {
                    signature: &c.signature,
                    active_parameter: formal,
                })
            })
            .collect();
        (!candidates.is_empty()).then_some(CallSignatureCandidates {
            active_parameter: cursor.slot,
            candidates,
        })
    }
}

struct PrefixCursor {
    slot: usize,
    written: usize,
    before_operand: bool,
}

impl PrefixCursor {
    fn new(list: &CallArgumentList, offset: u32) -> Option<Self> {
        let active_parameter = list.slot_at(offset)?;
        let written = list
            .arguments
            .partition_point(|range| range.end() <= offset);
        let before_operand = list
            .named_arguments
            .binary_search_by_key(&active_parameter, |label| label.written)
            .ok()
            .map_or_else(
                || {
                    list.arguments
                        .get(active_parameter)
                        .is_none_or(|arg| offset < arg.start())
                },
                |i| offset < list.named_arguments[i].label.start(),
            );
        Some(Self {
            slot: active_parameter,
            written,
            before_operand,
        })
    }

    fn formal(&self, prefix: &PrefixMatch, parameter_count: usize) -> Option<usize> {
        if self.written > prefix.compatible {
            return None;
        }
        if prefix.named_slots.is_empty() {
            return (self.slot < parameter_count).then_some(self.slot);
        }
        let hint = prefix.named_slots.get(self.slot)?;
        if self.before_operand {
            hint.before
        } else {
            hint.bound
        }
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
) -> Option<Vec<CandidatePrefix>> {
    list.callee?;
    let TreeKind::Apply(app) = &file.ast().get(tree).kind else {
        return None;
    };
    let methods = match &file.ast().get(app.function).kind {
        TreeKind::Ident(_) => {
            let bucket = typer
                .lookup_identifier_candidates(app.function, context)
                .ok()?;
            // A value in the winning scope can shadow every class overload.
            if bucket
                .iter()
                .any(|&id| typer.store().symbols.get(id).kind != SymbolKind::Method)
            {
                return None;
            }
            bucket
        }
        TreeKind::Select(select) => {
            let receiver = value_type(typer, select.qualifier, context)?;
            typer
                .lookup_members(receiver, select.name)
                .ok()?
                .into_iter()
                .map(|member| member.symbol)
                .collect()
        }
        _ => return None,
    };
    let prefix = written_types(typer, context, file, list, &app.args);
    let mut found = Vec::new();
    for method in methods {
        if !typer
            .is_member_accessible_at(method, app.function, context)
            .ok()?
        {
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
        let prefix = PrefixMatch::new(list, &parameters, &prefix, typer);
        found.push(CandidatePrefix {
            signature: CallSignatureCandidate {
                method,
                target,
                name,
                parameters,
                result,
            },
            prefix,
        });
    }
    Some(found)
}

fn written_types(
    typer: &mut SourceTyper<'_>,
    context: ExpressionContext,
    file: &NamedClassSource,
    list: &CallArgumentList,
    args: &[TreeId<Untyped>],
) -> Vec<TypeId> {
    args.iter()
        .take(list.arguments.len())
        .map_while(|&arg| {
            let value = match &file.ast().get(arg).kind {
                TreeKind::NamedArg(named) => named.arg,
                _ => arg,
            };
            value_type(typer, value, context)
        })
        .collect()
}

impl PrefixMatch {
    fn new(
        list: &CallArgumentList,
        parameters: &[SourceSignatureParameter],
        prefix: &[TypeId],
        typer: &mut SourceTyper<'_>,
    ) -> Self {
        let named_slots = if list.positional {
            Vec::new()
        } else {
            candidate_slots(list, parameters)
        };
        let compatible = prefix
            .iter()
            .enumerate()
            .take_while(|(written, arg)| {
                let formal = if list.positional {
                    Some(*written)
                } else {
                    named_slots.get(*written).and_then(|hint| hint.bound)
                };
                formal
                    .and_then(|i| parameters.get(i))
                    .is_some_and(|p| typer.conforms(**arg, p.ty.ty).unwrap_or(false))
            })
            .count();
        Self {
            compatible,
            named_slots,
        }
    }
}

fn candidate_slots(
    list: &CallArgumentList,
    parameters: &[SourceSignatureParameter],
) -> Vec<SlotHint> {
    let ordinals: HashMap<_, _> = parameters
        .iter()
        .enumerate()
        .map(|(i, p)| (&*p.name, i))
        .collect();
    let mut order = crate::typer::ArgumentSlots::new(parameters.len());
    let mut labels = list.named_arguments.iter().peekable();
    let mut hints = Vec::new();
    for written in 0..=list.arguments.len() {
        let before = order.next_unfilled();
        let bound = if let Some(label) = labels.next_if(|a| a.written == written) {
            ordinals
                .get(&*label.name)
                .and_then(|ordinal| order.bind(Some(*ordinal)).ok())
        } else if written == list.arguments.len() {
            before
        } else {
            order.bind(None).ok()
        };
        hints.push(SlotHint { before, bound });
        if bound.is_none() {
            break;
        }
    }
    hints
}
