//! Rebinding a [`TypeLambda`] to a fresh binder.
//!
//! A binder's identity is the [`TypeId`] it is stored under, and every
//! `ParamRef` inside it names that id. Changing something about a binder, such
//! as the variances its parameters were declared with, is therefore not a
//! clone with one field changed: the clone's nested `ParamRef`s would still
//! name the *old* id. Dotty does the same thing this module does:
//! `HKTypeLambda.withVariances` calls `newLikeThis`, which builds a new lambda
//! whose parameter infos and result are the old ones with `subst(this, x)`
//! applied, `x` being the new binder.
//!
//! [`rebind_type_lambda`] is that operation, on `TypeId` graphs and knowing
//! nothing about any wire format. It:
//!
//! * reserves the new binder's id *before* transforming the children, so a
//!   reference to the binder inside them resolves to the new id;
//! * rewrites every reachable `ParamRef` of the old binder to the same
//!   parameter of the new one;
//! * transforms the graph with a memo table, so a type reachable at two
//!   places becomes one type, and a type that contains no reference to a
//!   rebound binder keeps its own id (nothing is interned structurally);
//! * copies every `Method`, `Poly`, `TypeLambda` and `Recursive` it reaches,
//!   reserving the copy's id first and remapping its own `ParamRef`s and
//!   `RecThis`es, so a copied inner binder is internally consistent (it is
//!   conservative: a nested binder is copied whenever it is reached, even if
//!   nothing in it changes);
//! * transforms an `Annotated` type's annotation into a new annotation rather
//!   than mutating the stored one, and the type fields of a `ClassInfo`
//!   (keeping its class symbol and declaration scope: symbols are not part of
//!   a type graph);
//! * leaves the source graph unchanged and is atomic: on error every
//!   allocation is rolled back.
//!
//! The same traversal also abstracts parameter *symbols*
//! ([`method_type_from_symbols`], [`poly_type_from_symbols`],
//! [`type_lambda_from_symbols`]): a `TypeRef`/`TermRef` to an entered parameter
//! symbol becomes the `ParamRef` naming the new binder and the parameter's
//! position. The symbol is source identity, the `ParamRef` is identity inside
//! the binder; both are needed and neither replaces the other globally.
//!
//! The `match` over [`Type`] has no wildcard arm, so adding a variant forces
//! this module to be reviewed.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::ids::{AnnotationId, SymbolId, TypeId};
use crate::names::{TermName, TypeName};
use crate::store::SemanticStore;
use crate::types::annotation::{
    Annotation, AnnotationArgument, AnnotationArguments, AnnotationValue,
};
use crate::types::class_info::ClassInfo;
use crate::types::constant::Constant;
use crate::types::method::MethodKind;
use crate::types::method::{MethodParam, MethodType, PolyType, TypeLambda, TypeParam, Variance};
use crate::types::ty::{MatchType, TermRefTarget, Type, TypeRefTarget};

/// Types nest a handful of levels at most; a graph deeper than this is
/// treated as malformed rather than risking the stack.
const MAX_DEPTH: usize = 512;

/// Why a rebinding request was refused. Every one leaves the store exactly as
/// it was before the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeRebindError {
    /// `source` is not a [`Type::TypeLambda`].
    NotATypeLambda { source: TypeId },
    /// `declared_variances` has `actual` entries, but the lambda has
    /// `expected` parameters. Nothing is truncated or padded.
    VarianceArityMismatch {
        source: TypeId,
        expected: usize,
        actual: usize,
    },
    /// The graph reaches a slot that was reserved and not filled yet: a
    /// binder still under construction, referenced by id rather than through
    /// a `ParamRef`.
    UnfilledType { id: TypeId },
    /// The graph reaches `id` again through itself without a binder between,
    /// which no well-formed type does.
    CyclicType { id: TypeId },
    /// The graph nests deeper than the rebinder allows.
    TooDeep { id: TypeId },
    /// A parameter symbol was given twice to an abstraction: a reference to it
    /// could only name one of the two positions.
    DuplicateParameterSymbol { symbol: SymbolId },
}

impl fmt::Display for TypeRebindError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotATypeLambda { source } => {
                write!(formatter, "type {} is not a type lambda", source.index())
            }
            Self::VarianceArityMismatch {
                expected, actual, ..
            } => write!(
                formatter,
                "{actual} declared variances were given for a lambda with {expected} parameters"
            ),
            Self::UnfilledType { id } => write!(
                formatter,
                "type {} is reserved but not filled, so it cannot be rebound",
                id.index()
            ),
            Self::CyclicType { id } => {
                write!(formatter, "type {} contains itself", id.index())
            }
            Self::TooDeep { id } => write!(
                formatter,
                "type {} is nested too deeply to rebind",
                id.index()
            ),
            Self::DuplicateParameterSymbol { symbol } => write!(
                formatter,
                "parameter symbol {} was given for two positions",
                symbol.index()
            ),
        }
    }
}

impl std::error::Error for TypeRebindError {}

/// Builds a fresh `TypeLambda` from the lambda `source`, with
/// `declared_variances[i]` as the declared variance of parameter `i`, and
/// every reference to `source` inside it pointing at the new lambda.
///
/// The returned id is a new binder, different from `source`. `source` and
/// everything reachable from it are unchanged. Types that do not depend on
/// `source` are shared with the old graph rather than copied.
///
/// Returns a [`TypeRebindError`] if `source` is not a `TypeLambda`, if the
/// variance count differs from the parameter count, or if the graph is
/// malformed (see the variants). On any error the store is rolled back to its
/// state at the call, so no partial allocation survives. Interned names are
/// never needed, so none are added.
pub fn rebind_type_lambda(
    store: &mut SemanticStore,
    source: TypeId,
    declared_variances: &[Variance],
) -> Result<TypeId, TypeRebindError> {
    if !store.types.is_filled(source) {
        return Err(TypeRebindError::UnfilledType { id: source });
    }
    let Type::TypeLambda(lambda) = store.types.get(source) else {
        return Err(TypeRebindError::NotATypeLambda { source });
    };
    if lambda.params.len() != declared_variances.len() {
        return Err(TypeRebindError::VarianceArityMismatch {
            source,
            expected: lambda.params.len(),
            actual: declared_variances.len(),
        });
    }
    let lambda = lambda.clone();

    let checkpoint = store.checkpoint();
    let result = Rebinder::new(store).lambda(source, &lambda, Some(declared_variances));
    if result.is_err() {
        store.rollback_to(checkpoint);
    }
    result
}

/// Closes `parent` over `class`: every reachable `ThisType { class }` becomes
/// the one canonical `RecThis` of a fresh recursive binder (Dotty's
/// `RecType.closeOver`, used to turn a refinement's own `ThisType` into a
/// self-reference of the closed refined type).
///
/// `parent` and everything reachable from it are read but never mutated;
/// types that do not depend on `class` are shared with the old graph rather
/// than copied. Two passes decide the result:
///
/// * if nothing reachable from `parent` is `ThisType { class }`, `parent`
///   itself is returned unchanged and nothing is allocated (Dotty's
///   `isReferredToBy` check, `RecType.parent` in that case);
/// * otherwise a `Recursive` binder is allocated first, `parent` is copied
///   with every matching `ThisType` replaced by that binder's one `RecThis`
///   (further occurrences share the same `RecThis` id), and the new
///   `Recursive`'s id is returned.
///
/// Nested `Method`, `Poly`, `TypeLambda` and `Recursive` binders reachable
/// from `parent` are copied (their own bound references stay internally
/// consistent) and may themselves contain a matching `ThisType`; annotations
/// and class-literal annotation arguments are transformed the same way
/// [`rebind_type_lambda`] transforms them.
///
/// Returns a [`TypeRebindError`] if `parent` is not filled or the graph is
/// malformed (cyclic or nested too deeply); the store is rolled back to its
/// state at the call on any error.
pub fn close_over_this(
    store: &mut SemanticStore,
    parent: TypeId,
    class: SymbolId,
) -> Result<TypeId, TypeRebindError> {
    if !store.types.is_filled(parent) {
        return Err(TypeRebindError::UnfilledType { id: parent });
    }
    if !references_this_type(store, parent, class)? {
        return Ok(parent);
    }

    let checkpoint = store.checkpoint();
    let reserved = store.types.reserve();
    let binder = reserved.id();
    let mut rebinder = Rebinder::new(store);
    rebinder.close_over = Some((class, binder));
    let result = rebinder.ty(parent).map(|transformed| {
        rebinder.store.types.fill(
            reserved,
            Type::Recursive {
                parent: transformed,
            },
        );
        binder
    });
    if result.is_err() {
        store.rollback_to(checkpoint);
    }
    result
}

