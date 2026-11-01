use std::path::{Path, PathBuf};
use std::process::Command;

use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::reader::Reader as ClassReader;
use dotty_jvm_tasty_contract::validate_classfile_tasty_link;
use dotty_tasty::ast::{APPLY_TAG, DEFDEF_TAG, PARAM_TAG, SELECTIN_TAG, TERMREFDIRECT_TAG};
use dotty_tasty::file::TastyFile;
use dotty_tasty::tasty::{RenderedNameSignature, RenderedParamSig, RenderedSignedName};
use dotty_tasty::term::{AstRefKind, RawTree, TermValue};

const FIXTURE: &str = "tests/fixtures/jvm_tasty_dual_reference_forwarding";
const COUNTER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_dual_reference_forwarding/scala39/example/Counter.class");
const COUNTER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_dual_reference_forwarding/scala39/example/Counter.tasty");
const PAIR_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_dual_reference_forwarding/scala39/example/Pair.class");
const PAIR_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_dual_reference_forwarding/scala39/example/Pair.tasty");
const FACTORY_CLASS: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_dual_reference_forwarding/scala39/example/PairFactory.class"
);
const FACTORY_TASTY: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_dual_reference_forwarding/scala39/example/PairFactory.tasty"
);
const REQUIRE_ORACLE: &str = "DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_FORWARDING_ORACLE";

fn decode_class(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = ClassReader::new(bytes);
    let class = ClassFile::decode(&mut reader).expect("Scala classfile decodes");
    assert!(reader.is_at_end());
    class
}

fn tasty(bytes: &[u8]) -> TastyFile<'_> {
    let file = TastyFile::parse_and_validate_scala_3_9(bytes).expect("Scala TASTy 28.9 decodes");
    assert_eq!(
        (
            file.header().major_version,
            file.header().minor_version,
            file.header().experimental_version
        ),
        (28, 9, 0)
    );
    file
}

fn method<'a>(class: &'a ClassFile<'_>, name: &str) -> &'a dotty_classfile::method::MethodInfo<'a> {
    class
        .methods
        .iter()
        .find(|m| class.constant_pool.utf8(m.name_index) == Ok(name))
        .unwrap()
}

fn code<'a>(
    method: &'a dotty_classfile::method::MethodInfo<'a>,
) -> &'a dotty_classfile::attribute::CodeAttribute<'a> {
    method
        .attributes
        .iter()
        .find_map(|a| match a {
            Attribute::Code(code) => Some(code),
            _ => None,
        })
        .unwrap()
}

#[derive(Debug, PartialEq, Eq)]
enum Op {
    New(String),
    Dup,
    ALoad(u8),
    InvokeSpecial(String, String, String),
    AReturn,
}

fn decode_wrap(class: &ClassFile<'_>, bytes: &[u8]) -> Vec<Op> {
    let mut pc = 0;
    let mut ops = Vec::new();
    while pc < bytes.len() {
        let opcode = bytes[pc];
        pc += 1;
        match opcode {
            0xbb | 0xb7 => {
                let index = u16::from_be_bytes([bytes[pc], bytes[pc + 1]]);
                pc += 2;
                match class.constant_pool.get(ConstantPoolIndex(index)).unwrap() {
                    ConstantPoolEntry::Class { name_index } if opcode == 0xbb => ops.push(Op::New(
                        class.constant_pool.utf8(*name_index).unwrap().to_owned(),
                    )),
                    ConstantPoolEntry::Methodref {
                        class_index,
                        name_and_type_index,
                    } if opcode == 0xb7 => {
                        let ConstantPoolEntry::NameAndType {
                            name_index,
                            descriptor_index,
                        } = class.constant_pool.get(*name_and_type_index).unwrap()
                        else {
                            panic!("Methodref has NameAndType")
                        };
                        ops.push(Op::InvokeSpecial(
                            class
                                .constant_pool
                                .class_name(*class_index)
                                .unwrap()
                                .to_owned(),
                            class.constant_pool.utf8(*name_index).unwrap().to_owned(),
                            class
                                .constant_pool
                                .utf8(*descriptor_index)
                                .unwrap()
                                .to_owned(),
                        ));
                    }
                    entry => panic!("symbolic instruction entry matches opcode: {entry:?}"),
                }
            }
            0x59 => ops.push(Op::Dup),
            0x2b => ops.push(Op::ALoad(1)),
            0x2c => ops.push(Op::ALoad(2)),
            0xb0 => ops.push(Op::AReturn),
            other => panic!("unexpected wrap opcode {other:#x}"),
        }
    }
    ops
}

