#[test]
fn pinned_semantic_oracle_preserves_floating_values_and_control_flow() {
    let oracle = include_str!("fixtures/jvm_tasty_floating_values/oracle.txt");
    for shape in [
        "scala.Float",
        "scala.Double",
        "-0.0f",
        "1.4E-45f",
        "4.9E-324",
        "matchedD",
        "return",
    ] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}

use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class;

fn emit(methods: &str) -> dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class FloatingValues {{ {methods} }}"),
        "FloatingValues.scala",
        SourceId::from_index(2475),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn floating_literal_wire_payloads_preserve_exact_bits() {
    use dotty_tasty::tasty::*;
    let mut values = Vec::new();
    for (ty, literal) in [
        ("Float", "0.0f"),
        ("Float", "-0.0f"),
        ("Float", "1.4e-45f"),
        ("Float", "3.4028235e38f"),
        ("Double", "0.0"),
        ("Double", "-0.0"),
        ("Double", "4.9e-324"),
        ("Double", "1.7976931348623157e308"),
    ] {
        let pair = emit(&format!("def f(): {ty} = {literal}"));
        let file = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
        let payload = file.section(StandardSection::Asts).unwrap().payload;
        let index = file.ast_address_index().unwrap();
        for n in index
            .iter_nodes()
            .filter(|n| matches!(n.tag, FLOATCONST_TAG | DOUBLECONST_TAG))
        {
            let RawTree::Leaf(term) =
                RawTree::decode(&mut Reader::new(&payload[n.offset..])).unwrap()
            else {
                panic!()
            };
            values.push(term.constant_value().unwrap().unwrap());
        }
    }
    assert_eq!(
        values,
        [
            ConstantValue::FloatBits(0),
            ConstantValue::FloatBits(0x80000000),
            ConstantValue::FloatBits(1),
            ConstantValue::FloatBits(0x7f7fffff),
            ConstantValue::DoubleBits(0),
            ConstantValue::DoubleBits(0x8000000000000000),
            ConstantValue::DoubleBits(1),
            ConstantValue::DoubleBits(0x7fefffffffffffff)
        ]
    );
}

#[test]
fn mixed_primitive_formals_have_exact_f_d_descriptors_and_physical_slots() {
    use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
    let pair = emit(
        "def f(i: Int, a: Float, n: Long, b: Double, flag: Boolean): Float = a; def d(i: Int, a: Float, n: Long, b: Double, flag: Boolean): Double = b; def intResult(x: Double): Int = 7",
    );
    let file = ClassFile::decode(&mut Reader::new(pair.classfile_bytes())).unwrap();
    for (name, descriptor, slots, bytes) in [
        ("f", "(IFJDZ)F", 8, &[0x24, 0xae][..]),
        ("d", "(IFJDZ)D", 8, &[0x18, 5, 0xaf][..]),
        ("intResult", "(D)I", 3, &[0x10, 7, 0xac][..]),
    ] {
        let m = file
            .methods
            .iter()
            .find(|m| file.constant_pool.utf8(m.name_index) == Ok(name))
            .unwrap();
        assert_eq!(file.constant_pool.utf8(m.descriptor_index), Ok(descriptor));
        let [Attribute::Code(code)] = m.attributes.as_slice() else {
            panic!()
        };
        assert_eq!(code.max_locals, slots);
        assert_eq!(code.code, bytes);
    }
}
