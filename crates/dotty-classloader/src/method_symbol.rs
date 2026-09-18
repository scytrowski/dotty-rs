use crate::annotation::SemanticAnnotation;
use dotty_classfile::access_flags::MethodAccessFlags;
use dotty_classfile::descriptor::MethodDescriptor;
use dotty_classfile::signature::MethodSignature;

/// A loaded class's method: its name, access flags, descriptor, and
/// optional generic signature (JVMS §4.6).
///
/// JVMS represents constructors and static initializers as ordinary
/// methods (named `<init>` and `<clinit>` respectively). `<init>` becomes
/// a real `dotty-core` `Symbol` (`SymbolKind::Constructor`) in the owning
/// class's declarations scope, findable by this same `name`; `<clinit>`
/// does not (see `ClassLoader::enter_method`'s doc comment) and is only
/// ever reachable through this sidecar type.
///
/// `descriptor` is exactly what `dotty-classfile` already parses from the
/// method's descriptor (JVMS §4.3.3) — a parameter or return type's
/// `Object`/`Array` entry keeps its class name as an internal-form
/// string. The resolved semantic descriptor is *not* duplicated here: for
/// every method except `<clinit>`, it is a real `Type::Method` on that
/// same real `Symbol`. This type stays purely JVM-facing sidecar
/// metadata — see `docs/classloader.md`'s JVM metadata sidecar section.
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
    annotations: Vec<SemanticAnnotation>,
}

impl MethodSymbol {
    pub fn new(
        name: String,
        flags: MethodAccessFlags,
        descriptor: MethodDescriptor,
        signature: Option<MethodSignature>,
        annotations: Vec<SemanticAnnotation>,
    ) -> Self {
        Self {
            name,
            flags,
            descriptor,
            signature,
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
            Vec::new(),
        );

        assert_eq!(symbol.name(), "run");
        assert_eq!(symbol.flags(), MethodAccessFlags(0x0001));
        assert_eq!(symbol.descriptor(), &descriptor);
        assert_eq!(symbol.signature(), None);
        assert!(symbol.annotations().is_empty());
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
            Vec::new(),
        );

        assert_eq!(symbol.signature(), Some(&signature));
    }
}