fn method_body<'a>(file: &'a TastyFile<'a>, name: &str) -> (u32, dotty_tasty::ast::DefDefBody<'a>) {
    let index = file.ast_address_index().unwrap();
    index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let body = index.get(node.offset as u32)?.decode_defdef_body().ok()?;
            (file.names().render(body.name).ok()? == name).then_some((node.offset as u32, body))
        })
        .unwrap_or_else(|| panic!("TASTy declares {name}"))
}

fn parameter_addresses(
    index: &dotty_tasty::ast::AstAddressIndex<'_>,
    method_address: u32,
) -> Vec<u32> {
    index
        .iter_nodes_with_tag(PARAM_TAG)
        .filter(|node| {
            let mut parent = index.parent_of(node.offset as u32);
            while let Some(item) = parent {
                if item.offset as u32 == method_address {
                    return true;
                }
                parent = index.parent_of(item.offset as u32);
            }
            false
        })
        .map(|node| node.offset as u32)
        .collect()
}

#[test]
fn scala_oracle_freezes_dual_reference_abi_wire_identity_and_bytecode() {
    let pairs = [
        (
            decode_class(COUNTER_CLASS),
            tasty(COUNTER_TASTY),
            "example/Counter",
        ),
        (decode_class(PAIR_CLASS), tasty(PAIR_TASTY), "example/Pair"),
        (
            decode_class(FACTORY_CLASS),
            tasty(FACTORY_TASTY),
            "example/PairFactory",
        ),
    ];
    for (class, tasty, expected_name) in &pairs {
        assert_eq!(class.version.major, 65);
        assert_eq!(
            class.constant_pool.class_name(class.this_class).unwrap(),
            *expected_name
        );
        assert_eq!(
            class.tasty_link_id().unwrap().unwrap().as_bytes(),
            &tasty.header().uuid
        );
        validate_classfile_tasty_link(
            class,
            if *expected_name == "example/Counter" {
                COUNTER_TASTY
            } else if *expected_name == "example/Pair" {
                PAIR_TASTY
            } else {
                FACTORY_TASTY
            },
        )
        .unwrap();
    }

    let factory = &pairs[2].0;
    let wrap = method(factory, "wrap");
    assert_eq!(
        factory.constant_pool.utf8(wrap.descriptor_index),
        Ok("(Lexample/Counter;Lexample/Counter;)Lexample/Pair;")
    );
    let wrap_code = code(wrap);
    assert_eq!((wrap_code.max_stack, wrap_code.max_locals), (4, 3));
    assert_eq!(
        decode_wrap(factory, wrap_code.code),
        [
            Op::New("example/Pair".into()),
            Op::Dup,
            Op::ALoad(1),
            Op::ALoad(2),
            Op::InvokeSpecial(
                "example/Pair".into(),
                "<init>".into(),
                "(Lexample/Counter;Lexample/Counter;)V".into()
            ),
            Op::AReturn,
        ]
    );
    let bytecode = decode_wrap(factory, wrap_code.code);
    assert!(
        !bytecode
            .iter()
            .any(|op| matches!(op, Op::New(name) if name == "example/Counter"))
    );
    assert!(!bytecode.iter().any(|op| matches!(op, Op::InvokeSpecial(owner, name, _) if owner == "example/Counter" && name == "<init>")));

    let mut wrong_descriptor = FACTORY_CLASS.to_vec();
    let original_name = b"example/Counter";
    let name_offset = wrong_descriptor
        .windows(original_name.len())
        .position(|window| window == original_name)
        .unwrap();
    wrong_descriptor[name_offset + b"example/".len()] = b'Z';
    let wrong_descriptor_class = decode_class(&wrong_descriptor);
    let wrong_wrap = method(&wrong_descriptor_class, "wrap");
    assert_ne!(
        wrong_descriptor_class
            .constant_pool
            .utf8(wrong_wrap.descriptor_index),
        Ok("(Lexample/Counter;Lexample/Counter;)Lexample/Pair;")
    );

    let mut stale_tasty = FACTORY_TASTY.to_vec();
    let factory_uuid = pairs[2].1.header().uuid;
    let uuid_offset = stale_tasty
        .windows(factory_uuid.len())
        .position(|window| window == factory_uuid)
        .unwrap();
    stale_tasty[uuid_offset] ^= 1;
    let stale_file = tasty(&stale_tasty);
    assert!(matches!(
        validate_classfile_tasty_link(factory, &stale_tasty),
        Err(dotty_jvm_tasty_contract::JvmTastyContractError::StaleTastyLink { .. })
    ));
    assert_ne!(
        stale_file.header().uuid,
        *factory.tasty_link_id().unwrap().unwrap().as_bytes()
    );

    let factory_tasty = &pairs[2].1;
    let (method_address, body) = method_body(factory_tasty, "wrap");
    assert_eq!(factory_tasty.names().render(body.name).unwrap(), "wrap");
    assert_eq!(body.parameters.len(), 2);
    assert_eq!(
        body.parameters
            .iter()
            .map(|p| factory_tasty.names().render(p.name()).unwrap())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(
        body.return_type
            .name_refs()
            .into_iter()
            .map(|n| factory_tasty.names().render(n).unwrap())
            .collect::<Vec<_>>(),
        ["Pair"]
    );
    let index = factory_tasty.ast_address_index().unwrap();
    let params = parameter_addresses(&index, method_address);
    assert_eq!(params.len(), 2);
    assert_ne!(params[0], params[1]);
    let RawTree::LengthNode(outer_node) = body.rhs.unwrap() else {
        panic!("wrap body is APPLY")
    };
    assert_eq!(outer_node.tag, APPLY_TAG);
    let outer = outer_node.decode_apply().unwrap();
    let RawTree::LengthNode(select_node) = outer.function else {
        panic!("constructor function is SELECTin")
    };
    assert_eq!(select_node.tag, SELECTIN_TAG);
    let select = select_node.decode_select_in().unwrap();
    assert!(matches!(
        select.qualifier,
        RawTree::Ast {
            tag: dotty_tasty::ast::NEW_TAG,
            ..
        }
    ));
    assert_eq!(
        factory_tasty
            .names()
            .render_signed_name(select.name)
            .unwrap()
            .unwrap(),
        RenderedSignedName::Signed {
            original: "<init>".to_owned(),
            signature: RenderedNameSignature {
                parameters: [
                    RenderedParamSig::TermParameter("example.Counter".into()),
                    RenderedParamSig::TermParameter("example.Counter".into())
                ]
                .into(),
                result: "example.Pair".into()
            },
        }
    );
    assert_eq!(outer.arguments.len(), 2);
    for (argument, formal_address) in outer.arguments.iter().zip(params) {
        let RawTree::Leaf(term) = argument else {
            panic!("Pair arguments are direct formal references")
        };
        assert_eq!(term.tag, TERMREFDIRECT_TAG);
        let reference = term.ast_ref().unwrap();
        assert_eq!(reference.kind, AstRefKind::TermRefDirect);
        assert_eq!(reference.address, formal_address);
        assert!(matches!(term.value, TermValue::AstRef(address) if address == formal_address));
    }

    let pair_tasty = &pairs[1].1;
    let (constructor_address, constructor) = method_body(pair_tasty, "<init>");
    assert_eq!(constructor.parameters.len(), 2);
    assert_eq!(
        parameter_addresses(
            &pair_tasty.ast_address_index().unwrap(),
            constructor_address
        )
        .len(),
        2
    );
    let pair_dump =
        include_str!("fixtures/jvm_tasty_dual_reference_forwarding/oracle/Pair.tasty.txt");
    assert!(pair_dump.contains("VALDEF(14) 15 [left]"));
    assert!(pair_dump.contains("VALDEF(11) 16 [right]"));
}

#[test]
fn strict_pinned_scala_oracle_rebuild_and_consumer() {
    if std::env::var(REQUIRE_ORACLE).as_deref() != Ok("1") {
        return;
    }
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    let scala_root = std::env::var_os("SCALA39_ROOT").expect("pinned Scala 3.9 checkout");
    let java_home = std::env::var_os("JAVA_HOME").expect("Temurin JDK 21");
    let output = Command::new("bash")
        .arg(fixture.join("rebuild-oracle.sh"))
        .env("SCALA39_ROOT", scala_root)
        .env("JAVA_HOME", &java_home)
        .env(
            "DOTTY_ISSUE_1960_ORACLE_DIR",
            task_tmp().join("scala-oracle-rebuild"),
        )
        .status()
        .expect("run pinned Scala oracle rebuild");
    assert!(
        output.success(),
        "Scala 3.9 producer compile, consumer, and artifact comparisons pass"
    );
    let java_version = Command::new(Path::new(&java_home).join("bin/java"))
        .arg("-version")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&java_version.stdout),
        String::from_utf8_lossy(&java_version.stderr)
    );
    assert!(text.contains("21.0.13"), "{text}");
}

