use crate::annotation::SemanticAnnotation;
use crate::binary_name::BinaryName;
use crate::field_symbol::FieldSymbol;
use crate::method_symbol::MethodSymbol;
use crate::nesting::{EnclosingMethodRef, InnerClassEntry};
use crate::record_component::RecordComponentSymbol;
use dotty_classfile::access_flags::ClassAccessFlags;
use dotty_classfile::signature::ClassSignature;
use std::cell::RefCell;
use std::fmt;
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
/// direct superclass/interfaces, its fields, its methods, and its
/// optional generic signature.
///
/// Owns its data — it never borrows from the decode buffer that produced
/// it, matching the borrowed/owned separation `AGENTS.md` requires between
/// decoder and higher-level representations.
///
/// `name` and `flags` are known synchronously right after decoding a
/// class file, before any recursive resolution, so they are plain
/// fields. Everything else depends on resolving other classes (a
/// superclass, interfaces, or — from `docs/classloader.md` §9's
/// Milestone 6 onward — a member's declared type), which can recurse
/// back into a class that is still being loaded for a legitimate mutual
/// reference (e.g. two classes each having a field of the other's
/// type). That requires this same `ClassSymbol` identity (the same
/// `Rc`) to exist, empty, before its data is known, and to be mutated
/// in place once resolution finishes — hence `RefCell`. See
/// [`ClassSymbol::new_shell`]/[`ClassSymbol::complete`].
///
/// `signature` is `None` unless the class carries a `Signature`
/// attribute (JVMS §4.7.9.1) — the common case for a non-generic class.
/// It is the raw parsed grammar tree from `dotty-classfile`, verbatim:
/// no resolution of the class names it mentions, no semantic type model
/// built from it (`docs/classloader.md` §3/§9).
///
/// `Debug` is implemented manually (not derived) to print only the
/// name: a legitimate mutual member-type reference (Milestone 6) means
/// two `ClassSymbol`s can genuinely reference each other through
/// `Rc<ClassSymbol>` — a derived, field-recursing `Debug` would recurse
/// forever printing such a pair.
///
/// `nest_host`/`nest_members`/`permitted_subclasses`/`inner_classes`/
/// `enclosing_method`/`record_components`/`annotations` are
/// `docs/classloader.md` §9's Milestone 7: the remaining class-level
/// attributes `dotty-classfile` already decodes but this crate didn't
/// yet consume. `record_components` is `None` unless the class carries
/// a `Record` attribute at all (`Some(vec![])` is a real, distinct
/// state: a record with zero components), matching JVMS §4.7.30.
#[derive(Clone)]
pub struct ClassSymbol {
    name: BinaryName,
    flags: ClassAccessFlags,
    super_class: RefCell<Option<ClassRef>>,
    interfaces: RefCell<Vec<ClassRef>>,
    fields: RefCell<Vec<FieldSymbol>>,
    methods: RefCell<Vec<MethodSymbol>>,
    signature: RefCell<Option<ClassSignature>>,
    nest_host: RefCell<Option<ClassRef>>,
    nest_members: RefCell<Vec<ClassRef>>,
    permitted_subclasses: RefCell<Vec<ClassRef>>,
    inner_classes: RefCell<Vec<InnerClassEntry>>,
    enclosing_method: RefCell<Option<EnclosingMethodRef>>,
    record_components: RefCell<Option<Vec<RecordComponentSymbol>>>,
    annotations: RefCell<Vec<SemanticAnnotation>>,
}

impl fmt::Debug for ClassSymbol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ClassSymbol({})", self.name)
    }
}

