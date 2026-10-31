use dotty_core::ast::{AstArena, TreeKind, Typed, Untyped};
use dotty_core::types::{ClassInfo, TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, Name, Namespace, Packages, SemanticStore, SourceId, SourceSemanticIndex,
    SourceText, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, TreeId,
    Visibility,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_typer::{SourceTyper, TyperError};

const SOURCE: &str = include_str!("fixtures/inherited-members-3.9.0/InheritedMembers.scala");
const EXPLICIT_SELECTION: &str =
    include_str!("fixtures/inherited-members-3.9.0/ExplicitSelection.scala");
const ROOTS: [&str; 4] = ["direct", "withBias", "lifted", "mapped"];

#[test]
fn inherited_member_baseline_types_the_four_real_child_method_roots() {
    let first = analyze_candidate_session();
    let second = analyze_candidate_session();
    assert_eq!(first, second, "fresh sessions normalize identically");
    let actual = format!("{}\n", first.join("\n"));
    assert_eq!(
        actual,
        include_str!("fixtures/inherited-members-3.9.0/four-root-baseline.tsv")
            .split_once('\n')
            .expect("baseline header")
            .1
    );
    for row in first {
        eprintln!("{row}");
    }
}

#[test]
fn explicit_member_selection_controls_are_separate_from_the_four_root_denominator() {
    let source = SourceId::from_index(1904);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(EXPLICIT_SELECTION).unwrap();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(EXPLICIT_SELECTION).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "ExplicitSelection.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let child = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::TypeDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "ExplicitChild")
                .then(|| index.symbol_at(source, tree).unwrap())
        })
        .unwrap();
    let parent = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::TypeDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "ExplicitParent")
                .then(|| index.symbol_at(source, tree).unwrap())
        })
        .unwrap();
    let inherited_method = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "add")
                .then(|| index.symbol_at(source, tree).unwrap())
        })
        .unwrap();
    let inherited_field = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "bias")
                .then(|| index.symbol_at(source, tree).unwrap())
        })
        .unwrap();
    assert_eq!(store.symbols.get(inherited_method).owner, Some(parent));
    assert_eq!(store.symbols.get(inherited_field).owner, Some(parent));
    let controls = [
        ("direct", None),
        ("explicitMethod", Some(inherited_method)),
        ("withBias", Some(inherited_field)),
        ("explicitField", Some(inherited_field)),
    ]
    .map(|(name, expected_target)| {
        let (tree, symbol) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == name)
                    .then(|| (tree, index.symbol_at(source, tree).unwrap()))
            })
            .unwrap();
        (name, tree, symbol, expected_target)
    });
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.complete_symbol(child).unwrap();
    let mut outcomes = Vec::new();
    for (name, tree, symbol, expected_target) in controls {
        assert_eq!(typer.store().symbols.get(symbol).owner, Some(child));
        let outcome = typer.type_method_definition(tree);
        let outcome = match outcome {
            Ok(typed) => {
                assert_eq!(typer.source_typed_index().get(source, tree), Some(typed));
                if let Some(expected_target) = expected_target {
                    assert!(
                        typer.typed_ast().iter().any(|(_, node)| matches!(
                            typer.store().types.get(node.ty),
                            Type::TermRef {
                                target: TermRefTarget::Symbol(target),
                                ..
                            } if *target == expected_target
                        )),
                        "{name} must select its canonical Parent member"
                    );
                }
                Ok(typed)
            }
            Err(error) => Err(error),
        };
        outcomes.push((name, tree, outcome));
    }
    assert!(outcomes[0].2.is_ok(), "unqualified add is the target root");
    assert!(
        typer.typed_ast().iter().any(|(_, node)| matches!(
            typer.store().types.get(node.ty),
            Type::TermRef {
                target: TermRefTarget::Symbol(target),
                ..
            } if *target == inherited_method
        )),
        "unqualified add must select the original Parent.add symbol"
    );
    assert!(
        outcomes[1].2.is_ok(),
        "this.add is the diagnostic method control"
    );
    assert!(outcomes[2].2.is_ok(), "implicit inherited field control");
    assert!(
        outcomes[3].2.is_ok(),
        "this.bias is the diagnostic field control"
    );
}

