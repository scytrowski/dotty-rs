//! Term-tree `tpe` projection (Milestone 5b): the semantic type of the term
//! trees a `SELECTtpt` qualifier or a `SINGLETONtpt` reference is made of,
//! without building a typed AST and without inferring anything.
//!
//! Scala 3.9's `TreeUnpickler.readTree` reads a term and its `tpe` is whatever
//! the wire already says or a selection makes of it. This module answers only
//! that, for the path forms the corpora contain:
//!
//! | tree | projected type |
//! |------|----------------|
//! | `SHAREDterm target` | the projection of the target tree; no type and no cache entry of its own |
//! | `IDENT name Type` | exactly the embedded type; the name is syntax and is never resolved |
//! | `SELECT name qualifier` | a `TermRef` to the member of the qualifier's type, by the same selection as a name-based `TERMREF` (unsigned names only) |
//! | `QUALTHIS (IDENTtpt Type)` | `ThisType { class }`, the class the identifier's type reference names |
//! | a tree tag (`IDENTtpt`, `SELECTtpt`, `APPLIEDtpt`, ...) | the tree's own projection, [`type_of_tpt`](TastyUnpickler::type_of_tpt) |
//! | any other tag | a semantic type wire node (`readTree` falls back to `readType`): `TERMREF*`, `THIS`, a constant, `SHAREDtype`, `RECthis`, ... through `type_at` |
//!
//! Every other term (an `INLINED`, an `APPLY`, a `BLOCK`, ...) is
//! `UnsupportedTermTree`: counted, not guessed from its syntax.
//!
//! ## Identity
//!
//! The result is cached by *term address* in its own map
//! ([`TastySemanticIndex::term_tree_type_at`]), apart from `type_at` and the
//! type-tree map: a term address is neither. Only a type the projection
//! itself builds needs an entry (`QUALTHIS`, a selection); an `IDENT` and a
//! direct type node return an id `type_at` already owns, so a second entry
//! would only repeat it. Repeating a projection therefore allocates nothing.
//! Derived types are not interned by shape.
//!
//! [`TastySemanticIndex::term_tree_type_at`]: crate::index::TastySemanticIndex::term_tree_type_at

use dotty_core::ids::TypeId;
use dotty_core::names::Namespace;
use dotty_core::types::Type;
use dotty_tasty::tasty::{
    ANNOTATEDTPT_TAG, APPLIEDTPT_TAG, BYNAMETPT_TAG, EXPLICITTPT_TAG, IDENT_TAG, IDENTTPT_TAG,
    LAMBDATPT_TAG, MATCHTPT_TAG, QUALTHIS_TAG, REFINEDTPT_TAG, RawTree, SELECT_TAG, SELECTTPT_TAG,
    SHAREDTERM_TAG, SINGLETONTPT_TAG, TYPEBOUNDSTPT_TAG,
};

use crate::ast_view::{AstView, MAX_SHARED_DEPTH, address};
use crate::error::UnpickleError;
use crate::names::wire_name;
use crate::unpickler::TastyUnpickler;

impl TastyUnpickler<'_, '_, '_> {
    /// The semantic type of the term tree at `at`, named by the node at
    /// `from`. `depth` counts the `SHAREDterm` / `SHAREDtype` links being
    /// followed, one inside another. Not atomic by itself: the public entry
    /// points roll back.
    pub(crate) fn type_of_term(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        from: u32,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        if let Some(existing) = self.index.term_tree_type_at(at) {
            return Ok(existing);
        }
        let Some(tag) = ast.tag_at(at) else {
            return Err(UnpickleError::InvalidReferenceTarget { from, to: at });
        };

        if tag == SHAREDTERM_TAG {
            let target = ast.resolve_shared_term(at, from)?;
            if depth >= MAX_SHARED_DEPTH {
                return Err(UnpickleError::InvalidReferenceTarget {
                    from: at,
                    to: target,
                });
            }
            return self.type_of_term(ast, target, at, depth + 1);
        }
        if is_type_tree_tag(tag) {
            return self.type_of_tpt(ast, at, from, depth);
        }

        let children: Vec<u32> = ast
            .children(at)
            .iter()
            .map(|child| address(child.offset))
            .collect();
        match tag {
            IDENT_TAG => {
                // Dotty reads the name and never uses it.
                if let RawTree::NatAst { value, .. } = ast.tree_at(at, from)? {
                    wire_name(self.file.names(), value)?;
                }
                let [embedded] = children[..] else {
                    return Err(malformed(at, "an identifier has one type"));
                };
                return self.type_at(ast, embedded, at, depth);
            }
            SELECT_TAG => {
                // `SELECT name qualifier`, unsigned: a term member of the
                // qualifier's `tpe`, as a name-based `TERMREF` makes it. A
                // signed name is `UnsupportedSignedReference`, never the
                // first overload.
                let shape = ast.tree_at(at, from)?.decode_select()?;
                let [qualifier] = children[..] else {
                    return Err(malformed(at, "a selection has one qualifier"));
                };
                let qualifier = self.type_of_term(ast, qualifier, at, depth)?;
                let selected =
                    self.select_member_type(at, shape.name, qualifier, Namespace::Term)?;
                let ty = self.store.types.alloc(selected);
                self.index.insert_term_tree(at, ty)?;
                return Ok(ty);
            }
            QUALTHIS_TAG => {}
            // `readTree` falls back to `readType` for every other tag.
            _ => {
                return match self.type_at(ast, at, from, depth) {
                    Err(UnpickleError::UnsupportedType { tag, address }) if address == at => {
                        Err(UnpickleError::UnsupportedTermTree { address: at, tag })
                    }
                    other => other,
                };
            }
        }

        // `QUALTHIS`: the qualifier is a type identifier whose type reference
        // names the class, as Dotty's `ThisType.raw(qual.tpe)`.
        ast.tree_at(at, from)?.decode_qual_this()?;
        let [qualifier] = children[..] else {
            return Err(malformed(at, "a qualified this has one qualifier"));
        };
        if ast.tag_at(qualifier) != Some(IDENTTPT_TAG) {
            return Err(malformed(
                at,
                "the qualifier of a qualified this is not a type identifier",
            ));
        }
        let qualifier_tree = ast.tree_at(qualifier, at)?;
        if let RawTree::NatAst { value, .. } = &qualifier_tree {
            wire_name(self.file.names(), *value)?;
        }
        let RawTree::NatAst { child, .. } = &qualifier_tree else {
            return Err(malformed(qualifier, "an identifier type tree has one type"));
        };
        let class = self.this_class(ast, child, depth)?;
        let ty = self.store.types.alloc(Type::ThisType { class });
        self.index.insert_term_tree(at, ty)?;
        Ok(ty)
    }
}

/// The tags of the trees `readTree` reads that are type trees too, which the
/// type-tree projection owns.
pub(crate) fn is_type_tree_tag(tag: u8) -> bool {
    matches!(
        tag,
        IDENTTPT_TAG
            | SELECTTPT_TAG
            | SINGLETONTPT_TAG
            | APPLIEDTPT_TAG
            | BYNAMETPT_TAG
            | EXPLICITTPT_TAG
            | TYPEBOUNDSTPT_TAG
            | ANNOTATEDTPT_TAG
            | REFINEDTPT_TAG
            | LAMBDATPT_TAG
            | MATCHTPT_TAG
    )
}

fn malformed(address: u32, reason: &'static str) -> UnpickleError {
    UnpickleError::MalformedType { address, reason }
}
