use std::{env, fs, process};

use dotty_core::ast::{AstArena, Untyped, UntypedNode};
use dotty_core::{NameInterner, SourceId, SourceText, Tree, TreeId, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::parse_compilation_unit;

fn main() {
    let Some(path) = env::args().nth(1) else {
        eprintln!("usage: dotty-parser-smoke-dump <source-file>");
        process::exit(2);
    };

    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("failed to read {path}: {error}");
            process::exit(1);
        }
    };
    let scanner = match ContextualScanner::new(&source) {
        Ok(scanner) => scanner,
        Err(error) => {
            eprintln!("failed to scan {path}: {error}");
            process::exit(1);
        }
    };
    if !scanner.diagnostics().is_empty() {
        eprintln!("scanner reported diagnostics for {path}");
        process::exit(1);
    }

    let source_text = SourceText::new(&source).expect("fixture source must fit in 32-bit offsets");
    let mut names = NameInterner::new();
    let result = parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);
    if !result.diagnostics.is_empty() {
        eprintln!(
            "parser reported diagnostics for {path}: {:?}",
            result.diagnostics
        );
        process::exit(1);
    }

    let expression = match &result.ast.get(result.root).kind {
        TreeKind::Block(block) if block.stats.is_empty() => block.expr,
        _ => {
            eprintln!("smoke dump requires a single expression in {path}");
            process::exit(1);
        }
    };
    println!("{}", render_tree(expression, &result.ast, &names, &source));
}

fn render_tree(
    id: TreeId<Untyped>,
    arena: &AstArena<Untyped>,
    names: &NameInterner,
    source: &str,
) -> String {
    let tree = arena.get(id);
    let mut fields = Vec::new();
    fields.push(format!("\"kind\":{}", quote(kind_name(&tree.kind))));
    fields.push(format!("\"span\":{}", render_span(tree)));

    match &tree.kind {
        TreeKind::Ident(ident) => {
            fields.push(format!(
                "\"name\":{}",
                quote(names.resolve(ident.name.text()))
            ));
            if ident.backquoted {
                fields.push("\"backquoted\":true".to_owned());
            }
        }
        TreeKind::Select(selection) => {
            fields.push(format!(
                "\"name\":{}",
                quote(names.resolve(selection.name.text()))
            ));
            if selection.backquoted {
                fields.push("\"backquoted\":true".to_owned());
            }
        }
        TreeKind::Literal(_) | TreeKind::PhaseSpecific(UntypedNode::Number(_)) => {
            fields.push(format!("\"literal\":{}", quote(source_slice(tree, source))));
        }
        _ => {}
    }

    let children = child_ids(&tree.kind);
    fields.push(format!(
        "\"children\":[{}]",
        children
            .into_iter()
            .map(|child| render_tree(child, arena, names, source))
            .collect::<Vec<_>>()
            .join(",")
    ));
    format!("{{{}}}", fields.join(","))
}

fn kind_name(kind: &TreeKind<Untyped>) -> &'static str {
    match kind {
        TreeKind::Ident(_) => "Ident",
        TreeKind::Select(_) => "Select",
        TreeKind::Apply(_) => "Apply",
        TreeKind::This(_) => "This",
        TreeKind::Literal(_) => "Literal",
        TreeKind::PhaseSpecific(UntypedNode::Number(_)) => "Number",
        TreeKind::PhaseSpecific(UntypedNode::Parens(_)) => "Parens",
        TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) if tuple.elements.is_empty() => {
            "Literal"
        }
        TreeKind::PhaseSpecific(UntypedNode::Tuple(_)) => "Tuple",
        TreeKind::PhaseSpecific(UntypedNode::Error(_)) => "Error",
        _ => "Unsupported",
    }
}

fn child_ids(kind: &TreeKind<Untyped>) -> Vec<TreeId<Untyped>> {
    match kind {
        TreeKind::This(_) => Vec::new(),
        TreeKind::Select(selection) => vec![selection.qualifier],
        TreeKind::Apply(application) => {
            let mut children = Vec::with_capacity(application.args.len() + 1);
            children.push(application.function);
            children.extend(application.args.iter().copied());
            children
        }
        TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => vec![parens.inner],
        TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) => tuple.elements.clone(),
        _ => Vec::new(),
    }
}

fn render_span(tree: &Tree<Untyped>) -> String {
    let Some(position) = tree.position else {
        return "null".to_owned();
    };
    let range = position.span().range();
    format!("{{\"start\":{},\"end\":{}}}", range.start(), range.end())
}

fn source_slice(tree: &Tree<Untyped>, source: &str) -> String {
    let Some(position) = tree.position else {
        return String::new();
    };
    let range = position.span().range();
    source
        .get(range.start() as usize..range.end() as usize)
        .unwrap_or("")
        .to_owned()
}

fn quote(value: impl AsRef<str>) -> String {
    let mut escaped = String::new();
    for character in value.as_ref().chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character => escaped.push(character),
        }
    }
    format!("\"{escaped}\"")
}
