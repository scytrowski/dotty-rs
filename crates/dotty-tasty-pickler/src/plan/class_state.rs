//! Read-only validation for ordinary class fields and primary constructor inputs.

use std::collections::HashSet;

use dotty_core::ast::{
    AstArena, DefParamClauseOrder, Modifier, NumberKind, Tree, TreeKind, Typed, Untyped,
    UntypedNode, VisibilitySyntax,
};
use dotty_core::{
    Constant, Definitions, MethodKind, Name, SemanticStore, SourceDefinition, SourceSemanticIndex,
    Symbol, SymbolFlags, SymbolId, SymbolInfo, SymbolKind, SymbolOrigin, TermRefTarget, TreeId,
    Type, TypeId, Visibility,
};

use crate::{
    Builtin, LimitKind, PickleError, SourceRole, SourceTypedBindings, TypeRole, UnsupportedFeature,
};

use super::{SourceMetadata, ValidationLimits};

/// Exact constructor input evidence for the bounded class-state planning API.
/// Parameters are canonical symbols owned by the primary constructor, in source
/// clause order; they are never inferred from names or derived class accessors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrimaryConstructorInputPlan {
    pub symbol: SymbolId,
    pub source_tree: TreeId<Untyped>,
    pub parameter_clauses: Vec<Vec<SymbolId>>,
}

/// A source field paired with the exact typed initializer root produced by the
/// frontend. Semantic facts are read from the canonical symbol and source tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClassFieldPicklePlan {
    pub symbol: SymbolId,
    pub source_tree: TreeId<Untyped>,
    pub typed_initializer: TreeId<Typed>,
    pub declared_type: TypeId,
}

/// Ordered class body entry. The explicit sequence is authoritative; the
/// declaration scope is checked as a set and is never used to create order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassMemberPicklePlan {
    Field(ClassFieldPicklePlan),
    Method {
        symbol: SymbolId,
        source_tree: TreeId<Untyped>,
    },
}

/// Additive, read-only class-state contract. This plan deliberately has no
/// serializer entry point yet: #1390 validates the producer facts needed by
/// subsequent field and constructor emission increments while preserving the
/// existing output path byte-for-byte.
pub struct ClassStatePicklePlan<'a> {
    pub store: &'a SemanticStore,
    pub definitions: &'a Definitions,
    pub source_ast: &'a AstArena<Untyped>,
    pub typed_ast: &'a AstArena<Typed>,
    pub source_index: &'a SourceSemanticIndex,
    pub typed_bindings: &'a dyn SourceTypedBindings,
    pub source: SourceMetadata,
    pub class: SymbolId,
    pub class_tree: TreeId<Untyped>,
    pub constructor: PrimaryConstructorInputPlan,
    pub members: &'a [ClassMemberPicklePlan],
}

struct ClassStateValidator<'p, 'a> {
    plan: &'p ClassStatePicklePlan<'a>,
    limits: ValidationLimits,
    remaining: usize,
}

impl ClassStateValidator<'_, '_> {
    fn source_int_literal(&self, literal: &dotty_core::ast::NumberLiteral) -> Option<i32> {
        let NumberKind::Whole(radix) = literal.kind else {
            return None;
        };
        let text = self.plan.store.names.try_resolve(literal.text)?;
        if text.len() > self.limits.max_name_bytes {
            return None;
        }
        let (negative, text) = if let Some(text) = text.strip_prefix('-') {
            (true, text)
        } else {
            (false, text.strip_prefix('+').unwrap_or(text))
        };
        let digits = text.replace('_', "");
        let digits = match radix {
            2 => digits
                .strip_prefix("0b")
                .or_else(|| digits.strip_prefix("0B"))?,
            8 => digits
                .strip_prefix("0o")
                .or_else(|| digits.strip_prefix("0O"))?,
            10 => digits.as_str(),
            16 => digits
                .strip_prefix("0x")
                .or_else(|| digits.strip_prefix("0X"))?,
            _ => return None,
        };
        let value = u32::from_str_radix(digits, radix).ok()?;
        if radix == 10 {
            if negative {
                (value <= (i32::MAX as u32) + 1).then(|| (-(i64::from(value))) as i32)
            } else {
                i32::try_from(value).ok()
            }
        } else {
            let value = value as i32;
            Some(if negative {
                value.wrapping_neg()
            } else {
                value
            })
        }
    }

