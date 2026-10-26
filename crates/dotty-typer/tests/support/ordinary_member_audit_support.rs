use super::{audit_classpath_from_environment, audit_symbol_path, git_revision};
use dotty_classloader::classloader::{ClasspathSymbolResolver, LoadingSession};
use dotty_core::ast::{Modifier, TreeKind, Untyped, UntypedNode};
use dotty_core::types::{TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceDefinition, SourceId, SourceText, SymbolFlags,
    SymbolInfo, SymbolKind,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_parser::parse_compilation_unit;
use dotty_typer::SourceTyper;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn source_line(text: &str, start: usize) -> usize {
    text.get(..start)
        .map(|prefix| prefix.bytes().filter(|byte| *byte == b'\n').count() + 1)
        .unwrap_or(0)
}

fn ast_feature(kind: &TreeKind<Untyped>) -> Option<&'static str> {
    match kind {
        TreeKind::Apply(_) | TreeKind::TypeApply(_) => Some("calls"),
        TreeKind::Select(_) => Some("selections"),
        TreeKind::Ident(_) => Some("identifiers"),
        TreeKind::Literal(_) => Some("literals"),
        TreeKind::ValDef(_) => Some("local_values"),
        TreeKind::DefDef(_) => Some("nested_methods"),
        TreeKind::If(_) => Some("branches"),
        TreeKind::Match(_) => Some("matches"),
        TreeKind::Try(_) => Some("tries"),
        TreeKind::Closure(_) => Some("closures"),
        TreeKind::While(_) => Some("loops"),
        TreeKind::PhaseSpecific(
            UntypedNode::Function(_)
            | UntypedNode::FunctionWithMods(_)
            | UntypedNode::PolyFunction(_),
        ) => Some("closures"),
        TreeKind::PhaseSpecific(UntypedNode::ParsedTry(_)) => Some("tries"),
        TreeKind::PhaseSpecific(UntypedNode::ForYield(_) | UntypedNode::ForDo(_)) => {
            Some("for_comprehensions")
        }
        _ => None,
    }
}

