use dotty_classfile::access_flags::{ACC_FINAL, ACC_PRIVATE, ACC_PUBLIC};
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::reader::Reader;
use dotty_core::ast::{TreeKind, Untyped};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SourceTypedIndex, SymbolFlags,
    SymbolId, SymbolInfo, SymbolKind, TermRefTarget, Type, TypeRefTarget,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty::ast::{
    APPLY_TAG, DEFDEF_TAG, IDENTTPT_TAG, LOCAL_TAG, MUTABLE_TAG, NEW_TAG, PARAM_TAG, PRIVATE_TAG,
    QUALTHIS_TAG, SELECT_TAG, SELECTIN_TAG, SHAREDTYPE_TAG, TERMREFPKG_TAG, TERMREFSYMBOL_TAG,
    THIS_TAG, TYPEDEF_TAG, TYPEREF_TAG, TYPEREFSYMBOL_TAG, VALDEF_TAG,
};
use dotty_tasty::file::TastyFile;
use dotty_tasty::tasty::{DefinitionBody, DefinitionTail, RenderedParamSig, RenderedSignedName};
use dotty_tasty::term::{AstRefKind, RawTree};
use dotty_tasty_pickler::{
    ClassFieldPicklePlan, ClassMemberPicklePlan, ClassStatePicklePlan, PrimaryConstructorInputPlan,
    SourceMetadata, SourceTypedBindings,
};
use dotty_typer::SourceTyper;

use dotty::jvm_tasty_plan::{PairedPlanError, with_paired_jvm_tasty_batch_plan};

const SOURCE: &str = include_str!("fixtures/jvm_tasty_object_composition/CounterOwner.scala");
const REFERENCE_ALIAS_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_constructor_injection/CounterOwner.scala");
const COUNTER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_object_composition/scala39/example/Counter.class");
const COUNTER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_object_composition/scala39/example/Counter.tasty");
const OWNER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_object_composition/scala39/example/CounterOwner.class");
const OWNER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_object_composition/scala39/example/CounterOwner.tasty");
const ALIAS_COUNTER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_constructor_injection/scala39/example/Counter.class");
const ALIAS_COUNTER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_constructor_injection/scala39/example/Counter.tasty");
const ALIAS_OWNER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_constructor_injection/scala39/example/CounterOwner.class");
const ALIAS_OWNER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_constructor_injection/scala39/example/CounterOwner.tasty");

fn decode_class(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = Reader::new(bytes);
    let class = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    class
}

fn method<'a>(class: &'a ClassFile<'_>, name: &str) -> &'a dotty_classfile::method::MethodInfo<'a> {
    class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == name)
        .unwrap_or_else(|| panic!("class declares {name}"))
}

fn method_code<'a>(method: &'a dotty_classfile::method::MethodInfo<'a>) -> &'a [u8] {
    method
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Code(code) => Some(code.code),
            _ => None,
        })
        .expect("concrete method has Code")
}

fn field_ref_index(class: &ClassFile<'_>, owner: &str, name: &str, descriptor: &str) -> u16 {
    (1..=u16::MAX)
        .find_map(|raw_index| {
            let entry = class.constant_pool.get(ConstantPoolIndex(raw_index))?;
            let ConstantPoolEntry::Fieldref {
                class_index,
                name_and_type_index,
            } = entry
            else {
                return None;
            };
            let (member_name, member_descriptor) = class
                .constant_pool
                .name_and_type(*name_and_type_index)
                .ok()?;
            (class.constant_pool.class_name(*class_index).ok()? == owner
                && member_name == name
                && member_descriptor == descriptor)
                .then_some(raw_index)
        })
        .expect("matching Fieldref exists")
}

fn method_ref_index(class: &ClassFile<'_>, owner: &str, name: &str, descriptor: &str) -> u16 {
    (1..=u16::MAX)
        .find_map(|raw_index| {
            let entry = class.constant_pool.get(ConstantPoolIndex(raw_index))?;
            let ConstantPoolEntry::Methodref {
                class_index,
                name_and_type_index,
            } = entry
            else {
                return None;
            };
            let (member_name, member_descriptor) = class
                .constant_pool
                .name_and_type(*name_and_type_index)
                .ok()?;
            (class.constant_pool.class_name(*class_index).ok()? == owner
                && member_name == name
                && member_descriptor == descriptor)
                .then_some(raw_index)
        })
        .expect("matching Methodref exists")
}

fn class_index(class: &ClassFile<'_>, name: &str) -> u16 {
    (1..=u16::MAX)
        .find(|&raw_index| {
            matches!(
                class.constant_pool.get(ConstantPoolIndex(raw_index)),
                Some(ConstantPoolEntry::Class { name_index })
                    if class.constant_pool.utf8(*name_index).ok() == Some(name)
            )
        })
        .expect("matching Class entry exists")
}

fn assert_tasty_link<'a>(class: &ClassFile<'_>, bytes: &'a [u8]) -> TastyFile<'a> {
    let tasty = TastyFile::parse_and_validate_scala_3_9(bytes).unwrap();
    assert_eq!(
        (
            tasty.header().major_version,
            tasty.header().minor_version,
            tasty.header().experimental_version,
        ),
        (28, 9, 0)
    );
    assert_eq!(
        class.tasty_link_id().unwrap().unwrap().as_bytes(),
        &tasty.header().uuid
    );
    tasty
}

struct PlannedBindings<'a> {
    source_typed: &'a SourceTypedIndex,
    classes: &'a [SymbolId],
    methods: &'a [SymbolId],
}

impl SourceTypedBindings for PlannedBindings<'_> {
    fn typed_at(
        &self,
        source: SourceId,
        tree: dotty_core::TreeId<Untyped>,
    ) -> Option<dotty_core::TreeId<dotty_core::Typed>> {
        self.source_typed.get(source, tree)
    }

    fn planned_class_symbols(&self) -> &[SymbolId] {
        self.classes
    }

    fn planned_method_symbols(&self) -> &[SymbolId] {
        self.methods
    }
}

