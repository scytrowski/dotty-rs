use crate::descriptor::{Cursor, FieldType, peek_base_type};
use std::fmt;

/// An implementation-defined recursion guard for the signature grammar's
/// mutual recursion (`ReferenceTypeSignature` -> `ClassTypeSignature`/
/// `TypeArgument` -> `ReferenceTypeSignature`, and via
/// `ArrayTypeSignature`) — unlike [`crate::descriptor::MAX_ARRAY_DIMENSIONS`]
/// this isn't a JVMS-mandated limit, just a stack-safety bound (mirrors
/// `dotty_tasty::ast::DEFAULT_MAX_AST_INDEX_DEPTH`).
pub const DEFAULT_MAX_SIGNATURE_DEPTH: usize = 128;

/// Stop characters for a signature `Identifier` (JVMS §4.7.9.1): the
/// unqualified-name-forbidden characters `. ; [ /` plus `< > :`, which are
/// themselves signature grammar punctuation.
const IDENTIFIER_STOPS: &[u8] = b".;[/<>:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureError {
    UnexpectedEnd { offset: usize },
    UnknownLeadChar { offset: usize, found: char },
    EmptyIdentifier { offset: usize },
    MissingSemicolon { offset: usize },
    MissingColon { offset: usize },
    TooDeeplyNested { offset: usize },
    TrailingCharacters { offset: usize },
}

impl fmt::Display for SignatureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEnd { offset } => {
                write!(formatter, "unexpected end of signature at offset {offset}")
            }
            Self::UnknownLeadChar { offset, found } => {
                write!(
                    formatter,
                    "unknown lead character {found:?} at offset {offset}"
                )
            }
            Self::EmptyIdentifier { offset } => {
                write!(formatter, "empty identifier at offset {offset}")
            }
            Self::MissingSemicolon { offset } => {
                write!(formatter, "expected ';' at offset {offset}")
            }
            Self::MissingColon { offset } => {
                write!(formatter, "expected ':' at offset {offset}")
            }
            Self::TooDeeplyNested { offset } => write!(
                formatter,
                "signature nested past {DEFAULT_MAX_SIGNATURE_DEPTH} levels at offset {offset}"
            ),
            Self::TrailingCharacters { offset } => {
                write!(formatter, "trailing characters at offset {offset}")
            }
        }
    }
}

impl std::error::Error for SignatureError {}

fn read_identifier<'a>(cursor: &mut Cursor<'a>) -> Result<&'a str, SignatureError> {
    let offset = cursor.offset();
    cursor
        .take_identifier(IDENTIFIER_STOPS)
        .ok_or(SignatureError::EmptyIdentifier { offset })
}

fn parse_type_signature(
    cursor: &mut Cursor<'_>,
    depth: usize,
) -> Result<TypeSignature, SignatureError> {
    if let Some(base) = peek_base_type(cursor) {
        cursor.bump_ascii();
        return Ok(TypeSignature::Base(base));
    }
    Ok(TypeSignature::Reference(parse_reference_type_signature(
        cursor, depth,
    )?))
}

fn parse_reference_type_signature(
    cursor: &mut Cursor<'_>,
    depth: usize,
) -> Result<ReferenceTypeSignature, SignatureError> {
    if depth > DEFAULT_MAX_SIGNATURE_DEPTH {
        return Err(SignatureError::TooDeeplyNested {
            offset: cursor.offset(),
        });
    }

    match cursor.peek_ascii() {
        Some(b'L') => Ok(ReferenceTypeSignature::Class(parse_class_type_signature(
            cursor, depth,
        )?)),
        Some(b'T') => Ok(ReferenceTypeSignature::TypeVariable(parse_type_variable(
            cursor,
        )?)),
        Some(b'[') => {
            cursor.bump_ascii();
            let component = parse_type_signature(cursor, depth + 1)?;
            Ok(ReferenceTypeSignature::Array(Box::new(component)))
        }
        Some(found) => Err(SignatureError::UnknownLeadChar {
            offset: cursor.offset(),
            found: found as char,
        }),
        None => Err(SignatureError::UnexpectedEnd {
            offset: cursor.offset(),
        }),
    }
}