pub(super) fn inventory_candidates() {
    const PINNED_SCALA_REVISION: &str = "777528f19a58e794c9954a42f433373472ec57f8";
    let scala_root =
        PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT required"));
    assert_eq!(
        git_revision(&scala_root),
        PINNED_SCALA_REVISION,
        "ordinary candidate inventory requires the pinned Scala 3.9.0 source revision"
    );
    let output = PathBuf::from(
        std::env::var_os("DOTTY_ORDINARY_CANDIDATE_INVENTORY")
            .expect("DOTTY_ORDINARY_CANDIDATE_INVENTORY required"),
    );
    let source_root = scala_root.join("library/src/scala");
    let mut files = Vec::new();
    collect_scala_sources(&source_root, &mut files);
    files.sort();

    let mut records = Vec::<(usize, String, u32, String)>::new();
    let mut parse_diagnostics = 0usize;
    let mut naming_failures = 0usize;
    let mut eligible = 0usize;
    let mut excluded_reasons = BTreeMap::<&'static str, usize>::new();
    for path in &files {
        let relative_path = path
            .strip_prefix(&scala_root)
            .expect("source file is under the pinned Scala root")
            .to_string_lossy()
            .replace('\\', "/");
        let text = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("cannot read Scala source {relative_path}: {error}"));
        let source = SourceId::from_index(0);
        let mut store = SemanticStore::new();
        let _definitions = Definitions::bootstrap(&mut store);
        let scanner = ContextualScanner::new(&text)
            .unwrap_or_else(|error| panic!("cannot scan {relative_path}: {error}"));
        let parsed = parse_compilation_unit(
            SourceText::new(&text).expect("Scala source should be valid UTF-8"),
            source,
            scanner,
            &mut store.names,
        );
        parse_diagnostics += parsed.diagnostics.len();
        let mut packages = Packages::new();
        let index = match name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            &relative_path,
            &mut store,
            &mut packages,
        ) {
            Ok(index) => index,
            Err(_) => {
                naming_failures += 1;
                continue;
            }
        };

        let import_count = parsed
            .ast
            .iter()
            .filter(|(_, node)| matches!(node.kind, TreeKind::Import(_)))
            .count();
        let mut declaration_counts = BTreeMap::<(String, String), usize>::new();
        for (tree, node) in parsed.ast.iter() {
            let TreeKind::DefDef(definition) = &node.kind else {
                continue;
            };
            let Some(method) = index.symbol_at(source, tree) else {
                continue;
            };
            let symbol = store.symbols.get(method);
            let owner_path = symbol
                .owner
                .map(|owner| audit_symbol_path(&store, owner))
                .unwrap_or_default();
            let method_name = store
                .names
                .resolve(definition.name.as_name().text())
                .to_owned();
            *declaration_counts
                .entry((owner_path, method_name))
                .or_default() += 1;
        }

        for (tree, node) in parsed.ast.iter() {
            let TreeKind::DefDef(definition) = &node.kind else {
                continue;
            };
            let Some(method) = index.symbol_at(source, tree) else {
                continue;
            };
            let symbol = store.symbols.get(method);
            let canonical = matches!(
                index.definition_of(method),
                Some(SourceDefinition::Canonical {
                    source: canonical_source,
                    tree: canonical_tree
                }) if canonical_source == source && canonical_tree == tree
            );
            let owner_kind = symbol.owner.map(|owner| store.symbols.get(owner).kind);
            if let Some(reason) = member_exclusion(
                symbol.kind,
                symbol.flags,
                owner_kind,
                definition.rhs.is_some(),
                canonical,
            ) {
                *excluded_reasons.entry(reason).or_default() += 1;
                continue;
            }

            let owner_path = symbol
                .owner
                .map(|owner| audit_symbol_path(&store, owner))
                .unwrap_or_default();
            let method_name = store
                .names
                .resolve(definition.name.as_name().text())
                .to_owned();
            let range = node.position.map(|position| position.span().range());
            let Some(range) = range else {
                continue;
            };
            let line = source_line(&text, range.start() as usize);
            let rhs_range = definition
                .rhs
                .and_then(|rhs| parsed.ast.try_get(rhs))
                .and_then(|rhs| rhs.position)
                .map(|position| position.span().range());
            let rhs_text = rhs_range
                .and_then(|range| text.get(range.start() as usize..range.end() as usize))
                .unwrap_or_default();
            let has_placeholder = rhs_text.contains("???")
                || rhs_text.contains("NotImplementedError")
                || rhs_text.contains("??? :");
            let mut rhs_nodes = Vec::<(u32, u32)>::new();
            let mut counts = BTreeMap::<&'static str, usize>::new();
            if let Some(rhs_range) = rhs_range {
                for (_, rhs_node) in parsed.ast.iter() {
                    let Some(position) = rhs_node.position else {
                        continue;
                    };
                    let node_range = position.span().range();
                    if rhs_range.start() > node_range.start() || node_range.end() > rhs_range.end()
                    {
                        continue;
                    }
                    rhs_nodes.push((node_range.start(), node_range.end()));
                    let feature = ast_feature(&rhs_node.kind);
                    if let Some(feature) = feature {
                        *counts.entry(feature).or_default() += 1;
                    }
                }
            }
            rhs_nodes.sort_by_key(|(start, end)| (*start, std::cmp::Reverse(*end)));
            let mut nesting_ends = Vec::<u32>::new();
            let mut max_depth = 0usize;
            for (start, end) in &rhs_nodes {
                while nesting_ends
                    .last()
                    .is_some_and(|parent_end| parent_end < start)
                {
                    nesting_ends.pop();
                }
                nesting_ends.push(*end);
                max_depth = max_depth.max(nesting_ends.len());
            }
            let type_parameters = definition.type_params.len();
            let value_parameters = definition
                .value_param_clauses
                .iter()
                .map(Vec::len)
                .sum::<usize>();
            let defaults = definition
                .value_param_clauses
                .iter()
                .flatten()
                .filter(|parameter| {
                    parsed.ast.try_get(**parameter).is_some_and(|node| {
                        matches!(&node.kind, TreeKind::ValDef(parameter) if parameter.rhs.is_some())
                    })
                })
                .count();
            let result_inferred = parsed
                .ast
                .try_get(definition.tpt)
                .is_some_and(|tpt| matches!(tpt.kind, TreeKind::TypeTree(_)));
            let body_size = rhs_nodes.len();
            let calls = counts.get("calls").copied().unwrap_or(0);
            let selections = counts.get("selections").copied().unwrap_or(0);
            let nested_methods = counts.get("nested_methods").copied().unwrap_or(0);
            let control_flow = counts.get("branches").copied().unwrap_or(0)
                + counts.get("matches").copied().unwrap_or(0) * 2
                + counts.get("tries").copied().unwrap_or(0) * 2
                + counts.get("closures").copied().unwrap_or(0) * 2
                + counts.get("loops").copied().unwrap_or(0) * 2;
            let control_flow =
                control_flow + counts.get("for_comprehensions").copied().unwrap_or(0) * 2;
            let score = body_size
                + max_depth * 4
                + type_parameters * 8
                + value_parameters * 2
                + calls * 3
                + selections * 3
                + nested_methods * 10
                + control_flow * 4
                + defaults * 12
                + usize::from(result_inferred) * 4
                + import_count.min(5)
                + declaration_counts
                    .get(&(owner_path.clone(), method_name.clone()))
                    .copied()
                    .unwrap_or(1)
                    .saturating_sub(1)
                    * 8
                + usize::from(has_placeholder) * 100;
            let tier = if score <= 24 {
                "A0"
            } else if score <= 55 {
                "A1"
            } else {
                "edge"
            };
            let overload_count = declaration_counts
                .get(&(owner_path.clone(), method_name.clone()))
                .copied()
                .unwrap_or(1)
                .saturating_sub(1);
            let feature_text = counts
                .iter()
                .map(|(key, value)| (*key, *value))
                .chain(std::iter::once(("imports", import_count)))
                .chain(std::iter::once(("same_name_overloads", overload_count)))
                .chain(std::iter::once((
                    "placeholder",
                    usize::from(has_placeholder),
                )))
                .filter(|(_, value)| *value > 0)
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(",");
            let feature_text = if feature_text.is_empty() {
                "none".to_owned()
            } else {
                feature_text
            };
            let identity = format!("{relative_path}#tree={}", tree.index());
            let row = format!(
                "{identity}\t{owner_path}\t{method_name}\t{line}\t{}..{}\t{score}\t{tier}\t{body_size}\t{max_depth}\t{type_parameters}\t{value_parameters}\t{}\t{}\t{}\t{feature_text}",
                range.start(),
                range.end(),
                if result_inferred {
                    "inferred"
                } else {
                    "explicit"
                },
                defaults,
                if has_contextual_value_parameter(&parsed.ast, definition) {
                    "contextual"
                } else {
                    "ordinary"
                },
            );
            records.push((score, relative_path.clone(), tree.index(), row));
            eligible += 1;
        }
    }
    records.sort_by(|left, right| (&left.0, &left.1, &left.2).cmp(&(&right.0, &right.1, &right.2)));
    let mut report = format!(
        "scala_revision\t{PINNED_SCALA_REVISION}\nsource_scope\tlibrary/src/scala\nfiles_seen\t{}\nparse_diagnostics\t{parse_diagnostics}\nnamer_failed_files\t{naming_failures}\neligible_ordinary_methods\t{eligible}\n",
        files.len()
    );
    report.push_str("exclusions\n");
    for (reason, count) in excluded_reasons {
        report.push_str(&format!("{reason}\t{count}\n"));
    }
    report.push_str("\nmethod\towner_path\tmethod_name\tline\trange\trank_score\ttier\trhs_nodes\trhs_depth\ttype_params\tvalue_params\tresult_mode\tdefault_params\tparameter_kind\tfeatures\n");
    for (_, _, _, row) in records {
        report.push_str(&row);
        report.push('\n');
    }
    fs::write(&output, report).unwrap_or_else(|error| {
        panic!(
            "cannot write ordinary method candidate inventory {}: {error}",
            output.display()
        )
    });
}

