#[allow(dead_code)]
mod support;

use dotty_tasty::tasty::*;
use dotty_tasty_pickler::*;
use support::Fixture;

fn emit(f: &Fixture) -> Result<DeclarationSkeleton, PickleError> {
    f.plan().validate()?.declaration_skeleton()
}

#[test]
fn canonical_declarations_have_bound_addresses_and_explicitly_omitted_bodies() {
    let f = Fixture::new();
    let before = f.snapshot();
    let output = emit(&f).unwrap();
    let file = TastyFile::parse_and_validate_scala_3_9(output.provisional_bytes()).unwrap();
    assert!(file.header().tooling_version.contains("incomplete"));
    assert_eq!(output.intended_tasty_path(), "example/Answer.tasty");
    assert_eq!(output.source().id, f.source);
    assert_eq!(file.source_file().unwrap(), Some("Answer.scala"));
    assert_eq!(
        output.omitted_bodies(),
        &[OmittedMethodBody {
            method: f.methods[0].symbol,
            typed_root: f.body
        }]
    );
    let index = file.ast_address_index().unwrap();
    for (binding, (symbol, owner, tag)) in output.declarations().iter().zip([
        (f.class, f.package, TYPEDEF_TAG),
        (f.constructor.symbol, f.class, DEFDEF_TAG),
        (f.methods[0].symbol, f.class, DEFDEF_TAG),
    ]) {
        assert_eq!((binding.symbol, binding.owner), (symbol, owner));
        assert_eq!(index.get_node(binding.address).unwrap().tag, tag);
        let name = index
            .get(binding.address)
            .unwrap()
            .decode_definition()
            .unwrap()
            .name();
        assert_eq!(
            file.render_name(name).unwrap(),
            f.store
                .names
                .resolve(f.store.symbols.get(symbol).name.text())
        );
    }
    assert_eq!(output.declarations().len(), 3);
    assert_eq!(file.ast_nodes_with_tag(INTCONST_TAG).unwrap(), []);
    assert_eq!(file.ast_nodes_with_tag(ELIDED_TAG).unwrap(), []);
    let ctor = index
        .get(output.declarations()[1].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    assert_eq!(ctor.clauses, [EMPTYCLAUSE_TAG]);
    assert!(ctor.parameters.is_empty());
    assert!(ctor.rhs.is_none());
    assert_eq!(ctor.tail, [DefinitionTail::Modifier(STABLE_TAG)]);
    let method = index
        .get(output.declarations()[2].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    assert!(method.clauses.is_empty());
    assert!(method.parameters.is_empty());
    assert!(method.rhs.is_none());
    assert!(method.tail.is_empty());
    assert!(matches!(
        method.return_type,
        RawTree::NatAst {
            tag: IDENTTPT_TAG,
            ..
        }
    ));
    assert_eq!(f.snapshot(), before);
}
