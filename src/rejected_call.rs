//! Source evidence retained for rejected ordinary calls.
use crate::NamedClassSource;
use crate::core::SourceSpan;
use crate::parser::NamedCallArgument;

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

use crate::core::{SourceDefinition, SymbolId, SymbolKind, TreeKind};
use crate::typer::{NamedArgumentRejection, OverloadRejection};
use crate::{NamedClassSources, SourceMemberTarget, SourceMethodParameter};

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
        OverloadRejection::ArgumentNonConformance {parameter_index,..}
        | OverloadRejection::MissingRequiredArgument {parameter_index} => *clause.get(*parameter_index)?,
        OverloadRejection::NamedArgument {reason: NamedArgumentRejection::Duplicate(name),..} =>
            *clause.iter().find(|&&tree| matches!(&file.ast().get(tree).kind, TreeKind::ValDef(p) if p.name.as_name() == name))?,
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
