//! Proof receipt and validation for the existing one-call immutable-local Counter.set shape.
//!
//! This module extracts the original read-only proof without widening the accepted source shape.

use super::*;

/// Opaque read-only proof of the original non-final `first.set(secondSeed)`
/// statement following two initialized immutable Counter locals and preceding
/// their final Pair construction. This receipt does not authorize emission.
pub struct ValidatedImmutableLocalCounterSetCallProof<'session> {
    initializer_pair: ValidatedImmutableLocalInitializerPairProof<'session>,
    source_call: TreeId<Untyped>,
    typed_call: TreeId<Typed>,
    source_select: TreeId<Untyped>,
    typed_select: TreeId<Typed>,
    source_receiver: TreeId<Untyped>,
    typed_receiver: TreeId<Typed>,
    source_argument: TreeId<Untyped>,
    typed_argument: TreeId<Typed>,
    method: SymbolId,
    receiver: SymbolId,
    formal: SymbolId,
    argument_type: TypeId,
    result_type: TypeId,
}

impl<'session> ValidatedImmutableLocalCounterSetCallProof<'session> {
    pub const fn initializer_pair(&self) -> &ValidatedImmutableLocalInitializerPairProof<'session> {
        &self.initializer_pair
    }
    pub const fn source_call(&self) -> TreeId<Untyped> {
        self.source_call
    }
    pub const fn typed_call(&self) -> TreeId<Typed> {
        self.typed_call
    }
    pub const fn source_select(&self) -> TreeId<Untyped> {
        self.source_select
    }
    pub const fn typed_select(&self) -> TreeId<Typed> {
        self.typed_select
    }
    pub const fn source_receiver(&self) -> TreeId<Untyped> {
        self.source_receiver
    }
    pub const fn typed_receiver(&self) -> TreeId<Typed> {
        self.typed_receiver
    }
    pub const fn source_argument(&self) -> TreeId<Untyped> {
        self.source_argument
    }
    pub const fn typed_argument(&self) -> TreeId<Typed> {
        self.typed_argument
    }
    pub const fn method(&self) -> SymbolId {
        self.method
    }
    pub const fn receiver(&self) -> SymbolId {
        self.receiver
    }
    pub const fn formal(&self) -> SymbolId {
        self.formal
    }
    pub const fn argument_type(&self) -> TypeId {
        self.argument_type
    }
    pub const fn result_type(&self) -> TypeId {
        self.result_type
    }
}

