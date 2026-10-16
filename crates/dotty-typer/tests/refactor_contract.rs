//! Public-API characterization tests for the typer refactor contract.
//!
//! Later typer refactors must keep these semantic assertions unchanged unless
//! a separately scoped behavior issue explicitly updates the contract.

use std::collections::HashSet;

use dotty_core::ast::{AstArena, TreeKind, Typed, Untyped};
use dotty_core::types::{TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, Name, Namespace, Packages, SemanticStore, SourceId, SourceSemanticIndex,
    SourceText, SymbolId, SymbolInfo, SymbolKind, TreeId, TypeId,
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
        let scanner = ContextualScanner::new(text).expect("synthetic scanner should construct");
        let parsed = dotty_parser::parse_compilation_unit(
            SourceText::new(text).expect("synthetic source should be valid"),
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
            "refactor-contract.scala",
            &mut store,
            &mut packages,
        )
        .expect("synthetic source should be nameable");

        Self {
            ast: parsed.ast,
            index,
            store,
            definitions,
            packages,
        }
    }

    fn methods_named(&self, name: &str) -> Vec<(TreeId<Untyped>, SymbolId, TreeId<Untyped>)> {
        self.ast
            .iter()
            .filter_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                if self.store.names.resolve(definition.name.as_name().text()) != name {
                    return None;
                }
                Some((tree, self.index.symbol_at(SOURCE, tree)?, definition.rhs?))
            })
            .collect()
    }

    fn declaration_tree(&self, name: &str, kind: SymbolKind) -> TreeId<Untyped> {
        self.ast
            .iter()
            .find_map(|(tree, node)| {
                let declaration_name = match &node.kind {
                    TreeKind::ValDef(definition) => definition.name.as_name(),
                    TreeKind::DefDef(definition) => definition.name.as_name(),
                    _ => return None,
                };
                if self.store.names.resolve(declaration_name.text()) != name {
                    return None;
                }
                let symbol = self.index.symbol_at(SOURCE, tree)?;
                (self.store.symbols.get(symbol).kind == kind).then_some(tree)
            })
            .expect("expected source declaration tree")
    }

    fn method(&self, name: &str) -> (TreeId<Untyped>, SymbolId, TreeId<Untyped>) {
        let matches = self.methods_named(name);
        assert_eq!(matches.len(), 1, "expected one method named {name}");
        matches[0]
    }

    fn symbol_named(&self, name: &str, kind: SymbolKind) -> SymbolId {
        self.ast
            .iter()
            .find_map(|(tree, _node)| {
                let symbol = self.index.symbol_at(SOURCE, tree)?;
                let value = self.store.symbols.get(symbol);
                (value.kind == kind && self.store.names.resolve(value.name.text()) == name)
                    .then_some(symbol)
            })
            .expect("expected named symbol")
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

#[test]
fn normalized_observation_uses_semantic_names_and_shapes() {
    let mut fixture = Fixture::new("class C { val value: Int = 1; def use: Int = this.value }");
    let (_, method, rhs) = fixture.method("use");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();

    let snapshot = tree_snapshot(&typer, typed);
    assert_eq!(
        snapshot,
        "Select(value,Field:C.value,TermRef(Field:C.value),This(_):ThisType(Class:C))"
    );
    assert_eq!(typer.source_typed_index().get(SOURCE, rhs), Some(typed));
}

#[test]
fn normalized_symbol_info_omits_type_arena_indices() {
    let mut fixture = Fixture::new("class C { val value: Int = 1 }");
    let value = fixture.symbol_named("value", SymbolKind::Field);
    let mut typer = fixture.typer();
    typer.complete_symbol(value).unwrap();

    assert_eq!(
        symbol_info_snapshot(&typer, value),
        "Complete(TypeRef(Class:Int))"
    );
}

#[test]
fn normalized_errors_keep_variants_and_semantic_payloads() {
    let mut fixture =
        Fixture::new("class C { def target(x: Int): Int = x; def use: Int = target() }");
    let (_, method, rhs) = fixture.method("use");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();

    let error = typer.type_expression(rhs, context).unwrap_err();

    assert_eq!(
        error_snapshot(&typer, &error),
        "ApplicationArityMismatch(expected=1,actual=0)"
    );
}

#[test]
fn local_val_shadows_a_method_parameter() {
    let mut fixture =
        Fixture::new("class C { def use(value: Int): Int = { val value = 2; value } }");
    let (method_tree, method, rhs) = fixture.method("use");
    let parameter_tree = match &fixture.ast.get(method_tree).kind {
        TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
        _ => unreachable!(),
    };
    let parameter = fixture.index.symbol_at(SOURCE, parameter_tree).unwrap();
    let local_tree = fixture
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            (fixture
                .store
                .names
                .resolve(definition.name.as_name().text())
                == "value"
                && fixture.index.symbol_at(SOURCE, tree).is_none())
            .then_some(tree)
        })
        .unwrap();
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();
    let local = typer.local_symbol_at(SOURCE, local_tree).unwrap();
    let block_expr = match &typer.typed_ast().get(typed).kind {
        TreeKind::Block(block) => block.expr,
        _ => panic!("method body should remain a block"),
    };

    assert_ne!(local, parameter);
    assert_eq!(term_ref_symbol(&typer, block_expr), Some(local));
    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(block_expr).ty),
        "TermRef(Local:C.use.value)"
    );
}

#[test]
fn method_parameter_shadows_a_class_member() {
    let mut fixture =
        Fixture::new("class C { val value: Int = 1; def use(value: Int): Int = value }");
    let (method_tree, method, rhs) = fixture.method("use");
    let parameter_tree = match &fixture.ast.get(method_tree).kind {
        TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
        _ => unreachable!(),
    };
    let parameter = fixture.index.symbol_at(SOURCE, parameter_tree).unwrap();
    let field_tree = fixture.declaration_tree("value", SymbolKind::Field);
    let field = fixture.index.symbol_at(SOURCE, field_tree).unwrap();
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();

    assert_ne!(parameter, field);
    assert_eq!(term_ref_symbol(&typer, typed), Some(parameter));
    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(typed).ty),
        "TermRef(Parameter:C.use.value)"
    );
}

#[test]
fn typed_local_binding_preserves_the_source_declaration_and_symbol_identity() {
    let mut fixture = Fixture::new("class C { def run(x: Int): Int = { val saved = x; saved } }");
    let (_, method, rhs) = fixture.method("run");
    let local_tree = fixture
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            (fixture
                .store
                .names
                .resolve(definition.name.as_name().text())
                == "saved")
                .then_some(tree)
        })
        .unwrap();
    let method_tree = fixture.method("run").0;
    let parameter_tree = match &fixture.ast.get(method_tree).kind {
        TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
        _ => unreachable!(),
    };
    let parameter = fixture.index.symbol_at(SOURCE, parameter_tree).unwrap();
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    typer.type_expression(rhs, context).unwrap();

    let binding = typer.typed_local_binding_at(SOURCE, local_tree).unwrap();
    let typed_declaration = typer.typed_ast().get(binding.declaration());
    assert!(matches!(typed_declaration.kind, TreeKind::ValDef(_)));
    assert_eq!(
        typer.source_typed_index().get(SOURCE, local_tree),
        Some(binding.declaration())
    );
    assert_eq!(
        typer.local_symbol_at(SOURCE, local_tree),
        Some(binding.symbol())
    );
    assert_ne!(binding.symbol(), parameter);
    let TreeKind::Block(block) = &typer
        .typed_ast()
        .get(typer.source_typed_index().get(SOURCE, rhs).unwrap())
        .kind
    else {
        panic!("method body should remain a block")
    };
    assert_eq!(term_ref_symbol(&typer, block.expr), Some(binding.symbol()));
    assert_eq!(
        typer.store().symbols.get(binding.symbol()).owner,
        Some(method)
    );
}

#[test]
fn typed_local_bindings_keep_parameter_and_nested_local_shadowing_distinct() {
    let mut fixture = Fixture::new(
        "class C { def run(x: Int): Int = { val x = 1; { val value = x; { val value = 2; value } } } }",
    );
    let (method_tree, method, rhs) = fixture.method("run");
    let parameter_tree = match &fixture.ast.get(method_tree).kind {
        TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
        _ => unreachable!(),
    };
    let parameter = fixture.index.symbol_at(SOURCE, parameter_tree).unwrap();
    let local_trees = fixture
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            (matches!(node.kind, TreeKind::ValDef(_))
                && fixture.index.symbol_at(SOURCE, tree).is_none())
            .then_some(tree)
        })
        .collect::<Vec<_>>();
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    typer.type_expression(rhs, context).unwrap();

    let bindings = local_trees
        .iter()
        .map(|tree| {
            typer
                .typed_local_binding_at(SOURCE, *tree)
                .unwrap_or_else(|| panic!("missing typed local binding for {tree:?}"))
        })
        .collect::<Vec<_>>();
    assert_eq!(bindings.len(), 3);
    assert_ne!(bindings[0].symbol(), parameter);
    assert_ne!(bindings[0].symbol(), bindings[1].symbol());
    assert_ne!(bindings[1].symbol(), bindings[2].symbol());
    assert_ne!(bindings[0].declaration(), bindings[1].declaration());
    assert_ne!(bindings[1].declaration(), bindings[2].declaration());
    for binding in bindings {
        assert_eq!(
            typer.store().symbols.get(binding.symbol()).owner,
            Some(method)
        );
        assert!(matches!(
            typer.typed_ast().get(binding.declaration()).kind,
            TreeKind::ValDef(_)
        ));
    }
}

