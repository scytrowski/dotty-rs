use std::{env, fs, process};

use dotty_core::ast::{ApplyKind, AstArena, Untyped, UntypedNode};
use dotty_core::{NameInterner, SourceId, SourceText, Tree, TreeId, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::{parse_expression_fragment, parse_pattern_fragment};

fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    if let [flag, manifest] = args.as_slice()
        && flag == "--batch"
    {
        run_batch(manifest);
        return;
    }

    let (mode, path) = match args.as_slice() {
        [path] => ("expr", path.as_str()),
        [flag, mode, path] if flag == "--mode" && mode == "pattern" => ("pattern", path.as_str()),
        _ => {
            eprintln!(
                "usage: dotty-parser-smoke-dump [--mode pattern] <source-file> | --batch manifest"
            );
            process::exit(2);
        }
    };

    if path.is_empty() {
        eprintln!(
            "usage: dotty-parser-smoke-dump [--mode pattern] <source-file> | --batch manifest"
        );
        process::exit(2);
    }

    match dump_fixture(mode, path) {
        Ok(output) => println!("{output}"),
        Err(error) => {
            eprintln!("{error}");
            process::exit(1);
        }
    }
}

fn run_batch(manifest: &str) {
    let contents = match fs::read_to_string(manifest) {
        Ok(contents) => contents,
        Err(error) => {
            eprintln!("failed to read batch manifest {manifest}: {error}");
            process::exit(1);
        }
    };

    for (line_number, line) in contents.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let Some((mode, path)) = line.split_once('\t') else {
            eprintln!("invalid batch manifest entry at line {}", line_number + 1);
            process::exit(2);
        };
        match dump_fixture(mode, path) {
            Ok(output) => println!("{output}"),
            Err(error) => {
                eprintln!("{error}");
                process::exit(1);
            }
        }
    }
}