fn parse_type_variable(cursor: &mut Cursor<'_>) -> Result<String, SignatureError> {
    let offset = cursor.offset();
    if !cursor.eat_ascii(b'T') {
        return Err(SignatureError::UnknownLeadChar {
            offset,
            found: cursor.peek_ascii().map_or('\0', |byte| byte as char),
        });
    }
    let name = read_identifier(cursor)?.to_owned();
    if !cursor.eat_ascii(b';') {
        return Err(SignatureError::MissingSemicolon {
            offset: cursor.offset(),
        });
    }
    Ok(name)
}

fn parse_class_type_signature(
    cursor: &mut Cursor<'_>,
    depth: usize,
) -> Result<ClassTypeSignature, SignatureError> {
    let start_offset = cursor.offset();
    if !cursor.eat_ascii(b'L') {
        return Err(SignatureError::UnknownLeadChar {
            offset: start_offset,
            found: cursor.peek_ascii().map_or('\0', |byte| byte as char),
        });
    }

    let mut package = Vec::new();
    let mut current_name = read_identifier(cursor)?.to_owned();
    while cursor.eat_ascii(b'/') {
        package.push(current_name);
        current_name = read_identifier(cursor)?.to_owned();
    }
    let simple_name = current_name;
    let type_arguments = parse_optional_type_arguments(cursor, depth)?;

    let mut suffix = Vec::new();
    while cursor.eat_ascii(b'.') {
        let name = read_identifier(cursor)?.to_owned();
        let type_arguments = parse_optional_type_arguments(cursor, depth)?;
        suffix.push(SimpleClassTypeSignature {
            name,
            type_arguments,
        });
    }

    if !cursor.eat_ascii(b';') {
        return Err(SignatureError::MissingSemicolon {
            offset: cursor.offset(),
        });
    }

    Ok(ClassTypeSignature {
        package,
        simple_name,
        type_arguments,
        suffix,
    })
}

fn parse_optional_type_arguments(
    cursor: &mut Cursor<'_>,
    depth: usize,
) -> Result<Vec<TypeArgument>, SignatureError> {
    if !cursor.eat_ascii(b'<') {
        return Ok(Vec::new());
    }

    let mut arguments = Vec::new();
    while !cursor.eat_ascii(b'>') {
        arguments.push(parse_type_argument(cursor, depth + 1)?);
    }
    Ok(arguments)
}

fn parse_type_argument(
    cursor: &mut Cursor<'_>,
    depth: usize,
) -> Result<TypeArgument, SignatureError> {
    match cursor.peek_ascii() {
        Some(b'*') => {
            cursor.bump_ascii();
            Ok(TypeArgument::Wildcard)
        }
        Some(b'+') => {
            cursor.bump_ascii();
            Ok(TypeArgument::Extends(parse_reference_type_signature(
                cursor, depth,
            )?))
        }
        Some(b'-') => {
            cursor.bump_ascii();
            Ok(TypeArgument::Super(parse_reference_type_signature(
                cursor, depth,
            )?))
        }
        Some(_) => Ok(TypeArgument::Exact(parse_reference_type_signature(
            cursor, depth,
        )?)),
        None => Err(SignatureError::UnexpectedEnd {
            offset: cursor.offset(),
        }),
    }
}

fn parse_type_parameters(
    cursor: &mut Cursor<'_>,
    depth: usize,
) -> Result<Vec<TypeParameter>, SignatureError> {
    if !cursor.eat_ascii(b'<') {
        return Ok(Vec::new());
    }

    let mut parameters = Vec::new();
    while !cursor.eat_ascii(b'>') {
        parameters.push(parse_type_parameter(cursor, depth)?);
    }
    Ok(parameters)
}

