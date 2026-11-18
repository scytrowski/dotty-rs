use super::*;

#[test]
fn recursive_by_name_arguments_reuse_the_same_dictionary_field() {
    let fixture = include_str!("../../../tests/fixtures/contextual-arguments/Recursive.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(fixture);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "derived");
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
    let arena = typer.typed_ast();
    let rhs = typer.typed_index.get(source, rhs).unwrap();
    let TreeKind::Apply(read) = &arena.get(rhs).kind else {
        panic!("read")
    };
    let TreeKind::Block(block) = &arena.get(read.args[0]).kind else {
        panic!("dictionary")
    };
    let [class, instance] = block.stats.as_slice() else {
        panic!("class and instance")
    };
    let TreeKind::TypeDef(def) = &arena.get(*class).kind else {
        panic!("holder class")
    };
    let TreeKind::Template(template) = &arena.get(def.rhs).kind else {
        panic!("holder template")
    };
    let [field] = template.body.as_slice() else {
        panic!("one recursive entry")
    };
    let field_symbol = typer
        .typed_index
        .synthetic_definition_symbol(*field)
        .unwrap();
    let class_symbol = typer
        .typed_index
        .synthetic_definition_symbol(*class)
        .unwrap();
    let instance_symbol = typer
        .typed_index
        .synthetic_definition_symbol(*instance)
        .unwrap();
    assert_eq!(typer.store.symbols.owner(field_symbol), Some(class_symbol));
    assert_eq!(typer.store.symbols.owner(class_symbol), Some(method));
    assert_eq!(typer.store.symbols.owner(instance_symbol), Some(method));
    let TreeKind::ValDef(entry) = &arena.get(*field).kind else {
        panic!("dictionary field")
    };
    let TreeKind::Apply(provider) = &arena.get(entry.rhs.unwrap()).kind else {
        panic!("recursive provider")
    };
    assert_eq!(
        typer.store.names.resolve(
            typer
                .store
                .symbols
                .get(reference(&typer, provider.function))
                .name
                .text()
        ),
        "loop"
    );
    assert_eq!(provider.args.len(), 2);
    for &argument in &provider.args {
        assert_eq!(reference(&typer, argument), field_symbol);
        let TreeKind::Select(select) = &arena.get(argument).kind else {
            panic!("field selection")
        };
        assert!(
            matches!(typer.store.types.get(arena.get(select.qualifier).ty), Type::ThisType {class} if *class==class_symbol)
        );
    }
    let TreeKind::Select(result) = &arena.get(block.expr).kind else {
        panic!("dictionary result")
    };
    assert_eq!(reference(&typer, block.expr), field_symbol);
    assert_eq!(reference(&typer, result.qualifier), instance_symbol);
}

#[test]
fn dictionary_entries_are_visible_to_siblings_only_in_the_selected_search() {
    let text = "class Leaf; object Leaf { implicit def loop(implicit again: => Leaf): Leaf = new Leaf }; class Pair; object Pair { implicit def both(implicit first: Leaf, second: Leaf): Pair = new Pair }; class C { def ask(using pair: Pair): Int = 1; def use: Int = ask }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
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
    let arena = typer.typed_ast();
    let TreeKind::Apply(ask) = &arena.get(typer.typed_index.get(source, rhs).unwrap()).kind else {
        panic!("ask")
    };
    let TreeKind::Block(dictionary) = &arena.get(ask.args[0]).kind else {
        panic!("dictionary")
    };
    let TreeKind::Apply(pair) = &arena.get(dictionary.expr).kind else {
        panic!("pair")
    };
    let [first, second] = pair.args[..] else {
        panic!("two leaves")
    };
    assert_eq!(reference(&typer, first), reference(&typer, second));
    let (TreeKind::Select(a), TreeKind::Select(b)) =
        (&arena.get(first).kind, &arena.get(second).kind)
    else {
        panic!("shared dictionary")
    };
    assert_eq!(a.qualifier, b.qualifier);
    let instance = typer
        .typed_index
        .synthetic_definition_symbol(dictionary.stats[1])
        .unwrap();
    assert_eq!(reference(&typer, a.qualifier), instance);
}

#[test]
fn dictionary_allocations_roll_back_with_a_failed_recursive_candidate() {
    for alternative in ["", "given direct: Cycle = new Cycle"] {
        let text = format!(
            "class Missing; class Cycle; object Cycle {{ implicit def broken(implicit recur: => Cycle, other: Missing): Cycle = new Cycle; {alternative} }}; class C {{ def take(using c: Cycle): Int=1; def use:Int=take }}"
        );
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(&text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let checkpoint = typer.store.checkpoint();
        let root = definition_tree(&index, method);
        if alternative.is_empty() {
            for _ in 0..2 {
                assert!(matches!(
                    typer.type_method_definition(root),
                    Err(TyperError::MissingContextualArgument { .. })
                ));
                assert_eq!(typer.store.checkpoint(), checkpoint);
                assert!(typer.typed_index.is_empty());
                assert_eq!(typer.typed_ast().iter().count(), 0);
            }
        } else {
            typer.type_method_definition(root).unwrap();
            let TreeKind::Apply(take) = &typer
                .typed_ast()
                .get(typer.typed_index.get(source, rhs).unwrap())
                .kind
            else {
                panic!("take")
            };
            let entry = typer.store.symbols.get(reference(&typer, take.args[0]));
            assert_eq!(typer.store.names.resolve(entry.name.text()), "direct");
            assert!(typer.typed_ast().iter().all(|(tree, _)| {
                typer
                    .typed_index
                    .synthetic_definition_symbol(tree)
                    .is_none()
            }));
        }
    }
}
