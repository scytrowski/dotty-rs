use super::*;

fn definition_tree(index: &SourceSemanticIndex, symbol: SymbolId) -> TreeId<Untyped> {
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(symbol) else {
        panic!("expected a canonical source method")
    };
    tree
}

fn reference(typer: &SourceTyper<'_>, tree: TreeId<Typed>) -> SymbolId {
    match typer.store().types.get(typer.typed_ast().get(tree).ty) {
        Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        } => *symbol,
        other => panic!("expected symbol reference, got {other:?}"),
    }
}

#[test]
fn top_level_calls_resolve_the_file_owner_and_preserve_contextual_precedence() {
    let text = "package example
        object Imported { given imported: Int = 2 }
        import Imported.given
        given own: Int = 1
        def need(using i: Int): Int = i
        def fromFile: Int = need
        def fromParameter(using local: Int): Int = need";
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(text);
    let own = val_symbol(&parsed, &store, &index, source, "own").0;
    let need = method_symbol(&parsed, &store, &index, source, "need");
    let wrapper = store.symbols.owner(need).unwrap();
    assert_eq!(store.symbols.owner(own), Some(wrapper));
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for name in ["fromFile", "fromParameter"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let expected = if name == "fromFile" {
            own
        } else {
            method_parameter_symbol(&parsed, &index, source, method, 0)
        };
        typer
            .type_method_definition(definition_tree(&index, method))
            .unwrap();
        let rhs = typer.source_typed_index().get(source, rhs).unwrap();
        let TreeKind::Apply(call) = &typer.typed_ast().get(rhs).kind else {
            panic!("expected contextual argument insertion")
        };
        assert_eq!(reference(&typer, call.function), need);
        let [argument] = call.args[..] else {
            panic!("expected one Int argument")
        };
        assert_eq!(reference(&typer, argument), expected);
    }
    let info = typer.complete_symbol(wrapper).unwrap();
    assert_eq!(typer.complete_symbol(wrapper).unwrap(), info);
    assert!(matches!(typer.store().types.get(info),
        Type::ClassInfo(class) if class.class == wrapper
        && Some(class.declarations) == index.scope_of(wrapper)
        && class.parents == [defs.object_type]));
}

#[test]
fn ambiguous_file_givens_roll_back_lazy_owner_completion() {
    let text = "given first: Int = 1
        given second: Int = 2
        def need(using i: Int): Int = i
        def ambiguous: Int = need";
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(text);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "ambiguous");
    let candidates =
        ["first", "second"].map(|name| val_symbol(&parsed, &store, &index, source, name).0);
    let wrapper = store.symbols.owner(method).unwrap();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let checkpoint = typer.store().checkpoint();
    for _ in 0..2 {
        let error = typer
            .type_method_definition(definition_tree(&index, method))
            .unwrap_err();
        assert!(
            matches!(&error, TyperError::AmbiguousContextualArgument { source: at, tree_index, expected, candidates: actual }
            if *at == source && *tree_index == rhs.index() && *expected == defs.int && actual == &candidates),
            "{error:?}"
        );
        assert_eq!(*typer.store().symbols.info(wrapper), SymbolInfo::Missing);
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert!(typer.source_typed_index().is_empty());
        assert_eq!(typer.typed_ast().iter().count(), 0);
    }
}

#[test]
fn omitted_argument_forwards_the_canonical_parameter() {
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(include_str!(
        "../../../tests/fixtures/contextual-arguments/Calls.scala"
    ));
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "forward");
    let target = method_symbol(&parsed, &store, &index, source, "need");
    let parameter = method_parameter_symbol(&parsed, &index, source, method, 0);
    let method = definition_tree(&index, method);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let root = typer.type_method_definition(method).unwrap();
    assert_eq!(typer.type_method_definition(method).unwrap(), root);
    let rhs = typer.source_typed_index().get(source, rhs).unwrap();
    let node = typer.typed_ast().get(rhs);
    let TreeKind::Apply(app) = &node.kind else {
        panic!("missing contextual application")
    };
    assert_eq!(node.ty, definitions.int);
    assert_eq!(app.kind, ApplyKind::Regular);
    assert_eq!(reference(&typer, app.function), target);
    assert_eq!(app.args.len(), 1);
    assert_eq!(reference(&typer, app.args[0]), parameter);
}

#[test]
fn omitted_argument_reports_missing_and_ambiguous_without_partial_state() {
    let text = "class C { def need(using n: Int): Int = n; def missing(n: Int): Int = need; def amb(using x: Int, y: Int): Int = need }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for name in ["missing", "amb"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let checkpoint = typer.store().checkpoint();
        for _ in 0..2 {
            let result = typer.type_method_definition(definition_tree(&index, method));
            match result {
                Err(TyperError::MissingContextualArgument {
                    expected,
                    tree_index,
                    ..
                }) if name == "missing" => {
                    assert_eq!((expected, tree_index), (definitions.int, rhs.index()));
                }
                Err(TyperError::AmbiguousContextualArgument {
                    expected,
                    tree_index,
                    candidates,
                    ..
                }) if name == "amb" => {
                    assert_eq!((expected, tree_index), (definitions.int, rhs.index()));
                    assert_eq!(
                        candidates,
                        (0..2)
                            .map(|i| method_parameter_symbol(&parsed, &index, source, method, i))
                            .collect::<Vec<_>>()
                    );
                }
                other => panic!("{name}: {other:?}"),
            }
            assert_eq!(typer.store().checkpoint(), checkpoint);
            assert!(typer.source_typed_index().is_empty());
            assert_eq!(typer.typed_ast().iter().count(), 0);
        }
    }
}

#[test]
fn nested_local_evidence_is_applied_in_clause_and_parameter_order() {
    let text = "class C { def f(x: Int)(using n: Int)(using b: Boolean): Int = n; def use(using outer: Int): Int = { implicit val b: Boolean = true; given local: Int = 7; f(1) } }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
    let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
        panic!()
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap();
    let mut call = typer.source_typed_index().get(source, block.expr).unwrap();
    for stat in &block.stats {
        let expected = typer.local_symbol_at(source, *stat).unwrap();
        let TreeKind::Apply(app) = &typer.typed_ast().get(call).kind else {
            panic!()
        };
        assert_eq!(app.args.len(), 1);
        assert_eq!(reference(&typer, app.args[0]), expected);
        call = app.function;
    }
    assert!(matches!(
        typer.typed_ast().get(call).kind,
        TreeKind::Apply(_)
    ));
}

