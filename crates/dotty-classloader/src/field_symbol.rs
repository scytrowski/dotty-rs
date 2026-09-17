use crate::semantic_type::SemanticFieldType;
use dotty_classfile::access_flags::FieldAccessFlags;
use dotty_classfile::descriptor::FieldType;
use dotty_classfile::signature::FieldSignature;

/// A loaded class's field: its name, access flags, declared type,
/// optional generic signature, and semantic (class-reference-resolved)
/// type (JVMS §4.5).
///
/// `field_type` is exactly what `dotty-classfile` already parses from the
/// field's descriptor (JVMS §4.3.2) — an `Object`/`Array` entry's class
/// name stays an internal-form string, not a resolved [`crate::ClassRef`].
/// `semantic_type` is the resolved counterpart, built by
/// `ClassLoader` (`docs/classloader.md` §9, Milestone 6); kept alongside
/// `field_type` rather than replacing it, the same "attach without
/// replacing" approach Milestone 5 used for `signature`.
///
/// `signature` is `None` unless the field carries a `Signature` attribute
/// (JVMS §4.7.9.1) — the common case for a non-generic field type. It is
/// the raw parsed grammar tree from `dotty-classfile`, verbatim: no
/// resolution of the class names it mentions, no semantic type model
/// built from it.
#[derive(Debug, Clone)]
pub struct FieldSymbol {
    name: String,
    flags: FieldAccessFlags,
    field_type: FieldType,
    signature: Option<FieldSignature>,
    semantic_type: SemanticFieldType,
}

impl FieldSymbol {
    pub fn new(
        name: String,
        flags: FieldAccessFlags,
        field_type: FieldType,
        signature: Option<FieldSignature>,
        semantic_type: SemanticFieldType,
    ) -> Self {
        Self {
            name,
            flags,
            field_type,
            signature,
            semantic_type,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
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

    pub fn semantic_type(&self) -> &SemanticFieldType {
        &self.semantic_type
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_name_flags_and_field_type() {
        let symbol = FieldSymbol::new(
            "ANSWER".to_owned(),
            FieldAccessFlags(0x0019),
            FieldType::Int,
            None,
            SemanticFieldType::Int,
        );

        assert_eq!(symbol.name(), "ANSWER");
        assert_eq!(symbol.flags(), FieldAccessFlags(0x0019));
        assert_eq!(symbol.field_type(), &FieldType::Int);
        assert_eq!(symbol.signature(), None);
        assert!(matches!(symbol.semantic_type(), SemanticFieldType::Int));
    }

    #[test]
    fn exposes_a_generic_signature_when_present() {
        use dotty_classfile::signature::ReferenceTypeSignature;

        let signature = FieldSignature(ReferenceTypeSignature::TypeVariable("T".to_owned()));
        let symbol = FieldSymbol::new(
            "items".to_owned(),
            FieldAccessFlags(0x0001),
            FieldType::Object("java/util/List".to_owned()),
            Some(signature.clone()),
            SemanticFieldType::Object(crate::symbol::ClassRef::Unresolved(
                crate::binary_name::BinaryName::from_internal("java/util/List"),
            )),
        );

        assert_eq!(symbol.signature(), Some(&signature));
    }

    #[test]
    fn exposes_its_semantic_type() {
        use crate::binary_name::BinaryName;
        use crate::symbol::ClassRef;

        let symbol = FieldSymbol::new(
            "other".to_owned(),
            FieldAccessFlags(0x0001),
            FieldType::Object("Pong".to_owned()),
            None,
            SemanticFieldType::Object(ClassRef::Unresolved(BinaryName::from_internal("Pong"))),
        );

        assert!(matches!(
            symbol.semantic_type(),
            SemanticFieldType::Object(ClassRef::Unresolved(name)) if name.as_internal() == "Pong"
        ));
    }
}