/// Whether `ThisType { class }` is reachable from `root`, without allocating
/// anything: an iterative, visited-once walk of exactly the shapes
/// [`Rebinder::build`] copies (a `RecThis` carries no `ThisType` of its own;
/// its binder is reached, if at all, through the `Recursive` that owns it).
/// Bounded by the number of distinct types reachable, so a cyclic graph
/// (a `Recursive` reachable from its own parent) terminates instead of
/// looping; that is not itself an error here, only in the transforming pass.
fn references_this_type(
    store: &SemanticStore,
    root: TypeId,
    class: SymbolId,
) -> Result<bool, TypeRebindError> {
    let mut stack = vec![root];
    let mut visited = HashSet::new();
    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        if !store.types.is_filled(id) {
            return Err(TypeRebindError::UnfilledType { id });
        }
        match store.types.get(id) {
            Type::ThisType { class: found } => {
                if *found == class {
                    return Ok(true);
                }
            }
            Type::NoType | Type::Error(_) | Type::NoPrefix => {}
            Type::Constant(Constant::Class(class)) => stack.push(*class),
            Type::Constant(_) => {}
            Type::TermRef { prefix, .. } | Type::TypeRef { prefix, .. } => stack.push(*prefix),
            Type::SuperType {
                this_type,
                super_type,
            } => stack.extend([*this_type, *super_type]),
            Type::Applied { tycon, args } => {
                stack.push(*tycon);
                stack.extend(args.iter().copied());
            }
            Type::Bounds { low, high } => stack.extend([*low, *high]),
            Type::AliasingBounds { alias } => stack.push(*alias),
            Type::ByName { result } => stack.push(*result),
            Type::Flexible { underlying } => stack.push(*underlying),
            Type::And { left, right } | Type::Or { left, right } => stack.extend([*left, *right]),
            Type::Refined { parent, info, .. } => stack.extend([*parent, *info]),
            Type::Recursive { parent } => stack.push(*parent),
            Type::RecThis { .. } => {}
            Type::Method(method) => {
                stack.extend(method.params.iter().map(|param| param.ty));
                stack.push(method.result);
            }
            Type::Poly(poly) => {
                stack.extend(poly.params.iter().map(|param| param.bounds));
                stack.push(poly.result);
            }
            Type::TypeLambda(lambda) => {
                stack.extend(lambda.params.iter().map(|param| param.bounds));
                stack.push(lambda.result);
            }
            Type::ParamRef { .. } => {}
            Type::Match(MatchType {
                bound,
                scrutinee,
                cases,
            }) => {
                stack.extend([*bound, *scrutinee]);
                stack.extend(cases.iter().copied());
            }
            Type::MatchCase { pattern, result } => stack.extend([*pattern, *result]),
            Type::Annotated {
                underlying,
                annotation,
            } => {
                stack.push(*underlying);
                let annotation = store.annotations.get(*annotation);
                stack.push(annotation.ty);
                if let AnnotationArguments::Known(arguments) = &annotation.arguments {
                    for argument in arguments {
                        if let AnnotationValue::Constant(Constant::Class(class)) = argument.value {
                            stack.push(class);
                        }
                    }
                }
            }
            Type::Wildcard { bounds } => stack.push(*bounds),
            Type::JavaArray { element } => stack.push(*element),
            Type::ClassInfo(info) => {
                stack.push(info.prefix);
                stack.extend(info.parents.iter().copied());
                if let Some(self_type) = info.self_type {
                    stack.push(self_type);
                }
            }
        }
    }
    Ok(false)
}

/// A term parameter of the method being built by
/// [`method_type_from_symbols`]: the symbol it was entered as, and what its
/// `MethodParam` will say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MethodParamSpec {
    pub symbol: SymbolId,
    pub name: TermName,
    pub ty: TypeId,
    pub erased: bool,
    pub varargs: bool,
}

/// A type parameter of the polymorphic or lambda type being built by
/// [`poly_type_from_symbols`] / [`type_lambda_from_symbols`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeParamSpec {
    pub symbol: SymbolId,
    pub name: TypeName,
    pub bounds: TypeId,
    pub declared_variance: Option<Variance>,
}

