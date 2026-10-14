//! Characterize the current real-source pipeline before builtin Int operator
//! members are introduced. The expected failures in this file are temporary
//! and should be replaced during that symbol-registration work.

use dotty_core::ast::{AstArena, TreeKind, Untyped, UntypedNode};
use dotty_core::types::{MethodKind, TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceSemanticIndex, SourceText, SymbolId,
    SymbolInfo, SymbolKind, TreeId,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_typer::{SourceTyper, TyperError};

const SOURCE: SourceId = SourceId::from_index(0);

struct Fixture {
    ast: AstArena<Untyped>,
    index: SourceSemanticIndex,
    store: SemanticStore,
    definitions: Definitions,
    packages: Packages,
}

impl Fixture {
    fn new(text: &str) -> Self {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let scanner = ContextualScanner::new(text).expect("lexer should accept source");
        let parsed = dotty_parser::parse_compilation_unit(
            SourceText::new(text).unwrap(),
            SOURCE,
            scanner,
            &mut store.names,
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let mut packages = Packages::new();
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            SOURCE,
            "int-operator-audit.scala",
            &mut store,
            &mut packages,
        )
        .expect("namer should accept source declarations");

        Self {
            ast: parsed.ast,
            index,
            store,
            definitions,
            packages,
        }
    }

    fn method(&self, name: &str) -> (SymbolId, TreeId<Untyped>) {
        self.ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (self.store.names.resolve(definition.name.as_name().text()) == name).then(|| {
                    (
                        self.index.symbol_at(SOURCE, tree).unwrap(),
                        definition.rhs.unwrap(),
                    )
                })
            })
            .unwrap_or_else(|| panic!("missing method {name}"))
    }

    fn typer(&mut self) -> SourceTyper<'_> {
        SourceTyper::new(
            &self.ast,
            SOURCE,
            &self.index,
            &mut self.store,
            self.definitions,
            &self.packages,
        )
    }
}

const OPERATORS: [(&str, &str, &str); 9] = [
    ("plus", "+", "Int"),
    ("minus", "-", "Int"),
    ("times", "*", "Int"),
    ("divide", "/", "Int"),
    ("remainder", "%", "Int"),
    ("less", "<", "Boolean"),
    ("less_equal", "<=", "Boolean"),
    ("greater", ">", "Boolean"),
    ("greater_equal", ">=", "Boolean"),
];

fn operator_source() -> String {
    let mut methods = Vec::new();
    for (name, operator, result) in OPERATORS {
        methods.push(format!(
            "def {name}(x: Int, y: Int): {result} = x {operator} y"
        ));
        methods.push(format!(
            "def {name}_local(x: Int): {result} = {{ var y: Int = 1; x {operator} y }}"
        ));
    }
    format!("class IntOperators {{ {} }}", methods.join("; "))
}

fn class_symbol(fixture: &Fixture, ty: dotty_core::TypeId) -> SymbolId {
    let Type::TypeRef {
        target: TypeRefTarget::Symbol(symbol),
        ..
    } = fixture.store.types.get(ty)
    else {
        panic!("builtin type should have a canonical class symbol");
    };
    *symbol
}

fn assert_member_lookup_blocked(
    fixture: &mut Fixture,
    method_name: &str,
    expected_receiver: SymbolId,
    expected_operator: &str,
) {
    let (method, rhs) = fixture.method(method_name);
    let operator_tree = match &fixture.ast.get(rhs).kind {
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)) => rhs,
        TreeKind::Block(block) => block.expr,
        kind => panic!("expected an infix body or local block, got {kind:?}"),
    };
    let TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) = &fixture.ast.get(operator_tree).kind
    else {
        panic!("expected a source infix operator node");
    };
    assert_eq!(
        fixture.store.names.resolve(infix.op.text()),
        expected_operator
    );
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let error = typer.type_expression(rhs, context).unwrap_err();
    let TyperError::MemberLookup(error) = error else {
        panic!("expected a member lookup error, got {error:?}");
    };
    let dotty_typer::MemberLookupError::ClassInfoUnavailable { symbol, state } = *error else {
        panic!("expected missing builtin ClassInfo, got {error:?}");
    };
    assert_eq!(symbol, expected_receiver, "{method_name}");
    assert_eq!(state, dotty_typer::SymbolInfoState::Missing);
    assert_eq!(typer.source_typed_index().get(SOURCE, rhs), None);
    assert!(typer.typed_ast().iter().next().is_none());
}