#[test]
fn failed_expression_transaction_does_not_publish_a_typed_local_binding() {
    let mut fixture = Fixture::new("class C { def run(x: Int): Int = { val saved = x; missing } }");
    let (_, method, rhs) = fixture.method("run");
    let local_tree = fixture
        .ast
        .iter()
        .find_map(|(tree, node)| matches!(node.kind, TreeKind::ValDef(_)).then_some(tree))
        .unwrap();
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();

    assert!(typer.type_expression(rhs, context).is_err());
    assert_eq!(typer.typed_local_binding_at(SOURCE, local_tree), None);
    assert_eq!(typer.local_symbol_at(SOURCE, local_tree), None);
    assert_eq!(typer.source_typed_index().get(SOURCE, local_tree), None);
}

#[test]
fn local_binding_identity_order_is_deterministic_across_fresh_frontends() {
    fn snapshot() -> Vec<(u32, u32)> {
        let mut fixture = Fixture::new(
            "class C { def run(x: Int): Int = { val saved = x; { val value = 1; { val value = 2; value } } } }",
        );
        let (_, method, rhs) = fixture.method("run");
        let source_locals = fixture
            .ast
            .iter()
            .filter_map(|(tree, node)| {
                (matches!(node.kind, TreeKind::ValDef(_))
                    && fixture.index.symbol_at(SOURCE, tree).is_none())
                .then_some(tree)
            })
            .collect::<Vec<_>>();
        let mut typer = fixture.typer();
        let context = typer.expression_context_for(method).unwrap();
        typer.type_expression(rhs, context).unwrap();
        let mut bindings = source_locals
            .into_iter()
            .map(|tree| typer.typed_local_binding_at(SOURCE, tree).unwrap())
            .map(|binding| (binding.declaration().index(), binding.symbol().index()))
            .collect::<Vec<_>>();
        bindings.sort_by_key(|(declaration, _)| *declaration);
        bindings
    }

    assert_eq!(snapshot(), snapshot());
}

#[test]
fn deferred_local_value_forms_are_not_exported_as_ordinary_bindings() {
    let mut fixture = Fixture::new("class C { def run: Int = { lazy val value = 1; value } }");
    let (_, method, rhs) = fixture.method("run");
    let local_tree = fixture
        .ast
        .iter()
        .find_map(|(tree, node)| matches!(node.kind, TreeKind::ValDef(_)).then_some(tree))
        .unwrap();
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    typer.type_expression(rhs, context).unwrap();

    assert!(typer.local_symbol_at(SOURCE, local_tree).is_some());
    assert_eq!(typer.typed_local_binding_at(SOURCE, local_tree), None);
}

#[test]
fn local_method_forward_reference_resolves_to_its_later_declaration() {
    let mut fixture = Fixture::new(
        "object C { def outer: Int = { val before = later(); def later(): Int = 1; before } }",
    );
    let (_, method, rhs) = fixture.method("outer");
    let later_tree = fixture
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
                == "later")
                .then_some(tree)
        })
        .unwrap();
    let reference_tree = fixture
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::Ident(ident) = &node.kind else {
                return None;
            };
            (fixture.store.names.resolve(ident.name.text()) == "later").then_some(tree)
        })
        .unwrap();
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    typer.type_expression(rhs, context).unwrap();
    let local_method = typer.local_method_symbol_at(SOURCE, later_tree).unwrap();
    let typed_reference = typer
        .source_typed_index()
        .get(SOURCE, reference_tree)
        .unwrap();

    assert_eq!(term_ref_symbol(&typer, typed_reference), Some(local_method));
    assert_eq!(symbol_label(&typer, local_method), "Method:C$.outer.later");
}

#[test]
fn nested_block_declarations_do_not_leak_to_the_outer_block() {
    let mut fixture =
        Fixture::new("class C { def use: Int = { { val hidden = 1; hidden }; hidden } }");
    let (_, method, rhs) = fixture.method("use");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();

    let error = typer.type_expression(rhs, context).unwrap_err();

    assert_eq!(error_snapshot(&typer, &error), "TermNameNotFound(hidden)");
}

#[test]
fn imported_source_name_is_used_after_local_scopes_miss() {
    let mut fixture = Fixture::new(
        "object Library { val answer: Int = 42 }; object User { import Library.answer; def fallback: Int = answer; def local: Int = { val answer = 1; answer } }",
    );
    let (_, fallback_method, fallback_rhs) = fixture.method("fallback");
    let (_, local_method, local_rhs) = fixture.method("local");
    let imported = fixture.declaration_tree("answer", SymbolKind::Field);
    let imported_symbol = fixture.index.symbol_at(SOURCE, imported).unwrap();
    let local_tree = fixture
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            (fixture
                .store
                .names
                .resolve(definition.name.as_name().text())
                == "answer"
                && fixture.index.symbol_at(SOURCE, tree).is_none())
            .then_some(tree)
        })
        .unwrap();
    let mut typer = fixture.typer();
    let fallback_context = typer.expression_context_for(fallback_method).unwrap();
    let fallback = typer
        .type_expression(fallback_rhs, fallback_context)
        .unwrap();
    let local_context = typer.expression_context_for(local_method).unwrap();
    let local = typer.type_expression(local_rhs, local_context).unwrap();
    let local_symbol = typer.local_symbol_at(SOURCE, local_tree).unwrap();

    assert_eq!(term_ref_symbol(&typer, fallback), Some(imported_symbol));
    let local_expr = match &typer.typed_ast().get(local).kind {
        TreeKind::Block(block) => block.expr,
        _ => panic!("local method body should remain a block"),
    };
    assert_eq!(term_ref_symbol(&typer, local_expr), Some(local_symbol));
}

#[test]
fn monomorphic_overload_selects_the_unique_most_specific_candidate() {
    let mut fixture = Fixture::new(
        "class Parent {}; class Child extends Parent {}; class C { def choose(x: Parent): Int = 1; def choose(x: Child): Int = 2; def use(x: Child): Int = choose(x) }",
    );
    let (_, use_method, rhs) = fixture.method("use");
    let candidates = fixture.methods_named("choose");
    assert_eq!(candidates.len(), 2);
    let most_specific = candidates[1].1;
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(use_method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();

    assert_eq!(call_target(&typer, typed), Some(most_specific));
    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(typed).ty),
        "TypeRef(Class:Int)"
    );
}

#[test]
fn equally_specific_generic_and_monomorphic_overloads_remain_ambiguous() {
    let mut fixture = Fixture::new(
        "class A {}; class C { def choose(x: A): Int = 1; def choose[T](x: T): Int = 2; def use(x: A): Int = choose(x) }",
    );
    let (_, use_method, rhs) = fixture.method("use");
    let declared_candidates = fixture
        .methods_named("choose")
        .into_iter()
        .map(|(_, symbol, _)| symbol)
        .collect::<HashSet<_>>();
    assert_eq!(declared_candidates.len(), 2);
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(use_method).unwrap();
    let error = typer.type_expression(rhs, context).unwrap_err();
    let candidates = match &error {
        TyperError::AmbiguousOverloadApplication { candidates, .. } => candidates,
        other => panic!("expected ambiguity, got {other:?}"),
    };
    assert_eq!(candidates.len(), 2);
    assert_eq!(
        candidates.iter().copied().collect::<HashSet<_>>(),
        declared_candidates
    );
    assert_eq!(
        error_snapshot(&typer, &error),
        "AmbiguousOverloadApplication(Method:C.choose=Missing|Method:C.choose=Missing)"
    );
}

