use crate::binary_name::BinaryName;
use crate::field_symbol::FieldSymbol;
use dotty_classfile::access_flags::ClassAccessFlags;
use std::rc::Rc;

/// A reference to a class/interface from within another class's symbol,
/// e.g. a superclass or an implemented interface.
///
/// Superclass and interface references are always `Resolved` once a class
/// finishes loading (`ClassLoader` resolves them eagerly, see
/// `docs/classloader.md` §5). `Unresolved` is reserved for future lazy
/// member-type resolution (`docs/classloader.md` §6), so this shape does
/// not need to change when that lands.
#[derive(Debug, Clone)]
pub enum ClassRef {
    Unresolved(BinaryName),
    Resolved(Rc<ClassSymbol>),
}

/// A loaded class or interface's symbol: its name, access flags, its
/// direct superclass/interfaces, and its fields.
///
/// Owns its data — it never borrows from the decode buffer that produced
/// it, matching the borrowed/owned separation `AGENTS.md` requires between
/// decoder and higher-level representations.
#[derive(Debug, Clone)]
pub struct ClassSymbol {
    name: BinaryName,
    flags: ClassAccessFlags,
    super_class: Option<ClassRef>,
    interfaces: Vec<ClassRef>,
    fields: Vec<FieldSymbol>,
}

impl ClassSymbol {
    pub fn new(
        name: BinaryName,
        flags: ClassAccessFlags,
        super_class: Option<ClassRef>,
        interfaces: Vec<ClassRef>,
        fields: Vec<FieldSymbol>,
    ) -> Self {
        Self {
            name,
            flags,
            super_class,
            interfaces,
            fields,
        }
    }

    pub fn name(&self) -> &BinaryName {
        &self.name
    }

    pub fn flags(&self) -> ClassAccessFlags {
        self.flags
    }

    pub fn super_class(&self) -> Option<&ClassRef> {
        self.super_class.as_ref()
    }

    pub fn interfaces(&self) -> &[ClassRef] {
        &self.interfaces
    }

    pub fn fields(&self) -> &[FieldSymbol] {
        &self.fields
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_name_flags_super_class_and_interfaces() {
        let object_symbol = Rc::new(ClassSymbol::new(
            BinaryName::from_internal("java/lang/Object"),
            ClassAccessFlags(0x0021),
            None,
            Vec::new(),
            Vec::new(),
        ));
        let runnable_ref = ClassRef::Unresolved(BinaryName::from_internal("java/lang/Runnable"));

        let symbol = ClassSymbol::new(
            BinaryName::from_internal("PoolSample"),
            ClassAccessFlags(0x0021),
            Some(ClassRef::Resolved(object_symbol.clone())),
            vec![runnable_ref],
            Vec::new(),
        );

        assert_eq!(symbol.name().as_internal(), "PoolSample");
        assert_eq!(symbol.flags(), ClassAccessFlags(0x0021));
        assert!(matches!(
            symbol.super_class(),
            Some(ClassRef::Resolved(resolved)) if resolved.name() == object_symbol.name()
        ));
        assert!(matches!(
            symbol.interfaces(),
            [ClassRef::Unresolved(name)] if name.as_internal() == "java/lang/Runnable"
        ));
    }

    #[test]
    fn a_class_without_a_super_class_has_none() {
        let symbol = ClassSymbol::new(
            BinaryName::from_internal("java/lang/Object"),
            ClassAccessFlags(0x0021),
            None,
            Vec::new(),
            Vec::new(),
        );

        assert!(symbol.super_class().is_none());
        assert!(symbol.interfaces().is_empty());
    }

    #[test]
    fn exposes_its_fields() {
        use dotty_classfile::access_flags::FieldAccessFlags;
        use dotty_classfile::descriptor::FieldType;

        let field = FieldSymbol::new(
            "ANSWER".to_owned(),
            FieldAccessFlags(0x0019),
            FieldType::Int,
        );
        let symbol = ClassSymbol::new(
            BinaryName::from_internal("PoolSample"),
            ClassAccessFlags(0x0021),
            None,
            Vec::new(),
            vec![field],
        );

        assert!(matches!(symbol.fields(), [only] if only.name() == "ANSWER"));
    }
}
