//! Append-only construction of new JVM constant pools.
//!
//! The builder interns the basic constant families used by generated class
//! files and assigns each distinct value its index on first insertion. It
//! does not construct method handles, method types, dynamic constants,
//! invokedynamic entries, or bootstrap-method tables.

use crate::constant_pool::{ConstantPool, ConstantPoolEntry, ConstantPoolIndex};
use crate::encode_error::EncodeError;
use std::collections::HashMap;
use std::fmt;

const MAX_CONSTANT_POOL_COUNT: usize = u16::MAX as usize;

/// A typed failure while adding or finalizing a constant pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstantPoolBuildError {
    /// The requested entry and its prerequisites would exceed the `u2`
    /// `constant_pool_count` limit, which includes slot zero.
    CountOverflow { count: usize, max: u16 },
    /// A UTF8 value cannot be represented by the JVM modified UTF-8 format.
    InvalidUtf8(EncodeError),
    /// An internally constructed pool failed the existing encoder's
    /// structural validation.
    InvalidPool(EncodeError),
}

impl fmt::Display for ConstantPoolBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CountOverflow { count, max } => write!(
                formatter,
                "constant_pool_count {count} exceeds the JVM limit {max}"
            ),
            Self::InvalidUtf8(error) => write!(formatter, "invalid constant-pool UTF8: {error}"),
            Self::InvalidPool(error) => write!(formatter, "invalid built constant pool: {error}"),
        }
    }
}

impl std::error::Error for ConstantPoolBuildError {}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ConstantKey {
    Utf8(String),
    Integer(i32),
    Float(u32),
    Long(i64),
    Double(u64),
    Class(ConstantPoolIndex),
    String(ConstantPoolIndex),
    NameAndType(ConstantPoolIndex, ConstantPoolIndex),
    Fieldref(ConstantPoolIndex, ConstantPoolIndex),
    Methodref(ConstantPoolIndex, ConstantPoolIndex),
    InterfaceMethodref(ConstantPoolIndex, ConstantPoolIndex),
}

/// An append-only, deduplicating builder for the basic JVM constant-pool
/// entries.
///
/// Indices are assigned in first-insertion order and never change. Compound
/// operations such as [`class`](Self::class) and [`method_ref`](Self::method_ref)
/// intern their prerequisite UTF8, Class, and NameAndType entries. An
/// operation that fails leaves the builder unchanged. `Long` and `Double`
/// entries reserve their required following slot.
///
/// This builder does not create method handles, method types, dynamic or
/// invokedynamic entries, or bootstrap-method tables. These remain available
/// through the structural [`ConstantPool`] model when support is added.
#[derive(Debug, Clone, Default)]
pub struct ConstantPoolBuilder {
    entries: Vec<Option<ConstantPoolEntry>>,
    indices: HashMap<ConstantKey, ConstantPoolIndex>,
    transaction_keys: Vec<ConstantKey>,
}

impl ConstantPoolBuilder {
    /// Creates an empty constant-pool builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Interns a JVM modified-UTF8 string.
    pub fn utf8(&mut self, value: &str) -> Result<ConstantPoolIndex, ConstantPoolBuildError> {
        self.transaction(|builder| builder.intern_utf8(value))
    }

    /// Interns a class by its JVM internal name, such as `java/lang/Object`.
    /// The prerequisite UTF8 entry is interned automatically.
    pub fn class(
        &mut self,
        internal_name: &str,
    ) -> Result<ConstantPoolIndex, ConstantPoolBuildError> {
        self.transaction(|builder| {
            let name_index = builder.intern_utf8(internal_name)?;
            builder.intern(
                ConstantKey::Class(name_index),
                ConstantPoolEntry::Class { name_index },
                1,
            )
        })
    }

    /// Interns a string constant and its prerequisite UTF8 entry.
    pub fn string(&mut self, value: &str) -> Result<ConstantPoolIndex, ConstantPoolBuildError> {
        self.transaction(|builder| {
            let string_index = builder.intern_utf8(value)?;
            builder.intern(
                ConstantKey::String(string_index),
                ConstantPoolEntry::String { string_index },
                1,
            )
        })
    }

