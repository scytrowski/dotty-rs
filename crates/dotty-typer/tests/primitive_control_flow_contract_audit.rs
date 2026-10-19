//! SourceTyper evidence for the pinned Scala 3.9 primitive/control-flow fixtures.

use dotty_core::ast::{AstArena, TreeKind, Typed, Untyped};
use dotty_core::types::{MethodKind, TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceSemanticIndex, SourceText, SymbolFlags,
    SymbolId, SymbolInfo, SymbolKind, TreeId,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_typer::{SourceTyper, TyperError};

const ALGORITHMS: &str = include_str!("../../../tests/fixtures/tasty_pickler/Algorithms.scala");
const CUSTOM_OPERATORS: &str =
    include_str!("../../../tests/fixtures/tasty_pickler/CustomOperators.scala");
const ALGORITHMS_SOURCE: SourceId = SourceId::from_index(40);
const CUSTOM_SOURCE: SourceId = SourceId::from_index(41);

struct Fixture {
    source: SourceId,
    ast: AstArena<Untyped>,
    index: SourceSemanticIndex,
    store: SemanticStore,
    definitions: Definitions,
    packages: Packages,
}

impl Fixture {
    fn new(source: SourceId, text: &str, name: &str) -> Self {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let scanner = ContextualScanner::new(text).expect("pinned fixture lexes");
        let parsed = dotty_parser::parse_compilation_unit(
            SourceText::new(text).unwrap(),
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
            name,
            &mut store,
            &mut packages,
        )
        .expect("pinned fixture names");
        Self {
            source,
            ast: parsed.ast,
            index,
            store,
            definitions,
            packages,
        }
    }

    fn class(&self, name: &str) -> SymbolId {
        self.ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::TypeDef(definition) = &node.kind else {
                    return None;
                };
                (self.store.names.resolve(definition.name.as_name().text()) == name)
                    .then(|| self.index.symbol_at(self.source, tree).unwrap())
            })
            .unwrap_or_else(|| panic!("missing class {name}"))
    }

    fn method(&self, name: &str) -> (SymbolId, TreeId<Untyped>, TreeId<Untyped>) {
        self.ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (self.store.names.resolve(definition.name.as_name().text()) == name).then(|| {
                    (
                        self.index.symbol_at(self.source, tree).unwrap(),
                        tree,
                        definition.rhs.unwrap(),
                    )
                })
            })
            .unwrap_or_else(|| panic!("missing method {name}"))
    }

    fn typer(&mut self) -> SourceTyper<'_> {
        SourceTyper::new(
            &self.ast,
            self.source,
            &self.index,
            &mut self.store,
            self.definitions,
            &self.packages,
        )
    }
}

fn class_symbol(store: &SemanticStore, ty: dotty_core::TypeId) -> SymbolId {
    let Type::TypeRef {
        target: TypeRefTarget::Symbol(symbol),
        ..
    } = store.types.get(ty)
    else {
        panic!("expected canonical class type reference")
    };
    *symbol
}

fn selected_application(typer: &SourceTyper<'_>, root: TreeId<Typed>) -> (SymbolId, TreeId<Typed>) {
    let TreeKind::Apply(application) = &typer.typed_ast().get(root).kind else {
        panic!(
            "expected typed Apply, found {:?}",
            typer.typed_ast().get(root).kind
        )
    };
    let TreeKind::Select(_) = &typer.typed_ast().get(application.function).kind else {
        panic!("operator function should be a typed Select")
    };
    let Type::TermRef {
        target: TermRefTarget::Symbol(symbol),
        ..
    } = typer
        .store()
        .types
        .get(typer.typed_ast().get(application.function).ty)
    else {
        panic!("selected operator must retain a canonical target symbol")
    };
    (*symbol, application.function)
}

