//! Type-tree projection (Milestone 5a): the semantic `tpe` of a TASTy type
//! tree, without building a typed AST.
//!
//! A definition's declared type is written as a *type tree*, not as a type
//! node, and Scala 3.9's `TreeUnpickler.readTpt` reads it as follows:
//!
//! | tree | projected type |
//! |------|----------------|
//! | `SHAREDterm target` | the projection of the target tree (`forkAt(readAddr()).readTpt()`): no type of its own |
//! | `IDENTtpt name Type` | exactly the embedded type; the name is syntax and is never resolved |
//! | `APPLIEDtpt tycon args` | `Applied { tycon, args }` of the projected parts, in wire order; with the constructor `Definitions::and_type` / `or_type` and two arguments, `And` / `Or` |
//! | `BYNAMEtpt result` | `ByName { result }` |
//! | `EXPLICITtpt tpt` | exactly the projection of its child; no wrapper |
//! | `TYPEBOUNDStpt lo` | `AliasingBounds { alias: lo }` (upstream's `lo eq hi`) |
//! | `TYPEBOUNDStpt lo hi` | `Bounds { low: lo, high: hi }` |
//! | `TYPEBOUNDStpt lo hi alias` | the alias' own type, not a `Bounds` |
//! | `SELECTtpt name qualifier` | `TypeRef` to the member of the qualifier's term `tpe` ([`type_of_term`](TastyUnpickler::type_of_term)), by the selection a name-based `TYPEREF` makes (Milestone 5b) |
//! | `SINGLETONtpt ref` | exactly the `tpe` of `ref`, which must be a stable singleton (5b) |
//! | `ANNOTATEDtpt tpt annotation` | `Annotated { tpt's type, annotation }`, the annotation decoded as `ANNOTATEDtype`'s is (5b) |
//! | `LAMBDAtpt tparams body` | `TypeLambda`, built by `type_lambda_from_symbols` from the type parameters entered in pass 1 (completed first) and the projected body: parameter references become `ParamRef`s of that binder (Milestone 5c) |
//! | `REFINEDtpt parent stats...` | `Refined`/`Recursive`, folding the completed member infos over the projected parent and closing over the synthetic refinement class's `ThisType` (Milestone 5d2b, [`crate::refinement`]) |
//! | any other tag | a semantic type wire node (`readType`), through `type_at` |
//!
//! `MATCHtpt`, a `BLOCK` used as a tree, `HOLE` and any other tree that is
//! not a type are `UnsupportedTypeTree`: they are counted, not guessed from
//! their syntax.
//!
//! ## Identity
//!
//! The projection is cached by *tree address* in its own map
//! ([`TastySemanticIndex::type_tree_type_at`]), apart from the type-node map
//! `type_at`: a tree address is not a type address, several tree addresses may
//! project to one `TypeId` (an `IDENTtpt` shares its embedded type's), and a
//! derived type (`APPLIEDtpt`, `BYNAMEtpt`, `TYPEBOUNDStpt`) is owned by the
//! projection, not by a type node. Derived types are not interned by shape:
//! two equal `APPLIEDtpt` at different addresses get different `TypeId`s.
//! Children are found by absolute address in the AST index.
//!
//! [`TastySemanticIndex::type_tree_type_at`]: crate::index::TastySemanticIndex::type_tree_type_at

use dotty_core::ids::TypeId;
use dotty_core::names::{Namespace, TypeName};
use dotty_core::types::Type;
use dotty_core::{TypeParamSpec, Variance, type_lambda_from_symbols};
use dotty_tasty::tasty::{
    ANNOTATEDTPT_TAG, APPLIEDTPT_TAG, BYNAMETPT_TAG, CONTRAVARIANT_TAG, COVARIANT_TAG,
    DefinitionTail, EXPLICITTPT_TAG, IDENTTPT_TAG, LAMBDATPT_TAG, REFINEDTPT_TAG, RawTree,
    SELECTTPT_TAG, SHAREDTERM_TAG, SINGLETONTPT_TAG, STABLE_TAG, TYPEBOUNDSTPT_TAG,
};

