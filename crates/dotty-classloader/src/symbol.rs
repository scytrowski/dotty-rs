use crate::annotation::SemanticAnnotation;
use crate::binary_name::BinaryName;
use crate::field_symbol::FieldSymbol;
use crate::method_symbol::MethodSymbol;
use crate::nesting::{EnclosingMethodRef, InnerClassEntry};
use crate::record_component::RecordComponentSymbol;
use dotty_classfile::access_flags::ClassAccessFlags;
use dotty_classfile::signature::ClassSignature;
use dotty_core::SymbolId;

/// A reference to a class/interface used only by JVM-specific sidecar
/// metadata: a nest host/member, a permitted subclass, an inner/enclosing
/// class, or an annotation's own type. Never used for a class's canonical
/// superclass/interfaces — those are `Type::TypeRef`s in its
/// `Type::ClassInfo`'s `parents` (`dotty-core`'s semantic model), not this
/// crate's.
///
/// `Resolved` carries a bare [`SymbolId`], itself a stable, `Copy`,
/// session-wide identity — unlike the `Rc<ClassSymbol>` this replaced,
/// there is nothing left to share ownership of. `ClassRef` as a whole is
/// only `Clone`, not `Copy`: `Unresolved` still carries an owned
/// [`BinaryName`] (a class not yet loaded has no `SymbolId` to be `Copy`
/// about).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassRef {
    /// Symbolic binary class name awaiting resolution.
    Unresolved(BinaryName),
    /// Canonical class declaration identity in the current semantic session.
    Resolved(SymbolId),
}

/// The JVM-specific metadata `ClassLoader` decodes for a `.class`-backed
/// symbol but which does not belong in `dotty-core`'s canonical semantic
/// model (`docs/classloader.md`'s JVM metadata sidecar section): raw
/// access flags, fields/methods (still in their own erased-descriptor
/// shape, not yet `dotty-core` symbols — that is a later migration step),
/// the class's own raw generic `Signature`, and the remaining class-level
/// attributes (`NestHost`/`NestMembers`/`PermittedSubclasses`/
/// `InnerClasses`/`EnclosingMethod`/`Record`/annotations).
///
/// Kept in `ClassLoader`'s own `SymbolId`-keyed metadata table — this is
/// supplementary, diagnostic-and-bytecode-facing data, not the canonical
/// semantic representation of the class (that is its `Symbol` plus
/// `Type::ClassInfo`, in the `SemanticStore` itself).
///
/// Only produced for `.class`-backed symbols: a `.tasty`-backed symbol has
/// no entry in the metadata table yet (`docs/classloader.md`'s `.tasty`
/// convergence section) — nothing beyond name/flags/superclass/interfaces
/// is reconstructed from `.tasty` yet, the same "not yet populated" shape
/// earlier `.class`-only milestones used before their corresponding
/// feature landed.
#[derive(Debug, Clone)]
pub struct ClassfileMetadata {
    /// JVM binary name identifying the class.
    pub binary_name: BinaryName,
    /// Raw or typed JVM access flags; interpretation depends on the enclosing item.
    pub access_flags: ClassAccessFlags,
    /// Declared fields in class-file order.
    pub fields: Vec<FieldSymbol>,
    /// Declared methods in source or class-file order.
    pub methods: Vec<MethodSymbol>,
    /// Optional generic signature or semantic callable signature.
    pub signature: Option<ClassSignature>,
    /// Optional JVM nest host used for private-access semantics.
    pub nest_host: Option<ClassRef>,
    /// Classes recorded as members of this JVM nest.
    pub nest_members: Vec<ClassRef>,
    /// Direct subclasses permitted by a sealed class file.
    pub permitted_subclasses: Vec<ClassRef>,
    /// Nested-class table entries in class-file order.
    pub inner_classes: Vec<InnerClassEntry>,
    /// Enclosing method metadata for a local or anonymous class.
    pub enclosing_method: Option<EnclosingMethodRef>,
    /// Ordered record components, absent for non-record classes.
    pub record_components: Option<Vec<RecordComponentSymbol>>,
    /// Annotations attached to this item, preserving their recorded order.
    pub annotations: Vec<SemanticAnnotation>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::SemanticStore;

    fn some_symbol_id(store: &mut SemanticStore) -> SymbolId {
        use dotty_core::{
            Name, Namespace, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks,
            SymbolOrigin, Visibility,
        };

        let text = store.names.intern("Foo");
        store.symbols.alloc(Symbol {
            name: Name::new(text, Namespace::Type),
            owner: None,
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        })
    }

    #[test]
    fn resolved_carries_a_bare_symbol_id() {
        let mut store = SemanticStore::new();
        let id = some_symbol_id(&mut store);

        let reference = ClassRef::Resolved(id);

        assert_eq!(reference, ClassRef::Resolved(id));
    }

    #[test]
    fn unresolved_carries_a_binary_name() {
        let reference = ClassRef::Unresolved(BinaryName::from_internal("java/lang/Runnable"));

        assert!(matches!(
            reference,
            ClassRef::Unresolved(name) if name.as_internal() == "java/lang/Runnable"
        ));
    }

    #[test]
    fn resolved_class_refs_are_cloneable_and_compare_equal() {
        let mut store = SemanticStore::new();
        let id = some_symbol_id(&mut store);
        let reference = ClassRef::Resolved(id);

        let cloned = reference.clone();

        assert_eq!(reference, cloned);
    }

    #[test]
    fn classfile_metadata_exposes_the_data_it_was_built_with() {
        let metadata = ClassfileMetadata {
            binary_name: BinaryName::from_internal("PoolSample"),
            access_flags: ClassAccessFlags(0x0021),
            fields: Vec::new(),
            methods: Vec::new(),
            signature: None,
            nest_host: None,
            nest_members: Vec::new(),
            permitted_subclasses: Vec::new(),
            inner_classes: Vec::new(),
            enclosing_method: None,
            record_components: None,
            annotations: Vec::new(),
        };

        assert_eq!(metadata.binary_name.as_internal(), "PoolSample");
        assert_eq!(metadata.access_flags, ClassAccessFlags(0x0021));
        assert!(metadata.fields.is_empty());
        assert!(metadata.record_components.is_none());
    }
}
