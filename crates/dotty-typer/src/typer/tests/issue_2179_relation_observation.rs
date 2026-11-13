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

fn replay_original_either_swap_patterns() -> Vec<String> {
    let scala_root = PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT"));
    let relative = "library/src/scala/util/Either.scala";
    let text = fs::read_to_string(scala_root.join(relative)).unwrap();
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let scanner = ContextualScanner::new(&text).unwrap();
    let mut parsed = dotty_parser::parse_compilation_unit(
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
    let method_tree = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            (tree.index() == 84 && matches!(node.kind, TreeKind::DefDef(_))).then_some(tree)
        })
        .unwrap();
    let method = index.symbol_at(source, method_tree).unwrap();
    let TreeKind::DefDef(definition) = &parsed.ast.get(method_tree).kind else {
        unreachable!()
    };
    let TreeKind::Match(matched) = &parsed.ast.get(definition.rhs.unwrap()).kind else {
        unreachable!()
    };
    let selector_tree = matched.selector;
    let original_cases = matched.cases.clone();
    let mut isolated_cases = Vec::new();
    // Preserve both original cases and bodies so the application subtrees
    // prove Right(a) and Left(b) against their canonical generated methods.
    for case_tree in original_cases {
        let TreeKind::CaseDef(original_case) = parsed.ast.get(case_tree).kind else {
            unreachable!()
        };
        let TreeKind::Apply(original_pattern) = &parsed.ast.get(original_case.pattern).kind else {
            panic!("original Option extractor is an Apply")
        };
        let binder_tree = original_pattern.args[0];
        let case_position = parsed.ast.get(case_tree).position;
        let isolated_case = parsed.ast.alloc(Tree {
            kind: TreeKind::CaseDef(dotty_core::ast::CaseDef {
                pattern: original_case.pattern,
                guard: None,
                body: original_case.body,
            }),
            position: case_position,
            ty: (),
        });
        isolated_cases.push((
            case_tree,
            original_case.pattern,
            binder_tree,
            original_case.body,
            isolated_case,
        ));
    }
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME"));
    let release = std::env::var("SCALA39_JDK_RELEASE")
        .expect("SCALA39_JDK_RELEASE")
        .parse()
        .unwrap();
    assert_eq!(release, 21);
    let jars =
        std::env::split_paths(&std::env::var_os("SCALA39_CLASSPATH").expect("SCALA39_CLASSPATH"))
            .collect::<Vec<_>>();
    let mut entries: Vec<Box<dyn ClassPathEntry>> = vec![Box::new(
        JdkClassPath::new(java_home.join("jmods")).unwrap(),
    )];
    for jar in jars {
        entries.push(Box::new(JarClassPath::new(jar, release).unwrap()));
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
    let context = typer.expression_context_for(method).unwrap();
    let typed_selector = typer
        .run_expression_transaction(|typer, journal, mappings| {
            typer.type_value_expression_inner(selector_tree, context, journal, mappings)
        })
        .unwrap();
    let selector_value_type = typer.typed_arena.get(typed_selector).ty;
    let selector_receiver_type = typer.this_type_receiver_view(selector_value_type).unwrap();
    let selector_type = typer
        .pattern_selector_type(selector_receiver_type, &mut Vec::new())
        .unwrap();
    let selector_parameters = match typer.store.types.get(selector_type) {
        Type::Applied { args, .. } => args.clone(),
        other => panic!("pattern selector is applied Either: {other:?}"),
    };
    let either_class = index
        .definition_of(method)
        .and_then(|definition| match definition {
            SourceDefinition::Canonical { .. } => typer.store.symbols.get(method).owner,
            _ => None,
        })
        .expect("canonical Either source class");
    assert!(
        matches!(
            typer.store.types.get(selector_value_type),
            Type::ThisType { class } if *class == either_class
        ),
        "the independently typed selector is the original this: Either[A, B]"
    );
    let either_parameters = typer.source_class_type_parameters(either_class).unwrap();
    let selector_parameter_symbols = selector_parameters
        .iter()
        .map(|ty| match typer.store.types.get(*ty) {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("selector preserves canonical Either parameter: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(selector_parameter_symbols, either_parameters);
    let mut normalized = Vec::new();
    let mut canonical_binders = Vec::new();
    for (case_tree, original_pattern, binder_tree, original_body, isolated_case) in isolated_cases {
        let typed_case = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_case_def(isolated_case, selector_type, context, journal, mappings)
            })
            .unwrap_or_else(|error| {
                panic!(
                    "isolated original pattern in case tree {} types: {error:?}",
                    case_tree.index()
                )
            });
        let TreeKind::CaseDef(typed_case_node) = &typer.typed_arena.get(typed_case).kind else {
            unreachable!()
        };
        let typed_pattern = typed_case_node.pattern;
        assert_eq!(
            typer.typed_index.get(source, original_pattern),
            Some(typed_pattern)
        );
        let TreeKind::UnApply(unapply) = &typer.typed_arena.get(typed_pattern).kind else {
            panic!("original pattern lowers to UnApply")
        };
        assert_eq!(unapply.patterns.len(), 1);
        assert!(typer.typed_index.get(source, binder_tree).is_some());
        let Type::Applied { tycon, args } = typer
            .store
            .types
            .get(typer.typed_arena.get(typed_pattern).ty)
        else {
            panic!("applied source variant prototype")
        };
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(variant),
            ..
        } = typer.store.types.get(*tycon)
        else {
            panic!("canonical variant type reference")
        };
        assert_eq!(args, &selector_parameters);
        let Type::Method(instantiated_unapply) = typer
            .store
            .types
            .get(typer.typed_arena.get(unapply.function).ty)
        else {
            panic!("instantiated source unapply is a method")
        };
        let Type::Applied {
            args: option_args, ..
        } = typer.store.types.get(instantiated_unapply.result)
        else {
            panic!("compatibility-mode unapply returns Option[A] or Option[B]")
        };
        let binder = typer
            .pattern_binding_symbol_at(source, binder_tree)
            .expect("original source binder has case-local identity");
        canonical_binders.push(binder);
        let binder_type = match typer.store.symbols.info(binder) {
            SymbolInfo::Complete(ty) => *ty,
            other => panic!("original binder type is complete: {other:?}"),
        };
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(parameter),
            ..
        } = typer.store.types.get(binder_type)
        else {
            panic!("original binder has a canonical Either parameter type")
        };
        let parameter_ordinal = selector_parameter_symbols
            .iter()
            .position(|candidate| candidate == parameter)
            .expect("binder type is one of Either's source parameters");
        assert_eq!(binder_type, selector_parameters[parameter_ordinal]);
        assert_eq!(option_args, &[selector_parameters[parameter_ordinal]]);
        let parameter_name = typer
            .store
            .names
            .resolve(typer.store.symbols.get(*parameter).name.text())
            .to_owned();
        let TreeKind::Apply(source_application) = &parsed.ast.get(original_pattern).kind else {
            unreachable!()
        };
        let TreeKind::Ident(source_variant) = &parsed.ast.get(source_application.function).kind
        else {
            panic!("original source extractor has a canonical identifier")
        };
        let source_variant_name = typer
            .store
            .names
            .resolve(source_variant.name.text())
            .to_owned();
        let source_variant_class = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if typer.store.names.resolve(definition.name.as_name().text())
                        == source_variant_name =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .expect("original source identifier resolves to its canonical class declaration");
        assert_eq!(source_variant_class, *variant);
        let binder_name = typer
            .store
            .names
            .resolve(typer.store.symbols.get(binder).name.text())
            .to_owned();
        match binder_name.as_str() {
            "a" => {
                assert_eq!(source_variant_name, "Left");
                assert_eq!(parameter_name, "A");
                assert_eq!(parameter_ordinal, 0);
            }
            "b" => {
                assert_eq!(source_variant_name, "Right");
                assert_eq!(parameter_name, "B");
                assert_eq!(parameter_ordinal, 1);
            }
            other => panic!("original Either binder is a or b: {other}"),
        }
        let TreeKind::CaseDef(typed_case_def) = &typer.typed_arena.get(typed_case).kind else {
            unreachable!()
        };
        assert_eq!(
            typer.typed_index.get(source, original_body),
            Some(typed_case_def.body),
            "the authentic source case body retains its typed mapping"
        );
        let TreeKind::Block(original_body_block) = &parsed.ast.get(original_body).kind else {
            panic!("original Either case body retains its source Block")
        };
        assert!(original_body_block.stats.is_empty());
        let original_application = original_body_block.expr;
        let typed_application = typer
            .typed_index
            .get(source, original_application)
            .expect("authentic Right(a) or Left(b) application is typed");
        let TreeKind::Block(typed_body_block) = &typer.typed_arena.get(typed_case_def.body).kind
        else {
            panic!("original case body retains its typed Block")
        };
        assert_eq!(
            typed_body_block.expr, typed_application,
            "the typed Block maps to the authentic original application"
        );
        let TreeKind::Apply(typed_application_node) =
            &typer.typed_arena.get(typed_application).kind
        else {
            panic!("authentic Right(a) or Left(b) remains an Apply")
        };
        let body_name = if binder_name == "a" { "Right" } else { "Left" };
        let body_class = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if typer.store.names.resolve(definition.name.as_name().text()) == body_name =>
                {
                    typer.index.symbol_at(source, tree)
                }
                _ => None,
            })
            .expect("original generated application class has canonical source identity");
        let body_apply = typer
            .index
            .generated_case_class_method(
                body_class,
                dotty_core::GeneratedCaseClassMethodRole::Apply,
            )
            .expect("original case body selects the canonical generated Apply role");
        let typed_function = typer.typed_arena.get(typed_application_node.function);
        assert!(matches!(
            typer.store.types.get(typed_function.ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == body_apply
        ));
        let Type::Applied { tycon, args } = typer
            .store
            .types
            .get(typer.typed_arena.get(typed_application).ty)
        else {
            panic!("original generated Apply preserves its applied case-class result")
        };
        assert!(matches!(
            typer.store.types.get(*tycon),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == body_class
        ));
        let (unused_index, concrete_index) = if body_name == "Right" { (0, 1) } else { (1, 0) };
        assert_eq!(args[unused_index], definitions.nothing_type);
        assert!(matches!(
            typer.store.types.get(args[concrete_index]),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
                if *symbol == selector_parameter_symbols[parameter_ordinal]
        ));
        assert!(matches!(
            &typer.typed_arena.get(unapply.function).kind,
            TreeKind::TypeApply(_)
        ));
        let unapply_symbol = index
            .generated_case_class_method(
                *variant,
                dotty_core::source_semantics::GeneratedCaseClassMethodRole::Unapply,
            )
            .expect("canonical generated unapply for the matched source class");
        let TreeKind::TypeApply(type_apply) = &typer.typed_arena.get(unapply.function).kind else {
            panic!("generic unapply call retains its source type application")
        };
        let referenced_unapply = match typer
            .store
            .types
            .get(typer.typed_arena.get(type_apply.function).ty)
        {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("typed TypeApply references a canonical unapply symbol: {other:?}"),
        };
        assert_eq!(referenced_unapply, unapply_symbol);
        assert!(matches!(
            index.generated_case_class_method_identity(unapply_symbol),
            Some(dotty_core::source_semantics::GeneratedCaseClassMethod {
                role: dotty_core::source_semantics::GeneratedCaseClassMethodRole::Unapply,
                ..
            })
        ));
        let field_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if index.symbol_at(source, tree) == Some(*variant) =>
                {
                    let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
                        return None;
                    };
                    let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind
                    else {
                        return None;
                    };
                    constructor.value_param_clauses.first()?.first().copied()
                }
                _ => None,
            })
            .expect("canonical source case-class value field");
        let field_symbol = index
            .symbol_at(source, field_tree)
            .expect("source case-class field has canonical semantic identity");
        assert_eq!(typer.store.symbols.get(field_symbol).owner, Some(*variant));
        let field_name = typer
            .store
            .names
            .resolve(typer.store.symbols.get(field_symbol).name.text())
            .to_owned();
        assert_eq!(field_name, "value");
        normalized.push(format!(
            "case:{}:pattern:{}:binder:{}:{}:source-class:{}:typed-class:{}:field:{}:unapply:{}:Either-param-{}:{}:application:{}:apply:{}:result={:?}",
            case_tree.index(),
            original_pattern.index(),
            binder_tree.index(),
            binder_name,
            observed_symbol_path(typer.store, source_variant_class),
            observed_symbol_path(typer.store, *variant),
            observed_symbol_path(typer.store, field_symbol),
            observed_symbol_path(typer.store, unapply_symbol),
            parameter_ordinal,
            parameter_name,
            body_name,
            observed_symbol_path(typer.store, body_apply),
            args,
        ));
    }
    assert_eq!(canonical_binders.len(), 2);
    normalized
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

