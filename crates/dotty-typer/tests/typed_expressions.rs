//! End-to-end typed-expression checks over synthetic source.

use dotty_core::ast::TreeKind;
use dotty_core::types::{ClassInfo, TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, NoResolver, Packages, SemanticStore, SourceId, SourceText, SymbolInfo, SymbolKind,
    TreeId, Untyped,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_typer::{ExpressionContext, SourceTyper, TyperError};

fn class_definition(
    parsed: &dotty_parser::ParseResult,
    store: &SemanticStore,
    index: &dotty_core::SourceSemanticIndex,
    source: SourceId,
    wanted: &str,
) -> (dotty_core::SymbolId, TreeId<Untyped>) {
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
}

fn class_template(
    parsed: &dotty_parser::ParseResult,
    class_tree: TreeId<Untyped>,
) -> &dotty_core::ast::Template<Untyped> {
    let TreeKind::TypeDef(definition) = &parsed.ast.get(class_tree).kind else {
        panic!("class identity points to a type definition")
    };
    let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
        panic!("class definition contains a template")
    };
    template
}

#[test]
fn types_a_small_source_program_with_no_resolver_or_jdk_fixture() {
    let text = "class C { def answer: Int = 42 }";
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
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
        "no-resolver.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (method, rhs) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "answer").then(|| {
                (
                    index.symbol_at(source, tree).unwrap(),
                    definition.rhs.unwrap(),
                )
            })
        })
        .expect("source should contain method answer");
    let context = ExpressionContext {
        lexical: index.declaration_context_of(method).unwrap(),
        owner: method,
        local_scopes: None,
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(NoResolver));

    let typed = typer.type_expression(rhs, context).unwrap();

    assert_eq!(
        typer
            .widen_expression_type(typer.typed_ast().get(typed).ty)
            .unwrap(),
        definitions.int
    );
    assert!(matches!(
        typer.typed_ast().get(typed).kind,
        TreeKind::Literal(_)
    ));
}

#[test]
fn types_a_synthetic_field_selection_through_the_public_pipeline() {
    let text = "class C { val value: Int = 1; def use: Int = this.value }";
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
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
        "typed-expressions.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (method, rhs) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "use").then(|| {
                (
                    index.symbol_at(source, tree).unwrap(),
                    definition.rhs.unwrap(),
                )
            })
        })
        .expect("source should contain method use");
    let context = ExpressionContext {
        lexical: index.declaration_context_of(method).unwrap(),
        owner: method,
        local_scopes: None,
    };
    let source_position = parsed.ast.get(rhs).position;
    let field = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "value")
                .then(|| index.symbol_at(source, tree).unwrap())
        })
        .expect("source should contain field value");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );

    let typed = typer.type_expression(rhs, context).unwrap();

    assert!(matches!(
        typer.typed_ast().get(typed).kind,
        TreeKind::Select(_)
    ));
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(typed).ty),
        Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == field
    ));
    let widened = typer
        .widen_expression_type(typer.typed_ast().get(typed).ty)
        .unwrap();
    assert_eq!(widened, definitions.int);
    assert_eq!(typer.typed_ast().get(typed).position, source_position);
    assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed));
    for (_, node) in typer.typed_ast().iter() {
        assert!(typer.store().types.contains(node.ty));
    }
}

#[test]
fn types_a_monomorphic_application_through_the_public_pipeline() {
    let text = "class C { def inc(x: Int): Int = x; def use: Int = inc(1) }";
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
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
        "typed-application.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (method, rhs) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "use").then(|| {
                (
                    index.symbol_at(source, tree).unwrap(),
                    definition.rhs.unwrap(),
                )
            })
        })
        .expect("source should contain method use");
    let context = ExpressionContext {
        lexical: index.declaration_context_of(method).unwrap(),
        owner: method,
        local_scopes: None,
    };
    let source_position = parsed.ast.get(rhs).position;
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );

    let typed = typer.type_expression(rhs, context).unwrap();

    let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
        panic!("source method call should produce a typed Apply");
    };
    assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
    assert_eq!(application.kind, dotty_core::ast::ApplyKind::Regular);
    assert!(matches!(
        typer
            .store()
            .types
            .get(typer.typed_ast().get(application.args[0]).ty),
        Type::Constant(dotty_core::Constant::Int(1))
    ));
    assert_eq!(typer.typed_ast().get(typed).position, source_position);
    assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed));
    for (_, node) in typer.typed_ast().iter() {
        assert!(typer.store().types.contains(node.ty));
    }
}

