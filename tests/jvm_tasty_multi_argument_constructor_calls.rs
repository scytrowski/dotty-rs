use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use dotty::jvm_tasty_plan::with_paired_jvm_tasty_batch_plan;
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::reader::Reader as ClassReader;
use dotty_core::SourceId;
use dotty_jvm_tasty_contract::{JvmTastyContractError, validate_classfile_tasty_link};
use dotty_tasty::ast::{
    APPLY_TAG, DEFDEF_TAG, PARAM_TAG, SELECTIN_TAG, SHAREDTYPE_TAG, TERMREFDIRECT_TAG,
};
use dotty_tasty::file::TastyFile;
use dotty_tasty::tasty::{RenderedNameSignature, RenderedParamSig, RenderedSignedName};
use dotty_tasty::term::{AstRefKind, RawTree, TermValue};

const COUNTER_CLASS: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_multi_argument_constructor_calls/scala39/example/Counter.class"
);
const COUNTER_TASTY: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_multi_argument_constructor_calls/scala39/example/Counter.tasty"
);
const PAIR_CLASS: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_multi_argument_constructor_calls/scala39/example/Pair.class"
);
const PAIR_TASTY: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_multi_argument_constructor_calls/scala39/example/Pair.tasty"
);
const FACTORY_CLASS: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_multi_argument_constructor_calls/scala39/example/PairFactory.class"
);
const FACTORY_TASTY: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_multi_argument_constructor_calls/scala39/example/PairFactory.tasty"
);
const ORACLE_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactory.scala");
const ORACLE_CONSUMER: &str =
    include_str!("fixtures/jvm_tasty_multi_argument_constructor_calls/Consumer.scala");
const REQUIRE_SCALA_CONSUMER: &str = "DOTTY_REQUIRE_SCALA_TASTY_MULTI_ARGUMENT_CONSUMER";

