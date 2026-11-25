//! Source evidence retained for rejected ordinary calls.
use crate::core::{SourceDefinition, SourceSpan, Span, SymbolId, SymbolKind, TreeKind, Untyped};
use crate::parser::{CallArgumentList, NamedCallArgument};
use crate::typer::{NamedArgumentRejection, OverloadRejection, TyperError};
use crate::{NamedClassSource, NamedClassSources, SourceMemberTarget, SourceMethodParameter};

pub(crate) fn written_label(
    file: &NamedClassSource,
    site: SourceSpan,
) -> Option<&NamedCallArgument> {
    let range = site.span().range();
    let list = file.named().syntax().call_arguments.at(range.start())?;
    let written = list.argument_index(range.start())?;
    if site.source() != file.source() || list.arguments.get(written) != Some(&range) {
        return None;
    }
    list.named_arguments
        .iter()
        .find(|label| label.written == written)
}

/// Source declaration associated with an overload alternative in one naming revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverloadCandidateTarget {
    /// Declaration range and exact written method name in the provider file.
    pub method: SourceMemberTarget,
    /// Formal identified by this rejection, if it belongs to the supported source clause.
    pub parameter: Option<SourceMethodParameter>,
}

impl NamedClassSources<'_> {
    /// Locates a candidate named by a typer diagnostic from this session. Pass
    /// its own rejection, or `None` for an ambiguous alternative. An unavailable
    /// provider yields `None`; an unknown argument label identifies only the method.
    /// Formal targets cover ordinary non-generic source methods with one value list.
    pub fn overload_candidate_target(
        &self,
        method: SymbolId,
        reason: Option<&OverloadRejection>,
    ) -> Option<OverloadCandidateTarget> {
        let (file, decl) = self.declaration(method)?;
        if decl.kind != SymbolKind::Method {
            return None;
        }
        let target = file.navigation().ok()?.target(decl.source, method)?;
        let parameter = reason.and_then(|reason| candidate_parameter(file, method, reason));
        Some(OverloadCandidateTarget {
            method: target,
            parameter,
        })
    }
}

fn candidate_parameter(
    file: &NamedClassSource,
    method: SymbolId,
    reason: &OverloadRejection,
) -> Option<SourceMethodParameter> {
    let SourceDefinition::Canonical { source, tree } = file.index().definition_of(method)? else {
        return None;
    };
    if source != file.source() {
        return None;
    }
    let TreeKind::DefDef(def) = &file.ast().try_get(tree)?.kind else {
        return None;
    };
    let [clause] = def.value_param_clauses.as_slice() else {
        return None;
    };
    if !def.type_params.is_empty() {
        return None;
    }
    let tree = match reason {
        OverloadRejection::ArgumentNonConformance { parameter_index, .. }
        | OverloadRejection::MissingRequiredArgument { parameter_index } => {
            *clause.get(*parameter_index)?
        }
        OverloadRejection::NamedArgument {
            reason: NamedArgumentRejection::Duplicate(name), ..
        } => {
            *clause.iter().find(|&&tree| {
                matches!(&file.ast().get(tree).kind, TreeKind::ValDef(p) if p.name.as_name() == name)
            })?
        }
        _ => return None,
    };
    let node = file.ast().try_get(tree)?;
    let TreeKind::ValDef(param) = &node.kind else {
        return None;
    };
    if param.metadata.modifiers != [crate::core::ast::Modifier::Param] {
        return None;
    }
    let parameter = file.index().symbol_at(source, tree)?;
    if file.index().definition_of(parameter) != Some(SourceDefinition::Canonical { source, tree }) {
        return None;
    }
    let name = file.named().syntax().member_names.parameter_name(tree)?;
    let declaration = node
        .position
        .filter(|p| p.source() == source)?
        .span()
        .range();
    if name.start() < declaration.start() || declaration.end() < name.end() {
        return None;
    }
    Some(SourceMethodParameter {
        method,
        parameter,
        target: SourceMemberTarget {
            source,
            declaration,
            name,
        },
    })
}

