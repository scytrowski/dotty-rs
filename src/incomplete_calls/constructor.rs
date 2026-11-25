//! Declaration evidence for an unfinished primary constructor application.
use super::*;
use crate::{SourceConstructorParameter, SourceConstructorUse};

/// Signature of an eligible source primary constructor in an unfinished list.
/// IDs refer to one disposable naming revision. This candidate does not establish
/// a checked call or a valid class header.
#[derive(Debug)]
pub struct ProvisionalConstructorSignature<'a> {
    /// Whether the source allocates a class or initializes its direct parent.
    pub usage: SourceConstructorUse,
    /// Constructor owned by the class designated by `instance.symbol`.
    pub constructor: SymbolId,
    /// Named result type and canonical source class.
    pub instance: &'a SourceNamedType,
    /// Strict formals in declaration order.
    pub parameters: &'a [SourceSignatureParameter],
    /// Canonical class declarations, field roles and runtime formals in the same order.
    pub declarations: &'a [SourceConstructorParameter],
    /// Formal ordinal corresponding to the insertion position.
    pub active_parameter: usize,
}

#[derive(Debug)]
pub(super) struct ConstructorCandidate {
    usage: SourceConstructorUse,
    pub(super) prefix: PrefixMatch,
    constructor: SymbolId,
    instance: SourceNamedType,
    pub(super) parameters: Box<[SourceSignatureParameter]>,
    declarations: Box<[SourceConstructorParameter]>,
}

impl ConstructorCandidate {
    pub(super) fn at(&self, active_parameter: usize) -> ProvisionalConstructorSignature<'_> {
        ProvisionalConstructorSignature {
            usage: self.usage,
            constructor: self.constructor,
            instance: &self.instance,
            parameters: &self.parameters,
            declarations: &self.declarations,
            active_parameter,
        }
    }

    pub(super) fn capture(
        file: &NamedClassSource,
        list: &CallArgumentList,
        sources: &NamedClassSources<'_>,
        typer: &mut SourceTyper<'_>,
        context: ExpressionContext,
        usage: SourceConstructorUse,
    ) -> Option<Self> {
        let TreeKind::Apply(app) = &file.ast().get(list.application).kind else {
            return None;
        };
        let TreeKind::Select(select) = &file.ast().get(app.function).kind else {
            return None;
        };
        let qualifier = typer.type_expression(select.qualifier, context).ok()?;
        let candidates = typer
            .constructors_of(typer.typed_ast().get(qualifier).ty)
            .ok()?;
        let [candidate] = candidates.as_slice() else {
            return None;
        };
        let store = typer.store();
        let (provider, class) = sources.class(candidate.owner)?;
        if let SourceConstructorUse::DirectParent { child } = usage
            && (child == class.symbol
                || store
                    .symbols
                    .get(class.symbol)
                    .flags
                    .contains(crate::core::SymbolFlags::FINAL))
        {
            return None;
        }
        if class.constructor != candidate.symbol
            || store.symbols.get(candidate.symbol).visibility != crate::core::Visibility::Public
        {
            return None;
        }
        store.symbols.required_primary_clause(candidate.symbol)?;
        let SourceSymbolSignature::Method {
            parameters: Some(parameters),
            result: instance,
            ..
        } = SourceSymbolInfo::capture(store, candidate.symbol)?.signature
        else {
            return None;
        };
        let declarations =
            crate::source_parameters::constructor_declarations(provider, class, store)?;
        if parameters.is_empty()
            || parameters.len() != declarations.len()
            || instance.symbol != class.symbol
        {
            return None;
        }
        let written = written_types(typer, context, file, list, &app.args);
        let prefix = PrefixMatch::new(list, &parameters, &written, typer);
        Some(Self {
            prefix,
            usage,
            constructor: candidate.symbol,
            instance,
            parameters,
            declarations,
        })
    }
}
