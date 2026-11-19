use super::*;

const RECURSIVE: &str = "tests/pos/byname-implicits-1.scala";
const COMPANION: &str = "tests/pos/byname-implicits-2.scala";
const AMBIGUOUS: &str = "tests/neg/given-ambiguous-2.scala";
const GROWING: &str = "tests/neg/byname-implicits-11.scala";

/// Select the actual statement of the module constructor, as written, including
/// the type argument. The companion test contains six other `implicitly` calls.
fn object_statement(
    text: &str,
    fragment: &str,
    parsed: &dotty_parser::ParseResult,
    store: &SemanticStore,
    index: &SourceSemanticIndex,
    source: SourceId,
) -> (SymbolId, TreeId<Untyped>) {
    let template = parsed
        .ast
        .iter()
        .find_map(|(_, node)| match &node.kind {
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module))
                if store.names.resolve(module.name.as_name().text()) == "Test" =>
            {
                let TreeKind::Template(template) = &parsed.ast.get(module.template).kind else {
                    panic!("module template")
                };
                Some(template)
            }
            _ => None,
        })
        .unwrap();
    let statement = template
        .body
        .iter()
        .copied()
        .find(|tree| {
            parsed.ast.get(*tree).position.is_some_and(|pos| {
                let span = pos.span().range();
                &text[span.start() as usize..span.end() as usize] == fragment
            })
        })
        .unwrap_or_else(|| panic!("statement {fragment} in {:?}", template.body));
    (
        index.symbol_at(source, template.constructor).unwrap(),
        statement,
    )
}

fn check_module_statement(
    path: &str,
    fragment: &str,
    check: impl FnOnce(
        &dotty_parser::ParseResult,
        &mut SourceTyper<'_>,
        TreeId<Untyped>,
        ExpressionContext,
    ),
) {
    let text = upstream(path);
    let (parsed, mut store, packages, defs, index, source) =
        parse_and_name_file(&text, path.rsplit('/').next().unwrap());
    let (owner, stmt) = object_statement(&text, fragment, &parsed, &store, &index, source);
    let resolver = audit_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    let context = typer.expression_context_for(owner).unwrap();
    check(&parsed, &mut typer, stmt, context);
}

fn implicit_argument(typer: &SourceTyper<'_>, root: TreeId<Typed>) -> TreeId<Typed> {
    let arena = typer.typed_ast();
    let TreeKind::Apply(call) = &arena.get(root).kind else {
        panic!("implicitly[T](e)")
    };
    let [arg] = call.args[..] else {
        panic!("one argument")
    };
    let TreeKind::TypeApply(applied) = &arena.get(call.function).kind else {
        panic!("implicitly[T]")
    };
    let [tpt] = applied.args[..] else {
        panic!("one type argument")
    };
    assert_eq!(arena.get(root).ty, arena.get(tpt).ty);
    let fun = typer.store.symbols.get(referent(typer, applied.function));
    assert_eq!(typer.store.names.resolve(fun.name.text()), "implicitly");
    let TreeKind::Select(select) = &arena.get(applied.function).kind else {
        panic!("Predef.implicitly")
    };
    let predef = referent(typer, select.qualifier);
    let obj = typer.store.symbols.get(predef);
    assert_eq!(typer.store.names.resolve(obj.name.text()), "Predef");
    assert!(!typer.is_current_source_symbol(predef));
    assert_eq!(
        fun.owner,
        Some(typer.module_class_of_object(predef).unwrap())
    );
    arg
}

#[test]
#[ignore = "requires pinned Scala 3.9 sources"]
fn search_recursive_companion() {
    check_module_statement(RECURSIVE, "implicitly[Foo]", |parsed, typer, stmt, ctx| {
        let foo = method_symbol(parsed, typer.store(), typer.index, typer.source, "foo");
        let root = typer.type_expression(stmt, ctx).unwrap();
        let argument = implicit_argument(typer, root);
        let arena = typer.typed_ast();
        let TreeKind::Block(dictionary) = &arena.get(argument).kind else {
            panic!("dictionary block")
        };
        let [holder, _] = dictionary.stats[..] else {
            panic!("dictionary class and instance")
        };
        let TreeKind::TypeDef(class) = &arena.get(holder).kind else {
            panic!("dictionary class")
        };
        let TreeKind::Template(template) = &arena.get(class.rhs).kind else {
            panic!("dictionary template")
        };
        let [field] = template.body[..] else {
            panic!("one recursive entry")
        };
        let field_symbol = typer
            .source_typed_index()
            .synthetic_definition_symbol(field)
            .unwrap();
        let TreeKind::ValDef(def) = &arena.get(field).kind else {
            panic!("dictionary field")
        };
        let TreeKind::Apply(provider) = &arena.get(def.rhs.unwrap()).kind else {
            panic!("Foo.foo(rec)")
        };
        assert_eq!(referent(typer, provider.function), foo);
        let [rec] = provider.args[..] else {
            panic!("one recursive argument")
        };
        assert_eq!(referent(typer, rec), field_symbol);
        assert_eq!(referent(typer, dictionary.expr), field_symbol);
    });
    outcome("match");
}

