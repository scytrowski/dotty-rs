/// A parsed field descriptor (JVMS §4.3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldType {
    Byte,
    Char,
    Double,
    Float,
    Int,
    Long,
    Short,
    Boolean,
    /// Reference type; the class name is in internal form (e.g. `java/lang/Object`).
    Object(String),
    Array(Box<FieldType>),
}

/// A parsed method descriptor (JVMS §4.3.3). `return_type: None` means `void`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodDescriptor {
    pub parameters: Vec<FieldType>,
    pub return_type: Option<FieldType>,
}
