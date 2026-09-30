//! Public-API characterization tests for the typer refactor contract.
//!
//! Later typer refactors must keep these semantic assertions unchanged unless
//! a separately scoped behavior issue explicitly updates the contract.

use std::collections::HashSet;

use dotty_core::ast::{AstArena, TreeKind, Typed, Untyped};
use dotty_core::types::{TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceSemanticIndex, SourceText, SymbolId,
    SymbolInfo, SymbolKind, TreeId, TypeId,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_typer::{SourceTyper, TyperError};

const SOURCE: SourceId = SourceId::from_index(0);

struct Fixture {
    ast: AstArena<Untyped>,
    index: SourceSemanticIndex,
    store: SemanticStore,
    definitions: Definitions,
    packages: Packages,
}

impl Fixture {
    fn new(text: &str) -> Self {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let scanner = ContextualScanner::new(text).expect("synthetic scanner should construct");
        let parsed = dotty_parser::parse_compilation_unit(
            SourceText::new(text).expect("synthetic source should be valid"),
            SOURCE,
            scanner,
            &mut store.names,
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let mut packages = Packages::new();
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            SOURCE,
            "refactor-contract.scala",
            &mut store,
            &mut packages,
        )
        .expect("synthetic source should be nameable");

        Self {
            ast: parsed.ast,
            index,
            store,
            definitions,
            packages,
        }
    }

    fn methods_named(&self, name: &str) -> Vec<(TreeId<Untyped>, SymbolId, TreeId<Untyped>)> {
        self.ast
            .iter()
            .filter_map(|(tree, node)| {
                let TreeKind::DefDef(definition) = &node.kind else {
                    return None;
                };
                if self.store.names.resolve(definition.name.as_name().text()) != name {
                    return None;
                }
                Some((tree, self.index.symbol_at(SOURCE, tree)?, definition.rhs?))
            })
            .collect()
    }

    fn declaration_tree(&self, name: &str, kind: SymbolKind) -> TreeId<Untyped> {
        self.ast
            .iter()
            .find_map(|(tree, node)| {
                let declaration_name = match &node.kind {
                    TreeKind::ValDef(definition) => definition.name.as_name(),
                    TreeKind::DefDef(definition) => definition.name.as_name(),
                    _ => return None,
                };
                if self.store.names.resolve(declaration_name.text()) != name {
                    return None;
                }
                let symbol = self.index.symbol_at(SOURCE, tree)?;
                (self.store.symbols.get(symbol).kind == kind).then_some(tree)
            })
            .expect("expected source declaration tree")
    }

    fn method(&self, name: &str) -> (TreeId<Untyped>, SymbolId, TreeId<Untyped>) {
        let matches = self.methods_named(name);
        assert_eq!(matches.len(), 1, "expected one method named {name}");
        matches[0]
    }

    fn symbol_named(&self, name: &str, kind: SymbolKind) -> SymbolId {
        self.ast
            .iter()
            .find_map(|(tree, _node)| {
                let symbol = self.index.symbol_at(SOURCE, tree)?;
                let value = self.store.symbols.get(symbol);
                (value.kind == kind && self.store.names.resolve(value.name.text()) == name)
                    .then_some(symbol)
            })
            .expect("expected named symbol")
    }

    fn typer(&mut self) -> SourceTyper<'_> {
        SourceTyper::new(
            &self.ast,
            SOURCE,
            &self.index,
            &mut self.store,
            self.definitions,
            &self.packages,
        )
    }
}

#[test]
fn normalized_observation_uses_semantic_names_and_shapes() {
    let mut fixture = Fixture::new("class C { val value: Int = 1; def use: Int = this.value }");
    let (_, method, rhs) = fixture.method("use");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();

    let snapshot = tree_snapshot(&typer, typed);
    assert_eq!(
        snapshot,
        "Select(value,Field:C.value,TermRef(Field:C.value),This(_):ThisType(Class:C))"
    );
    assert_eq!(typer.source_typed_index().get(SOURCE, rhs), Some(typed));
}

#[test]
fn normalized_symbol_info_omits_type_arena_indices() {
    let mut fixture = Fixture::new("class C { val value: Int = 1 }");
    let value = fixture.symbol_named("value", SymbolKind::Field);
    let mut typer = fixture.typer();
    typer.complete_symbol(value).unwrap();

    assert_eq!(
        symbol_info_snapshot(&typer, value),
        "Complete(TypeRef(Class:Int))"
    );
}