#[test]
fn generic_overload_wins_when_the_monomorphic_candidate_does_not_apply() {
    let mut fixture = Fixture::new(
        "class Parent {}; class Child extends Parent {}; class C { def choose[T <: Parent](x: T): T = x; def choose(x: Parent): Parent = x; def use(x: Child): Parent = choose(x) }",
    );
    let (_, use_method, rhs) = fixture.method("use");
    let generic = fixture
        .methods_named("choose")
        .into_iter()
        .find(|(tree, _, _)| matches!(&fixture.ast.get(*tree).kind, TreeKind::DefDef(def) if !def.type_params.is_empty()))
        .unwrap()
        .1;
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(use_method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();

    assert_eq!(call_target(&typer, typed), Some(generic));
}

#[test]
fn explicit_type_application_preserves_its_exact_function_reference() {
    let mut fixture = Fixture::new(
        "class C { def identity[A](value: A): A = value; def use: Int = identity[Int](1) }",
    );
    let (_, identity, _) = fixture.method("identity");
    let (_, use_method, rhs) = fixture.method("use");
    let source_type_apply = match &fixture.ast.get(rhs).kind {
        TreeKind::Apply(application) => application.function,
        _ => panic!("source expression should be an application"),
    };
    let source_type_arg = match &fixture.ast.get(source_type_apply).kind {
        TreeKind::TypeApply(application) => application.args[0],
        _ => panic!("source function should be explicitly type applied"),
    };
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(use_method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();
    let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
        panic!("source call should remain a typed application")
    };
    let TreeKind::TypeApply(type_apply) = &typer.typed_ast().get(application.function).kind else {
        panic!("typed call should preserve its explicit type application")
    };
    let argument = type_apply.args[0];

    assert_eq!(term_ref_symbol(&typer, type_apply.function), Some(identity));
    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(argument).ty),
        "TypeRef(Class:Int)"
    );
    assert_eq!(
        typer.source_typed_index().get(SOURCE, source_type_arg),
        Some(argument)
    );
}

#[test]
fn generic_inference_preserves_poly_binder_indices() {
    let mut fixture = Fixture::new(
        "class C { def choose[A, B](left: A, right: B): B = right; def use: Boolean = choose(1, true) }",
    );
    let (_, choose, _) = fixture.method("choose");
    let (_, use_method, rhs) = fixture.method("use");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(use_method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();
    let signature = typer.complete_symbol(choose).unwrap();
    let Type::Poly(poly) = typer.store().types.get(signature) else {
        panic!("generic method signature should retain its Poly binder")
    };
    let Type::Method(method) = typer.store().types.get(poly.result) else {
        panic!("Poly result should be the method clause")
    };

    assert_eq!(poly.params.len(), 2);
    assert!(
        matches!(typer.store().types.get(method.params[0].ty), Type::ParamRef { binder, index: 0 } if *binder == signature)
    );
    assert!(
        matches!(typer.store().types.get(method.params[1].ty), Type::ParamRef { binder, index: 1 } if *binder == signature)
    );
    assert!(
        matches!(typer.store().types.get(method.result), Type::ParamRef { binder, index: 1 } if *binder == signature)
    );
    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(typed).ty),
        "TypeRef(Class:Boolean)"
    );
}

#[test]
fn explicit_using_application_consumes_the_contextual_clause() {
    let mut fixture = Fixture::new(
        "class C { def provide(using value: Int): Int = value; def use: Int = provide(using 1) }",
    );
    let (_, use_method, rhs) = fixture.method("use");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(use_method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();
    let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
        panic!("using call should produce an Apply")
    };

    assert_eq!(application.kind, dotty_core::ast::ApplyKind::Using);
    assert_eq!(application.args.len(), 1);
    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(typed).ty),
        "TypeRef(Class:Int)"
    );
}

#[test]
fn primary_constructor_application_preserves_constructor_identity() {
    let mut fixture = Fixture::new(
        "class Point(x: Int, y: Int); class Use { def make: Point = new Point(1, 2) }",
    );
    let class = fixture.symbol_named("Point", SymbolKind::Class);
    let constructors = constructors_of(&mut fixture, class);
    assert_eq!(constructors.len(), 1);
    let (_, method, rhs) = fixture.method("make");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();
    let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
        panic!("constructor call should remain an Apply")
    };

    assert_eq!(application.args.len(), 2);
    assert_eq!(
        term_ref_symbol(&typer, application.function),
        Some(constructors[0])
    );
    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(typed).ty),
        "TypeRef(Class:Point)"
    );
}

#[test]
fn source_constructor_and_reference_local_keep_canonical_identities() {
    let mut fixture = Fixture::new(
        "class Counter(start: Int, enabled: Boolean) { def get(): Int = 1 }; class Client { def make(): Counter = new Counter(42, true); def direct(): Int = new Counter(42, true).get(); def run(seed: Int): Int = { val counter = new Counter(seed, true); counter.get() } }",
    );
    let counter = fixture.symbol_named("Counter", SymbolKind::Class);
    let client = fixture.symbol_named("Client", SymbolKind::Class);
    let constructors = constructors_of(&mut fixture, counter);
    assert_eq!(constructors.len(), 1);
    let constructor = constructors[0];
    assert_eq!(fixture.store.symbols.get(constructor).owner, Some(counter));

    let method_in = |fixture: &Fixture, owner: SymbolId, name: &str| {
        fixture
            .ast
            .iter()
            .find_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                if fixture
                    .store
                    .names
                    .resolve(definition.name.as_name().text())
                    != name
                {
                    return None;
                }
                let symbol = fixture.index.symbol_at(SOURCE, tree)?;
                (fixture.store.symbols.get(symbol).owner == Some(owner)).then_some((
                    tree,
                    symbol,
                    definition.rhs?,
                ))
            })
            .unwrap_or_else(|| panic!("expected {name} in class {owner:?}"))
    };

    let (_, make_method, make_rhs) = method_in(&fixture, client, "make");
    let (_, direct_method, direct_rhs) = method_in(&fixture, client, "direct");
    let (_, run_method, run_rhs) = method_in(&fixture, client, "run");
    let local_tree = fixture
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            (fixture
                .store
                .names
                .resolve(definition.name.as_name().text())
                == "counter")
                .then_some(tree)
        })
        .expect("expected source local counter");
    let run_tree = method_in(&fixture, client, "run").0;
    let seed_tree = match &fixture.ast.get(run_tree).kind {
        TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
        _ => unreachable!(),
    };
    let seed = fixture.index.symbol_at(SOURCE, seed_tree).unwrap();

    let mut typer = fixture.typer();
    let make_context = typer.expression_context_for(make_method).unwrap();
    let make = typer.type_expression(make_rhs, make_context).unwrap();
    let TreeKind::Apply(constructor_call) = &typer.typed_ast().get(make).kind else {
        panic!("new Counter(args) should be a constructor Apply")
    };
    assert_eq!(constructor_call.args.len(), 2);
    assert_eq!(call_target(&typer, make), Some(constructor));
    let TreeKind::Select(constructor_selection) =
        &typer.typed_ast().get(constructor_call.function).kind
    else {
        panic!("constructor callee should be Select")
    };
    let receiver = constructor_selection.qualifier;
    assert!(matches!(
        typer.typed_ast().get(constructor_selection.qualifier).kind,
        TreeKind::New(_)
    ));
    assert!(matches!(
        typer
            .store()
            .types
            .get(typer.typed_ast().get(constructor_call.args[0]).ty),
        Type::Constant(dotty_core::Constant::Int(42))
    ));
    assert!(matches!(
        typer
            .store()
            .types
            .get(typer.typed_ast().get(constructor_call.args[1]).ty),
        Type::Constant(dotty_core::Constant::Boolean(true))
    ));
    assert_eq!(
        typer
            .store()
            .types
            .get(typer.typed_ast().get(constructor_call.function).ty),
        &Type::TermRef {
            prefix: typer.typed_ast().get(constructor_selection.qualifier).ty,
            target: TermRefTarget::Symbol(constructor),
        }
    );
    assert_eq!(typer.store().symbols.get(constructor).owner, Some(counter));
    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(receiver).ty),
        "TypeRef(Class:Counter)"
    );
    for ty in [
        typer.typed_ast().get(make).ty,
        typer.typed_ast().get(receiver).ty,
    ] {
        assert!(matches!(
            typer.store().types.get(ty),
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } if *symbol == counter
        ));
    }
    let direct_context = typer.expression_context_for(direct_method).unwrap();
    assert!(matches!(
        typer.type_expression(direct_rhs, direct_context),
        Err(TyperError::UnstableSelectionPrefix { .. })
    ));

    let run_context = typer.expression_context_for(run_method).unwrap();
    let run = typer.type_expression(run_rhs, run_context).unwrap();
    let local_binding = typer.typed_local_binding_at(SOURCE, local_tree).unwrap();
    assert_eq!(
        typer.local_symbol_at(SOURCE, local_tree),
        Some(local_binding.symbol())
    );
    assert_eq!(
        typer.store().symbols.get(local_binding.symbol()).owner,
        Some(run_method)
    );
    assert_eq!(
        typer.source_typed_index().get(SOURCE, local_tree),
        Some(local_binding.declaration())
    );
    let TreeKind::ValDef(local_declaration) =
        &typer.typed_ast().get(local_binding.declaration()).kind
    else {
        panic!("local binding must point at its typed ValDef")
    };
    let local_initializer = local_declaration.rhs.expect("local initializer");
    let TreeKind::Apply(local_constructor_call) = &typer.typed_ast().get(local_initializer).kind
    else {
        panic!("local initializer should be a constructor Apply")
    };
    let Type::TermRef {
        target: TermRefTarget::Symbol(actual_seed),
        ..
    } = typer
        .store()
        .types
        .get(typer.typed_ast().get(local_constructor_call.args[0]).ty)
    else {
        panic!("constructor parameter argument should keep its parameter identity")
    };
    assert_eq!(*actual_seed, seed);

    let (block_expr, local_qualifier) = {
        let TreeKind::Block(block) = &typer.typed_ast().get(run).kind else {
            panic!("run body should retain its local block")
        };
        let TreeKind::Apply(local_call) = &typer.typed_ast().get(block.expr).kind else {
            panic!("local receiver call should remain an Apply")
        };
        let TreeKind::Select(local_selection) = &typer.typed_ast().get(local_call.function).kind
        else {
            panic!("local receiver call should select its method")
        };
        assert!(matches!(
            typer.typed_ast().get(local_selection.qualifier).kind,
            TreeKind::Ident(_)
        ));
        (block.expr, local_selection.qualifier)
    };
    assert_eq!(
        term_ref_symbol(&typer, local_qualifier),
        Some(local_binding.symbol())
    );
    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(local_qualifier).ty),
        "TermRef(Local:Client.run.counter)"
    );
    let local_qualifier_type = typer.typed_ast().get(local_qualifier).ty;
    assert_eq!(
        typer.widen_expression_type(local_qualifier_type).unwrap(),
        typer.typed_ast().get(local_initializer).ty
    );
    assert_eq!(
        call_target(&typer, block_expr).map(|symbol| typer.store().symbols.get(symbol).owner),
        Some(Some(counter))
    );
}