    /// Interns a name and descriptor pair, with both UTF8 entries created as
    /// needed.
    pub fn name_and_type(
        &mut self,
        name: &str,
        descriptor: &str,
    ) -> Result<ConstantPoolIndex, ConstantPoolBuildError> {
        self.transaction(|builder| {
            let name_index = builder.intern_utf8(name)?;
            let descriptor_index = builder.intern_utf8(descriptor)?;
            builder.intern(
                ConstantKey::NameAndType(name_index, descriptor_index),
                ConstantPoolEntry::NameAndType {
                    name_index,
                    descriptor_index,
                },
                1,
            )
        })
    }

    /// Interns a field reference, including its class, name, and descriptor.
    pub fn field_ref(
        &mut self,
        owner: &str,
        name: &str,
        descriptor: &str,
    ) -> Result<ConstantPoolIndex, ConstantPoolBuildError> {
        self.member_ref(owner, name, descriptor, ReferenceKind::Field)
    }

    /// Interns a class method reference, including its prerequisites.
    pub fn method_ref(
        &mut self,
        owner: &str,
        name: &str,
        descriptor: &str,
    ) -> Result<ConstantPoolIndex, ConstantPoolBuildError> {
        self.member_ref(owner, name, descriptor, ReferenceKind::Method)
    }

    /// Interns an interface method reference, including its prerequisites.
    pub fn interface_method_ref(
        &mut self,
        owner: &str,
        name: &str,
        descriptor: &str,
    ) -> Result<ConstantPoolIndex, ConstantPoolBuildError> {
        self.member_ref(owner, name, descriptor, ReferenceKind::InterfaceMethod)
    }

    /// Interns an integer constant.
    pub fn integer(&mut self, value: i32) -> Result<ConstantPoolIndex, ConstantPoolBuildError> {
        self.transaction(|builder| {
            builder.intern(
                ConstantKey::Integer(value),
                ConstantPoolEntry::Integer(value),
                1,
            )
        })
    }

    /// Interns a float by its IEEE-754 bit pattern. Signed zero and distinct
    /// NaN payloads therefore remain distinct constants.
    pub fn float(&mut self, value: f32) -> Result<ConstantPoolIndex, ConstantPoolBuildError> {
        self.transaction(|builder| {
            builder.intern(
                ConstantKey::Float(value.to_bits()),
                ConstantPoolEntry::Float(value),
                1,
            )
        })
    }

    /// Interns a long constant and reserves its second constant-pool slot.
    pub fn long(&mut self, value: i64) -> Result<ConstantPoolIndex, ConstantPoolBuildError> {
        self.transaction(|builder| {
            builder.intern(ConstantKey::Long(value), ConstantPoolEntry::Long(value), 2)
        })
    }

    /// Interns a double by its IEEE-754 bit pattern and reserves its second
    /// constant-pool slot.
    pub fn double(&mut self, value: f64) -> Result<ConstantPoolIndex, ConstantPoolBuildError> {
        self.transaction(|builder| {
            builder.intern(
                ConstantKey::Double(value.to_bits()),
                ConstantPoolEntry::Double(value),
                2,
            )
        })
    }

    /// Consumes the builder and returns the checked constant pool.
    pub fn finish(self) -> Result<ConstantPool, ConstantPoolBuildError> {
        let pool = ConstantPool::from_entries(self.entries);
        pool.validate_for_encoding()
            .map_err(ConstantPoolBuildError::InvalidPool)?;
        Ok(pool)
    }

