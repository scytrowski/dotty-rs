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
fn public_analyzer_validates_int_bitwise_stack_effects() {
    let pool = ConstantPoolBuilder::new().finish().unwrap();
    for operation in [Instruction::IAnd, Instruction::IOr, Instruction::IXor] {
        let instructions = [
            Instruction::ILoad(LocalIndex::new(0)),
            Instruction::ILoad(LocalIndex::new(1)),
            operation,
            Instruction::IReturn,
        ];
        assert_eq!(
            analyze("(II)I", true, &instructions, &pool),
            Ok(CodeAnalysis {
                max_stack: 2,
                max_locals: 2,
            }),
            "{operation:?}"
        );
        assert_eq!(
            analyze("()I", true, &[operation, Instruction::IReturn], &pool),
            Err(dotty_classfile::bytecode::AnalysisError::StackUnderflow {
                instruction: 0,
                needed: 1,
                available: 0,
            }),
            "{operation:?}"
        );
    }

    let long_operand = [
        Instruction::LLoad(LocalIndex::new(0)),
        Instruction::IConst(dotty_classfile::bytecode::SmallIntConstant::One),
        Instruction::IAnd,
        Instruction::IReturn,
    ];
    assert_eq!(
        analyze("(J)I", true, &long_operand, &pool),
        Err(
            dotty_classfile::bytecode::AnalysisError::StackTypeMismatch {
                instruction: 2,
                expected: dotty_classfile::bytecode::ValueKind::Int,
                actual: dotty_classfile::bytecode::ValueKind::Long,
            }
        )
    );
    let reference_operand = [
        Instruction::AConstNull,
        Instruction::IConst(dotty_classfile::bytecode::SmallIntConstant::One),
        Instruction::IOr,
        Instruction::IReturn,
    ];
    assert_eq!(
        analyze("()I", true, &reference_operand, &pool),
        Err(
            dotty_classfile::bytecode::AnalysisError::StackTypeMismatch {
                instruction: 2,
                expected: dotty_classfile::bytecode::ValueKind::Int,
                actual: dotty_classfile::bytecode::ValueKind::Reference,
            }
        )
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