fn task_tmp() -> PathBuf {
    std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

#[test]
fn source_variants_record_first_reachable_paired_stage_without_forging_ids() {
    use dotty::jvm_tasty_plan::{PairedPlanError, with_paired_jvm_tasty_batch_plan};
    use dotty_core::SourceId;

    let source = include_str!("fixtures/jvm_tasty_dual_reference_forwarding/PairFactory.scala");
    let renamed = source
        .replace("package example", "package renamed")
        .replace("PairFactory", "Assembler")
        .replace("Pair", "Envelope")
        .replace("Counter", "Meter")
        .replace("wrap", "forward")
        .replace("first", "sourceLeft")
        .replace("second", "sourceRight");
    let reordered = source.replace(
        "class Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n\nclass Pair(first: Counter, second: Counter):\n  private val left: Counter = first\n  private val right: Counter = second\n  def leftValue(): Int = left.get()\n  def rightValue(): Int = right.get()\n\nclass PairFactory:",
        "class PairFactory:",
    );
    let reordered = reordered.replace("class PairFactory:\n  def wrap(first: Counter, second: Counter): Pair =\n    new Pair(first, second)\n", "class PairFactory:\n  def wrap(first: Counter, second: Counter): Pair =\n    new Pair(first, second)\n\nclass Pair(first: Counter, second: Counter):\n  private val left: Counter = first\n  private val right: Counter = second\n  def leftValue(): Int = left.get()\n  def rightValue(): Int = right.get()\n\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n");
    let cases = [
        ("renamed package/classes/method/formals", renamed, "cross-class constructor-call identity proof", "SemanticIdentity"),
        ("reordered declarations", reordered, "cross-class constructor-call identity proof", "SemanticIdentity"),
        ("intentional repeated first", source.replace("new Pair(first, second)", "new Pair(first, first)"), "cross-class constructor-call identity proof", "SemanticIdentity"),
        ("swapped arguments", source.replace("new Pair(first, second)", "new Pair(second, first)"), "cross-class constructor-call identity proof", "SemanticIdentity"),
        ("swapped formals", source.replace("def wrap(first: Counter, second: Counter)", "def wrap(second: Counter, first: Counter)"), "cross-class constructor-call identity proof", "SemanticIdentity"),
        ("third method formal", source.replace("second: Counter): Pair", "second: Counter, unused: Int): Pair"), "cross-class constructor-call identity proof", "SourceShape"),
        ("contextual formal", source.replace("def wrap(first: Counter, second: Counter)", "def wrap(first: Counter)(using second: Counter)"), "complete JVM unit validation", "ParameterCountMismatch"),
        ("default formal", source.replace("def wrap(first: Counter, second: Counter)", "def wrap(first: Counter, second: Counter = new Counter(0))"), "cross-class constructor-call identity proof", "SemanticIdentity"),
        ("generic method", source.replace("def wrap(", "def wrap[A]("), "member planning", "generic methods are unsupported"),
        ("erased formal used illegally", source.replace("def wrap(first: Counter, second: Counter)", "def wrap(first: Counter, erased second: Counter)"), "parsing", "ExpectedParameterColonAndType"),
        ("erased formal unused", source.replace("def wrap(first: Counter, second: Counter)", "def wrap(first: Counter, erased second: Counter)").replace("new Pair(first, second)", "new Pair(first, first)"), "parsing", "ExpectedParameterColonAndType"),
        ("overload", source.replace("new Pair(first, second)\n", "new Pair(first, second)\n  def wrap(value: Int, other: Int): Pair = new Pair(new Counter(value), new Counter(other))\n"), "cross-class constructor-call identity proof", "SemanticIdentity"),
        ("mixed direct and new", source.replace("new Pair(first, second)", "new Pair(first, new Counter(second.get()))"), "cross-class constructor-call identity proof", "SemanticIdentity"),
        ("wrong outer constructor", source.replace("class PairFactory:", "class PairChild(first: Counter, second: Counter) extends Pair(first, second):\n  def marker(): Int = 0\n\nclass PairFactory:").replace("new Pair(first, second)", "new PairChild(first, second)"), "source collection", "classes with explicit parents"),
        ("third outer constructor argument", source.replace("new Pair(first, second)", "new Pair(first, second, first)"), "method body typing", "ApplicationArityMismatch"),
        ("type mismatch", source.replace("def wrap(first: Counter, second: Counter)", "def wrap(first: Int, second: Counter)"), "method body typing", "ApplicationArgumentTypeMismatch"),
        ("wrong arity", source.replace("new Pair(first, second)", "new Pair(first)"), "method body typing", "ApplicationArityMismatch"),
        ("duplicate formal names", source.replace("def wrap(first: Counter, second: Counter)", "def wrap(first: Counter, first: Counter)"), "method body typing", "AmbiguousTermReference"),
        ("foreign same-spelled target", source.replace("package example\n", "package example\n\nobject foreign:\n  class Counter(start: Int):\n    def get(): Int = start\n\n").replace("def wrap(first: Counter, second: Counter)", "def wrap(first: foreign.Counter, second: Counter)"), "source collection", "top-level siblings must all be ordinary classes"),
    ];
    for (ordinal, (label, variant, expected_phase, expected_error)) in cases.into_iter().enumerate()
    {
        let mut callback_ran = false;
        let result = with_paired_jvm_tasty_batch_plan(
            &variant,
            "PairFactory.scala",
            SourceId::from_index(19620 + ordinal as u32),
            |_| callback_ran = true,
        );
        let error = match result {
            Ok(_) => panic!("expected {label} to fail at {expected_phase}"),
            Err(error) => error,
        };
        let (phase, reason) = match &error {
            PairedPlanError::BatchUnit { phase, reason }
            | PairedPlanError::BatchClass { phase, reason, .. } => (*phase, reason.as_str()),
            PairedPlanError::Parsing(diagnostic) => ("parsing", diagnostic.as_str()),
            other => panic!("{label}: expected a typed first-stage rejection, got {other:?}"),
        };
        assert_eq!(phase, expected_phase, "{label}: {error:?}");
        assert!(
            reason.contains(expected_error),
            "{label}: expected first failure containing {expected_error:?}, got {error:?}"
        );
        assert!(
            !callback_ran,
            "the unsupported/invalid control is never published: {label}"
        );
    }
}