#[test]
fn rust_batch_writer_emits_two_distinct_ordered_nested_constructor_receipts() {
    let first = rust_tasty_batch(1916);
    let second = rust_tasty_batch(1917);
    assert_eq!(first.len(), 3);
    assert_eq!(second.len(), 3);
    assert_eq!(first, second);
    assert_eq!(
        first
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        [
            "example/Counter.tasty",
            "example/Pair.tasty",
            "example/PairFactory.tasty"
        ]
    );
    let (_, factory_bytes) = first
        .iter()
        .find(|(path, _)| path == "example/PairFactory.tasty")
        .unwrap();
    let factory_tasty = tasty(factory_bytes);
    let (create_address, body) = method_body(&factory_tasty, "create");
    assert_eq!(
        body.parameters
            .iter()
            .map(|parameter| factory_tasty.names().render(parameter.name()).unwrap())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(body.parameters.len(), 2);
    for (ordinal, parameter) in body.parameters.iter().enumerate() {
        let parameter_type = parameter.decode_body().unwrap().type_tree;
        let type_names = parameter_type
            .name_refs()
            .into_iter()
            .map(|name| factory_tasty.names().render(name).unwrap())
            .collect::<Vec<_>>();
        if ordinal == 0 {
            assert_eq!(type_names, ["Int", "scala"]);
        } else {
            assert_eq!(type_names, ["Int"]);
            assert!(
                parameter_type
                    .nodes()
                    .iter()
                    .any(|node| node.tag == SHAREDTYPE_TAG)
            );
        }
    }
    assert_eq!(
        body.return_type
            .name_refs()
            .into_iter()
            .map(|name| factory_tasty.names().render(name).unwrap())
            .collect::<Vec<_>>(),
        ["Pair", "example"]
    );
    let index = factory_tasty.ast_address_index().unwrap();
    let first_address = parameter_address(&index, create_address);
    let second_address = index
        .iter_nodes_with_tag(PARAM_TAG)
        .find(|node| {
            node.offset as u32 != first_address
                && std::iter::successors(Some(*node), |parent| {
                    index.parent_of(parent.offset as u32)
                })
                .any(|parent| parent.offset as u32 == create_address)
        })
        .expect("create owns two separate formal PARAM nodes")
        .offset as u32;
    assert_ne!(first_address, second_address);
    let applies = index
        .iter_nodes_with_tag(APPLY_TAG)
        .filter(|node| {
            std::iter::successors(Some(*node), |parent| index.parent_of(parent.offset as u32))
                .any(|parent| parent.offset as u32 == create_address)
        })
        .map(|node| node.offset as u32)
        .collect::<Vec<_>>();
    assert_eq!(
        applies.len(),
        3,
        "outer call and both nested call roots are distinct"
    );
    assert_eq!(
        applies
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );

    let RawTree::LengthNode(outer_node) = body.rhs.expect("create has a body") else {
        panic!("outer expression is an APPLY node");
    };
    assert_eq!(outer_node.tag, APPLY_TAG);
    let outer = outer_node.decode_apply().unwrap();
    assert_eq!(outer.arguments.len(), 2);
    let RawTree::LengthNode(outer_select_node) = outer.function else {
        panic!("outer Pair constructor function is SELECTin");
    };
    assert_eq!(outer_select_node.tag, SELECTIN_TAG);
    let outer_select = outer_select_node.decode_select_in().unwrap();
    assert!(matches!(
        outer_select.qualifier,
        RawTree::Ast {
            tag: dotty_tasty::ast::NEW_TAG,
            ..
        }
    ));
    assert_eq!(
        factory_tasty
            .names()
            .render_signed_name(outer_select.name)
            .unwrap()
            .unwrap(),
        RenderedSignedName::Signed {
            original: "<init>".to_owned(),
            signature: RenderedNameSignature {
                parameters: [
                    RenderedParamSig::TermParameter("example.Counter".to_owned()),
                    RenderedParamSig::TermParameter("example.Counter".to_owned()),
                ]
                .into(),
                result: "example.Pair".to_owned(),
            },
        }
    );
    for (argument, formal_address) in outer.arguments.iter().zip([first_address, second_address]) {
        let RawTree::LengthNode(inner_node) = argument else {
            panic!("each Pair argument has a separate nested APPLY");
        };
        assert_eq!(inner_node.tag, APPLY_TAG);
        let inner = inner_node.decode_apply().unwrap();
        assert_eq!(inner.arguments.len(), 1);
        let RawTree::LengthNode(inner_select_node) = inner.function else {
            panic!("nested Counter constructor function is SELECTin");
        };
        assert_eq!(inner_select_node.tag, SELECTIN_TAG);
        let inner_select = inner_select_node.decode_select_in().unwrap();
        assert!(matches!(
            inner_select.qualifier,
            RawTree::Ast {
                tag: dotty_tasty::ast::NEW_TAG,
                ..
            }
        ));
        assert_eq!(
            factory_tasty
                .names()
                .render_signed_name(inner_select.name)
                .unwrap()
                .unwrap(),
            RenderedSignedName::Signed {
                original: "<init>".to_owned(),
                signature: RenderedNameSignature {
                    parameters: [RenderedParamSig::TermParameter("scala.Int".to_owned())].into(),
                    result: "example.Counter".to_owned(),
                },
            }
        );
        assert_eq!(
            inner.arguments[0].ast_refs()[0].kind,
            AstRefKind::TermRefDirect
        );
        assert_eq!(inner.arguments[0].ast_refs()[0].address, formal_address);
        let RawTree::Leaf(term) = &inner.arguments[0] else {
            panic!("each nested call refers to its original method formal");
        };
        assert_eq!(term.tag, TERMREFDIRECT_TAG);
        assert!(matches!(term.value, TermValue::AstRef(address) if address == formal_address));
    }
}

#[test]
fn rust_pair_writer_is_deterministic_for_renamed_and_reordered_source_variants() {
    let renamed = ORACLE_SOURCE
        .replace("package example", "package renamed.example")
        .replace("PairFactory", "Assembler")
        .replace("Pair", "Envelope");
    let counter_start = ORACLE_SOURCE.find("class Counter(start").unwrap();
    let pair_start = ORACLE_SOURCE.find("class Pair(first").unwrap();
    let factory_start = ORACLE_SOURCE.find("class PairFactory:").unwrap();
    let counter = &ORACLE_SOURCE[counter_start..pair_start];
    let pair = &ORACLE_SOURCE[pair_start..factory_start];
    let factory = &ORACLE_SOURCE[factory_start..];
    let reordered = format!("package example\n\n{factory}\n{pair}\n{counter}");
    for (source, source_name, first_id, second_id, expected) in [
        (
            renamed.as_str(),
            "Assembler.scala",
            1919,
            1920,
            [
                "renamed/example/Counter.tasty",
                "renamed/example/Envelope.tasty",
                "renamed/example/Assembler.tasty",
            ],
        ),
        (
            reordered.as_str(),
            "PairFactory.scala",
            1921,
            1922,
            [
                "example/PairFactory.tasty",
                "example/Pair.tasty",
                "example/Counter.tasty",
            ],
        ),
    ] {
        let first = rust_tasty_batch_for(source, source_name, first_id);
        let second = rust_tasty_batch_for(source, source_name, second_id);
        assert_eq!(
            first, second,
            "fresh sessions serialize {source_name} identically"
        );
        assert_eq!(
            first
                .iter()
                .map(|(path, _)| path.as_str())
                .collect::<Vec<_>>(),
            expected
        );
    }
}

fn task_tmp() -> PathBuf {
    std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/issue-1913/tmp"))
}

fn strip_ansi(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
        } else {
            output.push(character);
        }
    }
    output
}

