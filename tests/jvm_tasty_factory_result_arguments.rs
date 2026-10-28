use dotty::jvm_tasty_cross_class::{
    CrossClassConstructorCallProofError, CrossClassConstructorCallProofInput,
    validate_cross_class_constructor_call_proofs,
};
use dotty::jvm_tasty_plan::{PairedPlanError, with_paired_jvm_tasty_batch_plan};
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::reader::Reader as ClassReader;
use dotty_core::ast::TreeKind;
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SourceTypedIndex, SymbolId,
    SymbolInfo, TermRefTarget, Type, TypeRefTarget,
};
use dotty_jvm::{
    JvmClassInitializationPlan, JvmClassPlan, JvmCompilationUnit, JvmMethodPlan,
    JvmPrimaryConstructorPlan,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty::ast::{
    APPLY_TAG, DEFDEF_TAG, IDENTTPT_TAG, NEW_TAG, PARAM_TAG, QUALTHIS_TAG, SELECTIN_TAG,
    TERMREFDIRECT_TAG,
};
use dotty_tasty::file::TastyFile;
use dotty_tasty::tasty::{RenderedNameSignature, RenderedParamSig, RenderedSignedName};
use dotty_tasty::term::{AstRefKind, RawTree};
use dotty_tasty_pickler::{
    ClassMemberPicklePlan, ClassStatePicklePlan, PrimaryConstructorInputPlan, SourceMetadata,
    SourceTypedBindings,
};
use dotty_typer::SourceTyper;

const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_factory_result_arguments/CounterOwnerFactory.scala");

struct AuditBindings<'a> {
    index: &'a SourceTypedIndex,
    classes: Vec<SymbolId>,
    methods: Vec<SymbolId>,
}

impl SourceTypedBindings for AuditBindings<'_> {
    fn typed_at(
        &self,
        source: SourceId,
        tree: dotty_core::TreeId<dotty_core::ast::Untyped>,
    ) -> Option<dotty_core::TreeId<dotty_core::ast::Typed>> {
        self.index.get(source, tree)
    }

    fn planned_class_symbols(&self) -> &[SymbolId] {
        &self.classes
    }

    fn planned_method_symbols(&self) -> &[SymbolId] {
        &self.methods
    }
}

const COUNTER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_factory_result_arguments/scala39/example/Counter.class");
const COUNTER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_factory_result_arguments/scala39/example/Counter.tasty");
const OWNER_CLASS: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_factory_result_arguments/scala39/example/CounterOwner.class"
);
const OWNER_TASTY: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_factory_result_arguments/scala39/example/CounterOwner.tasty"
);
const FACTORY_CLASS: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_factory_result_arguments/scala39/example/CounterOwnerFactory.class"
);
const FACTORY_TASTY: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_factory_result_arguments/scala39/example/CounterOwnerFactory.tasty"
);

fn decode_class(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = ClassReader::new(bytes);
    let class = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    class
}

fn tasty(bytes: &[u8]) -> TastyFile<'_> {
    let file = TastyFile::parse_and_validate_scala_3_9(bytes).unwrap();
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

fn class_name<'a>(class: &'a ClassFile<'_>) -> &'a str {
    class.constant_pool.class_name(class.this_class).unwrap()
}

fn method<'a>(class: &'a ClassFile<'_>, name: &str) -> &'a dotty_classfile::method::MethodInfo<'a> {
    class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == name)
        .unwrap_or_else(|| panic!("class declares method {name}"))
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

fn method_body<'a>(
    tasty: &'a TastyFile<'_>,
    name: &str,
) -> (u32, dotty_tasty::ast::DefDefBody<'a>) {
    let index = tasty.ast_address_index().unwrap();
    index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let address = node.offset as u32;
            let body = index.get(address).unwrap().decode_defdef_body().unwrap();
            (tasty.names().render(body.name).unwrap() == name).then_some((address, body))
        })
        .unwrap_or_else(|| panic!("TASTy contains method {name}"))
}

fn parameter_address(index: &dotty_tasty::ast::AstAddressIndex<'_>, owner: u32) -> u32 {
    index
        .iter_nodes_with_tag(PARAM_TAG)
        .find_map(|candidate| {
            let mut parent = Some(candidate);
            while let Some(node) = parent {
                if node.offset as u32 == owner {
                    return Some(candidate.offset as u32);
                }
                parent = index.parent_of(node.offset as u32);
            }
            None
        })
        .expect("method owns a PARAM node")
}

