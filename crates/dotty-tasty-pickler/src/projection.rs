//! Canonical semantics to owned wire names and fresh labelled type fragments.

use std::collections::{HashMap, HashSet};

use dotty_core::*;
use dotty_tasty::tasty::*;

use crate::{Builtin, LimitKind, PickleError, ReferenceError, ValidatedPicklePlan};

mod primitive_operator;

/// Budgets for semantic projection; type and expression recursion have hard
/// ceilings of 256 for stack safety.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectionLimits {
    pub max_type_depth: usize,
    pub max_owner_depth: usize,
    pub max_expression_depth: usize,
    pub max_items: usize,
    pub max_names: usize,
    pub max_name_bytes: usize,
    pub wire: AstBuildLimits,
}

impl Default for ProjectionLimits {
    fn default() -> Self {
        Self {
            max_type_depth: 256,
            max_owner_depth: 256,
            max_expression_depth: 256,
            max_items: 1_000_000,
            max_names: 100_000,
            max_name_bytes: 4096,
            wire: AstBuildLimits::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum NameKey {
    Utf8(String),
    Qualified(NameRef, NameRef),
    SignedEmpty(NameRef, NameRef),
    SignedMany(NameRef, NameRef, Vec<i32>),
}

/// Owned wire result only; no semantic records are copied and no file is assembled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectedFragment {
    pub names: NameTable,
    pub ast: BuiltAst,
}

/// One compilation session's canonical identity maps and deterministic wire builder.
///
/// Calls establish emission order; hash maps are lookup-only. Local declarations
/// are reserved before references and bound once by the downstream tree producer.
/// Each returned label must occur exactly once among finish roots/child fields.
/// Repeated TypeIds produce backward SHAREDtype nodes, so callers must preserve
/// projection order. On any mutating-call error the context is poisoned; discard
/// it and start again. No partial result or semantic mutation is exposed.
pub struct EncodingContext<'a> {
    store: &'a SemanticStore,
    definitions: &'a Definitions,
    source_index: &'a SourceSemanticIndex,
    source: SourceId,
    limits: ProjectionLimits,
    remaining: usize,
    failed: bool,
    names: NameTableBuilder,
    name_refs: HashMap<NameKey, NameRef>,
    wire: AstBuilder,
    declarations: HashMap<SymbolId, AstLabel>,
    local_values_by_typed_declaration: HashMap<TreeId<Typed>, SymbolId>,
    local_value_symbols: HashSet<SymbolId>,
    external: HashMap<SymbolId, (NameRef, NameRef)>,
    same_source_external: HashSet<SymbolId>,
    types: HashMap<TypeId, AstLabel>,
    active: HashSet<TypeId>,
}

fn construction(error: AstBuildError) -> PickleError {
    PickleError::Construction {
        cause: Box::new(error),
    }
}
fn encoding(error: impl Into<TastyFileError>) -> PickleError {
    PickleError::Encoding {
        cause: Box::new(error.into()),
    }
}
fn reference(symbol: SymbolId, reason: ReferenceError) -> PickleError {
    PickleError::Reference { symbol, reason }
}

impl<'a> EncodingContext<'a> {
    pub(crate) fn definitions(&self) -> &'a Definitions {
        self.definitions
    }

    /// Standalone entry point. IDs, index and Definitions must come from these
    /// borrowed arenas in one session. Each supplied symbol/type is checked.
    pub fn new(
        store: &'a SemanticStore,
        definitions: &'a Definitions,
        source_index: &'a SourceSemanticIndex,
        source: SourceId,
        limits: ProjectionLimits,
    ) -> Result<Self, PickleError> {
        let mut context = Self {
            store,
            definitions,
            source_index,
            source,
            limits,
            remaining: limits.max_items,
            failed: false,
            names: NameTableBuilder::new(),
            name_refs: HashMap::new(),
            wire: AstBuilder::new(limits.wire),
            declarations: HashMap::new(),
            local_values_by_typed_declaration: HashMap::new(),
            local_value_symbols: HashSet::new(),
            external: HashMap::new(),
            same_source_external: HashSet::new(),
            types: HashMap::new(),
            active: HashSet::new(),
        };
        if !matches!(context.ty(definitions.no_prefix)?, Type::NoPrefix) {
            return Err(PickleError::MalformedType {
                ty: definitions.no_prefix,
                role: crate::TypeRole::NoPrefix,
            });
        }
        for builtin in [
            Builtin::Int,
            Builtin::Boolean,
            Builtin::Unit,
            Builtin::Object,
        ] {
            context.builtin(builtin)?;
        }
        Ok(context)
    }
    /// Seeds declaration reservations from the already validated source plan.
    pub fn for_plan(
        plan: &ValidatedPicklePlan<'_, 'a>,
        limits: ProjectionLimits,
    ) -> Result<Self, PickleError> {
        Self::for_plan_with_local_values(plan, limits, true)
    }

    pub(crate) fn for_plan_with_local_values(
        plan: &ValidatedPicklePlan<'_, 'a>,
        limits: ProjectionLimits,
        include_local_values: bool,
    ) -> Result<Self, PickleError> {
        let p = plan.plan();
        let mut context = Self::new(p.store, p.definitions, p.source_index, p.source.id, limits)?;
        context.reserve_declaration(p.class)?;
        context.reserve_declaration(p.constructor.symbol)?;
        let constructor_tree = p.source_ast.try_get(p.constructor.source_tree).ok_or(
            PickleError::InvalidSourceTree {
                tree: p.constructor.source_tree,
            },
        )?;
        let dotty_core::ast::TreeKind::DefDef(constructor_definition) = &constructor_tree.kind
        else {
            return Err(PickleError::SourceShape {
                tree: p.constructor.source_tree,
                expected: crate::SourceRole::Constructor,
            });
        };
        for (source_clause, symbols) in constructor_definition
            .value_param_clauses
            .iter()
            .zip(&p.constructor.parameter_clauses)
        {
            for (&tree, &parameter) in source_clause.iter().zip(symbols) {
                let template_parameter = p
                    .source_index
                    .symbol_at(p.source.id, tree)
                    .ok_or(PickleError::SourceDefinition { symbol: parameter })?;
                context.reserve_template_parameter(
                    template_parameter,
                    p.class,
                    p.source.id,
                    tree,
                )?;
                context.reserve_primary_constructor_parameter(
                    parameter,
                    p.constructor.symbol,
                    p.source.id,
                    tree,
                )?;
            }
        }
        for method in p.methods {
            context.reserve_declaration(method.symbol)?;
            for parameter in method.parameter_clauses.iter().flatten() {
                context.reserve_declaration(*parameter)?;
            }
        }
        if include_local_values {
            for local in p.local_values {
                let owner = context
                    .symbol(local.symbol)?
                    .owner
                    .ok_or(reference(local.symbol, ReferenceError::Owner))?;
                context.reserve_local_value(local.symbol, owner, local.scope)?;
                context.local_value_symbols.insert(local.symbol);
                if context
                    .local_values_by_typed_declaration
                    .insert(local.typed_declaration, local.symbol)
                    .is_some()
                {
                    return Err(PickleError::DuplicateDeclaration {
                        symbol: local.symbol,
                    });
                }
            }
        }
        Ok(context)
    }