/// Which binder [`abstract_symbols`] builds.
enum Abstraction<'a> {
    Method(&'a [MethodParamSpec], MethodKind),
    Poly(&'a [TypeParamSpec]),
    Lambda(&'a [TypeParamSpec]),
}

/// Builds `(params): result` with every reference to a parameter *symbol*, in
/// the parameter types and in `result`, replaced by the `ParamRef` naming the
/// new method and the parameter's position (Dotty's `MethodType.fromSymbols`).
///
/// A parameter symbol is source identity; a `ParamRef { binder, index }` is
/// identity inside the binder. Before a binder exists a reference to the
/// parameter can only name the symbol, and once it exists the reference must
/// name the binder: this is the operation between the two. Only the exact
/// `SymbolId` is replaced, never a name, and only its `TypeRef` / `TermRef`
/// (whatever its prefix, as `subst` does). An already built nested binder that
/// mentions a parameter is copied with the replacement, as `rebind_type_lambda`
/// copies one; nothing stored is mutated, and a type with no such reference
/// keeps its id. The result is a new binder.
///
/// Atomic: on error the store is as it was.
pub fn method_type_from_symbols(
    store: &mut SemanticStore,
    params: &[MethodParamSpec],
    result: TypeId,
    kind: MethodKind,
) -> Result<TypeId, TypeRebindError> {
    abstract_symbols(store, Abstraction::Method(params, kind), result)
}

/// `[params]: result` from type parameter symbols; see
/// [`method_type_from_symbols`] (Dotty's `PolyType.fromParams`). References in
/// the parameters' own bounds (F-bounds) name the new binder too.
pub fn poly_type_from_symbols(
    store: &mut SemanticStore,
    params: &[TypeParamSpec],
    result: TypeId,
) -> Result<TypeId, TypeRebindError> {
    abstract_symbols(store, Abstraction::Poly(params), result)
}

/// `[params] =>> result` from type parameter symbols; see
/// [`method_type_from_symbols`] (Dotty's `HKTypeLambda.fromParams`).
pub fn type_lambda_from_symbols(
    store: &mut SemanticStore,
    params: &[TypeParamSpec],
    result: TypeId,
) -> Result<TypeId, TypeRebindError> {
    abstract_symbols(store, Abstraction::Lambda(params), result)
}

fn abstract_symbols(
    store: &mut SemanticStore,
    what: Abstraction<'_>,
    result: TypeId,
) -> Result<TypeId, TypeRebindError> {
    let symbols: Vec<SymbolId> = match &what {
        Abstraction::Method(params, _) => params.iter().map(|param| param.symbol).collect(),
        Abstraction::Poly(params) | Abstraction::Lambda(params) => {
            params.iter().map(|param| param.symbol).collect()
        }
    };
    let mut seen = HashSet::new();
    for symbol in &symbols {
        if !seen.insert(*symbol) {
            return Err(TypeRebindError::DuplicateParameterSymbol { symbol: *symbol });
        }
    }

    let checkpoint = store.checkpoint();
    let built = build_abstraction(store, &what, &symbols, result);
    if built.is_err() {
        store.rollback_to(checkpoint);
    }
    built
}

fn build_abstraction(
    store: &mut SemanticStore,
    what: &Abstraction<'_>,
    symbols: &[SymbolId],
    result: TypeId,
) -> Result<TypeId, TypeRebindError> {
    let mut rebinder = Rebinder::new(store);
    let reserved = rebinder.store.types.reserve();
    let binder = reserved.id();
    for (index, symbol) in symbols.iter().enumerate() {
        let index = u32::try_from(index).unwrap_or(u32::MAX);
        rebinder.substitution.insert(*symbol, (binder, index));
    }
    let built = match what {
        Abstraction::Method(params, kind) => {
            let mut out = Vec::with_capacity(params.len());
            for param in *params {
                out.push(MethodParam {
                    name: param.name,
                    ty: rebinder.ty(param.ty)?,
                    erased: param.erased,
                    varargs: param.varargs,
                });
            }
            Type::Method(MethodType {
                params: out,
                result: rebinder.ty(result)?,
                kind: *kind,
            })
        }
        Abstraction::Poly(params) | Abstraction::Lambda(params) => {
            let mut out = Vec::with_capacity(params.len());
            for param in *params {
                out.push(TypeParam {
                    name: param.name,
                    bounds: rebinder.ty(param.bounds)?,
                    declared_variance: param.declared_variance,
                });
            }
            let result = rebinder.ty(result)?;
            if matches!(what, Abstraction::Poly(_)) {
                Type::Poly(PolyType {
                    params: out,
                    result,
                })
            } else {
                Type::TypeLambda(TypeLambda {
                    params: out,
                    result,
                })
            }
        }
    };
    rebinder.store.types.fill(reserved, built);
    Ok(binder)
}

struct Rebinder<'a> {
    store: &'a mut SemanticStore,
    /// Old type -> its transformed (or reused) type.
    memo: HashMap<TypeId, TypeId>,
    /// Old binder -> the copy that replaces it, for `ParamRef` and `RecThis`.
    binders: HashMap<TypeId, TypeId>,
    annotations: HashMap<AnnotationId, AnnotationId>,
    /// Parameter symbol -> the binder position that replaces every reference
    /// to it (`abstract_symbols`). Empty when only rebinding.
    substitution: HashMap<SymbolId, (TypeId, u32)>,
    /// `(target class, new recursive binder)` for [`close_over_this`]: every
    /// `ThisType { class: target }` becomes the one canonical `RecThis` of the
    /// new binder. `None` outside a close-over call.
    close_over: Option<(SymbolId, TypeId)>,
    /// The one `RecThis` [`close_over`](Self::close_over) has allocated so
    /// far, made on first use and reused for every later occurrence.
    close_over_rec_this: Option<TypeId>,
    /// Types being transformed right now, to refuse a cycle.
    in_progress: HashSet<TypeId>,
    depth: usize,
}

impl<'a> Rebinder<'a> {
    fn new(store: &'a mut SemanticStore) -> Self {
        Self {
            store,
            memo: HashMap::new(),
            binders: HashMap::new(),
            annotations: HashMap::new(),
            substitution: HashMap::new(),
            close_over: None,
            close_over_rec_this: None,
            in_progress: HashSet::new(),
            depth: 0,
        }
    }

    /// The transformed `id`: the copy if anything under it depends on a
    /// rebound binder, `id` itself if not.
    fn ty(&mut self, id: TypeId) -> Result<TypeId, TypeRebindError> {
        if let Some(&done) = self.memo.get(&id) {
            return Ok(done);
        }
        if !self.store.types.is_filled(id) {
            return Err(TypeRebindError::UnfilledType { id });
        }
        if !self.in_progress.insert(id) {
            return Err(TypeRebindError::CyclicType { id });
        }
        if self.depth >= MAX_DEPTH {
            return Err(TypeRebindError::TooDeep { id });
        }
        self.depth += 1;
        let built = self.build(id);
        self.depth -= 1;
        self.in_progress.remove(&id);
        let new = built?;
        self.memo.insert(id, new);
        Ok(new)
    }

    /// The `ParamRef` that replaces a reference to the parameter `symbol`.
    fn parameter_ref(&mut self, symbol: SymbolId) -> TypeId {
        let (binder, index) = self.substitution[&symbol];
        self.store.types.alloc(Type::ParamRef { binder, index })
    }

    /// The one canonical `RecThis` of `binder` for a [`close_over_this`]
    /// call, allocated on first use.
    fn close_over_this_ref(&mut self, binder: TypeId) -> TypeId {
        if let Some(existing) = self.close_over_rec_this {
            return existing;
        }
        let rec_this = self.store.types.alloc(Type::RecThis { binder });
        self.close_over_rec_this = Some(rec_this);
        rec_this
    }

    fn all(&mut self, ids: &[TypeId]) -> Result<Vec<TypeId>, TypeRebindError> {
        ids.iter().map(|&id| self.ty(id)).collect()
    }

    /// `old` if `new` is the same type, else a fresh allocation of `make`.
    fn keep_or_alloc(&mut self, old: TypeId, unchanged: bool, make: Type) -> TypeId {
        if unchanged {
            old
        } else {
            self.store.types.alloc(make)
        }
    }

    fn annotation(&mut self, id: AnnotationId) -> Result<AnnotationId, TypeRebindError> {
        if let Some(&done) = self.annotations.get(&id) {
            return Ok(done);
        }
        let annotation = self.store.annotations.get(id).clone();
        let ty = self.ty(annotation.ty)?;
        // The term arguments can name types too (`classOf[T]`); rebinding
        // them is part of the annotation, and the rest is copied as it is.
        let arguments = match &annotation.arguments {
            AnnotationArguments::Unavailable => AnnotationArguments::Unavailable,
            AnnotationArguments::Known(arguments) => {
                let mut rebound = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    let value = match &argument.value {
                        AnnotationValue::Constant(Constant::Class(class)) => {
                            AnnotationValue::Constant(Constant::Class(self.ty(*class)?))
                        }
                        unchanged @ AnnotationValue::Constant(_) => unchanged.clone(),
                    };
                    rebound.push(AnnotationArgument {
                        name: argument.name,
                        value,
                    });
                }
                AnnotationArguments::Known(rebound)
            }
        };
        let new = if ty == annotation.ty && arguments == annotation.arguments {
            id
        } else {
            // A new annotation, never a change to the stored one.
            self.store.annotations.alloc(Annotation {
                ty,
                arguments,
                tree: annotation.tree,
            })
        };
        self.annotations.insert(id, new);
        Ok(new)
    }

    /// Copies the lambda `id` under a reserved id. `declared` replaces the
    /// parameters' declared variances (the lambda being rebound); a nested
    /// lambda keeps its own.
    fn lambda(
        &mut self,
        id: TypeId,
        lambda: &TypeLambda,
        declared: Option<&[Variance]>,
    ) -> Result<TypeId, TypeRebindError> {
        let reserved = self.store.types.reserve();
        let new = reserved.id();
        self.memo.insert(id, new);
        self.binders.insert(id, new);
        let params = self.type_params(&lambda.params, declared)?;
        let result = self.ty(lambda.result)?;
        self.store
            .types
            .fill(reserved, Type::TypeLambda(TypeLambda { params, result }));
        Ok(new)
    }

    fn type_params(
        &mut self,
        params: &[TypeParam],
        declared: Option<&[Variance]>,
    ) -> Result<Vec<TypeParam>, TypeRebindError> {
        let mut out = Vec::with_capacity(params.len());
        for (position, param) in params.iter().enumerate() {
            out.push(TypeParam {
                name: param.name,
                bounds: self.ty(param.bounds)?,
                declared_variance: match declared {
                    Some(variances) => Some(variances[position]),
                    None => param.declared_variance,
                },
            });
        }
        Ok(out)
    }

    fn build(&mut self, id: TypeId) -> Result<TypeId, TypeRebindError> {
        let ty = self.store.types.get(id).clone();
        Ok(match ty {
            Type::NoType | Type::Error(_) | Type::NoPrefix => id,
            Type::ThisType { class } => match self.close_over {
                Some((target, binder)) if class == target => self.close_over_this_ref(binder),
                _ => id,
            },
            Type::Constant(Constant::Class(class)) => {
                let new = self.ty(class)?;
                self.keep_or_alloc(id, new == class, Type::Constant(Constant::Class(new)))
            }
            Type::Constant(_) => id,

            // Only the prefix is rebound; a symbol or a name designator stays
            // as it is.
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } if self.substitution.contains_key(&symbol) => self.parameter_ref(symbol),
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } if self.substitution.contains_key(&symbol) => self.parameter_ref(symbol),
            Type::TermRef { prefix, target } => {
                let new = self.ty(prefix)?;
                self.keep_or_alloc(
                    id,
                    new == prefix,
                    Type::TermRef {
                        prefix: new,
                        target,
                    },
                )
            }
            Type::TypeRef { prefix, target } => {
                let new = self.ty(prefix)?;
                self.keep_or_alloc(
                    id,
                    new == prefix,
                    Type::TypeRef {
                        prefix: new,
                        target,
                    },
                )
            }
            Type::SuperType {
                this_type,
                super_type,
            } => {
                let (a, b) = (self.ty(this_type)?, self.ty(super_type)?);
                self.keep_or_alloc(
                    id,
                    (a, b) == (this_type, super_type),
                    Type::SuperType {
                        this_type: a,
                        super_type: b,
                    },
                )
            }

            Type::Applied { tycon, args } => {
                let new_tycon = self.ty(tycon)?;
                let new_args = self.all(&args)?;
                self.keep_or_alloc(
                    id,
                    new_tycon == tycon && new_args == args,
                    Type::Applied {
                        tycon: new_tycon,
                        args: new_args,
                    },
                )
            }
            Type::Bounds { low, high } => {
                let (a, b) = (self.ty(low)?, self.ty(high)?);
                self.keep_or_alloc(id, (a, b) == (low, high), Type::Bounds { low: a, high: b })
            }
            Type::AliasingBounds { alias } => {
                let new = self.ty(alias)?;
                self.keep_or_alloc(id, new == alias, Type::AliasingBounds { alias: new })
            }
            Type::ByName { result } => {
                let new = self.ty(result)?;
                self.keep_or_alloc(id, new == result, Type::ByName { result: new })
            }
            Type::Flexible { underlying } => {
                let new = self.ty(underlying)?;
                self.keep_or_alloc(id, new == underlying, Type::Flexible { underlying: new })
            }
            Type::And { left, right } => {
                let (a, b) = (self.ty(left)?, self.ty(right)?);
                self.keep_or_alloc(id, (a, b) == (left, right), Type::And { left: a, right: b })
            }
            Type::Or { left, right } => {
                let (a, b) = (self.ty(left)?, self.ty(right)?);
                self.keep_or_alloc(id, (a, b) == (left, right), Type::Or { left: a, right: b })
            }
            Type::Refined { parent, name, info } => {
                let (a, b) = (self.ty(parent)?, self.ty(info)?);
                self.keep_or_alloc(
                    id,
                    (a, b) == (parent, info),
                    Type::Refined {
                        parent: a,
                        name,
                        info: b,
                    },
                )
            }

            // A recursive type is a binder: reserve the copy first so that
            // every `RecThis` in the parent resolves to it.
            Type::Recursive { parent } => {
                let reserved = self.store.types.reserve();
                let new = reserved.id();
                self.memo.insert(id, new);
                self.binders.insert(id, new);
                let parent = self.ty(parent)?;
                self.store.types.fill(reserved, Type::Recursive { parent });
                new
            }
            Type::RecThis { binder } => match self.binders.get(&binder).copied() {
                Some(new) => self.store.types.alloc(Type::RecThis { binder: new }),
                None => id,
            },

            Type::Method(method) => {
                let reserved = self.store.types.reserve();
                let new = reserved.id();
                self.memo.insert(id, new);
                self.binders.insert(id, new);
                let mut params = Vec::with_capacity(method.params.len());
                for param in &method.params {
                    params.push(MethodParam {
                        ty: self.ty(param.ty)?,
                        ..*param
                    });
                }
                let result = self.ty(method.result)?;
                self.store.types.fill(
                    reserved,
                    Type::Method(MethodType {
                        params,
                        result,
                        kind: method.kind,
                    }),
                );
                new
            }
            Type::Poly(poly) => {
                let reserved = self.store.types.reserve();
                let new = reserved.id();
                self.memo.insert(id, new);
                self.binders.insert(id, new);
                let params = self.type_params(&poly.params, None)?;
                let result = self.ty(poly.result)?;
                self.store
                    .types
                    .fill(reserved, Type::Poly(PolyType { params, result }));
                new
            }
            Type::TypeLambda(lambda) => self.lambda(id, &lambda, None)?,
            Type::ParamRef { binder, index } => match self.binders.get(&binder).copied() {
                Some(new) => self
                    .store
                    .types
                    .alloc(Type::ParamRef { binder: new, index }),
                None => id,
            },

            Type::Match(MatchType {
                bound,
                scrutinee,
                cases,
            }) => {
                let (a, b) = (self.ty(bound)?, self.ty(scrutinee)?);
                let new_cases = self.all(&cases)?;
                self.keep_or_alloc(
                    id,
                    (a, b) == (bound, scrutinee) && new_cases == cases,
                    Type::Match(MatchType {
                        bound: a,
                        scrutinee: b,
                        cases: new_cases,
                    }),
                )
            }
            Type::MatchCase { pattern, result } => {
                let (a, b) = (self.ty(pattern)?, self.ty(result)?);
                self.keep_or_alloc(
                    id,
                    (a, b) == (pattern, result),
                    Type::MatchCase {
                        pattern: a,
                        result: b,
                    },
                )
            }

            Type::Annotated {
                underlying,
                annotation,
            } => {
                let new_underlying = self.ty(underlying)?;
                let new_annotation = self.annotation(annotation)?;
                self.keep_or_alloc(
                    id,
                    new_underlying == underlying && new_annotation == annotation,
                    Type::Annotated {
                        underlying: new_underlying,
                        annotation: new_annotation,
                    },
                )
            }

            Type::Wildcard { bounds } => {
                let new = self.ty(bounds)?;
                self.keep_or_alloc(id, new == bounds, Type::Wildcard { bounds: new })
            }
            Type::JavaArray { element } => {
                let new = self.ty(element)?;
                self.keep_or_alloc(id, new == element, Type::JavaArray { element: new })
            }

            // The class symbol and its declaration scope are symbol-table
            // state, not part of the type graph: they are kept as they are.
            Type::ClassInfo(info) => {
                let prefix = self.ty(info.prefix)?;
                let parents = self.all(&info.parents)?;
                let self_type = match info.self_type {
                    Some(self_type) => Some(self.ty(self_type)?),
                    None => None,
                };
                let unchanged =
                    prefix == info.prefix && parents == info.parents && self_type == info.self_type;
                self.keep_or_alloc(
                    id,
                    unchanged,
                    Type::ClassInfo(ClassInfo {
                        prefix,
                        parents,
                        self_type,
                        ..info
                    }),
                )
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ScopeId, SymbolId};
    use crate::names::{Name, Namespace, TermName, TypeName};
    use crate::types::method::MethodKind;

    struct Fixture {
        store: SemanticStore,
        /// A type with no parts, for tycons and bounds.
        leaf: TypeId,
    }

    impl Fixture {
        fn new() -> Self {
            let mut store = SemanticStore::new();
            let leaf = store.types.alloc(Type::ThisType {
                class: SymbolId::new(0),
            });
            Self { store, leaf }
        }

        fn type_name(&mut self, text: &str) -> TypeName {
            TypeName::new(self.store.names.intern(text))
        }

        fn param(&mut self, text: &str, bounds: TypeId) -> TypeParam {
            TypeParam {
                name: self.type_name(text),
                bounds,
                declared_variance: None,
            }
        }

        /// `>: leaf <: leaf`, which mentions no binder.
        fn plain_bounds(&mut self) -> TypeId {
            let leaf = self.leaf;
            self.store.types.alloc(Type::Bounds {
                low: leaf,
                high: leaf,
            })
        }

        fn param_ref(&mut self, binder: TypeId, index: u32) -> TypeId {
            self.store.types.alloc(Type::ParamRef { binder, index })
        }

        fn applied(&mut self, args: &[TypeId]) -> TypeId {
            let tycon = self.leaf;
            self.store.types.alloc(Type::Applied {
                tycon,
                args: args.to_vec(),
            })
        }

        /// A lambda whose parameters all have plain bounds and whose result is
        /// built by `result` from the lambda's own id.
        fn lambda(
            &mut self,
            names: &[&str],
            result: impl FnOnce(&mut Self, TypeId) -> TypeId,
        ) -> TypeId {
            let reserved = self.store.types.reserve();
            let id = reserved.id();
            let params = names
                .iter()
                .map(|name| {
                    let bounds = self.plain_bounds();
                    self.param(name, bounds)
                })
                .collect();
            let result = result(self, id);
            self.store
                .types
                .fill(reserved, Type::TypeLambda(TypeLambda { params, result }));
            id
        }

        fn get_lambda(&self, id: TypeId) -> &TypeLambda {
            match self.store.types.get(id) {
                Type::TypeLambda(lambda) => lambda,
                other => panic!("not a lambda: {other:?}"),
            }
        }

        fn param_ref_of(&self, id: TypeId) -> (TypeId, u32) {
            match self.store.types.get(id) {
                Type::ParamRef { binder, index } => (*binder, *index),
                other => panic!("not a parameter reference: {other:?}"),
            }
        }
    }

    #[test]
    fn a_rebound_lambda_is_a_fresh_binder_whose_result_names_it() {
        let mut f = Fixture::new();
        let old = f.lambda(&["A"], |f, me| f.param_ref(me, 0));

        let new = rebind_type_lambda(&mut f.store, old, &[Variance::Covariant]).unwrap();

        assert_ne!(new, old);
        assert_eq!(f.get_lambda(old).params[0].declared_variance, None);
        assert_eq!(
            f.get_lambda(new).params[0].declared_variance,
            Some(Variance::Covariant)
        );
        // The old graph is untouched, the new one names the new binder.
        assert_eq!(f.param_ref_of(f.get_lambda(old).result), (old, 0));
        assert_eq!(f.param_ref_of(f.get_lambda(new).result), (new, 0));
    }

    #[test]
    fn an_explicit_invariant_is_not_the_absence_of_a_declaration() {
        let mut f = Fixture::new();
        let old = f.lambda(&["A"], |f, me| f.param_ref(me, 0));

        let new = rebind_type_lambda(&mut f.store, old, &[Variance::Invariant]).unwrap();

        assert_eq!(
            f.get_lambda(new).params[0].declared_variance,
            Some(Variance::Invariant)
        );
        assert_eq!(f.get_lambda(old).params[0].declared_variance, None);
    }

    #[test]
    fn variances_are_applied_in_parameter_order_and_names_and_bounds_are_kept() {
        let mut f = Fixture::new();
        let old = f.lambda(&["A", "B", "C"], |f, me| {
            let refs: Vec<_> = (0..3).map(|i| f.param_ref(me, i)).collect();
            f.applied(&refs)
        });
        let variances = [
            Variance::Covariant,
            Variance::Invariant,
            Variance::Contravariant,
        ];

        let new = rebind_type_lambda(&mut f.store, old, &variances).unwrap();

        let (old_params, new_params) = (&f.get_lambda(old).params, &f.get_lambda(new).params);
        for (position, variance) in variances.iter().enumerate() {
            assert_eq!(new_params[position].declared_variance, Some(*variance));
            assert_eq!(new_params[position].name, old_params[position].name);
            // The bounds mention no binder, so they are the very same types.
            assert_eq!(new_params[position].bounds, old_params[position].bounds);
        }
        // Every parameter reference moved to the new binder, keeping its index.
        let Type::Applied { args, .. } = f.store.types.get(f.get_lambda(new).result) else {
            panic!("not an application");
        };
        for (position, arg) in args.iter().enumerate() {
            assert_eq!(f.param_ref_of(*arg), (new, position as u32));
        }
    }

    #[test]
    fn an_f_bound_names_the_new_binder_not_the_old_one() {
        // `[A <: Box[A]] =>> A`: the bound of A mentions the lambda itself.
        let mut f = Fixture::new();
        let reserved = f.store.types.reserve();
        let old = reserved.id();
        let a = f.param_ref(old, 0);
        let box_of_a = f.applied(&[a]);
        let leaf = f.leaf;
        let bounds = f.store.types.alloc(Type::Bounds {
            low: leaf,
            high: box_of_a,
        });
        let param = f.param("A", bounds);
        let result = f.param_ref(old, 0);
        f.store.types.fill(
            reserved,
            Type::TypeLambda(TypeLambda {
                params: vec![param],
                result,
            }),
        );

        let new = rebind_type_lambda(&mut f.store, old, &[Variance::Covariant]).unwrap();

        let Type::Bounds { high, .. } = f.store.types.get(f.get_lambda(new).params[0].bounds)
        else {
            panic!("not bounds");
        };
        let Type::Applied { args, .. } = f.store.types.get(*high) else {
            panic!("not an application");
        };
        assert_eq!(f.param_ref_of(args[0]), (new, 0));
        // And the old bound still names the old binder.
        let Type::Bounds { high, .. } = f.store.types.get(f.get_lambda(old).params[0].bounds)
        else {
            panic!("not bounds");
        };
        let Type::Applied { args, .. } = f.store.types.get(*high) else {
            panic!("not an application");
        };
        assert_eq!(f.param_ref_of(args[0]), (old, 0));
    }

    #[test]
    fn a_type_that_mentions_no_rebound_binder_keeps_its_id() {
        let mut f = Fixture::new();
        let independent = f.applied(&[]);
        let old = f.lambda(&["A"], |f, me| {
            let a = f.param_ref(me, 0);
            f.store.types.alloc(Type::And {
                left: independent,
                right: a,
            })
        });

        let new = rebind_type_lambda(&mut f.store, old, &[Variance::Covariant]).unwrap();

        let Type::And { left, right } = f.store.types.get(f.get_lambda(new).result) else {
            panic!("not an And");
        };
        assert_eq!(*left, independent);
        assert_eq!(f.param_ref_of(*right), (new, 0));
    }

    #[test]
    fn a_nested_binder_is_copied_and_stays_internally_consistent() {
        // `[A] =>> [B] =>> (A, B)`: the inner lambda names both binders.
        let mut f = Fixture::new();
        let mut inner_old = None;
        let outer = f.lambda(&["A"], |f, outer| {
            let inner = f.lambda(&["B"], |f, inner| {
                let a = f.param_ref(outer, 0);
                let b = f.param_ref(inner, 0);
                f.applied(&[a, b])
            });
            inner_old = Some(inner);
            inner
        });
        let inner_old = inner_old.unwrap();

        let new_outer = rebind_type_lambda(&mut f.store, outer, &[Variance::Covariant]).unwrap();

        let new_inner = f.get_lambda(new_outer).result;
        assert_ne!(new_inner, inner_old);
        let Type::Applied { args, .. } = f.store.types.get(f.get_lambda(new_inner).result) else {
            panic!("not an application");
        };
        assert_eq!(f.param_ref_of(args[0]), (new_outer, 0));
        assert_eq!(f.param_ref_of(args[1]), (new_inner, 0));
        // The copy keeps the inner lambda's own (absent) declaration.
        assert_eq!(f.get_lambda(new_inner).params[0].declared_variance, None);
        // The original inner lambda still names the original binders.
        let Type::Applied { args, .. } = f.store.types.get(f.get_lambda(inner_old).result) else {
            panic!("not an application");
        };
        assert_eq!(f.param_ref_of(args[0]), (outer, 0));
        assert_eq!(f.param_ref_of(args[1]), (inner_old, 0));
    }

    #[test]
    fn nested_poly_and_method_binders_are_remapped_too() {
        // `[A] =>> [T] => (x: A, y: T): T`
        let mut f = Fixture::new();
        let mut poly_old = None;
        let outer = f.lambda(&["A"], |f, outer| {
            let poly = f.store.types.reserve();
            let poly_id = poly.id();
            let method = f.store.types.reserve();
            let method_id = method.id();
            let a = f.param_ref(outer, 0);
            let t = f.param_ref(poly_id, 0);
            let result = f.param_ref(poly_id, 0);
            let params = vec![
                MethodParam {
                    name: TermName::new(f.store.names.intern("x")),
                    ty: a,
                    erased: false,
                    varargs: false,
                },
                MethodParam {
                    name: TermName::new(f.store.names.intern("y")),
                    ty: t,
                    erased: false,
                    varargs: true,
                },
            ];
            f.store.types.fill(
                method,
                Type::Method(MethodType {
                    params,
                    result,
                    kind: MethodKind::Contextual,
                }),
            );
            let bounds = f.plain_bounds();
            let t_param = f.param("T", bounds);
            f.store.types.fill(
                poly,
                Type::Poly(PolyType {
                    params: vec![t_param],
                    result: method_id,
                }),
            );
            poly_old = Some(poly_id);
            poly_id
        });
        let poly_old = poly_old.unwrap();

        let new_outer = rebind_type_lambda(&mut f.store, outer, &[Variance::Covariant]).unwrap();

        let new_poly = f.get_lambda(new_outer).result;
        assert_ne!(new_poly, poly_old);
        let Type::Poly(poly) = f.store.types.get(new_poly) else {
            panic!("not a poly");
        };
        let new_method = poly.result;
        let Type::Method(method) = f.store.types.get(new_method) else {
            panic!("not a method");
        };
        assert_eq!(method.kind, MethodKind::Contextual);
        assert_eq!(f.param_ref_of(method.params[0].ty), (new_outer, 0));
        assert_eq!(f.param_ref_of(method.params[1].ty), (new_poly, 0));
        assert!(method.params[1].varargs && !method.params[1].erased);
        assert_eq!(f.param_ref_of(method.result), (new_poly, 0));
    }

    #[test]
    fn a_recursive_type_is_copied_with_its_rec_this_pointing_at_the_copy() {
        // `[A] =>> ({ this } & A)`: `Recursive { parent: And(RecThis, A) }`.
        let mut f = Fixture::new();
        let mut recursive_old = None;
        let old = f.lambda(&["A"], |f, me| {
            let rec = f.store.types.reserve();
            let rec_id = rec.id();
            let this = f.store.types.alloc(Type::RecThis { binder: rec_id });
            let a = f.param_ref(me, 0);
            let parent = f.store.types.alloc(Type::And {
                left: this,
                right: a,
            });
            f.store.types.fill(rec, Type::Recursive { parent });
            recursive_old = Some(rec_id);
            rec_id
        });
        let recursive_old = recursive_old.unwrap();

        let new = rebind_type_lambda(&mut f.store, old, &[Variance::Covariant]).unwrap();

        let new_rec = f.get_lambda(new).result;
        assert_ne!(new_rec, recursive_old);
        let Type::Recursive { parent } = f.store.types.get(new_rec) else {
            panic!("not recursive");
        };
        let Type::And { left, right } = f.store.types.get(*parent) else {
            panic!("not an And");
        };
        assert_eq!(f.store.types.get(*left), &Type::RecThis { binder: new_rec });
        assert_eq!(f.param_ref_of(*right), (new, 0));
        // The original recursive type still ties its own knot.
        let Type::Recursive { parent } = f.store.types.get(recursive_old) else {
            panic!("not recursive");
        };
        let Type::And { left, .. } = f.store.types.get(*parent) else {
            panic!("not an And");
        };
        assert_eq!(
            f.store.types.get(*left),
            &Type::RecThis {
                binder: recursive_old
            }
        );
    }

    #[test]
    fn a_shared_dependent_node_becomes_one_transformed_node() {
        let mut f = Fixture::new();
        let old = f.lambda(&["A"], |f, me| {
            let a = f.param_ref(me, 0);
            let shared = f.applied(&[a]);
            f.store.types.alloc(Type::And {
                left: shared,
                right: shared,
            })
        });

        let new = rebind_type_lambda(&mut f.store, old, &[Variance::Covariant]).unwrap();

        let Type::And { left, right } = f.store.types.get(f.get_lambda(new).result) else {
            panic!("not an And");
        };
        assert_eq!(left, right);
        let Type::And { left: old_side, .. } = f.store.types.get(f.get_lambda(old).result) else {
            panic!("not an And");
        };
        assert_ne!(left, old_side);
    }

    #[test]
    fn an_annotation_is_transformed_into_a_new_annotation_not_mutated() {
        let mut f = Fixture::new();
        let mut annotated_old = None;
        let mut unrelated = None;
        let old = f.lambda(&["A"], |f, me| {
            let a = f.param_ref(me, 0);
            let leaf = f.leaf;
            let dependent = f.store.annotations.alloc(Annotation::new(a, None));
            let independent = f.store.annotations.alloc(Annotation::new(leaf, None));
            let inner = f.store.types.alloc(Type::Annotated {
                underlying: leaf,
                annotation: dependent,
            });
            annotated_old = Some((inner, dependent));
            unrelated = Some(independent);
            f.store.types.alloc(Type::Annotated {
                underlying: inner,
                annotation: independent,
            })
        });
        let (inner_old, dependent_old) = annotated_old.unwrap();

        let new = rebind_type_lambda(&mut f.store, old, &[Variance::Covariant]).unwrap();

        let Type::Annotated {
            underlying,
            annotation,
        } = f.store.types.get(f.get_lambda(new).result)
        else {
            panic!("not annotated");
        };
        // The annotation that mentions nothing rebound is reused as it is.
        assert_eq!(*annotation, unrelated.unwrap());
        let Type::Annotated { annotation, .. } = f.store.types.get(*underlying) else {
            panic!("not annotated");
        };
        assert_ne!(*annotation, dependent_old);
        assert_eq!(
            f.param_ref_of(f.store.annotations.get(*annotation).ty),
            (new, 0)
        );
        // The stored annotation still names the old binder.
        assert_eq!(
            f.param_ref_of(f.store.annotations.get(dependent_old).ty),
            (old, 0)
        );
        assert_ne!(*underlying, inner_old);
    }

    fn class_argument(class: TypeId, name: Option<TermName>) -> AnnotationArgument {
        AnnotationArgument {
            name,
            value: AnnotationValue::Constant(Constant::Class(class)),
        }
    }

    #[test]
    fn a_class_literal_argument_naming_the_binder_is_rebound_in_a_new_annotation() {
        let mut f = Fixture::new();
        let name = TermName::new(crate::ids::NameId::new(3));
        let mut old_annotation = None;
        let old = f.lambda(&["A"], |f, me| {
            let a = f.param_ref(me, 0);
            let leaf = f.leaf;
            let annotation = f.store.annotations.alloc(Annotation::with_arguments(
                leaf,
                vec![
                    AnnotationArgument {
                        name: None,
                        value: AnnotationValue::Constant(Constant::Int(1)),
                    },
                    class_argument(a, Some(name)),
                ],
            ));
            old_annotation = Some(annotation);
            f.store.types.alloc(Type::Annotated {
                underlying: leaf,
                annotation,
            })
        });
        let old_annotation = old_annotation.unwrap();

        let new = rebind_type_lambda(&mut f.store, old, &[Variance::Covariant]).unwrap();

        let Type::Annotated { annotation, .. } = f.store.types.get(f.get_lambda(new).result) else {
            panic!("not annotated");
        };
        assert_ne!(*annotation, old_annotation);
        let AnnotationArguments::Known(arguments) = &f.store.annotations.get(*annotation).arguments
        else {
            panic!("arguments lost");
        };
        // Order and names are kept; only the type inside the class literal moves.
        assert_eq!(arguments.len(), 2);
        assert_eq!(
            arguments[0].value,
            AnnotationValue::Constant(Constant::Int(1))
        );
        assert_eq!(arguments[1].name, Some(name));
        let AnnotationValue::Constant(Constant::Class(class)) = arguments[1].value else {
            panic!("not a class literal");
        };
        assert_eq!(f.param_ref_of(class), (new, 0));
        // The stored annotation still names the old binder.
        let AnnotationArguments::Known(old_arguments) =
            &f.store.annotations.get(old_annotation).arguments
        else {
            panic!("arguments lost");
        };
        let AnnotationValue::Constant(Constant::Class(old_class)) = old_arguments[1].value else {
            panic!("not a class literal");
        };
        assert_eq!(f.param_ref_of(old_class), (old, 0));
    }

    #[test]
    fn annotation_arguments_that_name_no_type_are_reused_not_copied() {
        let mut f = Fixture::new();
        let mut unavailable = None;
        let mut known = None;
        let old = f.lambda(&["A"], |f, me| {
            let a = f.param_ref(me, 0);
            let leaf = f.leaf;
            let plain = f.store.annotations.alloc(Annotation::new(leaf, None));
            let with_constants = f.store.annotations.alloc(Annotation::with_arguments(
                leaf,
                vec![
                    AnnotationArgument {
                        name: None,
                        value: AnnotationValue::Constant(Constant::Int(1)),
                    },
                    class_argument(leaf, None),
                ],
            ));
            unavailable = Some(plain);
            known = Some(with_constants);
            let inner = f.store.types.alloc(Type::Annotated {
                underlying: a,
                annotation: plain,
            });
            f.store.types.alloc(Type::Annotated {
                underlying: inner,
                annotation: with_constants,
            })
        });

        let new = rebind_type_lambda(&mut f.store, old, &[Variance::Covariant]).unwrap();

        let Type::Annotated {
            underlying,
            annotation,
        } = f.store.types.get(f.get_lambda(new).result)
        else {
            panic!("not annotated");
        };
        assert_eq!(*annotation, known.unwrap());
        let Type::Annotated { annotation, .. } = f.store.types.get(*underlying) else {
            panic!("not annotated");
        };
        assert_eq!(*annotation, unavailable.unwrap());
        // Reuse keeps "unavailable" unavailable: it is never turned into "none".
        assert_eq!(
            f.store.annotations.get(*annotation).arguments,
            AnnotationArguments::Unavailable
        );
    }

    #[test]
    fn class_info_type_fields_are_rebound_and_its_symbol_and_scope_kept() {
        let mut f = Fixture::new();
        let old = f.lambda(&["A"], |f, me| {
            let a = f.param_ref(me, 0);
            let leaf = f.leaf;
            f.store.types.alloc(Type::ClassInfo(ClassInfo {
                prefix: leaf,
                class: SymbolId::new(7),
                parents: vec![leaf, a],
                declarations: ScopeId::new(9),
                self_type: Some(a),
            }))
        });

        let new = rebind_type_lambda(&mut f.store, old, &[Variance::Covariant]).unwrap();

        let Type::ClassInfo(info) = f.store.types.get(f.get_lambda(new).result) else {
            panic!("not class info");
        };
        assert_eq!(info.class, SymbolId::new(7));
        assert_eq!(info.declarations, ScopeId::new(9));
        assert_eq!(info.prefix, f.leaf);
        assert_eq!(info.parents[0], f.leaf);
        assert_eq!(f.param_ref_of(info.parents[1]), (new, 0));
        assert_eq!(f.param_ref_of(info.self_type.unwrap()), (new, 0));
    }

    #[test]
    fn a_name_designated_reference_gets_a_rebound_prefix_and_keeps_its_name() {
        use crate::names::{TermName, TypeName};
        use crate::types::{TermRefTarget, TypeRefTarget};
        let mut f = Fixture::new();
        let text = f.store.names.intern("T1");
        let old = f.lambda(&["A"], |f, me| {
            let a = f.param_ref(me, 0);
            let by_type = f.store.types.alloc(Type::TypeRef {
                prefix: a,
                target: TypeRefTarget::Name(TypeName::new(text)),
            });
            let by_term = f.store.types.alloc(Type::TermRef {
                prefix: a,
                target: TermRefTarget::Name(TermName::new(text)),
            });
            f.applied(&[by_type, by_term])
        });
        let old_result = f.get_lambda(old).result;

        let new = rebind_type_lambda(&mut f.store, old, &[Variance::Covariant]).unwrap();

        let Type::Applied { args, .. } = f.store.types.get(f.get_lambda(new).result).clone() else {
            panic!("not an applied type");
        };
        let [by_type, by_term] = args[..] else {
            panic!("two arguments expected");
        };
        let Type::TypeRef { prefix, target } = *f.store.types.get(by_type) else {
            panic!("not a type ref");
        };
        assert_eq!(target, TypeRefTarget::Name(TypeName::new(text)));
        assert_eq!(f.param_ref_of(prefix), (new, 0));
        let Type::TermRef { prefix, target } = *f.store.types.get(by_term) else {
            panic!("not a term ref");
        };
        assert_eq!(target, TermRefTarget::Name(TermName::new(text)));
        assert_eq!(f.param_ref_of(prefix), (new, 0));
        // The original graph is unchanged.
        assert_eq!(f.get_lambda(old).result, old_result);
        let Type::Applied { args, .. } = f.store.types.get(old_result).clone() else {
            panic!("not an applied type");
        };
        let Type::TypeRef { prefix, .. } = *f.store.types.get(args[0]) else {
            panic!("not a type ref");
        };
        assert_eq!(f.param_ref_of(prefix), (old, 0));
    }

    #[test]
    fn every_remaining_shape_is_traversed() {
        // One node of each remaining compound kind, all over `A`.
        let mut f = Fixture::new();
        let old = f.lambda(&["A"], |f, me| {
            let a = f.param_ref(me, 0);
            let leaf = f.leaf;
            let symbol = SymbolId::new(1);
            let members = [
                Type::term_ref(a, symbol),
                Type::type_ref(a, symbol),
                Type::SuperType {
                    this_type: a,
                    super_type: leaf,
                },
                Type::Constant(Constant::Class(a)),
                Type::AliasingBounds { alias: a },
                Type::ByName { result: a },
                Type::Flexible { underlying: a },
                Type::Or {
                    left: leaf,
                    right: a,
                },
                Type::Refined {
                    parent: leaf,
                    name: Name::new(f.store.names.intern("m"), Namespace::Type),
                    info: a,
                },
                Type::MatchCase {
                    pattern: a,
                    result: leaf,
                },
                Type::Wildcard { bounds: a },
                Type::JavaArray { element: a },
            ];
            let ids: Vec<_> = members
                .into_iter()
                .map(|t| f.store.types.alloc(t))
                .collect();
            let case = f.store.types.alloc(Type::MatchCase {
                pattern: leaf,
                result: a,
            });
            let matched = f.store.types.alloc(Type::Match(MatchType {
                bound: a,
                scrutinee: leaf,
                cases: vec![case],
            }));
            let mut all = ids;
            all.push(matched);
            f.applied(&all)
        });

        let new = rebind_type_lambda(&mut f.store, old, &[Variance::Covariant]).unwrap();

        // Nothing reachable from the new lambda names the old one: walk it.
        let mut seen = HashSet::new();
        let mut stack = vec![new];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            match f.store.types.get(id).clone() {
                Type::ParamRef { binder, .. } => assert_ne!(binder, old),
                Type::TermRef { prefix, .. } | Type::TypeRef { prefix, .. } => stack.push(prefix),
                Type::SuperType {
                    this_type,
                    super_type,
                } => stack.extend([this_type, super_type]),
                Type::Constant(Constant::Class(class)) => stack.push(class),
                Type::AliasingBounds { alias } => stack.push(alias),
                Type::ByName { result } => stack.push(result),
                Type::Flexible { underlying } => stack.push(underlying),
                Type::Or { left, right } | Type::And { left, right } => stack.extend([left, right]),
                Type::Refined { parent, info, .. } => stack.extend([parent, info]),
                Type::MatchCase { pattern, result } => stack.extend([pattern, result]),
                Type::Wildcard { bounds } => stack.push(bounds),
                Type::JavaArray { element } => stack.push(element),
                Type::Match(m) => {
                    stack.extend([m.bound, m.scrutinee]);
                    stack.extend(m.cases);
                }
                Type::Applied { tycon, args } => {
                    stack.push(tycon);
                    stack.extend(args);
                }
                Type::TypeLambda(l) => {
                    stack.push(l.result);
                    stack.extend(l.params.iter().map(|p| p.bounds));
                }
                _ => {}
            }
        }
        assert!(seen.len() > 12);
    }

    #[test]
    fn a_source_that_is_not_a_lambda_is_refused_and_the_store_untouched() {
        let mut f = Fixture::new();
        let before = f.store.checkpoint();
        let leaf = f.leaf;

        assert_eq!(
            rebind_type_lambda(&mut f.store, leaf, &[]),
            Err(TypeRebindError::NotATypeLambda { source: leaf })
        );
        assert_eq!(f.store.checkpoint(), before);
    }

    #[test]
    fn a_variance_count_that_differs_from_the_arity_is_refused_not_padded() {
        let mut f = Fixture::new();
        let old = f.lambda(&["A", "B"], |f, me| f.param_ref(me, 0));
        let before = f.store.checkpoint();

        for variances in [&[][..], &[Variance::Covariant], &[Variance::Covariant; 3]] {
            assert_eq!(
                rebind_type_lambda(&mut f.store, old, variances),
                Err(TypeRebindError::VarianceArityMismatch {
                    source: old,
                    expected: 2,
                    actual: variances.len()
                })
            );
        }
        assert_eq!(f.store.checkpoint(), before);
    }

    #[test]
    fn a_failure_part_way_through_leaves_no_allocation_behind() {
        // The result reaches a slot that was reserved and never filled, after
        // a copy of the parameter reference has already been allocated.
        let mut f = Fixture::new();
        let hole = f.store.types.reserve().id();
        let old = f.lambda(&["A"], |f, me| {
            let a = f.param_ref(me, 0);
            f.store.types.alloc(Type::And {
                left: a,
                right: hole,
            })
        });
        let before = f.store.checkpoint();

        assert_eq!(
            rebind_type_lambda(&mut f.store, old, &[Variance::Covariant]),
            Err(TypeRebindError::UnfilledType { id: hole })
        );
        assert_eq!(f.store.checkpoint(), before);
    }

    #[test]
    fn a_type_that_contains_itself_is_refused_not_looped_on() {
        let mut f = Fixture::new();
        let reserved = f.store.types.reserve();
        let looped = reserved.id();
        f.store
            .types
            .fill(reserved, Type::ByName { result: looped });
        let old = f.lambda(&["A"], |_, _| looped);
        let before = f.store.checkpoint();

        assert_eq!(
            rebind_type_lambda(&mut f.store, old, &[Variance::Covariant]),
            Err(TypeRebindError::CyclicType { id: looped })
        );
        assert_eq!(f.store.checkpoint(), before);
    }

    #[test]
    fn a_very_deep_graph_is_an_error_not_a_stack_overflow() {
        let mut f = Fixture::new();
        let mut deepest = f.leaf;
        for _ in 0..(MAX_DEPTH + 10) {
            deepest = f.store.types.alloc(Type::ByName { result: deepest });
        }
        let old = f.lambda(&["A"], |_, _| deepest);
        let before = f.store.checkpoint();

        assert!(matches!(
            rebind_type_lambda(&mut f.store, old, &[Variance::Covariant]),
            Err(TypeRebindError::TooDeep { .. })
        ));
        assert_eq!(f.store.checkpoint(), before);
    }

    #[test]
    fn rebinding_twice_gives_two_independent_binders() {
        let mut f = Fixture::new();
        let old = f.lambda(&["A"], |f, me| f.param_ref(me, 0));

        let covariant = rebind_type_lambda(&mut f.store, old, &[Variance::Covariant]).unwrap();
        let contravariant =
            rebind_type_lambda(&mut f.store, old, &[Variance::Contravariant]).unwrap();

        assert_ne!(covariant, contravariant);
        assert_eq!(
            f.param_ref_of(f.get_lambda(covariant).result),
            (covariant, 0)
        );
        assert_eq!(
            f.param_ref_of(f.get_lambda(contravariant).result),
            (contravariant, 0)
        );
    }

    // Abstracting parameter symbols into `ParamRef`s

    fn type_ref_to(f: &mut Fixture, symbol: SymbolId) -> TypeId {
        let prefix = f.store.types.alloc(Type::NoPrefix);
        f.store.types.alloc(Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(symbol),
        })
    }

    fn term_ref_to(f: &mut Fixture, symbol: SymbolId) -> TypeId {
        let prefix = f.store.types.alloc(Type::NoPrefix);
        f.store.types.alloc(Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(symbol),
        })
    }

    fn method_param(f: &mut Fixture, symbol: SymbolId, text: &str, ty: TypeId) -> MethodParamSpec {
        MethodParamSpec {
            symbol,
            name: TermName::new(f.store.names.intern(text)),
            ty,
            erased: false,
            varargs: false,
        }
    }

    fn type_param_spec(
        f: &mut Fixture,
        symbol: SymbolId,
        text: &str,
        bounds: TypeId,
    ) -> TypeParamSpec {
        TypeParamSpec {
            symbol,
            name: f.type_name(text),
            bounds,
            declared_variance: None,
        }
    }

    fn get_method(f: &Fixture, id: TypeId) -> &MethodType {
        match f.store.types.get(id) {
            Type::Method(method) => method,
            other => panic!("not a method: {other:?}"),
        }
    }

    fn type_count(f: &mut Fixture) -> u32 {
        f.store.types.alloc(Type::NoType).index()
    }

    #[test]
    fn a_reference_to_a_parameter_symbol_becomes_a_param_ref_of_the_new_method() {
        let mut f = Fixture::new();
        let (x, y) = (SymbolId::new(10), SymbolId::new(11));
        let x_ty = f.leaf;
        let y_ref = term_ref_to(&mut f, x);
        let first = method_param(&mut f, x, "x", x_ty);
        let second = method_param(&mut f, y, "y", y_ref);
        let result = term_ref_to(&mut f, y);

        let method =
            method_type_from_symbols(&mut f.store, &[first, second], result, MethodKind::Plain)
                .unwrap();

        let built = get_method(&f, method).clone();
        // `y: x.type` names position 0 of this very method, and the result
        // names position 1.
        assert_eq!(f.param_ref_of(built.params[1].ty), (method, 0));
        assert_eq!(f.param_ref_of(built.result), (method, 1));
        assert_eq!(built.params[0].ty, f.leaf);
        assert_eq!(built.kind, MethodKind::Plain);
        // The graph it was built from still names the symbols.
        assert!(matches!(
            f.store.types.get(y_ref),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == x
        ));
    }

    #[test]
    fn only_the_exact_symbol_is_replaced_never_a_name() {
        let mut f = Fixture::new();
        // Two parameters named `x`, one of this clause and one of another
        // method: only the first is abstracted.
        let (mine, other) = (SymbolId::new(10), SymbolId::new(11));
        let param = {
            let leaf = f.leaf;
            method_param(&mut f, mine, "x", leaf)
        };
        let stranger = term_ref_to(&mut f, other);

        let method =
            method_type_from_symbols(&mut f.store, &[param], stranger, MethodKind::Plain).unwrap();

        // No reference to `mine` in the result: it is the very same type.
        assert_eq!(get_method(&f, method).result, stranger);
    }

    #[test]
    fn a_type_with_no_reference_to_a_parameter_keeps_its_id() {
        let mut f = Fixture::new();
        let param = {
            let leaf = f.leaf;
            method_param(&mut f, SymbolId::new(10), "x", leaf)
        };
        let applied = f.applied(&[f.leaf]);

        let method =
            method_type_from_symbols(&mut f.store, &[param], applied, MethodKind::Contextual)
                .unwrap();

        assert_eq!(get_method(&f, method).result, applied);
        assert_eq!(get_method(&f, method).kind, MethodKind::Contextual);
    }

    #[test]
    fn an_f_bound_names_the_poly_that_is_built() {
        // `[A <: Comparable[A]]: A`
        let mut f = Fixture::new();
        let a = SymbolId::new(20);
        let a_ref = type_ref_to(&mut f, a);
        let comparable = f.applied(&[a_ref]);
        let bounds = f.store.types.alloc(Type::Bounds {
            low: f.leaf,
            high: comparable,
        });
        let spec = type_param_spec(&mut f, a, "A", bounds);

        let poly = poly_type_from_symbols(&mut f.store, &[spec], a_ref).unwrap();

        let Type::Poly(built) = f.store.types.get(poly).clone() else {
            panic!("not a poly");
        };
        assert_eq!(f.param_ref_of(built.result), (poly, 0));
        let Type::Bounds { high, .. } = f.store.types.get(built.params[0].bounds).clone() else {
            panic!("not bounds");
        };
        let Type::Applied { args, .. } = f.store.types.get(high).clone() else {
            panic!("not applied");
        };
        assert_eq!(f.param_ref_of(args[0]), (poly, 0));
        // The original bounds still name the symbol.
        assert!(
            matches!(f.store.types.get(bounds), Type::Bounds { high, .. } if *high == comparable)
        );
        assert!(matches!(
            f.store.types.get(a_ref),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == a
        ));
    }

    #[test]
    fn an_inner_clause_is_copied_and_rebound_when_the_outer_clause_is_built() {
        // `(x: T)(y: x.type): y.type`, built inner first.
        let mut f = Fixture::new();
        let (x, y) = (SymbolId::new(30), SymbolId::new(31));
        let x_ref = term_ref_to(&mut f, x);
        let inner_param = method_param(&mut f, y, "y", x_ref);
        let y_ref = term_ref_to(&mut f, y);
        let inner =
            method_type_from_symbols(&mut f.store, &[inner_param], y_ref, MethodKind::Plain)
                .unwrap();
        let before = get_method(&f, inner).clone();
        let outer_param = {
            let leaf = f.leaf;
            method_param(&mut f, x, "x", leaf)
        };

        let outer =
            method_type_from_symbols(&mut f.store, &[outer_param], inner, MethodKind::Plain)
                .unwrap();

        // The outer result is a copy of the inner method, not the inner one.
        let copy = get_method(&f, outer).result;
        assert_ne!(copy, inner);
        assert_eq!(
            f.param_ref_of(get_method(&f, copy).params[0].ty),
            (outer, 0)
        );
        assert_eq!(f.param_ref_of(get_method(&f, copy).result), (copy, 0));
        // The stored inner method was not touched.
        assert_eq!(get_method(&f, inner), &before);
        assert_eq!(f.param_ref_of(before.result), (inner, 0));
    }

    #[test]
    fn a_shared_dependent_subgraph_is_transformed_once() {
        let mut f = Fixture::new();
        let a = SymbolId::new(40);
        let a_ref = type_ref_to(&mut f, a);
        let shared = f.applied(&[a_ref]);
        let result = f.store.types.alloc(Type::And {
            left: shared,
            right: shared,
        });
        let spec = {
            let leaf = f.leaf;
            type_param_spec(&mut f, a, "A", leaf)
        };

        let poly = poly_type_from_symbols(&mut f.store, &[spec], result).unwrap();

        let Type::Poly(built) = f.store.types.get(poly).clone() else {
            panic!("not a poly");
        };
        let Type::And { left, right } = f.store.types.get(built.result).clone() else {
            panic!("not an intersection");
        };
        assert_eq!(left, right);
        assert_ne!(left, shared);
    }

    #[test]
    fn an_annotation_that_mentions_a_parameter_is_replaced_not_mutated() {
        let mut f = Fixture::new();
        let a = SymbolId::new(50);
        let a_ref = type_ref_to(&mut f, a);
        let annotation = f.store.annotations.alloc(Annotation::new(a_ref, None));
        let annotated = f.store.types.alloc(Type::Annotated {
            underlying: f.leaf,
            annotation,
        });
        let spec = {
            let leaf = f.leaf;
            type_param_spec(&mut f, a, "A", leaf)
        };

        let poly = poly_type_from_symbols(&mut f.store, &[spec], annotated).unwrap();

        let Type::Poly(built) = f.store.types.get(poly).clone() else {
            panic!("not a poly");
        };
        let Type::Annotated {
            annotation: new, ..
        } = f.store.types.get(built.result).clone()
        else {
            panic!("not annotated");
        };
        assert_ne!(new, annotation);
        assert_eq!(f.param_ref_of(f.store.annotations.get(new).ty), (poly, 0));
        assert_eq!(f.store.annotations.get(annotation).ty, a_ref);
    }

    #[test]
    fn a_type_lambda_from_symbols_is_its_own_binder_with_declared_variance() {
        let mut f = Fixture::new();
        let a = SymbolId::new(60);
        let a_ref = type_ref_to(&mut f, a);
        let mut spec = {
            let leaf = f.leaf;
            type_param_spec(&mut f, a, "A", leaf)
        };
        spec.declared_variance = Some(Variance::Covariant);

        let lambda = type_lambda_from_symbols(&mut f.store, &[spec], a_ref).unwrap();

        let built = f.get_lambda(lambda).clone();
        assert_eq!(built.params[0].declared_variance, Some(Variance::Covariant));
        assert_eq!(f.param_ref_of(built.result), (lambda, 0));
    }

    #[test]
    fn a_parameter_symbol_given_twice_is_refused_and_nothing_is_allocated() {
        let mut f = Fixture::new();
        let x = SymbolId::new(70);
        let first = {
            let leaf = f.leaf;
            method_param(&mut f, x, "x", leaf)
        };
        let second = {
            let leaf = f.leaf;
            method_param(&mut f, x, "y", leaf)
        };
        let before = type_count(&mut f);

        let result =
            method_type_from_symbols(&mut f.store, &[first, second], f.leaf, MethodKind::Plain);

        assert_eq!(
            result,
            Err(TypeRebindError::DuplicateParameterSymbol { symbol: x })
        );
        assert_eq!(type_count(&mut f), before + 1);
    }

    #[test]
    fn a_failure_deep_in_the_graph_rolls_back_the_binder_and_every_copy() {
        let mut f = Fixture::new();
        let x = SymbolId::new(80);
        let x_ref = term_ref_to(&mut f, x);
        // A slot that is reserved and never filled is reachable from the
        // result, after a part that does need copying.
        let unfilled = f.store.types.reserve().id();
        let result = f.store.types.alloc(Type::And {
            left: x_ref,
            right: unfilled,
        });
        let param = {
            let leaf = f.leaf;
            method_param(&mut f, x, "x", leaf)
        };
        let before = type_count(&mut f);

        assert_eq!(
            method_type_from_symbols(&mut f.store, &[param], result, MethodKind::Plain),
            Err(TypeRebindError::UnfilledType { id: unfilled })
        );
        assert_eq!(type_count(&mut f), before + 1);
        // And the same call after the graph is fixed is not affected.
    }

    #[test]
    fn an_empty_parameter_list_is_a_method_type_with_no_parameters() {
        let mut f = Fixture::new();

        let method =
            method_type_from_symbols(&mut f.store, &[], f.leaf, MethodKind::Plain).unwrap();

        assert!(get_method(&f, method).params.is_empty());
        assert_eq!(get_method(&f, method).result, f.leaf);
    }

    // --- close_over_this ---

    fn this_type(f: &mut Fixture, class: SymbolId) -> TypeId {
        f.store.types.alloc(Type::ThisType { class })
    }

    fn get_recursive(f: &Fixture, id: TypeId) -> TypeId {
        match f.store.types.get(id) {
            Type::Recursive { parent } => *parent,
            other => panic!("not recursive: {other:?}"),
        }
    }

    #[test]
    fn no_matching_this_type_returns_the_exact_original_id_and_allocates_nothing() {
        let mut f = Fixture::new();
        let class = SymbolId::new(1);
        let other = SymbolId::new(2);
        let parent = this_type(&mut f, other);
        let before = type_count(&mut f);

        let result = close_over_this(&mut f.store, parent, class).unwrap();

        assert_eq!(result, parent);
        assert_eq!(type_count(&mut f), before + 1);
    }

    #[test]
    fn one_matching_this_type_becomes_recursive_with_one_rec_this() {
        let mut f = Fixture::new();
        let class = SymbolId::new(1);
        let leaf = f.leaf;
        let this = this_type(&mut f, class);
        let name = dotty_core_test_name(&mut f, "T");
        let parent = f.store.types.alloc(Type::Refined {
            parent: leaf,
            name,
            info: this,
        });

        let closed = close_over_this(&mut f.store, parent, class).unwrap();

        assert_ne!(closed, parent);
        let transformed_parent = get_recursive(&f, closed);
        let Type::Refined { info, .. } = f.store.types.get(transformed_parent) else {
            panic!("not refined");
        };
        assert_eq!(f.store.types.get(*info), &Type::RecThis { binder: closed });
        // The original graph still names the class directly.
        assert_eq!(f.store.types.get(this), &Type::ThisType { class });
    }

    #[test]
    fn several_occurrences_share_one_rec_this_type_id() {
        let mut f = Fixture::new();
        let class = SymbolId::new(1);
        let (this_a, this_b) = (this_type(&mut f, class), this_type(&mut f, class));
        let parent = f.store.types.alloc(Type::And {
            left: this_a,
            right: this_b,
        });

        let closed = close_over_this(&mut f.store, parent, class).unwrap();

        let transformed = get_recursive(&f, closed);
        let Type::And { left, right } = f.store.types.get(transformed) else {
            panic!("not an And");
        };
        assert_eq!(left, right);
        assert_eq!(f.store.types.get(*left), &Type::RecThis { binder: closed });
    }

    #[test]
    fn a_this_type_of_another_class_is_left_alone() {
        let mut f = Fixture::new();
        let (class, other) = (SymbolId::new(1), SymbolId::new(2));
        let mine = this_type(&mut f, class);
        let theirs = this_type(&mut f, other);
        let parent = f.store.types.alloc(Type::And {
            left: mine,
            right: theirs,
        });

        let closed = close_over_this(&mut f.store, parent, class).unwrap();

        let transformed = get_recursive(&f, closed);
        let Type::And { left, right } = f.store.types.get(transformed) else {
            panic!("not an And");
        };
        assert_eq!(f.store.types.get(*left), &Type::RecThis { binder: closed });
        assert_eq!(*right, theirs);
    }

    #[test]
    fn an_occurrence_under_applied_bounds_aliasing_bounds_and_annotated_is_substituted() {
        let mut f = Fixture::new();
        let class = SymbolId::new(1);
        let leaf = f.leaf;
        let this_1 = this_type(&mut f, class);
        let applied = f.applied(&[this_1]);
        let this_2 = this_type(&mut f, class);
        let bounds = f.store.types.alloc(Type::Bounds {
            low: leaf,
            high: this_2,
        });
        let this_3 = this_type(&mut f, class);
        let aliasing = f.store.types.alloc(Type::AliasingBounds { alias: this_3 });
        let this_4 = this_type(&mut f, class);
        let annotation = f.store.annotations.alloc(Annotation::new(this_4, None));
        let annotated = f.store.types.alloc(Type::Annotated {
            underlying: leaf,
            annotation,
        });
        let inner = f.store.types.alloc(Type::And {
            left: aliasing,
            right: annotated,
        });
        let middle = f.store.types.alloc(Type::And {
            left: bounds,
            right: inner,
        });
        let parent = f.store.types.alloc(Type::And {
            left: applied,
            right: middle,
        });

        let closed = close_over_this(&mut f.store, parent, class).unwrap();

        // Every reachable copy shares the one canonical RecThis.
        let mut seen = HashSet::new();
        let mut stack = vec![get_recursive(&f, closed)];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            match f.store.types.get(id).clone() {
                Type::RecThis { binder } => assert_eq!(binder, closed),
                Type::And { left, right } => stack.extend([left, right]),
                Type::Applied { tycon, args } => {
                    stack.push(tycon);
                    stack.extend(args);
                }
                Type::Bounds { low, high } => stack.extend([low, high]),
                Type::AliasingBounds { alias } => stack.push(alias),
                Type::Annotated { underlying, .. } => stack.push(underlying),
                _ => {}
            }
        }
        assert!(
            seen.iter()
                .any(|id| matches!(f.store.types.get(*id), Type::RecThis { .. }))
        );
    }

    #[test]
    fn an_occurrence_inside_a_nested_method_poly_or_type_lambda_keeps_its_own_binder() {
        // `[A] => (x: this.type): this.type`, over a Method inside a fresh
        // TypeLambda: the ThisType is substituted and the lambda's own
        // ParamRef is untouched.
        let mut f = Fixture::new();
        let class = SymbolId::new(1);
        let lambda = f.lambda(&["A"], |f, me| {
            let a = f.param_ref(me, 0);
            let method = f.store.types.reserve();
            let method_id = method.id();
            let this = this_type(f, class);
            f.store.types.fill(
                method,
                Type::Method(MethodType {
                    params: vec![MethodParam {
                        name: TermName::new(f.store.names.intern("x")),
                        ty: this,
                        erased: false,
                        varargs: false,
                    }],
                    result: a,
                    kind: MethodKind::Plain,
                }),
            );
            method_id
        });

        let closed = close_over_this(&mut f.store, lambda, class).unwrap();

        let transformed = get_recursive(&f, closed);
        let Type::TypeLambda(outer) = f.store.types.get(transformed) else {
            panic!("not a lambda");
        };
        let Type::Method(method) = f.store.types.get(outer.result) else {
            panic!("not a method");
        };
        assert_eq!(
            f.store.types.get(method.params[0].ty),
            &Type::RecThis { binder: closed }
        );
        // The lambda's own binder is preserved: its result still names it.
        assert_eq!(f.param_ref_of(method.result), (transformed, 0));
    }

    #[test]
    fn an_occurrence_inside_an_existing_recursive_type_does_not_disturb_its_own_rec_this() {
        // `Recursive { inner -> Refined("T", ThisType(class)) }`: the inner
        // Recursive is copied, its own RecThis stays naming the copy, and the
        // ThisType(class) becomes a RecThis of the *outer* binder.
        let mut f = Fixture::new();
        let class = SymbolId::new(1);
        let leaf = f.leaf;
        let inner_reserved = f.store.types.reserve();
        let inner_old = inner_reserved.id();
        let inner_this = f.store.types.alloc(Type::RecThis { binder: inner_old });
        let outer_this = this_type(&mut f, class);
        let info = f.store.types.alloc(Type::And {
            left: inner_this,
            right: outer_this,
        });
        let name = dotty_core_test_name(&mut f, "T");
        let refined = f.store.types.alloc(Type::Refined {
            parent: leaf,
            name,
            info,
        });
        f.store
            .types
            .fill(inner_reserved, Type::Recursive { parent: refined });

        let closed = close_over_this(&mut f.store, inner_old, class).unwrap();

        assert_ne!(closed, inner_old);
        let outer_parent = get_recursive(&f, closed);
        let Type::Recursive {
            parent: inner_new_parent,
        } = f.store.types.get(outer_parent)
        else {
            panic!("not recursive");
        };
        let Type::Refined { info, .. } = f.store.types.get(*inner_new_parent) else {
            panic!("not refined");
        };
        let Type::And { left, right } = f.store.types.get(*info) else {
            panic!("not an And");
        };
        // The inner RecThis was copied to name the copied inner Recursive.
        assert_eq!(
            f.store.types.get(*left),
            &Type::RecThis {
                binder: outer_parent
            }
        );
        // The outer ThisType became a RecThis of the new outer binder.
        assert_eq!(f.store.types.get(*right), &Type::RecThis { binder: closed });
        assert_ne!(outer_parent, closed);
    }

    #[test]
    fn sharing_is_preserved_between_two_occurrences_reaching_the_same_dependent_node() {
        let mut f = Fixture::new();
        let class = SymbolId::new(1);
        let this = this_type(&mut f, class);
        let shared = f.applied(&[this]);
        let parent = f.store.types.alloc(Type::And {
            left: shared,
            right: shared,
        });

        let closed = close_over_this(&mut f.store, parent, class).unwrap();

        let transformed = get_recursive(&f, closed);
        let Type::And { left, right } = f.store.types.get(transformed) else {
            panic!("not an And");
        };
        assert_eq!(left, right);
    }

    #[test]
    fn the_original_graph_is_never_mutated() {
        let mut f = Fixture::new();
        let class = SymbolId::new(1);
        let this = this_type(&mut f, class);
        let leaf = f.leaf;
        let name = dotty_core_test_name(&mut f, "T");
        let parent = f.store.types.alloc(Type::Refined {
            parent: leaf,
            name,
            info: this,
        });
        let before = f.store.types.get(parent).clone();

        let closed = close_over_this(&mut f.store, parent, class).unwrap();

        assert_ne!(closed, parent);
        assert_eq!(f.store.types.get(parent), &before);
        assert_eq!(f.store.types.get(this), &Type::ThisType { class });
    }

    #[test]
    fn a_cyclic_graph_is_refused_not_looped_on() {
        // `root -> And(this, root)`: root reaches itself through the right
        // branch, with a `ThisType` in the left branch so the reachability
        // scan (which tolerates a cycle) still finds a match and the
        // transforming pass is entered.
        let mut f = Fixture::new();
        let class = SymbolId::new(1);
        let reserved = f.store.types.reserve();
        let root = reserved.id();
        let this = this_type(&mut f, class);
        f.store.types.fill(
            reserved,
            Type::And {
                left: this,
                right: root,
            },
        );

        assert_eq!(
            close_over_this(&mut f.store, root, class),
            Err(TypeRebindError::CyclicType { id: root })
        );
    }

    #[test]
    fn an_unfilled_source_is_refused_and_nothing_is_allocated() {
        let mut f = Fixture::new();
        let class = SymbolId::new(1);
        let hole = f.store.types.reserve().id();
        let before = type_count(&mut f);

        assert_eq!(
            close_over_this(&mut f.store, hole, class),
            Err(TypeRebindError::UnfilledType { id: hole })
        );
        assert_eq!(type_count(&mut f), before + 1);
    }

    #[test]
    fn a_failure_part_way_leaves_no_allocation_behind() {
        let mut f = Fixture::new();
        let class = SymbolId::new(1);
        let this = this_type(&mut f, class);
        let unfilled = f.store.types.reserve().id();
        let parent = f.store.types.alloc(Type::And {
            left: this,
            right: unfilled,
        });
        let before = type_count(&mut f);

        assert_eq!(
            close_over_this(&mut f.store, parent, class),
            Err(TypeRebindError::UnfilledType { id: unfilled })
        );
        assert_eq!(type_count(&mut f), before + 1);
    }

    fn dotty_core_test_name(f: &mut Fixture, text: &str) -> Name {
        Name::new(f.store.names.intern(text), Namespace::Type)
    }
}