#[test]
fn nine_int_operators_reach_typer_and_fail_at_member_lookup_for_parameters_and_vars() {
    let source = operator_source();
    let mut fixture = Fixture::new(&source);
    let int = fixture.definitions.int;
    let int_class = class_symbol(&fixture, int);

    for (method_name, operator, _) in OPERATORS {
        assert_member_lookup_blocked(&mut fixture, method_name, int_class, operator);
        assert_member_lookup_blocked(
            &mut fixture,
            &format!("{method_name}_local"),
            int_class,
            operator,
        );
    }
}

#[test]
fn invalid_and_unresolved_operator_sources_fail_without_a_typed_application() {
    let mut fixture = Fixture::new(
        "class Invalid { def intPlusBoolean(x: Int): Int = x + true; def booleanPlusInt(x: Boolean): Boolean = x + 1; def unresolved(x: Int, y: Int): Int = x %% y }",
    );
    let int = fixture.definitions.int;
    let boolean = fixture.definitions.boolean;
    let int_class = class_symbol(&fixture, int);
    let boolean_class = class_symbol(&fixture, boolean);

    for (name, receiver, operator) in [
        ("intPlusBoolean", int_class, "+"),
        ("booleanPlusInt", boolean_class, "+"),
        ("unresolved", int_class, "%%"),
    ] {
        assert_member_lookup_blocked(&mut fixture, name, receiver, operator);
    }
}

#[test]
fn user_defined_plus_resolves_to_its_own_method_identity() {
    let mut fixture = Fixture::new(
        "class Custom { def +(x: Int): Int = x; def use(left: Custom, right: Int): Int = left + right }",
    );
    let class = fixture
        .ast
        .iter()
        .find_map(|(tree, _)| {
            let symbol = fixture.index.symbol_at(SOURCE, tree)?;
            (fixture.store.symbols.get(symbol).kind == SymbolKind::Class).then_some(symbol)
        })
        .unwrap();
    let operator = fixture
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
                .then(|| fixture.index.symbol_at(SOURCE, tree).unwrap())
        })
        .unwrap();
    let (method, rhs) = fixture.method("use");
    let int = fixture.definitions.int;
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();
    let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
        panic!("infix method application should type as Apply");
    };
    let function = application.function;
    let argument = application.args[0];
    let TreeKind::Select(selection) = &typer.typed_ast().get(function).kind else {
        panic!("infix function should type as Select");
    };
    let qualifier = selection.qualifier;
    let selected_type = typer.typed_ast().get(function).ty;
    let Type::TermRef {
        prefix,
        target: TermRefTarget::Symbol(selected),
    } = typer.store().types.get(selected_type)
    else {
        panic!("selected method should carry its SymbolId");
    };
    assert_eq!(*selected, operator);
    let receiver_type = typer.typed_ast().get(qualifier).ty;
    assert_eq!(*prefix, receiver_type);
    assert_eq!(typer.store().symbols.get(operator).kind, SymbolKind::Method);
    assert_eq!(typer.store().symbols.get(operator).owner, Some(class));
    let argument_type = typer.typed_ast().get(argument).ty;
    let qualifier_type = typer.typed_ast().get(qualifier).ty;
    assert_eq!(typer.widen_expression_type(argument_type).unwrap(), int);
    assert_eq!(typer.typed_ast().get(typed).ty, int);
    let SymbolInfo::Complete(operator_info) = typer.store().symbols.get(operator).info else {
        panic!("user-defined method should have completed source signature");
    };
    let Type::Method(operator_method) = typer.store().types.get(operator_info) else {
        panic!("user-defined + should have an ordinary method type");
    };
    assert_eq!(operator_method.kind, MethodKind::Plain);
    assert_eq!(operator_method.params.len(), 1);
    assert_eq!(operator_method.params[0].ty, int);
    assert_eq!(operator_method.result, int);
    let qualifier_type = typer.widen_expression_type(qualifier_type).unwrap();
    assert!(matches!(
        typer.store().types.get(qualifier_type),
        Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == class
    ));
}