#[test]
fn normalized_errors_keep_variants_and_semantic_payloads() {
    let mut fixture =
        Fixture::new("class C { def target(x: Int): Int = x; def use: Int = target() }");
    let (_, method, rhs) = fixture.method("use");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();

    let error = typer.type_expression(rhs, context).unwrap_err();

    assert_eq!(
        error_snapshot(&typer, &error),
        "ApplicationArityMismatch(expected=1,actual=0)"
    );
}

#[test]
fn local_val_shadows_a_method_parameter() {
    let mut fixture =
        Fixture::new("class C { def use(value: Int): Int = { val value = 2; value } }");
    let (method_tree, method, rhs) = fixture.method("use");
    let parameter_tree = match &fixture.ast.get(method_tree).kind {
        TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
        _ => unreachable!(),
    };
    let parameter = fixture.index.symbol_at(SOURCE, parameter_tree).unwrap();
    let local_tree = fixture
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            (fixture
                .store
                .names
                .resolve(definition.name.as_name().text())
                == "value"
                && fixture.index.symbol_at(SOURCE, tree).is_none())
            .then_some(tree)
        })
        .unwrap();
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();
    let local = typer.local_symbol_at(SOURCE, local_tree).unwrap();
    let block_expr = match &typer.typed_ast().get(typed).kind {
        TreeKind::Block(block) => block.expr,
        _ => panic!("method body should remain a block"),
    };

    assert_ne!(local, parameter);
    assert_eq!(term_ref_symbol(&typer, block_expr), Some(local));
    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(block_expr).ty),
        "TermRef(Local:C.use.value)"
    );
}

#[test]
fn method_parameter_shadows_a_class_member() {
    let mut fixture =
        Fixture::new("class C { val value: Int = 1; def use(value: Int): Int = value }");
    let (method_tree, method, rhs) = fixture.method("use");
    let parameter_tree = match &fixture.ast.get(method_tree).kind {
        TreeKind::DefDef(definition) => definition.value_param_clauses[0][0],
        _ => unreachable!(),
    };
    let parameter = fixture.index.symbol_at(SOURCE, parameter_tree).unwrap();
    let field_tree = fixture.declaration_tree("value", SymbolKind::Field);
    let field = fixture.index.symbol_at(SOURCE, field_tree).unwrap();
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    let typed = typer.type_expression(rhs, context).unwrap();

    assert_ne!(parameter, field);
    assert_eq!(term_ref_symbol(&typer, typed), Some(parameter));
    assert_eq!(
        type_snapshot(&typer, typer.typed_ast().get(typed).ty),
        "TermRef(Parameter:C.use.value)"
    );
}

#[test]
fn local_method_forward_reference_resolves_to_its_later_declaration() {
    let mut fixture = Fixture::new(
        "object C { def outer: Int = { val before = later(); def later(): Int = 1; before } }",
    );
    let (_, method, rhs) = fixture.method("outer");
    let later_tree = fixture
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            (fixture
                .store
                .names
                .resolve(definition.name.as_name().text())
                == "later")
                .then_some(tree)
        })
        .unwrap();
    let reference_tree = fixture
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::Ident(ident) = &node.kind else {
                return None;
            };
            (fixture.store.names.resolve(ident.name.text()) == "later").then_some(tree)
        })
        .unwrap();
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();
    typer.type_expression(rhs, context).unwrap();
    let local_method = typer.local_method_symbol_at(SOURCE, later_tree).unwrap();
    let typed_reference = typer
        .source_typed_index()
        .get(SOURCE, reference_tree)
        .unwrap();

    assert_eq!(term_ref_symbol(&typer, typed_reference), Some(local_method));
    assert_eq!(symbol_label(&typer, local_method), "Method:C$.outer.later");
}

#[test]
fn nested_block_declarations_do_not_leak_to_the_outer_block() {
    let mut fixture =
        Fixture::new("class C { def use: Int = { { val hidden = 1; hidden }; hidden } }");
    let (_, method, rhs) = fixture.method("use");
    let mut typer = fixture.typer();
    let context = typer.expression_context_for(method).unwrap();

    let error = typer.type_expression(rhs, context).unwrap_err();

    assert_eq!(error_snapshot(&typer, &error), "TermNameNotFound(hidden)");
}

