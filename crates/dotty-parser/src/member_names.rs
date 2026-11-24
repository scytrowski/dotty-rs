//! Written names of value declarations and explicit term selections.
use dotty_core::{TextRange, TreeId, Untyped};

/// An explicitly written selector such as the name in `c.count`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceSelection {
    /// Half-open UTF-8 name range including any backticks.
    pub name: TextRange,
    /// Selection in the original parse arena.
    pub tree: TreeId<Untyped>,
    /// Enclosing assignment when this selection itself is the left-hand side.
    pub assignment: Option<TreeId<Untyped>>,
}

/// Name tokens paired with trees from one source parse. Semantic ownership
/// and visibility are determined later by the namer and typer.
#[derive(Debug, Default)]
pub struct SourceMemberNames {
    values: Vec<(TreeId<Untyped>, TextRange)>,
    selections: Vec<SourceSelection>,
}

impl SourceMemberNames {
    /// Exact name token for a simple value definition or explicit constructor
    /// `val`/`var` parameter. Supply a definition from this parse.
    pub fn value_name(&self, tree: TreeId<Untyped>) -> Option<TextRange> {
        let i = self
            .values
            .binary_search_by_key(&tree.index(), |(id, _)| id.index())
            .ok()?;
        Some(self.values[i].1)
    }

    /// Written term selections in token order, including selections used as callees.
    pub fn selections(&self) -> &[SourceSelection] {
        &self.selections
    }

    /// Selection whose half-open name token contains this UTF-8 byte offset.
    pub fn selection_at(&self, offset: u32) -> Option<SourceSelection> {
        let i = self.selections.partition_point(|s| s.name.end() <= offset);
        self.selections
            .get(i)
            .copied()
            .filter(|s| s.name.contains(offset))
    }

    pub(crate) fn value(&mut self, tree: TreeId<Untyped>, name: TextRange) {
        self.values.push((tree, name));
    }
    pub(crate) fn select(&mut self, tree: TreeId<Untyped>, name: TextRange) {
        self.selections.push(SourceSelection {
            name,
            tree,
            assignment: None,
        });
    }
    pub(crate) fn assign(&mut self, lhs: TreeId<Untyped>, assignment: TreeId<Untyped>) {
        // During parsing entries follow tree allocation order, even in nested assignments.
        if let Ok(i) = self
            .selections
            .binary_search_by_key(&lhs.index(), |s| s.tree.index())
        {
            self.selections[i].assignment = Some(assignment);
        }
    }
    pub(crate) fn finish(&mut self) {
        self.selections.sort_unstable_by_key(|s| s.name.start());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::{NameInterner, SourceId, SourceText, TreeKind};
    #[test]
    fn written_selection_and_value_tokens_survive_comments_backticks_and_assignment() {
        let text =
            "class A(var `count`: Int) { val n = 1; def copy(b: A): Unit = b./*x*/`count` = b.n }";
        let parsed = crate::parse_compilation_unit(
            SourceText::new(text).unwrap(),
            SourceId::from_index(1),
            dotty_lexer::ContextualScanner::new(text).unwrap(),
            &mut NameInterner::new(),
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let names = parsed.member_names;
        let slice = |r: TextRange| &text[r.start() as usize..r.end() as usize];
        let values: Vec<_> = parsed
            .ast
            .iter()
            .filter_map(|(id, _)| names.value_name(id))
            .map(slice)
            .collect();
        assert_eq!(values, ["`count`", "n"]);
        let [write, read] = names.selections() else {
            panic!("expected two selections")
        };
        assert_eq!((slice(write.name), slice(read.name)), ("`count`", "n"));
        assert!(
            matches!(&parsed.ast.get(write.assignment.unwrap()).kind, TreeKind::Assign(a) if a.lhs == write.tree && a.rhs == read.tree)
        );
        assert!(read.assignment.is_none());
        assert_eq!(names.selection_at(write.name.end() - 1), Some(*write));
        assert_eq!(names.selection_at(write.name.end()), None);
    }
}
