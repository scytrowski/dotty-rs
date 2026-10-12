use dotty_classfile::classfile::{
    ClassReference, ConstantPoolIndex, ConstantPoolIndexOperand, FieldReference, Instruction,
    LdcConstant, LocalIndex, SmallIntConstant, StaticMethodReference, VirtualMethodReference,
};

#[test]
fn public_classfile_api_exposes_typed_straight_line_instructions() {
    let class = ClassReference::new(ConstantPoolIndexOperand::new(ConstantPoolIndex(1)).unwrap());
    let field = FieldReference::new(ConstantPoolIndexOperand::new(ConstantPoolIndex(2)).unwrap());
    let static_method =
        StaticMethodReference::new(ConstantPoolIndexOperand::new(ConstantPoolIndex(3)).unwrap());
    let virtual_method =
        VirtualMethodReference::new(ConstantPoolIndexOperand::new(ConstantPoolIndex(4)).unwrap());
    let instructions = [
        Instruction::IConst(SmallIntConstant::Five),
        Instruction::Ldc(LdcConstant::Integer(
            ConstantPoolIndexOperand::new(ConstantPoolIndex(5)).unwrap(),
        )),
        Instruction::GetField(field),
        Instruction::InvokeStatic(static_method),
        Instruction::InvokeVirtual(virtual_method),
        Instruction::New(class),
        Instruction::Return,
    ];

    assert_eq!(instructions.len(), 7);
    assert_eq!(instructions[0], Instruction::IConst(SmallIntConstant::Five));
}

#[test]
fn public_operand_types_reject_unusable_constant_pool_indices() {
    assert!(ConstantPoolIndexOperand::new(ConstantPoolIndex(0)).is_err());
    assert!(ConstantPoolIndexOperand::new(ConstantPoolIndex(u16::MAX)).is_err());
    assert_eq!(LocalIndex::try_from(65535).unwrap().get(), 65535);
    assert!(LocalIndex::try_from(65536).is_err());
}