#[test]
fn scala39_factory_result_oracle_decodes_classes_and_both_factory_methods() {
    let classes = [
        decode_class(COUNTER_CLASS),
        decode_class(OWNER_CLASS),
        decode_class(FACTORY_CLASS),
    ];
    let tastys = [
        tasty(COUNTER_TASTY),
        tasty(OWNER_TASTY),
        tasty(FACTORY_TASTY),
    ];
    assert_eq!(
        classes.iter().map(class_name).collect::<Vec<_>>(),
        [
            "example/Counter",
            "example/CounterOwner",
            "example/CounterOwnerFactory"
        ]
    );
    let expected_uuids = [
        "00d64806c49b36e800ec22da2f7eb01b",
        "0035b89b8dc3a7ee00dd6c3944864066",
        "00526cc942642dd2001c7c921db70921",
    ];
    for ((class, tasty), expected_uuid) in classes.iter().zip(&tastys).zip(expected_uuids) {
        assert_eq!(class.version.major, 65);
        assert_eq!(
            class.tasty_link_id().unwrap().unwrap().as_bytes(),
            &tasty.header().uuid
        );
        assert_eq!(
            tasty
                .header()
                .uuid
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            expected_uuid
        );
        println!(
            "{} TASTY UUID={}",
            class_name(class),
            tasty
                .header()
                .uuid
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
    }
    let factory_class = &classes[2];
    assert_eq!(
        factory_class
            .constant_pool
            .utf8(method(factory_class, "makeCounter").descriptor_index),
        Ok("(I)Lexample/Counter;")
    );
    assert_eq!(
        factory_class
            .constant_pool
            .utf8(method(factory_class, "create").descriptor_index),
        Ok("(I)Lexample/CounterOwner;")
    );
    assert_eq!(
        method_code(method(factory_class, "create")),
        [
            0xbb, 0x00, 0x19, 0x59, 0x2a, 0x1b, 0xb6, 0x00, 0x1b, 0xb7, 0x00, 0x1e, 0xb0
        ]
    );
    assert_eq!(
        method_code(method(factory_class, "makeCounter")),
        [0xbb, 0x00, 0x11, 0x59, 0x1b, 0xb7, 0x00, 0x14, 0xb0]
    );
    assert_eq!(
        classes[1]
            .constant_pool
            .utf8(method(&classes[1], "<init>").descriptor_index),
        Ok("(Lexample/Counter;)V")
    );
    let factory = &tastys[2];
    let (make_counter_address, make_counter) = method_body(factory, "makeCounter");
    let (create_address, create) = method_body(factory, "create");
    let index = factory.ast_address_index().unwrap();
    let make_counter_parameter = parameter_address(&index, make_counter_address);
    let create_parameter = parameter_address(&index, create_address);
    assert_eq!((make_counter_address, make_counter_parameter), (30, 33));
    assert_eq!((create_address, create_parameter), (62, 65));
    let mut create_wire_nodes = [DEFDEF_TAG, APPLY_TAG, SELECTIN_TAG, PARAM_TAG]
        .into_iter()
        .flat_map(|tag| index.iter_nodes_with_tag(tag))
        .filter(|candidate| {
            let mut parent = Some(*candidate);
            while let Some(node) = parent {
                if node.offset as u32 == create_address {
                    return true;
                }
                parent = index.parent_of(node.offset as u32);
            }
            false
        })
        .map(|node| (node.offset as u32, node.tag))
        .collect::<Vec<_>>();
    create_wire_nodes.sort_unstable();
    assert_eq!(
        create_wire_nodes,
        [
            (62, DEFDEF_TAG),
            (65, PARAM_TAG),
            (78, APPLY_TAG),
            (80, SELECTIN_TAG),
            (90, APPLY_TAG),
            (92, SELECTIN_TAG),
        ]
    );
    assert_eq!(
        factory.names().render(make_counter.name).unwrap(),
        "makeCounter"
    );
    assert_eq!(factory.names().render(create.name).unwrap(), "create");
    assert_eq!(make_counter.parameters.len(), 1);
    assert_eq!(
        factory
            .names()
            .render(make_counter.parameters[0].name())
            .unwrap(),
        "seed"
    );
    assert_eq!(make_counter.return_type.nodes()[0].tag, IDENTTPT_TAG);
    assert_eq!(
        make_counter
            .return_type
            .name_refs()
            .into_iter()
            .map(|name| factory.names().render(name).unwrap())
            .collect::<Vec<_>>(),
        ["Counter", "example"]
    );
    let make_counter_wire_nodes = [DEFDEF_TAG, APPLY_TAG, SELECTIN_TAG, PARAM_TAG]
        .into_iter()
        .flat_map(|tag| index.iter_nodes_with_tag(tag))
        .filter(|candidate| {
            let mut parent = Some(*candidate);
            while let Some(node) = parent {
                if node.offset as u32 == make_counter_address {
                    return true;
                }
                parent = index.parent_of(node.offset as u32);
            }
            false
        })
        .map(|node| (node.offset as u32, node.tag))
        .collect::<Vec<_>>();
    let mut make_counter_wire_nodes = make_counter_wire_nodes;
    make_counter_wire_nodes.sort_unstable();
    assert_eq!(
        make_counter_wire_nodes,
        [
            (30, DEFDEF_TAG),
            (33, PARAM_TAG),
            (48, APPLY_TAG),
            (50, SELECTIN_TAG),
        ]
    );
    let RawTree::LengthNode(make_counter_apply_node) = make_counter.rhs.unwrap() else {
        panic!("makeCounter body is the Counter constructor APPLY");
    };
    let make_counter_apply = make_counter_apply_node.decode_apply().unwrap();
    let RawTree::LengthNode(make_counter_select_node) = make_counter_apply.function else {
        panic!("makeCounter selects Counter.<init>");
    };
    assert_eq!(make_counter_select_node.tag, SELECTIN_TAG);
    let make_counter_select = make_counter_select_node.decode_select_in().unwrap();
    assert_eq!(
        factory
            .names()
            .render_signed_name(make_counter_select.name)
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
    assert!(matches!(
        make_counter_select.qualifier,
        RawTree::Ast { tag: NEW_TAG, .. }
    ));
    let [counter_seed_reference] = make_counter_apply.arguments.as_slice() else {
        panic!("Counter constructor receives its method's seed");
    };
    let counter_seed_references = counter_seed_reference.ast_refs();
    let [counter_seed_reference] = counter_seed_references.as_slice() else {
        panic!("Counter seed is one direct reference");
    };
    assert_eq!(counter_seed_reference.kind, AstRefKind::TermRefDirect);
    assert_eq!(counter_seed_reference.address, make_counter_parameter);
    assert_eq!(create.parameters.len(), 1);
    assert_eq!(
        factory.names().render(create.parameters[0].name()).unwrap(),
        "seed"
    );
    assert_eq!(
        create.return_type.nodes()[0].tag,
        dotty_tasty::ast::IDENTTPT_TAG
    );
    assert_eq!(
        create
            .return_type
            .name_refs()
            .into_iter()
            .map(|name| factory.names().render(name).unwrap())
            .collect::<Vec<_>>(),
        ["CounterOwner"]
    );
    let RawTree::LengthNode(apply) = create.rhs.unwrap() else {
        panic!("create body is an outer APPLY");
    };
    assert_eq!(apply.tag, APPLY_TAG);
    let owner_apply = apply.decode_apply().unwrap();
    let RawTree::LengthNode(owner_select_node) = owner_apply.function else {
        panic!("outer constructor call has SELECTin");
    };
    assert_eq!(owner_select_node.tag, SELECTIN_TAG);
    let owner_select = owner_select_node.decode_select_in().unwrap();
    assert_eq!(
        factory
            .names()
            .render_signed_name(owner_select.name)
            .unwrap()
            .unwrap(),
        RenderedSignedName::Signed {
            original: "<init>".to_owned(),
            signature: RenderedNameSignature {
                parameters: [RenderedParamSig::TermParameter(
                    "example.Counter".to_owned()
                )]
                .into(),
                result: "example.CounterOwner".to_owned(),
            },
        }
    );
    assert!(matches!(
        owner_select.qualifier,
        RawTree::Ast { tag: NEW_TAG, .. }
    ));
    let [factory_call] = owner_apply.arguments.as_slice() else {
        panic!("Owner constructor receives exactly the factory result");
    };
    let RawTree::LengthNode(factory_apply_node) = factory_call else {
        panic!("the Owner argument is an APPLY of makeCounter");
    };
    assert_eq!(factory_apply_node.tag, APPLY_TAG);
    let factory_apply = factory_apply_node.decode_apply().unwrap();
    let RawTree::LengthNode(factory_select_node) = factory_apply.function else {
        panic!("factory invocation has SELECTin");
    };
    assert_eq!(factory_select_node.tag, SELECTIN_TAG);
    let factory_select = factory_select_node.decode_select_in().unwrap();
    assert_eq!(
        factory
            .names()
            .render_signed_name(factory_select.name)
            .unwrap()
            .unwrap(),
        RenderedSignedName::Signed {
            original: "makeCounter".to_owned(),
            signature: RenderedNameSignature {
                parameters: [RenderedParamSig::TermParameter("scala.Int".to_owned())].into(),
                result: "example.Counter".to_owned(),
            },
        }
    );
    assert!(matches!(
        factory_select.qualifier,
        RawTree::Ast {
            tag: QUALTHIS_TAG,
            ..
        }
    ));
    let [seed_reference] = factory_apply.arguments.as_slice() else {
        panic!("makeCounter receives create's original seed");
    };
    assert!(matches!(seed_reference, RawTree::Leaf(term) if term.tag == TERMREFDIRECT_TAG));
    let seed_references = seed_reference.ast_refs();
    let [seed_reference] = seed_references.as_slice() else {
        panic!("seed argument has one direct TASTy reference");
    };
    assert_eq!(seed_reference.kind, AstRefKind::TermRefDirect);
    assert_eq!(seed_reference.address, create_parameter);
    println!(
        "Factory TASTy addresses: makeCounter descendants={make_counter_wire_nodes:?}; create descendants={create_wire_nodes:?}"
    );
    println!(
        "Factory TASTy create.seed TERMREFdirect={}",
        seed_reference.address
    );
}

#[test]
fn rust_original_source_preserves_factory_call_identities_and_exposes_first_rejections() {
    let source_id = SourceId::from_index(1832);
    let source_text = SourceText::new(SOURCE).unwrap();
    let scanner = ContextualScanner::new(SOURCE).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source_text, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty());
    let mut packages = Packages::new();
    let source_index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "CounterOwnerFactory.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("the original source names cleanly: {outcome:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("the source has a package root");
    };
    assert_eq!(package.stats.len(), 3);

    let mut class_data = Vec::new();
    for &class_tree in &package.stats {
        let class = source_index.symbol_at(source_id, class_tree).unwrap();
        let TreeKind::TypeDef(definition) = &parsed.ast.get(class_tree).kind else {
            panic!("each package member is a class");
        };
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            panic!("each class has an ordinary template");
        };
        let constructor = source_index
            .symbol_at(source_id, template.constructor)
            .unwrap();
        let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(template.constructor).kind
        else {
            panic!("each primary constructor is a source method");
        };
        let constructor_parameters = constructor_definition
            .value_param_clauses
            .iter()
            .flatten()
            .map(|&tree| {
                source_index
                    .derived_symbol_at(constructor, source_id, tree)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        class_data.push((
            class_tree,
            class,
            template.constructor,
            constructor,
            constructor_parameters,
        ));
    }

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    for (_, class, _, constructor, parameters) in &class_data {
        typer.complete_symbol(*class).unwrap();
        typer.complete_symbol(*constructor).unwrap();
        for parameter in parameters {
            typer.complete_symbol(*parameter).unwrap();
        }
    }

    let mut jvm_classes = Vec::new();
    let mut typed_methods = Vec::new();
    let mut source_methods = Vec::new();
    for (class_tree, class, constructor_tree, constructor, constructor_parameters) in &class_data {
        let TreeKind::TypeDef(definition) = &parsed.ast.get(*class_tree).kind else {
            unreachable!();
        };
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            unreachable!();
        };
        let mut methods = Vec::new();
        for &member_tree in &template.body {
            let TreeKind::DefDef(method_definition) = &parsed.ast.get(member_tree).kind else {
                continue;
            };
            let method = source_index.symbol_at(source_id, member_tree).unwrap();
            typer.complete_symbol(method).unwrap();
            let parameters = method_definition
                .value_param_clauses
                .iter()
                .flatten()
                .map(|&tree| {
                    let symbol = source_index.symbol_at(source_id, tree).unwrap();
                    typer.complete_symbol(symbol).unwrap();
                    symbol
                })
                .collect::<Vec<_>>();
            source_methods.push((method, member_tree));
            let body = method_definition.rhs.map(|source_rhs| {
                let context = typer.expression_context_for(method).unwrap();
                let typed_rhs = typer.type_expression(source_rhs, context).unwrap();
                typed_methods.push((source_rhs, typed_rhs));
                typed_rhs
            });
            methods.push(JvmMethodPlan {
                method,
                parameters,
                locals: Vec::new(),
                body,
            });
        }
        jvm_classes.push(JvmClassPlan {
            class: *class,
            methods,
            fields: Vec::new(),
            initialization: JvmClassInitializationPlan::PrimaryConstructor(
                JvmPrimaryConstructorPlan::from_source(
                    *class,
                    *constructor,
                    constructor_parameters.clone(),
                    &parsed.ast,
                    &source_index,
                )
                .unwrap(),
            ),
        });
        assert_eq!(
            source_index.symbol_at(source_id, *constructor_tree),
            Some(*constructor)
        );
    }
    let jvm_unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: jvm_classes,
    };
    jvm_unit.metadata(&definitions).unwrap();

    let find_method = |name: &str| {
        source_methods
            .iter()
            .find_map(|(symbol, tree)| {
                let TreeKind::DefDef(definition) = &parsed.ast.get(*tree).kind else {
                    unreachable!();
                };
                (typer
                    .store()
                    .names
                    .resolve(definition.name.as_name().text())
                    == name)
                    .then_some((*symbol, *tree))
            })
            .unwrap()
    };
    let (make_counter, make_counter_tree) = find_method("makeCounter");
    let (create, create_tree) = find_method("create");
    let TreeKind::DefDef(create_definition) = &parsed.ast.get(create_tree).kind else {
        unreachable!();
    };
    let source_rhs = create_definition.rhs.unwrap();
    let TreeKind::Apply(source_owner_apply) = &parsed.ast.get(source_rhs).kind else {
        panic!("create source body is the Owner constructor Apply");
    };
    let TreeKind::Select(source_owner_selection) =
        &parsed.ast.get(source_owner_apply.function).kind
    else {
        panic!("create selects the Owner primary constructor");
    };
    let source_owner_new = source_owner_selection.qualifier;
    let TreeKind::New(source_owner_new_tree) = &parsed.ast.get(source_owner_new).kind else {
        panic!("Owner constructor selection retains its New");
    };
    let [source_factory_call] = source_owner_apply.args.as_slice() else {
        panic!("Owner receives the single factory result");
    };
    let TreeKind::Apply(source_factory_apply) = &parsed.ast.get(*source_factory_call).kind else {
        panic!("Owner argument is the makeCounter application");
    };
    let TreeKind::Ident(source_factory_name) = &parsed.ast.get(source_factory_apply.function).kind
    else {
        panic!("unqualified makeCounter call retains its original Ident");
    };
    assert_eq!(
        typer
            .store()
            .names
            .resolve(source_owner_selection.name.text()),
        "<init>"
    );
    assert_eq!(
        typer.store().names.resolve(source_factory_name.name.text()),
        "makeCounter"
    );
    let make_counter_parameter_tree = match &parsed.ast.get(make_counter_tree).kind {
        TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
        _ => unreachable!(),
    };
    let make_counter_parameter = source_index
        .symbol_at(source_id, make_counter_parameter_tree)
        .unwrap();
    let create_parameter_tree = create_definition.value_param_clauses[0][0];
    let create_parameter = source_index
        .symbol_at(source_id, create_parameter_tree)
        .unwrap();
    let owner_constructor = class_data[1].3;
    let owner_constructor_parameter = class_data[1].4[0];
    let counter_class = class_data[0].1;
    let owner_class = class_data[1].1;
    let factory_class = class_data[2].1;
    let (
        typed_rhs,
        typed_owner_select,
        typed_owner_new,
        typed_factory_call,
        typed_factory_select,
        typed_seed,
    ) = {
        let source_typed_index = typer.source_typed_index();
        for source_tree in [
            source_rhs,
            source_owner_apply.function,
            source_owner_new,
            source_owner_new_tree.tpt,
            *source_factory_call,
            source_factory_apply.function,
            source_factory_apply.args[0],
        ] {
            assert!(source_typed_index.get(source_id, source_tree).is_some());
        }
        let typed_rhs = source_typed_index.get(source_id, source_rhs).unwrap();
        let TreeKind::Apply(typed_owner_apply) = &typer.typed_ast().get(typed_rhs).kind else {
            panic!("typed RHS remains the outer constructor Apply");
        };
        let typed_owner_select = typed_owner_apply.function;
        let TreeKind::Select(owner_selection) = &typer.typed_ast().get(typed_owner_select).kind
        else {
            panic!("typed Owner constructor remains a selected method");
        };
        let typed_owner_new = owner_selection.qualifier;
        assert!(matches!(
            typer.typed_ast().get(typed_owner_new).kind,
            TreeKind::New(_)
        ));
        assert_eq!(typed_owner_apply.args.len(), 1);
        let typed_factory_call = typed_owner_apply.args[0];
        let TreeKind::Apply(typed_factory_apply) = &typer.typed_ast().get(typed_factory_call).kind
        else {
            panic!("typed Owner argument remains an Apply");
        };
        let typed_factory_select = typed_factory_apply.function;
        let TreeKind::Ident(_) = &typer.typed_ast().get(typed_factory_select).kind else {
            panic!("typed factory function retains its original unqualified reference");
        };
        let [typed_seed] = typed_factory_apply.args.as_slice() else {
            panic!("typed factory application has one seed");
        };
        (
            typed_rhs,
            typed_owner_select,
            typed_owner_new,
            typed_factory_call,
            typed_factory_select,
            *typed_seed,
        )
    };
    assert!(matches!(
        typer
            .store()
            .types
            .get(typer.typed_ast().get(typed_owner_select).ty),
        Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        } if *symbol == owner_constructor
    ));
    let factory_select_ty = typer.typed_ast().get(typed_factory_select).ty;
    let Type::TermRef {
        prefix,
        target: TermRefTarget::Symbol(selected_factory),
    } = typer.store().types.get(factory_select_ty)
    else {
        panic!("typed factory selection is a canonical TermRef");
    };
    assert_eq!(*selected_factory, make_counter);
    assert!(
        matches!(typer.store().types.get(*prefix), Type::ThisType { class } if *class == factory_class)
    );
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(typed_factory_call).ty),
        Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == counter_class
    ));
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(typed_seed).ty),
        Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == create_parameter
    ));
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(typed_rhs).ty),
        Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == owner_class
    ));
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(typed_owner_new).ty),
        Type::TypeRef { target: TypeRefTarget::Symbol(class), .. } if *class == owner_class
    ));
    let SymbolInfo::Complete(make_counter_type) = typer.store().symbols.get(make_counter).info
    else {
        panic!("makeCounter has a completed method type");
    };
    let Type::Method(make_counter_signature) = typer.store().types.get(make_counter_type) else {
        panic!("makeCounter signature is a method type");
    };
    assert_eq!(make_counter_signature.params.len(), 1);
    assert_eq!(
        typer
            .store()
            .names
            .resolve(make_counter_signature.params[0].name.as_name().text()),
        "seed"
    );
    assert!(matches!(
        typer.store().types.get(make_counter_signature.result),
        Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == counter_class
    ));
    let SymbolInfo::Complete(owner_constructor_type) =
        typer.store().symbols.get(owner_constructor).info
    else {
        panic!("Owner constructor has a completed method type");
    };
    let Type::Method(owner_constructor_signature) = typer.store().types.get(owner_constructor_type)
    else {
        panic!("Owner primary constructor signature is a method type");
    };
    assert_eq!(owner_constructor_signature.params.len(), 1);
    assert_eq!(
        typer
            .store()
            .names
            .resolve(owner_constructor_signature.params[0].name.as_name().text()),
        "counter"
    );
    assert!(matches!(
        typer.store().types.get(owner_constructor_signature.params[0].ty),
        Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == counter_class
    ));
    assert_eq!(
        source_index.symbol_at(source_id, make_counter_parameter_tree),
        Some(make_counter_parameter)
    );
    let owner_constructor_parameter_tree = match &parsed.ast.get(class_data[1].2).kind {
        TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
        _ => unreachable!(),
    };
    assert_eq!(
        source_index.derived_symbol_at(
            owner_constructor,
            source_id,
            owner_constructor_parameter_tree
        ),
        Some(owner_constructor_parameter)
    );
    assert_eq!(
        typer.source_typed_index().get(source_id, source_rhs),
        Some(typed_rhs)
    );
    assert_eq!(
        typer
            .source_typed_index()
            .get(source_id, *source_factory_call),
        Some(typed_factory_call)
    );
    let source_tree_set = [
        source_rhs,
        source_owner_apply.function,
        source_owner_new,
        source_owner_new_tree.tpt,
        *source_factory_call,
        source_factory_apply.function,
        source_factory_apply.args[0],
    ];
    let typed_tree_set =
        source_tree_set.map(|tree| typer.source_typed_index().get(source_id, tree).unwrap());
    let factory_constructor_tree = class_data[2].2;
    let factory_constructor_parameters = match &parsed.ast.get(factory_constructor_tree).kind {
        TreeKind::DefDef(definition) => definition
            .value_param_clauses
            .iter()
            .map(|clause| {
                clause
                    .iter()
                    .map(|&tree| {
                        source_index
                            .derived_symbol_at(class_data[2].3, source_id, tree)
                            .unwrap()
                    })
                    .collect::<Vec<_>>()
            })
            .collect(),
        _ => unreachable!(),
    };
    let planned_classes = class_data.iter().map(|item| item.1).collect::<Vec<_>>();
    let planned_methods = source_methods.iter().map(|item| item.0).collect::<Vec<_>>();
    let tasty_bindings = AuditBindings {
        index: typer.source_typed_index(),
        classes: planned_classes,
        methods: planned_methods,
    };
    let factory_members = [
        ClassMemberPicklePlan::Method {
            symbol: make_counter,
            source_tree: make_counter_tree,
        },
        ClassMemberPicklePlan::Method {
            symbol: create,
            source_tree: create_tree,
        },
    ];
    let class_state_plan = ClassStatePicklePlan {
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        typed_bindings: &tasty_bindings,
        source: SourceMetadata {
            id: source_id,
            name: "CounterOwnerFactory.scala".to_owned(),
        },
        class: factory_class,
        class_tree: class_data[2].0,
        constructor: PrimaryConstructorInputPlan {
            symbol: class_data[2].3,
            source_tree: factory_constructor_tree,
            parameter_clauses: factory_constructor_parameters,
        },
        members: &factory_members,
    };
    let _validated_class_state = class_state_plan
        .validate()
        .expect("class-state source/TASTy signature validation succeeds before body proof");
    println!(
        "Rust one-session identities: classes={:?}, Counter.<init>={:?}, Counter.start={:?}, Owner.<init>={:?}, Owner.counter={owner_constructor_parameter:?}, Factory.makeCounter={make_counter:?}, makeCounter.seed={make_counter_parameter:?}, Factory.create={create:?}, create.seed={create_parameter:?}, definitions=[makeCounter:{make_counter_tree:?}/{make_counter_parameter_tree:?}, create:{create_tree:?}/{create_parameter_tree:?}], source={source_tree_set:?}, typed={typed_tree_set:?}, constructor_selected={owner_constructor:?}, factory_receiver=This({factory_class:?}), factory_result=Counter({counter_class:?}), result=Owner({owner_class:?})",
        class_data.iter().map(|item| item.1).collect::<Vec<_>>(),
        class_data[0].3,
        class_data[0].4[0],
        class_data[1].3,
    );

    let direct_proof_result =
        validate_cross_class_constructor_call_proofs(CrossClassConstructorCallProofInput {
            source_id,
            source_root: parsed.root,
            store: typer.store(),
            definitions: &definitions,
            source_ast: &parsed.ast,
            typed_ast: typer.typed_ast(),
            source_index: &source_index,
            source_typed_index: typer.source_typed_index(),
            jvm_unit: &jvm_unit,
        });
    let direct_proof = match direct_proof_result {
        Err(error) => error,
        Ok(_) => panic!("current proof unexpectedly accepts a factory result constructor argument"),
    };
    assert!(matches!(
        direct_proof,
        CrossClassConstructorCallProofError::SourceShape(message)
            if message == "nested constructor function must be Select"
    ));
    let paired_error =
        with_paired_jvm_tasty_batch_plan(SOURCE, "CounterOwnerFactory.scala", source_id, |_| ())
            .expect_err("paired planning stops before TASTy encoding");
    assert!(matches!(
        &paired_error,
        PairedPlanError::BatchUnit { phase: "cross-class constructor-call identity proof", reason }
            if reason.contains("nested constructor function must be Select")
    ));
    assert_eq!(
        typed_methods.len(),
        source_methods.iter().filter(|(_, tree)| matches!(&parsed.ast.get(*tree).kind, TreeKind::DefDef(definition) if definition.rhs.is_some())).count()
    );
    println!(
        "Rust layer observations: class-state TASTy source plan validation succeeds; source body proof={direct_proof:?}; paired planner={paired_error:?}; expression-body TASTy encoding and artifact contract/publisher unreachable after paired preflight; JVM-only path is covered by the strict object-graph acceptance regression."
    );
    let _ = (make_counter_tree, create_tree);
}