    fn member_ref(
        &mut self,
        owner: &str,
        name: &str,
        descriptor: &str,
        kind: ReferenceKind,
    ) -> Result<ConstantPoolIndex, ConstantPoolBuildError> {
        self.transaction(|builder| {
            let class_index = builder.intern_class(owner)?;
            let name_index = builder.intern_utf8(name)?;
            let descriptor_index = builder.intern_utf8(descriptor)?;
            let name_and_type_index = builder.intern(
                ConstantKey::NameAndType(name_index, descriptor_index),
                ConstantPoolEntry::NameAndType {
                    name_index,
                    descriptor_index,
                },
                1,
            )?;
            let (key, entry) = match kind {
                ReferenceKind::Field => (
                    ConstantKey::Fieldref(class_index, name_and_type_index),
                    ConstantPoolEntry::Fieldref {
                        class_index,
                        name_and_type_index,
                    },
                ),
                ReferenceKind::Method => (
                    ConstantKey::Methodref(class_index, name_and_type_index),
                    ConstantPoolEntry::Methodref {
                        class_index,
                        name_and_type_index,
                    },
                ),
                ReferenceKind::InterfaceMethod => (
                    ConstantKey::InterfaceMethodref(class_index, name_and_type_index),
                    ConstantPoolEntry::InterfaceMethodref {
                        class_index,
                        name_and_type_index,
                    },
                ),
            };
            builder.intern(key, entry, 1)
        })
    }

    fn intern_class(
        &mut self,
        internal_name: &str,
    ) -> Result<ConstantPoolIndex, ConstantPoolBuildError> {
        let name_index = self.intern_utf8(internal_name)?;
        self.intern(
            ConstantKey::Class(name_index),
            ConstantPoolEntry::Class { name_index },
            1,
        )
    }

    fn intern_utf8(&mut self, value: &str) -> Result<ConstantPoolIndex, ConstantPoolBuildError> {
        crate::modified_utf8::encode_modified_utf8(value)
            .map_err(ConstantPoolBuildError::InvalidUtf8)?;
        self.intern(
            ConstantKey::Utf8(value.to_owned()),
            ConstantPoolEntry::Utf8(value.to_owned()),
            1,
        )
    }

    fn intern(
        &mut self,
        key: ConstantKey,
        entry: ConstantPoolEntry,
        slots: usize,
    ) -> Result<ConstantPoolIndex, ConstantPoolBuildError> {
        if let Some(index) = self.indices.get(&key) {
            return Ok(*index);
        }

        let required_count = self
            .entries
            .len()
            .checked_add(slots)
            .and_then(|len| len.checked_add(1))
            .unwrap_or(usize::MAX);
        if required_count > MAX_CONSTANT_POOL_COUNT {
            return Err(ConstantPoolBuildError::CountOverflow {
                count: required_count,
                max: u16::MAX,
            });
        }
        let index = u16::try_from(self.entries.len() + 1)
            .map(ConstantPoolIndex)
            .map_err(|_| ConstantPoolBuildError::CountOverflow {
                count: required_count,
                max: u16::MAX,
            })?;

        self.entries.push(Some(entry));
        if slots == 2 {
            self.entries.push(None);
        }
        self.indices.insert(key.clone(), index);
        self.transaction_keys.push(key);
        Ok(index)
    }

