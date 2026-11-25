//! Declaration identities and written tokens at a committed ordinary call.

use crate::core::{
    MethodKind, NamedCallBinding, SemanticStore, SymbolId, SymbolInfo, TextRange, Type,
};
use crate::parser::CallArgumentList;
use crate::{ResolvedSourceCall, SourceMethodParameter};

/// One explicitly supplied argument of a checked source call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedWrittenArgument {
    /// Canonical parameter of the selected overload and its declaration ranges.
    pub parameter: SourceMethodParameter,
    /// Zero-based position in the method's single value parameter clause.
    pub ordinal: usize,
    /// Entire argument, including the label and equals sign when present.
    /// UTF-8 byte offsets are relative to the file containing the call.
    pub range: TextRange,
    /// Exact written label token including backticks, absent for a positional argument.
    pub label: Option<TextRange>,
}

/// An omitted formal supplied by a getter in a checked call. It has no
/// written argument or label range in the calling document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedDefaultArgument {
    /// Source parameter belonging to the selected overload.
    pub parameter: SourceMethodParameter,
    /// Zero-based position in the declaration's single ordinary clause.
    pub ordinal: usize,
    /// Canonical symbol of the getter selected by the typer in this session.
    pub getter: SymbolId,
}

/// Arguments of a direct, successfully checked source call in one revision.
/// All symbols belong to the originating session. Source ranges remain usable
/// after the semantic arenas are released, with the corresponding source texts.
#[derive(Debug)]
pub struct ResolvedCallArguments {
    /// The selected overload and its written callee token.
    pub call: ResolvedSourceCall,
    /// Explicit arguments in source evaluation order.
    pub written: Box<[ResolvedWrittenArgument]>,
    /// Omitted arguments in declaration order, with the exact getters used.
    pub defaulted: Box<[ResolvedDefaultArgument]>,
}

impl ResolvedCallArguments {
    pub(crate) fn capture(
        call: ResolvedSourceCall,
        list: &CallArgumentList,
        formals: &[SourceMethodParameter],
        binding: Option<&NamedCallBinding>,
        store: &SemanticStore,
    ) -> Option<Self> {
        let SymbolInfo::Complete(ty) = *store.symbols.info(call.method) else {
            return None;
        };
        let Type::Method(method) = store.types.try_get(ty)? else {
            return None;
        };
        let omitted = binding.map_or(&[][..], |b| &*b.defaulted);
        if !list.regular
            || list.callee != Some(call.origin.span().range())
            || method.kind != MethodKind::Plain
            || method.params.len() != formals.len()
            || matches!(
                store.types.get(method.result),
                Type::Method(_) | Type::Poly(_)
            )
            || method.params.iter().any(|p| {
                p.erased || p.varargs || matches!(store.types.get(p.ty), Type::ByName { .. })
            })
            || list.arguments.len() + omitted.len() != formals.len()
        {
            return None;
        }
        let mut labels = list.named_arguments.iter().peekable();
        let mut written = Vec::with_capacity(list.arguments.len());
        for (slot, &range) in list.arguments.iter().enumerate() {
            let ordinal = match binding {
                Some(b) => *b.written_to_formal.get(slot)?,
                None if list.positional => slot,
                None => return None,
            };
            let label = labels.next_if(|l| l.written == slot).map(|l| l.label);
            written.push(ResolvedWrittenArgument {
                parameter: *formals.get(ordinal)?,
                ordinal,
                range,
                label,
            });
        }
        let defaulted = omitted
            .iter()
            .map(|d| {
                Some(ResolvedDefaultArgument {
                    parameter: *formals.get(d.formal)?,
                    ordinal: d.formal,
                    getter: d.getter,
                })
            })
            .collect::<Option<_>>()?;
        Some(Self {
            call,
            written: written.into_boxed_slice(),
            defaulted,
        })
    }
}
