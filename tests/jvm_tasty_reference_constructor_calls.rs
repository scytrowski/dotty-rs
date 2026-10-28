use std::fs;
use std::path::PathBuf;
use std::process::Command;

use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::ConstantPoolEntry;
use dotty_classfile::reader::Reader as ClassReader;
use dotty_core::ast::TreeKind;
use dotty_core::{
    AstArena, Definitions, Packages, SemanticStore, SourceId, SourceSpan, SourceText,
    SourceTypedIndex, Span, SymbolId, SymbolInfo, TermRefTarget, TextRange, TreeId, Type,
    TypeRefTarget, Typed, Untyped,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty::Reader;
use dotty_tasty::ast::{
    APPLY_TAG, DEFDEF_TAG, IDENTTPT_TAG, NEW_TAG, PARAM_TAG, SELECTIN_TAG, SHAREDTYPE_TAG,
    TYPEREF_TAG,
};
use dotty_tasty::file::TastyFile;
use dotty_tasty::tasty::RenderedParamSig;
use dotty_tasty::term::{AstRefKind, RawTree};
use dotty_tasty_pickler::{
    ClassMemberPicklePlan, ClassStatePicklePlan, PrimaryConstructorInputPlan, SourceMetadata,
    SourceTypedBindings,
};
use dotty_typer::SourceTyper;

use dotty::jvm_tasty_plan::{PairedPlanError, with_paired_jvm_tasty_batch_plan};

const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_reference_constructor_calls/CounterOwnerFactory.scala");

const COUNTER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_reference_constructor_calls/scala39/example/Counter.class");
const COUNTER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_reference_constructor_calls/scala39/example/Counter.tasty");
const OWNER_CLASS: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_reference_constructor_calls/scala39/example/CounterOwner.class"
);
const OWNER_TASTY: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_reference_constructor_calls/scala39/example/CounterOwner.tasty"
);
const FACTORY_CLASS: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_reference_constructor_calls/scala39/example/CounterOwnerFactory.class"
);
const FACTORY_TASTY: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_reference_constructor_calls/scala39/example/CounterOwnerFactory.tasty"
);
const NESTED_SOURCE: &str =
    include_str!("fixtures/jvm_tasty_nested_constructor_calls/NestedConstructorCalls.scala");
const NESTED_COUNTER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_nested_constructor_calls/scala39/example/Counter.class");
const NESTED_COUNTER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_nested_constructor_calls/scala39/example/Counter.tasty");
const NESTED_OWNER_CLASS: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_nested_constructor_calls/scala39/example/CounterOwner.class"
);
const NESTED_OWNER_TASTY: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_nested_constructor_calls/scala39/example/CounterOwner.tasty"
);
const NESTED_FACTORY_CLASS: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_nested_constructor_calls/scala39/example/CounterOwnerFactory.class"
);
const NESTED_FACTORY_TASTY: &[u8] = include_bytes!(
    "fixtures/jvm_tasty_nested_constructor_calls/scala39/example/CounterOwnerFactory.tasty"
);

fn decode_class(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = ClassReader::new(bytes);
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

fn tasty(bytes: &[u8]) -> TastyFile<'_> {
    let tasty = TastyFile::parse_and_validate_scala_3_9(bytes).unwrap();
    assert_eq!(
        (
            tasty.header().major_version,
            tasty.header().minor_version,
            tasty.header().experimental_version
        ),
        (28, 9, 0)
    );
    tasty
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
        .expect("method owns a formal PARAM")
}

fn class_name<'a>(class: &'a ClassFile<'_>) -> &'a str {
    class.constant_pool.class_name(class.this_class).unwrap()
}

fn assert_classfile_link(class: &ClassFile<'_>, tasty: &TastyFile<'_>) {
    let link = class
        .tasty_link_id()
        .unwrap()
        .expect("Scala class has a TASTY attribute");
    assert_eq!(link.as_bytes(), &tasty.header().uuid);
}

#[test]
fn scala39_nested_constructor_oracle_pins_abi_bytecode_and_tasty_shape() {
    let counter = decode_class(NESTED_COUNTER_CLASS);
    let owner = decode_class(NESTED_OWNER_CLASS);
    let factory = decode_class(NESTED_FACTORY_CLASS);
    assert_eq!(
        [
            class_name(&counter),
            class_name(&owner),
            class_name(&factory)
        ],
        [
            "example/Counter",
            "example/CounterOwner",
            "example/CounterOwnerFactory"
        ]
    );
    assert_eq!(
        [
            counter.version.major,
            owner.version.major,
            factory.version.major
        ],
        [65, 65, 65]
    );
    let counter_tasty = tasty(NESTED_COUNTER_TASTY);
    let owner_tasty = tasty(NESTED_OWNER_TASTY);
    let factory_tasty = tasty(NESTED_FACTORY_TASTY);
    assert_classfile_link(&counter, &counter_tasty);
    assert_classfile_link(&owner, &owner_tasty);
    assert_classfile_link(&factory, &factory_tasty);

    assert_eq!(
        counter
            .constant_pool
            .utf8(method(&counter, "<init>").descriptor_index),
        Ok("(I)V")
    );
    assert_eq!(
        owner
            .constant_pool
            .utf8(method(&owner, "<init>").descriptor_index),
        Ok("(Lexample/Counter;)V")
    );
    let create = method(&factory, "create");
    assert_eq!(
        factory.constant_pool.utf8(create.descriptor_index),
        Ok("(I)Lexample/CounterOwner;")
    );
    assert_eq!(
        method_code(create),
        [
            0xbb, 0x00, 0x11, 0x59, 0xbb, 0x00, 0x13, 0x59, 0x1b, 0xb7, 0x00, 0x16, 0xb7, 0x00,
            0x19, 0xb0
        ]
    );

    let (create_address, body) = method_body(&factory_tasty, "create");
    assert_eq!(body.parameters.len(), 1);
    let seed_parameter = &body.parameters[0];
    assert_eq!(seed_parameter.tag(), PARAM_TAG);
    assert_eq!(
        factory_tasty.names().render(seed_parameter.name()).unwrap(),
        "seed"
    );
    let seed_type = seed_parameter.decode_body().unwrap().type_tree;
    assert_eq!(seed_type.nodes()[0].tag, IDENTTPT_TAG);
    assert_eq!(
        seed_type
            .name_refs()
            .into_iter()
            .map(|name| factory_tasty.names().render(name).unwrap())
            .collect::<Vec<_>>(),
        ["Int", "scala"]
    );
    assert_eq!(body.return_type.nodes()[0].tag, IDENTTPT_TAG);
    assert_eq!(
        body.return_type
            .name_refs()
            .into_iter()
            .map(|name| factory_tasty.names().render(name).unwrap())
            .collect::<Vec<_>>(),
        ["CounterOwner", "example"]
    );
    let RawTree::LengthNode(outer_node) = body.rhs.expect("create has an RHS") else {
        panic!("create outer expression is APPLY");
    };
    assert_eq!(outer_node.tag, APPLY_TAG);
    let outer = outer_node.decode_apply().unwrap();
    let [inner_argument] = outer.arguments.as_slice() else {
        panic!("Owner constructor has one argument");
    };
    let RawTree::LengthNode(outer_select_node) = outer.function else {
        panic!("outer constructor selection is a node");
    };
    assert_eq!(outer_select_node.tag, SELECTIN_TAG);
    let outer_select = outer_select_node.decode_select_in().unwrap();
    assert_eq!(
        factory_tasty
            .names()
            .render_signed_name(outer_select.name)
            .unwrap()
            .unwrap(),
        dotty_tasty::tasty::RenderedSignedName::Signed {
            original: "<init>".to_owned(),
            signature: dotty_tasty::tasty::RenderedNameSignature {
                parameters: [RenderedParamSig::TermParameter(
                    "example.Counter".to_owned()
                )]
                .into(),
                result: "example.CounterOwner".to_owned(),
            },
        }
    );
    assert!(matches!(
        outer_select.qualifier,
        RawTree::Ast { tag: NEW_TAG, .. }
    ));

    let RawTree::LengthNode(inner_node) = inner_argument else {
        panic!("the Owner argument is another APPLY");
    };
    assert_eq!(inner_node.tag, APPLY_TAG);
    let inner = inner_node.decode_apply().unwrap();
    let [seed_reference] = inner.arguments.as_slice() else {
        panic!("Counter constructor receives the original seed");
    };
    let RawTree::LengthNode(inner_select_node) = inner.function else {
        panic!("inner constructor selection is a node");
    };
    assert_eq!(inner_select_node.tag, SELECTIN_TAG);
    let inner_select = inner_select_node.decode_select_in().unwrap();
    assert_eq!(
        factory_tasty
            .names()
            .render_signed_name(inner_select.name)
            .unwrap()
            .unwrap(),
        dotty_tasty::tasty::RenderedSignedName::Signed {
            original: "<init>".to_owned(),
            signature: dotty_tasty::tasty::RenderedNameSignature {
                parameters: [RenderedParamSig::TermParameter("scala.Int".to_owned())].into(),
                result: "example.Counter".to_owned(),
            },
        }
    );
    assert!(matches!(
        inner_select.qualifier,
        RawTree::Ast { tag: NEW_TAG, .. }
    ));
    assert_eq!(seed_reference.ast_refs()[0].kind, AstRefKind::TermRefDirect);
    let tasty_index = factory_tasty.ast_address_index().unwrap();
    let seed_address = parameter_address(&tasty_index, create_address);
    assert_eq!(seed_reference.ast_refs()[0].address, seed_address);
    let nested_nodes = [APPLY_TAG, SELECTIN_TAG, NEW_TAG, PARAM_TAG]
        .into_iter()
        .flat_map(|tag| tasty_index.iter_nodes_with_tag(tag))
        .filter(|candidate| {
            let mut parent = Some(*candidate);
            while let Some(node) = parent {
                if node.offset as u32 == create_address {
                    return true;
                }
                parent = tasty_index.parent_of(node.offset as u32);
            }
            false
        })
        .map(|node| (node.offset, node.tag))
        .collect::<Vec<_>>();
    println!(
        "Scala nested TASTy addresses: create DEFDEF={create_address}, seed PARAM={seed_address}, descendant (address,tag)={nested_nodes:?}, tags APPLY={APPLY_TAG}, SELECTin={SELECTIN_TAG}, NEW={NEW_TAG}, PARAM={PARAM_TAG}, TERMREFdirect={}",
        dotty_tasty::ast::TERMREFDIRECT_TAG
    );
}