#[test]
fn inherited_method_rejections_do_not_create_typed_roots() {
    let cases = [
        (
            "package negative\nclass Parent\nclass Child extends Parent:\n  def run(x: Int): Int = absent(x)\n",
            "run",
        ),
        (
            "package negative\nclass Parent:\n  def add(x: Int): Int = x\nclass Child extends Parent:\n  def run: Int = add(true)\n",
            "run",
        ),
        (
            "package negative\nclass Parent:\n  def add(x: Int, y: Int): Int = x + y\nclass Child extends Parent:\n  def run(x: Int): Int = add(x)\n",
            "run",
        ),
        (
            "package negative\nclass Parent:\n  private def add(x: Int): Int = x\nclass Child extends Parent:\n  def run(x: Int): Int = add(x)\n",
            "run",
        ),
        (
            "package negative\nclass Parent:\n  def add(x: Int): Int = x\n  def add(x: String): String = x\nclass Child extends Parent:\n  def run(x: Int): Int = add(x)\n",
            "run",
        ),
        (
            "package negative\nclass Outer:\n  def add(x: Int): Int = x\n  class Parent:\n    def add(x: Int): Int = x\n    def add(x: Boolean): Int = 0\n  class Child extends Parent:\n    def run(x: Int): Int = add(x)\n",
            "run",
        ),
        (
            "package negative\nclass Outer:\n  def add(x: Int): Int = x\n  class Parent:\n    def add(x: Int): Int = x\n    def add[A](x: A): Int = 0\n  class Child extends Parent:\n    def run(x: Int): Int = add(x)\n",
            "run",
        ),
        (
            "package negative\nclass Parent:\n  def add(x: Int)(y: Int): Int = x + y\nclass Child extends Parent:\n  def run(x: Int): Int = add(x)(1)\n",
            "run",
        ),
    ];
    for (index, (source, method)) in cases.into_iter().enumerate() {
        let error = rejected_method_root(source, method, index as u32 + 1910);
        assert!(
            error.is_some(),
            "invalid or ambiguous case unexpectedly typed"
        );
        if index >= 4 {
            assert!(
                matches!(error, Some(TyperError::OverloadedSelectionDeferred { .. })),
                "unsupported inherited buckets must stop lexical fallback: {error:?}"
            );
        }
    }
}

#[test]
fn inherited_single_clause_accepts_multiple_ordinary_parameters() {
    let source = "package multiarg\nclass Parent:\n  def add(x: Int, y: Int): Int = x + y\nclass Child extends Parent:\n  def run(x: Int): Int = add(x, 1)\n";
    assert!(rejected_method_root(source, "run", 1930).is_none());
}

