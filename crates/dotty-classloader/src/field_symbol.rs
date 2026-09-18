use crate::annotation::SemanticAnnotation;
use dotty_classfile::access_flags::FieldAccessFlags;
use dotty_classfile::descriptor::FieldType;
use dotty_classfile::signature::FieldSignature;
use dotty_core::SymbolId;

/// A loaded class's field: its name, access flags, declared type, and
/// optional generic signature (JVMS §4.5).
///
/// `field_type` is exactly what `dotty-classfile` already parses from the
/// field's descriptor (JVMS §4.3.2) — an `Object`/`Array` entry's class
/// name stays an internal-form string, not a resolved reference. The
/// resolved semantic type is *not* duplicated here: it is a real
/// `dotty-core` `Symbol` (`SymbolKind::Field`) entered into the owning
/// class's `Type::ClassInfo::declarations` scope — `symbol` names that
/// same `Symbol` directly, so a caller that already has a `FieldSymbol`
/// does not have to re-look it up by `name` through the scope to get it.
/// This type otherwise stays purely JVM-facing sidecar metadata — see
/// `docs/classloader.md`'s JVM metadata sidecar section.
///
/// `signature` is `None` unless the field carries a `Signature` attribute
/// (JVMS §4.7.9.1) — the common case for a non-generic field type. It is
/// the raw parsed grammar tree from `dotty-classfile`, verbatim: no
/// resolution of the class names it mentions, no semantic type model
/// built from it.
#[derive(Debug, Clone)]
pub struct FieldSymbol {
    name: String,
    symbol: SymbolId,
    flags: FieldAccessFlags,
    field_type: FieldType,
    signature: Option<FieldSignature>,
    annotations: Vec<SemanticAnnotation>,
}

impl FieldSymbol {
    pub fn new(
        name: String,
        symbol: SymbolId,
        flags: FieldAccessFlags,
        field_type: FieldType,
        signature: Option<FieldSignature>,
        annotations: Vec<SemanticAnnotation>,
    ) -> Self {
        Self {
            name,
            symbol,
            flags,
            field_type,
            signature,
            annotations,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The real `dotty-core` `Symbol` (`SymbolKind::Field`) this sidecar
    /// record describes — see this type's own doc comment.
    pub fn symbol(&self) -> SymbolId {
        self.symbol
    }

    pub fn flags(&self) -> FieldAccessFlags {
        self.flags
    }

    pub fn field_type(&self) -> &FieldType {
        &self.field_type
    }

    pub fn signature(&self) -> Option<&FieldSignature> {
        self.signature.as_ref()
    }

    pub fn annotations(&self) -> &[SemanticAnnotation] {
        &self.annotations
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::SemanticStore;

    /// A `SymbolId` real enough to stand in for a field/method's backing
    /// `Symbol` in a test — `FieldSymbol`/`MethodSymbol` are pure sidecar
    /// data with no `SemanticStore` of their own to allocate one from.
    fn some_symbol_id(store: &mut SemanticStore) -> SymbolId {
        use dotty_core::{
            Name, Namespace, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks,
            SymbolOrigin, Visibility,
        };

        let text = store.names.intern("field");
        store.symbols.alloc(Symbol {
            name: Name::new(text, Namespace::Term),
            owner: None,
            kind: SymbolKind::Field,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        })
    }

    #[test]
    fn exposes_name_flags_and_field_type() {
        let mut store = SemanticStore::new();
        let symbol = FieldSymbol::new(
            "ANSWER".to_owned(),
            some_symbol_id(&mut store),
            FieldAccessFlags(0x0019),
            FieldType::Int,
            None,
            Vec::new(),
        );

        assert_eq!(symbol.name(), "ANSWER");
        assert_eq!(symbol.flags(), FieldAccessFlags(0x0019));
        assert_eq!(symbol.field_type(), &FieldType::Int);
        assert_eq!(symbol.signature(), None);
        assert!(symbol.annotations().is_empty());
    }

    #[test]
    fn exposes_the_real_symbol_it_describes() {
        let mut store = SemanticStore::new();
        let symbol_id = some_symbol_id(&mut store);
        let symbol = FieldSymbol::new(
            "ANSWER".to_owned(),
            symbol_id,
            FieldAccessFlags(0x0019),
            FieldType::Int,
            None,
            Vec::new(),
        );

        assert_eq!(symbol.symbol(), symbol_id);
    }

    #[test]
    fn exposes_a_generic_signature_when_present() {
        use dotty_classfile::signature::ReferenceTypeSignature;

        let mut store = SemanticStore::new();
        let signature = FieldSignature(ReferenceTypeSignature::TypeVariable("T".to_owned()));
        let symbol = FieldSymbol::new(
            "items".to_owned(),
            some_symbol_id(&mut store),
            FieldAccessFlags(0x0001),
            FieldType::Object("java/util/List".to_owned()),
            Some(signature.clone()),
            Vec::new(),
        );

        assert_eq!(symbol.signature(), Some(&signature));
    }
}