#[test]
fn imported_source_name_is_used_after_local_scopes_miss() {
    let mut fixture = Fixture::new(
        "object Library { val answer: Int = 42 }; object User { import Library.answer; def fallback: Int = answer; def local: Int = { val answer = 1; answer } }",
    );
    let (_, fallback_method, fallback_rhs) = fixture.method("fallback");
    let (_, local_method, local_rhs) = fixture.method("local");
    let imported = fixture.declaration_tree("answer", SymbolKind::Field);
    let imported_symbol = fixture.index.symbol_at(SOURCE, imported).unwrap();
    let local_tree = fixture
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            (fixture
                .store
                .names
                .resolve(definition.name.as_name().text())
                == "answer"
                && fixture.index.symbol_at(SOURCE, tree).is_none())
            .then_some(tree)
        })
        .unwrap();
    let mut typer = fixture.typer();
    let fallback_context = typer.expression_context_for(fallback_method).unwrap();
    let fallback = typer
        .type_expression(fallback_rhs, fallback_context)
        .unwrap();
    let local_context = typer.expression_context_for(local_method).unwrap();
    let local = typer.type_expression(local_rhs, local_context).unwrap();
    let local_symbol = typer.local_symbol_at(SOURCE, local_tree).unwrap();

    assert_eq!(term_ref_symbol(&typer, fallback), Some(imported_symbol));
    let local_expr = match &typer.typed_ast().get(local).kind {
        TreeKind::Block(block) => block.expr,
        _ => panic!("local method body should remain a block"),
    };
    assert_eq!(term_ref_symbol(&typer, local_expr), Some(local_symbol));
}

fn tree_snapshot(typer: &SourceTyper<'_>, tree: TreeId<Typed>) -> String {
    let node = typer.typed_ast().get(tree);
    let ty = type_snapshot(typer, node.ty);
    let shape = match &node.kind {
        TreeKind::Ident(ident) => format!("Ident({})", name(typer, ident.name.text())),
        TreeKind::This(this) => format!(
            "This({})",
            this.qual
                .map(|qual| name(typer, qual.text()))
                .unwrap_or_else(|| "_".to_owned())
        ),
        TreeKind::Literal(literal) => format!("Literal({:?})", literal.value),
        TreeKind::Select(select) => {
            let reference = match typer.store().types.get(node.ty) {
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } => symbol_label(typer, *symbol),
                _ => "unresolved".to_owned(),
            };
            format!(
                "Select({},{},{},{})",
                name(typer, select.name.text()),
                reference,
                ty,
                tree_snapshot(typer, select.qualifier)
            )
        }
        TreeKind::Apply(application) => format!(
            "Apply({:?},{},{})",
            application.kind,
            tree_snapshot(typer, application.function),
            application
                .args
                .iter()
                .map(|arg| tree_snapshot(typer, *arg))
                .collect::<Vec<_>>()
                .join(";")
        ),
        TreeKind::TypeApply(application) => format!(
            "TypeApply({},{},{})",
            tree_snapshot(typer, application.function),
            application
                .args
                .iter()
                .map(|arg| tree_snapshot(typer, *arg))
                .collect::<Vec<_>>()
                .join(";"),
            ty
        ),
        TreeKind::New(new) => format!("New({},{ty})", tree_snapshot(typer, new.tpt)),
        TreeKind::Typed(typed) => format!(
            "Typed({},{},{ty})",
            tree_snapshot(typer, typed.expr),
            tree_snapshot(typer, typed.tpt)
        ),
        TreeKind::Assign(assign) => format!(
            "Assign({},{},{ty})",
            tree_snapshot(typer, assign.lhs),
            tree_snapshot(typer, assign.rhs)
        ),
        TreeKind::Block(block) => format!(
            "Block([{}],{},{ty})",
            block
                .stats
                .iter()
                .map(|stat| tree_snapshot(typer, *stat))
                .collect::<Vec<_>>()
                .join(";"),
            tree_snapshot(typer, block.expr)
        ),
        TreeKind::If(conditional) => format!(
            "If({},{},{},{ty})",
            tree_snapshot(typer, conditional.cond),
            tree_snapshot(typer, conditional.then_branch),
            tree_snapshot(typer, conditional.else_branch)
        ),
        TreeKind::Return(return_tree) => format!(
            "Return({},{ty})",
            return_tree
                .expr
                .map(|expr| tree_snapshot(typer, expr))
                .unwrap_or_else(|| "()".to_owned())
        ),
        TreeKind::While(while_tree) => format!(
            "While({},{},{ty})",
            tree_snapshot(typer, while_tree.cond),
            tree_snapshot(typer, while_tree.body)
        ),
        _ => format!("Other({ty})"),
    };
    if matches!(node.kind, TreeKind::Select(_)) {
        shape
    } else {
        format!("{shape}:{ty}")
    }
}

