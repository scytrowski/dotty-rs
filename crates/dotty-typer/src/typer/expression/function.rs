//! Explicitly typed ordinary function literals.

use super::super::{ExpressionContext, SourceFunctionKind, SourceTyper, TyperError};
use dotty_core::ast::*;
use dotty_core::types::*;
use dotty_core::*;
use std::collections::HashMap;

#[derive(Clone, Default)]
pub(in crate::typer) struct FunctionLiteralIndex {
    definitions: HashMap<(SourceId, TreeId<Untyped>), FunctionLiteralDefinition>,
    parameters: HashMap<(SourceId, TreeId<Untyped>), SymbolId>,
}

#[derive(Clone)]
struct FunctionLiteralDefinition {
    method: SymbolId,
    scope: ScopeId,
}

impl FunctionLiteralIndex {
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

    pub(in crate::typer) fn parameter_at(
        &self,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.parameters.get(&(source, tree)).copied()
    }
}

impl SourceTyper<'_> {
    pub(in crate::typer) fn type_function_literal(
        &mut self,
        tree: TreeId<Untyped>,
        function: Function,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
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
                    return Err(TyperError::UnsupportedFunctionLiteralParameter {
                        source: self.source,
                        tree_index: parameter_tree.index(),
                        reason: "an explicit parameter type is required",
                    });
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
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position,
            links: SymbolLinks::default(),
        });
        let method_scope = self.store.scopes.alloc(Scope::new(Some(method)));

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
                local_scopes: None,
            },
            method_scope,
        )?;
        let typed_body = self.type_value_expression_inner(
            function.body,
            lambda_context,
            info_journal,
            new_mappings,
        );
        let typed_body = typed_body?;
        let body_result = self.widen_expression_type_journaled(
            self.typed_arena.get(typed_body).ty,
            info_journal,
            0,
        )?;

        let method_type = self.store.types.alloc(Type::Method(MethodType {
            params: method_parameters,
            result: body_result,
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
            .type_tree(body_result, position);
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
        args.push(body_result);
        let function_type = self.store.types.alloc(Type::Applied { tycon, args });

        if self
            .function_literals
            .definitions
            .insert(
                (self.source, tree),
                FunctionLiteralDefinition {
                    method,
                    scope: method_scope,
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

        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).closure(
                Vec::new(),
                typed_method,
                None,
                function_type,
                position,
            ),
        )
    }
}
