//! Symbol lookup across the named classes in a source batch.

use std::{collections::HashMap, fmt};

use crate::core::{SemanticStore, SourceId, SourceSpan, SymbolId, SymbolKind, SymbolOrigin};
use crate::source_declarations::{ClassMember, SourceClass};

/// The declaring source, enclosing class and kind of an indexed symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceDeclaration {
    /// Identity of the original source file in this session.
    pub source: SourceId,
    /// A class declaration refers to itself here.
    pub class: SymbolId,
    /// Classifies the authenticated constructor-argument shape.
    pub kind: SymbolKind,
}

/// Owned declaration details for reporting a signature dependency after analysis.
/// Symbol and owner IDs refer to the original session, and the range refers to
/// the original source revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureDeclaration {
    /// Requested declaration, including when it is outside the source index.
    pub symbol: SymbolId,
    /// Enclosing class, file and kind if the symbol was indexed.
    pub declaration: Option<SourceDeclaration>,
    /// Captured spelling of an indexed declaration, when available.
    pub name: Option<Box<str>>,
    /// Declaration range if its position belongs to the indexed source.
    pub location: Option<SourceSpan>,
}

/// A named class view contains an inconsistent or repeated symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceDeclarationIndexError {
    /// The symbol has no supported canonical source declaration.
    InvalidDeclaration(SymbolId),
    /// The symbol already has an entry in this source declaration index.
    DuplicateDeclaration(SymbolId),
}

impl fmt::Display for SourceDeclarationIndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDeclaration(id) => write!(f, "invalid source declaration {id:?}"),
            Self::DuplicateDeclaration(id) => write!(f, "duplicate source declaration {id:?}"),
        }
    }
}

impl std::error::Error for SourceDeclarationIndexError {}

/// Read-only lookup by symbol for a caller-selected batch of ordinary source classes.
#[derive(Debug)]
pub struct SourceDeclarationIndex {
    declarations: HashMap<SymbolId, SourceDeclaration>,
}

impl SourceDeclarationIndex {
    /// Indexes classes returned by [`crate::source_declarations::source_classes`],
    /// their primary constructors, class-owned constructor fields and body members.
    /// Pass views and a store from the same naming session. The index owns its entries
    /// and can be queried while the typer mutates the store.
    pub fn from_classes<'a>(
        store: &SemanticStore,
        classes: impl IntoIterator<Item = &'a SourceClass>,
    ) -> Result<Self, SourceDeclarationIndexError> {
        use SourceDeclarationIndexError::{DuplicateDeclaration, InvalidDeclaration};

        let mut declarations = HashMap::new();
        for class in classes {
            let id = class.symbol;
            if !store.symbols.contains(id) {
                return Err(InvalidDeclaration(id));
            }
            let class_symbol = store.symbols.get(id);
            let SymbolOrigin::Source(source) = class_symbol.origin else {
                return Err(InvalidDeclaration(id));
            };
            let members = class.members.iter().map(|member| match member {
                ClassMember::Field(d) => (d.symbol, SymbolKind::Field),
                ClassMember::Method(d) => (d.symbol, SymbolKind::Method),
            });
            let fields = class
                .constructor_fields
                .iter()
                .map(|&field| (field, SymbolKind::Field));
            for (symbol, kind) in [
                (id, SymbolKind::Class),
                (class.constructor, SymbolKind::Constructor),
            ]
            .into_iter()
            .chain(fields)
            .chain(members)
            {
                if !store.symbols.contains(symbol) {
                    return Err(InvalidDeclaration(symbol));
                }
                let entry = store.symbols.get(symbol);
                if entry.kind != kind
                    || entry.origin != class_symbol.origin
                    || (kind != SymbolKind::Class && entry.owner != Some(id))
                {
                    return Err(InvalidDeclaration(symbol));
                }
                let declaration = SourceDeclaration {
                    source,
                    class: id,
                    kind,
                };
                if declarations.insert(symbol, declaration).is_some() {
                    return Err(DuplicateDeclaration(symbol));
                }
            }
        }
        Ok(Self { declarations })
    }

    /// Returns the declaration for a same-session symbol, or `None` outside the batch.
    pub fn get(&self, symbol: SymbolId) -> Option<SourceDeclaration> {
        self.declarations.get(&symbol).copied()
    }

    /// Captures the spelling and position of an indexed signature participant
    /// using its original semantic store. An unindexed symbol retains just
    /// its ID. A missing or foreign position leaves the declaration unlocated.
    pub fn signature_declaration(
        &self,
        symbol: SymbolId,
        store: &SemanticStore,
    ) -> SignatureDeclaration {
        let declaration = self.get(symbol);
        let entry = declaration.and_then(|d| {
            if !store.symbols.contains(symbol) {
                return None;
            }
            let entry = store.symbols.get(symbol);
            (entry.origin == SymbolOrigin::Source(d.source)).then_some((entry, d.source))
        });
        SignatureDeclaration {
            symbol,
            declaration,
            name: entry
                .and_then(|(s, _)| store.names.try_resolve(s.name.text()))
                .map(Into::into),
            location: entry.and_then(|(s, source)| s.position.filter(|p| p.source() == source)),
        }
    }

    /// Returns an indexed field or method eligible for individual signature completion.
    pub fn member(&self, symbol: SymbolId) -> Option<SourceDeclaration> {
        self.get(symbol)
            .filter(|d| matches!(d.kind, SymbolKind::Method | SymbolKind::Field))
    }
}
