use super::*;

const LIB: &str =
    include_str!("../../../../tests/fixtures/contextual-arguments/ProviderExports.scala");

struct Invocation {
    method: SymbolId,
    types: Vec<TypeId>,
    clauses: Vec<Vec<SymbolId>>,
}

fn invocation(typer: &SourceTyper<'_>, mut tree: TreeId<Typed>) -> Invocation {
    let mut clauses = Vec::new();
    while let TreeKind::Apply(app) = &typer.typed_ast().get(tree).kind {
        clauses.push(app.args.iter().map(|&arg| reference(typer, arg)).collect());
        tree = app.function;
    }
    clauses.reverse();
    let mut types = Vec::new();
    if let TreeKind::TypeApply(app) = &typer.typed_ast().get(tree).kind {
        types = app
            .args
            .iter()
            .map(|&arg| typer.typed_ast().get(arg).ty)
            .collect();
        tree = app.function;
    }
    Invocation {
        method: reference(typer, tree),
        types,
        clauses,
    }
}

fn delegate(
    typer: &mut SourceTyper<'_>,
    method: SymbolId,
    type_arity: usize,
    clauses: &[&[&str]],
) -> SymbolId {
    let tree = typer.type_export_forwarder_definition(method).unwrap();
    let TreeKind::DefDef(def) = &typer.typed_ast().get(tree).kind else {
        panic!("export definition")
    };
    let body = invocation(typer, def.rhs.unwrap());
    let param = |tree| {
        let symbol = typer.typed_index.synthetic_definition_symbol(tree).unwrap();
        assert_eq!(typer.store.symbols.owner(symbol), Some(method));
        symbol
    };
    let type_symbols: Vec<_> = def.type_params.iter().map(|&tree| param(tree)).collect();
    let type_args: Vec<_> = body
        .types
        .iter()
        .map(|&ty| typer.store.types.get(ty).reference_symbol().unwrap())
        .collect();
    assert_eq!(type_symbols.len(), type_arity);
    assert_eq!(type_symbols, type_args);
    let term_symbols: Vec<Vec<_>> = def
        .value_param_clauses
        .iter()
        .map(|clause| clause.iter().map(|&tree| param(tree)).collect())
        .collect();
    let names: Vec<Vec<_>> = term_symbols
        .iter()
        .map(|params| {
            params
                .iter()
                .map(|&symbol| qualified_name(typer, symbol).1)
                .collect()
        })
        .collect();
    assert_eq!(names, clauses);
    assert_eq!(term_symbols, body.clauses);
    assert_ne!(
        typer.store.symbols.info(method),
        typer.store.symbols.info(body.method)
    );
    body.method
}

fn check_sites(
    sites: &[(&str, &str)],
    method: &str,
    expected_types: &[fn(&Definitions) -> TypeId],
    expected_clauses: &[&[&str]],
) {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(LIB);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let mut leaf = None;
    for &(site, owner) in sites {
        let call = client_result(&mut typer, &parsed, site).unwrap();
        let selected = invocation(&typer, evidence(&typer, call));
        assert_eq!(qualified_name(&typer, selected.method), (owner, method));
        assert_eq!(
            selected.types,
            expected_types.iter().map(|f| f(&defs)).collect::<Vec<_>>()
        );
        let names: Vec<Vec<_>> = selected
            .clauses
            .iter()
            .map(|args| {
                args.iter()
                    .map(|&sym| {
                        let (actual_owner, name) = qualified_name(&typer, sym);
                        assert_eq!(actual_owner, site);
                        name
                    })
                    .collect()
            })
            .collect();
        assert_eq!(names, expected_clauses);
        if let Some(original) = leaf {
            assert_eq!(
                delegate(
                    &mut typer,
                    selected.method,
                    expected_types.len(),
                    expected_clauses
                ),
                original
            );
        } else {
            leaf = Some(selected.method);
        }
    }
}

#[test]
fn monomorphic_provider_delegates_with_its_own_witness() {
    check_sites(
        &[("directMono", "Records"), ("viaMono", "Mono")],
        "mono",
        &[],
        &[&["k"]],
    );
}

#[test]
fn a_single_using_clause_forwards_both_witnesses_in_declaration_order() {
    check_sites(
        &[("directFlat", "Records"), ("viaFlat", "Flat")],
        "flat",
        &[|d| d.int],
        &[&["k", "t"]],
    );
}