    fn charge(&mut self, amount: usize) -> Result<(), PickleError> {
        self.remaining = self
            .remaining
            .checked_sub(amount)
            .ok_or(PickleError::LimitExceeded {
                kind: LimitKind::Items,
                limit: self.limits.max_items,
            })?;
        Ok(())
    }

    fn source_tree(&self, tree: TreeId<Untyped>) -> Result<&Tree<Untyped>, PickleError> {
        self.plan
            .source_ast
            .try_get(tree)
            .ok_or(PickleError::InvalidSourceTree { tree })
    }

    fn typed_tree(&self, tree: TreeId<Typed>) -> Result<&Tree<Typed>, PickleError> {
        self.plan
            .typed_ast
            .try_get(tree)
            .ok_or(PickleError::InvalidTypedTree { tree })
    }

    fn symbol(&self, symbol: SymbolId) -> Result<&Symbol, PickleError> {
        if self.plan.store.symbols.contains(symbol) {
            Ok(self.plan.store.symbols.get(symbol))
        } else {
            Err(PickleError::InvalidSymbol { symbol })
        }
    }

    fn completed(&self, symbol: SymbolId) -> Result<TypeId, PickleError> {
        let record = self.symbol(symbol)?;
        match record.info {
            SymbolInfo::Complete(ty) => match self.plan.store.types.try_get(ty) {
                Some(Type::NoType | Type::Error(_)) => Err(PickleError::IncompleteType { ty }),
                Some(_) => Ok(ty),
                None => Err(PickleError::InvalidType { ty }),
            },
            info => Err(PickleError::IncompleteSymbol { symbol, info }),
        }
    }

    fn same_canonical_type(&self, actual: TypeId, expected: TypeId) -> bool {
        let expected_symbol = match self.plan.store.types.try_get(expected) {
            Some(Type::TypeRef {
                prefix,
                target: dotty_core::TypeRefTarget::Symbol(symbol),
            }) if *prefix == self.plan.definitions.no_prefix => *symbol,
            _ => return false,
        };
        matches!(
            self.plan.store.types.try_get(actual),
            Some(Type::TypeRef {
                prefix,
                target: dotty_core::TypeRefTarget::Symbol(symbol),
            }) if *prefix == self.plan.definitions.no_prefix && *symbol == expected_symbol
        )
    }

    fn initializer_has_type(
        &self,
        actual: TypeId,
        expected: TypeId,
        initializer: &Tree<Typed>,
    ) -> bool {
        match self.plan.store.types.try_get(actual) {
            Some(Type::Constant(value)) => {
                matches!(
                    &initializer.kind,
                    TreeKind::Literal(literal) if &literal.value == value
                ) && match value {
                    Constant::Int(_) => expected == self.plan.definitions.int,
                    Constant::Boolean(_) => expected == self.plan.definitions.boolean,
                    _ => false,
                }
            }
            _ => self.same_canonical_type(actual, expected),
        }
    }

    fn package_owner_chain(&self, class: SymbolId) -> Result<(), PickleError> {
        let mut seen = HashSet::new();
        seen.insert(class);
        let mut current = self.symbol(class)?.owner;
        let mut depth = 0usize;
        let mut found_package = false;
        while let Some(owner) = current {
            if depth >= self.limits.max_owner_depth {
                return Err(PickleError::LimitExceeded {
                    kind: LimitKind::OwnerDepth,
                    limit: self.limits.max_owner_depth,
                });
            }
            depth += 1;
            if !seen.insert(owner) {
                return Err(PickleError::OwnerCycle { symbol: owner });
            }
            let record = self.symbol(owner)?;
            if record.kind != SymbolKind::Package {
                return Err(PickleError::Owner {
                    symbol: class,
                    expected: Some(owner),
                    actual: Some(owner),
                });
            }
            found_package = true;
            current = record.owner;
        }
        if !found_package {
            return Err(PickleError::Owner {
                symbol: class,
                expected: None,
                actual: None,
            });
        }
        Ok(())
    }