#[test]
fn renamed_reordered_parent_and_unrelated_owner_keep_canonical_method_identity() {
    let source_text = "package reordered\nclass RenamedChild extends RenamedParent:\n  def run(value: Int): Int = combine(value)\nclass OverrideChild extends RenamedParent:\n  override def combine(value: Int): Int = value + 10\n  def ownRun(value: Int): Int = combine(value)\nclass Unrelated:\n  def combine(value: Int): Int = value + 100\nclass RenamedParent:\n  def combine(value: Int): Int = value + 1\n";
    let source = SourceId::from_index(1931);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(source_text).unwrap();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "ReorderedInheritedMethod.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let class = |wanted: &str| {
        parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::TypeDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == wanted)
                    .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap()
    };
    let child = class("RenamedChild");
    let override_child = class("OverrideChild");
    let parent = class("RenamedParent");
    let unrelated = class("Unrelated");
    let method = |owner| {
        parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "combine"
                    && store
                        .symbols
                        .get(index.symbol_at(source, tree).unwrap())
                        .owner
                        == Some(owner))
                .then(|| index.symbol_at(source, tree).unwrap())
            })
            .unwrap()
    };
    let parent_method = method(parent);
    let unrelated_method = method(unrelated);
    let override_method = method(override_child);
    assert_ne!(parent_method, unrelated_method);
    let root = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::DefDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "run" =>
            {
                Some(tree)
            }
            _ => None,
        })
        .unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.complete_symbol(child).unwrap();
    let root_typed = typer.type_method_definition(root).unwrap();
    let TreeKind::DefDef(definition) = &typer.typed_ast().get(root_typed).kind else {
        panic!("run must type as a complete DefDef")
    };
    let rhs = definition.rhs.unwrap();
    let TreeKind::Apply(application) = &typer.typed_ast().get(rhs).kind else {
        panic!("run must retain the direct application")
    };
    let callee = typer.typed_ast().get(application.function);
    let Type::TermRef { prefix, target } = typer.store().types.get(callee.ty) else {
        panic!("callee must retain a canonical term reference")
    };
    assert_eq!(*target, TermRefTarget::Symbol(parent_method));
    assert_ne!(*target, TermRefTarget::Symbol(unrelated_method));
    assert!(matches!(
        typer.store().types.get(*prefix),
        Type::ThisType { class } if *class == child
    ));
    assert_eq!(typer.store().symbols.get(parent_method).owner, Some(parent));
    assert!(matches!(
        &callee.kind,
        TreeKind::Ident(ident)
            if store_name_eq(typer.store(), ident.name, "combine")
    ));

    typer.complete_symbol(override_child).unwrap();
    let override_root = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::DefDef(definition)
                if typer
                    .store()
                    .names
                    .resolve(definition.name.as_name().text())
                    == "ownRun" =>
            {
                Some(tree)
            }
            _ => None,
        })
        .unwrap();
    let override_typed = typer.type_method_definition(override_root).unwrap();
    let TreeKind::DefDef(override_definition) = &typer.typed_ast().get(override_typed).kind else {
        panic!("override child method must type")
    };
    let TreeKind::Apply(override_application) =
        &typer.typed_ast().get(override_definition.rhs.unwrap()).kind
    else {
        panic!("override call must remain an application")
    };
    assert!(matches!(
        typer
            .store()
            .types
            .get(typer.typed_ast().get(override_application.function).ty),
        Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(symbol),
        } if *symbol == override_method
            && matches!(typer.store().types.get(*prefix), Type::ThisType { class } if *class == override_child)
    ));
}

#[test]
fn rejected_lookup_rolls_back_before_a_valid_same_session_retry() {
    let source_text = "package retry\nclass Parent:\n  def add(x: Int): Int = x + 1\nclass Child extends Parent:\n  def invalid(x: Int): Int = absent(x)\n  def valid(x: Int): Int = add(x)\n";
    let source = SourceId::from_index(1932);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(source_text).unwrap();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "InheritedMethodRetry.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let child = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "Child" =>
            {
                Some(index.symbol_at(source, tree).unwrap())
            }
            _ => None,
        })
        .unwrap();
    let method = |wanted: &str| {
        parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == wanted =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap()
    };
    let invalid = method("invalid");
    let valid = method("valid");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.complete_symbol(child).unwrap();
    assert!(typer.type_method_definition(invalid).is_err());
    assert!(typer.source_typed_index().get(source, invalid).is_none());
    let valid_typed = typer.type_method_definition(valid).unwrap();
    assert_eq!(
        typer.source_typed_index().get(source, valid),
        Some(valid_typed)
    );
}

