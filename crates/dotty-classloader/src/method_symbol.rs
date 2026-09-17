use dotty_classfile::access_flags::MethodAccessFlags;
use dotty_classfile::descriptor::MethodDescriptor;
use dotty_classfile::signature::MethodSignature;

/// A loaded class's method: its name, access flags, descriptor, and
/// optional generic signature (JVMS §4.6).
///
/// JVMS represents constructors and static initializers as ordinary
/// methods (named `<init>` and `<clinit>` respectively), so this type
/// covers "constructors" per `docs/classloader.md` §9 with no special
/// casing.
///
/// `descriptor` is exactly what `dotty-classfile` already parses from
/// the method's descriptor (JVMS §4.3.3) — a parameter or return type's
/// `Object`/`Array` entry keeps its class name as an internal-form
/// string, not a resolved [`crate::ClassRef`]. Resolving those names
/// into loaded classes is a later milestone (`docs/classloader.md` §3/§9).
///
/// `signature` is `None` unless the method carries a `Signature`
/// attribute (JVMS §4.7.9.1) — the common case for a non-generic method.
/// It is the raw parsed grammar tree from `dotty-classfile`, verbatim:
/// no resolution of the class names it mentions, no semantic type model
/// built from it (`docs/classloader.md` §3/§9).
#[derive(Debug, Clone, PartialEq)]
pub struct MethodSymbol {
    name: String,
    flags: MethodAccessFlags,
    descriptor: MethodDescriptor,
    signature: Option<MethodSignature>,
}

impl MethodSymbol {
    pub fn new(
        name: String,
        flags: MethodAccessFlags,
        descriptor: MethodDescriptor,
        signature: Option<MethodSignature>,
    ) -> Self {
        Self {
            name,
            flags,
            descriptor,
            signature,
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
        );

        assert_eq!(symbol.name(), "run");
        assert_eq!(symbol.flags(), MethodAccessFlags(0x0001));
        assert_eq!(symbol.descriptor(), &descriptor);
        assert_eq!(symbol.signature(), None);
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
        );

        assert_eq!(symbol.signature(), Some(&signature));
    }
}