fn parse_type_parameter(
    cursor: &mut Cursor<'_>,
    depth: usize,
) -> Result<TypeParameter, SignatureError> {
    let name = read_identifier(cursor)?.to_owned();

    let colon_offset = cursor.offset();
    if !cursor.eat_ascii(b':') {
        return Err(SignatureError::MissingColon {
            offset: colon_offset,
        });
    }

    // ClassBound's ReferenceTypeSignature is optional: a second ':'
    // immediately following means the bound was empty (implicit Object),
    // and what follows is actually the first InterfaceBound.
    let class_bound = if cursor.peek_ascii() == Some(b':') {
        None
    } else {
        Some(parse_reference_type_signature(cursor, depth + 1)?)
    };

    let mut interface_bounds = Vec::new();
    while cursor.eat_ascii(b':') {
        interface_bounds.push(parse_reference_type_signature(cursor, depth + 1)?);
    }

    Ok(TypeParameter {
        name,
        class_bound,
        interface_bounds,
    })
}

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

impl ClassSignature {
    /// Parses a complete `ClassSignature` (JVMS §4.7.9.1): optional
    /// `TypeParameters`, one `SuperclassSignature`, then
    /// `SuperinterfaceSignature*` until the input is exhausted.
    pub fn parse(input: &str) -> Result<Self, SignatureError> {
        let mut cursor = Cursor::new(input);
        let type_parameters = parse_type_parameters(&mut cursor, 0)?;
        let superclass = parse_class_type_signature(&mut cursor, 0)?;

        let mut superinterfaces = Vec::new();
        while !cursor.is_empty() {
            superinterfaces.push(parse_class_type_signature(&mut cursor, 0)?);
        }

        Ok(Self {
            type_parameters,
            superclass,
            superinterfaces,
        })
    }
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

impl FieldSignature {
    /// Parses a complete `FieldSignature` (JVMS §4.7.9.1): a
    /// `ReferenceTypeSignature`, nothing more.
    pub fn parse(input: &str) -> Result<Self, SignatureError> {
        let mut cursor = Cursor::new(input);
        let reference = parse_reference_type_signature(&mut cursor, 0)?;

        if !cursor.is_empty() {
            return Err(SignatureError::TrailingCharacters {
                offset: cursor.offset(),
            });
        }

        Ok(Self(reference))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plain_class_type() {
        assert_eq!(
            FieldSignature::parse("Ljava/lang/String;"),
            Ok(FieldSignature(ReferenceTypeSignature::Class(
                ClassTypeSignature {
                    package: vec!["java".to_owned(), "lang".to_owned()],
                    simple_name: "String".to_owned(),
                    type_arguments: vec![],
                    suffix: vec![],
                }
            )))
        );
    }

    #[test]
    fn parses_an_unqualified_class_type() {
        assert_eq!(
            FieldSignature::parse("LFoo;"),
            Ok(FieldSignature(ReferenceTypeSignature::Class(
                ClassTypeSignature {
                    package: vec![],
                    simple_name: "Foo".to_owned(),
                    type_arguments: vec![],
                    suffix: vec![],
                }
            )))
        );
    }

    #[test]
    fn parses_a_class_type_with_one_type_argument() {
        assert_eq!(
            FieldSignature::parse("Ljava/util/List<Ljava/lang/String;>;"),
            Ok(FieldSignature(ReferenceTypeSignature::Class(
                ClassTypeSignature {
                    package: vec!["java".to_owned(), "util".to_owned()],
                    simple_name: "List".to_owned(),
                    type_arguments: vec![TypeArgument::Exact(ReferenceTypeSignature::Class(
                        ClassTypeSignature {
                            package: vec!["java".to_owned(), "lang".to_owned()],
                            simple_name: "String".to_owned(),
                            type_arguments: vec![],
                            suffix: vec![],
                        }
                    ))],
                    suffix: vec![],
                }
            )))
        );
    }

    #[test]
    fn parses_all_three_wildcard_forms() {
        let number = ReferenceTypeSignature::Class(ClassTypeSignature {
            package: vec!["java".to_owned(), "lang".to_owned()],
            simple_name: "Number".to_owned(),
            type_arguments: vec![],
            suffix: vec![],
        });

        let unbounded = FieldSignature::parse("Ljava/util/List<*>;").unwrap();
        assert_eq!(
            unbounded.0,
            ReferenceTypeSignature::Class(ClassTypeSignature {
                package: vec!["java".to_owned(), "util".to_owned()],
                simple_name: "List".to_owned(),
                type_arguments: vec![TypeArgument::Wildcard],
                suffix: vec![],
            })
        );

        let extends = FieldSignature::parse("Ljava/util/List<+Ljava/lang/Number;>;").unwrap();
        assert_eq!(
            extends.0,
            ReferenceTypeSignature::Class(ClassTypeSignature {
                package: vec!["java".to_owned(), "util".to_owned()],
                simple_name: "List".to_owned(),
                type_arguments: vec![TypeArgument::Extends(number.clone())],
                suffix: vec![],
            })
        );

        let super_bound = FieldSignature::parse("Ljava/util/List<-Ljava/lang/Number;>;").unwrap();
        assert_eq!(
            super_bound.0,
            ReferenceTypeSignature::Class(ClassTypeSignature {
                package: vec!["java".to_owned(), "util".to_owned()],
                simple_name: "List".to_owned(),
                type_arguments: vec![TypeArgument::Super(number)],
                suffix: vec![],
            })
        );
    }

    #[test]
    fn parses_an_inner_class_qualified_suffix() {
        assert_eq!(
            FieldSignature::parse("LOuter<TT;>.Inner<TU;>;"),
            Ok(FieldSignature(ReferenceTypeSignature::Class(
                ClassTypeSignature {
                    package: vec![],
                    simple_name: "Outer".to_owned(),
                    type_arguments: vec![TypeArgument::Exact(
                        ReferenceTypeSignature::TypeVariable("T".to_owned())
                    )],
                    suffix: vec![SimpleClassTypeSignature {
                        name: "Inner".to_owned(),
                        type_arguments: vec![TypeArgument::Exact(
                            ReferenceTypeSignature::TypeVariable("U".to_owned())
                        )],
                    }],
                }
            )))
        );
    }

    #[test]
    fn parses_a_type_variable() {
        assert_eq!(
            FieldSignature::parse("TT;"),
            Ok(FieldSignature(ReferenceTypeSignature::TypeVariable(
                "T".to_owned()
            )))
        );
    }

    #[test]
    fn parses_an_array_of_a_class_type() {
        assert_eq!(
            FieldSignature::parse("[Ljava/lang/String;"),
            Ok(FieldSignature(ReferenceTypeSignature::Array(Box::new(
                TypeSignature::Reference(ReferenceTypeSignature::Class(ClassTypeSignature {
                    package: vec!["java".to_owned(), "lang".to_owned()],
                    simple_name: "String".to_owned(),
                    type_arguments: vec![],
                    suffix: vec![],
                }))
            ))))
        );
    }

    #[test]
    fn parses_a_nested_array_of_a_base_type() {
        assert_eq!(
            FieldSignature::parse("[[I"),
            Ok(FieldSignature(ReferenceTypeSignature::Array(Box::new(
                TypeSignature::Reference(ReferenceTypeSignature::Array(Box::new(
                    TypeSignature::Base(FieldType::Int)
                )))
            ))))
        );
    }

    #[test]
    fn rejects_signatures_nested_past_the_depth_guard() {
        let too_deep = "[".repeat(DEFAULT_MAX_SIGNATURE_DEPTH + 2) + "I";

        assert!(matches!(
            FieldSignature::parse(&too_deep),
            Err(SignatureError::TooDeeplyNested { .. })
        ));
    }

    #[test]
    fn rejects_a_class_type_missing_its_semicolon() {
        assert_eq!(
            FieldSignature::parse("Ljava/lang/String"),
            Err(SignatureError::MissingSemicolon { offset: 17 })
        );
    }

    #[test]
    fn rejects_an_empty_identifier() {
        assert_eq!(
            FieldSignature::parse("L;"),
            Err(SignatureError::EmptyIdentifier { offset: 1 })
        );
    }

    #[test]
    fn rejects_an_unknown_lead_character() {
        assert_eq!(
            FieldSignature::parse("X"),
            Err(SignatureError::UnknownLeadChar {
                offset: 0,
                found: 'X'
            })
        );
    }

    #[test]
    fn rejects_empty_input() {
        assert_eq!(
            FieldSignature::parse(""),
            Err(SignatureError::UnexpectedEnd { offset: 0 })
        );
    }

    fn object_class_type() -> ClassTypeSignature {
        ClassTypeSignature {
            package: vec!["java".to_owned(), "lang".to_owned()],
            simple_name: "Object".to_owned(),
            type_arguments: vec![],
            suffix: vec![],
        }
    }

    #[test]
    fn parses_a_type_parameter_with_an_explicit_class_bound() {
        let signature = ClassSignature::parse("<T:Ljava/lang/Object;>Ljava/lang/Object;").unwrap();

        assert_eq!(
            signature.type_parameters,
            vec![TypeParameter {
                name: "T".to_owned(),
                class_bound: Some(ReferenceTypeSignature::Class(object_class_type())),
                interface_bounds: vec![],
            }]
        );
    }

    #[test]
    fn parses_a_type_parameter_with_an_empty_class_bound_and_one_interface_bound() {
        let signature =
            ClassSignature::parse("<T::Ljava/lang/Comparable<TT;>;>Ljava/lang/Object;").unwrap();

        assert_eq!(
            signature.type_parameters,
            vec![TypeParameter {
                name: "T".to_owned(),
                class_bound: None,
                interface_bounds: vec![ReferenceTypeSignature::Class(ClassTypeSignature {
                    package: vec!["java".to_owned(), "lang".to_owned()],
                    simple_name: "Comparable".to_owned(),
                    type_arguments: vec![TypeArgument::Exact(
                        ReferenceTypeSignature::TypeVariable("T".to_owned())
                    )],
                    suffix: vec![],
                })],
            }]
        );
    }

    #[test]
    fn parses_a_type_parameter_with_multiple_interface_bounds() {
        let signature = ClassSignature::parse(
            "<T:Ljava/lang/Object;:Ljava/lang/Comparable<TT;>;:Ljava/io/Serializable;>Ljava/lang/Object;",
        )
        .unwrap();

        let type_parameter = &signature.type_parameters[0];
        assert_eq!(
            type_parameter.class_bound,
            Some(ReferenceTypeSignature::Class(object_class_type()))
        );
        assert_eq!(type_parameter.interface_bounds.len(), 2);
    }

    #[test]
    fn parses_a_class_signature_with_no_type_parameters_and_one_superinterface() {
        assert_eq!(
            ClassSignature::parse("Ljava/lang/Object;Ljava/lang/Runnable;"),
            Ok(ClassSignature {
                type_parameters: vec![],
                superclass: object_class_type(),
                superinterfaces: vec![ClassTypeSignature {
                    package: vec!["java".to_owned(), "lang".to_owned()],
                    simple_name: "Runnable".to_owned(),
                    type_arguments: vec![],
                    suffix: vec![],
                }],
            })
        );
    }

    #[test]
    fn parses_a_class_signature_with_type_parameters_and_multiple_superinterfaces() {
        let signature = ClassSignature::parse(
            "<T:Ljava/lang/Object;>Ljava/lang/Object;Ljava/lang/Runnable;Ljava/io/Serializable;",
        )
        .unwrap();

        assert_eq!(signature.type_parameters.len(), 1);
        assert_eq!(signature.superclass, object_class_type());
        assert_eq!(signature.superinterfaces.len(), 2);
    }
}
