//! Typed JVM instructions and symbolic branches for the deliberately small
//! bytecode subset supported by this crate.
//!
//! [`assembler::assemble`] encodes straight-line instructions, while
//! [`assembler::assemble_labeled`] lays out and encodes symbolic branches.
//! [`analysis::analyze`] computes straight-line `max_stack`/`max_locals`
//! bounds. These APIs do not decode bytecode or perform general JVM
//! verification or optimization. The linear assembler and analyzer continue
//! to represent one ordered straight-line block. [`code_sequence`] models
//! labels and a small branch family and validates targets before assembly.
//! The low-level [`Instruction`] constant-pool operands below contain assigned
//! [`ConstantPoolIndex`] values. [`CodeBuilder`] offers the owned class builder
//! a symbolic alternative that interns names before assembling those typed
//! operands. The assembler checks each low-level index and its constant-pool
//! tag without mutating the pool.
//!
//! The stack column uses JVM stack values except where it explicitly says
//! "slots". `getfield`, `putfield`, static fields, and calls depend on the
//! referenced descriptor. Local-index width is selected by the assembler:
//! the short `_0` through `_3` form is available where applicable, an index up
//! to 255 uses a one-byte operand, and larger indices use `wide` with a
//! two-byte operand. Constant-pool widths are `u2`, except `ldc`, whose
//! assembler may use a one-byte index when possible.
//!
//! | Instruction | Opcode / JVMS §6.5 | Operand width | Stack effect |
//! | --- | --- | --- | --- |
//! | `AConstNull` | [`aconst_null`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.aconst_null) `0x01` | 0 | `[] -> [reference]` |
//! | `IConst(MinusOne..Five)` | [`iconst_m1`..`iconst_5`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.iconst_i) `0x02`..`0x08` | 0 | `[] -> [int]` |
//! | `Bipush(i8)` | [`bipush`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.bipush) `0x10` | 1 signed | `[] -> [int]` |
//! | `Sipush(i16)` | [`sipush`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.sipush) `0x11` | 2 signed | `[] -> [int]` |
//! | `LConst(Zero..One)` | [`lconst_0`..`lconst_1`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.lconst_i) `0x09`..`0x0a` | 0 | `[] -> [long]` (2 slots) |
//! | `Ldc(Integer/Reference)` | [`ldc`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.ldc) `0x12` (u1 index), [`ldc_w`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.ldc_w) `0x13` (u2 index) | 1 or 2 | `[] -> [int/reference]` |
//! | `Ldc2(Long)` | [`ldc2_w`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.ldc2_w) `0x14` | 2 | `[] -> [long]` (2 slots) |
//! | `ILoad` | [`iload`, `iload_0`..`iload_3`, `wide iload`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.iload) `0x15`, `0x1a`..`0x1d`, `0xc4` | 0, 1, or `wide` + 2 | `[] -> [int]` |
//! | `LLoad` | [`lload`, `lload_0`..`lload_3`, `wide lload`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.lload) `0x16`, `0x1e`..`0x21`, `0xc4` | 0, 1, or `wide` + 2 | `[] -> [long]` (2 slots) |
//! | `ALoad` | [`aload`, `aload_0`..`aload_3`, `wide aload`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.aload) `0x19`, `0x2a`..`0x2d`, `0xc4` | 0, 1, or `wide` + 2 | `[] -> [reference]` |
//! | `IStore` | [`istore`, `istore_0`..`istore_3`, `wide istore`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.istore) `0x36`, `0x3b`..`0x3e`, `0xc4` | 0, 1, or `wide` + 2 | `[int] -> []` |
//! | `LStore` | [`lstore`, `lstore_0`..`lstore_3`, `wide lstore`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.lstore) `0x37`, `0x3f`..`0x42`, `0xc4` | 0, 1, or `wide` + 2 | `[long] -> []` |
//! | `AStore` | [`astore`, `astore_0`..`astore_3`, `wide astore`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.astore) `0x3a`, `0x4b`..`0x4e`, `0xc4` | 0, 1, or `wide` + 2 | `[reference] -> []` |
//! | `IAdd` | [`iadd`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.iadd) `0x60` | 0 | `[int, int] -> [int]` |
//! | `Dup` | [`dup`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.dup) `0x59` | 0 | `[category-1] -> [category-1, category-1]` |
//! | `GetField(FieldReference)` | [`getfield`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.getfield) `0xb4` | 2 | `[reference] -> [field value]` |
//! | `PutField(FieldReference)` | [`putfield`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.putfield) `0xb5` | 2 | `[reference, field value] -> []` |
//! | `GetStatic(FieldReference)` | [`getstatic`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.getstatic) `0xb2` | 2 | `[] -> [field value]` |
//! | `PutStatic(FieldReference)` | [`putstatic`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.putstatic) `0xb3` | 2 | `[field value] -> []` |
//! | `InvokeVirtual(VirtualMethodReference)` | [`invokevirtual`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.invokevirtual) `0xb6` | 2 | `[receiver, arguments] -> [result if non-void]` |
//! | `InvokeSpecial(SpecialMethodReference)` | [`invokespecial`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.invokespecial) `0xb7` | 2 | `[receiver, arguments] -> [result if non-void]` |
//! | `InvokeStatic(StaticMethodReference)` | [`invokestatic`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.invokestatic) `0xb8` | 2 | `[arguments] -> [result if non-void]` |
//! | `New(ClassReference)` | [`new`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.new) `0xbb` | 2 | `[] -> [reference]` |
//! | `Return` | [`return`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.return) `0xb1` | 0 | `[] -> terminal` |
//! | `IReturn` | [`ireturn`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.ireturn) `0xac` | 0 | `[int] -> terminal` |
//! | `AReturn` | [`areturn`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.areturn) `0xb0` | 0 | `[reference] -> terminal` |
//! | `LReturn` | [`lreturn`](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-6.html#jvms-6.5.lreturn) `0xad` | 0 | `[long] -> terminal` |
//!
//! The [`Instruction`] enum covers the straight-line instruction families
//! listed above. [`code_sequence::BranchInstruction`] additionally models
//! symbolic `goto`/`goto_w`, `ifeq`, `ifne`, and `if_icmp{eq,ne,lt,ge,gt,le}`
//! branches. It does not choose branch widths or emit branch bytes. Float and
//! double instructions, `invokeinterface`, `invokedynamic`, switches,
//! `jsr`/`ret`, exception handlers, monitor operations, array operations,
//! conversions, comparisons, and all other JVM instructions are intentionally
//! unsupported. There are no catch-all opcode or raw-byte variants, so
//! unsupported instructions cannot silently enter the model.
//!
//! [`ConstantPoolIndex`]: crate::constant_pool::ConstantPoolIndex