#[test]
fn factory_argument_variants_retain_semantic_identity_independently_of_names_and_order() {
    let renamed = SOURCE
        .replace("makeCounter", "buildItem")
        .replace("CounterOwnerFactory", "Assembler")
        .replace("CounterOwner", "Box")
        .replace("Counter", "Item")
        .replace("example", "renamed.pkg");
    let renamed_error = with_paired_jvm_tasty_batch_plan(
        &renamed,
        "RenamedFactoryResult.scala",
        SourceId::from_index(1833),
        |_| (),
    )
    .expect_err("renaming does not avoid the bounded proof gate");
    assert!(matches!(
        renamed_error,
        PairedPlanError::BatchUnit {
            phase: "cross-class constructor-call identity proof",
            reason,
        } if reason.contains("nested constructor function must be Select")
    ));

    let owner_start = SOURCE.find("class CounterOwner(").unwrap();
    let factory_start = SOURCE.find("class CounterOwnerFactory:").unwrap();
    let reordered = format!(
        "{}{}\n{}",
        &SOURCE[..owner_start],
        &SOURCE[factory_start..],
        &SOURCE[owner_start..factory_start]
    );
    let reordered_error = with_paired_jvm_tasty_batch_plan(
        &reordered,
        "ReorderedFactoryResult.scala",
        SourceId::from_index(1834),
        |_| (),
    )
    .expect_err("declaration order does not avoid the bounded proof gate");
    assert!(matches!(
        reordered_error,
        PairedPlanError::BatchUnit {
            phase: "cross-class constructor-call identity proof",
            reason,
        } if reason.contains("nested constructor function must be Select")
    ));
}

