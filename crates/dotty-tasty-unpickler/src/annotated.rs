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
//! * **full**: the payload is an annotation tree. A directly written
//!   constructor application (`APPLY` or `NEW`, the shape of nearly every
//!   annotation in the corpora) is read into an `Annotation` with its type and
//!   its ordered term arguments and no typed tree. Any other root stays
//!   `UnsupportedAnnotationTree`; nothing is ever stored as if it had no
//!   arguments.
//!
//! ## Shared annotation trees
//!
//! A full annotation may be a `SHAREDterm` naming a tree written earlier.
//! Scala 3.9 reads it as `forkAt(readAddr()).readTree()`: it follows the
//! address and reads the tree again, with no cache keyed by tree address. So
//! does this decoder: the chain of `SHAREDterm` links is followed to its end
//! (`AstView::resolve_shared_term`, bounded, every target a visible node) and
//! a constructor application there is decoded by the same code as a directly
//! written one. The `AnnotationId` belongs to the enclosing `ANNOTATEDtype`:
//! two of them sharing one tree get two annotations with equal payloads, and
//! the term tree's address never enters the type index (it is not a type
//! address). Errors name the enclosing `ANNOTATEDtype` as `address` and the
//! tree the link ended at as the annotation address; a link that cannot be
//! followed is `InvalidReferenceTarget { from: the ANNOTATEDtype, to }`.
//! Typed-tree identity is Milestone 7's business, not this layer's.
//!
//! ## Full annotation constructor applications
//!
//! The tree is a constructor call, and Dotty's `allTermArguments` reads its
//! arguments as the arguments of every `APPLY` layer, innermost first:
//!
//! ```text
//! APPLY (TYPEAPPLY (SELECTin <init> (NEW tpt)) targs) args      one layer, type arguments
//! APPLY (SELECTin <init> (NEW tpt)) args                        one layer
//! NEW tpt                                                       no application at all
//! ```
//!
//! The annotation type is `tpt`, applied to `targs` when there are any
//! (`Applied`, never reduced to the class). `tpt` is a type, or an `IDENTtpt`
//! whose type child is used; any other type tree is refused
//! (`UnsupportedAnnotationConstructor`). Each argument is a literal or class
//! literal, optionally under a `NAMEDARG`; those are `AnnotationValue::Constant`
//! and are the only values the corpora contain. No wrapper (`TYPED`, `BLOCK`,
//! `INLINED`, ...) is stripped and nothing is evaluated: every other argument
//! is `UnsupportedAnnotationArgument`. `Annotation.tree` stays `None`, which
//! means no typed tree is attached; the arguments say whether there were any.
//! Children are found by absolute address in the AST index, never from the
//! structural decoder's node-relative trees.
//!
//! The parent is decoded first, through the ordinary type pipeline, as Dotty
//! does. Both children come from the AST index by absolute address
//! (`children[0]` the parent, `children[1]` the annotation); the structural
//! decoder validates the shape only.

use dotty_core::ids::{AnnotationId, TypeId};
use dotty_core::names::TermName;
use dotty_core::types::{Annotation, AnnotationArgument, AnnotationValue, Type};
use dotty_tasty::tasty::{
    APPLY_TAG, AstError, CLASSCONST_TAG, IDENTTPT_TAG, NAMEDARG_TAG, NEW_TAG, RawNode, RawTree,
    SELECTIN_TAG, SHAREDTERM_TAG, TYPEAPPLY_TAG, is_compact_annot_type_tag,
};

use crate::ast_view::{AstView, address};
use crate::error::UnpickleError;
use crate::names::wire_name;
use crate::unpickler::TastyUnpickler;

/// The tag and absolute address of a tree's root node.
fn tree_head(tree: &RawTree<'_>) -> (u8, u32) {
    match tree {
        RawTree::Leaf(term) => (term.tag, address(term.offset)),
        RawTree::Ast { tag, offset, .. } | RawTree::NatAst { tag, offset, .. } => {
            (*tag, address(*offset))
        }
        RawTree::LengthNode(node) => (node.tag, address(node.offset)),
    }
}

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
        // Dotty reads `SHAREDterm` as `forkAt(readAddr()).readTree()`: the
        // annotation tree is whatever the chain of links ends at. The link
        // is followed, nothing is cached by its target, and the annotation
        // below belongs to this `ANNOTATEDtype` alone.
        let (tree_at, tree_tag) = if annotation_tag == SHAREDTERM_TAG {
            let target = ast.resolve_shared_term(annotation_at, at)?;
            let Some(tag) = ast.tag_at(target) else {
                return Err(UnpickleError::InvalidReferenceTarget {
                    from: at,
                    to: target,
                });
            };
            (target, tag)
        } else {
            (annotation_at, annotation_tag)
        };
        if tree_tag == APPLY_TAG || tree_tag == NEW_TAG {
            let annotation = self.decode_constructor_annotation(ast, at, tree_at, depth)?;
            return Ok(Type::Annotated {
                underlying,
                annotation,
            });
        }
        if tree_at != annotation_at {
            // A shared target that is no constructor call: the tree's own
            // address and tag are the ones reported.
            return Err(UnpickleError::UnsupportedAnnotationTree {
                address: at,
                annotation_address: tree_at,
                tag: tree_tag,
            });
        }
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
            .alloc(Annotation::compact(annotation_type));
        Ok(Type::Annotated {
            underlying,
            annotation,
        })
    }
}

