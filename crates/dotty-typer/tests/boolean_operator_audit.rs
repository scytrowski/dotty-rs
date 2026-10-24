//! Audit Boolean operator source typing without adding Boolean intrinsics.

use dotty_core::ast::{AstArena, TreeKind, Untyped, UntypedNode};
use dotty_core::types::{MethodKind, TermRefTarget, Type};
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
            "boolean-operator-audit.scala",
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

    fn method_named_symbol(&self, name: &str) -> SymbolId {
        self.ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (self.store.names.resolve(definition.name.as_name().text()) == name)
                    .then(|| self.index.symbol_at(SOURCE, tree).unwrap())
            })
            .unwrap_or_else(|| panic!("missing method symbol {name}"))
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

const SOURCE_OPERATORS: &str = r#"
class BooleanOperators {
  type IntAlias = Int
  def notLiteral(): Boolean = !true
  def andLiteral(): Boolean = true && false
  def notParameter(a: Boolean): Boolean = !a
  def andParameter(a: Boolean, b: Boolean): Boolean = a && b
  def orParameter(a: Boolean, b: Boolean): Boolean = a || b
  def eagerAndParameter(a: Boolean, b: Boolean): Boolean = a & b
  def eagerOrParameter(a: Boolean, b: Boolean): Boolean = a | b
  def xorParameter(a: Boolean, b: Boolean): Boolean = a ^ b
  def equalBoolean(a: Boolean, b: Boolean): Boolean = a == b
  def notEqualBoolean(a: Boolean, b: Boolean): Boolean = a != b
  def equalInt(a: Int): Boolean = a == 1
  def notEqualInt(a: Int): Boolean = a != 1
  def equalWidenedNumeric(a: Int, b: Long): Boolean = a == b
  def equalAlias(a: IntAlias, b: Int): Boolean = a == b
  def incompatibleAliasReference(a: IntAlias, b: IncompatibleReference): Boolean = a == b
  def equalAnyAndInt(a: Any, b: Int): Boolean = a == b
  def incompatibleIntReference(a: Int, b: IncompatibleReference): Boolean = a == b
  def incompatibleReferenceInt(a: IncompatibleReference, b: Int): Boolean = a != b
  def equalAny(a: Any, b: Any): Boolean = a == b
  def notEqualAny(a: Any, b: Any): Boolean = a != b
  def andLocal(): Boolean = { val a = false; val b = true; a && b }
  def andInIf(a: Boolean, b: Boolean): Boolean = if a && b then true else false
  def orInWhile(a: Boolean, b: Boolean): Unit = while a || b do ()
  def shortCircuitAndEffect(): Boolean = false && (1 / 0 > 0)
  def shortCircuitOrEffect(): Boolean = true || (1 / 0 > 0)
  def eagerAndEffect(): Boolean = false & (1 / 0 > 0)
  def eagerOrEffect(): Boolean = true | (1 / 0 > 0)
}
class EqualityReference {
  def ==(value: Int): Boolean = true
  def !=(value: Int): Boolean = false
}
class IncompatibleReference {}
class EqualityUse {
  def narrowEqualInt(b: Int): Boolean = (new EqualityReference) == b
  def narrowEqualReference(): Boolean = (new EqualityReference) == (new EqualityReference)
  def narrowNotEqualInt(b: Int): Boolean = (new EqualityReference) != b
  def narrowNotEqualReference(): Boolean = (new EqualityReference) != (new EqualityReference)
}
"#;

