use dotty::classloader::{ClasspathSymbolResolver, LoadingSession};
use dotty::core::{
    Definitions, Packages, SemanticStore, SourceId, SymbolInfo, SymbolKind, SymbolOrigin, Type,
    TypeRefTarget,
};
use dotty::typer::{SignatureCompletion, TyperError};
use dotty::{ClassHeaderError, ClassHeaderPhase, NamedClassSources, SemanticPhase};
#[path = "../../external-dependency/fixture.rs"]
mod fixture;

const TEXT: &str = "package demo\nclass Holder(x: External)";

#[test]
fn binary_constructor_parameter_and_missing_dependency_use_the_same_source() {
    let fixture = fixture::ExternalDependency::compile();
    let path = fixture.classpath();
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
        let sources = NamedClassSources::new([&file], &store).unwrap();
        let mut arena = Default::default();
        let mut bindings = Default::default();
        let prepared = ClassHeaderPhase::try_prepare_with_constructor_resolver(
            &sources,
            &mut store,
            defs,
            &packages,
            &mut arena,
            &mut bindings,
            resolver,
        );
        if present {
            let mut phase = prepared.unwrap();
            assert!(matches!(
                phase.attempt(holder.symbol).unwrap().completion,
                SignatureCompletion::Complete(_)
            ));
            assert_eq!(phase.finish().ready, [holder.symbol]);
            let SymbolInfo::Complete(ty) = store.symbols.info(holder.constructor) else {
                panic!("completed constructor")
            };
            let Type::Method(sig) = store.types.get(*ty) else {
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
            let failure = prepared.unwrap_err();
            assert_eq!(
                (failure.phase, failure.source, failure.class),
                (SemanticPhase::Constructor, Some(id), holder.symbol)
            );
            let ClassHeaderError::Typing(report) = failure.error else {
                panic!("expected typed error")
            };
            assert_eq!(report.member(), Some(holder.constructor));
            let TyperError::TypeNameNotFound { source, name, .. } = report.error() else {
                panic!("{report:?}")
            };
            assert_eq!(*source, id);
            assert_eq!(store.names.resolve(name.text()), "External");
            let location = report.location().unwrap();
            let range = location.span().range();
            assert_eq!(location.source(), id);
            assert_eq!(
                &TEXT[range.start() as usize..range.end() as usize],
                "External"
            );
        }
    }
}

#[test]
fn a_method_can_be_the_first_use_of_the_external_type_in_a_session() {
    let fixture = fixture::ExternalDependency::compile();
    let path = fixture.classpath();
    let text = "package demo\nclass Echo { def echo(x: External): External = x }";
    for present in [true, false] {
        if !present {
            fixture.remove_external();
        }
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let id = SourceId::from_index(2);
        let file = dotty::parse_source(id, text, Default::default())
            .unwrap()
            .bind(&mut store.names)
            .name("Echo.scala", &mut store, &mut packages)
            .unwrap()
            .into_classes(&store)
            .unwrap();
        let owner = &file.classes()[0];
        let method = owner.methods().next().unwrap().symbol;
        let sources = NamedClassSources::new([&file], &store).unwrap();
        let mut arena = Default::default();
        let mut bindings = Default::default();
        let mut resolver = ClasspathSymbolResolver::new(
            &path,
            defs,
            LoadingSession::with_packages(packages.snapshot()),
        );
        let mut headers = ClassHeaderPhase::try_prepare_with_constructor_resolver(
            &sources,
            &mut store,
            defs,
            &packages,
            &mut arena,
            &mut bindings,
            &mut resolver,
        )
        .unwrap();
        assert!(matches!(
            headers.attempt(owner.symbol).unwrap().completion,
            SignatureCompletion::Complete(_)
        ));
        let selected = sources
            .member_signatures(headers.finish().ready, &store)
            .unwrap();
        let result = dotty::MemberSignaturePhase::new(&selected, &mut store, defs, &packages)
            .try_complete_roots([method], &mut resolver, |_, _| {});
        if present {
            let phase = result.unwrap();
            let (_, work) = phase.finish_with_body_work();
            let (_, report) = work.check(
                owner.symbol,
                &mut store,
                defs,
                &packages,
                &mut arena,
                &mut resolver,
            );
            assert!(report.errors.is_empty(), "{report:?}");
            let SymbolInfo::Complete(ty) = store.symbols.info(method) else {
                panic!("completed method")
            };
            let Type::Method(signature) = store.types.get(*ty) else {
                panic!("method type")
            };
            let Type::TypeRef {
                target: TypeRefTarget::Symbol(param),
                ..
            } = store.types.get(signature.params[0].ty)
            else {
                panic!("parameter class")
            };
            let Type::TypeRef {
                target: TypeRefTarget::Symbol(result),
                ..
            } = store.types.get(signature.result)
            else {
                panic!("result class")
            };
            assert_eq!(param, result);
            assert!(matches!(
                store.symbols.get(*param).origin,
                SymbolOrigin::Classfile(_)
            ));
        } else {
            assert!(
                matches!(result, Err(dotty::SignatureRunError::Rejected(dotty::MemberSignatureError::Typing {member,error:TyperError::TypeNameNotFound {source,..},..})) if member == method && source == id)
            );
        }
    }
}