#[test]
fn parent_constructor_arguments_keep_child_parameter_and_source_identities() {
    let text = "class Base(seed: Int, enabled: Boolean) {}\nclass ByParameter(start: Int, active: Boolean) extends Base(start, active) {}\nclass ByLiteral(start: Int) extends Base(42, true) {}\nclass Dep {}\nclass RefBase(dep: Dep) {}\nclass RefChild(dep: Dep) extends RefBase(dep) {}\n";
    let source = SourceId::from_index(1370);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
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
        "parent-constructor-arguments.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (base, base_tree) = class_definition(&parsed, &store, &index, source, "Base");
    let (by_parameter, by_parameter_tree) =
        class_definition(&parsed, &store, &index, source, "ByParameter");
    let (by_literal, by_literal_tree) =
        class_definition(&parsed, &store, &index, source, "ByLiteral");
    let (ref_base, ref_base_tree) = class_definition(&parsed, &store, &index, source, "RefBase");
    let (ref_child, ref_child_tree) = class_definition(&parsed, &store, &index, source, "RefChild");
    let base_constructor = index
        .symbol_at(source, class_template(&parsed, base_tree).constructor)
        .unwrap();
    let ref_base_constructor = index
        .symbol_at(source, class_template(&parsed, ref_base_tree).constructor)
        .unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );

    for class in [by_parameter, by_literal, ref_child] {
        typer.complete_symbol(class).unwrap();
    }

    for (class, class_tree, expected_parent) in [
        (by_parameter, by_parameter_tree, base),
        (by_literal, by_literal_tree, base),
        (ref_child, ref_child_tree, ref_base),
    ] {
        let template = class_template(&parsed, class_tree);
        let [parent_application] = template.parents.as_slice() else {
            panic!("expected exactly one parent application")
        };
        let typed_application = typer
            .source_typed_index()
            .get(source, *parent_application)
            .expect("parent constructor application has a typed replacement");
        let completed_info = match *typer.store().symbols.info(class) {
            SymbolInfo::Complete(info) => info,
            _ => unreachable!(),
        };
        assert_eq!(typer.complete_symbol(class).unwrap(), completed_info);
        assert_eq!(
            typer.source_typed_index().get(source, *parent_application),
            Some(typed_application),
            "typing the completed class again preserves the same mapping"
        );
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed_application).kind else {
            panic!("parent constructor call retains its application node")
        };
        let typed_function = application.function;
        let TreeKind::Select(_) = &typer.typed_ast().get(typed_function).kind else {
            panic!("parent constructor target retains the selected constructor")
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed_function).ty),
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } if *symbol == if expected_parent == base { base_constructor } else { ref_base_constructor }
        ));
        let TreeKind::Apply(source_application) = &parsed.ast.get(*parent_application).kind else {
            unreachable!()
        };
        assert_eq!(application.args.len(), source_application.args.len());
        for (source_argument, typed_argument) in
            source_application.args.iter().zip(&application.args)
        {
            assert_eq!(
                typer.source_typed_index().get(source, *source_argument),
                Some(*typed_argument),
                "each original argument maps directly to its typed expression"
            );
        }
        let SymbolInfo::Complete(info) = *typer.store().symbols.info(class) else {
            panic!("class completion publishes ClassInfo")
        };
        let Type::ClassInfo(ClassInfo { parents, .. }) = typer.store().types.get(info) else {
            panic!("class completion preserves ClassInfo")
        };
        assert!(matches!(
            typer.store().types.get(parents[0]),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
                if *symbol == expected_parent
        ));
    }

    let template = class_template(&parsed, by_parameter_tree);
    let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
        panic!("child class has a primary constructor")
    };
    let constructor_symbol = index.symbol_at(source, template.constructor).unwrap();
    let parameter_symbols = constructor.value_param_clauses[0]
        .iter()
        .map(|tree| {
            index
                .derived_symbol_at(constructor_symbol, source, *tree)
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(parameter_symbols.iter().all(|symbol| {
        typer.store().symbols.get(*symbol).owner == Some(constructor_symbol)
            && typer.store().symbols.get(*symbol).kind == SymbolKind::Parameter
    }));
    let parent_application = template.parents[0];
    let typed_application = typer
        .source_typed_index()
        .get(source, parent_application)
        .unwrap();
    let TreeKind::Apply(application) = &typer.typed_ast().get(typed_application).kind else {
        unreachable!()
    };
    for (argument, expected_symbol) in application.args.iter().zip(parameter_symbols) {
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(*argument).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if *symbol == expected_symbol
        ));
    }

    let ref_template = class_template(&parsed, ref_child_tree);
    let TreeKind::DefDef(ref_constructor) = &parsed.ast.get(ref_template.constructor).kind else {
        panic!("reference child has a primary constructor")
    };
    let ref_constructor_symbol = index.symbol_at(source, ref_template.constructor).unwrap();
    let ref_parameter_symbol = index
        .derived_symbol_at(
            ref_constructor_symbol,
            source,
            ref_constructor.value_param_clauses[0][0],
        )
        .unwrap();
    let ref_parent_typed = typer
        .source_typed_index()
        .get(source, ref_template.parents[0])
        .unwrap();
    let TreeKind::Apply(ref_application) = &typer.typed_ast().get(ref_parent_typed).kind else {
        unreachable!()
    };
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(ref_application.args[0]).ty),
        Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
            if *symbol == ref_parameter_symbol
    ));

    let literal_template = class_template(&parsed, by_literal_tree);
    let literal_parent = literal_template.parents[0];
    let literal_typed = typer
        .source_typed_index()
        .get(source, literal_parent)
        .unwrap();
    let TreeKind::Apply(literal_application) = &typer.typed_ast().get(literal_typed).kind else {
        unreachable!()
    };
    assert_eq!(literal_application.args.len(), 2);
    assert!(matches!(
        typer
            .store()
            .types
            .get(typer.typed_ast().get(literal_application.args[0]).ty),
        Type::Constant(dotty_core::Constant::Int(42))
    ));
    assert!(matches!(
        typer
            .store()
            .types
            .get(typer.typed_ast().get(literal_application.args[1]).ty),
        Type::Constant(dotty_core::Constant::Boolean(true))
    ));
}

