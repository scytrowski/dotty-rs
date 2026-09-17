use dotty_classfile::access_flags::FieldAccessFlags;
use dotty_classfile::descriptor::FieldType;
use dotty_classfile::signature::FieldSignature;

/// A loaded class's field: its name, access flags, declared type, and
/// optional generic signature (JVMS §4.5).
///
/// `field_type` is exactly what `dotty-classfile` already parses from the
/// field's descriptor (JVMS §4.3.2) — an `Object`/`Array` entry's class
/// name stays an internal-form string, not a resolved [`crate::ClassRef`].
/// Resolving those names into loaded classes is a later milestone
/// (`docs/classloader.md` §3/§9).
///
/// `signature` is `None` unless the field carries a `Signature` attribute
/// (JVMS §4.7.9.1) — the common case for a non-generic field type. It is
/// the raw parsed grammar tree from `dotty-classfile`, verbatim: no
/// resolution of the class names it mentions, no semantic type model
/// built from it (`docs/classloader.md` §3/§9).
#[derive(Debug, Clone, PartialEq)]
pub struct FieldSymbol {
    name: String,
    flags: FieldAccessFlags,
    field_type: FieldType,
    signature: Option<FieldSignature>,
}

impl FieldSymbol {
    pub fn new(
        name: String,
        flags: FieldAccessFlags,
        field_type: FieldType,
        signature: Option<FieldSignature>,
    ) -> Self {
        Self {
            name,
            flags,
            field_type,
            signature,
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
        );

        assert_eq!(symbol.name(), "ANSWER");
        assert_eq!(symbol.flags(), FieldAccessFlags(0x0019));
        assert_eq!(symbol.field_type(), &FieldType::Int);
        assert_eq!(symbol.signature(), None);
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
        );

        assert_eq!(symbol.signature(), Some(&signature));
    }
}
