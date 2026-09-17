use dotty_classfile::access_flags::MethodAccessFlags;
use dotty_classfile::descriptor::MethodDescriptor;

/// A loaded class's method: its name, access flags, and descriptor
/// (JVMS §4.6).
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
#[derive(Debug, Clone, PartialEq)]
pub struct MethodSymbol {
    name: String,
    flags: MethodAccessFlags,
    descriptor: MethodDescriptor,
}

impl MethodSymbol {
    pub fn new(name: String, flags: MethodAccessFlags, descriptor: MethodDescriptor) -> Self {
        Self {
            name,
            flags,
            descriptor,
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
        );

        assert_eq!(symbol.name(), "run");
        assert_eq!(symbol.flags(), MethodAccessFlags(0x0001));
        assert_eq!(symbol.descriptor(), &descriptor);
    }
}
