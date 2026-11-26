//! Source-owned default getters and typed temporaries shared with their enclosing calls.

use super::*;
use dotty_core::types::MethodKind;
use dotty_core::{SourceDefinition, SourceSemanticIndex, SourceTypedIndex, SymbolOrigin};

impl JvmMethodPlan {
    /// Plans the registered parameter defaults of this source method. The caller
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
                    self.locals
                        .push(JvmLocalBinding::ordinary(tree, symbol, mutable));
                }
            }
            false
        });
        self.locals
            .sort_by_key(|binding| binding.declaration.index());
    }
}

/// A getter for a first-clause default is selected as a value with no Apply node.
pub(super) fn is_parameterless_getter(store: &SemanticStore, tree: &Tree<Typed>) -> bool {
    if !matches!(tree.kind, TreeKind::Ident(_) | TreeKind::Select(_)) {
        return false;
    }
    let Some(Type::TermRef {
        target: dotty_core::types::TermRefTarget::Symbol(symbol),
        ..
    }) = store.types.try_get(tree.ty)
    else {
        return false;
    };
    if store.default_getter_origin(*symbol).is_none() || !store.symbols.contains(*symbol) {
        return false;
    }
    let SymbolInfo::Complete(ty) = *store.symbols.info(*symbol) else {
        return false;
    };
    store
        .types
        .try_get(ty)
        .is_some_and(|ty| !matches!(ty, Type::Method(_) | Type::Poly(_)))
}

/// Verify the same-source getter registered for this parameter. The generated spelling is only its ABI name.
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
    let copies = if outer.kind == MethodKind::Plain && ordinal >= outer.params.len() {
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
        {
            return Err(invalid());
        }
        &outer.params[..]
    } else {
        // A getter with no preceding clause has the source parameter's result
        // type directly. An explicit empty () clause is a different signature.
        if !outer
            .params
            .get(ordinal)
            .is_some_and(|p| p.ty == getter_info)
            || matches!(
                store.types.try_get(getter_info),
                Some(Type::Method(_) | Type::Poly(_))
            )
        {
            return Err(invalid());
        }
        &[][..]
    };
    if copies.len() != plan.parameters.len() {
        return Err(invalid());
    }
    for (f, &id) in copies.iter().zip(&plan.parameters) {
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

/// Retains the identities checked against the source and the literal value,
/// so that a plan cannot substitute another local declaration or initializer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LiteralGiven {
    symbol: SymbolId,
    method: SymbolId,
    declaration: TreeId<Typed>,
    initializer: TreeId<Typed>,
    int: TypeId,
    value: i32,
}

impl LiteralGiven {
    pub(super) fn matches(
        self,
        binding: &JvmLocalBinding,
        method: SymbolId,
        ast: &TypedAst,
        store: &SemanticStore,
    ) -> bool {
        let Some(tree) = ast.try_get(self.declaration) else {
            return false;
        };
        let TreeKind::ValDef(def) = &tree.kind else {
            return false;
        };
        if self.symbol != binding.symbol || !store.symbols.contains(self.symbol) {
            return false;
        }
        let local = store.symbols.get(self.symbol);
        self.method == method && self.declaration == binding.declaration
            && !binding.mutable && local.owner == Some(method)
            && local.kind == SymbolKind::Local
            && local.flags == SymbolFlags::GIVEN | SymbolFlags::FINAL | SymbolFlags::LAZY
            && def.name.as_name() == &local.name
            && tree.position.is_some_and(|pos| local.origin == SymbolOrigin::Source(pos.source()))
            && tree.position == local.position
            && local.info == SymbolInfo::Complete(self.int) && tree.ty == self.int
            && def.rhs == Some(self.initializer)
            && ast.try_get(self.initializer).is_some_and(|node|
                matches!(&node.kind, TreeKind::Literal(l) if l.value == dotty_core::Constant::Int(self.value))
                && store.types.try_get(node.ty) == Some(&Type::Constant(dotty_core::Constant::Int(self.value))))
    }
}