    fn transaction<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, ConstantPoolBuildError>,
    ) -> Result<T, ConstantPoolBuildError> {
        let entry_checkpoint = self.entries.len();
        let key_checkpoint = self.transaction_keys.len();
        match operation(self) {
            Ok(value) => {
                self.transaction_keys.truncate(key_checkpoint);
                Ok(value)
            }
            Err(error) => {
                for key in self.transaction_keys.drain(key_checkpoint..) {
                    self.indices.remove(&key);
                }
                self.entries.truncate(entry_checkpoint);
                Err(error)
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ReferenceKind {
    Field,
    Method,
    InterfaceMethod,
}

#[cfg(test)]
mod tests {
    use super::{ConstantPoolBuildError, ConstantPoolBuilder};
    use crate::constant_pool::{ConstantPool, ConstantPoolEntry, ConstantPoolIndex};
    use crate::encode_error::EncodeError;
    use crate::reader::Reader;

    #[test]
    fn interns_prerequisites_and_keeps_reference_tags_distinct() {
        let mut builder = ConstantPoolBuilder::new();
        let utf8 = builder.utf8("example/Owner").unwrap();
        let class = builder.class("example/Owner").unwrap();
        let method = builder.method_ref("example/Owner", "run", "()V").unwrap();
        assert_eq!(builder.utf8("example/Owner"), Ok(utf8));
        assert_eq!(builder.class("example/Owner"), Ok(class));
        assert_eq!(
            builder.method_ref("example/Owner", "run", "()V"),
            Ok(method)
        );

        let field = builder.field_ref("example/Owner", "run", "()V").unwrap();
        let interface = builder
            .interface_method_ref("example/Owner", "run", "()V")
            .unwrap();
        assert_ne!(method, field);
        assert_ne!(method, interface);
        assert_ne!(field, interface);

        let pool = builder.finish().unwrap();
        assert!(matches!(
            pool.get(method),
            Some(ConstantPoolEntry::Methodref { .. })
        ));
        assert!(matches!(
            pool.get(field),
            Some(ConstantPoolEntry::Fieldref { .. })
        ));
        assert!(matches!(
            pool.get(interface),
            Some(ConstantPoolEntry::InterfaceMethodref { .. })
        ));
        assert_eq!(pool.class_name(class), Ok("example/Owner"));
        assert_eq!(pool.encode(), pool.encode());
    }

    #[test]
    fn all_supported_families_roundtrip_through_the_existing_codec() {
        let mut builder = ConstantPoolBuilder::new();
        builder.utf8("u").unwrap();
        builder.class("pkg/Type").unwrap();
        builder.string("text").unwrap();
        builder.name_and_type("field", "I").unwrap();
        builder.field_ref("pkg/Type", "field", "I").unwrap();
        builder.method_ref("pkg/Type", "run", "()V").unwrap();
        builder
            .interface_method_ref("pkg/Api", "run", "()V")
            .unwrap();
        builder.integer(-42).unwrap();
        builder.float(1.5).unwrap();
        builder.long(i64::MIN).unwrap();
        builder.double(f64::MAX).unwrap();

        let pool = builder.finish().unwrap();
        let encoded = pool.encode().unwrap();
        let mut reader = Reader::new(&encoded);
        let decoded = ConstantPool::decode(&mut reader).unwrap();
        assert!(reader.is_at_end());
        assert_eq!(decoded.encode().unwrap(), encoded);
    }

    #[test]
    fn floating_point_constants_deduplicate_by_bits() {
        let mut builder = ConstantPoolBuilder::new();
        let positive_zero = builder.float(0.0).unwrap();
        let negative_zero = builder.float(-0.0).unwrap();
        let nan_one = builder.float(f32::from_bits(0x7FC0_0001)).unwrap();
        let nan_two = builder.float(f32::from_bits(0x7FC0_0002)).unwrap();
        assert_eq!(builder.float(0.0), Ok(positive_zero));
        assert_eq!(builder.float(f32::from_bits(0x7FC0_0001)), Ok(nan_one));
        assert_ne!(positive_zero, negative_zero);
        assert_ne!(nan_one, nan_two);

        let double_zero = builder.double(0.0).unwrap();
        let double_negative_zero = builder.double(-0.0).unwrap();
        let double_nan_one = builder
            .double(f64::from_bits(0x7FF8_0000_0000_0001))
            .unwrap();
        let double_nan_two = builder
            .double(f64::from_bits(0x7FF8_0000_0000_0002))
            .unwrap();
        assert_ne!(double_zero, double_negative_zero);
        assert_ne!(double_nan_one, double_nan_two);

        let pool = builder.finish().unwrap();
        assert!(
            matches!(pool.get(negative_zero), Some(ConstantPoolEntry::Float(v)) if v.to_bits() == (-0.0f32).to_bits())
        );
        assert!(
            matches!(pool.get(nan_two), Some(ConstantPoolEntry::Float(v)) if v.to_bits() == 0x7FC0_0002)
        );
        assert!(
            matches!(pool.get(double_nan_two), Some(ConstantPoolEntry::Double(v)) if v.to_bits() == 0x7FF8_0000_0000_0002)
        );
    }

    #[test]
    fn insertion_order_is_stable_and_reproducible() {
        fn build(reverse: bool) -> Vec<u8> {
            let mut builder = ConstantPoolBuilder::new();
            if reverse {
                builder.long(9).unwrap();
                builder.method_ref("pkg/C", "m", "()V").unwrap();
                builder.class("pkg/C").unwrap();
                builder.integer(7).unwrap();
            } else {
                builder.integer(7).unwrap();
                builder.class("pkg/C").unwrap();
                builder.method_ref("pkg/C", "m", "()V").unwrap();
                builder.long(9).unwrap();
            }
            builder.finish().unwrap().encode().unwrap()
        }

        assert_eq!(build(false), build(false));
        assert_eq!(build(true), build(true));
        assert_ne!(build(false), build(true));
        let mut builder = ConstantPoolBuilder::new();
        let first = builder.integer(7).unwrap();
        builder.utf8("later").unwrap();
        assert_eq!(builder.integer(7), Ok(first));
        let pool = builder.finish().unwrap();
        assert_eq!(first, ConstantPoolIndex(1));
        assert!(matches!(
            pool.get(first),
            Some(ConstantPoolEntry::Integer(7))
        ));
    }

    #[test]
    fn long_and_double_reserve_one_following_slot() {
        let mut builder = ConstantPoolBuilder::new();
        let long = builder.long(1).unwrap();
        let double = builder.double(2.0).unwrap();
        let pool = builder.finish().unwrap();
        assert_eq!(long, ConstantPoolIndex(1));
        assert_eq!(double, ConstantPoolIndex(3));
        assert_eq!(pool.get(ConstantPoolIndex(2)), None);
        assert_eq!(pool.get(ConstantPoolIndex(4)), None);
    }

    #[test]
    fn modified_utf8_overflow_is_typed_and_does_not_mutate_the_builder() {
        let mut builder = ConstantPoolBuilder::new();
        let oversized = "x".repeat(usize::from(u16::MAX) + 1);
        assert_eq!(
            builder.utf8(&oversized),
            Err(ConstantPoolBuildError::InvalidUtf8(
                EncodeError::Utf8LengthOverflow {
                    byte_len: usize::from(u16::MAX) + 1,
                    max: u16::MAX,
                }
            ))
        );
        let next = builder.integer(1).unwrap();
        assert_eq!(next, ConstantPoolIndex(1));

        let mut compound = ConstantPoolBuilder::new();
        assert!(compound.method_ref("pkg/C", "m", &oversized).is_err());
        assert_eq!(compound.integer(1), Ok(ConstantPoolIndex(1)));
    }

    #[test]
    fn accepts_a_wide_constant_at_the_last_valid_boundary() {
        let mut builder = ConstantPoolBuilder::new();
        for value in 0..65_532 {
            builder.integer(value).unwrap();
        }
        let wide = builder.long(42).unwrap();
        assert_eq!(wide, ConstantPoolIndex(65_533));
        let pool = builder.finish().unwrap();
        assert_eq!(pool.entry_count(), 65_534);
        assert_eq!(pool.encode().unwrap()[..2], u16::MAX.to_be_bytes());
        assert_eq!(pool.get(ConstantPoolIndex(65_534)), None);
    }

    #[test]
    fn overflow_of_compound_insert_rolls_back_all_prerequisites() {
        let mut builder = ConstantPoolBuilder::new();
        for value in 0..65_530 {
            builder.integer(value).unwrap();
        }
        assert!(matches!(
            builder.method_ref("pkg/C", "m", "()V"),
            Err(ConstantPoolBuildError::CountOverflow { max: u16::MAX, .. })
        ));
        assert_eq!(builder.integer(65_530), Ok(ConstantPoolIndex(65_531)));
        assert_eq!(builder.entries.len(), 65_531);
        builder.finish().unwrap();
    }

    #[test]
    fn a_two_slot_insert_that_would_overflow_is_atomic() {
        let mut builder = ConstantPoolBuilder::new();
        for value in 0..65_533 {
            builder.integer(value).unwrap();
        }
        assert!(matches!(
            builder.long(42),
            Err(ConstantPoolBuildError::CountOverflow {
                count: 65_536,
                max: u16::MAX
            })
        ));
        assert_eq!(builder.integer(65_533), Ok(ConstantPoolIndex(65_534)));
        let pool = builder.finish().unwrap();
        assert_eq!(pool.entry_count(), 65_534);
    }
}
