//! Explicitly typed ordinary function literals.

use super::super::{ExpressionContext, SourceFunctionKind, SourceTyper, TyperError};
use dotty_core::ast::*;
use dotty_core::types::*;
use dotty_core::*;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Default)]
pub(in crate::typer) struct FunctionLiteralIndex {
    definitions: HashMap<(SourceId, TreeId<Untyped>), FunctionLiteralDefinition>,
    parameters: HashMap<(SourceId, TreeId<Untyped>), SymbolId>,
}

#[derive(Clone, Copy)]
pub(in crate::typer) struct FunctionLiteralTypingContext {
    pub context: ExpressionContext,
    pub expected_function1: Option<(TypeId, Option<TypeId>)>,
    pub poly_binder: Option<TypeId>,
}

#[derive(Clone)]
struct FunctionLiteralDefinition {
    method: SymbolId,
    scope: ScopeId,
    typed_definition: Option<TreeId<Typed>>,
    captures: Vec<(SymbolId, TreeId<Typed>)>,
    capture_enclosing_values: bool,
}

impl FunctionLiteralIndex {
    pub(in crate::typer) fn is_method(&self, method: SymbolId) -> bool {
        self.definitions
            .values()
            .any(|definition| definition.method == method)
    }

    pub(in crate::typer) fn method_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.definitions
            .get(&(source, tree))
            .map(|entry| entry.method)
    }

    pub(in crate::typer) fn scope_of(&self, method: SymbolId) -> Option<ScopeId> {
        self.definitions
            .values()
            .find(|entry| entry.method == method)
            .map(|entry| entry.scope)
    }

    pub(in crate::typer) fn definition_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<TreeId<Typed>> {
        self.definitions
            .get(&(source, tree))
            .and_then(|entry| entry.typed_definition)
    }

    pub(in crate::typer) fn parameter_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.parameters.get(&(source, tree)).copied()
    }

    pub(in crate::typer) fn capture(
        &mut self,
        method: SymbolId,
        symbol: SymbolId,
        value: TreeId<Typed>,
    ) {
        if let Some(definition) = self
            .definitions
            .values_mut()
            .find(|definition| definition.method == method)
            && !definition
                .captures
                .iter()
                .any(|(captured, _)| *captured == symbol)
        {
            definition.captures.push((symbol, value));
        }
    }

    pub(in crate::typer) fn captures(&self, method: SymbolId) -> Vec<TreeId<Typed>> {
        self.definitions
            .values()
            .find(|definition| definition.method == method)
            .map(|definition| {
                definition
                    .captures
                    .iter()
                    .map(|(_, value)| *value)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(in crate::typer) fn can_capture_enclosing_this(
        &self,
        store: &SemanticStore,
        method: SymbolId,
        class: SymbolId,
    ) -> bool {
        if !self
            .definitions
            .values()
            .any(|definition| definition.method == method && definition.capture_enclosing_values)
            || !store.symbols.contains(method)
            || !store.symbols.contains(class)
            || !matches!(
                store.symbols.get(class).kind,
                SymbolKind::Class | SymbolKind::Trait
            )
        {
            return false;
        }
        let lambda = store.symbols.get(method);
        let Some(source_method) = lambda.owner else {
            return false;
        };
        if lambda.kind != SymbolKind::Method
            || !store.symbols.contains(source_method)
            || store.symbols.get(source_method).kind != SymbolKind::Method
            || self.is_method(source_method)
        {
            return false;
        }
        let Some(owner) = store.symbols.get(source_method).owner else {
            return false;
        };
        if !store.symbols.contains(owner)
            || !matches!(
                store.symbols.get(owner).kind,
                SymbolKind::Class | SymbolKind::Trait
            )
        {
            return false;
        }
        let mut current = Some(owner);
        let mut seen = HashSet::new();
        while let Some(symbol) = current {
            if symbol == class {
                return true;
            }
            if !seen.insert(symbol) || !store.symbols.contains(symbol) {
                return false;
            }
            current = store.symbols.get(symbol).owner;
        }
        false
    }

    pub(in crate::typer) fn same_owner_instance_member(
        &self,
        store: &SemanticStore,
        method: SymbolId,
        symbol: SymbolId,
    ) -> Option<SymbolId> {
        if !self
            .definitions
            .values()
            .any(|definition| definition.method == method && definition.capture_enclosing_values)
            || !store.symbols.contains(method)
            || !store.symbols.contains(symbol)
        {
            return None;
        }
        let lambda = store.symbols.get(method);
        let source_method = lambda.owner?;
        if lambda.kind != SymbolKind::Method
            || !store.symbols.contains(source_method)
            || store.symbols.get(source_method).kind != SymbolKind::Method
            || self.is_method(source_method)
        {
            return None;
        }
        let owner = store.symbols.get(source_method).owner?;
        if !store.symbols.contains(owner)
            || !matches!(
                store.symbols.get(owner).kind,
                SymbolKind::Class | SymbolKind::Trait
            )
        {
            return None;
        }
        let member = store.symbols.get(symbol);
        if member.owner != Some(owner) || member.flags.contains(SymbolFlags::STATIC) {
            return None;
        }
        let capturable = match member.kind {
            SymbolKind::Method => !member.flags.contains(SymbolFlags::EXTENSION),
            SymbolKind::Field | SymbolKind::Value => !member.flags.contains(SymbolFlags::MUTABLE),
            _ => false,
        };
        capturable.then_some(owner)
    }

    pub(in crate::typer) fn can_capture_method_parameter(
        &self,
        store: &SemanticStore,
        method: SymbolId,
        symbol: SymbolId,
    ) -> bool {
        if !self
            .definitions
            .values()
            .any(|definition| definition.method == method && definition.capture_enclosing_values)
        {
            return false;
        }
        if !store.symbols.contains(symbol) {
            return false;
        }
        let parameter = store.symbols.get(symbol);
        let Some(owner) = store.symbols.get(method).owner else {
            return false;
        };
        if parameter.kind != SymbolKind::Parameter
            || parameter.owner != Some(owner)
            || parameter.flags.contains(SymbolFlags::MUTABLE)
            || !store.symbols.contains(owner)
            || store.symbols.get(owner).kind != SymbolKind::Method
            || self.is_method(owner)
        {
            return false;
        }
        matches!(
            parameter.info,
            SymbolInfo::Complete(ty)
                if !matches!(store.types.try_get(ty), Some(Type::ByName { .. } | Type::Repeated { .. }))
        )
    }

    pub(in crate::typer) fn can_capture_local_value(
        &self,
        store: &SemanticStore,
        method: SymbolId,
        symbol: SymbolId,
    ) -> bool {
        if !self
            .definitions
            .values()
            .any(|definition| definition.method == method && definition.capture_enclosing_values)
            || !store.symbols.contains(symbol)
        {
            return false;
        }
        let local = store.symbols.get(symbol);
        let Some(owner) = store.symbols.get(method).owner else {
            return false;
        };
        local.kind == SymbolKind::Local
            && local.owner == Some(owner)
            && local.flags.difference(SymbolFlags::FINAL) == SymbolFlags::EMPTY
            && matches!(local.info, SymbolInfo::Complete(ty) if !matches!(store.types.try_get(ty), Some(Type::ByName { .. } | Type::Repeated { .. })))
            && store.symbols.contains(owner)
            && store.symbols.get(owner).kind == SymbolKind::Method
            && !self.is_method(owner)
    }
}

impl SourceTyper<'_> {
    pub(in crate::typer) fn ordinary_function1_parts(
        &mut self,
        expected: TypeId,
        tree_index: u32,
    ) -> Result<Option<(TypeId, TypeId)>, TyperError> {
        let Some(Type::Applied { tycon, args }) = self.store.types.try_get(expected) else {
            return Ok(None);
        };
        if args.len() != 2 {
            return Ok(None);
        }
        let (tycon, arguments) = (*tycon, [args[0], args[1]]);
        let Some(Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(class),
        }) = self.store.types.try_get(tycon)
        else {
            return Ok(None);
        };
        let class = *class;
        let prefix = *prefix;
        let canonical = self.source_function_class(SourceFunctionKind::Ordinary, 1, tree_index)?;
        let canonical_package = self.store.symbols.get(canonical).owner;
        let canonical_prefix = canonical_package.is_some_and(|package| {
            matches!(
                self.store.types.try_get(prefix),
                Some(Type::TypeRef {
                    prefix: root,
                    target: TypeRefTarget::Symbol(target),
                }) if *root == self.definitions.no_prefix && *target == package
            )
        });
        if class != canonical || !canonical_prefix {
            return Ok(None);
        }
        Ok(Some((arguments[0], arguments[1])))
    }

    pub(in crate::typer) fn type_function_literal(
        &mut self,
        tree: TreeId<Untyped>,
        function: Function,
        position: Option<SourceSpan>,
        typing_context: FunctionLiteralTypingContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let FunctionLiteralTypingContext {
            context,
            expected_function1,
            poly_binder,
        } = typing_context;
        let mut parameters = Vec::with_capacity(function.params.len());
        for parameter_tree in function.params.iter().copied() {
            let Some(parameter_node) = self.arena.try_get(parameter_tree).cloned() else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: parameter_tree.index(),
                });
            };
            let TreeKind::ValDef(parameter) = parameter_node.kind else {
                return Err(TyperError::UnsupportedFunctionLiteralParameter {
                    source: self.source,
                    tree_index: parameter_tree.index(),
                    reason: "parameter is not a value definition",
                });
            };
            if parameter.metadata.visibility.is_some()
                || !parameter.metadata.modifiers.is_empty()
                || !parameter.metadata.annotations.is_empty()
            {
                return Err(TyperError::UnsupportedFunctionLiteralParameter {
                    source: self.source,
                    tree_index: parameter_tree.index(),
                    reason: "parameter modifiers and annotations are unsupported",
                });
            }
            let parameter_type = match self.type_of_tpt_inner_journaled(
                parameter.tpt,
                context.lexical,
                info_journal,
            ) {
                Err(TyperError::MissingDeclaredType { .. }) => {
                    let Some((inferred, _)) = expected_function1 else {
                        return Err(TyperError::UnsupportedFunctionLiteralParameter {
                            source: self.source,
                            tree_index: parameter_tree.index(),
                            reason: "an explicit parameter type is required",
                        });
                    };
                    if function.params.len() != 1 {
                        return Err(TyperError::UnsupportedFunctionLiteralParameter {
                            source: self.source,
                            tree_index: parameter_tree.index(),
                            reason: "Function1 expectations require exactly one lambda parameter",
                        });
                    }
                    inferred
                }
                result => result?,
            };
            parameters.push((
                parameter_tree,
                parameter_node.position,
                parameter,
                parameter_type,
            ));
        }

        let method_name_text = format!("$sourceLambda${}", tree.index());
        let method_name = Name::new(self.store.names.intern(&method_name_text), Namespace::Term);
        let method = self.store.symbols.alloc(Symbol {
            name: method_name,
            owner: Some(context.owner),
            kind: SymbolKind::Method,
            flags: SymbolFlags::SYNTHETIC | SymbolFlags::FINAL,
            visibility: Visibility::Private,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position,
            links: SymbolLinks::default(),
        });
        let method_scope = self.store.scopes.alloc(Scope::new(Some(method)));
        if self
            .function_literals
            .definitions
            .insert(
                (self.source, tree),
                FunctionLiteralDefinition {
                    method,
                    scope: method_scope,
                    typed_definition: None,
                    captures: Vec::new(),
                    capture_enclosing_values: function.params.len() == 1,
                },
            )
            .is_some()
        {
            return Err(TyperError::UnsupportedExpression {
                source: self.source,
                tree_index: tree.index(),
                expression_kind: "function literal identity was already assigned",
            });
        }

        let mut typed_parameters = Vec::with_capacity(parameters.len());
        let mut method_parameters = Vec::with_capacity(parameters.len());
        for (parameter_tree, parameter_position, parameter, parameter_type) in parameters {
            let name = *parameter.name.as_name();
            if !self
                .store
                .scopes
                .get(method_scope)
                .lookup_all(&name)
                .is_empty()
            {
                return Err(TyperError::UnsupportedFunctionLiteralParameter {
                    source: self.source,
                    tree_index: parameter_tree.index(),
                    reason: "duplicate parameter names are unsupported",
                });
            }
            let symbol = self.store.symbols.alloc(Symbol {
                name,
                owner: Some(method),
                kind: SymbolKind::Parameter,
                flags: SymbolFlags::EMPTY,
                visibility: Visibility::Public,
                info: SymbolInfo::Complete(parameter_type),
                origin: SymbolOrigin::Synthetic,
                annotations: Vec::new(),
                position: parameter_position,
                links: SymbolLinks::default(),
            });
            self.store.scopes.get_mut(method_scope).enter(name, symbol);
            let typed_tpt =
                self.reify_constructor_type_tree(parameter.tpt, parameter_type, new_mappings)?;
            let typed_parameter = self.typed_arena.alloc(Tree {
                kind: TreeKind::ValDef(ValDef {
                    name: parameter.name,
                    tpt: typed_tpt,
                    rhs: None,
                    metadata: (),
                }),
                position: parameter_position,
                ty: parameter_type,
            });
            self.typed_index
                .insert(self.source, parameter_tree, typed_parameter)
                .map_err(|error| TyperError::ConflictingTypedExpression {
                    source: error.source,
                    tree_index: error.untyped.index(),
                    existing: error.existing.index(),
                    attempted: error.attempted.index(),
                })?;
            new_mappings.push((self.source, parameter_tree));
            self.function_literals
                .parameters
                .insert((self.source, parameter_tree), symbol);
            typed_parameters.push(typed_parameter);
            method_parameters.push(MethodParam {
                name: parameter.name,
                ty: parameter_type,
                erased: false,
                varargs: false,
            });
        }

        let lambda_context = self.push_local_scope(
            ExpressionContext {
                lexical: context.lexical,
                owner: method,
                local_scopes: context.local_scopes,
            },
            method_scope,
        )?;
        let typed_body = if let Some((_, Some(expected_output))) = expected_function1 {
            self.type_expression_expected_with_poly_binder(
                function.body,
                lambda_context,
                expected_output,
                poly_binder,
                info_journal,
                new_mappings,
            )
        } else {
            self.type_value_expression_inner(
                function.body,
                lambda_context,
                info_journal,
                new_mappings,
            )
        };
        let typed_body = typed_body?;
        let body_result = self.widen_expression_type_journaled(
            self.typed_arena.get(typed_body).ty,
            info_journal,
            0,
        )?;

        let function_result = expected_function1
            .and_then(|(_, result)| result)
            .unwrap_or(body_result);
        let method_type = self.store.types.alloc(Type::Method(MethodType {
            params: method_parameters,
            result: function_result,
            kind: MethodKind::Plain,
        }));
        self.store
            .symbols
            .set_info(method, SymbolInfo::Complete(method_type));
        let method_ref = self.store.types.alloc(Type::TermRef {
            prefix: self.definitions.no_prefix,
            target: TermRefTarget::Symbol(method),
        });
        let typed_result = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .type_tree(function_result, position);
        let typed_method = self.typed_arena.alloc(Tree {
            kind: TreeKind::DefDef(DefDef {
                name: TermName::new(method_name.text()),
                type_params: Vec::new(),
                value_param_clauses: vec![typed_parameters],
                source_param_clause_order: None,
                tpt: typed_result,
                rhs: Some(typed_body),
                metadata: (),
            }),
            position,
            ty: method_ref,
        });
        let Some(definition) = self
            .function_literals
            .definitions
            .get_mut(&(self.source, tree))
        else {
            return Err(TyperError::UnsupportedExpression {
                source: self.source,
                tree_index: tree.index(),
                expression_kind: "function literal definition identity is missing",
            });
        };
        definition.typed_definition = Some(typed_method);

        let tycon = self.source_function_type_constructor(
            SourceFunctionKind::Ordinary,
            function.params.len(),
            tree.index(),
        )?;
        let mut args = function
            .params
            .iter()
            .map(|parameter| {
                self.function_literals
                    .parameter_at(self.source, *parameter)
                    .and_then(|symbol| match self.store.symbols.info(symbol) {
                        SymbolInfo::Complete(ty) => Some(*ty),
                        _ => None,
                    })
                    .ok_or(TyperError::UnsupportedFunctionLiteralParameter {
                        source: self.source,
                        tree_index: parameter.index(),
                        reason: "parameter type was not completed",
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        args.push(function_result);
        let function_type = self.store.types.alloc(Type::Applied { tycon, args });

        let typed_method_reference = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .ident(method_name, method_ref, position);
        let captures = self.function_literals.captures(method);
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).closure(
                captures,
                typed_method_reference,
                None,
                function_type,
                position,
            ),
        )
    }
}