#[test]
fn same_level_contextual_ranking_prefers_more_general_result_regardless_of_order() {
    for entries in [
        "given broad: Base = new Base; given narrow: Child = new Child",
        "given narrow: Child = new Child; given broad: Base = new Base",
    ] {
        let text = format!(
            "class Base; class Child extends Base; class C {{ {entries}; def need(using b: Base): Base = b; def call: Base = need }}"
        );
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(&text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "call");
        let (broad, _, _) = val_definition_and_rhs(&parsed, &store, &index, source, "broad");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        typer
            .type_method_definition(definition_tree(&index, method))
            .unwrap();
        let rhs = typer.source_typed_index().get(source, rhs).unwrap();
        let TreeKind::Apply(app) = &typer.typed_ast().get(rhs).kind else {
            panic!()
        };
        assert_eq!(reference(&typer, app.args[0]), broad);
        let Type::TermRef { prefix, .. } =
            typer.store.types.get(typer.typed_ast().get(app.args[0]).ty)
        else {
            panic!()
        };
        assert!(
            matches!(typer.store.types.get(*prefix), Type::ThisType { class } if Some(*class) == typer.store.symbols.get(method).owner)
        );
    }
}

#[test]
fn imported_values_retain_the_qualifier_and_selector_filter() {
    for (declaration, selector, selected) in [
        ("modern", "given", Some("modern")),
        ("modern", "given Int", Some("modern")),
        ("modern", "modern", Some("modern")),
        ("modern", "{modern as renamed}", Some("modern")),
        ("modern", "{modern as _, given}", None),
        ("modern", "*", None),
        ("`*`", "*", None),
        ("`*`", "`*`", Some("*")),
    ] {
        let text = format!(
            "object Instances {{ given {declaration}: Int = 7; given flag: Boolean = true }}; class C {{ def need(using n: Int): Int = n; def use: Int = {{ import Instances.{selector}; need }} }}"
        );
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(&text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
            panic!()
        };
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let result = typer.type_method_definition(definition_tree(&index, method));
        if let Some(name) = selected {
            result.unwrap_or_else(|e| panic!("{selector}: {e:?}"));
            let call = typer.typed_index.get(source, block.expr).unwrap();
            let TreeKind::Apply(app) = &typer.typed_ast().get(call).kind else {
                panic!()
            };
            let TreeKind::Select(select) = &typer.typed_ast().get(app.args[0]).kind else {
                panic!("missing imported receiver")
            };
            let symbol = reference(&typer, app.args[0]);
            let receiver = reference(&typer, select.qualifier);
            assert_eq!(
                typer
                    .store
                    .names
                    .resolve(typer.store.symbols.get(symbol).name.text()),
                name
            );
            assert_eq!(
                typer
                    .store
                    .names
                    .resolve(typer.store.symbols.get(receiver).name.text()),
                "Instances"
            );
        } else {
            assert!(
                matches!(result, Err(TyperError::MissingContextualArgument { .. })),
                "{selector}: {result:?}"
            );
        }
    }
}

#[test]
fn imports_from_distinct_instances_keep_distinct_evidence_paths() {
    let text = "class Provider { given value: Int = 1 }; class C { def need(using n: Int): Int = n; def ok(p: Provider): Int = { import p.given; need }; def latest(a: Provider, b: Provider): Int = { import a.given; import b.given; need } }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "ok");
    typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap();
    let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
        panic!()
    };
    let call = typer.typed_index.get(source, block.expr).unwrap();
    let TreeKind::Apply(app) = &typer.typed_ast().get(call).kind else {
        panic!()
    };
    let TreeKind::Select(selection) = &typer.typed_ast().get(app.args[0]).kind else {
        panic!()
    };
    assert_eq!(
        reference(&typer, selection.qualifier),
        method_parameter_symbol(&parsed, &index, source, method, 0)
    );
    let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "latest");
    typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap();
    let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
        panic!()
    };
    let call = typer.typed_index.get(source, block.expr).unwrap();
    let TreeKind::Apply(app) = &typer.typed_ast().get(call).kind else {
        panic!()
    };
    let TreeKind::Select(select) = &typer.typed_ast().get(app.args[0]).kind else {
        panic!()
    };
    assert_eq!(
        reference(&typer, select.qualifier),
        method_parameter_symbol(&parsed, &index, source, method, 1)
    );
}

#[test]
fn applied_type_and_argument_companions_contribute_evidence() {
    let text = "class Box[A]; class Item; object Item { given boxed: Box[Item] = new Box[Item] }; class C { def ask(using i: Box[Item]): Box[Item] = i; def call: Box[Item] = ask }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "call");
    let (boxed, _, _) = val_definition_and_rhs(&parsed, &store, &index, source, "boxed");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap();
    let rhs = typer.typed_index.get(source, rhs).unwrap();
    let TreeKind::Apply(app) = &typer.typed_ast().get(rhs).kind else {
        panic!()
    };
    assert_eq!(reference(&typer, app.args[0]), boxed);
}

#[test]
fn conditional_given_infers_its_binder_from_the_goal_and_solves_its_dependency() {
    let text = concat!(
        include_str!("../../../tests/fixtures/contextual-arguments/Evidence.scala"),
        "\nclass Client { def ask(using t: TC[Box[Int]]): TC[Box[Int]] = t; def result: TC[Box[Int]] = ask }"
    );
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "result");
    let producer = method_symbol(&parsed, &store, &index, source, "boxes");
    let (leaf, _, _) = val_definition_and_rhs(&parsed, &store, &index, source, "ints");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap();
    let rhs = typer.typed_index.get(source, rhs).unwrap();
    let TreeKind::Apply(ask) = &typer.typed_ast().get(rhs).kind else {
        panic!()
    };
    let TreeKind::Apply(boxes) = &typer.typed_ast().get(ask.args[0]).kind else {
        panic!()
    };
    let TreeKind::TypeApply(tapp) = &typer.typed_ast().get(boxes.function).kind else {
        panic!()
    };
    assert_eq!(reference(&typer, tapp.function), producer);
    assert_eq!(typer.typed_ast().get(tapp.args[0]).ty, definitions.int);
    assert_eq!(reference(&typer, boxes.args[0]), leaf);
}

#[test]
fn recursive_search_rejects_an_ambiguous_dependency_and_retries_after_failure() {
    let text = "class A; class B; object B { given fromA: A => B = new B }; class Use { def take(using b: B): B = b; def ambiguous(using a1: A, a2: A): B = take; def missing: B = take; def good(using a: A): B = take }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for name in ["missing", "ambiguous", "good"] {
        let method = method_symbol(&parsed, typer.store(), &index, source, name);
        let before = typer.store.checkpoint();
        let result = typer.type_method_definition(definition_tree(&index, method));
        match name {
            "missing" => assert!(
                matches!(result, Err(TyperError::MissingContextualArgument { .. })),
                "{result:?}"
            ),
            "ambiguous" => assert!(
                matches!(result, Err(TyperError::AmbiguousContextualArgument{ref candidates, ..}) if candidates.len() == 2),
                "{result:?}"
            ),
            _ => {
                result.unwrap();
                continue;
            }
        }
        assert_eq!(typer.store.checkpoint(), before);
        assert!(typer.typed_index.is_empty());
    }
}

