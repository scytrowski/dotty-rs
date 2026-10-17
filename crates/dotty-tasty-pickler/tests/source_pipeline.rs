use dotty_core::ast::*;
use dotty_core::*;
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty_pickler::*;
use dotty_typer::SourceTyper;
use std::path::PathBuf;
use std::process::Command;

const ANSWER: &str = include_str!("../../../tests/fixtures/tasty_pickler/Answer.scala");
const MULTI: &str = include_str!("../../../tests/fixtures/tasty_pickler/Multi.scala");
const PARAMETERS: &str = include_str!("../../../tests/fixtures/tasty_pickler/Parameters.scala");
const PARAMETER_DECL: &str =
    include_str!("../../../tests/fixtures/tasty_pickler/ParameterDecl.scala");
const REFERENCES: &str = include_str!("../../../tests/fixtures/tasty_pickler/References.scala");
const EXPLICIT_THIS_REFERENCES: &str =
    "package example\nclass References:\n  def first: Int = this.second\n  def second: Int = 42\n";
const OPERATIONS: &str = "package example\nclass Operations:\n  def use: Int = identity(42)\n  def identity(x: Int): Int = x\n  def propagate(x: Int): Int = identity(x)\n  def explicit: Int = this.identity(42)\n";
const RUN_SCALA_ORACLE: &str = "DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE";

