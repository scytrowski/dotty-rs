//! Canonical declarations of named parameters on source class methods.
use crate::core::ast::Modifier;
use crate::core::{SemanticStore, SourceDefinition, SymbolId, SymbolKind, TreeKind};
use crate::source_declarations::{Declaration, DeclarationDiagnostic, DeclarationError};
use crate::{NamedClassSource, SourceMemberTarget};

/// One named value parameter of an ordinary class method with a single value
/// parameter list. IDs and UTF-8 ranges belong to the originating revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceMethodParameter {
    /// Canonical parameter symbol in the original semantic store.
    pub parameter: SymbolId,
    /// Method symbol owning this formal, distinguishing same-named overloads.
    pub method: SymbolId,
    /// Complete parameter declaration and the written name within it.
    pub target: SourceMemberTarget,
}

pub(crate) fn declarations(
    file: &NamedClassSource,
    store: &SemanticStore,
    diagnostics: &mut Vec<DeclarationDiagnostic>,
) -> Vec<SourceMethodParameter> {
    let mut result = Vec::new();
    for class in file.classes() {
        for method in class.methods() {
            if let Err(error) = collect_method(file, method, store, &mut result) {
                diagnostics.push(error.diagnostic(file.ast()));
            }
        }
    }
    result.sort_unstable_by_key(|p| p.target.name.start());
    result
}

fn collect_method(
    file: &NamedClassSource,
    method: Declaration,
    store: &SemanticStore,
    result: &mut Vec<SourceMethodParameter>,
) -> Result<(), DeclarationError> {
    let source = file.source();
    let invalid = |tree| DeclarationError::InvalidBinding { source, tree };
    let method_node = file
        .ast()
        .try_get(method.tree)
        .ok_or_else(|| invalid(method.tree))?;
    let TreeKind::DefDef(def) = &method_node.kind else {
        return Err(invalid(method.tree));
    };
    let [clause] = def.value_param_clauses.as_slice() else {
        return Ok(());
    };
    let names = &file.named().syntax().member_names;
    for &tree in clause {
        let Some(name) = names.parameter_name(tree) else {
            continue;
        };
        let node = file.ast().try_get(tree).ok_or_else(|| invalid(tree))?;
        let TreeKind::ValDef(val) = &node.kind else {
            return Err(invalid(tree));
        };
        if val.metadata.modifiers != [Modifier::Param] {
            continue;
        }
        let parameter = file
            .index()
            .symbol_at(source, tree)
            .ok_or_else(|| invalid(tree))?;
        if file.index().definition_of(parameter)
            != Some(SourceDefinition::Canonical { source, tree })
            || !store.symbols.contains(parameter)
        {
            return Err(invalid(tree));
        }
        let symbol = store.symbols.get(parameter);
        let declaration = node
            .position
            .filter(|pos| pos.source() == source)
            .ok_or_else(|| invalid(tree))?
            .span()
            .range();
        if symbol.kind != SymbolKind::Parameter
            || symbol.owner != Some(method.symbol)
            || name.start() < declaration.start()
            || name.end() > declaration.end()
        {
            return Err(invalid(tree));
        }
        result.push(SourceMethodParameter {
            parameter,
            method: method.symbol,
            target: SourceMemberTarget {
                source,
                name,
                declaration,
            },
        });
    }
    Ok(())
}
