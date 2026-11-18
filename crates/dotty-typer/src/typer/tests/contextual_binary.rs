use super::*;
use dotty_classloader::classloader::{
    ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath, LoadingSession,
};

#[test]
fn separate_compilation_finds_the_binary_conditional_and_its_companion_leaf() {
    let text = "package consumer; import evidence.{TC, Box}; class Client { def ask(using t: TC[Box[Int]]): TC[Box[Int]] = t; def result: TC[Box[Int]] = ask }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "result");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let resolver = ClasspathSymbolResolver::new(
        CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(
                root.join("tests/fixtures/contextual-arguments/binary"),
            )),
            Box::new(DirectoryClassPath::new(
                root.join("../dotty-tasty/tests/fixtures/scala3-library"),
            )),
            Box::new(JavaObjectStubPath),
        ]),
        definitions,
        LoadingSession::with_packages(packages.snapshot()),
    );
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    typer.type_method_definition(tree).unwrap();
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
    assert_eq!(typer.typed_ast().get(tapp.args[0]).ty, definitions.int);
    for (tree, name) in [(tapp.function, "boxes"), (boxes.args[0], "ints")] {
        let Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        } = typer.store.types.get(typer.typed_ast().get(tree).ty)
        else {
            panic!()
        };
        assert_eq!(
            typer
                .store
                .names
                .resolve(typer.store.symbols.get(*symbol).name.text()),
            name
        );
        assert!(!typer.is_current_source_symbol(*symbol));
    }
}

#[test]
fn predef_context_methods_use_the_library_symbol_and_keep_local_shadowing() {
    let text = "class Use { def summoned(using n: Int): Int = summon[Int]; def supplied(using n: Int): Int = summon[Int](using n); def missing: Int = summon[Int]; def legacy(using n: Int): Int = implicitly[Int] }; class Shadow { def summon[A](using n: Int): Int = 7; def shadowed(using n: Int): Int = summon[Int] }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let resolver = library_resolver(definitions, &packages);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    for name in ["shadowed", "legacy", "missing", "summoned", "supplied"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        if name == "missing" {
            assert!(matches!(
                typer.type_method_definition(tree),
                Err(TyperError::MissingContextualArgument { .. })
            ));
            continue;
        }
        typer
            .type_method_definition(tree)
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        let typed = typer.typed_index.get(source, rhs).unwrap();
        if name == "summoned" || name == "supplied" {
            let param = method_parameter_symbol(&parsed, &index, source, method, 0);
            assert!(
                matches!(typer.store.types.get(typer.typed_ast().get(typed).ty), Type::TermRef {target:TermRefTarget::Symbol(symbol),..} if *symbol==param)
            );
        } else {
            let TreeKind::Apply(app) = &typer.typed_ast().get(typed).kind else {
                panic!()
            };
            let TreeKind::TypeApply(tapp) = &typer.typed_ast().get(app.function).kind else {
                panic!()
            };
            let Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } = typer
                .store
                .types
                .get(typer.typed_ast().get(tapp.function).ty)
            else {
                panic!()
            };
            assert_eq!(typer.is_current_source_symbol(*symbol), name == "shadowed");
        }
    }
}

fn library_resolver(
    definitions: Definitions,
    packages: &Packages,
) -> ClasspathSymbolResolver<CompositeClassPath> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    ClasspathSymbolResolver::new(
        CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(
                root.join("../dotty-tasty/tests/fixtures/scala3-library"),
            )),
            Box::new(
                dotty_classloader::classloader::JdkClassPath::new(
                    std::path::PathBuf::from(
                        std::env::var_os("JAVA_HOME").expect("JDK 21 JAVA_HOME"),
                    )
                    .join("jmods"),
                )
                .unwrap(),
            ),
        ]),
        definitions,
        LoadingSession::with_packages(packages.snapshot()),
    )
}

#[test]
fn library_not_given_inverts_each_search_and_selects_the_real_value_method() {
    let text = "import scala.util.NotGiven; class Ev; class C { def need(using m: NotGiven[Ev]): Int = 1; def free: Int = need; def present(using a: Ev): Int = need; def ambiguous(using a: Ev,b: Ev): Int = need }; object Other { class NotGiven[A]; def ordinary(using m: NotGiven[Int]): Int = 7; def shadow(using instance: NotGiven[Int]): Int = ordinary }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let resolver = library_resolver(definitions, &packages);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    for name in ["present", "ambiguous", "free", "shadow", "present"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        let result = typer.type_method_definition(tree);
        if name == "present" {
            assert!(
                matches!(result, Err(TyperError::MissingContextualArgument { .. })),
                "{name}: {result:?}"
            );
            continue;
        }
        result.unwrap_or_else(|error| panic!("{name}: {error:?}"));
        let typed = typer.typed_index.get(source, rhs).unwrap();
        let TreeKind::Apply(app) = &typer.typed_ast().get(typed).kind else {
            panic!()
        };
        if name == "shadow" {
            let param = method_parameter_symbol(&parsed, &index, source, method, 0);
            assert!(
                matches!(typer.store.types.get(typer.typed_ast().get(app.args[0]).ty), Type::TermRef{target:TermRefTarget::Symbol(symbol),..} if *symbol==param)
            );
            continue;
        }
        let TreeKind::Select(sel) = &typer.typed_ast().get(app.args[0]).kind else {
            panic!("expected NotGiven.value")
        };
        let Type::TermRef {
            target: TermRefTarget::Symbol(value),
            ..
        } = typer.store.types.get(typer.typed_ast().get(app.args[0]).ty)
        else {
            panic!()
        };
        let Type::TermRef {
            target: TermRefTarget::Symbol(object),
            ..
        } = typer
            .store
            .types
            .get(typer.typed_ast().get(sel.qualifier).ty)
        else {
            panic!()
        };
        assert_eq!(typer.store.names.resolve(sel.name.text()), "value");
        assert_eq!(
            typer
                .store
                .names
                .resolve(typer.store.symbols.get(*object).name.text()),
            "NotGiven"
        );
        assert_eq!(
            typer.store.symbols.get(*value).owner,
            Some(typer.module_class_of_object(*object).unwrap())
        );
    }
}