fn decode_class(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = ClassReader::new(bytes);
    let class = ClassFile::decode(&mut reader).expect("Scala classfile decodes");
    assert!(reader.is_at_end());
    class
}

fn tasty(bytes: &[u8]) -> TastyFile<'_> {
    let tasty = TastyFile::parse_and_validate_scala_3_9(bytes).expect("Scala TASTy 28.9 decodes");
    assert_eq!(
        (
            tasty.header().major_version,
            tasty.header().minor_version,
            tasty.header().experimental_version,
        ),
        (28, 9, 0)
    );
    tasty
}

fn method<'a>(class: &'a ClassFile<'_>, name: &str) -> &'a dotty_classfile::method::MethodInfo<'a> {
    class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == name)
        .unwrap_or_else(|| panic!("class declares method {name}"))
}

fn code<'a>(
    method: &'a dotty_classfile::method::MethodInfo<'a>,
) -> &'a dotty_classfile::attribute::CodeAttribute<'a> {
    method
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Code(code) => Some(code),
            _ => None,
        })
        .expect("method has Code")
}

#[derive(Debug, PartialEq, Eq)]
enum Instruction {
    New(String),
    Dup,
    ILoad(u8),
    InvokeSpecial(String, String, String),
    AReturn,
}

fn decode_create_instructions(class: &ClassFile<'_>, bytes: &[u8]) -> Vec<Instruction> {
    let mut pc = 0;
    let mut result = Vec::new();
    while pc < bytes.len() {
        let opcode = bytes[pc];
        pc += 1;
        match opcode {
            0xbb | 0xb7 => {
                let index = u16::from_be_bytes([bytes[pc], bytes[pc + 1]]);
                pc += 2;
                match class.constant_pool.get(ConstantPoolIndex(index)).unwrap() {
                    ConstantPoolEntry::Class { name_index } if opcode == 0xbb => result.push(
                        Instruction::New(class.constant_pool.utf8(*name_index).unwrap().to_owned()),
                    ),
                    ConstantPoolEntry::Methodref {
                        class_index,
                        name_and_type_index,
                    } if opcode == 0xb7 => {
                        let owner = class.constant_pool.class_name(*class_index).unwrap();
                        let ConstantPoolEntry::NameAndType {
                            name_index,
                            descriptor_index,
                        } = class.constant_pool.get(*name_and_type_index).unwrap()
                        else {
                            panic!("invokespecial points to NameAndType");
                        };
                        result.push(Instruction::InvokeSpecial(
                            owner.to_owned(),
                            class.constant_pool.utf8(*name_index).unwrap().to_owned(),
                            class
                                .constant_pool
                                .utf8(*descriptor_index)
                                .unwrap()
                                .to_owned(),
                        ));
                    }
                    entry => panic!("instruction has the correct symbolic constant: {entry:?}"),
                }
            }
            0x59 => result.push(Instruction::Dup),
            0x1b => result.push(Instruction::ILoad(1)),
            0x1c => result.push(Instruction::ILoad(2)),
            0xb0 => result.push(Instruction::AReturn),
            other => panic!("unexpected instruction in PairFactory.create: {other:#x}"),
        }
    }
    result
}

