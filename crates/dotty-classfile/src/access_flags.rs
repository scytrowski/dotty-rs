//! JVM access-flag bits and context-specific wrappers.
//!
//! Numeric flag values overlap across class, field, and method contexts.
//! Use the wrapper for the kind of declaration being inspected.

/// Publicly accessible class or member.
pub const ACC_PUBLIC: u16 = 0x0001;
/// Private member access.
pub const ACC_PRIVATE: u16 = 0x0002;
/// Protected member access.
pub const ACC_PROTECTED: u16 = 0x0004;
/// Static field or method.
pub const ACC_STATIC: u16 = 0x0008;
/// Class or member cannot be extended, overridden, or reassigned as applicable.
pub const ACC_FINAL: u16 = 0x0010;
/// Class uses the JVM superclass invocation semantics selected by ACC_SUPER.
pub const ACC_SUPER: u16 = 0x0020;
/// Method acquires its monitor during invocation.
pub const ACC_SYNCHRONIZED: u16 = 0x0020;
/// Field uses volatile memory-access semantics.
pub const ACC_VOLATILE: u16 = 0x0040;
/// Compiler-generated bridge method.
pub const ACC_BRIDGE: u16 = 0x0040;
/// Field is excluded from default Java serialization.
pub const ACC_TRANSIENT: u16 = 0x0080;
/// Method declares a variable-arity trailing parameter.
pub const ACC_VARARGS: u16 = 0x0080;
/// Method implementation is native rather than bytecode.
pub const ACC_NATIVE: u16 = 0x0100;
/// Class-file declaration is an interface.
pub const ACC_INTERFACE: u16 = 0x0200;
/// Class or method is abstract.
pub const ACC_ABSTRACT: u16 = 0x0400;
/// Method carries the historical strict floating-point flag.
pub const ACC_STRICT: u16 = 0x0800;
/// Declaration was generated rather than explicitly declared in source.
pub const ACC_SYNTHETIC: u16 = 0x1000;
/// Interface is an annotation type.
pub const ACC_ANNOTATION: u16 = 0x2000;
/// Class or field represents an enumeration declaration or constant.
pub const ACC_ENUM: u16 = 0x4000;
/// Class file is a module descriptor.
pub const ACC_MODULE: u16 = 0x8000;

/// Class/interface `access_flags` (JVMS §4.1, Table 4.1-A).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassAccessFlags(pub u16);

impl ClassAccessFlags {
    /// Returns whether every bit in the supplied mask is set. The mask must use flags for this
    /// wrapper’s class, field, or method context.
    pub fn contains(self, flag: u16) -> bool {
        self.0 & flag == flag
    }

    /// Returns whether this declaration carries the `ACC_PUBLIC` flag.
    pub fn is_public(self) -> bool {
        self.contains(ACC_PUBLIC)
    }

    /// Returns whether this declaration carries the `ACC_FINAL` flag.
    pub fn is_final(self) -> bool {
        self.contains(ACC_FINAL)
    }

    /// Returns whether this declaration carries the `ACC_SUPER` flag.
    pub fn is_super(self) -> bool {
        self.contains(ACC_SUPER)
    }

    /// Returns whether this declaration carries the `ACC_INTERFACE` flag.
    pub fn is_interface(self) -> bool {
        self.contains(ACC_INTERFACE)
    }

    /// Returns whether this declaration carries the `ACC_ABSTRACT` flag.
    pub fn is_abstract(self) -> bool {
        self.contains(ACC_ABSTRACT)
    }

    /// Returns whether this declaration carries the `ACC_SYNTHETIC` flag.
    pub fn is_synthetic(self) -> bool {
        self.contains(ACC_SYNTHETIC)
    }

    /// Returns whether this declaration carries the `ACC_ANNOTATION` flag.
    pub fn is_annotation(self) -> bool {
        self.contains(ACC_ANNOTATION)
    }

    /// Returns whether this declaration carries the `ACC_ENUM` flag.
    pub fn is_enum(self) -> bool {
        self.contains(ACC_ENUM)
    }

    /// Returns whether this declaration carries the `ACC_MODULE` flag.
    pub fn is_module(self) -> bool {
        self.contains(ACC_MODULE)
    }
}

