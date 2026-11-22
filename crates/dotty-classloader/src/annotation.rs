use crate::symbol::ClassRef;

/// A resolved annotation (JVMS §4.7.16), attached to a class, field, or
/// method (`docs/classloader.md` §9, Milestone 7).
///
/// `annotation_type` is the annotation interface itself. Unlike a field's
/// declared type, an annotation's type is *not* load-bearing: real-world
/// bytecode routinely carries annotations (build/processor-only ones in
/// particular) whose interface is absent from the runtime classpath, and
/// that must not fail loading the annotated class itself — so
/// `ClassLoader::resolve_annotation` resolves it best-effort and falls
/// back to `ClassRef::Unresolved` rather than propagating a
/// `ClassLoadError`. When it does resolve, the annotation also gets a real
/// `dotty_core::Annotation`/`AnnotationId` entered onto the annotated
/// symbol's own `Symbol::annotations` — see
/// `ClassLoader::enter_annotations`. `elements` keeps insertion order from
/// the class file, as a list of `(element name, value)` pairs rather than
/// a map: JVMS doesn't require unique names, and preserving source order
/// matters more than lookup speed here. The element values themselves
/// have no home in `dotty_core::Annotation` (its `tree` is a typed source
/// AST, which classfile-decoded values aren't), so they stay exclusively
/// in this JVM-facing sidecar shape.
///
/// Both `RuntimeVisibleAnnotations` and `RuntimeInvisibleAnnotations`
/// flatten into this one list on their owner — retention visibility is
/// JVM-reflection-facing metadata, not something this crate's semantic
/// model needs to distinguish.
#[derive(Debug, Clone)]
pub struct SemanticAnnotation {
    /// Class or type identity of the annotation.
    pub annotation_type: ClassRef,
    /// Elements in their original source or annotation order.
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
    /// Annotation signed 8-bit integer value.
    Byte(i32),
    /// Annotation UTF-16 code-unit character value.
    Char(i32),
    /// Annotation double-precision floating-point value.
    Double(f64),
    /// Annotation single-precision floating-point value.
    Float(f32),
    /// Annotation signed 32-bit integer value.
    Int(i32),
    /// Annotation signed 64-bit integer value.
    Long(i64),
    /// Annotation signed 16-bit integer value.
    Short(i32),
    /// Annotation Boolean value.
    Boolean(bool),
    /// Decoded string annotation value.
    String(String),
    /// Enumeration constant identified by its type and constant name.
    Enum {
        /// JVM descriptor of the annotation type or class literal.
        type_descriptor: String,
        /// Name of the selected enumeration constant.
        const_name: String,
    },
    /// Class literal represented by its JVM descriptor.
    Class(String),
    /// Nested annotation value.
    Annotation(Box<SemanticAnnotation>),
    /// Ordered array of annotation element values.
    Array(Vec<AnnotationValue>),
}