#[test]
fn an_intra_clause_dependency_is_rejected_before_publishing_a_forwarder() {
    let input = "package multi
trait Family { type Out }
class Product
object Records { given product(using f: Family, v: f.Out): Product = new Product }
object Api { export Records.product }";
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(input);
    let site = parsed
        .ast
        .iter()
        .find_map(|(tree, _)| index.export_site_at(source, tree))
        .unwrap();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let before = typer.store.checkpoint();
    let error = typer.complete_symbol(site.owner).unwrap_err();
    assert!(
        matches!(error,TyperError::UnsupportedExpression {tree_index,expression_kind:"contextual export with a dependency within a parameter clause",..} if tree_index == site.tree.index()),
        "{error:?}"
    );
    assert_eq!(typer.store.checkpoint(), before);
}

#[test]
fn exported_binary_type_constructor_uses_both_inferred_arguments() {
    check_sites(
        &[("directPoly", "Records"), ("viaPoly", "Poly")],
        "poly",
        &[|d| d.int, |d| d.boolean],
        &[&["k", "t"]],
    );
}

#[test]
fn later_type_parameter_bounds_refer_to_the_forwarders_own_parameter() {
    let input = "package multi
class Key[A]
class Pair[A,B]
object Records { given poly[A,B <: A](using k: Key[A]): Pair[A,B] = new Pair[A,B] }
object Poly { export Records.poly }
class Client {
  def need[A](using x: A): A = x
  def viaPoly(using k: Key[Any]): Pair[Any,Boolean] = { import Poly.given; need[Pair[Any,Boolean]] }
}";
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(input);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let call = client_result(&mut typer, &parsed, "viaPoly").unwrap();
    let app = invocation(&typer, evidence(&typer, call));
    let tree = typer.type_export_forwarder_definition(app.method).unwrap();
    let TreeKind::DefDef(def) = &typer.typed_ast().get(tree).kind else {
        panic!()
    };
    let symbols: Vec<_> = def
        .type_params
        .iter()
        .map(|&p| typer.typed_index.synthetic_definition_symbol(p).unwrap())
        .collect();
    let SymbolInfo::Complete(bounds) = *typer.store.symbols.info(symbols[1]) else {
        panic!()
    };
    let Type::Bounds { high, .. } = typer.store.types.get(bounds) else {
        panic!()
    };
    assert_eq!(
        typer.store.types.get(*high).reference_symbol(),
        Some(symbols[0])
    );
}

#[test]
fn successive_using_clauses_remain_separate_at_the_call_and_in_the_delegate() {
    check_sites(
        &[("directCurried", "Records"), ("viaCurried", "Curried")],
        "curried",
        &[|d| d.int, |d| d.boolean],
        &[&["k"], &["t"]],
    );
}

#[test]
fn the_next_clause_sees_the_forwarders_own_stable_parameter() {
    let input = "package multi
trait Family { type Out }
class Product
object Records { given product(using f: Family)(using v: f.Out): Product = new Product }
object Api { export Records.product }";
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(input);
    let site = parsed
        .ast
        .iter()
        .find_map(|(tree, _)| index.export_site_at(source, tree))
        .unwrap();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    typer.complete_symbol(site.owner).unwrap();
    let method = typer
        .store
        .scopes
        .get(index.scope_of(site.owner).unwrap())
        .entered_symbols()
        .find(|&s| {
            typer
                .store
                .symbols
                .get(s)
                .flags
                .contains(SymbolFlags::EXPORTED)
        })
        .unwrap();
    let tree = typer.type_export_forwarder_definition(method).unwrap();
    let TreeKind::DefDef(def) = &typer.typed_ast().get(tree).kind else {
        panic!()
    };
    let first = typer
        .typed_index
        .synthetic_definition_symbol(def.value_param_clauses[0][0])
        .unwrap();
    let second = typer
        .typed_index
        .synthetic_definition_symbol(def.value_param_clauses[1][0])
        .unwrap();
    let SymbolInfo::Complete(ty) = *typer.store.symbols.info(second) else {
        panic!()
    };
    let Type::TypeRef { prefix, .. } = typer.store.types.get(ty) else {
        panic!("dependent member type")
    };
    assert_eq!(
        typer.store.types.get(*prefix).reference_symbol(),
        Some(first)
    );
}

