use super::*;
use crate::{CheckedClassBodies, ResolvedMemberIndex};

#[test]
fn overloaded_methods_own_their_exact_parameter_declarations_before_body_checking() {
    let text = "class A(x: Int) { def pick(x: Int): Int = 1; def pick(x: Boolean): Int = 2; def curried(y: Int)(z: Int): Int = y }";
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
    assert!(index.diagnostics().is_empty());
    let mut parameters = Vec::new();
    for ((offset, _), method) in text
        .match_indices("pick(x")
        .zip(file.classes()[0].methods())
    {
        let start = offset as u32 + 5;
        let p = index.parameter_at(file.source(), start).unwrap();
        assert_eq!(p.method, method.symbol);
        assert_eq!(p.target.name.start(), start);
        assert_eq!(p.target.name.end(), start + 1);
        let full = p.target.declaration;
        assert_eq!(
            &text[full.start() as usize..full.end() as usize],
            if parameters.is_empty() {
                "x: Int"
            } else {
                "x: Boolean"
            }
        );
        assert_eq!(store.symbols.get(p.parameter).owner, Some(p.method));
        assert!(index.parameter_at(file.source(), start + 1).is_none());
        assert!(index.references_to(p.parameter).is_empty());
        parameters.push(p);
    }
    assert_eq!(parameters.len(), 2);
    assert_ne!(parameters[0].parameter, parameters[1].parameter);
    assert_ne!(parameters[0].method, parameters[1].method);
    for name in ["x:", "y:", "z:"] {
        assert!(
            index
                .parameter_at(file.source(), text.find(name).unwrap() as u32)
                .is_none()
        );
    }
}