pub mod analysis;
pub mod assembler;
pub(crate) mod code_builder;
pub mod code_sequence;

pub(crate) use analysis::analyze_validated;
pub use analysis::{AnalysisError, CodeAnalysis, LinearCodeAnalyzer, ValueKind, analyze};
pub(crate) use assembler::assemble_validated;
pub use assembler::{
    AssembleError, AssembledCode, CodeLayout, ConstantPoolOperandKind, assemble, assemble_labeled,
};
pub use code_builder::{CodeBuilder, CodeBuilderError};
pub use code_sequence::{
    BranchCondition, BranchInstruction, CodeElement, CodeSequence, CodeSequenceError,
    CodeSequenceErrorKind, LabelId, ValidatedCodeSequence,
};

use crate::constant_pool::ConstantPoolIndex;
use std::fmt;

/// A JVM local-variable slot index. The bytecode operand is a `u16`; whether
/// the slot is within a method's `max_locals` is a later analysis concern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalIndex(u16);

impl LocalIndex {
    /// Creates a local index from its full-width JVM representation.
    pub const fn new(index: u16) -> Self {
        Self(index)
    }

    /// Returns the full-width JVM representation of this index.
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<usize> for LocalIndex {
    type Error = OperandError;

    fn try_from(index: usize) -> Result<Self, Self::Error> {
        u16::try_from(index)
            .map(Self)
            .map_err(|_| OperandError::LocalIndexOutOfRange { index })
    }
}

/// A nonzero constant-pool index that can be represented in a class file.
///
/// `constant_pool_count` is a `u16` and index zero is reserved, so the largest
/// representable entry index is 65534. This wrapper validates only the numeric
/// range; it does not prove that the entry exists or has an instruction-
/// appropriate tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConstantPoolIndexOperand(ConstantPoolIndex);

impl ConstantPoolIndexOperand {
    /// Validates an existing class-file constant-pool index for use as an
    /// instruction operand.
    pub const fn new(index: ConstantPoolIndex) -> Result<Self, OperandError> {
        if index.0 == 0 || index.0 == u16::MAX {
            Err(OperandError::InvalidConstantPoolIndex { index: index.0 })
        } else {
            Ok(Self(index))
        }
    }

