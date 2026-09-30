//! Opaque alias completion: external bounds plus the owner's hidden alias.
//!
//! The public symbol receives `Bounds` (or a `TypeLambda` returning bounds).
//! The defining class/module's self type receives a same-named refinement
//! whose `AliasingBounds` retains the implementation RHS. Both changes are
//! made in the caller's completion transaction.

use std::collections::{HashMap, HashSet};

use dotty_core::ids::{SymbolId, TypeId};
use dotty_core::symbols::{SymbolFlags, SymbolInfo, SymbolKind};
use dotty_core::types::{
    MatchType, Type, TypeParamSpec, TypeRefTarget, type_lambda_from_symbols,
    type_lambda_with_result,
};
use dotty_tasty::tasty::{LAMBDATPT_TAG, TYPEBOUNDSTPT_TAG};

use crate::ast_view::{AstView, address};
use crate::error::UnpickleError;
use crate::unpickler::TastyUnpickler;

impl TastyUnpickler<'_, '_, '_> {
    pub(crate) fn complete_opaque_alias(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        symbol: SymbolId,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        let owner = self.store.symbols.get(symbol).owner.ok_or(
            UnpickleError::OpaqueAliasOwnerNotClassLike {
                address: at,
                owner: symbol,
                kind: self.store.symbols.get(symbol).kind,
            },
        )?;
        let owner_kind = self.store.symbols.get(owner).kind;
        if !matches!(
            owner_kind,
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
        ) {
            return Err(UnpickleError::OpaqueAliasOwnerNotClassLike {
                address: at,
                owner,
                kind: owner_kind,
            });
        }

        if matches!(self.store.symbols.get(owner).info, SymbolInfo::Missing) {
            let owner_at = self
                .index
                .definition_address(owner)
                .ok_or(UnpickleError::OpaqueAliasOwnerNotEntered { address: at, owner })?;
            self.complete_in(ast, owner_at, depth)?;
        }
        let owner_info_type = match self.store.symbols.get(owner).info {
            SymbolInfo::Complete(info) => info,
            _ => return Err(UnpickleError::OpaqueAliasOwnerNotEntered { address: at, owner }),
        };
        let mut owner_info = match self.store.types.get(owner_info_type) {
            Type::ClassInfo(info) => info.clone(),
            _ => {
                return Err(UnpickleError::MalformedOwnerClassInfo {
                    address: at,
                    owner,
                    info: owner_info_type,
                });
            }
        };

        let [rhs_child] = ast.children(at) else {
            return Err(UnpickleError::MalformedDefinition {
                address: at,
                reason: "an opaque type alias has one right-hand-side tree",
            });
        };
        let rhs_at = address(rhs_child.offset);
        let projected = self.type_of_tpt(ast, rhs_at, at, depth)?;
        self.ensure_alias_graph_acyclic(ast, symbol, projected, at, depth)?;
        let (body_at, lambda_params) = self.unwrap_lambda_tpt(ast, rhs_at)?;
        let body_tag = ast
            .tag_at(body_at)
            .ok_or(UnpickleError::MissingDefinition { address: body_at })?;

        let (public_bounds, implementation) = if body_tag == TYPEBOUNDSTPT_TAG {
            let shape = ast.node(body_at)?.decode_type_bounds()?;
            let children = ast.children(body_at);
            if children.len()
                != 1 + usize::from(shape.high.is_some()) + usize::from(shape.alias.is_some())
            {
                return Err(UnpickleError::MalformedDefinition {
                    address: body_at,
                    reason: "opaque bounds children do not match their encoded shape",
                });
            }
            if shape.high.is_some() {
                let low = self.type_of_tpt(ast, address(children[0].offset), body_at, depth)?;
                let high = self.type_of_tpt(ast, address(children[1].offset), body_at, depth)?;
                let bounds = self.store.types.alloc(Type::Bounds { low, high });
                let public = if lambda_params.is_empty() {
                    bounds
                } else {
                    self.opaque_type_lambda(ast, &lambda_params, bounds, at, depth)?
                };
                let implementation = if shape.alias.is_some() {
                    let alias =
                        self.type_of_tpt(ast, address(children[2].offset), body_at, depth)?;
                    Some(if lambda_params.is_empty() {
                        alias
                    } else {
                        self.opaque_type_lambda(ast, &lambda_params, alias, at, depth)?
                    })
                } else {
                    None
                };
                (public, implementation)
            } else {
                let alias = match self.store.types.get(projected) {
                    Type::AliasingBounds { alias } => *alias,
                    Type::TypeLambda(_) => projected,
                    _ => projected,
                };
                let empty = self.empty_opaque_bounds();
                let public = if lambda_params.is_empty() {
                    empty
                } else {
                    self.opaque_type_lambda(ast, &lambda_params, empty, at, depth)?
                };
                (public, Some(alias))
            }
        } else {
            let alias = match self.store.types.get(projected) {
                Type::TypeLambda(_) => projected,
                Type::AliasingBounds { alias } => *alias,
                _ => projected,
            };
            let empty = self.empty_opaque_bounds();
            let public = if lambda_params.is_empty() {
                empty
            } else {
                type_lambda_with_result(&mut self.store, projected, empty)
                    .map_err(|error| UnpickleError::ParameterAbstraction { address: at, error })?
            };
            (public, Some(alias))
        };

        if let Some(implementation) = implementation {
            let name = self.store.symbols.get(symbol).name;
            let alias_bounds = self.store.types.alloc(Type::AliasingBounds {
                alias: implementation,
            });
            owner_info.self_type = Some(self.insert_owner_alias(
                owner,
                owner_info.declarations,
                owner_info.self_type,
                name,
                alias_bounds,
                at,
            )?);
            let updated = self.store.types.alloc(Type::ClassInfo(owner_info));
            self.set_symbol_info(owner, SymbolInfo::Complete(updated));
        }
        self.set_symbol_info(symbol, SymbolInfo::Complete(public_bounds));
        Ok(public_bounds)
    }

    fn empty_opaque_bounds(&mut self) -> TypeId {
        self.store.types.alloc(Type::Bounds {
            low: self.definitions.nothing_type,
            high: self.definitions.any_type,
        })
    }

    /// Follows same-session type-alias RHSs without completing their symbols.
    /// This mirrors `checkNonCyclic`: it rejects direct and indirect alias
    /// cycles before owner or public info is published.
    fn ensure_alias_graph_acyclic(
        &mut self,
        ast: &AstView<'_>,
        root: SymbolId,
        root_type: TypeId,
        alias_address: u32,
        depth: usize,
    ) -> Result<(), UnpickleError> {
        let mut states = HashMap::<SymbolId, u8>::new();
        let mut pending = vec![(root, false)];
        while let Some((symbol, exiting)) = pending.pop() {
            if exiting {
                states.insert(symbol, 2);
                continue;
            }
            match states.get(&symbol).copied() {
                Some(1) => {
                    return Err(UnpickleError::OpaqueAliasCycle {
                        address: alias_address,
                    });
                }
                Some(2) => continue,
                _ => {}
            }
            if states.len() >= 512 {
                return Err(UnpickleError::MalformedDefinition {
                    address: alias_address,
                    reason: "an opaque alias dependency graph exceeds the recursion limit",
                });
            }
            states.insert(symbol, 1);
            pending.push((symbol, true));
            let rhs_type = if symbol == root {
                root_type
            } else {
                let Some(definition) = self.index.definition_address(symbol) else {
                    // References outside this unit are left to the external
                    // resolver, just like other cross-unit semantic links.
                    states.insert(symbol, 2);
                    continue;
                };
                let [rhs] = ast.children(definition) else {
                    return Err(UnpickleError::MalformedDefinition {
                        address: definition,
                        reason: "a type alias has one right-hand-side tree",
                    });
                };
                self.type_of_tpt(ast, address(rhs.offset), definition, depth)?
            };
            let mut dependencies = self.type_alias_references(rhs_type);
            dependencies.retain(|dependency| {
                self.store.symbols.get(*dependency).kind == SymbolKind::TypeAlias
            });
            dependencies.sort_by_key(|symbol| symbol.index());
            dependencies.dedup();
            pending.extend(
                dependencies
                    .into_iter()
                    .rev()
                    .map(|dependency| (dependency, false)),
            );
        }
        Ok(())
    }

    fn type_alias_references(&self, root: TypeId) -> Vec<SymbolId> {
        let mut found = Vec::new();
        let mut pending = vec![root];
        let mut visited = HashSet::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            match self.store.types.get(id) {
                Type::NoType | Type::Error(_) | Type::NoPrefix | Type::Constant(_) => {}
                Type::TermRef { prefix, .. } => pending.push(*prefix),
                Type::TypeRef { prefix, target } => {
                    pending.push(*prefix);
                    if let TypeRefTarget::Symbol(symbol) = target {
                        found.push(*symbol);
                    }
                }
                Type::ThisType { .. } | Type::RecThis { .. } | Type::ParamRef { .. } => {}
                Type::SuperType {
                    this_type,
                    super_type,
                } => pending.extend([*this_type, *super_type]),
                Type::Applied { tycon, args } => {
                    pending.push(*tycon);
                    pending.extend(args.iter().copied());
                }
                Type::Bounds { low, high } => pending.extend([*low, *high]),
                Type::AliasingBounds { alias } => pending.push(*alias),
                Type::ByName { result } => pending.push(*result),
                Type::Flexible { underlying } => pending.push(*underlying),
                Type::And { left, right } | Type::Or { left, right } => {
                    pending.extend([*left, *right]);
                }
                Type::Refined { parent, info, .. } => pending.extend([*parent, *info]),
                Type::Recursive { parent } => pending.push(*parent),
                Type::Method(method) => {
                    pending.extend(method.params.iter().map(|param| param.ty));
                    pending.push(method.result);
                }
                Type::Poly(poly) => {
                    pending.extend(poly.params.iter().map(|param| param.bounds));
                    pending.push(poly.result);
                }
                Type::TypeLambda(lambda) => {
                    pending.extend(lambda.params.iter().map(|param| param.bounds));
                    pending.push(lambda.result);
                }
                Type::Match(MatchType {
                    bound,
                    scrutinee,
                    cases,
                }) => {
                    pending.extend([*bound, *scrutinee]);
                    pending.extend(cases.iter().copied());
                }
                Type::MatchCase { pattern, result } => pending.extend([*pattern, *result]),
                Type::Annotated { underlying, .. } => pending.push(*underlying),
                Type::Wildcard { bounds } => pending.push(*bounds),
                Type::JavaArray { element } | Type::Repeated { element } => {
                    pending.push(*element);
                }
                Type::ClassInfo(info) => {
                    pending.extend(info.parents.iter().copied());
                    pending.extend(info.self_type);
                }
            }
        }
        found
    }

    fn unwrap_lambda_tpt(
        &self,
        ast: &AstView<'_>,
        mut at: u32,
    ) -> Result<(u32, Vec<u32>), UnpickleError> {
        let mut params = Vec::new();
        while ast.tag_at(at) == Some(LAMBDATPT_TAG) {
            ast.node(at)?.decode_lambda_tpt()?;
            let children = ast.children(at);
            let Some((body, type_params)) = children
                .split_last()
                .filter(|(_, params)| !params.is_empty())
            else {
                return Err(UnpickleError::MalformedDefinition {
                    address: at,
                    reason: "a generic opaque alias lambda has parameters and a body",
                });
            };
            if !params.is_empty() {
                return Err(UnpickleError::MalformedDefinition {
                    address: at,
                    reason: "nested opaque alias parameter lambdas are unsupported",
                });
            }
            params.extend(type_params.iter().map(|param| address(param.offset)));
            at = address(body.offset);
        }
        Ok((at, params))
    }

    fn opaque_type_lambda(
        &mut self,
        ast: &AstView<'_>,
        parameters: &[u32],
        result: TypeId,
        at: u32,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        let mut specs = Vec::with_capacity(parameters.len());
        for parameter in parameters {
            let Some(symbol) = self.index.symbol_at(*parameter) else {
                return Err(UnpickleError::MissingEnteredSymbol {
                    address: *parameter,
                });
            };
            let bounds = self.complete_in(ast, *parameter, depth)?;
            specs.push(TypeParamSpec {
                symbol,
                name: dotty_core::names::TypeName::new(self.store.symbols.get(symbol).name.text()),
                bounds,
                declared_variance: self.declared_variance(ast, *parameter)?,
            });
        }
        type_lambda_from_symbols(&mut self.store, &specs, result)
            .map_err(|error| UnpickleError::ParameterAbstraction { address: at, error })
    }

    fn insert_owner_alias(
        &mut self,
        owner: SymbolId,
        declarations: dotty_core::ids::ScopeId,
        self_type: Option<TypeId>,
        name: dotty_core::names::Name,
        info: TypeId,
        address: u32,
    ) -> Result<TypeId, UnpickleError> {
        let mut parent =
            self_type.unwrap_or_else(|| self.store.types.alloc(Type::ThisType { class: owner }));
        let mut opaque_refinements = Vec::new();
        loop {
            let Type::Refined {
                parent: next,
                name: old_name,
                info: old_info,
            } = self.store.types.get(parent).clone()
            else {
                break;
            };
            let is_opaque = self
                .store
                .scopes
                .get(declarations)
                .lookup(&old_name)
                .is_some_and(|symbol| {
                    self.store
                        .symbols
                        .get(symbol)
                        .flags
                        .contains(SymbolFlags::OPAQUE)
                });
            if !is_opaque {
                break;
            }
            if old_name == name {
                return Err(UnpickleError::OpaqueAliasCycle { address });
            }
            opaque_refinements.push((old_name, old_info));
            parent = next;
        }
        opaque_refinements.push((name, info));
        opaque_refinements.sort_by(|(left, _), (right, _)| {
            self.store
                .names
                .resolve(left.text())
                .cmp(self.store.names.resolve(right.text()))
        });
        for (name, info) in opaque_refinements.into_iter().rev() {
            parent = self.store.types.alloc(Type::Refined { parent, name, info });
        }
        Ok(parent)
    }
}
