use std::path::PathBuf;

use dotty_classloader::classloader::{
    ClassPathEntry, ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath, JdkClassPath,
    LoadingSession,
};
use dotty_core::ast::TreeKind;
use dotty_core::types::{Type, TypeRefTarget};
use dotty_core::{
    Definitions, GeneratedCaseClassMethodRole, Packages, SemanticStore, SourceId, SourceText,
    SymbolFlags, SymbolKind, SymbolOrigin, TermName, TypeName,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_typer::{SourceTyper, TyperError};

const SOURCE: &str = include_str!("fixtures/source-sealed-adt-match-3.9.0/Event.scala");
const ROOTS: [&str; 2] = ["extract", "classify"];

#[test]
fn sealed_event_two_real_method_roots_record_first_error_movement() {
    let first = analyze_session(SourceId::from_index(1992));
    let second = analyze_session(SourceId::from_index(2992));
    assert_eq!(first, second, "fresh source sessions normalize identically");
    assert_eq!(first.len(), 2, "only the original two DefDef roots count");
    let actual = format!("{}\n", first.join("\n"));
    let expected =
        include_str!("fixtures/source-sealed-adt-match-3.9.0/rust-after-extractor-narrowing.tsv")
            .split_once('\n')
            .expect("post-narrowing receipt header")
            .1;
    assert_eq!(actual, expected);
    for row in first {
        println!("ISSUE1996_ROOT\t{row}");
    }
    let class_info = class_info_evidence(SourceId::from_index(3992));
    let expected_class_info =
        include_str!("fixtures/source-sealed-adt-match-3.9.0/rust-after-product1.tsv")
            .split_once('\n')
            .expect("class metadata header")
            .1;
    assert_eq!(format!("{}\n", class_info.join("\n")), expected_class_info);
    for row in class_info {
        println!("ISSUE1992_CLASSINFO\t{row}");
    }
}

fn class_info_evidence(source: SourceId) -> Vec<String> {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(SOURCE).expect("valid frozen source"),
        source,
        ContextualScanner::new(SOURCE).expect("scan frozen source"),
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Event.scala",
        &mut store,
        &mut packages,
    )
    .expect("name frozen Event source");
    let variants = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if matches!(
                    store.names.resolve(definition.name.as_name().text()),
                    "Created" | "Deleted"
                ) =>
            {
                let name = store
                    .names
                    .resolve(definition.name.as_name().text())
                    .to_owned();
                Some((name, index.symbol_at(source, tree).unwrap(), tree))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dotty-tasty/tests/fixtures/scala3-library");
    let java_home = std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required");
    let classpath = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(fixture)) as Box<dyn ClassPathEntry>,
        Box::new(JdkClassPath::new(PathBuf::from(java_home).join("jmods")).expect("JDK modules")),
    ]);
    let resolver = ClasspathSymbolResolver::new(classpath, definitions, LoadingSession::new());
    let event_name = TypeName::new(store.names.intern("Event"));
    let selector_name = TermName::new(store.names.intern("_1"));
    let created_module_class_name = TypeName::new(store.names.intern("Created$"));
    let deleted_module_class_name = TypeName::new(store.names.intern("Deleted$"));
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    let mut rows = Vec::new();
    let event_symbol = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if typer
                    .store()
                    .names
                    .resolve(definition.name.as_name().text())
                    == "Event" =>
            {
                index.symbol_at(source, tree)
            }
            _ => None,
        })
        .expect("canonical source Event identity");
    let event_package = typer.store().symbols.get(event_symbol).owner.unwrap();
    let event_package_scope = index.scope_of(event_package).expect("Event package scope");
    assert_eq!(
        typer
            .store()
            .scopes
            .get(event_package_scope)
            .lookup(event_name.as_name()),
        Some(event_symbol),
        "parent spelling resolves to the original source Event symbol"
    );
    for (name, class, class_tree) in variants {
        let TreeKind::TypeDef(definition) = &parsed.ast.get(class_tree).kind else {
            panic!("variant source tree is not a type definition");
        };
        let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
            panic!("variant source body is not a template");
        };
        let parent_symbols = template
            .parents
            .iter()
            .map(|parent| {
                let TreeKind::Ident(ident) = &parsed.ast.get(*parent).kind else {
                    panic!(
                        "expected simple source parent identifier: {:?}",
                        parsed.ast.get(*parent).kind
                    );
                };
                assert_eq!(ident.name, *event_name.as_name());
                format!("Event@tree{}=>source-symbol", parent.index())
            })
            .collect::<Vec<_>>();
        assert_eq!(template.parents.len(), 1);
        assert_eq!(
            typer
                .store()
                .scopes
                .get(event_package_scope)
                .lookup(event_name.as_name()),
            Some(event_symbol)
        );
        let class_info_id = typer
            .complete_symbol(class)
            .expect("complete variant ClassInfo");
        let Type::ClassInfo(class_info) = typer.store().types.get(class_info_id) else {
            panic!("{name} is not a ClassInfo");
        };
        let parents = class_info
            .parents
            .iter()
            .map(|parent| {
                let Type::TypeRef {
                    target: TypeRefTarget::Symbol(symbol),
                    ..
                } = typer.store().types.get(*parent)
                else {
                    panic!(
                        "{name} parent is not a symbol type: {:?}",
                        typer.store().types.get(*parent)
                    );
                };
                let symbol_info = typer.store().symbols.get(*symbol);
                let spelling = typer.store().names.resolve(symbol_info.name.text());
                let origin = match symbol_info.origin {
                    SymbolOrigin::Source(_) => "source",
                    SymbolOrigin::Tasty(_) => "tasty",
                    SymbolOrigin::Classfile(_) => "classfile",
                    SymbolOrigin::Builtin => "builtin",
                    SymbolOrigin::Synthetic => "synthetic",
                };
                format!("{spelling}:{:?}:{origin}", symbol_info.kind)
            })
            .collect::<Vec<_>>();
        let has_product_parent = class_info
            .parents
            .iter()
            .find_map(|parent| match typer.store().types.get(*parent) {
                Type::TypeRef {
                    target: TypeRefTarget::Symbol(symbol),
                    ..
                } if typer
                    .store()
                    .names
                    .resolve(typer.store().symbols.get(*symbol).name.text())
                    == "Product" =>
                {
                    Some(true)
                }
                _ => None,
            })
            .unwrap_or(false);
        assert!(
            parent_symbols
                .iter()
                .any(|parent| parent.starts_with("Event@tree")),
            "{name} retains its direct source Event parent"
        );

        let companion = typer.store().symbols.get(class).links.companion.unwrap();
        let package = typer.store().symbols.get(class).owner.unwrap();
        let package_scope = index.scope_of(package).expect("variant package scope");
        let module_class = typer
            .store()
            .scopes
            .get(package_scope)
            .lookup(if name == "Created" {
                created_module_class_name.as_name()
            } else {
                deleted_module_class_name.as_name()
            })
            .expect("generated companion module class");
        let unapply = index
            .generated_case_class_method(class, GeneratedCaseClassMethodRole::Unapply)
            .expect("generated source unapply identity");
        assert_eq!(typer.store().symbols.get(unapply).owner, Some(module_class));
        let unapply_type = typer
            .complete_symbol(unapply)
            .expect("generated unapply type");
        let Type::Method(unapply_method) = typer.store().types.get(unapply_type) else {
            panic!("{name}.unapply is not a MethodType");
        };
        assert_eq!(unapply_method.params.len(), 1);
        let symbol_of = |typer: &SourceTyper<'_>, ty| match typer.store().types.get(ty) {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => Some(*symbol),
            _ => None,
        };
        let unapply_input = unapply_method.params[0].ty;
        let unapply_result = unapply_method.result;
        assert_eq!(symbol_of(&typer, unapply_input), Some(class));
        assert_eq!(symbol_of(&typer, unapply_result), Some(class));
        let class_scope = index.scope_of(class).unwrap();
        let selector = typer
            .store()
            .scopes
            .get(class_scope)
            .lookup(selector_name.as_name())
            .expect("source case-class selector identity");
        let selector_state = match typer.complete_symbol(selector) {
            Ok(selector_type) => {
                let Type::Method(selector_method) = typer.store().types.get(selector_type) else {
                    panic!("{name}._1 is not a MethodType");
                };
                assert_eq!(selector_method.result, definitions.int);
                "_1:Int@class".to_owned()
            }
            Err(error) => {
                assert!(matches!(
                    error,
                    TyperError::DeclarationContextMissing { symbol: missing } if missing == selector
                ));
                format!("_1=DeclarationContextMissing({selector:?})")
            }
        };
        rows.push(format!(
            "{name}\tclass={name}:source-CASE:{class:?}\tsource_parent_edges={}\tclass_info_parents={}\tresolver_product_parent={}\tcompanion={name}$-link:{companion:?}\tmodule_class={name}$:{module_class:?}\tunapply=unapply:{unapply:?},owner={name}$,type=({name}):{name},canonical_arg={:?},canonical_result={:?}\tselector={selector_state}",
            parent_symbols.join(","), parents.join(","), has_product_parent,
            symbol_of(&typer, unapply_input), symbol_of(&typer, unapply_result),
        ));
    }
    rows
}

