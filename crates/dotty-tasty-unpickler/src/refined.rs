//! `REFINEDtype`: a parent type refined by one named member.
//!
//! ```text
//! REFINEDtype Length name_NameRef parent_Type refinement_Type
//! ```
//!
//! becomes `Type::Refined { parent, name, info }`. A structural type with
//! several members is nested `Refined` values, one member per level, in the
//! order TASTy writes them: nothing is flattened or normalised.
//!
//! The name is a term name, unless the refinement's info is `TYPEBOUNDS`, in
//! which case it is a type name. That is Dotty's rule (`if nextUnsharedTag ==
//! TYPEBOUNDS then name = name.toTypeName`), applied to the tag of the info
//! *after* following `SHAREDtype` links, and never inferred from the text of
//! the name. The refinement has no symbol, so a member of a refined type
//! cannot be looked up here: a name-based reference through one keeps the
//! existing `UnsupportedResolutionPrefix`.
//!
//! Both children come from the AST index by absolute address (`children[0]` is
//! the parent, `children[1]` the info); the structural decoder's trees are
//! relative to the payload and only validate the shape and give the name.

use dotty_core::names::{Name, Namespace};
use dotty_core::types::Type;
use dotty_tasty::tasty::{RawNode, TYPEBOUNDS_TAG};

use crate::ast_view::{AstView, address};
use crate::error::UnpickleError;
use crate::names::{is_signed, wire_name};
use crate::unpickler::TastyUnpickler;

impl TastyUnpickler<'_, '_, '_> {
    /// The `Refined` type for the `REFINEDtype` at `at`.
    pub(crate) fn decode_refined_type(
        &mut self,
        ast: &AstView<'_>,
        node: &RawNode<'_>,
        at: u32,
        depth: usize,
    ) -> Result<Type, UnpickleError> {
        let shape = node.decode_refined_type()?;
        let children: Vec<u32> = ast
            .children(at)
            .iter()
            .map(|child| address(child.offset))
            .collect();
        let [parent_at, info_at] = children[..] else {
            return Err(UnpickleError::MalformedType {
                address: at,
                reason: "a refined type has a parent and a refinement",
            });
        };

        let text = wire_name(self.file.names(), shape.name)?;
        if is_signed(self.file.names(), shape.name) {
            // A refinement is a member *name*; a signature would be dropped.
            return Err(UnpickleError::UnsupportedSignedReference {
                address: at,
                name: text,
            });
        }
        let namespace = if ast.next_unshared_tag(info_at, at)? == TYPEBOUNDS_TAG {
            Namespace::Type
        } else {
            Namespace::Term
        };
        let name = Name::new(self.store.names.intern(&text), namespace);

        let parent = self.type_at(ast, parent_at, at, depth)?;
        let info = self.type_at(ast, info_at, at, depth)?;
        Ok(Type::Refined { parent, name, info })
    }
}