fn dump_fixture(mode: &str, path: &str) -> Result<String, String> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => return Err(format!("failed to read {path}: {error}")),
    };
    let scanner = match ContextualScanner::new(&source) {
        Ok(scanner) => scanner,
        Err(error) => return Err(format!("failed to scan {path}: {error}")),
    };
    if !scanner.diagnostics().is_empty() {
        return Err(format!("scanner reported diagnostics for {path}"));
    }

    let source_text = SourceText::new(&source)
        .map_err(|error| format!("failed to create source text for {path}: {error}"))?;
    let mut names = NameInterner::new();
    let result = if mode == "pattern" {
        parse_pattern_fragment(source_text, SourceId::from_index(0), scanner, &mut names)
    } else {
        parse_expression_fragment(source_text, SourceId::from_index(0), scanner, &mut names)
    };
    if !result.diagnostics.is_empty() {
        return Err(format!(
            "parser reported diagnostics for {path}: {:?}",
            result.diagnostics
        ));
    }

    let tree = if mode == "pattern" || mode == "expr" || mode == "block" {
        result.root
    } else {
        match &result.ast.get(result.root).kind {
            TreeKind::Block(block) if mode == "block" && block.stats.is_empty() => block.expr,
            TreeKind::Block(_) if mode == "block" => result.root,
            TreeKind::Block(block) if block.stats.is_empty() => block.expr,
            TreeKind::Block(block)
                if block.stats.len() == 1 && is_synthetic_unit(&result.ast, block.expr) =>
            {
                block.stats[0]
            }
            _ => {
                return Err(format!(
                    "expression dump requires a single expression in {path}"
                ));
            }
        }
    };
    Ok(render_tree(tree, &result.ast, &names, &source))
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
            let name = names.resolve(ident.name.text());
            fields.push(format!(
                "\"name\":{}",
                quote(normalize_placeholder_name(
                    name,
                    &source_slice(tree, source)
                ))
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
        TreeKind::NamedArg(named) => {
            fields.push(format!(
                "\"name\":{}",
                quote(names.resolve(named.name.text()))
            ));
        }
        TreeKind::Apply(application) => {
            fields.push(format!(
                "\"apply_kind\":{}",
                quote(match application.kind {
                    ApplyKind::Regular => "Regular",
                    ApplyKind::Using => "Using",
                })
            ));
        }
        TreeKind::Bind(bind) => {
            fields.push(format!(
                "\"name\":{}",
                quote(names.resolve(bind.name.text()))
            ));
        }
        TreeKind::TypeDef(definition) => {
            let name = names.resolve(definition.name.as_name().text());
            let source_text = source_slice(tree, source);
            fields.push(format!(
                "\"name\":{}",
                quote(if is_wildcard_type_param_source(&source_text) {
                    "$type_wildcard"
                } else {
                    name
                })
            ));
            if definition
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::Trait)
            {
                fields.push("\"trait\":true".to_owned());
            }
        }
        TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) => {
            fields.push(format!(
                "\"name\":{}",
                quote(names.resolve(module.name.as_name().text()))
            ));
        }
        TreeKind::ValDef(definition) => {
            let source_text = source_slice(tree, source);
            let name = names.resolve(definition.name.as_name().text());
            if !source_text.trim_start().starts_with('_') && !name.starts_with("$lambda_wildcard_")
            {
                fields.push(format!("\"name\":{}", quote(name)));
            }
            if definition
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::Given)
            {
                fields.push("\"given\":true".to_owned());
            }
        }
        TreeKind::DefDef(definition) => {
            fields.push(format!(
                "\"name\":{}",
                quote(names.resolve(definition.name.as_name().text()))
            ));
            fields.push(format!(
                "\"type_param_count\":{}",
                definition.type_params.len()
            ));
            let mut clause_sizes = Vec::with_capacity(
                usize::from(!definition.type_params.is_empty())
                    + definition.value_param_clauses.len(),
            );
            let mut using_clauses = Vec::with_capacity(clause_sizes.capacity());
            if !definition.type_params.is_empty() {
                clause_sizes.push(definition.type_params.len().to_string());
                using_clauses.push("false".to_owned());
            }
            for clause in &definition.value_param_clauses {
                clause_sizes.push(clause.len().to_string());
                let using = clause.first().is_some_and(|parameter| {
                    matches!(
                        &arena.get(*parameter).kind,
                        TreeKind::ValDef(value)
                            if value.metadata.modifiers.contains(&dotty_core::ast::Modifier::Given)
                    )
                });
                using_clauses.push(using.to_string());
            }
            fields.push(format!(
                "\"param_clause_sizes\":[{}]",
                clause_sizes.join(",")
            ));
            fields.push(format!("\"using_clauses\":[{}]", using_clauses.join(",")));
        }
        TreeKind::PhaseSpecific(UntypedNode::PatDef(_)) => {}
        TreeKind::Alternative(_) | TreeKind::Typed(_) => {}
        TreeKind::Literal(_) | TreeKind::PhaseSpecific(UntypedNode::Number(_)) => {
            fields.push(format!("\"literal\":{}", quote(source_slice(tree, source))));
        }
        TreeKind::PhaseSpecific(UntypedNode::PrefixOp(prefix)) => {
            fields.push(format!(
                "\"operator\":{}",
                quote(names.resolve(prefix.op.text()))
            ));
        }
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) => {
            fields.push(format!(
                "\"operator\":{}",
                quote(names.resolve(infix.op.text()))
            ));
        }
        TreeKind::PhaseSpecific(UntypedNode::PostfixOp(postfix)) => {
            fields.push(format!(
                "\"operator\":{}",
                quote(names.resolve(postfix.op.text()))
            ));
        }
        TreeKind::PhaseSpecific(UntypedNode::GenFrom(generator)) => {
            fields.push(format!(
                "\"check_mode\":{}",
                quote(match generator.check_mode {
                    dotty_core::ast::GenCheckMode::Ignore => "Ignore",
                    dotty_core::ast::GenCheckMode::Filtered => "Filtered",
                    dotty_core::ast::GenCheckMode::Check => "Check",
                    dotty_core::ast::GenCheckMode::CheckAndFilter => "CheckAndFilter",
                    dotty_core::ast::GenCheckMode::FilterNow => "FilterNow",
                    dotty_core::ast::GenCheckMode::FilterAlways => "FilterAlways",
                })
            ));
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
        && let Some(span) = find_mix_span(tree, names.resolve(mix.text()), source)
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
        TreeKind::NamedArg(_) => "NamedArg",
        TreeKind::Bind(_) => "Bind",
        TreeKind::Alternative(_) => "Alternative",
        TreeKind::Typed(_) => "Typed",
        TreeKind::Assign(_) => "Assign",
        TreeKind::ValDef(_) => "ValDef",
        TreeKind::DefDef(_) => "DefDef",
        TreeKind::PhaseSpecific(UntypedNode::PatDef(_)) => "PatDef",
        TreeKind::TypeDef(_) => "TypeDef",
        TreeKind::Template(_) => "Template",
        TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_)) => "ModuleDef",
        TreeKind::LambdaTypeTree(_) => "LambdaTypeTree",
        TreeKind::TypeBoundsTree(_) => "TypeBoundsTree",
        TreeKind::TypeTree(_) => "TypeTree",
        TreeKind::If(_) => "If",
        TreeKind::While(_) => "While",
        TreeKind::Match(_) => "Match",
        TreeKind::CaseDef(_) => "CaseDef",
        TreeKind::This(_) => "This",
        TreeKind::Super(_) => "Super",
        TreeKind::New(_) => "New",
        TreeKind::TypeApply(_) => "TypeApply",
        TreeKind::Block(_) => "Block",
        TreeKind::Literal(_) => "Literal",
        TreeKind::PhaseSpecific(UntypedNode::Number(_)) => "Number",
        TreeKind::PhaseSpecific(UntypedNode::Parens(_)) => "Parens",
        TreeKind::PhaseSpecific(UntypedNode::PrefixOp(_)) => "PrefixOp",
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)) => "InfixOp",
        TreeKind::PhaseSpecific(UntypedNode::PostfixOp(_)) => "PostfixOp",
        TreeKind::PhaseSpecific(UntypedNode::Function(_)) => "Function",
        TreeKind::PhaseSpecific(UntypedNode::PolyFunction(_)) => "PolyFunction",
        TreeKind::PhaseSpecific(UntypedNode::ForYield(_)) => "ForYield",
        TreeKind::PhaseSpecific(UntypedNode::ForDo(_)) => "ForDo",
        TreeKind::PhaseSpecific(UntypedNode::GenFrom(_)) => "GenFrom",
        TreeKind::PhaseSpecific(UntypedNode::GenAlias(_)) => "GenAlias",
        TreeKind::PhaseSpecific(UntypedNode::Throw(_)) => "Throw",
        TreeKind::PhaseSpecific(UntypedNode::ParsedTry(_)) => "ParsedTry",
        TreeKind::Return(_) => "Return",
        TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) if tuple.elements.is_empty() => {
            "Literal"
        }
        TreeKind::PhaseSpecific(UntypedNode::Tuple(_)) => "Tuple",
        TreeKind::PhaseSpecific(UntypedNode::Error(_)) => "Error",
        _ => "Unsupported",
    }
}

