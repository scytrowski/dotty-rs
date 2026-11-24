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
    pub(super) classes: Vec<(TreeId<Untyped>, TextRange)>,
    pub(super) parents: Vec<(TreeId<Untyped>, TextRange)>,
    pub(super) constructions: Vec<(TreeId<Untyped>, TextRange)>,
    parameters: Vec<(TreeId<Untyped>, TextRange)>,
    values: Vec<(TreeId<Untyped>, TextRange)>,
    identifiers: Vec<SourceTermName>,
    selections: Vec<SourceSelection>,
}

impl SourceMemberNames {
    /// Written name of an ordinary class declaration in this parse, including backticks.
    pub fn class_name(&self, tree: TreeId<Untyped>) -> Option<TextRange> {
        declaration_name(&self.classes, tree)
    }

    /// Written type name of a simple unqualified parent, bare or with one argument
    /// list. Supply the parent tree from the template in this parse.
    pub fn parent_name(&self, tree: TreeId<Untyped>) -> Option<TextRange> {
        declaration_name(&self.parents, tree)
    }

    /// Whole application trees and exact class-name tokens for simple written `new`
    /// expressions in token order. Includes a bare zero-argument construction.
    /// Qualified, generic and anonymous class constructions are excluded.
    pub fn constructions(&self) -> &[(TreeId<Untyped>, TextRange)] {
        &self.constructions
    }

    /// Exact name token for a simple value definition or explicit constructor
    /// `val`/`var` parameter. Supply a definition from this parse.
    pub fn value_name(&self, tree: TreeId<Untyped>) -> Option<TextRange> {
        declaration_name(&self.values, tree)
    }

    /// Exact written token of a named method parameter or a parameter without
    /// an explicit accessor in an ordinary class constructor, including backticks.
    /// Use [`Self::value_name`] for explicit constructor `val` and `var` fields.
    pub fn parameter_name(&self, tree: TreeId<Untyped>) -> Option<TextRange> {
        declaration_name(&self.parameters, tree)
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

    /// All bare identifiers and explicit selectors, interleaved in source token
    /// order. Each token is visited once without allocating a combined list.
    pub fn term_names(&self) -> impl Iterator<Item = SourceTermName> + '_ {
        let mut bare = self.identifiers.iter().peekable();
        let mut selected = self.selections.iter().peekable();
        std::iter::from_fn(move || match (bare.peek(), selected.peek()) {
            (Some(b), Some(s)) if s.name.start() < b.name.start() => selected.next().copied(),
            (Some(_), _) => bare.next().copied(),
            _ => selected.next().copied(),
        })
    }

    /// Bare expression identifier whose half-open name contains this UTF-8 byte offset.
    pub fn identifier_at(&self, offset: u32) -> Option<SourceTermName> {
        name_at(&self.identifiers, offset)
    }

    pub(crate) fn parameter(&mut self, tree: TreeId<Untyped>, name: TextRange) {
        self.parameters.push((tree, name));
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
        self.constructions
            .sort_unstable_by_key(|(_, name)| name.start());
        self.identifiers.sort_unstable_by_key(|s| s.name.start());
        self.selections.sort_unstable_by_key(|s| s.name.start());
    }
}