/// Validate the single original non-final `first.set(secondSeed)` statement
/// in the three-statement PairFactory envelope. Existing initializer-pair
/// callers remain restricted to their original two-statement shape.
pub fn validate_immutable_local_counter_set_call_proof<'session>(
    input: ImmutableLocalInitializerPairProofInput<'session>,
) -> Result<ValidatedImmutableLocalCounterSetCallProof<'session>, CrossClassConstructorCallProofError>
{
    let pair = validate_immutable_local_initializer_pair_proof_inner(input, true)?;
    let source_error = |s| CrossClassConstructorCallProofError::SourceShape(s);
    let identity_error = |s| CrossClassConstructorCallProofError::SourceIdentity(s);
    let semantic_error = |s| CrossClassConstructorCallProofError::SemanticIdentity(s);
    let typed_error = |s| CrossClassConstructorCallProofError::TypedBinding(s);
    let source_id = pair.source_id;
    let source_ast = pair._source_ast;
    let typed_ast = pair._typed_ast;
    let source_index = pair._source_index;
    let source_typed_index = pair._source_typed_index;
    let store = pair._store;
    let definitions = pair._definitions;
    let TreeKind::Block(block) = &source_ast
        .try_get(pair.source_block)
        .ok_or_else(|| source_error("original local Block is stale"))?
        .kind
    else {
        return Err(source_error("original local Block must remain a Block"));
    };
    let source_call = block.stats[2];
    let TreeKind::Apply(source_apply) = &source_ast
        .try_get(source_call)
        .ok_or_else(|| source_error("Counter.set statement is stale"))?
        .kind
    else {
        return Err(source_error(
            "third Block statistic must be the Counter.set Apply",
        ));
    };
    if source_apply.kind != ApplyKind::Regular || source_apply.args.len() != 1 {
        return Err(source_error(
            "Counter.set statement must be one regular one-argument Apply",
        ));
    }
    let source_select = source_apply.function;
    let TreeKind::Select(source_selection) = &source_ast
        .try_get(source_select)
        .ok_or_else(|| source_error("Counter.set Select is stale"))?
        .kind
    else {
        return Err(source_error("Counter.set Apply function must be Select"));
    };
    let source_receiver = source_selection.qualifier;
    let TreeKind::Ident(source_receiver_ident) = &source_ast
        .try_get(source_receiver)
        .ok_or_else(|| source_error("Counter.set receiver is stale"))?
        .kind
    else {
        return Err(source_error(
            "Counter.set receiver must be one of the initialized local Idents",
        ));
    };
    let source_argument = source_apply.args[0];
    let TreeKind::Ident(source_argument_ident) = &source_ast
        .try_get(source_argument)
        .ok_or_else(|| source_error("Counter.set argument is stale"))?
        .kind
    else {
        return Err(source_error(
            "Counter.set argument must be the secondSeed Ident",
        ));
    };
    let [first, second] = pair.initializers;
    let factory_method_tree = match source_index.definition_of(pair.method) {
        Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
        _ => {
            return Err(identity_error(
                "factory method is not the original source method",
            ));
        }
    };
    let TreeKind::DefDef(factory_method_def) = &source_ast
        .try_get(factory_method_tree)
        .ok_or_else(|| identity_error("factory method source tree is stale"))?
        .kind
    else {
        return Err(identity_error("factory method source root must be DefDef"));
    };
    let call_formal_tree = *factory_method_def.value_param_clauses[0]
        .get(1)
        .ok_or_else(|| identity_error("factory method must retain secondSeed formal"))?;
    let call_formal = source_index
        .symbol_at(source_id, call_formal_tree)
        .ok_or_else(|| identity_error("secondSeed has no original formal identity"))?;
    if source_index.definition_of(call_formal)
        != Some(SourceDefinition::Canonical {
            source: source_id,
            tree: call_formal_tree,
        })
        || !has_source_origin(store, call_formal, source_id)
    {
        return Err(identity_error(
            "secondSeed must retain its canonical original source identity",
        ));
    }
    let TreeKind::ValDef(call_formal_def) = &source_ast
        .try_get(call_formal_tree)
        .ok_or_else(|| identity_error("secondSeed formal tree is stale"))?
        .kind
    else {
        return Err(identity_error("secondSeed source formal must be a ValDef"));
    };
    if !store.symbols.contains(call_formal)
        || store.symbols.get(call_formal).owner != Some(pair.method)
        || store.symbols.get(call_formal).kind != SymbolKind::Parameter
        || !matches!(store.symbols.get(call_formal).info, SymbolInfo::Complete(ty) if ty == definitions.int)
    {
        return Err(identity_error("secondSeed must be a canonical Int formal"));
    }
    if source_argument_ident.name != *call_formal_def.name.as_name() {
        return Err(source_error("Counter.set argument must be secondSeed"));
    }
    let typed_call = source_typed_index
        .get(source_id, source_call)
        .ok_or_else(|| typed_error("Counter.set statement has no typed mapping"))?;
    let typed_select = source_typed_index
        .get(source_id, source_select)
        .ok_or_else(|| typed_error("Counter.set Select has no typed mapping"))?;
    let typed_receiver = source_typed_index
        .get(source_id, source_receiver)
        .ok_or_else(|| typed_error("Counter.set receiver has no typed mapping"))?;
    let typed_argument = source_typed_index
        .get(source_id, source_argument)
        .ok_or_else(|| typed_error("Counter.set argument has no typed mapping"))?;
    let TreeKind::Block(typed_block) = &typed_ast
        .try_get(pair.typed_block)
        .ok_or_else(|| typed_error("typed original Block is stale"))?
        .kind
    else {
        return Err(typed_error("original Block must remain a typed Block"));
    };
    if typed_block.stats.len() != 3 || typed_block.stats[2] != typed_call {
        return Err(typed_error(
            "typed Block must preserve exactly one original Counter.set stat after both locals",
        ));
    }
    // The initializer-pair receipt intentionally does not authenticate the
    // final Pair argument origins. This call receipt includes that envelope,
    // so bind both source and typed arguments to the two locals in order.
    let TreeKind::Apply(source_final_apply) = &source_ast
        .try_get(pair.source_final_expression)
        .ok_or_else(|| source_error("final Pair expression is stale"))?
        .kind
    else {
        return Err(source_error("final Pair expression must remain Apply"));
    };
    let TreeKind::Apply(typed_final_apply) = &typed_ast
        .try_get(pair.typed_final_expression)
        .ok_or_else(|| typed_error("typed final Pair expression is stale"))?
        .kind
    else {
        return Err(typed_error("final Pair expression must remain typed Apply"));
    };
    if source_final_apply.args.len() != 2
        || typed_final_apply.kind != ApplyKind::Regular
        || typed_final_apply.args.len() != 2
    {
        return Err(source_error(
            "final Pair call must retain two ordered arguments",
        ));
    }
    let mut seen_final_locals = [false; 2];
    for (index, source_arg) in source_final_apply.args.iter().copied().enumerate() {
        let TreeKind::Ident(source_ident) = &source_ast
            .try_get(source_arg)
            .ok_or_else(|| source_error("final Pair argument is stale"))?
            .kind
        else {
            return Err(source_error(
                "final Pair arguments must be the original local Idents",
            ));
        };
        let local_index = pair
            .initializers
            .iter()
            .position(|initializer| {
                source_ast
                    .try_get(initializer.declaration.declaration)
                    .is_some_and(|node| {
                        matches!(&node.kind, TreeKind::ValDef(local_def) if source_ident.name == *local_def.name.as_name())
                    })
            })
            .ok_or_else(|| {
                source_error("final Pair arguments must name the two initialized locals")
            })?;
        if std::mem::replace(&mut seen_final_locals[local_index], true) {
            return Err(source_error(
                "final Pair call must use each initialized local once",
            ));
        }
        let initializer = &pair.initializers[local_index];
        let expected_typed_arg = source_typed_index
            .get(source_id, source_arg)
            .ok_or_else(|| typed_error("final Pair argument has no original typed mapping"))?;
        let TreeKind::Ident(typed_ident) = &typed_ast
            .try_get(expected_typed_arg)
            .ok_or_else(|| typed_error("typed final Pair argument is stale"))?
            .kind
        else {
            return Err(typed_error(
                "typed final Pair argument must remain the original local Ident",
            ));
        };
        if store.names.resolve(typed_ident.name.text())
            != store.names.resolve(source_ident.name.text())
        {
            return Err(typed_error(
                "typed final Pair Ident must preserve its original local name",
            ));
        }
        if typed_final_apply.args[index] != expected_typed_arg
            || !matches!(store.types.try_get(typed_ast
                .try_get(expected_typed_arg)
                .ok_or_else(|| typed_error("typed final Pair argument is stale"))?.ty),
                Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. })
                    if *symbol == initializer.declaration.symbol)
        {
            return Err(typed_error(
                "typed final Pair arguments must retain the ordered local TermRefs",
            ));
        }
        require_matching_source_typed_positions(
            source_ast,
            typed_ast,
            source_id,
            source_arg,
            expected_typed_arg,
            "final Pair local argument",
        )?;
    }
    if seen_final_locals != [true, true] {
        return Err(source_error(
            "final Pair call must use both initialized locals",
        ));
    }
    let TreeKind::Apply(typed_apply) = &typed_ast
        .try_get(typed_call)
        .ok_or_else(|| typed_error("typed Counter.set Apply is stale"))?
        .kind
    else {
        return Err(typed_error("source Counter.set must remain a typed Apply"));
    };
    if typed_apply.kind != ApplyKind::Regular
        || typed_apply.args != [typed_argument]
        || typed_apply.function != typed_select
    {
        return Err(typed_error(
            "typed Counter.set Apply must preserve its exact Select and argument edges",
        ));
    }
    let TreeKind::Select(typed_selection) = &typed_ast
        .try_get(typed_select)
        .ok_or_else(|| typed_error("typed Counter.set Select is stale"))?
        .kind
    else {
        return Err(typed_error("typed Counter.set function must remain Select"));
    };
    if typed_selection.qualifier != typed_receiver {
        return Err(typed_error(
            "typed Counter.set must preserve its receiver edge",
        ));
    }
    let TreeKind::Ident(typed_receiver_ident) = &typed_ast
        .try_get(typed_receiver)
        .ok_or_else(|| typed_error("typed Counter.set receiver is stale"))?
        .kind
    else {
        return Err(typed_error(
            "typed Counter.set receiver must remain the original local Ident",
        ));
    };
    if store.names.resolve(typed_receiver_ident.name.text())
        != store.names.resolve(source_receiver_ident.name.text())
    {
        return Err(typed_error(
            "typed Counter.set receiver Ident must preserve its original local name",
        ));
    }
    let receiver = match store.types.try_get(
        typed_ast
            .try_get(typed_receiver)
            .ok_or_else(|| typed_error("typed Counter.set receiver is stale"))?
            .ty,
    ) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) => *symbol,
        _ => {
            return Err(typed_error(
                "typed Counter.set receiver must retain its TermRef",
            ));
        }
    };
    if receiver != first.declaration.symbol && receiver != second.declaration.symbol {
        return Err(typed_error(
            "typed Counter.set receiver must target one of the two initialized local declarations",
        ));
    }
    let receiver_local_tree = if receiver == first.declaration.symbol {
        first.declaration.declaration
    } else {
        second.declaration.declaration
    };
    let TreeKind::ValDef(receiver_local) = &source_ast
        .try_get(receiver_local_tree)
        .ok_or_else(|| identity_error("Counter.set receiver local is stale"))?
        .kind
    else {
        return Err(identity_error(
            "Counter.set receiver receipt must name a ValDef",
        ));
    };
    if source_receiver_ident.name != *receiver_local.name.as_name() {
        return Err(typed_error(
            "typed Counter.set receiver does not match the original source local Ident",
        ));
    }
    let method = match store.types.try_get(
        typed_ast
            .try_get(typed_select)
            .ok_or_else(|| typed_error("typed Counter.set Select is stale"))?
            .ty,
    ) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) => *symbol,
        _ => {
            return Err(typed_error(
                "typed Counter.set Select must retain its method TermRef",
            ));
        }
    };
    if !store.symbols.contains(method)
        || store.symbols.get(method).kind != SymbolKind::Method
        || store.symbols.get(method).owner != Some(pair.counter)
        || store.symbols.get(method).visibility != dotty_core::Visibility::Public
        || !has_source_origin(store, method, source_id)
    {
        return Err(semantic_error(
            "selected Counter.set must be the original accessible Counter member",
        ));
    }
    let method_tree = match source_index.definition_of(method) {
        Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
        _ => {
            return Err(identity_error(
                "selected Counter.set has no original source definition",
            ));
        }
    };
    let TreeKind::TypeDef(counter_def) = &source_ast
        .try_get(match source_index.definition_of(pair.counter) {
            Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
            _ => return Err(identity_error("Counter definition is not original")),
        })
        .ok_or_else(|| identity_error("Counter definition tree is stale"))?
        .kind
    else {
        return Err(identity_error("Counter source root must be TypeDef"));
    };
    let TreeKind::Template(counter_template) = &source_ast
        .try_get(counter_def.rhs)
        .ok_or_else(|| identity_error("Counter template is stale"))?
        .kind
    else {
        return Err(identity_error("Counter definition must have Template"));
    };
    if !counter_template.body.iter().any(|tree| {
        *tree == method_tree && source_index.symbol_at(source_id, *tree) == Some(method)
    }) {
        return Err(identity_error(
            "selected set method is not a direct Counter member",
        ));
    }
    let TreeKind::DefDef(set_def) = &source_ast
        .try_get(method_tree)
        .ok_or_else(|| identity_error("selected set declaration is stale"))?
        .kind
    else {
        return Err(identity_error(
            "selected Counter member must be a source method",
        ));
    };
    if store.names.resolve(source_selection.name.text()) != "set"
        || store.names.resolve(set_def.name.as_name().text()) != "set"
        || !set_def.type_params.is_empty()
        || set_def.value_param_clauses.len() != 1
        || set_def.value_param_clauses[0].len() != 1
        || set_def.metadata.visibility.is_some()
    {
        return Err(source_error(
            "selected member must be the accessible original one-argument set method",
        ));
    }
    let formal_tree = set_def.value_param_clauses[0][0];
    let TreeKind::ValDef(set_formal_def) = &source_ast
        .try_get(formal_tree)
        .ok_or_else(|| source_error("Counter.set formal is stale"))?
        .kind
    else {
        return Err(source_error("Counter.set must retain its value formal"));
    };
    if set_formal_def.rhs.is_some()
        || matches!(
            source_ast
                .try_get(set_formal_def.tpt)
                .map(|node| &node.kind),
            Some(TreeKind::TypeTree(_))
        )
    {
        return Err(source_error(
            "Counter.set formal must retain its explicit Int type",
        ));
    }
    let set_formal = source_index
        .symbol_at(source_id, formal_tree)
        .ok_or_else(|| identity_error("Counter.set formal has no canonical symbol"))?;
    let SymbolInfo::Complete(method_type) = store.symbols.get(method).info else {
        return Err(semantic_error("Counter.set signature is incomplete"));
    };
    let Some(Type::Method(signature)) = store.types.try_get(method_type) else {
        return Err(semantic_error("Counter.set must have a method signature"));
    };
    if signature.kind != dotty_core::MethodKind::Plain
        || signature.params.len() != 1
        || signature.params[0].ty != definitions.int
        || signature.result != definitions.unit
        || !store.symbols.contains(set_formal)
        || store.symbols.get(set_formal).owner != Some(method)
        || store.symbols.get(set_formal).kind != SymbolKind::Parameter
        || !matches!(store.symbols.get(set_formal).info, SymbolInfo::Complete(ty) if ty == definitions.int)
    {
        return Err(semantic_error(
            "Counter.set must be the canonical Int => Unit member",
        ));
    }
    let argument = match store.types.try_get(
        typed_ast
            .try_get(typed_argument)
            .ok_or_else(|| typed_error("typed Counter.set argument is stale"))?
            .ty,
    ) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) => *symbol,
        _ => {
            return Err(typed_error(
                "typed Counter.set argument must retain its formal TermRef",
            ));
        }
    };
    if argument != call_formal {
        return Err(typed_error("Counter.set argument must target secondSeed"));
    }
    let TreeKind::Ident(typed_argument_ident) = &typed_ast
        .try_get(typed_argument)
        .ok_or_else(|| typed_error("typed Counter.set argument is stale"))?
        .kind
    else {
        return Err(typed_error(
            "typed Counter.set argument must remain the original secondSeed Ident",
        ));
    };
    if store.names.resolve(typed_argument_ident.name.text())
        != store.names.resolve(source_argument_ident.name.text())
    {
        return Err(typed_error(
            "typed Counter.set argument Ident must preserve its original formal name",
        ));
    }
    let argument_type = match store.symbols.get(argument).info {
        SymbolInfo::Complete(ty) => ty,
        _ => return Err(semantic_error("secondSeed formal type is incomplete")),
    };
    let result_type = typed_ast
        .try_get(typed_call)
        .ok_or_else(|| typed_error("typed Counter.set Apply is stale"))?
        .ty;
    if argument_type != definitions.int || result_type != definitions.unit {
        return Err(typed_error(
            "typed Counter.set call must preserve Int argument and Unit result",
        ));
    }
    let call_pos = require_source_position(
        source_ast,
        source_id,
        source_call,
        "Counter.set call has a foreign source span",
    )?;
    let second_init_pos = require_source_position(
        source_ast,
        source_id,
        second.declaration.initializer,
        "second local initializer has a foreign source span",
    )?;
    let final_pos = require_source_position(
        source_ast,
        source_id,
        pair.source_final_expression,
        "final Pair expression has a foreign source span",
    )?;
    if second_init_pos.span().range().end() > call_pos.span().range().start()
        || call_pos.span().range().end() > final_pos.span().range().start()
    {
        return Err(source_error(
            "Counter.set must follow both initializers and precede Pair",
        ));
    }
    for (source_tree, typed_tree, label) in [
        (source_call, typed_call, "Counter.set statement"),
        (source_select, typed_select, "Counter.set Select"),
        (source_receiver, typed_receiver, "Counter.set receiver"),
        (source_argument, typed_argument, "Counter.set argument"),
    ] {
        require_matching_source_typed_positions(
            source_ast,
            typed_ast,
            source_id,
            source_tree,
            typed_tree,
            label,
        )?;
    }
    Ok(ValidatedImmutableLocalCounterSetCallProof {
        initializer_pair: pair,
        source_call,
        typed_call,
        source_select,
        typed_select,
        source_receiver,
        typed_receiver,
        source_argument,
        typed_argument,
        method,
        receiver,
        formal: call_formal,
        argument_type,
        result_type,
    })
}