fn normalize_placeholder_name(name: &str, source_text: &str) -> String {
    if source_text == "_" {
        return name
            .strip_prefix("$lambda_wildcard_")
            .map(|index| format!("$placeholder_{index}"))
            .unwrap_or_else(|| name.to_owned());
    }
    name.to_owned()
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
        TreeKind::NamedArg(named) => vec![named.arg],
        TreeKind::Bind(bind) => vec![bind.body],
        TreeKind::Alternative(alternative) => alternative.alternatives.clone(),
        TreeKind::Typed(typed) => vec![typed.expr, typed.tpt],
        TreeKind::Assign(assignment) => vec![assignment.lhs, assignment.rhs],
        TreeKind::If(if_tree) => {
            let mut children = vec![if_tree.cond, if_tree.then_branch];
            let include_else = arena
                .get(if_tree.else_branch)
                .position
                .is_some_and(|position| {
                    let range = position.span().range();
                    range.start() != range.end()
                });
            if include_else {
                children.push(if_tree.else_branch);
            }
            children
        }
        TreeKind::While(while_tree) => vec![while_tree.cond, while_tree.body],
        TreeKind::Match(match_tree) => {
            let mut children = Vec::with_capacity(match_tree.cases.len() + 1);
            let include_selector =
                arena
                    .get(match_tree.selector)
                    .position
                    .is_some_and(|position| {
                        let range = position.span().range();
                        range.start() != range.end()
                    });
            if include_selector {
                children.push(match_tree.selector);
            }
            children.extend(match_tree.cases.iter().copied());
            children
        }
        TreeKind::CaseDef(case_def) => {
            let mut children = vec![case_def.pattern];
            if let Some(guard) = case_def.guard {
                children.push(guard);
            }
            children.push(case_def.body);
            children
        }
        TreeKind::ValDef(definition) => {
            let mut children = vec![definition.tpt];
            if let Some(rhs) = definition.rhs {
                children.push(rhs);
            }
            children
        }
        TreeKind::DefDef(definition) => {
            let mut children = definition.type_params.clone();
            for clause in &definition.value_param_clauses {
                children.extend(clause.iter().copied());
            }
            children.push(definition.tpt);
            if let Some(rhs) = definition.rhs {
                children.push(rhs);
            }
            children
        }
        TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) => {
            let mut children = definition.patterns.clone();
            children.push(definition.tpt);
            if let Some(rhs) = definition.rhs {
                children.push(rhs);
            }
            children
        }
        TreeKind::TypeDef(definition) => vec![definition.rhs],
        TreeKind::Template(template) => {
            let mut children = Vec::with_capacity(
                1 + template.parents.len()
                    + usize::from(template.self_val.is_some())
                    + template.body.len(),
            );
            children.push(template.constructor);
            children.extend(template.parents.iter().copied());
            if let Some(self_val) = template.self_val {
                children.push(self_val);
            }
            children.extend(template.body.iter().copied());
            children
        }
        TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) => vec![module.template],
        TreeKind::LambdaTypeTree(lambda) => {
            let mut children = lambda.type_params.clone();
            children.push(lambda.body);
            children
        }
        TreeKind::TypeBoundsTree(bounds) => bounds
            .low
            .into_iter()
            .chain(bounds.high)
            .chain(bounds.alias)
            .collect(),
        TreeKind::TypeTree(_) => Vec::new(),
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
        TreeKind::PhaseSpecific(UntypedNode::PrefixOp(prefix)) => vec![prefix.operand],
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) => vec![infix.left, infix.right],
        TreeKind::PhaseSpecific(UntypedNode::PostfixOp(postfix)) => vec![postfix.operand],
        TreeKind::PhaseSpecific(UntypedNode::Function(function)) => {
            let mut children = function.params.clone();
            children.push(function.body);
            children
        }
        TreeKind::PhaseSpecific(UntypedNode::PolyFunction(function)) => {
            let mut children = function.type_params.clone();
            children.push(function.body);
            children
        }
        TreeKind::PhaseSpecific(UntypedNode::ForYield(for_tree)) => {
            let mut children = for_tree.enums.clone();
            children.push(for_tree.body);
            children
        }
        TreeKind::PhaseSpecific(UntypedNode::ForDo(for_tree)) => {
            let mut children = for_tree.enums.clone();
            children.push(for_tree.body);
            children
        }
        TreeKind::PhaseSpecific(UntypedNode::GenFrom(generator)) => {
            vec![generator.pattern, generator.expr]
        }
        TreeKind::PhaseSpecific(UntypedNode::GenAlias(alias)) => vec![alias.pattern, alias.expr],
        TreeKind::PhaseSpecific(UntypedNode::Throw(throw)) => vec![throw.expr],
        TreeKind::PhaseSpecific(UntypedNode::ParsedTry(parsed_try)) => {
            let mut children = vec![parsed_try.expr];
            if let Some(handler) = parsed_try.handler {
                children.push(handler);
            }
            if let Some(finalizer) = parsed_try.finalizer {
                children.push(finalizer);
            }
            children
        }
        TreeKind::Return(return_tree) => return_tree.expr.into_iter().collect(),
        _ => Vec::new(),
    }
}