fn method_body<'a>(
    tasty: &'a TastyFile<'_>,
    name: &str,
) -> (u32, dotty_tasty::ast::DefDefBody<'a>) {
    let index = tasty.ast_address_index().unwrap();
    index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let body = index
                .get(node.offset as u32)
                .unwrap()
                .decode_defdef_body()
                .unwrap();
            (tasty.names().render(body.name).unwrap() == name).then_some((node.offset as u32, body))
        })
        .unwrap_or_else(|| panic!("TASTy contains method {name}"))
}

fn parameter_address(index: &dotty_tasty::ast::AstAddressIndex<'_>, method: u32) -> u32 {
    index
        .iter_nodes_with_tag(PARAM_TAG)
        .find_map(|candidate| {
            let mut parent = Some(candidate);
            while let Some(node) = parent {
                if node.offset as u32 == method {
                    return Some(candidate.offset as u32);
                }
                parent = index.parent_of(node.offset as u32);
            }
            None
        })
        .expect("method owns PARAM nodes")
}

fn rust_tasty_batch(source_id: u32) -> Vec<(String, Vec<u8>)> {
    rust_tasty_batch_for(ORACLE_SOURCE, "PairFactory.scala", source_id)
}

fn rust_tasty_batch_for(source: &str, source_name: &str, source_id: u32) -> Vec<(String, Vec<u8>)> {
    with_paired_jvm_tasty_batch_plan(
        source,
        source_name,
        SourceId::from_index(source_id),
        |batch| {
            batch
                .classes
                .iter()
                .map(|class| {
                    let output = class.tasty_plan.pickle().unwrap();
                    (
                        output.intended_tasty_path().to_owned(),
                        output.bytes().to_vec(),
                    )
                })
                .collect::<Vec<_>>()
        },
    )
    .expect("the paired plan retains one canonical source session")
}

