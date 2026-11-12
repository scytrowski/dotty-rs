use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::reader::Reader as ClassReader;
use dotty_tasty::ast::{
    APPLY_TAG, BLOCK_TAG, DEFDEF_TAG, DefinitionBody, IF_TAG, SHAREDTYPE_TAG, TERMREFDIRECT_TAG,
    VALDEF_TAG,
};
use dotty_tasty::file::TastyFile;
use dotty_tasty::term::RawTree;

const COUNTER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_conditional_reference_selection/scala39/Counter.class");
const COUNTER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_conditional_reference_selection/scala39/Counter.tasty");
const PAIR_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_conditional_reference_selection/scala39/Pair.class");
const PAIR_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_conditional_reference_selection/scala39/Pair.tasty");
const FACTORY_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_conditional_reference_selection/scala39/PairFactory.class");
const FACTORY_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_conditional_reference_selection/scala39/PairFactory.tasty");

fn decode_class(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = ClassReader::new(bytes);
    let class = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    class
}

fn raw_tag(tree: &RawTree<'_>) -> u8 {
    match tree {
        RawTree::Leaf(term) => term.tag,
        RawTree::Ast { tag, .. } | RawTree::NatAst { tag, .. } => *tag,
        RawTree::LengthNode(node) => node.tag,
    }
}

#[test]
fn scala39_conditional_reference_selection_oracle_records_if_tasty_and_jvm_join() {
    for (class_bytes, tasty_bytes, expected_name) in [
        (COUNTER_CLASS, COUNTER_TASTY, "example/Counter"),
        (PAIR_CLASS, PAIR_TASTY, "example/Pair"),
        (FACTORY_CLASS, FACTORY_TASTY, "example/PairFactory"),
    ] {
        let class = decode_class(class_bytes);
        let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes).unwrap();
        assert_eq!(class.version.major, 65);
        assert_eq!(
            class.constant_pool.class_name(class.this_class).unwrap(),
            expected_name
        );
        assert_eq!(
            class.tasty_link_id().unwrap().unwrap().as_bytes(),
            &tasty.header().uuid
        );
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version
            ),
            (28, 9, 0)
        );
    }

    let class = decode_class(FACTORY_CLASS);
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "create")
        .unwrap();
    assert_eq!(
        class.constant_pool.utf8(method.descriptor_index).unwrap(),
        "(IIZI)Lexample/Pair;"
    );
    let code = method
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Code(code) => Some(code),
            _ => None,
        })
        .unwrap();
    assert_eq!((code.max_stack, code.max_locals), (4, 8));

    let tasty = TastyFile::parse_and_validate_scala_3_9(FACTORY_TASTY).unwrap();
    let index = tasty.ast_address_index().unwrap();
    let (_, create) = index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let body = index
                .get(node.offset as u32)
                .unwrap()
                .decode_defdef_body()
                .unwrap();
            (tasty.names().render(body.name).unwrap() == "create")
                .then_some((node.offset as u32, body))
        })
        .unwrap();
    let RawTree::LengthNode(block_node) = create.rhs.unwrap() else {
        panic!("Scala 3.9 encodes create as a BLOCK");
    };
    assert_eq!(block_node.tag, BLOCK_TAG);
    let block = block_node.decode_block().unwrap();
    assert_eq!(block.stats.len(), 4);
    assert_eq!(
        block.stats.iter().map(raw_tag).collect::<Vec<_>>(),
        [VALDEF_TAG, VALDEF_TAG, VALDEF_TAG, APPLY_TAG]
    );
    assert_eq!(raw_tag(&block.expression), APPLY_TAG);

    let selected = index
        .iter_nodes_with_tag(VALDEF_TAG)
        .find_map(|node| {
            let body = index
                .get(node.offset as u32)
                .unwrap()
                .decode_definition_body()
                .unwrap();
            let DefinitionBody::ValDef { name, rhs, .. } = body else {
                return None;
            };
            (tasty.names().render(name).unwrap() == "selected")
                .then_some(rhs.expect("selected has an initializer"))
        })
        .unwrap();
    let RawTree::LengthNode(selected_if) = selected else {
        panic!("the selected initializer is a TASTy IF node");
    };
    assert_eq!(selected_if.tag, IF_TAG);
    let selected_if = selected_if.decode_if().unwrap();
    assert_eq!(raw_tag(&selected_if.condition), TERMREFDIRECT_TAG);
    assert_eq!(raw_tag(&selected_if.then_branch), SHAREDTYPE_TAG);
    assert_eq!(raw_tag(&selected_if.else_branch), SHAREDTYPE_TAG);

    let printed = include_str!(
        "fixtures/jvm_tasty_conditional_reference_selection/oracle/PairFactory.print-tasty.txt"
    );
    for observation in [
        "BLOCK(87)",
        "VALDEF(13) 28 [selected]",
        "IF(6)",
        "TERMREFdirect 53",
        "SHAREDtype 89",
        "SHAREDtype 91",
        "TERMREFdirect 93",
        "TERMREFdirect 116",
        "APPLY(10)",
        "TERMREFdirect 93",
        "TERMREFdirect 116",
    ] {
        assert!(
            printed.contains(observation),
            "missing TASTy observation {observation:?}"
        );
    }
    let decompiled = include_str!(
        "fixtures/jvm_tasty_conditional_reference_selection/oracle/PairFactory.decompile.txt"
    );
    for observation in [
        "val first: example.Counter = new example.Counter(firstSeed)",
        "val second: example.Counter = new example.Counter(secondSeed)",
        "val selected: example.Counter = if (chooseFirst) first else second",
        "selected.set(nextValue)",
        "new example.Pair(first, second)",
    ] {
        assert!(
            decompiled.contains(observation),
            "missing TASTy decompiler observation {observation:?}"
        );
    }

    let javap = include_str!(
        "fixtures/jvm_tasty_conditional_reference_selection/oracle/PairFactory.javap.txt"
    );
    assert!(javap.contains("descriptor: (IIZI)Lexample/Pair;"));
    assert!(javap.contains("stack=4, locals=8, args_size=5"));
    assert!(javap.contains("20: iload_3\n        21: ifeq          29"));
    assert!(javap.contains("24: aload         5\n        26: goto          31"));
    assert!(javap.contains("29: aload         6\n        31: astore        7"));
    assert!(javap.contains("33: aload         7\n        35: iload         4"));
    assert!(javap.contains("frame_type = 253 /* append */"));
    assert!(javap.contains("frame_type = 65 /* same_locals_1_stack_item */"));
    assert!(javap.contains("44: aload         5\n        46: aload         6"));
    let counter_allocations = javap
        .lines()
        .filter(|line| line.contains("new           ") && line.contains("class example/Counter"))
        .count();
    assert_eq!(counter_allocations, 2);
}
