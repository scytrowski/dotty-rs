//! Class completion (Milestone 5d1): `Class`, `Trait` and `ModuleClass`
//! symbols become `Complete(Type::ClassInfo)`.
//!
//! A class is a `TYPEDEF` whose first semantic child is a `TEMPLATE`. Its
//! completion mirrors upstream's `readTemplate` order:
//!
//! 1. the template header's `TYPEPARAM`s and `PARAM`s are completed (a parent
//!    may mention them), each through the ordinary per-symbol path;
//! 2. the parents are projected, in wire order, by
//!    [`type_of_parent`](TastyUnpickler::type_of_parent);
//! 3. an explicit `SELFDEF` is projected as the class's self type;
//! 4. the declaration scope pass 1 allocated is looked up;
//! 5. one `Type::ClassInfo` is allocated and published.
//!
//! Nothing is published before every step succeeded, so a failure leaves the
//! class `Missing`. The pass-1 scope and the members entered in it predate the
//! completion and are never touched by it.
//!
//! ## `ClassInfo` is the cross-unit publication boundary
//!
//! [`TastySemanticIndex::scope_of`](crate::index::TastySemanticIndex::scope_of)
//! is the unit-local fast path to a class scope. Another unit reaches the same
//! scope only through `SymbolInfo::Complete(ClassInfo)`, whose `declarations`
//! is that exact `ScopeId` (never a copy). Completing a class does not
//! complete its members: their `SymbolInfo` stays `Missing` until each is
//! completed by its own call, as upstream's `unforcedDecls` allows.
//!
//! `ClassInfo.prefix` is `Definitions::no_prefix` for every class, top-level or
//! nested. Upstream stores `owner.thisType`; this repository normalizes the
//! prefix, as the classloader adapter does, so both adapters build one shape.
//!
//! A class type parameter is a declaration symbol in the class scope, so the
//! parents and the self type keep referring to its `SymbolId`. It is not
//! abstracted into a `ParamRef`, which is what `Method`/`Poly`/`TypeLambda`
//! binders do.

use dotty_core::ids::{SymbolId, TypeId};
use dotty_core::symbols::SymbolInfo;
use dotty_core::types::{ClassInfo, Type};
use dotty_tasty::tasty::{
    APPLY_TAG, BLOCK_TAG, DEFDEF_TAG, EXPORT_TAG, IMPORT_TAG, NEW_TAG, PACKAGE_TAG, PARAM_TAG,
    SELECTIN_TAG, SELFDEF_TAG, TEMPLATE_TAG, TYPEAPPLY_TAG, TYPEDEF_TAG, TYPEPARAM_TAG, VALDEF_TAG,
};

use crate::ast_view::{AstView, MAX_SHARED_DEPTH, address};
use crate::error::UnpickleError;
use crate::mapping::CONSTRUCTOR_NAME;
use crate::names::wire_name;
use crate::unpickler::TastyUnpickler;

/// The parts of a template's children that class completion reads.
struct TemplateParts {
    /// `TYPEPARAM`/`PARAM` children: the header parameters.
    params: Vec<u32>,
    /// The parent trees, in wire order.
    parents: Vec<u32>,
    /// The `SELFDEF` node, if the template has an explicit self definition.
    self_def: Option<u32>,
}

/// Splits the children of the template at `template` the way
/// `decode_template_structure` does: leading parameters, then parents and the
/// self definition, then the statements (the first non-parameter definition,
/// or a parameter after a parent, starts them). Statements are ignored.
fn template_parts(ast: &AstView<'_>, template: u32) -> TemplateParts {
    let mut parts = TemplateParts {
        params: Vec::new(),
        parents: Vec::new(),
        self_def: None,
    };
    let mut past_header = false;
    let mut in_stats = false;
    for child in ast.children(template) {
        let at = address(child.offset);
        if matches!(child.tag, TYPEPARAM_TAG | PARAM_TAG) {
            parts.params.push(at);
            // A parameter after a parent is written after a split clause.
            in_stats |= past_header;
            continue;
        }
        past_header = true;
        if in_stats {
            continue;
        }
        match child.tag {
            SELFDEF_TAG => parts.self_def = Some(at),
            VALDEF_TAG | DEFDEF_TAG | TYPEDEF_TAG | IMPORT_TAG | EXPORT_TAG | PACKAGE_TAG => {
                in_stats = true;
            }
            _ => parts.parents.push(at),
        }
    }
    parts
}

