use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use dotty_core::ast::{TreeKind, Untyped, UntypedNode};
use dotty_core::{Definitions, Packages, SemanticStore, SourceId, SourceText, TextRange};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_parser::parse_compilation_unit;
use dotty_typer::SourceTyper;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Audit {
    local_definitions: usize,
    buckets: BTreeMap<String, usize>,
    local_defdefs: usize,
    typed_local_defdefs: usize,
    failures: BTreeMap<String, FailureBucket>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FailureBucket {
    count: usize,
    examples: BTreeSet<String>,
}

impl Audit {
    fn merge(&mut self, other: Self) {
        self.local_definitions += other.local_definitions;
        self.local_defdefs += other.local_defdefs;
        self.typed_local_defdefs += other.typed_local_defdefs;
        for (name, count) in other.buckets {
            *self.buckets.entry(name).or_default() += count;
        }
        for (name, bucket) in other.failures {
            let target = self.failures.entry(name).or_default();
            target.count += bucket.count;
            target.examples.extend(bucket.examples);
            target.examples = target.examples.iter().take(5).cloned().collect();
        }
    }
}

#[test]
#[ignore = "run with SCALA39_ROOT=/path/to/pinned/scala3 checkout to regenerate the compatibility audit"]
fn pinned_scala39_local_definition_audit() {
    let root = PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT is required"));
    let revision = git_revision(&root);
    assert_eq!(
        revision, "777528f19a58e794c9954a42f433373472ec57f8",
        "audit requires the repository's pinned Scala 3.9.0 source revision"
    );
    let files = scala_files(&[root.join("library/src"), root.join("compiler/src")]);
    let mut audit = Audit::default();
    for file in files {
        let relative = file
            .strip_prefix(&root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(&file).expect("Scala source should be readable");
        audit.merge(audit_source(&source, &relative));
    }

    println!("scala_revision={revision}");
    println!(
        "files={}",
        scala_files(&[root.join("library/src"), root.join("compiler/src")]).len()
    );
    println!("local_definitions={}", audit.local_definitions);
    for (bucket, count) in &audit.buckets {
        println!("{bucket}={count}");
    }
    println!("local_defdefs={}", audit.local_defdefs);
    println!("typed_local_defdefs={}", audit.typed_local_defdefs);
    println!("top_local_defdef_failures:");
    let mut failures = audit.failures.into_iter().collect::<Vec<_>>();
    failures.sort_by(|(name_a, a), (name_b, b)| b.count.cmp(&a.count).then(name_a.cmp(name_b)));
    for (name, bucket) in failures.into_iter().take(5) {
        println!(
            "  {name}: {} [{}]",
            bucket.count,
            bucket.examples.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
}

#[test]
fn local_definition_audit_is_deterministic() {
    let source =
        "object Audit { def outer: Int = { val n = 1; def local[A](x: A) = x; local(n) } }";
    let first = audit_source(source, "Audit.scala");
    let second = audit_source(source, "Audit.scala");
    assert_eq!(first, second);
    assert_eq!(first.local_defdefs, 1);
    assert_eq!(first.typed_local_defdefs, 1);
    assert_eq!(first.buckets.get("local_val_defs"), Some(&1));
}

fn audit_source(text: &str, path: &str) -> Audit {
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = match ContextualScanner::new(text) {
        Ok(scanner) => scanner,
        Err(_) => return Audit::default(),
    };
    let parsed = parse_compilation_unit(
        SourceText::new(text).expect("source text should be valid"),
        source,
        scanner,
        &mut store.names,
    );
    let mut packages = Packages::new();
    let index = match name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        path,
        &mut store,
        &mut packages,
    ) {
        Ok(index) => index,
        Err(error) => {
            let mut audit = collect_local_nodes(&parsed.ast);
            if audit.local_defdefs != 0 {
                for _ in 0..audit.local_defdefs {
                    record_failure(
                        &mut audit,
                        format!("NamerError::{}", error_kind(&error)),
                        path,
                    );
                }
            }
            return audit;
        }
    };
    let mut audit = collect_local_nodes(&parsed.ast);
    if audit.local_defdefs == 0 {
        return audit;
    }

    let root_methods = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            let rhs = definition.rhs?;
            let method = index.symbol_at(source, tree)?;
            let range = parsed.ast.get(rhs).position?.span().range();
            Some((method, rhs, range))
        })
        .collect::<Vec<_>>();

    let rhs_ranges = method_rhs_ranges(&parsed.ast);
    let local_method_trees = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            matches!(node.kind, TreeKind::DefDef(_))
                .then_some((tree, node.position?.span().range()))
        })
        .filter(|(_, range)| {
            rhs_ranges
                .iter()
                .any(|parent| parent.start() <= range.start() && range.end() <= parent.end())
        })
        .collect::<Vec<_>>();

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let mut root_failures = Vec::new();
    for (method, rhs, range) in root_methods {
        let outcome = typer
            .expression_context_for(method)
            .and_then(|context| typer.type_expression(rhs, context));
        if let Err(error) = outcome {
            root_failures.push((range, error_kind(&error)));
        }
    }

    for (tree, range) in local_method_trees {
        if typer.source_typed_index().get(source, tree).is_some() {
            audit.typed_local_defdefs += 1;
        } else {
            let kind = root_failures
                .iter()
                .filter(|(parent, _)| {
                    parent.start() <= range.start() && range.end() <= parent.end()
                })
                .min_by_key(|(parent, _)| parent.end().saturating_sub(parent.start()))
                .map(|(_, kind)| kind.clone())
                .unwrap_or_else(|| "NoSuccessfulEnclosingMethodTyping".to_owned());
            record_failure(&mut audit, kind, path);
        }
    }
    audit
}