    /// Returns the underlying class-file constant-pool index.
    pub const fn get(self) -> ConstantPoolIndex {
        self.0
    }
}

impl TryFrom<ConstantPoolIndex> for ConstantPoolIndexOperand {
    type Error = OperandError;

    fn try_from(index: ConstantPoolIndex) -> Result<Self, Self::Error> {
        Self::new(index)
    }
}

impl TryFrom<u16> for ConstantPoolIndexOperand {
    type Error = OperandError;

    fn try_from(index: u16) -> Result<Self, Self::Error> {
        Self::new(ConstantPoolIndex(index))
    }
}

/// A typed operand-construction error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandError {
    /// The requested local index does not fit the JVM's `u16` operand.
    LocalIndexOutOfRange { index: usize },
    /// Constant-pool index zero or 65535 cannot identify an entry.
    InvalidConstantPoolIndex { index: u16 },
}

impl fmt::Display for OperandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LocalIndexOutOfRange { index } => {
                write!(formatter, "local index {index} exceeds the JVM u16 range")
            }
            Self::InvalidConstantPoolIndex { index } => {
                write!(
                    formatter,
                    "constant-pool index {index} cannot identify an entry"
                )
            }
        }
    }
}

impl std::error::Error for OperandError {}

macro_rules! cp_reference_operand {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(ConstantPoolIndexOperand);

        impl $name {
            /// Wraps a validated constant-pool index in this instruction-use
            /// reference type.
            pub const fn new(index: ConstantPoolIndexOperand) -> Self {
                Self(index)
            }

            /// Returns the already-assigned constant-pool index.
            pub const fn index(self) -> ConstantPoolIndexOperand {
                self.0
            }
        }
    };
}

cp_reference_operand!(ClassReference, "A `CONSTANT_Class` operand used by `new`.");
cp_reference_operand!(
    FieldReference,
    "A `CONSTANT_Fieldref` operand used by field instructions."
);
cp_reference_operand!(
    StaticMethodReference,
    "A method-reference operand used by `invokestatic`."
);
cp_reference_operand!(
    VirtualMethodReference,
    "A `CONSTANT_Methodref` operand used by `invokevirtual`."
);
cp_reference_operand!(
    SpecialMethodReference,
    "A method-reference operand used by `invokespecial`."
);

/// The JVM value category produced by a supported constant-pool load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JvmValueKind {
    /// A category-one integer value.
    Int,
    /// A category-two long value.
    Long,
    /// A category-one reference value.
    Reference,
}

/// A category-one constant loaded with `ldc`/`ldc_w`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LdcConstant {
    /// A `CONSTANT_Integer` entry.
    Integer(ConstantPoolIndexOperand),
    /// A reference-valued loadable entry such as `CONSTANT_String` or
    /// `CONSTANT_Class`. The eventual assembler validates the actual tag.
    Reference(ConstantPoolIndexOperand),
}

impl LdcConstant {
    /// Returns the JVM value category produced by this load.
    pub const fn kind(self) -> JvmValueKind {
        match self {
            Self::Integer(_) => JvmValueKind::Int,
            Self::Reference(_) => JvmValueKind::Reference,
        }
    }

    /// Returns the already-assigned constant-pool operand.
    pub const fn index(self) -> ConstantPoolIndexOperand {
        match self {
            Self::Integer(index) | Self::Reference(index) => index,
        }
    }
}

/// A category-two constant loaded with `ldc2_w`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ldc2Constant {
    /// A `CONSTANT_Long` entry. Double constants are outside this v1 subset.
    Long(ConstantPoolIndexOperand),
}

impl Ldc2Constant {
    /// Returns the JVM value category produced by this load.
    pub const fn kind(self) -> JvmValueKind {
        match self {
            Self::Long(_) => JvmValueKind::Long,
        }
    }

    /// Returns the already-assigned constant-pool operand.
    pub const fn index(self) -> ConstantPoolIndexOperand {
        match self {
            Self::Long(index) => index,
        }
    }
}

/// The immediately representable integer constants used by `iconst_m1` to
/// `iconst_5`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SmallIntConstant {
    /// `iconst_m1`.
    MinusOne,
    /// `iconst_0`.
    Zero,
    /// `iconst_1`.
    One,
    /// `iconst_2`.
    Two,
    /// `iconst_3`.
    Three,
    /// `iconst_4`.
    Four,
    /// `iconst_5`.
    Five,
}

