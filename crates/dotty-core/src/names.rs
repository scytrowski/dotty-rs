//! Interned, namespace-aware names shared by the semantic model.
//!
//! Scala keeps term and type identifiers in separate namespaces (`class Foo`
//! and `val Foo` can coexist), so a bare `String` is never enough to tell two
//! names apart. Every [`Name`] pairs interned text with the [`Namespace`] it
//! lives in.

use std::collections::HashMap;

use crate::ids::{NameId, checked_index};

/// The two namespaces Scala names live in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Namespace {
    Term,
    Type,
}

/// A namespaced, interned name.
///
/// Two `Name` values compare equal only if both their text and namespace
/// match — `TermName("Foo")` and `TypeName("Foo")` are distinct names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Name {
    text: NameId,
    namespace: Namespace,
}

impl Name {
    pub const fn new(text: NameId, namespace: Namespace) -> Self {
        Self { text, namespace }
    }

    pub const fn text(self) -> NameId {
        self.text
    }

    pub const fn namespace(self) -> Namespace {
        self.namespace
    }

    pub const fn is_term(self) -> bool {
        matches!(self.namespace, Namespace::Term)
    }

    pub const fn is_type(self) -> bool {
        matches!(self.namespace, Namespace::Type)
    }
}

/// A [`Name`] known to live in the term namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TermName(Name);

impl TermName {
    pub const fn new(text: NameId) -> Self {
        Self(Name::new(text, Namespace::Term))
    }

    pub const fn as_name(&self) -> &Name {
        &self.0
    }
}

/// A [`Name`] known to live in the type namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TypeName(Name);

impl TypeName {
    pub const fn new(text: NameId) -> Self {
        Self(Name::new(text, Namespace::Type))
    }

    pub const fn as_name(&self) -> &Name {
        &self.0
    }
}

/// Deduplicates name text and hands out stable [`NameId`] handles for it.
///
/// Interned bytes are kept in both `strings` (for `resolve`) and as the
/// `HashMap` key (for `intern`). This duplicates storage for every distinct
/// name; it is an accepted simplicity tradeoff for the foundation and should
/// not be "optimized" without a profiling reason (see `docs/dotty-core-design.md`).
#[derive(Debug, Default)]
pub struct NameInterner {
    strings: Vec<Box<str>>,
    lookup: HashMap<Box<str>, NameId>,
}

impl NameInterner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Interns `text`, returning the same [`NameId`] for equal strings.
    pub fn intern(&mut self, text: &str) -> NameId {
        if let Some(&id) = self.lookup.get(text) {
            return id;
        }

        let id = NameId::new(checked_index(self.strings.len()));
        let boxed: Box<str> = Box::from(text);
        self.strings.push(boxed.clone());
        self.lookup.insert(boxed, id);
        id
    }

    /// Resolves a previously interned [`NameId`] back to its text.
    ///
    /// Panics if `id` was not produced by this interner — an `id` from a
    /// different `NameInterner` is a compiler-internal bug, not recoverable
    /// input (see `docs/dotty-core-design.md`, "Error handling policy").
    pub fn resolve(&self, id: NameId) -> &str {
        &self.strings[id.index() as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_the_same_text_twice_returns_the_same_id() {
        let mut interner = NameInterner::new();

        let first = interner.intern("foo");
        let second = interner.intern("foo");

        assert_eq!(first, second);
    }

    #[test]
    fn interning_different_text_returns_different_ids() {
        let mut interner = NameInterner::new();

        let foo = interner.intern("foo");
        let bar = interner.intern("bar");

        assert_ne!(foo, bar);
    }

    #[test]
    fn resolve_returns_the_original_text() {
        let mut interner = NameInterner::new();
        let id = interner.intern("scala");

        assert_eq!(interner.resolve(id), "scala");
    }

    #[test]
    #[should_panic]
    fn resolving_an_id_from_a_different_interner_panics() {
        let mut first = NameInterner::new();
        let id = first.intern("foo");

        let second = NameInterner::new();
        second.resolve(id);
    }

    #[test]
    fn term_and_type_names_with_equal_text_are_not_equal() {
        let mut interner = NameInterner::new();
        let text = interner.intern("Foo");

        let term = TermName::new(text);
        let ty = TypeName::new(text);

        assert_ne!(term.as_name(), ty.as_name());
        assert!(term.as_name().is_term());
        assert!(ty.as_name().is_type());
    }

    #[test]
    fn name_exposes_its_interned_text_and_namespace() {
        let mut interner = NameInterner::new();
        let text = interner.intern("bar");
        let name = Name::new(text, Namespace::Term);

        assert_eq!(name.text(), text);
        assert_eq!(name.namespace(), Namespace::Term);
    }
}
