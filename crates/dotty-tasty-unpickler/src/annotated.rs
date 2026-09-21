//! `ANNOTATEDtype`: a parent type with one annotation.
//!
//! ```text
//! ANNOTATEDtype Length parent_Type annotation
//! ```
//!
//! Scala 3.9 reads the annotation in one of two forms and tells them apart by
//! the *first tag* of the payload (`TreeUnpickler.isCompactAnnotTypeTag`, kept
//! as [`is_compact_annot_type_tag`] in `dotty-tasty`):
//!
//! * **compact**: the payload is a type (`APPLIEDtype`, `SHAREDtype`,
//!   `TYPEREF`, `TYPEREFdirect`, `TYPEREFsymbol`, `TYPEREFin`). It becomes
//!   `Annotation { ty, tree: None }`, which loses nothing: a compact
//!   annotation *is* its type.
//! * **full**: the payload is an annotation tree. Its arguments and structure
//!   have no home in the semantic model yet (`AstArena<Typed>` is not built),
//!   so it is refused with `UnsupportedAnnotationTree`. Dropping the tree to
//!   `tree: None` would make it indistinguishable from a compact annotation.
//!
//! The parent is decoded first, through the ordinary type pipeline, as Dotty
//! does. Both children come from the AST index by absolute address
//! (`children[0]` the parent, `children[1]` the annotation); the structural
//! decoder validates the shape only.

use dotty_core::types::{Annotation, Type};
use dotty_tasty::tasty::{RawNode, is_compact_annot_type_tag};

use crate::ast_view::{AstView, address};
use crate::error::UnpickleError;
use crate::unpickler::TastyUnpickler;

impl TastyUnpickler<'_, '_, '_> {
    /// The `Annotated` type for the `ANNOTATEDtype` at `at`.
    pub(crate) fn decode_annotated_type(
        &mut self,
        ast: &AstView<'_>,
        node: &RawNode<'_>,
        at: u32,
        depth: usize,
    ) -> Result<Type, UnpickleError> {
        node.decode_annotated()?;
        let children: Vec<u32> = ast
            .children(at)
            .iter()
            .map(|child| address(child.offset))
            .collect();
        let [underlying_at, annotation_at] = children[..] else {
            return Err(UnpickleError::MalformedType {
                address: at,
                reason: "an annotated type has a parent and an annotation",
            });
        };

        let underlying = self.type_at(ast, underlying_at, at, depth)?;
        let Some(annotation_tag) = ast.tag_at(annotation_at) else {
            return Err(UnpickleError::MalformedType {
                address: at,
                reason: "the annotation of an annotated type is not a node",
            });
        };
        if !is_compact_annot_type_tag(annotation_tag) {
            return Err(UnpickleError::UnsupportedAnnotationTree {
                address: at,
                annotation_address: annotation_at,
                tag: annotation_tag,
            });
        }

        // The wire tag only says "a type": what it decodes to (a `SHAREDtype`
        // may reach anything) must be what `CompactAnnotation` accepts, a
        // `TypeRef` or an `AppliedType`. A binder still being decoded has no
        // readable slot yet, so it cannot be one.
        let annotation_type = self.type_at(ast, annotation_at, at, depth)?;
        let accepted = !self.is_pending(annotation_type)
            && matches!(
                self.store.types.get(annotation_type),
                Type::TypeRef { .. } | Type::Applied { .. }
            );
        if !accepted {
            return Err(UnpickleError::InvalidCompactAnnotationType {
                address: at,
                annotation_type,
            });
        }
        let annotation = self
            .store
            .annotations
            .alloc(Annotation::new(annotation_type, None));
        Ok(Type::Annotated {
            underlying,
            annotation,
        })
    }
}