#[test]
fn invalid_parent_class_info_does_not_fall_through_or_publish_a_root() {
    for (source_index, invalid_class_info) in [(1933, false), (1934, true)] {
        let source_text = "package invalidparent\nclass Outer:\n  def add(x: Int): Int = x\n  class Parent:\n    def add(x: Int): Int = x\n  class Child extends Parent:\n    def run(x: Int): Int = add(x)\n";
        let source = SourceId::from_index(source_index);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let scanner = ContextualScanner::new(source_text).unwrap();
        let parsed = dotty_parser::parse_compilation_unit(
            SourceText::new(source_text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let mut packages = Packages::new();
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "InvalidParentInfo.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        let class = |wanted: &str| {
            parsed
                .ast
                .iter()
                .find_map(|(tree, node)| {
                    let TreeKind::TypeDef(definition) = &node.kind else {
                        return None;
                    };
                    (store.names.resolve(definition.name.as_name().text()) == wanted)
                        .then(|| index.symbol_at(source, tree).unwrap())
                })
                .unwrap()
        };
        let parent = class("Parent");
        let child = class("Child");
        let root = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "run" =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap();
        let invalid_info = if invalid_class_info {
            SymbolInfo::Complete(store.types.alloc(Type::NoType))
        } else {
            SymbolInfo::Error
        };
        store.symbols.set_info(parent, invalid_info);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        typer.complete_symbol(child).unwrap();
        assert!(matches!(
            typer.type_method_definition(root),
            Err(TyperError::MemberLookup(_))
        ));
        assert!(typer.source_typed_index().get(source, root).is_none());
    }
}

fn store_name_eq(store: &SemanticStore, name: Name, expected: &str) -> bool {
    store.names.resolve(name.text()) == expected
}

fn rejected_method_root(
    source_text: &str,
    method_name: &str,
    source_index: u32,
) -> Option<TyperError> {
    let source = SourceId::from_index(source_index);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(source_text).unwrap();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "InheritedMethodNegative.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let child = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "Child" =>
            {
                Some(index.symbol_at(source, tree).unwrap())
            }
            _ => None,
        })
        .unwrap();
    let method_tree = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::DefDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == method_name =>
            {
                Some(tree)
            }
            _ => None,
        })
        .unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.complete_symbol(child).unwrap();
    let outcome = typer.type_method_definition(method_tree);
    if outcome.is_err() {
        assert!(
            typer
                .source_typed_index()
                .get(source, method_tree)
                .is_none(),
            "a rejected root must not retain a typed mapping"
        );
    }
    outcome.err()
}

struct DirectRootReceipt<'a> {
    source_ast: &'a AstArena<Untyped>,
    index: &'a SourceSemanticIndex,
    source: SourceId,
    source_root: TreeId<Untyped>,
    typed_root: TreeId<Typed>,
    source_rhs: TreeId<Untyped>,
    typed_rhs: TreeId<Typed>,
    child: dotty_core::SymbolId,
    parent: dotty_core::SymbolId,
    parent_add: dotty_core::SymbolId,
}

