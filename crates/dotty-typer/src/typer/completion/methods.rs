//! Completion of source method signatures and inferred method results.

use super::*;

impl SourceTyper<'_> {
    fn tree_qualified_name(&self, tree: TreeId<Untyped>) -> Option<Vec<String>> {
        match &self.arena.try_get(tree)?.kind {
            TreeKind::Ident(ident) => {
                Some(vec![self.store.names.resolve(ident.name.text()).to_owned()])
            }
            TreeKind::Select(select) => {
                let mut path = self.tree_qualified_name(select.qualifier)?;
                path.push(self.store.names.resolve(select.name.text()).to_owned());
                Some(path)
            }
            _ => None,
        }
    }

    fn uses_scala_2_13_case_class_unapply(&self, context: SourceContextId) -> bool {
        let mut current = Some(context);
        while let Some(context_id) = current {
            let Some(source_context) = self.index.try_source_context(context_id) else {
                return false;
            };
            if let Some(import_tree) = source_context.import
                && let Some(TreeKind::Import(import)) =
                    self.arena.try_get(import_tree).map(|tree| &tree.kind)
            {
                let path = self.tree_qualified_name(import.expr);
                let whole_feature_import = import.selectors.is_empty()
                    && path.as_deref().is_some_and(|path| {
                        path.iter()
                            .map(String::as_str)
                            .eq(["scala", "language", "2.13"])
                    });
                let selected_feature_import = path
                    .as_deref()
                    .is_some_and(|path| path.iter().map(String::as_str).eq(["scala", "language"]))
                    && matches!(import.selectors.as_slice(), [selector]
                    if self.store.names.resolve(selector.imported.text()) == "2.13"
                        && selector.renamed.is_none()
                        && selector.bound.is_none());
                if whole_feature_import || selected_feature_import {
                    return true;
                }
            }
            current = source_context.parent;
        }
        false
    }

    fn canonical_scala_option(&mut self, tree_index: u32) -> Result<Option<SymbolId>, TyperError> {
        let scala = match self.packages.symbol(&["scala"]) {
            Some(scala) => Some(scala),
            None => self.resolve_external_package(&["scala".to_owned()], tree_index)?,
        };
        let Some(scala) = scala else {
            return Ok(None);
        };
        if self.package_path(scala) != ["scala"] {
            return Err(TyperError::SymbolResolution {
                source: self.source,
                tree_index,
                error: dotty_core::ResolutionError::Malformed {
                    reason: "scala package resolution returned a non-canonical package".to_owned(),
                },
            });
        }
        let name = Name::new(
            self.store.names.intern("Option"),
            dotty_core::Namespace::Type,
        );
        let scoped = match self.packages.scope_of(scala) {
            Some(scope) => {
                let candidates = self
                    .store
                    .scopes
                    .get(scope)
                    .lookup_all(&name)
                    .iter()
                    .copied()
                    .filter(|symbol| self.store.symbols.get(*symbol).kind == SymbolKind::Class)
                    .collect::<Vec<_>>();
                match candidates.as_slice() {
                    [option] => Some(*option),
                    [] => None,
                    _ => {
                        return Err(TyperError::SymbolResolution {
                            source: self.source,
                            tree_index,
                            error: dotty_core::ResolutionError::Ambiguous {
                                candidates: candidates.len(),
                            },
                        });
                    }
                }
            }
            None => None,
        };
        let option = if let Some(option) = scoped {
            Some(option)
        } else {
            let request = dotty_core::MemberRequest {
                prefix: self.package_type_prefix(scala),
                name,
                selector: dotty_core::MemberSelector::Unique,
                space: dotty_core::MemberSpace::Prefix,
            };
            self.resolver
                .resolve_member(self.store, &request)
                .map_err(|error| TyperError::SymbolResolution {
                    source: self.source,
                    tree_index,
                    error,
                })?
        };
        Ok(option)
    }

    pub(in crate::typer) fn is_deferred_case_class_companion_apply(
        &self,
        name: Name,
        symbol: SymbolId,
    ) -> bool {
        self.store.names.resolve(name.text()) == "apply"
            && self.store.symbols.contains(symbol)
            && self.store.symbols.get(symbol).kind == SymbolKind::Method
            && self
                .store
                .symbols
                .get(symbol)
                .owner
                .is_some_and(|owner| self.deferred_case_class_for_module_class(owner).is_some())
    }

    pub(in crate::typer) fn deferred_case_class_for_module_class(
        &self,
        module_class: SymbolId,
    ) -> Option<SymbolId> {
        if !self.store.symbols.contains(module_class) {
            return None;
        }
        let module = self.store.symbols.get(module_class);
        if module.kind != SymbolKind::ModuleClass
            || !(matches!(
                module.origin,
                SymbolOrigin::Source(source) if source == self.source
            ) || module.origin == SymbolOrigin::Synthetic)
        {
            return None;
        }
        let SourceDefinition::Derived { source, tree } = self.index.definition_of(module_class)?
        else {
            return None;
        };
        if source != self.source {
            return None;
        }
        let declaration = self.index.symbol_at(source, tree)?;
        if !self.store.symbols.contains(declaration) {
            return None;
        }
        let declaration_symbol = self.store.symbols.get(declaration);
        let (class, object) = match declaration_symbol.kind {
            SymbolKind::Class => (declaration, declaration_symbol.links.companion?),
            SymbolKind::Object => (declaration_symbol.links.companion?, declaration),
            _ => return None,
        };
        if !self.store.symbols.contains(class) || !self.store.symbols.contains(object) {
            return None;
        }
        let object_symbol = self.store.symbols.get(object);
        if object_symbol.kind != SymbolKind::Object
            || object_symbol.owner != declaration_symbol.owner
            || !(matches!(
                object_symbol.origin,
                SymbolOrigin::Source(object_source) if object_source == source
            ) || object_symbol.origin == SymbolOrigin::Synthetic)
            || self
                .index
                .derived_symbol_at(declaration_symbol.owner?, source, tree)
                != Some(module_class)
        {
            return None;
        }
        let class_symbol = self.store.symbols.get(class);
        (class_symbol.kind == SymbolKind::Class
            && class_symbol.flags.contains(SymbolFlags::CASE)
            && class_symbol
                .flags
                .contains(SymbolFlags::CASE_CLASS_APPLY_DEFERRED)
            && class_symbol.origin == SymbolOrigin::Source(source)
            && class_symbol.links.companion == Some(object))
        .then_some(class)
    }

    pub(super) fn generated_case_class_for_apply(
        &self,
        method: SymbolId,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        let method_symbol = self.store.symbols.get(method);
        let owner = method_symbol.owner?;
        if source != self.source
            || method_symbol.kind != SymbolKind::Method
            || !method_symbol.flags.contains(SymbolFlags::SYNTHETIC)
            || self.store.names.resolve(method_symbol.name.text()) != "apply"
            || method_symbol.name.namespace() != dotty_core::Namespace::Term
            || method_symbol.origin != SymbolOrigin::Source(source)
            || self.store.symbols.get(owner).kind != SymbolKind::ModuleClass
            || self.index.definition_of(method) != Some(SourceDefinition::Derived { source, tree })
            || self.index.derived_symbol_at(owner, source, tree) != Some(method)
        {
            return None;
        }
        let class = self.index.symbol_at(source, tree)?;
        if self.index.definition_of(class) != Some(SourceDefinition::Canonical { source, tree }) {
            return None;
        }
        let class_symbol = self.store.symbols.get(class);
        if class_symbol.kind != SymbolKind::Class
            || !class_symbol.flags.contains(SymbolFlags::CASE)
            || class_symbol.origin != SymbolOrigin::Source(source)
        {
            return None;
        }
        let object = class_symbol.links.companion?;
        if !self.store.symbols.contains(object) {
            return None;
        }
        let object_symbol = self.store.symbols.get(object);
        if object_symbol.kind != SymbolKind::Object
            || object_symbol.owner != class_symbol.owner
            || object_symbol.links.companion != Some(class)
            || self.source_module_class_of_object(object).ok()? != owner
            || !matches!(
                self.arena.try_get(tree).map(|node| &node.kind),
                Some(TreeKind::TypeDef(definition))
                    if definition.metadata.modifiers.contains(&dotty_core::ast::Modifier::Case)
            )
        {
            return None;
        }
        Some(class)
    }

    pub(super) fn generated_case_class_for_unapply(
        &self,
        method: SymbolId,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        let method_symbol = self.store.symbols.get(method);
        let owner = method_symbol.owner?;
        let identity = self.index.generated_case_class_method_identity(method)?;
        if !self.store.symbols.contains(owner)
            || !self.store.symbols.contains(identity.class)
            || source != self.source
            || identity.role != dotty_core::GeneratedCaseClassMethodRole::Unapply
            || method_symbol.kind != SymbolKind::Method
            || !method_symbol.flags.contains(SymbolFlags::SYNTHETIC)
            || self.store.names.resolve(method_symbol.name.text()) != "unapply"
            || method_symbol.name.namespace() != dotty_core::Namespace::Term
            || method_symbol.origin != SymbolOrigin::Source(source)
            || self.store.symbols.get(owner).kind != SymbolKind::ModuleClass
            || self.index.definition_of(method) != Some(SourceDefinition::Derived { source, tree })
            || self.index.symbol_at(source, tree) != Some(identity.class)
            || self.index.generated_case_class_method(
                identity.class,
                dotty_core::GeneratedCaseClassMethodRole::Unapply,
            ) != Some(method)
        {
            return None;
        }
        let class_symbol = self.store.symbols.get(identity.class);
        if class_symbol.kind != SymbolKind::Class
            || !class_symbol.flags.contains(SymbolFlags::CASE)
            || class_symbol.origin != SymbolOrigin::Source(source)
            || !matches!(
                self.index.definition_of(identity.class),
                Some(SourceDefinition::Canonical { source: class_source, tree: class_tree })
                    if class_source == source && class_tree == tree
            )
            || class_symbol.links.companion.is_none()
            || !matches!(
                self.arena.try_get(tree).map(|node| &node.kind),
                Some(TreeKind::TypeDef(definition))
                    if definition.metadata.modifiers.contains(&dotty_core::ast::Modifier::Case)
            )
        {
            return None;
        }
        let companion = class_symbol.links.companion?;
        if !self.store.symbols.contains(companion)
            || self.store.symbols.get(companion).kind != SymbolKind::Object
            || !(self.store.symbols.get(companion).origin == SymbolOrigin::Synthetic
                || self.store.symbols.get(companion).origin == SymbolOrigin::Source(source))
            || self.store.symbols.get(companion).owner != class_symbol.owner
            || self.store.symbols.get(companion).links.companion != Some(identity.class)
            || self.source_module_class_of_object(companion).ok()? != owner
        {
            return None;
        }
        Some(identity.class)
    }

    pub(super) fn complete_generated_case_class_unapply_signature(
        &mut self,
        method: SymbolId,
        class: SymbolId,
        source: SourceId,
        class_tree: TreeId<Untyped>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let class_info = match *self.store.symbols.info(class) {
            SymbolInfo::Complete(info) => info,
            SymbolInfo::Missing => self.complete_symbol_inner(class, info_journal)?,
            SymbolInfo::Deferred(_) => {
                return Err(TyperError::DeferredSymbolCompletion { symbol: class });
            }
            SymbolInfo::Error => {
                return Err(TyperError::SymbolAlreadyErrored { symbol: class });
            }
        };
        self.validate_existing_class_info(class, class_info)?;
        self.validate_completed_source_case_class_product1(class, class_info, info_journal)?;

        let Some(TreeKind::TypeDef(class_definition)) =
            self.arena.try_get(class_tree).map(|node| &node.kind)
        else {
            return Err(TyperError::SymbolSourceKindMismatch {
                source,
                tree_index: class_tree.index(),
                symbol: method,
                kind: SymbolKind::Method,
            });
        };
        let Some(TreeKind::Template(template)) = self
            .arena
            .try_get(class_definition.rhs)
            .map(|node| &node.kind)
        else {
            return Err(TyperError::MalformedMethodClause {
                method,
                method_tree_index: class_tree.index(),
                clause_index: 0,
            });
        };
        let Some(TreeKind::DefDef(constructor)) = self
            .arena
            .try_get(template.constructor)
            .map(|node| &node.kind)
        else {
            return Err(TyperError::MalformedMethodClause {
                method,
                method_tree_index: class_tree.index(),
                clause_index: 0,
            });
        };
        let class_parameters = self.source_class_type_parameters(class)?;
        if !matches!(class_parameters.len(), 0 | 2)
            || constructor.type_params.len() != class_parameters.len()
        {
            return Err(TyperError::MalformedMethodClause {
                method,
                method_tree_index: class_tree.index(),
                clause_index: 0,
            });
        }
        let Some(parameter_trees) = constructor
            .value_param_clauses
            .as_slice()
            .first()
            .filter(|_| constructor.value_param_clauses.len() == 1)
        else {
            return Err(TyperError::MalformedMethodClause {
                method,
                method_tree_index: class_tree.index(),
                clause_index: 0,
            });
        };
        let expected_field_types = match parameter_trees.as_slice() {
            [_] if class_parameters.is_empty() => vec![self.definitions.int],
            [_] if class_parameters.len() == 2 => vec![self.definitions.any_type],
            [_, _] => vec![self.definitions.int, self.definitions.boolean],
            _ => {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index: class_tree.index(),
                    clause_index: 0,
                });
            }
        };
        if parameter_trees.len() == 2 && parameter_trees[0] == parameter_trees[1] {
            return Err(TyperError::MalformedMethodClause {
                method,
                method_tree_index: class_tree.index(),
                clause_index: 0,
            });
        }
        let mut two_reference_fields = false;
        let mut authenticated_reference_parent = None;
        let mut authenticated_fields = Vec::with_capacity(parameter_trees.len());
        let mut generic_field_parameter = None;
        for (field_index, (parameter_tree, expected_type)) in parameter_trees
            .iter()
            .copied()
            .zip(expected_field_types)
            .enumerate()
        {
            let Some(TreeKind::ValDef(parameter)) =
                self.arena.try_get(parameter_tree).map(|node| &node.kind)
            else {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index: class_tree.index(),
                    clause_index: 0,
                });
            };
            let field = self.index.symbol_at(source, parameter_tree).ok_or(
                TyperError::MethodParameterSymbolMissing {
                    method,
                    parameter_tree_index: parameter_tree.index(),
                },
            )?;
            if !self.store.symbols.contains(field)
                || authenticated_fields.contains(&field)
                || self.store.symbols.get(field).kind != SymbolKind::Field
                || !self.store.symbols.get(field).flags.is_empty()
                || self.store.symbols.get(field).owner != Some(class)
                || self.store.symbols.get(field).origin != SymbolOrigin::Source(source)
                || self.index.definition_of(field)
                    != Some(SourceDefinition::Canonical {
                        source,
                        tree: parameter_tree,
                    })
                || parameter.rhs.is_some()
                || parameter.metadata.visibility.is_some()
                || parameter
                    .metadata
                    .modifiers
                    .iter()
                    .any(|modifier| *modifier != dotty_core::ast::Modifier::ParamAccessor)
            {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index: class_tree.index(),
                    clause_index: 0,
                });
            }
            authenticated_fields.push(field);
            let parameter_context = self
                .parameter_source_context(field)
                .ok_or(TyperError::DeclarationContextMissing { symbol: field })?;
            let field_type =
                self.type_of_tpt_inner_journaled(parameter.tpt, parameter_context, info_journal)?;
            // Product1 keeps its existing reference exception. Product2 accepts
            // references only as a homogeneous pair authenticated against one
            // canonical direct source-trait parent.
            if parameter_trees.len() == 2 && field_index == 0 {
                two_reference_fields = field_type != expected_type;
            }
            let reference_field_candidate = if parameter_trees.len() == 1 {
                field_type != expected_type
                    && expected_type == self.definitions.int
                    && template.parents.len() == 1
            } else {
                two_reference_fields && template.parents.len() == 1
            };
            let supported_generic_field = class_parameters.len() == 2
                && parameter_trees.len() == 1
                && matches!(
                    self.store.types.get(field_type),
                    Type::TypeRef { target: TypeRefTarget::Symbol(parameter), .. }
                        if class_parameters.contains(parameter)
                            && self.store.symbols.get(*parameter).owner == Some(class)
                );
            if supported_generic_field
                && let Type::TypeRef {
                    target: TypeRefTarget::Symbol(parameter),
                    ..
                } = self.store.types.get(field_type)
            {
                generic_field_parameter = Some(*parameter);
            }
            let supported_reference_field = if reference_field_candidate {
                let parent_tree = template.parents[0];
                let parent_type =
                    self.project_parent_type(parent_tree, parameter_context, 0, info_journal)?;
                let parent = self.validate_source_case_class_trait_parent(
                    class,
                    parent_tree,
                    parent_type,
                    info_journal,
                )?;
                let same_parent = authenticated_reference_parent
                    .is_none_or(|authenticated| authenticated == parent);
                if parameter_trees.len() == 2 {
                    authenticated_reference_parent = Some(parent);
                }
                same_parent && self.is_canonical_source_trait_ref(field_type, parent)
            } else {
                false
            };
            let supported_field_type = supported_generic_field
                || if parameter_trees.len() == 2 && two_reference_fields {
                    supported_reference_field
                } else {
                    field_type == expected_type || supported_reference_field
                };
            if !supported_field_type {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index: class_tree.index(),
                    clause_index: 0,
                });
            }
            if parameter_trees.len() == 2 && two_reference_fields {
                match *self.store.symbols.info(field) {
                    // Source fields remain lazily completed for this generated
                    // signature. If another path has already supplied field
                    // info, it must agree with the independently projected
                    // canonical parent reference authenticated above.
                    SymbolInfo::Missing => {}
                    SymbolInfo::Complete(info) if info == field_type => {}
                    SymbolInfo::Complete(_) => {
                        return Err(TyperError::MalformedMethodClause {
                            method,
                            method_tree_index: class_tree.index(),
                            clause_index: 0,
                        });
                    }
                    SymbolInfo::Deferred(_) => {
                        return Err(TyperError::DeferredSymbolCompletion { symbol: field });
                    }
                    SymbolInfo::Error => {
                        return Err(TyperError::SymbolAlreadyErrored { symbol: field });
                    }
                }
            }
        }

        let input = self
            .index
            .derived_symbol_at(method, source, class_tree)
            .ok_or(TyperError::MethodParameterSymbolMissing {
                method,
                parameter_tree_index: class_tree.index(),
            })?;
        if !self.store.symbols.contains(input) {
            return Err(TyperError::MethodParameterSymbolMissing {
                method,
                parameter_tree_index: class_tree.index(),
            });
        }
        let input_symbol = self.store.symbols.get(input);
        let input_name = input_symbol.name;
        if input_symbol.kind != SymbolKind::Parameter
            || input_symbol.owner != Some(method)
            || input_symbol.name.namespace() != dotty_core::Namespace::Term
            || self.store.names.resolve(input_symbol.name.text()) != "x$1"
            || !input_symbol.flags.contains(SymbolFlags::SYNTHETIC)
            || self.index.definition_of(input)
                != Some(SourceDefinition::Derived {
                    source,
                    tree: class_tree,
                })
        {
            return Err(TyperError::MethodParameterSymbolMissing {
                method,
                parameter_tree_index: class_tree.index(),
            });
        }
        let prefix = self.type_symbol_prefix(class);
        let box_tycon = self.store.types.alloc(Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(class),
        });
        let box_type = if class_parameters.is_empty() {
            box_tycon
        } else {
            let arguments = class_parameters
                .iter()
                .map(|parameter| {
                    self.store.types.alloc(Type::TypeRef {
                        prefix: self.definitions.no_prefix,
                        target: TypeRefTarget::Symbol(*parameter),
                    })
                })
                .collect::<Vec<_>>();
            self.store.types.alloc(Type::Applied {
                tycon: box_tycon,
                args: arguments,
            })
        };
        let parameter_name = dotty_core::TermName::new(input_name.text());
        let compatibility_field_parameter = if let Some(field_parameter) = generic_field_parameter {
            let context = self
                .index
                .declaration_context_of(class)
                .ok_or(TyperError::DeclarationContextMissing { symbol: class })?;
            self.uses_scala_2_13_case_class_unapply(context)
                .then_some(field_parameter)
        } else {
            None
        };
        let result_type = if let Some(field_parameter) = compatibility_field_parameter {
            let option = self.canonical_scala_option(class_tree.index())?.ok_or(
                TyperError::TypeNameNotFound {
                    source,
                    tree_index: class_tree.index(),
                    name: Name::new(
                        self.store.names.intern("Option"),
                        dotty_core::Namespace::Type,
                    ),
                    position: None,
                },
            )?;
            if !self.store.symbols.contains(option)
                || self.store.symbols.get(option).kind != SymbolKind::Class
                || self.store.symbols.get(option).owner != self.packages.symbol(&["scala"])
            {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index: class_tree.index(),
                    clause_index: 0,
                });
            }
            let option_parameters = if self.is_current_source_symbol(option) {
                self.source_class_type_parameters(option)?
            } else {
                self.external_class_type_parameters(option).ok_or(
                    TyperError::MalformedMethodClause {
                        method,
                        method_tree_index: class_tree.index(),
                        clause_index: 0,
                    },
                )?
            };
            if option_parameters.len() != 1 {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index: class_tree.index(),
                    clause_index: 0,
                });
            }
            let option_prefix = self.type_symbol_prefix(option);
            let option_tycon = self.store.types.alloc(Type::TypeRef {
                prefix: option_prefix,
                target: TypeRefTarget::Symbol(option),
            });
            let field = self.store.types.alloc(Type::TypeRef {
                prefix: self.definitions.no_prefix,
                target: TypeRefTarget::Symbol(field_parameter),
            });
            self.store.types.alloc(Type::Applied {
                tycon: option_tycon,
                args: vec![field],
            })
        } else {
            box_type
        };
        let signature = method_type_from_symbols(
            self.store,
            &[MethodParamSpec {
                symbol: input,
                name: parameter_name,
                ty: box_type,
                erased: false,
                varargs: false,
            }],
            result_type,
            MethodKind::Plain,
        )
        .map_err(|error| TyperError::TypeRebinding {
            source,
            tree_index: class_tree.index(),
            error,
        })?;
        let previous = *self.store.symbols.info(input);
        info_journal.push((input, previous));
        self.store
            .symbols
            .set_info(input, SymbolInfo::Complete(box_type));
        if class_parameters.is_empty() {
            Ok(signature)
        } else {
            let mut type_parameters = Vec::with_capacity(class_parameters.len());
            for parameter in class_parameters {
                let bounds = self.complete_signature_parameter(parameter, info_journal)?;
                type_parameters.push(TypeParamSpec {
                    symbol: parameter,
                    name: dotty_core::TypeName::new(self.store.symbols.get(parameter).name.text()),
                    bounds,
                    declared_variance: None,
                });
            }
            poly_type_from_symbols(self.store, &type_parameters, signature).map_err(|error| {
                TyperError::TypeRebinding {
                    source,
                    tree_index: class_tree.index(),
                    error,
                }
            })
        }
    }

    pub(super) fn complete_generated_case_class_apply_signature(
        &mut self,
        method: SymbolId,
        class: SymbolId,
        source: SourceId,
        class_tree: TreeId<Untyped>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let Some(TreeKind::TypeDef(class_definition)) =
            self.arena.try_get(class_tree).map(|node| &node.kind)
        else {
            return Err(TyperError::SymbolSourceKindMismatch {
                source,
                tree_index: class_tree.index(),
                symbol: method,
                kind: SymbolKind::Method,
            });
        };
        let Some(TreeKind::Template(template)) = self
            .arena
            .try_get(class_definition.rhs)
            .map(|node| &node.kind)
        else {
            return Err(TyperError::MalformedMethodClause {
                method,
                method_tree_index: class_tree.index(),
                clause_index: 0,
            });
        };
        let Some(TreeKind::DefDef(constructor)) = self
            .arena
            .try_get(template.constructor)
            .map(|node| &node.kind)
        else {
            return Err(TyperError::MalformedMethodClause {
                method,
                method_tree_index: class_tree.index(),
                clause_index: 0,
            });
        };
        if constructor.value_param_clauses.len() != 1 {
            return Err(TyperError::MalformedMethodClause {
                method,
                method_tree_index: class_tree.index(),
                clause_index: 0,
            });
        }

        let type_parameters = self.type_parameter_specs(
            method,
            &constructor.type_params,
            true,
            info_journal,
            class_tree.index(),
            0,
        )?;
        let (value_parameters, kind) = self.method_parameter_specs(
            method,
            &constructor.value_param_clauses[0],
            true,
            class_tree.index(),
            usize::from(!type_parameters.is_empty()),
            info_journal,
        )?;
        if kind != MethodKind::Plain {
            return Err(TyperError::MalformedMethodClause {
                method,
                method_tree_index: class_tree.index(),
                clause_index: usize::from(!type_parameters.is_empty()),
            });
        }

        let class_owner = self
            .store
            .symbols
            .get(class)
            .owner
            .ok_or(TyperError::DeclarationContextMissing { symbol: class })?;
        let prefix = self
            .store
            .types
            .alloc(Type::ThisType { class: class_owner });
        let tycon = self.store.types.alloc(Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(class),
        });
        let arguments = type_parameters
            .iter()
            .map(|parameter| {
                self.store.types.alloc(Type::TypeRef {
                    prefix: self.definitions.no_prefix,
                    target: TypeRefTarget::Symbol(parameter.symbol),
                })
            })
            .collect::<Vec<_>>();
        let result = if arguments.is_empty() {
            tycon
        } else {
            self.store.types.alloc(Type::Applied {
                tycon,
                args: arguments,
            })
        };
        let method_type = method_type_from_symbols(self.store, &value_parameters, result, kind)
            .map_err(|error| TyperError::TypeRebinding {
                source,
                tree_index: class_tree.index(),
                error,
            })?;
        if type_parameters.is_empty() {
            Ok(method_type)
        } else {
            poly_type_from_symbols(self.store, &type_parameters, method_type).map_err(|error| {
                TyperError::TypeRebinding {
                    source,
                    tree_index: class_tree.index(),
                    error,
                }
            })
        }
    }

    pub(super) fn complete_method_signature(
        &mut self,
        method: SymbolId,
        method_tree_index: u32,
        definition: &dotty_core::ast::DefDef<Untyped>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let method_name = self.store.names.resolve(definition.name.as_name().text());
        let is_extension = self
            .store
            .symbols
            .get(method)
            .flags
            .contains(SymbolFlags::EXTENSION);
        if is_extension && method_name.ends_with(':') {
            return Err(TyperError::RightAssociativeExtensionDeferred {
                symbol: method,
                tree_index: method_tree_index,
            });
        }
        let Some(result_node) = self.arena.try_get(definition.tpt) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: definition.tpt.index(),
            });
        };
        let infer_result = matches!(result_node.kind, TreeKind::TypeTree(_));
        if infer_result && !self.inferred_method_results_in_progress.insert(method) {
            return Err(TyperError::RecursiveInferredMethodResult { symbol: method });
        }
        let result = self.complete_method_signature_body(
            method,
            method_tree_index,
            definition,
            infer_result,
            info_journal,
        );
        if infer_result {
            self.inferred_method_results_in_progress.remove(&method);
        }
        result
    }

    pub(super) fn complete_method_signature_body(
        &mut self,
        method: SymbolId,
        method_tree_index: u32,
        definition: &dotty_core::ast::DefDef<Untyped>,
        infer_result: bool,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let is_extension = self
            .store
            .symbols
            .get(method)
            .flags
            .contains(SymbolFlags::EXTENSION);
        let declaration_context = self
            .method_declaration_context(method)
            .ok_or(TyperError::DeclarationContextMissing { symbol: method })?;
        let mut clauses = Vec::new();
        let prefix_clauses = if is_extension {
            Some(
                self.extension_prefix_clauses(method)
                    .ok_or(TyperError::ExtensionPrefixClausesMissing { method })?
                    .to_vec(),
            )
        } else {
            None
        };
        let first_signature_parameter = prefix_clauses
            .as_ref()
            .and_then(|clauses| clauses.iter().flatten().next().map(|tree| (*tree, true)))
            .or_else(|| definition.type_params.first().map(|tree| (*tree, false)))
            .or_else(|| {
                definition
                    .value_param_clauses
                    .iter()
                    .flatten()
                    .next()
                    .map(|tree| (*tree, false))
            });
        let signature_parameter_context = if let Some((tree, derived)) = first_signature_parameter {
            let parameter = self.method_parameter_symbol(method, tree, derived)?;
            self.parameter_source_context(parameter)
                .ok_or(TyperError::DeclarationContextMissing { symbol: parameter })?
        } else {
            declaration_context.lexical
        };
        if let Some(prefix_clauses) = prefix_clauses {
            for (clause_index, trees) in prefix_clauses.iter().enumerate() {
                clauses.push(self.extension_prefix_clause(
                    method,
                    method_tree_index,
                    clause_index,
                    trees,
                    info_journal,
                )?);
            }
        }
        if !definition.type_params.is_empty() {
            clauses.push(MethodClauseSpec::Types(self.type_parameter_specs(
                method,
                &definition.type_params,
                false,
                info_journal,
                method_tree_index,
                clauses.len(),
            )?));
        }
        for trees in &definition.value_param_clauses {
            let clause_index = clauses.len();
            let (parameters, kind) = self.method_parameter_specs(
                method,
                trees,
                false,
                method_tree_index,
                clause_index,
                info_journal,
            )?;
            clauses.push(MethodClauseSpec::Terms(parameters, kind));
        }

        let Some(result_node) = self.arena.try_get(definition.tpt) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: definition.tpt.index(),
            });
        };
        let result_type = if infer_result {
            let rhs =
                definition
                    .rhs
                    .ok_or(TyperError::InferredMethodResultRightHandSideMissing {
                        symbol: method,
                        tree_index: method_tree_index,
                    })?;
            let context = self.expression_context_for(method)?;
            let mut new_mappings = Vec::new();
            let typed_rhs =
                self.type_value_expression_inner(rhs, context, info_journal, &mut new_mappings)?;
            let rhs_type = self.typed_arena.get(typed_rhs).ty;
            let inferred = self.widen_expression_type_journaled(rhs_type, info_journal, 0)?;
            self.validate_inferred_method_result(method, method_tree_index, inferred)?;
            inferred
        } else {
            if matches!(&result_node.kind, TreeKind::TypeTree(_)) {
                return Err(TyperError::MissingDeclaredType {
                    source: self.source,
                    tree_index: definition.tpt.index(),
                    position: result_node.position,
                });
            }
            self.type_of_tpt_inner_journaled(
                definition.tpt,
                signature_parameter_context,
                info_journal,
            )?
        };
        let mut signature = result_type;
        for clause in clauses.into_iter().rev() {
            signature = match clause {
                MethodClauseSpec::Types(parameters) => {
                    poly_type_from_symbols(self.store, &parameters, signature).map_err(|error| {
                        TyperError::TypeRebinding {
                            source: self.source,
                            tree_index: method_tree_index,
                            error,
                        }
                    })?
                }
                MethodClauseSpec::Terms(parameters, kind) => {
                    method_type_from_symbols(self.store, &parameters, signature, kind).map_err(
                        |error| TyperError::TypeRebinding {
                            source: self.source,
                            tree_index: method_tree_index,
                            error,
                        },
                    )?
                }
            };
        }
        Ok(signature)
    }

    pub(super) fn validate_inferred_method_result(
        &self,
        method: SymbolId,
        tree_index: u32,
        inferred: TypeId,
    ) -> Result<(), TyperError> {
        if matches!(
            self.store.types.try_get(inferred),
            None | Some(
                Type::NoType
                    | Type::NoPrefix
                    | Type::Error(_)
                    | Type::Bounds { .. }
                    | Type::AliasingBounds { .. }
                    | Type::ByName { .. }
                    | Type::Repeated { .. }
                    | Type::Method(_)
                    | Type::Poly(_)
                    | Type::TypeLambda(_)
                    | Type::RecThis { .. }
                    | Type::Wildcard { .. }
                    | Type::MatchCase { .. }
                    | Type::ClassInfo(_)
            )
        ) {
            return Err(TyperError::InvalidInferredMethodResult {
                symbol: method,
                tree_index,
                inferred,
            });
        }
        Ok(())
    }

    pub(super) fn extension_prefix_clause(
        &mut self,
        method: SymbolId,
        method_tree_index: u32,
        clause_index: usize,
        trees: &[TreeId<Untyped>],
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<MethodClauseSpec, TyperError> {
        let Some(first_tree) = trees.first() else {
            return Ok(MethodClauseSpec::Terms(Vec::new(), MethodKind::Plain));
        };
        let Some(first_node) = self.arena.try_get(*first_tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: first_tree.index(),
            });
        };
        match &first_node.kind {
            TreeKind::TypeDef(_) => Ok(MethodClauseSpec::Types(self.type_parameter_specs(
                method,
                trees,
                true,
                info_journal,
                method_tree_index,
                clause_index,
            )?)),
            TreeKind::ValDef(_) => {
                let (parameters, kind) = self.method_parameter_specs(
                    method,
                    trees,
                    true,
                    method_tree_index,
                    clause_index,
                    info_journal,
                )?;
                Ok(MethodClauseSpec::Terms(parameters, kind))
            }
            _ => Err(TyperError::MalformedMethodClause {
                method,
                method_tree_index,
                clause_index,
            }),
        }
    }

    pub(super) fn type_parameter_specs(
        &mut self,
        method: SymbolId,
        trees: &[TreeId<Untyped>],
        derived: bool,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        method_tree_index: u32,
        clause_index: usize,
    ) -> Result<Vec<TypeParamSpec>, TyperError> {
        let mut parameters = Vec::with_capacity(trees.len());
        for tree in trees {
            let Some(node) = self.arena.try_get(*tree) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: tree.index(),
                });
            };
            let TreeKind::TypeDef(definition) = &node.kind else {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            };
            let symbol = self.method_parameter_symbol(method, *tree, derived)?;
            if self.store.symbols.get(symbol).kind != SymbolKind::TypeParameter {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            }
            let bounds = self.complete_signature_parameter(symbol, info_journal)?;
            parameters.push(TypeParamSpec {
                symbol,
                name: definition.name,
                bounds,
                declared_variance: None,
            });
        }
        Ok(parameters)
    }

    pub(super) fn method_parameter_specs(
        &mut self,
        method: SymbolId,
        trees: &[TreeId<Untyped>],
        derived: bool,
        method_tree_index: u32,
        clause_index: usize,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(Vec<MethodParamSpec>, MethodKind), TyperError> {
        let mut parameters = Vec::with_capacity(trees.len());
        let mut clause_kind = None;
        for (parameter_index, tree) in trees.iter().enumerate() {
            let Some(node) = self.arena.try_get(*tree) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: tree.index(),
                });
            };
            let TreeKind::ValDef(definition) = &node.kind else {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            };
            let symbol = self.method_parameter_symbol(method, *tree, derived)?;
            if self.store.symbols.get(symbol).kind != SymbolKind::Parameter {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            }
            let flags = self.store.symbols.get(symbol).flags;
            let given = flags.contains(SymbolFlags::GIVEN);
            let implicit = flags.contains(SymbolFlags::IMPLICIT);
            if given && implicit {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            }
            let parameter_kind = if given {
                MethodKind::Contextual
            } else if implicit {
                MethodKind::Implicit
            } else {
                MethodKind::Plain
            };
            if clause_kind.is_some_and(|kind| kind != parameter_kind) {
                return Err(TyperError::MalformedMethodClause {
                    method,
                    method_tree_index,
                    clause_index,
                });
            }
            clause_kind = Some(parameter_kind);
            let repeated_parameter = matches!(
                self.arena.try_get(definition.tpt).map(|node| &node.kind),
                Some(TreeKind::PhaseSpecific(UntypedNode::PostfixOp(postfix)))
                    if self.store.names.resolve(postfix.op.text()) == "*"
            );
            if repeated_parameter && parameter_kind != MethodKind::Plain {
                return Err(TyperError::RepeatedParameterClauseUnsupported {
                    method,
                    method_tree_index,
                    clause_index,
                    parameter_index,
                    kind: parameter_kind,
                });
            }
            if repeated_parameter && parameter_index + 1 != trees.len() {
                return Err(TyperError::RepeatedParameterNotFinal {
                    method,
                    method_tree_index,
                    clause_index,
                    parameter_index,
                });
            }
            let completed_parameter_type =
                self.complete_method_parameter_for_signature(symbol, info_journal)?;
            let ty = if repeated_parameter {
                match self.store.types.try_get(completed_parameter_type) {
                    Some(Type::Repeated { element }) => *element,
                    _ => {
                        return Err(TyperError::MalformedMethodClause {
                            method,
                            method_tree_index,
                            clause_index,
                        });
                    }
                }
            } else {
                completed_parameter_type
            };
            parameters.push(MethodParamSpec {
                symbol,
                name: definition.name,
                ty,
                erased: flags.contains(SymbolFlags::ERASED),
                varargs: repeated_parameter,
            });
        }
        Ok((parameters, clause_kind.unwrap_or(MethodKind::Plain)))
    }

    pub(super) fn method_parameter_symbol(
        &self,
        method: SymbolId,
        tree: TreeId<Untyped>,
        derived: bool,
    ) -> Result<SymbolId, TyperError> {
        let symbol = if derived {
            self.index
                .derived_symbol_at(method, self.source, tree)
                .or_else(|| {
                    self.local_methods
                        .extension_receiver_parameter_symbol(method, tree)
                })
        } else {
            self.local_methods
                .type_parameter_symbol_at(self.source, tree)
                .or_else(|| self.local_methods.parameter_symbol_at(self.source, tree))
                .or_else(|| self.index.symbol_at(self.source, tree))
        };
        symbol.ok_or(TyperError::MethodParameterSymbolMissing {
            method,
            parameter_tree_index: tree.index(),
        })
    }
}
