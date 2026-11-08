use super::*;
use dotty_classloader::classloader::{
    BinaryName, ClassFormat, ClassOrigin, ClassPathEntry, ClassPathError, ClassResource,
    ClasspathSymbolResolver, CompositeClassPath, JarClassPath, JdkClassPath, LoadingSession,
};
use std::{
    cell::RefCell,
    fs,
    path::PathBuf,
    process::Command,
    rc::Rc,
    sync::{Arc, Mutex},
};

struct OriginRecordingEntry<E> {
    inner: E,
    observations: Arc<Mutex<Vec<(String, ClassFormat, ClassOrigin)>>>,
}

impl<E: ClassPathEntry> ClassPathEntry for OriginRecordingEntry<E> {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        let resource = self.inner.find_class(name)?;
        if let Some(resource) = &resource {
            self.observations.lock().unwrap().push((
                name.as_internal().to_owned(),
                resource.format(),
                resource.origin().clone(),
            ));
        }
        Ok(resource)
    }

    fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        self.inner.contains_package(package)
    }

    fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        self.inner.contains_package_prefix(package)
    }
}

fn observed_symbol_path(store: &SemanticStore, symbol: SymbolId) -> String {
    let mut names = Vec::new();
    let mut next = Some(symbol);
    while let Some(current) = next {
        let info = store.symbols.get(current);
        names.push(store.names.resolve(info.name.text()).to_owned());
        next = info.owner;
    }
    names.reverse();
    names.join(".")
}

fn parent_base_name(
    store: &SemanticStore,
    ast: &AstArena<Untyped>,
    tree: TreeId<Untyped>,
) -> Option<String> {
    let node = ast.try_get(tree)?;
    match &node.kind {
        TreeKind::Ident(ident) => Some(store.names.resolve(ident.name.text()).to_owned()),
        TreeKind::Select(select) => Some(store.names.resolve(select.name.text()).to_owned()),
        TreeKind::AppliedTypeTree(applied) => parent_base_name(store, ast, applied.tpt),
        _ => None,
    }
}

fn type_symbols(
    store: &SemanticStore,
    ty: TypeId,
    output: &mut Vec<SymbolId>,
    seen: &mut Vec<TypeId>,
) {
    if seen.contains(&ty) {
        return;
    }
    seen.push(ty);
    match store.types.get(ty) {
        Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }
        | Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        }
        | Type::ThisType { class: symbol } => output.push(*symbol),
        Type::Applied { tycon, args } => {
            type_symbols(store, *tycon, output, seen);
            for arg in args {
                type_symbols(store, *arg, output, seen);
            }
        }
        Type::And { left, right } | Type::Or { left, right } => {
            type_symbols(store, *left, output, seen);
            type_symbols(store, *right, output, seen);
        }
        Type::SuperType {
            this_type,
            super_type,
        } => {
            type_symbols(store, *this_type, output, seen);
            type_symbols(store, *super_type, output, seen);
        }
        Type::Recursive { parent } => type_symbols(store, *parent, output, seen),
        _ => {}
    }
}

