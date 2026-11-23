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

fn delegate(typer: &mut SourceTyper<'_>, method: SymbolId) -> SymbolId {
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
    assert_eq!(type_symbols, type_args);
    let term_symbols: Vec<Vec<_>> = def
        .value_param_clauses
        .iter()
        .map(|clause| clause.iter().map(|&tree| param(tree)).collect())
        .collect();
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
            assert_eq!(delegate(&mut typer, selected.method), original);
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