    pub(crate) fn reserve_local_value(
        &mut self,
        symbol: SymbolId,
        method: SymbolId,
        scope_id: ScopeId,
    ) -> Result<AstLabel, PickleError> {
        self.run(|context| {
            context.charge(1)?;
            let record = context.symbol(symbol)?;
            if record.kind != SymbolKind::Local
                || record.origin != SymbolOrigin::Source(context.source)
                || record.owner != Some(method)
                || record.visibility != Visibility::Public
                || !(record.flags.is_empty() || record.flags == SymbolFlags::MUTABLE)
                || !record.annotations.is_empty()
            {
                return Err(reference(symbol, ReferenceError::Metadata));
            }
            context.namespace(symbol, false)?;
            context.text(record.name.text())?;
            context.complete(symbol)?;
            if !context.store.scopes.contains(scope_id) {
                return Err(PickleError::InvalidScope { scope: scope_id });
            }
            let scope = context.store.scopes.get(scope_id);
            if scope.owner != Some(method) || scope.lookup_all(&record.name) != [symbol] {
                return Err(reference(symbol, ReferenceError::Owner));
            }
            if context.declarations.contains_key(&symbol) {
                return Err(PickleError::DuplicateDeclaration { symbol });
            }
            let label = context.wire.reserve().map_err(construction)?;
            context.declarations.insert(symbol, label);
            Ok(label)
        })
    }

    pub(crate) fn local_value_symbol(&self, typed_declaration: TreeId<Typed>) -> Option<SymbolId> {
        self.local_values_by_typed_declaration
            .get(&typed_declaration)
            .copied()
    }