/// The immediately representable long constants used by `lconst_0` and
/// `lconst_1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SmallLongConstant {
    /// `lconst_0`.
    Zero,
    /// `lconst_1`.
    One,
}

/// A typed member of the v1 JVM straight-line instruction subset.
///
/// Local operands are slot indices; constant-pool operands are validated,
/// stable numeric indices wrapped in a reference type appropriate to their
/// instruction family. This model intentionally contains no raw opcode,
/// arbitrary byte sequence, branch target, or label variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Instruction {
    /// Pushes a null reference.
    AConstNull,
    /// Pushes an integer in the `iconst_m1` through `iconst_5` range.
    IConst(SmallIntConstant),
    /// Pushes a signed 8-bit integer using `bipush`.
    Bipush(i8),
    /// Pushes a signed 16-bit integer using `sipush`.
    Sipush(i16),
    /// Pushes zero or one using `lconst_0` or `lconst_1`.
    LConst(SmallLongConstant),
    /// Loads an integer or reference from a category-one constant-pool entry.
    Ldc(LdcConstant),
    /// Loads a long from a category-two constant-pool entry.
    Ldc2(Ldc2Constant),
    /// Loads an integer local.
    ILoad(LocalIndex),
    /// Loads a long local.
    LLoad(LocalIndex),
    /// Loads a reference local.
    ALoad(LocalIndex),
    /// Stores an integer local.
    IStore(LocalIndex),
    /// Stores a long local.
    LStore(LocalIndex),
    /// Stores a reference local.
    AStore(LocalIndex),
    /// Adds two integers.
    IAdd,
    /// Duplicates the top category-one stack value.
    Dup,
    /// Reads an instance field.
    GetField(FieldReference),
    /// Writes an instance field.
    PutField(FieldReference),
    /// Reads a static field.
    GetStatic(FieldReference),
    /// Writes a static field.
    PutStatic(FieldReference),
    /// Invokes a static method.
    InvokeStatic(StaticMethodReference),
    /// Invokes a virtual class method.
    InvokeVirtual(VirtualMethodReference),
    /// Invokes a special method, such as a constructor or superclass method.
    InvokeSpecial(SpecialMethodReference),
    /// Allocates an instance of a class.
    New(ClassReference),
    /// Returns from a void method.
    Return,
    /// Returns an integer from a method.
    IReturn,
    /// Returns a reference from a method.
    AReturn,
    /// Returns a long from a method.
    LReturn,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp(index: u16) -> ConstantPoolIndexOperand {
        ConstantPoolIndexOperand::try_from(index).unwrap()
    }

    #[test]
    fn local_indices_cover_u16_boundaries_without_truncation() {
        for index in [0, 255, 256, usize::from(u16::MAX)] {
            let local = LocalIndex::try_from(index).unwrap();
            assert_eq!(usize::from(local.get()), index);
        }
        assert_eq!(
            LocalIndex::try_from(usize::from(u16::MAX) + 1),
            Err(OperandError::LocalIndexOutOfRange { index: 65536 })
        );
        assert_eq!(
            Instruction::ILoad(LocalIndex::new(u16::MAX)),
            Instruction::ILoad(LocalIndex::new(65535))
        );
    }

    #[test]
    fn constant_pool_indices_validate_reserved_and_boundary_values() {
        assert_eq!(cp(1).get(), ConstantPoolIndex(1));
        for index in [255, 256, u16::MAX - 1] {
            assert_eq!(cp(index).get(), ConstantPoolIndex(index));
        }
        for index in [0, u16::MAX] {
            assert_eq!(
                ConstantPoolIndexOperand::try_from(index),
                Err(OperandError::InvalidConstantPoolIndex { index })
            );
        }
    }

    #[test]
    fn ldc_operands_preserve_their_value_category() {
        let integer = LdcConstant::Integer(cp(1));
        let reference = LdcConstant::Reference(cp(2));
        let long = Ldc2Constant::Long(cp(3));

        assert_eq!(integer.kind(), JvmValueKind::Int);
        assert_eq!(reference.kind(), JvmValueKind::Reference);
        assert_eq!(long.kind(), JvmValueKind::Long);
        assert_eq!(integer.index().get(), ConstantPoolIndex(1));
        assert_eq!(reference.index().get(), ConstantPoolIndex(2));
        assert_eq!(long.index().get(), ConstantPoolIndex(3));
    }

    #[test]
    fn every_supported_instruction_variant_is_typed_and_comparable() {
        let local = LocalIndex::new(256);
        let field = FieldReference::new(cp(1));
        let class = ClassReference::new(cp(2));
        let static_method = StaticMethodReference::new(cp(3));
        let virtual_method = VirtualMethodReference::new(cp(4));
        let special_method = SpecialMethodReference::new(cp(5));
        let ldc_int = LdcConstant::Integer(cp(6));
        let ldc_ref = LdcConstant::Reference(cp(7));
        let ldc_long = Ldc2Constant::Long(cp(8));

        let instructions = [
            Instruction::AConstNull,
            Instruction::IConst(SmallIntConstant::MinusOne),
            Instruction::IConst(SmallIntConstant::Zero),
            Instruction::IConst(SmallIntConstant::One),
            Instruction::IConst(SmallIntConstant::Two),
            Instruction::IConst(SmallIntConstant::Three),
            Instruction::IConst(SmallIntConstant::Four),
            Instruction::IConst(SmallIntConstant::Five),
            Instruction::Bipush(i8::MIN),
            Instruction::Bipush(i8::MAX),
            Instruction::Sipush(i16::MIN),
            Instruction::Sipush(i16::MAX),
            Instruction::LConst(SmallLongConstant::Zero),
            Instruction::LConst(SmallLongConstant::One),
            Instruction::Ldc(ldc_int),
            Instruction::Ldc(ldc_ref),
            Instruction::Ldc2(ldc_long),
            Instruction::ILoad(local),
            Instruction::LLoad(local),
            Instruction::ALoad(local),
            Instruction::IStore(local),
            Instruction::LStore(local),
            Instruction::AStore(local),
            Instruction::IAdd,
            Instruction::Dup,
            Instruction::GetField(field),
            Instruction::PutField(field),
            Instruction::GetStatic(field),
            Instruction::PutStatic(field),
            Instruction::InvokeStatic(static_method),
            Instruction::InvokeVirtual(virtual_method),
            Instruction::InvokeSpecial(special_method),
            Instruction::New(class),
            Instruction::Return,
            Instruction::IReturn,
            Instruction::AReturn,
            Instruction::LReturn,
        ];
        assert_eq!(
            instructions,
            [
                Instruction::AConstNull,
                Instruction::IConst(SmallIntConstant::MinusOne),
                Instruction::IConst(SmallIntConstant::Zero),
                Instruction::IConst(SmallIntConstant::One),
                Instruction::IConst(SmallIntConstant::Two),
                Instruction::IConst(SmallIntConstant::Three),
                Instruction::IConst(SmallIntConstant::Four),
                Instruction::IConst(SmallIntConstant::Five),
                Instruction::Bipush(i8::MIN),
                Instruction::Bipush(i8::MAX),
                Instruction::Sipush(i16::MIN),
                Instruction::Sipush(i16::MAX),
                Instruction::LConst(SmallLongConstant::Zero),
                Instruction::LConst(SmallLongConstant::One),
                Instruction::Ldc(LdcConstant::Integer(cp(6))),
                Instruction::Ldc(LdcConstant::Reference(cp(7))),
                Instruction::Ldc2(Ldc2Constant::Long(cp(8))),
                Instruction::ILoad(local),
                Instruction::LLoad(local),
                Instruction::ALoad(local),
                Instruction::IStore(local),
                Instruction::LStore(local),
                Instruction::AStore(local),
                Instruction::IAdd,
                Instruction::Dup,
                Instruction::GetField(FieldReference::new(cp(1))),
                Instruction::PutField(FieldReference::new(cp(1))),
                Instruction::GetStatic(FieldReference::new(cp(1))),
                Instruction::PutStatic(FieldReference::new(cp(1))),
                Instruction::InvokeStatic(StaticMethodReference::new(cp(3))),
                Instruction::InvokeVirtual(VirtualMethodReference::new(cp(4))),
                Instruction::InvokeSpecial(SpecialMethodReference::new(cp(5))),
                Instruction::New(ClassReference::new(cp(2))),
                Instruction::Return,
                Instruction::IReturn,
                Instruction::AReturn,
                Instruction::LReturn,
            ]
        );
        assert_eq!(field.index().get(), ConstantPoolIndex(1));
        assert_eq!(static_method.index().get(), ConstantPoolIndex(3));
        assert_eq!(virtual_method.index().get(), ConstantPoolIndex(4));
        assert_eq!(special_method.index().get(), ConstantPoolIndex(5));
        assert_eq!(class.index().get(), ConstantPoolIndex(2));
    }
}
