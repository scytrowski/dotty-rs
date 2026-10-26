use super::{audit_classpath_from_environment, audit_symbol_path, git_revision};
use dotty_classloader::classloader::{ClasspathSymbolResolver, LoadingSession};
use dotty_core::ast::TreeKind;
use dotty_core::types::{TermRefTarget, Type};
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
use std::path::PathBuf;

fn source_line(text: &str, start: usize) -> usize {
    text.get(..start)
        .map(|prefix| prefix.bytes().filter(|byte| *byte == b'\n').count() + 1)
        .unwrap_or(0)
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
                        "typed".to_owned()
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
    println!("ORDINARY_METHOD_REPORT_BEGIN");
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
    use super::{member_exclusion, source_line};
    use dotty_core::ast::TreeKind;
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
