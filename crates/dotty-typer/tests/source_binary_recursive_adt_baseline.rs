use std::path::PathBuf;

use dotty_classloader::classloader::{
    ClassPathEntry, ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath, JdkClassPath,
    LoadingSession,
};
use dotty_core::ast::TreeKind;
use dotty_core::{
    Definitions, GeneratedCaseClassMethodRole, Packages, SemanticStore, SourceId, SourceText,
    SymbolFlags, SymbolKind, SymbolOrigin, TermName, TypeName,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;

const SOURCE: &str = include_str!("fixtures/source-binary-recursive-adt-3.9.0/Expr.scala");

#[test]
fn binary_recursive_adt_roots_are_attempted_in_two_fresh_resolver_sessions() {
    let first = analyze_roots(SourceId::from_index(20611), "session-a");
    let second = analyze_roots(SourceId::from_index(20612), "session-b");
    assert_eq!(first.normalized, second.normalized);
    assert_eq!(
        first.roots.len(),
        2,
        "only the original source roots count per session"
    );
    assert!(first.roots.iter().any(|row| row.contains("\teval\t")));
    assert!(first.roots.iter().any(|row| row.contains("\tselectLeft\t")));
    let receipt = first
        .roots
        .iter()
        .chain(second.roots.iter())
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        format!("{}\n", receipt.join("\n")),
        include_str!(
            "fixtures/source-binary-recursive-adt-3.9.0/rust-after-namer-identities-two-root.tsv"
        )
        .split_once('\n')
        .expect("post-Namer root receipt header")
        .1
    );
    for row in receipt {
        println!("ISSUE2066_ROOT_AFTER_NAMER\t{row}");
    }
}

#[test]
fn binary_recursive_adt_generated_identities_are_discoverable_without_completion() {
    let actual = characterize_class(SourceId::from_index(20613));
    assert_eq!(
        format!("{actual}\n"),
        include_str!("fixtures/source-binary-recursive-adt-3.9.0/rust-after-namer-identities.tsv")
            .split_once('\n')
            .expect("post-Namer identity evidence header")
            .1
    );
    println!("ISSUE2066_NAMER_IDENTITIES\t{actual}");
}

struct RootReceipt {
    roots: Vec<String>,
    normalized: Vec<String>,
}

fn setup(
    source: SourceId,
) -> (
    dotty_core::ast::AstArena<dotty_core::ast::Untyped>,
    dotty_core::TreeId<dotty_core::ast::Untyped>,
    dotty_core::SourceSemanticIndex,
    SemanticStore,
    Definitions,
    Packages,
) {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(SOURCE).expect("frozen source is valid"),
        source,
        ContextualScanner::new(SOURCE).expect("frozen source scans"),
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Expr.scala",
        &mut store,
        &mut packages,
    )
    .expect("frozen source names");
    (parsed.ast, parsed.root, index, store, definitions, packages)
}

fn resolver(definitions: Definitions) -> ClasspathSymbolResolver<CompositeClassPath> {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dotty-tasty/tests/fixtures/scala3-library");
    assert!(fixture.join("scala/Product.tasty").is_file());
    let java_home = std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required");
    let classpath = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(fixture)) as Box<dyn ClassPathEntry>,
        Box::new(JdkClassPath::new(PathBuf::from(java_home).join("jmods")).expect("JDK modules")),
    ]);
    ClasspathSymbolResolver::new(classpath, definitions, LoadingSession::new())
}