fn analyze_session(source: SourceId) -> Vec<String> {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(SOURCE).expect("valid frozen source"),
        source,
        ContextualScanner::new(SOURCE).expect("scan frozen source"),
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Event.scala",
        &mut store,
        &mut packages,
    )
    .expect("name frozen Event source");

    let mut event = None;
    let mut variants = Vec::new();
    let mut roots = Vec::new();
    for (tree, node) in parsed.ast.iter() {
        if let TreeKind::TypeDef(definition) = &node.kind {
            let spelling = store.names.resolve(definition.name.as_name().text());
            let symbol = index.symbol_at(source, tree).expect("source type symbol");
            if spelling == "Event" {
                event = Some((symbol, tree));
            } else if matches!(spelling, "Created" | "Deleted") {
                variants.push((spelling.to_owned(), symbol, tree));
            }
        }
        if let TreeKind::DefDef(definition) = &node.kind {
            let spelling = store.names.resolve(definition.name.as_name().text());
            if ROOTS.contains(&spelling) {
                let symbol = index.symbol_at(source, tree).expect("source method symbol");
                let owner = store.symbols.get(symbol).owner.expect("method owner");
                roots.push((
                    spelling.to_owned(),
                    tree,
                    node.position.unwrap(),
                    symbol,
                    owner,
                ));
            }
        }
    }
    let (event_symbol, event_tree) = event.expect("source Event trait");
    assert_eq!(store.symbols.get(event_symbol).kind, SymbolKind::Trait);
    assert!(
        store
            .symbols
            .get(event_symbol)
            .flags
            .contains(SymbolFlags::SEALED)
    );
    assert!(
        matches!(store.symbols.get(event_symbol).origin, SymbolOrigin::Source(id) if id == source)
    );
    variants.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(variants.len(), 2, "two distinct source variants");
    assert_ne!(variants[0].1, variants[1].1);
    for (name, symbol, _) in &variants {
        assert_eq!(store.symbols.get(*symbol).kind, SymbolKind::Class);
        assert!(store.symbols.get(*symbol).flags.contains(SymbolFlags::CASE));
        assert!(
            matches!(store.symbols.get(*symbol).origin, SymbolOrigin::Source(id) if id == source),
            "{name}"
        );
    }
    roots.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        roots.len(),
        2,
        "generated methods and CaseDefs are not roots"
    );
    assert_eq!(roots[0].4, roots[1].4, "both roots share Matcher$ owner");
    assert_eq!(
        store
            .names
            .resolve(store.symbols.get(roots[0].4).name.text()),
        "Matcher$"
    );

    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dotty-tasty/tests/fixtures/scala3-library");
    assert!(
        fixture.join("scala/Product.tasty").is_file(),
        "canonical Scala Product TASTy evidence"
    );
    let java_home = std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required");
    let classpath = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(fixture)) as Box<dyn ClassPathEntry>,
        Box::new(JdkClassPath::new(PathBuf::from(java_home).join("jmods")).expect("JDK modules")),
    ]);
    let resolver = ClasspathSymbolResolver::new(classpath, definitions, LoadingSession::new());
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));

    let mut rows = Vec::new();
    for (name, tree, position, method_symbol, owner) in roots {
        let outcome = match typer.type_method_definition(tree) {
            Ok(typed) => {
                assert_eq!(typer.source_typed_index().get(source, tree), Some(typed));
                assert!(matches!(
                    typer.typed_ast().get(typed).kind,
                    TreeKind::DefDef(_)
                ));
                format!("complete=typed#tree{}", typed.index())
            }
            Err(error) => {
                assert!(
                    typer.source_typed_index().get(source, tree).is_none(),
                    "failed methods have no typed root mapping"
                );
                let TyperError::ExtractorPatternConstraintDeferred {
                    tree_index,
                    selector,
                    input,
                    error: nested,
                    ..
                } = &error
                else {
                    panic!("unexpected first blocker for {name}: {error:?}");
                };
                let (error_tree, error_node) = parsed
                    .ast
                    .iter()
                    .find(|(candidate, _)| candidate.index() == *tree_index)
                    .expect("first blocker source tree");
                let error_range = error_node.position.unwrap().span().range();
                assert!(typer.store().types.try_get(*selector).is_none());
                assert!(typer.store().types.try_get(*input).is_none());
                let nested = if nested.is_some() { "present" } else { "none" };
                format!(
                    "first_error=ExtractorPatternConstraintDeferred(error_tree=Event.scala#tree{},error_bytes={}:{},selector_type=Event,extractor_input=rolled_back(Type),nested={nested});typed_index=absent",
                    error_tree.index(),
                    error_range.start(),
                    error_range.end(),
                )
            }
        };
        let range = position.span().range();
        let owner_tree = match index
            .definition_of(owner)
            .expect("Matcher$ source definition")
        {
            dotty_core::SourceDefinition::Canonical {
                source: owner_source,
                tree,
            }
            | dotty_core::SourceDefinition::Derived {
                source: owner_source,
                tree,
            } if owner_source == source => tree,
            other => panic!("unexpected Matcher$ source definition: {other:?}"),
        };
        rows.push(format!(
            "{name}\tpath=Event.scala#tree{}\tmethod_symbol={method_symbol:?}\tsource_bytes={}:{}\towner=Event.scala#tree{}(Matcher$):{owner:?}\tevent=Event.scala#tree{}(Trait,SEALED):{event_symbol:?}\tvariants=Created:CASE@tree{}:{:?},Deleted:CASE@tree{}:{:?}\t{outcome}",
            tree.index(), range.start(), range.end(), owner_tree.index(), event_tree.index(),
            variants.iter().find(|(variant, _, _)| variant == "Created").unwrap().2.index(),
            variants.iter().find(|(variant, _, _)| variant == "Created").unwrap().1,
            variants.iter().find(|(variant, _, _)| variant == "Deleted").unwrap().2.index(),
            variants.iter().find(|(variant, _, _)| variant == "Deleted").unwrap().1,
        ));
    }
    rows
}