impl ClassSymbol {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: BinaryName,
        flags: ClassAccessFlags,
        super_class: Option<ClassRef>,
        interfaces: Vec<ClassRef>,
        fields: Vec<FieldSymbol>,
        methods: Vec<MethodSymbol>,
        signature: Option<ClassSignature>,
        nest_host: Option<ClassRef>,
        nest_members: Vec<ClassRef>,
        permitted_subclasses: Vec<ClassRef>,
        inner_classes: Vec<InnerClassEntry>,
        enclosing_method: Option<EnclosingMethodRef>,
        record_components: Option<Vec<RecordComponentSymbol>>,
        annotations: Vec<SemanticAnnotation>,
    ) -> Self {
        let symbol = Self::new_shell(name, flags);
        symbol.complete(
            super_class,
            interfaces,
            fields,
            methods,
            signature,
            nest_host,
            nest_members,
            permitted_subclasses,
            inner_classes,
            enclosing_method,
            record_components,
            annotations,
        );
        symbol
    }

    /// An empty shell for `name`/`flags` — every resolved field starts
    /// empty. See the type's own doc comment for why this exists:
    /// other classes may capture this same `Rc<ClassSymbol>` while it is
    /// still a shell, for a legitimate mutual member-type reference.
    pub(crate) fn new_shell(name: BinaryName, flags: ClassAccessFlags) -> Self {
        Self {
            name,
            flags,
            super_class: RefCell::new(None),
            interfaces: RefCell::new(Vec::new()),
            fields: RefCell::new(Vec::new()),
            methods: RefCell::new(Vec::new()),
            signature: RefCell::new(None),
            nest_host: RefCell::new(None),
            nest_members: RefCell::new(Vec::new()),
            permitted_subclasses: RefCell::new(Vec::new()),
            inner_classes: RefCell::new(Vec::new()),
            enclosing_method: RefCell::new(None),
            record_components: RefCell::new(None),
            annotations: RefCell::new(Vec::new()),
        }
    }

    /// Fills in a shell's resolved data in place. Called exactly once,
    /// after everything it needs has itself been resolved.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn complete(
        &self,
        super_class: Option<ClassRef>,
        interfaces: Vec<ClassRef>,
        fields: Vec<FieldSymbol>,
        methods: Vec<MethodSymbol>,
        signature: Option<ClassSignature>,
        nest_host: Option<ClassRef>,
        nest_members: Vec<ClassRef>,
        permitted_subclasses: Vec<ClassRef>,
        inner_classes: Vec<InnerClassEntry>,
        enclosing_method: Option<EnclosingMethodRef>,
        record_components: Option<Vec<RecordComponentSymbol>>,
        annotations: Vec<SemanticAnnotation>,
    ) {
        *self.super_class.borrow_mut() = super_class;
        *self.interfaces.borrow_mut() = interfaces;
        *self.fields.borrow_mut() = fields;
        *self.methods.borrow_mut() = methods;
        *self.signature.borrow_mut() = signature;
        *self.nest_host.borrow_mut() = nest_host;
        *self.nest_members.borrow_mut() = nest_members;
        *self.permitted_subclasses.borrow_mut() = permitted_subclasses;
        *self.inner_classes.borrow_mut() = inner_classes;
        *self.enclosing_method.borrow_mut() = enclosing_method;
        *self.record_components.borrow_mut() = record_components;
        *self.annotations.borrow_mut() = annotations;
    }

    pub fn name(&self) -> &BinaryName {
        &self.name
    }

    pub fn flags(&self) -> ClassAccessFlags {
        self.flags
    }

    pub fn super_class(&self) -> Option<ClassRef> {
        self.super_class.borrow().clone()
    }

    pub fn interfaces(&self) -> Vec<ClassRef> {
        self.interfaces.borrow().clone()
    }

    pub fn fields(&self) -> Vec<FieldSymbol> {
        self.fields.borrow().clone()
    }

    pub fn methods(&self) -> Vec<MethodSymbol> {
        self.methods.borrow().clone()
    }

    pub fn signature(&self) -> Option<ClassSignature> {
        self.signature.borrow().clone()
    }

    pub fn nest_host(&self) -> Option<ClassRef> {
        self.nest_host.borrow().clone()
    }

    pub fn nest_members(&self) -> Vec<ClassRef> {
        self.nest_members.borrow().clone()
    }

    pub fn permitted_subclasses(&self) -> Vec<ClassRef> {
        self.permitted_subclasses.borrow().clone()
    }

    pub fn inner_classes(&self) -> Vec<InnerClassEntry> {
        self.inner_classes.borrow().clone()
    }

    pub fn enclosing_method(&self) -> Option<EnclosingMethodRef> {
        self.enclosing_method.borrow().clone()
    }

    pub fn record_components(&self) -> Option<Vec<RecordComponentSymbol>> {
        self.record_components.borrow().clone()
    }

    pub fn annotations(&self) -> Vec<SemanticAnnotation> {
        self.annotations.borrow().clone()
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
            Vec::new(),
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            None,
            Vec::new(),
        ));
        let runnable_ref = ClassRef::Unresolved(BinaryName::from_internal("java/lang/Runnable"));

        let symbol = ClassSymbol::new(
            BinaryName::from_internal("PoolSample"),
            ClassAccessFlags(0x0021),
            Some(ClassRef::Resolved(object_symbol.clone())),
            vec![runnable_ref],
            Vec::new(),
            Vec::new(),
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            None,
            Vec::new(),
        );

        assert_eq!(symbol.name().as_internal(), "PoolSample");
        assert_eq!(symbol.flags(), ClassAccessFlags(0x0021));
        assert!(matches!(
            symbol.super_class(),
            Some(ClassRef::Resolved(resolved)) if resolved.name() == object_symbol.name()
        ));
        assert!(matches!(
            symbol.interfaces().as_slice(),
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
            Vec::new(),
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            None,
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
            None,
            crate::semantic_type::SemanticFieldType::Int,
        );
        let symbol = ClassSymbol::new(
            BinaryName::from_internal("PoolSample"),
            ClassAccessFlags(0x0021),
            None,
            Vec::new(),
            vec![field],
            Vec::new(),
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            None,
            Vec::new(),
        );

        assert!(matches!(symbol.fields().as_slice(), [only] if only.name() == "ANSWER"));
    }

    #[test]
    fn exposes_its_methods() {
        use dotty_classfile::access_flags::MethodAccessFlags;
        use dotty_classfile::descriptor::MethodDescriptor;

        let method = MethodSymbol::new(
            "run".to_owned(),
            MethodAccessFlags(0x0001),
            MethodDescriptor {
                parameters: vec![],
                return_type: None,
            },
            None,
            crate::semantic_type::SemanticMethodDescriptor {
                parameters: vec![],
                return_type: None,
            },
        );
        let symbol = ClassSymbol::new(
            BinaryName::from_internal("PoolSample"),
            ClassAccessFlags(0x0021),
            None,
            Vec::new(),
            Vec::new(),
            vec![method],
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            None,
            Vec::new(),
        );

        assert!(matches!(symbol.methods().as_slice(), [only] if only.name() == "run"));
    }

    #[test]
    fn exposes_its_generic_signature_when_present() {
        use dotty_classfile::signature::ClassTypeSignature;

        let signature = ClassSignature {
            type_parameters: vec![],
            superclass: ClassTypeSignature {
                package: vec!["java".to_owned(), "lang".to_owned()],
                simple_name: "Object".to_owned(),
                type_arguments: vec![],
                suffix: vec![],
            },
            superinterfaces: vec![],
        };
        let symbol = ClassSymbol::new(
            BinaryName::from_internal("GenericSample"),
            ClassAccessFlags(0x0021),
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(signature.clone()),
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            None,
            Vec::new(),
        );

        assert_eq!(symbol.signature(), Some(signature));
    }

    #[test]
    fn has_no_signature_by_default() {
        let symbol = ClassSymbol::new(
            BinaryName::from_internal("PoolSample"),
            ClassAccessFlags(0x0021),
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            None,
            Vec::new(),
        );

        assert_eq!(symbol.signature(), None);
    }

    #[test]
    fn a_shell_starts_empty_and_reflects_completion_in_place() {
        let shell = Rc::new(ClassSymbol::new_shell(
            BinaryName::from_internal("Ping"),
            ClassAccessFlags(0x0021),
        ));

        assert!(shell.super_class().is_none());
        assert!(shell.fields().is_empty());

        shell.complete(
            Some(ClassRef::Unresolved(BinaryName::from_internal(
                "java/lang/Object",
            ))),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            None,
            Vec::new(),
        );

        assert!(matches!(
            shell.super_class(),
            Some(ClassRef::Unresolved(name)) if name.as_internal() == "java/lang/Object"
        ));
    }
}