#[test]
fn scala_39_oracle_pins_private_reference_val_tasty_and_jvm_abi() {
    let counter = decode_class(COUNTER_CLASS);
    let owner = decode_class(OWNER_CLASS);
    assert_eq!((counter.version.major, owner.version.major), (65, 65));
    assert_eq!(
        counter.constant_pool.class_name(counter.this_class),
        Ok("example/Counter")
    );
    let [current] = counter.fields.as_slice() else {
        panic!("Counter has exactly one mutable state field");
    };
    assert_eq!(
        counter.constant_pool.utf8(current.name_index),
        Ok("current")
    );
    assert_eq!(
        counter.constant_pool.utf8(current.descriptor_index),
        Ok("I")
    );
    assert!(current.access_flags.contains(ACC_PRIVATE));
    assert!(!current.access_flags.contains(ACC_FINAL));
    let counter_signatures = counter
        .methods
        .iter()
        .map(|method| {
            (
                counter.constant_pool.utf8(method.name_index).unwrap(),
                counter.constant_pool.utf8(method.descriptor_index).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        counter_signatures,
        [("<init>", "(I)V"), ("get", "()I"), ("set", "(I)V")]
    );
    assert_eq!(
        owner.constant_pool.class_name(owner.this_class),
        Ok("example/CounterOwner")
    );

    let [inner] = owner.fields.as_slice() else {
        panic!("CounterOwner has exactly one physical field");
    };
    assert_eq!(owner.constant_pool.utf8(inner.name_index), Ok("inner"));
    assert_eq!(
        owner.constant_pool.utf8(inner.descriptor_index),
        Ok("Lexample/Counter;")
    );
    assert!(inner.access_flags.contains(ACC_PRIVATE | ACC_FINAL));
    assert!(!inner.access_flags.contains(ACC_PUBLIC));

    let signatures = owner
        .methods
        .iter()
        .map(|method| {
            (
                owner.constant_pool.utf8(method.name_index).unwrap(),
                owner.constant_pool.utf8(method.descriptor_index).unwrap(),
                method.access_flags,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        signatures
            .iter()
            .map(|(name, descriptor, _)| (*name, *descriptor))
            .collect::<Vec<_>>(),
        [("<init>", "(I)V"), ("read", "()I")]
    );
    assert!(
        signatures
            .iter()
            .all(|(_, _, flags)| flags.contains(ACC_PUBLIC))
    );

    let constructor = method(&owner, "<init>");
    assert_eq!(
        owner.constant_pool.utf8(constructor.descriptor_index),
        Ok("(I)V")
    );
    let counter_init = method_ref_index(&owner, "example/Counter", "<init>", "(I)V");
    let super_init = method_ref_index(&owner, "java/lang/Object", "<init>", "()V");
    let counter_class = class_index(&owner, "example/Counter");
    let inner_ref = field_ref_index(&owner, "example/CounterOwner", "inner", "Lexample/Counter;");
    assert_eq!(
        method_code(constructor),
        [
            0x2a,
            0xb7,
            (super_init >> 8) as u8,
            super_init as u8,
            0x2a,
            0xbb,
            (counter_class >> 8) as u8,
            counter_class as u8,
            0x59,
            0x1b,
            0xb7,
            (counter_init >> 8) as u8,
            counter_init as u8,
            0xb5,
            (inner_ref >> 8) as u8,
            inner_ref as u8,
            0xb1,
        ]
    );
    let read = method(&owner, "read");
    assert_eq!(owner.constant_pool.utf8(read.descriptor_index), Ok("()I"));
    let get_ref = method_ref_index(&owner, "example/Counter", "get", "()I");
    assert_eq!(
        method_code(read),
        [
            0x2a,
            0xb4,
            (inner_ref >> 8) as u8,
            inner_ref as u8,
            0xb6,
            (get_ref >> 8) as u8,
            get_ref as u8,
            0xac
        ]
    );

    let counter_tasty = assert_tasty_link(&counter, COUNTER_TASTY);
    let owner_tasty = assert_tasty_link(&owner, OWNER_TASTY);
    let index = owner_tasty.ast_address_index().unwrap();
    let field = index
        .iter_nodes_with_tag(VALDEF_TAG)
        .find_map(|node| {
            let node = index.get(node.offset as u32)?;
            let DefinitionBody::ValDef {
                name,
                type_tree,
                rhs,
                tail,
            } = node.decode_definition_body().ok()?
            else {
                return None;
            };
            (owner_tasty.names().render(name).ok()? == "inner").then_some((
                node.offset as u32,
                type_tree,
                rhs,
                tail,
            ))
        })
        .expect("CounterOwner has the source inner VALDEF");
    let (field_address, field_type, initializer, tail) = field;
    assert_eq!(field_address, 44);
    assert!(
        tail.iter()
            .any(|modifier| matches!(modifier, DefinitionTail::Modifier(PRIVATE_TAG)))
    );
    assert!(
        tail.iter()
            .any(|modifier| matches!(modifier, DefinitionTail::Modifier(LOCAL_TAG))),
        "TASTy carries LOCAL"
    );
    assert!(
        !tail
            .iter()
            .any(|modifier| matches!(modifier, DefinitionTail::Modifier(MUTABLE_TAG))),
        "the source val is immutable"
    );
    assert_eq!(
        field_type
            .nodes()
            .iter()
            .map(|node| node.tag)
            .collect::<Vec<_>>(),
        [IDENTTPT_TAG, TYPEREF_TAG, TERMREFPKG_TAG]
    );
    assert_eq!(
        field_type
            .name_refs()
            .into_iter()
            .map(|name| owner_tasty.names().render(name).unwrap())
            .collect::<Vec<_>>(),
        ["Counter", "example"]
    );
    let initializer = initializer.expect("private val inner has an initializer");
    let RawTree::LengthNode(apply_node) = initializer else {
        panic!("field initializer is an APPLY node");
    };
    assert_eq!(apply_node.tag, APPLY_TAG);
    let apply = apply_node.decode_apply().unwrap();
    let RawTree::LengthNode(select_node) = apply.function else {
        panic!("initializer applies a selected constructor");
    };
    assert_eq!(select_node.tag, SELECTIN_TAG);
    let select = select_node.decode_select_in().unwrap();
    let RawTree::Ast {
        tag: NEW_TAG,
        child,
        ..
    } = select.qualifier
    else {
        panic!("constructor selection receiver is NEW");
    };
    assert_eq!(
        child
            .nodes()
            .iter()
            .map(|node| node.tag)
            .collect::<Vec<_>>(),
        [IDENTTPT_TAG, SHAREDTYPE_TAG]
    );
    assert_eq!(
        select
            .underlying_type
            .nodes()
            .iter()
            .map(|node| node.tag)
            .collect::<Vec<_>>(),
        [SHAREDTYPE_TAG]
    );
    let signature = owner_tasty
        .names()
        .render_signed_name(select.name)
        .unwrap()
        .unwrap();
    assert!(
        matches!(signature, RenderedSignedName::Signed { original, signature }
        if original == "<init>" && signature.result == "example.Counter"
            && signature.parameters == [RenderedParamSig::TermParameter("scala.Int".to_owned())])
    );
    assert_eq!(
        child
            .name_refs()
            .into_iter()
            .map(|name| owner_tasty.names().render(name).unwrap())
            .collect::<Vec<_>>(),
        ["Counter"]
    );
    let new_type_references = child.ast_refs();
    let [field_type_reference] = new_type_references.as_slice() else {
        panic!("Counter's New tpt has one shared-type reference to the field type");
    };
    let counter_type_address = field_type_reference.address;
    assert_eq!(counter_type_address, 49);
    assert_eq!(
        new_type_references,
        [dotty_tasty::term::AstRef {
            kind: AstRefKind::SharedType,
            address: counter_type_address,
        }]
    );
    assert_eq!(
        select.underlying_type.ast_refs(),
        [dotty_tasty::term::AstRef {
            kind: AstRefKind::SharedType,
            address: counter_type_address,
        }]
    );
    assert_eq!(apply.arguments.len(), 1);
    let seed_reference = apply.arguments[0].decode_reference().unwrap();
    assert_eq!(seed_reference.tag, TERMREFSYMBOL_TAG);
    assert_eq!(seed_reference.reference, 9);
    let RawTree::Ast {
        tag: THIS_TAG,
        child: seed_prefix,
        ..
    } = seed_reference.qualifier
    else {
        panic!("seed term reference is qualified by THIS");
    };
    let seed_prefix = seed_prefix.decode_reference().unwrap();
    assert_eq!(seed_prefix.tag, TYPEREFSYMBOL_TAG);
    assert_eq!(seed_prefix.reference, 4);
    assert_eq!(
        seed_prefix.qualifier.ast_refs(),
        [dotty_tasty::term::AstRef {
            kind: AstRefKind::SharedType,
            address: 51,
        }]
    );
    let constructor_address = index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let body = index.get(node.offset as u32)?.decode_defdef_body().ok()?;
            (owner_tasty.names().render(body.name).ok()? == "<init>").then_some(node.offset as u32)
        })
        .expect("CounterOwner primary constructor is present");
    let constructor_body = index
        .get(constructor_address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    assert_eq!(constructor_body.parameters.len(), 1);
    assert_eq!(constructor_body.parameters[0].tag(), PARAM_TAG);
    assert_eq!(
        owner_tasty
            .names()
            .render(constructor_body.parameters[0].name())
            .unwrap(),
        "seed"
    );
    let seed_declaration_addresses = index
        .iter_nodes_with_tag(PARAM_TAG)
        .filter_map(|node| {
            let node = index.get(node.offset as u32)?;
            let parameter = node.decode_parameter().ok()?;
            (owner_tasty.names().render(parameter.name()).ok()? == "seed")
                .then_some(node.offset as u32)
        })
        .collect::<Vec<_>>();
    assert_eq!(seed_declaration_addresses, [9, 33]);
    assert!(index.get(field_address).is_some());
    let class_names = index
        .iter_nodes_with_tag(TYPEDEF_TAG)
        .filter_map(|node| {
            match index
                .get(node.offset as u32)?
                .decode_definition_body()
                .ok()?
            {
                DefinitionBody::TypeDef { name, .. } => owner_tasty.names().render(name).ok(),
                _ => None,
            }
        })
        .collect::<Vec<_>>();
    assert!(class_names.contains(&"CounterOwner".to_owned()));
    let (read_address, read_body) = index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let body = index.get(node.offset as u32)?.decode_defdef_body().ok()?;
            (owner_tasty.names().render(body.name).ok()? == "read")
                .then_some((node.offset as u32, body))
        })
        .expect("CounterOwner sidecar contains read");
    assert_eq!(read_address, 74);
    let RawTree::LengthNode(read_apply_node) = read_body.rhs.unwrap() else {
        panic!("read body is an APPLY");
    };
    assert_eq!(read_apply_node.tag, APPLY_TAG);
    let read_apply = read_apply_node.decode_apply().unwrap();
    assert!(read_apply.arguments.is_empty());
    let RawTree::LengthNode(read_select_node) = read_apply.function else {
        panic!("read applies a selected Counter method");
    };
    assert_eq!(read_select_node.tag, SELECTIN_TAG);
    let read_select = read_select_node.decode_select_in().unwrap();
    assert!(
        matches!(owner_tasty.names().render_signed_name(read_select.name).unwrap(),
        Some(RenderedSignedName::Signed { original, signature })
            if original == "get" && signature.result == "scala.Int" && signature.parameters.is_empty())
    );
    let inner_select = read_select.qualifier.decode_select().unwrap();
    assert_eq!(inner_select.tag, SELECT_TAG);
    assert_eq!(
        owner_tasty.names().render(inner_select.name).unwrap(),
        "inner"
    );
    assert!(matches!(
        inner_select.qualifier,
        RawTree::Ast {
            tag: QUALTHIS_TAG,
            ..
        }
    ));
    let counter_index = counter_tasty.ast_address_index().unwrap();
    assert!(
        counter_index.iter_nodes_with_tag(DEFDEF_TAG).any(|node| {
            counter_index
                .get(node.offset as u32)
                .unwrap()
                .decode_defdef_body()
                .is_ok_and(|body| {
                    counter_tasty.names().render(body.name).ok().as_deref() == Some("get")
                })
        }),
        "Counter.get is declared in the separate Counter sidecar"
    );
    assert_eq!(counter_tasty.header().major_version, 28);
}

#[test]
fn scala_39_oracle_pins_constructor_reference_alias_tasty_and_jvm_abi() {
    let counter = decode_class(ALIAS_COUNTER_CLASS);
    let owner = decode_class(ALIAS_OWNER_CLASS);
    assert_eq!((counter.version.major, owner.version.major), (65, 65));
    assert_eq!(
        counter.constant_pool.class_name(counter.this_class),
        Ok("example/Counter")
    );
    assert_eq!(
        owner.constant_pool.class_name(owner.this_class),
        Ok("example/CounterOwner")
    );
    let [inner] = owner.fields.as_slice() else {
        panic!("CounterOwner has exactly one physical field");
    };
    assert_eq!(owner.constant_pool.utf8(inner.name_index), Ok("inner"));
    assert_eq!(
        owner.constant_pool.utf8(inner.descriptor_index),
        Ok("Lexample/Counter;")
    );
    assert!(inner.access_flags.contains(ACC_PRIVATE | ACC_FINAL));
    let constructor = method(&owner, "<init>");
    assert_eq!(
        owner.constant_pool.utf8(constructor.descriptor_index),
        Ok("(Lexample/Counter;)V")
    );
    let super_init = method_ref_index(&owner, "java/lang/Object", "<init>", "()V");
    let inner_ref = field_ref_index(&owner, "example/CounterOwner", "inner", "Lexample/Counter;");
    assert_eq!(
        method_code(constructor),
        [
            0x2a,
            0xb7,
            (super_init >> 8) as u8,
            super_init as u8,
            0x2a,
            0x2b,
            0xb5,
            (inner_ref >> 8) as u8,
            inner_ref as u8,
            0xb1,
        ],
        "Owner stores the incoming Counter after direct super initialization, with no allocation"
    );
    let read = method(&owner, "read");
    let get_ref = method_ref_index(&owner, "example/Counter", "get", "()I");
    assert_eq!(
        method_code(read),
        [
            0x2a,
            0xb4,
            (inner_ref >> 8) as u8,
            inner_ref as u8,
            0xb6,
            (get_ref >> 8) as u8,
            get_ref as u8,
            0xac,
        ]
    );

    let counter_tasty = assert_tasty_link(&counter, ALIAS_COUNTER_TASTY);
    let owner_tasty = assert_tasty_link(&owner, ALIAS_OWNER_TASTY);
    assert_eq!(
        (
            counter_tasty.header().major_version,
            counter_tasty.header().minor_version
        ),
        (28, 9)
    );
    assert_eq!(
        (
            owner_tasty.header().major_version,
            owner_tasty.header().minor_version
        ),
        (28, 9)
    );
    let index = owner_tasty.ast_address_index().unwrap();
    let counter_parameters = index
        .iter_nodes_with_tag(PARAM_TAG)
        .filter_map(|node| {
            let address = node.offset as u32;
            let parameter = index.get(address)?.decode_parameter().ok()?;
            (owner_tasty.names().render(parameter.name()).ok()? == "counter")
                .then_some((address, parameter))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        counter_parameters
            .iter()
            .map(|(address, _)| *address)
            .collect::<Vec<_>>(),
        [9, 33],
        "template and constructor parameters are separate PARAM declarations"
    );
    let template_parameter = counter_parameters[0].1.decode_body().unwrap();
    assert_eq!(
        template_parameter
            .type_tree
            .nodes()
            .iter()
            .map(|node| node.tag)
            .collect::<Vec<_>>(),
        [TYPEREF_TAG, TERMREFPKG_TAG]
    );
    assert_eq!(
        template_parameter
            .type_tree
            .name_refs()
            .into_iter()
            .map(|name| owner_tasty.names().render(name).unwrap())
            .collect::<Vec<_>>(),
        ["example"]
    );
    assert!(
        template_parameter
            .tail
            .contains(&DefinitionTail::Modifier(PRIVATE_TAG))
    );
    assert!(
        template_parameter
            .tail
            .contains(&DefinitionTail::Modifier(LOCAL_TAG))
    );
    let constructor_parameter = counter_parameters[1].1.decode_body().unwrap();
    assert_eq!(constructor_parameter.type_tree.nodes()[0].tag, IDENTTPT_TAG);
    assert_eq!(
        constructor_parameter.type_tree.ast_refs(),
        [dotty_tasty::term::AstRef {
            kind: AstRefKind::SharedType,
            address: 12,
        }]
    );
    let field = index
        .iter_nodes_with_tag(VALDEF_TAG)
        .find_map(|node| {
            let node = index.get(node.offset as u32)?;
            let DefinitionBody::ValDef {
                name,
                type_tree,
                rhs,
                tail,
            } = node.decode_definition_body().ok()?
            else {
                return None;
            };
            (owner_tasty.names().render(name).ok()? == "inner").then_some((
                node.offset as u32,
                type_tree,
                rhs,
                tail,
            ))
        })
        .expect("CounterOwner has the original inner VALDEF");
    let (field_address, field_type, initializer, tail) = field;
    assert_eq!(field_address, 44);
    assert_eq!(
        field_type
            .name_refs()
            .into_iter()
            .map(|name| owner_tasty.names().render(name).unwrap())
            .collect::<Vec<_>>(),
        ["Counter"]
    );
    assert!(
        tail.iter()
            .any(|modifier| matches!(modifier, DefinitionTail::Modifier(PRIVATE_TAG)))
    );
    assert!(
        tail.iter()
            .any(|modifier| matches!(modifier, DefinitionTail::Modifier(LOCAL_TAG)))
    );
    assert!(
        !tail
            .iter()
            .any(|modifier| matches!(modifier, DefinitionTail::Modifier(MUTABLE_TAG)))
    );
    let reference = initializer.unwrap().decode_reference().unwrap();
    assert_eq!(reference.tag, TERMREFSYMBOL_TAG);
    assert_eq!(
        reference.reference, 9,
        "field RHS targets the template PARAM"
    );
    let RawTree::Ast {
        tag: THIS_TAG,
        child: prefix,
        ..
    } = reference.qualifier
    else {
        panic!("field alias reference is qualified by this");
    };
    let prefix = prefix.decode_reference().unwrap();
    assert_eq!(prefix.tag, TYPEREFSYMBOL_TAG);
    assert_eq!(prefix.reference, 4, "this prefix is the CounterOwner class");
    assert_eq!(counter_tasty.header().major_version, 28);
    assert_eq!(owner_tasty.header().minor_version, 9);
}

#[test]
fn paired_producer_accepts_reference_field_receiver_cross_contract() {
    with_paired_jvm_tasty_batch_plan(
        SOURCE,
        "CounterOwner.scala",
        SourceId::from_index(1715),
        |batch| assert_eq!(batch.classes.len(), 2),
    )
    .expect("the validated field receiver TASTy shape passes the paired batch contract");
}

#[test]
fn constructor_reference_alias_preserves_source_identity_and_emits_source_tasty() {
    let source_id = SourceId::from_index(1751);
    let source = SourceText::new(REFERENCE_ALIAS_SOURCE).unwrap();
    let scanner = ContextualScanner::new(REFERENCE_ALIAS_SOURCE).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty());
    let mut packages = Packages::new();
    let source_index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "CounterOwner.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        other => panic!("constructor-alias source names without diagnostics: {other:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("fixture has one package root");
    };
    assert_eq!(package.stats.len(), 2);

    let mut classes = Vec::new();
    let mut owner_parts = None;
    let mut counter_parts = None;
    for &class_tree in &package.stats {
        let TreeKind::TypeDef(class_definition) = &parsed.ast.get(class_tree).kind else {
            panic!("fixture top-level declarations are classes");
        };
        let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
            panic!("class has a template");
        };
        let class = source_index.symbol_at(source_id, class_tree).unwrap();
        let constructor_tree = template.constructor;
        let constructor = source_index.symbol_at(source_id, constructor_tree).unwrap();
        let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(constructor_tree).kind
        else {
            panic!("class has a primary constructor");
        };
        let [parameter_tree] = constructor_definition.value_param_clauses[0].as_slice() else {
            panic!("class has one plain constructor parameter");
        };
        let template_parameter = source_index.symbol_at(source_id, *parameter_tree).unwrap();
        let constructor_parameter = source_index
            .derived_symbol_at(constructor, source_id, *parameter_tree)
            .unwrap();
        assert_ne!(template_parameter, constructor_parameter);
        assert_eq!(
            store.symbols.get(template_parameter).kind,
            SymbolKind::Parameter
        );
        assert_eq!(store.symbols.get(template_parameter).owner, Some(class));
        assert_eq!(
            store.symbols.get(constructor_parameter).kind,
            SymbolKind::Parameter
        );
        assert_eq!(
            store.symbols.get(constructor_parameter).owner,
            Some(constructor)
        );
        classes.push(class);

        let class_name = store
            .names
            .resolve(class_definition.name.as_name().text())
            .to_owned();
        if class_name == "CounterOwner" {
            let field_tree = template
                .body
                .iter()
                .copied()
                .find(|tree| matches!(parsed.ast.get(*tree).kind, TreeKind::ValDef(_)))
                .unwrap();
            let TreeKind::ValDef(field_definition) = &parsed.ast.get(field_tree).kind else {
                unreachable!();
            };
            let read_tree = template
                .body
                .iter()
                .copied()
                .find(|tree| {
                    matches!(&parsed.ast.get(*tree).kind, TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "read")
                })
                .unwrap();
            owner_parts = Some((
                class,
                constructor,
                *parameter_tree,
                template_parameter,
                constructor_parameter,
                field_tree,
                field_definition.rhs.unwrap(),
                read_tree,
            ));
        } else {
            let get_tree = template
                .body
                .iter()
                .copied()
                .find(|tree| {
                    matches!(&parsed.ast.get(*tree).kind, TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "get")
                })
                .unwrap();
            counter_parts = Some((class, constructor, constructor_parameter, get_tree));
        }
    }
    let (
        owner_class,
        owner_constructor,
        owner_parameter_tree,
        owner_template_parameter,
        owner_constructor_parameter,
        inner_tree,
        inner_rhs,
        read_tree,
    ) = owner_parts.unwrap();
    let (counter_class, counter_constructor, counter_parameter, get_tree) = counter_parts.unwrap();

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    for symbol in [
        owner_class,
        owner_constructor,
        owner_template_parameter,
        owner_constructor_parameter,
        counter_class,
        counter_constructor,
        counter_parameter,
    ] {
        typer.complete_symbol(symbol).unwrap();
    }
    for parameter in [owner_template_parameter, owner_constructor_parameter] {
        let SymbolInfo::Complete(parameter_type) = typer.store().symbols.get(parameter).info else {
            panic!("both source parameter identities have completed types");
        };
        assert!(matches!(
            typer.store().types.try_get(parameter_type),
            Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. })
                if *symbol == counter_class
        ));
    }
    let inner = source_index.symbol_at(source_id, inner_tree).unwrap();
    typer.complete_symbol(inner).unwrap();
    let SymbolInfo::Complete(inner_type) = typer.store().symbols.get(inner).info else {
        panic!("inner has a completed declared type");
    };
    assert_eq!(typer.store().symbols.get(inner).kind, SymbolKind::Field);
    assert_eq!(typer.store().symbols.get(inner).owner, Some(owner_class));
    assert!(
        !typer
            .store()
            .symbols
            .get(inner)
            .flags
            .contains(SymbolFlags::MUTABLE)
    );
    assert!(matches!(
        typer.store().types.try_get(inner_type),
        Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. })
            if *symbol == counter_class
    ));
    let field_context = typer.field_initializer_context_for(inner).unwrap();
    let inner_typed = typer
        .type_expression_expected(inner_rhs, field_context, inner_type)
        .unwrap();
    let TreeKind::ValDef(field_definition) = &parsed.ast.get(inner_tree).kind else {
        unreachable!();
    };
    assert_eq!(
        field_definition.metadata.visibility,
        Some(dotty_core::ast::VisibilitySyntax::Private { qualifier: None })
    );
    assert_eq!(
        typer.source_typed_index().get(source_id, inner_rhs),
        Some(inner_typed)
    );
    let Some(Type::TermRef { prefix, target }) = typer
        .store()
        .types
        .try_get(typer.typed_ast().get(inner_typed).ty)
    else {
        panic!("typed alias RHS has a TermRef type");
    };
    assert_eq!(*prefix, definitions.no_prefix);
    assert_eq!(*target, TermRefTarget::Symbol(owner_constructor_parameter));
    assert!(matches!(
        typer.typed_ast().get(inner_typed).kind,
        TreeKind::Ident(_)
    ));

    let counter_get = source_index.symbol_at(source_id, get_tree).unwrap();
    typer.complete_symbol(counter_get).unwrap();
    let read = source_index.symbol_at(source_id, read_tree).unwrap();
    typer.complete_symbol(read).unwrap();
    let TreeKind::DefDef(read_definition) = &parsed.ast.get(read_tree).kind else {
        unreachable!();
    };
    let read_rhs = read_definition.rhs.unwrap();
    let read_context = typer.expression_context_for(read).unwrap();
    let read_typed = typer.type_expression(read_rhs, read_context).unwrap();
    let TreeKind::Apply(read_apply) = &typer.typed_ast().get(read_typed).kind else {
        panic!("read is a call on inner.get()");
    };
    let TreeKind::Select(read_select) = &typer.typed_ast().get(read_apply.function).kind else {
        panic!("read selects Counter.get");
    };
    assert!(matches!(
        typer.store().types.try_get(typer.typed_ast().get(read_select.qualifier).ty),
        Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. })
            if *symbol == inner
    ));
    assert!(matches!(
        typer.store().types.try_get(typer.typed_ast().get(read_apply.function).ty),
        Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. })
            if *symbol == counter_get
    ));
    assert_eq!(
        typer.source_typed_index().get(source_id, read_rhs),
        Some(read_typed)
    );

    let owner_members = [
        ClassMemberPicklePlan::Field(ClassFieldPicklePlan {
            symbol: inner,
            source_tree: inner_tree,
            typed_initializer: inner_typed,
            declared_type: inner_type,
        }),
        ClassMemberPicklePlan::Method {
            symbol: read,
            source_tree: read_tree,
        },
    ];
    let methods = [read, counter_get];
    let planned = PlannedBindings {
        source_typed: typer.source_typed_index(),
        classes: &classes,
        methods: &methods,
    };
    let class_plan = ClassStatePicklePlan {
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        typed_bindings: &planned,
        source: SourceMetadata {
            id: source_id,
            name: "CounterOwner.scala".to_owned(),
        },
        class: owner_class,
        class_tree: match &parsed.ast.get(parsed.root).kind {
            TreeKind::PackageDef(package) => package.stats[1],
            _ => unreachable!(),
        },
        constructor: PrimaryConstructorInputPlan {
            symbol: owner_constructor,
            source_tree: source_index
                .definition_of(owner_constructor)
                .and_then(|definition| match definition {
                    dotty_core::SourceDefinition::Canonical { source, tree }
                        if source == source_id =>
                    {
                        Some(tree)
                    }
                    _ => None,
                })
                .unwrap(),
            parameter_clauses: vec![vec![owner_constructor_parameter]],
        },
        members: &owner_members,
    };
    let validated = class_plan
        .validate()
        .expect("the source-bound immutable reference alias is supported");
    let reference_fields = validated.same_batch_reference_fields().collect::<Vec<_>>();
    let [reference_field] = reference_fields.as_slice() else {
        panic!("one immutable Counter alias is proved");
    };
    assert_eq!(
        reference_field.template_parameter_alias(),
        Some(owner_template_parameter),
        "the proof maps the typed constructor formal to the source-indexed class parameter"
    );
    let emitted = validated.pickle().unwrap();
    let tasty = TastyFile::parse_and_validate_scala_3_9(emitted.bytes()).unwrap();
    let tasty_index = tasty.ast_address_index().unwrap();
    let alias = tasty_index
        .iter_nodes_with_tag(VALDEF_TAG)
        .find_map(|node| {
            let body = tasty_index
                .get(node.offset as u32)?
                .decode_definition_body()
                .ok()?;
            let DefinitionBody::ValDef { name, rhs, .. } = body else {
                return None;
            };
            (tasty.names().render(name).ok()? == "inner").then_some(rhs)
        })
        .expect("the emitted field declaration is present")
        .expect("the alias has a source initializer")
        .decode_reference()
        .unwrap();
    assert_eq!(alias.tag, TERMREFSYMBOL_TAG);
    assert_eq!(alias.reference, 9);
    assert!(matches!(
        alias.qualifier,
        RawTree::Ast { tag: THIS_TAG, .. }
    ));

    let wrong_derived_formal = match (ClassStatePicklePlan {
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        typed_bindings: &planned,
        source: SourceMetadata {
            id: source_id,
            name: "CounterOwner.scala".to_owned(),
        },
        class: owner_class,
        class_tree: class_plan.class_tree,
        constructor: PrimaryConstructorInputPlan {
            symbol: owner_constructor,
            source_tree: class_plan.constructor.source_tree,
            parameter_clauses: vec![vec![owner_template_parameter]],
        },
        members: &owner_members,
    })
    .validate()
    {
        Ok(_) => panic!("a class-owned template PARAM cannot stand in for the ctor formal"),
        Err(error) => error,
    };
    assert!(matches!(
        wrong_derived_formal,
        dotty_tasty_pickler::PickleError::ScopeMembers { owner }
            if owner == owner_constructor
    ));

    let paired = with_paired_jvm_tasty_batch_plan(
        REFERENCE_ALIAS_SOURCE,
        "CounterOwner.scala",
        SourceId::from_index(1752),
        |batch| batch.classes[1].tasty_plan.pickle().unwrap(),
    )
    .expect("paired source/TASTy planning accepts the exact alias fixture");
    assert_eq!(paired.bytes(), emitted.bytes());
    assert_eq!(
        owner_parameter_tree,
        source_index
            .definition_of(owner_template_parameter)
            .and_then(|definition| match definition {
                dotty_core::SourceDefinition::Canonical { source, tree } if source == source_id =>
                    Some(tree),
                _ => None,
            })
            .unwrap()
    );
}

