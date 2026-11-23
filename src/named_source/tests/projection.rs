use super::*;
use crate::source_declarations::read_source_classes;

fn named(store: &mut SemanticStore, text: &str) -> NamedSource {
    crate::parse_source(SourceId::from_index(7), text, Default::default())
        .unwrap()
        .bind(&mut store.names)
        .name("Model.scala", store, &mut Packages::new())
        .unwrap()
}

#[test]
fn classes_and_unsupported_sites_each_keep_package_order() {
    for traits in ["trait T", "trait T; trait U"] {
        let mut store = SemanticStore::new();
        let text = format!("class A; {traits}; class B");
        let file = named(&mut store, &text);
        let mut errors = Vec::new();
        let classes = read_source_classes(
            &file.syntax.ast,
            file.syntax.root,
            file.source(),
            file.index(),
            &store,
            |error| {
                errors.push(error);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            classes
                .iter()
                .map(|c| store.names.resolve(store.symbols.get(c.symbol).name.text()))
                .collect::<Vec<_>>(),
            ["A", "B"]
        );
        let sites: Vec<_> = errors.iter().map(|error| {
            assert!(matches!(error, DeclarationError::Unsupported {source, ..} if *source == file.source()));
            let range = error.location(&file.syntax.ast).unwrap().span().range();
            &text[range.start() as usize..range.end() as usize]
        }).collect();
        assert_eq!(sites, traits.split("; ").collect::<Vec<_>>());
        assert_eq!(file.into_classes(&store).unwrap_err().error, errors[0]);
    }
}
