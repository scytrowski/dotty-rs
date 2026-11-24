//! Contextual forwarders declared by a source object's export clauses.

use super::*;
use dotty_core::ast::{DefDef, TypeDef, ValDef};
use dotty_core::types::{freshen_type_binders, instantiate_method_result, instantiate_poly};

// Cold qualifiers recursively enter source class completion before their scope is read.
const MAX_EXPORT_QUALIFIER_DEPTH: usize = 16;

enum ExportSelection {
    Named(Name),
    Wildcard(Name),
}

struct ExportBody {
    rhs: TreeId<Typed>,
    type_params: Vec<TreeId<Typed>>,
    value_param_clauses: Vec<Vec<TreeId<Typed>>>,
}

impl SourceTyper<'_> {
    pub(in crate::typer) fn enter_contextual_exports(
        &mut self,
        owner: SymbolId,
        scope: ScopeId,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(), TyperError> {
        for site in self.index.export_sites_for_owner(owner) {
            if site.source != self.source || site.extension_prefix_clauses.is_some() {
                continue;
            }
            let TreeKind::Export(export) = &self.arena.get(site.tree).kind else {
                continue;
            };
            let position = self.arena.get(site.tree).position;
            let Some(object) = self.resolve_qualifier_symbol(
                export.expr,
                site.context,
                site.tree.index(),
                position,
            )?
            else {
                continue;
            };
            if self.store.symbols.get(object).kind != SymbolKind::Object
                || !self.is_current_source_symbol(object)
                || !self
                    .store
                    .symbols
                    .owner(object)
                    .is_some_and(|p| self.store.symbols.get(p).kind == SymbolKind::Package)
            {
                continue;
            }
            let module = self.source_module_class_of_object(object)?;
            // The namer enters ordinary members, while completing the qualifier
            // enters its export forwarders. Both must be visible to this site.
            // ClassInfo is installed before exports. A back edge to an object
            // already being completed can therefore read its declaration scope.
            if !self.index.export_sites_for_owner(module).is_empty()
                && !matches!(self.store.symbols.info(module), SymbolInfo::Complete(_))
            {
                if self.export_qualifier_depth >= MAX_EXPORT_QUALIFIER_DEPTH {
                    return Err(TyperError::UnsupportedExpression {
                        source: self.source,
                        tree_index: site.tree.index(),
                        expression_kind: "source export qualifier completion depth limit",
                    });
                }
                self.export_qualifier_depth += 1;
                let completed = self.completed_expression_symbol_info(module, journal);
                self.export_qualifier_depth -= 1;
                completed?;
            }
            let members = self.synthetic_module_scope(module)?;
            let prefix =
                self.term_reference_type(object, None, owner, site.tree.index(), journal)?;
            let ancestors = self.contextual_ancestors(owner, journal)?;
            let mut ineligible_named = Vec::new();
            let mut eligible_named = HashSet::new();
            for ContextualRef { symbol: target, .. } in self.contextual_members(members, journal)? {
                let entry = self.store.symbols.get(target);
                if !entry
                    .owner
                    .is_some_and(|class| self.is_current_source_symbol(class))
                    || entry.visibility != Visibility::Public
                    || !matches!(entry.kind, SymbolKind::Field | SymbolKind::Method)
                {
                    continue;
                }
                let declaring_class = entry.owner;
                let original_name = entry.name;
                let stable = entry.kind == SymbolKind::Field
                    || matches!(entry.info, SymbolInfo::Complete(info) if self.store.export_target(target) == Some(info));
                let contextual_flags = entry.flags & (SymbolFlags::GIVEN | SymbolFlags::IMPLICIT);
                let evidence = ContextualRef {
                    symbol: target,
                    receiver: Some(ContextualReceiver::Path(prefix)),
                };
                let Some(selection) =
                    self.contextual_export_name(site, &export.selectors, evidence, journal)?
                else {
                    continue;
                };
                let shared = ancestors
                    .iter()
                    .any(|(class, _)| Some(*class) == declaring_class);
                if declaring_class == Some(owner) || (shared && self.concrete_export_member(target))
                {
                    if matches!(selection, ExportSelection::Named(_)) {
                        ineligible_named.push((original_name, target));
                    }
                    continue;
                }
                let name = match selection {
                    ExportSelection::Named(name) => {
                        eligible_named.insert(original_name);
                        name
                    }
                    ExportSelection::Wildcard(name) => name,
                };
                if !(self.store.symbols.get(target).flags
                    & (SymbolFlags::MUTABLE | SymbolFlags::INLINE | SymbolFlags::ERASED))
                    .is_empty()
                {
                    return Err(TyperError::UnsupportedExpression {
                        source: self.source,
                        tree_index: site.tree.index(),
                        expression_kind: "mutable, inline, or erased contextual export",
                    });
                }
                let result = self.term_reference_type(
                    target,
                    Some(prefix),
                    owner,
                    site.tree.index(),
                    journal,
                )?;
                let signature = if stable {
                    result
                } else {
                    let source = self.widen_expression_type_journaled(result, journal, 0)?;
                    self.export_provider_signature(source, site.tree.index())?
                };
                let decl = dotty_core::Symbol {
                    name,
                    owner: Some(owner),
                    kind: SymbolKind::Method,
                    flags: SymbolFlags::EXPORTED | SymbolFlags::FINAL | contextual_flags,
                    visibility: Visibility::Public,
                    info: SymbolInfo::Complete(signature),
                    origin: SymbolOrigin::Synthetic,
                    position,
                    annotations: vec![],
                    links: Default::default(),
                };
                self.store
                    .enter_export_forwarder(scope, decl, result)
                    .ok_or(TyperError::AmbiguousTermReference {
                        source: self.source,
                        tree_index: site.tree.index(),
                        name,
                        position,
                    })?;
            }
            if let Some((_, member)) = ineligible_named
                .into_iter()
                .find(|(name, _)| !eligible_named.contains(name))
            {
                return Err(TyperError::InheritedContextualExport {
                    source: self.source,
                    tree_index: site.tree.index(),
                    member,
                });
            }
        }
        Ok(())
    }

    fn concrete_export_member(&self, symbol: SymbolId) -> bool {
        // Source declarations without a body are deferred even without an explicit abstract modifier.
        if let Some(SourceDefinition::Canonical { tree, .. }) = self.index.definition_of(symbol) {
            match &self.arena.get(tree).kind {
                TreeKind::ValDef(def) => return def.rhs.is_some(),
                TreeKind::DefDef(def) => return def.rhs.is_some(),
                _ => {}
            }
        }
        !self
            .store
            .symbols
            .get(symbol)
            .flags
            .contains(SymbolFlags::ABSTRACT)
    }

    fn contextual_export_name(
        &mut self,
        site: &dotty_core::SourceExportSite,
        selectors: &[dotty_core::ast::ImportSelector<Untyped>],
        evidence: ContextualRef,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<ExportSelection>, TyperError> {
        let entry = self.store.symbols.get(evidence.symbol);
        let name = entry.name;
        let legacy = entry.flags.contains(SymbolFlags::IMPLICIT);
        if let Some(named) = selectors.iter().find(|s| {
            s.imported == name
                && (s.imported_backquoted
                    || !matches!(self.store.names.resolve(s.imported.text()), "*" | ""))
        }) {
            let Some(rename) = named.renamed else {
                return Ok(Some(ExportSelection::Named(name)));
            };
            let TreeKind::Ident(ident) = &self.arena.get(rename).kind else {
                return Err(TyperError::MalformedSourceImport {
                    source: self.source,
                    import_tree_index: site.tree.index(),
                });
            };
            let visible = ident.backquoted || self.store.names.resolve(ident.name.text()) != "_";
            return Ok(visible.then_some(ExportSelection::Named(ident.name)));
        }
        for selector in selectors.iter().filter(|s| !s.imported_backquoted) {
            let selected = match self.store.names.resolve(selector.imported.text()) {
                "*" => legacy,
                "" => match selector.bound {
                    None => true,
                    Some(bound) => self.contextual_selector_matches(
                        evidence,
                        bound,
                        site.context,
                        site.tree.index(),
                        journal,
                    )?,
                },
                _ => false,
            };
            if selected {
                return Ok(Some(ExportSelection::Wildcard(name)));
            }
        }
        Ok(None)
    }

    // A conditional given has a new binder even when the forwarder keeps its
    // original name. Substitute by binder identity in both bounds and result.
    fn export_provider_signature(
        &mut self,
        signature: TypeId,
        tree_index: u32,
    ) -> Result<TypeId, TyperError> {
        let mut body = match self.store.types.get(signature) {
            Type::Poly(poly) if !poly.params.is_empty() => poly.result,
            _ => signature,
        };
        let unsupported = || TyperError::UnsupportedExpression {
            source: self.source,
            tree_index,
            expression_kind: "unsupported contextual export parameter clauses",
        };
        let mut depth = 0;
        while let Type::Method(clause) = self.store.types.get(body) {
            if depth >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH
                || clause.kind == MethodKind::Plain
                || clause.params.is_empty()
                || clause.params.iter().any(|p| p.erased || p.varargs)
            {
                return Err(unsupported());
            }
            depth += 1;
            body = clause.result;
        }
        if depth == 0 || matches!(self.store.types.get(body), Type::Poly(_)) {
            return Err(unsupported());
        }
        freshen_type_binders(self.store, signature).map_err(|error| TyperError::TypeRebinding {
            source: self.source,
            tree_index,
            error,
        })
    }

    /// Materializes a source object's exported contextual method and its delegation
    /// in this typer’s arena. The symbol must be a completed export from the
    /// same semantic store. Repeated requests return the same typed definition.
    /// Unknown or unsupported forwarders return an error without publishing a tree.
    pub fn type_export_forwarder_definition(
        &mut self,
        method: SymbolId,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(&tree) = self.export_definitions.get(&method) {
            return Ok(tree);
        }
        let tree = self.run_atomic(|typer, _| typer.build_export_definition(method))?;
        self.export_definitions.insert(method, tree);
        Ok(tree)
    }

    fn build_export_definition(&mut self, method: SymbolId) -> Result<TreeId<Typed>, TyperError> {
        let invalid = TyperError::SourceProvenanceMissing { symbol: method };
        let Some(target) = self.store.export_target(method) else {
            return Err(invalid);
        };
        let Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(member),
        } = *self.store.types.get(target)
        else {
            return Err(invalid);
        };
        let Some(object) = self
            .store
            .types
            .get(prefix)
            .reference_symbol()
            .filter(|&id| self.store.symbols.contains(id))
        else {
            return Err(invalid);
        };
        let entry = self.store.symbols.get(method);
        let position = entry.position;
        let name = dotty_core::TermName::new(entry.name.text());
        let mut builder = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types);
        let receiver = builder.ident(self.store.symbols.get(object).name, prefix, position);
        let rhs = builder.select(
            receiver,
            self.store.symbols.get(member).name,
            false,
            target,
            position,
        );
        let body = if entry.info == SymbolInfo::Complete(target) {
            ExportBody {
                rhs,
                type_params: vec![],
                value_param_clauses: vec![],
            }
        } else {
            self.build_export_provider_body(method, rhs)?
        };
        let result = self.typed_arena.get(body.rhs).ty;
        let tpt = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .type_tree(result, position);
        let ty = self
            .store
            .types
            .alloc(Type::term_ref(self.definitions.no_prefix, method));
        Ok(self.typed_index.alloc_synthetic_definition(
            &mut self.typed_arena,
            method,
            Tree {
                ty,
                position,
                kind: TreeKind::DefDef(DefDef {
                    name,
                    type_params: body.type_params,
                    value_param_clauses: body.value_param_clauses,
                    source_param_clause_order: None,
                    tpt,
                    rhs: Some(body.rhs),
                    metadata: (),
                }),
            },
        ))
    }

    fn export_parameter(
        &mut self,
        method: SymbolId,
        name: Name,
        kind: SymbolKind,
        flags: SymbolFlags,
        info: SymbolInfo,
    ) -> SymbolId {
        let position = self.store.symbols.get(method).position;
        self.store.symbols.alloc(dotty_core::Symbol {
            name,
            owner: Some(method),
            kind,
            flags,
            visibility: Visibility::Public,
            info,
            origin: SymbolOrigin::Synthetic,
            position,
            annotations: vec![],
            links: Default::default(),
        })
    }

    fn export_type_application(
        &mut self,
        method: SymbolId,
        body: &mut ExportBody,
        signature: TypeId,
    ) -> Result<TypeId, TyperError> {
        let Some(Type::Poly(poly)) = self.store.types.try_get(signature) else {
            return Ok(signature);
        };
        let params = poly.params.clone();
        let position = self.store.symbols.get(method).position;
        let mut declarations = Vec::with_capacity(params.len());
        let mut args = Vec::with_capacity(params.len());
        // Bounds can refer to other parameters of this same binder.
        for param in params {
            let symbol = self.export_parameter(
                method,
                *param.name.as_name(),
                SymbolKind::TypeParameter,
                SymbolFlags::EMPTY,
                SymbolInfo::Missing,
            );
            let arg = self
                .store
                .types
                .alloc(Type::type_ref(self.definitions.no_prefix, symbol));
            declarations.push((param, symbol, arg));
            args.push(arg);
        }
        let instance = instantiate_poly(self.store, signature, &args)
            .map_err(|_| TyperError::SourceProvenanceMissing { symbol: method })?;
        let mut type_args = Vec::with_capacity(args.len());
        for ((param, symbol, arg), bound) in declarations.into_iter().zip(instance.bounds) {
            self.store
                .symbols
                .set_info(symbol, SymbolInfo::Complete(bound));
            let mut builder = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types);
            let bound_tree = builder.type_tree(bound, position);
            type_args.push(builder.type_tree(arg, position));
            let definition = self.typed_index.alloc_synthetic_definition(
                &mut self.typed_arena,
                symbol,
                Tree {
                    ty: arg,
                    position,
                    kind: TreeKind::TypeDef(TypeDef {
                        name: param.name,
                        rhs: bound_tree,
                        metadata: (),
                        variance: param.declared_variance,
                    }),
                },
            );
            body.type_params.push(definition);
        }
        body.rhs = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).type_apply(
            body.rhs,
            type_args,
            instance.result,
            position,
        );
        Ok(instance.result)
    }

    fn build_export_provider_body(
        &mut self,
        method: SymbolId,
        select: TreeId<Typed>,
    ) -> Result<ExportBody, TyperError> {
        let invalid = || TyperError::SourceProvenanceMissing { symbol: method };
        let SymbolInfo::Complete(signature) = *self.store.symbols.info(method) else {
            return Err(invalid());
        };
        let mut body = ExportBody {
            rhs: select,
            type_params: Vec::new(),
            value_param_clauses: Vec::new(),
        };
        let mut callable = self.export_type_application(method, &mut body, signature)?;
        while let Type::Method(clause) = self.store.types.get(callable) {
            if body.value_param_clauses.len() >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
                return Err(invalid());
            }
            let clause = clause.clone();
            callable = self.export_value_application(method, &mut body, callable, clause)?;
        }
        if body.value_param_clauses.is_empty() {
            return Err(invalid());
        }
        Ok(body)
    }

    fn export_value_application(
        &mut self,
        method: SymbolId,
        body: &mut ExportBody,
        callable: TypeId,
        clause: dotty_core::types::MethodType,
    ) -> Result<TypeId, TyperError> {
        let position = self.store.symbols.get(method).position;
        let flags = match clause.kind {
            MethodKind::Contextual => SymbolFlags::GIVEN,
            MethodKind::Implicit => SymbolFlags::IMPLICIT,
            MethodKind::Plain => {
                return Err(TyperError::SourceProvenanceMissing { symbol: method });
            }
        };
        let mut params = Vec::new();
        let mut paths = Vec::new();
        let mut arguments = Vec::new();
        for formal in clause.params {
            // Later formals can select a type on one of the parameters we
            // just allocated for this forwarder.
            let ty = if self.type_contains_param_ref(formal.ty, callable)? {
                dotty_core::types::instantiate_next_method_parameter(self.store, callable, &paths)
                    .map_err(|_| TyperError::SourceProvenanceMissing { symbol: method })?
            } else {
                formal.ty
            };
            let witness = self.export_parameter(
                method,
                *formal.name.as_name(),
                SymbolKind::Parameter,
                flags,
                SymbolInfo::Complete(ty),
            );
            let path = self
                .store
                .types
                .alloc(Type::term_ref(self.definitions.no_prefix, witness));
            let mut builder = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types);
            let tpt = builder.type_tree(ty, position);
            let ref_tree = builder.ident(*formal.name.as_name(), path, position);

            let param = self.typed_index.alloc_synthetic_definition(
                &mut self.typed_arena,
                witness,
                Tree {
                    ty,
                    position,
                    kind: TreeKind::ValDef(ValDef {
                        name: formal.name,
                        tpt,
                        rhs: None,
                        metadata: (),
                    }),
                },
            );
            params.push(param);
            paths.push(Some(path));
            arguments.push(ref_tree);
        }
        let result = if self.type_contains_param_ref(clause.result, callable)? {
            instantiate_method_result(self.store, callable, &paths)
                .map_err(|_| TyperError::SourceProvenanceMissing { symbol: method })?
        } else {
            clause.result
        };
        body.rhs = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .apply(body.rhs, arguments, result, position);
        body.value_param_clauses.push(params);
        Ok(result)
    }
}
