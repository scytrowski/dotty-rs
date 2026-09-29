//! Structural class/trait ↔ companion-object linking during symbol entry.

use dotty_core::names::{Name, Namespace};
use dotty_core::symbols::SymbolKind;

use crate::error::UnpickleError;
use crate::unpickler::TastyUnpickler;

impl TastyUnpickler<'_, '_, '_> {
    /// Publishes links only after every identity in the unit has been entered.
    /// The owner scope and exact interned name are the identity boundary; the
    /// module class is deliberately not a candidate endpoint.
    pub(crate) fn link_companions(&mut self) -> Result<(), UnpickleError> {
        let mut pairs = Vec::new();
        for symbol in self.index.entered_symbols() {
            let candidate = self.store.symbols.get(symbol).clone();
            let Some(owner) = candidate.owner else {
                continue;
            };
            let counterpart_is_object = match candidate.kind {
                SymbolKind::Class | SymbolKind::Trait => true,
                SymbolKind::Object => false,
                _ => continue,
            };
            let scope = self
                .index
                .scope_of(owner)
                .or_else(|| self.packages.scope_of(owner))
                .or_else(|| self.shared_scopes.get(&owner).copied());
            let Some(scope) = scope else {
                continue;
            };
            let namespace = if counterpart_is_object {
                Namespace::Term
            } else {
                Namespace::Type
            };
            let name = Name::new(candidate.name.text(), namespace);
            let candidates: Vec<_> = self
                .store
                .scopes
                .get(scope)
                .lookup_all(&name)
                .iter()
                .copied()
                .filter(|other| {
                    let kind = self.store.symbols.get(*other).kind;
                    if counterpart_is_object {
                        kind == SymbolKind::Object
                    } else {
                        matches!(kind, SymbolKind::Class | SymbolKind::Trait)
                    }
                })
                .collect();
            let [other] = candidates.as_slice() else {
                continue;
            };
            let (class, object) = if candidate.kind == SymbolKind::Object {
                (*other, symbol)
            } else {
                (symbol, *other)
            };

            // Ensure the reverse lookup is unique too. This matters when an
            // invalid scope contains same-name declarations in both spaces.
            let class_symbol = self.store.symbols.get(class);
            let reverse_name = Name::new(class_symbol.name.text(), Namespace::Term);
            let reverse: Vec<_> = self
                .store
                .scopes
                .get(scope)
                .lookup_all(&reverse_name)
                .iter()
                .copied()
                .filter(|other| self.store.symbols.get(*other).kind == SymbolKind::Object)
                .collect();
            if reverse.as_slice() != [object] {
                continue;
            }
            if !pairs.contains(&(class, object)) {
                pairs.push((class, object));
            }
        }

        // Validate every endpoint before changing any pre-existing symbol.
        // Thus a conflict cannot publish a partial set of links.
        for &(class, object) in &pairs {
            for (symbol, expected) in [(class, object), (object, class)] {
                if let Some(existing) = self.store.symbols.get(symbol).links.companion
                    && existing != expected
                {
                    return Err(UnpickleError::ConflictingCompanion {
                        symbol,
                        existing,
                        candidate: expected,
                    });
                }
            }
        }
        for (class, object) in pairs {
            self.store.symbols.get_mut(class).links.companion = Some(object);
            self.store.symbols.get_mut(object).links.companion = Some(class);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::Definitions;
    use dotty_core::names::Name;
    use dotty_core::store::SemanticStore;
    use dotty_core::symbols::SymbolKind;
    use dotty_tasty::tasty::TastyFile;

    const BOTH: &[u8] = include_bytes!("../tests/fixtures/semantic/Both.tasty");

    #[test]
    fn rediscovering_an_exact_pair_is_idempotent() {
        let file = TastyFile::parse_scala_3_9(BOTH).unwrap();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
        unpickler.enter_symbols().unwrap();

        let class = *unpickler
            .store
            .scopes
            .get(
                unpickler
                    .index()
                    .scope_of(unpickler.index().symbol_at(0).unwrap())
                    .unwrap(),
            )
            .lookup_all(&Name::new(
                unpickler.store.names.intern("Both"),
                Namespace::Type,
            ))
            .first()
            .unwrap();
        let object = *unpickler
            .store
            .scopes
            .get(
                unpickler
                    .index()
                    .scope_of(unpickler.index().symbol_at(0).unwrap())
                    .unwrap(),
            )
            .lookup_all(&Name::new(
                unpickler.store.names.intern("Both"),
                Namespace::Term,
            ))
            .first()
            .unwrap();

        unpickler.link_companions().unwrap();
        assert_eq!(
            unpickler.store.symbols.get(class).links.companion,
            Some(object)
        );
        assert_eq!(
            unpickler.store.symbols.get(object).links.companion,
            Some(class)
        );
    }

    #[test]
    fn a_conflicting_existing_link_fails_before_mutating_its_other_endpoint() {
        let file = TastyFile::parse_scala_3_9(BOTH).unwrap();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
        unpickler.enter_symbols().unwrap();

        let package = unpickler.index().symbol_at(0).unwrap();
        let scope = unpickler.index().scope_of(package).unwrap();
        let class = *unpickler
            .store
            .scopes
            .get(scope)
            .lookup_all(&Name::new(
                unpickler.store.names.intern("Both"),
                Namespace::Type,
            ))
            .iter()
            .find(|symbol| unpickler.store.symbols.get(**symbol).kind == SymbolKind::Class)
            .unwrap();
        let object = *unpickler
            .store
            .scopes
            .get(scope)
            .lookup_all(&Name::new(
                unpickler.store.names.intern("Both"),
                Namespace::Term,
            ))
            .first()
            .unwrap();
        let module_class = *unpickler
            .store
            .scopes
            .get(scope)
            .lookup_all(&Name::new(
                unpickler.store.names.intern("Both$"),
                Namespace::Type,
            ))
            .first()
            .unwrap();
        unpickler.store.symbols.get_mut(class).links.companion = Some(module_class);
        unpickler.store.symbols.get_mut(object).links.companion = None;

        assert_eq!(
            unpickler.link_companions(),
            Err(UnpickleError::ConflictingCompanion {
                symbol: class,
                existing: module_class,
                candidate: object,
            })
        );
        assert_eq!(
            unpickler.store.symbols.get(class).links.companion,
            Some(module_class)
        );
        assert_eq!(unpickler.store.symbols.get(object).links.companion, None);
    }
}
