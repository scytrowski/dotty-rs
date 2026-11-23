//! Bind written using arguments to the selected method clause.
use super::*;

pub(super) struct NamedArgument {
    pub(super) written: usize,
    pub(super) value: TreeId<Untyped>,
}

impl SourceTyper<'_> {
    pub(super) fn has_named_using_arguments(
        &self,
        kind: ApplyKind,
        trees: &[TreeId<Untyped>],
    ) -> bool {
        kind == ApplyKind::Using
            && trees
                .iter()
                .any(|&t| matches!(self.arena.get(t).kind, TreeKind::NamedArg(_)))
    }

    pub(super) fn named_using_arguments(
        &self,
        kind: ApplyKind,
        method: &MethodType,
        trees: &[TreeId<Untyped>],
        tree_index: u32,
    ) -> Result<Option<Vec<Option<NamedArgument>>>, TyperError> {
        if !self.has_named_using_arguments(kind, trees) {
            return Ok(None);
        }
        let ordinals: HashMap<_, _> = method
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| (*p.name.as_name(), i))
            .collect();
        let mut slots: Vec<_> = (0..method.params.len()).map(|_| None).collect();
        for (written, &tree) in trees.iter().enumerate() {
            let (ordinal, value) = if let TreeKind::NamedArg(arg) = self.arena.get(tree).kind {
                let ordinal = ordinals
                    .get(&arg.name)
                    .ok_or(TyperError::UnknownNamedArgument {
                        source: self.source,
                        tree_index: tree.index(),
                        name: arg.name,
                    })?;
                (*ordinal, arg.arg)
            } else {
                (written, tree)
            };
            let Some(slot) = slots.get_mut(ordinal) else {
                return Err(TyperError::ApplicationArityMismatch {
                    source: self.source,
                    tree_index,
                    expected: method.params.len(),
                    actual: trees.len(),
                });
            };
            if slot.is_some() {
                return Err(TyperError::DuplicateNamedArgument {
                    source: self.source,
                    tree_index: tree.index(),
                    name: *method.params[ordinal].name.as_name(),
                });
            }
            *slot = Some(NamedArgument { written, value });
        }
        Ok(Some(slots))
    }
}