impl TastyUnpickler<'_, '_, '_> {
    /// Completes the class-like `class` defined by the `TYPEDEF` at `at`.
    pub(crate) fn complete_class(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        class: SymbolId,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        let template = match ast.children(at).first() {
            Some(child) if child.tag == TEMPLATE_TAG => address(child.offset),
            _ => {
                return Err(UnpickleError::MalformedDefinition {
                    address: at,
                    reason: "a class definition's first child is its template",
                });
            }
        };
        let parts = template_parts(ast, template);

        for param in &parts.params {
            self.complete_in(ast, *param, depth)?;
        }
        let mut parents = Vec::with_capacity(parts.parents.len());
        for parent in &parts.parents {
            parents.push(self.type_of_parent(ast, *parent, template, depth)?);
        }
        let self_type = match parts.self_def {
            Some(self_def) => Some(self.type_of_self_def(ast, self_def, depth)?),
            None => None,
        };
        let Some(declarations) = self.index.scope_of(class) else {
            return Err(UnpickleError::MissingClassScope {
                address: at,
                symbol: class,
            });
        };

        let info = self.store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: self.definitions.no_prefix,
            class,
            parents,
            declarations,
            self_type,
        }));
        self.set_symbol_info(class, SymbolInfo::Complete(info));
        Ok(info)
    }

    /// The semantic type of the template parent at `at`, in the template at
    /// `from`: upstream's `readParentType`, which reads only as much of a
    /// constructor call as its type needs.
    ///
    /// | tree | parent type |
    /// |------|-------------|
    /// | `SHAREDterm` | that of the tree it names |
    /// | `APPLY fun args...` | that of `fun`; the term arguments are never read |
    /// | `BLOCK expr stats...` | that of `expr`; the statements are never read |
    /// | `TYPEAPPLY fun targs...` | that of `fun` when it is already applied (the compiler writes `New(Parent[A])` and repeats the arguments), else `Applied { fun, targs }` |
    /// | `SELECTin <init> (NEW tpt) owner` | the projection of `tpt` |
    /// | any other tree | its type-tree projection (`readTpt`) |
    ///
    /// A constructor argument therefore never influences `ClassInfo.parents`.
    pub(crate) fn type_of_parent(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        from: u32,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        self.type_of_parent_in(ast, at, from, depth, 0)
    }

    /// `type_of_parent`, with `wrapped` counting the wrappers entered so far.
    fn type_of_parent_in(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        from: u32,
        depth: usize,
        wrapped: usize,
    ) -> Result<TypeId, UnpickleError> {
        if wrapped > MAX_SHARED_DEPTH {
            return Err(UnpickleError::MalformedParentTree {
                address: at,
                reason: "a parent constructor call nests too deeply",
            });
        }
        // Dotty reads `SHAREDterm` as the tree at its target.
        let at = ast.resolve_shared_term(at, from)?;
        let Some(tag) = ast.tag_at(at) else {
            return Err(UnpickleError::InvalidReferenceTarget { from, to: at });
        };
        if !matches!(tag, APPLY_TAG | BLOCK_TAG | TYPEAPPLY_TAG | SELECTIN_TAG) {
            return self.type_of_tpt(ast, at, from, depth);
        }
        if let Some(existing) = self.index.type_tree_type_at(at) {
            return Ok(existing);
        }
        let children: Vec<u32> = ast
            .children(at)
            .iter()
            .map(|child| address(child.offset))
            .collect();
        let Some(first) = children.first().copied() else {
            return Err(UnpickleError::MalformedParentTree {
                address: at,
                reason: "a parent constructor call has a function",
            });
        };
        let ty = match tag {
            // The arguments and the statements are children after the first.
            APPLY_TAG | BLOCK_TAG => self.type_of_parent_in(ast, first, at, depth, wrapped + 1)?,
            TYPEAPPLY_TAG => {
                let function = self.type_of_parent_in(ast, first, at, depth, wrapped + 1)?;
                if matches!(self.store.types.get(function), Type::Applied { .. }) {
                    function
                } else {
                    let mut args = Vec::with_capacity(children.len() - 1);
                    for argument in &children[1..] {
                        args.push(self.type_of_tpt(ast, *argument, at, depth)?);
                    }
                    self.store.types.alloc(Type::Applied {
                        tycon: function,
                        args,
                    })
                }
            }
            _ => {
                let selection = ast.node(at)?.decode_select_in()?;
                let name = wire_name(self.file.names(), selection.name)?;
                if name != CONSTRUCTOR_NAME {
                    return Err(UnpickleError::MalformedParentTree {
                        address: at,
                        reason: "a parent constructor selection selects <init>",
                    });
                }
                if ast.tag_at(first) != Some(NEW_TAG) {
                    return Err(UnpickleError::MalformedParentTree {
                        address: at,
                        reason: "a parent constructor selection is on NEW",
                    });
                }
                let [new_tpt] = ast.children(first) else {
                    return Err(UnpickleError::MalformedParentTree {
                        address: first,
                        reason: "NEW has one type tree",
                    });
                };
                self.type_of_tpt(ast, address(new_tpt.offset), first, depth)?
            }
        };
        self.index.insert_type_tree(at, ty)?;
        Ok(ty)
    }

    /// The self type of the `SELFDEF` at `at`: its type tree, projected.
    fn type_of_self_def(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        ast.tree_at(at, at)?.decode_self_def()?;
        let [tree] = ast.children(at) else {
            return Err(UnpickleError::InvalidSelfTypeTree {
                address: at,
                reason: "a self definition has one type tree",
            });
        };
        self.type_of_tpt(ast, address(tree.offset), at, depth)
    }
}