#[test]
fn same_spelled_methods_keep_selected_owner_and_runtime_receiver_class() {
    let mut fixture = Fixture::new(
        "class IntReader { def read(): Int = 1 }; class BooleanReader { def read(): Boolean = true }; class UnitWriter { def write(): Unit = () }; class Client { def intValue(reader: IntReader): Int = reader.read(); def booleanValue(reader: BooleanReader): Boolean = reader.read(); def unitValue(writer: UnitWriter): Unit = writer.write() }",
    );
    let int_reader = fixture.symbol_named("IntReader", SymbolKind::Class);
    let boolean_reader = fixture.symbol_named("BooleanReader", SymbolKind::Class);
    let unit_writer = fixture.symbol_named("UnitWriter", SymbolKind::Class);
    let client = fixture.symbol_named("Client", SymbolKind::Class);
    let int_type = fixture.definitions.int;
    let boolean_type = fixture.definitions.boolean;
    let unit_type = fixture.definitions.unit;
    let find_method = |name| {
        fixture
            .methods_named(name)
            .into_iter()
            .find(|(_, symbol, _)| fixture.store.symbols.get(*symbol).owner == Some(client))
            .unwrap()
    };
    let int_method = find_method("intValue");
    let boolean_method = find_method("booleanValue");
    let unit_method = find_method("unitValue");
    let first_parameter = |method_tree| match &fixture.ast.get(method_tree).kind {
        TreeKind::DefDef(definition) => fixture
            .index
            .symbol_at(SOURCE, definition.value_param_clauses[0][0])
            .unwrap(),
        _ => unreachable!(),
    };
    let int_reader_parameter = first_parameter(int_method.0);
    let boolean_reader_parameter = first_parameter(boolean_method.0);
    let unit_writer_parameter = first_parameter(unit_method.0);
    let mut typer = fixture.typer();

    for ((_, method, rhs), receiver_class, expected_type, receiver_name, receiver_symbol) in [
        (
            boolean_method,
            boolean_reader,
            boolean_type,
            "BooleanReader",
            boolean_reader_parameter,
        ),
        (
            int_method,
            int_reader,
            int_type,
            "IntReader",
            int_reader_parameter,
        ),
        (
            unit_method,
            unit_writer,
            unit_type,
            "UnitWriter",
            unit_writer_parameter,
        ),
    ] {
        let context = typer.expression_context_for(method).unwrap();
        let typed = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
            panic!("receiver call should remain Apply")
        };
        let TreeKind::Select(selection) = &typer.typed_ast().get(application.function).kind else {
            panic!("selected method should retain its receiver")
        };
        let receiver = selection.qualifier;
        let receiver_type = typer.typed_ast().get(receiver).ty;
        assert_eq!(
            call_target(&typer, typed).map(|symbol| typer.store().symbols.get(symbol).owner),
            Some(Some(receiver_class))
        );
        let widened_receiver = typer.widen_expression_type(receiver_type).unwrap();
        assert_eq!(
            type_snapshot(&typer, widened_receiver),
            format!("TypeRef(Class:{receiver_name})")
        );
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(receiver).ty),
            Type::TermRef {
                target: TermRefTarget::Symbol(actual),
                ..
            } if *actual == receiver_symbol
        ));
        assert_eq!(typer.typed_ast().get(typed).ty, expected_type);
    }
}

#[test]
fn explicit_this_receiver_keeps_its_class_and_selected_method_identity() {
    let mut fixture =
        Fixture::new("class Counter { def get(): Int = 1; def current(): Int = this.get() }");
    let counter = fixture.symbol_named("Counter", SymbolKind::Class);
    let (_, get_method, _) = fixture
        .methods_named("get")
        .into_iter()
        .find(|(_, symbol, _)| fixture.store.symbols.get(*symbol).owner == Some(counter))
        .unwrap();
    let (_, current_method, rhs) = fixture
        .methods_named("current")
        .into_iter()
        .find(|(_, symbol, _)| fixture.store.symbols.get(*symbol).owner == Some(counter))
        .unwrap();
    let int_type = fixture.definitions.int;
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(current_method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();
    let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
        panic!("explicit-this call should remain an Apply")
    };
    let TreeKind::Select(selection) = &typer.typed_ast().get(application.function).kind else {
        panic!("explicit-this call should keep its selected receiver")
    };
    assert!(matches!(
        typer.typed_ast().get(selection.qualifier).kind,
        TreeKind::This(_)
    ));
    assert_eq!(call_target(&typer, typed), Some(get_method));
    assert_eq!(typer.store().symbols.get(get_method).owner, Some(counter));
    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(selection.qualifier).ty),
        "ThisType(Class:Counter)"
    );
    assert_eq!(typer.typed_ast().get(typed).ty, int_type);
}

#[test]
fn no_argument_constructor_application_selects_its_class_owned_constructor() {
    let mut fixture = Fixture::new(
        "class Empty; class Client { def fresh(): Empty = new Empty(); def bare(): Empty = new Empty }",
    );
    let empty = fixture.symbol_named("Empty", SymbolKind::Class);
    let client = fixture.symbol_named("Client", SymbolKind::Class);
    let constructors = constructors_of(&mut fixture, empty);
    assert_eq!(constructors.len(), 1);
    let constructor = constructors[0];
    let (_, method, rhs) = fixture
        .methods_named("fresh")
        .into_iter()
        .find(|(_, symbol, _)| fixture.store.symbols.get(*symbol).owner == Some(client))
        .unwrap();
    let (_, bare_method, bare_rhs) = fixture
        .methods_named("bare")
        .into_iter()
        .find(|(_, symbol, _)| fixture.store.symbols.get(*symbol).owner == Some(client))
        .unwrap();
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();
    let TreeKind::Apply(constructor_call) = &typer.typed_ast().get(typed).kind else {
        panic!("empty constructor call should be represented explicitly as Apply")
    };
    assert!(constructor_call.args.is_empty());
    assert_eq!(call_target(&typer, typed), Some(constructor));
    let TreeKind::Select(selection) = &typer.typed_ast().get(constructor_call.function).kind else {
        panic!("constructor application should retain its selected class qualifier")
    };
    assert!(matches!(
        typer.typed_ast().get(selection.qualifier).kind,
        TreeKind::New(_)
    ));
    assert_eq!(typer.store().symbols.get(constructor).owner, Some(empty));
    let constructor_type = typer.complete_symbol(constructor).unwrap();
    let Type::Method(signature) = typer.store().types.get(constructor_type) else {
        panic!("no-argument constructor should have a completed method type")
    };
    assert!(signature.params.is_empty());
    assert_eq!(
        type_snapshot(&typer, signature.result),
        "TypeRef(Class:Empty)"
    );

    let bare_context = typer.expression_context_for(bare_method).unwrap();
    let bare = typer.type_expression(bare_rhs, bare_context).unwrap();
    let TreeKind::Apply(bare_call) = &typer.typed_ast().get(bare).kind else {
        panic!("bare new should normalize to a zero-argument constructor Apply")
    };
    assert!(bare_call.args.is_empty());
    assert_eq!(call_target(&typer, bare), Some(constructor));
    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(bare).ty),
        "TypeRef(Class:Empty)"
    );
}