fn has_non_empty_span(arena: &AstArena<Untyped>, id: TreeId<Untyped>) -> bool {
    arena
        .get(id)
        .position
        .is_some_and(|position| position.span().range().start() != position.span().range().end())
}

fn is_synthetic_unit(arena: &AstArena<Untyped>, id: TreeId<Untyped>) -> bool {
    let tree = arena.get(id);
    matches!(&tree.kind, TreeKind::Literal(literal) if literal.value == dotty_core::Constant::Unit)
        && !has_non_empty_span(arena, id)
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

fn is_wildcard_type_param_source(source: &str) -> bool {
    let mut chars = source.trim_start().chars();
    if chars.next() != Some('_') {
        return false;
    }

    match chars.next() {
        None | Some(',') | Some(']') => true,
        Some(character) if character.is_whitespace() => true,
        Some('<' | '>') => chars.next() == Some(':'),
        Some(_) => false,
    }
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

fn find_mix_span(tree: &Tree<Untyped>, name: &str, source: &str) -> Option<(u32, u32)> {
    let position = tree.position?;
    let range = position.span().range();
    let start = range.start() as usize;
    let end = range.end() as usize;
    let text = source.get(start..end)?;
    let left_bracket = text.find('[')?;
    let right_bracket = text[left_bracket + 1..].find(']')? + left_bracket + 1;
    let mix_start = start.checked_add(left_bracket + 1)?;
    let mix_end = start.checked_add(right_bracket)?;
    let offset = source.get(mix_start..mix_end)?.find(name)?;
    let name_start = mix_start.checked_add(offset)?;
    let name_end = name_start.checked_add(name.len())?;
    Some((name_start as u32, name_end as u32))
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
