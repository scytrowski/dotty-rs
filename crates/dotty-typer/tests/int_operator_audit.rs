//! Exercise canonical builtin Int operators through the real source pipeline.

use dotty_core::ast::{AstArena, TreeKind, Untyped};
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

const OPERATORS: [(&str, &str, &str); 10] = [
    ("plus", "+", "Int"),
    ("minus", "-", "Int"),
    ("times", "*", "Int"),
    ("divide", "/", "Int"),
    ("remainder", "%", "Int"),
    ("bitwise_and", "&", "Int"),
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

fn assert_operator_typed(
    fixture: &mut Fixture,
    method_name: &str,
    expected_symbol: SymbolId,
    expected_result: dotty_core::TypeId,
) {
    let (method, rhs) = fixture.method(method_name);
    let expected_owner = class_symbol(fixture, fixture.definitions.int);
    let int_type = fixture.definitions.int;
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();
    let (selected, prefix, qualifier_type, argument_type, argument_count) = {
        let application = match &typer.typed_ast().get(typed).kind {
            TreeKind::Apply(application) => application,
            TreeKind::Block(block) => {
                let TreeKind::Apply(application) = &typer.typed_ast().get(block.expr).kind else {
                    panic!("local block body should be a typed Apply");
                };
                application
            }
            kind => panic!("expected a typed Apply or local block, got {kind:?}"),
        };
        let TreeKind::Select(selection) = &typer.typed_ast().get(application.function).kind else {
            panic!("infix method application should select a member");
        };
        let function_type = typer.typed_ast().get(application.function).ty;
        let Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(selected),
        } = typer.store().types.get(function_type)
        else {
            panic!("selected method should retain its semantic SymbolId");
        };
        (
            *selected,
            *prefix,
            typer.typed_ast().get(selection.qualifier).ty,
            typer.typed_ast().get(application.args[0]).ty,
            application.args.len(),
        )
    };
    assert_eq!(selected, expected_symbol, "{method_name}");
    assert_eq!(prefix, qualifier_type);
    assert_eq!(
        typer.widen_expression_type(qualifier_type).unwrap(),
        int_type
    );
    assert_eq!(typer.typed_ast().get(typed).ty, expected_result);
    assert_eq!(
        typer.store().symbols.get(expected_symbol).kind,
        SymbolKind::Method
    );
    assert_eq!(
        typer.store().symbols.get(expected_symbol).owner,
        Some(expected_owner)
    );
    assert_eq!(argument_count, 1);
    assert_eq!(
        typer.widen_expression_type(argument_type).unwrap(),
        int_type
    );
}

#[test]
fn ten_int_operators_type_for_parameters_and_local_vars_with_canonical_symbols() {
    let source = operator_source();
    let mut fixture = Fixture::new(&source);
    let int = fixture.definitions.int;
    let int_class = class_symbol(&fixture, int);

    let operators = fixture
        .definitions
        .int_primitive_operator_symbols(&fixture.store)
        .unwrap()
        .source_all();
    for (index, (method_name, _, result)) in OPERATORS.into_iter().enumerate() {
        let result = if result == "Int" {
            int
        } else {
            fixture.definitions.boolean
        };
        assert_operator_typed(&mut fixture, method_name, operators[index], result);
        assert_operator_typed(
            &mut fixture,
            &format!("{method_name}_local"),
            operators[index],
            result,
        );
    }
    assert!(fixture.store.symbols.contains(int_class));
}

