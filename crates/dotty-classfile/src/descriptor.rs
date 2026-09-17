use crate::constant_pool::{ConstantPool, ConstantPoolEntry, ConstantPoolIndex};
use std::fmt;

/// A hard JVMS constraint (§4.3.2): an array type has at most 255
/// dimensions.
pub const MAX_ARRAY_DIMENSIONS: usize = 255;

/// A cursor over a descriptor/signature string (JVMS §4.3, §4.7.9.1 are
/// both plain UTF-8 text, already decoded from a `Utf8` constant pool
/// entry — this is not the binary [`crate::reader::Reader`]). Every
/// operation only shrinks `remaining` from the front and only splits on
/// single-byte ASCII needles, so slicing is always on a valid UTF-8 char
/// boundary regardless of what adversarial input contains.
pub(crate) struct Cursor<'a> {
    original_len: usize,
    remaining: &'a str,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(input: &'a str) -> Self {
        Self {
            original_len: input.len(),
            remaining: input,
        }
    }

    pub(crate) fn offset(&self) -> usize {
        self.original_len - self.remaining.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.remaining.is_empty()
    }

    pub(crate) fn peek_ascii(&self) -> Option<u8> {
        match self.remaining.as_bytes().first() {
            Some(&byte) if byte.is_ascii() => Some(byte),
            _ => None,
        }
    }

    pub(crate) fn bump_ascii(&mut self) -> Option<u8> {
        let byte = self.peek_ascii()?;
        self.remaining = &self.remaining[1..];
        Some(byte)
    }

    pub(crate) fn eat_ascii(&mut self, expected: u8) -> bool {
        if self.peek_ascii() == Some(expected) {
            self.remaining = &self.remaining[1..];
            true
        } else {
            false
        }
    }

    /// Returns everything before the first occurrence of `stop` and
    /// advances past `stop` itself; `None` if `stop` never appears.
    pub(crate) fn take_until_ascii(&mut self, stop: u8) -> Option<&'a str> {
        let position = self.remaining.as_bytes().iter().position(|&b| b == stop)?;
        let (taken, rest) = self.remaining.split_at(position);
        self.remaining = &rest[1..];
        Some(taken)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorError {
    Empty,
    UnknownTypeChar { offset: usize, found: char },
    UnterminatedClassName { offset: usize },
    TooManyArrayDimensions { offset: usize },
    TrailingCharacters { offset: usize },
    MissingOpenParen { offset: usize },
    MissingCloseParen { offset: usize },
}

impl fmt::Display for DescriptorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "empty descriptor"),
            Self::UnknownTypeChar { offset, found } => {
                write!(
                    formatter,
                    "unknown type character {found:?} at offset {offset}"
                )
            }
            Self::UnterminatedClassName { offset } => {
                write!(formatter, "unterminated class name at offset {offset}")
            }
            Self::TooManyArrayDimensions { offset } => write!(
                formatter,
                "more than {MAX_ARRAY_DIMENSIONS} array dimensions at offset {offset}"
            ),
            Self::TrailingCharacters { offset } => {
                write!(formatter, "trailing characters at offset {offset}")
            }
            Self::MissingOpenParen { offset } => {
                write!(formatter, "expected '(' at offset {offset}")
            }
            Self::MissingCloseParen { offset } => {
                write!(formatter, "expected ')' at offset {offset}")
            }
        }
    }
}

impl std::error::Error for DescriptorError {}

pub(crate) fn parse_field_type(
    cursor: &mut Cursor<'_>,
    depth: usize,
) -> Result<FieldType, DescriptorError> {
    let offset = cursor.offset();
    let lead = cursor.bump_ascii().ok_or(DescriptorError::Empty)?;

    match lead {
        b'B' => Ok(FieldType::Byte),
        b'C' => Ok(FieldType::Char),
        b'D' => Ok(FieldType::Double),
        b'F' => Ok(FieldType::Float),
        b'I' => Ok(FieldType::Int),
        b'J' => Ok(FieldType::Long),
        b'S' => Ok(FieldType::Short),
        b'Z' => Ok(FieldType::Boolean),
        b'L' => {
            let name = cursor
                .take_until_ascii(b';')
                .ok_or(DescriptorError::UnterminatedClassName { offset })?;
            Ok(FieldType::Object(name.to_owned()))
        }
        b'[' => {
            if depth + 1 > MAX_ARRAY_DIMENSIONS {
                return Err(DescriptorError::TooManyArrayDimensions { offset });
            }
            let component = parse_field_type(cursor, depth + 1)?;
            Ok(FieldType::Array(Box::new(component)))
        }
        other => Err(DescriptorError::UnknownTypeChar {
            offset,
            found: other as char,
        }),
    }
}

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