impl JvmLocalBinding {
    /// Checks a source local alias `given name: Int = <literal>`. Its lazy
    /// initializer is a constant, so eager storage cannot observe an effect.
    /// Supply the symbol from the typer's local index for this declaration.
    #[allow(clippy::too_many_arguments)]
    pub fn literal_given(
        method: SymbolId,
        symbol: SymbolId,
        source: SourceId,
        declaration: TreeId<Untyped>,
        source_ast: &AstArena<Untyped>,
        typed_ast: &TypedAst,
        bindings: &SourceTypedIndex,
        store: &SemanticStore,
        definitions: &Definitions,
    ) -> Result<Self, JvmCodegenError> {
        use dotty_core::ast::Modifier;
        let invalid = || JvmCodegenError::InvalidLiteralGiven { symbol };
        let node = source_ast.try_get(declaration).ok_or_else(invalid)?;
        let TreeKind::ValDef(def) = &node.kind else {
            return Err(invalid());
        };
        if def.metadata.modifiers != [Modifier::Given, Modifier::Final, Modifier::Lazy]
            || def.metadata.visibility.is_some()
            || !def.metadata.annotations.is_empty()
            || !store.symbols.contains(symbol)
            || store.symbols.get(symbol).origin != SymbolOrigin::Source(source)
            || def.name.as_name() != &store.symbols.get(symbol).name
            || !node.position.is_some_and(|pos| pos.source() == source)
            || node.position != store.symbols.get(symbol).position
        {
            return Err(invalid());
        }
        let rhs = def.rhs.ok_or_else(invalid)?;
        let source_rhs = source_ast.try_get(rhs).ok_or_else(invalid)?;
        let value = match &source_rhs.kind {
            TreeKind::Literal(literal) => match literal.value {
                dotty_core::Constant::Int(value) => value,
                _ => return Err(invalid()),
            },
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::Number(number)) => {
                let spelling = store.names.try_resolve(number.text).ok_or_else(invalid)?;
                let digits = spelling.replace('_', "");
                match number.kind {
                    dotty_core::ast::NumberKind::Whole(10) => digits.parse::<i32>().ok(),
                    dotty_core::ast::NumberKind::Whole(radix @ (2 | 16)) => {
                        let (lower, upper) = if radix == 16 {
                            ("0x", "0X")
                        } else {
                            ("0b", "0B")
                        };
                        digits
                            .strip_prefix(lower)
                            .or_else(|| digits.strip_prefix(upper))
                            .and_then(|s| u32::from_str_radix(s, radix).ok())
                            .map(|bits| bits as i32)
                    }
                    _ => None,
                }
                .ok_or_else(invalid)?
            }
            _ => return Err(invalid()),
        };
        let typed = bindings.get(source, declaration).ok_or_else(invalid)?;
        let initializer = bindings.get(source, rhs).ok_or_else(invalid)?;
        let Some(TreeKind::ValDef(typed_def)) = typed_ast.try_get(typed).map(|t| &t.kind) else {
            return Err(invalid());
        };
        if bindings.get(source, def.tpt) != Some(typed_def.tpt)
            || !typed_ast
                .try_get(typed_def.tpt)
                .is_some_and(|t| t.ty == definitions.int)
            || !typed_ast
                .try_get(initializer)
                .is_some_and(|node| node.position.is_some() && node.position == source_rhs.position)
        {
            return Err(invalid());
        }
        let proof = LiteralGiven {
            method,
            symbol,
            declaration: typed,
            initializer,
            int: definitions.int,
            value,
        };
        let binding = Self {
            declaration: typed,
            symbol,
            mutable: false,
            pattern: None,
            literal_given: Some(proof),
        };
        if !proof.matches(&binding, method, typed_ast, store) {
            return Err(invalid());
        }
        Ok(binding)
    }
}