fn assert_direct_root_identity(
    typer: &mut SourceTyper<'_>,
    receipt: DirectRootReceipt<'_>,
) -> String {
    let DirectRootReceipt {
        source_ast,
        index,
        source,
        source_root,
        typed_root,
        source_rhs,
        typed_rhs,
        child,
        parent,
        parent_add,
    } = receipt;
    assert!(index.symbol_at(source, source_root).is_some());
    assert_eq!(
        typer.source_typed_index().get(source, source_root),
        Some(typed_root)
    );
    let TreeKind::Apply(source_apply) = &source_ast.get(source_rhs).kind else {
        panic!("direct source RHS must be an Apply")
    };
    assert_eq!(source_apply.args.len(), 1);
    let TreeKind::Ident(source_callee) = &source_ast.get(source_apply.function).kind else {
        panic!("direct source callee must remain its unqualified Ident")
    };
    let TreeKind::DefDef(typed_definition) = &typer.typed_ast().get(typed_root).kind else {
        panic!("direct root must map to a typed DefDef")
    };
    assert_eq!(typed_definition.rhs, Some(typed_rhs));
    let TreeKind::Apply(typed_apply) = &typer.typed_ast().get(typed_rhs).kind else {
        panic!("direct typed RHS must be an Apply")
    };
    assert_eq!(typed_apply.args.len(), 1);
    assert_eq!(
        typer
            .source_typed_index()
            .get(source, source_apply.function),
        Some(typed_apply.function),
        "callee mapping must be canonical"
    );
    let source_argument = source_apply.args[0];
    let typed_argument = typed_apply.args[0];
    assert_eq!(
        typer.source_typed_index().get(source, source_argument),
        Some(typed_argument),
        "argument mapping must be canonical"
    );
    assert_eq!(
        typer.source_typed_index().get(source, source_rhs),
        Some(typed_rhs)
    );
    let callee = typer.typed_ast().get(typed_apply.function);
    assert!(matches!(callee.kind, TreeKind::Ident(_)));
    let Type::TermRef { prefix, target } = typer.store().types.get(callee.ty) else {
        panic!("inherited callee must retain a symbol term reference")
    };
    assert_eq!(*target, TermRefTarget::Symbol(parent_add));
    assert!(matches!(
        typer.store().types.get(*prefix),
        Type::ThisType { class } if *class == child
    ));
    let SymbolInfo::Complete(method_info) = typer.store().symbols.info(parent_add) else {
        panic!("Parent.add signature must be complete")
    };
    let Type::Method(method) = typer.store().types.get(*method_info) else {
        panic!("Parent.add must retain its ordinary method type")
    };
    assert_eq!(method.params.len(), 1);
    let Type::TermRef {
        target: TermRefTarget::Symbol(argument_symbol),
        ..
    } = typer
        .store()
        .types
        .get(typer.typed_ast().get(typed_argument).ty)
    else {
        panic!("direct argument must refer to its typed parameter")
    };
    let SymbolInfo::Complete(argument_type) = typer.store().symbols.info(*argument_symbol) else {
        panic!("direct argument parameter type must be complete")
    };
    assert_eq!(
        *argument_type, method.params[0].ty,
        "argument parameter type must equal the canonical formal type"
    );
    assert_eq!(
        typer.typed_ast().get(typed_rhs).ty,
        method.result,
        "application result must equal the canonical method result"
    );
    assert!(source_callee.name.is_term());
    assert_eq!(typer.store().symbols.get(parent_add).owner, Some(parent));

    let child_info = match *typer.store().symbols.info(child) {
        SymbolInfo::Complete(info) => info,
        _ => panic!("Child class info must be complete"),
    };
    let Type::ClassInfo(child_info) = typer.store().types.get(child_info) else {
        panic!("Child must have ClassInfo")
    };
    let method_name = typer.store().symbols.get(parent_add).name;
    assert!(
        typer
            .store()
            .scopes
            .get(child_info.declarations)
            .lookup_all(&method_name)
            .is_empty(),
        "no synthetic Child.add declaration may be inserted"
    );
    let owner = typer.store().symbols.get(parent_add).owner.unwrap();
    format!(
        "{}.{}",
        typer
            .store()
            .names
            .resolve(typer.store().symbols.get(owner).name.text()),
        typer
            .store()
            .names
            .resolve(typer.store().symbols.get(parent_add).name.text())
    )
}