#[test]
fn constructor_injection_negative_matrix_pins_first_batch_rejection() {
    struct Case {
        name: &'static str,
        source: String,
        phase: &'static str,
        reason_prefix: &'static str,
    }
    let variants = [
        Case {
            name: "constructor type mismatch",
            source: REFERENCE_ALIAS_SOURCE
                .replace("class CounterOwner(counter: Counter)", "class CounterOwner(counter: Int)"),
            phase: "field initializer typing",
            reason_prefix: "ExpectedExpressionTypeMismatch",
        },
        Case {
            name: "wrong same-package reference class",
            source: REFERENCE_ALIAS_SOURCE.replace(
                "class CounterOwner(counter: Counter)",
                "class OtherCounter(start: Int)\n\nclass CounterOwner(counter: OtherCounter)",
            ),
            phase: "field initializer typing",
            reason_prefix: "ExpectedExpressionTypeMismatch",
        },
        Case {
            name: "unplanned reference class",
            source: REFERENCE_ALIAS_SOURCE.replace(
                "class Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value",
                "class Other",
            ),
            phase: "constructor typing",
            reason_prefix: "TypeNameNotFound",
        },
        Case {
            name: "external reference class",
            source: REFERENCE_ALIAS_SOURCE.replace(
                "class CounterOwner(counter: Counter)",
                "class CounterOwner(counter: java.lang.String)",
            ),
            phase: "constructor typing",
            reason_prefix: "TypeNameNotFound",
        },
        Case {
            name: "wrong package reference class",
            source: REFERENCE_ALIAS_SOURCE.replace(
                "class CounterOwner(counter: Counter)",
                "class CounterOwner(counter: foreign.Counter)",
            ),
            phase: "constructor typing",
            reason_prefix: "TypeNameNotFound",
        },
        Case {
            name: "same-spelled foreign package class",
            source: r#"package example {
class Counter(start: Int):
  private var current: Int = start
  def get(): Int = current
  def set(value: Int): Unit = current = value
package foreign { class Counter(start: Int) }
class CounterOwner(counter: foreign.Counter):
  private val inner: Counter = counter
  def read(): Int = inner.get()
}"#
                .to_owned(),
            phase: "source collection",
            reason_prefix: "top-level siblings must all be ordinary classes",
        },
        Case {
            name: "private reference class",
            source: REFERENCE_ALIAS_SOURCE.replace(
                "class Counter(start: Int):",
                "private class Counter(start: Int):",
            ),
            phase: "complete JVM unit validation",
            reason_prefix: "UnsupportedJvmVisibility",
        },
        Case {
            name: "abstract reference class",
            source: REFERENCE_ALIAS_SOURCE.replace(
                "class Counter(start: Int):",
                "abstract class Counter(start: Int):",
            ),
            phase: "complete JVM unit validation",
            reason_prefix: "UnsupportedJvmClass",
        },
        Case {
            name: "trait reference class",
            source: REFERENCE_ALIAS_SOURCE.replace("class Counter(start: Int):", "trait Counter:"),
            phase: "source collection",
            reason_prefix: "top-level declaration is not an ordinary class",
        },
        Case {
            name: "private[this] alias field",
            source: REFERENCE_ALIAS_SOURCE
                .replace("private val inner: Counter", "private[this] val inner: Counter"),
            phase: "member planning",
            reason_prefix: "only unqualified private mutable fields and initialized private immutable same-batch reference fields are supported",
        },
        Case {
            name: "mutable alias field",
            source: REFERENCE_ALIAS_SOURCE
                .replace("private val inner: Counter", "private var inner: Counter"),
            phase: "method body typing",
            reason_prefix: "UnstableSelectionPrefix",
        },
        Case {
            name: "constructor val accessor",
            source: REFERENCE_ALIAS_SOURCE.replace(
                "class CounterOwner(counter: Counter)",
                "class CounterOwner(val counter: Counter)",
            ),
            phase: "complete JVM unit validation",
            reason_prefix: "InvalidPrimaryConstructor",
        },
        Case {
            name: "constructor var accessor",
            source: REFERENCE_ALIAS_SOURCE.replace(
                "class CounterOwner(counter: Counter)",
                "class CounterOwner(var counter: Counter)",
            ),
            phase: "complete JVM unit validation",
            reason_prefix: "InvalidPrimaryConstructor",
        },
        Case {
            name: "generic constructor",
            source: REFERENCE_ALIAS_SOURCE.replace(
                "class CounterOwner(counter: Counter)",
                "class CounterOwner[A](counter: Counter)",
            ),
            phase: "source collection",
            reason_prefix: "generic primary constructors are unsupported",
        },
        Case {
            name: "defaulted constructor formal",
            source: REFERENCE_ALIAS_SOURCE.replace(
                "class CounterOwner(counter: Counter)",
                "class CounterOwner(counter: Counter = new Counter(1))",
            ),
            phase: "constructor planning",
            reason_prefix: "InvalidPrimaryConstructor",
        },
        Case {
            name: "extra constructor formal",
            source: REFERENCE_ALIAS_SOURCE.replace(
                "class CounterOwner(counter: Counter)",
                "class CounterOwner(counter: Counter, x: Int)",
            ),
            phase: "TASTy plan validation",
            reason_prefix: "Unsupported { feature: PrimaryConstructorInput",
        },
        Case {
            name: "extra constructor clause",
            source: REFERENCE_ALIAS_SOURCE.replace(
                "class CounterOwner(counter: Counter)",
                "class CounterOwner(counter: Counter)(x: Int)",
            ),
            phase: "constructor planning",
            reason_prefix: "InvalidPrimaryConstructor",
        },
        Case {
            name: "contextual constructor formal",
            source: REFERENCE_ALIAS_SOURCE.replace(
                "class CounterOwner(counter: Counter)",
                "class CounterOwner(using counter: Counter)",
            ),
            phase: "constructor planning",
            reason_prefix: "InvalidPrimaryConstructor",
        },
        Case {
            name: "secondary constructor",
            source: REFERENCE_ALIAS_SOURCE.replace(
                "  def read(): Int = inner.get()",
                "  def this(x: Int) = this(new Counter(x))\n  def read(): Int = inner.get()",
            ),
            phase: "member planning",
            reason_prefix: "method is not owned by its declaring class",
        },
        Case {
            name: "wrong alias field type",
            source: REFERENCE_ALIAS_SOURCE.replace(
                "private val inner: Counter",
                "private val inner: Int",
            ),
            phase: "member planning",
            reason_prefix: "private immutable fields require a canonical same-batch class TypeRef",
        },
        Case {
            name: "alias to another same-typed formal",
            source: REFERENCE_ALIAS_SOURCE
                .replace(
                    "class CounterOwner(counter: Counter)",
                    "class CounterOwner(counter: Counter, other: Counter)",
                )
                .replace("private val inner: Counter = counter", "private val inner: Counter = other"),
            phase: "TASTy plan validation",
            reason_prefix: "Unsupported { feature: PrimaryConstructorInput",
        },
    ];
    for case in variants {
        let error = with_paired_jvm_tasty_batch_plan(
            &case.source,
            "CounterOwner.scala",
            SourceId::from_index(1771),
            |_| (),
        )
        .unwrap_err();
        match error {
            PairedPlanError::BatchClass { phase, reason, .. }
            | PairedPlanError::BatchUnit { phase, reason } => {
                assert_eq!(
                    phase, case.phase,
                    "{} rejected at the wrong stage: {reason}",
                    case.name,
                );
                assert!(
                    reason.starts_with(case.reason_prefix),
                    "{} expected {} but got {reason}",
                    case.name,
                    case.reason_prefix
                );
            }
            other => panic!(
                "{} expected a measured batch rejection, got {other:?}",
                case.name
            ),
        }
    }
}