#[test]
fn cyclic_dependency_fails_locally_and_does_not_hide_a_valid_alternative() {
    for (extra, succeeds) in [("", false), ("given direct: B = new B", true)] {
        let text = format!(
            "class B; object B {{ given loop: B => B = new B; {extra} }}; class C {{ def ask(using b: B): B = b; def call: B = ask }}"
        );
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(&text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "call");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let result = typer.type_method_definition(definition_tree(&index, method));
        if succeeds {
            result.unwrap();
            let call = typer.typed_index.get(source, rhs).unwrap();
            let TreeKind::Apply(call) = &typer.typed_ast().get(call).kind else {
                panic!()
            };
            let symbol = reference(&typer, call.args[0]);
            assert_eq!(
                typer
                    .store
                    .names
                    .resolve(typer.store.symbols.get(symbol).name.text()),
                "direct"
            );
        } else {
            assert!(
                matches!(result, Err(TyperError::DivergentContextualArgument { .. })),
                "{result:?}"
            );
        }
    }
}

#[test]
fn named_and_aggregate_context_bounds_supply_canonical_method_evidence() {
    let text = "class TC[A]; class Show[A]; class C { def witness[A: {TC as ev, Show}](using i: Int): TC[A] = ev; def call(using tc: TC[Int], sh: Show[Int], i: Int): TC[Int] = witness[Int] }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "witness");
    let (call, call_rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "call");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let root = typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap();
    let TreeKind::DefDef(def) = &typer.typed_ast().get(root).kind else {
        panic!()
    };
    assert_eq!(def.value_param_clauses.len(), 1);
    assert_eq!(def.value_param_clauses[0].len(), 3);
    let witness = reference(&typer, typer.typed_index.get(source, rhs).unwrap());
    assert_eq!(typer.store.symbols.owner(witness), Some(method));
    assert_eq!(
        typer
            .store
            .names
            .resolve(typer.store.symbols.get(witness).name.text()),
        "ev"
    );
    typer
        .type_method_definition(definition_tree(&index, call))
        .unwrap();
    let TreeKind::Apply(app) = &typer
        .typed_ast()
        .get(typer.typed_index.get(source, call_rhs).unwrap())
        .kind
    else {
        panic!()
    };
    for (i, arg) in app.args.iter().enumerate() {
        assert_eq!(
            reference(&typer, *arg),
            method_parameter_symbol(&parsed, &index, source, call, i)
        );
    }
}

#[test]
fn context_bound_placement_precedes_a_dependent_clause_or_follows_plain_clauses() {
    let text = "class TC[A]; class C { def before[A: TC as ev](same: ev.type): Int = 1; def after[A: TC](value: Int): Int = value }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for (name, kinds) in [
        ("before", [MethodKind::Contextual, MethodKind::Plain]),
        ("after", [MethodKind::Plain, MethodKind::Contextual]),
    ] {
        let method = method_symbol(&parsed, typer.store(), &index, source, name);
        let sig = typer.complete_symbol(method).unwrap();
        let Type::Poly(poly) = typer.store.types.get(sig) else {
            panic!()
        };
        let mut next = poly.result;
        for kind in kinds {
            let Type::Method(clause) = typer.store.types.get(next) else {
                panic!()
            };
            assert_eq!(clause.kind, kind);
            assert_eq!(clause.params.len(), 1);
            next = clause.result;
        }
        assert_eq!(next, definitions.int);
    }
}

#[test]
fn inherited_contextual_members_remain_on_the_lexical_or_imported_receiver() {
    let text = "trait Supplies { given supply: Int = 3 }; class Child extends Supplies { def ask(using n: Int): Int = n; def inside: Int = ask }; class Client { def read(c: Child): Int = { import c.given; c.ask } }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let (supply, _, _) = val_definition_and_rhs(&parsed, &store, &index, source, "supply");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for name in ["inside", "read"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        typer
            .type_method_definition(definition_tree(&index, method))
            .unwrap();
        let rhs = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(b) => b.expr,
            _ => rhs,
        };
        let TreeKind::Apply(call) = &typer
            .typed_ast()
            .get(typer.typed_index.get(source, rhs).unwrap())
            .kind
        else {
            panic!()
        };
        assert_eq!(reference(&typer, call.args[0]), supply);
        if name == "read" {
            let TreeKind::Select(s) = &typer.typed_ast().get(call.args[0]).kind else {
                panic!()
            };
            assert_eq!(
                reference(&typer, s.qualifier),
                method_parameter_symbol(&parsed, &index, source, method, 0)
            );
        }
    }
}

#[test]
fn inherited_given_priority_uses_declaring_classes_in_a_diamond() {
    let text = "class A; class B extends A;
        trait Base { given base: A = new A };
        trait Left extends Base { given left: B = new B };
        trait Right extends Base { given right: B = new B };
        class Child extends Left; class Both extends Left with Right;
        class Client {
          def need(using n: A): A = n;
          def pick(c: Child): A = { import c.given; need };
          def tie(c: Both): A = { import c.given; need }
        }";
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(text);
    let left = val_symbol(&parsed, &store, &index, source, "left").0;
    let right = val_symbol(&parsed, &store, &index, source, "right").0;
    let (pick, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "pick");
    let tie = method_symbol(&parsed, &store, &index, source, "tie");
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    typer
        .type_method_definition(definition_tree(&index, pick))
        .unwrap();
    let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
        panic!("import block")
    };
    let selected = typer.typed_index.get(source, block.expr).unwrap();
    let TreeKind::Apply(call) = &typer.typed_ast().get(selected).kind else {
        panic!("need")
    };
    assert_eq!(reference(&typer, call.args[0]), left);

    let err = typer
        .type_method_definition(definition_tree(&index, tie))
        .unwrap_err();
    let TyperError::AmbiguousContextualArgument { candidates, .. } = err else {
        panic!("{err:?}")
    };
    assert_eq!(candidates.len(), 2);
    assert!(candidates.contains(&left) && candidates.contains(&right));
}