fn analyze_candidate_session() -> Vec<String> {
    let source = SourceId::from_index(1903);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(SOURCE).expect("scan the frozen source candidate");
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(SOURCE).expect("valid source text"),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "InheritedMembers.scala",
        &mut store,
        &mut packages,
    )
    .expect("the frozen inheritance source should be named");

    let class = |wanted: &str| {
        parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::TypeDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == wanted)
                    .then(|| (index.symbol_at(source, tree).unwrap(), tree))
            })
            .unwrap_or_else(|| panic!("missing class {wanted}"))
    };
    let (parent, _) = class("Parent");
    let (child, _) = class("Child");
    assert_eq!(
        symbol_path(&store, parent),
        "Package:/Package:inheritancetyper/Class:Parent"
    );
    assert_eq!(
        symbol_path(&store, child),
        "Package:/Package:inheritancetyper/Class:Child"
    );
    let parent_add = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "add")
                .then(|| index.symbol_at(source, tree).unwrap())
        })
        .expect("Parent.add symbol");
    let parent_bias = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "bias")
                .then(|| index.symbol_at(source, tree).unwrap())
        })
        .expect("Parent.bias symbol");
    assert_eq!(store.symbols.get(parent_add).owner, Some(parent));
    assert_eq!(store.symbols.get(parent_bias).owner, Some(parent));
    assert_eq!(store.symbols.get(parent_add).kind, SymbolKind::Method);
    assert_eq!(store.symbols.get(parent_bias).kind, SymbolKind::Field);

    let roots = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            let name = store.names.resolve(definition.name.as_name().text());
            ROOTS.contains(&name).then(|| {
                (
                    name.to_owned(),
                    tree,
                    index.symbol_at(source, tree).unwrap(),
                    node.position.unwrap(),
                )
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        roots.len(),
        ROOTS.len(),
        "only the four Child methods count"
    );
    for (name, _, symbol, _) in &roots {
        assert_eq!(
            store.symbols.get(*symbol).owner,
            Some(child),
            "{name} owner"
        );
    }

    enter_scala_function1(&mut store, &mut packages);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer
        .complete_symbol(child)
        .expect("complete the real source Child hierarchy before typing roots");
    let child_info = match *typer.store().symbols.info(child) {
        SymbolInfo::Complete(info) => info,
        _ => panic!("Child class should be completed"),
    };
    let Type::ClassInfo(ClassInfo { parents, .. }) = typer.store().types.get(child_info) else {
        panic!("Child completion should produce ClassInfo")
    };
    assert_eq!(parents.len(), 1, "the frozen hierarchy has one parent");
    assert!(matches!(
        typer.store().types.get(parents[0]),
        Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == parent
    ));

    let mut receipt = Vec::with_capacity(roots.len());
    for (name, tree, symbol, position) in roots {
        let result = typer.type_method_definition(tree);
        match result {
            Ok(typed) => {
                assert_eq!(typer.source_typed_index().get(source, tree), Some(typed));
                let TreeKind::DefDef(typed_definition) = &typer.typed_ast().get(typed).kind else {
                    panic!("successful root must be a typed DefDef")
                };
                let TreeKind::DefDef(source_definition) = &parsed.ast.get(tree).kind else {
                    unreachable!("root discovery selected a DefDef")
                };
                let source_rhs = source_definition.rhs.expect("frozen method body");
                let typed_rhs = typed_definition.rhs.expect("complete typed method body");
                assert_eq!(
                    typer.source_typed_index().get(source, source_rhs),
                    Some(typed_rhs),
                    "complete root must map its original source body"
                );
                assert!(matches!(
                    typer.store().symbols.info(symbol),
                    SymbolInfo::Complete(_)
                ));
                let member_target = if name == "direct" {
                    assert_eq!(typer.store().symbols.get(symbol).owner, Some(child));
                    assert_direct_root_identity(
                        &mut typer,
                        DirectRootReceipt {
                            source_ast: &parsed.ast,
                            index: &index,
                            source,
                            source_root: tree,
                            typed_root: typed,
                            source_rhs,
                            typed_rhs,
                            child,
                            parent,
                            parent_add,
                        },
                    )
                } else if name == "withBias" {
                    "Parent.bias".to_owned()
                } else {
                    "none".to_owned()
                };
                let result_shape = match typer.store().symbols.info(symbol) {
                    SymbolInfo::Complete(info) => match typer.store().types.get(*info) {
                        Type::Method(method) => type_shape(typer.store(), method.result),
                        other => format!("non-method:{other:?}"),
                    },
                    _ => "signature-unavailable".to_owned(),
                };
                receipt.push(format!(
                    "inheritancetyper.Child\t{name}\tInheritedMembers.scala#{}\t{}..{}\tinheritancetyper.Parent\tinheritancetyper.Parent.add\tinheritancetyper.Parent.bias\t{member_target}\t{}\tfully_typed\t{result_shape}\t-",
                    tree.index(),
                    position.span().range().start(),
                    position.span().range().end(),
                    source_signature(&name),
                ));
            }
            Err(error) => {
                assert!(typer.source_typed_index().get(source, tree).is_none());
                let (first, chain) = nested_error_chain(&error, typer.store());
                receipt.push(format!(
                    "inheritancetyper.Child\t{name}\tInheritedMembers.scala#{}\t{}..{}\tinheritancetyper.Parent\tinheritancetyper.Parent.add\tinheritancetyper.Parent.bias\tunresolved\t{}\tblocked\t-\t{first}:{chain}",
                    tree.index(),
                    position.span().range().start(),
                    position.span().range().end(),
                    source_signature(&name),
                ));
            }
        }
    }
    receipt
}

