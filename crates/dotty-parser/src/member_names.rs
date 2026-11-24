//! Written names of value declarations, bare term identifiers and explicit selections.
use dotty_core::{TextRange, TreeId, Untyped};

/// A written expression name such as `count` or the selector in `c.count`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceTermName {
    /// Half-open UTF-8 name range including any backticks.
    pub name: TextRange,
    /// Identifier or selection in the original parse arena.
    pub tree: TreeId<Untyped>,
    /// Enclosing assignment when this expression itself is the left-hand side.
    pub assignment: Option<TreeId<Untyped>>,
}

/// Written selector token returned by the selection-only queries.
pub type SourceSelection = SourceTermName;

/// Name tokens paired with trees from one source parse. Semantic ownership
/// and visibility are determined later by the namer and typer.
#[derive(Debug, Default)]
pub struct SourceMemberNames {
    values: Vec<(TreeId<Untyped>, TextRange)>,
    identifiers: Vec<SourceTermName>,
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
        name_at(&self.selections, offset)
    }

    /// Bare term identifiers in token order, including receivers and callees.
    pub fn identifiers(&self) -> &[SourceTermName] {
        &self.identifiers
    }

    /// Bare expression identifier whose half-open name contains this UTF-8 byte offset.
    pub fn identifier_at(&self, offset: u32) -> Option<SourceTermName> {
        name_at(&self.identifiers, offset)
    }

    pub(crate) fn value(&mut self, tree: TreeId<Untyped>, name: TextRange) {
        self.values.push((tree, name));
    }
    pub(crate) fn term(&mut self, tree: TreeId<Untyped>, name: TextRange, selected: bool) {
        let entries = if selected {
            &mut self.selections
        } else {
            &mut self.identifiers
        };
        entries.push(SourceTermName {
            name,
            tree,
            assignment: None,
        });
    }
    pub(crate) fn assign(
        &mut self,
        lhs: TreeId<Untyped>,
        assignment: TreeId<Untyped>,
        selected: bool,
    ) {
        // Before finish, each list follows tree allocation order.
        let entries = if selected {
            &mut self.selections
        } else {
            &mut self.identifiers
        };
        if let Ok(i) = entries.binary_search_by_key(&lhs.index(), |s| s.tree.index()) {
            entries[i].assignment = Some(assignment);
        }
    }
    pub(crate) fn finish(&mut self) {
        self.identifiers.sort_unstable_by_key(|s| s.name.start());
        self.selections.sort_unstable_by_key(|s| s.name.start());
    }
}

fn name_at(entries: &[SourceTermName], offset: u32) -> Option<SourceTermName> {
    let i = entries.partition_point(|s| s.name.end() <= offset);
    entries.get(i).copied().filter(|s| s.name.contains(offset))
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

    #[test]
    fn bare_names_preserve_exact_tokens_and_only_the_assignment_lhs() {
        let text = "class A { def add(delta: Int): Unit = `count` = count + delta }";
        let parsed = crate::parse_compilation_unit(
            SourceText::new(text).unwrap(),
            SourceId::from_index(1),
            dotty_lexer::ContextualScanner::new(text).unwrap(),
            &mut NameInterner::new(),
        );
        assert!(parsed.diagnostics.is_empty());
        let names = parsed.member_names;
        let [lhs, read, delta] = names.identifiers() else {
            panic!("expected three identifiers")
        };
        assert!(names.selections().is_empty());
        for (name, expected) in [(lhs, "`count`"), (read, "count"), (delta, "delta")] {
            assert_eq!(
                &text[name.name.start() as usize..name.name.end() as usize],
                expected
            );
            assert!(matches!(parsed.ast.get(name.tree).kind, TreeKind::Ident(_)));
            assert_eq!(names.identifier_at(name.name.end() - 1), Some(*name));
            assert_eq!(names.identifier_at(name.name.end()), None);
            assert_eq!(names.selection_at(name.name.start()), None);
        }
        assert!(
            matches!(&parsed.ast.get(lhs.assignment.unwrap()).kind, TreeKind::Assign(a) if a.lhs == lhs.tree)
        );
        assert_eq!((read.assignment, delta.assignment), (None, None));
    }
}
