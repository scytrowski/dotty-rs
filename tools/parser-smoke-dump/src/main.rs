use std::{env, fs, process};

use dotty_core::ast::{ApplyKind, AstArena, Untyped, UntypedNode};
use dotty_core::{NameInterner, SourceId, SourceText, Tree, TreeId, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::{
    Parser, ParserFeatures, parse_compilation_unit, parse_expression_fragment,
    parse_pattern_fragment,
};

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
        [flag, mode, path]
            if flag == "--mode"
                && (mode == "pattern"
                    || mode == "compilation"
                    || mode == "compilation-capture"
                    || mode == "block"
                    || mode == "block-erased") =>
        {
            (mode.as_str(), path.as_str())
        }
        _ => {
            eprintln!(
                "usage: dotty-parser-smoke-dump [--mode pattern|block|block-erased|compilation|compilation-capture] <source-file> | --batch manifest"
            );
            process::exit(2);
        }
    };

    if path.is_empty() {
        eprintln!(
            "usage: dotty-parser-smoke-dump [--mode pattern|block|block-erased|compilation|compilation-capture] <source-file> | --batch manifest"
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
    } else if mode == "compilation-capture" {
        Parser::new(source_text, SourceId::from_index(0), scanner, &mut names)
            .with_features(ParserFeatures {
                capture_checking: true,
                ..ParserFeatures::default()
            })
            .source_compilation_unit()
    } else if mode == "compilation" || mode == "oracle-only" {
        parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names)
    } else if mode == "block-erased" {
        Parser::new(source_text, SourceId::from_index(0), scanner, &mut names)
            .with_features(ParserFeatures {
                erased_definitions: true,
                ..ParserFeatures::default()
            })
            .parse_expression_fragment()
    } else {
        parse_expression_fragment(source_text, SourceId::from_index(0), scanner, &mut names)
    };
    if !result.diagnostics.is_empty() {
        return Err(format!(
            "parser reported diagnostics for {path}: {:?}",
            result.diagnostics
        ));
    }

    let tree = if mode == "pattern"
        || mode == "expr"
        || mode == "block"
        || mode == "block-erased"
        || mode == "compilation"
        || mode == "compilation-capture"
        || mode == "oracle-only"
    {
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
    if let Some((parent, explicit, captures)) = capture_retaining_parts(id, arena, names) {
        let children = captures
            .into_iter()
            .map(|capture| render_tree(capture, arena, names, source))
            .collect::<Vec<_>>()
            .join(",");
        return format!(
            "{{\"kind\":\"Annotated\",\"span\":{},\"children\":[{},{{\"kind\":\"Retains\",\"span\":{},\"explicit\":{},\"children\":[{}]}}]}}",
            render_span(tree),
            render_tree(parent, arena, names, source),
            render_span(tree),
            explicit,
            children
        );
    }
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
        TreeKind::Import(import) => {
            fields.push(format!(
                "\"selectors\":{}",
                render_selectors(&import.selectors, arena, names, source)
            ));
        }
        TreeKind::Export(export) => {
            fields.push(format!(
                "\"selectors\":{}",
                render_selectors(&export.selectors, arena, names, source)
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
            if bind.given {
                fields.push("\"given\":true".to_owned());
            }
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
            if let Some(variance) = definition.variance {
                fields.push(format!(
                    "\"variance\":{}",
                    quote(match variance {
                        dotty_core::types::Variance::Covariant => "covariant",
                        dotty_core::types::Variance::Contravariant => "contravariant",
                        dotty_core::types::Variance::Invariant => "invariant",
                    })
                ));
            }
            if definition
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::Param)
            {
                fields.push("\"param\":true".to_owned());
            }
            if definition
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::PrivateLocal)
            {
                fields.push("\"private_local\":true".to_owned());
            }
            if definition
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::Trait)
            {
                fields.push("\"trait\":true".to_owned());
            }
            if definition.metadata.modifiers.iter().any(|modifier| {
                matches!(
                    modifier,
                    dotty_core::ast::Modifier::Enum | dotty_core::ast::Modifier::EnumCase
                )
            }) {
                fields.push("\"enum\":true".to_owned());
            }
            if is_type_definition_source(&source_text)
                || definition
                    .metadata
                    .modifiers
                    .contains(&dotty_core::ast::Modifier::EnumCase)
            {
                fields.extend(render_definition_metadata(
                    &definition.metadata,
                    arena,
                    names,
                    source,
                ));
            }
        }
        TreeKind::PhaseSpecific(UntypedNode::ContextBoundTypeTree(context_bound)) => {
            fields.push(format!(
                "\"parameter\":{}",
                quote(names.resolve(context_bound.parameter.as_name().text()))
            ));
            if let Some(name) = context_bound.name {
                fields.push(format!(
                    "\"name\":{}",
                    quote(names.resolve(name.as_name().text()))
                ));
            }
        }
        TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) => {
            fields.push(format!(
                "\"name\":{}",
                quote(names.resolve(module.name.as_name().text()))
            ));
            if module
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::PackageObject)
            {
                fields.push("\"package_object\":true".to_owned());
            }
            fields.extend(render_definition_metadata(
                &module.metadata,
                arena,
                names,
                source,
            ));
        }
        TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) => {
            fields.push(format!(
                "\"param_clause_sizes\":[{}]",
                extension
                    .param_clauses
                    .iter()
                    .map(|clause| clause.len().to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            ));
            fields.push(format!(
                "\"using_clauses\":[{}]",
                extension
                    .param_clauses
                    .iter()
                    .map(|clause| {
                        clause
                            .first()
                            .is_some_and(|parameter| {
                                matches!(
                                    &arena.get(*parameter).kind,
                                    TreeKind::ValDef(value)
                                        if value.metadata.modifiers.contains(
                                            &dotty_core::ast::Modifier::Given
                                        )
                                )
                            })
                            .to_string()
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            ));
            fields.push(format!(
                "\"implicit_clauses\":[{}]",
                extension
                    .param_clauses
                    .iter()
                    .map(|clause| {
                        clause
                            .first()
                            .is_some_and(|parameter| {
                                matches!(
                                    &arena.get(*parameter).kind,
                                    TreeKind::ValDef(value)
                                        if value.metadata.modifiers.contains(
                                            &dotty_core::ast::Modifier::Implicit
                                        )
                                )
                            })
                            .to_string()
                    })
                    .collect::<Vec<_>>()
                    .join(",")
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
            if definition
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::ParamAccessor)
            {
                fields.push("\"param_accessor\":true".to_owned());
            }
            if definition
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::PrivateLocal)
            {
                fields.push("\"private_local\":true".to_owned());
            }
            if (definition
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::ParamAccessor)
                || definition
                    .metadata
                    .modifiers
                    .contains(&dotty_core::ast::Modifier::PrivateLocal))
                && definition
                    .metadata
                    .modifiers
                    .contains(&dotty_core::ast::Modifier::Var)
            {
                fields.push("\"mutable\":true".to_owned());
            }
            let mut parameter_metadata = definition.metadata.clone();
            // The normalized ValDef already exposes context-clause ownership
            // through its `given` field; Scala's source-ordered modifier list
            // does not repeat the synthetic Given flag for that parameter.
            if parameter_metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::Param)
            {
                parameter_metadata
                    .modifiers
                    .retain(|modifier| *modifier != dotty_core::ast::Modifier::Given);
            }
            parameter_metadata.modifiers.retain(|modifier| {
                modifier_keyword(*modifier)
                    .is_some_and(|keyword| source_has_word(&source_text, keyword))
            });
            parameter_metadata.visibility = parameter_metadata.visibility.filter(|visibility| {
                let keyword = match visibility {
                    dotty_core::ast::VisibilitySyntax::Private { .. } => "private",
                    dotty_core::ast::VisibilitySyntax::Protected { .. } => "protected",
                };
                source_has_word(&source_text, keyword)
            });
            fields.extend(render_definition_metadata(
                &parameter_metadata,
                arena,
                names,
                source,
            ));
        }
        TreeKind::DefDef(definition) => {
            fields.push(format!(
                "\"name\":{}",
                quote(names.resolve(definition.name.as_name().text()))
            ));
            let order = definition
                .source_param_clause_order
                .clone()
                .unwrap_or_else(|| {
                    let mut order = Vec::new();
                    if !definition.type_params.is_empty() {
                        order.push(dotty_core::ast::DefParamClauseOrder::TypeParams(
                            0..definition.type_params.len(),
                        ));
                    }
                    order.extend(
                        (0..definition.value_param_clauses.len())
                            .map(dotty_core::ast::DefParamClauseOrder::ValueParams),
                    );
                    order
                });
            let type_param_count = match order.first() {
                Some(dotty_core::ast::DefParamClauseOrder::TypeParams(range)) => range.len(),
                _ => 0,
            };
            fields.push(format!("\"type_param_count\":{type_param_count}"));
            fields.push(format!(
                "\"param_clause_kinds\":[{}]",
                order
                    .iter()
                    .map(|clause| match clause {
                        dotty_core::ast::DefParamClauseOrder::TypeParams(_) => "\"type\"",
                        dotty_core::ast::DefParamClauseOrder::ValueParams(_) => "\"term\"",
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            ));
            let mut clause_sizes = Vec::with_capacity(order.len());
            let mut using_clauses = Vec::with_capacity(clause_sizes.capacity());
            let mut implicit_clauses = Vec::with_capacity(clause_sizes.capacity());
            for clause_order in &order {
                let clause = match clause_order {
                    dotty_core::ast::DefParamClauseOrder::TypeParams(range) => {
                        clause_sizes.push(range.len().to_string());
                        using_clauses.push("false".to_owned());
                        implicit_clauses.push("false".to_owned());
                        continue;
                    }
                    dotty_core::ast::DefParamClauseOrder::ValueParams(index) => {
                        &definition.value_param_clauses[*index]
                    }
                };
                clause_sizes.push(clause.len().to_string());
                let has_modifier = |modifier| {
                    clause.first().is_some_and(|parameter| {
                        matches!(
                            &arena.get(*parameter).kind,
                            TreeKind::ValDef(value)
                                if value.metadata.modifiers.contains(&modifier)
                        )
                    })
                };
                let using = has_modifier(dotty_core::ast::Modifier::Given);
                using_clauses.push(using.to_string());
                implicit_clauses
                    .push(has_modifier(dotty_core::ast::Modifier::Implicit).to_string());
            }
            fields.push(format!(
                "\"param_clause_sizes\":[{}]",
                clause_sizes.join(",")
            ));
            fields.push(format!("\"using_clauses\":[{}]", using_clauses.join(",")));
            fields.push(format!(
                "\"implicit_clauses\":[{}]",
                implicit_clauses.join(",")
            ));
            fields.extend(render_definition_metadata(
                &definition.metadata,
                arena,
                names,
                source,
            ));
        }
        TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) => {
            fields.extend(render_definition_metadata(
                &definition.modifiers,
                arena,
                names,
                source,
            ));
        }
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
        TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(interpolated)) => {
            fields.push(format!(
                "\"prefix\":{}",
                quote(names.resolve(interpolated.prefix.text()))
            ));
        }
        TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) => {
            fields.push(format!(
                "\"erased_params\":[{}]",
                function
                    .erased_params
                    .iter()
                    .map(bool::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        TreeKind::PhaseSpecific(UntypedNode::CapturesAndResult(_)) => {}
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

    let mut rendered_children = if let TreeKind::Template(template) = &tree.kind {
        render_template_children(template, arena, names, source)
    } else {
        child_ids(&tree.kind, arena)
            .into_iter()
            .map(|child| render_tree(child, arena, names, source))
            .collect()
    };
    if let TreeKind::Super(super_tree) = &tree.kind
        && matches!(arena.get(super_tree.qual).kind, TreeKind::This(this) if this.qual.is_none())
        && !arena.get(super_tree.qual).position.is_some_and(|position| {
            let range = position.span().range();
            range.start() == range.end()
        })
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
    if let TreeKind::Import(import) = &tree.kind
        && import.selectors.len() == 1
        && matches!(
            arena.get(import.expr).kind,
            TreeKind::Ident(ident)
                if names.resolve(ident.name.text()) == "<empty>"
                    && !has_non_empty_span(arena, import.expr)
        )
        && !rendered_children.is_empty()
    {
        rendered_children[0] = "{\"kind\":\"EmptyTree\",\"span\":null,\"children\":[]}".to_owned();
    }
    fields.push(format!("\"children\":[{}]", rendered_children.join(",")));
    format!("{{{}}}", fields.join(","))
}

fn capture_retaining_parts(
    id: TreeId<Untyped>,
    arena: &AstArena<Untyped>,
    names: &NameInterner,
) -> Option<(TreeId<Untyped>, bool, Vec<TreeId<Untyped>>)> {
    let TreeKind::Annotated(annotated) = &arena.get(id).kind else {
        return None;
    };
    let TreeKind::New(new_tree) = &arena.get(annotated.annotation).kind else {
        return None;
    };
    let (annotation_name, captures) = match &arena.get(new_tree.tpt).kind {
        TreeKind::AppliedTypeTree(applied) => (
            type_path_text(applied.tpt, arena, names)?,
            Some(applied.args.as_slice()),
        ),
        _ => (type_path_text(new_tree.tpt, arena, names)?, None),
    };
    let explicit = match annotation_name.as_str() {
        "scala.annotation.retains" => true,
        "scala.annotation.retainsCap" => false,
        _ => return None,
    };
    let mut references = Vec::new();
    if let Some(captures) = captures {
        for capture in captures {
            collect_capture_references(*capture, arena, names, &mut references);
        }
    }
    Some((annotated.expr, explicit, references))
}

fn type_path_text(
    id: TreeId<Untyped>,
    arena: &AstArena<Untyped>,
    names: &NameInterner,
) -> Option<String> {
    match &arena.get(id).kind {
        TreeKind::Ident(ident) => Some(names.resolve(ident.name.text()).to_owned()),
        TreeKind::Select(selection) => Some(format!(
            "{}.{}",
            type_path_text(selection.qualifier, arena, names)?,
            names.resolve(selection.name.text())
        )),
        _ => None,
    }
}

fn collect_capture_references(
    id: TreeId<Untyped>,
    arena: &AstArena<Untyped>,
    names: &NameInterner,
    references: &mut Vec<TreeId<Untyped>>,
) {
    match &arena.get(id).kind {
        TreeKind::SingletonTypeTree(singleton) => references.push(singleton.reference),
        TreeKind::AppliedTypeTree(applied)
            if type_path_text(applied.tpt, arena, names).as_deref() == Some("scala.|") =>
        {
            for capture in &applied.args {
                collect_capture_references(*capture, arena, names, references);
            }
        }
        TreeKind::Select(_) | TreeKind::Ident(_)
            if type_path_text(id, arena, names).as_deref() == Some("scala.Nothing") => {}
        _ => references.push(id),
    }
}

fn render_template_children(
    template: &dotty_core::ast::Template<Untyped>,
    arena: &AstArena<Untyped>,
    names: &NameInterner,
    source: &str,
) -> Vec<String> {
    let mut children = Vec::new();
    children.push(render_tree(template.constructor, arena, names, source));
    children.extend(
        template
            .parents
            .iter()
            .map(|parent| render_tree(*parent, arena, names, source)),
    );
    children.extend(
        template
            .metadata
            .derives
            .iter()
            .map(|derive| render_tree(*derive, arena, names, source)),
    );
    children.extend(template.metadata.uses.iter().map(|use_ref| {
        let reference = render_tree(use_ref.reference, arena, names, source);
        let span = render_use_ref_span(use_ref.reference, use_ref.initially, arena, source);
        format!(
            "{{\"kind\":\"UseRef\",\"span\":{},\"children\":[{}]}}",
            span, reference
        )
    }));
    if let Some(self_val) = template.self_val {
        children.push(render_tree(self_val, arena, names, source));
    }
    children.extend(
        template
            .body
            .iter()
            .map(|member| render_tree(*member, arena, names, source)),
    );
    children
}

fn render_use_ref_span(
    reference: TreeId<Untyped>,
    initially: bool,
    arena: &AstArena<Untyped>,
    source: &str,
) -> String {
    let Some(position) = arena.get(reference).position else {
        return "null".to_owned();
    };
    let range = position.span().range();
    let mut end = range.end() as usize;
    if initially {
        let remainder = &source[end.min(source.len())..];
        let limit = remainder
            .find([',', ':', '\n', '\r', '}'])
            .unwrap_or(remainder.len());
        if let Some(offset) = remainder[..limit].find("initially") {
            end = end.saturating_add(offset).saturating_add("initially".len());
        }
    }
    format!("{{\"start\":{},\"end\":{}}}", range.start(), end)
}

fn render_definition_metadata(
    metadata: &dotty_core::ast::Modifiers,
    arena: &AstArena<Untyped>,
    names: &NameInterner,
    source: &str,
) -> Vec<String> {
    let modifiers = metadata
        .modifiers
        .iter()
        .filter_map(|modifier| match modifier {
            dotty_core::ast::Modifier::Trait => None,
            dotty_core::ast::Modifier::Enum => None,
            dotty_core::ast::Modifier::EnumCase => None,
            dotty_core::ast::Modifier::PackageObject => None,
            dotty_core::ast::Modifier::Param
            | dotty_core::ast::Modifier::ParamAccessor
            | dotty_core::ast::Modifier::PrivateLocal => None,
            dotty_core::ast::Modifier::Abstract => Some("abstract"),
            dotty_core::ast::Modifier::Final => Some("final"),
            dotty_core::ast::Modifier::Sealed => Some("sealed"),
            dotty_core::ast::Modifier::Case => Some("case"),
            dotty_core::ast::Modifier::Var => Some("var"),
            dotty_core::ast::Modifier::Update => Some("update"),
            dotty_core::ast::Modifier::Implicit => Some("implicit"),
            dotty_core::ast::Modifier::Given => Some("given"),
            dotty_core::ast::Modifier::Impure => Some("impure"),
            dotty_core::ast::Modifier::Lazy => Some("lazy"),
            dotty_core::ast::Modifier::Override => Some("override"),
            dotty_core::ast::Modifier::Inline => Some("inline"),
            dotty_core::ast::Modifier::Transparent => Some("transparent"),
            dotty_core::ast::Modifier::Opaque => Some("opaque"),
            dotty_core::ast::Modifier::Open => Some("open"),
            dotty_core::ast::Modifier::Infix => Some("infix"),
            dotty_core::ast::Modifier::Tracked => Some("tracked"),
            dotty_core::ast::Modifier::Into => Some("into"),
            dotty_core::ast::Modifier::Extension => Some("extension"),
            dotty_core::ast::Modifier::Erased => Some("erased"),
        })
        .map(quote)
        .collect::<Vec<_>>();
    let visibility = metadata.visibility.map_or_else(
        || "null".to_owned(),
        |visibility| {
            quote(match visibility {
                dotty_core::ast::VisibilitySyntax::Private { .. } => "private",
                dotty_core::ast::VisibilitySyntax::Protected { .. } => "protected",
            })
        },
    );
    let qualifier = metadata.visibility.and_then(|visibility| match visibility {
        dotty_core::ast::VisibilitySyntax::Private { qualifier }
        | dotty_core::ast::VisibilitySyntax::Protected { qualifier } => qualifier,
    });
    let qualifier = qualifier.map_or_else(
        || "null".to_owned(),
        |name| quote(names.resolve(name.text())),
    );
    let annotations = metadata
        .annotations
        .iter()
        .map(|annotation| render_tree(*annotation, arena, names, source))
        .collect::<Vec<_>>()
        .join(",");
    let enum_case = metadata
        .modifiers
        .contains(&dotty_core::ast::Modifier::EnumCase);

    let mut fields = vec![
        format!("\"modifiers\":[{}]", modifiers.join(",")),
        format!("\"visibility\":{visibility}"),
        format!("\"visibility_qualifier\":{qualifier}"),
        format!("\"annotations\":[{annotations}]"),
    ];
    if enum_case {
        fields.push("\"enum_case\":true".to_owned());
    }
    fields
}

fn modifier_keyword(modifier: dotty_core::ast::Modifier) -> Option<&'static str> {
    use dotty_core::ast::Modifier;
    Some(match modifier {
        Modifier::Abstract => "abstract",
        Modifier::Final => "final",
        Modifier::Sealed => "sealed",
        Modifier::Case => "case",
        Modifier::Var => "var",
        Modifier::Update => "update",
        Modifier::Implicit => "implicit",
        Modifier::Given => "given",
        Modifier::Impure => "impure",
        Modifier::Lazy => "lazy",
        Modifier::Override => "override",
        Modifier::Inline => "inline",
        Modifier::Transparent => "transparent",
        Modifier::Opaque => "opaque",
        Modifier::Open => "open",
        Modifier::Infix => "infix",
        Modifier::Tracked => "tracked",
        Modifier::Into => "into",
        Modifier::Extension => "extension",
        Modifier::Erased => "erased",
        Modifier::Trait
        | Modifier::Enum
        | Modifier::EnumCase
        | Modifier::PackageObject
        | Modifier::Param
        | Modifier::ParamAccessor
        | Modifier::PrivateLocal => return None,
    })
}

fn source_has_word(source: &str, expected: &str) -> bool {
    source
        .split(|character: char| !character.is_ascii_alphabetic())
        .any(|word| word == expected)
}

fn is_type_definition_source(source: &str) -> bool {
    source
        .split(|character: char| !character.is_ascii_alphabetic())
        .any(|word| matches!(word, "type" | "class" | "trait" | "object" | "enum"))
}

fn kind_name(kind: &TreeKind<Untyped>) -> &'static str {
    match kind {
        TreeKind::Ident(_) => "Ident",
        TreeKind::Select(_) => "Select",
        TreeKind::Apply(_) => "Apply",
        TreeKind::NamedArg(_) => "NamedArg",
        TreeKind::Quote(_) => "Quote",
        TreeKind::Splice(_) => "Splice",
        TreeKind::QuotePattern(_) => "QuotePattern",
        TreeKind::SplicePattern(_) => "SplicePattern",
        TreeKind::PackageDef(_) => "PackageDef",
        TreeKind::Import(_) => "Import",
        TreeKind::Export(_) => "Export",
        TreeKind::Bind(_) => "Bind",
        TreeKind::Alternative(_) => "Alternative",
        TreeKind::Annotated(_) => "Annotated",
        TreeKind::Typed(_) => "Typed",
        TreeKind::Assign(_) => "Assign",
        TreeKind::ValDef(_) => "ValDef",
        TreeKind::DefDef(_) => "DefDef",
        TreeKind::PhaseSpecific(UntypedNode::PatDef(_)) => "PatDef",
        TreeKind::TypeDef(_) => "TypeDef",
        TreeKind::RefinedTypeTree(_) => "RefinedTypeTree",
        TreeKind::PhaseSpecific(UntypedNode::ContextBounds(_)) => "ContextBounds",
        TreeKind::PhaseSpecific(UntypedNode::ContextBoundTypeTree(_)) => "ContextBoundTypeTree",
        TreeKind::Template(template)
            if !template.metadata.derives.is_empty() || !template.metadata.uses.is_empty() =>
        {
            "DerivingTemplate"
        }
        TreeKind::Template(_) => "Template",
        TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_)) => "ModuleDef",
        TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(_)) => "ExtMethods",
        TreeKind::LambdaTypeTree(_) => "LambdaTypeTree",
        TreeKind::SingletonTypeTree(_) => "SingletonTypeTree",
        TreeKind::TypeBoundsTree(_) => "TypeBoundsTree",
        TreeKind::ByNameTypeTree(_) => "ByNameTypeTree",
        TreeKind::TypeTree(_) => "TypeTree",
        TreeKind::If(_) => "If",
        TreeKind::While(_) => "While",
        TreeKind::Match(_) => "Match",
        TreeKind::MatchTypeTree(_) => "MatchTypeTree",
        TreeKind::CaseDef(_) => "CaseDef",
        TreeKind::This(_) => "This",
        TreeKind::Super(_) => "Super",
        TreeKind::New(_) => "New",
        TreeKind::TypeApply(_) => "TypeApply",
        TreeKind::AppliedTypeTree(_) => "TypeApply",
        TreeKind::Block(_) => "Block",
        TreeKind::Literal(_) => "Literal",
        TreeKind::PhaseSpecific(UntypedNode::Number(_)) => "Number",
        TreeKind::PhaseSpecific(UntypedNode::Parens(_)) => "Parens",
        TreeKind::PhaseSpecific(UntypedNode::PrefixOp(_)) => "PrefixOp",
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)) => "InfixOp",
        TreeKind::PhaseSpecific(UntypedNode::PostfixOp(_)) => "PostfixOp",
        TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(_)) => "InterpolatedString",
        TreeKind::PhaseSpecific(UntypedNode::Function(_)) => "Function",
        TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_)) => "FunctionWithMods",
        TreeKind::PhaseSpecific(UntypedNode::CapturesAndResult(_)) => "CapturesAndResult",
        TreeKind::PhaseSpecific(UntypedNode::PolyFunction(_)) => "PolyFunction",
        TreeKind::PhaseSpecific(UntypedNode::ForYield(_)) => "ForYield",
        TreeKind::PhaseSpecific(UntypedNode::ForDo(_)) => "ForDo",
        TreeKind::PhaseSpecific(UntypedNode::GenFrom(_)) => "GenFrom",
        TreeKind::PhaseSpecific(UntypedNode::GenAlias(_)) => "GenAlias",
        TreeKind::PhaseSpecific(UntypedNode::Throw(_)) => "Throw",
        TreeKind::PhaseSpecific(UntypedNode::InlineIf(_)) => "InlineIf",
        TreeKind::PhaseSpecific(UntypedNode::InlineMatch(_)) => "InlineMatch",
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

