//! Short, owned type descriptions for argument diagnostics.
use super::{
    SemanticStore, SourceId, SymbolId, TermRefTarget, Type, TypeId, TypeRefTarget, TyperError,
};

impl TyperError {
    pub(super) fn argument_type_mismatch(
        store: &SemanticStore,
        source: SourceId,
        tree_index: u32,
        argument_index: usize,
        actual: TypeId,
        expected: TypeId,
    ) -> Self {
        let description = |ty| describe(store, ty, &mut 32).into_boxed_str();
        Self::ApplicationArgumentTypeMismatch {
            source,
            tree_index,
            argument_index,
            actual,
            expected,
            actual_description: description(actual),
            expected_description: description(expected),
        }
    }
}

// A single node budget bounds both nested and wide types, including cyclic arena graphs.
fn describe(store: &SemanticStore, ty: TypeId, remaining: &mut usize) -> String {
    if *remaining == 0 {
        return "…".into();
    }
    *remaining -= 1;
    let symbol_name = |id: SymbolId| {
        if store.symbols.contains(id) {
            store.names.try_resolve(store.symbols.get(id).name.text())
        } else {
            None
        }
        .unwrap_or("<unknown symbol>")
    };
    let Some(ty) = store.types.try_get(ty) else {
        return "<unknown type>".into();
    };
    match ty {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(id),
            ..
        } => symbol_name(*id).into(),
        Type::TypeRef {
            target: TypeRefTarget::Name(name),
            ..
        } => store
            .names
            .try_resolve(name.as_name().text())
            .unwrap_or("<unknown type>")
            .into(),
        Type::TermRef { target, .. } => {
            let name = match target {
                TermRefTarget::Symbol(id) => symbol_name(*id),
                TermRefTarget::Name(name) => store
                    .names
                    .try_resolve(name.as_name().text())
                    .unwrap_or("<unknown term>"),
            };
            format!("{name}.type")
        }
        Type::ThisType { class } => format!("{}.this.type", symbol_name(*class)),
        Type::Applied { tycon, args } => {
            let mut label = describe(store, *tycon, remaining);
            label.push('[');
            for (i, arg) in args.iter().enumerate() {
                if i != 0 {
                    label.push_str(", ");
                }
                if *remaining == 0 {
                    label.push('…');
                    break;
                }
                label.push_str(&describe(store, *arg, remaining));
            }
            label.push(']');
            label
        }
        Type::And { left, right } | Type::Or { left, right } => {
            let operator = if matches!(ty, Type::And { .. }) {
                "&"
            } else {
                "|"
            };
            format!(
                "({} {operator} {})",
                describe(store, *left, remaining),
                describe(store, *right, remaining)
            )
        }
        Type::JavaArray { element } => format!("Array[{}]", describe(store, *element, remaining)),
        Type::ByName { result } => format!("=> {}", describe(store, *result, remaining)),
        Type::Repeated { element } => format!("{}*", describe(store, *element, remaining)),
        Type::ClassInfo(info) => symbol_name(info.class).into(),
        Type::Constant(_) => "<literal type>".into(),
        Type::Method(_) => "<method type>".into(),
        Type::Poly(_) => "<polymorphic type>".into(),
        Type::TypeLambda(_) => "<type lambda>".into(),
        Type::ParamRef { .. } => "<parameter reference>".into(),
        Type::Refined { .. } => "<refined type>".into(),
        Type::Match(_) | Type::MatchCase { .. } => "<match type>".into(),
        Type::Recursive { .. } | Type::RecThis { .. } => "<recursive type>".into(),
        Type::Bounds { .. } => "<type bounds>".into(),
        Type::Flexible { .. } => "<flexible type>".into(),
        Type::Annotated { .. } => "<annotated type>".into(),
        Type::AliasingBounds { .. } => "<type alias>".into(),
        Type::Wildcard { .. } => "?".into(),
        Type::SuperType { .. } => "<super type>".into(),
        Type::NoType | Type::NoPrefix | Type::Error(_) => "<unknown type>".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::Definitions;

    #[test]
    fn descriptions_abbreviate_wide_applications_at_the_node_limit() {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let args = store.types.alloc(Type::Applied {
            tycon: defs.object_type,
            args: vec![defs.int, defs.boolean, defs.char],
        });
        assert_eq!(describe(&store, args, &mut 3), "Object[Int, …]");
    }

    #[test]
    fn both_descriptions_survive_reuse_of_rolled_back_type_slots() {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let before = store.checkpoint();
        let actual = store.types.alloc(Type::JavaArray { element: defs.int });
        let expected = store.types.alloc(Type::JavaArray {
            element: defs.boolean,
        });
        let error = TyperError::argument_type_mismatch(
            &store,
            SourceId::from_index(0),
            3,
            0,
            actual,
            expected,
        );
        store.rollback_to(before);
        assert_eq!(store.types.alloc(Type::NoType), actual);
        assert_eq!(store.types.alloc(Type::NoPrefix), expected);
        drop(store);
        let TyperError::ApplicationArgumentTypeMismatch {
            actual_description,
            expected_description,
            ..
        } = error
        else {
            unreachable!()
        };
        assert_eq!(&*actual_description, "Array[Int]");
        assert_eq!(&*expected_description, "Array[Boolean]");
    }
}
