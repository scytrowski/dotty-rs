//! Source name ranges for committed ordinary locals owned by class methods.
use crate::core::{SemanticStore, SymbolId, TreeKind};
use crate::{CheckedClassBodies, NamedClassSource, SourceFileCalls, SourceMemberTarget};

pub(crate) fn declarations(
    file: &NamedClassSource,
    navigation: &SourceFileCalls,
    bodies: &CheckedClassBodies,
    store: &SemanticStore,
) -> Vec<(SymbolId, SourceMemberTarget)> {
    let mut declarations = Vec::new();
    for class in file.classes() {
        let Some(body) = bodies.get(class.symbol) else {
            continue;
        };
        for binding in body.local_bindings() {
            let (source, tree) = binding.source_declaration();
            if source != file.source() || !store.symbols.contains(binding.symbol()) {
                continue;
            }
            let Some(owner) = store.symbols.get(binding.symbol()).owner else {
                continue;
            };
            // Exclude locals from other owners, e.g. field initializers and local methods.
            if navigation.target(source, owner).is_none() {
                continue;
            }
            let Some(name) = file.named().syntax().member_names.value_name(tree) else {
                continue;
            };
            let Some(node) = file.ast().try_get(tree) else {
                continue;
            };
            let Some(position) = node.position.filter(|p| p.source() == source) else {
                continue;
            };
            let declaration = position.span().range();
            if matches!(node.kind, TreeKind::ValDef(_))
                && declaration.start() <= name.start()
                && name.end() <= declaration.end()
            {
                declarations.push((
                    binding.symbol(),
                    SourceMemberTarget {
                        source,
                        declaration,
                        name,
                    },
                ));
            }
        }
    }
    declarations.sort_unstable_by_key(|(_, target)| target.name.start());
    declarations
}