#[test]
fn int_operators_type_inside_literals_assignments_loops_and_nested_expressions() {
    let mut fixture = Fixture::new(
        "class IntExpressions { def literal: Int = 1 + 2; def nested(x: Int): Int = { var y: Int = 0; y = x + (1 + 2); while (y < x) { y = y + 1 }; y } }",
    );
    let symbols = fixture
        .definitions
        .int_primitive_operator_symbols(&fixture.store)
        .unwrap();
    let int_type = fixture.definitions.int;
    let (literal, literal_rhs) = fixture.method("literal");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(literal).unwrap();
    let literal_typed = typer.type_expression(literal_rhs, context).unwrap();
    assert_eq!(typer.typed_ast().get(literal_typed).ty, int_type);
    let TreeKind::Apply(application) = &typer.typed_ast().get(literal_typed).kind else {
        panic!("literal arithmetic should type as Apply");
    };
    let selected_type = typer.typed_ast().get(application.function).ty;
    assert!(matches!(
        typer.store().types.get(selected_type),
        Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == symbols.plus
    ));
    drop(typer);

    let (nested, nested_rhs) = fixture.method("nested");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(nested).unwrap();
    let nested_typed = typer.type_expression(nested_rhs, context).unwrap();
    assert_eq!(
        typer
            .widen_expression_type(typer.typed_ast().get(nested_typed).ty)
            .unwrap(),
        int_type
    );
    let mut selected = Vec::new();
    let mut saw_assignment = false;
    let mut saw_while = false;
    for (_, tree) in typer.typed_ast().iter() {
        match &tree.kind {
            TreeKind::Select(_) => {
                if let Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } = typer.store().types.get(tree.ty)
                {
                    selected.push(*symbol);
                }
            }
            TreeKind::Assign(_) => saw_assignment = true,
            TreeKind::While(_) => saw_while = true,
            _ => {}
        }
    }
    assert!(saw_assignment, "assignment should remain in the typed tree");
    assert!(
        saw_while,
        "while expression should remain in the typed tree"
    );
    assert!(selected.contains(&symbols.plus));
    assert!(selected.contains(&symbols.less_than));
}

#[test]
fn invalid_and_unresolved_operator_sources_fail_without_a_typed_application() {
    let mut fixture = Fixture::new(
        "class Invalid { def intPlusBoolean(x: Int): Int = x + true; def booleanPlusInt(x: Boolean): Boolean = x + 1; def unresolved(x: Int, y: Int): Int = x %% y }",
    );
    let int = fixture.definitions.int;
    let int_class = class_symbol(&fixture, int);

    for (name, expected) in [
        ("intPlusBoolean", "no applicable overload"),
        ("booleanPlusInt", "missing member"),
        ("unresolved", "missing member"),
    ] {
        let (method, rhs) = fixture.method(name);
        let mut typer = fixture.typer();
        let context = typer.expression_context_for(method).unwrap();
        let error = typer.type_expression(rhs, context).unwrap_err();
        match (expected, error) {
            ("no applicable overload", TyperError::OverloadApplicationNoApplicable { .. }) => {}
            ("missing member", TyperError::MemberNotFound { .. }) => {}
            (_, error) => panic!("unexpected diagnostic for {name}: {error:?}"),
        }
        assert!(typer.typed_ast().iter().next().is_none());
    }
    assert!(fixture.store.symbols.contains(int_class));
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

#[test]
fn real_inline_flags_bitwise_and_selects_the_canonical_int_method() {
    let mut fixture = Fixture::new(include_str!("fixtures/int-bitwise-and/IntBitwiseAnd.scala"));
    let operators = fixture
        .definitions
        .int_primitive_operator_symbols(&fixture.store)
        .unwrap();
    let int = fixture.definitions.int;
    assert_operator_typed(&mut fixture, "mask", operators.bitwise_and, int);

    let (method, rhs) = fixture.method("hasFinalFlag");
    let boolean = fixture.definitions.boolean;
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();
    assert_eq!(typer.typed_ast().get(typed).ty, boolean);
    let selected_bitwise_operators = typer
        .typed_ast()
        .iter()
        .filter_map(|(_, tree)| match typer.store().types.get(tree.ty) {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } if *symbol == operators.bitwise_and => Some(*symbol),
            _ => None,
        })
        .count();
    assert_eq!(selected_bitwise_operators, 1);
}

#[test]
fn bitwise_and_does_not_accept_unrelated_reference_receivers() {
    let mut fixture = Fixture::new(include_str!(
        "fixtures/int-bitwise-and/IntBitwiseAndIncompatible.scala"
    ));
    let (method, rhs) = fixture.method("invalid");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let error = typer.type_expression(rhs, context).unwrap_err();
    assert!(
        matches!(error, TyperError::MemberNotFound { .. }),
        "{error:?}"
    );
    assert!(typer.typed_ast().iter().next().is_none());
}
