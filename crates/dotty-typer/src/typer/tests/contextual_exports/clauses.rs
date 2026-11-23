use super::*;

const LIB: &str =
    include_str!("../../../../tests/fixtures/contextual-arguments/ProviderExports.scala");

#[test]
fn monomorphic_provider_delegates_with_its_own_witness() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(LIB);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for (site, owner) in [("directMono", "Records"), ("viaMono", "Mono")] {
        let call = client_result(&mut typer, &parsed, site).unwrap();
        let TreeKind::Apply(app) = &typer.typed_ast().get(evidence(&typer, call)).kind else {
            panic!("provider call")
        };
        let target = reference(&typer, app.function);
        assert_eq!(qualified_name(&typer, target), (owner, "mono"));
        assert_eq!(
            qualified_name(&typer, reference(&typer, app.args[0])),
            (site, "k")
        );
        if owner == "Mono" {
            let tree = typer.type_export_forwarder_definition(target).unwrap();
            let TreeKind::DefDef(def) = &typer.typed_ast().get(tree).kind else {
                panic!()
            };
            assert!(def.type_params.is_empty());
            let witness = typer
                .typed_index
                .synthetic_definition_symbol(def.value_param_clauses[0][0])
                .unwrap();
            assert_eq!(typer.store.symbols.owner(witness), Some(target));
            let TreeKind::Apply(app) = &typer.typed_ast().get(def.rhs.unwrap()).kind else {
                panic!()
            };
            assert_eq!(reference(&typer, app.args[0]), witness);
            let leaf = reference(&typer, app.function);
            assert_eq!(qualified_name(&typer, leaf), ("Records", "mono"));
            assert_ne!(
                typer.store.symbols.info(target),
                typer.store.symbols.info(leaf)
            );
        }
    }
}