#[test]
fn wrong_constructor_argument_arity_and_type_fail_during_typing() {
    for (source, wrong_arity) in [
        (
            "class C(value: Int); class Use { def make(): C = new C() }",
            true,
        ),
        (
            "class C(value: Int); class Use { def make(): C = new C(true) }",
            false,
        ),
    ] {
        let mut fixture = Fixture::new(source);
        let (_, method, rhs) = fixture.method("make");
        let mut typer = fixture.typer();
        let context = typer.expression_context_for(method).unwrap();

        let error = typer.type_expression(rhs, context).unwrap_err();
        match (wrong_arity, error) {
            (
                true,
                TyperError::ApplicationArityMismatch {
                    expected: 1,
                    actual: 0,
                    ..
                },
            ) => {}
            (false, TyperError::ApplicationArgumentTypeMismatch { .. }) => {}
            (wrong_arity, error) => {
                panic!("unexpected Typer error for wrong_arity={wrong_arity}: {error:?}")
            }
        }
        assert!(typer.source_typed_index().get(SOURCE, rhs).is_none());
    }
}

#[test]
fn private_constructor_candidate_is_not_filtered_by_typer_access_checks() {
    let mut fixture = Fixture::new(
        "class Hidden private (value: Int); class Client { def make(): Hidden = new Hidden(1) }",
    );
    let hidden = fixture.symbol_named("Hidden", SymbolKind::Class);
    let client = fixture.symbol_named("Client", SymbolKind::Class);
    let constructor = constructors_of(&mut fixture, hidden)[0];
    let (_, method, rhs) = fixture
        .methods_named("make")
        .into_iter()
        .find(|(_, symbol, _)| fixture.store.symbols.get(*symbol).owner == Some(client))
        .unwrap();
    assert_eq!(
        fixture.store.symbols.get(constructor).visibility,
        dotty_core::Visibility::Private
    );

    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();

    assert_eq!(call_target(&typer, typed), Some(constructor));
}

#[test]
fn generic_constructor_inference_finalizes_the_new_instance_type() {
    let mut fixture = Fixture::new("class Box[A](value: A); class Use { def make = new Box(1) }");
    let box_class = fixture.symbol_named("Box", SymbolKind::Class);
    let (_, method, rhs) = fixture.method("make");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();
    let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
        panic!("generic constructor call should remain an Apply")
    };
    let TreeKind::Select(selection) = &typer.typed_ast().get(application.function).kind else {
        panic!("constructor callee should remain a Select")
    };
    let instance_type = typer.typed_ast().get(selection.qualifier).ty;
    let expected = "TypeRef(Class:Box)[TypeRef(Class:Int)]";

    assert_eq!(type_snapshot(&typer, instance_type), expected);
    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(typed).ty),
        expected
    );
    assert_eq!(
        call_target(&typer, typed).map(|symbol| typer.store().symbols.get(symbol).owner),
        Some(Some(box_class))
    );
}

#[test]
fn constructor_overload_selects_the_applicable_secondary_constructor() {
    let mut fixture = Fixture::new(
        "class C(value: Int) { def this(flag: Boolean) = this(1) }; class Use { def make: C = new C(true) }",
    );
    let class = fixture.symbol_named("C", SymbolKind::Class);
    let constructors = constructors_of(&mut fixture, class);
    assert_eq!(constructors.len(), 2);
    let secondary = constructors[1];
    let (_, method, rhs) = fixture.method("make");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();

    assert_eq!(call_target(&typer, typed), Some(secondary));
    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(typed).ty),
        "TypeRef(Class:C)"
    );
}

#[test]
fn equally_applicable_constructor_candidates_remain_ambiguous() {
    let mut fixture = Fixture::new(
        "class C(value: Int) { def this(value: Int) = this(1) }; class Use { def make: C = new C(1) }",
    );
    let class = fixture.symbol_named("C", SymbolKind::Class);
    let declared_constructors = constructors_of(&mut fixture, class);
    assert_eq!(declared_constructors.len(), 2);
    let (_, method, rhs) = fixture.method("make");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let checkpoint = typer.store().checkpoint();

    let error = typer.type_expression(rhs, context).unwrap_err();

    let candidates = match &error {
        TyperError::AmbiguousConstructorApplication { candidates, .. } => candidates,
        other => panic!("expected constructor ambiguity, got {other:?}"),
    };
    assert_eq!(candidates.len(), declared_constructors.len());
    assert_eq!(
        candidates.iter().copied().collect::<HashSet<_>>(),
        declared_constructors.into_iter().collect::<HashSet<_>>()
    );
    assert_eq!(
        error_snapshot(&typer, &error),
        "AmbiguousConstructorApplication(C;Constructor:C.<init>=Missing|Constructor:C.<init>=Missing)"
    );
    assert_eq!(typer.store().checkpoint(), checkpoint);
    assert!(typer.source_typed_index().get(SOURCE, rhs).is_none());
}

#[test]
fn inferred_local_value_widens_symbol_info_but_keeps_rhs_constant_type() {
    let mut fixture = Fixture::new("class C { def use = { val value = 1; value } }");
    let (_, method, rhs) = fixture.method("use");
    let local_tree = fixture
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            (fixture
                .store
                .names
                .resolve(definition.name.as_name().text())
                == "value"
                && fixture.index.symbol_at(SOURCE, tree).is_none())
            .then_some(tree)
        })
        .unwrap();
    let source_rhs = match &fixture.ast.get(local_tree).kind {
        TreeKind::ValDef(definition) => definition.rhs.unwrap(),
        _ => unreachable!(),
    };
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    typer.type_expression(rhs, context).unwrap();
    let local = typer.local_symbol_at(SOURCE, local_tree).unwrap();
    let typed_rhs = typer.source_typed_index().get(SOURCE, source_rhs).unwrap();

    assert_eq!(
        symbol_info_snapshot(&typer, local),
        "Complete(TypeRef(Class:Int))"
    );
    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(typed_rhs).ty),
        "Constant(Int(1))"
    );
    let typed_block = typer.source_typed_index().get(SOURCE, rhs).unwrap();
    assert!(tree_snapshot(&typer, typed_block).starts_with("Block([ValDef(value,"));
}

#[test]
fn inferred_local_method_keeps_a_poly_method_signature() {
    let mut fixture =
        Fixture::new("object C { def outer: Int = { def local[A](value: A) = value; local(1) } }");
    let (_, outer, rhs) = fixture.method("outer");
    let local_tree = fixture
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
                == "local")
                .then_some(tree)
        })
        .unwrap();
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(outer).unwrap();
    typer.type_expression(rhs, context).unwrap();
    let local = typer.local_method_symbol_at(SOURCE, local_tree).unwrap();
    let SymbolInfo::Complete(signature) = *typer.store().symbols.info(local) else {
        panic!("inferred local method should have a completed signature")
    };
    let Type::Poly(poly) = typer.store().types.get(signature) else {
        panic!("generic local method should retain its Poly binder")
    };
    let Type::Method(method) = typer.store().types.get(poly.result) else {
        panic!("Poly result should retain the method clause")
    };

    assert_eq!(
        type_snapshot(&typer, signature),
        "Poly[A](Bounds(TypeRef(Class:Nothing),TypeRef(Class:Any)))->Method<Plain>(value:A)->A"
    );
    assert_eq!(poly.params.len(), 1);
    assert!(
        matches!(typer.store().types.get(method.params[0].ty), Type::ParamRef { binder, index: 0 } if *binder == signature)
    );
    assert!(
        matches!(typer.store().types.get(method.result), Type::ParamRef { binder, index: 0 } if *binder == signature)
    );
    let typed_block = typer.source_typed_index().get(SOURCE, rhs).unwrap();
    assert!(tree_snapshot(&typer, typed_block).contains("DefDef(local,"));
}

#[test]
fn if_branch_join_preserves_supertype_and_union_shapes() {
    let mut common_fixture = Fixture::new(
        "class Parent; class Child extends Parent; def common(flag: Boolean, child: Child, parent: Parent) = if flag then child else parent",
    );
    let parent = common_fixture.symbol_named("Parent", SymbolKind::Class);
    let child = common_fixture.symbol_named("Child", SymbolKind::Class);
    let (_, common_method, common_rhs) = common_fixture.method("common");
    let mut common_typer = common_fixture.typer();
    common_typer.complete_symbol(parent).unwrap();
    common_typer.complete_symbol(child).unwrap();
    let common_context = common_typer.expression_context_for(common_method).unwrap();
    let common = common_typer
        .type_expression(common_rhs, common_context)
        .unwrap();
    assert_eq!(
        type_snapshot(&common_typer, common_typer.typed_ast().get(common).ty),
        "TypeRef(Class:Parent)"
    );

    let mut union_fixture = Fixture::new(
        "trait Parent; class Left extends Parent; class Right extends Parent; def union(flag: Boolean, left: Left, right: Right) = if flag then left else right",
    );
    let parent = union_fixture.symbol_named("Parent", SymbolKind::Trait);
    let left = union_fixture.symbol_named("Left", SymbolKind::Class);
    let right = union_fixture.symbol_named("Right", SymbolKind::Class);
    let (_, union_method, union_rhs) = union_fixture.method("union");
    let mut union_typer = union_fixture.typer();
    for symbol in [parent, left, right] {
        union_typer.complete_symbol(symbol).unwrap();
    }
    let union_context = union_typer.expression_context_for(union_method).unwrap();
    let union = union_typer
        .type_expression(union_rhs, union_context)
        .unwrap();
    assert_eq!(
        type_snapshot(&union_typer, union_typer.typed_ast().get(union).ty),
        "Or(TypeRef(Class:Left),TypeRef(Class:Right))"
    );
}