#[test]
fn constructor_contexts_follow_normalized_scala_clause_order() {
    let text = "class Context(using n:Int); class Old(implicit n:Int); class Pair(x:Int)(using n:Int); class C { def implicitCtor(using n:Int):Context=new Context; def explicitCtor(using n:Int):Context=new Context(using n); def legacy(using n:Int):Old=new Old; def trailing(using n:Int):Pair=new Pair(1) }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for (name, expected) in [
        ("implicitCtor", [1, 0]),
        ("explicitCtor", [1, 0]),
        ("legacy", [0, 1]),
        ("trailing", [1, 1]),
    ] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        typer
            .type_method_definition(definition_tree(&index, method))
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let outer = typer.typed_index.get(source, rhs).unwrap();
        let TreeKind::Apply(end) = &typer.typed_ast().get(outer).kind else {
            panic!()
        };
        let TreeKind::Apply(start) = &typer.typed_ast().get(end.function).kind else {
            panic!()
        };
        assert_eq!([start.args.len(), end.args.len()], expected);
        let argument = if expected[1] == 0 {
            start.args[0]
        } else {
            end.args[0]
        };
        assert_eq!(
            reference(&typer, argument),
            method_parameter_symbol(&parsed, &index, source, method, 0)
        );
    }
}

#[test]
fn nested_import_evidence_keeps_the_whole_qualifier_at_local_and_member_use_sites() {
    let text = "object Instances { object Nested { given n: Int = 3 } }; class C { def need(using n: Int): Int = n; def local: Int = { import Instances.Nested.given; need }; import Instances.Nested.given; def member: Int = need }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for name in ["local", "member"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        typer
            .type_method_definition(definition_tree(&index, method))
            .unwrap();
        let rhs = match &parsed.ast.get(rhs).kind {
            TreeKind::Block(b) => b.expr,
            _ => rhs,
        };
        let typed = typer.typed_index.get(source, rhs).unwrap();
        let TreeKind::Apply(app) = &typer.typed_ast().get(typed).kind else {
            panic!()
        };
        let TreeKind::Select(evidence) = &typer.typed_ast().get(app.args[0]).kind else {
            panic!()
        };
        let TreeKind::Select(nested) = &typer.typed_ast().get(evidence.qualifier).kind else {
            panic!("lost selected receiver in {name}")
        };
        assert_eq!(typer.store.names.resolve(nested.name.text()), "Nested");
        let outer = reference(&typer, nested.qualifier);
        assert_eq!(
            typer
                .store
                .names
                .resolve(typer.store.symbols.get(outer).name.text()),
            "Instances"
        );
    }
}

#[test]
fn expected_result_disambiguates_contextual_overloads_before_evidence_search() {
    let text = "class C { def one(using n:Int):Int=n; def one(using b:Boolean):Boolean=b; def from(x:Int)(using n:Int):Int=n; def from(x:Int)(using b:Boolean):Boolean=b; def bare(using n:Int):Int=one; def selected(using b:Boolean):Boolean=this.one; def curried(using n:Int):Int=from(1); def missing(using n:Int):Boolean=from(1); def ambiguous(using n:Int):Any=from(1) }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for name in ["bare", "selected", "curried", "missing", "ambiguous"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let result = typer.type_method_definition(definition_tree(&index, method));
        match name {
            "missing" => assert!(
                matches!(result,Err(TyperError::MissingContextualArgument{expected,..}) if expected==definitions.boolean),
                "{result:?}"
            ),
            "ambiguous" => assert!(
                matches!(result, Err(TyperError::AmbiguousOverloadApplication { .. })),
                "{result:?}"
            ),
            _ => {
                result.unwrap_or_else(|error| panic!("{name}: {error:?}"));
                let typed = typer.typed_index.get(source, rhs).unwrap();
                let TreeKind::Apply(app) = &typer.typed_ast().get(typed).kind else {
                    panic!()
                };
                assert_eq!(
                    reference(&typer, app.args[0]),
                    method_parameter_symbol(&parsed, &index, source, method, 0)
                );
                assert_eq!(
                    typer.typed_ast().get(typed).ty,
                    if name == "selected" {
                        definitions.boolean
                    } else {
                        definitions.int
                    }
                );
            }
        }
    }
}

#[test]
fn context_lambda_owns_the_evidence_used_by_its_body() {
    let text = "package scala; class C { def need(using n: Int): Int = n; def make: Int ?=> Int = (n: Int) ?=> need; def inferred: Int ?=> Int = n ?=> need }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for name in ["make", "inferred"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        typer
            .type_method_definition(definition_tree(&index, method))
            .unwrap();
        let TreeKind::PhaseSpecific(UntypedNode::Function(f)) = &parsed.ast.get(rhs).kind else {
            panic!()
        };
        let closure = typer.function_literals.method_at(source, rhs).unwrap();
        let param = typer
            .function_literals
            .parameter_at(source, f.params[0])
            .unwrap();
        assert_eq!(typer.store.symbols.get(param).owner, Some(closure));
        assert!(
            typer
                .store
                .symbols
                .get(param)
                .flags
                .contains(SymbolFlags::GIVEN)
        );
        assert!(
            matches!(*typer.store.symbols.info(closure),SymbolInfo::Complete(ty) if matches!(typer.store.types.get(ty),Type::Method(m) if m.kind==MethodKind::Contextual && m.result==definitions.int))
        );
        let TreeKind::Apply(call) = &typer
            .typed_ast()
            .get(typer.typed_index.get(source, f.body).unwrap())
            .kind
        else {
            panic!()
        };
        assert_eq!(reference(&typer, call.args[0]), param);
    }
}

#[test]
fn applied_context_function_uses_its_instantiated_apply_signature() {
    let text = "package scala; class C { def supplied(f: Int ?=> Boolean): Boolean = f(using 3); def auto(f: Int ?=> Boolean)(using n: Int): Boolean = f; def missing(f: Int ?=> Boolean): Boolean = f; def kept(f: Int ?=> Boolean): Int ?=> Boolean = f; def need(using n: Int): Int = n; def build: Int ?=> Int = need }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for name in ["supplied", "auto", "missing", "kept", "build"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let result = typer.type_method_definition(definition_tree(&index, method));
        if name == "missing" {
            assert!(
                matches!(result,Err(TyperError::MissingContextualArgument{expected,..}) if expected==definitions.int),
                "{result:?}"
            );
            continue;
        }
        result.unwrap_or_else(|error| panic!("{name}: {error:?}"));
        let typed = typer.typed_index.get(source, rhs).unwrap();
        if matches!(name, "kept" | "build") {
            let closure = typer.function_literals.method_at(source, rhs).unwrap();
            let definition = typer.function_literals.definition_at(source, rhs).unwrap();
            let TreeKind::DefDef(d) = &typer.typed_ast().get(definition).kind else {
                panic!()
            };
            let TreeKind::Apply(call) = &typer.typed_ast().get(d.rhs.unwrap()).kind else {
                panic!()
            };
            let evidence = reference(&typer, call.args[0]);
            assert_eq!(typer.store.symbols.get(evidence).owner, Some(closure));
            assert!(
                typer
                    .store
                    .symbols
                    .get(evidence)
                    .flags
                    .contains(SymbolFlags::GIVEN)
            );
            assert!(matches!(
                typer.typed_ast().get(typed).kind,
                TreeKind::Closure(_)
            ));
            continue;
        }
        let TreeKind::Apply(call) = &typer.typed_ast().get(typed).kind else {
            panic!()
        };
        let TreeKind::Select(selected) = &typer.typed_ast().get(call.function).kind else {
            panic!()
        };
        assert_eq!(typer.store.names.resolve(selected.name.text()), "apply");
        assert_eq!(
            reference(&typer, selected.qualifier),
            method_parameter_symbol(&parsed, &index, source, method, 0)
        );
        assert_eq!(typer.typed_ast().get(typed).ty, definitions.boolean);
        assert_eq!(
            call.kind,
            if name == "supplied" {
                ApplyKind::Using
            } else {
                ApplyKind::Regular
            }
        );
    }
}

