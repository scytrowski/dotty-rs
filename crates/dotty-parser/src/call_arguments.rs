//! Written argument lists for cursor queries in one source revision.

use dotty_core::{TextRange, TreeId, Untyped};

/// A written argument label and its operand in one source revision.
#[derive(Debug)]
pub struct NamedCallArgument {
    /// Zero-based position in the source argument list.
    pub written: usize,
    /// Semantic spelling (without backquotes), independent of the parse interner.
    pub name: Box<str>,
    /// Exact UTF-8 range of the label token, including any backquotes.
    pub label: TextRange,
    /// Operand range, absent for an unfinished assignment at the recovery boundary.
    pub value: Option<TextRange>,
}

/// Written context and class-name token of a simple constructor application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstructorSyntax {
    /// A single unqualified class after `new`.
    Allocation(TextRange),
    /// A single unqualified parent after `extends`.
    DirectParent(TextRange),
}

/// A parenthesized application, including nested and indirect applications.
#[derive(Debug)]
pub struct CallArgumentList {
    /// Exact application tree in the original parse arena. For an explicit
    /// constructor this is the application paired with its written class name
    /// by [`crate::SourceMemberNames::constructions`].
    pub application: TreeId<Untyped>,
    /// Simple allocation or direct parent clause, retained even if its list is unfinished.
    pub constructor: Option<ConstructorSyntax>,
    /// Written callee token for a direct named call. Other applications have no name here.
    pub callee: Option<TextRange>,
    /// UTF-8 byte range from the opening through the closing parenthesis, or to
    /// the recovery boundary of an unfinished list.
    pub range: TextRange,
    /// Complete argument ranges in written order, excluding delimiters and surrounding trivia.
    pub arguments: Box<[TextRange]>,
    /// Argument labels in source order, including a trailing label awaiting a value.
    pub named_arguments: Box<[NamedCallArgument]>,
    /// True for an ordinary argument clause, including one containing labels.
    pub regular: bool,
    /// True for an ordinary clause containing only positional arguments.
    pub positional: bool,
    /// Original application in the parse arena when its closing parenthesis is
    /// missing. Only the first `arguments.len()` argument trees are written.
    pub unfinished: Option<TreeId<Untyped>>,
    pub(crate) includes_end: bool,
    pub(crate) comma_ends: Box<[u32]>,
    pub(crate) parent: Option<usize>,
}

impl CallArgumentList {
    /// Positional slot at an insertion point, including the next unwritten slot
    /// after an opening parenthesis or comma. Only valid inside this list.
    pub fn slot_at(&self, offset: u32) -> Option<usize> {
        self.contains_cursor(offset)
            .then(|| self.comma_ends.partition_point(|end| *end <= offset))
    }

    fn contains_cursor(&self, offset: u32) -> bool {
        offset > self.range.start()
            && (offset < self.range.end() || self.includes_end && offset == self.range.end())
    }

    /// Index of the written argument at a UTF-8 insertion position. Trivia before
    /// a comma belongs to the previous argument; immediately after a comma the
    /// next argument becomes active. Empty lists, trailing commas without a next
    /// argument, and positions outside the parentheses yield `None`.
    pub fn argument_index(&self, offset: u32) -> Option<usize> {
        let slot = self.slot_at(offset)?;
        (slot < self.arguments.len()).then_some(slot)
    }
}

/// Index of parenthesized applications in a single parse. Ranges and unfinished
/// tree IDs can be retained after releasing the arena; dereference a tree ID
/// only in its original parse.
#[derive(Debug)]
pub struct SourceCallArguments(Vec<CallArgumentList>);

impl SourceCallArguments {
    pub(crate) fn new(mut lists: Vec<CallArgumentList>) -> Self {
        lists.sort_unstable_by_key(|list| list.range.start());
        let mut parent: Option<usize> = None;
        for i in 0..lists.len() {
            while let Some(p) = parent {
                if lists[p].range.contains(lists[i].range.start()) {
                    break;
                }
                parent = lists[p].parent;
            }
            lists[i].parent = parent;
            parent = Some(i);
        }
        Self(lists)
    }

    /// Finds the innermost list containing a cursor between the parentheses,
    /// including immediately before the closing token. The positions before
    /// the opening and after the closing token are outside. Supply a UTF-8
    /// byte offset from the original revision. Unclosed lists have no entry.
    pub fn at(&self, offset: u32) -> Option<&CallArgumentList> {
        self.innermost(offset)
            .filter(|list| list.unfinished.is_none())
    }

    /// Finds the innermost unfinished list at this cursor. EOF and the insertion
    /// position immediately before a closing brace are included. The next
    /// declaration and positions inside a completed nested list are excluded.
    pub fn unfinished_at(&self, offset: u32) -> Option<&CallArgumentList> {
        self.innermost(offset)
            .filter(|list| list.unfinished.is_some())
    }