fn declaration_name(
    entries: &[(TreeId<Untyped>, TextRange)],
    tree: TreeId<Untyped>,
) -> Option<TextRange> {
    let i = entries
        .binary_search_by_key(&tree.index(), |(id, _)| id.index())
        .ok()?;
    Some(entries[i].1)
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
    fn written_constructions_bind_the_name_to_the_whole_application_in_token_order() {
        let text = "class C { def f() = { new Empty; new Empty(); new /* comment */ `Base`(new Inner()); new pkg.Other(); new Anon() {} } }";
        let parsed = crate::parse_compilation_unit(
            SourceText::new(text).unwrap(),
            SourceId::from_index(0),
            dotty_lexer::ContextualScanner::new(text).unwrap(),
            &mut NameInterner::new(),
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let slice = |r: TextRange| &text[r.start() as usize..r.end() as usize];
        let sites: Vec<_> = parsed
            .member_names
            .constructions()
            .iter()
            .map(|&(tree, name)| {
                let node = parsed.ast.get(tree);
                assert!(matches!(node.kind, TreeKind::Apply(_)));
                (slice(name), slice(node.position.unwrap().span().range()))
            })
            .collect();
        assert_eq!(
            sites,
            [
                ("Empty", "new Empty"),
                ("Empty", "new Empty()"),
                ("`Base`", "new /* comment */ `Base`(new Inner())"),
                ("Inner", "new Inner()")
            ]
        );
    }

    #[test]
    fn written_class_and_parent_names_exclude_arguments_and_qualified_types() {
        let text = "class `Base`; class A extends /*comment*/ `Base`; class B(start: Int) extends `Base`(start); class C extends pkg.Base";
        let parsed = crate::parse_compilation_unit(
            SourceText::new(text).unwrap(),
            SourceId::from_index(0),
            dotty_lexer::ContextualScanner::new(text).unwrap(),
            &mut NameInterner::new(),
        );
        assert!(parsed.diagnostics.is_empty());
        let slice = |r: TextRange| &text[r.start() as usize..r.end() as usize];
        let mut seen = Vec::new();
        for (tree, node) in parsed.ast.iter() {
            if let Some(name) = parsed.member_names.class_name(tree) {
                let TreeKind::TypeDef(class) = &node.kind else {
                    panic!("class")
                };
                let TreeKind::Template(body) = &parsed.ast.get(class.rhs).kind else {
                    panic!("template")
                };
                let parent = body
                    .parents
                    .first()
                    .and_then(|&p| parsed.member_names.parent_name(p));
                if let Some(p) = parent {
                    assert!(!p.contains(p.end()));
                    assert!(p.start() > name.end());
                }
                seen.push((slice(name), parent.map(slice)));
            }
        }
        assert_eq!(
            seen,
            [
                ("`Base`", None),
                ("A", Some("`Base`")),
                ("B", Some("`Base`")),
                ("C", None)
            ]
        );
    }

    #[test]
    fn plain_constructor_names_preserve_order_backticks_and_field_boundaries() {
        let text = "class Pair(z: Int, `a`: Int, val first: Int, var next: Int)";
        let parsed = crate::parse_compilation_unit(
            SourceText::new(text).unwrap(),
            SourceId::from_index(0),
            dotty_lexer::ContextualScanner::new(text).unwrap(),
            &mut NameInterner::new(),
        );
        assert!(parsed.diagnostics.is_empty());
        let names = &parsed.member_names;
        let slice = |r: TextRange| &text[r.start() as usize..r.end() as usize];
        let parameters: Vec<_> = parsed
            .ast
            .iter()
            .filter_map(|(tree, _)| {
                let name = names.parameter_name(tree)?;
                assert!(names.value_name(tree).is_none());
                assert!(!name.contains(name.end()));
                Some((name.start() as usize, slice(name)))
            })
            .collect();
        assert_eq!(
            parameters,
            [
                (text.find("z:").unwrap(), "z"),
                (text.find("`a`").unwrap(), "`a`")
            ]
        );
        let fields: Vec<_> = parsed
            .ast
            .iter()
            .filter_map(|(tree, _)| names.value_name(tree))
            .map(slice)
            .collect();
        assert_eq!(fields, ["first", "next"]);
    }

    #[test]
    fn parameter_tokens_distinguish_formals_from_fields_and_uses() {
        let text =
            "class A(val field: Int) { def diff(left: Int, `right`: Int): Int = left - `right` }";
        let parsed = crate::parse_compilation_unit(
            SourceText::new(text).unwrap(),
            SourceId::from_index(0),
            dotty_lexer::ContextualScanner::new(text).unwrap(),
            &mut NameInterner::new(),
        );
        assert!(parsed.diagnostics.is_empty());
        let tokens: Vec<_> = parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| {
                let name = parsed.member_names.parameter_name(tree)?;
                assert!(matches!(node.kind, TreeKind::ValDef(_)));
                assert!(parsed.member_names.value_name(tree).is_none());
                assert!(!name.contains(name.end()));
                Some((
                    name.start() as usize,
                    &text[name.start() as usize..name.end() as usize],
                ))
            })
            .collect();
        assert_eq!(
            tokens,
            [
                (text.find("left").unwrap(), "left"),
                (text.find("`right`").unwrap(), "`right`")
            ]
        );
    }

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