    pub(crate) fn reserve_local_binding(
        &mut self,
        binding: crate::LocalValueBinding,
    ) -> Result<(), PickleError> {
        let owner = self
            .symbol(binding.symbol)?
            .owner
            .ok_or(reference(binding.symbol, ReferenceError::Owner))?;
        self.reserve_local_value(binding.symbol, owner, binding.scope)?;
        self.local_value_symbols.insert(binding.symbol);
        if self
            .local_values_by_typed_declaration
            .insert(binding.typed_declaration, binding.symbol)
            .is_some()
        {
            return Err(PickleError::DuplicateDeclaration {
                symbol: binding.symbol,
            });
        }
        Ok(())
    }
    fn run<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, PickleError>,
    ) -> Result<T, PickleError> {
        if self.failed {
            return Err(PickleError::ProjectionAborted);
        }
        let result = operation(self);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn charge(&mut self, count: usize) -> Result<(), PickleError> {
        self.remaining = self
            .remaining
            .checked_sub(count)
            .ok_or(PickleError::LimitExceeded {
                kind: LimitKind::Items,
                limit: self.limits.max_items,
            })?;
        Ok(())
    }
    fn text(&self, name: NameId) -> Result<&'a str, PickleError> {
        let text = self
            .store
            .names
            .try_resolve(name)
            .ok_or(PickleError::InvalidName { name })?;
        if text.len() > self.limits.max_name_bytes {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::NameBytes,
                limit: self.limits.max_name_bytes,
            });
        }
        if text.is_empty() || text.contains(['.', '/', '\\', '\0']) {
            return Err(PickleError::Unsupported {
                feature: crate::UnsupportedFeature::Name,
                symbol: None,
            });
        }
        Ok(text)
    }
    fn symbol(&self, symbol: SymbolId) -> Result<&'a Symbol, PickleError> {
        if !self.store.symbols.contains(symbol) {
            return Err(PickleError::InvalidSymbol { symbol });
        }
        let found = self.store.symbols.get(symbol);
        self.store
            .names
            .try_resolve(found.name.text())
            .ok_or(PickleError::InvalidName {
                name: found.name.text(),
            })?;
        Ok(found)
    }
    fn ty(&self, ty: TypeId) -> Result<&'a Type, PickleError> {
        let value = self.store.types.try_get(ty).ok_or_else(|| {
            if self.store.types.contains(ty) {
                PickleError::IncompleteType { ty }
            } else {
                PickleError::InvalidType { ty }
            }
        })?;
        if matches!(value, Type::NoType | Type::Error(_)) {
            return Err(PickleError::IncompleteType { ty });
        }
        Ok(value)
    }
    fn complete(&self, symbol: SymbolId) -> Result<TypeId, PickleError> {
        let info = self.symbol(symbol)?.info;
        let SymbolInfo::Complete(ty) = info else {
            return Err(PickleError::IncompleteSymbol { symbol, info });
        };
        self.ty(ty)?;
        Ok(ty)
    }
    fn class_scope(&self, symbol: SymbolId) -> Result<ScopeId, PickleError> {
        let ty = self.complete(symbol)?;
        let Type::ClassInfo(info) = self.ty(ty)? else {
            return Err(PickleError::MalformedType {
                ty,
                role: crate::TypeRole::ClassInfo,
            });
        };
        if info.class != symbol || info.prefix != self.definitions.no_prefix {
            return Err(PickleError::MalformedType {
                ty,
                role: crate::TypeRole::ClassInfo,
            });
        }
        if !self.store.scopes.contains(info.declarations) {
            return Err(PickleError::InvalidScope {
                scope: info.declarations,
            });
        }
        if self.store.scopes.get(info.declarations).owner != Some(symbol) {
            return Err(reference(symbol, ReferenceError::Owner));
        }
        Ok(info.declarations)
    }
    fn builtin(&mut self, builtin: Builtin) -> Result<SymbolId, PickleError> {
        self.charge(1)?;
        let (ty, expected) = match builtin {
            Builtin::Int => (self.definitions.int, "Int"),
            Builtin::Boolean => (self.definitions.boolean, "Boolean"),
            Builtin::Unit => (self.definitions.unit, "Unit"),
            Builtin::Object => (self.definitions.object_type, "Object"),
        };
        let Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(symbol),
        } = self.ty(ty)?
        else {
            return Err(PickleError::BuiltinIdentity { builtin, ty });
        };
        let s = self.symbol(*symbol)?;
        if *prefix != self.definitions.no_prefix
            || s.origin != SymbolOrigin::Builtin
            || s.owner.is_some()
            || s.kind != SymbolKind::Class
            || !s.name.is_type()
            || self.text(s.name.text())? != expected
            || s.visibility != Visibility::Public
            || !s.flags.is_empty()
            || !s.annotations.is_empty()
            || (builtin == Builtin::Object && *symbol != self.definitions.object_class)
        {
            return Err(PickleError::BuiltinIdentity { builtin, ty });
        }
        Ok(*symbol)
    }
    fn intern(&mut self, key: NameKey) -> Result<NameRef, PickleError> {
        self.charge(1)?;
        if let Some(&reference) = self.name_refs.get(&key) {
            return Ok(reference);
        }
        if self.names.len() >= self.limits.max_names {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::Names,
                limit: self.limits.max_names,
            });
        }
        let raw = match &key {
            NameKey::Utf8(text) => RawName::Utf8(text.clone()),
            NameKey::Qualified(prefix, selector) => RawName::Qualified {
                prefix: *prefix,
                selector: *selector,
            },
            NameKey::SignedEmpty(original, result_signature) => RawName::Signed {
                original: *original,
                result_signature: *result_signature,
                parameter_signatures: vec![],
            },
            NameKey::SignedMany(original, result_signature, parameters) => RawName::Signed {
                original: *original,
                result_signature: *result_signature,
                parameter_signatures: parameters.clone(),
            },
        };
        let reference = self.names.intern(raw).map_err(encoding)?;
        self.name_refs.insert(key, reference);
        Ok(reference)
    }
    fn utf8(&mut self, text: &str) -> Result<NameRef, PickleError> {
        if text.len() > self.limits.max_name_bytes {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::NameBytes,
                limit: self.limits.max_name_bytes,
            });
        }
        self.intern(NameKey::Utf8(text.to_owned()))
    }
    /// Interns ordinary names; term/type namespaces share bytes as in NameBuffer.
    /// Namespace is retained by reference tags, never by inventing new spelling.
    pub fn intern_name(&mut self, name: Name) -> Result<NameRef, PickleError> {
        self.run(|c| c.utf8(c.text(name.text())?))
    }
    /// Bounded wire metadata, which is not an ordinary semantic identifier.
    pub(crate) fn intern_wire_text(&mut self, text: &str) -> Result<NameRef, PickleError> {
        self.run(|c| c.utf8(text))
    }
    fn package_name(&mut self, package: SymbolId) -> Result<NameRef, PickleError> {
        let mut path = vec![];
        let mut seen = HashSet::new();
        let mut at = Some(package);
        while let Some(id) = at {
            self.charge(1)?;
            if path.len() >= self.limits.max_owner_depth {
                return Err(PickleError::LimitExceeded {
                    kind: LimitKind::OwnerDepth,
                    limit: self.limits.max_owner_depth,
                });
            }
            if !seen.insert(id) {
                return Err(PickleError::OwnerCycle { symbol: id });
            }
            let s = self.symbol(id)?;
            if s.kind != SymbolKind::Package || !s.name.is_term() {
                return Err(reference(id, ReferenceError::Owner));
            }
            let text =
                self.store
                    .names
                    .try_resolve(s.name.text())
                    .ok_or(PickleError::InvalidName {
                        name: s.name.text(),
                    })?;
            if s.owner.is_none() {
                if !text.is_empty() || path.is_empty() {
                    return Err(reference(id, ReferenceError::Owner));
                }
                break;
            }
            path.push(self.text(s.name.text())?);
            at = s.owner;
        }
        let mut path = path.into_iter().rev();
        let first = path
            .next()
            .ok_or(reference(package, ReferenceError::Owner))?;
        let mut name = self.utf8(first)?;
        for segment in path {
            let selector = self.utf8(segment)?;
            name = self.intern(NameKey::Qualified(name, selector))?;
        }
        Ok(name)
    }
    fn leaf(&mut self, tag: u8, value: TermValue) -> Result<AstLabel, PickleError> {
        let leaf = SimpleTerm::new(tag, value).map_err(encoding)?;
        self.wire.push(AstNode::Leaf(leaf)).map_err(construction)
    }
    fn package_inner(&mut self, package: SymbolId, is_type: bool) -> Result<AstLabel, PickleError> {
        let name = self.package_name(package)?;
        self.leaf(
            if is_type {
                TYPEREFPKG_TAG
            } else {
                TERMREFPKG_TAG
            },
            TermValue::NameRef(name),
        )
    }
    /// Projects a canonical package path as TERMREFpkg, in owner order.
    pub fn package_reference(&mut self, package: SymbolId) -> Result<AstLabel, PickleError> {
        self.run(|c| c.package_inner(package, false))
    }
    fn namespace(&self, symbol: SymbolId, is_type: bool) -> Result<(), PickleError> {
        let s = self.symbol(symbol)?;
        if s.name.is_type() != is_type {
            return Err(reference(symbol, ReferenceError::Namespace));
        }
        let valid = if is_type {
            matches!(s.kind, SymbolKind::Class | SymbolKind::TypeParameter)
        } else {
            matches!(
                s.kind,
                SymbolKind::Method
                    | SymbolKind::Constructor
                    | SymbolKind::Value
                    | SymbolKind::Field
                    | SymbolKind::Parameter
                    | SymbolKind::Local
            )
        };
        if !valid {
            return Err(reference(symbol, ReferenceError::Kind));
        }
        Ok(())
    }
    fn local(&mut self, symbol: SymbolId) -> Result<(), PickleError> {
        self.charge(1)?;
        let s = self.symbol(symbol)?;
        if s.origin != SymbolOrigin::Source(self.source) {
            return Err(reference(symbol, ReferenceError::NotLocal));
        }
        let Some(SourceDefinition::Canonical { source, tree }) =
            self.source_index.definition_of(symbol)
        else {
            return Err(PickleError::SourceDefinition { symbol });
        };
        if source != self.source || self.source_index.symbol_at(source, tree) != Some(symbol) {
            return Err(PickleError::SourceDefinition { symbol });
        }
        self.namespace(symbol, s.name.is_type())?;
        self.text(s.name.text())?;
        let flags_are_supported = s.flags.is_empty()
            || (s.flags == SymbolFlags::MUTABLE && self.local_value_symbols.contains(&symbol));
        if s.visibility != Visibility::Public || !flags_are_supported || !s.annotations.is_empty() {
            return Err(reference(symbol, ReferenceError::Metadata));
        }
        self.complete(symbol)?;
        if s.kind == SymbolKind::Class
            && self.source_index.scope_of(symbol) != Some(self.class_scope(symbol)?)
        {
            return Err(reference(symbol, ReferenceError::Owner));
        }
        let direct_owner = s.owner.ok_or(reference(symbol, ReferenceError::Owner))?;
        let scope_id = self
            .source_index
            .scope_of(direct_owner)
            .ok_or(reference(symbol, ReferenceError::Owner))?;
        if !self.store.scopes.contains(scope_id) {
            return Err(PickleError::InvalidScope { scope: scope_id });
        }
        let scope = self.store.scopes.get(scope_id);
        if scope.owner != Some(direct_owner) {
            return Err(reference(symbol, ReferenceError::Owner));
        }
        if self.symbol(direct_owner)?.kind == SymbolKind::Class
            && self.class_scope(direct_owner)? != scope_id
        {
            return Err(reference(symbol, ReferenceError::Owner));
        }
        if scope.lookup_all(&s.name) != [symbol] {
            return Err(reference(symbol, ReferenceError::Ambiguous));
        }
        let mut seen = HashSet::from([symbol]);
        let mut owner = s.owner.ok_or(reference(symbol, ReferenceError::Owner))?;
        for _ in 0..self.limits.max_owner_depth {
            self.charge(1)?;
            if !seen.insert(owner) {
                return Err(PickleError::OwnerCycle { symbol: owner });
            }
            let parent = self.symbol(owner)?;
            if parent.kind == SymbolKind::Package {
                self.package_name(owner)?;
                return Ok(());
            }
            if parent.origin != SymbolOrigin::Source(self.source)
                || !matches!(
                    parent.kind,
                    SymbolKind::Class | SymbolKind::Method | SymbolKind::Constructor
                )
            {
                return Err(reference(symbol, ReferenceError::Owner));
            }
            owner = parent
                .owner
                .ok_or(reference(symbol, ReferenceError::Owner))?;
        }
        Err(PickleError::LimitExceeded {
            kind: LimitKind::OwnerDepth,
            limit: self.limits.max_owner_depth,
        })
    }
    /// Reserves the canonical declaration identity before any forward reference.
    pub fn reserve_declaration(&mut self, symbol: SymbolId) -> Result<AstLabel, PickleError> {
        self.run(|c| {
            c.local(symbol)?;
            if c.declarations.contains_key(&symbol) {
                return Err(PickleError::DuplicateDeclaration { symbol });
            }
            let label = c.wire.reserve().map_err(construction)?;
            c.declarations.insert(symbol, label);
            Ok(label)
        })
    }

    /// Reserves the private class-owned PARAM used by Scala's template header
    /// for a plain primary-constructor parameter. Its public constructor-owned
    /// derived identity is serialized separately inside `<init>`.
    pub(crate) fn reserve_template_parameter(
        &mut self,
        symbol: SymbolId,
        class: SymbolId,
        source: SourceId,
        tree: dotty_core::TreeId<dotty_core::Untyped>,
    ) -> Result<AstLabel, PickleError> {
        self.run(|context| {
            context.charge(1)?;
            let record = context.symbol(symbol)?;
            if context.source != source
                || context.source_index.symbol_at(source, tree) != Some(symbol)
                || record.kind != SymbolKind::Parameter
                || record.origin != SymbolOrigin::Source(source)
                || record.owner != Some(class)
                || record.visibility != Visibility::Private
                || !record.flags.is_empty()
                || !record.annotations.is_empty()
            {
                return Err(reference(symbol, ReferenceError::Metadata));
            }
            context.namespace(symbol, false)?;
            context.text(record.name.text())?;
            if context.declarations.contains_key(&symbol) {
                return Err(PickleError::DuplicateDeclaration { symbol });
            }
            let label = context.wire.reserve().map_err(construction)?;
            context.declarations.insert(symbol, label);
            Ok(label)
        })
    }

    /// Reserves the exact constructor-owned derived PARAM paired with one
    /// source parameter tree, without treating the template's class-owned
    /// identity as the constructor signature binder.
    pub(crate) fn reserve_primary_constructor_parameter(
        &mut self,
        symbol: SymbolId,
        constructor: SymbolId,
        source: SourceId,
        tree: dotty_core::TreeId<dotty_core::Untyped>,
    ) -> Result<AstLabel, PickleError> {
        self.run(|context| {
            context.charge(1)?;
            let record = context.symbol(symbol)?;
            if context.source != source
                || context
                    .source_index
                    .derived_symbol_at(constructor, source, tree)
                    != Some(symbol)
                || record.kind != SymbolKind::Parameter
                || record.origin != SymbolOrigin::Source(source)
                || record.owner != Some(constructor)
                || record.visibility != Visibility::Public
                || !record.flags.is_empty()
                || !record.annotations.is_empty()
            {
                return Err(reference(symbol, ReferenceError::Metadata));
            }
            context.namespace(symbol, false)?;
            context.text(record.name.text())?;
            context.complete(symbol)?;
            if context.declarations.contains_key(&symbol) {
                return Err(PickleError::DuplicateDeclaration { symbol });
            }
            let label = context.wire.reserve().map_err(construction)?;
            context.declarations.insert(symbol, label);
            Ok(label)
        })
    }

    /// Reserves the canonical class-owned private field identity after its
    /// source/member contract has been validated by `ClassStatePicklePlan`.
    pub fn reserve_class_field(
        &mut self,
        symbol: SymbolId,
        class: SymbolId,
        source_tree: dotty_core::TreeId<dotty_core::Untyped>,
    ) -> Result<AstLabel, PickleError> {
        self.run(|context| {
            context.charge(1)?;
            let record = context.symbol(symbol)?;
            if record.kind != SymbolKind::Field
                || record.origin != SymbolOrigin::Source(context.source)
                || record.owner != Some(class)
                || record.visibility != Visibility::Private
                || !(record.flags.is_empty() || record.flags == SymbolFlags::MUTABLE)
                || !record.annotations.is_empty()
                || context.source_index.symbol_at(context.source, source_tree) != Some(symbol)
            {
                return Err(reference(symbol, ReferenceError::Metadata));
            }
            context.namespace(symbol, false)?;
            context.text(record.name.text())?;
            context.complete(symbol)?;
            if context.declarations.contains_key(&symbol) {
                return Err(PickleError::DuplicateDeclaration { symbol });
            }
            let label = context.wire.reserve().map_err(construction)?;
            context.declarations.insert(symbol, label);
            Ok(label)
        })
    }
    /// Binds an existing declaration to a downstream-supplied wire node. This
    /// increment does not build definition trees. Its tag must match symbol kind.
    pub fn bind_declaration(&mut self, symbol: SymbolId, node: AstNode) -> Result<(), PickleError> {
        self.run(|c| {
            let label = *c
                .declarations
                .get(&symbol)
                .ok_or(reference(symbol, ReferenceError::NotRegistered))?;
            let tag = match c.symbol(symbol)?.kind {
                SymbolKind::Class => TYPEDEF_TAG,
                SymbolKind::TypeParameter => TYPEPARAM_TAG,
                SymbolKind::Method | SymbolKind::Constructor => DEFDEF_TAG,
                SymbolKind::Parameter => PARAM_TAG,
                _ => VALDEF_TAG,
            };
            if node.tag() != tag {
                return Err(reference(symbol, ReferenceError::DeclarationTag));
            }
            c.wire.bind(label, node).map_err(construction)
        })
    }
    /// Returns this context's reserved label for an exact canonical declaration.
    pub fn declaration_label(&self, symbol: SymbolId) -> Option<AstLabel> {
        self.declarations.get(&symbol).copied()
    }
    /// Adds a caller-supplied wire node (e.g. a later definition's body).
    /// Reference leaves/category-four operands use symbolic labels; payload
    /// grammar remains the downstream producer's responsibility.
    pub fn wire_node(&mut self, node: AstNode) -> Result<AstLabel, PickleError> {
        self.run(|c| {
            c.charge(1)?;
            c.wire.push(node).map_err(construction)
        })
    }
    fn local_inner(
        &mut self,
        symbol: SymbolId,
        prefix: Option<TypeId>,
        depth: usize,
    ) -> Result<AstLabel, PickleError> {
        self.charge(1)?;
        let label = *self
            .declarations
            .get(&symbol)
            .ok_or(reference(symbol, ReferenceError::NotRegistered))?;
        let is_type = self.symbol(symbol)?.name.is_type();
        self.namespace(symbol, is_type)?;
        if let Some(prefix) = prefix {
            self.check_prefix(symbol, prefix)?;
            let child = self.lower(prefix, depth + 1)?;
            self.wire
                .push(AstNode::NatAst {
                    tag: if is_type {
                        TYPEREFSYMBOL_TAG
                    } else {
                        TERMREFSYMBOL_TAG
                    },
                    value: AstOperand::Reference(label),
                    child,
                })
                .map_err(construction)
        } else {
            self.wire
                .push(AstNode::Reference {
                    tag: if is_type {
                        TYPEREFDIRECT_TAG
                    } else {
                        TERMREFDIRECT_TAG
                    },
                    target: label,
                })
                .map_err(construction)
        }
    }
    /// References a reserved local declaration, directly or with its checked prefix.
    pub fn local_reference(
        &mut self,
        symbol: SymbolId,
        prefix: Option<TypeId>,
    ) -> Result<AstLabel, PickleError> {
        self.run(|c| c.local_inner(symbol, prefix, 0))
    }

    /// Lowers the frozen Scala 3.9 wire form for an unqualified same-class
    /// method selection using `QUALTHIS` and the canonical class declaration.
    pub fn same_class_method_select(
        &mut self,
        class: SymbolId,
        method: SymbolId,
    ) -> Result<AstLabel, PickleError> {
        self.run(|c| c.same_class_method_select_inner(class, method, false))
    }

    /// Lowers the same-class method selection with an explicit `this` receiver.
    pub fn same_class_method_select_on_this(
        &mut self,
        class: SymbolId,
        method: SymbolId,
    ) -> Result<AstLabel, PickleError> {
        self.run(|c| c.same_class_method_select_inner(class, method, true))
    }

    /// Lowers a class-field read to the Scala 3.9 name-bearing SELECT form.
    /// The canonical field label must already be reserved, but the wire tree
    /// intentionally selects by name from the current class receiver.
    pub(crate) fn same_class_field_select(
        &mut self,
        class: SymbolId,
        field: SymbolId,
        explicit_this: bool,
    ) -> Result<AstLabel, PickleError> {
        self.run(|context| {
            let class_label = *context
                .declarations
                .get(&class)
                .ok_or(reference(class, ReferenceError::NotRegistered))?;
            if !context.declarations.contains_key(&field) {
                return Err(reference(field, ReferenceError::NotRegistered));
            }
            let class_record = context.symbol(class)?;
            let field_record = context.symbol(field)?;
            if class_record.kind != SymbolKind::Class
                || field_record.kind != SymbolKind::Field
                || field_record.owner != Some(class)
                || field_record.origin != SymbolOrigin::Source(context.source)
                || field_record.visibility != Visibility::Private
                || !(field_record.flags.is_empty() || field_record.flags == SymbolFlags::MUTABLE)
                || !field_record.annotations.is_empty()
                || !field_record.name.is_term()
            {
                return Err(reference(field, ReferenceError::Metadata));
            }
            let SymbolInfo::Complete(field_type) = field_record.info else {
                return Err(PickleError::IncompleteSymbol {
                    symbol: field,
                    info: field_record.info,
                });
            };
            if field_type != context.definitions.int && field_type != context.definitions.boolean {
                return Err(PickleError::UnsupportedTypeReference { ty: field_type });
            }
            let package = class_record
                .owner
                .ok_or(reference(class, ReferenceError::Owner))?;
            let package_ref = context.package_reference(package)?;
            let class_name = context.utf8(context.text(class_record.name.text())?)?;
            let type_ref = context
                .wire
                .push(AstNode::NatAst {
                    tag: TYPEREFSYMBOL_TAG,
                    value: AstOperand::Reference(class_label),
                    child: package_ref,
                })
                .map_err(construction)?;
            let receiver = if explicit_this {
                context
                    .wire
                    .push(AstNode::Ast {
                        tag: THIS_TAG,
                        child: type_ref,
                    })
                    .map_err(construction)?
            } else {
                let class_syntax = context
                    .wire
                    .push(AstNode::NatAst {
                        tag: IDENTTPT_TAG,
                        value: AstOperand::Nat(class_name),
                        child: type_ref,
                    })
                    .map_err(construction)?;
                context
                    .wire
                    .push(AstNode::Ast {
                        tag: QUALTHIS_TAG,
                        child: class_syntax,
                    })
                    .map_err(construction)?
            };
            let field_name = context.utf8(context.text(field_record.name.text())?)?;
            context
                .wire
                .push(AstNode::NatAst {
                    tag: SELECT_TAG,
                    value: AstOperand::Nat(field_name),
                    child: receiver,
                })
                .map_err(construction)
        })
    }

    /// Lowers a verified plain Int method application target as SELECTin.
    pub(crate) fn same_class_method_select_in(
        &mut self,
        class: SymbolId,
        method: SymbolId,
        explicit_this: bool,
        parameter_count: usize,
    ) -> Result<AstLabel, PickleError> {
        self.run(|c| {
            if parameter_count > c.limits.max_items {
                return Err(PickleError::LimitExceeded {
                    kind: LimitKind::Items,
                    limit: c.limits.max_items,
                });
            }
            let class_label = *c
                .declarations
                .get(&class)
                .ok_or(reference(class, ReferenceError::NotRegistered))?;
            let record = c.symbol(class)?;
            let method_record = c.symbol(method)?;
            if record.kind != SymbolKind::Class
                || method_record.kind != SymbolKind::Method
                || method_record.owner != Some(class)
            {
                return Err(reference(method, ReferenceError::Owner));
            }
            c.local(method)?;
            let package = record
                .owner
                .ok_or(reference(class, ReferenceError::Owner))?;
            let package_ref = c.package_reference(package)?;
            let owner_package_ref = c.package_reference(package)?;
            let class_name = c.utf8(c.text(record.name.text())?)?;
            let method_name = c.utf8(c.text(method_record.name.text())?)?;
            let scala = c.utf8("scala")?;
            let int = c.utf8("Int")?;
            let int_signature = c.intern(NameKey::Qualified(scala, int))?;
            let parameter_signature =
                i32::try_from(int_signature).map_err(|_| PickleError::LimitExceeded {
                    kind: LimitKind::Names,
                    limit: i32::MAX as usize,
                })?;
            let signed_name = c.intern(NameKey::SignedMany(
                method_name,
                int_signature,
                vec![parameter_signature; parameter_count],
            ))?;
            let type_ref = c
                .wire
                .push(AstNode::NatAst {
                    tag: TYPEREFSYMBOL_TAG,
                    value: AstOperand::Reference(class_label),
                    child: package_ref,
                })
                .map_err(construction)?;
            let receiver = if explicit_this {
                c.wire
                    .push(AstNode::Ast {
                        tag: THIS_TAG,
                        child: type_ref,
                    })
                    .map_err(construction)?
            } else {
                let class_syntax = c
                    .wire
                    .push(AstNode::NatAst {
                        tag: IDENTTPT_TAG,
                        value: AstOperand::Nat(class_name),
                        child: type_ref,
                    })
                    .map_err(construction)?;
                c.wire
                    .push(AstNode::Ast {
                        tag: QUALTHIS_TAG,
                        child: class_syntax,
                    })
                    .map_err(construction)?
            };
            let owner_type = c
                .wire
                .push(AstNode::NatAst {
                    tag: TYPEREFSYMBOL_TAG,
                    value: AstOperand::Reference(class_label),
                    child: owner_package_ref,
                })
                .map_err(construction)?;
            c.wire
                .push(AstNode::Length {
                    tag: SELECTIN_TAG,
                    fields: vec![
                        AstField::Nat(signed_name),
                        AstField::Tree(receiver),
                        AstField::Tree(owner_type),
                    ],
                })
                .map_err(construction)
        })
    }

    fn same_class_method_select_inner(
        &mut self,
        class: SymbolId,
        method: SymbolId,
        explicit_this: bool,
    ) -> Result<AstLabel, PickleError> {
        let class_label = *self
            .declarations
            .get(&class)
            .ok_or(reference(class, ReferenceError::NotRegistered))?;
        let record = self.symbol(class)?;
        let method_record = self.symbol(method)?;
        if record.kind != SymbolKind::Class
            || method_record.kind != SymbolKind::Method
            || method_record.owner != Some(class)
        {
            return Err(reference(method, ReferenceError::Owner));
        }
        self.local(method)?;
        let package = record
            .owner
            .ok_or(reference(class, ReferenceError::Owner))?;
        let package_ref = self.package_reference(package)?;
        let class_name = self.utf8(self.text(record.name.text())?)?;
        let method_name = self.utf8(self.text(method_record.name.text())?)?;
        let type_ref = self
            .wire
            .push(AstNode::NatAst {
                tag: TYPEREFSYMBOL_TAG,
                value: AstOperand::Reference(class_label),
                child: package_ref,
            })
            .map_err(construction)?;
        let receiver = if explicit_this {
            self.wire
                .push(AstNode::Ast {
                    tag: THIS_TAG,
                    child: type_ref,
                })
                .map_err(construction)?
        } else {
            let class_syntax = self
                .wire
                .push(AstNode::NatAst {
                    tag: IDENTTPT_TAG,
                    value: AstOperand::Nat(class_name),
                    child: type_ref,
                })
                .map_err(construction)?;
            self.wire
                .push(AstNode::Ast {
                    tag: QUALTHIS_TAG,
                    child: class_syntax,
                })
                .map_err(construction)?
        };
        self.wire
            .push(AstNode::NatAst {
                tag: SELECT_TAG,
                value: AstOperand::Nat(method_name),
                child: receiver,
            })
            .map_err(construction)
    }
    fn check_prefix(&self, symbol: SymbolId, prefix: TypeId) -> Result<(), PickleError> {
        let owner = self.symbol(symbol)?.owner;
        let target = match self.ty(prefix)? {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(s),
                ..
            }
            | Type::TermRef {
                target: TermRefTarget::Symbol(s),
                ..
            }
            | Type::ThisType { class: s } => Some(*s),
            _ => None,
        };
        if owner.is_none() || owner != target {
            return Err(reference(symbol, ReferenceError::Owner));
        }
        Ok(())
    }
    /// Registers one public package-owned dependency class after exact scope
    /// disambiguation. Arbitrary external method signatures remain deferred.
    pub fn register_external(
        &mut self,
        symbol: SymbolId,
        owner_scope: ScopeId,
    ) -> Result<(), PickleError> {
        self.run(|c| {
            c.charge(1)?;
            c.namespace(symbol, true)?;
            let s = c.symbol(symbol)?;
            if s.origin == SymbolOrigin::Source(c.source) || s.origin == SymbolOrigin::Builtin {
                return Err(reference(symbol, ReferenceError::NotExternal));
            }
            if s.kind != SymbolKind::Class {
                return Err(reference(symbol, ReferenceError::Kind));
            }
            if s.visibility != Visibility::Public
                || !s.flags.is_empty()
                || !s.annotations.is_empty()
            {
                return Err(reference(symbol, ReferenceError::Metadata));
            }
            let owner = s.owner.ok_or(reference(symbol, ReferenceError::Owner))?;
            if !c.store.scopes.contains(owner_scope) {
                return Err(PickleError::InvalidScope { scope: owner_scope });
            }
            let scope = c.store.scopes.get(owner_scope);
            if scope.owner != Some(owner) {
                return Err(reference(symbol, ReferenceError::Owner));
            }
            if scope.lookup_all(&s.name) != [symbol] {
                return Err(reference(symbol, ReferenceError::Ambiguous));
            }
            c.class_scope(symbol)?;
            let package = c.package_name(owner)?;
            let name = c.utf8(c.text(s.name.text())?)?;
            c.external.insert(symbol, (package, name));
            Ok(())
        })
    }

    /// Registers a checked class from the same source batch for signature
    /// projection. The caller supplies the complete producer batch's class
    /// identities; this method additionally verifies canonical source
    /// provenance, same-package ownership, declaration scope, and ClassInfo.
    pub(crate) fn register_planned_source_class(
        &mut self,
        symbol: SymbolId,
        source_tree: dotty_core::TreeId<dotty_core::Untyped>,
        planned_classes: &[SymbolId],
    ) -> Result<(), PickleError> {
        self.run(|c| {
            c.charge(1)?;
            if !planned_classes.contains(&symbol)
                || c.declarations.contains_key(&symbol)
                || c.source_index.symbol_at(c.source, source_tree) != Some(symbol)
                || c.source_index.definition_of(symbol)
                    != Some(SourceDefinition::Canonical {
                        source: c.source,
                        tree: source_tree,
                    })
            {
                return Err(reference(symbol, ReferenceError::NotRegistered));
            }
            c.namespace(symbol, true)?;
            let sibling = c.symbol(symbol)?;
            if sibling.kind != SymbolKind::Class
                || sibling.origin != SymbolOrigin::Source(c.source)
                || sibling.visibility != Visibility::Public
                || !sibling.flags.is_empty()
                || !sibling.annotations.is_empty()
            {
                return Err(reference(symbol, ReferenceError::Metadata));
            }
            let owner = sibling
                .owner
                .ok_or(reference(symbol, ReferenceError::Owner))?;
            let sibling_name = sibling.name;
            let current_class = c
                .declarations
                .keys()
                .copied()
                .find(|&candidate| {
                    c.store.symbols.contains(candidate)
                        && c.store.symbols.get(candidate).kind == SymbolKind::Class
                })
                .ok_or(reference(symbol, ReferenceError::NotRegistered))?;
            if c.symbol(current_class)?.owner != Some(owner) {
                return Err(reference(symbol, ReferenceError::Owner));
            }
            let class_type = c.complete(symbol)?;
            let Type::ClassInfo(info) = c.ty(class_type)? else {
                return Err(PickleError::MalformedType {
                    ty: class_type,
                    role: crate::TypeRole::ClassInfo,
                });
            };
            if info.class != symbol
                || info.prefix != c.definitions.no_prefix
                || info.parents.as_slice() != [c.definitions.object_type]
                || info.self_type.is_some()
                || c.source_index.scope_of(symbol) != Some(info.declarations)
                || c.class_scope(symbol)? != info.declarations
                || c.store
                    .scopes
                    .get(info.declarations)
                    .entered_symbols()
                    .any(|member| {
                        c.store.symbols.contains(member)
                            && c.store.symbols.get(member).kind == SymbolKind::TypeParameter
                    })
            {
                return Err(PickleError::MalformedType {
                    ty: class_type,
                    role: crate::TypeRole::ClassInfo,
                });
            }
            let package = c.package_name(owner)?;
            let name = c.utf8(c.text(sibling_name.text())?)?;
            c.external.insert(symbol, (package, name));
            c.same_source_external.insert(symbol);
            Ok(())
        })
    }
    fn builtin_ref(&mut self, builtin: Builtin) -> Result<AstLabel, PickleError> {
        let symbol = self.builtin(builtin)?;
        let package = if builtin == Builtin::Object {
            let java = self.utf8("java")?;
            let lang = self.utf8("lang")?;
            self.intern(NameKey::Qualified(java, lang))?
        } else {
            self.utf8("scala")?
        };
        let name = self.utf8(self.text(self.symbol(symbol)?.name.text())?)?;
        let child = self.leaf(TERMREFPKG_TAG, TermValue::NameRef(package))?;
        self.wire
            .push(AstNode::NatAst {
                tag: TYPEREF_TAG,
                value: AstOperand::Nat(name),
                child,
            })
            .map_err(construction)
    }
    fn external_inner(&mut self, symbol: SymbolId) -> Result<AstLabel, PickleError> {
        for builtin in [
            Builtin::Int,
            Builtin::Boolean,
            Builtin::Unit,
            Builtin::Object,
        ] {
            if self.builtin(builtin)? == symbol {
                return self.builtin_ref(builtin);
            }
        }
        self.symbol(symbol)?;
        let &(package, name) = self
            .external
            .get(&symbol)
            .ok_or(reference(symbol, ReferenceError::NotRegistered))?;
        let child = self.leaf(TERMREFPKG_TAG, TermValue::NameRef(package))?;
        self.wire
            .push(AstNode::NatAst {
                tag: TYPEREF_TAG,
                value: AstOperand::Nat(name),
                child,
            })
            .map_err(construction)
    }
    /// References a registered dependency or canonical Int/Object builtin.
    /// Unit is available only through the explicit constructor-return API.
    pub fn external_reference(&mut self, symbol: SymbolId) -> Result<AstLabel, PickleError> {
        self.run(|c| {
            if c.builtin(Builtin::Unit)? == symbol {
                return Err(reference(symbol, ReferenceError::Kind));
            }
            c.external_inner(symbol)
        })
    }
    /// Lowers a semantic type, memoized by exact TypeId, never by spelling.
    pub fn lower_type(&mut self, ty: TypeId) -> Result<AstLabel, PickleError> {
        self.run(|c| c.lower(ty, 0))
    }
    fn lower(&mut self, ty: TypeId, depth: usize) -> Result<AstLabel, PickleError> {
        self.charge(1)?;
        if depth > self.limits.max_type_depth.min(256) {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::TypeDepth,
                limit: self.limits.max_type_depth.min(256),
            });
        }
        if self.active.contains(&ty) {
            return Err(PickleError::CyclicType { ty });
        }
        if let Some(&target) = self.types.get(&ty) {
            return self
                .wire
                .push(AstNode::Reference {
                    tag: SHAREDTYPE_TAG,
                    target,
                })
                .map_err(construction);
        }
        let value = self.ty(ty)?;
        self.active.insert(ty);
        let label = match value {
            Type::TypeRef {
                prefix,
                target: TypeRefTarget::Symbol(symbol),
            }
            | Type::TermRef {
                prefix,
                target: TermRefTarget::Symbol(symbol),
            } => {
                let is_type = matches!(value, Type::TypeRef { .. });
                let s = self.symbol(*symbol)?;
                self.ty(*prefix)?;
                if s.kind == SymbolKind::Package {
                    if *prefix != self.definitions.no_prefix {
                        return Err(reference(*symbol, ReferenceError::Owner));
                    }
                    self.package_inner(*symbol, is_type)?
                } else {
                    self.namespace(*symbol, is_type)?;
                    if *symbol == self.builtin(Builtin::Unit)? {
                        return Err(PickleError::UnsupportedTypeReference { ty });
                    }
                    if self.declarations.contains_key(symbol) {
                        self.local_inner(
                            *symbol,
                            (*prefix != self.definitions.no_prefix).then_some(*prefix),
                            depth,
                        )?
                    } else if *symbol == self.builtin(Builtin::Int)?
                        || *symbol == self.builtin(Builtin::Boolean)?
                        || *symbol == self.builtin(Builtin::Object)?
                    {
                        if *prefix != self.definitions.no_prefix {
                            return Err(reference(*symbol, ReferenceError::Owner));
                        }
                        self.external_inner(*symbol)?
                    } else {
                        self.check_prefix(*symbol, *prefix)?;
                        if self.same_source_external.contains(symbol) {
                            self.external_inner(*symbol)?
                        } else {
                            let &(_, name) = self
                                .external
                                .get(symbol)
                                .ok_or(reference(*symbol, ReferenceError::NotRegistered))?;
                            let child = self.lower(*prefix, depth + 1)?;
                            self.wire
                                .push(AstNode::NatAst {
                                    tag: TYPEREF_TAG,
                                    value: AstOperand::Nat(name),
                                    child,
                                })
                                .map_err(construction)?
                        }
                    }
                }
            }
            Type::ThisType { class } => match self.symbol(*class)?.kind {
                SymbolKind::Package => self.package_inner(*class, false)?,
                SymbolKind::Class => {
                    self.namespace(*class, true)?;
                    let child = self.local_inner(*class, None, depth)?;
                    self.wire
                        .push(AstNode::Ast {
                            tag: THIS_TAG,
                            child,
                        })
                        .map_err(construction)?
                }
                _ => return Err(reference(*class, ReferenceError::Kind)),
            },
            Type::Method(method) => {
                if self.active.contains(&method.result) {
                    return Err(PickleError::CyclicType { ty: method.result });
                }
                if method.kind != MethodKind::Plain
                    || matches!(self.ty(method.result)?, Type::Method(_))
                {
                    return Err(PickleError::UnsupportedTypeReference { ty });
                }
                self.charge(method.params.len())?;
                let result = self.lower(method.result, depth + 1)?;
                let mut fields = vec![AstField::Tree(result)];
                let mut seen = HashSet::new();
                for parameter in &method.params {
                    if self.active.contains(&parameter.ty) {
                        return Err(PickleError::CyclicType { ty: parameter.ty });
                    }
                    if parameter.erased
                        || parameter.varargs
                        || !seen.insert(parameter.name)
                        || matches!(self.ty(parameter.ty)?, Type::Method(_))
                    {
                        return Err(PickleError::UnsupportedTypeReference { ty });
                    }
                    let label = self.lower(parameter.ty, depth + 1)?;
                    let name = self.utf8(self.text(parameter.name.as_name().text())?)?;
                    fields.extend([AstField::Tree(label), AstField::Nat(name)]);
                }
                self.wire
                    .push(AstNode::Length {
                        tag: METHODTYPE_TAG,
                        fields,
                    })
                    .map_err(construction)?
            }
            _ => return Err(PickleError::UnsupportedTypeReference { ty }),
        };
        self.active.remove(&ty);
        self.types.insert(ty, label);
        Ok(label)
    }
    /// An explicit Int result syntax wrapper, distinct from its semantic type.
    pub fn int_type_syntax(&mut self) -> Result<AstLabel, PickleError> {
        self.run(|c| {
            let symbol = c.builtin(Builtin::Int)?;
            let name = c.utf8(c.text(c.symbol(symbol)?.name.text())?)?;
            let child = c.lower(c.definitions.int, 0)?;
            c.wire
                .push(AstNode::NatAst {
                    tag: IDENTTPT_TAG,
                    value: AstOperand::Nat(name),
                    child,
                })
                .map_err(construction)
        })
    }
    /// An explicit Boolean type syntax wrapper, distinct from its semantic type.
    pub fn boolean_type_syntax(&mut self) -> Result<AstLabel, PickleError> {
        self.run(|c| {
            let symbol = c.builtin(Builtin::Boolean)?;
            let name = c.utf8(c.text(c.symbol(symbol)?.name.text())?)?;
            let child = c.lower(c.definitions.boolean, 0)?;
            c.wire
                .push(AstNode::NatAst {
                    tag: IDENTTPT_TAG,
                    value: AstOperand::Nat(name),
                    child,
                })
                .map_err(construction)
        })
    }
    /// Projects the explicit source syntax wrapper for a supported primitive.
    pub fn primitive_type_syntax(&mut self, ty: TypeId) -> Result<AstLabel, PickleError> {
        if ty == self.definitions.int {
            self.int_type_syntax()
        } else if ty == self.definitions.boolean {
            self.boolean_type_syntax()
        } else if ty == self.definitions.unit {
            self.constructor_return_syntax()
        } else {
            Err(PickleError::UnsupportedTypeReference { ty })
        }
    }
    /// The oracle's wire Unit return type for a default constructor. It never
    /// replaces the constructor's canonical class-instance semantic signature.
    pub fn constructor_return_type(&mut self) -> Result<AstLabel, PickleError> {
        self.run(|c| c.builtin_ref(Builtin::Unit))
    }
    /// The corresponding explicit Unit result syntax wrapper.
    pub fn constructor_return_syntax(&mut self) -> Result<AstLabel, PickleError> {
        self.run(|c| {
            let symbol = c.builtin(Builtin::Unit)?;
            let name = c.utf8(c.text(c.symbol(symbol)?.name.text())?)?;
            let child = c.builtin_ref(Builtin::Unit)?;
            c.wire
                .push(AstNode::NatAst {
                    tag: IDENTTPT_TAG,
                    value: AstOperand::Nat(name),
                    child,
                })
                .map_err(construction)
        })
    }
    /// The frozen oracle's zero-parameter Object constructor SIGNED name.
    /// This names a dependency operation; it does not manufacture a constructor
    /// SymbolId for the ownerless bootstrapped Object class. Selection/tree
    /// construction is left to the next increment.
    pub fn object_constructor_name(&mut self) -> Result<NameRef, PickleError> {
        self.run(|c| {
            c.builtin(Builtin::Object)?;
            let original = c.utf8("<init>")?;
            let java = c.utf8("java")?;
            let lang = c.utf8("lang")?;
            let package = c.intern(NameKey::Qualified(java, lang))?;
            let object = c.utf8("Object")?;
            let result = c.intern(NameKey::Qualified(package, object))?;
            c.intern(NameKey::SignedEmpty(original, result))
        })
    }
    /// Resolves addresses and returns only owned names/type fragments. The next
    /// increment owns definition grammar, sections and full-file validation.
    pub fn finish(&self, roots: &[AstLabel]) -> Result<ProjectedFragment, PickleError> {
        if self.failed {
            return Err(PickleError::ProjectionAborted);
        }
        let names = self.names.clone().finish().map_err(encoding)?;
        let ast = self.wire.finish(roots).map_err(construction)?;
        Ok(ProjectedFragment { names, ast })
    }
}