#[test]
fn pinned_int_and_boolean_operators_keep_exact_source_symbols_and_shapes() {
    let mut fixture = Fixture::new(ALGORITHMS_SOURCE, ALGORITHMS, "Algorithms.scala");
    let int = fixture.definitions.int;
    let boolean = fixture.definitions.boolean;
    let int_class = class_symbol(&fixture.store, int);
    let boolean_class = class_symbol(&fixture.store, boolean);
    let int_symbols = fixture
        .definitions
        .int_primitive_operator_symbols(&fixture.store)
        .unwrap();
    let boolean_symbols = fixture
        .definitions
        .boolean_primitive_operator_symbols(&fixture.store)
        .unwrap();
    let int_cases = [
        ("add", "+", int_symbols.plus, int),
        ("subtract", "-", int_symbols.minus, int),
        ("multiply", "*", int_symbols.multiply, int),
        ("divide", "/", int_symbols.divide, int),
        ("remainder", "%", int_symbols.remainder, int),
        ("less", "<", int_symbols.less_than, boolean),
        ("lessOrEqual", "<=", int_symbols.less_or_equal, boolean),
        ("greater", ">", int_symbols.greater_than, boolean),
        (
            "greaterOrEqual",
            ">=",
            int_symbols.greater_or_equal,
            boolean,
        ),
    ];
    for (name, operator, expected_symbol, expected_result) in int_cases {
        let (method, _, rhs) = fixture.method(name);
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(source_op)) =
            &fixture.ast.get(rhs).kind
        else {
            panic!("{name} source RHS should be an infix operator")
        };
        assert_eq!(fixture.store.names.resolve(source_op.op.text()), operator);
        let mut typer = fixture.typer();
        let context = typer.expression_context_for(method).unwrap();
        let typed = typer.type_expression(rhs, context).unwrap();
        assert_eq!(
            typer.source_typed_index().get(ALGORITHMS_SOURCE, rhs),
            Some(typed)
        );
        assert_eq!(typer.typed_ast().get(typed).ty, expected_result, "{name}");
        let (selected, function) = selected_application(&typer, typed);
        assert_eq!(selected, expected_symbol, "{name}");
        let Type::TermRef { prefix, .. } =
            typer.store().types.get(typer.typed_ast().get(function).ty)
        else {
            unreachable!()
        };
        let TreeKind::Select(selection) = &typer.typed_ast().get(function).kind else {
            unreachable!()
        };
        assert_eq!(*prefix, typer.typed_ast().get(selection.qualifier).ty);
        let SymbolInfo::Complete(signature) = typer.store().symbols.get(expected_symbol).info
        else {
            panic!("canonical {operator} must have a completed signature")
        };
        let Type::Method(signature) = typer.store().types.get(signature) else {
            panic!("canonical {operator} should have MethodType")
        };
        assert_eq!(signature.kind, MethodKind::Plain);
        assert_eq!(signature.params.len(), 1);
        assert_eq!(signature.params[0].ty, int);
        assert_eq!(signature.result, expected_result);
        assert_eq!(
            typer.store().symbols.get(expected_symbol).owner,
            Some(int_class)
        );
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            unreachable!()
        };
        assert_eq!(application.args.len(), 1);
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(application.args[0]).ty)
                .unwrap(),
            int
        );
    }

    for (name, operator, expected_symbol) in [
        ("both", "&&", boolean_symbols.and_then),
        ("either", "||", boolean_symbols.or_else),
        ("eagerAnd", "&", boolean_symbols.eager_and),
        ("eagerOr", "|", boolean_symbols.eager_or),
        ("exclusiveOr", "^", boolean_symbols.xor),
    ] {
        let (method, _, rhs) = fixture.method(name);
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(source_op)) =
            &fixture.ast.get(rhs).kind
        else {
            panic!("{name} source RHS should be an infix operator")
        };
        assert_eq!(fixture.store.names.resolve(source_op.op.text()), operator);
        let mut typer = fixture.typer();
        let context = typer.expression_context_for(method).unwrap();
        let typed = typer.type_expression(rhs, context).unwrap();
        assert_eq!(
            typer.source_typed_index().get(ALGORITHMS_SOURCE, rhs),
            Some(typed)
        );
        assert_eq!(typer.typed_ast().get(typed).ty, boolean, "{name}");
        let (selected, function) = selected_application(&typer, typed);
        assert_eq!(selected, expected_symbol, "{name}");
        let SymbolInfo::Complete(signature) = typer.store().symbols.get(expected_symbol).info
        else {
            panic!("canonical {operator} should be completed")
        };
        let Type::Method(signature) = typer.store().types.get(signature) else {
            panic!("canonical {operator} should have MethodType")
        };
        assert_eq!(signature.kind, MethodKind::Plain);
        assert_eq!(signature.params.len(), 1);
        assert_eq!(signature.params[0].ty, boolean);
        assert_eq!(signature.result, boolean);
        assert_eq!(
            typer.store().symbols.get(expected_symbol).owner,
            Some(boolean_class)
        );
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            boolean
        );
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == expected_symbol
        ));
    }

    let (method, _, rhs) = fixture.method("negate");
    let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::PrefixOp(source_op)) =
        &fixture.ast.get(rhs).kind
    else {
        panic!("! source RHS should be PrefixOp")
    };
    assert_eq!(fixture.store.names.resolve(source_op.op.text()), "!");
    let expected_boolean = fixture.definitions.boolean;
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();
    assert_eq!(
        typer.source_typed_index().get(ALGORITHMS_SOURCE, rhs),
        Some(typed)
    );
    assert!(matches!(
        typer.typed_ast().get(typed).kind,
        TreeKind::Select(_)
    ));
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(typed).ty),
        Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
            if *symbol == boolean_symbols.unary_not
    ));
    assert_eq!(
        typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap(),
        expected_boolean
    );
}

