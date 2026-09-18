use crate::symbol::ClassRef;
use dotty_classfile::access_flags::ClassAccessFlags;

/// One entry of a class's `InnerClasses` attribute (JVMS §4.7.6):
/// `inner_class` is always present; `outer_class`/`inner_name` are
/// `None` for a local or anonymous class, which has no enclosing class
/// reference or simple name of its own in this attribute.
///
/// `inner_class`/`outer_class` are resolved the same tolerant way
/// `docs/classloader.md` §9's Milestone 6 resolves member types
/// (`ClassLoader::resolve_member_class`): a nest host and its member
/// classes commonly reference each other, which is a legitimate mutual
/// reference, not a supertype cycle.
#[derive(Debug, Clone)]
pub struct InnerClassEntry {
    pub inner_class: ClassRef,
    pub outer_class: Option<ClassRef>,
    pub inner_name: Option<String>,
    pub access_flags: ClassAccessFlags,
}

/// A class's `EnclosingMethod` attribute (JVMS §4.7.7), present only on
/// a local or anonymous class.
///
/// `class` is the immediately enclosing class, resolved the same
/// tolerant way as [`InnerClassEntry`]'s references. `method` is the
/// enclosing method or constructor, if the class isn't enclosed
/// directly by a class body (initializer) instead — kept as its raw
/// decoded `(name, descriptor)` pair rather than parsed/resolved
/// further: which method encloses this class is identifying metadata,
/// not a type link worth its own semantic model.
#[derive(Debug, Clone)]
pub struct EnclosingMethodRef {
    pub class: ClassRef,
    pub method: Option<(String, String)>,
}
