//! Read-only validation for ordinary class fields and primary constructor inputs.

use std::collections::HashSet;

use dotty_core::ast::{
    ApplyKind, AstArena, DefParamClauseOrder, Modifier, NumberKind, Tree, TreeKind, Typed, Untyped,
    UntypedNode, VisibilitySyntax,
};
use dotty_core::{
    Constant, Definitions, MethodKind, Name, SemanticStore, SourceDefinition, SourceSemanticIndex,
    Symbol, SymbolFlags, SymbolId, SymbolInfo, SymbolKind, SymbolOrigin, TermRefTarget, TreeId,
    Type, TypeId, TypeRefTarget, Visibility,
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

/// Validated signature roots for a one-formal method whose parameter and
/// result are different public classes in the same source batch. The method
/// body is intentionally not part of this proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DistinctReferenceMethodSignature {
    pub(crate) method: SymbolId,
    pub(crate) source_tree: TreeId<Untyped>,
    pub(crate) parameter: SymbolId,
    pub(crate) parameter_tree: TreeId<Untyped>,
    pub(crate) parameter_type: TypeId,
    pub(crate) parameter_class: SymbolId,
    pub(crate) parameter_class_tree: TreeId<Untyped>,
    pub(crate) parameter_type_syntax: TreeId<Untyped>,
    pub(crate) result_type: TypeId,
    pub(crate) result_class: SymbolId,
    pub(crate) result_class_tree: TreeId<Untyped>,
    pub(crate) result_type_syntax: TreeId<Untyped>,
    pub(crate) source_rhs: TreeId<Untyped>,
    pub(crate) typed_rhs: TreeId<Typed>,
}