/// What a constructor spine (`APPLY`/`TYPEAPPLY`/`SELECTin` down to `NEW`)
/// holds, as absolute addresses: the trees the structural decoder returns are
/// relative to their node, so only the AST index names children.
struct Spine {
    /// The class tree under `NEW`.
    class: u32,
    /// Type arguments of the constructor call, if written.
    type_arguments: Option<Vec<u32>>,
    /// The term arguments of every `APPLY` layer, innermost layer first.
    arguments: Vec<u32>,
}

impl TastyUnpickler<'_, '_, '_> {
    /// The annotation for the constructor application rooted at `annotation_at`
    /// inside the `ANNOTATEDtype` at `at`.
    fn decode_constructor_annotation(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        annotation_at: u32,
        depth: usize,
    ) -> Result<AnnotationId, UnpickleError> {
        let spine = self.constructor_spine(ast, at, annotation_at)?;

        let class = self.annotation_class_type(ast, spine.class, at, depth)?;
        // A link may name a binder still being decoded, whose slot is not
        // readable: it cannot be an annotation class.
        if self.is_pending(class) {
            return Err(UnpickleError::InvalidAnnotationType {
                address: at,
                annotation_type: class,
            });
        }
        let annotation_type = match spine.type_arguments {
            None => class,
            Some(type_arguments) => {
                if matches!(self.store.types.get(class), Type::Applied { .. }) {
                    return Err(UnpickleError::UnsupportedAnnotationConstructor {
                        address: at,
                        annotation_address: annotation_at,
                        tag: TYPEAPPLY_TAG,
                    });
                }
                let mut args = Vec::with_capacity(type_arguments.len());
                for argument in type_arguments {
                    args.push(self.annotation_class_type(ast, argument, at, depth)?);
                }
                self.store.types.alloc(Type::Applied { tycon: class, args })
            }
        };
        if !matches!(
            self.store.types.get(annotation_type),
            Type::TypeRef { .. } | Type::Applied { .. }
        ) {
            return Err(UnpickleError::InvalidAnnotationType {
                address: at,
                annotation_type,
            });
        }

        let mut arguments = Vec::with_capacity(spine.arguments.len());
        for argument in spine.arguments {
            arguments.push(self.annotation_argument(ast, argument, at, depth)?);
        }
        Ok(self
            .store
            .annotations
            .alloc(Annotation::with_arguments(annotation_type, arguments)))
    }

    /// Walks a constructor call down to its `NEW`, collecting what it applies.
    fn constructor_spine(
        &self,
        ast: &AstView<'_>,
        at: u32,
        annotation_at: u32,
    ) -> Result<Spine, UnpickleError> {
        let unsupported = |tag: u8| UnpickleError::UnsupportedAnnotationConstructor {
            address: at,
            annotation_address: annotation_at,
            tag,
        };
        let malformed = |reason: &'static str| UnpickleError::MalformedType {
            address: annotation_at,
            reason,
        };
        let mut layers: Vec<Vec<u32>> = Vec::new();
        let mut type_arguments = None;
        let mut current = annotation_at;
        loop {
            let tree = ast.tree_at(current, at)?;
            let (tag, _) = tree_head(&tree);
            let children = child_addresses(ast, current);
            match (&tree, tag) {
                (RawTree::LengthNode(node), APPLY_TAG) => {
                    let apply = node.decode_apply()?;
                    let [function, arguments @ ..] = &children[..] else {
                        return Err(malformed("an application has a function"));
                    };
                    if arguments.len() != apply.arguments.len() {
                        return Err(malformed(
                            "an application's arguments do not match its shape",
                        ));
                    }
                    layers.push(arguments.to_vec());
                    current = *function;
                }
                (RawTree::LengthNode(node), TYPEAPPLY_TAG) => {
                    let apply = node.decode_type_apply()?;
                    let [function, arguments @ ..] = &children[..] else {
                        return Err(malformed("a type application has a function"));
                    };
                    if arguments.len() != apply.type_arguments.len() {
                        return Err(malformed(
                            "a type application's arguments do not match its shape",
                        ));
                    }
                    // Type arguments belong to the constructor call, once.
                    if type_arguments.is_some() {
                        return Err(unsupported(tag));
                    }
                    type_arguments = Some(arguments.to_vec());
                    current = *function;
                }
                (RawTree::LengthNode(node), SELECTIN_TAG) => {
                    let select = node.decode_select_in()?;
                    // Only the constructor selection of a `new`.
                    if wire_name(self.file.names(), select.name)? != "<init>" {
                        return Err(unsupported(tag));
                    }
                    let [qualifier, _owner] = children[..] else {
                        return Err(malformed("a selection has a qualifier and an owner"));
                    };
                    current = qualifier;
                }
                (RawTree::Ast { .. }, NEW_TAG) => {
                    tree.decode_new()?;
                    let [class] = children[..] else {
                        return Err(malformed("a `new` has one class"));
                    };
                    // `allTermArguments`: the arguments of `fn` come before the
                    // arguments of the `Apply` around it.
                    return Ok(Spine {
                        class,
                        type_arguments,
                        arguments: layers.into_iter().rev().flatten().collect(),
                    });
                }
                _ => return Err(unsupported(tag)),
            }
        }
    }

    /// The type a type tree at `tree_at` stands for: the type itself, or the
    /// `Type` child of an `IDENTtpt`. Any other type tree is not an annotation
    /// constructor.
    fn annotation_class_type(
        &mut self,
        ast: &AstView<'_>,
        tree_at: u32,
        at: u32,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        let type_at = if ast.tag_at(tree_at) == Some(IDENTTPT_TAG) {
            let [type_at] = child_addresses(ast, tree_at)[..] else {
                return Err(UnpickleError::MalformedType {
                    address: tree_at,
                    reason: "an identifier type tree has one type",
                });
            };
            type_at
        } else {
            tree_at
        };
        match self.type_at(ast, type_at, at, depth) {
            Err(UnpickleError::UnsupportedType { tag, address }) if address == type_at => {
                Err(UnpickleError::UnsupportedAnnotationConstructor {
                    address: at,
                    annotation_address: tree_at,
                    tag,
                })
            }
            other => other,
        }
    }

    /// One term argument at `argument_at`: a literal or class literal,
    /// possibly named.
    fn annotation_argument(
        &mut self,
        ast: &AstView<'_>,
        argument_at: u32,
        at: u32,
        depth: usize,
    ) -> Result<AnnotationArgument, UnpickleError> {
        let tree = ast.tree_at(argument_at, at)?;
        let (name, value_at) = match &tree {
            RawTree::NatAst {
                tag: NAMEDARG_TAG, ..
            } => {
                let named = tree.decode_named_arg()?;
                let text = wire_name(self.file.names(), named.name)?;
                let name = TermName::new(self.store.names.intern(&text));
                let [value_at] = child_addresses(ast, argument_at)[..] else {
                    return Err(UnpickleError::MalformedType {
                        address: argument_at,
                        reason: "a named argument has one value",
                    });
                };
                (Some(name), value_at)
            }
            _ => (None, argument_at),
        };
        let value = ast.tree_at(value_at, at)?;
        let (tag, _) = tree_head(&value);
        let is_constant = match &value {
            RawTree::Leaf(term) => term.constant_value().map_err(AstError::from)?.is_some(),
            RawTree::Ast { .. } => tag == CLASSCONST_TAG,
            _ => false,
        };
        if !is_constant {
            return Err(UnpickleError::UnsupportedAnnotationArgument {
                address: at,
                argument_address: value_at,
                tag,
            });
        }
        let id = self.type_at(ast, value_at, at, depth)?;
        let Type::Constant(constant) = self.store.types.get(id) else {
            return Err(UnpickleError::MalformedType {
                address: value_at,
                reason: "an annotation argument literal is not a constant",
            });
        };
        Ok(AnnotationArgument {
            name,
            value: AnnotationValue::Constant(constant.clone()),
        })
    }
}

fn child_addresses(ast: &AstView<'_>, at: u32) -> Vec<u32> {
    ast.children(at)
        .iter()
        .map(|child| address(child.offset))
        .collect()
}
