pub const ACC_PUBLIC: u16 = 0x0001;
pub const ACC_PRIVATE: u16 = 0x0002;
pub const ACC_PROTECTED: u16 = 0x0004;
pub const ACC_STATIC: u16 = 0x0008;
pub const ACC_FINAL: u16 = 0x0010;
pub const ACC_SUPER: u16 = 0x0020;
pub const ACC_SYNCHRONIZED: u16 = 0x0020;
pub const ACC_VOLATILE: u16 = 0x0040;
pub const ACC_BRIDGE: u16 = 0x0040;
pub const ACC_TRANSIENT: u16 = 0x0080;
pub const ACC_VARARGS: u16 = 0x0080;
pub const ACC_NATIVE: u16 = 0x0100;
pub const ACC_INTERFACE: u16 = 0x0200;
pub const ACC_ABSTRACT: u16 = 0x0400;
pub const ACC_STRICT: u16 = 0x0800;
pub const ACC_SYNTHETIC: u16 = 0x1000;
pub const ACC_ANNOTATION: u16 = 0x2000;
pub const ACC_ENUM: u16 = 0x4000;
pub const ACC_MODULE: u16 = 0x8000;

/// Class/interface `access_flags` (JVMS §4.1, Table 4.1-A).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassAccessFlags(pub u16);

impl ClassAccessFlags {
    pub fn contains(self, flag: u16) -> bool {
        self.0 & flag == flag
    }

    pub fn is_public(self) -> bool {
        self.contains(ACC_PUBLIC)
    }

    pub fn is_final(self) -> bool {
        self.contains(ACC_FINAL)
    }

    pub fn is_super(self) -> bool {
        self.contains(ACC_SUPER)
    }

    pub fn is_interface(self) -> bool {
        self.contains(ACC_INTERFACE)
    }

    pub fn is_abstract(self) -> bool {
        self.contains(ACC_ABSTRACT)
    }

    pub fn is_synthetic(self) -> bool {
        self.contains(ACC_SYNTHETIC)
    }

    pub fn is_annotation(self) -> bool {
        self.contains(ACC_ANNOTATION)
    }

    pub fn is_enum(self) -> bool {
        self.contains(ACC_ENUM)
    }

    pub fn is_module(self) -> bool {
        self.contains(ACC_MODULE)
    }
}

/// Field `access_flags` (JVMS §4.5, Table 4.5-A).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldAccessFlags(pub u16);

impl FieldAccessFlags {
    pub fn contains(self, flag: u16) -> bool {
        self.0 & flag == flag
    }

    pub fn is_public(self) -> bool {
        self.contains(ACC_PUBLIC)
    }

    pub fn is_private(self) -> bool {
        self.contains(ACC_PRIVATE)
    }

    pub fn is_protected(self) -> bool {
        self.contains(ACC_PROTECTED)
    }

    pub fn is_static(self) -> bool {
        self.contains(ACC_STATIC)
    }

    pub fn is_final(self) -> bool {
        self.contains(ACC_FINAL)
    }

    pub fn is_volatile(self) -> bool {
        self.contains(ACC_VOLATILE)
    }

    pub fn is_transient(self) -> bool {
        self.contains(ACC_TRANSIENT)
    }

    pub fn is_synthetic(self) -> bool {
        self.contains(ACC_SYNTHETIC)
    }

    pub fn is_enum(self) -> bool {
        self.contains(ACC_ENUM)
    }
}

/// Method `access_flags` (JVMS §4.6, Table 4.6-A).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodAccessFlags(pub u16);

impl MethodAccessFlags {
    pub fn contains(self, flag: u16) -> bool {
        self.0 & flag == flag
    }

    pub fn is_public(self) -> bool {
        self.contains(ACC_PUBLIC)
    }

    pub fn is_private(self) -> bool {
        self.contains(ACC_PRIVATE)
    }

    pub fn is_protected(self) -> bool {
        self.contains(ACC_PROTECTED)
    }

    pub fn is_static(self) -> bool {
        self.contains(ACC_STATIC)
    }

    pub fn is_final(self) -> bool {
        self.contains(ACC_FINAL)
    }

    pub fn is_synchronized(self) -> bool {
        self.contains(ACC_SYNCHRONIZED)
    }

    pub fn is_bridge(self) -> bool {
        self.contains(ACC_BRIDGE)
    }

