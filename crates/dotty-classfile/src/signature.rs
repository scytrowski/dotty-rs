use crate::descriptor::FieldType;

/// A `JavaTypeSignature` (JVMS §4.7.9.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeSignature {
    Reference(ReferenceTypeSignature),
    Base(FieldType),
}

/// A `ReferenceTypeSignature` (JVMS §4.7.9.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferenceTypeSignature {
    Class(ClassTypeSignature),
    TypeVariable(String),
    Array(Box<TypeSignature>),
}

/// A `ClassTypeSignature` (JVMS §4.7.9.1): `L PackageSpecifier?
/// SimpleClassTypeSignature ClassTypeSignatureSuffix* ;`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassTypeSignature {
    pub package: Vec<String>,
    pub simple_name: String,
    pub type_arguments: Vec<TypeArgument>,
    /// The `.` -qualified inner-class suffixes (`ClassTypeSignatureSuffix*`).
    pub suffix: Vec<SimpleClassTypeSignature>,
}

/// A `SimpleClassTypeSignature` (JVMS §4.7.9.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleClassTypeSignature {
    pub name: String,
    pub type_arguments: Vec<TypeArgument>,
}

/// A `TypeArgument` (JVMS §4.7.9.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeArgument {
    Exact(ReferenceTypeSignature),
    Extends(ReferenceTypeSignature),
    Super(ReferenceTypeSignature),
    Wildcard,
}

/// A `TypeParameter` (JVMS §4.7.9.1): `Identifier ClassBound InterfaceBound*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeParameter {
    pub name: String,
    /// The `ClassBound`; `None` means the bound is implicitly `Object`.
    pub class_bound: Option<ReferenceTypeSignature>,
    pub interface_bounds: Vec<ReferenceTypeSignature>,
}

/// A `ClassSignature` (JVMS §4.7.9.1), carried by the `Signature` attribute
/// on a class or interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassSignature {
    pub type_parameters: Vec<TypeParameter>,
    pub superclass: ClassTypeSignature,
    pub superinterfaces: Vec<ClassTypeSignature>,
}

/// A `MethodSignature` (JVMS §4.7.9.1), carried by the `Signature` attribute
/// on a method or constructor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodSignature {
    pub type_parameters: Vec<TypeParameter>,
    pub parameters: Vec<TypeSignature>,
    /// The `Result`; `None` means `void`.
    pub result: Option<TypeSignature>,
    pub throws: Vec<ThrowsSignature>,
}

/// A `ThrowsSignature` (JVMS §4.7.9.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThrowsSignature {
    Class(ClassTypeSignature),
    TypeVariable(String),
}

/// A `FieldSignature` (JVMS §4.7.9.1), carried by the `Signature` attribute
/// on a field or record component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSignature(pub ReferenceTypeSignature);
