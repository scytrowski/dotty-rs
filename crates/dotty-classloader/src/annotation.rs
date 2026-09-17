use crate::symbol::ClassRef;

/// A resolved annotation (JVMS §4.7.16), attached to a class, field, or
/// method (`docs/classloader.md` §9, Milestone 7).
///
/// `annotation_type` is the annotation interface itself, resolved the
/// same tolerant way `docs/classloader.md` §9's Milestone 6 resolves a
/// member's declared type — it's the load-bearing "this annotation is
/// an instance of this type" reference, parallel to a field's declared
/// type. `elements` keeps insertion order from the class file, as a
/// list of `(element name, value)` pairs rather than a map: JVMS
/// doesn't require unique names, and preserving source order matters
/// more than lookup speed here.
///
/// Both `RuntimeVisibleAnnotations` and `RuntimeInvisibleAnnotations`
/// flatten into this one list on their owner — retention visibility is
/// JVM-reflection-facing metadata, not something this crate's semantic
/// model needs to distinguish.
#[derive(Debug, Clone)]
pub struct SemanticAnnotation {
    pub annotation_type: ClassRef,
    pub elements: Vec<(String, AnnotationValue)>,
}

/// One annotation element's value (JVMS §4.7.16.1), resolved from a
/// `dotty_classfile::attribute::ElementValue`.
///
/// Unlike [`SemanticAnnotation::annotation_type`], class names
/// mentioned *inside* a value are kept raw, not resolved — the same
/// "keep nested names verbatim" choice `docs/classloader.md` §9's
/// Milestone 5 already made for a parsed `Signature` tree's type
/// variable/class names. `Enum::type_descriptor` and `Class`'s payload
/// stay as their raw decoded descriptor strings.
#[derive(Debug, Clone)]
pub enum AnnotationValue {
    Byte(i32),
    Char(i32),
    Double(f64),
    Float(f32),
    Int(i32),
    Long(i64),
    Short(i32),
    Boolean(bool),
    String(String),
    Enum {
        type_descriptor: String,
        const_name: String,
    },
    Class(String),
    Annotation(Box<SemanticAnnotation>),
    Array(Vec<AnnotationValue>),
}