fn collect_local_nodes(arena: &dotty_core::AstArena<Untyped>) -> Audit {
    let ranges = method_rhs_ranges(arena);
    let parameter_trees = arena
        .iter()
        .flat_map(|(_, node)| match &node.kind {
            TreeKind::DefDef(definition) => definition
                .type_params
                .iter()
                .copied()
                .chain(definition.value_param_clauses.iter().flatten().copied())
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect::<HashSet<_>>();
    let mut audit = Audit::default();
    for (tree, node) in arena.iter() {
        if parameter_trees.contains(&tree) {
            continue;
        }
        let Some(position) = node.position else {
            continue;
        };
        let range = position.span().range();
        if !ranges
            .iter()
            .any(|parent| parent.start() <= range.start() && range.end() <= parent.end())
        {
            continue;
        }
        let bucket = match &node.kind {
            TreeKind::ValDef(_) => Some("local_val_defs"),
            TreeKind::DefDef(_) => Some("local_def_defs"),
            TreeKind::TypeDef(definition) => {
                if matches!(arena.get(definition.rhs).kind, TreeKind::Template(_)) {
                    Some("local_classes")
                } else {
                    Some("local_type_defs")
                }
            }
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_)) => Some("local_objects"),
            TreeKind::Import(_) => Some("local_imports"),
            TreeKind::Bind(_) => Some("local_pattern_bindings"),
            _ => None,
        };
        if let Some(bucket) = bucket {
            *audit.buckets.entry(bucket.to_owned()).or_default() += 1;
            audit.local_definitions += 1;
            if bucket == "local_def_defs" {
                audit.local_defdefs += 1;
            }
        }
    }
    audit
}

fn method_rhs_ranges(arena: &dotty_core::AstArena<Untyped>) -> Vec<TextRange> {
    arena
        .iter()
        .filter_map(|(_, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            let rhs = definition.rhs?;
            Some(arena.get(rhs).position?.span().range())
        })
        .collect()
}

fn record_failure(audit: &mut Audit, kind: String, path: &str) {
    let bucket = audit.failures.entry(kind).or_default();
    bucket.count += 1;
    if bucket.examples.len() < 5 {
        bucket.examples.insert(path.to_owned());
    }
}

fn error_kind(error: &impl std::fmt::Debug) -> String {
    format!("{error:?}")
        .split(['{', '(', ' '])
        .next()
        .unwrap_or("TyperError")
        .to_owned()
}

fn scala_files(roots: &[PathBuf]) -> Vec<PathBuf> {
    fn visit(path: &Path, files: &mut Vec<PathBuf>) {
        let Ok(metadata) = fs::metadata(path) else {
            return;
        };
        if metadata.is_file() {
            if path
                .extension()
                .is_some_and(|extension| extension == "scala")
            {
                files.push(path.to_owned());
            }
            return;
        }
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        let mut entries = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        entries.sort();
        for entry in entries {
            visit(&entry, files);
        }
    }
    let mut files = Vec::new();
    for root in roots {
        visit(root, &mut files);
    }
    files.sort();
    files.dedup();
    files
}

fn git_revision(root: &Path) -> String {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git should be available")
        .stdout
        .into_iter()
        .map(char::from)
        .collect::<String>()
        .trim()
        .to_owned()
}