fn render_selectors(
    selectors: &[dotty_core::ast::ImportSelector<Untyped>],
    arena: &AstArena<Untyped>,
    names: &NameInterner,
    source: &str,
) -> String {
    let selectors = selectors
        .iter()
        .map(|selector| {
            let mut fields = vec![format!(
                "\"name\":{}",
                quote(match names.resolve(selector.imported.text()) {
                    "*" => "_",
                    name => name,
                })
            )];
            if selector.imported_backquoted {
                fields.push("\"backquoted\":true".to_owned());
            }
            if let Some(rename) = selector.renamed {
                match &arena.get(rename).kind {
                    TreeKind::Ident(ident) => fields.push(format!(
                        "\"rename\":{}",
                        quote(names.resolve(ident.name.text()))
                    )),
                    _ => fields.push(format!(
                        "\"rename_tree\":{}",
                        render_tree(rename, arena, names, source)
                    )),
                }
            }
            if let Some(bound) = selector.bound {
                fields.push(format!(
                    "\"bound\":{}",
                    render_tree(bound, arena, names, source)
                ));
            }
            format!("{{{}}}", fields.join(","))
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{selectors}]")
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
        TreeKind::PackageDef(package) => {
            let mut children = Vec::with_capacity(package.stats.len() + 1);
            children.push(package.name);
            children.extend(package.stats.iter().copied());
            children
        }
        TreeKind::Import(import) => vec![import.expr],
        TreeKind::Export(export) => vec![export.expr],
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
        TreeKind::AppliedTypeTree(applied) => {
            let mut children = Vec::with_capacity(applied.args.len() + 1);
            children.push(applied.tpt);
            children.extend(applied.args.iter().copied());
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
        TreeKind::Annotated(annotated) => vec![annotated.expr, annotated.annotation],
        TreeKind::Quote(quote) => vec![quote.body],
        TreeKind::Splice(splice) => vec![splice.expr],
        TreeKind::QuotePattern(quote) => {
            let mut children = Vec::with_capacity(quote.bindings.len() + 2);
            children.extend(quote.bindings.iter().copied());
            children.push(quote.body);
            children.push(quote.quotes);
            children
        }
        TreeKind::SplicePattern(splice) => {
            let mut children = Vec::with_capacity(1 + splice.type_args.len() + splice.args.len());
            children.push(splice.body);
            children.extend(splice.type_args.iter().copied());
            children.extend(splice.args.iter().copied());
            children
        }
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
        TreeKind::MatchTypeTree(match_type) => {
            let mut children = Vec::with_capacity(match_type.cases.len() + 1);
            if let Some(bound) = match_type.bound {
                children.push(bound);
            }
            children.push(match_type.selector);
            children.extend(match_type.cases.iter().copied());
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
            let mut children = Vec::new();
            if let Some(order) = &definition.source_param_clause_order {
                for clause in order {
                    match clause {
                        dotty_core::ast::DefParamClauseOrder::TypeParams(range) => {
                            children.extend_from_slice(&definition.type_params[range.clone()]);
                        }
                        dotty_core::ast::DefParamClauseOrder::ValueParams(index) => {
                            children.extend(definition.value_param_clauses[*index].iter().copied());
                        }
                    }
                }
            } else {
                children.extend(definition.type_params.iter().copied());
                for clause in &definition.value_param_clauses {
                    children.extend(clause.iter().copied());
                }
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
        TreeKind::RefinedTypeTree(refined) => {
            let mut children = Vec::with_capacity(refined.refinements.len() + 1);
            let include_parent = arena.get(refined.tpt).position.is_some_and(|position| {
                let range = position.span().range();
                range.start() != range.end()
            });
            if include_parent {
                children.push(refined.tpt);
            }
            children.extend(refined.refinements.iter().copied());
            children
        }
        TreeKind::PhaseSpecific(UntypedNode::ContextBounds(bounds)) => {
            let mut children = vec![bounds.bounds];
            children.extend(bounds.context_bounds.iter().copied());
            children
        }
        TreeKind::PhaseSpecific(UntypedNode::ContextBoundTypeTree(bound)) => vec![bound.bound],
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
        TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) => {
            let mut children = Vec::new();
            children.extend(
                extension
                    .param_clauses
                    .iter()
                    .flat_map(|clause| clause.iter().copied()),
            );
            children.extend(extension.methods.iter().copied());
            children
        }
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
        TreeKind::ByNameTypeTree(by_name) => vec![by_name.result],
        TreeKind::SingletonTypeTree(singleton) => vec![singleton.reference],
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
        TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(interpolated)) => {
            interpolated.parts.clone()
        }
        TreeKind::PhaseSpecific(UntypedNode::Function(function)) => {
            let mut children = function.params.clone();
            children.push(function.body);
            children
        }
        TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) => {
            let mut children = function.params.clone();
            children.push(function.result);
            children
        }
        TreeKind::PhaseSpecific(UntypedNode::CapturesAndResult(captures)) => {
            let mut children = captures.captures.clone();
            children.push(captures.result);
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
        TreeKind::PhaseSpecific(UntypedNode::InlineIf(inline_if)) => {
            let mut children = vec![inline_if.cond, inline_if.then_branch];
            if has_non_empty_span(arena, inline_if.else_branch) {
                children.push(inline_if.else_branch);
            }
            children
        }
        TreeKind::PhaseSpecific(UntypedNode::InlineMatch(inline_match)) => {
            let mut children = vec![inline_match.selector];
            children.extend(inline_match.cases.iter().copied());
            children
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::ast::UntypedNode;
    use dotty_core::{HardKeyword, TokenSource};

    #[test]
    fn keeps_control_keywords_before_adjacent_literals_in_the_token_stream() {
        let source = concat!(
            "def thenString(x: Boolean) = if x then\"\" else \"result\"\n",
            "def elseInt(x: Boolean) = if x then 1 else\"other\"\n",
            "def doString(x: Boolean) = while x do\"body\"\n",
        );
        let mut scanner = ContextualScanner::new(source).expect("source should scan cleanly");
        let mut kinds = Vec::new();
        while scanner.current().kind != dotty_core::TokenKind::Eof {
            kinds.push(scanner.current().kind);
            scanner.advance();
        }

        assert!(kinds.contains(&dotty_core::TokenKind::Keyword(HardKeyword::Then)));
        assert!(kinds.contains(&dotty_core::TokenKind::Keyword(HardKeyword::Else)));
        assert!(kinds.contains(&dotty_core::TokenKind::Keyword(HardKeyword::Do)));

        let scanner = ContextualScanner::new(source).expect("source should scan cleanly");
        let source_text = SourceText::new(source).expect("source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn if_branch_can_start_with_a_quote() {
        const SOURCE: &str = "def quoted(x: Boolean) = if x then 1 else '{ 2 }";
        let scanner = ContextualScanner::new(SOURCE).expect("source should scan cleanly");
        let source_text = SourceText::new(SOURCE).expect("source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let rendered = render_tree(result.root, &result.ast, &names, SOURCE);
        assert!(rendered.contains("\"kind\":\"Quote\""), "{rendered}");
    }

    #[test]
    fn if_branches_can_start_with_character_literals() {
        const SOURCE: &str = "def choose(x: Boolean) = if x then 'a' else 'b'";
        let scanner = ContextualScanner::new(SOURCE).expect("source should scan cleanly");
        let source_text = SourceText::new(SOURCE).expect("source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn extension_end_marker_is_owned_by_the_extension_declaration() {
        const SOURCE: &str = "extension (x: Int)\n  def increment = x + 1\nend extension";
        let scanner = ContextualScanner::new(SOURCE).expect("source should scan cleanly");
        let source_text = SourceText::new(SOURCE).expect("source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn end_marker_for_anonymous_template_method_is_not_misaligned() {
        const SOURCE: &str = concat!(
            "object O:\n",
            "  def traverser = new TreeTraverser:\n",
            "    def traverse(tree: Tree) =\n",
            "      visit(tree)\n",
            "    end traverse\n",
            "  end traverser\n",
            "end O",
        );
        let scanner = ContextualScanner::new(SOURCE).expect("source should scan cleanly");
        let source_text = SourceText::new(SOURCE).expect("source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn enclosing_definition_end_marker_survives_anonymous_template_body() {
        const SOURCE: &str = concat!(
            "object O:\n",
            "  private def nestedTypeTraverser = new TreeTraverser:\n",
            "    def traverse(tree: Tree) =\n",
            "      visit(tree)\n",
            "    end traverse\n",
            "  end nestedTypeTraverser\n",
            "end O",
        );
        let scanner = ContextualScanner::new(SOURCE).expect("source should scan cleanly");
        let source_text = SourceText::new(SOURCE).expect("source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn enclosing_class_end_marker_survives_nested_anonymous_template() {
        const SOURCE: &str = concat!(
            "object O:\n",
            "  class TreeMapWithVariance:\n",
            "      def transform = new TreeTraverser:\n",
            "        def visit = ()\n",
            "        end visit\n",
            "    end TreeMapWithVariance\n",
            "end O",
        );
        let scanner = ContextualScanner::new(SOURCE).expect("source should scan cleanly");
        let source_text = SourceText::new(SOURCE).expect("source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn indented_lambda_body_can_begin_with_a_local_definition() {
        const SOURCE: &str = concat!(
            "object O:\n",
            "  def run =\n",
            "    val f = (x: Int) =>\n",
            "      val y = x + 1\n",
            "      y\n",
            "    f(1)",
        );
        let scanner = ContextualScanner::new(SOURCE).expect("source should scan cleanly");
        let source_text = SourceText::new(SOURCE).expect("source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn context_lambda_with_a_comment_can_begin_its_body_with_a_definition() {
        const SOURCE: &str = concat!(
            "object O:\n",
            "  def run =\n",
            "    consume { (ctx0: Context) ?=>\n",
            "      // Keep the current context out of the captured closure.\n",
            "      val ctx1 = localContext(ctx0)\n",
            "      inContext(ctx1) { 1 }\n",
            "    }",
        );
        let scanner = ContextualScanner::new(SOURCE).expect("source should scan cleanly");
        let source_text = SourceText::new(SOURCE).expect("source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn braced_template_dedented_else_remains_attached_to_its_if() {
        const SOURCE: &str = "object O {\n    val x = if true then 1\nelse 2\n}";
        let scanner = ContextualScanner::new(SOURCE).expect("source should scan cleanly");
        let source_text = SourceText::new(SOURCE).expect("source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
            panic!("expected package root");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) =
            &result.ast.get(package.stats[0]).kind
        else {
            panic!("expected object O");
        };
        let TreeKind::Template(template) = &result.ast.get(module.template).kind else {
            panic!("expected object template");
        };
        let TreeKind::ValDef(value) = &result.ast.get(template.body[0]).kind else {
            panic!("expected val x");
        };
        let TreeKind::If(if_tree) = &result.ast.get(value.rhs.unwrap()).kind else {
            panic!("expected if expression as the value RHS");
        };
        assert!(matches!(
            result.ast.get(if_tree.else_branch).kind,
            TreeKind::PhaseSpecific(UntypedNode::Number(_))
        ));
        assert_eq!(
            result
                .ast
                .get(value.rhs.unwrap())
                .position
                .unwrap()
                .span()
                .range(),
            dotty_core::TextRange::new(
                SOURCE.find("if true").unwrap() as u32,
                (SOURCE.find("else 2").unwrap() + "else 2".len()) as u32,
            )
            .unwrap()
        );
    }

    #[test]
    fn empty_final_nested_match_case_reports_at_boundary_and_keeps_next_expression() {
        const SOURCE: &str = concat!(
            "object O:\n",
            "  def f = {\n",
            "    value match\n",
            "      case A => )\n",
            "    after\n",
            "  }\n",
            "  def next = 2",
        );
        let scanner = ContextualScanner::new(SOURCE).expect("source should scan cleanly");
        let source_text = SourceText::new(SOURCE).expect("source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(
            result.diagnostics[0].kind(),
            dotty_parser::ParseDiagnosticKind::UnexpectedToken
        );
        let bad_token = SOURCE.find(')').expect("malformed token exists") as u32;
        assert_eq!(
            result.diagnostics[0].span(),
            dotty_core::TextRange::new(bad_token, bad_token + 1).unwrap()
        );

        let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
            panic!("expected package root");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) =
            &result.ast.get(package.stats[0]).kind
        else {
            panic!("expected object O");
        };
        let TreeKind::Template(template) = &result.ast.get(module.template).kind else {
            panic!("expected object template");
        };
        let method_id = template
            .body
            .iter()
            .copied()
            .find(|id| {
                matches!(
                    &result.ast.get(*id).kind,
                    TreeKind::DefDef(definition)
                        if names.resolve(definition.name.as_name().text()) == "f"
                )
            })
            .expect("method containing the match remains in the template");
        let TreeKind::DefDef(method) = &result.ast.get(method_id).kind else {
            unreachable!();
        };
        let TreeKind::Block(body) = &result.ast.get(method.rhs.unwrap()).kind else {
            panic!("expected the method's indented body block");
        };
        assert!(
            body.stats
                .iter()
                .any(|id| matches!(result.ast.get(*id).kind, TreeKind::Match(_)))
        );
        let TreeKind::Ident(final_expr) = &result.ast.get(body.expr).kind else {
            panic!("the following `after` expression should remain in the method body");
        };
        assert_eq!(names.resolve(final_expr.name.text()), "after");
        assert!(template.body.iter().any(|id| matches!(
            &result.ast.get(*id).kind,
            TreeKind::DefDef(definition)
                if names.resolve(definition.name.as_name().text()) == "next"
        )));
    }

    #[test]
    fn missing_match_case_arrow_preserves_following_case_and_template_member() {
        const SOURCE: &str = concat!(
            "object O:\n",
            "  def f = {\n",
            "    value match\n",
            "      case A\n",
            "      case B => 2\n",
            "    after\n",
            "  }\n",
            "  def next = 3",
        );
        let scanner = ContextualScanner::new(SOURCE).expect("source should scan cleanly");
        let source_text = SourceText::new(SOURCE).expect("source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(
            result.diagnostics[0].kind(),
            dotty_parser::ParseDiagnosticKind::ExpectedToken
        );
        let next_case = SOURCE.find("case B").unwrap() as u32;
        assert_eq!(
            result.diagnostics[0].span(),
            dotty_core::TextRange::new(next_case - 7, next_case).unwrap()
        );

        let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
            panic!("expected package root");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) =
            &result.ast.get(package.stats[0]).kind
        else {
            panic!("expected object O");
        };
        let TreeKind::Template(template) = &result.ast.get(module.template).kind else {
            panic!("expected object template");
        };
        let method_id = template
            .body
            .iter()
            .copied()
            .find(|id| {
                matches!(
                    &result.ast.get(*id).kind,
                    TreeKind::DefDef(definition)
                        if names.resolve(definition.name.as_name().text()) == "f"
                )
            })
            .expect("f method remains inside the object");
        let TreeKind::DefDef(method) = &result.ast.get(method_id).kind else {
            unreachable!();
        };
        let TreeKind::Block(body) = &result.ast.get(method.rhs.unwrap()).kind else {
            panic!("expected f's braced body");
        };
        let match_id = body
            .stats
            .iter()
            .copied()
            .find(|id| matches!(result.ast.get(*id).kind, TreeKind::Match(_)))
            .expect("match expression remains in f's body");
        let TreeKind::Match(match_tree) = &result.ast.get(match_id).kind else {
            unreachable!();
        };
        assert!(
            match_tree.cases.iter().any(|id| matches!(
                &result.ast.get(*id).kind,
                TreeKind::CaseDef(case_def)
                    if matches!(
                        &result.ast.get(case_def.pattern).kind,
                        TreeKind::Ident(identifier)
                            if names.resolve(identifier.name.text()) == "B"
                    )
            )),
            "the later case must not be swallowed by recovery"
        );
        let TreeKind::Ident(after) = &result.ast.get(body.expr).kind else {
            panic!("the following `after` expression remains in f's body");
        };
        assert_eq!(names.resolve(after.name.text()), "after");
        assert!(
            template.body.iter().any(|id| matches!(
                &result.ast.get(*id).kind,
                TreeKind::DefDef(definition)
                    if names.resolve(definition.name.as_name().text()) == "next"
            )),
            "the following method remains at template scope"
        );
    }

    #[test]
    fn match_valued_if_branch_keeps_outer_else_and_full_span() {
        const SOURCE: &str = concat!(
            "object IfElseAfterIndentedMatch:\n",
            "  def choose(flag: Boolean, value: Int): Int =\n",
            "    if flag then\n",
            "      value match\n",
            "        case 0 => 1\n",
            "        case _ => 2\n",
            "    else 3",
        );
        let scanner = ContextualScanner::new(SOURCE).expect("source should scan cleanly");
        let source_text = SourceText::new(SOURCE).expect("source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
            panic!("expected package root");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) =
            &result.ast.get(package.stats[0]).kind
        else {
            panic!("expected object");
        };
        let TreeKind::Template(template) = &result.ast.get(module.template).kind else {
            panic!("expected object template");
        };
        let TreeKind::DefDef(method) = &result.ast.get(template.body[0]).kind else {
            panic!("expected choose method");
        };
        let TreeKind::If(if_tree) = &result.ast.get(method.rhs.unwrap()).kind else {
            panic!("expected if expression");
        };
        assert!(matches!(
            result.ast.get(if_tree.then_branch).kind,
            TreeKind::Match(_)
        ));
        assert!(matches!(
            result.ast.get(if_tree.else_branch).kind,
            TreeKind::PhaseSpecific(UntypedNode::Number(_))
        ));
        assert_eq!(
            result.ast.get(method.rhs.unwrap()).position.unwrap().span().range(),
            dotty_core::TextRange::new(
                SOURCE.find("if flag").unwrap() as u32,
                SOURCE.len() as u32,
            )
            .unwrap()
        );
    }

    #[test]
    fn missing_else_body_diagnoses_without_consuming_the_next_template_member() {
        const SOURCE: &str = concat!(
            "object O {\n",
            "  def choose(flag: Boolean, value: Int): Int = {\n",
            "    if flag then\n",
            "      value match\n",
            "        case _ => 1\n",
            "    else\n",
            "  }\n",
            "  def after = 2\n",
            "}",
        );
        let scanner = ContextualScanner::new(SOURCE).expect("source should scan cleanly");
        let source_text = SourceText::new(SOURCE).expect("source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic.kind() == dotty_parser::ParseDiagnosticKind::ExpectedExpression
            }),
            "{:?}",
            result.diagnostics
        );
        let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
            panic!("expected package root");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) =
            &result.ast.get(package.stats[0]).kind
        else {
            panic!("expected object");
        };
        let TreeKind::Template(template) = &result.ast.get(module.template).kind else {
            panic!("expected object template");
        };
        assert!(template.body.iter().any(|id| matches!(
            &result.ast.get(*id).kind,
            TreeKind::DefDef(definition)
                if names.resolve(definition.name.as_name().text()) == "after"
        )));
    }

    #[test]
    fn context_lambda_in_indented_template_keeps_following_member() {
        const SOURCE: &str = concat!(
            "object ContextLambdaTemplateNextMember:\n",
            "  val instance = new C:\n",
            "    val value: Int | (Context ?=> Int) = ctx ?=> 1\n",
            "    val after = 2",
        );
        let scanner = ContextualScanner::new(SOURCE).expect("source should scan cleanly");
        let source_text = SourceText::new(SOURCE).expect("source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
            panic!("expected package root");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) =
            &result.ast.get(package.stats[0]).kind
        else {
            panic!("expected object");
        };
        let TreeKind::Template(outer_template) = &result.ast.get(module.template).kind else {
            panic!("expected outer template");
        };
        let TreeKind::ValDef(instance) = &result.ast.get(outer_template.body[0]).kind else {
            panic!("expected instance value");
        };
        let TreeKind::New(new_expr) = &result.ast.get(instance.rhs.unwrap()).kind else {
            panic!("expected anonymous class construction");
        };
        let TreeKind::Template(inner_template) = &result.ast.get(new_expr.tpt).kind else {
            panic!("expected anonymous template");
        };
        assert_eq!(inner_template.body.len(), 2);
        let TreeKind::ValDef(value) = &result.ast.get(inner_template.body[0]).kind else {
            panic!("expected lambda-valued member");
        };
        assert!(matches!(
            result.ast.get(value.rhs.unwrap()).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        let TreeKind::ValDef(after) = &result.ast.get(inner_template.body[1]).kind else {
            panic!("the following template member should remain after the lambda");
        };
        assert_eq!(names.resolve(after.name.as_name().text()), "after");
        assert_eq!(
            result
                .ast
                .get(value.rhs.unwrap())
                .position
                .unwrap()
                .span()
                .range()
                .end(),
            SOURCE.find("ctx ?=> 1").unwrap() as u32 + "ctx ?=> 1".len() as u32
        );
    }

    #[test]
    fn missing_context_lambda_body_diagnoses_and_keeps_next_template_member() {
        const SOURCE: &str = concat!(
            "object O:\n",
            "  val instance = new C:\n",
            "    val value: Int | (Context ?=> Int) = ctx ?=>\n",
            "    val after = 2",
        );
        let scanner = ContextualScanner::new(SOURCE).expect("source should scan cleanly");
        let source_text = SourceText::new(SOURCE).expect("source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic.kind() == dotty_parser::ParseDiagnosticKind::ExpectedExpression
            }),
            "{:?}",
            result.diagnostics
        );
        let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
            panic!("expected package root");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) =
            &result.ast.get(package.stats[0]).kind
        else {
            panic!("expected object");
        };
        let TreeKind::Template(outer_template) = &result.ast.get(module.template).kind else {
            panic!("expected outer template");
        };
        let TreeKind::ValDef(instance) = &result.ast.get(outer_template.body[0]).kind else {
            panic!("expected instance value");
        };
        let TreeKind::New(new_expr) = &result.ast.get(instance.rhs.unwrap()).kind else {
            panic!("expected anonymous class construction");
        };
        let TreeKind::Template(inner_template) = &result.ast.get(new_expr.tpt).kind else {
            panic!("expected anonymous template");
        };
        assert!(inner_template.body.iter().any(|id| matches!(
            &result.ast.get(*id).kind,
            TreeKind::ValDef(definition)
                if names.resolve(definition.name.as_name().text()) == "after"
        )));
    }

    #[test]
    fn parses_a_multiline_lambda_body_inside_a_nested_call_argument() {
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tools/scala-parser-oracle/fixtures/expressions/lambda-multiline-argument.scala"
        );
        let source = fs::read_to_string(fixture).expect("fixture should be readable");
        let scanner = ContextualScanner::new(&source).expect("fixture should scan cleanly");
        let source_text = SourceText::new(&source).expect("fixture source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_expression_fragment(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let TreeKind::Apply(outer) = &result.ast.get(result.root).kind else {
            panic!("expected the outer call");
        };
        assert_eq!(outer.args.len(), 2);
        let TreeKind::Select(sequence) = &result.ast.get(outer.args[0]).kind else {
            panic!("expected the chained `toSeq` selection");
        };
        assert_eq!(names.resolve(sequence.name.text()), "toSeq");
        let TreeKind::Apply(mapping) = &result.ast.get(sequence.qualifier).kind else {
            panic!("expected the `map` application");
        };
        let TreeKind::PhaseSpecific(UntypedNode::Function(function)) =
            &result.ast.get(mapping.args[0]).kind
        else {
            panic!("expected the lambda argument");
        };
        let TreeKind::Block(body) = &result.ast.get(function.body).kind else {
            panic!("expected a block for the multi-statement lambda body");
        };
        assert_eq!(body.stats.len(), 1);
        assert!(matches!(
            result.ast.get(body.stats[0]).kind,
            TreeKind::ValDef(_)
        ));
        assert!(matches!(result.ast.get(body.expr).kind, TreeKind::Ident(_)));
        let TreeKind::Ident(fallback) = &result.ast.get(outer.args[1]).kind else {
            panic!("the following call argument must remain outside the lambda");
        };
        assert_eq!(names.resolve(fallback.name.text()), "fallback");
    }

    #[test]
    fn missing_assignment_rhs_does_not_consume_following_definition() {
        let source = concat!("def f(x: Int) =\n", "  x =\n", "  val y = 1\n", "  y\n",);
        let scanner = ContextualScanner::new(source).expect("source should scan cleanly");
        let source_text = SourceText::new(source).expect("source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            dotty_parser::ParseDiagnosticKind::ExpectedExpression
        );
        let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
            panic!("expected the compilation-unit package");
        };
        let method_id = package
            .stats
            .iter()
            .copied()
            .find(|id| matches!(result.ast.get(*id).kind, TreeKind::DefDef(_)))
            .expect("method definition should remain at compilation-unit scope");
        let TreeKind::DefDef(method) = &result.ast.get(method_id).kind else {
            unreachable!();
        };
        let TreeKind::Block(body) = &result.ast.get(method.rhs.expect("method body")).kind else {
            panic!("expected the indented method body block");
        };
        assert_eq!(body.stats.len(), 2);
        assert!(matches!(
            result.ast.get(body.stats[0]).kind,
            TreeKind::Assign(_)
        ));
        let TreeKind::Assign(assignment) = &result.ast.get(body.stats[0]).kind else {
            unreachable!();
        };
        assert!(matches!(
            result.ast.get(assignment.rhs).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert!(matches!(
            result.ast.get(body.stats[1]).kind,
            TreeKind::ValDef(_)
        ));
        let TreeKind::Ident(final_expr) = &result.ast.get(body.expr).kind else {
            panic!("expected the final `y` expression to remain in the block");
        };
        assert_eq!(names.resolve(final_expr.name.text()), "y");
    }

    #[test]
    fn missing_multiline_lambda_body_preserves_enclosing_argument_boundaries() {
        let source = "consume(values.map(x =>\n), fallback)";
        let scanner = ContextualScanner::new(source).expect("source should scan");
        let source_text = SourceText::new(source).expect("source text should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_expression_fragment(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(result.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message()
                .contains("expected an expression after lambda arrow")
        }));
        let TreeKind::Apply(outer) = &result.ast.get(result.root).kind else {
            panic!("expected the outer call despite the missing lambda body");
        };
        assert_eq!(outer.args.len(), 2);
        let TreeKind::Ident(fallback) = &result.ast.get(outer.args[1]).kind else {
            panic!("the following argument must remain available after recovery");
        };
        assert_eq!(names.resolve(fallback.name.text()), "fallback");
    }

    #[test]
    fn parses_yield_tail_after_nested_anonymous_template() {
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tools/scala-parser-oracle/fixtures/for-yield-anonymous-template-tail.scala"
        );
        let source = fs::read_to_string(fixture).expect("fixture should be readable");
        let scanner = ContextualScanner::new(&source).expect("fixture should scan cleanly");
        let source_text = SourceText::new(&source).expect("fixture source should be valid");
        let mut names = NameInterner::new();
        let result =
            parse_expression_fragment(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let TreeKind::Block(outer) = &result.ast.get(result.root).kind else {
            panic!("expected outer braced block");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ForYield(for_yield)) =
            &result.ast.get(outer.expr).kind
        else {
            panic!("expected a for/yield expression");
        };
        let TreeKind::Block(body) = &result.ast.get(for_yield.body).kind else {
            panic!("expected a multi-statement yield body");
        };
        assert_eq!(body.stats.len(), 1);
        assert!(matches!(
            result.ast.get(body.stats[0]).kind,
            TreeKind::ValDef(_)
        ));
        assert!(matches!(
            result.ast.get(body.expr).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
        let TreeKind::ValDef(cleanup) = &result.ast.get(body.stats[0]).kind else {
            unreachable!();
        };
        assert!(matches!(
            result.ast.get(cleanup.rhs.unwrap()).kind,
            TreeKind::New(_)
        ));
    }

    #[test]
    fn nested_indented_template_closes_before_following_method_at_eof() {
        const SOURCE: &str = "object Outer:\n  def f =\n    val sf =\n      new Foo:\n        def x = 1\n    sf.foo\n  def next = 2";

        for source in [SOURCE, &format!("{SOURCE}\n")] {
            let scanner = ContextualScanner::new(source).expect("source scans");
            assert!(scanner.diagnostics().is_empty());
            let source_text = SourceText::new(source).expect("source text is valid");
            let mut names = NameInterner::new();
            let result =
                parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

            assert!(
                result.diagnostics.is_empty(),
                "unexpected parser diagnostics with final newline={}: {:?}",
                source.ends_with('\n'),
                result.diagnostics
            );

            let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
                panic!("expected a package root");
            };
            let module_id = package
                .stats
                .iter()
                .copied()
                .find(|id| {
                    matches!(
                        &result.ast.get(*id).kind,
                        TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module))
                            if names.resolve(module.name.as_name().text()) == "Outer"
                    )
                })
                .expect("Outer module remains at compilation-unit scope");
            let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) =
                &result.ast.get(module_id).kind
            else {
                unreachable!();
            };
            let TreeKind::Template(template) = &result.ast.get(module.template).kind else {
                panic!("expected Outer template");
            };
            let member_names: Vec<_> = template
                .body
                .iter()
                .filter_map(|id| match &result.ast.get(*id).kind {
                    TreeKind::DefDef(definition) => {
                        Some(names.resolve(definition.name.as_name().text()))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(member_names, ["f", "next"]);
        }
    }

    #[test]
    fn mismatched_nested_template_end_marker_preserves_outer_members_and_marker() {
        const SOURCE: &str = concat!(
            "object Outer:\n",
            "  class Inner:\n",
            "    val x = 1\n",
            "  end Wrong\n",
            "  val after = 2\n",
            "end Outer",
        );
        let scanner = ContextualScanner::new(SOURCE).expect("source scans");
        let source_text = SourceText::new(SOURCE).expect("source text is valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert!(
            result.diagnostics[0]
                .message()
                .contains("misaligned end marker")
        );
        assert_eq!(
            result.diagnostics[0].span(),
            dotty_core::TextRange::new(
                SOURCE.find("end Wrong").unwrap() as u32,
                (SOURCE.find("end Wrong").unwrap() + "end Wrong".len()) as u32,
            )
            .unwrap()
        );

        let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
            panic!("expected package root");
        };
        let outer_id = package
            .stats
            .iter()
            .copied()
            .find(|id| {
                matches!(
                    &result.ast.get(*id).kind,
                    TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module))
                        if names.resolve(module.name.as_name().text()) == "Outer"
                )
            })
            .expect("Outer object remains in the compilation unit");
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(outer)) = &result.ast.get(outer_id).kind
        else {
            unreachable!();
        };
        let TreeKind::Template(template) = &result.ast.get(outer.template).kind else {
            panic!("expected Outer template");
        };
        let inner = template
            .body
            .iter()
            .copied()
            .find(|id| {
                matches!(
                    &result.ast.get(*id).kind,
                    TreeKind::TypeDef(definition)
                        if names.resolve(definition.name.as_name().text()) == "Inner"
                )
            })
            .expect("Inner class remains in Outer");
        let after = template
            .body
            .iter()
            .copied()
            .find(|id| {
                matches!(
                    &result.ast.get(*id).kind,
                    TreeKind::ValDef(definition)
                        if names.resolve(definition.name.as_name().text()) == "after"
                )
            })
            .expect("member after the malformed marker remains in Outer");
        assert!(
            result.ast.get(inner).position.unwrap().span().range().end()
                < result
                    .ast
                    .get(after)
                    .position
                    .unwrap()
                    .span()
                    .range()
                    .start()
        );
        assert_eq!(
            result
                .ast
                .get(outer_id)
                .position
                .unwrap()
                .span()
                .range()
                .end(),
            SOURCE.len() as u32
        );
    }

    #[test]
    fn duplicate_nested_template_end_marker_preserves_outer_members_and_marker() {
        const SOURCE: &str = concat!(
            "object Outer:\n",
            "  class Inner:\n",
            "    val x = 1\n",
            "  end Inner\n",
            "  end Inner\n",
            "  val after = 2\n",
            "end Outer",
        );
        let scanner = ContextualScanner::new(SOURCE).expect("source scans");
        let source_text = SourceText::new(SOURCE).expect("source text is valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert!(
            result.diagnostics[0]
                .message()
                .contains("duplicate end marker")
        );
        let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
            panic!("expected package root");
        };
        let outer_id = package
            .stats
            .iter()
            .copied()
            .find(|id| {
                matches!(
                    &result.ast.get(*id).kind,
                    TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module))
                        if names.resolve(module.name.as_name().text()) == "Outer"
                )
            })
            .expect("Outer object remains in the compilation unit");
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(outer)) = &result.ast.get(outer_id).kind
        else {
            unreachable!();
        };
        let TreeKind::Template(template) = &result.ast.get(outer.template).kind else {
            panic!("expected Outer template");
        };
        assert!(template.body.iter().any(|id| matches!(
            &result.ast.get(*id).kind,
            TreeKind::ValDef(definition)
                if names.resolve(definition.name.as_name().text()) == "after"
        )));
        assert_eq!(
            result
                .ast
                .get(outer_id)
                .position
                .unwrap()
                .span()
                .range()
                .end(),
            SOURCE.len() as u32
        );
    }

    #[test]
    fn same_named_nested_end_marker_does_not_close_outer_template() {
        const SOURCE: &str = concat!(
            "object Outer:\n",
            "  class Outer:\n",
            "    val x = 1\n",
            "  end Outer\n",
            "  val after = 2\n",
            "end Outer",
        );
        let scanner = ContextualScanner::new(SOURCE).expect("source scans");
        let source_text = SourceText::new(SOURCE).expect("source text is valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
            panic!("expected package root");
        };
        let outer_id = package
            .stats
            .iter()
            .copied()
            .find(|id| {
                matches!(
                    &result.ast.get(*id).kind,
                    TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module))
                        if names.resolve(module.name.as_name().text()) == "Outer"
                )
            })
            .expect("Outer object remains in the compilation unit");
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(outer)) = &result.ast.get(outer_id).kind
        else {
            unreachable!();
        };
        let TreeKind::Template(template) = &result.ast.get(outer.template).kind else {
            panic!("expected Outer template");
        };
        assert!(template.body.iter().any(|id| matches!(
            &result.ast.get(*id).kind,
            TreeKind::TypeDef(definition)
                if names.resolve(definition.name.as_name().text()) == "Outer"
        )));
        assert!(template.body.iter().any(|id| matches!(
            &result.ast.get(*id).kind,
            TreeKind::ValDef(definition)
                if names.resolve(definition.name.as_name().text()) == "after"
        )));
        assert_eq!(
            result
                .ast
                .get(outer_id)
                .position
                .unwrap()
                .span()
                .range()
                .end(),
            SOURCE.len() as u32
        );
    }

    #[test]
    fn end_marker_at_outer_indentation_is_not_stolen_by_same_named_member() {
        const SOURCE: &str = "object Outer:\n  class Outer {}\nend Outer";
        let scanner = ContextualScanner::new(SOURCE).expect("source scans");
        let source_text = SourceText::new(SOURCE).expect("source text is valid");
        let mut names = NameInterner::new();
        let result =
            parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
            panic!("expected package root");
        };
        let outer_id = package
            .stats
            .iter()
            .copied()
            .find(|id| {
                matches!(
                    &result.ast.get(*id).kind,
                    TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module))
                        if names.resolve(module.name.as_name().text()) == "Outer"
                )
            })
            .expect("Outer object remains at package scope");
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(outer)) = &result.ast.get(outer_id).kind
        else {
            unreachable!();
        };
        let TreeKind::Template(template) = &result.ast.get(outer.template).kind else {
            panic!("expected outer template");
        };
        assert!(template.body.iter().any(|id| matches!(
            &result.ast.get(*id).kind,
            TreeKind::TypeDef(definition)
                if names.resolve(definition.name.as_name().text()) == "Outer"
        )));
        assert_eq!(
            result
                .ast
                .get(outer_id)
                .position
                .unwrap()
                .span()
                .range()
                .end(),
            SOURCE.len() as u32
        );
    }
}