fn normalized_type(store: &SemanticStore, ty: TypeId, method_binder: TypeId) -> String {
    match store.types.get(ty) {
        Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }
        | Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => observed_symbol_path(store, *symbol),
        Type::TermRef {
            target: TermRefTarget::Name(name),
            ..
        } => store.names.resolve(name.as_name().text()).to_owned(),
        Type::TypeRef {
            target: TypeRefTarget::Name(name),
            ..
        } => store.names.resolve(name.as_name().text()).to_owned(),
        Type::Applied { tycon, args } => format!(
            "{}[{}]",
            normalized_type(store, *tycon, method_binder),
            args.iter()
                .map(|arg| normalized_type(store, *arg, method_binder))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Type::ParamRef { binder, index } => {
            assert_eq!(
                *binder, method_binder,
                "parameter ref belongs to Option.map[B]"
            );
            format!("B#{index}")
        }
        Type::Or { left, right } => {
            let mut operands = [
                normalized_type(store, *left, method_binder),
                normalized_type(store, *right, method_binder),
            ];
            operands.sort();
            format!("{}|{}", operands[0], operands[1])
        }
        Type::ThisType { class } => format!("{}.this", observed_symbol_path(store, *class)),
        other => panic!("unexpected Option.map receipt type shape: {other:?}"),
    }
}

