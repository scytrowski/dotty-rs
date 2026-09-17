use crate::symbol::ClassRef;

/// A field or method member type, resolved to the classes it refers to
/// (`docs/classloader.md` §9, Milestone 6).
///
/// Mirrors `dotty_classfile::descriptor::FieldType` exactly, except
/// `Object` holds a resolved [`ClassRef`] instead of a raw internal-form
/// name string. Added *alongside* the existing raw
/// `FieldSymbol::field_type`/`MethodSymbol::descriptor` accessors, not
/// replacing them — the same "attach without replacing" approach
/// `docs/classloader.md` §9's Milestone 5 already used for generic
/// signatures.
#[derive(Debug, Clone)]
pub enum SemanticFieldType {
    Byte,
    Char,
    Double,
    Float,
    Int,
    Long,
    Short,
    Boolean,
    Object(ClassRef),
    Array(Box<SemanticFieldType>),
}