/// Written operand or label responsible for one candidate's rejection.
#[derive(Debug)]
pub struct RejectedCallArgument {
    /// Index in the caller's written argument list, before named-argument reordering.
    pub written: usize,
    /// Value for nonconformance, label for binding errors, or misplaced positional operand.
    pub location: SourceSpan,
    /// Semantic spelling of a written label without backticks, retained after analysis.
    pub label: Option<Box<str>>,
}

/// One candidate from the original overload error, paired with retained source evidence.
#[derive(Debug)]
pub struct ReportedOverloadCandidate<'a> {
    /// Candidate symbol in the originating naming session.
    pub method: SymbolId,
    /// Original rejection. Ambiguous alternatives are applicable and carry no rejection.
    pub reason: Option<&'a OverloadRejection>,
    /// Written source argument when this candidate identifies a specific operand or label.
    pub argument: Option<&'a RejectedCallArgument>,
    /// Provider declaration when available in the indexed revision.
    pub target: Option<OverloadCandidateTarget>,
}

#[derive(Debug)]
pub(crate) struct CandidateSites {
    pub argument: Option<RejectedCallArgument>,
    pub target: Option<OverloadCandidateTarget>,
}

pub(crate) fn candidates(
    error: &TyperError,
) -> impl Iterator<Item = (SymbolId, Option<&OverloadRejection>)> {
    let (rejected, ambiguous) = match error {
        TyperError::OverloadApplicationNoApplicable { candidates, .. } => {
            (candidates.as_slice(), &[][..])
        }
        TyperError::AmbiguousOverloadApplication { candidates, .. } => {
            (&[][..], candidates.as_slice())
        }
        _ => (&[][..], &[][..]),
    };
    rejected
        .iter()
        .map(|(method, reason)| (*method, Some(reason)))
        .chain(ambiguous.iter().map(|&method| (method, None)))
}

pub(crate) fn capture(
    sources: &NamedClassSources<'_>,
    error: &TyperError,
) -> Box<[CandidateSites]> {
    let (source, tree_index) = match *error {
        TyperError::OverloadApplicationNoApplicable {
            source, tree_index, ..
        }
        | TyperError::AmbiguousOverloadApplication {
            source, tree_index, ..
        } => (source, tree_index),
        _ => return Box::default(),
    };
    let caller = sources.file(source).and_then(|file| {
        let (_, tree) = file.ast().iter().nth(tree_index as usize)?;
        let TreeKind::Apply(call) = &tree.kind else {
            return None;
        };
        let site = tree.position.filter(|p| p.source() == source)?;
        let closing = site.span().range().end().checked_sub(1)?;
        let list = file.named().syntax().call_arguments.at(closing)?;
        (list.regular && list.callee.is_some() && list.arguments.len() == call.args.len())
            .then_some((file, call, list))
    });
    candidates(error)
        .map(|(method, reason)| {
            let argument =
                caller.and_then(|(file, call, list)| rejected_argument(file, call, list, reason?));
            CandidateSites {
                argument,
                target: sources.overload_candidate_target(method, reason),
            }
        })
        .collect()
}

fn rejected_argument(
    file: &NamedClassSource,
    call: &crate::core::ast::Apply<Untyped>,
    list: &CallArgumentList,
    reason: &OverloadRejection,
) -> Option<RejectedCallArgument> {
    let (written, label_only) = match reason {
        OverloadRejection::ArgumentNonConformance { argument_index, .. } => {
            (*argument_index, false)
        }
        OverloadRejection::NamedArgument { tree_index, reason } => (
            call.args.iter().position(|t| t.index() == *tree_index)?,
            !matches!(reason, NamedArgumentRejection::MisplacedPositional),
        ),
        _ => return None,
    };
    let arg = file.ast().try_get(*call.args.get(written)?)?;
    let span = arg.position.filter(|span| span.source() == file.source())?;
    if list.arguments.get(written) != Some(&span.span().range()) {
        return None;
    }
    let label = list.named_arguments.iter().find(|l| l.written == written);
    let range = match (label_only, label) {
        (true, label) => label?.label,
        (false, Some(label)) => label.value?,
        (false, None) => span.span().range(),
    };
    Some(RejectedCallArgument {
        written,
        location: SourceSpan::new(file.source(), Span::without_point(range)),
        label: label.map(|l| l.name.clone()),
    })
}