fn contextual_operator_sources() -> (String, Vec<String>, Vec<(String, String)>) {
    let operators = [
        ("not", "!", true),
        ("and", "&&", false),
        ("or", "||", false),
        ("eagerAnd", "&", false),
        ("eagerOr", "|", false),
        ("xor", "^", false),
        ("equal", "==", false),
        ("notEqual", "!=", false),
    ];
    let mut methods = Vec::new();
    let mut names = Vec::new();
    let mut while_conditions = Vec::new();
    for (name, operator, unary) in operators {
        let local_expression = if unary {
            "!left".to_owned()
        } else {
            format!("left {operator} right")
        };
        let condition = if unary {
            "!a".to_owned()
        } else {
            format!("a {operator} b")
        };
        let parameters = if unary {
            "a: Boolean".to_owned()
        } else {
            "a: Boolean, b: Boolean".to_owned()
        };
        let local_bindings = if unary {
            "val left = a;".to_owned()
        } else {
            "val left = a; val right = b;".to_owned()
        };
        let local = format!("{name}Local");
        methods.push(format!(
            "def {local}({parameters}): Boolean = {{ {local_bindings} {local_expression} }}"
        ));
        names.push(local);

        let in_if = format!("if{name}");
        methods.push(format!(
            "def {in_if}({parameters}): Boolean = if {condition} then true else false"
        ));
        names.push(in_if);

        let in_while = format!("while{name}");
        methods.push(format!(
            "def {in_while}({parameters}): Unit = while {condition} do ()"
        ));
        names.push(in_while);
        while_conditions.push((format!("while{name}"), operator.to_owned()));
    }
    (
        format!("class BooleanOperatorContexts {{ {} }}", methods.join("; ")),
        names,
        while_conditions,
    )
}

fn assert_selected_boolean_operator(
    typer: &SourceTyper<'_>,
    tree: TreeId<dotty_core::Typed>,
    expected: SymbolId,
    is_application: bool,
) {
    let function = if is_application {
        let TreeKind::Apply(application) = &typer.typed_ast().get(tree).kind else {
            panic!("binary Boolean operator should type as Apply");
        };
        application.function
    } else {
        tree
    };
    let TreeKind::Select(_) = &typer.typed_ast().get(function).kind else {
        panic!("Boolean operator should select its canonical member");
    };
    assert!(
        matches!(
            typer.store().types.get(typer.typed_ast().get(function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == expected
        ),
        "actual function type {:?}, expected symbol {expected:?}",
        typer.store().types.get(typer.typed_ast().get(function).ty)
    );
}

#[test]
fn boolean_operators_type_through_the_canonical_method_symbols() {
    let mut fixture = Fixture::new(SOURCE_OPERATORS);
    let canonical = fixture
        .definitions
        .boolean_primitive_operator_symbols(&fixture.store)
        .unwrap();
    let boolean = fixture.definitions.boolean;
    for (method_name, symbol, application) in [
        ("notParameter", canonical.unary_not, false),
        ("notLiteral", canonical.unary_not, false),
        ("andLiteral", canonical.and_then, true),
        ("andParameter", canonical.and_then, true),
        ("orParameter", canonical.or_else, true),
        ("eagerAndParameter", canonical.eager_and, true),
        ("eagerOrParameter", canonical.eager_or, true),
        ("xorParameter", canonical.xor, true),
        ("equalBoolean", canonical.equal, true),
        ("notEqualBoolean", canonical.not_equal, true),
    ] {
        let (method, rhs) = fixture.method(method_name);
        let mut typer = fixture.typer();
        let context = typer.expression_context_for(method).unwrap();
        let typed = typer.type_expression(rhs, context).unwrap();
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            boolean
        );
        assert_selected_boolean_operator(&typer, typed, symbol, application);
    }

    for method_name in [
        "andLocal",
        "andInIf",
        "orInWhile",
        "shortCircuitAndEffect",
        "shortCircuitOrEffect",
        "eagerAndEffect",
        "eagerOrEffect",
    ] {
        let (method, rhs) = fixture.method(method_name);
        let mut typer = fixture.typer();
        let context = typer.expression_context_for(method).unwrap();
        assert!(typer.type_expression(rhs, context).is_ok(), "{method_name}");
    }

    let (source, contextual_methods, while_conditions) = contextual_operator_sources();
    let mut contexts = Fixture::new(&source);
    for method in contextual_methods {
        let (symbol, rhs) = contexts.method(&method);
        let mut typer = contexts.typer();
        let context = typer.expression_context_for(symbol).unwrap();
        assert!(typer.type_expression(rhs, context).is_ok(), "{method}");
    }
    for (method, expected_operator) in while_conditions {
        let (_, rhs) = contexts.method(&method);
        let TreeKind::While(loop_tree) = &contexts.ast.get(rhs).kind else {
            panic!("{method} should have a While as its direct method body");
        };
        let actual_operator = match &contexts.ast.get(loop_tree.cond).kind {
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) => {
                contexts.store.names.resolve(infix.op.text())
            }
            TreeKind::PhaseSpecific(UntypedNode::PrefixOp(prefix)) => {
                contexts.store.names.resolve(prefix.op.text())
            }
            kind => panic!(
                "{method} should put its operator directly in the loop condition; got {kind:?}"
            ),
        };
        assert_eq!(actual_operator, expected_operator, "{method}");
    }
}