fn source_signature(name: &str) -> &'static str {
    match name {
        "direct" | "withBias" => "(Int):Int",
        "lifted" | "mapped" => "():Function1[Int,Int]",
        _ => panic!("unexpected frozen Child method {name}"),
    }
}

fn enter_scala_function1(store: &mut SemanticStore, packages: &mut Packages) {
    let scala = packages
        .enter(store, SymbolOrigin::Synthetic, &["scala"])
        .last()
        .expect("canonical scala package")
        .symbol;
    let scope = packages.scope_of(scala).expect("scala package scope");
    let name = Name::new(store.names.intern("Function1"), Namespace::Type);
    let class = store.symbols.alloc(Symbol {
        name,
        owner: Some(scala),
        kind: SymbolKind::Trait,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Missing,
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    store.scopes.get_mut(scope).enter(name, class);
}

fn nested_error_chain(error: &TyperError, store: &SemanticStore) -> (String, String) {
    match error {
        TyperError::TermNameNotFound {
            tree_index,
            name,
            position,
            ..
        } => {
            let name = store.names.resolve(name.text());
            let location = position
                .map(|position| {
                    let range = position.span().range();
                    format!("{}..{}", range.start(), range.end())
                })
                .unwrap_or_else(|| "unknown-span".to_owned());
            (
                "TermNameNotFound".to_owned(),
                format!("TermNameNotFound(name={name},tree={tree_index},span={location})"),
            )
        }
        other => {
            let debug = format!("{other:?}");
            let first = debug.split(" { ").next().unwrap_or(&debug).to_owned();
            (first.clone(), first)
        }
    }
}

fn type_shape(store: &SemanticStore, ty: dotty_core::TypeId) -> String {
    match store.types.get(ty) {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => store
            .names
            .resolve(store.symbols.get(*symbol).name.text())
            .to_owned(),
        Type::Applied { tycon, args } => format!(
            "{}[{}]",
            type_shape(store, *tycon),
            args.iter()
                .map(|arg| type_shape(store, *arg))
                .collect::<Vec<_>>()
                .join(",")
        ),
        other => format!("{other:?}"),
    }
}

fn symbol_path(store: &SemanticStore, symbol: dotty_core::SymbolId) -> String {
    let info = store.symbols.get(symbol);
    let name = store.names.resolve(info.name.text());
    let kind = match info.kind {
        SymbolKind::Package => "Package",
        SymbolKind::Class => "Class",
        SymbolKind::Trait => "Trait",
        other => return format!("{other:?}:{name}"),
    };
    match info.owner {
        Some(owner) => format!("{}/{kind}:{name}", symbol_path(store, owner)),
        None => format!("{kind}:{name}"),
    }
}
