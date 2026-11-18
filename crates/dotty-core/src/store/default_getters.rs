//! Canonical method, parameter and getter identities for already identified defaults.

use std::collections::HashMap;
use std::fmt;

use crate::{SemanticStore, SymbolId, SymbolKind};

/// A producer attempted to publish inconsistent default argument metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultGetterError {
    /// A symbol is outside the store, has the wrong kind, or belongs to a different owner.
    InvalidEndpoints,
    /// A method position, parameter or getter already participates in a different relation.
    ConflictingRelation,
}

impl fmt::Display for DefaultGetterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl std::error::Error for DefaultGetterError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Relation {
    method: SymbolId,
    ordinal: usize,
    parameter: SymbolId,
    getter: SymbolId,
}

#[derive(Default)]
pub(super) struct DefaultGetters {
    records: Vec<Relation>,
    positions: HashMap<(SymbolId, usize), usize>,
    endpoints: HashMap<SymbolId, usize>,
}

impl fmt::Debug for DefaultGetters {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.records.fmt(f)
    }
}

impl DefaultGetters {
    pub(super) fn len(&self) -> usize {
        self.records.len()
    }

    pub(super) fn truncate(&mut self, mark: usize) {
        for entry in self.records.drain(mark.min(self.records.len())..) {
            self.positions.remove(&(entry.method, entry.ordinal));
            self.endpoints.remove(&entry.parameter);
            self.endpoints.remove(&entry.getter);
        }
    }
}

impl SemanticStore {
    /// The verified getter for a zero-based term parameter position, flattened
    /// across all of the method's clauses. Type parameters consume no position.
    pub fn default_getter(&self, method: SymbolId, ordinal: usize) -> Option<SymbolId> {
        let index = *self.default_getters.positions.get(&(method, ordinal))?;
        Some(self.default_getters.records[index].getter)
    }

    /// The method and flattened term-parameter position whose default a getter implements.
    pub fn default_getter_origin(&self, getter: SymbolId) -> Option<(SymbolId, usize)> {
        let index = *self.default_getters.endpoints.get(&getter)?;
        let relation = self.default_getters.records[index];
        (relation.getter == getter).then_some((relation.method, relation.ordinal))
    }

    /// Records the relation proven by the caller's source declaration or binary
    /// metadata. The parameter must belong to the method; the getter and method
    /// must be distinct methods of the same owner. Repeating a relation is harmless.
    pub fn record_default_getter(
        &mut self,
        method: SymbolId,
        ordinal: usize,
        parameter: SymbolId,
        getter: SymbolId,
    ) -> Result<(), DefaultGetterError> {
        if ![method, parameter, getter]
            .iter()
            .all(|&id| self.symbols.contains(id))
        {
            return Err(DefaultGetterError::InvalidEndpoints);
        }
        let declaration = self.symbols.get(method);
        let value = self.symbols.get(parameter);
        let accessor = self.symbols.get(getter);
        if method == getter
            || declaration.kind != SymbolKind::Method
            || accessor.kind != SymbolKind::Method
            || value.kind != SymbolKind::Parameter
            || value.owner != Some(method)
            || !declaration
                .owner
                .is_some_and(|owner| self.symbols.contains(owner))
            || declaration.owner != accessor.owner
        {
            return Err(DefaultGetterError::InvalidEndpoints);
        }
        let relation = Relation {
            method,
            ordinal,
            parameter,
            getter,
        };
        let defaults = &mut self.default_getters;
        if let Some(&index) = defaults.positions.get(&(method, ordinal)) {
            return if defaults.records[index] == relation {
                Ok(())
            } else {
                Err(DefaultGetterError::ConflictingRelation)
            };
        }
        if defaults.endpoints.contains_key(&parameter) || defaults.endpoints.contains_key(&getter) {
            return Err(DefaultGetterError::ConflictingRelation);
        }
        let index = defaults.records.len();
        defaults.positions.insert((method, ordinal), index);
        defaults.endpoints.insert(parameter, index);
        defaults.endpoints.insert(getter, index);
        defaults.records.push(relation);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Name, Namespace, Symbol, SymbolFlags, SymbolInfo, SymbolLinks, SymbolOrigin, Visibility,
    };

    fn symbol(store: &mut SemanticStore, kind: SymbolKind, owner: Option<SymbolId>) -> SymbolId {
        let name = Name::new(store.names.intern("same"), Namespace::Term);
        store.symbols.alloc(Symbol {
            kind,
            owner,
            name,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: vec![],
            position: None,
            links: SymbolLinks::default(),
        })
    }

    #[test]
    fn default_getter_uses_endpoint_identity_and_rolls_back_relations_among_existing_symbols() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store, SymbolKind::Class, None);
        let method = symbol(&mut store, SymbolKind::Method, Some(owner));
        let overload = symbol(&mut store, SymbolKind::Method, Some(owner));
        let first = symbol(&mut store, SymbolKind::Parameter, Some(method));
        let second = symbol(&mut store, SymbolKind::Parameter, Some(method));
        let other_parameter = symbol(&mut store, SymbolKind::Parameter, Some(overload));
        let getter = symbol(&mut store, SymbolKind::Method, Some(owner));
        let other_getter = symbol(&mut store, SymbolKind::Method, Some(owner));
        let foreign_getter = symbol(&mut store, SymbolKind::Method, Some(overload));

        assert_eq!(
            store.record_default_getter(method, 0, first, getter),
            Ok(())
        );
        let checkpoint = store.checkpoint();
        assert_eq!(
            store.record_default_getter(method, 0, first, getter),
            Ok(())
        );
        assert_eq!(store.checkpoint(), checkpoint);
        for (ordinal, param, endpoint, error) in [
            (1, second, getter, DefaultGetterError::ConflictingRelation),
            (
                1,
                first,
                other_getter,
                DefaultGetterError::ConflictingRelation,
            ),
            (
                0,
                second,
                other_getter,
                DefaultGetterError::ConflictingRelation,
            ),
            (
                1,
                other_parameter,
                other_getter,
                DefaultGetterError::InvalidEndpoints,
            ),
            (
                1,
                second,
                foreign_getter,
                DefaultGetterError::InvalidEndpoints,
            ),
            (1, second, method, DefaultGetterError::InvalidEndpoints),
            (1, second, first, DefaultGetterError::InvalidEndpoints),
            (
                1,
                second,
                SymbolId::new(u32::MAX),
                DefaultGetterError::InvalidEndpoints,
            ),
        ] {
            assert_eq!(
                store.record_default_getter(method, ordinal, param, endpoint),
                Err(error)
            );
            assert_eq!(store.checkpoint(), checkpoint);
        }
        assert_eq!(
            store.record_default_getter(method, 1, second, other_getter),
            Ok(())
        );
        assert_eq!(store.default_getter_origin(other_getter), Some((method, 1)));
        assert_eq!(store.default_getter_origin(second), None);
        store.rollback_to(checkpoint);
        assert_eq!(store.default_getter(method, 0), Some(getter));
        assert_eq!(store.default_getter(method, 1), None);
        assert_eq!(store.default_getter_origin(other_getter), None);
        assert_eq!(
            store.record_default_getter(overload, 0, other_parameter, other_getter),
            Ok(())
        );
        assert_eq!(store.default_getter(overload, 0), Some(other_getter));
    }
}