#[test]
fn scala_oracle_pins_three_class_abi_tasty_identity_and_sibling_tree_order() {
    let classes = [
        (decode_class(COUNTER_CLASS), tasty(COUNTER_TASTY)),
        (decode_class(PAIR_CLASS), tasty(PAIR_TASTY)),
        (decode_class(FACTORY_CLASS), tasty(FACTORY_TASTY)),
    ];
    for (class, tasty) in &classes {
        assert_eq!(class.version.major, 65);
        assert_eq!(
            class.tasty_link_id().unwrap().unwrap().as_bytes(),
            &tasty.header().uuid,
            "each Scala classfile links its own sidecar UUID"
        );
    }
    assert_eq!(
        classes
            .iter()
            .map(|(class, _)| class.constant_pool.class_name(class.this_class).unwrap())
            .collect::<Vec<_>>(),
        ["example/Counter", "example/Pair", "example/PairFactory"]
    );

    let (factory, _) = &classes[2];
    let factory_tasty = &classes[2].1;
    let mut missing_link = decode_class(FACTORY_CLASS);
    missing_link.attributes.retain(|attribute| match attribute {
        Attribute::Other(attribute) => {
            missing_link
                .constant_pool
                .utf8(attribute.name_index)
                .unwrap()
                != "TASTY"
        }
        _ => true,
    });
    assert!(matches!(
        validate_classfile_tasty_link(&missing_link, FACTORY_TASTY),
        Err(JvmTastyContractError::MissingTastyLink { .. })
    ));
    let mut malformed = FACTORY_TASTY.to_vec();
    malformed[0] ^= 0xff;
    assert!(TastyFile::parse_and_validate_scala_3_9(&malformed).is_err());
    let mut stale_uuid = FACTORY_TASTY.to_vec();
    let tasty_uuid = factory_tasty.header().uuid;
    let uuid_offset = stale_uuid
        .windows(tasty_uuid.len())
        .position(|bytes| bytes == tasty_uuid)
        .expect("factory sidecar includes its header UUID");
    stale_uuid[uuid_offset] ^= 1;
    assert!(TastyFile::parse_and_validate_scala_3_9(&stale_uuid).is_ok());
    assert!(matches!(
        validate_classfile_tasty_link(factory, &stale_uuid),
        Err(JvmTastyContractError::StaleTastyLink { .. })
    ));

    let create = method(factory, "create");
    assert_eq!(
        factory.constant_pool.utf8(create.descriptor_index),
        Ok("(II)Lexample/Pair;")
    );
    let create_code = code(create);
    assert_eq!(create_code.max_stack, 6);
    assert_eq!(create_code.max_locals, 3);
    assert_eq!(
        decode_create_instructions(factory, create_code.code),
        [
            Instruction::New("example/Pair".into()),
            Instruction::Dup,
            Instruction::New("example/Counter".into()),
            Instruction::Dup,
            Instruction::ILoad(1),
            Instruction::InvokeSpecial("example/Counter".into(), "<init>".into(), "(I)V".into()),
            Instruction::New("example/Counter".into()),
            Instruction::Dup,
            Instruction::ILoad(2),
            Instruction::InvokeSpecial("example/Counter".into(), "<init>".into(), "(I)V".into()),
            Instruction::InvokeSpecial(
                "example/Pair".into(),
                "<init>".into(),
                "(Lexample/Counter;Lexample/Counter;)V".into(),
            ),
            Instruction::AReturn,
        ]
    );

    let factory_tasty = &classes[2].1;
    let (create_address, body) = method_body(factory_tasty, "create");
    assert_eq!(body.parameters.len(), 2);
    assert_eq!(
        body.parameters
            .iter()
            .map(|parameter| factory_tasty.names().render(parameter.name()).unwrap())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert!(
        body.parameters
            .iter()
            .all(|parameter| parameter.tag() == PARAM_TAG)
    );
    for (ordinal, parameter) in body.parameters.iter().enumerate() {
        let parameter_type = parameter.decode_body().unwrap().type_tree;
        let names = parameter_type
            .name_refs()
            .into_iter()
            .map(|name| factory_tasty.names().render(name).unwrap())
            .collect::<Vec<_>>();
        if ordinal == 0 {
            assert_eq!(names, ["Int", "scala"]);
        } else {
            assert_eq!(names, ["Int"]);
            assert!(
                parameter_type
                    .nodes()
                    .iter()
                    .any(|node| node.tag == SHAREDTYPE_TAG)
            );
            assert_eq!(parameter_type.ast_refs()[0].kind, AstRefKind::SharedType);
        }
    }
    assert_eq!(
        body.return_type
            .name_refs()
            .into_iter()
            .map(|name| factory_tasty.names().render(name).unwrap())
            .collect::<Vec<_>>(),
        ["Pair", "example"]
    );
    let index = factory_tasty.ast_address_index().unwrap();
    let first_address = parameter_address(&index, create_address);
    let second_address = index
        .iter_nodes_with_tag(PARAM_TAG)
        .find(|node| {
            node.offset as u32 != first_address && {
                let mut parent = Some(*node);
                while let Some(parent_node) = parent {
                    if parent_node.offset as u32 == create_address {
                        return true;
                    }
                    parent = index.parent_of(parent_node.offset as u32);
                }
                false
            }
        })
        .expect("create owns a second distinct PARAM")
        .offset as u32;
    assert_ne!(first_address, second_address);
    let RawTree::LengthNode(outer_node) = body.rhs.expect("create has a body") else {
        panic!("outer expression is an APPLY node");
    };
    assert_eq!(outer_node.tag, APPLY_TAG);
    let outer = outer_node.decode_apply().unwrap();
    assert_eq!(outer.arguments.len(), 2);
    let RawTree::LengthNode(outer_select_node) = outer.function else {
        panic!("outer Pair constructor function is SELECTin");
    };
    assert_eq!(outer_select_node.tag, SELECTIN_TAG);
    let outer_select = outer_select_node.decode_select_in().unwrap();
    assert!(matches!(
        outer_select.qualifier,
        RawTree::Ast {
            tag: dotty_tasty::ast::NEW_TAG,
            ..
        }
    ));
    assert_eq!(
        factory_tasty
            .names()
            .render_signed_name(outer_select.name)
            .unwrap()
            .unwrap(),
        RenderedSignedName::Signed {
            original: "<init>".to_owned(),
            signature: RenderedNameSignature {
                parameters: [
                    RenderedParamSig::TermParameter("example.Counter".to_owned()),
                    RenderedParamSig::TermParameter("example.Counter".to_owned()),
                ]
                .into(),
                result: "example.Pair".to_owned(),
            },
        }
    );
    for (ordinal, (argument, formal_address)) in outer
        .arguments
        .iter()
        .zip([first_address, second_address])
        .enumerate()
    {
        let RawTree::LengthNode(inner_node) = argument else {
            panic!("each outer argument is its own nested APPLY");
        };
        assert_eq!(inner_node.tag, APPLY_TAG);
        let inner = inner_node.decode_apply().unwrap();
        assert_eq!(inner.arguments.len(), 1);
        let RawTree::LengthNode(inner_select_node) = inner.function else {
            panic!("nested Counter constructor function is SELECTin");
        };
        assert_eq!(inner_select_node.tag, SELECTIN_TAG);
        let inner_select = inner_select_node.decode_select_in().unwrap();
        assert!(matches!(
            inner_select.qualifier,
            RawTree::Ast {
                tag: dotty_tasty::ast::NEW_TAG,
                ..
            }
        ));
        assert_eq!(
            factory_tasty
                .names()
                .render_signed_name(inner_select.name)
                .unwrap()
                .unwrap(),
            RenderedSignedName::Signed {
                original: "<init>".to_owned(),
                signature: RenderedNameSignature {
                    parameters: [RenderedParamSig::TermParameter("scala.Int".to_owned())].into(),
                    result: "example.Counter".to_owned(),
                },
            }
        );
        let references = inner.arguments[0].ast_refs();
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].kind, AstRefKind::TermRefDirect);
        assert_eq!(references[0].address, formal_address);
        let RawTree::Leaf(term) = &inner.arguments[0] else {
            panic!("factory formal argument is a direct reference leaf");
        };
        assert_eq!(term.tag, TERMREFDIRECT_TAG);
        assert!(matches!(term.value, TermValue::AstRef(address) if address == formal_address));
        assert_eq!(
            factory_tasty
                .names()
                .render(body.parameters[ordinal].name())
                .unwrap(),
            if ordinal == 0 { "first" } else { "second" }
        );
    }
    // TASTy presents the outer constructor function before its argument children;
    // the JVM sequence confirms runtime evaluates Pair allocation first, then the
    // first and second sibling Counter branches in source order.
}

