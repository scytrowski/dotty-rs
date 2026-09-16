pub const CONSTANT_UTF8_TAG: u8 = 1;
pub const CONSTANT_INTEGER_TAG: u8 = 3;
pub const CONSTANT_FLOAT_TAG: u8 = 4;
pub const CONSTANT_LONG_TAG: u8 = 5;
pub const CONSTANT_DOUBLE_TAG: u8 = 6;
pub const CONSTANT_CLASS_TAG: u8 = 7;
pub const CONSTANT_STRING_TAG: u8 = 8;
pub const CONSTANT_FIELDREF_TAG: u8 = 9;
pub const CONSTANT_METHODREF_TAG: u8 = 10;
pub const CONSTANT_INTERFACE_METHODREF_TAG: u8 = 11;
pub const CONSTANT_NAME_AND_TYPE_TAG: u8 = 12;
pub const CONSTANT_METHOD_HANDLE_TAG: u8 = 15;
pub const CONSTANT_METHOD_TYPE_TAG: u8 = 16;
pub const CONSTANT_DYNAMIC_TAG: u8 = 17;
pub const CONSTANT_INVOKE_DYNAMIC_TAG: u8 = 18;
pub const CONSTANT_MODULE_TAG: u8 = 19;
pub const CONSTANT_PACKAGE_TAG: u8 = 20;

/// A 1-based index into a [`ConstantPool`]. Index `0` is always invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConstantPoolIndex(pub u16);

/// An index into a `BootstrapMethods` attribute's `bootstrap_methods` array
/// (JVMS §4.7.23) — a distinct index space from [`ConstantPoolIndex`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BootstrapMethodIndex(pub u16);

/// The kind of a `CONSTANT_MethodHandle_info` reference (JVMS §4.4.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodHandleKind {
    GetField,
    GetStatic,
    PutField,
    PutStatic,
    InvokeVirtual,
    InvokeStatic,
    InvokeSpecial,
    NewInvokeSpecial,
    InvokeInterface,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConstantPoolEntry {
    Utf8(String),
    Integer(i32),
    Float(f32),
    Long(i64),
    Double(f64),
    Class {
        name_index: ConstantPoolIndex,
    },
    String {
        string_index: ConstantPoolIndex,
    },
    Fieldref {
        class_index: ConstantPoolIndex,
        name_and_type_index: ConstantPoolIndex,
    },
    Methodref {
        class_index: ConstantPoolIndex,
        name_and_type_index: ConstantPoolIndex,
    },
    InterfaceMethodref {
        class_index: ConstantPoolIndex,
        name_and_type_index: ConstantPoolIndex,
    },
    NameAndType {
        name_index: ConstantPoolIndex,
        descriptor_index: ConstantPoolIndex,
    },
    MethodHandle {
        reference_kind: MethodHandleKind,
        reference_index: ConstantPoolIndex,
    },
    MethodType {
        descriptor_index: ConstantPoolIndex,
    },
    Dynamic {
        bootstrap_method_attr_index: BootstrapMethodIndex,
        name_and_type_index: ConstantPoolIndex,
    },
    InvokeDynamic {
        bootstrap_method_attr_index: BootstrapMethodIndex,
        name_and_type_index: ConstantPoolIndex,
    },
    Module {
        name_index: ConstantPoolIndex,
    },
    Package {
        name_index: ConstantPoolIndex,
    },
}

/// A class file's constant pool.
///
/// `Long`/`Double` entries occupy two consecutive slots (JVMS §4.4.5); the
/// second slot is represented as `None` rather than a repeated entry.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConstantPool {
    entries: Vec<Option<ConstantPoolEntry>>,
}

impl ConstantPool {
    pub fn from_entries(entries: Vec<Option<ConstantPoolEntry>>) -> Self {
        Self { entries }
    }

    pub fn get(&self, index: ConstantPoolIndex) -> Option<&ConstantPoolEntry> {
        let position = usize::from(index.0).checked_sub(1)?;
        self.entries.get(position)?.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::{ConstantPool, ConstantPoolEntry, ConstantPoolIndex};

    #[test]
    fn constant_pool_get_returns_none_for_index_zero() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(1))]);

        assert_eq!(pool.get(ConstantPoolIndex(0)), None);
    }

    #[test]
    fn constant_pool_get_returns_the_entry_at_a_valid_index() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(42))]);

        assert_eq!(
            pool.get(ConstantPoolIndex(1)),
            Some(&ConstantPoolEntry::Integer(42))
        );
    }

    #[test]
    fn constant_pool_get_returns_none_for_an_out_of_range_index() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Integer(42))]);

        assert_eq!(pool.get(ConstantPoolIndex(2)), None);
    }

    #[test]
    fn constant_pool_get_returns_none_for_the_slot_after_a_long_or_double() {
        let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Long(1)), None]);

        assert_eq!(pool.get(ConstantPoolIndex(2)), None);
    }
}