#[test]
fn mutable_loop_bindings_assignments_and_while_keep_canonical_typed_edges() {
    let mut fixture = Fixture::new(ALGORITHMS_SOURCE, ALGORITHMS, "Algorithms.scala");
    let (method, _, rhs) = fixture.method("sumTo");
    let TreeKind::Block(source_block) = &fixture.ast.get(rhs).kind else {
        panic!("sumTo source RHS should be a block")
    };
    assert_eq!(source_block.stats.len(), 3);
    let mut local_bindings = Vec::new();
    for tree in &source_block.stats[..2] {
        let TreeKind::ValDef(definition) = &fixture.ast.get(*tree).kind else {
            panic!("sumTo starts with local declarations")
        };
        let name = fixture
            .store
            .names
            .resolve(definition.name.as_name().text())
            .to_owned();
        assert!(
            definition
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::Var)
        );
        local_bindings.push((*tree, name));
    }
    let while_tree = source_block.stats[2];
    assert!(matches!(
        fixture.ast.get(while_tree).kind,
        TreeKind::While(_)
    ));
    let source_while = match &fixture.ast.get(while_tree).kind {
        TreeKind::While(while_node) => *while_node,
        _ => unreachable!(),
    };
    let source_assignments = fixture
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            let TreeKind::Assign(assignment) = node.kind else {
                return None;
            };
            let TreeKind::Ident(ident) = &fixture.ast.get(assignment.lhs).kind else {
                panic!("mutable assignment LHS should be an identifier")
            };
            Some((
                tree,
                assignment,
                fixture.store.names.resolve(ident.name.text()).to_owned(),
            ))
        })
        .collect::<Vec<_>>();

    let int_symbols = fixture
        .definitions
        .int_primitive_operator_symbols(&fixture.store)
        .unwrap();
    let int = fixture.definitions.int;
    let boolean = fixture.definitions.boolean;
    let unit = fixture.definitions.unit;
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed_root = typer.type_expression(rhs, context).unwrap();
    assert_eq!(
        typer.source_typed_index().get(ALGORITHMS_SOURCE, rhs),
        Some(typed_root)
    );
    let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed_root).kind else {
        panic!("sumTo typed RHS should retain its block")
    };
    assert_eq!(typed_block.stats.len(), 3);
    let mut symbols = Vec::new();
    for (source_local, expected_name) in &local_bindings {
        let binding = typer
            .typed_local_binding_at(ALGORITHMS_SOURCE, *source_local)
            .expect("ordinary var has canonical typed local binding evidence");
        assert_eq!(
            typer
                .source_typed_index()
                .get(ALGORITHMS_SOURCE, *source_local),
            Some(binding.declaration())
        );
        let semantic = typer.store().symbols.get(binding.symbol());
        assert_eq!(semantic.kind, SymbolKind::Local);
        assert_eq!(semantic.owner, Some(method));
        assert!(semantic.flags.contains(SymbolFlags::MUTABLE));
        assert_eq!(
            typer.store().names.resolve(semantic.name.text()),
            expected_name
        );
        assert_eq!(typer.typed_ast().get(binding.declaration()).ty, int);
        symbols.push(binding.symbol());
    }
    assert_ne!(symbols[0], symbols[1]);

    let typed_while = typer
        .source_typed_index()
        .get(ALGORITHMS_SOURCE, while_tree)
        .unwrap();
    let TreeKind::While(typed_loop) = &typer.typed_ast().get(typed_while).kind else {
        panic!("source while must map to typed While")
    };
    assert_eq!(typer.typed_ast().get(typed_while).ty, unit);
    assert_eq!(typer.typed_ast().get(typed_loop.cond).ty, boolean);
    let (condition_operator, _) = selected_application(&typer, typed_loop.cond);
    assert_eq!(condition_operator, int_symbols.less_than);
    let TreeKind::Block(typed_loop_body) = &typer.typed_ast().get(typed_loop.body).kind else {
        panic!("the two loop statements should remain an ordered typed block")
    };
    assert_eq!(typed_loop_body.stats.len(), 1);
    assert_eq!(
        typer
            .source_typed_index()
            .get(ALGORITHMS_SOURCE, source_while.cond),
        Some(typed_loop.cond)
    );
    assert_eq!(
        typer
            .source_typed_index()
            .get(ALGORITHMS_SOURCE, source_while.body),
        Some(typed_loop.body)
    );
    for (source_assign, source_assignment, target_name) in source_assignments {
        let symbol = match target_name.as_str() {
            "total" => symbols[1],
            "i" => symbols[0],
            other => panic!("unexpected assigned local {other}"),
        };
        let typed_assignment = typer
            .source_typed_index()
            .get(ALGORITHMS_SOURCE, source_assign)
            .expect("source assignment maps to typed Assign");
        let TreeKind::Assign(typed_assignment_node) = &typer.typed_ast().get(typed_assignment).kind
        else {
            panic!("assignment source mapping should be TreeKind::Assign")
        };
        assert_eq!(typer.typed_ast().get(typed_assignment).ty, unit);
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed_assignment_node.lhs).ty),
            Type::TermRef { target: TermRefTarget::Symbol(actual), .. } if *actual == symbol
        ));
        assert_eq!(
            typer
                .source_typed_index()
                .get(ALGORITHMS_SOURCE, source_assignment.lhs),
            Some(typed_assignment_node.lhs)
        );
        let (rhs_operator, _) = selected_application(&typer, typed_assignment_node.rhs);
        assert_eq!(rhs_operator, int_symbols.plus);
    }
    assert_eq!(
        typer
            .widen_expression_type(typer.typed_ast().get(typed_root).ty)
            .unwrap(),
        int
    );
}