#[test]
fn strict_pinned_scala_consumer_reproduces_oracle_and_independent_mutation() {
    if std::env::var(REQUIRE_SCALA_CONSUMER).as_deref() != Ok("1") {
        return;
    }
    let scala_root = PathBuf::from(
        std::env::var_os("SCALA39_ROOT").expect("pinned Scala root for strict oracle"),
    );
    let java_home =
        PathBuf::from(std::env::var_os("JAVA_HOME").expect("JDK 21 home for strict oracle"));
    let revision = Command::new("git")
        .arg("-C")
        .arg(&scala_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("read pinned Scala revision");
    assert!(revision.status.success());
    assert_eq!(
        String::from_utf8_lossy(&revision.stdout).trim(),
        "777528f19a58e794c9954a42f433373472ec57f8"
    );
    let compiler = scala_root.join("dist/target/universal/stage/bin/scalac");
    let version = Command::new(&compiler)
        .arg("-version")
        .output()
        .expect("query pinned Scala compiler");
    assert!(version.status.success());
    assert!(
        String::from_utf8_lossy(&version.stdout).contains("3.9.0-bin-SNAPSHOT-git-777528f"),
        "{}",
        String::from_utf8_lossy(&version.stderr)
    );
    let java_version = Command::new(java_home.join("bin/java"))
        .arg("-version")
        .output()
        .expect("query JDK 21");
    assert!(java_version.status.success());
    let java_version_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&java_version.stdout),
        String::from_utf8_lossy(&java_version.stderr)
    );
    assert!(java_version_text.contains("21.0.13"), "{java_version_text}");

    let temp_root = task_tmp().join(format!("dotty-scala-multi-argument-{}", std::process::id()));
    let producer = temp_root.join("producer");
    let consumer = temp_root.join("consumer");
    let rust_output = temp_root.join("rust-output");
    let rust_rebuilt = temp_root.join("rust-from-tasty");
    let _ = fs::remove_dir_all(&temp_root);
    fs::create_dir_all(&producer).expect("create empty producer output");
    fs::create_dir_all(&consumer).expect("create empty consumer output");
    fs::create_dir_all(&rust_output).expect("create empty Rust TASTy output");
    fs::create_dir_all(&rust_rebuilt).expect("create empty Scala reconstruction output");
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source_path =
        Path::new("tests/fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactory.scala");
    let consumer_path =
        Path::new("tests/fixtures/jvm_tasty_multi_argument_constructor_calls/Consumer.scala");
    assert_eq!(
        fs::read_to_string(workspace.join(source_path)).unwrap(),
        ORACLE_SOURCE
    );
    assert_eq!(
        fs::read_to_string(workspace.join(consumer_path)).unwrap(),
        ORACLE_CONSUMER
    );
    let java_tmp_options = format!("-Djava.io.tmpdir={} -XX:-UsePerfData", task_tmp().display());
    let scalac = |args: &[&std::ffi::OsStr]| {
        let output = Command::new(&compiler)
            .args(args)
            .current_dir(&workspace)
            .env("JAVA_HOME", &java_home)
            .env("JAVA_TOOL_OPTIONS", &java_tmp_options)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    java_home.join("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .output()
            .expect("run pinned scalac");
        assert!(
            output.status.success(),
            "scalac failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    scalac(&[
        "-release".as_ref(),
        "21".as_ref(),
        "-d".as_ref(),
        producer.as_os_str(),
        source_path.as_os_str(),
    ]);
    let scala_lib = format!(
        "{}:{}",
        scala_root.join("dist/target/universal/stage/maven2/org/scala-lang/scala3-library_3/3.9.0-bin-SNAPSHOT/scala3-library_3-3.9.0-bin-SNAPSHOT.jar").display(),
        scala_root.join("dist/target/universal/stage/maven2/org/scala-lang/scala-library/3.9.0-bin-SNAPSHOT/scala-library-3.9.0-bin-SNAPSHOT.jar").display()
    );
    scalac(&[
        "-release".as_ref(),
        "21".as_ref(),
        "-classpath".as_ref(),
        format!("{}:{scala_lib}", producer.display()).as_ref(),
        "-d".as_ref(),
        consumer.as_os_str(),
        consumer_path.as_os_str(),
    ]);
    let expected = [
        ("example/Counter.class", COUNTER_CLASS),
        ("example/Counter.tasty", COUNTER_TASTY),
        ("example/Pair.class", PAIR_CLASS),
        ("example/Pair.tasty", PAIR_TASTY),
        ("example/PairFactory.class", FACTORY_CLASS),
        ("example/PairFactory.tasty", FACTORY_TASTY),
    ];
    for (path, bytes) in expected {
        assert_eq!(
            fs::read(producer.join(path)).expect("Scala emitted the pinned artifact"),
            bytes,
            "pinned Scala recompilation matches {path} byte for byte"
        );
    }
    let classpath = format!(
        "{}:{}:{}",
        consumer.display(),
        producer.display(),
        scala_lib
    );
    let run = Command::new(java_home.join("bin/java"))
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(classpath)
        .arg("example.PairFactoryConsumer")
        .current_dir(&workspace)
        .env("JAVA_HOME", &java_home)
        .output()
        .expect("run Scala-only consumer with JVM verification");
    assert!(
        run.status.success(),
        "Scala-only consumer failed:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );

    for (relative_path, bytes) in rust_tasty_batch(1918) {
        let output_path = rust_output.join(relative_path);
        fs::create_dir_all(output_path.parent().unwrap()).unwrap();
        fs::write(output_path, bytes).unwrap();
    }
    let rust_sidecars = [
        rust_output.join("example/Counter.tasty"),
        rust_output.join("example/Pair.tasty"),
        rust_output.join("example/PairFactory.tasty"),
    ];
    for sidecar in &rust_sidecars {
        let output = scalac(&["-decompile".as_ref(), sidecar.as_os_str()]);
        if sidecar.ends_with("PairFactory.tasty") {
            let decompiled = strip_ansi(&String::from_utf8_lossy(&output.stdout));
            assert!(
                decompiled
                    .contains("def create(first: scala.Int, second: scala.Int): example.Pair"),
                "{decompiled}"
            );
            let construction = decompiled.rsplit_once(" = ").unwrap().1;
            assert!(
                construction.contains(
                    "new example.Pair(new example.Counter(first), new example.Counter(second))"
                ),
                "{decompiled}"
            );
            let first = construction
                .find("new example.Counter(first)")
                .expect("decompiled Factory retains the first nested constructor");
            let second = construction
                .find("new example.Counter(second)")
                .expect("decompiled Factory retains the second nested constructor");
            assert!(
                first < second,
                "Scala decompilation preserves source argument order: {decompiled}"
            );
        }
    }
    let rust_classpath = format!("{}:{scala_lib}", rust_output.display());
    scalac(&[
        "-release".as_ref(),
        "21".as_ref(),
        "-from-tasty".as_ref(),
        "-classpath".as_ref(),
        rust_classpath.as_ref(),
        "-d".as_ref(),
        rust_rebuilt.as_os_str(),
        rust_sidecars[0].as_os_str(),
        rust_sidecars[1].as_os_str(),
        rust_sidecars[2].as_os_str(),
    ]);
    for class in ["Counter", "Pair", "PairFactory"] {
        assert!(
            rust_rebuilt
                .join(format!("example/{class}.class"))
                .is_file(),
            "Scala -from-tasty reconstructs {class}.class from the three Rust sidecars"
        );
    }
}