#[test]
fn assignment_preserves_unit_result_and_exact_local_target() {
    let mut fixture = Fixture::new("class C { def use: Unit = { var value = 1; value = 2 } }");
    let (_, method, rhs) = fixture.method("use");
    let source_assignment = match &fixture.ast.get(rhs).kind {
        TreeKind::Block(block) => block.expr,
        _ => panic!("source method body should remain a block"),
    };
    let (source_lhs, source_rhs) = match &fixture.ast.get(source_assignment).kind {
        TreeKind::Assign(assignment) => (assignment.lhs, assignment.rhs),
        _ => panic!("source block result should be an assignment"),
    };
    let local_tree = fixture
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            (fixture
                .store
                .names
                .resolve(definition.name.as_name().text())
                == "value"
                && fixture.index.symbol_at(SOURCE, tree).is_none())
            .then_some(tree)
        })
        .unwrap();
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();
    let local = typer.local_symbol_at(SOURCE, local_tree).unwrap();
    let TreeKind::Block(block) = &typer.typed_ast().get(typed).kind else {
        panic!("method body should preserve its block")
    };
    let TreeKind::Assign(assignment) = &typer.typed_ast().get(block.expr).kind else {
        panic!("block result should remain an assignment")
    };

    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(block.expr).ty),
        "TypeRef(Class:Unit)"
    );
    assert_eq!(term_ref_symbol(&typer, assignment.lhs), Some(local));
    assert_eq!(typer.source_typed_index().get(SOURCE, rhs), Some(typed));
    assert_eq!(
        typer.source_typed_index().get(SOURCE, source_lhs),
        Some(assignment.lhs)
    );
    assert_eq!(
        typer.source_typed_index().get(SOURCE, source_rhs),
        Some(assignment.rhs)
    );
}

#[test]
fn while_and_return_preserve_unit_nothing_and_their_children() {
    let mut fixture = Fixture::new(
        "class C { def loop(flag: Boolean): Unit = while flag do (); def leave(value: Int): Int = return value }",
    );
    let (loop_tree, loop_method, loop_rhs) = fixture.method("loop");
    let (leave_tree, leave_method, leave_rhs) = fixture.method("leave");
    let loop_ast = match &fixture.ast.get(loop_tree).kind {
        TreeKind::DefDef(_) => loop_rhs,
        _ => unreachable!(),
    };
    let (source_condition, source_body) = match &fixture.ast.get(loop_ast).kind {
        TreeKind::While(while_tree) => (while_tree.cond, while_tree.body),
        _ => panic!("source expression should remain a While"),
    };
    let leave_param_tree = match &fixture.ast.get(leave_tree).kind {
        TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
        _ => unreachable!(),
    };
    let leave_param = fixture.index.symbol_at(SOURCE, leave_param_tree).unwrap();
    let source_return_value = match &fixture.ast.get(leave_rhs).kind {
        TreeKind::Return(return_tree) => return_tree.expr.unwrap(),
        _ => panic!("source expression should remain a Return"),
    };
    let nothing_class = fixture.definitions.nothing_class;
    let mut typer = fixture.typer();
    let loop_context = typer.expression_context_for(loop_method).unwrap();
    let typed_loop = typer.type_expression(loop_ast, loop_context).unwrap();
    let leave_context = typer.expression_context_for(leave_method).unwrap();
    let typed_return = typer.type_expression(leave_rhs, leave_context).unwrap();
    let TreeKind::While(while_tree) = &typer.typed_ast().get(typed_loop).kind else {
        panic!("while body should remain a While")
    };
    let TreeKind::Return(return_tree) = &typer.typed_ast().get(typed_return).kind else {
        panic!("return expression should remain a Return")
    };
    let returned = return_tree.expr.unwrap();

    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(typed_loop).ty),
        "TypeRef(Class:Unit)"
    );
    assert!(
        matches!(typer.store().types.get(typer.typed_ast().get(typed_return).ty), Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == nothing_class)
    );
    assert_eq!(term_ref_symbol(&typer, returned), Some(leave_param));
    assert_eq!(
        typer.source_typed_index().get(SOURCE, loop_rhs),
        Some(typed_loop)
    );
    assert_eq!(
        typer.source_typed_index().get(SOURCE, leave_rhs),
        Some(typed_return)
    );
    assert_eq!(
        typer.source_typed_index().get(SOURCE, source_condition),
        Some(while_tree.cond)
    );
    assert_eq!(
        typer.source_typed_index().get(SOURCE, source_body),
        Some(while_tree.body)
    );
    assert_eq!(
        typer.source_typed_index().get(SOURCE, source_return_value),
        Some(returned)
    );
    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(while_tree.body).ty),
        "Constant(Unit)"
    );
}

#[test]
fn failed_expression_typing_rolls_back_publicly_observable_state() {
    let mut fixture = Fixture::new("class C { def use: Int = { val local = 1; missing } }");
    let (_, method, rhs) = fixture.method("use");
    let local_tree = fixture
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            (fixture
                .store
                .names
                .resolve(definition.name.as_name().text())
                == "local"
                && fixture.index.symbol_at(SOURCE, tree).is_none())
            .then_some(tree)
        })
        .unwrap();
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let checkpoint = typer.store().checkpoint();
    let typed_count = typer.typed_ast().iter().count();
    let index_count = typer.source_typed_index().len();

    let error = typer.type_expression(rhs, context).unwrap_err();

    assert_eq!(error_snapshot(&typer, &error), "TermNameNotFound(missing)");
    assert_eq!(typer.store().checkpoint(), checkpoint);
    assert_eq!(typer.typed_ast().iter().count(), typed_count);
    assert_eq!(typer.source_typed_index().len(), index_count);
    assert_eq!(typer.local_symbol_at(SOURCE, local_tree), None);
}

#[test]
fn losing_generic_overload_probe_does_not_change_candidate_state() {
    let mut fixture = Fixture::new(
        "class C { def pick[A](value: A, extra: Any): Int = 1; def pick(value: Int, extra: Int): Int = 2; def use: Int = pick(1, 1) }",
    );
    let candidates = fixture.methods_named("pick");
    assert_eq!(candidates.len(), 2);
    let generic = candidates
        .iter()
        .find(|(tree, _, _)| matches!(&fixture.ast.get(*tree).kind, TreeKind::DefDef(def) if !def.type_params.is_empty()))
        .unwrap()
        .1;
    let monomorphic = candidates
        .iter()
        .find(|(tree, _, _)| matches!(&fixture.ast.get(*tree).kind, TreeKind::DefDef(def) if def.type_params.is_empty()))
        .unwrap()
        .1;
    let (_, use_method, rhs) = fixture.method("use");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(use_method).unwrap();
    let generic_signature = typer.complete_symbol(generic).unwrap();
    let generic_snapshot = type_snapshot(&typer, generic_signature);
    let candidate_info = *typer.store().symbols.info(generic);

    let typed = typer.type_expression(rhs, context).unwrap();

    assert_eq!(call_target(&typer, typed), Some(monomorphic));
    assert_eq!(*typer.store().symbols.info(generic), candidate_info);
    let generic_after = typer.complete_symbol(generic).unwrap();
    assert_eq!(type_snapshot(&typer, generic_after), generic_snapshot,);
    assert_eq!(typer.source_typed_index().get(SOURCE, rhs), Some(typed));
}

#[test]
fn self_recursive_inferred_local_result_fails_deterministically() {
    let mut fixture = Fixture::new("class C { def outer: Int = { def loop = loop; loop } }");
    let (_, outer, rhs) = fixture.method("outer");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(outer).unwrap();
    let checkpoint = typer.store().checkpoint();

    let first = typer.type_expression(rhs, context).unwrap_err();
    let first_symbol = match first {
        TyperError::RecursiveInferredMethodResult { symbol } => symbol,
        other => panic!("expected recursive-result error, got {other:?}"),
    };
    assert_eq!(typer.store().checkpoint(), checkpoint);
    assert_eq!(typer.typed_ast().iter().count(), 0);
    let second = typer.type_expression(rhs, context).unwrap_err();
    let second_symbol = match second {
        TyperError::RecursiveInferredMethodResult { symbol } => symbol,
        other => panic!("expected recursive-result error, got {other:?}"),
    };
    assert_eq!(first_symbol, second_symbol);
}