#[test]
fn factory_argument_negative_controls_report_their_first_existing_gate() {
    let wrong_arity = SOURCE.replace("makeCounter(seed)", "makeCounter()");
    let wrong_arity_error = with_paired_jvm_tasty_batch_plan(
        &wrong_arity,
        "WrongFactoryArity.scala",
        SourceId::from_index(1835),
        |_| (),
    )
    .expect_err("wrong factory arity is rejected during source typing");
    assert!(matches!(
        wrong_arity_error,
        PairedPlanError::BatchClass {
            phase: "method body typing",
            reason,
            ..
        } if reason.contains("ApplicationArityMismatch")
    ));

    let wrong_result = SOURCE.replace(
        "def makeCounter(seed: Int): Counter =\n    new Counter(seed)",
        "def makeCounter(seed: Int): CounterOwner =\n    new CounterOwner(new Counter(seed))",
    );
    let wrong_result_error = with_paired_jvm_tasty_batch_plan(
        &wrong_result,
        "WrongFactoryResult.scala",
        SourceId::from_index(1836),
        |_| (),
    )
    .expect_err("a mismatched factory result is rejected during source typing");
    assert!(matches!(
        wrong_result_error,
        PairedPlanError::BatchClass {
            phase: "method body typing",
            reason,
            ..
        } if reason.contains("ApplicationArgumentTypeMismatch")
    ));

    let with_field = SOURCE
        .replace(
            "class CounterOwnerFactory:",
            "class CounterOwnerFactory:\n  private val cached: Counter = new Counter(0)",
        )
        .replace(
            "new CounterOwner(makeCounter(seed))",
            "new CounterOwner(cached)",
        );
    let field_error = with_paired_jvm_tasty_batch_plan(
        &with_field,
        "FieldFactoryArgument.scala",
        SourceId::from_index(1837),
        |_| (),
    )
    .expect_err("a field argument remains outside the source-formal proof");
    assert!(matches!(
        field_error,
        PairedPlanError::BatchUnit {
            phase: "cross-class constructor-call identity proof",
            reason,
        } if reason.contains("constructor argument spelling must resolve to the caller formal")
    ));

    let null_argument = SOURCE.replace(
        "new CounterOwner(makeCounter(seed))",
        "new CounterOwner(null)",
    );
    let null_error = with_paired_jvm_tasty_batch_plan(
        &null_argument,
        "NullFactoryArgument.scala",
        SourceId::from_index(1838),
        |_| (),
    )
    .expect_err("null is not a source formal or factory-result proof");
    assert!(matches!(
        null_error,
        PairedPlanError::BatchUnit {
            phase: "cross-class constructor-call identity proof",
            reason,
        } if reason.contains("constructor argument must be the exact caller formal identifier")
    ));

    let branch_argument = SOURCE.replace(
        "new CounterOwner(makeCounter(seed))",
        "new CounterOwner(if seed > 0 then makeCounter(seed) else new Counter(seed))",
    );
    let branch_error = with_paired_jvm_tasty_batch_plan(
        &branch_argument,
        "BranchFactoryArgument.scala",
        SourceId::from_index(1839),
        |_| (),
    )
    .expect_err("a branch result is outside the bounded source proof");
    assert!(matches!(
        branch_error,
        PairedPlanError::BatchUnit {
            phase: "cross-class constructor-call identity proof",
            reason,
        } if reason.contains("constructor argument must be the exact caller formal identifier")
    ));
}