#[test]
fn by_name_contexts_search_the_result_type_without_eager_evaluation() {
    let text = "class A; class B; object B { implicit def provide(implicit a: => A): B = new B }; class C { def read(using n: => A): A=n; def ask(using b:B):B=b; def forward(using a:A): A=read; def nested(using a:A): B=ask; def missing:B=ask }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for name in ["forward", "missing", "nested"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let result = typer.type_method_definition(definition_tree(&index, method));
        if name == "missing" {
            assert!(
                matches!(result, Err(TyperError::MissingContextualArgument { .. })),
                "{result:?}"
            );
            continue;
        }
        result.unwrap_or_else(|error| panic!("{name}: {error:?}"));
        let TreeKind::Apply(call) = &typer
            .typed_ast()
            .get(typer.typed_index.get(source, rhs).unwrap())
            .kind
        else {
            panic!()
        };
        let value = if name == "nested" {
            let TreeKind::Apply(provider) = &typer.typed_ast().get(call.args[0]).kind else {
                panic!()
            };
            provider.args[0]
        } else {
            call.args[0]
        };
        assert_eq!(
            reference(&typer, value),
            method_parameter_symbol(&parsed, &index, source, method, 0)
        );
    }
}

#[test]
fn bounded_import_selects_a_conditional_declaration_and_preserves_its_type_parameters() {
    let text = "class Box[A]; trait TC[A]; object Provider { given leaf:TC[Int] with {}; given boxes: [A] => TC[A] => TC[Box[A]] = new TC[Box[A]] {} }; class C { import Provider.given TC[Box[Int]]; def ask(using t:TC[Box[Boolean]]):Int=1; def use(using b:TC[Boolean]):Int=ask; def needInt(using t:TC[Int]):Int=1; def excluded:Int=needInt }; class Direct { import Provider.leaf; def requireInt(using t:TC[Int]):Int=1; def direct:Int=requireInt }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for name in ["excluded", "use", "direct"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let result = typer.type_method_definition(definition_tree(&index, method));
        if name == "excluded" {
            assert!(
                matches!(result, Err(TyperError::MissingContextualArgument { .. })),
                "{result:?}"
            );
            continue;
        }
        result.unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let TreeKind::Apply(call) = &typer
            .typed_ast()
            .get(typer.typed_index.get(source, rhs).unwrap())
            .kind
        else {
            panic!()
        };
        if name == "direct" {
            let evidence = reference(&typer, call.args[0]);
            let entry = typer.store.symbols.get(evidence);
            assert_eq!(entry.kind, SymbolKind::Object);
            assert_eq!(typer.store.names.resolve(entry.name.text()), "leaf");
            assert!(matches!(
                typer.typed_ast().get(call.args[0]).kind,
                TreeKind::Select(_)
            ));
            continue;
        }
        let TreeKind::Apply(provider) = &typer.typed_ast().get(call.args[0]).kind else {
            panic!()
        };
        let TreeKind::TypeApply(applied) = &typer.typed_ast().get(provider.function).kind else {
            panic!()
        };
        assert_eq!(
            typer.typed_ast().get(applied.args[0]).ty,
            definitions.boolean
        );
        assert_eq!(
            reference(&typer, provider.args[0]),
            method_parameter_symbol(&parsed, &index, source, method, 0)
        );
    }
}

