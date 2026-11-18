//! Source-owned default getters and typed temporaries shared with their enclosing calls.

use super::*;
use dotty_core::types::MethodKind;
use dotty_core::{SourceDefinition, SourceSemanticIndex, SourceTypedIndex, SymbolOrigin};

impl JvmMethodPlan {
    /// Plans the registered contextual defaults of this source method. The caller
    /// must first type the full method definition, including parameter defaults.
    pub fn source_default_getters(
        &self,
        source: SourceId,
        source_ast: &AstArena<Untyped>,
        index: &SourceSemanticIndex,
        typed_index: &SourceTypedIndex,
        store: &SemanticStore,
    ) -> Result<Vec<Self>, JvmCodegenError> {
        let invalid = || JvmCodegenError::InvalidContextualDefaultGetter {
            method: self.method,
        };
        let Some(SourceDefinition::Canonical {
            source: defining_source,
            tree,
        }) = index.definition_of(self.method)
        else {
            return Err(invalid());
        };
        if defining_source != source
            || index.symbol_at(source, tree) != Some(self.method)
            || !store.symbols.contains(self.method)
        {
            return Err(invalid());
        }
        let Some(TreeKind::DefDef(method)) = source_ast.try_get(tree).map(|t| &t.kind) else {
            return Err(invalid());
        };
        let owner = store.symbols.owner(self.method).ok_or_else(invalid)?;
        let mut result = Vec::new();
        let mut ordinal = 0;
        for (clause_index, clause) in method.value_param_clauses.iter().enumerate() {
            for &param in clause {
                let getter = store.default_getter(self.method, ordinal);
                ordinal += 1;
                let Some(getter) = getter else {
                    continue;
                };
                let parameter = index.symbol_at(source, param).ok_or_else(invalid)?;
                if index.derived_symbol_at(owner, source, param) != Some(getter)
                    || index.definition_of(getter)
                        != Some(SourceDefinition::Derived {
                            source,
                            tree: param,
                        })
                    || self.parameters.get(ordinal - 1) != Some(&parameter)
                {
                    return Err(invalid());
                }
                let Some(TreeKind::ValDef(declaration)) =
                    source_ast.try_get(param).map(|t| &t.kind)
                else {
                    return Err(invalid());
                };
                let body = declaration
                    .rhs
                    .and_then(|rhs| typed_index.get(source, rhs))
                    .ok_or_else(invalid)?;
                let parameters = method.value_param_clauses[..clause_index]
                    .iter()
                    .flatten()
                    .map(|&original| {
                        index
                            .derived_symbol_at(getter, source, original)
                            .ok_or_else(invalid)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                result.push(Self {
                    method: getter,
                    parameters,
                    locals: Vec::new(),
                    body: Some(body),
                });
            }
        }
        Ok(result)
    }

    /// Adds exact compiler-generated local declarations reachable from the body.
    /// Source and typed trees must belong to the same session as the plan.
    pub fn bind_generated_locals(
        &mut self,
        ast: &TypedAst,
        bindings: &SourceTypedIndex,
        store: &SemanticStore,
    ) {
        let Some(root) = self.body else {
            return;
        };
        let mut seen = self
            .locals
            .iter()
            .map(|b| (b.declaration, b.symbol, b.mutable))
            .collect::<HashSet<_>>();
        any_typed_tree(ast, root, |tree| {
            if let Some(symbol) = bindings.synthetic_definition_symbol(tree)
                && store.symbols.contains(symbol)
                && matches!(
                    ast.try_get(tree).map(|t| &t.kind),
                    Some(TreeKind::ValDef(_))
                )
            {
                let entry = store.symbols.get(symbol);
                let mutable = entry.flags.contains(SymbolFlags::MUTABLE);
                if entry.kind == SymbolKind::Local
                    && entry.owner == Some(self.method)
                    && seen.insert((tree, symbol, mutable))
                {
                    self.locals.push(JvmLocalBinding {
                        declaration: tree,
                        symbol,
                        mutable,
                    });
                }
            }
            false
        });
        self.locals
            .sort_by_key(|binding| binding.declaration.index());
    }
}

/// Verify the same-source getter registered for this ordinary clause followed
/// by a contextual clause. The generated spelling is only its ABI name.
pub(super) fn getter_name(
    store: &SemanticStore,
    plan: &JvmMethodPlan,
) -> Result<Option<String>, JvmCodegenError> {
    let Some((method, ordinal)) = store.default_getter_origin(plan.method) else {
        return Ok(None);
    };
    let invalid = || JvmCodegenError::InvalidContextualDefaultGetter {
        method: plan.method,
    };
    if !store.symbols.contains(method) || !store.symbols.contains(plan.method) {
        return Err(invalid());
    }
    let target = store.symbols.get(method);
    let getter = store.symbols.get(plan.method);
    if getter.kind != SymbolKind::Method
        || target.kind != SymbolKind::Method
        || getter.flags != SymbolFlags::SYNTHETIC
        || getter.origin != target.origin
        || !matches!(getter.origin, SymbolOrigin::Source(_))
        || getter.owner != target.owner
        || getter.visibility != target.visibility
    {
        return Err(invalid());
    }
    let SymbolInfo::Complete(target_info) = target.info else {
        return Err(invalid());
    };
    let SymbolInfo::Complete(getter_info) = getter.info else {
        return Err(invalid());
    };
    let Some(Type::Method(outer)) = store.types.try_get(target_info) else {
        return Err(invalid());
    };
    let tail = crate::descriptor::contextual_tail(store, outer).ok_or_else(invalid)?;
    let defaulted = ordinal
        .checked_sub(outer.params.len())
        .and_then(|position| tail.params.get(position))
        .ok_or_else(invalid)?;
    let Some(Type::Method(signature)) = store.types.try_get(getter_info) else {
        return Err(invalid());
    };
    if signature.kind != MethodKind::Plain
        || signature.params != outer.params
        || signature.result != defaulted.ty
        || signature.params.len() != plan.parameters.len()
    {
        return Err(invalid());
    }
    for (f, &id) in signature.params.iter().zip(&plan.parameters) {
        if !store.symbols.contains(id) {
            return Err(invalid());
        }
        let copy = store.symbols.get(id);
        if copy.name != *f.name.as_name()
            || copy.owner != Some(plan.method)
            || copy.kind != SymbolKind::Parameter
            || copy.info != SymbolInfo::Complete(f.ty)
        {
            return Err(invalid());
        }
    }
    let name = store
        .names
        .try_resolve(target.name.text())
        .and_then(crate::identity::jvm_method_name)
        .ok_or_else(invalid)?;
    let suffix = ordinal.checked_add(1).ok_or_else(invalid)?;
    let expected = format!("{name}$default${suffix}");
    if store.names.try_resolve(getter.name.text()) != Some(expected.as_str()) {
        return Err(invalid());
    }
    Ok(Some(expected))
}
