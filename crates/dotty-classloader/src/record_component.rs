use dotty_classfile::descriptor::FieldType;
use dotty_classfile::signature::FieldSignature;
use dotty_core::TypeId;

/// One component of a `record` class's `Record` attribute (JVMS
/// §4.7.30): its name, declared type, optional generic signature, and
/// resolved semantic type.
///
/// Same shape as [`crate::field_symbol::FieldSymbol`] minus access flags —
/// a record component has none in the class file format. `resolved_type`
/// is a `dotty-core` `TypeId` (the same descriptor-lowering pipeline a real
/// field's `Symbol` uses), not a `Symbol`/`Scope` entry of its own: a
/// record component describes the shape of an existing field/accessor
/// method pair rather than introducing new semantic identity.
#[derive(Debug, Clone)]
pub struct RecordComponentSymbol {
    name: String,
    field_type: FieldType,
    signature: Option<FieldSignature>,
    resolved_type: TypeId,
}

impl RecordComponentSymbol {
    pub fn new(
        name: String,
        field_type: FieldType,
        signature: Option<FieldSignature>,
        resolved_type: TypeId,
    ) -> Self {
        Self {
            name,
            field_type,
            signature,
            resolved_type,
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

    pub fn resolved_type(&self) -> TypeId {
        self.resolved_type
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::{SemanticStore, Type};

    #[test]
    fn exposes_name_field_type_and_resolved_type() {
        let mut store = SemanticStore::new();
        let resolved_type = store.types.alloc(Type::NoPrefix);

        let component =
            RecordComponentSymbol::new("radius".to_owned(), FieldType::Double, None, resolved_type);

        assert_eq!(component.name(), "radius");
        assert_eq!(component.field_type(), &FieldType::Double);
        assert_eq!(component.signature(), None);
        assert_eq!(component.resolved_type(), resolved_type);
    }
}
