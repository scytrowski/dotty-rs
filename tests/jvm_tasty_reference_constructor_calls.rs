use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::ConstantPoolEntry;
use dotty_classfile::reader::Reader as ClassReader;
use dotty_core::ast::TreeKind;
use dotty_core::{
    AstArena, Definitions, Packages, SemanticStore, SourceId, SourceText, SourceTypedIndex,
    SymbolId, SymbolInfo, TermRefTarget, TreeId, Type, TypeRefTarget, Typed, Untyped,
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
    stale_rhs: bool,
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
    if stale_rhs {
        let checkpoint = source_ast.checkpoint();
        let stale_rhs = source_ast.alloc(source_ast.get(wrap_tree).clone());
        source_ast.rollback_to(checkpoint);
        let TreeKind::DefDef(definition) = &mut source_ast.get_mut(wrap_tree).kind else {
            panic!("wrap method is a definition")
        };
        definition.rhs = Some(stale_rhs);
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
        typed_ast: typer.typed_ast(),
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
    if stale_rhs {
        assert!(matches!(
            validated.distinct_reference_method_signature(wrap),
            Err(dotty_tasty_pickler::PickleError::InvalidSourceTree { tree })
                if source_ast.try_get(tree).is_none()
        ));
        return;
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
fn rust_three_class_frontend_preserves_canonical_wrap_identities_before_cross_contract_rejection() {
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
    let direct_pickle_error = validated.pickle().unwrap_err();
    assert!(matches!(
        direct_pickle_error,
        dotty_tasty_pickler::PickleError::UnsupportedTypeReference { ty }
            if matches!(typer.store().types.get(ty), Type::TypeRef {
                target: TypeRefTarget::Symbol(found), ..
            } if typer.store().names.resolve(typer.store().symbols.get(*found).name.text()) == "Int")
    ));
    let plan_error = with_paired_jvm_tasty_batch_plan(
        SOURCE,
        "CounterOwnerFactory.scala",
        SourceId::from_index(17932),
        |_| (),
    )
    .unwrap_err();
    assert!(matches!(
        plan_error,
        PairedPlanError::BatchClass {
            class_index: 2,
            phase: "JVM/TASTy cross-contract validation",
            reason,
            ..
        } if reason.contains("UnsupportedTypeReference")
    ));
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
        false,
    );
    assert_distinct_signature_projection(
        &reordered,
        SourceId::from_index(17934),
        "Counter",
        "CounterOwner",
        "CounterOwnerFactory",
        false,
    );
}

#[test]
fn distinct_signature_rejects_a_rolled_back_source_rhs_id() {
    assert_distinct_signature_projection(
        SOURCE,
        SourceId::from_index(17936),
        "Counter",
        "CounterOwner",
        "CounterOwnerFactory",
        true,
    );
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
fn nested_constructor_application_reaches_the_existing_int_only_tasty_gate() {
    let source = SOURCE.replace(
        "new CounterOwner(counter)",
        "new CounterOwner(new Counter(1))",
    );
    assert_unit_failure(
        paired_failure(&source, 17942),
        "cross-class constructor-call identity proof",
        "constructor argument must be the exact caller formal identifier",
    );
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