impl DistinctReferenceMethodSignature {
    pub fn method(&self) -> SymbolId {
        self.method
    }
    pub fn parameter(&self) -> SymbolId {
        self.parameter
    }
    pub fn parameter_class(&self) -> SymbolId {
        self.parameter_class
    }
    pub fn result_class(&self) -> SymbolId {
        self.result_class
    }
    pub fn parameter_type(&self) -> TypeId {
        self.parameter_type
    }
    pub fn result_type(&self) -> TypeId {
        self.result_type
    }
    pub fn source_rhs(&self) -> TreeId<Untyped> {
        self.source_rhs
    }
    pub fn typed_rhs(&self) -> TreeId<Typed> {
        self.typed_rhs
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SameBatchReferenceFieldData {
    field: ClassFieldPicklePlan,
    target_class: SymbolId,
    target_class_tree: TreeId<Untyped>,
    type_name: Name,
    template_parameter_alias: Option<SymbolId>,
}

/// Read-only proof for one private immutable field whose class type is another
/// canonical public class in this plan's exact source batch. The proof records
/// its source and typed roots and validates the bounded initializer shape
/// before the serializer emits the initializer.
#[derive(Clone, Copy)]
pub struct ValidatedSameBatchReferenceField<'p, 'a> {
    plan: &'p ClassStatePicklePlan<'a>,
    data: SameBatchReferenceFieldData,
}

impl ValidatedSameBatchReferenceField<'_, '_> {
    /// Canonical class-owned field symbol.
    pub fn field(&self) -> SymbolId {
        self.data.field.symbol
    }

    /// Canonical owner class symbol.
    pub fn owner_class(&self) -> SymbolId {
        self.plan.class
    }

    /// Canonical public target class symbol from the same source batch.
    pub fn target_class(&self) -> SymbolId {
        self.data.target_class
    }

    /// Exact declared semantic TypeRef retained by the field.
    pub fn declared_type(&self) -> TypeId {
        self.data.field.declared_type
    }

    /// Original source initializer tree bound to this validated proof.
    pub fn source_initializer(&self) -> TreeId<Untyped> {
        let TreeKind::ValDef(definition) =
            &self.plan.source_ast.get(self.data.field.source_tree).kind
        else {
            unreachable!("validated reference field retains its ValDef")
        };
        definition.rhs.expect("validated reference initializer")
    }

    /// Exact typed initializer root bound to the source initializer.
    pub fn typed_initializer(&self) -> TreeId<Typed> {
        self.data.field.typed_initializer
    }

    /// Class-owned constructor-template parameter used by an audited alias
    /// initializer. The typed source reference remains constructor-owned.
    pub fn template_parameter_alias(&self) -> Option<SymbolId> {
        self.data.template_parameter_alias
    }

    /// Source field declaration tree.
    pub fn source_field(&self) -> TreeId<Untyped> {
        self.data.field.source_tree
    }

    pub(crate) fn projection_parts(
        &self,
    ) -> (
        &ClassStatePicklePlan<'_>,
        SymbolId,
        TreeId<Untyped>,
        Name,
        TypeId,
    ) {
        (
            self.plan,
            self.data.target_class,
            self.data.target_class_tree,
            self.data.type_name,
            self.data.field.declared_type,
        )
    }
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

    fn same_exact_type_ref(&self, actual: TypeId, expected: TypeId) -> bool {
        matches!(
            (self.plan.store.types.try_get(actual), self.plan.store.types.try_get(expected)),
            (
                Some(Type::TypeRef {
                    prefix: actual_prefix,
                    target: TypeRefTarget::Symbol(actual_target),
                }),
                Some(Type::TypeRef {
                    prefix: expected_prefix,
                    target: TypeRefTarget::Symbol(expected_target),
                })
            ) if actual_prefix == expected_prefix && actual_target == expected_target
        )
    }

    fn same_type_ref_target(&self, actual: TypeId, expected: TypeId) -> bool {
        matches!(
            (self.plan.store.types.try_get(actual), self.plan.store.types.try_get(expected)),
            (
                Some(Type::TypeRef {
                    target: TypeRefTarget::Symbol(actual_target),
                    ..
                }),
                Some(Type::TypeRef {
                    target: TypeRefTarget::Symbol(expected_target),
                    ..
                })
            ) if actual_target == expected_target
        )
    }

    fn same_batch_constructor_class_type(
        &self,
        ty: TypeId,
        owner_class: SymbolId,
        source_name: Name,
    ) -> Result<(SymbolId, TreeId<Untyped>), PickleError> {
        let Some(Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(target_class),
        }) = self.plan.store.types.try_get(ty)
        else {
            return Err(PickleError::UnsupportedTypeReference { ty });
        };
        let target_class = *target_class;
        let target_record = self.symbol(target_class)?;
        let owner_package = self.symbol(owner_class)?.owner;
        let prefix_matches_owner_package = owner_package.is_some_and(|package| {
            matches!(
                self.plan.store.types.try_get(*prefix),
                Some(Type::TypeRef {
                    prefix,
                    target: TypeRefTarget::Symbol(actual_package),
                }) if *prefix == self.plan.definitions.no_prefix && *actual_package == package
            )
        });
        if !prefix_matches_owner_package
            || target_class == owner_class
            || !self
                .plan
                .typed_bindings
                .planned_class_symbols()
                .contains(&target_class)
            || target_record.kind != SymbolKind::Class
            || target_record.origin != SymbolOrigin::Source(self.plan.source.id)
            || target_record.owner != owner_package
            || target_record.visibility != Visibility::Public
            || !target_record.flags.is_empty()
            || !target_record.annotations.is_empty()
            || target_record.name != source_name
        {
            return Err(PickleError::UnsupportedTypeReference { ty });
        }
        let Some(SourceDefinition::Canonical { source, tree }) =
            self.plan.source_index.definition_of(target_class)
        else {
            return Err(PickleError::SourceDefinition {
                symbol: target_class,
            });
        };
        let TreeKind::TypeDef(definition) = &self.source_tree(tree)?.kind else {
            return Err(PickleError::SourceShape {
                tree,
                expected: SourceRole::Class,
            });
        };
        if source != self.plan.source.id
            || self.plan.source_index.symbol_at(source, tree) != Some(target_class)
            || definition.name.as_name() != &source_name
        {
            return Err(PickleError::SourceDefinition {
                symbol: target_class,
            });
        }
        let target_type = self.completed(target_class)?;
        let Some(Type::ClassInfo(info)) = self.plan.store.types.try_get(target_type) else {
            return Err(PickleError::MalformedType {
                ty: target_type,
                role: TypeRole::ClassInfo,
            });
        };
        if info.class != target_class || info.prefix != self.plan.definitions.no_prefix {
            return Err(PickleError::MalformedType {
                ty: target_type,
                role: TypeRole::ClassInfo,
            });
        }
        Ok((target_class, tree))
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

    fn validate_planned_classes(&mut self) -> Result<(), PickleError> {
        let p = self.plan;
        let current = self.symbol(p.class)?;
        let package = current.owner.ok_or(PickleError::Owner {
            symbol: p.class,
            expected: None,
            actual: None,
        })?;
        let mut seen = HashSet::new();
        let mut found_current = false;
        for &class in p.typed_bindings.planned_class_symbols() {
            self.charge(1)?;
            if !seen.insert(class) {
                return Err(PickleError::DuplicateDeclaration { symbol: class });
            }
            let record = self.symbol(class)?;
            let Some(SourceDefinition::Canonical { source, tree }) =
                p.source_index.definition_of(class)
            else {
                return Err(PickleError::SourceDefinition { symbol: class });
            };
            let source_tree = self.source_tree(tree)?;
            let TreeKind::TypeDef(definition) = &source_tree.kind else {
                return Err(PickleError::SourceShape {
                    tree,
                    expected: SourceRole::Class,
                });
            };
            if source != p.source.id
                || p.source_index.symbol_at(source, tree) != Some(class)
                || record.kind != SymbolKind::Class
                || record.origin != SymbolOrigin::Source(p.source.id)
                || record.owner != Some(package)
                || record.name != *definition.name.as_name()
                || record.visibility != Visibility::Public
                || !record.flags.is_empty()
                || !record.annotations.is_empty()
                || definition.variance.is_some()
                || definition.metadata.visibility.is_some()
                || !definition.metadata.modifiers.is_empty()
                || !definition.metadata.annotations.is_empty()
            {
                return Err(PickleError::SourceDefinition { symbol: class });
            }
            let TreeKind::Template(template) = &self.source_tree(definition.rhs)?.kind else {
                return Err(PickleError::SourceShape {
                    tree: definition.rhs,
                    expected: SourceRole::Template,
                });
            };
            if !template.parents.is_empty()
                || template.self_val.is_some()
                || !template.metadata.derives.is_empty()
                || !template.metadata.uses.is_empty()
            {
                return Err(PickleError::SourceDefinition { symbol: class });
            }
            let TreeKind::DefDef(constructor) = &self.source_tree(template.constructor)?.kind
            else {
                return Err(PickleError::SourceShape {
                    tree: template.constructor,
                    expected: SourceRole::Constructor,
                });
            };
            if !constructor.type_params.is_empty() {
                return Err(PickleError::Unsupported {
                    feature: UnsupportedFeature::TypeParameters,
                    symbol: Some(class),
                });
            }
            let class_type = self.completed(class)?;
            let Type::ClassInfo(info) = self
                .plan
                .store
                .types
                .try_get(class_type)
                .ok_or(PickleError::InvalidType { ty: class_type })?
            else {
                return Err(PickleError::MalformedType {
                    ty: class_type,
                    role: TypeRole::ClassInfo,
                });
            };
            if info.class != class
                || info.prefix != p.definitions.no_prefix
                || info.parents.as_slice() != [p.definitions.object_type]
                || info.self_type.is_some()
                || p.source_index.scope_of(class) != Some(info.declarations)
                || !self.plan.store.scopes.contains(info.declarations)
                || self.plan.store.scopes.get(info.declarations).owner != Some(class)
                || self
                    .plan
                    .store
                    .scopes
                    .get(info.declarations)
                    .entered_symbols()
                    .any(|member| {
                        self.plan.store.symbols.contains(member)
                            && self.plan.store.symbols.get(member).kind == SymbolKind::TypeParameter
                    })
            {
                return Err(PickleError::MalformedType {
                    ty: class_type,
                    role: TypeRole::ClassInfo,
                });
            }
            found_current |= class == p.class;
        }
        if !p.typed_bindings.planned_class_symbols().is_empty() && !found_current {
            return Err(PickleError::SourceDefinition { symbol: p.class });
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

    fn validate(&mut self) -> Result<Vec<SameBatchReferenceFieldData>, PickleError> {
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
        self.validate_planned_classes()?;
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
        let mut reference_fields = Vec::new();
        let mut field_count = 0usize;
        let mut method_count = 0usize;
        self.charge(template_body.len())?;
        for (tree, member) in template_body.iter().zip(p.members) {
            self.charge(1)?;
            if tree != &member.source_tree() {
                return Err(PickleError::SourceOrder { class: p.class });
            }
            match member {
                ClassMemberPicklePlan::Field(field) => {
                    field_count += 1;
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
                    if let Some(reference_field) =
                        self.validate_field(*field, p.class, constructor, &fields)?
                    {
                        reference_fields.push(reference_field);
                    }
                }
                ClassMemberPicklePlan::Method {
                    symbol,
                    source_tree,
                } => {
                    method_count += 1;
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
        // The dual-reference extension is intentionally limited to the exact
        // ordered Pair shape: two private immutable Counter aliases followed
        // by two ordinary read methods. The per-field proofs below authenticate
        // the alias edges; this shape gate prevents unrelated class members
        // from becoming reachable merely because the constructor has two
        // same-batch reference inputs.
        let reference_parameter_count = constructor
            .parameter_clauses
            .iter()
            .flatten()
            .filter(|&&parameter| {
                self.completed(parameter)
                    .is_ok_and(|ty| !self.same_canonical_type(ty, self.plan.definitions.int))
            })
            .count();
        if reference_parameter_count == 2 && !p.members.is_empty() {
            if field_count != 2
                || method_count != 2
                || reference_fields.len() != 2
                || reference_fields
                    .iter()
                    .any(|field| field.template_parameter_alias.is_none())
            {
                return Err(PickleError::Unsupported {
                    feature: UnsupportedFeature::ClassMembers,
                    symbol: Some(p.class),
                });
            }
            let source_parameters = self.constructor_template_parameters(constructor)?;
            let aliases = reference_fields
                .iter()
                .map(|field| field.template_parameter_alias.expect("checked above"))
                .collect::<Vec<_>>();
            if aliases != source_parameters {
                return Err(PickleError::Unsupported {
                    feature: UnsupportedFeature::UnsafeFieldInitializer,
                    symbol: Some(p.class),
                });
            }
            self.validate_dual_reference_read_methods(p.class, &reference_fields, p.members)?;
        }
        for &symbol in &expected {
            let name = self.symbol(symbol)?.name;
            if !class_scope.lookup_all(&name).contains(&symbol) {
                return Err(PickleError::ScopeMembers { owner: p.class });
            }
        }
        Ok(reference_fields)
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
        let mut reference_parameter_count = 0usize;
        let mut reference_class = None;
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
                let parameter_type = self.completed(symbol)?;
                let is_int = self.same_canonical_type(parameter_type, self.plan.definitions.int);
                let is_reference = if is_int {
                    false
                } else {
                    let (target_class, _) =
                        self.same_batch_constructor_class_type(parameter_type, class, tpt.name)?;
                    if reference_class.is_some_and(|previous| previous != target_class) {
                        return Err(PickleError::UnsupportedTypeReference { ty: parameter_type });
                    }
                    reference_class = Some(target_class);
                    true
                };
                match canonical_record.info {
                    SymbolInfo::Complete(ty) => {
                        if matches!(
                            self.plan.store.types.try_get(ty),
                            Some(Type::NoType | Type::Error(_))
                        ) {
                            return Err(PickleError::IncompleteType { ty });
                        }
                        if is_int {
                            if !self.same_canonical_type(ty, self.plan.definitions.int) {
                                return Err(PickleError::UnsupportedTypeReference { ty });
                            }
                        } else if !self.same_exact_type_ref(ty, parameter_type) {
                            return Err(PickleError::UnsupportedTypeReference { ty });
                        }
                    }
                    SymbolInfo::Missing | SymbolInfo::Deferred(_) => {}
                    info => {
                        return Err(PickleError::IncompleteSymbol {
                            symbol: canonical,
                            info,
                        });
                    }
                }
                if is_reference {
                    if !self.same_exact_type_ref(signature_param.ty, parameter_type) {
                        return Err(PickleError::UnsupportedTypeReference {
                            ty: signature_param.ty,
                        });
                    }
                    reference_parameter_count += 1;
                }
                if (is_int && tpt.name != int_name)
                    || signature_param.name.as_name() != parameter.name.as_name()
                    || signature_param.erased
                    || signature_param.varargs
                    || (is_int
                        && !self.same_canonical_type(signature_param.ty, self.plan.definitions.int))
                {
                    return Err(PickleError::UnsupportedTypeReference { ty: parameter_type });
                }
                signature_index += 1;
            }
        }
        if reference_parameter_count != 0
            && ((reference_parameter_count == 1 && item_count != 1)
                || (reference_parameter_count == 2 && item_count != 2)
                || reference_parameter_count > 2)
        {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::PrimaryConstructorInput,
                symbol: Some(plan.symbol),
            });
        }
        Ok(())
    }

    fn validate_reference_field_type(
        &self,
        field: ClassFieldPicklePlan,
        class: SymbolId,
        type_syntax: Name,
    ) -> Result<SameBatchReferenceFieldData, PickleError> {
        let Some(Type::TypeRef { prefix, target }) =
            self.plan.store.types.try_get(field.declared_type)
        else {
            return Err(PickleError::UnsupportedTypeReference {
                ty: field.declared_type,
            });
        };
        let TypeRefTarget::Symbol(target_class) = target else {
            return Err(PickleError::UnsupportedTypeReference {
                ty: field.declared_type,
            });
        };
        let target_class = *target_class;
        let target_symbol = self.symbol(target_class)?;
        let owner = self.symbol(class)?.owner;
        let package_prefix_matches = matches!(
            self.plan.store.types.try_get(*prefix),
            Some(Type::TypeRef {
                prefix: package_prefix,
                target: TypeRefTarget::Symbol(package),
            }) if *package_prefix == self.plan.definitions.no_prefix
                && Some(*package) == target_symbol.owner
        );
        if !package_prefix_matches
            || target_class == class
            || !self
                .plan
                .typed_bindings
                .planned_class_symbols()
                .contains(&target_class)
            || target_symbol.kind != SymbolKind::Class
            || target_symbol.origin != SymbolOrigin::Source(self.plan.source.id)
            || target_symbol.owner != owner
            || target_symbol.visibility != Visibility::Public
            || !target_symbol.flags.is_empty()
            || !target_symbol.annotations.is_empty()
            || target_symbol.name != type_syntax
        {
            return Err(PickleError::UnsupportedTypeReference {
                ty: field.declared_type,
            });
        }
        let Some(SourceDefinition::Canonical { source, tree }) =
            self.plan.source_index.definition_of(target_class)
        else {
            return Err(PickleError::SourceDefinition {
                symbol: target_class,
            });
        };
        if source != self.plan.source.id
            || self.plan.source_index.symbol_at(source, tree) != Some(target_class)
        {
            return Err(PickleError::SourceDefinition {
                symbol: target_class,
            });
        }
        let target_type = self.completed(target_class)?;
        let Some(Type::ClassInfo(info)) = self.plan.store.types.try_get(target_type) else {
            return Err(PickleError::MalformedType {
                ty: target_type,
                role: TypeRole::ClassInfo,
            });
        };
        if info.class != target_class || info.prefix != self.plan.definitions.no_prefix {
            return Err(PickleError::MalformedType {
                ty: target_type,
                role: TypeRole::ClassInfo,
            });
        }
        Ok(SameBatchReferenceFieldData {
            field,
            target_class,
            target_class_tree: tree,
            type_name: type_syntax,
            template_parameter_alias: None,
        })
    }

    fn constructor_template_parameters(
        &self,
        constructor: &PrimaryConstructorInputPlan,
    ) -> Result<Vec<SymbolId>, PickleError> {
        let TreeKind::DefDef(definition) = &self.source_tree(constructor.source_tree)?.kind else {
            return Err(PickleError::SourceShape {
                tree: constructor.source_tree,
                expected: SourceRole::Constructor,
            });
        };
        if definition.value_param_clauses.len() != 1
            || constructor.parameter_clauses.len() != 1
            || definition.value_param_clauses[0].len() != 2
            || constructor.parameter_clauses[0].len() != 2
        {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::PrimaryConstructorInput,
                symbol: Some(constructor.symbol),
            });
        }
        definition.value_param_clauses[0]
            .iter()
            .map(|&tree| {
                self.plan
                    .source_index
                    .symbol_at(self.plan.source.id, tree)
                    .ok_or(PickleError::SourceDefinition {
                        symbol: constructor.symbol,
                    })
            })
            .collect()
    }

    fn validate_dual_reference_read_methods(
        &mut self,
        class: SymbolId,
        reference_fields: &[SameBatchReferenceFieldData],
        members: &[ClassMemberPicklePlan],
    ) -> Result<(), PickleError> {
        // Keep the newly enabled shape closed: both fields are declared before
        // the two read methods, and each method's actual typed receiver must
        // target the corresponding source-ordered field receipt.
        let [
            ClassMemberPicklePlan::Field(first_field),
            ClassMemberPicklePlan::Field(second_field),
            ClassMemberPicklePlan::Method {
                symbol: first_method,
                source_tree: first_tree,
            },
            ClassMemberPicklePlan::Method {
                symbol: second_method,
                source_tree: second_tree,
            },
        ] = members
        else {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::ClassMembers,
                symbol: Some(class),
            });
        };
        if reference_fields.len() != 2
            || first_field.symbol != reference_fields[0].field.symbol
            || second_field.symbol != reference_fields[1].field.symbol
            || reference_fields[0].target_class != reference_fields[1].target_class
        {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::ClassMembers,
                symbol: Some(class),
            });
        }
        self.validate_dual_reference_reader(
            class,
            reference_fields[0],
            *first_method,
            *first_tree,
        )?;
        self.validate_dual_reference_reader(
            class,
            reference_fields[1],
            *second_method,
            *second_tree,
        )
    }

    fn validate_dual_reference_reader(
        &mut self,
        class: SymbolId,
        field: SameBatchReferenceFieldData,
        method: SymbolId,
        source_tree: TreeId<Untyped>,
    ) -> Result<(), PickleError> {
        let reject = || PickleError::Unsupported {
            feature: UnsupportedFeature::ClassMembers,
            symbol: Some(method),
        };
        let source_node = self.source_tree(source_tree)?;
        let TreeKind::DefDef(definition) = &source_node.kind else {
            return Err(PickleError::SourceShape {
                tree: source_tree,
                expected: SourceRole::Method,
            });
        };
        if !self.plan.store.symbols.contains(method) {
            return Err(PickleError::InvalidSymbol { symbol: method });
        }
        let method_record = self.plan.store.symbols.get(method);
        let method_type = self.completed(method)?;
        let Some(Type::Method(method_signature)) = self.plan.store.types.try_get(method_type)
        else {
            return Err(reject());
        };
        if method_record.kind != SymbolKind::Method
            || method_record.owner != Some(class)
            || method_record.origin != SymbolOrigin::Source(self.plan.source.id)
            || method_record.visibility != Visibility::Public
            || !method_record.flags.is_empty()
            || !method_record.annotations.is_empty()
            || method_record.name != *definition.name.as_name()
            || self
                .plan
                .source_index
                .symbol_at(self.plan.source.id, source_tree)
                != Some(method)
            || self.plan.source_index.definition_of(method)
                != Some(SourceDefinition::Canonical {
                    source: self.plan.source.id,
                    tree: source_tree,
                })
            || !definition.type_params.is_empty()
            || definition.value_param_clauses.len() != 1
            || !definition.value_param_clauses[0].is_empty()
            || definition.source_param_clause_order.is_some()
            || definition.metadata.visibility.is_some()
            || !definition.metadata.modifiers.is_empty()
            || !definition.metadata.annotations.is_empty()
            || method_signature.kind != MethodKind::Plain
            || !method_signature.params.is_empty()
            || !self.same_canonical_type(method_signature.result, self.plan.definitions.int)
        {
            return Err(reject());
        }
        let Some(rhs) = definition.rhs else {
            return Err(PickleError::MissingBody { method });
        };
        let typed_rhs = self
            .plan
            .typed_bindings
            .typed_at(self.plan.source.id, rhs)
            .ok_or(PickleError::MissingBody { method })?;
        let source_apply_node = self.source_tree(rhs)?;
        let typed_apply_node = self.typed_tree(typed_rhs)?;
        let (TreeKind::Apply(source_apply), TreeKind::Apply(typed_apply)) =
            (&source_apply_node.kind, &typed_apply_node.kind)
        else {
            return Err(reject());
        };
        if source_apply.kind != ApplyKind::Regular
            || typed_apply.kind != ApplyKind::Regular
            || !source_apply.args.is_empty()
            || !typed_apply.args.is_empty()
            || !self.same_canonical_type(typed_apply_node.ty, self.plan.definitions.int)
        {
            return Err(reject());
        }
        let source_select_id = source_apply.function;
        let typed_select_id = typed_apply.function;
        let typed_select_binding = self
            .plan
            .typed_bindings
            .typed_at(self.plan.source.id, source_select_id);
        if typed_select_binding != Some(typed_select_id) {
            return Err(reject());
        }
        let source_select_node = self.source_tree(source_select_id)?;
        let typed_select_node = self.typed_tree(typed_select_id)?;
        let (TreeKind::Select(source_select), TreeKind::Select(typed_select)) =
            (&source_select_node.kind, &typed_select_node.kind)
        else {
            return Err(reject());
        };
        if source_select.name != typed_select.name
            || source_select.backquoted != typed_select.backquoted
            || source_select_node.position.is_none()
            || source_select_node.position != typed_select_node.position
        {
            return Err(reject());
        }
        let source_receiver_id = source_select.qualifier;
        let typed_receiver_id = typed_select.qualifier;
        if self
            .plan
            .typed_bindings
            .typed_at(self.plan.source.id, source_receiver_id)
            != Some(typed_receiver_id)
        {
            return Err(reject());
        }
        let source_receiver = self.source_tree(source_receiver_id)?;
        let typed_receiver = self.typed_tree(typed_receiver_id)?;
        let (TreeKind::Ident(source_ident), TreeKind::Ident(typed_ident)) =
            (&source_receiver.kind, &typed_receiver.kind)
        else {
            return Err(reject());
        };
        let field_symbol = self.symbol(field.field.symbol)?;
        if source_ident.name != typed_ident.name
            || source_ident.backquoted != typed_ident.backquoted
            || source_ident.name != field_symbol.name
            || source_receiver.position.is_none()
            || source_receiver.position != typed_receiver.position
        {
            return Err(reject());
        }
        let Some(Type::TermRef {
            prefix: receiver_prefix,
            target: TermRefTarget::Symbol(receiver_symbol),
        }) = self.plan.store.types.try_get(typed_receiver.ty)
        else {
            return Err(reject());
        };
        let Some(Type::ThisType {
            class: receiver_owner,
        }) = self.plan.store.types.try_get(*receiver_prefix)
        else {
            return Err(reject());
        };
        if *receiver_symbol != field.field.symbol || *receiver_owner != class {
            return Err(reject());
        }

        let Some(Type::TermRef {
            prefix: selected_prefix,
            target: TermRefTarget::Symbol(getter),
        }) = self.plan.store.types.try_get(typed_select_node.ty)
        else {
            return Err(reject());
        };
        if *selected_prefix != typed_receiver.ty {
            return Err(reject());
        }
        self.validate_dual_reference_getter(field.target_class, *getter, source_select.name)?;
        if !self.same_canonical_type(typed_apply_node.ty, self.plan.definitions.int) {
            return Err(reject());
        }
        for (source_tree, typed_tree) in [
            (rhs, typed_rhs),
            (source_select_id, typed_select_id),
            (source_receiver_id, typed_receiver_id),
        ] {
            let source = self.source_tree(source_tree)?;
            let typed = self.typed_tree(typed_tree)?;
            if source.position.is_none()
                || source.position != typed.position
                || source
                    .position
                    .is_some_and(|position| position.source() != self.plan.source.id)
            {
                return Err(reject());
            }
        }
        Ok(())
    }

    fn validate_dual_reference_getter(
        &self,
        counter: SymbolId,
        getter: SymbolId,
        selected_name: Name,
    ) -> Result<(), PickleError> {
        let reject = || PickleError::Unsupported {
            feature: UnsupportedFeature::ClassMembers,
            symbol: Some(getter),
        };
        let Some(SourceDefinition::Canonical { source, tree }) =
            self.plan.source_index.definition_of(getter)
        else {
            return Err(PickleError::SourceDefinition { symbol: getter });
        };
        if source != self.plan.source.id
            || self.plan.source_index.symbol_at(source, tree) != Some(getter)
            || !self
                .plan
                .typed_bindings
                .planned_method_symbols()
                .contains(&getter)
        {
            return Err(PickleError::SourceDefinition { symbol: getter });
        }
        let getter_record = self.symbol(getter)?;
        let getter_type = self.completed(getter)?;
        let Some(Type::Method(signature)) = self.plan.store.types.try_get(getter_type) else {
            return Err(reject());
        };
        let getter_tree = self.source_tree(tree)?;
        let TreeKind::DefDef(definition) = &getter_tree.kind else {
            return Err(PickleError::SourceShape {
                tree,
                expected: SourceRole::Method,
            });
        };
        let counter_type = match self.plan.store.symbols.get(counter).info {
            SymbolInfo::Complete(ty) => ty,
            info => {
                return Err(PickleError::IncompleteSymbol {
                    symbol: counter,
                    info,
                });
            }
        };
        let Some(Type::ClassInfo(class_info)) = self.plan.store.types.try_get(counter_type) else {
            return Err(PickleError::MalformedType {
                ty: counter_type,
                role: TypeRole::ClassInfo,
            });
        };
        if getter_record.kind != SymbolKind::Method
            || getter_record.owner != Some(counter)
            || getter_record.origin != SymbolOrigin::Source(self.plan.source.id)
            || getter_record.visibility != Visibility::Public
            || !getter_record.flags.is_empty()
            || !getter_record.annotations.is_empty()
            || getter_record.name != selected_name
            || definition.name.as_name() != &selected_name
            || !definition.type_params.is_empty()
            || definition.value_param_clauses.len() != 1
            || !definition.value_param_clauses[0].is_empty()
            || definition.rhs.is_none()
            || signature.kind != MethodKind::Plain
            || !signature.params.is_empty()
            || !self.same_canonical_type(signature.result, self.plan.definitions.int)
            || self.plan.store.names.try_resolve(getter_record.name.text()) != Some("get")
            || class_info.class != counter
            || !self.plan.store.scopes.contains(class_info.declarations)
            || !self
                .plan
                .store
                .scopes
                .get(class_info.declarations)
                .lookup_all(&getter_record.name)
                .contains(&getter)
        {
            return Err(reject());
        }
        Ok(())
    }

    fn validate_pending_reference_initializer(
        &self,
        field: ClassFieldPicklePlan,
        constructor: &PrimaryConstructorInputPlan,
        target_class: SymbolId,
        source_rhs: TreeId<Untyped>,
    ) -> Result<Option<SymbolId>, PickleError> {
        let plan = self.plan;
        let source_root = self.source_tree(source_rhs)?;
        let typed_root = self.typed_tree(field.typed_initializer)?;
        let binding = |source_tree| plan.typed_bindings.typed_at(plan.source.id, source_tree);
        if binding(source_rhs) != Some(field.typed_initializer)
            || source_root.position.is_none()
            || source_root.position != typed_root.position
            || source_root
                .position
                .is_some_and(|position| position.source() != plan.source.id)
        {
            return Err(PickleError::FieldInitializerBinding {
                source: plan.source.id,
                source_tree: source_rhs,
                expected: field.typed_initializer,
                actual: binding(source_rhs),
            });
        }
        if let (TreeKind::Ident(source_ident), TreeKind::Ident(typed_ident)) =
            (&source_root.kind, &typed_root.kind)
        {
            let TreeKind::DefDef(constructor_definition) =
                &self.source_tree(constructor.source_tree)?.kind
            else {
                return Err(PickleError::SourceShape {
                    tree: constructor.source_tree,
                    expected: SourceRole::Constructor,
                });
            };
            if constructor_definition.value_param_clauses.len() != 1
                || constructor.parameter_clauses.len() != 1
                || constructor_definition.value_param_clauses[0].len()
                    != constructor.parameter_clauses[0].len()
                || source_ident.name != typed_ident.name
            {
                return Err(PickleError::Unsupported {
                    feature: UnsupportedFeature::UnsafeFieldInitializer,
                    symbol: Some(field.symbol),
                });
            }
            let typed_target = match plan.store.types.try_get(typed_root.ty) {
                Some(Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(target),
                }) if *prefix == plan.definitions.no_prefix => Some(*target),
                _ => None,
            };
            let Some(parameter_index) = constructor.parameter_clauses[0]
                .iter()
                .position(|&parameter| typed_target == Some(parameter))
            else {
                return Err(PickleError::Unsupported {
                    feature: UnsupportedFeature::UnsafeFieldInitializer,
                    symbol: Some(field.symbol),
                });
            };
            let parameter_tree = constructor_definition.value_param_clauses[0][parameter_index];
            let Some(template_parameter) =
                plan.source_index.symbol_at(plan.source.id, parameter_tree)
            else {
                return Err(PickleError::SourceDefinition {
                    symbol: constructor.symbol,
                });
            };
            let constructor_parameter = constructor.parameter_clauses[0][parameter_index];
            let derived_parameter = plan.source_index.derived_symbol_at(
                constructor.symbol,
                plan.source.id,
                parameter_tree,
            );
            let source_definition = plan.source_index.definition_of(template_parameter);
            let parameter_type = self.completed(constructor_parameter)?;
            let Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(parameter_class),
                ..
            }) = plan.store.types.try_get(parameter_type)
            else {
                return Err(PickleError::Unsupported {
                    feature: UnsupportedFeature::UnsafeFieldInitializer,
                    symbol: Some(field.symbol),
                });
            };
            let alias_name = source_ident.name;
            let template_record = self.symbol(template_parameter)?;
            let derived_record = self.symbol(constructor_parameter)?;
            if source_ident.name != derived_record.name
                || typed_target != Some(constructor_parameter)
                || derived_parameter != Some(constructor_parameter)
                || source_definition
                    != Some(SourceDefinition::Canonical {
                        source: plan.source.id,
                        tree: parameter_tree,
                    })
                || template_record.kind != SymbolKind::Parameter
                || template_record.owner != Some(plan.class)
                || template_record.origin != SymbolOrigin::Source(plan.source.id)
                || template_record.name != alias_name
                || derived_record.kind != SymbolKind::Parameter
                || derived_record.owner != Some(constructor.symbol)
                || derived_record.origin != SymbolOrigin::Source(plan.source.id)
                || derived_record.name != alias_name
                || *parameter_class != target_class
                || !self.same_type_ref_target(parameter_type, field.declared_type)
                || source_root.position != typed_root.position
                || source_root
                    .position
                    .is_none_or(|position| position.source() != plan.source.id)
            {
                return Err(PickleError::Unsupported {
                    feature: UnsupportedFeature::UnsafeFieldInitializer,
                    symbol: Some(field.symbol),
                });
            }
            return Ok(Some(template_parameter));
        }
        let (TreeKind::Apply(source_apply), TreeKind::Apply(typed_apply)) =
            (&source_root.kind, &typed_root.kind)
        else {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::UnsafeFieldInitializer,
                symbol: Some(field.symbol),
            });
        };
        if source_apply.kind != dotty_core::ast::ApplyKind::Regular
            || typed_apply.kind != dotty_core::ast::ApplyKind::Regular
            || source_apply.args.len() != 1
            || typed_apply.args.len() != 1
            || binding(source_apply.function) != Some(typed_apply.function)
            || binding(source_apply.args[0]) != Some(typed_apply.args[0])
        {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::UnsafeFieldInitializer,
                symbol: Some(field.symbol),
            });
        }
        let (TreeKind::Select(source_select), TreeKind::Select(typed_select)) = (
            &self.source_tree(source_apply.function)?.kind,
            &self.typed_tree(typed_apply.function)?.kind,
        ) else {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::UnsafeFieldInitializer,
                symbol: Some(field.symbol),
            });
        };
        let typed_function = self.typed_tree(typed_apply.function)?;
        let Some(Type::TermRef {
            prefix: constructor_prefix,
            target: TermRefTarget::Symbol(constructor_symbol),
        }) = plan.store.types.try_get(typed_function.ty)
        else {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::UnsafeFieldInitializer,
                symbol: Some(field.symbol),
            });
        };
        let constructor_info = self.symbol(*constructor_symbol)?;
        let target_class_tree = match plan.source_index.definition_of(target_class) {
            Some(SourceDefinition::Canonical { source, tree }) if source == plan.source.id => tree,
            _ => {
                return Err(PickleError::SourceDefinition {
                    symbol: target_class,
                });
            }
        };
        let TreeKind::TypeDef(class_definition) = &self.source_tree(target_class_tree)?.kind else {
            return Err(PickleError::SourceShape {
                tree: target_class_tree,
                expected: SourceRole::Class,
            });
        };
        let TreeKind::Template(class_template) = &self.source_tree(class_definition.rhs)?.kind
        else {
            return Err(PickleError::SourceShape {
                tree: class_definition.rhs,
                expected: SourceRole::Template,
            });
        };
        let primary_constructor = plan
            .source_index
            .symbol_at(plan.source.id, class_template.constructor);
        if source_select.name != constructor_info.name
            || plan.store.names.resolve(source_select.name.text()) != "<init>"
            || typed_select.name != source_select.name
            || constructor_info.kind != SymbolKind::Constructor
            || constructor_info.owner != Some(target_class)
            || primary_constructor != Some(*constructor_symbol)
            || !matches!(
                plan.store.types.try_get(*constructor_prefix),
                Some(Type::TypeRef {
                    target: TypeRefTarget::Symbol(selected_class),
                    ..
                }) if *selected_class == target_class
            )
        {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::UnsafeFieldInitializer,
                symbol: Some(field.symbol),
            });
        }
        if binding(source_select.qualifier) != Some(typed_select.qualifier) {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::UnsafeFieldInitializer,
                symbol: Some(field.symbol),
            });
        }
        let (TreeKind::New(source_new), TreeKind::New(typed_new)) = (
            &self.source_tree(source_select.qualifier)?.kind,
            &self.typed_tree(typed_select.qualifier)?.kind,
        ) else {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::UnsafeFieldInitializer,
                symbol: Some(field.symbol),
            });
        };
        if binding(source_new.tpt) != Some(typed_new.tpt) {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::UnsafeFieldInitializer,
                symbol: Some(field.symbol),
            });
        }
        let TreeKind::Ident(source_type) = &self.source_tree(source_new.tpt)?.kind else {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::UnsafeFieldInitializer,
                symbol: Some(field.symbol),
            });
        };
        let TreeKind::Ident(source_argument) = &self.source_tree(source_apply.args[0])?.kind else {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::UnsafeFieldInitializer,
                symbol: Some(field.symbol),
            });
        };
        let typed_argument = self.typed_tree(typed_apply.args[0])?;
        let TreeKind::Ident(typed_argument_ident) = &typed_argument.kind else {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::UnsafeFieldInitializer,
                symbol: Some(field.symbol),
            });
        };
        let Some(Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(argument_symbol),
        }) = plan.store.types.try_get(typed_argument.ty)
        else {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::UnsafeFieldInitializer,
                symbol: Some(field.symbol),
            });
        };
        if source_argument.name != typed_argument_ident.name
            || *prefix != plan.definitions.no_prefix
            || self.symbol(*argument_symbol)?.kind != SymbolKind::Parameter
            || self.symbol(*argument_symbol)?.name != source_argument.name
            || self.symbol(*argument_symbol)?.owner != Some(constructor.symbol)
            || !constructor
                .parameter_clauses
                .iter()
                .flatten()
                .any(|parameter| parameter == argument_symbol)
        {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::UnsafeFieldInitializer,
                symbol: Some(field.symbol),
            });
        }
        let Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(source_target),
            ..
        }) = plan.store.types.try_get(self.typed_tree(typed_new.tpt)?.ty)
        else {
            return Err(PickleError::UnsupportedTypeReference { ty: typed_root.ty });
        };
        let Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(result_target),
            ..
        }) = plan.store.types.try_get(typed_root.ty)
        else {
            return Err(PickleError::FieldType {
                symbol: field.symbol,
                expected: field.declared_type,
                actual: typed_root.ty,
            });
        };
        if *source_target != target_class
            || *result_target != target_class
            || plan.store.symbols.get(target_class).name != source_type.name
        {
            return Err(PickleError::FieldType {
                symbol: field.symbol,
                expected: field.declared_type,
                actual: typed_root.ty,
            });
        }
        Ok(None)
    }

    fn validate_field(
        &mut self,
        field: ClassFieldPicklePlan,
        class: SymbolId,
        constructor: &PrimaryConstructorInputPlan,
        fields_seen: &HashSet<SymbolId>,
    ) -> Result<Option<SameBatchReferenceFieldData>, PickleError> {
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
        let reference_field = if !is_int && !is_boolean {
            let is_var = definition.metadata.modifiers.as_slice() == [Modifier::Var];
            if is_var {
                return Err(PickleError::Unsupported {
                    feature: UnsupportedFeature::ClassField,
                    symbol: Some(field.symbol),
                });
            }
            Some(self.validate_reference_field_type(field, class, type_syntax.name)?)
        } else {
            None
        };
        let expected_name = self
            .plan
            .store
            .types
            .get(field.declared_type)
            .reference_symbol()
            .map(|symbol| self.plan.store.symbols.get(symbol).name);
        if expected_name.is_none_or(|expected| expected != type_syntax.name) {
            return Err(PickleError::FieldType {
                symbol: field.symbol,
                expected: field.declared_type,
                actual: self.completed(field.symbol)?,
            });
        }
        let actual_type = self.completed(field.symbol)?;
        let field_type_matches = if reference_field.is_some() {
            self.same_exact_type_ref(actual_type, field.declared_type)
        } else {
            self.same_canonical_type(actual_type, field.declared_type)
        };
        if !field_type_matches {
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
        if let Some(reference_field) = reference_field {
            let template_parameter_alias = self.validate_pending_reference_initializer(
                field,
                constructor,
                reference_field.target_class,
                source_rhs,
            )?;
            if template_parameter_alias.is_none()
                && !self.same_type_ref_target(typed.ty, field.declared_type)
            {
                return Err(PickleError::FieldType {
                    symbol: field.symbol,
                    expected: field.declared_type,
                    actual: typed.ty,
                });
            }
            return Ok(Some(SameBatchReferenceFieldData {
                template_parameter_alias,
                ..reference_field
            }));
        }
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
        Ok(None)
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
    same_batch_reference_fields: Vec<SameBatchReferenceFieldData>,
}

impl<'p, 'a> ValidatedClassStatePlan<'p, 'a> {
    pub fn plan(&self) -> &'p ClassStatePicklePlan<'a> {
        self.plan
    }

    /// Validate and retain the source and semantic roots for a distinct
    /// same-batch class reference signature. This deliberately does not
    /// inspect or authorize serialization of the method body.
    pub fn distinct_reference_method_signature(
        &self,
        method: SymbolId,
    ) -> Result<DistinctReferenceMethodSignature, PickleError> {
        let p = self.plan;
        let source_tree = p
            .members
            .iter()
            .find_map(|member| match member {
                ClassMemberPicklePlan::Method {
                    symbol,
                    source_tree,
                } if *symbol == method => Some(*source_tree),
                _ => None,
            })
            .ok_or(PickleError::SourceDefinition { symbol: method })?;
        let tree = p
            .source_ast
            .try_get(source_tree)
            .ok_or(PickleError::InvalidSourceTree { tree: source_tree })?;
        let TreeKind::DefDef(definition) = &tree.kind else {
            return Err(PickleError::SourceShape {
                tree: source_tree,
                expected: SourceRole::Method,
            });
        };
        if !p.store.symbols.contains(method) {
            return Err(PickleError::InvalidSymbol { symbol: method });
        }
        let method_record = p.store.symbols.get(method);
        if method_record.kind != SymbolKind::Method
            || method_record.owner != Some(p.class)
            || method_record.origin != SymbolOrigin::Source(p.source.id)
            || method_record.visibility != Visibility::Public
            || !method_record.flags.is_empty()
            || !method_record.annotations.is_empty()
            || p.source_index.symbol_at(p.source.id, source_tree) != Some(method)
            || p.source_index.definition_of(method)
                != Some(SourceDefinition::Canonical {
                    source: p.source.id,
                    tree: source_tree,
                })
            || !definition.type_params.is_empty()
            || definition.value_param_clauses.len() != 1
            || definition.value_param_clauses[0].len() != 1
            || definition.source_param_clause_order.is_some()
            || definition.metadata.visibility.is_some()
            || !definition.metadata.modifiers.is_empty()
            || !definition.metadata.annotations.is_empty()
        {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::ClassMembers,
                symbol: Some(method),
            });
        }
        let parameter_tree = definition.value_param_clauses[0][0];
        let parameter_node =
            p.source_ast
                .try_get(parameter_tree)
                .ok_or(PickleError::InvalidSourceTree {
                    tree: parameter_tree,
                })?;
        let TreeKind::ValDef(parameter_definition) = &parameter_node.kind else {
            return Err(PickleError::SourceShape {
                tree: parameter_tree,
                expected: SourceRole::Parameter,
            });
        };
        let parameter = p
            .source_index
            .symbol_at(p.source.id, parameter_tree)
            .ok_or(PickleError::SourceDefinition { symbol: method })?;
        if !p.store.symbols.contains(parameter) {
            return Err(PickleError::InvalidSymbol { symbol: parameter });
        }
        let parameter_record = p.store.symbols.get(parameter);
        let signature_type = match method_record.info {
            SymbolInfo::Complete(ty) => ty,
            info => {
                return Err(PickleError::IncompleteSymbol {
                    symbol: method,
                    info,
                });
            }
        };
        let Type::Method(signature) = p
            .store
            .types
            .try_get(signature_type)
            .ok_or(PickleError::InvalidType { ty: signature_type })?
        else {
            return Err(PickleError::MalformedType {
                ty: signature_type,
                role: TypeRole::Parameter,
            });
        };
        if signature.kind != MethodKind::Plain
            || signature.params.len() != 1
            || signature.params[0].erased
            || signature.params[0].varargs
            || signature.params[0].name.as_name() != parameter_definition.name.as_name()
            || parameter_record.kind != SymbolKind::Parameter
            || parameter_record.owner != Some(method)
            || parameter_record.origin != SymbolOrigin::Source(p.source.id)
            || parameter_record.name != *parameter_definition.name.as_name()
            || parameter_record.visibility != Visibility::Public
            || !parameter_record.flags.is_empty()
            || !parameter_record.annotations.is_empty()
            || p.source_index.definition_of(parameter)
                != Some(SourceDefinition::Canonical {
                    source: p.source.id,
                    tree: parameter_tree,
                })
        {
            return Err(PickleError::MalformedType {
                ty: signature_type,
                role: TypeRole::Parameter,
            });
        }
        if parameter_definition.rhs.is_some()
            || parameter_definition.metadata.visibility.is_some()
            || !(parameter_definition.metadata.modifiers.is_empty()
                || parameter_definition.metadata.modifiers.as_slice() == [Modifier::Param])
            || !parameter_definition.metadata.annotations.is_empty()
        {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::ParameterClauses,
                symbol: Some(parameter),
            });
        }
        let parameter_type = signature.params[0].ty;
        let result_type = signature.result;
        let parameter_signature = match parameter_record.info {
            SymbolInfo::Complete(ty) => ty,
            info => {
                return Err(PickleError::IncompleteSymbol {
                    symbol: parameter,
                    info,
                });
            }
        };
        if parameter_signature != parameter_type {
            return Err(PickleError::MalformedType {
                ty: signature_type,
                role: TypeRole::Parameter,
            });
        }
        let class_target = |ty: TypeId| match p.store.types.try_get(ty) {
            Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(class),
                ..
            }) => Some(*class),
            _ => None,
        };
        let parameter_class = class_target(parameter_type).ok_or(PickleError::MalformedType {
            ty: parameter_type,
            role: TypeRole::Parameter,
        })?;
        let result_class = class_target(result_type).ok_or(PickleError::MalformedType {
            ty: result_type,
            role: TypeRole::MethodResult,
        })?;
        if parameter_class == result_class {
            return Err(PickleError::MalformedType {
                ty: result_type,
                role: TypeRole::MethodResult,
            });
        }
        let planned = p.typed_bindings.planned_class_symbols();
        let class_tree = |class: SymbolId| -> Result<TreeId<Untyped>, PickleError> {
            if !planned.contains(&class) {
                return Err(PickleError::SourceDefinition { symbol: class });
            }
            match p.source_index.definition_of(class) {
                Some(SourceDefinition::Canonical { source, tree }) if source == p.source.id => {
                    Ok(tree)
                }
                _ => Err(PickleError::SourceDefinition { symbol: class }),
            }
        };
        let parameter_class_tree = class_tree(parameter_class)?;
        let result_class_tree = class_tree(result_class)?;
        let package = p.store.symbols.get(p.class).owner;
        for class in [parameter_class, result_class] {
            if !p.store.symbols.contains(class) {
                return Err(PickleError::InvalidSymbol { symbol: class });
            }
            let record = p.store.symbols.get(class);
            if record.kind != SymbolKind::Class
                || record.owner != package
                || record.origin != SymbolOrigin::Source(p.source.id)
                || record.visibility != Visibility::Public
                || !record.flags.is_empty()
                || !record.annotations.is_empty()
            {
                return Err(PickleError::SourceDefinition { symbol: class });
            }
        }
        for (ty, class) in [
            (parameter_type, parameter_class),
            (result_type, result_class),
        ] {
            let Some(Type::TypeRef {
                prefix,
                target: TypeRefTarget::Symbol(target),
            }) = p.store.types.try_get(ty)
            else {
                return Err(PickleError::UnsupportedTypeReference { ty });
            };
            let owner = p.store.symbols.get(class).owner.ok_or(PickleError::Owner {
                symbol: class,
                expected: None,
                actual: None,
            })?;
            if *target != class
                || !matches!(p.store.types.try_get(*prefix), Some(Type::TypeRef {
                    prefix: package_prefix, target: TypeRefTarget::Symbol(package)
                }) if *package_prefix == p.definitions.no_prefix && *package == owner)
            {
                return Err(PickleError::UnsupportedTypeReference { ty });
            }
            let class_type = match p.store.symbols.get(class).info {
                SymbolInfo::Complete(class_type) => class_type,
                info => {
                    return Err(PickleError::IncompleteSymbol {
                        symbol: class,
                        info,
                    });
                }
            };
            let Some(Type::ClassInfo(info)) = p.store.types.try_get(class_type) else {
                return Err(PickleError::MalformedType {
                    ty: class_type,
                    role: TypeRole::ClassInfo,
                });
            };
            if info.class != class
                || info.prefix != p.definitions.no_prefix
                || info.parents.as_slice() != [p.definitions.object_type]
                || info.self_type.is_some()
                || p.source_index.scope_of(class) != Some(info.declarations)
                || !p.store.scopes.contains(info.declarations)
                || p.store
                    .scopes
                    .get(info.declarations)
                    .entered_symbols()
                    .any(|member| {
                        p.store.symbols.contains(member)
                            && p.store.symbols.get(member).kind == SymbolKind::TypeParameter
                    })
            {
                return Err(PickleError::MalformedType {
                    ty: class_type,
                    role: TypeRole::ClassInfo,
                });
            }
        }
        let TreeKind::Ident(parameter_syntax) = &p
            .source_ast
            .try_get(parameter_definition.tpt)
            .ok_or(PickleError::InvalidSourceTree {
                tree: parameter_definition.tpt,
            })?
            .kind
        else {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::ParameterClauses,
                symbol: Some(parameter),
            });
        };
        let TreeKind::Ident(result_syntax) = &p
            .source_ast
            .try_get(definition.tpt)
            .ok_or(PickleError::InvalidSourceTree {
                tree: definition.tpt,
            })?
            .kind
        else {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::ResultTypeSyntax,
                symbol: Some(method),
            });
        };
        if parameter_syntax.name != p.store.symbols.get(parameter_class).name
            || result_syntax.name != p.store.symbols.get(result_class).name
        {
            return Err(PickleError::MalformedType {
                ty: result_type,
                role: TypeRole::MethodResult,
            });
        }
        let source_rhs = definition.rhs.ok_or(PickleError::MissingBody { method })?;
        let source_rhs_node = p
            .source_ast
            .try_get(source_rhs)
            .ok_or(PickleError::InvalidSourceTree { tree: source_rhs })?;
        let typed_rhs = p
            .typed_bindings
            .typed_at(p.source.id, source_rhs)
            .ok_or(PickleError::MissingBody { method })?;
        let typed_rhs_node = p
            .typed_ast
            .try_get(typed_rhs)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_rhs })?;
        let source_position = source_rhs_node.position;
        let typed_position = typed_rhs_node.position;
        if source_position.is_none()
            || typed_position.is_none()
            || source_position != typed_position
            || source_position.is_some_and(|position| position.source() != p.source.id)
        {
            return Err(PickleError::MethodBodySpan {
                source_tree: source_rhs,
                typed_tree: typed_rhs,
            });
        }
        if !matches!(
            p.store.types.try_get(typed_rhs_node.ty),
            Some(Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }) if *class == result_class
        ) {
            return Err(PickleError::MalformedType {
                ty: typed_rhs_node.ty,
                role: TypeRole::Body,
            });
        }
        Ok(DistinctReferenceMethodSignature {
            method,
            source_tree,
            parameter,
            parameter_tree,
            parameter_type,
            parameter_class,
            parameter_class_tree,
            parameter_type_syntax: parameter_definition.tpt,
            result_type,
            result_class,
            result_class_tree,
            result_type_syntax: definition.tpt,
            source_rhs,
            typed_rhs,
        })
    }

    /// Validate and project only the `PARAM` and result type roots for one
    /// distinct same-batch reference signature. No method body is encoded.
    pub fn project_distinct_reference_method_signature(
        &self,
        method: SymbolId,
    ) -> Result<crate::ProjectedMethodSignature, PickleError> {
        let signature = self.distinct_reference_method_signature(method)?;
        let p = self.plan;
        let mut context = crate::EncodingContext::new(
            p.store,
            p.definitions,
            p.source_index,
            p.source.id,
            crate::ProjectionLimits::default(),
        )?;
        context.distinct_reference_method_signature(self, &signature)
    }

    /// Read-only proofs for immutable reference fields whose target class is
    /// part of this exact source batch. The initializer roots remain pending
    /// and are not passed to the expression serializer by this API.
    pub fn same_batch_reference_fields(
        &self,
    ) -> impl ExactSizeIterator<Item = ValidatedSameBatchReferenceField<'_, 'a>> + '_ {
        self.same_batch_reference_fields
            .iter()
            .copied()
            .map(|data| ValidatedSameBatchReferenceField {
                plan: self.plan,
                data,
            })
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
        let same_batch_reference_fields = ClassStateValidator {
            plan: self,
            limits,
            remaining: limits.max_items,
        }
        .validate()?;
        Ok(ValidatedClassStatePlan {
            plan: self,
            same_batch_reference_fields,
        })
    }
}