#[test]
fn immutable_val_and_mutable_var_have_distinct_source_flags() {
    let mut fixture = Fixture::new(ALGORITHMS_SOURCE, ALGORITHMS, "Algorithms.scala");
    let (_, _, immutable_rhs) = fixture.method("immutableLocal");
    let (mutable_method, _, mutable_rhs) = fixture.method("sumTo");
    let immutable_local = match &fixture.ast.get(immutable_rhs).kind {
        TreeKind::Block(block) => block.stats[0],
        _ => unreachable!(),
    };
    let mutable_local = match &fixture.ast.get(mutable_rhs).kind {
        TreeKind::Block(block) => block.stats[0],
        _ => unreachable!(),
    };
    let immutable_method = fixture.method("immutableLocal").0;
    let mut typer = fixture.typer();
    let immutable_context = typer.expression_context_for(immutable_method).unwrap();
    typer
        .type_expression(immutable_rhs, immutable_context)
        .unwrap();
    let mutable_context = typer.expression_context_for(mutable_method).unwrap();
    typer.type_expression(mutable_rhs, mutable_context).unwrap();
    let immutable = typer
        .typed_local_binding_at(ALGORITHMS_SOURCE, immutable_local)
        .unwrap();
    let mutable = typer
        .typed_local_binding_at(ALGORITHMS_SOURCE, mutable_local)
        .unwrap();
    assert!(
        !typer
            .store()
            .symbols
            .get(immutable.symbol())
            .flags
            .contains(SymbolFlags::MUTABLE)
    );
    assert!(
        typer
            .store()
            .symbols
            .get(mutable.symbol())
            .flags
            .contains(SymbolFlags::MUTABLE)
    );
}