#[test]
#[ignore = "requires pinned Scala 3.9.0 sources, classpath jars, and JDK environment"]
fn preserves_option_map_rollback_and_tuple_to_array_automatic_preparation() {
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
                TyperError::SymbolResolution {
                    error: dotty_core::resolution::ResolutionError::Malformed { reason },
                    ..
                } if reason == "test-injected failure after successful source typing" => {}
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
                TyperError::ExpectedExpressionConformanceUnsupported {
                    source,
                    tree_index,
                    actual,
                    error: relation,
                    ..
                } => {
                    assert_eq!(*source, typer.source);
                    assert_eq!(*tree_index, 213);
                    assert!(matches!(
                        relation.as_ref(),
                        TypeRelationError::ClassInfoUnavailable {
                            symbol,
                            state: crate::SymbolInfoState::Missing,
                        } if observed_symbol_path(typer.store, *symbol).ends_with(".scala.None$")
                    ));
                    let Type::Or { left, right } = typer.store.types.get(*actual) else {
                        panic!(
                            "post-prefix-relation actual operand should retain the branch join: {:?}",
                            typer.store.types.get(*actual)
                        );
                    };
                    let is_none_branch = |ty: TypeId| {
                        matches!(
                            typer.store.types.get(ty),
                            Type::TypeRef {
                                target: TypeRefTarget::Symbol(symbol),
                                ..
                            } if observed_symbol_path(typer.store, *symbol).ends_with(".scala.None$")
                        )
                    };
                    let is_some_branch = |ty: TypeId| {
                        matches!(
                            typer.store.types.get(ty),
                            Type::Applied { tycon, .. }
                                if matches!(typer.store.types.get(*tycon), Type::TypeRef {
                                    target: TypeRefTarget::Symbol(symbol),
                                    ..
                                } if observed_symbol_path(typer.store, *symbol).ends_with(".scala.Some"))
                        )
                    };
                    assert!(
                        (is_none_branch(*left) && is_some_branch(*right))
                            || (is_none_branch(*right) && is_some_branch(*left)),
                        "the joined operand should retain canonical None$ and Some[B] branches"
                    );
                    let (none_branch, some_branch) = if is_none_branch(*left) {
                        (*left, *right)
                    } else {
                        (*right, *left)
                    };
                    let Type::TypeRef {
                        prefix: none_prefix,
                        target: TypeRefTarget::Symbol(none_class),
                    } = typer.store.types.get(none_branch)
                    else {
                        unreachable!("the observed None$ branch was checked above")
                    };
                    let Type::TypeRef {
                        prefix: none_package_prefix,
                        target: TypeRefTarget::Symbol(none_package),
                    } = typer.store.types.get(*none_prefix)
                    else {
                        panic!("None$ should retain its package TypeRef prefix")
                    };
                    assert!(matches!(
                        typer.store.types.get(*none_package_prefix),
                        Type::NoPrefix
                    ));
                    let Type::Applied {
                        tycon: some_tycon, ..
                    } = typer.store.types.get(some_branch)
                    else {
                        unreachable!("the observed Some[B] branch was checked above")
                    };
                    let Type::TypeRef {
                        prefix: some_prefix,
                        target: TypeRefTarget::Symbol(some_class),
                    } = typer.store.types.get(*some_tycon)
                    else {
                        panic!("Some[B] should retain its class TypeRef prefix")
                    };
                    let Type::ThisType {
                        class: some_package,
                    } = typer.store.types.get(*some_prefix)
                    else {
                        panic!("Some should retain its package ThisType prefix")
                    };
                    assert_eq!(none_package, some_package);
                    assert_eq!(
                        typer.store.symbols.get(*none_class).owner,
                        Some(*none_package)
                    );
                    assert_eq!(
                        typer.store.symbols.get(*some_class).owner,
                        Some(*some_package)
                    );
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
                TyperError::ExpectedExpressionConformanceUnsupported {
                    actual, expected, ..
                } => vec![*actual, *expected],
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
        let store_before_method = typer.store().checkpoint();
        let typed_before_method = typer.typed_ast().checkpoint();
        let type_index_before_method = format!("{:?}", typer.type_index.checkpoint());
        let typed_index_before_method = format!("{:?}", typer.typed_index.clone());
        let source_typed_index_before_method = format!("{:?}", typer.source_typed_index());
        let rollback_observed = Rc::new(RefCell::new(false));
        let rollback_observed_by_callback = rollback_observed.clone();
        if relative.ends_with("Option.scala") {
            typer.transaction_failure_observer = Some(Rc::new(move |typer| {
                *rollback_observed_by_callback.borrow_mut() =
                    typer.source_typed_index().get(source, tree).is_some();
            }));
            typer.inject_failure_after_outermost_success = true;
        }
        let mut result = typer.type_method_definition(tree);
        if relative.ends_with("Option.scala") {
            assert!(matches!(
                result,
                Err(TyperError::SymbolResolution {
                    source: failure_source,
                    error: ResolutionError::Malformed { ref reason },
                    ..
                }) if failure_source == source
                    && reason == "test-injected failure after successful source typing"
            ));
            assert!(*rollback_observed.borrow());
            assert_eq!(typer.store().checkpoint(), store_before_method);
            assert_eq!(typer.typed_ast().checkpoint(), typed_before_method);
            assert_eq!(
                format!("{:?}", typer.type_index.checkpoint()),
                type_index_before_method
            );
            assert_eq!(
                format!("{:?}", typer.typed_index.clone()),
                typed_index_before_method
            );
            assert_eq!(
                format!("{:?}", typer.source_typed_index()),
                source_typed_index_before_method
            );
            assert_eq!(typer.source_typed_index().get(source, tree), None);
            assert!(!typer.inject_failure_after_outermost_success);
            typer.transaction_failure_observer = None;
            typer.transaction_failure_error_observer = None;
            *observed.borrow_mut() = None;
            result = typer.type_method_definition(tree);
            let typed_method = result.expect("the original full Option.map DefDef should type");
            assert!(
                observed.borrow().is_none(),
                "successful typing must not use the failure observer"
            );
            assert_eq!(
                typer.source_typed_index().get(source, tree),
                Some(typed_method)
            );
            let method_symbol = index.symbol_at(source, tree).unwrap();
            let source_binder = match &parsed_ast.get(tree).kind {
                TreeKind::DefDef(definition) => index
                    .symbol_at(source, definition.type_params[0])
                    .expect("original Option.map B binder has a canonical symbol"),
                _ => unreachable!("selected source tree is Option.map DefDef"),
            };
            assert_eq!(
                typer.store().symbols.get(source_binder).owner,
                Some(method_symbol)
            );
            assert_eq!(
                typer
                    .store()
                    .names
                    .resolve(typer.store().symbols.get(source_binder).name.text()),
                "B"
            );
            let owner = typer.store().symbols.get(method_symbol).owner.unwrap();
            assert_eq!(observed_symbol_path(typer.store(), owner), ".scala.Option");
            assert!(matches!(index.definition_of(method_symbol),
                Some(SourceDefinition::Canonical { source: actual_source, tree: actual_tree })
                    if actual_source == source && actual_tree == tree));
            let SymbolInfo::Complete(signature) = *typer.store().symbols.info(method_symbol) else {
                panic!("typed Option.map should retain its complete source signature");
            };
            let Type::Poly(poly) = typer.store().types.get(signature) else {
                panic!("Option.map should retain its polymorphic method binder");
            };
            assert_eq!(poly.params.len(), 1);
            assert_eq!(
                typer
                    .store()
                    .names
                    .resolve(poly.params[0].name.as_name().text()),
                "B"
            );
            let source_classes = parsed_ast
                .iter()
                .filter_map(|(decl_tree, node)| match &node.kind {
                    TreeKind::TypeDef(definition) => Some((
                        typer
                            .store()
                            .names
                            .resolve(definition.name.as_name().text())
                            .to_owned(),
                        typer.index.symbol_at(typer.source, decl_tree)?,
                    )),
                    _ => None,
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            let option = source_classes["Option"];
            let some = source_classes["Some"];
            let none_tree = parsed_ast
                .iter()
                .find_map(|(decl_tree, node)| match &node.kind {
                    TreeKind::PhaseSpecific(UntypedNode::ModuleDef(definition))
                        if typer
                            .store()
                            .names
                            .resolve(definition.name.as_name().text())
                            == "None" =>
                    {
                        Some(decl_tree)
                    }
                    _ => None,
                })
                .expect("canonical source None declaration");
            let none = typer.index.symbol_at(source, none_tree).unwrap();
            let none_module_class = typer
                .index
                .derived_symbol_at(
                    typer.store().symbols.get(none).owner.unwrap(),
                    source,
                    none_tree,
                )
                .unwrap();
            let Type::Method(method_type) = typer.store().types.get(poly.result) else {
                panic!("Option.map must preserve its original function method clause");
            };
            assert_eq!(method_type.params.len(), 1);
            assert_eq!(
                typer
                    .store()
                    .names
                    .resolve(method_type.params[0].name.as_name().text()),
                "f"
            );
            let source_f = match &parsed_ast.get(tree).kind {
                TreeKind::DefDef(definition) => index
                    .symbol_at(source, definition.value_param_clauses[0][0])
                    .expect("original Option.map function parameter is canonical"),
                _ => unreachable!("selected source tree is Option.map DefDef"),
            };
            let Type::Applied {
                tycon: function_tycon,
                args: function_args,
            } = typer.store().types.get(method_type.params[0].ty)
            else {
                panic!("Option.map f must retain its Function1[A, B] type");
            };
            assert_eq!(function_args.len(), 2);
            let mut function_heads = Vec::new();
            type_symbols(
                typer.store(),
                *function_tycon,
                &mut function_heads,
                &mut Vec::new(),
            );
            assert!(function_heads.iter().any(|symbol| {
                observed_symbol_path(typer.store(), *symbol) == ".scala.Function1"
            }));
            let mut a_symbols = Vec::new();
            type_symbols(
                typer.store(),
                function_args[0],
                &mut a_symbols,
                &mut Vec::new(),
            );
            assert!(a_symbols.iter().any(|symbol| {
                let info = typer.store().symbols.get(*symbol);
                info.owner == Some(option) && typer.store().names.resolve(info.name.text()) == "A"
            }));
            assert!(
                matches!(
                    typer.store().types.get(function_args[1]),
                    Type::TypeRef {
                        target: TypeRefTarget::Symbol(symbol),
                        ..
                    } if *symbol == source_binder
                ) || matches!(
                    typer.store().types.get(function_args[1]),
                    Type::ParamRef { binder, index: 0 } if *binder == signature
                )
            );
            let Type::Applied {
                tycon,
                args: result_args,
            } = typer.store().types.get(method_type.result)
            else {
                panic!("Option.map must retain its applied Option result");
            };
            let mut result_symbols = Vec::new();
            type_symbols(typer.store(), *tycon, &mut result_symbols, &mut Vec::new());
            assert!(result_symbols.contains(&option));
            assert_eq!(result_args.len(), 1);
            assert!(matches!(
                typer.store().types.get(result_args[0]),
                Type::ParamRef { binder, index: 0 } if *binder == signature
            ));
            let typed_root = typer.typed_ast().get(typed_method);
            let TreeKind::DefDef(typed_definition) = &typed_root.kind else {
                panic!("the source root must publish a complete typed DefDef");
            };
            let typed_rhs = typed_definition
                .rhs
                .expect("the original Option.map DefDef retains its RHS");
            let source_rhs = match &parsed_ast.get(tree).kind {
                TreeKind::DefDef(definition) => definition
                    .rhs
                    .expect("original Option.map has its source RHS"),
                _ => unreachable!("selected source tree is Option.map DefDef"),
            };
            assert_eq!(
                typer.source_typed_index().get(source, source_rhs),
                Some(typed_rhs),
                "the typed RHS must be the exact mapping of the original DefDef RHS"
            );
            let TreeKind::If(source_if) = &parsed_ast.get(source_rhs).kind else {
                panic!("the original Option.map RHS must remain its source conditional");
            };
            let TreeKind::If(typed_if) = &typer.typed_ast().get(typed_rhs).kind else {
                panic!("the original Option.map conditional must remain typed");
            };
            assert_eq!(
                typer
                    .source_typed_index()
                    .get(source, source_if.else_branch),
                Some(typed_if.else_branch),
                "the typed Some branch must map the original source branch"
            );
            let TreeKind::Apply(source_some_call) = &parsed_ast.get(source_if.else_branch).kind
            else {
                panic!("the original Some branch must be an application");
            };
            assert_eq!(source_some_call.args.len(), 1);
            let source_function_call = source_some_call.args[0];
            let Type::Or { left, right } =
                typer.store().types.get(typer.typed_ast().get(typed_rhs).ty)
            else {
                panic!("the original None/Some branch join must retain its union shape");
            };
            let union_type = typer.typed_ast().get(typed_rhs).ty;
            let mut union_operands = [
                normalized_type(typer.store(), *left, signature),
                normalized_type(typer.store(), *right, signature),
            ];
            union_operands.sort();
            assert_eq!(
                union_operands,
                [".scala.None$", ".scala.Some[.scala.Option.map.B]"],
                "the union must contain the canonical None$ and Some[B] operands"
            );
            assert!([*left, *right].into_iter().any(|operand| matches!(
                typer.store().types.get(operand),
                Type::TypeRef {
                    target: TypeRefTarget::Symbol(symbol),
                    ..
                } if *symbol == none_module_class
            )));
            assert!([*left, *right].into_iter().any(|operand| {
                let Type::Applied { tycon, args } = typer.store().types.get(operand) else {
                    return false;
                };
                let mut symbols = Vec::new();
                type_symbols(typer.store(), *tycon, &mut symbols, &mut Vec::new());
                symbols.contains(&some)
                    && args.len() == 1
                    && matches!(
                        typer.store().types.get(args[0]),
                        Type::TypeRef {
                            target: TypeRefTarget::Symbol(symbol),
                            ..
                        } if *symbol == source_binder
                    )
            }));
            let TreeKind::Apply(some_call) = &typer.typed_ast().get(typed_if.else_branch).kind
            else {
                panic!("the mapped Some branch must retain its reachable application");
            };
            assert_eq!(some_call.args.len(), 1);
            let TreeKind::Select(some_function) = &typer.typed_ast().get(some_call.function).kind
            else {
                panic!("Some.apply selection is missing from the mapped branch");
            };
            assert_eq!(
                typer.store().names.resolve(some_function.name.text()),
                "apply"
            );
            let Type::TermRef {
                target: TermRefTarget::Symbol(some_apply),
                ..
            } = typer
                .store()
                .types
                .get(typer.typed_ast().get(some_call.function).ty)
            else {
                panic!("Some.apply selection must retain its canonical symbol");
            };
            assert_eq!(
                observed_symbol_path(typer.store(), *some_apply),
                ".scala.Some$.apply"
            );
            let function_call_id = some_call.args[0];
            assert_eq!(
                typer.source_typed_index().get(source, source_function_call),
                Some(function_call_id),
                "the nested typed call must map the original f(this.get) source call"
            );
            let TreeKind::Apply(source_f_call) = &parsed_ast.get(source_function_call).kind else {
                panic!("the original branch argument must be f(this.get)");
            };
            assert_eq!(source_f_call.args.len(), 1);
            let source_get_argument = source_f_call.args[0];
            let TreeKind::Apply(function_call) = &typer.typed_ast().get(function_call_id).kind
            else {
                panic!("Some.apply must receive the original f(this.get) application");
            };
            assert_eq!(function_call.args.len(), 1);
            assert_eq!(
                typer.source_typed_index().get(source, source_get_argument),
                Some(function_call.args[0]),
                "the typed argument must map the original this.get source tree"
            );
            assert!(matches!(
                parsed_ast.get(source_get_argument).kind,
                TreeKind::Select(_)
            ));
            let TreeKind::Select(function_apply) =
                &typer.typed_ast().get(function_call.function).kind
            else {
                panic!("f application must retain Function1.apply");
            };
            assert_eq!(
                typer.store().names.resolve(function_apply.name.text()),
                "apply"
            );
            let Type::TermRef {
                target: TermRefTarget::Symbol(function_apply_symbol),
                ..
            } = typer
                .store()
                .types
                .get(typer.typed_ast().get(function_call.function).ty)
            else {
                panic!("Function1.apply must retain its canonical symbol");
            };
            assert_eq!(
                observed_symbol_path(typer.store(), *function_apply_symbol),
                ".scala.Function1.apply"
            );
            assert!(matches!(
                &typer.typed_ast().get(function_apply.qualifier).kind,
                TreeKind::Ident(_)
            ));
            assert!(matches!(
                typer
                    .store()
                    .types
                    .get(typer.typed_ast().get(function_apply.qualifier).ty),
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } if *symbol == source_f
            ));
            let get_argument = function_call.args[0];
            let TreeKind::Select(get_selection) = &typer.typed_ast().get(get_argument).kind else {
                panic!("f's original argument must remain this.get");
            };
            assert_eq!(
                typer.store().names.resolve(get_selection.name.text()),
                "get"
            );
            assert!(matches!(
                typer.typed_ast().get(get_selection.qualifier).kind,
                TreeKind::This(_)
            ));
            let Type::TermRef {
                target: TermRefTarget::Symbol(get_symbol),
                ..
            } = typer
                .store()
                .types
                .get(typer.typed_ast().get(get_argument).ty)
            else {
                panic!("this.get must retain its canonical Option.get symbol");
            };
            assert_eq!(
                observed_symbol_path(typer.store(), *get_symbol),
                ".scala.Option.get"
            );
            let SymbolInfo::Complete(get_signature) = *typer.store().symbols.info(*get_symbol)
            else {
                panic!("the canonical Option.get signature must be complete");
            };
            assert_eq!(
                normalized_type(typer.store(), get_signature, signature),
                ".scala.Option.A"
            );
            assert_eq!(
                normalized_type(
                    typer.store(),
                    typer.typed_ast().get(function_call_id).ty,
                    signature
                ),
                ".scala.Option.map.B"
            );
            let normalized_f_type =
                normalized_type(typer.store(), method_type.params[0].ty, signature);
            assert!(normalized_f_type.starts_with(".scala.Function1[.scala.Option.A,"));
            let normalized_result_type =
                normalized_type(typer.store(), method_type.result, signature);
            assert_eq!(normalized_result_type, ".scala.Option[B#0]");
            let normalized_union = normalized_type(typer.store(), union_type, signature);
            let normalized_some_result = normalized_type(
                typer.store(),
                typer.typed_ast().get(typed_if.else_branch).ty,
                signature,
            );
            assert_eq!(normalized_some_result, ".scala.Some[.scala.Option.map.B]");
            let Type::Applied {
                tycon,
                args: some_args,
            } = typer
                .store()
                .types
                .get(typer.typed_ast().get(typed_if.else_branch).ty)
            else {
                panic!("the original Some branch must retain its applied result type");
            };
            let mut some_symbols = Vec::new();
            type_symbols(typer.store(), *tycon, &mut some_symbols, &mut Vec::new());
            assert!(some_symbols.contains(&some));
            assert_eq!(some_args.len(), 1);
            assert!(matches!(
                typer.store().types.get(some_args[0]),
                Type::TypeRef {
                    target: TypeRefTarget::Symbol(symbol),
                    ..
                } if *symbol == source_binder
            ));
            let Type::TermRef {
                target: TermRefTarget::Symbol(none_branch),
                ..
            } = typer
                .store()
                .types
                .get(typer.typed_ast().get(typed_if.then_branch).ty)
            else {
                panic!("the original None branch must retain its canonical module reference");
            };
            assert_eq!(
                observed_symbol_path(typer.store(), *none_branch),
                ".scala.None"
            );

            for symbol in [option, some, none_module_class] {
                assert!(
                    matches!(*typer.store().symbols.info(symbol), SymbolInfo::Complete(_)),
                    "reachable canonical source class metadata should be complete: {}",
                    observed_symbol_path(typer.store(), symbol)
                );
            }
            for symbol in [option, some] {
                assert!(matches!(
                    typer.index.definition_of(symbol),
                    Some(SourceDefinition::Canonical { source: actual_source, .. })
                        if actual_source == source
                ));
            }
            assert!(matches!(
                typer.index.definition_of(none),
                Some(SourceDefinition::Canonical { source: actual_source, tree: actual_tree })
                    if actual_source == source && actual_tree == none_tree
            ));
            assert_eq!(
                typer.index.derived_symbol_at(
                    typer.store().symbols.get(none).owner.unwrap(),
                    source,
                    none_tree,
                ),
                Some(none_module_class)
            );
            let mappings = parsed_ast
                .iter()
                .filter_map(|(source_tree, _)| {
                    typer
                        .source_typed_index()
                        .get(source, source_tree)
                        .map(|typed_tree| format!("{}:{}", source_tree.index(), typed_tree.index()))
                })
                .collect::<Vec<_>>();
            for (source_tree, _) in parsed_ast.iter() {
                if let Some(mapped) = typer.source_typed_index().get(source, source_tree) {
                    let source_span = parsed_ast.get(source_tree).position.unwrap();
                    let typed_span = typer.typed_ast().get(mapped).position.unwrap();
                    assert_eq!(source_span.source(), typed_span.source());
                    assert!(
                        source_span.span().range().start() <= typed_span.span().range().start()
                            && source_span.span().range().end() >= typed_span.span().range().end(),
                        "mapped typed span must remain within its original source span"
                    );
                }
            }
            assert_eq!(
                mappings,
                [
                    "195:0", "196:1", "199:2", "200:3", "203:4", "204:5", "205:5", "206:6",
                    "207:7", "208:9", "209:11", "210:12", "211:13", "212:14", "213:15", "214:16",
                ]
                .map(str::to_owned),
                "the complete original Option.map source subtree must map deterministically"
            );
            let normalized_receipt = format!(
                "method={} owner={} function={} function_apply={} get={} some_apply={} some_argument={} branches={} result={}",
                observed_symbol_path(typer.store(), method_symbol),
                observed_symbol_path(typer.store(), owner),
                normalized_f_type,
                observed_symbol_path(typer.store(), *function_apply_symbol),
                observed_symbol_path(typer.store(), *get_symbol),
                observed_symbol_path(typer.store(), *some_apply),
                normalized_type(
                    typer.store(),
                    typer.typed_ast().get(function_call_id).ty,
                    signature
                ),
                normalized_union,
                normalized_result_type
            );
            println!(
                "OPTION_MAP_ACCEPTED {normalized_receipt} rollback=full_and_retry spans=preserved"
            );
            println!(
                "OPTION_MAP_TYPED_BEGIN\nmethod=canonical:scala.Option.map\nbinder=B\nsource_typed_mappings={}\nOPTION_MAP_TYPED_END",
                mappings.join(",")
            );
            continue;
        }
        if relative.ends_with("Tuple.scala") {
            let typed_method = result.expect(
                "the original Tuple.toArray DefDef should type after automatic preparation",
            );
            assert_eq!(
                typer.source_typed_index().get(source, tree),
                Some(typed_method),
                "successful Tuple.toArray typing retains the original source-to-typed root"
            );
            assert!(
                observed.borrow().is_none(),
                "successful Tuple.toArray typing must not report a pre-rollback failure"
            );
            let TreeKind::DefDef(typed_definition) = &typer.typed_ast().get(typed_method).kind
            else {
                panic!("the original Tuple.toArray root must remain a typed DefDef");
            };
            let typed_rhs = typed_definition
                .rhs
                .expect("the original Tuple.toArray DefDef keeps its RHS");
            println!(
                "TUPLE_TO_ARRAY_ACCEPTED_AFTER_AUTOMATIC_PREPARATION source_tree={} typed_root={} typed_rhs={:?}",
                tree.index(),
                typed_method.index(),
                typer.typed_ast().get(typed_rhs).kind
            );
            continue;
        }
        let error = result.expect_err("the original Tuple.toArray DefDef remains blocked");
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

type EitherSwapSourceCase = (
    TreeId<Untyped>,
    TreeId<Untyped>,
    TreeId<Untyped>,
    TreeId<Untyped>,
    TreeId<Untyped>,
);

fn assert_original_either_swap_typed_tree(
    typer: &SourceTyper<'_>,
    parsed: &dotty_parser::ParseResult,
    source: SourceId,
    source_root: TreeId<Untyped>,
    typed_root: TreeId<Typed>,
    source_cases: &[EitherSwapSourceCase],
    index: &SourceSemanticIndex,
) {
    let get = |source_tree| {
        typer
            .source_typed_index()
            .get(source, source_tree)
            .expect("every traversed original source node has a typed mapping")
    };
    let TreeKind::DefDef(typed_method) = &typer.typed_ast().get(typed_root).kind else {
        panic!("returned root is the typed original DefDef")
    };
    assert_eq!(get(source_root), typed_root);
    let TreeKind::DefDef(source_method) = &parsed.ast.get(source_root).kind else {
        unreachable!()
    };
    let source_rhs = source_method.rhs.unwrap();
    let typed_rhs = typed_method.rhs.expect("typed original DefDef retains RHS");
    assert_eq!(get(source_rhs), typed_rhs);
    let TreeKind::Match(source_match) = &parsed.ast.get(source_rhs).kind else {
        unreachable!()
    };
    let TreeKind::Match(typed_match) = &typer.typed_ast().get(typed_rhs).kind else {
        panic!("typed DefDef RHS is the connected Match")
    };
    assert_eq!(get(source_match.selector), typed_match.selector);
    let method = index.symbol_at(source, source_root).unwrap();
    let owner = typer.store().symbols.get(method).owner.unwrap();
    assert!(
        matches!(
            typer.typed_ast().get(typed_match.selector).kind,
            TreeKind::This(_)
        ),
        "the connected Match selector remains the original this"
    );
    assert!(
        matches!(
            typer.store().types.get(typer.typed_ast().get(typed_match.selector).ty),
            Type::ThisType { class } if *class == owner
        ),
        "the selector retains canonical Either this identity"
    );
    assert_eq!(typed_match.cases.len(), source_cases.len());
    let params = typer.source_class_type_parameters(owner).unwrap();
    let mut binders = Vec::new();
    for (
        (source_case, source_pattern, source_binder, source_body, source_application),
        typed_case,
    ) in source_cases.iter().zip(typed_match.cases.iter())
    {
        assert_eq!(get(*source_case), *typed_case);
        let TreeKind::CaseDef(typed_case) = &typer.typed_ast().get(*typed_case).kind else {
            panic!("connected case is CaseDef")
        };
        assert_eq!(typed_case.pattern, get(*source_pattern));
        assert_eq!(typed_case.body, get(*source_body));
        let TreeKind::UnApply(unapply) = &typer.typed_ast().get(typed_case.pattern).kind else {
            panic!("connected source extractor lowers to UnApply")
        };
        let TreeKind::Apply(source_extractor) = &parsed.ast.get(*source_pattern).kind else {
            panic!("original source extractor is an Apply")
        };
        let TreeKind::Ident(source_extractor_name) =
            &parsed.ast.get(source_extractor.function).kind
        else {
            panic!("original Either extractor has a canonical source identifier")
        };
        let variant_name = typer
            .store()
            .names
            .resolve(source_extractor_name.name.text());
        let variant_class = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if typer
                        .store()
                        .names
                        .resolve(definition.name.as_name().text())
                        == variant_name =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .expect("source extractor names its canonical Either variant class");
        let expected_unapply = index
            .generated_case_class_method(
                variant_class,
                dotty_core::source_semantics::GeneratedCaseClassMethodRole::Unapply,
            )
            .expect("the source variant has its canonical generated unapply method");
        let TreeKind::TypeApply(unapply_type_apply) = &typer.typed_ast().get(unapply.function).kind
        else {
            panic!("connected generic extractor keeps its type application")
        };
        let TreeKind::Select(typed_extractor_select) =
            &typer.typed_ast().get(unapply_type_apply.function).kind
        else {
            panic!("typed generic extractor is selected from its original source identifier")
        };
        assert_eq!(
            typed_extractor_select.qualifier,
            get(source_extractor.function),
            "the extractor selection retains the source identifier mapping"
        );
        let applied_unapply = match typer
            .store()
            .types
            .get(typer.typed_ast().get(unapply_type_apply.function).ty)
        {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("typed extractor function references a canonical method: {other:?}"),
        };
        assert_eq!(
            applied_unapply, expected_unapply,
            "connected UnApply calls the canonical generated variant method"
        );
        assert_eq!(unapply_type_apply.args.len(), params.len());
        for (argument, parameter) in unapply_type_apply.args.iter().zip(&params) {
            assert!(
                matches!(
                    typer.store().types.get(typer.typed_ast().get(*argument).ty),
                    Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if symbol == parameter
                ),
                "generated unapply instantiation preserves the original [A, B] parameter order"
            );
        }
        assert_eq!(unapply.patterns.len(), 1);
        assert_eq!(unapply.patterns[0], get(*source_binder));
        let TreeKind::Bind(bind) = &typer.typed_ast().get(unapply.patterns[0]).kind else {
            panic!("connected extractor argument is the original Bind")
        };
        let binder = typer
            .pattern_binding_symbol_at(source, *source_binder)
            .unwrap();
        assert_eq!(
            typer
                .store()
                .names
                .resolve(typer.store().symbols.get(binder).name.text()),
            typer.store().names.resolve(bind.name.text())
        );
        assert_eq!(typer.store().symbols.get(binder).owner, Some(method));
        assert!(
            !binders.contains(&binder),
            "case binder symbols are distinct"
        );
        binders.push(binder);
        let binder_type = match typer.store().symbols.info(binder) {
            SymbolInfo::Complete(ty) => *ty,
            other => panic!("binder has canonical completed type: {other:?}"),
        };
        let expected_parameter = if binders.len() == 1 {
            params[0]
        } else {
            params[1]
        };
        assert!(matches!(typer.store().types.get(binder_type),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == expected_parameter));
        let TreeKind::Block(typed_body) = &typer.typed_ast().get(typed_case.body).kind else {
            panic!("original case body remains Block")
        };
        assert_eq!(typed_body.expr, get(*source_application));
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed_body.expr).kind else {
            panic!("generated case constructor remains Apply")
        };
        assert_eq!(application.args.len(), 1);
        assert!(
            matches!(typer.store().types.get(typer.typed_ast().get(application.args[0]).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == binder),
            "constructor application argument references its own case binder"
        );
    }
}

#[test]
#[ignore = "requires pinned Scala 3.9.0 sources, classpath jars, and JDK environment"]
fn types_acceptance_v_either_swap_with_original_generated_applications() {
    let scala_root = PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT"));
    let revision = Command::new("git")
        .args(["-C", scala_root.to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .expect("git reads the pinned Scala source revision");
    assert!(revision.status.success());
    assert_eq!(
        String::from_utf8(revision.stdout).unwrap().trim(),
        "777528f19a58e794c9954a42f433373472ec57f8",
        "the Acceptance V observation uses pinned Scala 3.9.0 sources"
    );
    let relative = "library/src/scala/util/Either.scala";
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
        .find(|(tree, node)| tree.index() == 84 && matches!(node.kind, TreeKind::DefDef(_)))
        .expect("selected original Either.swap DefDef");
    let method_symbol = index
        .symbol_at(source, tree)
        .expect("canonical source identity for the original Either.swap");
    let owner = store.symbols.get(method_symbol).owner.unwrap();
    let TreeKind::DefDef(source_definition) = &parsed.ast.get(tree).kind else {
        unreachable!()
    };
    let TreeKind::Match(source_match) = &parsed.ast.get(source_definition.rhs.unwrap()).kind else {
        unreachable!()
    };
    let source_cases = source_match
        .cases
        .iter()
        .map(|case_tree| {
            let TreeKind::CaseDef(case) = &parsed.ast.get(*case_tree).kind else {
                unreachable!()
            };
            let TreeKind::Apply(pattern) = &parsed.ast.get(case.pattern).kind else {
                unreachable!()
            };
            let binder_tree = *pattern.args.first().expect("one source binder per case");
            let TreeKind::Block(body) = &parsed.ast.get(case.body).kind else {
                unreachable!()
            };
            (*case_tree, case.pattern, binder_tree, case.body, body.expr)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed_symbol_path(&store, owner),
        ".scala.util.Either",
        "the original swap method remains owned by scala.util.Either"
    );
    assert!(matches!(
        index.definition_of(method_symbol),
        Some(SourceDefinition::Canonical {
            source: definition_source,
            tree: definition_tree
        }) if definition_source == source && definition_tree == tree
    ));
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME"));
    let release = std::env::var("SCALA39_JDK_RELEASE")
        .expect("SCALA39_JDK_RELEASE")
        .parse()
        .unwrap();
    assert_eq!(release, 21, "the classpath resolver uses JDK 21");
    let jars =
        std::env::split_paths(&std::env::var_os("SCALA39_CLASSPATH").expect("SCALA39_CLASSPATH"))
            .collect::<Vec<_>>();
    let mut entries: Vec<Box<dyn ClassPathEntry>> = vec![Box::new(
        JdkClassPath::new(java_home.join("jmods")).unwrap(),
    )];
    for jar in &jars {
        entries.push(Box::new(JarClassPath::new(jar.clone(), release).unwrap()));
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
    typer.transaction_failure_error_observer = Some(Rc::new(move |typer, error| {
        let method = typer.index.symbol_at(typer.source, tree).unwrap();
        let method_info = typer.store.symbols.get(method);
        let mut lines = vec![
            format!(
                "root={relative}#tree=84 source_span={:?}",
                parsed_ast.get(tree).position
            ),
            format!("error={error:?}"),
            format!(
                "canonical_source_method={method:?} path={} owner={:?} owner_path={} origin={:?} canonical={}",
                observed_symbol_path(typer.store, method),
                method_info.owner,
                method_info
                    .owner
                    .map(|owner| observed_symbol_path(typer.store, owner))
                    .unwrap_or_else(|| "unavailable".to_owned()),
                method_info.origin,
                matches!(typer.index.definition_of(method), Some(SourceDefinition::Canonical { source, tree: definition_tree }) if source == typer.source && definition_tree == tree)
            ),
        ];
        for (source_tree, node) in parsed_ast.iter() {
            if !(65..=85).contains(&source_tree.index()) {
                continue;
            }
            lines.push(format!(
                "original_pattern_tree={} span={:?} shape={:?}",
                source_tree.index(),
                node.position,
                node.kind
            ));
        }
        let source_declarations = parsed_ast
            .iter()
            .filter_map(|(source_tree, node)| match &node.kind {
                TreeKind::TypeDef(definition) => {
                    let name = typer.store.names.resolve(definition.name.as_name().text());
                    matches!(name, "Either" | "Left" | "Right")
                        .then_some((name.to_owned(), source_tree))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        for (name, source_tree) in source_declarations {
            let Some(class) = typer.index.symbol_at(typer.source, source_tree) else {
                lines.push(format!(
                    "canonical_source_class={name} unavailable=no_source_symbol"
                ));
                continue;
            };
            let info = typer.store.symbols.get(class);
            lines.push(format!(
                "canonical_source_class={class:?} path={} kind={:?} origin={:?} links={:?} info={:?} tree={}",
                observed_symbol_path(typer.store, class), info.kind, info.origin, info.links, info.info, source_tree.index()
            ));
            if matches!(name.as_str(), "Left" | "Right") {
                if let Some(companion) = info.links.companion {
                    let companion_info = typer.store.symbols.get(companion);
                    lines.push(format!(
                        "generated_companion={companion:?} path={} kind={:?} origin={:?} info={:?} links={:?}",
                        observed_symbol_path(typer.store, companion), companion_info.kind,
                        companion_info.origin, companion_info.info, companion_info.links
                    ));
                }
                let generated = typer.index.generated_case_class_method(
                    class,
                    dotty_core::source_semantics::GeneratedCaseClassMethodRole::Unapply,
                );
                lines.push(format!("generated_unapply={generated:?} class={class:?}"));
                if let Some(unapply) = generated {
                    let unapply_info = typer.store.symbols.get(unapply);
                    lines.push(format!("generated_unapply_info={unapply:?} path={} owner={:?} info={:?} origin={:?}", observed_symbol_path(typer.store, unapply), unapply_info.owner, unapply_info.info, unapply_info.origin));
                    lines.push(format!(
                        "generated_unapply_identity={:?}",
                        typer.index.generated_case_class_method_identity(unapply)
                    ));
                }
            }
        }
        if let TyperError::ExtractorUnapplyNotFound {
            source,
            tree_index,
            extractor,
        } = error
        {
            let extractor_info = typer.store.symbols.get(*extractor);
            lines.push(format!(
                "extractor_failure=source:{source:?} tree:{tree_index} symbol:{extractor:?} path:{} kind:{:?} origin:{:?} info:{:?} links:{:?}",
                observed_symbol_path(typer.store, *extractor), extractor_info.kind, extractor_info.origin, extractor_info.info, extractor_info.links
            ));
        }
        *capture.borrow_mut() = Some(lines.join("\n"));
    }));
    let rollback_projection_before = issue_1999_late_failure_projection(&typer, &parsed);
    let rollback_store_before = typer.store().checkpoint();
    let rollback_typed_before = typer.typed_ast().checkpoint();
    let rollback_type_index_before = format!("{:?}", typer.type_index);
    let index_ref = &index;
    let generated_symbols = parsed
        .ast
        .iter()
        .filter_map(|(source_tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if matches!(
                    typer.store.names.resolve(definition.name.as_name().text()),
                    "Left" | "Right"
                ) =>
            {
                index.symbol_at(source, source_tree)
            }
            _ => None,
        })
        .flat_map(|class| {
            [
                dotty_core::source_semantics::GeneratedCaseClassMethodRole::Apply,
                dotty_core::source_semantics::GeneratedCaseClassMethodRole::Unapply,
            ]
            .into_iter()
            .filter_map(move |role| index_ref.generated_case_class_method(class, role))
        })
        .collect::<Vec<_>>();
    let pre_rollback_seen = Rc::new(RefCell::new(false));
    let pre_rollback_capture = Rc::clone(&pre_rollback_seen);
    let observed_cases = source_cases.clone();
    let observed_generated = generated_symbols.clone();
    typer.transaction_failure_observer = Some(Rc::new(move |typer| {
        for (_, _, binder_tree, _, _) in &observed_cases {
            let binder = typer
                .pattern_binding_symbol_at(source, *binder_tree)
                .expect("the complete method has published each original binder before rollback");
            assert!(matches!(
                typer.store().symbols.info(binder),
                SymbolInfo::Complete(_)
            ));
        }
        for symbol in &observed_generated {
            assert!(
                matches!(typer.store().symbols.info(*symbol), SymbolInfo::Complete(_)),
                "generated case-class completion is present before rollback: {symbol:?}"
            );
        }
        *pre_rollback_capture.borrow_mut() = true;
    }));
    let owner_info_before = *typer.store().symbols.info(owner);
    typer.inject_failure_after_outermost_success = true;
    let failed = typer
        .type_method_definition(tree)
        .expect_err("the injected late failure follows complete Either.swap typing");
    assert!(matches!(
        failed,
        TyperError::SymbolResolution {
            source: failed_source,
            error: ResolutionError::Malformed { ref reason },
            ..
        } if failed_source == source
            && reason == "test-injected failure after successful source typing"
    ));
    assert!(
        *pre_rollback_seen.borrow(),
        "the failure observer inspected state before rollback"
    );
    assert_eq!(
        typer.store().symbols.info(owner),
        &owner_info_before,
        "late failure rolls semantic publication back"
    );
    assert_eq!(
        typer.store().checkpoint(),
        rollback_store_before,
        "late failure restores the complete semantic store checkpoint"
    );
    assert_eq!(
        typer.typed_ast().checkpoint(),
        rollback_typed_before,
        "late failure restores the complete typed arena checkpoint"
    );
    assert_eq!(
        format!("{:?}", typer.type_index),
        rollback_type_index_before,
        "late failure restores source type/index state"
    );
    assert_issue_1999_projection_unchanged(
        &rollback_projection_before,
        &issue_1999_late_failure_projection(&typer, &parsed),
        "late failure restores mappings, binders, scopes, and typer-owned state",
    );
    assert_eq!(typer.source_typed_index().get(source, tree), None);
    for (case_tree, pattern_tree, binder_tree, body_tree, application_tree) in &source_cases {
        for source_tree in [
            *case_tree,
            *pattern_tree,
            *binder_tree,
            *body_tree,
            *application_tree,
        ] {
            assert!(
                typer
                    .source_typed_index()
                    .get(source, source_tree)
                    .is_none(),
                "rollback removes the partial typed mapping for source tree {}",
                source_tree.index()
            );
        }
        assert!(
            typer
                .pattern_binding_symbol_at(source, *binder_tree)
                .is_none(),
            "rollback removes the original pattern binder mapping"
        );
    }
    for symbol in &generated_symbols {
        assert_eq!(
            *typer.store().symbols.info(*symbol),
            SymbolInfo::Missing,
            "rollback removes generated case-class completion {symbol:?}"
        );
    }
    assert!(!typer.inject_failure_after_outermost_success);
    let typed_root = typer
        .type_method_definition(tree)
        .expect("the unchanged original method retries in the same session");
    assert_eq!(
        typer.source_typed_index().get(source, tree),
        Some(typed_root)
    );
    assert_original_either_swap_typed_tree(
        &typer,
        &parsed,
        source,
        tree,
        typed_root,
        &source_cases,
        &index,
    );
    let TreeKind::DefDef(definition) = &parsed.ast.get(tree).kind else {
        unreachable!()
    };
    let TreeKind::Match(matched) = &parsed.ast.get(definition.rhs.unwrap()).kind else {
        panic!("original Either.swap body remains a Match")
    };
    assert_eq!(matched.cases.len(), 2);
    let mut declared_result = match typer.store().symbols.info(method_symbol) {
        SymbolInfo::Complete(signature) => *signature,
        other => panic!("original method signature is complete: {other:?}"),
    };
    loop {
        declared_result = match typer.store().types.get(declared_result) {
            Type::Poly(poly) => poly.result,
            Type::Method(method) => method.result,
            _ => break,
        };
    }
    let (either_tycon, declared_arguments) = match typer.store().types.get(declared_result) {
        Type::Applied { tycon, args } => (*tycon, args.clone()),
        other => panic!("declared Either.swap result is applied Either[B,A]: {other:?}"),
    };
    let declared_argument_symbols = declared_arguments
        .iter()
        .map(|argument| match typer.store().types.get(*argument) {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("declared result preserves generic parameter identity: {other:?}"),
        })
        .collect::<Vec<_>>();
    let owner_parameters = typer.source_class_type_parameters(owner).unwrap();
    assert_eq!(
        declared_argument_symbols,
        [owner_parameters[1], owner_parameters[0]],
        "declared result retains the original reversed generic order"
    );
    assert!(matches!(
        typer.store().types.get(either_tycon),
        Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == owner
    ));
    let typed_match = typer
        .source_typed_index()
        .get(source, definition.rhs.unwrap())
        .unwrap();
    let match_type = typer.typed_ast().get(typed_match).ty;
    assert!(matches!(
        typer.store().types.get(match_type),
        Type::Or { .. }
    ));
    let match_conformance = typer.conforms(match_type, declared_result);
    assert!(
        matches!(match_conformance, Ok(true)),
        "Match {:?}, declared {:?}, conformance {:?}",
        typer.store().types.get(match_type),
        typer.store().types.get(declared_result),
        match_conformance
    );
    let wrong_order = typer.store.types.alloc(Type::Applied {
        tycon: either_tycon,
        args: vec![declared_arguments[1], declared_arguments[0]],
    });
    let wrong_order_control = typer.conforms(match_type, wrong_order);
    assert!(!matches!(wrong_order_control, Ok(true)));
    let incompatible_control = typer.conforms(match_type, typer.definitions.boolean);
    assert!(!matches!(incompatible_control, Ok(true)));
    let match_type_label = match typer.store().types.get(match_type) {
        Type::Or { left, right } => [*left, *right]
            .into_iter()
            .map(|branch| match typer.store().types.get(branch) {
                Type::Applied { tycon, args } => {
                    let Type::TypeRef {
                        target: TypeRefTarget::Symbol(constructor),
                        ..
                    } = typer.store().types.get(*tycon)
                    else {
                        panic!("branch constructor is canonical")
                    };
                    let parameters = args
                        .iter()
                        .map(|argument| match typer.store().types.get(*argument) {
                            Type::TypeRef {
                                target: TypeRefTarget::Symbol(parameter),
                                ..
                            } => typer
                                .store()
                                .names
                                .resolve(typer.store().symbols.get(*parameter).name.text()),
                            other => panic!("branch argument is a type reference: {other:?}"),
                        })
                        .collect::<Vec<_>>();
                    format!(
                        "{}[{}]",
                        observed_symbol_path(typer.store(), *constructor),
                        parameters.join(",")
                    )
                }
                other => panic!("branch type is applied: {other:?}"),
            })
            .collect::<Vec<_>>(),
        other => panic!("expected canonical union match result: {other:?}"),
    };
    assert_eq!(
        match_type_label,
        [
            ".scala.util.Right[Nothing,A]".to_owned(),
            ".scala.util.Left[B,Nothing]".to_owned(),
        ],
        "the Match retains the two original Scala branch result types"
    );
    println!(
        "EITHER_SWAP_CONFORMANCE match={} declared=Either[B,A] correct=accepted wrong_order=rejected incompatible_boolean=rejected",
        match_type_label.join(" | ")
    );
    for (case_tree, variant_name) in matched.cases.iter().copied().zip(["Right", "Left"]) {
        let TreeKind::CaseDef(case) = &parsed.ast.get(case_tree).kind else {
            panic!("original Either.swap arms remain case definitions")
        };
        let TreeKind::Block(body) = &parsed.ast.get(case.body).kind else {
            panic!("original case body retains its source block")
        };
        let source_application = body.expr;
        let typed_application = typer
            .source_typed_index()
            .get(source, source_application)
            .expect("the authentic constructor application has a typed mapping");
        let application = match &typer.typed_ast().get(typed_application).kind {
            TreeKind::Apply(application) => application.clone(),
            _ => panic!("the authentic case-body constructor remains an Apply"),
        };
        let branch_type = typer.typed_ast().get(typed_application).ty;
        assert!(typer.conforms(branch_type, declared_result).unwrap());
        assert!(!matches!(
            typer.conforms(branch_type, wrong_order),
            Ok(true)
        ));
        assert!(!matches!(
            typer.conforms(branch_type, typer.definitions.boolean),
            Ok(true)
        ));
        let class = parsed
            .ast
            .iter()
            .find_map(|(source_tree, node)| match &node.kind {
                TreeKind::TypeDef(class_definition)
                    if typer
                        .store
                        .names
                        .resolve(class_definition.name.as_name().text())
                        == variant_name =>
                {
                    typer.index.symbol_at(source, source_tree)
                }
                _ => None,
            })
            .expect("original variant has canonical source class identity");
        let apply = typer
            .index
            .generated_case_class_method(class, dotty_core::GeneratedCaseClassMethodRole::Apply)
            .expect("original variant has canonical generated Apply identity");
        let typed_function = typer.typed_ast().get(application.function);
        assert!(matches!(
            typer.store.types.get(typed_function.ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == apply
        ));
    }
    let method_span = parsed.ast.get(tree).position.unwrap();
    assert_eq!(method_span.span().range().start(), 8241);
    assert_eq!(method_span.span().range().end(), 8341);
    let mut source_mappings = parsed
        .ast
        .iter()
        .filter_map(|(source_tree, source_node)| {
            let source_span = source_node.position?;
            let range = source_span.span().range();
            if range.start() < method_span.span().range().start()
                || range.end() > method_span.span().range().end()
            {
                return None;
            }
            let typed_tree = typer.source_typed_index().get(source, source_tree)?;
            let typed_span = typer.typed_ast().get(typed_tree).position?;
            assert_eq!(source_span.source(), typed_span.source());
            assert_eq!(range, typed_span.span().range());
            Some(format!(
                "{}@{}..{}",
                source_tree.index(),
                range.start(),
                range.end()
            ))
        })
        .collect::<Vec<_>>();
    source_mappings.sort();
    assert_eq!(
        source_mappings,
        [
            "65@8251..8263",
            "66@8266..8270",
            "67@8288..8292",
            "68@8293..8294",
            "69@8288..8295",
            "70@8300..8305",
            "71@8306..8307",
            "72@8300..8308",
            "73@8297..8308",
            "74@8283..8308",
            "75@8318..8323",
            "76@8324..8325",
            "77@8318..8326",
            "78@8330..8334",
            "79@8335..8336",
            "80@8330..8337",
            "81@8327..8337",
            "82@8313..8337",
            "83@8266..8341",
            "84@8241..8341",
        ],
        "the original method's complete source mapping set remains exact"
    );
    println!(
        "EITHER_SWAP_RUST_TREE root=84@8241..8341 match=83@8266..8341 method_result=Either[B,A] source_mappings={}",
        source_mappings.join(",")
    );
    println!("whole_method_outcome=typed root={}", typed_root.index());
}

#[test]
#[ignore = "requires pinned Scala 3.9.0 sources, classpath jars, and JDK environment"]
fn original_either_swap_cases_type_authentic_application_subtrees() {
    let first = replay_original_either_swap_patterns();
    let second = replay_original_either_swap_patterns();
    assert_eq!(
        first, second,
        "fresh-session pattern evidence normalizes deterministically"
    );
    assert_eq!(first.len(), 2);
    println!("ISSUE_2282_ORIGINAL_CASE_APPLICATIONS {first:?}");
}

#[test]
#[ignore = "requires pinned Scala 3.9.0 sources, classpath jars, and JDK environment"]
fn original_tuple_to_array_automatically_prepares_canonical_source_owner() {
    let scala_root = PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT"));
    let revision = Command::new("git")
        .args(["-C", scala_root.to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .expect("git reads the pinned Scala source revision");
    assert!(revision.status.success());
    assert_eq!(
        String::from_utf8(revision.stdout).unwrap().trim(),
        "777528f19a58e794c9954a42f433373472ec57f8"
    );
    let scala_file = scala_root.join("library/src/scala/Tuple.scala");
    let text = fs::read_to_string(&scala_file).unwrap();
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(&text).unwrap(),
        source,
        ContextualScanner::new(&text).unwrap(),
        &mut store.names,
    );
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "library/src/scala/Tuple.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (method_tree, _) = parsed
        .ast
        .iter()
        .find(|(tree, node)| tree.index() == 23 && matches!(node.kind, TreeKind::DefDef(_)))
        .expect("original Tuple.scala#tree=23 remains a DefDef");
    let tuple_tree = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "Tuple" =>
            {
                Some(tree)
            }
            _ => None,
        })
        .expect("canonical source Tuple trait");
    let tuple = index
        .symbol_at(source, tuple_tree)
        .expect("Tuple has a source-index symbol");
    let non_empty_tuple = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "NonEmptyTuple" =>
            {
                index.symbol_at(source, tree)
            }
            _ => None,
        })
        .expect("unrelated source NonEmptyTuple trait");
    let method = index
        .symbol_at(source, method_tree)
        .expect("toArray has a canonical source symbol");
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME"));
    let release = std::env::var("SCALA39_JDK_RELEASE")
        .expect("SCALA39_JDK_RELEASE")
        .parse()
        .unwrap();
    let jars =
        std::env::split_paths(&std::env::var_os("SCALA39_CLASSPATH").expect("SCALA39_CLASSPATH"))
            .collect::<Vec<_>>();
    let mut entries: Vec<Box<dyn ClassPathEntry>> = vec![Box::new(
        JdkClassPath::new(java_home.join("jmods")).unwrap(),
    )];
    for jar in jars {
        entries.push(Box::new(JarClassPath::new(jar, release).unwrap()));
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

    assert!(matches!(
        *typer.store().symbols.info(non_empty_tuple),
        SymbolInfo::Missing
    ));
    assert!(matches!(
        *typer.store().symbols.info(tuple),
        SymbolInfo::Missing
    ));
    typer.inject_failure_after_outermost_success = true;
    let failed = typer
        .type_method_definition(method_tree)
        .expect_err("the test-injected late failure follows automatic owner preparation");
    assert!(
        matches!(
            failed,
            TyperError::SymbolResolution {
                source: failed_source,
                error: ResolutionError::Malformed { ref reason },
                ..
            } if failed_source == source
                && reason == "test-injected failure after successful source typing"
        ),
        "unexpected injected-transaction result: {failed:?}"
    );
    assert!(
        matches!(*typer.store().symbols.info(tuple), SymbolInfo::Missing),
        "failed method typing rolls back automatic source-owner completion"
    );
    assert_eq!(
        typer.source_typed_index().get(source, method_tree),
        None,
        "failed method typing rolls back the source-to-typed mapping"
    );
    assert!(!typer.inject_failure_after_outermost_success);

    let typed_method = match *typer.store().symbols.info(tuple) {
        SymbolInfo::Missing => typer
            .type_method_definition(method_tree)
            .expect("the same SourceTyper retries after rollback"),
        _ => unreachable!("failed transaction restored the source owner"),
    };
    assert_eq!(
        typer.source_typed_index().get(source, method_tree),
        Some(typed_method),
        "ordinary method typing records its canonical source mapping"
    );
    let SymbolInfo::Complete(prepared_type) = *typer.store().symbols.info(tuple) else {
        panic!("ordinary method typing completes its source owner");
    };
    assert!(
        matches!(
            *typer.store().symbols.info(non_empty_tuple),
            SymbolInfo::Missing
        ),
        "owner preparation leaves the unrelated source trait incomplete"
    );
    let Type::ClassInfo(class_info) = typer.store().types.get(prepared_type) else {
        panic!("Tuple completion publishes ClassInfo");
    };
    assert_eq!(class_info.class, tuple);
    let declaration_scope = index.scope_of(tuple).expect("source declaration scope");
    assert_eq!(class_info.declarations, declaration_scope);
    assert!(matches!(
        index.definition_of(tuple),
        Some(SourceDefinition::Canonical { source: owner_source, tree })
            if owner_source == source && tree == tuple_tree
    ));

    assert_eq!(
        typer.source_typed_index().get(source, method_tree),
        Some(typed_method)
    );
    let TreeKind::DefDef(_) = &parsed.ast.get(method_tree).kind else {
        unreachable!();
    };
    let TreeKind::DefDef(typed_definition) = &typer.typed_ast().get(typed_method).kind else {
        panic!("the accepted root remains the original owned DefDef");
    };
    let typed_result = typer.typed_ast().get(typed_definition.tpt).ty;
    let Type::Applied { tycon, args } = typer.store().types.get(typed_result) else {
        panic!("Tuple.toArray result remains Array[Object]");
    };
    let Type::TypeRef {
        target: TypeRefTarget::Symbol(array),
        ..
    } = typer.store().types.get(*tycon)
    else {
        panic!("Array result uses the canonical Array symbol");
    };
    assert_eq!(observed_symbol_path(typer.store(), *array), ".scala.Array");
    assert_eq!(args.len(), 1);
    let Type::TypeRef {
        target: TypeRefTarget::Symbol(object),
        ..
    } = typer.store().types.get(args[0])
    else {
        panic!("Array element uses the canonical Object symbol");
    };
    assert_eq!(
        observed_symbol_path(typer.store(), *object),
        ".java.lang.Object"
    );
    let source_to_array = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::Select(select)
                if typer.store().names.resolve(select.name.text()) == "toArray" =>
            {
                Some(tree)
            }
            _ => None,
        })
        .expect("original source selection of runtime toArray");
    let typed_to_array = typer
        .source_typed_index()
        .get(source, source_to_array)
        .expect("original toArray selection is mapped");
    let Type::TermRef {
        target: TermRefTarget::Symbol(selected),
        ..
    } = typer
        .store()
        .types
        .get(typer.typed_ast().get(typed_to_array).ty)
    else {
        panic!("selected runtime toArray has a canonical symbol reference");
    };
    assert_eq!(
        observed_symbol_path(typer.store(), *selected),
        ".scala.runtime.Tuples$.toArray"
    );
    let SymbolInfo::Complete(mut signature) = *typer.store().symbols.info(*selected) else {
        panic!("selected runtime toArray has a complete loaded signature");
    };
    while let Type::Poly(poly) = typer.store().types.get(signature) {
        signature = poly.result;
    }
    let Type::Method(method_signature) = typer.store().types.get(signature) else {
        panic!("selected runtime toArray has a method clause");
    };
    assert_eq!(method_signature.params.len(), 1);
    assert!(
        matches!(
            typer.store().types.get(method_signature.params[0].ty),
            Type::TypeRef { target: TypeRefTarget::Symbol(formal), .. } if *formal == tuple
        ),
        "the loaded formal is a TypeRef of the same canonical Tuple owner as the source this"
    );
    let source_this = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| matches!(node.kind, TreeKind::This(_)).then_some(tree))
        .expect("original this argument is retained");
    let typed_this = typer
        .source_typed_index()
        .get(source, source_this)
        .expect("original this argument is mapped");
    assert!(matches!(
        typer.store().types.get(typer.typed_ast().get(typed_this).ty),
        Type::ThisType { class } if *class == tuple
    ));
    let source_call = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::Apply(application) if application.args.contains(&source_this) => Some(tree),
            _ => None,
        })
        .expect("the original toArray application retains this as its argument");
    let typed_call = typer
        .source_typed_index()
        .get(source, source_call)
        .expect("original application is mapped into the typed method");
    let TreeKind::Apply(typed_application) = &typer.typed_ast().get(typed_call).kind else {
        panic!("the original method application remains an Apply");
    };
    assert_eq!(typed_application.args, vec![typed_this]);
    let method_span = parsed.ast.get(method_tree).position.unwrap().span().range();
    let mut mappings = parsed
        .ast
        .iter()
        .filter_map(|(source_tree, source_node)| {
            let source_range = source_node.position?.span().range();
            if source_range.start() < method_span.start() || source_range.end() > method_span.end()
            {
                return None;
            }
            let typed_tree = typer.source_typed_index().get(source, source_tree)?;
            let typed_node = typer.typed_ast().get(typed_tree);
            let typed_range = typed_node.position?.span().range();
            assert_eq!(
                (source_range.start(), source_range.end()),
                (typed_range.start(), typed_range.end()),
                "source-to-typed mapping preserves the original source span"
            );
            Some(format!(
                "{}..{}:{}=>{}",
                source_range.start(),
                source_range.end(),
                tuple_tree_kind_label(&source_node.kind),
                tuple_tree_kind_label(&typed_node.kind),
            ))
        })
        .collect::<Vec<_>>();
    mappings.sort();
    assert_eq!(
        mappings.len(),
        7,
        "all seven original method nodes are mapped"
    );
    let normalized = format!(
        "source=Tuple.scala#tree=23 method={} owner={} mappings={}",
        observed_symbol_path(typer.store(), method),
        observed_symbol_path(typer.store(), tuple),
        mappings.join(",")
    );
    assert_eq!(
        observed_symbol_path(typer.store(), method),
        ".scala.Tuple.toArray"
    );
    println!(
        "TUPLE_TO_ARRAY_ACCEPTED {normalized} selected={} argument=scala.Tuple.this result=scala.Array[java.lang.Object]",
        observed_symbol_path(typer.store(), *selected)
    );
    println!(
        "TUPLE_TO_ARRAY_AUTOMATIC_PREPARATION source_owner={} external_formal=TypeRef(source_owner) rollback_retry=passed",
        observed_symbol_path(typer.store(), tuple)
    );
}

#[test]
#[ignore = "requires pinned Scala 3.9.0 sources, classpath jars, and JDK environment"]
fn other_package_tuple_does_not_match_pinned_classpath_tuple_formal() {
    let scala_root = PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT"));
    let revision = Command::new("git")
        .args(["-C", scala_root.to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .expect("git reads the pinned Scala source revision");
    assert!(revision.status.success());
    assert_eq!(
        String::from_utf8(revision.stdout).unwrap().trim(),
        "777528f19a58e794c9954a42f433373472ec57f8"
    );
    let source_text =
        "package other\nclass Tuple { def use = _root_.scala.runtime.Tuples.toArray(this) }";
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        ContextualScanner::new(source_text).unwrap(),
        &mut store.names,
    );
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "issue-2237/OtherTuple.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let tuple_tree = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "Tuple" =>
            {
                Some(tree)
            }
            _ => None,
        })
        .expect("the same-spelled receiver is source-owned by package other");
    let tuple = index.symbol_at(source, tuple_tree).unwrap();
    let method_tree = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::DefDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "use" =>
            {
                Some(tree)
            }
            _ => None,
        })
        .expect("the ordinary method root is present");
    let rhs = match &parsed.ast.get(method_tree).kind {
        TreeKind::DefDef(definition) => definition.rhs.unwrap(),
        _ => unreachable!(),
    };
    let TreeKind::Apply(original_application) = &parsed.ast.get(rhs).kind else {
        panic!("the wrong-owner method body retains its original application");
    };
    assert_eq!(original_application.args.len(), 1);
    let argument_tree = original_application.args[0];
    assert!(matches!(
        parsed.ast.get(argument_tree).kind,
        TreeKind::This(_)
    ));
    let rhs_span = parsed.ast.get(rhs).position.unwrap().span().range();
    let selection_tree = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            if !node.position.is_some_and(|position| {
                let range = position.span().range();
                rhs_span.start() <= range.start() && range.end() <= rhs_span.end()
            }) {
                return None;
            }
            match &node.kind {
                TreeKind::Select(select)
                    if store.names.resolve(select.name.text()) == "toArray" =>
                {
                    Some(tree)
                }
                _ => None,
            }
        })
        .expect("the original selection names toArray");
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME"));
    let release = std::env::var("SCALA39_JDK_RELEASE")
        .expect("SCALA39_JDK_RELEASE")
        .parse()
        .unwrap();
    let jars =
        std::env::split_paths(&std::env::var_os("SCALA39_CLASSPATH").expect("SCALA39_CLASSPATH"))
            .collect::<Vec<_>>();
    let mut entries: Vec<Box<dyn ClassPathEntry>> = vec![Box::new(
        JdkClassPath::new(java_home.join("jmods")).unwrap(),
    )];
    for jar in jars {
        entries.push(Box::new(JarClassPath::new(jar, release).unwrap()));
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

    assert!(matches!(
        *typer.store().symbols.info(tuple),
        SymbolInfo::Missing
    ));
    type WrongOwnerObservation = (
        String,
        String,
        &'static str,
        String,
        &'static str,
        String,
        &'static str,
    );
    let observed = Rc::new(RefCell::new(None::<WrongOwnerObservation>));
    let capture = observed.clone();
    let parsed_ast = &parsed.ast;
    let tuple_definition = index.definition_of(tuple);
    typer.transaction_failure_error_observer = Some(Rc::new(move |typer, error| {
        assert!(
            matches!(
                error,
                TyperError::ApplicationArgumentTypeMismatch {
                    source: error_source,
                    tree_index,
                    argument_index: 0,
                    ..
                } if *error_source == source && *tree_index == rhs.index()
            ),
            "unexpected pre-rollback wrong-owner error: {error:?}"
        );
        assert!(matches!(
            tuple_definition,
            Some(SourceDefinition::Canonical {
                source: definition_source,
                tree,
            }) if definition_source == source && tree == tuple_tree
        ));
        let receiver_info = typer.store.symbols.get(tuple);
        assert_eq!(receiver_info.origin, SymbolOrigin::Source(source));
        assert_eq!(observed_symbol_path(typer.store, tuple), ".other.Tuple");

        let source_application = parsed_ast.get(rhs);
        let TreeKind::Apply(application) = &source_application.kind else {
            panic!("the original failing source node is not an Apply");
        };
        assert_eq!(application.args, vec![argument_tree]);
        let actual_type = match error {
            TyperError::ApplicationArgumentTypeMismatch { actual, .. } => *actual,
            _ => unreachable!("the error variant was checked above"),
        };
        assert!(matches!(
            typer.store.types.get(actual_type),
            Type::ThisType { class } if *class == tuple
        ));

        let selection_span = parsed_ast
            .get(selection_tree)
            .position
            .unwrap()
            .span()
            .range();
        let typed_selection = typer
            .typed_ast()
            .iter()
            .find_map(|(_, node)| {
                let range = node.position?.span().range();
                (range.start() == selection_span.start()
                    && range.end() == selection_span.end()
                    && matches!(node.kind, TreeKind::Select(_)))
                .then_some(node)
            })
            .expect("the external selection is typed before application conformance fails");
        let Type::TermRef {
            target: TermRefTarget::Symbol(selected),
            ..
        } = typer.store.types.get(typed_selection.ty)
        else {
            panic!("the external selection does not reference a canonical symbol");
        };
        let selected = *selected;
        let selected_info = typer.store.symbols.get(selected);
        let selected_path = observed_symbol_path(typer.store, selected);
        assert_eq!(selected_path, ".scala.runtime.Tuples$.toArray");
        let selected_origin = match selected_info.origin {
            SymbolOrigin::Tasty(_) => "Tasty",
            other => panic!("selected member is not from binary TASTy: {other:?}"),
        };
        assert_eq!(selected_info.kind, SymbolKind::Method);
        let SymbolInfo::Complete(mut signature) = selected_info.info else {
            panic!("the external selected member has no complete signature");
        };
        while let Type::Poly(poly) = typer.store.types.get(signature) {
            signature = poly.result;
        }
        let Type::Method(method_signature) = typer.store.types.get(signature) else {
            panic!("the external selected member does not have a method signature");
        };
        assert_eq!(method_signature.params.len(), 1);
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(formal),
            ..
        } = typer.store.types.get(method_signature.params[0].ty)
        else {
            panic!("the selected formal is not a canonical symbol TypeRef");
        };
        let formal = *formal;
        let formal_path = observed_symbol_path(typer.store, formal);
        assert_eq!(formal_path, ".scala.Tuple");
        let formal_origin = match typer.store.symbols.get(formal).origin {
            SymbolOrigin::Tasty(_) => "Tasty",
            other => panic!("the expected formal is not from binary TASTy: {other:?}"),
        };
        let application_span = format!("{}..{}", rhs_span.start(), rhs_span.end());
        *capture.borrow_mut() = Some((
            application_span,
            observed_symbol_path(typer.store, tuple),
            "Source",
            selected_path,
            selected_origin,
            formal_path,
            formal_origin,
        ));
    }));
    let error = typer
        .type_method_definition(method_tree)
        .expect_err("other.Tuple must not be accepted as scala.Tuple");
    assert!(
        matches!(
            error,
            TyperError::ApplicationArgumentTypeMismatch {
                source: error_source,
                tree_index,
                argument_index: 0,
                ..
            } if error_source == source && tree_index == rhs.index()
        ),
        "unexpected classpath-owner conflict result: {error:?}"
    );
    assert!(observed.borrow().is_some());
    assert!(matches!(
        *typer.store().symbols.info(tuple),
        SymbolInfo::Missing
    ));
    assert_eq!(typer.source_typed_index().get(source, method_tree), None);
    let (
        application_span,
        receiver_path,
        receiver_origin,
        selected_path,
        selected_origin,
        formal_path,
        formal_origin,
    ) = observed.borrow().clone().unwrap();
    println!(
        "TUPLE_TO_ARRAY_WRONG_OWNER_REJECTED application_span={application_span} receiver={receiver_path} receiver_origin={receiver_origin} selected={selected_path} selected_origin={selected_origin} formal={formal_path} formal_origin={formal_origin} error=ApplicationArgumentTypeMismatch rollback=passed"
    );
}

fn tuple_tree_kind_label<P: dotty_core::AstPhase>(kind: &TreeKind<P>) -> &'static str {
    match kind {
        TreeKind::DefDef(_) => "DefDef",
        TreeKind::Apply(_) => "Apply",
        TreeKind::Select(_) => "Select",
        TreeKind::This(_) => "This",
        TreeKind::Ident(_) => "Ident",
        TreeKind::AppliedTypeTree(_) => "AppliedTypeTree",
        TreeKind::ByNameTypeTree(_) => "ByNameTypeTree",
        TreeKind::TypeTree(_) => "TypeTree",
        TreeKind::PhaseSpecific(_) => "PhaseSpecific",
        _ => "Other",
    }
}