#[test]
fn boolean_and_universal_equality_use_their_canonical_symbols() {
    let mut fixture = Fixture::new(SOURCE_OPERATORS);
    let any = fixture
        .definitions
        .any_equality_operator_symbols(&fixture.store)
        .unwrap();
    let boolean = fixture.definitions.boolean;
    for (method_name, expected) in [
        ("equalAny", any.equal),
        ("notEqualAny", any.not_equal),
        ("equalInt", any.equal),
        ("notEqualInt", any.not_equal),
    ] {
        let (method, rhs) = fixture.method(method_name);
        let mut typer = fixture.typer();
        let context = typer.expression_context_for(method).unwrap();
        let typed = typer.type_expression(rhs, context).unwrap();
        assert_eq!(
            typer
                .widen_expression_type(typer.typed_ast().get(typed).ty)
                .unwrap(),
            boolean,
            "{method_name}"
        );
        assert_selected_boolean_operator(&typer, typed, expected, true);
    }
}

#[test]
fn universal_equality_checks_scala_39_operand_compatibility() {
    let mut fixture = Fixture::new(SOURCE_OPERATORS);
    let boolean = fixture.definitions.boolean;
    let equality = fixture
        .definitions
        .any_equality_operator_symbols(&fixture.store)
        .unwrap();

    for (method_name, expected_symbol) in [
        ("equalWidenedNumeric", equality.equal),
        ("equalAlias", equality.equal),
        ("equalAnyAndInt", equality.equal),
    ] {
        let (method, rhs) = fixture.method(method_name);
        let mut typer = fixture.typer();
        let context = typer.expression_context_for(method).unwrap();
        let typed = typer.type_expression(rhs, context).unwrap();
        assert_eq!(typer.typed_ast().get(typed).ty, boolean, "{method_name}");
        assert_selected_boolean_operator(&typer, typed, expected_symbol, true);
    }

    for method_name in [
        "incompatibleIntReference",
        "incompatibleReferenceInt",
        "incompatibleAliasReference",
    ] {
        let (method, rhs) = fixture.method(method_name);
        let mut typer = fixture.typer();
        let context = typer.expression_context_for(method).unwrap();
        let error = typer.type_expression(rhs, context).unwrap_err();
        assert!(
            matches!(error, TyperError::IncompatibleEqualityOperands { .. }),
            "{method_name}: {error:?}"
        );
        assert!(
            typer.typed_ast().iter().next().is_none(),
            "{method_name} failure must roll back typed operands"
        );
    }

    let narrow_equal = fixture.method_named_symbol("==");
    let narrow_not_equal = fixture.method_named_symbol("!=");
    for (method_name, expected_symbol) in [
        ("narrowEqualInt", narrow_equal),
        ("narrowEqualReference", equality.equal),
        ("narrowNotEqualInt", narrow_not_equal),
        ("narrowNotEqualReference", equality.not_equal),
    ] {
        let (method, rhs) = fixture.method(method_name);
        let mut typer = fixture.typer();
        let context = typer.expression_context_for(method).unwrap();
        let typed = typer
            .type_expression(rhs, context)
            .unwrap_or_else(|error| panic!("{method_name}: {error:?}"));
        assert_eq!(typer.typed_ast().get(typed).ty, boolean, "{method_name}");
        assert_selected_boolean_operator(&typer, typed, expected_symbol, true);
    }
}

#[test]
fn malformed_boolean_operands_fail_at_the_typer_boundary() {
    let mut fixture = Fixture::new(
        "class InvalidBooleanOperators { def wrongAnd(a: Boolean): Boolean = a && 42; def wrongAndLiteral(): Boolean = true && 42; def wrongNot: Boolean = !1 }",
    );
    let (and_method, and_rhs) = fixture.method("wrongAnd");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(and_method).unwrap();
    let error = typer.type_expression(and_rhs, context).unwrap_err();
    assert!(
        matches!(error, TyperError::OverloadApplicationNoApplicable { .. }),
        "{error:?}"
    );
    assert!(typer.typed_ast().iter().next().is_none());
    drop(typer);

    let (literal_method, literal_rhs) = fixture.method("wrongAndLiteral");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(literal_method).unwrap();
    let error = typer.type_expression(literal_rhs, context).unwrap_err();
    assert!(
        matches!(error, TyperError::OverloadApplicationNoApplicable { .. }),
        "{error:?}"
    );
    assert!(typer.typed_ast().iter().next().is_none());
    drop(typer);

    let (not_method, not_rhs) = fixture.method("wrongNot");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(not_method).unwrap();
    assert!(matches!(
        typer.type_expression(not_rhs, context),
        Err(TyperError::MemberNotFound { .. })
    ));
}

