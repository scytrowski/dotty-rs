use crate::semantic_type::SemanticFieldType;
use dotty_classfile::descriptor::FieldType;
use dotty_classfile::signature::FieldSignature;

/// One component of a `record` class's `Record` attribute (JVMS
/// §4.7.30): its name, declared type, optional generic signature, and
/// semantic (class-reference-resolved) type.
///
/// Same shape as [`crate::field_symbol::FieldSymbol`] minus access
/// flags — a record component has none in the class file format — and
/// the same "attach without replacing" relationship between
/// `field_type`/`signature` (raw, verbatim from `dotty-classfile`) and
/// `semantic_type` (resolved by `ClassLoader`).
#[derive(Debug, Clone)]
pub struct RecordComponentSymbol {
    name: String,
    field_type: FieldType,
    signature: Option<FieldSignature>,
    semantic_type: SemanticFieldType,
}

impl RecordComponentSymbol {
    pub fn new(
        name: String,
        field_type: FieldType,
        signature: Option<FieldSignature>,
        semantic_type: SemanticFieldType,
    ) -> Self {
        Self {
            name,
            field_type,
            signature,
            semantic_type,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
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
    fn exposes_name_field_type_and_semantic_type() {
        let component = RecordComponentSymbol::new(
            "radius".to_owned(),
            FieldType::Double,
            None,
            SemanticFieldType::Double,
        );

        assert_eq!(component.name(), "radius");
        assert_eq!(component.field_type(), &FieldType::Double);
        assert_eq!(component.signature(), None);
        assert!(matches!(
            component.semantic_type(),
            SemanticFieldType::Double
        ));
    }
}