#[test]
fn nested_constructor_rust_preserves_the_one_session_trees_and_requires_the_batch_proof() {
    let source_id = SourceId::from_index(181501);
    let source_text = SourceText::new(NESTED_SOURCE).unwrap();
    let scanner = ContextualScanner::new(NESTED_SOURCE).unwrap();
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
        "NestedConstructorCalls.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("the exact nested source names cleanly: {outcome:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("the source has one package root");
    };
    assert_eq!(package.stats.len(), 3);

    let mut declarations = Vec::new();
    let mut create_tree = None;
    for &class_tree in &package.stats {
        let class = source_index.symbol_at(source_id, class_tree).unwrap();
        let TreeKind::TypeDef(definition) = &parsed.ast.get(class_tree).kind else {
            panic!("package member is a class");
        };
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            panic!("class body is a template");
        };
        let constructor = source_index
            .symbol_at(source_id, template.constructor)
            .unwrap();
        let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(template.constructor).kind
        else {
            panic!("primary constructor is a definition");
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
        if store.names.resolve(definition.name.as_name().text()) == "CounterOwnerFactory" {
            create_tree = template.body.iter().copied().find(|&tree| {
                matches!(&parsed.ast.get(tree).kind, TreeKind::DefDef(method)
                    if store.names.resolve(method.name.as_name().text()) == "create")
            });
        }
        declarations.push((class, class_tree, constructor, constructor_parameters));
    }
    let create_tree = create_tree.expect("the source Factory.create declaration");
    let create = source_index.symbol_at(source_id, create_tree).unwrap();
    let TreeKind::DefDef(create_definition) = &parsed.ast.get(create_tree).kind else {
        panic!("create is a method definition");
    };
    let [seed_tree] = create_definition.value_param_clauses[0].as_slice() else {
        panic!("create has its original seed formal");
    };
    let seed = source_index.symbol_at(source_id, *seed_tree).unwrap();

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    for (class, _, constructor, parameters) in &declarations {
        typer.complete_symbol(*class).unwrap();
        typer.complete_symbol(*constructor).unwrap();
        for &parameter in parameters {
            typer.complete_symbol(parameter).unwrap();
        }
    }
    typer.complete_symbol(create).unwrap();
    typer.complete_symbol(seed).unwrap();
    let source_rhs = create_definition.rhs.unwrap();
    let create_context = typer.expression_context_for(create).unwrap();
    let typed_rhs = typer.type_expression(source_rhs, create_context).unwrap();
    assert_eq!(
        typer.source_typed_index().get(source_id, source_rhs),
        Some(typed_rhs)
    );
    let TreeKind::Apply(outer) = &typer.typed_ast().get(typed_rhs).kind else {
        panic!("the original factory RHS types as the outer Owner APPLY");
    };
    let [typed_inner_tree] = outer.args.as_slice() else {
        panic!("the Owner primary constructor has one argument");
    };
    let TreeKind::Apply(inner) = &typer.typed_ast().get(*typed_inner_tree).kind else {
        panic!("the Owner argument remains the nested Counter APPLY");
    };
    let [typed_seed_tree] = inner.args.as_slice() else {
        panic!("the Counter primary constructor has one seed argument");
    };
    let Type::TermRef {
        target: outer_constructor_target,
        ..
    } = typer
        .store()
        .types
        .get(typer.typed_ast().get(outer.function).ty)
    else {
        panic!("the outer selected constructor has a canonical TermRef");
    };
    let Type::TermRef {
        target: inner_constructor_target,
        ..
    } = typer
        .store()
        .types
        .get(typer.typed_ast().get(inner.function).ty)
    else {
        panic!("the inner selected constructor has a canonical TermRef");
    };
    let outer_constructor = match *outer_constructor_target {
        TermRefTarget::Symbol(symbol) => symbol,
        _ => panic!("outer constructor resolves to a canonical symbol"),
    };
    let inner_constructor = match *inner_constructor_target {
        TermRefTarget::Symbol(symbol) => symbol,
        _ => panic!("inner constructor resolves to a canonical symbol"),
    };
    assert_eq!(outer_constructor, declarations[1].2);
    assert_eq!(inner_constructor, declarations[0].2);
    let Type::TermRef {
        target: TermRefTarget::Symbol(seed_target),
        ..
    } = typer
        .store()
        .types
        .get(typer.typed_ast().get(*typed_seed_tree).ty)
    else {
        panic!("the innermost argument is a canonical parameter TermRef");
    };
    assert_eq!(*seed_target, seed);

    let TreeKind::Apply(source_outer) = &parsed.ast.get(source_rhs).kind else {
        panic!("the original source Factory RHS is the outer Apply");
    };
    let TreeKind::Select(source_outer_selection) = &parsed.ast.get(source_outer.function).kind
    else {
        panic!("the outer source function is the Owner constructor selection");
    };
    let TreeKind::New(source_owner_new) = &parsed.ast.get(source_outer_selection.qualifier).kind
    else {
        panic!("the source Owner constructor selection has its original New");
    };
    let [source_inner_tree] = source_outer.args.as_slice() else {
        panic!("the outer source Apply has the nested source argument");
    };
    let TreeKind::Apply(source_inner) = &parsed.ast.get(*source_inner_tree).kind else {
        panic!("the nested source argument is Counter Apply");
    };
    let TreeKind::Select(source_inner_selection) = &parsed.ast.get(source_inner.function).kind
    else {
        panic!("the inner source function is the Counter constructor selection");
    };
    let TreeKind::New(source_counter_new) = &parsed.ast.get(source_inner_selection.qualifier).kind
    else {
        panic!("the source Counter constructor selection has its original New");
    };
    let [source_seed_tree] = source_inner.args.as_slice() else {
        panic!("the innermost source argument is seed");
    };
    let source_typed_index = typer.source_typed_index();
    for source_tree in [
        source_rhs,
        source_outer.function,
        source_outer_selection.qualifier,
        *source_inner_tree,
        source_inner.function,
        source_inner_selection.qualifier,
        *source_seed_tree,
        source_owner_new.tpt,
        source_counter_new.tpt,
    ] {
        assert!(
            source_typed_index.get(source_id, source_tree).is_some(),
            "original source tree {source_tree:?} has a same-session typed binding"
        );
    }
    println!(
        "nested source identities: classes={:?}, constructors={:?}, create={create:?}, seed={seed:?}, source_root={:?}, source_rhs={source_rhs:?}, source_owner_new={:?}, source_counter_new={:?}",
        declarations.iter().map(|item| item.0).collect::<Vec<_>>(),
        declarations.iter().map(|item| item.2).collect::<Vec<_>>(),
        parsed.root,
        source_outer_selection.qualifier,
        source_inner_selection.qualifier,
    );
    println!(
        "nested source→typed bindings: {:?}",
        [
            source_rhs,
            source_outer.function,
            source_outer_selection.qualifier,
            *source_inner_tree,
            source_inner.function,
            source_inner_selection.qualifier,
            *source_seed_tree,
            source_owner_new.tpt,
            source_counter_new.tpt,
        ]
        .map(|source_tree| (source_tree, source_typed_index.get(source_id, source_tree)))
    );
    println!(
        "nested typed identities: rhs={typed_rhs:?}, owner_constructor={outer_constructor:?}, counter_constructor={inner_constructor:?}, counter_argument={typed_seed_tree:?}"
    );

    let factory_class = declarations[2].0;
    let factory_tree = declarations[2].1;
    let factory_constructor = declarations[2].2;
    let TreeKind::TypeDef(factory_definition) = &parsed.ast.get(factory_tree).kind else {
        unreachable!();
    };
    let TreeKind::Template(factory_template) = &parsed.ast.get(factory_definition.rhs).kind else {
        unreachable!();
    };
    let TreeKind::DefDef(factory_constructor_definition) =
        &parsed.ast.get(factory_template.constructor).kind
    else {
        unreachable!();
    };
    let constructor_clauses = factory_constructor_definition
        .value_param_clauses
        .iter()
        .map(|clause| {
            clause
                .iter()
                .map(|&tree| {
                    source_index
                        .derived_symbol_at(factory_constructor, source_id, tree)
                        .unwrap()
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let planned_classes = declarations.iter().map(|item| item.0).collect::<Vec<_>>();
    let planned_methods = [create];
    let bindings = AuditBindings {
        index: typer.source_typed_index(),
        classes: &planned_classes,
        methods: &planned_methods,
    };
    let members = [ClassMemberPicklePlan::Method {
        symbol: create,
        source_tree: create_tree,
    }];
    let plan = ClassStatePicklePlan {
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        typed_bindings: &bindings,
        source: SourceMetadata {
            id: source_id,
            name: "NestedConstructorCalls.scala".to_owned(),
        },
        class: factory_class,
        class_tree: factory_tree,
        constructor: PrimaryConstructorInputPlan {
            symbol: factory_constructor,
            source_tree: factory_template.constructor,
            parameter_clauses: constructor_clauses,
        },
        members: &members,
    };
    let validated = plan
        .validate()
        .expect("the read-only class-state plan validates the one-session identities");
    let pickler_error = validated.pickle().expect_err(
        "a standalone class plan without the canonical batch receipt cannot emit nesting",
    );
    assert!(matches!(
        pickler_error,
        dotty_tasty_pickler::PickleError::Unsupported {
            feature: dotty_tasty_pickler::UnsupportedFeature::BodyExpression,
            symbol: Some(symbol),
        } if symbol == create
    ));
}

#[test]
fn nested_constructor_arity_and_type_controls_fail_during_source_typing() {
    let owner_arity =
        NESTED_SOURCE.replace("new CounterOwner(new Counter(seed))", "new CounterOwner()");
    assert_batch_failure(
        paired_failure(&owner_arity, 181504),
        2,
        "method body typing",
        "ApplicationArityMismatch",
    );
    let counter_argument_type = NESTED_SOURCE.replace("new Counter(seed)", "new Counter(true)");
    assert_batch_failure(
        paired_failure(&counter_argument_type, 181505),
        2,
        "method body typing",
        "ApplicationArgumentTypeMismatch",
    );
    let owner_argument_type = NESTED_SOURCE.replace(
        "new CounterOwner(new Counter(seed))",
        "new CounterOwner(seed)",
    );
    assert_batch_failure(
        paired_failure(&owner_argument_type, 181506),
        2,
        "method body typing",
        "ApplicationArgumentTypeMismatch",
    );
}

fn type_ref_symbol(store: &SemanticStore, ty: dotty_core::TypeId) -> SymbolId {
    let Type::TypeRef {
        target: TypeRefTarget::Symbol(symbol),
        ..
    } = store.types.get(ty)
    else {
        panic!(
            "expected a canonical class TypeRef, found {:?}",
            store.types.get(ty)
        );
    };
    *symbol
}

fn paired_failure(source: &str, source_id: u32) -> PairedPlanError {
    with_paired_jvm_tasty_batch_plan(
        source,
        "CounterOwnerFactory.scala",
        SourceId::from_index(source_id),
        |_| (),
    )
    .expect_err("the changed control must fail through the real paired source pipeline")
}

fn assert_unit_failure(error: PairedPlanError, expected_phase: &str, expected_reason: &str) {
    assert!(
        matches!(
            error,
            PairedPlanError::BatchUnit { phase, ref reason }
                if phase == expected_phase && reason.contains(expected_reason)
        ),
        "unexpected measured rejection: {error:?}"
    );
}

fn assert_batch_failure(
    error: PairedPlanError,
    expected_class_index: usize,
    expected_phase: &str,
    expected_reason: &str,
) {
    assert!(
        matches!(
            error,
            PairedPlanError::BatchClass {
                class_index,
                phase,
                ref reason,
                ..
            } if class_index == expected_class_index
                && phase == expected_phase
                && reason.contains(expected_reason)
        ),
        "unexpected measured rejection: {error:?}"
    );
}

struct AuditBindings<'a> {
    index: &'a SourceTypedIndex,
    classes: &'a [SymbolId],
    methods: &'a [SymbolId],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RhsProvenanceMutation {
    None,
    RolledBackSourceId,
    MissingSourcePosition,
    MissingTypedPosition,
    ForeignSourcePosition,
    MismatchedPositions,
}

impl SourceTypedBindings for AuditBindings<'_> {
    fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        self.index.get(source, tree)
    }

    fn planned_class_symbols(&self) -> &[SymbolId] {
        self.classes
    }

    fn planned_method_symbols(&self) -> &[SymbolId] {
        self.methods
    }
}

fn assert_distinct_signature_projection(
    source_text: &str,
    source_id: SourceId,
    parameter_class: &str,
    result_class: &str,
    factory_class: &str,
    rhs_mutation: RhsProvenanceMutation,
) {
    let source = SourceText::new(source_text).unwrap();
    let scanner = ContextualScanner::new(source_text).unwrap();
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
        "signature.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("source is named cleanly: {outcome:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("package root")
    };
    let mut classes = Vec::new();
    let mut factory = None;
    let mut factory_constructor = None;
    let mut wrap_tree = None;
    for &class_tree in &package.stats {
        let class = source_index.symbol_at(source_id, class_tree).unwrap();
        let TreeKind::TypeDef(definition) = &parsed.ast.get(class_tree).kind else {
            panic!("class source")
        };
        let class_name = store
            .names
            .resolve(definition.name.as_name().text())
            .to_owned();
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            panic!("class template")
        };
        let constructor = source_index
            .symbol_at(source_id, template.constructor)
            .unwrap();
        let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(template.constructor).kind
        else {
            panic!("constructor")
        };
        let constructor_parameter_clauses = constructor_definition
            .value_param_clauses
            .iter()
            .map(|clause| {
                clause
                    .iter()
                    .map(|&tree| {
                        source_index
                            .derived_symbol_at(constructor, source_id, tree)
                            .unwrap()
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let constructor_parameters = constructor_parameter_clauses
            .iter()
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        if class_name == factory_class {
            factory = Some((class, class_tree));
            factory_constructor = Some((
                constructor,
                template.constructor,
                constructor_parameter_clauses.clone(),
            ));
        }
        classes.push((class, constructor, constructor_parameters));
        if class_name == factory_class {
            for &member in &template.body {
                if matches!(&parsed.ast.get(member).kind, TreeKind::DefDef(definition) if store.names.resolve(definition.name.as_name().text()) == "wrap")
                {
                    wrap_tree = Some(member);
                }
            }
        }
    }
    let (factory_symbol, factory_tree) = factory.expect("factory class exists");
    let (constructor, constructor_tree, constructor_parameter_clauses) =
        factory_constructor.unwrap();
    let wrap_tree = wrap_tree.expect("wrap method exists");
    let wrap = source_index.symbol_at(source_id, wrap_tree).unwrap();
    let TreeKind::DefDef(wrap_definition) = &parsed.ast.get(wrap_tree).kind else {
        panic!("wrap is a method")
    };
    let [parameter_tree] = wrap_definition.value_param_clauses[0].as_slice() else {
        panic!("one formal")
    };
    let parameter = source_index.symbol_at(source_id, *parameter_tree).unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    for (class, class_constructor, parameters) in &classes {
        typer.complete_symbol(*class).unwrap();
        typer.complete_symbol(*class_constructor).unwrap();
        for &parameter in parameters {
            typer.complete_symbol(parameter).unwrap();
        }
    }
    typer.complete_symbol(wrap).unwrap();
    typer.complete_symbol(parameter).unwrap();
    let context = typer.expression_context_for(wrap).unwrap();
    let typed_rhs = typer
        .type_expression(wrap_definition.rhs.unwrap(), context)
        .unwrap();
    assert_eq!(
        typer
            .source_typed_index()
            .get(source_id, wrap_definition.rhs.unwrap()),
        Some(typed_rhs)
    );
    let source_typed_index = typer.source_typed_index();
    let mut source_ast = AstArena::new();
    for (_, tree) in parsed.ast.iter() {
        source_ast.alloc(tree.clone());
    }
    if rhs_mutation == RhsProvenanceMutation::RolledBackSourceId {
        let checkpoint = source_ast.checkpoint();
        let stale_rhs = source_ast.alloc(source_ast.get(wrap_tree).clone());
        source_ast.rollback_to(checkpoint);
        let TreeKind::DefDef(definition) = &mut source_ast.get_mut(wrap_tree).kind else {
            panic!("wrap method is a definition")
        };
        definition.rhs = Some(stale_rhs);
    }
    let source_rhs = wrap_definition.rhs.unwrap();
    match rhs_mutation {
        RhsProvenanceMutation::MissingSourcePosition => {
            source_ast.get_mut(source_rhs).position = None;
        }
        RhsProvenanceMutation::ForeignSourcePosition => {
            let position = source_ast.get(source_rhs).position.unwrap();
            source_ast.get_mut(source_rhs).position = Some(SourceSpan::new(
                SourceId::from_index(source_id.index() + 1),
                position.span(),
            ));
        }
        RhsProvenanceMutation::None
        | RhsProvenanceMutation::RolledBackSourceId
        | RhsProvenanceMutation::MissingTypedPosition
        | RhsProvenanceMutation::MismatchedPositions => {}
    }
    let mut typed_ast = AstArena::new();
    for (_, tree) in typer.typed_ast().iter() {
        typed_ast.alloc(tree.clone());
    }
    match rhs_mutation {
        RhsProvenanceMutation::MissingTypedPosition => {
            typed_ast.get_mut(typed_rhs).position = None;
        }
        RhsProvenanceMutation::MismatchedPositions => {
            typed_ast.get_mut(typed_rhs).position = Some(SourceSpan::new(
                source_id,
                Span::without_point(TextRange::new(0, 0).unwrap()),
            ));
        }
        RhsProvenanceMutation::None
        | RhsProvenanceMutation::RolledBackSourceId
        | RhsProvenanceMutation::MissingSourcePosition
        | RhsProvenanceMutation::ForeignSourcePosition => {}
    }
    let planned_classes = classes
        .iter()
        .map(|(class, _, _)| *class)
        .collect::<Vec<_>>();
    let planned_methods = [wrap];
    let bindings = AuditBindings {
        index: source_typed_index,
        classes: &planned_classes,
        methods: &planned_methods,
    };
    let members = [ClassMemberPicklePlan::Method {
        symbol: wrap,
        source_tree: wrap_tree,
    }];
    let plan = ClassStatePicklePlan {
        store: typer.store(),
        definitions: &definitions,
        source_ast: &source_ast,
        typed_ast: &typed_ast,
        source_index: &source_index,
        typed_bindings: &bindings,
        source: SourceMetadata {
            id: source_id,
            name: "signature.scala".to_owned(),
        },
        class: factory_symbol,
        class_tree: factory_tree,
        constructor: PrimaryConstructorInputPlan {
            symbol: constructor,
            source_tree: constructor_tree,
            parameter_clauses: constructor_parameter_clauses,
        },
        members: &members,
    };
    let validated = plan.validate().unwrap();
    match rhs_mutation {
        RhsProvenanceMutation::RolledBackSourceId => {
            assert!(matches!(
                validated.distinct_reference_method_signature(wrap),
                Err(dotty_tasty_pickler::PickleError::InvalidSourceTree { tree })
                    if source_ast.try_get(tree).is_none()
            ));
            return;
        }
        RhsProvenanceMutation::MissingSourcePosition
        | RhsProvenanceMutation::MissingTypedPosition
        | RhsProvenanceMutation::ForeignSourcePosition
        | RhsProvenanceMutation::MismatchedPositions => {
            assert!(matches!(
                validated.distinct_reference_method_signature(wrap),
                Err(dotty_tasty_pickler::PickleError::MethodBodySpan { .. })
            ));
            return;
        }
        RhsProvenanceMutation::None => {}
    }
    let signature = validated.distinct_reference_method_signature(wrap).unwrap();
    let name = |symbol| {
        typer
            .store()
            .names
            .resolve(typer.store().symbols.get(symbol).name.text())
            .to_owned()
    };
    assert_eq!(name(signature.parameter_class()), parameter_class);
    assert_eq!(name(signature.result_class()), result_class);
    assert_ne!(signature.parameter_class(), signature.result_class());
    assert_eq!(signature.typed_rhs(), typed_rhs);
    let projected = validated
        .project_distinct_reference_method_signature(wrap)
        .unwrap();
    let parameter_root = RawTree::decode(&mut Reader::new(
        &projected.fragment.ast.bytes()[projected.parameter_address as usize..],
    ))
    .unwrap();
    let result_root = RawTree::decode(&mut Reader::new(
        &projected.fragment.ast.bytes()[projected.result_address as usize..],
    ))
    .unwrap();
    assert!(matches!(parameter_root, RawTree::LengthNode(node) if node.tag == PARAM_TAG));
    assert!(matches!(
        result_root,
        RawTree::NatAst {
            tag: IDENTTPT_TAG,
            ..
        }
    ));
}

#[test]
fn scala39_three_class_oracle_pins_constructor_argument_tasty_and_jvm_abi() {
    let counter = decode_class(COUNTER_CLASS);
    let owner = decode_class(OWNER_CLASS);
    let factory = decode_class(FACTORY_CLASS);
    assert_eq!(
        [
            class_name(&counter),
            class_name(&owner),
            class_name(&factory)
        ],
        [
            "example/Counter",
            "example/CounterOwner",
            "example/CounterOwnerFactory"
        ]
    );
    assert_eq!(
        [
            counter.version.major,
            owner.version.major,
            factory.version.major
        ],
        [65, 65, 65]
    );
    assert!(counter.access_flags.is_public());
    assert!(owner.access_flags.is_public());
    assert!(factory.access_flags.is_public());

    let counter_tasty = tasty(COUNTER_TASTY);
    let owner_tasty = tasty(OWNER_TASTY);
    let factory_tasty = tasty(FACTORY_TASTY);
    assert_classfile_link(&counter, &counter_tasty);
    assert_classfile_link(&owner, &owner_tasty);
    assert_classfile_link(&factory, &factory_tasty);

    let owner_constructor = method(&owner, "<init>");
    assert_eq!(
        owner.constant_pool.utf8(owner_constructor.descriptor_index),
        Ok("(Lexample/Counter;)V")
    );
    assert!(owner_constructor.access_flags.is_public());
    let wrap = method(&factory, "wrap");
    assert_eq!(
        factory.constant_pool.utf8(wrap.descriptor_index),
        Ok("(Lexample/Counter;)Lexample/CounterOwner;")
    );
    assert!(wrap.access_flags.is_public());
    assert_eq!(
        method_code(wrap),
        [0xbb, 0x00, 0x11, 0x59, 0x2b, 0xb7, 0x00, 0x14, 0xb0]
    );
    assert_eq!(
        method_code(wrap)
            .iter()
            .filter(|&&opcode| opcode == 0xbb)
            .count(),
        1,
        "wrap emits one allocation for the returned Owner"
    );
    assert!((1..=u16::MAX).any(|raw| matches!(
        factory.constant_pool.get(dotty_classfile::constant_pool::ConstantPoolIndex(raw)),
        Some(ConstantPoolEntry::Methodref { class_index, name_and_type_index })
            if factory.constant_pool.class_name(*class_index) == Ok("example/CounterOwner")
                && factory.constant_pool.name_and_type(*name_and_type_index)
                    .is_ok_and(|(name, descriptor)| name == "<init>" && descriptor == "(Lexample/Counter;)V")
    )));

    let (wrap_address, wrap_body) = method_body(&factory_tasty, "wrap");
    let address_index = factory_tasty.ast_address_index().unwrap();
    assert_eq!(wrap_body.parameters.len(), 1);
    let formal = &wrap_body.parameters[0];
    assert_eq!(formal.tag(), PARAM_TAG);
    assert_eq!(
        factory_tasty.names().render(formal.name()).unwrap(),
        "counter"
    );
    let formal_type = formal.decode_body().unwrap().type_tree;
    assert_eq!(formal_type.nodes()[0].tag, IDENTTPT_TAG);
    assert_eq!(
        formal_type
            .name_refs()
            .into_iter()
            .map(|name| factory_tasty.names().render(name).unwrap())
            .collect::<Vec<_>>(),
        ["Counter", "example"]
    );
    assert!(
        formal_type
            .nodes()
            .iter()
            .any(|node| node.tag == TYPEREF_TAG)
    );
    let result_refs = wrap_body.return_type.ast_refs();
    let [result_ref] = result_refs.as_slice() else {
        panic!("factory result type is a shared type reference");
    };
    assert_eq!(result_ref.kind, AstRefKind::SharedType);
    assert!(address_index.get_node(result_ref.address).is_some());
    assert!(
        wrap_body
            .return_type
            .nodes()
            .iter()
            .any(|node| node.tag == SHAREDTYPE_TAG)
    );
    assert_eq!(
        wrap_body
            .return_type
            .name_refs()
            .into_iter()
            .map(|name| factory_tasty.names().render(name).unwrap())
            .collect::<Vec<_>>(),
        ["CounterOwner"]
    );
    let formal_address = parameter_address(&address_index, wrap_address);
    let argument_reference = {
        let Some(RawTree::LengthNode(apply_node)) = wrap_body.rhs.as_ref() else {
            panic!("wrap TASTy RHS is constructor APPLY");
        };
        assert_eq!(apply_node.tag, APPLY_TAG);
        let apply = apply_node.decode_apply().unwrap();
        let [argument] = apply.arguments.as_slice() else {
            panic!("wrap has one reference argument");
        };
        let RawTree::LengthNode(select_node) = &apply.function else {
            panic!("wrap applies a selected primary constructor");
        };
        assert_eq!(select_node.tag, SELECTIN_TAG);
        let select = select_node.decode_select_in().unwrap();
        let signed = factory_tasty
            .names()
            .render_signed_name(select.name)
            .unwrap()
            .unwrap();
        let RawTree::Ast {
            tag: NEW_TAG,
            child,
            ..
        } = &select.qualifier
        else {
            panic!("constructor receiver is NEW");
        };
        let dotty_tasty::tasty::RenderedSignedName::Signed {
            original,
            signature,
        } = signed
        else {
            panic!("constructor reference has a signed name");
        };
        assert_eq!(original, "<init>");
        assert_eq!(signature.result, "example.CounterOwner");
        assert_eq!(
            signature.parameters,
            [RenderedParamSig::TermParameter(
                "example.Counter".to_owned()
            )]
        );
        assert!(!select.underlying_type.ast_refs().is_empty());
        assert!(!child.ast_refs().is_empty());
        let argument_refs = argument.ast_refs();
        let [reference] = argument_refs.as_slice() else {
            panic!("constructor argument is one direct source-parameter reference");
        };
        (reference.kind, reference.address)
    };
    assert_eq!(argument_reference.0, AstRefKind::TermRefDirect);
    assert_eq!(argument_reference.1, formal_address);

    assert!(counter_tasty.ast_address_index().is_ok());
    assert!(owner_tasty.ast_address_index().is_ok());
    assert!(factory_tasty.ast_address_index().is_ok());
}

#[test]
fn rust_three_class_frontend_pickles_the_original_reference_constructor_call() {
    let source_id = SourceId::from_index(17931);
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
        "CounterOwnerFactory.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("one-source three-class fixture is named cleanly: {outcome:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("fixture has one package root");
    };
    assert_eq!(package.stats.len(), 3);

    let mut class_declarations = Vec::new();
    for &class_tree in &package.stats {
        let class = source_index.symbol_at(source_id, class_tree).unwrap();
        let TreeKind::TypeDef(definition) = &parsed.ast.get(class_tree).kind else {
            panic!("all three declarations are classes");
        };
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            panic!("ordinary class has a template");
        };
        let constructor = source_index
            .symbol_at(source_id, template.constructor)
            .unwrap();
        let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(template.constructor).kind
        else {
            panic!("class primary constructor has a source DefDef");
        };
        let constructor_parameters = constructor_definition
            .value_param_clauses
            .iter()
            .flatten()
            .map(|&parameter_tree| {
                source_index
                    .derived_symbol_at(constructor, source_id, parameter_tree)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        class_declarations.push((class, constructor, constructor_parameters));
    }

    let factory_tree = package.stats[2];
    let TreeKind::TypeDef(factory_definition) = &parsed.ast.get(factory_tree).kind else {
        unreachable!();
    };
    let TreeKind::Template(factory_template) = &parsed.ast.get(factory_definition.rhs).kind else {
        unreachable!();
    };
    let wrap_tree = factory_template
        .body
        .iter()
        .copied()
        .find(|&tree| {
            matches!(&parsed.ast.get(tree).kind, TreeKind::DefDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "wrap")
        })
        .unwrap();

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    for (class, constructor, parameters) in &class_declarations {
        typer.complete_symbol(*class).unwrap();
        typer.complete_symbol(*constructor).unwrap();
        for parameter in parameters {
            typer.complete_symbol(*parameter).unwrap();
        }
    }

    let counter = class_declarations[0].0;
    let counter_constructor = class_declarations[0].1;
    let owner = class_declarations[1].0;
    let owner_constructor = class_declarations[1].1;
    let factory = class_declarations[2].0;
    let wrap = source_index.symbol_at(source_id, wrap_tree).unwrap();
    let TreeKind::DefDef(wrap_definition) = &parsed.ast.get(wrap_tree).kind else {
        unreachable!();
    };
    let [wrap_parameter_tree] = wrap_definition.value_param_clauses[0].as_slice() else {
        panic!("wrap has one source formal");
    };
    let wrap_parameter = source_index
        .symbol_at(source_id, *wrap_parameter_tree)
        .unwrap();
    typer.complete_symbol(wrap).unwrap();
    typer.complete_symbol(wrap_parameter).unwrap();
    assert_eq!(
        typer.store().symbols.get(wrap).kind,
        dotty_core::SymbolKind::Method
    );
    assert_eq!(typer.store().symbols.get(wrap).owner, Some(factory));
    assert_eq!(typer.store().symbols.get(wrap_parameter).owner, Some(wrap));
    let SymbolInfo::Complete(wrap_info) = typer.store().symbols.get(wrap).info else {
        panic!("wrap signature completes");
    };
    let Type::Method(wrap_signature) = typer.store().types.get(wrap_info) else {
        panic!("wrap signature is a method type");
    };
    assert_eq!(wrap_signature.params.len(), 1);
    assert_eq!(
        type_ref_symbol(typer.store(), wrap_signature.params[0].ty),
        counter
    );
    assert_eq!(type_ref_symbol(typer.store(), wrap_signature.result), owner);
    let rhs = wrap_definition.rhs.unwrap();
    let context = typer.expression_context_for(wrap).unwrap();
    let typed_rhs = typer.type_expression(rhs, context).unwrap();
    assert_eq!(
        typer.source_typed_index().get(source_id, rhs),
        Some(typed_rhs)
    );
    let TreeKind::Apply(application) = &typer.typed_ast().get(typed_rhs).kind else {
        panic!("wrap typed RHS is Apply");
    };
    assert_eq!(application.args.len(), 1);
    let TreeKind::Select(selection) = &typer.typed_ast().get(application.function).kind else {
        panic!("wrap typed function is Select");
    };
    let TreeKind::New(new_expression) = &typer.typed_ast().get(selection.qualifier).kind else {
        panic!("wrap typed receiver is New");
    };
    let typed_argument = application.args[0];
    let Type::TermRef {
        target: TermRefTarget::Symbol(argument_symbol),
        ..
    } = typer
        .store()
        .types
        .get(typer.typed_ast().get(typed_argument).ty)
    else {
        panic!("wrap argument keeps its source formal TermRef");
    };
    assert_eq!(*argument_symbol, wrap_parameter);
    assert_eq!(
        type_ref_symbol(typer.store(), typer.typed_ast().get(typed_rhs).ty),
        owner
    );
    assert_eq!(
        type_ref_symbol(typer.store(), typer.typed_ast().get(selection.qualifier).ty),
        owner
    );
    assert_eq!(
        type_ref_symbol(typer.store(), typer.typed_ast().get(new_expression.tpt).ty),
        owner
    );
    let Type::TermRef {
        target: TermRefTarget::Symbol(selected_constructor),
        ..
    } = typer
        .store()
        .types
        .get(typer.typed_ast().get(application.function).ty)
    else {
        panic!("wrap SELECT resolves a canonical constructor symbol");
    };
    assert_eq!(*selected_constructor, owner_constructor);
    let TreeKind::Apply(source_application) = &parsed.ast.get(rhs).kind else {
        panic!("original wrap body remains Apply");
    };
    let source_function = source_application.function;
    let TreeKind::Select(source_selection) = &parsed.ast.get(source_function).kind else {
        panic!("original wrap function remains Select");
    };
    let source_new = source_selection.qualifier;
    let TreeKind::New(source_new_expression) = &parsed.ast.get(source_new).kind else {
        panic!("original wrap receiver remains New");
    };
    let source_type = source_new_expression.tpt;
    let source_argument = source_application.args[0];
    for (source_tree, typed_tree) in [
        (rhs, typed_rhs),
        (source_function, application.function),
        (source_new, selection.qualifier),
        (source_type, new_expression.tpt),
        (source_argument, typed_argument),
    ] {
        assert_eq!(
            typer.source_typed_index().get(source_id, source_tree),
            Some(typed_tree),
            "source-to-typed identity for source {source_tree:?}"
        );
    }

    assert_eq!(
        typer.store().symbols.get(counter_constructor).owner,
        Some(counter)
    );
    assert_eq!(
        typer.store().symbols.get(owner_constructor).owner,
        Some(owner)
    );
    let factory_constructor_tree = factory_template.constructor;
    let TreeKind::DefDef(factory_constructor_definition) =
        &parsed.ast.get(factory_constructor_tree).kind
    else {
        unreachable!();
    };
    let factory_constructor = source_index
        .symbol_at(source_id, factory_constructor_tree)
        .unwrap();
    let constructor_clauses = factory_constructor_definition
        .value_param_clauses
        .iter()
        .map(|clause| {
            clause
                .iter()
                .map(|&tree| {
                    source_index
                        .derived_symbol_at(factory_constructor, source_id, tree)
                        .unwrap()
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let factory_members = [ClassMemberPicklePlan::Method {
        symbol: wrap,
        source_tree: wrap_tree,
    }];
    let source_typed_index: &SourceTypedIndex = typer.source_typed_index();
    let planned_classes = [counter, owner, factory];
    let planned_methods = [wrap];
    let source_typed_bindings = AuditBindings {
        index: source_typed_index,
        classes: &planned_classes,
        methods: &planned_methods,
    };
    let class_state = ClassStatePicklePlan {
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        typed_bindings: &source_typed_bindings,
        source: SourceMetadata {
            id: source_id,
            name: "CounterOwnerFactory.scala".to_owned(),
        },
        class: factory,
        class_tree: factory_tree,
        constructor: PrimaryConstructorInputPlan {
            symbol: factory_constructor,
            source_tree: factory_constructor_tree,
            parameter_clauses: constructor_clauses,
        },
        members: &factory_members,
    };
    let validated = class_state
        .validate()
        .expect("the exact source/typed ClassState plan validates");
    let signature = validated
        .distinct_reference_method_signature(wrap)
        .expect("the original method has a distinct same-batch reference signature");
    assert_eq!(signature.parameter(), wrap_parameter);
    assert_eq!(signature.parameter_class(), counter);
    assert_eq!(signature.result_class(), owner);
    assert_ne!(signature.parameter_class(), signature.result_class());
    assert_eq!(signature.source_rhs(), rhs);
    assert_eq!(signature.typed_rhs(), typed_rhs);
    assert_eq!(
        source_index.symbol_at(source_id, wrap_tree),
        Some(signature.method())
    );
    let projected = validated
        .project_distinct_reference_method_signature(wrap)
        .expect("the formal and result type roots project without a method body");
    let parameter_root = RawTree::decode(&mut Reader::new(
        &projected.fragment.ast.bytes()[projected.parameter_address as usize..],
    ))
    .unwrap();
    let result_root = RawTree::decode(&mut Reader::new(
        &projected.fragment.ast.bytes()[projected.result_address as usize..],
    ))
    .unwrap();
    assert!(matches!(parameter_root, RawTree::LengthNode(node) if node.tag == PARAM_TAG));
    assert!(matches!(
        result_root,
        RawTree::NatAst {
            tag: IDENTTPT_TAG,
            ..
        }
    ));
    let output = validated
        .pickle()
        .expect("the authentic Counter formal argument is serializable");
    let tasty = TastyFile::parse_and_validate_scala_3_9(output.bytes())
        .expect("the completed Factory TASTy is structurally valid");
    assert_eq!(tasty.encode_validated().unwrap(), output.bytes());
    let (method_address, wrap_body) = method_body(&tasty, "wrap");
    let parameter_address = parameter_address(&tasty.ast_address_index().unwrap(), method_address);
    let RawTree::LengthNode(application_node) = wrap_body.rhs.expect("wrap has its source RHS")
    else {
        panic!("wrap RHS is an APPLY node");
    };
    assert_eq!(application_node.tag, APPLY_TAG);
    let application = application_node.decode_apply().unwrap();
    assert_eq!(application.arguments.len(), 1);
    let RawTree::LengthNode(select_node) = application.function else {
        panic!("constructor function is SELECTin");
    };
    assert_eq!(select_node.tag, SELECTIN_TAG);
    let select = select_node.decode_select_in().unwrap();
    let signed = tasty
        .names()
        .render_signed_name(select.name)
        .unwrap()
        .unwrap();
    let dotty_tasty::tasty::RenderedSignedName::Signed {
        original,
        signature,
    } = signed
    else {
        panic!("constructor reference is a signed name");
    };
    assert_eq!(original, "<init>");
    assert_eq!(signature.result, "example.CounterOwner");
    assert_eq!(
        signature.parameters,
        [RenderedParamSig::TermParameter(
            "example.Counter".to_owned()
        )]
    );
    let RawTree::Ast {
        tag: NEW_TAG,
        child: new_type,
        ..
    } = &select.qualifier
    else {
        panic!("constructor receiver is NEW");
    };
    assert!(matches!(
        new_type.as_ref(),
        RawTree::NatAst {
            tag: IDENTTPT_TAG,
            ..
        }
    ));
    assert_eq!(
        new_type
            .name_refs()
            .into_iter()
            .map(|name| tasty.names().render(name).unwrap())
            .collect::<Vec<_>>(),
        ["CounterOwner"]
    );
    assert!(!select.underlying_type.ast_refs().is_empty());
    let [argument] = application.arguments.as_slice() else {
        panic!("one original argument is encoded");
    };
    let argument_refs = argument.ast_refs();
    let [argument_reference] = argument_refs.as_slice() else {
        panic!("the original formal is one direct term reference");
    };
    assert_eq!(
        argument_reference.kind,
        dotty_tasty::term::AstRefKind::TermRefDirect
    );
    assert_eq!(argument_reference.address, parameter_address);

    let batch_bytes = with_paired_jvm_tasty_batch_plan(
        SOURCE,
        "CounterOwnerFactory.scala",
        SourceId::from_index(17932),
        |batch| {
            assert_eq!(batch.classes.len(), 3);
            assert_eq!(batch.constructor_call_proofs.len(), 1);
            batch.classes[2]
                .tasty_plan
                .pickle()
                .expect("paired Factory plan emits the same body")
                .bytes()
                .to_vec()
        },
    )
    .expect("all three source classes pass paired JVM/TASTy validation");
    assert_eq!(batch_bytes, output.bytes());
}

#[test]
fn pinned_scala_reads_all_three_rust_reference_constructor_sidecars() {
    if std::env::var_os("DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return;
    }
    let files = with_paired_jvm_tasty_batch_plan(
        SOURCE,
        "CounterOwnerFactory.scala",
        SourceId::from_index(17960),
        |batch| {
            batch
                .classes
                .iter()
                .map(|class| {
                    (
                        class.tasty_path.clone(),
                        class
                            .tasty_plan
                            .pickle()
                            .expect("each genuine source sidecar is complete")
                            .bytes()
                            .to_vec(),
                    )
                })
                .collect::<Vec<_>>()
        },
    )
    .expect("the original one-source batch validates");
    run_reference_constructor_scala_oracle(&files);
}

fn run_reference_constructor_scala_oracle(files: &[(String, Vec<u8>)]) {
    let scala_root = PathBuf::from(
        std::env::var_os("SCALA39_ROOT").expect("pinned Scala root for explicit oracle"),
    );
    let revision = Command::new("git")
        .arg("-C")
        .arg(&scala_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("read pinned Scala source revision");
    assert!(revision.status.success());
    assert_eq!(
        String::from_utf8_lossy(&revision.stdout).trim(),
        "777528f19a58e794c9954a42f433373472ec57f8"
    );
    let compiler = scala_root.join("dist/target/universal/stage/bin/scalac");
    let version = Command::new(&compiler)
        .arg("-version")
        .output()
        .expect("query pinned Scala 3.9 compiler version");
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).contains("3.9.0-bin-SNAPSHOT-git-777528f"));

    let root = std::env::var_os("DOTTY_TASTY_PICKLER_ORACLE_TMP")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    let temporary = root.join(format!(
        "dotty-issue-1796-scala-oracle-{}",
        std::process::id()
    ));
    fs::remove_dir_all(&temporary).ok();
    let candidate = temporary.join("candidate");
    let compiled = temporary.join("compiled");
    fs::create_dir_all(&candidate).unwrap();
    fs::create_dir_all(&compiled).unwrap();
    for (path, bytes) in files {
        let path = candidate.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let factory_tasty = candidate.join("example/CounterOwnerFactory.tasty");
    let decompile = Command::new(&compiler)
        .arg("-decompile")
        .arg(&factory_tasty)
        .output()
        .expect("decompile Rust Factory TASTy with pinned Scala 3.9");
    assert!(
        decompile.status.success(),
        "Scala -decompile rejected Rust Factory TASTy: {}",
        String::from_utf8_lossy(&decompile.stderr)
    );
    let decompiled = String::from_utf8_lossy(&decompile.stdout);
    assert!(decompiled.contains("wrap"), "{decompiled}");
    assert!(decompiled.contains("CounterOwner"), "{decompiled}");
    let from_tasty = Command::new(&compiler)
        .arg("-from-tasty")
        .arg("-classpath")
        .arg(&candidate)
        .arg("-d")
        .arg(&compiled)
        .args([
            candidate.join("example/Counter.tasty"),
            candidate.join("example/CounterOwner.tasty"),
            factory_tasty,
        ])
        .output()
        .expect("consume all Rust sidecars with pinned Scala 3.9");
    assert!(
        from_tasty.status.success(),
        "Scala -from-tasty rejected the three Rust sidecars: {}",
        String::from_utf8_lossy(&from_tasty.stderr)
    );
    fs::remove_dir_all(temporary).unwrap();
}

#[test]
fn renamed_and_reordered_three_class_sources_do_not_depend_on_fixture_names_or_order() {
    let renamed = SOURCE
        .replace("package example", "package renamed")
        .replace("CounterOwnerFactory", "WrapperMaker")
        .replace("CounterOwner", "WrappedCounter")
        .replace("Counter", "Tally");
    let reordered = SOURCE
        .replace(
            "class Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n\nclass CounterOwner(counter: Counter):\n  private val inner: Counter = counter\n  def read(): Int = inner.get()\n\nclass CounterOwnerFactory:",
            "class CounterOwner(counter: Counter):\n  private val inner: Counter = counter\n  def read(): Int = inner.get()\n\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n\nclass CounterOwnerFactory:",
        );

    assert_distinct_signature_projection(
        &renamed,
        SourceId::from_index(17933),
        "Tally",
        "WrappedTally",
        "WrapperMaker",
        RhsProvenanceMutation::None,
    );
    assert_distinct_signature_projection(
        &reordered,
        SourceId::from_index(17934),
        "Counter",
        "CounterOwner",
        "CounterOwnerFactory",
        RhsProvenanceMutation::None,
    );
    for (source, source_id) in [(&renamed, 17961), (&reordered, 17962)] {
        let sidecars = with_paired_jvm_tasty_batch_plan(
            source,
            "renamed-reference-constructor.scala",
            SourceId::from_index(source_id),
            |batch| {
                assert_eq!(batch.constructor_call_proofs.len(), 1);
                batch
                    .classes
                    .iter()
                    .map(|class| {
                        class
                            .tasty_plan
                            .pickle()
                            .expect("renamed and reordered source plan emits TASTy")
                            .bytes()
                            .to_vec()
                    })
                    .collect::<Vec<_>>()
            },
        )
        .expect("renamed and reordered source uses the same semantic proof");
        assert_eq!(sidecars.len(), 3);
        for sidecar in sidecars {
            assert!(TastyFile::parse_and_validate_scala_3_9(&sidecar).is_ok());
        }
    }
}

#[test]
fn distinct_signature_rejects_a_rolled_back_source_rhs_id() {
    assert_distinct_signature_projection(
        SOURCE,
        SourceId::from_index(17936),
        "Counter",
        "CounterOwner",
        "CounterOwnerFactory",
        RhsProvenanceMutation::RolledBackSourceId,
    );
}

#[test]
fn distinct_signature_rejects_missing_foreign_and_mismatched_rhs_positions() {
    for (offset, mutation) in [
        RhsProvenanceMutation::MissingSourcePosition,
        RhsProvenanceMutation::MissingTypedPosition,
        RhsProvenanceMutation::ForeignSourcePosition,
        RhsProvenanceMutation::MismatchedPositions,
    ]
    .into_iter()
    .enumerate()
    {
        assert_distinct_signature_projection(
            SOURCE,
            SourceId::from_index(17937 + offset as u32),
            "Counter",
            "CounterOwner",
            "CounterOwnerFactory",
            mutation,
        );
    }
}

#[test]
fn zero_argument_constructor_call_is_rejected_by_method_body_typing() {
    let source = SOURCE.replace("new CounterOwner(counter)", "new CounterOwner()");
    assert_batch_failure(
        paired_failure(&source, 17935),
        2,
        "method body typing",
        "ApplicationArityMismatch",
    );
}

#[test]
fn wrong_constructor_argument_type_is_rejected_by_method_body_typing() {
    let source = SOURCE.replace("new CounterOwner(counter)", "new CounterOwner(1)");
    assert_batch_failure(
        paired_failure(&source, 17936),
        2,
        "method body typing",
        "ApplicationArgumentTypeMismatch",
    );
}

#[test]
fn mismatched_factory_result_is_rejected_by_jvm_tasty_cross_contract_validation() {
    let source = SOURCE.replace(
        "def wrap(counter: Counter): CounterOwner =",
        "def wrap(counter: Counter): Counter =",
    );
    assert_unit_failure(
        paired_failure(&source, 17937),
        "cross-class constructor-call identity proof",
        "method signature result must retain the exact target class TypeRef",
    );
}

#[test]
fn private_owner_constructor_is_rejected_by_tasty_plan_validation() {
    let source = SOURCE.replace(
        "class CounterOwner(counter: Counter):",
        "class CounterOwner private (counter: Counter):",
    );
    assert_unit_failure(
        paired_failure(&source, 17938),
        "cross-class constructor-call identity proof",
        "target constructor must be one plain public non-generic formal clause",
    );
}

#[test]
fn abstract_owner_instantiation_is_rejected_by_method_body_typing() {
    let source = SOURCE.replace(
        "class CounterOwner(counter",
        "abstract class CounterOwner(counter",
    );
    assert_batch_failure(
        paired_failure(&source, 17939),
        2,
        "method body typing",
        "AbstractClassInstantiation",
    );
}

#[test]
fn generic_owner_primary_constructor_is_rejected_during_source_collection() {
    let source = SOURCE.replace(
        "class CounterOwner(counter",
        "class CounterOwner[A](counter",
    );
    assert_batch_failure(
        paired_failure(&source, 17940),
        1,
        "source collection",
        "generic primary constructors are unsupported",
    );
}

#[test]
fn constructor_val_accessor_is_rejected_by_jvm_unit_validation() {
    let source = SOURCE.replace(
        "class CounterOwner(counter",
        "class CounterOwner(val counter",
    );
    assert!(matches!(
        paired_failure(&source, 17941),
        PairedPlanError::BatchUnit {
            phase: "complete JVM unit validation",
            reason,
        } if reason.contains("AccessorFieldSignature")
    ));
}

#[test]
fn nested_constructor_proof_serializes_the_original_nested_tasty_expression() {
    let output = with_paired_jvm_tasty_batch_plan(
        NESTED_SOURCE,
        "NestedConstructorCalls.scala",
        SourceId::from_index(181604),
        |batch| {
            assert_eq!(batch.constructor_call_proofs.len(), 1);
            batch.classes[2]
                .tasty_plan
                .pickle()
                .expect("the proof-backed nested factory body is serialized")
                .bytes()
                .to_vec()
        },
    )
    .expect("the source-proven two-level constructor body passes paired validation");
    let factory = tasty(&output);
    let (create_address, body) = method_body(&factory, "create");
    assert_eq!(body.parameters.len(), 1);
    assert_eq!(body.return_type.nodes()[0].tag, IDENTTPT_TAG);
    let RawTree::LengthNode(outer_node) = body.rhs.expect("create has an RHS") else {
        panic!("Factory.create body is the original outer constructor Apply");
    };
    assert_eq!(outer_node.tag, APPLY_TAG);
    let outer = outer_node.decode_apply().unwrap();
    let [inner_argument] = outer.arguments.as_slice() else {
        panic!("Owner constructor has one nested constructor argument");
    };
    let RawTree::LengthNode(inner_node) = inner_argument else {
        panic!("the original Counter constructor Apply remains the Owner argument");
    };
    assert_eq!(inner_node.tag, APPLY_TAG);
    let inner = inner_node.decode_apply().unwrap();
    let [seed_reference] = inner.arguments.as_slice() else {
        panic!("Counter constructor receives the original seed formal");
    };
    assert_eq!(seed_reference.ast_refs()[0].kind, AstRefKind::TermRefDirect);
    let index = factory.ast_address_index().unwrap();
    assert_eq!(
        seed_reference.ast_refs()[0].address,
        parameter_address(&index, create_address)
    );
}

#[test]
fn pinned_scala_reads_and_from_tasty_compiles_the_rust_nested_factory_sidecar() {
    if std::env::var_os("DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return;
    }
    let files = with_paired_jvm_tasty_batch_plan(
        NESTED_SOURCE,
        "NestedConstructorCalls.scala",
        SourceId::from_index(181606),
        |batch| {
            batch
                .classes
                .iter()
                .map(|class| {
                    (
                        class.tasty_path.clone(),
                        class
                            .tasty_plan
                            .pickle()
                            .expect("the nested source session emits all complete sidecars")
                            .bytes()
                            .to_vec(),
                    )
                })
                .collect::<Vec<_>>()
        },
    )
    .expect("nested source batch passes paired validation");
    run_nested_constructor_scala_oracle(&files);
}

fn run_nested_constructor_scala_oracle(files: &[(String, Vec<u8>)]) {
    let scala_root = PathBuf::from(
        std::env::var_os("SCALA39_ROOT").expect("pinned Scala root for explicit oracle"),
    );
    let revision = Command::new("git")
        .arg("-C")
        .arg(&scala_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("read pinned Scala source revision");
    assert!(revision.status.success());
    assert_eq!(
        String::from_utf8_lossy(&revision.stdout).trim(),
        "777528f19a58e794c9954a42f433373472ec57f8"
    );
    let compiler = scala_root.join("dist/target/universal/stage/bin/scalac");
    let version = Command::new(&compiler)
        .arg("-version")
        .output()
        .expect("query pinned Scala 3.9 compiler version");
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).contains("3.9.0-bin-SNAPSHOT-git-777528f"));

    let root = std::env::var_os("DOTTY_TASTY_PICKLER_ORACLE_TMP")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
        .expect("oracle scratch directory must be explicitly set outside /tmp");
    assert!(
        !root.starts_with("/tmp"),
        "Scala oracle scratch directory must remain outside /tmp"
    );
    let temporary = root.join(format!(
        "dotty-issue-1817-scala-oracle-{}",
        std::process::id()
    ));
    fs::remove_dir_all(&temporary).ok();
    let candidate = temporary.join("candidate");
    let compiled = temporary.join("compiled");
    fs::create_dir_all(&candidate).unwrap();
    fs::create_dir_all(&compiled).unwrap();
    for (path, bytes) in files {
        let path = candidate.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let factory_tasty = candidate.join("example/CounterOwnerFactory.tasty");
    let decompile = Command::new(&compiler)
        .arg("-decompile")
        .arg(&factory_tasty)
        .output()
        .expect("decompile nested Factory TASTy with pinned Scala 3.9");
    assert!(
        decompile.status.success(),
        "Scala -decompile rejected nested Factory TASTy: {}",
        String::from_utf8_lossy(&decompile.stderr)
    );
    let decompiled = String::from_utf8_lossy(&decompile.stdout);
    assert!(decompiled.contains("create"), "{decompiled}");
    assert!(decompiled.contains("CounterOwner"), "{decompiled}");
    assert!(decompiled.contains("Counter"), "{decompiled}");
    let from_tasty = Command::new(&compiler)
        .arg("-from-tasty")
        .arg("-classpath")
        .arg(&candidate)
        .arg("-d")
        .arg(&compiled)
        .args([
            candidate.join("example/Counter.tasty"),
            candidate.join("example/CounterOwner.tasty"),
            factory_tasty,
        ])
        .output()
        .expect("consume nested Rust sidecars with pinned Scala 3.9");
    assert!(
        from_tasty.status.success(),
        "Scala -from-tasty rejected nested Rust sidecars: {}",
        String::from_utf8_lossy(&from_tasty.stderr)
    );
    fs::remove_dir_all(temporary).unwrap();
}

#[test]
fn nested_constructor_application_with_a_literal_argument_is_rejected_at_source_shape() {
    let source = NESTED_SOURCE.replace("new Counter(seed)", "new Counter(1)");
    assert_unit_failure(
        paired_failure(&source, 181605),
        "cross-class constructor-call identity proof",
        "constructor argument must resolve directly to the caller formal",
    );
}

#[test]
fn nested_constructor_proof_rejects_computed_field_factory_null_and_third_level_arguments() {
    let branch = NESTED_SOURCE.replace(
        "new CounterOwner(new Counter(seed))",
        "new CounterOwner(if seed > 0 then new Counter(seed) else new Counter(0))",
    );
    let factory = NESTED_SOURCE
        .replace(
            "class CounterOwnerFactory:\n  def create",
            "class CounterOwnerFactory:\n  def build(seed: Int): Counter = new Counter(seed)\n  def create",
        )
        .replace("new CounterOwner(new Counter(seed))", "new CounterOwner(build(seed))");
    let field = NESTED_SOURCE
        .replace(
            "class CounterOwnerFactory:\n  def create",
            "class CounterOwnerFactory:\n  private var stored: Int = 1\n  def create",
        )
        .replace(
            "new CounterOwner(new Counter(seed))",
            "new CounterOwner(new Counter(stored))",
        );
    let null = NESTED_SOURCE.replace(
        "new CounterOwner(new Counter(seed))",
        "new CounterOwner(null)",
    );
    let third_level = NESTED_SOURCE
        .replace(
            "class CounterOwner(counter: Counter):\n  private val inner: Counter = counter\n  def read(): Int = inner.get()",
            "class CounterNest(counter: Counter)\nclass CounterOwner(counter: CounterNest):\n  def read(): Int = 0",
        )
        .replace(
            "new CounterOwner(new Counter(seed))",
            "new CounterOwner(new CounterNest(new Counter(seed)))",
        );

    assert_unit_failure(
        paired_failure(&branch, 181607),
        "cross-class constructor-call identity proof",
        "constructor argument",
    );
    assert_batch_failure(
        paired_failure(&factory, 181608),
        2,
        "JVM/TASTy cross-contract validation",
        "BodyExpression",
    );

    for (offset, source) in [field, null, third_level].iter().enumerate() {
        assert_unit_failure(
            paired_failure(source, 181609 + offset as u32),
            "cross-class constructor-call identity proof",
            "constructor argument",
        );
    }
}

#[test]
fn constructor_default_argument_is_rejected_during_constructor_planning() {
    let source = SOURCE.replace(
        "class CounterOwner(counter: Counter):",
        "class CounterOwner(counter: Counter = new Counter(0)):",
    );
    assert_batch_failure(
        paired_failure(&source, 17943),
        1,
        "constructor planning",
        "SourceShape",
    );
}

#[test]
fn constructor_var_accessor_is_rejected_by_jvm_unit_validation() {
    let source = SOURCE.replace(
        "class CounterOwner(counter",
        "class CounterOwner(var counter",
    );
    assert!(matches!(
        paired_failure(&source, 17944),
        PairedPlanError::BatchUnit {
            phase: "complete JVM unit validation",
            reason,
        } if reason.contains("AccessorFieldSignature")
    ));
}

#[test]
fn secondary_constructor_is_rejected_during_member_planning() {
    let source = SOURCE.replace(
        "class CounterOwner(counter: Counter):",
        "class CounterOwner(counter: Counter):\n  def this() = this(new Counter(0))",
    );
    assert_batch_failure(
        paired_failure(&source, 17945),
        1,
        "member planning",
        "method is not owned by its declaring class",
    );
}

#[test]
fn overloaded_secondary_constructor_is_rejected_during_member_planning() {
    let source = SOURCE.replace(
        "class CounterOwner(counter: Counter):",
        "class CounterOwner(counter: Counter):\n  def this(counter: Counter, flag: Boolean) = this(counter)",
    );
    assert_batch_failure(
        paired_failure(&source, 17946),
        1,
        "member planning",
        "method is not owned by its declaring class",
    );
}

#[test]
fn contextual_primary_constructor_is_rejected_during_constructor_planning() {
    let source = SOURCE
        .replace(
            "class CounterOwner(counter: Counter):",
            "class CounterOwner(using counter: Counter):",
        )
        .replace(
            "new CounterOwner(counter)",
            "new CounterOwner(using counter)",
        );
    assert_batch_failure(
        paired_failure(&source, 17947),
        1,
        "constructor planning",
        "SourceShape",
    );
}
