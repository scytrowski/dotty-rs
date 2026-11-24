use dotty::classloader::{
    ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath, JmodClassPath, LoadingSession,
};
use dotty::core::{
    Definitions, Packages, SemanticStore, SourceId, SymbolKind, SymbolOrigin, Type, TypeRefTarget,
};
use dotty::typer::{SourceTyper, TyperError};
#[path = "../../external-dependency/fixture.rs"]
mod fixture;

const TEXT: &str = "package demo\nclass Holder(x: External)";

#[test]
fn binary_constructor_parameter_and_missing_dependency_use_the_same_source() {
    let fixture = fixture::ExternalDependency::compile();
    let path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(&fixture.directory)),
        Box::new(JmodClassPath::new(&fixture.java_base).unwrap()),
    ]);
    for present in [true, false] {
        if !present {
            fixture.remove_external();
        }
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let id = SourceId::from_index(7);
        let file = dotty::parse_source(id, TEXT, Default::default())
            .unwrap()
            .bind(&mut store.names)
            .name("Holder.scala", &mut store, &mut packages)
            .unwrap()
            .into_classes(&store)
            .unwrap();
        let holder = &file.classes()[0];
        let resolver = ClasspathSymbolResolver::new(
            &path,
            defs,
            LoadingSession::with_packages(packages.snapshot()),
        );
        let result = SourceTyper::new(file.ast(), id, file.index(), &mut store, defs, &packages)
            .with_resolver(Box::new(resolver))
            .complete_symbol(holder.constructor);
        if present {
            let Type::Method(sig) = store.types.get(result.unwrap()) else {
                panic!("constructor signature")
            };
            let [param] = sig.params.as_slice() else {
                panic!("one parameter")
            };
            let Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } = store.types.get(param.ty)
            else {
                panic!("resolved type")
            };
            let entry = store.symbols.get(*symbol);
            assert_eq!(entry.kind, SymbolKind::Class);
            assert!(matches!(entry.origin, SymbolOrigin::Classfile(_)));
            assert_eq!(entry.owner, packages.symbol(&["demo"]));
            assert_eq!(store.names.resolve(entry.name.text()), "External");
        } else {
            assert!(
                matches!(result,Err(TyperError::TypeNameNotFound {source,position:Some(pos),..}) if source==id && pos.span().range().start()==TEXT.find("External").unwrap() as u32),
                "{result:?}"
            );
        }
    }
}