#[test]
#[ignore = "requires pinned Scala 3.9.0 sources, classpath jars, and JDK environment"]
fn captures_acceptance_iv_relation_operands_before_transaction_rollback() {
    let scala_root = PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT"));
    let revision = Command::new("git")
        .args(["-C", scala_root.to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .expect("git reads the pinned Scala source revision");
    assert!(revision.status.success());
    assert_eq!(
        String::from_utf8(revision.stdout).unwrap().trim(),
        "777528f19a58e794c9954a42f433373472ec57f8",
        "relation observation uses the pinned Scala 3.9.0 source"
    );
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME"));
    let release = std::env::var("SCALA39_JDK_RELEASE")
        .expect("SCALA39_JDK_RELEASE")
        .parse()
        .unwrap();
    let jars =
        std::env::split_paths(&std::env::var_os("SCALA39_CLASSPATH").expect("SCALA39_CLASSPATH"))
            .collect::<Vec<_>>();
    for (relative, wanted_tree) in [
        ("library/src/scala/Option.scala", 214),
        ("library/src/scala/Tuple.scala", 23),
    ] {
        let text = fs::read_to_string(scala_root.join(relative)).unwrap();
        let source = SourceId::from_index(0);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let scanner = ContextualScanner::new(&text).unwrap();
        let parsed = dotty_parser::parse_compilation_unit(
            SourceText::new(&text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            relative,
            &mut store,
            &mut packages,
        )
        .unwrap();
        let (tree, _) = parsed
            .ast
            .iter()
            .find(|(tree, node)| {
                tree.index() == wanted_tree && matches!(node.kind, TreeKind::DefDef(_))
            })
            .expect("selected original DefDef");
        let resource_origins = Arc::new(Mutex::new(Vec::new()));
        let mut entries: Vec<Box<dyn ClassPathEntry>> = vec![Box::new(OriginRecordingEntry {
            inner: JdkClassPath::new(java_home.join("jmods")).unwrap(),
            observations: resource_origins.clone(),
        })];
        for jar in &jars {
            entries.push(Box::new(OriginRecordingEntry {
                inner: JarClassPath::new(jar.clone(), release).unwrap(),
                observations: resource_origins.clone(),
            }));
        }
        let resolver = ClasspathSymbolResolver::new(
            CompositeClassPath::new(entries),
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
        let observed = Rc::new(RefCell::new(None::<String>));
        let capture = observed.clone();
        let parsed_ast = &parsed.ast;
        let observed_resource_origins = resource_origins.clone();
        typer.transaction_failure_error_observer = Some(Rc::new(move |typer, error| {
            let mut lines = vec![
                format!("root={relative}#tree={wanted_tree}"),
                format!("error={error:?}"),
            ];
            if let Some(method) = typer.index.symbol_at(typer.source, tree) {
                let info = typer.store.symbols.get(method);
                let owner = info.owner;
                lines.push(format!("canonical_source_method={method:?} path={} owner={owner:?} owner_path={} origin={:?} canonical={}",
                    observed_symbol_path(typer.store, method),
                    owner.map(|owner| observed_symbol_path(typer.store, owner)).unwrap_or_else(|| "unavailable".to_owned()),
                    info.origin,
                    matches!(typer.index.definition_of(method),
                        Some(SourceDefinition::Canonical { source, tree: definition_tree })
                            if source == typer.source && definition_tree == tree)));
            }
            let wanted_names: &[&str] = if relative.ends_with("Option.scala") {
                &["Option", "Some", "None"]
            } else {
                &["Tuple"]
            };
            let source_class_symbols = parsed_ast
                .iter()
                .filter_map(|(tree, node)| {
                    let TreeKind::TypeDef(definition) = &node.kind else {
                        return None;
                    };
                    let symbol = typer.index.symbol_at(typer.source, tree)?;
                    Some((
                        typer
                            .store
                            .names
                            .resolve(definition.name.as_name().text())
                            .to_owned(),
                        symbol,
                    ))
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            match error {
                TyperError::IfBranchJoinUnsupported {
                    left,
                    right,
                    error: relation,
                    ..
                } => {
                    assert!(matches!(
                        relation.as_ref(),
                        TypeRelationError::NonClassLikeSymbol {
                            kind: SymbolKind::Package,
                            ..
                        }
                    ));
                    assert!(matches!(
                        typer.store.types.get(*left),
                        Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
                            if observed_symbol_path(typer.store, *symbol).ends_with(".scala.None$")
                    ));
                    assert!(matches!(
                        typer.store.types.get(*right),
                        Type::Applied { tycon, .. }
                            if matches!(typer.store.types.get(*tycon), Type::TypeRef {
                                target: TypeRefTarget::Symbol(symbol), ..
                            } if observed_symbol_path(typer.store, *symbol).ends_with(".scala.Some"))
                    ));
                }
                TyperError::ApplicationArgumentConformanceUnsupported {
                    actual, expected, ..
                } => {
                    assert!(matches!(
                        typer.store.types.get(*actual),
                        Type::ThisType { class }
                            if observed_symbol_path(typer.store, *class).ends_with(".scala.Tuple")
                    ));
                    assert!(matches!(typer.store.types.get(*expected), Type::Error(_)));
                }
                other => panic!("unexpected acceptance IV failure: {other:?}"),
            }
            for (decl_tree, decl_node) in parsed_ast.iter() {
                let name_id = match &decl_node.kind {
                    TreeKind::TypeDef(definition) => definition.name.as_name().text(),
                    TreeKind::PhaseSpecific(UntypedNode::ModuleDef(definition)) => {
                        definition.name.as_name().text()
                    }
                    _ => continue,
                };
                let name = typer.store.names.resolve(name_id);
                if !wanted_names.contains(&name) {
                    continue;
                }
                let symbol = typer.index.symbol_at(typer.source, decl_tree);
                if let Some(symbol) = symbol {
                    let info = typer.store.symbols.get(symbol);
                    let canonical = matches!(typer.index.definition_of(symbol),
                        Some(SourceDefinition::Canonical { source, tree })
                            if source == typer.source && tree == decl_tree);
                    assert!(
                        canonical,
                        "selected parent/owner declaration is canonical: {name}"
                    );
                    lines.push(format!("canonical_source_class={symbol:?} path={} kind={:?} origin={:?} links={:?} canonical={canonical} info={:?}",
                        observed_symbol_path(typer.store, symbol), info.kind, info.origin, info.links, info.info));
                    let template_tree = match &decl_node.kind {
                        TreeKind::TypeDef(definition) => Some(definition.rhs),
                        TreeKind::PhaseSpecific(UntypedNode::ModuleDef(definition)) => {
                            Some(definition.template)
                        }
                        _ => None,
                    };
                    if let Some(template_tree) = template_tree
                        && let Some(template_node) = parsed_ast.try_get(template_tree)
                        && let TreeKind::Template(template) = &template_node.kind
                    {
                        for parent_tree in &template.parents {
                            let parent_symbol = typer.index.symbol_at(typer.source, *parent_tree);
                            let parent_shape =
                                parsed_ast.try_get(*parent_tree).map(|node| &node.kind);
                            let parent_name =
                                parent_base_name(typer.store, parsed_ast, *parent_tree);
                            let canonical_parent = parent_name
                                .as_ref()
                                .and_then(|name| source_class_symbols.get(name));
                            if matches!(name, "Some" | "None") {
                                assert_eq!(
                                    canonical_parent,
                                    source_class_symbols.get("Option"),
                                    "Option branch constructors retain the canonical source parent"
                                );
                            }
                            if name == "Some" {
                                let Some(parent_node) = parsed_ast.try_get(*parent_tree) else {
                                    panic!(
                                        "Some's original Option parent should remain in the AST"
                                    );
                                };
                                let TreeKind::AppliedTypeTree(applied) = &parent_node.kind else {
                                    panic!("Some's original parent should retain Option[T]");
                                };
                                assert_eq!(applied.args.len(), 1);
                                let Some(argument_node) = parsed_ast.try_get(applied.args[0])
                                else {
                                    panic!("Some's parent argument should remain in the AST");
                                };
                                let TreeKind::Ident(argument) = &argument_node.kind else {
                                    panic!("Some's parent argument should remain an identifier");
                                };
                                assert_eq!(
                                    typer.store.names.resolve(argument.name.text()),
                                    "A",
                                    "the original Scala Some[A] parent must be Option[A]"
                                );
                            }
                            lines.push(format!("canonical_declared_parent=child:{symbol:?} parent_tree:{parent_tree:?} parent_name:{parent_name:?} direct_tree_symbol:{parent_symbol:?} source_canonical_parent:{canonical_parent:?} parent_shape:{parent_shape:?}"));
                        }
                    }
                } else {
                    lines.push(format!(
                        "canonical_source_class={name} unavailable=no_source_symbol"
                    ));
                }
            }
            let operands = match error {
                TyperError::IfBranchJoinUnsupported { left, right, .. } => vec![*left, *right],
                TyperError::ApplicationArgumentConformanceUnsupported {
                    actual, expected, ..
                } => vec![*actual, *expected],
                _ => Vec::new(),
            };
            for operand in operands {
                lines.push(format!(
                    "relation_operand={operand:?} {:?}",
                    typer.store.types.get(operand)
                ));
                let mut pending = vec![operand];
                let mut seen = Vec::new();
                while let Some(type_id) = pending.pop() {
                    if seen.contains(&type_id) {
                        continue;
                    }
                    seen.push(type_id);
                    let semantic = typer.store.types.get(type_id);
                    lines.push(format!("relation_type_node={type_id:?} {semantic:?}"));
                    match semantic {
                        Type::TermRef { prefix, .. } | Type::TypeRef { prefix, .. } => {
                            pending.push(*prefix)
                        }
                        Type::Applied { tycon, args } => {
                            pending.push(*tycon);
                            pending.extend(args.iter().copied());
                        }
                        Type::And { left, right } | Type::Or { left, right } => {
                            pending.push(*left);
                            pending.push(*right);
                        }
                        Type::SuperType {
                            this_type,
                            super_type,
                        } => {
                            pending.push(*this_type);
                            pending.push(*super_type);
                        }
                        Type::Recursive { parent } => pending.push(*parent),
                        _ => {}
                    }
                }
                let mut symbols = Vec::new();
                type_symbols(typer.store, operand, &mut symbols, &mut Vec::new());
                symbols.sort_by_key(|symbol| symbol.index());
                symbols.dedup();
                for symbol in symbols {
                    let info = typer.store.symbols.get(symbol);
                    lines.push(format!(
                        "operand_symbol={symbol:?} path={} kind={:?} origin={:?} info={:?}",
                        observed_symbol_path(typer.store, symbol),
                        info.kind,
                        info.origin,
                        info.info
                    ));
                }
            }
            for (source_tree, source_node) in parsed_ast.iter() {
                if let Some(typed_tree) = typer.source_typed_index().get(typer.source, source_tree)
                {
                    let typed = typer.typed_ast().get(typed_tree);
                    if let Type::TermRef {
                        target: TermRefTarget::Symbol(symbol),
                        ..
                    } = typer.store.types.get(typed.ty)
                    {
                        let name = typer.store.names.resolve(match &source_node.kind {
                            TreeKind::Select(select) => select.name.text(),
                            _ => continue,
                        });
                        if name == "toArray" {
                            let info = typer.store.symbols.get(*symbol);
                            assert_eq!(
                                observed_symbol_path(typer.store, *symbol),
                                ".scala.runtime.Tuples$.toArray"
                            );
                            assert!(matches!(info.origin, SymbolOrigin::Tasty(_)));
                            lines.push(format!("resolved_member={symbol:?} path={} owner={:?} owner_path={} kind={:?} origin={:?} info={:?}",
                                observed_symbol_path(typer.store, *symbol), info.owner,
                                info.owner.map(|owner| observed_symbol_path(typer.store, owner)).unwrap_or_else(|| "unavailable".to_owned()),
                                info.kind, info.origin, info.info));
                            if let SymbolInfo::Complete(signature) = info.info {
                                lines.push(format!(
                                    "resolved_member_signature={signature:?} {:?}",
                                    typer.store.types.get(signature)
                                ));
                            }
                            if let Some(owner) = info.owner {
                                let owner_info = typer.store.symbols.get(owner);
                                lines.push(format!("resolved_member_owner={owner:?} path={} kind={:?} origin={:?} info={:?}",
                                    observed_symbol_path(typer.store, owner), owner_info.kind, owner_info.origin, owner_info.info));
                            }
                        }
                    }
                }
            }
            if relative.ends_with("Tuple.scala") {
                let matching_origins = observed_resource_origins
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(name, _, _)| name.contains("Tuples"))
                    .cloned()
                    .collect::<Vec<_>>();
                assert!(
                    matching_origins
                        .iter()
                        .any(|(_, format, origin)| *format == ClassFormat::Tasty
                            && matches!(origin, ClassOrigin::Jar(path)
                                if path.ends_with("org/scala-lang/scala-library/3.9.0/scala-library-3.9.0.jar"))),
                    "the resolver's Tuples$ TASTy load carries its classpath resource origin; observed={matching_origins:?}"
                );
                for (name, format, origin) in matching_origins {
                    let origin_suffix = match origin {
                        ClassOrigin::Jar(path) => path
                            .iter()
                            .rev()
                            .take(5)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect::<PathBuf>(),
                        other => PathBuf::from(format!("{other:?}")),
                    };
                    lines.push(format!(
                        "resolver_resource={name} format={format:?} origin_suffix={}",
                        origin_suffix.display()
                    ));
                }
            }
            if let TyperError::ApplicationArgumentConformanceUnsupported { expected, .. } = error
                && let Type::Error(error_type) = typer.store.types.get(*expected)
            {
                let message = typer.store.names.resolve(error_type.message);
                assert!(message.contains("cannot establish canonical identity for scala/Tuple"));
                lines.push(format!("expected_error_message={}", message));
            }
            *capture.borrow_mut() = Some(lines.join("\n"));
        }));
        let error = typer
            .type_method_definition(tree)
            .expect_err("the original full DefDef remains blocked");
        assert!(
            observed.borrow().is_some(),
            "observer runs before rollback: {error:?}"
        );
        println!(
            "ACCEPTANCE_IV_RELATION_OBSERVATION_BEGIN\n{}\nACCEPTANCE_IV_RELATION_OBSERVATION_END",
            observed.borrow().as_deref().unwrap()
        );
    }
}