#[test]
fn one_real_source_session_preserves_identity_and_emits_cross_class_field_receiver() {
    let source_id = SourceId::from_index(1715);
    let source = SourceText::new(SOURCE).unwrap();
    let scanner = ContextualScanner::new(SOURCE).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty());
    let mut packages = Packages::new();
    let source_index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "CounterOwner.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        other => panic!("exact audit source names without diagnostics: {other:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("fixture has one package root");
    };
    assert_eq!(package.stats.len(), 2);

    let mut class_declarations = Vec::new();
    for &class_tree in &package.stats {
        let TreeKind::TypeDef(class_definition) = &parsed.ast.get(class_tree).kind else {
            panic!("fixture top-level declarations are classes");
        };
        let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
            panic!("class has a template");
        };
        let class = source_index.symbol_at(source_id, class_tree).unwrap();
        let constructor = source_index
            .symbol_at(source_id, template.constructor)
            .unwrap();
        let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(template.constructor).kind
        else {
            panic!("class template has a primary constructor");
        };
        let [seed_tree] = constructor_definition.value_param_clauses[0].as_slice() else {
            panic!("fixture has one constructor parameter");
        };
        let seed = source_index
            .derived_symbol_at(constructor, source_id, *seed_tree)
            .unwrap();
        class_declarations.push((class_tree, class, template.constructor, constructor, seed));
    }

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    let mut class_symbols = Vec::new();
    let mut method_symbols = Vec::new();
    let mut owner_parts = None;
    let mut counter_get = None;
    let mut counter_constructor = None;
    for (class_tree, class, constructor_tree, constructor, seed) in class_declarations {
        typer.complete_symbol(class).unwrap();
        typer.complete_symbol(constructor).unwrap();
        typer.complete_symbol(seed).unwrap();
        assert_eq!(typer.store().symbols.get(seed).owner, Some(constructor));
        class_symbols.push(class);
        let TreeKind::TypeDef(class_definition) = &parsed.ast.get(class_tree).kind else {
            unreachable!();
        };
        let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
            unreachable!();
        };
        let class_name = typer
            .store()
            .names
            .resolve(class_definition.name.as_name().text())
            .to_owned();
        let mut field_info = None;
        let mut read_info = None;
        let mut get_info = None;
        for &member_tree in &template.body {
            match &parsed.ast.get(member_tree).kind {
                TreeKind::ValDef(definition) => {
                    let field = source_index.symbol_at(source_id, member_tree).unwrap();
                    typer.complete_symbol(field).unwrap();
                    let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info
                    else {
                        panic!("field signature completes");
                    };
                    let rhs = definition.rhs.unwrap();
                    let context = typer.field_initializer_context_for(field).unwrap();
                    let typed = typer
                        .type_expression_expected(rhs, context, field_type)
                        .unwrap();
                    assert_eq!(typer.source_typed_index().get(source_id, rhs), Some(typed));
                    assert_eq!(typer.store().symbols.get(field).owner, Some(class));
                    if class_name == "CounterOwner" {
                        assert_eq!(
                            definition.metadata.visibility,
                            Some(dotty_core::ast::VisibilitySyntax::Private { qualifier: None })
                        );
                        assert!(
                            !typer
                                .store()
                                .symbols
                                .get(field)
                                .flags
                                .contains(SymbolFlags::MUTABLE)
                        );
                        field_info = Some((field, member_tree, rhs, typed, field_type));
                    }
                }
                TreeKind::DefDef(definition) if member_tree != constructor_tree => {
                    let method = source_index.symbol_at(source_id, member_tree).unwrap();
                    typer.complete_symbol(method).unwrap();
                    for &formal_tree in definition.value_param_clauses.iter().flatten() {
                        typer
                            .complete_symbol(
                                source_index.symbol_at(source_id, formal_tree).unwrap(),
                            )
                            .unwrap();
                    }
                    let rhs = definition.rhs.unwrap();
                    let context = typer.expression_context_for(method).unwrap();
                    let typed = typer.type_expression(rhs, context).unwrap();
                    assert_eq!(typer.source_typed_index().get(source_id, rhs), Some(typed));
                    if class_name == "CounterOwner" {
                        assert_eq!(
                            typer
                                .store()
                                .names
                                .resolve(definition.name.as_name().text()),
                            "read"
                        );
                        read_info = Some((method, member_tree, rhs, typed));
                    } else if typer
                        .store()
                        .names
                        .resolve(definition.name.as_name().text())
                        == "get"
                    {
                        get_info = Some(method);
                    }
                    method_symbols.push(method);
                }
                other => panic!("unexpected source member: {other:?}"),
            }
        }
        if class_name == "CounterOwner" {
            owner_parts = Some((
                class_tree,
                class,
                constructor_tree,
                constructor,
                seed,
                field_info.unwrap(),
                read_info.unwrap(),
            ));
        } else {
            counter_get = get_info;
            counter_constructor = Some(constructor);
        }
    }
    let (
        owner_class_tree,
        owner_class,
        owner_constructor_tree,
        owner_constructor,
        owner_seed,
        (inner, inner_tree, inner_rhs, inner_typed, inner_type),
        (read, read_tree, read_rhs, read_typed),
    ) = owner_parts.unwrap();
    let counter_get = counter_get.unwrap();
    let counter_constructor = counter_constructor.unwrap();
    assert!(
        matches!(typer.store().types.try_get(inner_type), Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }) if *symbol == class_symbols[0])
    );
    assert_eq!(typer.store().symbols.get(inner).kind, SymbolKind::Field);
    assert_eq!(typer.store().symbols.get(inner).owner, Some(owner_class));
    assert_eq!(
        typer.store().symbols.get(owner_seed).owner,
        Some(owner_constructor)
    );

    let TreeKind::Apply(initializer) = &typer.typed_ast().get(inner_typed).kind else {
        panic!("inner initializer is a typed constructor application");
    };
    let TreeKind::Select(constructor_select) = &typer.typed_ast().get(initializer.function).kind
    else {
        panic!("initializer selects the constructor on new Counter");
    };
    let TreeKind::New(new_counter) = &typer.typed_ast().get(constructor_select.qualifier).kind
    else {
        panic!("constructor receiver is a typed New");
    };
    assert!(
        matches!(typer.store().types.try_get(typer.typed_ast().get(initializer.function).ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == counter_constructor)
    );
    assert_eq!(initializer.args.len(), 1);
    assert!(
        matches!(typer.store().types.try_get(typer.typed_ast().get(initializer.args[0]).ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == owner_seed)
    );
    assert!(
        matches!(typer.store().types.try_get(typer.typed_ast().get(new_counter.tpt).ty), Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }) if *symbol == class_symbols[0])
    );

    let TreeKind::Apply(read_apply) = &typer.typed_ast().get(read_typed).kind else {
        panic!("read body is a method application");
    };
    let TreeKind::Select(read_select) = &typer.typed_ast().get(read_apply.function).kind else {
        panic!("read body selects get");
    };
    let read_receiver = typer.typed_ast().get(read_select.qualifier);
    assert!(
        matches!(typer.store().types.try_get(read_receiver.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == inner)
    );
    assert!(
        matches!(typer.store().types.try_get(typer.typed_ast().get(read_apply.function).ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == counter_get)
    );
    assert_eq!(
        typer.source_typed_index().get(source_id, inner_rhs),
        Some(inner_typed)
    );
    assert_eq!(
        typer.source_typed_index().get(source_id, read_rhs),
        Some(read_typed)
    );

    let owner_members = [
        ClassMemberPicklePlan::Field(ClassFieldPicklePlan {
            symbol: inner,
            source_tree: inner_tree,
            typed_initializer: inner_typed,
            declared_type: inner_type,
        }),
        ClassMemberPicklePlan::Method {
            symbol: read,
            source_tree: read_tree,
        },
    ];
    let planned = PlannedBindings {
        source_typed: typer.source_typed_index(),
        classes: &class_symbols,
        methods: &method_symbols,
    };
    let plan = ClassStatePicklePlan {
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        typed_bindings: &planned,
        source: SourceMetadata {
            id: source_id,
            name: "CounterOwner.scala".to_owned(),
        },
        class: owner_class,
        class_tree: owner_class_tree,
        constructor: PrimaryConstructorInputPlan {
            symbol: owner_constructor,
            source_tree: owner_constructor_tree,
            parameter_clauses: vec![vec![owner_seed]],
        },
        members: &owner_members,
    };
    let validated = plan.validate().unwrap();
    assert_eq!(validated.same_batch_reference_fields().count(), 1);
    let output = validated
        .pickle()
        .expect("the validated owner field can receive Counter.get");
    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let index = file.ast_address_index().unwrap();
    let read_body = index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let body = index.get(node.offset as u32)?.decode_defdef_body().ok()?;
            (file.names().render(body.name).ok()? == "read").then_some(body)
        })
        .expect("CounterOwner TASTy contains read");
    let RawTree::LengthNode(read_apply_node) = read_body.rhs.unwrap() else {
        panic!("read body applies Counter.get");
    };
    assert_eq!(read_apply_node.tag, APPLY_TAG);
    let read_apply = read_apply_node.decode_apply().unwrap();
    assert!(read_apply.arguments.is_empty());
    let RawTree::LengthNode(read_select_node) = read_apply.function else {
        panic!("read applies the selected Counter method");
    };
    assert_eq!(read_select_node.tag, SELECTIN_TAG);
    let read_select = read_select_node.decode_select_in().unwrap();
    assert!(matches!(
        file.names().render_signed_name(read_select.name).unwrap(),
        Some(RenderedSignedName::Signed { original, signature })
            if original == "get" && signature.result == "scala.Int" && signature.parameters.is_empty()
    ));
    let inner_select = read_select.qualifier.decode_select().unwrap();
    assert_eq!(inner_select.tag, SELECT_TAG);
    assert_eq!(file.names().render(inner_select.name).unwrap(), "inner");
    assert!(matches!(
        inner_select.qualifier,
        RawTree::Ast {
            tag: QUALTHIS_TAG,
            ..
        }
    ));
}