#[test]
fn mutual_inferred_local_result_cycle_fails_deterministically() {
    let mut fixture = Fixture::new(
        "class C { def outer: Int = { def first = second; def second = first; first } }",
    );
    let (_, outer, rhs) = fixture.method("outer");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(outer).unwrap();

    let first_error = typer.type_expression(rhs, context).unwrap_err();
    let first_symbol = match first_error {
        TyperError::RecursiveInferredMethodResult { symbol } => symbol,
        other => panic!("expected recursive-result error, got {other:?}"),
    };
    let second_error = typer.type_expression(rhs, context).unwrap_err();
    let second_symbol = match second_error {
        TyperError::RecursiveInferredMethodResult { symbol } => symbol,
        other => panic!("expected recursive-result error, got {other:?}"),
    };

    assert_eq!(first_symbol, second_symbol);
    assert_eq!(typer.typed_ast().iter().count(), 0);
    assert!(typer.source_typed_index().is_empty());
}

#[test]
fn repeated_complete_symbol_returns_the_same_signature_without_new_state() {
    let mut fixture = Fixture::new("class C { def identity[A](value: A): A = value }");
    let (_, identity, _) = fixture.method("identity");
    let mut typer = fixture.typer();

    let first = typer.complete_symbol(identity).unwrap();
    let checkpoint = typer.store().checkpoint();
    let second = typer.complete_symbol(identity).unwrap();

    assert_eq!(first, second);
    assert_eq!(typer.store().checkpoint(), checkpoint);
    assert_eq!(
        symbol_info_snapshot(&typer, identity),
        "Complete(Poly[A](Bounds(TypeRef(Class:Nothing),TypeRef(Class:Any)))->Method<Plain>(value:A)->A)"
    );
}

#[test]
fn repeated_type_expression_returns_the_same_typed_tree_identity() {
    let mut fixture = Fixture::new("class C { def use: Int = 42 }");
    let (_, method, rhs) = fixture.method("use");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();

    let first = typer.type_expression(rhs, context).unwrap();
    let checkpoint = typer.store().checkpoint();
    let typed_count = typer.typed_ast().iter().count();
    let second = typer.type_expression(rhs, context).unwrap();

    assert_eq!(first, second);
    assert_eq!(typer.source_typed_index().get(SOURCE, rhs), Some(first));
    assert_eq!(typer.store().checkpoint(), checkpoint);
    assert_eq!(typer.typed_ast().iter().count(), typed_count);
}

fn tree_snapshot(typer: &SourceTyper<'_>, tree: TreeId<Typed>) -> String {
    let node = typer.typed_ast().get(tree);
    let ty = type_snapshot(typer, node.ty);
    let shape = match &node.kind {
        TreeKind::Ident(ident) => format!("Ident({})", name(typer, ident.name.text())),
        TreeKind::This(this) => format!(
            "This({})",
            this.qual
                .map(|qual| name(typer, qual.text()))
                .unwrap_or_else(|| "_".to_owned())
        ),
        TreeKind::Literal(literal) => format!("Literal({:?})", literal.value),
        TreeKind::Select(select) => {
            let reference = match typer.store().types.get(node.ty) {
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } => symbol_label(typer, *symbol),
                _ => "unresolved".to_owned(),
            };
            format!(
                "Select({},{},{},{})",
                name(typer, select.name.text()),
                reference,
                ty,
                tree_snapshot(typer, select.qualifier)
            )
        }
        TreeKind::Apply(application) => format!(
            "Apply({:?},{},{})",
            application.kind,
            tree_snapshot(typer, application.function),
            application
                .args
                .iter()
                .map(|arg| tree_snapshot(typer, *arg))
                .collect::<Vec<_>>()
                .join(";")
        ),
        TreeKind::TypeApply(application) => format!(
            "TypeApply({},{},{})",
            tree_snapshot(typer, application.function),
            application
                .args
                .iter()
                .map(|arg| tree_snapshot(typer, *arg))
                .collect::<Vec<_>>()
                .join(";"),
            ty
        ),
        TreeKind::New(new) => format!("New({},{ty})", tree_snapshot(typer, new.tpt)),
        TreeKind::Typed(typed) => format!(
            "Typed({},{},{ty})",
            tree_snapshot(typer, typed.expr),
            tree_snapshot(typer, typed.tpt)
        ),
        TreeKind::Assign(assign) => format!(
            "Assign({},{},{ty})",
            tree_snapshot(typer, assign.lhs),
            tree_snapshot(typer, assign.rhs)
        ),
        TreeKind::Block(block) => format!(
            "Block([{}],{},{ty})",
            block
                .stats
                .iter()
                .map(|stat| tree_snapshot(typer, *stat))
                .collect::<Vec<_>>()
                .join(";"),
            tree_snapshot(typer, block.expr)
        ),
        TreeKind::If(conditional) => format!(
            "If({},{},{},{ty})",
            tree_snapshot(typer, conditional.cond),
            tree_snapshot(typer, conditional.then_branch),
            tree_snapshot(typer, conditional.else_branch)
        ),
        TreeKind::Return(return_tree) => format!(
            "Return({},{ty})",
            return_tree
                .expr
                .map(|expr| tree_snapshot(typer, expr))
                .unwrap_or_else(|| "()".to_owned())
        ),
        TreeKind::While(while_tree) => format!(
            "While({},{},{ty})",
            tree_snapshot(typer, while_tree.cond),
            tree_snapshot(typer, while_tree.body)
        ),
        TreeKind::ValDef(definition) => format!(
            "ValDef({},tpt={},rhs={})",
            name(typer, definition.name.as_name().text()),
            tree_snapshot(typer, definition.tpt),
            definition
                .rhs
                .map(|rhs| tree_snapshot(typer, rhs))
                .unwrap_or_else(|| "_".to_owned())
        ),
        TreeKind::DefDef(definition) => format!(
            "DefDef({},type_params=[{}],clauses=[{}],result={},rhs={})",
            name(typer, definition.name.as_name().text()),
            definition
                .type_params
                .iter()
                .map(|parameter| tree_snapshot(typer, *parameter))
                .collect::<Vec<_>>()
                .join(";"),
            definition
                .value_param_clauses
                .iter()
                .map(|clause| clause
                    .iter()
                    .map(|parameter| tree_snapshot(typer, *parameter))
                    .collect::<Vec<_>>()
                    .join(","))
                .collect::<Vec<_>>()
                .join(";"),
            tree_snapshot(typer, definition.tpt),
            definition
                .rhs
                .map(|rhs| tree_snapshot(typer, rhs))
                .unwrap_or_else(|| "_".to_owned())
        ),
        TreeKind::TypeDef(definition) => format!(
            "TypeDef({},rhs={})",
            name(typer, definition.name.as_name().text()),
            tree_snapshot(typer, definition.rhs)
        ),
        TreeKind::Template(template) => format!(
            "Template(parents=[{}],body=[{}])",
            template
                .parents
                .iter()
                .map(|parent| tree_snapshot(typer, *parent))
                .collect::<Vec<_>>()
                .join(";"),
            template
                .body
                .iter()
                .map(|member| tree_snapshot(typer, *member))
                .collect::<Vec<_>>()
                .join(";")
        ),
        _ => format!("Other({ty})"),
    };
    if matches!(node.kind, TreeKind::Select(_)) {
        shape
    } else {
        format!("{shape}:{ty}")
    }
}

fn type_snapshot(typer: &SourceTyper<'_>, ty: TypeId) -> String {
    type_snapshot_inner(typer, ty, &mut Vec::new(), &mut HashSet::new(), 0)
}

