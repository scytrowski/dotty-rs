use super::*;
use crate::core::{SourceDefinition, SymbolKind};
use crate::{CheckedClassBodies, ResolvedMemberIndex};

#[test]
fn constructor_parameters_pair_the_exact_source_and_formal_across_classes() {
    let text = "class A(z: Int, a: Int); class B(z: Int, a: Int)";
    let mut store = SemanticStore::new();
    let mut packages = Packages::new();
    let file = named(7, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&file], &store).unwrap();
    let index = ResolvedMemberIndex::build(
        &sources,
        &CheckedClassBodies::default(),
        &AstArena::new(),
        &store,
    );
    assert!(index.diagnostics().is_empty(), "{:?}", index.diagnostics());
    let mut ids = std::collections::HashSet::new();
    for (class, start) in file
        .classes()
        .iter()
        .zip(text.match_indices('(').map(|(i, _)| i + 1))
    {
        for offset in [start, start + 8] {
            let p = index
                .constructor_parameter_at(file.source(), offset as u32)
                .unwrap();
            assert_eq!((p.class, p.constructor), (class.symbol, class.constructor));
            assert_eq!(p.target.name.start(), offset as u32);
            assert_eq!(p.target.name.end(), offset as u32 + 1);
            let Some(SourceDefinition::Canonical { source, tree }) =
                file.index().definition_of(p.parameter)
            else {
                panic!("source formal")
            };
            assert_eq!(
                file.index()
                    .derived_symbol_at(class.constructor, source, tree),
                Some(p.formal)
            );
            for (symbol, owner) in [(p.parameter, p.class), (p.formal, p.constructor)] {
                assert!(ids.insert(symbol));
                assert_eq!(
                    (
                        store.symbols.get(symbol).kind,
                        store.symbols.get(symbol).owner
                    ),
                    (SymbolKind::Parameter, Some(owner))
                );
            }
            assert!(index.parameter_at(source, offset as u32).is_none());
        }
    }
}

#[test]
fn unsupported_constructor_forms_do_not_publish_plain_parameters() {
    let text = "class A(val field: Int, var state: Int, defaulted: Int = 1); class B(first: Int)(second: Int); class C(using context: Int)";
    let mut store = SemanticStore::new();
    let mut packages = Packages::new();
    let file = named(1, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&file], &store).unwrap();
    let index = ResolvedMemberIndex::build(
        &sources,
        &CheckedClassBodies::default(),
        &AstArena::new(),
        &store,
    );
    assert!(index.diagnostics().is_empty());
    for name in ["field", "state", "defaulted", "first", "second", "context"] {
        let offset = text.find(name).unwrap() as u32;
        assert!(
            index
                .constructor_parameter_at(file.source(), offset)
                .is_none()
        );
        assert_eq!(
            index.member_at(file.source(), offset).is_some(),
            matches!(name, "field" | "state")
        );
    }
}