    fn source_identity(
        &self,
        symbol: SymbolId,
        tree: TreeId<Untyped>,
        kind: SymbolKind,
        name: Name,
        owner: SymbolId,
    ) -> Result<(), PickleError> {
        let record = self.symbol(symbol)?;
        if record.kind != kind {
            return Err(PickleError::Kind {
                symbol,
                expected: kind,
                actual: record.kind,
            });
        }
        if record.owner != Some(owner) {
            return Err(PickleError::Owner {
                symbol,
                expected: Some(owner),
                actual: record.owner,
            });
        }
        if record.origin != SymbolOrigin::Source(self.plan.source.id) {
            return Err(PickleError::SourceOrigin {
                symbol,
                expected: self.plan.source.id,
                actual: record.origin,
            });
        }
        if record.name != name {
            return Err(PickleError::NameMismatch { symbol, tree });
        }
        let name_text = self
            .plan
            .store
            .names
            .try_resolve(record.name.text())
            .ok_or(PickleError::InvalidName {
                name: record.name.text(),
            })?;
        if name_text.len() > self.limits.max_name_bytes {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::NameBytes,
                limit: self.limits.max_name_bytes,
            });
        }
        if self.plan.source_index.symbol_at(self.plan.source.id, tree) != Some(symbol)
            || self.plan.source_index.definition_of(symbol)
                != Some(SourceDefinition::Canonical {
                    source: self.plan.source.id,
                    tree,
                })
        {
            return Err(PickleError::SourceBinding {
                source: self.plan.source.id,
                tree,
                expected: symbol,
                actual: self.plan.source_index.symbol_at(self.plan.source.id, tree),
            });
        }
        if record
            .position
            .is_some_and(|position| position.source() != self.plan.source.id)
            || self
                .source_tree(tree)?
                .position
                .is_some_and(|position| position.source() != self.plan.source.id)
        {
            return Err(PickleError::SourceSpan {
                expected: self.plan.source.id,
                actual: record
                    .position
                    .or(self.source_tree(tree)?.position)
                    .expect("checked Some")
                    .source(),
            });
        }
        Ok(())
    }

    fn validate(&mut self) -> Result<(), PickleError> {
        let p = self.plan;
        self.charge(p.members.len())?;
        if p.source.name.is_empty()
            || p.source.name.contains(['\\', ':', '\0'])
            || p.source
                .name
                .split('/')
                .any(|part| matches!(part, "" | "." | ".."))
            || p.source.name.len() > self.limits.max_name_bytes
        {
            return Err(PickleError::InvalidSourceName);
        }
        let class = self.symbol(p.class)?;
        if class.kind != SymbolKind::Class || class.origin != SymbolOrigin::Source(p.source.id) {
            return Err(PickleError::Kind {
                symbol: p.class,
                expected: SymbolKind::Class,
                actual: class.kind,
            });
        }
        if class.visibility != Visibility::Public
            || !class.flags.is_empty()
            || !class.annotations.is_empty()
        {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::SymbolMetadata,
                symbol: Some(p.class),
            });
        }
        let TreeKind::TypeDef(class_tree) = &self.source_tree(p.class_tree)?.kind else {
            return Err(PickleError::SourceShape {
                tree: p.class_tree,
                expected: SourceRole::Class,
            });
        };
        if class_tree.metadata.visibility.is_some()
            || !class_tree.metadata.modifiers.is_empty()
            || !class_tree.metadata.annotations.is_empty()
        {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::SymbolMetadata,
                symbol: Some(p.class),
            });
        }
        self.source_identity(
            p.class,
            p.class_tree,
            SymbolKind::Class,
            *class_tree.name.as_name(),
            class.owner.ok_or(PickleError::Owner {
                symbol: p.class,
                expected: None,
                actual: None,
            })?,
        )?;
        if class_tree.variance.is_some() {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::TypeParameters,
                symbol: Some(p.class),
            });
        }
        self.package_owner_chain(p.class)?;
        let class_ty = self.completed(p.class)?;
        let Type::ClassInfo(info) = self
            .plan
            .store
            .types
            .try_get(class_ty)
            .ok_or(PickleError::InvalidType { ty: class_ty })?
        else {
            return Err(PickleError::MalformedType {
                ty: class_ty,
                role: TypeRole::ClassInfo,
            });
        };
        if info.class != p.class
            || info.prefix != p.definitions.no_prefix
            || info.parents.as_slice() != [p.definitions.object_type]
            || info.self_type.is_some()
            || p.source_index.scope_of(p.class) != Some(info.declarations)
        {
            return Err(PickleError::MalformedType {
                ty: class_ty,
                role: TypeRole::ClassInfo,
            });
        }
        let TreeKind::Template(template) = &self.source_tree(class_tree.rhs)?.kind else {
            return Err(PickleError::SourceShape {
                tree: class_tree.rhs,
                expected: SourceRole::Template,
            });
        };
        if !template.parents.is_empty()
            || template.self_val.is_some()
            || !template.metadata.derives.is_empty()
            || !template.metadata.uses.is_empty()
        {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::Parents,
                symbol: Some(p.class),
            });
        }
        let template_body = template.body.clone();
        let template_constructor = template.constructor;
        if template_body.len() != p.members.len() {
            return Err(PickleError::SourceOrder { class: p.class });
        }
        let constructor = &p.constructor;
        if template_constructor != constructor.source_tree {
            return Err(PickleError::SourceOrder { class: p.class });
        }
        self.validate_constructor(constructor, p.class)?;

        if !self.plan.store.scopes.contains(info.declarations) {
            return Err(PickleError::InvalidScope {
                scope: info.declarations,
            });
        }
        let class_scope = self.plan.store.scopes.get(info.declarations);
        if class_scope.owner != Some(p.class) {
            return Err(PickleError::InvalidScope {
                scope: info.declarations,
            });
        }
        let mut expected = HashSet::new();
        expected.insert(constructor.symbol);
        let mut fields = HashSet::new();
        let mut field_names = HashSet::new();
        self.charge(template_body.len())?;
        for (tree, member) in template_body.iter().zip(p.members) {
            self.charge(1)?;
            if tree != &member.source_tree() {
                return Err(PickleError::SourceOrder { class: p.class });
            }
            match member {
                ClassMemberPicklePlan::Field(field) => {
                    if !fields.insert(field.symbol) || !expected.insert(field.symbol) {
                        return Err(PickleError::DuplicateDeclaration {
                            symbol: field.symbol,
                        });
                    }
                    if !field_names.insert(self.symbol(field.symbol)?.name) {
                        return Err(PickleError::DuplicateDeclaration {
                            symbol: field.symbol,
                        });
                    }
                    self.validate_field(*field, p.class, constructor, &fields)?;
                }
                ClassMemberPicklePlan::Method {
                    symbol,
                    source_tree,
                } => {
                    if !expected.insert(*symbol) {
                        return Err(PickleError::DuplicateDeclaration { symbol: *symbol });
                    }
                    let TreeKind::DefDef(definition) = &self.source_tree(*source_tree)?.kind else {
                        return Err(PickleError::SourceShape {
                            tree: *source_tree,
                            expected: SourceRole::Method,
                        });
                    };
                    self.source_identity(
                        *symbol,
                        *source_tree,
                        SymbolKind::Method,
                        *definition.name.as_name(),
                        p.class,
                    )?;
                    let method = self.symbol(*symbol)?;
                    if method.visibility != Visibility::Public
                        || !method.flags.is_empty()
                        || !method.annotations.is_empty()
                        || definition.metadata.visibility.is_some()
                        || !definition.metadata.modifiers.is_empty()
                        || !definition.metadata.annotations.is_empty()
                    {
                        return Err(PickleError::Unsupported {
                            feature: UnsupportedFeature::ClassMembers,
                            symbol: Some(*symbol),
                        });
                    }
                    let method_ty = self.completed(*symbol)?;
                    self.plan
                        .store
                        .types
                        .try_get(method_ty)
                        .ok_or(PickleError::InvalidType { ty: method_ty })?;
                }
            }
        }
        let mut entered = HashSet::new();
        let mut invalid = false;
        for symbol in class_scope.entered_symbols() {
            self.charge(1)?;
            if !expected.contains(&symbol) || !entered.insert(symbol) {
                invalid = true;
            }
        }
        if invalid || entered != expected {
            return Err(PickleError::ScopeMembers { owner: p.class });
        }
        for &symbol in &expected {
            let name = self.symbol(symbol)?.name;
            if !class_scope.lookup_all(&name).contains(&symbol) {
                return Err(PickleError::ScopeMembers { owner: p.class });
            }
        }
        Ok(())
    }

    fn validate_constructor(
        &mut self,
        plan: &PrimaryConstructorInputPlan,
        class: SymbolId,
    ) -> Result<(), PickleError> {
        self.charge(plan.parameter_clauses.len())?;
        let item_count = plan
            .parameter_clauses
            .iter()
            .try_fold(0usize, |count, clause| count.checked_add(clause.len()))
            .ok_or(PickleError::LimitExceeded {
                kind: LimitKind::Items,
                limit: self.limits.max_items,
            })?;
        self.charge(item_count)?;
        let TreeKind::DefDef(definition) = &self.source_tree(plan.source_tree)?.kind else {
            return Err(PickleError::SourceShape {
                tree: plan.source_tree,
                expected: SourceRole::Constructor,
            });
        };
        self.source_identity(
            plan.symbol,
            plan.source_tree,
            SymbolKind::Constructor,
            *definition.name.as_name(),
            class,
        )?;
        let constructor_symbol = self.symbol(plan.symbol)?;
        if constructor_symbol.visibility != Visibility::Public
            || !constructor_symbol.flags.is_empty()
            || !constructor_symbol.annotations.is_empty()
        {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::PrimaryConstructorInput,
                symbol: Some(plan.symbol),
            });
        }
        if self
            .plan
            .store
            .names
            .try_resolve(definition.name.as_name().text())
            != Some("<init>")
            || !definition.type_params.is_empty()
            || definition.rhs.is_some()
            || definition.metadata.visibility.is_some()
            || !definition.metadata.modifiers.is_empty()
            || !definition.metadata.annotations.is_empty()
            || definition.value_param_clauses.len() > 1
            || definition
                .source_param_clause_order
                .as_ref()
                .is_some_and(|order| order.as_slice() != [DefParamClauseOrder::ValueParams(0)])
            || definition.value_param_clauses.len() != plan.parameter_clauses.len()
            || definition
                .value_param_clauses
                .iter()
                .zip(&plan.parameter_clauses)
                .any(|(trees, symbols)| trees.len() != symbols.len())
        {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::PrimaryConstructorInput,
                symbol: Some(plan.symbol),
            });
        }
        let signature_id = self.completed(plan.symbol)?;
        let Some(Type::Method(signature)) = self.plan.store.types.try_get(signature_id) else {
            return Err(PickleError::MalformedType {
                ty: signature_id,
                role: TypeRole::ConstructorSignature,
            });
        };
        if signature.kind != MethodKind::Plain
            || signature.params.len() != item_count
            || !matches!(self.plan.store.types.try_get(signature.result), Some(Type::TypeRef { prefix, target: dotty_core::TypeRefTarget::Symbol(owner) }) if *prefix == self.plan.definitions.no_prefix && *owner == class)
        {
            return Err(PickleError::MalformedType {
                ty: signature_id,
                role: TypeRole::ConstructorSignature,
            });
        }
        let mut signature_index = 0;
        let int_name = self
            .plan
            .store
            .symbols
            .get(
                self.plan
                    .store
                    .types
                    .get(self.plan.definitions.int)
                    .reference_symbol()
                    .ok_or(PickleError::BuiltinIdentity {
                        builtin: Builtin::Int,
                        ty: self.plan.definitions.int,
                    })?,
            )
            .name;
        for (clause_index, (trees, symbols)) in definition
            .value_param_clauses
            .iter()
            .zip(&plan.parameter_clauses)
            .enumerate()
        {
            if trees.is_empty() && !symbols.is_empty() {
                return Err(PickleError::ParameterOrder {
                    method: plan.symbol,
                    clause: clause_index,
                });
            }
            for (&tree, &symbol) in trees.iter().zip(symbols) {
                let TreeKind::ValDef(parameter) = &self.source_tree(tree)?.kind else {
                    return Err(PickleError::SourceShape {
                        tree,
                        expected: SourceRole::Parameter,
                    });
                };
                let constructor_scope_id = self
                    .plan
                    .source_index
                    .scope_of(plan.symbol)
                    .ok_or(PickleError::ScopeMembers { owner: plan.symbol })?;
                if !self.plan.store.scopes.contains(constructor_scope_id) {
                    return Err(PickleError::InvalidScope {
                        scope: constructor_scope_id,
                    });
                }
                let constructor_scope = self.plan.store.scopes.get(constructor_scope_id);
                if constructor_scope.owner != Some(plan.symbol)
                    || constructor_scope.lookup_all(parameter.name.as_name()) != [symbol]
                {
                    return Err(PickleError::ScopeMembers { owner: plan.symbol });
                }
                if self
                    .plan
                    .source_index
                    .derived_symbol_at(plan.symbol, self.plan.source.id, tree)
                    != Some(symbol)
                    || self.symbol(symbol)?.owner != Some(plan.symbol)
                    || self.symbol(symbol)?.kind != SymbolKind::Parameter
                {
                    return Err(PickleError::Owner {
                        symbol,
                        expected: Some(plan.symbol),
                        actual: self.symbol(symbol)?.owner,
                    });
                }
                let canonical = self
                    .plan
                    .source_index
                    .symbol_at(self.plan.source.id, tree)
                    .ok_or(PickleError::SourceDefinition { symbol })?;
                let canonical_record = self.symbol(canonical)?;
                let parameter_record = self.symbol(symbol)?;
                self.source_identity(
                    canonical,
                    tree,
                    SymbolKind::Parameter,
                    *parameter.name.as_name(),
                    class,
                )?;
                for position in [canonical_record.position, parameter_record.position]
                    .into_iter()
                    .flatten()
                {
                    if position.source() != self.plan.source.id {
                        return Err(PickleError::SourceSpan {
                            expected: self.plan.source.id,
                            actual: position.source(),
                        });
                    }
                }
                if canonical == symbol
                    || canonical_record.kind != SymbolKind::Parameter
                    || canonical_record.owner != Some(class)
                    || canonical_record.origin != SymbolOrigin::Source(self.plan.source.id)
                    || canonical_record.visibility != Visibility::Private
                    || !canonical_record.flags.is_empty()
                    || !canonical_record.annotations.is_empty()
                    || canonical_record.name != *parameter.name.as_name()
                    || parameter_record.name != *parameter.name.as_name()
                    || parameter_record.origin != SymbolOrigin::Source(self.plan.source.id)
                    || parameter_record.visibility != Visibility::Public
                    || !parameter_record.flags.is_empty()
                    || !parameter_record.annotations.is_empty()
                    || self.plan.source_index.definition_of(canonical)
                        != Some(SourceDefinition::Canonical {
                            source: self.plan.source.id,
                            tree,
                        })
                    || self.plan.source_index.definition_of(symbol)
                        != Some(SourceDefinition::Derived {
                            source: self.plan.source.id,
                            tree,
                        })
                {
                    return Err(PickleError::SourceDefinition { symbol });
                }
                let parameter_name = self
                    .plan
                    .store
                    .names
                    .try_resolve(parameter.name.as_name().text())
                    .ok_or(PickleError::InvalidName {
                        name: parameter.name.as_name().text(),
                    })?;
                if parameter_name.len() > self.limits.max_name_bytes {
                    return Err(PickleError::LimitExceeded {
                        kind: LimitKind::NameBytes,
                        limit: self.limits.max_name_bytes,
                    });
                }
                if !parameter.metadata.annotations.is_empty()
                    || parameter.rhs.is_some()
                    || parameter.metadata.modifiers.as_slice()
                        != [Modifier::ParamAccessor, Modifier::PrivateLocal]
                    || parameter.metadata.visibility.is_some()
                {
                    return Err(PickleError::Unsupported {
                        feature: UnsupportedFeature::PrimaryConstructorInput,
                        symbol: Some(symbol),
                    });
                }
                let TreeKind::Ident(tpt) = &self.source_tree(parameter.tpt)?.kind else {
                    return Err(PickleError::Unsupported {
                        feature: UnsupportedFeature::PrimaryConstructorInput,
                        symbol: Some(symbol),
                    });
                };
                let signature_param = signature.params[signature_index];
                match canonical_record.info {
                    SymbolInfo::Complete(ty) => match self.plan.store.types.try_get(ty) {
                        Some(Type::NoType | Type::Error(_)) => {
                            return Err(PickleError::IncompleteType { ty });
                        }
                        Some(_) if !self.same_canonical_type(ty, self.plan.definitions.int) => {
                            return Err(PickleError::UnsupportedTypeReference { ty });
                        }
                        Some(_) => {}
                        None => return Err(PickleError::InvalidType { ty }),
                    },
                    SymbolInfo::Missing | SymbolInfo::Deferred(_) => {}
                    info => {
                        return Err(PickleError::IncompleteSymbol {
                            symbol: canonical,
                            info,
                        });
                    }
                }
                if tpt.name != int_name
                    || signature_param.name.as_name() != parameter.name.as_name()
                    || signature_param.erased
                    || signature_param.varargs
                    || !self.same_canonical_type(signature_param.ty, self.plan.definitions.int)
                    || !self.same_canonical_type(self.completed(symbol)?, self.plan.definitions.int)
                {
                    return Err(PickleError::UnsupportedTypeReference {
                        ty: self.completed(symbol)?,
                    });
                }
                signature_index += 1;
            }
        }
        Ok(())
    }

    fn validate_field(
        &mut self,
        field: ClassFieldPicklePlan,
        class: SymbolId,
        constructor: &PrimaryConstructorInputPlan,
        fields_seen: &HashSet<SymbolId>,
    ) -> Result<(), PickleError> {
        let TreeKind::ValDef(definition) = &self.source_tree(field.source_tree)?.kind else {
            return Err(PickleError::SourceShape {
                tree: field.source_tree,
                expected: SourceRole::Field,
            });
        };
        self.source_identity(
            field.symbol,
            field.source_tree,
            SymbolKind::Field,
            *definition.name.as_name(),
            class,
        )?;
        let symbol = self.symbol(field.symbol)?;
        let is_var = definition.metadata.modifiers.as_slice() == [Modifier::Var];
        if definition.metadata.visibility != Some(VisibilitySyntax::Private { qualifier: None })
            || !definition.metadata.annotations.is_empty()
            || (!definition.metadata.modifiers.is_empty()
                && definition.metadata.modifiers.as_slice() != [Modifier::Var])
            || symbol.visibility != Visibility::Private
            || symbol.flags
                != if is_var {
                    SymbolFlags::MUTABLE
                } else {
                    SymbolFlags::EMPTY
                }
            || !symbol.annotations.is_empty()
        {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::ClassField,
                symbol: Some(field.symbol),
            });
        }
        if definition.rhs.is_none() {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::ClassField,
                symbol: Some(field.symbol),
            });
        }
        let TreeKind::Ident(type_syntax) = &self.source_tree(definition.tpt)?.kind else {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::ClassField,
                symbol: Some(field.symbol),
            });
        };
        let is_int = self.same_canonical_type(field.declared_type, self.plan.definitions.int);
        let is_boolean =
            self.same_canonical_type(field.declared_type, self.plan.definitions.boolean);
        if !is_int && !is_boolean {
            return Err(PickleError::UnsupportedTypeReference {
                ty: field.declared_type,
            });
        }
        let expected_name = self
            .plan
            .store
            .types
            .get(field.declared_type)
            .reference_symbol()
            .map(|symbol| self.plan.store.symbols.get(symbol).name);
        if expected_name.is_none_or(|expected| {
            let expected = self.plan.store.names.try_resolve(expected.text());
            let actual = self.plan.store.names.try_resolve(type_syntax.name.text());
            expected.is_none() || expected != actual
        }) {
            return Err(PickleError::FieldType {
                symbol: field.symbol,
                expected: field.declared_type,
                actual: self.completed(field.symbol)?,
            });
        }
        let actual_type = self.completed(field.symbol)?;
        if !self.same_canonical_type(actual_type, field.declared_type) {
            return Err(PickleError::FieldType {
                symbol: field.symbol,
                expected: field.declared_type,
                actual: actual_type,
            });
        }
        let source_rhs = definition.rhs.expect("checked present");
        let actual_binding = self
            .plan
            .typed_bindings
            .typed_at(self.plan.source.id, source_rhs);
        if actual_binding != Some(field.typed_initializer) {
            return Err(PickleError::FieldInitializerBinding {
                source: self.plan.source.id,
                source_tree: source_rhs,
                expected: field.typed_initializer,
                actual: actual_binding,
            });
        }
        let source_position = self.source_tree(source_rhs)?.position;
        let typed_position = self.typed_tree(field.typed_initializer)?.position;
        if source_position.is_none()
            || typed_position.is_none()
            || source_position != typed_position
            || source_position.is_some_and(|position| position.source() != self.plan.source.id)
        {
            return Err(PickleError::FieldInitializerSpan {
                source_tree: source_rhs,
                typed_tree: field.typed_initializer,
            });
        }
        let source_initializer = self.source_tree(source_rhs)?;
        let typed = self.typed_tree(field.typed_initializer)?;
        match (&source_initializer.kind, &typed.kind) {
            (
                TreeKind::PhaseSpecific(UntypedNode::Number(source_literal)),
                TreeKind::Literal(literal),
            ) if is_int
                && matches!(literal.value, Constant::Int(value) if self.source_int_literal(source_literal) == Some(value)) =>
                {}
            (TreeKind::Literal(source_literal), TreeKind::Literal(literal))
                if matches!(
                    (&literal.value, is_int),
                    (Constant::Int(_), true) | (Constant::Boolean(_), false)
                ) && source_literal.value == literal.value => {}
            (TreeKind::Ident(source_ident), TreeKind::Ident(typed_ident))
                if source_ident.name == typed_ident.name =>
            {
                let Some(Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(target),
                }) = self.plan.store.types.try_get(typed.ty)
                else {
                    return Err(PickleError::Unsupported {
                        feature: UnsupportedFeature::UnsafeFieldInitializer,
                        symbol: Some(field.symbol),
                    });
                };
                if *prefix != self.plan.definitions.no_prefix
                    || fields_seen.contains(target)
                    || self.symbol(*target)?.kind != SymbolKind::Parameter
                    || self.symbol(*target)?.owner != Some(constructor.symbol)
                    || self.symbol(*target)?.name != source_ident.name
                    || !constructor
                        .parameter_clauses
                        .iter()
                        .flatten()
                        .any(|parameter| parameter == target)
                {
                    return Err(PickleError::Unsupported {
                        feature: UnsupportedFeature::UnsafeFieldInitializer,
                        symbol: Some(field.symbol),
                    });
                }
            }
            (TreeKind::Ident(source_ident), TreeKind::Literal(literal))
                if !is_int
                    && matches!(literal.value, Constant::Boolean(value)
                        if self.plan.store.names.try_resolve(source_ident.name.text())
                            == Some(if value { "true" } else { "false" })) => {}
            _ => {
                return Err(PickleError::Unsupported {
                    feature: UnsupportedFeature::UnsafeFieldInitializer,
                    symbol: Some(field.symbol),
                });
            }
        }
        let initializer_type = match self.plan.store.types.try_get(typed.ty) {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(target),
                ..
            }) => self.completed(*target)?,
            Some(_) => typed.ty,
            None => return Err(PickleError::InvalidType { ty: typed.ty }),
        };
        if !self.initializer_has_type(initializer_type, field.declared_type, typed) {
            return Err(PickleError::FieldType {
                symbol: field.symbol,
                expected: field.declared_type,
                actual: initializer_type,
            });
        }
        Ok(())
    }
}