    /// Visits unfinished lists with an intact argument prefix in opening-token
    /// order. Inspect `positional` to exclude named or contextual arguments.
    pub fn unfinished_lists(&self) -> impl Iterator<Item = &CallArgumentList> {
        self.0.iter().filter(|list| list.unfinished.is_some())
    }

    fn innermost(&self, offset: u32) -> Option<&CallArgumentList> {
        let next = self.0.partition_point(|list| list.range.start() < offset);
        let mut list = self.0.get(next.checked_sub(1)?)?;
        while !list.contains_cursor(offset) {
            list = &self.0[list.parent?];
        }
        Some(list)
    }
}

#[cfg(test)]
mod tests {
    use dotty_core::{NameInterner, SourceId, SourceText};
    use dotty_lexer::ContextualScanner;

    fn parse(text: &str) -> crate::ParseResult {
        crate::parse_expression_fragment(
            SourceText::new(text).unwrap(),
            SourceId::from_index(1),
            ContextualScanner::new(text).unwrap(),
            &mut NameInterner::new(),
        )
    }

    #[test]
    fn nested_lists_use_written_delimiters_and_argument_ranges() {
        let text = "outer(inner(1 /* , */, 2), /* 😀 , */ 3)";
        let parsed = parse(text);
        assert!(parsed.diagnostics.is_empty());
        let pos = |token: &str| text.find(token).unwrap() as u32;
        let at = |offset| parsed.call_arguments.at(offset).unwrap();
        let slice = |r: dotty_core::TextRange| &text[r.start() as usize..r.end() as usize];
        let outer = at(pos("inner"));
        let inner = at(pos("1"));
        assert_eq!(slice(outer.range), &text[5..]);
        assert_eq!(slice(inner.callee.unwrap()), "inner");
        assert_eq!(
            inner
                .arguments
                .iter()
                .copied()
                .map(slice)
                .collect::<Vec<_>>(),
            ["1", "2"]
        );
        assert_eq!(
            outer
                .arguments
                .iter()
                .copied()
                .map(slice)
                .collect::<Vec<_>>(),
            ["inner(1 /* , */, 2)", "3"]
        );
        for (offset, callee, slot) in [
            (pos(","), "inner", 0),
            (pos(", 2"), "inner", 0),
            (pos(", 2") + 1, "inner", 1),
            (pos("),"), "inner", 1),
            (pos("),") + 1, "outer", 0),
            (pos("),") + 2, "outer", 1),
            (pos("😀"), "outer", 1),
            (pos("3)") + 1, "outer", 1),
        ] {
            let list = at(offset);
            assert_eq!(
                (slice(list.callee.unwrap()), list.argument_index(offset)),
                (callee, Some(slot))
            );
        }
        assert!(parsed.call_arguments.at(5).is_none());
        assert!(parsed.call_arguments.at(text.len() as u32).is_none());
    }

    #[test]
    fn explicit_allocations_own_their_written_labels_and_operands() {
        let text = "new Outer(child = new Pair(`right` = right, left = 9))";
        let parsed = parse(text);
        assert!(parsed.diagnostics.is_empty());
        let slice = |r: dotty_core::TextRange| &text[r.start() as usize..r.end() as usize];
        for ((tree, class), (name, arguments)) in parsed.member_names.constructions().iter().zip([
            (
                "Outer",
                vec![("child", "new Pair(`right` = right, left = 9)")],
            ),
            ("Pair", vec![("`right`", "right"), ("left", "9")]),
        ]) {
            assert_eq!(slice(*class), name);
            let list = parsed.call_arguments.at(class.end() + 1).unwrap();
            assert_eq!(list.application, *tree);
            assert_eq!(
                list.named_arguments
                    .iter()
                    .map(|arg| (slice(arg.label), slice(arg.value.unwrap())))
                    .collect::<Vec<_>>(),
                arguments
            );
        }
        assert_eq!(parsed.member_names.constructions().len(), 2);
    }

    #[test]
    fn unfinished_constructor_lists_retain_their_written_context_and_tree() {
        use super::ConstructorSyntax::*;
        for (text, class, parent) in [
            ("class Use { def f = new Base(", "Base", false),
            ("class Child extends Base(count = ", "Base", true),
            ("class Use { def f = new Outer(new Inner(", "Inner", false),
        ] {
            let parsed = crate::parse_compilation_unit(
                SourceText::new(text).unwrap(),
                SourceId::from_index(1),
                ContextualScanner::new(text).unwrap(),
                &mut NameInterner::new(),
            );
            assert!(!parsed.diagnostics.is_empty());
            let end = text.len() as u32;
            let list = parsed.call_arguments.unfinished_at(end).unwrap();
            let name = match (parent, list.constructor) {
                (true, Some(DirectParent(name))) | (false, Some(Allocation(name))) => name,
                other => panic!("{other:?}"),
            };
            assert_eq!(&text[name.start() as usize..name.end() as usize], class);
            assert_eq!(list.range.start(), name.end());
            assert_eq!(list.slot_at(end), Some(0));
            assert_eq!(list.unfinished, Some(list.application));
            assert_eq!(
                parsed.member_names.parent_name(list.application),
                Some(name)
            );
            assert_eq!(
                parsed
                    .member_names
                    .constructions()
                    .contains(&(list.application, name)),
                !parent
            );
        }
    }