#[test]
fn renamed_curried_provider_selects_its_immediate_exported_delegate() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(LIB);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let call = client_result(&mut typer, &parsed, "viaAlias").unwrap();
    let app = invocation(&typer, evidence(&typer, call));
    assert_eq!(qualified_name(&typer, app.method), ("Again", "derived"));
    assert_eq!(app.types, [defs.int, defs.boolean]);
    let clauses = &[&["k"][..], &["t"][..]];
    let names: Vec<Vec<_>> = app
        .clauses
        .iter()
        .map(|args| args.iter().map(|&s| qualified_name(&typer, s)).collect())
        .collect();
    assert_eq!(names, [vec![("viaAlias", "k")], vec![("viaAlias", "t")]]);
    let middle = delegate(&mut typer, app.method, 2, clauses);
    assert_eq!(qualified_name(&typer, middle), ("Curried", "curried"));
    let leaf = delegate(&mut typer, middle, 2, clauses);
    assert_eq!(qualified_name(&typer, leaf), ("Records", "curried"));
}

#[test]
fn legacy_wildcard_preserves_the_implicit_method_and_its_parameter_flags() {
    check_sites(
        &[("directLegacy", "Legacy"), ("viaLegacy", "Old")],
        "old",
        &[|d| d.int, |d| d.boolean],
        &[&["k", "t"]],
    );
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(LIB);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let call = client_result(&mut typer, &parsed, "viaLegacy").unwrap();
    let method = invocation(&typer, evidence(&typer, call)).method;
    let tree = typer.type_export_forwarder_definition(method).unwrap();
    let TreeKind::DefDef(def) = &typer.typed_ast().get(tree).kind else {
        panic!()
    };
    assert!(
        typer
            .store
            .symbols
            .get(method)
            .flags
            .contains(SymbolFlags::IMPLICIT | SymbolFlags::EXPORTED)
    );
    for &tree in &def.value_param_clauses[0] {
        let symbol = typer.typed_index.synthetic_definition_symbol(tree).unwrap();
        assert_eq!(
            typer.store.symbols.get(symbol).flags & (SymbolFlags::GIVEN | SymbolFlags::IMPLICIT),
            SymbolFlags::IMPLICIT
        );
    }
}

#[test]
fn failed_argument_search_reports_the_call_and_restores_all_export_scopes() {
    let bad =
        include_str!("../../../../tests/fixtures/contextual-arguments/ProviderExportErrors.scala");
    let input = format!("{LIB}\n{}", bad.strip_prefix("package multi\n").unwrap());
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(&input);
    let scopes: Vec<_> = parsed
        .ast
        .iter()
        .filter_map(|(tree, _)| index.export_site_at(source, tree))
        .map(|site| index.scope_of(site.owner).unwrap())
        .collect();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for warm in [false, true] {
        if warm {
            let call = client_result(&mut typer, &parsed, "viaAlias").unwrap();
            let alias = invocation(&typer, evidence(&typer, call)).method;
            assert_eq!(qualified_name(&typer, alias), ("Again", "derived"));
            delegate(&mut typer, alias, 2, &[&["k"], &["t"]]);
        }
        let checkpoint = typer.store.checkpoint();
        let tree_count = typer.typed_ast().iter().count();
        let scope_entries = |typer: &SourceTyper<'_>| {
            scopes
                .iter()
                .map(|&scope| {
                    typer
                        .store
                        .scopes
                        .get(scope)
                        .entered_symbols()
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        let before = scope_entries(&typer);
        for site in [
            "missingFirst",
            "missingSecond",
            "clashSecond",
            "directMissingFirst",
            "directMissingSecond",
            "directClashSecond",
        ] {
            for _ in 0..2 {
                let error = rejected_call(&mut typer, &parsed, site);
                if matches!(site, "clashSecond" | "directClashSecond") {
                    let TyperError::AmbiguousContextualArgument {
                        candidate_names, ..
                    } = error
                    else {
                        panic!("{site}: {error:?}")
                    };
                    let mut names: Vec<_> = candidate_names
                        .iter()
                        .map(|(owner, name)| {
                            (
                                typer.store.names.resolve(owner.unwrap().text()),
                                typer.store.names.resolve(name.text()),
                            )
                        })
                        .collect();
                    names.sort();
                    assert_eq!(names, [(site, "left"), (site, "right")]);
                } else {
                    assert!(
                        matches!(error, TyperError::MissingContextualArgument { .. }),
                        "{site}: {error:?}"
                    );
                }
                assert_eq!(typer.store.checkpoint(), checkpoint);
                assert_eq!(typer.typed_ast().iter().count(), tree_count);
                assert_eq!(scope_entries(&typer), before);
            }
        }
    }
}