impl FieldType {
    /// Parses a complete field descriptor (JVMS §4.3.2); the whole input
    /// must be consumed by exactly one `FieldType`.
    pub fn parse(input: &str) -> Result<Self, DescriptorError> {
        let mut cursor = Cursor::new(input);
        let field_type = parse_field_type(&mut cursor, 0)?;

        if !cursor.is_empty() {
            return Err(DescriptorError::TrailingCharacters {
                offset: cursor.offset(),
            });
        }

        Ok(field_type)
    }
}

impl MethodDescriptor {
    /// Parses a complete method descriptor (JVMS §4.3.3):
    /// `( ParameterDescriptor* ) ReturnDescriptor`.
    pub fn parse(input: &str) -> Result<Self, DescriptorError> {
        let mut cursor = Cursor::new(input);
        let open_paren_offset = cursor.offset();
        if !cursor.eat_ascii(b'(') {
            return Err(DescriptorError::MissingOpenParen {
                offset: open_paren_offset,
            });
        }

        let mut parameters = Vec::new();
        loop {
            match cursor.peek_ascii() {
                Some(b')') => break,
                Some(_) => parameters.push(parse_field_type(&mut cursor, 0)?),
                None => {
                    return Err(DescriptorError::MissingCloseParen {
                        offset: cursor.offset(),
                    });
                }
            }
        }
        cursor.bump_ascii();

        let return_type = if cursor.eat_ascii(b'V') {
            None
        } else {
            Some(parse_field_type(&mut cursor, 0)?)
        };

        if !cursor.is_empty() {
            return Err(DescriptorError::TrailingCharacters {
                offset: cursor.offset(),
            });
        }

        Ok(Self {
            parameters,
            return_type,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveDescriptorError {
    NotUtf8 { index: ConstantPoolIndex },
    Parse(DescriptorError),
}

impl fmt::Display for ResolveDescriptorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotUtf8 { index } => write!(
                formatter,
                "descriptor index {} does not resolve to a Utf8 entry",
                index.0
            ),
            Self::Parse(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ResolveDescriptorError {}

impl From<DescriptorError> for ResolveDescriptorError {
    fn from(error: DescriptorError) -> Self {
        Self::Parse(error)
    }
}

/// Resolves `index` through `constant_pool` to a `Utf8` entry and parses it
/// as a field descriptor.
pub fn resolve_field_type(
    constant_pool: &ConstantPool,
    index: ConstantPoolIndex,
) -> Result<FieldType, ResolveDescriptorError> {
    match constant_pool.get(index) {
        Some(ConstantPoolEntry::Utf8(text)) => Ok(FieldType::parse(text)?),
        _ => Err(ResolveDescriptorError::NotUtf8 { index }),
    }
}

/// Resolves `index` through `constant_pool` to a `Utf8` entry and parses it
/// as a method descriptor.
pub fn resolve_method_descriptor(
    constant_pool: &ConstantPool,
    index: ConstantPoolIndex,
) -> Result<MethodDescriptor, ResolveDescriptorError> {
    match constant_pool.get(index) {
        Some(ConstantPoolEntry::Utf8(text)) => Ok(MethodDescriptor::parse(text)?),
        _ => Err(ResolveDescriptorError::NotUtf8 { index }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_base_type() {
        let cases = [
            ("B", FieldType::Byte),
            ("C", FieldType::Char),
            ("D", FieldType::Double),
            ("F", FieldType::Float),
            ("I", FieldType::Int),
            ("J", FieldType::Long),
            ("S", FieldType::Short),
            ("Z", FieldType::Boolean),
        ];

        for (input, expected) in cases {
            assert_eq!(
                FieldType::parse(input),
                Ok(expected.clone()),
                "failed for {input:?}"
            );
        }
    }

    #[test]
    fn parses_an_object_type() {
        assert_eq!(
            FieldType::parse("Ljava/lang/String;"),
            Ok(FieldType::Object("java/lang/String".to_owned()))
        );
    }

    #[test]
    fn rejects_an_unterminated_object_type() {
        assert_eq!(
            FieldType::parse("Ljava/lang/String"),
            Err(DescriptorError::UnterminatedClassName { offset: 0 })
        );
    }

    #[test]
    fn parses_array_types_at_increasing_depth() {
        assert_eq!(
            FieldType::parse("[I"),
            Ok(FieldType::Array(Box::new(FieldType::Int)))
        );
        assert_eq!(
            FieldType::parse("[[I"),
            Ok(FieldType::Array(Box::new(FieldType::Array(Box::new(
                FieldType::Int
            )))))
        );
    }

    #[test]
    fn parses_an_array_type_at_the_maximum_dimension() {
        let descriptor = "[".repeat(MAX_ARRAY_DIMENSIONS) + "I";

        assert!(FieldType::parse(&descriptor).is_ok());
    }

    #[test]
    fn rejects_an_array_type_beyond_the_maximum_dimension() {
        let descriptor = "[".repeat(MAX_ARRAY_DIMENSIONS + 1) + "I";

        assert_eq!(
            FieldType::parse(&descriptor),
            Err(DescriptorError::TooManyArrayDimensions {
                offset: MAX_ARRAY_DIMENSIONS
            })
        );
    }

    #[test]
    fn rejects_an_empty_descriptor() {
        assert_eq!(FieldType::parse(""), Err(DescriptorError::Empty));
    }

    #[test]
    fn rejects_an_unknown_lead_character() {
        assert_eq!(
            FieldType::parse("X"),
            Err(DescriptorError::UnknownTypeChar {
                offset: 0,
                found: 'X'
            })
        );
    }

    #[test]
    fn rejects_trailing_characters_after_a_complete_field_type() {
        assert_eq!(
            FieldType::parse("II"),
            Err(DescriptorError::TrailingCharacters { offset: 1 })
        );
    }

    #[test]
    fn parses_a_no_argument_void_method_descriptor() {
        assert_eq!(
            MethodDescriptor::parse("()V"),
            Ok(MethodDescriptor {
                parameters: vec![],
                return_type: None,
            })
        );
    }

    #[test]
    fn parses_a_method_descriptor_with_several_parameters_and_a_typed_return() {
        assert_eq!(
            MethodDescriptor::parse("(IDLjava/lang/Thread;)Ljava/lang/Object;"),
            Ok(MethodDescriptor {
                parameters: vec![
                    FieldType::Int,
                    FieldType::Double,
                    FieldType::Object("java/lang/Thread".to_owned()),
                ],
                return_type: Some(FieldType::Object("java/lang/Object".to_owned())),
            })
        );
    }

    #[test]
    fn rejects_a_method_descriptor_missing_the_open_paren() {
        assert_eq!(
            MethodDescriptor::parse("I)V"),
            Err(DescriptorError::MissingOpenParen { offset: 0 })
        );
    }

    #[test]
    fn rejects_a_method_descriptor_missing_the_close_paren() {
        assert_eq!(
            MethodDescriptor::parse("(I"),
            Err(DescriptorError::MissingCloseParen { offset: 2 })
        );
    }

    #[test]
    fn rejects_a_method_descriptor_with_a_bad_parameter_type() {
        assert_eq!(
            MethodDescriptor::parse("(X)V"),
            Err(DescriptorError::UnknownTypeChar {
                offset: 1,
                found: 'X'
            })
        );
    }

    #[test]
    fn rejects_trailing_characters_after_a_complete_method_descriptor() {
        assert_eq!(
            MethodDescriptor::parse("()VV"),
            Err(DescriptorError::TrailingCharacters { offset: 3 })
        );
    }
}