fn source_outputs(
    text: &str,
    edit: impl FnOnce(&mut PicklePlan<'_>),
) -> Result<(DeclarationSkeleton, PickledTasty), PickleError> {
    source_outputs_named(text, "Answer.scala", edit)
}

fn source_outputs_named(
    text: &str,
    source_name: &str,
    edit: impl FnOnce(&mut PicklePlan<'_>),
) -> Result<(DeclarationSkeleton, PickledTasty), PickleError> {
    let source = SourceId::from_index(7);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let parsed = parse_compilation_unit(
        SourceText::new(text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source,
        source_name,
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        other => panic!("source fixture naming failed: {other:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!()
    };
    let [class_tree] = package.stats.as_slice() else {
        panic!()
    };
    let class = index.symbol_at(source, *class_tree).unwrap();
    let TreeKind::TypeDef(class_definition) = &parsed.ast.get(*class_tree).kind else {
        panic!()
    };
    let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
        panic!()
    };
    let constructor = index.symbol_at(source, template.constructor).unwrap();
    let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(template.constructor).kind
    else {
        panic!()
    };
    let clauses = |definition: &DefDef<Untyped>| {
        definition
            .value_param_clauses
            .iter()
            .map(|clause| {
                clause
                    .iter()
                    .map(|tree| index.symbol_at(source, *tree).unwrap())
                    .collect()
            })
            .collect()
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.complete_symbol(class).unwrap();
    typer.complete_symbol(constructor).unwrap();
    let mut methods = Vec::new();
    for &tree in &template.body {
        let TreeKind::DefDef(definition) = &parsed.ast.get(tree).kind else {
            continue;
        };
        let method = index.symbol_at(source, tree).unwrap();
        typer.complete_symbol(method).unwrap();
        let context = typer.expression_context_for(method).unwrap();
        let body = definition.rhs.map(|rhs| {
            typer
                .type_expression_expected(rhs, context, definitions.int)
                .unwrap()
        });
        if let (Some(rhs), Some(body)) = (definition.rhs, body) {
            assert_eq!(typer.source_typed_index().get(source, rhs), Some(body));
        }
        methods.push(MethodPicklePlan {
            symbol: method,
            source_tree: tree,
            parameter_clauses: clauses(definition),
            body,
        });
    }
    let bindings = |source, tree| typer.source_typed_index().get(source, tree);
    let mut plan = PicklePlan {
        store: typer.store(),
        definitions: &definitions,
        typed_ast: typer.typed_ast(),
        source_ast: &parsed.ast,
        source_index: &index,
        typed_bindings: &bindings,
        source: SourceMetadata {
            id: source,
            name: source_name.into(),
        },
        source_root: parsed.root,
        class,
        class_tree: *class_tree,
        constructor: DefaultConstructorPlan {
            symbol: constructor,
            source_tree: template.constructor,
            parameter_clauses: clauses(constructor_definition),
        },
        methods: &methods,
    };
    edit(&mut plan);
    let before = format!(
        "{:?}{:?}{:?}{:?}",
        typer.store(),
        typer.typed_ast(),
        index,
        parsed.ast
    );
    let mark = typer.store().checkpoint();
    let checked = plan.validate();
    assert_eq!(typer.store().checkpoint(), mark);
    assert_eq!(
        format!(
            "{:?}{:?}{:?}{:?}",
            typer.store(),
            typer.typed_ast(),
            index,
            parsed.ast
        ),
        before
    );
    let checked = checked?;
    assert!(std::ptr::eq(checked.plan().store, typer.store()));
    assert!(std::ptr::eq(checked.plan().typed_ast, typer.typed_ast()));
    assert_eq!(
        checked.package(),
        index.symbol_at(source, parsed.root).unwrap()
    );
    assert_eq!(checked.plan().constructor.symbol, constructor);
    assert_eq!(
        checked.plan().methods[0].symbol,
        index.symbol_at(source, template.body[0]).unwrap()
    );
    for method in checked.plan().methods {
        let body = method.body.expect("concrete source method body");
        assert!(matches!(
            typer.typed_ast().get(body).kind,
            TreeKind::Literal(Literal {
                value: Constant::Int(_)
            }) | TreeKind::Ident(_)
                | TreeKind::Select(_)
                | TreeKind::Apply(_)
        ));
    }
    let body = checked.plan().methods[0].body.unwrap();
    let expected_literal = match &typer.typed_ast().get(body).kind {
        TreeKind::Literal(Literal {
            value: Constant::Int(value),
        }) => Some(*value),
        TreeKind::Ident(_) | TreeKind::Select(_) | TreeKind::Apply(_) => None,
        other => panic!("unexpected typed method body: {other:?}"),
    };
    let output = checked.pickle()?;
    for method in &methods {
        assert_eq!(output.body(method.symbol), method.body);
    }
    assert_eq!(output.source(), &plan.source);
    let complete =
        dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let addresses = complete.ast_address_index().unwrap();
    let wire_method = addresses
        .get(output.declarations()[2].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    match (expected_literal, wire_method.rhs) {
        (Some(value), Some(dotty_tasty::tasty::RawTree::Leaf(rhs))) => {
            assert_eq!(rhs.tag, dotty_tasty::tasty::INTCONST_TAG);
            assert_eq!(rhs.value, dotty_tasty::tasty::TermValue::Int(value));
        }
        (None, Some(dotty_tasty::tasty::RawTree::Leaf(rhs))) => {
            assert_eq!(rhs.tag, dotty_tasty::tasty::TERMREFDIRECT_TAG);
            assert!(matches!(
                rhs.value,
                dotty_tasty::tasty::TermValue::AstRef(_)
            ));
        }
        (None, Some(dotty_tasty::tasty::RawTree::NatAst { tag, .. })) => {
            assert_eq!(tag, dotty_tasty::tasty::SELECT_TAG);
        }
        (None, Some(dotty_tasty::tasty::RawTree::LengthNode(node))) => {
            assert_eq!(node.tag, dotty_tasty::tasty::APPLY_TAG);
        }
        (expected, actual) => panic!("body shape changed: {expected:?} / {actual:?}"),
    }
    assert_eq!(complete.encode_validated().unwrap(), output.bytes());
    let skeleton = checked.declaration_skeleton()?;
    let file =
        dotty_tasty::tasty::TastyFile::parse_and_validate_scala_3_9(skeleton.provisional_bytes())
            .unwrap();
    assert_eq!(file.source_file().unwrap(), Some(source_name));
    assert_eq!(
        skeleton.omitted_bodies(),
        methods
            .iter()
            .map(|method| OmittedMethodBody {
                method: method.symbol,
                typed_root: method.body.unwrap(),
            })
            .collect::<Vec<_>>()
    );
    assert_eq!(typer.store().checkpoint(), mark);
    assert_eq!(
        format!(
            "{:?}{:?}{:?}{:?}",
            typer.store(),
            typer.typed_ast(),
            index,
            parsed.ast
        ),
        before
    );
    Ok((skeleton, output))
}

fn source_plan(
    text: &str,
    edit: impl FnOnce(&mut PicklePlan<'_>),
) -> Result<DeclarationSkeleton, PickleError> {
    source_outputs(text, edit).map(|(skeleton, _)| skeleton)
}

#[test]
fn actual_answer_source_hands_off_exact_canonical_symbols_and_typed_rhs() {
    let skeleton = source_plan(ANSWER, |_| {}).unwrap();
    assert_eq!(skeleton.intended_tasty_path(), "example/Answer.tasty");
}

#[test]
fn same_class_forward_method_reference_is_a_named_select_and_deterministic() {
    use dotty_tasty::tasty::*;

    let (_, first) = source_outputs_named(REFERENCES, "References.scala", |_| {}).unwrap();
    let (_, second) = source_outputs_named(REFERENCES, "References.scala", |_| {}).unwrap();
    assert_eq!(first.bytes(), second.bytes());
    let class = first.declarations()[0].symbol;
    let first_method = first.declarations()[2];
    let second_method = first.declarations()[3];
    assert_eq!(first_method.owner, class);
    assert_eq!(second_method.owner, class);

    let file = TastyFile::parse_and_validate_scala_3_9(first.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    let first_body = addresses
        .get(first_method.address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let select = first_body.rhs.unwrap().decode_select().unwrap();
    assert_eq!(select.tag, SELECT_TAG);
    let receiver = select.qualifier.decode_qual_this().unwrap();
    let class_syntax = receiver.child.decode_ident().unwrap();
    let class_ref = class_syntax.type_tree.decode_reference().unwrap();
    assert_eq!(class_ref.tag, TYPEREFSYMBOL_TAG);
    assert_eq!(class_ref.reference, first.declarations()[0].address);

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1242/generated/example/References.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, first.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1242/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 -from-tasty gate");
        assert!(
            result.success(),
            "requested References Scala interoperability gate failed"
        );
    }
}

#[test]
fn explicit_this_method_reference_preserves_the_verified_this_wire_shape() {
    use dotty_tasty::tasty::*;

    let (_, first) =
        source_outputs_named(EXPLICIT_THIS_REFERENCES, "References.scala", |_| {}).unwrap();
    let (_, second) =
        source_outputs_named(EXPLICIT_THIS_REFERENCES, "References.scala", |_| {}).unwrap();
    assert_eq!(first.bytes(), second.bytes());
    let file = TastyFile::parse_and_validate_scala_3_9(first.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    let first_body = addresses
        .get(first.declarations()[2].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let select = first_body.rhs.unwrap().decode_select().unwrap();
    let receiver = select.qualifier.decode_this().unwrap();
    let class_ref = receiver.child.decode_reference().unwrap();
    assert_eq!(class_ref.tag, TYPEREFSYMBOL_TAG);
    assert_eq!(class_ref.reference, first.declarations()[0].address);

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate =
            root.join("target/issue-1242/generated-explicit-this/example/References.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, first.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1242/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 -from-tasty gate");
        assert!(
            result.success(),
            "explicit-this Scala interoperability gate failed"
        );
    }
}

#[test]
fn same_class_int_applications_preserve_selected_target_and_argument_order() {
    use dotty_tasty::tasty::*;

    let (_, first) = source_outputs_named(OPERATIONS, "Operations.scala", |_| {}).unwrap();
    let (_, second) = source_outputs_named(OPERATIONS, "Operations.scala", |_| {}).unwrap();
    assert_eq!(first.bytes(), second.bytes());
    let file = TastyFile::parse_and_validate_scala_3_9(first.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    let use_body = addresses
        .get(first.declarations()[2].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let RawTree::LengthNode(call_node) = use_body.rhs.unwrap() else {
        panic!("expected APPLY")
    };
    let call = call_node.decode_apply().unwrap();
    let RawTree::LengthNode(select_node) = call.function else {
        panic!("expected SELECTin")
    };
    let select = select_node.decode_select_in().unwrap();
    assert_eq!(select.tag, SELECTIN_TAG);
    assert!(matches!(
        file.render_signed_name(select.name).unwrap(),
        Some(RenderedSignedName::Signed { original, signature })
            if original == "identity"
                && signature.result == "scala.Int"
                && signature.parameters == [RenderedParamSig::TermParameter("scala.Int".into())]
    ));
    assert_eq!(call.arguments.len(), 1);
    assert!(matches!(
        call.arguments[0],
        RawTree::Leaf(SimpleTerm {
            tag: INTCONST_TAG,
            value: TermValue::Int(42),
            ..
        })
    ));

    let propagate_body = addresses
        .get(first.declarations()[4].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let RawTree::LengthNode(propagate_node) = propagate_body.rhs.unwrap() else {
        panic!("expected APPLY")
    };
    let propagate_call = propagate_node.decode_apply().unwrap();
    let RawTree::LengthNode(propagate_select_node) = propagate_call.function else {
        panic!("expected SELECTin")
    };
    let propagate_select = propagate_select_node.decode_select_in().unwrap();
    assert!(
        matches!(file.render_signed_name(propagate_select.name).unwrap(), Some(RenderedSignedName::Signed { original, .. }) if original == "identity")
    );
    assert_eq!(propagate_call.arguments.len(), 1);
    assert!(
        matches!(propagate_call.arguments[0], RawTree::Leaf(ref leaf) if leaf.tag == TERMREFDIRECT_TAG)
    );
    let explicit_body = addresses
        .get(first.declarations()[5].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let RawTree::LengthNode(explicit_call_node) = explicit_body.rhs.unwrap() else {
        panic!("expected APPLY")
    };
    let explicit_call = explicit_call_node.decode_apply().unwrap();
    let RawTree::LengthNode(explicit_select_node) = explicit_call.function else {
        panic!("expected SELECTin")
    };
    let explicit_select = explicit_select_node.decode_select_in().unwrap();
    assert!(matches!(
        explicit_select.qualifier,
        RawTree::Ast { tag: THIS_TAG, .. }
    ));

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1243/generated/example/Operations.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, first.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1243/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 -from-tasty gate");
        assert!(
            result.success(),
            "Operations Scala interoperability gate failed"
        );
    }
}

#[test]
fn multiple_literal_methods_and_empty_clause_match_the_scala_390_oracle() {
    use dotty_tasty::tasty::*;

    let (skeleton, first) = source_outputs_named(MULTI, "Multi.scala", |_| {}).unwrap();
    let (_, second) = source_outputs_named(MULTI, "Multi.scala", |_| {}).unwrap();
    assert_eq!(first.bytes(), second.bytes());
    assert_eq!(first.intended_tasty_path(), "example/Multi.tasty");
    let class = first.declarations()[0].symbol;
    let declaration_ids = first
        .declarations()
        .iter()
        .map(|declaration| declaration.symbol)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(declaration_ids.len(), 5);
    assert_eq!(
        first.declarations()[1..]
            .iter()
            .map(|declaration| declaration.owner)
            .collect::<Vec<_>>(),
        [class; 4]
    );
    assert_eq!(first.declarations().len(), 5);
    assert_eq!(skeleton.declarations().len(), 5);
    assert_eq!(skeleton.omitted_bodies().len(), 3);
    let body_roots = first.declarations()[2..]
        .iter()
        .map(|declaration| first.body(declaration.symbol).unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(body_roots.len(), 3);
    assert!(
        first.declarations()[2..]
            .windows(2)
            .all(|pair| pair[0].address < pair[1].address)
    );

    let file = TastyFile::parse_and_validate_scala_3_9(first.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    let observed = first.declarations()[2..]
        .iter()
        .map(|declaration| {
            let method = addresses
                .get(declaration.address)
                .unwrap()
                .decode_defdef_body()
                .unwrap();
            let name = file.render_name(method.name).unwrap();
            let rhs = match method.rhs.unwrap() {
                RawTree::Leaf(leaf) if leaf.tag == INTCONST_TAG => match leaf.value {
                    TermValue::Int(value) => value,
                    other => panic!("unexpected Int constant payload: {other:?}"),
                },
                other => panic!("unexpected literal body: {other:?}"),
            };
            (name.to_owned(), method.clauses, rhs)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        [
            ("one".to_owned(), vec![], 1),
            ("two".to_owned(), vec![], 2),
            ("zero".to_owned(), vec![EMPTYCLAUSE_TAG], 0),
        ]
    );
    assert_eq!(
        first.declarations()[2..]
            .iter()
            .map(|declaration| first.body(declaration.symbol).unwrap())
            .collect::<Vec<_>>(),
        skeleton
            .omitted_bodies()
            .iter()
            .map(|body| body.typed_root)
            .collect::<Vec<_>>()
    );

    if std::env::var_os("DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE").is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1239/generated/example/Multi.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, first.bytes()).unwrap();
        let temporary = root.join("target/issue-1239/tmp");
        std::fs::create_dir_all(&temporary).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .env("TMPDIR", temporary)
            .status()
            .expect("start explicitly requested Scala 3.9 -from-tasty gate");
        assert!(
            result.success(),
            "requested Multi Scala interoperability gate failed"
        );
    }
}

#[test]
fn source_int_parameters_emit_ordered_canonical_param_declarations() {
    use dotty_tasty::tasty::*;

    let (skeleton, first) =
        source_outputs_named(PARAMETER_DECL, "ParameterDecl.scala", |_| {}).unwrap();
    let (_, second) = source_outputs_named(PARAMETER_DECL, "ParameterDecl.scala", |_| {}).unwrap();
    assert_eq!(first.bytes(), second.bytes());
    assert_eq!(first.intended_tasty_path(), "example/ParameterDecl.tasty");
    assert_eq!(first.declarations().len(), 5);
    assert_eq!(skeleton.declarations().len(), 5);

    let file = TastyFile::parse_and_validate_scala_3_9(first.bytes()).unwrap();
    let address_index = file.ast_address_index().unwrap();
    let method_declaration = &first.declarations()[2];
    let method = address_index
        .get(method_declaration.address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    assert!(method.clauses.is_empty());
    assert_eq!(
        method
            .parameters
            .iter()
            .map(|parameter| file.render_name(parameter.name()).unwrap())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert!(matches!(
        method.rhs,
        Some(RawTree::Leaf(SimpleTerm {
            tag: INTCONST_TAG,
            value: TermValue::Int(42),
            ..
        }))
    ));

    let parameter_declarations = &first.declarations()[3..];
    assert_ne!(
        parameter_declarations[0].symbol,
        parameter_declarations[1].symbol
    );
    assert!(
        parameter_declarations
            .iter()
            .all(|parameter| parameter.owner == method_declaration.symbol)
    );
    for (declaration, expected_name) in parameter_declarations.iter().zip(["first", "second"]) {
        let raw = address_index.get_node(declaration.address).unwrap();
        assert_eq!(raw.tag, PARAM_TAG);
        let parameter = address_index
            .get(declaration.address)
            .unwrap()
            .decode_parameter()
            .unwrap();
        assert_eq!(file.render_name(parameter.name()).unwrap(), expected_name);
        assert!(matches!(
            parameter.decode_body().unwrap().type_tree,
            RawTree::NatAst {
                tag: IDENTTPT_TAG,
                ..
            }
        ));
    }

    if std::env::var_os("DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE").is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1240/generated/example/ParameterDecl.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, first.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .env("TMPDIR", root.join("target/issue-1240/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 -from-tasty gate");
        assert!(
            result.success(),
            "requested ParameterDecl Scala interoperability gate failed"
        );
    }
}

#[test]
fn source_parameter_ident_emits_a_direct_reference_to_its_exact_param_declaration() {
    use dotty_tasty::tasty::*;

    let (_, first) = source_outputs_named(PARAMETERS, "Parameters.scala", |_| {}).unwrap();
    let (_, second) = source_outputs_named(PARAMETERS, "Parameters.scala", |_| {}).unwrap();
    assert_eq!(first.bytes(), second.bytes());
    assert_eq!(first.intended_tasty_path(), "example/Parameters.tasty");

    let file = TastyFile::parse_and_validate_scala_3_9(first.bytes()).unwrap();
    let address_index = file.ast_address_index().unwrap();
    let pick = first
        .declarations()
        .iter()
        .find(|declaration| {
            address_index
                .get(declaration.address)
                .and_then(|node| node.decode_defdef_body().ok())
                .is_some_and(|body| file.render_name(body.name).ok().as_deref() == Some("pick"))
        })
        .unwrap();
    let pick_body = address_index
        .get(pick.address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let parameters = first
        .declarations()
        .iter()
        .filter(|declaration| declaration.owner == pick.symbol)
        .collect::<Vec<_>>();
    assert_eq!(parameters.len(), 2);
    assert!(parameters[0].address < parameters[1].address);
    let names = parameters
        .iter()
        .map(|declaration| {
            let parameter = address_index
                .get(declaration.address)
                .unwrap()
                .decode_parameter()
                .unwrap();
            file.render_name(parameter.name()).unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(names, ["first", "second"]);
    let Some(RawTree::Leaf(reference)) = pick_body.rhs else {
        panic!("formal parameter RHS must be one direct reference leaf")
    };
    assert_eq!(reference.tag, TERMREFDIRECT_TAG);
    assert_eq!(
        reference.ast_ref().map(|reference| reference.address),
        Some(parameters[1].address),
        "the body must reference the second canonical PARAM declaration"
    );

    if std::env::var_os("DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE").is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1241/generated/example/Parameters.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, first.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1241/tmp"))
            .status()
            .expect("start explicitly requested Scala 3.9 -from-tasty gate");
        assert!(
            result.success(),
            "requested Parameters Scala interoperability gate failed"
        );
    }
}

#[test]
fn backquoted_class_and_package_components_use_encoded_artifact_paths_only() {
    use dotty_tasty::tasty::*;

    for (package, class, expected) in [
        ("example", "A-B", "example/A$minusB.tasty"),
        ("p-q", "Answer", "p$minusq/Answer.tasty"),
        ("p+q", "A B", "p$plusq/A$u0020B.tasty"),
        ("example", "😀", "example/$uD83D$uDE00.tasty"),
    ] {
        let text = format!("package `{package}`; class `{class}` {{ def value: Int = 42 }}");
        let skeleton = source_plan(&text, |_| {}).unwrap();
        assert_eq!(skeleton.intended_tasty_path(), expected);
        let file = TastyFile::parse_and_validate_scala_3_9(skeleton.provisional_bytes()).unwrap();
        let index = file.ast_address_index().unwrap();
        let definition = index
            .get(skeleton.declarations()[0].address)
            .unwrap()
            .decode_definition()
            .unwrap();
        assert_eq!(file.render_name(definition.name()).unwrap(), class);
        let roots = file.asts().unwrap();
        let wire_package = roots.get(0).unwrap().decode_package().unwrap();
        let RawTree::Leaf(path) = wire_package.path else {
            panic!()
        };
        assert_eq!(file.render_name(path.name_ref().unwrap()).unwrap(), package);
    }
}

#[test]
fn a_second_independent_source_session_validates_without_cross_session_identity_assumptions() {
    source_plan(ANSWER, |_| {}).unwrap();
    source_plan(ANSWER, |_| {}).unwrap();
}

#[test]
fn real_source_default_constructor_clauses_normalize_without_fabricating_method_parens() {
    let implicit = source_plan(ANSWER, |_| {}).unwrap();
    let explicit =
        source_plan(&ANSWER.replace("class Answer:", "class Answer():"), |_| {}).unwrap();
    assert_eq!(implicit.provisional_bytes(), explicit.provisional_bytes());
}

#[test]
fn contextual_and_type_parameter_methods_remain_explicitly_unsupported() {
    for (text, feature) in [
        (
            "package example; class Answer { def value(using x: Int): Int = x }",
            UnsupportedFeature::SourceMetadata,
        ),
        (
            "package example; class Answer { def value[A]: Int = 42 }",
            UnsupportedFeature::TypeParameters,
        ),
    ] {
        let result = source_plan(text, |_| {});
        assert!(
            matches!(
                &result,
                Err(PickleError::Unsupported { feature: actual, .. }) if *actual == feature
            ),
            "{text}: {result:?}"
        );
    }
}

#[test]
fn unsupported_parameter_kinds_and_defaults_fail_before_emission() {
    for (text, feature) in [
        (
            "package example; class Answer { def value(x: => Int): Int = 42 }",
            UnsupportedFeature::ParameterClauses,
        ),
        (
            "package example; class Answer { def value(x: Int = 1): Int = 42 }",
            UnsupportedFeature::ParameterClauses,
        ),
        (
            "package example; class Answer { def value(x: Int)(y: Int): Int = 42 }",
            UnsupportedFeature::ParameterClauses,
        ),
    ] {
        let result = source_plan(text, |_| {});
        assert!(
            matches!(
                &result,
                Err(PickleError::Unsupported { feature: actual, .. }) if *actual == feature
            ),
            "{text}: {result:?}"
        );
    }
}

#[test]
fn malformed_clause_shapes_and_empty_class_fail_explicitly() {
    for text in [
        "package example; class Answer { def value()(): Int = 42 }",
        "package example; class Answer { def value()(x: Int): Int = x }",
    ] {
        assert!(matches!(
            source_plan(text, |_| {}),
            Err(PickleError::Unsupported {
                feature: UnsupportedFeature::ParameterClauses,
                ..
            })
        ));
    }
    assert!(matches!(
        source_plan("package example; class Answer {}", |_| {}),
        Err(PickleError::Unsupported {
            feature: UnsupportedFeature::MethodCount,
            ..
        })
    ));
}

#[test]
fn missing_real_frontend_binding_is_an_error_without_mutating_the_pipeline() {
    fn missing(_: SourceId, _: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        None
    }
    assert!(matches!(
        source_plan(ANSWER, |plan| plan.typed_bindings = &missing),
        Err(PickleError::BodyBinding { actual: None, .. })
    ));
}

#[test]
fn valid_source_fields_are_unsupported_rather_than_malformed_method_order() {
    assert!(matches!(
        source_plan(
            "package example; class Answer { val field: Int = 0; def value: Int = 42 }",
            |_| {}
        ),
        Err(PickleError::Unsupported {
            feature: UnsupportedFeature::ClassMembers,
            ..
        })
    ));
}

#[test]
fn valid_source_traits_are_unsupported_rather_than_invalid_class_identities() {
    assert!(matches!(
        source_plan(
            "package example; trait Answer { def value: Int = 42 }",
            |_| {}
        ),
        Err(PickleError::Unsupported {
            feature: UnsupportedFeature::SourceMetadata,
            ..
        })
    ));
}

#[test]
fn real_typed_rhs_values_are_serialized_without_a_synthetic_method_tree() {
    for spelling in [
        "0",
        "7",
        "2147483647",
        "0x80000000",
        "0xffffffff",
        "0b101010",
    ] {
        source_outputs(&ANSWER.replace("42", spelling), |_| {}).unwrap();
    }
}

#[test]
fn complete_source_output_is_deterministic_across_actual_frontend_sessions() {
    let (_, first) = source_outputs(ANSWER, |_| {}).unwrap();
    let (_, second) = source_outputs(ANSWER, |_| {}).unwrap();
    assert_eq!(first.bytes(), second.bytes());
}