fn type_snapshot_inner(
    typer: &SourceTyper<'_>,
    ty: TypeId,
    binders: &mut Vec<(TypeId, Vec<String>)>,
    active: &mut HashSet<TypeId>,
    depth: usize,
) -> String {
    if depth > 64 {
        return "<depth-limit>".to_owned();
    }
    if !active.insert(ty) {
        return "<recursive>".to_owned();
    }
    let child = |typer: &SourceTyper<'_>, id, binders: &mut Vec<_>, active: &mut HashSet<_>| {
        type_snapshot_inner(typer, id, binders, active, depth + 1)
    };
    let result = match typer.store().types.get(ty) {
        Type::NoType => "NoType".to_owned(),
        Type::NoPrefix => "NoPrefix".to_owned(),
        Type::Error(_) => "Error".to_owned(),
        Type::TermRef { target, .. } => match target {
            TermRefTarget::Symbol(symbol) => format!("TermRef({})", symbol_label(typer, *symbol)),
            TermRefTarget::Name(name_id) => {
                format!("TermRef({})", name(typer, name_id.as_name().text()))
            }
        },
        Type::TypeRef { target, .. } => match target {
            TypeRefTarget::Symbol(symbol) => format!("TypeRef({})", symbol_label(typer, *symbol)),
            TypeRefTarget::Name(name_id) => {
                format!("TypeRef({})", name(typer, name_id.as_name().text()))
            }
        },
        Type::ThisType { class } => format!("ThisType({})", symbol_label(typer, *class)),
        Type::SuperType {
            this_type,
            super_type,
        } => format!(
            "Super({},{})",
            child(typer, *this_type, binders, active),
            child(typer, *super_type, binders, active)
        ),
        Type::Constant(value) => format!("Constant({value:?})"),
        Type::Applied { tycon, args } => format!(
            "{}[{}]",
            child(typer, *tycon, binders, active),
            args.iter()
                .map(|arg| child(typer, *arg, binders, active))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Type::Bounds { low, high } => format!(
            "Bounds({},{})",
            child(typer, *low, binders, active),
            child(typer, *high, binders, active)
        ),
        Type::AliasingBounds { alias } => {
            format!("Alias({})", child(typer, *alias, binders, active))
        }
        Type::ByName { result } => format!("ByName({})", child(typer, *result, binders, active)),
        Type::Flexible { underlying } => {
            format!("Flexible({})", child(typer, *underlying, binders, active))
        }
        Type::And { left, right } => format!(
            "And({},{})",
            child(typer, *left, binders, active),
            child(typer, *right, binders, active)
        ),
        Type::Or { left, right } => format!(
            "Or({},{})",
            child(typer, *left, binders, active),
            child(typer, *right, binders, active)
        ),
        Type::Refined {
            parent,
            name: member,
            info,
        } => format!(
            "Refined({};{}:{})",
            child(typer, *parent, binders, active),
            name(typer, member.text()),
            child(typer, *info, binders, active)
        ),
        Type::Recursive { parent } => {
            format!("Recursive({})", child(typer, *parent, binders, active))
        }
        Type::RecThis { binder } => binders
            .iter()
            .rev()
            .find(|(id, _)| id == binder)
            .map(|(_, names)| format!("RecThis({})", names.first().cloned().unwrap_or_default()))
            .unwrap_or_else(|| "RecThis".to_owned()),
        Type::Method(method) => format!(
            "Method<{:?}>({})->{}",
            method.kind,
            method
                .params
                .iter()
                .map(|param| format!(
                    "{}:{}",
                    name(typer, param.name.as_name().text()),
                    child(typer, param.ty, binders, active)
                ))
                .collect::<Vec<_>>()
                .join(","),
            child(typer, method.result, binders, active)
        ),
        Type::Poly(poly) => {
            let names = poly
                .params
                .iter()
                .map(|param| name(typer, param.name.as_name().text()))
                .collect::<Vec<_>>();
            binders.push((ty, names.clone()));
            let bounds = poly
                .params
                .iter()
                .map(|param| child(typer, param.bounds, binders, active))
                .collect::<Vec<_>>()
                .join(",");
            let result = child(typer, poly.result, binders, active);
            binders.pop();
            format!("Poly[{}]({bounds})->{result}", names.join(","))
        }
        Type::TypeLambda(lambda) => {
            let names = lambda
                .params
                .iter()
                .map(|param| name(typer, param.name.as_name().text()))
                .collect::<Vec<_>>();
            binders.push((ty, names.clone()));
            let result = child(typer, lambda.result, binders, active);
            binders.pop();
            format!("TypeLambda[{}]=>{result}", names.join(","))
        }
        Type::ParamRef { binder, index } => binders
            .iter()
            .rev()
            .find(|(id, _)| id == binder)
            .and_then(|(_, names)| names.get(*index as usize))
            .cloned()
            .unwrap_or_else(|| format!("Param[{index}]")),
        Type::Match(matched) => format!(
            "Match({};{};[{}])",
            child(typer, matched.bound, binders, active),
            child(typer, matched.scrutinee, binders, active),
            matched
                .cases
                .iter()
                .map(|case| child(typer, *case, binders, active))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Type::MatchCase { pattern, result } => format!(
            "MatchCase({},{})",
            child(typer, *pattern, binders, active),
            child(typer, *result, binders, active)
        ),
        Type::Annotated { underlying, .. } => {
            format!("Annotated({})", child(typer, *underlying, binders, active))
        }
        Type::Wildcard { bounds } => {
            format!("Wildcard({})", child(typer, *bounds, binders, active))
        }
        Type::JavaArray { element } => {
            format!("Array[{}]", child(typer, *element, binders, active))
        }
        Type::Repeated { element } => {
            format!("Repeated({})", child(typer, *element, binders, active))
        }
        Type::ClassInfo(info) => format!(
            "ClassInfo({},[{}])",
            symbol_label(typer, info.class),
            info.parents
                .iter()
                .map(|parent| child(typer, *parent, binders, active))
                .collect::<Vec<_>>()
                .join(",")
        ),
    };
    active.remove(&ty);
    result
}

fn symbol_info_snapshot(typer: &SourceTyper<'_>, symbol: SymbolId) -> String {
    match typer.store().symbols.info(symbol) {
        SymbolInfo::Missing => "Missing".to_owned(),
        SymbolInfo::Deferred(_) => "Deferred".to_owned(),
        SymbolInfo::Error => "Error".to_owned(),
        SymbolInfo::Complete(ty) => {
            format!("Complete({})", type_snapshot(typer, *ty))
        }
    }
}

fn symbol_label(typer: &SourceTyper<'_>, symbol: SymbolId) -> String {
    let value = typer.store().symbols.get(symbol);
    format!(
        "{:?}:{}",
        value.kind,
        symbol_path(typer, symbol, &mut HashSet::new())
    )
}

fn symbol_path(typer: &SourceTyper<'_>, symbol: SymbolId, seen: &mut HashSet<SymbolId>) -> String {
    if !seen.insert(symbol) {
        return "<owner-cycle>".to_owned();
    }
    let value = typer.store().symbols.get(symbol);
    let current = name(typer, value.name.text());
    if value.kind == SymbolKind::Package && current.is_empty() {
        return String::new();
    }
    match value.owner {
        Some(owner) => {
            let parent = symbol_path(typer, owner, seen);
            if parent.is_empty() {
                current
            } else {
                format!("{parent}.{current}")
            }
        }
        None => current,
    }
}

fn name(typer: &SourceTyper<'_>, name: dotty_core::NameId) -> String {
    typer.store().names.resolve(name).to_owned()
}

fn error_variant(error: &TyperError) -> String {
    let debug = format!("{error:?}");
    let end = debug.find(['{', '(']).unwrap_or(debug.len());
    debug[..end].trim().to_owned()
}

fn error_snapshot(typer: &SourceTyper<'_>, error: &TyperError) -> String {
    match error {
        TyperError::ApplicationArityMismatch {
            expected, actual, ..
        } => format!(
            "{}(expected={expected},actual={actual})",
            error_variant(error)
        ),
        TyperError::TermNameNotFound { name: missing, .. } => {
            format!("TermNameNotFound({})", name(typer, missing.text()))
        }
        TyperError::AmbiguousConstructorApplication {
            class, candidates, ..
        } => format!(
            "AmbiguousConstructorApplication({};{})",
            symbol_path(typer, *class, &mut HashSet::new()),
            candidate_snapshot(typer, candidates)
        ),
        TyperError::AmbiguousOverloadApplication { candidates, .. } => format!(
            "AmbiguousOverloadApplication({})",
            candidate_snapshot(typer, candidates)
        ),
        _ => error_variant(error),
    }
}

fn candidate_snapshot(typer: &SourceTyper<'_>, candidates: &[SymbolId]) -> String {
    let mut candidates = candidates
        .iter()
        .map(|symbol| {
            format!(
                "{}={}",
                symbol_label(typer, *symbol),
                symbol_info_snapshot(typer, *symbol)
            )
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.join("|")
}

fn constructors_of(fixture: &mut Fixture, class: SymbolId) -> Vec<SymbolId> {
    let name = Name::new(fixture.store.names.intern("<init>"), Namespace::Term);
    fixture
        .index
        .scope_of(class)
        .map(|scope| fixture.store.scopes.get(scope).lookup_all(&name).to_vec())
        .unwrap_or_default()
}

fn term_ref_symbol(typer: &SourceTyper<'_>, tree: TreeId<Typed>) -> Option<SymbolId> {
    match typer.store().types.get(typer.typed_ast().get(tree).ty) {
        Type::TermRef { target, .. } => target.symbol(),
        _ => None,
    }
}

fn call_target(typer: &SourceTyper<'_>, tree: TreeId<Typed>) -> Option<SymbolId> {
    match &typer.typed_ast().get(tree).kind {
        TreeKind::Apply(application) => call_target(typer, application.function),
        TreeKind::TypeApply(application) => call_target(typer, application.function),
        _ => term_ref_symbol(typer, tree),
    }
}
