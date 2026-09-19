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

    let mut rendered_children = child_ids(&tree.kind, arena)
        .into_iter()
        .map(|child| render_tree(child, arena, names, source))
        .collect::<Vec<_>>();
    if let TreeKind::Super(super_tree) = &tree.kind
        && matches!(arena.get(super_tree.qual).kind, TreeKind::This(this) if this.qual.is_none())
        && let Some(position) = tree.position
    {
        let start = position.span().range().start();
        rendered_children[0] = render_synthetic_this(start);
    }
    if let TreeKind::Super(super_tree) = &tree.kind
        && let Some(mix) = super_tree.mix
        && let Some(span) = find_name_span(tree, names.resolve(mix.text()), source)
    {
        rendered_children.push(render_synthetic_ident(names.resolve(mix.text()), span));
    }
    if let TreeKind::This(this_tree) = &tree.kind
        && let Some(qual) = this_tree.qual
        && let Some(span) = find_this_qualifier_span(tree, names.resolve(qual.text()), source)
    {
        rendered_children.push(render_synthetic_ident(names.resolve(qual.text()), span));
    }
    fields.push(format!("\"children\":[{}]", rendered_children.join(",")));
    format!("{{{}}}", fields.join(","))
}

fn kind_name(kind: &TreeKind<Untyped>) -> &'static str {
    match kind {
        TreeKind::Ident(_) => "Ident",
        TreeKind::Select(_) => "Select",
        TreeKind::Apply(_) => "Apply",
        TreeKind::This(_) => "This",
        TreeKind::Super(_) => "Super",
        TreeKind::New(_) => "New",
        TreeKind::TypeApply(_) => "TypeApply",
        TreeKind::Block(_) => "Block",
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

fn child_ids(kind: &TreeKind<Untyped>, arena: &AstArena<Untyped>) -> Vec<TreeId<Untyped>> {
    match kind {
        TreeKind::This(_) => Vec::new(),
        TreeKind::Super(super_tree) => vec![super_tree.qual],
        TreeKind::New(new_tree) => vec![new_tree.tpt],
        TreeKind::Select(selection) => vec![selection.qualifier],
        TreeKind::TypeApply(type_apply) => {
            let mut children = Vec::with_capacity(type_apply.args.len() + 1);
            children.push(type_apply.function);
            children.extend(type_apply.args.iter().copied());
            children
        }
        TreeKind::Apply(application) => {
            let mut children = Vec::with_capacity(application.args.len() + 1);
            children.push(application.function);
            children.extend(application.args.iter().copied());
            children
        }
        TreeKind::Block(block) => {
            let mut children = Vec::with_capacity(block.stats.len() + 1);
            children.extend(block.stats.iter().copied());
            let include_expr = match arena.get(block.expr).position {
                Some(position) => {
                    let range = position.span().range();
                    range.start() != range.end()
                }
                None => false,
            };
            if include_expr {
                children.push(block.expr);
            }
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

fn find_name_span(tree: &Tree<Untyped>, name: &str, source: &str) -> Option<(u32, u32)> {
    let position = tree.position?;
    let range = position.span().range();
    let start = range.start() as usize;
    let end = range.end() as usize;
    let offset = source.get(start..end)?.find(name)?;
    let start = start.checked_add(offset)?;
    let end = start.checked_add(name.len())?;
    Some((start as u32, end as u32))
}

fn find_this_qualifier_span(tree: &Tree<Untyped>, name: &str, source: &str) -> Option<(u32, u32)> {
    let (start, mut end) = find_name_span(tree, name, source)?;
    if source.as_bytes().get(end as usize) == Some(&b'.') {
        end += 1;
    }
    Some((start, end))
}

fn render_synthetic_ident(name: &str, span: (u32, u32)) -> String {
    format!(
        "{{\"kind\":\"Ident\",\"span\":{{\"start\":{},\"end\":{}}},\"name\":{},\"children\":[]}}",
        span.0,
        span.1,
        quote(name)
    )
}

fn render_synthetic_this(start: u32) -> String {
    format!("{{\"kind\":\"This\",\"span\":{{\"start\":{start},\"end\":{start}}},\"children\":[]}}")
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