    #[test]
    fn empty_trailing_named_and_unclosed_lists_remain_distinct() {
        for (text, positional, count, slot) in [
            ("f()", true, 0, None),
            ("f(1,)", true, 1, None),
            ("f(n = 1)", false, 1, Some(0)),
            ("f(using x)", false, 1, Some(0)),
        ] {
            let parsed = parse(text);
            assert!(
                parsed.diagnostics.is_empty(),
                "{text}: {:?}",
                parsed.diagnostics
            );
            let offset = text.len() as u32 - 1;
            let list = parsed.call_arguments.at(offset).unwrap();
            assert_eq!(
                (
                    list.positional,
                    list.arguments.len(),
                    list.argument_index(offset)
                ),
                (positional, count, slot)
            );
        }
        assert!(parse("f(1").call_arguments.at(2).is_none());
    }

    #[test]
    fn labels_retain_their_ranges_and_missing_value_in_nested_calls() {
        let text = "diff(right = inner(`x` = 4), left = ";
        let parsed = parse(text);
        assert!(!parsed.diagnostics.is_empty());
        let slice = |r: dotty_core::TextRange| &text[r.start() as usize..r.end() as usize];
        let outer = parsed
            .call_arguments
            .unfinished_at(text.len() as u32)
            .unwrap();
        let [right, left] = &*outer.named_arguments else {
            panic!()
        };
        assert!(outer.regular && !outer.positional);
        assert_eq!(outer.arguments.len(), 1);
        assert_eq!(
            (right.written, slice(right.label), right.value.map(slice)),
            (0, "right", Some("inner(`x` = 4)"))
        );
        assert_eq!(
            (left.written, slice(left.label), left.value),
            (1, "left", None)
        );
        let inner = parsed
            .call_arguments
            .at(text.find('4').unwrap() as u32)
            .unwrap();
        let x = &inner.named_arguments[0];
        assert_eq!(
            (&*x.name, slice(x.label), x.value.map(slice)),
            ("x", "`x`", Some("4"))
        );
    }

    #[test]
    fn eof_and_commas_preserve_every_written_argument_and_the_next_slot() {
        for (text, slot, count) in [
            ("o.pick(", 0, 0),
            ("o.pick(1, ", 1, 1),
            ("o.pick(1, true, ", 2, 2),
            ("o.pick(1, true, 3, ", 3, 3),
            ("o.pick(1, true, 3, 4", 3, 4),
        ] {
            let parsed = parse(text);
            let end = text.len() as u32;
            let list = parsed.call_arguments.unfinished_at(end).unwrap();
            assert_eq!(
                (list.slot_at(end), list.arguments.len(), list.unfinished),
                (Some(slot), count, Some(parsed.root))
            );
            assert!(parsed.call_arguments.at(end).is_none());
            assert!(parsed.call_arguments.unfinished_at(end + 1).is_none());
        }
    }

    #[test]
    fn recovery_respects_nesting_and_following_declarations() {
        let parsed = parse("a.pick(b.pick(");
        let inner = parsed.call_arguments.unfinished_at(14).unwrap();
        assert_eq!(inner.callee.unwrap().start(), 9);
        let text = "class Use { def run(o: Ops): Int = o.pick(1,  \n def next(): Int = 0 }";
        let parsed = crate::parse_compilation_unit(
            dotty_core::SourceText::new(text).unwrap(),
            dotty_core::SourceId::from_index(0),
            dotty_lexer::ContextualScanner::new(text).unwrap(),
            &mut dotty_core::NameInterner::new(),
        );
        let cursor = text.find("1,").unwrap() as u32 + 2;
        assert_eq!(
            parsed
                .call_arguments
                .unfinished_at(cursor)
                .unwrap()
                .slot_at(cursor),
            Some(1)
        );
        assert!(
            parsed
                .call_arguments
                .unfinished_at(text.find("def next").unwrap() as u32)
                .is_none()
        );
        let next = text.find("next()").unwrap() as u32;
        assert!(parsed.ast.iter().any(|(tree, _)| {
            parsed
                .call_names
                .method_name(tree)
                .is_some_and(|name| name.start() == next)
        }));
    }
}