fn has_contextual_value_parameter(
    ast: &dotty_core::ast::AstArena<Untyped>,
    definition: &dotty_core::ast::DefDef<Untyped>,
) -> bool {
    definition
        .value_param_clauses
        .iter()
        .flatten()
        .any(|parameter| {
            ast.try_get(*parameter).is_some_and(|node| {
                matches!(
                    &node.kind,
                    TreeKind::ValDef(parameter)
                        if parameter.metadata.modifiers.contains(&Modifier::Implicit)
                            || parameter.metadata.modifiers.contains(&Modifier::Given)
                )
            })
        })
}

fn method_result_shape(store: &SemanticStore, method: dotty_core::SymbolId) -> String {
    let &SymbolInfo::Complete(mut ty) = store.symbols.info(method) else {
        return String::new();
    };
    loop {
        match store.types.get(ty) {
            Type::Poly(poly) => ty = poly.result,
            Type::Method(method) => ty = method.result,
            _ => return stable_type_shape(store, ty, 0),
        }
    }
}

fn stable_type_shape(store: &SemanticStore, ty: dotty_core::TypeId, depth: usize) -> String {
    if depth >= 16 {
        return "<depth-limit>".to_owned();
    }
    match store.types.get(ty) {
        Type::TypeRef { target, .. } => match target {
            TypeRefTarget::Symbol(symbol) => audit_symbol_path(store, *symbol),
            TypeRefTarget::Name(name) => store.names.resolve(name.as_name().text()).to_owned(),
        },
        Type::Applied { tycon, args } => format!(
            "{}[{}]",
            stable_type_shape(store, *tycon, depth + 1),
            args.iter()
                .map(|argument| stable_type_shape(store, *argument, depth + 1))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Type::Constant(constant) => format!("constant:{constant:?}"),
        Type::ParamRef { index, .. } => format!("typeparam#{index}"),
        Type::JavaArray { element } => {
            format!("Array[{}]", stable_type_shape(store, *element, depth + 1))
        }
        Type::ByName { result } => {
            format!("=> {}", stable_type_shape(store, *result, depth + 1))
        }
        Type::Repeated { element } => {
            format!(
                "Repeated[{}]",
                stable_type_shape(store, *element, depth + 1)
            )
        }
        Type::And { left, right } => format!(
            "{} & {}",
            stable_type_shape(store, *left, depth + 1),
            stable_type_shape(store, *right, depth + 1)
        ),
        Type::Or { left, right } => format!(
            "{} | {}",
            stable_type_shape(store, *left, depth + 1),
            stable_type_shape(store, *right, depth + 1)
        ),
        Type::NoType => "<no-type>".to_owned(),
        Type::Error(_) => "<error>".to_owned(),
        Type::NoPrefix => "<no-prefix>".to_owned(),
        Type::TermRef { .. } => "<term-ref>".to_owned(),
        Type::ThisType { class } => format!("{}.this", audit_symbol_path(store, *class)),
        Type::SuperType { .. } => "<super>".to_owned(),
        Type::Bounds { .. } => "<bounds>".to_owned(),
        Type::AliasingBounds { alias } => stable_type_shape(store, *alias, depth + 1),
        Type::Flexible { underlying } => stable_type_shape(store, *underlying, depth + 1),
        Type::Refined { .. } => "<refined>".to_owned(),
        Type::Recursive { .. } => "<recursive>".to_owned(),
        Type::RecThis { .. } => "<recursive-this>".to_owned(),
        Type::Method(_) => "<method>".to_owned(),
        Type::Poly(_) => "<poly>".to_owned(),
        Type::TypeLambda(_) => "<type-lambda>".to_owned(),
        Type::Match(_) => "<match-type>".to_owned(),
        Type::MatchCase { .. } => "<match-case>".to_owned(),
        Type::Annotated { underlying, .. } => stable_type_shape(store, *underlying, depth + 1),
        Type::Wildcard { .. } => "<wildcard>".to_owned(),
        Type::ClassInfo(_) => "<class-info>".to_owned(),
    }
}

fn collect_scala_sources(directory: &Path, output: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(directory)
        .unwrap_or_else(|error| {
            panic!(
                "cannot read Scala source directory {}: {error}",
                directory.display()
            )
        })
        .map(|entry| entry.expect("directory entry should be readable").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_scala_sources(&path, output);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "scala")
        {
            output.push(path);
        }
    }
}

