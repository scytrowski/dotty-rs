use dotty_classfile::classfile::{
    CodeAnalysis, ConstantPoolBuilder, FieldReference, Instruction, LocalIndex, analyze,
};

fn cp_operand(
    index: dotty_classfile::classfile::ConstantPoolIndex,
) -> dotty_classfile::classfile::ConstantPoolIndexOperand {
    dotty_classfile::classfile::ConstantPoolIndexOperand::try_from(index).unwrap()
}

#[test]
fn public_analyzer_computes_static_arithmetic_bounds() {
    let mut pool_builder = ConstantPoolBuilder::new();
    pool_builder.integer(42).unwrap();
    let pool = pool_builder.finish().unwrap();
    let instructions = [
        Instruction::ILoad(LocalIndex::new(0)),
        Instruction::ILoad(LocalIndex::new(1)),
        Instruction::IAdd,
        Instruction::IReturn,
    ];

    assert_eq!(
        analyze("(II)I", true, &instructions, &pool),
        Ok(CodeAnalysis {
            max_stack: 2,
            max_locals: 2,
        })
    );
}

#[test]
fn public_analyzer_reads_field_widths_from_the_constant_pool() {
    let mut pool_builder = ConstantPoolBuilder::new();
    let field = pool_builder
        .field_ref("example/Counter", "count", "J")
        .unwrap();
    let pool = pool_builder.finish().unwrap();
    let instructions = [
        Instruction::GetStatic(FieldReference::new(cp_operand(field))),
        Instruction::LReturn,
    ];

    assert_eq!(
        analyze("()J", true, &instructions, &pool),
        Ok(CodeAnalysis {
            max_stack: 2,
            max_locals: 0,
        })
    );
}