#[test]
fn invalid_parent_constructor_arguments_roll_back_typer_state() {
    let text = "class Base(seed: Int) {}\nclass Derived extends Base(true) {}\n";
    let source = SourceId::from_index(1371);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
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
        "invalid-parent-constructor-argument.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (derived, derived_tree) = class_definition(&parsed, &store, &index, source, "Derived");
    let (base, _) = class_definition(&parsed, &store, &index, source, "Base");
    let parent_application = class_template(&parsed, derived_tree).parents[0];
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );

    let error = typer.complete_symbol(derived).unwrap_err();

    assert!(
        matches!(
            error,
            dotty_typer::TyperError::ApplicationArgumentTypeMismatch {
                argument_index: 0,
                ..
            }
        ),
        "unexpected parent argument error: {error:?}"
    );
    assert_eq!(*typer.store().symbols.info(derived), SymbolInfo::Missing);
    assert_eq!(*typer.store().symbols.info(base), SymbolInfo::Missing);
    assert!(typer.source_typed_index().is_empty());
    assert!(typer.typed_ast().iter().next().is_none());
    assert!(
        typer
            .source_typed_index()
            .get(source, parent_application)
            .is_none()
    );
}

#[test]
fn unsupported_parent_constructor_shapes_fail_at_the_typer_boundary() {
    for (label, text, expected) in [
        (
            "too-few-arguments",
            "class Base(seed: Int, enabled: Boolean) {}\nclass Derived(start: Int) extends Base(start) {}\n",
            "arity",
        ),
        (
            "too-many-arguments",
            "class Base(seed: Int) {}\nclass Derived(start: Int) extends Base(start, true) {}\n",
            "arity",
        ),
        (
            "curried-parent-constructor",
            "class Base(seed: Int)(enabled: Boolean) {}\nclass Derived(start: Int) extends Base(start)(true) {}\n",
            "shape",
        ),
        (
            "inaccessible-parent-constructor",
            "class Base private (seed: Int) {}\nclass Derived(start: Int) extends Base(start) {}\n",
            "access",
        ),
        (
            "invalid-parent-name",
            "class Derived extends Missing(1) {}\n",
            "missing",
        ),
        (
            "alias-to-non-class-parent",
            "class Alias { type Parent = Int; class Derived extends Parent(1) {} }\n",
            "no-constructor",
        ),
        (
            "this-is-not-an-int-parent-argument",
            "class Base(seed: Int) {}\nclass Derived extends Base(this) {}\n",
            "type",
        ),
        (
            "field-prefix-is-not-an-int-parent-argument",
            "class Base(seed: Int) {}\nclass Derived(val flag: Boolean) extends Base(this.flag) {}\n",
            "type",
        ),
    ] {
        let source = SourceId::from_index(1400 + label.len() as u32);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let scanner = ContextualScanner::new(text).unwrap();
        let parsed = dotty_parser::parse_compilation_unit(
            SourceText::new(text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        assert!(
            parsed.diagnostics.is_empty(),
            "{label}: {:?}",
            parsed.diagnostics
        );
        let mut packages = Packages::new();
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "unsupported-parent-shape.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        let (derived, derived_tree) = class_definition(&parsed, &store, &index, source, "Derived");
        let parent_application = class_template(&parsed, derived_tree).parents[0];
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );

        let error = match typer.complete_symbol(derived) {
            Ok(_) => panic!("{label}: invalid parent constructor unexpectedly typed"),
            Err(error) => error,
        };

        let accepted = match expected {
            "arity" => matches!(error, TyperError::ApplicationArityMismatch { .. }),
            "shape" => matches!(error, TyperError::MalformedClassParent { .. }),
            "access" => matches!(error, TyperError::ParentConstructorInaccessible { .. }),
            "type" => matches!(error, TyperError::ApplicationArgumentTypeMismatch { .. }),
            "missing" => matches!(error, TyperError::TypeNameNotFound { .. }),
            "parent" => matches!(error, TyperError::UnresolvedParentClassKind { .. }),
            "no-constructor" => {
                matches!(error, TyperError::ConstructorApplicationUnavailable { .. })
            }
            _ => unreachable!(),
        };
        assert!(accepted, "{label}: unexpected Typer error: {error:?}");
        assert_eq!(*typer.store().symbols.info(derived), SymbolInfo::Missing);
        assert!(typer.source_typed_index().is_empty());
        assert!(typer.typed_ast().iter().next().is_none());
        assert!(
            typer
                .source_typed_index()
                .get(source, parent_application)
                .is_none()
        );
    }
}

#[test]
fn types_return_from_an_explicit_result_method_through_the_public_pipeline() {
    let text = "class C { def choose(flag: Boolean, value: Int, fallback: Int): Int = if flag then return value else fallback }";
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
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
        "typed-return.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (method, rhs) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "choose").then(|| {
                (
                    index.symbol_at(source, tree).unwrap(),
                    definition.rhs.unwrap(),
                )
            })
        })
        .expect("source should contain method choose");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let context = typer.expression_context_for(method).unwrap();

    let typed = typer.type_expression(rhs, context).unwrap();

    assert_eq!(typer.typed_ast().get(typed).ty, definitions.int);
    let TreeKind::If(typed_if) = &typer.typed_ast().get(typed).kind else {
        panic!("source method body should remain a typed if");
    };
    assert_eq!(
        typer.typed_ast().get(typed_if.then_branch).ty,
        definitions.nothing_type
    );
    assert!(matches!(
        typer.typed_ast().get(typed_if.then_branch).kind,
        TreeKind::Return(_)
    ));
    assert_eq!(
        typer
            .widen_expression_type(typer.typed_ast().get(typed_if.else_branch).ty)
            .unwrap(),
        definitions.int
    );
}