fn analyze_roots(source: SourceId, session: &str) -> RootReceipt {
    let (ast, _root, index, mut store, definitions, packages) = setup(source);
    let mut roots = Vec::new();
    for (tree, node) in ast.iter() {
        let TreeKind::DefDef(definition) = &node.kind else {
            continue;
        };
        let name = store.names.resolve(definition.name.as_name().text());
        if !matches!(name, "selectLeft" | "eval") {
            continue;
        }
        let method = index
            .symbol_at(source, tree)
            .expect("source method identity");
        let parameter_tree = definition.value_param_clauses[0][0];
        let parameter = index
            .symbol_at(source, parameter_tree)
            .expect("source parameter identity");
        let owner = store.symbols.get(method).owner.expect("method owner");
        assert_eq!(store.symbols.get(parameter).owner, Some(method));
        assert_ne!(method, parameter);
        assert_eq!(
            store.names.resolve(store.symbols.get(owner).name.text()),
            "Evaluator$"
        );
        assert!(
            matches!(store.symbols.get(method).origin, SymbolOrigin::Source(id) if id == source)
        );
        assert!(
            matches!(store.symbols.get(parameter).origin, SymbolOrigin::Source(id) if id == source)
        );
        roots.push((
            name.to_owned(),
            tree,
            node.position.unwrap(),
            method,
            parameter,
            owner,
        ));
    }
    roots.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(roots.len(), 2);
    let resolver = resolver(definitions);
    let mut typer =
        dotty_typer::SourceTyper::new(&ast, source, &index, &mut store, definitions, &packages)
            .with_resolver(Box::new(resolver));
    let mut rows = Vec::new();
    let mut normalized = Vec::new();
    for (name, tree, position, _method, _parameter, _owner) in roots {
        let range = position.span().range();
        let outcome = match typer.type_method_definition(tree) {
            Ok(typed) => {
                assert_eq!(typer.source_typed_index().get(source, tree), Some(typed));
                assert!(matches!(
                    typer.typed_ast().get(typed).kind,
                    TreeKind::DefDef(_)
                ));
                format!("complete=whole_DefDef:typed#tree{}", typed.index())
            }
            Err(error) => {
                assert!(typer.source_typed_index().get(source, tree).is_none());
                let debug = format!("{error:?}");
                let error_kind = debug.split(" {").next().unwrap_or(&debug);
                let nested_tree = parse_tree_index(&debug);
                let nested = nested_tree
                    .and_then(|index| ast.iter().find(|(candidate, _)| candidate.index() == index))
                    .map(|(nested_tree, nested_node)| {
                        let nested_range = nested_node.position.unwrap().span().range();
                        format!(
                            "Expr.scala#tree{}@{}:{}",
                            nested_tree.index(),
                            nested_range.start(),
                            nested_range.end()
                        )
                    })
                    .unwrap_or_else(|| "UNREPORTED".to_owned());
                format!("first_error={error_kind}(nested={nested});typed_index=absent")
            }
        };
        rows.push(format!(
            "{session}\t{name}\tpath=Expr.scala#tree{}\tmethod=source-owned\tparameter=owned-by-source-method\tsource_bytes={}:{}\towner=Evaluator$\t{outcome}",
            tree.index(), range.start(), range.end(),
        ));
        normalized.push(format!(
            "{name}\tpath=Expr.scala#tree{}\tsource_bytes={}:{}\towner=Evaluator$\t{outcome}",
            tree.index(),
            range.start(),
            range.end(),
        ));
    }
    RootReceipt {
        roots: rows,
        normalized,
    }
}