use crate::annotated::FullAnnotation;
use crate::ast_view::{AstView, MAX_SHARED_DEPTH, address};
use crate::error::UnpickleError;
use crate::lookup::is_singleton_type;
use crate::names::wire_name;
use crate::unpickler::TastyUnpickler;

impl TastyUnpickler<'_, '_, '_> {
    /// The semantic type of the type tree at `at`, named by the node at
    /// `from`. Not atomic by itself: the public entry points roll back.
    pub(crate) fn type_of_tpt(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        from: u32,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        if let Some(existing) = self.index.type_tree_type_at(at) {
            return Ok(existing);
        }
        let Some(tag) = ast.tag_at(at) else {
            return Err(UnpickleError::InvalidReferenceTarget { from, to: at });
        };

        if tag == SHAREDTERM_TAG {
            // An indirection: no type and no cache entry of its own.
            let target = ast.resolve_shared_term(at, from)?;
            // A link back to an ancestor is the only way a tree can contain
            // itself: bound the links followed one inside another.
            if depth >= MAX_SHARED_DEPTH {
                return Err(UnpickleError::InvalidReferenceTarget {
                    from: at,
                    to: target,
                });
            }
            return self.type_of_tpt(ast, target, at, depth + 1);
        }

        let children = children_of(ast, at);
        let ty = match tag {
            IDENTTPT_TAG => {
                // Dotty reads the name and never uses it: it is validated
                // (and dropped), the embedded type is the tree's type.
                if let RawTree::NatAst { value, .. } = ast.tree_at(at, from)? {
                    wire_name(self.file.names(), value)?;
                }
                let [embedded] = children[..] else {
                    return Err(malformed(at, "an identifier type tree has one type"));
                };
                self.type_at(ast, embedded, at, depth)?
            }
            EXPLICITTPT_TAG => {
                ast.tree_at(at, from)?.decode_explicit_tpt()?;
                let [child] = children[..] else {
                    return Err(malformed(at, "an explicit type tree has one child"));
                };
                self.type_of_tpt(ast, child, at, depth)?
            }
            BYNAMETPT_TAG => {
                ast.tree_at(at, from)?.decode_by_name_tpt()?;
                let [child] = children[..] else {
                    return Err(malformed(at, "a by-name type tree has one child"));
                };
                let result = self.type_of_tpt(ast, child, at, depth)?;
                self.store.types.alloc(Type::ByName { result })
            }
            APPLIEDTPT_TAG => {
                ast.node(at)?.decode_applied_type()?;
                let [tycon, arguments @ ..] = &children[..] else {
                    return Err(malformed(at, "an applied type tree has a constructor"));
                };
                let tycon = self.type_of_tpt(ast, *tycon, at, depth)?;
                let mut args = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    args.push(self.type_of_tpt(ast, *argument, at, depth)?);
                }
                // Upstream's `processAppliedType`: the special `&` and `|`
                // aliases of the `scala` package become intersections and
                // unions. They are told apart by symbol identity
                // ([`Definitions::and_type`]), never by their name.
                let special = self.store.types.get(tycon).reference_symbol();
                match (&args[..], special) {
                    ([left, right], Some(symbol)) if symbol == self.definitions.and_type => {
                        self.store.types.alloc(Type::And {
                            left: *left,
                            right: *right,
                        })
                    }
                    ([left, right], Some(symbol)) if symbol == self.definitions.or_type => {
                        self.store.types.alloc(Type::Or {
                            left: *left,
                            right: *right,
                        })
                    }
                    _ => self.store.types.alloc(Type::Applied { tycon, args }),
                }
            }
            TYPEBOUNDSTPT_TAG => {
                let shape = ast.node(at)?.decode_type_bounds()?;
                let expected =
                    1 + usize::from(shape.high.is_some()) + usize::from(shape.alias.is_some());
                if children.len() != expected {
                    return Err(malformed(at, "the node's children do not match its shape"));
                }
                match children[..] {
                    [low] => {
                        let alias = self.type_of_tpt(ast, low, at, depth)?;
                        self.store.types.alloc(Type::AliasingBounds { alias })
                    }
                    [low, high] => {
                        let low = self.type_of_tpt(ast, low, at, depth)?;
                        let high = self.type_of_tpt(ast, high, at, depth)?;
                        self.store.types.alloc(Type::Bounds { low, high })
                    }
                    // `alias.tpe` when there is an alias: the tree's own type
                    // is the alias, not a second bounds object.
                    [low, high, alias] => {
                        self.type_of_tpt(ast, low, at, depth)?;
                        self.type_of_tpt(ast, high, at, depth)?;
                        self.type_of_tpt(ast, alias, at, depth)?
                    }
                    _ => {
                        return Err(malformed(
                            at,
                            "a bounds type tree has one to three children",
                        ));
                    }
                }
            }
            SELECTTPT_TAG => {
                // `SELECTtpt name qualifier`: a type member of the
                // qualifier's own `tpe`, the same selection as a name-based
                // `TYPEREF` makes.
                let shape = ast.tree_at(at, from)?.decode_select()?;
                let [qualifier] = children[..] else {
                    return Err(malformed(at, "a type selection has one qualifier"));
                };
                let qualifier = self.type_of_term(ast, qualifier, at, depth)?;
                let selected =
                    self.select_member_type(at, shape.name, qualifier, Namespace::Type)?;
                self.store.types.alloc(selected)
            }
            SINGLETONTPT_TAG => {
                // `SINGLETONtpt ref`: exactly the `tpe` of the reference; no
                // wrapper, because a `TermRef`, `ThisType` or constant type
                // already is a singleton. The reference must be one.
                ast.tree_at(at, from)?.decode_singleton_tpt()?;
                let [reference] = children[..] else {
                    return Err(malformed(at, "a singleton type tree has one reference"));
                };
                let ty = self.type_of_term(ast, reference, at, depth)?;
                if !is_singleton_type(self.store, ty) {
                    return Err(UnpickleError::InvalidSingletonTypeTree { address: at, ty });
                }
                ty
            }
            ANNOTATEDTPT_TAG => {
                // `ANNOTATEDtpt tpt annotation`: `AnnotatedType(tpt.tpe,
                // Annotation(tree))`. The base is projected first, then the
                // annotation tree is decoded by the decoder `ANNOTATEDtype`
                // uses; the `AnnotationId` belongs to this tree.
                ast.node(at)?.decode_annotated()?;
                let [base, annotation] = children[..] else {
                    return Err(malformed(
                        at,
                        "an annotated type tree has a type tree and an annotation",
                    ));
                };
                let underlying = self.type_of_tpt(ast, base, at, depth)?;
                match self.decode_annotation_tree(ast, at, annotation, depth)? {
                    FullAnnotation::Constructor(annotation) => {
                        self.store.types.alloc(Type::Annotated {
                            underlying,
                            annotation,
                        })
                    }
                    FullAnnotation::Other { at: tree, tag } => {
                        return Err(UnpickleError::UnsupportedAnnotationTree {
                            address: at,
                            annotation_address: tree,
                            tag,
                        });
                    }
                }
            }
            LAMBDATPT_TAG => self.type_of_lambda_tpt(ast, at, &children, depth)?,
            REFINEDTPT_TAG => self.type_of_refined_tpt(ast, at, &children, depth)?,
            // A dedicated tree with no projection yet.
            tag if is_deferred_tree(tag) => {
                return Err(UnpickleError::UnsupportedTypeTree { address: at, tag });
            }
            // `readTpt` falls back to `readType` for every other tag.
            _ => match self.type_at(ast, at, from, depth) {
                Err(UnpickleError::UnsupportedType { tag, address }) if address == at => {
                    return Err(UnpickleError::UnsupportedTypeTree { address: at, tag });
                }
                other => other?,
            },
        };
        self.index.insert_type_tree(at, ty)?;
        Ok(ty)
    }
}