#[test]
fn generic_evidence_infers_through_its_declared_parent_result() {
    let text = include_str!("../../../tests/fixtures/contextual-arguments/Inherited.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "run");
    let producer = method_symbol(&parsed, &store, &index, source, "instance");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap();
    let TreeKind::Apply(call) = &typer
        .typed_ast()
        .get(typer.typed_index.get(source, rhs).unwrap())
        .kind
    else {
        panic!()
    };
    let TreeKind::TypeApply(produced) = &typer.typed_ast().get(call.args[0]).kind else {
        panic!()
    };
    assert_eq!(reference(&typer, produced.function), producer);
    assert_eq!(typer.typed_ast().get(produced.args[0]).ty, definitions.int);
}

#[test]
fn a_strictly_better_provider_recovers_from_an_ambiguous_nested_search() {
    let text = include_str!("../../../tests/fixtures/contextual-arguments/Calls.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "healed");
    let (direct, _, _) = val_definition_and_rhs(&parsed, &store, &index, source, "direct");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap();
    let TreeKind::Apply(call) = &typer
        .typed_ast()
        .get(typer.typed_index.get(source, rhs).unwrap())
        .kind
    else {
        panic!()
    };
    assert_eq!(reference(&typer, call.args[0]), direct);
}

#[test]
fn an_equally_ranked_provider_preserves_the_ambiguous_dependency() {
    let text = include_str!("../../../tests/fixtures/contextual-arguments/Invalid.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "unresolved");
    let ambiguous: Vec<_> = (0..2)
        .map(|i| method_parameter_symbol(&parsed, &index, source, method, i))
        .collect();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let checkpoint = typer.store.checkpoint();
    let result = typer.type_method_definition(definition_tree(&index, method));
    assert!(
        matches!(result, Err(TyperError::AmbiguousContextualArgument{candidates,..}) if candidates==ambiguous)
    );
    assert_eq!(typer.store.checkpoint(), checkpoint);
}

#[test]
fn positional_arguments_apply_to_a_contextual_methods_returned_value() {
    let text = include_str!("../../../tests/fixtures/contextual-arguments/Calls.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "run");
    let param = method_parameter_symbol(&parsed, &index, source, method, 0);
    let apply = method_symbol(&parsed, &store, &index, source, "apply");
    let provider = method_symbol(&parsed, &store, &index, source, "provider");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap();
    let TreeKind::Apply(outer) = &typer
        .typed_ast()
        .get(typer.typed_index.get(source, rhs).unwrap())
        .kind
    else {
        panic!()
    };
    assert_eq!(reference(&typer, outer.function), apply);
    let TreeKind::Select(selected) = &typer.typed_ast().get(outer.function).kind else {
        panic!()
    };
    let TreeKind::Apply(inner) = &typer.typed_ast().get(selected.qualifier).kind else {
        panic!()
    };
    assert_eq!(reference(&typer, inner.function), provider);
    assert_eq!(reference(&typer, inner.args[0]), param);
    assert!(matches!(
        typer.typed_ast().get(outer.args[0]).kind,
        TreeKind::Literal(_)
    ));
}

#[test]
fn malformed_contextual_import_rename_rolls_back_with_the_import_location() {
    let text = "object P { given n:Int=1 }; class C { import P.{n as renamed}; def need(using i:Int):Int=i; def use:Int=need }";
    let (mut parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let (import, rename) = parsed
        .ast
        .iter()
        .find_map(|(id, node)| {
            let TreeKind::Import(import) = &node.kind else {
                return None;
            };
            Some((id, import.selectors.first()?.renamed?))
        })
        .unwrap();
    parsed.ast.get_mut(rename).kind = TreeKind::Literal(dotty_core::ast::Literal {
        value: dotty_core::Constant::Unit,
    });
    let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let before = typer.store.checkpoint();
    let result = typer.type_method_definition(definition_tree(&index, method));
    assert!(
        matches!(result, Err(TyperError::MalformedSourceImport{import_tree_index,..}) if import_tree_index==import.index()),
        "{result:?}"
    );
    assert_eq!(typer.store.checkpoint(), before);
    assert!(typer.typed_index.is_empty());
}

#[test]
fn structural_factories_preserve_type_only_and_context_only_clause_shapes() {
    let text = include_str!("../../../tests/fixtures/contextual-arguments/StructuralClauses.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for (client_name, implementation_name) in [("typeOnly", "simple"), ("contextOnly", "mono")] {
        let class = class_symbol(&parsed, typer.store(), &index, source, implementation_name);
        let factory = index
            .derived_symbol_at(
                typer.store.symbols.owner(class).unwrap(),
                source,
                definition_tree(&index, class),
            )
            .unwrap();
        let (client, rhs) =
            method_definition_and_rhs(&parsed, typer.store(), &index, source, client_name);
        typer
            .type_method_definition(definition_tree(&index, client))
            .unwrap_or_else(|e| panic!("{client_name}: {e:?}"));
        let root = typer.typed_index.get(source, rhs).unwrap();
        let TreeKind::Apply(request) = &typer.typed_ast().get(root).kind else {
            panic!("expected evidence application")
        };
        let selected = match &typer.typed_ast().get(request.args[0]).kind {
            TreeKind::TypeApply(app) if client_name == "typeOnly" => {
                assert_eq!(app.args.len(), 1);
                assert_eq!(typer.typed_ast().get(app.args[0]).ty, definitions.int);
                app.function
            }
            TreeKind::Apply(app) if client_name == "contextOnly" => {
                assert_eq!(app.args.len(), 1);
                assert_eq!(
                    reference(&typer, app.args[0]),
                    method_parameter_symbol(&parsed, &index, source, client, 0)
                );
                app.function
            }
            other => panic!("{client_name}: {other:?}"),
        };
        assert_eq!(reference(&typer, selected), factory);
    }
}

#[test]
fn imported_structural_given_builds_the_factory_call_and_selects_the_implementation_method() {
    let text = include_str!("../../../tests/fixtures/contextual-arguments/Structural.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let class = class_symbol(&parsed, &store, &index, source, "boxes");
    let tree = definition_tree(&index, class);
    let factory = index
        .derived_symbol_at(store.symbols.owner(class).unwrap(), source, tree)
        .unwrap();
    let tag_name = *dotty_core::TermName::new(store.names.intern("tag")).as_name();
    let [tag] = *store
        .scopes
        .get(index.scope_of(class).unwrap())
        .lookup_all(&tag_name)
    else {
        panic!("implementation tag")
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer
        .type_method_definition(definition_tree(&index, tag))
        .expect("type the implementation body");
    for name in ["inferred", "explicit"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let leaf = method_parameter_symbol(&parsed, &index, source, method, 0);
        typer
            .type_method_definition(definition_tree(&index, method))
            .unwrap_or_else(|err| panic!("{name}: {err:?}"));
        let root = typer.typed_index.get(source, rhs).unwrap();
        let factory_call = match &typer.typed_ast().get(root).kind {
            TreeKind::Apply(call) if name == "inferred" => call.args[0],
            TreeKind::Select(member) if name == "explicit" => {
                assert_eq!(reference(&typer, root), tag);
                member.qualifier
            }
            other => panic!("{name}: {other:?}"),
        };
        let TreeKind::Apply(call) = &typer.typed_ast().get(factory_call).kind else {
            panic!()
        };
        assert_eq!(reference(&typer, call.args[0]), leaf);
        let TreeKind::TypeApply(applied) = &typer.typed_ast().get(call.function).kind else {
            panic!()
        };
        assert_eq!(typer.typed_ast().get(applied.args[0]).ty, definitions.int);
        assert_eq!(reference(&typer, applied.function), factory);
        assert!(matches!(
            typer.typed_ast().get(applied.function).kind,
            TreeKind::Select(_)
        ));
    }
}

#[test]
fn structural_context_bound_owns_distinct_class_constructor_and_factory_witnesses() {
    let fixture =
        include_str!("../../../tests/fixtures/contextual-arguments/StructuralBound.scala");
    let text = format!(
        "{fixture}\nclass Missing {{ import Builders.given; def needBox(using t:TC[Box[Int]]):Int=1; def missing:Int=needBox }}"
    );
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(&text);
    let class = class_symbol(&parsed, &store, &index, source, "boxes");
    let tree = definition_tree(&index, class);
    let factory = index
        .derived_symbol_at(store.symbols.owner(class).unwrap(), source, tree)
        .unwrap();
    let bound = parsed
        .ast
        .iter()
        .find_map(|(id, node)| {
            matches!(
                node.kind,
                TreeKind::PhaseSpecific(UntypedNode::ContextBoundTypeTree(_))
            )
            .then_some(id)
        })
        .unwrap();
    let TreeKind::TypeDef(definition) = &parsed.ast.get(tree).kind else {
        panic!()
    };
    let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
        panic!()
    };
    let ctor = index.symbol_at(source, template.constructor).unwrap();
    let class_witness = index.symbol_at(source, bound).unwrap();
    assert_eq!(
        store.symbols.get(class_witness).visibility,
        Visibility::Private
    );
    for (owner, witness) in [
        (class, class_witness),
        (ctor, index.derived_symbol_at(ctor, source, bound).unwrap()),
        (
            factory,
            index.derived_symbol_at(factory, source, bound).unwrap(),
        ),
    ] {
        assert_eq!(store.symbols.owner(witness), Some(owner));
        assert!(
            store
                .symbols
                .get(witness)
                .flags
                .contains(SymbolFlags::GIVEN)
        );
    }
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let (missing, _) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "missing");
    let before = typer.store.checkpoint();
    assert!(matches!(
        typer.type_method_definition(definition_tree(&index, missing)),
        Err(TyperError::MissingContextualArgument { .. })
    ));
    assert_eq!(typer.store.checkpoint(), before);
    assert!(typer.typed_index.is_empty());
    for symbol in [ctor, factory, class_witness] {
        typer.complete_symbol(symbol).unwrap();
    }
    let own_tag = typer
        .store
        .scopes
        .get(index.scope_of(class).unwrap())
        .lookup_all(&Name::new(
            typer.store.names.get("tag").unwrap(),
            dotty_core::Namespace::Term,
        ))[0];
    typer
        .type_method_definition(definition_tree(&index, own_tag))
        .unwrap();
    let (method, rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "inferred");
    typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap();
    let TreeKind::Apply(need) = &typer
        .typed_ast()
        .get(typer.typed_index.get(source, rhs).unwrap())
        .kind
    else {
        panic!()
    };
    let TreeKind::Apply(call) = &typer.typed_ast().get(need.args[0]).kind else {
        panic!()
    };
    assert_eq!(
        reference(&typer, call.args[0]),
        method_parameter_symbol(&parsed, &index, source, method, 0)
    );
}

#[path = "contextual_paths.rs"]
mod contextual_paths;

#[path = "recursive_dictionary.rs"]
mod recursive_dictionary;

#[test]
fn defaults_use_original_receiver_and_earlier_arguments_only_after_missing_evidence() {
    let fixture = include_str!("../../../tests/fixtures/contextual-arguments/Defaults.scala");
    let text = format!(
        "{fixture}\nclass Amb {{ def amb(r: Reader)(using a: Int, b: Int): Int = r.read }}"
    );
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(&text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for name in ["read", "offset", "forwarded", "fallback", "previous", "amb"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let result = typer.type_method_definition(definition_tree(&index, method));
        if name == "amb" {
            assert!(
                matches!(result, Err(TyperError::AmbiguousContextualArgument { .. })),
                "{result:?}"
            );
            assert!(typer.typed_index.get(source, rhs).is_none());
            continue;
        }
        result.unwrap_or_else(|e| panic!("{name}: {e:?}"));
        if matches!(name, "read" | "offset") {
            continue;
        }
        let rhs = typer.typed_index.get(source, rhs).unwrap();
        let TreeKind::Apply(call) = &typer.typed_arena.get(rhs).kind else {
            panic!("missing call")
        };
        if name == "forwarded" {
            assert_eq!(call.kind, ApplyKind::Regular);
            let TreeKind::DefDef(def) = &parsed.ast.get(definition_tree(&index, method)).kind
            else {
                panic!()
            };
            assert_eq!(
                Some(reference(&typer, call.args[0])),
                index.symbol_at(source, def.value_param_clauses[1][0])
            );
            continue;
        }
        assert_eq!(call.kind, ApplyKind::Using);
        let (target, getter) = if name == "previous" {
            let TreeKind::Apply(target) = &typer.typed_arena.get(call.function).kind else {
                panic!()
            };
            let TreeKind::Apply(getter) = &typer.typed_arena.get(call.args[0]).kind else {
                panic!()
            };
            assert_eq!(target.args, getter.args);
            (target.function, getter.function)
        } else {
            (call.function, call.args[0])
        };
        let TreeKind::Select(member) = &typer.typed_arena.get(target).kind else {
            panic!()
        };
        let TreeKind::Select(default) = &typer.typed_arena.get(getter).kind else {
            panic!()
        };
        assert_eq!(member.qualifier, default.qualifier);
        let getter = reference(&typer, getter);
        let (selected, _, _) = typer.contextual_getter_source(getter).unwrap();
        assert_eq!(
            index.symbol_at(source, selected),
            Some(reference(&typer, target))
        );
    }
}

#[test]
fn legacy_default_preserves_ambiguity_and_rolls_back_the_failed_call() {
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(include_str!(
        "../../../tests/fixtures/contextual-arguments/LegacyDefaultAmbiguous.scala"
    ));
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "ambiguous");
    let formals: Vec<_> = (0..2)
        .map(|i| method_parameter_symbol(&parsed, &index, source, method, i))
        .collect();
    let legacy = method_symbol(&parsed, &store, &index, source, "legacy");
    let getter = store
        .default_getter(legacy, 0)
        .expect("named legacy default getter");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let checkpoint = typer.store().checkpoint();

    for _ in 0..2 {
        let error = typer
            .type_method_definition(definition_tree(&index, method))
            .unwrap_err();
        let TyperError::AmbiguousContextualArgument {
            source: at_source,
            tree_index,
            expected,
            candidates,
        } = error
        else {
            panic!("expected ambiguous legacy default: {error:?}")
        };
        assert_eq!(
            (at_source, tree_index, expected),
            (source, rhs.index(), definitions.int)
        );
        assert_eq!(candidates, formals);
        assert_eq!(typer.store().checkpoint(), checkpoint);
        assert_eq!(typer.store().default_getter(legacy, 0), Some(getter));
        assert!(typer.source_typed_index().is_empty());
        assert_eq!(typer.typed_ast().iter().count(), 0);
    }
}

#[test]
fn default_getter_types_its_body_in_the_scope_of_its_own_parameter_copies() {
    let text =
        "class R { def f[A](x: A)(using n: A = x): A = n; def bad(using n: Int = false): Int = n }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let bad = method_symbol(&parsed, typer.store(), &index, source, "bad");
    let error = typer
        .type_method_definition(definition_tree(&index, bad))
        .unwrap_err();
    assert!(
        matches!(error, TyperError::ExpectedExpressionTypeMismatch { .. }),
        "{error:?}"
    );
    assert!(typer.typed_index.is_empty());
    let method = method_symbol(&parsed, typer.store(), &index, source, "f");
    let tree = definition_tree(&index, method);
    typer.type_method_definition(tree).unwrap();
    let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
        panic!()
    };
    let param = def.value_param_clauses[1][0];
    let getter = index
        .derived_symbol_at(typer.store.symbols.owner(method).unwrap(), source, param)
        .unwrap();
    let TreeKind::ValDef(p) = &parsed.ast.get(param).kind else {
        panic!()
    };
    let typed = typer.typed_index.get(source, p.rhs.unwrap()).unwrap();
    assert_eq!(
        reference(&typer, typed),
        index
            .derived_symbol_at(getter, source, def.value_param_clauses[0][0])
            .unwrap()
    );
}

#[test]
fn contextual_defaults_evaluate_the_receiver_and_earlier_arguments_once_in_order() {
    let text = "class Reader { def offset(x:Int)(using n:Int = x, b:Boolean = true):Int = n }; class C { def reader():Reader = new Reader; def next():Int = 4; def run():Int = reader().offset(next()) }";
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(text);
    let (run, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "run");
    let reader = method_symbol(&parsed, &store, &index, source, "reader");
    let next = method_symbol(&parsed, &store, &index, source, "next");
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    typer
        .type_method_definition(definition_tree(&index, run))
        .unwrap();
    let TreeKind::Block(block) = &typer
        .typed_arena
        .get(typer.typed_index.get(source, rhs).unwrap())
        .kind
    else {
        panic!("missing shared operands")
    };
    let [receiver, argument] = block.stats.as_slice() else {
        panic!("each operand should be evaluated once")
    };
    let [receiver_local, argument_local] =
        [(*receiver, reader), (*argument, next)].map(|(tree, method)| {
            let TreeKind::ValDef(shared) = &typer.typed_arena.get(tree).kind else {
                panic!("expected temporary")
            };
            let TreeKind::Apply(init) = &typer.typed_arena.get(shared.rhs.unwrap()).kind else {
                panic!("expected initializer call")
            };
            assert_eq!(reference(&typer, init.function), method);
            typer.typed_index.synthetic_definition_symbol(tree).unwrap()
        });
    let TreeKind::Apply(call) = &typer.typed_arena.get(block.expr).kind else {
        panic!()
    };
    assert_eq!(call.args.len(), 2);
    for tree in std::iter::once(&call.function).chain(&call.args) {
        let TreeKind::Apply(prefix) = &typer.typed_arena.get(*tree).kind else {
            panic!("expected replay of the same earlier clause")
        };
        assert_eq!(reference(&typer, prefix.args[0]), argument_local);
        let TreeKind::Select(member) = &typer.typed_arena.get(prefix.function).kind else {
            panic!("expected receiver")
        };
        assert_eq!(reference(&typer, member.qualifier), receiver_local);
    }
}

#[test]
fn conditional_provider_uses_its_own_default_after_dependency_search() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(include_str!(
        "../../../tests/fixtures/contextual-arguments/Defaults.scala"
    ));
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "derived");
    let provider = method_symbol(&parsed, &store, &index, source, "box");
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap();
    let TreeKind::Apply(need) = &typer
        .typed_arena
        .get(typer.typed_index.get(source, rhs).unwrap())
        .kind
    else {
        panic!()
    };
    let TreeKind::Apply(boxes) = &typer.typed_arena.get(need.args[0]).kind else {
        panic!()
    };
    assert_eq!(reference(&typer, boxes.function), provider);
    assert_eq!(boxes.kind, ApplyKind::Using);
    let (def, _, _) = typer
        .contextual_getter_source(reference(&typer, boxes.args[0]))
        .unwrap();
    assert_eq!(index.symbol_at(source, def), Some(provider));
}

#[test]
fn nested_context_abstraction_registers_both_methods_and_rolls_back_a_bad_body() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(
        "package scala; class C { def constant: Int ?=> (Boolean ?=> Int) = 1; def bad: Int ?=> (Boolean ?=> Int) = false; def deeper: Int ?=> (Boolean ?=> (Int ?=> Int)) = 1 }",
    );
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let (method, rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "constant");
    let root = typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap();
    let mut closure_tree = typer.source_typed_index().get(source, rhs).unwrap();
    let mut owner = method;
    for expected in [defs.int, defs.boolean] {
        let TreeKind::Closure(closure) = &typer.typed_ast().get(closure_tree).kind else {
            panic!("expected a context function")
        };
        assert!(closure.env.is_empty());
        let implementation = closure.implementation.as_ref().unwrap();
        let method_symbol = reference(&typer, closure.method);
        assert_eq!(
            typer
                .source_typed_index()
                .synthetic_definition_symbol(implementation.definition),
            Some(method_symbol)
        );
        assert_eq!(typer.store().symbols.owner(method_symbol), Some(owner));
        if owner == method {
            assert_eq!(
                typer.function_literal_method_at(source, rhs),
                Some(method_symbol)
            );
            assert_eq!(
                typer.function_literal_definition_at(source, rhs),
                Some(implementation.definition)
            );
        }
        let [param] = implementation.parameters.as_slice() else {
            panic!("one binder")
        };
        let record = typer.store().symbols.get(*param);
        assert_eq!(record.owner, Some(method_symbol));
        assert_eq!(record.info, SymbolInfo::Complete(expected));
        assert!(record.flags.contains(SymbolFlags::GIVEN));
        let scope = typer.function_literal_method_scope(method_symbol).unwrap();
        assert_eq!(
            typer.store().scopes.get(scope).lookup_all(&record.name),
            &[*param]
        );
        let TreeKind::DefDef(def) = &typer.typed_ast().get(implementation.definition).kind else {
            panic!("method definition")
        };
        closure_tree = def.rhs.unwrap();
        owner = method_symbol;
    }
    assert!(
        matches!(&typer.typed_ast().get(closure_tree).kind, TreeKind::Literal(l) if l.value == dotty_core::types::Constant::Int(1))
    );
    assert_eq!(
        typer
            .type_method_definition(definition_tree(&index, method))
            .unwrap(),
        root
    );
    for name in ["bad", "deeper"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let checkpoint = typer.store().checkpoint();
        let nodes = typer.typed_ast().iter().count();
        for _ in 0..2 {
            let err = typer
                .type_method_definition(definition_tree(&index, method))
                .unwrap_err();
            if name == "bad" {
                assert!(
                    matches!(err, TyperError::ExpectedExpressionTypeMismatch { expected, .. } if expected == defs.int),
                    "{err:?}"
                );
            } else {
                assert!(
                    matches!(err, TyperError::UnsupportedExpression { tree_index, .. } if tree_index == rhs.index()),
                    "{err:?}"
                );
            }
            assert_eq!(typer.store().checkpoint(), checkpoint);
            assert_eq!(typer.typed_ast().iter().count(), nodes);
            assert_eq!(typer.function_literal_method_at(source, rhs), None);
            assert_eq!(typer.source_typed_index().get(source, rhs), None);
        }
    }
}