pub(super) fn run() {
    const PINNED_SCALA_REVISION: &str = "777528f19a58e794c9954a42f433373472ec57f8";
    let scala_root =
        PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT required"));
    assert_eq!(
        git_revision(&scala_root),
        PINNED_SCALA_REVISION,
        "ordinary method audit requires the pinned Scala 3.9.0 source revision"
    );
    let selection_path = PathBuf::from(
        std::env::var_os("DOTTY_ORDINARY_METHOD_SELECTION")
            .expect("DOTTY_ORDINARY_METHOD_SELECTION required"),
    );
    let lines = fs::read_to_string(&selection_path)
        .expect("validated ordinary method selection must be readable")
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let identities = lines.iter().cloned().collect::<BTreeSet<_>>();
    assert_eq!(
        lines.len(),
        identities.len(),
        "duplicate ordinary method identity"
    );
    assert!(!identities.is_empty(), "ordinary method selection is empty");

    let classpath = audit_classpath_from_environment();
    let mut by_source = BTreeMap::<String, BTreeSet<u32>>::new();
    for identity in &identities {
        let (path, tree) = identity
            .rsplit_once("#tree=")
            .expect("validated identity contains #tree=");
        let tree = tree
            .parse::<u32>()
            .expect("validated ordinary method tree index is numeric");
        by_source.entry(path.to_owned()).or_default().insert(tree);
    }

    let mut rows = Vec::<Vec<String>>::new();
    let mut attempts = 0usize;
    let mut typed = 0usize;
    let mut blocked = 0usize;
    let mut excluded = 0usize;
    let mut not_attempted = 0usize;
    for (path, selected_trees) in &by_source {
        let text = fs::read_to_string(scala_root.join(path))
            .unwrap_or_else(|error| panic!("cannot read selected Scala source {path}: {error}"));
        let source = SourceId::from_index(0);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let scanner = ContextualScanner::new(&text)
            .unwrap_or_else(|error| panic!("cannot scan selected Scala source {path}: {error}"));
        let parsed = parse_compilation_unit(
            SourceText::new(&text).expect("Scala source should be valid UTF-8"),
            source,
            scanner,
            &mut store.names,
        );
        let mut packages = Packages::new();
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            path,
            &mut store,
            &mut packages,
        );
        let index = match index {
            Ok(index) => index,
            Err(error) => {
                for tree in selected_trees {
                    rows.push(vec![
                        format!("{path}#tree={tree}"),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        format!("not_attempted::namer_error::{error:?}"),
                        String::new(),
                    ]);
                    not_attempted += 1;
                }
                continue;
            }
        };
        let typer_packages = packages.snapshot();
        let resolver_packages = packages.snapshot();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &typer_packages,
        )
        .with_resolver(Box::new(ClasspathSymbolResolver::new(
            classpath.clone(),
            definitions,
            LoadingSession::with_packages(resolver_packages),
        )));

        for tree_index in selected_trees {
            let identity = format!("{path}#tree={tree_index}");
            let (tree, node) = parsed
                .ast
                .iter()
                .find(|(tree, _)| tree.index() == *tree_index)
                .unwrap_or_else(|| panic!("selected tree does not exist: {identity}"));
            let TreeKind::DefDef(definition) = &node.kind else {
                panic!("selected tree is not a DefDef: {identity}");
            };
            let Some(method) = index.symbol_at(source, tree) else {
                rows.push(vec![
                    identity,
                    String::new(),
                    typer
                        .store()
                        .names
                        .resolve(definition.name.as_name().text())
                        .to_owned(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    "0".to_owned(),
                    "excluded::missing_namer_symbol".to_owned(),
                    String::new(),
                ]);
                excluded += 1;
                continue;
            };
            let symbol = typer.store().symbols.get(method);
            let method_name = typer
                .store()
                .names
                .resolve(definition.name.as_name().text())
                .to_owned();
            let owner = symbol.owner;
            let owner_path = owner
                .map(|owner| audit_symbol_path(typer.store(), owner))
                .unwrap_or_default();
            let span = node.position.map(|position| position.span().range());
            let line = span
                .and_then(|range| usize::try_from(range.start()).ok())
                .map(|start| source_line(&text, start))
                .unwrap_or(0);
            let range = span
                .map(|range| format!("{}..{}", range.start(), range.end()))
                .unwrap_or_default();
            let rhs_range = definition
                .rhs
                .and_then(|rhs| parsed.ast.get(rhs).position)
                .map(|position| position.span().range());
            let complexity = rhs_range.map_or(0, |rhs_range| {
                parsed
                    .ast
                    .iter()
                    .filter(|(_, node)| {
                        node.position.is_some_and(|position| {
                            let range = position.span().range();
                            rhs_range.start() <= range.start() && range.end() <= rhs_range.end()
                        })
                    })
                    .count()
            });
            let expected = parsed
                .ast
                .get(definition.tpt)
                .position
                .map(|position| position.span().range())
                .and_then(|range| text.get(range.start() as usize..range.end() as usize))
                .unwrap_or("")
                .replace(['\t', '\n', '\r'], " ");
            let modifiers =
                format!("{:?}", definition.metadata.modifiers).replace(['\t', '\n', '\r'], " ");

            let canonical = matches!(
                index.definition_of(method),
                Some(SourceDefinition::Canonical {
                    source: canonical_source,
                    tree: canonical_tree
                }) if canonical_source == source && canonical_tree == tree
            );
            let owner_kind = owner.map(|owner| typer.store().symbols.get(owner).kind);
            let exclusion = member_exclusion(
                symbol.kind,
                symbol.flags,
                owner_kind,
                definition.rhs.is_some(),
                canonical,
            );
            if let Some(reason) = exclusion {
                excluded += 1;
                rows.push(vec![
                    identity,
                    owner_path,
                    method_name,
                    line.to_string(),
                    range,
                    modifiers,
                    expected,
                    complexity.to_string(),
                    format!("excluded::{reason}"),
                    String::new(),
                ]);
                continue;
            }

            attempts += 1;
            let outcome = typer.type_method_definition(tree);
            let status = match outcome {
                Ok(typed_tree) => {
                    let typed_method = typer.source_typed_index().get(source, tree);
                    let typed_root = typer.typed_ast().get(typed_tree);
                    let owned_defdef = matches!(typed_root.kind, TreeKind::DefDef(_))
                        && matches!(
                            typer.store().types.get(typed_root.ty),
                            Type::TermRef {
                                target: TermRefTarget::Symbol(typed_symbol),
                                ..
                            } if *typed_symbol == method
                        );
                    let complete = typed_method == Some(typed_tree)
                        && owned_defdef
                        && matches!(typer.store().symbols.info(method), SymbolInfo::Complete(_))
                        && definition.rhs.is_some_and(|rhs| {
                            typer.source_typed_index().get(source, rhs).is_some()
                        })
                        && typer
                            .source_typed_index()
                            .get(source, definition.tpt)
                            .is_some()
                        && definition.type_params.iter().all(|parameter| {
                            typer.source_typed_index().get(source, *parameter).is_some()
                        })
                        && definition
                            .value_param_clauses
                            .iter()
                            .flatten()
                            .all(|parameter| {
                                typer.source_typed_index().get(source, *parameter).is_some()
                            });
                    if complete {
                        typed += 1;
                        format!(
                            "typed::result={}",
                            method_result_shape(typer.store(), method)
                        )
                    } else {
                        blocked += 1;
                        "blocked::missing_typed_method_subtree_mapping".to_owned()
                    }
                }
                Err(error) => {
                    blocked += 1;
                    format!("blocked::{error:?}").replace(['\t', '\n', '\r'], " ")
                }
            };
            rows.push(vec![
                identity,
                owner_path,
                method_name,
                line.to_string(),
                range,
                modifiers,
                expected,
                complexity.to_string(),
                status,
                tree_index.to_string(),
            ]);
        }
    }
    assert!(rows.iter().all(|row| row.len() == 10));
    rows.sort();
    println!("\nORDINARY_METHOD_REPORT_BEGIN");
    println!("schema=1");
    println!("pinned_scala_revision={PINNED_SCALA_REVISION}");
    println!("selected={}", identities.len());
    println!("attempted={attempts}");
    println!("typed={typed}");
    println!("blocked={blocked}");
    println!("excluded={excluded}");
    println!("not_attempted={not_attempted}");
    println!(
        "method\towner_path\tmethod_name\tline\trange\tmodifiers\texpected_result\tcomplexity\tstatus\tattempted_tree"
    );
    for row in rows {
        println!("{}", row.join("\t"));
    }
    println!("ORDINARY_METHOD_REPORT_END");
}