impl TastyUnpickler<'_, '_, '_> {
    /// `LAMBDAtpt tparams body`: `HKTypeLambda.fromParams(tparams, body.tpe)`.
    ///
    /// The parameters were entered in pass 1 (owned by the enclosing
    /// definition), so each is completed here first, through the ordinary
    /// type-parameter completion (its bounds tree, `toBounds`), and then the
    /// body is projected. The lambda is built by
    /// [`type_lambda_from_symbols`]: every reference to a parameter symbol in
    /// the bounds and body becomes a `ParamRef` naming the new `TypeLambda`,
    /// whose `TypeId` is the binder. The parameters' own infos keep naming the
    /// symbols.
    fn type_of_lambda_tpt(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        children: &[u32],
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        if self.index.has_lambda_owner_conflict(at) {
            return Err(UnpickleError::SharedLambdaOwnerConflict { address: at });
        }
        ast.node(at)?.decode_lambda_tpt()?;
        let Some((body, params)) = children
            .split_last()
            .filter(|(_, params)| !params.is_empty())
        else {
            return Err(malformed(
                at,
                "a lambda type tree has type parameters and a body",
            ));
        };

        let mut specs = Vec::with_capacity(params.len());
        for param in params {
            let Some(symbol) = self.index.symbol_at(*param) else {
                return Err(UnpickleError::MissingEnteredSymbol { address: *param });
            };
            let bounds = self.complete_in(ast, *param, depth)?;
            let entered = self.store.symbols.get(symbol);
            specs.push(TypeParamSpec {
                symbol,
                name: TypeName::new(entered.name.text()),
                bounds,
                declared_variance: self.declared_variance(ast, *param)?,
            });
        }
        let body = self.type_of_tpt(ast, *body, at, depth)?;
        type_lambda_from_symbols(self.store, &specs, body)
            .map_err(|error| UnpickleError::ParameterAbstraction { address: at, error })
    }

    /// The variance a `TYPEPARAM` was declared with: `COVARIANT` and
    /// `CONTRAVARIANT` say so, `STABLE` is an explicit invariant marker, and no
    /// modifier is no declaration (`None`, which is not `Some(Invariant)`).
    pub(crate) fn declared_variance(
        &self,
        ast: &AstView<'_>,
        param: u32,
    ) -> Result<Option<Variance>, UnpickleError> {
        let parameter = ast.node(param)?.decode_parameter()?;
        let mut declared = None;
        for entry in &parameter.decode_body()?.tail {
            if let DefinitionTail::Modifier(tag) = entry {
                declared = match *tag {
                    COVARIANT_TAG => Some(Variance::Covariant),
                    CONTRAVARIANT_TAG => Some(Variance::Contravariant),
                    STABLE_TAG => Some(Variance::Invariant),
                    _ => declared,
                };
            }
        }
        Ok(declared)
    }
}

/// Whether `tag` is a tree the projection knowingly does not build yet. The
/// list is documentation; anything else that is not a type is refused in the
/// same way by the fall-through.
fn is_deferred_tree(tag: u8) -> bool {
    use dotty_tasty::tasty::{BLOCK_TAG, HOLE_TAG, MATCHTPT_TAG};
    matches!(tag, MATCHTPT_TAG | BLOCK_TAG | HOLE_TAG)
}

fn children_of(ast: &AstView<'_>, at: u32) -> Vec<u32> {
    ast.children(at)
        .iter()
        .map(|child| address(child.offset))
        .collect()
}

fn malformed(address: u32, reason: &'static str) -> UnpickleError {
    UnpickleError::MalformedType { address, reason }
}
