use crate::annotation::SemanticAnnotation;
use crate::semantic_type::SemanticMethodDescriptor;
use dotty_classfile::access_flags::MethodAccessFlags;
use dotty_classfile::descriptor::MethodDescriptor;
use dotty_classfile::signature::MethodSignature;

/// A loaded class's method: its name, access flags, descriptor,
/// optional generic signature, and semantic (class-reference-resolved)
/// descriptor (JVMS §4.6).
///
/// JVMS represents constructors and static initializers as ordinary
/// methods (named `<init>` and `<clinit>` respectively), so this type
/// covers "constructors" per `docs/classloader.md` §9 with no special
/// casing.
///
/// `descriptor` is exactly what `dotty-classfile` already parses from
/// the method's descriptor (JVMS §4.3.3) — a parameter or return type's
/// `Object`/`Array` entry keeps its class name as an internal-form
/// string, not a resolved [`crate::ClassRef`]. `semantic_descriptor` is
/// the resolved counterpart, built by `ClassLoader`
/// (`docs/classloader.md` §9, Milestone 6); kept alongside `descriptor`
/// rather than replacing it, the same "attach without replacing"
/// approach used for `signature` and for `FieldSymbol::semantic_type`.
///
/// `signature` is `None` unless the method carries a `Signature`
/// attribute (JVMS §4.7.9.1) — the common case for a non-generic method.
/// It is the raw parsed grammar tree from `dotty-classfile`, verbatim:
/// no resolution of the class names it mentions, no semantic type model
/// built from it.
#[derive(Debug, Clone)]
pub struct MethodSymbol {
    name: String,
    flags: MethodAccessFlags,
    descriptor: MethodDescriptor,
    signature: Option<MethodSignature>,
    semantic_descriptor: SemanticMethodDescriptor,
    annotations: Vec<SemanticAnnotation>,
}

impl MethodSymbol {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        flags: MethodAccessFlags,
        descriptor: MethodDescriptor,
        signature: Option<MethodSignature>,
        semantic_descriptor: SemanticMethodDescriptor,
        annotations: Vec<SemanticAnnotation>,
    ) -> Self {
        Self {
            name,
            flags,
            descriptor,
            signature,
            semantic_descriptor,
            annotations,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn flags(&self) -> MethodAccessFlags {
        self.flags
    }

    pub fn descriptor(&self) -> &MethodDescriptor {
        &self.descriptor
    }

    pub fn signature(&self) -> Option<&MethodSignature> {
        self.signature.as_ref()
    }

    pub fn semantic_descriptor(&self) -> &SemanticMethodDescriptor {
        &self.semantic_descriptor
    }

    pub fn annotations(&self) -> &[SemanticAnnotation] {
        &self.annotations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_name_flags_and_descriptor() {
        let descriptor = MethodDescriptor {
            parameters: vec![],
            return_type: None,
        };
        let symbol = MethodSymbol::new(
            "run".to_owned(),
            MethodAccessFlags(0x0001),
            descriptor.clone(),
            None,
            SemanticMethodDescriptor {
                parameters: vec![],
                return_type: None,
            },
            Vec::new(),
        );

        assert_eq!(symbol.name(), "run");
        assert_eq!(symbol.flags(), MethodAccessFlags(0x0001));
        assert_eq!(symbol.descriptor(), &descriptor);
        assert_eq!(symbol.signature(), None);
        assert!(symbol.semantic_descriptor().parameters.is_empty());
        assert!(symbol.semantic_descriptor().return_type.is_none());
    }

    #[test]
    fn exposes_a_generic_signature_when_present() {
        use dotty_classfile::descriptor::FieldType;
        use dotty_classfile::signature::{ReferenceTypeSignature, TypeSignature};

        let signature = MethodSignature {
            type_parameters: vec![],
            parameters: vec![],
            result: Some(TypeSignature::Reference(
                ReferenceTypeSignature::TypeVariable("T".to_owned()),
            )),
            throws: vec![],
        };
        let symbol = MethodSymbol::new(
            "first".to_owned(),
            MethodAccessFlags(0x0001),
            MethodDescriptor {
                parameters: vec![],
                return_type: Some(FieldType::Object("java/lang/Comparable".to_owned())),
            },
            Some(signature.clone()),
            SemanticMethodDescriptor {
                parameters: vec![],
                return_type: Some(crate::semantic_type::SemanticFieldType::Object(
                    crate::symbol::ClassRef::Unresolved(
                        crate::binary_name::BinaryName::from_internal("java/lang/Comparable"),
                    ),
                )),
            },
            Vec::new(),
        );

        assert_eq!(symbol.signature(), Some(&signature));
    }

    #[test]
    fn exposes_its_semantic_descriptor() {
        use crate::binary_name::BinaryName;
        use crate::semantic_type::SemanticFieldType;
        use crate::symbol::ClassRef;

        let symbol = MethodSymbol::new(
            "exchange".to_owned(),
            MethodAccessFlags(0x0001),
            MethodDescriptor {
                parameters: vec![dotty_classfile::descriptor::FieldType::Object(
                    "Pong".to_owned(),
                )],
                return_type: Some(dotty_classfile::descriptor::FieldType::Object(
                    "Pong".to_owned(),
                )),
            },
            None,
            SemanticMethodDescriptor {
                parameters: vec![SemanticFieldType::Object(ClassRef::Unresolved(
                    BinaryName::from_internal("Pong"),
                ))],
                return_type: Some(SemanticFieldType::Object(ClassRef::Unresolved(
                    BinaryName::from_internal("Pong"),
                ))),
            },
            Vec::new(),
        );

        assert!(matches!(
            symbol.semantic_descriptor().parameters.as_slice(),
            [SemanticFieldType::Object(ClassRef::Unresolved(name))] if name.as_internal() == "Pong"
        ));
        assert!(matches!(
            &symbol.semantic_descriptor().return_type,
            Some(SemanticFieldType::Object(ClassRef::Unresolved(name))) if name.as_internal() == "Pong"
        ));
    }
}