fn parse_tree_index(debug: &str) -> Option<u32> {
    let value = debug.split_once("tree_index: ")?.1;
    value
        .split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

fn characterize_class(source: SourceId) -> String {
    let (ast, _root, index, mut store, _definitions, _packages) = setup(source);
    let mut expr = None;
    let mut add = None;
    let mut add_tree = None;
    let mut add_template = None;
    for (tree, node) in ast.iter() {
        let TreeKind::TypeDef(definition) = &node.kind else {
            continue;
        };
        match store.names.resolve(definition.name.as_name().text()) {
            "Expr" => expr = Some(index.symbol_at(source, tree).expect("Expr identity")),
            "Add" => {
                add = Some(index.symbol_at(source, tree).expect("Add identity"));
                add_tree = Some(tree);
                add_template = Some(definition.rhs);
            }
            _ => {}
        }
    }
    let expr = expr.expect("source Expr trait");
    let add = add.expect("source Add class");
    let expr_info = store.symbols.get(expr);
    let add_info = store.symbols.get(add);
    assert_eq!(expr_info.kind, SymbolKind::Trait);
    assert!(expr_info.flags.contains(SymbolFlags::SEALED));
    assert_eq!(add_info.kind, SymbolKind::Class);
    assert!(add_info.flags.contains(SymbolFlags::CASE));
    assert!(matches!(add_info.origin, SymbolOrigin::Source(id) if id == source));
    let add_tree = add_tree.unwrap();
    let add_template = add_template.unwrap();
    let TreeKind::Template(template) = &ast.get(add_template).kind else {
        panic!("Add has no source template");
    };
    assert_eq!(template.parents.len(), 1);
    let parent_name = match &ast.get(template.parents[0]).kind {
        TreeKind::Ident(ident) => store.names.resolve(ident.name.text()).to_owned(),
        other => panic!("unexpected Add parent: {other:?}"),
    };
    let package = add_info.owner.expect("Add package owner");
    let package_scope = index.scope_of(package).expect("source package scope");
    let parent_symbol = store
        .scopes
        .get(package_scope)
        .lookup(TypeName::new(store.names.intern(&parent_name)).as_name());
    assert_eq!(parent_symbol, Some(expr));
    let class_scope = index.scope_of(add).expect("Add class member scope");
    let left = store
        .scopes
        .get(class_scope)
        .lookup(TermName::new(store.names.intern("left")).as_name());
    let right = store
        .scopes
        .get(class_scope)
        .lookup(TermName::new(store.names.intern("right")).as_name());
    let selector1_name = TermName::new(store.names.intern("_1"));
    let selector2_name = TermName::new(store.names.intern("_2"));
    let selector1 = store
        .scopes
        .get(class_scope)
        .lookup(selector1_name.as_name());
    let selector2 = store
        .scopes
        .get(class_scope)
        .lookup(selector2_name.as_name());
    let companion = add_info.links.companion.expect("Add companion");
    let unapply = index.generated_case_class_method(add, GeneratedCaseClassMethodRole::Unapply);
    let companion_name = store
        .names
        .resolve(store.symbols.get(companion).name.text())
        .to_owned();
    let class_facts = format!(
        "Expr=sealed_trait;Add=source_case_class;parent={parent_name}:canonical;fields=left:source-member,right:source-member,distinct={};selectors=_1:{},_2:{},unapply={},companion=module({companion_name})",
        left.is_some() && right.is_some() && left != right,
        if selector1.is_some() {
            "published"
        } else {
            "absent"
        },
        if selector2.is_some() {
            "published"
        } else {
            "absent"
        },
        if unapply.is_some() {
            "published"
        } else {
            "absent"
        },
    );
    assert!(left.is_some() && right.is_some() && left != right);
    assert_eq!(store.symbols.get(left.unwrap()).owner, Some(add));
    assert_eq!(store.symbols.get(right.unwrap()).owner, Some(add));
    let (Some(selector1), Some(selector2), Some(unapply)) = (selector1, selector2, unapply) else {
        panic!("canonical Add must expose all bounded generated identities");
    };
    assert_ne!(selector1, selector2);
    assert_eq!(store.symbols.get(selector1).owner, Some(add));
    assert_eq!(store.symbols.get(selector2).owner, Some(add));
    let companion_module = store
        .scopes
        .get(package_scope)
        .lookup(TypeName::new(store.names.intern("Add$")).as_name())
        .expect("Add companion module class");
    assert_eq!(store.symbols.get(unapply).owner, Some(companion_module));
    let input_formals = store
        .scopes
        .get(index.scope_of(unapply).expect("unapply scope"))
        .lookup_all(TermName::new(store.names.intern("x$1")).as_name());
    assert_eq!(input_formals.len(), 1);
    assert_eq!(store.symbols.get(input_formals[0]).owner, Some(unapply));
    assert!(matches!(
        store.symbols.get(unapply).info,
        dotty_core::SymbolInfo::Missing
    ));
    let _ = add_tree;
    format!(
        "{class_facts}\tselectors=distinct-class-owned\tunapply=companion-owned\tformal=x$1\tsignature=uncompleted"
    )
}
