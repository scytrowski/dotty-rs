//! Canonical source parameters of class methods and primary constructors.
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

/// Two identities of a plain parameter in a single ordinary primary constructor
/// list. All symbols and ranges are meaningful only in the original session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceConstructorParameter {
    /// Class that owns the canonical source parameter.
    pub class: SymbolId,
    /// Primary constructor that owns the formal used for constructor execution.
    pub constructor: SymbolId,
    /// Class-owned source symbol. Use this ID with [`crate::ResolvedMemberIndex::references_to`].
    pub parameter: SymbolId,
    /// Derived constructor-owned formal for the same source declaration.
    pub formal: SymbolId,
    /// Complete declaration and exact written name in the defining file.
    pub target: SourceMemberTarget,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ParameterDeclaration {
    Method(SourceMethodParameter),
    Constructor(SourceConstructorParameter),
}

impl ParameterDeclaration {
    pub(crate) fn symbol(self) -> SymbolId {
        match self {
            Self::Method(p) => p.parameter,
            Self::Constructor(p) => p.parameter,
        }
    }
    pub(crate) fn target(self) -> SourceMemberTarget {
        match self {
            Self::Method(p) => p.target,
            Self::Constructor(p) => p.target,
        }
    }
}

pub(crate) fn declarations(
    file: &NamedClassSource,
    store: &SemanticStore,
    diagnostics: &mut Vec<DeclarationDiagnostic>,
) -> Vec<ParameterDeclaration> {
    let mut result = Vec::new();
    for class in file.classes() {
        if let Some(SourceDefinition::Canonical { source, tree }) =
            file.index().definition_of(class.constructor)
            && source == file.source()
            && let Err(error) = collect_list(
                file,
                Declaration {
                    symbol: class.constructor,
                    tree,
                },
                Some(class.symbol),
                store,
                &mut result,
            )
        {
            diagnostics.push(error.diagnostic(file.ast()));
        }
        for method in class.methods() {
            if let Err(error) = collect_list(file, method, None, store, &mut result) {
                diagnostics.push(error.diagnostic(file.ast()));
            }
        }
    }
    result.sort_unstable_by_key(|p| p.target().name.start());
    result
}

fn collect_list(
    file: &NamedClassSource,
    callable: Declaration,
    class: Option<SymbolId>,
    store: &SemanticStore,
    result: &mut Vec<ParameterDeclaration>,
) -> Result<(), DeclarationError> {
    let source = file.source();
    let invalid = |tree| DeclarationError::InvalidBinding { source, tree };
    let definition = file
        .ast()
        .try_get(callable.tree)
        .ok_or_else(|| invalid(callable.tree))?;
    let TreeKind::DefDef(def) = &definition.kind else {
        return Err(invalid(callable.tree));
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
        let supported = if class.is_some() {
            val.metadata.modifiers == [Modifier::ParamAccessor, Modifier::PrivateLocal]
                && val.rhs.is_none()
                && !matches!(
                    file.ast()
                        .try_get(val.tpt)
                        .ok_or_else(|| invalid(val.tpt))?
                        .kind,
                    TreeKind::ByNameTypeTree(_)
                        | TreeKind::PhaseSpecific(crate::core::ast::UntypedNode::PostfixOp(_))
                )
        } else {
            val.metadata.modifiers == [Modifier::Param]
        };
        if !supported {
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
            || symbol.owner != Some(class.unwrap_or(callable.symbol))
            || name.start() < declaration.start()
            || name.end() > declaration.end()
        {
            return Err(invalid(tree));
        }
        let target = SourceMemberTarget {
            source,
            name,
            declaration,
        };
        let parameter = if let Some(class) = class {
            let constructor = callable.symbol;
            let formal = file
                .index()
                .derived_symbol_at(constructor, source, tree)
                .ok_or_else(|| invalid(tree))?;
            if !store.symbols.contains(formal)
                || store.symbols.get(formal).kind != SymbolKind::Parameter
                || store.symbols.get(formal).owner != Some(constructor)
                || file.index().definition_of(formal)
                    != Some(SourceDefinition::Derived { source, tree })
            {
                return Err(invalid(tree));
            }
            ParameterDeclaration::Constructor(SourceConstructorParameter {
                class,
                constructor,
                parameter,
                formal,
                target,
            })
        } else {
            ParameterDeclaration::Method(SourceMethodParameter {
                parameter,
                method: callable.symbol,
                target,
            })
        };
        result.push(parameter);
    }
    Ok(())
}