    pub fn is_varargs(self) -> bool {
        self.contains(ACC_VARARGS)
    }

    pub fn is_native(self) -> bool {
        self.contains(ACC_NATIVE)
    }

    pub fn is_abstract(self) -> bool {
        self.contains(ACC_ABSTRACT)
    }

    pub fn is_strict(self) -> bool {
        self.contains(ACC_STRICT)
    }

    pub fn is_synthetic(self) -> bool {
        self.contains(ACC_SYNTHETIC)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type FlagCase<T> = (&'static str, u16, fn(T) -> bool);

    #[test]
    fn class_access_flag_predicates_detect_their_bit() {
        let cases: &[FlagCase<ClassAccessFlags>] = &[
            ("public", ACC_PUBLIC, ClassAccessFlags::is_public),
            ("final", ACC_FINAL, ClassAccessFlags::is_final),
            ("super", ACC_SUPER, ClassAccessFlags::is_super),
            ("interface", ACC_INTERFACE, ClassAccessFlags::is_interface),
            ("abstract", ACC_ABSTRACT, ClassAccessFlags::is_abstract),
            ("synthetic", ACC_SYNTHETIC, ClassAccessFlags::is_synthetic),
            (
                "annotation",
                ACC_ANNOTATION,
                ClassAccessFlags::is_annotation,
            ),
            ("enum", ACC_ENUM, ClassAccessFlags::is_enum),
            ("module", ACC_MODULE, ClassAccessFlags::is_module),
        ];

        for &(name, bit, predicate) in cases {
            assert!(
                predicate(ClassAccessFlags(bit)),
                "{name} flag should be detected when set"
            );
            assert!(
                !predicate(ClassAccessFlags(0)),
                "{name} flag should not be detected when unset"
            );
        }
    }

    #[test]
    fn field_access_flag_predicates_detect_their_bit() {
        let cases: &[FlagCase<FieldAccessFlags>] = &[
            ("public", ACC_PUBLIC, FieldAccessFlags::is_public),
            ("private", ACC_PRIVATE, FieldAccessFlags::is_private),
            ("protected", ACC_PROTECTED, FieldAccessFlags::is_protected),
            ("static", ACC_STATIC, FieldAccessFlags::is_static),
            ("final", ACC_FINAL, FieldAccessFlags::is_final),
            ("volatile", ACC_VOLATILE, FieldAccessFlags::is_volatile),
            ("transient", ACC_TRANSIENT, FieldAccessFlags::is_transient),
            ("synthetic", ACC_SYNTHETIC, FieldAccessFlags::is_synthetic),
            ("enum", ACC_ENUM, FieldAccessFlags::is_enum),
        ];

        for &(name, bit, predicate) in cases {
            assert!(
                predicate(FieldAccessFlags(bit)),
                "{name} flag should be detected when set"
            );
            assert!(
                !predicate(FieldAccessFlags(0)),
                "{name} flag should not be detected when unset"
            );
        }
    }

    #[test]
    fn method_access_flag_predicates_detect_their_bit() {
        let cases: &[FlagCase<MethodAccessFlags>] = &[
            ("public", ACC_PUBLIC, MethodAccessFlags::is_public),
            ("private", ACC_PRIVATE, MethodAccessFlags::is_private),
            ("protected", ACC_PROTECTED, MethodAccessFlags::is_protected),
            ("static", ACC_STATIC, MethodAccessFlags::is_static),
            ("final", ACC_FINAL, MethodAccessFlags::is_final),
            (
                "synchronized",
                ACC_SYNCHRONIZED,
                MethodAccessFlags::is_synchronized,
            ),
            ("bridge", ACC_BRIDGE, MethodAccessFlags::is_bridge),
            ("varargs", ACC_VARARGS, MethodAccessFlags::is_varargs),
            ("native", ACC_NATIVE, MethodAccessFlags::is_native),
            ("abstract", ACC_ABSTRACT, MethodAccessFlags::is_abstract),
            ("strict", ACC_STRICT, MethodAccessFlags::is_strict),
            ("synthetic", ACC_SYNTHETIC, MethodAccessFlags::is_synthetic),
        ];

        for &(name, bit, predicate) in cases {
            assert!(
                predicate(MethodAccessFlags(bit)),
                "{name} flag should be detected when set"
            );
            assert!(
                !predicate(MethodAccessFlags(0)),
                "{name} flag should not be detected when unset"
            );
        }
    }
}