#[test]
#[ignore = "requires pinned Scala 3.9 sources"]
fn search_stable_companion() {
    check_module_statement(
        COMPANION,
        "implicitly[Foo[Int]]",
        |parsed, typer, stmt, ctx| {
            let (int, _) = val_symbol(parsed, typer.store(), typer.index, typer.source, "fooInt");
            let foo = class_symbol(parsed, typer.store(), typer.index, typer.source, "Foo");
            let root = typer.type_expression(stmt, ctx).unwrap();
            let argument = implicit_argument(typer, root);
            assert_eq!(referent(typer, argument), int);
            let Type::Applied { tycon, args } =
                typer.store.types.get(typer.typed_ast().get(root).ty)
            else {
                panic!("Foo[Int]")
            };
            assert_eq!(args, &[typer.definitions.int]);
            assert!(matches!(
                typer.store.types.get(*tycon),
                Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == foo
            ));
        },
    );
    outcome("match");
}

#[test]
#[ignore = "requires pinned Scala 3.9 sources"]
fn search_growing_byname() {
    check_module_statement(
        GROWING,
        "implicitly[Foo[Int]]",
        |parsed, typer, stmt, ctx| {
            let before = typer.store.checkpoint();
            assert!(matches!(typer.type_expression(stmt, ctx),
            Err(TyperError::MissingContextualArgument { source, tree_index, .. })
            if source == typer.source && tree_index == stmt.index()));
            assert_eq!(typer.store.checkpoint(), before);
            // Pin the original type argument in the session before retrying, so
            // the expected type can be compared by identity after error rollback.
            let TreeKind::TypeApply(call) = &parsed.ast.get(stmt).kind else {
                panic!("implicitly[Foo[Int]]")
            };
            let expected = typer.type_of_tpt(call.args[0], ctx.lexical).unwrap();
            let error = typer.type_expression(stmt, ctx).unwrap_err();
            assert!(
                matches!(error, TyperError::MissingContextualArgument { source, tree_index, expected: missing }
                    if source == typer.source && tree_index == stmt.index() && missing == expected),
                "{error:?}"
            );
        },
    );
    outcome("match");
}

#[test]
#[ignore = "requires pinned Scala 3.9 sources"]
fn search_direct_ambiguity() {
    let text = upstream(AMBIGUOUS);
    let (parsed, mut store, packages, defs, index, source) =
        parse_and_name_file(&text, "given-ambiguous-2.scala");
    let (f, _) = method_definition_and_rhs(&parsed, &store, &index, source, "f");
    let g = method_symbol(&parsed, &store, &index, source, "g");
    let tree = declaration(&index, f);
    let wrapper = store.symbols.owner(f).unwrap();
    assert_eq!(store.symbols.owner(g), Some(wrapper));
    assert_eq!(store.symbols.get(wrapper).kind, SymbolKind::ModuleClass);
    let resolver = audit_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    let error = typer.type_method_definition(tree).unwrap_err();
    let TyperError::MemberLookup(lookup_error) = error else {
        panic!("{error:?}")
    };
    assert!(
        matches!(
            *lookup_error,
            MemberLookupError::ClassInfoUnavailable { symbol, state: crate::types::SymbolInfoState::Missing }
                if symbol == wrapper
        ),
        "{lookup_error:?}"
    );
    assert!(matches!(
        typer.complete_symbol(wrapper),
        Err(TyperError::SourceProvenanceMissing { symbol }) if symbol == wrapper
    ));
    check_ambiguity_with_source_owner();
    outcome("blocked");
}

/// Unscored control with an explicit source object owning the givens and methods.
fn check_ambiguity_with_source_owner() {
    let text =
        include_str!("../../../../tests/fixtures/contextual-v1/controls/OwnerAmbiguity.scala");
    let (parsed, mut store, packages, defs, index, source) =
        parse_and_name_file(text, "OwnerAmbiguity.scala");
    let (f, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "f");
    let tree = declaration(&index, f);
    let candidates = ["a1", "a2"].map(|name| val_symbol(&parsed, &store, &index, source, name).0);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let error = typer.type_method_definition(tree).unwrap_err();
    assert!(
        matches!(
            &error,
            TyperError::AmbiguousContextualArgument { source: at, tree_index, candidates: actual, .. }
                if *at == source && *tree_index == rhs.index() && actual == &candidates
        ),
        "{error:?}"
    );
}