/// Field `access_flags` (JVMS §4.5, Table 4.5-A).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldAccessFlags(pub u16);

impl FieldAccessFlags {
    /// Returns whether every bit in the supplied mask is set. The mask must use flags for this
    /// wrapper’s class, field, or method context.
    pub fn contains(self, flag: u16) -> bool {
        self.0 & flag == flag
    }

    /// Returns whether this declaration carries the `ACC_PUBLIC` flag.
    pub fn is_public(self) -> bool {
        self.contains(ACC_PUBLIC)
    }

    /// Returns whether this declaration carries the `ACC_PRIVATE` flag.
    pub fn is_private(self) -> bool {
        self.contains(ACC_PRIVATE)
    }

    /// Returns whether this declaration carries the `ACC_PROTECTED` flag.
    pub fn is_protected(self) -> bool {
        self.contains(ACC_PROTECTED)
    }

    /// Returns whether this declaration carries the `ACC_STATIC` flag.
    pub fn is_static(self) -> bool {
        self.contains(ACC_STATIC)
    }

    /// Returns whether this declaration carries the `ACC_FINAL` flag.
    pub fn is_final(self) -> bool {
        self.contains(ACC_FINAL)
    }

    /// Returns whether this declaration carries the `ACC_VOLATILE` flag.
    pub fn is_volatile(self) -> bool {
        self.contains(ACC_VOLATILE)
    }

    /// Returns whether this declaration carries the `ACC_TRANSIENT` flag.
    pub fn is_transient(self) -> bool {
        self.contains(ACC_TRANSIENT)
    }

    /// Returns whether this declaration carries the `ACC_SYNTHETIC` flag.
    pub fn is_synthetic(self) -> bool {
        self.contains(ACC_SYNTHETIC)
    }

    /// Returns whether this declaration carries the `ACC_ENUM` flag.
    pub fn is_enum(self) -> bool {
        self.contains(ACC_ENUM)
    }
}

/// Method `access_flags` (JVMS §4.6, Table 4.6-A).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodAccessFlags(pub u16);

impl MethodAccessFlags {
    /// Returns whether every bit in the supplied mask is set. The mask must use flags for this
    /// wrapper’s class, field, or method context.
    pub fn contains(self, flag: u16) -> bool {
        self.0 & flag == flag
    }

    /// Returns whether this declaration carries the `ACC_PUBLIC` flag.
    pub fn is_public(self) -> bool {
        self.contains(ACC_PUBLIC)
    }

    /// Returns whether this declaration carries the `ACC_PRIVATE` flag.
    pub fn is_private(self) -> bool {
        self.contains(ACC_PRIVATE)
    }

    /// Returns whether this declaration carries the `ACC_PROTECTED` flag.
    pub fn is_protected(self) -> bool {
        self.contains(ACC_PROTECTED)
    }

    /// Returns whether this declaration carries the `ACC_STATIC` flag.
    pub fn is_static(self) -> bool {
        self.contains(ACC_STATIC)
    }

    /// Returns whether this declaration carries the `ACC_FINAL` flag.
    pub fn is_final(self) -> bool {
        self.contains(ACC_FINAL)
    }

    /// Returns whether this declaration carries the `ACC_SYNCHRONIZED` flag.
    pub fn is_synchronized(self) -> bool {
        self.contains(ACC_SYNCHRONIZED)
    }

    /// Returns whether this declaration carries the `ACC_BRIDGE` flag.
    pub fn is_bridge(self) -> bool {
        self.contains(ACC_BRIDGE)
    }

    /// Returns whether this declaration carries the `ACC_VARARGS` flag.
    pub fn is_varargs(self) -> bool {
        self.contains(ACC_VARARGS)
    }

    /// Returns whether this declaration carries the `ACC_NATIVE` flag.
    pub fn is_native(self) -> bool {
        self.contains(ACC_NATIVE)
    }

    /// Returns whether this declaration carries the `ACC_ABSTRACT` flag.
    pub fn is_abstract(self) -> bool {
        self.contains(ACC_ABSTRACT)
    }

    /// Returns whether this declaration carries the `ACC_STRICT` flag.
    pub fn is_strict(self) -> bool {
        self.contains(ACC_STRICT)
    }

    /// Returns whether this declaration carries the `ACC_SYNTHETIC` flag.
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
