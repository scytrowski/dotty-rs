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
    ClassFieldPicklePlan, ClassMemberPicklePlan, ClassStatePicklePlan, PickleError,
    PrimaryConstructorInputPlan, SourceMetadata, SourceTypedBindings,
};
use dotty_typer::SourceTyper;

use dotty::jvm_tasty_plan::{PairedPlanError, with_paired_jvm_tasty_batch_plan};

const SOURCE: &str = include_str!("fixtures/jvm_tasty_object_composition/CounterOwner.scala");
const COUNTER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_object_composition/scala39/example/Counter.class");
const COUNTER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_object_composition/scala39/example/Counter.tasty");
const OWNER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_object_composition/scala39/example/CounterOwner.class");
const OWNER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_object_composition/scala39/example/CounterOwner.tasty");

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
fn paired_producer_advances_private_val_to_cross_contract_boundary() {
    let error = with_paired_jvm_tasty_batch_plan(
        SOURCE,
        "CounterOwner.scala",
        SourceId::from_index(1715),
        |_| (),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        PairedPlanError::BatchClass {
            class_index: 1,
            phase: "JVM/TASTy cross-contract validation",
            reason,
            ..
        } if reason.starts_with("TastyPlan(Reference")
    ));
}

#[test]
fn one_real_source_session_preserves_object_composition_identity_and_reports_pickler_gap() {
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
    let pickler_error = validated.pickle().unwrap_err();
    assert!(
        matches!(pickler_error, PickleError::UnsupportedTypeReference { ty } if ty == inner_type)
    );
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