#[test]
fn same_spelled_user_operator_is_not_a_canonical_int_operator() {
    let mut fixture = Fixture::new(CUSTOM_SOURCE, CUSTOM_OPERATORS, "CustomOperators.scala");
    let class = fixture.class("CustomOperators");
    let custom_plus = fixture
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            (fixture
                .store
                .names
                .resolve(definition.name.as_name().text())
                == "+")
                .then(|| fixture.index.symbol_at(CUSTOM_SOURCE, tree).unwrap())
        })
        .unwrap();
    let canonical_plus = fixture
        .definitions
        .int_primitive_operator_symbols(&fixture.store)
        .unwrap()
        .plus;
    let (method, _, rhs) = fixture.method("use");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();
    let (selected, _) = selected_application(&typer, typed);
    assert_eq!(selected, custom_plus);
    assert_ne!(selected, canonical_plus);
    assert_eq!(typer.store().symbols.get(selected).owner, Some(class));
    assert_eq!(typer.store().symbols.get(selected).kind, SymbolKind::Method);
}

#[test]
fn typed_short_circuit_rhs_and_eager_rhs_remain_distinguishable_applications() {
    let mut fixture = Fixture::new(ALGORITHMS_SOURCE, ALGORITHMS, "Algorithms.scala");
    let operators = fixture
        .definitions
        .boolean_primitive_operator_symbols(&fixture.store)
        .unwrap();
    for (name, expected) in [
        ("shortCircuitAnd", operators.and_then),
        ("shortCircuitOr", operators.or_else),
        ("eagerAndEffect", operators.eager_and),
        ("eagerOrEffect", operators.eager_or),
    ] {
        let (method, _, rhs) = fixture.method(name);
        let mut typer = fixture.typer();
        let context = typer.expression_context_for(method).unwrap();
        let typed = typer.type_expression(rhs, context).unwrap();
        let (selected, _) = selected_application(&typer, typed);
        assert_eq!(selected, expected, "{name}");
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            unreachable!()
        };
        assert!(matches!(
            typer.typed_ast().get(application.args[0]).kind,
            TreeKind::Apply(_)
        ));
    }
}

#[test]
fn unsupported_unresolved_int_operator_keeps_its_specific_typer_failure() {
    let mut fixture = Fixture::new(
        SourceId::from_index(42),
        "class UnknownOperator { def unresolved(x: Int, y: Int): Int = x %% y }",
        "UnknownOperator.scala",
    );
    let (method, _, rhs) = fixture.method("unresolved");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    assert!(matches!(
        typer.type_expression(rhs, context),
        Err(TyperError::MemberNotFound { .. })
    ));
    assert!(typer.typed_ast().iter().next().is_none());
}