#[test]
fn parameterless_prefix_member_types_as_a_select_without_an_argument_clause() {
    let mut fixture = Fixture::new(
        "class ParameterlessBoolean { def unary_! : Boolean = true; def use(value: ParameterlessBoolean): Boolean = !value }",
    );
    let unary_method = fixture.method_named_symbol("unary_!");
    let (method, rhs) = fixture.method("use");
    let boolean = fixture.definitions.boolean;
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();
    let TreeKind::Select(_) = &typer.typed_ast().get(typed).kind else {
        panic!("a parameterless unary method should select without Apply");
    };
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(typed).ty),
        Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
            if *symbol == unary_method
    ));
    assert_eq!(
        typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap(),
        boolean
    );
}

#[test]
fn user_defined_and_operator_keeps_its_own_method_identity() {
    let mut fixture = Fixture::new(
        "class CustomBoolean { def &&(rhs: Boolean): Boolean = rhs; def use(lhs: CustomBoolean, rhs: Boolean): Boolean = lhs && rhs; def booleanUse(lhs: Boolean, rhs: Boolean): Boolean = lhs && rhs }",
    );
    let custom_operator = fixture.method_named_symbol("&&");
    let (method, rhs) = fixture.method("use");
    let boolean = fixture.definitions.boolean;
    let custom_class = fixture
        .ast
        .iter()
        .find_map(|(tree, _)| {
            let symbol = fixture.index.symbol_at(SOURCE, tree)?;
            (fixture.store.symbols.get(symbol).kind == SymbolKind::Class).then_some(symbol)
        })
        .unwrap();

    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();
    let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
        panic!("a user-defined infix method should type as Apply");
    };
    let TreeKind::Select(selection) = &typer.typed_ast().get(application.function).kind else {
        panic!("the user-defined infix function should be a Select");
    };
    let selected_type = typer.typed_ast().get(application.function).ty;
    assert!(matches!(
        typer.store().types.get(selected_type),
        Type::TermRef { prefix, target: TermRefTarget::Symbol(symbol) }
            if *symbol == custom_operator
                && *prefix == typer.typed_ast().get(selection.qualifier).ty
    ));
    assert_eq!(
        typer.store().symbols.get(custom_operator).owner,
        Some(custom_class)
    );
    assert_eq!(
        typer.store().symbols.get(custom_operator).kind,
        SymbolKind::Method
    );
    assert_eq!(typer.typed_ast().get(typed).ty, boolean);
    assert_eq!(application.args.len(), 1);
    assert_eq!(
        typer
            .widen_expression_type(typer.typed_ast().get(application.args[0]).ty)
            .unwrap(),
        boolean
    );
    let SymbolInfo::Complete(method_type) = typer.store().symbols.get(custom_operator).info else {
        panic!("the source method should have a completed signature");
    };
    let Type::Method(signature) = typer.store().types.get(method_type) else {
        panic!("the source method should have a Method signature");
    };
    assert_eq!(signature.kind, MethodKind::Plain);
    assert_eq!(signature.params.len(), 1);
    assert_eq!(signature.params[0].ty, boolean);
    assert_eq!(signature.result, boolean);
    drop(typer);

    let canonical_and = fixture
        .definitions
        .boolean_primitive_operator_symbols(&fixture.store)
        .unwrap()
        .and_then;
    let (boolean_use_method, boolean_use_rhs) = fixture.method("booleanUse");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(boolean_use_method).unwrap();
    let typed = typer.type_expression(boolean_use_rhs, context).unwrap();
    assert_selected_boolean_operator(&typer, typed, canonical_and, true);
    assert_ne!(canonical_and, custom_operator);
}