#[test]
fn paired_producer_reports_first_failure_for_required_negative_shapes() {
    let variants = [
        (
            "mutable_field",
            SOURCE.replace(
                "private val inner: Counter = new Counter(seed)",
                "private var inner: Counter = new Counter(seed)",
            ),
        ),
        (
            "uninitialized_field",
            SOURCE.replace(
                "private val inner: Counter = new Counter(seed)",
                "private val inner: Counter",
            ),
        ),
        (
            "private_this_field",
            SOURCE.replace(
                "private val inner: Counter = new Counter(seed)",
                "private[this] val inner: Counter = new Counter(seed)",
            ),
        ),
        (
            "public_field",
            SOURCE.replace(
                "private val inner: Counter = new Counter(seed)",
                "val inner: Counter = new Counter(seed)",
            ),
        ),
        (
            "protected_field",
            SOURCE.replace(
                "private val inner: Counter = new Counter(seed)",
                "protected val inner: Counter = new Counter(seed)",
            ),
        ),
        (
            "local_shadow",
            SOURCE.replace(
                "def read(): Int = inner.get()",
                "def read(): Int = { val inner = new Counter(0); inner.get() }",
            ),
        ),
        (
            "unrelated_receiver",
            SOURCE.replace(
                "def read(): Int = inner.get()",
                "def read(): Int = new Counter(0).get()",
            ),
        ),
        (
            "duplicate_field",
            SOURCE.replace(
                "  def read(): Int = inner.get()",
                "  private val inner: Counter = new Counter(seed)\n  def read(): Int = inner.get()",
            ),
        ),
        (
            "wrong_reference_type",
            SOURCE.replace(
                "private val inner: Counter = new Counter(seed)",
                "private val inner: String = new Counter(seed)",
            ),
        ),
        (
            "wrong_constructor_argument",
            SOURCE.replace("new Counter(seed)", "new Counter(true)"),
        ),
        (
            "nested_new_initializer",
            SOURCE.replace("new Counter(seed)", "new Counter(new Counter(seed).get())"),
        ),
        (
            "factory_valued_initializer",
            SOURCE.replace("new Counter(seed)", "Counter.make(seed)"),
        ),
        (
            "branch_valued_initializer",
            SOURCE.replace(
                "new Counter(seed)",
                "if seed == 0 then new Counter(seed) else new Counter(seed)",
            ),
        ),
        (
            "missing_counter",
            format!(
                "package example\nclass Other {{}}\nclass CounterOwner{}",
                SOURCE.split_once("class CounterOwner").unwrap().1
            ),
        ),
        (
            "foreign_counter",
            SOURCE.replace(
                "private val inner: Counter = new Counter(seed)",
                "private val inner: foreign.Counter = new foreign.Counter(seed)",
            ),
        ),
    ];
    for (name, source) in variants {
        let error = with_paired_jvm_tasty_batch_plan(
            &source,
            "CounterOwner.scala",
            SourceId::from_index(1716),
            |_| (),
        )
        .unwrap_err();
        if name == "mutable_field" {
            assert!(matches!(
                &error,
                PairedPlanError::BatchClass {
                    class_index: 1,
                    declaration_index: 1,
                    phase: "method body typing",
                    reason,
                } if reason.starts_with("UnstableSelectionPrefix")
            ));
        } else {
            assert!(
                matches!(
                &error,
                    PairedPlanError::BatchClass {
                        class_index: 1,
                        declaration_index: 0,
                        phase: "member planning",
                        reason,
                    } if reason == "only unqualified private mutable fields and initialized private immutable same-batch reference fields are supported"
                ) || matches!(name, "local_shadow" | "unrelated_receiver")
                    && matches!(
                        &error,
                        PairedPlanError::BatchClass {
                            class_index: 1,
                            phase: "JVM/TASTy cross-contract validation",
                            reason,
                            ..
                        } if reason.starts_with("TastyPlan(Unsupported { feature: BodyExpression")
                    )
                    || name == "duplicate_field"
                        && matches!(
                            &error,
                            PairedPlanError::BatchClass {
                                class_index: 1,
                                phase: "method body typing",
                                reason,
                                ..
                            } if reason.starts_with("AmbiguousTermReference")
                        )
                    || matches!(
                        name,
                        "wrong_reference_type" | "missing_counter" | "foreign_counter"
                    ) && matches!(
                        &error,
                        PairedPlanError::BatchClass {
                            class_index: 1,
                            phase: "field typing",
                            reason,
                            ..
                        } if reason.starts_with("TypeNameNotFound")
                    )
                    || name == "wrong_constructor_argument"
                        && matches!(
                            &error,
                            PairedPlanError::BatchClass {
                                class_index: 1,
                                phase: "field initializer typing",
                                reason,
                                ..
                            } if reason.starts_with("ApplicationArgumentTypeMismatch")
                        )
                    || name == "nested_new_initializer"
                        && matches!(
                            &error,
                            PairedPlanError::BatchClass {
                                class_index: 1,
                                phase: "TASTy plan validation",
                                reason,
                                ..
                            } if reason.starts_with("Unsupported { feature: UnsafeFieldInitializer")
                        )
                    || name == "branch_valued_initializer"
                        && matches!(
                            &error,
                            PairedPlanError::BatchUnit {
                                phase: "complete JVM unit validation",
                                reason,
                            } if reason.starts_with("UnsupportedJvmReferenceFieldInitializer")
                        )
                    || name == "factory_valued_initializer"
                        && matches!(
                            &error,
                            PairedPlanError::BatchClass {
                                class_index: 1,
                                phase: "field initializer typing",
                                reason,
                                ..
                            } if reason.starts_with("TermNameNotFound")
                        ),
                "{name} must report the first current batch boundary: {error:?}"
            );
        }
    }
}