fn type_snapshot(typer: &SourceTyper<'_>, ty: TypeId) -> String {
    type_snapshot_inner(typer, ty, &mut Vec::new(), &mut HashSet::new(), 0)
}

fn type_snapshot_inner(
    typer: &SourceTyper<'_>,
    ty: TypeId,
    binders: &mut Vec<(TypeId, Vec<String>)>,
    active: &mut HashSet<TypeId>,
    depth: usize,
) -> String {
    if depth > 64 {
        return "<depth-limit>".to_owned();
    }
    if !active.insert(ty) {
        return "<recursive>".to_owned();
    }
    let child = |typer: &SourceTyper<'_>, id, binders: &mut Vec<_>, active: &mut HashSet<_>| {
        type_snapshot_inner(typer, id, binders, active, depth + 1)
    };
    let result = match typer.store().types.get(ty) {
        Type::NoType => "NoType".to_owned(),
        Type::NoPrefix => "NoPrefix".to_owned(),
        Type::Error(_) => "Error".to_owned(),
        Type::TermRef { target, .. } => match target {
            TermRefTarget::Symbol(symbol) => format!("TermRef({})", symbol_label(typer, *symbol)),
            TermRefTarget::Name(name_id) => {
                format!("TermRef({})", name(typer, name_id.as_name().text()))
            }
        },
        Type::TypeRef { target, .. } => match target {
            TypeRefTarget::Symbol(symbol) => format!("TypeRef({})", symbol_label(typer, *symbol)),
            TypeRefTarget::Name(name_id) => {
                format!("TypeRef({})", name(typer, name_id.as_name().text()))
            }
        },
        Type::ThisType { class } => format!("ThisType({})", symbol_label(typer, *class)),
        Type::SuperType {
            this_type,
            super_type,
        } => format!(
            "Super({},{})",
            child(typer, *this_type, binders, active),
            child(typer, *super_type, binders, active)
        ),
        Type::Constant(value) => format!("Constant({value:?})"),
        Type::Applied { tycon, args } => format!(
            "{}[{}]",
            child(typer, *tycon, binders, active),
            args.iter()
                .map(|arg| child(typer, *arg, binders, active))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Type::Bounds { low, high } => format!(
            "Bounds({},{})",
            child(typer, *low, binders, active),
            child(typer, *high, binders, active)
        ),
        Type::AliasingBounds { alias } => {
            format!("Alias({})", child(typer, *alias, binders, active))
        }
        Type::ByName { result } => format!("ByName({})", child(typer, *result, binders, active)),
        Type::Flexible { underlying } => {
            format!("Flexible({})", child(typer, *underlying, binders, active))
        }
        Type::And { left, right } => format!(
            "And({},{})",
            child(typer, *left, binders, active),
            child(typer, *right, binders, active)
        ),
        Type::Or { left, right } => format!(
            "Or({},{})",
            child(typer, *left, binders, active),
            child(typer, *right, binders, active)
        ),
        Type::Refined {
            parent,
            name: member,
            info,
        } => format!(
            "Refined({};{}:{})",
            child(typer, *parent, binders, active),
            name(typer, member.text()),
            child(typer, *info, binders, active)
        ),
        Type::Recursive { parent } => {
            format!("Recursive({})", child(typer, *parent, binders, active))
        }
        Type::RecThis { binder } => binders
            .iter()
            .rev()
            .find(|(id, _)| id == binder)
            .map(|(_, names)| format!("RecThis({})", names.first().cloned().unwrap_or_default()))
            .unwrap_or_else(|| "RecThis".to_owned()),
        Type::Method(method) => format!(
            "Method<{:?}>({})->{}",
            method.kind,
            method
                .params
                .iter()
                .map(|param| format!(
                    "{}:{}",
                    name(typer, param.name.as_name().text()),
                    child(typer, param.ty, binders, active)
                ))
                .collect::<Vec<_>>()
                .join(","),
            child(typer, method.result, binders, active)
        ),
        Type::Poly(poly) => {
            let names = poly
                .params
                .iter()
                .map(|param| name(typer, param.name.as_name().text()))
                .collect::<Vec<_>>();
            binders.push((ty, names.clone()));
            let bounds = poly
                .params
                .iter()
                .map(|param| child(typer, param.bounds, binders, active))
                .collect::<Vec<_>>()
                .join(",");
            let result = child(typer, poly.result, binders, active);
            binders.pop();
            format!("Poly[{}]({bounds})->{result}", names.join(","))
        }
        Type::TypeLambda(lambda) => {
            let names = lambda
                .params
                .iter()
                .map(|param| name(typer, param.name.as_name().text()))
                .collect::<Vec<_>>();
            binders.push((ty, names.clone()));
            let result = child(typer, lambda.result, binders, active);
            binders.pop();
            format!("TypeLambda[{}]=>{result}", names.join(","))
        }
        Type::ParamRef { binder, index } => binders
            .iter()
            .rev()
            .find(|(id, _)| id == binder)
            .and_then(|(_, names)| names.get(*index as usize))
            .cloned()
            .unwrap_or_else(|| format!("Param[{index}]")),
        Type::Match(matched) => format!(
            "Match({};{};[{}])",
            child(typer, matched.bound, binders, active),
            child(typer, matched.scrutinee, binders, active),
            matched
                .cases
                .iter()
                .map(|case| child(typer, *case, binders, active))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Type::MatchCase { pattern, result } => format!(
            "MatchCase({},{})",
            child(typer, *pattern, binders, active),
            child(typer, *result, binders, active)
        ),
        Type::Annotated { underlying, .. } => {
            format!("Annotated({})", child(typer, *underlying, binders, active))
        }
        Type::Wildcard { bounds } => {
            format!("Wildcard({})", child(typer, *bounds, binders, active))
        }
        Type::JavaArray { element } => {
            format!("Array[{}]", child(typer, *element, binders, active))
        }
        Type::Repeated { element } => {
            format!("Repeated({})", child(typer, *element, binders, active))
        }
        Type::ClassInfo(info) => format!(
            "ClassInfo({},[{}])",
            symbol_label(typer, info.class),
            info.parents
                .iter()
                .map(|parent| child(typer, *parent, binders, active))
                .collect::<Vec<_>>()
                .join(",")
        ),
    };
    active.remove(&ty);
    result
}

fn symbol_info_snapshot(typer: &SourceTyper<'_>, symbol: SymbolId) -> String {
    match typer.store().symbols.info(symbol) {
        SymbolInfo::Missing => "Missing".to_owned(),
        SymbolInfo::Deferred(_) => "Deferred".to_owned(),
        SymbolInfo::Error => "Error".to_owned(),
        SymbolInfo::Complete(ty) => {
            format!("Complete({})", type_snapshot(typer, *ty))
        }
    }
}

fn symbol_label(typer: &SourceTyper<'_>, symbol: SymbolId) -> String {
    let value = typer.store().symbols.get(symbol);
    format!(
        "{:?}:{}",
        value.kind,
        symbol_path(typer, symbol, &mut HashSet::new())
    )
}

fn symbol_path(typer: &SourceTyper<'_>, symbol: SymbolId, seen: &mut HashSet<SymbolId>) -> String {
    if !seen.insert(symbol) {
        return "<owner-cycle>".to_owned();
    }
    let value = typer.store().symbols.get(symbol);
    let current = name(typer, value.name.text());
    if value.kind == SymbolKind::Package && current.is_empty() {
        return String::new();
    }
    match value.owner {
        Some(owner) => {
            let parent = symbol_path(typer, owner, seen);
            if parent.is_empty() {
                current
            } else {
                format!("{parent}.{current}")
            }
        }
        None => current,
    }
}

fn name(typer: &SourceTyper<'_>, name: dotty_core::NameId) -> String {
    typer.store().names.resolve(name).to_owned()
}

fn error_variant(error: &TyperError) -> String {
    let debug = format!("{error:?}");
    let end = debug.find(['{', '(']).unwrap_or(debug.len());
    debug[..end].trim().to_owned()
}

fn error_snapshot(typer: &SourceTyper<'_>, error: &TyperError) -> String {
    match error {
        TyperError::ApplicationArityMismatch {
            expected, actual, ..
        } => format!(
            "{}(expected={expected},actual={actual})",
            error_variant(error)
        ),
        TyperError::TermNameNotFound { name: missing, .. } => {
            format!("TermNameNotFound({})", name(typer, missing.text()))
        }
        _ => error_variant(error),
    }
}

fn term_ref_symbol(typer: &SourceTyper<'_>, tree: TreeId<Typed>) -> Option<SymbolId> {
    match typer.store().types.get(typer.typed_ast().get(tree).ty) {
        Type::TermRef { target, .. } => target.symbol(),
        _ => None,
    }
}