fn member_exclusion(
    kind: SymbolKind,
    flags: SymbolFlags,
    owner_kind: Option<SymbolKind>,
    has_rhs: bool,
    canonical: bool,
) -> Option<&'static str> {
    if kind == SymbolKind::Constructor {
        Some("constructor")
    } else if !canonical {
        Some("noncanonical_source_symbol")
    } else if !has_rhs {
        Some("abstract_or_no_rhs")
    } else if kind != SymbolKind::Method {
        Some("non_method_symbol")
    } else if flags.contains(SymbolFlags::SYNTHETIC) {
        Some("synthetic_or_forwarder")
    } else if flags.contains(SymbolFlags::EXTENSION) {
        Some("extension_method_out_of_scope")
    } else if !matches!(
        owner_kind,
        Some(SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass)
    ) {
        Some("nested_or_non_classlike_owner")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::super::collect_local_nodes;
    use super::{ast_feature, has_contextual_value_parameter, member_exclusion, source_line};
    use dotty_core::ast::{TreeKind, UntypedNode};
    use dotty_core::{
        Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolFlags, SymbolKind,
    };
    use dotty_lexer::ContextualScanner;
    use dotty_namer::name_compilation_unit;
    use dotty_parser::parse_compilation_unit;

    #[test]
    fn ordinary_member_inventory_exclusions_are_explicit() {
        assert_eq!(
            member_exclusion(
                SymbolKind::Method,
                SymbolFlags::EMPTY,
                Some(SymbolKind::ModuleClass),
                true,
                true,
            ),
            None
        );
        assert_eq!(
            member_exclusion(
                SymbolKind::Method,
                SymbolFlags::EMPTY,
                Some(SymbolKind::Method),
                true,
                true,
            ),
            Some("nested_or_non_classlike_owner")
        );
        assert_eq!(
            member_exclusion(
                SymbolKind::Method,
                SymbolFlags::EMPTY,
                Some(SymbolKind::Class),
                false,
                true,
            ),
            Some("abstract_or_no_rhs")
        );
        assert_eq!(
            member_exclusion(
                SymbolKind::Method,
                SymbolFlags::SYNTHETIC,
                Some(SymbolKind::Class),
                true,
                true,
            ),
            Some("synthetic_or_forwarder")
        );
    }

    #[test]
    fn candidate_profile_counts_source_lambdas_tries_and_for_comprehensions() {
        let source_text = "object C { def mapped(xs: List[Int]) = xs.map(x => try x catch { case _: Exception => 0 }); def values(xs: List[Int]) = for (x <- xs) yield x }";
        let source = SourceId::from_index(0);
        let mut store = SemanticStore::new();
        let _definitions = Definitions::bootstrap(&mut store);
        let scanner = ContextualScanner::new(source_text).unwrap();
        let parsed = parse_compilation_unit(
            SourceText::new(source_text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        let features = parsed
            .ast
            .iter()
            .filter_map(|(_, node)| ast_feature(&node.kind))
            .collect::<Vec<_>>();

        assert!(features.contains(&"closures"));
        assert!(features.contains(&"tries"));
        assert!(features.contains(&"for_comprehensions"));
        assert!(parsed.ast.iter().any(|(_, node)| matches!(
            &node.kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        )));
    }

    #[test]
    fn candidate_parameter_kind_uses_parameter_contextual_modifiers() {
        let source_text = "object C { def contextual(using value: Int): Int = value; implicit def ordinary(value: Int): Int = value }";
        let source = SourceId::from_index(0);
        let mut store = SemanticStore::new();
        let _definitions = Definitions::bootstrap(&mut store);
        let scanner = ContextualScanner::new(source_text).unwrap();
        let parsed = parse_compilation_unit(
            SourceText::new(source_text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        let kinds = parsed
            .ast
            .iter()
            .filter_map(|(_, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                let name = store.names.resolve(definition.name.as_name().text());
                (!matches!(name, "<init>" | "$init$")).then_some((
                    name,
                    has_contextual_value_parameter(&parsed.ast, definition),
                ))
            })
            .collect::<Vec<_>>();

        assert_eq!(kinds, [("contextual", true), ("ordinary", false)]);
    }

    #[test]
    fn ordinary_member_report_uses_the_definition_start_line() {
        let source_text = include_str!("../fixtures/ordinary-member-audit/MemberOnly.scala");
        let source = SourceId::from_index(0);
        let mut store = SemanticStore::new();
        let _definitions = Definitions::bootstrap(&mut store);
        let scanner = ContextualScanner::new(source_text).unwrap();
        let parsed = parse_compilation_unit(
            SourceText::new(source_text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        let method_start = parsed
            .ast
            .iter()
            .find_map(|(_, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                (store.names.resolve(definition.name.as_name().text()) == "answer")
                    .then(|| node.position.unwrap().span().range().start() as usize)
            })
            .unwrap();

        assert_eq!(source_line(source_text, method_start), 2);
    }

    #[test]
    fn member_inventory_runs_when_local_method_inventory_is_empty() {
        let source_text = "object C { def answer: Int = 42 }";
        let source = SourceId::from_index(0);
        let mut store = SemanticStore::new();
        let _definitions = Definitions::bootstrap(&mut store);
        let scanner = ContextualScanner::new(source_text).unwrap();
        let parsed = parse_compilation_unit(
            SourceText::new(source_text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        let mut packages = Packages::new();
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "MemberOnly.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        let members = parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                let method = index.symbol_at(source, tree)?;
                let symbol = store.symbols.get(method);
                let canonical = matches!(
                    index.definition_of(method),
                    Some(dotty_core::SourceDefinition::Canonical {
                        source: canonical_source,
                        tree: canonical_tree
                    }) if canonical_source == source && canonical_tree == tree
                );
                member_exclusion(
                    symbol.kind,
                    symbol.flags,
                    symbol.owner.map(|owner| store.symbols.get(owner).kind),
                    definition.rhs.is_some(),
                    canonical,
                )
                .is_none()
                .then(|| {
                    store
                        .names
                        .resolve(definition.name.as_name().text())
                        .to_owned()
                })
            })
            .collect::<Vec<_>>();

        assert_eq!(members, ["answer"]);
        assert_eq!(collect_local_nodes(&parsed.ast).local_defdefs, 0);
    }

    #[test]
    fn ordinary_member_inventory_is_separate_from_the_frozen_local_inventory() {
        let source_text =
            "object C {\n  def member: Int = 1\n  def outer: Int = { def local = 1; local }\n}";
        let source = SourceId::from_index(0);
        let mut store = SemanticStore::new();
        let _definitions = Definitions::bootstrap(&mut store);
        let scanner = ContextualScanner::new(source_text).unwrap();
        let parsed = parse_compilation_unit(
            SourceText::new(source_text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        let mut packages = Packages::new();
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "SeparatedInventories.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        let ordinary_names = parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                let method = index.symbol_at(source, tree)?;
                let symbol = store.symbols.get(method);
                let canonical = matches!(
                    index.definition_of(method),
                    Some(dotty_core::SourceDefinition::Canonical {
                        source: canonical_source,
                        tree: canonical_tree
                    }) if canonical_source == source && canonical_tree == tree
                );
                member_exclusion(
                    symbol.kind,
                    symbol.flags,
                    symbol.owner.map(|owner| store.symbols.get(owner).kind),
                    definition.rhs.is_some(),
                    canonical,
                )
                .is_none()
                .then(|| {
                    store
                        .names
                        .resolve(definition.name.as_name().text())
                        .to_owned()
                })
            })
            .collect::<Vec<_>>();
        let local_trees = super::super::local_method_trees(&parsed.ast);

        assert_eq!(ordinary_names, ["member", "outer"]);
        assert_eq!(local_trees.len(), 1);
        let TreeKind::DefDef(local) = &parsed.ast.get(local_trees[0].0).kind else {
            panic!("the frozen local inventory must contain the nested def");
        };
        assert_eq!(store.names.resolve(local.name.as_name().text()), "local");
        assert_eq!(collect_local_nodes(&parsed.ast).local_defdefs, 1);
    }
}