impl ClassMemberPicklePlan {
    fn source_tree(self) -> TreeId<Untyped> {
        match self {
            Self::Field(field) => field.source_tree,
            Self::Method { source_tree, .. } => source_tree,
        }
    }
}

/// A checked class-state input. It only borrows its source compilation session.
pub struct ValidatedClassStatePlan<'p, 'a> {
    plan: &'p ClassStatePicklePlan<'a>,
}

impl<'p, 'a> ValidatedClassStatePlan<'p, 'a> {
    pub fn plan(&self) -> &'p ClassStatePicklePlan<'a> {
        self.plan
    }

    /// Canonical relative path intended for this class's `.tasty` artifact.
    ///
    /// This is a path projection only; it does not encode or publish bytes.
    pub fn intended_tasty_path(&self) -> Result<String, PickleError> {
        crate::class_state_output::intended_class_tasty_path(self.plan, 256)
    }
}

impl ClassStatePicklePlan<'_> {
    /// Validate a narrow class state slice without mutating the semantic store,
    /// source/typed arenas, indexes, or the legacy serializer.
    pub fn validate(&self) -> Result<ValidatedClassStatePlan<'_, '_>, PickleError> {
        self.validate_with_limits(ValidationLimits::default())
    }

    pub fn validate_with_limits(
        &self,
        limits: ValidationLimits,
    ) -> Result<ValidatedClassStatePlan<'_, '_>, PickleError> {
        ClassStateValidator {
            plan: self,
            limits,
            remaining: limits.max_items,
        }
        .validate()?;
        Ok(ValidatedClassStatePlan { plan: self })
    }
}
